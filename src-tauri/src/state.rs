use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;
use std::str::FromStr;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// 没有显式 TTL 的会话（即 idle）静默多久后被回收。
/// 只作用于无 TTL 的会话，因此不会与用户显式传入的 ttl_ms 冲突。
const IDLE_SESSION_GC: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Signal {
    Off,
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
            other => return Err(format!("unknown state {other:?}")),
        })
    }

    /// 聚合时取 max。数值本身无意义，只有相对顺序有意义。
    pub fn priority(self) -> u8 {
        match self {
            Signal::Off => 0,
            Signal::Idle => 1,
            Signal::Done => 5,
            Signal::Thinking => 15,
            Signal::Working => 20,
            Signal::Attention => 30,
            Signal::Permission => 40,
            Signal::Error => 50,
        }
    }

    /// 到期后会话被丢弃，聚合自然回落到下一个会话或 idle。
    /// None 表示最后写入者胜、不自动过期。
    pub fn default_ttl(self) -> Option<Duration> {
        match self {
            Signal::Off | Signal::Idle => None,
            Signal::Done => Some(Duration::from_secs(30)),
            Signal::Thinking => Some(Duration::from_secs(120)),
            Signal::Working => Some(Duration::from_secs(300)),
            Signal::Attention => Some(Duration::from_secs(300)),
            Signal::Permission => Some(Duration::from_secs(600)),
            Signal::Error => Some(Duration::from_secs(300)),
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
    /// 单调时钟，用于 TTL 判定
    expires_at: Option<Instant>,
    /// 最后一次上报时间，用于空闲回收
    last_seen: Instant,
    /// 进入当前状态的时间，用于 UI 的「已持续多久」
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
        let expires_at = req
            .ttl_ms
            .map(Duration::from_millis)
            .or_else(|| req.state.default_ttl())
            .map(|ttl| now + ttl);

        self.sessions.insert(
            session.clone(),
            Session {
                id: session,
                state: req.state,
                agent,
                cwd,
                message: req.message,
                expires_at,
                last_seen: now,
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

    /// 丢弃过期会话，并回收长时间无事件的会话。由 sweep 任务定期调用。
    pub fn expire(&mut self, now: Instant) -> bool {
        let before = self.visible();
        self.sessions.retain(|_, s| match s.expires_at {
            Some(expires_at) => now < expires_at,
            None => now.saturating_duration_since(s.last_seen) < IDLE_SESSION_GC,
        });
        self.settle(before)
    }

    /// 聚合：跨会话取最高优先级 —— 「红 > 黄 > 绿」就实现在这里。
    /// 这样别的会话报绿不会顶掉正在等授权的那一路红，而等授权的那一路
    /// 自己恢复工作后红又自然消失，不会卡在红上。
    pub fn aggregate(&self) -> Signal {
        if self.paused {
            return Signal::Off;
        }
        self.winner().map(|s| s.state).unwrap_or(Signal::Idle)
    }

    pub fn snapshot(&self) -> Snapshot {
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

    #[test]
    fn empty_store_is_idle() {
        let s = Store::default();
        assert_eq!(s.aggregate(), Signal::Idle);
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

    #[test]
    fn done_expires_back_to_idle() {
        let (mut s, t) = (Store::default(), Instant::now());
        rep(&mut s, t, "a", Signal::Done);

        assert!(!s.expire(t + Duration::from_secs(29)));
        assert_eq!(s.aggregate(), Signal::Done);

        assert!(s.expire(t + Duration::from_secs(31)));
        assert_eq!(s.aggregate(), Signal::Idle);
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
    fn explicit_ttl_protects_a_long_task_from_idle_gc() {
        let (mut s, t) = (Store::default(), Instant::now());
        let mut req = Report::new(Signal::Working);
        req.session = Some("long".into());
        req.ttl_ms = Some(3_600_000);
        s.apply(t, req);

        assert!(!s.expire(t + Duration::from_secs(1800)));
        assert_eq!(s.aggregate(), Signal::Working);
    }

    #[test]
    fn idle_sessions_are_gc_ed_after_silence() {
        let (mut s, t) = (Store::default(), Instant::now());
        rep(&mut s, t, "a", Signal::Idle);
        assert_eq!(s.snapshot().sessions.len(), 1);

        assert!(s.expire(t + IDLE_SESSION_GC + Duration::from_secs(1)));
        assert!(s.snapshot().sessions.is_empty());
        assert_eq!(s.aggregate(), Signal::Idle);
    }

    #[test]
    fn off_state_clears_the_session() {
        let (mut s, t) = (Store::default(), Instant::now());
        rep(&mut s, t, "a", Signal::Error);
        assert!(rep(&mut s, t, "a", Signal::Off), "清除应报告变化");
        assert!(s.snapshot().sessions.is_empty());
        assert_eq!(s.aggregate(), Signal::Idle);
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

        // 删掉最后一个会话，聚合回到 idle
        assert_eq!(s.remove_session("a"), (true, true));
        assert_eq!(s.aggregate(), Signal::Idle);
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
    }

    #[test]
    fn priority_order_matches_spec() {
        let ascending = [
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
        assert_eq!(Signal::Off.priority(), 0, "off 最低");
    }
}
