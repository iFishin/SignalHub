//! 出站钩子：聚合变化时按 `when` 过滤后依次执行。
//!
//! 和 `hooks.rs` 的区别：那个是**入站**的（把智能体的 hook 装进它们的配置文件，
//! 让它们能上报进来）；这个是**出站**的（SignalHub 主动往外发或执行东西）。
//! 两个名字都在用「hook」这个词，但方向相反。
//!
//! 三条约束（另外一条：**发出去的那行字要能看懂**，见 [`compose`]）：
//! 1. **必须异步。** 触发点在状态变更路径上（HTTP 上报和 sweep 都会走到），
//!    同步发网络请求或等命令结束会把状态机卡住。
//! 2. **一条钩子失败不影响其它钩子**，也不影响状态机本身。失败只写 stderr。
//! 3. **命令要带超时。** 挂死的钩子会把 tokio 的阻塞线程池占满。

use crate::config::{Hook, HookAction, HttpPreset, Lang, Settings};
use crate::lang;
use crate::state::{Signal, Snapshot};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// 单条钩子的执行上限。HTTP 有 ureq 自己的超时；命令靠这个兜底。
const HOOK_TIMEOUT: Duration = Duration::from_secs(10);

/// 触发所有匹配的钩子。**不阻塞**：每条都丢进 `spawn_blocking`。
///
/// `transitioned` 是全局聚合本次有没有跃迁，`changes` 是本次跃迁了的组 ——
/// 前者给不限定组的钩子用，后者给限定了组的钩子用（见 `config::Hook::fires`）。
/// `settings` 和 `lang` 只用来拼那行给人看的摘要（组显示名要按用户改的名字来，
/// 文案要按界面语言来）。
pub fn fire(
    settings: &Settings,
    snapshot: &Snapshot,
    transitioned: bool,
    changes: &[(String, Signal)],
    lang: Lang,
    log: &OutboundLog,
) {
    let aggregate = snapshot.aggregate.name();

    for hook in settings
        .hooks
        .iter()
        .filter(|hook| hook.fires(aggregate, transitioned, changes))
    {
        // 还没填目标的钩子不触发 —— 它是编辑器里的中间态，不是错误
        if !hook.action.is_configured() {
            continue;
        }

        let action = hook.action.clone();
        let payload = match serde_json::to_string(&Payload {
            snapshot,
            at_ms: now_ms(),
        }) {
            Ok(payload) => payload,
            Err(err) => {
                eprintln!("[signalhub] 序列化钩子载荷失败: {err}");
                continue;
            }
        };
        // 摘要**按这条钩子**算：限定了组的写它自己那几个组里最紧急的那个。
        let line = summarize(lang, lang::text(lang).summary_prefix, &subject(hook, snapshot, changes, settings));
        dispatch(action, line, payload, log.clone());
    }
}

/// 把所有**已配置**的钩子各发一遍 —— 「发送测试」按钮。
///
/// 它**忽略 `when`、忽略组过滤、忽略跃迁判据**：用户就是想现在验证一次「这条通路通不通」，
/// 而等下一次真实跃迁可能等一整天。连「等不到」这件事本身都没法判断 —— 这正是
/// 「疑似不可用」的来源。
///
/// 除此之外**和真实触发走的是同一条路**（同一个 [`compose`]、同一个 `post`、同一份
/// 日志）：测试按钮必须发出和真实触发**一模一样**的请求，否则它会通过而真实触发失败，
/// 那比没有测试按钮更糟。唯一的区别是前缀（`SignalHub 测试`）——测试消息必须看起来
/// 就像测试，否则用户会以为真出事了。
///
/// 摘要写的是**当前最紧急的那一组**：一条测试消息要能当样本看（「真出事时手机上大概
/// 长这样」），写死一句「测试」就没这个作用了。没填目标的钩子跳过（同 [`fire`]）。
///
/// 返回发了几条，界面据此说「已发出 2 条」还是「还没有配好的转发」。
pub fn test_all(settings: &Settings, snapshot: &Snapshot, lang: Lang, log: &OutboundLog) -> usize {
    let subject = snapshot
        .groups
        .iter()
        .max_by_key(|group| group.aggregate.priority())
        .map(|group| Subject {
            state: group.aggregate,
            label: settings.display_label(group),
            message: group.message.as_deref(),
            since_ms: group.since_ms,
        })
        .unwrap_or(Subject {
            state: snapshot.aggregate,
            label: "",
            message: snapshot.message.as_deref(),
            since_ms: snapshot.since_ms,
        });
    let line = summarize(lang, lang::text(lang).summary_prefix_test, &subject);
    let payload = match serde_json::to_string(&Payload {
        snapshot,
        at_ms: now_ms(),
    }) {
        Ok(payload) => payload,
        Err(err) => {
            eprintln!("[signalhub] 序列化测试载荷失败: {err}");
            return 0;
        }
    };

    let mut sent = 0;
    for hook in settings.hooks.iter().filter(|hook| hook.action.is_configured()) {
        dispatch(
            hook.action.clone(),
            line.clone(),
            payload.clone(),
            log.clone(),
        );
        sent += 1;
    }
    sent
}

/// 真的把一条钩子发出去，并把结果记进日志。
///
/// [`fire`] 和 [`test_all`] 共用它 —— 两条路径必须一模一样（见 `test_all` 的说明）。
fn dispatch(action: HookAction, line: String, payload: String, log: OutboundLog) {
    tauri::async_runtime::spawn_blocking(move || {
        let (ok, detail) = match &action {
            HookAction::Http {
                url,
                headers,
                preset,
            } => match post(&compose(*preset, &line, url, headers, &payload)) {
                Ok(status) => (true, status),
                Err(err) => (false, err),
            },
            HookAction::Command { command } => match run_command(command) {
                Ok(()) => (true, String::new()),
                Err(err) => (false, err),
            },
        };
        record(&log, action.target(), ok, &detail);
    });
}

/// 这一条钩子的摘要写的是**谁**。
struct Subject<'a> {
    state: Signal,
    /// 组的显示名。全局聚合没有「哪一个组」，所以是空的。
    label: &'a str,
    message: Option<&'a str>,
    /// 这个状态的起点（epoch 毫秒）。`0` = 不必写「已经多久」。
    since_ms: u64,
}

/// 算出一条钩子的主语。
///
/// 判据和 `Hook::fires` 同源：那边已经决定了这条钩子该触发，所以限定了组时这里一定
/// 能从 `changes` 里找到自己那几个组中的一个。找不到（那一组刚好在两次快照之间消失）
/// 就退回全局聚合 —— 宁可写得不那么具体，也不要写一个已经被删掉的组名。
fn subject<'a>(
    hook: &Hook,
    snapshot: &'a Snapshot,
    changes: &[(String, Signal)],
    settings: &'a Settings,
) -> Subject<'a> {
    if !hook.groups.is_empty() {
        let winner = changes
            .iter()
            .filter(|(name, state)| {
                // 判据和 `Hook::fires` **同源**：这条钩子是为**哪一个**变化触发的。
                // 只用 `groups` 过滤的话，「ci 组变成 idle 了」也会被选成主语 ——
                // 而那条变化根本不是这条钩子关心的事（它的 `when` 里没有 idle）。
                hook.groups.iter().any(|group| group == name) && hook.matches(state.name())
            })
            .max_by_key(|(_, state)| state.priority());
        if let Some((name, state)) = winner
            && let Some(group) = snapshot.groups.iter().find(|group| &group.name == name)
        {
            return Subject {
                state: *state,
                // **用户看到的那个名字**，不是原始的组名
                label: settings.display_label(group),
                message: group.message.as_deref(),
                since_ms: group.since_ms,
            };
        }
    }

    Subject {
        state: snapshot.aggregate,
        label: "",
        message: snapshot.message.as_deref(),
        since_ms: snapshot.since_ms,
    }
}

/// 把主语拼成一行摘要：`[SignalHub] 需要授权 · work — 等你批准 (2m)`
///
/// 后面三段有内容才出现。末尾是**已经持续了多久** —— 收件人最想知道的两件事就是
/// 「什么状态」和「卡了多久」，而后者正是系统通知里催人的那一条信息。
///
/// **没有 emoji。** Rust 这边没有状态 emoji 表（`lang::signal_label` 只有文字），
/// 新加一份就是第三处「状态长什么样」的真值 —— 而这类漂移是最难自查的。收件人读的
/// 是文字。
///
/// `prefix` 走 `lang::text()`：正式通知是 `SignalHub`，**测试消息是
/// `SignalHub 测试`** —— 测试消息必须看起来就像测试，否则用户会以为真出事了。
fn summarize(lang: Lang, prefix: &str, subject: &Subject<'_>) -> String {
    let mut line = format!("[{prefix}] {}", lang::signal_label(lang, subject.state));
    if !subject.label.is_empty() {
        line.push_str(" · ");
        line.push_str(subject.label);
    }
    if let Some(message) = subject.message.map(str::trim).filter(|m| !m.is_empty()) {
        line.push_str(" — ");
        line.push_str(message);
    }
    if subject.since_ms > 0 {
        let secs = now_ms().saturating_sub(subject.since_ms) / 1000;
        line.push_str(&format!(" ({})", short_age(secs)));
    }
    line
}

/// 「多久了」的短写法：`45s` / `12m` / `3h` / `2d`。
///
/// 比 `lang::stuck_for` 短一档：那是系统通知正文里的句子（「已等待 5 分钟」），而这里
/// 要塞在一行摘要末尾的括号里 —— 长了会把状态本身挤掉。分段规则和灯上那段
/// 「已持续多久」（前端 `formatAge`）一致，免得同一件事在两处读起来不一样。
///
/// 单位不进 i18n：`s` / `m` / `h` / `d` 两边都这么读。
fn short_age(secs: u64) -> String {
    let (value, unit) = match secs {
        0..=59 => (secs, "s"),
        60..=3599 => (secs / 60, "m"),
        3600..=86399 => (secs / 3600, "h"),
        _ => (secs / 86400, "d"),
    };
    format!("{value}{unit}")
}

/// 一条组装好的转发请求。
///
/// 单独一个结构体而不是元组：这几样字段各有类型相近的味道，元组在调用点会变成一串
/// 看不出谁是谁的东西。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// 按哪个预设组的。`post` 之后要靠它判定「对方到底接受了没有」（见 [`verdict`]）
    /// —— 几个服务出错时照样回 200，把原因写在响应体里，只有知道是哪一家才读得对。
    pub preset: HttpPreset,
    pub url: String,
    /// 要带的头（预设自己加的那些也在里面，例如 ntfy 的 `Title`）。
    pub headers: Vec<(String, String)>,
    pub content_type: &'static str,
    pub body: String,
}

/// 组装一条转发请求。
///
/// **纯函数**：输入就是那几个参数，输出就是最终要发出去的东西 —— 所以「每个服务到底
/// 发什么形状」全都能单测，而且**只有这一处实现**。
///
/// `raw` 之外的所有预设都只发**一行文字**（[`summarize`] 拼出来的那条）。这不是偷懒：
/// 那些服务的形状本来就是「一个 URL + 一段文字」，服务端自己决定怎么排版。给它们发
/// 结构化数据，用户收到的是一坨看不懂的 JSON —— 而「转发看起来什么都没发生」正是
/// 这一轮要解决的问题。`json_payload` 只有 `raw` 用得上。
pub fn compose(
    preset: HttpPreset,
    summary: &str,
    url: &str,
    headers: &BTreeMap<String, String>,
    json_payload: &str,
) -> Request {
    let url = url.trim();
    let mut extra: Vec<(String, String)> =
        headers.iter().map(|(k, v)| (k.clone(), v.clone())).collect();

    let (url, content_type, body) = match preset {
        // 原样发我们自己的快照 JSON（向后兼容）
        HttpPreset::Raw => (url.to_string(), "application/json", json_payload.to_string()),
        // ntfy / Bark / 任何要一行字的地方
        HttpPreset::Text | HttpPreset::Bark => (url.to_string(), "text/plain", summary.to_string()),
        HttpPreset::Ntfy => {
            extra.push(("Title".to_string(), "SignalHub".to_string()));
            (url.to_string(), "text/plain", summary.to_string())
        }
        HttpPreset::Slack => (
            url.to_string(),
            "application/json",
            json!({ "text": summary }).to_string(),
        ),
        HttpPreset::Discord => (
            url.to_string(),
            "application/json",
            json!({ "content": summary }).to_string(),
        ),
        // 飞书自定义机器人
        HttpPreset::Lark => (
            url.to_string(),
            "application/json",
            json!({ "msg_type": "text", "content": { "text": summary } }).to_string(),
        ),
        // 企业微信 / 钉钉的群机器人形状完全一样
        HttpPreset::Wecom | HttpPreset::Dingtalk => (
            url.to_string(),
            "application/json",
            json!({ "msgtype": "text", "text": { "content": summary } }).to_string(),
        ),
        HttpPreset::Telegram => {
            // `chat_id` 按 Telegram 的文档是**查询参数**，于是它天然会被写进 URL
            // （`…/sendMessage?chat_id=123`）。但 Telegram 的 sendMessage 只认表单体，
            // 所以整条查询串要**搬进 body**，URL 上不留 —— 留着的话，同一个参数会以
            // 两种方式各出现一次。裸的 `…/sendMessage`（没有 chat_id）也允许：Telegram
            // 会回一个 400，而那条状态码正好通过「非 2xx 带码回来」显示给用户。
            let (base, query) = url.split_once('?').unwrap_or((url, ""));
            let mut body = String::from(query);
            if !body.is_empty() {
                body.push('&');
            }
            body.push_str("text=");
            body.push_str(&form_encode(summary));
            (
                base.to_string(),
                "application/x-www-form-urlencoded",
                body,
            )
        }
        HttpPreset::Pushdeer => {
            // 官方给的地址本身就带 `pushkey`（`…/message/push?pushkey=…`），**不动
            // 用户填的东西** —— 实测查询串和表单体会被合起来读，所以 key 留在它
            // 文档里的位置，正文走 body。
            //
            // 两处**实测**：只填服务器根地址（自架版 README 给的就是 `host:8800`）
            // 时 POST 到根路径回 405，所以空路径补上官方的 `/message/push`
            // （见 [`pushdeer_endpoint`]）；而它默认按 **markdown** 渲染正文，摘要
            // 里的 `·` `—`、会话消息里的 `*` `_` 都会被当成标记 —— 所以固定
            // `type=text` 发纯文本。
            (
                pushdeer_endpoint(url),
                "application/x-www-form-urlencoded",
                format!("text={}&type=text", form_encode(summary)),
            )
        }
    };

    Request {
        preset,
        url,
        headers: extra,
        content_type,
        body,
    }
}

/// 补全 PushDeer 的路径：用户只填了服务器地址时加上官方的 `/message/push`。
///
/// 只在**主机之后没有路径**时才动手（`https://api2.pushdeer.com`、
/// `http://192.168.1.5:8800/`）。写全了的一律原样返回 —— 包括反代在别的路径下的
/// 自建服务（`https://pd.example.com/api/message/push`），那种地址我们猜不出来，
/// 也不该猜。查询串（`?pushkey=…`）跟着搬过去。
fn pushdeer_endpoint(url: &str) -> String {
    let (base, query) = match url.split_once('?') {
        Some((base, query)) => (base, Some(query)),
        None => (url, None),
    };
    let base = base.trim_end_matches('/');
    // 主机之后还有没有路径？`https://host:8800` 没有（整条就是主机），
    // `https://host:8800/message/push` 有。
    let has_path = base
        .split_once("://")
        .map(|(_, rest)| rest.contains('/'))
        .unwrap_or(false);
    if has_path {
        return url.to_string();
    }
    match query {
        Some(query) => format!("{base}/message/push?{query}"),
        None => format!("{base}/message/push"),
    }
}

/// `application/x-www-form-urlencoded` 的转义。
///
/// 手写而不引一个依赖：规则只有一条 —— 非 unreserved 的字节一律 `%XX`（空格用
/// `%20`），一共十行。摘要里会有中文、`·`、`—`、括号，全都得转。
fn form_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// 一条钩子的执行结果。
///
/// 存在的理由很具体：「我的服务器没收到」这件事，光看 stderr 是查不出来的（从
/// Dock 启动的话 stderr 根本没地方去）。界面上要能回答「发出去没有、对方回了几」。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attempt {
    pub at_ms: u64,
    /// 目标（网址或命令）。
    ///
    /// **按目标而不是按下标去对应钩子**：钩子列表可以增删、可以排序，下标会错位，
    /// 于是界面上会显示成「另一条钩子的结果」—— 比不显示更糟。
    pub target: String,
    pub ok: bool,
    /// 成功是 HTTP 状态码（命令为空），失败是原因。
    pub detail: String,
}

/// 最近几次出站调用，从旧到新。
pub type OutboundLog = Arc<Mutex<VecDeque<Attempt>>>;

/// 记多少条。出站钩子本来就不频繁（只在跃迁时触发），200 条够回溯很久。
const LOG_CAPACITY: usize = 200;

/// 记一笔结果：写日志文件、失败时打 stderr、推进环形缓冲。
///
/// `pub(crate)`：物理灯（`devices.rs`）记进**同一个**日志 —— 用户只需要一个地方
/// 看「我配的东西到底发出去了没有」。
pub(crate) fn record(log: &OutboundLog, target: String, ok: bool, detail: &str) {
    let at_ms = now_ms();
    let outcome = if ok {
        format!("ok {detail}")
    } else {
        format!("失败: {detail}")
    };

    // 和入站（智能体调 CLI）写的是同一个文件，`→` 是出站的标记。
    // **测试里不写**：那是开发者真实的 `~/.signalhub/hooks.log`，跑一次 `cargo test`
    // 就往上添几行跟谁都不相干的东西（同下面 `persist` 的理由）。
    if !cfg!(test) {
        crate::config::append_hook_log(&format!(
            "{}  → {target:<44} {outcome}\n",
            crate::config::utc_hms()
        ));
    }
    if !ok {
        // 「出站」而不是「钩子」：物理灯也走这一条（见 `devices.rs`），
        // 而把一台灯说成钩子会让人去钩子列表里找一个不存在的东西。
        eprintln!("[signalhub] 出站失败（{target}）: {detail}");
    }

    // 锁**只拿着推一条**，落盘在锁外做：写文件比拷贝 200 条记录慢得多，而这条
    // 路径已经跑在后台线程上了，没有理由再让别的线程在锁上等一次磁盘。
    let snapshot = {
        let mut guard = log.lock().expect("outbound log poisoned");
        push_capped(
            &mut guard,
            Attempt {
                at_ms,
                target,
                ok,
                detail: detail.to_string(),
            },
        );
        guard.clone()
    };
    persist(&snapshot);
}

/// 推进环形缓冲并保持上限。**单独的纯函数**，好让「上限怎么起作用」能被测到 ——
/// 不然它只能靠往用户的 `~/.signalhub` 里写东西来验证。
fn push_capped(log: &mut VecDeque<Attempt>, attempt: Attempt) {
    log.push_back(attempt);
    while log.len() > LOG_CAPACITY {
        log.pop_front();
    }
}

/// `~/.signalhub/outbound.json`。
///
/// 和 `lamps.rs` 的 `lamp-positions.json` 一个路子：内存里留一份（界面要立刻读到），
/// 同时落盘一份（重启后还在）。**落盘那半是必须的**：日志原来是纯内存的，重启一次
/// 界面就显示「从未触发」—— 用户重启一次就会以为转发坏了，而这正是这一轮要消灭的
/// 那种「疑似」。
fn log_file() -> io::Result<PathBuf> {
    Ok(crate::config::data_dir()?.join("outbound.json"))
}

/// 启动时把上次的尾巴读回内存。
pub fn load_log() -> OutboundLog {
    let attempts = log_file().map(|path| load_from(&path)).unwrap_or_default();
    Arc::new(Mutex::new(attempts))
}

/// 从磁盘读回一份，**读不出来就当空的**。
///
/// 这只是历史记录：文件不在（第一次跑）、格式坏了（手改过）、没有权限，都不该拦住
/// 程序起来。坏文件会打一行 stderr —— 静默吞掉的话，「转发结果一直是空的」会变成
/// 一个查不出来的现象。
fn load_from(path: &Path) -> VecDeque<Attempt> {
    match fs::read_to_string(path) {
        // 第一次跑：文件本来就不该在，不算异常
        Err(err) if err.kind() == io::ErrorKind::NotFound => VecDeque::new(),
        Err(err) => {
            eprintln!("[signalhub] 读转发结果失败（{}）: {err}", path.display());
            VecDeque::new()
        }
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|err| {
            eprintln!("[signalhub] 转发结果文件坏了（{}）: {err}", path.display());
            VecDeque::new()
        }),
    }
}

/// 写一份到磁盘。**失败只打 stderr**：记不住一条历史结果不该影响转发本身 ——
/// 那是用户真正要的事，而且这条路径已经在后台线程上，报错也没人接。
fn persist(attempts: &VecDeque<Attempt>) {
    // 测试里不落盘：`log_file()` 是**真实的** `~/.signalhub/outbound.json`，
    // 跑一次 `cargo test` 就把开发者自己那份历史结果盖掉了。文件那一半（`write_log`
    // / `load_from`）由各自的测试拿临时路径直接测，所以这里跳过不丢覆盖面。
    if cfg!(test) {
        return;
    }

    let Ok(path) = log_file() else {
        return;
    };
    if let Err(err) = write_log(&path, attempts) {
        eprintln!("[signalhub] 记转发结果失败（{}）: {err}", path.display());
    }
}

fn write_log(path: &Path, attempts: &VecDeque<Attempt>) -> io::Result<()> {
    let json = serde_json::to_string_pretty(attempts)?;
    crate::config::atomic_write(path, &json, None)
}

#[derive(serde::Serialize)]
struct Payload<'a> {
    snapshot: &'a Snapshot,
    /// 触发时刻（epoch 毫秒）。接收方多半要记时间，而 since_ms 是状态起点。
    at_ms: u64,
}

/// 只看响应体的**前一小段**：要判定的信封就几十字节，而这个地址是用户自己填的
/// （可能指向不受我们控制的地方）—— 不该由它决定我们往内存里读多少。
const ENVELOPE_LIMIT: u64 = 8 * 1024;

/// 发一条请求。成功时把**状态码**带回来 —— 只报「没报错」区分不了 200 和 500，
/// 而「对方到底收没收到」正是用户要看的。
///
/// 头名或头值里有非法字符时，ureq 会把错误存进 builder、在 `send` 时抛出来 ——
/// 于是一条写错的 `Authorization` 会带着原因出现在结果里，而不是静默不发。
fn post(request: &Request) -> Result<String, String> {
    let config = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(3)))
        .timeout_per_call(Some(HOOK_TIMEOUT))
        .build();

    let mut builder = ureq::Agent::new_with_config(config)
        .post(&request.url)
        .header("Content-Type", request.content_type);
    for (name, value) in &request.headers {
        builder = builder.header(name.as_str(), value.as_str());
    }

    // ureq 默认把 4xx/5xx 也当成 Err（http_status_as_error），所以 Ok 就意味着 2xx
    let mut response = builder.send(&request.body).map_err(|err| err.to_string())?;
    let status = response.status().as_u16();
    // **body 也得看一眼**：见 [`verdict`] —— 有几个服务出错时照样回 200，原因写在
    // 响应体里。读不出来（太大、半路断了、不是 utf-8）就当「看不出形状」，退回状态码：
    // 请求本身是发出去了，不该因为多看的这一眼把它判成失败。
    let body = response
        .body_mut()
        .with_config()
        .limit(ENVELOPE_LIMIT)
        .lossy_utf8(true)
        .read_to_string()
        .unwrap_or_default();
    verdict(request.preset, status, &body)
}

/// 「对方到底接受了没有」。
///
/// **只看 HTTP 状态码会撒谎。** 几个通知服务在应用层出错时照样回 200，把原因写在
/// 响应体里：飞书 `{"code":…}`、企业微信/钉钉 `{"errcode":…}`、Slack `{"ok":false}`、
/// PushDeer `{"code":…}` —— PushDeer 这条是**实测**的：一个坏的 key 回的就是
/// `200 {"code":80501,"error":"错误的Key"}`。不读那个信封的话，填错的 key 在面板上
/// 是绿色的「✓ 200」，而手机一声不响。这正是「转发疑似不可用」里最难查的一类：
/// 界面上写着成功。
///
/// 2xx 之外的情况 ureq 直接报错（`http_status_as_error`），走不到这里。
fn verdict(preset: HttpPreset, status: u16, body: &str) -> Result<String, String> {
    match envelope_error(preset, body) {
        Some(error) => Err(error),
        None => Ok(status.to_string()),
    }
}

/// 这个服务在 200 的响应体里**明说它失败了**没有。`None` = 从 body 看不出来。
///
/// 键名是一张查过文档、PushDeer 那条还实测过的表（见 `docs/api.md` §4.9）。
/// **认不出形状就返回 `None`**：这只是顺手多看一眼，不该因为解析不出形状就把一次
/// 真发出去的推送判成失败 —— 那会变成一种新的「疑似」。
fn envelope_error(preset: HttpPreset, body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    match preset {
        // PushDeer：`{"code":0,…}` / `{"code":80501,"error":"错误的Key"}`
        HttpPreset::Pushdeer => code_failure(&value, "code", "error"),
        // 飞书自定义机器人：`{"code":0,"msg":"success"}`
        HttpPreset::Lark => code_failure(&value, "code", "msg"),
        // 企业微信 / 钉钉：`{"errcode":0,"errmsg":"ok"}`
        HttpPreset::Wecom | HttpPreset::Dingtalk => code_failure(&value, "errcode", "errmsg"),
        // Slack：`{"ok":true}` / `{"ok":false,"error":"invalid_token"}`
        HttpPreset::Slack => not_true(&value, "error"),
        // Telegram 出错时给 4xx（走不到这里），但它的信封里也有 `ok`
        HttpPreset::Telegram => not_true(&value, "description"),
        // 纯文本 / 我们自己的 JSON 快照这些没有一个统一的失败信封，只能看状态码
        HttpPreset::Raw
        | HttpPreset::Text
        | HttpPreset::Ntfy
        | HttpPreset::Bark
        | HttpPreset::Discord => None,
    }
}

/// `{"code":0}` 这类信封：`code` 不是 0，或者压根不是个数字，就算失败。
fn code_failure(value: &serde_json::Value, code_key: &str, text_key: &str) -> Option<String> {
    let code = value.get(code_key)?;
    if code.as_i64() == Some(0) {
        return None;
    }
    Some(explain(value, text_key, &format!("{code_key}={code}")))
}

/// `{"ok":true}` 这类信封。没有 `ok` 字段 = 不是这个形状，不多嘴。
fn not_true(value: &serde_json::Value, text_key: &str) -> Option<String> {
    match value.get("ok") {
        Some(serde_json::Value::Bool(true)) => None,
        Some(serde_json::Value::Bool(false)) => Some(explain(value, text_key, "ok=false")),
        _ => None,
    }
}

/// 失败原因优先用服务自己给的那句话 —— 用户要的是「为什么」，不是「失败」两个字。
/// 它没给就退回兜底那段，免得界面上只剩一个孤零零的「✗」。
fn explain(value: &serde_json::Value, text_key: &str, fallback: &str) -> String {
    value
        .get(text_key)
        .and_then(|text| text.as_str())
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

/// 跑一条 shell 命令。
///
/// 走 `sh -c` / `cmd /C`，所以用户能写管道和重定向（这正是钩子有用的地方）。
/// 输出全部丢弃：钩子是用来触发副作用的，它的 stdout 没地方去。
fn run_command(command: &str) -> Result<(), String> {
    #[cfg(windows)]
    let mut process = {
        let mut process = std::process::Command::new("cmd");
        process.args(["/C", command]);
        process
    };
    #[cfg(not(windows))]
    let mut process = {
        let mut process = std::process::Command::new("sh");
        process.args(["-c", command]);
        process
    };

    let mut child = process
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|err| format!("启动命令失败（{command}）: {err}"))?;

    // 轮询等它结束，超时就杀掉 —— 挂死的钩子不该把阻塞线程池占满
    let deadline = Instant::now() + HOOK_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return Ok(()),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                let _ = child.kill();
                return Err(format!("命令超过 {} 秒未结束，已终止", HOOK_TIMEOUT.as_secs()));
            }
            Err(err) => return Err(format!("等待命令失败: {err}")),
        }
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
    use crate::config::HookAction;

    /// 每个用例一个独立目录（同 `config.rs` 的测试）。
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let unique = format!(
                "signalhub-outbound-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::SeqCst)
            );
            let dir = std::env::temp_dir().join(unique);
            fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }

        fn file(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn attempt(at_ms: u64, target: &str, ok: bool) -> Attempt {
        Attempt {
            at_ms,
            target: target.to_string(),
            ok,
            detail: if ok { "200".into() } else { "连接被拒".into() },
        }
    }

    /// 结果**要能扛过一次重启**。日志原来是纯内存的，重启一次界面就显示「从未触发」
    /// —— 用户重启一次就以为转发坏了，而这正是这一轮要消灭的那种「疑似」。
    #[test]
    fn the_outbound_log_survives_a_restart() {
        let dir = TempDir::new();
        let path = dir.file("outbound.json");

        let mut written = VecDeque::new();
        written.push_back(attempt(1_700_000_000_000, "https://ntfy.sh/x", true));
        written.push_back(attempt(1_700_000_001_000, "curl http://x", false));
        write_log(&path, &written).unwrap();

        assert_eq!(load_from(&path), written, "写出去再读回来必须一模一样");
    }

    /// 读不出来的那份**当空的**，不让它挡住程序起来 —— 它只是历史记录。
    /// 但也不能静默：坏文件和「第一次跑」是两回事，前者要留一行 stderr。
    #[test]
    fn a_missing_or_corrupt_outbound_log_reads_as_empty() {
        let dir = TempDir::new();

        // 第一次跑：文件本来就不该在
        assert!(load_from(&dir.file("nope.json")).is_empty());

        // 手改坏了 / 被别的程序覆盖了：也只当空的
        let corrupt = dir.file("corrupt.json");
        fs::write(&corrupt, "{ 这不是 json").unwrap();
        assert!(load_from(&corrupt).is_empty());

        // 合法 JSON 但形状不对（例如整个对象而不是数组）：同样当空的
        let wrong_shape = dir.file("wrong.json");
        fs::write(&wrong_shape, r#"{"at_ms":1}"#).unwrap();
        assert!(load_from(&wrong_shape).is_empty());
    }

    /// 上限：只留最近的 `LOG_CAPACITY` 条，丢的是**最旧的**那几条。
    #[test]
    fn the_outbound_log_keeps_only_the_newest_entries() {
        let mut log = VecDeque::new();
        for index in 0..(LOG_CAPACITY + 5) {
            push_capped(&mut log, attempt(index as u64, "https://x/y", true));
        }

        assert_eq!(log.len(), LOG_CAPACITY);
        assert_eq!(log.front().unwrap().at_ms, 5, "丢的该是最旧的几条");
        assert_eq!(
            log.back().unwrap().at_ms,
            (LOG_CAPACITY + 4) as u64,
            "最新的那条必须在"
        );
    }

    fn group_view(name: &str, aggregate: Signal, message: Option<&str>) -> crate::state::GroupView {
        crate::state::GroupView {
            name: name.to_string(),
            default_label: None,
            aggregate,
            message: message.map(str::to_string),
            since_ms: 1_700_000_000_000,
            stale_progress: None,
            sessions: Vec::new(),
        }
    }

    /// 摘要的主语是**触发这条钩子的那个变化**，不是「它盯的组里最紧急的那个」。
    ///
    /// 这两者多数时候是同一个，但钩子的 `when` 和 `groups` 能各自设：「盯 ci 组、只在
    /// 出错时触发」的钩子，在 `ci` 变成 `idle` 时**不该**触发 —— 也就没有什么可写的。
    /// 要是只按 `groups` 选主语，「ci 变成 idle」这一条也会被选中，于是手机上收到一条
    /// 「会话空闲 · ci」，而用户配的是「ci 出错时告诉我」。
    #[test]
    fn the_subject_is_the_change_that_actually_triggered_the_hook() {
        let snapshot = Snapshot {
            aggregate: Signal::Error,
            paused: false,
            since_ms: 1_700_000_000_000,
            message: Some("整体出错".into()),
            groups: vec![
                group_view("ci", Signal::Error, Some("测试挂了")),
                group_view("deploy", Signal::Idle, Some("坐等")),
            ],
        };
        let settings = Settings::default();
        let hook = Hook {
            when: vec!["error".into()],
            groups: vec!["ci".into()],
            action: HookAction::http("https://x/y"),
        };

        // ci 出了错：主语是 ci 那一条，不是更紧急/更不紧急的别的组
        let picked = subject(
            &hook,
            &snapshot,
            &[("deploy".into(), Signal::Idle), ("ci".into(), Signal::Error)],
            &settings,
        );
        assert_eq!(picked.state, Signal::Error);
        assert_eq!(picked.label, "ci");
        assert_eq!(picked.message, Some("测试挂了"));
        assert_eq!(picked.since_ms, 1_700_000_000_000);

        // ci 变成 idle（`when` 里没有 idle）：没有匹配的变化，退回全局聚合 ——
        // 宁可写得不那么具体，也不要写一个和触发原因不相干的状态
        let picked = subject(
            &hook,
            &snapshot,
            &[("ci".into(), Signal::Idle), ("deploy".into(), Signal::Error)],
            &settings,
        );
        assert_eq!(picked.state, Signal::Error, "退回的是全局聚合");
        assert_eq!(picked.label, "");
        assert_eq!(picked.message, Some("整体出错"));
    }

    /// 摘要里的组名用**用户改的名字**（`Settings::display_label`），不是原始组名。
    #[test]
    fn the_subject_label_is_what_the_user_renamed_it_to() {
        let snapshot = Snapshot {
            aggregate: Signal::Error,
            paused: false,
            since_ms: 1_700_000_000_000,
            message: None,
            groups: vec![group_view("build-1", Signal::Error, None)],
        };
        let mut settings = Settings::default();
        settings.group_meta.insert(
            "build-1".to_string(),
            crate::config::GroupMeta {
                name: Some("构建".into()),
                order: None,
            },
        );
        let hook = Hook {
            when: Vec::new(),
            groups: vec!["build-1".into()],
            action: HookAction::http("https://x/y"),
        };

        let picked = subject(&hook, &snapshot, &[("build-1".into(), Signal::Error)], &settings);
        assert_eq!(picked.label, "构建", "转发里该用用户在灯上起的名字");
    }

    /// 出站是异步的（`spawn_blocking`），所以日志不是立刻就有。等它一下 ——
    /// 上限 5 秒，超时就让断言失败，而不是挂住。
    fn wait_for_attempt(log: &OutboundLog) -> Attempt {
        for _ in 0..500 {
            if let Some(attempt) = log.lock().expect("outbound log poisoned").back().cloned() {
                return attempt;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("等了 5 秒也没等到那次出站的结果");
    }

    /// **「发送测试」整条路都真的跑一遍**：组装 → 发出去 → 记进日志。
    ///
    /// 这条测试的存在理由和这一轮本身一样：出站是「看不见」的，「点了没反应」和
    /// 「其实发出去了」从外面看一模一样。所以它同时盯着三件事 —— 请求真的到了，
    /// 摘要**看起来就像测试**（不然用户会以为真出事了），结果真的进了界面读的那份
    /// 日志（不然点了等于没点）。
    #[test]
    fn the_test_button_really_sends_and_records() {
        let (addr, server) = one_shot_server("HTTP/1.1 200 OK");

        let settings = Settings {
            hooks: vec![
                Hook {
                    when: vec!["permission".into()],
                    groups: Vec::new(),
                    action: HookAction::Http {
                        url: format!("http://{addr}/ntfy"),
                        headers: [("X-Token".to_string(), "t".to_string())]
                            .into_iter()
                            .collect(),
                        preset: HttpPreset::Ntfy,
                    },
                },
                // 目标没填的那条要跳过（同 fire）：否则用户会收到一条空请求，
                // 而那条请求的失败原因跟他的配置毫无关系
                hook(&[], HookAction::http("   ")),
            ],
            ..Settings::default()
        };
        // 一条会话都没有的快照：主语退回全局聚合
        let snapshot = Snapshot {
            aggregate: Signal::Permission,
            paused: false,
            since_ms: now_ms() - 60_000,
            message: Some("等你批准".into()),
            groups: Vec::new(),
        };
        let log = OutboundLog::default();

        assert_eq!(
            test_all(&settings, &snapshot, Lang::Zh, &log),
            1,
            "只该发那条填了目标的"
        );

        let seen = server.join().unwrap();
        assert!(seen.starts_with("POST /ntfy "), "实际请求: {seen}");
        let lower = seen.to_ascii_lowercase();
        assert!(lower.contains("x-token: t"), "自定义头没带上: {seen}");
        assert!(
            lower.contains("content-type: text/plain"),
            "预设没生效: {seen}"
        );
        assert!(seen.contains("SignalHub \u{6d4b}\u{8bd5}"), "测试消息要看起来就像测试: {seen}");
        assert!(seen.contains("(1m)"), "摘要该带上已经多久了: {seen}");

        let attempt = wait_for_attempt(&log);
        assert!(attempt.ok, "结果该是成功的: {attempt:?}");
        assert_eq!(attempt.detail, "200");
        assert_eq!(attempt.target, format!("http://{addr}/ntfy"));
    }

    fn hook(when: &[&str], action: HookAction) -> Hook {
        Hook {
            when: when.iter().map(|s| s.to_string()).collect(),
            groups: Vec::new(),
            action,
        }
    }

    fn grouped_hook(when: &[&str], groups: &[&str]) -> Hook {
        Hook {
            when: when.iter().map(|s| s.to_string()).collect(),
            groups: groups.iter().map(|s| s.to_string()).collect(),
            action: HookAction::http("https://x/y"),
        }
    }

    /// 起一个只回一次的最小 HTTP 服务，把收到的那一份请求原样交回来。
    ///
    /// 必须**按 `Content-Length` 把 body 读满**：请求头和体是两个 TCP 段，
    /// 只 `read` 一次拿到的是头，于是「体没发出去」会变成一个假阳性。
    fn one_shot_server(status_line: &'static str) -> (String, std::thread::JoinHandle<String>) {
        one_shot_server_body(status_line, "ok")
    }

    /// 同上，但能指定回什么 body —— 判「200 里的错误信封」那几条要用。
    fn one_shot_server_body(
        status_line: &'static str,
        response_body: &'static str,
    ) -> (String, std::thread::JoinHandle<String>) {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut raw = Vec::new();
            let mut buf = [0u8; 1024];
            // 先把头读全（空行结束）
            let header_end = loop {
                let n = stream.read(&mut buf).unwrap();
                raw.extend_from_slice(&buf[..n]);
                if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                    break pos + 4;
                }
            };
            // 再按 Content-Length 把体读满
            let headers = String::from_utf8_lossy(&raw[..header_end]).to_ascii_lowercase();
            let len: usize = headers
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|value| value.trim().parse().ok())
                .unwrap_or(0);
            while raw.len() < header_end + len {
                let n = stream.read(&mut buf).unwrap();
                raw.extend_from_slice(&buf[..n]);
            }
            let response = format!(
                "{status_line}\r\nContent-Length: {}\r\n\r\n{response_body}",
                response_body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
            String::from_utf8_lossy(&raw).to_string()
        });
        (addr, handle)
    }

    /// HTTP 钩子**确实把 JSON 发出去了**。
    ///
    /// 这条测试存在的理由只有一个：这一路出站是「看不见」的 —— 用户只说
    /// 「我的服务器没收到」，而 stderr 从 Dock 启动时根本没地方去。有一条能跑的
    /// 端到端测试，才谈得上「转发可用」。
    #[test]
    fn a_http_hook_posts_the_json_payload() {
        let (addr, server) = one_shot_server("HTTP/1.1 200 OK");

        let request = compose(
            HttpPreset::Raw,
            "摘要没用上",
            &format!("http://{addr}/hook"),
            &BTreeMap::new(),
            r#"{"hello":"world"}"#,
        );
        let status = post(&request).unwrap();
        assert_eq!(status, "200");

        let seen = server.join().unwrap();
        assert!(seen.starts_with("POST /hook "), "实际请求: {seen}");
        assert!(
            seen.to_ascii_lowercase()
                .contains("content-type: application/json"),
            "实际请求: {seen}"
        );
        assert!(seen.ends_with(r#"{"hello":"world"}"#), "实际请求: {seen}");
    }

    /// 对方回 4xx/5xx 要被认成失败，并把状态码带回来 —— 只报「没报错」
    /// 区分不了 200 和 500，而「对方到底收没收到」正是用户要看的。
    #[test]
    fn a_non_2xx_response_is_reported_as_a_failure_with_its_status() {
        let (addr, server) = one_shot_server("HTTP/1.1 401 Unauthorized");

        let request = compose(
            HttpPreset::Text,
            "炸了",
            &format!("http://{addr}/hook"),
            &BTreeMap::new(),
            "",
        );
        let err = post(&request).unwrap_err();
        server.join().unwrap();
        assert!(err.contains("401"), "错误里要带状态码，实际: {err}");
    }

    /// 自定义请求头**真的发出去了**，而且预设自己加的头也在。
    #[test]
    fn custom_and_preset_headers_are_sent() {
        let (addr, server) = one_shot_server("HTTP/1.1 200 OK");

        let headers: BTreeMap<String, String> = [(
            "Authorization".to_string(),
            "Bearer secret".to_string(),
        )]
        .into_iter()
        .collect();
        let request = compose(
            HttpPreset::Ntfy,
            "[SignalHub] 出错",
            &format!("http://{addr}/hook"),
            &headers,
            "",
        );
        post(&request).unwrap();

        let seen = server.join().unwrap();
        let lower = seen.to_ascii_lowercase();
        assert!(lower.contains("authorization: bearer secret"), "实际请求: {seen}");
        assert!(lower.contains("title: signalhub"), "ntfy 的 Title 该带上: {seen}");
        assert!(lower.contains("content-type: text/plain"), "实际请求: {seen}");
    }

    // ---- 服务预设：形状都测在这一处 -----------------------------------------

    fn composed(preset: HttpPreset, summary: &str) -> Request {
        compose(preset, summary, "https://x/y", &BTreeMap::new(), "{}")
    }

    /// 每个预设的 content-type 和 body 形状。**这张表就是「转发到底发了什么」的
    /// 唯一一处真值** —— 用户在手机上收到的东西全在这里决定。
    #[test]
    fn every_preset_has_the_shape_its_service_expects() {
        let summary = "[SignalHub] 需要授权 · work — 等你批准";

        // raw：还是我们自己的 JSON（向后兼容那条）
        let raw = compose(HttpPreset::Raw, "摘要不该出现", "https://x/y", &BTreeMap::new(), r#"{"a":1}"#);
        assert_eq!(raw.content_type, "application/json");
        assert_eq!(raw.body, r#"{"a":1}"#);
        assert!(!raw.body.contains("摘要不该出现"), "raw 发的是结构化数据，不是那行字");

        for preset in [
            HttpPreset::Text,
            HttpPreset::Bark,
        ] {
            let request = composed(preset, summary);
            assert_eq!(request.content_type, "text/plain", "{preset:?}");
            assert_eq!(request.body, summary, "{preset:?}");
        }

        // ntfy：一字不差的正文，另加 Title
        let ntfy = composed(HttpPreset::Ntfy, summary);
        assert_eq!(ntfy.content_type, "text/plain");
        assert_eq!(ntfy.body, summary);
        assert_eq!(ntfy.headers, vec![("Title".to_string(), "SignalHub".to_string())]);

        for (preset, body) in [
            (HttpPreset::Slack, json!({ "text": summary }).to_string()),
            (HttpPreset::Discord, json!({ "content": summary }).to_string()),
            (
                HttpPreset::Lark,
                json!({ "msg_type": "text", "content": { "text": summary } }).to_string(),
            ),
            (
                HttpPreset::Wecom,
                json!({ "msgtype": "text", "text": { "content": summary } }).to_string(),
            ),
            (
                HttpPreset::Dingtalk,
                json!({ "msgtype": "text", "text": { "content": summary } }).to_string(),
            ),
        ] {
            let request = composed(preset, summary);
            assert_eq!(request.content_type, "application/json", "{preset:?}");
            assert_eq!(request.body, body, "{preset:?}");
        }

        // **每个预设都得保住那条 URL**：填错地址是用户最常犯的错，而把 URL 弄坏
        // （例如 Telegram 那条把路径也切掉）只会表现成「发不出去」，查不出来。
        let telegram = compose(
            HttpPreset::Telegram,
            summary,
            "https://api.telegram.org/botTOKEN/sendMessage",
            &BTreeMap::new(),
            "{}",
        );
        assert_eq!(telegram.url, "https://api.telegram.org/botTOKEN/sendMessage");

        // PushDeer：`pushkey` 留在用户填的 URL 上（那是它文档里的位置），正文按表单体
        let pushdeer = compose(
            HttpPreset::Pushdeer,
            summary,
            "https://api2.pushdeer.com/message/push?pushkey=PDU123",
            &BTreeMap::new(),
            "{}",
        );
        assert_eq!(
            pushdeer.url,
            "https://api2.pushdeer.com/message/push?pushkey=PDU123"
        );
        assert_eq!(
            pushdeer.content_type,
            "application/x-www-form-urlencoded"
        );
        assert!(pushdeer.body.starts_with("text="), "{}", pushdeer.body);
        assert!(pushdeer.body.ends_with("&type=text"), "{}", pushdeer.body);
        // 摘要里的中文和那几个标点都得转义，否则 PushDeer 那边会只显示一串乱码
        assert!(!pushdeer.body.contains('·'), "正文里不该有没转义的原文");
        assert!(pushdeer.body.contains("%20"), "空格该转成 %20：{}", pushdeer.body);
    }

    /// PushDeer 的 `type=text`：它**默认按 markdown 渲染**正文。会话消息里一个
    /// `_` 或 `*` 会被吃成排版标记 —— 而通知里看不懂的排版比不排版更糟。
    #[test]
    fn pushdeer_asks_for_plain_text() {
        let request = compose(
            HttpPreset::Pushdeer,
            "hi",
            "https://api2.pushdeer.com/message/push?pushkey=K",
            &BTreeMap::new(),
            "{}",
        );
        assert_eq!(request.body, "text=hi&type=text");
    }

    /// 只填服务器地址（自架版 README 给的就是 `host:8800`）也要能用：POST 到根路径
    /// **实测**回 405，所以空路径补上官方的 `/message/push`；写全了的一个字都不动
    /// —— 反代在别的路径下的地址我们猜不出来，也不该猜。
    #[test]
    fn pushdeer_fills_in_the_endpoint_the_user_left_off() {
        for (given, expected) in [
            ("http://192.168.1.5:8800", "http://192.168.1.5:8800/message/push"),
            (
                "http://192.168.1.5:8800/",
                "http://192.168.1.5:8800/message/push",
            ),
            (
                "https://api2.pushdeer.com?pushkey=K",
                "https://api2.pushdeer.com/message/push?pushkey=K",
            ),
            (
                "https://api2.pushdeer.com/message/push?pushkey=K",
                "https://api2.pushdeer.com/message/push?pushkey=K",
            ),
            (
                "https://pd.example.com/api/message/push?pushkey=K",
                "https://pd.example.com/api/message/push?pushkey=K",
            ),
        ] {
            assert_eq!(pushdeer_endpoint(given), expected, "{given}");
        }
    }

    /// **200 不代表成功。** 这几个服务在应用层出错时照样回 200，把原因写在响应体里。
    /// PushDeer 那条是**实测**的：坏的 key 回的就是 `200 {"code":80501,"error":"错误的Key"}`。
    /// 不读这个信封的话，面板上是绿色的「✓ 200」而手机一声不响。
    #[test]
    fn a_service_that_reports_an_error_inside_a_200_is_not_called_a_success() {
        // 各家的信封键名都不一样，这就是那张表
        for (preset, body, reason) in [
            (
                HttpPreset::Pushdeer,
                r#"{"code":80501,"error":"错误的Key"}"#,
                "错误的Key",
            ),
            (
                HttpPreset::Wecom,
                r#"{"errcode":93000,"errmsg":"invalid webhook url"}"#,
                "invalid webhook url",
            ),
            (
                HttpPreset::Dingtalk,
                r#"{"errcode":310000,"errmsg":"keywords not in content"}"#,
                "keywords not in content",
            ),
            (HttpPreset::Lark, r#"{"code":9499,"msg":"Bad Request"}"#, "Bad Request"),
            (
                HttpPreset::Slack,
                r#"{"ok":false,"error":"invalid_token"}"#,
                "invalid_token",
            ),
        ] {
            assert_eq!(
                envelope_error(preset, body).as_deref(),
                Some(reason),
                "{preset:?} {body}"
            );
        }

        // 成功的那些信封一个字都不算失败
        for (preset, body) in [
            (HttpPreset::Pushdeer, r#"{"code":0,"content":{"result":[]}}"#),
            (HttpPreset::Wecom, r#"{"errcode":0,"errmsg":"ok"}"#),
            (HttpPreset::Dingtalk, r#"{"errcode":0,"errmsg":"ok"}"#),
            (HttpPreset::Lark, r#"{"code":0,"msg":"success"}"#),
            (HttpPreset::Slack, r#"{"ok":true}"#),
        ] {
            assert_eq!(envelope_error(preset, body), None, "{preset:?} {body}");
        }

        // **认不出形状就不当失败** —— 这是关键的一半：有人把自己的纯文本接收端挂在
        // 某个预设上，响应是「ok」两个字，不该因此被显示成失败
        for (preset, body) in [
            (HttpPreset::Pushdeer, "SENT"),
            (HttpPreset::Pushdeer, ""),
            (HttpPreset::Slack, "ok"),
            (HttpPreset::Wecom, r#"{"hello":"world"}"#),
            (HttpPreset::Ntfy, r#"{"code":1}"#),
        ] {
            assert_eq!(envelope_error(preset, body), None, "{preset:?} {body}");
        }
    }

    /// 整条路都算上：**对方在 200 里说它失败了**，界面读到的是失败加它的原话
    /// —— 而不是绿色的一句「✓ 200」。
    #[test]
    fn a_200_with_an_error_envelope_comes_back_as_a_failure() {
        let (addr, server) = one_shot_server_body(
            "HTTP/1.1 200 OK",
            r#"{"code":80501,"error":"错误的Key"}"#,
        );

        let request = compose(
            HttpPreset::Pushdeer,
            "需要授权 · work",
            &format!("http://{addr}/message/push?pushkey=K"),
            &BTreeMap::new(),
            "{}",
        );
        let err = post(&request).unwrap_err();
        server.join().unwrap();
        assert!(err.contains("错误的Key"), "要把对方的原话带回来，实际: {err}");
    }

    /// Telegram 的 `chat_id`：URL 上的查询串要**搬进 body**，URL 上不留 ——
    /// Telegram 的 sendMessage 只认表单体，而 `chat_id` 是官方文档给的查询参数写法，
    /// 所以用户填的地址必然是带 `?chat_id=…` 的。
    #[test]
    fn telegram_moves_the_query_string_into_the_body() {
        let request = compose(
            HttpPreset::Telegram,
            "需要授权 · work",
            " https://api.telegram.org/botTOKEN/sendMessage?chat_id=123 ",
            &BTreeMap::new(),
            "{}",
        );
        assert_eq!(request.url, "https://api.telegram.org/botTOKEN/sendMessage");
        assert!(
            !request.url.contains("chat_id"),
            "URL 上不该还留着 chat_id：{}",
            request.url
        );
        assert_eq!(
            request.content_type,
            "application/x-www-form-urlencoded"
        );
        assert!(request.body.starts_with("chat_id=123&text="), "{}", request.body);
        // 中文和那几个标点都得转义，否则 Telegram 会回 400
        assert!(!request.body.contains('·'), "正文里不该有没转义的原文");
        assert!(request.body.contains("%20"), "空格该转成 %20：{}", request.body);

        // 裸的 …/sendMessage 也得能发（没有 chat_id 时 Telegram 会回 400，
        // 那条状态码正好通过「非 2xx 带码回来」显示给用户）
        let bare = compose(
            HttpPreset::Telegram,
            "hi",
            "https://api.telegram.org/botTOKEN/sendMessage",
            &BTreeMap::new(),
            "{}",
        );
        assert_eq!(bare.url, "https://api.telegram.org/botTOKEN/sendMessage");
        assert_eq!(bare.body, "text=hi");
    }

    /// 摘要：有消息 / 没消息 / 派生状态。三种都拼得出来，且**组名是显示名**。
    #[test]
    fn the_summary_reads_like_a_sentence() {
        let snapshot = crate::state::Snapshot {
            aggregate: Signal::Permission,
            paused: false,
            since_ms: now_ms() - 120_000,
            message: Some("等你批准".into()),
            groups: Vec::new(),
        };
        let subject = Subject {
            state: snapshot.aggregate,
            label: "构建",
            message: snapshot.message.as_deref(),
            since_ms: snapshot.since_ms,
        };
        let line = summarize(Lang::Zh, "SignalHub", &subject);
        assert!(line.starts_with("[SignalHub] 需要授权 · 构建 — 等你批准"), "{line}");
        assert!(line.ends_with("(2m)"), "末尾该带上已经多久了：{line}");

        // 没有消息、没有组名：只剩状态，但仍然是完整的一句话
        let bare = Subject {
            state: Signal::Stale,
            label: "",
            message: None,
            since_ms: 0,
        };
        assert_eq!(summarize(Lang::Zh, "SignalHub", &bare), "[SignalHub] 会话已沉默");
        assert_eq!(
            summarize(Lang::En, "SignalHub test", &bare),
            "[SignalHub test] Session went quiet"
        );

        // 只有空白的消息等于没有消息 —— 服务端每次上报都整体替换 message，
        // 空串是常见值，不该在牌上留一个空荡荡的破折号
        let blank = Subject {
            state: Signal::Error,
            label: "ci",
            message: Some("   "),
            since_ms: 0,
        };
        assert_eq!(summarize(Lang::Zh, "SignalHub", &blank), "[SignalHub] 出错 / 阻塞 · ci");
    }

    /// 「多久了」的几档。**边界**要准：59s 和 60s 是两个不同的单位。
    #[test]
    fn short_age_picks_the_readable_unit() {
        assert_eq!(short_age(0), "0s");
        assert_eq!(short_age(45), "45s");
        assert_eq!(short_age(59), "59s");
        assert_eq!(short_age(60), "1m");
        assert_eq!(short_age(3599), "59m");
        assert_eq!(short_age(3600), "1h");
        assert_eq!(short_age(86_399), "23h");
        assert_eq!(short_age(86_400), "1d");
    }

    #[test]
    fn an_empty_when_matches_every_state() {
        let any = hook(&[], HookAction::http("https://x/y"));
        for name in crate::state::Signal::ALL_NAMES {
            assert!(any.matches(name), "{name} 应被空 when 匹配");
        }
    }

    #[test]
    fn when_filters_to_the_listed_states_only() {
        let only_urgent = hook(
            &["permission", "error"],
            HookAction::http("https://x/y"),
        );
        assert!(only_urgent.matches("permission"));
        assert!(only_urgent.matches("error"));
        for name in ["idle", "working", "done", "stale", "offline", "off"] {
            assert!(!only_urgent.matches(name), "{name} 不该被匹配");
        }
    }

    /// 派生状态也要能作为过滤条件 —— 用户可能只想知道「会话沉默了」
    #[test]
    fn derived_states_can_be_filtered_on() {
        let stale_only = hook(&["stale"], HookAction::Command { command: "true".into() });
        assert!(stale_only.matches("stale"));
        assert!(!stale_only.matches("idle"));
    }

    // ---- 按组过滤 -------------------------------------------------------------

    /// 不限定组的钩子看**全局聚合的跃迁**。这是原有语义，不能变 —— 改成「任何组
    /// 一变就发」会让用户已经配好的钩子静默变成刷屏。
    #[test]
    fn an_ungrouped_hook_fires_only_on_an_aggregate_transition() {
        let any = hook(&["error"], HookAction::http("https://x/y"));
        let changes = vec![("ci".to_string(), Signal::Error)];

        assert!(any.fires("error", true, &changes), "聚合跃迁到 error 要触发");
        assert!(
            !any.fires("error", false, &changes),
            "聚合没跃迁就不触发 —— 否则任何组一变都会发"
        );
        assert!(!any.fires("idle", true, &changes), "when 不匹配就不触发");
    }

    /// 限定了组的钩子看**那几个组各自的跃迁**，不看全局聚合
    #[test]
    fn a_grouped_hook_fires_on_that_groups_own_transition() {
        let ci_errors = grouped_hook(&["error"], &["ci"]);

        assert!(
            !ci_errors.fires("error", true, &[("deploy".into(), Signal::Error)]),
            "别的组跃迁不该触发"
        );
        assert!(
            ci_errors.fires("working", false, &[("ci".into(), Signal::Error)]),
            "自己跃迁到 error 就触发，不需要全局聚合也跃迁"
        );
        assert!(
            !ci_errors.fires("working", false, &[("ci".into(), Signal::Working)]),
            "自己跃迁了，但状态不在 when 里"
        );
    }

    #[test]
    fn a_grouped_hook_can_watch_several_groups() {
        let both = grouped_hook(&["error"], &["ci", "deploy"]);
        assert!(both.fires("idle", false, &[("deploy".into(), Signal::Error)]));
        assert!(!both.fires("idle", false, &[("docs".into(), Signal::Error)]));
    }

    /// **这条就是按组过滤存在的理由**：一组失败时，别的组更紧急会把这件事从全局
    /// 聚合里整个盖掉 —— 只看聚合的话，ci 的失败会被另一组在等授权这件事吞掉。
    #[test]
    fn a_grouped_hook_is_not_gagged_by_a_more_urgent_group() {
        let ci_errors = grouped_hook(&["error"], &["ci"]);
        assert!(
            ci_errors.fires("permission", false, &[("ci".into(), Signal::Error)]),
            "别的组更紧急不该把 ci 的失败吞掉"
        );
    }
}
