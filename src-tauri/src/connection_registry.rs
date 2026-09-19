//! Backend-authoritative connection registry (MCP V1 phase A, ORQ-28).
//!
//! The frontend owns display state ("running" tabs); this registry owns the
//! security-relevant truth: which SSH connections exist, which host snapshot
//! they were opened for, whether a managed mux entry exists, and whether the
//! connection is currently allowed to back MCP operations.

use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::models::Host;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    /// PTY spawned; SSH may still be negotiating, prompting for a password,
    /// waiting on host-key confirmation or MFA. Never MCP-usable here:
    /// first output bytes prove nothing about authentication.
    Connecting,
    /// The managed mux entry answered `ssh -O check`, proving the master
    /// connection for this exact generation is alive and authenticated.
    Ready,
    /// Close requested. No new derived channels; grants revoked first.
    Closing,
    /// Child exited. Inert record kept for diagnostics; never revivable.
    Exited,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostSnapshot {
    pub host_id: String,
    pub alias: String,
    pub hostname: String,
    pub user: String,
    pub port: u16,
    pub proxy_jump: Option<String>,
    pub has_identity_file: bool,
    pub encoding: Option<String>,
}

impl HostSnapshot {
    pub fn from_host(host: &Host) -> Self {
        Self {
            host_id: host.id.clone(),
            alias: host.alias.trim().to_string(),
            hostname: host.hostname.trim().to_string(),
            user: host.user.trim().to_string(),
            port: host.port,
            proxy_jump: host
                .proxy_jump
                .as_ref()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty()),
            has_identity_file: host
                .identity_file
                .as_ref()
                .map(|v| !v.trim().is_empty())
                .unwrap_or(false),
            encoding: host.encoding.clone(),
        }
    }

    /// The ssh config alias/hostname the connection was opened with. Derived
    /// channels must use this snapshot, not the live host config, so a later
    /// config edit cannot redirect an existing grant to another machine.
    pub fn ssh_target(&self) -> String {
        if self.alias.is_empty() {
            self.hostname.clone()
        } else {
            self.alias.clone()
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionRecord {
    pub connection_id: String,
    pub generation: u64,
    pub pty_session_id: String,
    pub host: HostSnapshot,
    pub state: ConnectionState,
    /// Managed mux control socket owned by this app instance. Connections
    /// without one (spawned by an older build) never gain it implicitly.
    pub mux_socket: Option<PathBuf>,
    pub created_at_ms: u64,
    pub ready_at_ms: Option<u64>,
    pub closed_at_ms: Option<u64>,
    pub exit_code: Option<u32>,
}

impl ConnectionRecord {
    /// MCP may only touch connections carrying a managed mux entry created
    /// by this app instance that are not closing/exited.
    pub fn mcp_eligible(&self) -> bool {
        self.mux_socket.is_some()
            && matches!(
                self.state,
                ConnectionState::Connecting | ConnectionState::Ready
            )
    }

    /// Live connection without a managed entry: the UI can say "reconnect to
    /// enable MCP" instead of silently re-authenticating in the background.
    pub fn requires_reconnect_for_mcp(&self) -> bool {
        self.mux_socket.is_none()
            && matches!(
                self.state,
                ConnectionState::Connecting | ConnectionState::Ready
            )
    }

    /// Minimal metadata safe to hand to an authorized MCP client. Never
    /// includes socket paths or anything usable to bypass the registry.
    pub fn mcp_summary(&self) -> McpConnectionSummary {
        McpConnectionSummary {
            connection_id: self.connection_id.clone(),
            generation: self.generation,
            host_name: self.host.hostname.clone(),
            user: self.host.user.clone(),
            port: self.host.port,
            alias: self.host.alias.clone(),
            state: self.state,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpConnectionSummary {
    pub connection_id: String,
    pub generation: u64,
    pub host_name: String,
    pub user: String,
    pub port: u16,
    pub alias: String,
    pub state: ConnectionState,
}

struct RegistryInner {
    by_connection: HashMap<String, ConnectionRecord>,
    by_session: HashMap<String, String>,
}

pub struct ConnectionRegistry {
    inner: Mutex<RegistryInner>,
    next_generation: AtomicU64,
}

impl Default for ConnectionRegistry {
    fn default() -> Self {
        Self {
            inner: Mutex::new(RegistryInner {
                by_connection: HashMap::new(),
                by_session: HashMap::new(),
            }),
            next_generation: AtomicU64::new(1),
        }
    }
}

impl ConnectionRegistry {
    /// Register a freshly spawned connection. `connection_id` must be a fresh
    /// UUID generated by the backend for this spawn attempt.
    pub fn register(
        &self,
        connection_id: String,
        pty_session_id: String,
        host: HostSnapshot,
        mux_socket: Option<PathBuf>,
    ) -> Result<ConnectionRecord, String> {
        if connection_id.trim().is_empty() || pty_session_id.trim().is_empty() {
            return Err("connection_id and pty_session_id are required".to_string());
        }
        let generation = self.next_generation.fetch_add(1, Ordering::SeqCst);
        let record = ConnectionRecord {
            connection_id: connection_id.clone(),
            generation,
            pty_session_id: pty_session_id.clone(),
            host,
            state: ConnectionState::Connecting,
            mux_socket,
            created_at_ms: now_ms(),
            ready_at_ms: None,
            closed_at_ms: None,
            exit_code: None,
        };
        let mut inner = self.inner.lock().map_err(|_| "registry poisoned")?;
        if inner.by_session.contains_key(&pty_session_id)
            || inner.by_connection.contains_key(&connection_id)
        {
            return Err("connection or session id already registered".to_string());
        }
        inner
            .by_connection
            .insert(connection_id.clone(), record.clone());
        inner.by_session.insert(pty_session_id, connection_id);
        Ok(record)
    }

    #[allow(dead_code)] // retained as phase B/C integration surface
    pub fn attach_mux_socket(
        &self,
        connection_id: &str,
        socket: PathBuf,
    ) -> Option<ConnectionRecord> {
        let mut inner = self.inner.lock().ok()?;
        let record = inner.by_connection.get_mut(connection_id)?;
        if matches!(
            record.state,
            ConnectionState::Closing | ConnectionState::Exited
        ) {
            return None;
        }
        record.mux_socket = Some(socket);
        Some(record.clone())
    }

    /// Mark Ready. Only valid from `Connecting`, and only after the mux layer
    /// proved the master alive and authenticated for this generation. A late
    /// call after close/exit is ignored so stale probes never resurrect a
    /// dead record.
    pub fn mark_ready(&self, connection_id: &str) -> Option<ConnectionRecord> {
        let mut inner = self.inner.lock().ok()?;
        let record = inner.by_connection.get_mut(connection_id)?;
        if record.state != ConnectionState::Connecting {
            return None;
        }
        record.state = ConnectionState::Ready;
        record.ready_at_ms = Some(now_ms());
        Some(record.clone())
    }

    /// A previously verified connection dropped out of Ready (a later mux
    /// check failed). No new derived channel may start until a fresh
    /// verification marks it Ready again.
    #[allow(dead_code)] // retained as phase B/C integration surface
    pub fn mark_unready(&self, connection_id: &str) -> Option<ConnectionRecord> {
        let mut inner = self.inner.lock().ok()?;
        let record = inner.by_connection.get_mut(connection_id)?;
        if record.state != ConnectionState::Ready {
            return None;
        }
        record.state = ConnectionState::Connecting;
        record.ready_at_ms = None;
        Some(record.clone())
    }

    pub fn begin_close_by_session(&self, pty_session_id: &str) -> Option<ConnectionRecord> {
        let mut inner = self.inner.lock().ok()?;
        let connection_id = inner.by_session.get(pty_session_id)?.clone();
        let record = inner.by_connection.get_mut(&connection_id)?;
        match record.state {
            ConnectionState::Connecting | ConnectionState::Ready => {
                record.state = ConnectionState::Closing;
                Some(record.clone())
            }
            _ => None,
        }
    }

    /// Final transition, invoked by the PTY wait thread. Safe against late
    /// and duplicate exit events: an exited record keeps its first exit code
    /// and can never move again.
    pub fn mark_exited_by_session(
        &self,
        pty_session_id: &str,
        exit_code: u32,
    ) -> Option<ConnectionRecord> {
        let mut inner = self.inner.lock().ok()?;
        let connection_id = inner.by_session.get(pty_session_id)?.clone();
        let record = inner.by_connection.get_mut(&connection_id)?;
        if record.state == ConnectionState::Exited {
            return None;
        }
        record.state = ConnectionState::Exited;
        record.closed_at_ms = Some(now_ms());
        record.exit_code = Some(exit_code);
        Some(record.clone())
    }

    pub fn get(&self, connection_id: &str) -> Option<ConnectionRecord> {
        let inner = self.inner.lock().ok()?;
        inner.by_connection.get(connection_id).cloned()
    }

    #[allow(dead_code)] // retained as phase B/C integration surface
    pub fn get_by_session(&self, pty_session_id: &str) -> Option<ConnectionRecord> {
        let inner = self.inner.lock().ok()?;
        let connection_id = inner.by_session.get(pty_session_id)?;
        inner.by_connection.get(connection_id).cloned()
    }

    pub fn list(&self) -> Vec<ConnectionRecord> {
        let mut records: Vec<ConnectionRecord> = self
            .inner
            .lock()
            .map(|inner| inner.by_connection.values().cloned().collect())
            .unwrap_or_default();
        records.sort_by_key(|record| record.generation);
        records
    }

    /// Forget old exited records. Live records are never purged.
    pub fn purge_exited_before(&self, cutoff_ms: u64) {
        if let Ok(mut inner) = self.inner.lock() {
            let stale: Vec<String> = inner
                .by_connection
                .values()
                .filter(|record| {
                    record.state == ConnectionState::Exited
                        && record.closed_at_ms.unwrap_or(0) < cutoff_ms
                })
                .map(|record| record.connection_id.clone())
                .collect();
            for connection_id in stale {
                if let Some(record) = inner.by_connection.remove(&connection_id) {
                    inner.by_session.remove(&record.pty_session_id);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> HostSnapshot {
        HostSnapshot {
            host_id: "h1".to_string(),
            alias: "prod".to_string(),
            hostname: "example.com".to_string(),
            user: "root".to_string(),
            port: 22,
            proxy_jump: None,
            has_identity_file: false,
            encoding: Some("utf-8".to_string()),
        }
    }

    fn register(registry: &ConnectionRegistry, session: &str) -> ConnectionRecord {
        registry
            .register(
                uuid::Uuid::new_v4().to_string(),
                session.to_string(),
                snapshot(),
                Some(PathBuf::from("/tmp/mux.sock")),
            )
            .unwrap()
    }

    #[test]
    fn generations_are_monotonic_and_unique() {
        let registry = ConnectionRegistry::default();
        let a = register(&registry, "s1");
        let b = register(&registry, "s2");
        assert!(b.generation > a.generation);
        assert_ne!(a.connection_id, b.connection_id);
    }

    #[test]
    fn duplicate_session_registration_is_rejected() {
        let registry = ConnectionRegistry::default();
        register(&registry, "s1");
        let err = registry
            .register(
                uuid::Uuid::new_v4().to_string(),
                "s1".to_string(),
                snapshot(),
                None,
            )
            .unwrap_err();
        assert!(err.contains("already registered"));
    }

    #[test]
    fn ready_only_from_connecting_and_never_after_exit() {
        let registry = ConnectionRegistry::default();
        let record = register(&registry, "s1");
        assert_eq!(record.state, ConnectionState::Connecting);

        let ready = registry.mark_ready(&record.connection_id).unwrap();
        assert_eq!(ready.state, ConnectionState::Ready);
        assert!(ready.ready_at_ms.is_some());
        assert!(registry.mark_ready(&record.connection_id).is_none());

        registry.begin_close_by_session("s1").unwrap();
        let exited = registry.mark_exited_by_session("s1", 0).unwrap();
        assert_eq!(exited.state, ConnectionState::Exited);
        assert_eq!(exited.exit_code, Some(0));
        // Late ready/exit events cannot resurrect or rewrite the record.
        assert!(registry.mark_ready(&record.connection_id).is_none());
        assert!(registry.mark_exited_by_session("s1", 137).is_none());
        let after = registry.get(&record.connection_id).unwrap();
        assert_eq!(after.exit_code, Some(0));
    }

    #[test]
    fn reconnect_creates_new_generation_and_old_grants_cannot_follow() {
        let registry = ConnectionRegistry::default();
        let first = register(&registry, "s1");
        registry.mark_ready(&first.connection_id).unwrap();
        registry.mark_exited_by_session("s1", 0).unwrap();

        // Same host, same alias: identity must still be brand new.
        let second = register(&registry, "s2");
        assert_ne!(first.connection_id, second.connection_id);
        assert!(second.generation > first.generation);
    }

    #[test]
    fn mcp_eligibility_requires_managed_socket_and_live_state() {
        let registry = ConnectionRegistry::default();
        let unmanaged = registry
            .register(
                uuid::Uuid::new_v4().to_string(),
                "s1".to_string(),
                snapshot(),
                None,
            )
            .unwrap();
        assert!(!unmanaged.mcp_eligible());
        assert!(unmanaged.requires_reconnect_for_mcp());

        let managed = register(&registry, "s2");
        assert!(managed.mcp_eligible());
        registry.begin_close_by_session("s2");
        let closing = registry.get(&managed.connection_id).unwrap();
        assert!(!closing.mcp_eligible());
    }

    #[test]
    fn ssh_target_prefers_alias() {
        let mut snap = snapshot();
        assert_eq!(snap.ssh_target(), "prod");
        snap.alias.clear();
        assert_eq!(snap.ssh_target(), "example.com");
    }

    #[test]
    fn purge_only_removes_old_exited_records() {
        let registry = ConnectionRegistry::default();
        let exited = register(&registry, "s1");
        registry.mark_exited_by_session("s1", 0).unwrap();
        let live = register(&registry, "s2");

        registry.purge_exited_before(u64::MAX);
        assert!(registry.get(&exited.connection_id).is_none());
        assert!(registry.get_by_session("s1").is_none());
        assert!(registry.get(&live.connection_id).is_some());
    }
}
