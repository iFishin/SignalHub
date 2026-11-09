//! HTTP 层：路由、鉴权、限流落点、错误形状。
//!
//! 两条纪律：
//!
//! 1. **所有错误都是 JSON**（`{error, message, retry_after_secs?}`）。外部调用方
//!    是智能体，纯文本的 422 它读不懂；错误码也要稳定，方便提示词里写「遇到
//!    `rate_limited` 就等 `retry_after_secs` 秒再试」。
//! 2. **公开端点只认路径里的令牌**，不带 Bearer —— 用户在文章里贴的是一条裸链接，
//!    让智能体额外带 header 会显著降低被正确使用的概率。

use crate::model::{ErrorBody, Incoming, MessagesPage, NewSource, SourceView};
use crate::store::{now_ms, AcceptError, Store};
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};

/// 请求体上限。正文本身限 4000 字符（约 12KB UTF-8 最坏情况），64KB 给 JSON
/// 包装和其他字段留足余量；再大就是有人在往这里灌东西。
const MAX_BODY_BYTES: usize = 64 * 1024;

/// 拉取的默认页大小。200 是上限（见 `Store::messages_after`）。
const DEFAULT_PAGE: usize = 100;

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Mutex<Store>>,
    /// 客户端拉取 / 管理来源用的 token。
    pub admin_token: String,
    /// 反代后面才信任 `X-Forwarded-For`。
    pub trust_proxy: bool,
}

/// 组装路由。集成测试直接对它的返回值 `oneshot`。
pub fn router(state: AppState) -> Router {
    Router::new()
        // 公开收信。**短路径**：这条链接会被贴进文章里，越短越好。
        // 同一路径的 GET 返回用法说明（见 `describe`）—— 链接保持短，但拿到它的
        // 智能体先 GET 一次就知道该 POST 什么。
        .route("/c/{token}", post(receive).get(describe))
        .route("/v1/callme/health", get(health))
        .route("/v1/callme/sources", get(list_sources).post(create_source))
        .route("/v1/callme/sources/{id}/revoke", post(revoke_source))
        .route("/v1/callme/messages", get(list_messages))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        // 所有响应（成功与错误）一律不可缓存。放在 layer 里而不是每个 handler
        // 手动加：这里返回的是私人留言，将来加端点时漏写就是一次泄露。
        .layer(axum::middleware::from_fn(no_store))
        .with_state(state)
}

/// 给每个出站响应盖上 `Cache-Control: no-store`。
async fn no_store(request: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

// ---- handlers ----

async fn health(State(state): State<AppState>) -> Json<serde_json::Value> {
    let (sources, messages) = {
        let store = state.store.lock().expect("store poisoned");
        (store.source_count(), store.message_count())
    };
    Json(serde_json::json!({
        "ok": true,
        "version": env!("CARGO_PKG_VERSION"),
        "sources": sources,
        "messages": messages,
    }))
}

/// 公开收信。校验顺序与 `Store::accept` 一致（长度 → 限速 → 配额）。
async fn receive(
    State(state): State<AppState>,
    Path(token): Path<String>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Result<Json<Incoming>, JsonRejection>,
) -> Response {
    // 浏览器请求一律拒绝：这个端点只给智能体 / curl 用。挡掉「你正在浏览的网页
    // 偷偷往这里 POST」这一整类滥用，而正常调用方不会带 Origin。
    if headers.contains_key(header::ORIGIN) {
        return fail(
            StatusCode::FORBIDDEN,
            "browser_blocked",
            "这个链接只接受非浏览器请求（智能体 / curl）",
            None,
        );
    }

    let Json(incoming) = match body {
        Ok(body) => body,
        Err(rejection) => return rejection_response(rejection),
    };

    let ip = client_ip(&headers, peer, state.trust_proxy);
    let result = {
        let mut store = state.store.lock().expect("store poisoned");
        store.accept(&token, ip, now_ms(), incoming)
    };

    match result {
        Ok(message) => {
            // 只记元数据，不记正文：服务器日志可能被转发/收集，而留言是私人的。
            eprintln!(
                "收到留言 #{}（来源 {}，{} 字）",
                message.seq,
                message.source_id,
                message.text.chars().count()
            );
            Json(serde_json::json!({ "ok": true, "seq": message.seq })).into_response()
        }
        Err(err) => accept_error(err),
    }
}

/// 同一条短链接的 **GET**：返回一段纯文本的用法说明。
///
/// 为什么需要它：贴进文章里的链接必须短，但拿到链接的智能体并不知道该 POST
/// 什么、带什么字段。让它对**同一个 URL** 先 GET 一次就有全部答案，链接不用
/// 变长，用户也不用额外复制一段提示词贴在文章里。人在浏览器里打开同样能读。
async fn describe(State(state): State<AppState>, Path(token): Path<String>) -> Response {
    let (found, limits) = {
        let store = state.store.lock().expect("store poisoned");
        (store.source_for_token(&token).is_some(), store.limits())
    };
    if !found {
        // 和 POST 一样：不存在与已吊销同一个 404。
        return fail(
            StatusCode::NOT_FOUND,
            "not_found",
            "没有这个链接（或已吊销）",
            None,
        );
    }

    let body = format!(
        "Call Me — 给这个页面的作者留个言 / leave a note for the author.\n\
         \n\
         用法：对「你刚刚 GET 的这个 URL」发一个 POST，正文是 JSON。\n\
         How: POST to the exact URL you just fetched, with a JSON body.\n\
         \n\
         Content-Type: application/json\n\
         \n\
         {{\n\
         \x20 \"text\":    \"your note (REQUIRED, 1-{max_text} characters)\",\n\
         \x20 \"title\":   \"short subject (optional; also names this link)\",\n\
         \x20 \"agent\":   \"your agent name, e.g. claude-code (optional)\",\n\
         \x20 \"context\": \"the page URL or file you are commenting on (optional)\"\n\
         }}\n\
         \n\
         Example (replace $URL with this link):\n\
         \x20 curl -X POST \"$URL\" -H 'Content-Type: application/json' \\\n\
         \x20   -d '{{\"text\":\"...\",\"title\":\"...\",\"context\":\"https://...\"}}'\n\
         \n\
         Limits / 限额：{ip_hourly} notes per IP per hour, {source_daily} per link per 24h,\n\
         \x20 {max_text} characters per note.\n\
         \n\
         Success / 成功：200 {{\"ok\":true,\"seq\":N}}\n\
         Errors / 失败：JSON {{\"error\",\"message\",\"retry_after_secs\"}} —\n\
         \x20 bad_request (missing/empty text), too_long, rate_limited, quota_exceeded,\n\
         \x20 not_found (link revoked or unknown). On rate_limited / quota_exceeded wait\n\
         \x20 retry_after_secs before retrying.\n\
         \n\
         No auth header needed: the token in this URL is the credential.\n\
         不需要额外的鉴权头；POST 时不要带浏览器 Origin 头（带了一律 403）。\n",
        max_text = limits.max_text_chars,
        ip_hourly = limits.ip_hourly,
        source_daily = limits.source_daily,
    );

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        body,
    )
        .into_response()
}

async fn list_sources(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(response) = unauthorized(&state, &headers) {
        return response;
    }
    let sources: Vec<SourceView> = {
        let store = state.store.lock().expect("store poisoned");
        store.sources_view(now_ms())
    };
    Json(serde_json::json!({ "sources": sources })).into_response()
}

async fn create_source(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<NewSource>, JsonRejection>,
) -> Response {
    if let Some(response) = unauthorized(&state, &headers) {
        return response;
    }
    let Json(new) = match body {
        Ok(body) => body,
        Err(rejection) => return rejection_response(rejection),
    };
    let created = {
        let mut store = state.store.lock().expect("store poisoned");
        store.create_source(&new.label, new.note.as_deref(), now_ms())
    };
    match created {
        Ok(source) => {
            // 立刻回一份带统计的视图，客户端不用再拉一次列表。
            let view = {
                let store = state.store.lock().expect("store poisoned");
                store
                    .sources_view(now_ms())
                    .into_iter()
                    .find(|s| s.id == source.id)
            };
            match view {
                Some(view) => Json(view).into_response(),
                None => Json(serde_json::json!({ "ok": true })).into_response(),
            }
        }
        Err(message) => fail(StatusCode::BAD_REQUEST, "bad_request", message, None),
    }
}

async fn revoke_source(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Some(response) = unauthorized(&state, &headers) {
        return response;
    }
    let result = {
        let mut store = state.store.lock().expect("store poisoned");
        store.revoke(&id)
    };
    match result {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(message) => fail(StatusCode::NOT_FOUND, "not_found", message, None),
    }
}

#[derive(Debug, Deserialize)]
struct MessagesQuery {
    after: Option<u64>,
    limit: Option<usize>,
    source: Option<String>,
}

async fn list_messages(
    State(state): State<AppState>,
    headers: HeaderMap,
    query: Result<Query<MessagesQuery>, QueryRejection>,
) -> Response {
    if let Some(response) = unauthorized(&state, &headers) {
        return response;
    }
    let Query(query) = match query {
        Ok(query) => query,
        Err(rejection) => {
            return fail(
                StatusCode::BAD_REQUEST,
                "bad_request",
                format!("查询参数不合法：{}", rejection.body_text()),
                None,
            );
        }
    };

    let source = query
        .source
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let (messages, next_after) = {
        let store = state.store.lock().expect("store poisoned");
        store.messages_after(query.after, query.limit.unwrap_or(DEFAULT_PAGE), source)
    };
    Json(MessagesPage {
        messages,
        next_after,
    })
    .into_response()
}

// ---- 鉴权与工具 ----

/// 管理端点的 Bearer 校验。返回 `Some(response)` = 直接回 401。
fn unauthorized(state: &AppState, headers: &HeaderMap) -> Option<Response> {
    let presented = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .unwrap_or("");
    if ct_eq(presented, &state.admin_token) {
        return None;
    }
    Some(fail(
        StatusCode::UNAUTHORIZED,
        "unauthorized",
        "admin token 不对（客户端里填的是服务器上的 CALLME_ADMIN_TOKEN）",
        None,
    ))
}

/// 常数时间比较：不因为「第几个字符开始不一样」泄露 token 前缀。
///
/// 不等长直接返回 false —— 长度本身不是秘密（它写在文档里）。
fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// 真实客户端 IP。`trust_proxy` 关着时**绝不**读 `X-Forwarded-For`：那个头
/// 谁都能伪造，信了就等于把限速变成摆设。
fn client_ip(headers: &HeaderMap, peer: SocketAddr, trust_proxy: bool) -> IpAddr {
    if trust_proxy
        && let Some(value) = headers
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok())
        && let Some(first) = value.split(',').next()
        && let Ok(ip) = first.trim().parse::<IpAddr>()
    {
        return ip;
    }
    peer.ip()
}

fn rejection_response(rejection: JsonRejection) -> Response {
    let status = rejection.status();
    let code = if status == StatusCode::PAYLOAD_TOO_LARGE {
        "too_long"
    } else {
        "bad_request"
    };
    fail(status, code, rejection.body_text(), None)
}

fn accept_error(err: AcceptError) -> Response {
    match err {
        AcceptError::NotFound => fail(
            StatusCode::NOT_FOUND,
            "not_found",
            "链接不存在或已被吊销",
            None,
        ),
        AcceptError::BadRequest(message) => {
            fail(StatusCode::BAD_REQUEST, "bad_request", message, None)
        }
        AcceptError::TooLong => fail(
            StatusCode::PAYLOAD_TOO_LARGE,
            "too_long",
            "text 太长了（上限 4000 字）",
            None,
        ),
        AcceptError::RateLimited { retry_after_secs } => fail(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "发得太频繁了，等一会儿再试",
            Some(retry_after_secs),
        ),
        AcceptError::QuotaExceeded { retry_after_secs } => fail(
            StatusCode::TOO_MANY_REQUESTS,
            "quota_exceeded",
            "这条链接 24 小时内的额度用完了",
            Some(retry_after_secs),
        ),
        AcceptError::Storage(message) => {
            eprintln!("存储失败：{message}");
            fail(
                StatusCode::INTERNAL_SERVER_ERROR,
                "storage",
                "服务器写入失败，稍后再试",
                None,
            )
        }
    }
}

/// 统一错误响应。带 `Retry-After` 头（有些客户端只看头不看 body）。
/// `Cache-Control` 由 [`no_store`] 统一加，这里不重复。
fn fail(
    status: StatusCode,
    code: &'static str,
    message: impl Into<String>,
    retry_after_secs: Option<u64>,
) -> Response {
    let body = ErrorBody {
        error: code.to_string(),
        message: message.into(),
        retry_after_secs,
    };
    let mut response = (status, Json(body)).into_response();
    if let Some(secs) = retry_after_secs
        && let Ok(value) = HeaderValue::from_str(&secs.to_string())
    {
        response.headers_mut().insert(header::RETRY_AFTER, value);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_compare() {
        assert!(ct_eq("abc", "abc"));
        assert!(!ct_eq("abc", "abd"));
        assert!(!ct_eq("abc", "ab"));
        assert!(!ct_eq("", "abc"));
    }

    #[test]
    fn forwarded_for_only_when_trusted() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_static("203.0.113.9, 10.0.0.1"));
        let peer: SocketAddr = "10.0.0.1:5555".parse().unwrap();

        assert_eq!(
            client_ip(&headers, peer, false),
            peer.ip(),
            "默认不信任 X-Forwarded-For"
        );
        assert_eq!(
            client_ip(&headers, peer, true),
            "203.0.113.9".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn forwarded_for_garbage_falls_back_to_peer() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_static("not-an-ip"));
        let peer: SocketAddr = "10.0.0.1:5555".parse().unwrap();
        assert_eq!(client_ip(&headers, peer, true), peer.ip());
    }
}
