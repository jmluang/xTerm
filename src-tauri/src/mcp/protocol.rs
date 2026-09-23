//! Line-delimited JSON protocol between the stdio MCP bridge and the xTerm
//! core over a local unix socket (phase B/C). One request, one response.

use serde::{Deserialize, Serialize};

fn default_working_directory() -> String {
    "login".to_string()
}

fn default_timeout_seconds() -> u64 {
    300
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum IpcRequest {
    ListConnections {
        client_id: String,
        token: String,
    },
    ReadConnectionOutput {
        client_id: String,
        token: String,
        connection_id: String,
        cursor: u64,
        max_chars: Option<usize>,
    },
    RunCommand {
        client_id: String,
        token: String,
        connection_id: String,
        request_id: String,
        command: String,
        #[serde(default = "default_working_directory")]
        working_directory: String,
        #[serde(default = "default_timeout_seconds")]
        timeout_seconds: u64,
    },
    ReadTask {
        client_id: String,
        token: String,
        task_id: String,
        /// Independent UTF-8 byte cursor for stdout.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stdout_offset: Option<usize>,
        /// Independent UTF-8 byte cursor for stderr.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stderr_offset: Option<usize>,
        /// Deprecated compatibility cursor. When both independent cursors
        /// are absent, the service maps this to stdout and starts stderr at
        /// offset zero; no cross-stream order is implied.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_offset: Option<usize>,
    },
    CancelTask {
        client_id: String,
        token: String,
        task_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IpcConnection {
    pub connection_id: String,
    pub generation: u64,
    pub host_name: String,
    pub user: String,
    pub port: u16,
    pub alias: String,
    pub state: String,
    /// Output sequence the grant starts from; only data after this is
    /// visible to the client.
    pub grant_start_seq: u64,
    pub observe: bool,
    pub execute: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IpcOutputRead {
    pub data: String,
    pub next_cursor: u64,
    pub gap: bool,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IpcTaskRead {
    pub status: String,
    pub exit_code: Option<i32>,
    /// stdout and stderr are independent streams; there is intentionally no
    /// combined output field or global ordering declaration.
    pub stdout: String,
    pub stderr: String,
    pub next_stdout_offset: usize,
    pub next_stderr_offset: usize,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub stdout_gap: bool,
    pub stderr_gap: bool,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IpcRunAccepted {
    pub task_id: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IpcResponse {
    Connections { connections: Vec<IpcConnection> },
    Output(IpcOutputRead),
    RunAccepted(IpcRunAccepted),
    Task(Box<IpcTaskRead>),
    Ack { ok: bool },
    Error { code: String, message: String },
}

impl IpcResponse {
    pub fn error(code: &str, message: impl Into<String>) -> Self {
        Self::Error {
            code: code.to_string(),
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_all_requests() {
        let requests = vec![
            IpcRequest::ListConnections {
                client_id: "c".into(),
                token: "t".into(),
            },
            IpcRequest::ReadConnectionOutput {
                client_id: "c".into(),
                token: "t".into(),
                connection_id: "conn".into(),
                cursor: 7,
                max_chars: Some(1024),
            },
            IpcRequest::RunCommand {
                client_id: "c".into(),
                token: "t".into(),
                connection_id: "conn".into(),
                request_id: "r1".into(),
                command: "uptime".into(),
                working_directory: "login".into(),
                timeout_seconds: 300,
            },
            IpcRequest::ReadTask {
                client_id: "c".into(),
                token: "t".into(),
                task_id: "task".into(),
                stdout_offset: None,
                stderr_offset: None,
                output_offset: Some(12),
            },
            IpcRequest::CancelTask {
                client_id: "c".into(),
                token: "t".into(),
                task_id: "task".into(),
            },
        ];
        for request in requests {
            let line = serde_json::to_string(&request).unwrap();
            let back: IpcRequest = serde_json::from_str(&line).unwrap();
            assert_eq!(serde_json::to_string(&back).unwrap(), line);
        }
    }

    #[test]
    fn legacy_run_command_defaults_to_login_directory_and_five_minutes() {
        let request: IpcRequest = serde_json::from_value(serde_json::json!({
            "method": "run_command",
            "params": {
                "client_id": "client",
                "token": "secret",
                "connection_id": "connection",
                "request_id": "request",
                "command": "id"
            }
        }))
        .unwrap();

        assert!(matches!(
            request,
            IpcRequest::RunCommand {
                working_directory,
                timeout_seconds: 300,
                ..
            } if working_directory == "login"
        ));
    }

    #[test]
    fn read_task_round_trips_independent_stream_offsets() {
        let request = serde_json::json!({
            "method": "read_task",
            "params": {
                "client_id": "c",
                "token": "t",
                "task_id": "task",
                "stdout_offset": 7,
                "stderr_offset": 11
            }
        });
        let parsed: IpcRequest = serde_json::from_value(request).unwrap();
        let encoded = serde_json::to_value(parsed).unwrap();
        assert_eq!(encoded["params"]["stdout_offset"], 7);
        assert_eq!(encoded["params"]["stderr_offset"], 11);
    }

    #[test]
    fn legacy_read_task_output_offset_remains_wire_compatible() {
        let request = serde_json::json!({
            "method": "read_task",
            "params": {
                "client_id": "c",
                "token": "t",
                "task_id": "task",
                "output_offset": 13
            }
        });
        let parsed: IpcRequest = serde_json::from_value(request).unwrap();
        let encoded = serde_json::to_value(parsed).unwrap();
        assert_eq!(encoded["params"]["output_offset"], 13);
    }
}
