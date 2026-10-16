//! 事件降级通道：app 没在跑的时候，上报先攒在盘上，等它起来再补。
//!
//! 为什么需要它：hook 上报是「一次 HTTP，失败就记一行日志」，而 app 没在运行时
//! （重启电脑、退出 app、刚装还没启动）**所有事件静默丢失** —— hook 在后台跑，
//! 用户根本不会发现灯少了什么。`docs/api.md` §5 此前一直写着这条「还没做」。
//!
//! 这个文件是**共享**的：写它的是 CLI（`bin/signalhub.rs`，hook 连不上时），
//! 读它的是 app（`server.rs`，启动时重放一遍）。所以它只依赖 `config` 这一个
//! CLI 也会 `#[path]` 带上的模块。
//!
//! 三条硬性约束：
//!
//! 1. **写入失败绝不能影响 hook。** 磁盘满、权限不对、文件被删 —— 一律吞掉。
//!    hook 是用来监控别人的，自己不能变成故障点。
//! 2. **有界。** hook 是每次工具调用都跑一遍的，没有上限的话，一次「app 忘了开」
//!    的周末就能堆出几百 MB，第二天一起来就要解析它。满了**从最老的丢**。
//! 3. **一行一条自足的 JSON**（JSONL）。写入者是多个并发的 hook 进程，追加一条
//!    独立的行不需要锁；坏行跳过即可。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config;

/// 攒下的最大字节数。超过就从最老的开始丢。
pub const MAX_BYTES: usize = 256 * 1024;

/// 攒下的最大条数。同上。
///
/// 200 条的分量：正常使用下这个文件**平时是空的**（app 在跑，什么都不攒），
/// 它只在「app 没开」时积累。200 条足够覆盖一次退出期间的完整活动，又小到
/// 启动时重放一遍毫无感觉。
pub const MAX_LINES: usize = 200;

/// 小于这个字节数就不必整理。
///
/// 一条记录**最短**也有 40 多字节（`{"at_ms":0,"op":{"kind":"clear","session":""}}`），
/// 所以 4 KiB 里最多 90 来行、也远不到 256 KiB —— 两个上限都不可能越界。
/// 这是热路径上唯一的一次 `metadata()`（只读元数据，不读内容），换来的是
/// 「app 正常在跑、文件是空的」这个最常见的情况下一点都不读盘。
const TRIM_BELOW: u64 = 4 * 1024;

/// 攒下来的一条。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Spooled {
    /// 事件**真的发生**的时刻（Unix 毫秒）。重放要靠它算「还剩多久」。
    ///
    /// 不存 `Instant`（不可序列化、也不跨进程），也不存给人看的文本
    /// （重放还要解析回来）。
    pub at_ms: u64,

    pub op: SpoolOp,
}

/// 一条事件的**意图**。
///
/// 存意图而不是 HTTP 的 `method` + `path`：重放要按它决定走哪条 ingest 路径，
/// 而重新解析一遍我们自己拼出来的 URL（还得从 `/sessions/{id}` 里把 id 抠回来）
/// 只是把「我知道这是什么」绕一圈再说一遍。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SpoolOp {
    /// 等价于 `POST /v1/state`；`body` 是那次请求体**原样**存下的一份。
    ///
    /// 存请求体而不是解析成 `Report`：CLI 按设计**不校验状态名**（权威定义在
    /// 服务端，见 `bin/signalhub.rs` 顶部约束 3），它连 `Report` 的形状都不该知道。
    Report { body: Value },

    /// 等价于 `DELETE /v1/sessions/{session}`。
    Clear { session: String },
}

impl Spooled {
    /// 记下刚刚发生的一件事。
    pub fn now(op: SpoolOp) -> Self {
        Self { at_ms: now_ms(), op }
    }
}

/// 当前 Unix 毫秒。
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// spool 文件：`~/.signalhub/spool.jsonl`。
pub fn path() -> Option<PathBuf> {
    config::data_dir().ok().map(|dir| dir.join("spool.jsonl"))
}

/// 追加一条。**任何失败都吞掉**（约束 1）。
///
/// 返回值刻意是 `()`：调用方（hook）没有任何合理的反应方式，能做的只有在日志里
/// 说一句「这条没保住」，而那由调用方自己决定。
pub fn append(record: &Spooled) {
    let Some(path) = path() else { return };
    let Ok(line) = serde_json::to_string(record) else { return };
    // 单条就超上限：存它进去等于把别人全挤掉，不如直接放弃这一条
    if line.len() > MAX_BYTES {
        return;
    }
    if write_line(&path, &line).is_err() {
        return;
    }
    let _ = trim(&path);
}

fn write_line(path: &Path, line: &str) -> std::io::Result<()> {
    // 「一行 + 换行」**一次写完**：并发的 hook 进程各自 append 时，只有单次写
    // 才是原子的（两次写会让两条记录交错成一堆坏行）。
    let mut buf = String::with_capacity(line.len() + 1);
    buf.push_str(line);
    buf.push('\n');
    let mut file = fs::OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(buf.as_bytes())
}

/// 超过任一上限就从**最老的**开始丢。
///
/// 「最老的先丢」不是随手定的：留下来的必须是最新的那一段活动，因为重放时
/// 「三小时前在干嘛」本来就会被 TTL 判过期丢掉 —— 保它等于白占位置。
fn trim(path: &Path) -> std::io::Result<()> {
    if fs::metadata(path)?.len() <= TRIM_BELOW {
        return Ok(());
    }
    let raw = fs::read_to_string(path)?;
    let lines: Vec<&str> = raw.lines().collect();
    if lines.len() <= MAX_LINES && raw.len() <= MAX_BYTES {
        return Ok(());
    }

    // 从尾部往前留，两个上限一起满足
    let mut keep: Vec<&str> = Vec::new();
    let mut bytes = 0usize;
    for line in lines.iter().rev() {
        let cost = line.len() + 1;
        if keep.len() == MAX_LINES || bytes + cost > MAX_BYTES {
            break;
        }
        bytes += cost;
        keep.push(line);
    }
    keep.reverse();

    let mut out = keep.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    // 截断重写。此刻可能有另一个 hook 也在写这个文件 —— 那种交错最多丢一两条
    // 记录，而「丢掉最老的那些」本来就是这一步在做的事（红线是不许无界，
    // 不是不许丢；真一条都不能丢的话，这个文件就不该有上限）。
    fs::write(path, out)
}

/// 读回全部记录，顺便报告坏行的条数。
///
/// **坏行跳过**：文件可能被手改过、被磁盘满截断、或两个进程交错写花（见上）。
/// 一条坏行不该让后面所有记录都读不出来。
pub fn read_all(path: &Path) -> (Vec<Spooled>, usize) {
    let Ok(raw) = fs::read_to_string(path) else {
        return (Vec::new(), 0);
    };
    let mut records = Vec::new();
    let mut bad = 0;
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<Spooled>(line) {
            Ok(record) => records.push(record),
            Err(_) => bad += 1,
        }
    }
    (records, bad)
}

/// 重放完把文件删掉。
///
/// 删失败不算错（下次启动会再重放一遍）。**代价要说清**：状态上报不是严格幂等的
/// —— 同一会话的旧状态重放一遍，可能把已经前进的状态倒回去。所以这里只在
/// `remove_file` 真的失败时才会发生，而那种情况（权限、只读磁盘）本来就少见。
pub fn remove(path: &Path) {
    let _ = fs::remove_file(path);
}

// ---- 单测用的入口 ----------------------------------------------------------
//
// 放在模块级（而不是 `mod tests` 里）是为了让 `server.rs` 的重放用例也能用：
// 那边要往一个**临时文件**里写记录，绝不能碰用户真实的 `~/.signalhub/spool.jsonl`。

/// 每个用例一个独立目录的临时文件名。
#[cfg(test)]
pub(crate) fn temp_path(name: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);

    let dir = std::env::temp_dir().join(format!(
        "signalhub-spool-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// 写进指定路径的版本（`append` 写的是真实路径）。测试和单测助手用。
#[cfg(test)]
pub(crate) fn append_at(path: &Path, record: &Spooled) {
    let line = serde_json::to_string(record).unwrap();
    write_line(path, &line).unwrap();
    trim(path).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn report(state: &str) -> Spooled {
        Spooled {
            at_ms: 1_000,
            op: SpoolOp::Report { body: json!({ "state": state }) },
        }
    }

    #[test]
    fn appends_one_json_object_per_line() {
        let path = temp_path("spool.jsonl");
        append_at(&path, &report("working"));
        append_at(&path, &Spooled::now(SpoolOp::Clear { session: "a".into() }));

        let raw = fs::read_to_string(&path).unwrap();
        assert_eq!(raw.lines().count(), 2, "一条一行");
        let (records, bad) = read_all(&path);
        assert_eq!(bad, 0);
        assert_eq!(records.len(), 2);
        match &records[0].op {
            SpoolOp::Report { body } => assert_eq!(body["state"], "working"),
            other => panic!("第一条应该是上报，实际是 {other:?}"),
        }
        match &records[1].op {
            SpoolOp::Clear { session } => assert_eq!(session, "a"),
            other => panic!("第二条应该是清除，实际是 {other:?}"),
        }
    }

    /// 超过**条数**上限时丢最老的。
    #[test]
    fn drops_the_oldest_when_too_many_lines() {
        let path = temp_path("spool.jsonl");
        for index in 0..(MAX_LINES + 10) {
            append_at(&path, &Spooled { at_ms: index as u64, op: SpoolOp::Clear { session: format!("s{index}") } });
        }

        let (records, bad) = read_all(&path);
        assert_eq!(bad, 0);
        assert_eq!(records.len(), MAX_LINES, "留下的正好是上限条");
        // 第一条是第 10 条 —— 前 10 条（最老的）被丢掉了
        match &records[0].op {
            SpoolOp::Clear { session } => assert_eq!(session, "s10"),
            other => panic!("实际是 {other:?}"),
        }
        match &records[MAX_LINES - 1].op {
            SpoolOp::Clear { session } => assert_eq!(session, &format!("s{}", MAX_LINES + 9)),
            other => panic!("实际是 {other:?}"),
        }
    }

    /// 超过**字节数**上限时也丢最老的（大消息按字节算，不能只看条数）。
    #[test]
    fn drops_the_oldest_when_too_many_bytes() {
        let path = temp_path("spool.jsonl");
        // 每条 ~8 KiB，40 条就是 320 KiB —— 越过 256 KiB
        let filler = "x".repeat(8 * 1024);
        for index in 0..40 {
            append_at(
                &path,
                &Spooled {
                    at_ms: index as u64,
                    op: SpoolOp::Report { body: json!({ "state": "working", "message": filler }) },
                },
            );
        }

        let raw_len = fs::metadata(&path).unwrap().len() as usize;
        assert!(raw_len <= MAX_BYTES, "字节数要压回上限内，实际 {raw_len}");
        let (records, bad) = read_all(&path);
        assert_eq!(bad, 0);
        assert!(records.len() < 40, "总得丢掉几条，实际留下 {}", records.len());
        assert_eq!(records.last().map(|r| r.at_ms), Some(39), "最新的那条一定在");
        assert_eq!(
            records.first().map(|r| r.at_ms),
            Some(40 - records.len() as u64),
            "丢的必须是最老的那几条，留下的连着"
        );
    }

    /// 坏行跳过，不拖累好行 —— 文件会被手改、会被截断、会被交错写花。
    #[test]
    fn skips_bad_lines_without_losing_the_good_ones() {
        let path = temp_path("spool.jsonl");
        fs::write(
            &path,
            "{\"at_ms\":1,\"op\":{\"kind\":\"clear\",\"session\":\"a\"}}\n\
             这不是 JSON\n\
             {\"at_ms\":2,\"op\":{\"kind\":\"nonsense\"}}\n\
             \n\
             {\"at_ms\":3,\"op\":{\"kind\":\"clear\",\"session\":\"b\"}}\n",
        )
        .unwrap();

        let (records, bad) = read_all(&path);
        assert_eq!(records.len(), 2, "两条好的都要读出来");
        assert_eq!(bad, 2, "坏行（非 JSON / 不认识的 kind）各记一条");
    }

    /// 整理一步不该动到还没到上限的文件（别把正常的追加也搞成重写）。
    #[test]
    fn trim_is_a_noop_below_the_caps() {
        let path = temp_path("spool.jsonl");
        for index in 0..5 {
            append_at(&path, &Spooled { at_ms: index, op: SpoolOp::Clear { session: format!("s{index}") } });
        }
        let before = fs::read_to_string(&path).unwrap();
        trim(&path).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), before);
    }
}
