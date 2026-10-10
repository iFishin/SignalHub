mod notify;
mod server;
mod state;
mod sweep;
mod tray;

// CLI (`src/bin/signalhub.rs`) 通过 #[path] 直接包含 config.rs 的源码，
// 因此它是 lib 的公开 API，而不是只给 app 内部用的私有模块。
pub mod config;

use config::Settings;
use notify::Notifier;
use state::{Report, Signal, Snapshot, Store};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tauri::{Emitter, LogicalSize, Manager};

type SharedStore = Arc<Mutex<Store>>;
type SharedSettings = Mutex<Settings>;

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
    apply_settings(&app, settings);
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

/// 窗口无边框、不可关闭、不在任务栏显示，所以必须留一个显式的退出命令，
/// 供前端的 Cmd/Ctrl+Q 兜底（主入口是托盘菜单）。
#[tauri::command]
fn quit(app: tauri::AppHandle) {
    app.exit(0);
}

fn apply_settings(app: &tauri::AppHandle, settings: Settings) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let side = settings.window_side();
    let _ = window.set_size(LogicalSize::new(side, side));
    let _ = window.set_always_on_top(settings.always_on_top);
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
        .manage(SharedStore::new(Mutex::new(Store::default())))
        .invoke_handler(tauri::generate_handler![
            get_state,
            set_state,
            set_paused,
            get_settings,
            set_settings,
            get_endpoint,
            open_settings,
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
            apply_settings(app.handle(), settings);
            if let Some(window) = app.get_webview_window("main") {
                window.set_visible_on_all_workspaces(true)?;
                window.show()?;
            }

            // token 建不出来说明 ~/.signalhub 不可写。这是无法降级的配置问题，
            // 让启动直接失败并打出来，好过一个「能启动但上报接口静默不工作」的 app。
            let token = config::load_or_create_token()?;

            let notify: Notifier = notify::to_lamp_window(app.handle().clone());
            app.manage(notify.clone());

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
