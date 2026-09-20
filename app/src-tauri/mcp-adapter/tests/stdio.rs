//! Black-box MCP client coverage for the production STDIO adapter binary.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::os::unix::process::CommandExt;
use std::os::unix::{io::AsRawFd, net::UnixStream};
use std::process::{Command, Stdio};

fn read_response(reader: &mut BufReader<std::process::ChildStdout>, id: u64) -> Value {
    loop {
        let mut line = String::new();
        assert!(
            reader.read_line(&mut line).unwrap() > 0,
            "adapter closed stdout"
        );
        let value: Value = serde_json::from_str(&line).expect("stdout is MCP JSON only");
        if value.get("id").and_then(Value::as_u64) == Some(id) {
            return value;
        }
    }
}

fn spawn_adapter(args: &[&str]) -> std::process::Child {
    let (mut sender, receiver) = UnixStream::pair().unwrap();
    let receiver_fd = receiver.as_raw_fd();
    let mut command = Command::new(env!("CARGO_BIN_EXE_deck-mcp"));
    command
        .args(args)
        .args(["--credential-fd", "3"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // SAFETY: the closure only duplicates the already-open synthetic credential
    // socket onto a fixed child descriptor before exec.
    unsafe {
        command.pre_exec(move || {
            if libc::dup2(receiver_fd, 3) < 0 || libc::fcntl(3, libc::F_SETFD, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command.spawn().unwrap();
    sender.write_all(b"mcp_synthetic_test").unwrap();
    sender.shutdown(std::net::Shutdown::Write).unwrap();
    child
}

#[test]
fn negotiates_away_from_an_unsupported_protocol_version() {
    let mut child = spawn_adapter(&[
        "--client-id",
        "client_test",
        "--socket",
        "/tmp/deck-mcp-intentionally-absent.sock",
    ]);
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    writeln!(input, "{}", json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"1900-01-01","capabilities":{},"clientInfo":{"name":"deck-test-client","version":"1"}}})).unwrap();
    input.flush().unwrap();
    let initialized = read_response(&mut output, 1);
    assert_ne!(initialized["result"]["protocolVersion"], "1900-01-01");
    assert_eq!(initialized["result"]["serverInfo"]["name"], "deck-mcp");
    drop(input);
    assert!(child.wait().unwrap().success());
}

#[test]
fn initializes_lists_and_calls_over_stdio_without_stdout_noise() {
    let root = std::env::temp_dir().join(format!("deck-mcp-stdio-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = root.join("control.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let mock = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut request)
            .unwrap();
        let request: Value = serde_json::from_str(&request).unwrap();
        assert_eq!(request["clientId"], "client_test");
        assert_eq!(request["credential"], "mcp_synthetic_test");
        assert_eq!(request["version"], 3);
        assert_eq!(request["tool"], "deck_capabilities");
        writeln!(
            stream,
            "{}",
            json!({"ok":true,"protocolVersion":1,"executionMode":"trusted-host"})
        )
        .unwrap();
    });

    let socket_text = socket.to_str().unwrap();
    let mut child = spawn_adapter(&["--client-id", "client_test", "--socket", socket_text]);
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());

    writeln!(input, "{}", json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"deck-test-client","version":"1"}}})).unwrap();
    input.flush().unwrap();
    let initialized = read_response(&mut output, 1);
    assert_eq!(initialized["result"]["serverInfo"]["name"], "deck-mcp");

    writeln!(
        input,
        "{}",
        json!({"jsonrpc":"2.0","method":"notifications/initialized"})
    )
    .unwrap();
    writeln!(
        input,
        "{}",
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}})
    )
    .unwrap();
    input.flush().unwrap();
    let listed = read_response(&mut output, 2);
    let tools = listed["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 14);
    let exec = tools
        .iter()
        .find(|tool| tool["name"] == "deck_exec")
        .unwrap();
    assert_eq!(exec["annotations"]["readOnlyHint"], false);
    assert_eq!(exec["annotations"]["openWorldHint"], true);
    assert_eq!(exec["inputSchema"]["additionalProperties"], false);
    assert_eq!(exec["outputSchema"]["required"][0], "ok");
    for name in [
        "deck_project_list",
        "deck_project_read",
        "deck_project_search",
    ] {
        let tool = tools.iter().find(|tool| tool["name"] == name).unwrap();
        assert_eq!(tool["annotations"]["readOnlyHint"], true);
        assert_eq!(tool["annotations"]["openWorldHint"], false);
    }

    writeln!(input, "{}", json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"deck_capabilities","arguments":{"unknown":true}}})).unwrap();
    input.flush().unwrap();
    let rejected = read_response(&mut output, 3);
    assert_eq!(rejected["result"]["isError"], true);

    writeln!(input, "{}", json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"deck_capabilities","arguments":{}}})).unwrap();
    input.flush().unwrap();
    let called = read_response(&mut output, 4);
    assert_eq!(called["result"]["isError"], false);
    assert_eq!(
        called["result"]["structuredContent"]["executionMode"],
        "trusted-host"
    );

    drop(input);
    assert!(child.wait().unwrap().success());
    mock.join().unwrap();
    std::fs::remove_file(&socket).unwrap();
    std::fs::remove_dir(&root).unwrap();
}
