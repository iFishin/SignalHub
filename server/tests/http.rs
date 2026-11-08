//! 全链路集成测试：真的走 axum Router（oneshot，不绑端口），覆盖
//! 「建链接 → 收信 → 限流 → 配额 → 吊销 → 拉取」这条主路径。
//!
//! 这里断言的是**对外契约**（路径、状态码、`error` 码），不是内部函数 —— 改了
//! `docs/api.md` 却没改这里、或者反过来，都应当让 CI 变红。

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use signalhub_callme::{router, AppState, Limits, Store};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

const ADMIN: &str = "test-admin-token-0123456789";

static COUNTER: AtomicU32 = AtomicU32::new(0);

struct Harness {
    app: Router,
    dir: PathBuf,
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Harness {
    fn new(tag: &str, limits: Limits) -> Harness {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "signalhub-callme-it-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::open(&dir, limits).expect("打开 store");
        let app = router(AppState {
            store: Arc::new(Mutex::new(store)),
            admin_token: ADMIN.to_string(),
            trust_proxy: false,
        });
        Harness { app, dir }
    }

    fn request(
        &self,
        method: &str,
        uri: &str,
        body: Option<Value>,
        bearer: bool,
        ip: &str,
    ) -> Request<Body> {
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json");
        if bearer {
            builder = builder.header("authorization", format!("Bearer {ADMIN}"));
        }
        let mut request = builder
            .body(
                body.map(|value| Body::from(value.to_string()))
                    .unwrap_or_else(Body::empty),
            )
            .unwrap();
        // 线上由 `into_make_service_with_connect_info` 注入；测试手动塞进去。
        request.extensions_mut().insert(ConnectInfo(SocketAddr::new(
            ip.parse::<IpAddr>().unwrap(),
            4444,
        )));
        request
    }

    async fn call(&self, request: Request<Body>) -> (StatusCode, Value) {
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, value)
    }

    /// 建一条链接，返回 (id, token)。
    async fn create_link(&self, label: &str) -> (String, String) {
        let (status, body) = self
            .call(self.request("POST", "/v1/callme/sources", Some(json!({ "label": label })), true, "10.0.0.1"))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        (
            body["id"].as_str().unwrap().to_string(),
            body["token"].as_str().unwrap().to_string(),
        )
    }
}

#[tokio::test]
async fn health_is_public() {
    let harness = Harness::new("health", Limits::default());
    let (status, body) = harness
        .call(harness.request("GET", "/v1/callme/health", None, false, "10.0.0.1"))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], true);
    assert!(body["version"].is_string());
}

#[tokio::test]
async fn every_response_is_uncacheable() {
    let harness = Harness::new("cache", Limits::default());
    let (_, token) = harness.create_link("文章").await;

    // 成功、校验失败、鉴权失败、404 都走同一条出口：中间任何一层代理把
    // 留言缓存下来都是泄露，所以头不能只加在部分 handler 上。
    let cases: Vec<(&str, String, Option<Value>, bool, &str)> = vec![
        ("GET", "/v1/callme/health".to_string(), None, false, "10.0.0.1"),
        ("GET", "/v1/callme/messages".to_string(), None, true, "10.0.0.1"),
        ("GET", "/v1/callme/messages".to_string(), None, false, "10.0.0.1"),
        (
            "POST",
            format!("/c/{token}"),
            Some(json!({ "text": "看过了，第三节可以再具体些。" })),
            false,
            "10.0.0.2",
        ),
        (
            "POST",
            "/c/deadbeefdeadbeefdeadbeefdeadbeef".to_string(),
            Some(json!({ "text": "x" })),
            false,
            "10.0.0.3",
        ),
        (
            "POST",
            "/v1/callme/sources".to_string(),
            Some(json!({ "label": "" })),
            true,
            "10.0.0.1",
        ),
    ];

    for (method, uri, body, bearer, ip) in cases {
        let response = harness
            .app
            .clone()
            .oneshot(harness.request(method, &uri, body, bearer, ip))
            .await
            .unwrap();
        let cache = response
            .headers()
            .get("cache-control")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        assert_eq!(
            cache.as_deref(),
            Some("no-store"),
            "{method} {uri} 缺 Cache-Control"
        );
    }
}

#[tokio::test]
async fn admin_endpoints_require_token() {
    let harness = Harness::new("auth", Limits::default());
    for (method, uri) in [
        ("GET", "/v1/callme/sources"),
        ("POST", "/v1/callme/sources"),
        ("GET", "/v1/callme/messages"),
    ] {
        let (status, body) = harness
            .call(harness.request(method, uri, Some(json!({ "label": "x" })), false, "10.0.0.1"))
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {uri}");
        assert_eq!(body["error"], "unauthorized");
    }

    // 错的 token 也一样
    let request = harness.request("GET", "/v1/callme/sources", None, false, "10.0.0.1");
    let mut request = request;
    request.headers_mut().insert(
        "authorization",
        "Bearer wrong-token-0123456789".parse().unwrap(),
    );
    let (status, _) = harness.call(request).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn create_link_then_receive() {
    let harness = Harness::new("flow", Limits::default());
    let (id, token) = harness.create_link("博客：SignalHub").await;
    assert_eq!(token.len(), 32, "令牌应当是 32 位 hex");

    let (status, body) = harness
        .call(harness.request(
            "POST",
            &format!("/c/{token}"),
            Some(json!({
                "text": "开头这段建议换个说法",
                "title": "关于第一段",
                "agent": "claude",
                "context": "https://blog.example.com/signalhub"
            })),
            false,
            "10.9.9.9",
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], true);

    let (status, body) = harness
        .call(harness.request("GET", "/v1/callme/messages", None, true, "10.0.0.1"))
        .await;
    assert_eq!(status, StatusCode::OK);
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["text"], "开头这段建议换个说法");
    assert_eq!(messages[0]["source_id"], id);
    assert_eq!(messages[0]["context"], "https://blog.example.com/signalhub");
    assert_eq!(body["next_after"], 1);

    // 来源列表里带统计
    let (status, body) = harness
        .call(harness.request("GET", "/v1/callme/sources", None, true, "10.0.0.1"))
        .await;
    assert_eq!(status, StatusCode::OK);
    let sources = body["sources"].as_array().unwrap();
    assert_eq!(sources[0]["total"], 1);
    assert_eq!(sources[0]["today"], 1);
    assert_eq!(sources[0]["link"], format!("/c/{token}"));
}

#[tokio::test]
async fn revoked_and_unknown_tokens_are_indistinguishable() {
    let harness = Harness::new("revoke", Limits::default());
    let (id, token) = harness.create_link("文章").await;

    let (status, body) = harness
        .call(harness.request(
            "POST",
            "/v1/callme/sources/nope/revoke",
            None,
            true,
            "10.0.0.1",
        ))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    let (status, _) = harness
        .call(harness.request(
            "POST",
            &format!("/v1/callme/sources/{id}/revoke"),
            None,
            true,
            "10.0.0.1",
        ))
        .await;
    assert_eq!(status, StatusCode::OK);

    let revoked = harness
        .call(harness.request(
            "POST",
            &format!("/c/{token}"),
            Some(json!({ "text": "x" })),
            false,
            "10.0.0.9",
        ))
        .await;
    let unknown = harness
        .call(harness.request(
            "POST",
            "/c/00000000000000000000000000000000",
            Some(json!({ "text": "x" })),
            false,
            "10.0.0.9",
        ))
        .await;
    assert_eq!(revoked.0, StatusCode::NOT_FOUND);
    assert_eq!(revoked.0, unknown.0);
    assert_eq!(revoked.1["error"], unknown.1["error"]);
    assert_eq!(revoked.1["message"], unknown.1["message"]);
}

#[tokio::test]
async fn browser_origin_is_blocked() {
    let harness = Harness::new("origin", Limits::default());
    let (_, token) = harness.create_link("文章").await;
    let mut request = harness.request(
        "POST",
        &format!("/c/{token}"),
        Some(json!({ "text": "x" })),
        false,
        "10.0.0.1",
    );
    request
        .headers_mut()
        .insert("origin", "https://evil.example.com".parse().unwrap());
    let (status, body) = harness.call(request).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "browser_blocked");
}

#[tokio::test]
async fn per_ip_rate_limit() {
    let harness = Harness::new(
        "ip",
        Limits {
            ip_hourly: 2,
            ..Limits::default()
        },
    );
    let (_, token) = harness.create_link("文章").await;

    for _ in 0..2 {
        let (status, _) = harness
            .call(harness.request(
                "POST",
                &format!("/c/{token}"),
                Some(json!({ "text": "hi" })),
                false,
                "10.7.7.7",
            ))
            .await;
        assert_eq!(status, StatusCode::OK);
    }

    let (status, body) = harness
        .call(harness.request(
            "POST",
            &format!("/c/{token}"),
            Some(json!({ "text": "hi" })),
            false,
            "10.7.7.7",
        ))
        .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["error"], "rate_limited");
    assert!(body["retry_after_secs"].as_u64().unwrap() > 0);

    // 换 IP 不受影响
    let (status, _) = harness
        .call(harness.request(
            "POST",
            &format!("/c/{token}"),
            Some(json!({ "text": "hi" })),
            false,
            "10.7.7.8",
        ))
        .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn per_link_quota() {
    let harness = Harness::new(
        "quota",
        Limits {
            source_daily: 1,
            ip_hourly: 100,
            ..Limits::default()
        },
    );
    let (_, token) = harness.create_link("文章").await;

    let (status, _) = harness
        .call(harness.request(
            "POST",
            &format!("/c/{token}"),
            Some(json!({ "text": "第一条" })),
            false,
            "10.8.8.1",
        ))
        .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = harness
        .call(harness.request(
            "POST",
            &format!("/c/{token}"),
            Some(json!({ "text": "第二条" })),
            false,
            "10.8.8.2",
        ))
        .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["error"], "quota_exceeded");
}

#[tokio::test]
async fn text_is_validated() {
    let harness = Harness::new("text", Limits::default());
    let (_, token) = harness.create_link("文章").await;

    let (status, body) = harness
        .call(harness.request(
            "POST",
            &format!("/c/{token}"),
            Some(json!({ "text": "   " })),
            false,
            "10.0.0.1",
        ))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "bad_request");

    let long = "字".repeat(4_001);
    let (status, body) = harness
        .call(harness.request(
            "POST",
            &format!("/c/{token}"),
            Some(json!({ "text": long })),
            false,
            "10.0.0.2",
        ))
        .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(body["error"], "too_long");
}

#[tokio::test]
async fn malformed_json_gets_json_error() {
    let harness = Harness::new("badjson", Limits::default());
    let (_, token) = harness.create_link("文章").await;

    let request = Request::builder()
        .method("POST")
        .uri(format!("/c/{token}"))
        .header("content-type", "application/json")
        .body(Body::from("{ 这不是 JSON"))
        .unwrap();
    let mut request = request;
    request.extensions_mut().insert(ConnectInfo(SocketAddr::new(
        "10.0.0.1".parse::<IpAddr>().unwrap(),
        4444,
    )));
    let (status, body) = harness.call(request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "bad_request");
    assert!(body["message"].is_string());
}

#[tokio::test]
async fn cursor_pagination_and_source_filter() {
    let harness = Harness::new("cursor", Limits::default());
    let (id_a, token_a) = harness.create_link("A").await;
    let (id_b, token_b) = harness.create_link("B").await;

    for i in 0..3 {
        let (status, _) = harness
            .call(harness.request(
                "POST",
                &format!("/c/{token_a}"),
                Some(json!({ "text": format!("a{i}") })),
                false,
                format!("10.6.0.{}", i + 1).as_str(),
            ))
            .await;
        assert_eq!(status, StatusCode::OK);
    }
    let (status, _) = harness
        .call(harness.request(
            "POST",
            &format!("/c/{token_b}"),
            Some(json!({ "text": "b0" })),
            false,
            "10.6.0.9",
        ))
        .await;
    assert_eq!(status, StatusCode::OK);

    // 增量：只要 seq > 2 的两条
    let (status, body) = harness
        .call(harness.request("GET", "/v1/callme/messages?after=2", None, true, "10.0.0.1"))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["messages"].as_array().unwrap().len(), 2);
    assert_eq!(body["next_after"], 4);

    // 过滤
    let (status, body) = harness
        .call(harness.request(
            "GET",
            &format!("/v1/callme/messages?source={id_b}"),
            None,
            true,
            "10.0.0.1",
        ))
        .await;
    assert_eq!(status, StatusCode::OK);
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["source_id"], id_b);

    let _ = id_a;
}

#[tokio::test]
async fn link_creation_validates_label() {
    let harness = Harness::new("label", Limits::default());
    let (status, body) = harness
        .call(harness.request(
            "POST",
            "/v1/callme/sources",
            Some(json!({ "label": "   " })),
            true,
            "10.0.0.1",
        ))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"], "bad_request");
}
