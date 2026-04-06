use axum::{
    body::{Body, Bytes},
    extract::{Request, State},
    http::{HeaderMap, Response, StatusCode},
    routing::post,
    Router,
};
use creavor_broker::{
    config::Config,
    interceptor::{openai_block_response_with_status, strip_session_header},
    proxy::{forward_upstream, ProxyTimeouts, UpstreamResponse},
    router::{provider_for_path, Provider},
    rule_engine::{scan_request, RuleSet},
};
use futures_core::Stream;
use futures_util::{FutureExt, TryStreamExt};
use http_body_util::BodyExt;
use hyper_util::{
    client::legacy::{connect::HttpConnector, Client},
    rt::TokioExecutor,
};
use serde_json::json;
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    net::TcpListener,
    sync::oneshot,
    task::JoinHandle,
};

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
        .uri(format!("{}/stream", state.upstream_base_url));
    if let Some(content_type) = headers.get("content-type") {
        upstream_request = upstream_request.header("content-type", content_type);
    }

    let forwarded = forward_upstream(
        async move {
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
            let body = upstream_response
                .into_body()
                .into_data_stream()
                .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> { Box::new(error) });

            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(UpstreamResponse::new(
                status, headers, body,
            ))
        },
        ProxyTimeouts::new(Duration::from_secs(5), Duration::from_secs(5)),
    )
    .await;

    forwarded.response
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

#[derive(Clone)]
struct UpstreamState {
    release_second: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
}

#[derive(Clone)]
struct GatedSseStream {
    state: Arc<Mutex<GatedSseState>>,
}

struct GatedSseState {
    phase: usize,
    release_second: Option<oneshot::Receiver<()>>,
}

impl GatedSseStream {
    fn new(release_second: oneshot::Receiver<()>) -> Self {
        Self {
            state: Arc::new(Mutex::new(GatedSseState {
                phase: 0,
                release_second: Some(release_second),
            })),
        }
    }
}

impl Stream for GatedSseStream {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut state = self.state.lock().unwrap();
        match state.phase {
            0 => {
                state.phase = 1;
                Poll::Ready(Some(Ok(Bytes::from_static(b"data: first\n\n"))))
            }
            1 => {
                let receiver = state.release_second.as_mut().expect("receiver present");
                match Pin::new(receiver).poll(cx) {
                    Poll::Ready(Ok(())) => {
                        state.phase = 2;
                        state.release_second = None;
                        Poll::Ready(Some(Ok(Bytes::from_static(b"data: second\n\n"))))
                    }
                    Poll::Ready(Err(_)) => {
                        state.phase = 2;
                        state.release_second = None;
                        Poll::Ready(Some(Err(std::io::Error::other("gate dropped"))))
                    }
                    Poll::Pending => Poll::Pending,
                }
            }
            _ => Poll::Ready(None),
        }
    }
}

async fn upstream_stream(State(state): State<UpstreamState>) -> Response<Body> {
    let release_second = state
        .release_second
        .lock()
        .unwrap()
        .take()
        .expect("stream gate should only be consumed once");
    let stream = GatedSseStream::new(release_second);
    let mut headers = HeaderMap::new();
    headers.insert("content-type", "text/event-stream".parse().unwrap());
    headers.insert("x-upstream-header", "kept".parse().unwrap());

    Response::builder()
        .status(StatusCode::OK)
        .body(Body::from_stream(stream))
        .map(|mut response| {
            *response.headers_mut() = headers;
            response
        })
        .unwrap()
}

fn upstream_app(release_second: oneshot::Receiver<()>) -> Router {
    Router::new()
        .route("/stream", post(upstream_stream))
        .with_state(UpstreamState {
            release_second: Arc::new(Mutex::new(Some(release_second))),
        })
}

async fn spawn_http_app(app: Router) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (format!("http://{addr}"), handle)
}

async fn send_stream_request(base_url: &str) -> hyper::Response<hyper::body::Incoming> {
    let client: Client<HttpConnector, Body> = Client::builder(TokioExecutor::new()).build_http();
    client
        .request(
            axum::http::Request::builder()
                .method("POST")
                .uri(format!("{base_url}/v1/openai/responses"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"input":"hello"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn allowed_streaming_request_passthroughs_sse_chunks() {
    let (release_second_tx, release_second_rx) = oneshot::channel();
    let (upstream_base_url, upstream_server) = spawn_http_app(upstream_app(release_second_rx)).await;

    let mut config = Config::default();
    config.broker.stream_passthrough = true;
    let (broker_base_url, broker_server) = spawn_http_app(proxy_app(config, upstream_base_url)).await;

    let response = send_stream_request(&broker_base_url).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "text/event-stream"
    );
    assert_eq!(response.headers().get("x-upstream-header").unwrap(), "kept");

    let mut body = response.into_body();
    let first = body.frame().await.unwrap().unwrap().into_data().unwrap();
    assert_eq!(first, Bytes::from_static(b"data: first\n\n"));

    let mut second = Box::pin(body.frame());
    tokio::task::yield_now().await;
    assert!(matches!(second.as_mut().now_or_never(), None));

    release_second_tx.send(()).unwrap();
    let second = second.await.unwrap().unwrap().into_data().unwrap();
    assert_eq!(second, Bytes::from_static(b"data: second\n\n"));

    broker_server.abort();
    upstream_server.abort();
}
