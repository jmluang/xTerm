//! Line-delimited JSON protocol between the stdio MCP bridge and the xTerm
//! core over a local unix socket (phase B/C). One request, one response.

use serde::{Deserialize, Serialize};

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
    },
    ReadTask {
        client_id: String,
        token: String,
        task_id: String,
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
    pub output: String,
    pub next_offset: usize,
    pub truncated: bool,
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
    Task(IpcTaskRead),
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
            },
            IpcRequest::ReadTask {
                client_id: "c".into(),
                token: "t".into(),
                task_id: "task".into(),
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
}
