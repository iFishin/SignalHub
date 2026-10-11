use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;
use std::str::FromStr;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// 会话静默超时后转入 `stale`（而不是直接消失）并再保留多久。
///
/// 这一段存在的意义是把「智能体不说话了」和「智能体空闲待命」分开显示。
/// 直接删掉的话，智能体崩溃/被强杀（SessionEnd 没触发）会表现为灯保持原状
/// 直到 TTL 到期，用户看不出区别。
const STALE_GRACE: Duration = Duration::from_secs(5 * 60);

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

    /// 中文名。**只用于系统通知的文案** —— 界面上的文案在前端的 `signals.ts`。
    ///
    /// 在这里重复一份是因为通知由 Rust 侧发出（比前端可靠：webview 出问题也
    /// 照发）。match 是穷尽的，所以新增状态时编译器会强制补上标签，
    /// 不会静默漏掉。
    pub fn label(self) -> &'static str {
        match self {
            Signal::Offline => "没有智能体在上报",
            Signal::Off => "已暂停监控",
            Signal::Stale => "会话已沉默",
            Signal::Idle => "空闲",
            Signal::Done => "已完成",
            Signal::Thinking => "推理中",
            Signal::Working => "执行中",
            Signal::Attention => "需要关注",
            Signal::Permission => "需要授权",
            Signal::Error => "出错 / 阻塞",
        }
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
            Signal::Thinking => Duration::from_secs(60),
            Signal::Working => Duration::from_secs(90),
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

/// 一次状态上报。HTTP 请求体与 CLI 参数都归一化成它。
#[derive(Debug, Clone, Deserialize)]
pub struct Report {
    pub state: Signal,
    pub session: Option<String>,
    pub agent: Option<String>,
    pub cwd: Option<String>,
    pub message: Option<String>,
    pub ttl_ms: Option<u64>,
}

impl Report {
    /// 只给单测用的紧凑构造器；生产路径上的 Report 都来自反序列化。
    #[cfg(test)]
    pub fn new(state: Signal) -> Self {
        Report { state, session: None, agent: None, cwd: None, message: None, ttl_ms: None }
    }

    pub fn session_id(&self) -> &str {
        self.session.as_deref().unwrap_or("default")
    }
}

#[derive(Debug, Clone)]
struct Session {
    /// 冗余存一份 id，让聚合平局时的排序确定（HashMap 迭代顺序不确定）。
    id: String,
    state: Signal,
    agent: Option<String>,
    cwd: Option<String>,
    message: Option<String>,
    /// 单调时钟，用于 TTL 判定。每个状态都有 TTL，所以这里必定有值。
    expires_at: Instant,
    /// 进入当前状态的时间，用于 UI 的「已持续多久」。
    /// 转入 `stale` 时刻意不改它，这样它读起来就是「沉默多久了」。
    since: SystemTime,
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

#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub aggregate: Signal,
    pub paused: bool,
    pub since_ms: u64,
    pub message: Option<String>,
    pub sessions: Vec<SessionView>,
}

/// 前端真正会渲染的那部分。用它做前后比对，避免「message 变了但聚合没变」
/// 这类更新被当成无变化而丢掉。
#[derive(Debug, PartialEq)]
struct Visible {
    aggregate: Signal,
    paused: bool,
    message: Option<String>,
    sessions: Vec<(String, Signal)>,
}

#[derive(Debug, Default)]
pub struct Store {
    sessions: HashMap<String, Session>,
    paused: bool,
    /// 聚合值最后一次发生变化的时间
    since: Option<SystemTime>,
}

impl Store {
    pub fn apply(&mut self, now: Instant, req: Report) -> bool {
        // 先取出会话名，之后就只移动 req 的各个字段，不再整体借用它
        let session = req.session_id().to_string();

        // `off` 作用在会话上等价于清除该会话；全局暂停走 set_paused。
        if req.state == Signal::Off {
            return self.clear(&session);
        }

        let before = self.visible();
        let prev = self.sessions.get(&session);

        // agent / cwd 是身份信息，沿用上一次；message 是事件细节，每次替换。
        let agent = req.agent.or_else(|| prev.and_then(|s| s.agent.clone()));
        let cwd = req.cwd.or_else(|| prev.and_then(|s| s.cwd.clone()));
        let since = match prev {
            Some(p) if p.state == req.state => p.since,
            _ => SystemTime::now(),
        };
        let ttl = req
            .ttl_ms
            .map(Duration::from_millis)
            .unwrap_or_else(|| req.state.default_ttl());
        let expires_at = now + ttl;

        self.sessions.insert(
            session.clone(),
            Session {
                id: session,
                state: req.state,
                agent,
                cwd,
                message: req.message,
                expires_at,
                since,
            },
        );

        self.settle(before)
    }

    pub fn clear(&mut self, session: &str) -> bool {
        self.remove_session(session).1
    }

    /// 返回 `(是否真的移除了会话, 可见状态是否变化)`。
    ///
    /// 第二个值指的是「前端渲染的东西变了」，即聚合值、会话列表或 tooltip 文本
    /// 任一发生变化 —— 不只是聚合值。移除非当前决定聚合的那一路会话时，聚合不变
    /// 但会话列表变了，多会话列表 UI 需要知道。
    ///
    /// HTTP 的 DELETE 需要区分这两者：清除一个本就不存在的会话要幂等地回
    /// `removed: false`，且不该被当成变化去推前端。
    pub fn remove_session(&mut self, session: &str) -> (bool, bool) {
        let before = self.visible();
        let removed = self.sessions.remove(session).is_some();
        (removed, self.settle(before))
    }

    pub fn set_paused(&mut self, paused: bool) -> bool {
        let before = self.visible();
        self.paused = paused;
        self.settle(before)
    }

    /// 处理静默超时的会话：转入 `stale`，而不是直接删掉。
    ///
    /// 由 sweep 任务定期调用。这是「智能体消失后灯还会错多久」的唯一闸门。
    pub fn expire(&mut self, now: Instant) -> bool {
        let before = self.visible();

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

        self.settle(before)
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
        let mut sessions: Vec<SessionView> = self
            .sessions
            .values()
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
                stale_progress: (s.state == Signal::Stale).then(|| {
                    let remaining = s.expires_at.saturating_duration_since(now);
                    (remaining.as_secs_f32() / STALE_GRACE.as_secs_f32()).clamp(0.0, 1.0)
                }),
            })
            .collect();
        sessions.sort_by(|a, b| {
            b.state.priority().cmp(&a.state.priority()).then_with(|| a.id.cmp(&b.id))
        });

        Snapshot {
            aggregate: self.aggregate(),
            paused: self.paused,
            since_ms: self
                .since
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
            message: self.winner().and_then(|s| s.message.clone()),
            sessions,
        }
    }

    /// 优先级最高者；平局时取最近进入该状态的，再平局取 id 最小者，保证结果确定。
    fn winner(&self) -> Option<&Session> {
        self.sessions.values().max_by(|a, b| {
            a.state
                .priority()
                .cmp(&b.state.priority())
                .then_with(|| a.since.cmp(&b.since))
                .then_with(|| b.id.cmp(&a.id))
        })
    }

    /// 用于前后比对：只要会话集合、聚合值或 tooltip 文本变了就返回不同的值。
    fn visible(&self) -> Visible {
        let mut sessions: Vec<(String, Signal)> =
            self.sessions.iter().map(|(k, s)| (k.clone(), s.state)).collect();
        // 按会话名排序即可得到规范顺序（会话名唯一），无需 Signal 实现 Ord
        sessions.sort_by(|a, b| a.0.cmp(&b.0));
        Visible {
            aggregate: self.aggregate(),
            paused: self.paused,
            message: self.winner().and_then(|s| s.message.clone()),
            sessions,
        }
    }

    fn settle(&mut self, before: Visible) -> bool {
        let changed = self.visible() != before;
        if changed {
            self.since = Some(SystemTime::now());
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rep(s: &mut Store, t: Instant, session: &str, state: Signal) -> bool {
        let mut req = Report::new(state);
        req.session = Some(session.to_string());
        s.apply(t, req)
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
        assert!(s
            .snapshot()
            .sessions
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
        assert!(s.snapshot().sessions.is_empty(), "不该留下 stale 会话");
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
        s.apply(t, first);

        let mut second = Report::new(Signal::Working);
        second.session = Some("a".into());
        second.message = Some("cargo build".into());
        assert!(s.apply(t, second), "tooltip 文本变了，UI 需要更新");
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
        s.apply(t, first);

        let mut second = Report::new(Signal::Thinking);
        second.session = Some("a".into());
        s.apply(t, second);

        let v = &s.snapshot().sessions[0];
        assert_eq!(v.agent.as_deref(), Some("claude-code"));
        assert_eq!(v.cwd.as_deref(), Some("/proj"));
        assert_eq!(v.message, None);
    }

    #[test]
    fn paused_shows_off_but_keeps_sessions() {
        let (mut s, t) = (Store::default(), Instant::now());
        rep(&mut s, t, "a", Signal::Working);

        assert!(s.set_paused(true));
        assert_eq!(s.aggregate(), Signal::Off);

        assert!(s.set_paused(false));
        assert_eq!(s.aggregate(), Signal::Working, "恢复后应立即回到真实状态");
    }

    #[test]
    fn explicit_ttl_overrides_the_short_working_window() {
        let (mut s, t) = (Store::default(), Instant::now());
        let mut req = Report::new(Signal::Working);
        req.session = Some("long".into());
        req.ttl_ms = Some(3_600_000);
        s.apply(t, req);

        assert!(!s.expire(t + Duration::from_secs(1800)));
        assert_eq!(s.aggregate(), Signal::Working);
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
        assert_eq!(s.snapshot().sessions.len(), 1, "stale 会话要保留，不是删掉");
    }

    /// 会话静默超时后转 stale，而不是消失 —— 这是「智能体不说话了」和
    /// 「智能体空闲待命」能分开显示的关键
    #[test]
    fn a_silent_working_session_becomes_stale_not_gone() {
        let (mut s, t) = (Store::default(), Instant::now());
        rep(&mut s, t, "a", Signal::Working);

        assert!(s.expire(t + Signal::Working.default_ttl()));

        let view = &s.snapshot().sessions[0];
        assert_eq!(view.state, Signal::Stale);
        assert_eq!(view.id, "a", "会话还在，只是标记为沉默");
        assert_eq!(s.aggregate(), Signal::Stale);
    }

    #[test]
    fn a_stale_session_is_dropped_after_its_grace_period() {
        let (mut s, t) = (Store::default(), Instant::now());
        rep(&mut s, t, "a", Signal::Working);
        let ttl = Signal::Working.default_ttl();

        s.expire(t + ttl);
        assert_eq!(s.aggregate(), Signal::Stale);

        assert!(!s.expire(t + ttl + Duration::from_secs(299)));
        assert_eq!(s.aggregate(), Signal::Stale, "宽限期内仍应保留");

        assert!(s.expire(t + ttl + Duration::from_secs(301)));
        assert!(s.snapshot().sessions.is_empty());
        assert_eq!(s.aggregate(), Signal::Offline);
    }

    /// 一路会话还活着（idle）时，不该被另一路已经沉默的会话盖成灰的
    #[test]
    fn a_live_idle_session_outranks_a_stale_one() {
        let (mut s, t) = (Store::default(), Instant::now());
        rep(&mut s, t, "silent", Signal::Working);
        rep(&mut s, t, "alive", Signal::Idle);

        s.expire(t + Signal::Working.default_ttl());

        let states: Vec<Signal> = s.snapshot().sessions.iter().map(|v| v.state).collect();
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
        assert_eq!(s.snapshot_at(t).sessions[0].stale_progress, None);

        // 刚转 stale：还剩满额宽限
        s.expire(t + ttl);
        let fresh = s.snapshot_at(t + ttl).sessions[0].stale_progress.unwrap();
        assert!(fresh > 0.98, "刚转 stale 应接近 1.0，实际 {fresh}");

        // 宽限期过半
        let half = s
            .snapshot_at(t + ttl + Duration::from_secs(150))
            .sessions[0]
            .stale_progress
            .unwrap();
        assert!((0.4..0.6).contains(&half), "过半时应接近 0.5，实际 {half}");

        // 到点：归零
        let gone = s
            .snapshot_at(t + ttl + Duration::from_secs(300))
            .sessions[0]
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

    /// 每个状态都必须有 TTL。工作类状态的窗口尤其要短 —— 它决定了
    /// 「智能体消失后灯还会错多久」。
    #[test]
    fn every_state_has_a_ttl_and_working_is_short() {
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
        assert!(Signal::Working.default_ttl() <= Duration::from_secs(120));
        assert!(Signal::Thinking.default_ttl() <= Duration::from_secs(120));
    }

    #[test]
    fn off_state_clears_the_session() {
        let (mut s, t) = (Store::default(), Instant::now());
        rep(&mut s, t, "a", Signal::Error);
        assert!(rep(&mut s, t, "a", Signal::Off), "清除应报告变化");
        assert!(s.snapshot().sessions.is_empty());
        assert_eq!(s.aggregate(), Signal::Offline);
    }

    #[test]
    fn clear_is_idempotent() {
        let (mut s, t) = (Store::default(), Instant::now());
        rep(&mut s, t, "a", Signal::Error);
        assert!(s.clear("a"));
        assert!(!s.clear("a"));
    }

    #[test]
    fn remove_session_reports_removed_and_visible_change_separately() {
        let (mut s, t) = (Store::default(), Instant::now());
        rep(&mut s, t, "a", Signal::Error);
        rep(&mut s, t, "b", Signal::Idle);

        // 移除的是没在决定聚合的那一路：聚合不变，但会话列表变了
        let (removed, changed) = s.remove_session("b");
        assert!(removed);
        assert!(changed, "会话表变了，前端需要更新");
        assert_eq!(s.aggregate(), Signal::Error, "聚合不该被影响");

        // 再删一次：既没移除，也没有任何可见变化
        assert_eq!(s.remove_session("b"), (false, false));

        // 删掉最后一个会话，就没有任何会话在报了
        assert_eq!(s.remove_session("a"), (true, true));
        assert_eq!(s.aggregate(), Signal::Offline);
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
