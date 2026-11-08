//! Call Me：定时拉取用户自部署的收信服务器，把留言交给设置面板的「留言」页。
//!
//! 和 `remote.rs` 同一套骨架（`std::thread` + `ureq` 阻塞拉取），但**数据不进
//! `Store`**，这是一条刻意的分叉：
//!
//! - 留言不是状态信号。它没有 TTL、不参与优先级聚合、不该让灯变色，也不该混进
//!   收件箱的「需要你」——那里是「哪个智能体卡住了」，这里是「有人在你的文章下面
//!   留了话」。两件事混在一起，两个界面都会失去意义。
//! - 留言是**长期可读**的：拉回来存在内存里，页面重开还能看；而 `Store` 里的会话
//!   过期就该消失。
//!
//! 已读状态存在本地（`~/.signalhub/callme_read.json`），不回写服务器：已读是
//! 界面状态，服务器可能迁走 / 重装，它不该跟着数据走；这也让服务器保持纯
//! append-only。

use crate::config::{CallMeConfig, MAX_CALLME_POLL_MS, MIN_CALLME_POLL_MS};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::Emitter;

/// 有新数据 / 状态变化时广播给前端的事件名。前端字面量必须与之一致。
pub const CALLME_EVENT: &str = "signalhub://callme";

/// 内存里最多留多少条留言。和服务器侧的 `MAX_STORED` 不是一个数量级：
/// 客户端只负责「最近能读到的」，更久远的回服务器翻。
pub const MAX_CACHED: usize = 500;

/// 每页多少条。服务器上限 200，一次取满。
const PAGE: usize = 200;

/// 一次拉取的超时。比轮询间隔短，慢服务器不该拖住下一拍。
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);

/// 服务器的来源视图。字段与 `server/src/model.rs` 的 `SourceView` 一一对应。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SourceView {
    pub id: String,
    pub label: String,
    pub note: Option<String>,
    pub token: String,
    pub link: String,
    pub created_ms: u64,
    pub revoked: bool,
    pub total: usize,
    /// 近 24 小时收到几条（与服务器配额同口径）。
    pub today: usize,
    pub last_received_ms: u64,
}

/// 一条留言。字段与服务器 `Message` 一致，但**不带 `ip`** —— 那是服务器排障用的，
/// 没必要送到前端。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MessageView {
    pub seq: u64,
    pub id: String,
    pub source_id: String,
    pub received_ms: u64,
    pub title: Option<String>,
    pub text: String,
    pub agent: Option<String>,
    pub context: Option<String>,
}

/// 给界面的整份状态（`get_callme` 的返回，也是事件的 payload）。
#[derive(Debug, Clone, Default, Serialize)]
pub struct CallMeState {
    /// 配置完整（地址 + token 都有）。
    pub enabled: bool,
    /// 最近一次拉取成功没有。
    pub ok: bool,
    /// 最近一次成功的时间（epoch 毫秒，0 = 从来没成功过）。
    pub last_ok_ms: u64,
    /// 最近一次成功拉取的耗时（毫秒）。
    pub latency_ms: u64,
    /// 最近一次失败的原因。成功时清空。
    pub error: Option<String>,
    pub sources: Vec<SourceView>,
    /// 按 `seq` 升序。
    pub messages: Vec<MessageView>,
    /// 每个来源读到第几条（`source_id → last_read_seq`）。
    pub read: BTreeMap<String, u64>,
    /// 上一拍拉的是哪台服务器（归一化后的地址）。只用来判断「地址换了」——
    /// 换了就得把缓存清掉：`seq` 只在**同一台服务器**内单调，换了服务器之后
    /// 两边各自的 1、2、3 会按 seq 撞在一起，缓存里会混进上一台机器的留言。
    #[serde(skip)]
    pub origin: String,
}

pub type SharedCallMe = Arc<Mutex<CallMeState>>;

/// 「测试连接」的结果。不写进共享状态：它是一次性的探测。
#[derive(Debug, Clone, Default, Serialize)]
pub struct Probe {
    pub ok: bool,
    pub latency_ms: u64,
    pub error: Option<String>,
    pub version: Option<String>,
    pub sources: usize,
}

pub fn state() -> SharedCallMe {
    let initial = CallMeState {
        read: load_read(),
        ..CallMeState::default()
    };
    Arc::new(Mutex::new(initial))
}

/// 起轮询线程。没配置时也起（用户随时可能填），只是空转 —— 一圈只读一次设置。
pub fn spawn(app: tauri::AppHandle, settings: crate::SharedSettings, state: SharedCallMe) {
    std::thread::spawn(move || {
        // **首跳不睡**：用户刚填完地址切回页面，就该看到「已连接」或一条明确的
        // 错误，而不是等一个完整的 15 秒。
        loop {
            let config = settings
                .lock()
                .expect("settings poisoned")
                .callme
                .clone();
            let poll = config.poll_ms.clamp(MIN_CALLME_POLL_MS, MAX_CALLME_POLL_MS);
            if poll_once(&config, &state) {
                let snapshot = state.lock().expect("callme state poisoned").clone();
                let _ = app.emit(CALLME_EVENT, snapshot);
            }
            std::thread::sleep(Duration::from_millis(poll));
        }
    });
}

/// 跑一拍。返回「状态有变化」——只有变化才广播，别每 15 秒都推一份相同的快照。
fn poll_once(config: &CallMeConfig, state: &SharedCallMe) -> bool {
    if !config.enabled() {
        let mut current = state.lock().expect("callme state poisoned");
        if !current.enabled && current.sources.is_empty() && current.messages.is_empty() {
            return false;
        }
        current.enabled = false;
        current.ok = false;
        current.error = None;
        current.sources.clear();
        current.messages.clear();
        return true;
    }

    // 地址换了就先把缓存清空 —— 见 `origin` 字段的注释。清完 `after` 自然是 0，
    // 等于「对着新服务器从头拉最近一页」，这正是换服务器时想要的。
    let base = endpoint(&config.url);
    let switched = {
        let mut current = state.lock().expect("callme state poisoned");
        reorient(&mut current, &base)
    };

    // 只有拿到缓存里最大的 seq 才需要读锁；网络 I/O 一律在锁外。
    let after = {
        let current = state.lock().expect("callme state poisoned");
        current.messages.last().map(|message| message.seq)
    };

    let started = Instant::now();
    let fetched = fetch_messages(&base, &config.token, after)
        .and_then(|messages| fetch_sources(&base, &config.token).map(|sources| (messages, sources)));
    let latency_ms = started.elapsed().as_millis() as u64;

    let mut current = state.lock().expect("callme state poisoned");
    match fetched {
        Ok((messages, sources)) => {
            let before_len = current.messages.len();
            let was_ok = current.ok;
            merge_messages(&mut current.messages, &messages);
            let sources_changed = current.sources != sources;
            current.sources = sources;
            current.enabled = true;
            current.ok = true;
            current.last_ok_ms = now_ms();
            current.latency_ms = latency_ms;
            current.error = None;
            current.messages.len() != before_len || sources_changed || !was_ok || switched
        }
        Err(error) => {
            let changed =
                switched || current.ok || current.error.as_deref() != Some(error.as_str());
            current.enabled = true;
            current.ok = false;
            current.latency_ms = latency_ms;
            current.error = Some(error);
            changed
        }
    }
}

/// 记下这一拍打的是哪台服务器；地址变了就把缓存清空并返回 `true`。
///
/// 这看起来像个细节，其实是**换服务器时唯一不撒谎的做法**：`seq` 由各台服务器
/// 自己发号，本机服务今天才起（seq 从 1 开始），VPS 上已经有了 300 条 —— 不清
/// 就会把两边的留言按 seq 交叉拼在一起，而且 `after=<旧的最大 seq>` 会让新服务器
/// 直接跳过它自己的前几条。
fn reorient(current: &mut CallMeState, base: &str) -> bool {
    if current.origin == base {
        return false;
    }
    let had_data = !current.messages.is_empty() || !current.sources.is_empty() || current.ok;
    current.origin = base.to_string();
    current.messages.clear();
    current.sources.clear();
    current.ok = false;
    current.last_ok_ms = 0;
    current.error = None;
    had_data
}

/// 把服务端的一页合并进缓存：按 `seq` 去重（首拉 / 重装后重新拉最近 200 条，
/// 会和已有的重叠），只留最新 `MAX_CACHED` 条。
pub fn merge_messages(cached: &mut Vec<MessageView>, incoming: &[MessageView]) {
    let mut by_seq: BTreeMap<u64, MessageView> = cached
        .iter()
        .cloned()
        .map(|message| (message.seq, message))
        .collect();
    for message in incoming {
        by_seq.insert(message.seq, message.clone());
    }
    let mut merged: Vec<MessageView> = by_seq.into_values().collect();
    if merged.len() > MAX_CACHED {
        let drop = merged.len() - MAX_CACHED;
        merged.drain(..drop);
    }
    *cached = merged;
}

/// 服务器地址归一化。与 `remote::endpoint` 同一套规矩，**但默认 https**：
/// 这是公网服务，不像多机中枢那样默认在局域网里。
pub fn endpoint(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('/');
    // 先去掉误粘的接口路径，再判 scheme
    let base = trimmed
        .strip_suffix("/v1/callme")
        .unwrap_or(trimmed)
        .trim_end_matches('/');
    if base.starts_with("http://") || base.starts_with("https://") {
        base.to_string()
    } else {
        format!("https://{base}")
    }
}

// ---- HTTP ----

fn agent() -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(3)))
        .timeout_per_call(Some(FETCH_TIMEOUT))
        .build();
    ureq::Agent::new_with_config(config)
}

fn http_get(url: &str, token: &str) -> Result<String, String> {
    let mut request = agent()
        .get(url)
        .header(
            "User-Agent",
            &format!("signalhub-callme/{}", env!("CARGO_PKG_VERSION")),
        );
    if !token.is_empty() {
        request = request.header("Authorization", &format!("Bearer {token}"));
    }
    let mut response = request.call().map_err(|err| friendly(&err))?;
    response
        .body_mut()
        .with_config()
        .limit(2 * 1024 * 1024)
        .lossy_utf8(true)
        .read_to_string()
        .map_err(|err| short_error(&err.to_string()))
}

fn http_post(url: &str, token: &str, body: Option<&str>) -> Result<String, String> {
    let request = agent()
        .post(url)
        .header(
            "User-Agent",
            &format!("signalhub-callme/{}", env!("CARGO_PKG_VERSION")),
        )
        .header("Authorization", &format!("Bearer {token}"))
        .header("Content-Type", "application/json");
    let response = match body {
        Some(body) => request.send(body),
        None => request.send(""),
    };
    let mut response = response.map_err(|err| friendly(&err))?;
    response
        .body_mut()
        .with_config()
        .limit(1024 * 1024)
        .lossy_utf8(true)
        .read_to_string()
        .map_err(|err| short_error(&err.to_string()))
}

fn fetch_messages(base: &str, token: &str, after: Option<u64>) -> Result<Vec<MessageView>, String> {
    let url = match after {
        Some(after) => format!("{base}/v1/callme/messages?after={after}&limit={PAGE}"),
        // 首拉（或重装后）没有游标：取最近一页，让页面一打开就有东西读。
        None => format!("{base}/v1/callme/messages?limit={PAGE}"),
    };
    let body = http_get(&url, token)?;
    let page: MessagesPage = serde_json::from_str(&body).map_err(|err| {
        format!(
            "服务器的应答不是 Call Me 的格式：{}（确认地址填的是服务器，而不是别的服务）",
            short_error(&err.to_string())
        )
    })?;
    Ok(page.messages)
}

fn fetch_sources(base: &str, token: &str) -> Result<Vec<SourceView>, String> {
    let body = http_get(&format!("{base}/v1/callme/sources"), token)?;
    let parsed: SourcesPage = serde_json::from_str(&body)
        .map_err(|err| format!("来源列表解析失败：{}", short_error(&err.to_string())))?;
    Ok(parsed.sources)
}

/// 「测试连接」：先探健康检查（不需要 token），再拉来源（验证 token）。
pub fn probe(url: &str, token: &str) -> Probe {
    let started = Instant::now();
    let base = endpoint(url);
    let health = match http_get(&format!("{base}/v1/callme/health"), "") {
        Ok(body) => body,
        Err(error) => {
            return Probe {
                ok: false,
                latency_ms: started.elapsed().as_millis() as u64,
                error: Some(error),
                ..Probe::default()
            };
        }
    };
    let version = serde_json::from_str::<Health>(&health)
        .ok()
        .and_then(|health| health.version);
    match fetch_sources(&base, token) {
        Ok(sources) => Probe {
            ok: true,
            latency_ms: started.elapsed().as_millis() as u64,
            error: None,
            version,
            sources: sources.len(),
        },
        Err(error) => Probe {
            ok: false,
            latency_ms: started.elapsed().as_millis() as u64,
            error: Some(error),
            version,
            sources: 0,
        },
    }
}

pub fn add_source(
    url: &str,
    token: &str,
    label: &str,
    note: Option<&str>,
) -> Result<SourceView, String> {
    let base = endpoint(url);
    let body = serde_json::to_string(&serde_json::json!({
        "label": label,
        "note": note,
    }))
    .map_err(|err| err.to_string())?;
    let response = http_post(&format!("{base}/v1/callme/sources"), token, Some(&body))?;
    serde_json::from_str::<SourceView>(&response)
        .map_err(|err| format!("服务器没有返回新链接：{}", short_error(&err.to_string())))
}

pub fn revoke_source(url: &str, token: &str, id: &str) -> Result<(), String> {
    let base = endpoint(url);
    http_post(
        &format!("{base}/v1/callme/sources/{id}/revoke"),
        token,
        None,
    )
    .map(|_| ())
}

// ---- 已读 ----

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ReadFile {
    #[serde(default)]
    per_source: BTreeMap<String, u64>,
}

fn read_path() -> Option<PathBuf> {
    crate::config::data_dir()
        .ok()
        .map(|dir| dir.join("callme_read.json"))
}

fn load_read_at(path: &Path) -> BTreeMap<String, u64> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    serde_json::from_str::<ReadFile>(&raw)
        .map(|file| file.per_source)
        .unwrap_or_default()
}

fn save_read_at(path: &Path, map: &BTreeMap<String, u64>) -> Result<(), String> {
    let body = serde_json::to_string_pretty(&ReadFile {
        per_source: map.clone(),
    })
    .map_err(|err| err.to_string())?;
    crate::config::atomic_write(path, &body, Some(0o600))
        .map_err(|err| format!("记不住已读状态：{err}"))
}

pub fn load_read() -> BTreeMap<String, u64> {
    read_path()
        .map(|path| load_read_at(&path))
        .unwrap_or_default()
}

/// 把某个来源读到第 `seq` 条。**只前进不后退**：连点两下「全部已读」也不该
/// 把更早的读数改小。
pub fn mark_read(state: &SharedCallMe, source_id: &str, seq: u64) -> Result<(), String> {
    let path = read_path().ok_or("找不到 ~/.signalhub 目录")?;
    mark_read_at(state, &path, source_id, seq)
}

fn mark_read_at(
    state: &SharedCallMe,
    path: &Path,
    source_id: &str,
    seq: u64,
) -> Result<(), String> {
    let mut current = state.lock().expect("callme state poisoned");
    let previous = current.read.get(source_id).copied().unwrap_or(0);
    if seq <= previous {
        return Ok(());
    }
    current.read.insert(source_id.to_string(), seq);
    let map = current.read.clone();
    drop(current);
    save_read_at(path, &map)
}

// ---- 小工具 ----

#[derive(Debug, Deserialize)]
struct MessagesPage {
    #[serde(default)]
    messages: Vec<MessageView>,
}

#[derive(Debug, Deserialize)]
struct SourcesPage {
    #[serde(default)]
    sources: Vec<SourceView>,
}

#[derive(Debug, Deserialize)]
struct Health {
    #[serde(default)]
    version: Option<String>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// 把错误翻译成「人话」，和 `remote::friendly` 同一套语气。
fn friendly(err: &ureq::Error) -> String {
    match err {
        ureq::Error::StatusCode(401 | 403) => {
            "服务器拒绝了 token —— 核对服务器上的 CALLME_ADMIN_TOKEN，客户端里填一样的".to_string()
        }
        ureq::Error::StatusCode(404) => {
            "服务器上没有这个接口（版本不对？地址后面多写了路径？）".to_string()
        }
        ureq::Error::StatusCode(code) => format!("服务器返回 HTTP {code}"),
        ureq::Error::HostNotFound => "找不到服务器（域名写错，或者没解析）".to_string(),
        ureq::Error::Timeout(_) => "连接超时".to_string(),
        ureq::Error::Io(err) if err.kind() == std::io::ErrorKind::ConnectionRefused => {
            "服务器没有在这个端口监听（服务没起？端口写错？）".to_string()
        }
        ureq::Error::ConnectionFailed => {
            "连不上（域名 / 证书 / 防火墙哪一环？）".to_string()
        }
        ureq::Error::BadUri(uri) => format!("地址不合法：{uri}"),
        other => short_error(&other.to_string()),
    }
}

fn short_error(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= 120 {
        trimmed.to_string()
    } else {
        trimmed.chars().take(120).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_normalizes() {
        assert_eq!(endpoint("https://callme.example.com"), "https://callme.example.com");
        assert_eq!(endpoint("https://callme.example.com/"), "https://callme.example.com");
        assert_eq!(
            endpoint("  https://callme.example.com/v1/callme  "),
            "https://callme.example.com"
        );
        assert_eq!(endpoint("callme.example.com"), "https://callme.example.com");
        assert_eq!(endpoint("http://10.0.0.2:8080"), "http://10.0.0.2:8080");
        // 明确写 http 的不升级 —— 本地调试常常就是 http
        assert_eq!(endpoint("http://127.0.0.1:8080/"), "http://127.0.0.1:8080");
    }

    #[test]
    fn merge_dedupes_and_sorts() {
        let mut cached = vec![message(1, "a"), message(2, "b")];
        merge_messages(&mut cached, &[message(2, "b2"), message(3, "c")]);
        assert_eq!(cached.len(), 3);
        assert_eq!(cached[0].text, "a");
        assert_eq!(cached[1].text, "b2", "同 seq 以新到的为准");
        assert_eq!(cached[2].seq, 3);
    }

    #[test]
    fn switching_servers_drops_the_cache() {
        let mut state = CallMeState {
            enabled: true,
            ok: true,
            last_ok_ms: 42,
            messages: vec![message(1, "a"), message(2, "b")],
            sources: vec![SourceView {
                id: "a".into(),
                ..SourceView::default()
            }],
            origin: "http://127.0.0.1:43877".into(),
            ..CallMeState::default()
        };

        // 同一个地址：一拍不动（否则每次轮询都会把列表清空再拉回来）
        assert!(!reorient(&mut state, "http://127.0.0.1:43877"));
        assert_eq!(state.messages.len(), 2);
        assert_eq!(state.last_ok_ms, 42);

        // 换了服务器：缓存必须清掉 —— seq 只在同一台机器内单调
        assert!(reorient(&mut state, "https://callme.example.com"));
        assert!(state.messages.is_empty());
        assert!(state.sources.is_empty());
        assert!(!state.ok);
        assert_eq!(state.last_ok_ms, 0);
        assert_eq!(state.origin, "https://callme.example.com");

        // 空状态换地址没什么可清，不该触发一次多余的广播
        assert!(!reorient(&mut state, "https://other.example.com"));
    }

    #[test]
    fn merge_keeps_newest() {
        let mut cached: Vec<MessageView> = (1..=MAX_CACHED as u64).map(|i| message(i, "old")).collect();
        merge_messages(&mut cached, &[message(MAX_CACHED as u64 + 1, "new")]);
        assert_eq!(cached.len(), MAX_CACHED);
        assert_eq!(cached.last().unwrap().seq, MAX_CACHED as u64 + 1);
        assert_eq!(cached[0].seq, 2, "最旧的一条被丢掉");
    }

    #[test]
    fn read_state_roundtrip() {
        let dir = std::env::temp_dir().join(format!(
            "signalhub-callme-read-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("callme_read.json");

        let mut map = BTreeMap::new();
        map.insert("source-a".to_string(), 12u64);
        save_read_at(&path, &map).unwrap();
        assert_eq!(load_read_at(&path), map);

        // 文件不存在 / 内容坏掉都退回空表，不 panic
        assert!(load_read_at(&dir.join("nope.json")).is_empty());
        std::fs::write(dir.join("bad.json"), "not json").unwrap();
        assert!(load_read_at(&dir.join("bad.json")).is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mark_read_only_moves_forward() {
        let dir = std::env::temp_dir().join(format!(
            "signalhub-callme-mark-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("callme_read.json");

        let state = state();
        mark_read_at(&state, &path, "s", 10).unwrap();
        assert_eq!(state.lock().unwrap().read.get("s").copied(), Some(10));

        mark_read_at(&state, &path, "s", 4).unwrap();
        assert_eq!(
            state.lock().unwrap().read.get("s").copied(),
            Some(10),
            "读数不该后退"
        );

        mark_read_at(&state, &path, "s", 12).unwrap();
        assert_eq!(state.lock().unwrap().read.get("s").copied(), Some(12));
        assert_eq!(load_read_at(&path).get("s").copied(), Some(12), "落盘了");

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn message(seq: u64, text: &str) -> MessageView {
        MessageView {
            seq,
            id: format!("id{seq}"),
            source_id: "s".to_string(),
            received_ms: seq,
            title: None,
            text: text.to_string(),
            agent: None,
            context: None,
        }
    }
}
