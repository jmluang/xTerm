use crate::pty::PtyState;
use tauri::Manager;

#[cfg(target_os = "macos")]
fn install_system_session_observer() {
    crate::session_monitor::install();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    crate::init_mcp_service();
    let builder = tauri::Builder::default()
        .manage(PtyState::default())
        .setup(|app| {
            #[cfg(target_os = "macos")]
            {
                install_system_session_observer();

                use window_vibrancy::{
                    apply_vibrancy, NSVisualEffectMaterial, NSVisualEffectState,
                };

                if let Some(window) = app.get_webview_window("main") {
                    let _ = apply_vibrancy(
                        &window,
                        NSVisualEffectMaterial::Sidebar,
                        Some(NSVisualEffectState::Active),
                        None,
                    );
                }
            }
            Ok(())
        })
        .plugin(tauri_plugin_dialog::init());

    #[cfg(desktop)]
    let builder = builder
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build());

    #[cfg(not(desktop))]
    let builder = builder;

    builder
        .invoke_handler(tauri::generate_handler![
            crate::host_store::hosts_load,
            crate::host_store::hosts_save,
            crate::ssh_config::generate_ssh_config,
            crate::ssh_import::ssh_config_scan_importable_hosts,
            crate::host_store::settings_load,
            crate::host_store::settings_save,
            crate::host_probe::host_probe_static,
            crate::host_probe::host_probe_live,
            crate::webdav_sync::webdav_pull,
            crate::webdav_sync::webdav_push,
            crate::credential_store::host_password_set,
            crate::credential_store::host_password_delete,
            crate::pty::pty_spawn_ssh,
            crate::pty::pty_write,
            crate::pty::pty_resize,
            crate::pty::pty_kill,
            crate::mcp::commands::mcp_pair_client,
            crate::mcp::commands::mcp_status,
            crate::mcp::commands::mcp_set_enabled,
            crate::mcp::commands::mcp_bridge_info,
            crate::mcp::commands::mcp_list_clients,
            crate::mcp::commands::mcp_unpair_client,
            crate::mcp::commands::mcp_list_connections,
            crate::mcp::commands::mcp_grant_connection,
            crate::mcp::commands::mcp_set_grant_permissions,
            crate::mcp::commands::mcp_set_auto_approve,
            crate::mcp::commands::mcp_revoke_connection,
            crate::mcp::commands::mcp_pending_tasks,
            crate::mcp::commands::mcp_recent_task_audit,
            crate::mcp::commands::mcp_approve_task,
            crate::mcp::commands::mcp_reject_task,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
