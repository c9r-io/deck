//! Operation records: stable error codes, request identity, reservation, lapsed-lease closure and emergency fences.
//!
//! Split out of the one-file `mcp.rs` on 2026-09-23; the contract stays in
//! `mcp/mod.rs`.

use super::*;

pub(super) fn error_value(code: &str, message: &str, next_action: &str) -> Value {
    json!({"ok":false,"error":{"code":code,"message":message,"nextAction":next_action}})
}

/// Stable machine codes carried as the DeckError message on local Tauri
/// commands. The webview maps each to one localized sentence; no free text.
pub(super) const RUNNER_STALE: &str = "mcp-runner-stale";
pub(super) const SESSION_BUSY: &str = "mcp-session-busy";
pub(super) const CLIENT_REVOKED: &str = "mcp-client-revoked";
pub(super) const FEATURE_DISABLED: &str = "mcp-feature-disabled";
pub(super) const RUNNER_UNCONFIRMED: &str = "mcp-runner-unconfirmed";
pub(super) const FENCE_UNPERSISTED: &str = "mcp-fence-unpersisted";
/// A request bound to a control or create sequence that is no longer current.
pub(super) const STALE_REQUEST: &str = "mcp-stale-request";

pub(super) fn map_error(error: DeckError) -> Value {
    if error.message() == STALE_REQUEST {
        return error_value(
            "STALE_REQUEST",
            "the request names a control or create sequence that is no longer current; it was not applied",
            "Do not resubmit it unchanged. Inspect the current state (deck_session_inspect, deck_capabilities); send a NEW request with a new request_id and the current sequence only if the effect is still wanted.",
        );
    }
    if error.message() == RUNNER_STALE {
        return error_value(
            "RUNNER_STALE",
            "Deck restarted after this managed session was created",
            "Ask the local Deck user to close this session, then create a new one.",
        );
    }
    if error.kind() == ErrorKind::Perm && error.message().contains("execution grant") {
        return error_value(
            "EXECUTION_GRANT_REQUIRED",
            error.message(),
            "Ask the local Deck user to approve a short trusted-host execution window.",
        );
    }
    if error.kind() == ErrorKind::Perm && error.message().contains("stdin") {
        return error_value(
            "STDIN_NOT_AUTHORIZED",
            error.message(),
            "Ask the local Deck user to approve interactive stdin for this execution window.",
        );
    }
    let (code, next) = match error.kind() {
        ErrorKind::Perm => (
            "PERMISSION_DENIED",
            "Open Deck MCP settings and grant or restore the required scope.",
        ),
        ErrorKind::Missing | ErrorKind::NoSession => {
            ("SESSION_NOT_FOUND", "Refresh the authorized session list.")
        }
        ErrorKind::NotDir => (
            "CONTEXT_CHANGED",
            "Choose an existing authorized working directory.",
        ),
        ErrorKind::ContextChanged => (
            "CONTEXT_CHANGED",
            "Inspect the session and use its current generation and control epoch.",
        ),
        ErrorKind::ControlRevoked => (
            "CONTROL_REVOKED",
            "Stop writing, inspect the session, and wait for the local user to return control.",
        ),
        ErrorKind::RequestConflict => (
            "REQUEST_ID_CONFLICT",
            "Reuse a request id only with byte-equivalent validated arguments.",
        ),
        ErrorKind::DiskFull => (
            "CAPACITY_EXCEEDED",
            "Release control and request it again: control changes never need an ordinary slot, and the new epoch retires this session's older records (their request ids then stay rejected). A lapsed lease is closed automatically when capacity is short.",
        ),
        ErrorKind::Locked => (
            "SESSION_BUSY",
            "Inspect the active job or finish human control before retrying.",
        ),
        _ => (
            "INTERNAL_ERROR",
            "Inspect Deck status and logs, then retry only after checking the request id.",
        ),
    };
    error_value(code, error.message(), next)
}

/// Fingerprint the PARSED arguments, not the raw JSON: an optional field sent
/// as null and the same field omitted are the same request, and key order
/// never matters.
pub(super) fn request_hash<T: Serialize>(tool: &str, arguments: &T) -> String {
    let mut bytes = b"deck-request-v4\0".to_vec();
    bytes.extend(tool.as_bytes());
    bytes.push(0);
    bytes.extend(
        serde_json::to_value(arguments)
            .and_then(|value| serde_json::to_vec(&value))
            .unwrap_or_default(),
    );
    sha(&bytes)
}

/// Which journal pool a new record draws from.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Slot {
    Ordinary,
    /// `deck_job_interrupt`: the ordinary pool first, then the interrupt
    /// reserve (bounded per client).
    Interrupt,
    /// A control change. Called after the change is applied to the candidate
    /// document, so compaction has already retired what it supersedes.
    Control,
}

pub(super) fn is_control_record(operation: &Operation) -> bool {
    operation.kind == "session-control" && operation.control_sequence.is_some()
}

/// A lease that has run out can never become valid again at its epoch
/// (renew and release need a live lease; a new request opens a new epoch),
/// so under capacity pressure its epoch is closed: holder cleared, epoch
/// advanced. The records bound to it are then retired by `compact`. A
/// human-locked or closing session is left alone.
pub(super) fn close_lapsed_leases(doc: &mut DiskDoc) {
    let now = now_ms();
    for session in &mut doc.sessions {
        if session.control_owner.is_some()
            && !session.human_lock
            && !session.closing
            && session.lease_expires_at.is_none_or(|lease| lease <= now)
        {
            session.fence(FenceMode::Release);
        }
    }
}

/// Retire what can be retired, then reserve one journal slot for `client_id`
/// in `slot`'s pool. Control records never count against the ordinary pool,
/// so release/request (which retire an epoch's records) always fit.
pub(super) fn reserve_operation(
    runtime: &Runtime,
    doc: &mut DiskDoc,
    client_id: &str,
    slot: Slot,
) -> Result<(), DeckError> {
    compact(doc, Some(&runtime.service_instance));
    let fits = |doc: &DiskDoc| {
        let control = doc
            .operations
            .iter()
            .filter(|op| is_control_record(op))
            .count();
        if slot == Slot::Control {
            return control < CONTROL_RESERVE;
        }
        let others = doc.operations.len() - control;
        let own = doc
            .operations
            .iter()
            .filter(|op| op.client_id == client_id && !is_control_record(op))
            .count();
        if others < ORDINARY_OPERATIONS && own < MAX_OPERATIONS_PER_CLIENT {
            return true;
        }
        slot == Slot::Interrupt
            && others < MAX_OPERATIONS - CONTROL_RESERVE
            && doc
                .operations
                .iter()
                .filter(|op| op.client_id == client_id && op.kind == "job-interrupt")
                .count()
                < INTERRUPT_RESERVE_PER_CLIENT
    };
    if fits(doc) {
        return Ok(());
    }
    if slot != Slot::Control {
        close_lapsed_leases(doc);
        compact(doc, Some(&runtime.service_instance));
        if fits(doc) {
            return Ok(());
        }
    }
    Err(DeckError::new(
        ErrorKind::DiskFull,
        "MCP operation capacity reached",
    ))
}

/// Emergency fences are in-memory and set BEFORE the local command waits for
/// the delivery lock. Every side-effect path re-checks them while holding
/// that lock, as its last step before the side effect (runner I/O or Board
/// admission).
///
/// Linearization: a local command whose fence is visible to that check wins
/// and nothing is dispatched. A fence set after the check belongs to a local
/// command that is still waiting for the same delivery lock; it takes effect
/// only after the in-flight dispatch finished, and is ordered after it (its
/// audit event follows the dispatch's). Holding the fence mutex across runner
/// I/O would make an emergency stop wait for that I/O, so it is not done.
pub(super) fn emergency_denial(
    runtime: &Runtime,
    client_id: &str,
    session_id: &str,
    admission: Admission,
) -> Option<Value> {
    let emergency = runtime.emergency.lock_or_recover();
    if emergency.disabled || emergency.clients.contains(client_id) {
        return Some(error_value(
            "AUTH_REQUIRED",
            "MCP authority was revoked locally",
            "Reauthorize this integration locally in Deck.",
        ));
    }
    if emergency.human_sessions.contains(session_id) {
        return Some(error_value(
            "HUMAN_CONTROL",
            "local takeover has fenced remote access",
            "Wait for the local user to explicitly return control.",
        ));
    }
    if admission == Admission::Execution && emergency.execution_fenced(session_id) {
        return Some(error_value(
            "EXECUTION_GRANT_REQUIRED",
            "local execution authority was revoked",
            "Ask the local Deck user to approve a new execution window.",
        ));
    }
    None
}

/// The emergency fences as a local-command error, for Board admissions
/// (create start, close validation and admission). Checked while holding the
/// delivery lock, before the state write (never inside it: the fence mutex is
/// taken before the state lock elsewhere).
pub(super) fn emergency_fence_error(
    runtime: &Runtime,
    client_id: &str,
    session_id: &str,
) -> Option<DeckError> {
    let emergency = runtime.emergency.lock_or_recover();
    if emergency.disabled {
        return Some(DeckError::new(ErrorKind::Perm, FEATURE_DISABLED));
    }
    if emergency.clients.contains(client_id) {
        return Some(DeckError::new(ErrorKind::Perm, CLIENT_REVOKED));
    }
    if emergency.human_sessions.contains(session_id) {
        return Some(DeckError::new(
            ErrorKind::ControlRevoked,
            "local takeover has fenced remote access",
        ));
    }
    None
}

pub(super) fn existing_operation<'a>(
    doc: &'a DiskDoc,
    client_id: &str,
    request_id: &str,
    hash: &str,
) -> Result<Option<&'a Operation>, DeckError> {
    if let Some(operation) = doc
        .operations
        .iter()
        .find(|operation| operation.client_id == client_id && operation.request_id == request_id)
    {
        if operation.request_hash != hash {
            return Err(DeckError::new(
                ErrorKind::RequestConflict,
                "request id was already used with different arguments",
            ));
        }
        return Ok(Some(operation));
    }
    Ok(None)
}

/// Operation state is the control decision only; process completion and
/// output EOF are reported by `deck_job_read`. Internal plan fields (the
/// runner socket path) never leave Deck.
pub(super) fn operation_view(operation: &Operation) -> Value {
    let mut result = operation.result.clone();
    if let Some(Value::Object(fields)) = result.as_mut() {
        fields.remove("runnerSocket");
    }
    json!({
        "ok": true,
        "operationId": operation.operation_id,
        "kind": operation.kind,
        "state": operation.state,
        "code": operation.code,
        "result": result,
        "acceptedAt": operation.accepted_at,
        "updatedAt": operation.updated_at
    })
}
