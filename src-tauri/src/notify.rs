use crate::state::Snapshot;
use std::sync::Arc;

/// 推给灯球窗口的事件名。前端 `src/lib/useSignal.ts` 里的字面量必须与之一致。
pub const STATE_EVENT: &str = "signalhub://state";

/// 状态变化时的通知出口。
///
/// 抽成 trait object 而不是直接持有 `AppHandle`，是为了让 HTTP 层不必构造
/// Tauri 应用就能单测 —— `AppHandle` 在测试里是造不出来的。
pub type Notifier = Arc<dyn Fn(&Snapshot) + Send + Sync>;

/// 生产实现：把快照发给灯球窗口。
pub fn to_lamp_window(app: tauri::AppHandle) -> Notifier {
    use tauri::Emitter;
    Arc::new(move |snapshot| {
        // 窗口可能尚未创建或已被关闭，不该让调用方 panic
        if let Err(err) = app.emit_to("main", STATE_EVENT, snapshot) {
            eprintln!("[signalhub] 推送状态失败: {err}");
        }
    })
}

/// 给「手上已有 AppHandle」的命令和托盘用。
/// notifier 存在 Tauri state 里，与 sweep 任务共用同一个实例。
pub fn push(app: &tauri::AppHandle, snapshot: &Snapshot) {
    use tauri::Manager;
    (app.state::<Notifier>().inner())(snapshot);
}
