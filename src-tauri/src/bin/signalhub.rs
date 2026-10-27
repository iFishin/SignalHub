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
use std::io::Read;
use std::process::{Command, ExitCode};
use std::sync::mpsc;
use std::time::{Duration, Instant};

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

// spool 是**写入方**在 CLI、**读取方**在 app 的共享文件（见 spool.rs 顶部）：
// hook 连不上 app 时把这一条攒进去，app 下次启动重放。
#[allow(dead_code)]
#[path = "../spool.rs"]
mod spool;

// 每个信号源「最近一次上报」的凭证。app 侧由上报路径写、接入页读；CLI 侧的
// `diagnose` 只读它 —— 同一份文件、同一份解析代码，不另写一遍。
#[allow(dead_code)]
#[path = "../receipts.rs"]
mod receipts;

// 一键诊断的正文。**CLI 和 app 共用这一份**：接不上的人里，有人只开了终端；
// 而两处各写一遍的话，用户贴出来的那段迟早和界面上显示的对不上。
#[allow(dead_code)]
#[path = "../diagnose.rs"]
mod diagnose;

// hook payload → 状态的映射表，和 app 侧的 HTTP 接入共用同一份（见 hookmap.rs 顶部）。
#[allow(dead_code)]
#[path = "../hookmap.rs"]
mod hookmap;

use hookmap::{
    clip, cwd_of, event_name, map_event, session_id, tool_target, tool_verb, Action, Verb,
    TOOL_NAME_MAX,
};

// CLI 自己的文案（中英各一份）。**故意不挂进 `lib.rs` 的模块树**：只有这个二进制
// 用它，挂到 app 那侧换来的是一片 dead_code（而 CI 是 `-D warnings`）。
#[path = "../cli_text.rs"]
mod cli_text;

use cli_text::{fill, text as cli_text_for, CliText};
use config::Lang;

/// 这次调用该说哪种语言，**只解析一次**。
///
/// 懒解析 + `OnceLock` 而不是每次现算：`hook` 那条路径上智能体每轮工具调用都会走
/// 一遍，而读 `config.json` 是有代价的 —— 这个值在一次调用里又不会变。
fn t() -> &'static CliText {
    static LANG: std::sync::OnceLock<Lang> = std::sync::OnceLock::new();
    cli_text_for(*LANG.get_or_init(|| config::resolve_lang(&config::load_settings())))
}

const CONNECT_TIMEOUT: Duration = Duration::from_millis(800);
const CALL_TIMEOUT: Duration = Duration::from_millis(1500);

const EXIT_OK: u8 = 0;
const EXIT_ERROR: u8 = 1;
const EXIT_NOT_RUNNING: u8 = 3;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // hook 模式天然 never-fail。从原始参数判断，这样即使参数是坏的也成立。
    let never_fail = args.first().is_some_and(|arg| arg == "hook")
        || args.iter().any(|arg| arg == "--never-fail");

    let code = match run(&args) {
        Ok(()) => EXIT_OK,
        Err(failure) => {
            // 空 message = 这是 `run` 传出来的被包命令退出码，不是我们的错误 ——
            // 被包命令自己的输出已经说明了一切，不该再补一句「signalhub: 」
            if !failure.message.is_empty() {
                eprintln!("signalhub: {}", failure.message);
            }
            failure.code()
        }
    };

    ExitCode::from(if never_fail { EXIT_OK } else { code })
}

#[derive(Debug)]
struct Failure {
    message: String,
    not_running: bool,
    /// 直接给定的退出码，**跳过 message / not_running 那套映射**。
    ///
    /// `run` 用它把被包命令的退出码原样传出去。这时 `message` 是空的：那不是
    /// 「signalhub 出错了」，而是「被包的命令失败了」—— 它自己的输出已经说明了一切，
    /// 我们再补一句只会让人以为是包装器的问题。
    passthrough: Option<u8>,
}

impl Failure {
    fn other(message: impl Into<String>) -> Self {
        Self { message: message.into(), not_running: false, passthrough: None }
    }

    fn not_running(message: impl Into<String>) -> Self {
        Self { message: message.into(), not_running: true, passthrough: None }
    }

    /// 把退出码原样传出去（见字段注释）。
    fn passthrough(code: u8) -> Self {
        Self { message: String::new(), not_running: false, passthrough: Some(code) }
    }

    fn code(&self) -> u8 {
        if let Some(code) = self.passthrough {
            return code;
        }
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
    /// 声明的信号源类型（`agent` / `system` / `custom`）。不传就按 `--agent` 推
    /// （见 `inferred_source`），hook / run 另有自己的缺省。
    source: Option<String>,
    cwd: Option<String>,
    /// 归属的组。不传就是 `default` —— 单组用户完全不用管这个字段。
    group: Option<String>,
    message: Option<String>,
    ttl_ms: Option<u64>,
    json: bool,
    dry_run: bool,
    /// `install hooks`：用 **HTTP 方式**接入（不需要命令，目前只有 Claude Code）。
    http: bool,
    /// `install hooks --approve`：装「从灯上审批」（C3）。**隐含 `--http`** ——
    /// 审批就是 HTTP hook 回一个决定，没有别的形态。
    approve: bool,
    port: Option<u16>,
    /// watch 专用
    interval_ms: Option<u64>,
    until: Option<String>,
    timeout_ms: Option<u64>,
    /// `--` 之后**原样收下**的命令（`run` 用）。
    ///
    /// 单独存一个字段而不是塞进 positional：那样 `signalhub run foo -- bar` 里
    /// 的 `foo` 会和被包的命令混在一起，分不出哪个是哪个。`--` 的语义就是
    /// 「这之后不是我的参数了」，那就该有个地方把它原封不动放着。
    command: Vec<String>,
}

fn take_value(args: &[String], index: &mut usize, name: &str) -> Result<String, Failure> {
    *index += 1;
    args.get(*index)
        .cloned()
        .ok_or_else(|| Failure::other(fill(t().missing_value, &[name])))
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
                "--source" => {
                    let raw = take_value(args, &mut index, "--source")?;
                    flags.source = Some(parse_source(&raw)?);
                }
                "--cwd" => flags.cwd = Some(take_value(args, &mut index, "--cwd")?),
                "--group" => flags.group = Some(take_value(args, &mut index, "--group")?),
                "--message" => flags.message = Some(take_value(args, &mut index, "--message")?),
                "--ttl" => {
                    let raw = take_value(args, &mut index, "--ttl")?;
                    flags.ttl_ms = Some(parse_ttl(&raw)?);
                }
                "--port" => {
                    let raw = take_value(args, &mut index, "--port")?;
                    flags.port = Some(raw.parse().map_err(|_| {
                        Failure::other(fill(t().port_not_number, &[&format!("{raw:?}")]))
                    })?);
                }
                "--json" => flags.json = true,
                "--dry-run" => flags.dry_run = true,
                "--http" => flags.http = true,
                "--approve" => {
                    flags.approve = true;
                    flags.http = true;
                }
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
                // `--` 之后**原样收下、不再解析** —— `run` 用它把命令和我们的选项
                // 分开。**这条必须在下面那个兜底分支之前**：`--` 自己也以 `--` 开头，
                // 落到兜底里会被当成「未知选项 --」。
                "--" => {
                    flags.command = args[index + 1..].to_vec();
                    break;
                }
                other if other.starts_with("--") => {
                    return Err(Failure::other(fill(t().unknown_option, &[other])));
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
        .map_err(|_| Failure::other(fill(t().bad_duration, &[&format!("{raw:?}")])))?;
    value
        .checked_mul(per_unit)
        .ok_or_else(|| Failure::other(fill(t().duration_too_big, &[&format!("{raw:?}")])))
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
        .ok_or_else(|| Failure::other(t().no_command))?;

    match command.as_str() {
        "hook" => {
            let agent = rest.first().ok_or_else(|| {
                Failure::other(t().hook_needs_agent)
            })?;
            hook(agent, &flags)
        }
        "status" => status(&flags),
        "watch" => watch(&flags),
        "run" => run_command(&flags),
        "clear" => clear(&flags),
        "monitor" => match rest.first().map(String::as_str) {
            Some("on") => monitor(true, &flags),
            Some("off") => monitor(false, &flags),
            // 没给值时别把 `None` 打给用户看 —— 那是 Rust 的 Debug 输出
            other => Err(Failure::other(fill(
                t().monitor_needs_on_off,
                &[other.unwrap_or(t().nothing)],
            ))),
        },
        // signalhub on / off 是 monitor 的简写
        "on" => monitor(true, &flags),
        "off" => monitor(false, &flags),
        "install" => match rest.first().map(String::as_str) {
            Some("hooks") => install_hooks(required_agent(rest, "install hooks")?, &flags),
            other => Err(Failure::other(fill(
                t().only_hooks,
                &["install", other.unwrap_or(t().nothing)],
            ))),
        },
        "uninstall" => match rest.first().map(String::as_str) {
            Some("hooks") => uninstall_hooks(required_agent(rest, "uninstall hooks")?, &flags),
            other => Err(Failure::other(fill(
                t().only_hooks,
                &["uninstall", other.unwrap_or(t().nothing)],
            ))),
        },
        // `rest` 是**子命令之后**的位置参数，所以 `diagnose opencode` 里智能体在
        // `rest[0]`（不像 `install hooks <agent>` 那样在第 1 位）
        "diagnose" => run_diagnose(rest.first().map(String::as_str)),
        state => {
            if !rest.is_empty() {
                return Err(Failure::other(fill(t().too_many_args, &[&rest.join(" ")])));
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
                std::io::ErrorKind::NotFound => Failure::not_running(fill(t().no_port_file, &[t().app_not_running])),
                _ => Failure::other(fill(t().read_port_failed, &[&err.to_string()])),
            })?,
        };

        let token = config::read_token().map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound => {
                Failure::not_running(fill(t().no_token_file, &[t().app_not_running]))
            }
            _ => Failure::other(fill(t().read_token_failed, &[&err.to_string()])),
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
            Failure::not_running(t().app_not_running)
        }
        ureq::Error::ConnectionFailed => Failure::not_running(t().app_not_running),
        ureq::Error::Timeout(_) => {
            Failure::other(t().request_timeout)
        }
        other => Failure::other(fill(t().request_failed, &[&other.to_string()])),
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
            .map_err(|err| Failure::other(fill(t().read_response_failed, &[&err.to_string()])))?;
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
    Err(Failure::other(fill(t().server_error, &[&status.to_string(), &detail])))
}

// ---- 子命令 ----------------------------------------------------------------

/// 把命令行上**真的给了值**的选项补进上报体。`report` 和 `hook` 共用。
///
/// 键名必须与 `state.rs` 的 `Report` 字段逐字一致：服务端**没有**
/// `deny_unknown_fields`，拼错一个键不会报错、只会被静默丢掉。多分组此前正是
/// 栽在这上面 —— `Report.group` 一直在、服务端也一直认，CLI 侧却从来没填过
/// 这个字段，于是所有 hook 和 CLI 用户报的状态全落进 `default`，
/// README 当招牌的多分组只剩手写 curl 能用。
fn apply_flag_extras(body: &mut Value, flags: &Flags) {
    if let Some(value) = &flags.session {
        body["session"] = json!(value);
    }
    if let Some(value) = &flags.agent {
        body["agent"] = json!(value);
    }
    if let Some(value) = &flags.source {
        body["source"] = json!(value);
    }
    if let Some(value) = &flags.cwd {
        body["cwd"] = json!(value);
    }
    if let Some(value) = &flags.group {
        body["group"] = json!(value);
    }
    if let Some(value) = &flags.message {
        body["message"] = json!(value);
    }
    if let Some(value) = flags.ttl_ms {
        body["ttl_ms"] = json!(value);
    }
}

/// 把上报方进程号补进请求体（有就补）。
///
/// **为什么走环境变量、不走命令行选项**：装进别人配置里的命令可能由**旧版本**
/// 的 CLI 执行（用户把 CLI 拷贝到 PATH 而没有跟着应用一起升级）。旧 CLI 不认识
/// 新选项，会把整条 hook 当成错误 —— 上报直接断掉；而环境变量它根本不看，最坏
/// 也只是退回「沉默」那条老路。插件式接入同理（见 `hooks.rs` 的模板）。
fn apply_env_pid(body: &mut Value) {
    if let Some(pid) = env_pid() {
        body["pid"] = json!(pid);
    }
}

fn env_pid() -> Option<u32> {
    parse_pid(&std::env::var("SIGNALHUB_PID").ok()?)
}

/// 只认正整数。没被 shell 展开的 `$PPID`、0、空串都算「没给」。
fn parse_pid(raw: &str) -> Option<u32> {
    raw.trim().parse().ok().filter(|pid: &u32| *pid > 0)
}

/// `--source` 只认三个值。
///
/// **在这里就报错**，而不是留给服务端：服务端不做校验（认不出来的类别会被界面归到
/// 「自定义」），打错一个字不会有任何提示 —— 那正是「我明明声明了 system，怎么跑到
/// 自定义里去了」的来源。
fn parse_source(raw: &str) -> Result<String, Failure> {
    let value = raw.trim().to_ascii_lowercase();
    match value.as_str() {
        "agent" | "system" | "custom" => Ok(value),
        other => Err(Failure::other(fill(t().unknown_source, &[&format!("{other:?}")]))),
    }
}

/// 没声明 `--source` 时按名字推。**这是推断，所以它只是缺省值**：`--source` 永远优先。
///
/// 推的是「这个名字是不是我们那 9 家」：是就按智能体算，否则按自定义算。手写
/// `signalhub report` 的两种常见情形（给某家没有一键接入的智能体补一条上报 / 自己的
/// 脚本报进度）正好被这条规则分开，而分错的后果只是「接入」页里那一行落在别的组，
/// 补一句 `--source` 就能改。
fn inferred_source(agent: Option<&str>) -> &'static str {
    match agent.map(|name| hooks::Agent::parse(name).is_ok()) {
        Some(true) => "agent",
        _ => "custom",
    }
}

fn report(state: &str, flags: &Flags) -> Result<(), Failure> {
    let endpoint = Endpoint::discover(flags)?;

    let mut body = json!({ "state": state });
    apply_flag_extras(&mut body, flags);
    if body.get("source").is_none() {
        body["source"] = json!(inferred_source(flags.agent.as_deref()));
    }
    apply_env_pid(&mut body);

    let (status, text) = read_response!(client()
        .post(&endpoint.url("/state"))
        .header("Authorization", &endpoint.auth())
        .header("Content-Type", "application/json")
        .send(body.to_string()));
    let parsed = expect_success(status, &text)?;

    println!(
        "{}",
        fill(
            t().report_ok,
            &[
                state,
                parsed["aggregate"].as_str().unwrap_or("?"),
                &parsed["sessions"].as_u64().unwrap_or(0).to_string(),
                if parsed["changed"].as_bool().unwrap_or(false) { "" } else { t().no_change },
            ]
        )
    );
    Ok(())
}

/// 发一次上报，**什么都不打印**。
///
/// 心跳和「收尾」用它：一轮 `run` 会发很多次，每次都打一行会把被包命令自己的输出
/// 淹掉 —— 而那个输出才是用户要看的东西。
///
/// `ttl_ms` 由调用方**强制指定**，覆盖命令行上的 `--ttl`：包装器的心跳必须知道
/// 自己留给下一次续期多少时间，让用户随手写的 `--ttl 1h` 打乱它只会制造「命令还
/// 在跑、灯已经沉默」这种怪现象。
fn post_state_quiet(
    endpoint: &Endpoint,
    state: &str,
    flags: &Flags,
    ttl_ms: u64,
) -> Result<(), Failure> {
    // 包装器不是我们装的 hook，而是用户自己的东西 —— 归到「自定义」那一组
    let mut body = json!({ "state": state, "source": "custom" });
    apply_flag_extras(&mut body, flags);
    body["ttl_ms"] = json!(ttl_ms);
    // 包装器自己就是被包命令的「会话进程」：它活着说明命令还在跑。包装器被
    // Ctrl-C / kill -9 掉时收尾跑不到，这个进程号能让灯立刻收掉，而不是等 TTL。
    body["pid"] = json!(std::process::id());

    let (status, text) = read_response!(client()
        .post(&endpoint.url("/state"))
        .header("Authorization", &endpoint.auth())
        .header("Content-Type", "application/json")
        .send(body.to_string()));
    expect_success(status, &text).map(|_| ())
}

/// 心跳间隔，以及它对应的 TTL。
///
/// TTL 给到心跳的 3 倍：连丢两次才转沉默 —— 本机回环丢包极罕见，这点余量是为了
/// 「机器正好忙了一下」不至于让灯闪成沉默。反过来，**这个值决定了「包装器被杀掉
/// 之后灯还会显示在跑多久」**，所以不能给太长。
const RUN_HEARTBEAT: Duration = Duration::from_secs(20);
const RUN_TTL_MS: u64 = 60_000;

/// 把一条命令包起来上报 —— **通用适配器**。
///
/// 任何跑在终端里的工具都能这么接，**不需要知道它自己的 hook 格式**：开始前报
/// `working`，跑的过程中按心跳续期，退出后按退出码报 `done` / `error`。
/// VS Code 的任务（`tasks.json`）里也用它，见 api.md §4.13。
///
/// 四条要守住的，少一条都会把「集成」变成「添乱」：
///
/// 1. **退出码原样传出去。** 包一层不能改被包命令的语义 —— 否则 CI 里
///    `signalhub run -- make test` 会永远是绿的，那是最坏的一种集成。
/// 2. **stdout / stderr 原样透传**（继承父进程），所以管道、重定向、颜色都照旧。
/// 3. **上报失败绝不影响被包的命令。** 灯没开、端口读不到 —— 命令照跑。用户是来
///    干活还是来点灯的，这个顺序不能颠倒。
/// 4. **起不来要如实报错**：命令名写错是调用方的错，那时报 `error` 并返回非零。
fn run_command(flags: &Flags) -> Result<(), Failure> {
    let Some((program, args)) = flags.command.split_first() else {
        return Err(Failure::other(t().run_needs_command));
    };

    // 端点只解析一次。**读不到就直接不报**（见第 3 条），而不是让整个命令失败。
    //
    // 上报失败**只提醒一次**：说一次「灯没通」就够，每次心跳都说会把被包命令的输出
    // 淹掉；而一声不吭又会让「灯为什么不亮」无从查起 —— 尤其是端口文件还在、进程
    // 却已经关了的时候，那时 `discover` 是成功的，失败要到真正发请求才暴露。
    let endpoint = Endpoint::discover(flags).ok();
    let mut warned = false;
    let mut report = |state: &str| {
        let Some(endpoint) = &endpoint else {
            return;
        };
        if let Err(err) = post_state_quiet(endpoint, state, flags, RUN_TTL_MS)
            && !warned
        {
            warned = true;
            eprintln!("signalhub: {}", fill(t().run_report_failed, &[&err.message]));
        }
    };

    // 先报一次，让灯在命令启动**之前**就亮起来 —— 否则启动慢的工具会有几秒钟
    // 是「什么都没发生」的样子
    report("working");

    let mut child = match Command::new(program).args(args).spawn() {
        Ok(child) => child,
        Err(err) => {
            report("error");
            return Err(Failure::other(fill(
                t().run_spawn_failed,
                &[&format!("{program:?}"), &err.to_string()],
            )));
        }
    };

    // **用一个线程等它。** 主线程要腾出来按心跳续期，而 `recv_timeout` 让「子进程
    // 结束」**立刻**被看到 —— 不用轮询，也就不会出现「命令早就跑完了，灯还亮着
    // 最多一个心跳」。
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait());
    });

    let status = loop {
        match rx.recv_timeout(RUN_HEARTBEAT) {
            Ok(Ok(status)) => break status,
            Ok(Err(err)) => {
                return Err(Failure::other(fill(
                    t().run_wait_failed,
                    &[&format!("{program:?}"), &err.to_string()],
                )));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => report("working"),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(Failure::other(t().run_wait_thread_died));
            }
        }
    };

    // 非零退出报 `error`：对被包的命令来说那就是出错了，和它自己会怎么表达无关。
    // 「包装器被 Ctrl-C 掉、收尾没跑到」那种情况由 `stale` 兜住。
    report(if status.success() { "done" } else { "error" });

    if status.success() {
        return Ok(());
    }
    // 被信号杀掉时 `code()` 是 `None`（Unix 上）—— 那时没有可传递的码，按我们的
    // 通用错误码走，但**仍然不打任何东西**（见 `Failure::passthrough`）
    Err(Failure::passthrough(
        status.code().map_or(EXIT_ERROR, |code| (code & 0xff) as u8),
    ))
}

/// 路径段百分号编码。
///
/// 会话名是上报方随便起的（`feature/x`、`我的会话`、带空格的都有），直接拼进
/// `/sessions/{session}` 会变成另一个路径或者一个非法 URI —— 表现是 `clear`
/// 和 `SessionEnd` 静默地没清掉那条会话（issue #17）。
fn encode_path_segment(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn clear(flags: &Flags) -> Result<(), Failure> {    let endpoint = Endpoint::discover(flags)?;
    let session = flags.session.as_deref().unwrap_or("default");

    let (status, text) = read_response!(client()
        .delete(&endpoint.url(&format!("/sessions/{}", encode_path_segment(session))))
        .header("Authorization", &endpoint.auth())
        .call());
    let parsed = expect_success(status, &text)?;

    println!(
        "{}",
        fill(
            t().clear_line,
            &[
                session,
                if parsed["removed"].as_bool().unwrap_or(false) { t().clear_removed } else { t().clear_absent },
                parsed["aggregate"].as_str().unwrap_or("?"),
            ]
        )
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

    let paused = parsed["paused"].as_bool().unwrap_or(false);
    println!(
        "{}",
        fill(
            t().monitor_line,
            &[
                if paused { t().monitor_paused } else { t().monitor_resumed },
                if paused { "off" } else { "ok" },
            ]
        )
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
            return Err(Failure::other(fill(
                t().until_unknown,
                &[&format!("{until:?}"), &WATCHABLE_STATES.join(" ")],
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
        Some(until) => eprintln!("{}", fill(t().watch_until_hint, &[until])),
        None => eprintln!("{}", t().watch_follow_hint),
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
            println!("{}", fill(t().watch_line, &[&aggregate, &sessions.to_string()]));
            return Ok(());
        }

        // 只在变化时输出：每 500ms 刷一行会把日志淹掉
        if last.as_deref() != Some(aggregate.as_str()) {
            if flags.json {
                println!("{}", serde_json::to_string(&parsed).unwrap_or_default());
            } else {
                println!("{}", fill(t().watch_line, &[&aggregate, &sessions.to_string()]));
            }
            last = Some(aggregate.clone());
        }

        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Err(Failure::other(fill(
                t().watch_timeout,
                &[&aggregate, until.as_deref().unwrap_or("?")],
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
        "{}",
        fill(t().status_aggregate, &[parsed["aggregate"].as_str().unwrap_or("?")])
    );
    if parsed["paused"].as_bool().unwrap_or(false) {
        println!("{}", t().status_paused);
    }
    if let Some(message) = parsed["message"].as_str() {
        println!("{}", fill(t().status_hint, &[message]));
    }

    let empty = Vec::new();
    let sessions = parsed["sessions"].as_array().unwrap_or(&empty);
    println!("{}", fill(t().status_sessions, &[&sessions.len().to_string()]));
    for session in sessions {
        println!(
            "  - {:<16} {:<11} {}{}{}",
            session["id"].as_str().unwrap_or("?"),
            session["state"].as_str().unwrap_or("?"),
            format_ms(session["age_ms"].as_u64().unwrap_or(0)),
            session["agent"]
                .as_str()
                .map(|agent| fill(t().session_agent, &[agent]))
                .unwrap_or_default(),
            session["message"]
                .as_str()
                .map(|message| fill(t().session_message, &[message]))
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
        Failure::other(fill(t().agent_needed, &[what, what]))
    })
}

/// 一键诊断：打印一段排查用的文本（不发送、不修改任何东西）。
///
/// **给谁用**：接不上的人里有一部分只开终端。它和设置面板里那个「复制诊断信息」
/// 按钮走的是**同一份代码**（`diagnose::text`），所以贴出来的东西和界面上的一致 ——
/// 两处各写一遍的话，用户贴出来的那段迟早会和界面对不上。
///
/// 位置参数可选：`signalhub diagnose opencode` 只看那一个。
fn run_diagnose(agent: Option<&str>) -> Result<(), Failure> {
    let filter = match agent {
        Some(name) => Some(hooks::Agent::parse(name).map_err(Failure::other)?),
        None => None,
    };
    print!("{}", diagnose::text(&receipts::Receipts::load(), filter));
    Ok(())
}

fn install_hooks(agent: &str, flags: &Flags) -> Result<(), Failure> {
    let agent = hooks::Agent::parse(agent).map_err(Failure::other)?;
    let report = if flags.approve {
        // 端口和 token 都取**此刻**的真实值：写进用户配置文件的东西不能来自一份
        // 可能过期的快照（token 是每台机器一份的秘密，从文件里读，不从参数收）
        let port = config::read_port()
            .map_err(|err| Failure::other(fill(t().http_needs_port, &[&err.to_string()])))?;
        let token = config::read_token()
            .map_err(|err| Failure::other(fill(t().approve_needs_token, &[&err.to_string()])))?;
        hooks::install_approve(agent, port, &token, flags.dry_run).map_err(Failure::other)?
    } else if flags.http {
        // 端口取**此刻**服务真正在用的那个：写进用户配置文件的东西不能来自一份
        // 可能过期的快照，而读不到端口就说明应用还没起来 —— 那时 URL 一定是错的
        let port = config::read_port().map_err(|err| {
            Failure::other(fill(t().http_needs_port, &[&err.to_string()]))
        })?;
        hooks::install_http(agent, port, flags.dry_run).map_err(Failure::other)?
    } else {
        hooks::install(agent, flags.dry_run).map_err(Failure::other)?
    };

    println!(
        "{}",
        fill(
            t().install_title,
            &[
                agent.name(),
                if report.dry_run { t().dry_run_suffix } else { t().installed },
            ]
        )
    );
    if let Some(command) = &report.command {
        println!("{}", fill(t().install_command, &[command]));
    }
    if flags.approve {
        // **这一条必须说**：装了 hook 不等于审批生效 —— 真正的闸门是应用里那个
        // 「从灯上审批」开关（默认关）。不说的话，用户看到的是「装好了」，
        // 而实际上每次要批准时还是回终端里问 —— 那种「装上了却永远不亮」正是
        // 这个项目最不能接受的一种失败。
        println!("{}", t().approve_needs_switch);
    }
    println!("{}", fill(t().install_config, &[&report.path.display().to_string()]));
    if let Some(backup) = &report.backup {
        println!("{}", fill(t().install_backup, &[&backup.display().to_string()]));
    }
    if !report.added.is_empty() {
        println!("{}", fill(t().install_added, &[&report.added.len().to_string()]));
        println!("        {}", report.added.join(" "));
    }
    if !report.updated.is_empty() {
        println!("{}", fill(t().install_updated, &[&report.updated.len().to_string()]));
        println!("        {}", report.updated.join(" "));
    }
    if !report.already.is_empty() {
        println!("{}", fill(t().install_skipped, &[&report.already.len().to_string()]));
    }

    if report.added.is_empty() && report.updated.is_empty() {
        println!("{}", t().install_no_change);
    } else if report.dry_run {
        println!("{}", t().dry_run_footer);
    } else {
        println!("{}", fill(t().install_restart, &[agent.name()]));
    }

    // 没在 PATH 里就只能写绝对路径，此时二进制一挪动 hook 就失效——这种失效
    // 是静默的（智能体会话里什么都看不到），所以必须主动提醒。
    if !report.dry_run && !hooks::signalhub_on_path() {
        println!();
        println!("{}", t().path_warning_1);
        println!("{}", t().path_warning_2);
        println!("{}", t().path_warning_3);
    }
    Ok(())
}

fn uninstall_hooks(agent: &str, flags: &Flags) -> Result<(), Failure> {
    let agent = hooks::Agent::parse(agent).map_err(Failure::other)?;
    let report = hooks::uninstall(agent, flags.dry_run).map_err(Failure::other)?;

    println!(
        "{}",
        fill(
            t().install_title,
            &[
                agent.name(),
                if report.dry_run { t().dry_run_suffix } else { t().uninstalled },
            ]
        )
    );
    println!("{}", fill(t().install_config, &[&report.path.display().to_string()]));
    if let Some(backup) = &report.backup {
        println!("{}", fill(t().install_backup, &[&backup.display().to_string()]));
    }

    if report.removed_handlers == 0 {
        println!("{}", t().uninstall_nothing);
    } else {
        println!("{}", fill(t().install_removed_handlers, &[&report.removed_handlers.to_string()]));
        if !report.removed.is_empty() {
            println!("        {}", report.removed.join(" "));
        }
        if report.dry_run {
            println!("{}", t().dry_run_footer);
        }
    }
    Ok(())
}

// ---- hook ------------------------------------------------------------------

/// 把一次 hook 上报拼成请求体。
///
/// **单独摘出来是为了能单测**：这段以前直接写在 `hook()` 里，而它最容易出错的一
/// 项（`message`）恰好没有任何测试够得着 —— 于是它空了不知道多久也没人发现。
///
/// `message` 走的是「工具事件顺手生成一句正在做什么」，见 [`tool_summary`]。
/// 注意服务端是**每次上报整体替换** message 的（`state.rs` 里那句「message 是
/// 事件细节，每次替换」），所以 PreToolUse 和 PostToolUse 都得带 —— 只带一个的话
/// 牌上那行会在两步之间闪一下空。
fn hook_body(
    text: &CliText,
    state: &str,
    session: &str,
    agent: &str,
    payload: &Value,
    flags: &Flags,
    pid: Option<u32>,
) -> Value {
    // 走 hook 报的**一定是智能体**：这个子命令就是给我们装进各家配置里的那条命令用的
    let mut body = json!({ "state": state, "session": session, "agent": agent, "source": "agent" });
    if let Some(cwd) = cwd_of(payload) {
        body["cwd"] = json!(cwd);
    }
    if let Some(summary) = tool_summary(text, payload) {
        body["message"] = json!(summary);
    }
    // payload 里的 cwd / session 是准的；但用户在命令行上显式写的选项优先 ——
    // 装出来的 hook 命令本来一个选项都不带，所以这条只在手改过命令时才会生效。
    apply_flag_extras(&mut body, flags);
    // 进程号由调用方读好传进来（而不是在这里读环境变量）：这个函数要能单测，
    // 而进程级的环境变量在并行测试里改不得。
    if let Some(pid) = pid {
        body["pid"] = json!(pid);
    }
    body
}


/// 从工具事件生成一句「正在做什么」，填进上报的 `message`。
///
/// **它补的是一个洞**：`hook` 模式此前从不发 `message`，而牌上「消息」那一行默认
/// 是开的（窗口高度也为此留了第二档）—— 也就是说走 hook 接入的人（绝大多数）
/// 永远看不到那一行，`tool_name` / `tool_input` 却一直躺在 payload 里没人用。
///
/// **只取一个词，绝不整条打出去。** `tool_input.command` 可能带密钥
/// （`API_KEY=xxx curl ...`），也可能是一长串参数，而这句话会进灯的状态、系统通知
/// 正文和消息流 —— 那不是它该去的地方。所以：命令只取第一个不是 `KEY=value` 的
/// 词、路径只取末段、URL 只取主机名，一律截断。
///
/// **认不出来就回落到工具原名，不猜动词。** 各家的工具名不一样、还会随版本变，
/// 与其按关键字猜一个（`TodoWrite` 里也有 "write"），不如把原名显示出来 ——
/// 那至少是真的。这条兜底也让上面那张表可以随时补，补漏了只是少一点可读性。
fn tool_summary(text: &CliText, payload: &Value) -> Option<String> {
    let name = payload["tool_name"].as_str()?.trim();
    if name.is_empty() {
        return None;
    }
    match (tool_verb(name), tool_target(&payload["tool_input"])) {
        (Some(verb), Some(target)) => Some(fill(text.tool_doing, &[verb_text(text, verb), &target])),
        (Some(verb), None) => Some(verb_text(text, verb).to_string()),
        (None, _) => Some(clip(name, TOOL_NAME_MAX)),
    }
}

/// 动词分类 → 这个词。分类在共享模块（`hookmap`），词在这里 —— 因为这句话要进
/// 灯、系统通知和消息流，那三处都要说用户的语言。
fn verb_text(text: &CliText, verb: Verb) -> &'static str {
    match verb {
        Verb::Exec => text.tool_v_exec,
        Verb::Edit => text.tool_v_edit,
        Verb::Read => text.tool_v_read,
        Verb::Search => text.tool_v_search,
        Verb::Net => text.tool_v_net,
    }
}

/// hook 路径上**不允许**出现任何 `println!`。见文件头的约束 1。
fn hook(agent: &str, flags: &Flags) -> Result<(), Failure> {
    // **约束 1 的唯一例外，而且是有理由的。** Gemini 明确要求 hook 的 stdout 是
    // 纯 JSON（"must not print any plain text to stdout other than the final
    // JSON"），而 exit 0 时它会把 stdout 当 JSON 解析；空 stdout 算不算合法 JSON
    // 它没写清楚。打一个空对象在两种解读下都安全：合法的 JSON，且不含任何
    // decision / continue 字段 —— 等于「没有意见」，不改变它的任何行为。
    //
    // 放在最前面是为了覆盖所有出口（包括下面「stdin 不是合法 JSON」那条提前
    // return 的路径）。stdout 是进程结束后才被读的，先打后打都一样。
    //
    // 其余几家都明确接受空 stdout，所以不加。
    if agent == "gemini" {
        println!("{{}}");
    }

    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(|err| Failure::other(fill(t().read_stdin_failed, &[&err.to_string()])))?;

    let payload: Value = match serde_json::from_str(&input) {
        Ok(payload) => payload,
        Err(err) => {
            log_hook(agent, "?", "-", &fill(t().stdin_not_json, &[&err.to_string()]));
            return Err(Failure::other(fill(t().stdin_not_json, &[&err.to_string()])));
        }
    };

    let event = event_name(&payload).unwrap_or("?").to_string();
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

    // 上报的请求体**在这里构造一次**：它既是发出去的内容，也是攒下来等重放的内容。
    // 清除没有请求体（会话名就在 URL 里），所以是 `None`。
    let report_body = match &action {
        Action::Report(state) => Some(hook_body(
            t(),
            state,
            &session,
            agent,
            &payload,
            flags,
            env_pid(),
        )),
        Action::Clear => None,
    };

    let result = send_action(flags, &action, report_body.as_ref(), &session);

    match &result {
        Ok(()) => log_hook(agent, &event, &session, &outcome),
        // **只在「连不上」时攒起来**，而不是任何失败：
        // - 4xx/5xx 是这次请求本身不对（或服务端明确拒绝），重放只会再错一次；
        // - 超时也不攒：那个请求**可能已经被处理过了**，重放会造成重复，而重复的
        //   一条会把已经前进的状态倒回去（一次迟到的 `done` 能让 `working` 复活）。
        // 真正要救的场景是「app 没开 / 刚重启 / 还没启动」，那时是连接被拒，
        // 判据正好就是 `not_running`（见 `classify`）。
        Err(err) if err.not_running => {
            // 有请求体的一定是上报 —— `report_body` 就是照着 `action` 构造的
            let op = match report_body {
                Some(body) => spool::SpoolOp::Report { body },
                None => spool::SpoolOp::Clear { session: session.clone() },
            };
            spool::append(&spool::Spooled::now(op));
            log_hook(agent, &event, &session, &fill(t().hook_spooled, &[&err.message]));
        }
        Err(err) => log_hook(agent, &event, &session, &fill(t().hook_failed, &[&err.message])),
    }
    result
}

/// 把这一条动作真的发出去。
///
/// **单独一个函数，而不是写在 `hook()` 里** —— 这一点很关键：`read_response!`
/// 里有一个 `?`，而 `?` 是把失败从**当前函数**直接抛出去的。留在 `hook()` 里的话，
/// 「连不上 app」这条**最常见的**失败会绕过调用方的日志和攒盘，一路抛到 `main`：
/// 于是 hook 出了最该被记下来的那种故障，日志里却一个字都没有（早先正是如此 ——
/// 只有服务端明确拒绝的那几种留得下痕）。放在这里，失败就是一个返回值，
/// 由 `hook()` 决定怎么记、要不要攒。
fn send_action(
    flags: &Flags,
    action: &Action,
    report_body: Option<&Value>,
    session: &str,
) -> Result<(), Failure> {
    let endpoint = Endpoint::discover(flags)?;
    match action {
        Action::Report(_) => {
            let (status, text) = read_response!(client()
                .post(&endpoint.url("/state"))
                .header("Authorization", &endpoint.auth())
                .header("Content-Type", "application/json")
                .send(report_body.expect("上报一定有请求体").to_string()));
            expect_success(status, &text).map(|_| ())
        }
        Action::Clear => {
            let (status, text) = read_response!(client()
                .delete(&endpoint.url(&format!("/sessions/{}", encode_path_segment(session))))
                .header("Authorization", &endpoint.auth())
                .call());
            expect_success(status, &text).map(|_| ())
        }
    }
}

/// 记一行 hook 调用。
///
/// **只写日志文件，绝不碰 stdout**（约束 1）。任何失败都忽略：为了记日志而
/// 弄坏智能体的 hook 是本末倒置。
///
/// 真正落盘那一步在 `config::append_hook_log` —— 出站钩子（app 那边发的）写的是
/// 同一个文件，轮转逻辑只能有一份，否则迟早有一处忘了加、那个文件就无界增长。
fn log_hook(agent: &str, event: &str, session: &str, outcome: &str) {
    config::append_hook_log(&format!(
        "{}  {agent:<12} {event:<20} session={session:<38} {outcome}\n",
        config::utc_hms()
    ));
}


fn help() -> String {
    // 正文在 `cli_text.rs`（中英各一份），这里只把版本号和智能体名单填进去。
    // 一屏帮助有 60 多行，塞在这个文件里会让逻辑更难找。
    fill(
        t().help,
        &[env!("CARGO_PKG_VERSION"), &hooks::ALL_NAMES.join(" ")],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    // 只有这条用例用得着（非测试构建里是死代码，所以不放进上面的导入表）
    use crate::hookmap::TARGET_MAX;

    /// 会话名进 URL 路径前必须编码：`feature/x` 会变成另一个路径，中文和空格
    /// 会拼出非法 URI —— 两种都表现为 `clear` 静默地没清掉（issue #17）。
    #[test]
    fn session_names_are_encoded_before_going_into_the_path() {
        assert_eq!(encode_path_segment("build-1"), "build-1");
        assert_eq!(encode_path_segment("feature/x"), "feature%2Fx");
        assert_eq!(encode_path_segment("我的会话"), "%E6%88%91%E7%9A%84%E4%BC%9A%E8%AF%9D");
        assert_eq!(encode_path_segment("a b?c#d"), "a%20b%3Fc%23d");
    }

    fn event(name: &str) -> Value {
        json!({ "hook_event_name": name, "session_id": "s-1", "cwd": "/proj" })
    }

    /// **「试一下」那条合成 payload 必须真的被映射表认出来。**
    ///
    /// 这是那个按钮唯一会骗人的地方：需求里它就是「把配置里那条命令跑一遍」，而如果
    /// 它合成的事件名映射表不认识，那条命令只会静默地什么都不做（日志里留一行
    /// `ignored`）——按钮却会说「命令跑起来了」。所以这张表和映射表必须有这条测试
    /// 把它们钉在一起。
    ///
    /// 走的是和真实钩子**同一条**解析路径（`events_of` + `map_event`），不是照着
    /// 名字对一遍：认不认识由映射表说了算。
    #[test]
    fn the_probe_payload_is_recognised_by_every_command_agent() {
        for name in hooks::ALL_NAMES {
            let agent = hooks::Agent::parse(name).unwrap();
            let payload = hooks::probe_payload(agent);

            let Some(payload) = payload else {
                // 插件式接入：没有能单独跑的命令，所以也没有 payload。反过来也要成立 ——
                // 除了那三家，谁都不该缺 payload。
                assert!(
                    matches!(name, "opencode" | "pi" | "hermes"),
                    "{name}: 只有插件式接入才该没有 payload"
                );
                continue;
            };

            let payload: Value = serde_json::from_str(&payload).expect("payload 要是合法 JSON");
            assert!(
                state_of(name, &payload).is_some(),
                "{name} 不认识合成 payload —— 「试一下」会变成一次假通过"
            );
            assert_eq!(
                session_id(&payload).as_deref(),
                Some("signalhub-probe"),
                "{name}: 会话名要一眼看得出是试出来的"
            );
        }
    }

    fn state_of(agent: &str, payload: &Value) -> Option<&'static str> {
        match map_event(agent, payload)? {
            Action::Report(state) => Some(state),
            Action::Clear => None,
        }
    }

    /// 和 `state_of` 的区别：把「清除会话」也表达出来，而不是和「不认识这个事件」
    /// 一样返回 None。要断言 SessionEnd 的映射就得用这个。
    fn action_of(agent: &str, payload: &Value) -> Option<&'static str> {
        match map_event(agent, payload)? {
            Action::Report(state) => Some(state),
            Action::Clear => Some("clear"),
        }
    }

    // ---- Gemini CLI ----

    /// Gemini 用 PascalCase，语义和 Claude Code 那一族一致
    #[test]
    fn gemini_maps_its_event_set() {
        for (name, want) in [
            ("SessionStart", "idle"),
            ("BeforeAgent", "thinking"),
            ("PreCompress", "thinking"),
            ("BeforeTool", "working"),
            ("AfterTool", "working"),
            ("Notification", "attention"),
            ("AfterAgent", "done"),
        ] {
            assert_eq!(state_of("gemini", &event(name)), Some(want), "{name}");
        }
        assert_eq!(action_of("gemini", &event("SessionEnd")), Some("clear"));

        // 每次模型调用都触发的那几个：不装也不映射，否则灯会一直在闪
        for noise in ["BeforeModel", "AfterModel", "BeforeToolSelection"] {
            assert_eq!(state_of("gemini", &event(noise)), None, "{noise} 该被忽略");
        }
    }

    // ---- Goose ----

    /// Goose 的事件名和 Claude Code 同构，但**事件名在 `event`**、
    /// 工作目录在 `working_dir` —— 这两条是它和其余几家最容易踩岔的地方
    #[test]
    fn goose_reads_its_own_field_names() {
        let payload = json!({
            "event": "PreToolUse",
            "session_id": "s-9",
            "working_dir": "/proj",
        });

        assert_eq!(state_of("goose", &payload), Some("working"));
        assert_eq!(session_id(&payload).as_deref(), Some("s-9"));
        assert_eq!(cwd_of(&payload).as_deref(), Some("/proj"));
    }

    #[test]
    fn goose_maps_its_event_set() {
        for (name, want) in [
            ("SessionStart", "idle"),
            ("UserPromptSubmit", "thinking"),
            ("PreToolUse", "working"),
            ("PostToolUse", "working"),
            ("PostToolUseFailure", "attention"),
            ("Stop", "done"),
        ] {
            assert_eq!(state_of("goose", &event(name)), Some(want), "{name}");
        }
        assert_eq!(action_of("goose", &event("SessionEnd")), Some("clear"));

        // 读文件是高频事件，装了灯会一直闪
        for noise in ["BeforeReadFile", "AfterFileEdit"] {
            assert_eq!(state_of("goose", &event(noise)), None, "{noise} 该被忽略");
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

    /// CSC（issue #22）与 Claude Code 的映射**逐条一致** —— 它是同一个 hook 机制的
    /// 分支，认错一条的表现是那种事件在灯上完全没有反应。
    #[test]
    fn csc_maps_exactly_like_claude_code() {
        for name in [
            "SessionStart",
            "UserPromptSubmit",
            "PreToolUse",
            "PostToolUse",
            "PostToolUseFailure",
            "PermissionRequest",
            "PermissionDenied",
            "Notification",
            "SubagentStart",
            "PreCompact",
            "PostCompact",
            "Stop",
            "StopFailure",
            "TeammateIdle",
            "SessionEnd",
        ] {
            let payload = event(name);
            assert_eq!(
                action_of("csc", &payload),
                action_of("claude-code", &payload),
                "{name} 两个名字给出了不同的动作"
            );
        }
        assert_eq!(state_of("csc", &event("PreToolUse")), Some("working"));
        assert_eq!(state_of("csc", &event("PermissionRequest")), Some("permission"));
        assert_eq!(state_of("csc", &event("Stop")), Some("done"));
        assert!(matches!(map_event("csc", &event("SessionEnd")), Some(Action::Clear)));
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

    /// `--` 之后的一切**原样**进 `command`，一个都不解析。
    ///
    /// 这条是 `run` 的地基：被包命令自己的选项（`--message`、`--ttl`…）很可能和我们
    /// 的重名，不隔开的话就会被我们吃掉 —— 而那种错**不会报错**，只会让被包的命令
    /// 莫名其妙少了个参数。
    #[test]
    fn everything_after_a_double_dash_is_handed_over_untouched() {
        let (flags, _) = parse(&[
            "run", "--agent", "aider", "--",
            "aider", "--model", "sonnet", "--ttl", "1h", "-x",
        ])
        .unwrap();

        // 我们自己的选项照常解析
        assert_eq!(flags.agent.as_deref(), Some("aider"));
        // 但 `--` 之后的 `--ttl` 是**被包命令**的，不该被我们吃掉
        assert_eq!(flags.ttl_ms, None, "-- 之后的选项不该被我们解析");
        assert_eq!(
            flags.command,
            vec!["aider", "--model", "sonnet", "--ttl", "1h", "-x"]
        );
    }

    /// 没有 `--` 就没有命令 —— `run` 要因此报错，而不是静默跑一个空命令。
    #[test]
    fn a_run_without_a_command_has_nothing_to_run() {
        let (flags, _) = parse(&["run", "--agent", "aider"]).unwrap();
        assert!(flags.command.is_empty());
    }

    /// `--` 单独出现（后面什么都没有）也要能解析，而不是被当成「未知选项」。
    #[test]
    fn a_bare_double_dash_is_not_an_unknown_option() {
        let (flags, _) = parse(&["run", "--"]).unwrap();
        assert!(flags.command.is_empty());
    }

    /// `run` 把被包命令的退出码**原样**传出去，而且不补一句我们自己的话。
    ///
    /// 这是「包一层不能改语义」那条约束的落点：CI 里
    /// `signalhub run -- make test` 必须让 make 的失败仍然是失败。
    #[test]
    fn a_passthrough_failure_carries_the_childs_code_and_says_nothing() {
        let failure = Failure::passthrough(7);
        assert_eq!(failure.code(), 7);
        assert!(failure.message.is_empty(), "不该补我们自己的话");
        // 边界：0 也能原样传（虽然实践中不会走到，`run` 对 0 直接返回 Ok）
        assert_eq!(Failure::passthrough(0).code(), 0);
        assert_eq!(Failure::passthrough(255).code(), 255);
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

    /// P2-1 的核心回归测试：CLI 写进上报体的键名，服务端**必须认**。
    ///
    /// 断言不能是「CLI 侧再写一遍同样的 `json!`」—— 那只是把同一个拼写错误
    /// 抄两遍。这里把 `apply_flag_extras` 的输出**反序列化进服务端那个
    /// `Report`**（`state.rs` 通过 `#[path]` 被两边一起编，是同一个类型），
    /// 于是键名一旦对不上，这条测试就红。
    #[test]
    fn flag_extras_land_in_the_servers_own_report_struct() {
        let (flags, _) = parse(&[
            "--session", "s-1",
            "--agent", "ci",
            "--cwd", "/proj",
            "--group", "builds",
            "--message", "compiling",
            "--ttl", "30s",
        ])
        .unwrap();

        let mut body = json!({ "state": "working" });
        apply_flag_extras(&mut body, &flags);

        let report: state::Report =
            serde_json::from_value(body).expect("服务端应能解析 CLI 构造的请求体");
        assert_eq!(report.group_id(), "builds");
        assert_eq!(report.session_id(), "s-1");
        assert_eq!(report.agent.as_deref(), Some("ci"));
        assert_eq!(report.cwd.as_deref(), Some("/proj"));
        assert_eq!(report.message.as_deref(), Some("compiling"));
        assert_eq!(report.ttl_ms, Some(30_000));
    }

    /// `$PPID` 没被 shell 展开时会一整个传进来 —— 当作没给，而不是把「$PPID」
    /// 当成一个进程号，更不能让整条 hook 失败。
    #[test]
    fn only_a_plain_positive_number_counts_as_a_pid() {
        assert_eq!(parse_pid("4242"), Some(4242));
        assert_eq!(parse_pid(" 7 "), Some(7));
        for raw in ["$PPID", "", "0", "12x", "-1", "99999999999999999999"] {
            assert_eq!(parse_pid(raw), None, "{raw:?} 不该被当成 pid");
        }
    }

    // ---- 牌上的「正在做什么」 --------------------------------------------------

    /// 工具事件都该变成一句人话，中英两套都对。
    ///
    /// 语言是**注入**的（直接传文案表），不是走 `t()` —— 后者背后是进程级的
    /// `OnceLock`，测试里改不了它，那这条就只能按机器上的语言碰运气。
    #[test]
    fn a_tool_event_becomes_a_readable_line() {
        let cases: [(Value, &str, &str); 6] = [
            (
                json!({"tool_name": "Bash", "tool_input": {"command": "cargo test --all"}}),
                "执行 cargo",
                "Running cargo",
            ),
            (
                json!({"tool_name": "Edit", "tool_input": {"file_path": "/a/b/server.rs"}}),
                "改 server.rs",
                "Editing server.rs",
            ),
            (
                json!({"tool_name": "Read", "tool_input": {"file_path": "/a/b/README.md"}}),
                "读 README.md",
                "Reading README.md",
            ),
            (
                json!({"tool_name": "Grep", "tool_input": {"pattern": "resolve_group"}}),
                "找 resolve_group",
                "Searching resolve_group",
            ),
            (
                json!({"tool_name": "WebFetch", "tool_input": {"url": "https://docs.rs/tauri/latest"}}),
                "访问 docs.rs",
                "Fetching docs.rs",
            ),
            // 只有工具名、没有可用参数时也给一句，只是没有目标
            (
                json!({"tool_name": "Write", "tool_input": {}}),
                "改",
                "Editing",
            ),
        ];

        for (payload, zh, en) in cases {
            assert_eq!(tool_summary(&cli_text::ZH, &payload).as_deref(), Some(zh), "{payload}");
            assert_eq!(tool_summary(&cli_text::EN, &payload).as_deref(), Some(en), "{payload}");
        }
    }

    /// **命令只取第一个不是赋值的词。**
    ///
    /// 整条 `tool_input.command` 可能带密钥（`API_KEY=xxx curl ...`）或者一长串
    /// 参数，而这句话会进灯的状态、系统通知正文和消息流 —— 那不是它该去的地方。
    /// 这条测试就是「不把整条命令打出去」那条规则的落点。
    #[test]
    fn a_shell_command_is_reduced_to_its_program() {
        let get = |cmd: &str| {
            tool_summary(
                &cli_text::ZH,
                &json!({"tool_name": "Bash", "tool_input": {"command": cmd}}),
            )
        };

        assert_eq!(get("cargo test --features=a=b").as_deref(), Some("执行 cargo"));
        assert_eq!(get("API_KEY=sk-live-123 curl https://x").as_deref(), Some("执行 curl"));
        assert_eq!(get("/usr/local/bin/npx vite").as_deref(), Some("执行 npx"));
        // 参数里的 `=` 不该让整条命令被跳过
        assert_eq!(get("npm run build -- --mode=prod").as_deref(), Some("执行 npm"));
        // 最后一道：密钥绝不能出现在结果里
        assert!(!get("TOKEN=abcd1234 curl x").unwrap().contains("abcd1234"));
    }

    /// 认不出来的工具名**原样显示**，不按关键字猜。
    ///
    /// `TodoWrite` 里有 "write"、`ExitPlanMode` 里有 "plan" —— 按关键字来会一路
    /// 认错，而认错比认不出来更糟。漏一个名字只是少一点可读性。
    #[test]
    fn an_unknown_tool_keeps_its_own_name() {
        for name in ["TodoWrite", "ExitPlanMode", "Task", "some_new_tool"] {
            let payload = json!({"tool_name": name, "tool_input": {}});
            assert_eq!(
                tool_summary(&cli_text::ZH, &payload).as_deref(),
                Some(name),
                "{name} 不该被猜成别的动词"
            );
        }
    }

    /// 工具名的大小写和连字符写法都要认 —— 各家不一致，还会随版本变。
    #[test]
    fn tool_names_are_matched_loosely() {
        for name in ["Bash", "bash", "run_terminal_cmd", "run-terminal-cmd", "Shell"] {
            let payload = json!({"tool_name": name, "tool_input": {}});
            assert_eq!(tool_summary(&cli_text::ZH, &payload).as_deref(), Some("执行"), "{name}");
        }
    }

    /// 没有工具名的事件（`SessionStart` / `Stop` / `UserPromptSubmit` 之类）
    /// **不生成 message** —— 那时没有任何「正在做什么」可说，硬编一句是撒谎。
    #[test]
    fn events_without_a_tool_produce_no_message() {
        assert_eq!(tool_summary(&cli_text::ZH, &json!({"session_id": "s-1"})), None);
        assert_eq!(tool_summary(&cli_text::ZH, &json!({"tool_name": "   "})), None);
        // 工具名不是字符串（版本差异下可能给成 null）
        assert_eq!(tool_summary(&cli_text::ZH, &json!({"tool_name": null})), None);
    }

    /// 一律截断，而且**按字符截**。
    ///
    /// 按字节切会把一个汉字切成半个 —— 那不只是难看，是无效 UTF-8，而这句话要
    /// 经过 HTTP 和 JSON 两层，出问题的地方会离这里很远。
    #[test]
    fn long_targets_are_clipped_on_character_boundaries() {
        let ascii = "a".repeat(200);
        let out = tool_summary(
            &cli_text::ZH,
            &json!({"tool_name": "Read", "tool_input": {"file_path": format!("/x/{ascii}")}}),
        )
        .unwrap();
        assert!(out.ends_with('…'), "{out}");
        // 上限 = 动词 + 空格 + 目标上限 + 省略号。写成算式而不是一个魔数：
        // 英文的动词比中文长（"Reading" vs "读"），换个语言这个界限就得跟着变。
        let budget = cli_text::ZH.tool_v_read.chars().count() + 1 + TARGET_MAX + 1;
        assert!(out.chars().count() <= budget, "截得不够: {out}（上限 {budget}）");

        // 全中文：不能 panic，也不能切出半个字（`chars()` 会替我们保证）
        let han = "中文文件名".repeat(20);
        let out = tool_summary(
            &cli_text::ZH,
            &json!({"tool_name": "Read", "tool_input": {"file_path": format!("/x/{han}")}}),
        )
        .unwrap();
        assert!(out.starts_with("读 中文文件名"), "{out}");
        assert_eq!(out.chars().last(), Some('…'), "{out}");
    }

    /// 走一遍请求体组装：工具事件要带上 message，而且**服务端得认得这个键**。
    ///
    /// 最后那步反序列化不是形式主义：服务端没有 `deny_unknown_fields`，键名拼错
    /// 只会被静默丢掉 —— 多分组以前正是这么栽的（见 `apply_flag_extras` 的注释）。
    #[test]
    fn the_hook_body_carries_the_tool_summary_in_its_message_field() {
        let payload = json!({
            "hook_event_name": "PreToolUse",
            "session_id": "s-1",
            "cwd": "/proj",
            "tool_name": "Edit",
            "tool_input": {"file_path": "/proj/src/state.rs"},
        });
        let flags = Flags::default();

        let body = hook_body(
            &cli_text::ZH,
            "working",
            "s-1",
            "claude-code",
            &payload,
            &flags,
            Some(4242),
        );
        assert_eq!(body["message"], "改 state.rs");
        assert_eq!(body["pid"], 4242, "hook 路径也要带上进程号");

        let report: state::Report =
            serde_json::from_value(body).expect("服务端应能解析 hook 构造的请求体");
        assert_eq!(report.message.as_deref(), Some("改 state.rs"));
        assert_eq!(report.session_id(), "s-1");
        assert_eq!(report.cwd.as_deref(), Some("/proj"));

        // 没有工具名的事件：这个键**根本不出现**，而不是发个空串
        let plain = json!({"hook_event_name": "Stop", "session_id": "s-1"});
        let body = hook_body(&cli_text::ZH, "done", "s-1", "claude-code", &plain, &flags, None);
        assert!(body.get("message").is_none(), "{body}");
    }

    /// 不传 `--group` 时这个键**根本不出现**，而不是发一个空串。
    /// `docs/api.md` 里「不传就是 default」说的是「键缺席」；空串虽然也会被
    /// `group_id()` 归到 default，但在线上多一个没意义的字段，也让抓包时
    /// 分不清「没给」和「给空了」。
    #[test]
    fn an_absent_group_is_left_out_of_the_body_entirely() {
        let (flags, _) = parse(&["working"]).unwrap();
        let mut body = json!({ "state": "working" });
        apply_flag_extras(&mut body, &flags);

        assert!(body.get("group").is_none(), "{body}");
        assert_eq!(body.as_object().unwrap().len(), 1, "只该有 state：{body}");
    }

    /// 报错文案**跟着界面语言走**，所以断言里不能写死某一种语言的措辞。
    ///
    /// 这个坑真的踩到过：CI 的 macOS runner 设了 `LANG=en_US.UTF-8`，而开发机上
    /// 常常根本没有 `LANG`（于是走默认的中文）—— 写死中文的断言就「本地绿、CI 红」。
    ///
    /// 两种语言下的说法都算通过，`message` 必须**正好是其中一个**（不是「包含」，
    /// 那样漏掉了措辞变化也看不出来）。
    fn assert_says_one_of(message: &str, zh: &str, en: &str) {
        assert!(
            message == zh || message == en,
            "既不是中文那份也不是英文那份：{message}"
        );
    }

    #[test]
    fn an_unknown_flag_is_an_error() {
        let err = parse(&["--nope"]).unwrap_err();
        assert_says_one_of(
            &err.message,
            &fill(cli_text::ZH.unknown_option, &["--nope"]),
            &fill(cli_text::EN.unknown_option, &["--nope"]),
        );
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
        let states = WATCHABLE_STATES.join(" ");
        assert_says_one_of(
            &err.message,
            &fill(cli_text::ZH.until_unknown, &["\"workng\"", &states]),
            &fill(cli_text::EN.until_unknown, &["\"workng\"", &states]),
        );
        // 可用状态那一段是**语言无关**的，两种语言下都该在
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
    fn help_documents_every_command_and_option_in_both_languages() {
        // **刻意不走 `help()`**：它按当前配置解析语言，而那取决于开发机上的
        // `~/.signalhub` —— 同一个测试在中文机器和英文机器上会走不同的分支。
        // 这两张表是给用户看的，两边都得站得住。
        for (name, table, exit_heading) in [
            ("中文", &cli_text::ZH, "退出码"),
            ("英文", &cli_text::EN, "Exit codes"),
        ] {
            // 版本号和智能体名单是填进去的，替两个假的看看占位符在不在
            let text = fill(table.help, &["9.9.9", "claude-code codex"]);
            for expected in [
                "hook", "status", "watch", "run", "clear", "monitor", "install", "uninstall",
                "claude-code", "9.9.9", "--never-fail", "--ttl", "--dry-run", "--until",
                "--timeout", "--interval", "--group", "--session", "--agent", "--port",
                "--source",
                exit_heading,
            ] {
                assert!(text.contains(expected), "{name} 帮助文本缺少 {expected}");
            }
            // 通用适配那一节也得在 —— 它是「其他工具怎么接」的唯一出处
            assert!(text.contains("run --agent"), "{name} 帮助里没有 run 的用法示例");
        }
    }

    /// `--source` 只认三个值，别的**当场报错**。
    ///
    /// 不校验的话，服务端会照收（它不做校验）、界面会把它归到「自定义」—— 于是
    /// 「我明明声明了 system」变成一个没有任何提示的哑谜。
    #[test]
    fn only_three_source_types_are_accepted() {
        assert_eq!(parse_source("system").unwrap(), "system");
        assert_eq!(parse_source("  AGENT ").unwrap(), "agent", "两头空白和大小写都该收");
        let err = parse_source("sytem").unwrap_err();
        assert!(err.message.contains("sytem"), "{}", err.message);
        assert!(err.message.contains("custom"), "错误里要写清认哪几个: {}", err.message);
    }

    /// 没声明 `--source` 时按名字推：我们那 9 家（含简称）算智能体，其余算自定义。
    #[test]
    fn the_source_is_inferred_from_the_agent_name() {
        assert_eq!(inferred_source(Some("claude-code")), "agent");
        assert_eq!(inferred_source(Some("claude")), "agent", "简称也该认出来");
        assert_eq!(inferred_source(Some("opencode")), "agent");
        assert_eq!(inferred_source(Some("aider")), "custom", "没一键接入的智能体归自定义");
        assert_eq!(inferred_source(None), "custom", "没给名字就是用户自己的东西");
    }

    #[test]
    fn format_ms_is_readable() {
        assert_eq!(format_ms(400), "400ms");
        assert_eq!(format_ms(12_000), "12s");
        assert_eq!(format_ms(300_000), "5m");
    }
}
