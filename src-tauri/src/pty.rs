use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashMap},
    ffi::OsString,
    io::{Read, Write},
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc, Mutex,
    },
    thread,
};
use tauri::{AppHandle, Emitter, Runtime};

type SessionId = u32;
const AUTO_PASSWORD_TAIL_CHARS: usize = 512;

fn extract_ready_utf8_chunks(pending: &mut Vec<u8>) -> Vec<String> {
    let mut chunks = Vec::new();

    loop {
        if pending.is_empty() {
            break;
        }

        match std::str::from_utf8(pending.as_slice()) {
            Ok(text) => {
                if !text.is_empty() {
                    chunks.push(text.to_string());
                }
                pending.clear();
                break;
            }
            Err(error) => {
                let valid_up_to = error.valid_up_to();
                if let Some(error_len) = error.error_len() {
                    if valid_up_to > 0 {
                        chunks.push(String::from_utf8_lossy(&pending[..valid_up_to]).to_string());
                    }

                    let invalid_end = valid_up_to + error_len;
                    chunks.push(
                        String::from_utf8_lossy(&pending[valid_up_to..invalid_end]).to_string(),
                    );
                    pending.drain(..invalid_end);
                    continue;
                }

                if valid_up_to > 0 {
                    chunks.push(String::from_utf8_lossy(&pending[..valid_up_to]).to_string());
                    pending.drain(..valid_up_to);
                }
                break;
            }
        }
    }

    chunks
}

fn drain_utf8_tail(pending: &mut Vec<u8>) -> Option<String> {
    if pending.is_empty() {
        return None;
    }

    let text = String::from_utf8_lossy(pending.as_slice()).to_string();
    pending.clear();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

pub struct PtyState {
    next_id: AtomicU32,
    sessions: Arc<Mutex<HashMap<SessionId, Arc<Session>>>>,
}

impl Default for PtyState {
    fn default() -> Self {
        Self {
            next_id: AtomicU32::new(1),
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

struct Session {
    master: Mutex<Box<dyn portable_pty::MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    killer: Mutex<Box<dyn portable_pty::ChildKiller + Send + Sync>>,
}

#[derive(Debug, Serialize, Clone)]
pub struct PtyDataPayload {
    pub session_id: String,
    pub data: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct PtyExitPayload {
    pub session_id: String,
    pub code: u32,
}

fn trim_auto_password_tail(tail: &mut String) {
    let len = tail.chars().count();
    if len > AUTO_PASSWORD_TAIL_CHARS {
        *tail = tail.chars().skip(len - AUTO_PASSWORD_TAIL_CHARS).collect();
    }
}

fn should_send_auto_password(tail: &str) -> bool {
    let normalized = tail.replace('\r', "\n");
    let prompt = normalized
        .rsplit('\n')
        .next()
        .unwrap_or("")
        .trim_end()
        .to_ascii_lowercase();
    if prompt.is_empty() || !prompt.ends_with("password:") {
        return false;
    }

    ![
        "new password",
        "retype",
        "confirm",
        "verification",
        "two-step",
        "otp",
        "passphrase",
    ]
    .iter()
    .any(|needle| prompt.contains(needle))
}

fn maybe_send_auto_password(
    session: &Arc<Session>,
    auto_password: &mut Option<String>,
    prompt_tail: &mut String,
    data: &str,
) {
    if auto_password.is_none() {
        return;
    }

    prompt_tail.push_str(data);
    trim_auto_password_tail(prompt_tail);
    if !should_send_auto_password(prompt_tail) {
        return;
    }

    if let Some(password) = auto_password.take() {
        match session.writer.lock() {
            Ok(mut writer) => {
                if let Err(error) = writer.write_all(format!("{password}\n").as_bytes()) {
                    eprintln!("[pty] failed to write saved SSH password: {error}");
                    return;
                }
                if let Err(error) = writer.flush() {
                    eprintln!("[pty] failed to flush saved SSH password: {error}");
                }
            }
            Err(_) => eprintln!("[pty] writer poisoned while sending saved SSH password"),
        }
    }
}

async fn spawn_pty_command<R: Runtime>(
    file: String,
    args: Vec<String>,
    cols: u16,
    rows: u16,
    cwd: Option<String>,
    env: BTreeMap<String, String>,
    auto_password: Option<String>,
    app: AppHandle<R>,
    state: tauri::State<'_, PtyState>,
) -> Result<String, String> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string())?;

    // Take writer + clone reader now; keep master for resize.
    let master = pair.master;
    let writer = master.take_writer().map_err(|e| e.to_string())?;
    let mut reader = master.try_clone_reader().map_err(|e| e.to_string())?;

    let mut cmd = CommandBuilder::new(file);
    cmd.args(args);
    if let Some(cwd) = cwd {
        cmd.cwd(OsString::from(cwd));
    }
    // Ensure SSH always forwards a valid terminal type unless caller overrides it.
    if !env.contains_key("TERM") {
        cmd.env(OsString::from("TERM"), OsString::from("xterm-256color"));
    }
    for (k, v) in env.iter() {
        cmd.env(OsString::from(k), OsString::from(v));
    }

    let mut child = match pair.slave.spawn_command(cmd) {
        Ok(child) => child,
        Err(e) => return Err(e.to_string()),
    };
    let killer = child.clone_killer();

    let id = state.next_id.fetch_add(1, Ordering::Relaxed);
    let id_s = id.to_string();

    let session = Arc::new(Session {
        master: Mutex::new(master),
        writer: Mutex::new(writer),
        killer: Mutex::new(killer),
    });

    {
        let mut sessions = state.sessions.lock().map_err(|_| "PtyState poisoned")?;
        sessions.insert(id, session.clone());
    }

    // Reader thread: blocks on PTY read and emits data events.
    let app_data = app.clone();
    let id_data = id_s.clone();
    let session_for_reader = session.clone();
    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        let mut pending = Vec::new();
        let mut auto_password = auto_password;
        let mut prompt_tail = String::new();
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    pending.extend_from_slice(&buf[..n]);
                    for data in extract_ready_utf8_chunks(&mut pending) {
                        maybe_send_auto_password(
                            &session_for_reader,
                            &mut auto_password,
                            &mut prompt_tail,
                            &data,
                        );
                        // Emit to all windows; frontend filters by session_id.
                        let _ = app_data.emit(
                            "pty:data",
                            PtyDataPayload {
                                session_id: id_data.clone(),
                                data,
                            },
                        );
                    }
                }
                Err(_) => break,
            }
        }

        if let Some(data) = drain_utf8_tail(&mut pending) {
            let _ = app_data.emit(
                "pty:data",
                PtyDataPayload {
                    session_id: id_data.clone(),
                    data,
                },
            );
        }
    });

    // Wait thread: emits exit event and removes session from state.
    let app_exit = app.clone();
    let id_exit = id_s.clone();
    let sessions_for_exit = state.sessions.clone();
    thread::spawn(move || {
        let code = child.wait().ok().map(|s| s.exit_code()).unwrap_or(1);
        let _ = app_exit.emit(
            "pty:exit",
            PtyExitPayload {
                session_id: id_exit.clone(),
                code,
            },
        );
        if let Ok(mut sessions) = sessions_for_exit.lock() {
            sessions.remove(&id);
        }
    });

    Ok(id_s)
}

#[tauri::command]
pub async fn pty_spawn_ssh<R: Runtime>(
    host_id: String,
    cols: u16,
    rows: u16,
    app: AppHandle<R>,
    state: tauri::State<'_, PtyState>,
) -> Result<String, String> {
    let hosts = crate::host_store::hosts_load()?;
    let host = hosts
        .iter()
        .find(|host| host.id == host_id && !host.deleted)
        .cloned()
        .ok_or_else(|| "Host not found".to_string())?;
    crate::ssh_config::generate_ssh_config(hosts)?;

    let ssh_config_path = crate::ssh_config::get_ssh_config_path();
    let target_alias = if host.alias.trim().is_empty() {
        host.hostname.trim().to_string()
    } else {
        host.alias.trim().to_string()
    };
    if target_alias.is_empty() {
        return Err("Host alias or hostname is required".to_string());
    }

    let mut args = vec![
        "-F".to_string(),
        ssh_config_path.to_string_lossy().to_string(),
        "-o".to_string(),
        "ConnectTimeout=10".to_string(),
        "-o".to_string(),
        "ConnectionAttempts=1".to_string(),
    ];
    let env = BTreeMap::new();
    let auto_password = crate::credential_store::keychain_get_password(&host.id)?
        .map(|password| password.trim().to_string())
        .filter(|password| !password.is_empty());
    if auto_password.is_some() {
        args.extend(["-o".to_string(), "BatchMode=no".to_string()]);
    }

    args.push(target_alias);

    spawn_pty_command(
        "/usr/bin/ssh".to_string(),
        args,
        cols,
        rows,
        None,
        env,
        auto_password,
        app,
        state,
    )
    .await
}

#[tauri::command]
pub async fn pty_write(
    session_id: String,
    data: String,
    state: tauri::State<'_, PtyState>,
) -> Result<(), String> {
    let id: u32 = session_id.parse().map_err(|_| "invalid session_id")?;
    let session = {
        let sessions = state.sessions.lock().map_err(|_| "PtyState poisoned")?;
        sessions.get(&id).cloned().ok_or("Unavailable session")?
    };
    let mut w = session.writer.lock().map_err(|_| "writer poisoned")?;
    w.write_all(data.as_bytes()).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn pty_resize(
    session_id: String,
    cols: u16,
    rows: u16,
    state: tauri::State<'_, PtyState>,
) -> Result<(), String> {
    let id: u32 = session_id.parse().map_err(|_| "invalid session_id")?;
    let session = {
        let sessions = state.sessions.lock().map_err(|_| "PtyState poisoned")?;
        sessions.get(&id).cloned().ok_or("Unavailable session")?
    };
    let master = session.master.lock().map_err(|_| "master poisoned")?;
    master
        .resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn pty_kill(session_id: String, state: tauri::State<'_, PtyState>) -> Result<(), String> {
    let id: u32 = session_id.parse().map_err(|_| "invalid session_id")?;
    let session = {
        let sessions = state.sessions.lock().map_err(|_| "PtyState poisoned")?;
        sessions.get(&id).cloned().ok_or("Unavailable session")?
    };
    let mut k = session.killer.lock().map_err(|_| "killer poisoned")?;
    k.kill().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{drain_utf8_tail, extract_ready_utf8_chunks, should_send_auto_password};

    #[test]
    fn keeps_split_utf8_sequence_until_complete() {
        let mut pending = vec![0xE4, 0xB8];
        assert!(extract_ready_utf8_chunks(&mut pending).is_empty());
        assert_eq!(pending, vec![0xE4, 0xB8]);

        pending.extend_from_slice(&[0xAD, b'!']);
        let expected = String::from_utf8(vec![0xE4, 0xB8, 0xAD, b'!']).unwrap();
        assert_eq!(extract_ready_utf8_chunks(&mut pending), vec![expected]);
        assert!(pending.is_empty());
    }

    #[test]
    fn replaces_invalid_bytes_without_losing_following_text() {
        let mut pending = vec![b'A', 0xFF, b'B'];
        let combined = extract_ready_utf8_chunks(&mut pending).join("");
        assert_eq!(combined, format!("A{}B", char::REPLACEMENT_CHARACTER));
        assert!(pending.is_empty());
    }

    #[test]
    fn drains_incomplete_tail_lossily_at_eof() {
        let mut pending = vec![b'A', 0xE4, 0xB8];
        assert_eq!(
            extract_ready_utf8_chunks(&mut pending),
            vec!["A".to_string()]
        );
        assert_eq!(
            drain_utf8_tail(&mut pending),
            Some(String::from_utf8_lossy(&[0xE4, 0xB8]).to_string())
        );
        assert!(pending.is_empty());
    }

    #[test]
    fn auto_password_matches_only_first_password_prompt() {
        assert!(should_send_auto_password("user@example.com's password: "));
        assert!(should_send_auto_password("Password:"));
        assert!(!should_send_auto_password(
            "Two-Step Vertification required\nVerification code: "
        ));
        assert!(!should_send_auto_password("Enter verification password: "));
        assert!(!should_send_auto_password(
            "Enter passphrase for key '/Users/me/.ssh/id_rsa': "
        ));
        assert!(!should_send_auto_password("New password: "));
        assert!(!should_send_auto_password("Confirm password: "));
    }
}
