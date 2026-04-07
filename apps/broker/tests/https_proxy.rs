use axum::{body::Body, http::StatusCode, routing::post, Router};
use creavor_broker::{config::Config, router};
use http_body_util::BodyExt;
use hyper_util::{
    client::legacy::{connect::HttpConnector, Client},
    rt::TokioExecutor,
};
use serde_json::{json, Value};
use std::{
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
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn https_connector_forwards_to_http_upstream() {
    let upstream_call_count = Arc::new(AtomicUsize::new(0));
    let (upstream_base_url, upstream_server) =
        spawn_http_app(upstream_app(upstream_call_count.clone())).await;

    let config = Config::default();
    let (broker_base_url, broker_server) =
        spawn_http_app(router::proxy_app(config, upstream_base_url)).await;

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

    let config = Config::default();
    let (broker_base_url, broker_server) =
        spawn_http_app(router::proxy_app(config, upstream_base_url)).await;

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
    let config = Config::default();
    // Point upstream to a port that nobody listens on
    let (broker_base_url, broker_server) = spawn_http_app(router::proxy_app(
        config,
        "http://127.0.0.1:1".to_string(),
    ))
    .await;

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

    let config = Config::default();
    let (broker_base_url, broker_server) =
        spawn_http_app(router::proxy_app(config, upstream_base_url)).await;

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

    let mut config = Config::default();
    config.broker.stream_passthrough = true;
    let (broker_base_url, broker_server) =
        spawn_http_app(router::proxy_app(config, upstream_base_url)).await;

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
