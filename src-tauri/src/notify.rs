use crate::alerts;
use crate::outbound;
use crate::{SharedSettings, SharedStats};
use crate::state::{Signal, Snapshot};
use std::sync::{Arc, Mutex};
use std::time::Instant;
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
/// 每次状态变化都要做四件事，缺一不可：
///   1. 按可见圆点个数核对窗口尺寸
///   2. 记进今日时长统计
///   3. 需要你注意时发系统通知、并转发到 webhook
///   4. 推快照给灯球窗口
///
/// 这个函数是唯一的构造入口，是刻意的：早先 HTTP 那条路径自己构造了一个
/// 「只推事件」的版本，于是 1–3 全部被绕过 —— 而智能体的 hook 恰好全走
/// HTTP。表现是「通知和 webhook 从来不发」，但两边都不报错，极难发现。
/// 只有一个入口就不会再出现这种事。
pub fn compose(app: tauri::AppHandle) -> Notifier {
    let last_aggregate: Arc<Mutex<Option<Signal>>> = Arc::new(Mutex::new(None));

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

        // 通知和出站钩子都要知道「聚合是不是真的变了」，所以要记住上一次的值。
        // previous 无论是否真的发出去都要更新 —— 否则用户重新打开开关时会立刻
        // 收到一条关于「早就发生过的」状态的通知。
        let (transitioned, alerting) = {
            let mut previous = last_aggregate.lock().expect("alert state poisoned");
            let transitioned = *previous != Some(snapshot.aggregate);
            let alerting = alerts::should_alert(*previous, snapshot.aggregate);
            *previous = Some(snapshot.aggregate);
            (transitioned, alerting)
        };

        // 两件事都在锁外做：发通知和跑钩子都不该占着状态
        if transitioned {
            // 钩子自带过滤条件（`when`），所以只按「聚合跃迁了」触发，
            // 不像通知那样限定在需要注意的状态 —— 用户可以配「done 时跑个脚本」。
            outbound::fire(&settings.hooks, snapshot);
        }
        if alerting && settings.notify {
            alerts::send(&app, snapshot);
        }

        emit(&app, snapshot);
    })
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
