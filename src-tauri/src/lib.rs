mod agents;
mod alerts;
mod approval_log;
mod approvals;
mod callme;
mod callme_local;
mod collect;
mod devices;
mod diagnose;
mod events;
mod hookmap;
mod inbox;
mod jump;
mod lang;
mod lamps;
mod manual;
mod menu;
mod notify;
mod open;
mod outbound;
mod panel;
mod proc;
mod receipts;
mod remote;
mod rules;
mod server;
mod shortcuts;
mod shell;
mod state;
mod stats;
mod sweep;
mod tray;
mod update;
mod watches;
mod win_toast;

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
    /// 圆环菜单正开着的那盏灯的 label。
    ///
    /// 和 `peek` 同一套思路（记 label、不落盘、一次只可能有一盏），但用途不同：
    /// `peek` 让窗口按**展开态**算，这个让窗口换成**正方形**（见
    /// `config::menu_window_size`）—— 圆环画在灯牌外面，窗口不放大就会被
    /// `overflow: hidden` 裁掉，和右键菜单当初必须用原生菜单是同一个原因。
    ///
    /// 菜单一关就清空，窗口自然缩回内容尺寸。**不落盘**：菜单是瞬态界面，
    /// 重启后当然不该还开着。
    menu: Option<String>,
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

/// 当前界面语言的**共享**句柄。
///
/// 要 `Arc` 是因为 HTTP 上报那条路也要它：HTTP 式 hook（`POST /v1/hooks/{agent}`）
/// 由**应用自己**生成「正在执行 npm」那一行（命令式那边是 CLI 生成的），而那句话
/// 得说用户的语言。
type SharedLang = Arc<lang::LocalLang>;

/// 每个信号源「最近一次上报」的凭证（见 `receipts.rs`）。
///
/// 需要 `Arc` 共享：上报路径（HTTP 服务）要写它，接入页要读它 —— 两份真值就会
/// 撒谎，所以只能有一份。
type SharedReceipts = Arc<receipts::Receipts>;

/// 最近几次出站钩子的执行结果。界面上要能回答「我的服务器到底收到没有」——
/// 光看 stderr 是查不出来的（从 Dock 启动的话根本没地方看）。
type SharedOutbound = outbound::OutboundLog;

/// 多机中枢的健康状态（见 `remote.rs`）。
///
/// 和 `SharedReceipts` 同一个道理：轮询线程写它，「多机」页读它。两份真值就会
/// 撒谎，所以只能有一份。
type SharedMachines = remote::Registry;

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
        // 「调试上报」不是智能体报的，是应用自己注入的：归到自定义那一组
        source: Some("custom".into()),
        cwd: None,
        message,
        ttl_ms: None,
        pid: None,
        machine: None,
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
        source: Some("custom".into()),
        cwd: None,
        message: None,
        ttl_ms: Some(TEST_REPORT_TTL_MS),
        pid: None,
        machine: None,
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

    // 通知的状态过滤。界面只会发合法值，所以这里挡的是「导入一份手改过的配置」——
    // 报错要指到具体那个名字，而不是「设置不合法」。
    if let Some(problem) = config::notify_when_problem(&settings.notify_when) {
        return Err(format!("通知状态不合法：{problem}"));
    }

    for device in &settings.devices {
        if let Some(problem) = config::device_problem(device) {
            return Err(format!("物理灯不合法：{problem}"));
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

/// 待审批列表变化的事件名（灯上那个气泡据此刷新）。前端 `src/lib/useApprovals.ts`
/// 里的字面量必须与之一致。
const APPROVALS_EVENT: &str = "signalhub://approvals";

/// 改设置的统一出口：存盘 → 改内存 → 按新设置调整界面 → 广播给两个窗口。
///
/// 三条路径都会改设置，各写一遍迟早会漏掉其中一步（尤其是「广播」，漏了不报错，
/// 只是界面偶尔不对）。收成一处就不会。
pub(crate) fn persist_settings(
    app: &tauri::AppHandle,
    settings: Settings,
) -> Result<Settings, String> {
    let settings = settings.clamped();

    // 切「灯的数量」是**唯一**会建 / 关最多 `lamps::MAX_LAMPS` 个 webview 的操作，
    // 它出过两次「整机卡住」的报告（issue #4 是抱着 lamps 锁建窗；Windows 上那次
    // 是嵌套消息泵，见 [`lamp_worker`]）。而从 Dock 启动的 .app 没有 stderr，
    // 「卡住了」这件事事后完全查不到 —— 于是切换前后各记一行到 `hooks.log`：
    // **有头无尾就是卡在中间**。只在布局真的变了时才写，别的设置改动不会往里灌。
    let switching = {
        let previous = current_settings(app);
        if previous.lamp_layout == settings.lamp_layout {
            None
        } else {
            Some((previous.lamp_layout, std::time::Instant::now()))
        }
    };

    // 先落盘再改内存和界面：保存失败时不该出现「界面变了但重启就没了」
    config::save_settings(&settings).map_err(|err| format!("保存设置失败: {err}"))?;
    // 换一个新的 `Arc` 而不是就地改：读取方（状态热路径）只克隆那个 `Arc`，
    // 拿到的永远是一份**一致**的设置（见 `SharedSettings`）。这里深拷贝一次
    // 无所谓 —— 改设置是用户手点出来的低频操作。
    *app.state::<SharedSettings>().lock().expect("settings poisoned") = Arc::new(settings.clone());

    let snapshot = current_snapshot(app);
    if let Some((before, _)) = switching {
        config::append_hook_log(&format!(
            "{} [lamp] 开始切换布局 {:?} → {:?}（可见组 {}）\n",
            config::utc_hms(),
            before,
            settings.lamp_layout,
            visible_group_count(&settings, &snapshot),
        ));
    }
    apply_settings(app, &settings, &snapshot);
    if let Some((before, started)) = switching {
        // 只记**同步这一半**的耗时：建窗已经挪到后台线程了（见 [`lamp_worker`]），
        // 它自己的耗时由 `run_lamp_plan` 另记一行。两行合起来才是完整的一次切换。
        config::append_hook_log(&format!(
            "{} [lamp] 布局切换完成 {:?} → {:?}（{} ms）\n",
            config::utc_hms(),
            before,
            settings.lamp_layout,
            started.elapsed().as_millis(),
        ));
    }
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

/// 下载页。给「这个平台暂不支持自动更新」那条路用 —— 说清楚「请手动下载」之后，
/// 总得给个去处，否则那句话等于把用户丢在原地。
const RELEASES_URL: &str = "https://github.com/iFishin/SignalHub/releases";

#[tauri::command]
fn open_docs() -> Result<(), String> {
    open::open(DOCS_URL).map_err(|err| format!("打开浏览器失败: {err}"))
}

/// 在文件管理器里指出应用自己的文件。
///
/// 接入方最需要的那几样都在 `~/.signalhub/` 里（`token`、`port`、`config.json`），
/// 而「它在哪」这件事从界面上说不清楚，直接打开最省事。
#[tauri::command]
fn open_releases() -> Result<(), String> {
    open::open(RELEASES_URL).map_err(|err| format!("打开浏览器失败: {err}"))
}

/// 用系统默认浏览器打开一个 http/https 地址。
///
/// 目标是 Call Me 留言里智能体自报的 `context`（「我是在这个页面读到链接的」），
/// **它是不可信输入**：只认 http/https，别的一律拒绝 —— 否则一个 `file://`、
/// `javascript:` 或者自定义 scheme 就可能被 `open` 交给别的程序。`open::open`
/// 本身不过 shell（argv 直传），这里再补一道 scheme 白名单。
#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    let trimmed = url.trim();
    if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
        return Err("只支持 http / https 链接".to_string());
    }
    open::open(trimmed).map_err(|err| format!("打开浏览器失败: {err}"))
}

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

/// 打开 / 关闭收件箱。灯上圆环菜单、托盘菜单、全局快捷键都走它（见 `inbox.rs`）。
/// v0.5.0 之后灯左键单击是折叠 / 展开，不再直达这里。
#[tauri::command]
fn toggle_inbox(app: tauri::AppHandle) {
    inbox::toggle(&app);
}

/// 收件箱里的 Esc / 关闭按钮。**只隐藏**，不销毁窗口 —— 和设置窗同一个理由。
#[tauri::command]
fn hide_inbox(app: tauri::AppHandle) {
    inbox::hide(&app);
}

/// 静音 / 取消静音某一组（收件箱行内的动作）。
///
/// 和灯右键菜单、通知按钮共用 `set_group_muted` —— 同一件事有三条入口时，
/// 各写一遍行为迟早分叉。
#[tauri::command]
fn mute_group(app: tauri::AppHandle, group: String, muted: bool) -> Result<Settings, String> {
    set_group_muted(&app, &group, muted)
}

/// 把焦点切到某个会话所在的终端（收件箱的「跳转」）。
///
/// `pid` / `cwd` 只在锁里取出来，**出锁之后才跑外部命令** —— 否则一次 osascript
/// 卡住就会把整条上报链路一起堵住。
#[tauri::command]
fn jump_to_session(app: tauri::AppHandle, session: String) -> jump::JumpResult {
    let store = app.state::<SharedStore>();
    let (pid, cwd) = {
        let guard = store.lock().expect("state store poisoned");
        (guard.session_pid(&session), guard.session_cwd(&session))
    };
    jump::to_session(pid, cwd.as_deref())
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
        pass_through_shortcut: shortcuts::pass_through_label(),
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

    // **不重算窗口**：暂停角标是绝对定位在灯体内部最上沿的徽标（见 lamp.css 的
    // `.lamp-flag`），不占牌身宽度、也不占折叠态的点位。0.6.4 起穿透不再画角标，
    // 而是「四角挂什么」里可选的一枚 `.sign-corner`，同样不进几何。曾经这里按角标数
    // 加宽窗口，表现就是一开穿透整排灯珠被推偏；别把它加回来。

    // 开启时发一条通知说明怎么退出。**不再顺手把外形收起来**：穿透下窗口本来
    // 就点不动，而外形是用户自己选的、存在设置里的，替用户改掉反而是越权。
    if enabled {
        let lang = current_lang(app);
        alerts::send_message(
            app,
            lang::text(lang).pass_through_title,
            &lang::pass_through_body(lang, &shortcuts::pass_through_label()),
        );
    }

    // 托盘那一项在穿透开着时会写成「退出点击穿透 · ⌃⌥S」。命令 / 托盘 / 快捷键
    // 三条入口都汇到这里，所以菜单只在这一处重建，文案不会和真实状态漂移。
    // 通知是一次性的，这条是穿透期间**一直查得到**的退出指引。
    tray::rebuild_menu(app, current_lang(app));

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

/// 「全部归位」：把散落在桌面各处的组灯摆回默认堆叠，并丢掉拖过的位置记忆。
///
/// 用户要的是「保持自由散落 + 只加一个归位入口」：默认摆放、自定义拖动、位置
/// 记忆一个都不改，只多一条「一键收拾」的路。入口有三处 —— 灯上右键的通用菜单
/// （`menu::show_general`）、灯珠 / 牌面右键的组菜单（`menu::show`，多灯下最顺手
/// 的那一个）和设置页「分组」页（`SettingsPanel`），都走下面这个共用实现，免得
/// 三条路的行为分叉。
///
/// 返回摆了几盏。**清记忆是无条件的**：点「全部归位」本身就是「丢掉我拖过的
/// 位置」这个明确意图，不该因为当下没有开着的组灯而变成空操作。尤其是「每组分灯
/// 但可见组超过 `MAX_LAMPS` 回落单灯」时 `lamp_specs` 为空，而设置页按钮仍在 ——
/// 早退会让它显示「已归位」却什么都没做，下次组数回落解除又照旧散回去。前端按
/// 返回值 0 给另一种回执（「已清空位置」），不谎称归了位。
#[tauri::command]
fn reset_lamp_positions(app: tauri::AppHandle) -> usize {
    reset_lamp_positions_impl(&app)
}

pub(crate) fn reset_lamp_positions_impl(app: &tauri::AppHandle) -> usize {
    let settings = current_settings(app);
    let snapshot = current_snapshot(app);
    // 顺序跟「第一次出现」完全一致：置顶在前、其余按紧急度（和前端渲染同一条
    // `lamp_specs`）。归位完从上往下读到的组，和刚切到多灯时看到的是同一串。
    // 尺寸也用同一份规格，见 `lamps::reset_positions` 的说明。空规格不早退：
    // 下面清记忆那一步仍要做完。
    let specs = lamp_specs(app, &settings, &snapshot);

    // 锁内只清记忆 + 取快照；落盘和动窗口都在锁外（`LampWindows::commit` 那条
    // 纪律）。磁盘也写空表：只清内存的话，用户归位后马上退出，`save_open_positions`
    // 会以旧文件为底把老坐标 merge 回去。
    let positions = {
        let state = app.state::<SharedLamps>();
        let mut lamps = state.lock().expect("lamps poisoned");
        lamps.clear_positions();
        lamps.positions_snapshot()
    };
    lamps::save_positions(&positions);

    lamps::reset_positions(app, &specs)
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
    // hover / 圆环菜单本身就会改变尺寸，增量判断够用，不必强制重设。
    sync_layout(&app, &settings, &snapshot, false);
    Ok(())
}

/// 打开 / 关闭某一盏灯上的圆环菜单。
///
/// 打开时窗口换成正方形（`config::menu_window_size`）—— 圆环画在灯牌外面，
/// 窗口不放大就会被 `overflow: hidden` 裁掉。关掉后自动缩回内容尺寸。
///
/// **为什么不用原生菜单**（`menu.rs` 那套）：圆环的形状、动效和「中心那颗灯珠」
/// 原生菜单都做不出来。原生菜单仍然留着（右键），两条入口各管一件事 ——
/// 右键是「这一组的操作」（改名 / 静音 / 隐藏 / 手动状态），长按圆环是
/// 「灯本身的动作」（收件箱 / 设置 / 暂停 / 退出）。
///
/// `window` 由 Tauri 注入，所以多灯下只有被长按的那一盏会变形，别的灯一动不动。
#[tauri::command]
fn set_lamp_menu(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    open: bool,
) -> Result<(), String> {
    {
        let ui_state = app.state::<SharedUi>();
        let mut ui = ui_state.lock().expect("ui poisoned");
        if open {
            ui.menu = Some(window.label().to_string());
        } else if ui.menu.as_deref() == Some(window.label()) {
            // 只清自己那一条（同 `set_peek`）：另一盏灯的菜单不该被这个窗口关掉
            ui.menu = None;
        }
    }
    let settings = current_settings(&app);
    let snapshot = current_snapshot(&app);
    // hover / 圆环菜单本身就会改变尺寸，增量判断够用，不必强制重设。
    sync_layout(&app, &settings, &snapshot, false);
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
/// **入口只剩一个**：右键菜单里的「清除该组的状态」（`menu.rs::CLEAR`）。
/// 2026-11-10 之前牌右上角还有一枚悬停出现的 `×`（命令 `clear_group_state`），
/// 那枚撤了 —— 常驻窗口上的一枚 × 读起来是「关掉这块牌」，而它只清一个状态；
/// 何况那个角落现在是四角小灯 `tr` 的落点。命令也一并删掉，免得留下一个没人调的
/// 入口，日后被人当成「还有第二条路」。
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

/// 「推迟」按多久。写在一处：菜单文案里那个「15 分钟」要是和这里不一致，
/// 用户按下去就会觉得时间不对，而那种错没人会去 commit 里找。
pub(crate) const SNOOZE_MS: u64 = 15 * 60 * 1000;

/// 推迟该组 15 分钟；已经推迟过就取消（菜单里两面同一个 id，见 `menu::SNOOZE`）。
///
/// **只改展示**：会话、设置、统计、钩子、通知一概不碰 —— 理由见
/// [`crate::state::Store::snooze`] 的长注释。因此这里不走 `notify::push`（那条链
/// 会记一遍今日时长），只把新快照推给窗口。
pub(crate) fn toggle_snooze(app: &tauri::AppHandle, group: &str) {
    let store = app.state::<SharedStore>();
    let snapshot = {
        let mut guard = store.lock().expect("state store poisoned");
        let now = std::time::SystemTime::now();
        // 判据和菜单里那句文案是同一处（`snooze_until` 比 `now`），所以不会
        // 出现「写着取消推迟、点了却又推迟 15 分钟」。
        if guard.snooze_until(group).is_some_and(|until| until > now) {
            guard.clear_snooze(group);
        } else {
            // 把「按下那一刻的状态」一起记下来：它一变，这条推迟自己作废
            // （见 `state::Snooze`）。否则一组从「等批准」变成「出错了」还会被
            // 压着不闪，新的事实被一个旧动作藏起来。
            let state = guard.group_state(group);
            guard.snooze(group, state, now + std::time::Duration::from_millis(SNOOZE_MS));
        }
        guard.snapshot()
    };
    notify::push_ui(app, &snapshot);
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

/// 四个全局快捷键现在的处境（设置 →「系统」页那几行）。
///
/// 注册失败（多半是被别的程序占了）时界面必须**明说哪一个没生效** —— 一个按了
/// 没反应的键，用户只会以为是 SignalHub 坏了。
#[tauri::command]
fn get_shortcuts(
    status: tauri::State<'_, shortcuts::SharedShortcuts>,
) -> Vec<shortcuts::ShortcutStatus> {
    status.lock().expect("shortcut status poisoned").clone()
}

/// 现在有几条审批在等（灯上那个气泡读它）。
///
/// 和 HTTP 的 `GET /v1/approvals` 是同一个函数 —— 那个接口是给脚本的（也是这个
/// 功能唯一能被端到端验证的入口），灯走 IPC，因为前端手上没有 token。
#[tauri::command]
fn get_approvals(
    registry: tauri::State<'_, approvals::SharedApprovals>,
) -> Vec<approvals::PendingView> {
    registry.list()
}

/// 替用户点一下「允许 / 拒绝」。
///
/// **「看不全不给放行」在登记表里挡着**（`Registry::decide`），所以这个命令只是把
/// 失败翻译成一句人话；界面上那个按钮本来也不会画出来。
///
/// `reason` / `custom` 只在拒绝时有意义（`docs/plan-approval.md` A2）：
/// `reason` 是预设键（`unsafe` / `safer` / `explain` / `custom`），`custom` 是
/// 用户自己写的那句话。认不出来的键一律当默认 —— 拒绝不能因为参数写错而失效。
#[tauri::command]
fn decide_approval(
    registry: tauri::State<'_, approvals::SharedApprovals>,
    id: String,
    decision: String,
    reason: Option<String>,
    custom: Option<String>,
) -> Result<(), String> {
    let decision = match decision.as_str() {
        "allow" => approvals::Decision::Allow,
        "deny" => approvals::Decision::Deny(approvals::DenyReason::parse(
            reason.as_deref(),
            custom,
        )),
        other => return Err(format!("decision 只能是 allow 或 deny，收到 {other:?}")),
    };
    match registry.decide(&id, decision, "lamp") {
        approvals::Decided::Done => Ok(()),
        approvals::Decided::NotFound => Err("没有这一条了（多半是刚超时）".to_string()),
        approvals::Decided::NotAllowable => {
            Err("这条在灯上看不全，只能拒绝 —— 要放行请回终端里看".to_string())
        }
    }
}

/// 审批记录（设置 → 历史页读它，见 `docs/plan-approval.md` A1）。
///
/// 直接读盘而不是从内存里取：那份文件才是「跨重启」的那一份，而读它是低频动作
/// （只在打开那一页时）。读不出来就当空 —— 历史坏了不该挡住别的。
///
/// 不给 `since_ms` 就是**最近若干条**（只读最近两个文件，够列表翻的）；给了就
/// 读整个保留期，给「本周洞察」按工具聚合用。两种读法共用一套落盘格式。
#[tauri::command]
fn get_approval_history(
    limit: Option<usize>,
    since_ms: Option<u64>,
) -> Result<Vec<approval_log::Record>, String> {
    match since_ms {
        // 洞察用的这条要区分「读不到」和「没有记录」：前者让前端显示错误态。
        Some(since) => approval_log::since(since, limit.unwrap_or(2000).min(5000))
            .map_err(|err| err.to_string()),
        None => Ok(approval_log::recent(limit.unwrap_or(100).min(500))),
    }
}

/// 某时刻之后的事件流水（收件箱摘要 v2、历史页「本周洞察」读它）。
///
/// `since_ms` 是**前端给的墙上时钟**：Rust 不知道本地时区（和 `stats.rs` 同一个
/// 取舍），而「今天零点」「上次打开收件箱的时刻」都只有前端说得准。上限是硬的 ——
/// 常驻应用攒一天的事件可以有几千条，IPC 不该一次搬一座山；界面如实说明这个
/// 上限，并在真的截断时把「数字可能偏小」说出来，而不是把截断后的数据当全量。
#[tauri::command]
fn get_events(since_ms: u64, limit: Option<usize>) -> Result<Vec<events::Event>, String> {
    events::since(since_ms, limit.unwrap_or(2000).min(5000)).map_err(|err| err.to_string())
}

/// 收件箱里「跑完叫我」。`label` 是那一行的显示名，通知里要能认出是哪个任务。
///
/// 订阅只活在内存里、触发即移除（见 `watches.rs`）：它盯的是这一次运行，
/// 不是一条要跨重启保留的偏好。
#[tauri::command]
fn watch_session(
    watches: tauri::State<'_, watches::Watches>,
    session: String,
    label: String,
) -> Vec<String> {
    watches.add(session, label);
    watches.ids()
}

#[tauri::command]
fn unwatch_session(
    watches: tauri::State<'_, watches::Watches>,
    session: String,
) -> Vec<String> {
    let _ = watches.remove(&session);
    watches.ids()
}

#[tauri::command]
fn get_watches(watches: tauri::State<'_, watches::Watches>) -> Vec<String> {
    watches.ids()
}

/// 清掉某一个会话（设置 →「状态」页每一行末尾的 `×`）。
///
/// 粒度比 [`clear_group`] 细一档：一组里跑着三个会话、卡住一个的时候，
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
    // 会话没了，挂在它名下的审批也不再作数（和 HTTP 的 DELETE 走的是同一个判断，
    // 见 `server::drop_session`）。**不清整个登记表**：别的会话还在等。
    app.state::<approvals::SharedApprovals>()
        .forget_session(&session);
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

/// 系统通知权限的当前状态（`granted` / `denied` / `prompt` / `unknown` /
/// `unavailable`）。
///
/// 界面上要能回答「通知到底通没通」—— 没权限时通知是静默丢弃的，不弹也不报错，
/// 光看界面和日志都查不出来（见 `alerts::permission_state`）。
///
/// **Windows 上 `unavailable` 优先**：插件的 `permission_state()` 在 Windows 恒返回
/// `Granted`（那里没有 macOS 那种授权弹窗），所以「已允许」根本不能证明 Toast 发得
/// 出去。AUMID 没注册成功时要如实说没注册，否则 portable 版会一直显示「已允许」
/// 却一条都不弹（issue #25）。
#[tauri::command]
fn get_notify_permission(app: tauri::AppHandle) -> &'static str {
    if win_toast::failed(&app) {
        return "unavailable";
    }
    alerts::permission_state(&app)
}

/// Windows 通知身份的注册结果：`ok` / `n/a`（非 Windows）/ 具体错误。
#[tauri::command]
fn get_notify_backend(app: tauri::AppHandle) -> String {
    app.state::<win_toast::ToastBackend>()
        .error()
        .unwrap_or_else(|| {
            if cfg!(windows) {
                "ok".to_string()
            } else {
                "n/a".to_string()
            }
        })
}

/// 设置页的「重新注册」按钮。`ok` 之外一律是能直接贴进 issue 的失败原因。
#[tauri::command]
fn ensure_notify_backend(app: tauri::AppHandle) -> String {
    win_toast::retry(&app)
}

/// 主动要一次通知权限，返回要过之后的状态。
#[tauri::command]
fn request_notify_permission(app: tauri::AppHandle) -> &'static str {
    alerts::request_permission(&app)
}

/// 发一条测试通知，并把发完之后的权限状态回给界面。
///
/// 为什么要有这一条：没权限时 `builder.show()` 是**静默失败**的 —— 不弹、不响、
/// 也不报错（见 `alerts::permission_state`）。用户点完按钮什么都没发生，界面必须
/// 能说清「是系统没放行，还是系统专注模式拦了」。所以这里走的是和真实告警同一个
/// 出口（`send_timer`，跟着 `Settings::sound` 带提示音），并回报权限；标题正文由界面
/// 给，好跟着界面语言走。它不改任何状态，也不记进统计。
///
/// **「已发出」不等于「已显示」**：我们注册了动作按钮，插件因此走 `show_tracked`
/// 的后台线程，那条路径把底层 `show()` 的错误吞掉了，返回值一律是成功。Windows
/// 上尤其如此 —— 能不能弹取决于 AUMID 有没有注册好，界面文案不能替系统打包票。
#[tauri::command]
fn test_notification(app: tauri::AppHandle, title: String, body: String) -> String {
    let sound = current_settings(&app).sound;
    alerts::send_timer(&app, &title, &body, sound);
    alerts::permission_state(&app).to_string()
}

/// 一个智能体在这台机器上的状态。
#[derive(serde::Serialize)]
struct AgentStatus {
    /// CLI 里用的名字（`claude-code`）
    ///
    /// `String` 而不是 `&'static str`：自定义预设（`~/.signalhub/agents.json`）
    /// 的名字是运行期读进来的。
    id: String,
    /// 显示名（`Claude Code`）
    name: String,
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
    /// 装的是不是**旧版本**写的。
    ///
    /// **`installed && stale`** = 升级后没重装：功能还在，行为停在旧版本 ——
    /// 典型是 Claude Code 的 `SIGNALHUB_PID="$PPID"` 前缀（0.3.2 起才有），
    /// 没重装的人界面上看到「已接入」，灯却回到「沉默」而不是「空闲」。
    /// 和 `command_ok`（程序还跑不跑得起来）是两个不同的问题，所以分开报。
    ///
    /// 判据按接入方式分两套：命令式比「配置里那条命令 vs 当前会写的那条」；
    /// 插件式比「源码里的格式版本 vs 这一版的模板版本」（插件没有一条可比的命令）。
    stale: bool,
    /// 装进去的是 **HTTP 式**（`Some(true)`）还是命令式（`Some(false)`）。
    /// `None` = 没装。
    ///
    /// 两种模式写进同一个配置，界面必须说得出「你在跑哪条路」—— 否则用户看到
    /// 「已接入」却不知道自己改不改 PATH 有没有关系。
    http: Option<bool>,
    /// 这条是用户在 `~/.signalhub/agents.json` 里自己写的。
    ///
    /// 界面据此加一枚「自定义」标记，并**不给它 HTTP / 审批那两个按钮** ——
    /// 那两条建立在「这一家的文档写清了它能回什么」之上，自定义的那一家没人验证过。
    custom: bool,
}

/// 扫一遍机器上有哪些智能体、我们接入了哪些。
///
/// 检测只是给界面排个序，**不决定能不能装** —— 没检测到的也照样能一键接入，
/// 因为「检测」看的是痕迹（配置目录、PATH 上的命令），而痕迹可能因为路径变化、
/// 便携安装等原因对不上。宁可让用户看到一个「没检测到」但其实装得了的条目，
/// 也不要因为判错而把路堵死。
#[tauri::command]
fn detect_agents() -> Vec<AgentStatus> {
    let builtin = hooks::ALL_NAMES
        .iter()
        .filter_map(|id| hooks::Preset::parse(id).ok());
    // 自定义预设排在后面：它们多半是长尾里的一家，不该把常见的那几家挤下去。
    // **一条写错不影响别的** —— `agents::load` 是部分成功的（见那个模块的说明）。
    let custom = agents::load()
        .agents
        .into_iter()
        .map(hooks::Preset::Custom);

    builtin.chain(custom).map(|preset| status_of(&preset)).collect()
}

fn status_of(preset: &hooks::Preset) -> AgentStatus {
    // `installed_command` 要读一次配置文件，所以只算一次
    let command = hooks::preset_installed_command(preset);
    let command_ok = command.as_deref().is_some_and(hooks::command_resolves);
    // **HTTP 式没有命令可解析**，所以 `command_ok` 在那种情况下无意义 ——
    // 界面按 `http` 那个字段改口（见 `SettingsPanel` 的 `agentTryHint`）。
    let http = hooks::preset_installed_http(preset);
    let installed = hooks::preset_installed(preset);
    // 装的是不是**旧版本**写的（判据和 `diagnose` 共用一份，见
    // `hooks::installed_is_stale`）。
    let stale = hooks::preset_installed_is_stale(preset);
    AgentStatus {
        id: preset.id().to_string(),
        name: preset.name().to_string(),
        detected: hooks::preset_detect(preset),
        installed,
        config: preset
            .config_path()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default(),
        command,
        command_ok,
        stale,
        http,
        custom: preset.is_custom(),
    }
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
        // **界面上的日志也要脱敏**：这一页的文本会被选中复制进 issue，而日志里
        // 记着出站钩子的完整 URL（推送服务的密钥就在查询串上）。和「一键诊断」
        // 用同一份规则 —— 那是另一条复制出去的路，规则不该有两份（issue #15）。
        .map(crate::diagnose::redact)
        .collect();
    lines.reverse();
    lines
}

/// 每个信号源「最近一次上报」的凭证。
///
/// 接入页靠它回答那个真正的问题：**它到底有没有在报**。以前能说的只有「配置文件里
/// 写着我们的命令」，而那只说明我们写过了 —— 「装了却没反应」的接入看起来一模一样
/// （见 `receipts.rs`）。
#[tauri::command]
fn agent_receipts(receipts: tauri::State<'_, SharedReceipts>) -> Vec<receipts::ReceiptView> {
    receipts.view()
}

/// 一键诊断：一段可复制的排查文本（见 `diagnose.rs`）。
///
/// `agent` 给 `Some` 时只写那一个 —— 用户问「opencode 怎么接不上」时，他最想看到
/// 的就是那一段，而不是八个智能体一起。
#[tauri::command]
fn diagnose(
    app: tauri::AppHandle,
    receipts: tauri::State<'_, SharedReceipts>,
    agent: Option<String>,
) -> Result<String, String> {
    let filter = agent
        .map(|name| hooks::Agent::parse(&name))
        .transpose()?;
    let mut out = diagnose::text(&receipts, filter);
    // 通知那一段挂在报告末尾：CLI 也调 `diagnose::text`，但它没有 AppHandle，
    // 不该为了 app 独有的一条状态去改那个共享模块的函数签名。
    out.push_str(&format!("\n== 通知 ==\n{}\n", win_toast::status_line(&app)));
    Ok(out)
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
/// 与 CLI 的 `install hooks <agent>` **走同一段逻辑**（`hooks::install_preset`）——
/// 界面和命令行两条入口共用实现，行为不会分叉。`agent` 既可以是内置的名字，
/// 也可以是 `~/.signalhub/agents.json` 里自己加的那一条。
#[tauri::command]
fn install_agent_hooks(agent: String) -> Result<HookChange, String> {
    hooks::install_preset(&hooks::Preset::parse(&agent)?, false).map(HookChange::from)
}

/// 用 **HTTP 方式**接入（目前只有 Claude Code 支持）。
///
/// 和上面那个分开，而不是加一个「模式」参数：**用户点的是一件事，不是一个选项**。
/// 界面上它是「改用 HTTP 方式」这一句，对应的就是这条命令；而参数化的接口会让人
/// 以为还有第三种组合。
///
/// 端口取**此刻**服务真正在用的那个（`config::read_port`）。拿不到就说明应用还没
/// 起来 —— 那时写进去的 URL 一定是错的，宁可失败也不要留一条打不通的 hook。
///
/// 端口在 **Rust 这边读**，不从界面上传：界面上那个值是打开设置面板时取的，
/// 中途端口可能已经变了（原来那个被占用），而写进用户配置文件的东西不该来自
/// 一份可能过期的快照。
#[tauri::command]
fn install_agent_http_hooks(agent: String) -> Result<HookChange, String> {
    let port = config::read_port()
        .map_err(|err| format!("读不到上报服务的端口（应用还没跑起来？）: {err}"))?;
    hooks::install_http(hooks::Agent::parse(&agent)?, port, false).map(HookChange::from)
}

/// 装 / 卸「从灯上审批」（C3）。
///
/// **一次调用做完两件事**：改配置里的 hook、改设置里那个开关。分开的话会出现
/// 「hook 装上了但开关还关着」（审批永远不生效）或者反过来（开关开着但没有 hook
/// 会打过来）—— 两种都是静默的，用户只会觉得「这个功能坏了」。
///
/// 关掉的时候**装回普通的 HTTP 式接入**，而不是卸载：`PermissionRequest` 那一条
/// 刚才被换成了审批端点，直接卸掉会让「它本来要问你」这个事件从此没人上报。
/// 用户想彻底移除，用旁边的「移除接入」。
///
/// 端口和 token 都在 Rust 这边读（见 [`install_agent_http_hooks`] 的同一条理由）。
/// 「接入」页里那一节：这台机器上做得到的采集器。
#[tauri::command]
fn list_collectors(app: tauri::AppHandle) -> Vec<collect::CollectorView> {
    let settings = current_settings(&app);
    collect::views(current_lang(&app), &settings)
}

/// 打开 / 关掉一条采集器。
///
/// 只改设置：采集线程一直在跑，每一跳自己看开关（见 `collect.rs`）。所以这里
/// 没有「重启线程」这一步，也就没有「线程重启失败但开关已经翻过去了」这种状态。
#[tauri::command]
fn set_collector(app: tauri::AppHandle, id: String, on: bool) -> Result<(), String> {
    let collector = collect::find(&id).ok_or_else(|| format!("认不出这条采集器：{id}"))?;
    let mut settings = (*current_settings(&app)).clone();
    match collector {
        collect::Collector::Meeting => settings.collect_meeting = on,
        collect::Collector::Backup => settings.collect_backup = on,
    }
    persist_settings(&app, settings).map(|_| ())
}

/// 「多机」页：每台对端最近一次拉取的结果。
///
/// **只读**：轮询线程是唯一写入方（见 `remote.rs`）。这里不给前端「改健康状态」的
/// 能力 —— 那个值是事实，不是设置。
#[tauri::command]
fn list_machines(machines: tauri::State<'_, SharedMachines>) -> Vec<remote::Health> {
    machines
        .lock()
        .expect("registry poisoned")
        .values()
        .cloned()
        .collect()
}

/// 「测试连接」：立刻拉一次，就地显示结果。
///
/// 地址和 token 由界面传进来（而不是从设置里读）：用户点测试时那一行**可能还没
/// 保存**，读设置会测到一个刚被改掉的旧值 —— 而「点了测试，结果测的是别的地址」
/// 是这个交互里最坏的一种表现。
#[tauri::command]
async fn probe_machine(id: String, url: String, token: String) -> remote::Health {
    // 拉取是阻塞的（`ureq`），扔进 async 命令里必须挪到阻塞线程池，否则会占着
    // 一个 worker 直到超时（最长 3 秒）—— 那期间这条 IPC 之外的命令也排队。
    tauri::async_runtime::spawn_blocking(move || {
        remote::probe(&config::Machine {
            id,
            url,
            token,
            enabled: true,
        })
    })
    .await
    .unwrap_or_else(|err| remote::Health {
        id: String::new(),
        ok: false,
        last_ok_ms: 0,
        latency_ms: 0,
        error: Some(format!("测试没能跑起来：{err}")),
        sessions: 0,
    })
}

/// 「Call Me」页的初始 / 对账拉取。**只读**：轮询线程是唯一写入方（见 `callme.rs`）。
#[tauri::command]
fn get_callme(state: tauri::State<'_, callme::SharedCallMe>) -> callme::CallMeState {
    state.lock().expect("callme state poisoned").clone()
}

/// 「测试连接」：地址和 token 由界面传进来（可能还没保存），理由同 `probe_machine`。
#[tauri::command]
async fn callme_probe(url: String, token: String) -> callme::Probe {
    tauri::async_runtime::spawn_blocking(move || callme::probe(&url, &token))
        .await
        .unwrap_or_else(|err| callme::Probe {
            ok: false,
            error: Some(format!("测试没能跑起来：{err}")),
            ..callme::Probe::default()
        })
}

/// 新建一条来源链接。同样用界面上的地址 / token（可能还没保存）——
/// 「点了新建，结果建到了另一个服务器上」是这个交互里最坏的表现。
#[tauri::command]
async fn callme_add_source(
    url: String,
    token: String,
    label: String,
    note: Option<String>,
) -> Result<callme::SourceView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        callme::add_source(&url, &token, &label, note.as_deref())
    })
    .await
    .map_err(|err| format!("调用没能跑起来：{err}"))?
}

#[tauri::command]
async fn callme_revoke_source(url: String, token: String, id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || callme::revoke_source(&url, &token, &id))
        .await
        .map_err(|err| format!("调用没能跑起来：{err}"))?
}

/// 来源管理的三个动作。和 `add_source` / `revoke_source` 一样用界面当前的
/// 地址 / token（可能还没保存），避免「改到了另一台服务器上」。
#[tauri::command]
async fn callme_rename_source(
    url: String,
    token: String,
    id: String,
    label: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        callme::rename_source(&url, &token, &id, &label)
    })
    .await
    .map_err(|err| format!("调用没能跑起来：{err}"))?
}

#[tauri::command]
async fn callme_delete_source(url: String, token: String, id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || callme::delete_source(&url, &token, &id))
        .await
        .map_err(|err| format!("调用没能跑起来：{err}"))?
}

#[tauri::command]
async fn callme_restore_source(url: String, token: String, id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || callme::restore_source(&url, &token, &id))
        .await
        .map_err(|err| format!("调用没能跑起来：{err}"))?
}

/// 本机 Call Me 服务的状态。只监听回环地址；公网部署仍走 `server/` 独立服务。
#[tauri::command]
fn callme_local_status(
    local: tauri::State<'_, callme_local::SharedLocal>,
) -> callme_local::LocalStatus {
    callme_local::status(&local)
}

/// 启动本机服务并返回界面需要填写的地址与 token。tokio 任务随应用退出而结束。
#[tauri::command]
async fn callme_local_start(
    local: tauri::State<'_, callme_local::SharedLocal>,
    port: u16,
) -> Result<callme_local::LocalStatus, String> {
    callme_local::start(local.inner().clone(), port).await
}

#[tauri::command]
fn callme_local_stop(
    local: tauri::State<'_, callme_local::SharedLocal>,
) -> callme_local::LocalStatus {
    callme_local::stop(&local)
}

/// 标记某个来源读到第几条。写本地文件，**不回写服务器**（见 `callme.rs` 文件头）。
#[tauri::command]
fn callme_mark_read(
    app: tauri::AppHandle,
    state: tauri::State<'_, callme::SharedCallMe>,
    source_id: String,
    seq: u64,
) -> Result<(), String> {
    callme::mark_read(&state, &source_id, seq)?;
    let snapshot = state.lock().expect("callme state poisoned").clone();
    let _ = app.emit(callme::CALLME_EVENT, snapshot);
    Ok(())
}

#[tauri::command]
fn set_tool_approval(
    app: tauri::AppHandle,
    agent: String,
    on: bool,
) -> Result<HookChange, String> {
    let agent = hooks::Agent::parse(&agent)?;
    let port = config::read_port()
        .map_err(|err| format!("读不到上报服务的端口（应用还没跑起来？）: {err}"))?;

    let report = if on {
        let token = config::read_token()
            .map_err(|err| format!("读不到 token（应用还没跑起来？）: {err}"))?;
        hooks::install_approve(agent, port, &token, false)?
    } else {
        // 回到「只上报」：那一条 PermissionRequest 又变回上报端点
        hooks::install_http(agent, port, false)?
    };

    // 开关跟着一起改。**落盘失败就说出来**：配置已经改了而开关没改，比不做还糟。
    let mut settings = (*current_settings(&app)).clone();
    settings.approve_tools = on;
    persist_settings(&app, settings)?;
    Ok(HookChange::from(report))
}

#[tauri::command]
fn uninstall_agent_hooks(agent: String) -> Result<HookChange, String> {
    hooks::uninstall_preset(&hooks::Preset::parse(&agent)?, false).map(HookChange::from)
}

/// 自定义预设那本「账」：文件在哪、读不读得懂、现在有几条。
///
/// 单独一条命令而不是塞进 `detect_agents` 的返回里：那个返回值是**一排智能体**，
/// 往里加一个「元信息」字段会让它变成一个既装列表又装错误的杂货袋，而界面里
/// 这两样东西的位置也完全不同（列表在中间，文件入口在下面）。
#[derive(serde::Serialize)]
struct CustomAgentsInfo {
    /// `~/.signalhub/agents.json` 的绝对路径
    path: String,
    /// 读不懂的地方，每条一句话。空 = 没问题。
    ///
    /// **必须报出来**：配置文件最坏的失败方式是被静默忽略 —— 用户改了文件、
    /// 点了接入、什么都没发生，也没有任何提示。
    problems: Vec<String>,
    /// 能当 `family` 用的几个名字（给界面填提示，别让用户去猜）
    families: Vec<String>,
}

#[tauri::command]
fn custom_agents_info() -> CustomAgentsInfo {
    let loaded = agents::load();
    CustomAgentsInfo {
        path: agents::file_path()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default(),
        problems: loaded.problems,
        families: agents::families()
            .into_iter()
            .map(|agent| hooks::cli_name(agent).to_string())
            .collect(),
    }
}

/// 打开 `~/.signalhub/agents.json`，**没有就建一份带说明的空文件**。
///
/// 建文件是这一步的重点：让用户先对着一个空目录想「格式是什么」，等于把文档
/// 翻出来这件事也交给他。文件里带一条 `_example`（加载器不看它），照着抄就行。
#[tauri::command]
fn open_custom_agents() -> Result<String, String> {
    let path = agents::ensure_file()?;
    let text = path.to_string_lossy().into_owned();
    open::open(&text).map_err(|err| format!("打不开 {}：{err}", path.display()))?;
    Ok(text)
}

/// 往 `agents.json` 校验并追加一条自定义预设。
///
/// 和 CLI 的 `signalhub add-agent` 共用 `agents::add` —— 同一份字段校验、同一份
/// 原子写。界面目前只提供「打开配置文件」和提示词两个入口，这个命令是给面板里
/// 将来的添加表单、以及本机自动化用的；外部智能体走 CLI（它在终端里，够不到
/// webview 的 IPC）。
///
/// `replace` 默认 false：同 id 已存在且内容不同时**拒绝**，免得一次误操作把用户
/// 手写的一条悄悄改掉。
#[tauri::command]
fn add_custom_agent(
    id: String,
    name: String,
    family: String,
    config: String,
    marker: Option<String>,
    command: Option<String>,
    replace: Option<bool>,
) -> Result<agents::AddResult, String> {
    agents::add(
        agents::AddRequest {
            id,
            name,
            family,
            config,
            marker,
            command,
        },
        replace.unwrap_or(false),
    )
}

/// HTTP 式接入的探测上限。对面就是我们自己、走的是回环，3 秒绰绰有余 ——
/// 而这个数字同时是「点下去界面最多等多久」的上限。
const PROBE_HTTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// 打一次自己的 hook 端点，确认它活着、也认得这个 payload。
///
/// **必须真发一次 HTTP**，不能直接调 handler：这条路上要验证的恰恰是「端点在不在」
/// ——路由挂上去没有、是不是真的绑在回环上、body 限长有没有把正常的 payload 挡掉。
/// 直接调函数把这些全都跳过去了，那正是最需要证明的部分。
///
/// 阻塞式的（ureq），调用方负责挪到线程上（见 `try_agent`）。
fn probe_http(url: &str, payload: &str) -> Result<String, String> {
    let config = ureq::Agent::config_builder()
        .timeout_connect(Some(PROBE_HTTP_TIMEOUT))
        .timeout_per_call(Some(PROBE_HTTP_TIMEOUT))
        .build();

    // ureq 把 4xx/5xx 也当成 Err，所以 Ok 就意味着 2xx。这里唯一的非 2xx 是
    // 「开了局域网访问」时的 403，而它的原因写在响应体里 —— 由 err 带出去。
    let mut response = ureq::Agent::new_with_config(config)
        .post(url)
        .header("Content-Type", "application/json")
        .send(payload)
        .map_err(|err| err.to_string())?;
    let status = response.status().as_u16();
    let body = response
        .body_mut()
        .with_config()
        .limit(1024)
        .lossy_utf8(true)
        .read_to_string()
        .unwrap_or_default();
    Ok(format!("HTTP {status} {body}").trim().to_string())
}

/// 「试一下」的结果。**返回结构而不是一句话**：文案分语言（见 `lang.rs`），
/// Rust 这边只负责干活。
#[derive(serde::Serialize)]
struct ProbeOutcome {
    /// 走的是哪条路：`command` = 真跑了一遍接入命令；`plugin` = 插件式，内部注入
    kind: &'static str,
    /// 命令起来了没有（只有 `command` 那条路有意义）
    ran: bool,
    /// 退出码
    code: Option<i32>,
    /// 命令的输出（stdout + stderr）。失败时它就是原因。
    output: String,
    /// 跑的是哪条命令 —— 界面上要能看见「我们替你跑的是这一条」
    command: Option<String>,
    /// 内部注入时报了哪几个状态
    states: Vec<String>,
}

/// 「试一下」的等待上限。
///
/// 命令自己带网络超时（钩子那条约几秒），这里只是兜底：**绝不能让界面转不完**。
const PROBE_TIMEOUT: Duration = Duration::from_secs(8);

/// 试一下这条接入：**真的把配置里那条命令跑一遍**。
///
/// 这是「装了却没反应」最直接的自证。它验证的东西比回执（`receipts.rs`）更多：
/// 回执回答「最近有没有在报」，这里回答「**现在**这条链路还通不通」——包括那个
/// 最隐蔽的成因：app 从 Dock / LaunchAgent 起来时 PATH 和用户 shell 里的不一样，
/// 而配置里写的偏偏是裸命令名 `signalhub`。
///
/// 插件式接入（opencode / Pi / Hermes）没有能单独运行的东西 —— 它的代码跑在对方
/// 的进程里。那时退化成内部注入一条测试信号，**证不了对方会不会调我们**（那正是
/// 回执要回答的问题），所以 `kind` 要如实告诉界面走了哪条路。
#[tauri::command]
async fn try_agent(app: tauri::AppHandle, agent: String) -> Result<ProbeOutcome, String> {
    let agent = hooks::Agent::parse(&agent)?;
    // 两条路都在事后清理探测信号（见 `sweep_probe`）。放在最前面：命令式那条路
    // 是从这里返回的，它报上来的会话名同样是 `signalhub-probe`。
    sweep_probe(app.clone());

    if hooks::installed_http(agent) == Some(true) {
        // **HTTP 式那条路跑不了命令**（配置里根本没有命令），所以要单独探一次：
        // 真发一个 HTTP 过去，看那个端点认不认得我们的 payload。
        let port = config::read_port()
            .map_err(|err| format!("读不到上报服务的端口（应用还没跑起来？）: {err}"))?;
        let url = format!("http://127.0.0.1:{port}/v1/hooks/{}", hooks::cli_name(agent));
        let payload = hooks::probe_payload(agent)
            .ok_or_else(|| "这个智能体没有可用的探测事件".to_string())?;

        // **不能就地发**：ureq 是阻塞的，而这里是 async 上下文 —— 就地发会把整个
        // 异步运行时（上报服务也在上面）卡住最多 3 秒。这正是「界面卡住」和
        // 「上报超时」一起出现的那种怪现象的来源。
        let target = url.clone();
        let outcome = tauri::async_runtime::spawn_blocking(move || probe_http(&target, &payload))
            .await
            .map_err(|err| err.to_string())?;
        return Ok(match outcome {
            Ok(text) => ProbeOutcome {
                kind: "http",
                ran: true,
                code: Some(200),
                output: text,
                command: Some(url),
                states: Vec::new(),
            },
            Err(err) => ProbeOutcome {
                kind: "http",
                ran: false,
                code: None,
                output: err,
                command: Some(url),
                states: Vec::new(),
            },
        });
    }

    if let Some(payload) = hooks::probe_payload(agent) {
        let command = hooks::installed_command(agent)
            .ok_or_else(|| "配置里没有我们的命令（先点「接入」）".to_string())?;

        let (tx, rx) = std::sync::mpsc::channel();
        let program = command.clone();
        // 单独一个线程 + 有上限的等待：**界面不能被一条卡住的命令吊住**。
        std::thread::spawn(move || {
            use std::io::Write;

            let spawned = shell::command(&program)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn();
            let mut child = match spawned {
                Ok(child) => child,
                Err(err) => {
                    let _ = tx.send(Err(err.to_string()));
                    return;
                }
            };
            if let Some(stdin) = child.stdin.as_mut() {
                let _ = stdin.write_all(payload.as_bytes());
            }
            // 关掉 stdin：有些命令会一直等 EOF，不关就是白等一次超时
            drop(child.stdin.take());
            let _ = tx.send(child.wait_with_output().map_err(|err| err.to_string()));
        });

        let outcome = rx.recv_timeout(PROBE_TIMEOUT);
        return Ok(match outcome {
            Ok(Ok(output)) => ProbeOutcome {
                kind: "command",
                ran: true,
                code: output.status.code(),
                output: combined_output(&output),
                command: Some(command),
                states: Vec::new(),
            },
            Ok(Err(err)) => ProbeOutcome {
                kind: "command",
                ran: false,
                code: None,
                output: err,
                command: Some(command),
                states: Vec::new(),
            },
            Err(_) => ProbeOutcome {
                kind: "command",
                ran: false,
                code: None,
                output: "超时".to_string(),
                command: Some(command),
                states: Vec::new(),
            },
        });
    }

    // ---- 插件式：内部注入三条状态 ----
    //
    // 它证明的是「应用这一头是好的」：状态机、分组、窗口、灯。**不证明对方会调我们**——
    // 那句只能靠回执里的「最近上报」。
    let store = app.state::<SharedStore>();
    let states = ["idle", "working", "done"];
    for state in states {
        let req = Report {
            state: Signal::parse(state)?,
            // 会话名固定：它一出现就说明这是试出来的，而不是某个真实项目在报
            session: Some(PROBE_SESSION.into()),
            // 组名也留成探测名：灯上会显示「signalhub-probe · 执行中」，一眼认得出
            group: Some(PROBE_SESSION.into()),
            agent: Some("test".into()),
            source: Some("custom".into()),
            cwd: None,
            message: None,
            // 短停留：试出来的东西不该在灯上赖着
            ttl_ms: Some(PROBE_TTL_MS),
            pid: None,
            machine: None,
        };
        let ttl = report_ttl(&app, &req);
        mutate(&app, &store, |guard, now| guard.apply(now, req, ttl));
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    clear_session(app.clone(), PROBE_SESSION.to_string());

    Ok(ProbeOutcome {
        kind: "plugin",
        ran: true,
        code: None,
        output: String::new(),
        command: hooks::installed_command(agent),
        states: states.iter().map(|state| (*state).to_string()).collect(),
    })
}

/// 探测用的会话名。**两条路共用同一个名字**，这样：
///
/// - 灯上出现的这块牌一眼看得出是试出来的，而不是某个真实项目在报；
/// - 命令式那条路（真跑了一遍接入命令）报上来的会话名是同一个，所以收工时的清理
///   对两条路都管用。
const PROBE_SESSION: &str = "signalhub-probe";

/// 探测信号停留多久。
///
/// 命令式那条路带不了 TTL（钩子命令是装在对方配置里的字符串，不该为「试一下」改它），
/// 所以两条路都靠**跑完等一会儿再清掉**来收尾（见 [`sweep_probe`]）。
const PROBE_TTL_MS: u64 = 30_000;

/// 探测信号在灯上留多久，然后清掉。
///
/// 不留着它是有实际理由的：命令式接入报上来的那条用的是**默认停留时长**（`working`
/// 是 5 分钟），而一块亮五分钟的牌会让人以为真有个叫 `signalhub-probe` 的项目在跑。
/// 清一个不存在的会话是幂等的，所以这里不需要先判断它在不在。
fn sweep_probe(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(PROBE_TTL_MS)).await;
        clear_session(app, PROBE_SESSION.to_string());
    });
}

/// 命令的输出，stdout 和 stderr 拼起来。
///
/// 合并而不是只取一边：这条命令成功时**什么都不说**（钩子不许往 stdout 写东西，
/// 见 `bin/signalhub.rs` 顶部），失败时原因可能在任意一边。
fn combined_output(output: &std::process::Output) -> String {
    combined_bytes(&output.stdout, &output.stderr)
}

/// [`combined_output`] 的本体。拆出来只为让截断能在单测里直接喂字节 ——
/// 造一个 `std::process::Output` 需要先跑一个真进程，而这条规则和进程无关。
fn combined_bytes(stdout: &[u8], stderr: &[u8]) -> String {
    let mut text = String::from_utf8_lossy(stdout).into_owned();
    let err = String::from_utf8_lossy(stderr);
    if !err.trim().is_empty() {
        if !text.trim().is_empty() {
            text.push('\n');
        }
        text.push_str(&err);
    }
    // **按字符截**，判据也是字符数：一段 666 个汉字（1998 字节）加几个 ASCII 的
    // 输出只有 672 个字符，本来就该原样显示 —— 旧的按**字节** `truncate(2000)`
    // 既会在 UTF-8 中间 panic，也会把这种其实不长的中文输出截掉半截（issue #6）。
    // 复用 hookmap 里那条已经按字符走的实现，别再写一份。
    hookmap::clip(&text, 2000)
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
    why: Option<Vec<String>>,
) -> Result<(), String> {
    menu::show(&app, &window, &group, why.as_deref().unwrap_or_default())
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
    app.state::<SharedLang>().set(lang);
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
    let lang = app.state::<SharedLang>().get();
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
    app.state::<SharedLang>().get()
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
    // `force = true`：**用户主动改了设置**（以及启动后第一次按落盘设置摆放）。
    // 前端收到广播就立刻重画，窗口尺寸必须同一轮真的落地，不能靠增量缓存判断。
    sync_layout(app, settings, snapshot, true);
    // 开关可能刚被改过，所以尺寸之外还要重画一次托盘
    refresh_tray(app);
    // 灯可能刚被加上 / 刚被改过地址 / 刚换了配色 —— 立刻按**当前**状态发一次，
    // 那是「填完了到底通不通」唯一一次能马上看到的回答（见 `devices::sync`）。
    devices::sync(app, settings, snapshot);
    // 用户刚把通知打开时也要能申请到权限 —— 只在启动时申请的话，启动时通知是关着的
    // 用户永远不会被问一次，而没权限时通知是静默丢弃的（见 alerts::ensure_permission）。
    alerts::ensure_permission(app, settings);
    // 刚把「从灯上审批」关掉：**当场把还在等的那几条放掉**（对面立刻收到「没有决定」，
    // 回到终端里的询问框）。等 45 秒超时也是一种放掉，但那是「我已经关了它，它却还
    // 挡着智能体」—— 用户刚表达过「别管这件事」，就不该再等。
    if !settings.approve_tools {
        app.state::<approvals::SharedApprovals>().clear();
    }
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
/// 被截断的组那枚 `+N`）。暂停角标是绝对定位在灯体内部最上沿的徽标，
/// **不占点位、不进几何**。**必须和前端渲染对齐** —— 多一块牌或少一枚角标，
/// 窗口就会裁掉内容。
fn lamp_specs(app: &tauri::AppHandle, settings: &Settings, snapshot: &Snapshot) -> Vec<lamps::LampSpec> {
    if !per_group_active(settings, snapshot) {
        return Vec::new();
    }
    visible_group_names(settings, snapshot)
        .into_iter()
        .map(|name| {
            let label = lamps::label_for(&name);
            // 这一盏的圆环菜单开着时，窗口换成正方形（和主灯同一条规则）。
            // **不按折叠 / 展开算** —— 菜单形态是第三种，和那两种都无关。
            if lamp_menu_open(app, &label) {
                return lamps::LampSpec {
                    group: name,
                    size: config::menu_window_size(),
                };
            }
            // 折叠 / 展开按**这个灯窗**自己被 hover 与否算（多灯下各算各的）
            let collapsed = effective_collapsed(app, settings, &label);
            let has_message = !collapsed
                && settings.show.message
                && snapshot
                    .groups
                    .iter()
                    .find(|group| group.name == name)
                    .is_some_and(|group| group.message.is_some());
            let size = if collapsed {
                settings.folded_size(1, config::more_badge(1))
            } else {
                settings.window_size(&[config::sign_height(settings.size, has_message, settings.sign_h_mult)])
            };
            lamps::LampSpec { group: name, size }
        })
        .collect()
}

/// 让灯窗集合跟上这一帧。单灯模式下 `lamps::MAX_LAMPS` 那个分支不成立，`specs` 为空，
/// 于是所有组灯被关掉 —— 从多灯切回单灯自然也走这条路。
///
/// **锁里只算计划，窗口操作一律在锁外**。`open` 最终会调
/// `WebviewWindowBuilder::build()`，从非主线程发起时会阻塞等主事件循环；而主线程上
/// 的 `set_peek` / `set_settings` 也要调进这里抢同一把锁 —— 抱着锁建窗，两边就是
/// 互等（issue #4）。所以固定三步：锁内 `plan` → 锁外动窗口 → 锁内 `commit`。
///
/// **建 / 关还要再离开调用方的栈**（见 [`lamp_worker`]）：在 Windows 上就地建窗会
/// 整机冻住，而在 macOS 上完全看不出来。
fn sync_lamps(
    app: &tauri::AppHandle,
    settings: &Settings,
    snapshot: &Snapshot,
    force: bool,
) {
    let specs = lamp_specs(app, settings, snapshot);
    let handle = app.state::<SharedLamps>();

    // 只有改尺寸就够时**就地做**：那是热路径（折叠态悬停临时展开），晚一帧就能
    // 看见窗口跟不上鼠标。
    let resize_only = {
        let mut lamps = handle.lock().expect("lamps poisoned");
        // 用户改设置时丢掉尺寸记账，`plan` 就会对每个开着的窗重新生成 resize。
        // 理由见 `LampWindows::invalidate_sizes`：乐观记账一旦领先真实窗口，
        // 后续同尺寸的帧会一直跳过 `set_size`，灯被裁在旧窗口里。
        if force {
            lamps.invalidate_sizes();
        }
        let plan = lamps.plan(&specs);
        if plan.open.is_empty() && plan.close.is_empty() {
            Some(plan.resize)
        } else {
            // **在锁里投递**：这样队列顺序就是计划算出来的顺序。`send` 是不阻塞的
            // （无界通道），所以抱着锁投递没问题。乱序的代价见 [`lamp_worker`]。
            let _ = lamp_worker().send((app.clone(), plan));
            None
        }
    };

    if let Some(resize) = resize_only {
        use lamps::LampHost;

        let host = lamps::TauriHost(app);
        for (label, size) in &resize {
            host.resize(label, *size);
        }
    }
}

/// 建 / 关灯窗交给一条专门的线程，**按计划算出来的顺序**执行。
///
/// 为什么不能就地建窗：`WebviewWindowBuilder::build()` 在 Windows 上会走
/// `webview2_com::wait_with_pump` —— 那是一个**嵌套的 Windows 消息泵**。而 Windows
/// 上的 IPC（`set_settings` 就是从那儿来的）本身跑在 WebView2 的消息回调里，于是我们
/// 在它的回调内部又泵起一层消息循环去等它自己：回调不返回，它就不返回；它不返回，
/// 回调就不返回。**点一下「每组分灯」就整机冻住，而且和几盏灯无关 —— 建第一盏时
/// 就已经卡死了。**（用户报的就是这个：Windows、只能强制退出、一个组多个组都一样。）
///
/// macOS 上恰好不会：那里的 IPC 不在主线程，`build()` 走的是「投递给事件循环 +
/// 阻塞等回执」，事件循环是空闲的，所以这条 bug 只在 Windows 上现形。
///
/// 关窗（`destroy`）和改尺寸（`set_size`）都是直接投递、不等回执的，本来安全；
/// 但**关也必须排在同一条队列里**：乐观记账让「切过去」和「切回来」两份计划可以紧挨
/// 着算出来，乱序执行会留下一个谁也不认的孤儿灯窗（关的时候窗还没建出来，
/// 建完就没人记得它了）。
fn lamp_worker() -> &'static std::sync::mpsc::Sender<(tauri::AppHandle, lamps::Plan)> {
    use std::sync::{mpsc, OnceLock};

    static QUEUE: OnceLock<mpsc::Sender<(tauri::AppHandle, lamps::Plan)>> = OnceLock::new();

    QUEUE.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<(tauri::AppHandle, lamps::Plan)>();
        std::thread::spawn(move || {
            // 通道一直活到进程结束（`QUEUE` 是静态的），所以这里不会提前退出
            for (app, plan) in rx {
                run_lamp_plan(&app, &plan);
            }
        });
        tx
    })
}

/// 真正动窗口的那一段：关 → 改尺寸 → 建 → 记账。
///
/// 跑在 [`lamp_worker`] 的线程上，**不是** IPC / 事件循环那条线程。
fn run_lamp_plan(app: &tauri::AppHandle, plan: &lamps::Plan) {
    use lamps::LampHost;

    let started = std::time::Instant::now();
    let host = lamps::TauriHost(app);
    let closed: Vec<(String, Option<(i32, i32)>)> = plan
        .close
        .iter()
        .map(|label| (label.clone(), host.close(label)))
        .collect();
    for (label, size) in &plan.resize {
        host.resize(label, *size);
    }
    let failed: Vec<String> = plan
        .open
        .iter()
        .filter(|open| !host.open(&open.label, &open.group, open.at, open.size))
        .map(|open| open.label.clone())
        .collect();

    // 记账 + 取快照都在这一小段里；落盘放到锁外。
    let state = app.state::<SharedLamps>();
    let (dirty, positions) = {
        let mut lamps = state.lock().expect("lamps poisoned");
        let dirty = lamps.commit(&closed, &failed);
        (dirty, lamps.positions_snapshot())
    };
    if dirty {
        lamps::save_positions(&positions);
    }

    // 只在**真建了窗**的时候记一行：这是唯一会创建 webview 的操作，也是最慢、
    // 最出过事的一步（Windows 上它曾经把整机冻住，见 [`lamp_worker`]）。关灯窗
    // 和改尺寸不值得各记一行 —— 这个文件同时装着 hook 事件，不能变成流水账。
    if !plan.open.is_empty() {
        config::append_hook_log(&format!(
            "{} [lamp] 建窗完成：成功 {} 盏 / 共 {} 盏（{} ms）\n",
            config::utc_hms(),
            plan.open.len() - failed.len(),
            plan.open.len(),
            started.elapsed().as_millis(),
        ));
    }
}

/// 一次状态 / 设置变化之后，把**所有**窗口的布局同步一遍：主灯尺寸 + 各组的灯。
/// 待审批气泡要占的地方：`(宽, 高)`。没有待审批时是 `(0, 0)`。
///
/// 单独一个函数，因为它读的是**和快照无关**的一份状态（审批登记表）。窗口尺寸本来
/// 只由「牌」决定，现在多了一个来源 —— 而它在 `apply_window_size` 里，那是热路径，
/// 所以这里只做一次加锁 + 取长度。
fn approval_area(app: &tauri::AppHandle) -> (u16, u16) {
    let count = app
        .state::<approvals::SharedApprovals>()
        .list()
        .len()
        .min(MAX_APPROVAL_BUBBLES);
    if count == 0 {
        return (0, 0);
    }
    (
        config::APPROVAL_BUBBLE_W,
        config::APPROVAL_BUBBLE_H * count as u16,
    )
}

/// 一次最多画几条待审批。多出来的**不画**（也不占窗口高度）—— 它们不会丢：那几条
/// 的牌在灯上是黄的，而每答完一条，下一条会自己冒出来。真堆到三条以上时，
/// 「先把眼前这条处理掉」本来就是对的顺序。
const MAX_APPROVAL_BUBBLES: usize = 3;

/// `force` = 用户**主动改了设置**（`apply_settings`）。热路径传 false。
///
/// 两处尺寸都是「缓存里记着上次设下去的值，一样就跳过」的增量判断：主灯是
/// `SharedGeometry.size`，组灯是 `LampWindows.sizes`。它们对状态推送（每秒好几次）
/// 是必要的优化，但前端在设置广播到达时**立刻**按新设置重画了 DOM —— 只要缓存与
/// 真实窗口失步（圆环菜单开合、hover 临时展开、某次 `set_size` 没落地，或新设置
/// 恰好算出同尺寸），Rust 这边就会跳过 `set_size`，窗口停在旧尺寸、内容被
/// `overflow: hidden` 裁掉。改设置是低频且必须生效的操作，所以这条路径强制重设
/// 主灯尺寸、并让组灯重新生成 resize；状态热路径仍走增量，别把那段也拖慢。
fn sync_layout(
    app: &tauri::AppHandle,
    settings: &Settings,
    snapshot: &Snapshot,
    force: bool,
) {
    apply_window_size(app, settings, snapshot, force);
    sync_lamps(app, settings, snapshot, force);
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

/// 按可见牌的个数（折叠时是点的个数）调整窗口尺寸。
///
/// 暂停角标不在这儿：它是绝对定位的徽标，不占点位，切换它不该改窗口尺寸。
/// 只在尺寸真的变了才 `set_size`：状态变化一秒可能好几次，每次都 resize
/// 是没必要的系统调用。
///
/// **热路径上不问窗口**（见 `SharedGeometry`）：缩放比和上次设下去的尺寸都
/// 从缓存读，一次 `inner_size` / `scale_factor` 的同步往返都不做。
///
/// `force` 的由来看 [`sync_layout`]：用户改设置时跳过「和上次一样就收工」，
/// 但**仍然更新缓存与 `previous`**，所以 `keep_on_screen` / `center_grow` 拿到的
/// 旧尺寸照旧是对的，不会因为强制重设而把灯挪偏。
fn apply_window_size(
    app: &tauri::AppHandle,
    settings: &Settings,
    snapshot: &Snapshot,
    force: bool,
) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };

    // 圆环菜单开着时窗口换成正方形 —— 这一条必须**先判**：下面那套「牌 / 点」
    // 的几何对菜单形态没有意义，而菜单一关就该回到它。
    let menu_open = lamp_menu_open(app, "main");

    // 展开态是一叠牌，折叠态是一列 / 一排格 —— 两套几何，见 config.rs。
    // 多灯模式下主灯只看全局（各组交给各自的灯），所以这里拿的是「主灯该画的组」。
    let groups = main_window_groups(settings, snapshot);
    // 悬停临时展开也算「不折叠」（见 `effective_collapsed`）—— 前端会按同一个条件
    // 渲染，两边不一致就会裁内容。
    let collapsed = effective_collapsed(app, settings, "main");
    // 待审批的气泡挂在牌子下面（见 `approval_area`）：有多条就多占一份高度。
    let (bubble_w, bubble_h) = approval_area(app);
    let (width, height) = if menu_open {
        // 菜单形态：正方形，圆环居中。牌和气泡都不画（前端也只画圆环 + 中心那颗
        // 灯珠），所以**不叠** approval_area —— 叠上去圆环就不在窗口中心了。
        config::menu_window_size()
    } else {
        let (width, height) = if collapsed {
            // 折叠态里没有牌面，被截断的组那枚 `+N` 得自己占一个点位。
            // 少算一枚，那枚角标就落在窗口外面 —— 而折叠态最不该出的就是这种错：
            // 看不见的东西，人不会想到去查。
            // 暂停角标不在这里：它是灯体内部最上沿的绝对定位徽标
            // （`.lamp-flag`），不占点位，所以切换穿透不该让窗口变宽、把整排灯珠推偏。
            //
            // **点数和角标都按未截断的可见组数算**：`groups` 已经被 `MAX_SIGNS` 截过，
            // 拿它的长度喂 `more_badge` 会恒得 0 —— 5 组以上时那枚 `+N` 就被裁掉了
            // （issue #7）。`collapsed_window_size` 内部会把点数夹到 `MAX_DOTS`。
            // 多灯模式下主灯只剩一颗全局灯，没有「多出来几组」可言。
            let visible = if per_group_active(settings, snapshot) {
                0
            } else {
                visible_group_names(settings, snapshot).len()
            };
            let badges = config::more_badge(visible);
            settings.folded_size(visible, badges)
        } else {
            settings.window_size(&sign_heights(settings, &groups))
        };

        (width.max(bubble_w), height + bubble_h)
    };

    // 和上次设下去的尺寸一样就收工。**这是热路径**：拿缓存比，不问窗口。
    // `force` 只在用户主动改设置时为 true —— 那条路上「缓存说没变」不可信，
    // 前端已经重画了，必须再设一次（见 `sync_layout`）。
    let (previous, scale) = {
        let handle = app.state::<SharedGeometry>();
        let mut geometry = handle.lock().expect("geometry poisoned");
        if !force && geometry.size == Some((width, height)) {
            return;
        }
        let previous = geometry.size;
        geometry.size = Some((width, height));
        (previous, geometry.scale)
    };

    let _ = window.set_size(LogicalSize::new(width, height));
    // 尺寸变了才可能越界，所以放在这个分支里。
    //
    // **`was_menu` 这一半不能省**：菜单关掉那一帧 `menu_open` 已经是 false 了，
    // 只按它判断的话，关闭会走 `keep_on_screen` 的「贴边往里长」—— 而打开走的是
    // 居中扩展，两者不对称，灯就会往左上偏一截（196→174 差 22 宽，一半是 11px；
    // 高度差 141，一半是 70px）。一次偏这么多，用户一眼就看得出来。
    // 判据用「上一次设下去的尺寸是不是菜单尺寸」，而不是再加一个状态位。
    let was_menu = previous == Some(config::menu_window_size());
    if menu_open || was_menu {
        // 菜单是**以窗口中心为基准**扩大的：灯原地展开成一个圆环，而不是从左上角
        // 长出去（那样圆环会整体偏向右下，看着像灯跳了一下）。
        // 不能走 `keep_on_screen` —— 它的 `grow_away_from_edges` 会按「贴边就往里
        // 长」再挪一次，和这里的居中偏移叠起来，圆环就歪了。
        center_grow(&window, previous, (width, height), scale);
    } else {
        keep_on_screen(&window, width, height, previous, scale);
    }
}

/// 某一盏灯上的圆环菜单是不是开着（同 `effective_collapsed` 那套 label 判据）。
fn lamp_menu_open(app: &tauri::AppHandle, label: &str) -> bool {
    app.state::<SharedUi>()
        .lock()
        .expect("ui poisoned")
        .menu
        .as_deref()
        == Some(label)
}

/// 尺寸变化时让窗口**中心不动**：左上角让出两个方向差值的一半。
///
/// 只在圆环菜单打开 / 关闭时用。为什么要居中而不是像 `grow_away_from_edges`
/// 那样贴边往里长：菜单的圆心是窗口中心，中心一动圆环就跟着跳。居中扩展让
/// 灯看起来是「原地展开」的，关掉时又原路缩回去。
///
/// `previous` 是变化前的逻辑尺寸（`None` = 还没设过，不挪）。最后一步仍然夹回
/// 工作区：灯贴着屏幕右下角时，居中扩展会有一部分长到屏外，而圆环被切掉的那半边
/// 就等于点不到的按钮。
fn center_grow(
    window: &tauri::WebviewWindow,
    previous: Option<(u16, u16)>,
    size: (u16, u16),
    scale: f64,
) {
    let Some(previous) = previous else {
        return;
    };
    let Ok(position) = window.outer_position() else {
        return;
    };
    let to_physical = |(w, h): (u16, u16)| {
        (
            (w as f64 * scale).round() as i32,
            (h as f64 * scale).round() as i32,
        )
    };
    let old = to_physical(previous);
    let new = to_physical(size);
    let target = centered_offset((position.x, position.y), old, new);

    let (x, y) = match screen_bounds(window) {
        Some(bounds) => clamp_to_area(target, new, bounds),
        None => target,
    };
    if (x, y) != (position.x, position.y) {
        let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
    }
}

/// 让窗口中心不动的那个左上角：新位置 = 旧位置 −（新尺寸 − 旧尺寸）/ 2。
///
/// 差值可能不是偶数（缩放比不是整数时），整除截断会让中心差半个物理像素 ——
/// 肉眼不可见，且每次都朝同一个方向，不会累积漂移。
///
/// 抽成纯函数是为了能单测：菜单一开一关是一次往返，这里算错一点点，
/// 灯就会在屏幕上来回蹭 —— 而那种「用久了灯自己跑偏」的 bug 最难查。
fn centered_offset(position: (i32, i32), previous: (i32, i32), size: (i32, i32)) -> (i32, i32) {
    (
        position.0 - (size.0 - previous.0) / 2,
        position.1 - (size.1 - previous.1) / 2,
    )
}

/// 这个窗口现在盖在哪块屏的工作区上；拿不到就退回「离它最近的那块」。
///
/// 从 `keep_on_screen` 里提出来：圆环菜单的居中扩展也要夹回同一块屏，
/// 两处判据必须一致（否则会出现「主灯夹在 A 屏、菜单夹在 B 屏」）。
fn screen_bounds(window: &tauri::WebviewWindow) -> Option<Bounds> {
    if let Ok(Some(monitor)) = window.current_monitor() {
        return Some(work_bounds(&monitor));
    }
    let position = window.outer_position().ok()?;
    nearest_screen(window.app_handle(), (position.x, position.y)).map(|(bounds, _)| bounds)
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

/// 「打开设置，并且翻到某一页」那个事件的 id。
///
/// **与前端 `src/lib/settingsIntent.ts::OPEN_TAB_EVENT` 逐字一致** —— 两边各写
/// 一份字符串是刻意的（Rust 读不到 TS 常量），改名要一起改，否则表现是「点了
/// 重命名，设置窗开了但停在原来那页」，没有任何报错。
const SETTINGS_TAB_EVENT: &str = "signalhub://settings-tab";

/// 打开设置并翻到某一页。
///
/// 为什么要事件：`reveal_settings` 只是 `show()` —— 设置窗启动时就以
/// `visible: false` 建好了（组件只挂载一次），翻页只能靠「窗口显形之后再告诉它」。
/// 前端自己的入口走 localStorage 那张纸条（同源两窗口共享），而灯上的原生右键
/// 菜单在 Rust 这边，写不了 localStorage，只能走事件。
///
/// **先 show 再 emit**：事件是发给那个窗口的，窗口还没显示时它照样收得到（webview
/// 已经加载完），但先显示更符合「用户点了什么就立刻看到什么」的顺序。
pub(crate) fn reveal_settings_tab(app: &tauri::AppHandle, tab: &str) {
    reveal_settings(app);
    let _ = app.emit(SETTINGS_TAB_EVENT, tab);
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
            // 收件箱可能还开着 —— 那个窗口也需要 Regular / 焦点，不能在这里
            // 无条件退回 Accessory。判断收在 `inbox::restore_activation` 一处。
            inbox::restore_activation(window_handle.app_handle());
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

    // 设置**必须在窗口创建之前**读好并注册，不能等到 `setup` 里（issue #24）。
    //
    // 前端一挂载就 `invoke("get_settings")`（见 `useSettings.ts`），而 `setup` 是在
    // 窗口创建**之后**才跑的 —— 中间有一段窗口期。Windows 上 WebView2 起得比后端快
    // （冷启动、盘慢、杀毒扫描）时，前端会抢在 `setup` 的 `manage()` 之前调用命令，
    // `state::<SharedSettings>()` 找不到状态，Tauri 直接 panic：
    //
    //   state() called before manage() for Arc<Mutex<Arc<Settings>>>   (tauri 2.12.2)
    //
    // `load_settings()` 不依赖 `AppHandle`，所以能提到这里；`.manage()` 在 `build()`
    // 里、窗口创建之前执行，前端再快也只能在那之后才拿得到 webview。
    let settings = config::load_settings();

    tauri::Builder::default()
        // 单实例要最先注册：多开会让端口文件和状态互相打架，表现为「灯球
        // 偶尔不动」这种极难排查的现象。
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            // 第二个实例起来时打开收件箱 —— 用户多半是点了应用图标 / `open -a`
            // 想看看「现在有什么要处理」。灯球本身一直可见，再把它 show 一遍没有信息量。
            crate::inbox::show(app);
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
        // 自动更新。**目前只在 macOS 上真正生效**（见 update.rs 顶部）。
        .plugin(tauri_plugin_updater::Builder::new().build())
        // 设置排在最前：前端发出的第一条命令就是 `get_settings`，它必须已经在了
        // （issue #24，见 `run()` 开头那段注释）
        .manage(Arc::new(Mutex::new(Arc::new(settings))))
        .manage(SharedStore::new(Mutex::new(Store::default())))
        .manage(SharedUi::default())
        .manage(SharedGeometry::default())
        // 位置记忆从盘上读回来（`LampWindows::load`），这样组灯一开就在上次的位置
        .manage(Mutex::new(lamps::LampWindows::load()))
        .manage(SharedBind::default())
        // 每个信号源「最近一次上报」的凭证：接入页要能回答「到底有没有在报」，
        // 而不是只说一句「文件写进去了」。
        .manage(Arc::new(receipts::Receipts::load()))
        // 统计从盘上读回来：灯球是常驻的，但「常驻」不等于「永不重启」——
        // 改设置、升级、崩了重来，用户对「今天等了多久」的期待是跨重启的。
        .manage(Arc::new(Mutex::new(stats::Stats::load())))
        // 启动时把上次的转发结果读回来：日志原来是纯内存的，重启一次界面就显示
        // 「从未触发」—— 用户重启一次就会以为转发坏了。
        .manage(outbound::load_log())
        // 每台物理灯「上一次成功发出去的颜色」（不落盘，重启后重新发一次）
        .manage(devices::SharedDevices::default())
        .manage(SharedPalette::default())
        .manage(alerts::LocalHour::default())
        .manage(Arc::new(lang::LocalLang::default()))
        // 最近一条告警通知说的是谁 —— 动作按钮回调时要用，见 alerts::LastAlert
        .manage(alerts::LastAlert::default())
        // 更新检查的结果（不联网，纯粹给界面读的一张快照）
        .manage(update::SharedUpdate::default())
        // 右键菜单只记「刚点的是哪一组」，见 menu.rs
        .manage(menu::Target::default())
        // 四个全局快捷键抢到了没有 —— 注册时算一次，设置 →「系统」页读它
        .manage(shortcuts::SharedShortcuts::default())
        // 专注倒计时的代号（取消上一个线程用，见 manual.rs）
        .manage(manual::SharedManual::default())
        .manage(watches::Watches::default())
        // 等用户点的那些审批（灯上的气泡读它，见 approvals.rs）。
        // 历史落到 `~/.signalhub/approvals/`；拿不到目录就只在内存里 —— 功能不受影响。
        .manage(std::sync::Arc::new(approvals::Registry::new(
            approval_log::dir().ok(),
        )))
        // 多机中枢的健康状态（轮询线程写，「多机」页读，见 remote.rs）
        .manage(remote::registry())
        // Call Me 的拉取状态（轮询线程写，「留言」页读，见 callme.rs）
        .manage(callme::state())
        // 可选的本机收信服务。默认不开，只有用户在「Call Me → 服务器」点启动。
        .manage(callme_local::state())
        // Windows 通知身份注册的结果。放进 manage 链而不是 setup 里，是为了让
        // 前端抢在 setup 跑完之前调 `get_notify_permission` 也读得到一份确定的状态。
        .manage(win_toast::ToastBackend::new(None))
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
            open_releases,
            open_url,
            open_app_dir,
            open_settings,
            toggle_inbox,
            hide_inbox,
            jump_to_session,
            mute_group,
            get_toggles,
            set_autostart,
            set_pass_through,
            set_peek,
            get_stats,
            get_history,
            get_timeline,
            get_messages,
            get_outbound_log,
            get_shortcuts,
            get_approvals,
            get_approval_history,
            get_events,
            watch_session,
            unwatch_session,
            get_watches,
            decide_approval,
            get_notify_permission,
            get_notify_backend,
            ensure_notify_backend,
            request_notify_permission,
            test_notification,
            detect_agents,
            custom_agents_info,
            open_custom_agents,
            add_custom_agent,
            agent_receipts,
            get_hook_log,
            update::get_update_state,
            update::check_update,
            update::install_update,
            install_agent_hooks,
            install_agent_http_hooks,
            set_tool_approval,
            list_collectors,
            set_collector,
            list_machines,
            probe_machine,
            get_callme,
            callme_probe,
            callme_add_source,
            callme_revoke_source,
            callme_rename_source,
            callme_delete_source,
            callme_restore_source,
            callme_local_status,
            callme_local_start,
            callme_local_stop,
            callme_mark_read,
            uninstall_agent_hooks,
            try_agent,
            diagnose,
            toggle_collapsed,
            reset_lamp_positions,
            set_lamp_menu,
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

            // 设置已经在 Builder 链上注册过了（见 `run()` 开头，issue #24）。
            // 这里从状态里读回来，而不是再 `load_settings()` 一次 —— 读两次会让
            // 内存里这份和 `server::Shared` 拿到的那个 `Arc` 各走各的。
            let settings = current_settings(app.handle());

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

            // 审批历史超过保留期的文件删掉。启动时做一次就够 —— 它不是实时要求。
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_millis() as u64)
                .unwrap_or(0);
            approval_log::prune(now_ms);
            events::prune(now_ms);

            // token 建不出来说明 ~/.signalhub 不可写。这是无法降级的配置问题，
            // 让启动直接失败并打出来，好过一个「能启动但上报接口静默不工作」的 app。
            let token = config::load_or_create_token()?;

            // 通知器只有一个构造入口（notify::compose），它把窗口尺寸、统计、
            // 托盘图标、系统通知、出站钩子、推事件这几件事一起管起来。
            let notify: Notifier = notify::compose(app.handle().clone());
            app.manage(notify.clone());

            // Windows 的通知身份必须在**第一条通知之前**注册好：插件会把通知发给
            // `tauri.conf.json` 的 identifier，而这个 AUMID 只有开始菜单快捷方式
            // 带着它时系统才肯投递。portable zip 没有安装器，缺的正是这一步
            // （见 win_toast.rs 与 issue #25）。失败不阻断启动，记进状态让设置页
            // 说清楚，并提供「重新注册」。
            if let Err(err) = win_toast::ensure(&app.config().identifier) {
                eprintln!("[signalhub] Windows 通知身份注册失败：{err}");
                app.state::<win_toast::ToastBackend>().set(Some(err));
            }

            alerts::ensure_permission(app.handle(), &settings);
            // 通知上的动作按钮（已知晓 / 静音该组）。注册类型和挂处理函数
            // 必须一起做，只做一半的表现是「按钮从来没见过」且不报错。
            alerts::wire_actions(app.handle());

            // 全局快捷键。**不阻断启动**：抢不到一个组合不该让 app 起不来，
            // 而且抢不到这件事会记进状态、在设置 →「系统」页明说出来 —— 一个
            // 按了没反应的键比一个「这一个没生效」的说明糟得多。
            let registered = shortcuts::register_all(app.handle());
            // 打一行：不开设置窗口时，这是唯一能确认「快捷键到底抢到了没有」的地方。
            // 文案从注册结果派生，所以不会和真正生效的那个组合漂移。
            eprintln!(
                "[signalhub] 全局快捷键: {}",
                registered
                    .iter()
                    .map(|status| if status.ok {
                        status.label.clone()
                    } else {
                        format!("{}（没抢到）", status.label)
                    })
                    .collect::<Vec<_>>()
                    .join(" / ")
            );
            *app.state::<shortcuts::SharedShortcuts>()
                .lock()
                .expect("shortcut status poisoned") = registered;

            wire_settings_window(app);
            inbox::wire(app);
            // 右键菜单是运行时构造的，`Menu` 自己没有事件回调 —— 只能挂在应用上，
            // 靠 id 分派。托盘菜单有它自己的回调，两边的 id 不重合，互不打扰。
            app.on_menu_event(|app, event| {
                menu::handle(app, event.id().as_ref());
            });
            tray::init(app.handle().clone())?;
            sweep::spawn(notify.clone(), app.state::<SharedStore>().inner().clone());
            // 内置采集器（B2）：一条采集器一条线程，自己在每一跳看开关。
            // 做不到的（不是 macOS）在 `spawn` 里直接返回，不会起空转的线程。
            for collector in collect::Collector::ALL {
                collector.spawn(app.handle().clone());
            }
            // 预设规则（B4）：一条线程、5 秒一拍，只读快照和设置。
            rules::spawn(app.handle().clone());
            // 更新检查是**静默**的后台任务（见 update.rs 顶部）：失败只写日志，
            // 绝不在用户没点过「检查更新」的时候弹东西。
            update::spawn_auto_check(app.handle().clone());
            // 审批列表变了就广播一次：灯上的气泡据此刷新。服务器那边**不持有
            // AppHandle**（见 `server::spawn` 的注释），所以这件事在这里挂进去。
            //
            // **还要重算布局**：气泡是挂在牌子下面的（见 `approval_area`），而
            // 「没人点、超时了」这条路上一次状态推送都没有 —— 只广播不重算的话，
            // 窗口会一直留着一块空出来的地方。
            {
                let handle = app.handle().clone();
                app.state::<approvals::SharedApprovals>()
                    .inner()
                    .on_change(std::sync::Arc::new(move || {
                        let _ = handle.emit(APPROVALS_EVENT, ());
                        let settings = current_settings(&handle);
                        let snapshot = current_snapshot(&handle);
                        // 审批条数变化只改高度，走增量即可。
                        sync_layout(&handle, &settings, &snapshot, false);
                    }));
            }
            server::spawn(
                notify.clone(),
                server::Shared {
                    // 全部取自 app 里 manage 的那几份 —— 各写一遍真值迟早会漂移
                    store: app.state::<SharedStore>().inner().clone(),
                    settings: app.state::<SharedSettings>().inner().clone(),
                    bind: app.state::<SharedBind>().inner().clone(),
                    receipts: app.state::<SharedReceipts>().inner().clone(),
                    lang: app.state::<SharedLang>().inner().clone(),
                    approvals: app.state::<approvals::SharedApprovals>().inner().clone(),
                },
                token,
            );

            // 多机中枢（见 `remote.rs`）。**排在 `server::spawn` 之后**：它是纯
            // 出站轮询，不依赖本机的监听端口；而且 `notify` 已经被上面那个 move
            // 吃掉了，所以这里传的是克隆 —— 同一个组合版通知器（记统计、发通知、
            // 转发都在里面），不是另造一个。
            remote::spawn(
                app.state::<SharedStore>().inner().clone(),
                app.state::<SharedSettings>().inner().clone(),
                app.state::<SharedMachines>().inner().clone(),
                notify,
            );

            // Call Me（见 `callme.rs`）。纯出站轮询，不依赖 `notify`，所以放在
            // remote 之后；没配置服务器时线程只是空转，一个请求都不发。
            callme::spawn(
                app.handle().clone(),
                app.state::<SharedSettings>().inner().clone(),
                app.state::<callme::SharedCallMe>().inner().clone(),
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
        centered_offset, clamp_to_area, combined_bytes, distance_to_bounds, grow_away_from_edges,
        nearest_index, snap_to_edges, state_ttl_defaults, validate_settings, visible_groups,
        TEST_REPORT_TTL_MS,
    };
    use crate::config::{GroupMeta, Settings, StateStyle, MAX_STATE_TTL_MS, MIN_STATE_TTL_MS};
    use crate::state::{state_ttl, GroupView, Report, Signal, Snapshot, Store};
    use std::time::{Duration, Instant};

    // ---- 测试上报 -------------------------------------------------------------

    /// 命令输出截断**必须按字符走**：`String::truncate` 落在 UTF-8 中间会 panic，
    /// 而这条路径喂的是用户命令的原始输出 —— 中文帮助文本超过 2 KB 太容易了，
    /// 用户把 `--help` 误写进 hook 命令时正好会走到（issue #6）。
    #[test]
    fn a_long_multibyte_output_is_clipped_without_panicking() {
        // 6000 个汉字 = 18000 字节，第 2000 个字节必定落在某个汉字中间
        let stdout = "汉".repeat(6000);
        let text = combined_bytes(stdout.as_bytes(), b"");
        assert_eq!(text.chars().count(), 2001, "2000 个字符 + 一个省略号");
        assert!(text.ends_with('…'));
    }

    /// 反过来也要钉住：**字符数少、字节数多**的中文输出不该被当成超长。
    /// 666 个汉字 + 6 个 ASCII 是 2004 字节、672 个字符 —— 按旧实现会被截掉。
    #[test]
    fn a_short_multibyte_output_is_left_alone() {
        let mut stdout = "汉".repeat(666);
        stdout.push_str("abcdef");
        assert!(stdout.len() > 2000, "字节数确实过线了");
        assert_eq!(combined_bytes(stdout.as_bytes(), b""), stdout);
    }

    /// stdout 和 stderr 都要带上：命令成功时什么都不说，失败的原因可能在任意一边。
    #[test]
    fn both_streams_are_kept() {
        assert_eq!(combined_bytes(b"out", b"err"), "out\nerr");
        // 一边为空时不留一个多余的空行
        assert_eq!(combined_bytes(b"out", b""), "out");
        assert_eq!(combined_bytes(b"", b"err"), "err");
    }

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
                source: None,
                cwd: None,
                message: None,
                ttl_ms: Some(TEST_REPORT_TTL_MS),
                pid: None,
                machine: None,
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
        // 从 `default_ttl()` 推，不写死毫秒数 —— 这两个默认值正是会被产品调优
        // 改来改去的东西（`done` 30s → 180s、`working` 300s → 600s）。
        assert_eq!(
            defaults["working"],
            Signal::Working.default_ttl().as_millis() as u64
        );
        assert_eq!(
            defaults["done"],
            Signal::Done.default_ttl().as_millis() as u64
        );
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

    // ---- 圆环菜单的居中扩展 ---------------------------------------------------

    /// 变大时中心不动：左上角让出两个方向差值的一半。
    ///
    /// 例：174×55 的灯长到 196×196，宽差 22 → x 减 11，高差 141 → y 减 70
    /// （整除截断）。中心仍然是原来那个点，所以圆环看起来是「原地展开」的。
    #[test]
    fn growing_keeps_the_center_still() {
        assert_eq!(
            centered_offset((100, 200), (174, 55), (196, 196)),
            (100 - 11, 200 - 70)
        );
    }

    /// **一开一关必须回到原点** —— 这是「灯会不会用久了慢慢跑偏」的判据。
    ///
    /// 菜单是高频操作（每次长按都开一次），只要往返差一个像素，用几百次之后
    /// 灯就从屏幕角落挪走了。所以这条测试盯的是「对称性」，不只是「方向对」。
    #[test]
    fn closing_the_menu_returns_to_the_same_place() {
        let start = (640, 480);
        let small = (174, 55);
        let big = (196, 196);
        let opened = centered_offset(start, small, big);
        let closed = centered_offset(opened, big, small);
        assert_eq!(closed, start, "开→关必须回到原位");
    }

    /// 尺寸没变就一个像素都不动（状态推送会让 `apply_window_size` 反复跑，
    /// 这里多挪一下灯就会自己走）。
    #[test]
    fn an_unchanged_size_does_not_move_the_window() {
        assert_eq!(centered_offset((300, 400), (196, 196), (196, 196)), (300, 400));
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
            snooze_until_ms: 0,
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
            last_done_ms: 0,
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
