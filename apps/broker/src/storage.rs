use rusqlite::Connection;
use std::path::Path;

pub struct AuditStorage {
    connection: Connection,
}

impl AuditStorage {
    pub fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let connection = Connection::open(path)?;
        connection.execute_batch("PRAGMA foreign_keys = ON;")?;
        Self::from_connection(connection)
    }

    pub fn open_in_memory() -> anyhow::Result<Self> {
        let connection = Connection::open_in_memory()?;
        connection.execute_batch("PRAGMA foreign_keys = ON;")?;
        Self::from_connection(connection)
    }

    pub fn initialize(&self) -> anyhow::Result<()> {
        self.connection.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT,
                event_type TEXT NOT NULL,
                tool_name TEXT,
                payload TEXT,
                source TEXT,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );

            CREATE TABLE IF NOT EXISTS requests (
                request_id TEXT PRIMARY KEY,
                session_id TEXT,
                runtime TEXT NOT NULL,
                provider TEXT NOT NULL,
                method TEXT NOT NULL,
                path TEXT NOT NULL,
                blocked BOOLEAN DEFAULT FALSE,
                block_reason TEXT,
                rule_id TEXT,
                severity TEXT,
                terminal_reason TEXT,
                response_status INTEGER,
                latency_ms INTEGER,
                started_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                completed_at TEXT
            );

            CREATE TABLE IF NOT EXISTS request_payloads (
                request_id TEXT PRIMARY KEY,
                body TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY (request_id) REFERENCES requests(request_id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS response_payloads (
                request_id TEXT PRIMARY KEY,
                body TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY (request_id) REFERENCES requests(request_id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS violations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                request_id TEXT NOT NULL,
                rule_id TEXT NOT NULL,
                rule_name TEXT NOT NULL,
                severity TEXT NOT NULL,
                matched_content TEXT,
                action TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY (request_id) REFERENCES requests(request_id) ON DELETE CASCADE
            );
            ",
        )?;

        Ok(())
    }

    fn from_connection(connection: Connection) -> anyhow::Result<Self> {
        let storage = Self { connection };
        storage.initialize()?;
        Ok(storage)
    }

    pub(crate) fn connection(&self) -> &Connection {
        &self.connection
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::TerminalReason;
    use std::{
        env, fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn unique_temp_path(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        env::temp_dir().join(format!("creavor-broker-{name}-{nanos}.sqlite"))
    }

    fn table_names(storage: &AuditStorage) -> Vec<String> {
        let mut statement = storage
            .connection()
            .prepare(
                "SELECT name
                 FROM sqlite_master
                 WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
                 ORDER BY name",
            )
            .unwrap();

        statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    fn column_names(storage: &AuditStorage, table: &str) -> Vec<String> {
        let pragma = format!("PRAGMA table_info({table})");
        let mut statement = storage.connection().prepare(&pragma).unwrap();

        statement
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    #[test]
    fn initializes_five_expected_tables() {
        let storage = AuditStorage::open_in_memory().unwrap();

        assert_eq!(
            table_names(&storage),
            vec![
                "events".to_string(),
                "request_payloads".to_string(),
                "requests".to_string(),
                "response_payloads".to_string(),
                "violations".to_string(),
            ]
        );
    }

    #[test]
    fn events_table_has_expected_columns() {
        let storage = AuditStorage::open_in_memory().unwrap();

        assert_eq!(
            column_names(&storage, "events"),
            vec![
                "id",
                "session_id",
                "event_type",
                "tool_name",
                "payload",
                "source",
                "created_at",
            ]
        );
    }

    #[test]
    fn requests_table_has_expected_columns() {
        let storage = AuditStorage::open_in_memory().unwrap();

        assert_eq!(
            column_names(&storage, "requests"),
            vec![
                "request_id",
                "session_id",
                "runtime",
                "provider",
                "method",
                "path",
                "blocked",
                "block_reason",
                "rule_id",
                "severity",
                "terminal_reason",
                "response_status",
                "latency_ms",
                "started_at",
                "completed_at",
            ]
        );
    }

    #[test]
    fn request_payloads_table_has_expected_columns() {
        let storage = AuditStorage::open_in_memory().unwrap();

        assert_eq!(
            column_names(&storage, "request_payloads"),
            vec!["request_id", "body", "created_at"]
        );
    }

    #[test]
    fn response_payloads_table_has_expected_columns() {
        let storage = AuditStorage::open_in_memory().unwrap();

        assert_eq!(
            column_names(&storage, "response_payloads"),
            vec!["request_id", "body", "created_at"]
        );
    }

    #[test]
    fn violations_table_has_expected_columns() {
        let storage = AuditStorage::open_in_memory().unwrap();

        assert_eq!(
            column_names(&storage, "violations"),
            vec![
                "id",
                "request_id",
                "rule_id",
                "rule_name",
                "severity",
                "matched_content",
                "action",
                "created_at",
            ]
        );
    }

    #[test]
    fn initialize_is_idempotent() {
        let storage = AuditStorage::open_in_memory().unwrap();

        storage.initialize().unwrap();
        storage.initialize().unwrap();

        assert_eq!(table_names(&storage).len(), 5);
    }

    #[test]
    fn request_lifecycle_with_blocked_and_violation() {
        let storage = AuditStorage::open_in_memory().unwrap();

        storage
            .insert_request_start(
                "req-blocked",
                Some("session-1"),
                "codex",
                "openai",
                "POST",
                "/v1/responses",
                true,
                Some("api key detected"),
                Some("rule-secrets"),
                Some("high"),
            )
            .unwrap();

        storage
            .insert_violation(
                "req-blocked",
                "rule-secrets",
                "secrets",
                "high",
                "sk-12345",
                "block",
            )
            .unwrap();

        storage
            .finalize_request("req-blocked", TerminalReason::Ok, Some(200), Some(42))
            .unwrap();

        let request = storage
            .connection()
            .query_row(
                "SELECT runtime, provider, method, path, session_id, blocked, block_reason,
                        rule_id, severity, terminal_reason, response_status, latency_ms
                 FROM requests
                 WHERE request_id = ?1",
                ["req-blocked"],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, bool>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, Option<String>>(8)?,
                        row.get::<_, Option<String>>(9)?,
                        row.get::<_, Option<i64>>(10)?,
                        row.get::<_, Option<i64>>(11)?,
                    ))
                },
            )
            .unwrap();

        assert_eq!(request.0, "codex");
        assert_eq!(request.1, "openai");
        assert_eq!(request.2, "POST");
        assert_eq!(request.3, "/v1/responses");
        assert_eq!(request.4, Some("session-1".to_string()));
        assert!(request.5); // blocked
        assert_eq!(request.6, Some("api key detected".to_string()));
        assert_eq!(request.7, Some("rule-secrets".to_string()));
        assert_eq!(request.8, Some("high".to_string()));
        assert_eq!(request.9, Some("ok".to_string()));
        assert_eq!(request.10, Some(200));
        assert_eq!(request.11, Some(42));

        let violation = storage
            .connection()
            .query_row(
                "SELECT rule_id, rule_name, severity, action
                 FROM violations WHERE request_id = ?1",
                ["req-blocked"],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .unwrap();

        assert_eq!(violation.0, "rule-secrets");
        assert_eq!(violation.1, "secrets");
        assert_eq!(violation.2, "high");
        assert_eq!(violation.3, "block");
    }

    #[test]
    fn request_payload_stored_when_requested() {
        let storage = AuditStorage::open_in_memory().unwrap();

        storage
            .insert_request_start(
                "req-payload",
                None,
                "codex",
                "anthropic",
                "POST",
                "/v1/messages",
                false,
                None,
                None,
                None,
            )
            .unwrap();

        storage
            .insert_request_payload("req-payload", "{\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}]}")
            .unwrap();

        let body: String = storage
            .connection()
            .query_row(
                "SELECT body FROM request_payloads WHERE request_id = ?1",
                ["req-payload"],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(body, "{\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}]}");
    }

    #[test]
    fn response_payload_stored_when_requested() {
        let storage = AuditStorage::open_in_memory().unwrap();

        storage
            .insert_request_start(
                "req-resp",
                None,
                "codex",
                "openai",
                "POST",
                "/v1/responses",
                false,
                None,
                None,
                None,
            )
            .unwrap();

        storage
            .insert_response_payload("req-resp", "{\"output\":\"hello world\"}")
            .unwrap();

        let body: String = storage
            .connection()
            .query_row(
                "SELECT body FROM response_payloads WHERE request_id = ?1",
                ["req-resp"],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(body, "{\"output\":\"hello world\"}");
    }

    #[test]
    fn finalize_request_rejects_double_finalize() {
        let storage = AuditStorage::open_in_memory().unwrap();

        storage
            .insert_request_start(
                "req-dbl",
                None,
                "codex",
                "openai",
                "POST",
                "/v1/responses",
                false,
                None,
                None,
                None,
            )
            .unwrap();

        storage
            .finalize_request("req-dbl", TerminalReason::Ok, Some(200), Some(10))
            .unwrap();

        let second = storage.finalize_request(
            "req-dbl",
            TerminalReason::ClientCancelled,
            Some(499),
            Some(20),
        );
        assert!(second.is_err());

        let terminal_reason: String = storage
            .connection()
            .query_row(
                "SELECT terminal_reason FROM requests WHERE request_id = ?1",
                ["req-dbl"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(terminal_reason, "ok");

        let latency: Option<i64> = storage
            .connection()
            .query_row(
                "SELECT latency_ms FROM requests WHERE request_id = ?1",
                ["req-dbl"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(latency, Some(10));
    }

    #[test]
    fn latency_ms_is_recorded() {
        let storage = AuditStorage::open_in_memory().unwrap();

        storage
            .insert_request_start(
                "req-latency",
                None,
                "codex",
                "anthropic",
                "POST",
                "/v1/messages",
                false,
                None,
                None,
                None,
            )
            .unwrap();

        storage
            .finalize_request("req-latency", TerminalReason::Ok, Some(200), Some(1234))
            .unwrap();

        let latency: Option<i64> = storage
            .connection()
            .query_row(
                "SELECT latency_ms FROM requests WHERE request_id = ?1",
                ["req-latency"],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(latency, Some(1234));
    }

    #[test]
    fn open_file_allows_immediate_audit_writes() {
        let path = unique_temp_path("storage-open");

        let storage = AuditStorage::open(&path).unwrap();
        storage
            .insert_event(
                Some("session-x"),
                "broker.started",
                None,
                Some("{\"ready\":true}"),
                None,
            )
            .unwrap();

        let event_count: i64 = storage
            .connection()
            .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(event_count, 1);

        drop(storage);
        let _ = fs::remove_file(path);
    }
}
