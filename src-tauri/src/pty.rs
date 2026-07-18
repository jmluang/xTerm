use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashMap},
    ffi::OsString,
    io::{Read, Write},
    sync::{
        atomic::{AtomicU32, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Runtime};

type SessionId = u32;
const AUTO_PASSWORD_TAIL_CHARS: usize = 512;
const AUTO_PASSWORD_ARM_SECONDS: u64 = 15;
// Terminal UI lives in the main window; targeted emits avoid serializing
// PTY traffic for every open window (e.g. the settings window).
const MAIN_WINDOW_LABEL: &str = "main";
const PTY_READ_BUFFER_BYTES: usize = 64 * 1024;
// Cap for how much decoded output a single pty:data event may carry when the
// emitter coalesces backlogged chunks.
const PTY_EMIT_MAX_BATCH_CHARS: usize = 1024 * 1024;

struct PtyOutputDecoder {
    decoder: encoding_rs::Decoder,
}

impl PtyOutputDecoder {
    fn new(label: Option<&str>) -> Self {
        let encoding = label
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .and_then(|value| encoding_rs::Encoding::for_label(value.as_bytes()))
            .unwrap_or(encoding_rs::UTF_8);
        Self {
            decoder: encoding.new_decoder(),
        }
    }

    fn decode(&mut self, bytes: &[u8], last: bool) -> Option<String> {
        let mut output = String::with_capacity(bytes.len().max(8));
        let mut total_read = 0;
        loop {
            let (result, read, _) =
                self.decoder
                    .decode_to_string(&bytes[total_read..], &mut output, last);
            total_read += read;
            match result {
                encoding_rs::CoderResult::InputEmpty => break,
                encoding_rs::CoderResult::OutputFull => output.reserve(bytes.len().max(8)),
            }
        }
        if output.is_empty() {
            None
        } else {
            Some(output)
        }
    }
}

fn extract_ready_output_chunks(
    decoder: &mut PtyOutputDecoder,
    pending: &mut Vec<u8>,
) -> Vec<String> {
    if pending.is_empty() {
        return Vec::new();
    }

    let data = std::mem::take(pending);
    match decoder.decode(&data, false) {
        Some(output) => vec![output],
        None => Vec::new(),
    }
}

fn drain_output_tail(decoder: &mut PtyOutputDecoder, pending: &mut Vec<u8>) -> Option<String> {
    let data = std::mem::take(pending);
    decoder.decode(&data, true)
}

fn parse_env_vars(input: Option<&str>) -> Result<BTreeMap<String, String>, String> {
    let mut env = BTreeMap::new();
    let Some(input) = input else {
        return Ok(env);
    };
    for (index, raw) in input.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!(
                "Invalid env var on line {}: expected KEY=VALUE",
                index + 1
            ));
        };
        let key = key.trim();
        if key.is_empty()
            || !key
                .chars()
                .next()
                .is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic())
            || key
                .chars()
                .any(|ch| !(ch == '_' || ch.is_ascii_alphanumeric()))
        {
            return Err(format!("Invalid env var name on line {}: {key}", index + 1));
        }
        // Values are forwarded via `-o SetEnv=KEY=VALUE`; whitespace or quotes there
        // would be re-tokenized by ssh's config parser and break the connection.
        if value.chars().any(|ch| ch.is_whitespace() || ch == '"' || ch.is_control()) {
            return Err(format!(
                "Invalid env var value on line {}: {key} must not contain whitespace or quotes (one KEY=VALUE per line)",
                index + 1
            ));
        }
        env.insert(key.to_string(), value.to_string());
    }
    Ok(env)
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
    auto_password: Mutex<Option<AutoPasswordState>>,
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

#[derive(Clone, Debug)]
struct AutoPasswordPromptMatcher {
    owners: Vec<String>,
}

impl AutoPasswordPromptMatcher {
    fn new<I>(owners: I) -> Self
    where
        I: IntoIterator<Item = String>,
    {
        let mut normalized = Vec::new();
        for owner in owners {
            let owner = owner.trim().to_ascii_lowercase();
            if owner.is_empty() || normalized.contains(&owner) {
                continue;
            }
            normalized.push(owner);
        }
        Self { owners: normalized }
    }

    fn for_host(host: &crate::models::Host) -> Self {
        let user = host.user.trim();
        let hostname = host.hostname.trim();
        let alias = host.alias.trim();
        let mut owners = Vec::new();
        for target in [hostname, alias] {
            if target.is_empty() {
                continue;
            }
            owners.push(target.to_string());
            if !user.is_empty() {
                owners.push(format!("{user}@{target}"));
            }
        }
        Self::new(owners)
    }

    fn matches(&self, tail: &str) -> bool {
        if self.owners.is_empty() {
            return false;
        }

        let normalized = tail.replace('\r', "\n");
        let prompt = normalized
            .rsplit('\n')
            .next()
            .unwrap_or("")
            .trim_end()
            .to_ascii_lowercase();
        if prompt.is_empty() {
            return false;
        }

        if [
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
        {
            return false;
        }

        let Some(owner) = prompt.strip_suffix("'s password:") else {
            return false;
        };
        self.owners.iter().any(|allowed| allowed == owner.trim())
    }
}

#[derive(Debug)]
struct AutoPasswordState {
    password: Option<String>,
    matcher: AutoPasswordPromptMatcher,
    armed_until: Instant,
    prompt_tail: String,
}

impl AutoPasswordState {
    fn new(password: String, matcher: AutoPasswordPromptMatcher, armed_until: Instant) -> Self {
        Self {
            password: Some(password),
            matcher,
            armed_until,
            prompt_tail: String::new(),
        }
    }

    fn disarm(&mut self) {
        self.password = None;
        self.prompt_tail.clear();
    }

    fn is_armed(&self) -> bool {
        self.password.is_some()
    }

    fn take_password_for_output(&mut self, data: &str, now: Instant) -> Option<String> {
        self.password.as_ref()?;
        if now > self.armed_until {
            self.disarm();
            return None;
        }

        self.prompt_tail.push_str(data);
        trim_auto_password_tail(&mut self.prompt_tail);
        if !self.matcher.matches(&self.prompt_tail) {
            return None;
        }

        let password = self.password.take();
        self.prompt_tail.clear();
        password
    }
}

fn maybe_send_auto_password(session: &Arc<Session>, data: &str) {
    let password = {
        let Ok(mut state) = session.auto_password.lock() else {
            eprintln!("[pty] auto password state poisoned");
            return;
        };
        let password = state
            .as_mut()
            .and_then(|state| state.take_password_for_output(data, Instant::now()));
        if state.as_ref().is_some_and(|state| !state.is_armed()) {
            *state = None;
        }
        password
    };

    if let Some(password) = password {
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
    encoding: Option<String>,
    auto_password: Option<AutoPasswordState>,
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
        auto_password: Mutex::new(auto_password),
    });

    {
        let mut sessions = state.sessions.lock().map_err(|_| "PtyState poisoned")?;
        sessions.insert(id, session.clone());
    }

    // Reader thread: blocks on PTY read, decodes, and hands chunks to the
    // emitter thread. Kept separate so slow event emission never stalls reads.
    let id_data = id_s.clone();
    let session_for_reader = session.clone();
    let mut output_decoder = PtyOutputDecoder::new(encoding.as_deref());
    let (chunk_tx, chunk_rx) = mpsc::channel::<String>();
    let reader_handle = thread::spawn(move || {
        let mut buf = [0u8; PTY_READ_BUFFER_BYTES];
        let mut pending = Vec::new();
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    pending.extend_from_slice(&buf[..n]);
                    for data in extract_ready_output_chunks(&mut output_decoder, &mut pending) {
                        maybe_send_auto_password(&session_for_reader, &data);
                        if chunk_tx.send(data).is_err() {
                            return;
                        }
                    }
                }
                Err(_) => break,
            }
        }

        if let Some(data) = drain_output_tail(&mut output_decoder, &mut pending) {
            let _ = chunk_tx.send(data);
        }
    });

    // Emitter thread: coalesces whatever backlog accumulated while the
    // previous emit was in flight into a single event. Adds no latency for
    // interactive output; batches aggressively under heavy throughput.
    let app_data = app.clone();
    let emitter_handle = thread::spawn(move || {
        while let Ok(first) = chunk_rx.recv() {
            let mut batch = first;
            while batch.len() < PTY_EMIT_MAX_BATCH_CHARS {
                match chunk_rx.try_recv() {
                    Ok(chunk) => batch.push_str(&chunk),
                    Err(_) => break,
                }
            }
            let _ = app_data.emit_to(
                MAIN_WINDOW_LABEL,
                "pty:data",
                PtyDataPayload {
                    session_id: id_data.clone(),
                    data: batch,
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
        let _ = reader_handle.join();
        let _ = emitter_handle.join();
        let _ = app_exit.emit_to(
            MAIN_WINDOW_LABEL,
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
    crate::ssh_config::ensure_ssh_config()?;

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
    let env = parse_env_vars(host.env_vars.as_deref())?;
    for (key, value) in env.iter() {
        args.extend(["-o".to_string(), format!("SetEnv={key}={value}")]);
    }
    let auto_password_state = crate::credential_store::keychain_get_password(&host.id)?
        .map(|password| password.trim().to_string())
        .filter(|password| !password.is_empty())
        .map(|password| {
            AutoPasswordState::new(
                password,
                AutoPasswordPromptMatcher::for_host(&host),
                Instant::now() + Duration::from_secs(AUTO_PASSWORD_ARM_SECONDS),
            )
        });
    if auto_password_state.is_some() {
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
        host.encoding.clone(),
        auto_password_state,
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
    if let Ok(mut auto_password) = session.auto_password.lock() {
        if let Some(state) = auto_password.as_mut() {
            state.disarm();
        }
        *auto_password = None;
    }
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
    use super::{
        drain_output_tail, extract_ready_output_chunks, parse_env_vars, AutoPasswordPromptMatcher,
        AutoPasswordState, PtyOutputDecoder,
    };
    use std::time::{Duration, Instant};

    #[test]
    fn keeps_split_utf8_sequence_until_complete() {
        let mut pending = vec![0xE4, 0xB8];
        let mut decoder = PtyOutputDecoder::new(Some("utf-8"));
        assert!(extract_ready_output_chunks(&mut decoder, &mut pending).is_empty());
        assert!(pending.is_empty());

        pending.extend_from_slice(&[0xAD, b'!']);
        let expected = String::from_utf8(vec![0xE4, 0xB8, 0xAD, b'!']).unwrap();
        assert_eq!(
            extract_ready_output_chunks(&mut decoder, &mut pending),
            vec![expected]
        );
        assert!(pending.is_empty());
    }

    #[test]
    fn replaces_invalid_bytes_without_losing_following_text() {
        let mut pending = vec![b'A', 0xFF, b'B'];
        let mut decoder = PtyOutputDecoder::new(Some("utf-8"));
        let combined = extract_ready_output_chunks(&mut decoder, &mut pending).join("");
        assert_eq!(combined, format!("A{}B", char::REPLACEMENT_CHARACTER));
        assert!(pending.is_empty());
    }

    #[test]
    fn drains_incomplete_tail_lossily_at_eof() {
        let mut pending = vec![b'A', 0xE4, 0xB8];
        let mut decoder = PtyOutputDecoder::new(Some("utf-8"));
        assert_eq!(
            extract_ready_output_chunks(&mut decoder, &mut pending),
            vec!["A".to_string()]
        );
        assert_eq!(
            drain_output_tail(&mut decoder, &mut pending),
            Some(String::from_utf8_lossy(&[0xE4, 0xB8]).to_string())
        );
        assert!(pending.is_empty());
    }

    #[test]
    fn decodes_split_gbk_output() {
        let mut decoder = PtyOutputDecoder::new(Some("gbk"));
        let mut pending = vec![0xD6];
        assert!(extract_ready_output_chunks(&mut decoder, &mut pending).is_empty());
        assert!(pending.is_empty());

        pending.extend_from_slice(&[0xD0, 0xCE, 0xC4]);
        assert_eq!(
            extract_ready_output_chunks(&mut decoder, &mut pending),
            vec!["中文".to_string()]
        );
    }

    #[test]
    fn parses_host_env_vars() {
        let env =
            parse_env_vars(Some("FOO=bar\nEMPTY=\n# ignored\nNAME=value=with=equals")).unwrap();
        assert_eq!(env.get("FOO").map(String::as_str), Some("bar"));
        assert_eq!(env.get("EMPTY").map(String::as_str), Some(""));
        assert_eq!(
            env.get("NAME").map(String::as_str),
            Some("value=with=equals")
        );
        assert!(parse_env_vars(Some("1BAD=value")).is_err());
        assert!(parse_env_vars(Some("BAD-NAME=value")).is_err());
        // Whitespace/quotes in values would be re-tokenized by ssh's SetEnv parsing.
        assert!(parse_env_vars(Some("FOO=some value")).is_err());
        assert!(parse_env_vars(Some("FOO=\"quoted\"")).is_err());
        // The legacy comma-separated single-line format must fail loudly, not
        // silently fold everything into the first variable.
        assert!(parse_env_vars(Some("VAR1=value1, VAR2=value2")).is_err());
    }

    #[test]
    fn auto_password_matches_only_configured_ssh_target_prompt() {
        let matcher = AutoPasswordPromptMatcher::new([
            "example.com".to_string(),
            "user@example.com".to_string(),
        ]);
        assert!(matcher.matches("user@example.com's password: "));
        assert!(matcher.matches("example.com's password: "));
        assert!(!matcher.matches("Password:"));
        assert!(!matcher.matches("Enter password: "));
        assert!(!matcher.matches("user@other.example.com's password: "));
        assert!(!matcher.matches("Two-Step Vertification required\nVerification code: "));
        assert!(!matcher.matches("Enter verification password: "));
        assert!(!matcher.matches("Enter passphrase for key '/Users/me/.ssh/id_rsa': "));
        assert!(!matcher.matches("New password: "));
        assert!(!matcher.matches("Confirm password: "));
    }

    #[test]
    fn auto_password_disarms_after_user_input_or_expiry() {
        let matcher = AutoPasswordPromptMatcher::new(["user@example.com".to_string()]);
        let now = Instant::now();
        let mut state = AutoPasswordState::new(
            "secret".to_string(),
            matcher.clone(),
            now + Duration::from_secs(5),
        );

        state.disarm();
        assert_eq!(
            state.take_password_for_output("user@example.com's password: ", now),
            None
        );

        let mut expired =
            AutoPasswordState::new("secret".to_string(), matcher, now + Duration::from_secs(5));
        assert_eq!(
            expired.take_password_for_output(
                "user@example.com's password: ",
                now + Duration::from_secs(6)
            ),
            None
        );
    }
}
