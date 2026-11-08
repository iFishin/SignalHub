//! 存储：两份人可读的文件 + 一份内存索引。
//!
//! 结构选择的理由（也写进 `docs/architecture.md`）：
//!
//! - **不引 SQLite**。写入量极低（每链接封顶 50 条/天），消息是 append-only，
//!   查询只有「按游标顺序拉」「按来源过滤」「数 24 小时内的条数」三种，内存
//!   线性扫一遍足够；而 SQLite 要带 C 编译，Docker 构建和交叉编译多一道坎。
//! - **人可以 `cat` 排障**。`messages.jsonl` 一行一条，出问题时直接在服务器上
//!   看一眼，不用装 sqlite3。
//! - **锁内不做跨 await 的 I/O**。所有方法都是同步的，外面一个 `std::Mutex`，
//!   和 src-tauri 的习惯一致；追加写是小文件一次 write，不引入复杂度。

use crate::model::{Incoming, Source, SourceView};
use crate::token;
use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const HOUR_MS: u64 = 3_600_000;
pub const DAY_MS: u64 = 86_400_000;

/// 最多允许多少条**未吊销**的链接。上界防误操作：脚本反复创建会把客户端列表
/// 撑爆，而且每条都是一个对外可写的入口。
pub const MAX_SOURCES: usize = 50;

/// 内存里最多留多少条留言。超过就丢最旧的，同时把文件一起重写 —— 否则重启后
/// 又把丢掉的读回来，内存和磁盘就各说各话了。
pub const MAX_STORED: usize = 20_000;

pub const MAX_LABEL_CHARS: usize = 60;
pub const MAX_NOTE_CHARS: usize = 200;
pub const MAX_TITLE_CHARS: usize = 120;
pub const MAX_AGENT_CHARS: usize = 60;
pub const MAX_CONTEXT_CHARS: usize = 500;

/// 防滥用的三个旋钮 + 保留期。默认值和 `config.rs` 的环境变量默认一致。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    /// 同一 IP 每小时最多收几条。
    pub ip_hourly: u32,
    /// 同一条链接 24 小时内最多收几条。
    pub source_daily: u32,
    /// `text` 的字符上限（按字符数，不是字节）。
    pub max_text_chars: usize,
    /// 留言保留多少天。
    pub retention_days: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            ip_hourly: 10,
            source_daily: 50,
            max_text_chars: 4_000,
            retention_days: 90,
        }
    }
}

/// 收信失败的原因。每个变体对应一个稳定错误码（见 `http.rs`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcceptError {
    /// 令牌不存在或已吊销。**两种情况同码**，不给探测者区分。
    NotFound,
    BadRequest(String),
    TooLong,
    RateLimited { retry_after_secs: u64 },
    QuotaExceeded { retry_after_secs: u64 },
    Storage(String),
}

/// 服务器的一份内存状态。所有公开方法都要求 `&mut self`（读多写少，但读也要
/// 动限流窗口的惰性清理，统一成一类更不容易出错）。
pub struct Store {
    dir: PathBuf,
    limits: Limits,
    sources: Vec<Source>,
    messages: Vec<crate::model::Message>,
    next_seq: u64,
    /// IP → (窗口起点, 计数)。**不落盘**：重启清零可以接受（这是限速不是账本），
    /// 换来的是不用管文件格式和并发写。
    ip_windows: HashMap<IpAddr, (u64, u32)>,
}

impl Store {
    /// 打开数据目录（不存在就建）。会把超过保留期的留言剪掉并重写文件。
    pub fn open(dir: impl Into<PathBuf>, limits: Limits) -> io::Result<Store> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        let sources = load_sources(&dir)?;
        let mut messages = load_messages(&dir)?;
        // 文件顺序理论上就是 seq 顺序，但被手改过也照样能工作 —— 拉取游标依赖
        // 单调性，这里不假设。
        messages.sort_by_key(|m| m.seq);
        let next_seq = messages.iter().map(|m| m.seq).max().unwrap_or(0) + 1;

        let mut store = Store {
            dir,
            limits,
            sources,
            messages,
            next_seq,
            ip_windows: HashMap::new(),
        };
        store.prune(now_ms())?;
        Ok(store)
    }

    pub fn limits(&self) -> Limits {
        self.limits
    }

    pub fn source_count(&self) -> usize {
        self.sources.len()
    }

    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    // ---- 来源管理 ----

    /// 新建一条链接。`label` 必填；`note` 超长会被截断而不是拒绝（它只是备注）。
    pub fn create_source(
        &mut self,
        label: &str,
        note: Option<&str>,
        now: u64,
    ) -> Result<Source, String> {
        let label = label.trim();
        if label.is_empty() {
            return Err("名称不能为空".to_string());
        }
        if label.chars().count() > MAX_LABEL_CHARS {
            return Err(format!("名称最多 {MAX_LABEL_CHARS} 个字"));
        }
        if self.sources.iter().filter(|s| !s.revoked).count() >= MAX_SOURCES {
            return Err(format!("最多 {MAX_SOURCES} 条有效链接，先吊销几条再建"));
        }

        let source = Source {
            id: token::short_id(),
            label: label.to_string(),
            note: note
                .map(|n| cut(n.trim(), MAX_NOTE_CHARS))
                .filter(|n| !n.is_empty()),
            token: token::token(),
            created_ms: now,
            revoked: false,
        };
        self.sources.push(source.clone());
        self.save_sources().map_err(|err| format!("保存失败：{err}"))?;
        Ok(source)
    }

    /// 吊销。幂等：已经吊销的再调一次也返回成功（界面上连点两下不该报错）。
    pub fn revoke(&mut self, id: &str) -> Result<(), String> {
        let Some(source) = self.sources.iter_mut().find(|s| s.id == id) else {
            return Err(format!("没有这个来源：{id}"));
        };
        if !source.revoked {
            source.revoked = true;
            self.save_sources()
                .map_err(|err| format!("保存失败：{err}"))?;
        }
        Ok(())
    }

    /// 来源列表 + 统计（给客户端用）。
    pub fn sources_view(&self, now: u64) -> Vec<SourceView> {
        self.sources
            .iter()
            .map(|source| {
                let mut total = 0usize;
                let mut today = 0usize;
                let mut last_received_ms = 0u64;
                for message in &self.messages {
                    if message.source_id != source.id {
                        continue;
                    }
                    total += 1;
                    if message.received_ms + DAY_MS > now {
                        today += 1;
                    }
                    last_received_ms = last_received_ms.max(message.received_ms);
                }
                SourceView {
                    id: source.id.clone(),
                    label: source.label.clone(),
                    note: source.note.clone(),
                    token: source.token.clone(),
                    link: format!("/c/{}", source.token),
                    created_ms: source.created_ms,
                    revoked: source.revoked,
                    total,
                    today,
                    last_received_ms,
                }
            })
            .collect()
    }

    // ---- 收信 ----

    /// 一条公开链接的收信入口。所有校验按「便宜的先做」排序：长度 → 限速 →
    /// 配额 → 落盘。限速在配额之前：被限的 IP 不该消耗链接的日配额。
    pub fn accept(
        &mut self,
        token: &str,
        ip: IpAddr,
        now: u64,
        incoming: Incoming,
    ) -> Result<crate::model::Message, AcceptError> {
        let source = self
            .sources
            .iter()
            .find(|s| s.token == token && !s.revoked)
            .cloned()
            .ok_or(AcceptError::NotFound)?;

        let text = incoming.text.trim();
        if text.is_empty() {
            return Err(AcceptError::BadRequest("text 不能为空".to_string()));
        }
        if text.chars().count() > self.limits.max_text_chars {
            return Err(AcceptError::TooLong);
        }

        if let Some(retry_after_secs) = self.ip_wait(ip, now) {
            return Err(AcceptError::RateLimited { retry_after_secs });
        }
        if let Some(retry_after_secs) = self.quota_wait(&source.id, now) {
            return Err(AcceptError::QuotaExceeded { retry_after_secs });
        }

        let message = crate::model::Message {
            seq: self.next_seq,
            id: token::short_id(),
            source_id: source.id,
            received_ms: now,
            ip: ip.to_string(),
            title: incoming
                .title
                .map(|t| cut(t.trim(), MAX_TITLE_CHARS))
                .filter(|t| !t.is_empty()),
            text: text.to_string(),
            agent: incoming
                .agent
                .map(|a| cut(a.trim(), MAX_AGENT_CHARS))
                .filter(|a| !a.is_empty()),
            context: incoming
                .context
                .map(|c| cut(c.trim(), MAX_CONTEXT_CHARS))
                .filter(|c| !c.is_empty()),
        };

        self.next_seq += 1;
        self.append_message(&message)
            .map_err(AcceptError::Storage)?;
        self.messages.push(message.clone());
        self.bump_ip(ip);
        Ok(message)
    }

    /// 拉取：`after` 缺省 = 最近 `limit` 条（首拉/重装恢复）；给了就是从旧到新
    /// 的下一页。
    pub fn messages_after(
        &self,
        after: Option<u64>,
        limit: usize,
        source: Option<&str>,
    ) -> (Vec<crate::model::Message>, u64) {
        let limit = limit.clamp(1, 200);
        let filtered: Vec<&crate::model::Message> = self
            .messages
            .iter()
            .filter(|m| after.is_none_or(|a| m.seq > a))
            .filter(|m| source.is_none_or(|s| m.source_id == s))
            .collect();

        let page: Vec<crate::model::Message> = if after.is_none() {
            filtered
                .iter()
                .rev()
                .take(limit)
                .rev()
                .map(|m| (*m).clone())
                .collect()
        } else {
            filtered.into_iter().take(limit).cloned().collect()
        };

        let next_after = page
            .last()
            .map(|m| m.seq)
            .unwrap_or_else(|| after.unwrap_or(0));
        (page, next_after)
    }

    /// 同一 IP 还要等多少秒。返回 `None` = 现在可以收。
    ///
    /// 固定 1 小时窗（对齐到整点）：实现只有几行，且「整点重置」比滑动窗口容易
    /// 解释。代价是窗口边界的突发可以收到 2 倍，对个人留言板无所谓。
    fn ip_wait(&mut self, ip: IpAddr, now: u64) -> Option<u64> {
        let window_start = now - now % HOUR_MS;
        let entry = self.ip_windows.entry(ip).or_insert((window_start, 0));
        if entry.0 != window_start {
            *entry = (window_start, 0);
        }
        if entry.1 >= self.limits.ip_hourly {
            Some(secs_until(window_start + HOUR_MS, now))
        } else {
            None
        }
    }

    fn bump_ip(&mut self, ip: IpAddr) {
        if let Some(entry) = self.ip_windows.get_mut(&ip) {
            entry.1 += 1;
        }
    }

    /// 这条链接的滚动 24 小时配额。用**滚动窗口**而不是自然日：不依赖时区，
    /// 也不需要 chrono —— 服务器上 `TZ` 是什么都不影响配额语义。
    fn quota_wait(&self, source_id: &str, now: u64) -> Option<u64> {
        let mut count = 0u32;
        let mut oldest = 0u64;
        for message in &self.messages {
            if message.source_id != source_id || message.received_ms + DAY_MS <= now {
                continue;
            }
            count += 1;
            if oldest == 0 || message.received_ms < oldest {
                oldest = message.received_ms;
            }
        }
        if count >= self.limits.source_daily {
            Some(secs_until(oldest + DAY_MS, now))
        } else {
            None
        }
    }

    // ---- 维护 ----

    /// 剪掉过期留言 + 超过内存上限的旧消息。变了就重写文件，保证重启后一致。
    pub fn prune(&mut self, now: u64) -> io::Result<()> {
        let cutoff = now.saturating_sub(self.limits.retention_days.saturating_mul(DAY_MS));
        let before = self.messages.len();
        self.messages.retain(|m| m.received_ms >= cutoff);
        if self.messages.len() > MAX_STORED {
            let drop = self.messages.len() - MAX_STORED;
            self.messages.drain(..drop);
        }
        if self.messages.len() != before {
            self.rewrite_messages()?;
        }
        Ok(())
    }

    // ---- 文件 ----

    fn sources_path(&self) -> PathBuf {
        self.dir.join("sources.json")
    }

    fn messages_path(&self) -> PathBuf {
        self.dir.join("messages.jsonl")
    }

    fn save_sources(&self) -> io::Result<()> {
        let path = self.sources_path();
        let body = serde_json::to_vec_pretty(&self.sources)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        write_atomic(&path, &body)?;
        restrict(&path);
        Ok(())
    }

    fn append_message(&self, message: &crate::model::Message) -> Result<(), String> {
        let line = serde_json::to_string(message).map_err(|err| err.to_string())?;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.messages_path())
            .map_err(|err| format!("打开 messages.jsonl 失败：{err}"))?;
        file.write_all(line.as_bytes())
            .and_then(|()| file.write_all(b"\n"))
            .and_then(|()| file.flush())
            .map_err(|err| format!("写 messages.jsonl 失败：{err}"))
    }

    fn rewrite_messages(&self) -> io::Result<()> {
        let mut body = Vec::new();
        for message in &self.messages {
            serde_json::to_writer(&mut body, message)
                .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
            body.push(b'\n');
        }
        write_atomic(&self.messages_path(), &body)
    }
}

/// 当前时间（epoch 毫秒）。测试里都显式传 `now`，这里只给 `open` / `prune` 用。
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 按字符数截断（不切坏 UTF-8）。
fn cut(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    text.chars().take(max_chars).collect()
}

/// 距离 `deadline`（epoch 毫秒）还有多少**秒**，向上取整、至少 1。
///
/// 对外契约里 `retry_after_secs` 是秒（也是 `Retry-After` 头的单位）；内部全是
/// 毫秒，这一处转换必须在出口做，否则客户端会等 1000 倍的时间。
fn secs_until(deadline: u64, now: u64) -> u64 {
    deadline.saturating_sub(now).div_ceil(1000).max(1)
}

/// 临时文件 + rename：写到一半崩掉也不会留下半个 JSON。
fn write_atomic(path: &Path, body: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, body)?;
    fs::rename(&tmp, path)
}

/// 只有 Unix 支持改权限；`sources.json` 里有令牌，尽力设成 0600。
fn restrict(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

fn load_sources(dir: &Path) -> io::Result<Vec<Source>> {
    let path = dir.join("sources.json");
    match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{} 解析失败（{}）。它是权限 0600 的小文件，从备份恢复或删掉重建；\
                     不要用 messages.jsonl 去猜。",
                    path.display(),
                    err
                ),
            )
        }),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(err) => Err(err),
    }
}

/// 逐行解析；**坏行跳过**。一条手改坏的记录不该让整个服务起不来 —— 留言是
/// 增量数据，丢一条比全丢好。
fn load_messages(dir: &Path) -> io::Result<Vec<crate::model::Message>> {
    let path = dir.join("messages.jsonl");
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let mut out = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<crate::model::Message>(line) {
            Ok(message) => out.push(message),
            Err(err) => eprintln!(
                "跳过 messages.jsonl 第 {} 行的坏记录：{err}",
                index + 1
            ),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// 用完即删的临时目录。不引 tempfile：这份测试只需要十来行。
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> TempDir {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "signalhub-callme-{tag}-{}-{n}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("建临时目录");
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn store(tag: &str) -> (TempDir, Store) {
        let dir = TempDir::new(tag);
        let store = Store::open(dir.0.clone(), Limits::default()).expect("打开 store");
        (dir, store)
    }

    fn incoming(text: &str) -> Incoming {
        Incoming {
            text: text.to_string(),
            ..Incoming::default()
        }
    }

    fn ip(last: u8) -> IpAddr {
        IpAddr::from([10, 0, 0, last])
    }

    #[test]
    fn accepts_and_persists() {
        let (dir, mut store) = store("accept");
        // 用真实时间窗：`open` 会按保留期剪掉「90 天前」的留言，假时间戳会被剪。
        let now = now_ms();
        let source = store
            .create_source("博客", Some("about.md 页脚"), now)
            .unwrap();
        let message = store
            .accept(&source.token, ip(1), now, incoming("这段建议改一下开头"))
            .unwrap();
        assert_eq!(message.seq, 1);
        assert_eq!(message.text, "这段建议改一下开头");

        // 重新打开：seq 续接，内容还在
        let reopened = Store::open(dir.0.clone(), Limits::default()).unwrap();
        let (page, next) = reopened.messages_after(None, 10, None);
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].text, "这段建议改一下开头");
        assert_eq!(next, 1);

        let view = reopened.sources_view(now);
        assert_eq!(view[0].total, 1);
        assert_eq!(view[0].today, 1);
    }

    #[test]
    fn unknown_and_revoked_tokens_look_the_same() {
        let (_dir, mut store) = store("token");
        let source = store.create_source("文章", None, 1_000).unwrap();
        assert_eq!(
            store.accept("deadbeef", ip(1), 2_000, incoming("x")),
            Err(AcceptError::NotFound)
        );
        store.revoke(&source.id).unwrap();
        assert_eq!(
            store.accept(&source.token, ip(1), 2_000, incoming("x")),
            Err(AcceptError::NotFound)
        );
    }

    #[test]
    fn text_is_validated() {
        let (_dir, mut store) = store("text");
        let source = store.create_source("文章", None, 1_000).unwrap();
        assert!(matches!(
            store.accept(&source.token, ip(1), 2_000, incoming("   ")),
            Err(AcceptError::BadRequest(_))
        ));

        let long = "字".repeat(4_001);
        assert_eq!(
            store.accept(&source.token, ip(1), 2_000, incoming(&long)),
            Err(AcceptError::TooLong)
        );
    }

    #[test]
    fn per_ip_hourly_limit() {
        let (_dir, mut store) = store("ip");
        let source = store.create_source("文章", None, 1_000).unwrap();
        // limits.ip_hourly = 10
        for i in 0..10 {
            store
                .accept(&source.token, ip(9), 2_000 + i, incoming("hi"))
                .unwrap_or_else(|err| panic!("第 {i} 条不该被拒：{err:?}"));
        }
        match store.accept(&source.token, ip(9), 3_000, incoming("hi")) {
            Err(AcceptError::RateLimited { retry_after_secs }) => {
                assert!(retry_after_secs > 0 && retry_after_secs <= HOUR_MS / 1000);
            }
            other => panic!("应当被限速，实际：{other:?}"),
        }
        // 换一个 IP 不受影响
        store.accept(&source.token, ip(8), 3_000, incoming("hi")).unwrap();
    }

    #[test]
    fn window_resets_next_hour() {
        let (_dir, mut store) = store("window");
        let source = store.create_source("文章", None, 1_000).unwrap();
        for i in 0..10 {
            store
                .accept(&source.token, ip(9), 2_000 + i, incoming("hi"))
                .unwrap();
        }
        // 下一小时窗（对齐到整点）：用 2 * HOUR_MS 之内的时刻
        store
            .accept(&source.token, ip(9), 2 * HOUR_MS + 1, incoming("hi"))
            .expect("新窗口应当放行");
    }

    #[test]
    fn per_link_daily_quota() {
        let dir = TempDir::new("quota");
        let limits = Limits {
            source_daily: 2,
            ip_hourly: 100,
            ..Limits::default()
        };
        let mut store = Store::open(dir.0.clone(), limits).unwrap();
        let source = store.create_source("文章", None, 1_000).unwrap();
        store.accept(&source.token, ip(1), 2_000, incoming("a")).unwrap();
        store.accept(&source.token, ip(2), 2_001, incoming("b")).unwrap();
        match store.accept(&source.token, ip(3), 2_002, incoming("c")) {
            Err(AcceptError::QuotaExceeded { retry_after_secs }) => {
                // 最早那条在 2_000，24 小时后才出窗：还差 86_399.998 秒，向上取整
                assert_eq!(retry_after_secs, 86_400);
            }
            other => panic!("应当超配额，实际：{other:?}"),
        }
        // 换一条链接不受影响
        let other = store.create_source("另一篇", None, 1_000).unwrap();
        store.accept(&other.token, ip(3), 2_003, incoming("c")).unwrap();
    }

    #[test]
    fn quota_rolls_out_of_window() {
        let dir = TempDir::new("roll");
        let limits = Limits {
            source_daily: 1,
            ip_hourly: 100,
            ..Limits::default()
        };
        let mut store = Store::open(dir.0.clone(), limits).unwrap();
        let source = store.create_source("文章", None, 0).unwrap();
        store.accept(&source.token, ip(1), 1_000, incoming("a")).unwrap();
        store
            .accept(&source.token, ip(1), 1_000 + DAY_MS + 1, incoming("b"))
            .expect("过了 24 小时应当放行");
    }

    #[test]
    fn revoke_is_idempotent() {
        let (_dir, mut store) = store("revoke");
        let source = store.create_source("文章", None, 1_000).unwrap();
        store.revoke(&source.id).unwrap();
        store.revoke(&source.id).expect("再吊销一次也应当成功");
        assert!(store.revoke("nope").is_err());
    }

    #[test]
    fn source_limits() {
        let (_dir, mut store) = store("limits");
        assert!(store.create_source("   ", None, 1_000).is_err());
        assert!(store.create_source(&"字".repeat(61), None, 1_000).is_err());
        for i in 0..MAX_SOURCES {
            store
                .create_source(&format!("第 {i} 条"), None, 1_000)
                .unwrap();
        }
        assert!(store.create_source("再来一条", None, 1_000).is_err());
    }

    #[test]
    fn cursor_pagination_and_filter() {
        let (_dir, mut store) = store("cursor");
        let a = store.create_source("A", None, 0).unwrap();
        let b = store.create_source("B", None, 0).unwrap();
        for i in 0..5 {
            store.accept(&a.token, ip(1), 1_000 + i, incoming("a")).unwrap();
        }
        store.accept(&b.token, ip(1), 2_000, incoming("b")).unwrap();

        // 缺省 after = 最近 limit 条
        let (page, next) = store.messages_after(None, 3, None);
        assert_eq!(page.len(), 3);
        assert_eq!(next, page.last().unwrap().seq);

        // 增量
        let (page, next) = store.messages_after(Some(3), 10, None);
        assert_eq!(page.len(), 3);
        assert_eq!(next, 6);
        assert!(page.iter().all(|m| m.seq > 3));

        // 过滤
        let (page, _) = store.messages_after(None, 10, Some(&b.id));
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].source_id, b.id);

        // 没有新消息
        let (page, next) = store.messages_after(Some(6), 10, None);
        assert!(page.is_empty());
        assert_eq!(next, 6);
    }

    #[test]
    fn prune_drops_old_messages_and_rewrites() {
        let dir = TempDir::new("prune");
        let limits = Limits {
            retention_days: 1,
            ..Limits::default()
        };
        let mut store = Store::open(dir.0.clone(), limits).unwrap();
        let source = store.create_source("文章", None, 0).unwrap();
        store.accept(&source.token, ip(1), 1_000, incoming("旧")).unwrap();
        store
            .accept(&source.token, ip(1), 2_000 + DAY_MS, incoming("新"))
            .unwrap();
        store.prune(2_000 + DAY_MS).unwrap();
        let (page, _) = store.messages_after(None, 10, None);
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].text, "新");

        // 文件也重写了
        let text = fs::read_to_string(dir.0.join("messages.jsonl")).unwrap();
        assert_eq!(text.lines().count(), 1);
        assert!(text.contains("新"));
    }

    #[test]
    fn optional_fields_are_trimmed_not_rejected() {
        let (_dir, mut store) = store("trim");
        let source = store.create_source("文章", Some(&"备".repeat(300)), 0).unwrap();
        let message = store
            .accept(
                &source.token,
                ip(1),
                1_000,
                Incoming {
                    text: "正文".to_string(),
                    title: Some("标".repeat(200)),
                    agent: Some("  claude  ".to_string()),
                    context: Some("https://example.com/a".to_string()),
                },
            )
            .unwrap();
        assert_eq!(message.title.unwrap().chars().count(), MAX_TITLE_CHARS);
        assert_eq!(message.agent.as_deref(), Some("claude"));
        let view = store.sources_view(1_000);
        assert_eq!(view[0].note.as_ref().unwrap().chars().count(), MAX_NOTE_CHARS);
    }

    #[test]
    fn bad_jsonl_lines_are_skipped() {
        let dir = TempDir::new("badline");
        // 时间戳要在保留期之内，否则 `open` 的修剪会先把它剪掉（那是另一条测试）。
        let now = now_ms();
        fs::write(
            dir.0.join("messages.jsonl"),
            format!(
                "{{\"seq\":1,\"id\":\"a\",\"source_id\":\"s\",\"received_ms\":{now},\"text\":\"ok\"}}\nnot json\n"
            ),
        )
        .unwrap();
        let store = Store::open(dir.0.clone(), Limits::default()).unwrap();
        assert_eq!(store.message_count(), 1);
    }

    #[test]
    fn sources_file_is_private() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let (dir, mut store) = store("perm");
            store.create_source("文章", None, 0).unwrap();
            let mode = fs::metadata(dir.0.join("sources.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
