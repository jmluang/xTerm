use url::Url;

pub(crate) fn webdav_resolve_url(input: &str, filename: &str) -> Result<String, String> {
    let raw = input.trim();
    if raw.is_empty() {
        return Err("WebDAV URL not configured".to_string());
    }

    let mut url = Url::parse(raw).map_err(|e| format!("Invalid WebDAV URL: {e}"))?;
    let path = url.path().to_string();

    let last_seg = path.rsplit('/').next().unwrap_or("");
    if last_seg == filename {
        return Ok(url.to_string());
    }

    if path.ends_with('/') || last_seg.is_empty() {
        url.path_segments_mut()
            .map_err(|_| "Invalid WebDAV URL (cannot modify path)".to_string())?
            .pop_if_empty()
            .push(filename);
        return Ok(url.to_string());
    }

    let looks_like_file =
        last_seg.ends_with(".db") || last_seg.ends_with(".json") || last_seg.ends_with(".sqlite");

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

pub(crate) fn webdav_resolve_url_with_folder(
    input: &str,
    folder: Option<&str>,
    filename: &str,
) -> Result<String, String> {
    let raw = input.trim();
    if raw.is_empty() {
        return Err("WebDAV URL not configured".to_string());
    }

    let folder = folder.unwrap_or("").trim().trim_matches('/');

    let u0 = Url::parse(raw).map_err(|e| format!("Invalid WebDAV URL: {e}"))?;
    let path0 = u0.path().to_string();
    let last0 = path0.rsplit('/').next().unwrap_or("");
    let looks_like_file_url =
        last0.ends_with(".db") || last0.ends_with(".json") || last0.ends_with(".sqlite");
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

#[cfg(test)]
mod tests {
    use super::{webdav_resolve_url, webdav_resolve_url_with_folder};

    #[test]
    fn resolve_file_path_to_hosts_db() {
        let got =
            webdav_resolve_url("https://dav.example.com/path/custom.json", "hosts.db").unwrap();
        assert!(got.contains("/path/hosts.db"));
    }

    #[test]
    fn resolve_with_folder_uses_folder_for_base_urls() {
        let got = webdav_resolve_url_with_folder(
            "https://dav.example.com/dav/",
            Some("xTermius"),
            "hosts.db",
        )
        .unwrap();
        assert_eq!(got, "https://dav.example.com/dav/xTermius/hosts.db");
    }

    #[test]
    fn resolve_with_folder_keeps_explicit_file_url() {
        let got = webdav_resolve_url_with_folder(
            "https://dav.example.com/dav/current.db",
            Some("ignored"),
            "hosts.db",
        )
        .unwrap();
        assert_eq!(got, "https://dav.example.com/dav/hosts.db");
    }
}
