//! 「接入到底有没有在报」的凭证。
//!
//! **为什么需要这一层**：接入页以前只能告诉你「文件写进去了」——而**成功和失败看起来
//! 一模一样**。点完接入、用一次智能体、灯没亮，用户不知道是该怀疑智能体那一头，还是
//! 我们这一头。这里记下每个信号源**最后一次上报的时刻**（以及报的是什么状态），接入页
//! 把那句话摆出来，问题立刻变成可判断的：有「12 秒前」就是通的，没有就是没通。
//!
//! **记在上报路径上**（`server::ingest_core`），所以 HTTP 上报、CLI 钩子、spool 重放、
//! 手写 curl 全都算数。另一条路是解析 `hooks.log`，不采用：那是**另一个进程**写的文本，
//! 格式可能随版本漂移（CLI 和 app 不一定是同一个版本），拿它当真值来源迟早会读到一具
//! 尸体还说得很确定。
//!
//! **落盘**（`~/.signalhub/receipts.json`）的理由和 `stats.json` 一样：常驻不等于
//! 永不重启 —— 改设置、升级、崩了重来，用户对「它上次报是什么时候」的期待是跨重启的。

use crate::config;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// 两次落盘之间至少隔这么久。
///
/// 上报可以很密（每次工具调用一条），而这张表要回答的是「最近有没有在报」——秒级的
/// 新鲜度完全够用。不节流的话，一个跑得欢的智能体会让这个小文件每秒被写好几次。
const SAVE_EVERY: Duration = Duration::from_secs(5);

/// 最多记多少个信号源。正常用量的量级是「个位数 + 几个自己的脚本」；触到上限
/// 只可能是上报方每次换一个名字。
const MAX_RECEIPTS: usize = 64;

/// 落盘的样子。键是信号源名字（CLI 那个 `claude-code` / `opencode`）。
#[derive(Serialize, Deserialize, Default)]
struct Stored {
    agents: BTreeMap<String, StoredEntry>,
}

/// 落盘的那部分：**只存跨重启还有意义的字段**。
#[derive(Serialize, Deserialize, Default)]
struct StoredEntry {
    /// 最后一次上报的**墙上**时刻（Unix 毫秒）。墙上时钟存得下来，单调钟存不下来。
    last_epoch_ms: u64,
    /// 最后一次上报的状态名。「它报过」和「它报的是什么」是两个问题。
    state: String,
    /// 最后一次上报**声明**的信号源类型（`agent` / `system` / `custom`）。
    ///
    /// `serde(default)` 是给老文件的：这个字段是后来才加的，缺了就是空串 ——
    /// 空串在界面上落到「自定义」，正是「没声明」该有的待遇。
    #[serde(default)]
    source: String,
}

#[derive(Default, Clone)]
struct Entry {
    last_epoch_ms: u64,
    state: String,
    /// 最后一次声明的那一类。**后一次覆盖前一次**：同一个名字先后声明成不同类别时，
    /// 以最近那次为准（和 `state` 一样，这张表回答的是「它现在是什么」）。
    source: String,
    /// **本次运行以来**的次数。刻意不落盘：它回答的是「它现在还在报吗」，而这个
    /// 问题的时效就是这一次运行。跨重启的总量在 `stats.json` 那边（按时长算的）。
    count: u64,
}

/// 给前端的视图。相对时间由前端算（它有一个会走的钟，Rust 这边没有）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReceiptView {
    pub agent: String,
    /// `0` = 从来没收到过这个信号源的上报。
    pub last_epoch_ms: u64,
    /// 最后一次上报的状态名，没有就是空串。
    pub state: String,
    /// 最后一次上报声明的信号源类型（`agent` / `system` / `custom`）。
    ///
    /// **空串 = 没声明**（老文件，或者上报方压根没带这个字段）。界面据此把信号源
    /// 分成「AI 智能体 / 系统 / 自定义」三组，见 `SettingsPanel` 的 `groupReceipts`。
    pub source: String,
    /// 本次运行以来的上报条数。
    pub count: u64,
}

/// 每个信号源的「最近一次上报」。挂在应用状态里，上报路径和界面共用同一份。
pub struct Receipts {
    /// `None` = 不落盘（单测走这条：**绝不能写开发者真实的 `~/.signalhub`**）。
    path: Option<PathBuf>,
    inner: Mutex<Inner>,
}

struct Inner {
    agents: BTreeMap<String, Entry>,
    /// 上次落盘的时刻，用来节流
    saved: Instant,
}

impl Receipts {
    /// 从盘上读回来。读不出来（第一次跑、文件被改坏、没权限）就是空的 —— 这只是
    /// 一份排查用的旁路信息，**不值得为它报错**。
    pub fn load() -> Self {
        Self::with_path(config::receipts_file().ok())
    }

    /// 不落盘的一个（单测用）：**绝不能写开发者真实的 `~/.signalhub`**。
    #[cfg(test)]
    pub(crate) fn in_memory() -> Self {
        Self::with_path(None)
    }

    fn with_path(path: Option<PathBuf>) -> Self {
        let agents = path
            .as_deref()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str::<Stored>(&text).ok())
            .map(|stored| {
                stored
                    .agents
                    .into_iter()
                    .map(|(agent, entry)| {
                        (
                            agent,
                            Entry {
                                last_epoch_ms: entry.last_epoch_ms,
                                state: entry.state,
                                source: entry.source,
                                // 次数不跨重启：它是「这次运行里还在报吗」
                                count: 0,
                            },
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();

        // `saved` 从此刻起算：刚读完盘没必要马上再写一遍
        Self {
            path,
            inner: Mutex::new(Inner {
                agents,
                saved: Instant::now(),
            }),
        }
    }

    /// 记一次上报。**这是上报路径上的函数，所以它绝不能失败、也绝不能慢。**
    ///
    /// `source` 是上报方声明的那一类（见 `state::Report::source_id`），只用于界面分组。
    pub fn record(&self, agent: &str, source: &str, state: &str) {
        let agent = agent.trim();
        // 空名字丢掉（上报里 `agent` 是可选字段，没写就是没有信号源可记）。
        // 除此之外**不认识的名字也照收**：这一层不认识智能体清单，也不该认识 ——
        // 用户自己的脚本同样值得有一份凭证。
        if agent.is_empty() {
            return;
        }

        let mut inner = self.inner.lock().expect("receipts poisoned");
        let entry = inner.agents.entry(agent.to_string()).or_default();
        entry.last_epoch_ms = epoch_ms();
        entry.state = state.to_string();
        entry.source = source.to_string();
        entry.count = entry.count.saturating_add(1);

        // 名字是上报方给的：一个每次运行换名字的脚本能把它撑成无界的一张表，
        // 而这张表每 5 秒会被整份重写一遍（issue #16）。满了丢最久没报的那个。
        while inner.agents.len() > MAX_RECEIPTS {
            let Some(oldest) = inner
                .agents
                .iter()
                .min_by_key(|(_, entry)| entry.last_epoch_ms)
                .map(|(name, _)| name.clone())
            else {
                break;
            };
            inner.agents.remove(&oldest);
        }

        if inner.saved.elapsed() < SAVE_EVERY {
            return;
        }
        inner.saved = Instant::now();
        save(&inner, self.path.as_deref());
    }

    /// 全部凭证。键有序（`BTreeMap`），所以界面上的顺序是稳定的。
    pub fn view(&self) -> Vec<ReceiptView> {
        let inner = self.inner.lock().expect("receipts poisoned");
        inner
            .agents
            .iter()
            .map(|(agent, entry)| ReceiptView {
                agent: agent.clone(),
                last_epoch_ms: entry.last_epoch_ms,
                state: entry.state.clone(),
                source: entry.source.clone(),
                count: entry.count,
            })
            .collect()
    }
}

/// 尽力落盘。**失败就算了**：这是排查用的旁路信息，绝不能因为它出错而影响上报。
fn save(inner: &Inner, path: Option<&std::path::Path>) {
    let Some(path) = path else {
        return;
    };
    let stored = Stored {
        agents: inner
            .agents
            .iter()
            .map(|(agent, entry)| {
                (
                    agent.clone(),
                    StoredEntry {
                        last_epoch_ms: entry.last_epoch_ms,
                        state: entry.state.clone(),
                        source: entry.source.clone(),
                    },
                )
            })
            .collect(),
    };
    let Ok(text) = serde_json::to_string(&stored) else {
        return;
    };
    let _ = config::atomic_write(path, &text, None);
}

fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 信号源名字由上报方给：一个每次运行换一个名字的脚本不该让这张表无界增长，
    /// 也不该让它每 5 秒重写一份越来越大的文件（issue #16）。
    #[test]
    fn the_agent_table_is_capped() {
        let receipts = Receipts::in_memory();
        for index in 0..(MAX_RECEIPTS + 10) {
            receipts.record(&format!("agent-{index}"), "custom", "working");
        }
        assert_eq!(receipts.view().len(), MAX_RECEIPTS);
    }

    /// 临时目录。单测**绝不能**写真实的 `~/.signalhub`（那里是开发者自己的数据）。
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let mut path = std::env::temp_dir();
            path.push(format!(
                "signalhub-receipts-test-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn file(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn in_memory() -> Receipts {
        Receipts::with_path(None)
    }

    #[test]
    fn nothing_is_recorded_before_any_report() {
        assert!(in_memory().view().is_empty(), "没报过就该是空的，而不是一堆 0");
    }

    #[test]
    fn a_report_sets_the_time_state_and_count() {
        let receipts = in_memory();
        receipts.record("opencode", "agent", "working");
        receipts.record("opencode", "agent", "done");

        let view = receipts.view();
        assert_eq!(view.len(), 1);
        assert_eq!(view[0].agent, "opencode");
        assert_eq!(view[0].state, "done", "记的该是最后一次报的状态");
        assert_eq!(view[0].count, 2, "两次上报算两次");
        assert_eq!(view[0].source, "agent", "声明的那一类也要记下来");
        assert!(view[0].last_epoch_ms > 0, "时刻要填上");
    }

    /// 同一个名字先后声明成不同类别时**以最近那次为准**：这张表回答的是「它现在是
    /// 什么」，不是「它曾经是什么」。
    #[test]
    fn the_source_is_the_last_one_declared() {
        let receipts = in_memory();
        receipts.record("watch-build", "custom", "working");
        receipts.record("watch-build", "system", "working");

        assert_eq!(receipts.view()[0].source, "system");
    }

    /// **老文件没有 `source` 这个字段**（它是后加的），读回来该是空串 —— 空串在
    /// 界面上落到「自定义」，正是「没声明」该有的待遇，而不是让整个文件读不出来。
    #[test]
    fn an_old_file_without_a_source_still_loads() {
        let dir = TempDir::new();
        let path = dir.file("receipts.json");
        std::fs::write(
            &path,
            r#"{"agents":{"my-script":{"last_epoch_ms":1700000000000,"state":"working"}}}"#,
        )
        .unwrap();

        let view = Receipts::with_path(Some(path)).view();
        assert_eq!(view.len(), 1);
        assert_eq!(view[0].agent, "my-script");
        assert_eq!(view[0].source, "");
    }

    /// 没有名字的上报不记 —— `agent` 是可选字段，不写就是「没有信号源」。
    /// 空白字符也算没有：写进去会得到一个看不见名字的行。
    #[test]
    fn nameless_reports_are_ignored() {
        let receipts = in_memory();
        receipts.record("", "custom", "working");
        receipts.record("   ", "custom", "working");
        assert!(receipts.view().is_empty());
    }

    /// 跨重启**只**保留「最近一次是什么时候、报的什么」；次数从 0 重新数。
    #[test]
    fn the_last_report_survives_a_restart_but_the_count_does_not() {
        let dir = TempDir::new();
        let path = dir.file("receipts.json");

        {
            let receipts = Receipts::with_path(Some(path.clone()));
            receipts.record("claude-code", "agent", "permission");
            // 落盘是节流的，第一次记就会写（`saved` 从构造时起算，已过 5 秒是不可能的，
            // 所以这里显式催一次）
            save(
                &receipts.inner.lock().unwrap(),
                Some(path.as_path()),
            );
        }

        let reloaded = Receipts::with_path(Some(path));
        let view = reloaded.view();
        assert_eq!(view.len(), 1, "重启后该还记得有个信号源报过");
        assert_eq!(view[0].agent, "claude-code");
        assert_eq!(view[0].state, "permission");
        assert_eq!(view[0].source, "agent", "类别也要跨重启");
        assert!(view[0].last_epoch_ms > 0);
        assert_eq!(view[0].count, 0, "次数只算本次运行");
    }

    /// 文件被改坏 / 只有半截时**当作没记过**，而不是让应用起不来。
    #[test]
    fn a_corrupt_file_is_treated_as_empty() {
        let dir = TempDir::new();
        let path = dir.file("receipts.json");
        std::fs::write(&path, "{ 这不是 json").unwrap();

        assert!(Receipts::with_path(Some(path)).view().is_empty());
    }

    /// 落盘是节流的：紧挨着的第二次上报不写盘（否则一个跑得欢的智能体会让这个小
    /// 文件每秒被写好几次）。这里断言的是「节流真的在起作用」，不是具体时间。
    #[test]
    fn saving_is_throttled() {
        let dir = TempDir::new();
        let path = dir.file("receipts.json");
        let receipts = Receipts::with_path(Some(path.clone()));

        // 构造时 `saved` 就是此刻，所以第一条也会被节流掉 —— 这正是要的行为：
        // 刚启动时不需要立刻写盘。
        receipts.record("one", "custom", "working");
        assert!(
            !path.exists(),
            "刚记下的第一条不该立刻落盘（节流从构造时刻起算）"
        );
    }
}
