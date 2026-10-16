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
    token: String,
    started: Instant,
}

/// 绑定并开始服务。返回后不阻塞调用方。
///
/// 接收 `Notifier` 而不是 `AppHandle`：**必须用 app 里 manage 的那个组合版
/// 通知器**，它除了推事件给前端，还要核对窗口尺寸、记统计、发通知和转发。
/// 这里若自己构造一个（早先就是这么写的），HTTP 上报这条路径就会绕过
/// 那一整套逻辑 —— 而智能体的 hook 恰好全部走这条路。
pub fn spawn(
    notify: Notifier,
    store: Arc<Mutex<Store>>,
    settings: crate::SharedSettings,
    bind: crate::SharedBind,
    receipts: Arc<crate::receipts::Receipts>,
    lang: Arc<crate::lang::LocalLang>,
    token: String,
) {
    // 监听地址**在启动时定一次**。设置里那个开关改了要重启才生效 —— 理由见
    // `Settings::lan_access`：运行中重绑会让端口变化，表现为「灯突然收不到上报」。
    let lan = settings.lock().expect("settings poisoned").lan_access;
    let host = bind_host(lan);

    let ctx = Ctx {
        store,
        notify,
        settings,
        receipts,
        // 记「实际绑到哪了」和给 handler 看的是**同一份**：handler 要按它决定
        // 免鉴权那条端点开不开，两处各存一个就会有一边撒谎
        bind: bind.clone(),
        lang,
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
        *bind.lock().expect("bind poisoned") = lan;
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
        .layer(axum::extract::DefaultBodyLimit::max(HOOK_BODY_MAX));

    Router::new()
        .route("/v1/health", get(health))
        .merge(hooks)
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
                cwd: crate::hookmap::cwd_of(payload),
                message: hook_message(ctx, payload),
                ttl_ms: None,
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
        ctx.receipts.record(agent, report.state.name());
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
    (removed, changed, guard.snapshot())
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
    }

    async fn start() -> TestServer {
        let (ctx, notifications, settings, bind) = test_ctx();

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, router(ctx)).await.unwrap();
        });

        TestServer { port, token: "s3cret".to_string(), notifications, settings, bind }
    }

    /// 一个不经 HTTP 的 ctx + 通知计数 + 那份共享设置。
    ///
    /// 重放那几条用例（`replay_spool`）不需要真的起服务器 —— 重放本来就不走网络，
    /// 它是「把盘上的东西补进状态机」。
    fn test_ctx() -> (Ctx, Arc<AtomicUsize>, crate::SharedSettings, crate::SharedBind) {
        let notifications = Arc::new(AtomicUsize::new(0));
        let counter = notifications.clone();
        let settings: crate::SharedSettings =
            Arc::new(Mutex::new(Arc::new(crate::config::Settings::default())));
        // 回环（`false`）：HTTP 式 hook 那条端点默认开着，才测得到它
        let bind: crate::SharedBind = Arc::new(Mutex::new(false));

        let ctx = Ctx {
            store: Arc::new(Mutex::new(Store::default())),
            notify: Arc::new(move |_header: &Snapshot| {
                counter.fetch_add(1, Ordering::SeqCst);
            }),
            settings: settings.clone(),
            // 不落盘（`None`）：单测绝不能写开发者真实的 `~/.signalhub`。
            // 上报路径会顺手记凭证，所以这里必须真的给一个，不能用 `Default`。
            receipts: Arc::new(crate::receipts::Receipts::in_memory()),
            bind: bind.clone(),
            lang: Arc::new(crate::lang::LocalLang::default()),
            token: "s3cret".to_string(),
            started: Instant::now(),
        };
        (ctx, notifications, settings, bind)
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
    async fn call(
        server: &TestServer,
        method: &str,
        path: &str,
        token: Option<&str>,
        origin: bool,
        body: Option<&str>,
    ) -> (u16, String) {
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", server.port))
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

        let text = String::from_utf8_lossy(&raw).into_owned();
        let status = text
            .split_whitespace()
            .nth(1)
            .and_then(|code| code.parse().ok())
            .unwrap_or(0);
        let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
        (status, body)
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
        let (ctx, notifications, _settings, _bind) = test_ctx();
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
        let (ctx, notifications, _settings, _bind) = test_ctx();
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
        let (ctx, _notifications, _settings, _bind) = test_ctx();
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
        let (ctx, _notifications, _settings, _bind) = test_ctx();
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
}
