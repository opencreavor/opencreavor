use axum::{body::Body, http::StatusCode, routing::post, Router};
use creavor_broker::{config::Settings, router, storage::AuditStorage};
use http_body_util::BodyExt;
use hyper_util::{
    client::legacy::{connect::HttpConnector, Client},
    rt::TokioExecutor,
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};
use tokio::{net::TcpListener, task::JoinHandle};

fn upstream_app(call_count: Arc<AtomicUsize>) -> Router {
    Router::new().route(
        "/messages",
        post(move |req: axum::extract::Request| {
            let call_count = call_count.clone();
            async move {
                call_count.fetch_add(1, Ordering::SeqCst);
                let body = req.into_body().collect().await.unwrap().to_bytes();
                let payload: Value = serde_json::from_slice(&body).unwrap_or_default();
                let model = payload.get("model").and_then(|m| m.as_str()).unwrap_or("unknown");
                (
                    StatusCode::OK,
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    Body::from(
                        json!({
                            "id": "msg_test",
                            "type": "message",
                            "role": "assistant",
                            "model": model,
                            "content": [{"type":"text","text":"Hi!"}],
                            "stop_reason": "end_turn"
                        })
                        .to_string(),
                    ),
                )
            }
        }),
    )
}

async fn spawn_http_app(app: Router) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (format!("http://{addr}"), handle)
}

async fn send_json_request(
    base_url: &str,
    path: &str,
    body: Value,
) -> hyper::Response<hyper::body::Incoming> {
    let client: Client<HttpConnector, Body> = Client::builder(TokioExecutor::new()).build_http();
    client
        .request(
            axum::http::Request::builder()
                .method("POST")
                .uri(format!("{base_url}{path}"))
                .header("content-type", "application/json")
                .header("x-creavor-runtime", "test-runtime")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}

fn broker_settings(upstream_base_url: String) -> Settings {
    let mut settings = Settings::default();
    settings.broker.stream_passthrough = false;
    settings.upstream = HashMap::from([("test-runtime".to_string(), upstream_base_url)]);
    settings
}

#[tokio::test]
async fn https_connector_forwards_to_http_upstream() {
    let upstream_call_count = Arc::new(AtomicUsize::new(0));
    let (upstream_base_url, upstream_server) =
        spawn_http_app(upstream_app(upstream_call_count.clone())).await;

    let settings = broker_settings(upstream_base_url);
    let storage = AuditStorage::open_in_memory().unwrap();
    let (broker_base_url, broker_server) =
        spawn_http_app(router::app(settings, storage)).await;

    let response = send_json_request(
        &broker_base_url,
        "/v1/anthropic/messages",
        json!({
            "model": "glm-4-flash",
            "max_tokens": 64,
            "messages": [{"role":"user","content":"hi"}]
        }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(upstream_call_count.load(Ordering::SeqCst), 1);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["model"], "glm-4-flash");
    assert_eq!(json["content"][0]["text"], "Hi!");

    broker_server.abort();
    upstream_server.abort();
}

#[tokio::test]
async fn https_connector_forwards_openai_path() {
    let upstream_call_count = Arc::new(AtomicUsize::new(0));
    let (upstream_base_url, upstream_server) =
        spawn_http_app(upstream_app(upstream_call_count.clone())).await;

    let settings = broker_settings(upstream_base_url);
    let storage = AuditStorage::open_in_memory().unwrap();
    let (broker_base_url, broker_server) =
        spawn_http_app(router::app(settings, storage)).await;

    let response = send_json_request(
        &broker_base_url,
        "/v1/openai/messages",
        json!({
            "model": "glm-4-flash",
            "messages": [{"role":"user","content":"hi"}]
        }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(upstream_call_count.load(Ordering::SeqCst), 1);

    broker_server.abort();
    upstream_server.abort();
}

#[tokio::test]
async fn https_connector_upstream_unreachable_returns_502() {
    let mut settings = Settings::default();
    settings.broker.stream_passthrough = false;
    settings.upstream = HashMap::from([("test-runtime".to_string(), "http://127.0.0.1:1".to_string())]);
    let storage = AuditStorage::open_in_memory().unwrap();
    // Point upstream to a port that nobody listens on
    let (broker_base_url, broker_server) = spawn_http_app(router::app(settings, storage)).await;

    let response = send_json_request(
        &broker_base_url,
        "/v1/anthropic/messages",
        json!({
            "model": "glm-4-flash",
            "max_tokens": 64,
            "messages": [{"role":"user","content":"hi"}]
        }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);

    broker_server.abort();
}

#[tokio::test]
async fn https_connector_preserves_upstream_headers() {
    let upstream = Router::new().route(
        "/messages",
        post(|| async {
            (
                StatusCode::OK,
                [
                    (axum::http::header::CONTENT_TYPE, "application/json"),
                    (
                        axum::http::header::HeaderName::from_static("x-custom-header"),
                        "broker-test",
                    ),
                ],
                Body::from(r#"{"id":"msg_header_test"}"#),
            )
        }),
    );
    let (upstream_base_url, upstream_server) = spawn_http_app(upstream).await;

    let settings = broker_settings(upstream_base_url);
    let storage = AuditStorage::open_in_memory().unwrap();
    let (broker_base_url, broker_server) =
        spawn_http_app(router::app(settings, storage)).await;

    let response = send_json_request(
        &broker_base_url,
        "/v1/anthropic/messages",
        json!({"model":"test","max_tokens":1,"messages":[{"role":"user","content":"hi"}]}),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("x-custom-header").unwrap(),
        "broker-test"
    );

    broker_server.abort();
    upstream_server.abort();
}

#[tokio::test]
async fn https_connector_streaming_passthrough_forwards_chunks() {
    let upstream = Router::new().route(
        "/messages",
        post(|| async {
            let body = "data: {\"type\":\"content_block_delta\",\"delta\":{\"text\":\"Hello\"}}\n\n\
                        data: {\"type\":\"content_block_delta\",\"delta\":{\"text\":\" world\"}}\n\n\
                        data: [DONE]\n\n";
            (
                StatusCode::OK,
                [
                    (axum::http::header::CONTENT_TYPE, "text/event-stream"),
                    (
                        axum::http::header::HeaderName::from_static("cache-control"),
                        "no-cache",
                    ),
                ],
                Body::from(body),
            )
        }),
    );
    let (upstream_base_url, upstream_server) = spawn_http_app(upstream).await;

    let mut settings = broker_settings(upstream_base_url);
    settings.broker.stream_passthrough = true;
    let storage = AuditStorage::open_in_memory().unwrap();
    let (broker_base_url, broker_server) =
        spawn_http_app(router::app(settings, storage)).await;

    let response = send_json_request(
        &broker_base_url,
        "/v1/anthropic/messages",
        json!({"model":"test","max_tokens":16,"messages":[{"role":"user","content":"hi"}],"stream":true}),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "text/event-stream"
    );

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("Hello"));
    assert!(text.contains("world"));
    assert!(text.contains("[DONE]"));

    broker_server.abort();
    upstream_server.abort();
}

#[tokio::test]
async fn https_connector_get_request_405() {
    let mut settings = Settings::default();
    settings.broker.stream_passthrough = false;
    settings.upstream = HashMap::from([("test-runtime".to_string(), "http://127.0.0.1:8080".to_string())]);
    let storage = AuditStorage::open_in_memory().unwrap();
    let (broker_base_url, broker_server) = spawn_http_app(router::app(settings, storage)).await;

    let client: Client<HttpConnector, Body> = Client::builder(TokioExecutor::new()).build_http();
    let response = client
        .request(
            axum::http::Request::builder()
                .method("GET")
                .uri(format!("{broker_base_url}/v1/anthropic/messages"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);

    broker_server.abort();
}


#[tokio::test]
async fn https_connector_large_request_body_handling() {
    let upstream = Router::new().route(
        "/messages",
        post(|req: axum::extract::Request| async move {
            let body = req.into_body().collect().await.unwrap().to_bytes();
            let size = body.len();
            let first_chars = String::from_utf8(body.to_vec());
            (
                StatusCode::OK,
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                Body::from(
                    json!({
                        "status": "ok",
                        "received_size": size,
                        "first_chars": if first_chars.is_ok() { first_chars.unwrap() } else { "error".to_string() }
                    })
                    .to_string(),
                ),
            )
        }),
    );
    let (upstream_base_url, upstream_server) = spawn_http_app(upstream).await;

    let settings = broker_settings(upstream_base_url);
    let storage = AuditStorage::open_in_memory().unwrap();
    let (broker_base_url, broker_server) =
        spawn_http_app(router::app(settings, storage)).await;

    // Create a large request body
    let large_content = "x".repeat(10000);
    let large_json = json!({
        "model": "claude-3-opus",
        "max_tokens": 100,
        "messages": [
            {
                "role": "user",
                "content": "This is a test message with some longer content to test larger request bodies. "
            },
            {
                "role": "assistant",
                "content": large_content
            }
        ]
    });

    let response = send_json_request(
        &broker_base_url,
        "/v1/anthropic/messages",
        large_json,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert!(json["received_size"].as_u64().unwrap() > 1000);
    let first_chars = json["first_chars"].as_str().unwrap();
    assert!(!first_chars.is_empty() && (first_chars.starts_with('{') || first_chars.starts_with('x')));

    broker_server.abort();
    upstream_server.abort();
}

#[tokio::test]
async fn https_connector_query_param_forwarding() {
    let upstream = Router::new().route(
        "/messages",
        post(move |req: axum::extract::Request| async move {
            let body = req.into_body().collect().await.unwrap().to_bytes();
            let payload: Value = serde_json::from_slice(&body).unwrap_or_default();
            (
                StatusCode::OK,
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                Body::from(
                    json!({
                        "id": "test_query",
                        "model": payload.get("model").cloned(),
                        "messages": payload.get("messages").cloned()
                    })
                    .to_string(),
                ),
            )
        }),
    );
    let (upstream_base_url, upstream_server) = spawn_http_app(upstream).await;

    let settings = broker_settings(upstream_base_url);
    let storage = AuditStorage::open_in_memory().unwrap();
    let (broker_base_url, broker_server) =
        spawn_http_app(router::app(settings, storage)).await;

    let response = send_json_request(
        &broker_base_url,
        "/v1/anthropic/messages",
        json!({"model":"test","max_tokens":1,"messages":[{"role":"user","content":"hi"}]}),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["model"], "test");
    assert_eq!(json["messages"][0]["role"], "user");
    assert_eq!(json["messages"][0]["content"], "hi");

    broker_server.abort();
    upstream_server.abort();
}

#[tokio::test]
async fn https_connector_custom_headers_forwarding() {
    let upstream = Router::new().route(
        "/messages",
        post(move |req: axum::extract::Request| async move {
            let headers = req.headers();
            let trace_id = headers.get("x-trace-id").and_then(|v| v.to_str().ok());
            let creavor_session = headers.get("x-creavor-session-id").and_then(|v| v.to_str().ok());
            let auth_header = headers.get("authorization").and_then(|v| v.to_str().ok());

            (
                StatusCode::OK,
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                Body::from(
                    json!({
                        "trace_id": trace_id,
                        "creavor_session": creavor_session,
                        "auth_present": auth_header.is_some(),
                        "content_type": headers.get("content-type").and_then(|v| v.to_str().ok()),
                        "other_present": headers.get("other-header").is_some()
                    })
                    .to_string(),
                ),
            )
        }),
    );
    let (upstream_base_url, upstream_server) = spawn_http_app(upstream).await;

    let settings = broker_settings(upstream_base_url);
    let storage = AuditStorage::open_in_memory().unwrap();
    let (broker_base_url, broker_server) =
        spawn_http_app(router::app(settings, storage)).await;

    let response = send_json_request_with_headers(
        &broker_base_url,
        "/v1/anthropic/messages",
        json!({"model":"test","messages":[{"role":"user","content":"hi"}]}),
        vec![
            ("x-trace-id", "test-123-456"),
            ("x-creavor-session-id", "session-abc"),
            ("x-creavor-runtime", "test-runtime"),
            ("authorization", "Bearer test-token"),
            ("content-type", "application/json"),
            ("other-header", "should-be-kept"),
        ],
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["trace_id"], "test-123-456");
    assert_eq!(json["creavor_session"], Value::Null); // x-creavor-session-id should be stripped
    assert_eq!(json["auth_present"], true); // authorization should be passed through
    assert_eq!(json["content_type"], "application/json");
    assert_eq!(json["other_present"], true);

    broker_server.abort();
    upstream_server.abort();
}

async fn send_json_request_with_headers(
    base_url: &str,
    path: &str,
    body: Value,
    headers: Vec<(&str, &str)>,
) -> hyper::Response<hyper::body::Incoming> {
    let client: Client<HttpConnector, Body> = Client::builder(TokioExecutor::new()).build_http();
    let mut request = axum::http::Request::builder()
        .method("POST")
        .uri(format!("{base_url}{path}"))
        .header("content-type", "application/json");

    for (name, value) in headers {
        request = request.header(name, value);
    }

    client
        .request(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap()
}
