use crate::{proxy::TerminalReason, storage::AuditStorage};
use anyhow::Context;
use axum::http::HeaderMap;
use rusqlite::params;
use serde_json::Value;
use std::path::Path;
use time::{format_description::well_known::Rfc3339, macros::format_description, OffsetDateTime};

const SESSION_HEADER: &str = "x-creavor-session-id";

impl AuditStorage {
    pub fn insert_event(
        &self,
        session_id: Option<&str>,
        event_type: &str,
        tool_name: Option<&str>,
        payload: Option<&str>,
        source: Option<&str>,
    ) -> anyhow::Result<i64> {
        self.connection().execute(
            "INSERT INTO events (session_id, event_type, tool_name, payload, source)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![session_id, event_type, tool_name, payload, source],
        )?;

        Ok(self.connection().last_insert_rowid())
    }

    pub fn insert_request_start(
        &self,
        request_id: &str,
        session_id: Option<&str>,
        runtime: &str,
        provider: &str,
        method: &str,
        path: &str,
        blocked: bool,
        block_reason: Option<&str>,
        rule_id: Option<&str>,
        severity: Option<&str>,
    ) -> anyhow::Result<()> {
        self.connection().execute(
            "INSERT INTO requests (request_id, session_id, runtime, provider, method, path,
                                  blocked, block_reason, rule_id, severity)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                request_id,
                session_id,
                runtime,
                provider,
                method,
                path,
                blocked,
                block_reason,
                rule_id,
                severity,
            ],
        )?;

        Ok(())
    }

    pub fn insert_request_payload(
        &self,
        request_id: &str,
        body: &str,
    ) -> anyhow::Result<()> {
        self.connection()
            .execute(
                "INSERT INTO request_payloads (request_id, body)
                 VALUES (?1, ?2)",
                params![request_id, body],
            )
            .with_context(|| format!("failed to insert request payload for {request_id}"))?;

        Ok(())
    }

    pub fn finalize_request(
        &self,
        request_id: &str,
        terminal_reason: TerminalReason,
        response_status: Option<u16>,
        latency_ms: Option<i64>,
    ) -> anyhow::Result<()> {
        let transaction = self.connection().unchecked_transaction()?;
        let changed = transaction.execute(
            "UPDATE requests
             SET terminal_reason = ?2,
                 response_status = ?3,
                 latency_ms = ?4,
                 completed_at = CURRENT_TIMESTAMP
             WHERE request_id = ?1
               AND completed_at IS NULL",
            params![
                request_id,
                terminal_reason.as_str(),
                response_status.map(i64::from),
                latency_ms,
            ],
        )?;

        if changed == 0 {
            let request_state = transaction.query_row(
                "SELECT completed_at IS NOT NULL
                 FROM requests
                 WHERE request_id = ?1",
                [request_id],
                |row| row.get::<_, i64>(0),
            );
            match request_state {
                Ok(1) => anyhow::bail!("request already finalized: {request_id}"),
                Ok(_) => anyhow::bail!("cannot finalize missing request: {request_id}"),
                Err(rusqlite::Error::QueryReturnedNoRows) => {
                    anyhow::bail!("cannot finalize missing request: {request_id}")
                }
                Err(error) => return Err(error.into()),
            }
        }

        transaction.commit()?;
        Ok(())
    }

    pub fn insert_response_payload(
        &self,
        request_id: &str,
        body: &str,
    ) -> anyhow::Result<()> {
        self.connection()
            .execute(
                "INSERT INTO response_payloads (request_id, body)
                 VALUES (?1, ?2)
                 ON CONFLICT(request_id) DO UPDATE SET body = excluded.body",
                params![request_id, body],
            )
            .with_context(|| format!("failed to insert response payload for {request_id}"))?;

        Ok(())
    }

    pub fn insert_violation(
        &self,
        request_id: &str,
        rule_id: &str,
        rule_name: &str,
        severity: &str,
        matched_content: &str,
        action: &str,
    ) -> anyhow::Result<i64> {
        self.connection()
            .execute(
                "INSERT INTO violations (request_id, rule_id, rule_name, severity, matched_content, action)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![request_id, rule_id, rule_name, severity, matched_content, action],
            )
            .with_context(|| format!("failed to insert violation for request {request_id}"))?;

        Ok(self.connection().last_insert_rowid())
    }
}

pub fn event_type_from_payload(payload: &Value) -> &str {
    payload
        .get("type")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("local.event")
}

pub fn correlation_id_for_event(headers: &HeaderMap, payload: &Value) -> String {
    if let Some(session_id) = headers
        .get(SESSION_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
    {
        return session_id.to_string();
    }

    let runtime = payload
        .get("runtime")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown");
    let bucket = payload
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(timestamp_bucket)
        .unwrap_or("unknown-time".to_string());
    let cwd = payload
        .get("cwd")
        .and_then(Value::as_str)
        .and_then(cwd_suffix);

    match cwd {
        Some(cwd) => format!("{runtime}:{bucket}:{cwd}"),
        None => format!("{runtime}:{bucket}"),
    }
}

pub fn sanitize_local_event_payload(payload: Value, correlation_id: String) -> Value {
    let mut sanitized = redact_sensitive_fields(payload);
    match &mut sanitized {
        Value::Object(object) => {
            object.insert("correlation_id".to_string(), Value::String(correlation_id));
        }
        other => {
            *other = serde_json::json!({
                "correlation_id": correlation_id,
                "value": other.clone(),
            });
        }
    }
    sanitized
}

fn redact_sensitive_fields(value: Value) -> Value {
    match value {
        Value::Array(values) => {
            Value::Array(values.into_iter().map(redact_sensitive_fields).collect())
        }
        Value::Object(object) => Value::Object(
            object
                .into_iter()
                .filter_map(|(key, value)| {
                    if is_sensitive_key(&key) {
                        return None;
                    }

                    Some((key, redact_sensitive_fields(value)))
                })
                .collect(),
        ),
        other => other,
    }
}

fn is_sensitive_key(key: &str) -> bool {
    key.eq_ignore_ascii_case("authorization")
        || key.eq_ignore_ascii_case("event_auth_token")
        || key.eq_ignore_ascii_case("proxy-authorization")
        || key.eq_ignore_ascii_case("cookie")
        || key.eq_ignore_ascii_case("set-cookie")
        || key.eq_ignore_ascii_case("x-api-key")
}

fn timestamp_bucket(timestamp: &str) -> Option<String> {
    let timestamp = OffsetDateTime::parse(timestamp, &Rfc3339).ok()?;
    let utc_timestamp = timestamp.to_offset(time::UtcOffset::UTC);
    let minute_bucket = utc_timestamp
        .replace_second(0)
        .ok()?
        .replace_millisecond(0)
        .ok()?
        .replace_microsecond(0)
        .ok()?
        .replace_nanosecond(0)
        .ok()?;

    minute_bucket
        .format(&format_description!(
            "[year]-[month]-[day]T[hour]:[minute]:00Z"
        ))
        .ok()
}

fn cwd_suffix(cwd: &str) -> Option<String> {
    Path::new(cwd)
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn started_storage() -> AuditStorage {
        AuditStorage::open_in_memory().unwrap()
    }

    #[test]
    fn insert_event_persists_event_row() {
        let storage = started_storage();

        let event_id = storage
            .insert_event(
                Some("session-1"),
                "request.received",
                None,
                Some("{\"ok\":true}"),
                None,
            )
            .unwrap();

        let persisted = storage
            .connection()
            .query_row(
                "SELECT session_id, event_type, tool_name, payload, source
                 FROM events
                 WHERE id = ?1",
                [event_id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                },
            )
            .unwrap();

        assert_eq!(
            persisted,
            (
                Some("session-1".to_string()),
                "request.received".to_string(),
                None,
                Some("{\"ok\":true}".to_string()),
                None,
            )
        );
    }

    #[test]
    fn insert_event_persists_all_optional_fields() {
        let storage = started_storage();

        let event_id = storage
            .insert_event(
                Some("session-2"),
                "tool.invocation",
                Some("bash"),
                Some("{\"cmd\":\"ls\"}"),
                Some("codex"),
            )
            .unwrap();

        let persisted = storage
            .connection()
            .query_row(
                "SELECT session_id, event_type, tool_name, payload, source
                 FROM events
                 WHERE id = ?1",
                [event_id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                },
            )
            .unwrap();

        assert_eq!(persisted.0, Some("session-2".to_string()));
        assert_eq!(persisted.1, "tool.invocation");
        assert_eq!(persisted.2, Some("bash".to_string()));
        assert_eq!(persisted.3, Some("{\"cmd\":\"ls\"}".to_string()));
        assert_eq!(persisted.4, Some("codex".to_string()));
    }

    #[test]
    fn request_start_and_successful_finalization_persist_request_lifecycle() {
        let storage = started_storage();

        storage
            .insert_request_start(
                "req-1",
                Some("session-1"),
                "codex",
                "openai",
                "POST",
                "/v1/openai/responses",
                false,
                None,
                None,
                None,
            )
            .unwrap();
        storage
            .insert_request_payload("req-1", "{\"input\":\"hello\"}")
            .unwrap();
        storage
            .finalize_request("req-1", TerminalReason::Ok, Some(200), Some(50))
            .unwrap();
        storage
            .insert_response_payload("req-1", "{\"id\":\"resp-1\"}")
            .unwrap();

        let request = storage
            .connection()
            .query_row(
                "SELECT runtime, provider, method, path, session_id, terminal_reason,
                        response_status, latency_ms, started_at IS NOT NULL, completed_at IS NOT NULL
                 FROM requests
                 WHERE request_id = ?1",
                ["req-1"],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<i64>>(6)?,
                        row.get::<_, Option<i64>>(7)?,
                        row.get::<_, i64>(8)?,
                        row.get::<_, i64>(9)?,
                    ))
                },
            )
            .unwrap();

        assert_eq!(
            request,
            (
                "codex".to_string(),
                "openai".to_string(),
                "POST".to_string(),
                "/v1/openai/responses".to_string(),
                Some("session-1".to_string()),
                Some("ok".to_string()),
                Some(200),
                Some(50),
                1,
                1,
            )
        );

        let request_body: String = storage
            .connection()
            .query_row(
                "SELECT body FROM request_payloads WHERE request_id = ?1",
                ["req-1"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(request_body, "{\"input\":\"hello\"}");

        let response_body: String = storage
            .connection()
            .query_row(
                "SELECT body FROM response_payloads WHERE request_id = ?1",
                ["req-1"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(response_body, "{\"id\":\"resp-1\"}");
    }

    #[test]
    fn finalize_request_records_early_termination_without_response_payload() {
        let storage = started_storage();

        storage
            .insert_request_start(
                "req-2",
                None,
                "codex",
                "anthropic",
                "POST",
                "/v1/anthropic/messages",
                false,
                None,
                None,
                None,
            )
            .unwrap();
        storage
            .finalize_request("req-2", TerminalReason::ClientCancelled, None, None)
            .unwrap();

        let persisted = storage
            .connection()
            .query_row(
                "SELECT terminal_reason, response_status, latency_ms, completed_at IS NOT NULL
                 FROM requests
                 WHERE request_id = ?1",
                ["req-2"],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            persisted,
            (Some("client_cancelled".to_string()), None, None, 1)
        );

        let response_payload_count: i64 = storage
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM response_payloads WHERE request_id = ?1",
                ["req-2"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(response_payload_count, 0);
    }

    #[test]
    fn insert_violation_persists_related_violation_row() {
        let storage = started_storage();

        storage
            .insert_request_start(
                "req-3",
                None,
                "codex",
                "openai",
                "POST",
                "/v1/openai/responses",
                false,
                None,
                None,
                None,
            )
            .unwrap();

        let violation_id = storage
            .insert_violation(
                "req-3",
                "rule-secrets",
                "secrets",
                "high",
                "sk-12345",
                "block",
            )
            .unwrap();

        let persisted = storage
            .connection()
            .query_row(
                "SELECT request_id, rule_id, rule_name, severity, matched_content, action
                 FROM violations
                 WHERE id = ?1",
                [violation_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .unwrap();

        assert_eq!(
            persisted,
            (
                "req-3".to_string(),
                "rule-secrets".to_string(),
                "secrets".to_string(),
                "high".to_string(),
                "sk-12345".to_string(),
                "block".to_string(),
            )
        );
    }

    #[test]
    fn finalize_request_fails_on_second_terminal_write_and_preserves_first_result() {
        let storage = started_storage();

        storage
            .insert_request_start(
                "req-4",
                None,
                "codex",
                "openai",
                "POST",
                "/v1/openai/responses",
                false,
                None,
                None,
                None,
            )
            .unwrap();
        storage
            .finalize_request("req-4", TerminalReason::Ok, Some(200), Some(10))
            .unwrap();

        let second = storage.finalize_request(
            "req-4",
            TerminalReason::ClientCancelled,
            Some(499),
            Some(20),
        );
        assert!(second.is_err());

        let request = storage
            .connection()
            .query_row(
                "SELECT terminal_reason, response_status, latency_ms
                 FROM requests
                 WHERE request_id = ?1",
                ["req-4"],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            request,
            (Some("ok".to_string()), Some(200), Some(10))
        );
    }

    #[test]
    fn correlation_id_prefers_creavor_session_header() {
        let mut headers = HeaderMap::new();
        headers.insert(SESSION_HEADER, HeaderValue::from_static("session-123"));

        let correlation_id = correlation_id_for_event(
            &headers,
            &serde_json::json!({
                "runtime": "codex",
                "timestamp": "2026-04-07T12:34:56Z",
                "cwd": "/tmp/demo",
            }),
        );

        assert_eq!(correlation_id, "session-123");
    }

    #[test]
    fn correlation_id_falls_back_to_runtime_bucket_and_cwd() {
        let correlation_id = correlation_id_for_event(
            &HeaderMap::new(),
            &serde_json::json!({
                "runtime": "codex",
                "timestamp": "2026-04-07T12:34:56.789Z",
                "cwd": "/Users/norman/project",
            }),
        );

        assert_eq!(correlation_id, "codex:2026-04-07T12:34:00Z:project");
    }

    #[test]
    fn correlation_id_uses_utc_bucket_for_offset_timestamp() {
        let correlation_id = correlation_id_for_event(
            &HeaderMap::new(),
            &serde_json::json!({
                "runtime": "codex",
                "timestamp": "2026-04-07T20:34:56+08:00",
                "cwd": "/Users/norman/project",
            }),
        );

        assert_eq!(correlation_id, "codex:2026-04-07T12:34:00Z:project");
    }

    #[test]
    fn correlation_id_keeps_unknown_time_fallback_for_malformed_timestamp() {
        let correlation_id = correlation_id_for_event(
            &HeaderMap::new(),
            &serde_json::json!({
                "runtime": "codex",
                "timestamp": "definitely-not-rfc3339",
                "cwd": "/Users/norman/project",
            }),
        );

        assert_eq!(correlation_id, "codex:unknown-time:project");
    }

    #[test]
    fn sanitize_local_event_payload_adds_correlation_and_drops_sensitive_keys() {
        let payload = sanitize_local_event_payload(
            serde_json::json!({
                "type": "editor.event",
                "authorization": "Bearer secret",
                "nested": {
                    "event_auth_token": "secret",
                    "proxy-authorization": "Basic abc",
                    "cookie": "session=value",
                    "set-cookie": "session=value",
                    "x-api-key": "secret-key"
                }
            }),
            "session-123".to_string(),
        );

        assert_eq!(payload["correlation_id"], serde_json::json!("session-123"));
        assert!(payload.get("authorization").is_none());
        assert!(payload["nested"].get("event_auth_token").is_none());
        assert!(payload["nested"].get("proxy-authorization").is_none());
        assert!(payload["nested"].get("cookie").is_none());
        assert!(payload["nested"].get("set-cookie").is_none());
        assert!(payload["nested"].get("x-api-key").is_none());
    }
}
