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
///
/// **系统通知和出站转发共用这一个判断** —— 否则两处的触发时机迟早会分叉。
pub fn should_alert(previous: Option<Signal>, current: Signal) -> bool {
    previous != Some(current)
        && matches!(
            current,
            Signal::Permission | Signal::Attention | Signal::Error
        )
}

/// 通知正文：胜出会话的项目名 + 它自己的说明。
///
/// 与分组无关 —— 取全局优先级最高的那个会话，也就是决定 `aggregate` 的那一路。
fn body(snapshot: &Snapshot) -> String {
    let Some(session) = snapshot
        .all_sessions()
        .max_by_key(|session| session.state.priority())
    else {
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

/// 系统「默认提示音」的名字。**没有三平台通吃的写法**，只能逐平台取值：
///
/// - macOS 认这个哨兵串就播默认音效；换成别的名字，它会当成自定义音效去
///   `/System/Library/Sounds` 里找同名文件，找不到就一声不吭。
/// - Windows 的 toast 只认 `Default` / `IM` / `Mail` / `Reminder` / `SMS` /
///   `Alarm*` 这几个内置标识，解析不了的串会被丢掉 —— 拿 macOS 那个哨兵喂它，
///   结果就是静音。
/// - Linux 走 XDG 的 `sound-name` 提示，而 dunst / GNOME 这些常见的通知守护进程
///   压根不理会它。传个名字过去只会让人以为配好了，不如老实返回 `None`。
#[cfg(target_os = "macos")]
const DEFAULT_SOUND: Option<&str> = Some("NSUserNotificationDefaultSoundName");
#[cfg(target_os = "windows")]
const DEFAULT_SOUND: Option<&str> = Some("Default");
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const DEFAULT_SOUND: Option<&str> = None;

/// 把开关翻译成「挂在通知上的声音名」。抽出来是为了能在没有 AppHandle 的单测里
/// 验证这个映射 —— `AppHandle` 在测试里根本造不出来。
fn sound_name(sound: bool) -> Option<&'static str> {
    if sound { DEFAULT_SOUND } else { None }
}

/// 发一条通知。`sound` 决定是否带系统提示音。任何失败只记 stderr ——
/// 通知发不出去不该影响状态机。
///
/// 声音是挂在系统通知上的（`NotificationBuilder::sound`），所以它只有在通知本身
/// 发得出去时才存在 —— 这正是 `Settings::sound` 与 `Settings::notify` 的耦合：
/// 关掉通知，这里根本不会被调用，声音自然也一并没了。
pub fn send(app: &tauri::AppHandle, snapshot: &Snapshot, sound: bool) {
    show(app, snapshot.aggregate.label(), &body(snapshot), sound);
}

/// 发一条自定义文案的通知。状态通知之外的通告用它（例如「已进入穿透模式」）。
/// 这类通告**不是**告警，固定静音。
pub fn send_message(app: &tauri::AppHandle, title: &str, body: &str) {
    show(app, title, body, false);
}

/// 通知的统一出口：标题 + 正文 + 可选的提示音，再 show。
///
/// 抽这一层是因为两条路径（告警 / 通告）走的是同一个 builder 链，只是声音不同，
/// 把 `.title().body().show()` 抄两遍迟早会分叉。
fn show(app: &tauri::AppHandle, title: &str, body: &str, sound: bool) {
    let mut builder = app.notification().builder().title(title).body(body);
    if let Some(name) = sound_name(sound) {
        builder = builder.sound(name);
    }

    if let Err(err) = builder.show() {
        eprintln!("[signalhub] 发送通知失败: {err}");
    }
}

/// 出站转发已搬到 `outbound.rs`（钩子列表），这里只负责系统通知。

/// 首次启动时申请通知权限。
///
/// 放在启动时而不是「第一次要发通知时」，是因为弹权限框和发通知没法同时发生
/// —— 那样第一条通知必然丢掉，而第一条往往正是最要紧的那条。
pub fn ensure_permission(app: &tauri::AppHandle, settings: &Settings) {
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
        Snapshot {
            aggregate,
            paused: false,
            since_ms: 0,
            message: None,
            groups: vec![crate::state::GroupView {
                name: crate::state::DEFAULT_GROUP.to_string(),
                aggregate,
                message: None,
                since_ms: 0,
                sessions,
            }],
        }
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

    /// 开关到声音名的映射是唯一能在无 AppHandle 下单测的部分，所以单独钉住它。
    #[test]
    fn the_sound_flag_selects_the_default_sound() {
        assert_eq!(sound_name(true), DEFAULT_SOUND, "开着就该带上本平台的默认音");
        assert_eq!(sound_name(false), None, "关掉就不该带声音");
    }
}
