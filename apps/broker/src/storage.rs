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
        // Enable WAL mode and set busy_timeout for concurrent write safety
        self.connection.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA busy_timeout = 5000;"
        )?;

        // Create tables (idempotent via IF NOT EXISTS)
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
                upstream_id TEXT,
                protocol_family TEXT,
                request_headers TEXT,
                request_summary TEXT,
                response_summary TEXT,
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
                session_id TEXT,
                runtime TEXT,
                upstream_id TEXT,
                rule_id TEXT NOT NULL,
                rule_name TEXT NOT NULL,
                severity TEXT NOT NULL,
                matched_content TEXT,
                action TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY (request_id) REFERENCES requests(request_id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS approval_requests (
                id TEXT PRIMARY KEY,
                request_id TEXT NOT NULL,
                session_id TEXT,
                runtime TEXT NOT NULL,
                upstream_id TEXT,
                risk_level TEXT NOT NULL,
                rule_id TEXT NOT NULL,
                sanitized_summary TEXT NOT NULL,
                status TEXT NOT NULL,
                expires_at TEXT,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY (request_id) REFERENCES requests(request_id)
            );

            CREATE TABLE IF NOT EXISTS approval_actions (
                id TEXT PRIMARY KEY,
                approval_request_id TEXT NOT NULL,
                action TEXT NOT NULL,
                actor TEXT NOT NULL,
                source TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY (approval_request_id) REFERENCES approval_requests(id)
            );
            ",
        )?;

        // Migrate existing databases: add columns if they don't exist
        self.migrate_schema()?;

        Ok(())
    }

    /// Add columns to existing tables for forward-compatible schema migration.
    fn migrate_schema(&self) -> anyhow::Result<()> {
        let request_migrations = [
            "ALTER TABLE requests ADD COLUMN upstream_id TEXT",
            "ALTER TABLE requests ADD COLUMN protocol_family TEXT",
            "ALTER TABLE requests ADD COLUMN request_headers TEXT",
            "ALTER TABLE requests ADD COLUMN request_summary TEXT",
            "ALTER TABLE requests ADD COLUMN response_summary TEXT",
        ];
        for sql in &request_migrations {
            if let Err(e) = self.connection.execute_batch(sql) {
                let msg = e.to_string();
                if !msg.contains("duplicate column name") {
                    return Err(e.into());
                }
            }
        }

        let violation_migrations = [
            "ALTER TABLE violations ADD COLUMN session_id TEXT",
            "ALTER TABLE violations ADD COLUMN runtime TEXT",
            "ALTER TABLE violations ADD COLUMN upstream_id TEXT",
        ];
        for sql in &violation_migrations {
            if let Err(e) = self.connection.execute_batch(sql) {
                let msg = e.to_string();
                if !msg.contains("duplicate column name") {
                    return Err(e.into());
                }
            }
        }

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

    // -- New methods for upstream and approval --

    /// Update upstream_id and protocol_family on an existing request record.
    pub fn update_request_upstream(
        &self,
        request_id: &str,
        upstream_id: Option<&str>,
        protocol_family: Option<&str>,
    ) -> anyhow::Result<()> {
        self.connection.execute(
            "UPDATE requests SET upstream_id = ?1, protocol_family = ?2 WHERE request_id = ?3",
            rusqlite::params![upstream_id, protocol_family, request_id],
        )?;
        Ok(())
    }

    // -- Approval methods --

    pub fn insert_approval_request(
        &self,
        id: &str,
        request_id: &str,
        session_id: Option<&str>,
        runtime: &str,
        upstream_id: Option<&str>,
        risk_level: &str,
        rule_id: &str,
        sanitized_summary: &str,
        status: &str,
        expires_at: Option<&str>,
    ) -> anyhow::Result<()> {
        self.connection.execute(
            "INSERT INTO approval_requests (id, request_id, session_id, runtime, upstream_id, risk_level, rule_id, sanitized_summary, status, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![id, request_id, session_id, runtime, upstream_id, risk_level, rule_id, sanitized_summary, status, expires_at],
        )?;
        Ok(())
    }

    pub fn insert_approval_action(
        &self,
        id: &str,
        approval_request_id: &str,
        action: &str,
        actor: &str,
        source: &str,
    ) -> anyhow::Result<()> {
        self.connection.execute(
            "INSERT INTO approval_actions (id, approval_request_id, action, actor, source)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![id, approval_request_id, action, actor, source],
        )?;
        Ok(())
    }

    pub fn update_approval_request_status(
        &self,
        id: &str,
        status: &str,
    ) -> anyhow::Result<()> {
        self.connection.execute(
            "UPDATE approval_requests SET status = ?1 WHERE id = ?2",
            rusqlite::params![status, id],
        )?;
        Ok(())
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
    fn initializes_seven_expected_tables() {
        let storage = AuditStorage::open_in_memory().unwrap();

        assert_eq!(
            table_names(&storage),
            vec![
                "approval_actions".to_string(),
                "approval_requests".to_string(),
                "events".to_string(),
                "request_payloads".to_string(),
                "requests".to_string(),
                "response_payloads".to_string(),
                "violations".to_string(),
            ]
        );
    }

    #[test]
    fn requests_table_has_new_columns() {
        let storage = AuditStorage::open_in_memory().unwrap();
        let columns = column_names(&storage, "requests");
        assert!(columns.contains(&"upstream_id".to_string()));
        assert!(columns.contains(&"protocol_family".to_string()));
        assert!(columns.contains(&"request_headers".to_string()));
        assert!(columns.contains(&"request_summary".to_string()));
        assert!(columns.contains(&"response_summary".to_string()));
    }

    #[test]
    fn violations_table_has_new_columns() {
        let storage = AuditStorage::open_in_memory().unwrap();
        let columns = column_names(&storage, "violations");
        assert!(columns.contains(&"session_id".to_string()));
        assert!(columns.contains(&"runtime".to_string()));
        assert!(columns.contains(&"upstream_id".to_string()));
    }

    #[test]
    fn approval_requests_table_has_expected_columns() {
        let storage = AuditStorage::open_in_memory().unwrap();
        assert_eq!(
            column_names(&storage, "approval_requests"),
            vec![
                "id", "request_id", "session_id", "runtime", "upstream_id",
                "risk_level", "rule_id", "sanitized_summary", "status", "expires_at", "created_at",
            ]
        );
    }

    #[test]
    fn approval_actions_table_has_expected_columns() {
        let storage = AuditStorage::open_in_memory().unwrap();
        assert_eq!(
            column_names(&storage, "approval_actions"),
            vec!["id", "approval_request_id", "action", "actor", "source", "created_at"]
        );
    }

    #[test]
    fn initialize_is_idempotent() {
        let storage = AuditStorage::open_in_memory().unwrap();
        storage.initialize().unwrap();
        storage.initialize().unwrap();
        assert_eq!(table_names(&storage).len(), 7);
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
        let count: i64 = storage
            .connection()
            .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn update_request_upstream_sets_fields() {
        let storage = AuditStorage::open_in_memory().unwrap();
        storage
            .insert_request_start(
                "req-upstream",
                None,
                "claude-code",
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
            .update_request_upstream("req-upstream", Some("zhipu-anthropic"), Some("anthropic"))
            .unwrap();
        let result = storage
            .connection()
            .query_row(
                "SELECT upstream_id, protocol_family FROM requests WHERE request_id = ?1",
                ["req-upstream"],
                |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .unwrap();
        assert_eq!(result.0, Some("zhipu-anthropic".to_string()));
        assert_eq!(result.1, Some("anthropic".to_string()));
    }

    #[test]
    fn approval_request_lifecycle() {
        let storage = AuditStorage::open_in_memory().unwrap();

        storage
            .insert_request_start(
                "req-approval",
                Some("session-1"),
                "claude-code",
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
            .insert_approval_request(
                "approval-1",
                "req-approval",
                Some("session-1"),
                "claude-code",
                Some("zhipu-anthropic"),
                "high",
                "rule-secrets",
                "API key detected in request body",
                "pending",
                Some("2026-04-09T12:35:00Z"),
            )
            .unwrap();

        storage
            .insert_approval_action("action-1", "approval-1", "allow_once", "local_user", "guard_mcp")
            .unwrap();

        storage
            .update_approval_request_status("approval-1", "approved")
            .unwrap();

        let approval = storage
            .connection()
            .query_row(
                "SELECT status, risk_level, runtime, upstream_id FROM approval_requests WHERE id = ?1",
                ["approval-1"],
                |row| Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                )),
            )
            .unwrap();

        assert_eq!(approval.0, "approved");
        assert_eq!(approval.1, "high");
        assert_eq!(approval.2, "claude-code");
        assert_eq!(approval.3, Some("zhipu-anthropic".to_string()));

        let action = storage
            .connection()
            .query_row(
                "SELECT action, actor, source FROM approval_actions WHERE approval_request_id = ?1",
                ["approval-1"],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)),
            )
            .unwrap();

        assert_eq!(action.0, "allow_once");
        assert_eq!(action.1, "local_user");
        assert_eq!(action.2, "guard_mcp");
    }
}
