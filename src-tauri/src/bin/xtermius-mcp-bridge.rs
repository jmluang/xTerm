//! xTermius MCP bridge: a minimal stdio MCP server that forwards tool calls
//! to the running xTermius app over its local unix socket.
//!
//! The bridge intentionally contains no authorization logic: pairing tokens
//! come from the user (env `XTERMIUS_MCP_CLIENT_ID` / `XTERMIUS_MCP_TOKEN`
//! or the first two CLI arguments), and every request is re-authorized by
//! the app on each call. The bridge only speaks MCP on stdio and the app's
//! line-delimited JSON IPC on the socket.
//!
//! MCP protocol notes: JSON-RPC 2.0 over newline-delimited JSON on stdio.
//! Implements initialize / tools\//list / tools\//call / ping; everything else
//! gets a standard method-not-found error.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
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
    let mut args = std::env::args().skip(1);
    let client_id = std::env::var("XTERMIUS_MCP_CLIENT_ID")
        .ok()
        .or_else(|| args.next())
        .filter(|v| !v.trim().is_empty())
        .ok_or("missing client id (XTERMIUS_MCP_CLIENT_ID)")?;
    let token = std::env::var("XTERMIUS_MCP_TOKEN")
        .ok()
        .or_else(|| args.next())
        .filter(|v| !v.trim().is_empty())
        .ok_or("missing pairing token (XTERMIUS_MCP_TOKEN)")?;
    Ok(Credentials { client_id, token })
}

fn run() -> Result<(), String> {
    let credentials = credentials()?;
    let socket = socket_path();
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
                handle_tool_call(id, params, &credentials, &socket)
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
            "description": "Submit a command for execution on a granted connection. Runs on an independent channel over the user's existing authenticated SSH session, only after the xTermius user approves this exact command. Use the returned task_id with read_task.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "connection_id": { "type": "string" },
                    "request_id": { "type": "string", "description": "Idempotency key; retries with the same id never execute twice" },
                    "command": { "type": "string" }
                },
                "required": ["connection_id", "request_id", "command"],
                "additionalProperties": false
            }
        },
        {
            "name": "read_task",
            "description": "Read the status and incremental output of a task owned by this client.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "task_id": { "type": "string" },
                    "output_offset": { "type": "integer", "minimum": 0 }
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

fn handle_tool_call(id: Value, params: Value, credentials: &Credentials, socket: &PathBuf) -> Value {
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
        "run_command" => json!({
            "method": "run_command",
            "params": {
                "client_id": credentials.client_id,
                "token": credentials.token,
                "connection_id": arguments.get("connection_id").and_then(Value::as_str).unwrap_or(""),
                "request_id": arguments.get("request_id").and_then(Value::as_str).unwrap_or(""),
                "command": arguments.get("command").and_then(Value::as_str).unwrap_or("")
            }
        }),
        "read_task" => json!({
            "method": "read_task",
            "params": {
                "client_id": credentials.client_id,
                "token": credentials.token,
                "task_id": arguments.get("task_id").and_then(Value::as_str).unwrap_or(""),
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

    match ipc_roundtrip(socket, &request) {
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

fn ipc_roundtrip(socket: &PathBuf, request: &Value) -> Result<Value, String> {
    let mut stream = UnixStream::connect(socket)
        .map_err(|e| format!("cannot reach xTermius (is it running?): {e}"))?;
    let mut line = serde_json::to_string(request).map_err(|e| e.to_string())?;
    line.push('\n');
    stream.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader.read_line(&mut response).map_err(|e| e.to_string())?;
    serde_json::from_str(response.trim()).map_err(|e| e.to_string())
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
