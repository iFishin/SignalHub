//! 事件流水：给「回来之后」和「本周洞察」一份能回放的记录。
//!
//! `Snapshot` 只回答「现在是什么样」。一个会话在你离开的 47 分钟里出现、跑完、
//! 到期消失，快照里连一行都不剩 —— `awaySummary.ts` 第一版只能靠 `ageMs` 反推
//! 「当前这一行是不是离开之后变的」，所以 README 一直写着「没有事件流水，出现过
//! 又消失的会话这里看不到」。这份流水补的就是这一段。
//!
//! **它不是第二套状态机。** 事件只有一个来源：[`tap`] 收到的每一帧快照。那里是
//! 所有状态变化的唯一出口（`crate::mutate` 和 `sweep::spawn` 都汇到
//! `notify::compose`），所以 HTTP 上报、CLI、hook、手动状态、远程会话、TTL 过期
//! 全部自动认它，不用在每个 `Store::apply` 调用点各写一遍。挂在 `coalesce`
//! **外层**是刻意的：合帧会丢掉 100ms 内的中间帧，而流水要的就是每一帧。
//!
//! 形状沿用 `approval_log.rs`：每日一个 jsonl、追加型、坏一行只丢一条、`0o600`、
//! 留 30 天。文件枚举和日期换算直接复用那边的函数，不另写一份历法。
//!
//! **盘上不存命令原文。** `message` 是智能体自己写的那句短说明（上报接口本来就
//! 限了长度），再截到 200 字符；审批详情那种可能带密钥的东西不经过这里。
//! 写盘失败只记一行日志，绝不挡住灯 —— 流水是增强，不是状态机的依赖。

use crate::notify::Notifier;
use crate::state::Snapshot;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::AppHandle;

/// 消息落盘前截到多少字符。和 `approval_log::DETAIL_KEEP` 同一个口径。
const MESSAGE_KEEP: usize = 200;

/// 一条事件。`kind` 只有三种：`start` / `change` / `end`。
///
/// `end` 里的 `state` 是**最后已知状态**（不是「结束」这个状态）：`done` 的牌到期
/// 被删时，那条 `end` 就该说 `done`，否则「你不在的时候有几个跑完了」没法答。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// Unix 毫秒。界面按本地时区显示，存的是绝对时间。
    pub ts_ms: u64,
    pub kind: String,
    pub session: String,
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub agent: String,
    /// 来自哪台远程机器；空串 = 本机。
    #[serde(default)]
    pub machine: String,
    pub state: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub message: String,
}

fn clip(text: &str) -> String {
    text.chars().take(MESSAGE_KEEP).collect()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// 把连续的快照流翻译成事件流。只记「上一帧是什么」，不碰状态机。
#[derive(Default)]
pub struct Observer {
    prev: HashMap<String, Event>,
}

impl Observer {
    /// 与上一帧对比，返回这一帧新发生的事件。
    ///
    /// 只认**状态**变化，不认 `message` 变化：一轮工具调用里「正在跑 npm」→
    /// 「正在读文件」会改几十次说明，全写进流水只会把真正的跃迁淹掉。
    /// `start` / `change` 带的是这一刻的字段，`end` 带的是上一刻记住的字段。
    pub fn observe(&mut self, snapshot: &Snapshot, ts_ms: u64) -> Vec<Event> {
        let mut current: HashMap<String, Event> = HashMap::new();
        for group in &snapshot.groups {
            for session in &group.sessions {
                current.insert(
                    session.id.clone(),
                    Event {
                        ts_ms,
                        kind: "change".to_string(),
                        session: session.id.clone(),
                        group: group.name.clone(),
                        agent: session.agent.clone().unwrap_or_default(),
                        machine: session.machine.clone().unwrap_or_default(),
                        state: session.state.name().to_string(),
                        cwd: session.cwd.clone().unwrap_or_default(),
                        message: clip(session.message.as_deref().unwrap_or("")),
                    },
                );
            }
        }

        let mut out: Vec<Event> = Vec::new();
        for (id, event) in &current {
            match self.prev.get(id) {
                None => {
                    let mut started = event.clone();
                    started.kind = "start".to_string();
                    out.push(started);
                }
                Some(before) if before.state != event.state => out.push(event.clone()),
                _ => {}
            }
        }
        for (id, before) in &self.prev {
            if !current.contains_key(id) {
                let mut ended = before.clone();
                ended.kind = "end".to_string();
                ended.ts_ms = ts_ms;
                out.push(ended);
            }
        }

        // HashMap 的迭代顺序不能当输出顺序：同一帧里的几条事件要有个确定的次序，
        // 否则单测和「离开回来」的计数会随哈希种子抖动。
        out.sort_by(|a, b| {
            a.kind
                .cmp(&b.kind)
                .then_with(|| a.session.cmp(&b.session))
                .then_with(|| a.state.cmp(&b.state))
        });
        self.prev = current;
        out
    }
}

/// 包在 `coalesce` 外层：**先记事件，再把帧交给原来那条通知链**。
///
/// 放外层而不是内层，是因为合帧只送 100ms 里最后一帧；那样一轮里「出现→完成」
/// 的中间态会被吃掉，恰好是流水最该保住的那一段。
pub fn tap(app: AppHandle, inner: Notifier) -> Notifier {
    let observer = Arc::new(Mutex::new(Observer::default()));
    Arc::new(move |snapshot: &Snapshot| {
        let events = observer
            .lock()
            .expect("events observer poisoned")
            .observe(snapshot, now_ms());
        if !events.is_empty() {
            if let Err(err) = append(&events) {
                eprintln!("[signalhub] 写事件流水失败: {err}");
            }
            // 「盯住这个任务」也只看这一份事件流：HTTP、hook、TTL 到期、手动
            // 清掉会话都会走到这里，不用另挂一条状态监听。
            crate::watches::fire(&app, &events);
        }
        inner(snapshot);
    })
}

/// `~/.signalhub/events`。目录不存在就建（和 `approval_log::dir` 完全同型）。
pub fn dir() -> io::Result<PathBuf> {
    let dir = crate::config::data_dir()?.join("events");
    if !dir.exists() {
        std::fs::create_dir_all(&dir)?;
        let _ = crate::config::set_mode(&dir, 0o700);
    }
    Ok(dir)
}

pub fn append(events: &[Event]) -> io::Result<()> {
    let dir = dir()?;
    append_in(&dir, events)
}

/// 指定目录的版本（单测用）。
pub fn append_in(dir: &Path, events: &[Event]) -> io::Result<()> {
    let Some(first) = events.first() else {
        return Ok(());
    };
    // 系统时钟回拨到 epoch 之前时 `now_ms()` 给 0。写进 `1970-01-01.jsonl`
    // 既没有意义，又会被 `read_since_in` 的 `ts_of >= since_ms` 永远滤掉
    // （写了读不出），还会平白占一个 30 天后才删的文件。
    if first.ts_ms == 0 {
        return Ok(());
    }
    if !dir.exists() {
        std::fs::create_dir_all(dir)?;
        let _ = crate::config::set_mode(dir, 0o700);
    }
    // 一帧里的事件共享同一个 `ts_ms`，必然落在同一天。跨零点的那一帧由下一帧
    // 自己开新文件，不需要在这里分桶。
    let path = dir.join(format!(
        "{}.jsonl",
        crate::approval_log::day_key(first.ts_ms)
    ));
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    let _ = crate::config::set_mode(&path, 0o600);
    for event in events {
        let line = serde_json::to_string(event)
            .map_err(|err| io::Error::other(format!("序列化事件失败: {err}")))?;
        writeln!(file, "{line}")?;
    }
    Ok(())
}

/// 某个时刻之后的事件，新的在前。`limit` 是硬上限，别让 IPC 一次搬一座山。
///
/// **读不到要报错，不能当空**：数据目录建不出来（HOME 不可写）时，界面必须说
/// 「读不到」，而不是「这段时间没有记录」—— 那两句对用户完全是两回事，而且
/// 后者会让一份本该有数据的卡片安静地显示成空白。
pub fn since(since_ms: u64, limit: usize) -> io::Result<Vec<Event>> {
    Ok(since_in(&dir()?, since_ms, limit))
}

pub fn since_in(dir: &Path, since_ms: u64, limit: usize) -> Vec<Event> {
    // 读盘那份逻辑和审批日志共用 `approval_log::read_since_in`（文件名约定、
    // 坏行处理、截止日剪枝都一样）。这里只决定「新的在前」和上限。
    let mut out = crate::approval_log::read_since_in(dir, since_ms, |event: &Event| event.ts_ms);
    out.sort_by(|a, b| {
        b.ts_ms
            .cmp(&a.ts_ms)
            .then_with(|| a.session.cmp(&b.session))
    });
    out.truncate(limit);
    out
}

pub fn prune(now_ms: u64) {
    if let Ok(dir) = dir() {
        crate::approval_log::prune_in(&dir, now_ms);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{GroupView, SessionView, Signal};

    fn session(id: &str, state: Signal, message: &str) -> SessionView {
        SessionView {
            id: id.to_string(),
            state,
            agent: Some("claude-code".to_string()),
            cwd: Some("/work/signalhub".to_string()),
            message: Some(message.to_string()),
            age_ms: 0,
            stale_progress: None,
            machine: None,
        }
    }

    fn snapshot(sessions: Vec<SessionView>) -> Snapshot {
        Snapshot {
            aggregate: Signal::Idle,
            paused: false,
            since_ms: 0,
            message: None,
            last_done_ms: 0,
            groups: vec![GroupView {
                name: "default".to_string(),
                default_label: None,
                aggregate: Signal::Idle,
                message: None,
                since_ms: 0,
                stale_progress: None,
                sessions,
            }],
        }
    }

    fn kinds(events: &[Event]) -> Vec<&str> {
        events.iter().map(|event| event.kind.as_str()).collect()
    }

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("shub-events-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 这条是这个模块存在的理由：跑完就被删掉的会话，快照里什么都不剩，
    /// 但流水里留着一条 `end: done`。
    #[test]
    fn a_session_that_appears_and_disappears_is_still_visible() {
        let mut observer = Observer::default();
        let first = observer.observe(&snapshot(vec![session("a", Signal::Working, "build")]), 1000);
        assert_eq!(kinds(&first), vec!["start"]);
        assert_eq!(first[0].state, "working");

        let second = observer.observe(&snapshot(vec![session("a", Signal::Done, "done")]), 2000);
        assert_eq!(kinds(&second), vec!["change"]);
        assert_eq!(second[0].state, "done");

        let third = observer.observe(&snapshot(vec![]), 3000);
        assert_eq!(kinds(&third), vec!["end"]);
        assert_eq!(third[0].state, "done", "end 要带着最后已知状态");
        assert_eq!(third[0].session, "a");
        assert_eq!(third[0].cwd, "/work/signalhub");
    }

    /// 一轮工具调用只改说明不改状态 —— 不该把流水淹掉。
    #[test]
    fn a_message_only_change_is_not_an_event() {
        let mut observer = Observer::default();
        observer.observe(&snapshot(vec![session("a", Signal::Working, "npm")]), 1000);
        let out = observer.observe(&snapshot(vec![session("a", Signal::Working, "read")]), 2000);
        assert!(out.is_empty(), "只换了 message，不该产生事件: {out:?}");
    }

    /// 同一帧里几条事件的次序不能取决于 HashMap 的迭代顺序。
    #[test]
    fn the_output_order_is_deterministic() {
        let mut observer = Observer::default();
        let out = observer.observe(
            &snapshot(vec![
                session("c", Signal::Working, ""),
                session("a", Signal::Error, ""),
                session("b", Signal::Idle, ""),
            ]),
            1000,
        );
        let ids: Vec<&str> = out.iter().map(|event| event.session.as_str()).collect();
        assert_eq!(ids, vec!["a", "b", "c"]);
    }

    #[test]
    fn the_message_is_clipped_before_it_hits_the_disk() {
        let long = "x".repeat(MESSAGE_KEEP + 50);
        let mut observer = Observer::default();
        let out = observer.observe(&snapshot(vec![session("a", Signal::Working, &long)]), 1000);
        assert_eq!(out[0].message.chars().count(), MESSAGE_KEEP);
    }

    #[test]
    fn events_round_trip_through_the_file_in_reverse_order() {
        let dir = temp("roundtrip");
        let mut observer = Observer::default();
        let first = observer.observe(&snapshot(vec![session("a", Signal::Working, "build")]), 1_704_067_200_000);
        append_in(&dir, &first).unwrap();
        let second = observer.observe(&snapshot(vec![]), 1_704_067_201_000);
        append_in(&dir, &second).unwrap();

        let out = since_in(&dir, 0, 10);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].kind, "end");
        assert_eq!(out[1].kind, "start");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 时钟异常时别在盘上留下一个永远读不出来的 `1970-01-01.jsonl`。
    #[test]
    fn a_zero_timestamp_never_hits_the_disk() {
        let dir = temp("zero");
        let mut observer = Observer::default();
        let events = observer.observe(&snapshot(vec![session("a", Signal::Working, "")]), 0);
        append_in(&dir, &events).unwrap();
        assert!(
            std::fs::read_dir(&dir).unwrap().next().is_none(),
            "ts=0 不该在盘上留下任何文件"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_line_does_not_poison_the_rest() {
        let dir = temp("corrupt");
        let good = serde_json::to_string(&Event {
            ts_ms: 1_704_067_200_000,
            kind: "start".to_string(),
            session: "a".to_string(),
            group: "default".to_string(),
            agent: String::new(),
            machine: String::new(),
            state: "working".to_string(),
            cwd: String::new(),
            message: String::new(),
        })
        .unwrap();
        std::fs::write(
            dir.join("2024-01-01.jsonl"),
            format!("{good}\n{{ 这不是 json\n"),
        )
        .unwrap();
        assert_eq!(since_in(&dir, 0, 10).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 截止日之前的文件连打开都不该打开；`limit` 是硬上限。
    #[test]
    fn since_respects_the_cutoff_and_the_limit() {
        let dir = temp("cutoff");
        let mut observer = Observer::default();
        let events = observer.observe(
            &snapshot(vec![
                session("a", Signal::Working, ""),
                session("b", Signal::Error, ""),
            ]),
            1_704_067_200_000,
        );
        append_in(&dir, &events).unwrap();
        assert_eq!(since_in(&dir, 1_704_067_201_000, 10).len(), 0);
        assert_eq!(since_in(&dir, 0, 1).len(), 1);
        assert_eq!(since_in(&dir, 0, 0).len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
