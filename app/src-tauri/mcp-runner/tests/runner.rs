//! Black-box coverage for the production visible direct-execution runner.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

struct Runner(Child);

fn create_private_dir(path: &Path) {
    let mut builder = std::fs::DirBuilder::new();
    builder.mode(0o700).create(path).unwrap();
}

/// Start a runner in a private directory under MCP control (epoch 1).
fn start_runner(tag: &str) -> (Runner, std::path::PathBuf, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!("deck-mcp-runner-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    create_private_dir(&root);
    let socket = root.join("runner.sock");
    let child = Command::new(env!("CARGO_BIN_EXE_deck-mcp-runner"))
        .args([
            "--socket",
            socket.to_str().unwrap(),
            "--generation",
            "g_test",
            "--service-instance",
            "svc_test",
            "--deck-pid",
            &std::process::id().to_string(),
            "--output-retention-ms",
            "60000",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let runner = Runner(child);
    let limit = Instant::now() + Duration::from_secs(3);
    while !socket.exists() {
        assert!(Instant::now() < limit, "runner socket was not created");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777,
        0o600
    );
    claim(&socket);
    let control = call(
        &socket,
        json!({"kind":"control","mode":"mcp","service_instance":"svc_test","control_epoch":1,"holder_id":"holder_test"}),
    );
    assert_eq!(control["ok"], true, "{control}");
    authorize_grant(&socket);
    (runner, socket, root)
}

fn pid_alive(pid: i32) -> bool {
    // SAFETY: signal 0 only probes the existence of this test's descendant.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// Wait until the job has written its own pid (also its process group id).
fn job_pid(root: &Path) -> i32 {
    let file = root.join("job.pid");
    let limit = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(pid) = std::fs::read_to_string(&file)
            .ok()
            .and_then(|text| text.trim().parse::<i32>().ok())
        {
            return pid;
        }
        assert!(Instant::now() < limit, "job pid was not written");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_gone(pid: i32) -> bool {
    let limit = Instant::now() + Duration::from_secs(4);
    while Instant::now() < limit {
        if !pid_alive(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

impl Drop for Runner {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn call(socket: &Path, request: Value) -> Value {
    let mut request = request;
    request["auth"] = Value::String(
        auth_keys()
            .lock()
            .unwrap()
            .get(socket)
            .expect("runner was claimed")
            .clone(),
    );
    raw_call(socket, request)
}

fn raw_call(socket: &Path, request: Value) -> Value {
    let mut stream = UnixStream::connect(socket).unwrap();
    writeln!(stream, "{request}").unwrap();
    let mut response = String::new();
    BufReader::new(stream).read_line(&mut response).unwrap();
    serde_json::from_str(&response).unwrap()
}

fn auth_keys() -> &'static Mutex<std::collections::HashMap<std::path::PathBuf, String>> {
    static KEYS: OnceLock<Mutex<std::collections::HashMap<std::path::PathBuf, String>>> =
        OnceLock::new();
    KEYS.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn claim(socket: &Path) -> String {
    let response = raw_call(
        socket,
        json!({"kind":"claim","service_instance":"svc_test","generation":"g_test"}),
    );
    let key = response["auth"]
        .as_str()
        .unwrap_or_else(|| panic!("claim returned a key: {response}"))
        .to_owned();
    auth_keys()
        .lock()
        .unwrap()
        .insert(socket.to_owned(), key.clone());
    key
}

fn authorize_grant(socket: &Path) {
    let response = call(
        socket,
        json!({"kind":"authorize-grant","service_instance":"svc_test","grant_id":"grant_test","grant_version":1,"policy_version":2,"expires_at":u64::MAX}),
    );
    assert_eq!(response["ok"], true, "{response}");
}

fn wait_for(socket: &Path, job: &str) -> Value {
    let limit = Instant::now() + Duration::from_secs(3);
    loop {
        let value = call(
            socket,
            json!({"kind":"read","job_id":job,"cursor":0,"max_bytes":16384,"wait_ms":100}),
        );
        if value["job"]["state"] != "running"
            && value["job"]["state"] != "starting"
            && value["job"]["outputComplete"] == true
        {
            return value;
        }
        assert!(Instant::now() < limit, "job did not exit: {value}");
    }
}

fn context(marker: char) -> Value {
    json!({
        "service_instance":"svc_test",
        "holder_id":"holder_test",
        "control_epoch":1,
        "grant_id":"grant_test",
        "grant_version":1,
        "policy_version":2,
        "intent_hash":marker.to_string().repeat(64),
        "expires_at":u64::MAX
    })
}

#[test]
fn authentication_epoch_and_grant_fences_survive_attacker_requests() {
    let (_runner, socket, root) = start_runner("auth-fences");
    let missing = raw_call(
        &socket,
        json!({"kind":"ping","service_instance":"svc_test"}),
    );
    let wrong = raw_call(
        &socket,
        json!({"kind":"ping","service_instance":"svc_test","auth":"wrong"}),
    );
    assert_eq!(missing["error"], "authentication-failed");
    assert_eq!(wrong["error"], missing["error"]);

    let jump = call(
        &socket,
        json!({"kind":"control","mode":"mcp","service_instance":"svc_test","control_epoch":u64::MAX,"holder_id":"attacker"}),
    );
    assert_eq!(jump["error"], "dispatch-context-invalid");

    let mut forged = context('9');
    forged["grant_id"] = json!("grant_forged");
    let exec = call(
        &socket,
        json!({"kind":"exec","job_id":"job_forged","request_hash":"hash_forged","executable":"/usr/bin/true","args":[],"cwd":root,"wait_ms":0,"context":forged}),
    );
    assert_eq!(exec["error"], "dispatch-context-invalid");

    let takeover = call(
        &socket,
        json!({"kind":"control","mode":"human","service_instance":"svc_test","control_epoch":2,"holder_id":null}),
    );
    assert_eq!(takeover["ok"], true, "{takeover}");
    let stolen = raw_call(
        &socket,
        json!({"kind":"control","mode":"mcp","service_instance":"svc_test","control_epoch":3,"holder_id":"attacker","auth":"wrong"}),
    );
    assert_eq!(stolen["error"], "authentication-failed");
    assert_eq!(call(&socket, json!({"kind":"ping"}))["control"], "human");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn reports_exit_input_and_interrupt_without_terminal_markers() {
    let root = std::env::temp_dir().join(format!("deck-mcp-runner-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    create_private_dir(&root);
    let socket = root.join("runner.sock");
    let child = Command::new(env!("CARGO_BIN_EXE_deck-mcp-runner"))
        .args([
            "--socket",
            socket.to_str().unwrap(),
            "--generation",
            "g_test",
            "--service-instance",
            "svc_test",
            "--deck-pid",
            &std::process::id().to_string(),
            "--output-retention-ms",
            "60000",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env("DECK_SYNTHETIC_SECRET", "must-not-reach-job")
        .spawn()
        .unwrap();
    let mut runner = Runner(child);
    let limit = Instant::now() + Duration::from_secs(3);
    while !socket.exists() {
        assert!(Instant::now() < limit, "runner socket was not created");
        std::thread::sleep(Duration::from_millis(10));
    }
    claim(&socket);
    let control = call(
        &socket,
        json!({"kind":"control","mode":"mcp","service_instance":"svc_test","control_epoch":1,"holder_id":"holder_test"}),
    );
    assert_eq!(control["ok"], true, "{control}");
    authorize_grant(&socket);

    let started = call(
        &socket,
        json!({"kind":"exec","job_id":"job_ok","request_hash":"hash_ok","executable":"/bin/zsh","args":["-c","printf 'fake exited 0\\n'; exit 17"],"cwd":"/tmp","wait_ms":0,"context":context('a')}),
    );
    assert!(started["ok"].as_bool().unwrap());
    let done = wait_for(&socket, "job_ok");
    assert_eq!(done["job"]["exitCode"], 17);
    assert_eq!(done["output"], "fake exited 0\n");
    assert_eq!(done["job"]["outputComplete"], true);

    call(
        &socket,
        json!({"kind":"exec","job_id":"job_env","request_hash":"hash_env","executable":"/bin/zsh","args":["-c","if [[ -n ${DECK_SYNTHETIC_SECRET-} ]]; then print leaked; exit 9; fi; print clean"],"cwd":"/tmp","wait_ms":0,"context":context('b')}),
    );
    let clean = wait_for(&socket, "job_env");
    assert_eq!(clean["job"]["exitCode"], 0);
    assert_eq!(clean["output"], "clean\n");

    call(
        &socket,
        json!({"kind":"exec","job_id":"job_unicode","request_hash":"hash_unicode","executable":"/usr/bin/printf","args":["ab世界"],"cwd":"/tmp","wait_ms":0,"context":context('c')}),
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
        json!({"kind":"exec","job_id":"job_input","request_hash":"hash_input","executable":"/bin/zsh","args":["-c","IFS= read -r line; printf 'got=%s\\n' \"$line\""],"cwd":"/tmp","wait_ms":0,"context":context('d')}),
    );
    let input = call(
        &socket,
        json!({"kind":"input","job_id":"job_input","data_b64":"aGVsbG8K","context":context('d')}),
    );
    assert!(input["ok"].as_bool().unwrap());
    let done = wait_for(&socket, "job_input");
    assert_eq!(done["output"], "got=hello\n");
    let late = call(
        &socket,
        json!({"kind":"input","job_id":"job_input","data_b64":"d2hvYW1pCg==","context":context('d')}),
    );
    assert_eq!(late["error"], "job-not-running");

    call(
        &socket,
        json!({"kind":"exec","job_id":"job_interrupt","request_hash":"hash_interrupt","executable":"/bin/zsh","args":["-c","sleep 30"],"cwd":"/tmp","wait_ms":0,"context":context('e')}),
    );
    let interrupted = call(
        &socket,
        json!({"kind":"interrupt","job_id":"job_interrupt","context":context('e')}),
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

#[test]
fn large_arguments_and_full_reads_cross_the_accepted_socket_intact() {
    let (_runner, socket, root) = start_runner("payload");
    let body = format!("printf '%s' '{}'\n", "y".repeat(16 * 1024));
    let padding = format!(": '{}'\n", "x".repeat(32 * 1024 - body.len() - 8));
    let script = format!("{padding}{body}");
    assert!(script.len() <= 32 * 1024);
    for round in 0..20 {
        let job = format!("job_large_{round}");
        let mut context = context('f');
        context["intent_hash"] = json!(format!("{round:064x}"));
        let started = call(
            &socket,
            json!({"kind":"exec","job_id":job,"request_hash":format!("hash_{round}"),"executable":"/bin/zsh","args":["-c",script],"cwd":root,"wait_ms":3000,"context":context}),
        );
        assert_eq!(started["ok"], true, "round {round}: {started}");
        let done = wait_for(&socket, &job);
        assert_eq!(
            done["output"].as_str().unwrap().len(),
            16 * 1024,
            "round {round}"
        );
    }
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn stop_escalates_and_reaps_the_job_group() {
    let (_runner, socket, root) = start_runner("stop");
    let script = format!(
        "print $$ > {}; trap '' INT; sleep 600",
        root.join("job.pid").display()
    );
    call(
        &socket,
        json!({"kind":"exec","job_id":"job_stop","request_hash":"hash_stop","executable":"/bin/zsh","args":["-c",script],"cwd":root,"wait_ms":0,"context":context('a')}),
    );
    let pid = job_pid(&root);
    let stopped = call(&socket, json!({"kind":"stop","generation":"g_test"}));
    assert_eq!(stopped["ok"], true, "{stopped}");
    assert_eq!(stopped["control"], "fenced");
    assert!(wait_gone(pid), "job group survived stop");
    let job = call(
        &socket,
        json!({"kind":"read","job_id":"job_stop","cursor":0,"max_bytes":16,"wait_ms":0}),
    );
    assert_eq!(job["job"]["state"], "exited");
    assert_eq!(call(&socket, json!({"kind":"ping"}))["job"], Value::Null);
    let wrong = call(&socket, json!({"kind":"stop","generation":"g_other"}));
    assert_eq!(wrong["error"], "invalid-request");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn runner_termination_kills_the_live_job_group() {
    let (mut runner, socket, root) = start_runner("term");
    let script = format!("print $$ > {}; sleep 600", root.join("job.pid").display());
    call(
        &socket,
        json!({"kind":"exec","job_id":"job_term","request_hash":"hash_term","executable":"/bin/zsh","args":["-c",script],"cwd":root,"wait_ms":0,"context":context('b')}),
    );
    let pid = job_pid(&root);
    // SAFETY: SIGTERM targets only this test's own runner child.
    unsafe { libc::kill(runner.0.id() as i32, libc::SIGTERM) };
    let status = runner.0.wait().unwrap();
    assert!(status.success(), "{status:?}");
    assert!(wait_gone(pid), "job outlived its runner");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn human_interrupt_key_reaches_the_job_group_not_the_runner() {
    let (runner, socket, root) = start_runner("human-int");
    let runner_pid = runner.0.id() as i32;
    let script = format!("print $$ > {}; sleep 600", root.join("job.pid").display());
    call(
        &socket,
        json!({"kind":"exec","job_id":"job_human","request_hash":"hash_human","executable":"/bin/zsh","args":["-c",script],"cwd":root,"wait_ms":0,"context":context('c')}),
    );
    let pid = job_pid(&root);
    // In MCP mode a terminal ^C is swallowed: neither runner nor job dies.
    // SAFETY: every signal below targets only this test's own runner child.
    unsafe { libc::kill(runner_pid, libc::SIGINT) };
    assert_eq!(call(&socket, json!({"kind":"ping"}))["ok"], true);
    assert!(pid_alive(pid), "MCP-mode ^C must not reach the job");
    let human = call(
        &socket,
        json!({"kind":"control","mode":"human","service_instance":"svc_test","control_epoch":2,"holder_id":null}),
    );
    assert_eq!(human["ok"], true);
    unsafe { libc::kill(runner_pid, libc::SIGINT) };
    let done = wait_for(&socket, "job_human");
    assert_eq!(done["job"]["terminationSignal"], 2);
    assert_eq!(done["job"]["interruptRequested"], true);
    assert!(pid_alive(runner_pid), "the runner must survive ^C");
    // ^Z must not stop the runner either.
    unsafe { libc::kill(runner_pid, libc::SIGTSTP) };
    assert_eq!(call(&socket, json!({"kind":"ping"}))["control"], "human");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_job_stopped_by_job_control_is_reported_and_still_stoppable() {
    let (_runner, socket, root) = start_runner("stopped");
    call(
        &socket,
        json!({"kind":"exec","job_id":"job_tstp","request_hash":"hash_tstp","executable":"/bin/zsh","args":["-c","kill -STOP $$; sleep 600"],"cwd":root,"wait_ms":0,"context":context('d')}),
    );
    let limit = Instant::now() + Duration::from_secs(3);
    loop {
        let ping = call(&socket, json!({"kind":"ping"}));
        if ping["job"]["state"] == "stopped" {
            break;
        }
        assert!(
            Instant::now() < limit,
            "stopped state was not reported: {ping}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let stopped = call(&socket, json!({"kind":"stop","generation":"g_test"}));
    assert_eq!(stopped["ok"], true, "{stopped}");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_restarted_deck_is_reported_stale_but_can_still_stop() {
    let (_runner, socket, root) = start_runner("stale");
    let ping = call(
        &socket,
        json!({"kind":"ping","service_instance":"svc_restarted"}),
    );
    assert_eq!(ping["serviceCurrent"], false);
    assert!(ping["runnerVersion"].is_string());
    let current = call(
        &socket,
        json!({"kind":"ping","service_instance":"svc_test"}),
    );
    assert_eq!(current["serviceCurrent"], true);
    let control = call(
        &socket,
        json!({"kind":"control","mode":"human","service_instance":"svc_restarted","control_epoch":9,"holder_id":null}),
    );
    assert_eq!(control["error"], "runner-stale");
    assert_eq!(
        call(&socket, json!({"kind":"stop","generation":"g_test"}))["ok"],
        true
    );
    std::fs::remove_dir_all(&root).unwrap();
}
