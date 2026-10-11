//! 需要你注意时发系统通知。
//!
//! 灯是**被动**的：你不盯着屏幕它就白搭，而「不用看终端」恰恰意味着你可能
//! 根本没在看灯。通知补上这半边 —— 把「你得看一眼」变成「它来叫你」。

use crate::config::Settings;
use crate::state::{Signal, Snapshot};
use tauri_plugin_notification::NotificationExt;

/// 只在聚合**新变成**需要注意的状态时通知一次。
///
/// 不是每次状态变化都弹：一轮工具调用会触发几十次状态推送，每次都弹就成了
/// 骚扰。`previous == Some(current)` 这个判断是这条逻辑的核心。
pub fn should_alert(previous: Option<Signal>, current: Signal) -> bool {
    previous != Some(current)
        && matches!(
            current,
            Signal::Permission | Signal::Attention | Signal::Error
        )
}

/// 通知正文：胜出会话的项目名 + 它自己的说明。
///
/// `sessions` 已按优先级排序，所以第一个就是决定聚合的那一路。
fn body(snapshot: &Snapshot) -> String {
    let Some(session) = snapshot.sessions.first() else {
        return String::new();
    };

    let project = session.cwd.as_deref().map(project_name).unwrap_or_default();
    match (project.is_empty(), session.message.as_deref()) {
        (false, Some(message)) => format!("{project} · {message}"),
        (false, None) => project,
        (true, Some(message)) => message.to_string(),
        (true, None) => String::new(),
    }
}

/// 取路径最后一段。两种分隔符都认，免得 Windows 上写的路径在别处解析错。
fn project_name(cwd: &str) -> String {
    cwd.rsplit(['/', '\\'])
        .find(|segment| !segment.is_empty())
        .unwrap_or(cwd)
        .to_string()
}

/// 发一条通知。任何失败只记 stderr —— 通知发不出去不该影响状态机。
pub fn send(app: &tauri::AppHandle, snapshot: &Snapshot) {
    send_message(app, snapshot.aggregate.label(), &body(snapshot));
}

/// 发一条自定义文案的通知。状态通知之外的通告用它（例如「已进入穿透模式」）。
pub fn send_message(app: &tauri::AppHandle, title: &str, body: &str) {
    let result = app
        .notification()
        .builder()
        .title(title)
        .body(body)
        .show();

    if let Err(err) = result {
        eprintln!("[signalhub] 发送通知失败: {err}");
    }
}

/// 首次启动时申请通知权限。
///
/// 放在启动时而不是「第一次要发通知时」，是因为弹权限框和发通知没法同时发生
/// —— 那样第一条通知必然丢掉，而第一条往往正是最要紧的那条。
pub fn ensure_permission(app: &tauri::AppHandle, settings: Settings) {
    if !settings.notify {
        // 用户关掉了通知就完全不要打扰他，连权限框也不要弹
        return;
    }

    let notification = app.notification();
    match notification.permission_state() {
        Ok(tauri_plugin_notification::PermissionState::Granted) => {}
        Ok(_) => {
            if let Err(err) = notification.request_permission() {
                eprintln!("[signalhub] 申请通知权限失败: {err}");
            }
        }
        Err(err) => eprintln!("[signalhub] 读取通知权限失败: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::SessionView;

    fn session(id: &str, state: Signal, cwd: Option<&str>, message: Option<&str>) -> SessionView {
        SessionView {
            id: id.to_string(),
            state,
            agent: None,
            cwd: cwd.map(str::to_string),
            message: message.map(str::to_string),
            age_ms: 0,
            stale_progress: None,
        }
    }

    fn snapshot(aggregate: Signal, sessions: Vec<SessionView>) -> Snapshot {
        Snapshot { aggregate, paused: false, since_ms: 0, message: None, sessions }
    }

    #[test]
    fn only_the_transition_into_an_urgent_state_alerts() {
        // 新变成需要注意的状态 → 弹
        assert!(should_alert(Some(Signal::Working), Signal::Permission));
        assert!(should_alert(Some(Signal::Idle), Signal::Attention));
        assert!(should_alert(Some(Signal::Working), Signal::Error));
        // 首次拿到状态就是需要注意的 → 也弹
        assert!(should_alert(None, Signal::Permission));
    }

    /// 这条是核心：一轮工具调用会触发几十次状态推送，每次都弹就是骚扰
    #[test]
    fn staying_in_an_urgent_state_does_not_alert_again() {
        assert!(!should_alert(Some(Signal::Permission), Signal::Permission));
        assert!(!should_alert(Some(Signal::Attention), Signal::Attention));
        assert!(!should_alert(Some(Signal::Error), Signal::Error));
    }

    #[test]
    fn ordinary_states_never_alert() {
        for state in [
            Signal::Offline,
            Signal::Off,
            Signal::Stale,
            Signal::Idle,
            Signal::Done,
            Signal::Thinking,
            Signal::Working,
        ] {
            assert!(!should_alert(None, state), "{state:?} 不该弹通知");
            assert!(!should_alert(Some(Signal::Permission), state), "{state:?} 不该弹通知");
        }
    }

    #[test]
    fn leaving_an_urgent_state_does_not_alert() {
        assert!(!should_alert(Some(Signal::Permission), Signal::Working));
        assert!(!should_alert(Some(Signal::Error), Signal::Idle));
    }

    #[test]
    fn the_body_names_the_project_and_the_agents_own_words() {
        let snapshot = snapshot(
            Signal::Permission,
            vec![session("a", Signal::Permission, Some("/Users/me/proj"), Some("需要批准: rm -rf build"))],
        );
        assert_eq!(body(&snapshot), "proj · 需要批准: rm -rf build");
    }

    #[test]
    fn the_body_degrades_gracefully() {
        let with = |cwd: Option<&str>, message: Option<&str>| {
            body(&snapshot(
                Signal::Attention,
                vec![session("a", Signal::Attention, cwd, message)],
            ))
        };
        assert_eq!(with(Some("/srv/build"), None), "build");
        assert_eq!(with(None, Some("agent needs input")), "agent needs input");
        assert_eq!(with(None, None), "");
        // 没有会话时不该 panic
        assert_eq!(body(&snapshot(Signal::Offline, vec![])), "");
    }

    #[test]
    fn project_name_handles_both_separators_and_trailing_slashes() {
        assert_eq!(project_name("/Users/me/proj"), "proj");
        assert_eq!(project_name("/Users/me/proj/"), "proj");
        assert_eq!(project_name(r"C:\work\api"), "api");
        assert_eq!(project_name("proj"), "proj");
    }
}
