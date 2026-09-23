//! Local Tauri commands: status, enable/disable, clients, grants, pending intents, takeover, guards.
//!
//! Split out of the one-file `mcp.rs` on 2026-09-23; the contract stays in
//! `mcp/mod.rs`.

use super::*;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StatusView {
    pub(super) enabled: bool,
    pub(super) socket_ready: bool,
    pub(super) clients: Vec<ClientView>,
    pub(super) output_retention_ms: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClientView {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) revoked: bool,
    pub(super) allow_create: bool,
    pub(super) projects: Vec<ProjectScope>,
}

#[tauri::command]
pub(crate) fn mcp_status() -> Result<StatusView, DeckError> {
    let runtime = runtime()?;
    let emergency = runtime.emergency.lock_or_recover();
    runtime.read(|doc| StatusView {
        enabled: doc.config.enabled && !emergency.disabled,
        socket_ready: runtime.socket.exists(),
        output_retention_ms: doc.config.output_retention_ms,
        clients: doc
            .config
            .clients
            .iter()
            .map(|client| ClientView {
                id: client.id.clone(),
                name: client.name.clone(),
                revoked: client.revoked_at.is_some() || emergency.clients.contains(&client.id),
                allow_create: client.allow_create,
                projects: client.projects.clone(),
            })
            .collect(),
    })
}

#[tauri::command]
pub(crate) fn mcp_output_retention(duration_ms: u64) -> Result<(), DeckError> {
    if !(60_000..=MAX_OUTPUT_RETENTION_MS).contains(&duration_ms) {
        return Err(DeckError::new(
            ErrorKind::Invalid,
            "MCP output retention must be between one minute and seven days",
        ));
    }
    let runtime = runtime()?;
    let _delivery = runtime.delivery.lock_or_recover();
    let sessions = runtime.write(|doc| {
        doc.config.output_retention_ms = duration_ms;
        Ok(doc.sessions.clone())
    })?;
    let mut uncertain = false;
    for session in sessions {
        let applied = send_runner(
            runtime,
            &session,
            &json!({"kind":"retention","service_instance":runtime.service_instance,"output_retention_ms":duration_ms}),
        )
        .ok()
        .is_some_and(|value| value.get("ok").and_then(Value::as_bool) == Some(true));
        uncertain |= !applied;
    }
    if uncertain {
        return Err(DeckError::new(
            ErrorKind::ContextChanged,
            "retention is saved for new runners but one or more live runners did not confirm it",
        ));
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn mcp_adapter_path() -> Result<String, DeckError> {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join("deck-mcp")))
        .filter(|path| path.is_file())
        .map(|path| path.display().to_string())
        .ok_or_else(|| DeckError::new(ErrorKind::Missing, "MCP adapter is not bundled"))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ProjectScopeInput {
    pub(super) project_id: String,
    pub(super) roots: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScopePreview {
    pub(super) ok: bool,
    pub(super) root: Option<String>,
    pub(super) error: Option<&'static str>,
}

pub(super) fn canonical_project_root(root: &str) -> Result<String, DeckError> {
    canonical_project_root_with_access(root, |path| std::fs::read_dir(path).map(|_| ()))
}

pub(super) fn canonical_project_root_with_access(
    root: &str,
    access: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<String, DeckError> {
    let path = std::fs::canonicalize(root).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => {
            DeckError::new(ErrorKind::Missing, "authorized root does not exist")
        }
        std::io::ErrorKind::PermissionDenied => {
            DeckError::new(ErrorKind::Perm, "authorized root is not accessible")
        }
        std::io::ErrorKind::NotADirectory => {
            DeckError::new(ErrorKind::NotDir, "authorized root is not a directory")
        }
        _ => DeckError::new(ErrorKind::Other, "authorized root could not be resolved"),
    })?;
    if !path.is_dir() {
        return Err(DeckError::new(
            ErrorKind::NotDir,
            "authorized root is not a directory",
        ));
    }
    access(&path).map_err(|error| match error.kind() {
        std::io::ErrorKind::PermissionDenied => {
            DeckError::new(ErrorKind::Perm, "authorized root is not accessible")
        }
        std::io::ErrorKind::NotFound => {
            DeckError::new(ErrorKind::Missing, "authorized root does not exist")
        }
        std::io::ErrorKind::NotADirectory => {
            DeckError::new(ErrorKind::NotDir, "authorized root is not a directory")
        }
        _ => DeckError::new(ErrorKind::Other, "authorized root could not be inspected"),
    })?;
    // `/`, the account home, its ancestors and excluded directories would turn
    // a project scope into account-wide structured reads.
    crate::mcp_fs::root_policy(&path)
        .map_err(|error| DeckError::new(ErrorKind::Invalid, error.message))?;
    Ok(path.display().to_string())
}

#[tauri::command]
pub(crate) fn mcp_scope_preview(root: String) -> ScopePreview {
    match canonical_project_root(&root) {
        Ok(root) => ScopePreview {
            ok: true,
            root: Some(root),
            error: None,
        },
        Err(error) => ScopePreview {
            ok: false,
            root: None,
            error: Some(match error.kind() {
                ErrorKind::Missing => "not_found",
                ErrorKind::NotDir => "not_directory",
                ErrorKind::Perm => "not_accessible",
                ErrorKind::Invalid => "too_broad",
                _ => "unavailable",
            }),
        },
    }
}

#[tauri::command]
pub(crate) fn mcp_enable() -> Result<(), DeckError> {
    let runtime = runtime()?;
    {
        let _lifecycle = SOCKET_LIFECYCLE.lock_or_recover();
        runtime.write(|doc| {
            doc.config.enabled = true;
            Ok(())
        })?;
        runtime.emergency.lock_or_recover().disabled = false;
    }
    wake_control_thread();
    Ok(())
}

/// Disable fences every client in memory BEFORE waiting for the delivery lock,
/// then persists and hands every managed pane to the local user (human mode:
/// the keyboard and the ^C stop key reach the job; MCP reaches nothing).
#[tauri::command(async)]
pub(crate) fn mcp_disable() -> Result<(), DeckError> {
    disable(runtime()?)
}

pub(super) fn disable(runtime: &Runtime) -> Result<(), DeckError> {
    runtime.emergency.lock_or_recover().disabled = true;
    {
        let _lifecycle = SOCKET_LIFECYCLE.lock_or_recover();
        match std::fs::remove_file(&runtime.socket) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(DeckError::from(error)),
        }
    }
    let _delivery = runtime.delivery.lock_or_recover();
    let sessions = runtime.write(|doc| {
        doc.config.enabled = false;
        for grant in &mut doc.execution_grants {
            if grant.revoked_at.is_none() {
                grant.revoked_at = Some(now_ms());
                grant.revocation_version = grant.grant_version;
            }
        }
        let closing = doc
            .operations
            .iter()
            .filter(|operation| operation.state == "accepted" && operation.kind == "session-close")
            .filter_map(|operation| operation.result.as_ref()?.get("sessionId")?.as_str())
            .map(str::to_owned)
            .collect::<HashSet<_>>();
        for session in &mut doc.sessions {
            session.control_owner = None;
            session.control_holder = None;
            session.control_epoch = session.control_epoch.saturating_add(1);
            session.lease_expires_at = None;
            session.human_lock = true;
        }
        // The human now owns every pane; no earlier binding reads what follows.
        for job in &mut doc.jobs {
            job.allow_output = false;
        }
        for operation in &mut doc.operations {
            if operation.state == "accepted" {
                operation.state = "rejected".into();
                operation.code = Some("feature-disabled".into());
                operation.updated_at = now_ms();
            }
        }
        for session in &mut doc.sessions {
            if closing.contains(&session.session_id) {
                session.closing = false;
            }
        }
        Ok(doc.sessions.clone())
    })?;
    let mut uncertain = false;
    for session in sessions {
        uncertain |=
            runner_control_result(send_runner_control(runtime, &session, "human")).is_err();
    }
    if uncertain {
        return Err(DeckError::new(
            ErrorKind::ContextChanged,
            "MCP is disabled locally but one or more runner fences are unconfirmed",
        ));
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn mcp_client_add(
    name: String,
    projects: Vec<ProjectScopeInput>,
    allow_create: bool,
) -> Result<ClientView, DeckError> {
    if !valid_title(&name) || projects.is_empty() || projects.len() > MAX_PROJECTS_PER_CLIENT {
        return Err(DeckError::new(
            ErrorKind::Invalid,
            "invalid MCP client scope",
        ));
    }
    let runtime = runtime()?;
    let enforce_projects = runtime.app.is_some();
    let scopes = validate_new_client_scopes(projects, enforce_projects, |project_id| {
        crate::documents::board_project_exists(project_id)
    })?;
    let client_id = random_id("client_")?;
    let credential = random_id("mcp_")?;
    if runtime.app.is_some() {
        crate::keychain::set_mcp_credential(&client_id, &credential)?;
    }
    let result = runtime.write(|doc| {
        if doc.config.clients.len() >= MAX_CLIENTS {
            return Err(DeckError::new(
                ErrorKind::DiskFull,
                "MCP client capacity reached",
            ));
        }
        let client = Client {
            id: client_id.clone(),
            name,
            credential_hash: sha(credential.as_bytes()),
            credential_version: 1,
            revoked_at: None,
            allow_create,
            projects: scopes,
            create_sequence: 0,
        };
        doc.config.clients.push(client.clone());
        Ok(ClientView {
            id: client.id,
            name: client.name,
            revoked: false,
            allow_create: client.allow_create,
            projects: client.projects,
        })
    });
    if result.is_err() && runtime.app.is_some() {
        let _ = crate::keychain::clear_mcp_credential(&client_id);
    }
    result
}

pub(super) fn validate_new_client_scopes(
    projects: Vec<ProjectScopeInput>,
    enforce_projects: bool,
    mut project_exists: impl FnMut(&str) -> Result<bool, DeckError>,
) -> Result<Vec<ProjectScope>, DeckError> {
    let mut scopes = Vec::new();
    for project in projects {
        if !valid_id(&project.project_id)
            || project.roots.is_empty()
            || project.roots.len() > MAX_ROOTS_PER_PROJECT
        {
            return Err(DeckError::new(
                ErrorKind::Invalid,
                "invalid MCP project scope",
            ));
        }
        let roots = project
            .roots
            .into_iter()
            .map(|root| canonical_project_root(&root))
            .collect::<Result<Vec<_>, _>>()?;
        if enforce_projects && !project_exists(&project.project_id)? {
            return Err(DeckError::new(
                ErrorKind::Missing,
                "MCP project no longer exists",
            ));
        }
        scopes.push(ProjectScope {
            project_id: project.project_id,
            roots,
        });
    }
    Ok(scopes)
}

/// Revocation fences the client in memory and removes its Keychain bearer
/// BEFORE waiting for the delivery lock, persists, then hands the client's
/// panes to the local user. The bearer goes first so a failed state write
/// cannot leave a client that re-authenticates after a Deck restart.
#[tauri::command(async)]
pub(crate) fn mcp_client_revoke(client_id: String) -> Result<(), DeckError> {
    client_revoke(runtime()?, client_id)
}

pub(super) fn client_revoke(runtime: &Runtime, client_id: String) -> Result<(), DeckError> {
    runtime
        .read(|doc| {
            doc.config
                .clients
                .iter()
                .any(|client| client.id == client_id)
        })?
        .then_some(())
        .ok_or_else(|| DeckError::new(ErrorKind::Missing, "MCP client not found"))?;
    runtime
        .emergency
        .lock_or_recover()
        .clients
        .insert(client_id.clone());
    let credential = if runtime.app.is_some() {
        crate::keychain::clear_mcp_credential(&client_id)
    } else {
        Ok(())
    };
    let _delivery = runtime.delivery.lock_or_recover();
    let sessions =
        runtime.write(|doc| {
            let client = doc
                .config
                .clients
                .iter_mut()
                .find(|client| client.id == client_id)
                .ok_or_else(|| DeckError::new(ErrorKind::Missing, "MCP client not found"))?;
            client.revoked_at = Some(now_ms());
            let closing = doc
                .operations
                .iter()
                .filter(|operation| {
                    operation.client_id == client_id
                        && operation.state == "accepted"
                        && operation.kind == "session-close"
                })
                .filter_map(|operation| operation.result.as_ref()?.get("sessionId")?.as_str())
                .map(str::to_owned)
                .collect::<HashSet<_>>();
            for grant in doc
                .execution_grants
                .iter_mut()
                .filter(|grant| grant.client_id == client_id && grant.revoked_at.is_none())
            {
                grant.revoked_at = Some(now_ms());
                grant.revocation_version = grant.grant_version;
            }
            for session in doc
                .sessions
                .iter_mut()
                .filter(|session| session.owner_client_id == client_id)
            {
                session.control_owner = None;
                session.control_holder = None;
                session.control_epoch = session.control_epoch.saturating_add(1);
                session.lease_expires_at = None;
                session.human_lock = true;
            }
            for job in doc.jobs.iter_mut().filter(|job| job.client_id == client_id) {
                job.allow_output = false;
            }
            for operation in doc.operations.iter_mut().filter(|operation| {
                operation.client_id == client_id && operation.state == "accepted"
            }) {
                operation.state = "rejected".into();
                operation.code = Some("client-revoked".into());
                operation.updated_at = now_ms();
            }
            for session in &mut doc.sessions {
                if closing.contains(&session.session_id) {
                    session.closing = false;
                }
            }
            audit(
                doc,
                "client-revoked",
                AuditLink {
                    principal_id: Some(&client_id),
                    ..Default::default()
                },
            )?;
            Ok(doc
                .sessions
                .iter()
                .filter(|session| session.owner_client_id == client_id)
                .cloned()
                .collect::<Vec<_>>())
        })?;
    let mut uncertain = false;
    for session in sessions {
        uncertain |=
            runner_control_result(send_runner_control(runtime, &session, "human")).is_err();
    }
    credential?;
    if uncertain {
        return Err(DeckError::new(
            ErrorKind::ContextChanged,
            "client is revoked locally but one or more runner fences are unconfirmed",
        ));
    }
    Ok(())
}

/// Remove a local authorization display record after revocation has already
/// fenced its sessions and pending side effects. Historical sessions,
/// operations, jobs, grants, and audit links intentionally retain the opaque
/// client id so deleting a client cannot erase the security ledger.
#[tauri::command]
pub(crate) fn mcp_client_delete(client_id: String) -> Result<(), DeckError> {
    if !valid_id(&client_id) {
        return Err(DeckError::new(ErrorKind::Invalid, "invalid MCP client id"));
    }
    let runtime = runtime()?;
    let _delivery = runtime.delivery.lock_or_recover();
    runtime
        .read(|doc| {
            doc.config
                .clients
                .iter()
                .find(|client| client.id == client_id)
                .map(|client| client.revoked_at.is_some())
        })?
        .ok_or_else(|| DeckError::new(ErrorKind::Missing, "MCP client not found"))?
        .then_some(())
        .ok_or_else(|| {
            DeckError::new(ErrorKind::Perm, "revoke the MCP client before deleting it")
        })?;
    if runtime.app.is_some() {
        crate::keychain::clear_mcp_credential(&client_id)?;
    }
    runtime.write(|doc| {
        let index = doc
            .config
            .clients
            .iter()
            .position(|client| client.id == client_id && client.revoked_at.is_some())
            .ok_or_else(|| DeckError::new(ErrorKind::Missing, "revoked MCP client not found"))?;
        doc.config.clients.remove(index);
        audit(
            doc,
            "client-deleted",
            AuditLink {
                principal_id: Some(&client_id),
                ..Default::default()
            },
        )?;
        Ok(())
    })?;
    runtime
        .emergency
        .lock_or_recover()
        .clients
        .remove(&client_id);
    Ok(())
}

/// Create a short-lived trusted-host execution window. This command is only
/// exposed to the local Tauri UI; there is intentionally no MCP route for it.
#[tauri::command(async)]
pub(crate) fn mcp_execution_grant(
    session_id: String,
    duration_ms: Option<u64>,
    allow_stdin: bool,
    allow_output: bool,
) -> Result<(), DeckError> {
    execution_grant(
        runtime()?,
        session_id,
        duration_ms,
        allow_stdin,
        allow_output,
    )
}

pub(super) fn execution_grant(
    runtime: &Runtime,
    session_id: String,
    duration_ms: Option<u64>,
    allow_stdin: bool,
    allow_output: bool,
) -> Result<(), DeckError> {
    let _delivery = runtime.delivery.lock_or_recover();
    let duration = duration_ms
        .unwrap_or(DEFAULT_EXECUTION_GRANT_MS)
        .clamp(60_000, MAX_EXECUTION_GRANT_MS);
    let service_instance = runtime.service_instance.clone();
    let monotonic = runtime.monotonic_ms();
    let (session, revoked) =
        runtime.write(|doc| {
            let session = doc
                .sessions
                .iter()
                .find(|session| session.session_id == session_id || session.card_id == session_id)
                .cloned()
                .ok_or_else(|| DeckError::new(ErrorKind::Missing, "MCP session not found"))?;
            let session_for_runner = session.clone();
            let credential_version = client(doc, &session.owner_client_id)?.credential_version;
            let version = doc
                .execution_grants
                .iter()
                .filter(|grant| grant.session_id == session.session_id)
                .map(|grant| grant.grant_version)
                .max()
                .unwrap_or(0)
                .saturating_add(1);
            let mut revoked = Vec::new();
            for grant in doc.execution_grants.iter_mut().filter(|grant| {
                grant.session_id == session.session_id && grant.revoked_at.is_none()
            }) {
                grant.revoked_at = Some(now_ms());
                grant.revocation_version = grant.grant_version;
                revoked.push((grant.grant_id.clone(), grant.grant_version));
            }
            let granted_session_id = session.session_id.clone();
            let principal_id = session.owner_client_id.clone();
            let grant_id = random_id("grant_")?;
            // Bounded by construction: superseded, closed-session and
            // earlier-process grants are retired before a new one is added,
            // so a later write (a takeover) can never fail validation.
            compact(doc, Some(&runtime.service_instance));
            if doc.execution_grants.len() >= MAX_GRANTS {
                return Err(DeckError::new(
                    ErrorKind::DiskFull,
                    "MCP execution grant capacity reached",
                ));
            }
            doc.execution_grants.push(ExecutionGrant {
                grant_id: grant_id.clone(),
                client_id: session.owner_client_id,
                credential_version,
                project_id: session.project_id,
                session_id: session.session_id,
                session_generation: session.generation,
                profile: "trusted-host-v1".into(),
                environment_profile: ENVIRONMENT_PROFILE.into(),
                environment_profile_version: 1,
                issued_at: now_ms(),
                expires_at: now_ms().saturating_add(duration),
                issued_monotonic_ms: monotonic,
                duration_ms: duration,
                allow_stdin,
                allow_output,
                _legacy_allow_shell: None,
                grant_version: version,
                revocation_version: version.saturating_sub(1),
                service_instance,
                revoked_at: None,
            });
            if let Some(managed) = doc
                .sessions
                .iter_mut()
                .find(|managed| managed.session_id == granted_session_id)
            {
                managed.output_shared = allow_output;
            }
            audit(
                doc,
                "grant-approved",
                AuditLink {
                    principal_id: Some(&principal_id),
                    session_id: Some(&granted_session_id),
                    grant_id: Some(&grant_id),
                    ..Default::default()
                },
            )?;
            Ok((session_for_runner, revoked))
        })?;
    let mut uncertain = false;
    for (grant_id, grant_version) in revoked {
        uncertain |= send_runner(
            runtime,
            &session,
            &json!({"kind":"revoke-grant","service_instance":runtime.service_instance,"grant_id":grant_id,"grant_version":grant_version}),
        )
        .ok()
        .is_none_or(|value| value.get("ok").and_then(Value::as_bool) != Some(true));
    }
    if uncertain {
        return Err(DeckError::new(
            ErrorKind::ContextChanged,
            "the new grant is saved but an older runner grant fence is unconfirmed",
        ));
    }
    // An approval lifts only a revocation that failed to persist BEFORE it
    // (both ran under the delivery lock, so that one is ordered earlier and
    // this approval's write revoked its grants durably). A revocation that is
    // still pending keeps its own count.
    runtime
        .emergency
        .lock_or_recover()
        .execution_unpersisted
        .remove(&session.session_id);
    Ok(())
}

#[tauri::command(async)]
pub(crate) fn mcp_execution_revoke(session_id: String) -> Result<(), DeckError> {
    execution_revoke(runtime()?, &session_id)
}

pub(super) fn execution_revoke(runtime: &Runtime, session_id: &str) -> Result<(), DeckError> {
    let fenced_session_id = resolve_session_id(runtime, session_id)?;
    *runtime
        .emergency
        .lock_or_recover()
        .execution_pending
        .entry(fenced_session_id.clone())
        .or_default() += 1;
    pause_point(runtime, "revoke-fenced");
    let _delivery = runtime.delivery.lock_or_recover();
    let persisted =
        runtime.write(|doc| {
            let session = doc
                .sessions
                .iter()
                .find(|session| session.session_id == session_id || session.card_id == session_id)
                .cloned()
                .ok_or_else(|| DeckError::new(ErrorKind::Missing, "MCP session not found"))?;
            let mut revoked = Vec::new();
            for grant in doc.execution_grants.iter_mut().filter(|grant| {
                grant.session_id == session.session_id && grant.revoked_at.is_none()
            }) {
                grant.revoked_at = Some(now_ms());
                grant.revocation_version = grant.grant_version;
                revoked.push((grant.grant_id.clone(), grant.grant_version));
            }
            audit(
                doc,
                "grant-revoked",
                AuditLink {
                    principal_id: Some(&session.owner_client_id),
                    session_id: Some(&session.session_id),
                    ..Default::default()
                },
            )?;
            // Output sharing is a separate local switch: revoking execution
            // leaves it exactly as it was.
            Ok((session, revoked))
        });
    {
        // Still under the delivery lock: the persisted revocation (or, when it
        // failed, the sticky fence) replaces this revocation's pending count
        // before any waiting request or approval can run.
        let mut emergency = runtime.emergency.lock_or_recover();
        if let Some(count) = emergency.execution_pending.get_mut(&fenced_session_id) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                emergency.execution_pending.remove(&fenced_session_id);
            }
        }
        if persisted.is_err() {
            emergency.execution_unpersisted.insert(fenced_session_id);
        }
    }
    let (session, revoked) = persisted?;
    let mut uncertain = false;
    for (grant_id, grant_version) in revoked {
        uncertain |= send_runner(
            runtime,
            &session,
            &json!({"kind":"revoke-grant","service_instance":runtime.service_instance,"grant_id":grant_id,"grant_version":grant_version}),
        )
        .ok()
        .is_none_or(|value| value.get("ok").and_then(Value::as_bool) != Some(true));
    }
    if uncertain {
        return Err(DeckError::new(
            ErrorKind::ContextChanged,
            "execution is revoked locally but the runner revocation fence is unconfirmed",
        ));
    }
    Ok(())
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PendingView {
    pub(super) operation_id: String,
    pub(super) kind: String,
    pub(super) result: Value,
}

#[tauri::command]
pub(crate) fn mcp_pending() -> Result<Vec<PendingView>, DeckError> {
    runtime()?.read(|doc| {
        if !doc.config.enabled {
            Vec::new()
        } else {
            doc.operations
                .iter()
                .filter(|operation| {
                    operation.state == "accepted"
                        && matches!(operation.kind.as_str(), "session-create" | "session-close")
                })
                .filter_map(|operation| {
                    operation.result.clone().map(|result| PendingView {
                        operation_id: operation.operation_id.clone(),
                        kind: operation.kind.clone(),
                        result,
                    })
                })
                .collect()
        }
    })
}

#[tauri::command]
pub(crate) fn mcp_claim(operation_id: String) -> Result<PendingView, DeckError> {
    claim(runtime()?, &operation_id)
}

pub(super) fn claim(runtime: &Runtime, operation_id: &str) -> Result<PendingView, DeckError> {
    runtime.write(|doc| {
        if !doc.config.enabled {
            return Err(DeckError::new(ErrorKind::Perm, "MCP control is disabled"));
        }
        let operation = doc
            .operations
            .iter_mut()
            .find(|operation| {
                operation.operation_id == operation_id && operation.state == "accepted"
            })
            .ok_or_else(|| DeckError::new(ErrorKind::Missing, "MCP operation not pending"))?;
        operation.state = "executing".into();
        operation.updated_at = now_ms();
        Ok(PendingView {
            operation_id: operation.operation_id.clone(),
            kind: operation.kind.clone(),
            result: operation.result.clone().unwrap_or(Value::Null),
        })
    })
}

#[tauri::command]
pub(crate) fn mcp_validate(operation_id: String) -> Result<(), DeckError> {
    validate(runtime()?, &operation_id)
}

pub(super) fn validate(runtime: &Runtime, operation_id: &str) -> Result<(), DeckError> {
    let _delivery = runtime.delivery.lock_or_recover();
    let target = runtime.read(|doc| {
        doc.operations
            .iter()
            .find(|operation| operation.operation_id == operation_id)
            .map(|operation| {
                (
                    operation.client_id.clone(),
                    operation
                        .result
                        .as_ref()
                        .and_then(|result| result.get("sessionId"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                )
            })
    })?;
    if let Some((client_id, session_id)) = &target {
        if let Some(error) = emergency_fence_error(runtime, client_id, session_id) {
            return Err(error);
        }
    }
    runtime.read(|doc| {
        if !doc.config.enabled {
            return Err(DeckError::new(ErrorKind::Perm, "MCP control is disabled"));
        }
        let operation = doc
            .operations
            .iter()
            .find(|operation| {
                operation.operation_id == operation_id
                    && matches!(operation.state.as_str(), "executing" | "admitted")
            })
            .ok_or_else(|| {
                DeckError::new(
                    ErrorKind::ContextChanged,
                    "MCP operation is no longer executable",
                )
            })?;
        client(doc, &operation.client_id)?;
        if operation.kind == "session-close" {
            let result = operation.result.as_ref().ok_or_else(|| {
                DeckError::new(ErrorKind::ContextChanged, "MCP close plan is missing")
            })?;
            let session_id = result
                .get("sessionId")
                .and_then(Value::as_str)
                .unwrap_or("");
            let generation = result
                .get("sessionGeneration")
                .and_then(Value::as_str)
                .unwrap_or("");
            let epoch = result
                .get("controlEpoch")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let holder = result.get("holderId").and_then(Value::as_str).unwrap_or("");
            let session = authorized_session(doc, &operation.client_id, session_id)?;
            check_control(session, &operation.client_id, generation, epoch, holder)?;
            if !session.closing {
                return Err(DeckError::new(
                    ErrorKind::ContextChanged,
                    "MCP close plan is no longer current",
                ));
            }
        }
        Ok(())
    })?
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CloseAdmissionView {
    pub(super) admission: String,
}

/// Linearize a remote close inside the Board transaction, immediately before
/// its first durable queue-cancellation side effect. Once admitted, later
/// revocation does not pretend that the already-admitted close never started.
#[tauri::command]
pub(crate) fn mcp_close_admit(operation_id: String) -> Result<CloseAdmissionView, DeckError> {
    let runtime = runtime()?;
    close_admit(runtime, &operation_id)
}

pub(super) fn close_admit(
    runtime: &Runtime,
    operation_id: &str,
) -> Result<CloseAdmissionView, DeckError> {
    let _delivery = runtime.delivery.lock_or_recover();
    pause_point(runtime, "close-admit");
    // Emergency fences first, under the delivery lock: a takeover, client
    // revocation or disable that fenced while this admission waited wins.
    let target = runtime.read(|doc| {
        doc.operations
            .iter()
            .find(|operation| operation.operation_id == operation_id)
            .map(|operation| {
                (
                    operation.client_id.clone(),
                    operation
                        .result
                        .as_ref()
                        .and_then(|result| result.get("sessionId"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                )
            })
    })?;
    if let Some((client_id, session_id)) = &target {
        if let Some(error) = emergency_fence_error(runtime, client_id, session_id) {
            return Err(error);
        }
    }
    let admission = random_id("close_")?;
    let admission_hash = sha(admission.as_bytes());
    runtime.write(|doc| {
        if !doc.config.enabled {
            return Err(DeckError::new(ErrorKind::Perm, "MCP control is disabled"));
        }
        let index = doc
            .operations
            .iter()
            .position(|operation| {
                operation.operation_id == operation_id
                    && operation.kind == "session-close"
                    && operation.state == "executing"
            })
            .ok_or_else(|| {
                DeckError::new(
                    ErrorKind::ContextChanged,
                    "MCP close operation is no longer admissible",
                )
            })?;
        let operation = doc.operations[index].clone();
        client(doc, &operation.client_id)?;
        let result = operation.result.as_ref().ok_or_else(|| {
            DeckError::new(ErrorKind::ContextChanged, "MCP close plan is missing")
        })?;
        let session_id = result
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or("");
        let generation = result
            .get("sessionGeneration")
            .and_then(Value::as_str)
            .unwrap_or("");
        let epoch = result
            .get("controlEpoch")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let holder = result.get("holderId").and_then(Value::as_str).unwrap_or("");
        let session = authorized_session(doc, &operation.client_id, session_id)?;
        check_control(session, &operation.client_id, generation, epoch, holder)?;
        if !session.closing {
            return Err(DeckError::new(
                ErrorKind::ContextChanged,
                "MCP close plan is no longer current",
            ));
        }
        let saved = &mut doc.operations[index];
        saved.state = "admitted".into();
        saved.admission_hash = Some(admission_hash);
        saved.updated_at = now_ms();
        Ok(())
    })?;
    Ok(CloseAdmissionView { admission })
}

/// Validate an already-linearized close at each native side-effect boundary.
/// This does not re-run revocation checks: revocation ordered after admission
/// cannot roll back queue cancellation that may already have committed.
pub(crate) fn validate_close_admission(
    admission: Option<&str>,
    tmux_sessions: &[String],
) -> Result<(), DeckError> {
    validate_close_admission_with(runtime()?, admission, tmux_sessions)
}

pub(super) fn validate_close_admission_with(
    runtime: &Runtime,
    admission: Option<&str>,
    tmux_sessions: &[String],
) -> Result<(), DeckError> {
    let Some(admission) = admission else {
        return Ok(());
    };
    let hash = sha(admission.as_bytes());
    runtime.read(|doc| {
        let operation = doc
            .operations
            .iter()
            .find(|operation| {
                operation.kind == "session-close"
                    && operation.state == "admitted"
                    && operation.admission_hash.as_deref() == Some(&hash)
            })
            .ok_or_else(|| {
                DeckError::new(ErrorKind::ContextChanged, "MCP close admission is invalid")
            })?;
        let session_id = operation
            .result
            .as_ref()
            .and_then(|result| result.get("sessionId"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let session = doc
            .sessions
            .iter()
            .find(|session| session.session_id == session_id)
            .ok_or_else(|| DeckError::new(ErrorKind::ContextChanged, "MCP close target changed"))?;
        if tmux_sessions.len() != 1 || tmux_sessions[0] != session.tmux_session {
            return Err(DeckError::new(
                ErrorKind::ContextChanged,
                "MCP close admission target changed",
            ));
        }
        Ok(())
    })?
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StartResult {
    pub(super) created: bool,
}

#[tauri::command]
pub(crate) fn mcp_start_session(
    operation_id: String,
    name: String,
    dir: String,
) -> Result<StartResult, DeckError> {
    let runtime = runtime()?;
    let _delivery = runtime.delivery.lock_or_recover();
    crate::tmux::validate_session_name(&name)?;
    let plan = runtime.read(|doc| {
        let operation = doc
            .operations
            .iter()
            .find(|operation| {
                operation.operation_id == operation_id
                    && operation.kind == "session-create"
                    && operation.state == "executing"
            })
            .ok_or_else(|| {
                DeckError::new(
                    ErrorKind::ContextChanged,
                    "MCP create operation is no longer executable",
                )
            })?;
        client(doc, &operation.client_id)?;
        Ok::<(Value, String), DeckError>((
            operation.result.clone().unwrap_or(Value::Null),
            operation.client_id.clone(),
        ))
    })??;
    let (plan, client_id) = plan;
    // A client revocation or disable that fenced while this start waited for
    // the delivery lock wins (checked outside the state lock).
    if let Some(error) = emergency_fence_error(runtime, &client_id, "") {
        return Err(error);
    }
    if plan.get("cwd").and_then(Value::as_str) != Some(&dir) {
        return Err(DeckError::new(
            ErrorKind::ContextChanged,
            "MCP create context changed",
        ));
    }
    let socket = plan
        .get("runnerSocket")
        .and_then(Value::as_str)
        .ok_or_else(|| DeckError::new(ErrorKind::Invalid, "MCP create plan is invalid"))?;
    let generation = plan
        .get("generation")
        .and_then(Value::as_str)
        .ok_or_else(|| DeckError::new(ErrorKind::Invalid, "MCP create plan is invalid"))?;
    let output_retention_ms = runtime.read(|doc| doc.config.output_retention_ms)?;
    if crate::tmux::tmux(&["has-session", "-t", &crate::tmux::session_target(&name)]).is_ok() {
        if runner_socket_matches(runtime, &name, socket, generation) {
            return Ok(StartResult { created: true });
        }
        return Err(DeckError::new(
            ErrorKind::ContextChanged,
            "unmanaged matching session already exists",
        ));
    }
    let _activity = crate::session_runtime::activity_guard()?;
    let _creation = crate::tmux_lifecycle::session_creation_guard()?;
    let runner = runner_program()?;
    let args = vec![
        "new-session".into(),
        "-d".into(),
        "-s".into(),
        name.clone(),
        "-c".into(),
        dir,
        runner.display().to_string(),
        "--socket".into(),
        socket.into(),
        "--generation".into(),
        generation.into(),
        "--service-instance".into(),
        runtime.service_instance.clone(),
        "--deck-pid".into(),
        std::process::id().to_string(),
        "--output-retention-ms".into(),
        output_retention_ms.to_string(),
    ];
    crate::tmux::tmux_owned(&args)?;
    let ready = (0..40).any(|_| {
        if UnixStream::connect(socket).is_ok() {
            true
        } else {
            std::thread::sleep(Duration::from_millis(50));
            false
        }
    });
    if !ready {
        let _ = crate::tmux::tmux(&["kill-session", "-t", &crate::tmux::session_target(&name)]);
        return Err(DeckError::new(
            ErrorKind::Other,
            "MCP runner did not become ready",
        ));
    }
    Ok(StartResult { created: true })
}

#[tauri::command]
pub(crate) fn mcp_complete(
    operation_id: String,
    state: String,
    code: Option<String>,
    tmux_session: Option<String>,
) -> Result<(), DeckError> {
    complete(runtime()?, operation_id, state, code, tmux_session)
}

pub(super) fn complete(
    runtime: &Runtime,
    operation_id: String,
    state: String,
    code: Option<String>,
    tmux_session: Option<String>,
) -> Result<(), DeckError> {
    if !matches!(state.as_str(), "committed" | "rejected" | "ambiguous") {
        return Err(DeckError::new(
            ErrorKind::Invalid,
            "invalid MCP operation result",
        ));
    }
    let _delivery = runtime.delivery.lock_or_recover();
    let switch_to_human = runtime.write(|doc| {
        let index = doc
            .operations
            .iter()
            .position(|operation| operation.operation_id == operation_id)
            .ok_or_else(|| DeckError::new(ErrorKind::Missing, "MCP operation not found"))?;
        let operation = doc.operations[index].clone();
        // Transitions: executing → committed|rejected|ambiguous; a close that
        // was admitted (its first Board side effect may have run) →
        // committed|ambiguous only. Repeating the recorded terminal state is
        // an idempotent no-op; anything else is refused.
        match (operation.state.as_str(), state.as_str()) {
            ("executing", _) => {}
            ("admitted", "committed" | "ambiguous") if operation.kind == "session-close" => {}
            (recorded, requested) if recorded == requested => return Ok(None),
            _ => {
                return Err(DeckError::new(
                    ErrorKind::ContextChanged,
                    "MCP operation cannot take that result",
                ))
            }
        }
        let authorized = doc.config.enabled
            && doc
                .config
                .clients
                .iter()
                .any(|client| client.id == operation.client_id && client.revoked_at.is_none());
        let mut created_session = None;
        if operation.kind == "session-create" && state == "committed" {
            let result = operation
                .result
                .as_ref()
                .ok_or_else(|| DeckError::new(ErrorKind::Invalid, "MCP create result missing"))?;
            let tmux_session = tmux_session
                .clone()
                .ok_or_else(|| DeckError::new(ErrorKind::Invalid, "MCP tmux session missing"))?;
            let get = |key: &str| {
                result
                    .get(key)
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| DeckError::new(ErrorKind::Invalid, "MCP create result invalid"))
            };
            let session = ManagedSession {
                session_id: get("sessionId")?,
                card_id: get("cardId")?,
                tmux_session,
                project_id: get("projectId")?,
                title: get("title")?,
                cwd: get("cwd")?,
                generation: get("generation")?,
                runner_socket: get("runnerSocket")?,
                owner_client_id: operation.client_id.clone(),
                control_owner: None,
                control_holder: None,
                control_epoch: 1,
                lease_expires_at: None,
                human_lock: !authorized,
                output_shared: false,
                closing: false,
                created_at: now_ms(),
                control_sequence: 0,
            };
            doc.sessions.push(session.clone());
            created_session = (!authorized).then_some(session);
        }
        if operation.kind == "session-close" && state == "committed" {
            if let Some(id) = operation
                .result
                .as_ref()
                .and_then(|value| value.get("sessionId"))
                .and_then(Value::as_str)
            {
                doc.sessions.retain(|session| session.session_id != id);
                doc.jobs.retain(|job| job.session_id != id);
            }
        } else if operation.kind == "session-close" {
            if let Some(id) = operation
                .result
                .as_ref()
                .and_then(|value| value.get("sessionId"))
                .and_then(Value::as_str)
            {
                if let Some(session) = doc
                    .sessions
                    .iter_mut()
                    .find(|session| session.session_id == id)
                {
                    session.closing = false;
                }
            }
        }
        let saved = &mut doc.operations[index];
        saved.state = state;
        saved.code = code;
        saved.updated_at = now_ms();
        compact(doc, Some(&runtime.service_instance));
        Ok(created_session)
    })?;
    if let Some(session) = switch_to_human {
        let _ = send_runner_control(runtime, &session, "human");
    }
    Ok(())
}

/// Reconcile an explicit local Board close that did not originate as an MCP
/// operation. The card is already durably absent when this is called.
#[tauri::command]
pub(crate) fn mcp_card_closed(card_id: String) -> Result<(), DeckError> {
    let runtime = runtime()?;
    let removed = runtime.write(|doc| {
        let removed = doc
            .sessions
            .iter()
            .filter(|session| session.card_id == card_id)
            .map(|session| session.session_id.clone())
            .collect::<HashSet<_>>();
        doc.sessions.retain(|session| session.card_id != card_id);
        doc.jobs.retain(|job| !removed.contains(&job.session_id));
        // Records and grants of a closed session can no longer act.
        compact(doc, Some(&runtime.service_instance));
        Ok(removed)
    })?;
    let mut emergency = runtime.emergency.lock_or_recover();
    for session_id in removed {
        emergency.human_sessions.remove(&session_id);
        emergency.execution_unpersisted.remove(&session_id);
    }
    Ok(())
}

pub(super) fn resolve_session_id(runtime: &Runtime, session_id: &str) -> Result<String, DeckError> {
    runtime
        .read(|doc| {
            doc.sessions
                .iter()
                .find(|session| session.session_id == session_id || session.card_id == session_id)
                .map(|session| session.session_id.clone())
        })?
        .ok_or_else(|| DeckError::new(ErrorKind::Missing, "MCP session not found"))
}

/// Local takeover. The in-memory fence is set FIRST — before waiting for the
/// delivery lock that an in-flight exec may hold — so no new remote request
/// passes route or dispatch after this point. Then the takeover is persisted
/// (new epoch, holder/lease cleared, output sharing off, and every existing
/// job binding's output closed to MCP for good) and the runner hands the pane
/// keyboard and the ^C stop key to the human. Errors are stable codes; the
/// fence holds whatever happens afterwards.
#[tauri::command(async)]
pub(crate) fn mcp_takeover(session_id: String) -> Result<(), DeckError> {
    takeover(runtime()?, &session_id)
}

pub(super) fn takeover(runtime: &Runtime, session_id: &str) -> Result<(), DeckError> {
    let fenced_session_id = resolve_session_id(runtime, session_id)?;
    runtime
        .emergency
        .lock_or_recover()
        .human_sessions
        .insert(fenced_session_id.clone());
    let _delivery = runtime.delivery.lock_or_recover();
    let persisted = runtime.write(|doc| {
        let session = doc
            .sessions
            .iter_mut()
            .find(|session| session.session_id == fenced_session_id)
            .ok_or_else(|| DeckError::new(ErrorKind::Missing, "MCP session not found"))?;
        session.control_owner = None;
        session.control_holder = None;
        session.control_epoch = session.control_epoch.saturating_add(1);
        session.lease_expires_at = None;
        session.human_lock = true;
        session.output_shared = false;
        let session = session.clone();
        // Output produced from here on may include the human's own typing
        // into the job: no pre-takeover binding may ever read it.
        for job in doc
            .jobs
            .iter_mut()
            .filter(|job| job.session_id == session.session_id)
        {
            job.allow_output = false;
        }
        audit(
            doc,
            "human-takeover",
            AuditLink {
                principal_id: Some(&session.owner_client_id),
                session_id: Some(&session.session_id),
                ..Default::default()
            },
        )?;
        Ok(session)
    });
    let session = match &persisted {
        Ok(session) => session.clone(),
        Err(_) => {
            // Still hand the keyboard over under a higher epoch; the
            // in-memory fence keeps MCP out for this Deck run.
            let mut session = runtime
                .read(|doc| {
                    doc.sessions
                        .iter()
                        .find(|session| session.session_id == fenced_session_id)
                        .cloned()
                })?
                .ok_or_else(|| DeckError::new(ErrorKind::Missing, "MCP session not found"))?;
            session.control_epoch = session.control_epoch.saturating_add(1);
            session.control_holder = None;
            session
        }
    };
    runner_control_result(send_runner_control(runtime, &session, "human"))?;
    if persisted.is_err() {
        return Err(DeckError::new(ErrorKind::Other, FENCE_UNPERSISTED));
    }
    Ok(())
}

/// Local return of a human-locked session to MCP. It needs no execution
/// grant, creates or extends none, restores no holder, lease or output
/// sharing, and never revives an old epoch: the persisted state moves to a
/// NEW epoch with no holder, then the runner is told. A runner failure
/// re-fences (human lock back on, another new epoch). Machine codes:
/// SESSION_BUSY (a job still runs; stop it locally first), RUNNER_STALE
/// (Deck restarted since the session was created; close it),
/// CLIENT_REVOKED / FEATURE_DISABLED (nothing to return to).
#[tauri::command(async)]
pub(crate) fn mcp_return_control(session_id: String) -> Result<(), DeckError> {
    return_control(runtime()?, &session_id)
}

pub(super) fn return_control(runtime: &Runtime, session_id: &str) -> Result<(), DeckError> {
    let _delivery = runtime.delivery.lock_or_recover();
    let session = runtime.read(|doc| {
        let session = doc
            .sessions
            .iter()
            .find(|session| session.session_id == session_id || session.card_id == session_id)
            .cloned()
            .ok_or_else(|| DeckError::new(ErrorKind::Missing, "MCP session not found"))?;
        if !doc.config.enabled {
            return Err(DeckError::new(ErrorKind::Perm, FEATURE_DISABLED));
        }
        if client(doc, &session.owner_client_id).is_err() {
            return Err(DeckError::new(ErrorKind::Perm, CLIENT_REVOKED));
        }
        Ok(session)
    })??;
    {
        let emergency = runtime.emergency.lock_or_recover();
        if emergency.disabled {
            return Err(DeckError::new(ErrorKind::Perm, FEATURE_DISABLED));
        }
        if emergency.clients.contains(&session.owner_client_id) {
            return Err(DeckError::new(ErrorKind::Perm, CLIENT_REVOKED));
        }
    }
    match probe_runner(runtime, &session) {
        None => {
            return Err(DeckError::new(
                ErrorKind::ContextChanged,
                RUNNER_UNCONFIRMED,
            ))
        }
        Some(probe) if !probe.current => {
            return Err(DeckError::new(ErrorKind::ContextChanged, RUNNER_STALE))
        }
        Some(probe) if !probe.job.is_null() => {
            return Err(DeckError::new(ErrorKind::Locked, SESSION_BUSY))
        }
        Some(_) => {}
    }
    let returned = runtime.write(|doc| {
        let current = doc
            .sessions
            .iter_mut()
            .find(|item| {
                item.session_id == session.session_id && item.generation == session.generation
            })
            .ok_or_else(|| {
                DeckError::new(ErrorKind::ContextChanged, "MCP session generation changed")
            })?;
        current.human_lock = false;
        current.control_epoch = current.control_epoch.saturating_add(1);
        current.control_owner = None;
        current.control_holder = None;
        current.lease_expires_at = None;
        let current = current.clone();
        audit(
            doc,
            "human-return",
            AuditLink {
                principal_id: Some(&current.owner_client_id),
                session_id: Some(&current.session_id),
                ..Default::default()
            },
        )?;
        Ok(current)
    })?;
    if let Err(error) = runner_control_result(send_runner_control(runtime, &returned, "mcp")) {
        // Re-fence: the persisted state must not claim MCP control that the
        // runner never confirmed. The in-memory fence is still in place.
        let refenced = runtime.write(|doc| {
            Ok(doc
                .sessions
                .iter_mut()
                .find(|item| item.session_id == returned.session_id)
                .map(|current| {
                    current.human_lock = true;
                    current.control_epoch = current.control_epoch.saturating_add(1);
                    current.control_owner = None;
                    current.control_holder = None;
                    current.lease_expires_at = None;
                    current.clone()
                }))
        });
        if let Ok(Some(current)) = refenced {
            let _ = send_runner_control(runtime, &current, "human");
        }
        return Err(error);
    }
    runtime
        .emergency
        .lock_or_recover()
        .human_sessions
        .remove(&session.session_id);
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionUiView {
    pub(super) managed: bool,
    /// The runner is unreachable or belongs to an earlier Deck process: it can
    /// only be closed.
    pub(super) stale: bool,
    pub(super) human_control: bool,
    pub(super) control_owner: Option<String>,
    pub(super) control_holder: Option<String>,
    pub(super) control_epoch: u64,
    pub(super) active_job: bool,
    pub(super) client_name: Option<String>,
    pub(super) job_state: Option<String>,
    pub(super) recent_error: Option<String>,
    pub(super) execution_grant_active: bool,
    pub(super) execution_expires_at: Option<u64>,
    pub(super) stdin_allowed: bool,
    pub(super) output_shared: bool,
}

#[tauri::command(async)]
pub(crate) fn mcp_session_ui(card_id: String) -> Result<SessionUiView, DeckError> {
    let runtime = runtime()?;
    record_expired_grants(runtime)?;
    let session = runtime.read(|doc| {
        doc.sessions
            .iter()
            .find(|session| session.card_id == card_id)
            .cloned()
    })?;
    let Some(session) = session else {
        return Ok(SessionUiView {
            managed: false,
            stale: false,
            human_control: false,
            control_owner: None,
            control_holder: None,
            control_epoch: 0,
            active_job: false,
            client_name: None,
            job_state: None,
            recent_error: None,
            execution_grant_active: false,
            execution_expires_at: None,
            stdin_allowed: false,
            output_shared: false,
        });
    };
    let (client_name, recent_error, grant) = runtime.read(|doc| {
        let name = doc
            .config
            .clients
            .iter()
            .find(|client| client.id == session.owner_client_id)
            .map(|client| client.name.clone());
        let error = doc
            .operations
            .iter()
            .rev()
            .find(|operation| {
                operation.client_id == session.owner_client_id
                    && operation.code.is_some()
                    && operation
                        .result
                        .as_ref()
                        .and_then(|result| result.get("sessionId"))
                        .and_then(Value::as_str)
                        == Some(&session.session_id)
            })
            .and_then(|operation| operation.code.clone());
        let grant = doc
            .execution_grants
            .iter()
            .rev()
            .find(|grant| {
                grant.session_id == session.session_id
                    && grant.service_instance == runtime.service_instance
                    && grant.revoked_at.is_none()
                    && now_ms() < grant.expires_at
                    && runtime
                        .monotonic_ms()
                        .saturating_sub(grant.issued_monotonic_ms)
                        < grant.duration_ms
            })
            .cloned();
        (name, error, grant)
    })?;
    let runner = probe_runner(runtime, &session);
    let emergency_human = runtime
        .emergency
        .lock_or_recover()
        .human_sessions
        .contains(&session.session_id);
    let job_state = runner
        .as_ref()
        .and_then(|probe| probe.job.get("state"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok(SessionUiView {
        managed: true,
        stale: runner.as_ref().is_none_or(|probe| !probe.current),
        human_control: session.human_lock || emergency_human,
        control_owner: session.control_owner,
        control_holder: session.control_holder,
        control_epoch: session.control_epoch,
        active_job: job_state.is_some(),
        client_name,
        job_state,
        recent_error,
        execution_grant_active: grant.is_some(),
        execution_expires_at: grant.as_ref().map(|grant| grant.expires_at),
        stdin_allowed: grant.as_ref().is_some_and(|grant| grant.allow_stdin),
        output_shared: session.output_shared && !emergency_human,
    })
}

/// All ordinary Deck terminal-input paths call this before writing. A managed
/// runner also discards pane stdin while MCP owns control, closing the check /
/// write race for keyboard input already queued at takeover.
pub(crate) fn guard_terminal_input(tmux_session: &str) -> Result<(), DeckError> {
    let Some(runtime) = RUNTIME.get() else {
        return Ok(());
    };
    let session = runtime.read(|doc| {
        doc.sessions
            .iter()
            .find(|session| session.tmux_session == tmux_session)
            .map(|session| (session.session_id.clone(), session.human_lock))
    })?;
    match session {
        None | Some((_, true)) => Ok(()),
        // A takeover whose state write failed still hands the keyboard over
        // for this Deck run: the in-memory fence is authoritative.
        Some((session_id, false))
            if runtime
                .emergency
                .lock_or_recover()
                .human_sessions
                .contains(&session_id) =>
        {
            Ok(())
        }
        Some(_) => Err(DeckError::new(ErrorKind::Perm, "MCP owns terminal control")),
    }
}

/// Close path hook (`commands::kill_session`): before tmux kills a managed
/// pane, ask its runner to stop every job group with bounded escalation
/// (SIGINT → SIGTERM → SIGKILL). Best effort by design — after a Deck restart
/// the in-memory runner key is unavailable, and the runner's SIGHUP handler
/// still SIGKILLs live job groups when the pane dies.
pub(crate) fn stop_managed_jobs(tmux_session: &str) {
    let Some(runtime) = RUNTIME.get() else {
        return;
    };
    let Ok(Some(session)) = runtime.read(|doc| {
        doc.sessions
            .iter()
            .find(|session| session.tmux_session == tmux_session)
            .cloned()
    }) else {
        return;
    };
    let _ = send_runner(
        runtime,
        &session,
        &json!({"kind":"stop","generation":session.generation}),
    );
}

/// Managed runner panes cannot be reconstructed by the ordinary shell
/// restart transaction. The user must explicitly close them first.
pub(crate) fn guard_server_restart() -> Result<(), DeckError> {
    let Some(runtime) = RUNTIME.get() else {
        return Ok(());
    };
    runtime.read(|doc| {
        if doc.sessions.is_empty() {
            Ok(())
        } else {
            Err(DeckError::new(
                ErrorKind::Locked,
                "tmux-restart-mcp-managed-sessions",
            ))
        }
    })?
}
