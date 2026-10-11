mod alerts;
mod notify;
mod server;
mod state;
mod sweep;
mod tray;

// CLI (`src/bin/signalhub.rs`) 通过 #[path] 直接包含这两个模块的源码，
// 因此它们是 lib 的公开 API，而不是只给 app 内部用的私有模块。
pub mod config;
pub mod hooks;

use config::Settings;
use notify::Notifier;
use state::{Report, Signal, Snapshot, Store};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tauri::{Emitter, LogicalSize, Manager};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_global_shortcut::{
    Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState,
};

type SharedStore = Arc<Mutex<Store>>;
type SharedSettings = Mutex<Settings>;

/// 瞬态的界面开关。**不是设置，所以不落盘** —— 尤其是 `pass_through`：
/// 重启后必须回到可点击状态，否则用户一开机就面对一个点不动、拖不动的圆点。
#[derive(Debug, Default, Clone, Copy)]
struct UiFlags {
    /// 灯球是否展开成会话列表面板
    expanded: bool,
    /// 点击穿透：整个窗口忽略指针事件
    pass_through: bool,
}

type SharedUi = Mutex<UiFlags>;

/// 点击穿透的逃生快捷键。**穿透一旦开启，鼠标就没法再操作那个窗口**，
/// 所以这是唯一的键盘出路（另一条是托盘菜单）。
#[cfg(target_os = "macos")]
const PASS_THROUGH_SHORTCUT: (Modifiers, Code) = (Modifiers::SUPER, Code::KeyS);
#[cfg(not(target_os = "macos"))]
const PASS_THROUGH_SHORTCUT: (Modifiers, Code) = (Modifiers::CONTROL, Code::KeyS);

#[cfg(target_os = "macos")]
const PASS_THROUGH_SHORTCUT_LABEL: &str = "⌘⌥S";
#[cfg(not(target_os = "macos"))]
const PASS_THROUGH_SHORTCUT_LABEL: &str = "Ctrl+Alt+S";

#[tauri::command]
fn get_state(store: tauri::State<'_, SharedStore>) -> Snapshot {
    store.lock().expect("state store poisoned").snapshot()
}

/// dev 调试入口：等价于一次 HTTP 上报，走完全相同的 Store 路径，
/// 因此能顺带验证聚合、TTL、事件推送三条链路。
#[tauri::command]
fn set_state(
    app: tauri::AppHandle,
    store: tauri::State<'_, SharedStore>,
    state: String,
    session: Option<String>,
    message: Option<String>,
) -> Result<Snapshot, String> {
    let req = Report {
        state: Signal::parse(&state)?,
        session,
        agent: Some("debug".into()),
        cwd: None,
        message,
        ttl_ms: None,
    };
    Ok(mutate(&app, &store, |guard| guard.apply(Instant::now(), req)))
}

#[tauri::command]
fn set_paused(
    app: tauri::AppHandle,
    store: tauri::State<'_, SharedStore>,
    paused: bool,
) -> Snapshot {
    mutate(&app, &store, |guard| guard.set_paused(paused))
}

#[tauri::command]
fn get_settings(settings: tauri::State<'_, SharedSettings>) -> Settings {
    *settings.lock().expect("settings poisoned")
}

#[tauri::command]
fn set_settings(
    app: tauri::AppHandle,
    state: tauri::State<'_, SharedSettings>,
    settings: Settings,
) -> Result<Settings, String> {
    let settings = settings.clamped();

    // 先落盘再改内存和界面：保存失败时不该出现「界面变了但重启就没了」
    config::save_settings(settings).map_err(|err| format!("保存设置失败: {err}"))?;

    *state.lock().expect("settings poisoned") = settings;

    let snapshot = app
        .state::<SharedStore>()
        .lock()
        .expect("state store poisoned")
        .snapshot();
    apply_settings(&app, settings, &snapshot);
    // 两个窗口都要跟着变（灯球改尺寸，设置窗改控件状态）
    let _ = app.emit("signalhub://settings", settings);
    Ok(settings)
}

/// 给设置面板显示「怎么手动调用这个服务」用。端口和 token 都从文件读，
/// 与 CLI 用的是同一份来源，因此界面显示的和 CLI 实际使用的必然一致。
#[tauri::command]
fn get_endpoint() -> serde_json::Value {
    serde_json::json!({
        "port": config::read_port().ok(),
        "token_preview": config::read_token()
            .map(|token| token.chars().take(8).collect::<String>())
            .ok(),
    })
}

#[tauri::command]
fn open_settings(app: tauri::AppHandle) {
    reveal_settings(&app);
}

/// 把设置窗调整到内容需要的高度。
///
/// 高度由**前端量出来**传进来：内容高度是 CSS 的事，Rust 这边算不出来。
/// 上限取屏幕可用高度（留出菜单栏和边距），超出就让面板内部滚动 —— 否则
/// 在 13 寸笔记本上窗口会顶到屏幕外。
///
/// 这么做是为了去掉「每加一行设置都要手工量一次高度」的反复劳动：之前
/// 620 → 640 → 760 → 820 就是这么一路量上来的，而且每次都可能量漏。
#[tauri::command]
fn fit_settings_window(app: tauri::AppHandle, height: u32) -> Result<(), String> {
    let Some(window) = app.get_webview_window("settings") else {
        return Ok(());
    };

    let available = window
        .current_monitor()
        .ok()
        .flatten()
        .map(|monitor| {
            // monitor.size() 是物理像素，除以缩放比才是逻辑像素
            let logical = monitor.size().height as f64 / monitor.scale_factor();
            (logical as u32).saturating_sub(120)
        })
        .unwrap_or(800);

    let height = height.clamp(240, available.max(240));
    window
        .set_size(LogicalSize::new(420, height))
        .map_err(|err| format!("调整设置窗高度失败: {err}"))
}

/// 展开 / 收起会话列表面板。
///
/// 由前端点击触发，但**尺寸仍由 Rust 算** —— 它取决于会话数和设置，而这两样
/// 都在 Rust 这边。前端只负责「点了一下」。
#[tauri::command]
fn set_lamp_expanded(
    app: tauri::AppHandle,
    ui: tauri::State<'_, SharedUi>,
    expanded: bool,
) -> bool {
    // 先释放这把锁再调 apply_window_size，后者也会取它（std 的 Mutex 不可重入）
    ui.lock().expect("ui poisoned").expanded = expanded;

    let settings = *app
        .state::<SharedSettings>()
        .lock()
        .expect("settings poisoned");
    let snapshot = app
        .state::<SharedStore>()
        .lock()
        .expect("state store poisoned")
        .snapshot();
    apply_window_size(&app, settings, &snapshot);
    expanded
}

/// 三个开关类功能的当前状态。做成一个命令是为了让前端**不必知道**
/// 快捷键的文案 —— 那个字符串由 Rust 定义一次，两边不会漂移。
#[derive(serde::Serialize, Clone)]
struct Toggles {
    autostart: bool,
    pass_through: bool,
    pass_through_shortcut: &'static str,
}

/// 设置面板要跟着变的事件名。**必须发**：穿透和自启也能从托盘菜单和全局
/// 快捷键改，不发的话设置面板里的开关会显示过时的状态。
const TOGGLES_EVENT: &str = "signalhub://toggles";

fn toggles(app: &tauri::AppHandle) -> Toggles {
    Toggles {
        // 开机自启的真值在插件那边（它管着 LaunchAgent plist），不复制进 Settings
        autostart: app.autolaunch().is_enabled().unwrap_or(false),
        pass_through: app
            .state::<SharedUi>()
            .lock()
            .expect("ui poisoned")
            .pass_through,
        pass_through_shortcut: PASS_THROUGH_SHORTCUT_LABEL,
    }
}

#[tauri::command]
fn get_toggles(app: tauri::AppHandle) -> Toggles {
    toggles(&app)
}

#[tauri::command]
fn set_autostart(app: tauri::AppHandle, enabled: bool) -> Result<bool, String> {
    let manager = app.autolaunch();
    let result = if enabled { manager.enable() } else { manager.disable() };
    result.map_err(|err| format!("设置开机自启失败: {err}"))?;

    let _ = app.emit(TOGGLES_EVENT, toggles(&app));
    Ok(enabled)
}

/// 切换点击穿透的核心逻辑。
///
/// 命令和全局快捷键都走它 —— 两处各写一遍的话，行为迟早会分叉。
fn set_pass_through_impl(app: &tauri::AppHandle, enabled: bool) -> Result<(), String> {
    if enabled {
        app.state::<SharedUi>().lock().expect("ui poisoned").expanded = false;
    }
    app.state::<SharedUi>()
        .lock()
        .expect("ui poisoned")
        .pass_through = enabled;

    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "找不到灯球窗口".to_string())?;
    window
        .set_ignore_cursor_events(enabled)
        .map_err(|err| format!("切换点击穿透失败: {err}"))?;

    // 开启时把面板收回去，否则会留下一个点不动的大窗口
    if enabled {
        let settings = *app
            .state::<SharedSettings>()
            .lock()
            .expect("settings poisoned");
        let snapshot = app
            .state::<SharedStore>()
            .lock()
            .expect("state store poisoned")
            .snapshot();
        apply_window_size(app, settings, &snapshot);

        alerts::send_message(
            app,
            "已进入点击穿透模式",
            &format!(
                "圆点不再响应鼠标。按 {PASS_THROUGH_SHORTCUT_LABEL} 或从托盘菜单退出。"
            ),
        );
    }

    let _ = app.emit(TOGGLES_EVENT, toggles(app));
    Ok(())
}

/// 切换点击穿透。
///
/// 开启后**整个窗口忽略指针事件** —— 鼠标再也点不到、拖不动它，只能靠托盘
/// 菜单或全局快捷键退出。所以开启时会发一条通知告诉用户怎么退出：
/// 一个没有逃生通道的穿透模式就是个陷阱。
#[tauri::command]
fn set_pass_through(app: tauri::AppHandle, enabled: bool) -> Result<bool, String> {
    set_pass_through_impl(&app, enabled)?;
    Ok(enabled)
}

/// 窗口无边框、不可关闭、不在任务栏显示，所以必须留一个显式的退出命令，
/// 供前端的 Cmd/Ctrl+Q 兜底（主入口是托盘菜单）。
#[tauri::command]
fn quit(app: tauri::AppHandle) {
    app.exit(0);
}

fn apply_settings(app: &tauri::AppHandle, settings: Settings, snapshot: &Snapshot) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_always_on_top(settings.always_on_top);
    }
    apply_window_size(app, settings, snapshot);
}

/// 灯球上可见的圆点个数。没有会话时也要显示一个点（表示 offline），所以下限是
/// 1；上限是 `MAX_DOTS`，超出的按优先级截断 —— 会话列表本就按优先级排，
/// 所以剩下的恰好是最紧急的那几个。
fn visible_dots(snapshot: &Snapshot) -> u16 {
    (snapshot.sessions.len() as u16).clamp(1, config::MAX_DOTS)
}

/// 按可见圆点个数（或展开后的行数）调整窗口尺寸。
///
/// 只在尺寸真的变了才 `set_size`：状态变化一秒可能好几次，每次都 resize
/// 是没必要的系统调用。
fn apply_window_size(app: &tauri::AppHandle, settings: Settings, snapshot: &Snapshot) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };

    let expanded = app
        .state::<SharedUi>()
        .lock()
        .expect("ui poisoned")
        .expanded;
    let (width, height) = if expanded {
        settings.panel_size(snapshot.sessions.len() as u16)
    } else {
        settings.window_size(visible_dots(snapshot))
    };

    // inner_size 给的是物理像素，要除掉缩放比才能和逻辑尺寸比较
    let scale = window.scale_factor().unwrap_or(1.0);
    let needs_resize = window
        .inner_size()
        .map(|size| {
            (size.width as f64 / scale).round() as u16 != width
                || (size.height as f64 / scale).round() as u16 != height
        })
        .unwrap_or(true);

    if needs_resize {
        let _ = window.set_size(LogicalSize::new(width, height));
    }
}

/// 由命令和托盘菜单共用。
pub(crate) fn reveal_settings(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("settings") else {
        return;
    };
    // 灯球应用是 Accessory 策略（不占 Dock），这种策略下窗口不会主动到前台。
    // 打开设置时临时切回 Regular，关掉时再切回去。
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);

    let _ = window.show();
    let _ = window.set_focus();
}

/// 改状态 -> 有变化才推前端。返回最新快照，省掉前端再拉一次。
fn mutate(
    app: &tauri::AppHandle,
    store: &tauri::State<'_, SharedStore>,
    f: impl FnOnce(&mut Store) -> bool,
) -> Snapshot {
    let (changed, snapshot) = {
        let mut guard = store.lock().expect("state store poisoned");
        let changed = f(&mut guard);
        (changed, guard.snapshot())
    };
    if changed {
        notify::push(app, &snapshot);
    }
    snapshot
}

/// 设置窗被关掉时只隐藏、不销毁，下次打开不用重建；同时把激活策略切回
/// Accessory，免得应用一直占着 Dock 图标。
fn wire_settings_window(app: &tauri::App) {
    let Some(window) = app.get_webview_window("settings") else {
        return;
    };

    let window_handle = window.clone();
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = window_handle.hide();
            #[cfg(target_os = "macos")]
            let _ = window_handle
                .app_handle()
                .set_activation_policy(tauri::ActivationPolicy::Accessory);
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // 单实例要最先注册：多开会让端口文件和状态互相打架，表现为「灯球
        // 偶尔不动」这种极难排查的现象。
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            // 第二个实例起来时把灯球亮出来 —— 用户多半是没找到它才又点了一次图标
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        // 只记位置、不记尺寸：尺寸由设置和会话数决定，是程序自己管的，
        // 让插件跟着恢复会和 apply_window_size 打架。
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(tauri_plugin_window_state::StateFlags::POSITION)
                .build(),
        )
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(SharedStore::new(Mutex::new(Store::default())))
        .manage(SharedUi::default())
        .invoke_handler(tauri::generate_handler![
            get_state,
            set_state,
            set_paused,
            get_settings,
            set_settings,
            get_endpoint,
            open_settings,
            fit_settings_window,
            set_lamp_expanded,
            get_toggles,
            set_autostart,
            set_pass_through,
            quit
        ])
        .setup(|app| {
            // macOS：变成「配件应用」，不出现在 Dock 和 Cmd-Tab 里。
            // 配置里的 skipTaskbar 在 macOS 上无效，隐藏 Dock 图标只能靠这个。
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let settings = config::load_settings();
            app.manage(SharedSettings::new(settings));

            // 先套用尺寸和置顶，再显示：否则会先按配置里的默认尺寸闪一下
            let initial = app
                .state::<SharedStore>()
                .lock()
                .expect("state store poisoned")
                .snapshot();
            apply_settings(app.handle(), settings, &initial);
            if let Some(window) = app.get_webview_window("main") {
                window.set_visible_on_all_workspaces(true)?;
                window.show()?;
            }

            // token 建不出来说明 ~/.signalhub 不可写。这是无法降级的配置问题，
            // 让启动直接失败并打出来，好过一个「能启动但上报接口静默不工作」的 app。
            let token = config::load_or_create_token()?;

            // 通知出口多包一层：每次状态变化都要先按圆点个数核对窗口尺寸、
            // 判断该不该弹系统通知，再推给前端。放在这里而不是 sweep 里，
            // 是因为这两件事都取决于快照内容。
            let emit = notify::to_lamp_window(app.handle().clone());
            let last_alerted: Arc<Mutex<Option<Signal>>> = Arc::new(Mutex::new(None));
            let notify: Notifier = {
                let app_handle = app.handle().clone();
                let last_alerted = last_alerted.clone();
                Arc::new(move |snapshot: &Snapshot| {
                    let settings = *app_handle
                        .state::<SharedSettings>()
                        .lock()
                        .expect("settings poisoned");
                    apply_window_size(&app_handle, settings, snapshot);

                    // 通知只在「新变成需要注意」时发一次，所以要记住上一次的聚合。
                    // previous 无论是否真的发通知都要更新 —— 否则用户重新打开通知
                    // 开关时会立刻收到一条关于「早就发生过的」状态的通知。
                    let mut previous = last_alerted.lock().expect("alert state poisoned");
                    if settings.notify && alerts::should_alert(*previous, snapshot.aggregate) {
                        alerts::send(&app_handle, snapshot);
                    }
                    *previous = Some(snapshot.aggregate);

                    emit(snapshot);
                })
            };
            app.manage(notify.clone());

            alerts::ensure_permission(app.handle(), settings);

            // 全局快捷键是点击穿透唯一的键盘出路，所以先把它注册成功 ——
            // 注册失败只记日志（不阻断启动），但用户从托盘菜单仍能退出穿透。
            let shortcut = Shortcut::new(
                Some(PASS_THROUGH_SHORTCUT.0),
                PASS_THROUGH_SHORTCUT.1,
            );
            if let Err(err) = app.global_shortcut().on_shortcut(
                shortcut,
                |app, _shortcut, event| {
                    // 只认按下，否则一次按键会来回切换两次
                    if event.state != ShortcutState::Pressed {
                        return;
                    }
                    let enabled = app
                        .state::<SharedUi>()
                        .lock()
                        .expect("ui poisoned")
                        .pass_through;
                    if let Err(err) = set_pass_through_impl(app, !enabled) {
                        eprintln!("[signalhub] 快捷键切换点击穿透失败: {err}");
                    }
                },
            ) {
                eprintln!(
                    "[signalhub] 注册全局快捷键 {PASS_THROUGH_SHORTCUT_LABEL} 失败（{err}）；\
                     点击穿透仍可从托盘菜单退出"
                );
            }

            wire_settings_window(app);
            tray::init(app.handle().clone())?;
            sweep::spawn(notify, app.state::<SharedStore>().inner().clone());
            server::spawn(
                app.handle().clone(),
                app.state::<SharedStore>().inner().clone(),
                token,
            );

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
