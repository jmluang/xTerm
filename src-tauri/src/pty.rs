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

#[tauri::command]
pub async fn pty_spawn<R: Runtime>(
    file: String,
    args: Vec<String>,
    cols: u16,
    rows: u16,
    cwd: Option<String>,
    env: BTreeMap<String, String>,
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
    for (k, v) in env.iter() {
        cmd.env(OsString::from(k), OsString::from(v));
    }

    let mut child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
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
    thread::spawn(move || {
      let mut buf = [0u8; 4096];
      loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let data = String::from_utf8_lossy(&buf[..n]).to_string();
                    // Emit to all windows; frontend filters by session_id.
                    let _ = app_data.emit("pty:data", PtyDataPayload {
                        session_id: id_data.clone(),
                        data,
                    });
                }
                Err(_) => break,
            }
        }
    });

    // Wait thread: emits exit event and removes session from state.
    let app_exit = app.clone();
    let id_exit = id_s.clone();
    let sessions_for_exit = state.sessions.clone();
    thread::spawn(move || {
        let code = child.wait().ok().map(|s| s.exit_code()).unwrap_or(1);
        let _ = app_exit.emit("pty:exit", PtyExitPayload {
            session_id: id_exit.clone(),
            code,
        });
        if let Ok(mut sessions) = sessions_for_exit.lock() {
            sessions.remove(&id);
        }
    });

    Ok(id_s)
}

#[tauri::command]
pub async fn pty_write(session_id: String, data: String, state: tauri::State<'_, PtyState>) -> Result<(), String> {
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
pub async fn pty_resize(session_id: String, cols: u16, rows: u16, state: tauri::State<'_, PtyState>) -> Result<(), String> {
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
