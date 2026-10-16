mod alerts;
mod lang;
mod lamps;
mod menu;
mod notify;
mod open;
mod outbound;
mod server;
mod state;
mod stats;
mod sweep;
mod tray;

// CLI (`src/bin/signalhub.rs`) 通过 #[path] 直接包含这几个模块的源码，
// 因此它们是 lib 的公开 API，而不是只给 app 内部用的私有模块。
//
// `pub` 还有个作用：**写入方**（`spool::append`）只有 CLI 用得到，app 这边只读。
// 私有模块的话，那些函数在 lib 里就是「没人用」，而 CI 是 `-D warnings`。
pub mod config;
pub mod hooks;
pub mod spool;

use config::{LampLayout, Settings};
use notify::Notifier;
use state::{state_ttl, GroupView, Report, Signal, Snapshot, Store};
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
///
/// **里层那个 `Arc<Settings>` 是刻意的**：状态推送的热路径（`notify::compose`）
/// 每次都要一份设置来算窗口尺寸和聚合。整份 `Settings` 深拷贝一次是几十个字段
/// 加若干张表，而推送一秒可能好几次 —— 多灯之后还要 ×N（见 docs/plan-lamp-expansion.md
/// 的 P0-2）。所以设置**改**的时候（很少）换一个新的 `Arc`，**读**的时候只克隆
/// 那个 `Arc`（一次指针拷贝）。两条路径都拿得到一份可用、不可变的快照。
type SharedSettings = Arc<Mutex<Arc<Settings>>>;

/// 瞬态的界面开关。**不是设置，所以不落盘** —— 尤其是 `pass_through`：
/// 重启后必须回到可点击状态，否则用户一开机就面对一个点不动、拖不动的圆点。
#[derive(Debug, Default, Clone)]
struct UiFlags {
    /// 点击穿透：整个窗口忽略指针事件
    pass_through: bool,
    /// 折叠态下正被鼠标指着、**临时**当展开态画的那盏灯的 label。
    ///
    /// 记 label 而不是一个 bool：多灯下悬停的是某一盏，只该展开那一盏，别的灯
    /// 一动不动。一次只可能有一盏（鼠标只有一个），所以是 `Option` 不是集合。
    /// 它**不是设置**，所以不落盘 —— 退出重开仍是用户选的折叠态。
    peek: Option<String>,
}

type SharedUi = Mutex<UiFlags>;

/// 多信号灯：组名 → 灯窗，加位置 / 尺寸记忆（见 `lamps.rs`）。
///
/// 用 `Mutex` 而不是 `Arc<Mutex>`：只有本模块（`sync_lamps` 和退出时的
/// `save_open_positions`）碰它，不像 `Settings` 那样要跨 HTTP 那条路共享。
type SharedLamps = Mutex<lamps::LampWindows>;

/// 悬浮窗几何的缓存。**存在的理由：热路径上不许问窗口。**
///
/// `apply_window_size` 在每一次状态推送里跑（一秒可能好几次），而
/// `window.scale_factor()` / `inner_size()` 都是**阻塞主线程的同步往返**
/// （tauri-runtime-wry 的 `window_getter!`）。这两样其实都能自己记住：
///
/// - `scale`：窗口创建时取一次，之后跟着 `ScaleFactorChanged` 事件更新
///   （把窗口拖到另一块缩放比不同的屏幕时会触发）；
/// - `size`：**上一次 `set_size` 下去的逻辑尺寸**。要不要 resize 只需拿新算的
///   尺寸和它比，不必回读窗口（`set_size` 是往事件循环投消息，紧接着读到的
///   还是旧值，回读本来也不可靠）。
#[derive(Debug)]
struct Geometry {
    /// 缩放比。默认 1.0 —— 万一 `wire_main_window` 没跑成，也不至于算出一个
    /// 0 倍的物理尺寸把窗口夹到左上角。
    scale: f64,
    /// 上次设下去的逻辑尺寸；`None` = 还没设过（启动后第一次必然重设一次）。
    size: Option<(u16, u16)>,
}

impl Default for Geometry {
    fn default() -> Self {
        Self { scale: 1.0, size: None }
    }
}

type SharedGeometry = Mutex<Geometry>;

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
    let ttl = report_ttl(&app, &req);
    Ok(mutate(&app, &store, |guard, now| guard.apply(now, req, ttl)))
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
    // 显式给了 `ttl_ms`，所以设置里的值压不过它（`state_ttl` 的第一级）
    let ttl = report_ttl(&app, &req);
    Ok(mutate(&app, &store, |guard, now| guard.apply(now, req, ttl)))
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
    settings.lock().expect("settings poisoned").as_ref().clone()
}

/// 每个状态**内置**的停留时长（毫秒）：`{ "working": 300000, "done": 30000, ... }`。
///
/// **为什么要有这个命令**：「逐个状态覆盖」那张表里，「跟随」那一项如果只写「跟随」两个字，
/// 用户没法知道跟随的是什么 —— 而这一列问的恰恰是「这个状态静默多久算过期」，那是他看这一列
/// 唯一想知道的数。标成「跟随 5m」就得有这个数，而它的真值在 Rust（`Signal::default_ttl`）；
/// 前端硬写一份的话，改了那边这边不会报错，只会**悄悄显示错的**。
///
/// 拿不到就退化成「跟随」两个字，不影响功能（见前端的 `useStateTtls`）。
#[tauri::command]
fn state_ttl_defaults() -> std::collections::HashMap<&'static str, u64> {
    Signal::ALL_NAMES
        .iter()
        .filter_map(|name| {
            let state = Signal::from_name(name)?;
            Some((*name, state.default_ttl().as_millis() as u64))
        })
        .collect()
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
    // 换一个新的 `Arc` 而不是就地改：读取方（状态热路径）只克隆那个 `Arc`，
    // 拿到的永远是一份**一致**的设置（见 `SharedSettings`）。这里深拷贝一次
    // 无所谓 —— 改设置是用户手点出来的低频操作。
    *app.state::<SharedSettings>().lock().expect("settings poisoned") = Arc::new(settings.clone());

    let snapshot = current_snapshot(app);
    apply_settings(app, &settings, &snapshot);
    let _ = app.emit(SETTINGS_EVENT, &settings);
    Ok(settings)
}

/// 读出当前设置。
///
/// 返回 `Arc<Settings>`：调用方多半只是拿它看一眼（算尺寸、判断静音），克隆一次
/// `Arc` 比深拷贝整份设置便宜得多，而且是给状态热路径用的（见 `SharedSettings`）。
pub(crate) fn current_settings(app: &tauri::AppHandle) -> Arc<Settings> {
    app.state::<SharedSettings>()
        .lock()
        .expect("settings poisoned")
        .clone()
}

/// 读一份当前快照。命令里要重算布局、但手上没有快照时用它（`persist_settings`
/// 那种自己顺手取的路径就不必绕这一下）。
fn current_snapshot(app: &tauri::AppHandle) -> Snapshot {
    app.state::<SharedStore>()
        .lock()
        .expect("state store poisoned")
        .snapshot()
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

/// 历史里的一天。
#[derive(serde::Serialize)]
struct DayRow {
    /// 本地日期 `YYYY-MM-DD`。**原样给前端**，格式化交给它 —— Rust 这边没有时区，
    /// 把 `2026-10-12` 加工成「10月12日」这种事只能由知道怎么显示时间的那边做。
    day: String,
    rows: Vec<StatRow>,
    /// 这一天各状态之和的秒数。
    ///
    /// 可以和，是因为统计记的是**聚合状态占用**的时间：同一时刻只有一个聚合值，
    /// 所以各状态的区间互不重叠，「和」就是这一天灯亮着多久。
    total_seconds: u64,
}

/// 过去 `days` 天（含今天）的每日合计，由近及远。
#[tauri::command]
fn get_history(
    app: tauri::AppHandle,
    stats: tauri::State<'_, SharedStats>,
    today: String,
    days: usize,
) -> Vec<DayRow> {
    /// 落盘只留 30 天（`stats::KEEP_DAYS`），再多要也没有
    const MAX_DAYS: usize = 30;

    let lang = current_lang(&app);
    stats
        .lock()
        .expect("stats poisoned")
        .recent_days(Instant::now(), &today, days.min(MAX_DAYS))
        .into_iter()
        .map(|(day, rows)| DayRow {
            day,
            total_seconds: rows.iter().map(|(_, duration)| duration.as_secs()).sum(),
            rows: rows
                .into_iter()
                .map(|(state, duration)| StatRow {
                    state,
                    label: lang::signal_label(lang, state),
                    seconds: duration.as_secs(),
                })
                .collect(),
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
        sync_layout(app, &settings, &snapshot);
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
        return Ok((*current).clone());
    }
    persist_settings(app, Settings { collapsed, ..(*current).clone() })
}

/// 灯上左键点击：折叠 ↔ 展开。前端靠 `signalhub://settings` 广播重画，
/// 这里把新设置返回去只是省掉一次多余的读取。
#[tauri::command]
fn toggle_collapsed(app: tauri::AppHandle) -> Result<Settings, String> {
    let collapsed = !current_settings(&app).collapsed;
    set_collapsed(&app, collapsed)
}

/// 折叠态被鼠标指着时**临时**展开（3s 或移开就回去）。
///
/// 只影响渲染和窗口尺寸，**不改设置** —— 退出重开仍是折叠态（这是这一项的验收
/// 之一）。`window` 由 Tauri 注入，所以多灯下只有被指着的那一盏会展开，别的灯
/// 一动不动（见 `effective_collapsed`）。
#[tauri::command]
fn set_peek(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    peek: bool,
) -> Result<(), String> {
    {
        let ui_state = app.state::<SharedUi>();
        let mut ui = ui_state.lock().expect("ui poisoned");
        if peek {
            ui.peek = Some(window.label().to_string());
        } else if ui.peek.as_deref() == Some(window.label()) {
            // **只清自己那一条**：新窗口挂载时也会发一次 `peek: false`，不能顺手把
            // 另一盏正被 hover 的灯收掉。
            ui.peek = None;
        }
    }
    // 尺寸跟着「实际折叠与否」变，所以要重算一遍所有窗口的布局。
    let settings = current_settings(&app);
    let snapshot = current_snapshot(&app);
    sync_layout(&app, &settings, &snapshot);
    Ok(())
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

    persist_settings(app, Settings { muted_groups, ..(*settings).clone() })
}

/// 清掉某一组的全部会话。
///
/// 这是「这个组的状态卡住了」的解药：上报方崩了、或者用户直接关掉了那个智能体会话、
/// 没发收尾事件时，灯会一直停在最后一个状态上，等 TTL 到期要好几分钟。清除是立刻的。
///
/// **两条入口共用它**：灯上那块牌的 `×`（命令 `clear_group_state`）和右键菜单里的
/// 「清除该组的状态」（`menu.rs`）。和 `set_group_muted` 一样的道理 —— 同一件事有两条
/// 入口时，各写一遍行为迟早分叉。
///
/// 返回清掉了几个会话，供界面报数。
pub(crate) fn clear_group(app: &tauri::AppHandle, group: &str) -> usize {
    let store = app.state::<SharedStore>();
    let (removed, changed, snapshot) = {
        let mut guard = store.lock().expect("state store poisoned");
        let (removed, changed) = guard.clear_group(group, std::time::Instant::now());
        (removed, changed, guard.snapshot())
    };

    // 一个会话都没清掉就不推前端 —— 和 Store 各处「没变化不推」的规矩一致
    if changed {
        notify::push(app, &snapshot);
    }
    removed
}

/// 把已配置的转发各发一遍（设置 →「通知转发」里的「发送测试」）。
///
/// 复用**当前**快照，忽略 `when` / 组过滤 / 跃迁判据（见 `outbound::test_all`）——
/// 用户就是想现在验证一次「这条通路通不通」，等下一次真实跃迁可能等一整天。
/// 结果照常进 `OutboundLog`，前端和真实触发读的是同一份（`get_outbound_log`）。
///
/// 返回发了几条，界面据此说「已发出 2 条」还是「还没有配好的转发」。
#[tauri::command]
fn test_hooks(app: tauri::AppHandle) -> usize {
    let settings = current_settings(&app);
    let snapshot = current_snapshot(&app);
    outbound::test_all(
        &settings,
        &snapshot,
        current_lang(&app),
        app.state::<SharedOutbound>().inner(),
    )
}

/// 清掉某一组的全部会话（灯上那块牌右上角的 `×`）。
///
/// 与右键菜单里的「清除该组的状态」走同一个函数（[`clear_group`]）。**不做二次
/// 确认**：这是低频、幂等的操作 —— 智能体下次上报就会回来，而且它本来就是
/// 「状态卡住了，赶紧清掉」这条路上的动作，加个确认框只会让紧急的事变慢。
#[tauri::command]
fn clear_group_state(app: tauri::AppHandle, group: String) -> usize {
    clear_group(&app, &group)
}

/// 清掉某一个会话（设置 →「状态」页每一行末尾的 `×`）。
///
/// 粒度比 [`clear_group_state`] 细一档：一组里跑着三个会话、卡住一个的时候，
/// 不必把其它两个也清掉。HTTP 的 `DELETE /v1/sessions/{id}` 用的是同一个
/// `Store::remove_session`。
///
/// 清一个不存在的会话**静默幂等**（返回 `false`，不报错）—— HTTP 那条已经在依赖
/// 这个语义，两条入口不该有两种脾气。
#[tauri::command]
fn clear_session(app: tauri::AppHandle, session: String) -> bool {
    let store = app.state::<SharedStore>();
    let (removed, changed, snapshot) = {
        let mut guard = store.lock().expect("state store poisoned");
        let (removed, changed) = guard.remove_session(&session, std::time::Instant::now());
        (removed, changed, guard.snapshot())
    };
    if changed {
        notify::push(&app, &snapshot);
    }
    removed
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
    /// 配置里**实际写着**的那条命令（不是我们现在会写的那条）。
    /// `None` = 没装，或者那个文件读不出来 / 认不出我们的 handler。
    command: Option<String>,
    /// 那条命令的程序现在还解析得到吗。
    ///
    /// **`installed && !command_ok` 就是一次静默失效的接入** —— 智能体不报错
    /// （它只是执行了一个不存在的程序，然后照常继续），灯也不亮。这是「装了却没
    /// 反应」里最值得主动指出的一种，所以把它算在服务端、摆到界面上。
    command_ok: bool,
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
        .map(|(id, agent)| {
            // `installed_command` 要读一次配置文件，所以只算一次
            let command = hooks::installed_command(agent);
            let command_ok = command.as_deref().is_some_and(hooks::command_resolves);
            AgentStatus {
                id,
                name: agent.name(),
                detected: hooks::detect(agent),
                installed: hooks::is_installed(agent),
                config: hooks::config_path(agent)
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                command,
                command_ok,
            }
        })
        .collect()
}

/// 最近的 hook 调用记录，**最新的在后**。
///
/// 直接读那个文件的尾巴，不解析成结构体：它本来就是给人看的（`append_hook_log`
/// 写的是定宽的人类可读行），解析一遍只会多出一处会跟格式漂移的地方 —— 而格式
/// 一漂，界面上就会安静地少几行，比不显示更糟。文件本身有 512KB 的轮转上限。
#[tauri::command]
fn get_hook_log(limit: usize) -> Vec<String> {
    /// 一次最多给这么多行：这个文件是给人扫一眼的，不是拿来做分析的
    const MAX_LINES: usize = 200;

    let Ok(path) = config::hook_log_path() else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        // 一次都没跑过 hook 时文件还不存在 —— 那是正常状态，不是错误
        return Vec::new();
    };
    let mut lines: Vec<String> = text
        .lines()
        .rev()
        .take(limit.min(MAX_LINES))
        .map(str::to_string)
        .collect();
    lines.reverse();
    lines
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
    /// 插件式接入（`hooks::InstallKind::PluginFile`）写下的那个文件。
    ///
    /// 界面据此改口成「已写入 <路径>」：JSON 类是「新增 N 个事件」，而插件类没有
    /// 事件清单这回事，说「新增 1 个事件」对不上用户能去 `cat` 的那个东西。
    plugin: Option<String>,
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
            plugin: report
                .plugin
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

/// 某一组的「项目目录」：决定聚合那个会话的 `cwd`，且必须**还是个真目录**。
///
/// 两个用途共用这一条判断：组右键菜单里那一项要不要置灰（`menu.rs`），以及点下去
/// 时打开哪儿。`None` = 这一组没有可打开的项目目录。
pub(crate) fn group_project_dir(app: &tauri::AppHandle, group: &str) -> Option<String> {
    let store = app.state::<SharedStore>();
    let cwd = store.lock().expect("state store poisoned").winner_cwd_in(group)?;
    std::path::Path::new(&cwd).is_dir().then_some(cwd)
}

/// 在灯上某一组的位置弹出右键菜单。
///
/// `window` 是**发起右键的那个灯窗**（多灯模式下可能是某个组灯）。Tauri 会把调用
/// 来源窗口注入进来，菜单因此弹在用户真正点的那盏灯上。
#[tauri::command]
fn open_group_menu(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    group: String,
) -> Result<(), String> {
    menu::show(&app, &window, &group)
}

/// 在灯的空白处（或还没接上报方时那块占位牌）弹出通用菜单。
///
/// 与 `open_group_menu` 分开是必须的：占位牌不对应任何真实的组，传一个空组名
/// 过去只会得到一张「隐藏/静音/清除那个叫 "" 的组」的菜单，点哪个都不会发生
/// 任何事 —— 而它恰恰是首次安装时灯上唯一能右键的地方。
#[tauri::command]
fn open_lamp_menu(app: tauri::AppHandle, window: tauri::WebviewWindow) -> Result<(), String> {
    menu::show_general(&app, &window)
}

/// 前端推过来的**解析后**的界面语言（`"auto"` 由前端解析 —— Rust 不知道系统语言）。
///
/// 原生界面（托盘菜单、右键菜单、通知按钮、窗口标题）不经过 webview，前端换不了，
/// 所以语言一变就得把这几样重建一遍。收在 `apply_lang` 一个函数里，免得以后新增
/// 一处原生文案时忘了加进来 —— 漏掉的表现是界面上混着两种语言，而那只有肉眼能发现。
#[tauri::command]
fn set_lang(app: tauri::AppHandle, lang: String) -> Result<(), String> {
    let Some(lang) = config::Lang::parse(&lang) else {
        return Err(format!("不认识的语言 {lang:?}；可用 zh / en"));
    };
    app.state::<lang::LocalLang>().set(lang);
    // 落一份给 CLI 读：它是独立进程，挑不了自己那些文案的语言（`auto` 的解析只有
    // 前端做得了）。见 `config::lang_file`。
    if let Err(err) = config::write_lang(lang) {
        eprintln!("[signalhub] 写语言文件失败: {err}（CLI 的文案会退回按环境变量猜）");
    }
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
pub(crate) fn current_lang(app: &tauri::AppHandle) -> config::Lang {
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
    // 主灯和每个组灯都要跟着「总在最前」走 —— 只改主灯的话，切到多灯之后
    // 组灯会一直是最前的（它们是按建窗时的设置建的）。
    for (label, window) in app.webview_windows() {
        if label == "main" || label.starts_with(lamps::LABEL_PREFIX) {
            let _ = window.set_always_on_top(settings.always_on_top);
        }
    }
    sync_layout(app, settings, snapshot);
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

/// 可见组的名字，**不截断**到 `MAX_SIGNS` —— 多灯模式下每组一盏灯，截断没有意义。
///
/// 顺序和 `visible_groups` 一致（置顶在前），只是这里要的是名字。
fn visible_group_names(settings: &Settings, snapshot: &Snapshot) -> Vec<String> {
    let mut groups: Vec<&GroupView> = snapshot
        .groups
        .iter()
        .filter(|group| settings.shows_group(&group.name))
        .collect();
    groups.sort_by_key(|group| match settings.group_order(&group.name) {
        Some(order) => (0, order),
        None => (1, 0),
    });
    groups.into_iter().map(|group| group.name.clone()).collect()
}

/// 某个灯窗这一帧**实际**是不是折叠态：设置里折叠了，且它没在被悬停临时展开。
///
/// `peek` 只是「临时当展开态画」，**不改设置** —— 退出重开仍是折叠态。
///
/// **主灯（`apply_window_size`）和组灯（`lamp_specs`）都必须用它**，而且前端渲染
/// 也必须按同一条件切 —— 前端画展开、窗口按折叠算（或反过来）就会被
/// `overflow: hidden` 裁掉一块（红线 1）。
fn effective_collapsed(app: &tauri::AppHandle, settings: &Settings, label: &str) -> bool {
    if !settings.collapsed {
        return false;
    }
    let peeking = app
        .state::<SharedUi>()
        .lock()
        .expect("ui poisoned")
        .peek
        .as_deref()
        == Some(label);
    !peeking
}

/// 这一帧是否走「每组分灯」。
///
/// 两个条件缺一不可：布局选的是 `PerGroup`，且可见组数没超过硬上限。超过就**回落
/// 单灯**（各组仍画在主灯上）—— 8 个 webview 已经不少了，再多不该默默开下去。
///
/// **前端必须按同一条规则算**（`src/lib/lampLayout.ts` 的 `perGroupActive`），否则
/// 主灯一边以为要画全局、一边以为要画各组，窗口尺寸就会对不上。
fn per_group_active(settings: &Settings, snapshot: &Snapshot) -> bool {
    // 先看便宜的枚举，隔开一次不必要的计数 —— 默认的单灯模式下这里是零成本
    settings.lamp_layout == LampLayout::PerGroup
        && visible_group_count(settings, snapshot) <= lamps::MAX_LAMPS
}

/// 可见组**个数**。只数不排队、不拷名字：`per_group_active` 在热路径上被问好几次，
/// 为了数个数去排序 + 拷一串 `String` 不值当。
fn visible_group_count(settings: &Settings, snapshot: &Snapshot) -> usize {
    snapshot
        .groups
        .iter()
        .filter(|group| settings.shows_group(&group.name))
        .count()
}

/// 主灯这一帧该画哪些组。
///
/// 每组分灯时主灯退出「一叠牌」的角色，只留全局 —— 传空就等于走 `SignalLamp` 现有
/// 的占位 / 全局分支（和前端 `App.tsx` 的同一判断对齐）。
fn main_window_groups<'a>(settings: &Settings, snapshot: &'a Snapshot) -> Vec<&'a GroupView> {
    if per_group_active(settings, snapshot) {
        Vec::new()
    } else {
        visible_groups(settings, snapshot)
    }
}

/// 每个可见组该有的灯窗：组名 + 这一帧的尺寸。
///
/// 一盏组灯的几何 = 「只有这一组的单灯」：展开态是一块牌，折叠态是一个点（外加
/// 暂停 / 穿透那枚角标）。**必须和前端渲染对齐** —— 多一块牌或少一枚角标，窗口
/// 就会裁掉内容。
fn lamp_specs(app: &tauri::AppHandle, settings: &Settings, snapshot: &Snapshot) -> Vec<lamps::LampSpec> {
    if !per_group_active(settings, snapshot) {
        return Vec::new();
    }
    let flags = ui_flag_count(app);
    visible_group_names(settings, snapshot)
        .into_iter()
        .map(|name| {
            // 折叠 / 展开按**这个灯窗**自己被 hover 与否算（多灯下各算各的）
            let collapsed = effective_collapsed(app, settings, &lamps::label_for(&name));
            let has_message = !collapsed
                && settings.show.message
                && snapshot
                    .groups
                    .iter()
                    .find(|group| group.name == name)
                    .is_some_and(|group| group.message.is_some());
            let size = if collapsed {
                config::collapsed_window_size(settings.size, 1, flags + config::more_badge(1))
            } else {
                settings.window_size(&[config::sign_height(settings.size, has_message, settings.sign_h_mult)])
            };
            lamps::LampSpec { group: name, size }
        })
        .collect()
}

/// 让灯窗集合跟上这一帧。单灯模式下 `lamps::MAX_LAMPS` 那个分支不成立，`specs` 为空，
/// 于是所有组灯被关掉 —— 从多灯切回单灯自然也走这条路。
fn sync_lamps(app: &tauri::AppHandle, settings: &Settings, snapshot: &Snapshot) {
    let specs = lamp_specs(app, settings, snapshot);
    let handle = app.state::<SharedLamps>();
    let mut lamps = handle.lock().expect("lamps poisoned");
    lamps.reconcile(&lamps::TauriHost(app), &specs);
}

/// 一次状态 / 设置变化之后，把**所有**窗口的布局同步一遍：主灯尺寸 + 各组的灯。
fn sync_layout(app: &tauri::AppHandle, settings: &Settings, snapshot: &Snapshot) {
    apply_window_size(app, settings, snapshot);
    sync_lamps(app, settings, snapshot);
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
                settings.sign_h_mult,
            )
        })
        .collect();

    // 一块牌都没有时也要留一块（表示 offline，或者用户把所有组都藏了），
    // 否则窗口会塌成一条缝 —— 那样既看不见状态，也没法拖它
    if heights.is_empty() {
        vec![config::sign_height(settings.size, false, settings.sign_h_mult)]
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
///
/// **热路径上不问窗口**（见 `SharedGeometry`）：缩放比和上次设下去的尺寸都
/// 从缓存读，一次 `inner_size` / `scale_factor` 的同步往返都不做。
fn apply_window_size(app: &tauri::AppHandle, settings: &Settings, snapshot: &Snapshot) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };

    // 展开态是一叠牌，折叠态是一排点 —— 两套几何，见 config.rs。
    // 多灯模式下主灯只看全局（各组交给各自的灯），所以这里拿的是「主灯该画的组」。
    let groups = main_window_groups(settings, snapshot);
    // 悬停临时展开也算「不折叠」（见 `effective_collapsed`）—— 前端会按同一个条件
    // 渲染，两边不一致就会裁内容。
    let collapsed = effective_collapsed(app, settings, "main");
    let (width, height) = if collapsed {
        // 折叠态里没有牌面，全局状态角标和「还有几组」的 `+N` 都得自己占一个点位。
        // 少算一枚，那枚角标就落在窗口外面 —— 而折叠态最不该出的就是这种错：
        // 看不见的东西，人不会想到去查。
        let badges = ui_flag_count(app) + config::more_badge(groups.len());
        config::collapsed_window_size(settings.size, groups.len(), badges)
    } else {
        settings.window_size(&sign_heights(settings, &groups))
    };

    // 和上次设下去的尺寸一样就收工。**这是热路径**：拿缓存比，不问窗口。
    let (previous, scale) = {
        let handle = app.state::<SharedGeometry>();
        let mut geometry = handle.lock().expect("geometry poisoned");
        if geometry.size == Some((width, height)) {
            return;
        }
        let previous = geometry.size;
        geometry.size = Some((width, height));
        (previous, geometry.scale)
    };

    let _ = window.set_size(LogicalSize::new(width, height));
    // 尺寸变了才可能越界，所以放在这个分支里
    keep_on_screen(&window, width, height, previous, scale);
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
/// `scale` 由调用方传入（缓存的那份），这里不再问窗口 —— 见 `SharedGeometry`。
///
/// 用工作区（`work_area`）而不是整块屏幕：任务栏 / Dock 占掉的那一条不算可用空间，
/// 按整屏算的话，贴着底边的窗口会有一截压在任务栏底下。
///
/// `previous` 是**变化前**的逻辑尺寸。贴着右 / 下边缘的窗口按它把位置让出来，好让
/// 窗口**往屏幕里长**（而不是从左上角长出去、再被这里夹回来）—— 见
/// `grow_away_from_edges`。
fn keep_on_screen(
    window: &tauri::WebviewWindow,
    width: u16,
    height: u16,
    previous: Option<(u16, u16)>,
    scale: f64,
) {
    let Ok(position) = window.outer_position() else {
        // 拿不到位置就什么都不做 —— 这只是个兜底，失败不该有任何副作用
        return;
    };
    // 窗口盖在哪块屏上；拿不到就退回「离它最近的那块」。
    //
    // **这一条的退路必须有**：拔掉外接屏之后，窗口的坐标还留在那块不存在的屏上，
    // 而 `current_monitor()` 对「一块屏都没盖住」的窗口给 `None` —— 原来直接
    // `return`，于是这条兜底在**最需要它的时候**（正是屏没了的时候）恰好失效，
    // 灯永远留在屏幕外，用户再也点不到它。
    let bounds = match window.current_monitor() {
        Ok(Some(monitor)) => work_bounds(&monitor),
        // 屏不见了（拔掉外接屏 / 改分辨率 / 换布局）：`current_monitor()` 给不出屏，
        // 就退回「离它最近的那一块」。这条路平时不走，所以留一行记录 —— 「我的灯
        // 怎么跑到这边来了」得有个地方查。
        _ => match nearest_screen(window.app_handle(), (position.x, position.y)) {
            Some((bounds, _)) => {
                config::append_hook_log(&format!(
                    "{} [lamp] 当前屏幕不可用，({}, {}) 就近回到 {bounds:?}\n",
                    config::utc_hms(),
                    position.x,
                    position.y,
                ));
                bounds
            }
            None => return,
        },
    };

    let to_physical = |size: (u16, u16)| {
        (
            (size.0 as f64 * scale).round() as i32,
            (size.1 as f64 * scale).round() as i32,
        )
    };
    let size = to_physical((width, height));

    // 先按「贴边就往里长」挪，再夹回区内兜底（窗口比工作区还大时只有夹这一步管用）
    let anchored = match previous {
        Some(previous) => grow_away_from_edges(
            (position.x, position.y),
            to_physical(previous),
            size,
            bounds,
            EDGE_ANCHOR_PX,
        ),
        None => (position.x, position.y),
    };
    let (x, y) = clamp_to_area(anchored, size, bounds);
    if (x, y) != (position.x, position.y) {
        let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
    }
}

/// 尺寸变了之后，把左上角挪一下，让窗口**往屏幕里长**：本来贴着右 / 下边缘的窗口，
/// 保持那条边不动（内容就不会长到屏幕外），**缩回去时再挪回来**。
///
/// `previous` / `size` 是变化前 / 后的物理尺寸。只挪贴边的那一维；不在边缘的窗口
/// 一个像素都不动（照旧从左上角长）。
///
/// **为什么不能只靠 `clamp_to_area` 兜底**：夹只会把越界的窗口推回区内，而**缩回去
/// 时它不会推回来** —— 于是「悬停临时展开」这种一伸一缩的操作会一次比一次把灯往屏幕
/// 里挪（第一次悬停就往左跳几百像素，之后再也不回去）。让伸缩对称，就不会有这种漂移。
fn grow_away_from_edges(
    position: (i32, i32),
    previous: (i32, i32),
    size: (i32, i32),
    bounds: ((i32, i32), (i32, i32)),
    margin: i32,
) -> (i32, i32) {
    let ((_left, _top), (right, bottom)) = bounds;
    let (mut x, mut y) = position;
    let delta_w = size.0 - previous.0;
    let delta_h = size.1 - previous.1;
    // 贴着右边缘：右边不动 ⇒ 左上角让出宽度变化
    if delta_w != 0 && x + previous.0 >= right - margin {
        x -= delta_w;
    }
    // 贴着下边缘：底边不动
    if delta_h != 0 && y + previous.1 >= bottom - margin {
        y -= delta_h;
    }
    (x, y)
}

/// 把一个窗口位置夹进工作区。返回夹紧后的左上角。
///
/// `bounds` 是 `((左, 上), (右, 下))`。抽成纯函数是为了能单测 —— 边界情况（窗口
/// 比工作区还大、副屏在左侧时的负坐标）是这里最容易写错的地方，而它们在真机上
/// 很难复现。
///
/// **组灯也用它**（`lamps.rs::TauriHost::clamp_to_visible`）：那边记着的位置同样会
/// 落在已经不在了的屏幕上，判据必须和主灯的 `keep_on_screen` 是同一套，否则会出现
/// 「主灯拉回来了、组灯还丢在屏外」。
pub(crate) fn clamp_to_area(
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

/// 一块屏的工作区，`((左, 上), (右, 下))`，物理像素 —— 和 `clamp_to_area` /
/// `snap_to_edges` 是同一套坐标准则。
pub(crate) type Bounds = ((i32, i32), (i32, i32));

/// 某块屏的工作区（物理像素）。任务栏 / Dock 占掉的那条不算可用空间。
pub(crate) fn work_bounds(monitor: &tauri::Monitor) -> Bounds {
    let area = monitor.work_area();
    (
        (area.position.x, area.position.y),
        (
            area.position.x + area.size.width as i32,
            area.position.y + area.size.height as i32,
        ),
    )
}

/// 点到矩形（闭区间）的距离平方。点在矩形内为 0。用来「挑离它最近的那块屏」。
fn distance_to_bounds(((left, top), (right, bottom)): Bounds, (x, y): (i32, i32)) -> i64 {
    let dx = i64::from((left - x).max(0).max(x - right));
    let dy = i64::from((top - y).max(0).max(y - bottom));
    dx * dx + dy * dy
}

/// 在若干块屏里挑离 `at` 最近的那块，返回下标（并列时取先出现的）。
fn nearest_index(areas: &[Bounds], at: (i32, i32)) -> Option<usize> {
    (0..areas.len()).min_by_key(|index| distance_to_bounds(areas[*index], at))
}

/// 离 `at` 最近的那块屏的工作区 + 缩放比。
///
/// **为什么不直接用 `current_monitor()`**：那个函数问的是「窗口盖在哪块屏上」，而
/// 窗口的坐标可能落在一块**已经不在了**的屏上（拔掉外接屏 / 改分辨率 / 换布局）——
/// 那一刻它给 `None`，于是「把窗口拉回屏幕内」这条兜底会整条失效，灯就永远留在
/// 屏幕外。这里换成「点到矩形的距离」，任何坐标都挑得出一块最近的屏。
///
/// 主灯（`keep_on_screen`）和组灯（`lamps.rs`）共用它，两边的判据必须一致。
/// 拿不到显示器信息（无头环境、多屏拔插的瞬间）返回 `None`。
pub(crate) fn nearest_screen(app: &tauri::AppHandle, at: (i32, i32)) -> Option<(Bounds, f64)> {
    let monitors = app.available_monitors().ok()?;
    let areas: Vec<Bounds> = monitors.iter().map(work_bounds).collect();
    let index = nearest_index(&areas, at)?;
    Some((areas[index], monitors[index].scale_factor()))
}

/// 吸附判定：窗口边缘离工作区边缘这么近就贴齐（物理像素）。
const SNAP_THRESHOLD_PX: i32 = 12;

/// 贴边判定的余量：窗口边和屏幕边差这么多以内就算「贴着那条边」（物理像素）。
const EDGE_ANCHOR_PX: i32 = 24;

/// 停多久算「拖完了」。`WindowEvent::Moved` 在拖动过程中一路都在发，所以不能一收到
/// 就吸 —— 得等它安静下来。见 `wire_snap`。
const SNAP_SETTLE_MS: u64 = 220;

/// 把 `position` 吸到最近的**左 / 右**工作区边缘，够近才吸。`size` / `bounds` 与
/// `clamp_to_area` 是同一套（物理像素、工作区）。
///
/// **只动 `x`**：上下不吸 —— 任务栏 / Dock 在上下，吸过去会和 `keep_on_screen` 抢
/// 地方。窗口比工作区还宽时贴左，和 `clamp_to_area` 的兜底一致。
///
/// 抽成纯函数是为了能单测：左右都够近、都不够近、窗口有一半在屏外这些边界，真机上
/// 很难摆出来。
///
/// **必须吸到工作区边缘**（`clamp_to_area` 用的也是工作区）：吸到一个它不认可的
/// 位置，尺寸一变就会被拉回来 —— 那就是要避免的「吸过去又被弹开」。
fn snap_to_edges(
    position: (i32, i32),
    size: (i32, i32),
    bounds: ((i32, i32), (i32, i32)),
    threshold: i32,
) -> (i32, i32) {
    let ((left, _top), (right, _bottom)) = bounds;
    let (x, y) = position;
    if (x - left).abs() <= threshold {
        // 左边缘够近（含已经拖出屏外一点的情况，此时把它拉回来贴齐）
        (left, y)
    } else if (right - (x + size.0)).abs() <= threshold {
        // 右边缘够近。(right - width).max(left) 与 clamp_to_area 的兜底同款
        ((right - size.0).max(left), y)
    } else {
        (x, y)
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

/// 这次上报该活多久：**显式给的 > 用户为这个状态设的 > 内置默认**（见 `state_ttl`）。
///
/// 单独一个函数而不是把设置传进 `apply`：`apply` 是在**持着状态锁**的时候调的，
/// 而设置是另一个锁 —— 在那里再去拿它，就多了一处锁序要维护。
fn report_ttl(app: &tauri::AppHandle, req: &Report) -> Duration {
    state_ttl(req.state, req.ttl_ms, &current_settings(app))
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

/// 记下主窗口的缩放比，之后跟着 DPI 变化更新。
///
/// 缩放比原本每次推送都要问窗口（`scale_factor()` 是**阻塞主线程的同步往返**），
/// 而 `apply_window_size` 在状态热路径上。取一次、随 `ScaleFactorChanged` 更新，
/// 热路径上就再也不用问窗口了（见 `SharedGeometry`）。
///
/// 只在启动时调一次；`apply_settings` 必须排在它后面 —— 否则第一次算尺寸时缓存里
/// 还是默认的 1.0，在 2× 屏上会把窗口的物理尺寸算错一半。
fn wire_main_window(app: &tauri::App) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };

    let scale = window.scale_factor().unwrap_or(1.0);
    app.state::<SharedGeometry>()
        .lock()
        .expect("geometry poisoned")
        .scale = scale;

    let app_handle = app.handle().clone();
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::ScaleFactorChanged { scale_factor, .. } = event {
            app_handle
                .state::<SharedGeometry>()
                .lock()
                .expect("geometry poisoned")
                .scale = *scale_factor;
        }
    });

    wire_snap(app.handle(), &window);
}

/// 拖完之后贴边吸附。**只吸左右**，且要用户在设置里打开（见 `Settings::snap_to_edge`）。
///
/// 「拖完了」为什么不用前端 mouseup：原生拖动（`startDragging`）由系统接管鼠标，
/// mouseup 会被拖动自己的事件循环吃掉、未必回得到 webview。改成听 `Moved` —— 拖动
/// 过程中它一路都在发，那就**等它安静 `SNAP_SETTLE_MS`**：这期间没有新的 `Moved`
/// 才动手（靠后台线程醒来后比一次「位置还是不是刚才那个」来判断）。
fn wire_snap(app: &tauri::AppHandle, window: &tauri::WebviewWindow) {
    let app = app.clone();
    // 闭包里要用 `window`，但调用 `on_window_event` 本身又借着它 —— 所以先单独克隆
    // 一份给闭包用，别去动外面的 `window`。
    let handle = window.clone();
    window.on_window_event(move |event| {
        let tauri::WindowEvent::Moved(moved) = event else {
            return;
        };
        if !current_settings(&app).snap_to_edge {
            return;
        }
        // 先记下候选位置、丢到后台线程去等：事件回调在事件循环上，在这儿 `sleep`
        // 会把整个界面卡住。
        let moved = (moved.x, moved.y);
        let app = app.clone();
        let window = handle.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(SNAP_SETTLE_MS));
            // 这段里又移动过 = 还在拖，放过这一次（等下一轮安静下来）
            let Ok(current) = window.outer_position() else {
                return;
            };
            if (current.x, current.y) != moved {
                return;
            }
            // 设置可能就在这 220ms 里被关掉了
            if !current_settings(&app).snap_to_edge {
                return;
            }
            snap_window(&window);
        });
    });
}

/// 把这个窗口吸到最近的左 / 右工作区边缘（够近才吸）。几何判断在 `snap_to_edges`。
fn snap_window(window: &tauri::WebviewWindow) {
    let (Ok(Some(monitor)), Ok(position), Ok(size)) = (
        window.current_monitor(),
        window.outer_position(),
        window.outer_size(),
    ) else {
        // 拿不到显示器信息（多屏拔插的瞬间、无头环境）就什么都不做
        return;
    };

    let area = monitor.work_area();
    let bounds = (
        (area.position.x, area.position.y),
        (
            area.position.x + area.size.width as i32,
            area.position.y + area.size.height as i32,
        ),
    );
    let size = (size.width as i32, size.height as i32);
    let snapped = snap_to_edges((position.x, position.y), size, bounds, SNAP_THRESHOLD_PX);
    if snapped != (position.x, position.y) {
        let _ = window.set_position(tauri::PhysicalPosition::new(snapped.0, snapped.1));
    }
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
        .manage(SharedGeometry::default())
        // 位置记忆从盘上读回来（`LampWindows::load`），这样组灯一开就在上次的位置
        .manage(Mutex::new(lamps::LampWindows::load()))
        .manage(SharedBind::default())
        // 统计从盘上读回来：灯球是常驻的，但「常驻」不等于「永不重启」——
        // 改设置、升级、崩了重来，用户对「今天等了多久」的期待是跨重启的。
        .manage(Arc::new(Mutex::new(stats::Stats::load())))
        // 启动时把上次的转发结果读回来：日志原来是纯内存的，重启一次界面就显示
        // 「从未触发」—— 用户重启一次就会以为转发坏了。
        .manage(outbound::load_log())
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
            state_ttl_defaults,
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
            set_peek,
            get_stats,
            get_history,
            get_timeline,
            get_messages,
            get_outbound_log,
            get_notify_permission,
            request_notify_permission,
            detect_agents,
            get_hook_log,
            install_agent_hooks,
            uninstall_agent_hooks,
            toggle_collapsed,
            clear_group_state,
            clear_session,
            test_hooks,
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
            app.manage(Arc::new(Mutex::new(Arc::new(settings.clone()))));

            // **先取一次缩放比**（唯一的同步查询），再套用尺寸：否则
            // `apply_window_size` 会拿默认的 1.0 去算物理尺寸，在 2× 屏上把
            // 窗口夹到错的位置。之后热路径只用缓存的值。
            wire_main_window(app);

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
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // 退出前把灯窗的位置落盘：用户拖完最后一个动作就退出时，那点位移还只在
            // 窗口里（见 `lamps::save_open_positions`）。
            if let tauri::RunEvent::Exit = event {
                lamps::save_open_positions(app);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::{
        clamp_to_area, distance_to_bounds, grow_away_from_edges, nearest_index,
        pass_through_shortcut_label, shortcut_label,
        shortcut_label_for, snap_to_edges, validate_settings, visible_groups, KeyStyle,
        state_ttl_defaults, PASS_THROUGH_SHORTCUT, TEST_REPORT_TTL_MS,
    };
    use crate::config::{GroupMeta, Settings, StateStyle, MAX_STATE_TTL_MS, MIN_STATE_TTL_MS};
    use crate::state::{state_ttl, GroupView, Report, Signal, Snapshot, Store};
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
            // 显式 TTL 是最高那一级，所以这里给个空设置也走它
            state_ttl(Signal::Permission, Some(TEST_REPORT_TTL_MS), &Settings::default()),
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

    // ---- 停留时长的内置默认 ---------------------------------------------------

    /// 「跟随 5m」里那个 `5m` 由后端给（`state_ttl_defaults`），前端不存第二份。
    ///
    /// 钉两件事：**每个状态都有**（漏一个，那一格就只能显示成「跟随」两个字 ——
    /// 而这一列问的恰恰是「到底多久」），以及**每个内置值都落在用户能选的区间里**
    /// （否则「跟随」和手选档位分属两个世界：一个被 `clamped()` 夹过，一个夹不到）。
    #[test]
    fn ttl_defaults_cover_every_state_and_fit_the_range() {
        let defaults = state_ttl_defaults();
        assert_eq!(defaults.len(), Signal::ALL_NAMES.len(), "每个状态都要有一档");
        assert_eq!(defaults["working"], 300_000);
        assert_eq!(defaults["done"], 30_000);
        for (name, ms) in &defaults {
            assert!(
                (u64::from(MIN_STATE_TTL_MS)..=u64::from(MAX_STATE_TTL_MS)).contains(ms),
                "{name} 的内置停留时长 {ms}ms 不在用户可选的区间里"
            );
        }
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

    // ---- 贴边吸附 -------------------------------------------------------------

    /// 离边缘还远就不动 —— 吸附是「够近才吸」，不是「总要贴一个边」。
    #[test]
    fn nothing_snaps_far_from_the_edges() {
        assert_eq!(snap_to_edges((960, 500), WINDOW, SCREEN, 12), (960, 500));
        // 差一像素超过阈值也不吸（阈值是「以内」）
        assert_eq!(snap_to_edges((13, 500), WINDOW, SCREEN, 12), (13, 500));
    }

    /// 够近就贴齐，而且**只动 x** —— 上下不吸（任务栏 / Dock 在那儿）。
    #[test]
    fn near_the_left_or_right_edge_snaps_flat() {
        assert_eq!(snap_to_edges((5, 500), WINDOW, SCREEN, 12), (0, 500));
        assert_eq!(snap_to_edges((12, 500), WINDOW, SCREEN, 12), (0, 500), "正好在阈值上也吸");
        assert_eq!(snap_to_edges((1712, 500), WINDOW, SCREEN, 12), (1720, 500), "右边贴齐");
        assert_eq!(snap_to_edges((100, 7), WINDOW, SCREEN, 12), (100, 7), "上下不吸");
    }

    /// 拖出屏外一点（左 / 右）也拉回来贴齐，而不是留在外面。
    #[test]
    fn a_window_hanging_off_an_edge_is_snapped_back_in() {
        assert_eq!(snap_to_edges((-8, 500), WINDOW, SCREEN, 12), (0, 500));
        assert_eq!(snap_to_edges((1732, 500), WINDOW, SCREEN, 12), (1720, 500));
    }

    /// 窗口比工作区还宽时贴左，和 `clamp_to_area` 的兜底一致（别 panic）。
    #[test]
    fn a_window_wider_than_the_area_snaps_left() {
        assert_eq!(snap_to_edges((5, 500), (4000, 3000), SCREEN, 12), (0, 500));
    }

    /// 吸完的位置必须**同时**是 `clamp_to_area` 的不动点 —— 否则尺寸一变，
    /// `keep_on_screen` 就会把窗口从吸好的边上拉走，变成「吸过去又弹开」。
    #[test]
    fn a_snapped_window_is_not_pulled_back_by_keep_on_screen() {
        for position in [(-8, 300), (5, 300), (1712, 300), (1732, 300), (700, 300)] {
            let snapped = snap_to_edges(position, WINDOW, SCREEN, 12);
            assert_eq!(
                clamp_to_area(snapped, WINDOW, SCREEN),
                snapped,
                "{position:?} 吸到 {snapped:?} 之后又被夹了"
            );
        }
    }

    // ---- 挑屏（灯不能丢在屏幕外）----------------------------------------------

    /// 挑最近的屏靠这个：点在矩形内为 0，在外就是到最近边的平方距离。
    #[test]
    fn distance_to_bounds_is_zero_inside_and_squared_outside() {
        let area = ((0, 0), (1920, 1080));
        assert_eq!(distance_to_bounds(area, (500, 500)), 0, "区内为 0");
        assert_eq!(distance_to_bounds(area, (0, 0)), 0, "边界也算在内");
        assert_eq!(distance_to_bounds(area, (1920, 1080)), 0);
        assert_eq!(distance_to_bounds(area, (-30, 500)), 900, "左边出去 30 → 30²");
        assert_eq!(distance_to_bounds(area, (1950, 500)), 900, "右边出去 30");
        assert_eq!(distance_to_bounds(area, (500, -20)), 400, "上边出去 20");
        assert_eq!(distance_to_bounds(area, (500, 1100)), 400, "下边出去 20");
        assert_eq!(distance_to_bounds(area, (-3, -4)), 25, "角上按勾股");
    }

    /// 一块**已经不在了**的屏上那个坐标，必须挑一块最近的屏 —— 这就是「灯丢在
    /// 屏幕外」那条 bug 的判据本身：`current_monitor()` 对这种情况给 `None`，
    /// 而 `nearest_index` 总给得出一块。
    #[test]
    fn nearest_index_picks_the_closest_screen() {
        // 主屏 (0,0)-(1920,1080)，副屏挂在**左边** (-1920,0)-(0,1080)
        let areas = [((0, 0), (1920, 1080)), ((-1920, 0), (0, 1080))];
        assert_eq!(nearest_index(&areas, (500, 500)), Some(0), "主屏正中");
        assert_eq!(nearest_index(&areas, (-500, 500)), Some(1), "左副屏正中");
        assert_eq!(nearest_index(&areas, (-1900, 540)), Some(1), "左副屏边缘内");
        // 右边那块拔掉之后留下的坐标：离主屏最近，于是被夹回主屏
        assert_eq!(nearest_index(&areas, (9000, 500)), Some(0));
        assert_eq!(nearest_index(&areas, (-2000, 540)), Some(1), "只出界一点，还是副屏");
        assert_eq!(nearest_index(&areas, (-3000, 540)), Some(1), "更左仍然是副屏最近");
        assert_eq!(nearest_index(&[], (0, 0)), None, "一块屏都没有");
    }

    // ---- 贴边生长 -------------------------------------------------------------

    /// 贴右边缘的窗口变大时，左上角往左让，右边不动 —— 内容不会长到屏幕外。
    #[test]
    fn growing_at_the_right_edge_keeps_the_right_edge_still() {
        // 收起态在右边缘，右边 1920 贴着屏幕右边
        let grown = grow_away_from_edges((1860, 500), (60, 60), (400, 60), SCREEN, 24);
        assert_eq!(grown, (1520, 500));
        assert_eq!(grown.0 + 400, 1920, "右边必须还在屏幕边上");
    }

    /// **缩回去必须挪回来**：不然「悬停临时展开」一次就把灯往屏幕里推一次。
    #[test]
    fn shrinking_back_returns_to_where_it_was() {
        let there = (1860, 500);
        let grown = grow_away_from_edges(there, (60, 60), (400, 60), SCREEN, 24);
        let shrunk = grow_away_from_edges(grown, (400, 60), (60, 60), SCREEN, 24);
        assert_eq!(shrunk, there, "一伸一缩必须回到原地");
    }

    /// 不在边缘的窗口一个像素都不动 —— 既有行为不变。
    #[test]
    fn growing_away_from_the_edges_does_not_move() {
        assert_eq!(
            grow_away_from_edges((700, 300), WINDOW, (400, 200), SCREEN, 24),
            (700, 300)
        );
    }

    /// 贴下边缘同理（底边不动）。
    #[test]
    fn growing_at_the_bottom_edge_keeps_the_bottom_still() {
        let grown = grow_away_from_edges((500, 980), (60, 60), (60, 260), SCREEN, 24);
        assert_eq!(grown, (500, 780));
    }

    // ---- 置顶排序 -------------------------------------------------------------

    fn group(name: &str, aggregate: Signal) -> GroupView {
        GroupView {
            name: name.to_string(),
            // 这些用例只关心牌的**顺序**，不关心牌上显示成什么
            default_label: None,
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
