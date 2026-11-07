//! 审批历史（`docs/plan-approval.md` A1）。
//!
//! 为什么值得单独存一份：审批是**会改变智能体行为**的动作，出事时第一个问题
//! 永远是「谁在什么时候放行了这条命令」。当前状态（`approvals.rs` 的登记表）
//! 只记「正挂着的」，点完就没了 —— 那份东西回答不了这个问题。
//!
//! 形状刻意做得和 `stats.rs` 一样：**每日一个 jsonl**，不是一个大 JSON。
//! 审批记录是追加型的（只写不改），jsonl 正好；而且坏一行只丢一条，不会让整份
//! 历史都读不出来。
//!
//! **存的是截到 200 字符的详情，不是全文。** 审计要回答「大概是什么」，
//! 而把 `API_KEY=…` 那种东西永久写盘是另一回事 —— 全文只该活在本地界面里。

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

/// 保留多少天。和统计（30 天）对齐：用户对「历史有多长」只需要一个预期。
pub const KEEP_DAYS: i64 = 30;

/// 详情写盘时截到多少字符。和界面里的 240 分开：**盘上留得更短**，
/// 因为界面是「此刻要看全」，盘上是「以后要知道大概」。
pub const DETAIL_KEEP: usize = 200;

/// 一条审批记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// Unix 毫秒。界面按本地时区显示，存的是绝对时间。
    pub ts_ms: u64,
    pub session: String,
    #[serde(default)]
    pub agent: String,
    pub tool: String,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub truncated: bool,
    /// `allow` / `deny` / `expired`。
    pub decision: String,
    /// 拒绝理由的键（`unsafe` / `safer` / `explain` / `custom`）；允许与超时为空。
    #[serde(default)]
    pub reason: String,
    /// `lamp` / `api` / `web`。
    pub source: String,
    /// 从登记到出结果等了多久。
    pub waited_ms: u64,
}

impl Record {
    /// 详情截到 [`DETAIL_KEEP`]，并保证 `truncated` 如实反映「盘上这份是不是全的」。
    ///
    /// 参数多是因为它**就是那条记录本身**（11 个字段一一对应）。这里不引 builder：
    /// 只有一个调用点（`Registry::record`），而 builder 只会让「哪个字段忘了填」
    /// 从编译错误变成一个默认值。
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ts_ms: u64,
        session: String,
        agent: String,
        tool: String,
        target: String,
        detail: String,
        truncated: bool,
        decision: &str,
        reason: String,
        source: &str,
        waited_ms: u64,
    ) -> Self {
        let clipped: String = detail.chars().take(DETAIL_KEEP).collect();
        let truncated = truncated || clipped != detail;
        Self {
            ts_ms,
            session,
            agent,
            tool,
            target,
            detail: clipped,
            truncated,
            decision: decision.to_string(),
            reason,
            source: source.to_string(),
            waited_ms,
        }
    }
}

/// `~/.signalhub/approvals`。目录不存在就建（权限跟 `data_dir` 一样是 0o700）。
pub fn dir() -> io::Result<PathBuf> {
    let dir = crate::config::data_dir()?.join("approvals");
    if !dir.exists() {
        std::fs::create_dir_all(&dir)?;
        let _ = crate::config::set_mode(&dir, 0o700);
    }
    Ok(dir)
}

/// 指定目录的版本 —— 单测用它写临时目录，不碰用户真实的 `~/.signalhub`。
pub fn append_in(dir: &Path, record: &Record) -> io::Result<()> {
    if !dir.exists() {
        std::fs::create_dir_all(dir)?;
        let _ = crate::config::set_mode(dir, 0o700);
    }
    let path = dir.join(format!("{}.jsonl", day_key(record.ts_ms)));
    let mut line = serde_json::to_string(record)
        .map_err(|err| io::Error::other(format!("序列化审批记录失败: {err}")))?;
    line.push('\n');

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    // 这条落盘的是**命令原文**（`API_KEY=… curl …` 里就是要看的那一整行）。
    // 目录已经是 0o700，但只靠目录挡不住「配置目录被别的东西改宽」这种情况 ——
    // 文件自己也收窄，和 token 同一档（`config::write_private`）。
    // 每条都设一次：一次 chmod 的代价可以忽略，而它顺手修好权限被改过的老文件。
    let _ = crate::config::set_mode(&path, 0o600);
    file.write_all(line.as_bytes())
}

/// 最近 `limit` 条，新的在前。
///
/// 只读最近两个文件：一条记录只会落在「今天」或「昨天」（跨零点的边界），
/// 再多读就是给一个翻不到底的列表做无用的 I/O。
pub fn recent(limit: usize) -> Vec<Record> {
    match dir() {
        Ok(dir) => recent_in(&dir, limit),
        Err(_) => Vec::new(),
    }
}

/// 指定目录的版本（单测用）。
pub fn recent_in(dir: &Path, limit: usize) -> Vec<Record> {
    let mut files = jsonl_files(dir);
    // 文件名就是日期，倒序就是「新的在前」
    files.sort_by(|a, b| b.cmp(a));
    files.truncate(2);

    let mut out: Vec<Record> = Vec::new();
    for name in files {
        let Ok(file) = std::fs::File::open(dir.join(&name)) else {
            continue;
        };
        for line in io::BufReader::new(file).lines() {
            let Ok(line) = line else { continue };
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            // 坏一行只丢一条：这是审计日志，不该因为一次写坏就整份读不出来
            if let Ok(record) = serde_json::from_str::<Record>(line) {
                out.push(record);
            }
        }
    }
    out.sort_by_key(|record| std::cmp::Reverse(record.ts_ms));
    out.truncate(limit);
    out
}

/// 某个时刻之后的审批记录，新的在前。
///
/// 和 [`recent`] 不同：那个只读最近两个文件（界面上的「审批记录」列表只需要
/// 翻到昨天），而「本周洞察」要跨 7 天按工具聚合，得读整个保留期。
/// 同上，但**读不到要报错**：`get_approval_history` 拿它做「本周洞察」，
/// 目录不可读时界面得说「读不到」，不能说「这段时间没有记录」。
/// 供列表用的 [`recent`] 保持原来的「读不到就当空」语义不变。
pub fn since(since_ms: u64, limit: usize) -> io::Result<Vec<Record>> {
    Ok(since_in(&dir()?, since_ms, limit))
}

pub fn since_in(dir: &Path, since_ms: u64, limit: usize) -> Vec<Record> {
    let mut out = read_since_in(dir, since_ms, |record: &Record| record.ts_ms);
    out.sort_by_key(|record| std::cmp::Reverse(record.ts_ms));
    out.truncate(limit);
    out
}

/// 「每日一个 jsonl、按时间下界读」的实现只写这一份：事件流水（`events.rs`）
/// 和审批日志的文件名约定、坏行处理、截止日剪枝完全一样，差别只有记录类型和
/// 排序规则。各写一份的话，跨零点那天两份日志会对不上。
pub(crate) fn read_since_in<T, F>(dir: &Path, since_ms: u64, ts_of: F) -> Vec<T>
where
    T: DeserializeOwned,
    F: Fn(&T) -> u64,
{
    let cutoff = day_key(since_ms);
    let mut files = jsonl_files(dir);
    files.sort_by(|a, b| b.cmp(a));

    let mut out: Vec<T> = Vec::new();
    for name in files {
        // 文件名是 `YYYY-MM-DD.jsonl`，倒序扫到比截止日更早的就可以停。
        // `cutoff` 少了后缀，所以当天那个文件字典序更大，不会被误跳过。
        if name.as_str() < cutoff.as_str() {
            break;
        }
        let Ok(file) = std::fs::File::open(dir.join(&name)) else {
            continue;
        };
        for line in io::BufReader::new(file).lines() {
            let Ok(line) = line else { continue };
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            // 坏一行只丢一条：这是日志，不该因为一次写坏就整份读不出来。
            if let Ok(item) = serde_json::from_str::<T>(line)
                && ts_of(&item) >= since_ms
            {
                out.push(item);
            }
        }
    }
    out
}

/// 删掉超过 [`KEEP_DAYS`] 天的文件。启动时调一次就够 —— 它不是实时要求。
pub fn prune(now_ms: u64) {
    if let Ok(dir) = dir() {
        prune_in(&dir, now_ms);
    }
}

pub fn prune_in(dir: &Path, now_ms: u64) {
    let cutoff = day_key(now_ms.saturating_sub(KEEP_DAYS as u64 * 86_400_000));
    for name in jsonl_files(dir) {
        // 文件名是 `YYYY-MM-DD.jsonl`，字典序就是时间序
        if name.as_str() < cutoff.as_str() {
            let _ = std::fs::remove_file(dir.join(&name));
        }
    }
}

fn jsonl_files(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.ends_with(".jsonl"))
        .collect()
}

/// Unix 毫秒 → `YYYY-MM-DD`（UTC）。
///
/// 用 UTC 而不是本地时区：这里不引时区数据依赖（和 `config::utc_hms` 同一个取舍），
/// 而**跨零点那条记录落在哪个文件并不重要** —— 界面按 `ts_ms` 排序显示，不按文件。
pub(crate) fn day_key(ts_ms: u64) -> String {
    let (year, month, day) = civil_from_days((ts_ms / 86_400_000) as i64);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Howard Hinnant 的 `civil_from_days`：把「1970-01-01 起的天数」换成公历年月日。
///
/// 自己写而不是引 `chrono`：这里只需要一个方向、一个历法，而 CLI 的依赖树里
/// 少一棵是一棵（见 `config::home_dir` 的同一条理由）。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(ts_ms: u64, decision: &str) -> Record {
        Record::new(
            ts_ms,
            "s1".to_string(),
            "claude-code".to_string(),
            "Bash".to_string(),
            "rm".to_string(),
            "rm -rf build".to_string(),
            false,
            decision,
            String::new(),
            "lamp",
            1200,
        )
    }

    /// 日历换算只在一个地方用，写错的表现是「记录跑到隔壁日期」——
    /// 三个已知点钉住它就够了。
    #[test]
    fn the_day_key_matches_known_dates() {
        assert_eq!(day_key(0), "1970-01-01");
        assert_eq!(day_key(951_868_800_000), "2000-03-01");
        assert_eq!(day_key(1_704_067_200_000), "2024-01-01");
        // 闰日：2024-02-29T00:00:00Z
        assert_eq!(day_key(1_709_164_800_000), "2024-02-29");
    }

    /// 写进去、读出来，字段一个不少。
    #[test]
    fn a_record_round_trips_through_the_file() {
        let dir = std::env::temp_dir().join(format!("shub-approvals-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        append_in(&dir, &record(1_704_067_200_000, "allow")).unwrap();
        append_in(&dir, &record(1_704_067_201_000, "deny")).unwrap();

        let out = recent_in(&dir, 10);
        assert_eq!(out.len(), 2);
        // 新的在前
        assert_eq!(out[0].decision, "deny");
        assert_eq!(out[1].decision, "allow");
        assert_eq!(out[0].agent, "claude-code");
        assert_eq!(out[0].waited_ms, 1200);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 坏一行只丢一条，其余照读 —— 审计日志不该被一次写坏整份废掉。
    #[test]
    fn a_corrupt_line_does_not_poison_the_rest() {
        let dir = std::env::temp_dir().join(format!("shub-approvals-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let path = dir.join("2024-01-01.jsonl");
        let good = serde_json::to_string(&record(1_704_067_200_000, "allow")).unwrap();
        std::fs::write(&path, format!("{good}\n{{ 这不是 json\n{good}\n")).unwrap();

        assert_eq!(recent_in(&dir, 10).len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 详情落盘要截短：盘上留「大概是什么」，全文只活在该死的本地界面里。
    #[test]
    fn the_detail_is_clipped_before_it_hits_the_disk() {
        let long = "x".repeat(DETAIL_KEEP + 50);
        let record = Record::new(
            0,
            "s1".to_string(),
            "claude-code".to_string(),
            "Bash".to_string(),
            "run".to_string(),
            long,
            false,
            "allow",
            String::new(),
            "lamp",
            0,
        );
        assert_eq!(record.detail.chars().count(), DETAIL_KEEP);
        assert!(record.truncated, "截过就要如实标出来");
    }

    /// 超过保留期的文件被删，没超过的不动。
    #[test]
    fn pruning_drops_only_old_files() {
        let dir = std::env::temp_dir().join(format!("shub-approvals-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        std::fs::write(dir.join("2024-01-01.jsonl"), "").unwrap();
        std::fs::write(dir.join("2024-06-01.jsonl"), "").unwrap();
        // 现在假设是 2024-06-15：2024-01-01 超过 30 天，2024-06-01 没有
        prune_in(&dir, 1_718_409_600_000);

        assert!(!dir.join("2024-01-01.jsonl").exists(), "该删的没删");
        assert!(dir.join("2024-06-01.jsonl").exists(), "不该删的被删了");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 「本周洞察」要跨 7 天按工具聚合，`recent` 只读两个文件是不够的。
    #[test]
    fn since_reads_across_more_than_two_days() {
        let dir = std::env::temp_dir().join(format!("shub-approvals-since-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        append_in(&dir, &record(1_704_067_200_000, "allow")).unwrap();
        append_in(&dir, &record(1_704_153_600_000, "deny")).unwrap();
        append_in(&dir, &record(1_704_240_000_000, "expired")).unwrap();

        assert_eq!(since_in(&dir, 0, 10).len(), 3, "三天都要读到");
        let since = since_in(&dir, 1_704_153_600_000, 10);
        assert_eq!(since.len(), 2);
        assert_eq!(since[0].decision, "expired", "新的在前");
        assert_eq!(since_in(&dir, 0, 1).len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 落盘的是命令原文 —— 同机器上的其他用户不该读得到。
    ///
    /// 钉住的是「文件自己收窄」这件事，不是目录：目录的 0o700 由 `data_dir`
    /// 保证，但那是**创建时**设的，老安装或者被人改宽过的那种挡不住。
    #[cfg(unix)]
    #[test]
    fn the_log_file_is_not_readable_by_other_users() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("shub-approvals-mode-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let record = Record::new(
            1_700_000_000_000,
            "s1".to_string(),
            "claude-code".to_string(),
            "Bash".to_string(),
            "curl".to_string(),
            "API_KEY=secret curl https://example.com".to_string(),
            false,
            "deny",
            "unsafe".to_string(),
            "lamp",
            120,
        );
        append_in(&dir, &record).unwrap();

        let path = dir.join("2023-11-14.jsonl");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o077,
            0,
            "审批历史对同组/其他用户可读（mode = {:o}）",
            mode & 0o777
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
