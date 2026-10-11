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

type SharedStore = Arc<Mutex<Store>>;
type SharedSettings = Mutex<Settings>;
/// 灯球是否处于「展开成会话列表面板」状态。
/// 它是个瞬态 UI 开关，不是设置，所以不落盘、也不进 Settings。
type SharedExpanded = Mutex<bool>;

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

/// 展开 / 收起会话列表面板。
///
/// 由前端点击触发，但**尺寸仍由 Rust 算** —— 它取决于会话数和设置，而这两样
/// 都在 Rust 这边。前端只负责「点了一下」。
#[tauri::command]
fn set_lamp_expanded(
    app: tauri::AppHandle,
    state: tauri::State<'_, SharedExpanded>,
    expanded: bool,
) -> bool {
    // 先释放这把锁再调 apply_window_size，后者也会取它（std 的 Mutex 不可重入）
    *state.lock().expect("expanded poisoned") = expanded;

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

    let expanded = *app.state::<SharedExpanded>().lock().expect("expanded poisoned");
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
        // 只记位置、不记尺寸：尺寸由设置和会话数决定，是程序自己管的，
        // 让插件跟着恢复会和 apply_window_size 打架。
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(tauri_plugin_window_state::StateFlags::POSITION)
                .build(),
        )
        .plugin(tauri_plugin_notification::init())
        .manage(SharedStore::new(Mutex::new(Store::default())))
        .manage(SharedExpanded::new(false))
        .invoke_handler(tauri::generate_handler![
            get_state,
            set_state,
            set_paused,
            get_settings,
            set_settings,
            get_endpoint,
            open_settings,
            set_lamp_expanded,
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
