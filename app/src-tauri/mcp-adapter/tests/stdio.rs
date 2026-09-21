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

/// Asserts one Protocol 4 sequence field in a tool's advertised inputSchema.
fn assert_sequence_field(tool: &Value, field: &str) {
    let schema = &tool["inputSchema"];
    let name = &tool["name"];
    assert!(
        schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value == field),
        "{name}: {field} must be required: {schema}"
    );
    let property = &schema["properties"][field];
    assert_eq!(
        property["type"], "integer",
        "{name}: {field} must be a non-nullable integer: {property}"
    );
    assert_eq!(property["minimum"], 0, "{name}: {field}: {property}");
    assert!(
        property.get("default").is_none(),
        "{name}: {field} must not carry a default the server could fill in: {property}"
    );
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
    assert_eq!(
        initialized["result"]["serverInfo"]["version"],
        env!("CARGO_PKG_VERSION")
    );
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
        assert_eq!(request["version"], 4);
        assert_eq!(request["tool"], "deck_capabilities");
        writeln!(
            stream,
            "{}",
            json!({"ok":true,"protocolVersion":1,"executionMode":"trusted-host","adapterVersion":"stale","tools":["stale"],"mayCreateSession":false})
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
    let listed_names = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    for tool in tools {
        let read_only = tool["annotations"]["readOnlyHint"] == true;
        // A side-effecting tool is idempotent only inside Deck's replay
        // window, so none may claim idempotence.
        if !read_only {
            assert_eq!(tool["annotations"]["idempotentHint"], false, "{tool}");
            assert_eq!(tool["annotations"]["destructiveHint"], true, "{tool}");
        }
    }
    let inspect = tools
        .iter()
        .find(|tool| tool["name"] == "deck_session_inspect")
        .unwrap();
    assert!(!inspect["description"]
        .as_str()
        .unwrap()
        .contains("terminal context"));
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
    let control = tools
        .iter()
        .find(|tool| tool["name"] == "deck_session_control")
        .unwrap();
    let required = control["inputSchema"]["required"].as_array().unwrap();
    assert!(required.iter().any(|value| value == "holder_id"));
    let create = tools
        .iter()
        .find(|tool| tool["name"] == "deck_session_create")
        .unwrap();
    // Protocol 4 binds every create and control change to a server-issued
    // sequence. The advertised schema is what a client plans its calls from,
    // so the field must be there, required, and a plain non-negative integer.
    assert_sequence_field(create, "create_sequence");
    assert_sequence_field(control, "control_sequence");
    assert_eq!(
        control["inputSchema"]["properties"]["lease_ms"]["minimum"],
        1000
    );
    assert_eq!(
        control["inputSchema"]["properties"]["lease_ms"]["maximum"],
        300000
    );

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
    assert_eq!(
        called["result"]["structuredContent"]["adapterVersion"],
        env!("CARGO_PKG_VERSION")
    );
    assert_eq!(
        called["result"]["structuredContent"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|name| name.as_str().unwrap().to_owned())
            .collect::<Vec<_>>(),
        listed_names,
        "capabilities.tools and tools/list are one registry"
    );
    assert!(called["result"]["structuredContent"]
        .get("adapterBuild")
        .is_some());
    assert_eq!(
        called["result"]["structuredContent"]["mayCreateSession"],
        false
    );

    drop(input);
    assert!(child.wait().unwrap().success());
    mock.join().unwrap();
    std::fs::remove_file(&socket).unwrap();
    std::fs::remove_dir(&root).unwrap();
}

#[test]
fn a_lost_answer_to_a_side_effect_is_ambiguous_not_unavailable() {
    let root = std::env::temp_dir().join(format!("deck-mcp-ambig-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = root.join("control.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    // Deck reads the whole request, then the connection dies unanswered.
    let mock = std::thread::spawn(move || {
        for _ in 0..2 {
            let (stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request)
                .unwrap();
            drop(stream);
        }
    });
    let mut child = spawn_adapter(&[
        "--client-id",
        "client_test",
        "--socket",
        socket.to_str().unwrap(),
    ]);
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    writeln!(input, "{}", json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}})).unwrap();
    input.flush().unwrap();
    read_response(&mut output, 1);
    writeln!(
        input,
        "{}",
        json!({"jsonrpc":"2.0","method":"notifications/initialized"})
    )
    .unwrap();
    writeln!(input, "{}", json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"deck_exec","arguments":{"request_id":"r","session_id":"s","expected_generation":"g","control_epoch":1,"holder_id":"h","script":"true"}}})).unwrap();
    input.flush().unwrap();
    let exec = read_response(&mut output, 2);
    assert_eq!(
        exec["result"]["structuredContent"]["error"]["code"],
        "OPERATION_AMBIGUOUS"
    );
    assert!(exec["result"]["structuredContent"]["error"]["nextAction"]
        .as_str()
        .unwrap()
        .contains("SAME request_id"));
    writeln!(input, "{}", json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"deck_sessions_list","arguments":{}}})).unwrap();
    input.flush().unwrap();
    let list = read_response(&mut output, 3);
    assert_eq!(
        list["result"]["structuredContent"]["error"]["code"], "DECK_UNAVAILABLE",
        "a read-only call has no side effect to be ambiguous about"
    );
    drop(input);
    assert!(child.wait().unwrap().success());
    mock.join().unwrap();
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn control_validation_rejects_before_connecting_and_valid_samples_cross_validation() {
    let mut child = spawn_adapter(&[
        "--client-id",
        "client_test",
        "--socket",
        "/tmp/deck-mcp-validation-absent.sock",
    ]);
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    writeln!(input, "{}", json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}})).unwrap();
    input.flush().unwrap();
    read_response(&mut output, 1);
    writeln!(
        input,
        "{}",
        json!({"jsonrpc":"2.0","method":"notifications/initialized"})
    )
    .unwrap();

    let invalid = [
        json!({"request_id":"req_missing_holder","session_id":"s","expected_generation":"g","action":"request"}),
        json!({"request_id":"req_low_lease","session_id":"s","expected_generation":"g","action":"request","holder_id":"holder_a","lease_ms":999,"control_sequence":0}),
        json!({"request_id":"req_renew_epoch","session_id":"s","expected_generation":"g","action":"renew","holder_id":"holder_a","control_sequence":0}),
        json!({"request_id":"req_release_lease","session_id":"s","expected_generation":"g","action":"release","holder_id":"holder_a","control_epoch":1,"lease_ms":1000,"control_sequence":0}),
        json!({"request_id":"x".repeat(129),"session_id":"s","expected_generation":"g","action":"request","holder_id":"holder_a","control_sequence":0}),
        json!({"request_id":"req_no_sequence","session_id":"s","expected_generation":"g","action":"request","holder_id":"holder_a"}),
        json!({"request_id":"req_bad_sequence","session_id":"s","expected_generation":"g","action":"request","holder_id":"holder_a","control_sequence":-1}),
    ];
    for (offset, arguments) in invalid.into_iter().enumerate() {
        let id = 10 + offset as u64;
        writeln!(input, "{}", json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"deck_session_control","arguments":arguments}})).unwrap();
        input.flush().unwrap();
        let response = read_response(&mut output, id);
        assert_eq!(
            response["result"]["structuredContent"]["error"]["code"],
            "INVALID_ARGUMENTS"
        );
    }

    for (id, arguments) in [
        (
            20,
            json!({"request_id":"req_valid_default","session_id":"s","expected_generation":"g","action":"request","holder_id":"holder_a","control_sequence":0}),
        ),
        (
            21,
            json!({"request_id":"req_valid_lease","session_id":"s","expected_generation":"g","action":"request","holder_id":"holder_b","lease_ms":60000,"control_sequence":0}),
        ),
        (
            22,
            json!({"request_id":"req_valid_renew","session_id":"s","expected_generation":"g","action":"renew","holder_id":"holder_b","control_epoch":2,"lease_ms":null,"control_sequence":1}),
        ),
        (
            23,
            json!({"request_id":"req_valid_release","session_id":"s","expected_generation":"g","action":"release","holder_id":"holder_b","control_epoch":2,"lease_ms":null,"control_sequence":2}),
        ),
        (
            24,
            json!({"request_id":"x".repeat(128),"session_id":"s","expected_generation":"g","action":"request","holder_id":"holder_b","control_sequence":0}),
        ),
    ] {
        writeln!(input, "{}", json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"deck_session_control","arguments":arguments}})).unwrap();
        input.flush().unwrap();
        let response = read_response(&mut output, id);
        assert_eq!(
            response["result"]["structuredContent"]["error"]["code"],
            "DECK_UNAVAILABLE"
        );
    }
    drop(input);
    assert!(child.wait().unwrap().success());
}

#[test]
fn a_call_without_a_sequence_names_the_field_before_connecting() {
    // A client still holding a Protocol 3 tool list sends no sequence. The
    // adapter must name the missing field (and never reach Deck) so the
    // client can tell its schema is stale instead of guessing a value.
    let mut child = spawn_adapter(&[
        "--client-id",
        "client_test",
        "--socket",
        "/tmp/deck-mcp-sequence-absent.sock",
    ]);
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    writeln!(input, "{}", json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}})).unwrap();
    input.flush().unwrap();
    read_response(&mut output, 1);
    writeln!(
        input,
        "{}",
        json!({"jsonrpc":"2.0","method":"notifications/initialized"})
    )
    .unwrap();

    for (id, tool, arguments, field, category) in [
        (
            10,
            "deck_session_create",
            json!({"request_id":"req_create","project_id":"P1","cwd":"/tmp","title":null}),
            "create_sequence",
            "required",
        ),
        (
            11,
            "deck_session_create",
            json!({"request_id":"req_create","project_id":"P1","cwd":"/tmp","create_sequence":null}),
            "create_sequence",
            "type",
        ),
        (
            12,
            "deck_session_control",
            json!({"request_id":"req_control","session_id":"s","expected_generation":"g","action":"request","holder_id":"holder_a"}),
            "control_sequence",
            "required",
        ),
    ] {
        writeln!(input, "{}", json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":tool,"arguments":arguments}})).unwrap();
        input.flush().unwrap();
        let response = read_response(&mut output, id);
        let error = &response["result"]["structuredContent"]["error"];
        assert_eq!(error["code"], "INVALID_ARGUMENTS", "{response}");
        assert_eq!(error["details"]["fieldPath"], field, "{response}");
        assert_eq!(error["details"]["category"], category, "{response}");
        if category == "required" {
            assert!(
                error["nextAction"]
                    .as_str()
                    .unwrap()
                    .contains("refresh tool discovery"),
                "{response}"
            );
        }
    }

    // With the sequence present the call passes validation and only then
    // fails on the absent socket.
    writeln!(input, "{}", json!({"jsonrpc":"2.0","id":20,"method":"tools/call","params":{"name":"deck_session_create","arguments":{"request_id":"req_create","project_id":"P1","cwd":"/tmp","create_sequence":0}}})).unwrap();
    input.flush().unwrap();
    let response = read_response(&mut output, 20);
    assert_eq!(
        response["result"]["structuredContent"]["error"]["code"],
        "DECK_UNAVAILABLE"
    );
    drop(input);
    assert!(child.wait().unwrap().success());
}
