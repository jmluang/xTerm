//! Independent command tasks with per-command user approval (phase C, ORQ-30).
//!
//! Design guarantees:
//! - A task is *created* by an authorized MCP client but only *starts* after
//!   the xTerm user approves the exact command string. The approved
//!   parameters are immutable: the engine executes `task.command`, never
//!   anything passed at approval time, so a stale approval cannot be
//!   repurposed.
//! - Approvals die with the connection generation: reconnect, close, revoke
//!   or client exit cancels pending approvals and requests best-effort
//!   cancellation of running ones.
//! - `request_id` dedupe: client retries return the same task instead of
//!   re-executing the command.
//! - Output is bounded per task (chars + lines) and reports truncation
//!   explicitly. Result state distinguishes completed/failed/timed-out/
//!   cancelled/unknown-after-disconnect — unknown tasks are never replayed.
//! - One running command per connection at a time (per the V1 guidance), so
//!   the human terminal and a single command channel never interleave.

use crate::connection_registry::{ConnectionRecord, ConnectionState};
use crate::mcp::audit::{
    request_digest, CreateTaskResult, TaskAuditRecord, TaskAuditStore, TaskAuditTransition,
};
use crate::mcp::auth::ConnectionGrant;
use serde::Serialize;
use std::collections::HashMap;
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

/// Per-task retained output cap across stdout and stderr, measured in UTF-8
/// bytes. The two streams share this budget; keeping them separate does not
/// double the retained-memory limit.
pub const TASK_OUTPUT_MAX_BYTES: usize = 1_000_000;
/// A single read_task response is bounded across both independent streams.
pub const TASK_READ_MAX_BYTES: usize = 32 * 1024;
/// Pipe readers hand chunks to the supervisor through this bounded queue.
pub const TASK_OUTPUT_CHANNEL_CAPACITY: usize = 64;
pub const TASK_DEFAULT_TIMEOUT_SECONDS: u64 = 300;
/// Hard cap on task lifetime; callers may choose a shorter timeout.
pub const TASK_DEFAULT_TIMEOUT: Duration = Duration::from_secs(TASK_DEFAULT_TIMEOUT_SECONDS);
pub const TASK_MAX_TIMEOUT_SECONDS: u64 = 1800;
#[allow(dead_code)] // retained as phase B/C integration surface
pub const TASK_MAX_TIMEOUT: Duration = Duration::from_secs(TASK_MAX_TIMEOUT_SECONDS);
/// How long finished task results are kept for the owning client to read.
pub const TASK_RETENTION: Duration = Duration::from_secs(600);
pub const TASK_APPROVAL_TIMEOUT: Duration = Duration::from_secs(300);
pub const TASK_MAX_PENDING_PER_CLIENT: usize = 8;
pub const TASK_MAX_PENDING_GLOBAL: usize = 32;
pub const TASK_MAX_RETAINED: usize = 256;
pub const TASK_MAX_REQUEST_ID_BYTES: usize = 128;
pub const TASK_MAX_COMMAND_BYTES: usize = 16 * 1024;
const CANCEL_REQUESTED_DETAIL: &str =
    "cancel requested; local SSH channel will end, but remote descendants are not guaranteed";
const CANCELLED_DETAIL: &str = "local SSH channel ended; remote descendants are not guaranteed";
const TIMED_OUT_DETAIL: &str =
    "local SSH channel ended after timeout; remote descendants are not guaranteed";

/// The two task output streams deliberately have no shared ordering. Each
/// stream has its own byte cursor, so a caller can resume stdout and stderr
/// independently without implying which one arrived first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, Default)]
struct TaskOutputBuffer {
    /// Retained UTF-8 data. The buffer keeps the oldest prefix and reports
    /// truncation once the shared task budget has been exhausted.
    data: String,
    /// Absolute byte offset of `data[0]`. This is kept explicit so reads can
    /// report a gap if the retention policy ever moves the window forward.
    base_offset: usize,
    truncated: bool,
}

#[derive(Debug, Clone)]
struct TaskStreamRead {
    data: String,
    next_offset: usize,
    truncated: bool,
    gap: bool,
}

#[derive(Debug)]
struct TaskOutputChunk {
    stream: StreamKind,
    bytes: Vec<u8>,
}

/// Independent stdout/stderr result returned by the task engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskOutputRead {
    pub stdout: String,
    pub stdout_next_offset: usize,
    pub stdout_truncated: bool,
    pub stdout_gap: bool,
    pub stderr: String,
    pub stderr_next_offset: usize,
    pub stderr_truncated: bool,
    pub stderr_gap: bool,
}

impl TaskOutputBuffer {
    fn len(&self) -> usize {
        self.data.len()
    }

    fn end_offset(&self) -> usize {
        self.base_offset.saturating_add(self.data.len())
    }

    /// Append only a UTF-8-safe prefix. `available` is the remaining shared
    /// task budget, so callers can enforce one cap across both streams.
    fn append(&mut self, data: &str, available: usize) -> usize {
        if data.is_empty() {
            return 0;
        }
        let prefix = safe_prefix_bytes(data, available);
        if prefix.len() < data.len() {
            self.truncated = true;
        }
        if prefix.is_empty() {
            return 0;
        }
        self.data.push_str(&prefix);
        prefix.len()
    }

    fn read(&self, requested_offset: Option<usize>, byte_budget: usize) -> TaskStreamRead {
        let requested = requested_offset.unwrap_or(self.base_offset);
        let end = self.end_offset();
        let mut gap = false;

        // Compare before subtracting so usize::MAX and other arbitrary
        // external offsets never underflow or panic while slicing.
        let start_offset = if requested < self.base_offset {
            gap = true;
            self.base_offset
        } else if requested > end {
            gap = true;
            end
        } else {
            let relative = requested - self.base_offset;
            let adjusted = safe_byte_offset_at_or_after(&self.data, relative);
            if adjusted != relative {
                gap = true;
            }
            self.base_offset.saturating_add(adjusted)
        };

        let relative = start_offset
            .saturating_sub(self.base_offset)
            .min(self.data.len());
        let remaining = &self.data[relative..];
        let prefix = safe_prefix_bytes(remaining, byte_budget);
        let next_offset = start_offset.saturating_add(prefix.len());
        let page_truncated = prefix.len() < remaining.len();
        TaskStreamRead {
            data: prefix,
            next_offset,
            truncated: self.truncated || page_truncated,
            gap,
        }
    }
}

/// Take at most `max_bytes` bytes without splitting a UTF-8 code point.
fn safe_prefix_bytes(data: &str, max_bytes: usize) -> String {
    let end = data
        .char_indices()
        .map(|(index, ch)| index + ch.len_utf8())
        .take_while(|end| *end <= max_bytes)
        .last()
        .unwrap_or(0);
    data[..end].to_string()
}

/// Round an arbitrary byte cursor forward to the next UTF-8 character
/// boundary. An offset at or beyond the end is already safe.
fn safe_byte_offset_at_or_after(data: &str, offset: usize) -> usize {
    let offset = offset.min(data.len());
    if offset == 0 || offset == data.len() || data.is_char_boundary(offset) {
        return offset;
    }
    data.char_indices()
        .find(|(index, _)| *index > offset)
        .map(|(index, _)| index)
        .unwrap_or(data.len())
}

fn spawn_task_output_reader<R>(
    stream: StreamKind,
    mut pipe: R,
    sender: std::sync::mpsc::SyncSender<TaskOutputChunk>,
) where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match pipe.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if sender
                        .send(TaskOutputChunk {
                            stream,
                            bytes: buf[..n].to_vec(),
                        })
                        .is_err()
                    {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Created, waiting for the xTerm user's approval.
    PendingApproval,
    /// Approved, waiting for the per-connection slot.
    Queued,
    /// Mux has verified; final authorization and local process startup are in progress.
    Dispatching,
    Running,
    /// A local channel cancellation has been durably requested but not yet confirmed.
    CancelRequested,
    Completed,
    Failed,
    TimedOut,
    /// Cancel was requested *and* the ssh child was confirmed dead.
    Cancelled,
    /// The connection/master died mid-run. We do not know whether the remote
    /// command finished; it is never replayed automatically.
    UnknownAfterDisconnect,
    /// Approval was never granted (revoked, rejected or expired).
    Rejected,
}

impl TaskStatus {
    pub fn is_terminal(self) -> bool {
        !matches!(
            self,
            TaskStatus::PendingApproval
                | TaskStatus::Queued
                | TaskStatus::Dispatching
                | TaskStatus::Running
                | TaskStatus::CancelRequested
        )
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSnapshot {
    pub task_id: String,
    pub request_id: String,
    pub client_id: String,
    pub connection_id: String,
    pub generation: u64,
    pub command: String,
    pub working_directory: String,
    pub timeout_seconds: u64,
    pub status: TaskStatus,
    pub created_at_ms: u64,
    pub approved_at_ms: Option<u64>,
    pub started_at_ms: Option<u64>,
    pub ended_at_ms: Option<u64>,
    pub exit_code: Option<i32>,
    pub output_chars: usize,
    pub output_truncated: bool,
    pub detail: Option<String>,
    /// Whether the latest lifecycle state was durably written to the audit DB.
    pub audit_recorded: bool,
    /// Last audit error observed in memory or persisted by a later successful write.
    pub audit_error: Option<String>,
}

/// Immutable MCP command input, kept together so every lifecycle operation
/// observes the same request identity and execution constraints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandSpec<'a> {
    pub request_id: &'a str,
    pub command: &'a str,
    pub working_directory: &'a str,
    pub timeout_seconds: u64,
}

struct Task {
    snapshot: TaskSnapshot,
    grant_scope: GrantScope,
    dispatch_recorded: bool,
    stdout: TaskOutputBuffer,
    stderr: TaskOutputBuffer,
    cancelled: Arc<AtomicBool>,
    spawn_gate: Arc<Mutex<()>>,
    child: Option<Arc<Mutex<Child>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GrantScope {
    grant_id: String,
    grant_start_seq: u64,
}

impl Task {
    fn to_snapshot(&self) -> TaskSnapshot {
        self.snapshot.clone()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitError {
    /// Connection missing, not managed, closing/exited, or generation
    /// mismatch — the request can never be satisfied, do not retry blindly.
    ConnectionUnavailable,
    /// Connection exists but has not finished mux verification yet.
    NotReady,
    /// The request_id already names a different immutable request or grant scope.
    RequestConflict,
    /// This request_id was recorded by an earlier app instance and is never replayed.
    RequestFromPreviousSession,
    /// The audit ledger could not record the request, so no task was created.
    AuditUnavailable,
    /// The supplied grant does not belong to this client and connection generation.
    GrantMismatch,
    InvalidRequestId,
    InvalidCommand,
    InvalidWorkingDirectory,
    InvalidTimeout,
    PendingApprovalLimitPerClient,
    PendingApprovalLimitGlobal,
    RetainedTaskLimit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalError {
    /// No such task, wrong owner, or the task already left PendingApproval.
    NotApprovable,
    /// Another queued or running task already owns this connection's slot.
    ConnectionBusy,
    /// The connection generation moved on since submission: stale approvals
    /// must not execute.
    StaleGeneration,
    /// The audit ledger could not record the approval or rejection transition.
    AuditUnavailable,
    /// The request exceeded its five-minute approval window.
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskTransitionError {
    /// The audit ledger could not record the requested status transition.
    AuditUnavailable,
    /// The local supervisor thread could not be started.
    DispatchFailed,
    /// A task has already reached a terminal state and cannot be cancelled.
    NotCancellable,
}

pub struct TaskEngine {
    tasks: Mutex<HashMap<String, Task>>,
    /// (client_id, request_id) -> task_id, for idempotent retries.
    by_request: Mutex<HashMap<(String, String), String>>,
    /// Serializes lookup/create/index updates so concurrent retries cannot
    /// create multiple tasks for the same request_id.
    submit_lock: Mutex<()>,
    /// Serializes status transitions without holding task/running locks across SQLite I/O.
    lifecycle_lock: Mutex<()>,
    /// connection_id -> running task_id (one command channel per connection).
    running_by_connection: Mutex<HashMap<String, String>>,
    cleanup_counter: AtomicU64,
    /// Always `/usr/bin/ssh` in production; overridable in tests so the
    /// engine's lifecycle can be exercised without a real SSH server.
    ssh_bin: String,
    audit: Option<Arc<TaskAuditStore>>,
}

impl Default for TaskEngine {
    fn default() -> Self {
        Self::build(None)
    }
}

impl TaskEngine {
    pub(crate) fn with_audit(audit: Arc<TaskAuditStore>) -> Self {
        Self::build(Some(audit))
    }

    fn build(audit: Option<Arc<TaskAuditStore>>) -> Self {
        Self {
            tasks: Mutex::new(HashMap::new()),
            by_request: Mutex::new(HashMap::new()),
            submit_lock: Mutex::new(()),
            lifecycle_lock: Mutex::new(()),
            running_by_connection: Mutex::new(HashMap::new()),
            cleanup_counter: AtomicU64::new(0),
            ssh_bin: "/usr/bin/ssh".to_string(),
            audit,
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl TaskEngine {
    /// Create a task in PendingApproval. Idempotency is scoped to a client and
    /// request_id, and only an exact retry under the same grant returns the
    /// original task.
    pub fn submit(
        &self,
        client_id: &str,
        record: &ConnectionRecord,
        spec: CommandSpec<'_>,
        grant: &ConnectionGrant,
    ) -> Result<TaskSnapshot, SubmitError> {
        let _submit_guard = self
            .submit_lock
            .lock()
            .map_err(|_| SubmitError::ConnectionUnavailable)?;
        self.expire_pending_approvals()
            .map_err(|_| SubmitError::AuditUnavailable)?;

        if spec.request_id.is_empty() || spec.request_id.len() > TASK_MAX_REQUEST_ID_BYTES {
            return Err(SubmitError::InvalidRequestId);
        }
        if spec.command.is_empty() || spec.command.len() > TASK_MAX_COMMAND_BYTES {
            return Err(SubmitError::InvalidCommand);
        }
        if spec.working_directory != "login" {
            return Err(SubmitError::InvalidWorkingDirectory);
        }
        if !(1..=TASK_MAX_TIMEOUT_SECONDS).contains(&spec.timeout_seconds) {
            return Err(SubmitError::InvalidTimeout);
        }

        if grant.client_id != client_id
            || grant.connection_id != record.connection_id
            || grant.generation != record.generation
        {
            return Err(SubmitError::GrantMismatch);
        }

        let grant_scope = GrantScope {
            grant_id: grant.grant_id.clone(),
            grant_start_seq: grant.grant_start_seq,
        };
        let key = (client_id.to_string(), spec.request_id.to_string());
        let existing = self
            .by_request
            .lock()
            .map_err(|_| SubmitError::ConnectionUnavailable)?
            .get(&key)
            .cloned();
        if let Some(existing_task_id) = existing {
            let existing_task = self.tasks.lock().ok().and_then(|tasks| {
                tasks
                    .get(&existing_task_id)
                    .map(|task| (task.to_snapshot(), task.grant_scope.clone()))
            });
            if let Some((snapshot, existing_scope)) = existing_task {
                if snapshot.connection_id != record.connection_id
                    || snapshot.generation != record.generation
                    || snapshot.command != spec.command
                    || snapshot.working_directory != spec.working_directory
                    || snapshot.timeout_seconds != spec.timeout_seconds
                    || existing_scope != grant_scope
                {
                    return Err(SubmitError::RequestConflict);
                }
                return Ok(snapshot);
            }
        }

        if !record.mcp_eligible() {
            return Err(SubmitError::ConnectionUnavailable);
        }
        if record.state != ConnectionState::Ready {
            return Err(SubmitError::NotReady);
        }

        self.ensure_task_capacity()?;
        self.ensure_pending_capacity(client_id)?;

        let task_id = uuid::Uuid::new_v4().to_string();
        let snapshot = TaskSnapshot {
            task_id: task_id.clone(),
            request_id: spec.request_id.to_string(),
            client_id: client_id.to_string(),
            connection_id: record.connection_id.clone(),
            generation: record.generation,
            command: spec.command.to_string(),
            working_directory: spec.working_directory.to_string(),
            timeout_seconds: spec.timeout_seconds,
            status: TaskStatus::PendingApproval,
            created_at_ms: now_ms(),
            approved_at_ms: None,
            started_at_ms: None,
            ended_at_ms: None,
            exit_code: None,
            output_chars: 0,
            output_truncated: false,
            detail: None,
            audit_recorded: self.audit.is_some(),
            audit_error: None,
        };
        let digest = request_digest(
            &snapshot,
            &grant_scope.grant_id,
            grant_scope.grant_start_seq,
        );
        if let Some(audit) = &self.audit {
            match audit.create_task(&snapshot, &digest) {
                Ok(CreateTaskResult::Inserted) => {}
                Ok(CreateTaskResult::Existing(record)) => {
                    if record.app_instance_id != audit.app_instance_id() {
                        return Err(SubmitError::RequestFromPreviousSession);
                    }
                    if record.request_digest != digest {
                        return Err(SubmitError::RequestConflict);
                    }
                    let snapshot = record.to_snapshot();
                    let task = Task {
                        snapshot: snapshot.clone(),
                        grant_scope,
                        dispatch_recorded: snapshot.started_at_ms.is_some(),
                        stdout: TaskOutputBuffer::default(),
                        stderr: TaskOutputBuffer::default(),
                        cancelled: Arc::new(AtomicBool::new(false)),
                        spawn_gate: Arc::new(Mutex::new(())),
                        child: None,
                    };
                    self.tasks
                        .lock()
                        .map_err(|_| SubmitError::ConnectionUnavailable)?
                        .insert(snapshot.task_id.clone(), task);
                    self.by_request
                        .lock()
                        .map_err(|_| SubmitError::ConnectionUnavailable)?
                        .insert(key, snapshot.task_id.clone());
                    return Ok(snapshot);
                }
                Err(_) => return Err(SubmitError::AuditUnavailable),
            }
        }
        let task = Task {
            snapshot,
            grant_scope,
            dispatch_recorded: false,
            stdout: TaskOutputBuffer::default(),
            stderr: TaskOutputBuffer::default(),
            cancelled: Arc::new(AtomicBool::new(false)),
            spawn_gate: Arc::new(Mutex::new(())),
            child: None,
        };
        {
            let mut tasks = self
                .tasks
                .lock()
                .map_err(|_| SubmitError::ConnectionUnavailable)?;
            tasks.insert(task_id.clone(), task);
        }
        self.by_request
            .lock()
            .map_err(|_| SubmitError::ConnectionUnavailable)?
            .insert(key, task_id.clone());
        Ok(self.get_task_by_id(&task_id).expect("just inserted"))
    }

    fn ensure_task_capacity(&self) -> Result<(), SubmitError> {
        let removed = {
            let mut tasks = self
                .tasks
                .lock()
                .map_err(|_| SubmitError::ConnectionUnavailable)?;
            if tasks.len() < TASK_MAX_RETAINED {
                return Ok(());
            }
            let terminal_ids: Vec<String> = tasks
                .values()
                .filter(|task| task.snapshot.status.is_terminal())
                .map(|task| task.snapshot.task_id.clone())
                .collect();
            for task_id in &terminal_ids {
                tasks.remove(task_id);
            }
            if tasks.len() >= TASK_MAX_RETAINED {
                return Err(SubmitError::RetainedTaskLimit);
            }
            terminal_ids
        };

        if !removed.is_empty() {
            let removed: std::collections::HashSet<&str> =
                removed.iter().map(String::as_str).collect();
            self.by_request
                .lock()
                .map_err(|_| SubmitError::ConnectionUnavailable)?
                .retain(|_, task_id| !removed.contains(task_id.as_str()));
        }
        Ok(())
    }

    fn ensure_pending_capacity(&self, client_id: &str) -> Result<(), SubmitError> {
        let tasks = self
            .tasks
            .lock()
            .map_err(|_| SubmitError::ConnectionUnavailable)?;
        let global = tasks
            .values()
            .filter(|task| task.snapshot.status == TaskStatus::PendingApproval)
            .count();
        if global >= TASK_MAX_PENDING_GLOBAL {
            return Err(SubmitError::PendingApprovalLimitGlobal);
        }
        let client = tasks
            .values()
            .filter(|task| {
                task.snapshot.status == TaskStatus::PendingApproval
                    && task.snapshot.client_id == client_id
            })
            .count();
        if client >= TASK_MAX_PENDING_PER_CLIENT {
            return Err(SubmitError::PendingApprovalLimitPerClient);
        }
        Ok(())
    }

    /// Expire approvals using the same durable transition path as explicit
    /// rejection. Callers may run this from request handling or maintenance.
    pub fn expire_pending_approvals(&self) -> Result<usize, TaskTransitionError> {
        let _lifecycle = self
            .lifecycle_lock
            .lock()
            .map_err(|_| TaskTransitionError::AuditUnavailable)?;
        self.expire_pending_approvals_locked()
    }

    fn expire_pending_approvals_locked(&self) -> Result<usize, TaskTransitionError> {
        let now = now_ms();
        let cutoff_ms = TASK_APPROVAL_TIMEOUT.as_millis() as u64;
        let expired = self
            .tasks
            .lock()
            .map_err(|_| TaskTransitionError::AuditUnavailable)?
            .values()
            .filter(|task| {
                task.snapshot.status == TaskStatus::PendingApproval
                    && now.saturating_sub(task.snapshot.created_at_ms) >= cutoff_ms
            })
            .map(Task::to_snapshot)
            .collect::<Vec<_>>();

        let mut expired_count = 0;
        for snapshot in expired {
            let ended_at_ms = now_ms();
            const DETAIL: &str = "approval expired after 5 minutes";
            if let Err(error) = self.record_transition(
                &snapshot,
                TaskAuditTransition {
                    status: TaskStatus::Rejected,
                    approved_at_ms: None,
                    started_at_ms: None,
                    ended_at_ms: Some(ended_at_ms),
                    exit_code: None,
                    detail: Some(DETAIL),
                    audit_error: None,
                },
            ) {
                self.mark_audit_failure(&snapshot.task_id, &error, "approval expiration");
                return Err(TaskTransitionError::AuditUnavailable);
            }

            let mut tasks = self
                .tasks
                .lock()
                .map_err(|_| TaskTransitionError::AuditUnavailable)?;
            if let Some(task) = tasks.get_mut(&snapshot.task_id) {
                if task.snapshot.status == TaskStatus::PendingApproval {
                    task.snapshot.status = TaskStatus::Rejected;
                    task.snapshot.ended_at_ms = Some(ended_at_ms);
                    task.snapshot.detail = Some(DETAIL.to_string());
                    task.snapshot.audit_recorded = self.audit.is_some();
                    expired_count += 1;
                }
            }
        }
        Ok(expired_count)
    }

    fn pending_approvals_unchecked(&self) -> Result<Vec<TaskSnapshot>, TaskTransitionError> {
        self.tasks
            .lock()
            .map(|tasks| {
                tasks
                    .values()
                    .filter(|task| task.snapshot.status == TaskStatus::PendingApproval)
                    .map(Task::to_snapshot)
                    .collect()
            })
            .map_err(|_| TaskTransitionError::AuditUnavailable)
    }

    fn get_task_by_id(&self, task_id: &str) -> Option<TaskSnapshot> {
        self.tasks
            .lock()
            .ok()
            .and_then(|tasks| tasks.get(task_id).map(Task::to_snapshot))
    }

    fn record_transition<'a>(
        &self,
        snapshot: &'a TaskSnapshot,
        mut transition: TaskAuditTransition<'a>,
    ) -> Result<(), String> {
        let Some(audit) = &self.audit else {
            return Ok(());
        };
        transition.audit_error = snapshot.audit_error.as_deref();
        audit.record_transition(&snapshot.task_id, transition)
    }

    fn mark_audit_failure(&self, task_id: &str, error: &str, transition: &str) {
        if let Ok(mut tasks) = self.tasks.lock() {
            if let Some(task) = tasks.get_mut(task_id) {
                task.snapshot.audit_recorded = false;
                task.snapshot.audit_error = Some(error.to_string());
                task.snapshot.detail = Some(format!(
                    "audit write failed; {transition} was blocked: {error}"
                ));
            }
        }
    }

    /// Unscoped snapshot for trusted in-process callers (the MCP service's
    /// approval/start path). Never exposed over IPC.
    pub fn internal_snapshot(&self, task_id: &str) -> Option<TaskSnapshot> {
        self.get_task_by_id(task_id)
    }

    /// Read a task, enforcing ownership: a client can only ever see its own
    /// tasks.
    pub fn get_task_for_client(&self, client_id: &str, task_id: &str) -> Option<TaskSnapshot> {
        let snapshot = self.get_task_by_id(task_id)?;
        if snapshot.client_id != client_id {
            return None;
        }
        Some(snapshot)
    }

    /// A current grant may access only tasks created under that same grant
    /// scope. Regranting a connection never restores access to older tasks.
    pub fn grant_matches_task(
        &self,
        client_id: &str,
        task_id: &str,
        grant: &ConnectionGrant,
    ) -> bool {
        self.tasks
            .lock()
            .ok()
            .and_then(|tasks| {
                tasks.get(task_id).map(|task| {
                    task.snapshot.client_id == client_id
                        && task.snapshot.connection_id == grant.connection_id
                        && task.snapshot.generation == grant.generation
                        && task.grant_scope.grant_id == grant.grant_id
                        && task.grant_scope.grant_start_seq == grant.grant_start_seq
                })
            })
            .unwrap_or(false)
    }

    /// Read and consume nothing from the two independent task streams. The
    /// returned response is bounded across both streams and each stream
    /// reports its own cursor, truncation and gap state.
    pub fn read_task_output_for_client(
        &self,
        client_id: &str,
        task_id: &str,
        stdout_offset: Option<usize>,
        stderr_offset: Option<usize>,
    ) -> Option<TaskOutputRead> {
        let mut tasks = self.tasks.lock().ok()?;
        let task = tasks.get_mut(task_id)?;
        if task.snapshot.client_id != client_id {
            return None;
        }

        // Reserve a fair first share for each stream, then use any remaining
        // budget for a stream that still has data. This keeps one noisy stream
        // from making the other unreadable while preserving the hard total.
        let stdout_probe = task.stdout.read(stdout_offset, TASK_READ_MAX_BYTES);
        let stderr_probe = task.stderr.read(stderr_offset, TASK_READ_MAX_BYTES);
        let half = TASK_READ_MAX_BYTES / 2;
        let mut stdout_budget = stdout_probe.data.len().min(half);
        let mut stderr_budget = stderr_probe.data.len().min(half);
        let mut remaining = TASK_READ_MAX_BYTES - stdout_budget - stderr_budget;
        if remaining > 0 {
            let stdout_extra = stdout_probe
                .data
                .len()
                .saturating_sub(stdout_budget)
                .min(remaining);
            stdout_budget += stdout_extra;
            remaining -= stdout_extra;
        }
        if remaining > 0 {
            let stderr_extra = stderr_probe
                .data
                .len()
                .saturating_sub(stderr_budget)
                .min(remaining);
            stderr_budget += stderr_extra;
        }

        let stdout = task.stdout.read(stdout_offset, stdout_budget);
        let stderr = task.stderr.read(stderr_offset, stderr_budget);
        Some(TaskOutputRead {
            stdout: stdout.data,
            stdout_next_offset: stdout.next_offset,
            stdout_truncated: stdout.truncated,
            stdout_gap: stdout.gap,
            stderr: stderr.data,
            stderr_next_offset: stderr.next_offset,
            stderr_truncated: stderr.truncated,
            stderr_gap: stderr.gap,
        })
    }

    #[cfg(test)]
    pub(crate) fn task_output_bytes(&self, task_id: &str) -> Option<usize> {
        self.tasks.lock().ok().and_then(|tasks| {
            tasks
                .get(task_id)
                .map(|task| task.stdout.len() + task.stderr.len())
        })
    }

    /// Approve the exact command captured at submit time. The UI passes the
    /// current generation of the connection so an approval that crosses a
    /// reconnect is refused instead of executing against the new session.
    pub fn approve(
        &self,
        task_id: &str,
        current_generation: u64,
    ) -> Result<TaskSnapshot, ApprovalError> {
        let _lifecycle = self
            .lifecycle_lock
            .lock()
            .map_err(|_| ApprovalError::NotApprovable)?;
        self.expire_pending_approvals_locked()
            .map_err(|_| ApprovalError::AuditUnavailable)?;
        let snapshot = self
            .get_task_by_id(task_id)
            .ok_or(ApprovalError::NotApprovable)?;
        if snapshot.status == TaskStatus::Rejected
            && snapshot.detail.as_deref() == Some("approval expired after 5 minutes")
        {
            return Err(ApprovalError::Expired);
        }
        if snapshot.status != TaskStatus::PendingApproval {
            return Err(ApprovalError::NotApprovable);
        }
        let stale_generation = snapshot.generation != current_generation;
        let approved_at_ms = (!stale_generation).then(now_ms);
        let ended_at_ms = stale_generation.then(now_ms);
        let target_status = if stale_generation {
            TaskStatus::Rejected
        } else {
            TaskStatus::Queued
        };
        let detail = stale_generation.then_some("connection generation changed before approval");
        if !stale_generation {
            let mut slots = self
                .running_by_connection
                .lock()
                .map_err(|_| ApprovalError::NotApprovable)?;
            if slots
                .get(&snapshot.connection_id)
                .is_some_and(|owner| owner != task_id)
            {
                return Err(ApprovalError::ConnectionBusy);
            }
            slots.insert(snapshot.connection_id.clone(), task_id.to_string());
        }
        if let Err(error) = self.record_transition(
            &snapshot,
            TaskAuditTransition {
                status: target_status,
                approved_at_ms,
                started_at_ms: None,
                ended_at_ms,
                exit_code: None,
                detail,
                audit_error: None,
            },
        ) {
            if !stale_generation {
                self.release_connection_slot(&snapshot.connection_id, task_id);
            }
            self.mark_audit_failure(task_id, &error, "approval");
            return Err(ApprovalError::AuditUnavailable);
        }

        let mut tasks = self
            .tasks
            .lock()
            .map_err(|_| ApprovalError::NotApprovable)?;
        let Some(task) = tasks.get_mut(task_id) else {
            drop(tasks);
            if !stale_generation {
                self.release_connection_slot(&snapshot.connection_id, task_id);
            }
            return Err(ApprovalError::NotApprovable);
        };
        if task.snapshot.status != TaskStatus::PendingApproval {
            drop(tasks);
            if !stale_generation {
                self.release_connection_slot(&snapshot.connection_id, task_id);
            }
            return Err(ApprovalError::NotApprovable);
        }
        task.snapshot.status = target_status;
        task.snapshot.approved_at_ms = approved_at_ms;
        task.snapshot.ended_at_ms = ended_at_ms;
        task.snapshot.detail = detail.map(str::to_owned);
        task.snapshot.audit_recorded = self.audit.is_some();
        if stale_generation {
            return Err(ApprovalError::StaleGeneration);
        }
        Ok(task.to_snapshot())
    }

    pub fn reject(&self, task_id: &str) -> Result<Option<TaskSnapshot>, TaskTransitionError> {
        let _lifecycle = self
            .lifecycle_lock
            .lock()
            .map_err(|_| TaskTransitionError::AuditUnavailable)?;
        let Some(snapshot) = self.get_task_by_id(task_id) else {
            return Ok(None);
        };
        if snapshot.status != TaskStatus::PendingApproval {
            return Ok(None);
        }
        let ended_at_ms = now_ms();
        if let Err(error) = self.record_transition(
            &snapshot,
            TaskAuditTransition {
                status: TaskStatus::Rejected,
                approved_at_ms: None,
                started_at_ms: None,
                ended_at_ms: Some(ended_at_ms),
                exit_code: None,
                detail: snapshot.detail.as_deref(),
                audit_error: None,
            },
        ) {
            self.mark_audit_failure(task_id, &error, "rejection");
            return Err(TaskTransitionError::AuditUnavailable);
        }
        let mut tasks = self
            .tasks
            .lock()
            .map_err(|_| TaskTransitionError::AuditUnavailable)?;
        let Some(task) = tasks.get_mut(task_id) else {
            return Ok(None);
        };
        if task.snapshot.status != TaskStatus::PendingApproval {
            return Ok(None);
        }
        task.snapshot.status = TaskStatus::Rejected;
        task.snapshot.ended_at_ms = Some(ended_at_ms);
        task.snapshot.audit_recorded = self.audit.is_some();
        Ok(Some(task.to_snapshot()))
    }

    /// Record that dispatching has started before the access gate is acquired.
    /// The task remains Dispatching and retains its reserved slot until the
    /// local supervisor records the Running transition immediately before the
    /// SSH child is spawned.
    pub fn record_dispatch_start(&self, task_id: &str) -> Result<bool, TaskTransitionError> {
        let _lifecycle = self
            .lifecycle_lock
            .lock()
            .map_err(|_| TaskTransitionError::AuditUnavailable)?;
        let Some(snapshot) = self.get_task_by_id(task_id) else {
            return Ok(false);
        };
        if snapshot.status != TaskStatus::Queued
            || self
                .running_by_connection
                .lock()
                .map_err(|_| TaskTransitionError::AuditUnavailable)?
                .get(&snapshot.connection_id)
                .map(String::as_str)
                != Some(task_id)
        {
            return Ok(false);
        }
        if self
            .tasks
            .lock()
            .map_err(|_| TaskTransitionError::AuditUnavailable)?
            .get(task_id)
            .is_some_and(|task| task.dispatch_recorded)
        {
            return Ok(true);
        }

        if let Err(error) = self.record_transition(
            &snapshot,
            TaskAuditTransition {
                status: TaskStatus::Dispatching,
                approved_at_ms: None,
                started_at_ms: None,
                ended_at_ms: None,
                exit_code: None,
                detail: snapshot.detail.as_deref(),
                audit_error: None,
            },
        ) {
            self.mark_audit_failure(task_id, &error, "dispatch");
            return Err(TaskTransitionError::AuditUnavailable);
        }

        let mut tasks = self
            .tasks
            .lock()
            .map_err(|_| TaskTransitionError::AuditUnavailable)?;
        let Some(task) = tasks.get_mut(task_id) else {
            return Ok(false);
        };
        if task.snapshot.status != TaskStatus::Queued {
            return Ok(false);
        }
        task.dispatch_recorded = true;
        task.snapshot.status = TaskStatus::Dispatching;
        task.snapshot.audit_recorded = self.audit.is_some();
        Ok(true)
    }

    /// Record Running at the final spawn boundary. The caller must hold the
    /// task's spawn mutex so cancellation cannot pass the check between this
    /// durable transition and `Command::spawn`.
    fn record_running_start(&self, task_id: &str) -> Result<bool, TaskTransitionError> {
        let _lifecycle = self
            .lifecycle_lock
            .lock()
            .map_err(|_| TaskTransitionError::AuditUnavailable)?;
        let Some(snapshot) = self.get_task_by_id(task_id) else {
            return Ok(false);
        };
        if snapshot.status != TaskStatus::Dispatching {
            return Ok(false);
        }
        let started_at_ms = now_ms();
        if let Err(error) = self.record_transition(
            &snapshot,
            TaskAuditTransition {
                status: TaskStatus::Running,
                approved_at_ms: None,
                started_at_ms: Some(started_at_ms),
                ended_at_ms: None,
                exit_code: None,
                detail: snapshot.detail.as_deref(),
                audit_error: None,
            },
        ) {
            self.mark_audit_failure(task_id, &error, "running transition");
            return Err(TaskTransitionError::AuditUnavailable);
        }
        let mut tasks = self
            .tasks
            .lock()
            .map_err(|_| TaskTransitionError::AuditUnavailable)?;
        let Some(task) = tasks.get_mut(task_id) else {
            return Ok(false);
        };
        if task.snapshot.status != TaskStatus::Dispatching {
            return Ok(false);
        }
        task.snapshot.status = TaskStatus::Running;
        task.snapshot.started_at_ms = Some(started_at_ms);
        task.snapshot.audit_recorded = self.audit.is_some();
        Ok(true)
    }

    /// Terminalize a task whose mux verification or pre-spawn dispatch failed.
    /// Durable audit is written before the in-memory status and slot change.
    pub fn fail_dispatch(
        &self,
        task_id: &str,
        status: TaskStatus,
        detail: &str,
    ) -> Result<Option<TaskSnapshot>, TaskTransitionError> {
        if !matches!(status, TaskStatus::Failed | TaskStatus::Rejected) {
            return Ok(None);
        }
        let _lifecycle = self
            .lifecycle_lock
            .lock()
            .map_err(|_| TaskTransitionError::AuditUnavailable)?;
        let Some(snapshot) = self.get_task_by_id(task_id) else {
            return Ok(None);
        };
        if !matches!(
            snapshot.status,
            TaskStatus::Queued | TaskStatus::Dispatching | TaskStatus::Running
        ) {
            return Ok(None);
        }
        let ended_at_ms = now_ms();
        if let Err(error) = self.record_transition(
            &snapshot,
            TaskAuditTransition {
                status,
                approved_at_ms: None,
                started_at_ms: None,
                ended_at_ms: Some(ended_at_ms),
                exit_code: None,
                detail: Some(detail),
                audit_error: None,
            },
        ) {
            self.mark_audit_failure(task_id, &error, "dispatch failure");
            return Err(TaskTransitionError::AuditUnavailable);
        }
        let mut tasks = self
            .tasks
            .lock()
            .map_err(|_| TaskTransitionError::AuditUnavailable)?;
        let Some(task) = tasks.get_mut(task_id) else {
            return Ok(None);
        };
        if !matches!(
            task.snapshot.status,
            TaskStatus::Queued | TaskStatus::Dispatching | TaskStatus::Running
        ) {
            return Ok(None);
        }
        task.snapshot.status = status;
        task.snapshot.ended_at_ms = Some(ended_at_ms);
        task.snapshot.detail = Some(detail.to_string());
        task.snapshot.audit_recorded = self.audit.is_some();
        task.child = None;
        let snapshot = task.to_snapshot();
        let connection_id = task.snapshot.connection_id.clone();
        drop(tasks);
        self.release_connection_slot(&connection_id, task_id);
        Ok(Some(snapshot))
    }

    /// Launch a previously audited, still-queued task. The caller holds the
    /// service access gate while scheduling the local supervisor thread; the
    /// worker reacquires its read side around the final cancellation check and
    /// local `ssh` spawn.
    pub fn start_prepared(
        self: &Arc<Self>,
        task_id: &str,
        spawn_args: Vec<String>,
        spawn_gate: Option<Arc<RwLock<()>>>,
    ) -> Result<bool, TaskTransitionError> {
        let lifecycle = self
            .lifecycle_lock
            .lock()
            .map_err(|_| TaskTransitionError::AuditUnavailable)?;
        let Some(snapshot) = self.get_task_by_id(task_id) else {
            return Ok(false);
        };
        if snapshot.status != TaskStatus::Dispatching
            || self
                .running_by_connection
                .lock()
                .map_err(|_| TaskTransitionError::AuditUnavailable)?
                .get(&snapshot.connection_id)
                .map(String::as_str)
                != Some(task_id)
        {
            return Ok(false);
        }
        let (cancelled, spawn_mutex) = {
            let tasks = self
                .tasks
                .lock()
                .map_err(|_| TaskTransitionError::AuditUnavailable)?;
            let Some(task) = tasks.get(task_id) else {
                return Ok(false);
            };
            if !task.dispatch_recorded || task.snapshot.status != TaskStatus::Dispatching {
                return Ok(false);
            }
            (task.cancelled.clone(), task.spawn_gate.clone())
        };

        let (ready_sender, ready_receiver) = std::sync::mpsc::sync_channel::<()>(0);
        let engine = Arc::clone(self);
        let task_id_owned = task_id.to_string();
        let connection_id = snapshot.connection_id.clone();
        std::thread::Builder::new()
            .name("xtermius-mcp-task".into())
            .spawn(move || {
                if ready_receiver.recv().is_ok() {
                    engine.run_task(
                        &task_id_owned,
                        spawn_args,
                        cancelled,
                        connection_id,
                        spawn_mutex,
                        spawn_gate,
                    );
                }
            })
            .map_err(|_| TaskTransitionError::DispatchFailed)?;

        let mut tasks = self
            .tasks
            .lock()
            .map_err(|_| TaskTransitionError::AuditUnavailable)?;
        let Some(task) = tasks.get_mut(task_id) else {
            drop(tasks);
            drop(lifecycle);
            drop(ready_sender);
            return Ok(false);
        };
        if task.snapshot.status != TaskStatus::Dispatching || !task.dispatch_recorded {
            drop(tasks);
            drop(lifecycle);
            drop(ready_sender);
            return Ok(false);
        }
        task.snapshot.audit_recorded = self.audit.is_some();
        drop(tasks);
        drop(lifecycle);
        ready_sender
            .send(())
            .map_err(|_| TaskTransitionError::DispatchFailed)?;
        Ok(true)
    }

    /// In-process convenience wrapper for unit tests. Production dispatch
    /// performs its final authorization check in `McpService` first.
    #[cfg(test)]
    pub fn start_approved(
        self: &Arc<Self>,
        task_id: &str,
        spawn_args: Vec<String>,
    ) -> Result<bool, TaskTransitionError> {
        match self.record_dispatch_start(task_id) {
            Ok(true) => {}
            Ok(false) => return Ok(false),
            Err(error) => return Err(error),
        }
        match self.start_prepared(task_id, spawn_args, None) {
            Ok(true) => Ok(true),
            Ok(false) => Ok(false),
            Err(error) => {
                let _ = self.fail_dispatch(
                    task_id,
                    TaskStatus::Failed,
                    "failed to start the local task supervisor",
                );
                Err(error)
            }
        }
    }

    fn run_task(
        &self,
        task_id: &str,
        spawn_args: Vec<String>,
        cancelled: Arc<AtomicBool>,
        connection_id: String,
        spawn_mutex: Arc<Mutex<()>>,
        spawn_gate: Option<Arc<RwLock<()>>>,
    ) {
        let access_guard = match spawn_gate.as_ref() {
            Some(gate) => match gate.read() {
                Ok(guard) => Some(guard),
                Err(_) => {
                    self.finish_task(
                        task_id,
                        TaskStatus::Failed,
                        None,
                        Some("MCP access gate was unavailable before SSH spawn".into()),
                        &connection_id,
                    );
                    return;
                }
            },
            None => None,
        };
        let spawn_guard = match spawn_mutex.lock() {
            Ok(guard) => guard,
            Err(_) => {
                drop(access_guard);
                self.finish_task(
                    task_id,
                    TaskStatus::Failed,
                    None,
                    Some("task spawn gate was unavailable".into()),
                    &connection_id,
                );
                return;
            }
        };
        if cancelled.load(Ordering::SeqCst) {
            drop(spawn_guard);
            drop(access_guard);
            self.finish_task(
                task_id,
                TaskStatus::Cancelled,
                None,
                Some(
                    "local SSH channel was never opened; remote descendants are not guaranteed"
                        .into(),
                ),
                &connection_id,
            );
            return;
        }
        match self.record_running_start(task_id) {
            Ok(true) => {}
            Ok(false) => {
                drop(spawn_guard);
                drop(access_guard);
                self.finish_task(
                    task_id,
                    TaskStatus::Cancelled,
                    None,
                    Some(
                        "local SSH channel was never opened; remote descendants are not guaranteed"
                            .into(),
                    ),
                    &connection_id,
                );
                return;
            }
            Err(error) => {
                let detail = "running audit transition failed; local SSH channel was not spawned";
                let _ = self.fail_dispatch(task_id, TaskStatus::Failed, detail);
                drop(spawn_guard);
                drop(access_guard);
                if let Some(snapshot) = self.internal_snapshot(task_id) {
                    if !snapshot.status.is_terminal() {
                        self.finish_task(
                            task_id,
                            TaskStatus::Failed,
                            None,
                            Some(format!("{detail}: {error:?}")),
                            &connection_id,
                        );
                    }
                }
                return;
            }
        }
        let spawn = Command::new(&self.ssh_bin)
            .args(&spawn_args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_remove("SSH_ASKPASS")
            .env_remove("SSH_ASKPASS_REQUIRE")
            .env_remove("DISPLAY")
            .spawn();
        drop(spawn_guard);
        drop(access_guard);

        let mut child = match spawn {
            Ok(child) => child,
            Err(error) => {
                self.finish_task(
                    task_id,
                    TaskStatus::Failed,
                    None,
                    Some(format!("failed to spawn ssh: {error}")),
                    &connection_id,
                );
                return;
            }
        };

        // Dedicated reader threads own the pipes and forward tagged chunks
        // through a bounded queue. The supervisor is the only consumer, so a
        // client that never calls read_task cannot create an unbounded pipe or
        // channel backlog.
        let (out_tx, out_rx) =
            std::sync::mpsc::sync_channel::<TaskOutputChunk>(TASK_OUTPUT_CHANNEL_CAPACITY);
        if let Some(pipe) = child.stdout.take() {
            spawn_task_output_reader(StreamKind::Stdout, pipe, out_tx.clone());
        }
        if let Some(pipe) = child.stderr.take() {
            spawn_task_output_reader(StreamKind::Stderr, pipe, out_tx.clone());
        }
        drop(out_tx);

        let started = Instant::now();
        let mut status: TaskStatus;
        let mut detail: Option<String> = None;

        let exit_code = loop {
            if cancelled.load(Ordering::SeqCst) {
                let _ = child.kill();
                let _ = child.wait();
                status = TaskStatus::Cancelled;
                detail = Some(CANCELLED_DETAIL.into());
                break None;
            }
            // Drain whatever the readers delivered.
            while let Ok(chunk) = out_rx.try_recv() {
                self.append_stream_output(task_id, chunk.stream, &chunk.bytes);
            }
            match child.try_wait() {
                Ok(Some(exit)) => {
                    // Final drain after exit so no tail output is lost; the
                    // channel closes once both reader threads finish.
                    while let Ok(chunk) = out_rx.recv() {
                        self.append_stream_output(task_id, chunk.stream, &chunk.bytes);
                    }
                    status = if exit.success() {
                        TaskStatus::Completed
                    } else {
                        TaskStatus::Failed
                    };
                    break exit.code();
                }
                Ok(None) => {
                    if started.elapsed() > self.task_timeout(task_id) {
                        let _ = child.kill();
                        let _ = child.wait();
                        status = TaskStatus::TimedOut;
                        detail = Some(TIMED_OUT_DETAIL.into());
                        break None;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(error) => {
                    status = TaskStatus::Failed;
                    detail = Some(format!("failed waiting on ssh: {error}"));
                    break None;
                }
            }
        };

        // A killed-by-mux-loss child surfaces as ssh exit 255 with a mux
        // error message; classify it so the client knows the result is
        // indeterminate rather than a normal command failure.
        if status == TaskStatus::Failed && exit_code == Some(255) {
            let mux_related = self
                .tasks
                .lock()
                .ok()
                .and_then(|tasks| {
                    tasks.get(task_id).map(|task| {
                        let has_mux_error = |out: &str| {
                            out.contains("mux_client")
                                || out.contains("Control socket")
                                || out.contains("control socket")
                                || out.contains("proxy command")
                                || out.contains("kex_exchange_identification")
                        };
                        has_mux_error(&task.stdout.data) || has_mux_error(&task.stderr.data)
                    })
                })
                .unwrap_or(false);
            if mux_related {
                status = TaskStatus::UnknownAfterDisconnect;
                detail = Some(
                    "the shared connection dropped mid-run; remote command state is unknown and was not replayed"
                        .into(),
                );
            }
        }

        self.finish_task(task_id, status, exit_code, detail, &connection_id);
    }

    fn task_timeout(&self, task_id: &str) -> Duration {
        self.get_task_by_id(task_id)
            .map(|snapshot| Duration::from_secs(snapshot.timeout_seconds))
            .unwrap_or(TASK_DEFAULT_TIMEOUT)
    }

    fn append_stream_output(&self, task_id: &str, stream: StreamKind, data: &[u8]) {
        if let Ok(mut tasks) = self.tasks.lock() {
            if let Some(task) = tasks.get_mut(task_id) {
                let text = String::from_utf8_lossy(data);
                let remaining = TASK_OUTPUT_MAX_BYTES
                    .saturating_sub(task.stdout.len().saturating_add(task.stderr.len()));
                let accepted = match stream {
                    StreamKind::Stdout => task.stdout.append(&text, remaining),
                    StreamKind::Stderr => task.stderr.append(&text, remaining),
                };
                task.snapshot.output_chars = task.snapshot.output_chars.saturating_add(accepted);
                if accepted < text.len()
                    || match stream {
                        StreamKind::Stdout => task.stdout.truncated,
                        StreamKind::Stderr => task.stderr.truncated,
                    }
                {
                    task.snapshot.output_truncated = true;
                }
            }
        }
    }

    fn finish_task(
        &self,
        task_id: &str,
        status: TaskStatus,
        exit_code: Option<i32>,
        detail: Option<String>,
        connection_id: &str,
    ) {
        let _lifecycle = self.lifecycle_lock.lock().ok();
        let snapshot = self.get_task_by_id(task_id);
        if let Some(snapshot) = snapshot.filter(|snapshot| !snapshot.status.is_terminal()) {
            let ended_at_ms = now_ms();
            let audit_result = self.record_transition(
                &snapshot,
                TaskAuditTransition {
                    status,
                    approved_at_ms: None,
                    started_at_ms: None,
                    ended_at_ms: Some(ended_at_ms),
                    exit_code,
                    detail: detail.as_deref(),
                    audit_error: None,
                },
            );
            if let Ok(mut tasks) = self.tasks.lock() {
                if let Some(task) = tasks.get_mut(task_id) {
                    if !task.snapshot.status.is_terminal() {
                        task.snapshot.status = status;
                        task.snapshot.exit_code = exit_code;
                        task.snapshot.ended_at_ms = Some(ended_at_ms);
                        task.snapshot.detail = detail;
                        task.snapshot.audit_recorded = audit_result.is_ok() && self.audit.is_some();
                        if let Err(error) = audit_result {
                            task.snapshot.audit_error = Some(error.clone());
                            task.snapshot.detail = Some(match task.snapshot.detail.take() {
                                Some(detail) => format!("{detail}; audit update failed: {error}"),
                                None => format!("audit update failed: {error}"),
                            });
                        }
                        task.child = None;
                    }
                }
            }
        }
        self.release_connection_slot(connection_id, task_id);
        self.cleanup_counter.fetch_add(1, Ordering::SeqCst);
    }

    fn release_connection_slot(&self, connection_id: &str, task_id: &str) {
        if let Ok(mut slots) = self.running_by_connection.lock() {
            if slots.get(connection_id).map(String::as_str) == Some(task_id) {
                slots.remove(connection_id);
            }
        }
    }

    /// Client-facing cancel. Distinct from "confirmed ended": a cancel on a
    /// running task asks us to kill the ssh child; the task only moves to
    /// Cancelled once the child is confirmed dead in run_task.
    pub fn cancel_for_client(
        &self,
        client_id: &str,
        task_id: &str,
    ) -> Result<Option<TaskSnapshot>, TaskTransitionError> {
        let lifecycle = self
            .lifecycle_lock
            .lock()
            .map_err(|_| TaskTransitionError::AuditUnavailable)?;
        let Some(snapshot) = self.get_task_by_id(task_id) else {
            return Ok(None);
        };
        if snapshot.client_id != client_id {
            return Ok(None);
        }
        let mut to_signal = None;
        let mut audit_error = None;
        let result = match snapshot.status {
            TaskStatus::PendingApproval | TaskStatus::Queued => {
                let ended_at_ms = now_ms();
                if let Err(error) = self.record_transition(
                    &snapshot,
                    TaskAuditTransition {
                        status: TaskStatus::Cancelled,
                        approved_at_ms: None,
                        started_at_ms: None,
                        ended_at_ms: Some(ended_at_ms),
                        exit_code: None,
                        detail: snapshot.detail.as_deref(),
                        audit_error: None,
                    },
                ) {
                    self.mark_audit_failure(task_id, &error, "cancellation");
                    return Err(TaskTransitionError::AuditUnavailable);
                }
                if let Ok(mut tasks) = self.tasks.lock() {
                    if let Some(task) = tasks.get_mut(task_id) {
                        if matches!(
                            task.snapshot.status,
                            TaskStatus::PendingApproval | TaskStatus::Queued
                        ) {
                            task.snapshot.status = TaskStatus::Cancelled;
                            task.snapshot.ended_at_ms = Some(ended_at_ms);
                            task.snapshot.detail = Some(CANCELLED_DETAIL.to_string());
                            task.snapshot.audit_recorded = self.audit.is_some();
                        }
                    }
                }
                if snapshot.status == TaskStatus::Queued {
                    self.release_connection_slot(&snapshot.connection_id, task_id);
                }
                self.get_task_by_id(task_id)
            }
            TaskStatus::Dispatching | TaskStatus::Running | TaskStatus::CancelRequested => {
                if snapshot.status != TaskStatus::CancelRequested {
                    let transition = self.record_transition(
                        &snapshot,
                        TaskAuditTransition {
                            status: TaskStatus::CancelRequested,
                            approved_at_ms: None,
                            started_at_ms: None,
                            ended_at_ms: None,
                            exit_code: None,
                            detail: Some(CANCEL_REQUESTED_DETAIL),
                            audit_error: None,
                        },
                    );
                    audit_error = transition.err();
                }
                let mut tasks = self
                    .tasks
                    .lock()
                    .map_err(|_| TaskTransitionError::AuditUnavailable)?;
                let Some(task) = tasks.get_mut(task_id) else {
                    return Ok(None);
                };
                if !matches!(
                    task.snapshot.status,
                    TaskStatus::Dispatching | TaskStatus::Running | TaskStatus::CancelRequested
                ) {
                    return Err(TaskTransitionError::NotCancellable);
                }
                task.snapshot.status = TaskStatus::CancelRequested;
                task.snapshot.detail = Some(CANCEL_REQUESTED_DETAIL.to_string());
                task.snapshot.audit_recorded = audit_error.is_none() && self.audit.is_some();
                if let Some(error) = &audit_error {
                    task.snapshot.audit_error = Some(error.clone());
                }
                to_signal = Some((task.cancelled.clone(), task.spawn_gate.clone()));
                Some(task.to_snapshot())
            }
            _ => return Err(TaskTransitionError::NotCancellable),
        };

        drop(lifecycle);
        if let Some((flag, spawn_gate)) = to_signal {
            let _spawn_guard = spawn_gate.lock().ok();
            flag.store(true, Ordering::SeqCst);
        }
        if audit_error.is_some() {
            return Err(TaskTransitionError::AuditUnavailable);
        }
        Ok(result)
    }

    /// Revocation path: cancel every non-terminal task for a connection
    /// (generation change, close, or explicit revoke). Running tasks get the
    /// cancel flag; pending approvals are cancelled outright so a stale
    /// approval can never execute later.
    pub fn cancel_connection_tasks(&self, connection_id: &str) {
        self.cancel_matching(|snapshot| snapshot.connection_id == connection_id);
    }

    /// Cancel tasks authorized under one client's grant for one connection.
    pub fn cancel_client_connection_tasks(&self, client_id: &str, connection_id: &str) {
        self.cancel_matching(|snapshot| {
            snapshot.client_id == client_id && snapshot.connection_id == connection_id
        });
    }

    /// Cancel every non-terminal task when MCP access is disabled. Running
    /// commands receive a best-effort kill signal and settle asynchronously.
    pub fn cancel_all_tasks(&self) {
        self.cancel_matching(|_| true);
    }

    /// End every non-terminal task owned by a client when it is unpaired.
    pub fn cancel_client_tasks(&self, client_id: &str) {
        self.cancel_matching(|snapshot| snapshot.client_id == client_id);
    }

    fn cancel_matching(&self, matches: impl Fn(&TaskSnapshot) -> bool) {
        let Ok(lifecycle) = self.lifecycle_lock.lock() else {
            return;
        };
        let mut to_cancel = Vec::new();
        let mut to_request = Vec::new();
        let mut to_release = Vec::new();
        if let Ok(tasks) = self.tasks.lock() {
            for task in tasks.values() {
                if !matches(&task.snapshot) {
                    continue;
                }
                match task.snapshot.status {
                    TaskStatus::PendingApproval | TaskStatus::Queued => {
                        if task.snapshot.status == TaskStatus::Queued {
                            to_release.push((
                                task.snapshot.connection_id.clone(),
                                task.snapshot.task_id.clone(),
                            ));
                        }
                        to_cancel.push(task.snapshot.clone());
                    }
                    TaskStatus::Dispatching | TaskStatus::Running | TaskStatus::CancelRequested => {
                        to_request.push((
                            task.snapshot.clone(),
                            task.cancelled.clone(),
                            task.spawn_gate.clone(),
                        ))
                    }
                    _ => {}
                }
            }
        }
        for snapshot in to_cancel {
            let ended_at_ms = now_ms();
            let result = self.record_transition(
                &snapshot,
                TaskAuditTransition {
                    status: TaskStatus::Cancelled,
                    approved_at_ms: None,
                    started_at_ms: None,
                    ended_at_ms: Some(ended_at_ms),
                    exit_code: None,
                    detail: Some(CANCELLED_DETAIL),
                    audit_error: None,
                },
            );
            if let Ok(mut tasks) = self.tasks.lock() {
                if let Some(task) = tasks.get_mut(&snapshot.task_id) {
                    if matches!(
                        task.snapshot.status,
                        TaskStatus::PendingApproval | TaskStatus::Queued
                    ) {
                        task.snapshot.status = TaskStatus::Cancelled;
                        task.snapshot.ended_at_ms = Some(ended_at_ms);
                        task.snapshot.detail = Some(CANCELLED_DETAIL.to_string());
                        task.snapshot.audit_recorded = result.is_ok() && self.audit.is_some();
                        if let Err(error) = &result {
                            task.snapshot.audit_error = Some(error.clone());
                        }
                    }
                }
            }
        }
        let mut to_signal = Vec::new();
        for (snapshot, flag, spawn_gate) in to_request {
            let result = if snapshot.status == TaskStatus::CancelRequested {
                Ok(())
            } else {
                self.record_transition(
                    &snapshot,
                    TaskAuditTransition {
                        status: TaskStatus::CancelRequested,
                        approved_at_ms: None,
                        started_at_ms: None,
                        ended_at_ms: None,
                        exit_code: None,
                        detail: Some(CANCEL_REQUESTED_DETAIL),
                        audit_error: None,
                    },
                )
            };
            if let Ok(mut tasks) = self.tasks.lock() {
                if let Some(task) = tasks.get_mut(&snapshot.task_id) {
                    if matches!(
                        task.snapshot.status,
                        TaskStatus::Dispatching | TaskStatus::Running | TaskStatus::CancelRequested
                    ) {
                        task.snapshot.status = TaskStatus::CancelRequested;
                        task.snapshot.detail = Some(CANCEL_REQUESTED_DETAIL.to_string());
                        task.snapshot.audit_recorded = result.is_ok() && self.audit.is_some();
                        if let Err(error) = &result {
                            task.snapshot.audit_error = Some(error.clone());
                        }
                    }
                }
            }
            to_signal.push((flag, spawn_gate));
        }
        drop(lifecycle);
        for (flag, spawn_gate) in to_signal {
            let _spawn_guard = spawn_gate.lock().ok();
            flag.store(true, Ordering::SeqCst);
        }
        for (connection_id, task_id) in to_release {
            self.release_connection_slot(&connection_id, &task_id);
        }
    }

    /// Request cancellation while the caller holds the service access gate.
    /// The durable/in-memory CancelRequested transition happens before the
    /// per-task spawn mutex is acquired and the flag is signalled.
    pub fn signal_cancel_connection_tasks(&self, connection_id: &str) {
        self.cancel_matching(|snapshot| snapshot.connection_id == connection_id);
    }

    pub fn signal_cancel_client_connection_tasks(&self, client_id: &str, connection_id: &str) {
        self.cancel_matching(|snapshot| {
            snapshot.client_id == client_id && snapshot.connection_id == connection_id
        });
    }

    pub fn signal_cancel_client_tasks(&self, client_id: &str) {
        self.cancel_matching(|snapshot| snapshot.client_id == client_id);
    }

    pub fn signal_cancel_all_tasks(&self) {
        self.cancel_matching(|_| true);
    }

    pub fn pending_approvals(&self) -> Result<Vec<TaskSnapshot>, TaskTransitionError> {
        self.expire_pending_approvals()?;
        self.pending_approvals_unchecked()
    }

    #[allow(dead_code)] // exposed to the trusted service surface for the later audit UI
    pub fn recent_audit(&self, limit: Option<usize>) -> Result<Vec<TaskAuditRecord>, String> {
        self.audit
            .as_ref()
            .ok_or_else(|| "persistent task audit is unavailable".to_string())?
            .recent(limit)
    }

    pub fn sweep_audit(&self) -> Result<usize, String> {
        self.audit
            .as_ref()
            .ok_or_else(|| "persistent task audit is unavailable".to_string())?
            .sweep()
    }

    /// Forget finished tasks older than TASK_RETENTION.
    pub fn sweep_finished(&self) {
        let cutoff = now_ms().saturating_sub(TASK_RETENTION.as_millis() as u64);
        if let Ok(mut tasks) = self.tasks.lock() {
            let expired: Vec<String> = tasks
                .values()
                .filter(|task| {
                    task.snapshot.status.is_terminal()
                        && task.snapshot.ended_at_ms.unwrap_or(0) < cutoff
                })
                .map(|task| task.snapshot.task_id.clone())
                .collect();
            for task_id in &expired {
                tasks.remove(task_id);
            }
            if !expired.is_empty() {
                if let Ok(mut index) = self.by_request.lock() {
                    index.retain(|_, id| !expired.contains(id));
                }
            }
        }
    }

    #[allow(dead_code)] // retained as phase B/C integration surface
    pub fn running_for_connection(&self, connection_id: &str) -> Option<String> {
        self.running_by_connection
            .lock()
            .ok()
            .and_then(|running| running.get(connection_id).cloned())
    }

    pub fn is_queued_and_reserved(&self, task_id: &str) -> bool {
        let Some(snapshot) = self.internal_snapshot(task_id) else {
            return false;
        };
        snapshot.status == TaskStatus::Queued
            && self
                .running_by_connection
                .lock()
                .ok()
                .and_then(|slots| slots.get(&snapshot.connection_id).cloned())
                .as_deref()
                == Some(task_id)
    }

    pub fn is_dispatching_and_reserved(&self, task_id: &str) -> bool {
        let Some(snapshot) = self.internal_snapshot(task_id) else {
            return false;
        };
        snapshot.status == TaskStatus::Dispatching
            && self
                .running_by_connection
                .lock()
                .ok()
                .and_then(|slots| slots.get(&snapshot.connection_id).cloned())
                .as_deref()
                == Some(task_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection_registry::HostSnapshot;
    use std::path::PathBuf;

    fn ready_record(generation: u64) -> ConnectionRecord {
        ConnectionRecord {
            connection_id: format!("conn-{generation}"),
            generation,
            pty_session_id: format!("sess-{generation}"),
            host: HostSnapshot {
                host_id: "h1".into(),
                alias: "prod".into(),
                hostname: "example.com".into(),
                user: "root".into(),
                port: 22,
                proxy_jump: None,
                has_identity_file: false,
                encoding: None,
            },
            state: ConnectionState::Ready,
            mux_socket: Some(PathBuf::from("/tmp/mux.sock")),
            created_at_ms: 0,
            ready_at_ms: Some(1),
            closed_at_ms: None,
            exit_code: None,
        }
    }

    fn test_grant(client_id: &str, record: &ConnectionRecord) -> ConnectionGrant {
        ConnectionGrant {
            grant_id: format!("grant-{}-{}", client_id, record.generation),
            client_id: client_id.to_string(),
            connection_id: record.connection_id.clone(),
            generation: record.generation,
            grant_start_seq: 0,
            observe: true,
            execute: true,
            created_at_ms: 0,
        }
    }

    fn submit_task(
        engine: &TaskEngine,
        client_id: &str,
        request_id: &str,
        record: &ConnectionRecord,
        command: &str,
    ) -> Result<TaskSnapshot, SubmitError> {
        let grant = test_grant(client_id, record);
        engine.submit(
            client_id,
            record,
            CommandSpec {
                request_id,
                command,
                working_directory: "login",
                timeout_seconds: TASK_DEFAULT_TIMEOUT_SECONDS,
            },
            &grant,
        )
    }

    fn start_sleeping_task(
        engine: &Arc<TaskEngine>,
        client_id: &str,
        request_id: &str,
        record: &ConnectionRecord,
    ) -> TaskSnapshot {
        let task = submit_task(engine, client_id, request_id, record, "sleep").unwrap();
        engine.approve(&task.task_id, record.generation).unwrap();
        assert!(engine
            .start_approved(&task.task_id, vec!["-c".into(), "sleep 10".into()])
            .unwrap());
        task
    }

    #[test]
    fn submit_is_idempotent_per_request_id() {
        let engine = TaskEngine::default();
        let record = ready_record(1);
        let first = submit_task(&engine, "c1", "req-1", &record, "uptime").unwrap();
        let second = submit_task(&engine, "c1", "req-1", &record, "uptime").unwrap();
        assert_eq!(first.task_id, second.task_id);
        // A different request id is a different task.
        let third = submit_task(&engine, "c1", "req-2", &record, "uptime").unwrap();
        assert_ne!(first.task_id, third.task_id);
    }

    #[test]
    fn submit_rejects_request_id_reuse_with_changed_request() {
        let record = ready_record(1);
        let engine = TaskEngine::default();
        submit_task(&engine, "c1", "req-1", &record, "uptime").unwrap();
        assert!(
            submit_task(&engine, "c1", "req-1", &record, "whoami").is_err(),
            "the same request_id must not silently reuse a task for another command"
        );

        let engine = TaskEngine::default();
        submit_task(&engine, "c1", "req-1", &record, "uptime").unwrap();
        let mut other_connection = record.clone();
        other_connection.connection_id = "conn-other".into();
        assert!(
            submit_task(&engine, "c1", "req-1", &other_connection, "uptime").is_err(),
            "the same request_id must stay bound to its original connection"
        );

        let engine = TaskEngine::default();
        submit_task(&engine, "c1", "req-1", &record, "uptime").unwrap();
        let mut next_generation = record.clone();
        next_generation.generation += 1;
        assert!(
            submit_task(&engine, "c1", "req-1", &next_generation, "uptime").is_err(),
            "the same request_id must stay bound to its original connection generation"
        );

        let engine = TaskEngine::default();
        submit_task(&engine, "c1", "req-1", &record, "uptime").unwrap();
        let mut reissued_grant = test_grant("c1", &record);
        reissued_grant.grant_id = "grant-reissued".into();
        reissued_grant.grant_start_seq = 12;
        assert!(matches!(
            engine.submit(
                "c1",
                &record,
                CommandSpec {
                    request_id: "req-1",
                    command: "uptime",
                    working_directory: "login",
                    timeout_seconds: TASK_DEFAULT_TIMEOUT_SECONDS,
                },
                &reissued_grant,
            ),
            Err(SubmitError::RequestConflict)
        ));
    }

    #[test]
    fn request_id_retry_conflicts_when_timeout_changes() {
        let record = ready_record(1);
        let engine = TaskEngine::default();
        let grant = test_grant("client-a", &record);
        engine
            .submit(
                "client-a",
                &record,
                CommandSpec {
                    request_id: "request",
                    command: "id",
                    working_directory: "login",
                    timeout_seconds: 300,
                },
                &grant,
            )
            .unwrap();

        assert_eq!(
            engine
                .submit(
                    "client-a",
                    &record,
                    CommandSpec {
                        request_id: "request",
                        command: "id",
                        working_directory: "login",
                        timeout_seconds: 301,
                    },
                    &grant,
                )
                .unwrap_err(),
            SubmitError::RequestConflict
        );
    }

    #[test]
    fn submit_enforces_pending_approval_limits_per_client_and_globally() {
        let record = ready_record(1);
        let per_client_engine = TaskEngine::default();
        for index in 0..8 {
            submit_task(
                &per_client_engine,
                "client-a",
                &format!("client-request-{index}"),
                &record,
                "id",
            )
            .unwrap();
        }
        assert!(submit_task(
            &per_client_engine,
            "client-a",
            "client-request-over-limit",
            &record,
            "id",
        )
        .is_err());
        assert_eq!(per_client_engine.pending_approvals().unwrap().len(), 8);

        let global_engine = TaskEngine::default();
        for client_index in 0..4 {
            for request_index in 0..8 {
                submit_task(
                    &global_engine,
                    &format!("client-{client_index}"),
                    &format!("request-{request_index}"),
                    &record,
                    "id",
                )
                .unwrap();
            }
        }
        assert!(submit_task(&global_engine, "client-extra", "over-limit", &record, "id").is_err());
        assert_eq!(global_engine.pending_approvals().unwrap().len(), 32);
    }

    #[test]
    fn submit_caps_total_retained_tasks_by_sweeping_terminal_tasks_first() {
        let engine = TaskEngine::default();
        let record = ready_record(1);
        for index in 0..257 {
            let task = submit_task(
                &engine,
                "client-a",
                &format!("request-{index}"),
                &record,
                "id",
            )
            .unwrap();
            engine.reject(&task.task_id).unwrap();
        }
        assert!(engine.tasks.lock().unwrap().len() <= 256);
    }

    #[test]
    fn submit_rejects_when_256_nonterminal_tasks_are_retained() {
        let engine = TaskEngine::default();
        for index in 0..TASK_MAX_RETAINED {
            let record = ready_record(index as u64 + 1);
            let task = submit_task(
                &engine,
                "client-a",
                &format!("active-{index}"),
                &record,
                "id",
            )
            .unwrap();
            engine.approve(&task.task_id, record.generation).unwrap();
        }

        let overflow_record = ready_record(TASK_MAX_RETAINED as u64 + 1);
        assert_eq!(
            submit_task(
                &engine,
                "client-a",
                "active-over-limit",
                &overflow_record,
                "id"
            )
            .unwrap_err(),
            SubmitError::RetainedTaskLimit
        );
        assert_eq!(engine.tasks.lock().unwrap().len(), TASK_MAX_RETAINED);
    }

    #[test]
    fn expired_pending_approval_is_rejected_and_audited_when_listed() {
        let db_path =
            std::env::temp_dir().join(format!("xt-task-expire-{}.db", uuid::Uuid::new_v4()));
        let engine = TaskEngine::with_audit(Arc::new(
            TaskAuditStore::open(&db_path).expect("audit store should open"),
        ));
        let record = ready_record(1);
        let task = submit_task(&engine, "client-a", "expires", &record, "id").unwrap();
        {
            let mut tasks = engine.tasks.lock().unwrap();
            tasks.get_mut(&task.task_id).unwrap().snapshot.created_at_ms =
                now_ms().saturating_sub(5 * 60 * 1000 + 1);
        }

        assert!(engine.pending_approvals().unwrap().is_empty());
        let expired = engine.internal_snapshot(&task.task_id).unwrap();
        assert_eq!(expired.status, TaskStatus::Rejected);
        assert_eq!(
            expired.detail.as_deref(),
            Some("approval expired after 5 minutes")
        );
        let (status, detail): (String, Option<String>) = rusqlite::Connection::open(&db_path)
            .unwrap()
            .query_row(
                "SELECT status, detail FROM mcp_task_ledger WHERE task_id = ?1",
                [&task.task_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "rejected");
        assert_eq!(detail.as_deref(), Some("approval expired after 5 minutes"));
        drop(engine);
        std::fs::remove_file(db_path).unwrap();
    }

    #[test]
    fn submit_and_approve_expire_pending_approvals_before_proceeding() {
        let engine = TaskEngine::default();
        let record = ready_record(1);
        let expired_by_submit =
            submit_task(&engine, "client-a", "submit-expiry", &record, "id").unwrap();
        {
            let mut tasks = engine.tasks.lock().unwrap();
            tasks
                .get_mut(&expired_by_submit.task_id)
                .unwrap()
                .snapshot
                .created_at_ms = now_ms().saturating_sub(5 * 60 * 1000 + 1);
        }
        let fresh = submit_task(&engine, "client-a", "fresh", &record, "id").unwrap();
        assert_eq!(
            engine
                .internal_snapshot(&expired_by_submit.task_id)
                .unwrap()
                .status,
            TaskStatus::Rejected
        );
        assert_eq!(fresh.status, TaskStatus::PendingApproval);

        let expired_by_approve =
            submit_task(&engine, "client-a", "approve-expiry", &record, "id").unwrap();
        {
            let mut tasks = engine.tasks.lock().unwrap();
            tasks
                .get_mut(&expired_by_approve.task_id)
                .unwrap()
                .snapshot
                .created_at_ms = now_ms().saturating_sub(5 * 60 * 1000 + 1);
        }
        assert_eq!(
            engine
                .approve(&expired_by_approve.task_id, record.generation)
                .unwrap_err(),
            ApprovalError::Expired
        );
        assert_eq!(
            engine
                .internal_snapshot(&expired_by_approve.task_id)
                .unwrap()
                .status,
            TaskStatus::Rejected
        );
    }

    #[test]
    fn submit_requires_ready_managed_connection() {
        let engine = TaskEngine::default();
        let mut record = ready_record(1);
        record.state = ConnectionState::Connecting;
        assert_eq!(
            submit_task(&engine, "c1", "r1", &record, "x").unwrap_err(),
            SubmitError::NotReady
        );
        record.state = ConnectionState::Exited;
        assert_eq!(
            submit_task(&engine, "c1", "r2", &record, "x").unwrap_err(),
            SubmitError::ConnectionUnavailable
        );
        record.state = ConnectionState::Ready;
        record.mux_socket = None;
        assert_eq!(
            submit_task(&engine, "c1", "r3", &record, "x").unwrap_err(),
            SubmitError::ConnectionUnavailable
        );
    }

    #[test]
    fn approval_audit_failure_leaves_task_pending_and_unapproved() {
        let db_path =
            std::env::temp_dir().join(format!("xt-task-audit-approve-{}.db", uuid::Uuid::new_v4()));
        let engine = TaskEngine::with_audit(Arc::new(
            TaskAuditStore::open(&db_path).expect("audit store should open"),
        ));
        let record = ready_record(1);
        let task = submit_task(&engine, "c1", "audit-approve", &record, "id").unwrap();
        let db = rusqlite::Connection::open(&db_path).unwrap();
        db.execute_batch(
            "CREATE TRIGGER reject_approval_audit
             BEFORE UPDATE ON mcp_task_ledger
             WHEN NEW.status = 'queued'
             BEGIN SELECT RAISE(ABORT, 'approval audit blocked'); END;",
        )
        .unwrap();

        assert_eq!(
            engine
                .approve(&task.task_id, record.generation)
                .unwrap_err(),
            ApprovalError::AuditUnavailable
        );
        let snapshot = engine.internal_snapshot(&task.task_id).unwrap();
        assert_eq!(snapshot.status, TaskStatus::PendingApproval);
        assert!(!snapshot.audit_recorded);
        assert!(snapshot.audit_error.is_some());
        let durable_status: String = db
            .query_row(
                "SELECT status FROM mcp_task_ledger WHERE task_id = ?1",
                [&task.task_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(durable_status, "pending_approval");
        drop(db);
        drop(engine);
        std::fs::remove_file(db_path).unwrap();
    }

    #[test]
    fn submission_audit_failure_does_not_insert_an_in_memory_or_durable_task() {
        let db_path =
            std::env::temp_dir().join(format!("xt-task-audit-submit-{}.db", uuid::Uuid::new_v4()));
        let engine = TaskEngine::with_audit(Arc::new(
            TaskAuditStore::open(&db_path).expect("audit store should open"),
        ));
        let db = rusqlite::Connection::open(&db_path).unwrap();
        db.execute_batch(
            "CREATE TRIGGER reject_submission_audit
             BEFORE INSERT ON mcp_task_ledger
             BEGIN SELECT RAISE(ABORT, 'submission audit blocked'); END;",
        )
        .unwrap();
        let record = ready_record(1);

        assert_eq!(
            submit_task(&engine, "c1", "audit-submit", &record, "id").unwrap_err(),
            SubmitError::AuditUnavailable
        );
        assert!(engine.pending_approvals().unwrap().is_empty());
        let durable_count: i64 = db
            .query_row("SELECT COUNT(*) FROM mcp_task_ledger", [], |row| row.get(0))
            .unwrap();
        assert_eq!(durable_count, 0);
        drop(db);
        drop(engine);
        std::fs::remove_file(db_path).unwrap();
    }

    #[test]
    fn dispatch_audit_failure_leaves_task_queued_without_a_running_slot() {
        let db_path = std::env::temp_dir().join(format!(
            "xt-task-audit-dispatch-{}.db",
            uuid::Uuid::new_v4()
        ));
        let mut engine = TaskEngine::with_audit(Arc::new(
            TaskAuditStore::open(&db_path).expect("audit store should open"),
        ));
        engine.ssh_bin = "/bin/sh".into();
        let engine = Arc::new(engine);
        let record = ready_record(1);
        let task = submit_task(&engine, "c1", "audit-dispatch", &record, "id").unwrap();
        engine.approve(&task.task_id, record.generation).unwrap();
        let db = rusqlite::Connection::open(&db_path).unwrap();
        db.execute_batch(
            "CREATE TRIGGER reject_dispatch_audit
             BEFORE UPDATE ON mcp_task_ledger
             WHEN NEW.status = 'dispatching'
             BEGIN SELECT RAISE(ABORT, 'dispatch audit blocked'); END;",
        )
        .unwrap();

        assert_eq!(
            engine
                .start_approved(&task.task_id, vec!["-c".into(), "exit 0".into()])
                .unwrap_err(),
            TaskTransitionError::AuditUnavailable
        );
        let snapshot = engine.internal_snapshot(&task.task_id).unwrap();
        assert_eq!(snapshot.status, TaskStatus::Queued);
        assert!(!snapshot.audit_recorded);
        assert!(snapshot.audit_error.is_some());
        assert_eq!(
            engine
                .running_for_connection(&record.connection_id)
                .as_deref(),
            Some(task.task_id.as_str())
        );
        let durable_status: String = db
            .query_row(
                "SELECT status FROM mcp_task_ledger WHERE task_id = ?1",
                [&task.task_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(durable_status, "queued");
        drop(db);
        drop(engine);
        std::fs::remove_file(db_path).unwrap();
    }

    #[test]
    fn terminal_audit_failure_marks_snapshot_and_keeps_output_out_of_database() {
        let db_path =
            std::env::temp_dir().join(format!("xt-task-audit-finish-{}.db", uuid::Uuid::new_v4()));
        let mut engine = TaskEngine::with_audit(Arc::new(
            TaskAuditStore::open(&db_path).expect("audit store should open"),
        ));
        engine.ssh_bin = "/bin/sh".into();
        let engine = Arc::new(engine);
        let record = ready_record(1);
        let task = submit_task(&engine, "c1", "audit-finish", &record, "print-marker").unwrap();
        engine.approve(&task.task_id, record.generation).unwrap();
        let db = rusqlite::Connection::open(&db_path).unwrap();
        db.execute_batch(
            "CREATE TRIGGER reject_finish_audit
             BEFORE UPDATE ON mcp_task_ledger
             WHEN NEW.status = 'completed'
             BEGIN SELECT RAISE(ABORT, 'finish audit blocked'); END;",
        )
        .unwrap();
        assert!(engine
            .start_approved(
                &task.task_id,
                vec!["-c".into(), "printf output-marker".into()],
            )
            .unwrap());

        let deadline = Instant::now() + Duration::from_secs(2);
        let snapshot = loop {
            let snapshot = engine.internal_snapshot(&task.task_id).unwrap();
            if snapshot.status.is_terminal() {
                break snapshot;
            }
            assert!(
                Instant::now() < deadline,
                "task did not finish before timeout"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(snapshot.status, TaskStatus::Completed);
        assert!(!snapshot.audit_recorded);
        assert!(snapshot.audit_error.is_some());
        let columns = {
            let mut statement = db.prepare("PRAGMA table_info(mcp_task_ledger)").unwrap();
            statement
                .query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        assert!(!columns
            .iter()
            .any(|name| { matches!(name.as_str(), "output" | "stdout" | "stderr") }));
        let stored_text: String = db
            .query_row(
                "SELECT command || COALESCE(detail, '') || COALESCE(audit_error, '')
                 FROM mcp_task_ledger WHERE task_id = ?1",
                [&task.task_id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!stored_text.contains("output-marker"));
        drop(db);
        drop(engine);
        std::fs::remove_file(db_path).unwrap();
    }

    #[test]
    fn successful_task_lifecycle_is_written_as_ordered_audit_events() {
        let db_path =
            std::env::temp_dir().join(format!("xt-task-audit-events-{}.db", uuid::Uuid::new_v4()));
        let mut engine = TaskEngine::with_audit(Arc::new(
            TaskAuditStore::open(&db_path).expect("audit store should open"),
        ));
        engine.ssh_bin = "/bin/sh".into();
        let engine = Arc::new(engine);
        let record = ready_record(1);
        let task = submit_task(&engine, "c1", "audit-events", &record, "id").unwrap();
        assert!(task.audit_recorded);
        let approved = engine.approve(&task.task_id, record.generation).unwrap();
        assert_eq!(approved.status, TaskStatus::Queued);
        assert!(approved.approved_at_ms.is_some());
        assert!(engine
            .start_approved(&task.task_id, vec!["-c".into(), "exit 0".into()])
            .unwrap());

        let deadline = Instant::now() + Duration::from_secs(2);
        let completed = loop {
            let snapshot = engine.internal_snapshot(&task.task_id).unwrap();
            if snapshot.status == TaskStatus::Completed {
                break snapshot;
            }
            assert!(
                Instant::now() < deadline,
                "task did not finish before timeout"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(completed.audit_recorded);
        assert!(completed.started_at_ms.is_some());
        assert!(completed.ended_at_ms.is_some());

        let recent = engine.recent_audit(None).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].task_id, task.task_id);
        assert_eq!(recent[0].status, TaskStatus::Completed);
        assert_eq!(recent[0].working_directory, "login");
        assert_eq!(recent[0].timeout_seconds, TASK_DEFAULT_TIMEOUT_SECONDS);
        assert!(recent[0].approved_at_ms.is_some());
        assert!(recent[0].started_at_ms.is_some());
        assert!(recent[0].ended_at_ms.is_some());
        let event_statuses = {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            let mut stmt = conn
                .prepare("SELECT status FROM mcp_task_events WHERE task_id = ?1 ORDER BY event_id")
                .unwrap();
            stmt.query_map([&task.task_id], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        assert_eq!(
            event_statuses,
            vec![
                "pending_approval".to_string(),
                "queued".to_string(),
                "dispatching".to_string(),
                "running".to_string(),
                "completed".to_string(),
            ]
        );
        drop(engine);
        std::fs::remove_file(db_path).unwrap();
    }

    #[test]
    fn approval_is_generation_checked_and_single_use() {
        let engine = TaskEngine::default();
        let record = ready_record(5);
        let task = submit_task(&engine, "c1", "r1", &record, "id").unwrap();
        assert_eq!(task.status, TaskStatus::PendingApproval);

        // Stale generation: the approval must not execute.
        assert_eq!(
            engine.approve(&task.task_id, 6).unwrap_err(),
            ApprovalError::StaleGeneration
        );
        let after = engine.internal_snapshot(&task.task_id).unwrap();
        assert_eq!(after.status, TaskStatus::Rejected);

        let task2 = submit_task(&engine, "c1", "r2", &record, "id").unwrap();
        let approved = engine.approve(&task2.task_id, 5).unwrap();
        assert_eq!(approved.status, TaskStatus::Queued);
        // Approving twice is rejected: queued is not pending.
        assert_eq!(
            engine.approve(&task2.task_id, 5).unwrap_err(),
            ApprovalError::NotApprovable
        );
    }

    #[test]
    fn approval_reserves_one_connection_slot_and_busy_task_stays_pending() {
        let engine = TaskEngine::default();
        let record = ready_record(1);
        let first = submit_task(&engine, "alice", "first", &record, "id").unwrap();
        let second = submit_task(&engine, "bob", "second", &record, "whoami").unwrap();

        assert_eq!(
            engine
                .approve(&first.task_id, record.generation)
                .unwrap()
                .status,
            TaskStatus::Queued
        );
        assert_eq!(
            engine
                .running_for_connection(&record.connection_id)
                .as_deref(),
            Some(first.task_id.as_str()),
            "approval must reserve the slot before mux verification"
        );
        assert_eq!(
            engine
                .approve(&second.task_id, record.generation)
                .unwrap_err(),
            ApprovalError::ConnectionBusy
        );
        assert_eq!(
            engine.internal_snapshot(&second.task_id).unwrap().status,
            TaskStatus::PendingApproval,
            "a busy approval remains available for a later retry"
        );
    }

    #[test]
    fn clients_cannot_see_or_cancel_each_others_tasks() {
        let engine = TaskEngine::default();
        let record = ready_record(1);
        let task = submit_task(&engine, "alice", "r1", &record, "id").unwrap();
        assert!(engine.get_task_for_client("bob", &task.task_id).is_none());
        assert!(engine
            .cancel_for_client("bob", &task.task_id)
            .unwrap()
            .is_none());
        let cancelled = engine
            .cancel_for_client("alice", &task.task_id)
            .unwrap()
            .unwrap();
        assert_eq!(cancelled.status, TaskStatus::Cancelled);
    }

    #[test]
    fn revoking_one_clients_connection_does_not_cancel_other_clients_tasks() {
        let engine = TaskEngine::default();
        let record = ready_record(1);
        let revoked = submit_task(&engine, "alice", "r1", &record, "id").unwrap();
        let other = submit_task(&engine, "bob", "r1", &record, "id").unwrap();

        engine.cancel_client_connection_tasks("alice", &record.connection_id);

        assert_eq!(
            engine.internal_snapshot(&revoked.task_id).unwrap().status,
            TaskStatus::Cancelled
        );
        assert_eq!(
            engine.internal_snapshot(&other.task_id).unwrap().status,
            TaskStatus::PendingApproval
        );
    }

    #[test]
    fn cancel_connection_tasks_kills_pending_and_signals_running() {
        let engine = TaskEngine::default();
        let record = ready_record(1);
        let pending = submit_task(&engine, "c1", "r1", &record, "a").unwrap();
        let queued = submit_task(&engine, "c1", "r2", &record, "b").unwrap();
        engine.approve(&queued.task_id, 1).unwrap();

        engine.cancel_connection_tasks(&record.connection_id);
        assert_eq!(
            engine.internal_snapshot(&pending.task_id).unwrap().status,
            TaskStatus::Cancelled
        );
        assert_eq!(
            engine.internal_snapshot(&queued.task_id).unwrap().status,
            TaskStatus::Cancelled
        );
        assert_eq!(engine.running_for_connection(&record.connection_id), None);
    }

    #[test]
    fn revoke_unpair_and_disable_signal_only_their_running_task_scopes() {
        let mut engine = TaskEngine::default();
        engine.ssh_bin = "/bin/sh".into();
        let engine = Arc::new(engine);
        let record_a = ready_record(1);
        let record_b = ready_record(2);
        let record_c = ready_record(3);
        let task_a = start_sleeping_task(&engine, "alice", "r1", &record_a);
        let task_b = start_sleeping_task(&engine, "bob", "r1", &record_b);
        let task_c = start_sleeping_task(&engine, "carol", "r1", &record_c);

        let cancelled = |task_id: &str| {
            engine
                .tasks
                .lock()
                .unwrap()
                .get(task_id)
                .unwrap()
                .cancelled
                .load(Ordering::SeqCst)
        };

        engine.cancel_client_connection_tasks("bob", &record_b.connection_id);
        assert!(cancelled(&task_b.task_id));
        assert!(!cancelled(&task_a.task_id));
        assert!(!cancelled(&task_c.task_id));

        engine.cancel_client_tasks("alice");
        assert!(cancelled(&task_a.task_id));
        assert!(!cancelled(&task_c.task_id));

        engine.cancel_all_tasks();
        assert!(cancelled(&task_c.task_id));

        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            let statuses = [
                task_a.task_id.as_str(),
                task_b.task_id.as_str(),
                task_c.task_id.as_str(),
            ]
            .map(|task_id| engine.internal_snapshot(task_id).unwrap().status);
            if statuses
                .iter()
                .all(|status| *status == TaskStatus::Cancelled)
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("running tasks did not settle after cancellation");
    }

    #[test]
    fn one_queued_or_running_task_per_connection() {
        let mut task_engine = TaskEngine::default();
        task_engine.ssh_bin = "/bin/sh".into();
        let engine = Arc::new(task_engine);
        let record = ready_record(1);
        let t1 = submit_task(&engine, "c1", "r1", &record, "sleep 0.2").unwrap();
        let t2 = submit_task(&engine, "c1", "r2", &record, "sleep 0.2").unwrap();
        engine.approve(&t1.task_id, 1).unwrap();
        assert_eq!(
            engine.approve(&t2.task_id, 1).unwrap_err(),
            ApprovalError::ConnectionBusy
        );
        assert_eq!(
            engine.internal_snapshot(&t2.task_id).unwrap().status,
            TaskStatus::PendingApproval
        );

        // Use /bin/sh via real spawn args — the engine itself only needs a
        // runnable argv, so point it at a trivial local command.
        let args_ok = vec!["-c".to_string(), "sleep 0.2; exit 0".to_string()];
        assert!(engine.start_approved(&t1.task_id, args_ok.clone()).unwrap());
        assert!(!engine.start_approved(&t2.task_id, args_ok).unwrap());
        assert_eq!(
            engine
                .running_for_connection(&record.connection_id)
                .as_deref(),
            Some(t1.task_id.as_str())
        );

        let deadline = Instant::now() + Duration::from_secs(2);
        while engine.internal_snapshot(&t1.task_id).unwrap().status == TaskStatus::Running {
            assert!(Instant::now() < deadline, "first task did not finish");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(engine.running_for_connection(&record.connection_id), None);
        assert_eq!(
            engine.approve(&t2.task_id, 1).unwrap().status,
            TaskStatus::Queued
        );
        assert!(engine
            .start_approved(&t2.task_id, vec!["-c".into(), "exit 0".into()])
            .unwrap());
    }

    #[test]
    fn revoke_before_worker_spawn_does_not_launch_ssh() {
        let mut task_engine = TaskEngine::default();
        task_engine.ssh_bin = "/path/that/does/not/exist".into();
        let engine = Arc::new(task_engine);
        let record = ready_record(1);
        let task = submit_task(&engine, "alice", "pre-spawn-revoke", &record, "id").unwrap();
        engine.approve(&task.task_id, record.generation).unwrap();
        let (cancelled, spawn_mutex) = {
            let mut tasks = engine.tasks.lock().unwrap();
            let current = tasks.get_mut(&task.task_id).unwrap();
            current.snapshot.status = TaskStatus::Running;
            (current.cancelled.clone(), current.spawn_gate.clone())
        };

        engine.cancel_client_connection_tasks("alice", &record.connection_id);
        assert!(cancelled.load(Ordering::SeqCst));
        engine.run_task(
            &task.task_id,
            vec!["-c".into(), "exit 0".into()],
            cancelled,
            record.connection_id.clone(),
            spawn_mutex,
            None,
        );

        assert_eq!(
            engine.internal_snapshot(&task.task_id).unwrap().status,
            TaskStatus::Cancelled,
            "a revoke before child creation must not become a spawn failure"
        );
        assert_eq!(engine.running_for_connection(&record.connection_id), None);
    }

    #[test]
    fn running_cancel_is_audited_as_requested_before_confirmed_cancelled() {
        let db_path =
            std::env::temp_dir().join(format!("xt-task-cancel-{}.db", uuid::Uuid::new_v4()));
        let mut task_engine = TaskEngine::with_audit(Arc::new(
            TaskAuditStore::open(&db_path).expect("audit store should open"),
        ));
        task_engine.ssh_bin = "/bin/sh".into();
        let engine = Arc::new(task_engine);
        let record = ready_record(1);
        let task = submit_task(&engine, "client-a", "cancel-requested", &record, "sleep").unwrap();
        engine.approve(&task.task_id, record.generation).unwrap();
        assert!(engine
            .start_approved(&task.task_id, vec!["-c".into(), "exec sleep 10".into()],)
            .unwrap());

        let requested = engine
            .cancel_for_client("client-a", &task.task_id)
            .unwrap()
            .unwrap();
        assert_eq!(requested.status, TaskStatus::CancelRequested);
        let deadline = Instant::now() + Duration::from_secs(2);
        let cancelled = loop {
            let current = engine.internal_snapshot(&task.task_id).unwrap();
            if current.status == TaskStatus::Cancelled {
                break current;
            }
            assert!(
                Instant::now() < deadline,
                "local SSH child did not terminate"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(cancelled.detail.as_deref().is_some_and(|detail| {
            detail.contains("local SSH channel ended")
                && detail.contains("remote descendants are not guaranteed")
        }));
        let statuses = {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            let mut statement = conn
                .prepare("SELECT status FROM mcp_task_events WHERE task_id = ?1 ORDER BY event_id")
                .unwrap();
            statement
                .query_map([&task.task_id], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        assert_eq!(
            statuses,
            vec![
                "pending_approval",
                "queued",
                "dispatching",
                "running",
                "cancel_requested",
                "cancelled"
            ]
        );
        drop(engine);
        std::fs::remove_file(db_path).unwrap();
    }

    #[test]
    fn output_is_bounded_and_reports_truncation() {
        let engine = TaskEngine::default();
        let record = ready_record(1);
        let task = submit_task(&engine, "c1", "r1", &record, "big").unwrap();
        let chunk = "x".repeat(4096);
        for _ in 0..(TASK_OUTPUT_MAX_BYTES / 4096 + 4) {
            engine.append_stream_output(&task.task_id, StreamKind::Stdout, chunk.as_bytes());
        }
        assert_eq!(
            engine.task_output_bytes(&task.task_id),
            Some(TASK_OUTPUT_MAX_BYTES)
        );
        let read = engine
            .read_task_output_for_client("c1", &task.task_id, Some(0), Some(0))
            .unwrap();
        assert_eq!(read.stdout_next_offset, TASK_READ_MAX_BYTES);
        assert_eq!(read.stdout.len(), TASK_READ_MAX_BYTES);
        assert!(read.stdout_truncated);
        // Other clients read nothing.
        assert!(engine
            .read_task_output_for_client("mallory", &task.task_id, Some(0), Some(0))
            .is_none());
    }

    #[test]
    fn task_output_red_group_one_keeps_streams_independent_and_offsets_safe() {
        let engine = TaskEngine::default();
        let record = ready_record(1);
        let task = submit_task(&engine, "c1", "streams", &record, "echo").unwrap();

        engine.append_stream_output(&task.task_id, StreamKind::Stdout, b"out");
        engine.append_stream_output(&task.task_id, StreamKind::Stdout, "\u{1f642}".as_bytes());
        engine.append_stream_output(&task.task_id, StreamKind::Stderr, b"err");

        let first = engine
            .read_task_output_for_client("c1", &task.task_id, Some(0), Some(0))
            .unwrap();
        assert_eq!(first.stdout, "out\u{1f642}");
        assert_eq!(first.stderr, "err");
        assert_eq!(first.stdout_next_offset, "out\u{1f642}".len());
        assert_eq!(first.stderr_next_offset, 3);
        assert!(!first.stdout_gap);
        assert!(!first.stderr_gap);

        // An offset in the middle of the four-byte emoji is adjusted forward
        // to a UTF-8 boundary and reports the skipped byte range as a gap.
        let mid_emoji = engine
            .read_task_output_for_client("c1", &task.task_id, Some("out".len() + 1), Some(0))
            .unwrap();
        assert!(mid_emoji.stdout_gap);
        assert!(mid_emoji.stdout.is_empty());
        assert_eq!(mid_emoji.stdout_next_offset, "out\u{1f642}".len());

        // Arbitrary external offsets, including usize::MAX, must never panic.
        let impossible = engine
            .read_task_output_for_client("c1", &task.task_id, Some(usize::MAX), Some(usize::MAX))
            .unwrap();
        assert!(impossible.stdout_gap);
        assert!(impossible.stderr_gap);
        assert!(impossible.stdout.is_empty());
        assert!(impossible.stderr.is_empty());
    }

    #[test]
    fn task_output_red_group_two_is_byte_bounded_and_read_budgeted_without_polling() {
        let engine = TaskEngine::default();
        let record = ready_record(1);
        let task = submit_task(&engine, "c1", "bounded", &record, "yes").unwrap();

        let stdout = "\u{1f642}".repeat(TASK_OUTPUT_MAX_BYTES);
        let stderr = "e".repeat(TASK_OUTPUT_MAX_BYTES);
        engine.append_stream_output(&task.task_id, StreamKind::Stdout, stdout.as_bytes());
        engine.append_stream_output(&task.task_id, StreamKind::Stderr, stderr.as_bytes());

        let retained = engine.tasks.lock().unwrap();
        let retained_task = retained.get(&task.task_id).unwrap();
        assert!(retained_task.stdout.len() + retained_task.stderr.len() <= TASK_OUTPUT_MAX_BYTES);
        drop(retained);

        let read = engine
            .read_task_output_for_client("c1", &task.task_id, Some(0), Some(0))
            .unwrap();
        assert!(read.stdout.len() + read.stderr.len() <= TASK_READ_MAX_BYTES);
        assert!(read.stdout.is_char_boundary(read.stdout.len()));
        assert!(read.stderr.is_char_boundary(read.stderr.len()));

        // A client that never reads cannot make the retained cache grow past
        // the hard two-stream byte budget.
        let before = engine.task_output_bytes(&task.task_id).unwrap();
        engine.append_stream_output(&task.task_id, StreamKind::Stdout, b"more");
        engine.append_stream_output(&task.task_id, StreamKind::Stderr, b"more");
        assert_eq!(engine.task_output_bytes(&task.task_id), Some(before));
        assert!(TASK_OUTPUT_CHANNEL_CAPACITY > 0);
    }

    #[test]
    fn task_output_red_group_one_reader_tags_stdout_and_stderr_separately() {
        let mut task_engine = TaskEngine::default();
        task_engine.ssh_bin = "/bin/sh".into();
        let engine = Arc::new(task_engine);
        let record = ready_record(1);
        let task = submit_task(&engine, "c1", "reader-streams", &record, "printf").unwrap();
        engine.approve(&task.task_id, record.generation).unwrap();
        assert!(engine
            .start_approved(
                &task.task_id,
                vec!["-c".into(), "printf out; printf err >&2".into()],
            )
            .unwrap());

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let snapshot = engine.internal_snapshot(&task.task_id).unwrap();
            if snapshot.status.is_terminal() {
                break;
            }
            assert!(Instant::now() < deadline, "task reader did not finish");
            std::thread::sleep(Duration::from_millis(10));
        }

        let read = engine
            .read_task_output_for_client("c1", &task.task_id, Some(0), Some(0))
            .unwrap();
        assert_eq!(read.stdout, "out");
        assert_eq!(read.stderr, "err");
        assert!(!read.stdout_gap);
        assert!(!read.stderr_gap);
    }
}
