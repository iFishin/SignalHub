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

/// 灯球上最多同时显示几块牌。超出的截断 —— 牌按优先级排，所以留下的是最紧急的。
pub const MAX_SIGNS: usize = 4;

/// 牌的宽度。
///
/// 由 `size` 派生，不另加一个设置 —— `size` 现在表示整块牌的尺度，「大小」
/// 一个旋钮就够。文字长度不定，所以宽度**固定**、超出用省略号截断：让宽度
/// 跟着文字走会让窗口每次消息变化都跳一下。
pub fn sign_width(size: u16) -> u16 {
    size * 7
}

/// 一块牌的高度。有消息就多一行 —— 那块牌会变高，正好是个「有东西可读」的提示。
///
/// **CSS 里的 `.sign` / `.sign[data-has-message="true"]` 必须与这两个值严格对齐**，
/// 否则窗口会裁掉最后一块牌、或者底下留一条缝。
pub fn sign_height(size: u16, has_message: bool) -> u16 {
    if has_message {
        size * 5 / 2
    } else {
        size * 3 / 2
    }
}

/// 折叠形态的圆点个数上限。折叠成一个单纯的灯时，一个组一个点。
///
/// 刻意和 `MAX_SIGNS` 取同一个值：折叠只是换了个外形，不该顺带少显示几个组 ——
/// 那样会让人以为一折叠就把状态弄丢了。
pub const MAX_DOTS: usize = MAX_SIGNS;

/// 折叠形态的窗口尺寸：N 个点排成一行。
///
/// 这套几何比展开态小得多 —— 折叠的意义就是让出地方。一个组一个点，
/// 所以点数和组数一样；一个组都没有时也要有一个点（表示 offline）。
///
/// `flags` 是**额外的角标数**（暂停 / 穿透那一枚，见 `lib.rs` 的 `ui_flag_count`）。
/// 角标也占一个点的位置，所以点数要加上它 —— 忘了加的话那一枚会把窗口撑出界，
/// 而折叠态的意义正是「让出地方」，撑出去就白让了。
///
/// ⚠️ 点径、点距、留白和 CSS 的 `.lamp-dot` / `.lamp-dot-flag` / `.lamp-root` 是
/// 两处真值（见本文件头），改一处必须同步另一处。
pub fn collapsed_window_size(size: u16, dots: usize, flags: usize) -> (u16, u16) {
    // 分别封顶再相加，而不是封顶它们的和：封住和的话「多枚角标」会被当成
    // 「少几个组」，于是牌少了一块 —— 那正是截断要避免的那种谎。
    let dots = dots.clamp(0, MAX_DOTS);
    // 暂停 / 穿透共用同一枚角标（同时开着时穿透优先），所以最多 1
    let flags = flags.min(1);
    let dots = (dots + flags).max(1) as u16;
    let gap = size / 3;
    (dots * size + (dots - 1) * gap + LAMP_MARGIN, size + LAMP_MARGIN)
}

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

/// 日志时间戳。用 UTC 并在值上标 `Z`：不引入时区数据依赖，且不会含糊。
pub fn utc_hms() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
        % 86_400;
    format!("{:02}:{:02}:{:02}Z", secs / 3600, (secs % 3600) / 60, secs % 60)
}

/// 往 hook 日志追加一行。
///
/// **入站（智能体调 CLI）和出站（SignalHub 发钩子）共用这一个出口。** 两处各写
/// 一遍轮转逻辑的话，迟早有一处忘了加，那个文件就会无界增长。
///
/// 单代轮转：超过 512KB 就改名为 `.log.1`（覆盖上一次的）。日志是排查用的，不是
/// 档案 —— 保留一代够回答「刚才到底发生了什么」。
///
/// app 和 CLI 是两个进程，可能同时写。`O_APPEND` 下单行的写入是原子的，所以不会
/// 互相截断；但**行与行的先后顺序不保证**。
pub fn append_hook_log(line: &str) {
    let Ok(path) = hook_log_path() else {
        return;
    };

    const MAX_BYTES: u64 = 512 * 1024;
    if fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0) > MAX_BYTES {
        let _ = fs::rename(&path, path.with_extension("log.1"));
    }

    use std::io::Write;
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = file.write_all(line.as_bytes());
    }
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
    /// 限定只看哪几个组的跃迁。**空 = 看全局聚合。**
    ///
    /// 两种语义是有意的，不是二选一：
    ///
    /// - **空**：关心「整体现在出事了没有」。一组在等授权、另一组在跑，整体是
    ///   `permission` —— 这时不该因为「某个组变成 error 了」就发。
    /// - **非空**：关心「这一组怎么了」。于是每个组**各自的跃迁**都算数，不必等
    ///   它成为全局最高的那个。想「ci 组失败发到 A、deploy 组失败发到 B」就得用它。
    ///
    /// 之所以不去掉第一种：现有的钩子都是按全局聚合配的，改掉会让用户已经配好的
    /// 自动化静默变行为 —— 那是这类改动里最不该发生的事。
    #[serde(default)]
    pub groups: Vec<String>,
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
    /// 这次聚合变化该不该触发它（只看 `when` 那部分）。
    pub fn matches(&self, aggregate: &str) -> bool {
        self.when.is_empty() || self.when.iter().any(|name| name == aggregate)
    }

    /// 这次该不该触发它。
    ///
    /// `aggregate` 是全局聚合，`transitioned` 是全局聚合**本次有没有跃迁**，
    /// `changes` 是本次发生跃迁的组（组名 + 新状态）。两条路径的区别见 `groups`
    /// 字段的说明。
    pub fn fires(
        &self,
        aggregate: &str,
        transitioned: bool,
        changes: &[(String, crate::state::Signal)],
    ) -> bool {
        if self.groups.is_empty() {
            // 不限定组：只在**全局聚合跃迁**时触发。这个语义是从一开始就有的，
            // 去掉它会让用户已经配好的钩子变成「任何组一变就发」—— 静默改行为
            // 是这类改动里最不该发生的事。
            return transitioned && self.matches(aggregate);
        }
        changes.iter().any(|(name, state)| {
            self.groups.iter().any(|group| group == name) && self.matches(state.name())
        })
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

    /// 目标本身（网址或命令）。
    ///
    /// 日志和界面都**按它去认一条钩子**，而不是按下标 —— 钩子列表可以增删排序，
    /// 下标会错位，于是会显示成「另一条钩子的结果」，比不显示更糟。
    pub fn target(&self) -> String {
        match self {
            HookAction::Http { url } => url.clone(),
            HookAction::Command { command } => command.clone(),
        }
    }
}

/// 展开的牌上显示哪几段内容。四段都可以单独关掉 —— 有人只要一眼颜色，
/// 有人要看清「卡在哪一步」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SignFields {
    /// 组名
    pub name: bool,
    /// 状态名（如「需授权」）
    pub state: bool,
    /// 已持续多久
    pub age: bool,
    /// 消息 —— 关掉之后牌永远是单行高
    pub message: bool,
}

impl Default for SignFields {
    fn default() -> Self {
        // 默认全开：牌的信息量就是它存在的理由
        Self { name: true, state: true, age: true, message: true }
    }
}

/// 某一个状态的个性化外观。三样都可以单独覆盖，**没写的用全局默认**。
///
/// 为什么是「可选的字段」而不是「填满的表格」：默认值会随版本演进（新增状态、
/// 调整默认节奏、改配色），如果用户配置文件里存着一份填满的表，那些改进就永远
/// 到不了他那儿。空着 = 跟随，是唯一能让默认值继续演进的表达。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StateStyle {
    /// 颜色 `#rrggbb`。没写 = 用配色方案里那个状态的默认色。
    pub color: Option<String>,
    /// 灯珠形状。没写 = 用全局形状（全局是「圆」时 permission / error 仍会
    /// 画成圆角方，那是给色盲用的形状区分）。
    pub shape: Option<Shape>,
    /// 闪烁周期（毫秒）。**0 = 常亮**。没写 = 按全局节奏倍率算默认周期。
    pub blink_ms: Option<u16>,
}

impl StateStyle {
    /// 三样都没写。这种条目没有任何作用，`clamped()` 会把它们丢掉 ——
    /// 留着会让「预设是否还生效」的判断无谓地失败。
    pub fn is_empty(&self) -> bool {
        self.color.is_none() && self.shape.is_none() && self.blink_ms.is_none()
    }
}

/// 勿扰时段。这段时间里**不弹通知、不出声**，但灯照常显示 —— 勿扰是「别吵我」，
/// 不是「别告诉我」。
///
/// **只有小时，没有分钟**：分钟级精度只会让配置变麻烦，而「晚上十点到早上八点
/// 别吵我」这种需求本来就不需要分钟。
///
/// 起止都按**用户本地时间**。本地时区只有前端知道（Rust 这边没有时区数据，
/// `SystemTime` 只给 UTC），所以前端每分钟把当前小时推给 Rust，**由 Rust 判断在
/// 不在窗口内** —— 策略留在 Rust 是为了能单测，前端只提供「现在是几点」这个事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct QuietHours {
    pub enabled: bool,
    /// 起始小时（0–23）
    pub start: u8,
    /// 结束小时（0–23），**不含**这一小时
    pub end: u8,
}

impl Default for QuietHours {
    fn default() -> Self {
        // 22:00–08:00。enabled 默认 false，但这个窗口是最常见的那个 ——
        // 用户一打开开关就是个合理的值，不用先调时间。
        Self { enabled: false, start: 22, end: 8 }
    }
}

impl QuietHours {
    /// 这个小时算不算「勿扰」。
    ///
    /// 跨午夜（`start > end`）是常态而不是特例，所以必须分两段判断：
    /// 22–8 表示 `[22, 24) ∪ [0, 8)`。
    pub fn covers(&self, hour: u8) -> bool {
        if !self.enabled {
            return false;
        }
        // start == end 读作「空窗口」而不是「全天」：一个手滑把两个值调成一样，
        // 后果应该是「没生效」，而不是「从此再也不提醒」。
        if self.start == self.end {
            return false;
        }
        if self.start < self.end {
            hour >= self.start && hour < self.end
        } else {
            hour >= self.start || hour < self.end
        }
    }
}

/// 界面语言的**偏好**。注意和 `lang::Lang` 的区别：那个是**解析后**的语言，
/// 只有中文和英文；这里的 `Auto` 要由前端解析成具体语言 —— Rust 不知道系统语言。
///
/// 定义在这个文件而不是 `lang.rs`：`Settings` 要被 CLI 通过 `#[path]` 共用，
/// 而 `lang.rs` 是 app 侧的模块。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Locale {
    /// 跟随系统
    #[default]
    Auto,
    Zh,
    En,
}

/// 某一组的显示偏好。
///
/// **只影响灯上怎么显示**，不影响聚合、通知和钩子 —— 组名是上报方给的标识
/// （`build-1`、`ci`），这里改的是「怎么称呼它、排在第几位」，不是「它是什么」。
/// 隐藏 / 静音的匹配也仍然按**原始组名**，不按显示名。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GroupMeta {
    /// 显示名。没写就用上报方给的原始组名。
    pub name: Option<String>,
    /// 置顶权重。`Some(n)` = 置顶，越小越靠前；`None` = 不置顶，按紧急程度排。
    ///
    /// 用 `Option<i16>` 而不是 `pinned: bool`：置顶的组之间也要有确定的顺序，
    /// 而「先置顶的排前面」正是用户按下开关的顺序 —— 那就不必再做一个拖拽界面。
    pub order: Option<i16>,
}

impl GroupMeta {
    /// 两样都没写。这种条目没有任何作用，`clamped()` 会把它们丢掉。
    pub fn is_empty(&self) -> bool {
        self.name.is_none() && self.order.is_none()
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
    /// 配色方案（用户没单独指定颜色时的默认值）
    pub palette: Palette,
    /// 界面语言偏好。`Auto` 由**前端**解析成具体语言 —— Rust 不知道系统语言。
    pub locale: Locale,
    /// 逐组的显示偏好（显示名 / 置顶）。键是**上报方给的原始组名**。
    ///
    /// 与 `hidden_groups` / `muted_groups` 是两回事：那两个管「显不显示 / 要不要
    /// 吵我」，这个管「怎么称呼它、排第几位」。
    pub group_meta: BTreeMap<String, GroupMeta>,
    /// 逐个状态的外观覆盖（颜色 / 形状 / 节奏）。没写的项跟随全局。
    ///
    /// 键是状态名，**含 `stale` / `offline` 这类派生状态** —— `Signal::parse`
    /// 刻意不接受派生状态（它们不能上报），但用户要能改它们长什么样。
    /// 所以这里存字符串键、单独校验。
    pub styles: BTreeMap<String, StateStyle>,
    /// **已废弃**，只为读得懂旧配置。启动时会搬进 `styles` 的 `color` 字段
    /// （见 `migrate_legacy_colors`），并且**永不写回** —— 一个状态的外观只有
    /// 一处表达，不让「单独的颜色表」和「外观表」并存。
    #[serde(skip_serializing)]
    pub colors: BTreeMap<String, String>,

    /// 折叠成一个单纯的灯（只剩灯，没有牌面）。**默认 false：展开成牌。**
    ///
    /// 存进设置而不是当作瞬态开关：用户折叠是有意的选择，重启后该保持。
    /// 托盘里有「展开 / 折叠外形」可以随时切回来，不会卡在折叠态。
    pub collapsed: bool,
    /// 展开的牌上显示哪几段内容
    pub show: SignFields,
    /// 牌面背景的不透明度（0–100）。调低能看见桌面。
    pub sign_opacity: u8,
    /// 用户从灯上右键「隐藏」掉的组。
    ///
    /// **只影响灯上显示哪几块牌**，不影响聚合、通知和钩子 —— 聚合是全局概念，
    /// 把一块牌藏起来不等于当这一组不存在。想彻底忽略一组应该让它的上报方
    /// 别再报，而不是指望这里。
    pub hidden_groups: Vec<String>,
    /// 闪烁节奏倍率（50–200，100 = 默认）。闪得太快会分心，太慢又看不出在动。
    pub blink_speed: u16,
    /// 需要你注意时，系统通知是否带声音。
    ///
    /// 声音挂在系统通知上（`NotificationBuilder::sound`），所以关掉通知就没有
    /// 声音 —— 这是刻意的：不为一个提示音再加一条独立的音频通路和它的跨平台麻烦。
    pub sound: bool,
    /// 勿扰时段：这段时间里不弹通知、不出声，灯照常显示。
    pub quiet_hours: QuietHours,
    /// 被静音的组：**只影响通知和提示音**。
    ///
    /// 与 `hidden_groups` 是两件事，别混：那个管「灯上显不显示」，这个管
    /// 「要不要吵我」。一个组可以只隐藏不静音（看得见但不打扰），也可以只静音
    /// 不隐藏（灯上照常红着，只是不弹通知）。
    pub muted_groups: Vec<String>,
    /// 「需要你注意」的状态要**持续**这么多秒才弹通知。0 = 立刻弹。
    ///
    /// 存在的理由：智能体经常在一秒内进出 permission（先问再自己答，或者
    /// 弹一下就被自动批准）。立刻弹的话，用户被叫过去看到的是一个已经解决的
    /// 问题。等一会儿能过滤掉这类「一闪而过」。
    pub alert_after_secs: u16,
    /// 托盘图标是否跟随当前聚合状态变色。关掉就回到单色模板图（跟着菜单栏明暗反色）。
    pub tray_follows_state: bool,
    /// 上报没带 `group` 时，怎么给它兜一个。
    ///
    /// **默认 `None`，也就是不分组**，这条比看着重要：一旦默认按目录分组，
    /// 所有现有用户的灯会从一块牌变成若干块、窗口尺寸跟着变，而他们没配过的
    /// 每一组的显示名都是原始路径 —— 属于无声改变既有行为。
    pub group_by: GroupBy,
}

/// 兜底分组的依据。
///
/// 只有两个取值，而且**默认那个必须是不分组**（理由见 `Settings::group_by`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum GroupBy {
    /// 不分组：所有不带 `group` 的上报归 `default`，和没有这个开关时完全一样。
    #[default]
    None,
    /// 用上报里的 `cwd` 当组名。一个工作目录一块牌。
    Cwd,
}

/// 牌面背景的默认不透明度。够看清内容，又能透出一点桌面。
pub const DEFAULT_SIGN_OPACITY: u8 = 86;

/// 闪烁节奏的可调范围。上下界都留得住意义：再慢看不出在动，再快只剩烦。
pub const MIN_BLINK_SPEED: u16 = 50;
pub const MAX_BLINK_SPEED: u16 = 200;

/// 单个状态的闪烁周期范围（毫秒）。**0 单独表示常亮**，不落在这个区间里。
///
/// 下限取 120ms：比这更快就只剩一片模糊，读不出「在动」。上限取 4 秒：
/// 再慢用户会以为它卡住了。
pub const MIN_BLINK_MS: u16 = 120;
pub const MAX_BLINK_MS: u16 = 4000;

/// 「持续多久才弹通知」的上限（秒）。一小时已经远超任何有意义的等待。
pub const MAX_ALERT_DELAY_SECS: u16 = 3600;

/// 默认的「持续多久才弹通知」。5 秒足够滤掉「问一下又自己答了」这类一闪而过，
/// 又不至于让真正的等待等太久。
pub const DEFAULT_ALERT_DELAY_SECS: u16 = 5;

/// 组名列表的规整：去空白、去空串、去重、排序。
///
/// 隐藏和静音两个列表都要它。脏数据（`" ci"` 和 `"ci"`）会让「这一组藏了吗」
/// 时灵时不灵，而那是最难查的一类问题。
fn normalize_groups(names: Vec<String>) -> Vec<String> {
    let mut names: Vec<String> = names
        .into_iter()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// 由工作目录得出的组名：**完整路径**，去掉末尾的分隔符。
///
/// 用完整路径而不是最后一段目录名：同名目录（`~/a/api` 和 `~/b/api`）太常见，
/// 撞名会把两个项目的会话混进同一块牌 —— 那正是这个开关最不该出的错。代价是
/// 组名很长，所以灯上会自动截断（见 `settings.css` 的 `.group-name`），
/// 设置里也能逐组改名。
fn group_from_cwd(cwd: &str) -> Option<String> {
    let name = cwd.trim().trim_end_matches(['/', '\\']);
    // 根目录 `/` 去掉分隔符就成了空串 —— 那不是个能用的组名，退回 `default`。
    // 一个会话的工作目录就是根目录，本来也少见。
    (!name.is_empty()).then(|| name.to_string())
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
            palette: Palette::default(),
            locale: Locale::default(),
            group_meta: BTreeMap::new(),
            styles: BTreeMap::new(),
            colors: BTreeMap::new(),
            collapsed: false,
            show: SignFields::default(),
            sign_opacity: DEFAULT_SIGN_OPACITY,
            blink_speed: 100,
            hidden_groups: Vec::new(),
            sound: true,
            quiet_hours: QuietHours::default(),
            muted_groups: Vec::new(),
            alert_after_secs: DEFAULT_ALERT_DELAY_SECS,
            tray_follows_state: true,
            group_by: GroupBy::default(),
        }
    }
}

impl Settings {
    /// 把越界值夹回允许范围。设置来自两处不可信输入：手改过的 config.json
    /// 和前端传来的值。
    pub fn clamped(mut self) -> Self {
        self.size = self.size.clamp(MIN_SIZE, MAX_SIZE);
        self.sign_opacity = self.sign_opacity.min(100);
        self.blink_speed = self.blink_speed.clamp(MIN_BLINK_SPEED, MAX_BLINK_SPEED);
        self.alert_after_secs = self.alert_after_secs.min(MAX_ALERT_DELAY_SECS);
        self.quiet_hours.start = self.quiet_hours.start.min(23);
        self.quiet_hours.end = self.quiet_hours.end.min(23);
        // 组名去空白、去空串、去重并排序。这两个列表都会被反复比较（每块牌都要问
        // 一次「我该不该显示 / 我该不该吵」），脏数据留在里面就每次都要重新规整。
        self.hidden_groups = normalize_groups(self.hidden_groups);
        self.muted_groups = normalize_groups(self.muted_groups);
        // 空白串等于「不转发」，归一成 None，免得每次转发都白试一次
        self.webhook_url = self
            .webhook_url
            .as_deref()
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .map(str::to_string);
        // 丢掉形状不对的颜色，并把十六进制统一成小写（便于前端比较）。
        // **不认识的键名刻意留着**：clamped 是手改配置文件的兜底，静默删掉用户
        // 写的东西是数据丢失；键名合法性由 set_settings 校验（会明确报错），
        // 这里只保证值本身可用。
        for style in self.styles.values_mut() {
            if let Some(color) = style.color.take() {
                let color = color.trim().to_ascii_lowercase();
                if is_hex_color(&color) {
                    style.color = Some(color);
                }
            }
            // 0 是「常亮」，单独放行；其余夹回有意义的区间
            if let Some(ms) = style.blink_ms {
                style.blink_ms = Some(if ms == 0 { 0 } else { ms.clamp(MIN_BLINK_MS, MAX_BLINK_MS) });
            }
        }
        // 三样都没写的条目没有任何作用，留着只会让「预设是否还生效」的判断无谓失败
        self.styles.retain(|_, style| !style.is_empty());
        // 显示名去空白、空串归一成 None；两样都没写的条目同样丢掉
        for meta in self.group_meta.values_mut() {
            if let Some(name) = meta.name.take() {
                let name = name.trim().to_string();
                meta.name = (!name.is_empty()).then_some(name);
            }
        }
        self.group_meta.retain(|_, meta| !meta.is_empty());
        // 同样地，写坏的钩子只丢掉那一条，不让整份设置失效
        self.hooks.retain(|hook| hook_problem(hook).is_none());
        // 钩子里的组名也要规整，理由同 hidden_groups
        for hook in &mut self.hooks {
            hook.groups = normalize_groups(std::mem::take(&mut hook.groups));
        }
        self
    }

    /// 这个组该不该出现在灯上。Rust 算窗口高度和前端渲染都必须问同一句话 ——
    /// 两边算出不同的组，窗口就会裁掉最后一块牌，或者底下空一条。
    pub fn shows_group(&self, name: &str) -> bool {
        !self.hidden_groups.iter().any(|hidden| hidden == name)
    }

    /// 这次上报最终归哪个组。
    ///
    /// 上报**自己带的 `group` 优先**，`group_by` 只是兜底，不是覆盖。这条顺序
    /// 不能反：反了就等于「开了按目录分组之后，手写 `group` 的用法全失效」——
    /// 而多分组最早、也最稳的用法就是手写 `group`（见 docs/api.md §2.7）。
    /// 想只给某一路单独分组的人也不必关掉全局开关。
    pub fn resolve_group(&self, group: Option<&str>, cwd: Option<&str>) -> Option<String> {
        let explicit = group.map(str::trim).filter(|name| !name.is_empty());
        if let Some(name) = explicit {
            return Some(name.to_string());
        }
        match self.group_by {
            GroupBy::None => None,
            GroupBy::Cwd => cwd.and_then(group_from_cwd),
        }
    }

    /// 这一组在灯上叫什么。没设显示名就用上报方给的原始组名。
    ///
    /// 注意**只是显示**：隐藏 / 静音的匹配、`/v1/state` 里的组名都仍然是原始名。
    pub fn group_label<'a>(&'a self, name: &'a str) -> &'a str {
        self.group_meta
            .get(name)
            .and_then(|meta| meta.name.as_deref())
            .unwrap_or(name)
    }

    /// 置顶权重。`None` = 不置顶，按紧急程度排。
    pub fn group_order(&self, name: &str) -> Option<i16> {
        self.group_meta.get(name).and_then(|meta| meta.order)
    }

    /// 窗口尺寸。`signs` 是可见牌的高度列表（Rust 侧算好，见 `sign_height`）。
    ///
    /// 一块牌时窗口就是「牌 + 留白」；多块竖着叠，高度逐块累加。
    pub fn window_size(&self, signs: &[u16]) -> (u16, u16) {
        let gap = self.size / 2;
        let count = signs.len().clamp(1, MAX_SIGNS) as u16;
        let stacked: u16 = signs.iter().take(count as usize).sum();
        let gaps = gap * count.saturating_sub(1);
        (sign_width(self.size) + LAMP_MARGIN, stacked + gaps + LAMP_MARGIN)
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
        Ok(settings) => migrate_legacy_colors(migrate_legacy_webhook(settings)).clamped(),
        Err(err) => {
            eprintln!("[signalhub] {} 无法解析（{err}），使用默认设置", path.display());
            Settings::default()
        }
    }
}

/// 把旧版的逐状态颜色表搬进 `styles`。
///
/// 旧版本用一张扁平的 `colors` 表存颜色；现在一个状态的外观是「颜色 + 形状 +
/// 节奏」三样，所以改成了 `styles`。不搬的话，用户调过的颜色会在升级后静默
/// 变回默认 —— 而那正是最难发现的一类回归。
///
/// 如果某个状态在 `styles` 里已经有颜色（用户同时写了两处），以 `styles` 为准。
fn migrate_legacy_colors(mut settings: Settings) -> Settings {
    let legacy = std::mem::take(&mut settings.colors);
    if legacy.is_empty() {
        return settings;
    }

    let mut moved = 0usize;
    for (state, color) in legacy {
        let style = settings.styles.entry(state).or_default();
        if style.color.is_none() {
            style.color = Some(color);
            moved += 1;
        }
    }
    if moved > 0 {
        eprintln!("[signalhub] 把旧的逐状态颜色表搬进了 styles（{moved} 项）");
    }
    settings
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
                // 旧的单条 webhook 只看全局聚合，这正是 groups 为空的语义
                groups: Vec::new(),
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
            groups: Vec::new(),
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
            groups: Vec::new(),
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
    fn clamping_drops_bad_styles_but_keeps_the_rest() {
        // 用 r##"..."## ：内容里有 "# ，单个 # 会提前结束原始字符串
        let raw = r##"{"size": 26, "styles": {
            "error": {"color": "#FF0000"},
            "idle":  {"color": "not-a-color", "blink_ms": 900},
            "nope":  {"color": "#fff"}
        }}"##;
        let settings = serde_json::from_str::<Settings>(raw).unwrap().clamped();

        assert_eq!(settings.size, 26, "其余设置不受影响");
        assert_eq!(
            settings.styles["error"].color.as_deref(),
            Some("#ff0000"),
            "合法的颜色要保留，并统一成小写"
        );
        assert_eq!(
            settings.styles["idle"].blink_ms,
            Some(900),
            "颜色写坏了只丢颜色，同一状态上别的覆盖要留着"
        );
        assert_eq!(settings.styles["idle"].color, None, "坏色值应被丢掉");
        // 键名合法性由 set_settings 校验（会报错），clamped 只保证值可用
        assert!(settings.styles.contains_key("nope"));
    }

    /// 旧版本用一张扁平的 `colors` 表存颜色，升级后不能让它静默失效
    #[test]
    fn a_legacy_color_table_moves_into_styles() {
        let raw = r##"{"colors": {"error": "#ff0000", "idle": "#00ff00"}}"##;
        let settings =
            migrate_legacy_colors(serde_json::from_str::<Settings>(raw).unwrap()).clamped();

        assert_eq!(settings.styles["error"].color.as_deref(), Some("#ff0000"));
        assert_eq!(settings.styles["idle"].color.as_deref(), Some("#00ff00"));
        assert!(settings.colors.is_empty(), "搬完就该空了");

        // 两处都写了同一个状态时，新的 styles 说了算
        let both = r##"{"colors": {"error": "#111111"}, "styles": {"error": {"color": "#222222"}}}"##;
        let settings =
            migrate_legacy_colors(serde_json::from_str::<Settings>(both).unwrap()).clamped();
        assert_eq!(settings.styles["error"].color.as_deref(), Some("#222222"));
    }

    /// 废弃字段永不写回 —— 一个状态的外观只有一处表达
    #[test]
    fn the_legacy_color_field_is_never_written_back() {
        let mut settings = Settings::default();
        settings.colors.insert("error".into(), "#ff0000".into());
        settings.styles.insert("idle".into(), StateStyle::default());
        let json = serde_json::to_string(&settings).unwrap();
        assert!(!json.contains("\"colors\""), "不该写回废弃字段: {json}");
    }

    /// 三样都没写的条目没有作用，留着会让「预设是否还生效」的判断无谓地失败
    #[test]
    fn empty_style_entries_are_dropped() {
        let raw = r##"{"styles": {"idle": {}, "error": {"shape": "ring"}}}"##;
        let settings = serde_json::from_str::<Settings>(raw).unwrap().clamped();

        assert!(!settings.styles.contains_key("idle"), "空条目应被丢掉");
        assert_eq!(settings.styles["error"].shape, Some(Shape::Ring));
    }

    /// 0 是「常亮」，是合法值；其余夹回有意义的区间 —— 太快只剩模糊，太慢像卡住
    #[test]
    fn per_state_blink_allows_zero_but_clamps_everything_else() {
        let raw = r##"{"styles": {
            "idle": {"blink_ms": 0},
            "thinking": {"blink_ms": 1},
            "working": {"blink_ms": 60000},
            "done": {"blink_ms": 700}
        }}"##;
        let settings = serde_json::from_str::<Settings>(raw).unwrap().clamped();

        assert_eq!(settings.styles["idle"].blink_ms, Some(0), "0 表示常亮，要放行");
        assert_eq!(settings.styles["thinking"].blink_ms, Some(MIN_BLINK_MS));
        assert_eq!(settings.styles["working"].blink_ms, Some(MAX_BLINK_MS));
        assert_eq!(settings.styles["done"].blink_ms, Some(700), "区间内原样保留");
    }

    /// 勿扰时段跨午夜是常态，不是特例
    #[test]
    fn quiet_hours_cover_the_midnight_wrap() {
        // 22:00 – 08:00
        let night = QuietHours { enabled: true, start: 22, end: 8 };
        for hour in [22, 23, 0, 1, 7] {
            assert!(night.covers(hour), "{hour} 点应在勿扰时段内");
        }
        for hour in [8, 12, 21] {
            assert!(!night.covers(hour), "{hour} 点不该在勿扰时段内");
        }

        // 不跨午夜的窗口（比如午休 12:00–14:00）
        let noon = QuietHours { enabled: true, start: 12, end: 14 };
        assert!(noon.covers(12));
        assert!(noon.covers(13));
        assert!(!noon.covers(14), "结束那一小时不含在内");
        assert!(!noon.covers(11));
    }

    /// 关掉开关就完全不管时间；起止相同读作「空窗口」而不是「全天」
    #[test]
    fn quiet_hours_degrade_safely() {
        let off = QuietHours { enabled: false, start: 0, end: 23 };
        for hour in 0..24 {
            assert!(!off.covers(hour), "没启用就不该有任何小时被算进去");
        }

        let equal = QuietHours { enabled: true, start: 9, end: 9 };
        for hour in 0..24 {
            assert!(
                !equal.covers(hour),
                "起止相同应读作空窗口 —— 手滑调成一样不该变成全天静音"
            );
        }

        assert!(!QuietHours::default().enabled, "默认不启用");
    }

    /// 手改的配置里小时可以是 200，夹回 0–23
    #[test]
    fn quiet_hours_are_clamped_to_real_hours() {
        let raw = r#"{"quiet_hours": {"enabled": true, "start": 200, "end": 250}}"#;
        let settings = serde_json::from_str::<Settings>(raw).unwrap().clamped();
        assert_eq!(settings.quiet_hours.start, 23);
        assert_eq!(settings.quiet_hours.end, 23);
    }

    /// 静音列表和隐藏列表用同一套规整规则 —— 两处各写一遍迟早分叉
    #[test]
    fn muted_groups_are_normalized_like_hidden_ones() {
        let raw = r#"{"hidden_groups": [" ci ", "ci", ""], "muted_groups": ["work", " work ", "zeta", "alpha"]}"#;
        let settings = serde_json::from_str::<Settings>(raw).unwrap().clamped();

        assert_eq!(settings.hidden_groups, vec!["ci"]);
        assert_eq!(settings.muted_groups, vec!["alpha", "work", "zeta"]);
    }

    /// 阈值告警的秒数也要夹紧：手改成一个巨大的值等于永久不提醒
    #[test]
    fn the_alert_delay_is_clamped() {
        let raw = r#"{"alert_after_secs": 60000}"#;
        let settings = serde_json::from_str::<Settings>(raw).unwrap().clamped();
        assert_eq!(settings.alert_after_secs, MAX_ALERT_DELAY_SECS);
    }

    // ---- 逐组显示偏好 ---------------------------------------------------------

    #[test]
    fn group_names_are_trimmed_and_empty_entries_are_dropped() {
        let raw = r#"{"group_meta": {
            "ci":   {"name": "  构建  ", "order": 0},
            "work": {"name": "   "},
            "docs": {},
            "x":    {"order": 2}
        }}"#;
        let settings = serde_json::from_str::<Settings>(raw).unwrap().clamped();

        assert_eq!(settings.group_label("ci"), "构建", "两头空白要去掉");
        assert!(!settings.group_meta.contains_key("work"), "全空白的名字等于没设");
        assert!(!settings.group_meta.contains_key("docs"), "空条目没有作用，丢掉");
        assert_eq!(settings.group_label("x"), "x", "只有置顶、没设名字的条目要留着");
        assert_eq!(settings.group_order("x"), Some(2));
    }

    /// 显示名只是显示：没设就回落到上报方给的原始组名
    #[test]
    fn the_group_label_falls_back_to_the_raw_id() {
        let mut settings = Settings::default();
        assert_eq!(settings.group_label("build-1"), "build-1", "没设就该原样显示");

        settings
            .group_meta
            .insert("build-1".into(), GroupMeta { name: Some("构建".into()), order: None });
        assert_eq!(settings.group_label("build-1"), "构建");
    }

    #[test]
    fn a_group_is_pinned_only_when_it_has_an_order() {
        let settings = Settings::default();
        assert_eq!(settings.group_order("ci"), None, "默认不置顶");

        let raw = r#"{"group_meta": {"ci": {"order": 0}, "work": {"name": "工作"}}}"#;
        let settings = serde_json::from_str::<Settings>(raw).unwrap().clamped();
        assert_eq!(settings.group_order("ci"), Some(0), "0 也是有效的置顶权重");
        assert_eq!(settings.group_order("work"), None, "只有显示名不算置顶");
    }

    #[test]
    fn an_empty_object_yields_all_defaults() {
        assert_eq!(
            serde_json::from_str::<Settings>("{}").unwrap(),
            Settings::default()
        );
    }

    /// 默认必须是「展开」：灯的本职是让人一眼看到状态，折叠是用户主动要的例外
    #[test]
    fn the_default_shape_is_expanded_and_the_sign_shows_everything() {
        let settings = Settings::default();
        assert!(!settings.collapsed, "默认应是展开的牌面");
        assert_eq!(
            settings.show,
            SignFields { name: true, state: true, age: true, message: true },
            "默认每段都显示"
        );
        assert!(settings.hidden_groups.is_empty(), "默认不藏任何组");
        assert_eq!(settings.sign_opacity, DEFAULT_SIGN_OPACITY);
        assert_eq!(settings.blink_speed, 100, "100 = 原始节奏");
    }

    /// 隐藏的组名来自右键菜单、也会被用户手改进配置文件，所以要规整：
    /// 否则同一个组写成 " ci" 和 "ci" 会被当成两个，"已隐藏" 就时灵时不灵。
    #[test]
    fn hidden_groups_are_trimmed_deduped_and_sorted() {
        let raw = r#"{"hidden_groups": [" ci ", "ci", "", "  ", "work", "zeta", "alpha"]}"#;
        let settings = serde_json::from_str::<Settings>(raw).unwrap().clamped();

        assert_eq!(
            settings.hidden_groups,
            vec!["alpha", "ci", "work", "zeta"],
            "去空白、去空串、去重、排序"
        );
        assert!(!settings.shows_group("ci"), "藏起来的组不该显示");
        assert!(settings.shows_group("other"), "没被藏起来的照常显示");
    }

    /// 旧版本的 config.json 里没有这些新字段，不能因此让整份设置失效
    #[test]
    fn missing_appearance_fields_fall_back_to_expanded() {
        let settings: Settings = serde_json::from_str(r#"{"size": 30}"#).unwrap();
        assert_eq!(settings.size, 30, "用户调过的值必须保留");
        assert!(!settings.collapsed, "缺字段应退回「展开」，那是默认形态");
        assert_eq!(settings.sign_opacity, DEFAULT_SIGN_OPACITY);
        assert!(settings.hidden_groups.is_empty());
    }

    #[test]
    fn clamped_pulls_out_of_range_sizes_back() {
        let wild = Settings { size: 9999, ..Settings::default() };
        assert_eq!(wild.clamped().size, MAX_SIZE);
        let tiny = Settings { size: 1, ..Settings::default() };
        assert_eq!(tiny.clamped().size, MIN_SIZE);
    }

    // ---- 兜底分组 ----

    /// 默认必须是**不分组**。这条比看着重要：一旦默认按目录分组，所有现有
    /// 用户的灯会从一块牌变成若干块、窗口尺寸跟着变，属于无声改变既有行为。
    #[test]
    fn grouping_is_off_by_default() {
        let settings = Settings::default();
        assert_eq!(settings.group_by, GroupBy::None);
        assert_eq!(settings.resolve_group(None, Some("/proj/api")), None);
    }

    /// 上报自己带的 `group` **优先**，`group_by` 只是兜底而不是覆盖。
    /// 顺序反了就等于「开了按目录分组之后手写 group 全失效」。
    #[test]
    fn an_explicit_group_beats_the_cwd_fallback() {
        let settings = Settings { group_by: GroupBy::Cwd, ..Settings::default() };
        assert_eq!(
            settings.resolve_group(Some("work"), Some("/proj/api")).as_deref(),
            Some("work")
        );
    }

    #[test]
    fn cwd_mode_falls_back_to_the_full_path() {
        let settings = Settings { group_by: GroupBy::Cwd, ..Settings::default() };

        // **完整路径**，不是最后一段目录名：`~/a/api` 和 `~/b/api` 撞名会让
        // 两个项目的会话混进同一块牌。
        assert_eq!(
            settings.resolve_group(None, Some("/Users/me/code/api")).as_deref(),
            Some("/Users/me/code/api")
        );
        // 末尾分隔符要去掉，否则 `/proj` 和 `/proj/` 会变成两个组
        assert_eq!(
            settings.resolve_group(None, Some("/Users/me/code/api/")).as_deref(),
            Some("/Users/me/code/api")
        );
        assert_eq!(settings.resolve_group(None, Some("  /proj  ")).as_deref(), Some("/proj"));

        // 没给 cwd 就退回 default，而不是造一个空名字的组
        assert_eq!(settings.resolve_group(None, None), None);
        // 空串和空白串都不算「给了一个组」
        assert_eq!(settings.resolve_group(Some("   "), Some("/proj")).as_deref(), Some("/proj"));
        // 根目录去掉分隔符只剩空串 —— 那不是个能用的组名
        assert_eq!(settings.resolve_group(None, Some("/")), None);
    }

    /// `group_by` 是设置里的一个值，所以旧配置（没有这个键）必须能读成 `None`。
    #[test]
    fn settings_without_group_by_read_as_none() {
        let json = serde_json::to_string(&Settings::default()).unwrap();
        assert!(json.contains(r#""group_by":"none""#), "{json}");

        let legacy = json.replace(r#""group_by":"none","#, "");
        let parsed: Settings = serde_json::from_str(&legacy).unwrap();
        assert_eq!(parsed.group_by, GroupBy::None);
    }

    #[test]
    fn window_grows_downward_as_signs_stack() {
        let settings = Settings::default();
        let one_line = sign_height(settings.size, false);
        let one = settings.window_size(&[one_line]);

        // 宽度不随牌数变 —— 牌是竖着叠的，不是横着排
        let two = settings.window_size(&[one_line, one_line]);
        assert_eq!(two.0, one.0, "宽度固定");
        assert!(two.1 > one.1, "多一块牌就多占一份高度");

        // 有消息的那块多一行，于是窗口更高 —— 那是刻意的提示
        let with_message = settings.window_size(&[sign_height(settings.size, true)]);
        assert!(with_message.1 > one.1, "有消息的牌多占一行");

        // 超出上限按上限算，不会无限往下长
        let many = vec![one_line; MAX_SIGNS + 3];
        let capped = vec![one_line; MAX_SIGNS];
        assert_eq!(settings.window_size(&many), settings.window_size(&capped));
    }

    #[test]
    fn a_sign_is_wider_than_tall_and_scales_with_size() {
        for size in [16, 22, 32] {
            let width = sign_width(size);
            let height = sign_height(size, false);
            assert!(width > height, "牌是横条：{size} → {width}×{height}");
            assert_eq!(sign_height(size, true), height * 5 / 3, "有消息时约 1.67 倍高");
        }
    }

    /// 折叠态：一个组一个点，窗口比展开态小得多
    #[test]
    fn collapsed_window_holds_one_dot_per_group() {
        let size = Settings::default().size;

        let (w1, h1) = collapsed_window_size(size, 1, 0);
        assert_eq!(w1, size + LAMP_MARGIN, "一个点：方窗");
        assert_eq!(h1, size + LAMP_MARGIN);

        let (w3, h3) = collapsed_window_size(size, 3, 0);
        assert!(w3 > w1, "多点横着排，所以变宽");
        assert_eq!(h3, h1, "高度只和点的大小有关");

        // 一个组都没有时也要有一个点（表示 offline）
        assert_eq!(collapsed_window_size(size, 0, 0), collapsed_window_size(size, 1, 0));
        // 超出上限按上限算
        assert_eq!(
            collapsed_window_size(size, MAX_DOTS + 5, 0),
            collapsed_window_size(size, MAX_DOTS, 0)
        );
    }

    /// 暂停 / 穿透那枚角标也占一个点的位置 —— 忘了算它，那一枚会把折叠态的
    /// 窗口撑出去一截，而折叠态的意义正是让出地方。
    #[test]
    fn the_flag_badge_takes_a_dot_of_its_own() {
        let size = Settings::default().size;

        let (bare, _) = collapsed_window_size(size, 2, 0);
        let (flagged, _) = collapsed_window_size(size, 2, 1);
        assert_eq!(flagged - bare, size + size / 3, "多一个点加一个点距");

        // 两枚角标不会同时出现（它们共用同一处，穿透优先），所以入参 0/1 之外
        // 的取值是防御性的；但上限仍然要封住，否则窗口会无限变宽。
        let (crazy, _) = collapsed_window_size(size, MAX_DOTS, 9);
        let (capped, _) = collapsed_window_size(size, MAX_DOTS, 1);
        assert_eq!(crazy, capped, "角标再多也只按一枚算");

        // 角标上限是**单独**封的，不侵占组的份额：组数按 MAX_DOTS 截断，
        // 角标再占一枚，两者相加才是最终的点数。
        let (few, _) = collapsed_window_size(size, 2, 0);
        let (few_flagged, _) = collapsed_window_size(size, 2, 1);
        assert_eq!(
            few_flagged - few,
            size + size / 3,
            "空列表时角标也照样占一枚"
        );
        let (full, _) = collapsed_window_size(size, MAX_DOTS, 0);
        let (full_flagged, _) = collapsed_window_size(size, MAX_DOTS, 1);
        assert_eq!(full_flagged - full, size + size / 3, "满屏也不该挤掉角标");
    }

    /// 折叠态必须比展开态小 —— 「折叠」的意义就是让出地方
    #[test]
    fn collapsed_is_much_smaller_than_expanded() {
        let settings = Settings::default();
        let (cw, ch) = collapsed_window_size(settings.size, 1, 0);
        let (ew, eh) = settings.window_size(&[sign_height(settings.size, false)]);
        assert!(cw < ew, "折叠态更窄: {cw} vs {ew}");
        assert!(ch < eh, "折叠态更矮: {ch} vs {eh}");
    }
}
