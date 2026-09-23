//! Persistent, local-only audit ledger for MCP command tasks (ORQ-30).
//!
//! The ledger shares `mcp.db` with pairing state but owns an independent
//! SQLite connection. It stores request identity and lifecycle metadata only;
//! task stdout/stderr, pairing tokens, and passwords are never written here.

use crate::task_engine::{TaskSnapshot, TaskStatus};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

pub const DEFAULT_RECENT_LIMIT: usize = 100;
pub const MAX_RECENT_LIMIT: usize = 500;
pub const DEFAULT_RETENTION_MS: u64 = 30 * 24 * 60 * 60 * 1000;
pub const MAX_RECORDS: usize = 10_000;
const BUSY_TIMEOUT: Duration = Duration::from_millis(150);

/// A durable description of one command request and its latest known state.
/// This record intentionally has no output fields.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskAuditRecord {
    pub app_instance_id: String,
    pub task_id: String,
    pub client_id: String,
    pub connection_id: String,
    pub generation: u64,
    pub request_id: String,
    pub request_digest: String,
    pub command: String,
    pub working_directory: String,
    pub timeout_seconds: u64,
    pub created_at_ms: u64,
    pub approved_at_ms: Option<u64>,
    pub started_at_ms: Option<u64>,
    pub ended_at_ms: Option<u64>,
    pub status: TaskStatus,
    pub exit_code: Option<i32>,
    pub detail: Option<String>,
    pub audit_error: Option<String>,
}

impl TaskAuditRecord {
    pub(crate) fn to_snapshot(&self) -> TaskSnapshot {
        TaskSnapshot {
            task_id: self.task_id.clone(),
            request_id: self.request_id.clone(),
            client_id: self.client_id.clone(),
            connection_id: self.connection_id.clone(),
            generation: self.generation,
            command: self.command.clone(),
            working_directory: self.working_directory.clone(),
            timeout_seconds: self.timeout_seconds,
            status: self.status,
            created_at_ms: self.created_at_ms,
            approved_at_ms: self.approved_at_ms,
            started_at_ms: self.started_at_ms,
            ended_at_ms: self.ended_at_ms,
            exit_code: self.exit_code,
            output_chars: 0,
            output_truncated: false,
            detail: self.detail.clone(),
            audit_recorded: true,
            audit_error: self.audit_error.clone(),
        }
    }
}

pub(crate) enum CreateTaskResult {
    Inserted,
    Existing(Box<TaskAuditRecord>),
}

pub(crate) struct TaskAuditTransition<'a> {
    pub status: TaskStatus,
    pub approved_at_ms: Option<u64>,
    pub started_at_ms: Option<u64>,
    pub ended_at_ms: Option<u64>,
    pub exit_code: Option<i32>,
    pub detail: Option<&'a str>,
    pub audit_error: Option<&'a str>,
}

pub struct TaskAuditStore {
    conn: Mutex<Connection>,
    app_instance_id: String,
}

impl TaskAuditStore {
    /// Open the audit ledger using the same private SQLite file as MCP pairing.
    pub fn open(db_path: &Path) -> Result<Self, String> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            set_private_directory(parent)?;
        }

        let conn = Connection::open(db_path).map_err(|error| error.to_string())?;
        conn.busy_timeout(BUSY_TIMEOUT)
            .map_err(|error| error.to_string())?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA foreign_keys=ON;
             CREATE TABLE IF NOT EXISTS mcp_task_ledger (
               task_id          TEXT PRIMARY KEY NOT NULL,
               app_instance_id  TEXT NOT NULL,
               client_id        TEXT NOT NULL,
               connection_id    TEXT NOT NULL,
               generation       INTEGER NOT NULL,
               request_id       TEXT NOT NULL,
               request_digest   TEXT NOT NULL,
               command          TEXT NOT NULL,
               working_directory TEXT NOT NULL DEFAULT 'login',
               timeout_seconds INTEGER NOT NULL DEFAULT 300,
               created_at_ms    INTEGER NOT NULL,
               approved_at_ms   INTEGER,
               started_at_ms    INTEGER,
               ended_at_ms      INTEGER,
               status           TEXT NOT NULL,
               exit_code        INTEGER,
               detail           TEXT,
               audit_error      TEXT,
               updated_at_ms    INTEGER NOT NULL,
               UNIQUE (client_id, request_id)
             );
             CREATE INDEX IF NOT EXISTS mcp_task_ledger_recent
               ON mcp_task_ledger (updated_at_ms DESC);
             CREATE INDEX IF NOT EXISTS mcp_task_ledger_status_created
               ON mcp_task_ledger (status, created_at_ms);
             CREATE TABLE IF NOT EXISTS mcp_task_events (
               event_id         INTEGER PRIMARY KEY AUTOINCREMENT,
               task_id          TEXT NOT NULL REFERENCES mcp_task_ledger(task_id) ON DELETE CASCADE,
               app_instance_id  TEXT NOT NULL,
               occurred_at_ms   INTEGER NOT NULL,
               status           TEXT NOT NULL,
               detail           TEXT,
               audit_error      TEXT
             );
             CREATE INDEX IF NOT EXISTS mcp_task_events_by_task
               ON mcp_task_events (task_id, event_id);",
        )
        .map_err(|error| error.to_string())?;
        ensure_column(
            &conn,
            "working_directory",
            "ALTER TABLE mcp_task_ledger ADD COLUMN working_directory TEXT NOT NULL DEFAULT 'login'",
        )?;
        ensure_column(
            &conn,
            "timeout_seconds",
            "ALTER TABLE mcp_task_ledger ADD COLUMN timeout_seconds INTEGER NOT NULL DEFAULT 300",
        )?;
        set_private_file(db_path)?;

        let store = Self {
            conn: Mutex::new(conn),
            app_instance_id: uuid::Uuid::new_v4().to_string(),
        };
        store.recover_abandoned_tasks(now_ms())?;
        Ok(store)
    }

    pub fn open_default() -> Result<Self, String> {
        Self::open(&default_db_path())
    }

    pub fn app_instance_id(&self) -> &str {
        &self.app_instance_id
    }

    pub(crate) fn create_task(
        &self,
        snapshot: &TaskSnapshot,
        request_digest: &str,
    ) -> Result<CreateTaskResult, String> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| "audit database lock poisoned".to_string())?;
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;

        if let Some(record) = read_by_request(&tx, &snapshot.client_id, &snapshot.request_id)? {
            return Ok(CreateTaskResult::Existing(Box::new(record)));
        }

        let created = to_sql_i64(snapshot.created_at_ms)?;
        tx.execute(
            "INSERT INTO mcp_task_ledger (
               task_id, app_instance_id, client_id, connection_id, generation,
               request_id, request_digest, command, working_directory, timeout_seconds,
               created_at_ms, status, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?11)",
            params![
                snapshot.task_id,
                self.app_instance_id,
                snapshot.client_id,
                snapshot.connection_id,
                to_sql_i64(snapshot.generation)?,
                snapshot.request_id,
                request_digest,
                snapshot.command,
                snapshot.working_directory,
                to_sql_i64(snapshot.timeout_seconds)?,
                created,
                status_name(snapshot.status),
            ],
        )
        .map_err(|error| error.to_string())?;
        insert_event(
            &tx,
            &self.app_instance_id,
            &snapshot.task_id,
            created,
            snapshot.status,
            snapshot.detail.as_deref(),
            snapshot.audit_error.as_deref(),
        )?;
        tx.commit().map_err(|error| error.to_string())?;
        Ok(CreateTaskResult::Inserted)
    }

    pub(crate) fn record_transition(
        &self,
        task_id: &str,
        transition: TaskAuditTransition<'_>,
    ) -> Result<(), String> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| "audit database lock poisoned".to_string())?;
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let updated_at = now_ms();
        let changed = tx
            .execute(
                "UPDATE mcp_task_ledger SET
                   approved_at_ms = COALESCE(?1, approved_at_ms),
                   started_at_ms = COALESCE(?2, started_at_ms),
                   ended_at_ms = COALESCE(?3, ended_at_ms),
                   status = ?4,
                   exit_code = COALESCE(?5, exit_code),
                   detail = ?6,
                   audit_error = ?7,
                   updated_at_ms = ?8
                 WHERE task_id = ?9 AND app_instance_id = ?10",
                params![
                    transition.approved_at_ms.map(to_sql_i64).transpose()?,
                    transition.started_at_ms.map(to_sql_i64).transpose()?,
                    transition.ended_at_ms.map(to_sql_i64).transpose()?,
                    status_name(transition.status),
                    transition.exit_code,
                    transition.detail,
                    transition.audit_error,
                    to_sql_i64(updated_at)?,
                    task_id,
                    self.app_instance_id,
                ],
            )
            .map_err(|error| error.to_string())?;
        if changed != 1 {
            return Err("audit task row is missing or belongs to another app instance".into());
        }
        insert_event(
            &tx,
            &self.app_instance_id,
            task_id,
            to_sql_i64(updated_at)?,
            transition.status,
            transition.detail,
            transition.audit_error,
        )?;
        tx.commit().map_err(|error| error.to_string())
    }

    /// Return at most 100 rows by default; caller-provided limits are bounded.
    pub fn recent(&self, limit: Option<usize>) -> Result<Vec<TaskAuditRecord>, String> {
        let limit = limit
            .unwrap_or(DEFAULT_RECENT_LIMIT)
            .clamp(1, MAX_RECENT_LIMIT) as i64;
        let conn = self
            .conn
            .lock()
            .map_err(|_| "audit database lock poisoned".to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT app_instance_id, task_id, client_id, connection_id, generation,
                        request_id, request_digest, command, working_directory, timeout_seconds,
                        created_at_ms, approved_at_ms, started_at_ms, ended_at_ms,
                        status, exit_code, detail, audit_error
                 FROM mcp_task_ledger
                 ORDER BY updated_at_ms DESC, created_at_ms DESC
                 LIMIT ?1",
            )
            .map_err(|error| error.to_string())?;
        let rows = stmt
            .query_map([limit], audit_record_from_row)
            .map_err(|error| error.to_string())?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|error| error.to_string())
    }

    /// Retain at most 30 days and 10,000 records where possible. Active tasks
    /// are never deleted; if they exceed the cap, the total may temporarily do
    /// so until they become terminal.
    pub fn sweep(&self) -> Result<usize, String> {
        self.sweep_at(now_ms(), DEFAULT_RETENTION_MS, MAX_RECORDS)
    }

    fn sweep_at(&self, now: u64, retention_ms: u64, max_records: usize) -> Result<usize, String> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| "audit database lock poisoned".to_string())?;
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let cutoff = to_sql_i64(now.saturating_sub(retention_ms))?;
        let before = tx
            .query_row("SELECT COUNT(*) FROM mcp_task_ledger", [], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(|error| error.to_string())?;

        tx.execute(
            "DELETE FROM mcp_task_ledger
             WHERE status NOT IN ('pending_approval', 'queued', 'dispatching', 'running', 'cancel_requested')
               AND created_at_ms < ?1",
            [cutoff],
        )
        .map_err(|error| error.to_string())?;

        let active: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM mcp_task_ledger
                 WHERE status IN ('pending_approval', 'queued', 'dispatching', 'running', 'cancel_requested')",
                [],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        let terminal_limit = (max_records as i64).saturating_sub(active).max(0);
        tx.execute(
            "DELETE FROM mcp_task_ledger
             WHERE task_id IN (
               SELECT task_id FROM mcp_task_ledger
               WHERE status NOT IN ('pending_approval', 'queued', 'dispatching', 'running', 'cancel_requested')
               ORDER BY created_at_ms DESC, task_id
               LIMIT -1 OFFSET ?1
             )",
            [terminal_limit],
        )
        .map_err(|error| error.to_string())?;

        let after: i64 = tx
            .query_row("SELECT COUNT(*) FROM mcp_task_ledger", [], |row| row.get(0))
            .map_err(|error| error.to_string())?;
        tx.commit().map_err(|error| error.to_string())?;
        Ok(before.saturating_sub(after).max(0) as usize)
    }

    fn recover_abandoned_tasks(&self, now: u64) -> Result<(), String> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| "audit database lock poisoned".to_string())?;
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let abandoned = {
            let mut stmt = tx
                .prepare(
                    "SELECT task_id FROM mcp_task_ledger
                     WHERE app_instance_id <> ?1
                       AND status IN ('pending_approval', 'queued', 'dispatching', 'running', 'cancel_requested')",
                )
                .map_err(|error| error.to_string())?;
            let rows = stmt
                .query_map([&self.app_instance_id], |row| row.get::<_, String>(0))
                .map_err(|error| error.to_string())?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|error| error.to_string())?
        };
        for task_id in abandoned {
            let detail =
                "app instance ended while task was active; remote command outcome is indeterminate";
            tx.execute(
                "UPDATE mcp_task_ledger
                 SET status = 'unknown_after_disconnect', ended_at_ms = ?1,
                     detail = ?2, updated_at_ms = ?1
                 WHERE task_id = ?3",
                params![to_sql_i64(now)?, detail, task_id],
            )
            .map_err(|error| error.to_string())?;
            insert_event(
                &tx,
                &self.app_instance_id,
                &task_id,
                to_sql_i64(now)?,
                TaskStatus::UnknownAfterDisconnect,
                Some(detail),
                None,
            )?;
        }
        tx.commit().map_err(|error| error.to_string())
    }
}

fn ensure_column(conn: &Connection, name: &str, alter_sql: &str) -> Result<(), String> {
    let columns = {
        let mut stmt = conn
            .prepare("PRAGMA table_info(mcp_task_ledger)")
            .map_err(|error| error.to_string())?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|error| error.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|error| error.to_string())?;
        rows
    };
    if !columns.iter().any(|column| column == name) {
        conn.execute(alter_sql, [])
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn read_by_request(
    tx: &Transaction<'_>,
    client_id: &str,
    request_id: &str,
) -> Result<Option<TaskAuditRecord>, String> {
    tx.query_row(
        "SELECT app_instance_id, task_id, client_id, connection_id, generation,
                request_id, request_digest, command, working_directory, timeout_seconds,
                created_at_ms, approved_at_ms, started_at_ms, ended_at_ms,
                status, exit_code, detail, audit_error
         FROM mcp_task_ledger WHERE client_id = ?1 AND request_id = ?2",
        params![client_id, request_id],
        audit_record_from_row,
    )
    .optional()
    .map_err(|error| error.to_string())
}

fn audit_record_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskAuditRecord> {
    let status: String = row.get(14)?;
    Ok(TaskAuditRecord {
        app_instance_id: row.get(0)?,
        task_id: row.get(1)?,
        client_id: row.get(2)?,
        connection_id: row.get(3)?,
        generation: from_sql_u64(row.get(4)?),
        request_id: row.get(5)?,
        request_digest: row.get(6)?,
        command: row.get(7)?,
        working_directory: row.get(8)?,
        timeout_seconds: from_sql_u64(row.get(9)?),
        created_at_ms: from_sql_u64(row.get(10)?),
        approved_at_ms: row.get::<_, Option<i64>>(11)?.map(from_sql_u64),
        started_at_ms: row.get::<_, Option<i64>>(12)?.map(from_sql_u64),
        ended_at_ms: row.get::<_, Option<i64>>(13)?.map(from_sql_u64),
        status: parse_status(&status),
        exit_code: row.get(15)?,
        detail: row.get(16)?,
        audit_error: row.get(17)?,
    })
}

fn insert_event(
    tx: &Transaction<'_>,
    app_instance_id: &str,
    task_id: &str,
    occurred_at_ms: i64,
    status: TaskStatus,
    detail: Option<&str>,
    audit_error: Option<&str>,
) -> Result<(), String> {
    tx.execute(
        "INSERT INTO mcp_task_events
           (task_id, app_instance_id, occurred_at_ms, status, detail, audit_error)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            task_id,
            app_instance_id,
            occurred_at_ms,
            status_name(status),
            detail,
            audit_error,
        ],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

fn status_name(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::PendingApproval => "pending_approval",
        TaskStatus::Queued => "queued",
        TaskStatus::Dispatching => "dispatching",
        TaskStatus::Running => "running",
        TaskStatus::CancelRequested => "cancel_requested",
        TaskStatus::Completed => "completed",
        TaskStatus::Failed => "failed",
        TaskStatus::TimedOut => "timed_out",
        TaskStatus::Cancelled => "cancelled",
        TaskStatus::UnknownAfterDisconnect => "unknown_after_disconnect",
        TaskStatus::Rejected => "rejected",
    }
}

fn parse_status(value: &str) -> TaskStatus {
    match value {
        "pending_approval" => TaskStatus::PendingApproval,
        "queued" => TaskStatus::Queued,
        "dispatching" => TaskStatus::Dispatching,
        "running" => TaskStatus::Running,
        "cancel_requested" => TaskStatus::CancelRequested,
        "completed" => TaskStatus::Completed,
        "failed" => TaskStatus::Failed,
        "timed_out" => TaskStatus::TimedOut,
        "cancelled" => TaskStatus::Cancelled,
        "unknown_after_disconnect" => TaskStatus::UnknownAfterDisconnect,
        "rejected" => TaskStatus::Rejected,
        _ => TaskStatus::Failed,
    }
}

fn to_sql_i64(value: u64) -> Result<i64, String> {
    i64::try_from(value).map_err(|_| "audit numeric value exceeds SQLite integer range".into())
}

fn from_sql_u64(value: i64) -> u64 {
    value.max(0) as u64
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn default_db_path() -> PathBuf {
    crate::ssh_config::get_ssh_config_path()
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("mcp.db")
}

#[cfg(unix)]
fn set_private_directory(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn set_private_directory(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn set_private_file(_path: &Path) -> Result<(), String> {
    Ok(())
}

/// Hash all immutable request fields using length-prefixed components to
/// avoid ambiguous concatenation. The grant token is never an input.
pub(crate) fn request_digest(
    snapshot: &TaskSnapshot,
    grant_id: &str,
    grant_start_seq: u64,
) -> String {
    let mut hasher = Sha256::new();
    let generation_component = snapshot.generation.to_string();
    let timeout_component = snapshot.timeout_seconds.to_string();
    let grant_start_seq_component = grant_start_seq.to_string();
    for component in [
        snapshot.client_id.as_bytes(),
        snapshot.connection_id.as_bytes(),
        generation_component.as_bytes(),
        snapshot.command.as_bytes(),
        snapshot.working_directory.as_bytes(),
        timeout_component.as_bytes(),
        grant_id.as_bytes(),
        grant_start_seq_component.as_bytes(),
    ] {
        hasher.update((component.len() as u64).to_be_bytes());
        hasher.update(component);
    }
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_digest_binds_working_directory_and_timeout() {
        let identity = snapshot("task", "client", "request", TaskStatus::PendingApproval, 0);
        let base = request_digest(&identity, "grant", 0);

        let mut changed_timeout = identity.clone();
        changed_timeout.timeout_seconds = 301;
        assert_ne!(base, request_digest(&changed_timeout, "grant", 0));

        let mut changed_directory = identity;
        changed_directory.working_directory = "other".into();
        assert_ne!(base, request_digest(&changed_directory, "grant", 0));
    }

    fn temp_db_dir(prefix: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("{prefix}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn snapshot(
        task_id: &str,
        client_id: &str,
        request_id: &str,
        status: TaskStatus,
        created_at_ms: u64,
    ) -> TaskSnapshot {
        TaskSnapshot {
            task_id: task_id.into(),
            request_id: request_id.into(),
            client_id: client_id.into(),
            connection_id: "connection".into(),
            generation: 7,
            command: "id".into(),
            working_directory: "login".into(),
            timeout_seconds: 300,
            status,
            created_at_ms,
            approved_at_ms: None,
            started_at_ms: None,
            ended_at_ms: None,
            exit_code: None,
            output_chars: 0,
            output_truncated: false,
            detail: None,
            audit_recorded: true,
            audit_error: None,
        }
    }

    #[test]
    fn opening_a_legacy_ledger_adds_immutable_execution_fields() {
        let dir = temp_db_dir("xt-audit-options-migration");
        let db_path = dir.join("mcp.db");
        Connection::open(&db_path)
            .unwrap()
            .execute_batch(
                "CREATE TABLE mcp_task_ledger (
                   task_id TEXT PRIMARY KEY NOT NULL,
                   app_instance_id TEXT NOT NULL,
                   client_id TEXT NOT NULL,
                   connection_id TEXT NOT NULL,
                   generation INTEGER NOT NULL,
                   request_id TEXT NOT NULL,
                   request_digest TEXT NOT NULL,
                   command TEXT NOT NULL,
                   created_at_ms INTEGER NOT NULL,
                   approved_at_ms INTEGER,
                   started_at_ms INTEGER,
                   ended_at_ms INTEGER,
                   status TEXT NOT NULL,
                   exit_code INTEGER,
                   detail TEXT,
                   audit_error TEXT,
                   updated_at_ms INTEGER NOT NULL,
                   UNIQUE (client_id, request_id)
                 );",
            )
            .unwrap();

        let store = TaskAuditStore::open(&db_path).unwrap();
        let columns = {
            let conn = Connection::open(&db_path).unwrap();
            let mut stmt = conn.prepare("PRAGMA table_info(mcp_task_ledger)").unwrap();
            stmt.query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        assert!(columns.iter().any(|name| name == "working_directory"));
        assert!(columns.iter().any(|name| name == "timeout_seconds"));
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reopening_marks_abandoned_tasks_unknown_and_keeps_transition_events() {
        let dir = temp_db_dir("xt-audit-recovery");
        let db_path = dir.join("mcp.db");
        let first = TaskAuditStore::open(&db_path).unwrap();
        let first_id = first.app_instance_id().to_string();
        let pending = snapshot(
            "task-pending",
            "client",
            "request-pending",
            TaskStatus::PendingApproval,
            100,
        );
        first.create_task(&pending, "digest-pending").unwrap();
        drop(first);

        let second = TaskAuditStore::open(&db_path).unwrap();
        let second_id = second.app_instance_id().to_string();
        let recent = second.recent(None).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].status, TaskStatus::UnknownAfterDisconnect);
        assert!(recent[0].ended_at_ms.is_some());
        assert_eq!(recent[0].app_instance_id, first_id);
        let event_statuses = {
            let conn = Connection::open(&db_path).unwrap();
            let mut stmt = conn
                .prepare("SELECT status FROM mcp_task_events WHERE task_id = ?1 ORDER BY event_id")
                .unwrap();
            stmt.query_map([&pending.task_id], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        let recovery_event_app: String = Connection::open(&db_path)
            .unwrap()
            .query_row(
                "SELECT app_instance_id FROM mcp_task_events
                 WHERE task_id = ?1 AND status = 'unknown_after_disconnect'",
                [&pending.task_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(recovery_event_app, second_id);
        assert_eq!(
            event_statuses,
            vec![
                "pending_approval".to_string(),
                "unknown_after_disconnect".to_string()
            ]
        );
        drop(second);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn sweep_keeps_active_records_and_enforces_age_and_count_for_terminal_rows() {
        let dir = temp_db_dir("xt-audit-sweep");
        let db_path = dir.join("mcp.db");
        let store = TaskAuditStore::open(&db_path).unwrap();
        let aged_terminal = snapshot("task-old", "c1", "r1", TaskStatus::Completed, 100);
        let recent_terminal = snapshot("task-new", "c2", "r2", TaskStatus::Completed, 995);
        let aged_active = snapshot("task-live", "c3", "r3", TaskStatus::PendingApproval, 50);
        store.create_task(&aged_terminal, "digest-1").unwrap();
        store.create_task(&recent_terminal, "digest-2").unwrap();
        store.create_task(&aged_active, "digest-3").unwrap();

        assert_eq!(store.sweep_at(1_000, 10, 2).unwrap(), 1);
        let remaining = store.recent(Some(10)).unwrap();
        let ids = remaining
            .iter()
            .map(|record| record.task_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"task-new"));
        assert!(ids.contains(&"task-live"));
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
