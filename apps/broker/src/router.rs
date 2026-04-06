use crate::{
    config::Config,
    events::{post_events, EventsState},
    storage::AuditStorage,
};
use axum::{routing::post, Router};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Anthropic,
    OpenAI,
}

pub fn provider_for_path(path: &str) -> Option<Provider> {
    if path == "/v1/anthropic" || path.starts_with("/v1/anthropic/") {
        return Some(Provider::Anthropic);
    }

    if path == "/v1/openai" || path.starts_with("/v1/openai/") {
        return Some(Provider::OpenAI);
    }

    None
}

pub fn app(config: Config, storage: AuditStorage) -> Router {
    Router::new()
        .route("/api/v1/events", post(post_events))
        .with_state(EventsState::new(config.audit.event_auth_token, storage))
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

    async fn spawn_app(config: Config, storage: AuditStorage) -> (String, JoinHandle<()>) {
        let app = app(config, storage);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        (format!("http://{addr}"), handle)
    }

    fn test_config() -> Config {
        let mut config = Config::default();
        config.audit.event_auth_token = Some("local-events-secret".to_string());
        config
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
                "SELECT event_type, request_id, payload
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
    fn ignores_unknown_paths() {
        assert_eq!(provider_for_path("/v1/other/messages"), None);
    }

    #[tokio::test]
    async fn events_missing_token_returns_unauthorized() {
        let path = unique_temp_path("missing-token");
        let storage = AuditStorage::open(&path).unwrap();
        let (base_url, server) = spawn_app(test_config(), storage).await;

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
        let (base_url, server) = spawn_app(test_config(), storage).await;

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
    async fn events_valid_token_returns_accepted_and_persists_sanitized_payload() {
        let path = unique_temp_path("valid-token");
        let storage = AuditStorage::open(&path).unwrap();
        let (base_url, server) = spawn_app(test_config(), storage).await;

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
        let (base_url, server) = spawn_app(test_config(), storage).await;

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
        let (base_url, server) = spawn_app(test_config(), storage).await;

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
        let (base_url, server) = spawn_app(test_config(), storage).await;

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
}
