use axum::{
    body::{Body, Bytes},
    http::{HeaderMap, HeaderName, Response, StatusCode},
};
use futures_core::Stream;
use std::{
    error::Error,
    fmt,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::sync::oneshot;
use tokio::time::{self, Instant, Sleep};

pub type BoxError = Box<dyn Error + Send + Sync>;
pub type UpstreamBody = Pin<Box<dyn Stream<Item = Result<Bytes, BoxError>> + Send + 'static>>;

const CONTENT_LENGTH: HeaderName = HeaderName::from_static("content-length");
const CONNECTION: HeaderName = HeaderName::from_static("connection");
const KEEP_ALIVE: HeaderName = HeaderName::from_static("keep-alive");
const PROXY_AUTHENTICATE: HeaderName = HeaderName::from_static("proxy-authenticate");
const PROXY_AUTHORIZATION: HeaderName = HeaderName::from_static("proxy-authorization");
const TE: HeaderName = HeaderName::from_static("te");
const TRAILER: HeaderName = HeaderName::from_static("trailer");
const TRANSFER_ENCODING: HeaderName = HeaderName::from_static("transfer-encoding");
const UPGRADE: HeaderName = HeaderName::from_static("upgrade");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalReason {
    Ok,
    ClientCancelled,
    UpstreamTimeout,
    IdleTimeout,
    NetworkError,
}

impl TerminalReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::ClientCancelled => "client_cancelled",
            Self::UpstreamTimeout => "upstream_timeout",
            Self::IdleTimeout => "idle_timeout",
            Self::NetworkError => "network_error",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProxyTimeouts {
    pub upstream: Duration,
    pub idle: Duration,
}

impl ProxyTimeouts {
    pub fn new(upstream: Duration, idle: Duration) -> Self {
        Self { upstream, idle }
    }
}

pub struct UpstreamResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: UpstreamBody,
}

impl UpstreamResponse {
    pub fn new<S>(status: StatusCode, headers: HeaderMap, body: S) -> Self
    where
        S: Stream<Item = Result<Bytes, BoxError>> + Send + 'static,
    {
        Self {
            status,
            headers,
            body: Box::pin(body),
        }
    }
}

pub struct ForwardedResponse {
    pub response: Response<Body>,
    completion: CompletionHandle,
}

impl ForwardedResponse {
    pub fn into_parts(self) -> (Response<Body>, CompletionHandle) {
        (self.response, self.completion)
    }
}

pub struct CompletionHandle {
    receiver: oneshot::Receiver<TerminalReason>,
}

impl CompletionHandle {
    pub async fn reason(self) -> TerminalReason {
        self.receiver.await.unwrap_or(TerminalReason::NetworkError)
    }
}

#[derive(Debug)]
struct ProxyStreamError {
    reason: TerminalReason,
}

impl fmt::Display for ProxyStreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.reason.as_str())
    }
}

impl Error for ProxyStreamError {}

struct CompletionSignal {
    sender: Option<oneshot::Sender<TerminalReason>>,
}

impl CompletionSignal {
    fn new(sender: oneshot::Sender<TerminalReason>) -> Self {
        Self {
            sender: Some(sender),
        }
    }

    fn finish(&mut self, reason: TerminalReason) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(reason);
        }
    }
}

struct ProxyBodyStream {
    upstream: UpstreamBody,
    idle_timeout: Duration,
    idle_sleep: Pin<Box<Sleep>>,
    idle_armed: bool,
    finished: bool,
    completion: CompletionSignal,
}

impl ProxyBodyStream {
    fn new(upstream: UpstreamBody, idle_timeout: Duration, completion: CompletionSignal) -> Self {
        Self {
            upstream,
            idle_timeout,
            idle_sleep: Box::pin(time::sleep(Duration::ZERO)),
            idle_armed: false,
            finished: false,
            completion,
        }
    }

    fn finish(&mut self, reason: TerminalReason) {
        if !self.finished {
            self.finished = true;
            self.completion.finish(reason);
        }
    }
}

impl Stream for ProxyBodyStream {
    type Item = Result<Bytes, ProxyStreamError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.finished {
            return Poll::Ready(None);
        }

        if !self.idle_armed {
            let idle_timeout = self.idle_timeout;
            self.idle_sleep
                .as_mut()
                .reset(Instant::now() + idle_timeout);
            self.idle_armed = true;
        }

        match self.upstream.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(chunk))) => {
                self.idle_armed = false;
                Poll::Ready(Some(Ok(chunk)))
            }
            Poll::Ready(Some(Err(_err))) => {
                self.finish(TerminalReason::NetworkError);
                Poll::Ready(Some(Err(ProxyStreamError {
                    reason: TerminalReason::NetworkError,
                })))
            }
            Poll::Ready(None) => {
                self.finish(TerminalReason::Ok);
                Poll::Ready(None)
            }
            Poll::Pending => {
                if self.idle_sleep.as_mut().poll(cx).is_ready() {
                    self.finish(TerminalReason::IdleTimeout);
                    return Poll::Ready(Some(Err(ProxyStreamError {
                        reason: TerminalReason::IdleTimeout,
                    })));
                }

                Poll::Pending
            }
        }
    }
}

impl Drop for ProxyBodyStream {
    fn drop(&mut self) {
        if !self.finished {
            self.finish(TerminalReason::ClientCancelled);
        }
    }
}

pub async fn forward_upstream<F>(upstream: F, timeouts: ProxyTimeouts) -> ForwardedResponse
where
    F: Future<Output = Result<UpstreamResponse, BoxError>> + Send,
{
    let (tx, rx) = oneshot::channel();
    let completion = CompletionHandle { receiver: rx };

    match time::timeout(timeouts.upstream, upstream).await {
        Ok(Ok(upstream)) => {
            let mut response = Response::new(Body::from_stream(ProxyBodyStream::new(
                upstream.body,
                timeouts.idle,
                CompletionSignal::new(tx),
            )));
            *response.status_mut() = upstream.status;
            *response.headers_mut() = sanitize_forwarded_headers(upstream.headers);

            ForwardedResponse {
                response,
                completion,
            }
        }
        Ok(Err(_err)) => {
            let mut signal = CompletionSignal::new(tx);
            signal.finish(TerminalReason::NetworkError);
            ForwardedResponse {
                response: terminal_response(StatusCode::BAD_GATEWAY),
                completion,
            }
        }
        Err(_) => {
            let mut signal = CompletionSignal::new(tx);
            signal.finish(TerminalReason::UpstreamTimeout);
            ForwardedResponse {
                response: terminal_response(StatusCode::GATEWAY_TIMEOUT),
                completion,
            }
        }
    }
}

fn terminal_response(status: StatusCode) -> Response<Body> {
    Response::builder()
        .status(status)
        .body(Body::empty())
        .expect("synthetic terminal response should be valid")
}

fn sanitize_forwarded_headers(mut headers: HeaderMap) -> HeaderMap {
    headers.remove(CONTENT_LENGTH);
    headers.remove(CONNECTION);
    headers.remove(KEEP_ALIVE);
    headers.remove(PROXY_AUTHENTICATE);
    headers.remove(PROXY_AUTHORIZATION);
    headers.remove(TE);
    headers.remove(TRAILER);
    headers.remove(TRANSFER_ENCODING);
    headers.remove(UPGRADE);
    headers
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::FutureExt;
    use http_body_util::BodyExt;
    use std::{
        collections::VecDeque,
        io,
        sync::{Arc, Mutex},
    };

    #[derive(Clone)]
    struct MockChunkStream {
        state: Arc<Mutex<MockChunkState>>,
    }

    struct MockChunkState {
        steps: VecDeque<ChunkStep>,
        current_sleep: Option<Pin<Box<Sleep>>>,
    }

    #[derive(Clone, Copy)]
    enum ChunkStep {
        Ready(&'static [u8]),
        Delay {
            after: Duration,
            bytes: &'static [u8],
        },
        Error(&'static str),
        PendingForever,
        End,
    }

    impl MockChunkStream {
        fn new(steps: impl IntoIterator<Item = ChunkStep>) -> Self {
            Self {
                state: Arc::new(Mutex::new(MockChunkState {
                    steps: steps.into_iter().collect(),
                    current_sleep: None,
                })),
            }
        }
    }

    impl Stream for MockChunkStream {
        type Item = Result<Bytes, BoxError>;

        fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            let mut state = self.state.lock().unwrap();
            loop {
                let Some(step) = state.steps.front().copied() else {
                    return Poll::Ready(None);
                };

                match step {
                    ChunkStep::Ready(bytes) => {
                        let bytes = Bytes::from_static(bytes);
                        state.steps.pop_front();
                        return Poll::Ready(Some(Ok(bytes)));
                    }
                    ChunkStep::Delay { after, bytes } => {
                        if state.current_sleep.is_none() {
                            state.current_sleep =
                                Some(Box::pin(time::sleep_until(Instant::now() + after)));
                        }

                        let sleep = state.current_sleep.as_mut().unwrap();
                        if sleep.as_mut().poll(cx).is_pending() {
                            return Poll::Pending;
                        }

                        let bytes = Bytes::from_static(bytes);
                        state.current_sleep = None;
                        state.steps.pop_front();
                        return Poll::Ready(Some(Ok(bytes)));
                    }
                    ChunkStep::Error(message) => {
                        let err = io::Error::other(message);
                        state.steps.pop_front();
                        return Poll::Ready(Some(Err(Box::new(err))));
                    }
                    ChunkStep::PendingForever => return Poll::Pending,
                    ChunkStep::End => {
                        state.steps.pop_front();
                        return Poll::Ready(None);
                    }
                }
            }
        }
    }

    async fn frame_bytes(body: &mut Body) -> Result<Option<Bytes>, axum::Error> {
        match body.frame().await {
            Some(Ok(frame)) => Ok(frame.into_data().ok()),
            Some(Err(err)) => Err(err),
            None => Ok(None),
        }
    }

    fn passthrough_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("content-type", "text/event-stream".parse().unwrap());
        headers
    }

    fn upstream_headers_with_hop_by_hop_values() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("content-type", "text/event-stream".parse().unwrap());
        headers.insert("x-upstream-header", "kept".parse().unwrap());
        headers.insert("content-length", "123".parse().unwrap());
        headers.insert("connection", "keep-alive".parse().unwrap());
        headers.insert("keep-alive", "timeout=5".parse().unwrap());
        headers.insert("proxy-authenticate", "Basic realm=test".parse().unwrap());
        headers.insert("proxy-authorization", "Basic dGVzdA==".parse().unwrap());
        headers.insert("te", "trailers".parse().unwrap());
        headers.insert("trailer", "expires".parse().unwrap());
        headers.insert("transfer-encoding", "chunked".parse().unwrap());
        headers.insert("upgrade", "h2c".parse().unwrap());
        headers
    }

    #[tokio::test(start_paused = true)]
    async fn sse_chunks_are_forwarded_without_waiting_for_full_body() {
        let forwarded = forward_upstream(
            async {
                Ok(UpstreamResponse::new(
                    StatusCode::OK,
                    passthrough_headers(),
                    MockChunkStream::new([
                        ChunkStep::Ready(b"data: first\n\n"),
                        ChunkStep::Delay {
                            after: Duration::from_secs(10),
                            bytes: b"data: second\n\n",
                        },
                        ChunkStep::End,
                    ]),
                ))
            },
            ProxyTimeouts::new(Duration::from_secs(5), Duration::from_secs(30)),
        )
        .await;

        assert_eq!(forwarded.response.status(), StatusCode::OK);
        assert_eq!(
            forwarded.response.headers().get("content-type").unwrap(),
            "text/event-stream"
        );

        let (response, _completion) = forwarded.into_parts();
        let mut body = response.into_body();
        assert_eq!(
            frame_bytes(&mut body).await.unwrap().unwrap(),
            Bytes::from_static(b"data: first\n\n")
        );

        let mut second_frame = Box::pin(body.frame());
        tokio::task::yield_now().await;
        assert!(matches!(second_frame.as_mut().now_or_never(), None));

        time::advance(Duration::from_secs(10)).await;
        let second = second_frame.await.unwrap().unwrap().into_data().unwrap();
        assert_eq!(second, Bytes::from_static(b"data: second\n\n"));
    }

    #[tokio::test(start_paused = true)]
    async fn forwarded_response_strips_content_length_and_hop_by_hop_headers() {
        let forwarded = forward_upstream(
            async {
                Ok(UpstreamResponse::new(
                    StatusCode::OK,
                    upstream_headers_with_hop_by_hop_values(),
                    MockChunkStream::new([ChunkStep::Ready(b"chunk"), ChunkStep::End]),
                ))
            },
            ProxyTimeouts::new(Duration::from_secs(5), Duration::from_secs(5)),
        )
        .await;

        let headers = forwarded.response.headers();
        assert_eq!(headers.get("content-type").unwrap(), "text/event-stream");
        assert_eq!(headers.get("x-upstream-header").unwrap(), "kept");
        assert!(headers.get("content-length").is_none());
        assert!(headers.get("connection").is_none());
        assert!(headers.get("keep-alive").is_none());
        assert!(headers.get("proxy-authenticate").is_none());
        assert!(headers.get("proxy-authorization").is_none());
        assert!(headers.get("te").is_none());
        assert!(headers.get("trailer").is_none());
        assert!(headers.get("transfer-encoding").is_none());
        assert!(headers.get("upgrade").is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn completion_reason_is_ok_when_stream_finishes_cleanly() {
        let forwarded = forward_upstream(
            async {
                Ok(UpstreamResponse::new(
                    StatusCode::OK,
                    HeaderMap::new(),
                    MockChunkStream::new([ChunkStep::Ready(b"chunk"), ChunkStep::End]),
                ))
            },
            ProxyTimeouts::new(Duration::from_secs(5), Duration::from_secs(5)),
        )
        .await;

        let (response, completion) = forwarded.into_parts();
        let mut body = response.into_body();
        assert_eq!(
            frame_bytes(&mut body).await.unwrap().unwrap(),
            Bytes::from_static(b"chunk")
        );
        assert!(frame_bytes(&mut body).await.unwrap().is_none());
        assert_eq!(completion.reason().await, TerminalReason::Ok);
    }

    #[tokio::test(start_paused = true)]
    async fn client_cancelled_is_reported_when_body_is_dropped_early() {
        let forwarded = forward_upstream(
            async {
                Ok(UpstreamResponse::new(
                    StatusCode::OK,
                    HeaderMap::new(),
                    MockChunkStream::new([
                        ChunkStep::Ready(b"chunk"),
                        ChunkStep::Delay {
                            after: Duration::from_secs(30),
                            bytes: b"later",
                        },
                    ]),
                ))
            },
            ProxyTimeouts::new(Duration::from_secs(5), Duration::from_secs(60)),
        )
        .await;

        let (response, completion) = forwarded.into_parts();
        let mut body = response.into_body();
        assert_eq!(
            frame_bytes(&mut body).await.unwrap().unwrap(),
            Bytes::from_static(b"chunk")
        );
        drop(body);

        assert_eq!(completion.reason().await, TerminalReason::ClientCancelled);
    }

    #[tokio::test(start_paused = true)]
    async fn upstream_timeout_is_reported_when_response_headers_never_arrive() {
        let forwarded = forward_upstream(
            async {
                time::sleep(Duration::from_secs(10)).await;
                Ok(UpstreamResponse::new(
                    StatusCode::OK,
                    HeaderMap::new(),
                    MockChunkStream::new([ChunkStep::End]),
                ))
            },
            ProxyTimeouts::new(Duration::from_secs(5), Duration::from_secs(5)),
        )
        .await;

        assert_eq!(forwarded.response.status(), StatusCode::GATEWAY_TIMEOUT);
        let (_response, completion) = forwarded.into_parts();
        assert_eq!(completion.reason().await, TerminalReason::UpstreamTimeout);
    }

    #[tokio::test(start_paused = true)]
    async fn idle_timeout_is_reported_when_stream_stalls_between_chunks() {
        let forwarded = forward_upstream(
            async {
                Ok(UpstreamResponse::new(
                    StatusCode::OK,
                    HeaderMap::new(),
                    MockChunkStream::new([ChunkStep::Ready(b"chunk"), ChunkStep::PendingForever]),
                ))
            },
            ProxyTimeouts::new(Duration::from_secs(5), Duration::from_secs(3)),
        )
        .await;

        let (response, completion) = forwarded.into_parts();
        let mut body = response.into_body();
        assert_eq!(
            frame_bytes(&mut body).await.unwrap().unwrap(),
            Bytes::from_static(b"chunk")
        );

        let mut next_frame = Box::pin(body.frame());
        tokio::task::yield_now().await;
        assert!(matches!(next_frame.as_mut().now_or_never(), None));
        time::advance(Duration::from_secs(3)).await;
        assert!(next_frame.await.unwrap().is_err());
        assert_eq!(completion.reason().await, TerminalReason::IdleTimeout);
    }

    #[tokio::test(start_paused = true)]
    async fn network_error_is_reported_when_upstream_body_errors() {
        let forwarded = forward_upstream(
            async {
                Ok(UpstreamResponse::new(
                    StatusCode::OK,
                    HeaderMap::new(),
                    MockChunkStream::new([ChunkStep::Error("boom")]),
                ))
            },
            ProxyTimeouts::new(Duration::from_secs(5), Duration::from_secs(5)),
        )
        .await;

        let (response, completion) = forwarded.into_parts();
        let mut body = response.into_body();
        assert!(frame_bytes(&mut body).await.is_err());
        assert_eq!(completion.reason().await, TerminalReason::NetworkError);
    }

    #[tokio::test(start_paused = true)]
    async fn network_error_is_reported_when_connect_fails() {
        let forwarded = forward_upstream(
            async { Err::<UpstreamResponse, BoxError>(Box::new(io::Error::other("connect"))) },
            ProxyTimeouts::new(Duration::from_secs(5), Duration::from_secs(5)),
        )
        .await;

        assert_eq!(forwarded.response.status(), StatusCode::BAD_GATEWAY);
        let (_response, completion) = forwarded.into_parts();
        assert_eq!(completion.reason().await, TerminalReason::NetworkError);
    }
}
