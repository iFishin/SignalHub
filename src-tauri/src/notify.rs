use crate::alerts;
use crate::outbound;
use crate::state::{Signal, Snapshot};
use crate::tray;
use crate::{SharedPalette, SharedSettings, SharedStats, SharedStore};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
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
    coalesce(compose_now(app))
}

/// 合帧窗口。见 [`coalesce`]。
///
/// 100ms 的来由：一轮里并行发起几个工具调用是常态，它们之间只隔几毫秒；而人眼对
/// 「灯变了」的分辨率就在 100ms 上下 —— 比这更短没有任何人能看出来，更长就开始
/// 显得迟钝。
const COALESCE_MS: u64 = 100;

/// 把 100ms 内的多次推送**合成一帧**（尾沿触发）。
///
/// 为什么值得：上面那段「每次变化都要做六件事」里，有序列化快照、IPC 到两个 webview、
/// 前端整棵树的重渲染、各组灯的核对、统计记录 —— 一轮并行工具调用会在几十毫秒里
/// 推好几帧，而那些中间帧**人根本看不到**（灯上的说明文字在这一轮里本来就一直在变）。
///
/// **尾沿而不是首沿**：首沿会让状态变化晚 100ms 才出现在灯上，而尾沿把「最后那一帧」
/// 送出去 —— 也就是真正该被看见的那一帧。中间帧被丢掉是安全的，因为每一帧都是
/// **整份快照**（不是增量），丢掉中间帧不会丢状态。
///
/// 送出去的保证：`take()` 拿的永远是最新的那一份，而旗子先放下、再取快照（反过来
/// 会丢掉恰好在这两步之间到达的那一次 —— 那时旗子还是「已有一帧在路上」，没人再来取）。
fn coalesce(inner: Notifier) -> Notifier {
    let pending: Arc<Mutex<Option<Snapshot>>> = Arc::new(Mutex::new(None));
    let scheduled = Arc::new(AtomicBool::new(false));

    Arc::new(move |snapshot: &Snapshot| {
        *pending.lock().expect("pending poisoned") = Some(snapshot.clone());

        // 已经有一帧在路上：它会把这一份一起带走，直接返回
        if scheduled.swap(true, Ordering::SeqCst) {
            return;
        }

        let pending = pending.clone();
        let scheduled = scheduled.clone();
        let inner = inner.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_millis(COALESCE_MS)).await;
            scheduled.store(false, Ordering::SeqCst);
            let latest = pending.lock().expect("pending poisoned").take();
            if let Some(latest) = latest {
                inner(&latest);
            }
        });
    })
}

/// 真正干活的那一版：每一次推送都把那六件事全做一遍。
fn compose_now(app: tauri::AppHandle) -> Notifier {
    // 钩子只看**全局**聚合的跃迁：钩子是用户显式配置的程序化集成，不是「打扰」，
    // 所以静音一组不该让它的钩子失效。
    let last_aggregate: Arc<Mutex<Option<Signal>>> = Arc::new(Mutex::new(None));
    // 通知另有一套判断：排除静音组、要够「持续」、还要避开勿扰时段。
    let gate: Arc<Mutex<alerts::Gate>> = Arc::new(Mutex::new(alerts::Gate::default()));
    // 上一次各组的聚合值，用来算「哪个组跃迁了」—— 限定了组的出站钩子看它。
    let last_groups: Arc<Mutex<HashMap<String, Signal>>> = Arc::new(Mutex::new(HashMap::new()));

    Arc::new(move |snapshot: &Snapshot| {
        // **克隆的是 `Arc`，不是整份设置**（见 `SharedSettings`）：这条闭包在每一次
        // 状态推送里跑，一秒可能好几次。`.clone()` 通过 `MutexGuard` 解引用到里层
        // 的 `Arc<Settings>`，只是一次指针拷贝。
        let settings = app
            .state::<SharedSettings>()
            .lock()
            .expect("settings poisoned")
            .clone();
        crate::sync_layout(&app, &settings, snapshot);

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
                &settings,
                snapshot,
                transitioned,
                &group_changes,
                crate::current_lang(&app),
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
    let quiet = is_quiet_now(app);

    let threshold = Duration::from_secs(settings.alert_after_secs as u64);
    // 决定和轮次**在同一次加锁里取**：分两次的话，中间可能被另一次状态推送插进来，
    // 于是这里抓到的会是新一轮的编号、而决定属于上一轮 —— 那正好让轮次判断失效。
    let (decision, episode) = {
        let mut gate = gate.lock().expect("gate poisoned");
        let decision = gate.observe(alerting, threshold, quiet);
        (decision, gate.episode())
    };

    match decision {
        alerts::Decision::None => {}
        alerts::Decision::Now => {
            alerts::send(app, snapshot, settings.sound, muted);
            spawn_reminders(app.clone(), gate.clone(), muted.clone(), alerting, episode);
        }
        alerts::Decision::Later => {
            let app = app.clone();
            let gate = gate.clone();
            let muted = muted.clone();
            let sound = settings.sound;

            // 延迟到点后重新判，**判据全部从快照现读**，不看排任务时捕获的值：
            // 状态可能已经变了，或者离开又回来变成了一个同名的「新」状态。
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(threshold).await;

                // 还是排任务时那一轮吗？状态离开又回来（哪怕回到同一个值）已经是
                // 新的一轮了 —— 拿旧一轮的检查去替它弹，等于跳过了它自己的阈值。
                if gate.lock().expect("gate poisoned").episode() != episode {
                    return;
                }

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
                spawn_reminders(app.clone(), gate.clone(), muted.clone(), alerting, episode);
            });
        }
    }
}

/// 现在是不是勿扰时段。
///
/// 前端还没推过时间就当作「不勿扰」：宁可多弹一条，也不要在用户不知情时把通知静掉
/// —— 那会表现为「通知功能坏了」，是最难归因的一类问题。
///
/// 每步重算而不是沿用第一次通知时的判断：等待期间用户可能改了时段设置。
fn is_quiet_now(app: &tauri::AppHandle) -> bool {
    let settings = app
        .state::<SharedSettings>()
        .lock()
        .expect("settings poisoned")
        .clone();
    match app.state::<alerts::LocalHour>().get() {
        Some(hour) => settings.quiet_hours.covers(hour),
        None => false,
    }
}

/// 排一条重复提醒的链：每到一个间隔就重判一次，该提就提，然后等下一个间隔。
///
/// **每一步都要重判，不能一路睡到底。** 这中间状态可能已经过去了、用户可能把通知
/// 关了、也可能进了勿扰时段 —— 链上任何一步发现不该提，就整条停掉。
///
/// 判据全部现读（快照、设置、本地时间），不看排链时捕获的值：`sound` 这类设置用户
/// 随时可能改，用旧值就是在替他做决定。
fn spawn_reminders(
    app: tauri::AppHandle,
    gate: Arc<Mutex<alerts::Gate>>,
    muted: Vec<String>,
    alerting: Signal,
    episode: u64,
) {
    tauri::async_runtime::spawn(async move {
        for gap in alerts::REMIND_GAPS {
            tokio::time::sleep(gap).await;

            // 还是同一轮吗？状态离开又回来是新的一轮，上一轮排下的提醒必须失效 ——
            // 否则会替新一轮发一条「已等待 20 分钟」，而它才刚开始。
            if gate.lock().expect("gate poisoned").episode() != episode {
                return;
            }

            let settings = app
                .state::<SharedSettings>()
                .lock()
                .expect("settings poisoned")
                .clone();
            if !settings.notify {
                return; // 用户把通知关了
            }
            // 进了勿扰时段就停，**不延后补发** —— 和第一次通知同一条理由：早上醒来
            // 面对一夜攒下的通知，正是勿扰想避免的。
            if is_quiet_now(&app) {
                return;
            }

            let snapshot = app
                .state::<SharedStore>()
                .lock()
                .expect("state store poisoned")
                .snapshot();
            // 事情过去了：状态变了、被静音了、或者暂停了
            if alerts::alerting_aggregate(&snapshot, &muted) != alerting {
                return;
            }

            alerts::send_reminder(&app, &snapshot, settings.sound, &muted);
        }
    });
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
///
/// **两个窗口都要推。** 这里原来只发 `main`，而设置窗（`visible: false` 也照样在
/// 启动时就把 webview 加载起来）只在挂载时 `get_state` 拉过一次 —— 于是打开设置
/// 面板看到的永远是**应用启动那一刻**的会话和时间线。
///
/// 不按「设置窗是否可见」挑着发，是因为那要问窗口，而 `is_visible()` 在 macOS 上
/// 是一次**阻塞的主线程往返**（tauri-runtime-wry 的 `window_getter!`），而这条
/// 推送路径是从 HTTP 上报线程和 sweep 任务里走的 —— 往热路径上加同步调用不值当。
/// 隐藏的设置窗多渲染几次只是它自己的开销，不会拖慢上报。
fn emit(app: &tauri::AppHandle, snapshot: &Snapshot) {
    if let Err(err) = app.emit(STATE_EVENT, snapshot) {
        eprintln!("[signalhub] 推送状态失败: {err}");
    }
}

/// 给「手上已有 AppHandle」的命令和托盘用。
pub fn push(app: &tauri::AppHandle, snapshot: &Snapshot) {
    (app.state::<Notifier>().inner())(snapshot);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Store;

    fn snapshot(aggregate: Signal) -> Snapshot {
        Snapshot {
            aggregate,
            ..Store::default().snapshot()
        }
    }

    /// 记账用的内层通知器：把每一次真的推出去的聚合值记下来。
    fn recorder() -> (Notifier, Arc<Mutex<Vec<Signal>>>) {
        let seen: Arc<Mutex<Vec<Signal>>> = Arc::new(Mutex::new(Vec::new()));
        let handle = seen.clone();
        let inner: Notifier = Arc::new(move |snapshot: &Snapshot| {
            handle.lock().expect("seen poisoned").push(snapshot.aggregate);
        });
        (inner, seen)
    }

    /// 一轮并行工具调用会在几十毫秒里推好几帧 —— 它们合成一帧，而且**是最后那一帧**。
    ///
    /// 「最后那一帧」是这个用例真正要钉住的东西：合帧的实现只要稍微写歪一点（比如
    /// 送出去的是第一帧），灯上就会停在一个**已经过时的**状态上，而这件事在界面上
    /// 完全看不出来（它就是一个合法的旧状态）。
    #[tokio::test]
    async fn a_burst_becomes_one_frame_and_it_is_the_last_one() {
        let (inner, seen) = recorder();
        let notify = coalesce(inner);

        notify(&snapshot(Signal::Idle));
        notify(&snapshot(Signal::Working));
        notify(&snapshot(Signal::Done));

        assert!(seen.lock().expect("seen poisoned").is_empty(), "还在窗口里，一帧都还没推");
        tokio::time::sleep(Duration::from_millis(COALESCE_MS * 3)).await;

        let seen = seen.lock().expect("seen poisoned").clone();
        assert_eq!(seen, vec![Signal::Done], "三帧该合成一帧，且是最后那一帧");
    }

    /// **不许丢帧**：窗口过后的下一次推送必须照样出去。
    ///
    /// 这条防的是那个很自然的写法 —— 「取快照」和「放下旗子」顺序反了的话，恰好
    /// 卡在这两步之间的那一次会被判成「已有一帧在路上」，于是谁也不来取它，
    /// 灯就永远停在上一帧上。
    #[tokio::test]
    async fn a_push_after_the_window_is_delivered_too() {
        let (inner, seen) = recorder();
        let notify = coalesce(inner);

        notify(&snapshot(Signal::Working));
        tokio::time::sleep(Duration::from_millis(COALESCE_MS * 3)).await;
        notify(&snapshot(Signal::Error));
        tokio::time::sleep(Duration::from_millis(COALESCE_MS * 3)).await;

        let seen = seen.lock().expect("seen poisoned").clone();
        assert_eq!(seen, vec![Signal::Working, Signal::Error]);
    }
}
