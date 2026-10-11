use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// 圆点直径的允许范围。config.json 是用户可手改的文件，属于系统边界，
/// 所以加载时要夹紧——否则一个手滑的 2000 会在屏幕上摊出一个巨型窗口。
const MIN_SIZE: u16 = 12;
const MAX_SIZE: u16 = 64;
pub const DEFAULT_SIZE: u16 = 22;

/// 圆点四周留白的**总量**（不是每边）。留白是为了好抓取圆点来拖动。
pub const LAMP_MARGIN: u16 = 20;

/// 灯球最多同时显示几个会话。超出的按优先级截断 —— 会话列表本就是按优先级
/// 排的，所以截断后剩下的恰好是最紧急的那几个。
pub const MAX_DOTS: u16 = 3;

/// 展开面板的宽度。固定值：面板内容是一列短文本，宽度不必随会话数变。
pub const PANEL_WIDTH: u16 = 260;
/// 面板的固定开销：上下内边距 + 头部那一行。
pub const PANEL_CHROME: u16 = 56;
/// 面板里每一行的高度。**CSS 里的 `.lamp-row` 必须正好也是这个高度**，
/// 否则窗口会裁掉最后一行、或者底下留一条缝。
pub const PANEL_ROW: u16 = 38;

/// `~/.signalhub`
///
/// 这里不用 Tauri 的 `PathResolver`：CLI 是独立二进制、不链接 Tauri，
/// 而两边必须解析出完全相同的路径，所以只能靠一个共享的实现。
pub fn data_dir() -> io::Result<PathBuf> {
    let dir = home_dir()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "找不到用户主目录"))?
        .join(".signalhub");
    if !dir.exists() {
        fs::create_dir_all(&dir)?;
        set_mode(&dir, 0o700)?;
    }
    Ok(dir)
}

/// 只认环境变量，不引入 `dirs` 之类的依赖。
/// 代价是 HOME / USERPROFILE 都缺失的极端情况会失败（此时会明确报错，不会
/// 静默写错地方）；换来的是 CLI 的依赖树里少一整棵。app 由终端或 launchd
/// 启动时两者必有其一，所以这个取舍是安全的。
pub fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    let key = "USERPROFILE";
    #[cfg(not(windows))]
    let key = "HOME";

    std::env::var_os(key)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

pub fn port_file() -> io::Result<PathBuf> {
    Ok(data_dir()?.join("port"))
}

fn token_file() -> io::Result<PathBuf> {
    Ok(data_dir()?.join("token"))
}

/// 读取 app 写入的端口。CLI 用它自动发现服务；读不到即视为 app 未运行。
pub fn read_port() -> io::Result<u16> {
    let raw = fs::read_to_string(port_file()?)?;
    raw.trim().parse().map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidData, format!("端口文件内容不是数字: {raw:?}"))
    })
}

/// 读取已有 token。
///
/// CLI 只该调用这个，绝不该走 `load_or_create_token` —— 自己生成会造出一个
/// app 并不认识的值，表现为「服务在跑但所有上报都 401」，很难排查。
pub fn read_token() -> io::Result<String> {
    let raw = fs::read_to_string(token_file()?)?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "token 文件为空"));
    }
    Ok(trimmed.to_string())
}

/// 首次启动生成 token，之后复用。
/// 复用而非每次重生成，是为了重启 app 后已配置好的 CLI / hook 不用改。
pub fn load_or_create_token() -> io::Result<String> {
    // 读失败（含文件不存在、内容为空）就当作需要新建。真要是权限问题，
    // 下面那次写入会再报一次错，错误信息不会丢。
    if let Ok(existing) = read_token() {
        return Ok(existing);
    }

    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|err| io::Error::other(format!("生成随机 token 失败: {err}")))?;
    let token: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();

    write_private(&token_file()?, &token)?;
    Ok(token)
}

/// 把实际监听端口写进文件，供 CLI 自动发现。
pub fn write_port(port: u16) -> io::Result<()> {
    fs::write(port_file()?, port.to_string())
}

/// hook 调用日志的路径。
///
/// 存在的原因很实际：出问题时（「灯为什么显示成这样」）需要知道**哪些事件
/// 真的触发了**。事件本身看不到的话，这类问题只能靠猜——而「一轮结束时
/// Stop 有没有触发」正是这样一个问题。
pub fn hook_log_path() -> io::Result<PathBuf> {
    Ok(data_dir()?.join("hooks.log"))
}

// ---- 用户设置 --------------------------------------------------------------

/// 加 `serde(default)` 是为了向前兼容：这是用户可手改、也会跨版本留存的文件，
/// 缺字段时应退回该字段的默认值，而不是整份解析失败 —— 否则每次新增设置项
/// 都会把用户已经调好的其他设置一起重置。
///
/// 不派生 `Copy`：`webhook_url` 是 `Option<String>`。相关方法因此都取 `&self`。
/// 圆点形状。
///
/// 默认是圆；此时 `permission` / `error` 会额外画成**圆角方**，那是给色盲用的
/// 形状区分（红绿是红绿色盲最难分的一对）。用户若把形状改成别的，这个区分就
/// 失效了 —— 设置面板里会说明这一点。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Shape {
    Circle,
    Rounded,
    /// 空心圆环。桌面上几乎没有别的东西长这样，最好认。
    Ring,
}

impl Default for Shape {
    fn default() -> Self {
        Shape::Circle
    }
}

/// 配色方案。
///
/// 给的是**预设而不是任意取色**：红 = 危险、黄 = 注意是有语义的，
/// 让用户随便改会把这层含义弄没。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Palette {
    /// 默认：绿 / 黄 / 红，颜色压暗一档以便在浅色桌面上站得住
    Default,
    /// 色盲友好：Okabe-Ito 的蓝 / 橙 / 朱红
    Colorblind,
    /// 高饱和：在花哨或深色的桌面上更容易看清
    Vivid,
}

impl Default for Palette {
    fn default() -> Self {
        Palette::Default
    }
}

/// 一条出站钩子：聚合变成 `when` 里的某个状态时，执行 `action`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hook {
    /// 哪些状态变化时触发。**空 = 任何变化都触发。**
    ///
    /// 值是状态名，含 `stale` / `offline` 这类派生状态 —— 和 `colors` 一样
    /// 单独用 `Signal::ALL_NAMES` 校验，因为 `Signal::parse` 不接受派生状态。
    #[serde(default)]
    pub when: Vec<String>,
    #[serde(flatten)]
    pub action: HookAction,
}

/// 钩子能做的事。
///
/// `Command` 会执行本地命令。信任模型和 git hooks / npm scripts 一样：配置文件
/// 是用户自己的，能改它就能执行任何东西。所以不额外加开关 —— 多一个开关只会
/// 给人一种「关掉就安全」的错觉，而配置文件本来就在用户手里。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum HookAction {
    Http { url: String },
    Command { command: String },
}

impl Hook {
    /// 这次聚合变化该不该触发它。
    pub fn matches(&self, aggregate: &str) -> bool {
        self.when.is_empty() || self.when.iter().any(|name| name == aggregate)
    }
}

impl HookAction {
    /// 目标填了吗。没填的钩子不触发、也不报错 —— 编辑时必然要经过
    /// 「刚添加、还没填目标」这个中间态，那时候报错就没法编辑了。
    pub fn is_configured(&self) -> bool {
        match self {
            HookAction::Http { url } => !url.trim().is_empty(),
            HookAction::Command { command } => !command.trim().is_empty(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// 圆点直径（px）
    pub size: u16,
    pub always_on_top: bool,
    /// 「有事才显形」：空闲 / 沉默 / 离线时把圆点压到几乎透明，
    /// 只有真正有活动或需要你注意时才完全显形。
    pub quiet: bool,
    /// 需要你注意（授权 / 关注 / 出错）时弹系统通知。
    pub notify: bool,
    /// **已废弃**，只为读得懂旧配置。启动时会搬成一条 `hooks` 条目
    /// （见 `migrate_legacy_webhook`），并且**永不写回** —— 出口只有一条路，
    /// 不让「单条 webhook」和「钩子列表」两种表达并存。
    #[serde(skip_serializing)]
    pub webhook_url: Option<String>,
    /// 出站钩子。聚合变化时按 `when` 过滤后依次触发。
    pub hooks: Vec<Hook>,
    /// 圆点形状
    pub shape: Shape,
    /// 给圆点套一圈中性细环。
    ///
    /// 解决的是「一个纯色点很容易和桌面上别的 UI 元素混在一起」—— 加了外壳
    /// 之后它读起来像「灰壳里的一颗灯」，也就是实体指示灯的样子。默认开。
    pub bezel: bool,
    /// 配色方案（用户没单独指定颜色时的默认值）
    pub palette: Palette,
    /// 每个状态的自定义颜色，`#rrggbb`。没写的用配色的默认值。
    ///
    /// 键是状态名，**含 `stale` / `offline` 这类派生状态** —— `Signal::parse`
    /// 刻意不接受派生状态（它们不能上报），但用户要能改它们显示成什么颜色。
    /// 所以这里存字符串键、单独校验。
    pub colors: BTreeMap<String, String>,
}

/// `#rrggbb` 或 `#rgb`。只做形状检查，不判断这个色值好不好看。
pub fn is_hex_color(value: &str) -> bool {
    let hex = value.strip_prefix('#').unwrap_or("");
    matches!(hex.len(), 3 | 6) && hex.chars().all(|c| c.is_ascii_hexdigit())
}

/// 只做最基本的判断（协议 + 有主机名），不试图猜目标是 ntfy 还是 Slack。
pub fn is_http_url(url: &str) -> bool {
    let url = url.trim();
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or("");
    !rest.is_empty() && !rest.starts_with(['.', '/'])
}

/// 一条钩子是否可用。校验和 `clamped()` 的清理都用它，免得两处规则漂移。
pub fn hook_problem(hook: &Hook) -> Option<String> {
    for name in &hook.when {
        if !crate::state::Signal::is_known_name(name) {
            return Some(format!(
                "未知的状态名 {name:?}；可用: {}",
                crate::state::Signal::ALL_NAMES.join(" ")
            ));
        }
    }
    // 空目标不算错（见 is_configured）；填了但填错才算
    match &hook.action {
        HookAction::Http { url } if !url.trim().is_empty() && !is_http_url(url) => {
            Some(format!("钩子地址不合法：{url:?}（需要以 http:// 或 https:// 开头）"))
        }
        _ => None,
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            size: DEFAULT_SIZE,
            always_on_top: true,
            quiet: true,
            notify: true,
            webhook_url: None,
            hooks: Vec::new(),
            shape: Shape::default(),
            bezel: true,
            palette: Palette::default(),
            colors: BTreeMap::new(),
        }
    }
}

impl Settings {
    /// 把越界值夹回允许范围。设置来自两处不可信输入：手改过的 config.json
    /// 和前端传来的值。
    pub fn clamped(mut self) -> Self {
        self.size = self.size.clamp(MIN_SIZE, MAX_SIZE);
        // 空白串等于「不转发」，归一成 None，免得每次转发都白试一次
        self.webhook_url = self
            .webhook_url
            .as_deref()
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .map(str::to_string);
        // 丢掉形状不对的颜色，并把十六进制统一成小写（便于前端比较）
        self.colors.retain(|_, color| is_hex_color(color));
        for color in self.colors.values_mut() {
            *color = color.to_ascii_lowercase();
        }
        // 同样地，写坏的钩子只丢掉那一条，不让整份设置失效
        self.hooks.retain(|hook| hook_problem(hook).is_none());
        self
    }

    /// 按可见圆点个数算出窗口尺寸。
    ///
    /// 只有一个会话时窗口仍是 `size + LAMP_MARGIN` 的方形（默认 22+20=42）——
    /// 多点只在真有多个会话时才占宽。否则一个常驻的宽透明窗口会白白吃掉桌面的
    /// 点击，而那正是这个工具最不该有的副作用。
    pub fn window_size(&self, dots: u16) -> (u16, u16) {
        let dots = dots.clamp(1, MAX_DOTS);
        let gap = self.size / 3;
        let width = dots * self.size + (dots - 1) * gap + LAMP_MARGIN;
        let height = self.size + LAMP_MARGIN;
        (width, height)
    }

    /// 展开面板的尺寸。行数取会话数，至少一行（避免出现一个空面板）。
    pub fn panel_size(&self, rows: u16) -> (u16, u16) {
        let rows = rows.max(1);
        (PANEL_WIDTH, PANEL_CHROME + rows * PANEL_ROW)
    }
}

fn settings_file() -> io::Result<PathBuf> {
    Ok(data_dir()?.join("config.json"))
}

/// 读设置。任何问题（文件不存在、内容坏掉）都退回默认值并写一行 stderr ——
/// 为了一个配置文件解析失败而拒绝启动，代价远大于收益。
pub fn load_settings() -> Settings {
    let Ok(path) = settings_file() else {
        return Settings::default();
    };
    let Ok(raw) = fs::read_to_string(&path) else {
        return Settings::default();
    };
    match serde_json::from_str::<Settings>(&raw) {
        Ok(settings) => migrate_legacy_webhook(settings).clamped(),
        Err(err) => {
            eprintln!("[signalhub] {} 无法解析（{err}），使用默认设置", path.display());
            Settings::default()
        }
    }
}

/// 把旧版的单条 `webhook_url` 搬成一条钩子。
///
/// 旧版本只支持「一条 webhook，只在需要注意时转发」，正好等价于一条
/// `when = [permission, attention, error]` 的 HTTP 钩子。不搬的话用户已经配好的
/// 转发会在升级后静默失效 —— 而那正是最难发现的一类回归。
fn migrate_legacy_webhook(mut settings: Settings) -> Settings {
    if let Some(url) = settings.webhook_url.take() {
        let url = url.trim().to_string();
        if !url.is_empty() {
            eprintln!("[signalhub] 把旧的 webhook 地址搬成了一条钩子：{url}");
            settings.hooks.push(Hook {
                when: vec!["permission".into(), "attention".into(), "error".into()],
                action: HookAction::Http { url },
            });
        }
    }
    settings
}

pub fn save_settings(settings: &Settings) -> io::Result<()> {
    let settings = settings.clone().clamped();
    let json = serde_json::to_string_pretty(&settings)
        .map_err(|err| io::Error::other(format!("序列化设置失败: {err}")))?;
    fs::write(settings_file()?, json)
}

/// 创建时就带上权限，避免出现「先 world-readable 再收紧」的窗口期。
#[cfg(unix)]
fn write_private(path: &Path, contents: &str) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(contents.as_bytes())
}

/// Windows 上文件默认继承用户目录的 ACL，不需要额外处理。
#[cfg(not(unix))]
fn write_private(path: &Path, contents: &str) -> io::Result<()> {
    fs::write(path, contents)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实踩到的坑：加了 `notify` 字段之后，旧版本写的 config.json 里没有它。
    /// 若不加 `serde(default)`，整份解析会失败、退回全默认 —— 用户已经调好的
    /// 圆点尺寸会被静默重置。这条测试锁住「缺字段只退回该字段的默认值」。
    #[test]
    fn settings_tolerate_missing_fields_from_older_versions() {
        let old = r#"{"size": 32, "always_on_top": false, "quiet": false}"#;
        let settings: Settings = serde_json::from_str(old).unwrap();

        assert_eq!(settings.size, 32, "用户调过的值必须保留");
        assert!(!settings.always_on_top);
        assert!(!settings.quiet);
        assert!(settings.notify, "缺的字段退回该字段的默认值");
        assert!(settings.colors.is_empty(), "缺的颜色表是空的，不是解析失败");
    }

    #[test]
    fn hex_colors_are_checked_by_shape_only() {
        for good in ["#fff", "#FFFFFF", "#16a34a", "#000000"] {
            assert!(is_hex_color(good), "{good} 应被接受");
        }
        for bad in ["16a34a", "#12345", "#gggggg", "", "#", "red"] {
            assert!(!is_hex_color(bad), "{bad:?} 应被拒绝");
        }
    }

    #[test]
    fn hook_urls_are_checked_at_the_boundary() {
        for good in [
            "https://ntfy.sh/my-topic",
            "http://127.0.0.1:8080/hook",
            "  https://hooks.slack.com/services/x  ",
        ] {
            assert!(is_http_url(good), "{good} 应被接受");
        }
        for bad in ["", "ntfy.sh/topic", "ftp://x/y", "https://", "https:///path", "  "] {
            assert!(!is_http_url(bad), "{bad:?} 应被拒绝");
        }
    }

    fn http_hook(when: &[&str], url: &str) -> Hook {
        Hook {
            when: when.iter().map(|s| s.to_string()).collect(),
            action: HookAction::Http { url: url.to_string() },
        }
    }

    #[test]
    fn hook_problems_are_named_precisely() {
        assert!(hook_problem(&http_hook(&["permission"], "https://x/y")).is_none());
        // 空 when 是合法的：任何变化都触发
        assert!(hook_problem(&http_hook(&[], "https://x/y")).is_none());

        let bad_state = http_hook(&["nonsense"], "https://x/y");
        assert!(hook_problem(&bad_state).unwrap().contains("未知的状态名"));

        let bad_url = http_hook(&["idle"], "not-a-url");
        assert!(hook_problem(&bad_url).unwrap().contains("不合法"));

        // 空目标是「还没配好」，不算错 —— 否则编辑器里刚点「添加」就报错
        assert!(hook_problem(&http_hook(&[], "")).is_none());
        let empty_command = Hook {
            when: vec![],
            action: HookAction::Command { command: "   ".into() },
        };
        assert!(hook_problem(&empty_command).is_none());
        assert!(!empty_command.action.is_configured(), "空命令不触发");
    }

    #[test]
    fn an_empty_hook_target_is_not_configured() {
        assert!(!http_hook(&[], "").action.is_configured());
        assert!(http_hook(&[], "https://x/y").action.is_configured());
        assert!(!HookAction::Command { command: "  ".into() }.is_configured());
        assert!(HookAction::Command { command: "say hi".into() }.is_configured());
    }

    /// 派生状态也能作为过滤条件 —— 用户可能只想知道「会话沉默了」
    #[test]
    fn hooks_may_filter_on_derived_states() {
        assert!(hook_problem(&http_hook(&["stale", "offline"], "https://x/y")).is_none());
    }

    /// 旧版本只有一条 webhook 地址，升级后不能让它静默失效
    #[test]
    fn a_legacy_webhook_becomes_a_hook() {
        let legacy = r##"{"webhook_url": "https://ntfy.sh/x"}"##;
        let settings = migrate_legacy_webhook(serde_json::from_str::<Settings>(legacy).unwrap());

        assert_eq!(settings.hooks.len(), 1, "应搬成一条钩子");
        assert_eq!(
            settings.hooks[0].when,
            vec!["permission", "attention", "error"],
            "触发条件要和旧行为一致（只在需要注意时转发）"
        );
        assert_eq!(
            settings.hooks[0].action,
            HookAction::Http { url: "https://ntfy.sh/x".into() }
        );

        // 空白的旧值不该凭空造出一条钩子
        let blank = migrate_legacy_webhook(
            serde_json::from_str::<Settings>(r##"{"webhook_url": "  "}"##).unwrap(),
        );
        assert!(blank.hooks.is_empty());
    }

    /// 废弃字段永不写回 —— 出口只有一条路
    #[test]
    fn the_legacy_webhook_field_is_never_written_back() {
        let mut settings = Settings::default();
        settings.webhook_url = Some("https://x/y".into());
        let json = serde_json::to_string(&settings).unwrap();
        assert!(!json.contains("webhook_url"), "不该写回废弃字段: {json}");
    }

    /// 手改配置文件时写坏的钩子只丢掉那一条
    #[test]
    fn clamping_drops_bad_hooks_but_keeps_the_rest() {
        let raw = r##"{"size": 24, "hooks": [
            {"when": ["error"], "type": "http", "url": "https://x/y"},
            {"when": ["nope"], "type": "http", "url": "https://x/y"},
            {"when": [], "type": "http", "url": "not-a-url"},
            {"when": [], "type": "command", "command": ""}
        ]}"##;
        let settings = serde_json::from_str::<Settings>(raw).unwrap().clamped();

        assert_eq!(settings.size, 24, "其余设置不受影响");
        assert_eq!(settings.hooks.len(), 2, "未知状态名和坏地址应被丢掉，空命令留下");
        assert_eq!(settings.hooks[0].when, vec!["error"]);
        assert!(!settings.hooks[1].action.is_configured(), "空命令那条还在，但不触发");
    }

    /// 手改的配置文件里颜色写坏了不该让整份设置失效 —— 只丢掉那一条
    #[test]
    fn clamping_drops_bad_colors_but_keeps_the_rest() {
        // 用 r##"..."## ：内容里有 "# ，单个 # 会提前结束原始字符串
        let raw = r##"{"size": 26, "colors": {"error": "#FF0000", "idle": "not-a-color", "nope": "#fff"}}"##;
        let settings = serde_json::from_str::<Settings>(raw).unwrap().clamped();

        assert_eq!(settings.size, 26, "其余设置不受影响");
        assert_eq!(
            settings.colors.get("error").map(String::as_str),
            Some("#ff0000"),
            "合法的颜色要保留，并统一成小写"
        );
        assert!(!settings.colors.contains_key("idle"), "坏色值应被丢掉");
        // 键名合法性由 set_settings 校验（会报错），clamped 只保证色值可用
        assert!(settings.colors.contains_key("nope"));
    }

    #[test]
    fn an_empty_object_yields_all_defaults() {
        assert_eq!(
            serde_json::from_str::<Settings>("{}").unwrap(),
            Settings::default()
        );
    }

    #[test]
    fn clamped_pulls_out_of_range_sizes_back() {
        let wild = Settings { size: 9999, ..Settings::default() };
        assert_eq!(wild.clamped().size, MAX_SIZE);
        let tiny = Settings { size: 1, ..Settings::default() };
        assert_eq!(tiny.clamped().size, MIN_SIZE);
    }

    #[test]
    fn window_size_only_grows_when_there_are_multiple_dots() {
        let settings = Settings::default();
        let size = settings.size;

        // 一个圆点：方形，和加多点之前一样
        let (w1, h1) = settings.window_size(1);
        assert_eq!(w1, size + LAMP_MARGIN);
        assert_eq!(h1, size + LAMP_MARGIN);

        // 两个圆点起才开始变宽，高度不变
        let (w2, _) = settings.window_size(2);
        assert!(w2 > w1, "多一个圆点就该多占一份宽度");
        let (_, h3) = settings.window_size(3);
        assert_eq!(h3, h1, "高度只和圆点直径有关");

        // 超出上限按上限算，不会无限变宽
        assert_eq!(settings.window_size(99), settings.window_size(MAX_DOTS));
    }

    #[test]
    fn panel_grows_one_row_per_session_and_never_collapses_to_nothing() {
        let settings = Settings::default();
        let (w, h1) = settings.panel_size(1);
        assert_eq!(w, PANEL_WIDTH, "面板宽度固定");
        assert_eq!(h1, PANEL_CHROME + PANEL_ROW);

        let (_, h3) = settings.panel_size(3);
        assert_eq!(h3, PANEL_CHROME + 3 * PANEL_ROW, "每个会话一行");

        // 没有会话时也要留一行，否则展开会得到一个空壳
        assert_eq!(settings.panel_size(0), settings.panel_size(1));
    }
}
