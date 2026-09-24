//! Session-level requests: capabilities, listing, create, inspect, control transfer and close.
//!
//! Split out of the one-file `mcp.rs` on 2026-09-23; the contract stays in
//! `mcp/mod.rs`.

use super::*;

pub(super) fn capabilities(
    runtime: &Runtime,
    client_id: &str,
    arguments: Value,
) -> Result<Value, Value> {
    parse::<Empty>(arguments)?;
    runtime
        .read(|doc| {
            let client = doc
                .config
                .clients
                .iter()
                .find(|client| client.id == client_id && client.revoked_at.is_none())
                .ok_or_else(|| DeckError::new(ErrorKind::Perm, "MCP client is not authorized"))?;
            Ok(json!({
                "ok": true,
                "deckConnection": "connected",
                "controlProtocolVersion": CONTROL_PROTOCOL,
                "stateSchemaVersion": STATE_VERSION,
                "deckVersion": DECK_VERSION,
                "deckBuild": DECK_BUILD,
                "executionMode": "structured-direct-default",
                "realOsSandbox": false,
                "directExecution": {
                    "default": true,
                    "arbitraryPrograms": true,
                    "executableResolution": "absolute-path-only",
                    "argumentsVisibleInProcessMetadata": true,
                    "path": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
                    "pathPurpose": "child-process-environment",
                    "outputKind": "pty_combined"
                },
                "limits": {
                    "executableBytes": MAX_EXECUTABLE_BYTES,
                    "argumentCount": MAX_ARGUMENTS,
                    "argumentBytes": MAX_ARGUMENT_BYTES,
                    "inputBytes": MAX_INPUT_BYTES,
                    "readBytesDefault": MAX_READ_BYTES,
                    "readBytesMax": MAX_READ_BYTES,
                    "waitMsMax": 5000,
                    "retainedOutputBytesPerJob": 1024 * 1024,
                    "retainedOutputBytesPerSession": 16 * 1024 * 1024,
                    "retainedOutputBytesGlobal": MAX_SESSIONS * 16 * 1024 * 1024,
                    "managedSessions": MAX_SESSIONS,
                    "operations": MAX_OPERATIONS
                },
                "outputRetentionMs": doc.config.output_retention_ms,
                "interactiveInput": true,
                "structuredProjectRead": true,
                "reliableJobExitTracking": true,
                "authorizedWorkspaces": client.projects,
                "mayCreateSession": client.allow_create,
                "nextCreateSequence": client.create_sequence
            }))
        })
        .map_err(map_error)?
        .map_err(map_error)
}

pub(super) fn sessions_list(
    runtime: &Runtime,
    client_id: &str,
    arguments: Value,
) -> Result<Value, Value> {
    parse::<Empty>(arguments)?;
    let (sessions, next_create_sequence) = runtime
        .read(|doc| {
            let create_sequence = client(doc, client_id)?.create_sequence;
            Ok((
                doc.sessions
                    .iter()
                    .filter(|session| session.owner_client_id == client_id)
                    .cloned()
                    .collect::<Vec<_>>(),
                create_sequence,
            ))
        })
        .map_err(map_error)?
        .map_err(map_error)?;
    let values = sessions
        .into_iter()
        .map(|session| {
            let runner = probe_runner(runtime, &session);
            json!({
                "sessionId": session.session_id,
                "cardId": session.card_id,
                "projectId": session.project_id,
                "title": session.title,
                "sessionGeneration": session.generation,
                "controlOwner": session.control_owner,
                "controlHolder": session.control_holder,
                "controlEpoch": session.control_epoch,
                "controlSequence": session.control_sequence,
                "activeJob": runner.as_ref().map(|probe| probe.job.clone()),
                "foreground": runner.as_ref().is_some_and(|probe| !probe.job.is_null()).then_some("managed-job"),
                "stale": runner.as_ref().is_none_or(|probe| !probe.current)
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({"ok":true,"sessions":values,"nextCreateSequence":next_create_sequence}))
}

pub(super) fn session_create(
    runtime: &Runtime,
    client_id: &str,
    arguments: Value,
) -> Result<Value, Value> {
    let args: CreateArgs = parse(arguments.clone())?;
    if !valid_id(&args.request_id)
        || !valid_id(&args.project_id)
        || args
            .title
            .as_deref()
            .is_some_and(|title| !valid_title(title))
    {
        return Err(error_value(
            "INVALID_ARGUMENTS",
            "request, project, or title is invalid",
            "Use ids returned by Deck and a short printable title.",
        ));
    }
    let hash = request_hash("deck_session_create", &args);
    let title = args.title.clone().unwrap_or_else(|| "MCP shell".into());
    let operation = runtime
        .write(|doc| {
            client(doc, client_id)?;
            // Request-key lookup, fingerprint comparison, capacity reservation,
            // and insertion are one persisted transaction. A separate read here
            // allowed two concurrent callers to reserve two create plans.
            if let Some(existing) = existing_operation(doc, client_id, &args.request_id, &hash)? {
                return Ok(existing.clone());
            }
            // A create must name the client's current create sequence, which
            // advances on every accepted create: a create whose record left
            // the result window can never be accepted again.
            if args.create_sequence != client(doc, client_id)?.create_sequence {
                return Err(DeckError::new(ErrorKind::ContextChanged, STALE_REQUEST));
            }
            reserve_operation(runtime, doc, client_id, Slot::Ordinary)?;
            if doc.sessions.len() >= MAX_SESSIONS {
                return Err(DeckError::new(
                    ErrorKind::DiskFull,
                    "MCP operation capacity reached",
                ));
            }
            // Accepted creates reserve a future managed-session slot.
            let reserved = doc
                .operations
                .iter()
                .filter(|operation| {
                    operation.kind == "session-create"
                        && matches!(operation.state.as_str(), "accepted" | "executing")
                })
                .count();
            if doc.sessions.len().saturating_add(reserved) >= MAX_SESSIONS {
                return Err(DeckError::new(
                    ErrorKind::DiskFull,
                    "MCP managed-session capacity reached",
                ));
            }
            let client = client(doc, client_id)?;
            if !client.allow_create {
                return Err(DeckError::new(
                    ErrorKind::Perm,
                    "client may not create sessions",
                ));
            }
            let project = scoped_project(client, &args.project_id)?;
            let cwd = canonical_scope(&args.cwd, &project.roots)?;
            let operation_id = random_id("op_", 16)?;
            let card_id = random_id("M", 16)?;
            let session_id = random_id("mcp_", 16)?;
            let generation = random_id("g_", 16)?;
            let socket = crate::datadir::deck_dir()
                .join("mcp-runners")
                .join(format!("{generation}.sock"));
            let result = json!({
                "cardId": card_id,
                "sessionId": &session_id,
                "projectId": args.project_id,
                "title": title,
                "cwd": cwd,
                "generation": generation,
                "controlSequence": 0,
                "runnerSocket": socket
            });
            let operation = Operation {
                operation_id,
                client_id: client_id.into(),
                request_id: args.request_id.clone(),
                request_hash: hash,
                kind: "session-create".into(),
                state: "accepted".into(),
                code: None,
                result: Some(result),
                accepted_at: now_ms(),
                updated_at: now_ms(),
                admission_hash: None,
                session_id: Some(session_id),
                control_epoch: None,
                control_sequence: None,
            };
            if let Some(client) = doc
                .config
                .clients
                .iter_mut()
                .find(|client| client.id == client_id)
            {
                client.create_sequence = client.create_sequence.saturating_add(1);
            }
            doc.operations.push(operation.clone());
            Ok(operation)
        })
        .map_err(map_error)?;
    emit_changed(runtime);
    Ok(operation_view(&operation))
}

pub(super) fn operation_get(
    runtime: &Runtime,
    client_id: &str,
    arguments: Value,
) -> Result<Value, Value> {
    let args: OperationArgs = parse(arguments)?;
    runtime
        .read(|doc| {
            client(doc, client_id)?;
            doc.operations
                .iter()
                .find(|operation| {
                    operation.operation_id == args.operation_id && operation.client_id == client_id
                })
                .map(operation_view)
                .ok_or_else(|| DeckError::new(ErrorKind::Missing, "operation not found"))
        })
        .map_err(map_error)?
        .map_err(map_error)
}

pub(super) fn inspect(
    runtime: &Runtime,
    client_id: &str,
    arguments: Value,
) -> Result<Value, Value> {
    let args: SessionArgs = parse(arguments)?;
    let session = runtime
        .read(|doc| authorized_session(doc, client_id, &args.session_id).cloned())
        .map_err(map_error)?
        .map_err(map_error)?;
    let runner = probe_runner(runtime, &session);
    // The execution fence is read BEFORE the grants: a revocation that has
    // fenced but not yet persisted already refuses exec and stdin, so its
    // grant reads `revoked` here, and once it persists the grant itself
    // reads `revoked` — the view never returns to `active` in between.
    let (emergency_human, execution_fenced) = {
        let emergency = runtime.emergency.lock_or_recover();
        (
            emergency.human_sessions.contains(&session.session_id),
            emergency.execution_fenced(&session.session_id),
        )
    };
    let (
        authorization_status,
        authorization_expiry,
        stdin_approved,
        job_bindings_open,
        job_bindings_closed,
    ) = runtime
        .read(|doc| {
            let authorization = match execution_authorization(runtime, doc, client_id, &session) {
                ExecutionAuthorization::None => ("none", None, false),
                ExecutionAuthorization::Active(grant) if execution_fenced => {
                    ("revoked", Some(grant.expires_at), false)
                }
                ExecutionAuthorization::Active(grant) => {
                    ("active", Some(grant.expires_at), grant.allow_stdin)
                }
                ExecutionAuthorization::Expired(grant) => {
                    ("expired", Some(grant.expires_at), false)
                }
                ExecutionAuthorization::Revoked(grant) => {
                    ("revoked", Some(grant.expires_at), false)
                }
            };
            let bindings = doc.jobs.iter().filter(|job| {
                job.client_id == client_id
                    && job.session_id == session.session_id
                    && job.session_generation == session.generation
            });
            let (open, closed) = bindings.fold((0u64, 0u64), |(open, closed), job| {
                if job.allow_output {
                    (open + 1, closed)
                } else {
                    (open, closed + 1)
                }
            });
            Ok((
                authorization.0,
                authorization.1,
                authorization.2,
                open,
                closed,
            ))
        })
        .map_err(map_error)?
        .map_err(map_error)?;
    let job = runner
        .as_ref()
        .map(|probe| probe.job.clone())
        .unwrap_or(Value::Null);
    let denial = if runner.is_none() {
        Some("RUNNER_UNAVAILABLE")
    } else if runner.as_ref().is_some_and(|probe| !probe.current) {
        Some("RUNNER_STALE")
    } else if session.closing {
        Some("TARGET_CLOSING")
    } else if session.human_lock || emergency_human {
        Some("HUMAN_CONTROL")
    } else if session.control_owner.as_deref() != Some(client_id) {
        Some("CONTROL_REVOKED")
    } else if args.holder_id.as_deref() != session.control_holder.as_deref() {
        Some("HOLDER_MISMATCH")
    } else if session
        .lease_expires_at
        .is_none_or(|deadline| deadline <= now_ms())
    {
        Some("CONTROL_LEASE_EXPIRED")
    } else if !job.is_null() {
        Some("SESSION_BUSY")
    } else if authorization_status != "active" {
        Some("EXECUTION_GRANT_REQUIRED")
    } else {
        None
    };
    // Terminal screen content is deliberately NOT returned: output reaches a
    // client only through deck_job_read, gated per job binding.
    Ok(json!({
        "ok": true,
        "sessionId": session.session_id,
        "sessionGeneration": session.generation,
        "controlOwner": session.control_owner,
        "controlHolder": session.control_holder,
        "controlEpoch": session.control_epoch,
        "controlSequence": session.control_sequence,
        "leaseExpiresAt": session.lease_expires_at,
        "humanLock": session.human_lock || emergency_human,
        "executionAuthorization": {
            "status": authorization_status,
            "active": authorization_status == "active",
            "expiresAtUnixMs": authorization_expiry,
            "stdinApprovedForActiveGrant": stdin_approved
        },
        "outputSharing": {
            "sessionGateOpen": session.output_shared && !session.human_lock && !emergency_human,
            "independentOfExecutionAuthorization": true,
            "jobBindingsOpen": job_bindings_open,
            "jobBindingsClosed": job_bindings_closed
        },
        "activeJob": job,
        "foreground": (!job.is_null()).then_some("managed-job"),
        "stale": runner.as_ref().is_none_or(|probe| !probe.current),
        "runnerVersion": runner.as_ref().and_then(|probe| probe.version.clone()),
        "mayStartNextJob": denial.is_none(),
        "mayStartNextJobReason": denial,
    }))
}

pub(super) fn session_control(
    runtime: &Runtime,
    client_id: &str,
    arguments: Value,
) -> Result<Value, Value> {
    let args: ControlArgs = parse(arguments)?;
    if !valid_id(&args.request_id) || !valid_id(&args.holder_id) {
        return Err(error_value(
            "INVALID_ARGUMENTS",
            "request id is invalid",
            "Use a stable opaque request id.",
        ));
    }
    if let Some(lease_ms) = args.lease_ms {
        if !(MIN_LEASE_MS..=MAX_LEASE_MS).contains(&lease_ms) {
            return Err(error_value(
                "INVALID_ARGUMENTS",
                "lease_ms is outside the allowed range",
                "Use an integer from 1000 through 300000 milliseconds.",
            ));
        }
    }
    match args.action {
        ControlAction::Request if args.control_epoch.is_some() => {
            return Err(error_value(
                "INVALID_ARGUMENTS",
                "control_epoch is not allowed for request",
                "Omit control_epoch when requesting a new control lease.",
            ));
        }
        ControlAction::Renew | ControlAction::Release if args.control_epoch.is_none() => {
            return Err(error_value(
                "INVALID_ARGUMENTS",
                "control_epoch is required for this action",
                "Use the current epoch returned by a successful control response.",
            ));
        }
        ControlAction::Release if args.lease_ms.is_some() => {
            return Err(error_value(
                "INVALID_ARGUMENTS",
                "lease_ms is not allowed for release",
                "Omit lease_ms when releasing control.",
            ));
        }
        _ => {}
    }
    let hash = request_hash("deck_session_control", &args);
    let action = args.action;
    let _delivery = runtime.delivery.lock_or_recover();
    if let Some(denied) = emergency_denial(runtime, client_id, &args.session_id, Admission::Control)
    {
        return Err(denied);
    }
    // (operation, session, replayed)
    let (operation, session, replayed) = runtime
        .write(|doc| {
            client(doc, client_id)?;
            if let Some(existing) = existing_operation(doc, client_id, &args.request_id, &hash)? {
                // An exact replay returns the recorded decision only. It never
                // re-sends a runner control side effect: after a holder change
                // that would re-install a stale control context.
                let session = doc
                    .sessions
                    .iter()
                    .find(|session| session.session_id == args.session_id)
                    .cloned();
                return Ok((existing.clone(), session, true));
            }
            let session = doc
                .sessions
                .iter_mut()
                .find(|session| session.session_id == args.session_id && session.owner_client_id == client_id)
                .ok_or_else(|| DeckError::new(ErrorKind::Missing, "session not found"))?;
            if session.generation != args.expected_generation {
                return Err(DeckError::new(ErrorKind::ContextChanged, "session generation changed"));
            }
            // Every accepted change advances the sequence, and a request must
            // name the current value. A retry of a change whose record was
            // superseded or retired therefore fails here — it is never
            // applied a second time (a renew included: it cannot extend the
            // lease again).
            if args.control_sequence != session.control_sequence {
                return Err(DeckError::new(ErrorKind::ContextChanged, STALE_REQUEST));
            }
            match args.action {
                ControlAction::Request => {
                    let lease_active = session.lease_expires_at.is_some_and(|lease| lease > now_ms());
                    if session.human_lock
                        || (lease_active
                            && (session.control_owner.as_deref() != Some(client_id)
                                || session.control_holder.as_deref() != Some(&args.holder_id)))
                    {
                        return Err(DeckError::new(ErrorKind::Perm, "user owns terminal control"));
                    }
                    if !lease_active {
                        session.control_epoch = session.control_epoch.saturating_add(1);
                    }
                    session.control_owner = Some(client_id.into());
                    session.control_holder = Some(args.holder_id.clone());
                    session.lease_expires_at = Some(now_ms() + args.lease_ms.unwrap_or(DEFAULT_LEASE_MS));
                }
                ControlAction::Renew => {
                    // Changes only the lease deadline: never the epoch, and
                    // never an execution grant.
                    check_control(session, client_id, &args.expected_generation, args.control_epoch.unwrap_or(0), &args.holder_id)?;
                    session.lease_expires_at = Some(now_ms() + args.lease_ms.unwrap_or(DEFAULT_LEASE_MS));
                }
                ControlAction::Release => {
                    check_control(session, client_id, &args.expected_generation, args.control_epoch.unwrap_or(0), &args.holder_id)?;
                    session.control_owner = None;
                    session.control_holder = None;
                    session.lease_expires_at = None;
                    session.control_epoch = session.control_epoch.saturating_add(1);
                }
            }
            session.control_sequence = session.control_sequence.saturating_add(1);
            let result = json!({"sessionId":session.session_id,"sessionGeneration":session.generation,"controlOwner":session.control_owner,"controlHolder":session.control_holder,"controlEpoch":session.control_epoch,"controlSequence":session.control_sequence,"leaseExpiresAt":session.lease_expires_at});
            let session = session.clone();
            // The change is applied first: compaction then retires what it
            // supersedes (older control records, and an advanced epoch's
            // records), and the control pool is never taken by ordinary work.
            reserve_operation(runtime, doc, client_id, Slot::Control)?;
            let operation = Operation { operation_id: random_id("op_", 16)?, client_id: client_id.into(), request_id: args.request_id.clone(), request_hash: hash.clone(), kind: "session-control".into(), state: "committed".into(), code: None, result: Some(result), accepted_at: now_ms(), updated_at: now_ms(), admission_hash: None, session_id: Some(session.session_id.clone()), control_epoch: Some(session.control_epoch), control_sequence: Some(session.control_sequence) };
            doc.operations.push(operation.clone());
            audit(doc, "control-changed", AuditLink { principal_id: Some(client_id), session_id: Some(&session.session_id), operation_id: Some(&operation.operation_id), ..Default::default() })?;
            Ok((operation, Some(session), false))
        })
        .map_err(map_error)?;
    // A renew changes only Deck's lease deadline; the runner's fence (epoch,
    // holder) is unchanged, so nothing is sent.
    if replayed || matches!(action, ControlAction::Renew) {
        return Ok(operation_view(&operation));
    }
    let Some(session) = session else {
        return Ok(operation_view(&operation));
    };
    let mode = if matches!(action, ControlAction::Release) {
        "fenced"
    } else {
        "mcp"
    };
    match runner_control_result(send_runner_control(runtime, &session, mode)) {
        Ok(()) => Ok(operation_view(&operation)),
        Err(error) if error.message() == RUNNER_STALE => Err(map_error(error)),
        Err(_) => Err(error_value(
            "OPERATION_AMBIGUOUS",
            "the control decision is durable but the runner fence is unconfirmed",
            "Inspect the session locally; do not assume the prior holder can still run or is stopped.",
        )),
    }
}

pub(super) fn valid_direct_launch(executable: &str, arguments: &[String]) -> bool {
    !executable.is_empty()
        && executable.len() <= MAX_EXECUTABLE_BYTES
        && !executable.chars().any(char::is_control)
        && Path::new(executable).is_absolute()
        && arguments.len() <= MAX_ARGUMENTS
        && arguments.iter().all(|value| !value.as_bytes().contains(&0))
        && arguments.iter().map(String::len).sum::<usize>() <= MAX_ARGUMENT_BYTES
}

pub(super) fn session_close(
    runtime: &Runtime,
    client_id: &str,
    arguments: Value,
) -> Result<Value, Value> {
    let args: CloseArgs = parse(arguments)?;
    if !valid_id(&args.request_id) {
        return Err(error_value(
            "INVALID_ARGUMENTS",
            "request id is invalid",
            "Use a stable opaque request id.",
        ));
    }
    let hash = request_hash("deck_session_close", &args);
    let _delivery = runtime.delivery.lock_or_recover();
    if let Some(denied) = emergency_denial(runtime, client_id, &args.session_id, Admission::Control)
    {
        return Err(denied);
    }
    // Replay, authorization and control are checked first without I/O. The
    // runner ping happens OUTSIDE the state write: the doc lock also guards
    // every terminal keystroke (`guard_terminal_input`). No job can start
    // meanwhile because exec needs the delivery lock held here.
    let session = runtime
        .read(|doc| {
            client(doc, client_id)?;
            if let Some(existing) = existing_operation(doc, client_id, &args.request_id, &hash)? {
                return Ok(Err(existing.clone()));
            }
            let session = authorized_session(doc, client_id, &args.session_id)?.clone();
            check_control(
                &session,
                client_id,
                &args.expected_generation,
                args.control_epoch,
                &args.holder_id,
            )?;
            Ok(Ok(session))
        })
        .map_err(map_error)?
        .map_err(map_error)?;
    let session = match session {
        Ok(session) => session,
        Err(existing) => return Ok(operation_view(&existing)),
    };
    let runner = send_runner(runtime, &session, &json!({"kind":"ping"})).map_err(|_| {
        map_error(DeckError::new(
            ErrorKind::ContextChanged,
            "managed runner state is unknown; remote close is refused",
        ))
    })?;
    let active = runner.get("job").is_some_and(|job| !job.is_null());
    if active && !args.confirm_running {
        return Err(map_error(DeckError::new(
            ErrorKind::Locked,
            "session has a running job",
        )));
    }
    let operation = runtime.write(|doc| {
        client(doc, client_id)?;
        if let Some(existing) = existing_operation(doc, client_id, &args.request_id, &hash)? { return Ok(existing.clone()); }
        reserve_operation(runtime, doc, client_id, Slot::Ordinary)?;
        let current = authorized_session(doc, client_id, &args.session_id)?.clone();
        check_control(&current, client_id, &args.expected_generation, args.control_epoch, &args.holder_id)?;
        let managed = doc.sessions.iter_mut().find(|item| item.session_id == session.session_id).ok_or_else(|| DeckError::new(ErrorKind::Missing, "session not found"))?;
        managed.closing = true;
        let operation = Operation { operation_id:random_id("op_", 16)?, client_id:client_id.into(), request_id:args.request_id.clone(), request_hash:hash.clone(), kind:"session-close".into(), state:"accepted".into(), code:None, result:Some(json!({"sessionId":session.session_id,"cardId":session.card_id,"sessionGeneration":session.generation,"controlEpoch":session.control_epoch,"holderId":args.holder_id,"confirmRunning":args.confirm_running})), accepted_at:now_ms(), updated_at:now_ms(), admission_hash:None, session_id: Some(session.session_id.clone()), control_epoch: Some(args.control_epoch), control_sequence: None };
        doc.operations.push(operation.clone());
        Ok(operation)
    }).map_err(map_error)?;
    emit_changed(runtime);
    Ok(operation_view(&operation))
}
