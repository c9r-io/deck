//! Request routing plus the 0600 control socket thread and its lifecycle.
//!
//! Split out of the one-file `mcp.rs` on 2026-09-23; the contract stays in
//! `mcp/mod.rs`.

use super::*;

pub(super) fn route(runtime: &Runtime, request: WireRequest) -> Value {
    if request.version != CONTROL_PROTOCOL {
        return error_value(
            "PROTOCOL_MISMATCH",
            "the adapter and Deck speak different control protocol versions",
            "Use the deck-mcp adapter bundled with the running Deck build.",
        );
    }
    if !valid_id(&request.client_id) || request.credential.len() > 128 {
        return error_value(
            "AUTH_REQUIRED",
            "invalid MCP control request",
            "Use the bundled deck-mcp adapter and an authorized client id.",
        );
    }
    // The socket is bound only while enabled, but a disable can race a request
    // already connected. Then every tool — including capabilities — is refused
    // with this distinct code; it discloses nothing about clients or scopes.
    let disabled = runtime.emergency.lock_or_recover().disabled
        || !runtime.read(|doc| doc.config.enabled).unwrap_or(false);
    if disabled {
        return error_value(
            "FEATURE_DISABLED",
            "MCP terminal control is disabled in Deck",
            "Ask the local Deck user to enable MCP terminal control in Settings.",
        );
    }
    let emergency_auth_blocked = runtime
        .emergency
        .lock_or_recover()
        .clients
        .contains(&request.client_id);
    let authenticated = !emergency_auth_blocked
        && runtime
            .read(|doc| {
                doc.config.enabled
                    && doc.config.clients.iter().any(|client| {
                        client.id == request.client_id
                            && client.revoked_at.is_none()
                            && client.credential_version > 0
                            && secret_hash_matches(
                                &client.credential_hash,
                                &sha(request.credential.as_bytes()),
                            )
                    })
            })
            .unwrap_or(false);
    if !authenticated {
        return error_value(
            "AUTH_REQUIRED",
            "MCP credential is invalid or revoked",
            "Reauthorize this integration locally in Deck.",
        );
    }
    let request_session = request
        .arguments
        .get("session_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            let job_id = request.arguments.get("job_id")?.as_str()?;
            runtime
                .read(|doc| {
                    doc.jobs
                        .iter()
                        .find(|job| job.job_id == job_id && job.client_id == request.client_id)
                        .map(|job| job.session_id.clone())
                })
                .ok()
                .flatten()
        });
    if let Some(session_id) = request_session.as_deref() {
        let emergency = runtime.emergency.lock_or_recover();
        if emergency.human_sessions.contains(session_id)
            && matches!(
                request.tool.as_str(),
                "deck_session_control"
                    | "deck_exec"
                    | "deck_job_read"
                    | "deck_job_input"
                    | "deck_session_close"
            )
        {
            return error_value(
                "HUMAN_CONTROL",
                "local takeover has fenced remote access",
                "Wait for the local user to explicitly return control.",
            );
        }
        if emergency.execution_fenced(session_id)
            && matches!(request.tool.as_str(), "deck_exec" | "deck_job_input")
        {
            return error_value(
                "EXECUTION_GRANT_REQUIRED",
                "local execution authority was revoked",
                "Ask the local Deck user to approve a new execution window.",
            );
        }
    }
    let audit_tool = request.tool.clone();
    let audit_principal = request.client_id.clone();
    let result = match request.tool.as_str() {
        "deck_capabilities" => capabilities(runtime, &request.client_id, request.arguments),
        "deck_project_list" => project_list(runtime, &request.client_id, request.arguments),
        "deck_project_read" => project_read(runtime, &request.client_id, request.arguments),
        "deck_project_search" => project_search(runtime, &request.client_id, request.arguments),
        "deck_sessions_list" => sessions_list(runtime, &request.client_id, request.arguments),
        "deck_session_create" => session_create(runtime, &request.client_id, request.arguments),
        "deck_operation_get" => operation_get(runtime, &request.client_id, request.arguments),
        "deck_session_inspect" => inspect(runtime, &request.client_id, request.arguments),
        "deck_session_control" => session_control(runtime, &request.client_id, request.arguments),
        "deck_exec" => exec(runtime, &request.client_id, request.arguments),
        "deck_job_read" => job_read(runtime, &request.client_id, request.arguments),
        "deck_job_input" | "deck_job_interrupt" => job_side_effect(
            runtime,
            &request.client_id,
            &request.tool,
            request.arguments,
        ),
        "deck_session_close" => session_close(runtime, &request.client_id, request.arguments),
        _ => Err(error_value(
            "UNSUPPORTED",
            "unknown Deck MCP tool",
            "Refresh tools/list and use an advertised tool.",
        )),
    };
    if let Err(error) = &result {
        let kind = if audit_tool.starts_with("deck_project_") {
            Some("read-denied")
        } else if matches!(
            audit_tool.as_str(),
            "deck_exec" | "deck_job_input" | "deck_session_create" | "deck_session_close"
        ) {
            Some("request-denied")
        } else {
            None
        };
        if let Some(kind) = kind {
            let reason = error
                .get("error")
                .and_then(|value| value.get("code"))
                .and_then(Value::as_str)
                .unwrap_or("rejected");
            let _ = runtime.write(|doc| {
                audit(
                    doc,
                    kind,
                    AuditLink {
                        principal_id: Some(&audit_principal),
                        reason_code: Some(reason),
                        ..Default::default()
                    },
                )
            });
        }
    }
    result.unwrap_or_else(|error| error)
}

#[cfg(target_os = "macos")]
pub(super) fn same_uid(stream: &UnixStream) -> bool {
    let mut uid = 0;
    let mut gid = 0;
    // SAFETY: getpeereid writes two scalar outputs for this connected socket.
    unsafe {
        libc::getpeereid(std::os::fd::AsRawFd::as_raw_fd(stream), &mut uid, &mut gid) == 0
            && uid == libc::geteuid()
    }
}

#[cfg(not(target_os = "macos"))]
pub(super) fn same_uid(_stream: &UnixStream) -> bool {
    false
}

/// Live control connections; excess connections are closed immediately.
pub(super) static CONNECTIONS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
/// Serializes bind/remove decisions with enable and disable. The listener
/// itself remains owned by the one control thread.
pub(super) static SOCKET_LIFECYCLE: Mutex<()> = Mutex::new(());
/// Wakes the control thread when MCP is enabled. While disabled the thread
/// waits here (a long backstop timeout only) rather than polling every tick.
pub(super) static CONTROL_WAKE: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());
/// Backstop re-check interval for the idle control thread.
pub(super) const DISABLED_IDLE: Duration = Duration::from_secs(60);
/// Longest the enabled control thread blocks waiting for a connection before
/// it re-runs the socket lifecycle check.
pub(super) const ENABLED_RECHECK: Duration = Duration::from_secs(1);

pub(super) fn wake_control_thread() {
    let (flag, wake) = &CONTROL_WAKE;
    *flag.lock_or_recover() = true;
    wake.notify_all();
}

pub(super) fn wait_for_enable() {
    let (flag, wake) = &CONTROL_WAKE;
    let mut woken = flag.lock_or_recover();
    if !*woken {
        woken = match wake.wait_timeout(woken, DISABLED_IDLE) {
            Ok((guard, _)) => guard,
            Err(poisoned) => poisoned.into_inner().0,
        };
    }
    *woken = false;
}

/// Sleep in the kernel until the listener has a pending connection, or for
/// at most `ENABLED_RECHECK` so the thread still re-runs the lifecycle check
/// (disable already unlinks the socket path itself, under `SOCKET_LIFECYCLE`).
pub(super) fn wait_for_connection(listener: &UnixListener) {
    let mut fd = libc::pollfd {
        fd: std::os::fd::AsRawFd::as_raw_fd(listener),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: `fd` is one valid pollfd for a listener this thread owns.
    unsafe { libc::poll(&mut fd, 1, ENABLED_RECHECK.as_millis() as libc::c_int) };
}

/// One newline-terminated request of at most `MAX_REQUEST_BYTES`, complete
/// within `REQUEST_DEADLINE`. Each read waits only for the time that is left,
/// so a peer trickling bytes cannot extend the deadline.
pub(super) fn read_request_line(stream: &mut UnixStream) -> Option<Vec<u8>> {
    let deadline = Instant::now() + REQUEST_DEADLINE;
    let mut line = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())?;
        stream.set_read_timeout(Some(remaining)).ok()?;
        let count = match stream.read(&mut buffer) {
            Ok(0) => return None,
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return None,
        };
        let chunk = &buffer[..count];
        if let Some(end) = chunk.iter().position(|byte| *byte == b'\n') {
            line.extend_from_slice(&chunk[..=end]);
            return (line.len() <= MAX_REQUEST_BYTES).then_some(line);
        }
        line.extend_from_slice(chunk);
        if line.len() > MAX_REQUEST_BYTES {
            return None;
        }
    }
}

pub(super) fn handle_connection(runtime: Arc<Runtime>, mut stream: UnixStream) {
    if !same_uid(&stream) {
        return;
    }
    // macOS hands the listener's O_NONBLOCK to accepted sockets; without
    // switching back, the timeouts below are inert and any request or
    // response larger than one socket buffer fails with WouldBlock.
    // A same-uid peer that connects and never sends a full request holds its
    // slot for at most `REQUEST_DEADLINE`; one that never reads its response
    // is bounded by the write timeout.
    if stream.set_nonblocking(false).is_err()
        || stream.set_write_timeout(Some(CONNECTION_TIMEOUT)).is_err()
    {
        return;
    }
    let response = match read_request_line(&mut stream) {
        Some(line) => serde_json::from_slice::<WireRequest>(&line)
            .map(|request| route(&runtime, request))
            .unwrap_or_else(|_| {
                error_value(
                    "INVALID_ARGUMENTS",
                    "invalid local control request",
                    "Use the bundled deck-mcp adapter.",
                )
            }),
        None => error_value(
            "INVALID_ARGUMENTS",
            "invalid local control request",
            "Use the bundled deck-mcp adapter.",
        ),
    };
    if let Ok(mut bytes) = serde_json::to_vec(&response) {
        if bytes.len() > MAX_RESPONSE_BYTES {
            bytes = serde_json::to_vec(&error_value(
                "INTERNAL_ERROR",
                "response exceeded its bound",
                "Use a smaller read request.",
            ))
            .unwrap_or_default();
        }
        let _ = stream.write_all(&bytes);
        let _ = stream.write_all(b"\n");
        let _ = stream.flush();
    }
}

pub(super) fn bind_private_socket(socket: &Path) -> Result<UnixListener, DeckError> {
    let parent = socket
        .parent()
        .ok_or_else(|| DeckError::new(ErrorKind::Invalid, "MCP socket path is invalid"))?;
    // The private Deck directory and single-instance lock make this name
    // exclusive; keeping it short also preserves macOS SUN_LEN headroom.
    let temporary = parent.join(".mcp-bind");
    let _ = std::fs::remove_file(&temporary);
    let listener = UnixListener::bind(&temporary).map_err(DeckError::from)?;
    if let Err(error) = std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))
        .and_then(|_| std::fs::rename(&temporary, socket))
    {
        let _ = std::fs::remove_file(&temporary);
        return Err(DeckError::from(error));
    }
    Ok(listener)
}

pub(super) fn socket_should_listen(runtime: &Runtime) -> bool {
    runtime.read(|doc| doc.config.enabled).unwrap_or(false)
        && !runtime.emergency.lock_or_recover().disabled
}

pub(super) fn reconcile_control_socket(
    runtime: &Runtime,
    listener: &mut Option<UnixListener>,
) -> Result<(), DeckError> {
    let _lifecycle = SOCKET_LIFECYCLE.lock_or_recover();
    if !socket_should_listen(runtime) {
        *listener = None;
        match std::fs::remove_file(&runtime.socket) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(DeckError::from(error)),
        }
        return Ok(());
    }
    if listener.is_some() {
        return Ok(());
    }
    let _ = std::fs::remove_file(&runtime.socket);
    let bound = bind_private_socket(&runtime.socket)?;
    bound.set_nonblocking(true).map_err(DeckError::from)?;
    *listener = Some(bound);
    Ok(())
}

pub(crate) fn spawn(app: AppHandle) {
    let dir = crate::datadir::deck_dir();
    let path = dir.join("mcp.json");
    let socket = dir.join("mcp-control.sock");
    let runtime = Arc::new(Runtime {
        app: Some(app),
        path: path.clone(),
        socket: socket.clone(),
        doc: Mutex::new(load(&path)),
        io: Mutex::new(()),
        delivery: Mutex::new(()),
        emergency: Mutex::new(EmergencyFences::default()),
        service_instance: random_id("svc_").unwrap_or_else(|_| "svc_unavailable".into()),
        runner_auth: Mutex::new(HashMap::new()),
        started: Instant::now(),
    });
    let _ = RUNTIME.set(runtime.clone());
    std::thread::Builder::new()
        .name("deck-mcp-control".into())
        .spawn(move || {
            let mut listener = None;
            loop {
                let _ = reconcile_control_socket(&runtime, &mut listener);
                if let Some(bound) = &listener {
                    match bound.accept() {
                        Ok((stream, _)) => {
                            use std::sync::atomic::Ordering;
                            if CONNECTIONS.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                                CONNECTIONS.fetch_sub(1, Ordering::SeqCst);
                                continue;
                            }
                            let runtime = runtime.clone();
                            std::thread::spawn(move || {
                                handle_connection(runtime, stream);
                                CONNECTIONS.fetch_sub(1, Ordering::SeqCst);
                            });
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            wait_for_connection(bound);
                        }
                        Err(_) => listener = None,
                    }
                } else if socket_should_listen(&runtime) {
                    // enabled but the bind failed: retry on the slow tick
                    std::thread::sleep(Duration::from_secs(1));
                } else {
                    wait_for_enable();
                }
            }
        })
        .ok();
}
