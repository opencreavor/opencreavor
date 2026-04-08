use crate::{
    config::Settings,
    events::{post_events, EventsState},
    interceptor::{
        anthropic_block_response_with_status, gemini_block_response_with_status,
        openai_block_response_with_status, strip_creavor_headers,
    },
    proxy::{forward_upstream, BoxError, ProxyTimeouts, TerminalReason, UpstreamResponse},
    rule_engine::{scan_request, RuleSet},
    storage::AuditStorage,
};
use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, HeaderName, Response, StatusCode, Uri},
    routing::{get, post},
    Router,
};
use futures_util::TryStreamExt;
use http_body_util::BodyExt;
use hyper_tls::HttpsConnector;
use hyper_util::{
    client::legacy::Client,
    rt::TokioExecutor,
};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const CONTENT_LENGTH: HeaderName = HeaderName::from_static("content-length");
const HOST: HeaderName = HeaderName::from_static("host");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Anthropic,
    OpenAI,
    Gemini,
}

pub fn provider_for_path(path: &str) -> Option<Provider> {
    if path.starts_with("/v1/anthropic") {
        return Some(Provider::Anthropic);
    }
    if path.starts_with("/v1/openai") {
        return Some(Provider::OpenAI);
    }
    if path.starts_with("/v1/gemini") {
        return Some(Provider::Gemini);
    }
    None
}

fn provider_name(provider: Provider) -> &'static str {
    match provider {
        Provider::Anthropic => "anthropic",
        Provider::OpenAI => "openai",
        Provider::Gemini => "gemini",
    }
}

#[derive(Clone)]
struct AppState {
    settings: Settings,
    rules: RuleSet,
    storage: Arc<Mutex<AuditStorage>>,
    client: Client<HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>, Body>,
}

pub fn app(settings: Settings, storage: AuditStorage) -> Router {
    let shared_storage = Arc::new(Mutex::new(storage));
    let client: Client<HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>, Body> =
        Client::builder(TokioExecutor::new()).build(HttpsConnector::new());

    let events_state =
        EventsState::new(settings.audit.event_auth_token.clone(), shared_storage.clone());
    let state = AppState {
        settings,
        rules: RuleSet::builtin(),
        storage: shared_storage,
        client,
    };

    Router::new()
        .route("/api/v1/events", post(post_events))
        .with_state(events_state)
        .merge(
            Router::new()
                .route("/", get(health))
                .route("/health", get(health))
                .route("/v1/openai", post(proxy_request))
                .route("/v1/openai/{*path}", post(proxy_request))
                .route("/v1/anthropic", post(proxy_request))
                .route("/v1/anthropic/{*path}", post(proxy_request))
                .route("/v1/gemini", post(proxy_request))
                .route("/v1/gemini/{*path}", post(proxy_request))
                .with_state(state),
        )
}

async fn health() -> Response<Body> {
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .body(Body::from(r#"{"status":"ok","service":"creavor-broker"}"#))
        .expect("health response should be valid")
}

async fn proxy_request(State(state): State<AppState>, request: Request) -> Response<Body> {
    let provider = match provider_for_path(request.uri().path()) {
        Some(p) => p,
        None => return terminal_response(StatusCode::NOT_FOUND),
    };

    // 1. Read headers
    let runtime = request
        .headers()
        .get("x-creavor-runtime")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_string();
    let session_id = request
        .headers()
        .get("x-creavor-session-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // 2. Resolve upstream from settings
    let upstream_base = match state.settings.get_upstream(&runtime) {
        Some(url) => url.to_string(),
        None => {
            tracing::error!(runtime = %runtime, "no upstream configured for runtime");
            return terminal_response(StatusCode::BAD_GATEWAY);
        }
    };

    let method = request.method().clone();
    let request_path = request.uri().path().to_owned();
    let upstream_uri = upstream_uri(&upstream_base, provider, request.uri());

    tracing::info!(
        method = %method,
        path = %request_path,
        upstream = %upstream_uri,
        runtime = %runtime,
        "proxy request"
    );

    // 3. Strip Creavor headers before forwarding
    let mut headers = request.headers().clone();
    strip_creavor_headers(&mut headers);
    headers.remove(HOST);
    headers.remove(CONTENT_LENGTH);

    // 4. Read request body
    let request_body = request.into_body().collect().await.unwrap().to_bytes();
    let request_body_text = String::from_utf8(request_body.to_vec()).unwrap();
    let request_id = uuid::Uuid::new_v4().to_string();
    let start = Instant::now();

    // 5. Rule scan
    if let Some(rule_match) = scan_request(&request_body_text, &state.rules) {
        tracing::warn!(
            rule = %rule_match.rule_name,
            runtime = %runtime,
            "request blocked"
        );
        let message = format!(
            "Blocked by Creavor broker: {} ({})",
            rule_match.rule_name, rule_match.matched_content_sanitized
        );

        // Write audit for blocked request
        if let Ok(storage) = state.storage.lock() {
            let _ = storage.insert_request_start(
                &request_id,
                session_id.as_deref(),
                &runtime,
                provider_name(provider),
                method.as_str(),
                &request_path,
                true,
                Some(&message),
                Some(&rule_match.rule_id),
                Some(&rule_match.severity),
            );
            let _ = storage.insert_violation(
                &request_id,
                &rule_match.rule_id,
                &rule_match.rule_name,
                &rule_match.severity,
                &rule_match.matched_content_sanitized,
                "blocked",
            );
            let latency = start.elapsed().as_millis() as i64;
            let _ = storage.finalize_request(
                &request_id,
                TerminalReason::Ok,
                Some(state.settings.broker.block_status_code),
                Some(latency),
            );
        }

        return block_response(provider, &state.settings, &message);
    }

    // 6. Write audit for allowed request
    if let Ok(storage) = state.storage.lock() {
        let _ = storage.insert_request_start(
            &request_id,
            session_id.as_deref(),
            &runtime,
            provider_name(provider),
            method.as_str(),
            &request_path,
            false,
            None,
            None,
            None,
        );
        if state.settings.audit.store_request_payloads {
            let _ = storage.insert_request_payload(&request_id, &request_body_text);
        }
    }

    // 7. Forward upstream
    let upstream_request =
        build_upstream_request(method.clone(), upstream_uri.clone(), headers, request_body_text.clone());

    if state.settings.broker.stream_passthrough {
        let forwarded = forward_upstream(
            send_upstream(state.client.clone(), upstream_request),
            ProxyTimeouts::new(
                Duration::from_secs(state.settings.broker.upstream_timeout_secs),
                Duration::from_secs(state.settings.broker.idle_stream_timeout_secs),
            ),
        )
        .await;
        let status = forwarded.response.status();
        let latency = start.elapsed().as_millis() as i64;
        if let Ok(storage) = state.storage.lock() {
            let _ = storage.finalize_request(
                &request_id,
                TerminalReason::Ok,
                Some(status.as_u16()),
                Some(latency),
            );
        }
        tracing::info!(
            path = %request_path,
            status = %status,
            latency_ms = latency,
            "proxy completed (streaming)"
        );
        return forwarded.response;
    }

    // Buffered mode
    match state.client.request(upstream_request).await {
        Ok(upstream_response) => {
            let status = upstream_response.status();
            let latency = start.elapsed().as_millis() as i64;
            if let Ok(storage) = state.storage.lock() {
                let _ = storage.finalize_request(
                    &request_id,
                    TerminalReason::Ok,
                    Some(status.as_u16()),
                    Some(latency),
                );
            }
            tracing::info!(
                path = %request_path,
                status = %status,
                latency_ms = latency,
                "proxy completed (buffered)"
            );
            buffered_response(upstream_response).await
        }
        Err(e) => {
            tracing::error!(path = %request_path, error = %e, "upstream failed");
            if let Ok(storage) = state.storage.lock() {
                let latency = start.elapsed().as_millis() as i64;
                let _ = storage.finalize_request(
                    &request_id,
                    TerminalReason::NetworkError,
                    None,
                    Some(latency),
                );
            }
            terminal_response(StatusCode::BAD_GATEWAY)
        }
    }
}

fn block_response(provider: Provider, settings: &Settings, message: &str) -> Response<Body> {
    let status =
        StatusCode::from_u16(settings.broker.block_status_code).unwrap_or(StatusCode::BAD_REQUEST);
    match provider {
        Provider::Anthropic => anthropic_block_response_with_status(status, message),
        Provider::OpenAI => openai_block_response_with_status(status, message),
        Provider::Gemini => gemini_block_response_with_status(status, message),
    }
}

fn upstream_uri(base_url: &str, provider: Provider, uri: &Uri) -> String {
    let prefix = match provider {
        Provider::Anthropic => "/v1/anthropic",
        Provider::OpenAI => "/v1/openai",
        Provider::Gemini => "/v1/gemini",
    };
    let path_and_query = uri
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or(uri.path());
    let suffix = path_and_query.strip_prefix(prefix).unwrap_or(path_and_query);
    let suffix = if suffix.is_empty() { "/" } else { suffix };

    format!("{}{}", base_url.trim_end_matches('/'), suffix)
}

fn build_upstream_request(
    method: axum::http::Method,
    uri: String,
    headers: HeaderMap,
    body: String,
) -> Request {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::from(body))
        .expect("proxy upstream request should be valid");
    *request.headers_mut() = headers;
    request
}

async fn send_upstream(
    client: Client<HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>, Body>,
    request: Request,
) -> Result<UpstreamResponse, BoxError> {
    let upstream_response = client.request(request).await.map_err(box_error)?;
    let status = upstream_response.status();
    let headers = upstream_response.headers().clone();
    let body = upstream_response.into_body().into_data_stream().map_err(box_error);

    Ok(UpstreamResponse::new(status, headers, body))
}

async fn buffered_response(upstream_response: hyper::Response<hyper::body::Incoming>) -> Response<Body> {
    let status = upstream_response.status();
    let headers = upstream_response.headers().clone();
    let bytes = upstream_response
        .into_body()
        .collect()
        .await
        .map(|body| body.to_bytes())
        .unwrap_or_default();

    Response::builder()
        .status(status)
        .body(Body::from(bytes))
        .map(|mut response| {
            *response.headers_mut() = headers;
            response
        })
        .expect("buffered upstream response should be valid")
}

fn terminal_response(status: StatusCode) -> Response<Body> {
    Response::builder()
        .status(status)
        .body(Body::empty())
        .expect("synthetic terminal response should be valid")
}

fn box_error(error: impl Into<BoxError>) -> BoxError {
    error.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use hyper_util::{
        client::legacy::{connect::HttpConnector, Client},
        rt::TokioExecutor,
    };
    use serde_json::{json, Value};
    use std::{
        env, fs,
        net::SocketAddr,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };
    use tokio::{net::TcpListener, task::JoinHandle};

    async fn send_events_request(
        base_url: &str,
        token: Option<&str>,
        session_id: Option<&str>,
        body: Value,
    ) -> hyper::Response<hyper::body::Incoming> {
        let client: Client<HttpConnector, Body> =
            Client::builder(TokioExecutor::new()).build_http();
        let mut request = Request::builder()
            .method("POST")
            .uri(format!("{base_url}/api/v1/events"))
            .header("content-type", "application/json");

        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }

        if let Some(session_id) = session_id {
            request = request.header("x-creavor-session-id", session_id);
        }

        client
            .request(request.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap()
    }

    async fn spawn_app(settings: Settings, storage: AuditStorage) -> (String, JoinHandle<()>) {
        let app = app(settings, storage);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        (format!("http://{addr}"), handle)
    }

    fn test_settings() -> Settings {
        let mut settings = Settings::default();
        settings.audit.event_auth_token = Some("local-events-secret".to_string());
        settings
    }

    fn blank_token_settings() -> Settings {
        let mut settings = Settings::default();
        settings.audit.event_auth_token = Some("   ".to_string());
        settings
    }

    fn unique_temp_path(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        env::temp_dir().join(format!("creavor-broker-router-{name}-{nanos}.sqlite"))
    }

    fn persisted_event(path: &PathBuf) -> (String, Option<String>, Value) {
        let storage = AuditStorage::open(path).unwrap();
        storage
            .connection()
            .query_row(
                "SELECT event_type, session_id, payload
                 FROM events
                 ORDER BY id DESC
                 LIMIT 1",
                [],
                |row| {
                    let payload = row.get::<_, Option<String>>(2)?;
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        serde_json::from_str::<Value>(&payload.unwrap()).unwrap(),
                    ))
                },
            )
            .unwrap()
    }

    #[test]
    fn routes_anthropic_paths_to_anthropic() {
        assert_eq!(
            provider_for_path("/v1/anthropic/messages"),
            Some(Provider::Anthropic)
        );
    }

    #[test]
    fn routes_openai_paths_to_openai() {
        assert_eq!(
            provider_for_path("/v1/openai/responses"),
            Some(Provider::OpenAI)
        );
    }

    #[test]
    fn routes_gemini_paths_to_gemini() {
        assert_eq!(
            provider_for_path("/v1/gemini/generateContent"),
            Some(Provider::Gemini)
        );
    }

    #[test]
    fn ignores_unknown_paths() {
        assert_eq!(provider_for_path("/v1/other/messages"), None);
    }

    #[test]
    fn provider_name_returns_correct_string() {
        assert_eq!(provider_name(Provider::Anthropic), "anthropic");
        assert_eq!(provider_name(Provider::OpenAI), "openai");
        assert_eq!(provider_name(Provider::Gemini), "gemini");
    }

    #[test]
    fn proxy_upstream_uri_strips_provider_prefix() {
        let uri = Uri::from_static("/v1/openai/responses?stream=true");

        assert_eq!(
            upstream_uri("http://127.0.0.1:8080/", Provider::OpenAI, &uri),
            "http://127.0.0.1:8080/responses?stream=true"
        );
    }

    #[test]
    fn proxy_upstream_uri_strips_gemini_prefix() {
        let uri = Uri::from_static("/v1/gemini/models/gemini-pro:generateContent");

        assert_eq!(
            upstream_uri("https://generativelanguage.googleapis.com", Provider::Gemini, &uri),
            "https://generativelanguage.googleapis.com/models/gemini-pro:generateContent"
        );
    }

    #[tokio::test]
    async fn events_missing_token_returns_unauthorized() {
        let path = unique_temp_path("missing-token");
        let storage = AuditStorage::open(&path).unwrap();
        let (base_url, server) = spawn_app(test_settings(), storage).await;

        let response = send_events_request(
            &base_url,
            None,
            Some("session-123"),
            json!({"type":"editor.event","timestamp":"2026-04-07T00:00:00Z"}),
        )
        .await;

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        server.abort();
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn events_invalid_token_returns_unauthorized() {
        let path = unique_temp_path("invalid-token");
        let storage = AuditStorage::open(&path).unwrap();
        let (base_url, server) = spawn_app(test_settings(), storage).await;

        let response = send_events_request(
            &base_url,
            Some("wrong-secret"),
            Some("session-123"),
            json!({"type":"editor.event","timestamp":"2026-04-07T00:00:00Z"}),
        )
        .await;

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        server.abort();
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn events_blank_configured_token_still_rejects_bearer_request() {
        let path = unique_temp_path("blank-token");
        let storage = AuditStorage::open(&path).unwrap();
        let (base_url, server) = spawn_app(blank_token_settings(), storage).await;

        let response = send_events_request(
            &base_url,
            Some(""),
            Some("session-123"),
            json!({"type":"editor.event","timestamp":"2026-04-07T00:00:00Z"}),
        )
        .await;

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        server.abort();
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn events_valid_token_returns_accepted_and_persists_sanitized_payload() {
        let path = unique_temp_path("valid-token");
        let storage = AuditStorage::open(&path).unwrap();
        let (base_url, server) = spawn_app(test_settings(), storage).await;

        let response = send_events_request(
            &base_url,
            Some("local-events-secret"),
            Some("session-123"),
            json!({
                "type":"editor.event",
                "timestamp":"2026-04-07T00:00:00Z",
                "runtime":"codex",
                "cwd":"/tmp/demo"
            }),
        )
        .await;

        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            json!({"accepted":true})
        );
        server.abort();
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn events_prefer_session_header_for_correlation_and_do_not_store_auth_token() {
        let path = unique_temp_path("session-correlation");
        let storage = AuditStorage::open(&path).unwrap();
        let (base_url, server) = spawn_app(test_settings(), storage).await;

        let response = send_events_request(
            &base_url,
            Some("local-events-secret"),
            Some("session-abc"),
            json!({
                "type":"editor.event",
                "timestamp":"2026-04-07T00:00:00Z",
                "runtime":"codex",
                "cwd":"/tmp/demo"
            }),
        )
        .await;

        assert_eq!(response.status(), StatusCode::ACCEPTED);

        let (_, request_id, payload) = persisted_event(&path);
        assert_eq!(request_id.as_deref(), Some("session-abc"));
        assert_eq!(payload["correlation_id"], json!("session-abc"));
        assert_eq!(payload["type"], json!("editor.event"));
        assert_eq!(payload["runtime"], json!("codex"));
        assert!(payload.get("authorization").is_none());
        assert!(!payload.to_string().contains("local-events-secret"));
        server.abort();
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn events_fallback_correlation_uses_runtime_bucket_and_cwd() {
        let path = unique_temp_path("fallback-correlation");
        let storage = AuditStorage::open(&path).unwrap();
        let (base_url, server) = spawn_app(test_settings(), storage).await;

        let response = send_events_request(
            &base_url,
            Some("local-events-secret"),
            None,
            json!({
                "type":"editor.event",
                "timestamp":"2026-04-07T12:34:56.789Z",
                "runtime":"codex",
                "cwd":"/Users/norman/project"
            }),
        )
        .await;

        assert_eq!(response.status(), StatusCode::ACCEPTED);

        let (_, request_id, payload) = persisted_event(&path);
        assert_eq!(
            request_id.as_deref(),
            Some("codex:2026-04-07T12:34:00Z:project")
        );
        assert_eq!(
            payload["correlation_id"],
            json!("codex:2026-04-07T12:34:00Z:project")
        );
        server.abort();
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn events_rate_limit_returns_too_many_requests() {
        let path = unique_temp_path("rate-limit");
        let storage = AuditStorage::open(&path).unwrap();
        let (base_url, server) = spawn_app(test_settings(), storage).await;

        for _ in 0..32 {
            let response = send_events_request(
                &base_url,
                Some("local-events-secret"),
                Some("burst-session"),
                json!({"type":"editor.event","timestamp":"2026-04-07T00:00:00Z"}),
            )
            .await;
            assert_eq!(response.status(), StatusCode::ACCEPTED);
        }

        let response = send_events_request(
            &base_url,
            Some("local-events-secret"),
            Some("burst-session"),
            json!({"type":"editor.event","timestamp":"2026-04-07T00:00:00Z"}),
        )
        .await;

        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        server.abort();
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn events_rate_limit_isolated_between_distinct_correlation_keys() {
        let path = unique_temp_path("rate-limit-isolated");
        let storage = AuditStorage::open(&path).unwrap();
        let (base_url, server) = spawn_app(test_settings(), storage).await;

        for _ in 0..32 {
            let response = send_events_request(
                &base_url,
                Some("local-events-secret"),
                Some("burst-session-a"),
                json!({"type":"editor.event","timestamp":"2026-04-07T00:00:00Z"}),
            )
            .await;
            assert_eq!(response.status(), StatusCode::ACCEPTED);
        }

        let response = send_events_request(
            &base_url,
            Some("local-events-secret"),
            Some("burst-session-b"),
            json!({"type":"editor.event","timestamp":"2026-04-07T00:00:00Z"}),
        )
        .await;

        assert_eq!(response.status(), StatusCode::ACCEPTED);
        server.abort();
        let _ = fs::remove_file(path);
    }

    // -- Health endpoint tests --

    async fn send_health_request(
        base_url: &str,
        path: &str,
    ) -> hyper::Response<hyper::body::Incoming> {
        let client: Client<HttpConnector, Body> =
            Client::builder(TokioExecutor::new()).build_http();
        client
            .request(
                axum::http::Request::builder()
                    .uri(format!("{base_url}{path}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn health_root_returns_ok() {
        let path = unique_temp_path("health-root");
        let storage = AuditStorage::open(&path).unwrap();
        let (base_url, server) = spawn_app(test_settings(), storage).await;

        let response = send_health_request(&base_url, "/").await;
        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "ok");
        assert_eq!(json["service"], "creavor-broker");

        server.abort();
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn health_explicit_path_returns_ok() {
        let path = unique_temp_path("health-path");
        let storage = AuditStorage::open(&path).unwrap();
        let (base_url, server) = spawn_app(test_settings(), storage).await;

        let response = send_health_request(&base_url, "/health").await;
        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "ok");
        assert_eq!(json["service"], "creavor-broker");

        server.abort();
        let _ = fs::remove_file(path);
    }
}
