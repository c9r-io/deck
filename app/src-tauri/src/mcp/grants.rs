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
    if session.generation != generation {
        return Err(DeckError::new(
            ErrorKind::ContextChanged,
            "session generation changed",
        ));
    }
    if session.human_lock
        || session.control_owner.as_deref() != Some(client_id)
        || session.control_holder.as_deref() != Some(holder_id)
        || session.control_epoch != epoch
        || session
            .lease_expires_at
            .is_none_or(|lease| lease <= now_ms())
    {
        return Err(DeckError::new(
            ErrorKind::ControlRevoked,
            "session control was revoked",
        ));
    }
    Ok(())
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
