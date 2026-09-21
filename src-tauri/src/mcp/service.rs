//! MCP core service: ties the connection registry, output buffers, auth and
//! task engine together, and serves the local IPC socket the stdio bridge
//! talks to (phases B/C, ORQ-29/ORQ-30).
//!
//! The socket lives next to the app config in an app-created directory with
//! `0600` permissions. Every request carries (client_id, token) and is
//! authorized against the *current* connection generation on each call — a
//! reconnect or revoke mid-session fails closed on the next request.

use crate::connection_registry::{ConnectionRegistry, ConnectionState};
use crate::mcp::auth::AuthState;
use crate::mcp::protocol::*;
use crate::mux::MuxManager;
use crate::output_buffer::{OutputBuffers, MCP_READ_MAX_CHARS};
use crate::task_engine::{SubmitError, TaskEngine, TaskStatus};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::Arc;

const MAX_IPC_LINE_BYTES: usize = 64 * 1024;

pub struct McpService {
    pub registry: Arc<ConnectionRegistry>,
    pub buffers: Arc<OutputBuffers>,
    pub auth: Arc<AuthState>,
    pub tasks: Arc<TaskEngine>,
    pub mux: Arc<MuxManager>,
}

impl McpService {
    /// In-memory auth (tests). The app uses `with_auth` +
    /// `AuthState::open_default` so pairings survive restarts.
    #[cfg(test)]
    pub fn new(mux: Arc<MuxManager>) -> Self {
        Self::with_auth(mux, AuthState::default())
    }

    pub fn with_auth(mux: Arc<MuxManager>, auth: AuthState) -> Self {
        Self {
            registry: Arc::new(ConnectionRegistry::default()),
            buffers: Arc::new(OutputBuffers::default()),
            auth: Arc::new(auth),
            tasks: Arc::new(TaskEngine::default()),
            mux,
        }
    }

    /// Lifecycle hook: connection closed or exited. Grants die immediately,
    /// pending approvals and running tasks are cancelled, the mux entry is
    /// removed; buffered output stays briefly so clients can read the tail.
    pub fn on_connection_closed(&self, connection_id: &str) {
        self.auth.revoke_connection(connection_id);
        self.tasks.cancel_connection_tasks(connection_id);
        self.mux.teardown(connection_id);
    }

    fn handle(&self, request: IpcRequest) -> IpcResponse {
        match request {
            IpcRequest::ListConnections { client_id, token } => {
                if self.auth.authenticate(&client_id, &token).is_err() {
                    return IpcResponse::error("unauthorized", "unknown client or bad token");
                }
                let clients = self.auth.list_clients();
                let granted: Vec<String> = clients
                    .iter()
                    .filter(|c| c.client_id == client_id)
                    .flat_map(|c| c.grants.clone())
                    .collect();
                let mut connections = Vec::new();
                for connection_id in granted {
                    let Some(record) = self.registry.get(&connection_id) else {
                        continue;
                    };
                    // Grant + token validated; re-check generation & state.
                    let Ok(grant) = self.auth.authorize(
                        &client_id,
                        &token,
                        &connection_id,
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
                let Some(record) = self.registry.get(&connection_id) else {
                    return IpcResponse::error("not_found", "unknown connection");
                };
                let grant = match self.auth.authorize(
                    &client_id,
                    &token,
                    &connection_id,
                    record.generation,
                ) {
                    Ok(grant) => grant,
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
            } => self.handle_run_command(&client_id, &token, &connection_id, &request_id, &command),
            IpcRequest::ReadTask {
                client_id,
                token,
                task_id,
                output_offset,
            } => {
                if self.auth.authenticate(&client_id, &token).is_err() {
                    return IpcResponse::error("unauthorized", "unknown client or bad token");
                }
                let Some(snapshot) = self.tasks.get_task_for_client(&client_id, &task_id) else {
                    return IpcResponse::error("not_found", "unknown task");
                };
                let (output, next_offset, truncated) = self
                    .tasks
                    .read_output_for_client(&client_id, &task_id, output_offset.unwrap_or(0))
                    .unwrap_or((String::new(), 0, false));
                IpcResponse::Task(IpcTaskRead {
                    status: task_status_name(snapshot.status).to_string(),
                    exit_code: snapshot.exit_code,
                    output,
                    next_offset,
                    truncated,
                    detail: snapshot.detail,
                })
            }
            IpcRequest::CancelTask {
                client_id,
                token,
                task_id,
            } => {
                if self.auth.authenticate(&client_id, &token).is_err() {
                    return IpcResponse::error("unauthorized", "unknown client or bad token");
                }
                match self.tasks.cancel_for_client(&client_id, &task_id) {
                    Some(_) => IpcResponse::Ack { ok: true },
                    None => IpcResponse::error("not_found", "unknown or finished task"),
                }
            }
        }
    }

    fn handle_run_command(
        &self,
        client_id: &str,
        token: &str,
        connection_id: &str,
        request_id: &str,
        command: &str,
    ) -> IpcResponse {
        let Some(record) = self.registry.get(connection_id) else {
            return IpcResponse::error("not_found", "unknown connection");
        };
        if self
            .auth
            .authorize(client_id, token, connection_id, record.generation)
            .is_err()
        {
            return IpcResponse::error("unauthorized", "no grant for connection");
        }
        let snapshot = match self
            .tasks
            .submit(client_id, request_id, &record, command)
        {
            Ok(snapshot) => snapshot,
            Err(SubmitError::NotReady) => {
                return IpcResponse::error(
                    "not_ready",
                    "connection has not finished authentication verification",
                )
            }
            Err(SubmitError::ConnectionUnavailable) => {
                return IpcResponse::error("unavailable", "connection is closing or unmanaged")
            }
        };
        // If the task already exists (idempotent retry) and was approved
        // meanwhile, make sure it is actually started.
        if snapshot.status == TaskStatus::Queued {
            self.try_start(&snapshot.task_id);
        }
        IpcResponse::RunAccepted(IpcRunAccepted {
            task_id: snapshot.task_id,
            status: task_status_name(snapshot.status).to_string(),
        })
    }

    /// Called by the UI approval flow after `tasks.approve()`. Verifies the
    /// mux *now* (fail closed) and starts the task with derived-channel args
    /// built from the registry snapshot — never from caller input.
    pub fn try_start(&self, task_id: &str) {
        let Some(snapshot) = self.pending_approved_snapshot(task_id) else {
            return;
        };
        let Some(record) = self.registry.get(&snapshot.connection_id) else {
            return;
        };
        if record.state != ConnectionState::Ready || record.generation != snapshot.generation {
            return;
        }
        if self.mux.verify(&record).is_err() {
            // Fail closed: leave the task Queued; a later user action or
            // connection event will settle it. No new connection is opened.
            return;
        }
        let Some(socket) = record.mux_socket.clone() else {
            return;
        };
        let args = self
            .mux
            .derived_channel_args(&socket, &record.host.ssh_target(), &snapshot.command);
        self.tasks.start_approved(task_id, args);
    }

    fn pending_approved_snapshot(
        &self,
        task_id: &str,
    ) -> Option<crate::task_engine::TaskSnapshot> {
        let snapshot = self.tasks_snapshot(task_id)?;
        if snapshot.status == TaskStatus::Queued {
            Some(snapshot)
        } else {
            None
        }
    }

    fn tasks_snapshot(&self, task_id: &str) -> Option<crate::task_engine::TaskSnapshot> {
        // TaskEngine exposes client-scoped reads; for the internal start path
        // we look up without a client scope.
        self.tasks.internal_snapshot(task_id)
    }
}

fn task_status_name(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::PendingApproval => "pending_approval",
        TaskStatus::Queued => "queued",
        TaskStatus::Running => "running",
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
    // A stale socket from a crashed instance is safe to remove: clients
    // re-connect (and re-authenticate) anyway.
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
    }

    let socket_path = path.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let service = Arc::clone(&service);
                    std::thread::spawn(move || handle_client(service, stream));
                }
                Err(error) => {
                    eprintln!("[mcp] ipc accept failed: {error}");
                }
            }
        }
    });
    Ok(socket_path)
}

fn handle_client(service: Arc<McpService>, stream: UnixStream) {
    let reader_stream = match stream.try_clone() {
        Ok(clone) => clone,
        Err(_) => return,
    };
    let mut reader = BufReader::new(reader_stream);
    let mut writer = stream;
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => return, // client session ended
            Ok(_) => {
                if line.len() > MAX_IPC_LINE_BYTES {
                    let _ = write_response(
                        &mut writer,
                        &IpcResponse::error("too_large", "request exceeds line budget"),
                    );
                    continue;
                }
                let request: IpcRequest = match serde_json::from_str(line.trim()) {
                    Ok(request) => request,
                    Err(_) => {
                        let _ = write_response(
                            &mut writer,
                            &IpcResponse::error("bad_request", "invalid request frame"),
                        );
                        continue;
                    }
                };
                let response = service.handle(request);
                if write_response(&mut writer, &response).is_err() {
                    return;
                }
            }
            Err(_) => return,
        }
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
        let mux = Arc::new(
            MuxManager::for_test(
                std::env::temp_dir().join(format!("xt-mux-test-{}", std::process::id())),
                PathBuf::from("/tmp/ssh_config"),
            ),
        );
        let service = Arc::new(McpService::new(mux));
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
        service.buffers.push(&connection_id, "SECRET-HISTORY".to_string());

        let (client_id, token) = service.auth.pair_client(None).unwrap();
        let grant_start = service.buffers.current_seq();
        service
            .auth
            .grant_connection(&client_id, &connection_id, generation, grant_start)
            .unwrap();

        service.buffers.push(&connection_id, "after-grant".to_string());

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
        service.registry.mark_exited_by_session("sess-1", 0);
        service.on_connection_closed(&connection_id);
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
        });
        assert!(matches!(denied, IpcResponse::Error { .. }));

        service
            .auth
            .grant_connection(&client_id, &connection_id, generation, 0)
            .unwrap();
        let accepted = service.handle(IpcRequest::RunCommand {
            client_id: client_id.clone(),
            token: token.clone(),
            connection_id: connection_id.clone(),
            request_id: "r1".into(),
            command: "uptime".into(),
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
        });
        let IpcResponse::RunAccepted(run2) = retry else {
            panic!("expected run accepted");
        };
        assert_eq!(run.task_id, run2.task_id);
    }

    #[test]
    fn closing_connection_cancels_pending_and_revokes() {
        let (service, connection_id, generation) = service_with_connection();
        let (client_id, token) = service.auth.pair_client(None).unwrap();
        service
            .auth
            .grant_connection(&client_id, &connection_id, generation, 0)
            .unwrap();
        let accepted = service.handle(IpcRequest::RunCommand {
            client_id: client_id.clone(),
            token: token.clone(),
            connection_id: connection_id.clone(),
            request_id: "r1".into(),
            command: "uptime".into(),
        });
        let IpcResponse::RunAccepted(run) = accepted else {
            panic!("expected run accepted");
        };

        service.registry.begin_close_by_session("sess-1");
        service.on_connection_closed(&connection_id);

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
            client_id,
            token,
            task_id: run.task_id,
            output_offset: None,
        });
        let IpcResponse::Task(task) = task else {
            panic!("expected task");
        };
        assert_eq!(task.status, "cancelled");
    }
}
