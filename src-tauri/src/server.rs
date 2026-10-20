use crate::config;
use crate::notify::Notifier;
use crate::spool;
use crate::state::{state_ttl, Report, Signal, Snapshot, Store};
use axum::{
    extract::{rejection::JsonRejection, Path, Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 首选端口。被占用就退到系统分配，两种情况下都把实际端口写进 `~/.signalhub/port`。
const PREFERRED_PORT: u16 = 41320;

#[derive(Clone)]
struct Ctx {
    store: Arc<Mutex<Store>>,
    notify: Notifier,
    /// 上报路径要读 `group_by`（兜底分组）。和 `store` 一样是**共享的同一份**，
    /// 不是启动时的快照 —— 用户在设置面板里一改开关，下一次上报就得生效。
    settings: crate::SharedSettings,
    /// 每个信号源「最近一次上报」的凭证。接入页靠它回答「到底有没有在报」。
    receipts: Arc<crate::receipts::Receipts>,
    /// 实际绑到哪了（`true` = 0.0.0.0 / 局域网）。**HTTP 式 hook 那条免鉴权的端点
    /// 靠它决定开不开**：绑到回环时本地进程本来就能读 token 文件，多一个只写状态的
    /// 端点不改变信任边界；绑到局域网时它就是给全网开的一扇门。
    bind: crate::SharedBind,
    /// 当前界面语言。HTTP 式 hook 的「正在做什么」那行由应用自己生成。
    lang: Arc<crate::lang::LocalLang>,
    /// 等用户点的那些审批（见 `approvals.rs`）。**和 `store` 一样是共享的同一份** ——
    /// 灯上那个气泡读的就是它。
    approvals: Arc<crate::approvals::Registry>,
    token: String,
    started: Instant,
}

/// 上报 / 审批路径要用的那几样「共享的同一份」。
///
/// 收成一个结构而不是八个参数：**它们必须都是 app 里 manage 的那几份**，参数一多
/// 就容易在调用点按位置传错（类型还都长得差不多），而传错的后果是「上报绕过聚合 /
/// 审批看不到灯上的表」这类静默的错。按名字打包之后，调用点读起来也是自证的。
pub struct Shared {
    pub store: Arc<Mutex<Store>>,
    /// 上报路径要读 `group_by`（兜底分组），所以设置也共享给它，而不是让它自己
    /// 从盘上再读一份 —— 两份真值迟早会漂移。
    pub settings: crate::SharedSettings,
    /// 实际绑到哪了，回填给界面判断「那个开关生效了没有」
    pub bind: crate::SharedBind,
    /// 每个信号源的最近一次上报 —— 和上报路径共用同一份
    pub receipts: Arc<crate::receipts::Receipts>,
    /// 当前界面语言：HTTP 式 hook 要用它生成「正在做什么」那行
    pub lang: Arc<crate::lang::LocalLang>,
    /// 审批端点和灯上那个气泡共用同一份登记表
    pub approvals: Arc<crate::approvals::Registry>,
}

/// 绑定并开始服务。返回后不阻塞调用方。
///
/// 接收 `Notifier` 而不是 `AppHandle`：**必须用 app 里 manage 的那个组合版
/// 通知器**，它除了推事件给前端，还要核对窗口尺寸、记统计、发通知和转发。
/// 这里若自己构造一个（早先就是这么写的），HTTP 上报这条路径就会绕过
/// 那一整套逻辑 —— 而智能体的 hook 恰好全部走这条路。
pub fn spawn(notify: Notifier, shared: Shared, token: String) {
    // 监听地址**在启动时定一次**。设置里那个开关改了要重启才生效 —— 理由见
    // `Settings::lan_access`：运行中重绑会让端口变化，表现为「灯突然收不到上报」。
    let lan = shared
        .settings
        .lock()
        .expect("settings poisoned")
        .lan_access;
    let host = bind_host(lan);

    let ctx = Ctx {
        store: shared.store,
        notify,
        settings: shared.settings,
        receipts: shared.receipts,
        // 记「实际绑到哪了」和给 handler 看的是**同一份**：handler 要按它决定
        // 免鉴权那条端点开不开，两处各存一个就会有一边撒谎
        bind: shared.bind.clone(),
        lang: shared.lang,
        approvals: shared.approvals,
        token,
        started: Instant::now(),
    };

    tauri::async_runtime::spawn(async move {
        let listener = match tokio::net::TcpListener::bind((host, PREFERRED_PORT)).await {
            Ok(listener) => listener,
            Err(err) => {
                eprintln!(
                    "[signalhub] 端口 {PREFERRED_PORT} 不可用（{err}），改用系统分配端口"
                );
                tokio::net::TcpListener::bind((host, 0))
                    .await
                    .expect("无法绑定本地端口")
            }
        };

        let port = listener.local_addr().expect("无法读取监听地址").port();
        if let Err(err) = config::write_port(port) {
            eprintln!("[signalhub] 写入端口文件失败: {err}（CLI 将无法自动发现端口）");
        }
        // 记下「实际绑到哪了」。**在真的 bind 成功之后记** —— 界面上那句
        // 「重启才生效」靠它判断，记早了会说谎。
        *shared.bind.lock().expect("bind poisoned") = lan;
        if lan {
            // 这条要显眼：开了局域网访问之后，网内任何人拿到 token 就能上报
            match local_ip() {
                Some(ip) => println!("[signalhub] HTTP 服务监听 0.0.0.0:{port}（局域网 {ip}:{port}）"),
                None => println!("[signalhub] HTTP 服务监听 0.0.0.0:{port}（已暴露到局域网）"),
            }
        } else {
            println!("[signalhub] HTTP 服务监听 127.0.0.1:{port}");
        }

        // ---- 降级通道的补写（见 `spool`）----
        //
        // 位置是刻意的：**在 `axum::serve` 之前**。上面那行已经把端口文件写好了，
        // 所以此刻可能有 CLI 正连上来、连接被内核放进 backlog 里等着 —— 但它们
        // **一条都还没被处理**（`serve` 还没开始 poll）。于是「先把攒下的补上、
        // 再处理新的」这个顺序是被保证的：补的每一条都比新来的旧。
        //
        // 补完才 `serve`，也就不会出现「新的先进状态机、旧的后进」（那会让灯的
        // 状态往回跳）。代价是启动时多了几十毫秒 —— 上限 200 条，可以接受。
        if let Some(path) = spool::path() {
            let stats = replay_spool(&ctx, &path);
            if stats.replayed > 0 || stats.expired > 0 || stats.bad > 0 {
                // 补了东西（或者丢了东西）就要留痕：用户唯一能查的地方就是这份日志
                config::append_hook_log(&format!(
                    "{} [spool] 重放 {} 条，丢弃 {} 条（过期）/ {} 条（读不出）\n",
                    config::utc_hms(),
                    stats.replayed,
                    stats.expired,
                    stats.bad,
                ));
            }
        }

        if let Err(err) = axum::serve(listener, router(ctx)).await {
            eprintln!("[signalhub] HTTP 服务退出: {err}");
        }
    });
}

/// 该绑哪个地址。
///
/// 摘成纯函数是为了能单测 —— 这是整个「局域网访问」里**唯一真正决定安全模型**的
/// 一行：绑 `127.0.0.1` 时别的机器连不上，绑 `0.0.0.0` 时网内任何人都能连。
/// 写成内联的三元表达式就只能靠跑起来看。
fn bind_host(lan: bool) -> &'static str {
    if lan { "0.0.0.0" } else { "127.0.0.1" }
}

/// 本机在局域网里的地址，取不到就 `None`。
///
/// 用「连一个 UDP socket 再看自己的 local_addr」这个经典办法：UDP 的 `connect`
/// **不发任何包**，只做一次路由查找，所以不引入依赖、也不产生流量。查不到路由
/// （完全离线、只有回环）时会失败，那时返回 `None` —— 这个值只是用来显示给人看，
/// 取不到就不显示，不该因此报错。
pub fn local_ip() -> Option<String> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    // 8.8.8.8 只是个「挑一条出网路由」的靶子，不会真的发东西过去
    socket.connect("8.8.8.8:80").ok()?;
    let ip = socket.local_addr().ok()?.ip();
    // 万一查出回环（没有别的路由时可能），那对局域网没有意义
    (!ip.is_loopback()).then(|| ip.to_string())
}

/// 只读 Web 面板（`GET /`）：手机 / 平板上打开一个链接就能看灯。
///
/// **默认关**（设置里的 `web_panel`），关着的时候这个路由不存在（404）—— 它是一条
/// **新的对外读取面**，而灯是私人的东西（哪个项目在跑、卡在哪一步），要看得见
/// 得由用户明确打开。
///
/// 关着时给一句能照做的话，而不是一个光秃秃的 404：404 只说明「没有」，不说明
/// 「怎么有」。
async fn web_panel(State(ctx): State<Ctx>) -> Response {
    let settings = ctx.settings.lock().expect("settings poisoned").clone();
    if !settings.web_panel {
        return (
            StatusCode::NOT_FOUND,
            "只读 Web 面板没开。设置 → 接入 → 只读 Web 面板\n",
        )
            .into_response();
    }

    let html = crate::panel::page(&settings, ctx.lang.get());
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    )
        .into_response()
}

/// 只有 health 和 HTTP 式 hook **免鉴权**，其余整组挂上中间件。
///
/// 免鉴权的那两个各自的理由见 [`hook_report`] 的注释 —— 尤其是第二条：它只在绑到
/// 回环时可用，所以「局域网访问」打开时它自己会关掉。
fn router(ctx: Ctx) -> Router {
    let protected = Router::new()
        .route("/v1/state", get(read_state).post(report_state))
        .route("/v1/sessions", get(list_sessions))
        .route("/v1/sessions/{session}", delete(clear_session))
        .route("/v1/sessions/{session}/state", post(report_state_at_path))
        .route("/v1/monitor", post(set_monitor))
        .route_layer(axum::middleware::from_fn_with_state(
            ctx.clone(),
            require_auth,
        ));

    // HTTP 式 hook 的落点。**body 限长**：这个端点不需要 token，任何本地进程都能
    // POST 进来，而 axum 的默认上限（2MB）在这里没有理由 —— hook 的 payload 里带着
    // 工具输入，几 KB 到几十 KB，256KB 足够放下任何真实事件。
    let hooks = Router::new()
        .route("/v1/hooks/{agent}", post(hook_report))
        // 免鉴权意味着「谁都能打」，但**浏览器里的网页不能算谁** —— 见下面那条
        // 中间件的说明。层顺序无所谓：两道都是只看请求头的纯判断。
        .layer(axum::middleware::from_fn(reject_browser_requests))
        .layer(axum::extract::DefaultBodyLimit::max(HOOK_BODY_MAX));

    // 审批端点（C3）。**在 `protected` 那一组里** —— 它要 token，这是它和上报端点
    // 最重要的区别（见 `approve_request` 的注释）。body 限长和上报端点同一把尺子：
    // payload 是同一个东西。
    let approvals = Router::new()
        .route("/v1/approve/{agent}", post(approve_request))
        .route("/v1/approvals", get(approval_list))
        .route("/v1/approvals/{id}", post(approval_decide))
        .layer(axum::extract::DefaultBodyLimit::max(HOOK_BODY_MAX))
        // **鉴权中间件必须挂在这一组上**：它是独立的一个 Router，不会继承
        // `protected` 那一层。少写这一行就是「任何本地进程都能替用户点允许」——
        // 而这正是 C3 那条红线要挡住的事。
        .route_layer(axum::middleware::from_fn_with_state(
            ctx.clone(),
            require_auth,
        ));

    Router::new()
        .route("/v1/health", get(health))
        // 只读 Web 面板。**免鉴权**：页面里没有任何数据，数据由页面自己去拉
        // `/v1/state`（那条路要 token）。见 `panel.rs`。
        .route("/", get(web_panel))
        .merge(hooks)
        .merge(approvals)
        .merge(protected)
        .with_state(ctx)
}

// ---- 鉴权 ------------------------------------------------------------------

async fn require_auth(
    State(ctx): State<Ctx>,
    request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    // 浏览器发起的请求一定带 Origin，直接拒绝。这挡住的是「你正在浏览的某个
    // 网页偷偷 POST 到你本地端口」——本地 HTTP 服务最现实的攻击面。
    if request.headers().contains_key(header::ORIGIN) {
        return Err(ApiError::unauthorized("拒绝携带 Origin 的请求"));
    }

    let presented = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));

    match presented {
        Some(token) if token_matches(token, &ctx.token) => Ok(next.run(request).await),
        _ => Err(ApiError::unauthorized("缺少或错误的 Bearer token")),
    }
}

/// 定长比较，不短路。长度不同直接返回 false——长度不是秘密。
fn token_matches(presented: &str, expected: &str) -> bool {
    let (presented, expected) = (presented.as_bytes(), expected.as_bytes());
    if presented.len() != expected.len() {
        return false;
    }
    presented
        .iter()
        .zip(expected)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

/// 挡住浏览器发起的请求。**只用在免鉴权的 HTTP 式 hook 端点上。**
///
/// 那个端点为什么不能要 token，见 [`hook_report`]；代价是它「谁都能打」，本机
/// 进程打它本来也是设计的一部分。但要挡住**你正在浏览的网页**：跨站 POST（包括
/// `Content-Type: text/plain` 那种绕过预检的简单请求）浏览器一定会带 `Origin`
/// 头，而真实的 hook 客户端不是浏览器，不会带。于是「带 Origin 的一律 403」就
/// 足够把那条路封死（issue #10）。
///
/// **刻意不加「必须 application/json」**：那一条能把预检也逼出来，看起来更严，
/// 但对方是不是真的设置了 content-type 我们控制不了 —— Claude Code 的 HTTP
/// hook 如果按 `fetch(url, { body: JSON.stringify(...) })` 写，浏览器/undici
/// 默认给的是 `text/plain`。为了一道本来就由 Origin 覆盖的检查把接入弄哑，
/// 不值得。
async fn reject_browser_requests(request: Request, next: Next) -> Result<Response, ApiError> {
    if request.headers().contains_key(header::ORIGIN) {
        return Err(ApiError::forbidden("拒绝携带 Origin 的请求"));
    }
    Ok(next.run(request).await)
}

// ---- handlers --------------------------------------------------------------

async fn health(State(ctx): State<Ctx>) -> Json<Value> {
    Json(json!({
        "ok": true,
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_s": ctx.started.elapsed().as_secs(),
    }))
}

/// HTTP 式 hook 的 body 上限。见 [`router`] 里的说明。
const HOOK_BODY_MAX: usize = 256 * 1024;

/// HTTP 式接入的落点：**智能体的进程直接 POST 过来**（装出来的配置见
/// `hooks::install_http`，目前只给 Claude Code 用）。
///
/// 它和命令式接入的差别只有一个：不需要一个「能跑起来的命令」。于是**整类
/// 「命令不在 PATH 上 / 应用被挪走了」的失败都不存在** —— 那是接入页红色提示里
/// 最常见的那一条。
///
/// 三条硬约束（见 `docs/plan-round4.md` §9）：
///
/// 1. **不需要 token，但只在绑回环时可用。** 这个端点不能要 token：Claude Code 的
///    HTTP hook 配置写在用户的 `settings.json` 里（那是个会被 commit 的文件，而
///    token 是每台机器一份的秘密）。代之以两条防线：**绑到 0.0.0.0 时一律 403**
///    （那时它就是给全网开的一扇门），以及上面的 body 限长。
/// 2. **绝不阻塞、绝不把错误抛给智能体。** 任何情况都回 200 `{}`：这是上报端点，
///    我们没有任何决定要交给它。装上时的 `onFailure: "continue"` +
///    `timeout: 5` 只是第二道保险（连不上时对方也不该卡住）。
/// 3. **只写状态。** payload 只用来映射事件、拿会话名和 cwd；里面的工具输入不会
///    被存下来（`message` 只取一个短词，见 `hookmap::tool_target`）。
async fn hook_report(
    State(ctx): State<Ctx>,
    Path(agent): Path<String>,
    body: axum::body::Bytes,
) -> Result<Json<Value>, ApiError> {
    if *ctx.bind.lock().expect("bind poisoned") {
        // 开了局域网访问。这里**刻意不静默工作**：让请求方拿到明确的 403，接不上的
        // 人至少能看出是「端点关着」，而不是「什么都没发生」。
        return Err(ApiError::forbidden(
            "hook endpoint disabled while LAN access is on",
        ));
    }

    let event = match serde_json::from_slice::<Value>(&body) {
        Ok(payload) => apply_hook(&ctx, &agent, &payload),
        Err(err) => {
            // 早返回：这一条已经有自己的日志了，落到下面会把「读不出 JSON」再记成
            // 「unknown agent」—— 两条互相矛盾的记录比没有记录更坏
            log_http_hook(&agent, "?", "?", &format!("body 不是 JSON: {err}"));
            return Ok(Json(json!({})));
        }
    };

    match event {
        Some((name, session, outcome)) => log_http_hook(&agent, &name, &session, &outcome),
        // 不明智能体（路径写错了）：留痕，让人能自己看出来
        None => log_http_hook(&agent, "?", "?", "unknown agent"),
    }
    // **永远回 200 空对象**：这是上报端点，没有任何决定要交给智能体；报错只会在
    // 对方的界面上留下一行属于我们的故障。上面那个 403 是唯一的例外，而且它只
    // 发生在端点整个关着的时候（那种情况下请求方本来就该知道）。
    Ok(Json(json!({})))
}

/// 把一条 hook payload 送进状态机。返回 `(事件名, 会话名, 结果)` 供记日志 ——
/// **日志和副作用都在这一个函数里决定**，免得两个地方各判一次「这个事件认不认得」。
fn apply_hook(ctx: &Ctx, agent: &str, payload: &Value) -> Option<(String, String, String)> {
    // 不认识的智能体名：什么也不做。`map_event` 本来就只认那五家，这里提前返回
    // 是为了在日志里区分「名字写错了」和「这个事件我们明确忽略」。
    if !crate::hooks::ALL_NAMES.contains(&agent) {
        return None;
    }
    let name = crate::hookmap::event_name(payload).unwrap_or("?").to_string();
    let session = crate::hookmap::session_id(payload).unwrap_or_else(|| agent.to_string());

    let Some(action) = crate::hookmap::map_event(agent, payload) else {
        // 明确忽略的事件：一次状态都不改，但日志里要留痕（和命令式那条路一样）
        return Some((name, session, "ignored".to_string()));
    };

    match action {
        crate::hookmap::Action::Clear => {
            let (_, changed, snapshot) = drop_session(ctx, &session);
            if changed {
                (ctx.notify)(&snapshot);
            }
            Some((name, session, "clear".to_string()))
        }
        crate::hookmap::Action::Report(state) => {
            let report = Report {
                state: Signal::parse(state).ok()?,
                session: Some(session.clone()),
                // 组不填：交给 `ingest_core` 按用户的兜底设置解析（按目录 / 按会话），
                // 和 HTTP 上报完全同一条路
                group: None,
                agent: Some(agent.to_string()),
                // **走 hook 报的一律是智能体**（这个端点是给装了 hook 的智能体用的），
                // 所以不用上报方声明 —— 这里比 CLI 更清楚它是什么。
                source: Some("agent".to_string()),
                cwd: crate::hookmap::cwd_of(payload),
                message: hook_message(ctx, payload),
                ttl_ms: None,
                pid: None,
            };
            ingest(ctx, report, None);
            Some((name, session, state.to_string()))
        }
    }
}

/// HTTP 式 hook 的「正在做什么」那一行。
///
/// 命令式的同一句话由 CLI 生成（`bin/signalhub.rs` 的 `tool_summary`），词表在
/// `lang.rs` —— **和 CLI 那套词必须一致**，否则同一个工具在两种接入方式下会显示成
/// 两个说法（见 `hookmap::Verb` 的注释）。
fn hook_message(ctx: &Ctx, payload: &Value) -> Option<String> {
    let tool = payload["tool_name"].as_str()?.trim();
    if tool.is_empty() {
        return None;
    }
    let lang = ctx.lang.get();
    match (
        crate::hookmap::tool_verb(tool),
        crate::hookmap::tool_target(&payload["tool_input"]),
    ) {
        (Some(verb), Some(target)) => Some(crate::lang::tool_doing(lang, verb, &target)),
        (Some(verb), None) => Some(crate::lang::tool_verb(lang, verb).to_string()),
        // 认不出来就把工具原名照实写出来（和 CLI 一样：认不出来比认错好）
        (None, _) => Some(crate::hookmap::clip(tool, crate::hookmap::TOOL_NAME_MAX)),
    }
}

/// 给 HTTP 式 hook 记一行。**格式和命令式那条路完全一致** —— 「最近的 hook 调用」
/// 那张表得能同时显示两种接入方式的记录，否则 HTTP 式接入的人在自己的日志里
/// 什么也看不到。末尾多一个 `(http)` 只是为了让人一眼分清是谁写的那一行。
fn log_http_hook(agent: &str, event: &str, session: &str, outcome: &str) {
    config::append_hook_log(&format!(
        "{}  {agent:<12} {event:<20} session={session:<38} {outcome} (http)\n",
        config::utc_hms()
    ));
}

// ---- 审批（C3，`approvals.rs`）------------------------------------------------

/// 一个待批准条目的短 id：6 字节随机 → 12 位十六进制。够短，能在电话里念出来。
fn new_approval_id() -> String {
    let mut bytes = [0u8; 6];
    if getrandom::fill(&mut bytes).is_err() {
        // 取不到随机数也不该挡住审批：退回按时间取。登记表按会话去重，本来也堆不了几条，
        // 同一微秒内撞上两条的概率可以忽略。
        let micros = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_micros() as u64)
            .unwrap_or(0);
        return format!("{micros:012x}");
    }
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// 审批端点：Claude Code 的 `PermissionRequest` hook 打到这里，**这一条会等到有人点**。
///
/// 和上报端点（`/v1/hooks/{agent}`）分开是刻意的，而且是安全要求：
///
/// - **要 token。** 上报端点不能要（那条配置写在用户的 Claude Code 配置里，而那个
///   文件不该放秘密）；这一条替用户做决定，所以必须有凭证 —— 没有 token 的本地进程
///   不该能替用户点「允许」。token 由 `hooks::install_approve` 写进配置。
/// - **默认关。** 关着的时候照常回 200 但**什么都不说**（`{}`），Claude Code 走自己
///   的询问流程：装了 hook 也不改变任何行为。
/// - **只认「它本来就要问你」的那一刻**（`PermissionRequest`）。`PreToolUse` 会拦下
///   本来不需要批准的事，那是替用户做决定，不是把确认框搬个地方。
/// - **出任何问题都不报错给智能体**：回 `{}`。报错只会在对方界面上留一行属于我们的
///   故障，而它自己那个询问框本来就是对的答案。
async fn approve_request(
    State(ctx): State<Ctx>,
    Path(agent): Path<String>,
    body: axum::body::Bytes,
) -> Json<Value> {
    let payload = serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null);
    let event = payload["hook_event_name"].as_str().unwrap_or("?").to_string();
    let session = crate::hookmap::session_id(&payload).unwrap_or_else(|| agent.clone());

    // 局域网访问打开时和上报端点一样整条关掉：这一条**替用户做决定**，比只写状态
    // 严重得多。要 token 是第二道防线（见 `router` 里那一层）。
    if *ctx.bind.lock().expect("bind poisoned") {
        log_http_hook(&agent, &event, &session, "approve: 局域网访问开着，端点关着");
        return Json(crate::approvals::response(None));
    }

    let enabled = ctx.settings.lock().expect("settings poisoned").approve_tools;
    if !enabled || agent != "claude-code" || event != "PermissionRequest" {
        let why = if !enabled {
            "approve: off"
        } else {
            "approve: 不是 PermissionRequest，不管"
        };
        log_http_hook(&agent, &event, &session, why);
        return Json(crate::approvals::response(None));
    }

    let tool = payload["tool_name"].as_str().unwrap_or("").trim().to_string();
    let input = &payload["tool_input"];
    let (detail, truncated) = crate::hookmap::approval_detail(&tool, input);
    let target = crate::hookmap::tool_target(input)
        .unwrap_or_else(|| crate::hookmap::clip(&tool, crate::hookmap::TOOL_NAME_MAX));
    let id = new_approval_id();
    let pending = crate::approvals::Pending::new(
        id.clone(),
        session.clone(),
        tool,
        target,
        detail,
        truncated,
    );

    // 灯上先变黄 —— 走的是和上报完全相同的那条路（凭证、兜底分组、推送都跟着来）。
    // 这条上报**只带那个短词**（`hook_message`），命令全文只进本地界面。
    ingest(
        &ctx,
        Report {
            state: Signal::Permission,
            session: Some(session.clone()),
            group: None,
            agent: Some(agent.clone()),
            source: Some("agent".to_string()),
            cwd: crate::hookmap::cwd_of(&payload),
            message: hook_message(&ctx, &payload),
            ttl_ms: None,
            pid: None,
        },
        None,
    );

    let rx = ctx.approvals.open(pending);
    log_http_hook(&agent, &event, &session, "approve: 等你点");
    let decision = crate::approvals::wait(rx).await;
    if decision.is_none() {
        // 没人点，这一条到此为止。**按 id 收**：同一个会话可能刚又来了一条（新的
        // 顶掉旧的），按会话收会把那条新的也带走。
        // （请求方中途断开那条路不经过这里 —— 那种条目由 `Registry::list` 顺手扫掉。）
        ctx.approvals.expire(&id);
    }

    // 点完之后把牌子从「等你」改回「在干活」：无论允许还是拒绝，智能体都已经往下走了。
    // 后面的事件（PostToolUse 之类）会把它修正成更准确的状态。
    if decision.is_some() {
        ingest(
            &ctx,
            Report {
                state: Signal::Working,
                session: Some(session.clone()),
                group: None,
                agent: Some(agent.clone()),
                source: Some("agent".to_string()),
                cwd: None,
                message: None,
                ttl_ms: None,
                pid: None,
            },
            None,
        );
    }
    let outcome = match decision {
        Some(crate::approvals::Decision::Allow) => "approve: 允许",
        Some(crate::approvals::Decision::Deny) => "approve: 拒绝",
        None => "approve: 没人点，交回 Claude Code 自己问",
    };
    log_http_hook(&agent, &event, &session, outcome);
    Json(crate::approvals::response(decision))
}

/// 现在有几条在等。灯上那个气泡读它（IPC 走同一个函数）。
async fn approval_list(State(ctx): State<Ctx>) -> Json<Value> {
    Json(json!({ "approvals": ctx.approvals.list() }))
}

/// 从接口上替用户点一下 —— **和灯上那两个按钮走的是同一个函数**（`Registry::decide`）。
///
/// 存在这条路的理由很实际：它是这个功能唯一能被端到端验证的入口（灯上的按钮没法
/// 自动化点）。它和 `/v1/state` 在同一个 token 后面，所以不扩大信任边界；
/// 「看不全不给放行」那条规则在登记表里挡着，这条接口绕不过去。
async fn approval_decide(
    State(ctx): State<Ctx>,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Json<Value> {
    let payload = serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null);
    let decision = match payload["decision"].as_str() {
        Some("allow") => crate::approvals::Decision::Allow,
        Some("deny") => crate::approvals::Decision::Deny,
        other => {
            return Json(json!({
                "ok": false,
                "error": format!("decision 只能是 allow 或 deny，收到 {other:?}"),
            }));
        }
    };
    match ctx.approvals.decide(&id, decision) {
        crate::approvals::Decided::Done => Json(json!({ "ok": true })),
        crate::approvals::Decided::NotFound => {
            Json(json!({ "ok": false, "error": "没有这一条（超时了，或者被同一个会话的新请求顶掉了）" }))
        }
        crate::approvals::Decided::NotAllowable => Json(json!({
            "ok": false,
            "error": "这条的内容在灯上看不全，只能拒绝 —— 要放行请回终端里看",
        })),
    }
}

async fn read_state(State(ctx): State<Ctx>) -> Json<Snapshot> {
    Json(ctx.store.lock().expect("state store poisoned").snapshot())
}

async fn list_sessions(State(ctx): State<Ctx>) -> Json<Value> {
    let snapshot = ctx.store.lock().expect("state store poisoned").snapshot();
    let sessions: Vec<_> = snapshot.all_sessions().cloned().collect();
    Json(json!({ "sessions": sessions }))
}

async fn report_state(
    State(ctx): State<Ctx>,
    body: Result<Json<Report>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    apply(&ctx, None, parse_body(body)?)
}

async fn report_state_at_path(
    State(ctx): State<Ctx>,
    Path(session): Path<String>,
    body: Result<Json<Report>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    // 路径里的 session 覆盖请求体里的，避免两处不一致时行为难以预测
    apply(&ctx, Some(session), parse_body(body)?)
}

async fn clear_session(State(ctx): State<Ctx>, Path(session): Path<String>) -> Json<Value> {
    let (removed, changed, snapshot) = drop_session(&ctx, &session);
    if changed {
        (ctx.notify)(&snapshot);
    }
    // 清除是幂等的：会话本来就不存在时返回 removed:false 而不是报错
    Json(json!({ "ok": true, "removed": removed, "aggregate": snapshot.aggregate }))
}

#[derive(Deserialize)]
struct MonitorRequest {
    paused: bool,
}

async fn set_monitor(
    State(ctx): State<Ctx>,
    body: Result<Json<MonitorRequest>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(request) = body.map_err(invalid_body)?;

    let (changed, snapshot) = {
        let mut guard = ctx.store.lock().expect("state store poisoned");
        let changed = guard.set_paused(request.paused, std::time::Instant::now());
        (changed, guard.snapshot())
    };
    if changed {
        (ctx.notify)(&snapshot);
    }
    Ok(Json(json!({ "ok": true, "paused": snapshot.paused })))
}

// ---- 共用 ------------------------------------------------------------------

fn apply(ctx: &Ctx, path_session: Option<String>, mut report: Report) -> Result<Json<Value>, ApiError> {
    if let Some(session) = path_session {
        report.session = Some(session);
    }
    let (changed, snapshot) = ingest(ctx, report, None);

    // 顺带把聚合结果回给调用方，这样一次 curl 就能看到全局状态
    Ok(Json(json!({
        "ok": true,
        "aggregate": snapshot.aggregate,
        "sessions": snapshot.all_sessions().count(),
        "groups": snapshot.groups.len(),
        "changed": changed,
    })))
}

/// 记一次上报，有变化就推前端。返回 `(是否变化, 快照)`。`ttl` 见 [`ingest_core`]。
fn ingest(ctx: &Ctx, report: Report, ttl: Option<Duration>) -> (bool, Snapshot) {
    let (changed, snapshot) = ingest_core(ctx, report, ttl);
    if changed {
        (ctx.notify)(&snapshot);
    }
    (changed, snapshot)
}

/// 记一次上报的**核心**：解析兜底分组 → 定停留时长 → 进状态机。
///
/// `ttl` 给 `Some` 时**跳过算法**直接用。正常上报不给（走三级优先），只有启动时
/// 的 spool 重放会给 —— 它要的是「从原本那一刻起算的剩余时长」，而不是从此刻
/// 起算的整段（见 [`replay_spool`]）。
///
/// 不推前端：调用方决定什么时候推。HTTP 是每条都推（改完就得看见），重放是攒完
/// 推一次（见 [`replay_spool`]）—— 但**聚合和分组解析只有这一份**，重放不会走
/// 到另一套逻辑上去。
fn ingest_core(ctx: &Ctx, mut report: Report, ttl: Option<Duration>) -> (bool, Snapshot) {
    // 先记凭证：**这里是上报的唯一入口**，所以 HTTP、CLI 钩子、spool 重放全都
    // 会留下痕迹（见 `receipts.rs`）。放在最前面是有意的 —— 哪怕后面这条上报
    // 因为某种原因没能改变状态，用户也该看到「它确实调过我们」，这正是接入页
    // 要回答的问题。
    if let Some(agent) = report.agent.as_deref() {
        ctx.receipts.record(agent, report.source_id(), report.state.name());
    }
    // 兜底分组。**放在这里，因为这是唯一的上报入口** —— `/v1/state` 和
    // `/v1/sessions/{id}/state` 都走它，所以下面那两个 handler 不可能漏掉一个。
    //
    // 做在服务端而不是「改钩子命令」有两个理由：钩子命令是**写死的字符串**，
    // 而按目录 / 按会话分组要的是每次调用各自的值，写进命令行就成了一个固定值，
    // 根本表达不了；而且改服务端就顺带覆盖了 curl / CI / 手写脚本，还不用去动
    // 用户自己的智能体配置文件。
    let session = report.session_id().to_string();
    // 分组的解析和停留时长的兜底**共用这一次加锁**：两者都要看设置，而下面马上要拿
    // 状态锁 —— 先设置、后状态，和 `persist_settings` 那边是同一个顺序（反过来就是死锁）。
    //
    // 兜底停留时长：显式 `ttl_ms` > 用户为这个状态设的 > 内置默认（见 `state_ttl`）。
    let (resolved, ttl) = {
        let settings = ctx.settings.lock().expect("settings poisoned");
        let resolved =
            settings.resolve_group(report.group.as_deref(), report.cwd.as_deref(), &session);
        let ttl = ttl.unwrap_or_else(|| state_ttl(report.state, report.ttl_ms, &settings));
        (resolved, ttl)
    };
    report.group = resolved;

    let mut guard = ctx.store.lock().expect("state store poisoned");
    let changed = guard.apply(Instant::now(), report, ttl);
    (changed, guard.snapshot())
}

/// 清掉一个会话（`DELETE /v1/sessions/{id}` 和重放共用）。
/// 返回 `(removed, changed, 快照)`；推前端由调用方决定。
fn drop_session(ctx: &Ctx, session: &str) -> (bool, bool, Snapshot) {
    let mut guard = ctx.store.lock().expect("state store poisoned");
    let (removed, changed) = guard.remove_session(session, Instant::now());
    let snapshot = guard.snapshot();
    drop(guard);
    // 会话没了，挂在它名下的审批也不再作数：对面收到「没有决定」，走它自己的询问流程
    // （智能体已经退出时，那一头本来也没人在等）。**放在这里**是因为它是服务端
    // 唯一一处「丢掉一个会话」的落点 —— hook 的 SessionEnd 和 HTTP 的 DELETE 都走它。
    ctx.approvals.forget_session(session);
    (removed, changed, snapshot)
}

/// 启动时把 `spool.jsonl` 里攒下的上报补进状态机，然后删掉它。
///
/// 为什么会攒下东西：app 没在跑（重启、退出、刚装完还没启动）时，智能体的 hook
/// 上报会**连接被拒**，那一条就永远丢了 —— 而 hook 在后台跑，用户根本不会发现。
/// CLI 于是把它写进这个文件（见 `spool`），这里补上。
///
/// 四条刻意的选择：
///
/// - **判据是连接本身，不是「端口文件在不在」**（那是 CLI 侧的事）。这边只看文件
///   里有没有东西 —— 进程崩过之后端口文件还在，那是常见的假阳性。
/// - 走**[`ingest_core`]，和 HTTP 上报同一条路**：分组解析、状态机、TTL 都不另写一套。
/// - **过了 TTL 的直接丢**。一条三小时前的 `working`（TTL 5 分钟）此刻补出来只能是
///   一块「沉默」的牌，而且还会再挂 30 分钟宽限期 —— 它确实过期了，就别复活它。
///   没过期的按「原本那一刻起算」给一个**缩短了的 TTL**，于是它消失的时刻和当初
///   正常上报时**完全一致**。
/// - **只在最后推一次前端**。逐条推的话，一次退出期间攒下的事件会让灯闪一阵子，
///   而且每一条都会过一遍通知 / 出站钩子的判断。攒完推一次拿到的是**最终**状态，
///   那才是用户真正该看到的。
fn replay_spool(ctx: &Ctx, path: &std::path::Path) -> ReplayStats {
    let (records, bad) = spool::read_all(path);
    let mut stats = ReplayStats { bad, ..ReplayStats::default() };
    if records.is_empty() && bad == 0 {
        return stats;
    }

    let now_ms = spool::now_ms();
    for record in records {
        // 记录里的时刻是**墙上时钟**（跨进程只能存这个），先折算成「过去了多久」，
        // 再拿它去缩 TTL。`saturating_sub`：时钟被往回调过时当作「刚刚发生」，
        // 宁可多显示一会儿，也不要算出负的时长。
        let age = Duration::from_millis(now_ms.saturating_sub(record.at_ms));
        match record.op {
            spool::SpoolOp::Clear { session } => {
                let (_, changed, _) = drop_session(ctx, &session);
                stats.changed |= changed;
                stats.replayed += 1;
            }
            spool::SpoolOp::Report { body } => {
                let Ok(report) = serde_json::from_value::<Report>(body) else {
                    // 请求体没能解析成 `Report`（状态名写错、或 JSON 形状不对）。
                    // 这和坏行是一回事，记下来跳过。
                    stats.bad += 1;
                    continue;
                };
                let ttl = {
                    let settings = ctx.settings.lock().expect("settings poisoned");
                    state_ttl(report.state, report.ttl_ms, &settings)
                };
                let Some(remaining) = ttl.checked_sub(age) else {
                    stats.expired += 1;
                    continue;
                };
                let (changed, _) = ingest_core(ctx, report, Some(remaining));
                stats.changed |= changed;
                stats.replayed += 1;
            }
        }
    }

    // 攒完推一次（见上面那第四条）。没推过的话，重放的效果只落在状态机里，
    // 灯上什么都不会变 —— 那就等于没重放。
    if stats.changed {
        let snapshot = ctx.store.lock().expect("state store poisoned").snapshot();
        (ctx.notify)(&snapshot);
    }
    spool::remove(path);
    stats
}

/// 一次重放的结果。写日志、写单测都要它。
#[derive(Debug, Default, PartialEq, Eq)]
struct ReplayStats {
    /// 真的应用上了的条数（含清除）
    replayed: usize,
    /// 因为过了 TTL 而丢掉的条数
    expired: usize,
    /// 读不出来的条数（坏行 + 解析不成 `Report` 的请求体）
    bad: usize,
    /// 是否真的改变了状态（决定要不要推前端）
    changed: bool,
}


/// axum 的 Json rejection 默认给 422，而 `state` 非法应当算 400（客户端写错了）。
fn parse_body(body: Result<Json<Report>, JsonRejection>) -> Result<Report, ApiError> {
    body.map(|Json(report)| report).map_err(invalid_body)
}

fn invalid_body(rejection: JsonRejection) -> ApiError {
    ApiError::bad_request(format!("请求体无效: {}", rejection.body_text()))
}

struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self { status: StatusCode::BAD_REQUEST, message: message.into() }
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self { status: StatusCode::UNAUTHORIZED, message: message.into() }
    }

    fn forbidden(message: impl Into<String>) -> Self {
        Self { status: StatusCode::FORBIDDEN, message: message.into() }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Signal;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// 监听地址是这一整个功能里**唯一真正决定安全模型**的一行。
    ///
    /// 绑回环时别的机器连不上（默认）；绑 `0.0.0.0` 时网内任何人都能连 —— 而那时
    /// 唯一的防线只剩 token。所以这两条各钉一个断言：将来谁把它改成「默认开」，
    /// 或者把两个分支写反，都会在这里红。
    #[test]
    fn the_bind_host_is_loopback_unless_lan_access_is_on() {
        assert_eq!(bind_host(false), "127.0.0.1", "默认必须只绑本机");
        assert_eq!(bind_host(true), "0.0.0.0");
        assert!(
            !crate::config::Settings::default().lan_access,
            "默认值必须是关的 —— 见 Settings::lan_access"
        );
    }

    struct TestServer {
        port: u16,
        token: String,
        notifications: Arc<AtomicUsize>,
        /// 服务器手里**那一份**设置。测试通过 `set_group_by` 改它，就是在模拟
        /// 用户在设置面板里拨开关 —— 走的是同一条共享路径，不是另造一个快照。
        settings: crate::SharedSettings,
        /// 和服务器共用的「实际绑到哪了」。测试用它模拟「局域网访问开着」。
        bind: crate::SharedBind,
        /// 和服务器共用的凭证表。上报会顺手往这里记一笔，所以测试能直接看它。
        receipts: Arc<crate::receipts::Receipts>,
        /// 和服务器共用的审批登记表（灯上那个气泡读的就是它）。
        approvals: Arc<crate::approvals::Registry>,
    }

    async fn start() -> TestServer {
        let (ctx, notifications, settings, bind, receipts) = test_ctx();
        // 先从 ctx 里取出来再把它交给 router（`Ctx` 不是 `Clone`）
        let approvals = ctx.approvals.clone();

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, router(ctx)).await.unwrap();
        });

        TestServer {
            port,
            token: "s3cret".to_string(),
            notifications,
            settings,
            bind,
            receipts,
            approvals,
        }
    }

    /// 一个不经 HTTP 的 ctx + 通知计数 + 那份共享设置。
    ///
    /// 重放那几条用例（`replay_spool`）不需要真的起服务器 —— 重放本来就不走网络，
    /// 它是「把盘上的东西补进状态机」。
    fn test_ctx() -> (
        Ctx,
        Arc<AtomicUsize>,
        crate::SharedSettings,
        crate::SharedBind,
        Arc<crate::receipts::Receipts>,
    ) {
        let notifications = Arc::new(AtomicUsize::new(0));
        let counter = notifications.clone();
        let settings: crate::SharedSettings =
            Arc::new(Mutex::new(Arc::new(crate::config::Settings::default())));
        // 回环（`false`）：HTTP 式 hook 那条端点默认开着，才测得到它
        let bind: crate::SharedBind = Arc::new(Mutex::new(false));

        let receipts = Arc::new(crate::receipts::Receipts::in_memory());
        let ctx = Ctx {
            store: Arc::new(Mutex::new(Store::default())),
            notify: Arc::new(move |_header: &Snapshot| {
                counter.fetch_add(1, Ordering::SeqCst);
            }),
            settings: settings.clone(),
            // 不落盘（`None`）：单测绝不能写开发者真实的 `~/.signalhub`。
            // 上报路径会顺手记凭证，所以这里必须真的给一个，不能用 `Default`。
            receipts: receipts.clone(),
            bind: bind.clone(),
            lang: Arc::new(crate::lang::LocalLang::default()),
            // 审批登记表：单测里不挂回调（`on_change` 的调用点是 `lib.rs`），
            // 所以这里和 `notify` 不一样 —— 没有 app 也能测
            approvals: Arc::new(crate::approvals::Registry::default()),
            token: "s3cret".to_string(),
            started: Instant::now(),
        };
        (ctx, notifications, settings, bind, receipts)
    }

    /// 上报里声明的 `source` **原样**记进凭证表：界面靠它把信号源分成
    /// 「AI 智能体 / 系统 / 自定义」三组，而凭证是那条分组的唯一来源。
    ///
    /// 这条走的是真的 HTTP（`/v1/state`）而不是直接调 `ingest_core`：CLI、curl、
    /// spool 重放全都从这一个入口进来，而「字段名拼错」正是这条链上最容易发生、
    /// 又最安静的一种错（服务端没有 `deny_unknown_fields`）。
    #[tokio::test]
    async fn the_declared_source_lands_in_the_receipt() {
        let server = start().await;
        let body = json!({
            "state": "working",
            "session": "build-watch",
            "agent": "build-watch",
            "source": "system",
        });
        let (status, _) = call(
            &server,
            "POST",
            "/v1/state",
            Some(&server.token),
            false,
            Some(body.to_string().as_str()),
        )
        .await;
        assert_eq!(status, 200);

        let receipts = server.receipts.view();
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0].agent, "build-watch");
        assert_eq!(receipts[0].source, "system");
    }

    /// 「自检」在 HTTP 式接入下走的是**真一次 HTTP**（`crate::probe_http`）。
    /// 这条用例同时钉住两件事：那条路真的能打通（不是只调了一遍 handler），
    /// 以及它**不需要 token** —— 装出来的配置里本来就没有 token。
    #[tokio::test]
    async fn the_self_check_probe_reaches_the_hook_endpoint() {
        let server = start().await;
        let payload = json!({
            "hook_event_name": "UserPromptSubmit",
            "session_id": "signalhub-probe",
        })
        .to_string();
        let url = format!("http://127.0.0.1:{}/v1/hooks/claude-code", server.port);

        let text = tokio::task::spawn_blocking(move || crate::probe_http(&url, &payload))
            .await
            .unwrap()
            .expect("探测应当成功");
        assert!(text.starts_with("HTTP 200"), "{text}");

        // 探测真的走完了整条路：状态进了状态机
        let (_, state) = call(&server, "GET", "/v1/state", Some(&server.token), false, None).await;
        assert!(state.contains("signalhub-probe"), "state: {state}");
    }

    /// 免鉴权**不等于**「谁都能替你改灯」：浏览器发起的跨站 POST 一律 403。
    ///
    /// 挡的是 issue #10 那条路 —— 你随便访问一个网页，它用 `text/plain` 的简单
    /// 请求（不触发预检）往本地端口 POST 一段 JSON，就能让灯显示「需要授权」并
    /// 弹一条系统通知。真实的 hook 客户端不是浏览器，不会带 `Origin`。
    #[tokio::test]
    async fn the_hook_endpoint_refuses_browser_requests() {
        let server = start().await;
        let body = json!({
            "hook_event_name": "PreToolUse",
            "session_id": "csrf-session",
            "tool_name": "Bash",
        })
        .to_string();

        let (status, _) =
            call(&server, "POST", "/v1/hooks/claude-code", None, true, Some(&body)).await;
        assert_eq!(status, 403, "带 Origin 的请求必须被拒");

        // 拒绝的是浏览器，不是这份 payload：上面那条要是漏了，这里会看到它
        let (_, state) = call(&server, "GET", "/v1/state", Some(&server.token), false, None).await;
        assert!(!state.contains("csrf-session"), "跨站请求改动了状态: {state}");

        // 同一个请求去掉 Origin 就能进 —— 证明 403 只因为那一颗头
        let (status, _) =
            call(&server, "POST", "/v1/hooks/claude-code", None, false, Some(&body)).await;
        assert_eq!(status, 200);
    }

    impl TestServer {
        /// 模拟用户在设置面板里拨「按目录 / 按会话分组」——改的是服务器手里那一份，
        /// 走的是同一条共享路径，不是另造一个快照。
        ///
        /// 设置是 `Arc<Mutex<Arc<Settings>>>`，所以这里换一个新的 `Arc`（和
        /// `persist_settings` 一样），而不是就地去改它指向的内容。
        fn set_group_by(&self, by: crate::config::GroupBy) {
            *self.settings.lock().unwrap() =
                Arc::new(crate::config::Settings { group_by: by, ..Default::default() });
        }

        /// 模拟「局域网访问开着」—— 启动时 `spawn` 就是在 bind 成功之后写这个值的。
        fn set_lan(&self, lan: bool) {
            *self.bind.lock().unwrap() = lan;
        }

        /// 拨「只读 Web 面板」那个开关（模拟用户在设置里打开它）。
        fn set_web_panel(&self, on: bool) {
            let current = self.settings.lock().unwrap().clone();
            *self.settings.lock().unwrap() =
                Arc::new(crate::config::Settings { web_panel: on, ..(*current).clone() });
        }
    }

    /// 面板**默认关**：没打开时这个路由不存在。
    ///
    /// 它是一条新的对外读取面，而灯是私人的东西（哪个项目在跑、卡在哪一步）——
    /// 「默认关」是红线，这条用例就是那道线的把手。
    #[tokio::test]
    async fn the_web_panel_is_off_by_default() {
        let server = start().await;
        let (status, _) = call(&server, "GET", "/", None, false, None).await;
        assert_eq!(status, 404);
    }

    /// 打开之后 `GET /` 给出整页，而且**不需要 token**。
    ///
    /// 不鉴权是有意的：页面里没有任何数据，数据由页面自己去拉 `/v1/state`
    /// （那条路要 token）。所以拿到这个空壳的人得不到任何信息。
    #[tokio::test]
    async fn the_web_panel_serves_the_page_when_switched_on() {
        let server = start().await;
        server.set_web_panel(true);

        // 整条响应都要看：**Content-Type 不对的话浏览器会把这页当纯文本显示**
        // （也就是把源码摊给用户看），所以它和正文一样是这条用例的一部分。
        let raw = call_raw(server.port, "GET", "/", None, false, None).await;
        assert!(raw.starts_with("HTTP/1.1 200"), "{}", &raw[..60.min(raw.len())]);
        assert!(raw.to_lowercase().contains("content-type: text/html"), "要声明成 HTML");
        let body = raw.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
        // 注进去的东西也在：状态名（和系统通知同一张表）+ 缺省配色的颜色
        assert!(body.contains("执行中"), "状态名没注进去");
        assert!(body.contains("#16a34a"), "颜色没注进去");
        assert!(!body.contains("__CONFIG__"), "占位符必须被换掉");
    }

    /// 面板的空壳是免费的，**数据不是** —— 这正是这个路由敢不鉴权的原因。
    ///
    /// 拿到页面但没拿到 token 的人，看到的只有一句「token 不对」；而页面里的 JS
    /// 去拉 `/v1/state` 时是**同源** GET，浏览器不会给它加 `Origin`，所以也不会
    /// 撞上「带 Origin 一律拒绝」那道防线。反过来，跨站的页面想借你的浏览器读灯，
    /// 一定带 `Origin` —— 那时两道防线（Origin、token）它都过不去。
    #[tokio::test]
    async fn the_panel_shell_is_free_but_the_state_is_not() {
        let server = start().await;
        server.set_web_panel(true);

        let (status, _) = call(&server, "GET", "/", None, false, None).await;
        assert_eq!(status, 200, "空壳不要 token");

        // 面板里那行 fetch 的形状：同源、无 Origin、带 token
        let (status, _) = call(&server, "GET", "/v1/state", Some(&server.token), false, None).await;
        assert_eq!(status, 200, "带 token 的同源 GET 要能过");

        // 没有 token 就只有 401（页面里的 JS 会据此提示「token 不对」）
        let (status, _) = call(&server, "GET", "/v1/state", None, false, None).await;
        assert_eq!(status, 401);

        // 跨站页面借浏览器来读：一定带 Origin，两道防线都过不去
        let (status, _) = call(&server, "GET", "/v1/state", Some(&server.token), true, None).await;
        assert_eq!(status, 401);
    }

    /// HTTP 式 hook：**不带 token** 就该能上报。它是这个端点存在的全部理由 ——
    /// 要求 token 就得把每台机器一份的秘密写进用户的 `settings.json`。
    #[tokio::test]
    async fn the_http_hook_reports_without_a_token() {
        let server = start().await;
        let payload = json!({
            "hook_event_name": "PreToolUse",
            "session_id": "probe-1",
            "cwd": "/proj/signalhub",
            "tool_name": "Bash",
            "tool_input": { "command": "npm test" },
        })
        .to_string();

        let (status, body) = call(
            &server,
            "POST",
            "/v1/hooks/claude-code",
            None,
            false,
            Some(&payload),
        )
        .await;
        assert_eq!(status, 200, "body: {body}");
        assert_eq!(body.trim(), "{}", "回给智能体的必须是空对象（没有决定要说）");

        // 状态真的进去了，而且**「正在做什么」那一行也在**（默认语言是中文，
        // 和命令式接入生成的是同一句话）
        let (_, state) = call(&server, "GET", "/v1/state", Some(&server.token), false, None).await;
        assert!(state.contains("working"), "state: {state}");
        assert!(state.contains("执行 npm"), "state: {state}");
    }

    /// **认不出来也绝不报错给智能体。** 这是上报端点，我们没有任何决定要交给它；
    /// 一个 4xx 会让 hook 在对方的界面上留下一行报错，而那是我们的问题，不是它的。
    #[tokio::test]
    async fn the_http_hook_answers_ok_to_everything_it_cannot_use() {
        let server = start().await;
        let cases = [
            // 事件名不认识（明确忽略）
            ("claude-code", json!({ "hook_event_name": "SomethingNew" }).to_string()),
            // payload 不合法
            ("claude-code", "{ 这不是 json".to_string()),
            // 路径里的智能体名写错了
            ("nope", json!({ "hook_event_name": "PreToolUse" }).to_string()),
            // 空的
            ("claude-code", String::new()),
        ];
        for (agent, body) in cases {
            let (status, text) = call(
                &server,
                "POST",
                &format!("/v1/hooks/{agent}"),
                None,
                false,
                Some(&body),
            )
            .await;
            assert_eq!(status, 200, "agent={agent} body={body} → {text}");
            assert_eq!(text.trim(), "{}", "agent={agent}");
        }

        // 一条状态都不该被写进去
        let (_, state) = call(&server, "GET", "/v1/state", Some(&server.token), false, None).await;
        assert!(!state.contains("working"), "state: {state}");
    }

    /// `SessionEnd` 是清除：会话没了，而不是留一块牌。
    #[tokio::test]
    async fn the_http_hook_clears_sessions() {
        let server = start().await;
        let working = json!({ "hook_event_name": "PreToolUse", "session_id": "s-9" }).to_string();
        call(&server, "POST", "/v1/hooks/claude-code", None, false, Some(&working)).await;

        let end = json!({ "hook_event_name": "SessionEnd", "session_id": "s-9" }).to_string();
        let (status, _) =
            call(&server, "POST", "/v1/hooks/claude-code", None, false, Some(&end)).await;
        assert_eq!(status, 200);

        let (_, sessions) =
            call(&server, "GET", "/v1/sessions", Some(&server.token), false, None).await;
        assert!(!sessions.contains("s-9"), "sessions: {sessions}");
    }

    /// **开了局域网访问就关掉这个端点。** 它不需要 token，绑到 0.0.0.0 时等于给
    /// 全网开了一扇能写状态的门；那时唯一诚实的做法是拒绝，并说明为什么。
    #[tokio::test]
    async fn the_http_hook_is_refused_while_lan_access_is_on() {
        let server = start().await;
        server.set_lan(true);

        let payload = json!({ "hook_event_name": "PreToolUse", "session_id": "s-1" }).to_string();
        let (status, body) = call(
            &server,
            "POST",
            "/v1/hooks/claude-code",
            None,
            false,
            Some(&payload),
        )
        .await;
        assert_eq!(status, 403, "body: {body}");
        assert!(body.contains("LAN"), "body: {body}");

        // 带 token 的常规上报不受影响（局域网模式下它照常工作）
        let with_token = json!({ "state": "working", "session": "s-1" }).to_string();
        let (status, _) = post_state(&server, &with_token).await;
        assert_eq!(status, 200);
    }

    /// 手写一个 HTTP/1.1 请求，就为了几个测试引入一个 HTTP 客户端依赖不划算。
    /// 返回**整条响应**（含响应头）—— 需要看 `Content-Type` 时用它。
    async fn call_raw(
        port: u16,
        method: &str,
        path: &str,
        token: Option<&str>,
        origin: bool,
        body: Option<&str>,
    ) -> String {
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();

        let mut request = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
        if let Some(token) = token {
            request.push_str(&format!("Authorization: Bearer {token}\r\n"));
        }
        if origin {
            request.push_str("Origin: http://evil.example\r\n");
        }
        if let Some(body) = body {
            request.push_str("Content-Type: application/json\r\n");
            request.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
        request.push_str("\r\n");
        if let Some(body) = body {
            request.push_str(body);
        }

        stream.write_all(request.as_bytes()).await.unwrap();
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).await.unwrap();
        String::from_utf8_lossy(&raw).into_owned()
    }

    /// 一段 HTTP 应答 → 状态码 + 正文。
    fn split_response(text: &str) -> (u16, String) {
        let status = text
            .split_whitespace()
            .nth(1)
            .and_then(|code| code.parse().ok())
            .unwrap_or(0);
        let body = text
            .split_once("\r\n\r\n")
            .map(|(_, b)| b.to_string())
            .unwrap_or_default();
        (status, body)
    }

    /// [`call_raw`] 的常用视图：状态码 + 正文。
    async fn call(
        server: &TestServer,
        method: &str,
        path: &str,
        token: Option<&str>,
        origin: bool,
        body: Option<&str>,
    ) -> (u16, String) {
        split_response(&call_raw(server.port, method, path, token, origin, body).await)
    }

    /// 只要端口的那种调用：审批那条会**一直挂着**，所以它必须能跑在 `tokio::spawn`
    /// 里，而那边借不到 `TestServer`。
    async fn call_at(
        port: u16,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Option<&str>,
    ) -> (u16, String) {
        split_response(&call_raw(port, method, path, token, false, body).await)
    }

    async fn post_state(server: &TestServer, body: &str) -> (u16, String) {
        call(server, "POST", "/v1/state", Some(&server.token), false, Some(body)).await
    }

    #[test]
    fn token_comparison_rejects_wrong_values() {
        assert!(token_matches("abc", "abc"));
        assert!(!token_matches("abc", "abd"));
        assert!(!token_matches("abc", "abcd"));
        assert!(!token_matches("", "abc"));
    }

    #[tokio::test]
    async fn health_needs_no_token() {
        let server = start().await;
        let (status, body) = call(&server, "GET", "/v1/health", None, false, None).await;
        assert_eq!(status, 200);
        assert!(body.contains("\"ok\":true"), "body: {body}");
    }

    #[tokio::test]
    async fn state_requires_a_token() {
        let server = start().await;
        let (status, _) = call(&server, "GET", "/v1/state", None, false, None).await;
        assert_eq!(status, 401);
    }

    #[tokio::test]
    async fn wrong_token_is_rejected() {
        let server = start().await;
        let (status, _) = call(&server, "GET", "/v1/state", Some("nope"), false, None).await;
        assert_eq!(status, 401);
    }

    /// 本地 HTTP 服务最现实的攻击面：网页带着 Origin 打过来
    #[tokio::test]
    async fn origin_header_is_rejected_even_with_a_valid_token() {
        let server = start().await;
        let (status, _) =
            call(&server, "GET", "/v1/state", Some(&server.token), true, None).await;
        assert_eq!(status, 401);
    }

    #[tokio::test]
    async fn reporting_updates_the_aggregate_and_notifies_once() {
        let server = start().await;

        let (status, body) = post_state(&server, r#"{"state":"working","session":"a"}"#).await;
        assert_eq!(status, 200);
        assert!(body.contains("\"aggregate\":\"working\""), "body: {body}");
        assert_eq!(server.notifications.load(Ordering::SeqCst), 1);

        let (status, _) = call(&server, "GET", "/v1/state", Some(&server.token), false, None).await;
        assert_eq!(status, 200);
    }

    #[tokio::test]
    async fn a_repeat_report_of_an_unchanged_state_does_not_notify() {
        let server = start().await;
        post_state(&server, r#"{"state":"working","session":"a"}"#).await;
        assert_eq!(server.notifications.load(Ordering::SeqCst), 1);

        post_state(&server, r#"{"state":"working","session":"a"}"#).await;
        assert_eq!(
            server.notifications.load(Ordering::SeqCst),
            1,
            "聚合没变就不该再推一次给前端"
        );
    }

    #[tokio::test]
    async fn an_unknown_state_is_a_400_not_a_422() {
        let server = start().await;
        let (status, body) = post_state(&server, r#"{"state":"nonsense"}"#).await;
        assert_eq!(status, 400, "body: {body}");
        assert!(body.contains("error"), "body: {body}");
    }

    #[tokio::test]
    async fn a_body_missing_state_is_a_400() {
        let server = start().await;
        let (status, _) = post_state(&server, r#"{}"#).await;
        assert_eq!(status, 400);
    }

    #[tokio::test]
    async fn state_aliases_are_accepted_over_http() {
        let server = start().await;
        let (status, body) = post_state(&server, r#"{"state":"blocked"}"#).await;
        assert_eq!(status, 200);
        assert!(body.contains("\"aggregate\":\"error\""), "body: {body}");
    }

    #[tokio::test]
    async fn the_path_session_overrides_the_body_session() {
        let server = start().await;
        let (status, _) = call(
            &server,
            "POST",
            "/v1/sessions/from-path/state",
            Some(&server.token),
            false,
            Some(r#"{"state":"permission","session":"from-body"}"#),
        )
        .await;
        assert_eq!(status, 200);

        let (_, body) = call(&server, "GET", "/v1/state", Some(&server.token), false, None).await;
        assert!(body.contains("from-path"), "body: {body}");
        assert!(!body.contains("from-body"), "body: {body}");
    }

    // ---- 兜底分组（group_by = cwd）----

    /// 开关**关着**时（默认），带 cwd 的上报仍然全归 `default` ——
    /// 现有用户打开新版本看到的东西必须一模一样。
    #[tokio::test]
    async fn without_the_switch_a_cwd_report_still_lands_in_default() {
        let server = start().await;
        post_state(&server, r#"{"state":"working","session":"a","cwd":"/proj/api"}"#).await;

        let (_, body) = call(&server, "GET", "/v1/state", Some(&server.token), false, None).await;
        assert!(body.contains(r#""name":"default""#), "body: {body}");
        // 会话视图里**照旧**带着 cwd（那是它自己的字段），这里要否掉的是
        // 「路径被拿来当组名了」这件事本身，所以匹配的是组名的位置。
        assert!(!body.contains(r#""name":"/proj/api""#), "body: {body}");
    }

    /// 开关打开后，两个不同目录的上报要变成**两块牌**。
    #[tokio::test]
    async fn cwd_mode_makes_one_sign_per_directory() {
        let server = start().await;
        server.set_group_by(crate::config::GroupBy::Cwd);

        post_state(&server, r#"{"state":"working","session":"a","cwd":"/proj/api"}"#).await;
        let (status, body) =
            post_state(&server, r#"{"state":"error","session":"b","cwd":"/proj/web"}"#).await;
        assert_eq!(status, 200);
        // 回给调用方的是**派生之后**的组数 —— 一次 curl 就能看出开关生效没有
        assert!(body.contains(r#""groups":2"#), "body: {body}");

        let (_, body) = call(&server, "GET", "/v1/state", Some(&server.token), false, None).await;
        assert!(body.contains(r#""name":"/proj/api""#), "body: {body}");
        assert!(body.contains(r#""name":"/proj/web""#), "body: {body}");
    }

    /// 按会话分组：**同一个目录里的两个会话也要各占一块牌**。
    ///
    /// 这是用户报的那个场景 —— 同一个智能体并行开了两个会话（两个窗口 / 两个
    /// worktree），进度各不相同，却挤在同一块牌上只看得到最紧急的那个。
    /// 注意 cwd 故意给成一样的：按目录分组在这个用例里**分不开它们**，
    /// 而按会话分得开，这正是两种模式的区别。
    #[tokio::test]
    async fn session_mode_gives_each_session_its_own_sign() {
        let server = start().await;
        server.set_group_by(crate::config::GroupBy::Session);

        post_state(
            &server,
            r#"{"state":"working","session":"sess-aaa","cwd":"/proj/same"}"#,
        )
        .await;
        let (status, body) = post_state(
            &server,
            r#"{"state":"error","session":"sess-bbb","cwd":"/proj/same"}"#,
        )
        .await;
        assert_eq!(status, 200);
        assert!(body.contains(r#""groups":2"#), "两个会话该是两块牌：{body}");

        let (_, body) = call(&server, "GET", "/v1/state", Some(&server.token), false, None).await;
        assert!(body.contains(r#""name":"sess-aaa""#), "body: {body}");
        assert!(body.contains(r#""name":"sess-bbb""#), "body: {body}");
        // 目录名不该成为组名 —— 那样两个会话又挤回一块了
        assert!(!body.contains(r#""name":"/proj/same""#), "body: {body}");
    }

    /// 上报自己带的 `group` 仍然优先 —— 开关是兜底，不是覆盖。
    #[tokio::test]
    async fn cwd_mode_does_not_override_an_explicit_group() {        let server = start().await;
        server.set_group_by(crate::config::GroupBy::Cwd);

        post_state(
            &server,
            r#"{"state":"working","session":"a","group":"work","cwd":"/proj/api"}"#,
        )
        .await;

        let (_, body) = call(&server, "GET", "/v1/state", Some(&server.token), false, None).await;
        assert!(body.contains(r#""name":"work""#), "body: {body}");
        assert!(!body.contains(r#""name":"/proj/api""#), "body: {body}");
    }

    /// 没带 cwd 的上报在 cwd 模式下也只该退回 `default` —— 不能造出空名字的组。
    #[tokio::test]
    async fn cwd_mode_still_falls_back_to_default_without_a_cwd() {
        let server = start().await;
        server.set_group_by(crate::config::GroupBy::Cwd);

        post_state(&server, r#"{"state":"idle","session":"a"}"#).await;

        let (_, body) = call(&server, "GET", "/v1/state", Some(&server.token), false, None).await;
        assert!(body.contains(r#""name":"default""#), "body: {body}");
    }

    #[tokio::test]
    async fn clearing_a_session_is_idempotent() {
        let server = start().await;
        post_state(&server, r#"{"state":"error","session":"a"}"#).await;

        let (status, body) = call(
            &server,
            "DELETE",
            "/v1/sessions/a",
            Some(&server.token),
            false,
            None,
        )
        .await;
        assert_eq!(status, 200);
        assert!(body.contains("\"removed\":true"), "body: {body}");
        // 清掉最后一个会话后就没人上报了，因此是 offline 而不是 idle
        assert!(body.contains("\"aggregate\":\"offline\""), "body: {body}");

        let (status, body) = call(
            &server,
            "DELETE",
            "/v1/sessions/a",
            Some(&server.token),
            false,
            None,
        )
        .await;
        assert_eq!(status, 200, "清除不存在的会话不该是错误");
        assert!(body.contains("\"removed\":false"), "body: {body}");
    }

    #[tokio::test]
    async fn pausing_shows_off_but_keeps_the_sessions() {
        let server = start().await;
        post_state(&server, r#"{"state":"working","session":"a"}"#).await;

        let (status, body) = call(
            &server,
            "POST",
            "/v1/monitor",
            Some(&server.token),
            false,
            Some(r#"{"paused":true}"#),
        )
        .await;
        assert_eq!(status, 200);
        assert!(body.contains("\"paused\":true"), "body: {body}");

        let (_, body) = call(&server, "GET", "/v1/state", Some(&server.token), false, None).await;
        assert!(body.contains("\"aggregate\":\"off\""), "body: {body}");
        assert!(body.contains("\"id\":\"a\""), "暂停不该丢掉会话: {body}");

        call(
            &server,
            "POST",
            "/v1/monitor",
            Some(&server.token),
            false,
            Some(r#"{"paused":false}"#),
        )
        .await;
        let (_, body) = call(&server, "GET", "/v1/state", Some(&server.token), false, None).await;
        assert!(
            body.contains("\"aggregate\":\"working\""),
            "恢复后应立刻回到真实状态: {body}"
        );
    }

    #[tokio::test]
    async fn decoding_the_state_endpoint_matches_the_documented_shape() {
        let server = start().await;
        post_state(&server, r#"{"state":"permission","session":"a","message":"需要授权"}"#).await;

        let (_, body) = call(&server, "GET", "/v1/state", Some(&server.token), false, None).await;
        let parsed: Value = serde_json::from_str(&body).unwrap();

        assert_eq!(parsed["aggregate"], "permission");
        assert_eq!(parsed["paused"], false);
        assert_eq!(parsed["message"], "需要授权");
        assert!(parsed["since_ms"].is_number());

        // 会话挂在组下面 —— 灯球上每组一块牌
        let group = &parsed["groups"][0];
        assert_eq!(group["name"], "default", "没指定组时归 default");
        assert_eq!(group["aggregate"], "permission");
        assert!(group["since_ms"].is_number(), "牌上的「已持续」用它");
        assert_eq!(group["sessions"][0]["id"], "a");
        assert_eq!(group["sessions"][0]["state"], "permission");
        assert!(group["sessions"][0]["age_ms"].is_number());
    }

    #[tokio::test]
    async fn sessions_can_be_split_into_groups() {
        let server = start().await;
        post_state(&server, r#"{"state":"working","session":"a","group":"work"}"#).await;
        post_state(&server, r#"{"state":"error","session":"b","group":"ci"}"#).await;

        let (_, body) = call(&server, "GET", "/v1/state", Some(&server.token), false, None).await;
        let parsed: Value = serde_json::from_str(&body).unwrap();

        assert_eq!(parsed["aggregate"], "error", "全局取各组里最高的");
        let names: Vec<&str> = parsed["groups"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| g["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["ci", "work"], "最紧急的组排在最前");
        assert_eq!(parsed["groups"][0]["aggregate"], "error");
        assert_eq!(parsed["groups"][1]["aggregate"], "working");
    }

    #[tokio::test]
    async fn an_unknown_path_is_a_404() {
        let server = start().await;
        let (status, _) = call(&server, "GET", "/v1/nope", Some(&server.token), false, None).await;
        assert_eq!(status, 404);
    }

    // ---- 降级通道的重放（见 `spool`）----------------------------------------

    /// 攒在盘上的记录补进状态机，然后**文件清掉** —— 不然下次启动会再补一遍。
    #[test]
    fn a_replay_applies_the_records_and_removes_the_file() {
        let (ctx, notifications, _settings, _bind, _receipts) = test_ctx();
        let path = spool::temp_path("spool.jsonl");
        spool::append_at(
            &path,
            &spool::Spooled {
                at_ms: spool::now_ms(),
                op: spool::SpoolOp::Report {
                    body: serde_json::json!({
                        "state": "permission",
                        "session": "a",
                        "group": "work",
                        "message": "需要授权",
                    }),
                },
            },
        );
        spool::append_at(
            &path,
            &spool::Spooled {
                at_ms: spool::now_ms(),
                op: spool::SpoolOp::Report {
                    body: serde_json::json!({ "state": "working", "session": "b" }),
                },
            },
        );
        spool::append_at(
            &path,
            &spool::Spooled {
                at_ms: spool::now_ms(),
                op: spool::SpoolOp::Clear { session: "b".into() },
            },
        );

        let stats = replay_spool(&ctx, &path);

        assert_eq!(stats.replayed, 3);
        assert_eq!(stats.expired, 0);
        assert_eq!(stats.bad, 0);
        assert!(stats.changed);
        assert!(!path.exists(), "补完就把文件删掉 —— 否则下次启动会再补一遍");

        let snapshot = ctx.store.lock().unwrap().snapshot();
        let sessions: Vec<&str> = snapshot
            .all_sessions()
            .map(|session| session.id.as_str())
            .collect();
        assert_eq!(sessions, vec!["a"], "b 报了又被清掉，最后不该留下");
        assert_eq!(snapshot.aggregate, Signal::Permission);
        // **只推一次**：逐条推的话，一次退出期间攒下的事件会让灯闪一阵子
        assert_eq!(notifications.load(Ordering::SeqCst), 1);
    }

    /// 过了 TTL 的记录**直接丢**，不要补出一块「沉默」的牌。
    #[test]
    fn a_replay_drops_records_that_already_expired() {
        let (ctx, notifications, _settings, _bind, _receipts) = test_ctx();
        let path = spool::temp_path("spool.jsonl");
        // `done` 的 TTL 是 30 秒，这条是五分钟前的 —— 它早就不该在灯上了
        spool::append_at(
            &path,
            &spool::Spooled {
                at_ms: spool::now_ms() - 5 * 60 * 1000,
                op: spool::SpoolOp::Report {
                    body: serde_json::json!({ "state": "done", "session": "a" }),
                },
            },
        );

        let stats = replay_spool(&ctx, &path);

        assert_eq!(stats.replayed, 0);
        assert_eq!(stats.expired, 1);
        assert!(!stats.changed, "什么都没进去，就不该推前端");
        assert_eq!(notifications.load(Ordering::SeqCst), 0);
        assert_eq!(ctx.store.lock().unwrap().snapshot().all_sessions().count(), 0);
    }

    /// 没到期的记录要按**「原本那一刻起算」**补，而不是从现在重新起算。
    ///
    /// 这条是重放的语义核心：一条四分钟前的 `working`（TTL 五分钟）补进来之后
    /// 应该**只剩一分钟**，而不是又拿到完整五分钟 —— 后者会让一块本该消失的牌
    /// 在灯上多待四分钟。
    #[test]
    fn a_replay_keeps_the_remaining_life_of_a_record() {
        let (ctx, _notifications, _settings, _bind, _receipts) = test_ctx();
        let path = spool::temp_path("spool.jsonl");
        spool::append_at(
            &path,
            &spool::Spooled {
                at_ms: spool::now_ms() - 4 * 60 * 1000,
                op: spool::SpoolOp::Report {
                    body: serde_json::json!({ "state": "working", "session": "a" }),
                },
            },
        );

        let stats = replay_spool(&ctx, &path);
        assert_eq!(stats.replayed, 1);

        let mut store = ctx.store.lock().unwrap();
        assert_eq!(store.snapshot().aggregate, Signal::Working);
        // 半分多钟后还活着（还剩一分钟），两分钟后就该转 stale 了
        assert!(!store.expire(Instant::now() + Duration::from_secs(20)));
        assert!(store.expire(Instant::now() + Duration::from_secs(90)));
    }

    /// 坏行跳过，好行照补 —— 文件会被手改、被截断、被交错写花。
    #[test]
    fn a_replay_skips_junk_without_losing_the_rest() {
        let (ctx, _notifications, _settings, _bind, _receipts) = test_ctx();
        let path = spool::temp_path("spool.jsonl");
        std::fs::write(
            &path,
            format!(
                "这不是 JSON\n{}\n{}\n",
                serde_json::json!({
                    "at_ms": spool::now_ms(),
                    "op": { "kind": "report", "body": { "state": "没有这个状态" } },
                }),
                serde_json::json!({
                    "at_ms": spool::now_ms(),
                    "op": { "kind": "report", "body": { "state": "attention", "session": "a" } },
                }),
            ),
        )
        .unwrap();

        let stats = replay_spool(&ctx, &path);

        assert_eq!(stats.replayed, 1, "最后那条好的要补上");
        assert_eq!(stats.bad, 2, "非 JSON 的行 + 状态名不合法的请求体");
        assert_eq!(ctx.store.lock().unwrap().snapshot().aggregate, Signal::Attention);
        assert!(!path.exists(), "读完了照样要清掉 —— 坏行留着只会每次都白读一遍");
    }

    // ---- 从灯上审批（C3）-----------------------------------------------------

    fn permission_request(event: &str, tool: &str, input: serde_json::Value) -> String {
        serde_json::json!({
            "hook_event_name": event,
            "session_id": "cc-1",
            "cwd": "/proj/signalhub",
            "tool_name": tool,
            "tool_input": input,
        })
        .to_string()
    }

    /// 拨开关走的是**服务器手里那一份共享设置**（和用户在设置面板里拨是同一个路径）。
    fn enable_approvals(settings: &crate::SharedSettings) {
        let mut guard = settings.lock().unwrap();
        let mut next = (**guard).clone();
        next.approve_tools = true;
        *guard = std::sync::Arc::new(next);
    }

    /// 等那条登记上（灯上那个气泡读的就是这个列表），返回它的 id。
    async fn wait_for_approval(server: &TestServer) -> String {
        for _ in 0..200 {
            let (_, body) = call(server, "GET", "/v1/approvals", Some(&server.token), false, None).await;
            let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
            if let Some(id) = parsed["approvals"][0]["id"].as_str() {
                return id.to_string();
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("审批一直没登记上");
    }

    /// **审批端点必须要有 token。** 它和上报端点正好相反（那一条刻意免鉴权：
    /// 它写在用户的 Claude Code 配置里，而 token 是每台机器一份的秘密）——
    /// 这一条替用户做决定，所以没有凭证的本地进程不该能点「允许」。
    #[tokio::test]
    async fn the_approval_endpoint_refuses_a_request_without_the_token() {
        let server = start().await;
        let body = permission_request(
            "PermissionRequest",
            "Bash",
            serde_json::json!({ "command": "rm -rf build" }),
        );
        let (status, _) = call(
            &server,
            "POST",
            "/v1/approve/claude-code",
            None,
            false,
            Some(&body),
        )
        .await;
        assert_eq!(status, 401, "没有 token 就不该能替用户做决定");
        assert!(server.approvals.list().is_empty());
    }

    /// 默认关：装了 hook 也**不改变任何行为** —— 端点照常回话，但什么都不说，
    /// 于是 Claude Code 走它自己的询问框。
    #[tokio::test]
    async fn approval_is_off_by_default_so_nothing_is_decided() {
        let server = start().await;
        assert!(
            !crate::config::Settings::default().approve_tools,
            "默认必须是关的 —— 这是唯一会改变智能体行为的开关"
        );
        let body = permission_request(
            "PermissionRequest",
            "Bash",
            serde_json::json!({ "command": "rm -rf build" }),
        );
        let (status, body) = call(
            &server,
            "POST",
            "/v1/approve/claude-code",
            Some(&server.token),
            false,
            Some(&body),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(body.trim(), "{}", "关着的时候不能替用户做决定");
        assert!(server.approvals.list().is_empty(), "关着时不该登记任何东西");
    }

    /// **只认 `PermissionRequest`。** `PreToolUse` 会拦下本来不需要批准的事 ——
    /// 那是替用户做决定，不是把确认框搬个地方。
    #[tokio::test]
    async fn only_the_moment_that_would_have_asked_is_intercepted() {
        let server = start().await;
        enable_approvals(&server.settings);
        for event in ["PreToolUse", "PostToolUse", "Stop"] {
            let body = permission_request(event, "Bash", serde_json::json!({ "command": "ls" }));
            let (status, body) = call(
                &server,
                "POST",
                "/v1/approve/claude-code",
                Some(&server.token),
                false,
                Some(&body),
            )
            .await;
            assert_eq!(status, 200);
            assert_eq!(body.trim(), "{}", "{event} 不该被拦");
        }
        assert!(server.approvals.list().is_empty());
    }

    /// 开着的时候：请求会**等**，灯上出现那一条，点一下 → 决定回给对方。
    #[tokio::test]
    async fn a_pending_approval_waits_for_the_decision() {
        let server = start().await;
        enable_approvals(&server.settings);
        let port = server.port;
        let token = server.token.clone();
        let body = permission_request(
            "PermissionRequest",
            "Bash",
            serde_json::json!({ "command": "rm -rf build" }),
        );

        // 这一条会一直挂着（等用户点），所以放到后台
        let waiting = tokio::spawn(async move {
            call_at(port, "POST", "/v1/approve/claude-code", Some(&token), Some(&body)).await
        });

        let id = wait_for_approval(&server).await;

        // 灯上先变黄：这条上报和普通上报走的是同一条路（分组 / 凭证 / 推送都跟着）
        let (_, state) = call(&server, "GET", "/v1/state", Some(&server.token), false, None).await;
        assert!(state.contains("\"permission\""), "{state}");
        assert!(state.contains("cc-1"), "{state}");
        // 状态里**只有那个短词**，没有命令全文
        assert!(!state.contains("rm -rf build"), "命令全文不该进状态：{state}");
        // 但灯上那个气泡看得到全文
        let (_, list) = call(&server, "GET", "/v1/approvals", Some(&server.token), false, None).await;
        assert!(list.contains("rm -rf build"), "{list}");

        // 点「允许」
        let (status, body) = call(
            &server,
            "POST",
            &format!("/v1/approvals/{id}"),
            Some(&server.token),
            false,
            Some("{\"decision\":\"allow\"}"),
        )
        .await;
        assert_eq!(status, 200);
        assert!(body.contains("\"ok\":true"), "{body}");

        let (status, body) = waiting.await.unwrap();
        assert_eq!(status, 200);
        assert!(body.contains("\"behavior\":\"allow\""), "{body}");
        assert!(server.approvals.list().is_empty(), "点过之后那条就不该再挂着");
    }

    /// **看不全就不给放行。** 长命令只能拒绝 —— 而且这条规则在登记表里挡着，
    /// 从接口上也绕不过去。
    #[tokio::test]
    async fn a_command_too_long_to_show_cannot_be_allowed() {
        let server = start().await;
        enable_approvals(&server.settings);
        let port = server.port;
        let token = server.token.clone();
        let long = format!("echo {}", "x".repeat(400));
        let body = permission_request(
            "PermissionRequest",
            "Bash",
            serde_json::json!({ "command": long }),
        );
        let waiting = tokio::spawn(async move {
            call_at(port, "POST", "/v1/approve/claude-code", Some(&token), Some(&body)).await
        });

        let id = wait_for_approval(&server).await;
        let (_, list) = call(&server, "GET", "/v1/approvals", Some(&server.token), false, None).await;
        assert!(list.contains("\"allowable\":false"), "{list}");
        assert!(list.contains("\"truncated\":true"), "{list}");

        let (_, body) = call(
            &server,
            "POST",
            &format!("/v1/approvals/{id}"),
            Some(&server.token),
            false,
            Some("{\"decision\":\"allow\"}"),
        )
        .await;
        assert!(body.contains("\"ok\":false"), "{body}");
        assert!(
            !server.approvals.list().is_empty(),
            "被拒之后那条还得在，好让人改主意去拒绝"
        );

        // 拒绝照样可以
        let (_, body) = call(
            &server,
            "POST",
            &format!("/v1/approvals/{id}"),
            Some(&server.token),
            false,
            Some("{\"decision\":\"deny\"}"),
        )
        .await;
        assert!(body.contains("\"ok\":true"), "{body}");
        let (_, body) = waiting.await.unwrap();
        assert!(body.contains("\"behavior\":\"deny\""), "{body}");
    }

    /// 局域网访问打开时和上报端点一样整条关掉：这一条**替用户做决定**，
    /// 比只写状态严重得多。要 token 是第二道防线。
    #[tokio::test]
    async fn approval_is_closed_while_lan_access_is_on() {
        let server = start().await;
        enable_approvals(&server.settings);
        *server.bind.lock().unwrap() = true;
        let body = permission_request(
            "PermissionRequest",
            "Bash",
            serde_json::json!({ "command": "ls" }),
        );
        let (status, body) = call(
            &server,
            "POST",
            "/v1/approve/claude-code",
            Some(&server.token),
            false,
            Some(&body),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(body.trim(), "{}");
        assert!(server.approvals.list().is_empty());
    }

    /// 会话结束（`SessionEnd`）时挂着的那条不再作数：对面收到「没有决定」，
    /// 也就不会有人对着一条已经没有意义的审批点。
    #[tokio::test]
    async fn a_session_ending_drops_its_pending_approval() {
        let server = start().await;
        enable_approvals(&server.settings);
        let port = server.port;
        let token = server.token.clone();
        let body = permission_request(
            "PermissionRequest",
            "Bash",
            serde_json::json!({ "command": "ls" }),
        );
        let waiting = tokio::spawn(async move {
            call_at(port, "POST", "/v1/approve/claude-code", Some(&token), Some(&body)).await
        });
        wait_for_approval(&server).await;

        // 会话结束走的是上报端点（命令式 / HTTP 式 hook 都一样）
        let end = serde_json::json!({ "hook_event_name": "SessionEnd", "session_id": "cc-1" }).to_string();
        call(&server, "POST", "/v1/hooks/claude-code", None, false, Some(&end)).await;

        assert!(server.approvals.list().is_empty(), "会话没了，那条也该没了");
        let (_, body) = waiting.await.unwrap();
        assert_eq!(body.trim(), "{}", "没有决定：交回 Claude Code 自己问");
    }
}
