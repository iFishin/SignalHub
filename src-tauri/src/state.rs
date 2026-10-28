use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{HashMap, VecDeque};
use std::str::FromStr;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::config::Settings;

/// 会话静默超时后转入 `stale`（而不是直接消失）并再保留多久。
///
/// 这一段存在的意义是把「智能体不说话了」和「智能体空闲待命」分开显示。
/// 直接删掉的话，智能体崩溃/被强杀（SessionEnd 没触发）会表现为灯保持原状
/// 直到 TTL 到期，用户看不出区别。
///
/// **从 5 分钟放宽到 30 分钟**：5 分钟太短了 —— 一个正在跑长任务的智能体只要
/// 超过这个时间没有事件，就会从灯上彻底消失，用户看到的是「没有智能体上报」，
/// 而它其实还在干活。`stale` 本身已经是「它不说话了」这个信息，没必要那么快
/// 升级成「它不存在」。半小时足够人从别的窗口切回来看到它。
///
/// ⚠️ 细环（`stale_progress`）画的就是这一段的剩余比例，所以这个值同时决定了
/// 环缩短得多慢。
const STALE_GRACE: Duration = Duration::from_secs(30 * 60);

/// 活着但没上报的会话多久复查一次进程。到点只续期、不改变可见状态 ——
/// 进程死没死由 [`Store::refresh_liveness`] 每一跳的探活决定。
const LIVENESS_RECHECK: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Signal {
    /// 一个会话都没有。派生状态，不可上报。
    Offline,
    /// 监控被全局暂停。派生状态，不可上报。
    Off,
    /// 会话静默超过 TTL。派生状态，不可上报。
    Stale,
    Idle,
    Done,
    Thinking,
    Working,
    Attention,
    Permission,
    Error,
}

impl Signal {
    /// 别名表只存在于这一处，CLI 与 HTTP 共用（见下面的 Deserialize 实现）。
    ///
    /// `stale` / `offline` / `off` 之外的派生状态**不在**这里，因此无法被上报：
    /// 它们是聚合出来的结果，不是外部程序该声称的状态。
    pub fn parse(s: &str) -> Result<Self, String> {
        let key = s.trim().to_ascii_lowercase().replace('-', "_");
        Ok(match key.as_str() {
            "off" | "pause" | "paused" | "disabled" => Signal::Off,
            "idle" | "ready" | "empty" => Signal::Idle,
            "done" | "completed" | "complete" | "success" | "finished" => Signal::Done,
            "thinking" | "reasoning" | "processing" => Signal::Thinking,
            "working" | "tool_done" | "tool_use" | "active" | "running" => Signal::Working,
            "attention" | "notification" | "needs_attention" => Signal::Attention,
            "permission" | "permission_request" | "needs_permission" => Signal::Permission,
            "error" | "blocked" | "fail" | "failure" | "stuck" => Signal::Error,
            "stale" | "offline" => {
                return Err(format!(
                    "{key:?} 是聚合派生出来的状态，不能上报；\
                     可上报的状态: idle thinking working done attention permission error off"
                ));
            }
            other => {
                return Err(format!(
                    "unknown state {other:?}；\
                     可上报的状态: idle thinking working done attention permission error off"
                ));
            }
        })
    }

    /// 聚合时取 max。数值本身无意义，只有相对顺序有意义。
    ///
    /// `stale` 刻意排在 `idle` 之下：一路会话在报 idle（活着）时，不该被另一路
    /// 已经沉默的会话盖成灰的。
    pub fn priority(self) -> u8 {
        match self {
            Signal::Offline | Signal::Off => 0,
            Signal::Stale => 1,
            Signal::Idle => 2,
            Signal::Done => 6,
            Signal::Thinking => 15,
            Signal::Working => 20,
            Signal::Attention => 30,
            Signal::Permission => 40,
            Signal::Error => 50,
        }
    }

    /// 规范的小写状态名。与 `serde` 序列化出来的字符串一致 ——
    /// 有单测盯着这一点，免得两处漂移（钩子的 `when` 过滤、配置里的颜色键
    /// 都按这个名字匹配）。
    pub fn name(self) -> &'static str {
        match self {
            Signal::Offline => "offline",
            Signal::Off => "off",
            Signal::Stale => "stale",
            Signal::Idle => "idle",
            Signal::Done => "done",
            Signal::Thinking => "thinking",
            Signal::Working => "working",
            Signal::Attention => "attention",
            Signal::Permission => "permission",
            Signal::Error => "error",
        }
    }

    /// 全部状态名，含派生状态。
    ///
    /// `parse` 刻意**不接受** `stale` / `offline`（它们是聚合产物，不能被上报），
    /// 但用户配置颜色时需要能指定它们。所以校验配置用这个，不用 `parse`。
    pub const ALL_NAMES: [&'static str; 10] = [
        "offline", "off", "stale", "idle", "done", "thinking", "working",
        "attention", "permission", "error",
    ];

    pub fn is_known_name(name: &str) -> bool {
        Self::ALL_NAMES.contains(&name)
    }

    /// `name()` 的反向，**接受全部状态名**（含派生状态）。
    ///
    /// 和 `parse` 的区别正是这一点：`parse` 是给**外部上报**用的，刻意拒绝
    /// `stale` / `offline`（它们是聚合产物，不该被外部声称）；这个读的是
    /// **自己落盘的数据** —— 统计里那两个恰恰要有，灯灰着的那段时间也是时间。
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "offline" => Signal::Offline,
            "off" => Signal::Off,
            "stale" => Signal::Stale,
            "idle" => Signal::Idle,
            "done" => Signal::Done,
            "thinking" => Signal::Thinking,
            "working" => Signal::Working,
            "attention" => Signal::Attention,
            "permission" => Signal::Permission,
            "error" => Signal::Error,
            _ => return None,
        })
    }

    /// 静默多久后该状态视为过期。
    ///
    /// 现在**每个状态都有 TTL**（早先 `idle` 没有，导致一个忘关的会话能一直
    /// 显示「空闲」长达半小时）。工作类状态的窗口刻意很短：干活时每次工具调用
    /// 都会触发 hook，静默 60–90 秒基本可以断定不在工作了。这决定了「智能体
    /// 消失后灯还会错多久」。
    pub fn default_ttl(self) -> Duration {
        match self {
            // 这两个永远不会成为会话状态（Off 在 apply 里转成 clear，
            // Offline 是聚合产物），取值无实际意义。
            Signal::Off | Signal::Offline => Duration::from_secs(600),
            Signal::Stale => STALE_GRACE,
            Signal::Idle => Duration::from_secs(600),
            Signal::Done => Duration::from_secs(30),
            // 「推理中」和「执行中」的静默期要**给得很宽**。
            //
            // 钩子挂在**事件**上（`UserPromptSubmit` → thinking、`PreToolUse` →
            // working…），而模型思考、或者跑一个没有工具调用的长回复时，**一个
            // 事件都不触发**。早先这两个分别是 60s / 90s，于是一次超过一分半的
            // 思考就会让灯显示「沉默」，再过五分钟干脆变成「没有智能体上报」
            // —— 智能体明明在干活，灯却当它没了。
            //
            // 放宽的代价是「崩溃的智能体」晚一点才被发现。但那时它是转成 `stale`
            // （「沉默」）而不是消失，用户仍然看得到它 —— 这比「活着的智能体看着
            // 像死了」轻得多。
            Signal::Thinking => Duration::from_secs(600),
            Signal::Working => Duration::from_secs(300),
            Signal::Attention => Duration::from_secs(300),
            Signal::Permission => Duration::from_secs(600),
            Signal::Error => Duration::from_secs(300),
        }
    }
}

impl FromStr for Signal {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Signal::parse(s)
    }
}

// 手写 Deserialize 而非逐变体 `#[serde(alias = ...)]`，这样别名表只存在于
// Signal::parse 一处，CLI 与 HTTP 不会各自漂移。
impl<'de> Deserialize<'de> for Signal {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Signal::parse(&String::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

/// 消息流最多记多少条。
///
/// 一轮工具调用就能产生十几条不同的消息，200 条约等于十几轮 —— 够回答「刚才到底
/// 发生了什么」，又不会让一个常驻应用无限长胖。
const MESSAGE_HISTORY: usize = 200;

/// 会话表的上限。**正常用法的会话数是个位数** —— 触到上限基本只有一种可能：
/// 上报方在造（每次运行换一个 session id）。到顶时丢「最快过期」的那条。
const MAX_SESSIONS: usize = 512;

/// 一条消息最多留多少字符。**在入口处截断**：它会进灯、通知、历史流，
/// 而 HTTP body 本身可以到 2 MB。
const MAX_MESSAGE_CHARS: usize = 200;

/// 一条上报过的消息。
///
/// 灯上只显示每个会话**当前**那条；这个流是用来回答「刚才发生了什么」的 ——
/// 「十分钟前灯为什么红了」这种事，当前状态答不了。
#[derive(Debug, Clone, Serialize)]
pub struct MessageEntry {
    /// epoch 毫秒
    pub at_ms: u64,
    pub session: String,
    pub group: String,
    /// 上报时的状态
    pub state: Signal,
    pub message: String,
}

/// 这次上报的状态该活多久。
///
/// **三级优先**：上报方显式给的 `ttl_ms`（CLI 的 `--ttl`、HTTP 字段）>
/// 用户在「逐个状态覆盖」里为这个状态设的 `ttl_ms` > 内置默认（`Signal::default_ttl`）。
///
/// 用户设的为什么排在显式之后：显式值来自**那一次调用**（`signalhub report --ttl 30s`、
/// 包装器心跳、测试上报），它比「我一般想让工作态活 5 分钟」更具体。反过来就是
/// 「设置里改过之后 `--ttl` 再也不生效」—— 那是个很难查的坑。
///
/// 它只在**上报进来的那一刻**算一次：已存在的会话不会被追溯（`expires_at` 是上报时
/// 写死的）。追溯会让「刚才还亮着的牌突然消失」，而它消失的理由（你改了配置）在灯上
/// 根本看不见。
pub fn state_ttl(state: Signal, explicit_ms: Option<u64>, settings: &Settings) -> Duration {
    if let Some(ms) = explicit_ms {
        return Duration::from_millis(ms);
    }
    settings
        .styles
        .get(state.name())
        .and_then(|style| style.ttl_ms)
        .map(|ms| Duration::from_millis(u64::from(ms)))
        .unwrap_or_else(|| state.default_ttl())
}

/// 一次状态上报。HTTP 请求体与 CLI 参数都归一化成它。
#[derive(Debug, Clone, Deserialize)]
pub struct Report {
    pub state: Signal,
    pub session: Option<String>,
    /// 归属的组。不传就是 `default` —— 单组用户完全不用管这个字段。
    ///
    /// 分组是为了「多组状态灯」：灯球上每组一块牌，各自聚合。
    pub group: Option<String>,
    pub agent: Option<String>,
    /// 上报方声明的**信号源类型**（`agent` / `system` / `custom`），缺省 `agent`。
    ///
    /// **只影响「接入」页怎么分组**，不参与分组聚合、TTL、通知、托盘任何决策 ——
    /// 它是纯粹的**描述**，所以缺省值选什么都改不了灯的行为。
    ///
    /// 存字符串而不是枚举是刻意的：多一个取值不该让老版本 app 解析失败（服务端没有
    /// `deny_unknown_fields`），而认不出来的一律落到「自定义」那一组。
    pub source: Option<String>,
    pub cwd: Option<String>,
    pub message: Option<String>,
    pub ttl_ms: Option<u64>,
    /// 上报方（智能体）的进程号。**可选，只有内置集成会带。**
    ///
    /// 用途只有一个：TTL 到期时区分「只是没上报」和「进程真的没了」——
    /// 前者转成 `idle` 留在灯上，后者立刻清掉。不带的会话完全按老路走
    /// （到期转 `stale`）。
    ///
    /// **只认 Unix**（见 `effective_pid`）；Windows 上会被丢掉，行为与没带一样。
    pub pid: Option<u32>,
}

impl Report {
    /// 只给单测用的紧凑构造器；生产路径上的 Report 都来自反序列化。
    #[cfg(test)]
    pub fn new(state: Signal) -> Self {
        Report {
            state,
            session: None,
            group: None,
            agent: None,
            source: None,
            cwd: None,
            message: None,
            ttl_ms: None,
            pid: None,
        }
    }

    pub fn session_id(&self) -> &str {
        self.session.as_deref().unwrap_or("default")
    }

    pub fn group_id(&self) -> &str {
        self.group.as_deref().map(str::trim).filter(|g| !g.is_empty()).unwrap_or(DEFAULT_GROUP)
    }

    /// 声明的那一类，缺省 [`DEFAULT_SOURCE`]。
    ///
    /// 缺省给 `agent` 而不是 `custom`：**老版本 CLI 和手写 curl 都不带这个字段**，
    /// 而它们报的多半就是某个智能体（老 CLI 装的 hook 正是这么报的）。反过来，
    /// 用户自己的脚本要落到「自定义」那一组，由 CLI 显式声明（`--source` 或按
    /// `--agent` 推出来的缺省），不靠这里猜。
    pub fn source_id(&self) -> &str {
        self.source
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_SOURCE)
    }
}

/// 没指定 `source` 时的类型。见 [`Report::source_id`]。
pub const DEFAULT_SOURCE: &str = "agent";

/// 进程号只在 Unix 上认（见 [`Report::pid`]）。
///
/// 非 Unix 上直接丢掉，而不是存下来又不去探活：那样一个已经死掉的进程号会
/// 永远有值，`refresh_liveness` 也没法判断，最后只能变成「假空闲」。
fn effective_pid(pid: Option<u32>) -> Option<u32> {
    // 0 在 `kill(0, …)` 里表示「当前进程组」，不是一个进程号。存下来只会让
    // 这个会话在下一拍被当成「进程没了」直接清掉 —— 当作没给更诚实。
    let pid = pid.filter(|pid| *pid > 0);
    if cfg!(unix) { pid } else { None }
}

/// 没指定组时的组名。
pub const DEFAULT_GROUP: &str = "default";

#[derive(Debug, Clone)]
struct Session {
    /// 冗余存一份 id，让聚合平局时的排序确定（HashMap 迭代顺序不确定）。
    id: String,
    /// 归属的组。灯球上每组一块牌。
    group: String,
    state: Signal,
    agent: Option<String>,
    cwd: Option<String>,
    message: Option<String>,
    /// 单调时钟，用于 TTL 判定。每个状态都有 TTL，所以这里必定有值。
    expires_at: Instant,
    /// 进入当前状态的时间，用于 UI 的「已持续多久」。
    /// 转入 `stale` 时刻意不改它，这样它读起来就是「沉默多久了」。
    since: SystemTime,
    /// 上报方进程号。**没有就用不上探活**，到期照旧转 `stale`。
    /// 见 [`Report::pid`] 与 [`Store::refresh_liveness`]。
    pid: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionView {
    pub id: String,
    pub state: Signal,
    pub agent: Option<String>,
    pub cwd: Option<String>,
    pub message: Option<String>,
    pub age_ms: u64,
    /// 仅 `stale` 会话有值：距被删除还剩多少宽限期，1.0 = 刚转 stale，0.0 = 即将消失。
    ///
    /// 由服务端算好给前端画细环。不在前端算是因为那要求前端也复刻一遍 TTL 常量
    /// —— 而 `age_ms` 是「距上次状态变化」，并不等于「沉默多久」。
    pub stale_progress: Option<f32>,
}

/// 一个组在灯球上的样子：一块牌。
#[derive(Debug, Clone, Serialize)]
pub struct GroupView {
    pub name: String,
    /// 默认显示名。**只在「按会话分组」出来的组上有值** —— 那种组的组名是一个
    /// 会话 id（多半是 UUID），直接画在牌上没法看。这里给出「目录末段 · 智能体」，
    /// 一眼能认出是哪个窗口、在哪个项目里。见 [`session_label`]。
    ///
    /// **只是默认**：用户在「外观 → 各组的显示」里改的名字优先于它（见
    /// `lampSort.ts` 的 `labelOf`）。而隐藏 / 静音 / 通知动作的目标匹配的仍然是
    /// 原始的 `name` —— 和自定义显示名一样，这个字段不参与任何匹配。
    ///
    /// `None` = 这个组没有更好读的名字，前端退回 `name`。
    pub default_label: Option<String>,
    /// 这一组自己的聚合（组内取最高优先级）
    pub aggregate: Signal,
    /// 这一组里决定聚合的那个会话的说明，用作牌上的消息行
    pub message: Option<String>,
    /// 这一组里最早进入当前状态的时间，用于牌上的「已持续多久」
    pub since_ms: u64,
    /// 这一组沉默到哪一步了，灯上画细环用。
    ///
    /// **取的是决定聚合那个会话的**，不是组内第一个：那颗圆点正是因为那个会话
    /// 才变成 `stale`，要缩短的自然也是它。组内没有会话时是 `None`（那一组的
    /// 聚合是 `offline` 而不是 `stale`，画不出环来）。
    pub stale_progress: Option<f32>,
    pub sessions: Vec<SessionView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    /// 所有组里最高的那个 —— 通知、钩子、托盘都用它，与分组无关
    pub aggregate: Signal,
    pub paused: bool,
    pub since_ms: u64,
    pub message: Option<String>,
    /// 每组的聚合结果，按组名排序（顺序稳定，牌不会跳）
    pub groups: Vec<GroupView>,
}

impl Snapshot {
    /// 摊平所有组里的会话。通知文案、会话列表这类「与分组无关」的用途走它。
    pub fn all_sessions(&self) -> impl Iterator<Item = &SessionView> {
        self.groups.iter().flat_map(|group| group.sessions.iter())
    }
}

/// 前端真正会渲染的那部分。用它做前后比对，避免「message 变了但聚合没变」
/// 这类更新被当成无变化而丢掉。
///
/// 只放**离散**的东西。静默进度是个连续的 0.0–1.0，直接放进来会让每次快照都
/// 「有变化」（`now` 每 tick 都在动），于是每秒推几十次、灯球窗口跟着抖。
/// 放的是它的**档位**，见 `progress_step`。
#[derive(Debug, PartialEq)]
struct Visible {
    aggregate: Signal,
    paused: bool,
    message: Option<String>,
    /// 带上组名 —— 一个会话换组会改变牌的分组，那是可见变化
    sessions: Vec<(String, String, Signal)>,
    /// 每个会话的静默进度档位。跨档才算变化 —— 理由见上面那段。
    progress: Vec<(String, u8)>,
}

#[derive(Debug, Default)]
pub struct Store {
    sessions: HashMap<String, Session>,
    paused: bool,
    /// 聚合值最后一次发生变化的时间
    since: Option<SystemTime>,
    /// 最近上报过的消息，从旧到新。满了丢最旧的。
    messages: VecDeque<MessageEntry>,
    /// 上一次「前端收到的那一帧」长什么样。
    ///
    /// **必须有这份基线**，否则细环动不起来：`expire` 的一次调用里前后两个瞬间是
    /// 同一个 `now`，进度档位当然一样，于是永远比不出「跨档」—— 而 `sweep` 每
    /// 250ms 调一次 `expire`、期间什么都不改，环就停在刚转 `stale` 那一刻的长度上，
    /// 直到会话被删掉。有了基线，`expire` 拿「现在」和「上次推出去的那一帧」比，
    /// 跨档才被发现。
    last_pushed: Option<Visible>,
}

impl Store {
    /// 记一次上报。
    ///
    /// `ttl` 由调用方算好（`state_ttl`），不在这里读设置：生产路径上设置是**另一个
    /// 锁**（`SharedSettings`），在这个已经持着状态锁的地方再去拿它会多一对锁序，
    /// 而算它只需要一次不涉及状态的纯计算。
    pub fn apply(&mut self, now: Instant, req: Report, ttl: Duration) -> bool {
        // 先取出会话名和组名，之后就只移动 req 的各个字段，不再整体借用它
        let session = req.session_id().to_string();
        let group = req.group_id().to_string();
        // `off` 作用在会话上等价于清除该会话；全局暂停走 set_paused。
        //
        // `now` 顺手传下去：清理路径也要做前后比对，而比对包含静默进度档位。
        if req.state == Signal::Off {
            return self.clear(&session, now);
        }

        let prev = self.sessions.get(&session);

        // agent / cwd 是身份信息，沿用上一次；message 是事件细节，每次替换。
        let agent = req.agent.or_else(|| prev.and_then(|s| s.agent.clone()));
        let cwd = req.cwd.or_else(|| prev.and_then(|s| s.cwd.clone()));
        // 进程号只在**这次带了**的时候更新：HTTP 审批钩子、手写 curl 都不带它，
        // 那些上报不该把已经记住的进程号抹掉。
        let pid = effective_pid(req.pid).or_else(|| prev.and_then(|s| s.pid));
        let since = match prev {
            Some(p) if p.state == req.state => p.since,
            _ => SystemTime::now(),
        };
        let expires_at = now + ttl;

        // 消息流只在**消息真的变了**时记一笔。一轮工具调用里同一条说明会被反复
        // 上报（每次 PreToolUse 都带同一个 message），每次都记就成了噪音。
        //
        // 在 insert 之前算好：`prev` 是 `self.sessions` 的不可变借用，而下面那次
        // insert 要可变借用，两者不能重叠。
        let state = req.state;
        // **消息在入口处截断**，不指望上报方自觉：`/v1/state` 的 body 可以到 2 MB，
        // 而这条会上灯、进通知、进 200 条的历史流 —— 一个把文件内容当 message
        // 发的脚本就能把内存撑到几百 MB（issue #16）。
        let message = req
            .message
            .as_deref()
            .map(|text| crate::hookmap::clip(text, MAX_MESSAGE_CHARS));
        let previous_message = prev.and_then(|s| s.message.clone());
        let feed = message
            .as_deref()
            .filter(|text| !text.trim().is_empty())
            .filter(|text| previous_message.as_deref() != Some(*text))
            .map(|text| MessageEntry {
                at_ms: epoch_ms(SystemTime::now()),
                session: session.clone(),
                group: group.clone(),
                state,
                message: text.to_string(),
            });

        self.sessions.insert(
            session.clone(),
            Session {
                id: session,
                // 组和状态一样是后写覆盖：换个组上报就把这个会话挪过去
                group,
                state,
                agent,
                cwd,
                message,
                expires_at,
                since,
                pid,
            },
        );
        self.enforce_session_cap();

        if let Some(entry) = feed {
            self.record_message(entry);
        }

        self.settle(now)
    }

    /// 会话表满了先丢「最快过期」的那一条。
    ///
    /// 正常用法的会话数是个位数；触到上限只可能是上报方在造（例如每次运行换一个
    /// 时间戳当 session id）。不这样的话一张表能一直涨到进程退出（issue #16）。
    ///
    /// 选「最快过期」而不是「最久没动」：前者本来也快自己走掉了，丢它造成的可见
    /// 变化最小。
    fn enforce_session_cap(&mut self) {
        while self.sessions.len() > MAX_SESSIONS {
            let Some(id) = self
                .sessions
                .iter()
                .min_by_key(|(_, session)| session.expires_at)
                .map(|(id, _)| id.clone())
            else {
                break;
            };
            self.sessions.remove(&id);
        }
    }

    /// 记一条消息进历史流。满了丢最旧的 —— 常驻应用不该无界增长。
    fn record_message(&mut self, entry: MessageEntry) {
        self.messages.push_back(entry);
        while self.messages.len() > MESSAGE_HISTORY {
            self.messages.pop_front();
        }
    }

    /// 最近上报过的消息，**从旧到新**。前端要倒序显示就自己 reverse。
    pub fn messages(&self) -> Vec<MessageEntry> {
        self.messages.iter().cloned().collect()
    }

    /// `now` 的唯一用途是算静默进度档位（前后比对要用），见 `progress_step`。
    pub fn clear(&mut self, session: &str, now: Instant) -> bool {
        self.remove_session(session, now).1
    }

    /// 返回 `(是否真的移除了会话, 可见状态是否变化)`。
    ///
    /// 第二个值指的是「前端渲染的东西变了」，即聚合值、会话列表或 tooltip 文本
    /// 任一发生变化 —— 不只是聚合值。移除非当前决定聚合的那一路会话时，聚合不变
    /// 但会话列表变了，多会话列表 UI 需要知道。
    ///
    /// HTTP 的 DELETE 需要区分这两者：清除一个本就不存在的会话要幂等地回
    /// `removed: false`，且不该被当成变化去推前端。
    ///
    /// `now` 同 `clear`：只用于静默进度档位。
    pub fn remove_session(&mut self, session: &str, now: Instant) -> (bool, bool) {
        let removed = self.sessions.remove(session).is_some();
        (removed, self.settle(now))
    }

    /// 某个会话的进程号。**只给本机界面用**（收件箱的「跳过去」）——
    /// `SessionView` 刻意不带 pid：那东西也是 `/v1/state` 的返回，会去只读面板
    /// 和手机，进程号没有理由跟着出门。
    pub fn session_pid(&self, id: &str) -> Option<u32> {
        self.sessions.get(id).and_then(|session| session.pid)
    }

    /// 某个会话的工作目录（同样只给「跳过去」用）。会话已经过期时返回 `None`。
    pub fn session_cwd(&self, id: &str) -> Option<String> {
        self.sessions
            .get(id)
            .and_then(|session| session.cwd.clone())
    }

    /// 清掉某一组的全部会话（用户从灯上右键「清除该组」）。
    ///
    /// 返回 `(移除了几个会话, 可见状态是否变化)` —— 两个值要分开报，理由同
    /// `remove_session`：清一个空的组不该被当成变化去推前端。
    ///
    /// `now` 同 `clear`：只用于静默进度档位。
    pub fn clear_group(&mut self, group: &str, now: Instant) -> (usize, bool) {
        // 两趟处理，同 `expire`：先只读地收集要删的 id，再动手删。
        // 一趟里边迭代边删要跟借用检查绕，反而更难看。
        let ids: Vec<String> = self
            .sessions
            .values()
            .filter(|s| s.group == group)
            .map(|s| s.id.clone())
            .collect();

        for id in &ids {
            self.sessions.remove(id);
        }

        (ids.len(), self.settle(now))
    }

    /// `now` 只用于静默进度档位（见 `progress_step`）—— 暂停本身不改变进度，
    /// 但前后比对要用同一个时钟，否则切换暂停的瞬间会凭空多出一次「有变化」。
    pub fn set_paused(&mut self, paused: bool, now: Instant) -> bool {
        self.paused = paused;
        self.settle(now)
    }

    /// 监控是否暂停中。灯上那枚持续可见的角标和折叠态的窗口尺寸都要读它。
    pub fn paused(&self) -> bool {
        self.paused
    }

    /// 用「进程还在不在」修正会话 —— **必须在 [`Store::expire`] 之前调用**
    /// （`sweep` 里就是这么排的）。
    ///
    /// 两件事：
    ///
    /// 1. **进程没了就立刻删。** 不等 TTL：智能体被 `kill -9` 时不会再发
    ///    `SessionEnd`，而灯上还要错 5–10 分钟（再沉默 30 分钟）才消失。
    /// 2. **进程还在、但 TTL 到了 → 保持最后的状态，只续期。** 长任务（一次跑
    ///    20 分钟的构建、一段十分钟的推理）在这期间**一个事件都不会触发**；
    ///    如果因为「多久没说话」就把 `working` 降成 `idle`，灯会在任务还在跑的时候
    ///    自己安静下去 —— 用户看到的是「跑了几分钟就沉默了」。一轮工作什么时候
    ///    结束，由智能体自己的结束事件（`Stop` / `agent_end` /
    ///    `execution.succeeded` …）说了算，不由「沉默时长」说了算。
    ///
    /// `permission` / `attention` / `error` 是「有人在等」，进程活着**不能**说明
    /// 那件事已经过去了，照旧走沉默那条路（TTL → stale → 宽限期后删除）。
    /// `done` 也一样 —— 它本来就会按时消失。
    pub fn refresh_liveness(&mut self, now: Instant, alive: &dyn Fn(u32) -> bool) -> bool {
        // 两趟处理，理由同 `expire`：先只读地判定，再动手改。
        let mut to_remove = Vec::new();
        let mut to_extend = Vec::new();
        for (id, session) in &self.sessions {
            let Some(pid) = session.pid else {
                continue;
            };
            if !alive(pid) {
                to_remove.push(id.clone());
                continue;
            }
            if now < session.expires_at || session.state == Signal::Done {
                continue;
            }
            // 在这里续期的三档都是「进程活着就还可能继续」的：working / thinking
            // 的长任务、以及本来就在等指令的 idle。状态本身不动，所以下面不改
            // 字段，返回的「有没有可见变化」也不会因为它们而变 true。
            if matches!(
                session.state,
                Signal::Thinking | Signal::Working | Signal::Idle
            ) {
                to_extend.push(id.clone());
            }
        }

        for id in to_remove {
            self.sessions.remove(&id);
        }
        for id in to_extend {
            if let Some(session) = self.sessions.get_mut(&id) {
                session.expires_at = now + LIVENESS_RECHECK;
            }
        }

        self.settle(now)
    }

    /// 处理静默超时的会话：转入 `stale`，而不是直接删掉。
    ///
    /// 由 sweep 任务定期调用。这是「智能体消失后灯还会错多久」的唯一闸门。
    ///
    /// **进度跨档也算变化**：一个 `stale` 会话在宽限期内什么都不发生（状态不变、
    /// 集合不变），但细环每分每秒都在缩短。不把它算成变化的话，环会停在刚转
    /// `stale` 那一刻画好的长度上，直到会话被删掉 —— 比不画更误导。
    /// 分档而不逐次推的理由见 `progress_step`。
    pub fn expire(&mut self, now: Instant) -> bool {

        // 两趟处理：先只读地判定谁该走哪条路，再改动。
        // 一趟里同时 retain + 改字段需要绕借用检查，反而更难看。
        let mut to_remove = Vec::new();
        let mut to_stale = Vec::new();
        for (id, session) in &self.sessions {
            if now < session.expires_at {
                continue;
            }
            // done 的语义就是「完成，回到常态」，直接消失；
            // 已经是 stale 的再到期就真删掉。
            if matches!(session.state, Signal::Done | Signal::Stale) {
                to_remove.push(id.clone());
            } else {
                to_stale.push(id.clone());
            }
        }

        for id in to_remove {
            self.sessions.remove(&id);
        }
        for id in to_stale {
            if let Some(session) = self.sessions.get_mut(&id) {
                session.state = Signal::Stale;
                session.expires_at = now + STALE_GRACE;
                // since 保持不动，于是面板里的 age 读起来是「沉默多久了」
            }
        }

        self.settle(now)
    }

    /// 聚合：跨会话取最高优先级 —— 「红 > 黄 > 绿」就实现在这里。
    /// 这样别的会话报绿不会顶掉正在等授权的那一路红，而等授权的那一路
    /// 自己恢复工作后红又自然消失，不会卡在红上。
    pub fn aggregate(&self) -> Signal {
        if self.paused {
            return Signal::Off;
        }
        // 没有任何会话时给 Offline 而不是 Idle：后者会被读成
        // 「智能体在待命」，那是错的。
        self.winner().map(|s| s.state).unwrap_or(Signal::Offline)
    }

    pub fn snapshot(&self) -> Snapshot {
        self.snapshot_at(Instant::now())
    }

    /// 时间可注入的版本。`stale_progress` 依赖「现在」，用真实时钟就没法单测。
    pub fn snapshot_at(&self, now: Instant) -> Snapshot {
        let mut groups: Vec<GroupView> = self
            .group_names()
            .into_iter()
            .map(|name| {
                let winner = self.winner_in(&name);
                let sessions = self.views_in(&name, now);
                // 按会话分组时组名是个 UUID，牌上没法直接画（见 `session_label`）
                let default_label = session_label(&name, &sessions);
                GroupView {
                    default_label,
                    aggregate: winner.map(|s| s.state).unwrap_or(Signal::Offline),
                    message: winner.and_then(|s| s.message.clone()),
                    // 这一组进入当前状态多久了 —— 牌上「已持续」那一段
                    since_ms: winner.map(|s| epoch_ms(s.since)).unwrap_or(0),
                    // 细环取的是**决定聚合那个会话**的进度（见字段注释）
                    stale_progress: winner.and_then(|s| stale_progress(s, now)),
                    sessions,
                    name,
                }
            })
            .collect();

        // 最紧急的排在最前：灯球上牌的顺序就是优先级顺序，瞥一眼先看到该看的那块。
        // 平局按名字，保证顺序稳定。
        //
        // 排序放在服务端而不是前端，是为了让「哪几块牌可见」只有一个判断点 ——
        // 窗口高度取决于可见牌的高度，Rust 必须和前端算出同一批牌。
        groups.sort_by(|a, b| {
            b.aggregate
                .priority()
                .cmp(&a.aggregate.priority())
                .then_with(|| a.name.cmp(&b.name))
        });

        Snapshot {
            aggregate: self.aggregate(),
            paused: self.paused,
            since_ms: self.since.map(epoch_ms).unwrap_or(0),
            message: self.winner().and_then(|s| s.message.clone()),
            groups,
        }
    }

    /// 所有组名，排序过 —— 牌的顺序因此稳定，不随 HashMap 的迭代顺序跳。
    fn group_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.sessions.values().map(|s| s.group.clone()).collect();
        names.sort();
        names.dedup();
        names
    }

    fn views_in(&self, group: &str, now: Instant) -> Vec<SessionView> {
        let mut views: Vec<SessionView> = self
            .sessions
            .values()
            .filter(|s| s.group == group)
            .map(|s| SessionView {
                id: s.id.clone(),
                state: s.state,
                agent: s.agent.clone(),
                cwd: s.cwd.clone(),
                message: s.message.clone(),
                // duration_since 而非 saturating_duration_since：后者对 SystemTime
                // 尚未稳定。自定义时钟回拨时 unwrap_or_default 退化成 0，可接受。
                age_ms: SystemTime::now()
                    .duration_since(s.since)
                    .unwrap_or_default()
                    .as_millis() as u64,
                stale_progress: stale_progress(s, now),
            })
            .collect();
        views.sort_by(|a, b| {
            b.state.priority().cmp(&a.state.priority()).then_with(|| a.id.cmp(&b.id))
        });
        views
    }

    /// 优先级最高者；平局时取最近进入该状态的，再平局取 id 最小者，保证结果确定。
    fn winner(&self) -> Option<&Session> {
        self.sessions.values().max_by(|a, b| by_priority(a, b))
    }

    /// 某一组里优先级最高的会话 —— 那块牌的颜色和消息由它决定。
    fn winner_in(&self, group: &str) -> Option<&Session> {
        self.sessions
            .values()
            .filter(|s| s.group == group)
            .max_by(|a, b| by_priority(a, b))
    }

    /// 某一组「决定聚合那个会话」的工作目录 —— 「打开项目目录」用它。
    ///
    /// 取 winner 而不是组内随便一个：牌上那块颜色 / 消息就是 winner 的，菜单要打开
    /// 的自然也该是同一个会话的目录（组内多个会话在同一个仓库时看不出来，但按会话
    /// 分组时一组就是一个会话，这条就没有歧义了）。
    pub fn winner_cwd_in(&self, group: &str) -> Option<String> {
        self.winner_in(group).and_then(|s| s.cwd.clone())
    }

    /// 用于前后比对：只要会话集合、聚合值或 tooltip 文本变了就返回不同的值。
    ///
    /// `now` 是必需的：静默进度依赖它（见 `Visible::progress`）。
    fn visible(&self, now: Instant) -> Visible {
        let mut sessions: Vec<(String, String, Signal)> = self
            .sessions
            .iter()
            .map(|(k, s)| (k.clone(), s.group.clone(), s.state))
            .collect();
        // 按会话名排序即可得到规范顺序（会话名唯一），无需 Signal 实现 Ord
        sessions.sort_by(|a, b| a.0.cmp(&b.0));

        let mut progress: Vec<(String, u8)> = self
            .sessions
            .iter()
            .map(|(id, s)| (id.clone(), progress_step(stale_progress(s, now))))
            .collect();
        progress.sort();

        Visible {
            aggregate: self.aggregate(),
            paused: self.paused,
            message: self.winner().and_then(|s| s.message.clone()),
            sessions,
            progress,
        }
    }

    /// 和 `last_pushed` 比对：这一帧和上次推给前端的那一帧不一样就算变化。
    ///
    /// **首次调用（还没有基线）时算有变化**：`last_pushed` 是 `None` 表示前端还没
    /// 收到过任何东西，这时哪怕内容一样也得推一次，否则前端永远停在初始状态。
    fn settle(&mut self, now: Instant) -> bool {
        let current = self.visible(now);
        let changed = match &self.last_pushed {
            Some(previous) => *previous != current,
            None => true,
        };
        self.last_pushed = Some(current);
        if changed {
            self.since = Some(SystemTime::now());
        }
        changed
    }
}

/// 会话分组的默认显示名：「目录末段 · 智能体」。
///
/// **为什么需要**：按会话分组时组名就是会话 id，多半是一串 UUID。真拿它当牌子
/// 上的字，一排牌全是 `9f3c1a2e-…`，等于没显示 —— 用户看不出哪个窗口是哪个。
///
/// **为什么目录在前**：牌宽是固定的（`size × 7`），名字那一行还要跟状态文字分，
/// 所以这个词**一定会被截断**，能看见的只有前几个字。而按会话分组的典型情形恰恰
/// 是「同一个智能体开了好几个窗口」—— 那时智能体名是重复的噪音，只有项目名能
/// 把它们区分开。项目放前面，截断切掉的就是那截重复的尾巴。
///
/// **怎么知道这个组是按会话分出来的**：`Settings::resolve_group` 在按会话分组时
/// 直接拿会话 id 当组名，所以只有那种情况下「组名 == 组内某个会话的 id」才成立。
/// 显式给了 `group`、或者按目录分组时，组名是用户或路径给的字符串，不会恰好等于
/// 一个会话 id。这里就靠这条判据区分，不需要额外的标志位。
///
/// `agent` / `cwd` 都拿不到时返回 `None`，前端退回原始组名 —— 那时能显示的只有
/// 那个 id，不好读，但总比编一个假的强。
fn session_label(name: &str, sessions: &[SessionView]) -> Option<String> {
    let session = sessions.iter().find(|s| s.id == name)?;
    let agent = session
        .agent
        .as_deref()
        .map(str::trim)
        .filter(|a| !a.is_empty());
    let project = session
        .cwd
        .as_deref()
        .map(crate::config::project_name)
        .filter(|p| !p.is_empty());

    match (agent, project) {
        (Some(agent), Some(project)) => Some(format!("{project} · {agent}")),
        (Some(agent), None) => Some(agent.to_string()),
        (None, Some(project)) => Some(project),
        (None, None) => None,
    }
}

/// 静默进度分几档。
///
/// **为什么要分档**：`stale_progress` 是个连续的 0.0–1.0，而 `sweep` 每 250ms
/// 醒一次。直接把连续值放进 `Visible` 做前后比对的话，**每次都比出「有变化」**，
/// 于是灯球在整段宽限期里每秒收到 4 次推送，窗口跟着重排 —— 细环本身是给
/// 「省着看」的状态用的，为它把 CPU 烧起来得不偿失。
///
/// 细环是**环的长短**，5 分钟的宽限期里肉眼能看出的变化也就那么几档；而且
/// conic-gradient 的角度变化本身就是离散的。分 16 档（每档约 19 秒）比平滑
/// 动画粗，但换来的是这个常驻窗口在沉默时几乎不工作。
///
/// `None`（非 stale）归到第 0 档 —— 它和「即将消失」都是「没有环」，
/// 不需要区分。
const PROGRESS_STEPS: u8 = 16;

fn progress_step(progress: Option<f32>) -> u8 {
    match progress {
        Some(p) => (p * PROGRESS_STEPS as f32).ceil() as u8,
        None => 0,
    }
}

/// 一个会话的沉默进度：1.0 = 刚转 `stale`，0.0 = 即将被删除。
///
/// 组和会话两处都取它，`visible` 的档位比对也走它 —— 摘成自由函数是为了这三处
/// 用的是同一个定义，不会各自漂移。档位换算见 `progress_step`。
fn stale_progress(session: &Session, now: Instant) -> Option<f32> {
    (session.state == Signal::Stale).then(|| {
        let remaining = session.expires_at.saturating_duration_since(now);
        (remaining.as_secs_f32() / STALE_GRACE.as_secs_f32()).clamp(0.0, 1.0)
    })
}

/// 优先级最高者胜；平局取最近进入该状态的，再平局取 id 最小者 —— 结果确定。
fn by_priority(a: &Session, b: &Session) -> std::cmp::Ordering {
    a.state
        .priority()
        .cmp(&b.state.priority())
        .then_with(|| a.since.cmp(&b.since))
        .then_with(|| b.id.cmp(&a.id))
}

fn epoch_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 大部分用例只用一个组，摊平后就是「所有会话」。
    fn sessions(snapshot: &Snapshot) -> Vec<SessionView> {
        snapshot.all_sessions().cloned().collect()
    }

    fn rep(s: &mut Store, t: Instant, session: &str, state: Signal) -> bool {
        let mut req = Report::new(state);
        req.session = Some(session.to_string());
        let ttl = ttl_of(req.state, req.ttl_ms);
        s.apply(t, req, ttl)
    }

    /// 指定组的版本，多组场景（如 clear_group 的用例）用它。
    fn rep_in(s: &mut Store, t: Instant, session: &str, group: &str, state: Signal) -> bool {
        let mut req = Report::new(state);
        req.session = Some(session.to_string());
        req.group = Some(group.to_string());
        let ttl = ttl_of(req.state, req.ttl_ms);
        s.apply(t, req, ttl)
    }

    /// `source` 只影响「接入」页怎么分组，**不参与任何决策** —— 所以缺省值是
    /// `agent`，而空串 / 空白也算没声明（老上报方就是这么发的）。
    #[test]
    fn the_source_defaults_to_agent_and_blank_counts_as_unset() {
        let mut req = Report::new(Signal::Working);
        assert_eq!(req.source_id(), DEFAULT_SOURCE);

        req.source = Some("   ".into());
        assert_eq!(req.source_id(), DEFAULT_SOURCE, "空白不算声明");

        req.source = Some("  system ".into());
        assert_eq!(req.source_id(), "system", "两头空白要去掉");

        // 认不出来的一律原样返回：这一层不认识类别清单（界面把它归到「自定义」）
        req.source = Some("whatever".into());
        assert_eq!(req.source_id(), "whatever");
    }

    /// 测试里的 TTL：**内置默认那一档**（`Settings::default()` 没有任何逐状态覆盖）。
    /// 要测「用户设的覆盖」和「显式 `ttl_ms` 覆盖」就自己调 `state_ttl`。
    fn ttl_of(state: Signal, explicit_ms: Option<u64>) -> Duration {
        state_ttl(state, explicit_ms, &Settings::default())
    }

    /// 消息在**入口处**截断：它会进灯、通知和 200 条历史流，而 HTTP body 可以到
    /// 2 MB —— 一个把文件内容当 `message` 发的脚本就能把内存撑爆（issue #16）。
    #[test]
    fn an_overlong_message_is_clipped_when_it_enters() {
        let mut store = Store::default();
        let t = Instant::now();
        let mut req = Report::new(Signal::Working);
        req.session = Some("s".into());
        req.message = Some("汉".repeat(500));
        let ttl = ttl_of(req.state, None);
        store.apply(t, req, ttl);

        let snapshot = store.snapshot();
        let message = snapshot.groups[0].sessions[0].message.clone().unwrap();
        assert_eq!(
            message.chars().count(),
            MAX_MESSAGE_CHARS + 1,
            "200 个字符 + 一个省略号"
        );
        // 历史流里那一份也必须是截过的
        assert_eq!(store.messages()[0].message, message);
    }

    /// 会话表有上限：一个「每次运行换一个 session id」的上报方不该让内存无界增长。
    #[test]
    fn the_session_table_stops_growing_at_the_cap() {
        let mut store = Store::default();
        let t = Instant::now();
        for index in 0..(MAX_SESSIONS + 20) {
            rep(&mut store, t, &format!("s{index}"), Signal::Working);
        }
        assert_eq!(store.snapshot().all_sessions().count(), MAX_SESSIONS);
    }

    /// 造一个会话视图 —— 这几个用例只看 `session_label` 关心的 `id` / `agent` / `cwd`。
    fn session(id: &str, agent: Option<&str>, cwd: Option<&str>) -> SessionView {
        SessionView {
            id: id.to_string(),
            state: Signal::Working,
            agent: agent.map(str::to_string),
            cwd: cwd.map(str::to_string),
            message: None,
            age_ms: 0,
            stale_progress: None,
        }
    }

    /// 按会话分组时，牌上显示「目录末段 · 智能体」，而不是那一串 UUID。
    ///
    /// 这是这个字段存在的全部理由：不这么写，一排牌全是 `9f3c1a2e-…`，
    /// 「哪个窗口是哪个」等于没显示。
    ///
    /// **顺序是有意的**：牌宽固定、名字一定被截断，所以只有前几个字看得见；
    /// 而按会话分组的典型情形是「同一个智能体开了好几个窗口」，那时智能体名是
    /// 重复的，能区分的只有项目名 —— 所以项目放前面。
    #[test]
    fn a_session_group_is_labelled_by_project_and_agent() {
        let s = [session(
            "9f3c1a2e",
            Some("claude-code"),
            Some("/Users/me/code/SignalHub"),
        )];
        assert_eq!(
            session_label("9f3c1a2e", &s).as_deref(),
            Some("SignalHub · claude-code")
        );
    }

    /// **组名不是会话 id 的组一律不动** —— 它们是显式 `group` 给的、或按目录分出来的，
    /// 名字本来就是人给的字（或那条路径）。悄悄换掉等于推翻用户的命名。
    ///
    /// 这条同时也是那段判据的锚：`resolve_group` 只在按会话分组时拿会话 id 当组名，
    /// 所以「组名 ≠ 会话 id」就等价于「不是会话分组出来的」。
    #[test]
    fn groups_named_by_hand_or_by_path_keep_their_name() {
        let s = [session(
            "uuid-1",
            Some("claude-code"),
            Some("/Users/me/code/SignalHub"),
        )];
        assert_eq!(session_label("builds", &s), None, "显式给的组名");
        assert_eq!(
            session_label("/Users/me/code/SignalHub", &s),
            None,
            "按目录分的组名"
        );
    }

    /// 信息缺一半时给能给的：只有 agent 就显示 agent，只有目录就显示目录末段。
    #[test]
    fn a_partial_session_still_gets_a_readable_label() {
        assert_eq!(
            session_label("s1", &[session("s1", Some("claude-code"), None)]).as_deref(),
            Some("claude-code")
        );
        assert_eq!(
            session_label("s1", &[session("s1", None, Some("/code/api"))]).as_deref(),
            Some("api")
        );
    }

    /// 两样都拿不到时返回 `None`，让前端退回原始组名 —— 编一个假名字比显示 UUID 更糟。
    #[test]
    fn no_identity_at_all_falls_back_to_the_raw_name() {
        assert_eq!(session_label("s1", &[session("s1", None, None)]), None);
        // 空白串不算数，否则会拼出「 · api」这种开头缺一截的名字
        assert_eq!(session_label("s1", &[session("s1", Some("   "), None)]), None);
        // 组内没有会话（正常到不了，防御）
        assert_eq!(session_label("s1", &[]), None);
    }

    /// 没有任何会话时是 Offline 而不是 Idle：后者会被读成「智能体在待命」
    #[test]
    fn an_empty_store_is_offline_not_idle() {
        let s = Store::default();
        assert_eq!(s.aggregate(), Signal::Offline);
    }

    #[test]
    fn aggregate_takes_highest_priority_across_sessions() {
        let (mut s, t) = (Store::default(), Instant::now());
        rep(&mut s, t, "a", Signal::Working);
        rep(&mut s, t, "b", Signal::Permission);
        assert_eq!(s.aggregate(), Signal::Permission);
    }

    /// 核心规则：另一路会话报绿不能顶掉正在等授权的那一路红。
    #[test]
    fn ordinary_work_does_not_override_pending_permission() {
        let (mut s, t) = (Store::default(), Instant::now());
        rep(&mut s, t, "a", Signal::Permission);
        rep(&mut s, t, "b", Signal::Working);

        assert_eq!(s.aggregate(), Signal::Permission);
        assert!(sessions(&s.snapshot())
            .iter()
            .any(|v| v.id == "a" && v.state == Signal::Permission));
    }

    /// 用户批准后，那一路自己发新事件即可让红消失 —— 不存在卡死的红色。
    #[test]
    fn resolving_permission_in_same_session_falls_back() {
        let (mut s, t) = (Store::default(), Instant::now());
        rep(&mut s, t, "a", Signal::Permission);
        rep(&mut s, t, "a", Signal::Working);
        assert_eq!(s.aggregate(), Signal::Working);
    }

    /// done 的语义是「完成，回到常态」，所以它到期后直接消失，不转 stale
    #[test]
    fn done_expires_away_without_becoming_stale() {
        let (mut s, t) = (Store::default(), Instant::now());
        rep(&mut s, t, "a", Signal::Done);

        assert!(!s.expire(t + Duration::from_secs(29)));
        assert_eq!(s.aggregate(), Signal::Done);

        assert!(s.expire(t + Duration::from_secs(31)));
        assert!(sessions(&s.snapshot()).is_empty(), "不该留下 stale 会话");
        assert_eq!(s.aggregate(), Signal::Offline);
    }

    #[test]
    fn repeated_identical_report_is_not_a_change() {
        let (mut s, t) = (Store::default(), Instant::now());
        rep(&mut s, t, "a", Signal::Working);
        assert!(!rep(&mut s, t, "a", Signal::Working));
    }

    #[test]
    fn message_change_is_a_change() {
        let (mut s, t) = (Store::default(), Instant::now());
        let mut first = Report::new(Signal::Working);
        first.session = Some("a".into());
        first.message = Some("npm test".into());
        let ttl = ttl_of(first.state, first.ttl_ms);
        s.apply(t, first, ttl);

        let mut second = Report::new(Signal::Working);
        second.session = Some("a".into());
        second.message = Some("cargo build".into());
        let ttl = ttl_of(second.state, second.ttl_ms);
        assert!(s.apply(t, second, ttl), "tooltip 文本变了，UI 需要更新");
        assert_eq!(s.snapshot().message.as_deref(), Some("cargo build"));
    }

    #[test]
    fn agent_and_cwd_carry_over_but_message_does_not() {
        let (mut s, t) = (Store::default(), Instant::now());
        let mut first = Report::new(Signal::Working);
        first.session = Some("a".into());
        first.agent = Some("claude-code".into());
        first.cwd = Some("/proj".into());
        first.message = Some("npm test".into());
        let ttl = ttl_of(first.state, first.ttl_ms);
        s.apply(t, first, ttl);

        let mut second = Report::new(Signal::Thinking);
        second.session = Some("a".into());
        let ttl = ttl_of(second.state, second.ttl_ms);
        s.apply(t, second, ttl);

        let v = &sessions(&s.snapshot())[0];
        assert_eq!(v.agent.as_deref(), Some("claude-code"));
        assert_eq!(v.cwd.as_deref(), Some("/proj"));
        assert_eq!(v.message, None);
    }

    #[test]
    fn paused_shows_off_but_keeps_sessions() {
        let (mut s, t) = (Store::default(), Instant::now());
        rep(&mut s, t, "a", Signal::Working);

        assert!(s.set_paused(true, t));
        assert_eq!(s.aggregate(), Signal::Off);

        assert!(s.set_paused(false, t));
        assert_eq!(s.aggregate(), Signal::Working, "恢复后应立即回到真实状态");
    }

    /// 上报方显式给的 `ttl_ms` 覆盖该状态的默认值。
    ///
    /// 这条对**自定义上报方**尤其重要：他们的任务可能一跑就是几十分钟，中间的
    /// 进度由自己控制，所以「多久算沉默」也该由他们说了算，而不是被状态名的
    /// 默认值绑死。
    #[test]
    fn an_explicit_ttl_overrides_the_states_default() {
        let (mut s, t) = (Store::default(), Instant::now());
        let mut req = Report::new(Signal::Working);
        req.session = Some("long".into());
        req.ttl_ms = Some(3_600_000); // 一小时
        let ttl = ttl_of(req.state, req.ttl_ms);
        s.apply(t, req, ttl);

        // 半小时后仍然活着 —— 按 `Working` 的默认值（5 分钟）早该转 stale 了
        assert!(!s.expire(t + Duration::from_secs(1800)));
        assert_eq!(s.aggregate(), Signal::Working);
    }

    /// 设置里写的停留时长（`styles.<状态>.ttl_ms`）：压过内置默认，压不过显式。
    ///
    /// 从 JSON 建而不是手搭结构体：这顺带钉住了**配置里的字段名和形状**
    /// （`styles` 是个按状态名索引的对象、缺的字段有 `#[serde(default)]` 兜底）。
    #[test]
    fn a_user_ttl_beats_the_builtin_but_not_an_explicit_one() {
        let settings: Settings =
            serde_json::from_str(r#"{"styles":{"working":{"ttl_ms":60000}}}"#).unwrap();

        assert_eq!(
            state_ttl(Signal::Working, None, &settings),
            Duration::from_secs(60)
        );
        // 显式给的仍然最高：CLI 的 `--ttl`、包装器心跳、测试上报都靠这一条
        assert_eq!(
            state_ttl(Signal::Working, Some(7_000), &settings),
            Duration::from_secs(7)
        );
        // 没被覆盖的状态照旧走内置默认
        assert_eq!(
            state_ttl(Signal::Idle, None, &settings),
            Signal::Idle.default_ttl()
        );
    }

    /// 用户设的停留时长要**真的作用在过期上**，不只是算出来一个数。
    #[test]
    fn a_user_ttl_changes_when_the_session_expires() {
        let settings: Settings =
            serde_json::from_str(r#"{"styles":{"working":{"ttl_ms":20000}}}"#).unwrap();
        let (mut s, t) = (Store::default(), Instant::now());
        let mut req = Report::new(Signal::Working);
        req.session = Some("a".into());
        s.apply(t, req, state_ttl(Signal::Working, None, &settings));

        // 20 秒这一档比默认的 5 分钟短得多 —— 过期时刻要跟着它走
        assert!(!s.expire(t + Duration::from_secs(19)));
        assert!(s.expire(t + Duration::from_secs(20)));
    }

    /// idle 会话静默过久也会转 stale。早先它没有 TTL（半小时才回收），
    /// 于是一个忘关的会话能一直显示「空闲」—— 这正是「智能体走了灯还亮着」
    /// 的成因之一。
    #[test]
    fn a_silent_idle_session_eventually_goes_stale() {
        let (mut s, t) = (Store::default(), Instant::now());
        rep(&mut s, t, "a", Signal::Idle);
        assert_eq!(s.aggregate(), Signal::Idle);

        let idle_ttl = Signal::Idle.default_ttl();
        assert!(!s.expire(t + idle_ttl - Duration::from_secs(1)));
        assert_eq!(s.aggregate(), Signal::Idle, "还没到时间，不该变");

        assert!(s.expire(t + idle_ttl));
        assert_eq!(s.aggregate(), Signal::Stale);
        assert_eq!(sessions(&s.snapshot()).len(), 1, "stale 会话要保留，不是删掉");
    }

    /// 带进程号的上报。**探活只在 Unix 上生效**，所以这些用例也只在 Unix 上跑。
    #[cfg(unix)]
    fn rep_pid(s: &mut Store, t: Instant, session: &str, state: Signal, pid: u32) -> bool {
        let mut req = Report::new(state);
        req.session = Some(session.to_string());
        req.pid = Some(pid);
        let ttl = ttl_of(req.state, req.ttl_ms);
        s.apply(t, req, ttl)
    }

    /// 0 不是进程号（`kill(0, …)` 指的是当前进程组），入库时就该丢掉 ——
    /// 否则下一拍探活会把它当成「进程没了」，会话凭空消失。
    #[test]
    #[cfg(unix)]
    fn pid_zero_is_treated_as_no_pid() {
        assert_eq!(effective_pid(Some(0)), None);
        assert_eq!(effective_pid(Some(4242)), Some(4242));

        let (mut s, t) = (Store::default(), Instant::now());
        rep_pid(&mut s, t, "a", Signal::Working, 0);
        assert!(s.sessions.values().all(|session| session.pid.is_none()));
    }

    /// 进程还活着、只是没上报 → **保持 last state**，不是「空闲」，更不是「沉默」。
    ///
    /// 这条是长任务的关键：一次 20 分钟的构建 / 十分钟的推理期间一个 hook 事件
    /// 都没有；若按「沉默多久」把它降成 idle，灯会在任务还在跑的时候自己暗下去。
    /// 结束与否由智能体自己的结束事件决定（`Stop` → done、`agent_end` → done …）。
    #[test]
    #[cfg(unix)]
    fn a_live_process_keeps_working_through_a_long_task() {
        let (mut s, t) = (Store::default(), Instant::now());
        let mut req = Report::new(Signal::Working);
        req.session = Some("a".into());
        req.pid = Some(42);
        req.message = Some("npm test".into());
        let ttl = ttl_of(req.state, req.ttl_ms);
        s.apply(t, req, ttl);

        let expired = t + Signal::Working.default_ttl();
        // 状态没变 → 不该白白推一帧
        assert!(!s.refresh_liveness(expired, &|pid| pid == 42));

        let view = &sessions(&s.snapshot())[0];
        assert_eq!(view.state, Signal::Working, "长任务不能被降成空闲");
        assert_eq!(view.message.as_deref(), Some("npm test"), "上一句在跑的东西还成立");
        assert_eq!(s.aggregate(), Signal::Working);
        assert!(!s.expire(expired), "刚续过期，expire 不该再动它");

        // 再过一分钟还是活着、还是没有事件 → 继续续期，仍然 working
        let later = expired + Duration::from_secs(61);
        assert!(!s.refresh_liveness(later, &|pid| pid == 42));
        assert_eq!(s.aggregate(), Signal::Working);

        // 任务结束（进程退出 / 没发结束事件）→ 探活立刻收掉，不留假灯
        assert!(s.refresh_liveness(later, &|_| false));
        assert_eq!(s.aggregate(), Signal::Offline);
    }

    /// 长思考同理：十分钟不出事件也不能降成空闲。
    #[test]
    #[cfg(unix)]
    fn a_live_process_keeps_thinking_through_a_long_reasoning_turn() {
        let (mut s, t) = (Store::default(), Instant::now());
        rep_pid(&mut s, t, "a", Signal::Thinking, 42);

        let expired = t + Signal::Thinking.default_ttl();
        assert!(!s.refresh_liveness(expired, &|pid| pid == 42));
        assert_eq!(s.aggregate(), Signal::Thinking);
        assert!(!s.expire(expired));
    }

    /// 进程没了就立刻收掉，不用等 TTL —— 被 `kill -9` 的智能体不会再发
    /// `SessionEnd`，等下去只会让灯多错 5–10 分钟（再沉默半小时）。
    #[test]
    #[cfg(unix)]
    fn a_dead_process_is_dropped_before_its_ttl() {
        let (mut s, t) = (Store::default(), Instant::now());
        rep_pid(&mut s, t, "a", Signal::Working, 42);

        assert!(s.refresh_liveness(t + Duration::from_secs(1), &|_| false));
        assert_eq!(s.aggregate(), Signal::Offline);
    }

    /// 「有人在等」的三档不靠探活改判：进程活着**不能**说明授权已经批了。
    /// 它们照旧走 TTL → stale → 宽限期那条路。
    #[test]
    #[cfg(unix)]
    fn permission_keeps_the_silence_path_even_when_the_process_lives() {
        let (mut s, t) = (Store::default(), Instant::now());
        rep_pid(&mut s, t, "a", Signal::Permission, 42);

        let expired = t + Signal::Permission.default_ttl();
        assert!(!s.refresh_liveness(expired, &|_| true), "探活不该改它");
        assert!(s.expire(expired));
        assert_eq!(s.aggregate(), Signal::Stale);
    }

    /// done 的语义是「这一轮干完了」，和进程在不在无关：照旧按时消失。
    #[test]
    #[cfg(unix)]
    fn done_still_disappears_on_schedule_with_a_live_process() {
        let (mut s, t) = (Store::default(), Instant::now());
        rep_pid(&mut s, t, "a", Signal::Done, 42);

        let expired = t + Signal::Done.default_ttl();
        assert!(!s.refresh_liveness(expired, &|_| true));
        assert!(s.expire(expired));
        assert_eq!(s.aggregate(), Signal::Offline);
    }

    /// 活着但没上报的 idle 会话不会过期 —— 它就是「空闲」，一直空闲也还是空闲。
    #[test]
    #[cfg(unix)]
    fn an_idle_session_with_a_live_process_never_goes_stale() {
        let (mut s, t) = (Store::default(), Instant::now());
        rep_pid(&mut s, t, "a", Signal::Idle, 42);

        let expired = t + Signal::Idle.default_ttl();
        assert!(!s.refresh_liveness(expired, &|_| true), "状态没变，不推帧");
        assert!(!s.expire(expired), "已经续过期了");
        assert_eq!(s.aggregate(), Signal::Idle);
    }

    /// 不带 pid 的上报（HTTP 审批钩子、手写 curl）不能把已经记住的进程号抹掉。
    #[test]
    #[cfg(unix)]
    fn a_report_without_a_pid_keeps_the_one_we_already_know() {
        let (mut s, t) = (Store::default(), Instant::now());
        rep_pid(&mut s, t, "a", Signal::Working, 42);

        rep(&mut s, t, "a", Signal::Working);
        assert!(s.refresh_liveness(t + Duration::from_secs(1), &|_| false));
        assert_eq!(s.aggregate(), Signal::Offline, "旧的 pid 仍然管用");
    }

    /// `pid` 是可选字段：老客户端不带它必须照常解析。
    #[test]
    fn the_pid_field_is_optional_in_the_wire_format() {
        let with_pid: Report =
            serde_json::from_str(r#"{"state":"working","pid":123}"#).unwrap();
        assert_eq!(with_pid.pid, Some(123));

        let without: Report = serde_json::from_str(r#"{"state":"working"}"#).unwrap();
        assert_eq!(without.pid, None);
    }

    /// 会话静默超时后转 stale，而不是消失 —— 这是「智能体不说话了」和
    /// 「智能体空闲待命」能分开显示的关键
    #[test]
    fn a_silent_working_session_becomes_stale_not_gone() {
        let (mut s, t) = (Store::default(), Instant::now());
        rep(&mut s, t, "a", Signal::Working);

        assert!(s.expire(t + Signal::Working.default_ttl()));

        let view = &sessions(&s.snapshot())[0];
        assert_eq!(view.state, Signal::Stale);
        assert_eq!(view.id, "a", "会话还在，只是标记为沉默");
        assert_eq!(s.aggregate(), Signal::Stale);
    }

    #[test]
    fn a_stale_session_is_dropped_after_its_grace_period() {
        let (mut s, t) = (Store::default(), Instant::now());
        rep(&mut s, t, "a", Signal::Working);
        // 用 `STALE_GRACE` 相对算而不是写死秒数：写死的话，改宽限期就得回来改测试，
        // 而「改了宽限期却忘了改测试」正是这类常量最容易出的事
        let ttl = Signal::Working.default_ttl();

        assert!(s.expire(t + ttl));
        assert_eq!(s.aggregate(), Signal::Stale);

        // 宽限期里推一帧仍然是 `stale`：会话本身没动，变的只是细环的进度档位
        s.expire(t + ttl + Duration::from_secs(STALE_GRACE.as_secs() - 1));
        assert_eq!(s.aggregate(), Signal::Stale, "宽限期内仍应保留");

        assert!(s.expire(t + ttl + Duration::from_secs(STALE_GRACE.as_secs() + 1)));
        assert!(sessions(&s.snapshot()).is_empty());
        assert_eq!(s.aggregate(), Signal::Offline);
    }

    /// 细环进度**跨档**要算一次变化。没有这条，环会停在刚转 `stale` 那一刻画好的
    /// 长度上，直到会话被删掉 —— 那比不画更误导。
    ///
    /// 断言的是**档与档之间**那只推一次，而不是每 tick 都推：连续值直接进前后
    /// 比对的话，`sweep` 每 250ms 都会比出「有变化」，一个常驻窗口会一直重排。
    #[test]
    fn a_progress_step_counts_as_a_change() {
        let (mut s, t) = (Store::default(), Instant::now());
        rep(&mut s, t, "a", Signal::Working);
        let ttl = Signal::Working.default_ttl();

        assert!(s.expire(t + ttl), "转 stale 本身当然是一次变化");

        // 5 分钟宽限期分 16 档，每档约 18.75 秒。第一次 expire 已经把进度记在第 16
        // 档（当时还剩满额），所以走到「正好同一档」的时间点就该再推一次。
        let one_step = Duration::from_secs_f32(STALE_GRACE.as_secs_f32() / 16.0);
        assert!(
            s.expire(t + ttl + one_step),
            "进度跨了一档，前端要重画那条环"
        );

        // 同一档内重复调用不该再推 —— 这一条才是分档的意义所在
        assert!(
            !s.expire(t + ttl + one_step + Duration::from_millis(1)),
            "同一档里不该反复推"
        );
    }

    /// 组上的细环取的是**决定聚合那个会话**的进度。
    ///
    /// 组内两路都沉默时，圆点是因为聚合值更高的那一路才变灰的，环要缩短的
    /// 自然也是它 —— 取组内第一个的话，画出来的是另一路的进度。
    /// 组上的细环取的是**决定聚合那个会话**的进度。
    ///
    /// 组内两路都沉默时，圆点是因为聚合值更高的那一路才变 `stale` 的，环要缩短的
    /// 自然也是它 —— 取组内第一个的话，画出来的是另一路的进度。
    #[test]
    fn group_progress_follows_the_winning_session() {
        let (mut s, t) = (Store::default(), Instant::now());
        // `working` 比 `done` 优先；沉默后 `working` 那一路就是组内的胜者
        rep_in(&mut s, t, "slacker", "g", Signal::Done);
        rep_in(&mut s, t, "busy", "g", Signal::Working);
        let ttl = Signal::Working.default_ttl();

        // 同一时刻过期，所以宽限期同时开始
        s.expire(t + ttl);

        let snapshot = s.snapshot();
        let group = snapshot
            .groups
            .iter()
            .find(|g| g.name == "g")
            .expect("组 g 在");
        assert_eq!(group.aggregate, Signal::Stale);
        assert!(group.stale_progress.is_some(), "该组的圆点要画环");

        // 胜者必须是 `busy`（priority 更高），环也就必须取它的 —— 用「精确等于那个
        // 会话的值」来锁，而不是只断言个 None / Some
        let winner = group
            .sessions
            .iter()
            .find(|v| v.id == "busy")
            .expect("busy 在组里");
        assert_eq!(
            group.stale_progress, winner.stale_progress,
            "组的环要跟着决定聚合的那个会话"
        );

        // 反面：`done` 那一路沉默后会**直接消失**（done 的语义是「完成，回到常态」），
        // 所以活着的只有 busy，这一条同时也是「组里只剩胜者」的旁证
        assert!(group.sessions.iter().all(|v| v.id == "busy"), "{:?}", group.sessions);
    }

    /// 「打开项目目录」取的是**决定聚合那个会话**的 cwd —— 牌上那块颜色是谁的，
    /// 打开的目录就该是谁的（见 `winner_cwd_in`）。
    #[test]
    fn the_project_dir_comes_from_the_winning_session() {
        let (mut s, t) = (Store::default(), Instant::now());
        let mut slow = Report::new(Signal::Done);
        slow.session = Some("slow".into());
        slow.group = Some("g".into());
        slow.cwd = Some("/tmp/slow".into());
        let ttl = ttl_of(slow.state, slow.ttl_ms);
        s.apply(t, slow, ttl);
        // `working` 比 `done` 优先，所以 winner 是 busy
        let mut busy = Report::new(Signal::Working);
        busy.session = Some("busy".into());
        busy.group = Some("g".into());
        busy.cwd = Some("/tmp/busy".into());
        let ttl = ttl_of(busy.state, busy.ttl_ms);
        s.apply(t, busy, ttl);

        assert_eq!(s.winner_cwd_in("g").as_deref(), Some("/tmp/busy"), "取 winner 的目录");
        assert_eq!(s.winner_cwd_in("nope"), None, "没有这一组就没有目录");

        // 一组里只有没报 cwd 的会话时，也不能凭空编一个出来
        let mut bare = Report::new(Signal::Working);
        bare.session = Some("bare".into());
        bare.group = Some("empty".into());
        let ttl = ttl_of(bare.state, bare.ttl_ms);
        s.apply(t, bare, ttl);
        assert_eq!(s.winner_cwd_in("empty"), None);
    }

    /// 一路会话还活着（idle）时，不该被另一路已经沉默的会话盖成灰的
    #[test]
    fn a_live_idle_session_outranks_a_stale_one() {
        let (mut s, t) = (Store::default(), Instant::now());
        rep(&mut s, t, "silent", Signal::Working);
        rep(&mut s, t, "alive", Signal::Idle);

        s.expire(t + Signal::Working.default_ttl());

        let states: Vec<Signal> = sessions(&s.snapshot()).iter().map(|v| v.state).collect();
        assert!(states.contains(&Signal::Stale), "{states:?}");
        assert!(states.contains(&Signal::Idle), "{states:?}");
        assert_eq!(s.aggregate(), Signal::Idle, "活着的那一路优先");
    }

    #[test]
    fn stale_progress_counts_down_from_one_to_zero() {
        let (mut s, t) = (Store::default(), Instant::now());
        rep(&mut s, t, "a", Signal::Working);
        let ttl = Signal::Working.default_ttl();

        // 非 stale 会话没有这个值
        assert_eq!(sessions(&s.snapshot_at(t))[0].stale_progress, None);

        // 刚转 stale：还剩满额宽限
        s.expire(t + ttl);
        let fresh = sessions(&s.snapshot_at(t + ttl))[0].stale_progress.unwrap();
        assert!(fresh > 0.98, "刚转 stale 应接近 1.0，实际 {fresh}");

        // 宽限期过半（相对 `STALE_GRACE` 算，改宽限期不用回来改这里）
        let half_at = STALE_GRACE.as_secs() / 2;
        let half = sessions(&s.snapshot_at(t + ttl + Duration::from_secs(half_at)))[0]
            .stale_progress
            .unwrap();
        assert!((0.4..0.6).contains(&half), "过半时应接近 0.5，实际 {half}");

        // 到点：归零
        let gone = sessions(&s.snapshot_at(t + ttl + STALE_GRACE))[0]
            .stale_progress
            .unwrap();
        assert!(gone < 0.02, "宽限用尽应接近 0.0，实际 {gone}");
    }

    #[test]
    fn reporting_again_clears_stale() {
        let (mut s, t) = (Store::default(), Instant::now());
        rep(&mut s, t, "a", Signal::Working);
        s.expire(t + Signal::Working.default_ttl());
        assert_eq!(s.aggregate(), Signal::Stale);

        rep(&mut s, t, "a", Signal::Working);
        assert_eq!(s.aggregate(), Signal::Working, "恢复上报就该离开 stale");
    }

    /// stale / offline 是聚合派生出来的，外部程序不该声称自己处于其中
    #[test]
    fn derived_states_cannot_be_reported() {
        for raw in ["stale", "offline"] {
            let err = raw.parse::<Signal>().unwrap_err();
            assert!(err.contains("不能上报"), "{raw}: {err}");
        }
        assert!(serde_json::from_str::<Report>(r#"{"state":"stale"}"#).is_err());
        assert!(serde_json::from_str::<Report>(r#"{"state":"offline"}"#).is_err());
    }

    /// 每个状态都要有 TTL，而且**「忙碌」那两个必须给得足够宽**。
    ///
    /// 这是一条回归测试，起因是一个真实的体验问题：用户看到「智能体正在思考，
    /// 灯却说没有智能体上报」。根因是钩子挂在**事件**上（`UserPromptSubmit` →
    /// thinking、`PreToolUse` → working），而模型思考、或跑一个没有工具调用的长
    /// 回复时**一个事件都不触发** —— 原先这两个是 60s / 90s，一次稍长的思考就让
    /// 它转沉默、再过五分钟彻底消失。
    ///
    /// 所以钉的是「不少于 5 分钟」，和早先那条「不超过 2 分钟」的方向**整个反了**
    /// —— 把旧的那条留在原地，正是这个 bug 能活到今天的原因。
    #[test]
    fn every_state_has_a_ttl_and_the_busy_ones_are_generous() {
        for state in [
            Signal::Offline,
            Signal::Off,
            Signal::Stale,
            Signal::Idle,
            Signal::Done,
            Signal::Thinking,
            Signal::Working,
            Signal::Attention,
            Signal::Permission,
            Signal::Error,
        ] {
            assert!(state.default_ttl() > Duration::ZERO, "{state:?} 缺少 TTL");
        }
        for busy in [Signal::Thinking, Signal::Working] {
            assert!(
                busy.default_ttl() >= Duration::from_secs(5 * 60),
                "{busy:?} 的静默期太短：钩子是按事件触发的，长任务期间一个都没有"
            );
        }
    }

    /// 沉默之后**要留得住**：`stale` 是「它不说话了」，不该那么快升级成「它不存在」。
    #[test]
    fn a_quiet_session_stays_visible_for_a_long_while() {
        assert!(
            STALE_GRACE >= Duration::from_secs(30 * 60),
            "沉默后保留 {STALE_GRACE:?} 太短 —— 用户切回来时灯上已经空了"
        );
    }

    #[test]
    fn off_state_clears_the_session() {
        let (mut s, t) = (Store::default(), Instant::now());
        rep(&mut s, t, "a", Signal::Error);
        assert!(rep(&mut s, t, "a", Signal::Off), "清除应报告变化");
        assert!(sessions(&s.snapshot()).is_empty());
        assert_eq!(s.aggregate(), Signal::Offline);
    }

    #[test]
    fn clear_is_idempotent() {
        let (mut s, t) = (Store::default(), Instant::now());
        rep(&mut s, t, "a", Signal::Error);
        assert!(s.clear("a", t));
        assert!(!s.clear("a", t));
    }

    #[test]
    fn remove_session_reports_removed_and_visible_change_separately() {
        let (mut s, t) = (Store::default(), Instant::now());
        rep(&mut s, t, "a", Signal::Error);
        rep(&mut s, t, "b", Signal::Idle);

        // 移除的是没在决定聚合的那一路：聚合不变，但会话列表变了
        let (removed, changed) = s.remove_session("b", t);
        assert!(removed);
        assert!(changed, "会话表变了，前端需要更新");
        assert_eq!(s.aggregate(), Signal::Error, "聚合不该被影响");

        // 再删一次：既没移除，也没有任何可见变化
        assert_eq!(s.remove_session("b", t), (false, false));

        // 删掉最后一个会话，就没有任何会话在报了
        assert_eq!(s.remove_session("a", t), (true, true));
        assert_eq!(s.aggregate(), Signal::Offline);
    }

    /// 清组只动那一个组：别的组的会话必须原样留着
    #[test]
    fn clear_group_removes_only_that_group() {
        let (mut s, t) = (Store::default(), Instant::now());
        rep_in(&mut s, t, "a1", "alpha", Signal::Working);
        rep_in(&mut s, t, "a2", "alpha", Signal::Idle);
        rep_in(&mut s, t, "b1", "beta", Signal::Error);

        let (removed, changed) = s.clear_group("alpha", t);
        assert_eq!(removed, 2);
        assert!(changed, "会话表变了，前端需要更新");

        let ids: Vec<String> = sessions(&s.snapshot()).iter().map(|v| v.id.clone()).collect();
        assert_eq!(ids, vec!["b1".to_string()], "只该留下 beta 组的会话");
    }

    /// 清一个空组/不存在的组既没移除任何会话，也没有任何可见变化 ——
    /// 前端不该为它收到一次推送
    #[test]
    fn clearing_an_empty_group_is_a_no_op() {
        let (mut s, t) = (Store::default(), Instant::now());
        rep_in(&mut s, t, "a", "alpha", Signal::Working);

        assert_eq!(s.clear_group("nope", t), (0, false));
        assert_eq!(s.clear_group("alpha", t), (1, true), "有会话的组要报移除了 1 个");
    }

    /// 决定聚合的那一组被清掉时聚合会变；不动聚合的那一组也会报变化
    /// （会话表变了），但聚合保持不动
    #[test]
    fn clearing_a_group_reports_change_even_when_aggregate_is_untouched() {
        let (mut s, t) = (Store::default(), Instant::now());
        rep_in(&mut s, t, "a", "alpha", Signal::Idle);
        rep_in(&mut s, t, "b", "beta", Signal::Error);

        // beta 的 Error 优先级最高、在决定聚合，清掉 alpha 不影响聚合
        let (removed, changed) = s.clear_group("alpha", t);
        assert_eq!(removed, 1);
        assert!(changed, "会话表变了，前端需要更新");
        assert_eq!(s.aggregate(), Signal::Error, "聚合不该被影响");

        // 再清掉决定聚合的 beta，聚合随之落下
        let (removed, changed) = s.clear_group("beta", t);
        assert_eq!(removed, 1);
        assert!(changed);
        assert_eq!(s.aggregate(), Signal::Offline, "一个会话都不剩了");
    }

    /// 把所有组都清空后回到 Offline —— 没有会话在报了，而不是 Idle
    #[test]
    fn clearing_every_group_returns_to_offline() {
        let (mut s, t) = (Store::default(), Instant::now());
        rep_in(&mut s, t, "a", "alpha", Signal::Working);
        rep_in(&mut s, t, "b", "beta", Signal::Permission);

        s.clear_group("alpha", t);
        s.clear_group("beta", t);

        assert!(sessions(&s.snapshot()).is_empty());
        assert_eq!(s.aggregate(), Signal::Offline);
    }

    // ---- 消息流 ---------------------------------------------------------------

    fn rep_with_message(
        s: &mut Store,
        t: Instant,
        session: &str,
        state: Signal,
        message: &str,
    ) -> bool {
        let mut req = Report::new(state);
        req.session = Some(session.to_string());
        req.message = Some(message.to_string());
        let ttl = ttl_of(req.state, req.ttl_ms);
        s.apply(t, req, ttl)
    }

    #[test]
    fn a_message_is_recorded_in_the_feed() {
        let (mut s, t) = (Store::default(), Instant::now());
        rep_with_message(&mut s, t, "a", Signal::Working, "npm test");

        let feed = s.messages();
        assert_eq!(feed.len(), 1);
        assert_eq!(feed[0].message, "npm test");
        assert_eq!(feed[0].session, "a");
        assert_eq!(feed[0].group, DEFAULT_GROUP);
        assert_eq!(feed[0].state, Signal::Working);
    }

    /// 一轮工具调用里同一条说明会被反复上报（每次 PreToolUse 都带同一个
    /// message），每次都记就成了噪音
    #[test]
    fn repeating_the_same_message_records_it_once() {
        let (mut s, t) = (Store::default(), Instant::now());
        for _ in 0..10 {
            rep_with_message(&mut s, t, "a", Signal::Working, "npm test");
        }
        assert_eq!(s.messages().len(), 1, "同一条消息不该刷屏");

        rep_with_message(&mut s, t, "a", Signal::Working, "cargo build");
        assert_eq!(s.messages().len(), 2, "换了内容才记第二笔");
    }

    /// 没有消息的上报（绝大多数心跳）不该在流里留空行
    #[test]
    fn a_report_without_a_message_adds_nothing() {
        let (mut s, t) = (Store::default(), Instant::now());
        rep(&mut s, t, "a", Signal::Working);
        assert!(s.messages().is_empty());

        rep_with_message(&mut s, t, "a", Signal::Working, "   ");
        assert!(s.messages().is_empty(), "只有空白也算没消息");
    }

    /// 流是封顶的，满了丢最旧的 —— 常驻应用不该无界增长
    #[test]
    fn the_feed_is_capped_and_drops_the_oldest() {
        let (mut s, t) = (Store::default(), Instant::now());
        for i in 0..(MESSAGE_HISTORY + 50) {
            rep_with_message(&mut s, t, "a", Signal::Working, &format!("msg {i}"));
        }

        let feed = s.messages();
        assert_eq!(feed.len(), MESSAGE_HISTORY);
        assert_eq!(
            feed.last().unwrap().message,
            format!("msg {}", MESSAGE_HISTORY + 49),
            "最新的在最后"
        );
        assert_ne!(feed[0].message, "msg 0", "最旧的应该已经被丢掉");
    }

    /// 从旧到新 —— 前端要倒序显示就自己 reverse，别在两边各倒一次
    #[test]
    fn the_feed_is_oldest_first() {
        let (mut s, t) = (Store::default(), Instant::now());
        rep_with_message(&mut s, t, "a", Signal::Working, "first");
        rep_with_message(&mut s, t + Duration::from_secs(1), "a", Signal::Working, "second");

        let feed = s.messages();
        assert_eq!(feed[0].message, "first");
        assert_eq!(feed[1].message, "second");
        assert!(feed[0].at_ms <= feed[1].at_ms);
    }

    #[test]
    fn signal_aliases_parse() {
        for (input, want) in [
            ("blocked", Signal::Error),
            ("failure", Signal::Error),
            ("STUCK", Signal::Error),
            ("permission_request", Signal::Permission),
            ("tool_done", Signal::Working),
            ("running", Signal::Working),
            ("completed", Signal::Done),
            ("notification", Signal::Attention),
            (" paused ", Signal::Off),
            ("permission-request", Signal::Permission),
        ] {
            assert_eq!(input.parse::<Signal>().unwrap(), want, "解析 {input:?} 失败");
        }
        assert!("nonsense".parse::<Signal>().is_err());
    }

    #[test]
    fn serde_deserialize_accepts_canonical_names_and_aliases() {
        let canonical: Report = serde_json::from_str(r#"{"state":"working"}"#).unwrap();
        assert_eq!(canonical.state, Signal::Working);
        assert_eq!(canonical.session_id(), "default");

        let alias: Report = serde_json::from_str(r#"{"state":"blocked"}"#).unwrap();
        assert_eq!(alias.state, Signal::Error);

        assert!(serde_json::from_str::<Report>(r#"{"state":"nonsense"}"#).is_err());
        assert!(serde_json::from_str::<Report>(r#"{}"#).is_err(), "state 必填");
    }

    #[test]
    fn serde_serialize_uses_canonical_lowercase() {
        assert_eq!(serde_json::to_string(&Signal::Permission).unwrap(), r#""permission""#);
        assert_eq!(serde_json::to_string(&Signal::Error).unwrap(), r#""error""#);
        // 派生状态也要能序列化出去（前端的灯球与设置面板都按这个值取色）
        assert_eq!(serde_json::to_string(&Signal::Stale).unwrap(), r#""stale""#);
        assert_eq!(serde_json::to_string(&Signal::Offline).unwrap(), r#""offline""#);
    }

    /// `name()` 必须和 serde 的序列化结果、以及 ALL_NAMES 完全一致 ——
    /// 钩子的 when 过滤和配置里的颜色键都按名字匹配，漂移了会静默失配。
    #[test]
    fn names_agree_across_name_serde_and_all_names() {
        let mut from_names: Vec<&str> = Vec::new();
        for state in [
            Signal::Offline,
            Signal::Off,
            Signal::Stale,
            Signal::Idle,
            Signal::Done,
            Signal::Thinking,
            Signal::Working,
            Signal::Attention,
            Signal::Permission,
            Signal::Error,
        ] {
            let serialized = serde_json::to_string(&state).unwrap();
            assert_eq!(serialized, format!("\"{}\"", state.name()), "{state:?} 不一致");
            assert!(Signal::is_known_name(state.name()), "{state:?} 不在 ALL_NAMES 里");
            // `from_name` 必须能把每个名字读回同一个变体 —— 统计落盘走的就是这条路
            assert_eq!(
                Signal::from_name(state.name()),
                Some(state),
                "{state:?} 的名字读不回来"
            );
            from_names.push(state.name());
        }
        assert_eq!(from_names.len(), Signal::ALL_NAMES.len(), "ALL_NAMES 数量对不上");
        for name in Signal::ALL_NAMES {
            assert!(from_names.contains(&name), "ALL_NAMES 里的 {name} 没有对应变体");
        }
        // 认不出来的名字要返回 None，而不是悄悄落回某个默认值
        assert_eq!(Signal::from_name("nonsense"), None);
    }

    #[test]
    fn priority_order_matches_spec() {
        let ascending = [
            Signal::Stale,
            Signal::Idle,
            Signal::Done,
            Signal::Thinking,
            Signal::Working,
            Signal::Attention,
            Signal::Permission,
            Signal::Error,
        ];
        for pair in ascending.windows(2) {
            assert!(
                pair[0].priority() < pair[1].priority(),
                "{:?} 的优先级应低于 {:?}",
                pair[0],
                pair[1]
            );
        }
        assert_eq!(Signal::Offline.priority(), 0, "没有任何会话时最低");
        assert_eq!(Signal::Off.priority(), 0, "暂停时最低");
    }
}
