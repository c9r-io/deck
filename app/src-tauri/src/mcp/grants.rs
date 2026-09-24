//! Client, project-scope and execution-grant authorization checks over the loaded document.
//!
//! Split out of the one-file `mcp.rs` on 2026-09-23; the contract stays in
//! `mcp/mod.rs`.

use super::*;

pub(super) fn runtime() -> Result<&'static Arc<Runtime>, DeckError> {
    RUNTIME
        .get()
        .ok_or_else(|| DeckError::new(ErrorKind::Other, "MCP control is not initialized"))
}

pub(super) fn runner_program() -> Result<PathBuf, DeckError> {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join("deck-mcp-runner")))
        .filter(|path| path.is_file())
        .ok_or_else(|| DeckError::new(ErrorKind::Missing, "MCP runner is not bundled"))
}

pub(super) fn canonical_scope(path: &str, roots: &[String]) -> Result<PathBuf, DeckError> {
    let path = std::fs::canonicalize(path)
        .map_err(|_| DeckError::new(ErrorKind::NotDir, "working directory is unavailable"))?;
    if !path.is_dir() {
        return Err(DeckError::new(
            ErrorKind::NotDir,
            "working directory is unavailable",
        ));
    }
    let allowed = roots.iter().any(|root| {
        std::fs::canonicalize(root)
            .ok()
            .is_some_and(|root| path.starts_with(root))
    });
    if allowed {
        Ok(path)
    } else {
        Err(DeckError::new(
            ErrorKind::Perm,
            "working directory is outside the authorized roots",
        ))
    }
}

pub(super) fn client<'a>(doc: &'a DiskDoc, client_id: &str) -> Result<&'a Client, DeckError> {
    if !doc.config.enabled {
        return Err(DeckError::new(ErrorKind::Perm, "MCP control is disabled"));
    }
    doc.config
        .clients
        .iter()
        .find(|client| client.id == client_id && client.revoked_at.is_none())
        .ok_or_else(|| DeckError::new(ErrorKind::Perm, "MCP client is not authorized"))
}

pub(super) fn scoped_project<'a>(
    client: &'a Client,
    project_id: &str,
) -> Result<&'a ProjectScope, DeckError> {
    client
        .projects
        .iter()
        .find(|project| project.project_id == project_id)
        .ok_or_else(|| DeckError::new(ErrorKind::Perm, "project is not authorized"))
}

pub(super) fn authorized_session<'a>(
    doc: &'a DiskDoc,
    client_id: &str,
    session_id: &str,
) -> Result<&'a ManagedSession, DeckError> {
    client(doc, client_id)?;
    doc.sessions
        .iter()
        .find(|session| session.session_id == session_id && session.owner_client_id == client_id)
        .ok_or_else(|| DeckError::new(ErrorKind::Missing, "session not found"))
}

pub(super) fn check_control(
    session: &ManagedSession,
    client_id: &str,
    generation: &str,
    epoch: u64,
    holder_id: &str,
) -> Result<(), DeckError> {
    let claim = ControlClaim {
        generation,
        epoch: Some(epoch),
        holder_id: Some(holder_id),
    };
    control_reason(session, client_id, &claim).map_or(Ok(()), |reason| Err(reason.error()))
}

/// What a caller claims to hold. `exec`, stdin/interrupt, renew and release
/// claim an exact generation, epoch and holder; `inspect` claims the
/// session's own generation, no epoch, and the holder it was given (if any).
pub(super) struct ControlClaim<'a> {
    pub(super) generation: &'a str,
    pub(super) epoch: Option<u64>,
    pub(super) holder_id: Option<&'a str>,
}

/// Why `exec` would refuse to start a job, in the order it checks: the
/// session generation, then control (human lock, owner, holder, epoch,
/// lease — `exec` reports all five as one `ControlRevoked`), then the
/// execution grant, then a close in progress. `inspect`'s
/// `mayStartNextJobReason` reports the same first failure, so the advisory
/// cannot drift from the real admission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AdmissionReason {
    GenerationChanged,
    HumanControl,
    ControlRevoked,
    HolderMismatch,
    EpochMismatch,
    LeaseExpired,
    GrantRequired,
    Closing,
}

impl AdmissionReason {
    /// The error `exec` (and `check_control`) returns — unchanged from
    /// before this was one function; `map_error` turns it into the client's
    /// machine code.
    pub(super) fn error(self) -> DeckError {
        match self {
            AdmissionReason::GenerationChanged => {
                DeckError::new(ErrorKind::ContextChanged, "session generation changed")
            }
            AdmissionReason::HumanControl
            | AdmissionReason::ControlRevoked
            | AdmissionReason::HolderMismatch
            | AdmissionReason::EpochMismatch
            | AdmissionReason::LeaseExpired => {
                DeckError::new(ErrorKind::ControlRevoked, "session control was revoked")
            }
            AdmissionReason::GrantRequired => {
                DeckError::new(ErrorKind::Perm, "a local execution grant is required")
            }
            AdmissionReason::Closing => DeckError::new(ErrorKind::Locked, "session is closing"),
        }
    }

    /// `inspect`'s finer `mayStartNextJobReason` word.
    pub(super) fn inspect_code(self) -> &'static str {
        match self {
            AdmissionReason::GenerationChanged => "CONTEXT_CHANGED",
            AdmissionReason::HumanControl => "HUMAN_CONTROL",
            AdmissionReason::ControlRevoked | AdmissionReason::EpochMismatch => "CONTROL_REVOKED",
            AdmissionReason::HolderMismatch => "HOLDER_MISMATCH",
            AdmissionReason::LeaseExpired => "CONTROL_LEASE_EXPIRED",
            AdmissionReason::GrantRequired => "EXECUTION_GRANT_REQUIRED",
            AdmissionReason::Closing => "TARGET_CLOSING",
        }
    }
}

fn control_reason(
    session: &ManagedSession,
    client_id: &str,
    claim: &ControlClaim<'_>,
) -> Option<AdmissionReason> {
    if session.generation != claim.generation {
        Some(AdmissionReason::GenerationChanged)
    } else if session.human_lock {
        Some(AdmissionReason::HumanControl)
    } else if session.control_owner.as_deref() != Some(client_id) {
        Some(AdmissionReason::ControlRevoked)
    } else if session.control_holder.as_deref() != claim.holder_id {
        Some(AdmissionReason::HolderMismatch)
    } else if claim
        .epoch
        .is_some_and(|epoch| epoch != session.control_epoch)
    {
        Some(AdmissionReason::EpochMismatch)
    } else if session
        .lease_expires_at
        .is_none_or(|lease| lease <= now_ms())
    {
        Some(AdmissionReason::LeaseExpired)
    } else {
        None
    }
}

/// Where in `exec` the admission runs. `Final` (the last step before the
/// runner, under the delivery lock) re-checks control and the grant but not
/// `closing` — unchanged from before this extraction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ExecStage {
    Accept,
    Final,
}

/// The ONE exec admission: `exec` accept, `exec` final admission and
/// `inspect`'s advisory all ask this. Emergency fences stay with
/// `emergency_denial` (checked by the callers around it, unchanged).
pub(super) fn admit_exec<'a>(
    runtime: &Runtime,
    doc: &'a DiskDoc,
    client_id: &str,
    session: &ManagedSession,
    claim: &ControlClaim<'_>,
    stage: ExecStage,
) -> Result<&'a ExecutionGrant, AdmissionReason> {
    if let Some(reason) = control_reason(session, client_id, claim) {
        return Err(reason);
    }
    let grant = match execution_authorization(runtime, doc, client_id, session) {
        ExecutionAuthorization::Active(grant) => grant,
        _ => return Err(AdmissionReason::GrantRequired),
    };
    if stage == ExecStage::Accept && session.closing {
        return Err(AdmissionReason::Closing);
    }
    Ok(grant)
}

#[derive(Clone, Copy)]
pub(super) enum ExecutionAuthorization<'a> {
    None,
    Active(&'a ExecutionGrant),
    Expired(&'a ExecutionGrant),
    Revoked(&'a ExecutionGrant),
}

pub(super) fn execution_authorization<'a>(
    runtime: &Runtime,
    doc: &'a DiskDoc,
    client_id: &str,
    session: &ManagedSession,
) -> ExecutionAuthorization<'a> {
    let Some(grant) = doc.execution_grants.iter().rev().find(|grant| {
        grant.client_id == client_id
            && grant.project_id == session.project_id
            && grant.session_id == session.session_id
            && grant.session_generation == session.generation
    }) else {
        return ExecutionAuthorization::None;
    };
    match grant_standing(runtime, doc, grant) {
        GrantStanding::Revoked => ExecutionAuthorization::Revoked(grant),
        GrantStanding::Expired => ExecutionAuthorization::Expired(grant),
        GrantStanding::Active => ExecutionAuthorization::Active(grant),
    }
}

/// Whether one execution grant is usable right now, however the caller
/// selected it. The ONE derivation: `execution_authorization`, the stdin
/// check in `job_side_effect`, the Session UI and `record_expired_grants` all
/// ask this and add only their own extra condition (e.g. `allow_stdin`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GrantStanding {
    Active,
    Expired,
    Revoked,
}

pub(super) fn grant_standing(
    runtime: &Runtime,
    doc: &DiskDoc,
    grant: &ExecutionGrant,
) -> GrantStanding {
    let credential = doc
        .config
        .clients
        .iter()
        .find(|client| client.id == grant.client_id)
        .map(|client| client.credential_version);
    standing_at(
        grant,
        credential,
        &runtime.service_instance,
        runtime.monotonic_ms(),
        now_ms(),
    )
}

/// Pure core of `grant_standing`: revocation (a revoked_at stamp, a
/// revocation version at or above the grant's, a client credential that
/// moved on or vanished, another Deck service instance) outranks expiry
/// (the monotonic window elapsed, the clock went backwards, or the wall
/// deadline passed).
pub(super) fn standing_at(
    grant: &ExecutionGrant,
    client_credential_version: Option<u64>,
    service_instance: &str,
    elapsed_ms: u64,
    now: u64,
) -> GrantStanding {
    if grant.revoked_at.is_some()
        || grant.grant_version <= grant.revocation_version
        || client_credential_version != Some(grant.credential_version)
        || grant.service_instance != service_instance
    {
        return GrantStanding::Revoked;
    }
    if elapsed_ms < grant.issued_monotonic_ms
        || elapsed_ms.saturating_sub(grant.issued_monotonic_ms) >= grant.duration_ms
        || now >= grant.expires_at
    {
        return GrantStanding::Expired;
    }
    GrantStanding::Active
}

// Production admission is `admit_exec`; this older form stays for the
// linearization tests in `tests.rs`, which must not change.
#[cfg(test)]
pub(super) fn active_execution_grant<'a>(
    runtime: &Runtime,
    doc: &'a DiskDoc,
    client_id: &str,
    session: &ManagedSession,
    require_stdin: bool,
) -> Result<&'a ExecutionGrant, DeckError> {
    match execution_authorization(runtime, doc, client_id, session) {
        ExecutionAuthorization::Active(grant) if !require_stdin || grant.allow_stdin => Ok(grant),
        _ => Err(DeckError::new(
            ErrorKind::Perm,
            if require_stdin {
                "interactive stdin is not locally authorized"
            } else {
                "a local execution grant is required"
            },
        )),
    }
}

pub(super) fn record_expired_grants(runtime: &Runtime) -> Result<(), DeckError> {
    let expired = runtime.read(|doc| {
        doc.execution_grants
            .iter()
            .filter(|grant| {
                grant_standing(runtime, doc, grant) == GrantStanding::Expired
                    && !doc.audit.iter().any(|event| {
                        event.kind == "grant-expired"
                            && event.grant_id.as_deref() == Some(&grant.grant_id)
                    })
            })
            .map(|grant| {
                (
                    grant.client_id.clone(),
                    grant.session_id.clone(),
                    grant.grant_id.clone(),
                )
            })
            .collect::<Vec<_>>()
    })?;
    if expired.is_empty() {
        return Ok(());
    }
    runtime.write(|doc| {
        for (principal_id, session_id, grant_id) in &expired {
            if doc.audit.iter().any(|event| {
                event.kind == "grant-expired"
                    && event.grant_id.as_deref() == Some(grant_id.as_str())
            }) {
                continue;
            }
            audit(
                doc,
                "grant-expired",
                AuditLink {
                    principal_id: Some(principal_id),
                    session_id: Some(session_id),
                    grant_id: Some(grant_id),
                    reason_code: Some("deadline-reached"),
                    ..Default::default()
                },
            )?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant() -> ExecutionGrant {
        ExecutionGrant {
            grant_id: "grant_a".into(),
            client_id: "client_a".into(),
            credential_version: 1,
            project_id: "P1".into(),
            session_id: "mcp_a".into(),
            session_generation: "g_a".into(),
            profile: "trusted-host-v1".into(),
            environment_profile: ENVIRONMENT_PROFILE.into(),
            environment_profile_version: 1,
            issued_at: 1_000,
            expires_at: 61_000,
            issued_monotonic_ms: 100,
            duration_ms: 60_000,
            allow_stdin: true,
            allow_output: true,
            _legacy_allow_shell: None,
            grant_version: 2,
            revocation_version: 1,
            service_instance: "svc".into(),
            revoked_at: None,
        }
    }

    /// The four axes of the one derivation: revocation stamp/version,
    /// client credential, Deck service instance, and time (monotonic window,
    /// clock going backwards, wall deadline). Revocation outranks expiry.
    #[test]
    fn standing_covers_every_axis_and_revocation_outranks_expiry() {
        let at =
            |grant: &ExecutionGrant,
             credential: Option<u64>,
             service: &str,
             elapsed: u64,
             now: u64| { standing_at(grant, credential, service, elapsed, now) };
        let live = grant();
        assert_eq!(at(&live, Some(1), "svc", 200, 2_000), GrantStanding::Active);

        // revocation axis
        let mut stamped = grant();
        stamped.revoked_at = Some(1_500);
        assert_eq!(
            at(&stamped, Some(1), "svc", 200, 2_000),
            GrantStanding::Revoked
        );
        let mut versioned = grant();
        versioned.revocation_version = 2;
        assert_eq!(
            at(&versioned, Some(1), "svc", 200, 2_000),
            GrantStanding::Revoked
        );
        // credential axis
        assert_eq!(
            at(&live, Some(2), "svc", 200, 2_000),
            GrantStanding::Revoked
        );
        assert_eq!(
            at(&live, None, "svc", 200, 2_000),
            GrantStanding::Revoked,
            "client gone"
        );
        // service-instance axis
        assert_eq!(
            at(&live, Some(1), "svc_old", 200, 2_000),
            GrantStanding::Revoked
        );
        // time axis
        assert_eq!(
            at(&live, Some(1), "svc", 60_100, 2_000),
            GrantStanding::Expired,
            "window elapsed"
        );
        assert_eq!(
            at(&live, Some(1), "svc", 99, 2_000),
            GrantStanding::Expired,
            "clock went backwards"
        );
        assert_eq!(
            at(&live, Some(1), "svc", 200, 61_000),
            GrantStanding::Expired,
            "wall deadline"
        );
        assert_eq!(
            at(&live, Some(1), "svc", 60_099, 60_999),
            GrantStanding::Active,
            "last usable instant"
        );
        // revocation outranks expiry on every revocation axis
        assert_eq!(
            at(&stamped, Some(1), "svc", 60_100, 61_000),
            GrantStanding::Revoked
        );
        assert_eq!(
            at(&live, Some(2), "svc", 60_100, 61_000),
            GrantStanding::Revoked
        );
        assert_eq!(
            at(&live, Some(1), "svc_old", 60_100, 61_000),
            GrantStanding::Revoked
        );
    }
}
