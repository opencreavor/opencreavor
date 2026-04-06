use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use creavor_broker::{config::Config, router::app, storage::AuditStorage};
use http_body_util::BodyExt;
use hyper_util::{
    client::legacy::{connect::HttpConnector, Client},
    rt::TokioExecutor,
};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::{
    env, fs,
    net::SocketAddr,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::{net::TcpListener, task::JoinHandle};

fn unique_temp_path(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    env::temp_dir().join(format!("creavor-broker-events-auth-{name}-{nanos}.sqlite"))
}

async fn spawn_http_app(config: Config, storage: AuditStorage) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app(config, storage)).await.unwrap();
    });

    (format!("http://{addr}"), handle)
}

async fn send_events_request(
    base_url: &str,
    token: Option<&str>,
    payload: Value,
) -> hyper::Response<hyper::body::Incoming> {
    let client: Client<HttpConnector, Body> = Client::builder(TokioExecutor::new()).build_http();
    let mut request = Request::builder()
        .method("POST")
        .uri(format!("{base_url}/api/v1/events"))
        .header("content-type", "application/json");

    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }

    client
        .request(request.body(Body::from(payload.to_string())).unwrap())
        .await
        .unwrap()
}

fn persisted_events(path: &PathBuf) -> Vec<(String, Option<String>, String)> {
    let connection = Connection::open(path).unwrap();
    let mut statement = connection
        .prepare(
            "SELECT event_type, request_id, payload
             FROM events
             ORDER BY id ASC",
        )
        .unwrap();

    statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

#[tokio::test]
async fn p0_event_ingestion_requires_valid_bearer_token_before_persisting() {
    let path = unique_temp_path("acceptance");
    let storage = AuditStorage::open(&path).unwrap();
    let mut config = Config::default();
    config.audit.event_auth_token = Some("local-events-secret".to_string());
    let (base_url, server) = spawn_http_app(config, storage).await;

    let unauthorized = send_events_request(
        &base_url,
        None,
        json!({
            "type":"editor.event",
            "timestamp":"2026-04-07T00:00:00Z",
            "runtime":"codex",
            "cwd":"/tmp/demo"
        }),
    )
    .await;
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    let unauthorized_body = unauthorized.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        serde_json::from_slice::<Value>(&unauthorized_body).unwrap(),
        json!({"error":"unauthorized"})
    );
    assert!(persisted_events(&path).is_empty());

    let authorized = send_events_request(
        &base_url,
        Some("local-events-secret"),
        json!({
            "type":"editor.event",
            "timestamp":"2026-04-07T00:00:00Z",
            "runtime":"codex",
            "cwd":"/tmp/demo",
            "authorization":"Bearer should-not-persist"
        }),
    )
    .await;
    assert_eq!(authorized.status(), StatusCode::ACCEPTED);
    let authorized_body = authorized.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        serde_json::from_slice::<Value>(&authorized_body).unwrap(),
        json!({"accepted":true})
    );

    let persisted = persisted_events(&path);
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].0, "editor.event");
    let payload = serde_json::from_str::<Value>(&persisted[0].2).unwrap();
    assert_eq!(payload["correlation_id"], json!("codex:2026-04-07T00:00:00Z:demo"));
    assert!(payload.get("authorization").is_none());

    server.abort();
    let _ = fs::remove_file(path);
}
