use crate::config;
use crate::notify::Notifier;
use crate::state::{state_ttl, Report, Snapshot, Store};
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
use std::time::Instant;

/// 首选端口。被占用就退到系统分配，两种情况下都把实际端口写进 `~/.signalhub/port`。
const PREFERRED_PORT: u16 = 41320;

#[derive(Clone)]
struct Ctx {
    store: Arc<Mutex<Store>>,
    notify: Notifier,
    /// 上报路径要读 `group_by`（兜底分组）。和 `store` 一样是**共享的同一份**，
    /// 不是启动时的快照 —— 用户在设置面板里一改开关，下一次上报就得生效。
    settings: crate::SharedSettings,
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
    token: String,
) {
    // 监听地址**在启动时定一次**。设置里那个开关改了要重启才生效 —— 理由见
    // `Settings::lan_access`：运行中重绑会让端口变化，表现为「灯突然收不到上报」。
    let lan = settings.lock().expect("settings poisoned").lan_access;
    let host = bind_host(lan);

    let ctx = Ctx { store, notify, settings, token, started: Instant::now() };

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

fn router(ctx: Ctx) -> Router {
    // 只有 health 免鉴权，其余整组挂上中间件。
    // 用中间件而不是在每个 handler 里手写一次检查：漏掉一处就是一个安全洞。
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

    Router::new()
        .route("/v1/health", get(health))
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
    let (removed, changed, snapshot) = {
        let mut guard = ctx.store.lock().expect("state store poisoned");
        let (removed, changed) = guard.remove_session(&session, std::time::Instant::now());
        (removed, changed, guard.snapshot())
    };
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
        let ttl = state_ttl(report.state, report.ttl_ms, &settings);
        (resolved, ttl)
    };
    report.group = resolved;

    let (changed, snapshot) = {
        let mut guard = ctx.store.lock().expect("state store poisoned");
        let changed = guard.apply(Instant::now(), report, ttl);
        (changed, guard.snapshot())
    };
    if changed {
        (ctx.notify)(&snapshot);
    }

    // 顺带把聚合结果回给调用方，这样一次 curl 就能看到全局状态
    Ok(Json(json!({
        "ok": true,
        "aggregate": snapshot.aggregate,
        "sessions": snapshot.all_sessions().count(),
        "groups": snapshot.groups.len(),
        "changed": changed,
    })))
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
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    }

    async fn start() -> TestServer {
        let notifications = Arc::new(AtomicUsize::new(0));
        let counter = notifications.clone();
        let settings: crate::SharedSettings =
            Arc::new(Mutex::new(Arc::new(crate::config::Settings::default())));

        let ctx = Ctx {
            store: Arc::new(Mutex::new(Store::default())),
            notify: Arc::new(move |_header: &Snapshot| {
                counter.fetch_add(1, Ordering::SeqCst);
            }),
            settings: settings.clone(),
            token: "s3cret".to_string(),
            started: Instant::now(),
        };

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, router(ctx)).await.unwrap();
        });

        TestServer { port, token: "s3cret".to_string(), notifications, settings }
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
}
