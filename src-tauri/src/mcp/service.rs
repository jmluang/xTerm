//! MCP core service: ties the connection registry, output buffers, auth and
//! task engine together, and serves the local IPC socket the stdio bridge
//! talks to (phases B/C, ORQ-29/ORQ-30).
//!
//! The socket lives next to the app config in an app-created directory with
//! `0600` permissions. Every request carries (client_id, token) and is
//! authorized against the *current* connection generation on each call — a
//! reconnect or revoke mid-session fails closed on the next request.

use crate::connection_registry::{ConnectionRegistry, ConnectionState};
use crate::mcp::audit::TaskAuditStore;
use crate::mcp::auth::{AuthError, AuthState, GrantPermission, GrantPermissions};
use crate::mcp::protocol::*;
use crate::mux::MuxManager;
use crate::output_buffer::{OutputBuffers, MCP_READ_MAX_CHARS};
use crate::task_engine::{CommandSpec, SubmitError, TaskEngine, TaskStatus, TaskTransitionError};
use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

const MAX_IPC_LINE_BYTES: usize = 64 * 1024;
const MAX_ACTIVE_IPC_CONNECTIONS: usize = 32;
pub(crate) const MAX_IPC_REQUESTS_PER_SECOND_PER_CLIENT: usize = 60;
pub(crate) const MAX_IPC_REQUESTS_PER_SECOND_GLOBAL: usize = 120;
pub(crate) const MAX_IPC_CLIENT_ID_BYTES: usize = 128;
pub(crate) const MAX_IPC_TOKEN_BYTES: usize = 256;
const IPC_RATE_WINDOW: Duration = Duration::from_secs(1);

pub struct McpService {
    pub registry: Arc<ConnectionRegistry>,
    pub buffers: Arc<OutputBuffers>,
    pub auth: Arc<AuthState>,
    pub tasks: Arc<TaskEngine>,
    pub mux: Arc<MuxManager>,
    access_gate: Arc<RwLock<()>>,
    settings_gate: Mutex<()>,
    ipc_rate_limiter: Arc<IpcRateLimiter>,
}

struct IpcRateWindow {
    started_at: Instant,
    global_count: usize,
    per_client: HashMap<String, usize>,
}

struct IpcRateLimiter {
    state: Mutex<IpcRateWindow>,
}

impl IpcRateLimiter {
    fn new() -> Self {
        Self {
            state: Mutex::new(IpcRateWindow {
                started_at: Instant::now(),
                global_count: 0,
                per_client: HashMap::new(),
            }),
        }
    }

    fn check_at(&self, auth: &AuthState, client_id: &str, token: &str, now: Instant) -> bool {
        let authenticated = auth.authenticate(client_id, token).is_ok();
        let Ok(mut window) = self.state.lock() else {
            return true;
        };
        if now < window.started_at || now.duration_since(window.started_at) >= IPC_RATE_WINDOW {
            window.started_at = now;
            window.global_count = 0;
            window.per_client.clear();
        }
        // Pairings are capped by AuthState. Drop stale keys here as well so
        // an unpaired client never consumes a future pairing's rate slot.
        window.per_client.retain(|id, _| auth.is_paired(id));
        if window.global_count >= MAX_IPC_REQUESTS_PER_SECOND_GLOBAL {
            return true;
        }
        if !authenticated {
            // Invalid/unknown credentials are intentionally global-only: do
            // not let arbitrary client_id values grow the per-client map.
            window.global_count += 1;
            return false;
        }
        let client_count = window.per_client.get(client_id).copied().unwrap_or(0);
        if client_count >= MAX_IPC_REQUESTS_PER_SECOND_PER_CLIENT {
            return true;
        }
        if client_count == 0 && window.per_client.len() >= crate::mcp::auth::MAX_PAIRED_CLIENTS {
            // This should be unreachable with the pairing cap, but fail
            // closed if an inconsistent store is ever observed.
            return true;
        }
        window.global_count += 1;
        *window.per_client.entry(client_id.to_string()).or_insert(0) += 1;
        false
    }

    fn forget_client(&self, client_id: &str) {
        if let Ok(mut window) = self.state.lock() {
            window.per_client.remove(client_id);
        }
    }

    #[cfg(test)]
    fn tracked_client_count(&self) -> usize {
        self.state
            .lock()
            .map(|window| window.per_client.len())
            .unwrap_or(usize::MAX)
    }
}

impl McpService {
    /// In-memory auth (tests). The app uses `with_auth` +
    /// `AuthState::open_default` so pairings survive restarts.
    #[cfg(test)]
    pub fn new(mux: Arc<MuxManager>) -> Self {
        Self::with_auth(mux, AuthState::default())
    }

    #[cfg(test)]
    pub fn with_auth(mux: Arc<MuxManager>, auth: AuthState) -> Self {
        Self::build(mux, auth, Arc::new(TaskEngine::default()))
    }

    /// Production construction requires both persistent MCP stores. An audit
    /// open failure prevents service startup instead of silently losing task
    /// request idempotency.
    pub fn with_persistent_state(
        mux: Arc<MuxManager>,
        auth: AuthState,
        audit: Arc<TaskAuditStore>,
    ) -> Self {
        Self::build(mux, auth, Arc::new(TaskEngine::with_audit(audit)))
    }

    fn build(mux: Arc<MuxManager>, auth: AuthState, tasks: Arc<TaskEngine>) -> Self {
        Self {
            registry: Arc::new(ConnectionRegistry::default()),
            buffers: Arc::new(OutputBuffers::default()),
            auth: Arc::new(auth),
            tasks,
            mux,
            access_gate: Arc::new(RwLock::new(())),
            settings_gate: Mutex::new(()),
            ipc_rate_limiter: Arc::new(IpcRateLimiter::new()),
        }
    }

    pub fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        let _settings = self
            .settings_gate
            .lock()
            .map_err(|_| "MCP settings gate poisoned")?;
        if enabled {
            self.auth.persist_enabled_value(true, false)?;
            let _gate = self
                .access_gate
                .write()
                .map_err(|_| "MCP access gate poisoned")?;
            return self.auth.set_enabled_in_memory(true);
        }

        {
            let _gate = self
                .access_gate
                .write()
                .map_err(|_| "MCP access gate poisoned")?;
            self.auth.set_enabled_in_memory(false)?;
            self.tasks.signal_cancel_all_tasks();
        }
        let persist_result = self.auth.persist_enabled_value(false, true);
        self.tasks.cancel_all_tasks();
        for record in self.registry.list() {
            self.buffers.remove(&record.connection_id);
        }
        persist_result
    }

    pub fn enabled_status(&self) -> Result<bool, String> {
        let _gate = self
            .access_gate
            .read()
            .map_err(|_| "MCP access gate poisoned")?;
        Ok(self.auth.is_enabled())
    }

    pub fn pending_approvals(&self) -> Result<Vec<crate::task_engine::TaskSnapshot>, String> {
        let pending = self
            .tasks
            .pending_approvals()
            .map_err(|_| "task approvals could not be expired in the audit ledger".to_string())?;
        let _gate = self
            .access_gate
            .read()
            .map_err(|_| "MCP access gate poisoned")?;
        if !self.auth.is_enabled() {
            return Ok(Vec::new());
        }
        Ok(pending)
    }

    /// Return a bounded, newest-first audit view for trusted in-process UI.
    #[allow(dead_code)] // exposed for the upcoming Tauri audit command
    pub fn recent_task_audit(
        &self,
        limit: Option<usize>,
    ) -> Result<Vec<crate::mcp::audit::TaskAuditRecord>, String> {
        let _gate = self
            .access_gate
            .read()
            .map_err(|_| "MCP access gate poisoned")?;
        self.tasks.recent_audit(limit)
    }

    /// Accept one decoded PTY chunk for the MCP observation buffer.
    pub fn capture_pty_output(&self, connection_id: &str, data: String) {
        let _gate = match self.access_gate.try_read() {
            Ok(gate) => gate,
            Err(std::sync::TryLockError::WouldBlock) => {
                self.buffers.record_capture_gap(connection_id, data.len());
                return;
            }
            Err(std::sync::TryLockError::Poisoned(_)) => {
                self.buffers.record_capture_gap(connection_id, data.len());
                return;
            }
        };
        if !self.auth.is_enabled() {
            return;
        }
        let Some(record) = self.registry.get(connection_id) else {
            return;
        };
        if !record.mcp_eligible()
            || !self
                .auth
                .has_observe_grant(connection_id, record.generation)
        {
            return;
        }
        self.buffers.push(connection_id, data);
    }

    pub fn set_client_connection_permissions(
        &self,
        client_id: &str,
        connection_id: &str,
        permissions: GrantPermissions,
    ) -> Result<(), String> {
        if !permissions.observe && !permissions.execute {
            self.revoke_client_connection(client_id, connection_id)?;
            return Ok(());
        }

        let (generation, grant_start_seq) = {
            let _gate = self
                .access_gate
                .read()
                .map_err(|_| "MCP access gate poisoned")?;
            if !self.auth.is_enabled() {
                return Err("MCP is disabled".into());
            }
            let record = self
                .registry
                .get(connection_id)
                .ok_or("unknown connection")?;
            if !record.mcp_eligible() || record.state != ConnectionState::Ready {
                if record.requires_reconnect_for_mcp() {
                    return Err(
                        "this connection predates managed reuse; reconnect it to enable MCP".into(),
                    );
                }
                return Err("connection is closing or gone".into());
            }
            (record.generation, self.buffers.current_seq())
        };
        let grant = self.auth.prepare_grant(
            client_id,
            connection_id,
            generation,
            grant_start_seq,
            permissions,
        )?;
        self.auth.persist_prepared_grant(&grant)?;
        let publish_result = {
            let _gate = self
                .access_gate
                .write()
                .map_err(|_| "MCP access gate poisoned")?;
            let current = self.registry.get(connection_id);
            if !self.auth.is_enabled()
                || !current.as_ref().is_some_and(|record| {
                    record.state == ConnectionState::Ready
                        && record.generation == generation
                        && record.mcp_eligible()
                })
            {
                Err("connection changed before permissions were applied".to_string())
            } else {
                self.auth.publish_prepared_grant(grant.clone())
            }
        };
        if let Err(error) = publish_result {
            let _ = self.auth.persist_grant_removal(client_id, connection_id);
            return Err(error);
        }
        self.tasks
            .cancel_client_connection_tasks(client_id, connection_id);
        self.clear_buffer_if_unobserved(connection_id);
        Ok(())
    }

    pub fn revoke_client_connection(
        &self,
        client_id: &str,
        connection_id: &str,
    ) -> Result<bool, String> {
        let removed = {
            let _gate = self
                .access_gate
                .write()
                .map_err(|_| "MCP access gate poisoned")?;
            let removed = self.auth.remove_grant_in_memory(client_id, connection_id)?;
            self.tasks
                .signal_cancel_client_connection_tasks(client_id, connection_id);
            removed
        };
        let persist_result = self.auth.persist_grant_removal(client_id, connection_id);
        if removed || persist_result.is_err() {
            self.tasks
                .cancel_client_connection_tasks(client_id, connection_id);
        }
        self.clear_buffer_if_unobserved(connection_id);
        persist_result.map(|()| removed)
    }

    pub fn unpair_client(&self, client_id: &str) -> Result<(), String> {
        let affected_connections: Vec<String> = {
            let _gate = self
                .access_gate
                .write()
                .map_err(|_| "MCP access gate poisoned")?;
            let connections = self
                .auth
                .list_clients()
                .into_iter()
                .find(|client| client.client_id == client_id)
                .map(|client| {
                    client
                        .grants
                        .into_iter()
                        .map(|grant| grant.connection_id)
                        .collect()
                })
                .unwrap_or_default();
            self.auth.unpair_in_memory(client_id)?;
            self.tasks.signal_cancel_client_tasks(client_id);
            connections
        };
        let result = self.auth.persist_unpair(client_id);
        self.ipc_rate_limiter.forget_client(client_id);
        self.tasks.cancel_client_tasks(client_id);
        for connection_id in affected_connections {
            self.clear_buffer_if_unobserved(&connection_id);
        }
        result
    }

    /// End the authorization session represented by one IPC transport.
    ///
    /// A bridge socket is bound to its first successfully authenticated
    /// client. Once that transport ends, every operation grant for the bound
    /// client is invalidated at the access-gate write boundary. Pairing
    /// identity remains available for a later bridge session, but no grant,
    /// task, or observation buffer is allowed to survive this transport.
    pub fn on_client_transport_closed(&self, client_id: &str) {
        let affected_connections = {
            let Ok(_gate) = self.access_gate.write() else {
                eprintln!("[mcp] cannot close client transport: access gate poisoned");
                return;
            };

            let grants = self
                .auth
                .list_clients()
                .into_iter()
                .find(|client| client.client_id == client_id)
                .map(|client| client.grants)
                .unwrap_or_default();
            let affected_connections: Vec<String> = grants
                .iter()
                .map(|grant| grant.connection_id.clone())
                .collect();

            for grant in grants {
                if let Err(error) = self
                    .auth
                    .remove_grant_in_memory(client_id, &grant.connection_id)
                {
                    eprintln!(
                        "[mcp] failed to revoke transport grant in memory for {client_id}: {error}"
                    );
                }
            }

            // Pending/queued tasks are terminalized immediately; running or
            // dispatching tasks receive the existing best-effort cancel
            // signal and settle asynchronously in the task supervisor.
            self.tasks.signal_cancel_client_tasks(client_id);

            // Do this inside the same write boundary after the client's
            // grants are gone. Another client that still observes the same
            // connection keeps its buffer alive.
            for connection_id in &affected_connections {
                let observed_elsewhere = self.registry.get(connection_id).is_some_and(|record| {
                    self.auth
                        .has_observe_grant(connection_id, record.generation)
                });
                if !observed_elsewhere {
                    self.buffers.remove(connection_id);
                }
            }
            affected_connections
        };

        self.ipc_rate_limiter.forget_client(client_id);
        // Persistence is deliberately after in-memory invalidation. A locked
        // or failing SQLite store must not leave a live grant in this app
        // session; the next app startup already drops operation grants.
        for connection_id in affected_connections {
            if let Err(error) = self.auth.persist_grant_removal(client_id, &connection_id) {
                eprintln!(
                    "[mcp] client transport grant was invalidated in memory but could not be persisted for {client_id}/{connection_id}: {error}"
                );
            }
        }
    }

    /// End every operation authorization when macOS moves the login session
    /// out of the active state. Pairing identities and the enabled preference
    /// are deliberately retained, but grants, task execution and observation
    /// data must fail closed until the user grants access again.
    pub fn on_system_session_resigned_active(&self) {
        let grants = {
            let Ok(_gate) = self.access_gate.write() else {
                eprintln!("[mcp] cannot resign system session: access gate poisoned");
                return;
            };

            let grants: Vec<(String, String)> = self
                .auth
                .list_clients()
                .into_iter()
                .flat_map(|client| {
                    client
                        .grants
                        .into_iter()
                        .map(move |grant| (client.client_id.clone(), grant.connection_id))
                })
                .collect();

            for (client_id, connection_id) in &grants {
                if let Err(error) = self.auth.remove_grant_in_memory(client_id, connection_id) {
                    eprintln!(
                        "[mcp] failed to revoke session-resign grant in memory for {client_id}/{connection_id}: {error}"
                    );
                }
            }

            // Pending/queued tasks become terminally cancelled. Dispatching,
            // running and already-cancel-requested tasks receive the existing
            // best-effort cancellation signal and settle asynchronously.
            self.tasks.signal_cancel_all_tasks();
            self.buffers.clear_all();
            grants
        };

        // In-memory authorization is already fail-closed before attempting
        // SQLite. A persistence failure must never restore a live grant or
        // automatically re-authorize when the session becomes active again.
        for (client_id, connection_id) in grants {
            if let Err(error) = self.auth.persist_grant_removal(&client_id, &connection_id) {
                eprintln!(
                    "[mcp] system session grant was invalidated in memory but could not be persisted for {client_id}/{connection_id}: {error}"
                );
            }
        }
    }

    fn check_ipc_rate_at(&self, request: &IpcRequest, now: Instant) -> Option<IpcResponse> {
        let (client_id, token) = ipc_credentials(request);
        if client_id.len() > MAX_IPC_CLIENT_ID_BYTES {
            return Some(IpcResponse::error(
                "invalid_client_id",
                "client_id must be at most 128 UTF-8 bytes",
            ));
        }
        if token.len() > MAX_IPC_TOKEN_BYTES {
            return Some(IpcResponse::error(
                "invalid_token",
                "token must be at most 256 UTF-8 bytes",
            ));
        }
        self.ipc_rate_limiter
            .check_at(&self.auth, client_id, token, now)
            .then(|| IpcResponse::error("rate_limited", "MCP request rate limit exceeded"))
    }

    fn check_ipc_rate(&self, request: &IpcRequest) -> Option<IpcResponse> {
        self.check_ipc_rate_at(request, Instant::now())
    }

    fn clear_buffer_if_unobserved(&self, connection_id: &str) {
        let Some(record) = self.registry.get(connection_id) else {
            self.buffers.remove(connection_id);
            return;
        };
        if !self
            .auth
            .has_observe_grant(connection_id, record.generation)
        {
            self.buffers.remove(connection_id);
        }
    }

    /// Persist and tear down resources only after the registry and in-memory
    /// grants have crossed the write-gate boundary.
    fn on_connection_closed_after_gate(&self, connection_id: &str) {
        if let Err(error) = self.auth.persist_connection_revoke(connection_id) {
            eprintln!("[mcp] failed to persist connection revoke: {error}");
        }
        self.tasks.cancel_connection_tasks(connection_id);
        self.buffers.remove(connection_id);
        self.mux.teardown(connection_id);
    }

    pub fn begin_connection_close(&self, session_id: &str) -> bool {
        let record = {
            let Ok(_gate) = self.access_gate.write() else {
                return false;
            };
            let Some(record) = self.registry.begin_close_by_session(session_id) else {
                return false;
            };
            if let Err(error) = self.auth.revoke_connection_in_memory(&record.connection_id) {
                eprintln!("[mcp] failed to revoke closing connection in memory: {error}");
            }
            self.tasks
                .signal_cancel_connection_tasks(&record.connection_id);
            record
        };
        self.on_connection_closed_after_gate(&record.connection_id);
        true
    }

    pub fn mark_connection_exited(&self, session_id: &str, exit_code: u32) {
        let record = {
            let Ok(_gate) = self.access_gate.write() else {
                return;
            };
            let Some(record) = self.registry.mark_exited_by_session(session_id, exit_code) else {
                return;
            };
            if let Err(error) = self.auth.revoke_connection_in_memory(&record.connection_id) {
                eprintln!("[mcp] failed to revoke exited connection in memory: {error}");
            }
            self.tasks
                .signal_cancel_connection_tasks(&record.connection_id);
            record
        };
        self.on_connection_closed_after_gate(&record.connection_id);
    }

    fn handle(&self, request: IpcRequest) -> IpcResponse {
        if let Some(response) = self.check_ipc_rate(&request) {
            return response;
        }
        match request {
            IpcRequest::ListConnections { client_id, token } => {
                let _gate = match self.access_gate.read() {
                    Ok(gate) => gate,
                    Err(_) => return IpcResponse::error("disabled", "MCP access is unavailable"),
                };
                if !self.auth.is_enabled() {
                    return IpcResponse::error("disabled", "MCP is disabled");
                }
                if self.auth.authenticate(&client_id, &token).is_err() {
                    return IpcResponse::error("unauthorized", "unknown client or bad token");
                }
                let clients = self.auth.list_clients();
                let granted: Vec<_> = clients
                    .iter()
                    .filter(|c| c.client_id == client_id)
                    .flat_map(|c| c.grants.clone())
                    .collect();
                let mut connections = Vec::new();
                for listed_grant in granted {
                    let Some(record) = self.registry.get(&listed_grant.connection_id) else {
                        continue;
                    };
                    // Grant + token validated; re-check generation & state.
                    let Ok(grant) = self.auth.authorize_grant(
                        &client_id,
                        &token,
                        &listed_grant.connection_id,
                        record.generation,
                    ) else {
                        continue;
                    };
                    if !record.mcp_eligible() {
                        continue;
                    }
                    let summary = record.mcp_summary();
                    connections.push(IpcConnection {
                        connection_id: summary.connection_id,
                        generation: summary.generation,
                        host_name: summary.host_name,
                        user: summary.user,
                        port: summary.port,
                        alias: summary.alias,
                        state: format!("{:?}", summary.state).to_lowercase(),
                        grant_start_seq: grant.grant_start_seq,
                        observe: grant.observe,
                        execute: grant.execute,
                    });
                }
                IpcResponse::Connections { connections }
            }
            IpcRequest::ReadConnectionOutput {
                client_id,
                token,
                connection_id,
                cursor,
                max_chars,
            } => {
                let _gate = match self.access_gate.read() {
                    Ok(gate) => gate,
                    Err(_) => return IpcResponse::error("disabled", "MCP access is unavailable"),
                };
                if !self.auth.is_enabled() {
                    return IpcResponse::error("disabled", "MCP is disabled");
                }
                let Some(record) = self.registry.get(&connection_id) else {
                    return IpcResponse::error("not_found", "unknown connection");
                };
                let grant = match self.auth.authorize(
                    &client_id,
                    &token,
                    &connection_id,
                    record.generation,
                    GrantPermission::Observe,
                ) {
                    Ok(grant) => grant,
                    Err(AuthError::MissingPermission) => {
                        return IpcResponse::error("forbidden", "grant lacks observe permission")
                    }
                    Err(_) => return IpcResponse::error("unauthorized", "no grant for connection"),
                };
                if !record.mcp_eligible() {
                    return IpcResponse::error("unavailable", "connection is closing or gone");
                }
                // Never serve data from before the grant, even if retained.
                let effective = cursor.max(grant.grant_start_seq);
                let gap = cursor < grant.grant_start_seq;
                let Some(read) = self.buffers.read(
                    &connection_id,
                    effective,
                    max_chars.unwrap_or(MCP_READ_MAX_CHARS),
                ) else {
                    return IpcResponse::error("not_found", "no output buffer for connection");
                };
                let data: String = read.chunks.iter().map(|c| c.data.as_str()).collect();
                IpcResponse::Output(IpcOutputRead {
                    data,
                    next_cursor: read.next_seq,
                    gap: gap || read.gap_from_seq.is_some(),
                    truncated: read.truncated,
                })
            }
            IpcRequest::RunCommand {
                client_id,
                token,
                connection_id,
                request_id,
                command,
                working_directory,
                timeout_seconds,
            } => self.handle_run_command(
                &client_id,
                &token,
                &connection_id,
                CommandSpec {
                    request_id: &request_id,
                    command: &command,
                    working_directory: &working_directory,
                    timeout_seconds,
                },
            ),
            IpcRequest::ReadTask {
                client_id,
                token,
                task_id,
                stdout_offset,
                stderr_offset,
                output_offset,
            } => {
                let _gate = match self.access_gate.read() {
                    Ok(gate) => gate,
                    Err(_) => return IpcResponse::error("disabled", "MCP access is unavailable"),
                };
                if !self.auth.is_enabled() {
                    return IpcResponse::error("disabled", "MCP is disabled");
                }
                let snapshot = match self.authorized_task(&client_id, &token, &task_id) {
                    Ok(snapshot) => snapshot,
                    Err(response) => return response,
                };
                // Legacy output_offset is explicitly stdout-only. stderr has
                // its own cursor and starts at zero when omitted; no global
                // ordering between the streams is claimed.
                let stdout_offset = stdout_offset.or(output_offset);
                let Some(read) = self.tasks.read_task_output_for_client(
                    &client_id,
                    &task_id,
                    stdout_offset,
                    stderr_offset,
                ) else {
                    return IpcResponse::error("not_found", "unknown task");
                };
                IpcResponse::Task(Box::new(IpcTaskRead {
                    status: task_status_name(snapshot.status).to_string(),
                    exit_code: snapshot.exit_code,
                    stdout: read.stdout,
                    stderr: read.stderr,
                    next_stdout_offset: read.stdout_next_offset,
                    next_stderr_offset: read.stderr_next_offset,
                    stdout_truncated: read.stdout_truncated,
                    stderr_truncated: read.stderr_truncated,
                    stdout_gap: read.stdout_gap,
                    stderr_gap: read.stderr_gap,
                    detail: snapshot.detail,
                }))
            }
            IpcRequest::CancelTask {
                client_id,
                token,
                task_id,
            } => {
                // Keep authorization and the durable cancellation transition
                // under one access gate. Signalling first and performing the
                // state transition later lets a fast worker settle the task
                // between the two calls, turning a valid cancel into a
                // misleading NotCancellable response.
                let _gate = match self.access_gate.write() {
                    Ok(gate) => gate,
                    Err(_) => return IpcResponse::error("disabled", "MCP access is unavailable"),
                };
                if !self.auth.is_enabled() {
                    return IpcResponse::error("disabled", "MCP is disabled");
                }
                if let Err(response) = self.authorized_task(&client_id, &token, &task_id) {
                    return response;
                }
                match self.tasks.cancel_for_client(&client_id, &task_id) {
                    Ok(Some(_)) => IpcResponse::Ack { ok: true },
                    Ok(None) => IpcResponse::error("not_found", "unknown or finished task"),
                    Err(TaskTransitionError::NotCancellable) => {
                        IpcResponse::error("not_found", "unknown or finished task")
                    }
                    Err(
                        TaskTransitionError::AuditUnavailable | TaskTransitionError::DispatchFailed,
                    ) => IpcResponse::error(
                        "audit_unavailable",
                        "task cancellation could not be recorded in the audit ledger",
                    ),
                }
            }
        }
    }

    fn authorized_task(
        &self,
        client_id: &str,
        token: &str,
        task_id: &str,
    ) -> Result<crate::task_engine::TaskSnapshot, IpcResponse> {
        if self.auth.authenticate(client_id, token).is_err() {
            return Err(IpcResponse::error(
                "unauthorized",
                "unknown client or bad token",
            ));
        }
        let Some(snapshot) = self.tasks.get_task_for_client(client_id, task_id) else {
            return Err(IpcResponse::error("not_found", "unknown task"));
        };
        let Some(record) = self.registry.get(&snapshot.connection_id) else {
            return Err(IpcResponse::error(
                "unauthorized",
                "task connection is no longer available",
            ));
        };
        if !record.mcp_eligible() || record.generation != snapshot.generation {
            return Err(IpcResponse::error(
                "unauthorized",
                "task connection grant is no longer valid",
            ));
        }
        let grant = self
            .auth
            .authorize(
                client_id,
                token,
                &snapshot.connection_id,
                record.generation,
                GrantPermission::Execute,
            )
            .map_err(|error| match error {
                AuthError::MissingPermission => {
                    IpcResponse::error("forbidden", "grant lacks execute permission")
                }
                _ => IpcResponse::error("unauthorized", "task connection grant is no longer valid"),
            })?;
        if !self.tasks.grant_matches_task(client_id, task_id, &grant) {
            return Err(IpcResponse::error(
                "unauthorized",
                "task belongs to a different connection grant",
            ));
        }
        Ok(snapshot)
    }

    fn handle_run_command(
        &self,
        client_id: &str,
        token: &str,
        connection_id: &str,
        spec: CommandSpec<'_>,
    ) -> IpcResponse {
        if !self.auth.is_enabled() {
            return IpcResponse::error("disabled", "MCP is disabled");
        }
        let Some(record) = self.registry.get(connection_id) else {
            return IpcResponse::error("not_found", "unknown connection");
        };
        let grant = match self.auth.authorize(
            client_id,
            token,
            connection_id,
            record.generation,
            GrantPermission::Execute,
        ) {
            Ok(grant) => grant,
            Err(AuthError::MissingPermission) => {
                return IpcResponse::error("forbidden", "grant lacks execute permission")
            }
            Err(_) => return IpcResponse::error("unauthorized", "no grant for connection"),
        };
        let snapshot = match self.tasks.submit(client_id, &record, spec, &grant) {
            Ok(snapshot) => snapshot,
            Err(SubmitError::InvalidRequestId) => {
                return IpcResponse::error(
                    "invalid_request_id",
                    "request_id must be non-empty and at most 128 UTF-8 bytes",
                )
            }
            Err(SubmitError::InvalidCommand) => {
                return IpcResponse::error(
                    "invalid_command",
                    "command must be non-empty and at most 16384 UTF-8 bytes",
                )
            }
            Err(SubmitError::InvalidWorkingDirectory) => {
                return IpcResponse::error(
                    "invalid_working_directory",
                    "working_directory must be 'login' in MCP V1",
                )
            }
            Err(SubmitError::InvalidTimeout) => {
                return IpcResponse::error(
                    "invalid_timeout",
                    "timeout_seconds must be between 1 and 1800",
                )
            }
            Err(SubmitError::PendingApprovalLimitPerClient) => {
                return IpcResponse::error(
                    "pending_approval_limit",
                    "this client already has 8 pending command approvals",
                )
            }
            Err(SubmitError::PendingApprovalLimitGlobal) => {
                return IpcResponse::error(
                    "pending_approval_limit",
                    "the app already has 32 pending command approvals",
                )
            }
            Err(SubmitError::RetainedTaskLimit) => {
                return IpcResponse::error(
                    "task_capacity",
                    "the app has 256 retained command tasks; retry after a task finishes",
                )
            }
            Err(SubmitError::NotReady) => {
                return IpcResponse::error(
                    "not_ready",
                    "connection has not finished authentication verification",
                )
            }
            Err(SubmitError::ConnectionUnavailable) => {
                return IpcResponse::error("unavailable", "connection is closing or unmanaged")
            }
            Err(SubmitError::RequestConflict) => {
                return IpcResponse::error(
                    "request_conflict",
                    "request_id was already used for a different request or grant scope",
                )
            }
            Err(SubmitError::RequestFromPreviousSession) => {
                return IpcResponse::error(
                    "request_from_previous_session",
                    "request_id belongs to an earlier app session and cannot be replayed",
                )
            }
            Err(SubmitError::AuditUnavailable) => {
                return IpcResponse::error(
                    "audit_unavailable",
                    "task request could not be recorded; no task was created",
                )
            }
            Err(SubmitError::GrantMismatch) => {
                return IpcResponse::error("unauthorized", "grant does not match the request")
            }
        };
        let authorized = {
            let _gate = match self.access_gate.read() {
                Ok(gate) => gate,
                Err(_) => return IpcResponse::error("disabled", "MCP access is unavailable"),
            };
            if !self.auth.is_enabled() {
                false
            } else if let Some(current) = self.registry.get(&snapshot.connection_id) {
                current.state == ConnectionState::Ready
                    && current.generation == snapshot.generation
                    && self
                        .auth
                        .current_grant(
                            client_id,
                            &snapshot.connection_id,
                            current.generation,
                            GrantPermission::Execute,
                        )
                        .is_ok_and(|grant| {
                            self.tasks
                                .grant_matches_task(client_id, &snapshot.task_id, &grant)
                        })
            } else {
                false
            }
        };
        if !authorized {
            let _ = self.tasks.cancel_for_client(client_id, &snapshot.task_id);
            return IpcResponse::error(
                "unauthorized",
                "the connection grant changed while the request was being recorded",
            );
        }
        // If the task already exists (idempotent retry) and was approved
        // meanwhile, make sure it is actually started.
        if snapshot.status == TaskStatus::Queued {
            if let Err(message) = self.try_start_task(&snapshot.task_id) {
                return IpcResponse::error("dispatch_failed", message);
            }
        }
        let current = self
            .tasks
            .internal_snapshot(&snapshot.task_id)
            .unwrap_or(snapshot);
        IpcResponse::RunAccepted(IpcRunAccepted {
            task_id: current.task_id,
            status: task_status_name(current.status).to_string(),
        })
    }

    fn try_start_task(&self, task_id: &str) -> Result<(), String> {
        self.try_start_task_with_verifier(task_id, |record| {
            self.mux.verify(record).map_err(|error| error.to_string())
        })
    }

    fn try_start_task_with_verifier(
        &self,
        task_id: &str,
        verify: impl FnOnce(&crate::connection_registry::ConnectionRecord) -> Result<(), String>,
    ) -> Result<(), String> {
        let Some(snapshot) = self.tasks.internal_snapshot(task_id) else {
            return Ok(());
        };
        if snapshot.status != TaskStatus::Queued {
            return Ok(());
        }
        let reject = |detail: &str| {
            self.tasks
                .fail_dispatch(task_id, TaskStatus::Rejected, detail)
                .map(|_| ())
                .map_err(|_| {
                    "dispatch rejection could not be written to the audit ledger".to_string()
                })
        };
        let fail = |detail: &str| {
            self.tasks
                .fail_dispatch(task_id, TaskStatus::Failed, detail)
                .map(|_| ())
                .map_err(|_| {
                    "dispatch failure could not be written to the audit ledger".to_string()
                })
        };

        let Some(record) = self.registry.get(&snapshot.connection_id) else {
            reject("connection disappeared before command dispatch")?;
            return Err("connection disappeared before command dispatch".into());
        };
        if record.state != ConnectionState::Ready
            || record.generation != snapshot.generation
            || !record.mcp_eligible()
        {
            reject("connection was no longer Ready at command dispatch")?;
            return Err("connection was no longer Ready at command dispatch".into());
        }
        let grant = self.auth.current_grant(
            &snapshot.client_id,
            &snapshot.connection_id,
            record.generation,
            GrantPermission::Execute,
        );
        let Ok(grant) = grant else {
            reject("execute grant was revoked before command dispatch")?;
            return Err("execute grant was revoked before command dispatch".into());
        };
        if !self
            .tasks
            .grant_matches_task(&snapshot.client_id, task_id, &grant)
        {
            reject("task execute grant changed before command dispatch")?;
            return Err("task execute grant changed before command dispatch".into());
        }
        if let Err(error) = verify(&record) {
            let detail = format!("mux verification failed: {error}");
            fail(&detail)?;
            return Err(detail);
        }
        let Some(socket) = record.mux_socket.clone() else {
            let detail = "managed mux socket was missing after verification";
            fail(detail)?;
            return Err(detail.into());
        };
        if let Err(error) = self.tasks.record_dispatch_start(task_id) {
            let detail = "dispatch audit transition failed before SSH spawn";
            return Err(format!("{detail}: {error:?}"));
        }
        if !self.tasks.is_dispatching_and_reserved(task_id) {
            return Ok(());
        }
        let args =
            self.mux
                .derived_channel_args(&socket, &record.host.ssh_target(), &snapshot.command);

        let gate = match self.access_gate.write() {
            Ok(gate) => gate,
            Err(_) => {
                let detail = "MCP access gate was unavailable before command dispatch";
                fail(detail)?;
                return Err(detail.into());
            }
        };
        let invalid = if !self.auth.is_enabled() {
            Some("MCP was disabled before command dispatch")
        } else if let Some(current) = self.registry.get(&snapshot.connection_id) {
            if current.state != ConnectionState::Ready
                || current.generation != snapshot.generation
                || !current.mcp_eligible()
            {
                Some("connection generation or Ready state changed before command dispatch")
            } else if self
                .auth
                .current_grant(
                    &snapshot.client_id,
                    &snapshot.connection_id,
                    current.generation,
                    GrantPermission::Execute,
                )
                .is_err_and(|_| true)
            {
                Some("execute grant was revoked before command dispatch")
            } else {
                let current_grant = self.auth.current_grant(
                    &snapshot.client_id,
                    &snapshot.connection_id,
                    current.generation,
                    GrantPermission::Execute,
                );
                if current_grant.is_ok_and(|grant| {
                    !self
                        .tasks
                        .grant_matches_task(&snapshot.client_id, task_id, &grant)
                }) {
                    Some("task execute grant changed before command dispatch")
                } else if !self.tasks.is_queued_and_reserved(task_id) {
                    Some("task left its reserved Queued state before command dispatch")
                } else {
                    None
                }
            }
        } else {
            Some("connection disappeared before command dispatch")
        };
        if let Some(detail) = invalid {
            drop(gate);
            reject(detail)?;
            return Err(detail.into());
        }
        let result = self
            .tasks
            .start_prepared(task_id, args, Some(Arc::clone(&self.access_gate)));
        drop(gate);
        match result {
            Ok(true) => Ok(()),
            Ok(false) => {
                let detail = "task left its reserved Queued state before dispatch";
                reject(detail)?;
                Err(detail.into())
            }
            Err(error) => {
                let detail = "failed to start the local task supervisor";
                fail(detail)?;
                Err(format!("{detail}: {error:?}"))
            }
        }
    }

    pub fn approve_task(&self, task_id: &str) -> Result<crate::task_engine::TaskSnapshot, String> {
        if !self.auth.is_enabled() {
            return Err("MCP is disabled".into());
        }
        let snapshot = self
            .tasks
            .internal_snapshot(task_id)
            .ok_or("unknown task")?;
        let record = self
            .registry
            .get(&snapshot.connection_id)
            .ok_or("connection is gone")?;
        let grant = self
            .auth
            .current_grant(
                &snapshot.client_id,
                &snapshot.connection_id,
                record.generation,
                GrantPermission::Execute,
            )
            .map_err(|error| format!("approval grant is invalid: {error:?}"))?;
        if record.generation != snapshot.generation
            || !self
                .tasks
                .grant_matches_task(&snapshot.client_id, task_id, &grant)
        {
            self.tasks
                .cancel_for_client(&snapshot.client_id, task_id)
                .map_err(|_| {
                    "task cancellation could not be recorded in the audit ledger".to_string()
                })?;
            return Err("approval grant no longer matches this task".into());
        }
        let approved = self
            .tasks
            .approve(task_id, record.generation)
            .map_err(|error| {
                if error == crate::task_engine::ApprovalError::AuditUnavailable {
                    "approval could not be recorded in the audit ledger; task remains pending"
                        .into()
                } else if error == crate::task_engine::ApprovalError::Expired {
                    "approval expired after 5 minutes".into()
                } else if error == crate::task_engine::ApprovalError::ConnectionBusy {
                    "another queued or running command already owns this connection".into()
                } else {
                    format!("approval failed: {error:?}")
                }
            })?;
        if approved.status == TaskStatus::Queued {
            self.try_start_task(task_id)?;
        }
        Ok(self.tasks.internal_snapshot(task_id).unwrap_or(approved))
    }
}

fn ipc_credentials(request: &IpcRequest) -> (&str, &str) {
    match request {
        IpcRequest::ListConnections { client_id, token }
        | IpcRequest::ReadConnectionOutput {
            client_id, token, ..
        }
        | IpcRequest::RunCommand {
            client_id, token, ..
        }
        | IpcRequest::ReadTask {
            client_id, token, ..
        }
        | IpcRequest::CancelTask {
            client_id, token, ..
        } => (client_id, token),
    }
}

fn task_status_name(status: TaskStatus) -> &'static str {
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

// ---------------------------------------------------------------------------
// Local unix-socket IPC server
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
enum FrameRead {
    Eof,
    Frame(Vec<u8>),
    TooLarge,
}

/// Read one newline-delimited frame while keeping the retained frame bounded.
///
/// `BufRead::read_line` grows its destination until it sees a newline. That
/// is unsafe at an IPC boundary because an authenticated client can keep a
/// connection open while sending an arbitrarily large unterminated line. We
/// retain at most `MAX_IPC_LINE_BYTES`; once that boundary is crossed, the
/// remaining bytes are consumed only until the frame delimiter (or EOF) so a
/// following frame cannot be parsed as a continuation of the oversized one.
fn read_frame<R: BufRead>(reader: &mut R) -> io::Result<FrameRead> {
    let mut frame = Vec::with_capacity(MAX_IPC_LINE_BYTES);

    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            return Ok(if frame.is_empty() {
                FrameRead::Eof
            } else {
                FrameRead::Frame(frame)
            });
        }

        if let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
            let end = newline + 1;
            if frame.len().saturating_add(end) > MAX_IPC_LINE_BYTES {
                reader.consume(end);
                return Ok(FrameRead::TooLarge);
            }
            frame.extend_from_slice(&buffer[..end]);
            reader.consume(end);
            return Ok(FrameRead::Frame(frame));
        }

        let remaining = MAX_IPC_LINE_BYTES - frame.len();
        if buffer.len() <= remaining {
            let length = buffer.len();
            frame.extend_from_slice(buffer);
            reader.consume(length);
            continue;
        }

        // Retain exactly the remaining budget, then drain the rest of this
        // buffered chunk without copying it. The helper continues draining
        // future chunks until the frame delimiter or EOF.
        frame.extend_from_slice(&buffer[..remaining]);
        let rest = &buffer[remaining..];
        let consumed_rest = rest
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|newline| newline + 1)
            .unwrap_or(rest.len());
        let found_delimiter = consumed_rest < rest.len();
        reader.consume(remaining + consumed_rest);
        if !found_delimiter {
            drain_oversized_frame(reader)?;
        }
        return Ok(FrameRead::TooLarge);
    }
}

fn drain_oversized_frame<R: BufRead>(reader: &mut R) -> io::Result<()> {
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            return Ok(());
        }
        if let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
            reader.consume(newline + 1);
            return Ok(());
        }
        let length = buffer.len();
        reader.consume(length);
    }
}

struct IpcConnectionLimiter {
    active: Arc<AtomicUsize>,
    limit: usize,
}

struct IpcConnectionPermit {
    active: Arc<AtomicUsize>,
}

impl IpcConnectionLimiter {
    fn new(limit: usize) -> Self {
        assert!(limit > 0, "the IPC connection limit must be positive");
        Self {
            active: Arc::new(AtomicUsize::new(0)),
            limit,
        }
    }

    fn try_acquire(&self) -> Option<IpcConnectionPermit> {
        let mut current = self.active.load(Ordering::Acquire);
        loop {
            if current >= self.limit {
                return None;
            }
            match self.active.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return Some(IpcConnectionPermit {
                        active: Arc::clone(&self.active),
                    })
                }
                Err(observed) => current = observed,
            }
        }
    }
}

impl Drop for IpcConnectionPermit {
    fn drop(&mut self) {
        let previous = self.active.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "IPC connection permit count underflowed");
    }
}

fn peer_uid_allowed(peer_uid: u32, effective_uid: u32) -> bool {
    peer_uid == effective_uid
}

#[cfg(target_os = "macos")]
fn peer_uid_matches_effective_user(stream: &UnixStream) -> io::Result<bool> {
    use std::os::fd::AsRawFd;

    unsafe extern "C" {
        fn getpeereid(socket: i32, effective_uid: *mut u32, effective_gid: *mut u32) -> i32;
        fn geteuid() -> u32;
    }

    let mut peer_uid = 0;
    let mut peer_gid = 0;
    let result = unsafe { getpeereid(stream.as_raw_fd(), &mut peer_uid, &mut peer_gid) };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    let effective_uid = unsafe { geteuid() };
    Ok(peer_uid_allowed(peer_uid, effective_uid))
}

#[cfg(not(target_os = "macos"))]
fn peer_uid_matches_effective_user(_stream: &UnixStream) -> io::Result<bool> {
    // Linux and the other Unix targets already expose the socket below a
    // private 0700 directory and 0600 node. Keep this helper available on
    // those targets so the accept path and its tests stay cross-platform;
    // macOS additionally verifies the peer credential with getpeereid.
    Ok(true)
}

fn is_stale_socket_connect_error(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
    )
}

fn bind_ipc_listener(path: &Path) -> Result<UnixListener, String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_socket() {
                return Err(format!(
                    "IPC socket path is occupied by a non-socket: {}",
                    path.display()
                ));
            }
            match UnixStream::connect(path) {
                Ok(_) => {
                    return Err(format!(
                        "AlreadyRunning: MCP IPC server is already running at {}",
                        path.display()
                    ));
                }
                Err(error) if is_stale_socket_connect_error(&error) => {
                    match std::fs::remove_file(path) {
                        Ok(()) => {}
                        Err(remove_error) if remove_error.kind() == io::ErrorKind::NotFound => {}
                        Err(remove_error) => {
                            return Err(format!(
                                "stale IPC socket could not be removed: {remove_error}"
                            ));
                        }
                    }
                }
                Err(error) => {
                    return Err(format!(
                        "could not determine whether IPC socket is stale: {error}"
                    ));
                }
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!("could not inspect IPC socket path: {error}"));
        }
    }

    UnixListener::bind(path).map_err(|error| error.to_string())
}

/// Default socket location: `<app-config>/mcp/bridge.sock`.
pub fn default_socket_path() -> PathBuf {
    crate::ssh_config::get_ssh_config_path()
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("mcp")
        .join("bridge.sock")
}

pub fn start_ipc_server(service: Arc<McpService>) -> Result<PathBuf, String> {
    start_ipc_server_at(service, default_socket_path())
}

pub fn start_ipc_server_at(service: Arc<McpService>, path: PathBuf) -> Result<PathBuf, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| e.to_string())?;
        }
    }
    let listener = bind_ipc_listener(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
    }

    let limiter = Arc::new(IpcConnectionLimiter::new(MAX_ACTIVE_IPC_CONNECTIONS));
    let socket_path = path.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    match peer_uid_matches_effective_user(&stream) {
                        Ok(true) => {}
                        Ok(false) => {
                            eprintln!("[mcp] rejecting IPC peer with a different uid");
                            drop(stream);
                            continue;
                        }
                        Err(error) => {
                            eprintln!("[mcp] unable to verify IPC peer uid: {error}");
                            drop(stream);
                            continue;
                        }
                    }
                    let Some(permit) = limiter.try_acquire() else {
                        // The cap is deliberately fail-closed. In
                        // particular, do not spawn a thread just to tell a
                        // client that the server is busy.
                        drop(stream);
                        continue;
                    };
                    let service = Arc::clone(&service);
                    std::thread::spawn(move || handle_client(service, stream, permit));
                }
                Err(error) => {
                    eprintln!("[mcp] ipc accept failed: {error}");
                }
            }
        }
    });
    Ok(socket_path)
}

fn handle_client(service: Arc<McpService>, stream: UnixStream, _permit: IpcConnectionPermit) {
    let reader_stream = match stream.try_clone() {
        Ok(clone) => clone,
        Err(_) => return,
    };
    let mut reader = BufReader::new(reader_stream);
    let mut writer = stream;
    let mut bound_client_id: Option<String> = None;
    loop {
        match read_frame(&mut reader) {
            Ok(FrameRead::Eof) => break, // client session ended
            Ok(FrameRead::TooLarge) => {
                if write_response(
                    &mut writer,
                    &IpcResponse::error("too_large", "request exceeds line budget"),
                )
                .is_err()
                {
                    break;
                }
            }
            Ok(FrameRead::Frame(frame)) => {
                let request: IpcRequest = match serde_json::from_slice(&frame) {
                    Ok(request) => request,
                    Err(_) => {
                        if write_response(
                            &mut writer,
                            &IpcResponse::error("bad_request", "invalid request frame"),
                        )
                        .is_err()
                        {
                            break;
                        }
                        continue;
                    }
                };

                let (client_id, token) = ipc_credentials(&request);
                if let Some(bound) = bound_client_id.as_deref() {
                    if bound != client_id {
                        if write_response(
                            &mut writer,
                            &IpcResponse::error(
                                "client_mismatch",
                                "an IPC transport cannot switch client identity",
                            ),
                        )
                        .is_err()
                        {
                            break;
                        }
                        continue;
                    }
                } else if client_id.len() <= MAX_IPC_CLIENT_ID_BYTES
                    && token.len() <= MAX_IPC_TOKEN_BYTES
                    && service.auth.authenticate(client_id, token).is_ok()
                {
                    bound_client_id = Some(client_id.to_string());
                }

                let response = service.handle(request);
                if write_response(&mut writer, &response).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    if let Some(client_id) = bound_client_id {
        service.on_client_transport_closed(&client_id);
    }
}

fn write_response(writer: &mut UnixStream, response: &IpcResponse) -> std::io::Result<()> {
    let mut line = serde_json::to_string(response).unwrap_or_else(|_| {
        "{\"kind\":\"error\",\"code\":\"internal\",\"message\":\"encode failed\"}".to_string()
    });
    line.push('\n');
    writer.write_all(line.as_bytes())?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection_registry::HostSnapshot;
    use std::path::PathBuf;

    fn snapshot() -> HostSnapshot {
        HostSnapshot {
            host_id: "h1".into(),
            alias: "prod".into(),
            hostname: "example.com".into(),
            user: "root".into(),
            port: 22,
            proxy_jump: None,
            has_identity_file: false,
            encoding: None,
        }
    }

    fn service_with_connection() -> (Arc<McpService>, String, u64) {
        let mux = Arc::new(MuxManager::for_test(
            std::env::temp_dir().join(format!("xt-mux-test-{}", std::process::id())),
            PathBuf::from("/tmp/ssh_config"),
        ));
        let service = Arc::new(McpService::new(mux));
        service.set_enabled(true).unwrap();
        let record = service
            .registry
            .register(
                uuid::Uuid::new_v4().to_string(),
                "sess-1".to_string(),
                snapshot(),
                Some(PathBuf::from("/tmp/mux.sock")),
            )
            .unwrap();
        service.registry.mark_ready(&record.connection_id).unwrap();
        (service, record.connection_id, record.generation)
    }

    fn short_ipc_test_path(prefix: &str) -> (PathBuf, PathBuf) {
        let directory = PathBuf::from(format!(
            "/private/tmp/xt-{}-{}-{}",
            prefix,
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("bridge.sock");
        assert!(path.as_os_str().len() < 100);
        (directory, path)
    }

    fn grant_execution(
        service: &McpService,
        client_id: &str,
        connection_id: &str,
        generation: u64,
        grant_start_seq: u64,
    ) {
        service
            .auth
            .set_grant_permissions(
                client_id,
                connection_id,
                generation,
                grant_start_seq,
                GrantPermissions {
                    observe: true,
                    execute: true,
                },
            )
            .expect("permission write should succeed")
            .expect("client should be paired");
    }

    #[test]
    fn ipc_transport_eof_revokes_grants_cancels_tasks_and_keeps_pairing() {
        let (service, connection_id, generation) = service_with_connection();
        let (client_id, token) = service.auth.pair_client(None).unwrap();
        grant_execution(&service, &client_id, &connection_id, generation, 0);
        service.buffers.push(&connection_id, "buffered".into());
        let accepted = service.handle(IpcRequest::RunCommand {
            client_id: client_id.clone(),
            token: token.clone(),
            connection_id: connection_id.clone(),
            request_id: "transport-eof-task".into(),
            command: "id".into(),
            working_directory: "login".into(),
            timeout_seconds: 300,
        });
        let IpcResponse::RunAccepted(run) = accepted else {
            panic!("expected pending task");
        };

        let (server_stream, mut client_stream) = UnixStream::pair().unwrap();
        let limiter = IpcConnectionLimiter::new(1);
        let permit = limiter.try_acquire().unwrap();
        let server_service = Arc::clone(&service);
        let server =
            std::thread::spawn(move || handle_client(server_service, server_stream, permit));
        let request = serde_json::to_string(&IpcRequest::ListConnections {
            client_id: client_id.clone(),
            token: token.clone(),
        })
        .unwrap();
        client_stream.write_all(request.as_bytes()).unwrap();
        client_stream.write_all(b"\n").unwrap();
        client_stream.flush().unwrap();
        let mut response = String::new();
        BufReader::new(client_stream.try_clone().unwrap())
            .read_line(&mut response)
            .unwrap();
        assert!(response.contains("connections"));
        drop(client_stream);
        server.join().unwrap();

        assert!(service
            .auth
            .authorize_grant(&client_id, &token, &connection_id, generation)
            .is_err());
        assert_eq!(
            service
                .tasks
                .internal_snapshot(&run.task_id)
                .unwrap()
                .status,
            TaskStatus::Cancelled
        );
        assert!(service.buffers.read(&connection_id, 0, 1024).is_none());
        assert!(service.auth.authenticate(&client_id, &token).is_ok());
    }

    #[test]
    fn run_command_rejects_empty_request_id_before_creating_a_task() {
        let (service, connection_id, generation) = service_with_connection();
        let (client_id, token) = service.auth.pair_client(None).unwrap();
        grant_execution(&service, &client_id, &connection_id, generation, 0);

        let response = service.handle(IpcRequest::RunCommand {
            client_id,
            token,
            connection_id,
            request_id: String::new(),
            command: "uptime".into(),
            working_directory: "login".into(),
            timeout_seconds: 300,
        });

        assert!(matches!(
            response,
            IpcResponse::Error { ref code, .. } if code == "invalid_request_id"
        ));
        assert!(service.tasks.pending_approvals().unwrap().is_empty());
    }

    #[test]
    fn read_task_wire_response_exposes_independent_stdout_and_stderr_fields() {
        let (service, connection_id, generation) = service_with_connection();
        let (client_id, token) = service.auth.pair_client(None).unwrap();
        grant_execution(&service, &client_id, &connection_id, generation, 0);

        let accepted = service.handle(IpcRequest::RunCommand {
            client_id: client_id.clone(),
            token: token.clone(),
            connection_id,
            request_id: "wire-streams".into(),
            command: "printf".into(),
            working_directory: "login".into(),
            timeout_seconds: 300,
        });
        let IpcResponse::RunAccepted(run) = accepted else {
            panic!("expected run accepted");
        };

        let response = service.handle(IpcRequest::ReadTask {
            client_id,
            token,
            task_id: run.task_id,
            stdout_offset: None,
            stderr_offset: None,
            output_offset: None,
        });
        let encoded = serde_json::to_value(response).unwrap();
        assert!(encoded["stdout"].is_string());
        assert!(encoded["stderr"].is_string());
        assert!(encoded["nextStdoutOffset"].is_number());
        assert!(encoded["nextStderrOffset"].is_number());
        assert!(encoded["stdoutGap"].is_boolean());
        assert!(encoded["stderrGap"].is_boolean());
    }

    #[test]
    fn mux_verification_failure_terminalizes_approved_task_and_releases_slot() {
        let (service, connection_id, generation) = service_with_connection();
        let (client_id, token) = service.auth.pair_client(None).unwrap();
        grant_execution(&service, &client_id, &connection_id, generation, 0);
        let response = service.handle(IpcRequest::RunCommand {
            client_id,
            token,
            connection_id: connection_id.clone(),
            request_id: "verify-failure".into(),
            command: "id".into(),
            working_directory: "login".into(),
            timeout_seconds: 300,
        });
        let IpcResponse::RunAccepted(run) = response else {
            panic!("task should be accepted pending approval");
        };

        let approval = service.approve_task(&run.task_id);
        assert!(
            approval.is_err(),
            "failed mux verification must be reported"
        );
        let task = service.tasks.internal_snapshot(&run.task_id).unwrap();
        assert_eq!(task.status, TaskStatus::Failed);
        assert!(task
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("mux verification")));
        assert_eq!(service.tasks.running_for_connection(&connection_id), None);
    }

    #[test]
    fn run_command_rejects_invalid_utf8_byte_lengths_and_execution_options() {
        let (service, connection_id, generation) = service_with_connection();
        let (client_id, token) = service.auth.pair_client(None).unwrap();
        grant_execution(&service, &client_id, &connection_id, generation, 0);
        let submit =
            |request_id: String, command: String, working_directory: &str, timeout_seconds: u64| {
                service.handle(IpcRequest::RunCommand {
                    client_id: client_id.clone(),
                    token: token.clone(),
                    connection_id: connection_id.clone(),
                    request_id,
                    command,
                    working_directory: working_directory.to_string(),
                    timeout_seconds,
                })
            };

        let invalid = [
            (
                submit("r".repeat(129), "id".into(), "login", 300),
                "invalid_request_id",
            ),
            (
                submit("r".into(), String::new(), "login", 300),
                "invalid_command",
            ),
            (
                submit("r".into(), "é".repeat(8193), "login", 300),
                "invalid_command",
            ),
            (
                submit("r".into(), "id".into(), "working", 300),
                "invalid_working_directory",
            ),
            (
                submit("r".into(), "id".into(), "login", 0),
                "invalid_timeout",
            ),
            (
                submit("r".into(), "id".into(), "login", 1801),
                "invalid_timeout",
            ),
        ];
        for (response, expected_code) in invalid {
            assert!(matches!(
                response,
                IpcResponse::Error { ref code, .. } if code == expected_code
            ));
        }
        assert!(service.tasks.pending_approvals().unwrap().is_empty());

        let accepted = submit("r".repeat(128), "x".repeat(16 * 1024), "login", 1800);
        let IpcResponse::RunAccepted(run) = accepted else {
            panic!("the documented upper input boundaries should be accepted");
        };
        let task = service.tasks.internal_snapshot(&run.task_id).unwrap();
        assert_eq!(task.command.len(), 16 * 1024);
        assert_eq!(task.working_directory, "login");
        assert_eq!(task.timeout_seconds, 1800);
    }

    #[test]
    fn ipc_frame_reader_rejects_oversized_frames_without_losing_next_frame() {
        let valid = br#"{"method":"list_connections","params":{"client_id":"c","token":"t"}}"#;
        let mut input = vec![b'x'; MAX_IPC_LINE_BYTES + 1];
        input.push(b'\n');
        input.extend_from_slice(valid);
        input.push(b'\n');
        let mut reader = BufReader::new(std::io::Cursor::new(input));

        assert!(matches!(
            read_frame(&mut reader).unwrap(),
            FrameRead::TooLarge
        ));
        let FrameRead::Frame(frame) = read_frame(&mut reader).unwrap() else {
            panic!("the frame after an oversized request must remain readable");
        };
        assert_eq!(
            frame,
            valid.iter().copied().chain([b'\n']).collect::<Vec<_>>()
        );
        assert!(frame.capacity() <= MAX_IPC_LINE_BYTES);
    }

    #[test]
    fn ipc_frame_reader_rejects_oversized_unterminated_frame_at_eof() {
        let input = vec![b'x'; MAX_IPC_LINE_BYTES + 1];
        let mut reader = BufReader::new(std::io::Cursor::new(input));
        assert!(matches!(
            read_frame(&mut reader).unwrap(),
            FrameRead::TooLarge
        ));
        assert!(matches!(read_frame(&mut reader).unwrap(), FrameRead::Eof));
    }

    #[test]
    fn ipc_connection_limiter_is_raii_bounded() {
        let limiter = IpcConnectionLimiter::new(2);
        let first = limiter.try_acquire().expect("first connection slot");
        let second = limiter.try_acquire().expect("second connection slot");
        assert!(limiter.try_acquire().is_none());
        drop(first);
        assert!(limiter.try_acquire().is_some());
        drop(second);
    }

    #[test]
    fn authenticated_client_rate_limit_rejects_request_61_and_recovers_after_window() {
        let mux = Arc::new(MuxManager::for_test(
            std::env::temp_dir().join(format!("xt-mux-rate-{}", uuid::Uuid::new_v4())),
            PathBuf::from("/tmp/ssh_config"),
        ));
        let service = McpService::new(mux);
        let (client_id, token) = service.auth.pair_client(None).unwrap();
        let request = IpcRequest::ListConnections { client_id, token };
        let base = std::time::Instant::now();
        for _ in 0..MAX_IPC_REQUESTS_PER_SECOND_PER_CLIENT {
            assert!(service.check_ipc_rate_at(&request, base).is_none());
        }
        assert!(matches!(
            service.check_ipc_rate_at(&request, base),
            Some(IpcResponse::Error { ref code, .. }) if code == "rate_limited"
        ));
        assert!(service
            .check_ipc_rate_at(&request, base + std::time::Duration::from_secs(1))
            .is_none());
    }

    #[test]
    fn unknown_or_bad_tokens_count_global_only_and_do_not_grow_client_map() {
        let mux = Arc::new(MuxManager::for_test(
            std::env::temp_dir().join(format!("xt-mux-rate-unknown-{}", uuid::Uuid::new_v4())),
            PathBuf::from("/tmp/ssh_config"),
        ));
        let service = McpService::new(mux);
        let (paired_id, _) = service.auth.pair_client(None).unwrap();
        let base = std::time::Instant::now();
        for index in 0..MAX_IPC_REQUESTS_PER_SECOND_GLOBAL {
            let request = IpcRequest::ListConnections {
                client_id: if index % 2 == 0 {
                    format!("unknown-{index}")
                } else {
                    paired_id.clone()
                },
                token: "bad-token".into(),
            };
            assert!(service.check_ipc_rate_at(&request, base).is_none());
        }
        assert_eq!(service.ipc_rate_limiter.tracked_client_count(), 0);
        let request = IpcRequest::ListConnections {
            client_id: "unknown-final".into(),
            token: "bad-token".into(),
        };
        assert!(matches!(
            service.check_ipc_rate_at(&request, base),
            Some(IpcResponse::Error { ref code, .. }) if code == "rate_limited"
        ));
        assert_eq!(service.ipc_rate_limiter.tracked_client_count(), 0);
    }

    #[test]
    fn ipc_rejects_oversized_client_credentials_before_authentication() {
        let mux = Arc::new(MuxManager::for_test(
            std::env::temp_dir().join(format!("xt-mux-input-limits-{}", uuid::Uuid::new_v4())),
            PathBuf::from("/tmp/ssh_config"),
        ));
        let service = McpService::new(mux);
        let response = service.handle(IpcRequest::ListConnections {
            client_id: "c".repeat(MAX_IPC_CLIENT_ID_BYTES + 1),
            token: "t".into(),
        });
        assert!(matches!(
            response,
            IpcResponse::Error { ref code, .. } if code == "invalid_client_id"
        ));
        let response = service.handle(IpcRequest::ListConnections {
            client_id: "c".into(),
            token: "t".repeat(MAX_IPC_TOKEN_BYTES + 1),
        });
        assert!(matches!(
            response,
            IpcResponse::Error { ref code, .. } if code == "invalid_token"
        ));
    }

    #[test]
    fn second_ipc_listener_is_rejected_without_destroying_first_listener() {
        let (directory, path) = short_ipc_test_path("mcp-ipc");
        let mux = Arc::new(MuxManager::for_test(
            std::env::temp_dir().join(format!("xt-mux-ipc-{}", uuid::Uuid::new_v4())),
            PathBuf::from("/tmp/ssh_config"),
        ));
        let service = Arc::new(McpService::new(mux));
        start_ipc_server_at(Arc::clone(&service), path.clone()).unwrap();
        let second = start_ipc_server_at(Arc::clone(&service), path.clone());

        let mut stream = UnixStream::connect(&path).unwrap();
        stream
            .write_all(
                br#"{"method":"list_connections","params":{"client_id":"c","token":"t"}}
"#,
            )
            .unwrap();
        let mut response = String::new();
        BufReader::new(stream).read_line(&mut response).unwrap();
        assert!(response.contains("\"code\":\"disabled\""));

        if path.exists() {
            std::fs::remove_file(&path).unwrap();
        }
        let error = second.expect_err("a second listener must be refused");
        assert!(
            error.contains("AlreadyRunning"),
            "unexpected error: {error}"
        );
        std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn stale_ipc_socket_is_removed_and_rebound() {
        let (directory, path) = short_ipc_test_path("mcp-stale");
        let stale_listener = UnixListener::bind(&path).unwrap();
        drop(stale_listener);
        assert!(path.exists());

        let mux = Arc::new(MuxManager::for_test(
            std::env::temp_dir().join(format!("xt-mux-stale-{}", uuid::Uuid::new_v4())),
            PathBuf::from("/tmp/ssh_config"),
        ));
        let service = Arc::new(McpService::new(mux));
        start_ipc_server_at(service, path.clone()).expect("stale socket should be recoverable");
        let connected = UnixStream::connect(&path).is_ok();
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
        assert!(connected);
    }

    #[test]
    fn ordinary_file_at_ipc_path_is_not_removed() {
        let (directory, path) = short_ipc_test_path("mcp-file");
        std::fs::write(&path, b"keep me").unwrap();
        let mux = Arc::new(MuxManager::for_test(
            std::env::temp_dir().join(format!("xt-mux-file-{}", uuid::Uuid::new_v4())),
            PathBuf::from("/tmp/ssh_config"),
        ));
        let service = Arc::new(McpService::new(mux));
        let error = start_ipc_server_at(service, path.clone()).unwrap_err();
        assert!(error.contains("socket path"), "unexpected error: {error}");
        assert_eq!(std::fs::read(&path).unwrap(), b"keep me");
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn peer_uid_policy_is_strictly_equal() {
        assert!(peer_uid_allowed(501, 501));
        assert!(!peer_uid_allowed(501, 502));
    }

    #[cfg(unix)]
    #[test]
    fn unix_peer_uid_guard_accepts_current_peer() {
        let (stream, _peer) = UnixStream::pair().unwrap();
        assert!(peer_uid_matches_effective_user(&stream).unwrap());
    }

    #[test]
    fn request_id_is_not_replayed_after_reopening_the_persistent_database() {
        let config_dir =
            std::env::temp_dir().join(format!("xt-mcp-restart-ledger-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&config_dir).unwrap();
        let db_path = config_dir.join("mcp.db");

        let first_auth = AuthState::open(&db_path).unwrap();
        first_auth.set_enabled(true).unwrap();
        let first_mux = Arc::new(MuxManager::for_test(
            config_dir.join("mux-first"),
            PathBuf::from("/tmp/ssh_config"),
        ));
        let first_audit = Arc::new(TaskAuditStore::open(&db_path).unwrap());
        let first_service = Arc::new(McpService::with_persistent_state(
            first_mux,
            first_auth,
            first_audit,
        ));
        let first_record = first_service
            .registry
            .register(
                uuid::Uuid::new_v4().to_string(),
                "sess-restart-first".into(),
                snapshot(),
                Some(config_dir.join("mux-first.sock")),
            )
            .unwrap();
        first_service
            .registry
            .mark_ready(&first_record.connection_id)
            .unwrap();
        let (client_id, token) = first_service.auth.pair_client(None).unwrap();
        grant_execution(
            &first_service,
            &client_id,
            &first_record.connection_id,
            first_record.generation,
            0,
        );

        let first_response = first_service.handle(IpcRequest::RunCommand {
            client_id: client_id.clone(),
            token: token.clone(),
            connection_id: first_record.connection_id.clone(),
            request_id: "restart-ledger-request".into(),
            command: "uptime".into(),
            working_directory: "login".into(),
            timeout_seconds: 300,
        });
        let IpcResponse::RunAccepted(first_run) = first_response else {
            panic!("first request should be accepted");
        };
        let same_session_retry = first_service.handle(IpcRequest::RunCommand {
            client_id: client_id.clone(),
            token: token.clone(),
            connection_id: first_record.connection_id.clone(),
            request_id: "restart-ledger-request".into(),
            command: "uptime".into(),
            working_directory: "login".into(),
            timeout_seconds: 300,
        });
        assert!(matches!(
            same_session_retry,
            IpcResponse::RunAccepted(ref run) if run.task_id == first_run.task_id
        ));
        drop(first_service);

        let second_auth = AuthState::open(&db_path).unwrap();
        let second_mux = Arc::new(MuxManager::for_test(
            config_dir.join("mux-second"),
            PathBuf::from("/tmp/ssh_config"),
        ));
        let second_audit = Arc::new(TaskAuditStore::open(&db_path).unwrap());
        let second_service = Arc::new(McpService::with_persistent_state(
            second_mux,
            second_auth,
            second_audit,
        ));
        let second_record = second_service
            .registry
            .register(
                uuid::Uuid::new_v4().to_string(),
                "sess-restart-second".into(),
                snapshot(),
                Some(config_dir.join("mux-second.sock")),
            )
            .unwrap();
        second_service
            .registry
            .mark_ready(&second_record.connection_id)
            .unwrap();
        grant_execution(
            &second_service,
            &client_id,
            &second_record.connection_id,
            second_record.generation,
            0,
        );

        let second_response = second_service.handle(IpcRequest::RunCommand {
            client_id: client_id.clone(),
            token: token.clone(),
            connection_id: second_record.connection_id.clone(),
            request_id: "restart-ledger-request".into(),
            command: "uptime".into(),
            working_directory: "login".into(),
            timeout_seconds: 300,
        });
        assert!(matches!(
            second_response,
            IpcResponse::Error { ref code, .. } if code == "request_from_previous_session"
        ));

        let changed_request_response = second_service.handle(IpcRequest::RunCommand {
            client_id,
            token,
            connection_id: second_record.connection_id.clone(),
            request_id: "restart-ledger-request".into(),
            command: "whoami".into(),
            working_directory: "login".into(),
            timeout_seconds: 300,
        });
        assert!(matches!(
            changed_request_response,
            IpcResponse::Error { ref code, .. } if code == "request_from_previous_session"
        ));

        drop(second_service);
        std::fs::remove_dir_all(config_dir).unwrap();
    }

    #[test]
    fn agent_ipc_is_explicitly_disabled_by_default() {
        let mux = Arc::new(MuxManager::for_test(
            std::env::temp_dir().join(format!("xt-mux-disabled-{}", std::process::id())),
            PathBuf::from("/tmp/ssh_config"),
        ));
        let service = McpService::new(mux);
        let requests = [
            IpcRequest::ListConnections {
                client_id: "client".into(),
                token: "token".into(),
            },
            IpcRequest::ReadConnectionOutput {
                client_id: "client".into(),
                token: "token".into(),
                connection_id: "connection".into(),
                cursor: 0,
                max_chars: None,
            },
            IpcRequest::RunCommand {
                client_id: "client".into(),
                token: "token".into(),
                connection_id: "connection".into(),
                request_id: "request".into(),
                command: "id".into(),
                working_directory: "login".into(),
                timeout_seconds: 300,
            },
            IpcRequest::ReadTask {
                client_id: "client".into(),
                token: "token".into(),
                task_id: "task".into(),
                stdout_offset: None,
                stderr_offset: None,
                output_offset: None,
            },
            IpcRequest::CancelTask {
                client_id: "client".into(),
                token: "token".into(),
                task_id: "task".into(),
            },
        ];

        for request in requests {
            let response = service.handle(request);
            assert!(
                matches!(response, IpcResponse::Error { ref code, .. } if code == "disabled"),
                "expected disabled response, received {response:?}"
            );
        }
    }

    #[test]
    fn disabling_mcp_cancels_pending_tasks_and_blocks_agent_ipc() {
        let (service, connection_id, generation) = service_with_connection();
        let (client_id, token) = service.auth.pair_client(None).unwrap();
        grant_execution(&service, &client_id, &connection_id, generation, 0);
        let accepted = service.handle(IpcRequest::RunCommand {
            client_id: client_id.clone(),
            token: token.clone(),
            connection_id: connection_id.clone(),
            request_id: "disable-r1".into(),
            command: "uptime".into(),
            working_directory: "login".into(),
            timeout_seconds: 300,
        });
        let IpcResponse::RunAccepted(run) = accepted else {
            panic!("expected run accepted");
        };

        service.set_enabled(false).unwrap();

        assert_eq!(
            service
                .tasks
                .internal_snapshot(&run.task_id)
                .unwrap()
                .status,
            TaskStatus::Cancelled
        );
        assert!(matches!(
            service.handle(IpcRequest::ListConnections { client_id, token }),
            IpcResponse::Error { ref code, .. } if code == "disabled"
        ));
    }

    #[test]
    fn disable_storage_failure_still_cancels_tasks_and_fails_closed() {
        let config_dir =
            std::env::temp_dir().join(format!("xt-mcp-disable-failure-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&config_dir).unwrap();
        let auth = AuthState::open(&config_dir.join("mcp.db")).unwrap();
        auth.set_enabled(true).unwrap();
        let mux = Arc::new(MuxManager::for_test(
            config_dir.join("mux"),
            PathBuf::from("/tmp/ssh_config"),
        ));
        let service = Arc::new(McpService::with_auth(mux, auth));
        let record = service
            .registry
            .register(
                uuid::Uuid::new_v4().to_string(),
                "sess-disable-failure".into(),
                snapshot(),
                Some(PathBuf::from("/tmp/mux.sock")),
            )
            .unwrap();
        service.registry.mark_ready(&record.connection_id).unwrap();
        let (client_id, token) = service.auth.pair_client(None).unwrap();
        grant_execution(
            &service,
            &client_id,
            &record.connection_id,
            record.generation,
            0,
        );
        let accepted = service.handle(IpcRequest::RunCommand {
            client_id: client_id.clone(),
            token,
            connection_id: record.connection_id.clone(),
            request_id: "disable-failure".into(),
            command: "id".into(),
            working_directory: "login".into(),
            timeout_seconds: 300,
        });
        let IpcResponse::RunAccepted(run) = accepted else {
            panic!("expected run accepted");
        };
        rusqlite::Connection::open(config_dir.join("mcp.db"))
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER reject_disable BEFORE UPDATE ON mcp_settings
                 BEGIN SELECT RAISE(ABORT, 'disable write rejected'); END;",
            )
            .unwrap();

        assert!(service.set_enabled(false).is_err());
        assert!(!service.auth.is_enabled());
        assert_eq!(
            service
                .tasks
                .internal_snapshot(&run.task_id)
                .unwrap()
                .status,
            TaskStatus::Cancelled
        );
        assert!(matches!(
            service.handle(IpcRequest::ListConnections { client_id, token: "token".into() }),
            IpcResponse::Error { ref code, .. } if code == "disabled"
        ));
    }

    #[test]
    fn pty_output_is_captured_only_for_enabled_observers() {
        let (service, connection_id, generation) = service_with_connection();
        service.capture_pty_output(&connection_id, "no observer".into());
        assert!(service.buffers.read(&connection_id, 0, 1024).is_none());

        let (client_id, _) = service.auth.pair_client(None).unwrap();
        service
            .auth
            .grant_connection(&client_id, &connection_id, generation, 0)
            .unwrap();
        service.capture_pty_output(&connection_id, "visible".into());
        let read = service.buffers.read(&connection_id, 0, 1024).unwrap();
        assert_eq!(
            read.chunks
                .iter()
                .map(|chunk| chunk.data.as_str())
                .collect::<String>(),
            "visible"
        );
    }

    #[test]
    fn pty_capture_skips_a_busy_access_gate_without_blocking_and_reports_gap() {
        let (service, connection_id, _) = service_with_connection();
        let (client_id, _) = service.auth.pair_client(None).unwrap();
        service
            .set_client_connection_permissions(
                &client_id,
                &connection_id,
                GrantPermissions::observe_only(),
            )
            .unwrap();
        let cursor = service.buffers.current_seq();
        let gate = service.access_gate.write().unwrap();
        let capture_service = Arc::clone(&service);
        let capture_connection_id = connection_id.clone();
        let (done_tx, done_rx) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            capture_service
                .capture_pty_output(&capture_connection_id, "skipped while gated".into());
            let _ = done_tx.send(());
        });

        assert!(done_rx.recv_timeout(Duration::from_millis(100)).is_ok());
        drop(gate);

        let read = service
            .buffers
            .read(&connection_id, cursor, MCP_READ_MAX_CHARS)
            .expect("capture gap retains a cursor endpoint");
        assert!(read.gap_from_seq.is_some());
        assert!(read.chunks.is_empty());
        assert_eq!(
            service.buffers.current_seq(),
            cursor + "skipped while gated".len() as u64
        );
    }

    #[test]
    fn disabling_mcp_clears_existing_output_buffers() {
        let (service, connection_id, generation) = service_with_connection();
        let (client_id, _) = service.auth.pair_client(None).unwrap();
        service
            .auth
            .grant_connection(&client_id, &connection_id, generation, 0)
            .unwrap();
        service.capture_pty_output(&connection_id, "observed".into());
        assert!(service.buffers.read(&connection_id, 0, 1024).is_some());

        service.set_enabled(false).unwrap();
        service.capture_pty_output(&connection_id, "after disable".into());
        assert!(service.buffers.read(&connection_id, 0, 1024).is_none());
    }

    #[test]
    fn system_session_resignation_revokes_grants_cancels_tasks_and_keeps_pairings() {
        let (service, first_connection_id, first_generation) = service_with_connection();
        let second = service
            .registry
            .register(
                uuid::Uuid::new_v4().to_string(),
                "sess-2".into(),
                snapshot(),
                Some(PathBuf::from("/tmp/mux-2.sock")),
            )
            .unwrap();
        service.registry.mark_ready(&second.connection_id).unwrap();

        let (first_client, first_token) = service.auth.pair_client(None).unwrap();
        let (second_client, second_token) = service.auth.pair_client(None).unwrap();
        grant_execution(
            &service,
            &first_client,
            &first_connection_id,
            first_generation,
            0,
        );
        grant_execution(
            &service,
            &second_client,
            &second.connection_id,
            second.generation,
            0,
        );
        service.buffers.push(&first_connection_id, "first".into());
        service.buffers.push(&second.connection_id, "second".into());

        let pending = service.handle(IpcRequest::RunCommand {
            client_id: first_client.clone(),
            token: first_token.clone(),
            connection_id: first_connection_id.clone(),
            request_id: "session-resign-pending".into(),
            command: "id".into(),
            working_directory: "login".into(),
            timeout_seconds: 300,
        });
        let IpcResponse::RunAccepted(pending) = pending else {
            panic!("expected pending task");
        };
        let dispatching = service.handle(IpcRequest::RunCommand {
            client_id: second_client.clone(),
            token: second_token.clone(),
            connection_id: second.connection_id.clone(),
            request_id: "session-resign-dispatching".into(),
            command: "id".into(),
            working_directory: "login".into(),
            timeout_seconds: 300,
        });
        let IpcResponse::RunAccepted(dispatching) = dispatching else {
            panic!("expected dispatching task");
        };
        service
            .tasks
            .approve(&dispatching.task_id, second.generation)
            .unwrap();
        assert!(service
            .tasks
            .start_approved(
                &dispatching.task_id,
                vec![
                    "-o".into(),
                    "ProxyCommand=exec /bin/sleep 30".into(),
                    "session-resign-test-host".into(),
                    "id".into(),
                ],
            )
            .unwrap());
        let running_deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < running_deadline
            && service
                .tasks
                .internal_snapshot(&dispatching.task_id)
                .is_some_and(|task| task.status != TaskStatus::Running)
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            service
                .tasks
                .internal_snapshot(&dispatching.task_id)
                .unwrap()
                .status,
            TaskStatus::Running
        );

        service.on_system_session_resigned_active();

        assert!(service
            .auth
            .authenticate(&first_client, &first_token)
            .is_ok());
        assert!(service
            .auth
            .authenticate(&second_client, &second_token)
            .is_ok());
        assert!(service
            .auth
            .list_clients()
            .iter()
            .all(|client| client.grants.is_empty()));
        assert!(service.enabled_status().unwrap());
        assert_eq!(
            service
                .tasks
                .internal_snapshot(&pending.task_id)
                .unwrap()
                .status,
            TaskStatus::Cancelled
        );
        assert_eq!(
            service
                .tasks
                .internal_snapshot(&dispatching.task_id)
                .unwrap()
                .status,
            TaskStatus::CancelRequested
        );
        assert!(service
            .buffers
            .read(&first_connection_id, 0, 1024)
            .is_none());
        assert!(service
            .buffers
            .read(&second.connection_id, 0, 1024)
            .is_none());

        service.on_system_session_resigned_active();
        assert!(service
            .auth
            .authenticate(&first_client, &first_token)
            .is_ok());
        assert!(service
            .auth
            .authenticate(&second_client, &second_token)
            .is_ok());
        assert!(service
            .auth
            .list_clients()
            .iter()
            .all(|client| client.grants.is_empty()));
        assert!(service.enabled_status().unwrap());
    }

    #[test]
    fn default_grant_allows_observation_but_not_command_execution() {
        let (service, connection_id, generation) = service_with_connection();
        let (client_id, token) = service.auth.pair_client(None).unwrap();
        service
            .auth
            .grant_connection(&client_id, &connection_id, generation, 0)
            .unwrap();
        service.buffers.push(&connection_id, "visible".into());

        assert!(matches!(
            service.handle(IpcRequest::ReadConnectionOutput {
                client_id: client_id.clone(),
                token: token.clone(),
                connection_id: connection_id.clone(),
                cursor: 0,
                max_chars: None,
            }),
            IpcResponse::Output(_)
        ));

        assert!(matches!(
            service.handle(IpcRequest::RunCommand {
                client_id,
                token,
                connection_id,
                request_id: "observe-only".into(),
                command: "id".into(),
                working_directory: "login".into(),
                timeout_seconds: 300,
            }),
            IpcResponse::Error { ref code, .. } if code == "forbidden"
        ));
    }

    #[test]
    fn list_connections_reports_grant_permissions() {
        let (service, connection_id, generation) = service_with_connection();
        let (client_id, token) = service.auth.pair_client(None).unwrap();
        service
            .auth
            .grant_connection(&client_id, &connection_id, generation, 0)
            .unwrap();

        let IpcResponse::Connections { connections } =
            service.handle(IpcRequest::ListConnections { client_id, token })
        else {
            panic!("expected connections");
        };
        let value = serde_json::to_value(&connections[0]).unwrap();
        assert_eq!(value["observe"], true);
        assert_eq!(value["execute"], false);
    }

    #[test]
    fn execute_only_grant_runs_and_manages_tasks_without_output_observation() {
        let (service, connection_id, generation) = service_with_connection();
        let (client_id, token) = service.auth.pair_client(None).unwrap();
        service
            .auth
            .set_grant_permissions(
                &client_id,
                &connection_id,
                generation,
                0,
                GrantPermissions {
                    observe: false,
                    execute: true,
                },
            )
            .unwrap();
        service
            .buffers
            .push(&connection_id, "private output".into());

        assert!(matches!(
            service.handle(IpcRequest::ReadConnectionOutput {
                client_id: client_id.clone(),
                token: token.clone(),
                connection_id: connection_id.clone(),
                cursor: 0,
                max_chars: None,
            }),
            IpcResponse::Error { ref code, .. } if code == "forbidden"
        ));

        let accepted = service.handle(IpcRequest::RunCommand {
            client_id: client_id.clone(),
            token: token.clone(),
            connection_id: connection_id.clone(),
            request_id: "execute-only".into(),
            command: "id".into(),
            working_directory: "login".into(),
            timeout_seconds: 300,
        });
        let IpcResponse::RunAccepted(run) = accepted else {
            panic!("expected run accepted");
        };

        assert!(matches!(
            service.handle(IpcRequest::ReadTask {
                client_id: client_id.clone(),
                token: token.clone(),
                task_id: run.task_id.clone(),
                stdout_offset: None,
                stderr_offset: None,
                output_offset: None,
            }),
            IpcResponse::Task(_)
        ));
        let cancel = service.handle(IpcRequest::CancelTask {
            client_id,
            token,
            task_id: run.task_id.clone(),
        });
        assert!(matches!(cancel, IpcResponse::Ack { ok: true }));
        assert_eq!(
            service
                .tasks
                .internal_snapshot(&run.task_id)
                .unwrap()
                .status,
            TaskStatus::Cancelled,
            "a valid cancel must not race into a terminal NotCancellable response"
        );
    }

    #[test]
    fn unpaired_client_cannot_list_or_read() {
        let (service, connection_id, _) = service_with_connection();
        let response = service.handle(IpcRequest::ListConnections {
            client_id: "nope".into(),
            token: "bad".into(),
        });
        assert!(matches!(response, IpcResponse::Error { .. }));

        let response = service.handle(IpcRequest::ReadConnectionOutput {
            client_id: "nope".into(),
            token: "bad".into(),
            connection_id,
            cursor: 0,
            max_chars: None,
        });
        assert!(matches!(response, IpcResponse::Error { .. }));
    }

    #[test]
    fn granted_client_sees_only_post_grant_output() {
        let (service, connection_id, generation) = service_with_connection();
        // Pre-grant history the client must never receive.
        service
            .buffers
            .push(&connection_id, "SECRET-HISTORY".to_string());

        let (client_id, token) = service.auth.pair_client(None).unwrap();
        let grant_start = service.buffers.current_seq();
        service
            .auth
            .grant_connection(&client_id, &connection_id, generation, grant_start)
            .unwrap();

        service
            .buffers
            .push(&connection_id, "after-grant".to_string());

        let list = service.handle(IpcRequest::ListConnections {
            client_id: client_id.clone(),
            token: token.clone(),
        });
        let IpcResponse::Connections { connections } = list else {
            panic!("expected connections");
        };
        assert_eq!(connections.len(), 1);
        assert_eq!(connections[0].connection_id, connection_id);

        // Asking from cursor 0 must clamp to the grant start, and the
        // response must not contain pre-grant bytes.
        let read = service.handle(IpcRequest::ReadConnectionOutput {
            client_id,
            token,
            connection_id: connection_id.clone(),
            cursor: 0,
            max_chars: None,
        });
        let IpcResponse::Output(output) = read else {
            panic!("expected output");
        };
        assert!(output.data.contains("after-grant"));
        assert!(!output.data.contains("SECRET-HISTORY"));
        assert!(output.gap); // clamped cursor reported honestly
    }

    #[test]
    fn reconnect_invalidates_grant_on_next_request() {
        let (service, connection_id, generation) = service_with_connection();
        let (client_id, token) = service.auth.pair_client(None).unwrap();
        service
            .auth
            .grant_connection(&client_id, &connection_id, generation, 0)
            .unwrap();

        // Simulate close + reconnect: old record exits, a new generation is
        // registered for the same host.
        service.mark_connection_exited("sess-1", 0);
        let record2 = service
            .registry
            .register(
                uuid::Uuid::new_v4().to_string(),
                "sess-2".to_string(),
                snapshot(),
                Some(PathBuf::from("/tmp/mux2.sock")),
            )
            .unwrap();

        let list = service.handle(IpcRequest::ListConnections {
            client_id: client_id.clone(),
            token,
        });
        let IpcResponse::Connections { connections } = list else {
            panic!("expected connections");
        };
        // Old connection is exited (filtered), new generation has no grant.
        assert!(connections.is_empty());
        assert_ne!(record2.generation, generation);
    }

    #[test]
    fn run_command_requires_grant_and_returns_task() {
        let (service, connection_id, generation) = service_with_connection();
        let (client_id, token) = service.auth.pair_client(None).unwrap();

        // No grant yet: refused.
        let denied = service.handle(IpcRequest::RunCommand {
            client_id: client_id.clone(),
            token: token.clone(),
            connection_id: connection_id.clone(),
            request_id: "r1".into(),
            command: "uptime".into(),
            working_directory: "login".into(),
            timeout_seconds: 300,
        });
        assert!(matches!(denied, IpcResponse::Error { .. }));

        grant_execution(&service, &client_id, &connection_id, generation, 0);
        let accepted = service.handle(IpcRequest::RunCommand {
            client_id: client_id.clone(),
            token: token.clone(),
            connection_id: connection_id.clone(),
            request_id: "r1".into(),
            command: "uptime".into(),
            working_directory: "login".into(),
            timeout_seconds: 300,
        });
        let IpcResponse::RunAccepted(run) = accepted else {
            panic!("expected run accepted");
        };
        assert_eq!(run.status, "pending_approval");

        // Retry with same request_id returns the same task, no duplicate.
        let retry = service.handle(IpcRequest::RunCommand {
            client_id: client_id.clone(),
            token,
            connection_id,
            request_id: "r1".into(),
            command: "uptime".into(),
            working_directory: "login".into(),
            timeout_seconds: 300,
        });
        let IpcResponse::RunAccepted(run2) = retry else {
            panic!("expected run accepted");
        };
        assert_eq!(run.task_id, run2.task_id);
    }

    #[test]
    #[ignore = "explicit release-only MCP performance baseline"]
    fn ignored_auth_and_idempotent_scheduler_performance_baseline() {
        const ITERATIONS: usize = 10_000;

        let (service, connection_id, generation) = service_with_connection();
        let (client_id, token) = service.auth.pair_client(None).unwrap();
        grant_execution(&service, &client_id, &connection_id, generation, 0);
        let record = service
            .registry
            .get(&connection_id)
            .expect("the performance fixture must remain ready");
        let request_id = "mcp-performance-idempotent-request";
        let spec = CommandSpec {
            request_id,
            command: "uptime",
            working_directory: "login",
            timeout_seconds: 300,
        };

        // Pre-create the task so every measured iteration is the stable
        // authenticate/authorize/idempotent-retry hot path. This deliberately
        // bypasses IPC rate limiting and excludes SSH startup and approval.
        let grant = service
            .auth
            .authorize(
                &client_id,
                &token,
                &connection_id,
                generation,
                GrantPermission::Execute,
            )
            .expect("the performance fixture must be authorized");
        let first = service
            .tasks
            .submit(&client_id, &record, spec, &grant)
            .expect("the performance fixture task must be accepted");

        let mut samples = Vec::with_capacity(ITERATIONS);
        let started = std::time::Instant::now();
        for _ in 0..ITERATIONS {
            let iteration_started = std::time::Instant::now();
            service
                .auth
                .authenticate(&client_id, &token)
                .expect("the paired client must authenticate");
            let grant = service
                .auth
                .authorize(
                    &client_id,
                    &token,
                    &connection_id,
                    generation,
                    GrantPermission::Execute,
                )
                .expect("the grant must remain valid");
            let retry = service
                .tasks
                .submit(&client_id, &record, spec, &grant)
                .expect("the exact request_id retry must be accepted");
            assert_eq!(retry.task_id, first.task_id);
            samples.push(iteration_started.elapsed());
        }
        samples.sort_unstable();
        let percentile = |percent: usize| {
            let index = (samples.len() - 1) * percent / 100;
            samples[index]
        };
        let p50 = percentile(50);
        let p95 = percentile(95);
        let p99 = percentile(99);
        let total = started.elapsed();

        println!(
            concat!(
                "mcp_perf_auth_scheduler iterations={} p50_us={} p95_us={} ",
                "p99_us={} total_ms={:.3}"
            ),
            ITERATIONS,
            p50.as_micros(),
            p95.as_micros(),
            p99.as_micros(),
            total.as_secs_f64() * 1000.0
        );
        assert!(
            p95 < std::time::Duration::from_millis(10),
            "p95 auth/scheduler retry latency was {:?}",
            p95
        );
    }

    #[test]
    fn run_command_rejects_request_id_reuse_with_different_command() {
        let (service, connection_id, generation) = service_with_connection();
        let (client_id, token) = service.auth.pair_client(None).unwrap();
        grant_execution(&service, &client_id, &connection_id, generation, 0);

        let accepted = service.handle(IpcRequest::RunCommand {
            client_id: client_id.clone(),
            token: token.clone(),
            connection_id: connection_id.clone(),
            request_id: "r1".into(),
            command: "uptime".into(),
            working_directory: "login".into(),
            timeout_seconds: 300,
        });
        assert!(matches!(accepted, IpcResponse::RunAccepted(_)));

        let conflict = service.handle(IpcRequest::RunCommand {
            client_id,
            token,
            connection_id,
            request_id: "r1".into(),
            command: "whoami".into(),
            working_directory: "login".into(),
            timeout_seconds: 300,
        });
        assert!(matches!(
            conflict,
            IpcResponse::Error { ref code, .. } if code == "request_conflict"
        ));
    }

    #[test]
    fn run_command_rejects_request_id_reuse_after_regrant() {
        let (service, connection_id, generation) = service_with_connection();
        let (client_id, token) = service.auth.pair_client(None).unwrap();
        grant_execution(&service, &client_id, &connection_id, generation, 0);

        let accepted = service.handle(IpcRequest::RunCommand {
            client_id: client_id.clone(),
            token: token.clone(),
            connection_id: connection_id.clone(),
            request_id: "r1".into(),
            command: "uptime".into(),
            working_directory: "login".into(),
            timeout_seconds: 300,
        });
        assert!(matches!(accepted, IpcResponse::RunAccepted(_)));

        assert!(service.auth.revoke(&client_id, &connection_id).unwrap());
        grant_execution(&service, &client_id, &connection_id, generation, 0);

        let conflict = service.handle(IpcRequest::RunCommand {
            client_id,
            token,
            connection_id,
            request_id: "r1".into(),
            command: "uptime".into(),
            working_directory: "login".into(),
            timeout_seconds: 300,
        });
        assert!(matches!(
            conflict,
            IpcResponse::Error { ref code, .. } if code == "request_conflict"
        ));
    }

    #[test]
    fn dispatch_rechecks_the_task_execute_grant_scope() {
        let (service, connection_id, generation) = service_with_connection();
        let (client_id, token) = service.auth.pair_client(None).unwrap();
        grant_execution(&service, &client_id, &connection_id, generation, 0);
        let accepted = service.handle(IpcRequest::RunCommand {
            client_id: client_id.clone(),
            token,
            connection_id: connection_id.clone(),
            request_id: "dispatch-scope".into(),
            command: "id".into(),
            working_directory: "login".into(),
            timeout_seconds: 300,
        });
        let IpcResponse::RunAccepted(run) = accepted else {
            panic!("expected run accepted");
        };
        service.tasks.approve(&run.task_id, generation).unwrap();

        service
            .revoke_client_connection(&client_id, &connection_id)
            .unwrap();
        grant_execution(&service, &client_id, &connection_id, generation, 0);
        service.try_start_task(&run.task_id).unwrap();

        assert_eq!(
            service
                .tasks
                .internal_snapshot(&run.task_id)
                .unwrap()
                .status,
            TaskStatus::Cancelled,
            "a regrant must not dispatch work approved under an older execute grant"
        );
    }

    #[test]
    fn revoked_grant_denies_task_read_and_cancel() {
        let (service, connection_id, generation) = service_with_connection();
        let (client_id, token) = service.auth.pair_client(None).unwrap();
        grant_execution(&service, &client_id, &connection_id, generation, 0);

        let accepted = service.handle(IpcRequest::RunCommand {
            client_id: client_id.clone(),
            token: token.clone(),
            connection_id: connection_id.clone(),
            request_id: "r1".into(),
            command: "uptime".into(),
            working_directory: "login".into(),
            timeout_seconds: 300,
        });
        let IpcResponse::RunAccepted(run) = accepted else {
            panic!("expected run accepted");
        };

        assert!(service.auth.revoke(&client_id, &connection_id).unwrap());

        let read = service.handle(IpcRequest::ReadTask {
            client_id: client_id.clone(),
            token: token.clone(),
            task_id: run.task_id.clone(),
            stdout_offset: None,
            stderr_offset: None,
            output_offset: None,
        });
        assert!(matches!(read, IpcResponse::Error { .. }));

        let cancel = service.handle(IpcRequest::CancelTask {
            client_id,
            token,
            task_id: run.task_id.clone(),
        });
        assert!(matches!(cancel, IpcResponse::Error { .. }));
        assert_eq!(
            service
                .tasks
                .internal_snapshot(&run.task_id)
                .unwrap()
                .status,
            TaskStatus::PendingApproval,
            "a revoked client must not be able to change its historical task"
        );
    }

    #[test]
    fn regrant_does_not_restore_access_to_historical_task() {
        let (service, connection_id, generation) = service_with_connection();
        let (client_id, token) = service.auth.pair_client(None).unwrap();
        grant_execution(&service, &client_id, &connection_id, generation, 0);

        let accepted = service.handle(IpcRequest::RunCommand {
            client_id: client_id.clone(),
            token: token.clone(),
            connection_id: connection_id.clone(),
            request_id: "r1".into(),
            command: "uptime".into(),
            working_directory: "login".into(),
            timeout_seconds: 300,
        });
        let IpcResponse::RunAccepted(run) = accepted else {
            panic!("expected run accepted");
        };

        assert!(service.auth.revoke(&client_id, &connection_id).unwrap());
        grant_execution(&service, &client_id, &connection_id, generation, 0);

        let read = service.handle(IpcRequest::ReadTask {
            client_id: client_id.clone(),
            token: token.clone(),
            task_id: run.task_id.clone(),
            stdout_offset: None,
            stderr_offset: None,
            output_offset: None,
        });
        assert!(matches!(read, IpcResponse::Error { .. }));

        let cancel = service.handle(IpcRequest::CancelTask {
            client_id,
            token,
            task_id: run.task_id.clone(),
        });
        assert!(matches!(cancel, IpcResponse::Error { .. }));
        assert_eq!(
            service
                .tasks
                .internal_snapshot(&run.task_id)
                .unwrap()
                .status,
            TaskStatus::PendingApproval,
            "a new grant must not regain access to tasks created under the revoked grant"
        );
    }

    #[test]
    fn reconnect_does_not_restore_access_to_historical_task() {
        let (service, old_connection_id, old_generation) = service_with_connection();
        let (client_id, token) = service.auth.pair_client(None).unwrap();
        grant_execution(&service, &client_id, &old_connection_id, old_generation, 0);

        let accepted = service.handle(IpcRequest::RunCommand {
            client_id: client_id.clone(),
            token: token.clone(),
            connection_id: old_connection_id.clone(),
            request_id: "r1".into(),
            command: "uptime".into(),
            working_directory: "login".into(),
            timeout_seconds: 300,
        });
        let IpcResponse::RunAccepted(run) = accepted else {
            panic!("expected run accepted");
        };

        service.mark_connection_exited("sess-1", 0);
        let new_record = service
            .registry
            .register(
                uuid::Uuid::new_v4().to_string(),
                "sess-2".to_string(),
                snapshot(),
                Some(PathBuf::from("/tmp/mux2.sock")),
            )
            .unwrap();
        service
            .registry
            .mark_ready(&new_record.connection_id)
            .unwrap();
        grant_execution(
            &service,
            &client_id,
            &new_record.connection_id,
            new_record.generation,
            0,
        );

        let read = service.handle(IpcRequest::ReadTask {
            client_id: client_id.clone(),
            token: token.clone(),
            task_id: run.task_id.clone(),
            stdout_offset: None,
            stderr_offset: None,
            output_offset: None,
        });
        assert!(matches!(read, IpcResponse::Error { .. }));

        let cancel = service.handle(IpcRequest::CancelTask {
            client_id,
            token,
            task_id: run.task_id,
        });
        assert!(matches!(cancel, IpcResponse::Error { .. }));
    }

    #[test]
    fn closing_connection_cancels_pending_and_revokes() {
        let (service, connection_id, generation) = service_with_connection();
        let (client_id, token) = service.auth.pair_client(None).unwrap();
        grant_execution(&service, &client_id, &connection_id, generation, 0);
        let accepted = service.handle(IpcRequest::RunCommand {
            client_id: client_id.clone(),
            token: token.clone(),
            connection_id: connection_id.clone(),
            request_id: "r1".into(),
            command: "uptime".into(),
            working_directory: "login".into(),
            timeout_seconds: 300,
        });
        let IpcResponse::RunAccepted(run) = accepted else {
            panic!("expected run accepted");
        };

        assert!(service.begin_connection_close("sess-1"));

        // Grant revoked: subsequent reads fail closed.
        let read = service.handle(IpcRequest::ReadConnectionOutput {
            client_id: client_id.clone(),
            token: token.clone(),
            connection_id,
            cursor: 0,
            max_chars: None,
        });
        assert!(matches!(read, IpcResponse::Error { .. }));
        // Pending task was cancelled.
        let task = service.handle(IpcRequest::ReadTask {
            client_id: client_id.clone(),
            token: token.clone(),
            task_id: run.task_id.clone(),
            stdout_offset: None,
            stderr_offset: None,
            output_offset: None,
        });
        assert!(matches!(task, IpcResponse::Error { .. }));

        let cancel = service.handle(IpcRequest::CancelTask {
            client_id,
            token,
            task_id: run.task_id.clone(),
        });
        assert!(matches!(cancel, IpcResponse::Error { .. }));
        assert_eq!(
            service
                .tasks
                .internal_snapshot(&run.task_id)
                .unwrap()
                .status,
            TaskStatus::Cancelled
        );
    }
}
