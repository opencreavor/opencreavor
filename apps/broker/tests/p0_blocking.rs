use axum::{
    body::Body,
    extract::{Request, State},
    http::{Response, StatusCode},
    routing::post,
    Router,
};
use creavor_broker::{
    config::Config,
    interceptor::{openai_block_response_with_status, strip_session_header},
    router::{provider_for_path, Provider},
    rule_engine::{scan_request, RuleSet},
};
use futures_util::TryStreamExt;
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

#[derive(Clone)]
struct ProxyState {
    config: Config,
    rules: RuleSet,
    upstream_base_url: String,
    client: Client<HttpConnector, Body>,
}

async fn proxy_openai(
    State(state): State<ProxyState>,
    request: Request,
) -> Response<Body> {
    let provider = provider_for_path(request.uri().path()).expect("known provider path");
    let method = request.method().clone();
    let mut headers = request.headers().clone();
    let request_body = request.into_body().collect().await.unwrap().to_bytes();
    let request_body_text = String::from_utf8(request_body.to_vec()).unwrap();

    if let Some(rule_match) = scan_request(&request_body_text, &state.rules) {
        let message = format!(
            "Blocked by Creavor broker: {} ({})",
            rule_match.rule_name, rule_match.matched_content_sanitized
        );
        return match provider {
            Provider::OpenAI => {
                openai_block_response_with_status(
                    StatusCode::from_u16(state.config.broker.block_status_code).unwrap(),
                    &message,
                )
            }
            Provider::Anthropic => unreachable!("test only wires OpenAI path"),
        };
    }

    strip_session_header(&mut headers);
    let mut upstream_request = axum::http::Request::builder()
        .method(method)
        .uri(format!("{}/responses", state.upstream_base_url));
    if let Some(content_type) = headers.get("content-type") {
        upstream_request = upstream_request.header("content-type", content_type);
    }

    let upstream_response = state
        .client
        .request(
            upstream_request
                .body(Body::from(request_body_text))
                .expect("valid upstream request"),
        )
        .await
        .expect("upstream request should succeed");

    let status = upstream_response.status();
    let headers = upstream_response.headers().clone();
    let body = Body::from_stream(
        upstream_response
            .into_body()
            .into_data_stream()
            .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> { Box::new(error) }),
    );

    Response::builder()
        .status(status)
        .body(body)
        .map(|mut response| {
            *response.headers_mut() = headers;
            response
        })
        .unwrap()
}

fn proxy_app(config: Config, upstream_base_url: String) -> Router {
    let client: Client<HttpConnector, Body> = Client::builder(TokioExecutor::new()).build_http();

    Router::new()
        .route("/v1/openai/responses", post(proxy_openai))
        .with_state(ProxyState {
            config,
            rules: RuleSet::builtin(),
            upstream_base_url,
            client,
        })
}

fn upstream_app(call_count: Arc<AtomicUsize>) -> Router {
    Router::new().route(
        "/responses",
        post(move || {
            let call_count = call_count.clone();
            async move {
                call_count.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::OK,
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    Body::from(r#"{"id":"upstream-ok"}"#),
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

async fn send_json_request(base_url: &str, path: &str, body: Value) -> hyper::Response<hyper::body::Incoming> {
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
async fn blocked_secret_request_returns_provider_block_and_skips_upstream() {
    let upstream_call_count = Arc::new(AtomicUsize::new(0));
    let (upstream_base_url, upstream_server) =
        spawn_http_app(upstream_app(upstream_call_count.clone())).await;

    let mut config = Config::default();
    config.broker.block_status_code = 451;
    let (broker_base_url, broker_server) = spawn_http_app(proxy_app(config, upstream_base_url)).await;

    let response = send_json_request(
        &broker_base_url,
        "/v1/openai/responses",
        json!({
            "model": "gpt-5",
            "input": "My key is sk-1234567890abcdef123456"
        }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::UNAVAILABLE_FOR_LEGAL_REASONS);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({
            "error": {
                "message": "Blocked by Creavor broker: OpenAI API Key (sk-***456)",
                "type": "invalid_request_error",
                "param": Value::Null,
                "code": Value::Null,
            }
        })
    );
    assert_eq!(upstream_call_count.load(Ordering::SeqCst), 0);

    broker_server.abort();
    upstream_server.abort();
}
