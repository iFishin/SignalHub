mod alerts;
mod lang;
mod menu;
mod notify;
mod open;
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
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{Emitter, LogicalSize, Manager};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_global_shortcut::{
    Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState,
};

type SharedStore = Arc<Mutex<Store>>;
/// 共享的设置。**和 `SharedStore` 一样是 `Arc`，不是裸 `Mutex`** ——
/// HTTP 上报那条路径也要读设置（`group_by` 兜底分组），而 `server::spawn`
/// 拿到的是在那之前就构造好的 ctx，只能靠 Arc 共享同一份，不能再 manage 一个。
type SharedSettings = Arc<Mutex<Settings>>;

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

/// HTTP 服务**实际**绑到了哪里（`true` = 0.0.0.0，暴露到局域网）。
///
/// 单独记一份而不是直接读设置：设置是「用户想要什么」，这个是「现在到底是什么」。
/// 监听地址只在启动时绑一次（见 `Settings::lan_access`），所以两者在用户改完开关
/// 之后、重启之前**必然不一致** —— 界面靠这个差值决定要不要提示「重启才生效」，
/// 而不是无条件挂一句让人以为一直没生效的说明。
///
/// 和 `SharedStore` 一样是 `Arc`：`server::spawn` 要拿到一个能活过它自己的句柄。
type SharedBind = Arc<Mutex<bool>>;

/// 最近几次出站钩子的执行结果。界面上要能回答「我的服务器到底收到没有」——
/// 光看 stderr 是查不出来的（从 Dock 启动的话根本没地方看）。
type SharedOutbound = outbound::OutboundLog;

/// 点击穿透的逃生快捷键。**穿透一旦开启，鼠标就没法再操作那个窗口**，
/// 所以这是唯一的键盘出路（另一条是托盘菜单）。
///
/// 组合里**必须带 Alt**。这里曾经注册的是 ⌘S / Ctrl+S —— 那是全世界的「保存」，
/// 全局注册等于把它从所有其它应用手里抢走；而当时的文案写的是「⌘⌥S」。两头
/// 都错：提示里的组合按不出来，真正生效的那个又在别处捣乱。
#[cfg(target_os = "macos")]
const PASS_THROUGH_SHORTCUT: (Modifiers, Code) =
    (Modifiers::SUPER.union(Modifiers::ALT), Code::KeyS);
#[cfg(not(target_os = "macos"))]
const PASS_THROUGH_SHORTCUT: (Modifiers, Code) =
    (Modifiers::CONTROL.union(Modifiers::ALT), Code::KeyS);

/// 快捷键写法的两种平台习惯。
///
/// **做成显式参数、而不是在函数体里直接 `cfg!`**，是为了让两套写法在**任何平台
/// 上都能被单测走到**。起因是一次真实的漏网：`efc8f16` 把 macOS 的修饰键顺序照抄
/// 进了 Windows / Linux 那一支，写出来的标签是 `Alt+Super+S`（哪个平台都不这么写），
/// 而本地是 macOS、`cfg!` 永远走不到那一支 —— CI 上 Linux 连红三次才被发现。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyStyle {
    /// macOS：按 Apple HIG 的 ⌃⌥⇧⌘，Command 紧贴键名。
    Mac,
    /// Windows / Linux：Super 打头 —— 微软文档里 Win 键一律写最前
    /// （`Win+Shift+S` 截图、`Win+Ctrl+Shift+B` 重启显卡驱动），
    /// Linux 的快捷键语法也是 `<Super><Alt>s`。
    Other,
}

/// 当前平台用哪种写法。
fn key_style() -> KeyStyle {
    if cfg!(target_os = "macos") { KeyStyle::Mac } else { KeyStyle::Other }
}

/// 快捷键的人话写法：macOS 上是「⌥⌘S」，别处是「Super+Alt+S」。
///
/// **从 `PASS_THROUGH_SHORTCUT` 派生，不另立常量。** 这里原来有一份独立的
/// label 常量，结果和真正注册的组合漂移了 —— 而且漂移了也没人会发现，因为
/// 文案只出现在提示和日志里，没有任何东西拿它和注册的那份比对。派生出来就
/// 不存在「两份真值」这件事了。
fn shortcut_label(mods: Modifiers, code: Code) -> String {
    // `Code` 的 `Display` 给的是 W3C 名字（`KeyS`、`Digit1`），键盘上印的不是这个。
    let w3c = code.to_string();
    let key = w3c
        .strip_prefix("Key")
        .or_else(|| w3c.strip_prefix("Digit"))
        .or_else(|| w3c.strip_prefix("Numpad"))
        .unwrap_or(&w3c);

    shortcut_label_for(mods, key, key_style())
}

/// 指定写法的版本。单测用它把**两套**都跑一遍，不受当前平台限制。
fn shortcut_label_for(mods: Modifiers, key: &str, style: KeyStyle) -> String {
    match style {
        KeyStyle::Mac => {
            let mut out = String::new();
            for (flag, glyph) in [
                (Modifiers::CONTROL, '⌃'),
                (Modifiers::ALT, '⌥'),
                (Modifiers::SHIFT, '⇧'),
                (Modifiers::SUPER, '⌘'),
            ] {
                if mods.contains(flag) {
                    out.push(glyph);
                }
            }
            out.push_str(key);
            out
        }
        KeyStyle::Other => {
            let mut parts: Vec<&str> = Vec::new();
            for (flag, name) in [
                (Modifiers::SUPER, "Super"),
                (Modifiers::CONTROL, "Ctrl"),
                (Modifiers::ALT, "Alt"),
                (Modifiers::SHIFT, "Shift"),
            ] {
                if mods.contains(flag) {
                    parts.push(name);
                }
            }
            parts.push(key);
            parts.join("+")
        }
    }
}

/// 当前平台穿透快捷键的文案。托盘、通知、设置面板都从这里取。
fn pass_through_shortcut_label() -> String {
    shortcut_label(PASS_THROUGH_SHORTCUT.0, PASS_THROUGH_SHORTCUT.1)
}

#[tauri::command]
fn get_state(store: tauri::State<'_, SharedStore>) -> Snapshot {
    store.lock().expect("state store poisoned").snapshot()
}

/// dev 调试入口：等价于一次 HTTP 上报，走完全相同的 Store 路径，
/// 因此能顺带验证聚合、TTL、事件推送三条链路。
///
/// `group` 可选是为了能在 dev 里凑出多分组 —— 灯球是一组一块牌，只报
/// `default` 的话那条渲染路径根本走不到。
#[tauri::command]
fn set_state(
    app: tauri::AppHandle,
    store: tauri::State<'_, SharedStore>,
    state: String,
    session: Option<String>,
    group: Option<String>,
    message: Option<String>,
) -> Result<Snapshot, String> {
    let req = Report {
        state: Signal::parse(&state)?,
        session,
        group,
        agent: Some("debug".into()),
        cwd: None,
        message,
        ttl_ms: None,
    };
    Ok(mutate(&app, &store, |guard, now| guard.apply(now, req)))
}

/// 测试上报的存活时间。
///
/// **必须短。** 这个按钮的用途是「试一下，看灯有没有反应」—— 留一块要等五到十分钟
/// 才走的牌，比不试还让人困惑：用户会以为那是某个真实项目在报，还会怀疑是不是卡住了。
/// 20 秒够看清灯上发生了什么（换色、闪、窗口变高），又短到转过头就没了。
const TEST_REPORT_TTL_MS: u64 = 20_000;

/// 上限写死成**编译期断言**而不是单测：`const` 比字面量本来就是编译期求值的，
/// 写成 `assert!` 的话 clippy 会说「这个断言是常量」（它等于什么都没测）。
/// 这样写，谁把这个值调过头就是**编译不过**，比跑一次测试才红更早也更硬。
const _: () = assert!(TEST_REPORT_TTL_MS <= 60_000);

/// 从设置面板发一条**测试上报**，用来回答「灯到底通没通」。
///
/// 和 `set_state`（dev 快捷键用的那个）分开，因为两件事的诉求不一样：那个要的是
/// 「立刻切到某个状态」，这个是「试一下就收工」—— 所以 TTL 走上面那个短值。
///
/// 走的是和 HTTP 上报**完全相同的 Store 路径**（`mutate` → `notify`），所以它验证的是
/// 真实链路：聚合、窗口尺寸、今日统计、推给灯球窗口。它**不**验证 HTTP 那一层
/// （服务起没起来、token 对不对）—— 那得用「接入」页给的那条 curl。
#[tauri::command]
fn send_test_report(
    app: tauri::AppHandle,
    store: tauri::State<'_, SharedStore>,
    state: String,
) -> Result<Snapshot, String> {
    let req = Report {
        state: Signal::parse(&state)?,
        session: Some("test".into()),
        // 组名也留成 `test`：灯上会显示「test · 需要授权」，一眼看得出这是试出来的，
        // 而不是某个真实项目在报
        group: Some("test".into()),
        agent: Some("test".into()),
        cwd: None,
        message: None,
        ttl_ms: Some(TEST_REPORT_TTL_MS),
    };
    Ok(mutate(&app, &store, |guard, now| guard.apply(now, req)))
}

#[tauri::command]
fn set_paused(
    app: tauri::AppHandle,
    store: tauri::State<'_, SharedStore>,
    paused: bool,
) -> Snapshot {
    mutate(&app, &store, |guard, now| guard.set_paused(paused, now))
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

/// 导出：把当前设置写到用户选好的路径。
///
/// 路径由前端用 dialog 插件选（那是唯一需要插件的地方）—— **读写都在 Rust**，
/// 前端不碰文件内容。
#[tauri::command]
fn export_settings(app: tauri::AppHandle, path: String) -> Result<(), String> {
    let settings = current_settings(&app);
    let json = serde_json::to_string_pretty(&settings)
        .map_err(|err| format!("序列化设置失败: {err}"))?;
    // 原子写：导出的文件用户多半会留着当备份，写了一半的备份比没有更糟
    config::atomic_write(Path::new(&path), &json, None)
        .map_err(|err| format!("写入 {path} 失败: {err}"))
}

/// 导入：从用户选的文件读设置。
///
/// **必须和设置面板改设置走完全同一条路**（`validate_settings` → `persist_settings`）。
/// 绕过的话，一份手改坏了的文件能把内存里的设置弄成非法状态；而 `persist_settings`
/// 里还有 `clamped()`，所以「超范围的值」是被夹回去、不是整份拒绝 —— 后者对
/// 「从别处拷来的配置」太苛刻了。
#[tauri::command]
fn import_settings(app: tauri::AppHandle, path: String) -> Result<Settings, String> {
    let raw = std::fs::read_to_string(&path).map_err(|err| format!("读取 {path} 失败: {err}"))?;
    let settings: Settings = serde_json::from_str(&raw)
        .map_err(|err| format!("{path} 不像一份设置文件: {err}"))?;
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
///
/// **给完整的 token，不给前 8 位。** 对接方要拿它去调接口，给一截等于没给；
/// 而且它就是 `~/.signalhub/token` 那个文件的内容，能打开设置面板的人本来就能
/// 读到它 —— 只显示前几位换来的是「看起来安全」而不是安全。
#[tauri::command]
fn get_endpoint(app: tauri::AppHandle) -> serde_json::Value {
    serde_json::json!({
        "port": config::read_port().ok(),
        "token": config::read_token().ok(),
        "dir": config::data_dir()
            .ok()
            .map(|path| path.to_string_lossy().into_owned()),
        // 局域网那几样是给「接入」页显示用的。**`bound_lan` 是「现在真的绑到哪了」
        // 而不是设置里的值** —— 两者在用户改完开关、重启之前必然不一致，界面靠
        // 这个差值决定要不要提示「重启才生效」。
        "lan_access": current_settings(&app).lan_access,
        "bound_lan": *app.state::<SharedBind>().lock().expect("bind poisoned"),
        "lan_ip": server::local_ip(),
    })
}

/// 文档在 GitHub 上。
///
/// **刻意不把 `docs/` 打进 bundle**：那份文档大半是维护者向的设计取舍（为什么
/// 这么决定、踩过什么坑），对只想接个接口的人来说太吵。对接方要的那部分在设置
/// 面板的「接入」页里有一份速查，跟包一起走、不联网也能看。
const DOCS_URL: &str = "https://github.com/iFishin/SignalHub/blob/main/docs/api.md";

#[tauri::command]
fn open_docs() -> Result<(), String> {
    open::open(DOCS_URL).map_err(|err| format!("打开浏览器失败: {err}"))
}

/// 在文件管理器里指出应用自己的文件。
///
/// 接入方最需要的那几样都在 `~/.signalhub/` 里（`token`、`port`、`config.json`），
/// 而「它在哪」这件事从界面上说不清楚，直接打开最省事。
#[tauri::command]
fn open_app_dir(what: String) -> Result<(), String> {
    let path = match what.as_str() {
        "config" => config::data_dir(),
        "log" => config::hook_log_path(),
        other => return Err(format!("不认识的目录 {other:?}；可用 config / log")),
    }
    .map_err(|err| format!("找不到目录: {err}"))?;

    // 日志文件可能还不存在（一次 hook 都没跑过）。那就退回到它的父目录 ——
    // 报「文件不存在」对用户没有任何帮助。
    let target = if path.exists() {
        path
    } else {
        path.parent().map(Path::to_path_buf).unwrap_or(path)
    };

    open::reveal(&target.to_string_lossy()).map_err(|err| format!("打开失败: {err}"))
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
fn get_stats(
    app: tauri::AppHandle,
    stats: tauri::State<'_, SharedStats>,
    today: String,
) -> Vec<StatRow> {
    let lang = current_lang(&app);
    stats
        .lock()
        .expect("stats poisoned")
        .report(Instant::now(), &today)
        .into_iter()
        .map(|(state, duration)| StatRow {
            state,
            label: lang::signal_label(lang, state),
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
    pass_through_shortcut: String,
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
        pass_through_shortcut: pass_through_shortcut_label(),
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

    // 折叠态那枚角标跟着这个开关出现 / 消失，所以窗口要重新算一次尺寸。
    // 不改这一句的表现是：穿透开启时角标把窗口撑出去一截（或者退出时留着一截空白）。
    {
        let snapshot = app
            .state::<SharedStore>()
            .lock()
            .expect("store poisoned")
            .snapshot();
        let settings = app
            .state::<SharedSettings>()
            .lock()
            .expect("settings poisoned")
            .clone();
        apply_window_size(app, &settings, &snapshot);
    }

    // 开启时发一条通知说明怎么退出。**不再顺手把外形收起来**：穿透下窗口本来
    // 就点不动，而外形是用户自己选的、存在设置里的，替用户改掉反而是越权。
    if enabled {
        let lang = current_lang(app);
        alerts::send_message(
            app,
            lang::text(lang).pass_through_title,
            &lang::pass_through_body(lang, &pass_through_shortcut_label()),
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

/// 静音 / 取消静音某一组。
///
/// 两条路径都走它：灯上右键菜单、通知上的「静音该组」按钮。和 `set_collapsed`
/// 一样 —— 同一件事有两条入口时，各写一遍行为迟早分叉。
pub(crate) fn set_group_muted(
    app: &tauri::AppHandle,
    group: &str,
    muted: bool,
) -> Result<Settings, String> {
    let settings = current_settings(app);
    let mut muted_groups = settings.muted_groups.clone();
    if muted {
        if !muted_groups.iter().any(|name| name == group) {
            muted_groups.push(group.to_string());
        }
    } else {
        muted_groups.retain(|name| name != group);
    }

    persist_settings(app, Settings { muted_groups, ..settings })
}

/// 最近上报过的消息，从旧到新。前端要倒序显示就自己 reverse。
#[tauri::command]
fn get_messages(store: tauri::State<'_, SharedStore>) -> Vec<state::MessageEntry> {
    store.lock().expect("state store poisoned").messages()
}

/// 最近几次出站钩子的执行结果，从旧到新。
///
/// 界面按**目标**（网址或命令）去对应钩子，不按下标 —— 钩子列表可以增删排序，
/// 下标会错位，于是会显示成「另一条钩子的结果」，比不显示更糟。
#[tauri::command]
fn get_outbound_log(log: tauri::State<'_, SharedOutbound>) -> Vec<outbound::Attempt> {
    log.lock()
        .expect("outbound log poisoned")
        .iter()
        .cloned()
        .collect()
}

/// 系统通知权限的当前状态（`granted` / `denied` / `prompt` / `unknown`）。
///
/// 界面上要能回答「通知到底通没通」—— 没权限时通知是静默丢弃的，不弹也不报错，
/// 光看界面和日志都查不出来（见 `alerts::permission_state`）。
#[tauri::command]
fn get_notify_permission(app: tauri::AppHandle) -> &'static str {
    alerts::permission_state(&app)
}

/// 主动要一次通知权限，返回要过之后的状态。
#[tauri::command]
fn request_notify_permission(app: tauri::AppHandle) -> &'static str {
    alerts::request_permission(&app)
}

/// 一个智能体在这台机器上的状态。
#[derive(serde::Serialize)]
struct AgentStatus {
    /// CLI 里用的名字（`claude-code`）
    id: &'static str,
    /// 显示名（`Claude Code`）
    name: &'static str,
    /// 这台机器上有没有它
    detected: bool,
    /// 我们有没有给它装过 hook
    installed: bool,
    /// 配置文件路径，界面上显示用
    config: String,
}

/// 扫一遍机器上有哪些智能体、我们接入了哪些。
///
/// 检测只是给界面排个序，**不决定能不能装** —— 没检测到的也照样能一键接入，
/// 因为「检测」看的是痕迹（配置目录、PATH 上的命令），而痕迹可能因为路径变化、
/// 便携安装等原因对不上。宁可让用户看到一个「没检测到」但其实装得了的条目，
/// 也不要因为判错而把路堵死。
#[tauri::command]
fn detect_agents() -> Vec<AgentStatus> {
    hooks::ALL_NAMES
        .iter()
        .filter_map(|id| hooks::Agent::parse(id).ok().map(|agent| (*id, agent)))
        .map(|(id, agent)| AgentStatus {
            id,
            name: agent.name(),
            detected: hooks::detect(agent),
            installed: hooks::is_installed(agent),
            config: hooks::config_path(agent)
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_default(),
        })
        .collect()
}

/// 一次接入 / 卸载改了什么。
///
/// **返回结构而不是一句拼好的话**：文案要分语言，而 Rust 只负责干活（见 lang.rs）。
#[derive(serde::Serialize)]
struct HookChange {
    added: Vec<String>,
    updated: Vec<String>,
    already: Vec<String>,
    removed_handlers: usize,
    config: String,
    backup: Option<String>,
}

impl From<hooks::Report> for HookChange {
    fn from(report: hooks::Report) -> Self {
        Self {
            added: report.added,
            updated: report.updated,
            already: report.already,
            removed_handlers: report.removed_handlers,
            config: report.path.to_string_lossy().into_owned(),
            backup: report
                .backup
                .map(|path| path.to_string_lossy().into_owned()),
        }
    }
}

/// 给一个智能体装 hook。
///
/// 与 CLI 的 `install hooks <agent>` **走同一段逻辑**（`hooks::install`）——
/// 界面和命令行两条入口共用实现，行为不会分叉。
#[tauri::command]
fn install_agent_hooks(agent: String) -> Result<HookChange, String> {
    hooks::install(hooks::Agent::parse(&agent)?, false).map(HookChange::from)
}

#[tauri::command]
fn uninstall_agent_hooks(agent: String) -> Result<HookChange, String> {
    hooks::uninstall(hooks::Agent::parse(&agent)?, false).map(HookChange::from)
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

/// 在灯的空白处（或还没接上报方时那块占位牌）弹出通用菜单。
///
/// 与 `open_group_menu` 分开是必须的：占位牌不对应任何真实的组，传一个空组名
/// 过去只会得到一张「隐藏/静音/清除那个叫 "" 的组」的菜单，点哪个都不会发生
/// 任何事 —— 而它恰恰是首次安装时灯上唯一能右键的地方。
#[tauri::command]
fn open_lamp_menu(app: tauri::AppHandle) -> Result<(), String> {
    menu::show_general(&app)
}

/// 前端推过来的**解析后**的界面语言（`"auto"` 由前端解析 —— Rust 不知道系统语言）。
///
/// 原生界面（托盘菜单、右键菜单、通知按钮、窗口标题）不经过 webview，前端换不了，
/// 所以语言一变就得把这几样重建一遍。收在 `apply_lang` 一个函数里，免得以后新增
/// 一处原生文案时忘了加进来 —— 漏掉的表现是界面上混着两种语言，而那只有肉眼能发现。
#[tauri::command]
fn set_lang(app: tauri::AppHandle, lang: String) -> Result<(), String> {
    let Some(lang) = lang::Lang::parse(&lang) else {
        return Err(format!("不认识的语言 {lang:?}；可用 zh / en"));
    };
    app.state::<lang::LocalLang>().set(lang);
    apply_lang(&app);
    Ok(())
}

/// 把语言应用到所有原生界面。语言变了就调它。
pub(crate) fn apply_lang(app: &tauri::AppHandle) {
    let lang = app.state::<lang::LocalLang>().get();
    // 托盘菜单是启动时建好就一直挂着的，换语言得整个重建
    tray::rebuild_menu(app, lang);
    // 通知的动作按钮也带文案，重新注册一次（同 id 覆盖）
    alerts::wire_actions(app);
    // 设置窗的标题在 tauri.conf.json 里，是静态的，只能运行时改
    if let Some(window) = app.get_webview_window("settings") {
        let _ = window.set_title(lang::text(lang).settings_window_title);
    }
}

/// 当前语言。命令和托盘菜单共用。
pub(crate) fn current_lang(app: &tauri::AppHandle) -> lang::Lang {
    app.state::<lang::LocalLang>().get()
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

/// 前端推过来的「现在几点 + 今天几号（本地时间）」。
///
/// Rust 只有 UTC，没有时区数据，所以这两个**事实**只能由前端提供；但「在不在勿扰
/// 窗口内」的判断留在 Rust（`config::QuietHours::covers`），跨午夜那种边界逻辑
/// 只有一处实现才敢信，而且那边能单测。
///
/// **日期也要推（而不只是在 `get_stats` 时带过来）**，是为了让统计能及时跨天结算：
/// 那条查询只在设置面板开着时才发生，而灯球是常驻应用 —— 整天不开面板是常态，
/// 于是「今日时长」会横跨多天一路累加。前端每分钟推一次，跨天因此在分钟级被发现。
///
/// **顺手落盘**：这条心跳是「应用还活着」的现成节拍，拿它当统计的保存点，不必再
/// 起一个定时器。代价是崩掉最多丢一分钟的时长 —— 对一个「今天等了多久」的数字
/// 完全够用。（两个窗口都在推，所以实际是每分钟写两次；文件几百字节，无妨。）
#[tauri::command]
fn set_local_clock(app: tauri::AppHandle, hour: u8, today: String) {
    app.state::<alerts::LocalHour>().set(hour.min(23));

    // 先把 `State` 句柄绑出来：`app.state::<T>()` 返回的是个临时值，直接接 `.lock()`
    // 会让那个临时值在这一句结束时就被释放，而锁还借着它 —— 借用检查过不了。
    let handle = app.state::<SharedStats>();
    let mut stats = handle.lock().expect("stats poisoned");
    let now = Instant::now();
    stats.rollover(now, &today);
    if let Err(err) = stats.save(now) {
        eprintln!("[signalhub] 保存统计失败: {err}");
    }
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
    // 用户刚把通知打开时也要能申请到权限 —— 只在启动时申请的话，启动时通知是关着的
    // 用户永远不会被问一次，而没权限时通知是静默丢弃的（见 alerts::ensure_permission）。
    alerts::ensure_permission(app, settings);
}

/// 灯上真正会画出来的组：右键隐藏掉的不算，再按优先级截断到 `MAX_SIGNS`。
///
/// 牌的**顺序**由服务端定（最紧急在前，见 `Store::snapshot_at`），所以这里只截断、
/// 不重排。
///
/// **前端渲染和窗口高度都依赖同一个判断。** 两边算出不同的组，窗口要么裁掉最后
/// 一块牌，要么底下空一条。
fn visible_groups<'a>(settings: &Settings, snapshot: &'a Snapshot) -> Vec<&'a GroupView> {
    let mut groups: Vec<&GroupView> = snapshot
        .groups
        .iter()
        .filter(|group| settings.shows_group(&group.name))
        .collect();

    // 置顶的排前面。**用稳定排序**：没置顶的组保持 `snapshot.groups` 原有的紧急度
    // 顺序，置顶组之间按权重、同权重时也保持原顺序 —— 一条规则说清两件事。
    //
    // 顺序为什么在这里算而不是在 `Store` 里：`snapshot.groups` 的顺序是**紧急度**，
    // 那是状态语义（`/v1/state` 也按它返回）；而置顶是**显示偏好**。混进 Store 就得
    // 让 Store 知道 Settings，而它们是分开的两块状态。
    groups.sort_by_key(|group| match settings.group_order(&group.name) {
        Some(order) => (0, order),
        None => (1, 0),
    });

    groups.truncate(config::MAX_SIGNS);
    groups
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

/// 折叠态末尾那枚全局状态角标算几个点。
///
/// 「暂停监控」和「点击穿透」共用同一枚角标（同时开着时穿透优先，见
/// `SignalLamp.tsx` 的 `flagTitle`），所以**最多 1**。
///
/// 单独摘一个函数而不是在 `apply_window_size` 里现算：这里要读两个互不相干的
/// 状态桶（`Store::paused` 和 `UiFlags::pass_through`），而调用点只关心「几枚」。
fn ui_flag_count(app: &tauri::AppHandle) -> usize {
    let paused = app
        .state::<SharedStore>()
        .lock()
        .expect("store poisoned")
        .paused();
    let pass_through = app
        .state::<SharedUi>()
        .lock()
        .expect("ui poisoned")
        .pass_through;

    usize::from(paused || pass_through)
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
        // 折叠态那枚全局状态角标单独占一个点的位置，所以点数要加上它
        config::collapsed_window_size(settings.size, groups.len(), ui_flag_count(app))
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
        // 尺寸变了才可能越界，所以放在这个分支里
        keep_on_screen(&window, width, height);
    }
}

/// 尺寸变了之后，把窗口推回屏幕内。
///
/// **为什么需要**：位置是记住的（`tauri-plugin-window-state`），而尺寸由内容决定
/// （`apply_window_size`）。组一多、或者从旧版本升上来（默认从一颗灯变成一叠牌），
/// 窗口就会长大 —— 一个原本贴着屏幕右下角的点会长到屏幕外面去。用户看到的是一块
/// 被裁掉一半的牌子，而且多半想不到是尺寸变了。
///
/// **只在尺寸真的变了之后调**，所以不会和用户拖动打架：用户把窗口拖到哪就是哪，
/// 只要它没越界，这里一个像素都不动。
///
/// `width`/`height` 是**刚设下去的逻辑尺寸**，不是读回来的 —— `set_size` 是往事件
/// 循环投一条消息，紧接着读 `outer_size()` 拿到的还是旧值，用它算会把窗口夹到错的
/// 位置。所以这里自己换算成物理像素。
///
/// 用工作区（`work_area`）而不是整块屏幕：任务栏 / Dock 占掉的那一条不算可用空间，
/// 按整屏算的话，贴着底边的窗口会有一截压在任务栏底下。
fn keep_on_screen(window: &tauri::WebviewWindow, width: u16, height: u16) {
    let (Ok(Some(monitor)), Ok(position)) = (window.current_monitor(), window.outer_position())
    else {
        // 拿不到显示器信息（多屏拔插的瞬间、无头环境）就什么都不做 ——
        // 这只是个兜底，失败不该有任何副作用
        return;
    };

    let scale = window.scale_factor().unwrap_or(1.0);
    let size = (
        (width as f64 * scale).round() as i32,
        (height as f64 * scale).round() as i32,
    );
    let area = monitor.work_area();
    let bounds = (
        (area.position.x, area.position.y),
        (
            area.position.x + area.size.width as i32,
            area.position.y + area.size.height as i32,
        ),
    );

    let (x, y) = clamp_to_area((position.x, position.y), size, bounds);
    if (x, y) != (position.x, position.y) {
        let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
    }
}

/// 把一个窗口位置夹进工作区。返回夹紧后的左上角。
///
/// `bounds` 是 `((左, 上), (右, 下))`。抽成纯函数是为了能单测 —— 边界情况（窗口
/// 比工作区还大、副屏在左侧时的负坐标）是这里最容易写错的地方，而它们在真机上
/// 很难复现。
fn clamp_to_area(
    position: (i32, i32),
    size: (i32, i32),
    bounds: ((i32, i32), (i32, i32)),
) -> (i32, i32) {
    let ((left, top), (right, bottom)) = bounds;
    // 窗口比工作区还大时 `right - width` 会小于 `left`，`clamp` 遇到 min > max 会
    // panic。取 max 保证区间非空，此时结果就是「贴到左上角」—— 那也是唯一合理的
    // 位置（屏幕装不下它，至少让左上角可见）。
    let max_x = (right - size.0).max(left);
    let max_y = (bottom - size.1).max(top);
    (position.0.clamp(left, max_x), position.1.clamp(top, max_y))
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
    f: impl FnOnce(&mut Store, Instant) -> bool,
) -> Snapshot {
    let now = Instant::now();
    let (changed, snapshot) = {
        let mut guard = store.lock().expect("state store poisoned");
        let changed = f(&mut guard, now);
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

/// 崩了要留下一条记录。
///
/// **为什么需要**：release 用 `panic = "abort"`（理由见 Cargo.toml），进程会直接
/// 消失 —— 而灯球没有 Dock 图标、也没有窗口，用户看到的只是圆点不见了。没有这条
/// 记录的话，「应用为什么自己退了」就完全查不出来；而它是常驻应用，静默退出是
/// 最容易被当成「我没注意到」的那类故障。
///
/// `panic = "abort"` 下 hook **照样会跑**，跑完才 abort，所以这里来得及落盘。
///
/// 写进 `~/.signalhub/hooks.log`（和 hook 事件共用那份带轮转的日志）—— 那是用来
/// 排查的地方，多一种条目比多一个文件好找。同时打一份到 stderr，从终端启动时
/// 一眼能看见。
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        // hook 里**绝不能再 panic**（双 panic 直接 abort，且什么都来不及记），
        // 所以全程 `let _ =` / `unwrap_or_else`，不给它第二次出错的机会。
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|text| (*text).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "（读不出 panic 信息）".to_string());
        let place = info
            .location()
            .map(|location| format!("{}:{}", location.file(), location.line()))
            .unwrap_or_else(|| "未知位置".to_string());

        config::append_hook_log(&format!(
            "{} [panic] {message} @ {place}\n",
            config::utc_hms()
        ));
        eprintln!("[signalhub] 崩溃：{message} @ {place}");
    }));
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 第一件事就装它：后面任何一步崩了都要留下记录
    install_panic_hook();

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
        // 只用来选文件（拿路径）。读写和校验都在 Rust 侧的命令里。
        .plugin(tauri_plugin_dialog::init())
        .manage(SharedStore::new(Mutex::new(Store::default())))
        .manage(SharedUi::default())
        .manage(SharedBind::default())
        // 统计从盘上读回来：灯球是常驻的，但「常驻」不等于「永不重启」——
        // 改设置、升级、崩了重来，用户对「今天等了多久」的期待是跨重启的。
        .manage(Arc::new(Mutex::new(stats::Stats::load())))
        .manage(SharedOutbound::default())
        .manage(SharedPalette::default())
        .manage(alerts::LocalHour::default())
        .manage(lang::LocalLang::default())
        // 最近一条告警通知说的是谁 —— 动作按钮回调时要用，见 alerts::LastAlert
        .manage(alerts::LastAlert::default())
        // 右键菜单只记「刚点的是哪一组」，见 menu.rs
        .manage(menu::Target::default())
        .invoke_handler(tauri::generate_handler![
            get_state,
            set_state,
            send_test_report,
            set_paused,
            get_settings,
            set_settings,
            export_settings,
            import_settings,
            get_endpoint,
            open_docs,
            open_app_dir,
            open_settings,
            get_toggles,
            set_autostart,
            set_pass_through,
            get_stats,
            get_timeline,
            get_messages,
            get_outbound_log,
            get_notify_permission,
            request_notify_permission,
            detect_agents,
            install_agent_hooks,
            uninstall_agent_hooks,
            toggle_collapsed,
            open_group_menu,
            open_lamp_menu,
            set_palette_colors,
            set_local_clock,
            set_lang,
            quit
        ])
        .setup(|app| {
            // macOS：变成「配件应用」，不出现在 Dock 和 Cmd-Tab 里。
            // 配置里的 skipTaskbar 在 macOS 上无效，隐藏 Dock 图标只能靠这个。
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let settings = config::load_settings();
            app.manage(Arc::new(Mutex::new(settings.clone())));

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
            // 通知上的动作按钮（打开项目目录 / 静音该组）。注册类型和挂处理函数
            // 必须一起做，只做一半的表现是「按钮从来没见过」且不报错。
            alerts::wire_actions(app.handle());

            // 全局快捷键是点击穿透唯一的键盘出路，所以先把它注册成功 ——
            // 注册失败只记日志（不阻断启动），但用户从托盘菜单仍能退出穿透。
            let label = pass_through_shortcut_label();
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
                    "[signalhub] 注册全局快捷键 {label} 失败（{err}）；\
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
                // 上报路径要读 `group_by`，所以设置也共享给它，而不是让它自己
                // 从盘上再读一份 —— 两份真值迟早会漂移。
                app.state::<SharedSettings>().inner().clone(),
                // 实际绑到哪了，回填给界面判断「那个开关生效了没有」
                app.state::<SharedBind>().inner().clone(),
                token,
            );

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::{
        clamp_to_area, pass_through_shortcut_label, shortcut_label, shortcut_label_for,
        validate_settings, visible_groups, KeyStyle, PASS_THROUGH_SHORTCUT,
        TEST_REPORT_TTL_MS,
    };
    use crate::config::{GroupMeta, Settings, StateStyle};
    use crate::state::{GroupView, Report, Signal, Snapshot, Store};
    use std::time::{Duration, Instant};
    use tauri_plugin_global_shortcut::{Code, Modifiers};

    // ---- 测试上报 -------------------------------------------------------------

    /// 测试上报**不能留下**：点完「试一下」之后，那块牌要在半分钟量级自己走掉，
    /// 而不是拖五到十分钟。
    ///
    /// 钉的是 `TEST_REPORT_TTL_MS` 那个值 —— 谁把它随手调大，这个按钮就从
    /// 「试一下」变成了「留下一块看不懂的牌」。同时钉住「它自己占一组」，
    /// 因为用户要靠组名看出那是试出来的、不是某个真实项目在报。
    #[test]
    fn a_test_report_gets_its_own_group_and_expires_quickly() {
        let mut store = Store::default();
        let t = Instant::now();
        store.apply(
            t,
            Report {
                state: Signal::Permission,
                session: Some("test".into()),
                group: Some("test".into()),
                agent: Some("test".into()),
                cwd: None,
                message: None,
                ttl_ms: Some(TEST_REPORT_TTL_MS),
            },
        );

        let snapshot = store.snapshot_at(t);
        assert_eq!(snapshot.groups.len(), 1);
        assert_eq!(snapshot.groups[0].name, "test", "自己占一组，一眼看得出");
        assert_eq!(snapshot.groups[0].aggregate, Signal::Permission);

        // 半分钟后就不该还在报 permission 了。**要显式跑一次 `expire`** ——
        // 快照只是读，超时是 sweep 那条心跳做的事。
        store.expire(t + Duration::from_secs(30));
        let later = store.snapshot_at(t + Duration::from_secs(30));
        assert_eq!(
            later.groups[0].aggregate,
            Signal::Stale,
            "测试上报要很快过期，否则它会变成一块看不懂的牌"
        );
    }

    // ---- 导入的校验 -----------------------------------------------------------

    /// 导入走 `validate_settings`，所以**我们自己导出的文件必须过得了它** ——
    /// 过不了的话，导出功能就是坏的：用户导出一份、在另一台机器导入，得到的是
    /// 一条报错。
    ///
    /// 顺带锁住「两类错误会被明确报出来」：状态名不认识、颜色格式不对。这两条是
    /// 用户手改配置文件时最可能犯的错，而它们的兜底（`clamped`）是**静默丢掉**、
    /// 只在启动读配置时合适。
    #[test]
    fn import_validation_accepts_our_own_output_and_rejects_junk() {
        assert!(
            validate_settings(&Settings::default()).is_ok(),
            "默认设置必须过得了校验"
        );

        let mut unknown_state = Settings::default();
        unknown_state
            .styles
            .insert("teleporting".into(), StateStyle::default());
        assert!(
            validate_settings(&unknown_state).is_err(),
            "不认识的状态名要报错"
        );

        let mut bad_color = Settings::default();
        bad_color.styles.insert(
            "error".into(),
            StateStyle {
                color: Some("red".into()),
                ..Default::default()
            },
        );
        assert!(validate_settings(&bad_color).is_err(), "非 #rrggbb 要报错");
    }

    // ---- 穿透快捷键 -----------------------------------------------------------

    /// 裸的 ⌘S / Ctrl+S 是「保存」—— 全局注册会把它从所有其它应用手里抢走，
    /// 而且用户按保存时灯会悄悄切换穿透。这里曾经真的就是这么注册的。
    #[test]
    fn the_pass_through_shortcut_does_not_steal_save() {
        let (mods, _) = PASS_THROUGH_SHORTCUT;
        assert!(mods.contains(Modifiers::ALT), "不带 Alt 的组合会和「保存」撞车");
        assert!(
            mods.contains(Modifiers::CONTROL) || mods.contains(Modifiers::SUPER),
            "总得有一个主修饰键"
        );
    }

    /// 文案必须真的描述那个注册的组合。这两样曾经各是一个常量、并且漂移了
    /// —— 文案写 ⌘⌥S，实际注册 ⌘S —— 而没有任何东西会发现。这条测试就是那个
    /// 「会发现的东西」。
    #[test]
    fn the_label_matches_the_registered_shortcut() {
        let label = pass_through_shortcut_label();
        let (mods, _) = PASS_THROUGH_SHORTCUT;

        assert_eq!(
            mods.contains(Modifiers::ALT),
            label.contains('⌥') || label.contains("Alt"),
            "{label} 与注册的修饰键对不上"
        );
        assert_eq!(
            mods.contains(Modifiers::SHIFT),
            label.contains('⇧') || label.contains("Shift"),
            "{label} 与注册的修饰键对不上"
        );
        assert!(
            label.ends_with('S'),
            "{label} 里没有键名 —— 说明 Code 的前缀没被剥掉"
        );
    }

    /// 格式化函数本身：W3C 名字要去掉前缀，修饰键要按平台习惯写。
    ///
    /// **两套写法都断言，不管跑在哪个平台。** 用 `shortcut_label_for` 显式指定
    /// 写法，而不是靠 `cfg!` 只走到当前平台那一支 —— 这个测试原来正是那么写的，
    /// 于是「Windows / Linux 那一支照抄了 macOS 的顺序」在 macOS 开发机上永远
    /// 测不出来，CI 上 Linux 连红三次。
    #[test]
    fn shortcut_labels_read_like_keyboards() {
        let word = Modifiers::CONTROL.union(Modifiers::ALT);
        let command = Modifiers::SUPER.union(Modifiers::ALT);

        // macOS：Apple HIG 的 ⌃⌥⇧⌘，Command 紧贴键名
        assert_eq!(shortcut_label_for(word, "S", KeyStyle::Mac), "⌃⌥S");
        assert_eq!(shortcut_label_for(command, "S", KeyStyle::Mac), "⌥⌘S");

        // Windows / Linux：Super 打头，别把上面那套照抄下来
        assert_eq!(shortcut_label_for(word, "S", KeyStyle::Other), "Ctrl+Alt+S");
        assert_eq!(
            shortcut_label_for(command, "S", KeyStyle::Other),
            "Super+Alt+S"
        );

        // 数字键同样要去掉 `Digit` 前缀，否则会写出「Ctrl+Digit1」这种东西。
        // 走公开入口（`shortcut_label`），顺带覆盖「当前平台是哪种写法」那一层
        let digit = shortcut_label(Modifiers::CONTROL, Code::Digit1);
        assert!(digit.ends_with('1'), "{digit}");
        assert!(!digit.contains("Digit"), "{digit} 里还留着 W3C 前缀");
    }

    // ---- 把窗口夹回工作区 -----------------------------------------------------

    /// 一块 1920×1080、任务栏占掉底部 40px 的屏幕
    const SCREEN: ((i32, i32), (i32, i32)) = ((0, 0), (1920, 1040));
    /// 一块 200×160 的窗口
    const WINDOW: (i32, i32) = (200, 160);

    #[test]
    fn a_window_that_fits_is_left_alone() {
        for position in [(0, 0), (960, 520), (100, 100)] {
            assert_eq!(clamp_to_area(position, WINDOW, SCREEN), position, "{position:?}");
        }
    }

    /// 贴着右下边缘是允许的 —— 夹的是「不许整块出去」，不是「必须留出边距」
    #[test]
    fn hugging_the_bottom_right_corner_is_fine() {
        assert_eq!(clamp_to_area((1720, 880), WINDOW, SCREEN), (1720, 880));
    }

    /// 尺寸变大之后跑到右/下边外面 —— 这就是这个函数存在的理由
    #[test]
    fn a_window_hanging_off_the_right_or_bottom_is_pulled_back() {
        assert_eq!(clamp_to_area((1900, 500), WINDOW, SCREEN), (1720, 500));
        assert_eq!(clamp_to_area((500, 1200), WINDOW, SCREEN), (500, 880));
        assert_eq!(clamp_to_area((1900, 1200), WINDOW, SCREEN), (1720, 880));
    }

    /// 左/上越界也要管：副屏摆在左侧时坐标是负的，主屏在右侧，窗口可能被推到
    /// 主屏左边之外
    #[test]
    fn a_window_hanging_off_the_left_or_top_is_pulled_back() {
        assert_eq!(clamp_to_area((-500, 100), WINDOW, SCREEN), (0, 100));
        assert_eq!(clamp_to_area((100, -500), WINDOW, SCREEN), (100, 0));
    }

    /// 窗口比工作区还大：`clamp` 在 min > max 时会 panic，所以这条既是行为测试
    /// 也是「别 panic」的测试
    #[test]
    fn a_window_bigger_than_the_screen_goes_to_the_top_left() {
        assert_eq!(clamp_to_area((500, 500), (4000, 3000), SCREEN), (0, 0));
    }

    /// 工作区不一定从 (0,0) 开始（副屏在主屏左边/上边时是负的）
    #[test]
    fn an_area_with_a_negative_origin_is_respected() {
        let area = ((-1920, -200), (0, 880));
        assert_eq!(clamp_to_area((-1900, 700), WINDOW, area), (-1900, 700), "区内不动");
        assert_eq!(clamp_to_area((-2000, 900), WINDOW, area), (-1920, 720), "越界拉回");
    }

    // ---- 置顶排序 -------------------------------------------------------------

    fn group(name: &str, aggregate: Signal) -> GroupView {
        GroupView {
            name: name.to_string(),
            aggregate,
            message: None,
            since_ms: 0,
            stale_progress: None,
            sessions: Vec::new(),
        }
    }

    /// 快照里各组的顺序就是**紧急度**顺序（`Store::snapshot_at` 排好的）
    fn snapshot(groups: Vec<GroupView>) -> Snapshot {
        Snapshot {
            aggregate: Signal::Error,
            paused: false,
            since_ms: 0,
            message: None,
            groups,
        }
    }

    fn names(settings: &Settings, snapshot: &Snapshot) -> Vec<String> {
        visible_groups(settings, snapshot)
            .iter()
            .map(|group| group.name.clone())
            .collect()
    }

    /// 置顶的组排最前，**其余保持紧急度顺序** —— 那才是灯的本职，不该被排序偏好
    /// 整个盖掉
    #[test]
    fn pinned_groups_come_first_and_the_rest_keep_urgency_order() {
        let snapshot = snapshot(vec![
            group("ci", Signal::Error),
            group("work", Signal::Working),
            group("docs", Signal::Idle),
        ]);

        assert_eq!(names(&Settings::default(), &snapshot), ["ci", "work", "docs"]);

        let mut settings = Settings::default();
        settings
            .group_meta
            .insert("docs".into(), GroupMeta { name: None, order: Some(0) });
        assert_eq!(
            names(&settings, &snapshot),
            ["docs", "ci", "work"],
            "置顶的排最前，另外两组相对顺序不变"
        );
    }

    /// 多个置顶组按权重排；权重相同时**保持原顺序**（稳定排序）
    #[test]
    fn pinned_groups_sort_by_weight_and_ties_keep_their_order() {
        let snapshot = snapshot(vec![
            group("a", Signal::Error),
            group("b", Signal::Working),
            group("c", Signal::Idle),
        ]);
        let mut settings = Settings::default();
        for (name, order) in [("c", 1), ("b", 0), ("a", 0)] {
            settings
                .group_meta
                .insert(name.into(), GroupMeta { name: None, order: Some(order) });
        }

        assert_eq!(names(&settings, &snapshot), ["a", "b", "c"], "权重小的在前，平局保持原顺序");
    }

    /// 置顶不越过别的规则：它能让一组挤进可见名单，但仍然只留 `MAX_SIGNS` 块；
    /// 而**隐藏优先于置顶** —— 藏起来的组不显示，哪怕置顶了
    #[test]
    fn pinning_does_not_bypass_hiding_or_the_limit() {
        let snapshot = snapshot(vec![
            group("a", Signal::Error),
            group("b", Signal::Working),
            group("c", Signal::Idle),
            group("d", Signal::Idle),
            group("e", Signal::Idle),
        ]);
        let mut settings = Settings::default();
        settings
            .group_meta
            .insert("e".into(), GroupMeta { name: None, order: Some(0) });

        assert_eq!(
            names(&settings, &snapshot),
            ["e", "a", "b", "c"],
            "置顶能让它挤进可见名单，但上限仍是 4"
        );

        settings.hidden_groups = vec!["e".into()];
        assert_eq!(names(&settings, &snapshot), ["a", "b", "c", "d"], "隐藏优先于置顶");
    }
}
