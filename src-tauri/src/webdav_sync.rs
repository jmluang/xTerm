use crate::host_store::{
    get_hosts_db_path, hosts_load, import_hosts_json_to_db, open_hosts_db, settings_load,
};
use crate::models::{Host, Settings};
use crate::ssh_config::generate_ssh_config;
use crate::webdav_url::webdav_resolve_url_with_folder;
use reqwest::Method;
use std::fs;
use std::path::PathBuf;
use url::Url;

async fn webdav_mkcol(
    client: &reqwest::Client,
    settings: &Settings,
    url: &str,
) -> Result<reqwest::Response, String> {
    let mut req = client.request(Method::from_bytes(b"MKCOL").unwrap(), url);
    if let (Some(username), Some(password)) = (&settings.webdav_username, &settings.webdav_password)
    {
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

    let path0 = base.path().to_string();
    if !path0.ends_with('/') {
        let last0 = path0.rsplit('/').next().unwrap_or("");
        let looks_like_file_url =
            last0.ends_with(".db") || last0.ends_with(".json") || last0.ends_with(".sqlite");
        if looks_like_file_url {
            {
                let mut segs = base
                    .path_segments_mut()
                    .map_err(|_| "Invalid WebDAV URL (cannot modify path)".to_string())?;
                let _ = segs.pop();
            }
        }
    }

    let mut p = base.path().to_string();
    if !p.ends_with('/') {
        p.push('/');
        base.set_path(&p);
    }

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
        if status.is_success() || status.as_u16() == 405 {
            continue;
        }

        let body = resp.text().await.unwrap_or_default();
        let body = body.trim();
        if body.is_empty() {
            return Err(format!(
                "Failed to create remote directory: {status} ({})",
                cur.as_str()
            ));
        }
        return Err(format!(
            "Failed to create remote directory: {status} ({}) ({})",
            cur.as_str(),
            &body.chars().take(180).collect::<String>()
        ));
    }

    Ok(())
}

fn hosts_db_sidecar_paths() -> Vec<PathBuf> {
    let db = get_hosts_db_path();
    let db_str = db.to_string_lossy().to_string();
    vec![
        PathBuf::from(format!("{db_str}-wal")),
        PathBuf::from(format!("{db_str}-shm")),
    ]
}

#[tauri::command]
pub async fn webdav_pull() -> Result<(), String> {
    let settings = settings_load()?;
    let webdav_url = settings.webdav_url.ok_or("WebDAV URL not configured")?;

    let client = reqwest::Client::new();
    let url_json = webdav_resolve_url_with_folder(
        &webdav_url,
        settings.webdav_folder.as_deref(),
        "hosts.json",
    )?;
    let mut request = client.get(&url_json);

    if let (Some(username), Some(password)) = (&settings.webdav_username, &settings.webdav_password)
    {
        request = request.basic_auth(username, Some(password));
    }

    let response = request.send().await.map_err(|e| e.to_string())?;
    let status = response.status();
    if status.is_success() {
        let content = response.text().await.map_err(|e| e.to_string())?;
        let hosts: Vec<Host> = serde_json::from_str(&content).map_err(|e| e.to_string())?;
        let mut conn = open_hosts_db()?;
        import_hosts_json_to_db(&mut conn, hosts)?;
        let _ = generate_ssh_config(hosts_load()?);
        return Ok(());
    }

    if status.as_u16() == 404 {
        let url_db =
            webdav_resolve_url_with_folder(&webdav_url, settings.webdav_folder.as_deref(), "hosts.db")?;
        let mut r2 = client.get(&url_db);
        if let (Some(username), Some(password)) =
            (&settings.webdav_username, &settings.webdav_password)
        {
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
        let bytes = resp2.bytes().await.map_err(|e| e.to_string())?;

        let backup_path = get_hosts_db_path();
        if backup_path.exists() {
            let timestamp = chrono::Utc::now().format("%Y%m%d%H%M%S");
            let backup = backup_path.with_extension(format!("db.bak.{}", timestamp));
            let _ = fs::copy(&backup_path, &backup);
        }

        // If the local DB was ever in WAL mode, stale sidecar files can replay local edits
        // (e.g. deletions) after we overwrite the main DB file. Remove them before/after write.
        for p in hosts_db_sidecar_paths() {
            let _ = fs::remove_file(p);
        }
        fs::write(&backup_path, bytes).map_err(|e| e.to_string())?;
        for p in hosts_db_sidecar_paths() {
            let _ = fs::remove_file(p);
        }
        let _ = generate_ssh_config(hosts_load()?);
        return Ok(());
    }
    let body = response.text().await.unwrap_or_default();
    let body = body.trim();
    if body.is_empty() {
        return Err(format!("Pull failed: {status}"));
    }
    Err(format!(
        "Pull failed: {status} ({})",
        &body.chars().take(180).collect::<String>()
    ))
}

#[tauri::command]
pub async fn webdav_push() -> Result<(), String> {
    let settings = settings_load()?;
    let webdav_url = settings
        .webdav_url
        .clone()
        .ok_or("WebDAV URL not configured")?;

    let hosts_path = get_hosts_db_path();
    if !hosts_path.exists() {
        let _ = hosts_load()?;
    }
    // Flush WAL into the main DB file so WebDAV uploads the latest state.
    if let Ok(conn) = open_hosts_db() {
        let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
    }
    let content = fs::read(&hosts_path).map_err(|e| e.to_string())?;
    let hosts_json = serde_json::to_vec_pretty(&hosts_load()?).map_err(|e| e.to_string())?;

    let client = reqwest::Client::new();
    let url_db =
        webdav_resolve_url_with_folder(&webdav_url, settings.webdav_folder.as_deref(), "hosts.db")?;
    let url_json =
        webdav_resolve_url_with_folder(&webdav_url, settings.webdav_folder.as_deref(), "hosts.json")?;
    webdav_ensure_remote_folder(
        &client,
        &settings,
        &webdav_url,
        settings.webdav_folder.as_deref(),
    )
    .await?;
    let mut request = client.put(&url_db).body(content);

    if let (Some(username), Some(password)) = (&settings.webdav_username, &settings.webdav_password)
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

    let mut req_json = client.put(&url_json).body(hosts_json);
    if let (Some(username), Some(password)) = (&settings.webdav_username, &settings.webdav_password)
    {
        req_json = req_json.basic_auth(username, Some(password));
    }
    let resp_json = req_json.send().await.map_err(|e| e.to_string())?;
    if !resp_json.status().is_success() {
        let status = resp_json.status();
        let body = resp_json.text().await.unwrap_or_default();
        let body = body.trim();
        if body.is_empty() {
            return Err(format!("Push failed (hosts.json): {status} ({url_json})"));
        }
        return Err(format!(
            "Push failed (hosts.json): {status} ({url_json}) ({})",
            &body.chars().take(180).collect::<String>()
        ));
    }

    Ok(())
}
