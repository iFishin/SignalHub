mod alerts;
mod menu;
mod notify;
mod outbound;
mod server;
mod state;
mod stats;
mod sweep;
mod tray;

// CLI (`src/bin/signalhub.rs`) 通过 #[path] 直接包含这两个模块的源码，
// 因此它们是 lib 的公开 API，而不是只给 app 内部用的私有模块。
pub mod config;
pub mod hooks;

use config::Settings;
use notify::Notifier;
use state::{GroupView, Report, Signal, Snapshot, Store};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
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
    /// 点击穿透：整个窗口忽略指针事件
    pass_through: bool,
}

type SharedUi = Mutex<UiFlags>;

/// 前端算好的「状态名 → `#rrggbb`」颜色表，供托盘图标取色。
///
/// **不在 Rust 里复制一份配色**：颜色是「配色方案 + 用户逐个覆盖」解析出来的，
/// 只存在于前端的 `lib/palettes.ts`。复制一份就意味着两处会漂移，而「用户改了
/// 颜色但托盘不变」是最难注意到的那种 bug。
type SharedPalette = Mutex<BTreeMap<String, String>>;

/// 状态时长统计。按前端给的本地日期分桶，见 stats.rs。
type SharedStats = Arc<Mutex<stats::Stats>>;

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
        group: None,
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
    settings.lock().expect("settings poisoned").clone()
}

/// 校验前端传来的设置。
///
/// **必须在 `clamped()` 之前调**：`clamped()` 会静默丢掉不合法的项（那是给
/// 手改配置文件用的兜底），如果先 clamp 再校验，用户就永远收不到「你写的
/// 颜色不合法」这条反馈。
fn validate_settings(settings: &Settings) -> Result<(), String> {
    for (name, style) in &settings.styles {
        if !Signal::is_known_name(name) {
            return Err(format!(
                "未知的状态名 {name:?}；可用: {}",
                Signal::ALL_NAMES.join(" ")
            ));
        }
        // 颜色是可选的（没写就是跟随配色方案），所以「写了且不合法」才算错
        if let Some(color) = &style.color
            && !config::is_hex_color(color)
        {
            return Err(format!("{name} 的颜色 {color:?} 不是 #rrggbb 形式"));
        }
    }

    // 钩子自己带过滤条件，所以这里逐条校验，报错要能指出是哪一条
    for hook in &settings.hooks {
        if let Some(problem) = config::hook_problem(hook) {
            return Err(format!("钩子不合法：{problem}"));
        }
    }

    Ok(())
}

#[tauri::command]
fn set_settings(app: tauri::AppHandle, settings: Settings) -> Result<Settings, String> {
    validate_settings(&settings)?;
    persist_settings(&app, settings)
}

/// 设置变化的广播事件名。前端 `src/lib/useSettings.ts` 里的字面量必须与之一致。
///
/// 改设置的路径有三条（设置面板、托盘、灯上右键菜单），而两个窗口都得跟着变 ——
/// 所以**每条路径都必须广播**，否则设置窗里的开关会显示过时的状态。
const SETTINGS_EVENT: &str = "signalhub://settings";

/// 改设置的统一出口：存盘 → 改内存 → 按新设置调整界面 → 广播给两个窗口。
///
/// 三条路径都会改设置，各写一遍迟早会漏掉其中一步（尤其是「广播」，漏了不报错，
/// 只是界面偶尔不对）。收成一处就不会。
pub(crate) fn persist_settings(
    app: &tauri::AppHandle,
    settings: Settings,
) -> Result<Settings, String> {
    let settings = settings.clamped();

    // 先落盘再改内存和界面：保存失败时不该出现「界面变了但重启就没了」
    config::save_settings(&settings).map_err(|err| format!("保存设置失败: {err}"))?;
    *app.state::<SharedSettings>().lock().expect("settings poisoned") = settings.clone();

    let snapshot = app
        .state::<SharedStore>()
        .lock()
        .expect("state store poisoned")
        .snapshot();
    apply_settings(app, &settings, &snapshot);
    let _ = app.emit(SETTINGS_EVENT, &settings);
    Ok(settings)
}

/// 读出当前设置。
pub(crate) fn current_settings(app: &tauri::AppHandle) -> Settings {
    app.state::<SharedSettings>()
        .lock()
        .expect("settings poisoned")
        .clone()
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

/// 今日各状态的累计时长，按时长从多到少。
///
/// `today` 由前端传本地日期 —— Rust 这边没有时区数据，只有 UTC。
#[derive(serde::Serialize)]
struct StatRow {
    state: Signal,
    label: &'static str,
    seconds: u64,
}

#[tauri::command]
fn get_stats(stats: tauri::State<'_, SharedStats>, today: String) -> Vec<StatRow> {
    stats
        .lock()
        .expect("stats poisoned")
        .report(Instant::now(), &today)
        .into_iter()
        .map(|(state, duration)| StatRow {
            state,
            label: state.label(),
            seconds: duration.as_secs(),
        })
        .collect()
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

    // 开启时发一条通知说明怎么退出。**不再顺手把外形收起来**：穿透下窗口本来
    // 就点不动，而外形是用户自己选的、存在设置里的，替用户改掉反而是越权。
    if enabled {
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

/// 折叠 / 展开外形。
///
/// 三条路径都走它：灯上左键点击、灯上右键菜单、托盘菜单。写两份行为迟早分叉。
pub(crate) fn set_collapsed(app: &tauri::AppHandle, collapsed: bool) -> Result<Settings, String> {
    let current = current_settings(app);
    if current.collapsed == collapsed {
        // 没变就别写盘、别广播 —— 左键连点两下不该产生两次磁盘写入
        return Ok(current);
    }
    persist_settings(app, Settings { collapsed, ..current })
}

/// 灯上左键点击：折叠 ↔ 展开。前端靠 `signalhub://settings` 广播重画，
/// 这里把新设置返回去只是省掉一次多余的读取。
#[tauri::command]
fn toggle_collapsed(app: tauri::AppHandle) -> Result<Settings, String> {
    let collapsed = !current_settings(&app).collapsed;
    set_collapsed(&app, collapsed)
}

/// 时间线的窗口长度。近半小时够看出「刚才卡了多久」，又不至于挤成一条线。
const TIMELINE_WINDOW: Duration = Duration::from_secs(30 * 60);

/// 近 30 分钟的状态时间线，从旧到新，供设置面板画那条小色带。
#[tauri::command]
fn get_timeline(stats: tauri::State<'_, SharedStats>) -> Vec<stats::Segment> {
    stats
        .lock()
        .expect("stats poisoned")
        .timeline(Instant::now(), TIMELINE_WINDOW)
}

/// 在灯上某一组的位置弹出右键菜单。
#[tauri::command]
fn open_group_menu(app: tauri::AppHandle, group: String) -> Result<(), String> {
    menu::show(&app, &group)
}

/// 前端把算好的颜色表推过来 —— 托盘图标用它取色。
///
/// 由前端推而不是 Rust 自己算，是因为配色是「配色方案 + 逐个状态覆盖」解析出来的
/// 结果，只存在于前端（见 `SharedPalette` 的说明）。
#[tauri::command]
fn set_palette_colors(app: tauri::AppHandle, colors: BTreeMap<String, String>) {
    *app.state::<SharedPalette>().lock().expect("palette poisoned") = colors;
    // 立刻重画一次：否则用户改完颜色要等下一次状态变化才在托盘上看到
    refresh_tray(&app);
}

/// 前端推过来的「现在是几点（本地时间）」。
///
/// Rust 只有 UTC，没有时区数据，所以这个**事实**只能由前端提供；但「在不在勿扰
/// 窗口内」的判断留在 Rust（`config::QuietHours::covers`），跨午夜那种边界逻辑
/// 只有一处实现才敢信，而且那边能单测。
#[tauri::command]
fn set_local_hour(app: tauri::AppHandle, hour: u8) {
    app.state::<alerts::LocalHour>().set(hour.min(23));
}

/// 按当前状态重画托盘图标。
///
/// 三条路径都要走它：状态变了（notify 里）、颜色表变了、`tray_follows_state`
/// 开关变了。少走一条的表现是「托盘图标偶尔不更新」，很难归因。
pub(crate) fn refresh_tray(app: &tauri::AppHandle) {
    let settings = current_settings(app);
    let state = app
        .state::<SharedStore>()
        .lock()
        .expect("state store poisoned")
        .snapshot()
        .aggregate;
    let colors = app
        .state::<SharedPalette>()
        .lock()
        .expect("palette poisoned")
        .clone();
    tray::sync(app, state, settings.tray_follows_state, &colors);
}

fn apply_settings(app: &tauri::AppHandle, settings: &Settings, snapshot: &Snapshot) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_always_on_top(settings.always_on_top);
    }
    apply_window_size(app, settings, snapshot);
    // 开关可能刚被改过，所以尺寸之外还要重画一次托盘
    refresh_tray(app);
}

/// 灯上真正会画出来的组：右键隐藏掉的不算，再按优先级截断到 `MAX_SIGNS`。
///
/// 牌的**顺序**由服务端定（最紧急在前，见 `Store::snapshot_at`），所以这里只截断、
/// 不重排。
///
/// **前端渲染和窗口高度都依赖同一个判断。** 两边算出不同的组，窗口要么裁掉最后
/// 一块牌，要么底下空一条。
fn visible_groups<'a>(settings: &Settings, snapshot: &'a Snapshot) -> Vec<&'a GroupView> {
    snapshot
        .groups
        .iter()
        .filter(|group| settings.shows_group(&group.name))
        .take(config::MAX_SIGNS)
        .collect()
}

/// 这些组在展开态各占多高。有东西可读的牌多占一行，正好是个提示。
///
/// 注意「有没有消息」还要看**用户有没有把消息那段关掉** —— 关掉之后牌永远是
/// 单行，窗口高度也得跟着少一行。两边算不一致就会裁掉最后一块牌。
fn sign_heights(settings: &Settings, groups: &[&GroupView]) -> Vec<u16> {
    let heights: Vec<u16> = groups
        .iter()
        .map(|group| {
            config::sign_height(
                settings.size,
                settings.show.message && group.message.is_some(),
            )
        })
        .collect();

    // 一块牌都没有时也要留一块（表示 offline，或者用户把所有组都藏了），
    // 否则窗口会塌成一条缝 —— 那样既看不见状态，也没法拖它
    if heights.is_empty() {
        vec![config::sign_height(settings.size, false)]
    } else {
        heights
    }
}

/// 按可见牌的个数（折叠时是点的个数）调整窗口尺寸。
///
/// 只在尺寸真的变了才 `set_size`：状态变化一秒可能好几次，每次都 resize
/// 是没必要的系统调用。
fn apply_window_size(app: &tauri::AppHandle, settings: &Settings, snapshot: &Snapshot) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };

    // 展开态是一叠牌，折叠态是一排点 —— 两套几何，见 config.rs
    let groups = visible_groups(settings, snapshot);
    let (width, height) = if settings.collapsed {
        config::collapsed_window_size(settings.size, groups.len())
    } else {
        settings.window_size(&sign_heights(settings, &groups))
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
        .manage(SharedStats::default())
        .manage(SharedPalette::default())
        .manage(alerts::LocalHour::default())
        // 右键菜单只记「刚点的是哪一组」，见 menu.rs
        .manage(menu::Target::default())
        .invoke_handler(tauri::generate_handler![
            get_state,
            set_state,
            set_paused,
            get_settings,
            set_settings,
            get_endpoint,
            open_settings,
            get_toggles,
            set_autostart,
            set_pass_through,
            get_stats,
            get_timeline,
            toggle_collapsed,
            open_group_menu,
            set_palette_colors,
            set_local_hour,
            quit
        ])
        .setup(|app| {
            // macOS：变成「配件应用」，不出现在 Dock 和 Cmd-Tab 里。
            // 配置里的 skipTaskbar 在 macOS 上无效，隐藏 Dock 图标只能靠这个。
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let settings = config::load_settings();
            app.manage(SharedSettings::new(settings.clone()));

            // 先套用尺寸和置顶，再显示：否则会先按配置里的默认尺寸闪一下
            let initial = app
                .state::<SharedStore>()
                .lock()
                .expect("state store poisoned")
                .snapshot();
            apply_settings(app.handle(), &settings, &initial);
            if let Some(window) = app.get_webview_window("main") {
                window.set_visible_on_all_workspaces(true)?;
                window.show()?;
            }

            // token 建不出来说明 ~/.signalhub 不可写。这是无法降级的配置问题，
            // 让启动直接失败并打出来，好过一个「能启动但上报接口静默不工作」的 app。
            let token = config::load_or_create_token()?;

            // 通知器只有一个构造入口（notify::compose），它把窗口尺寸、统计、
            // 托盘图标、系统通知、出站钩子、推事件这几件事一起管起来。
            let notify: Notifier = notify::compose(app.handle().clone());
            app.manage(notify.clone());

            alerts::ensure_permission(app.handle(), &settings);

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
            // 右键菜单是运行时构造的，`Menu` 自己没有事件回调 —— 只能挂在应用上，
            // 靠 id 分派。托盘菜单有它自己的回调，两边的 id 不重合，互不打扰。
            app.on_menu_event(|app, event| {
                menu::handle(app, event.id().as_ref());
            });
            tray::init(app.handle().clone())?;
            sweep::spawn(notify.clone(), app.state::<SharedStore>().inner().clone());
            server::spawn(
                notify,
                app.state::<SharedStore>().inner().clone(),
                token,
            );

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
