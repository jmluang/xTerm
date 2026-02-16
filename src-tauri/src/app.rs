use crate::pty::PtyState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(PtyState::default())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            crate::host_store::hosts_load,
            crate::host_store::hosts_save,
            crate::ssh_config::generate_ssh_config,
            crate::host_store::settings_load,
            crate::host_store::settings_save,
            crate::webdav_sync::webdav_pull,
            crate::webdav_sync::webdav_push,
            crate::credential_store::host_password_get,
            crate::credential_store::host_password_set,
            crate::credential_store::host_password_delete,
            crate::pty::pty_spawn,
            crate::pty::pty_write,
            crate::pty::pty_resize,
            crate::pty::pty_kill,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
