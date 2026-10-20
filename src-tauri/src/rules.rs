//! **预设规则**（B4）—— 三个写死的判断，不是通用规则引擎。
//!
//! 计划里说得很清楚：表达式 / DSL 引擎会违背「宁可少而精」，所以这里只有三条
//! 用户能开关的固定规则，每条都对应一个真实场景：
//!
//! 1. **授权卡住超过 60 秒 → 用出站钩子再推一次手机**（[`stuck_episodes`]）。
//! 2. **有智能体在思考 / 执行 → 暂停通知**（[`silence`] 的 `Busy`）。
//! 3. **会议中 → 灯上那一组变蓝 + 暂停通知**（[`silence`] 的 `Meeting`；蓝色那半
//!    在前端，见 `SignalLamp.tsx`）。
//!
//! 三条的共同点是**不改状态机**：规则只读快照，做的都是「要不要打扰你」这一层
//! 的决定。灯怎么亮、组怎么排、钩子怎么配，仍然只有原来那一套在管。

use crate::config::Settings;
use crate::state::{Signal, Snapshot};
use std::collections::HashSet;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager};

/// 预设 1 的阈值。**60 秒**是计划里定的：一闪而过的权限请求不值得吵你，
/// 一分钟还没人处理才值得。
pub const STUCK_AFTER: Duration = Duration::from_secs(60);

/// 规则线程的节拍。和采集器同一个下限（≥ 5 秒）：它检查的是「已经卡了多久」，
/// 早半秒晚半秒没有任何区别，没必要更勤。
const TICK: Duration = Duration::from_secs(5);

/// 通知为什么被暂停。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Silence {
    /// 会议中（预设 3）。
    Meeting,
    /// 有智能体正在思考 / 执行（预设 2）。
    Busy,
}

/// 现在该不该暂停通知。
///
/// 两条都关着时永远是 `None` —— 默认行为一点不变。会议优先于忙碌：两个都成立时
/// 原因算「会议」，日志和未来的界面文案才不会说成「有会话在跑」。
///
/// **只管「打扰」这一层**：系统通知和重复提醒会停，灯照常显示、上报照常进状态机、
/// 出站钩子照常触发（钩子是用户配置的程序化集成，不是打扰）。
pub fn silence(snapshot: &Snapshot, settings: &Settings) -> Option<Silence> {
    if settings.rule_meeting && meeting_running(snapshot) {
        return Some(Silence::Meeting);
    }
    if settings.rule_busy_silence && busy(snapshot) {
        return Some(Silence::Busy);
    }
    None
}

/// 会议组里有没有活着的会话。
///
/// 判据是**采集器的固定 session id**，不是组名：组名跟着界面语言变（「会议」/
/// 「Meeting」），还可能被用户改名，拿名字认人迟早认错。
pub fn meeting_running(snapshot: &Snapshot) -> bool {
    snapshot.groups.iter().flat_map(|g| &g.sessions).any(|s| {
        s.id == crate::collect::Collector::Meeting.id()
    })
}

/// 有没有智能体正在干活。
///
/// **采集器自己的会话不算**：会议中、备份时它们也报 `working`，但那是机器状态，
/// 不是「有智能体在跑」。把它们算进来的话，开个备份就会让通知静音 —— 没人想要。
pub fn busy(snapshot: &Snapshot) -> bool {
    snapshot
        .groups
        .iter()
        .flat_map(|g| &g.sessions)
        .filter(|s| s.agent.as_deref() != Some(crate::collect::AGENT))
        .any(|s| matches!(s.state, Signal::Thinking | Signal::Working))
}

/// 这一跳里「卡在授权、而且已经够久」的组，返回 `(组名, 这一轮的起点)`。
///
/// `since_ms` 是**这一轮的身份证**：同一组离开授权又回来，起点会变，于是会被
/// 当成新的一轮、重新推一次。抽出纯函数是为了能直接断言阈值边界。
pub fn stuck_episodes(snapshot: &Snapshot, now_ms: u64) -> Vec<(String, u64)> {
    let threshold = STUCK_AFTER.as_millis() as u64;
    snapshot
        .groups
        .iter()
        .filter(|g| g.aggregate == Signal::Permission)
        .filter(|g| now_ms.saturating_sub(g.since_ms) >= threshold)
        .map(|g| (g.name.clone(), g.since_ms))
        .collect()
}

/// 起规则线程。**一条线程、5 秒一拍、错自己吞掉** —— 和采集器同一套纪律。
///
/// 即使三条预设全关也照常跑：每一拍只读一下设置就跳过，代价比「设置变了再唤醒」
/// 那套通知机制小得多，也不会漏掉用户刚打开开关的那种情况。
pub fn spawn(app: AppHandle) {
    thread::spawn(move || run(&app));
}

fn run(app: &AppHandle) {
    // 已经推过的「轮次」。键是 (组名, 这一轮起点)：起点变了就是新的一轮。
    let mut pushed: HashSet<(String, u64)> = HashSet::new();

    loop {
        thread::sleep(TICK);
        let settings = crate::current_settings(app);

        if !settings.rule_stuck_push {
            pushed.clear();
            continue;
        }

        let snapshot = app
            .state::<crate::SharedStore>()
            .lock()
            .expect("state store poisoned")
            .snapshot();
        let current: HashSet<(String, u64)> = stuck_episodes(&snapshot, now_ms())
            .into_iter()
            .collect();

        for (group, since) in &current {
            let key = (group.clone(), *since);
            if pushed.contains(&key) {
                continue;
            }
            let sent = crate::outbound::remind(
                &settings,
                &snapshot,
                Signal::Permission,
                group,
                crate::current_lang(app),
                app.state::<crate::SharedOutbound>().inner(),
            );
            // 没配钩子时 sent = 0：不记「推过了」——用户可能一会儿才去配，
            // 配好之后这一轮还卡着的话，应当补上。每 5 秒重试一次很便宜。
            if sent > 0 {
                pushed.insert(key);
            }
        }

        // 这一轮已经结束的键不能一直留着：同样的 (组名, 起点) 不会再来，但集合
        // 会长；重开开关时也要能重新推。
        pushed.retain(|key| current.contains(key));
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{GroupView, SessionView};

    fn session(id: &str, state: Signal, agent: Option<&str>) -> SessionView {
        SessionView {
            id: id.to_string(),
            state,
            agent: agent.map(str::to_string),
            cwd: None,
            message: None,
            age_ms: 0,
            stale_progress: None,
        }
    }

    fn group(name: &str, aggregate: Signal, since_ms: u64, sessions: Vec<SessionView>) -> GroupView {
        GroupView {
            name: name.to_string(),
            default_label: None,
            aggregate,
            message: None,
            since_ms,
            stale_progress: None,
            sessions,
        }
    }

    fn snapshot(groups: Vec<GroupView>) -> Snapshot {
        Snapshot {
            aggregate: groups
                .iter()
                .map(|g| g.aggregate)
                .max_by_key(|s| s.priority())
                .unwrap_or(Signal::Idle),
            paused: false,
            since_ms: 0,
            message: None,
            groups,
        }
    }

    fn on() -> Settings {
        Settings {
            rule_stuck_push: true,
            rule_busy_silence: true,
            rule_meeting: true,
            ..Settings::default()
        }
    }

    #[test]
    fn everything_off_means_no_silence() {
        let snap = snapshot(vec![group(
            "meeting",
            Signal::Working,
            0,
            vec![session("meeting", Signal::Working, Some("system"))],
        )]);
        assert_eq!(silence(&snap, &Settings::default()), None);
    }

    #[test]
    fn a_meeting_silences_by_session_id_not_group_name() {
        let settings = on();
        let snap = snapshot(vec![group(
            "随便改的名字",
            Signal::Working,
            0,
            vec![session("meeting", Signal::Working, Some("system"))],
        )]);
        assert_eq!(silence(&snap, &settings), Some(Silence::Meeting));
    }

    #[test]
    fn collector_sessions_do_not_count_as_agents_working() {
        let settings = Settings {
            rule_meeting: false,
            ..on()
        };
        let snap = snapshot(vec![group(
            "backup",
            Signal::Working,
            0,
            vec![session("backup", Signal::Working, Some("system"))],
        )]);
        assert_eq!(silence(&snap, &settings), None, "备份不是「有智能体在跑」");
    }

    #[test]
    fn agents_working_silence_notifications() {
        let settings = Settings {
            rule_meeting: false,
            ..on()
        };
        let snap = snapshot(vec![group(
            "default",
            Signal::Thinking,
            0,
            vec![session("abc", Signal::Thinking, Some("claude-code"))],
        )]);
        assert_eq!(silence(&snap, &settings), Some(Silence::Busy));
    }

    #[test]
    fn stuck_permission_only_after_sixty_seconds() {
        let snap = snapshot(vec![group("default", Signal::Permission, 1_000, vec![])]);
        assert!(stuck_episodes(&snap, 1_000 + 59_999).is_empty(), "差 1ms 都不算");
        assert_eq!(
            stuck_episodes(&snap, 1_000 + 60_000),
            vec![("default".to_string(), 1_000)],
            "到点才算，键里带这一轮的起点"
        );
        // 不是 permission 的组再久也不算
        let working = snapshot(vec![group("ci", Signal::Working, 0, vec![])]);
        assert!(stuck_episodes(&working, u64::MAX).is_empty());
    }
}
