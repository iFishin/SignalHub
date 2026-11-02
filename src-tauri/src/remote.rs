//! 多机中枢：把别的机器上的看板**拉**过来。
//!
//! 完整方案见 `docs/plan-multi-machine.md`。三条要点：
//!
//! 1. **拉，不是推。** 对端不用升级 —— `GET /v1/state` 从 0.1 就有，所以一台
//!    0.4.1 的机器立刻能当对端，只要它开了「允许局域网访问」。
//! 2. **远程会话就是普通会话。** 拉回来的每条会话都按 `m:{机器}:{会话}` 的名字喂进
//!    本机同一个 `Store`，于是灯、收件箱、托盘、通知、统计、物理灯、只读面板
//!    **全部零改动**就看到了多机内容。另立一套数据结构意味着那七个消费者各写一次
//!    合并逻辑，而那七份迟早会漂移。
//! 3. **失联由 TTL 兜底。** 每个远程会话按 `3 × 轮询间隔` 上报，对端一掉线就自然
//!    过期 —— 不需要另做一套心跳超时。
//!
//! 轮询用一条 `std::thread` 而不是 async 任务：`ureq` 是阻塞的，扔进 tokio 只会
//! 白占一个 worker。和 `collect` / `rules` 同一个模式。

use crate::config::Machine;
use crate::notify::Notifier;
use crate::state::{Report, Signal, Store};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// 远程会话 id 的前缀。
///
/// 一个前缀干三件事（这也是选它的原因）：
///
/// 1. **循环防护**：拉取时跳过对端快照里以它开头的会话。A 拉 B、B 又拉了 A 时不会
///    无限套娃 —— B 里那份「A 的远程会话」会被 A 丢掉。
/// 2. **UI 判据**：收件箱据此把「跳转到终端」关掉 —— 那个 `cwd` 在对端机器上。
/// 3. 不占组名空间：组名里不会出现它，用户自己的组叫什么都不受影响。
pub const REMOTE_PREFIX: &str = "m:";

/// 机器掉线时那条健康会话的 id 尾段（`m:{机器}:__hub__`）。
///
/// 用一个正常会话 id 绝不会撞上的形状：真实会话 id 是 UUID / 智能体给的短名。
const HEALTH_TAIL: &str = "__hub__";

/// 连续失败几次才算「这台机器掉线了」。
///
/// 3 次 × 默认 2 秒 ≈ 6 秒。不设成 1：一次丢包就在灯上报错，牌会闪成红灯再变回去，
/// 比不报还烦。也不设太大：机器真挂了要让人在半分钟内知道。
const FAILURES_BEFORE_DOWN: u32 = 3;

/// 一台对端最多收多少个会话。
///
/// 对端是**不可信输入**（可能是别人的机器、也可能是有 bug 的版本）：几千个会话
/// 会把本机的 `Store` 和灯都拖死。200 足够放下任何真实工作台 —— 超过这个数，
/// 那台机器上乱的是它自己，不是 hub 该替它扛的。
const MAX_PEER_SESSIONS: usize = 200;

/// 一次拉取的超时。比轮询间隔略大，别让一台慢机器拖住整轮 —— 但也不能太小：
/// 对端第一次算快照可能要几十毫秒。
const FETCH_TIMEOUT: Duration = Duration::from_secs(3);

/// 每台机器的健康状态。给「多机」页显示，也给轮询器自己判断要不要报错。
#[derive(Debug, Clone, Serialize)]
pub struct Health {
    pub id: String,
    /// 最近一次拉取成功没有。
    pub ok: bool,
    /// 最近一次成功的时间（epoch 毫秒，0 = 从来没成功过）。
    pub last_ok_ms: u64,
    /// 最近一次成功拉取的耗时（毫秒）。
    pub latency_ms: u64,
    /// 最近一次失败的原因。成功时清空。
    pub error: Option<String>,
    /// 这次拉回来几个会话。
    pub sessions: usize,
}

/// 健康状态表。**和 `Store` 一样是共享的同一份** —— 轮询器写，设置页读。
pub type Registry = Arc<Mutex<BTreeMap<String, Health>>>;

pub fn registry() -> Registry {
    Arc::new(Mutex::new(BTreeMap::new()))
}

/// 起轮询线程。设置里一台机器都没有时也起（用户随时可能加一台），只是空转。
pub fn spawn(
    store: Arc<Mutex<Store>>,
    settings: crate::SharedSettings,
    registry: Registry,
    notify: Notifier,
) {
    std::thread::spawn(move || {
        // 连续失败次数。**只在这里存活**，不进 `Health` —— 它是判断「要不要报错」
        // 的内部计数，而 `Health` 是给人看的事实（成没成、多快、为什么）。
        let mut failures: HashMap<String, u32> = HashMap::new();

        loop {
            let (machines, poll_ms) = {
                let settings = settings.lock().expect("settings poisoned").clone();
                (settings.machines.clone(), settings.machine_poll_ms)
            };
            std::thread::sleep(Duration::from_millis(poll_ms));

            // TTL 取轮询间隔的 3 倍：连续两拍没拉到还不算过期（一次抖动不该让牌
            // 消失），第三拍一定过期。下界 6 秒 —— 用户把间隔调到 1 秒时，3 秒的
            // TTL 会让牌在正常网络抖动下频繁进出 `stale`。
            let ttl = Duration::from_millis((poll_ms * 3).max(6000));
            let now = Instant::now();

            // ---- 1. 清掉配置里已经没有的机器 ----
            //
            // 放在拉取之前：删掉一台机器之后，那一拍就不该再看到它的牌。
            let configured: HashSet<&str> = machines
                .iter()
                .filter(|machine| machine.enabled)
                .map(|machine| machine.id.as_str())
                .collect();
            let gone: Vec<String> = {
                let store = store.lock().expect("state store poisoned");
                store
                    .remote_machines()
                    .into_iter()
                    .filter(|id| !configured.contains(id.as_str()))
                    .collect()
            };
            let mut changed = false;
            if !gone.is_empty() {
                let mut store = store.lock().expect("state store poisoned");
                for id in &gone {
                    changed |= store.clear_machine(id, now).1;
                }
            }
            // 这些机器的健康记录和失败计数也一起清掉，免得「删了还在列表里」
            {
                let mut registry = registry.lock().expect("registry poisoned");
                for id in &gone {
                    registry.remove(id);
                }
            }
            for id in &gone {
                failures.remove(id);
            }

            // ---- 2. 逐台拉 ----
            //
            // **网络 I/O 一律在锁外**：对端可能超时 3 秒，握着 `store` 的锁等它
            // 等于让整个灯的刷新停 3 秒。
            let mut pulled: Vec<(Machine, Result<Peer, String>, u64)> = Vec::new();
            for machine in machines.iter().filter(|m| m.enabled) {
                // 地址空着 = 「刚添加还没填」，不是错误，跳过就好（见 `Machine::url`）
                if machine.url.trim().is_empty() {
                    continue;
                }
                let started = Instant::now();
                let result = fetch(machine);
                let latency = started.elapsed().as_millis() as u64;
                pulled.push((machine.clone(), result, latency));
            }

            // ---- 3. 合并进 Store，并把健康状态写下来 ----
            //
            // 先把上一轮的健康状态拷出来：失败分支要沿用「上一次成功是什么时候」，
            // 而在 `store` 的锁里再去拿 `registry` 的锁就多了一对锁序 —— 这类
            // 嵌套锁是死锁的唯一来源，而这个文件里没有任何理由需要它。
            let previous: BTreeMap<String, Health> = registry
                .lock()
                .expect("registry poisoned")
                .clone();
            let mut health_updates: Vec<Health> = Vec::new();
            {
                let mut store = store.lock().expect("state store poisoned");
                for (machine, result, latency) in pulled {
                    match result {
                        Ok(peer) => {
                            failures.remove(&machine.id);
                            let count = apply_peer(&mut store, &machine.id, &peer, ttl, now);
                            changed |= count.1;
                            health_updates.push(Health {
                                id: machine.id.clone(),
                                ok: true,
                                last_ok_ms: now_ms(),
                                latency_ms: latency,
                                error: None,
                                sessions: count.0,
                            });
                            // 拉到了就说明它活着 —— 掉线那条会话该撤掉了。
                            // 它不存在时 `clear` 什么都不做，所以无条件撤一次很便宜。
                            changed |= store.apply(
                                now,
                                off_report(&health_session(&machine.id)),
                                ttl,
                            );
                        }
                        Err(error) => {
                            let count = failures.entry(machine.id.clone()).or_insert(0);
                            *count += 1;
                            if *count >= FAILURES_BEFORE_DOWN {
                                changed |= store.apply_remote(
                                    now,
                                    down_report(&health_session(&machine.id), &machine.id, &error),
                                    ttl,
                                    &machine.id,
                                );
                            }
                            health_updates.push(Health {
                                id: machine.id.clone(),
                                ok: false,
                                // 保留上一次成功的时间：界面上「3 分钟前还能连上」
                                // 比「从来没成功过」有用得多
                                last_ok_ms: previous
                                    .get(&machine.id)
                                    .map(|h| h.last_ok_ms)
                                    .unwrap_or(0),
                                latency_ms: latency,
                                error: Some(error),
                                sessions: 0,
                            });
                        }
                    }
                }
            }

            {
                let mut registry = registry.lock().expect("registry poisoned");
                for health in health_updates {
                    registry.insert(health.id.clone(), health);
                }
            }

            // 一次轮询只推一次快照。逐个会话推会让灯在 5 台机器时每秒重排几十次。
            if changed {
                let snapshot = store.lock().expect("state store poisoned").snapshot();
                notify(&snapshot);
            }
        }
    });
}

// ---- 拉取 ------------------------------------------------------------------

/// 对端快照里我们真正用到的部分。
///
/// **每个字段都带 `#[serde(default)]`**：对端可能比我们新、也可能比我们旧，任何
/// 一个字段缺失都不该让整次拉取失败 —— 少显示一条消息，比整台机器变灰好。
#[derive(Debug, Default, Deserialize)]
struct Peer {
    #[serde(default)]
    groups: Vec<PeerGroup>,
}

#[derive(Debug, Default, Deserialize)]
struct PeerGroup {
    #[serde(default)]
    name: String,
    #[serde(default)]
    sessions: Vec<PeerSession>,
}

#[derive(Debug, Default, Deserialize)]
struct PeerSession {
    #[serde(default)]
    id: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

fn fetch(machine: &Machine) -> Result<Peer, String> {
    let url = endpoint(&machine.url);
    let config = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(2)))
        .timeout_per_call(Some(FETCH_TIMEOUT))
        .proxy(proxy_for(&url))
        .build();

    // ureq 默认把 4xx / 5xx 也当成 `Err`（`http_status_as_error`），所以下面
    // 拿到 `Ok` 就意味着 2xx —— 不用再判一次状态码。
    let mut response = ureq::Agent::new_with_config(config)
        .get(&url)
        .header("Authorization", format!("Bearer {}", machine.token).as_str())
        .call()
        .map_err(|err| friendly(&err))?;

    // 快照是几 KB 到几十 KB。限到 4 MB 是防对端被换成了一个吐流量的服务，
    // 而不是防正常的 SignalHub。
    let text = response
        .body_mut()
        .with_config()
        .limit(4 * 1024 * 1024)
        .lossy_utf8(true)
        .read_to_string()
        .map_err(|err| short_error(&err.to_string()))?;

    serde_json::from_str::<Peer>(&text)
        .map_err(|err| format!("应答不是 SignalHub 的快照：{}", short_error(&err.to_string())))
}

/// 把 `ureq` 的错误翻译成**用户能照着做**的一句话。
///
/// 牌上的消息行只有一行的宽度，而 `ureq` 的原始错误里带着完整 URL 和一大段
/// context —— 直接显示出来，被截断的正好是「为什么失败」那半句。
fn friendly(err: &ureq::Error) -> String {
    match err {
        // 401 是这一整个功能里最常见的一种：token 抄错 / 抄的是自己那台的
        ureq::Error::StatusCode(401 | 403) => {
            "对端拒绝了 token —— 去对端「设置 → 接入」里重新抄一遍".to_string()
        }
        ureq::Error::StatusCode(code) => format!("对端返回 HTTP {code}"),
        ureq::Error::HostNotFound => "找不到这台主机（名字写错，或者不在同一个网络里）".to_string(),
        ureq::Error::Timeout(_) => "连接超时".to_string(),
        // `ConnectionRefused` 是这一整个功能里第二常见的一种：地址对了、端口对了，
        // 但那台机器上的 SignalHub 没在跑 —— 或者端口抄的是别的程序。
        ureq::Error::Io(err) if err.kind() == std::io::ErrorKind::ConnectionRefused => {
            "对端没有在监听这个端口（它上面的 SignalHub 没开？端口写错了？）".to_string()
        }
        ureq::Error::ConnectionFailed => {
            "连不上（对端没开「允许局域网访问」，或者被防火墙挡了）".to_string()
        }
        ureq::Error::BadUri(uri) => format!("地址不合法：{uri}"),
        other => short_error(&other.to_string()),
    }
}

/// 把对端快照合并进本机 `Store`。返回 `(会话数, 可见状态是否变化)`。
fn apply_peer(
    store: &mut Store,
    machine: &str,
    peer: &Peer,
    ttl: Duration,
    now: Instant,
) -> (usize, bool) {
    let mut count = 0;
    let mut changed = false;

    for group in &peer.groups {
        let group_name = group_name(machine, &group.name);
        for session in &group.sessions {
            if count >= MAX_PEER_SESSIONS {
                return (count, changed);
            }
            // 循环防护：对端自己也是个 hub 时，它快照里那些「它的远程会话」不是
            // 我们的业务 —— 转过来就会一层层套下去（见 `REMOTE_PREFIX`）。
            if session.id.starts_with(REMOTE_PREFIX) {
                continue;
            }
            let Some(state) = state_of(&session.state) else {
                continue;
            };
            let report = Report {
                state,
                session: Some(session_id(machine, &session.id)),
                group: Some(group_name.clone()),
                agent: session.agent.clone(),
                source: None,
                cwd: session.cwd.clone(),
                message: session.message.clone(),
                ttl_ms: None,
                pid: None,
                machine: None,
            };
            changed |= store.apply_remote(now, report, ttl, machine);
            count += 1;
        }
    }

    (count, changed)
}

// ---- 纯函数（可单测）-------------------------------------------------------

/// 把用户填的地址补成 `GET /v1/state` 的完整 URL。
///
/// 认这几种写法，因为用户会从四个不同的地方抄：
///
/// - `192.168.1.20:41320` —— 从对端「设置 → 接入」抄的局域网地址
/// - `http://192.168.1.20:41320` / `https://hub.example.com` —— 带 scheme 的
/// - `http://192.168.1.20:41320/v1/state` —— 把接口地址整个粘进来的
/// - 末尾带 `/` 的
///
/// **不校验形状**：`hub.local:41320` 这类 mDNS 名字是正经用法，「通不通」只有真拉
/// 一次才知道 —— 那是「测试连接」要回答的，不是这里。
fn endpoint(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('/');
    // 先去掉误粘的接口路径，再判 scheme —— 否则 `http://h/v1/state` 会变成
    // `http://h/v1/state/v1/state`
    let base = trimmed.strip_suffix("/v1/state").unwrap_or(trimmed);
    let base = base.trim_end_matches('/');
    let with_scheme = if base.starts_with("http://") || base.starts_with("https://") {
        base.to_string()
    } else {
        format!("http://{base}")
    };
    format!("{with_scheme}/v1/state")
}

/// 远程会话在本机的 id。
fn session_id(machine: &str, peer_session: &str) -> String {
    format!("{REMOTE_PREFIX}{machine}:{peer_session}")
}

/// 这次请求走不走系统代理。
///
/// **私网 / 回环 / mDNS 名字一律直连。** `ureq` 会读 `HTTP_PROXY` / `HTTPS_PROXY`，
/// 而一个公司网络里常见的代理会把 `GET http://192.168.1.20:41320/v1/state` 当成
/// 「经代理访问外网」，回一句 `CONNECT proxy failed: 502` —— 实测过。那条错误
/// 既不对（对端好好的），也没法照着修：用户唯一的出路是全局 unset `HTTP_PROXY`，
/// 而那会连带弄坏别的东西。
///
/// 其余地址（`https://hub.example.com` 这类）照旧尊重系统代理 —— 那种情况下代理
/// 可能是唯一出口，绕过它才是错的。
fn proxy_for(url: &str) -> Option<ureq::Proxy> {
    if direct_connect(url) {
        None
    } else {
        ureq::Proxy::try_from_env()
    }
}

/// URL 里的主机是不是「一定在本地网络里」。
///
/// 认三种：回环、私网地址（IPv4 的 10/8、172.16/12、192.168/16、169.254/16；
/// IPv6 的 `::1`、`fc00::/7`、`fe80::/10`）、以及 `localhost` / `*.local`。
/// **认不出来就当公网**（返回 `false`）—— 猜错的代价是「多走一次代理」，
/// 而不是「本该走代理的直连了」。
fn direct_connect(url: &str) -> bool {
    let Some(host) = host_of(url) else {
        return false;
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.eq_ignore_ascii_case("localhost") || host.to_ascii_lowercase().ends_with(".local") {
        return true;
    }
    if let Ok(ip) = host.parse::<std::net::Ipv4Addr>() {
        return ip.is_loopback() || ip.is_private() || ip.is_link_local();
    }
    if let Ok(ip) = host.parse::<std::net::Ipv6Addr>() {
        // `is_unique_local` / `is_unicast_link_local` 还是 unstable，所以看首段
        let first = ip.segments()[0];
        return ip.is_loopback()
            || (first & 0xfe00) == 0xfc00 // fc00::/7 唯一本地地址
            || (first & 0xffc0) == 0xfe80; // fe80::/10 链路本地
    }
    false
}

/// 从 `http://host:port/path` 里抠出主机。IPv6 带方括号时整段返回。
fn host_of(url: &str) -> Option<&str> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .unwrap_or(url);
    let authority = rest.split('/').next()?;
    if authority.is_empty() {
        return None;
    }
    if let Some(end) = authority.strip_prefix('[').and_then(|tail| tail.find(']')) {
        return Some(&authority[..end + 2]);
    }
    match authority.rsplit_once(':') {
        // 冒号后面全是数字才当成端口，**而且整段只能有一个冒号** —— 否则那是
        // 一个没加方括号的 IPv6（`::1` 会被切成主机 `::`、端口 `1`）。
        Some((host, port))
            if port.chars().all(|c| c.is_ascii_digit())
                && !host.contains(':') =>
        {
            Some(host)
        }
        _ => Some(authority),
    }
}

/// 远程组在本机的组名。对端没给组名时落到 `default`（和对端自己的语义一致）。
fn group_name(machine: &str, peer_group: &str) -> String {
    let peer_group = peer_group.trim();
    let peer_group = if peer_group.is_empty() { "default" } else { peer_group };
    format!("{machine}/{peer_group}")
}

/// 对端状态 → 本机状态。
///
/// - `offline` → `None`（对端没有会话，不该在我这儿造一个）
/// - `stale` → `idle`。`stale` 的含义是「它沉默超过**对端**的 TTL 了」，而这件事
///   在本机由本机自己的 TTL 重新判定；原样转发会让远程会话一进来就是灰的，而且
///   它的 `stale_progress` 是用对端时钟算的，在本机没有意义。
/// - 其余原样，认不出来的丢掉（`None`）。
fn state_of(name: &str) -> Option<Signal> {
    match name.trim().to_ascii_lowercase().as_str() {
        "offline" => None,
        "stale" => Some(Signal::Idle),
        other => Signal::from_name(other),
    }
}

fn health_session(machine: &str) -> String {
    format!("{REMOTE_PREFIX}{machine}:{HEALTH_TAIL}")
}

fn off_report(session: &str) -> Report {
    Report {
        state: Signal::Off,
        session: Some(session.to_string()),
        group: None,
        agent: None,
        source: None,
        cwd: None,
        message: None,
        ttl_ms: None,
        pid: None,
        machine: None,
    }
}

/// 「这台机器连不上了」那条会话。
///
/// 用 `error` 而不是 `attention`：它是真的出错了，而且用户明确把这台机器加进了
/// 列表 —— 不需要再问一次「要不要看」。组名用机器名本身（不带 `/`），于是牌上
/// 读作 `mac-mini · 连不上…`。
fn down_report(session: &str, machine: &str, error: &str) -> Report {
    Report {
        state: Signal::Error,
        session: Some(session.to_string()),
        group: Some(machine.to_string()),
        agent: None,
        source: None,
        cwd: None,
        message: Some(format!("连不上 · {error}")),
        ttl_ms: None,
        pid: None,
        machine: None,
    }
}

/// 错误信息截短。`ureq` 的错误里带着完整 URL 和一大段 context，塞进牌上的消息行
/// 只会被截断成一串看不出重点的字 —— 前 120 个字符足够说清「超时 / 拒绝连接 /
/// 401」。
fn short_error(raw: &str) -> String {
    let one_line = raw.replace(['\n', '\r'], " ");
    let text = one_line.trim();
    if text.chars().count() <= 120 {
        return text.to_string();
    }
    let clipped: String = text.chars().take(119).collect();
    format!("{clipped}…")
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 单次拉取，给「测试连接」按钮用。
///
/// **不碰 `Store`**：测试连接不该改变灯上的任何东西 —— 用户只是想确认地址和 token
/// 对不对，如果它顺手把对端的状态也灌进来，那「点了测试，牌多了一块」会是个惊喜
/// 还是惊吓取决于他当时在干什么。
///
/// 返回的结构和轮询器写的 [`Health`] 是同一个 —— 界面上这两处显示的是同一件事，
/// 两种形状迟早会长歪。
pub fn probe(machine: &Machine) -> Health {
    let started = Instant::now();
    let latency_ms;
    match fetch(machine) {
        Ok(peer) => {
            latency_ms = started.elapsed().as_millis() as u64;
            let sessions: usize = peer
                .groups
                .iter()
                .map(|group| {
                    group
                        .sessions
                        .iter()
                        .filter(|s| !s.id.starts_with(REMOTE_PREFIX) && state_of(&s.state).is_some())
                        .count()
                })
                .sum();
            Health {
                id: machine.id.clone(),
                ok: true,
                last_ok_ms: now_ms(),
                latency_ms,
                error: None,
                sessions,
            }
        }
        Err(error) => Health {
            id: machine.id.clone(),
            ok: false,
            last_ok_ms: 0,
            latency_ms: started.elapsed().as_millis() as u64,
            error: Some(error),
            sessions: 0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 用户会从四个不同的地方抄地址，四种都得认。
    #[test]
    fn every_way_of_writing_an_address_ends_at_v1_state() {
        for raw in [
            "192.168.1.20:41320",
            "http://192.168.1.20:41320",
            "http://192.168.1.20:41320/",
            "http://192.168.1.20:41320/v1/state",
            "  http://192.168.1.20:41320/v1/state/  ",
        ] {
            assert_eq!(
                endpoint(raw),
                "http://192.168.1.20:41320/v1/state",
                "{raw:?} 没归一成同一个地址"
            );
        }
        // https 不能被降级成 http —— 反代后面是正经用法
        assert_eq!(endpoint("https://hub.example.com"), "https://hub.example.com/v1/state");
    }

    /// 命名空间：同一台机器上的同名会话不会和本机的撞，两台机器的同名组也不会合并。
    #[test]
    fn remote_ids_and_groups_are_namespaced() {
        assert_eq!(session_id("mac-mini", "abc"), "m:mac-mini:abc");
        assert_eq!(group_name("mac-mini", "ci"), "mac-mini/ci");
        // 对端没给组名时落到 default，和对端自己的语义一致
        assert_eq!(group_name("mac-mini", "  "), "mac-mini/default");
        assert_ne!(
            session_id("a", "x"),
            session_id("b", "x"),
            "两台机器上的同名会话必须是两条"
        );
    }

    /// 状态映射：`offline` 不进来、`stale` 变 `idle`、认不出来的丢掉。
    #[test]
    fn peer_states_map_onto_ours() {
        assert_eq!(state_of("offline"), None, "对端没会话，不该在我这儿造一个");
        assert_eq!(state_of("stale"), Some(Signal::Idle));
        assert_eq!(state_of("permission"), Some(Signal::Permission));
        assert_eq!(state_of("WORKING"), Some(Signal::Working));
        assert_eq!(state_of("  "), None);
        assert_eq!(state_of("nonsense"), None);
    }

    /// 合并：对端的会话进了本机 store，组名带机器前缀，机器标记挂上了。
    #[test]
    fn a_peer_snapshot_lands_in_the_store_under_its_machine() {
        let mut store = Store::default();
        let peer = Peer {
            groups: vec![PeerGroup {
                name: "ci".into(),
                sessions: vec![
                    PeerSession {
                        id: "s1".into(),
                        state: "permission".into(),
                        agent: Some("claude-code".into()),
                        cwd: Some("/home/me/app".into()),
                        message: Some("要不要允许 rm -rf".into()),
                    },
                    // 对端自己也是 hub：它快照里的远程会话必须被丢掉
                    PeerSession {
                        id: "m:other:s9".into(),
                        state: "working".into(),
                        ..PeerSession::default()
                    },
                ],
            }],
        };

        let now = Instant::now();
        let (count, changed) =
            apply_peer(&mut store, "mac-mini", &peer, Duration::from_secs(6), now);
        assert_eq!(count, 1, "循环防护没挡住对端自己的远程会话");
        assert!(changed);

        let snapshot = store.snapshot_at(now);
        assert_eq!(snapshot.groups.len(), 1);
        let group = &snapshot.groups[0];
        assert_eq!(group.name, "mac-mini/ci");
        assert_eq!(group.aggregate, Signal::Permission);
        // 牌上读作「机器 · 组」，而不是「mac-mini/ci」
        assert_eq!(group.default_label.as_deref(), Some("mac-mini · ci"));
        assert_eq!(group.sessions[0].machine.as_deref(), Some("mac-mini"));
    }

    /// 清掉一台机器只清它自己的会话 —— 别的机器和本机的都留着。
    #[test]
    fn clearing_one_machine_leaves_the_others_alone() {
        let mut store = Store::default();
        let now = Instant::now();
        let ttl = Duration::from_secs(6);
        for (machine, session) in [("a", "s1"), ("b", "s2")] {
            let report = Report {
                state: Signal::Working,
                session: Some(session_id(machine, session)),
                group: Some(group_name(machine, "ci")),
                ..Report::new(Signal::Working)
            };
            store.apply_remote(now, report, ttl, machine);
        }
        // 本机的一条
        store.apply(now, Report::new(Signal::Idle), ttl);

        assert_eq!(store.remote_machines(), vec!["a".to_string(), "b".to_string()]);
        let (removed, changed) = store.clear_machine("a", now);
        assert_eq!(removed, 1);
        assert!(changed);
        assert_eq!(store.remote_machines(), vec!["b".to_string()]);
        // 本机那条还在
        assert_eq!(store.snapshot_at(now).groups.len(), 2);
    }

    /// 错误信息要压成一行并截断 —— 它会进牌上的消息行。
    #[test]
    fn errors_are_flattened_and_clipped() {
        let long = format!("line1\nline2 {}", "x".repeat(300));
        let text = short_error(&long);
        assert!(!text.contains('\n'));
        assert_eq!(text.chars().count(), 120);
        assert!(text.ends_with('…'));
    }

    /// 起一个只会答一次的假对端，把收到的请求行/头原样交回来。
    ///
    /// 手写而不是用 axum：这里要验的是**线上格式**（请求路径、鉴权头、响应解析），
    /// 用我们自己的服务端去验等于两边一起错也测得过。
    fn fake_peer(status: &'static str, body: &'static str) -> (u16, std::thread::JoinHandle<String>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut buffer = [0u8; 8192];
            let read = socket.read(&mut buffer).unwrap_or(0);
            let request = String::from_utf8_lossy(&buffer[..read]).to_string();
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes());
            let _ = socket.flush();
            request
        });
        (port, handle)
    }

    fn peer_at(port: u16) -> Machine {
        Machine {
            id: "mac-mini".into(),
            url: format!("127.0.0.1:{port}"),
            token: "t0ken".into(),
            enabled: true,
        }
    }

    /// 真拉一次：请求打到 `/v1/state`、带 Bearer token、应答被解成会话数。
    ///
    /// 这条测的是纯函数测不到的那一段 —— `ureq` 的调用方式、头怎么拼、对端多给
    /// 几个我们不认识的字段时会不会解析失败。
    #[test]
    fn a_real_round_trip_counts_the_peers_sessions() {
        let body = r#"{"aggregate":"permission","paused":false,"since_ms":1,
            "message":null,"future_field":42,
            "groups":[{"name":"ci","aggregate":"permission","default_label":null,
              "message":"允许?","since_ms":1,"stale_progress":null,
              "sessions":[
                {"id":"s1","state":"permission","agent":"claude-code",
                 "cwd":"/home/me/app","message":"允许?","age_ms":10,"stale_progress":null},
                {"id":"s2","state":"offline"},
                {"id":"m:other:s3","state":"working"}]}]}"#;
        let (port, handle) = fake_peer("200 OK", body);

        let health = probe(&peer_at(port));
        assert!(health.ok, "拉取失败：{:?}", health.error);
        // offline 那条不算、对端自己的远程会话也不算
        assert_eq!(health.sessions, 1);
        assert_eq!(health.id, "mac-mini");

        let request = handle.join().unwrap();
        assert!(
            request.starts_with("GET /v1/state "),
            "请求行不对：{request}"
        );
        assert!(
            request.to_lowercase().contains("authorization: bearer t0ken"),
            "没带鉴权头：{request}"
        );
    }

    /// 401 要说人话：token 抄错是这个功能里最常见的一种失败。
    #[test]
    fn a_rejected_token_says_what_to_do() {
        let (port, handle) = fake_peer("401 Unauthorized", "{}");
        let health = probe(&peer_at(port));
        assert!(!health.ok);
        let error = health.error.unwrap_or_default();
        assert!(error.contains("token"), "{error}");
        assert!(error.contains("接入"), "要指出去哪里重新抄：{error}");
        let _ = handle.join();
    }

    /// 私网地址必须**绕过系统代理**。
    ///
    /// 这条是真机上撞出来的：环境里有 `HTTP_PROXY` 时，`ureq` 会把
    /// `GET http://127.0.0.1:45999/v1/state` 当成「经代理访问外网」，
    /// 于是灯上出现一句 `连不上 · CONNECT proxy failed: 502` —— 而对端好好的。
    #[test]
    fn lan_peers_bypass_the_system_proxy() {
        for url in [
            "http://192.168.1.20:41320/v1/state",
            "http://10.0.0.5:41320/v1/state",
            "http://172.16.3.9:41320/v1/state",
            "http://127.0.0.1:41320/v1/state",
            "http://localhost:41320/v1/state",
            "http://mac-mini.local:41320/v1/state",
            "http://[::1]:41320/v1/state",
            "http://[fd00::1]:41320/v1/state",
        ] {
            assert!(direct_connect(url), "{url} 应该直连");
        }
        // 公网地址照旧尊重系统代理 —— 那种情况下代理可能是唯一出口
        for url in [
            "https://hub.example.com/v1/state",
            "http://8.8.8.8/v1/state",
        ] {
            assert!(!direct_connect(url), "{url} 不该被当成局域网");
        }
    }

    /// 主机抠得对：带端口 / 不带端口 / IPv6 方括号。
    #[test]
    fn the_host_is_extracted_from_every_shape_of_url() {
        assert_eq!(host_of("http://192.168.1.20:41320/v1/state"), Some("192.168.1.20"));
        assert_eq!(host_of("https://hub.example.com/v1/state"), Some("hub.example.com"));
        assert_eq!(host_of("http://[::1]:41320/v1/state"), Some("[::1]"));
        // 没加方括号的 IPv6：最后一个冒号后面不是数字，所以整段都是主机
        assert_eq!(host_of("http://::1/v1/state"), Some("::1"));
        assert_eq!(host_of("http:///v1/state"), None);
    }
}
