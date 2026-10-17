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

/// 牌宽 / 牌高的倍率范围。1.0 = 现状。
///
/// 下限 0.8：再小牌上那几段字就压成一团了（牌宽已经是「文字被截断」的前提，
/// 缩太多等于把信息全砍掉）。上限 2.5：已经大到能占掉小半块屏，再多就不该靠
/// 这一盏灯回答了。
pub const MIN_SIGN_MULT: f32 = 0.8;
pub const MAX_SIGN_MULT: f32 = 2.5;

/// 倍率的夹紧。**NaN 要单独挡掉**：浮点的 `f32::clamp` 遇到 NaN 会把 NaN 原样
/// 返回（它是自查式比较，和一切都不相等），而 NaN 一路传到 `ceil` 会变成 0、
/// 把牌宽归零 —— 那正是这份夹紧要防的事。非有限值也一并退回 1.0。
fn clamp_mult(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(MIN_SIGN_MULT, MAX_SIGN_MULT)
    } else {
        1.0
    }
}

/// 圆点四周留白的**总量**（不是每边）。留白是为了好抓取圆点来拖动。
pub const LAMP_MARGIN: u16 = 20;

/// 灯球上最多同时显示几块牌。超出的截断 —— 牌按优先级排，所以留下的是最紧急的。
pub const MAX_SIGNS: usize = 4;

/// 牌的宽度。
///
/// 由 `size` 派生，不另加一个设置 —— `size` 现在表示整块牌的尺度，「大小」
/// 一个旋钮就够。文字长度不定，所以宽度**固定**、超出用省略号截断：让宽度
/// 跟着文字走会让窗口每次消息变化都跳一下。
///
/// `w` 是用户设的**宽度倍率**（见 `Settings::sign_w_mult`），1.0 = 原样。
/// 两步都重要：
/// - **先算整数 `size * 7`，再乘倍率**：这样 `w = 1.0` 时逐字等于旧公式 ——
///   默认值不许无声改变既有行为（红线 3）。
/// - **向上取整**，不是四舍五入：CSS 那边的宽度是 `size*7*w` 这个小数（浏览器
///   不会帮我们取整），取整方向反了（`round` 可能向下）会让窗口比牌窄一线，
///   `overflow: hidden` 就把右边缘裁掉了。向上只会多留不到 1px。
pub fn sign_width(size: u16, w: f32) -> u16 {
    ((size * 7) as f32 * w).ceil() as u16
}

/// 一块牌的高度。有消息就多一行 —— 那块牌会变高，正好是个「有东西可读」的提示。
///
/// `h` 是高度倍率，弄法和 `sign_width` 一样：先整数、后乘、向上取整。
///
/// **CSS 里的 `.sign` / `.sign[data-has-message="true"]` 必须与这两个值严格对齐**，
/// 否则窗口会裁掉最后一块牌、或者底下留一条缝。
pub fn sign_height(size: u16, has_message: bool, h: f32) -> u16 {
    let base = if has_message { size * 5 / 2 } else { size * 3 / 2 };
    (base as f32 * h).ceil() as u16
}

/// 折叠形态的圆点个数上限。折叠成一个单纯的灯时，一个组一个点。
///
/// 刻意和 `MAX_SIGNS` 取同一个值：折叠只是换了个外形，不该顺带少显示几个组 ——
/// 那样会让人以为一折叠就把状态弄丢了。
pub const MAX_DOTS: usize = MAX_SIGNS;

/// 折叠态最多几枚额外角标：暂停 / 穿透一枚，`+N` 一枚。
///
/// 两种各自都最多一枚，所以这是个真上限、不是随手取的数 —— 它只用来兜住
/// 一个算错的调用点，别让它把窗口撑得没边。
const MAX_BADGES: usize = 2;

/// 折叠态要不要挂那枚 `+N` 角标：可见组数超过 [`MAX_DOTS`] 时，多出来的那些
/// 在灯上**一点痕迹都没有** —— 而「有几个智能体在跑」恰恰是这盏灯要回答的问题。
///
/// 展开态同样的提示直接挂在最后一块牌上（见 `SignalLamp.tsx` 的 `sign-more`），
/// 折叠态没有牌面可寄住，只能自己占一个点位。
///
/// **两边必须是同一个判断**：前端画几枚，`collapsed_window_size` 的 `badges`
/// 就得算几枚 —— 少了那枚角标会被窗口右边裁掉，多了会白留一块。
pub fn more_badge(visible: usize) -> usize {
    usize::from(visible > MAX_DOTS)
}

/// 折叠形态的窗口尺寸：N 个点排成一行。
///
/// 这套几何比展开态小得多 —— 折叠的意义就是让出地方。一个组一个点，
/// 所以点数和组数一样；一个组都没有时也要有一个点（表示 offline）。
///
/// `badges` 是**占一个点位的额外角标数**，目前两种：暂停 / 穿透那一枚（见 `lib.rs`
/// 的 `ui_flag_count`），和「还有几组没画出来」的 `+N` 那一枚（见 [`more_badge`]）。
///
/// 角标也占一个点的位置，所以点数要加上它 —— 忘了加的话那一枚会把窗口撑出界，
/// 而折叠态的意义正是「让出地方」，撑出去就白让了。
///
/// ⚠️ 点径、点距、留白和 CSS 的 `.lamp-dot` / `.lamp-dot-flag` / `.lamp-dot-more` /
/// `.lamp-root` 是两处真值（见本文件头），改一处必须同步另一处。
pub fn collapsed_window_size(size: u16, dots: usize, badges: usize) -> (u16, u16) {
    // 分别封顶再相加，而不是封顶它们的和：封住和的话「多枚角标」会被当成
    // 「少几个组」，于是牌少了一块 —— 那正是截断要避免的那种谎。
    let dots = dots.clamp(0, MAX_DOTS);
    let badges = badges.min(MAX_BADGES);
    let dots = (dots + badges).max(1) as u16;
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
///
/// 也走原子写：端口文件被读到的时机是「CLI 刚启动、要连上去」，一个残缺的数字
/// 会让它连到错的端口或者干脆解析失败 —— 而这个文件每次启动只写一次，代价可以忽略。
pub fn write_port(port: u16) -> io::Result<()> {
    atomic_write(&port_file()?, &port.to_string(), None)
}

/// hook 调用日志的路径。
///
/// 存在的原因很实际：出问题时（「灯为什么显示成这样」）需要知道**哪些事件
/// 真的触发了**。事件本身看不到的话，这类问题只能靠猜——而「一轮结束时
/// Stop 有没有触发」正是这样一个问题。
pub fn hook_log_path() -> io::Result<PathBuf> {
    Ok(data_dir()?.join("hooks.log"))
}

/// 每日状态时长的落盘位置。
///
/// 单独一个文件而不是塞进 `config.json`：那份是**用户手改的偏好**（改了要能一眼
/// 看懂、甚至能直接编辑），而这个是机器攒的运行数据 —— 混在一起会让配置文件里
/// 冒出一大坨没人看得懂、又不敢删的东西。
pub fn stats_file() -> io::Result<PathBuf> {
    Ok(data_dir()?.join("stats.json"))
}

/// 每个信号源「最近一次上报」的落盘位置（见 `receipts.rs`）。
///
/// 和 `stats.json` 一样是**机器攒的运行数据**，不塞进 `config.json`（那份是用户手改
/// 的偏好，混进来会让配置文件里冒出一坨没人看得懂、又不敢删的东西）。
pub fn receipts_file() -> io::Result<PathBuf> {
    Ok(data_dir()?.join("receipts.json"))
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

/// 往排查日志追加一行。
///
/// **四个出口共用这一个。** 入站（智能体调 CLI）、出站（SignalHub 发钩子）、
/// 崩溃记录（`install_panic_hook`）都写它 —— 各写一遍轮转逻辑的话，迟早有一处
/// 忘了加，那个文件就会无界增长。
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
    // **0600**，和 token 一个待遇：这份日志里记着出站钩子的完整 URL（带推送服务的
    // 密钥），不该让同一台机器上别的用户读得到（见 issue #2）。只在**建文件**时生效；
    // 已经存在的老文件保持原样（不偷偷改用户的文件权限）。
    let mut options = fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    if let Ok(mut file) = options.open(&path) {
        let _ = file.write_all(line.as_bytes());
    }
}

// ---- 用户设置 --------------------------------------------------------------

/// 圆点形状。
///
/// 默认是圆；此时 `permission` / `error` 会额外画成**圆角方**，那是给色盲用的
/// 形状区分（红绿是红绿色盲最难分的一对）。用户若把形状改成别的，这个区分就
/// 失效了 —— 设置面板里会说明这一点。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Shape {
    #[default]
    Circle,
    Rounded,
    /// 空心圆环。桌面上几乎没有别的东西长这样，最好认。
    Ring,
}

/// 多灯布局：一盏灯画一叠牌，还是每个可见组一盏自己的灯。
///
/// **默认单灯**，也是永远可用的回落形态（可见组超过 `lamps::MAX_LAMPS` 时自动回到
/// 它）—— 新能力默认关（红线 3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LampLayout {
    /// 一盏灯画一叠牌（现状）。
    #[default]
    Single,
    /// 每个可见组一盏自己的灯窗，可以各自拖到不同位置；主灯只剩一颗全局灯。
    PerGroup,
}

/// 配色方案。
///
/// 给的是**预设而不是任意取色**：红 = 危险、黄 = 注意是有语义的，
/// 让用户随便改会把这层含义弄没。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Palette {
    /// 默认：绿 / 黄 / 红，颜色压暗一档以便在浅色桌面上站得住
    #[default]
    Default,
    /// 色盲友好：Okabe-Ito 的蓝 / 橙 / 朱红
    Colorblind,
    /// 高饱和：在花哨或深色的桌面上更容易看清
    Vivid,
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
    Http {
        url: String,
        /// 额外请求头。**鉴权靠它** —— `Authorization: Bearer …`、ntfy 的
        /// `X-Token`。没有它的话，任何要鉴权的地址都配不了。
        #[serde(default)]
        headers: BTreeMap<String, String>,
        /// 服务预设：决定 content-type 和 body 的形状（见 [`HttpPreset`]）。
        ///
        /// `#[serde(default)]` 是「老配置照旧」那条契约的落点：
        /// `{"type":"http","url":"…"}` 解析出来就是 `raw`，一个字节都不变。
        #[serde(default)]
        preset: HttpPreset,
    },
    Command { command: String },
}

/// HTTP 转发的**服务预设**：决定 content-type 和 body 形状。
///
/// 起因是一条很具体的反馈：照着占位符填了 ntfy，收到的是一大坨 JSON。默认发的
/// `raw` 是 SignalHub 自己的快照 schema，而现实里做通知的那些服务（Slack /
/// Discord / Telegram / 飞书 / 企业微信 / 钉钉 / Bark / ntfy）要的都是
/// **一个 URL + 一段文字**，服务端自己决定怎么排版。
///
/// **默认 `raw`**：新能力默认关（红线 3），也保证老的 `{"type":"http","url":…}`
/// 行为一个字都不变。每个预设的形状都在 `outbound::compose` 里，那是唯一一处实现。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HttpPreset {
    /// 原样发 SignalHub 自己的 JSON 快照（默认）。给「自己写了个接收端」的人用。
    #[default]
    Raw,
    /// 一行纯文本。任何「贴一行字」的地址都吃（ntfy / Bark / 自己的脚本）。
    Text,
    /// 同文本，另加 `Title` 头 —— ntfy 用它当通知的标题。
    Ntfy,
    /// Bark（iOS 推送）
    Bark,
    /// Slack incoming webhook：`{"text": …}`
    Slack,
    /// Discord webhook：`{"content": …}`
    Discord,
    /// 飞书自定义机器人：`{"msg_type":"text","content":{"text": …}}`
    Lark,
    /// 企业微信群机器人：`{"msgtype":"text","text":{"content": …}}`
    Wecom,
    /// 钉钉自定义机器人：形状同企业微信
    Dingtalk,
    /// Telegram Bot API。`chat_id` 写在 URL 的查询串上
    /// （`…/bot<token>/sendMessage?chat_id=<id>`），发送时搬进表单体。
    Telegram,
    /// PushDeer（官方服务器或自架）。`pushkey` 写在 URL 的查询串上
    /// （`https://api2.pushdeer.com/message/push?pushkey=<key>`），正文按表单体发。
    Pushdeer,
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

/// **宽松解析**：认识的值照收（大小写不敏感），**不认识的落到 `raw`**。
///
/// 不是随手加的宽容。`Settings` 是一个**整体**：任何一个字段解析失败都会让它整份
/// 退回默认值（`load_settings` 那个兜底）。而手写配置里把 `"slack"` 写成 `"Slack"`、
/// 或者照着别的工具写成 `"webhook"`，都是很正常的事 —— 代价却是用户丢掉**全部**设置
/// （组显示名、配色、所有钩子）。退回一个预设，用户看到的是「发出去的是 JSON」，
/// 而设置面板上那个下拉就明写着「原样 JSON」，改一下就好了。
///
/// 序列化永远是那几个小写名字（`#[serde(rename_all = "lowercase")]`）。
impl<'de> Deserialize<'de> for HttpPreset {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Ok(match raw.trim().to_ascii_lowercase().as_str() {
            "text" => HttpPreset::Text,
            "ntfy" => HttpPreset::Ntfy,
            "bark" => HttpPreset::Bark,
            "slack" => HttpPreset::Slack,
            "discord" => HttpPreset::Discord,
            "lark" => HttpPreset::Lark,
            "wecom" => HttpPreset::Wecom,
            "dingtalk" => HttpPreset::Dingtalk,
            "telegram" => HttpPreset::Telegram,
            "pushdeer" => HttpPreset::Pushdeer,
            _ => HttpPreset::Raw,
        })
    }
}

impl HookAction {
    /// 目标填了吗。没填的钩子不触发、也不报错 —— 编辑时必然要经过
    /// 「刚添加、还没填目标」这个中间态，那时候报错就没法编辑了。
    pub fn is_configured(&self) -> bool {
        match self {
            HookAction::Http { url, .. } => !url.trim().is_empty(),
            HookAction::Command { command } => !command.trim().is_empty(),
        }
    }

    /// 目标本身（网址或命令）。
    ///
    /// 日志和界面都**按它去认一条钩子**，而不是按下标 —— 钩子列表可以增删排序，
    /// 下标会错位，于是会显示成「另一条钩子的结果」，比不显示更糟。
    pub fn target(&self) -> String {
        match self {
            HookAction::Http { url, .. } => url.clone(),
            HookAction::Command { command } => command.clone(),
        }
    }

    /// 一条只要网址的 HTTP 钩子（预设 `raw`、不带额外头）。
    ///
    /// 给「就要默认行为」的调用方和测试一个短写法：多出来两个字段之后，每处都写全
    /// `Http { url, headers: BTreeMap::new(), preset: HttpPreset::Raw }` 会把真正的
    /// 意图（「默认那条路」）埋在一片噪音里。
    pub fn http(url: impl Into<String>) -> Self {
        HookAction::Http {
            url: url.into(),
            headers: BTreeMap::new(),
            preset: HttpPreset::default(),
        }
    }
}

/// 额外请求头的条数上限。
///
/// 转发目标（尤其是别人给你的一个 webhook 地址）算**不可信输入**：一个几千条的
/// map 会让每次转发都多花一大截时间在拼头上，而正常的鉴权只需要一两条。
pub const MAX_HOOK_HEADERS: usize = 16;

/// 规整额外请求头：去空白、丢掉空键、按上限截断。
///
/// **不做「只差大小写」的去重**：HTTP 头名是大小写不敏感的，但要判断
/// `Authorization` 和 `authorization` 哪个是「后写的那条」，得知道用户写下的
/// **顺序** —— 而 `BTreeMap` 已经把顺序排掉了，任何胜者都是任意挑的。宁可两条都发
/// （服务器会用一个状态码告诉我们它不接受），也不要悄悄丢掉一条用户明确写下的鉴权头。
/// 编辑界面在输入那一层挡重复的键，那里顺序还在。
///
/// 截断按**键的字典序**（`BTreeMap` 的顺序），所以丢哪几条是确定的，不是随机。
fn normalize_headers(headers: BTreeMap<String, String>) -> BTreeMap<String, String> {
    headers
        .into_iter()
        .filter_map(|(name, value)| {
            let name = name.trim().to_string();
            if name.is_empty() {
                return None;
            }
            Some((name, value.trim().to_string()))
        })
        .take(MAX_HOOK_HEADERS)
        .collect()
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
    /// 停留时长（毫秒）：这个状态**静默多久算过期**。没写 = 用内置默认
    /// （`state::Signal::default_ttl`，如工作态 5 分钟、空闲态 10 分钟）。
    ///
    /// 和上面三样不同，它**不是外观**：它决定「智能体不出声之后，它的牌还在灯上
    /// 待多久」。放在同一张表里是因为调节它的场景就是「我盯着某一个状态觉得它
    /// 消失得太早 / 太晚」—— 那时人已经在看这张表了。
    ///
    /// 它只在**上报进来的那一刻**算一次，不追溯已存在的会话（见 `state::state_ttl`）。
    pub ttl_ms: Option<u32>,
}

impl StateStyle {
    /// 一样都没写。这种条目没有任何作用，`clamped()` 会把它们丢掉 ——
    /// 留着会让「预设是否还生效」的判断无谓地失败。
    pub fn is_empty(&self) -> bool {
        self.color.is_none() && self.shape.is_none() && self.blink_ms.is_none() && self.ttl_ms.is_none()
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

/// 界面语言的**偏好**。注意和 `Lang` 的区别：那个是**解析后**的语言，只有中文和
/// 英文；这里的 `Auto` 要由前端（CLI 那边则由环境变量）解析成具体语言 ——
/// Rust 不知道系统语言。
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

/// 解析之后的界面语言。
///
/// **和 `Locale` 放在一起**而不是 `lang.rs`：CLI 也要用它挑自己的文案（`--help`
/// 和报错），而 `lang.rs` 里那套 `NativeText` 是 app 侧的（托盘菜单、系统通知），
/// CLI 一条都用不上 —— 把整个 `lang.rs` 拖进去只会换来一堆 dead_code 警告，而
/// CI 现在是 `-D warnings`。理由和上面 `Locale` 那段是同一条。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lang {
    /// 默认中文：这是这个项目的母语，也是「谁都没告诉我们」时唯一有依据的猜测。
    #[default]
    Zh,
    En,
}

impl Lang {
    /// 认不出就返回 `None`，由调用方决定怎么办 —— 不在这里猜一个默认值，
    /// 因为「拿到一个不认识的语言」是需要能被发现的事。
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "zh" | "zh-cn" | "zh-hans" | "zh-hant" => Some(Lang::Zh),
            "en" | "en-us" | "en-gb" => Some(Lang::En),
            _ => None,
        }
    }
}

/// app 解析出来的**当前界面语言**，供 CLI 读。
///
/// 为什么要有这个文件：CLI 是独立进程，它要挑自己的 `--help` / 报错文案，而
/// `locale: "auto"` 的解析**只有前端做得了**（它读 `navigator.language`）。让 CLI
/// 自己去猜环境变量在 Windows 上会直接落空，也会和界面上实际显示的语言不一致 ——
/// 而这两个**必须一致**：用户在英文界面上敲 `signalhub --help`、看到的却是中文，
/// 会以为装错了版本。
///
/// 和 `port` 是同一类东西：app 写、CLI 读的运行时事实。
pub fn lang_file() -> io::Result<PathBuf> {
    Ok(data_dir()?.join("lang"))
}

/// app 在 `set_lang` 时写一次（见 `lib.rs`）。
pub fn write_lang(lang: Lang) -> io::Result<()> {
    let text = match lang {
        Lang::Zh => "zh",
        Lang::En => "en",
    };
    atomic_write(&lang_file()?, text, None)
}

/// CLI 读它。**读不到就返回 `None`**（app 从没跑过），由调用方决定退到哪儿。
pub fn read_lang() -> Option<Lang> {
    Lang::parse(&fs::read_to_string(lang_file().ok()?).ok()?)
}

/// 把「偏好 + app 写下的实际语言 + 环境变量」折成一个具体语言。
///
/// 顺序是有讲究的：
///   1. `Settings::locale` 是**显式**的 zh / en → 用它。用户明确选过，最权威。
///   2. 是 `auto` → 用 app 写下的 `lang` 文件。那是界面**实际**在显示的语言。
///   3. 还没有（app 从没跑过）→ 猜环境变量。
///   4. 连环境变量都没有 → `Lang::default()`。
///
/// **`lang` 文件只对 `auto` 生效**：显式偏好排在它前面，所以用户在应用没跑的时候
/// 手改 `config.json` 也立刻有效，不会被上一次运行留下的文件压住。
pub fn resolve_lang(settings: &Settings) -> Lang {
    match settings.locale {
        Locale::Zh => Lang::Zh,
        Locale::En => Lang::En,
        Locale::Auto => read_lang().or_else(lang_from_env).unwrap_or_default(),
    }
}

/// 从环境变量猜系统语言。
///
/// 按 POSIX 的优先级取：`LC_ALL` > `LC_MESSAGES` > `LANG`。**`LANG` 在 Windows 上
/// 通常不存在**，所以这条路只对 Unix 可靠 —— Windows 靠的是上面那个 `lang` 文件，
/// 而 app 只要跑过一次就会把它写下来。
///
/// 取不到就继续往下试，**不因为 `LC_ALL=C` 或空串而放弃**：那两个都表示「没有
/// 指定 locale」，而不是「指定成某种不认识的语言」。
fn lang_from_env() -> Option<Lang> {
    for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(value) = std::env::var(key)
            && let Some(lang) = Lang::parse(&normalize_posix_locale(&value))
        {
            return Some(lang);
        }
    }
    None
}

/// POSIX 的 locale 写法 → `Lang::parse` 认得的那种。
///
/// `en_US.UTF-8` → `en-US`：去掉 `.` 之后的编码，下划线换成连字符。`Lang::parse`
/// 认的是 `navigator.language` 那一套（`en-US`），两边只差这一层。
pub fn normalize_posix_locale(raw: &str) -> String {
    // `@` 是修饰符（`sr_RS@latin`），和编码一样丢掉
    let base = raw.split(['.', '@']).next().unwrap_or("");
    base.replace('_', "-")
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

/// 用户设置。**这份文件是用户可手改、也会跨版本留存的**，所以带 `serde(default)`：
/// 缺字段时退回该字段的默认值，而不是整份解析失败 —— 否则每次新增设置项，都会把
/// 用户已经调好的其他设置一起重置。
///
/// 注意它是**容器级**的：加新字段**不需要**逐字段写 serde 属性，只要在下面那个
/// 手写的 `Default` 里给出想要的默认值。
///
/// 不派生 `Copy`：`webhook_url` 是 `Option<String>`。相关方法因此都取 `&self`。
///
/// 也**不派生 `Eq`**：牌宽 / 牌高是 `f32` 倍率，而 `f32` 不是 `Eq`（NaN ≠ NaN）。
/// 需要比较的地方（导出 / 导入的往返测试）用 `PartialEq` 就够了。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// 圆点直径（px）
    pub size: u16,
    /// 牌宽倍率。**1.0 = 现状**（`size * 7`）。
    ///
    /// 只作用于**展开态**的牌 —— 折叠态是一排正方形点，长宽倍率对它没有意义
    /// （写了也只会让点变椭圆）。窗口尺寸照旧按内容算（见 `apply_window_size`），
    /// 这个倍率只是把「内容」本身放大或缩小。
    pub sign_w_mult: f32,
    /// 牌高倍率。**1.0 = 现状**（一行 `size * 3/2`，带消息两行 `size * 5/2`）。
    ///
    /// 两档都乘 —— 漏一档窗口就会裁掉带消息的那块牌。
    pub sign_h_mult: f32,
    /// 多灯布局。默认 `Single`（一盏灯画一叠牌），`PerGroup` 时每个可见组一盏灯。
    pub lamp_layout: LampLayout,
    pub always_on_top: bool,
    /// 拖完之后若窗口离屏幕**左 / 右**边缘 12px 以内，就吸附过去贴齐。
    ///
    /// **默认关**（红线 3）：吸附会改变「拖到哪就是哪」的既有手感，得由用户明确
    /// 打开。只在左右吸 —— 上下是任务栏 / Dock 的地盘，吸过去会和 `keep_on_screen`
    /// 的底边夹取抢地方。
    pub snap_to_edge: bool,
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
    /// 把 HTTP 服务暴露到局域网，让别的机器 / 容器 / WSL 里的智能体也能上报。
    ///
    /// **默认必须是关的**，这条比看着重要：开了之后网内任何人只要拿到 token 就能
    /// 上报，而「带 Origin 头一律拒绝」那道防线对 curl 这类**非浏览器**客户端无效
    /// —— 它挡的是「你正在浏览的网页偷偷打本地端口」，挡不住同一网段里的另一台机器。
    /// 换句话说开了之后唯一的防线就是 token 本身。所以这是个显式开关，不悄悄放宽。
    ///
    /// **改完要重启应用才生效**：监听地址在启动时绑定一次，运行中重绑会让端口变化
    /// （CLI 的自动发现跟着断一下），出问题时表现为「灯突然收不到上报」这种很难
    /// 归因的现象。与其在运行中做这件事，不如写明「下次启动生效」。
    pub lan_access: bool,
    /// **只读 Web 面板**（`GET /`）：手机 / 平板上打开一个链接就能看灯。
    ///
    /// **默认关**（红线）：它是一个**新的对外读取面**，而灯本身是私人的东西
    /// （哪个项目在跑、跑到哪一步、哪一步卡住了）。要看得见，得由用户明确打开。
    ///
    /// 它和上报服务**共用监听地址**：绑回环时只有本机能看；开了「允许局域网访问」
    /// 才轮到同一 WiFi 下的设备 —— 那时设置页会明说这一点。
    ///
    /// **改完立刻生效**（不像 `lan_access` 要重启）：它只是一个路由的开关，
    /// 读的是这份设置的当前值，不涉及重新绑定。
    pub web_panel: bool,
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
    /// 用上报里的 `session` 当组名。**一个会话一块牌。**
    ///
    /// 同一个智能体并行开好几个会话时（每个窗口 / 每个 worktree 一个），只按目录
    /// 分组还是会把它们挤在同一块牌上 —— 而它们的进度往往是各不相同的。按会话分
    /// 就把每个会话摊成自己的一块，多个智能体自然也分开了（它们的会话必定不同）。
    ///
    /// 代价是组名是上报方给的会话 id，多半是个 UUID 那样不好读的串；牌上会自动
    /// 截断，想去「外观 → 各组的显示」改名也行。
    Session,
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

/// 单个状态的**停留时长**（毫秒）的可调范围：这个状态静默多久后算过期。
///
/// **刻意没有「永不过期」这一档。** 一盏不会自己熄灭的灯等于一条永远不撤的
/// 告警：智能体被强杀（`SessionEnd` 没触发）之后它会一直亮着，用户拿它没办法。
/// 早先 `idle` 没有 TTL，一个忘关的会话能在灯上挂半小时 —— 那还只是半小时。
///
/// 下界 1 秒：比一次上报还短的 TTL 等于「报完立刻消失」，那不是设置，是自相矛盾。
/// 上界 24 小时：有理由要很久（跨夜跑的任务），但不能没有边 —— 灯最终必须能回到
/// 「没有智能体上报」。上界同时受 `Signal::default_ttl` 那条注释约束：放宽的代价
/// 是「崩溃的智能体」更晚被发现。
pub const MIN_STATE_TTL_MS: u32 = 1_000;
pub const MAX_STATE_TTL_MS: u32 = 24 * 60 * 60 * 1_000;

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

/// 取路径最后一段。两种分隔符都认，免得 Windows 上写的路径在别处解析错。
///
/// 通知正文（`alerts.rs`）和会话分组的默认显示名（`state.rs`）都用它 ——
/// 两处各写一遍「最后一段怎么切」迟早会分叉，而它们要的显然是同一个东西。
pub fn project_name(cwd: &str) -> String {
    cwd.rsplit(['/', '\\'])
        .find(|segment| !segment.is_empty())
        .unwrap_or(cwd)
        .to_string()
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
        HookAction::Http { url, .. } if !url.trim().is_empty() && !is_http_url(url) => {
            Some(format!("钩子地址不合法：{url:?}（需要以 http:// 或 https:// 开头）"))
        }
        _ => None,
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            size: DEFAULT_SIZE,
            // 「原样」。**必须是 1.0**：写成 0.0 就是牌宽归零（红线 3 的经典翻车）。
            sign_w_mult: 1.0,
            sign_h_mult: 1.0,
            // 默认单灯 —— 新能力默认关（红线 3）
            lamp_layout: LampLayout::Single,
            always_on_top: true,
            // 吸附会改变拖动的手感，默认关（红线 3）
            snap_to_edge: false,
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
            // 默认只监听本机。见字段注释：开了之后唯一的防线就是 token
            lan_access: false,
            web_panel: false,
        }
    }
}

impl Settings {
    /// 把越界值夹回允许范围。设置来自两处不可信输入：手改过的 config.json
    /// 和前端传来的值。
    pub fn clamped(mut self) -> Self {
        self.size = self.size.clamp(MIN_SIZE, MAX_SIZE);
        // 倍率先夹紧再往下走：NaN / 无穷 / 手改的离谱值都在这儿被挡掉
        self.sign_w_mult = clamp_mult(self.sign_w_mult);
        self.sign_h_mult = clamp_mult(self.sign_h_mult);
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
            // 停留时长没有「常驻」这一档，所以直接夹（见 MIN_STATE_TTL_MS 的注释）
            if let Some(ms) = style.ttl_ms {
                style.ttl_ms = Some(ms.clamp(MIN_STATE_TTL_MS, MAX_STATE_TTL_MS));
            }
        }
        // 一样都没写的条目没有任何作用，留着只会让「预设是否还生效」的判断无谓失败
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
            if let HookAction::Http { headers, .. } = &mut hook.action {
                *headers = normalize_headers(std::mem::take(headers));
            }
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
    ///
    /// `session` 是**必定有值**的（`Report::session_id()` 兜到 `default`），所以
    /// `GroupBy::Session` 不需要 `cwd` 那样的 `Option` 处理。
    pub fn resolve_group(
        &self,
        group: Option<&str>,
        cwd: Option<&str>,
        session: &str,
    ) -> Option<String> {
        let explicit = group.map(str::trim).filter(|name| !name.is_empty());
        if let Some(name) = explicit {
            return Some(name.to_string());
        }
        match self.group_by {
            GroupBy::None => None,
            GroupBy::Cwd => cwd.and_then(group_from_cwd),
            // 会话 id 本身就是组名。空串理论上到不了这儿（`session_id()` 兜了
            // `default`），但真到了也没有意义 —— 退回不分组比造一个空组名好。
            GroupBy::Session => {
                let id = session.trim();
                (!id.is_empty()).then(|| id.to_string())
            }
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

    /// 灯上那块牌**显示**的组名：用户改的名字 > 服务端给的默认名 > 原始组名。
    ///
    /// 和前端 `lampSort.ts::labelOf` 是同一条三级规则。转发摘要用它，因为转发出去的
    /// 那行字必须是**同一个人看到的那个名字**：用户在灯上把 `build-1` 改成了「构建」，
    /// 转发里却还是 `build-1`，那就是两处真值打架。
    ///
    /// 匹配（隐藏 / 静音 / 通知动作 / 钩子的 `groups`）**永远用原始 `name`** ——
    /// 和前端一样，显示名不参与任何匹配。
    pub fn display_label<'a>(&'a self, group: &'a crate::state::GroupView) -> &'a str {
        if let Some(custom) = self
            .group_meta
            .get(&group.name)
            .and_then(|meta| meta.name.as_deref())
        {
            return custom;
        }
        group.default_label.as_deref().unwrap_or(&group.name)
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
        (sign_width(self.size, self.sign_w_mult) + LAMP_MARGIN, stacked + gaps + LAMP_MARGIN)
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
                action: HookAction::http(url),
            });
        }
    }
    settings
}

pub fn save_settings(settings: &Settings) -> io::Result<()> {
    let settings = settings.clone().clamped();
    let json = serde_json::to_string_pretty(&settings)
        .map_err(|err| io::Error::other(format!("序列化设置失败: {err}")))?;
    atomic_write(&settings_file()?, &json, None)
}

/// 原子地写一个文件：先写同目录的临时文件，再 `rename` 过去。
///
/// **为什么必须原子**：`load_settings` 的失败兜底是**静默退回全部默认值**。非原子写
/// 写到一半崩溃（断电、强杀、磁盘满），留下的就是残缺的 JSON —— 用户调了一小时的
/// 颜色、钩子、分组会一声不响地全没，而且没有任何地方会告诉他。`rename` 在同一个
/// 文件系统内是原子的，所以读到的要么是旧的完整内容、要么是新的完整内容。
///
/// 三处要点，少一处都会出问题：
///
/// - 临时文件**必须和目标同目录**。`rename` 跨文件系统会失败（`EXDEV`），而放
///   `/tmp` 正好就跨了 —— 配置在 `~/.signalhub`，`/tmp` 常常是另一个挂载点。
/// - 权限在建临时文件时就设好，不是 `rename` 之后再 `chmod`。后者会留下一个
///   「已经是正式文件、但还没收紧权限」的窗口期 —— 那正是 `write_private` 注释里
///   说的那个窗口。
/// - 目标是**符号链接**时写到它指向的地方。用 dotfiles 管配置的人常把
///   `~/.claude/settings.json` 做成链接，而 `rename` 换的是目录项，会把链接替换成
///   一个普通文件 —— 用户的仓库就此断掉，而且不会报错。
pub(crate) fn atomic_write(path: &Path, contents: &str, mode: Option<u32>) -> io::Result<()> {
    // symlink 要顺着写。canonicalize 对「存在但不是链接」也返回同一个路径（只是
    // 顺带解了目录上的链接），对「还不存在」失败 —— 那时用原路径正是要的。
    let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("目标路径没有父目录"))?;
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("目标路径没有文件名"))?;
    // 带上进程 id：两个实例同时写（托盘和设置窗各触发一次）时不至于互相踩同一个临时文件
    let tmp = dir.join(format!(
        ".{}.{}.tmp",
        name.to_string_lossy(),
        std::process::id()
    ));

    match write_then_rename(&tmp, &path, contents, mode) {
        Ok(()) => Ok(()),
        Err(err) => {
            // 半截的临时文件留在配置目录里只会让下次困惑
            let _ = fs::remove_file(&tmp);
            Err(err)
        }
    }
}

fn write_then_rename(tmp: &Path, path: &Path, contents: &str, mode: Option<u32>) -> io::Result<()> {
    use std::io::Write;

    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    if let Some(mode) = mode {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(mode);
    }
    // Windows 上文件默认继承用户目录的 ACL，`mode` 没有对应概念
    #[cfg(not(unix))]
    let _ = mode;

    let mut file = options.open(tmp)?;
    file.write_all(contents.as_bytes())?;
    // 先把数据刷下去再改名。不 sync 的话，崩溃后可能出现「名字已经换了、内容还是空的」
    // —— 那正是这个函数要防的那种残缺文件。
    file.sync_all()?;
    drop(file);

    fs::rename(tmp, path)
}

/// 只有它能读的东西（目前是 token）才用这个。分开命名是为了让调用点自己说清
/// 「这个文件是私密的」，而不是靠传一个 `Some(0o600)` 让人猜。
fn write_private(path: &Path, contents: &str) -> io::Result<()> {
    atomic_write(path, contents, Some(0o600))
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
    use std::sync::atomic::{AtomicUsize, Ordering};

    // ---- 原子写 ---------------------------------------------------------------

    /// 每个用例一个独立目录，避免相互干扰（同 hooks.rs 的测试）
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let unique = format!(
                "signalhub-config-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::SeqCst)
            );
            let dir = std::env::temp_dir().join(unique);
            fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }

        fn file(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn atomic_write_creates_and_replaces() {
        let dir = TempDir::new();
        let path = dir.file("config.json");

        atomic_write(&path, "first", None).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "first");

        atomic_write(&path, "second", None).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "second", "覆盖要生效");
    }

    /// **不留临时文件。** 半截的 `.config.json.<pid>.tmp` 落在用户的 `~/.signalhub`
    /// 里，下次只会让人困惑「这是什么、能不能删」。
    #[test]
    fn atomic_write_leaves_no_temp_file_behind() {
        let dir = TempDir::new();
        atomic_write(&dir.file("config.json"), "x", None).unwrap();

        let mut names: Vec<String> = fs::read_dir(&dir.0)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["config.json"], "配置目录里只该有目标文件");
    }

    /// 目标是符号链接时写到**它指向的地方**，不能把链接本身替换掉。
    ///
    /// 用 dotfiles 管配置的人常把 `~/.claude/settings.json` 做成链接 —— 就地
    /// `rename` 会把链接变成一个普通文件，用户的仓库就此断掉，而且不会报错。
    #[cfg(unix)]
    #[test]
    fn atomic_write_follows_a_symlink_instead_of_replacing_it() {
        let dir = TempDir::new();
        let real = dir.file("real.json");
        let link = dir.file("link.json");
        fs::write(&real, "old").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();

        atomic_write(&link, "new", None).unwrap();

        assert_eq!(
            fs::read_to_string(&real).unwrap(),
            "new",
            "内容要落到链接指向的那个文件上"
        );
        assert!(
            fs::symlink_metadata(&link).unwrap().file_type().is_symlink(),
            "链接本身必须还在，否则 dotfiles 仓库就断了"
        );
    }

    /// 私密文件建出来就该是 0600 —— 不能有「先 world-readable 再收紧」的窗口期。
    #[cfg(unix)]
    #[test]
    fn atomic_write_applies_the_private_mode_up_front() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new();
        let path = dir.file("token");
        atomic_write(&path, "s3cret", Some(0o600)).unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "实际 {mode:o}");
    }

    /// 目标目录不存在时给出错误，而不是 panic —— 调用点要把错误报给用户。
    #[test]
    fn atomic_write_reports_a_missing_directory() {
        let dir = TempDir::new();
        let missing = dir.file("nope").join("config.json");
        assert!(atomic_write(&missing, "x", None).is_err());
    }

    // ---- 导出 / 导入 ----------------------------------------------------------

    /// 导出 → 导入的往返：序列化出来的必须能**一模一样**地读回来。
    ///
    /// 这是「换机器 / 重装后导入回去」的全部承诺。漏一个字段、类型对不上，用户的
    /// 颜色 / 钩子 / 分组就白导了 —— 而且导入那边只会「成功」，不会有任何提示。
    ///
    /// **每一项都设成非默认值**：全用默认的话，漏掉哪个字段都看不出来。
    /// 两个已在迁移里搬走的旧字段（`webhook_url` / `colors`）保持默认：它们标了
    /// `skip_serializing`，本来就不落盘，设了反而会失败。
    #[test]
    fn settings_survive_an_export_import_round_trip() {
        let settings = Settings {
            size: 30,
            // 新字段设成**非默认值**：全默认的话「漏了字段」这种错看不出来
            sign_w_mult: 1.4,
            sign_h_mult: 0.9,
            lamp_layout: LampLayout::PerGroup,
            always_on_top: false,
            snap_to_edge: true,
            quiet: false,
            notify: false,
            hooks: vec![
                Hook {
                    when: vec!["permission".into(), "error".into()],
                    groups: vec!["builds".into()],
                    // preset / headers 都设成**非默认值**：全默认的话「漏了这个字段」
                    // 这种错在往返里看不出来，而这两个字段正好是新加的
                    action: HookAction::Http {
                        url: "https://example.test/hook".into(),
                        preset: HttpPreset::Slack,
                        headers: [("X-Token".to_string(), "abc".to_string())]
                            .into_iter()
                            .collect(),
                    },
                },
                Hook {
                    when: vec![],
                    groups: vec![],
                    action: HookAction::Command {
                        command: "notify-send hi".into(),
                    },
                },
            ],
            shape: Shape::Ring,
            palette: Palette::Vivid,
            locale: Locale::En,
            group_meta: [(
                "builds".to_string(),
                GroupMeta {
                    name: Some("构建".into()),
                    order: Some(2),
                },
            )]
            .into_iter()
            .collect(),
            styles: [(
                "error".to_string(),
                StateStyle {
                    color: Some("#ff0000".into()),
                    shape: Some(Shape::Rounded),
                    blink_ms: Some(0),
                    ttl_ms: None,
                },
            )]
            .into_iter()
            .collect(),
            collapsed: true,
            show: SignFields {
                name: false,
                state: true,
                age: false,
                message: true,
            },
            sign_opacity: 40,
            hidden_groups: vec!["noisy".into()],
            blink_speed: 150,
            sound: false,
            quiet_hours: QuietHours {
                enabled: true,
                start: 23,
                end: 7,
            },
            muted_groups: vec!["quiet".into()],
            alert_after_secs: 30,
            tray_follows_state: false,
            group_by: GroupBy::Cwd,
            lan_access: true,
            // 面板也要设成非默认值 —— 它是这条往返里最容易漏的一个（`GET /` 的
            // 那个开关只在服务端读，前端不碰）
            web_panel: true,
            webhook_url: None,
            colors: BTreeMap::new(),
        };

        let json = serde_json::to_string_pretty(&settings).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back, settings, "导出再导入必须一模一样：\n{json}");
    }

    /// **老配置照旧**：`{"type":"http","url":"…"}` 里没有 `preset` / `headers`，
    /// 靠两条 `#[serde(default)]` 落到 `raw` + 空表。这是整个 R4 里最该盯住的一条
    /// —— 它要是坏了，所有已经配好转发的人在升级后都会静默变成「发一坨 JSON」。
    #[test]
    fn an_old_http_hook_without_preset_reads_as_raw() {
        let legacy: Hook =
            serde_json::from_str(r#"{"type":"http","url":"https://x/y","when":["error"]}"#)
                .unwrap();
        assert_eq!(
            legacy.action,
            HookAction::Http {
                url: "https://x/y".into(),
                headers: BTreeMap::new(),
                preset: HttpPreset::Raw,
            }
        );

        // 反过来：`raw` 写出去之后也还能读回来
        let wire = serde_json::to_string(&legacy.action).unwrap();
        assert!(wire.contains(r#""preset":"raw""#), "{wire}");
        assert_eq!(
            serde_json::from_str::<HookAction>(&wire).unwrap(),
            legacy.action
        );
    }

    /// 预设写错一个字母**不该毁掉整份设置**。
    ///
    /// `Settings` 是一个整体，任何一个字段解析失败都会让它整份退回默认值 ——
    /// 组显示名、配色、所有钩子一起没。手写配置时把 `"slack"` 写成 `"Slack"` 是
    /// 很正常的事，所以预设的解析是宽松的：认不出来就落到 `raw`（发 JSON），
    /// 而设置面板上那个下拉本身就写着「原样 JSON」。
    #[test]
    fn a_mistyped_preset_falls_back_to_raw_instead_of_losing_the_config() {
        let raw = r#"{
            "sign_w_mult": 1.4,
            "group_meta": { "builds": { "name": "构建" } },
            "hooks": [
                { "type": "http", "url": "https://x/y", "preset": "Slack" },
                { "type": "http", "url": "https://x/z", "preset": "webhook" },
                { "type": "http", "url": "https://api2.pushdeer.com/message/push?pushkey=K", "preset": "pushdeer" }
            ]
        }"#;
        let settings = serde_json::from_str::<Settings>(raw)
            .expect("不认识的预设不该让整份设置解析失败");
        assert_eq!(settings.sign_w_mult, 1.4, "别的设置一个都不该丢");
        assert_eq!(settings.group_meta["builds"].name.as_deref(), Some("构建"));
        assert_eq!(
            settings.hooks.len(),
            3,
            "钩子也不该被丢掉 —— 它们只是多发一坨 JSON"
        );
        // 大小写写错的那个能认出来，完全没见过的落到 raw
        assert_eq!(
            settings.hooks[0].action,
            HookAction::Http {
                url: "https://x/y".into(),
                headers: BTreeMap::new(),
                preset: HttpPreset::Slack,
            }
        );
        assert_eq!(settings.hooks[1].action.target(), "https://x/z");
        // **每个预设的名字都得认得出来**：认不出来的会静默落到 `raw`，于是
        // 「配了 PushDeer」变成「发了一坨 JSON 过去」—— 手机一声不响，而面板上
        // 那个下拉显示的却是「原样 JSON」，用户得自己发现。
        assert_eq!(
            settings.hooks[2].action,
            HookAction::Http {
                url: "https://api2.pushdeer.com/message/push?pushkey=K".into(),
                headers: BTreeMap::new(),
                preset: HttpPreset::Pushdeer,
            }
        );
    }

    /// 请求头的规整：去空白、丢掉空键、按上限截断（**确定的**截断）。
    #[test]
    fn extra_headers_are_trimmed_and_capped() {
        let mut headers = BTreeMap::new();
        headers.insert("  X-Token  ".to_string(), "  abc  ".to_string());
        headers.insert("   ".to_string(), "没人要的值".to_string());
        headers.insert("".to_string(), "也没人要".to_string());
        let clean = normalize_headers(headers);
        assert_eq!(
            clean,
            [("X-Token".to_string(), "abc".to_string())]
                .into_iter()
                .collect(),
            "键值都该去空白，空键那条该被丢掉"
        );

        // 条数上限：多出来的按键的字典序丢，丢哪几条是确定的
        let many: BTreeMap<String, String> = (0..MAX_HOOK_HEADERS + 5)
            .map(|index| (format!("X-H{index:03}"), "v".to_string()))
            .collect();
        let capped = normalize_headers(many);
        assert_eq!(capped.len(), MAX_HOOK_HEADERS);
        assert!(capped.contains_key("X-H000"), "前几个该留着");
        assert!(!capped.contains_key("X-H020"), "超出的该丢掉");

        // 经过 clamped() 也一样（手改的配置文件走的是这条路）
        let raw = r#"{"hooks":[{"type":"http","url":"https://x/y",
            "headers":{" A ":" 1 ","  ":""}}]}"#;
        let settings = serde_json::from_str::<Settings>(raw).unwrap().clamped();
        let HookAction::Http { headers, .. } = &settings.hooks[0].action else {
            panic!("应是一条 http 钩子");
        };
        assert_eq!(
            headers,
            &[("A".to_string(), "1".to_string())].into_iter().collect(),
            "clamped() 也要规整请求头"
        );
    }

    /// 转发摘要里的组名是**用户看到的那个**：用户改的名字 > 服务端默认名 > 原始组名。
    /// 和前端 `lampSort.ts::labelOf` 是同一条三级规则 —— 两处算出不同的名字，就是
    /// 「转发里写着 build-1、灯上写着构建」这种两处真值打架。
    #[test]
    fn display_label_follows_the_same_three_levels_as_the_lamp() {
        let group = |name: &str, default_label: Option<&str>| crate::state::GroupView {
            name: name.to_string(),
            default_label: default_label.map(str::to_string),
            aggregate: crate::state::Signal::Idle,
            message: None,
            since_ms: 0,
            stale_progress: None,
            sessions: Vec::new(),
        };

        let mut settings = Settings::default();
        // 三级里最低的一级：什么都没有就是原始组名
        assert_eq!(settings.display_label(&group("build-1", None)), "build-1");
        // 中间那一级：会话分组出来的组有 default_label
        assert_eq!(
            settings.display_label(&group("0191f0aa", Some("proj · claude"))),
            "proj · claude"
        );
        // 最高一级：用户起的名字压过 default_label
        settings.group_meta.insert(
            "0191f0aa".to_string(),
            GroupMeta {
                name: Some("构建".into()),
                order: None,
            },
        );
        assert_eq!(
            settings.display_label(&group("0191f0aa", Some("proj · claude"))),
            "构建"
        );
    }

    /// 导入一个坏文件要**报错**（而不是静默套用默认值）。
    ///
    /// 静默退回默认值在「启动时读配置」那条路上是对的（总得起得来），但在
    /// 「用户主动点导入」这条路上是错的 —— 用户以为导进去了，其实什么都没变。
    #[test]
    fn importing_a_broken_file_is_an_error_not_a_silent_default() {
        let parsed = serde_json::from_str::<Settings>("{ 这不是 json");
        assert!(parsed.is_err(), "坏文件必须报错");
    }

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
            action: HookAction::http(url),
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
            HookAction::http("https://ntfy.sh/x")
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
        let settings = Settings {
            webhook_url: Some("https://x/y".into()),
            ..Settings::default()
        };
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
        assert_eq!(settings.resolve_group(None, Some("/proj/api"), "s-1"), None);
    }

    /// 上报自己带的 `group` **优先**，`group_by` 只是兜底而不是覆盖。
    /// 顺序反了就等于「开了按目录分组之后手写 group 全失效」。
    #[test]
    fn an_explicit_group_beats_the_cwd_fallback() {
        let settings = Settings { group_by: GroupBy::Cwd, ..Settings::default() };
        assert_eq!(
            settings.resolve_group(Some("work"), Some("/proj/api"), "s-1").as_deref(),
            Some("work")
        );
    }

    #[test]
    fn cwd_mode_falls_back_to_the_full_path() {
        let settings = Settings { group_by: GroupBy::Cwd, ..Settings::default() };

        // **完整路径**，不是最后一段目录名：`~/a/api` 和 `~/b/api` 撞名会让
        // 两个项目的会话混进同一块牌。
        assert_eq!(
            settings.resolve_group(None, Some("/Users/me/code/api"), "s-1").as_deref(),
            Some("/Users/me/code/api")
        );
        // 末尾分隔符要去掉，否则 `/proj` 和 `/proj/` 会变成两个组
        assert_eq!(
            settings.resolve_group(None, Some("/Users/me/code/api/"), "s-1").as_deref(),
            Some("/Users/me/code/api")
        );
        assert_eq!(settings.resolve_group(None, Some("  /proj  "), "s-1").as_deref(), Some("/proj"));

        // 没给 cwd 就退回 default，而不是造一个空名字的组
        assert_eq!(settings.resolve_group(None, None, "s-1"), None);
        // 空串和空白串都不算「给了一个组」
        assert_eq!(settings.resolve_group(Some("   "), Some("/proj"), "s-1").as_deref(), Some("/proj"));
        // 根目录去掉分隔符只剩空串 —— 那不是个能用的组名
        assert_eq!(settings.resolve_group(None, Some("/"), "s-1"), None);
    }

    /// 取路径最后一段。两种分隔符都认，免得 Windows 上写的路径在别处解析错。
    ///
    /// 通知正文和会话分组的默认显示名（「目录末段 · 智能体」）共用它，
    /// 所以这条同时挡住「两处各写一遍、然后分叉」。
    #[test]
    fn project_name_is_the_last_path_segment() {
        assert_eq!(project_name("/Users/me/proj"), "proj");
        assert_eq!(project_name("/Users/me/proj/"), "proj");
        assert_eq!(project_name(r"C:\work\api"), "api");
        assert_eq!(project_name("proj"), "proj");
    }

    /// 按会话分组：**一个会话一块牌**。
    ///
    /// 这是「同一个智能体并行开了几个会话、进度各不相同」那个场景的解：只按目录
    /// 分组还是会把它们挤在同一块牌上，只能看到最紧急的那一个。
    #[test]
    fn session_mode_gives_every_session_its_own_sign() {
        let settings = Settings {
            group_by: GroupBy::Session,
            ..Settings::default()
        };

        assert_eq!(
            settings.resolve_group(None, Some("/proj"), "abc-123").as_deref(),
            Some("abc-123")
        );

        // **同一个目录里的两个会话必须分开** —— 这正是这个模式存在的理由，
        // 所以用 `assert_ne!` 直接钉这一点，而不是只断言各自等于自己的 id
        assert_ne!(
            settings.resolve_group(None, Some("/proj"), "abc-123"),
            settings.resolve_group(None, Some("/proj"), "def-456"),
            "同目录的两个会话被分到同一块牌了"
        );

        // 显式 `group` 仍然优先 —— 「上报自己带的优先」这条对所有模式一视同仁
        assert_eq!(
            settings.resolve_group(Some("work"), None, "abc-123").as_deref(),
            Some("work")
        );

        // 没有 cwd 也照样能分（会话 id 总是有的，不像 cwd）
        assert_eq!(
            settings.resolve_group(None, None, "abc-123").as_deref(),
            Some("abc-123")
        );
    }

    // ---- 界面语言（CLI 也用它） ------------------------------------------------

    /// POSIX 的 locale 写法要能翻译成 `Lang::parse` 认得的那种。
    ///
    /// 这一条是**实测踩到的**：`LANG=en_US.UTF-8` 一开始换不出英文，因为
    /// `Lang::parse` 认的是 `navigator.language` 那一套（`en-US`），而环境变量给的是
    /// 带编码、带下划线的 POSIX 写法。少了这层归一，CLI 在几乎所有 Unix 上都会
    /// 退回中文 —— 而那正是「看起来做了、其实没生效」。
    #[test]
    fn posix_locale_spellings_are_normalized() {
        for (raw, want) in [
            ("en_US.UTF-8", "en-US"),
            ("zh_CN.UTF-8", "zh-CN"),
            ("en_US", "en-US"),
            ("en", "en"),
            ("C", "C"),
            ("", ""),
            // `@` 是修饰符，和编码一样该丢掉
            ("sr_RS@latin", "sr-RS"),
        ] {
            assert_eq!(normalize_posix_locale(raw), want, "{raw:?}");
        }

        // 归一之后要真的能被认出来
        assert_eq!(Lang::parse(&normalize_posix_locale("en_US.UTF-8")), Some(Lang::En));
        assert_eq!(Lang::parse(&normalize_posix_locale("zh_CN.UTF-8")), Some(Lang::Zh));
        // `C` / `POSIX` 表示「没有指定 locale」，认不出来是对的 —— 调用方会继续往下试
        assert_eq!(Lang::parse(&normalize_posix_locale("C")), None);
    }

    /// **显式偏好最权威**，排在 `lang` 文件和环境变量前面。
    ///
    /// 这条不走文件系统：`Locale::Zh` / `Locale::En` 两个分支直接返回，只有 `Auto`
    /// 才会去读文件。所以这里能确定性地断言 —— 而「用户明确选过」优先于「上次 app
    /// 解析出来的」，正是为了让手改 `config.json` 立刻生效。
    #[test]
    fn an_explicit_locale_beats_everything_else() {
        let zh = Settings { locale: Locale::Zh, ..Settings::default() };
        let en = Settings { locale: Locale::En, ..Settings::default() };
        assert_eq!(resolve_lang(&zh), Lang::Zh);
        assert_eq!(resolve_lang(&en), Lang::En);
    }

    /// `group_by` 是设置里的一个值，所以它必须能**原样存取** —— 值名字写错了
    /// （比如 serde 那边写成 `by_session`）会让开关打开后静默退回不分组。
    #[test]
    fn group_by_round_trips_through_json() {        for (mode, wire) in [
            (GroupBy::None, "none"),
            (GroupBy::Cwd, "cwd"),
            (GroupBy::Session, "session"),
        ] {
            let json = serde_json::to_string(&mode).unwrap();
            assert_eq!(json, format!("\"{wire}\""), "{mode:?} 的线上名字变了");
            assert_eq!(serde_json::from_str::<GroupBy>(&json).unwrap(), mode);
        }
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
        let one_line = sign_height(settings.size, false, 1.0);
        let one = settings.window_size(&[one_line]);

        // 宽度不随牌数变 —— 牌是竖着叠的，不是横着排
        let two = settings.window_size(&[one_line, one_line]);
        assert_eq!(two.0, one.0, "宽度固定");
        assert!(two.1 > one.1, "多一块牌就多占一份高度");

        // 有消息的那块多一行，于是窗口更高 —— 那是刻意的提示
        let with_message = settings.window_size(&[sign_height(settings.size, true, 1.0)]);
        assert!(with_message.1 > one.1, "有消息的牌多占一行");

        // 超出上限按上限算，不会无限往下长
        let many = vec![one_line; MAX_SIGNS + 3];
        let capped = vec![one_line; MAX_SIGNS];
        assert_eq!(settings.window_size(&many), settings.window_size(&capped));
    }

    #[test]
    fn a_sign_is_wider_than_tall_and_scales_with_size() {
        for size in [16, 22, 32] {
            let width = sign_width(size, 1.0);
            let height = sign_height(size, false, 1.0);
            assert!(width > height, "牌是横条：{size} → {width}×{height}");
            assert_eq!(sign_height(size, true, 1.0), height * 5 / 3, "有消息时约 1.67 倍高");
        }
    }

    /// 倍率在 1.0 时必须**逐字**等于旧公式 —— 默认值不许无声改变既有行为（红线 3）。
    /// 它一旦不是 1.0，就该真的把牌放大 / 缩小。
    #[test]
    fn sign_multipliers_scale_the_sign_and_are_identity_at_one() {
        for size in [12u16, 22, 33, 64] {
            assert_eq!(sign_width(size, 1.0), size * 7, "宽 {size}");
            assert_eq!(sign_height(size, false, 1.0), size * 3 / 2, "单行高 {size}");
            assert_eq!(sign_height(size, true, 1.0), size * 5 / 2, "双行高 {size}");
        }

        let size = 22;
        // 放大：确实更大，且随倍率单调
        assert!(sign_width(size, 1.5) > sign_width(size, 1.0));
        assert!(sign_width(size, 2.0) > sign_width(size, 1.5));
        assert!(sign_height(size, false, 1.2) > sign_height(size, false, 1.0));
        assert!(sign_height(size, true, 1.2) > sign_height(size, true, 1.0));
        // 缩小也认
        assert!(sign_width(size, 0.8) < sign_width(size, 1.0));

        // **向上取整**：22*7*1.3 = 200.2 → 201（不是 200）。差这一下，CSS 那边
        // 200.2px 的牌就会被 `overflow: hidden` 裁掉右边缘一线。
        assert_eq!(sign_width(22, 1.3), 201);
    }

    /// 手改配置 / 前端都可能塞进越界的倍率，`clamped()` 必须夹住它们。
    #[test]
    fn clamped_pulls_the_sign_multipliers_into_range() {
        let wild = Settings {
            sign_w_mult: 99.0,
            sign_h_mult: 0.01,
            ..Settings::default()
        };
        let clamped = wild.clamped();
        assert_eq!(clamped.sign_w_mult, MAX_SIGN_MULT);
        assert_eq!(clamped.sign_h_mult, MIN_SIGN_MULT);

        // NaN / 无穷必须被挡掉：NaN 一路传到 `ceil` 会变成 0，把牌宽归零。
        let broken = Settings {
            sign_w_mult: f32::NAN,
            sign_h_mult: f32::INFINITY,
            ..Settings::default()
        };
        let clamped = broken.clamped();
        assert_eq!(clamped.sign_w_mult, 1.0, "NaN 要退回 1.0，不能令牌宽归零");
        assert_eq!(clamped.sign_h_mult, 1.0, "无穷一律退回 1.0");
        assert!(sign_width(22, clamped.sign_w_mult) > 0);
    }

    /// 旧配置（没有这两个键）必须读成倍率 1.0 —— 也就是完全不变的行为。
    /// 这是 `serde(default)` 那条契约的落点：升级不该让任何人的灯变形。
    #[test]
    fn settings_without_the_sign_multipliers_read_as_one() {
        let json = serde_json::to_string(&Settings::default()).unwrap();
        assert!(json.contains(r#""sign_w_mult":1.0,"#), "{json}");

        // 把两个键整个删掉，模拟旧版本写出来的文件
        let legacy = json
            .replace(r#""sign_w_mult":1.0,"#, "")
            .replace(r#""sign_h_mult":1.0,"#, "");
        let parsed: Settings = serde_json::from_str(&legacy).unwrap();
        assert_eq!(parsed.sign_w_mult, 1.0);
        assert_eq!(parsed.sign_h_mult, 1.0);
    }

    /// 旧配置（没有 `lamp_layout`）必须读成单灯 —— 新能力默认关（红线 3）。
    #[test]
    fn settings_without_lamp_layout_read_as_single() {
        let json = serde_json::to_string(&Settings::default()).unwrap();
        assert!(json.contains(r#""lamp_layout":"single""#), "{json}");

        let legacy = json.replace(r#""lamp_layout":"single","#, "");
        let parsed: Settings = serde_json::from_str(&legacy).unwrap();
        assert_eq!(parsed.lamp_layout, LampLayout::Single);
    }

    /// 旧配置（没有 `snap_to_edge`）必须读成「不吸附」—— 新能力默认关（红线 3）。
    #[test]
    fn settings_without_snap_to_edge_read_as_off() {
        let json = serde_json::to_string(&Settings::default()).unwrap();
        assert!(json.contains(r#""snap_to_edge":false"#), "{json}");

        let legacy = json.replace(r#""snap_to_edge":false,"#, "");
        let parsed: Settings = serde_json::from_str(&legacy).unwrap();
        assert!(!parsed.snap_to_edge);
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

    /// 每一枚角标都占一个点的位置 —— 忘了算哪一枚，它就会落到窗口外面，
    /// 而折叠态的意义正是让出地方。
    #[test]
    fn every_badge_takes_a_dot_of_its_own() {
        let size = Settings::default().size;

        let (bare, _) = collapsed_window_size(size, 2, 0);
        let (flagged, _) = collapsed_window_size(size, 2, 1);
        let (both, _) = collapsed_window_size(size, 2, 2);
        assert_eq!(flagged - bare, size + size / 3, "多一个点加一个点距");
        assert_eq!(both - flagged, size + size / 3, "两枚就是两个点位");

        // 上限仍然要封住，否则一个算错的调用点会让窗口无限变宽
        let (crazy, _) = collapsed_window_size(size, MAX_DOTS, 9);
        let (capped, _) = collapsed_window_size(size, MAX_DOTS, MAX_BADGES);
        assert_eq!(crazy, capped, "角标再多也只按 MAX_BADGES 算");

        // 角标上限是**单独**封的，不侵占组的份额：组数按 MAX_DOTS 截断，
        // 角标再占位，两者相加才是最终的点数。
        let (full, _) = collapsed_window_size(size, MAX_DOTS, 0);
        let (full_badged, _) = collapsed_window_size(size, MAX_DOTS, MAX_BADGES);
        assert_eq!(
            full_badged - full,
            MAX_BADGES as u16 * (size + size / 3),
            "满屏也不该挤掉角标"
        );
    }

    /// `+N` 角标出现的时机。**前端按同一条件画**（`SignalLamp.tsx` 里的
    /// `hidden.length > 0`），所以这条同时也是两边契约的锚。
    #[test]
    fn the_more_badge_appears_only_past_the_dot_limit() {
        assert_eq!(more_badge(0), 0, "一个组都没有时没有可藏的");
        assert_eq!(more_badge(MAX_DOTS), 0, "刚好放得下，一枚都不藏");
        assert_eq!(more_badge(MAX_DOTS + 1), 1, "藏了一个就得说");
        assert_eq!(more_badge(MAX_DOTS + 10), 1, "藏得再多也只有一枚角标");
    }

    /// 折叠态必须比展开态小 —— 「折叠」的意义就是让出地方
    #[test]
    fn collapsed_is_much_smaller_than_expanded() {
        let settings = Settings::default();
        let (cw, ch) = collapsed_window_size(settings.size, 1, 0);
        let (ew, eh) = settings.window_size(&[sign_height(settings.size, false, 1.0)]);
        assert!(cw < ew, "折叠态更窄: {cw} vs {ew}");
        assert!(ch < eh, "折叠态更矮: {ch} vs {eh}");
    }
}
