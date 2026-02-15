use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use crate::pty::PtyState;
use rusqlite::{params, Connection};
use url::Url;
use reqwest::Method;
use keyring::{Entry, Error as KeyringError};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Host {
    pub id: String,
    #[serde(rename = "sortOrder")]
    #[serde(default)]
    pub sort_order: Option<i64>,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub alias: String,
    pub hostname: String,
    #[serde(default)]
    pub user: String,
    #[serde(default = "default_port")]
    pub port: u16,
    // NOTE: Stored on disk in hosts.json currently (plaintext). If this app grows,
    // move secrets to the OS keychain/credential store.
    #[serde(default)]
    pub password: Option<String>,
    #[serde(rename = "hasPassword")]
    #[serde(default)]
    pub has_password: bool,
    #[serde(rename = "identityFile")]
    pub identity_file: Option<String>,
    #[serde(rename = "proxyJump")]
    pub proxy_jump: Option<String>,
    #[serde(rename = "envVars")]
    #[serde(default)]
    pub env_vars: Option<String>,
    #[serde(default)]
    pub encoding: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub notes: String,
    #[serde(rename = "updatedAt")]
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub deleted: bool,
}

fn default_port() -> u16 {
    22
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Settings {
    pub webdav_url: Option<String>,
    #[serde(default)]
    pub webdav_folder: Option<String>,
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

fn get_hosts_db_path() -> PathBuf {
    get_config_dir().join("hosts.db")
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

fn open_hosts_db() -> Result<Connection, String> {
    ensure_config_dir()?;
    let path = get_hosts_db_path();
    Connection::open(path).map_err(|e| e.to_string())
}

fn ensure_hosts_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS hosts (
          id            TEXT PRIMARY KEY,
          sort_order    INTEGER NOT NULL DEFAULT 0,
          name          TEXT NOT NULL,
          alias         TEXT NOT NULL,
          hostname      TEXT NOT NULL,
          user          TEXT NOT NULL,
          port          INTEGER NOT NULL,
          password      TEXT,
          has_password  INTEGER NOT NULL DEFAULT 0,
          identity_file TEXT,
          proxy_jump    TEXT,
          env_vars      TEXT,
          encoding      TEXT,
          tags_json     TEXT NOT NULL,
          notes         TEXT NOT NULL,
          updated_at    TEXT NOT NULL,
          deleted       INTEGER NOT NULL
        );
        "#,
    )
    .map_err(|e| e.to_string())?;

    // Lightweight migrations (ignore "duplicate column" errors).
    let _ = conn.execute("ALTER TABLE hosts ADD COLUMN sort_order INTEGER NOT NULL DEFAULT 0", []);
    let _ = conn.execute("ALTER TABLE hosts ADD COLUMN has_password INTEGER NOT NULL DEFAULT 0", []);
    Ok(())
}

fn keychain_entry(host_id: &str) -> Result<Entry, String> {
    // Use host id as the account key; stable even if alias/hostname changes.
    Entry::new("xTermius", host_id).map_err(|e| e.to_string())
}

fn keychain_set_password(host_id: &str, password: &str) -> Result<(), String> {
    keychain_entry(host_id)?
        .set_password(password)
        .map_err(|e| e.to_string())
}

fn keychain_has_password(host_id: &str) -> bool {
    let entry = match keychain_entry(host_id) {
        Ok(e) => e,
        Err(_) => return false,
    };
    match entry.get_password() {
        Ok(pw) => !pw.trim().is_empty(),
        Err(KeyringError::NoEntry) => false,
        Err(_) => false,
    }
}

fn keychain_delete_password(host_id: &str) -> Result<(), String> {
    // Deleting a non-existent entry should be treated as success.
    match keychain_entry(host_id)?.delete_credential() {
        Ok(()) => Ok(()),
        Err(KeyringError::NoEntry) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

fn migrate_db_passwords_to_keychain(conn: &Connection) -> Result<(), String> {
    ensure_hosts_schema(conn)?;
    let mut stmt = conn
        .prepare("SELECT id, password, has_password FROM hosts WHERE password IS NOT NULL AND password != ''")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            let id: String = row.get(0)?;
            let pw: String = row.get(1)?;
            let hp: i64 = row.get(2)?;
            Ok((id, pw, hp != 0))
        })
        .map_err(|e| e.to_string())?;

    for r in rows {
        let (id, pw, has_pw) = r.map_err(|e| e.to_string())?;
        if has_pw {
            // Already migrated; just clear plaintext.
            let _ = conn.execute("UPDATE hosts SET password = NULL WHERE id = ?1", params![id]);
            continue;
        }
        let pwt = pw.trim();
        if pwt.is_empty() {
            continue;
        }
        match keychain_set_password(&id, pwt) {
            Ok(()) => {
                let _ = conn.execute(
                    "UPDATE hosts SET has_password = 1, password = NULL WHERE id = ?1",
                    params![id],
                );
            }
            Err(e) => {
                // Don't erase plaintext if Keychain isn't available; keep it so user can recover.
                eprintln!("[keychain] migrate failed for host {id}: {e}");
            }
        }
    }
    Ok(())
}

fn import_hosts_json_to_db(conn: &mut Connection, hosts: Vec<Host>) -> Result<(), String> {
    ensure_hosts_schema(conn)?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM hosts", []).map_err(|e| e.to_string())?;
    for (i, h) in hosts.into_iter().enumerate() {
        let tags_json = serde_json::to_string(&h.tags).map_err(|e| e.to_string())?;
        let sort_order = h.sort_order.unwrap_or(i as i64);

        // Passwords are stored in the OS credential store (Keychain, etc.), not in SQLite/WebDAV.
        // Rules:
        // - If a password is provided, store/update it in keychain and mark has_password=1.
        // - If a password is provided but empty, delete keychain entry and mark has_password=0.
        // - If no password is provided and has_password=false, delete any existing entry.
        // - If no password is provided and has_password=true, keep existing keychain entry.
        // has_password is a local-only indicator. It must reflect the current device's Keychain,
        // not whatever value came from a synced hosts.db.
        let has_password;
        if let Some(ref pw) = h.password {
            let pwt = pw.trim();
            if pwt.is_empty() {
                keychain_delete_password(&h.id)?;
                has_password = false;
            } else {
                // If we can't save to Keychain, fail the write so we don't lose the password.
                keychain_set_password(&h.id, pwt)
                    .map_err(|e| format!("Failed to save password to Keychain: {e}"))?;
                has_password = true;
            }
        } else {
            // No password in payload: keep Keychain as-is.
            has_password = keychain_has_password(&h.id);
        }

        tx.execute(
            r#"
            INSERT INTO hosts (
              id, sort_order, name, alias, hostname, user, port,
              password, has_password, identity_file, proxy_jump, env_vars, encoding,
              tags_json, notes, updated_at, deleted
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
            "#,
            params![
                h.id,
                sort_order,
                h.name,
                h.alias,
                h.hostname,
                h.user,
                h.port as u32,
                Option::<String>::None,
                if has_password { 1 } else { 0 },
                h.identity_file,
                h.proxy_jump,
                h.env_vars,
                h.encoding,
                tags_json,
                h.notes,
                h.updated_at,
                if h.deleted { 1 } else { 0 }
            ],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn hosts_load() -> Result<Vec<Host>, String> {
    // Primary store: SQLite (single file).
    // One-time migration: if hosts.db is missing but hosts.json exists, import and keep using SQLite.
    ensure_config_dir()?;
    let db_path = get_hosts_db_path();
    if !db_path.exists() {
        let json_path = get_hosts_path();
        if json_path.exists() {
            let content = fs::read_to_string(&json_path).map_err(|e| e.to_string())?;
            let hosts: Vec<Host> = serde_json::from_str(&content).map_err(|e| e.to_string())?;
            let mut conn = open_hosts_db()?;
            import_hosts_json_to_db(&mut conn, hosts.clone())?;
            // Ensure ssh_config exists after migration.
            let _ = generate_ssh_config(hosts.clone());
            return Ok(hosts);
        }
    }

    let conn = open_hosts_db()?;
    ensure_hosts_schema(&conn)?;
    let _ = migrate_db_passwords_to_keychain(&conn);

    let mut stmt = conn
        .prepare(
            r#"
            SELECT
              id, sort_order, name, alias, hostname, user, port,
              password, has_password, identity_file, proxy_jump, env_vars, encoding,
              tags_json, notes, updated_at, deleted
            FROM hosts
            ORDER BY sort_order ASC, updated_at DESC
            "#,
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map([], |row| {
            let tags_json: String = row.get(13)?;
            let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();
            let id: String = row.get(0)?;
            Ok(Host {
                id: id.clone(),
                sort_order: Some(row.get(1)?),
                name: row.get(2)?,
                alias: row.get(3)?,
                hostname: row.get(4)?,
                user: row.get(5)?,
                port: {
                    let p: u32 = row.get(6)?;
                    p as u16
                },
                // Never return the plaintext password to the frontend.
                password: None,
                // Local-only: reflect actual Keychain state on this device.
                has_password: keychain_has_password(&id),
                identity_file: row.get(9)?,
                proxy_jump: row.get(10)?,
                env_vars: row.get(11)?,
                encoding: row.get(12)?,
                tags,
                notes: row.get(14)?,
                updated_at: row.get(15)?,
                deleted: {
                    let d: i64 = row.get(16)?;
                    d != 0
                },
            })
        })
        .map_err(|e| e.to_string())?;

    let mut hosts: Vec<Host> = Vec::new();
    for r in rows {
        hosts.push(r.map_err(|e| e.to_string())?);
    }
    Ok(hosts)
}

#[tauri::command]
fn hosts_save(hosts: Vec<Host>) -> Result<(), String> {
    let mut conn = open_hosts_db()?;
    import_hosts_json_to_db(&mut conn, hosts.clone())?;
    // Keep ssh_config generation behaviour unchanged.
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

#[tauri::command]
fn settings_load() -> Result<Settings, String> {
    ensure_config_dir()?;
    let path = get_settings_path();
    if !path.exists() {
        return Ok(Settings {
            webdav_url: None,
            webdav_folder: Some("xTermius".to_string()),
            webdav_username: None,
            webdav_password: None,
        });
    }
    let content = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut settings: Settings = serde_json::from_str(&content).map_err(|e| e.to_string())?;
    if settings.webdav_folder.is_none() {
        settings.webdav_folder = Some("xTermius".to_string());
    }
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

fn webdav_resolve_url(input: &str, filename: &str) -> Result<String, String> {
    let raw = input.trim();
    if raw.is_empty() {
        return Err("WebDAV URL not configured".to_string());
    }

    let mut url = Url::parse(raw).map_err(|e| format!("Invalid WebDAV URL: {e}"))?;
    let path = url.path().to_string();

    // If already pointing at the target file, keep it.
    let last_seg = path.rsplit('/').next().unwrap_or("");
    if last_seg == filename {
        return Ok(url.to_string());
    }

    // Trailing slash (or empty last segment) means "directory".
    if path.ends_with('/') || last_seg.is_empty() {
        url.path_segments_mut()
            .map_err(|_| "Invalid WebDAV URL (cannot modify path)".to_string())?
            .pop_if_empty()
            .push(filename);
        return Ok(url.to_string());
    }

    // Treat explicit .db/.json/.sqlite as a file path; otherwise treat as a directory-like URL.
    let looks_like_file = last_seg.ends_with(".db") || last_seg.ends_with(".json") || last_seg.ends_with(".sqlite");

    {
        let mut segs = url
            .path_segments_mut()
            .map_err(|_| "Invalid WebDAV URL (cannot modify path)".to_string())?;
        if looks_like_file {
            let _ = segs.pop();
        }
        segs.push(filename);
    }
    Ok(url.to_string())
}

async fn webdav_mkcol(client: &reqwest::Client, settings: &Settings, url: &str) -> Result<reqwest::Response, String> {
    let mut req = client.request(Method::from_bytes(b"MKCOL").unwrap(), url);
    if let (Some(username), Some(password)) = (&settings.webdav_username, &settings.webdav_password) {
        req = req.basic_auth(username, Some(password));
    }
    req.send().await.map_err(|e| e.to_string())
}

async fn webdav_ensure_remote_folder(
    client: &reqwest::Client,
    settings: &Settings,
    webdav_url: &str,
    folder: Option<&str>,
) -> Result<(), String> {
    let folder = folder.unwrap_or("").trim().trim_matches('/');
    if folder.is_empty() {
        return Ok(());
    }

    let mut base = Url::parse(webdav_url.trim()).map_err(|e| format!("Invalid WebDAV URL: {e}"))?;

    // If user pastes a file URL, create folders under its parent directory.
    let path0 = base.path().to_string();
    if !path0.ends_with('/') {
        let last0 = path0.rsplit('/').next().unwrap_or("");
        let looks_like_file_url = last0.ends_with(".db") || last0.ends_with(".json") || last0.ends_with(".sqlite");
        if looks_like_file_url {
            {
                let mut segs = base
                    .path_segments_mut()
                    .map_err(|_| "Invalid WebDAV URL (cannot modify path)".to_string())?;
                let _ = segs.pop();
            }
        }
    }

    // Ensure base is a "directory URL" for correct joining.
    let mut p = base.path().to_string();
    if !p.ends_with('/') {
        p.push('/');
        base.set_path(&p);
    }

    // Create folders relative to base (do not attempt to MKCOL the base itself).
    let parts: Vec<&str> = folder.split('/').filter(|s| !s.is_empty()).collect();
    let mut cur = base.clone();
    for part in parts {
        {
            let mut segs = cur
                .path_segments_mut()
                .map_err(|_| "Invalid WebDAV URL (cannot modify path)".to_string())?;
            segs.pop_if_empty();
            segs.push(part);
        }
        let mut pp = cur.path().to_string();
        if !pp.ends_with('/') {
            pp.push('/');
            cur.set_path(&pp);
        }

        let resp = webdav_mkcol(client, settings, cur.as_str()).await?;
        let status = resp.status();
        // Many servers respond 405 if the collection already exists.
        if status.is_success() || status.as_u16() == 405 {
            continue;
        }

        let body = resp.text().await.unwrap_or_default();
        let body = body.trim();
        if body.is_empty() {
            return Err(format!("Failed to create remote directory: {status} ({})", cur.as_str()));
        }
        return Err(format!(
            "Failed to create remote directory: {status} ({}) ({})",
            cur.as_str(),
            &body.chars().take(180).collect::<String>()
        ));
    }

    Ok(())
}

fn webdav_resolve_url_with_folder(
    input: &str,
    folder: Option<&str>,
    filename: &str,
) -> Result<String, String> {
    let raw = input.trim();
    if raw.is_empty() {
        return Err("WebDAV URL not configured".to_string());
    }

    let folder = folder.unwrap_or("").trim().trim_matches('/');

    // If the user already points at a concrete file URL, keep the existing behavior and ignore folder.
    // This avoids surprising "double nesting" for users who paste full file paths.
    let u0 = Url::parse(raw).map_err(|e| format!("Invalid WebDAV URL: {e}"))?;
    let path0 = u0.path().to_string();
    let last0 = path0.rsplit('/').next().unwrap_or("");
    let looks_like_file_url = last0.ends_with(".db") || last0.ends_with(".json") || last0.ends_with(".sqlite");
    if looks_like_file_url {
        return webdav_resolve_url(raw, filename);
    }

    let mut url = Url::parse(raw).map_err(|e| format!("Invalid WebDAV URL: {e}"))?;
    url.path_segments_mut()
        .map_err(|_| "Invalid WebDAV URL (cannot modify path)".to_string())?
        .pop_if_empty();

    if !folder.is_empty() {
        url.path_segments_mut()
            .map_err(|_| "Invalid WebDAV URL (cannot modify path)".to_string())?
            .push(folder);
    }

    url.path_segments_mut()
        .map_err(|_| "Invalid WebDAV URL (cannot modify path)".to_string())?
        .push(filename);

    Ok(url.to_string())
}

#[tauri::command]
async fn webdav_pull() -> Result<(), String> {
    let settings = settings_load()?;
    let webdav_url = settings.webdav_url.ok_or("WebDAV URL not configured")?;

    let client = reqwest::Client::new();
    // Prefer syncing the SQLite single-file DB.
    let url_db = webdav_resolve_url_with_folder(&webdav_url, settings.webdav_folder.as_deref(), "hosts.db")?;
    let mut request = client.get(&url_db);

    if let (Some(username), Some(password)) =
        (&settings.webdav_username, &settings.webdav_password)
    {
        request = request.basic_auth(username, Some(password));
    }

    let response = request.send().await.map_err(|e| e.to_string())?;
    let status = response.status();
    if status.as_u16() == 404 {
        // Backward compat: try legacy hosts.json and import into SQLite.
        let url_json = webdav_resolve_url_with_folder(&webdav_url, settings.webdav_folder.as_deref(), "hosts.json")?;
        let mut r2 = client.get(&url_json);
        if let (Some(username), Some(password)) = (&settings.webdav_username, &settings.webdav_password) {
            r2 = r2.basic_auth(username, Some(password));
        }
        let resp2 = r2.send().await.map_err(|e| e.to_string())?;
        if !resp2.status().is_success() {
            let s2 = resp2.status();
            let body = resp2.text().await.unwrap_or_default();
            let body = body.trim();
            if body.is_empty() {
                return Err(format!("Pull failed: {s2}"));
            }
            return Err(format!(
                "Pull failed: {s2} ({})",
                &body.chars().take(180).collect::<String>()
            ));
        }
        let content = resp2.text().await.map_err(|e| e.to_string())?;
        let hosts: Vec<Host> = serde_json::from_str(&content).map_err(|e| e.to_string())?;
        let mut conn = open_hosts_db()?;
        import_hosts_json_to_db(&mut conn, hosts)?;
        let _ = generate_ssh_config(hosts_load()?);
        return Ok(());
    }
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        let body = body.trim();
        if body.is_empty() {
            return Err(format!("Pull failed: {status}"));
        }
        return Err(format!(
            "Pull failed: {status} ({})",
            &body.chars().take(180).collect::<String>()
        ));
    }

    let bytes = response.bytes().await.map_err(|e| e.to_string())?;

    let backup_path = get_hosts_db_path();
    if backup_path.exists() {
        let timestamp = chrono::Utc::now().format("%Y%m%d%H%M%S");
        let backup = backup_path.with_extension(format!("db.bak.{}", timestamp));
        let _ = fs::copy(&backup_path, &backup);
    }

    fs::write(&backup_path, bytes).map_err(|e| e.to_string())?;
    // Refresh ssh_config to match pulled DB.
    let _ = generate_ssh_config(hosts_load()?);
    Ok(())
}

#[tauri::command]
async fn webdav_push() -> Result<(), String> {
    let settings = settings_load()?;
    let webdav_url = settings
        .webdav_url
        .clone()
        .ok_or("WebDAV URL not configured")?;

    let hosts_path = get_hosts_db_path();
    if !hosts_path.exists() {
        // Ensure DB exists so push can succeed even right after install.
        let _ = hosts_load()?;
    }
    let content = fs::read(&hosts_path).map_err(|e| e.to_string())?;

    let client = reqwest::Client::new();
    let url_db =
        webdav_resolve_url_with_folder(&webdav_url, settings.webdav_folder.as_deref(), "hosts.db")?;
    // Some WebDAV providers (e.g. Jianguoyun) return 404/409 on PUT when the parent collection is missing.
    // Ensure the configured folder exists first.
    webdav_ensure_remote_folder(&client, &settings, &webdav_url, settings.webdav_folder.as_deref()).await?;
    let mut request = client
        .put(&url_db)
        .body(content);

    if let (Some(username), Some(password)) =
        (&settings.webdav_username, &settings.webdav_password)
    {
        request = request.basic_auth(username, Some(password));
    }

    let response = request.send().await.map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        let body = body.trim();
        if body.is_empty() {
            return Err(format!("Push failed: {status} ({url_db})"));
        }
        return Err(format!(
            "Push failed: {status} ({url_db}) ({})",
            &body.chars().take(180).collect::<String>()
        ));
    }

    Ok(())
}

#[tauri::command]
fn host_password_get(host_id: String) -> Result<Option<String>, String> {
    if host_id.trim().is_empty() {
        return Ok(None);
    }
    let entry = keychain_entry(host_id.trim())?;
    match entry.get_password() {
        Ok(pw) => Ok(Some(pw)),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
fn host_password_set(host_id: String, password: String) -> Result<(), String> {
    let id = host_id.trim();
    if id.is_empty() {
        return Err("host_id is required".to_string());
    }
    let pw = password.trim();
    if pw.is_empty() {
        // Treat empty as delete.
        return keychain_delete_password(id);
    }
    keychain_set_password(id, pw).map_err(|e| format!("Failed to save password to Keychain: {e}"))
}

#[tauri::command]
fn host_password_delete(host_id: String) -> Result<(), String> {
    let id = host_id.trim();
    if id.is_empty() {
        return Err("host_id is required".to_string());
    }
    keychain_delete_password(id)
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
            host_password_get,
            host_password_set,
            host_password_delete,
            crate::pty::pty_spawn,
            crate::pty::pty_write,
            crate::pty::pty_resize,
            crate::pty::pty_kill,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
