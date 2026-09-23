//! xTermius MCP bridge: a minimal stdio MCP server that forwards tool calls
//! to the running xTermius app over its local unix socket.
//!
//! The bridge intentionally contains no authorization logic: pairing tokens
//! come from the user via `XTERMIUS_MCP_CLIENT_ID` / `XTERMIUS_MCP_TOKEN`
//! environment variables, and every request is re-authorized by the app on
//! each call. The bridge only speaks MCP on stdio and the app's line-delimited
//! JSON IPC on the socket.
//!
//! MCP protocol notes: JSON-RPC 2.0 over newline-delimited JSON on stdio.
//! Implements initialize / tools\//list / tools\//call / ping; everything else
//! gets a standard method-not-found error.

use serde_json::{json, Value};
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

const PROTOCOL_VERSION: &str = "2024-11-05";

fn main() {
    if let Err(error) = run() {
        eprintln!("[xtermius-mcp-bridge] {error}");
        std::process::exit(1);
    }
}

fn socket_path() -> PathBuf {
    if let Ok(custom) = std::env::var("XTERMIUS_MCP_SOCKET") {
        return PathBuf::from(custom);
    }
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("xtermius")
        .join("mcp")
        .join("bridge.sock")
}

struct Credentials {
    client_id: String,
    token: String,
}

fn credentials() -> Result<Credentials, String> {
    let client_id = std::env::var("XTERMIUS_MCP_CLIENT_ID")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .ok_or("missing client id (XTERMIUS_MCP_CLIENT_ID)")?;
    let token = std::env::var("XTERMIUS_MCP_TOKEN")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .ok_or("missing pairing token (XTERMIUS_MCP_TOKEN)")?;
    Ok(Credentials { client_id, token })
}

fn run() -> Result<(), String> {
    let credentials = credentials()?;
    let socket = socket_path();
    let mut ipc = IpcClient::new(socket);
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(_) => break,
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_str(trimmed) {
            Ok(message) => message,
            Err(_) => continue, // not JSON-RPC; ignore per spec robustness
        };
        // Notifications (no id) need no response.
        let Some(id) = message.get("id").cloned() else {
            continue;
        };
        let method = message.get("method").and_then(Value::as_str).unwrap_or("");
        let response = match method {
            "initialize" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "xtermius", "version": env!("CARGO_PKG_VERSION") }
                }
            }),
            "ping" => json!({ "jsonrpc": "2.0", "id": id, "result": {} }),
            "tools/list" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "tools": tool_descriptors() }
            }),
            "tools/call" => {
                let params = message.get("params").cloned().unwrap_or(json!({}));
                handle_tool_call(id, params, &credentials, &mut ipc)
            }
            _ => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": "method not found" }
            }),
        };
        let mut out = serde_json::to_string(&response).map_err(|e| e.to_string())?;
        out.push('\n');
        stdout
            .write_all(out.as_bytes())
            .and_then(|_| stdout.flush())
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn tool_descriptors() -> Value {
    json!([
        {
            "name": "list_connections",
            "description": "List SSH connections the xTermius user has granted this client access to. Only connections that are currently usable are returned.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "read_connection_output",
            "description": "Read terminal output of a granted connection starting at a cursor. Output after the grant point only; gaps are reported explicitly.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "connection_id": { "type": "string" },
                    "cursor": { "type": "integer", "minimum": 0 },
                    "max_chars": { "type": "integer", "minimum": 1 }
                },
                "required": ["connection_id"],
                "additionalProperties": false
            }
        },
        {
            "name": "run_command",
            "description": "Submit a command for execution on a granted connection. Runs on an independent channel over the user's existing authenticated SSH session, only after the xTermius user approves the immutable command, working directory, and timeout. Use the returned task_id with read_task.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "connection_id": { "type": "string" },
                    "request_id": { "type": "string", "minLength": 1, "maxLength": 128, "description": "Idempotency key, at most 128 UTF-8 bytes; retries with the same id never execute twice" },
                    "command": { "type": "string", "minLength": 1, "maxLength": 16384 },
                    "working_directory": {
                        "type": "string",
                        "enum": ["login"],
                        "default": "login",
                        "description": "Remote working directory. V1 supports the SSH login directory only; omitted values default to login."
                    },
                    "timeout_seconds": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 1800,
                        "default": 300,
                        "description": "Maximum runtime before the local SSH channel is stopped. Omitted values default to 300 seconds."
                    }
                },
                "required": ["connection_id", "request_id", "command"],
                "additionalProperties": false
            }
        },
        {
            "name": "read_task",
            "description": "Read the status and independent stdout/stderr streams of a task owned by this client. The streams have separate byte cursors and no shared ordering.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "task_id": { "type": "string" },
                    "stdout_offset": { "type": "integer", "minimum": 0 },
                    "stderr_offset": { "type": "integer", "minimum": 0 },
                    "output_offset": {
                        "type": "integer",
                        "minimum": 0,
                        "deprecated": true,
                        "description": "Legacy stdout-only cursor; stderr starts at offset zero when this is used."
                    }
                },
                "required": ["task_id"],
                "additionalProperties": false
            }
        },
        {
            "name": "cancel_task",
            "description": "Request cancellation of a task owned by this client. Cancellation is confirmed asynchronously; keep polling read_task.",
            "inputSchema": {
                "type": "object",
                "properties": { "task_id": { "type": "string" } },
                "required": ["task_id"],
                "additionalProperties": false
            }
        }
    ])
}

fn handle_tool_call(
    id: Value,
    params: Value,
    credentials: &Credentials,
    ipc: &mut IpcClient,
) -> Value {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

    let request = match name {
        "list_connections" => json!({
            "method": "list_connections",
            "params": { "client_id": credentials.client_id, "token": credentials.token }
        }),
        "read_connection_output" => json!({
            "method": "read_connection_output",
            "params": {
                "client_id": credentials.client_id,
                "token": credentials.token,
                "connection_id": arguments.get("connection_id").and_then(Value::as_str).unwrap_or(""),
                "cursor": arguments.get("cursor").and_then(Value::as_u64).unwrap_or(0),
                "max_chars": arguments.get("max_chars").and_then(Value::as_u64)
            }
        }),
        "run_command" => {
            let working_directory = match arguments.get("working_directory") {
                None => "login",
                Some(value) => value.as_str().unwrap_or(""),
            };
            let timeout_seconds = match arguments.get("timeout_seconds") {
                None => 300,
                Some(value) => value.as_u64().unwrap_or(0),
            };
            json!({
                "method": "run_command",
                "params": {
                    "client_id": credentials.client_id,
                    "token": credentials.token,
                    "connection_id": arguments.get("connection_id").and_then(Value::as_str).unwrap_or(""),
                    "request_id": arguments.get("request_id").and_then(Value::as_str).unwrap_or(""),
                    "command": arguments.get("command").and_then(Value::as_str).unwrap_or(""),
                    "working_directory": working_directory,
                    "timeout_seconds": timeout_seconds
                }
            })
        }
        "read_task" => json!({
            "method": "read_task",
            "params": {
                "client_id": credentials.client_id,
                "token": credentials.token,
                "task_id": arguments.get("task_id").and_then(Value::as_str).unwrap_or(""),
                "stdout_offset": arguments.get("stdout_offset").and_then(Value::as_u64),
                "stderr_offset": arguments.get("stderr_offset").and_then(Value::as_u64),
                "output_offset": arguments.get("output_offset").and_then(Value::as_u64)
            }
        }),
        "cancel_task" => json!({
            "method": "cancel_task",
            "params": {
                "client_id": credentials.client_id,
                "token": credentials.token,
                "task_id": arguments.get("task_id").and_then(Value::as_str).unwrap_or("")
            }
        }),
        _ => {
            return json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32602, "message": format!("unknown tool: {name}") }
            })
        }
    };

    match ipc.request(&request) {
        Ok(response) => tool_result_from_ipc(id, response),
        Err(message) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "content": [{ "type": "text", "text": message }],
                "isError": true
            }
        }),
    }
}

/// One stdio bridge lifetime owns one lazy IPC session. Keeping the socket
/// open is important for the server-side transport lifecycle: EOF on stdin
/// (or bridge drop) must correspond to one IPC transport ending, rather than
/// five independent transports for five tool calls.
struct IpcClient {
    socket: PathBuf,
    reader: Option<BufReader<UnixStream>>,
}

impl IpcClient {
    fn new(socket: PathBuf) -> Self {
        Self {
            socket,
            reader: None,
        }
    }

    fn connect(&mut self) -> Result<(), IpcError> {
        let stream = UnixStream::connect(&self.socket).map_err(IpcError::Connect)?;
        self.reader = Some(BufReader::new(stream));
        Ok(())
    }

    fn disconnect(&mut self) {
        // Dropping the BufReader also drops its UnixStream and closes the
        // transport. No request bytes are retained across a reconnect.
        self.reader = None;
    }

    fn request(&mut self, request: &Value) -> Result<Value, String> {
        let mut line = serde_json::to_string(request).map_err(|error| error.to_string())?;
        line.push('\n');

        if self.reader.is_none() {
            self.connect().map_err(IpcError::message)?;
        }

        match self.roundtrip_connected(line.as_bytes()) {
            Ok(response) => Ok(response),
            Err(IpcError::WriteBeforeSend(_)) => {
                // No request byte reached the app. Reconnecting and sending
                // once is safe even for run_command because the server could
                // not have observed this request.
                self.disconnect();
                self.connect().map_err(IpcError::message)?;
                self.roundtrip_connected(line.as_bytes())
                    .map_err(IpcError::message)
            }
            Err(error) => Err(error.message()),
        }
    }

    fn roundtrip_connected(&mut self, line: &[u8]) -> Result<Value, IpcError> {
        let reader = self.reader.as_mut().ok_or_else(|| {
            IpcError::Connect(io::Error::new(io::ErrorKind::NotConnected, "not connected"))
        })?;
        let stream = reader.get_mut();
        let mut written = 0;
        while written < line.len() {
            match stream.write(&line[written..]) {
                Ok(0) => {
                    let error = io::Error::new(io::ErrorKind::WriteZero, "IPC socket closed");
                    self.disconnect();
                    return Err(if written == 0 {
                        IpcError::WriteBeforeSend(error)
                    } else {
                        IpcError::WriteAfterSend(error)
                    });
                }
                Ok(count) => written += count,
                Err(error) => {
                    self.disconnect();
                    return Err(if written == 0 {
                        IpcError::WriteBeforeSend(error)
                    } else {
                        IpcError::WriteAfterSend(error)
                    });
                }
            }
        }
        if let Err(error) = stream.flush() {
            self.disconnect();
            return Err(IpcError::WriteAfterSend(error));
        }

        let mut response = String::new();
        match reader.read_line(&mut response) {
            Ok(0) => {
                self.disconnect();
                Err(IpcError::Read(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "IPC peer closed before a response",
                )))
            }
            Ok(_) => serde_json::from_str(response.trim()).map_err(IpcError::Decode),
            Err(error) => {
                self.disconnect();
                Err(IpcError::Read(error))
            }
        }
    }
}

#[derive(Debug)]
enum IpcError {
    Connect(io::Error),
    WriteBeforeSend(io::Error),
    WriteAfterSend(io::Error),
    Read(io::Error),
    Decode(serde_json::Error),
}

impl IpcError {
    fn message(self) -> String {
        match self {
            Self::Connect(error) => format!("cannot reach xTermius (is it running?): {error}"),
            Self::WriteBeforeSend(error) => format!("cannot send request to xTermius: {error}"),
            Self::WriteAfterSend(error) | Self::Read(error) => format!(
                "transport_unknown: xTermius may have received the request; no automatic retry: {error}"
            ),
            Self::Decode(error) => format!("invalid response from xTermius: {error}"),
        }
    }
}

/// IPC errors become MCP tool errors (isError), not JSON-RPC errors, so the
/// agent sees the reason (unauthorized / not_ready / ...) in-band.
fn tool_result_from_ipc(id: Value, response: Value) -> Value {
    let is_error = response.get("kind").and_then(Value::as_str) == Some("error");
    let text = serde_json::to_string_pretty(&response).unwrap_or_default();
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{ "type": "text", "text": text }],
            "isError": is_error
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    fn test_socket_path(label: &str) -> PathBuf {
        let directory = std::env::current_dir()
            .unwrap()
            .join(format!(".xtb-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        directory.join("bridge.sock")
    }

    fn cleanup_socket(path: &PathBuf) {
        let _ = std::fs::remove_file(path);
        if let Some(parent) = path.parent() {
            let _ = std::fs::remove_dir(parent);
        }
    }

    fn request(label: &str) -> Value {
        json!({ "method": "list_connections", "params": { "request": label } })
    }

    fn send_response(stream: &mut UnixStream) {
        stream
            .write_all(
                br#"{"kind":"ack"}
"#,
            )
            .unwrap();
        stream.flush().unwrap();
    }

    #[test]
    fn persistent_client_reuses_one_accept_for_multiple_requests() {
        let path = test_socket_path("persistent");
        let listener = UnixListener::bind(&path).unwrap();
        let accepts = Arc::new(AtomicUsize::new(0));
        let server_accepts = Arc::clone(&accepts);
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            server_accepts.fetch_add(1, Ordering::SeqCst);
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            for _ in 0..2 {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                assert!(!line.trim().is_empty());
                send_response(&mut writer);
            }
        });

        let mut client = IpcClient::new(path.clone());
        assert_eq!(client.request(&request("first")).unwrap()["kind"], "ack");
        assert_eq!(client.request(&request("second")).unwrap()["kind"], "ack");
        drop(client);
        server.join().unwrap();
        assert_eq!(accepts.load(Ordering::SeqCst), 1);
        cleanup_socket(&path);
    }

    #[test]
    fn read_failure_is_unknown_and_next_call_connects_without_replaying() {
        let path = test_socket_path("reconnect");
        let listener = UnixListener::bind(&path).unwrap();
        let accepts = Arc::new(AtomicUsize::new(0));
        let received = Arc::new(Mutex::new(Vec::<String>::new()));
        let server_accepts = Arc::clone(&accepts);
        let server_received = Arc::clone(&received);
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            server_accepts.fetch_add(1, Ordering::SeqCst);
            let mut first_reader = BufReader::new(stream);
            let mut first_line = String::new();
            first_reader.read_line(&mut first_line).unwrap();
            server_received.lock().unwrap().push(first_line);
            drop(first_reader); // no response: the request outcome is unknown

            let (stream, _) = listener.accept().unwrap();
            server_accepts.fetch_add(1, Ordering::SeqCst);
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            server_received.lock().unwrap().push(line);
            let mut writer = stream;
            send_response(&mut writer);
        });

        let first = request("first");
        let second = request("second");
        let first_wire = format!("{}\n", serde_json::to_string(&first).unwrap());
        let second_wire = format!("{}\n", serde_json::to_string(&second).unwrap());
        let mut client = IpcClient::new(path.clone());
        let error = client.request(&first).unwrap_err();
        assert!(error.contains("transport_unknown"));
        assert_eq!(client.request(&second).unwrap()["kind"], "ack");
        server.join().unwrap();

        assert_eq!(accepts.load(Ordering::SeqCst), 2);
        assert_eq!(
            *received.lock().unwrap(),
            vec![first_wire, second_wire],
            "a read failure must not replay the uncertain request"
        );
        cleanup_socket(&path);
    }

    #[test]
    fn run_command_schema_exposes_directory_and_timeout_contract() {
        let tools = tool_descriptors();
        let run_command = tools
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "run_command")
            .unwrap();
        let properties = &run_command["inputSchema"]["properties"];

        assert_eq!(properties["working_directory"]["enum"][0], "login");
        assert_eq!(properties["timeout_seconds"]["minimum"], 1);
        assert_eq!(properties["timeout_seconds"]["maximum"], 1800);
        assert_eq!(properties["timeout_seconds"]["default"], 300);
    }

    #[test]
    fn read_task_schema_exposes_independent_stream_offsets() {
        let tools = tool_descriptors();
        let read_task = tools
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "read_task")
            .unwrap();
        let properties = &read_task["inputSchema"]["properties"];

        assert_eq!(properties["stdout_offset"]["type"], "integer");
        assert_eq!(properties["stderr_offset"]["type"], "integer");
        assert_eq!(properties["output_offset"]["deprecated"], true);
    }
}
