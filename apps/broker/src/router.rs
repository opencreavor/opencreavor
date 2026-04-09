use crate::{
    config::Settings,
    events::{post_events, EventsState},
    interceptor::{
        anthropic_block_response_with_status, gemini_block_response_with_status,
        openai_block_response_with_status, strip_creavor_headers,
    },
    path_rewrite::{normalize_join, parse_request_path},
    proxy::{forward_upstream, BoxError, ProxyTimeouts, TerminalReason, UpstreamResponse},
    rule_engine::{scan_request, RuleSet},
    storage::AuditStorage,
};
use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, HeaderName, Response, StatusCode},
    routing::{get, post},
    Router,
};
use creavor_core::{SessionRegistry, resolve_upstream};
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

fn provider_from_protocol(protocol: &str) -> Option<Provider> {
    match protocol {
        "anthropic" => Some(Provider::Anthropic),
        "openai" => Some(Provider::OpenAI),
        "gemini" => Some(Provider::Gemini),
        _ => None,
    }
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
    session_registry: Arc<Mutex<SessionRegistry>>,
}

pub fn app(settings: Settings, storage: AuditStorage) -> Router {
    let shared_storage = Arc::new(Mutex::new(storage));
    let client: Client<HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>, Body> =
        Client::builder(TokioExecutor::new()).build(HttpsConnector::new());

    let events_state =
        EventsState::new(settings.audit.event_auth_token.clone(), shared_storage.clone());
    let rules = if let Some(ref rules_dir) = settings.rules.rules_dir {
        let path = std::path::Path::new(rules_dir);
        if path.is_dir() {
            RuleSet::builtin_with_custom_dir(path)
        } else {
            tracing::warn!("rules_dir '{}' not found, using builtin rules only", rules_dir);
            RuleSet::builtin()
        }
    } else {
        RuleSet::builtin()
    };

    let state = AppState {
        settings,
        rules,
        storage: shared_storage,
        client,
        session_registry: Arc::new(Mutex::new(SessionRegistry::new())),
    };

    Router::new()
        .route("/api/v1/events", post(post_events))
        .with_state(events_state)
        .merge(
            Router::new()
                .route("/", get(health))
                .route("/health", get(health))
                // Legacy routes: /v1/{provider}/* (no upstream-id in path)
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
    let request_path = request.uri().path().to_owned();

    // 1. Parse path using new path_rewrite logic
    let parsed = match parse_request_path(&request_path) {
        Some(p) => p,
        None => return terminal_response(StatusCode::NOT_FOUND),
    };

    let provider = match provider_from_protocol(&parsed.protocol) {
        Some(p) => p,
        None => return terminal_response(StatusCode::NOT_FOUND),
    };

    // 2. Read headers
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
    let header_upstream = request
        .headers()
        .get("x-creavor-upstream")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // 3. Resolve upstream using the priority chain from design document
    let resolved = {
        let session_reg = state.session_registry.lock().unwrap();
        resolve_upstream(
            header_upstream.as_deref(),
            session_id.as_deref(),
            Some(&parsed.protocol),
            Some(&runtime),
            &state.settings.upstream_registry,
            &session_reg,
            &state.settings.upstream,
        )
    };

    // Fallback to legacy upstream resolution if new registry doesn't resolve
    let upstream_base = match resolved {
        Some(ref r) => r.entry.upstream.clone(),
        None => {
            // Legacy fallback: use runtime→URL mapping
            // Try runtime name first (e.g. "claude-code"), then provider name
            // (e.g. "anthropic"), then any first configured upstream as default.
            match state.settings.get_upstream(&runtime)
                .or_else(|| state.settings.get_upstream(provider_name(provider)))
                .or_else(|| state.settings.first_upstream())
            {
                Some(url) => url.to_string(),
                None => {
                    tracing::error!(runtime = %runtime, "no upstream configured for runtime");
                    return terminal_response(StatusCode::BAD_GATEWAY);
                }
            }
        }
    };

    // 4. Determine the tail path for URL construction
    let tail = match &parsed.upstream_id {
        Some(_) => {
            // Full path: tail already has the correct path after stripping upstream-id
            parsed.tail.clone()
        }
        None => {
            // Simplified path (legacy): strip the protocol prefix to get the API path
            let prefix = format!("/v1/{}", parsed.protocol);
            let path_and_query = request.uri()
                .path_and_query()
                .map(|v| v.as_str())
                .unwrap_or(&request_path);
            let suffix = path_and_query.strip_prefix(&prefix).unwrap_or(path_and_query);
            if suffix.is_empty() { "/".to_string() } else { suffix.to_string() }
        }
    };

    let method = request.method().clone();

    // Build the upstream URI using normalize_join
    let upstream_uri = normalize_join(&upstream_base, &tail);

    tracing::info!(
        method = %method,
        path = %request_path,
        upstream = %upstream_uri,
        runtime = %runtime,
        upstream_id = ?parsed.upstream_id.or_else(|| resolved.as_ref().map(|r| r.upstream_id.clone())),
        "proxy request"
    );

    // 5. Strip Creavor headers before forwarding
    let mut headers = request.headers().clone();
    strip_creavor_headers(&mut headers);
    headers.remove(HOST);
    headers.remove(CONTENT_LENGTH);

    // 6. Read request body
    let request_body = request.into_body().collect().await.unwrap().to_bytes();
    let request_body_text = String::from_utf8(request_body.to_vec()).unwrap();
    let request_id = uuid::Uuid::new_v4().to_string();
    let start = Instant::now();

    // 7. Rule scan with risk-level handling
    if let Some(rule_match) = scan_request(&request_body_text, &state.rules) {
        let severity = &rule_match.severity;
        let risk_level = risk_level_from_severity(severity);

        let message = format!(
            "Blocked by Creavor broker: {} ({})",
            rule_match.rule_name, rule_match.matched_content_sanitized
        );

        // Risk-level handling per design document:
        // - Critical: direct block, no approval
        // - High/Medium: block + create approval_request for Guard
        // - Low: allow with logging (no block)
        if risk_level == "low" {
            // Low risk: allow but log
            tracing::info!(
                rule = %rule_match.rule_name,
                runtime = %runtime,
                severity = %severity,
                "low-risk match, allowing with logging"
            );
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
                let _ = storage.insert_violation(
                    &request_id,
                    &rule_match.rule_id,
                    &rule_match.rule_name,
                    &rule_match.severity,
                    &rule_match.matched_content_sanitized,
                    "logged",
                );
            }
            // Fall through to forward the request
        } else {
            // Critical/High/Medium: block
            tracing::warn!(
                rule = %rule_match.rule_name,
                runtime = %runtime,
                severity = %severity,
                risk_level = %risk_level,
                "request blocked"
            );

            let blocked = true;
            if let Ok(storage) = state.storage.lock() {
                let _ = storage.insert_request_start(
                    &request_id,
                    session_id.as_deref(),
                    &runtime,
                    provider_name(provider),
                    method.as_str(),
                    &request_path,
                    blocked,
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

                // For High/Medium: create approval_request so Guard can review
                if risk_level == "high" || risk_level == "medium" {
                    let approval_id = uuid::Uuid::new_v4().to_string();
                    let expires_secs = state.settings.guard.approval_timeout_secs;
                    let expires_at = format_expires_at(expires_secs);
                    let _ = storage.insert_approval_request(
                        &approval_id,
                        &request_id,
                        session_id.as_deref(),
                        &runtime,
                        resolved.as_ref().map(|r| r.upstream_id.as_str()),
                        &risk_level,
                        &rule_match.rule_id,
                        &rule_match.matched_content_sanitized,
                        "pending",
                        Some(&expires_at),
                    );
                    tracing::info!(
                        approval_id = %approval_id,
                        risk_level = %risk_level,
                        "approval request created for Guard review"
                    );
                }

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
    }

    // 8. Write audit for allowed request
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

    // 9. Forward upstream
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

/// Map severity string to risk level category.
fn risk_level_from_severity(severity: &str) -> &'static str {
    match severity.to_ascii_lowercase().as_str() {
        "critical" => "critical",
        "high" => "high",
        "medium" => "medium",
        "low" => "low",
        _ => "low",
    }
}

/// Generate an ISO-8601 expiry timestamp from now + timeout seconds.
fn format_expires_at(timeout_secs: u64) -> String {
    // Simple epoch-based expiry. In production, use a proper datetime library.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("{}", now + timeout_secs)
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

    // -- Provider routing tests --

    #[test]
    fn provider_from_protocol_returns_correct_provider() {
        assert_eq!(provider_from_protocol("anthropic"), Some(Provider::Anthropic));
        assert_eq!(provider_from_protocol("openai"), Some(Provider::OpenAI));
        assert_eq!(provider_from_protocol("gemini"), Some(Provider::Gemini));
    }

    #[test]
    fn provider_from_protocol_ignores_unknown() {
        assert_eq!(provider_from_protocol("unknown"), None);
    }

    #[test]
    fn provider_name_returns_correct_string() {
        assert_eq!(provider_name(Provider::Anthropic), "anthropic");
        assert_eq!(provider_name(Provider::OpenAI), "openai");
        assert_eq!(provider_name(Provider::Gemini), "gemini");
    }

    // -- Events endpoint tests (preserved from original) --

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
        let json: Value = serde_json::from_slice::<Value>(&body).unwrap();
        assert_eq!(json["status"], "ok");
        assert_eq!(json["service"], "creavor-broker");

        server.abort();
        let _ = fs::remove_file(path);
    }

    fn test_settings_with_upstream() -> Settings {
        let mut settings = test_settings();
        settings
            .upstream
            .insert("claude-code".to_string(), "https://api.anthropic.com".to_string());
        settings
            .upstream
            .insert("codex".to_string(), "https://api.openai.com".to_string());
        settings
    }

    // -- Sensitive content blocking integration tests --

    async fn send_proxy_request(
        base_url: &str,
        path: &str,
        body: &str,
        runtime: &str,
        session_id: Option<&str>,
    ) -> hyper::Response<hyper::body::Incoming> {
        let client: Client<HttpConnector, Body> =
            Client::builder(TokioExecutor::new()).build_http();
        let mut request = Request::builder()
            .method("POST")
            .uri(format!("{base_url}{path}"))
            .header("content-type", "application/json")
            .header("x-creavor-runtime", runtime);

        if let Some(sid) = session_id {
            request = request.header("x-creavor-session-id", sid);
        }

        client
            .request(request.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn request_with_profanity_gets_blocked_with_anthropic_error_format() {
        let path = unique_temp_path("profanity-block");
        let storage = AuditStorage::open(&path).unwrap();
        let (base_url, server) = spawn_app(test_settings_with_upstream(), storage).await;

        let body = serde_json::json!({
            "model": "claude-3-opus-20240229",
            "max_tokens": 1024,
            "messages": [{"role": "user", "content": "What the fuck is going on?"}]
        })
        .to_string();

        let response =
            send_proxy_request(&base_url, "/v1/anthropic/messages", &body, "claude-code", Some("session-test")).await;

        // Should be blocked (default block_status_code is 400)
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let resp_body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&resp_body).unwrap();

        // Anthropic error format
        assert_eq!(json["type"], "error");
        assert!(json["error"]["message"].as_str().unwrap().contains("Blocked by Creavor broker"));

        // Verify DB records
        let storage = AuditStorage::open(&path).unwrap();
        let req = storage.connection()
            .query_row(
                "SELECT blocked, block_reason FROM requests WHERE runtime = 'claude-code'",
                [],
                |row| Ok((row.get::<_, bool>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .unwrap();
        assert!(req.0, "request should be marked as blocked");
        assert!(req.1.unwrap().contains("Profanity"));

        let violation = storage.connection()
            .query_row(
                "SELECT rule_id, severity, action FROM violations WHERE rule_id = 'profanity-en-001'",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)),
            )
            .unwrap();
        assert_eq!(violation.0, "profanity-en-001");
        assert_eq!(violation.1, "medium");
        assert_eq!(violation.2, "blocked");

        let approval = storage.connection()
            .query_row(
                "SELECT status, risk_level FROM approval_requests WHERE rule_id = 'profanity-en-001'",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .unwrap();
        assert_eq!(approval.0, "pending");
        assert_eq!(approval.1, "medium");

        server.abort();
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn request_with_api_key_gets_blocked_and_creates_approval() {
        let path = unique_temp_path("apikey-block");
        let storage = AuditStorage::open(&path).unwrap();
        let (base_url, server) = spawn_app(test_settings_with_upstream(), storage).await;

        let body = serde_json::json!({
            "model": "claude-3-opus-20240229",
            "max_tokens": 1024,
            "messages": [{"role": "user", "content": "my key is sk-1234567890abcdef123456"}]
        })
        .to_string();

        let response =
            send_proxy_request(&base_url, "/v1/anthropic/messages", &body, "claude-code", Some("session-apikey")).await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let resp_body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&resp_body).unwrap();
        assert!(json["error"]["message"].as_str().unwrap().contains("OpenAI API Key"));

        let storage = AuditStorage::open(&path).unwrap();
        let approval = storage.connection()
            .query_row(
                "SELECT risk_level, status FROM approval_requests WHERE risk_level = 'high'",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .unwrap();
        assert_eq!(approval.0, "high");
        assert_eq!(approval.1, "pending");

        server.abort();
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn request_with_email_passes_after_rule_removal() {
        let path = unique_temp_path("email-pass");
        let storage = AuditStorage::open(&path).unwrap();
        let (base_url, server) = spawn_app(test_settings_with_upstream(), storage).await;

        let body = serde_json::json!({
            "model": "gpt-4",
            "messages": [{"role": "user", "content": "Contact alice@example.com"}]
        })
        .to_string();

        let response =
            send_proxy_request(&base_url, "/v1/openai/chat/completions", &body, "codex", None).await;

        // Email rule was removed — should NOT be blocked (502 = upstream unreachable, not 400 block)
        assert_ne!(response.status(), StatusCode::BAD_REQUEST, "email should not be blocked after rule removal");

        // Verify no violation was recorded
        let storage = AuditStorage::open(&path).unwrap();
        let count: i64 = storage.connection()
            .query_row(
                "SELECT COUNT(*) FROM violations",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "no violations should be recorded for email");

        server.abort();
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn clean_request_passes_without_blocking() {
        let path = unique_temp_path("clean-pass");
        let storage = AuditStorage::open(&path).unwrap();
        let (base_url, server) = spawn_app(test_settings_with_upstream(), storage).await;

        let body = serde_json::json!({
            "model": "claude-3-opus-20240229",
            "max_tokens": 1024,
            "messages": [{"role": "user", "content": "Hello, how are you today?"}]
        })
        .to_string();

        let response =
            send_proxy_request(&base_url, "/v1/anthropic/messages", &body, "claude-code", None).await;

        let status = response.status();
        let resp_body = response.into_body().collect().await.unwrap().to_bytes();
        let body_str = String::from_utf8_lossy(&resp_body);

        // Should NOT be 400 (blocked). Will be 502 (upstream unreachable) or similar network error.
        // Just verify it's not a block response.
        assert_ne!(
            status,
            StatusCode::BAD_REQUEST,
            "clean request should not be blocked. Got status {status}, body: {body_str}"
        );

        // Verify no violation was recorded (DB may be empty or request not blocked)
        let storage = AuditStorage::open(&path).unwrap();
        let blocked_count: i64 = storage.connection()
            .query_row(
                "SELECT COUNT(*) FROM violations",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(blocked_count, 0, "clean request should have no violations");

        server.abort();
        let _ = fs::remove_file(path);
    }

    #[test]
    fn risk_level_mapping_is_correct() {
        assert_eq!(risk_level_from_severity("critical"), "critical");
        assert_eq!(risk_level_from_severity("high"), "high");
        assert_eq!(risk_level_from_severity("medium"), "medium");
        assert_eq!(risk_level_from_severity("low"), "low");
        assert_eq!(risk_level_from_severity("unknown"), "low");
        assert_eq!(risk_level_from_severity("High"), "high");
    }
}
