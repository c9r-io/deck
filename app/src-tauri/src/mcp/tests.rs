//! MCP unit tests: an in-process runner double plus the production control
//! lifecycle. Split out of `mcp.rs` on 2026-09-23.

use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

/// In-process runner double. Every connection is served on its own
/// thread so a held request never blocks unrelated ones.
struct FakeRunner {
    socket: PathBuf,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// Every request received, in order.
    seen: Arc<Mutex<Vec<Value>>>,
    /// A managed job is live: ping reports it and control mcp is busy.
    busy: Arc<AtomicBool>,
    /// Requests of this kind park until `release` is sent.
    hold: Arc<Mutex<Option<HeldKind>>>,
    /// Runner control state uses the production monotonic epoch contract.
    control: Arc<Mutex<FakeControl>>,
    /// Reject one control request without advancing the runner epoch.
    fail_next_control: Arc<AtomicBool>,
}

struct FakeControl {
    epoch: u64,
    mode: String,
    holder: Option<String>,
    /// The next `exec` is answered with this runner error instead.
    exec_error: Option<String>,
}

struct HeldKind {
    kind: String,
    entered: std::sync::mpsc::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

impl FakeRunner {
    fn start(root: &Path, generation: &str) -> Self {
        let socket = root.join("runner.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let busy = Arc::new(AtomicBool::new(false));
        let hold: Arc<Mutex<Option<HeldKind>>> = Arc::new(Mutex::new(None));
        let control = Arc::new(Mutex::new(FakeControl {
            epoch: 0,
            mode: "fenced".into(),
            holder: None,
            exec_error: None,
        }));
        let fail_next_control = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let generation = generation.to_owned();
        let (thread_seen, thread_busy, thread_hold, thread_control, thread_fail_control) = (
            seen.clone(),
            busy.clone(),
            hold.clone(),
            control.clone(),
            fail_next_control.clone(),
        );
        let thread = std::thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                let Ok((stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                };
                let (generation, seen, busy, hold, control, fail_control) = (
                    generation.clone(),
                    thread_seen.clone(),
                    thread_busy.clone(),
                    thread_hold.clone(),
                    thread_control.clone(),
                    thread_fail_control.clone(),
                );
                std::thread::spawn(move || {
                    Self::serve(
                        stream,
                        &generation,
                        &seen,
                        &busy,
                        &hold,
                        &control,
                        &fail_control,
                    )
                });
            }
        });
        Self {
            socket,
            stop,
            thread: Some(thread),
            seen,
            busy,
            hold,
            control,
            fail_next_control,
        }
    }

    fn serve(
        mut stream: UnixStream,
        generation: &str,
        seen: &Mutex<Vec<Value>>,
        busy: &AtomicBool,
        hold: &Mutex<Option<HeldKind>>,
        control: &Mutex<FakeControl>,
        fail_next_control: &AtomicBool,
    ) {
        stream.set_nonblocking(false).unwrap();
        let mut line = String::new();
        if BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut line)
            .is_err()
        {
            return;
        }
        let request: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
        seen.lock_or_recover().push(request.clone());
        let kind = request.get("kind").and_then(Value::as_str).unwrap_or("");
        let held = {
            let mut slot = hold.lock_or_recover();
            if slot.as_ref().is_some_and(|held| held.kind == kind) {
                slot.take()
            } else {
                None
            }
        };
        if let Some(held) = held {
            held.entered.send(()).unwrap();
            held.release
                .recv_timeout(Duration::from_secs(10))
                .expect("held runner request was never released");
        }
        let job_id = request
            .get("job_id")
            .and_then(Value::as_str)
            .unwrap_or("job_a");
        let service = request
            .get("service_instance")
            .or_else(|| {
                request
                    .get("context")
                    .and_then(|context| context.get("service_instance"))
            })
            .and_then(Value::as_str)
            .unwrap_or("");
        let stale = service == "svc_stale";
        let live = busy.load(Ordering::SeqCst);
        let dispatch_matches = || {
            let current = control.lock_or_recover();
            let context = request.get("context").unwrap_or(&Value::Null);
            current.mode == "mcp"
                && context.get("control_epoch").and_then(Value::as_u64) == Some(current.epoch)
                && context.get("holder_id").and_then(Value::as_str) == current.holder.as_deref()
        };
        let response = match kind {
            "claim" => json!({"ok":true,"generation":generation,
                "auth":base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([7u8;32])}),
            "ping" => json!({"ok":true,"generation":generation,
                "job": if live { json!({"jobId":"job_live","state":"running"}) } else { Value::Null },
                "serviceCurrent":!service.is_empty() && !stale,"runnerVersion":"0.1.0"}),
            _ if stale => json!({"ok":false,"generation":generation,"error":"runner-stale"}),
            "control" if fail_next_control.swap(false, Ordering::SeqCst) => {
                json!({"ok":false,"generation":generation,"error":"dispatch-context-invalid"})
            }
            "control" if live && request["mode"] == "mcp" => {
                json!({"ok":false,"generation":generation,"error":"session-busy"})
            }
            "control" => {
                let epoch = request["control_epoch"].as_u64().unwrap_or(0);
                let mut current = control.lock_or_recover();
                if epoch <= current.epoch {
                    json!({"ok":false,"generation":generation,"error":"dispatch-context-invalid"})
                } else {
                    current.epoch = epoch;
                    current.mode = request["mode"].as_str().unwrap_or("fenced").into();
                    current.holder = request["holder_id"].as_str().map(str::to_owned);
                    json!({"ok":true,"generation":generation})
                }
            }
            "read" => json!({
                "ok":true,
                "generation":generation,
                "job":{"jobId":job_id,"state":"exited","exitCode":0,
                    "terminationSignal":null,"interruptRequested":false,
                    "timeoutRequested":false,"startedAt":1,"endedAt":2},
                "output":"done\n","nextCursor":5,"gap":false,"droppedBytes":0
            }),
            "exec" if control.lock_or_recover().exec_error.is_some() => {
                let error = control.lock_or_recover().exec_error.take();
                json!({"ok":false,"generation":generation,"error":error})
            }
            "exec" if dispatch_matches() => json!({
                "ok":true,
                "generation":generation,
                "job":{"jobId":job_id,"state":"exited","exitCode":0}
            }),
            "exec" | "input" | "interrupt" if !dispatch_matches() => {
                json!({"ok":false,"generation":generation,"error":"control-revoked"})
            }
            "input" | "interrupt" | "retention" | "revoke-grant" | "authorize-grant" | "stop" => {
                json!({"ok":true,"generation":generation})
            }
            _ => json!({"ok":false,"generation":generation,"error":"invalid-request"}),
        };
        if serde_json::to_writer(&mut stream, &response).is_ok() {
            let _ = stream.write_all(b"\n");
        }
    }

    fn count(&self, kind: &str) -> usize {
        self.seen
            .lock_or_recover()
            .iter()
            .filter(|request| request["kind"] == kind)
            .count()
    }

    fn last(&self, kind: &str) -> Option<Value> {
        self.seen
            .lock_or_recover()
            .iter()
            .rev()
            .find(|request| request["kind"] == kind)
            .cloned()
    }

    fn set_control(&self, epoch: u64, mode: &str, holder: Option<&str>) {
        *self.control.lock_or_recover() = FakeControl {
            epoch,
            mode: mode.into(),
            holder: holder.map(str::to_owned),
            exec_error: None,
        };
    }

    fn control_epoch(&self) -> u64 {
        self.control.lock_or_recover().epoch
    }

    fn fail_next_exec(&self, error: &str) {
        self.control.lock_or_recover().exec_error = Some(error.into());
    }

    fn fail_next_control(&self) {
        self.fail_next_control.store(true, Ordering::SeqCst);
    }

    /// Park the next request of `kind`; returns (entered, release).
    fn hold(&self, kind: &str) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        *self.hold.lock_or_recover() = Some(HeldKind {
            kind: kind.into(),
            entered: entered_tx,
            release: release_rx,
        });
        (entered_rx, release_tx)
    }
}

impl Drop for FakeRunner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = UnixStream::connect(&self.socket);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
        let _ = std::fs::remove_file(&self.socket);
    }
}

fn test_root(tag: &str) -> PathBuf {
    // A per-process counter instead of a timestamp keeps runner socket
    // paths below SUN_LEN under a long macOS TMPDIR.
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "deck-mcp-{tag}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn control_socket_is_private_before_its_public_name_exists() {
    let root = test_root("private-control-socket");
    let socket = root.join("mcp-control.sock");
    let listener = bind_private_socket(&socket).unwrap();
    assert_eq!(
        std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::read_dir(&root)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name() == ".mcp-bind")
            .count(),
        0
    );
    drop(listener);
    std::fs::remove_file(socket).unwrap();
    std::fs::remove_dir(root).unwrap();
}

fn client_record(root: &Path) -> Client {
    Client {
        id: "client_a".into(),
        name: "Client A".into(),
        credential_hash: sha(b"mcp_test"),
        credential_version: 1,
        revoked_at: None,
        allow_create: true,
        projects: vec![ProjectScope {
            project_id: "P1".into(),
            // Authorized roots are canonical (`/var` is a link on macOS).
            roots: vec![std::fs::canonicalize(root).unwrap().display().to_string()],
        }],
        create_sequence: 0,
    }
}

fn session_record(root: &Path, runner: &FakeRunner) -> ManagedSession {
    ManagedSession {
        session_id: "mcp_a".into(),
        card_id: "M1".into(),
        tmux_session: "deck-mcp-test".into(),
        project_id: "P1".into(),
        title: "MCP shell".into(),
        cwd: root.display().to_string(),
        generation: "g_a".into(),
        runner_socket: runner.socket.display().to_string(),
        owner_client_id: "client_a".into(),
        control_owner: Some("client_a".into()),
        control_holder: Some("holder_a".into()),
        control_epoch: 1,
        lease_expires_at: Some(now_ms() + 60_000),
        human_lock: false,
        output_shared: true,
        closing: false,
        created_at: now_ms(),
        control_sequence: 0,
    }
}

fn execution_grant(session: &ManagedSession) -> ExecutionGrant {
    ExecutionGrant {
        grant_id: "grant_test".into(),
        client_id: session.owner_client_id.clone(),
        credential_version: 1,
        project_id: session.project_id.clone(),
        session_id: session.session_id.clone(),
        session_generation: session.generation.clone(),
        profile: "trusted-host-v1".into(),
        environment_profile: ENVIRONMENT_PROFILE.into(),
        environment_profile_version: 1,
        issued_at: now_ms(),
        expires_at: now_ms() + 60_000,
        issued_monotonic_ms: 0,
        duration_ms: 60_000,
        allow_stdin: true,
        allow_output: true,
        _legacy_allow_shell: None,
        grant_version: 1,
        revocation_version: 0,
        service_instance: "svc_test".into(),
        revoked_at: None,
    }
}

#[test]
fn inspect_separates_execution_authorization_from_output_sharing() {
    let root = test_root("inspect");
    let runner = FakeRunner::start(&root, "g_a");
    let session = session_record(&root, &runner);
    let mut doc = DiskDoc::default();
    doc.config.enabled = true;
    doc.config.clients.push(client_record(&root));
    doc.sessions.push(session.clone());
    let runtime = Runtime {
        app: None,
        path: root.join("mcp.json"),
        socket: root.join("control.sock"),
        doc: Mutex::new(Ok(doc)),
        io: Mutex::new(()),
        delivery: Mutex::new(()),
        emergency: Mutex::new(EmergencyFences::default()),
        service_instance: "svc_test".into(),
        runner_auth: Mutex::new(HashMap::new()),
        started: Instant::now(),
    };
    let view = || inspect(&runtime, "client_a", json!({"session_id":"mcp_a"})).unwrap();

    let none = view();
    assert_eq!(none["executionAuthorization"]["status"], "none");
    assert_eq!(none["outputSharing"]["sessionGateOpen"], true);

    runtime
        .write(|doc| {
            doc.execution_grants.push(execution_grant(&session));
            Ok(())
        })
        .unwrap();
    let active = view();
    assert_eq!(active["executionAuthorization"]["status"], "active");
    assert_eq!(
        active["executionAuthorization"]["stdinApprovedForActiveGrant"],
        true
    );
    assert!(active["executionAuthorization"]
        .get("shellApprovedForActiveGrant")
        .is_none());
    runtime
        .write(|doc| {
            doc.sessions[0].control_owner = None;
            doc.sessions[0].lease_expires_at = None;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        view()["executionAuthorization"]["status"],
        "active",
        "control lease is independent"
    );
    runtime
        .write(|doc| {
            doc.sessions[0].human_lock = true;
            Ok(())
        })
        .unwrap();
    let human = view();
    assert_eq!(human["executionAuthorization"]["status"], "active");
    assert_eq!(human["outputSharing"]["sessionGateOpen"], false);
    assert!(
        human.get("controlEpoch").is_some(),
        "control metadata remains visible during takeover"
    );
    runtime
        .write(|doc| {
            doc.sessions[0].human_lock = false;
            Ok(())
        })
        .unwrap();

    runtime
        .write(|doc| {
            doc.execution_grants[0].expires_at = doc.execution_grants[0].issued_at;
            Ok(())
        })
        .unwrap();
    let expired = view();
    assert_eq!(expired["executionAuthorization"]["status"], "expired");
    assert_eq!(
        expired["outputSharing"]["sessionGateOpen"], true,
        "retained output sharing is independent"
    );

    runtime
        .write(|doc| {
            doc.execution_grants[0].revoked_at = Some(now_ms());
            doc.execution_grants[0].revocation_version = 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(view()["executionAuthorization"]["status"], "revoked");

    runtime
        .write(|doc| {
            doc.sessions[0].output_shared = false;
            Ok(())
        })
        .unwrap();
    let paused = view();
    assert_eq!(paused["executionAuthorization"]["status"], "revoked");
    assert_eq!(paused["outputSharing"]["sessionGateOpen"], false);
    assert!(
        paused.get("controlEpoch").is_some(),
        "control metadata remains visible"
    );

    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn runner_exchange_writes_nothing_to_a_socket_another_process_serves() {
    let root = test_root("runner-peer-pid");
    let socket = root.join("runner.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let reader = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut received = Vec::new();
        stream.read_to_end(&mut received).unwrap();
        received
    });
    // this process serves the socket; any other pid is an impostor
    let impostor = std::process::id() as libc::pid_t + 1;
    let err = runner_exchange(
        socket.to_str().unwrap(),
        "g_a",
        impostor,
        &json!({"kind":"ping","auth":"secret"}),
    )
    .unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Perm);
    assert!(
        reader.join().unwrap().is_empty(),
        "no byte reaches the impostor"
    );
}

#[test]
fn control_socket_exists_only_while_mcp_is_enabled() {
    let root = test_root("conditional-control-socket");
    let runtime = Runtime {
        app: None,
        path: root.join("mcp.json"),
        socket: root.join("mcp-control.sock"),
        doc: Mutex::new(Ok(DiskDoc::default())),
        io: Mutex::new(()),
        delivery: Mutex::new(()),
        emergency: Mutex::new(EmergencyFences::default()),
        service_instance: "svc_socket".into(),
        runner_auth: Mutex::new(HashMap::new()),
        started: Instant::now(),
    };
    let mut listener = None;
    reconcile_control_socket(&runtime, &mut listener).unwrap();
    assert!(listener.is_none());
    assert!(!runtime.socket.exists());

    runtime
        .doc
        .lock_or_recover()
        .as_mut()
        .unwrap()
        .config
        .enabled = true;
    reconcile_control_socket(&runtime, &mut listener).unwrap();
    assert!(listener.is_some());
    assert_eq!(
        std::fs::metadata(&runtime.socket)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );

    runtime
        .doc
        .lock_or_recover()
        .as_mut()
        .unwrap()
        .config
        .enabled = false;
    reconcile_control_socket(&runtime, &mut listener).unwrap();
    assert!(listener.is_none());
    assert!(!runtime.socket.exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn scope_preview_classifies_paths_and_final_scope_check_rejects_stale_project() {
    let root = test_root("scope-preview");
    let missing = mcp_scope_preview(root.join("missing").display().to_string());
    assert!(!missing.ok);
    assert_eq!(missing.error, Some("not_found"));
    let file = root.join("file.txt");
    std::fs::write(&file, b"file").unwrap();
    let not_directory = mcp_scope_preview(file.display().to_string());
    assert!(!not_directory.ok);
    assert_eq!(not_directory.error, Some("not_directory"));
    let valid = mcp_scope_preview(root.display().to_string());
    assert!(valid.ok);
    let whole_disk = mcp_scope_preview("/".into());
    assert!(!whole_disk.ok);
    assert_eq!(whole_disk.error, Some("too_broad"));
    let home = crate::mcp_fs::home_directory().unwrap();
    assert_eq!(
        mcp_scope_preview(home.display().to_string()).error,
        Some("too_broad")
    );
    let inaccessible = canonical_project_root_with_access(root.to_str().unwrap(), |_| {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "synthetic access denial",
        ))
    })
    .unwrap_err();
    assert_eq!(inaccessible.kind(), ErrorKind::Perm);

    let stale = match validate_new_client_scopes(
        vec![ProjectScopeInput {
            project_id: "P1".into(),
            roots: vec![root.display().to_string()],
        }],
        true,
        |_| Ok(false),
    ) {
        Err(error) => error,
        Ok(_) => panic!("stale project was accepted"),
    };
    assert_eq!(stale.kind(), ErrorKind::Missing);
    std::fs::remove_dir_all(root).unwrap();
}

fn request(tool: &str, arguments: Value) -> WireRequest {
    WireRequest {
        version: CONTROL_PROTOCOL,
        client_id: "client_a".into(),
        credential: "mcp_test".into(),
        tool: tool.into(),
        arguments,
    }
}

fn operation(kind: &str, state: &str) -> Operation {
    Operation {
        operation_id: "op_a".into(),
        client_id: "client_a".into(),
        request_id: "request_a".into(),
        request_hash: "a".repeat(64),
        kind: kind.into(),
        state: state.into(),
        code: None,
        result: None,
        accepted_at: 1,
        updated_at: 1,
        admission_hash: None,
        session_id: None,
        control_epoch: None,
        control_sequence: None,
    }
}

#[test]
fn fresh_state_is_disabled_and_strictly_bounded() {
    let doc = DiskDoc::default();
    assert!(!doc.config.enabled);
    validate_doc(&doc).unwrap();
    assert_eq!(MAX_RESPONSE_BYTES, 128 * 1024);
}

#[test]
fn cursor_is_bound_to_job_and_generation() {
    let binding = JobBinding {
        job_id: "job_a".into(),
        client_id: "client_a".into(),
        session_id: "mcp_a".into(),
        session_generation: "g_a".into(),
        request_hash: "a".repeat(64),
        operation_id: "op_a".into(),
        grant_id: "grant_a".into(),
        grant_version: 1,
        allow_output: true,
    };
    assert_eq!(
        decode_cursor(Some("g_a:job_a:42".into()), &binding).unwrap(),
        42
    );
    assert!(decode_cursor(Some("g_b:job_a:42".into()), &binding).is_err());
    assert!(decode_cursor(Some("g_a:job_b:42".into()), &binding).is_err());
}

#[test]
fn request_id_conflict_is_distinct_and_never_reused() {
    let mut doc = DiskDoc::default();
    doc.operations.push(operation("exec", "committed"));
    assert!(
        existing_operation(&doc, "client_a", "request_a", &"a".repeat(64))
            .unwrap()
            .is_some()
    );
    let error = match existing_operation(&doc, "client_a", "request_a", &"b".repeat(64)) {
        Err(error) => error,
        Ok(_) => panic!("conflicting request id was accepted"),
    };
    assert_eq!(error.kind(), ErrorKind::RequestConflict);
}

#[test]
fn revoked_control_has_its_own_protocol_error() {
    let session = ManagedSession {
        session_id: "mcp_a".into(),
        card_id: "M1".into(),
        tmux_session: "deck-mcp-a".into(),
        project_id: "P1".into(),
        title: "MCP shell".into(),
        cwd: "/tmp".into(),
        generation: "g_a".into(),
        runner_socket: "/tmp/runner.sock".into(),
        owner_client_id: "client_a".into(),
        control_owner: None,
        control_holder: None,
        control_epoch: 2,
        lease_expires_at: None,
        human_lock: true,
        output_shared: false,
        closing: false,
        created_at: 1,
        control_sequence: 0,
    };
    let error = check_control(&session, "client_a", "g_a", 1, "holder_a").unwrap_err();
    assert_eq!(error.kind(), ErrorKind::ControlRevoked);
    assert_eq!(map_error(error)["error"]["code"], "CONTROL_REVOKED");
}

#[test]
fn authorization_is_client_project_session_and_control_scoped() {
    let root = std::fs::canonicalize("/tmp").unwrap().display().to_string();
    let mut doc = DiskDoc::default();
    doc.config.clients.push(Client {
        id: "client_a".into(),
        name: "Client A".into(),
        credential_hash: sha(b"mcp_test"),
        credential_version: 1,
        revoked_at: None,
        allow_create: true,
        projects: vec![ProjectScope {
            project_id: "P1".into(),
            roots: vec![root.clone()],
        }],
        create_sequence: 0,
    });
    let disabled = match client(&doc, "client_a") {
        Err(error) => error,
        Ok(_) => panic!("disabled MCP state authorized a client"),
    };
    assert_eq!(disabled.kind(), ErrorKind::Perm);
    doc.config.enabled = true;
    assert!(client(&doc, "client_missing").is_err());
    assert!(scoped_project(client(&doc, "client_a").unwrap(), "P2").is_err());
    assert_eq!(
        canonical_scope("/tmp", &[root]).unwrap(),
        std::fs::canonicalize("/tmp").unwrap()
    );

    doc.sessions.push(ManagedSession {
        session_id: "mcp_a".into(),
        card_id: "M1".into(),
        tmux_session: "deck-mcp-a".into(),
        project_id: "P1".into(),
        title: "MCP shell".into(),
        cwd: "/tmp".into(),
        generation: "g_a".into(),
        runner_socket: "/tmp/runner.sock".into(),
        owner_client_id: "client_a".into(),
        control_owner: Some("client_a".into()),
        control_holder: Some("holder_a".into()),
        control_epoch: 2,
        lease_expires_at: Some(now_ms() + 10_000),
        human_lock: false,
        output_shared: true,
        closing: false,
        created_at: 1,
        control_sequence: 0,
    });
    assert!(authorized_session(&doc, "client_missing", "mcp_a").is_err());
    let session = authorized_session(&doc, "client_a", "mcp_a").unwrap();
    assert!(check_control(session, "client_a", "g_other", 2, "holder_a").is_err());
    assert!(check_control(session, "client_a", "g_a", 1, "holder_a").is_err());
    assert!(check_control(session, "client_a", "g_a", 2, "other_holder").is_err());
    assert!(check_control(session, "client_a", "g_a", 2, "holder_a").is_ok());
}

#[test]
fn restart_never_replays_an_accepted_exec() {
    let root = std::env::temp_dir().join(format!("deck-mcp-load-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir(&root).unwrap();
    let path = root.join("mcp.json");
    let mut doc = DiskDoc::default();
    doc.operations.push(operation("exec", "accepted"));
    save(&path, &doc).unwrap();
    let recovered = load(&path).unwrap();
    assert_eq!(recovered.operations[0].state, "ambiguous");
    assert_eq!(
        recovered.operations[0].code.as_deref(),
        Some("deck-restarted")
    );
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(root).unwrap();
}

#[test]
fn grant_expiry_is_audited_once_without_reviving_authority() {
    let root = test_root("grant-expiry-audit");
    let path = root.join("mcp.json");
    let mut doc = DiskDoc::default();
    doc.config.enabled = true;
    doc.config.clients.push(client_record(&root));
    let session = ManagedSession {
        session_id: "mcp_a".into(),
        card_id: "M1".into(),
        tmux_session: "deck-mcp-test".into(),
        project_id: "P1".into(),
        title: "MCP shell".into(),
        cwd: root.display().to_string(),
        generation: "g_a".into(),
        runner_socket: root.join("missing.sock").display().to_string(),
        owner_client_id: "client_a".into(),
        control_owner: Some("client_a".into()),
        control_holder: Some("holder_a".into()),
        control_epoch: 1,
        lease_expires_at: Some(now_ms() + 60_000),
        human_lock: false,
        output_shared: true,
        closing: false,
        created_at: now_ms(),
        control_sequence: 0,
    };
    let mut grant = execution_grant(&session);
    grant.issued_at = now_ms().saturating_sub(1_000);
    grant.expires_at = now_ms().saturating_sub(1);
    grant.duration_ms = 100;
    doc.execution_grants.push(grant);
    doc.sessions.push(session.clone());
    save(&path, &doc).unwrap();
    let runtime = Runtime {
        app: None,
        path,
        socket: root.join("control.sock"),
        doc: Mutex::new(Ok(doc)),
        io: Mutex::new(()),
        delivery: Mutex::new(()),
        emergency: Mutex::new(EmergencyFences::default()),
        service_instance: "svc_test".into(),
        runner_auth: Mutex::new(HashMap::new()),
        started: Instant::now(),
    };

    record_expired_grants(&runtime).unwrap();
    record_expired_grants(&runtime).unwrap();
    runtime
        .read(|doc| {
            assert!(active_execution_grant(&runtime, doc, "client_a", &session, false).is_err());
            assert_eq!(
                doc.audit
                    .iter()
                    .filter(|event| event.kind == "grant-expired")
                    .count(),
                1
            );
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn emergency_fence_survives_state_write_failure() {
    let root = test_root("ef");
    let runner = FakeRunner::start(&root, "g_a");
    let mut doc = DiskDoc::default();
    doc.config.enabled = true;
    doc.config.clients.push(client_record(&root));
    let session = session_record(&root, &runner);
    doc.sessions.push(session.clone());
    let runtime = Runtime {
        app: None,
        path: root.join("missing-parent/state.json"),
        socket: root.join("control.sock"),
        doc: Mutex::new(Ok(doc)),
        io: Mutex::new(()),
        delivery: Mutex::new(()),
        emergency: Mutex::new(EmergencyFences::default()),
        service_instance: "svc_test".into(),
        runner_auth: Mutex::new(HashMap::new()),
        started: Instant::now(),
    };
    runtime
        .emergency
        .lock_or_recover()
        .human_sessions
        .insert(session.session_id.clone());
    assert!(runtime.write(|_| Ok(())).is_err());

    let inspect = route(
        &runtime,
        request(
            "deck_session_inspect",
            json!({"session_id":"mcp_a","holder_id":"holder_a"}),
        ),
    );
    assert_eq!(inspect["humanLock"], true);
    assert!(inspect["terminalContext"].is_null());
    assert_eq!(
        route(
            &runtime,
            request(
                "deck_exec",
                json!({"session_id":"mcp_a","request_id":"blocked"}),
            ),
        )["error"]["code"],
        "HUMAN_CONTROL"
    );
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn corrupt_or_extended_state_fails_closed() {
    let root = std::env::temp_dir().join(format!("deck-mcp-corrupt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir(&root).unwrap();
    let path = root.join("mcp.json");
    std::fs::write(&path, br#"{"version":1,"config":{"enabled":true,"clients":[]},"sessions":[],"operations":[],"jobs":[],"unexpected":true}"#).unwrap();
    let error = match load(&path) {
        Err(error) => error,
        Ok(_) => panic!("extended state was accepted"),
    };
    assert_eq!(error.kind(), ErrorKind::Recovery);
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(root).unwrap();
}

#[test]
fn command_surface_exercises_local_authorization_and_board_reconciliation() {
    let root = test_root("commands");
    let runner = FakeRunner::start(&root, "g_a");
    let mut doc = DiskDoc::default();
    doc.config.enabled = true;
    doc.config.clients.push(client_record(&root));
    let session = session_record(&root, &runner);
    doc.execution_grants.push(execution_grant(&session));
    doc.sessions.push(session);
    let path = root.join("mcp.json");
    save(&path, &doc).unwrap();
    let runtime = Arc::new(Runtime {
        app: None,
        path,
        socket: root.join("control.sock"),
        doc: Mutex::new(Ok(doc)),
        io: Mutex::new(()),
        delivery: Mutex::new(()),
        emergency: Mutex::new(EmergencyFences::default()),
        service_instance: "svc_test".into(),
        runner_auth: Mutex::new(HashMap::new()),
        started: Instant::now(),
    });
    // Before the service exists nothing is managed: ordinary input, a close
    // and a server restart all pass through.
    assert!(guard_terminal_input("deck-mcp-test").is_ok());
    stop_managed_jobs("deck-mcp-test");
    assert!(guard_server_restart().unwrap().is_empty());
    assert_eq!(runner.count("stop"), 0);
    assert!(RUNTIME.set(runtime.clone()).is_ok());

    let status = mcp_status().unwrap();
    assert!(status.enabled);
    assert!(!status.socket_ready);
    assert_eq!(status.clients.len(), 1);
    assert!(mcp_adapter_path().is_err());
    assert!(mcp_client_add("".into(), vec![], false).is_err());
    assert!(mcp_client_add(
        "Bad scope".into(),
        vec![ProjectScopeInput {
            project_id: "bad id".into(),
            roots: vec![root.display().to_string()],
        }],
        false,
    )
    .is_err());
    let added = mcp_client_add(
        "Client B".into(),
        vec![ProjectScopeInput {
            project_id: "P2".into(),
            roots: vec![root.display().to_string()],
        }],
        true,
    )
    .unwrap();
    assert_eq!(added.name, "Client B");
    let preview = mcp_scope_preview(root.display().to_string());
    assert!(preview.ok);
    assert_eq!(
        preview.root.unwrap(),
        std::fs::canonicalize(&root).unwrap().display().to_string()
    );
    assert_eq!(
        mcp_client_delete(added.id.clone()).unwrap_err().kind(),
        ErrorKind::Perm
    );

    let unmanaged = mcp_session_ui("missing".into()).unwrap();
    assert!(!unmanaged.managed);
    runtime
        .write(|doc| {
            let mut failed = operation("exec", "rejected");
            failed.operation_id = "op_failed".into();
            failed.request_id = "req_failed".into();
            failed.code = Some("control-revoked".into());
            failed.result = Some(json!({"sessionId":"mcp_a"}));
            doc.operations.push(failed);
            Ok(())
        })
        .unwrap();
    let managed = mcp_session_ui("M1".into()).unwrap();
    assert!(managed.managed);
    assert!(!managed.human_control);
    assert_eq!(managed.client_name.as_deref(), Some("Client A"));
    assert_eq!(
        managed.recent_error.as_deref(),
        Some("control-revoked"),
        "the newest coded operation of this session is surfaced"
    );
    assert!(managed.execution_grant_active);
    assert!(guard_terminal_input("deck-mcp-test").is_err());
    let blockers = guard_server_restart().unwrap();
    assert_eq!(blockers.len(), 1);
    assert_eq!(
        blockers[0].kind,
        crate::tmux_lifecycle::RestartBlockerKind::ManagedSession
    );
    assert_eq!(blockers[0].card_id, "M1");

    // Retention is a closed range, applied to every live runner.
    assert_eq!(
        mcp_output_retention(59_999).unwrap_err().kind(),
        ErrorKind::Invalid
    );
    mcp_output_retention(120_000).unwrap();
    assert_eq!(
        runtime.read(|doc| doc.config.output_retention_ms).unwrap(),
        120_000
    );
    assert_eq!(
        runner.last("retention").unwrap()["output_retention_ms"],
        120_000
    );
    // The close path asks the managed runner to stop its job groups first.
    stop_managed_jobs("deck-mcp-test");
    assert_eq!(runner.last("stop").unwrap()["generation"], "g_a");
    stop_managed_jobs("deck-unmanaged");
    assert_eq!(runner.count("stop"), 1);
    // The local grant commands reach the same core as the tests above.
    mcp_execution_grant("M1".into(), Some(90_000), true, false).unwrap();
    assert!(!mcp_session_ui("M1".into()).unwrap().output_shared);
    mcp_execution_revoke("M1".into()).unwrap();
    assert!(!mcp_session_ui("M1".into()).unwrap().execution_grant_active);
    assert_eq!(
        mcp_client_delete("bad id".into()).unwrap_err().kind(),
        ErrorKind::Invalid
    );
    assert_eq!(
        mcp_close_admit("missing".into()).err().unwrap().kind(),
        ErrorKind::ContextChanged
    );
    assert!(runner_socket_matches(
        &runtime,
        "deck-mcp-test",
        runner.socket.to_str().unwrap(),
        "g_a"
    ));
    assert!(!runner_socket_matches(
        &runtime,
        "deck-mcp-test",
        runner.socket.to_str().unwrap(),
        "g_other"
    ));

    mcp_takeover("mcp_a".into()).unwrap();
    assert!(guard_terminal_input("deck-mcp-test").is_ok());
    mcp_return_control("M1".into()).unwrap();
    assert!(guard_terminal_input("deck-mcp-test").is_err());

    mcp_disable().unwrap();
    assert!(!mcp_status().unwrap().enabled);
    assert!(mcp_pending().unwrap().is_empty());
    mcp_enable().unwrap();

    let create = session_create(
        &runtime,
        "client_a",
        json!({"request_id":"command_create","project_id":"P1","cwd":root.display().to_string(),"create_sequence":0}),
    )
    .unwrap();
    let create_id = create["operationId"].as_str().unwrap().to_owned();
    assert_eq!(mcp_pending().unwrap().len(), 1);
    let claimed = mcp_claim(create_id.clone()).unwrap();
    assert_eq!(claimed.kind, "session-create");
    assert!(mcp_complete(create_id.clone(), "invalid".into(), None, None).is_err());
    mcp_complete(
        create_id,
        "committed".into(),
        None,
        Some("deck-created".into()),
    )
    .unwrap();

    let created_session = runtime
        .read(|doc| {
            doc.sessions
                .iter()
                .find(|session| session.tmux_session == "deck-created")
                .cloned()
                .unwrap()
        })
        .unwrap();
    let close = session_close(
        &runtime,
        "client_a",
        json!({
            "request_id":"command_close",
            "session_id":created_session.session_id,
            "expected_generation":created_session.generation,
            "control_epoch":1,
            "holder_id":"holder_a",
            "confirm_running":false
        }),
    );
    assert_eq!(close.unwrap_err()["error"]["code"], "CONTROL_REVOKED");
    mcp_card_closed(created_session.card_id).unwrap();

    assert!(mcp_claim("missing".into()).is_err());
    assert!(mcp_start_session(
        "missing".into(),
        "deck-valid".into(),
        root.display().to_string()
    )
    .is_err());
    mcp_client_revoke(added.id.clone()).unwrap();
    mcp_client_delete(added.id.clone()).unwrap();
    assert!(mcp_status()
        .unwrap()
        .clients
        .iter()
        .all(|client| client.id != added.id));
    assert!(runtime
        .read(|doc| doc.audit.iter().any(|event| {
            event.kind == "client-deleted"
                && event.principal_id.as_deref() == Some(added.id.as_str())
        }))
        .unwrap());
    mcp_card_closed("M1".into()).unwrap();
    assert!(guard_server_restart().unwrap().is_empty());
    assert!(guard_terminal_input("ordinary-session").is_ok());

    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn production_routes_cover_authorized_job_and_control_lifecycle() {
    let root = test_root("routes");
    let runner = FakeRunner::start(&root, "g_a");
    let mut doc = DiskDoc::default();
    doc.config.enabled = true;
    doc.config.clients.push(client_record(&root));
    let mut session = session_record(&root, &runner);
    session.control_owner = None;
    session.control_holder = None;
    session.lease_expires_at = None;
    doc.sessions.push(session);
    let path = root.join("mcp.json");
    save(&path, &doc).unwrap();
    let runtime = Arc::new(Runtime {
        app: None,
        path,
        socket: root.join("control.sock"),
        doc: Mutex::new(Ok(doc)),
        io: Mutex::new(()),
        delivery: Mutex::new(()),
        emergency: Mutex::new(EmergencyFences::default()),
        service_instance: "svc_test".into(),
        runner_auth: Mutex::new(HashMap::new()),
        started: Instant::now(),
    });

    assert_eq!(
        route(
            &runtime,
            WireRequest {
                version: 99,
                client_id: "client_a".into(),
                credential: "mcp_test".into(),
                tool: "deck_capabilities".into(),
                arguments: json!({}),
            }
        )["error"]["code"],
        "PROTOCOL_MISMATCH",
        "an adapter/app version skew is not reported as an auth failure"
    );
    let mut wrong_credential = request("deck_capabilities", json!({}));
    wrong_credential.credential = "mcp_wrong".into();
    assert_eq!(
        route(&runtime, wrong_credential)["error"]["code"],
        "AUTH_REQUIRED"
    );
    assert_eq!(
        route(&runtime, request("unknown", json!({})))["error"]["code"],
        "UNSUPPORTED"
    );
    assert!(route(&runtime, request("deck_capabilities", json!({})))["ok"] == true);
    assert_eq!(
        route(
            &runtime,
            request(
                "deck_project_read",
                json!({"project_id":"P1","path":".env"}),
            ),
        )["error"]["code"],
        "READ_DENIED"
    );
    std::fs::write(root.join("target.txt"), "DECK_SYNTHETIC_ROUTE_MARKER\n").unwrap();
    let file_search = route(
        &runtime,
        request(
            "deck_project_search",
            json!({"project_id":"P1","path":"target.txt","query":"DECK_SYNTHETIC_ROUTE_MARKER"}),
        ),
    );
    assert_eq!(file_search["matches"].as_array().unwrap().len(), 1);
    assert_eq!(file_search["complete"], true);
    assert_eq!(
        route(
            &runtime,
            request(
                "deck_project_search",
                json!({"project_id":"P1","path":"../outside","query":"x"}),
            ),
        )["error"]["code"],
        "INVALID_ARGUMENTS"
    );
    assert_eq!(
        route(&runtime, request("deck_sessions_list", json!({})))["sessions"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        route(
            &runtime,
            request("deck_session_inspect", json!({"session_id":"mcp_a"}))
        )["sessionGeneration"],
        "g_a"
    );

    let control = |request_id: &str, action: &str, epoch: Option<u64>| {
        let mut value = json!({
            "request_id":request_id,
            "session_id":"mcp_a",
            "expected_generation":"g_a",
            "action":action,
            "holder_id":"holder_a"
        });
        if action != "release" {
            value["lease_ms"] = json!(2_000);
        }
        if let Some(epoch) = epoch {
            value["control_epoch"] = json!(epoch);
        }
        value["control_sequence"] = json!(session_state(&runtime).control_sequence);
        route(&runtime, request("deck_session_control", value))
    };
    let operation_count = runtime.read(|doc| doc.operations.len()).unwrap();
    for arguments in [
        json!({"request_id":"bad_lease_low","session_id":"mcp_a","expected_generation":"g_a","action":"request","holder_id":"holder_a","lease_ms":999,"control_sequence":0}),
        json!({"request_id":"bad_request_epoch","session_id":"mcp_a","expected_generation":"g_a","action":"request","holder_id":"holder_a","control_epoch":1,"control_sequence":0}),
        json!({"request_id":"bad_renew_epoch","session_id":"mcp_a","expected_generation":"g_a","action":"renew","holder_id":"holder_a","control_sequence":0}),
        json!({"request_id":"bad_release_lease","session_id":"mcp_a","expected_generation":"g_a","action":"release","holder_id":"holder_a","control_epoch":1,"lease_ms":1000,"control_sequence":0}),
    ] {
        assert_eq!(
            route(&runtime, request("deck_session_control", arguments))["error"]["code"],
            "INVALID_ARGUMENTS"
        );
    }
    assert_eq!(
        runtime.read(|doc| doc.operations.len()).unwrap(),
        operation_count
    );
    assert_eq!(
        control("control_request", "request", None)["state"],
        "committed"
    );
    assert_eq!(
        route(
            &runtime,
            request(
                "deck_session_control",
                json!({"request_id":"holder_conflict","session_id":"mcp_a","expected_generation":"g_a","action":"request","holder_id":"holder_other","control_sequence":session_state(&runtime).control_sequence}),
            ),
        )["error"]["code"],
        "PERMISSION_DENIED"
    );
    assert_eq!(
        control("control_renew", "renew", Some(2))["state"],
        "committed"
    );
    assert_eq!(
        control("control_release", "release", Some(2))["state"],
        "committed"
    );
    let control_again = control("control_again", "request", None);
    assert_eq!(control_again["state"], "committed", "{control_again}");

    let denied = route(
        &runtime,
        request(
            "deck_exec",
            json!({"request_id":"exec_without_grant","session_id":"mcp_a","expected_generation":"g_a","control_epoch":4,"holder_id":"holder_a","executable":"/usr/bin/printf","args":["forbidden"],"cwd":root.display().to_string()}),
        ),
    );
    assert_eq!(denied["error"]["code"], "EXECUTION_GRANT_REQUIRED");
    runtime
        .write(|doc| {
            let session = doc.sessions[0].clone();
            doc.execution_grants.push(execution_grant(&session));
            Ok(())
        })
        .unwrap();
    let grant_expiry = runtime
        .read(|doc| doc.execution_grants.last().unwrap().expires_at)
        .unwrap();
    assert_eq!(
        control("control_after_grant", "renew", Some(4))["state"],
        "committed"
    );
    assert_eq!(
        runtime
            .read(|doc| doc.execution_grants.last().unwrap().expires_at)
            .unwrap(),
        grant_expiry
    );

    let exec_arguments = json!({
        "request_id":"exec_a",
        "session_id":"mcp_a",
        "expected_generation":"g_a",
        "control_epoch":4,
        "holder_id":"holder_a",
        "executable":"/usr/bin/printf",
        "args":["done\\n"],
        "cwd":root.display().to_string(),
        "wait_ms":10,
        "execution_timeout_ms":1_000
    });
    let executed = route(&runtime, request("deck_exec", exec_arguments.clone()));
    assert_eq!(executed["state"], "exited", "{executed}");
    let persisted_launch = runtime
        .read(|doc| {
            doc.operations
                .iter()
                .find(|operation| operation.request_id == "exec_a")
                .and_then(|operation| operation.result.clone())
                .unwrap()
        })
        .unwrap();
    assert_eq!(persisted_launch["launch"]["launchKind"], "direct");
    let persisted_text = persisted_launch.to_string();
    assert!(!persisted_text.contains("/usr/bin/printf"));
    assert!(!persisted_text.contains("done\\n"));
    let job_id = executed["jobId"].as_str().unwrap().to_owned();
    assert_eq!(
        route(&runtime, request("deck_exec", exec_arguments))["jobId"],
        job_id
    );

    let read = route(
        &runtime,
        request(
            "deck_job_read",
            json!({"job_id":job_id,"max_bytes":1024,"wait_ms":0}),
        ),
    );
    assert_eq!(read["exitCode"], 0);
    assert_eq!(read["output"], "done\n");
    assert_eq!(
        route(
            &runtime,
            request(
                "deck_job_input",
                json!({"request_id":"input_a","job_id":job_id,"session_generation":"g_a","control_epoch":4,"holder_id":"holder_a","input":"yes\n"}),
            ),
        )["state"],
        "committed"
    );
    assert_eq!(
        route(
            &runtime,
            request(
                "deck_job_interrupt",
                json!({"request_id":"interrupt_a","job_id":job_id,"session_generation":"g_a","control_epoch":4,"holder_id":"holder_a"}),
            ),
        )["state"],
        "committed"
    );

    let create = route(
        &runtime,
        request(
            "deck_session_create",
            json!({"request_id":"create_a","project_id":"P1","cwd":root.display().to_string(),"title":"Visible shell","create_sequence":0}),
        ),
    );
    assert_eq!(create["state"], "accepted");
    let operation_id = create["operationId"].as_str().unwrap();
    assert_eq!(
        route(
            &runtime,
            request("deck_operation_get", json!({"operation_id":operation_id}),),
        )["kind"],
        "session-create"
    );

    let close = route(
        &runtime,
        request(
            "deck_session_close",
            json!({"request_id":"close_a","session_id":"mcp_a","expected_generation":"g_a","control_epoch":4,"holder_id":"holder_a","confirm_running":false}),
        ),
    );
    assert_eq!(close["state"], "accepted", "{close}");
    let close_id = close["operationId"].as_str().unwrap().to_owned();
    runtime
        .write(|doc| {
            doc.operations
                .iter_mut()
                .find(|operation| operation.operation_id == close_id)
                .unwrap()
                .state = "executing".into();
            let session = doc
                .sessions
                .iter_mut()
                .find(|session| session.session_id == "mcp_a")
                .unwrap();
            session.human_lock = true;
            session.control_owner = None;
            session.control_epoch += 1;
            Ok(())
        })
        .unwrap();
    assert!(close_admit(&runtime, &close_id).is_err());

    runtime
        .write(|doc| {
            let operation = doc
                .operations
                .iter_mut()
                .find(|operation| operation.operation_id == close_id)
                .unwrap();
            operation.state = "executing".into();
            let epoch = operation.result.as_ref().unwrap()["controlEpoch"]
                .as_u64()
                .unwrap();
            let session = doc
                .sessions
                .iter_mut()
                .find(|session| session.session_id == "mcp_a")
                .unwrap();
            session.human_lock = false;
            session.control_owner = Some("client_a".into());
            session.control_epoch = epoch;
            Ok(())
        })
        .unwrap();
    let admission = close_admit(&runtime, &close_id).unwrap().admission;
    runtime
        .write(|doc| {
            doc.config.clients[0].revoked_at = Some(now_ms());
            Ok(())
        })
        .unwrap();
    validate_close_admission_with(&runtime, Some(&admission), &["deck-mcp-test".into()]).unwrap();
    runtime
        .write(|doc| {
            doc.config.clients[0].revoked_at = None;
            Ok(())
        })
        .unwrap();

    assert_eq!(
        route(&runtime, request("deck_exec", json!({})))["error"]["code"],
        "INVALID_ARGUMENTS"
    );
    assert_eq!(
        route(
            &runtime,
            request(
                "deck_job_read",
                json!({"job_id":job_id,"cursor":"wrong:cursor:1"}),
            ),
        )["error"]["code"],
        "OUTPUT_CURSOR_INVALID"
    );
    let audit_kinds = runtime
        .read(|doc| {
            doc.audit
                .iter()
                .map(|event| event.kind.clone())
                .collect::<Vec<_>>()
        })
        .unwrap();
    assert!(audit_kinds.iter().any(|kind| kind == "control-changed"));
    assert!(audit_kinds.iter().any(|kind| kind == "exec-intent"));
    assert!(audit_kinds.iter().any(|kind| kind == "exec-dispatch"));
    assert!(audit_kinds.iter().any(|kind| kind == "request-denied"));
    assert!(audit_kinds.iter().any(|kind| kind == "read-denied"));

    drop(runtime);
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn validation_and_error_mapping_cover_rejected_boundaries() {
    assert!(!valid_direct_launch("git", &[]));
    assert!(!valid_direct_launch("./tool", &[]));
    assert!(valid_direct_launch("/usr/bin/git", &[]));
    assert!(!valid_id(""));
    assert!(!valid_id("bad id"));
    assert!(valid_id("good_ID-1"));
    assert!(!valid_title("\n"));
    assert!(!valid_title(&"x".repeat(121)));
    assert_eq!(sha(b"same"), sha(b"same"));
    assert!(random_id("test_", 16).unwrap().starts_with("test_"));
    assert!(parse::<Empty>(json!({"extra":true})).is_err());
    assert!(runner_error(&json!({"error":"session-busy"})).is_some());
    assert!(runner_error(&json!({"error":"control-revoked"})).is_some());
    assert!(runner_error(&json!({"error":"job-not-found"})).is_some());
    assert!(runner_error(&json!({"error":"job-not-running"})).is_some());
    assert!(runner_error(&json!({"error":"request-id-conflict"})).is_some());
    assert!(runner_error(&json!({"error":"capacity-exceeded"})).is_some());
    assert!(runner_error(&json!({"error":"invalid-cwd"})).is_some());
    assert!(runner_error(&json!({"error":"other"})).is_none());

    for (kind, code) in [
        (ErrorKind::Perm, "PERMISSION_DENIED"),
        (ErrorKind::Missing, "SESSION_NOT_FOUND"),
        (ErrorKind::NotDir, "CONTEXT_CHANGED"),
        (ErrorKind::ContextChanged, "CONTEXT_CHANGED"),
        (ErrorKind::ControlRevoked, "CONTROL_REVOKED"),
        (ErrorKind::RequestConflict, "REQUEST_ID_CONFLICT"),
        (ErrorKind::DiskFull, "CAPACITY_EXCEEDED"),
        (ErrorKind::Locked, "SESSION_BUSY"),
        (ErrorKind::Other, "INTERNAL_ERROR"),
    ] {
        assert_eq!(
            map_error(DeckError::new(kind, "test"))["error"]["code"],
            code
        );
    }

    let root = test_root("validation");
    let missing = root.join("missing");
    assert!(canonical_scope(missing.to_str().unwrap(), &[]).is_err());
    assert!(canonical_scope(root.to_str().unwrap(), &["/elsewhere".into()]).is_err());
    assert_eq!(
        load(&root.join("absent.json")).unwrap().version,
        STATE_VERSION
    );

    let mut invalid = DiskDoc {
        version: STATE_VERSION + 1,
        ..DiskDoc::default()
    };
    assert!(validate_doc(&invalid).is_err());
    invalid.version = STATE_VERSION;
    invalid.config.clients.push(Client {
        id: "bad id".into(),
        name: "Bad".into(),
        credential_hash: sha(b"mcp_test"),
        credential_version: 1,
        revoked_at: None,
        allow_create: false,
        projects: vec![],
        create_sequence: 0,
    });
    assert!(validate_doc(&invalid).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn v2_state_migration_revokes_clients_and_never_restores_execution() {
    let root = test_root("v2-migration");
    let path = root.join("mcp.json");
    let runner = FakeRunner::start(&root, "g_a");
    let mut doc = DiskDoc {
        version: 2,
        ..DiskDoc::default()
    };
    doc.config.enabled = true;
    doc.config.clients.push(client_record(&root));
    let session = session_record(&root, &runner);
    doc.execution_grants.push(execution_grant(&session));
    doc.sessions.push(session);
    doc.operations.push(operation("exec", "accepted"));
    std::fs::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();

    let migrated = load(&path).unwrap();
    assert_eq!(migrated.version, STATE_VERSION);
    assert!(!migrated.config.enabled);
    assert!(migrated.config.clients[0].revoked_at.is_some());
    assert_eq!(migrated.config.clients[0].credential_version, 0);
    assert!(migrated.execution_grants.is_empty());
    assert_eq!(migrated.operations[0].state, "ambiguous");
    assert_eq!(
        migrated.operations[0].code.as_deref(),
        Some("v3-reauthorization-required")
    );
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn encoded_frames_fit_the_advertised_budget() {
    let hostile = "\"\\\n\r\t\u{0001}".repeat(MAX_ARGUMENT_BYTES / 6);
    let request = json!({"kind":"exec","executable":"/usr/bin/printf","args":[hostile],"context":{"intent_hash":"a".repeat(64)}});
    assert!(serde_json::to_vec(&request).unwrap().len() < MAX_REQUEST_BYTES);
    let output = "\"\\\n\r\t\u{0001}".repeat(MAX_READ_BYTES / 6);
    let response = json!({"ok":true,"output":output});
    assert!(serde_json::to_vec(&response).unwrap().len() < MAX_RESPONSE_BYTES);
}

#[test]
fn concurrent_create_reserves_one_operation_and_one_plan() {
    let root = test_root("concurrent-create");
    let mut doc = DiskDoc::default();
    doc.config.enabled = true;
    doc.config.clients.push(client_record(&root));
    let runtime = Arc::new(Runtime {
        app: None,
        path: root.join("mcp.json"),
        socket: root.join("control.sock"),
        doc: Mutex::new(Ok(doc)),
        io: Mutex::new(()),
        delivery: Mutex::new(()),
        emergency: Mutex::new(EmergencyFences::default()),
        service_instance: "svc_concurrent".into(),
        runner_auth: Mutex::new(HashMap::new()),
        started: Instant::now(),
    });
    let barrier = Arc::new(std::sync::Barrier::new(3));
    let mut threads = Vec::new();
    for _ in 0..2 {
        let runtime = runtime.clone();
        let barrier = barrier.clone();
        let cwd = root.display().to_string();
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            session_create(
                &runtime,
                "client_a",
                json!({"request_id":"same_create","project_id":"P1","cwd":cwd,"create_sequence":0}),
            )
            .unwrap()["operationId"]
                .as_str()
                .unwrap()
                .to_owned()
        }));
    }
    barrier.wait();
    let ids = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(ids[0], ids[1]);
    runtime
        .read(|doc| {
            assert_eq!(doc.operations.len(), 1);
            assert_eq!(
                doc.operations
                    .iter()
                    .filter(|operation| operation.kind == "session-create")
                    .count(),
                1
            );
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

/// Runtime + fake runner + one managed session (`mcp_a`, card `M1`).
fn fixture(tag: &str, service: &str) -> (Arc<Runtime>, FakeRunner, PathBuf) {
    let root = test_root(tag);
    let runner = FakeRunner::start(&root, "g_a");
    // This fixture represents an already-established session. Tests that
    // exercise creation use a fresh runner at epoch zero instead.
    runner.set_control(1, "mcp", Some("holder_a"));
    let mut doc = DiskDoc::default();
    doc.config.enabled = true;
    doc.config.clients.push(client_record(&root));
    doc.sessions.push(session_record(&root, &runner));
    let path = root.join("mcp.json");
    save(&path, &doc).unwrap();
    let runtime = Arc::new(Runtime {
        app: None,
        path,
        socket: root.join("control.sock"),
        doc: Mutex::new(Ok(doc)),
        io: Mutex::new(()),
        delivery: Mutex::new(()),
        emergency: Mutex::new(EmergencyFences::default()),
        service_instance: service.into(),
        runner_auth: Mutex::new(HashMap::new()),
        started: Instant::now(),
    });
    (runtime, runner, root)
}

fn session_state(runtime: &Runtime) -> ManagedSession {
    runtime.read(|doc| doc.sessions[0].clone()).unwrap()
}

#[test]
fn authorized_create_control_job_takeover_return_and_release_stay_in_sync() {
    let root = test_root("authorized-create-epoch");
    let runner = FakeRunner::start(&root, "g_a");
    let mut doc = DiskDoc::default();
    doc.config.enabled = true;
    doc.config.clients.push(client_record(&root));
    let path = root.join("mcp.json");
    let runtime = Arc::new(Runtime {
        app: None,
        path,
        socket: root.join("control.sock"),
        doc: Mutex::new(Ok(doc)),
        io: Mutex::new(()),
        delivery: Mutex::new(()),
        emergency: Mutex::new(EmergencyFences::default()),
        service_instance: "svc_test".into(),
        runner_auth: Mutex::new(HashMap::new()),
        started: Instant::now(),
    });
    let created = session_create(
        &runtime,
        "client_a",
        json!({"request_id":"create_epoch","project_id":"P1","cwd":root.display().to_string(),"create_sequence":0}),
    )
    .unwrap();
    let operation_id = created["operationId"].as_str().unwrap().to_owned();
    runtime
        .write(|doc| {
            let operation = doc
                .operations
                .iter_mut()
                .find(|operation| operation.operation_id == operation_id)
                .unwrap();
            operation.state = "executing".into();
            let result = operation.result.as_mut().unwrap();
            result["sessionId"] = json!("mcp_created");
            result["cardId"] = json!("Mcreated");
            result["generation"] = json!("g_a");
            result["runnerSocket"] = json!(runner.socket.display().to_string());
            operation.session_id = Some("mcp_created".into());
            Ok(())
        })
        .unwrap();
    complete(
        &runtime,
        operation_id,
        "committed".into(),
        None,
        Some("deck-created".into()),
    )
    .unwrap();
    assert_eq!(
        runner.control_epoch(),
        0,
        "authorized create sends no control"
    );

    let request_control = |request_id: &str, sequence: u64| {
        route(
            &runtime,
            request(
                "deck_session_control",
                json!({"request_id":request_id,"session_id":"mcp_created","expected_generation":"g_a","action":"request","holder_id":"holder_created","control_sequence":sequence}),
            ),
        )
    };
    let first = request_control("request_first", 0);
    assert_eq!(first["state"], "committed", "{first}");
    assert_eq!(first["result"]["controlEpoch"], 2);
    assert_eq!(runner.control_epoch(), 2);

    super::execution_grant(&runtime, "mcp_created".into(), Some(60_000), true, true).unwrap();
    let exec = route(
        &runtime,
        request(
            "deck_exec",
            json!({"request_id":"exec_created","session_id":"mcp_created","expected_generation":"g_a","control_epoch":2,"holder_id":"holder_created","cwd":root.display().to_string(),"executable":"/usr/bin/true","args":[],"wait_ms":0}),
        ),
    );
    assert_eq!(exec["state"], "exited", "{exec}");

    takeover(&runtime, "mcp_created").unwrap();
    assert_eq!(runner.control_epoch(), 3);
    return_control(&runtime, "mcp_created").unwrap();
    assert_eq!(runner.control_epoch(), 4);

    let second = request_control("request_second", 1);
    assert_eq!(second["state"], "committed", "{second}");
    assert_eq!(second["result"]["controlEpoch"], 5);
    let released = route(
        &runtime,
        request(
            "deck_session_control",
            json!({"request_id":"release_created","session_id":"mcp_created","expected_generation":"g_a","action":"release","holder_id":"holder_created","control_epoch":5,"control_sequence":2}),
        ),
    );
    assert_eq!(released["state"], "committed", "{released}");
    assert_eq!(runner.control_epoch(), 6);
    drop(runtime);
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn lapsed_lease_epoch_gap_is_resynchronized_by_next_request() {
    let (runtime, runner, root) = fixture("lapsed-epoch-gap", "svc_test");
    runtime
        .write(|doc| {
            doc.sessions[0].lease_expires_at = Some(now_ms().saturating_sub(1));
            close_lapsed_leases(doc);
            Ok(())
        })
        .unwrap();
    assert_eq!(session_state(&runtime).control_epoch, 2);
    assert_eq!(runner.control_epoch(), 1);
    let requested = route(
        &runtime,
        request(
            "deck_session_control",
            json!({"request_id":"request_after_lapse","session_id":"mcp_a","expected_generation":"g_a","action":"request","holder_id":"holder_a","control_sequence":0}),
        ),
    );
    assert_eq!(requested["state"], "committed", "{requested}");
    assert_eq!(requested["result"]["controlEpoch"], 3);
    assert_eq!(runner.control_epoch(), 3);
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_runner_control_does_not_poison_the_next_takeover() {
    let (runtime, runner, root) = fixture("failed-control-gap", "svc_test");
    runner.fail_next_control();
    assert_eq!(
        takeover(&runtime, "mcp_a").unwrap_err().message(),
        RUNNER_UNCONFIRMED
    );
    assert_eq!(session_state(&runtime).control_epoch, 2);
    assert_eq!(runner.control_epoch(), 1);

    takeover(&runtime, "mcp_a").unwrap();
    assert_eq!(session_state(&runtime).control_epoch, 3);
    assert_eq!(runner.control_epoch(), 3);
    assert_eq!(runner.last("control").unwrap()["mode"], "human");
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn execution_grant_allows_any_program_including_shells() {
    let (runtime, runner, root) = fixture("shell-boundary", "svc_test");
    runtime
        .write(|doc| {
            doc.execution_grants.push(execution_grant(&doc.sessions[0]));
            Ok(())
        })
        .unwrap();
    let direct = json!({"request_id":"direct_shell","session_id":"mcp_a","expected_generation":"g_a","control_epoch":1,"holder_id":"holder_a","cwd":root.display().to_string(),"executable":"/bin/zsh","args":["-c","true"]});
    assert_eq!(route(&runtime, request("deck_exec", direct))["ok"], true);
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

fn expired_grant(session: &ManagedSession) -> ExecutionGrant {
    let mut grant = execution_grant(session);
    grant.issued_at = now_ms().saturating_sub(120_000);
    grant.expires_at = now_ms().saturating_sub(60_000);
    grant
}

#[test]
fn return_after_takeover_needs_no_grant_and_restores_nothing() {
    // The acceptance failure: epoch 2 → takeover → grant already expired →
    // return was refused with "a local execution grant is required".
    let (runtime, runner, root) = fixture("return-expired", "svc_test");
    runtime
        .write(|doc| {
            let grant = expired_grant(&doc.sessions[0]);
            doc.execution_grants.push(grant);
            doc.sessions[0].control_epoch = 2;
            Ok(())
        })
        .unwrap();
    takeover(&runtime, "M1").unwrap();
    let taken = session_state(&runtime);
    assert!(taken.human_lock);
    assert_eq!(taken.control_epoch, 3);
    assert!(taken.control_owner.is_none() && taken.control_holder.is_none());
    assert!(taken.lease_expires_at.is_none());
    assert!(!taken.output_shared);
    assert_eq!(runner.last("control").unwrap()["mode"], "human");
    let grants_before = runtime.read(|doc| doc.execution_grants.len()).unwrap();

    return_control(&runtime, "M1").unwrap();
    let returned = session_state(&runtime);
    assert!(!returned.human_lock);
    assert_eq!(returned.control_epoch, 4, "a new epoch, never the old one");
    assert!(returned.control_owner.is_none() && returned.control_holder.is_none());
    assert!(returned.lease_expires_at.is_none());
    assert!(!returned.output_shared, "sharing is not restored");
    let control = runner.last("control").unwrap();
    assert_eq!(control["mode"], "mcp");
    assert_eq!(control["control_epoch"], 4);
    assert!(control["holder_id"].is_null());
    runtime
        .read(|doc| {
            assert_eq!(
                doc.execution_grants.len(),
                grants_before,
                "no grant created"
            );
            assert!(matches!(
                execution_authorization(&runtime, doc, "client_a", &doc.sessions[0]),
                ExecutionAuthorization::Expired(_)
            ));
        })
        .unwrap();
    assert!(!runtime
        .emergency
        .lock_or_recover()
        .human_sessions
        .contains("mcp_a"));
    // The pre-takeover holder's epoch is dead.
    assert!(check_control(&returned, "client_a", "g_a", 2, "holder_a").is_err());
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn return_is_refused_with_stable_codes_and_changes_nothing() {
    let (runtime, runner, root) = fixture("return-refused", "svc_test");
    takeover(&runtime, "mcp_a").unwrap();
    let fenced = session_state(&runtime);

    runner.busy.store(true, Ordering::SeqCst);
    let busy = return_control(&runtime, "M1").unwrap_err();
    assert_eq!(busy.message(), SESSION_BUSY);
    assert_eq!(session_state(&runtime).control_epoch, fenced.control_epoch);
    assert!(session_state(&runtime).human_lock);
    runner.busy.store(false, Ordering::SeqCst);

    runtime
        .write(|doc| {
            doc.config.clients[0].revoked_at = Some(now_ms());
            Ok(())
        })
        .unwrap();
    assert_eq!(
        return_control(&runtime, "M1").unwrap_err().message(),
        CLIENT_REVOKED
    );
    runtime
        .write(|doc| {
            doc.config.clients[0].revoked_at = None;
            doc.config.enabled = false;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        return_control(&runtime, "M1").unwrap_err().message(),
        FEATURE_DISABLED
    );
    let after = session_state(&runtime);
    assert!(after.human_lock);
    assert_eq!(after.control_epoch, fenced.control_epoch);
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_runner_from_before_a_restart_is_stale_but_still_fenced_and_stoppable() {
    let (runtime, runner, root) = fixture("stale", "svc_stale");
    let error = takeover(&runtime, "M1").unwrap_err();
    assert_eq!(error.message(), RUNNER_STALE);
    assert!(session_state(&runtime).human_lock, "the fence is durable");
    assert_eq!(
        return_control(&runtime, "M1").unwrap_err().message(),
        RUNNER_STALE
    );
    let inspect = route(
        &runtime,
        request("deck_session_inspect", json!({"session_id":"mcp_a"})),
    );
    assert_eq!(inspect["stale"], true);
    assert_eq!(inspect["mayStartNextJobReason"], "RUNNER_STALE");
    // `stop` is bound to the generation only: a stale runner still stops.
    assert_eq!(runner.count("stop"), 0);
    let session = session_state(&runtime);
    send_runner(
        &runtime,
        &session,
        &json!({"kind":"stop","generation":session.generation}),
    )
    .unwrap();
    assert_eq!(runner.count("stop"), 1);
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn takeover_fences_before_waiting_for_an_in_flight_dispatch() {
    let (runtime, runner, root) = fixture("fence-first", "svc_test");
    // An exec holds the delivery lock across runner I/O.
    let delivery = runtime.delivery.lock_or_recover();
    let local = runtime.clone();
    let takeover_thread = std::thread::spawn(move || takeover(&local, "M1"));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !runtime
        .emergency
        .lock_or_recover()
        .human_sessions
        .contains("mcp_a")
    {
        assert!(Instant::now() < deadline, "fence waited for the lock");
        std::thread::yield_now();
    }
    // While the lock is still held, every new remote request is refused.
    let exec = route(
        &runtime,
        request(
            "deck_exec",
            json!({"request_id":"late","session_id":"mcp_a","expected_generation":"g_a","control_epoch":1,"holder_id":"holder_a","executable":"/usr/bin/true","args":[]}),
        ),
    );
    assert_eq!(exec["error"]["code"], "HUMAN_CONTROL");
    assert!(emergency_denial(&runtime, "client_a", "mcp_a", Admission::Control).is_some());
    drop(delivery);
    takeover_thread.join().unwrap().unwrap();
    assert!(session_state(&runtime).human_lock);
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn job_read_drops_output_when_a_takeover_lands_during_the_wait() {
    let (runtime, runner, root) = fixture("read-recheck", "svc_test");
    runtime
        .write(|doc| {
            let grant = execution_grant(&doc.sessions[0]);
            doc.jobs.push(JobBinding {
                job_id: "job_a".into(),
                client_id: "client_a".into(),
                session_id: "mcp_a".into(),
                session_generation: "g_a".into(),
                request_hash: "a".repeat(64),
                operation_id: "op_exec".into(),
                grant_id: grant.grant_id.clone(),
                grant_version: 1,
                allow_output: true,
            });
            doc.execution_grants.push(grant);
            Ok(())
        })
        .unwrap();
    let read = || {
        route(
            &runtime,
            request(
                "deck_job_read",
                json!({"job_id":"job_a","max_bytes":1024,"wait_ms":5000}),
            ),
        )
    };
    assert_eq!(read()["output"], "done\n", "baseline read is allowed");

    let (entered, release) = runner.hold("read");
    let reader = {
        let runtime = runtime.clone();
        std::thread::spawn(move || {
            route(
                &runtime,
                request(
                    "deck_job_read",
                    json!({"job_id":"job_a","max_bytes":1024,"wait_ms":5000}),
                ),
            )
        })
    };
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    // Takeover while the runner read is in flight. The runner control
    // call is served on its own connection, so this completes.
    takeover(&runtime, "M1").unwrap();
    release.send(()).unwrap();
    let late = reader.join().unwrap();
    assert_eq!(late["error"]["code"], "HUMAN_CONTROL", "{late}");
    assert!(late.get("output").is_none(), "no bytes after takeover");

    // Return + a fresh grant does not reopen the pre-takeover job.
    return_control(&runtime, "M1").unwrap();
    runtime
        .write(|doc| {
            doc.sessions[0].output_shared = true;
            Ok(())
        })
        .unwrap();
    let reopened = read();
    assert_eq!(
        reopened["error"]["code"], "JOB_OUTPUT_BINDING_CLOSED",
        "{reopened}"
    );
    let inspect = route(
        &runtime,
        request("deck_session_inspect", json!({"session_id":"mcp_a"})),
    );
    assert_eq!(inspect["outputSharing"]["sessionGateOpen"], true);
    assert_eq!(inspect["outputSharing"]["jobBindingsOpen"], 0);
    assert_eq!(inspect["outputSharing"]["jobBindingsClosed"], 1);
    assert!(inspect.get("terminalContext").is_none());
    assert!(inspect.get("readiness").is_none());
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

fn close_plan(runtime: &Runtime) -> String {
    let close = route(
        runtime,
        request(
            "deck_session_close",
            json!({"request_id":format!("close_{}", now_ms()),"session_id":"mcp_a","expected_generation":"g_a","control_epoch":1,"holder_id":"holder_a"}),
        ),
    );
    assert_eq!(close["state"], "accepted", "{close}");
    close["operationId"].as_str().unwrap().to_owned()
}

#[test]
fn a_remote_close_commits_after_admission_and_never_sticks() {
    let (runtime, runner, root) = fixture("close-a", "svc_test");
    let operation_id = close_plan(&runtime);
    assert!(session_state(&runtime).closing);
    claim(&runtime, &operation_id).unwrap();
    let admission = close_admit(&runtime, &operation_id).unwrap().admission;
    validate_close_admission_with(&runtime, Some(&admission), &["deck-mcp-test".into()]).unwrap();
    // After admission a side effect may have run: rejected is not allowed.
    assert!(complete(
        &runtime,
        operation_id.clone(),
        "rejected".into(),
        None,
        None
    )
    .is_err());
    complete(
        &runtime,
        operation_id.clone(),
        "committed".into(),
        None,
        None,
    )
    .unwrap();
    runtime
        .read(|doc| assert!(doc.sessions.is_empty(), "committed close removes it"))
        .unwrap();
    // A repeated completion is idempotent; a different one is refused.
    complete(
        &runtime,
        operation_id.clone(),
        "committed".into(),
        None,
        None,
    )
    .unwrap();
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();

    // Failure after admission: ambiguous, and `closing` is cleared.
    let (runtime, runner, root) = fixture("close-b", "svc_test");
    let operation_id = close_plan(&runtime);
    claim(&runtime, &operation_id).unwrap();
    close_admit(&runtime, &operation_id).unwrap();
    complete(
        &runtime,
        operation_id.clone(),
        "ambiguous".into(),
        Some("close-failed".into()),
        None,
    )
    .unwrap();
    assert!(!session_state(&runtime).closing, "closing never sticks");
    let view = route(
        &runtime,
        request("deck_operation_get", json!({"operation_id":operation_id})),
    );
    assert_eq!(view["state"], "ambiguous");
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn restart_turns_pending_board_operations_ambiguous() {
    let root = test_root("restart-board");
    let path = root.join("mcp.json");
    let runner = FakeRunner::start(&root, "g_a");
    let mut doc = DiskDoc::default();
    doc.config.enabled = true;
    doc.config.clients.push(client_record(&root));
    let mut session = session_record(&root, &runner);
    session.closing = true;
    doc.sessions.push(session);
    for (id, kind, state) in [
        ("op_create", "session-create", "accepted"),
        ("op_close", "session-close", "admitted"),
        ("op_exec", "session-create", "executing"),
    ] {
        let mut item = operation(kind, state);
        item.operation_id = id.into();
        item.request_id = format!("request_{id}");
        item.result = Some(json!({"sessionId":"mcp_a"}));
        doc.operations.push(item);
    }
    save(&path, &doc).unwrap();
    let recovered = load(&path).unwrap();
    for item in &recovered.operations {
        assert_eq!(item.state, "ambiguous", "{}", item.operation_id);
        assert_eq!(item.code.as_deref(), Some("deck-restarted"));
    }
    assert!(!recovered.sessions[0].closing);
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_control_replay_never_resends_a_runner_side_effect() {
    let (runtime, runner, root) = fixture("control-replay", "svc_test");
    runtime
        .write(|doc| {
            let session = &mut doc.sessions[0];
            session.control_owner = None;
            session.control_holder = None;
            session.lease_expires_at = None;
            Ok(())
        })
        .unwrap();
    let control = |arguments: Value| route(&runtime, request("deck_session_control", arguments));
    let first = control(
        json!({"request_id":"req_a","session_id":"mcp_a","expected_generation":"g_a","action":"request","holder_id":"holder_a","control_sequence":0}),
    );
    assert_eq!(first["state"], "committed", "{first}");
    let epoch = first["result"]["controlEpoch"].as_u64().unwrap();
    let release = json!({"request_id":"rel_a","session_id":"mcp_a","expected_generation":"g_a","action":"release","holder_id":"holder_a","control_epoch":epoch,"control_sequence":1});
    assert_eq!(control(release.clone())["state"], "committed");
    let taken = control(
        json!({"request_id":"req_b","session_id":"mcp_a","expected_generation":"g_a","action":"request","holder_id":"holder_b","control_sequence":2}),
    );
    assert_eq!(taken["state"], "committed");
    let controls = runner.count("control");
    // Replaying A's old release must not fence B's runner context.
    // A's release was superseded by B's request: it is refused as stale
    // (its sequence is no longer current), never applied again.
    let replay = control(release);
    assert_eq!(replay["error"]["code"], "STALE_REQUEST", "{replay}");
    assert_eq!(
        runner.count("control"),
        controls,
        "replay sent a runner control"
    );
    assert_eq!(
        session_state(&runtime).control_holder.as_deref(),
        Some("holder_b")
    );
    // null and omitted are the same request, not a conflict.
    let with_null = control(
        json!({"request_id":"req_b","session_id":"mcp_a","expected_generation":"g_a","action":"request","holder_id":"holder_b","lease_ms":null,"control_epoch":null,"control_sequence":2}),
    );
    assert_eq!(
        with_null["operationId"], taken["operationId"],
        "{with_null}"
    );
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

/// Grant a fresh window directly (the local UI path) for route tests.
fn grant_window(runtime: &Runtime) {
    super::execution_grant(runtime, "mcp_a".into(), Some(60_000), true, true).unwrap();
}

fn control_request(runtime: &Runtime, request_id: &str) -> u64 {
    let value = route(
        runtime,
        request(
            "deck_session_control",
            json!({"request_id":request_id,"session_id":"mcp_a","expected_generation":"g_a","action":"request","holder_id":"holder_a","control_sequence":session_state(runtime).control_sequence}),
        ),
    );
    assert_eq!(value["state"], "committed", "{value}");
    value["result"]["controlEpoch"].as_u64().unwrap()
}

fn release(runtime: &Runtime, request_id: &str, epoch: u64) {
    let value = route(
        runtime,
        request(
            "deck_session_control",
            json!({"request_id":request_id,"session_id":"mcp_a","expected_generation":"g_a","action":"release","holder_id":"holder_a","control_epoch":epoch,"control_sequence":session_state(runtime).control_sequence}),
        ),
    );
    assert_eq!(value["state"], "committed", "{value}");
}

fn exec_request(request_id: &str, epoch: u64) -> WireRequest {
    request(
        "deck_exec",
        json!({"request_id":request_id,"session_id":"mcp_a","expected_generation":"g_a","control_epoch":epoch,"holder_id":"holder_a","executable":"/usr/bin/true","args":[],"wait_ms":0}),
    )
}

fn unowned(runtime: &Runtime) {
    runtime
        .write(|doc| {
            let session = &mut doc.sessions[0];
            session.control_owner = None;
            session.control_holder = None;
            session.lease_expires_at = None;
            Ok(())
        })
        .unwrap();
}

#[test]
fn compaction_keeps_three_thousand_epochs_bounded() {
    // Pure ledger model of 3000 request/exec/release cycles: each cycle
    // journals three records bound to its epoch.
    let root = test_root("compact-model");
    let runner = FakeRunner::start(&root, "g_a");
    let mut doc = DiskDoc::default();
    doc.sessions.push(session_record(&root, &runner));
    for cycle in 0..3_000u64 {
        let epoch = cycle * 2 + 1;
        doc.sessions[0].control_epoch = epoch;
        for kind in ["session-control", "exec", "session-control"] {
            let mut record = operation(kind, "committed");
            record.operation_id = format!("op_{}", doc.operations.len() + cycle as usize * 3);
            record.request_id = format!("{kind}_{cycle}_{}", doc.operations.len());
            record.session_id = Some("mcp_a".into());
            record.control_epoch = Some(epoch);
            doc.operations.push(record);
        }
        doc.sessions[0].control_epoch = epoch + 1;
        compact(&mut doc, Some("svc_test"));
        assert!(
            doc.operations.len() <= 3,
            "cycle {cycle}: {}",
            doc.operations.len()
        );
    }
    validate_doc(&doc).unwrap();
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn sustained_use_stays_within_the_journal() {
    // End-to-end through the routes. 700 cycles journal 2100 records —
    // past the pre-fix global limit of 2000 (it failed at cycle 667).
    // Each debug-build write serializes the audit-capped document, so the
    // 3000-cycle bound is covered by the pure model test above.
    let (runtime, runner, root) = fixture("sustained", "svc_test");
    unowned(&runtime);
    super::execution_grant(
        &runtime,
        "mcp_a".into(),
        Some(MAX_EXECUTION_GRANT_MS),
        true,
        true,
    )
    .unwrap();
    for cycle in 0..700 {
        let epoch = control_request(&runtime, &format!("req_{cycle}"));
        let executed = route(&runtime, exec_request(&format!("exec_{cycle}"), epoch));
        assert_eq!(executed["state"], "exited", "cycle {cycle}: {executed}");
        release(&runtime, &format!("rel_{cycle}"), epoch);
    }
    runtime
        .read(|doc| {
            assert!(
                doc.operations.len() < 16,
                "journal grew: {}",
                doc.operations.len()
            );
            assert!(doc.jobs.len() <= MAX_JOBS_PER_SESSION);
            assert!(doc.execution_grants.len() <= 2);
        })
        .unwrap();
    assert_eq!(runner.count("exec"), 700);
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn many_grants_never_block_a_takeover() {
    let (runtime, runner, root) = fixture("grants", "svc_test");
    for _ in 0..2_001 {
        grant_window(&runtime);
    }
    assert!(runtime.read(|doc| doc.execution_grants.len()).unwrap() <= 2);
    takeover(&runtime, "M1").unwrap();
    assert!(session_state(&runtime).human_lock, "takeover persisted");
    assert_eq!(runner.last("control").unwrap()["mode"], "human");
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_retired_request_id_is_rejected_never_reexecuted() {
    let (runtime, runner, root) = fixture("retired", "svc_test");
    unowned(&runtime);
    grant_window(&runtime);
    let epoch = control_request(&runtime, "req_old");
    let first = route(&runtime, exec_request("exec_once", epoch));
    assert_eq!(first["state"], "exited");
    // Exact replay inside the window: same job, no second dispatch.
    let again = route(&runtime, exec_request("exec_once", epoch));
    assert_eq!(again["jobId"], first["jobId"]);
    assert_eq!(runner.count("exec"), 1);
    release(&runtime, "rel_old", epoch);
    let next = control_request(&runtime, "req_new");
    // Force retirement, then replay the old request id verbatim.
    runtime
        .write(|doc| {
            compact(doc, Some("svc_test"));
            assert!(doc
                .operations
                .iter()
                .all(|operation| operation.request_id != "exec_once"));
            Ok(())
        })
        .unwrap();
    let replay = route(&runtime, exec_request("exec_once", epoch));
    assert_eq!(replay["error"]["code"], "CONTROL_REVOKED", "{replay}");
    assert_eq!(runner.count("exec"), 1, "a retired request was re-executed");
    assert!(next > epoch);
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_full_journal_still_admits_an_interrupt_and_names_a_real_recovery() {
    let (runtime, runner, root) = fixture("reserve", "svc_test");
    grant_window(&runtime);
    let executed = route(&runtime, exec_request("exec_live", 1));
    let job_id = executed["jobId"].as_str().unwrap().to_owned();
    runtime
        .write(|doc| {
            while doc.operations.len() < ORDINARY_OPERATIONS {
                let mut filler = operation("exec", "committed");
                filler.operation_id = format!("op_fill_{}", doc.operations.len());
                filler.request_id = format!("fill_{}", doc.operations.len());
                filler.client_id = format!("client_fill_{}", doc.operations.len() % 8);
                filler.session_id = Some("mcp_a".into());
                filler.control_epoch = Some(1);
                doc.operations.push(filler);
            }
            Ok(())
        })
        .unwrap();
    let blocked = route(&runtime, exec_request("exec_full", 1));
    assert_eq!(blocked["error"]["code"], "CAPACITY_EXCEEDED");
    assert!(blocked["error"]["nextAction"]
        .as_str()
        .unwrap()
        .contains("Release control"));
    let interrupt = route(
        &runtime,
        request(
            "deck_job_interrupt",
            json!({"request_id":"stop_now","job_id":job_id,"session_generation":"g_a","control_epoch":1,"holder_id":"holder_a"}),
        ),
    );
    assert_eq!(interrupt["state"], "committed", "{interrupt}");
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn v3_state_upgrades_stickily_and_future_state_is_refused() {
    let root = test_root("v4");
    let path = root.join("mcp.json");
    let mut doc = DiskDoc {
        version: 3,
        ..DiskDoc::default()
    };
    doc.config.clients.push(client_record(&root));
    let mut old = operation("session-control", "committed");
    old.result = Some(json!({"sessionId":"mcp_gone"}));
    doc.operations.push(old);
    // A v3 file never contains the v4 fields.
    let mut bytes = serde_json::to_value(&doc).unwrap();
    for operation in bytes["operations"].as_array_mut().unwrap() {
        let fields = operation.as_object_mut().unwrap();
        fields.remove("sessionId");
        fields.remove("controlEpoch");
    }
    std::fs::write(&path, serde_json::to_vec(&bytes).unwrap()).unwrap();
    let upgraded = load(&path).unwrap();
    assert_eq!(upgraded.version, STATE_VERSION);
    assert!(
        upgraded.operations.is_empty(),
        "a v3 record of a closed session is retired"
    );
    let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        saved["version"], STATE_VERSION,
        "the upgrade is written back (sticky)"
    );
    // The v3 build accepted exactly version 3; the same rule refuses a
    // future schema here.
    let mut future = saved.clone();
    future["version"] = json!(STATE_VERSION + 1);
    std::fs::write(&path, serde_json::to_vec(&future).unwrap()).unwrap();
    assert_eq!(load(&path).err().unwrap().kind(), ErrorKind::Recovery);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        serde_json::to_vec(&future).unwrap(),
        "refused state is left untouched"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_allow_shell_is_ignored_and_not_reserialized() {
    let root = test_root("v5-shell-grant");
    let runner = FakeRunner::start(&root, "g_a");
    let session = session_record(&root, &runner);
    let mut value = serde_json::to_value(execution_grant(&session)).unwrap();
    value["allowShell"] = json!(true);
    let migrated: ExecutionGrant = serde_json::from_value(value).unwrap();
    assert_eq!(migrated._legacy_allow_shell, Some(true));
    assert!(serde_json::to_value(migrated)
        .unwrap()
        .get("allowShell")
        .is_none());
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn protocol_views_are_honest_about_state_and_identity() {
    let (runtime, runner, root) = fixture("views", "svc_test");
    let capabilities = route(&runtime, request("deck_capabilities", json!({})));
    assert_eq!(capabilities["deckVersion"], DECK_VERSION);
    assert_eq!(capabilities["stateSchemaVersion"], STATE_VERSION);
    assert_eq!(capabilities["executionMode"], "structured-direct-default");
    assert_eq!(capabilities["directExecution"]["arbitraryPrograms"], true);
    assert!(capabilities.get("shellFallback").is_none());
    assert!(capabilities.get("featureEnabled").is_none());
    let create = route(
        &runtime,
        request(
            "deck_session_create",
            json!({"request_id":"create_view","project_id":"P1","cwd":root.display().to_string(),"create_sequence":0}),
        ),
    );
    assert!(create.get("shellJobComplete").is_none());
    assert!(create["result"].get("runnerSocket").is_none());
    assert!(create["result"].get("sessionId").is_some());
    let listed = route(&runtime, request("deck_sessions_list", json!({})));
    assert!(listed["sessions"][0].get("readiness").is_none());
    runtime
        .write(|doc| {
            doc.config.enabled = false;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        route(&runtime, request("deck_capabilities", json!({})))["error"]["code"],
        "FEATURE_DISABLED"
    );
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

// ---- F1: execution revocation reaches the final admission ----

fn wait_until(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready() {
        assert!(Instant::now() < deadline, "condition never became true");
        std::thread::yield_now();
    }
}

fn execution_fenced(runtime: &Runtime, session_id: &str) -> bool {
    runtime
        .emergency
        .lock_or_recover()
        .execution_fenced(session_id)
}

fn operation_by_request(runtime: &Runtime, request_id: &str) -> Option<Operation> {
    runtime
        .read(|doc| {
            doc.operations
                .iter()
                .find(|operation| operation.request_id == request_id)
                .cloned()
        })
        .unwrap()
}

fn input_request(request_id: &str, job_id: &str) -> WireRequest {
    request(
        "deck_job_input",
        json!({"request_id":request_id,"job_id":job_id,"session_generation":"g_a","control_epoch":1,"holder_id":"holder_a","input":"y\n"}),
    )
}

fn interrupt_request(request_id: &str, job_id: &str) -> WireRequest {
    request(
        "deck_job_interrupt",
        json!({"request_id":request_id,"job_id":job_id,"session_generation":"g_a","control_epoch":1,"holder_id":"holder_a"}),
    )
}

#[test]
fn f1_exec_past_route_is_stopped_by_a_pending_execution_revoke() {
    let (runtime, runner, root) = fixture("f1-exec", "svc_test");
    grant_window(&runtime);
    // Baseline: the same production path dispatches when nothing revokes.
    let baseline = route(&runtime, exec_request("exec_base", 1));
    assert_eq!(baseline["state"], "exited", "{baseline}");
    assert_eq!(runner.count("exec"), 1);
    // The request passes route, persists its intent and parks right
    // before its final admission.
    let (entered, release) = pause::arm(&runtime.path, "exec-final");
    let racing = runtime.clone();
    let exec_thread = std::thread::spawn(move || route(&racing, exec_request("exec_raced", 1)));
    entered.recv_timeout(Duration::from_secs(10)).unwrap();
    // The local user revokes: the fence is set, persistence waits for the
    // delivery lock the parked request holds.
    let revoking = runtime.clone();
    let revoke_thread = std::thread::spawn(move || execution_revoke(&revoking, "M1"));
    wait_until(|| execution_fenced(&runtime, "mcp_a"));
    release.send(()).unwrap();
    let raced = exec_thread.join().unwrap();
    revoke_thread.join().unwrap().unwrap();
    assert_eq!(
        runner.count("exec"),
        1,
        "an exec admitted after the revocation fence reached the runner: {raced}"
    );
    assert_eq!(
        raced["error"]["code"], "EXECUTION_GRANT_REQUIRED",
        "{raced}"
    );
    let record = operation_by_request(&runtime, "exec_raced").unwrap();
    assert_eq!(record.state, "rejected");
    let job_id = record.result.unwrap()["jobId"].as_str().unwrap().to_owned();
    runtime
        .read(|doc| assert!(doc.jobs.iter().all(|job| job.job_id != job_id)))
        .unwrap();
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn f1_stdin_past_route_delivers_no_bytes_after_a_pending_execution_revoke() {
    let (runtime, runner, root) = fixture("f1-input", "svc_test");
    grant_window(&runtime);
    let executed = route(&runtime, exec_request("exec_job", 1));
    let job_id = executed["jobId"].as_str().unwrap().to_owned();
    let baseline = route(&runtime, input_request("input_base", &job_id));
    assert_eq!(baseline["state"], "committed", "{baseline}");
    assert_eq!(runner.count("input"), 1);
    let (entered, release) = pause::arm(&runtime.path, "side-effect-final");
    let racing = runtime.clone();
    let racing_job = job_id.clone();
    let input_thread =
        std::thread::spawn(move || route(&racing, input_request("input_raced", &racing_job)));
    entered.recv_timeout(Duration::from_secs(10)).unwrap();
    let revoking = runtime.clone();
    let revoke_thread = std::thread::spawn(move || execution_revoke(&revoking, "M1"));
    wait_until(|| execution_fenced(&runtime, "mcp_a"));
    release.send(()).unwrap();
    let raced = input_thread.join().unwrap();
    revoke_thread.join().unwrap().unwrap();
    assert_eq!(
        runner.count("input"),
        1,
        "stdin bytes reached the runner after the revocation fence: {raced}"
    );
    assert_eq!(
        raced["error"]["code"], "EXECUTION_GRANT_REQUIRED",
        "{raced}"
    );
    assert_eq!(
        operation_by_request(&runtime, "input_raced").unwrap().state,
        "rejected"
    );
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn f1_close_admission_rechecks_takeover_revocation_and_disable() {
    for fence in ["takeover", "client-revoke", "disable"] {
        let (runtime, runner, root) = fixture(&format!("f1-close-{fence}"), "svc_test");
        let operation_id = close_plan(&runtime);
        claim(&runtime, &operation_id).unwrap();
        let (entered, release) = pause::arm(&runtime.path, "close-admit");
        let admitting = runtime.clone();
        let admitted_id = operation_id.clone();
        let admit_thread =
            std::thread::spawn(move || close_admit(&admitting, &admitted_id).map(|_| ()));
        entered.recv_timeout(Duration::from_secs(10)).unwrap();
        let fencing = runtime.clone();
        let fence_thread = std::thread::spawn(move || match fence {
            "takeover" => takeover(&fencing, "M1"),
            "client-revoke" => client_revoke(&fencing, "client_a".into()),
            _ => disable(&fencing),
        });
        wait_until(|| {
            let emergency = runtime.emergency.lock_or_recover();
            emergency.disabled
                || emergency.clients.contains("client_a")
                || emergency.human_sessions.contains("mcp_a")
        });
        release.send(()).unwrap();
        let admitted = admit_thread.join().unwrap();
        fence_thread.join().unwrap().unwrap();
        assert!(admitted.is_err(), "{fence}: close admitted across a fence");
        let record = runtime
            .read(|doc| {
                doc.operations
                    .iter()
                    .find(|operation| operation.operation_id == operation_id)
                    .cloned()
            })
            .unwrap()
            .unwrap();
        assert!(
            !matches!(record.state.as_str(), "admitted" | "committed"),
            "{fence}: {}",
            record.state
        );
        // The Board reports the refusal; the card and session remain.
        complete(
            &runtime,
            operation_id.clone(),
            "rejected".into(),
            None,
            None,
        )
        .unwrap();
        let session = session_state(&runtime);
        assert_eq!(session.session_id, "mcp_a", "{fence}: session was removed");
        assert!(!session.closing, "{fence}: closing stuck");
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn f1_revocation_after_the_commit_point_is_ordered_after_the_dispatch() {
    let (runtime, runner, root) = fixture("f1-inflight", "svc_test");
    grant_window(&runtime);
    // The exec is at the runner: it crossed the admission boundary.
    let (entered, release) = runner.hold("exec");
    let racing = runtime.clone();
    let exec_thread = std::thread::spawn(move || route(&racing, exec_request("exec_inflight", 1)));
    entered.recv_timeout(Duration::from_secs(10)).unwrap();
    let revoking = runtime.clone();
    let revoke_thread = std::thread::spawn(move || execution_revoke(&revoking, "M1"));
    wait_until(|| execution_fenced(&runtime, "mcp_a"));
    release.send(()).unwrap();
    let inflight = exec_thread.join().unwrap();
    revoke_thread.join().unwrap().unwrap();
    assert_eq!(inflight["state"], "exited", "{inflight}");
    assert_eq!(runner.count("exec"), 1);
    assert_eq!(
        operation_by_request(&runtime, "exec_inflight")
            .unwrap()
            .state,
        "committed"
    );
    // The revocation is linearized after the dispatch it waited for.
    runtime
        .read(|doc| {
            let position = |kind: &str| {
                doc.audit
                    .iter()
                    .position(|event| event.kind == kind)
                    .unwrap()
            };
            assert!(position("exec-dispatch") < position("grant-revoked"));
        })
        .unwrap();
    let later = route(&runtime, exec_request("exec_after", 1));
    assert_eq!(
        later["error"]["code"], "EXECUTION_GRANT_REQUIRED",
        "{later}"
    );
    assert_eq!(runner.count("exec"), 1);
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn f1_a_pending_execution_revoke_leaves_reads_and_interrupt_alone() {
    let (runtime, runner, root) = fixture("f1-read", "svc_test");
    grant_window(&runtime);
    let executed = route(&runtime, exec_request("exec_job", 1));
    let job_id = executed["jobId"].as_str().unwrap().to_owned();
    let (entered, release) = pause::arm(&runtime.path, "revoke-fenced");
    let revoking = runtime.clone();
    let revoke_thread = std::thread::spawn(move || execution_revoke(&revoking, "M1"));
    entered.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(execution_fenced(&runtime, "mcp_a"));
    let read = route(
        &runtime,
        request("deck_job_read", json!({"job_id":job_id,"max_bytes":64})),
    );
    assert_eq!(read["ok"], true, "{read}");
    let interrupt = route(&runtime, interrupt_request("stop_job", &job_id));
    assert_eq!(interrupt["state"], "committed", "{interrupt}");
    assert_eq!(runner.count("interrupt"), 1);
    let exec = route(&runtime, exec_request("exec_blocked", 1));
    assert_eq!(exec["error"]["code"], "EXECUTION_GRANT_REQUIRED", "{exec}");
    release.send(()).unwrap();
    revoke_thread.join().unwrap().unwrap();
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn f1_an_approval_never_clears_a_revocation_that_persists_after_it() {
    let (runtime, runner, root) = fixture("f1-approve", "svc_test");
    grant_window(&runtime);
    let (entered, release) = pause::arm(&runtime.path, "revoke-fenced");
    let revoking = runtime.clone();
    let revoke_thread = std::thread::spawn(move || execution_revoke(&revoking, "M1"));
    entered.recv_timeout(Duration::from_secs(10)).unwrap();
    // An approval runs to completion while that revocation is pending.
    grant_window(&runtime);
    let exec = route(&runtime, exec_request("exec_between", 1));
    assert_eq!(exec["error"]["code"], "EXECUTION_GRANT_REQUIRED", "{exec}");
    assert_eq!(
        runner.count("exec"),
        0,
        "the approval erased a pending revocation"
    );
    release.send(()).unwrap();
    revoke_thread.join().unwrap().unwrap();
    let after = route(&runtime, exec_request("exec_after", 1));
    assert_eq!(
        after["error"]["code"], "EXECUTION_GRANT_REQUIRED",
        "{after}"
    );
    assert_eq!(runner.count("exec"), 0);
    // Only a later explicit approval opens a new window.
    grant_window(&runtime);
    let fresh = route(&runtime, exec_request("exec_fresh", 1));
    assert_eq!(fresh["state"], "exited", "{fresh}");
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

fn inspect_view(runtime: &Runtime) -> Value {
    let value = route(
        runtime,
        request(
            "deck_session_inspect",
            json!({"session_id":"mcp_a","holder_id":"holder_a"}),
        ),
    );
    assert_eq!(value["ok"], true, "{value}");
    value
}

#[test]
fn inspect_reports_a_pending_execution_revoke_as_revoked() {
    let (runtime, runner, root) = fixture("inspect-fence", "svc_test");
    grant_window(&runtime);
    let executed = route(&runtime, exec_request("exec_job", 1));
    let job_id = executed["jobId"].as_str().unwrap().to_owned();
    let before = inspect_view(&runtime);
    assert_eq!(before["executionAuthorization"]["active"], true, "{before}");

    // C1/C2: the fence is set, the grant revocation is not persisted yet.
    let (entered, release) = pause::arm(&runtime.path, "revoke-fenced");
    let revoking = runtime.clone();
    let revoke_thread = std::thread::spawn(move || execution_revoke(&revoking, "M1"));
    entered.recv_timeout(Duration::from_secs(10)).unwrap();
    runtime
        .read(|doc| assert!(doc.execution_grants[0].revoked_at.is_none()))
        .unwrap();
    let pending = inspect_view(&runtime);
    let authorization = &pending["executionAuthorization"];
    assert_eq!(authorization["status"], "revoked", "{pending}");
    assert_eq!(authorization["active"], false);
    assert_eq!(authorization["stdinApprovedForActiveGrant"], false);
    assert_eq!(
        authorization["expiresAtUnixMs"],
        before["executionAuthorization"]["expiresAtUnixMs"]
    );
    assert_eq!(
        pending["mayStartNextJobReason"], "EXECUTION_GRANT_REQUIRED",
        "{pending}"
    );
    assert_eq!(pending["outputSharing"], before["outputSharing"]);
    assert_eq!(pending["activeJob"], before["activeJob"]);
    assert_eq!(pending["foreground"], before["foreground"]);
    let exec = route(&runtime, exec_request("exec_pending", 1));
    assert_eq!(exec["error"]["code"], "EXECUTION_GRANT_REQUIRED", "{exec}");
    let input = route(&runtime, input_request("input_pending", &job_id));
    assert_eq!(
        input["error"]["code"], "EXECUTION_GRANT_REQUIRED",
        "{input}"
    );
    assert_eq!(runner.count("interrupt"), 0, "nothing was interrupted");

    // C3: persisted, fence lowered — still revoked, never active again.
    release.send(()).unwrap();
    revoke_thread.join().unwrap().unwrap();
    assert!(!execution_fenced(&runtime, "mcp_a"));
    let persisted = inspect_view(&runtime);
    assert_eq!(persisted["executionAuthorization"]["status"], "revoked");
    assert_eq!(persisted["executionAuthorization"]["active"], false);
    assert_eq!(persisted["outputSharing"], before["outputSharing"]);
    assert_eq!(persisted["outputSharing"]["sessionGateOpen"], true);

    // C4: a later local approval is a new, active grant.
    grant_window(&runtime);
    let renewed = inspect_view(&runtime);
    assert_eq!(renewed["executionAuthorization"]["status"], "active");
    assert_eq!(
        renewed["executionAuthorization"]["stdinApprovedForActiveGrant"],
        true
    );
    let fresh = route(&runtime, exec_request("exec_fresh", 1));
    assert_eq!(fresh["state"], "exited", "{fresh}");

    // C5: a naturally expired grant stays `expired` under a pending fence.
    runtime
        .write(|doc| {
            for grant in &mut doc.execution_grants {
                grant.expires_at = grant.issued_at;
            }
            Ok(())
        })
        .unwrap();
    let (entered, release) = pause::arm(&runtime.path, "revoke-fenced");
    let revoking = runtime.clone();
    let revoke_thread = std::thread::spawn(move || execution_revoke(&revoking, "M1"));
    entered.recv_timeout(Duration::from_secs(10)).unwrap();
    let expired = inspect_view(&runtime);
    assert_eq!(expired["executionAuthorization"]["status"], "expired");
    assert_eq!(expired["executionAuthorization"]["active"], false);
    release.send(()).unwrap();
    revoke_thread.join().unwrap().unwrap();
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

fn read_job(runtime: &Runtime, job_id: &str) -> Value {
    route(
        runtime,
        request("deck_job_read", json!({"job_id":job_id,"max_bytes":1024})),
    )
}

#[test]
fn execution_revoke_leaves_output_sharing_and_a_running_job_alone() {
    let (runtime, runner, root) = fixture("revoke-sharing", "svc_test");
    grant_window(&runtime);
    let executed = route(&runtime, exec_request("exec_job", 1));
    let job_id = executed["jobId"].as_str().unwrap().to_owned();
    // The job is running at the runner, and stdin reaches it: the later
    // denial is the revocation, not JOB_NOT_RUNNING.
    runner.busy.store(true, Ordering::SeqCst);
    let baseline = route(&runtime, input_request("input_base", &job_id));
    assert_eq!(baseline["state"], "committed", "{baseline}");
    let before = inspect_view(&runtime);
    assert_eq!(before["executionAuthorization"]["active"], true);
    assert_eq!(before["outputSharing"]["sessionGateOpen"], true);
    assert_eq!(before["activeJob"]["state"], "running", "{before}");
    let (exec_count, input_count) = (runner.count("exec"), runner.count("input"));

    execution_revoke(&runtime, "M1").unwrap();

    let after = inspect_view(&runtime);
    let authorization = &after["executionAuthorization"];
    assert_eq!(authorization["status"], "revoked", "{after}");
    assert_eq!(authorization["active"], false);
    assert_eq!(authorization["stdinApprovedForActiveGrant"], false);
    assert_eq!(after["outputSharing"], before["outputSharing"], "{after}");
    assert!(session_state(&runtime).output_shared);
    // A1: the job's output is read through its binding, not the grant.
    runtime
        .read(|doc| {
            let binding = doc.jobs.iter().find(|job| job.job_id == job_id).unwrap();
            let grant = doc
                .execution_grants
                .iter()
                .find(|grant| grant.grant_id == binding.grant_id)
                .unwrap();
            assert!(grant.revoked_at.is_some(), "its grant is revoked");
        })
        .unwrap();
    let read = read_job(&runtime, &job_id);
    assert_eq!(read["ok"], true, "{read}");
    assert_eq!(read["output"], "done\n", "{read}");
    assert_eq!(read["gap"], false);
    assert_eq!(read["droppedBytes"], 0);
    // A2/A3: sharing grants no execution authority.
    let exec = route(&runtime, exec_request("exec_after", 1));
    assert_eq!(exec["error"]["code"], "EXECUTION_GRANT_REQUIRED", "{exec}");
    // Once persisted, stdin is refused by the job's own (revoked) grant.
    let input = route(&runtime, input_request("input_after", &job_id));
    assert_eq!(input["error"]["code"], "STDIN_NOT_AUTHORIZED", "{input}");
    assert_eq!(runner.count("exec"), exec_count);
    assert_eq!(runner.count("input"), input_count);
    // A4: revocation is not an interrupt.
    assert_eq!(runner.count("interrupt"), 0);
    assert_eq!(runner.count("stop"), 0);
    assert_eq!(after["activeJob"], before["activeJob"]);
    assert_eq!(after["foreground"], "managed-job");
    runner.busy.store(false, Ordering::SeqCst);
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn natural_expiry_leaves_completed_retained_output_readable() {
    let (runtime, runner, root) = fixture("expiry-sharing", "svc_test");
    grant_window(&runtime);
    let executed = route(&runtime, exec_request("exec_completed", 1));
    assert_eq!(executed["state"], "exited", "{executed}");
    let job_id = executed["jobId"].as_str().unwrap().to_owned();

    test_clock::advance(60_001);

    let expired = inspect_view(&runtime);
    assert_eq!(expired["executionAuthorization"]["status"], "expired");
    assert_eq!(expired["outputSharing"]["sessionGateOpen"], true);
    assert_eq!(expired["outputSharing"]["jobBindingsOpen"], 1);
    assert_eq!(expired["outputSharing"]["jobBindingsClosed"], 0);
    let read = read_job(&runtime, &job_id);
    assert_eq!(read["ok"], true, "{read}");
    assert_eq!(read["state"], "exited", "{read}");
    assert_eq!(read["output"], "done\n", "{read}");

    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn session_pause_is_distinct_from_an_open_job_binding() {
    let (runtime, runner, root) = fixture("session-sharing-paused", "svc_test");
    grant_window(&runtime);
    let executed = route(&runtime, exec_request("exec_shared", 1));
    let job_id = executed["jobId"].as_str().unwrap().to_owned();
    assert_eq!(read_job(&runtime, &job_id)["ok"], true);

    super::execution_grant(&runtime, "mcp_a".into(), Some(60_000), true, false).unwrap();

    let view = inspect_view(&runtime);
    assert_eq!(view["outputSharing"]["sessionGateOpen"], false);
    assert_eq!(view["outputSharing"]["jobBindingsOpen"], 1);
    assert_eq!(view["outputSharing"]["jobBindingsClosed"], 0);
    let paused = read_job(&runtime, &job_id);
    assert_eq!(
        paused["error"]["code"], "SESSION_OUTPUT_SHARING_PAUSED",
        "{paused}"
    );

    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn reapprove_then_expiry_keeps_takeover_closed_job_distinct_from_fresh_job() {
    let (runtime, runner, root) = fixture("take-exp", "svc_test");
    grant_window(&runtime);
    let historical = route(&runtime, exec_request("exec_historical", 1));
    assert_eq!(historical["state"], "exited", "{historical}");
    let historical_job = historical["jobId"].as_str().unwrap().to_owned();
    assert_eq!(read_job(&runtime, &historical_job)["ok"], true);

    takeover(&runtime, "M1").unwrap();
    return_control(&runtime, "M1").unwrap();
    grant_window(&runtime);
    let epoch = control_request(&runtime, "control_after_return");
    let fresh = route(&runtime, exec_request("exec_fresh_after_return", epoch));
    assert_eq!(fresh["state"], "exited", "{fresh}");
    let fresh_job = fresh["jobId"].as_str().unwrap().to_owned();

    let active = inspect_view(&runtime);
    assert_eq!(active["executionAuthorization"]["status"], "active");
    assert_eq!(active["outputSharing"]["sessionGateOpen"], true);
    assert_eq!(active["outputSharing"]["jobBindingsOpen"], 1);
    assert_eq!(active["outputSharing"]["jobBindingsClosed"], 1);
    let historical_denied = read_job(&runtime, &historical_job);
    assert_eq!(
        historical_denied["error"]["code"], "JOB_OUTPUT_BINDING_CLOSED",
        "{historical_denied}"
    );
    assert_eq!(read_job(&runtime, &fresh_job)["ok"], true);

    test_clock::advance(60_001);

    let expired = inspect_view(&runtime);
    assert_eq!(expired["executionAuthorization"]["status"], "expired");
    assert_eq!(expired["outputSharing"]["sessionGateOpen"], true);
    assert_eq!(expired["outputSharing"]["jobBindingsOpen"], 1);
    assert_eq!(expired["outputSharing"]["jobBindingsClosed"], 1);
    let still_closed = read_job(&runtime, &historical_job);
    assert_eq!(
        still_closed["error"]["code"], "JOB_OUTPUT_BINDING_CLOSED",
        "{still_closed}"
    );
    let fresh_after_expiry = read_job(&runtime, &fresh_job);
    assert_eq!(fresh_after_expiry["ok"], true, "{fresh_after_expiry}");
    assert_eq!(fresh_after_expiry["state"], "exited");

    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn execution_revoke_never_opens_output_sharing() {
    let (runtime, runner, root) = fixture("revoke-unshared", "svc_test");
    super::execution_grant(&runtime, "mcp_a".into(), Some(60_000), true, false).unwrap();
    let executed = route(&runtime, exec_request("exec_job", 1));
    let job_id = executed["jobId"].as_str().unwrap().to_owned();
    assert_eq!(
        inspect_view(&runtime)["outputSharing"]["sessionGateOpen"],
        false
    );
    execution_revoke(&runtime, "M1").unwrap();
    let after = inspect_view(&runtime);
    assert_eq!(after["executionAuthorization"]["status"], "revoked");
    assert_eq!(after["outputSharing"]["sessionGateOpen"], false, "{after}");
    assert!(!session_state(&runtime).output_shared);
    let read = read_job(&runtime, &job_id);
    assert_eq!(read["error"]["code"], "JOB_OUTPUT_BINDING_CLOSED", "{read}");
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn f1_a_job_bound_to_a_revoked_grant_never_revives_under_a_new_grant() {
    let (runtime, runner, root) = fixture("f1-revive", "svc_test");
    grant_window(&runtime);
    let executed = route(&runtime, exec_request("exec_old", 1));
    let job_id = executed["jobId"].as_str().unwrap().to_owned();
    execution_revoke(&runtime, "M1").unwrap();
    grant_window(&runtime);
    let input = route(&runtime, input_request("input_old_grant", &job_id));
    assert_eq!(input["ok"], false, "{input}");
    assert_eq!(runner.count("input"), 0);
    // The replayed exec returns its record and is never re-dispatched.
    let replay = route(&runtime, exec_request("exec_old", 1));
    assert_eq!(replay["jobId"], executed["jobId"], "{replay}");
    assert_eq!(runner.count("exec"), 1);
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

// ---- F2 / C1 / F3: request identity, renew replay, capacity recovery ----

fn control(
    runtime: &Runtime,
    request_id: &str,
    action: &str,
    epoch: Option<u64>,
    sequence: u64,
) -> Value {
    let mut arguments = json!({"request_id":request_id,"session_id":"mcp_a","expected_generation":"g_a","action":action,"holder_id":"holder_a","control_sequence":sequence});
    if let Some(epoch) = epoch {
        arguments["control_epoch"] = json!(epoch);
    }
    route(runtime, request("deck_session_control", arguments))
}

fn create(runtime: &Runtime, root: &Path, request_id: &str, sequence: u64, title: &str) -> Value {
    route(
        runtime,
        request(
            "deck_session_create",
            json!({"request_id":request_id,"project_id":"P1","cwd":root.display().to_string(),"title":title,"create_sequence":sequence}),
        ),
    )
}

/// A new Deck process over the same state file (restart).
fn reload(runtime: &Runtime, service: &str) -> Arc<Runtime> {
    Arc::new(Runtime {
        app: None,
        path: runtime.path.clone(),
        socket: runtime.socket.clone(),
        doc: Mutex::new(load(&runtime.path)),
        io: Mutex::new(()),
        delivery: Mutex::new(()),
        emergency: Mutex::new(EmergencyFences::default()),
        service_instance: service.into(),
        runner_auth: Mutex::new(HashMap::new()),
        started: Instant::now(),
    })
}

/// Terminal exec records of `client_id`, bound to `session_id`/`epoch`.
fn fill(runtime: &Runtime, client_id: &str, session_id: &str, epoch: u64, count: usize, tag: &str) {
    runtime
        .write(|doc| {
            for index in 0..count {
                let mut filler = operation("exec", "committed");
                filler.operation_id = format!("op_{tag}_{index}");
                filler.request_id = format!("{tag}_{index}");
                filler.client_id = client_id.into();
                filler.session_id = Some(session_id.into());
                filler.control_epoch = Some(epoch);
                doc.operations.push(filler);
            }
            Ok(())
        })
        .unwrap();
}

fn ordinary_count(runtime: &Runtime) -> usize {
    runtime
        .read(|doc| {
            doc.operations
                .iter()
                .filter(|op| !is_control_record(op))
                .count()
        })
        .unwrap()
}

fn own_count(runtime: &Runtime, client_id: &str) -> usize {
    runtime
        .read(|doc| {
            doc.operations
                .iter()
                .filter(|op| op.client_id == client_id && !is_control_record(op))
                .count()
        })
        .unwrap()
}

#[test]
fn f2_a_retired_control_request_is_never_applied_again() {
    let (runtime, runner, root) = fixture("f2-control", "svc_test");
    unowned(&runtime);
    let first = control(&runtime, "req_first", "request", None, 0);
    assert_eq!(first["state"], "committed", "{first}");
    let epoch = first["result"]["controlEpoch"].as_u64().unwrap();
    let released = control(&runtime, "rel_first", "release", Some(epoch), 1);
    assert_eq!(released["state"], "committed", "{released}");
    // The first request's record is retired by the release.
    assert!(operation_by_request(&runtime, "req_first").is_none());
    let before = session_state(&runtime);
    assert!(!before.human_lock && before.control_owner.is_none());
    let controls = runner.count("control");
    let replay = control(&runtime, "req_first", "request", None, 0);
    assert_eq!(replay["error"]["code"], "STALE_REQUEST", "{replay}");
    let after = session_state(&runtime);
    assert_eq!(after.control_owner, None);
    assert_eq!(after.control_holder, None);
    assert_eq!(after.lease_expires_at, None);
    assert_eq!(after.control_epoch, before.control_epoch);
    assert_eq!(after.control_sequence, before.control_sequence);
    assert_eq!(
        runner.count("control"),
        controls,
        "a replay reached the runner"
    );
    // Same after a local takeover and an explicit return.
    takeover(&runtime, "M1").unwrap();
    return_control(&runtime, "M1").unwrap();
    let controls = runner.count("control");
    let replay = control(&runtime, "req_first", "request", None, 0);
    assert_eq!(replay["error"]["code"], "STALE_REQUEST", "{replay}");
    assert_eq!(session_state(&runtime).control_owner, None);
    assert_eq!(runner.count("control"), controls);
    // Same after a restart.
    let reloaded = reload(&runtime, "svc_reloaded");
    let replay = control(&reloaded, "req_first", "request", None, 0);
    assert_eq!(replay["error"]["code"], "STALE_REQUEST", "{replay}");
    assert_eq!(session_state(&reloaded).control_owner, None);
    assert_eq!(runner.count("control"), controls);
    // New requests are never locked out.
    let sequence = session_state(&reloaded).control_sequence;
    let fresh = control(&reloaded, "req_fresh", "request", None, sequence);
    assert_eq!(fresh["state"], "committed", "{fresh}");
    // Same identity, different arguments: a conflict while the record is
    // kept; once superseded, stale (never re-accepted, never compared).
    let conflict = control(&reloaded, "req_fresh", "request", None, sequence + 7);
    assert_eq!(
        conflict["error"]["code"], "REQUEST_ID_CONFLICT",
        "{conflict}"
    );
    let moved = control(
        &reloaded,
        "req_moved",
        "renew",
        Some(fresh["result"]["controlEpoch"].as_u64().unwrap()),
        sequence + 1,
    );
    assert_eq!(moved["state"], "committed", "{moved}");
    let stale = control(&reloaded, "req_fresh", "request", None, sequence + 7);
    assert_eq!(stale["error"]["code"], "STALE_REQUEST", "{stale}");
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn f2_a_retired_create_is_never_accepted_again() {
    let (runtime, runner, root) = fixture("f2-create", "svc_test");
    let first = create(&runtime, &root, "create_first", 0, "First");
    assert_eq!(first["state"], "accepted", "{first}");
    let first_id = first["operationId"].as_str().unwrap().to_owned();
    claim(&runtime, &first_id).unwrap();
    complete(
        &runtime,
        first_id,
        "committed".into(),
        None,
        Some("deck-mcp-first".into()),
    )
    .unwrap();
    // Push it out of the result window with later, finished creates.
    for index in 1..=CREATE_REPLAY_WINDOW as u64 {
        let later = create(&runtime, &root, &format!("create_{index}"), index, "Later");
        assert_eq!(later["state"], "accepted", "{later}");
        let id = later["operationId"].as_str().unwrap().to_owned();
        claim(&runtime, &id).unwrap();
        complete(
            &runtime,
            id,
            "rejected".into(),
            Some("create-failed".into()),
            None,
        )
        .unwrap();
    }
    assert!(operation_by_request(&runtime, "create_first").is_none());
    let sessions = runtime.read(|doc| doc.sessions.len()).unwrap();
    let operations = runtime.read(|doc| doc.operations.len()).unwrap();
    let pending = || {
        runtime
            .read(|doc| {
                doc.operations
                    .iter()
                    .filter(|op| op.state == "accepted")
                    .count()
            })
            .unwrap()
    };
    let replay = create(&runtime, &root, "create_first", 0, "First");
    assert_eq!(replay["error"]["code"], "STALE_REQUEST", "{replay}");
    assert_eq!(runtime.read(|doc| doc.sessions.len()).unwrap(), sessions);
    assert_eq!(
        runtime.read(|doc| doc.operations.len()).unwrap(),
        operations
    );
    assert_eq!(pending(), 0, "a retired create was accepted again");
    // A different body under the retired identity is stale too — Deck
    // does not pretend to have compared it with deleted arguments.
    let changed = create(&runtime, &root, "create_first", 0, "Changed");
    assert_eq!(changed["error"]["code"], "STALE_REQUEST", "{changed}");
    // Inside the window the same identity with other arguments conflicts.
    let last = format!("create_{}", CREATE_REPLAY_WINDOW);
    let conflict = create(&runtime, &root, &last, CREATE_REPLAY_WINDOW as u64, "Other");
    assert_eq!(
        conflict["error"]["code"], "REQUEST_ID_CONFLICT",
        "{conflict}"
    );
    let reloaded = reload(&runtime, "svc_reloaded");
    let replay = create(&reloaded, &root, "create_first", 0, "First");
    assert_eq!(replay["error"]["code"], "STALE_REQUEST", "{replay}");
    // A new create at the current sequence still works.
    let next = CREATE_REPLAY_WINDOW as u64 + 1;
    let fresh = create(&reloaded, &root, "create_fresh", next, "Fresh");
    assert_eq!(fresh["state"], "accepted", "{fresh}");
    let listed = route(&reloaded, request("deck_sessions_list", json!({})));
    assert_eq!(listed["nextCreateSequence"], next + 1);
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn f2_an_ambiguous_exec_is_never_dispatched_again() {
    let (runtime, runner, root) = fixture("f2-unknown", "svc_test");
    grant_window(&runtime);
    // The runner vanishes: Deck cannot prove whether dispatch happened.
    drop(runner);
    let lost = route(&runtime, exec_request("exec_lost", 1));
    assert_eq!(lost["error"]["code"], "OPERATION_AMBIGUOUS", "{lost}");
    assert_eq!(
        operation_by_request(&runtime, "exec_lost").unwrap().state,
        "ambiguous"
    );
    let runner = FakeRunner::start(&root, "g_a");
    let replay = route(&runtime, exec_request("exec_lost", 1));
    assert_eq!(replay["ok"], true, "{replay}");
    assert_eq!(
        runner.count("exec"),
        0,
        "an ambiguous exec was dispatched again"
    );
    // Restart: the record's epoch is closed, the replay is refused.
    let reloaded = reload(&runtime, "svc_reloaded");
    let replay = route(&reloaded, exec_request("exec_lost", 1));
    assert_eq!(replay["error"]["code"], "CONTROL_REVOKED", "{replay}");
    assert_eq!(runner.count("exec"), 0);
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn f2_an_ambiguous_close_outlives_the_result_window() {
    let (runtime, runner, root) = fixture("f2-close", "svc_test");
    let close = |request_id: &str| {
        route(
            &runtime,
            request(
                "deck_session_close",
                json!({"request_id":request_id,"session_id":"mcp_a","expected_generation":"g_a","control_epoch":1,"holder_id":"holder_a"}),
            ),
        )
    };
    let unknown = close("close_unknown");
    let unknown_id = unknown["operationId"].as_str().unwrap().to_owned();
    claim(&runtime, &unknown_id).unwrap();
    close_admit(&runtime, &unknown_id).unwrap();
    complete(
        &runtime,
        unknown_id.clone(),
        "ambiguous".into(),
        Some("close-failed".into()),
        None,
    )
    .unwrap();
    for index in 0..CREATE_REPLAY_WINDOW {
        let later = close(&format!("close_{index}"));
        let id = later["operationId"].as_str().unwrap().to_owned();
        claim(&runtime, &id).unwrap();
        complete(
            &runtime,
            id,
            "rejected".into(),
            Some("close-failed".into()),
            None,
        )
        .unwrap();
    }
    let replay = close("close_unknown");
    assert_eq!(replay["operationId"], unknown_id.as_str(), "{replay}");
    assert_eq!(replay["state"], "ambiguous");
    runtime
        .read(|doc| {
            assert!(
                doc.operations.iter().all(|op| op.state != "accepted"),
                "re-admitted"
            );
            assert!(!doc.sessions[0].closing);
        })
        .unwrap();
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn c1_an_exact_renew_replay_never_extends_the_lease() {
    let (runtime, runner, root) = fixture("c1-renew", "svc_test");
    unowned(&runtime);
    grant_window(&runtime);
    let grant = runtime
        .read(|doc| doc.execution_grants.last().unwrap().clone())
        .unwrap();
    let requested = control(&runtime, "req_a", "request", None, 0);
    let epoch = requested["result"]["controlEpoch"].as_u64().unwrap();
    let renew = control(&runtime, "renew_a", "renew", Some(epoch), 1);
    assert_eq!(renew["state"], "committed", "{renew}");
    assert!(
        renew["operationId"].is_string(),
        "a renew has a queryable operation"
    );
    let first_deadline = renew["result"]["leaseExpiresAt"].as_u64().unwrap();
    test_clock::advance(30_000);
    let replay = control(&runtime, "renew_a", "renew", Some(epoch), 1);
    assert_eq!(replay["operationId"], renew["operationId"], "{replay}");
    assert_eq!(replay["result"]["leaseExpiresAt"], first_deadline);
    assert_eq!(
        session_state(&runtime).lease_expires_at,
        Some(first_deadline),
        "the replay extended the lease"
    );
    let queried = route(
        &runtime,
        request(
            "deck_operation_get",
            json!({"operation_id":renew["operationId"]}),
        ),
    );
    assert_eq!(queried["kind"], "session-control");
    // A new logical renew extends it.
    let renewed = control(&runtime, "renew_b", "renew", Some(epoch), 2);
    let second_deadline = renewed["result"]["leaseExpiresAt"].as_u64().unwrap();
    assert!(second_deadline >= first_deadline + 30_000);
    // The superseded renew is stale now and changes nothing.
    let stale = control(&runtime, "renew_a", "renew", Some(epoch), 1);
    assert_eq!(stale["error"]["code"], "STALE_REQUEST", "{stale}");
    assert_eq!(
        session_state(&runtime).lease_expires_at,
        Some(second_deadline)
    );
    // The execution window never moves with the lease.
    let current = runtime
        .read(|doc| doc.execution_grants.last().unwrap().clone())
        .unwrap();
    assert_eq!(
        (current.grant_id.clone(), current.expires_at),
        (grant.grant_id.clone(), grant.expires_at)
    );
    // Sustained renewals keep one control record per session.
    let mut sequence = 3;
    let mut last = Value::Null;
    for index in 0..200 {
        let value = control(
            &runtime,
            &format!("renew_many_{index}"),
            "renew",
            Some(epoch),
            sequence,
        );
        assert_eq!(value["state"], "committed", "{value}");
        sequence += 1;
        last = value;
    }
    assert_eq!(
        runtime
            .read(|doc| doc
                .operations
                .iter()
                .filter(|op| is_control_record(op))
                .count())
            .unwrap(),
        1
    );
    // After the context ends, the latest renew cannot restore control.
    takeover(&runtime, "M1").unwrap();
    return_control(&runtime, "M1").unwrap();
    let replay = control(
        &runtime,
        "renew_many_199",
        "renew",
        Some(epoch),
        sequence - 1,
    );
    // Either the recorded (historical) receipt of that very renew, or a
    // stale refusal once it is retired — never a new effect.
    assert!(
        replay["operationId"] == last["operationId"] || replay["error"]["code"] == "STALE_REQUEST",
        "{replay}"
    );
    assert_eq!(
        session_state(&runtime).control_owner,
        None,
        "a renew replay restored control"
    );
    assert_eq!(session_state(&runtime).lease_expires_at, None);
    let reloaded = reload(&runtime, "svc_reloaded");
    let replay = control(&reloaded, "renew_a", "renew", Some(epoch), 1);
    assert_eq!(replay["error"]["code"], "STALE_REQUEST", "{replay}");
    assert_eq!(session_state(&reloaded).lease_expires_at, None);
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn f3_a_full_client_quota_recovers_by_release_and_request() {
    let (runtime, runner, root) = fixture("f3-client", "svc_test");
    unowned(&runtime);
    grant_window(&runtime);
    let epoch = control_request(&runtime, "req_a");
    let before = route(&runtime, exec_request("exec_before", epoch));
    assert_eq!(before["state"], "exited", "{before}");
    let own = own_count(&runtime, "client_a");
    fill(
        &runtime,
        "client_a",
        "mcp_a",
        epoch,
        MAX_OPERATIONS_PER_CLIENT - own,
        "own",
    );
    let blocked = route(&runtime, exec_request("exec_blocked", epoch));
    assert_eq!(blocked["error"]["code"], "CAPACITY_EXCEEDED", "{blocked}");
    assert_eq!(runner.count("exec"), 1);
    // The recovery the error names, performed for real.
    release(&runtime, "rel_a", epoch);
    let next = control_request(&runtime, "req_b");
    assert!(
        own_count(&runtime, "client_a") < 8,
        "the old epoch was not retired"
    );
    let after = route(&runtime, exec_request("exec_after", next));
    assert_eq!(after["state"], "exited", "{after}");
    assert_eq!(runner.count("exec"), 2);
    let replay = route(&runtime, exec_request("exec_before", epoch));
    assert_eq!(replay["error"]["code"], "CONTROL_REVOKED", "{replay}");
    assert_eq!(runner.count("exec"), 2, "a retired exec ran again");
    // The boundary survives a restart; new work continues.
    let reloaded = reload(&runtime, "svc_reloaded");
    for (request_id, bound) in [("exec_before", epoch), ("exec_after", next)] {
        let replay = route(&reloaded, exec_request(request_id, bound));
        assert_eq!(replay["error"]["code"], "CONTROL_REVOKED", "{replay}");
    }
    assert_eq!(runner.count("exec"), 2);
    super::execution_grant(&reloaded, "mcp_a".into(), Some(60_000), true, true).unwrap();
    let epoch = control_request(&reloaded, "req_c");
    let resumed = route(&reloaded, exec_request("exec_resumed", epoch));
    assert_eq!(resumed["state"], "exited", "{resumed}");
    assert_eq!(runner.count("exec"), 3);
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

/// A second managed session owned by another client, for global tests.
fn add_other_session(runtime: &Runtime, lease_expires_at: Option<u64>) {
    runtime
        .write(|doc| {
            let mut other = doc.sessions[0].clone();
            other.session_id = "mcp_b".into();
            other.card_id = "M2".into();
            other.owner_client_id = "client_b".into();
            other.control_owner = Some("client_b".into());
            other.control_holder = Some("holder_b".into());
            other.lease_expires_at = lease_expires_at;
            doc.sessions.push(other);
            Ok(())
        })
        .unwrap();
}

fn fill_others(runtime: &Runtime, session_id: &str, mut count: usize) {
    let mut client = 0;
    while count > 0 {
        let chunk = count.min(MAX_OPERATIONS_PER_CLIENT);
        fill(
            runtime,
            &format!("client_fill_{client}"),
            session_id,
            1,
            chunk,
            &format!("fill{client}"),
        );
        count -= chunk;
        client += 1;
    }
}

#[test]
fn f3_a_full_global_pool_recovers_by_release_and_request() {
    let (runtime, runner, root) = fixture("f3-global", "svc_test");
    unowned(&runtime);
    grant_window(&runtime);
    add_other_session(&runtime, Some(now_ms() + 600_000));
    let epoch = control_request(&runtime, "req_a");
    let before = route(&runtime, exec_request("exec_before", epoch));
    assert_eq!(before["state"], "exited", "{before}");
    fill(&runtime, "client_a", "mcp_a", epoch, 300, "own");
    fill_others(
        &runtime,
        "mcp_b",
        ORDINARY_OPERATIONS - ordinary_count(&runtime),
    );
    assert_eq!(ordinary_count(&runtime), ORDINARY_OPERATIONS);
    assert!(own_count(&runtime, "client_a") < MAX_OPERATIONS_PER_CLIENT);
    let blocked = route(&runtime, exec_request("exec_blocked", epoch));
    assert_eq!(blocked["error"]["code"], "CAPACITY_EXCEEDED", "{blocked}");
    release(&runtime, "rel_a", epoch);
    let next = control_request(&runtime, "req_b");
    let after = route(&runtime, exec_request("exec_after", next));
    assert_eq!(after["state"], "exited", "{after}");
    assert_eq!(runner.count("exec"), 2);
    let replay = route(&runtime, exec_request("exec_before", epoch));
    assert_eq!(replay["error"]["code"], "CONTROL_REVOKED", "{replay}");
    assert_eq!(runner.count("exec"), 2);
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();

    // Another client whose lease lapsed holds the whole pool: its dead
    // epoch is closed under pressure and this client proceeds.
    let (runtime, runner, root) = fixture("f3-lapsed", "svc_test");
    unowned(&runtime);
    grant_window(&runtime);
    add_other_session(&runtime, Some(now_ms().saturating_sub(1)));
    let epoch = control_request(&runtime, "req_a");
    fill_others(
        &runtime,
        "mcp_b",
        ORDINARY_OPERATIONS - ordinary_count(&runtime),
    );
    let proceeded = route(&runtime, exec_request("exec_after_lapse", epoch));
    assert_eq!(proceeded["state"], "exited", "{proceeded}");
    runtime
        .read(|doc| {
            let other = doc
                .sessions
                .iter()
                .find(|s| s.session_id == "mcp_b")
                .unwrap();
            assert_eq!(other.control_owner, None);
            assert_eq!(other.control_epoch, 2);
            assert!(doc.operations.len() < 16);
        })
        .unwrap();
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn f3_a_lapsed_lease_at_full_capacity_recovers_by_request_without_a_new_grant() {
    let (runtime, runner, root) = fixture("f3-lapse", "svc_test");
    unowned(&runtime);
    super::execution_grant(
        &runtime,
        "mcp_a".into(),
        Some(MAX_EXECUTION_GRANT_MS),
        true,
        true,
    )
    .unwrap();
    let grant = runtime
        .read(|doc| doc.execution_grants.last().unwrap().clone())
        .unwrap();
    let epoch = control_request(&runtime, "req_a");
    let own = own_count(&runtime, "client_a");
    fill(
        &runtime,
        "client_a",
        "mcp_a",
        epoch,
        MAX_OPERATIONS_PER_CLIENT - own,
        "own",
    );
    // The lease runs out naturally.
    test_clock::advance(DEFAULT_LEASE_MS + 1);
    let blocked = route(&runtime, exec_request("exec_blocked", epoch));
    assert_eq!(blocked["ok"], false, "{blocked}");
    let released = control(
        &runtime,
        "rel_late",
        "release",
        Some(epoch),
        session_state(&runtime).control_sequence,
    );
    assert_eq!(released["error"]["code"], "CONTROL_REVOKED", "{released}");
    let next = control_request(&runtime, "req_b");
    assert!(next > epoch);
    let current = runtime
        .read(|doc| doc.execution_grants.last().unwrap().clone())
        .unwrap();
    assert_eq!(
        (current.grant_id.clone(), current.expires_at),
        (grant.grant_id.clone(), grant.expires_at),
        "control changed the execution window"
    );
    let after = route(&runtime, exec_request("exec_after", next));
    assert_eq!(after["state"], "exited", "{after}");
    assert_eq!(runner.count("exec"), 1);
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn f3_interrupt_reserve_survives_a_full_pool_and_one_greedy_client() {
    let (runtime, runner, root) = fixture("f3-interrupt", "svc_test");
    grant_window(&runtime);
    let executed = route(&runtime, exec_request("exec_live", 1));
    let job_id = executed["jobId"].as_str().unwrap().to_owned();
    // A second client with its own session, runner and job.
    let root_b = test_root("f3-interrupt-b");
    let runner_b = FakeRunner::start(&root_b, "g_b");
    runner_b.set_control(1, "mcp", Some("holder_b"));
    runtime
        .write(|doc| {
            let mut client_b = client_record(&root);
            client_b.id = "client_b".into();
            client_b.credential_hash = sha(b"mcp_test_b");
            doc.config.clients.push(client_b);
            let mut session_b = session_record(&root, &runner_b);
            session_b.session_id = "mcp_b".into();
            session_b.card_id = "M2".into();
            session_b.generation = "g_b".into();
            session_b.owner_client_id = "client_b".into();
            session_b.control_owner = Some("client_b".into());
            session_b.control_holder = Some("holder_b".into());
            doc.sessions.push(session_b.clone());
            let mut grant_b = execution_grant(&session_b);
            grant_b.grant_id = "grant_b".into();
            doc.execution_grants.push(grant_b);
            doc.jobs.push(JobBinding {
                job_id: "job_b".into(),
                client_id: "client_b".into(),
                session_id: "mcp_b".into(),
                session_generation: "g_b".into(),
                request_hash: "b".repeat(64),
                operation_id: "op_b".into(),
                grant_id: "grant_b".into(),
                grant_version: 1,
                allow_output: true,
            });
            Ok(())
        })
        .unwrap();
    fill_others(
        &runtime,
        "mcp_a",
        ORDINARY_OPERATIONS - ordinary_count(&runtime),
    );
    let blocked = route(&runtime, exec_request("exec_full", 1));
    assert_eq!(blocked["error"]["code"], "CAPACITY_EXCEEDED", "{blocked}");
    let first = route(&runtime, interrupt_request("stop_0", &job_id));
    assert_eq!(first["state"], "committed", "{first}");
    // Accepted is only a request: the job state comes from deck_job_read.
    assert!(first.get("processComplete").is_none());
    // An exact repeat uses no slot and sends nothing.
    let interrupts = runner.count("interrupt");
    let repeated = route(&runtime, interrupt_request("stop_0", &job_id));
    assert_eq!(repeated["operationId"], first["operationId"]);
    assert_eq!(runner.count("interrupt"), interrupts);
    for index in 1..INTERRUPT_RESERVE_PER_CLIENT {
        let value = route(
            &runtime,
            interrupt_request(&format!("stop_{index}"), &job_id),
        );
        assert_eq!(value["state"], "committed", "{value}");
    }
    let greedy = route(&runtime, interrupt_request("stop_greedy", &job_id));
    assert_eq!(greedy["error"]["code"], "CAPACITY_EXCEEDED", "{greedy}");
    // The other client can still stop its own work.
    let other = route(
        &runtime,
        WireRequest {
            version: CONTROL_PROTOCOL,
            client_id: "client_b".into(),
            credential: "mcp_test_b".into(),
            tool: "deck_job_interrupt".into(),
            arguments: json!({"request_id":"stop_b","job_id":"job_b","session_generation":"g_b","control_epoch":1,"holder_id":"holder_b"}),
        },
    );
    assert_eq!(other["state"], "committed", "{other}");
    assert_eq!(runner_b.count("interrupt"), 1);
    drop(runner_b);
    drop(runner);
    std::fs::remove_dir_all(root_b).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn control_request_must_arrive_complete_within_the_deadline() {
    let (mut idle, _peer) = UnixStream::pair().unwrap();
    let started = Instant::now();
    assert!(read_request_line(&mut idle).is_none());
    assert!(started.elapsed() < CONNECTION_TIMEOUT);

    let (mut served, mut client) = UnixStream::pair().unwrap();
    client.write_all(b"{\"a\":1}\n").unwrap();
    assert_eq!(read_request_line(&mut served).unwrap(), b"{\"a\":1}\n");

    let (mut partial, mut client) = UnixStream::pair().unwrap();
    client.write_all(b"{\"a\":").unwrap();
    assert!(read_request_line(&mut partial).is_none());
}

/// Runtime with one client scoped to `root` and no managed session: the
/// structured project reads need no runner.
fn project_runtime(tag: &str) -> (Arc<Runtime>, PathBuf) {
    let root = test_root(tag);
    let mut doc = DiskDoc::default();
    doc.config.enabled = true;
    doc.config.clients.push(client_record(&root));
    let path = root.join("mcp.json");
    save(&path, &doc).unwrap();
    let runtime = Arc::new(Runtime {
        app: None,
        path,
        socket: root.join("control.sock"),
        doc: Mutex::new(Ok(doc)),
        io: Mutex::new(()),
        delivery: Mutex::new(()),
        emergency: Mutex::new(EmergencyFences::default()),
        service_instance: "svc_test".into(),
        runner_auth: Mutex::new(HashMap::new()),
        started: Instant::now(),
    });
    (runtime, root)
}

/// A read cursor is bound to this Deck run, the client, the project, the
/// root, the target and the content it was issued for: an altered, foreign or
/// malformed cursor restarts the read instead of slicing another file.
#[test]
fn project_read_pages_under_a_bound_cursor_and_lists_the_authorized_root() {
    let (runtime, root) = project_runtime("project-read");
    std::fs::write(root.join("a.txt"), "hello world\nsecond line\n").unwrap();
    std::fs::create_dir(root.join("sub")).unwrap();
    std::fs::write(root.join("sub").join("b.txt"), "needle one\n").unwrap();
    std::fs::write(root.join("c.txt"), "needle two\nneedle three\n").unwrap();

    let listing = project_list(&runtime, "client_a", json!({"project_id":"P1"})).unwrap();
    assert_eq!(listing["ok"], true);
    assert_eq!(listing["rootIndex"], 0);
    let names: Vec<&str> = listing["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry["name"].as_str())
        .collect();
    assert!(
        names.contains(&"a.txt") && names.contains(&"sub"),
        "{names:?}"
    );
    assert_eq!(
        project_list(
            &runtime,
            "client_a",
            json!({"project_id":"P1","path":"missing"})
        )
        .unwrap_err()["error"]["code"],
        "READ_DENIED"
    );
    assert_eq!(
        project_list(
            &runtime,
            "client_a",
            json!({"project_id":"P1","path":"../outside"})
        )
        .unwrap_err()["error"]["code"],
        "INVALID_ARGUMENTS"
    );
    assert_eq!(
        project_list(
            &runtime,
            "client_a",
            json!({"project_id":"P1","root_index":5})
        )
        .unwrap_err()["error"]["code"],
        "PERMISSION_DENIED",
        "an unauthorized root index never names a path"
    );

    let first = project_read(
        &runtime,
        "client_a",
        json!({"project_id":"P1","path":"a.txt","max_bytes":16}),
    )
    .unwrap();
    assert_eq!(first["content"], "hello world\nseco");
    assert_eq!(first["truncated"], true);
    let cursor = first["nextCursor"].as_str().unwrap().to_owned();
    assert!(cursor.starts_with("16."), "{cursor}");
    let rest = project_read(
        &runtime,
        "client_a",
        json!({"project_id":"P1","path":"a.txt","max_bytes":16,"cursor":cursor}),
    )
    .unwrap();
    assert_eq!(rest["content"], "nd line\n");
    assert_eq!(rest["truncated"], false);
    assert!(rest["nextCursor"].is_null());

    let read_error = |cursor: &str, path: &str| {
        project_read(
            &runtime,
            "client_a",
            json!({"project_id":"P1","path":path,"max_bytes":16,"cursor":cursor}),
        )
        .unwrap_err()["error"]["code"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(read_error("nodot", "a.txt"), "OUTPUT_CURSOR_INVALID");
    assert_eq!(read_error("x.abc", "a.txt"), "OUTPUT_CURSOR_INVALID");
    assert_eq!(read_error("16.deadbeef", "a.txt"), "CONTENT_CHANGED");
    assert_eq!(
        read_error(&cursor, "c.txt"),
        "CONTENT_CHANGED",
        "a cursor issued for one file never slices another"
    );
    assert_eq!(
        cursor_offset(&runtime, None, "client_a", "P1", 0, "a.txt", "snap").unwrap(),
        0
    );

    // Search pages the same way; every page is a slice of one result set.
    for limit in [0, 101] {
        assert_eq!(
            project_search(
                &runtime,
                "client_a",
                json!({"project_id":"P1","query":"needle","max_results":limit}),
            )
            .unwrap_err()["error"]["code"],
            "INVALID_ARGUMENTS"
        );
    }
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    let mut found = Vec::new();
    loop {
        let page = project_search(
            &runtime,
            "client_a",
            json!({"project_id":"P1","query":"needle","max_results":1,"cursor":cursor}),
        )
        .unwrap();
        pages += 1;
        let matches = page["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 1, "{page}");
        found.push((
            matches[0]["path"].as_str().unwrap().to_owned(),
            matches[0]["line"].as_u64().unwrap(),
        ));
        match page["nextCursor"].as_str() {
            Some(next) => {
                assert_eq!(page["complete"], false);
                assert_eq!(page["truncated"], true);
                cursor = Some(next.to_owned());
            }
            None => {
                assert_eq!(page["complete"], true);
                assert_eq!(page["truncated"], false);
                break;
            }
        }
        assert!(pages < 10, "search paging never terminated");
    }
    assert_eq!(pages, 3);
    found.sort();
    assert_eq!(
        found,
        [
            ("c.txt".to_owned(), 1),
            ("c.txt".to_owned(), 2),
            ("sub/b.txt".to_owned(), 1)
        ]
    );
    assert_eq!(
        project_search(
            &runtime,
            "client_a",
            json!({"project_id":"P1","query":"other","max_results":1,"cursor":"0.deadbeef"}),
        )
        .unwrap_err()["error"]["code"],
        "CONTENT_CHANGED"
    );

    // A scope change between the read and its final check fails the read.
    let changed = recheck_read(&runtime, "client_a", "P1", &["/elsewhere".into()]).unwrap_err();
    assert_eq!(changed["error"]["code"], "CONTEXT_CHANGED");
    assert!(recheck_read(
        &runtime,
        "client_a",
        "P1",
        &[std::fs::canonicalize(&root).unwrap().display().to_string()]
    )
    .is_ok());
    use crate::mcp_fs::{FsError, FsErrorKind};
    for (kind, code) in [
        (FsErrorKind::Invalid, "INVALID_ARGUMENTS"),
        (FsErrorKind::Denied, "READ_DENIED"),
        (FsErrorKind::Limit, "READ_LIMIT"),
        (FsErrorKind::Changed, "CONTENT_CHANGED"),
        (FsErrorKind::Cancelled, "CONTEXT_CHANGED"),
    ] {
        let mapped = fs_error(
            FsError {
                kind,
                message: "why",
            },
            "denied next action",
        );
        assert_eq!(mapped["error"]["code"], code);
        assert_eq!(mapped["error"]["message"], "why");
    }
    std::fs::remove_dir_all(root).unwrap();
}

/// Pending Board intents, job output bindings, grants and a close in flight
/// all lose their authority when the feature is disabled or the client is
/// revoked, and `closing` never sticks on the session.
fn seed_pending_authority(runtime: &Runtime) {
    runtime
        .write(|doc| {
            let mut close = operation("session-close", "accepted");
            close.operation_id = "op_close".into();
            close.request_id = "req_close".into();
            close.result = Some(json!({"sessionId":"mcp_a"}));
            doc.operations.push(close);
            let mut exec = operation("exec", "accepted");
            exec.operation_id = "op_exec".into();
            exec.request_id = "req_exec".into();
            doc.operations.push(exec);
            doc.sessions[0].closing = true;
            doc.jobs.push(JobBinding {
                job_id: "job_a".into(),
                client_id: "client_a".into(),
                session_id: "mcp_a".into(),
                session_generation: "g_a".into(),
                request_hash: "a".repeat(64),
                operation_id: "op_exec".into(),
                grant_id: "grant_test".into(),
                grant_version: 1,
                allow_output: true,
            });
            let session = doc.sessions[0].clone();
            doc.execution_grants.push(execution_grant(&session));
            Ok(())
        })
        .unwrap();
}

fn assert_authority_fenced(runtime: &Runtime, code: &str) {
    runtime
        .read(|doc| {
            for operation in &doc.operations {
                assert_eq!(operation.state, "rejected", "{}", operation.operation_id);
                assert_eq!(operation.code.as_deref(), Some(code));
            }
            assert!(
                !doc.jobs[0].allow_output,
                "no binding reads the human's pane"
            );
            assert!(doc.execution_grants[0].revoked_at.is_some());
            let session = &doc.sessions[0];
            assert!(session.human_lock);
            assert!(!session.closing, "closing never sticks");
            assert_eq!(session.control_holder, None);
            assert_eq!(session.lease_expires_at, None);
            assert_eq!(session.control_epoch, 2);
        })
        .unwrap();
}

#[test]
fn disable_and_client_revocation_reject_pending_intents_and_close_output() {
    let (runtime, runner, root) = fixture("disable-pending", "svc_test");
    seed_pending_authority(&runtime);
    disable(&runtime).unwrap();
    assert_authority_fenced(&runtime, "feature-disabled");
    assert_eq!(runner.last("control").unwrap()["mode"], "human");
    assert!(!runtime.read(|doc| doc.config.enabled).unwrap());
    assert_eq!(
        claim(&runtime, "op_close").err().unwrap().kind(),
        ErrorKind::Perm,
        "nothing is claimable while disabled"
    );
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();

    let (runtime, runner, root) = fixture("revoke-pending", "svc_test");
    seed_pending_authority(&runtime);
    assert_eq!(
        client_revoke(&runtime, "client_missing".into())
            .unwrap_err()
            .kind(),
        ErrorKind::Missing
    );
    client_revoke(&runtime, "client_a".into()).unwrap();
    assert_authority_fenced(&runtime, "client-revoked");
    assert!(runtime
        .read(|doc| {
            doc.config.clients[0].revoked_at.is_some()
                && doc.audit.iter().any(|event| {
                    event.kind == "client-revoked"
                        && event.principal_id.as_deref() == Some("client_a")
                })
        })
        .unwrap());
    assert!(runtime
        .emergency
        .lock_or_recover()
        .clients
        .contains("client_a"));
    assert_eq!(runner.last("control").unwrap()["mode"], "human");
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

/// A close plan is admitted against the live session at the Board
/// boundary: a session that stopped closing, a fenced client, a disabled
/// feature, a foreign admission or another tmux target refuses it.
#[test]
fn close_plan_validation_and_admission_track_the_live_session() {
    let (runtime, runner, root) = fixture("close-validate", "svc_test");
    let operation_id = close_plan(&runtime);
    claim(&runtime, &operation_id).unwrap();

    let set_closing = |closing: bool| {
        runtime
            .write(|doc| {
                doc.sessions[0].closing = closing;
                Ok(())
            })
            .unwrap();
    };
    set_closing(false);
    assert_eq!(
        close_admit(&runtime, &operation_id).err().unwrap().kind(),
        ErrorKind::ContextChanged
    );
    set_closing(true);

    runtime
        .emergency
        .lock_or_recover()
        .human_sessions
        .insert("mcp_a".into());
    assert_eq!(
        close_admit(&runtime, &operation_id).err().unwrap().kind(),
        ErrorKind::ControlRevoked
    );
    runtime
        .emergency
        .lock_or_recover()
        .human_sessions
        .remove("mcp_a");

    runtime
        .write(|doc| {
            doc.config.enabled = false;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        close_admit(&runtime, &operation_id).err().unwrap().kind(),
        ErrorKind::Perm
    );
    runtime
        .write(|doc| {
            doc.config.enabled = true;
            Ok(())
        })
        .unwrap();

    assert_eq!(
        validate_close_admission_with(&runtime, Some("close_forged"), &["deck-mcp-test".into()])
            .unwrap_err()
            .kind(),
        ErrorKind::ContextChanged
    );
    let admission = close_admit(&runtime, &operation_id).unwrap().admission;
    assert_eq!(
        close_admit(&runtime, &operation_id).err().unwrap().kind(),
        ErrorKind::ContextChanged,
        "an admitted close is not admitted twice"
    );
    assert_eq!(
        validate_close_admission_with(&runtime, Some(&admission), &["deck-other".into()])
            .unwrap_err()
            .kind(),
        ErrorKind::ContextChanged
    );
    assert_eq!(
        validate_close_admission_with(
            &runtime,
            Some(&admission),
            &["deck-mcp-test".into(), "deck-other".into()]
        )
        .unwrap_err()
        .kind(),
        ErrorKind::ContextChanged
    );
    validate_close_admission_with(&runtime, None, &[]).unwrap();
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();

    // A close rejected before admission releases the session's closing flag.
    let (runtime, runner, root) = fixture("close-rejected", "svc_test");
    let operation_id = close_plan(&runtime);
    claim(&runtime, &operation_id).unwrap();
    complete(
        &runtime,
        operation_id.clone(),
        "rejected".into(),
        Some("board-refused".into()),
        None,
    )
    .unwrap();
    let session = session_state(&runtime);
    assert!(!session.closing);
    assert_eq!(
        session.session_id, "mcp_a",
        "a rejected close keeps the session"
    );
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

/// Return of control is refused by the in-memory fences, and a runner that
/// declines the new epoch re-fences the session under yet another epoch.
#[test]
fn return_control_honours_memory_fences_and_refences_when_the_runner_declines() {
    let (runtime, runner, root) = fixture("return-fences", "svc_test");
    takeover(&runtime, "mcp_a").unwrap();
    let fenced = session_state(&runtime);

    runtime.emergency.lock_or_recover().disabled = true;
    assert_eq!(
        return_control(&runtime, "M1").unwrap_err().message(),
        FEATURE_DISABLED
    );
    runtime.emergency.lock_or_recover().disabled = false;
    runtime
        .emergency
        .lock_or_recover()
        .clients
        .insert("client_a".into());
    assert_eq!(
        return_control(&runtime, "M1").unwrap_err().message(),
        CLIENT_REVOKED
    );
    runtime
        .emergency
        .lock_or_recover()
        .clients
        .remove("client_a");
    assert_eq!(session_state(&runtime).control_epoch, fenced.control_epoch);

    runner.fail_next_control();
    let declined = return_control(&runtime, "M1").unwrap_err();
    assert_eq!(declined.message(), RUNNER_UNCONFIRMED);
    let refenced = session_state(&runtime);
    assert!(
        refenced.human_lock,
        "the persisted state never claims MCP control"
    );
    assert_eq!(refenced.control_epoch, fenced.control_epoch + 2);
    let last = runner.last("control").unwrap();
    assert_eq!(last["mode"], "human");
    assert_eq!(last["control_epoch"], fenced.control_epoch + 2);
    assert!(runtime
        .emergency
        .lock_or_recover()
        .human_sessions
        .contains("mcp_a"));

    drop(runner);
    assert_eq!(
        return_control(&runtime, "M1").unwrap_err().message(),
        RUNNER_UNCONFIRMED,
        "an unreachable runner cannot confirm the return"
    );
    assert!(session_state(&runtime).human_lock);
    std::fs::remove_dir_all(root).unwrap();
}

/// A takeover whose state write fails still hands the keyboard over under a
/// higher epoch and reports the unpersisted fence.
#[test]
fn takeover_hands_the_pane_over_even_when_its_fence_cannot_be_persisted() {
    let root = test_root("takeover-unpersisted");
    let runner = FakeRunner::start(&root, "g_a");
    runner.set_control(1, "mcp", Some("holder_a"));
    let mut doc = DiskDoc::default();
    doc.config.enabled = true;
    doc.config.clients.push(client_record(&root));
    doc.sessions.push(session_record(&root, &runner));
    let runtime = Runtime {
        app: None,
        path: root.join("missing-parent/state.json"),
        socket: root.join("control.sock"),
        doc: Mutex::new(Ok(doc)),
        io: Mutex::new(()),
        delivery: Mutex::new(()),
        emergency: Mutex::new(EmergencyFences::default()),
        service_instance: "svc_test".into(),
        runner_auth: Mutex::new(HashMap::new()),
        started: Instant::now(),
    };
    let error = takeover(&runtime, "M1").unwrap_err();
    assert_eq!(error.message(), FENCE_UNPERSISTED);
    assert_eq!(
        runner.control_epoch(),
        2,
        "the runner moved to a newer epoch"
    );
    let last = runner.last("control").unwrap();
    assert_eq!(last["mode"], "human");
    assert!(last["holder_id"].is_null());
    assert!(runtime
        .emergency
        .lock_or_recover()
        .human_sessions
        .contains("mcp_a"));
    assert_eq!(
        takeover(&runtime, "M9").unwrap_err().kind(),
        ErrorKind::Missing
    );
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

macro_rules! mcp_fixture {
    ($name:literal) => {
        serde_json::from_str::<Value>(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/mcp-fixtures/",
            $name
        )))
        .unwrap()
    };
}

/// `mcp-fixtures/` is shared with the adapter and the runner: each crate
/// compares the fixture with its own constants.
#[test]
fn control_protocol_and_limits_match_the_shared_fixture() {
    assert_eq!(
        mcp_fixture!("control-protocol.json")["version"],
        CONTROL_PROTOCOL
    );
    let limits = mcp_fixture!("limits.json");
    let expected: [(&str, u64); 13] = [
        ("max_request_bytes", MAX_REQUEST_BYTES as u64),
        ("max_response_bytes", MAX_RESPONSE_BYTES as u64),
        ("max_executable", MAX_EXECUTABLE_BYTES as u64),
        ("max_arguments", MAX_ARGUMENTS as u64),
        ("max_argument_bytes", MAX_ARGUMENT_BYTES as u64),
        ("max_read_bytes", MAX_READ_BYTES as u64),
        ("max_input_bytes", MAX_INPUT_BYTES as u64),
        ("wait_ms_default", DEFAULT_WAIT_MS),
        ("wait_ms_max", MAX_WAIT_MS),
        ("lease_ms_min", MIN_LEASE_MS),
        ("lease_ms_max", MAX_LEASE_MS),
        ("output_retention_ms_min", MIN_OUTPUT_RETENTION_MS),
        ("output_retention_ms_max", MAX_OUTPUT_RETENTION_MS),
    ];
    for (key, value) in expected {
        assert_eq!(limits[key].as_u64(), Some(value), "limits.json {key}");
    }
    assert_eq!(limits["max_response_bytes_includes_newline"], true);
    assert_eq!(
        limits.as_object().unwrap().len(),
        14,
        "every fixture limit is asserted here"
    );
}

#[test]
fn runner_error_table_matches_the_shared_fixture() {
    let fixture = mcp_fixture!("runner-errors.json");
    let listed: std::collections::BTreeMap<String, String> = fixture["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                entry["error"].as_str().unwrap().to_owned(),
                entry["class"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let table: std::collections::BTreeMap<String, String> = RUNNER_ERRORS
        .iter()
        .map(|(error, class, ..)| {
            let class = match class {
                RunnerErrorClass::Rejection => "rejection",
                RunnerErrorClass::Ambiguous => "ambiguous",
            };
            ((*error).to_owned(), class.to_owned())
        })
        .collect();
    assert_eq!(
        table.len(),
        RUNNER_ERRORS.len(),
        "a runner error mapped twice"
    );
    assert_eq!(table, listed);
}

#[test]
fn runner_launch_args_use_exactly_the_fixture_flags() {
    let argv = runner_launch_args("/tmp/runner.sock", "g_a", "svc_a", 42, 60_000);
    let flags: std::collections::BTreeSet<&str> = argv
        .iter()
        .map(String::as_str)
        .filter(|arg| arg.starts_with("--"))
        .collect();
    let fixture = mcp_fixture!("runner-argv.json");
    let expected: std::collections::BTreeSet<&str> = fixture["flags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|flag| flag.as_str().unwrap())
        .collect();
    assert_eq!(flags, expected);
    assert_eq!(argv.len(), 2 * expected.len(), "one value per flag");
}

/// The control surface is closed both ways: CONTROL_TOOLS equals the shared
/// fixture (an arm added to `route` without registering it is unreachable by
/// construction), every member routes, and a name outside it is UNSUPPORTED.
#[test]
fn control_tools_are_exactly_the_fixture_and_every_one_routes() {
    let (runtime, runner, root) = fixture("fixture-tools", "svc_test");
    let tools = mcp_fixture!("tools.json");
    let listed: std::collections::BTreeSet<&str> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool.as_str().unwrap())
        .collect();
    let surface: std::collections::BTreeSet<&str> = CONTROL_TOOLS.into_iter().collect();
    assert_eq!(
        surface.len(),
        CONTROL_TOOLS.len(),
        "a control tool listed twice"
    );
    assert_eq!(surface, listed);
    for name in CONTROL_TOOLS {
        let value = route(&runtime, request(name, json!({})));
        assert_ne!(
            value["error"]["code"], "UNSUPPORTED",
            "{name} is not routed"
        );
    }
    for outside in ["deck_not_a_tool", "deck_project_delete", ""] {
        let value = route(&runtime, request(outside, json!({})));
        assert_eq!(value["error"]["code"], "UNSUPPORTED", "{outside:?}");
    }
    for tool in HUMAN_FENCED_TOOLS.iter().chain(&EXECUTION_FENCED_TOOLS) {
        assert!(
            CONTROL_TOOLS.contains(tool),
            "fence names unknown tool {tool}"
        );
    }
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

/// A runner error given before the process could start is an ordinary
/// rejection with an actionable code; one given after it may have started
/// stays ambiguous, under its own code.
#[test]
fn runner_errors_are_journaled_by_class() {
    let (runtime, runner, root) = fixture("runner-error-class", "svc_test");
    unowned(&runtime);
    grant_window(&runtime);
    let epoch = control_request(&runtime, "req_class");

    runner.fail_next_exec("spawn-failed");
    let refused = route(&runtime, exec_request("exec_spawn", epoch));
    assert_eq!(refused["error"]["code"], "SPAWN_FAILED", "{refused}");
    let record = operation_by_request(&runtime, "exec_spawn").unwrap();
    assert_eq!(record.state, "rejected");
    assert_eq!(record.code.as_deref(), Some("SPAWN_FAILED"));
    assert!(
        runtime.read(|doc| doc.jobs.is_empty()).unwrap(),
        "a rejected exec keeps no job binding"
    );

    runner.fail_next_exec("job-state-unknown");
    let unknown = route(&runtime, exec_request("exec_unknown", epoch));
    assert_eq!(unknown["error"]["code"], "JOB_STATE_UNKNOWN", "{unknown}");
    let record = operation_by_request(&runtime, "exec_unknown").unwrap();
    assert_eq!(record.state, "ambiguous");
    assert_eq!(record.code.as_deref(), Some("JOB_STATE_UNKNOWN"));

    // Retired like any finished record once its epoch moves on.
    release(&runtime, "rel_class", epoch);
    control_request(&runtime, "req_class_next");
    runtime
        .write(|doc| {
            compact(doc, Some("svc_test"));
            assert!(doc
                .operations
                .iter()
                .all(|operation| operation.request_id != "exec_spawn"));
            Ok(())
        })
        .unwrap();
    drop(runner);
    std::fs::remove_dir_all(root).unwrap();
}

/// The webview maps each local-command failure code to one sentence
/// (`pure.js` MCP_ERROR_KEYS); both are held to `ui/test/fixtures/limits.json`.
#[test]
fn local_command_error_codes_match_the_frontend_fixture() {
    let limits: Value =
        serde_json::from_str(include_str!("../../../ui/test/fixtures/limits.json")).unwrap();
    let listed: Vec<&str> = limits["mcp_local_errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|code| code.as_str().unwrap())
        .collect();
    assert_eq!(
        listed,
        [
            SESSION_BUSY,
            RUNNER_STALE,
            CLIENT_REVOKED,
            FEATURE_DISABLED,
            RUNNER_UNCONFIRMED,
            FENCE_UNPERSISTED,
        ]
    );
}
