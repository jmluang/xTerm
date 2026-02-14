use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use crate::pty::PtyState;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Host {
    pub id: String,
    pub name: String,
    pub alias: String,
    pub hostname: String,
    pub user: String,
    pub port: u16,
    pub identity_file: Option<String>,
    pub proxy_jump: Option<String>,
    pub tags: Vec<String>,
    pub notes: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    pub deleted: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Settings {
    pub webdav_url: Option<String>,
    pub webdav_username: Option<String>,
    pub webdav_password: Option<String>,
}

fn get_config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("xtermius")
}

fn get_hosts_path() -> PathBuf {
    get_config_dir().join("hosts.json")
}

fn get_settings_path() -> PathBuf {
    get_config_dir().join("settings.json")
}

fn get_ssh_config_path() -> PathBuf {
    get_config_dir().join("ssh_config")
}

fn ensure_config_dir() -> Result<(), String> {
    let config_dir = get_config_dir();
    if !config_dir.exists() {
        fs::create_dir_all(&config_dir).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn hosts_load() -> Result<Vec<Host>, String> {
    ensure_config_dir()?;
    let path = get_hosts_path();
    if !path.exists() {
        return Ok(vec![]);
    }
    let content = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let hosts: Vec<Host> = serde_json::from_str(&content).map_err(|e| e.to_string())?;
    Ok(hosts)
}

#[tauri::command]
fn hosts_save(hosts: Vec<Host>) -> Result<(), String> {
    ensure_config_dir()?;
    let path = get_hosts_path();
    let content = serde_json::to_string_pretty(&hosts).map_err(|e| e.to_string())?;
    fs::write(&path, content).map_err(|e| e.to_string())?;
    generate_ssh_config(hosts)?;
    Ok(())
}

#[tauri::command]
fn generate_ssh_config(hosts: Vec<Host>) -> Result<(), String> {
    ensure_config_dir()?;
    let mut config = String::new();
    for host in hosts {
        if host.deleted {
            continue;
        }
        config.push_str(&format!("Host {}\n", host.alias));
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

#[tauri::command]
fn settings_load() -> Result<Settings, String> {
    ensure_config_dir()?;
    let path = get_settings_path();
    if !path.exists() {
        return Ok(Settings {
            webdav_url: None,
            webdav_username: None,
            webdav_password: None,
        });
    }
    let content = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let settings: Settings = serde_json::from_str(&content).map_err(|e| e.to_string())?;
    Ok(settings)
}

#[tauri::command]
fn settings_save(settings: Settings) -> Result<(), String> {
    ensure_config_dir()?;
    let path = get_settings_path();
    let content = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    fs::write(&path, content).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn webdav_pull() -> Result<(), String> {
    let settings = settings_load()?;
    let webdav_url = settings.webdav_url.ok_or("WebDAV URL not configured")?;

    let client = reqwest::Client::new();
    let mut request = client.get(&format!("{}/hosts.json", webdav_url));

    if let (Some(username), Some(password)) =
        (&settings.webdav_username, &settings.webdav_password)
    {
        request = request.basic_auth(username, Some(password));
    }

    let response = request.send().await.map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("Pull failed: {}", response.status()));
    }

    let content = response.text().await.map_err(|e| e.to_string())?;

    let backup_path = get_hosts_path();
    if backup_path.exists() {
        let timestamp = chrono::Utc::now().format("%Y%m%d%H%M%S");
        let backup = backup_path.with_extension(format!("json.bak.{}", timestamp));
        let _ = fs::copy(&backup_path, &backup);
    }

    fs::write(&backup_path, content).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn webdav_push() -> Result<(), String> {
    let settings = settings_load()?;
    let webdav_url = settings.webdav_url.ok_or("WebDAV URL not configured")?;

    let hosts_path = get_hosts_path();
    let content = fs::read_to_string(&hosts_path).map_err(|e| e.to_string())?;

    let client = reqwest::Client::new();
    let mut request = client
        .put(&format!("{}/hosts.json", webdav_url))
        .body(content);

    if let (Some(username), Some(password)) =
        (&settings.webdav_username, &settings.webdav_password)
    {
        request = request.basic_auth(username, Some(password));
    }

    let response = request.send().await.map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("Push failed: {}", response.status()));
    }

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(PtyState::default())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            hosts_load,
            hosts_save,
            generate_ssh_config,
            settings_load,
            settings_save,
            webdav_pull,
            webdav_push,
            crate::pty::pty_spawn,
            crate::pty::pty_write,
            crate::pty::pty_resize,
            crate::pty::pty_kill,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
