use keyring::{Entry, Error as KeyringError};

const HOST_PASSWORD_SERVICE: &str = "xTermius";
const WEBDAV_PASSWORD_ACCOUNT: &str = "webdav-sync";

fn keychain_entry(host_id: &str) -> Result<Entry, String> {
    Entry::new(HOST_PASSWORD_SERVICE, host_id).map_err(|e| e.to_string())
}

pub(crate) fn keychain_set_password(host_id: &str, password: &str) -> Result<(), String> {
    let entry = keychain_entry(host_id)?;
    // Some keychain backends do not reliably replace existing entries in-place.
    // Best-effort delete first makes password updates deterministic.
    match entry.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => {}
        Err(e) => return Err(e.to_string()),
    }
    entry.set_password(password).map_err(|e| e.to_string())
}

pub(crate) fn keychain_has_password(host_id: &str) -> bool {
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

pub(crate) fn keychain_delete_password(host_id: &str) -> Result<(), String> {
    match keychain_entry(host_id)?.delete_credential() {
        Ok(()) => Ok(()),
        Err(KeyringError::NoEntry) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

pub(crate) fn keychain_get_password(host_id: &str) -> Result<Option<String>, String> {
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
pub fn host_password_set(host_id: String, password: String) -> Result<(), String> {
    let id = host_id.trim();
    if id.is_empty() {
        return Err("host_id is required".to_string());
    }
    let pw = password.trim();
    if pw.is_empty() {
        return keychain_delete_password(id);
    }
    keychain_set_password(id, pw).map_err(|e| format!("Failed to save password to Keychain: {e}"))
}

#[tauri::command]
pub fn host_password_delete(host_id: String) -> Result<(), String> {
    let id = host_id.trim();
    if id.is_empty() {
        return Err("host_id is required".to_string());
    }
    keychain_delete_password(id)
}

pub(crate) fn webdav_password_get() -> Result<Option<String>, String> {
    keychain_get_password(WEBDAV_PASSWORD_ACCOUNT)
}

pub(crate) fn webdav_password_has() -> bool {
    keychain_has_password(WEBDAV_PASSWORD_ACCOUNT)
}

pub(crate) fn webdav_password_set(password: &str) -> Result<(), String> {
    let pw = password.trim();
    if pw.is_empty() {
        return webdav_password_delete();
    }
    keychain_set_password(WEBDAV_PASSWORD_ACCOUNT, pw)
        .map_err(|e| format!("Failed to save WebDAV password to Keychain: {e}"))
}

pub(crate) fn webdav_password_delete() -> Result<(), String> {
    keychain_delete_password(WEBDAV_PASSWORD_ACCOUNT)
}
