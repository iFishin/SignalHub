//! 从灯上审批工具调用（`docs/plan-round4.md` C3）。
//!
//! **这是全项目唯一会改变智能体行为的功能**，所以它的形状是刻意收窄的：
//!
//! - 只在 Claude Code 的 `PermissionRequest` 事件上工作 —— 那正是「它本来就要问你」
//!   的那一刻。我们做的是**把终端里的那个确认框搬到灯上**，不是替智能体做决定，
//!   也不是拦截每一个工具调用（`PreToolUse` 那种会拦下本来不需要批准的事）。
//! - 默认关（`Settings.approve_tools`）。关着的时候端点**照常回话但什么都不说**
//!   （`{}` = 没有决定），于是 Claude Code 走它自己的询问流程 —— 装了 hook 也不改变
//!   任何行为。
//! - 没人点（超时）时同样回 `{}`：**交回 Claude Code 自己问**。永远不替用户说「允许」。
//! - 看全了才给「允许」：详情被截断过的（长命令 / 大输入）只能拒绝，或者回终端自己看。
//!   这是「白名单动作模式」的落点 —— 一条在灯上看不全的命令不该被一键放行。
//! - **拒绝规则永远优先**是对方保证的（官方文档：hook 返回 `allow` 也压不过匹配的
//!   deny 规则），我们这边只需要保证自己**只在被问的时候才答**。
//!
//! 端点在 `server.rs`（`POST /v1/approve/{agent}`，**要 token**，和免鉴权的上报端点
//! 分开）；装进 Claude Code 配置里的那一条见 `hooks::install_approve`。

use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 我们等用户点一下的上限。
///
/// **必须比写进 Claude Code 配置里的 `timeout` 短**（那边是 60 秒），否则对方会先超时，
/// 而它超时的行为（`onFailure: continue`）虽然也是「回到终端里问」，但那一次就白等了。
/// 45 秒是「人在桌边会点一下」和「人不在就别挡路」之间的取舍。
pub const WAIT: Duration = Duration::from_secs(45);

/// 用户点了什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny,
}

/// 一条等着的审批（内部形态，`created` 是 `Instant` 所以不直接序列化）。
#[derive(Debug, Clone)]
pub struct Pending {
    pub id: String,
    pub session: String,
    pub tool: String,
    /// 牌上那行字：`hookmap::tool_target` 的短词。
    pub target: String,
    /// 气泡里的详情：`hookmap::approval_detail`。
    pub detail: String,
    /// 详情被截断过 —— **不给「允许」**（见 `allowable`）。
    pub truncated: bool,
    created: Instant,
}

impl Pending {
    pub fn new(id: String, session: String, tool: String, target: String, detail: String, truncated: bool) -> Self {
        Self {
            id,
            session,
            tool,
            target,
            detail,
            truncated,
            created: Instant::now(),
        }
    }

    /// 能不能在灯上放行。**看不全就不给放行** —— 这是这个功能里唯一一条关于「允不允许
    /// 允许」的规则，所以它单独是一个函数、也单独有一条用例。
    pub fn allowable(&self) -> bool {
        !self.truncated
    }

    fn view(&self, now: Instant) -> PendingView {
        PendingView {
            id: self.id.clone(),
            session: self.session.clone(),
            tool: self.tool.clone(),
            target: self.target.clone(),
            detail: self.detail.clone(),
            truncated: self.truncated,
            allowable: self.allowable(),
            age_ms: now.saturating_duration_since(self.created).as_millis() as u64,
        }
    }
}

/// 发给灯的那一份（气泡里的全部内容）。
#[derive(Debug, Clone, Serialize)]
pub struct PendingView {
    pub id: String,
    pub session: String,
    pub tool: String,
    pub target: String,
    pub detail: String,
    pub truncated: bool,
    /// 详情看全了才为 `true`（前端据此决定要不要画「允许」那个按钮）。
    pub allowable: bool,
    pub age_ms: u64,
}

/// 决定的结果。**「看不全就不能放行」必须在这里挡住** —— 灯上的按钮、HTTP 接口、
/// 以后的任何入口都走这一个函数，规则只有一处才不会被绕过去。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decided {
    /// 送出去了
    Done,
    /// 没有这一条（多半是刚超时，或者被同一个会话的新请求顶掉了）
    NotFound,
    /// 这条的详情是截断过的 —— **只允许拒绝**
    NotAllowable,
}

struct Entry {
    pending: Pending,
    /// 谁来接这个决定。**丢掉发送端 = 对方收到「没有决定」**，所以「顶掉旧的那条」
    /// 和「会话结束了」都只是把它 drop 掉，不需要一条专门的取消路径。
    tx: tokio::sync::oneshot::Sender<Decision>,
}

pub type SharedApprovals = Arc<Registry>;

/// 登记表。**一个会话同时最多一条**：同一个会话里又来了一个要批准的调用，说明前一个
/// 已经不作数了（对方在等前一个的时候不会发起下一个，所以这只可能是重试或者乱序）。
#[derive(Default)]
pub struct Registry {
    entries: Mutex<HashMap<String, Entry>>,
    /// 变了就喊一声（灯上那个气泡据此刷新）。
    ///
    /// **服务器那边不持有 `AppHandle`**（见 `server::spawn` 的注释：它要能脱离 app
    /// 单测），所以这件事由 `lib.rs` 在构造时挂进来 —— 和 `Notifier` 同一个套路。
    on_change: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl Registry {
    /// 挂一个「变了」的回调（只有一处调用点，见 `lib.rs`）。
    pub fn on_change(&self, f: Arc<dyn Fn() + Send + Sync>) {
        *self.on_change.lock().expect("approvals poisoned") = Some(f);
    }

    /// 喊一声。**必须在放下 `entries` 锁之后调** —— 回调最终会走到前端的 IPC，
    /// 而前端会回头读 `list()`，在锁里喊就是一次自找的卡死。
    fn changed(&self) {
        let hook = self.on_change.lock().expect("approvals poisoned").clone();
        if let Some(f) = hook {
            f();
        }
    }

    /// 登记一条待批准，返回等决定的那一头。
    pub fn open(&self, pending: Pending) -> tokio::sync::oneshot::Receiver<Decision> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let mut entries = self.entries.lock().expect("approvals poisoned");
            // 顶掉同会话旧的那条：drop 掉它的 tx，那边立刻按「没有决定」回话
            entries.insert(pending.session.clone(), Entry { pending, tx });
        }
        self.changed();
        rx
    }

    /// 用户点了（或者接口被调了）。**对不上时什么也不做**：那条可能刚好超时了，
    /// 或者被同一个会话的新请求顶掉了。
    pub fn decide(&self, id: &str, decision: Decision) -> Decided {
        let outcome = {
            let mut entries = self.entries.lock().expect("approvals poisoned");
            let Some(session) = entries
                .iter()
                .find(|(_, entry)| entry.pending.id == id)
                .map(|(session, _)| session.clone())
            else {
                return Decided::NotFound;
            };
            // 「看不全就不给放行」：拒绝永远可以，放行要看全了才行
            if decision == Decision::Allow
                && !entries.get(&session).expect("刚查过").pending.allowable()
            {
                return Decided::NotAllowable;
            }
            let Some(entry) = entries.remove(&session) else {
                return Decided::NotFound;
            };
            // 收不到就说明对面已经走了（超时），那也没什么可做的
            let _ = entry.tx.send(decision);
            Decided::Done
        };
        self.changed();
        outcome
    }

    /// 会话结束了：那一条不再作数（对方收到「没有决定」，走它自己的流程）。
    pub fn forget_session(&self, session: &str) {
        let dropped = self
            .entries
            .lock()
            .expect("approvals poisoned")
            .remove(session)
            .is_some();
        if dropped {
            self.changed();
        }
    }

    /// 全部忘掉（关掉审批开关时用）。
    pub fn clear(&self) {
        let dropped = {
            let mut entries = self.entries.lock().expect("approvals poisoned");
            let n = entries.len();
            entries.clear();
            n
        };
        if dropped > 0 {
            self.changed();
        }
    }

    pub fn list(&self) -> Vec<PendingView> {
        let now = Instant::now();
        let mut entries = self.entries.lock().expect("approvals poisoned");
        // **顺手把过期的扫掉**：正常超时由 handler 自己收（见 `expire`），但那条路
        // 有一种走不到的情况 —— 请求方在等待中途断开（`curl -m`、Claude Code 被杀），
        // handler 那个 future 直接被丢掉，`expire` 永远不执行。那种条目留在表里的
        // 表现是**灯上永远挂着一条没人等的审批**，而点它什么也不会发生。
        //
        // 判据是 `age > WAIT`：超过这个岁数的条目，等它的那一头一定已经按超时走了
        // （哪怕只是早了几微秒 —— 那时它收到的也是「没有决定」，和这里丢掉的结果一样）。
        //
        // **不喊 `changed()`**：这里是读路径，而前端正因为读了它才走到这儿 ——
        // 喊一声就是「刷新 → 发现过期 → 再喊 → 再刷新」。前端这一轮拿到的列表已经
        // 是对的，不需要第二次。
        entries.retain(|_, entry| now.duration_since(entry.pending.created) <= WAIT);
        let mut views: Vec<PendingView> = entries.values().map(|entry| entry.pending.view(now)).collect();
        // 等了最久的那条排最前 —— 气泡里第一条就是最该先看的那条
        views.sort_by_key(|view| std::cmp::Reverse(view.age_ms));
        views
    }

    /// 那一条已经过期了，把它收掉（**只收 id 对得上的那一条**）。
    ///
    /// 由 handler 在超时之后调。**必须按 id 收，不能按会话**：同一个会话可能在旧的
    /// 那条还没收干净时又来了新的一条（新的会顶掉旧的），按会话收就会把**新的**
    /// 那条一起收掉 —— 表现是「用户刚看到气泡，它自己没了」。
    pub fn expire(&self, id: &str) {
        let dropped = {
            let mut entries = self.entries.lock().expect("approvals poisoned");
            let Some(session) = entries
                .iter()
                .find(|(_, entry)| entry.pending.id == id)
                .map(|(session, _)| session.clone())
            else {
                return;
            };
            entries.remove(&session).is_some()
        };
        if dropped {
            self.changed();
        }
    }
}

/// 回给 Claude Code 的那段 JSON。
///
/// `None` = 没有决定 —— **这必须是超时和关掉开关时的答案**：官方文档里「2xx + 空对象」
/// 等价于「hook 什么都没说」，于是对方走它自己的询问流程（也就是计划里说的「超时自动
/// 交回 Claude Code 自己问」）。
///
/// `PermissionRequest` 只有 `allow` / `deny` 两种行为，**没有 `ask`** —— 「回到自己问」
/// 只能靠不给决定来表达。
pub fn response(decision: Option<Decision>) -> Value {
    match decision {
        Some(Decision::Allow) => json!({
            "hookSpecificOutput": {
                "hookEventName": "PermissionRequest",
                "decision": { "behavior": "allow" },
            }
        }),
        Some(Decision::Deny) => json!({
            "hookSpecificOutput": {
                "hookEventName": "PermissionRequest",
                "decision": {
                    "behavior": "deny",
                    // 这条是**给模型看的**（官方文档：deny 的 message 会交给 Claude），
                    // 所以用英文 —— 它进的是模型的上下文，不是界面文案。
                    "message": "Denied from SignalHub (the desk lamp). Do not retry this call; ask the user what to do instead.",
                },
            }
        }),
        None => json!({}),
    }
}

/// 等一个决定，最多 `WAIT`。`None` = 没人点（超时 / 开关被关掉 / 被顶掉）。
pub async fn wait(rx: tokio::sync::oneshot::Receiver<Decision>) -> Option<Decision> {
    match tokio::time::timeout(WAIT, rx).await {
        Ok(Ok(decision)) => Some(decision),
        // 发送端没了（顶掉 / 清空）或者超时 —— 两种都是「没有决定」
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 写进 Claude Code 配置里的超时**必须比我们内部那个截止时间长**：反过来的话
    /// 对方会先超时，我们那个「等用户点」的窗口永远走不满 —— 表现是「灯上的按钮
    /// 点不动」。两个常量在两个文件里，所以这条关系只能靠一条用例钉住。
    #[test]
    fn the_configured_timeout_is_longer_than_our_own_deadline() {
        assert!(
            u64::from(crate::hooks::APPROVE_TIMEOUT_SECS) > WAIT.as_secs(),
            "配置里的超时要留出余量（现在 {} 秒 vs 内部 {} 秒）",
            crate::hooks::APPROVE_TIMEOUT_SECS,
            WAIT.as_secs()
        );
    }

    fn pending(id: &str, session: &str) -> Pending {
        Pending::new(
            id.to_string(),
            session.to_string(),
            "Bash".to_string(),
            "rm".to_string(),
            "rm -rf build".to_string(),
            false,
        )
    }

    /// 回了话之后，那条就不该再挂在灯上（否则用户会对着一条已经处理过的气泡点）。
    #[test]
    fn deciding_delivers_the_decision_and_clears_the_entry() {
        let registry = Registry::default();
        let mut rx = registry.open(pending("a1", "s1"));
        assert_eq!(registry.list().len(), 1);

        assert_eq!(registry.decide("a1", Decision::Allow), Decided::Done);
        assert_eq!(rx.try_recv(), Ok(Decision::Allow));
        assert!(registry.list().is_empty());

        // 同一个 id 再点一次：已经没有那一条了
        assert_eq!(registry.decide("a1", Decision::Allow), Decided::NotFound);
    }

    /// 同一个会话里新来的那条把旧的顶掉 —— 旧的收到「没有决定」，不残留。
    #[test]
    fn a_new_request_for_the_same_session_releases_the_old_one() {
        let registry = Registry::default();
        let mut old = registry.open(pending("a1", "s1"));
        let _new = registry.open(pending("a2", "s1"));

        assert!(old.try_recv().is_err(), "旧的那条该被放掉");
        let views = registry.list();
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].id, "a2");
    }

    /// 会话结束 = 那条不再作数（对方回「没有决定」，自己问去）。
    #[test]
    fn forgetting_a_session_drops_its_pending() {
        let registry = Registry::default();
        let mut rx = registry.open(pending("a1", "s1"));
        registry.forget_session("s1");
        assert!(rx.try_recv().is_err());
        assert!(registry.list().is_empty());
        // 别的会话不受影响
        let mut other = registry.open(pending("a2", "s2"));
        registry.forget_session("s1");
        assert!(registry.list().len() == 1);
        assert_eq!(registry.decide("a2", Decision::Deny), Decided::Done);
        assert_eq!(other.try_recv(), Ok(Decision::Deny));
    }

    /// **看不全就不给放行。** 这是这个功能唯一一条关于「允不允许允许」的规则。
    #[test]
    fn a_truncated_detail_cannot_be_allowed() {
        let mut long = pending("a1", "s1");
        long.truncated = true;
        long.detail = "rm -rf …".to_string();
        assert!(!long.allowable());
        assert!(!long.view(Instant::now()).allowable);

        let short = pending("a2", "s2");
        assert!(short.allowable());
    }

    /// 过期的条目会被扫掉，而且**不喊 changed**（读路径不能触发第二次刷新）。
    #[test]
    fn list_sweeps_entries_past_their_deadline() {
        let registry = Registry::default();
        let changes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = changes.clone();
        registry.on_change(Arc::new(move || {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }));

        // 手工造一条「已经等了 WAIT + 1 秒」的
        let mut stale = pending("old", "s1");
        stale.created = Instant::now() - WAIT - Duration::from_secs(1);
        registry.open(stale);
        assert_eq!(changes.load(std::sync::atomic::Ordering::SeqCst), 1, "开一条要喊一声");

        assert!(registry.list().is_empty(), "过了截止时间的那条不该再出现在列表里");
        assert_eq!(
            changes.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "扫除是读路径，不该再喊一声（喊了就是刷新死循环）"
        );
    }

    /// `expire` 按 id 收：**新来的那条不能被旧的超时带走**。
    #[test]
    fn expire_only_drops_the_entry_with_that_id() {
        let registry = Registry::default();
        let mut rx = registry.open(pending("old", "s1"));
        // 同一个会话又来了一条（顶掉旧的），此时旧的那条按超时走
        let _fresh = registry.open(pending("new", "s1"));

        registry.expire("old");
        assert_eq!(
            registry.list().len(),
            1,
            "收的是 id 对不上的那一条 —— 新的必须还在"
        );
        assert_eq!(registry.list()[0].id, "new");
        // 旧的那一头收到的是「没有决定」（它的 tx 被顶掉时 drop 了）
        assert!(rx.try_recv().is_err());
    }

    /// 规则在**登记表这一层**就挡住，不只是界面上不画那个按钮 ——
    /// 否则「脚本绕过界面」就是一条真的能一键放行长命令的路。
    #[test]
    fn the_registry_refuses_to_allow_a_truncated_detail() {
        let registry = Registry::default();
        let mut long = pending("a1", "s1");
        long.truncated = true;
        let mut rx = registry.open(long);

        assert_eq!(registry.decide("a1", Decision::Allow), Decided::NotAllowable);
        assert!(rx.try_recv().is_err(), "没送出去，对方还在等");
        // 拒绝照样可以，而且那一条跟着消失
        assert_eq!(registry.decide("a1", Decision::Deny), Decided::Done);
        assert_eq!(rx.try_recv(), Ok(Decision::Deny));
    }

    /// 回给 Claude Code 的字段名和取值 —— 写错一个的表现是「审批永远不生效」，
    /// 而那看起来就像「灯上的按钮坏了」。
    #[test]
    fn the_response_matches_the_documented_shape() {
        let allow = response(Some(Decision::Allow));
        assert_eq!(allow["hookSpecificOutput"]["hookEventName"], "PermissionRequest");
        assert_eq!(allow["hookSpecificOutput"]["decision"]["behavior"], "allow");

        let deny = response(Some(Decision::Deny));
        assert_eq!(deny["hookSpecificOutput"]["hookEventName"], "PermissionRequest");
        assert_eq!(deny["hookSpecificOutput"]["decision"]["behavior"], "deny");
        assert!(
            deny["hookSpecificOutput"]["decision"]["message"].is_string(),
            "拒绝要带一句话给模型"
        );
        // 拒绝**不能**顺手带上 updatedInput / updatedPermissions（那是放行才有的东西）
        assert!(deny["hookSpecificOutput"]["decision"]["updatedInput"].is_null());

        // 没有决定 = 空对象：对方照自己的流程问
        assert_eq!(response(None), json!({}));
    }

    /// 列表按「等了多久」排，最久的在最前面。
    #[test]
    fn the_list_puts_the_oldest_first() {
        let registry = Registry::default();
        let _a = registry.open(pending("a1", "s1"));
        std::thread::sleep(Duration::from_millis(2));
        let _b = registry.open(pending("a2", "s2"));
        let views = registry.list();
        assert_eq!(views[0].id, "a1");
        assert_eq!(views[1].id, "a2");
    }
}
