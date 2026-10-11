//! SignalHub CLI —— 上报接口的瘦客户端。
//!
//! 三条硬性约束，改这个文件前先读：
//!
//! 1. **`hook` 模式下 stdout 必须绝对为空。** 智能体的异步 hook 若在 stdout 输出
//!    畸形 JSON 会导致会话崩溃（Claude Code v2.1.202 之前），且每次恢复会话都会
//!    重现。所以 hook 路径上不许出现任何 `println!`，诊断一律走 stderr。
//! 2. **不能依赖智能体的 hook timeout。** 异步 hook 不受 timeout 约束，必须自己
//!    限制连接与整体超时，否则一个卡住的请求会一直悬着。
//! 3. **CLI 只做传输，不校验状态名。** 状态名与别名的权威定义在服务端
//!    (`state::Signal::parse`)，这里原样转发，从而不可能出现两张别名表漂移。
//!    状态名写错会拿到服务端的 400 和它的错误文本。
//!
//! 另外 `hook` 模式隐含 `--never-fail`：上报客户端不该有能力弄坏它监控的对象。

use serde_json::{json, Value};
use std::fs;
use std::io::{Read, Write};
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// 直接包含源码，而不是 `use signalhub_lib::config`：lib.rs 是 Tauri 应用，
// 链接它会把整个 Tauri 拖进这个本该只有几 MB 的 CLI 二进制。
// config.rs / hooks.rs 里有些函数只有 app 侧用得到，CLI 只用其中一部分。
//
// state.rs 是**被动**带上的：config.rs 校验钩子的状态名时要问它有哪些状态。
// 它本身是纯逻辑（只有 serde + std），不引入任何额外依赖。
#[allow(dead_code)]
#[path = "../config.rs"]
mod config;

#[allow(dead_code)]
#[path = "../hooks.rs"]
mod hooks;

#[allow(dead_code)]
#[path = "../state.rs"]
mod state;

const CONNECT_TIMEOUT: Duration = Duration::from_millis(800);
const CALL_TIMEOUT: Duration = Duration::from_millis(1500);

const EXIT_OK: u8 = 0;
const EXIT_ERROR: u8 = 1;
const EXIT_NOT_RUNNING: u8 = 3;

const APP_NOT_RUNNING: &str = "SignalHub 没有在运行";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // hook 模式天然 never-fail。从原始参数判断，这样即使参数是坏的也成立。
    let never_fail = args.first().is_some_and(|arg| arg == "hook")
        || args.iter().any(|arg| arg == "--never-fail");

    let code = match run(&args) {
        Ok(()) => EXIT_OK,
        Err(failure) => {
            eprintln!("signalhub: {}", failure.message);
            failure.code()
        }
    };

    ExitCode::from(if never_fail { EXIT_OK } else { code })
}

#[derive(Debug)]
struct Failure {
    message: String,
    not_running: bool,
}

impl Failure {
    fn other(message: impl Into<String>) -> Self {
        Self { message: message.into(), not_running: false }
    }

    fn not_running(message: impl Into<String>) -> Self {
        Self { message: message.into(), not_running: true }
    }

    fn code(&self) -> u8 {
        if self.not_running {
            EXIT_NOT_RUNNING
        } else {
            EXIT_ERROR
        }
    }
}

// ---- 参数 ------------------------------------------------------------------

#[derive(Debug, Default)]
struct Flags {
    session: Option<String>,
    agent: Option<String>,
    cwd: Option<String>,
    message: Option<String>,
    ttl_ms: Option<u64>,
    json: bool,
    dry_run: bool,
    port: Option<u16>,
    /// watch 专用
    interval_ms: Option<u64>,
    until: Option<String>,
    timeout_ms: Option<u64>,
}

fn take_value(args: &[String], index: &mut usize, name: &str) -> Result<String, Failure> {
    *index += 1;
    args.get(*index)
        .cloned()
        .ok_or_else(|| Failure::other(format!("{name} 缺少取值")))
}

impl Flags {
    /// 把 `--x` 选项收进 Flags，其余位置参数原样返回。
    fn parse(args: &[String]) -> Result<(Self, Vec<String>), Failure> {
        let mut flags = Self::default();
        let mut positional = Vec::new();
        let mut index = 0;

        while index < args.len() {
            let arg = args[index].as_str();
            match arg {
                "--session" => flags.session = Some(take_value(args, &mut index, "--session")?),
                "--agent" => flags.agent = Some(take_value(args, &mut index, "--agent")?),
                "--cwd" => flags.cwd = Some(take_value(args, &mut index, "--cwd")?),
                "--message" => flags.message = Some(take_value(args, &mut index, "--message")?),
                "--ttl" => {
                    let raw = take_value(args, &mut index, "--ttl")?;
                    flags.ttl_ms = Some(parse_ttl(&raw)?);
                }
                "--port" => {
                    let raw = take_value(args, &mut index, "--port")?;
                    flags.port = Some(raw.parse().map_err(|_| {
                        Failure::other(format!("--port 需要数字，收到 {raw:?}"))
                    })?);
                }
                "--json" => flags.json = true,
                "--dry-run" => flags.dry_run = true,
                "--interval" => {
                    let raw = take_value(args, &mut index, "--interval")?;
                    flags.interval_ms = Some(parse_ttl(&raw)?);
                }
                "--timeout" => {
                    let raw = take_value(args, &mut index, "--timeout")?;
                    flags.timeout_ms = Some(parse_ttl(&raw)?);
                }
                "--until" => flags.until = Some(take_value(args, &mut index, "--until")?),
                "--never-fail" => {} // main 已经从原始参数里处理过了
                other if other.starts_with("--") => {
                    return Err(Failure::other(format!("未知选项 {other}，用 --help 看用法")));
                }
                other => positional.push(other.to_string()),
            }
            index += 1;
        }

        Ok((flags, positional))
    }
}

/// 接受 `30s` / `5m` / `1h` / `250ms` / 裸数字（按秒）。
fn parse_ttl(raw: &str) -> Result<u64, Failure> {
    let raw = raw.trim();
    // "ms" 必须在 "s" 之前判断，否则 "30ms" 会被剥成 "30m" 然后解析失败
    let (digits, per_unit) = if let Some(rest) = raw.strip_suffix("ms") {
        (rest, 1u64)
    } else if let Some(rest) = raw.strip_suffix('s') {
        (rest, 1_000)
    } else if let Some(rest) = raw.strip_suffix('m') {
        (rest, 60_000)
    } else if let Some(rest) = raw.strip_suffix('h') {
        (rest, 3_600_000)
    } else {
        (raw, 1_000)
    };

    let value: u64 = digits
        .trim()
        .parse()
        .map_err(|_| Failure::other(format!("无法解析时长 {raw:?}，试试 30s / 5m / 1h")))?;
    value
        .checked_mul(per_unit)
        .ok_or_else(|| Failure::other(format!("时长 {raw:?} 过大")))
}

fn run(args: &[String]) -> Result<(), Failure> {
    // 用户把 --help 写进 hook 命令里是个真实的手滑，而 hook 的 stdout 一个字
    // 都不能有，所以这种情况下帮助信息改走 stderr。
    let running_as_hook = args.first().is_some_and(|arg| arg == "hook");

    if args.is_empty() || args.iter().any(|arg| arg == "-h" || arg == "--help") {
        if running_as_hook {
            eprint!("{}", help());
        } else {
            print!("{}", help());
        }
        return Ok(());
    }
    if args.iter().any(|arg| arg == "-V" || arg == "--version") {
        println!("signalhub {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    // 一次性解析全部参数，再从位置参数里取子命令。这样选项写在子命令前后都成立：
    // `signalhub --port 1 status` 与 `signalhub status --port 1` 等价。
    // 直接拿 args[0] 当子命令会漏掉前者。
    let (flags, positional) = Flags::parse(args)?;
    let (command, rest) = positional
        .split_first()
        .ok_or_else(|| Failure::other("缺少状态或子命令，用 --help 看用法"))?;

    match command.as_str() {
        "hook" => {
            let agent = rest.first().ok_or_else(|| {
                Failure::other("hook 需要智能体名，例如 signalhub hook claude-code")
            })?;
            hook(agent, &flags)
        }
        "status" => status(&flags),
        "watch" => watch(&flags),
        "clear" => clear(&flags),
        "monitor" => match rest.first().map(String::as_str) {
            Some("on") => monitor(true, &flags),
            Some("off") => monitor(false, &flags),
            other => Err(Failure::other(format!(
                "monitor 需要 on 或 off，收到 {other:?}"
            ))),
        },
        // signalhub on / off 是 monitor 的简写
        "on" => monitor(true, &flags),
        "off" => monitor(false, &flags),
        "install" => match rest.first().map(String::as_str) {
            Some("hooks") => install_hooks(required_agent(rest, "install hooks")?, &flags),
            other => Err(Failure::other(format!(
                "install 只支持 hooks，收到 {other:?}"
            ))),
        },
        "uninstall" => match rest.first().map(String::as_str) {
            Some("hooks") => uninstall_hooks(required_agent(rest, "uninstall hooks")?, &flags),
            other => Err(Failure::other(format!(
                "uninstall 只支持 hooks，收到 {other:?}"
            ))),
        },
        state => {
            if !rest.is_empty() {
                return Err(Failure::other(format!(
                    "多余的参数: {}",
                    rest.join(" ")
                )));
            }
            report(state, &flags)
        }
    }
}

// ---- HTTP ------------------------------------------------------------------

/// 连本机的明文请求，所以关掉 ureq 的默认 TLS 特性就够了。
fn client() -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        // 自己判状态码：默认行为会把 4xx/5xx 变成错误，那样就拿不到
        // 服务端返回的 {"error": "..."} 文本了
        .http_status_as_error(false)
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_per_call(Some(CALL_TIMEOUT))
        .build();
    ureq::Agent::new_with_config(config)
}

struct Endpoint {
    base: String,
    token: String,
}

impl Endpoint {
    fn discover(flags: &Flags) -> Result<Self, Failure> {
        let port = match flags.port {
            Some(port) => port,
            None => config::read_port().map_err(|err| match err.kind() {
                std::io::ErrorKind::NotFound => Failure::not_running(format!(
                    "{APP_NOT_RUNNING}（找不到 ~/.signalhub/port）"
                )),
                _ => Failure::other(format!("读取端口失败: {err}")),
            })?,
        };

        let token = config::read_token().map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound => Failure::not_running(format!(
                "{APP_NOT_RUNNING}（找不到 ~/.signalhub/token）"
            )),
            _ => Failure::other(format!("读取 token 失败: {err}")),
        })?;

        Ok(Self { base: format!("http://127.0.0.1:{port}/v1"), token })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    fn auth(&self) -> String {
        format!("Bearer {}", self.token)
    }
}

fn classify(err: ureq::Error) -> Failure {
    match err {
        ureq::Error::Io(ref io) if io.kind() == std::io::ErrorKind::ConnectionRefused => {
            Failure::not_running(APP_NOT_RUNNING)
        }
        ureq::Error::ConnectionFailed => Failure::not_running(APP_NOT_RUNNING),
        ureq::Error::Timeout(_) => {
            Failure::other("请求超时 —— app 可能在运行但卡住了")
        }
        other => Failure::other(format!("请求失败: {other}")),
    }
}

/// 读响应体并返回 (状态码, 文本)。
macro_rules! read_response {
    ($result:expr) => {{
        let mut response = $result.map_err(classify)?;
        let status = response.status().as_u16();
        let text = response
            .body_mut()
            .read_to_string()
            .map_err(|err| Failure::other(format!("读取响应失败: {err}")))?;
        (status, text)
    }};
}

/// 服务端非 2xx 时优先把它返回的 `{"error": ...}` 文本抛出来，
/// 只报一个状态码对排查毫无帮助。
fn expect_success(status: u16, body: &str) -> Result<Value, Failure> {
    let parsed = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    if (200..300).contains(&status) {
        return Ok(parsed);
    }
    let detail = parsed["error"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| body.trim().to_string());
    Err(Failure::other(format!("服务返回 {status}: {detail}")))
}

// ---- 子命令 ----------------------------------------------------------------

fn report(state: &str, flags: &Flags) -> Result<(), Failure> {
    let endpoint = Endpoint::discover(flags)?;

    let mut body = json!({ "state": state });
    if let Some(value) = &flags.session {
        body["session"] = json!(value);
    }
    if let Some(value) = &flags.agent {
        body["agent"] = json!(value);
    }
    if let Some(value) = &flags.cwd {
        body["cwd"] = json!(value);
    }
    if let Some(value) = &flags.message {
        body["message"] = json!(value);
    }
    if let Some(value) = flags.ttl_ms {
        body["ttl_ms"] = json!(value);
    }

    let (status, text) = read_response!(client()
        .post(&endpoint.url("/state"))
        .header("Authorization", &endpoint.auth())
        .header("Content-Type", "application/json")
        .send(body.to_string()));
    let parsed = expect_success(status, &text)?;

    println!(
        "{} → 聚合 {}（{} 个会话{})",
        state,
        parsed["aggregate"].as_str().unwrap_or("?"),
        parsed["sessions"].as_u64().unwrap_or(0),
        if parsed["changed"].as_bool().unwrap_or(false) { "" } else { "，无变化" }
    );
    Ok(())
}

fn clear(flags: &Flags) -> Result<(), Failure> {
    let endpoint = Endpoint::discover(flags)?;
    let session = flags.session.as_deref().unwrap_or("default");

    let (status, text) = read_response!(client()
        .delete(&endpoint.url(&format!("/sessions/{session}")))
        .header("Authorization", &endpoint.auth())
        .call());
    let parsed = expect_success(status, &text)?;

    println!(
        "会话 {session} {}（聚合 {}）",
        if parsed["removed"].as_bool().unwrap_or(false) { "已清除" } else { "本就不存在" },
        parsed["aggregate"].as_str().unwrap_or("?")
    );
    Ok(())
}

fn monitor(on: bool, flags: &Flags) -> Result<(), Failure> {
    let endpoint = Endpoint::discover(flags)?;

    let (status, text) = read_response!(client()
        .post(&endpoint.url("/monitor"))
        .header("Authorization", &endpoint.auth())
        .header("Content-Type", "application/json")
        .send(json!({ "paused": !on }).to_string()));
    let parsed = expect_success(status, &text)?;

    println!(
        "监控{}（聚合 {}）",
        if parsed["paused"].as_bool().unwrap_or(false) { "已暂停" } else { "已恢复" },
        if parsed["paused"].as_bool().unwrap_or(false) { "off" } else { "正常" }
    );
    Ok(())
}

/// `watch` 能等的状态。这是 CLI 里**唯一**硬编码状态名的地方。
///
/// 别处都刻意不校验状态名（权威定义在服务端）。这里破例是因为 `--until` 写错
/// 一个字母就会永远等下去，而没有任何端点可以问「有哪些状态」—— 用一个明确的
/// 报错换掉一个静默的无限等待，值得。注意这里只列名字、不含别名，
/// 因为要和 `GET /v1/state` 返回的 `aggregate` 做字面比较。
const WATCHABLE_STATES: [&str; 10] = [
    "idle", "thinking", "working", "done", "attention", "permission", "error", "off",
    "stale", "offline",
];

/// 跟随状态：状态变化时打印一行。
///
/// 两种用法：终端里开一个窗口盯着（Ctrl-C 退出），以及在 CI 里等某个状态
/// （`signalhub watch --until done --timeout 10m`）。
fn watch(flags: &Flags) -> Result<(), Failure> {
    let until = match &flags.until {
        Some(until) if WATCHABLE_STATES.contains(&until.as_str()) => Some(until.clone()),
        Some(until) => {
            return Err(Failure::other(format!(
                "--until {until:?} 不是已知状态；可用: {}",
                WATCHABLE_STATES.join(" ")
            )));
        }
        None => None,
    };

    let endpoint = Endpoint::discover(flags)?;
    let interval = Duration::from_millis(flags.interval_ms.unwrap_or(500));
    let deadline = flags
        .timeout_ms
        .map(|ms| Instant::now() + Duration::from_millis(ms));

    // 提示走 stderr，这样 `signalhub watch > 日志` 得到的是干净的状态时间线
    match &until {
        Some(until) => eprintln!("等待聚合变为 {until}…（Ctrl-C 取消）"),
        None => eprintln!("跟随状态变化（Ctrl-C 退出）"),
    }

    let mut last: Option<String> = None;
    loop {
        let (status, text) = read_response!(client()
            .get(&endpoint.url("/state"))
            .header("Authorization", &endpoint.auth())
            .call());
        let parsed = expect_success(status, &text)?;

        let aggregate = parsed["aggregate"].as_str().unwrap_or("?").to_string();
        let sessions = parsed["sessions"].as_array().map_or(0, Vec::len);

        if until.as_deref() == Some(aggregate.as_str()) {
            // 目标状态这一行也要打出来，否则 CI 日志里看不到到达了
            println!("{aggregate}  {sessions} 个会话");
            return Ok(());
        }

        // 只在变化时输出：每 500ms 刷一行会把日志淹掉
        if last.as_deref() != Some(aggregate.as_str()) {
            if flags.json {
                println!("{}", serde_json::to_string(&parsed).unwrap_or_default());
            } else {
                println!("{aggregate}  {sessions} 个会话");
            }
            last = Some(aggregate.clone());
        }

        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Err(Failure::other(format!(
                "等待超时：仍是 {aggregate}，未变成 {}",
                until.as_deref().unwrap_or("?")
            )));
        }

        std::thread::sleep(interval);
    }
}

fn status(flags: &Flags) -> Result<(), Failure> {
    let endpoint = Endpoint::discover(flags)?;

    let (status, text) = read_response!(client()
        .get(&endpoint.url("/state"))
        .header("Authorization", &endpoint.auth())
        .call());
    let parsed = expect_success(status, &text)?;

    if flags.json {
        println!("{}", serde_json::to_string_pretty(&parsed).unwrap_or(text));
        return Ok(());
    }

    println!(
        "聚合: {}",
        parsed["aggregate"].as_str().unwrap_or("?")
    );
    if parsed["paused"].as_bool().unwrap_or(false) {
        println!("监控: 已暂停");
    }
    if let Some(message) = parsed["message"].as_str() {
        println!("提示: {message}");
    }

    let empty = Vec::new();
    let sessions = parsed["sessions"].as_array().unwrap_or(&empty);
    println!("会话: {}", sessions.len());
    for session in sessions {
        println!(
            "  - {:<16} {:<11} {}{}{}",
            session["id"].as_str().unwrap_or("?"),
            session["state"].as_str().unwrap_or("?"),
            format_ms(session["age_ms"].as_u64().unwrap_or(0)),
            session["agent"]
                .as_str()
                .map(|agent| format!("  {agent}"))
                .unwrap_or_default(),
            session["message"]
                .as_str()
                .map(|message| format!("  「{message}」"))
                .unwrap_or_default(),
        );
    }
    Ok(())
}

fn format_ms(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.0}s", ms as f64 / 1_000.0)
    } else {
        format!("{:.0}m", ms as f64 / 60_000.0)
    }
}

// ---- hook 安装 / 卸载 ------------------------------------------------------

fn required_agent<'a>(rest: &'a [String], what: &str) -> Result<&'a str, Failure> {
    rest.get(1).map(String::as_str).ok_or_else(|| {
        Failure::other(format!("{what} 需要智能体名，例如 signalhub {what} claude-code"))
    })
}

fn install_hooks(agent: &str, flags: &Flags) -> Result<(), Failure> {
    let agent = hooks::Agent::parse(agent).map_err(Failure::other)?;
    let report = hooks::install(agent, flags.dry_run).map_err(Failure::other)?;

    println!(
        "{} 的 hook{}",
        agent.name(),
        if report.dry_run { "（演练，未写入任何文件）" } else { " 已安装" }
    );
    if let Some(command) = &report.command {
        println!("  命令  {command}");
    }
    println!("  配置  {}", report.path.display());
    if let Some(backup) = &report.backup {
        println!("  备份  {}", backup.display());
    }
    if !report.added.is_empty() {
        println!("  新增  {} 个事件", report.added.len());
        println!("        {}", report.added.join(" "));
    }
    if !report.updated.is_empty() {
        println!("  更新  {} 个事件（写入的命令变了）", report.updated.len());
        println!("        {}", report.updated.join(" "));
    }
    if !report.already.is_empty() {
        println!("  跳过  {} 个（已存在且命令相同）", report.already.len());
    }

    if report.added.is_empty() && report.updated.is_empty() {
        println!("无需改动。");
    } else if report.dry_run {
        println!("以上为演练结果；去掉 --dry-run 才会真正写入。");
    } else {
        println!("重启 {} 后生效。", agent.name());
    }

    // 没在 PATH 里就只能写绝对路径，此时二进制一挪动 hook 就失效——这种失效
    // 是静默的（智能体会话里什么都看不到），所以必须主动提醒。
    if !report.dry_run && !hooks::signalhub_on_path() {
        println!();
        println!("提示：signalhub 不在 PATH 中，已写入绝对路径。");
        println!("      若移动或删除了该二进制，hook 会静默失效。");
        println!("      把它放进 PATH（例如软链到 /usr/local/bin）后重新执行本命令即可改为裸名字。");
    }
    Ok(())
}

fn uninstall_hooks(agent: &str, flags: &Flags) -> Result<(), Failure> {
    let agent = hooks::Agent::parse(agent).map_err(Failure::other)?;
    let report = hooks::uninstall(agent, flags.dry_run).map_err(Failure::other)?;

    println!(
        "{} 的 hook{}",
        agent.name(),
        if report.dry_run { "（演练，未写入任何文件）" } else { " 已移除" }
    );
    println!("  配置  {}", report.path.display());
    if let Some(backup) = &report.backup {
        println!("  备份  {}", backup.display());
    }

    if report.removed_handlers == 0 {
        println!("没有找到本工具装过的 hook，无需改动。");
    } else {
        println!("  移除  {} 个 handler", report.removed_handlers);
        if !report.removed.is_empty() {
            println!("        {}", report.removed.join(" "));
        }
        if report.dry_run {
            println!("以上为演练结果；去掉 --dry-run 才会真正写入。");
        }
    }
    Ok(())
}

// ---- hook ------------------------------------------------------------------

/// hook 路径上**不允许**出现任何 `println!`。见文件头的约束 1。
fn hook(agent: &str, flags: &Flags) -> Result<(), Failure> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(|err| Failure::other(format!("读取 stdin 失败: {err}")))?;

    let payload: Value = match serde_json::from_str(&input) {
        Ok(payload) => payload,
        Err(err) => {
            log_hook(agent, "?", "-", &format!("stdin 不是合法 JSON: {err}"));
            return Err(Failure::other(format!("stdin 不是合法 JSON: {err}")));
        }
    };

    let event = payload["hook_event_name"].as_str().unwrap_or("?").to_string();
    let session = session_id(&payload).unwrap_or_else(|| agent.to_string());

    let Some(action) = map_event(agent, &payload) else {
        // 明确忽略的事件：一次 HTTP 都不发，但日志里要留痕 ——
        // 「某个事件其实一直在触发、只是被我们忽略了」正是需要能看见的信息
        log_hook(agent, &event, &session, "ignored");
        return Ok(());
    };

    let outcome = match &action {
        Action::Report(state) => (*state).to_string(),
        Action::Clear => "clear".to_string(),
    };

    // 端口/token 解析失败也要记进日志：那是失败里最常见的一种
    let result = match Endpoint::discover(flags) {
        Err(err) => Err(err),
        Ok(endpoint) => match action {
            Action::Report(state) => {
                let mut body = json!({ "state": state, "session": session, "agent": agent });
                if let Some(cwd) = cwd_of(&payload) {
                    body["cwd"] = json!(cwd);
                }
                let (status, text) = read_response!(client()
                    .post(&endpoint.url("/state"))
                    .header("Authorization", &endpoint.auth())
                    .header("Content-Type", "application/json")
                    .send(body.to_string()));
                expect_success(status, &text).map(|_| ())
            }
            Action::Clear => {
                let (status, text) = read_response!(client()
                    .delete(&endpoint.url(&format!("/sessions/{session}")))
                    .header("Authorization", &endpoint.auth())
                    .call());
                expect_success(status, &text).map(|_| ())
            }
        },
    };

    match &result {
        Ok(()) => log_hook(agent, &event, &session, &outcome),
        Err(err) => log_hook(agent, &event, &session, &format!("失败: {}", err.message)),
    }
    result
}

/// 记一行 hook 调用。
///
/// **只写日志文件，绝不碰 stdout**（约束 1）。任何失败都忽略：为了记日志而
/// 弄坏智能体的 hook 是本末倒置。
fn log_hook(agent: &str, event: &str, session: &str, outcome: &str) {
    let Ok(path) = config::hook_log_path() else {
        return;
    };

    // 单代轮转。每次工具调用一行，不设上限的话会无界增长。
    const MAX_BYTES: u64 = 512 * 1024;
    if fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0) > MAX_BYTES {
        let _ = fs::rename(&path, path.with_extension("log.1"));
    }

    let line = format!(
        "{}  {agent:<12} {event:<20} session={session:<38} {outcome}\n",
        utc_hms()
    );
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = file.write_all(line.as_bytes());
    }
}

/// 日志时间戳。用 UTC 并在值上标 `Z`：不引入时区数据依赖，且不会含糊。
fn utc_hms() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        % 86_400;
    format!("{:02}:{:02}:{:02}Z", secs / 3600, (secs % 3600) / 60, secs % 60)
}

enum Action {
    Report(&'static str),
    Clear,
}

fn map_event(agent: &str, payload: &Value) -> Option<Action> {
    let event = payload["hook_event_name"].as_str()?;
    match agent {
        "claude-code" => map_claude(event, payload),
        "codex" => map_codex(event),
        "cursor" => map_cursor(event, payload),
        _ => None,
    }
}

/// 映射表见 docs/api.md §4.2，含**明确忽略**的事件清单 ——
/// TaskCreated / FileChanged 这类事件频繁且与「一轮工作」无关，误用会让灯球乱闪。
fn map_claude(event: &str, payload: &Value) -> Option<Action> {
    Some(match event {
        "SessionStart" => {
            // 只有 startup / resume 才该报 idle。compact / fork / clear 是会话中途
            // 发生的，此时智能体仍在工作，报 idle 是错的。
            // 字段名在不同版本上可能不同，所以只排除明确知道不该报的那几个，
            // 其余一律按 idle 处理 —— 宁可多报也不要在版本差异下静默丢事件。
            match payload["source"].as_str() {
                Some("compact" | "fork" | "clear") => return None,
                _ => Action::Report("idle"),
            }
        }
        "UserPromptSubmit" | "SubagentStart" | "PreCompact" => Action::Report("thinking"),
        "PreToolUse" | "PostToolUse" | "PermissionDenied" | "PostCompact" => {
            Action::Report("working")
        }
        "PostToolUseFailure" => Action::Report("attention"),
        "PermissionRequest" => Action::Report("permission"),
        "Notification" => match payload["notification_type"].as_str().unwrap_or("") {
            "permission_prompt" => Action::Report("permission"),
            "agent_needs_input" | "idle_prompt" => Action::Report("attention"),
            "agent_completed" => Action::Report("done"),
            _ => return None,
        },
        "Stop" => Action::Report("done"),
        "StopFailure" => Action::Report("error"),
        "TeammateIdle" => Action::Report("attention"),
        "SessionEnd" => Action::Clear,
        _ => return None,
    })
}

/// Codex 用 PascalCase，事件集与 Claude Code 同构（docs/api.md §4.6）。
fn map_codex(event: &str) -> Option<Action> {
    Some(match event {
        "SessionStart" => Action::Report("idle"),
        "UserPromptSubmit" | "SubagentStart" | "PreCompact" => Action::Report("thinking"),
        "PreToolUse" | "PostToolUse" | "PostCompact" => Action::Report("working"),
        "PermissionRequest" => Action::Report("permission"),
        "Stop" => Action::Report("done"),
        "Interrupt" | "SessionEnd" => Action::Clear,
        _ => return None,
    })
}

/// Cursor 用 camelCase（docs/api.md §4.7）。
/// 注意它没有可映射的「需要授权」事件 —— 那要靠 hook 返回 permission 字段实现，
/// 上报型 hook 不返回该字段，所以这里确实没有对应项，不是漏了。
fn map_cursor(event: &str, payload: &Value) -> Option<Action> {
    Some(match event {
        "sessionStart" => Action::Report("idle"),
        "beforeSubmitPrompt" | "afterAgentThought" | "subagentStart" | "preCompact" => {
            Action::Report("thinking")
        }
        "preToolUse" | "postToolUse" | "beforeShellExecution" | "afterShellExecution" => {
            Action::Report("working")
        }
        "postToolUseFailure" => Action::Report("attention"),
        "afterAgentResponse" => Action::Report("done"),
        // 三家里只有 Cursor 的结束事件带 status，能直接区分正常结束和异常中断
        "stop" => match payload["status"].as_str() {
            Some("error" | "aborted") => Action::Report("error"),
            _ => Action::Report("done"),
        },
        "sessionEnd" => Action::Clear,
        _ => return None,
    })
}

/// 三家用的字段名不同，做宽容匹配。Codex 老式 `notify` 用 kebab 的 `thread-id`。
fn session_id(payload: &Value) -> Option<String> {
    ["session_id", "conversation_id", "thread_id", "thread-id"]
        .iter()
        .find_map(|key| payload[*key].as_str())
        .map(str::to_string)
}

/// Claude Code / Codex 用 `cwd`；Cursor 给的是 `workspace_roots` 数组。
fn cwd_of(payload: &Value) -> Option<String> {
    payload["cwd"]
        .as_str()
        .or_else(|| payload["workspace_roots"].as_array()?.first()?.as_str())
        .map(str::to_string)
}

fn help() -> String {
    format!(
        "\
signalhub {version} —— 桌面悬浮状态灯的上报客户端

用法:
  signalhub <状态> [选项]            上报状态
  signalhub hook <智能体>            从 stdin 读智能体 hook 的 JSON 并上报
  signalhub status [--json]          查询当前聚合状态
  signalhub watch [选项]             跟随状态变化（终端里盯着，或 CI 里等状态）
  signalhub clear --session <id>     清除某个会话
  signalhub monitor on|off           全局暂停 / 恢复监控
  signalhub on | off                 monitor 的简写
  signalhub install hooks <智能体>   把 hook 写进智能体的配置文件
  signalhub uninstall hooks <智能体> 移除本工具写进去的 hook

状态:
  idle thinking working done attention permission error off
  off 作用在会话上等价于清除该会话；全局暂停请用 monitor off。
  状态名接受别名，如 blocked→error、completed→done、permission_request→permission。

选项:
  --session <id>    会话标识，默认 default
  --agent <name>    上报方名字，用于 UI 归因
  --cwd <path>      项目路径
  --message <text>  悬停提示文本
  --ttl <时长>      覆盖默认过期时间，如 30s / 5m / 1h / 90（裸数字按秒）
  --json            status 输出原始 JSON
  --dry-run         install / uninstall 只打印将要做的改动，不写文件
  --interval <时长>  watch 的轮询间隔，默认 500ms
  --until <状态>     watch 等到聚合变成该状态就退出 0（如 --until done）
  --timeout <时长>   watch 的等待上限，超时退出 1
  --port <n>        覆盖端口自动发现
  --never-fail      任何错误都以退出码 0 结束（hook 模式隐含此行为）
  -h, --help        显示本帮助
  -V, --version     显示版本

退出码:
  0  成功
  1  参数错误或服务端返回错误
  3  SignalHub 没有在运行

智能体: claude-code codex cursor
  install hooks 会改动它们的配置文件，写之前自动备份，且只动 hooks 这一个键。
",
        version = env!("CARGO_PKG_VERSION")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(name: &str) -> Value {
        json!({ "hook_event_name": name, "session_id": "s-1", "cwd": "/proj" })
    }

    fn state_of(agent: &str, payload: &Value) -> Option<&'static str> {
        match map_event(agent, payload)? {
            Action::Report(state) => Some(state),
            Action::Clear => None,
        }
    }

    // ---- Claude Code ----

    #[test]
    fn claude_code_maps_the_turn_lifecycle() {
        assert_eq!(state_of("claude-code", &event("UserPromptSubmit")), Some("thinking"));
        assert_eq!(state_of("claude-code", &event("PreToolUse")), Some("working"));
        assert_eq!(state_of("claude-code", &event("PostToolUse")), Some("working"));
        assert_eq!(state_of("claude-code", &event("Stop")), Some("done"));
    }

    #[test]
    fn claude_code_maps_permission_and_failure() {
        assert_eq!(
            state_of("claude-code", &event("PermissionRequest")),
            Some("permission")
        );
        assert_eq!(
            state_of("claude-code", &event("StopFailure")),
            Some("error")
        );
        assert_eq!(
            state_of("claude-code", &event("PostToolUseFailure")),
            Some("attention")
        );
        // 权限被拒说明权限点已解决，红灯该灭
        assert_eq!(
            state_of("claude-code", &event("PermissionDenied")),
            Some("working")
        );
    }

    #[test]
    fn claude_code_distinguishes_notification_types() {
        let with_type = |kind: &str| {
            json!({ "hook_event_name": "Notification", "notification_type": kind })
        };
        assert_eq!(
            state_of("claude-code", &with_type("permission_prompt")),
            Some("permission")
        );
        assert_eq!(
            state_of("claude-code", &with_type("agent_needs_input")),
            Some("attention")
        );
        assert_eq!(
            state_of("claude-code", &with_type("agent_completed")),
            Some("done")
        );
        // 认不出的通知类型：宁可不报，也不要瞎报
        assert_eq!(state_of("claude-code", &with_type("auth_success")), None);
    }

    #[test]
    fn claude_code_session_start_only_reports_for_real_starts() {
        let with_source = |source: &str| {
            json!({ "hook_event_name": "SessionStart", "source": source })
        };
        assert_eq!(state_of("claude-code", &with_source("startup")), Some("idle"));
        assert_eq!(state_of("claude-code", &with_source("resume")), Some("idle"));

        // 压缩上下文 / fork 时智能体还在干活，报 idle 是错的
        assert_eq!(state_of("claude-code", &with_source("compact")), None);
        assert_eq!(state_of("claude-code", &with_source("fork")), None);

        // 字段名在版本间可能有出入：不认识就按 idle 处理，不要静默丢事件
        assert_eq!(
            state_of("claude-code", &json!({ "hook_event_name": "SessionStart" })),
            Some("idle")
        );
    }

    #[test]
    fn claude_code_session_end_clears() {
        assert!(matches!(
            map_event("claude-code", &event("SessionEnd")),
            Some(Action::Clear)
        ));
    }

    /// 实测踩到的 bug：`Stop` 之后约 9 秒会跟来一个 `SubagentStop`。
    /// 把它映射成 working 会把刚设好的 done 覆盖掉 —— 于是轮次明明结束了，
    /// 灯却继续闪绿直到 TTL 到期。三家都有这个事件，所以三家都忽略它：
    /// 主智能体还在干活时，PreToolUse / PostToolUse 本来就会持续报 working，
    /// 丢掉这个事件几乎没有信息损失。
    #[test]
    fn subagent_stop_is_ignored_so_it_cannot_clobber_done() {
        for (agent, subagent_event, stop_event) in [
            ("claude-code", "SubagentStop", "Stop"),
            ("codex", "SubagentStop", "Stop"),
            ("cursor", "subagentStop", "stop"),
        ] {
            assert_eq!(
                state_of(agent, &event(subagent_event)),
                None,
                "{agent} 的 {subagent_event} 应被忽略"
            );
            // 而轮次结束本身仍要正常报 done
            assert_eq!(
                state_of(agent, &event(stop_event)),
                Some("done"),
                "{agent} 的 {stop_event} 仍应报 done"
            );
        }
    }

    /// 这些事件频繁且与「一轮工作」无关，误报会让灯球乱闪
    #[test]
    fn claude_code_ignores_the_chatty_events() {
        for name in [
            "FileChanged",
            "CwdChanged",
            "TaskCreated",
            "TaskCompleted",
            "InstructionsLoaded",
            "MessageDisplay",
            "PostToolBatch",
            "ConfigChange",
            "WorktreeCreate",
            "Elicitation",
        ] {
            assert_eq!(state_of("claude-code", &event(name)), None, "{name} 应被忽略");
        }
    }

    #[test]
    fn claude_code_ignores_unknown_events_and_missing_event_name() {
        assert_eq!(state_of("claude-code", &event("SomethingBrandNew")), None);
        assert_eq!(state_of("claude-code", &json!({ "session_id": "s" })), None);
    }

    // ---- Codex / Cursor ----

    #[test]
    fn codex_maps_its_event_set() {
        assert_eq!(state_of("codex", &event("PreToolUse")), Some("working"));
        assert_eq!(state_of("codex", &event("PermissionRequest")), Some("permission"));
        assert_eq!(state_of("codex", &event("Stop")), Some("done"));
        assert!(matches!(map_event("codex", &event("Interrupt")), Some(Action::Clear)));
        assert!(matches!(map_event("codex", &event("SessionEnd")), Some(Action::Clear)));
    }

    #[test]
    fn cursor_maps_its_camel_case_event_set() {
        assert_eq!(state_of("cursor", &event("preToolUse")), Some("working"));
        assert_eq!(state_of("cursor", &event("beforeSubmitPrompt")), Some("thinking"));
        assert_eq!(state_of("cursor", &event("afterAgentResponse")), Some("done"));
        assert!(matches!(
            map_event("cursor", &event("sessionEnd")),
            Some(Action::Clear)
        ));
    }

    /// 三家里只有 Cursor 的结束事件带 status
    #[test]
    fn cursor_stop_uses_status_to_distinguish_failure() {
        let stop = |status: &str| json!({ "hook_event_name": "stop", "status": status });
        assert_eq!(state_of("cursor", &stop("completed")), Some("done"));
        assert_eq!(state_of("cursor", &stop("error")), Some("error"));
        assert_eq!(state_of("cursor", &stop("aborted")), Some("error"));
    }

    #[test]
    fn an_unknown_agent_maps_nothing() {
        assert!(map_event("some-other-tool", &event("PreToolUse")).is_none());
    }

    // ---- 字段提取 ----

    #[test]
    fn session_id_is_matched_leniently_across_agents() {
        assert_eq!(session_id(&json!({ "session_id": "a" })).as_deref(), Some("a"));
        // Cursor
        assert_eq!(
            session_id(&json!({ "conversation_id": "b" })).as_deref(),
            Some("b")
        );
        // Codex 老式 notify 用 kebab-case
        assert_eq!(session_id(&json!({ "thread-id": "c" })).as_deref(), Some("c"));
        assert_eq!(session_id(&json!({ "thread_id": "d" })).as_deref(), Some("d"));
        assert_eq!(session_id(&json!({})), None);
    }

    #[test]
    fn cwd_falls_back_to_workspace_roots() {
        assert_eq!(cwd_of(&json!({ "cwd": "/a" })).as_deref(), Some("/a"));
        assert_eq!(
            cwd_of(&json!({ "workspace_roots": ["/b", "/c"] })).as_deref(),
            Some("/b")
        );
        assert_eq!(cwd_of(&json!({ "workspace_roots": [] })), None);
        assert_eq!(cwd_of(&json!({})), None);
    }

    // ---- 选项解析 ----

    fn parse(args: &[&str]) -> Result<(Flags, Vec<String>), Failure> {
        let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        Flags::parse(&owned)
    }

    #[test]
    fn flags_are_parsed_with_their_values() {
        let (flags, positional) = parse(&[
            "--session",
            "build-1",
            "--agent",
            "ci",
            "--message",
            "compiling",
            "--json",
        ])
        .unwrap();
        assert_eq!(flags.session.as_deref(), Some("build-1"));
        assert_eq!(flags.agent.as_deref(), Some("ci"));
        assert_eq!(flags.message.as_deref(), Some("compiling"));
        assert!(flags.json);
        assert!(positional.is_empty());
    }

    #[test]
    fn positionals_are_collected_separately_from_flags() {
        let (flags, positional) = parse(&["on", "--port", "9999"]).unwrap();
        assert_eq!(positional, vec!["on".to_string()]);
        assert_eq!(flags.port, Some(9999));
    }

    /// 选项写在子命令之前也该成立 —— 把 args[0] 当子命令就会漏掉这种写法
    #[test]
    fn flags_may_come_before_the_command() {
        let (flags, positional) = parse(&["--port", "1", "status"]).unwrap();
        assert_eq!(flags.port, Some(1));
        assert_eq!(positional, vec!["status".to_string()]);
    }

    #[test]
    fn a_flag_missing_its_value_is_an_error() {
        assert!(parse(&["--session"]).is_err());
    }

    #[test]
    fn an_unknown_flag_is_an_error() {
        let err = parse(&["--nope"]).unwrap_err();
        assert!(err.message.contains("未知选项"), "{}", err.message);
    }

    #[test]
    fn a_bad_port_is_an_error() {
        assert!(parse(&["--port", "abc"]).is_err());
    }

    #[test]
    fn watch_flags_are_parsed() {
        let (flags, _) = parse(&["--until", "done", "--timeout", "5m", "--interval", "250ms"])
            .unwrap();
        assert_eq!(flags.until.as_deref(), Some("done"));
        assert_eq!(flags.timeout_ms, Some(300_000));
        assert_eq!(flags.interval_ms, Some(250));
    }

    /// 写错一个字母就会永远等下去，所以这里必须报错而不是静默等待。
    /// 校验发生在连接服务之前，所以不需要有 app 在跑。
    #[test]
    fn watch_rejects_an_unknown_until_state() {
        let (flags, _) = parse(&["--until", "workng"]).unwrap();
        let err = watch(&flags).unwrap_err();
        assert!(err.message.contains("不是已知状态"), "{}", err.message);
        assert!(
            err.message.contains("working"),
            "错误里该列出可用状态: {}",
            err.message
        );
    }

    #[test]
    fn watch_accepts_every_aggregate_state_including_derived_ones() {
        // 派生状态（stale / offline）也能等 —— 它们只不能「上报」
        for state in WATCHABLE_STATES {
            assert!(
                WATCHABLE_STATES.contains(&state),
                "{state} 应在可等列表里"
            );
        }
        assert!(WATCHABLE_STATES.contains(&"stale"));
        assert!(WATCHABLE_STATES.contains(&"offline"));
    }

    #[test]
    fn ttl_accepts_suffixes_and_bare_seconds() {
        assert_eq!(parse_ttl("250ms").unwrap(), 250);
        assert_eq!(parse_ttl("30s").unwrap(), 30_000);
        assert_eq!(parse_ttl("5m").unwrap(), 300_000);
        assert_eq!(parse_ttl("1h").unwrap(), 3_600_000);
        // 裸数字按秒，和 --ttl 30 的直觉一致
        assert_eq!(parse_ttl("90").unwrap(), 90_000);
        assert_eq!(parse_ttl(" 30s ").unwrap(), 30_000);
    }

    #[test]
    fn ttl_rejects_nonsense() {
        assert!(parse_ttl("abc").is_err());
        assert!(parse_ttl("").is_err());
        assert!(parse_ttl("99999999999999999999h").is_err(), "应报溢出而不是 panic");
    }

    // ---- 错误处理 ----

    #[test]
    fn server_errors_surface_the_documented_error_text() {
        let err = expect_success(400, r#"{"error":"state 非法"}"#).unwrap_err();
        assert!(err.message.contains("400"), "{}", err.message);
        assert!(err.message.contains("state 非法"), "{}", err.message);
        assert!(!err.not_running);
    }

    #[test]
    fn a_2xx_response_is_success() {
        assert!(expect_success(200, r#"{"ok":true}"#).is_ok());
    }

    #[test]
    fn a_non_json_error_body_still_reports_something() {
        let err = expect_success(500, "<html>oops</html>").unwrap_err();
        assert!(err.message.contains("oops"), "{}", err.message);
    }

    #[test]
    fn exit_codes_distinguish_not_running_from_other_failures() {
        assert_eq!(Failure::not_running("x").code(), EXIT_NOT_RUNNING);
        assert_eq!(Failure::other("x").code(), EXIT_ERROR);
        assert_eq!(Failure::not_running("x").code(), 3);
    }

    #[test]
    fn help_documents_every_exit_code_and_command() {
        let text = help();
        for expected in [
            "hook", "status", "watch", "clear", "monitor", "install", "uninstall",
            "claude-code", "codex", "cursor", "--never-fail", "--ttl", "--dry-run",
            "--until", "--timeout", "--interval", "退出码",
        ] {
            assert!(text.contains(expected), "帮助文本缺少 {expected}");
        }
    }

    #[test]
    fn format_ms_is_readable() {
        assert_eq!(format_ms(400), "400ms");
        assert_eq!(format_ms(12_000), "12s");
        assert_eq!(format_ms(300_000), "5m");
    }
}
