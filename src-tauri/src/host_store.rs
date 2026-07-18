use crate::credential_store::{
    keychain_delete_password, keychain_has_password, keychain_set_password, webdav_password_delete,
    webdav_password_has, webdav_password_set,
};
use crate::models::{Host, Settings};
use crate::ssh_config::generate_ssh_config;
use rusqlite::{params, Connection};
use std::fs;
use std::path::PathBuf;
use std::sync::Once;

fn get_config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("xtermius")
}

fn get_hosts_path() -> PathBuf {
    get_config_dir().join("hosts.json")
}

pub(crate) fn get_hosts_db_path() -> PathBuf {
    get_config_dir().join("hosts.db")
}

fn get_settings_path() -> PathBuf {
    get_config_dir().join("settings.json")
}

pub(crate) fn ensure_config_dir() -> Result<(), String> {
    let config_dir = get_config_dir();
    if !config_dir.exists() {
        fs::create_dir_all(&config_dir).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Write `contents` to `path` atomically: a concurrent reader (e.g. `ssh -F`)
/// always sees either the old or the new complete file, never a truncated one.
pub(crate) fn atomic_write(path: &std::path::Path, contents: &[u8]) -> Result<(), String> {
    let dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("tmp");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = dir.join(format!(".{file_name}.tmp.{nonce}"));
    if let Err(e) = fs::write(&tmp, contents) {
        let _ = fs::remove_file(&tmp);
        return Err(e.to_string());
    }
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(e.to_string());
    }
    Ok(())
}

pub(crate) fn open_hosts_db() -> Result<Connection, String> {
    ensure_config_dir()?;
    let path = get_hosts_db_path();
    let conn = Connection::open(path).map_err(|e| e.to_string())?;
    // Main and settings windows share this DB from the same process; without a
    // busy timeout a concurrent write surfaces to the user as "database is locked".
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    Ok(conn)
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
          host_insights_enabled INTEGER NOT NULL DEFAULT 1,
          host_live_metrics_enabled INTEGER NOT NULL DEFAULT 1,
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

    let _ = conn.execute(
        "ALTER TABLE hosts ADD COLUMN sort_order INTEGER NOT NULL DEFAULT 0",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE hosts ADD COLUMN has_password INTEGER NOT NULL DEFAULT 0",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE hosts ADD COLUMN host_insights_enabled INTEGER NOT NULL DEFAULT 1",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE hosts ADD COLUMN host_live_metrics_enabled INTEGER NOT NULL DEFAULT 1",
        [],
    );
    Ok(())
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
            let _ = conn.execute(
                "UPDATE hosts SET password = NULL WHERE id = ?1",
                params![id],
            );
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
                eprintln!("[keychain] migrate failed for host {id}: {e}");
            }
        }
    }
    Ok(())
}

pub(crate) fn import_hosts_json_to_db(
    conn: &mut Connection,
    hosts: Vec<Host>,
) -> Result<(), String> {
    ensure_hosts_schema(conn)?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM hosts", [])
        .map_err(|e| e.to_string())?;
    for (i, h) in hosts.into_iter().enumerate() {
        let tags_json = serde_json::to_string(&h.tags).map_err(|e| e.to_string())?;
        let sort_order = h.sort_order.unwrap_or(i as i64);

        let has_password;
        if let Some(ref pw) = h.password {
            let pwt = pw.trim();
            if pwt.is_empty() {
                keychain_delete_password(&h.id)?;
                has_password = false;
            } else {
                keychain_set_password(&h.id, pwt)
                    .map_err(|e| format!("Failed to save password to Keychain: {e}"))?;
                has_password = true;
            }
        } else {
            has_password = keychain_has_password(&h.id);
        }

        tx.execute(
            r#"
            INSERT INTO hosts (
              id, sort_order, name, alias, hostname, user, port,
              password, has_password, host_insights_enabled, host_live_metrics_enabled, identity_file, proxy_jump, env_vars, encoding,
              tags_json, notes, updated_at, deleted
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)
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
                if h.host_insights_enabled { 1 } else { 0 },
                if h.host_live_metrics_enabled { 1 } else { 0 },
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

fn sanitize_hosts_for_frontend(hosts: Vec<Host>) -> Vec<Host> {
    hosts
        .into_iter()
        .map(|mut host| {
            if host
                .password
                .as_ref()
                .map(|password| !password.trim().is_empty())
                .unwrap_or(false)
            {
                host.has_password = true;
            }
            host.password = None;
            host
        })
        .collect()
}

#[tauri::command]
pub fn hosts_load() -> Result<Vec<Host>, String> {
    ensure_config_dir()?;
    let db_path = get_hosts_db_path();
    if !db_path.exists() {
        let json_path = get_hosts_path();
        if json_path.exists() {
            let content = fs::read_to_string(&json_path).map_err(|e| e.to_string())?;
            let hosts: Vec<Host> = serde_json::from_str(&content).map_err(|e| e.to_string())?;
            let mut conn = open_hosts_db()?;
            import_hosts_json_to_db(&mut conn, hosts.clone())?;
            let _ = generate_ssh_config(hosts.clone());
            return Ok(sanitize_hosts_for_frontend(hosts));
        }
    }

    let conn = open_hosts_db()?;
    ensure_hosts_schema(&conn)?;
    // Legacy plaintext-password migration only needs one best-effort pass per
    // process; hosts_load is on hot paths (spawn, probes) where the repeated
    // table scan plus keychain writes would add avoidable latency.
    static PASSWORD_MIGRATION: Once = Once::new();
    PASSWORD_MIGRATION.call_once(|| {
        let _ = migrate_db_passwords_to_keychain(&conn);
    });

    let mut stmt = conn
        .prepare(
            r#"
            SELECT
              id, sort_order, name, alias, hostname, user, port,
              password, has_password, host_insights_enabled, host_live_metrics_enabled, identity_file, proxy_jump, env_vars, encoding,
              tags_json, notes, updated_at, deleted
            FROM hosts
            ORDER BY sort_order ASC, updated_at DESC
            "#,
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map([], |row| {
            let tags_json: String = row.get(15)?;
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
                password: None,
                has_password: keychain_has_password(&id),
                host_insights_enabled: {
                    let v: i64 = row.get(9)?;
                    v != 0
                },
                host_live_metrics_enabled: {
                    let v: i64 = row.get(10)?;
                    v != 0
                },
                identity_file: row.get(11)?,
                proxy_jump: row.get(12)?,
                env_vars: row.get(13)?,
                encoding: row.get(14)?,
                tags,
                notes: row.get(16)?,
                updated_at: row.get(17)?,
                deleted: {
                    let d: i64 = row.get(18)?;
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
pub fn hosts_save(hosts: Vec<Host>) -> Result<(), String> {
    let mut conn = open_hosts_db()?;
    import_hosts_json_to_db(&mut conn, hosts.clone())?;
    generate_ssh_config(hosts)?;
    Ok(())
}

#[tauri::command]
pub fn settings_load() -> Result<Settings, String> {
    ensure_config_dir()?;
    let path = get_settings_path();
    if !path.exists() {
        return Ok(Settings {
            webdav_url: None,
            webdav_folder: Some("xTermius".to_string()),
            webdav_username: None,
            has_webdav_password: webdav_password_has(),
            webdav_password: None,
            webdav_password_clear: false,
        });
    }
    let content = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut settings: Settings = serde_json::from_str(&content).map_err(|e| e.to_string())?;
    if let Some(password) = settings
        .webdav_password
        .as_ref()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
    {
        webdav_password_set(password)?;
        settings.webdav_password = None;
        settings.has_webdav_password = webdav_password_has();
        settings.webdav_password_clear = false;
        let content = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
        atomic_write(&path, content.as_bytes())?;
    }
    if settings.webdav_folder.is_none() {
        settings.webdav_folder = Some("xTermius".to_string());
    }
    settings.has_webdav_password = webdav_password_has();
    settings.webdav_password = None;
    settings.webdav_password_clear = false;
    Ok(settings)
}

#[tauri::command]
pub fn settings_save(mut settings: Settings) -> Result<(), String> {
    ensure_config_dir()?;
    if settings.webdav_password_clear {
        webdav_password_delete()?;
    } else if let Some(password) = settings
        .webdav_password
        .as_ref()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
    {
        webdav_password_set(password)?;
    }
    settings.has_webdav_password = webdav_password_has();
    settings.webdav_password = None;
    settings.webdav_password_clear = false;
    let path = get_settings_path();
    let content = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    atomic_write(&path, content.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::sanitize_hosts_for_frontend;
    use crate::models::Host;

    #[test]
    fn sanitize_hosts_for_frontend_removes_plaintext_passwords() {
        let hosts = vec![Host {
            id: "1".to_string(),
            sort_order: Some(0),
            name: "prod".to_string(),
            alias: "prod".to_string(),
            hostname: "example.com".to_string(),
            user: "root".to_string(),
            port: 22,
            password: Some("secret".to_string()),
            has_password: false,
            host_insights_enabled: true,
            host_live_metrics_enabled: true,
            identity_file: None,
            proxy_jump: None,
            env_vars: None,
            encoding: Some("utf-8".to_string()),
            tags: vec![],
            notes: "".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            deleted: false,
        }];

        let sanitized = sanitize_hosts_for_frontend(hosts);
        assert_eq!(sanitized[0].password, None);
        assert!(sanitized[0].has_password);
    }
}
