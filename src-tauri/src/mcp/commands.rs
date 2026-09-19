//! Tauri commands backing the MCP settings / approval UI (phases B/C).
//!
//! These are the *user-driven* surface: pairing, granting, revoking,
//! approving. The agent-facing surface is the IPC socket in `service.rs`;
//! nothing here is callable by MCP clients.

use serde::Serialize;

use crate::mcp::service::McpService;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingCreated {
    pub client_id: String,
    /// Shown exactly once; only its hash is retained by the app.
    pub token: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpConnectionView {
    pub connection_id: String,
    pub generation: u64,
    pub host_name: String,
    pub user: String,
    pub alias: String,
    pub state: String,
    /// Connection predates managed reuse or mux setup failed: the user must
    /// reconnect before MCP can be enabled for it. We never silently
    /// re-authenticate in the background to upgrade it.
    pub requires_reconnect: bool,
}

fn service() -> Result<std::sync::Arc<McpService>, String> {
    crate::mcp_service().ok_or_else(|| "MCP service is not running".to_string())
}

#[tauri::command]
pub fn mcp_pair_client(label: Option<String>) -> Result<PairingCreated, String> {
    let (client_id, token) = service()?
        .auth
        .pair_client(label)
        .ok_or("failed to create pairing")?;
    Ok(PairingCreated { client_id, token })
}

#[tauri::command]
pub fn mcp_list_clients() -> Result<Vec<crate::mcp::auth::PairedClientSummary>, String> {
    Ok(service()?.auth.list_clients())
}

#[tauri::command]
pub fn mcp_unpair_client(client_id: String) -> Result<(), String> {
    let service = service()?;
    service.auth.unpair(&client_id);
    // Client session over: pending approvals from it must never execute.
    service.tasks.cancel_client_pending(&client_id);
    Ok(())
}

#[tauri::command]
pub fn mcp_list_connections() -> Result<Vec<McpConnectionView>, String> {
    let service = service()?;
    Ok(service
        .registry
        .list()
        .into_iter()
        .filter(|record| {
            !matches!(
                record.state,
                crate::connection_registry::ConnectionState::Exited
            )
        })
        .map(|record| {
            let requires_reconnect = record.requires_reconnect_for_mcp();
            let state = format!("{:?}", record.state).to_lowercase();
            McpConnectionView {
                connection_id: record.connection_id,
                generation: record.generation,
                host_name: record.host.hostname,
                user: record.host.user,
                alias: record.host.alias,
                state,
                requires_reconnect,
            }
        })
        .collect())
}

/// Grant a paired client access to one connection, starting from the current
/// output position: the client never sees output from before the grant.
#[tauri::command]
pub fn mcp_grant_connection(client_id: String, connection_id: String) -> Result<bool, String> {
    let service = service()?;
    let record = service
        .registry
        .get(&connection_id)
        .ok_or("unknown connection")?;
    if !record.mcp_eligible() {
        if record.requires_reconnect_for_mcp() {
            return Err("this connection predates managed reuse; reconnect it to enable MCP".into());
        }
        return Err("connection is closing or gone".into());
    }
    let grant_start = service.buffers.current_seq();
    Ok(service
        .auth
        .grant_connection(&client_id, &connection_id, record.generation, grant_start)
        .is_some())
}

#[tauri::command]
pub fn mcp_revoke_connection(client_id: String, connection_id: String) -> Result<bool, String> {
    let service = service()?;
    let revoked = service.auth.revoke(&client_id, &connection_id);
    if revoked {
        // Revocation must take effect on pending work immediately.
        service.tasks.cancel_connection_tasks(&connection_id);
    }
    Ok(revoked)
}

#[tauri::command]
pub fn mcp_pending_tasks() -> Result<Vec<crate::task_engine::TaskSnapshot>, String> {
    Ok(service()?.tasks.pending_approvals())
}

/// Approve the exact command captured when the task was submitted. The
/// engine re-checks the connection generation before queueing, and the start
/// path re-verifies the mux before spawning — a stale approval cannot run.
#[tauri::command]
pub fn mcp_approve_task(task_id: String) -> Result<crate::task_engine::TaskSnapshot, String> {
    let service = service()?;
    let snapshot = service
        .tasks
        .internal_snapshot(&task_id)
        .ok_or("unknown task")?;
    let record = service
        .registry
        .get(&snapshot.connection_id)
        .ok_or("connection is gone")?;
    let approved = service
        .tasks
        .approve(&task_id, record.generation)
        .map_err(|error| format!("approval failed: {error:?}"))?;
    service.try_start(&task_id);
    Ok(approved)
}

#[tauri::command]
pub fn mcp_reject_task(task_id: String) -> Result<bool, String> {
    Ok(service()?.tasks.reject(&task_id).is_some())
}
