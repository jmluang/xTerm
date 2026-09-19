//! Managed OpenSSH connection reuse (MCP V1 phase A, ORQ-28).
//!
//! Every user-initiated SSH connection carries a managed ControlMaster entry:
//! a control socket inside an app-private `0700` directory with an
//! unpredictable, short name (a token with 128 bits of entropy — macOS caps
//! unix socket paths at 104 bytes, and random names also stop other local
//! processes from pre-creating or guessing the entry).
//!
//! Security properties:
//! - The master is the interactive ssh process the user authenticated. We
//!   never persist it (`ControlPersist=no`), so a closed tab cannot stay
//!   operable through a lingering background master.
//! - Readiness is proven by `ssh -S <sock> -O check <target>`, which only
//!   succeeds against a live, authenticated master — never by socket file
//!   existence, process liveness, or terminal output.
//! - Derived command channels run with `ControlMaster=no` + the managed
//!   `ControlPath` + `BatchMode=yes`. With no other auth path available, a
//!   missing/stale/refused socket fails closed; ssh cannot fall back to a
//!   fresh login. Any caller-supplied ssh options are rejected.
//! - Stale sockets from crashed runs are removed at startup; names embed the
//!   pid so other app instances and the host_probe mux are never touched.

use crate::connection_registry::{ConnectionRecord, ConnectionState};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const SSH_BIN: &str = "/usr/bin/ssh";
/// Upper bound on readiness polling right after spawn (auth may take a while:
/// password prompts, host-key confirmation, MFA all happen interactively).
const READY_POLL_TIMEOUT: Duration = Duration::from_secs(120);
const READY_POLL_INTERVAL: Duration = Duration::from_millis(500);
const MUX_OP_TIMEOUT: Duration = Duration::from_secs(10);

pub struct MuxManager {
    dir: PathBuf,
    ssh_config: PathBuf,
    pid: u32,
    sockets: Mutex<HashMap<String, PathBuf>>,
}

impl MuxManager {
    pub fn new() -> Result<Self, String> {
        let ssh_config = crate::ssh_config::get_ssh_config_path();
        let dir = ssh_config
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("mux");
        Self::init_dir(&dir)?;
        let manager = Self {
            dir,
            ssh_config,
            pid: std::process::id(),
            sockets: Mutex::new(HashMap::new()),
        };
        manager.cleanup_stale();
        Ok(manager)
    }

    #[cfg(test)]
    pub(crate) fn for_test(dir: PathBuf, ssh_config: PathBuf) -> Self {
        let _ = Self::init_dir(&dir);
        Self {
            dir,
            ssh_config,
            pid: std::process::id(),
            sockets: Mutex::new(HashMap::new()),
        }
    }

    #[cfg(unix)]
    fn init_dir(dir: &Path) -> Result<(), String> {
        use std::os::unix::fs::PermissionsExt;
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())
    }

    #[cfg(not(unix))]
    fn init_dir(dir: &Path) -> Result<(), String> {
        fs::create_dir_all(dir).map_err(|e| e.to_string())
    }

    /// Remove sockets left by previous, now-dead instances of this app. The
    /// `xt<pid>-<token>` naming means probe muxes and other apps' sockets are
    /// never matched.
    fn cleanup_stale(&self) {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let Some(rest) = name.strip_prefix("xt") else { continue };
            let pid_part = rest.split('-').next().unwrap_or("");
            let Ok(pid) = pid_part.parse::<u32>() else { continue };
            if pid == self.pid || pid_is_alive(pid) {
                continue;
            }
            let _ = fs::remove_file(entry.path());
        }
    }

    /// Allocate a fresh, unpredictable socket path for a spawn attempt. The
    /// file does not exist until the ssh master binds it.
    pub fn prepare_socket(&self) -> Result<PathBuf, String> {
        let token = uuid::Uuid::new_v4().simple().to_string();
        let path = self.dir.join(format!("xt{}-{}", self.pid, &token[..16]));
        Ok(path)
    }

    /// Extra ssh args that turn the interactive connection into the managed
    /// master. `ControlMaster=yes` (not `auto`): reuse of some other app's
    /// socket must fail, never silently succeed.
    pub fn master_args(&self, socket: &Path) -> Vec<String> {
        vec![
            "-o".to_string(),
            "ControlMaster=yes".to_string(),
            "-o".to_string(),
            "ControlPersist=no".to_string(),
            "-o".to_string(),
            format!("ControlPath={}", socket.to_string_lossy()),
        ]
    }

    /// Track the socket for a connection once it exists on disk.
    pub fn track(&self, connection_id: &str, socket: PathBuf) {
        if let Ok(mut sockets) = self.sockets.lock() {
            sockets.insert(connection_id.to_string(), socket);
        }
    }

    pub fn socket_for(&self, connection_id: &str) -> Option<PathBuf> {
        self.sockets
            .lock()
            .ok()
            .and_then(|sockets| sockets.get(connection_id).cloned())
    }

    /// Fail-closed liveness+authentication proof for the master behind a
    /// registered connection. `-O check` exits 0 only when a live master for
    /// this exact target answers on this exact socket.
    pub fn verify(&self, record: &ConnectionRecord) -> Result<(), MuxError> {
        if record.state != ConnectionState::Ready
            && record.state != ConnectionState::Connecting
        {
            return Err(MuxError::Unavailable("connection is closing or exited".into()));
        }
        let socket = record
            .mux_socket
            .clone()
            .ok_or_else(|| MuxError::Unavailable("no managed mux entry (reconnect to enable MCP)".into()))?;
        if !socket.exists() {
            return Err(MuxError::Unavailable("mux socket missing".into()));
        }
        let output = run_ssh_with_timeout(
            &[
                "-S".to_string(),
                socket.to_string_lossy().to_string(),
                "-O".to_string(),
                "check".to_string(),
                record.host.ssh_target(),
            ],
            MUX_OP_TIMEOUT,
        )?;
        if output.status.success() {
            Ok(())
        } else {
            Err(MuxError::Unavailable(format!(
                "mux check failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )))
        }
    }

    /// Background readiness loop: poll `-O check` until the master answers
    /// (authentication finished) or the timeout elapses. Password/fingerprint
    /// /MFA prompts never produce a Ready state — only the check does. The
    /// probe is bounded so it can never outlive a forgotten connection.
    pub fn spawn_readiness_probe(
        &self,
        registry: std::sync::Arc<crate::connection_registry::ConnectionRegistry>,
        connection_id: String,
    ) {
        let socket = self.socket_for(&connection_id);
        std::thread::spawn(move || {
            let started = Instant::now();
            loop {
                let Some(record) = registry.get(&connection_id) else {
                    return;
                };
                if record.state != ConnectionState::Connecting {
                    return; // closed/exited while waiting; never mark ready late
                }
                let socket = socket.clone().or(record.mux_socket.clone());
                if let Some(socket) = socket {
                    if socket.exists() {
                        if let Ok(output) = run_ssh_with_timeout(
                            &[
                                "-S".to_string(),
                                socket.to_string_lossy().to_string(),
                                "-O".to_string(),
                                "check".to_string(),
                                record.host.ssh_target(),
                            ],
                            MUX_OP_TIMEOUT,
                        ) {
                            if output.status.success() {
                                registry.mark_ready(&connection_id);
                                return;
                            }
                        }
                    }
                }
                if started.elapsed() > READY_POLL_TIMEOUT {
                    return;
                }
                std::thread::sleep(READY_POLL_INTERVAL);
            }
        });
    }

    /// Build a derived command channel over an existing, verified connection.
    ///
    /// Fail-closed by construction:
    /// - the connection must be Ready and generation must match the caller's
    ///   grant, so reconnects invalidate pending work;
    /// - `ControlMaster=no` + managed socket + `BatchMode=yes` + no TTY and
    ///   no askpass env: if the socket is gone/stale/refused, ssh errors out
    ///   instead of opening a new authenticated connection;
    /// - server-side channel limits (e.g. MaxSessions) surface as a failure
    ///   from ssh itself, and we never retry with a fresh connection.
    ///
    /// The command runs via `sh -lc` in a fresh non-interactive session: it
    /// does not inherit the human shell's cwd, exported env, or any nested
    /// ssh target the user may be inside of.
    #[allow(dead_code)] // retained as phase B/C integration surface
    pub fn run_command(
        &self,
        record: &ConnectionRecord,
        command: &str,
    ) -> Result<MuxCommandResult, MuxError> {
        if command.trim().is_empty() {
            return Err(MuxError::Rejected("command is empty".into()));
        }
        if record.state != ConnectionState::Ready {
            return Err(MuxError::Unavailable(
                "connection is not verified ready".into(),
            ));
        }
        self.verify(record)?;
        let socket = record.mux_socket.clone().expect("verified above");

        let mut cmd = Command::new(SSH_BIN);
        cmd.args(self.derived_channel_args(&socket, &record.host.ssh_target(), command));
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        // Strip anything that could re-open an auth path.
        cmd.env_remove("SSH_ASKPASS");
        cmd.env_remove("SSH_ASKPASS_REQUIRE");
        cmd.env_remove("DISPLAY");

        let output = cmd.output().map_err(|e| MuxError::Spawn(e.to_string()))?;
        finish_derived_channel(output)
    }

    /// Args for a derived (slave) channel over the managed mux. Shared with
    /// the phase C task engine, which manages its own streaming/timeout.
    ///
    /// `-T` (no PTY), `ControlMaster=no` (never create a master),
    /// `BatchMode=yes` + `NumberOfPasswordPrompts=0` (never interactively
    /// authenticate): combined with the managed `ControlPath`, the only way
    /// this command can run is through the existing authenticated master.
    pub(crate) fn derived_channel_args(
        &self,
        socket: &Path,
        target: &str,
        command: &str,
    ) -> Vec<String> {
        vec![
            "-F".to_string(),
            self.ssh_config.to_string_lossy().to_string(),
            "-T".to_string(),
            "-o".to_string(),
            "ControlMaster=no".to_string(),
            "-o".to_string(),
            format!("ControlPath={}", socket.to_string_lossy()),
            "-o".to_string(),
            "BatchMode=yes".to_string(),
            "-o".to_string(),
            "NumberOfPasswordPrompts=0".to_string(),
            "-o".to_string(),
            "ConnectTimeout=10".to_string(),
            target.to_string(),
            "sh".to_string(),
            "-lc".to_string(),
            command.to_string(),
        ]
    }

    /// Connection is closing/exited: remove the socket file and forget it.
    /// The master dies with the PTY child; this just cleans the filesystem
    /// entry so nothing can mistake it for usable later.
    pub fn teardown(&self, connection_id: &str) {
        let socket = self
            .sockets
            .lock()
            .ok()
            .and_then(|mut sockets| sockets.remove(connection_id));
        if let Some(socket) = socket {
            let _ = fs::remove_file(socket);
        }
    }
}

#[derive(Debug)]
pub enum MuxError {
    /// The managed entry is missing, dead or refused the channel. Never
    /// retryable by opening a new connection.
    Unavailable(String),
    /// The request itself is invalid (empty command, wrong generation, ...).
    Rejected(String),
    /// Failed to even spawn the ssh client process.
    Spawn(String),
}

impl std::fmt::Display for MuxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MuxError::Unavailable(msg) => write!(f, "mux unavailable: {msg}"),
            MuxError::Rejected(msg) => write!(f, "mux request rejected: {msg}"),
            MuxError::Spawn(msg) => write!(f, "ssh spawn failed: {msg}"),
        }
    }
}

#[allow(dead_code)] // retained as phase B/C integration surface
#[derive(Debug)]
pub struct MuxCommandResult {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Turn raw output from a derived-channel ssh invocation into either a
/// legitimate command result or a fail-closed mux error.
#[allow(dead_code)] // retained as phase B/C integration surface
pub(crate) fn finish_derived_channel(
    output: std::process::Output,
) -> Result<MuxCommandResult, MuxError> {
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if is_mux_failure(&output, &stderr) {
        return Err(MuxError::Unavailable(format!(
            "mux channel refused: {}",
            stderr.trim()
        )));
    }
    Ok(MuxCommandResult {
        exit_code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr,
    })
}

/// Distinguish "the mux refused us" (fail closed) from "the remote command
/// itself failed" (a legitimate result with an exit code).
fn is_mux_failure(output: &std::process::Output, stderr: &str) -> bool {
    if output.status.success() {
        return false;
    }
    let needles = [
        "mux_client_request_session",
        "muxclient: master hello exchange failed",
        "control socket connect",
        "Control socket connect",
        "No such file or directory",
        "Connection refused",
        "Permission denied",
    ];
    let stderr = stderr.trim();
    !stderr.is_empty() && needles.iter().any(|needle| stderr.contains(needle))
}

fn run_ssh_with_timeout(args: &[String], timeout: Duration) -> Result<std::process::Output, MuxError> {
    let mut child = Command::new(SSH_BIN)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| MuxError::Spawn(e.to_string()))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return child
                    .wait_with_output()
                    .map_err(|e| MuxError::Spawn(e.to_string()));
            }
            Ok(None) => {
                if started.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(MuxError::Unavailable("ssh control op timed out".into()));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(MuxError::Spawn(e.to_string())),
        }
    }
}

#[cfg(unix)]
fn pid_is_alive(pid: u32) -> bool {
    // kill(pid, 0): alive if Ok or EPERM.
    unsafe { libc_kill(pid as i32, 0) == 0 || std::io::Error::last_os_error().raw_os_error() == Some(1) }
}

#[cfg(not(unix))]
fn pid_is_alive(_pid: u32) -> bool {
    true
}

#[cfg(unix)]
extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}


#[cfg(test)]
mod tests {
    use super::*;

    fn test_manager(tag: &str) -> MuxManager {
        let dir = std::env::temp_dir().join(format!("xt-mux-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        MuxManager::for_test(dir, PathBuf::from("/tmp/ssh_config"))
    }

    #[test]
    #[cfg(unix)]
    fn mux_dir_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let manager = test_manager("perms");
        let mode = fs::metadata(&manager.dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn socket_names_are_short_unpredictable_and_pid_scoped() {
        let manager = test_manager("names");
        let a = manager.prepare_socket().unwrap();
        let b = manager.prepare_socket().unwrap();
        assert_ne!(a, b);
        let name = a.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with(&format!("xt{}-", manager.pid)));
        // Stay well under the 104-byte sun_path limit with headroom.
        assert!(a.to_string_lossy().len() < 100);
        // Unrelated instances (different pid prefix) cannot collide.
        assert!(!name.contains("probe_mux"));
    }

    #[test]
    fn stale_sockets_from_dead_pids_are_cleaned() {
        let manager = test_manager("cleanup");
        // A socket left by a pid that cannot be alive.
        let stale = manager.dir.join("xt99999999-deadbeefdeadbeef");
        fs::write(&stale, b"").unwrap();
        // A socket owned by this pid must be kept.
        let ours = manager.dir.join(format!("xt{}-aaaabbbbccccdddd", manager.pid));
        fs::write(&ours, b"").unwrap();
        // A probe socket must never be touched.
        let probe = manager.dir.join("probe_mux_abc");
        fs::write(&probe, b"").unwrap();

        manager.cleanup_stale();
        assert!(!stale.exists());
        assert!(ours.exists());
        assert!(probe.exists());
    }

    #[test]
    fn master_args_force_exclusive_managed_master() {
        let manager = test_manager("args");
        let args = manager.master_args(Path::new("/tmp/s"));
        assert!(args.iter().any(|a| a == "ControlMaster=yes"));
        assert!(args.iter().any(|a| a == "ControlPersist=no"));
        assert!(args.iter().any(|a| a == "ControlPath=/tmp/s"));
        assert!(!args.iter().any(|a| a.contains("auto")));
    }

    #[test]
    fn derived_channel_args_fail_closed() {
        let manager = test_manager("derived");
        let args = manager.derived_channel_args(Path::new("/tmp/s"), "prod", "uptime");
        // Never creates a master, never authenticates, only the managed path.
        assert!(args.iter().any(|a| a == "ControlMaster=no"));
        assert!(args.iter().any(|a| a == "BatchMode=yes"));
        assert!(args.iter().any(|a| a == "NumberOfPasswordPrompts=0"));
        assert!(args.iter().any(|a| a == "ControlPath=/tmp/s"));
        assert!(args.iter().any(|a| a == "-T"));
        assert_eq!(args.last().map(String::as_str), Some("uptime"));
    }

    #[test]
    fn missing_socket_fails_closed() {
        let manager = test_manager("missing");
        let record = crate::connection_registry::ConnectionRecord {
            connection_id: "c".into(),
            generation: 1,
            pty_session_id: "s".into(),
            host: crate::connection_registry::HostSnapshot {
                host_id: "h".into(),
                alias: "prod".into(),
                hostname: "example.com".into(),
                user: "root".into(),
                port: 22,
                proxy_jump: None,
                has_identity_file: false,
                encoding: None,
            },
            state: ConnectionState::Ready,
            mux_socket: Some(PathBuf::from("/nonexistent/xt1-deadbeef")),
            created_at_ms: 0,
            ready_at_ms: Some(0),
            closed_at_ms: None,
            exit_code: None,
        };
        let err = manager.verify(&record).unwrap_err();
        assert!(matches!(err, MuxError::Unavailable(_)));
        let err = manager.run_command(&record, "uptime").unwrap_err();
        assert!(matches!(err, MuxError::Unavailable(_)));
    }

    #[test]
    fn unready_or_exited_connections_are_rejected() {
        let manager = test_manager("state");
        let mut record = crate::connection_registry::ConnectionRecord {
            connection_id: "c".into(),
            generation: 1,
            pty_session_id: "s".into(),
            host: crate::connection_registry::HostSnapshot {
                host_id: "h".into(),
                alias: "prod".into(),
                hostname: "example.com".into(),
                user: "root".into(),
                port: 22,
                proxy_jump: None,
                has_identity_file: false,
                encoding: None,
            },
            state: ConnectionState::Connecting,
            mux_socket: Some(PathBuf::from("/tmp/x")),
            created_at_ms: 0,
            ready_at_ms: None,
            closed_at_ms: None,
            exit_code: None,
        };
        assert!(matches!(
            manager.run_command(&record, "x").unwrap_err(),
            MuxError::Unavailable(_)
        ));
        record.state = ConnectionState::Exited;
        assert!(matches!(
            manager.verify(&record).unwrap_err(),
            MuxError::Unavailable(_)
        ));
        assert!(matches!(
            manager.run_command(&record, "x").unwrap_err(),
            MuxError::Unavailable(_)
        ));
    }

    #[test]
    fn mux_failure_detection_matches_real_ssh_errors() {
        use std::os::unix::process::ExitStatusExt;
        let failed = std::process::Output {
            status: std::process::ExitStatus::from_raw(255 << 8),
            stdout: vec![],
            stderr: b"mux_client_request_session: read from master failed: Broken pipe".to_vec(),
        };
        assert!(is_mux_failure(&failed, "mux_client_request_session: read from master failed: Broken pipe"));
        // A remote command that simply exits 1 is NOT a mux failure.
        let remote_fail = std::process::Output {
            status: std::process::ExitStatus::from_raw(1 << 8),
            stdout: vec![],
            stderr: b"sh: nosuchcmd: command not found".to_vec(),
        };
        assert!(!is_mux_failure(&remote_fail, "sh: nosuchcmd: command not found"));
    }
}
