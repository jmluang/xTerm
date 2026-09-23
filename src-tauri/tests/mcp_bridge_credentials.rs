use std::process::{Command, Stdio};

#[test]
fn bridge_does_not_accept_credentials_from_command_line_arguments() {
    let output = Command::new(env!("CARGO_BIN_EXE_xtermius-mcp-bridge"))
        .args(["cli-client-id", "cli-pairing-token"])
        .env_remove("XTERMIUS_MCP_CLIENT_ID")
        .env_remove("XTERMIUS_MCP_TOKEN")
        .stdin(Stdio::null())
        .output()
        .expect("bridge should start");

    assert!(
        !output.status.success(),
        "credentials supplied only as CLI arguments must be rejected"
    );
}

#[test]
fn bridge_accepts_credentials_from_environment_variables() {
    let output = Command::new(env!("CARGO_BIN_EXE_xtermius-mcp-bridge"))
        .env("XTERMIUS_MCP_CLIENT_ID", "env-client-id")
        .env("XTERMIUS_MCP_TOKEN", "env-pairing-token")
        .stdin(Stdio::null())
        .output()
        .expect("bridge should start");

    assert!(
        output.status.success(),
        "environment credentials should allow the bridge to start"
    );
}
