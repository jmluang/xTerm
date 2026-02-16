use crate::host_store::ensure_config_dir;
use crate::models::Host;
use std::fs;
use std::path::PathBuf;

fn get_ssh_config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("xtermius")
        .join("ssh_config")
}

#[tauri::command]
pub fn generate_ssh_config(hosts: Vec<Host>) -> Result<(), String> {
    ensure_config_dir()?;
    let mut config = String::new();
    for host in hosts {
        if host.deleted {
            continue;
        }
        let alias = if host.alias.trim().is_empty() {
            host.hostname.clone()
        } else {
            host.alias.clone()
        };
        config.push_str(&format!("Host {}\n", alias));
        config.push_str(&format!("  HostName {}\n", host.hostname));
        if !host.user.is_empty() {
            config.push_str(&format!("  User {}\n", host.user));
        }
        if host.port != 22 {
            config.push_str(&format!("  Port {}\n", host.port));
        }
        if let Some(ref identity_file) = host.identity_file {
            config.push_str(&format!("  IdentityFile {}\n", identity_file));
            config.push_str("  IdentitiesOnly yes\n");
        }
        if let Some(ref proxy_jump) = host.proxy_jump {
            config.push_str(&format!("  ProxyJump {}\n", proxy_jump));
        }
        config.push_str("  ServerAliveInterval 30\n");
        config.push('\n');
    }
    let path = get_ssh_config_path();
    fs::write(&path, config).map_err(|e| e.to_string())?;
    Ok(())
}
