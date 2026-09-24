//! The pane runner's private control channel: key exchange, peer checks, probes and control requests.
//!
//! Split out of the one-file `mcp.rs` on 2026-09-23; the contract stays in
//! `mcp/mod.rs`.

use super::*;

#[derive(Clone)]
pub(super) struct RunnerAuth {
    pub(super) key: String,
    pub(super) pid: libc::pid_t,
}

/// The runner socket directory is writable by the pane's own jobs (same
/// uid), so its path proves nothing: a job could move the socket aside and
/// listen in its place to collect the key. Every exchange, the claim
/// included, therefore first checks through the kernel that the listening
/// peer is the process tmux started as the pane (`#{pane_pid}`, resolved
/// once at claim and kept with the key); nothing is written to another peer.
pub(super) fn runner_exchange(
    socket: &str,
    generation: &str,
    runner_pid: libc::pid_t,
    request: &Value,
) -> Result<Value, DeckError> {
    let mut stream = UnixStream::connect(socket)
        .map_err(|_| DeckError::new(ErrorKind::Missing, "managed runner is unavailable"))?;
    if peer_pid(&stream) != Some(runner_pid) {
        return Err(DeckError::new(
            ErrorKind::Perm,
            "managed runner socket is not served by its pane",
        ));
    }
    stream
        .set_read_timeout(Some(Duration::from_secs(7)))
        .map_err(DeckError::from)?;
    stream
        .set_write_timeout(Some(Duration::from_secs(7)))
        .map_err(DeckError::from)?;
    let mut frame = serde_json::to_vec(request)
        .map_err(|_| DeckError::new(ErrorKind::Other, "runner request encoding failed"))?;
    frame.push(b'\n');
    if frame.len() > MAX_REQUEST_BYTES {
        return Err(DeckError::new(
            ErrorKind::Invalid,
            "encoded runner request exceeds its bound",
        ));
    }
    stream.write_all(&frame).map_err(DeckError::from)?;
    stream.flush().map_err(DeckError::from)?;
    let mut bytes = Vec::new();
    BufReader::new(stream)
        .take((MAX_RESPONSE_BYTES + 1) as u64)
        .read_until(b'\n', &mut bytes)
        .map_err(DeckError::from)?;
    if bytes.len() > MAX_RESPONSE_BYTES || !bytes.ends_with(b"\n") {
        return Err(DeckError::new(
            ErrorKind::Other,
            "runner response exceeded its bound",
        ));
    }
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| DeckError::new(ErrorKind::Other, "runner response is invalid"))?;
    if value.get("generation").and_then(Value::as_str) != Some(generation) {
        return Err(DeckError::new(
            ErrorKind::ContextChanged,
            "managed runner generation changed",
        ));
    }
    Ok(value)
}

pub(super) fn runner_auth(
    runtime: &Runtime,
    session: &ManagedSession,
) -> Result<RunnerAuth, DeckError> {
    let mut keys = runtime.runner_auth.lock_or_recover();
    if let Some(auth) = keys.get(&session.runner_socket) {
        return Ok(auth.clone());
    }
    let pid = runner_pane_pid(&session.tmux_session)?;
    let response = runner_exchange(
        &session.runner_socket,
        &session.generation,
        pid,
        &json!({
            "kind":"claim",
            "service_instance":runtime.service_instance,
            "generation":session.generation,
        }),
    )?;
    let key = response
        .get("auth")
        .and_then(Value::as_str)
        .filter(|encoded| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(encoded)
                .is_ok_and(|bytes| bytes.len() == 32)
        })
        .ok_or_else(|| DeckError::new(ErrorKind::Perm, "managed runner authentication failed"))?
        .to_owned();
    let auth = RunnerAuth { key, pid };
    keys.insert(session.runner_socket.clone(), auth.clone());
    Ok(auth)
}

/// The runner is the pane's own process: tmux execs a multi-argument pane
/// command directly, without a shell.
#[cfg(not(test))]
pub(super) fn runner_pane_pid(tmux_session: &str) -> Result<libc::pid_t, DeckError> {
    let target = crate::tmux::pane_target(tmux_session);
    crate::tmux::tmux(&["display-message", "-p", "-t", &target, "#{pane_pid}"])?
        .trim()
        .parse::<libc::pid_t>()
        .ok()
        .filter(|pid| *pid > 0)
        .ok_or_else(|| DeckError::new(ErrorKind::Missing, "managed runner pane is unavailable"))
}

/// Unit tests serve fake runners from this process.
#[cfg(test)]
pub(super) fn runner_pane_pid(_tmux_session: &str) -> Result<libc::pid_t, DeckError> {
    Ok(std::process::id() as libc::pid_t)
}

/// `procinfo::peer_pid`, in this module's pid type.
pub(super) fn peer_pid(stream: &UnixStream) -> Option<libc::pid_t> {
    crate::procinfo::peer_pid(stream).map(|pid| pid as libc::pid_t)
}

pub(super) fn send_runner(
    runtime: &Runtime,
    session: &ManagedSession,
    request: &Value,
) -> Result<Value, DeckError> {
    let auth = runner_auth(runtime, session)?;
    let mut authenticated = request.clone();
    authenticated
        .as_object_mut()
        .ok_or_else(|| DeckError::new(ErrorKind::Invalid, "runner request must be an object"))?
        .insert("auth".into(), Value::String(auth.key));
    runner_exchange(
        &session.runner_socket,
        &session.generation,
        auth.pid,
        &authenticated,
    )
}

pub(super) fn send_runner_control(
    runtime: &Runtime,
    session: &ManagedSession,
    mode: &str,
) -> Result<Value, DeckError> {
    send_runner(
        runtime,
        session,
        &json!({
            "kind": "control",
            "mode": mode,
            "service_instance": runtime.service_instance,
            "control_epoch": session.control_epoch,
            "holder_id": session.control_holder,
        }),
    )
}

/// Ping a runner and learn whether it still belongs to this Deck process.
/// `None` means unreachable or unparseable.
pub(super) struct RunnerProbe {
    pub(super) current: bool,
    pub(super) job: Value,
    pub(super) version: Option<String>,
}

pub(super) fn probe_runner(runtime: &Runtime, session: &ManagedSession) -> Option<RunnerProbe> {
    let value = send_runner(
        runtime,
        session,
        &json!({"kind":"ping","service_instance":runtime.service_instance}),
    )
    .ok()?;
    if value.get("ok").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    Some(RunnerProbe {
        // A runner that predates `serviceCurrent` cannot prove it is current.
        current: value.get("serviceCurrent").and_then(Value::as_bool) == Some(true),
        job: value.get("job").cloned().unwrap_or(Value::Null),
        version: value
            .get("runnerVersion")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

/// Classify a runner control reply for local commands.
pub(super) fn runner_control_result(response: Result<Value, DeckError>) -> Result<(), DeckError> {
    match response {
        Ok(value) if value.get("ok").and_then(Value::as_bool) == Some(true) => Ok(()),
        Ok(value) => Err(match value.get("error").and_then(Value::as_str) {
            Some("runner-stale") => DeckError::new(ErrorKind::ContextChanged, RUNNER_STALE),
            Some("session-busy") => DeckError::new(ErrorKind::Locked, SESSION_BUSY),
            _ => DeckError::new(ErrorKind::ContextChanged, RUNNER_UNCONFIRMED),
        }),
        Err(_) => Err(DeckError::new(
            ErrorKind::ContextChanged,
            RUNNER_UNCONFIRMED,
        )),
    }
}

pub(super) fn runner_socket_matches(
    runtime: &Runtime,
    tmux_session: &str,
    socket: &str,
    generation: &str,
) -> bool {
    let session = ManagedSession {
        session_id: String::new(),
        card_id: String::new(),
        tmux_session: tmux_session.into(),
        project_id: String::new(),
        title: String::new(),
        cwd: String::new(),
        generation: generation.into(),
        runner_socket: socket.into(),
        owner_client_id: String::new(),
        control_owner: None,
        control_holder: None,
        control_epoch: 0,
        lease_expires_at: None,
        human_lock: true,
        output_shared: false,
        closing: false,
        created_at: 0,
        control_sequence: 0,
    };
    send_runner(runtime, &session, &json!({"kind":"ping"}))
        .ok()
        .is_some_and(|value| value.get("ok").and_then(Value::as_bool) == Some(true))
}

/// How Deck journals a runner error. `Rejection`: the runner answered before
/// any job process could start or any input byte could be written, so the
/// request had no effect and its record is `rejected`. `Ambiguous`: a process
/// may already have started (or bytes been written) and the runner cannot
/// prove otherwise, so the record is `ambiguous` — with the error's own code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RunnerErrorClass {
    Rejection,
    Ambiguous,
}

/// Every error string the runner can emit: (runner error, class, client
/// code, next action). `mcp-fixtures/runner-errors.json` lists the same set
/// with the same classes, and the runner's `RunnerError::ALL` is held to it.
pub(super) const RUNNER_ERRORS: &[(&str, RunnerErrorClass, &str, &str)] = {
    use RunnerErrorClass::{Ambiguous, Rejection};
    &[
        (
            "session-busy",
            Rejection,
            "SESSION_BUSY",
            "Read or interrupt the active job before retrying.",
        ),
        (
            "runner-stale",
            Rejection,
            "RUNNER_STALE",
            "Deck restarted after this session was created; ask the local user to close it and create a new session.",
        ),
        (
            "authentication-failed",
            Rejection,
            "RUNNER_AUTHENTICATION_FAILED",
            "Ask the local user to close this session and create a new one.",
        ),
        (
            "control-revoked",
            Rejection,
            "CONTROL_REVOKED",
            "Inspect control state and wait for the local user to return control.",
        ),
        (
            "dispatch-context-invalid",
            Rejection,
            "CONTROL_REVOKED",
            "Acquire current control and submit a new request under the active service context.",
        ),
        (
            "holder-conflict",
            Rejection,
            "CONTROL_REVOKED",
            "Acquire current control and submit a new request under the active service context.",
        ),
        (
            "job-not-found",
            Rejection,
            "JOB_NOT_FOUND",
            "Refresh the authorized job state.",
        ),
        (
            "job-not-running",
            Rejection,
            "JOB_NOT_RUNNING",
            "Read the job; input and interrupt are accepted only while it is running.",
        ),
        (
            "request-id-conflict",
            Rejection,
            "REQUEST_ID_CONFLICT",
            "Do not reuse the identifier with different arguments.",
        ),
        (
            "capacity-exceeded",
            Rejection,
            "CAPACITY_EXCEEDED",
            "Close old managed sessions before retrying.",
        ),
        (
            "invalid-cwd",
            Rejection,
            "INVALID_ARGUMENTS",
            "Correct the rejected arguments before retrying.",
        ),
        (
            "invalid-executable",
            Rejection,
            "INVALID_ARGUMENTS",
            "Correct the rejected arguments before retrying.",
        ),
        (
            "invalid-request",
            Rejection,
            "INVALID_ARGUMENTS",
            "Correct the rejected arguments before retrying.",
        ),
        (
            "spawn-failed",
            Rejection,
            "SPAWN_FAILED",
            "Nothing ran. Check that the executable exists and the logged-in user may run it, then submit a new request.",
        ),
        (
            "output-cursor-invalid",
            Rejection,
            "OUTPUT_CURSOR_INVALID",
            "Restart reading with no cursor; a gap may be reported.",
        ),
        (
            "internal-error",
            Rejection,
            "RUNNER_INTERNAL_ERROR",
            "The runner did not read the request and nothing ran; submit it again with a new request_id.",
        ),
        (
            "job-state-unknown",
            Ambiguous,
            "JOB_STATE_UNKNOWN",
            "Read the job and operation before deciding what to do next; do not re-execute it.",
        ),
        (
            "stop-unconfirmed",
            Ambiguous,
            "STOP_UNCONFIRMED",
            "Read the job; its process may still be running.",
        ),
        (
            "response-too-large",
            Ambiguous,
            "RESPONSE_TOO_LARGE",
            "Read the existing job again with a smaller max_bytes value; do not re-execute it.",
        ),
    ]
};

/// The client code and next action for a runner error reply, with its
/// class; `None` for a success, a non-runner reply or an unknown string
/// (callers treat that as ambiguous).
pub(super) fn runner_error(
    value: &Value,
) -> Option<(RunnerErrorClass, &'static str, &'static str)> {
    let error = value.get("error").and_then(Value::as_str)?;
    RUNNER_ERRORS
        .iter()
        .find(|(name, ..)| *name == error)
        .map(|(_, class, code, next)| (*class, *code, *next))
}
