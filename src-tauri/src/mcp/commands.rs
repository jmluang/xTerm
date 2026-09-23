//! Tauri commands backing the MCP settings / approval UI (phases B/C).
//!
//! These are the *user-driven* surface: pairing, granting, revoking,
//! approving. The agent-facing surface is the IPC socket in `service.rs`;
//! nothing here is callable by MCP clients.

use serde::Serialize;

use crate::mcp::auth::GrantPermissions;
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
    pub port: u16,
    pub alias: String,
    pub state: String,
    /// Connection predates managed reuse or mux setup failed: the user must
    /// reconnect before MCP can be enabled for it. We never silently
    /// re-authenticate in the background to upgrade it.
    pub requires_reconnect: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpStatus {
    pub enabled: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpBridgeInfo {
    /// The bridge is resolved beside the running application binary. This is
    /// deliberately not a PATH lookup, and no socket or credential is
    /// returned to the frontend.
    pub executable_path: String,
    /// `packaged` for an app bundle, otherwise `development` for a local
    /// target/debug or target/release checkout.
    pub environment: &'static str,
}

fn service() -> Result<std::sync::Arc<McpService>, String> {
    crate::mcp_service().ok_or_else(|| "MCP service is not running".to_string())
}

#[tauri::command]
pub fn mcp_status() -> Result<McpStatus, String> {
    Ok(McpStatus {
        enabled: service()?.enabled_status()?,
    })
}

#[tauri::command]
pub fn mcp_set_enabled(enabled: bool) -> Result<McpStatus, String> {
    let service = service()?;
    service.set_enabled(enabled)?;
    Ok(McpStatus { enabled })
}

#[tauri::command]
pub fn mcp_bridge_info() -> Result<McpBridgeInfo, String> {
    let app_executable = std::env::current_exe()
        .map_err(|error| format!("failed to resolve the running app executable: {error}"))?;
    let app_directory = app_executable
        .parent()
        .ok_or_else(|| "running app executable has no parent directory".to_string())?;
    let bridge_name = format!("xtermius-mcp-bridge{}", std::env::consts::EXE_SUFFIX);
    let bridge_path = app_directory.join(bridge_name);

    let environment = if is_packaged_app_executable(&app_executable) {
        "packaged"
    } else {
        "development"
    };

    Ok(McpBridgeInfo {
        executable_path: bridge_path.to_string_lossy().into_owned(),
        environment,
    })
}

fn is_packaged_app_executable(path: &std::path::Path) -> bool {
    let mut saw_app_bundle = false;
    let mut saw_contents = false;
    let mut saw_macos_directory = false;
    for component in path.components() {
        let value = component.as_os_str().to_string_lossy();
        saw_app_bundle |= value.ends_with(".app");
        saw_contents |= value == "Contents";
        saw_macos_directory |= value == "MacOS";
    }
    saw_app_bundle && saw_contents && saw_macos_directory
}

#[tauri::command]
pub fn mcp_pair_client(label: Option<String>) -> Result<PairingCreated, String> {
    let (client_id, token) = service()?
        .auth
        .pair_client(label)
        .map_err(|error| format!("failed to persist pairing: {error}"))?;
    Ok(PairingCreated { client_id, token })
}

#[tauri::command]
pub fn mcp_list_clients() -> Result<Vec<crate::mcp::auth::PairedClientSummary>, String> {
    Ok(service()?.auth.list_clients())
}

#[tauri::command]
pub fn mcp_unpair_client(client_id: String) -> Result<(), String> {
    service()?.unpair_client(&client_id)
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
                port: record.host.port,
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
    service.set_client_connection_permissions(
        &client_id,
        &connection_id,
        GrantPermissions::observe_only(),
    )?;
    Ok(true)
}

#[tauri::command]
pub fn mcp_revoke_connection(client_id: String, connection_id: String) -> Result<bool, String> {
    service()?.revoke_client_connection(&client_id, &connection_id)
}

#[tauri::command]
pub fn mcp_set_grant_permissions(
    client_id: String,
    connection_id: String,
    observe: bool,
    execute: bool,
) -> Result<(), String> {
    let service = service()?;
    service.set_client_connection_permissions(
        &client_id,
        &connection_id,
        GrantPermissions { observe, execute },
    )
}

#[tauri::command]
pub fn mcp_set_auto_approve(
    client_id: String,
    connection_id: String,
    enabled: bool,
) -> Result<(), String> {
    service()?.set_client_auto_approve_commands(&client_id, &connection_id, enabled)
}

#[tauri::command]
pub fn mcp_pending_tasks() -> Result<Vec<crate::task_engine::TaskSnapshot>, String> {
    service()?.pending_approvals()
}

/// Return a bounded, newest-first view of command metadata. Output is never
/// included in the audit record or this Tauri response.
#[tauri::command]
pub fn mcp_recent_task_audit(
    limit: Option<usize>,
) -> Result<Vec<crate::mcp::audit::TaskAuditRecord>, String> {
    if let Some(limit) = limit {
        if !(1..=100).contains(&limit) {
            return Err("audit limit must be between 1 and 100".to_string());
        }
    }
    service()?.recent_task_audit(limit)
}

/// Approve the exact command captured when the task was submitted. The
/// engine re-checks the connection generation before queueing, and the start
/// path re-verifies the mux before spawning — a stale approval cannot run.
#[tauri::command]
pub fn mcp_approve_task(task_id: String) -> Result<crate::task_engine::TaskSnapshot, String> {
    service()?.approve_task(&task_id)
}

#[tauri::command]
pub fn mcp_reject_task(task_id: String) -> Result<bool, String> {
    service()?
        .tasks
        .reject(&task_id)
        .map(|snapshot| snapshot.is_some())
        .map_err(|_| "task rejection could not be recorded in the audit ledger".to_string())
}
