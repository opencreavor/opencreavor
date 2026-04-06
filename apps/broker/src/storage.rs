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
                event_type TEXT NOT NULL,
                request_id TEXT,
                payload TEXT,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );

            CREATE TABLE IF NOT EXISTS requests (
                request_id TEXT PRIMARY KEY,
                provider TEXT NOT NULL,
                method TEXT NOT NULL,
                path TEXT NOT NULL,
                session_id TEXT,
                terminal_reason TEXT,
                response_status INTEGER,
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
                rule_name TEXT NOT NULL,
                action TEXT NOT NULL,
                detail TEXT NOT NULL,
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
    fn initializes_expected_audit_tables() {
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
        assert_eq!(
            column_names(&storage, "events"),
            vec!["id", "event_type", "request_id", "payload", "created_at"]
        );
        assert_eq!(
            column_names(&storage, "requests"),
            vec![
                "request_id",
                "provider",
                "method",
                "path",
                "session_id",
                "terminal_reason",
                "response_status",
                "started_at",
                "completed_at",
            ]
        );
        assert_eq!(
            column_names(&storage, "request_payloads"),
            vec!["request_id", "body", "created_at"]
        );
        assert_eq!(
            column_names(&storage, "response_payloads"),
            vec!["request_id", "body", "created_at"]
        );
        assert_eq!(
            column_names(&storage, "violations"),
            vec!["id", "request_id", "rule_name", "action", "detail", "created_at"]
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
    fn open_in_memory_allows_immediate_audit_writes() {
        let storage = AuditStorage::open_in_memory().unwrap();

        storage
            .insert_event("broker.started", None, Some("{\"ready\":true}"))
            .unwrap();
        storage
            .insert_request_start(
                "req-open-memory",
                "openai",
                "POST",
                "/v1/openai/responses",
                None,
                "{\"input\":\"hello\"}",
            )
            .unwrap();
        storage
            .insert_violation("req-open-memory", "secrets", "block", "matched")
            .unwrap();
        storage
            .finalize_request(
                "req-open-memory",
                TerminalReason::Ok,
                Some(200),
                Some("{\"id\":\"resp-open-memory\"}"),
            )
            .unwrap();

        let counts = (
            storage
                .connection()
                .query_row("SELECT COUNT(*) FROM events", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            storage
                .connection()
                .query_row("SELECT COUNT(*) FROM requests", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            storage
                .connection()
                .query_row("SELECT COUNT(*) FROM violations", [], |row| row.get::<_, i64>(0))
                .unwrap(),
        );
        assert_eq!(counts, (1, 1, 1));
    }

    #[test]
    fn open_file_allows_immediate_audit_writes() {
        let path = unique_temp_path("storage-open");

        let storage = AuditStorage::open(&path).unwrap();
        storage
            .insert_event("broker.started", None, Some("{\"ready\":true}"))
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
