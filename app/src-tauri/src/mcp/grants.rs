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
    let elapsed = runtime.monotonic_ms();
    let Some(grant) = doc.execution_grants.iter().rev().find(|grant| {
        grant.client_id == client_id
            && grant.project_id == session.project_id
            && grant.session_id == session.session_id
            && grant.session_generation == session.generation
    }) else {
        return ExecutionAuthorization::None;
    };
    if grant.revoked_at.is_some() || grant.grant_version <= grant.revocation_version {
        return ExecutionAuthorization::Revoked(grant);
    }
    let credential_current = doc
        .config
        .clients
        .iter()
        .find(|client| client.id == client_id)
        .is_some_and(|client| client.credential_version == grant.credential_version);
    if !credential_current || grant.service_instance != runtime.service_instance {
        return ExecutionAuthorization::Revoked(grant);
    }
    if elapsed < grant.issued_monotonic_ms
        || elapsed.saturating_sub(grant.issued_monotonic_ms) >= grant.duration_ms
        || now_ms() >= grant.expires_at
    {
        return ExecutionAuthorization::Expired(grant);
    }
    ExecutionAuthorization::Active(grant)
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
    let wall = now_ms();
    let elapsed = runtime.monotonic_ms();
    let expired = runtime.read(|doc| {
        doc.execution_grants
            .iter()
            .filter(|grant| {
                grant.service_instance == runtime.service_instance
                    && grant.revoked_at.is_none()
                    && (wall >= grant.expires_at
                        || elapsed.saturating_sub(grant.issued_monotonic_ms) >= grant.duration_ms)
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
