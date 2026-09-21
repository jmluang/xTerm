mod app;
mod connection_registry;
mod credential_store;
mod host_probe;
mod host_store;
mod mcp;
mod models;
mod mux;
mod output_buffer;
mod pty;
mod ssh_config;
mod ssh_import;
mod task_engine;
mod webdav_sync;
mod webdav_url;

use std::sync::{Arc, OnceLock};

/// Process-wide MCP core (ORQ-27). Initialized during app setup; PTY code
/// checks availability on every use so the terminal keeps working even if
/// MCP failed to start (MCP failure must never break the terminal).
static MCP_SERVICE: OnceLock<Arc<mcp::service::McpService>> = OnceLock::new();

pub(crate) fn mcp_service() -> Option<Arc<mcp::service::McpService>> {
    MCP_SERVICE.get().cloned()
}

fn init_mcp_service() {
    match mux::MuxManager::new() {
        Ok(mux) => {
            let auth = mcp::auth::AuthState::open_default();
            let service = Arc::new(mcp::service::McpService::with_auth(Arc::new(mux), auth));
            match mcp::service::start_ipc_server(Arc::clone(&service)) {
                Ok(path) => {
                    eprintln!("[mcp] local bridge socket listening at {}", path.display());
                    let _ = MCP_SERVICE.set(service);
                    // Periodic maintenance: drop finished task results past
                    // retention and forget long-exited connection records.
                    std::thread::spawn(|| loop {
                        std::thread::sleep(std::time::Duration::from_secs(60));
                        let Some(service) = mcp_service() else { return };
                        service.tasks.sweep_finished();
                        let cutoff = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as u64)
                            .unwrap_or(0)
                            .saturating_sub(600_000);
                        service.registry.purge_exited_before(cutoff);
                    });
                }
                Err(error) => eprintln!("[mcp] ipc server failed to start: {error}"),
            }
        }
        Err(error) => eprintln!("[mcp] mux manager failed to start: {error}"),
    }
}

pub use app::*;
