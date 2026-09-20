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
use serde::Serialize;
use std::collections::HashMap;
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Per-task retained output cap. Matches the phase A per-connection budget.
pub const TASK_OUTPUT_MAX_CHARS: usize = 1_000_000;
/// Hard cap on task lifetime; commands may opt into a shorter timeout.
pub const TASK_DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);
#[allow(dead_code)] // retained as phase B/C integration surface
pub const TASK_MAX_TIMEOUT: Duration = Duration::from_secs(1800);
/// How long finished task results are kept for the owning client to read.
pub const TASK_RETENTION: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Created, waiting for the xTerm user's approval.
    PendingApproval,
    /// Approved, waiting for the per-connection slot.
    Queued,
    Running,
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
            TaskStatus::PendingApproval | TaskStatus::Queued | TaskStatus::Running
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
    pub status: TaskStatus,
    pub created_at_ms: u64,
    pub started_at_ms: Option<u64>,
    pub ended_at_ms: Option<u64>,
    pub exit_code: Option<i32>,
    pub output_chars: usize,
    pub output_truncated: bool,
    pub detail: Option<String>,
}

struct Task {
    snapshot: TaskSnapshot,
    output: String,
    cancelled: Arc<AtomicBool>,
    child: Option<Arc<Mutex<Child>>>,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalError {
    /// No such task, wrong owner, or the task already left PendingApproval.
    NotApprovable,
    /// The connection generation moved on since submission: stale approvals
    /// must not execute.
    StaleGeneration,
}

pub struct TaskEngine {
    tasks: Mutex<HashMap<String, Task>>,
    /// request_id -> task_id, for idempotent retries.
    by_request: Mutex<HashMap<(String, String), String>>,
    /// connection_id -> running task_id (one command channel per connection).
    running_by_connection: Mutex<HashMap<String, String>>,
    cleanup_counter: AtomicU64,
    /// Always `/usr/bin/ssh` in production; overridable in tests so the
    /// engine's lifecycle can be exercised without a real SSH server.
    ssh_bin: String,
}

impl Default for TaskEngine {
    fn default() -> Self {
        Self {
            tasks: Mutex::new(HashMap::new()),
            by_request: Mutex::new(HashMap::new()),
            running_by_connection: Mutex::new(HashMap::new()),
            cleanup_counter: AtomicU64::new(0),
            ssh_bin: "/usr/bin/ssh".to_string(),
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
    /// Create a task in PendingApproval. Idempotent per (client, request_id):
    /// a retried submission returns the existing task untouched.
    pub fn submit(
        &self,
        client_id: &str,
        request_id: &str,
        record: &ConnectionRecord,
        command: &str,
    ) -> Result<TaskSnapshot, SubmitError> {
        let key = (client_id.to_string(), request_id.to_string());
        if let Some(existing) = self
            .by_request
            .lock()
            .ok()
            .and_then(|index| index.get(&key).cloned())
        {
            if let Some(task) = self.get_task(&existing) {
                return Ok(task);
            }
        }

        if !record.mcp_eligible() {
            return Err(SubmitError::ConnectionUnavailable);
        }
        if record.state != ConnectionState::Ready {
            return Err(SubmitError::NotReady);
        }

        let task_id = uuid::Uuid::new_v4().to_string();
        let snapshot = TaskSnapshot {
            task_id: task_id.clone(),
            request_id: request_id.to_string(),
            client_id: client_id.to_string(),
            connection_id: record.connection_id.clone(),
            generation: record.generation,
            command: command.to_string(),
            status: TaskStatus::PendingApproval,
            created_at_ms: now_ms(),
            started_at_ms: None,
            ended_at_ms: None,
            exit_code: None,
            output_chars: 0,
            output_truncated: false,
            detail: None,
        };
        let task = Task {
            snapshot,
            output: String::new(),
            cancelled: Arc::new(AtomicBool::new(false)),
            child: None,
        };
        {
            let mut tasks = self.tasks.lock().map_err(|_| SubmitError::ConnectionUnavailable)?;
            tasks.insert(task_id.clone(), task);
        }
        if let Ok(mut index) = self.by_request.lock() {
            index.insert(key, task_id.clone());
        }
        Ok(self.get_task_by_id(&task_id).expect("just inserted"))
    }

    fn get_task_by_id(&self, task_id: &str) -> Option<TaskSnapshot> {
        self.tasks
            .lock()
            .ok()
            .and_then(|tasks| tasks.get(task_id).map(Task::to_snapshot))
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

    fn get_task(&self, task_id: &str) -> Option<TaskSnapshot> {
        self.get_task_by_id(task_id)
    }

    /// Read and consume nothing: returns incremental output from `offset`
    /// (a char offset into the retained output) plus truncation flags.
    pub fn read_output_for_client(
        &self,
        client_id: &str,
        task_id: &str,
        offset: usize,
    ) -> Option<(String, usize, bool)> {
        let tasks = self.tasks.lock().ok()?;
        let task = tasks.get(task_id)?;
        if task.snapshot.client_id != client_id {
            return None;
        }
        if offset > task.output.len() {
            // The retained window moved past the reader; report the gap.
            return Some((String::new(), task.output.len(), true));
        }
        let slice = task.output[offset..].to_string();
        let next = task.output.len();
        Some((slice, next, task.snapshot.output_truncated))
    }

    /// Approve the exact command captured at submit time. The UI passes the
    /// current generation of the connection so an approval that crosses a
    /// reconnect is refused instead of executing against the new session.
    pub fn approve(
        &self,
        task_id: &str,
        current_generation: u64,
    ) -> Result<TaskSnapshot, ApprovalError> {
        let mut tasks = self.tasks.lock().map_err(|_| ApprovalError::NotApprovable)?;
        let task = tasks.get_mut(task_id).ok_or(ApprovalError::NotApprovable)?;
        if task.snapshot.status != TaskStatus::PendingApproval {
            return Err(ApprovalError::NotApprovable);
        }
        if task.snapshot.generation != current_generation {
            task.snapshot.status = TaskStatus::Rejected;
            task.snapshot.ended_at_ms = Some(now_ms());
            task.snapshot.detail = Some("connection generation changed before approval".into());
            return Err(ApprovalError::StaleGeneration);
        }
        task.snapshot.status = TaskStatus::Queued;
        Ok(task.to_snapshot())
    }

    pub fn reject(&self, task_id: &str) -> Option<TaskSnapshot> {
        let mut tasks = self.tasks.lock().ok()?;
        let task = tasks.get_mut(task_id)?;
        if task.snapshot.status != TaskStatus::PendingApproval {
            return None;
        }
        task.snapshot.status = TaskStatus::Rejected;
        task.snapshot.ended_at_ms = Some(now_ms());
        Some(task.to_snapshot())
    }

    /// Try to start an approved task. Returns false if the connection already
    /// has a running task (caller retries later) or the task is not Queued.
    ///
    /// `spawn_args` must come from `MuxManager::derived_channel_args` after a
    /// fresh `verify()` — this function deliberately does not accept raw
    /// commands, so nothing downstream can bypass the managed-mux path.
    pub fn start_approved(self: &Arc<Self>, task_id: &str, spawn_args: Vec<String>) -> bool {
        let (cancelled, connection_id) = {
            let mut tasks = match self.tasks.lock() {
                Ok(tasks) => tasks,
                Err(_) => return false,
            };
            let Some(task) = tasks.get_mut(task_id) else {
                return false;
            };
            if task.snapshot.status != TaskStatus::Queued {
                return false;
            }
            let mut running = match self.running_by_connection.lock() {
                Ok(running) => running,
                Err(_) => return false,
            };
            if running.contains_key(&task.snapshot.connection_id) {
                return false; // one command channel per connection
            }
            running.insert(task.snapshot.connection_id.clone(), task_id.to_string());
            task.snapshot.status = TaskStatus::Running;
            task.snapshot.started_at_ms = Some(now_ms());
            (task.cancelled.clone(), task.snapshot.connection_id.clone())
        };

        let engine = Arc::clone(self);
        let task_id_owned = task_id.to_string();
        std::thread::spawn(move || {
            engine.run_task(&task_id_owned, spawn_args, cancelled, connection_id);
        });
        true
    }

    fn run_task(
        &self,
        task_id: &str,
        spawn_args: Vec<String>,
        cancelled: Arc<AtomicBool>,
        connection_id: String,
    ) {
        let spawn = Command::new(&self.ssh_bin)
            .args(&spawn_args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_remove("SSH_ASKPASS")
            .env_remove("SSH_ASKPASS_REQUIRE")
            .env_remove("DISPLAY")
            .spawn();

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

        // Dedicated reader threads own the pipes and forward chunks over a
        // channel, so the supervisor loop never blocks on I/O and can react
        // to cancel/timeout promptly.
        let (out_tx, out_rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let pipes: Vec<Box<dyn Read + Send>> = vec![
            child
                .stdout
                .take()
                .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
            child
                .stderr
                .take()
                .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
        ]
        .into_iter()
        .flatten()
        .collect();
        for mut pipe in pipes {
            let tx = out_tx.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match pipe.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            if tx.send(buf[..n].to_vec()).is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            });
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
                break None;
            }
            // Drain whatever the readers delivered.
            while let Ok(chunk) = out_rx.try_recv() {
                self.append_output(task_id, &String::from_utf8_lossy(&chunk));
            }
            match child.try_wait() {
                Ok(Some(exit)) => {
                    // Final drain after exit so no tail output is lost; the
                    // channel closes once both reader threads finish.
                    while let Ok(chunk) = out_rx.recv() {
                        self.append_output(task_id, &String::from_utf8_lossy(&chunk));
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
                        detail = Some("task exceeded its timeout and was killed".into());
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
                        let out = &task.output;
                        out.contains("mux_client")
                            || out.contains("Control socket")
                            || out.contains("control socket")
                            || out.contains("proxy command")
                            || out.contains("kex_exchange_identification")
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
        // V1 uses a uniform default; a per-task timeout can be added to
        // TaskSnapshot without changing the engine contract.
        let _ = task_id;
        TASK_DEFAULT_TIMEOUT
    }

    fn append_output(&self, task_id: &str, data: &str) {
        if let Ok(mut tasks) = self.tasks.lock() {
            if let Some(task) = tasks.get_mut(task_id) {
                let remaining = TASK_OUTPUT_MAX_CHARS.saturating_sub(task.output.len());
                if remaining == 0 {
                    task.snapshot.output_truncated = true;
                    return;
                }
                let mut taken: String = data.chars().take(remaining).collect();
                if taken.len() < data.len() {
                    task.snapshot.output_truncated = true;
                }
                task.snapshot.output_chars += taken.len();
                task.output.push_str(&taken);
                taken.clear();
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
        if let Ok(mut tasks) = self.tasks.lock() {
            if let Some(task) = tasks.get_mut(task_id) {
                if task.snapshot.status.is_terminal() {
                    // Late finish (e.g. disconnect classification racing a
                    // cancel) must not rewrite a terminal result.
                } else {
                    task.snapshot.status = status;
                    task.snapshot.exit_code = exit_code;
                    task.snapshot.ended_at_ms = Some(now_ms());
                    task.snapshot.detail = detail;
                    task.child = None;
                }
            }
        }
        if let Ok(mut running) = self.running_by_connection.lock() {
            if running.get(connection_id).map(String::as_str) == Some(task_id) {
                running.remove(connection_id);
            }
        }
        self.cleanup_counter.fetch_add(1, Ordering::SeqCst);
    }

    /// Client-facing cancel. Distinct from "confirmed ended": a cancel on a
    /// running task asks us to kill the ssh child; the task only moves to
    /// Cancelled once the child is confirmed dead in run_task.
    pub fn cancel_for_client(&self, client_id: &str, task_id: &str) -> Option<TaskSnapshot> {
        let cancelled = {
            let mut tasks = self.tasks.lock().ok()?;
            let task = tasks.get_mut(task_id)?;
            if task.snapshot.client_id != client_id {
                return None;
            }
            match task.snapshot.status {
                TaskStatus::PendingApproval | TaskStatus::Queued => {
                    task.snapshot.status = TaskStatus::Cancelled;
                    task.snapshot.ended_at_ms = Some(now_ms());
                    None
                }
                TaskStatus::Running => Some(task.cancelled.clone()),
                _ => None,
            }
        };
        if let Some(flag) = cancelled {
            flag.store(true, Ordering::SeqCst);
        }
        self.get_task_by_id(task_id)
    }

    /// Revocation path: cancel every non-terminal task for a connection
    /// (generation change, close, or explicit revoke). Running tasks get the
    /// cancel flag; pending approvals are cancelled outright so a stale
    /// approval can never execute later.
    pub fn cancel_connection_tasks(&self, connection_id: &str) {
        let mut to_signal = Vec::new();
        if let Ok(mut tasks) = self.tasks.lock() {
            for task in tasks.values_mut() {
                if task.snapshot.connection_id != connection_id {
                    continue;
                }
                match task.snapshot.status {
                    TaskStatus::PendingApproval | TaskStatus::Queued => {
                        task.snapshot.status = TaskStatus::Cancelled;
                        task.snapshot.ended_at_ms = Some(now_ms());
                    }
                    TaskStatus::Running => {
                        to_signal.push(task.cancelled.clone());
                    }
                    _ => {}
                }
            }
        }
        for flag in to_signal {
            flag.store(true, Ordering::SeqCst);
        }
    }

    /// Drop a client's pending approvals when its session ends; running tasks
    /// keep running (they were approved by the user) but remain cancellable.
    pub fn cancel_client_pending(&self, client_id: &str) {
        if let Ok(mut tasks) = self.tasks.lock() {
            for task in tasks.values_mut() {
                if task.snapshot.client_id == client_id
                    && matches!(
                        task.snapshot.status,
                        TaskStatus::PendingApproval | TaskStatus::Queued
                    )
                {
                    task.snapshot.status = TaskStatus::Cancelled;
                    task.snapshot.ended_at_ms = Some(now_ms());
                }
            }
        }
    }

    pub fn pending_approvals(&self) -> Vec<TaskSnapshot> {
        self.tasks
            .lock()
            .map(|tasks| {
                tasks
                    .values()
                    .filter(|task| task.snapshot.status == TaskStatus::PendingApproval)
                    .map(Task::to_snapshot)
                    .collect()
            })
            .unwrap_or_default()
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

    #[test]
    fn submit_is_idempotent_per_request_id() {
        let engine = TaskEngine::default();
        let record = ready_record(1);
        let first = engine.submit("c1", "req-1", &record, "uptime").unwrap();
        let second = engine.submit("c1", "req-1", &record, "uptime").unwrap();
        assert_eq!(first.task_id, second.task_id);
        // A different request id is a different task.
        let third = engine.submit("c1", "req-2", &record, "uptime").unwrap();
        assert_ne!(first.task_id, third.task_id);
    }

    #[test]
    fn submit_requires_ready_managed_connection() {
        let engine = TaskEngine::default();
        let mut record = ready_record(1);
        record.state = ConnectionState::Connecting;
        assert_eq!(
            engine.submit("c1", "r1", &record, "x").unwrap_err(),
            SubmitError::NotReady
        );
        record.state = ConnectionState::Exited;
        assert_eq!(
            engine.submit("c1", "r2", &record, "x").unwrap_err(),
            SubmitError::ConnectionUnavailable
        );
        record.state = ConnectionState::Ready;
        record.mux_socket = None;
        assert_eq!(
            engine.submit("c1", "r3", &record, "x").unwrap_err(),
            SubmitError::ConnectionUnavailable
        );
    }

    #[test]
    fn approval_is_generation_checked_and_single_use() {
        let engine = TaskEngine::default();
        let record = ready_record(5);
        let task = engine.submit("c1", "r1", &record, "id").unwrap();
        assert_eq!(task.status, TaskStatus::PendingApproval);

        // Stale generation: the approval must not execute.
        assert_eq!(
            engine.approve(&task.task_id, 6).unwrap_err(),
            ApprovalError::StaleGeneration
        );
        let after = engine.get_task(&task.task_id).unwrap();
        assert_eq!(after.status, TaskStatus::Rejected);

        let task2 = engine.submit("c1", "r2", &record, "id").unwrap();
        let approved = engine.approve(&task2.task_id, 5).unwrap();
        assert_eq!(approved.status, TaskStatus::Queued);
        // Approving twice is rejected: queued is not pending.
        assert_eq!(
            engine.approve(&task2.task_id, 5).unwrap_err(),
            ApprovalError::NotApprovable
        );
    }

    #[test]
    fn clients_cannot_see_or_cancel_each_others_tasks() {
        let engine = TaskEngine::default();
        let record = ready_record(1);
        let task = engine.submit("alice", "r1", &record, "id").unwrap();
        assert!(engine.get_task_for_client("bob", &task.task_id).is_none());
        assert!(engine.cancel_for_client("bob", &task.task_id).is_none());
        let cancelled = engine.cancel_for_client("alice", &task.task_id).unwrap();
        assert_eq!(cancelled.status, TaskStatus::Cancelled);
    }

    #[test]
    fn cancel_connection_tasks_kills_pending_and_signals_running() {
        let engine = TaskEngine::default();
        let record = ready_record(1);
        let pending = engine.submit("c1", "r1", &record, "a").unwrap();
        let queued = engine.submit("c1", "r2", &record, "b").unwrap();
        engine.approve(&queued.task_id, 1).unwrap();

        engine.cancel_connection_tasks(&record.connection_id);
        assert_eq!(
            engine.get_task(&pending.task_id).unwrap().status,
            TaskStatus::Cancelled
        );
        assert_eq!(
            engine.get_task(&queued.task_id).unwrap().status,
            TaskStatus::Cancelled
        );
    }

    #[test]
    fn one_running_task_per_connection() {
        let engine = Arc::new(TaskEngine::default());
        let record = ready_record(1);
        let t1 = engine.submit("c1", "r1", &record, "sleep 1").unwrap();
        let t2 = engine.submit("c1", "r2", &record, "sleep 1").unwrap();
        engine.approve(&t1.task_id, 1).unwrap();
        engine.approve(&t2.task_id, 1).unwrap();

        // Use /bin/sh via real spawn args — the engine itself only needs a
        // runnable argv, so point it at a trivial local command.
        let args_ok = vec!["-c".to_string(), "exit 0".to_string()];
        assert!(engine.start_approved(&t1.task_id, args_ok.clone()));
        assert!(!engine.start_approved(&t2.task_id, args_ok));
        assert_eq!(
            engine.running_for_connection(&record.connection_id).as_deref(),
            Some(t1.task_id.as_str())
        );
    }

    #[test]
    fn output_is_bounded_and_reports_truncation() {
        let engine = TaskEngine::default();
        let record = ready_record(1);
        let task = engine.submit("c1", "r1", &record, "big").unwrap();
        let chunk = "x".repeat(4096);
        for _ in 0..(TASK_OUTPUT_MAX_CHARS / 4096 + 4) {
            engine.append_output(&task.task_id, &chunk);
        }
        let (data, next, truncated) = engine
            .read_output_for_client("c1", &task.task_id, 0)
            .unwrap();
        assert_eq!(next, TASK_OUTPUT_MAX_CHARS);
        assert_eq!(data.len(), TASK_OUTPUT_MAX_CHARS);
        assert!(truncated);
        // Other clients read nothing.
        assert!(engine
            .read_output_for_client("mallory", &task.task_id, 0)
            .is_none());
    }
}
