//! Black-box coverage for the production visible shell runner.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Runner(Child);

impl Drop for Runner {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn call(socket: &Path, request: Value) -> Value {
    let mut stream = UnixStream::connect(socket).unwrap();
    writeln!(stream, "{request}").unwrap();
    let mut response = String::new();
    BufReader::new(stream).read_line(&mut response).unwrap();
    serde_json::from_str(&response).unwrap()
}

fn wait_for(socket: &Path, job: &str) -> Value {
    let limit = Instant::now() + Duration::from_secs(3);
    loop {
        let value = call(
            socket,
            json!({"kind":"read","job_id":job,"cursor":0,"max_bytes":32768,"wait_ms":100}),
        );
        if value["job"]["state"] != "running" && value["job"]["state"] != "starting" {
            return value;
        }
        assert!(Instant::now() < limit, "job did not exit: {value}");
    }
}

#[test]
fn reports_exit_input_and_interrupt_without_terminal_markers() {
    let root = std::env::temp_dir().join(format!("deck-mcp-runner-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir(&root).unwrap();
    let socket = root.join("runner.sock");
    let child = Command::new(env!("CARGO_BIN_EXE_deck-mcp-runner"))
        .args([
            "--socket",
            socket.to_str().unwrap(),
            "--generation",
            "g_test",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut runner = Runner(child);
    let limit = Instant::now() + Duration::from_secs(3);
    while !socket.exists() {
        assert!(Instant::now() < limit, "runner socket was not created");
        std::thread::sleep(Duration::from_millis(10));
    }

    let started = call(
        &socket,
        json!({"kind":"exec","job_id":"job_ok","request_hash":"hash_ok","script":"printf 'fake exited 0\\n'; exit 17","cwd":"/tmp","wait_ms":0}),
    );
    assert!(started["ok"].as_bool().unwrap());
    let done = wait_for(&socket, "job_ok");
    assert_eq!(done["job"]["exitCode"], 17);
    assert_eq!(done["output"], "fake exited 0\n");

    call(
        &socket,
        json!({"kind":"exec","job_id":"job_unicode","request_hash":"hash_unicode","script":"printf 'ab世界'","cwd":"/tmp","wait_ms":0}),
    );
    wait_for(&socket, "job_unicode");
    let first = call(
        &socket,
        json!({"kind":"read","job_id":"job_unicode","cursor":0,"max_bytes":4,"wait_ms":0}),
    );
    assert_eq!(first["output"], "ab");
    assert_eq!(first["nextCursor"], 2);
    let second = call(
        &socket,
        json!({"kind":"read","job_id":"job_unicode","cursor":2,"max_bytes":4,"wait_ms":0}),
    );
    assert_eq!(second["output"], "世");
    assert_eq!(second["nextCursor"], 5);
    let third = call(
        &socket,
        json!({"kind":"read","job_id":"job_unicode","cursor":5,"max_bytes":4,"wait_ms":0}),
    );
    assert_eq!(third["output"], "界");
    assert_eq!(third["nextCursor"], 8);

    call(
        &socket,
        json!({"kind":"exec","job_id":"job_input","request_hash":"hash_input","script":"IFS= read -r line; printf 'got=%s\\n' \"$line\"","cwd":"/tmp","wait_ms":0}),
    );
    let input = call(
        &socket,
        json!({"kind":"input","job_id":"job_input","data_b64":"aGVsbG8K"}),
    );
    assert!(input["ok"].as_bool().unwrap());
    let done = wait_for(&socket, "job_input");
    assert_eq!(done["output"], "got=hello\n");
    let late = call(
        &socket,
        json!({"kind":"input","job_id":"job_input","data_b64":"d2hvYW1pCg=="}),
    );
    assert_eq!(late["error"], "job-not-running");

    call(
        &socket,
        json!({"kind":"exec","job_id":"job_interrupt","request_hash":"hash_interrupt","script":"sleep 30","cwd":"/tmp","wait_ms":0}),
    );
    let interrupted = call(
        &socket,
        json!({"kind":"interrupt","job_id":"job_interrupt"}),
    );
    assert!(interrupted["ok"].as_bool().unwrap());
    let done = wait_for(&socket, "job_interrupt");
    assert_eq!(done["job"]["terminationSignal"], 2);
    assert_eq!(done["job"]["interruptRequested"], true);

    let shutdown = call(&socket, json!({"kind":"shutdown"}));
    assert!(shutdown["ok"].as_bool().unwrap());
    assert!(runner.0.wait().unwrap().success());
    std::fs::remove_dir(&root).unwrap();
}
