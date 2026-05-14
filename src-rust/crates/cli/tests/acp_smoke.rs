//! End-to-end smoke test: spawn the `claurst` binary in ACP mode, send a
//! short JSON-RPC conversation over its stdin, and verify the responses on
//! stdout match what the Agent Client Protocol spec mandates.
//!
//! This guards the wire-format and capability surface that registry-listed
//! ACP clients (Zed, Neovim, JetBrains, …) rely on. Runs against the
//! debug binary produced by `cargo build` — Cargo provides the path via
//! `CARGO_BIN_EXE_claurst`.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

fn binary_path() -> String {
    env!("CARGO_BIN_EXE_claurst").to_string()
}

fn run_with_input(stdin: &str, timeout: Duration) -> (String, String) {
    let mut child = Command::new(binary_path())
        .arg("acp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn claurst acp");

    {
        let mut stdin_handle = child.stdin.take().expect("stdin");
        stdin_handle.write_all(stdin.as_bytes()).expect("write stdin");
        // Dropping stdin signals EOF — the agent will finish in-flight work
        // and then exit cleanly.
    }

    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait().expect("try_wait") {
            Some(_status) => break,
            None if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                panic!("claurst acp did not exit within {timeout:?}");
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    }

    let output = child.wait_with_output().expect("wait_with_output");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn find_response(stdout: &str, id: i64) -> serde_json::Value {
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
            if v.get("id") == Some(&serde_json::json!(id)) {
                return v;
            }
        }
    }
    panic!("no response for id={id} in stdout:\n{stdout}");
}

#[test]
fn initialize_returns_spec_compliant_response() {
    let request = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{"fs":{"readTextFile":true,"writeTextFile":true},"terminal":false},"clientInfo":{"name":"smoke","version":"0.0.0"}}}
"#;
    let (stdout, _stderr) = run_with_input(request, Duration::from_secs(20));
    let resp = find_response(&stdout, 1);
    assert_eq!(resp["jsonrpc"], "2.0");
    let result = &resp["result"];
    assert_eq!(result["protocolVersion"], 1);
    // Agent identifies itself.
    assert_eq!(result["agentInfo"]["name"], "claurst");
    assert!(result["agentInfo"]["version"].is_string());
    // authMethods MUST be an array (even if empty).
    assert!(result["authMethods"].is_array());
    // Standard capability blocks present.
    let caps = &result["agentCapabilities"];
    assert!(caps["promptCapabilities"].is_object());
    assert!(caps["mcpCapabilities"].is_object());
    assert!(caps["loadSession"].is_boolean());
}

#[test]
fn session_new_returns_session_id() {
    // Use a path that's absolute on both Unix and Windows.
    let cwd = std::env::current_dir().expect("cwd");
    let cwd_str = cwd.to_string_lossy().replace('\\', "/");
    let request = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":1,"clientCapabilities":{{}},"clientInfo":{{"name":"smoke","version":"0.0.0"}}}}}}
{{"jsonrpc":"2.0","id":2,"method":"session/new","params":{{"cwd":"{cwd_str}","mcpServers":[]}}}}
"#
    );
    let (stdout, _stderr) = run_with_input(&request, Duration::from_secs(20));
    let resp = find_response(&stdout, 2);
    let session_id = resp["result"]["sessionId"]
        .as_str()
        .expect("sessionId should be a string");
    assert!(session_id.starts_with("acp-"), "sessionId not prefixed: {session_id}");
}

#[test]
fn session_load_returns_method_not_found() {
    // We do not advertise loadSession, so well-behaved clients should never
    // call session/load — but the agent must answer correctly if they do.
    let request = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{},"clientInfo":{"name":"smoke","version":"0.0.0"}}}
{"jsonrpc":"2.0","id":2,"method":"session/load","params":{"sessionId":"x","cwd":"/","mcpServers":[]}}
"#;
    let (stdout, _stderr) = run_with_input(request, Duration::from_secs(20));
    let resp = find_response(&stdout, 2);
    assert_eq!(resp["error"]["code"], -32601, "should be MethodNotFound");
}

#[test]
fn cancel_notification_is_silent() {
    // session/cancel is a notification — the agent MUST NOT respond, even
    // for an unknown session.
    let request = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{},"clientInfo":{"name":"smoke","version":"0.0.0"}}}
{"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":"nonexistent"}}
"#;
    let (stdout, _stderr) = run_with_input(request, Duration::from_secs(20));
    // Only the initialize response (id=1) should appear; no extra responses.
    let response_count = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter(|l| serde_json::from_str::<serde_json::Value>(l).is_ok())
        .count();
    assert_eq!(response_count, 1, "unexpected extra responses in:\n{stdout}");
}
