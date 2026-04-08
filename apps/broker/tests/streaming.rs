use axum::{
    body::{Body, Bytes},
    extract::State,
    http::{HeaderMap, Response, StatusCode},
    routing::post,
    Router,
};
use creavor_broker::{config::Settings, router, storage::AuditStorage};
use futures_core::Stream;
use futures_util::FutureExt;
use http_body_util::BodyExt;
use hyper_util::{
    client::legacy::{connect::HttpConnector, Client},
    rt::TokioExecutor,
};
use serde_json::json;
use std::{
    collections::HashMap,
    future::Future,
    net::SocketAddr,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};
use tokio::{
    net::TcpListener,
    sync::oneshot,
    task::JoinHandle,
};

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
    let addr: SocketAddr = listener.local_addr().unwrap();
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
                .uri(format!("{base_url}/v1/openai/stream"))
                .header("content-type", "application/json")
                .header("x-creavor-runtime", "test-runtime")
                .body(Body::from(json!({"input":"hello"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn p0_allowed_streaming_request_passthroughs_sse_chunks() {
    let (release_second_tx, release_second_rx) = oneshot::channel();
    let (upstream_base_url, upstream_server) =
        spawn_http_app(upstream_app(release_second_rx)).await;

    let mut settings = Settings::default();
    settings.broker.stream_passthrough = true;
    settings.upstream = HashMap::from([("test-runtime".to_string(), upstream_base_url)]);
    let storage = AuditStorage::open_in_memory().unwrap();
    let (broker_base_url, broker_server) =
        spawn_http_app(router::app(settings, storage)).await;

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
