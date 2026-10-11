use crate::alerts;
use crate::outbound;
use crate::state::{Signal, Snapshot};
use crate::tray;
use crate::{SharedPalette, SharedSettings, SharedStats, SharedStore};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager};

/// 推给灯球窗口的事件名。前端 `src/lib/useSignal.ts` 里的字面量必须与之一致。
pub const STATE_EVENT: &str = "signalhub://state";

/// 状态变化时的通知出口。
///
/// 抽成 trait object 而不是直接持有 `AppHandle`，是为了让 HTTP 层不必构造
/// Tauri 应用就能单测 —— `AppHandle` 在测试里是造不出来的。
pub type Notifier = Arc<dyn Fn(&Snapshot) + Send + Sync>;

/// **唯一**的状态通知入口。
///
/// 每次状态变化都要做这几件事，缺一不可：
///   1. 按可见牌的数量核对窗口尺寸
///   2. 记进今日时长统计
///   3. 让托盘图标跟上状态
///   4. 聚合跃迁时跑出站钩子
///   5. 该打扰你的时候发系统通知
///   6. 推快照给灯球窗口
///
/// 这个函数是唯一的构造入口，是刻意的：早先 HTTP 那条路径自己构造了一个
/// 「只推事件」的版本，于是 1–5 全部被绕过 —— 而智能体的 hook 恰好全走
/// HTTP。表现是「通知和 webhook 从来不发」，但两边都不报错，极难发现。
/// 只有一个入口就不会再出现这种事。
pub fn compose(app: tauri::AppHandle) -> Notifier {
    // 钩子只看**全局**聚合的跃迁：钩子是用户显式配置的程序化集成，不是「打扰」，
    // 所以静音一组不该让它的钩子失效。
    let last_aggregate: Arc<Mutex<Option<Signal>>> = Arc::new(Mutex::new(None));
    // 通知另有一套判断：排除静音组、要够「持续」、还要避开勿扰时段。
    let gate: Arc<Mutex<alerts::Gate>> = Arc::new(Mutex::new(alerts::Gate::default()));
    // 上一次各组的聚合值，用来算「哪个组跃迁了」—— 限定了组的出站钩子看它。
    let last_groups: Arc<Mutex<HashMap<String, Signal>>> = Arc::new(Mutex::new(HashMap::new()));

    Arc::new(move |snapshot: &Snapshot| {
        let settings = app
            .state::<SharedSettings>()
            .lock()
            .expect("settings poisoned")
            .clone();
        crate::apply_window_size(&app, &settings, snapshot);

        app.state::<SharedStats>()
            .lock()
            .expect("stats poisoned")
            .record(Instant::now(), snapshot.aggregate);

        // previous 无论是否真的发出去都要更新 —— 否则用户重新打开开关时会立刻
        // 收到一条关于「早就发生过的」状态的通知。
        let transitioned = {
            let mut previous = last_aggregate.lock().expect("alert state poisoned");
            let transitioned = *previous != Some(snapshot.aggregate);
            *previous = Some(snapshot.aggregate);
            transitioned
        };

        // 只在真的跃迁时才重画托盘图标：一轮工具调用会推几十次状态，
        // 每次重画图标是无谓的系统调用。
        if transitioned {
            let colors = app
                .state::<SharedPalette>()
                .lock()
                .expect("palette poisoned")
                .clone();
            tray::sync(&app, snapshot.aggregate, settings.tray_follows_state, &colors);
        }

        // 每个组**各自**的跃迁。限定了组的钩子看这个（见 `config::Hook::fires`）——
        // 只看全局聚合的话，「ci 组失败了」这件事会在别的组更紧急时被整个吞掉。
        let group_changes = {
            let mut previous = last_groups.lock().expect("group state poisoned");
            let mut changes = Vec::new();
            for group in &snapshot.groups {
                if previous.get(&group.name) != Some(&group.aggregate) {
                    changes.push((group.name.clone(), group.aggregate));
                }
            }
            // 整份替换而不是逐个更新：已经消失的组要跟着删掉，否则它下次出现时
            // 会被当成「没变」，那一次跃迁就丢了。
            previous.clear();
            for group in &snapshot.groups {
                previous.insert(group.name.clone(), group.aggregate);
            }
            changes
        };

        // 钩子自带过滤条件（`when`），所以只按「有东西跃迁了」触发，
        // 不像通知那样限定在需要注意的状态 —— 用户可以配「done 时跑个脚本」。
        if transitioned || !group_changes.is_empty() {
            outbound::fire(
                &settings.hooks,
                snapshot,
                transitioned,
                &group_changes,
                app.state::<crate::SharedOutbound>().inner(),
            );
        }

        if settings.notify {
            decide_alert(&app, &gate, &settings, snapshot);
        }

        emit(&app, snapshot);
    })
}

/// 通知的决策。
///
/// 与钩子分开：钩子看的是**全局**聚合的跃迁，而通知要看的是「该不该打扰你」——
/// 排除静音组、要够持续、还要避开勿扰时段。
fn decide_alert(
    app: &tauri::AppHandle,
    gate: &Arc<Mutex<alerts::Gate>>,
    settings: &crate::config::Settings,
    snapshot: &Snapshot,
) {
    let muted = &settings.muted_groups;
    let alerting = alerts::alerting_aggregate(snapshot, muted);

    // 前端还没推过时间就当作「不勿扰」：宁可多弹一条，也不要在用户不知情时
    // 把通知静掉 —— 那会表现为「通知功能坏了」，是最难归因的一类问题。
    let quiet = match app.state::<alerts::LocalHour>().get() {
        Some(hour) => settings.quiet_hours.covers(hour),
        None => false,
    };

    let threshold = Duration::from_secs(settings.alert_after_secs as u64);
    let decision = gate
        .lock()
        .expect("gate poisoned")
        .observe(alerting, threshold, quiet);

    match decision {
        alerts::Decision::None => {}
        alerts::Decision::Now => alerts::send(app, snapshot, settings.sound, muted),
        alerts::Decision::Later => {
            let app = app.clone();
            let gate = gate.clone();
            let muted = muted.clone();
            let sound = settings.sound;

            // 延迟到点后重新判，**判据全部从快照现读**，不看排任务时捕获的值：
            // 状态可能已经变了，或者离开又回来变成了一个同名的「新」状态。
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(threshold).await;

                let snapshot = app
                    .state::<SharedStore>()
                    .lock()
                    .expect("state store poisoned")
                    .snapshot();
                if alerts::alerting_aggregate(&snapshot, &muted) != alerting {
                    return; // 已经不是那个状态了
                }
                if !sustained(&snapshot, &muted, alerting, threshold) {
                    // 还是那个状态，但没真的持续够久 —— 这正是这个阈值存在的
                    // 意义：智能体经常在一秒内进出 permission，立刻弹的话用户
                    // 被叫过去看到的是一个已经解决的问题。
                    return;
                }
                // 最后抢一下「这个 episode 已经通知过」的名额。
                if !gate.lock().expect("gate poisoned").confirm(alerting) {
                    return;
                }
                alerts::send(&app, &snapshot, sound, &muted);
            });
        }
    }
}

/// 这个聚合值真的已经持续够久了吗。
///
/// 从快照里读，而不是自己记一个「什么时候开始的」：`GroupView::since_ms` 就是
/// 「这一组进入当前状态的时刻」，是唯一的事实来源。自己记一份的话，状态离开又
/// 回来时那份记录会说谎，而且还得再写一套失效逻辑。
///
/// 用「任一组成立」而不是「最早的那组」：几组同时处于同一个紧急状态时，只要
/// 有一组已经卡了很久，这个状态就该算数。
fn sustained(snapshot: &Snapshot, muted: &[String], state: Signal, threshold: Duration) -> bool {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let threshold_ms = threshold.as_millis() as u64;

    snapshot
        .groups
        .iter()
        .filter(|group| !muted.iter().any(|name| name == &group.name))
        .filter(|group| group.aggregate == state)
        .any(|group| now_ms.saturating_sub(group.since_ms) >= threshold_ms)
}

/// 只负责把快照推给前端窗口。窗口尺寸、统计、通知都由 `compose` 里的闭包管。
fn emit(app: &tauri::AppHandle, snapshot: &Snapshot) {
    if let Err(err) = app.emit_to("main", STATE_EVENT, snapshot) {
        eprintln!("[signalhub] 推送状态失败: {err}");
    }
}

/// 给「手上已有 AppHandle」的命令和托盘用。
pub fn push(app: &tauri::AppHandle, snapshot: &Snapshot) {
    (app.state::<Notifier>().inner())(snapshot);
}
