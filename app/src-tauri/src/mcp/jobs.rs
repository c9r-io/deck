//! Job execution and output reads through the runner, and every job side effect.
//!
//! Split out of the one-file `mcp.rs` on 2026-09-23; the contract stays in
//! `mcp/mod.rs`.

use super::*;

pub(super) fn exec(runtime: &Runtime, client_id: &str, arguments: Value) -> Result<Value, Value> {
    let direct: DirectExecArgs = parse(arguments)?;
    let args = direct.common;
    let executable = direct.executable;
    let argv = direct.args;
    if !valid_id(&args.request_id)
        || !valid_direct_launch(&executable, &argv)
        || args.wait_ms.unwrap_or(DEFAULT_WAIT_MS) > MAX_WAIT_MS
        || args
            .execution_timeout_ms
            .is_some_and(|value| !(100..=24 * 60 * 60 * 1000).contains(&value))
    {
        return Err(error_value(
            "INVALID_ARGUMENTS",
            "exec arguments exceed their bounds",
            "Use a bounded executable and exact argument vector.",
        ));
    }
    // Fingerprinting is independent of JSON object key order. Raw arguments
    // are never journaled; only bounded metadata and digests
    // participate in the stable request fingerprint.
    let executable_digest = sha(executable.as_bytes());
    let argument_digest = sha(&serde_json::to_vec(&argv).unwrap_or_default());
    let argument_bytes = argv.iter().map(String::len).sum::<usize>();
    let launch_fingerprint = json!([
        "direct",
        executable_digest,
        argument_digest,
        argv.len(),
        argument_bytes
    ]);
    let launch_metadata = json!({"launchKind":"direct","executableDigest":executable_digest,"argumentDigest":argument_digest,"argumentCount":argv.len(),"argumentBytes":argument_bytes});
    let hash = sha(serde_json::to_vec(&json!([
        "deck-exec-v3",
        &args.request_id,
        &args.session_id,
        &args.expected_generation,
        args.control_epoch,
        &args.holder_id,
        &args.cwd,
        args.wait_ms,
        args.execution_timeout_ms,
        launch_fingerprint,
        ENVIRONMENT_PROFILE,
        POLICY_VERSION
    ]))
    .unwrap_or_default()
    .as_slice());
    let _delivery = runtime.delivery.lock_or_recover();
    if let Some(denied) =
        emergency_denial(runtime, client_id, &args.session_id, Admission::Execution)
    {
        return Err(denied);
    }
    let prepared = runtime
        .write(|doc| {
            let client = client(doc, client_id)?.clone();
            if let Some(existing) = existing_operation(doc, client_id, &args.request_id, &hash)? {
                let job = existing.result.as_ref().and_then(|result| result.get("jobId")).and_then(Value::as_str).and_then(|job_id| doc.jobs.iter().find(|job| job.job_id == job_id)).cloned();
                return Ok((existing.clone(), job, None));
            }
            reserve_operation(runtime, doc, client_id, Slot::Ordinary)?;
            let session = doc.sessions.iter().find(|session| session.session_id == args.session_id && session.owner_client_id == client_id).cloned().ok_or_else(|| DeckError::new(ErrorKind::Missing, "session not found"))?;
            // Keep a bounded window of bindings per session. The runner holds
            // at most one live job and retires finished jobs oldest-first, so
            // the oldest binding here is never the live one.
            let bound = doc.jobs.iter().filter(|job| job.session_id == session.session_id).count();
            if bound >= MAX_JOBS_PER_SESSION {
                if let Some(oldest) = doc.jobs.iter().position(|job| job.session_id == session.session_id) {
                    doc.jobs.remove(oldest);
                }
            }
            if doc.jobs.len() >= MAX_JOBS {
                return Err(DeckError::new(ErrorKind::DiskFull, "MCP operation capacity reached"));
            }
            let claim = ControlClaim { generation: &args.expected_generation, epoch: Some(args.control_epoch), holder_id: Some(&args.holder_id) };
            let grant = admit_exec(runtime, doc, client_id, &session, &claim, ExecStage::Accept).map_err(AdmissionReason::error)?.clone();
            let project = scoped_project(&client, &session.project_id)?;
            let cwd = canonical_scope(args.cwd.as_deref().unwrap_or(&session.cwd), &project.roots)?;
            let operation_id = random_id("op_", 16)?;
            let job_id = random_id("job_", 16)?;
            let binding = JobBinding { job_id: job_id.clone(), client_id: client_id.into(), session_id: session.session_id.clone(), session_generation: session.generation.clone(), request_hash: hash.clone(), operation_id: operation_id.clone(), grant_id: grant.grant_id.clone(), grant_version: grant.grant_version, allow_output: grant.allow_output };
            let operation = Operation { operation_id, client_id: client_id.into(), request_id: args.request_id.clone(), request_hash: hash.clone(), kind: "exec".into(), state: "accepted".into(), code: None, result: Some(json!({"jobId":job_id,"sessionId":session.session_id,"sessionGeneration":session.generation,"executionGrantId":grant.grant_id,"executionGrantVersion":grant.grant_version,"policyVersion":POLICY_VERSION,"environmentProfile":ENVIRONMENT_PROFILE,"launch":launch_metadata.clone(),"cwd":cwd})), accepted_at: now_ms(), updated_at: now_ms(), admission_hash: None, session_id: Some(session.session_id.clone()), control_epoch: Some(args.control_epoch), control_sequence: None };
            doc.jobs.push(binding.clone());
            doc.operations.push(operation.clone());
            audit(doc, "exec-intent", AuditLink { principal_id: Some(client_id), session_id: Some(&session.session_id), operation_id: Some(&operation.operation_id), job_id: Some(&binding.job_id), grant_id: Some(&grant.grant_id), reason_code: None })?;
            Ok((operation, Some(binding), Some((session, cwd, grant))))
        })
        .map_err(map_error)?;
    let (mut operation, binding, dispatch) = prepared;
    let Some(binding) = binding else {
        return Ok(operation_view(&operation));
    };
    let Some((session, cwd, _accepted_grant)) = dispatch else {
        let runner = send_runner(
            runtime,
            &runtime
                .read(|doc| authorized_session(doc, client_id, &binding.session_id).cloned())
                .map_err(map_error)?
                .map_err(map_error)?,
            &json!({"kind":"read","job_id":binding.job_id,"max_bytes":1,"wait_ms":0}),
        );
        if let Ok(value) = runner {
            return Ok(
                json!({"ok":true,"operationId":operation.operation_id,"jobId":binding.job_id,"state":value.get("job").and_then(|job| job.get("state")).cloned().unwrap_or(json!("unknown")),"initialOutput":"","outputCursor":format!("{}:{}:0",binding.session_generation,binding.job_id)}),
            );
        }
        return Ok(operation_view(&operation));
    };
    pause_point(runtime, "exec-final");
    // Final admission: the last step before the runner, still under the
    // delivery lock (see `emergency_denial` for the linearization). Control,
    // the SAME execution grant this intent was accepted under (a re-approval
    // never adopts an older intent), and every emergency fence — a pending
    // execution revocation included — must hold. Otherwise nothing is sent
    // and the intent is rejected with the refusal's code.
    let admitted = runtime
        .read(|doc| {
            let current = authorized_session(doc, client_id, &session.session_id)?;
            let claim = ControlClaim {
                generation: &args.expected_generation,
                epoch: Some(args.control_epoch),
                holder_id: Some(&args.holder_id),
            };
            let grant = admit_exec(runtime, doc, client_id, current, &claim, ExecStage::Final)
                .map_err(AdmissionReason::error)?;
            if grant.grant_id != binding.grant_id || grant.grant_version != binding.grant_version {
                return Err(DeckError::new(
                    ErrorKind::Perm,
                    "the execution window changed; a local execution grant is required",
                ));
            }
            Ok(grant.clone())
        })
        .map_err(map_error)
        .and_then(|admitted| admitted.map_err(map_error))
        .and_then(|grant| {
            match emergency_denial(
                runtime,
                client_id,
                &session.session_id,
                Admission::Execution,
            ) {
                Some(denied) => Err(denied),
                None => Ok(grant),
            }
        });
    let runner = admitted.as_ref().ok().map(|grant| {
        let context = json!({
            "service_instance": runtime.service_instance,
            "holder_id": args.holder_id,
            "control_epoch": args.control_epoch,
            "grant_id": grant.grant_id,
            "grant_version": grant.grant_version,
            "policy_version": POLICY_VERSION,
            "intent_hash": hash,
            "expires_at": grant.expires_at,
        });
        let authorization = json!({
            "kind":"authorize-grant",
            "service_instance":runtime.service_instance,
            "grant_id":grant.grant_id,
            "grant_version":grant.grant_version,
            "policy_version":POLICY_VERSION,
            "expires_at":grant.expires_at,
        });
        let authorized = send_runner(runtime, &session, &authorization)?;
        if authorized.get("ok").and_then(Value::as_bool) != Some(true) {
            return Ok(authorized);
        }
        let request = json!({"kind":"exec","job_id":binding.job_id,"request_hash":hash,"executable":executable,"args":argv,"cwd":cwd,"wait_ms":args.wait_ms.unwrap_or(DEFAULT_WAIT_MS),"timeout_ms":args.execution_timeout_ms,"context":context});
        send_runner(runtime, &session, &request)
    });
    let reply = runner.as_ref().and_then(|runner| runner.as_ref().ok());
    let committed =
        reply.is_some_and(|value| value.get("ok").and_then(Value::as_bool) == Some(true));
    // A runner error given before the process could start is a rejection;
    // one given after it may have started keeps the record ambiguous under
    // the error's own code; anything unrecognised is ambiguous too.
    let runner_failure = reply.and_then(runner_error);
    let rejected = runner_failure.filter(|(class, ..)| *class == RunnerErrorClass::Rejection);
    let refused = admitted.as_ref().err().map(|denied| {
        denied["error"]["code"]
            .as_str()
            .unwrap_or("ADMISSION_REFUSED")
            .to_owned()
    });
    let audit_grant_id = operation
        .result
        .as_ref()
        .and_then(|result| result.get("executionGrantId"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let journaled = runtime.write(|doc| {
        if let Some(saved) = doc
            .operations
            .iter_mut()
            .find(|saved| saved.operation_id == operation.operation_id)
        {
            saved.state = if committed {
                "committed"
            } else if rejected.is_some() || refused.is_some() {
                "rejected"
            } else {
                "ambiguous"
            }
            .into();
            saved.code = if committed {
                None
            } else if let Some(code) = &refused {
                Some(code.clone())
            } else {
                Some(
                    runner_failure
                        .map(|(_, code, _)| code)
                        .unwrap_or("dispatch-unknown")
                        .into(),
                )
            };
            saved.updated_at = now_ms();
            operation = saved.clone();
        }
        if rejected.is_some() || refused.is_some() {
            // The runner never created this job: drop its binding.
            doc.jobs.retain(|job| job.job_id != binding.job_id);
        }
        audit(
            doc,
            "exec-dispatch",
            AuditLink {
                principal_id: Some(client_id),
                session_id: Some(&session.session_id),
                operation_id: Some(&operation.operation_id),
                job_id: Some(&binding.job_id),
                grant_id: audit_grant_id.as_deref(),
                reason_code: (!committed).then_some(if refused.is_some() {
                    "admission-refused"
                } else if rejected.is_some() {
                    "runner-rejected"
                } else {
                    "dispatch-unknown"
                }),
            },
        )?;
        Ok(())
    });
    if journaled.is_err() {
        if let Err(denied) = admitted {
            // Nothing was dispatched; the unrecorded intent turns ambiguous
            // on restart and is never replayed.
            return Err(denied);
        }
        return Err(error_value(
            "OPERATION_AMBIGUOUS",
            "job dispatch occurred but its audit result could not be persisted",
            "Inspect the live job; do not re-execute it.",
        ));
    }
    let runner = match admitted {
        Err(denied) => return Err(denied),
        Ok(_) => runner.unwrap_or_else(|| Err(DeckError::new(ErrorKind::Other, "not dispatched"))),
    };
    match runner {
        Ok(value) if committed => Ok(
            json!({"ok":true,"operationId":operation.operation_id,"jobId":binding.job_id,"sessionId":session.session_id,"sessionGeneration":session.generation,"state":value.get("job").and_then(|job| job.get("state")).cloned().unwrap_or(json!("unknown")),"initialOutput":"","outputCursor":format!("{}:{}:0",session.generation,binding.job_id)}),
        ),
        Ok(_) if rejected.is_some() => {
            let (_, code, next) = rejected.unwrap();
            Err(error_value(code, "managed runner rejected the job", next))
        }
        Ok(_) if runner_failure.is_some() => {
            let (_, code, next) = runner_failure.unwrap();
            Err(error_value(
                code,
                "Deck cannot prove whether the job started",
                next,
            ))
        }
        _ => Err(error_value(
            "OPERATION_AMBIGUOUS",
            "Deck cannot prove whether job dispatch completed",
            "Inspect the operation and job; do not resubmit the script under a new request id.",
        )),
    }
}

pub(super) fn decode_cursor(cursor: Option<String>, binding: &JobBinding) -> Result<u64, Value> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    let expected = format!("{}:{}:", binding.session_generation, binding.job_id);
    cursor
        .strip_prefix(&expected)
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| {
            error_value(
                "OUTPUT_CURSOR_INVALID",
                "cursor does not belong to this job generation",
                "Restart reading with no cursor; a gap may be reported.",
            )
        })
}

/// Evaluate the complete control-side read gate for one retained job binding.
/// This is deliberately independent of the execution grant and control lease.
/// Keep the failure codes distinct: reopening the session gate can fix a
/// session pause, but it can never revive a binding closed by takeover.
pub(super) fn job_read_gate(
    runtime: &Runtime,
    client_id: &str,
    job_id: &str,
) -> Result<(JobBinding, ManagedSession), Value> {
    let (binding, session) = runtime
        .read(|doc| {
            client(doc, client_id)?;
            let binding = doc
                .jobs
                .iter()
                .find(|job| job.job_id == job_id && job.client_id == client_id)
                .cloned()
                .ok_or_else(|| DeckError::new(ErrorKind::Missing, "job not found"))?;
            let session = authorized_session(doc, client_id, &binding.session_id)?.clone();
            Ok((binding, session))
        })
        .map_err(map_error)?
        .map_err(map_error)?;
    if let Some(denied) =
        emergency_denial(runtime, client_id, &session.session_id, Admission::Control)
    {
        return Err(denied);
    }
    if session.human_lock {
        return Err(error_value(
            "HUMAN_CONTROL",
            "the local user controls this session",
            "Wait for the local user to explicitly return control.",
        ));
    }
    if session.generation != binding.session_generation {
        return Err(map_error(DeckError::new(
            ErrorKind::ContextChanged,
            "session generation changed",
        )));
    }
    if !binding.allow_output {
        return Err(error_value(
            "JOB_OUTPUT_BINDING_CLOSED",
            "this job's output-sharing binding is closed",
            "This historical binding cannot be restored. Start a new job while session output sharing is open.",
        ));
    }
    if !session.output_shared {
        return Err(error_value(
            "SESSION_OUTPUT_SHARING_PAUSED",
            "session output sharing is paused",
            "Ask the local Deck user to approve output sharing for this session.",
        ));
    }
    Ok((binding, session))
}

pub(super) fn job_read(
    runtime: &Runtime,
    client_id: &str,
    arguments: Value,
) -> Result<Value, Value> {
    let args: ReadArgs = parse(arguments)?;
    // The same output gate runs before the runner read and again after it
    // returns (the read may wait up to 5 s). Bytes read across a takeover,
    // sharing pause, revocation or generation change are dropped. The gate is
    // the job binding plus the session's independent sharing switch — never
    // the execution grant.
    let gate = || job_read_gate(runtime, client_id, &args.job_id);
    let (binding, session) = gate()?;
    let cursor = decode_cursor(args.cursor, &binding)?;
    let max_bytes = args.max_bytes.unwrap_or(MAX_READ_BYTES);
    if !(4..=MAX_READ_BYTES).contains(&max_bytes) || args.wait_ms.unwrap_or(0) > MAX_WAIT_MS {
        return Err(error_value(
            "INVALID_ARGUMENTS",
            "read limits are invalid",
            "Use max_bytes 4..16384 and wait_ms up to 5000.",
        ));
    }
    let value = send_runner(runtime, &session, &json!({"kind":"read","job_id":binding.job_id,"cursor":cursor,"max_bytes":max_bytes,"wait_ms":args.wait_ms.unwrap_or(0)})).map_err(map_error)?;
    // Linearization: authority is judged at return time. Output already
    // delivered by an earlier call cannot be recalled.
    gate()?;
    if let Some((_, code, next)) = runner_error(&value) {
        return Err(error_value(code, "managed runner refused the read", next));
    }
    if value.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(error_value(
            "JOB_STATE_UNKNOWN",
            "managed runner could not read the job",
            "Inspect the session; do not re-execute the command.",
        ));
    }
    let next = value
        .get("nextCursor")
        .and_then(Value::as_u64)
        .unwrap_or(cursor);
    Ok(
        json!({"ok":true,"sessionId":session.session_id,"sessionGeneration":session.generation,"jobId":binding.job_id,"state":value.get("job").and_then(|job| job.get("state")).cloned().unwrap_or(json!("unknown")),"exitCode":value.get("job").and_then(|job| job.get("exitCode")).cloned().unwrap_or(Value::Null),"terminationSignal":value.get("job").and_then(|job| job.get("terminationSignal")).cloned().unwrap_or(Value::Null),"interruptRequested":value.get("job").and_then(|job| job.get("interruptRequested")).cloned().unwrap_or(json!(false)),"timeoutRequested":value.get("job").and_then(|job| job.get("timeoutRequested")).cloned().unwrap_or(json!(false)),"processComplete":matches!(value.get("job").and_then(|job| job.get("state")).and_then(Value::as_str),Some("exited" | "lost")),"stdoutEof":value.get("job").and_then(|job| job.get("stdoutEof")).cloned().unwrap_or(json!(false)),"stderrEof":value.get("job").and_then(|job| job.get("stderrEof")).cloned().unwrap_or(json!(false)),"outputComplete":value.get("job").and_then(|job| job.get("outputComplete")).cloned().unwrap_or(json!(false)),"output":value.get("output").cloned().unwrap_or(json!("")),"outputKind":"pty_combined","nextCursor":format!("{}:{}:{}",session.generation,binding.job_id,next),"truncated":value.get("gap").cloned().unwrap_or(json!(false)),"gap":value.get("gap").cloned().unwrap_or(json!(false)),"droppedBytes":value.get("droppedBytes").cloned().unwrap_or(json!(0)),"startedAt":value.get("job").and_then(|job| job.get("startedAt")).cloned().unwrap_or(Value::Null),"endedAt":value.get("job").and_then(|job| job.get("endedAt")).cloned().unwrap_or(Value::Null),"nextAction":if value.get("job").and_then(|job| job.get("outputComplete")).and_then(Value::as_bool)==Some(true) {"Evaluate the exit code and output; exit 0 alone does not prove the requested work is correct."} else {"Continue reading with nextCursor; process exit does not imply that all output has reached EOF."}}),
    )
}

pub(super) fn job_side_effect(
    runtime: &Runtime,
    client_id: &str,
    tool: &str,
    arguments: Value,
) -> Result<Value, Value> {
    let (request_id, job_id, generation, epoch, holder_id, input, hash) =
        if tool == "deck_job_input" {
            let args: InputArgs = parse(arguments)?;
            if args.input.len() > MAX_INPUT_BYTES {
                return Err(error_value(
                    "INVALID_ARGUMENTS",
                    "input exceeds its bound",
                    "Send a smaller input chunk.",
                ));
            }
            let hash = request_hash(tool, &args);
            (
                args.request_id,
                args.job_id,
                args.session_generation,
                args.control_epoch,
                args.holder_id,
                Some(args.input),
                hash,
            )
        } else {
            let args: InterruptArgs = parse(arguments)?;
            let hash = request_hash(tool, &args);
            (
                args.request_id,
                args.job_id,
                args.session_generation,
                args.control_epoch,
                args.holder_id,
                None,
                hash,
            )
        };
    if !valid_id(&request_id) {
        return Err(error_value(
            "INVALID_ARGUMENTS",
            "request id is invalid",
            "Use a stable opaque request id.",
        ));
    }
    let _delivery = runtime.delivery.lock_or_recover();
    let prepared = runtime
        .write(|doc| {
            client(doc, client_id)?;
            if let Some(existing) = existing_operation(doc, client_id, &request_id, &hash)? {
                return Ok((existing.clone(), None));
            }
            // Interrupt may use the reserved slots: stopping work must not
            // fail because ordinary requests filled the journal.
            reserve_operation(
                runtime,
                doc,
                client_id,
                if input.is_none() {
                    Slot::Interrupt
                } else {
                    Slot::Ordinary
                },
            )?;
            let binding = doc
                .jobs
                .iter()
                .find(|job| job.job_id == job_id && job.client_id == client_id)
                .cloned()
                .ok_or_else(|| DeckError::new(ErrorKind::Missing, "job not found"))?;
            let session = authorized_session(doc, client_id, &binding.session_id)?.clone();
            check_control(&session, client_id, &generation, epoch, &holder_id)?;
            let grant = doc
                .execution_grants
                .iter()
                .find(|grant| {
                    grant.grant_id == binding.grant_id
                        && grant.grant_version == binding.grant_version
                        && grant.client_id == client_id
                        && grant.session_id == session.session_id
                        && grant.session_generation == session.generation
                        && grant.service_instance == runtime.service_instance
                })
                .cloned()
                .ok_or_else(|| DeckError::new(ErrorKind::ContextChanged, "job grant changed"))?;
            if input.is_some()
                && (grant_standing(runtime, doc, &grant) != GrantStanding::Active
                    || !grant.allow_stdin)
            {
                return Err(DeckError::new(
                    ErrorKind::Perm,
                    "interactive stdin is not locally authorized",
                ));
            }
            let operation = Operation {
                operation_id: random_id("op_", 16)?,
                client_id: client_id.into(),
                request_id,
                request_hash: hash,
                kind: if input.is_some() {
                    "job-input"
                } else {
                    "job-interrupt"
                }
                .into(),
                state: "accepted".into(),
                code: None,
                result: Some(json!({"jobId":job_id,"sessionId":session.session_id})),
                accepted_at: now_ms(),
                updated_at: now_ms(),
                admission_hash: None,
                session_id: Some(session.session_id.clone()),
                control_epoch: Some(epoch),
                control_sequence: None,
            };
            doc.operations.push(operation.clone());
            Ok((operation, Some((binding, session, grant))))
        })
        .map_err(map_error)?;
    let (operation, target) = prepared;
    let Some((binding, session, grant)) = target else {
        return Ok(operation_view(&operation));
    };
    pause_point(runtime, "side-effect-final");
    // Final admission under the delivery lock (see `emergency_denial`): stdin
    // is refused by a pending execution revocation; an interrupt is not — it
    // stops work and needs only its own client, control and job authority.
    let admission = if input.is_some() {
        Admission::Execution
    } else {
        Admission::Control
    };
    if let Some(denied) = emergency_denial(runtime, client_id, &session.session_id, admission) {
        let code = denied["error"]["code"]
            .as_str()
            .unwrap_or("ADMISSION_REFUSED")
            .to_owned();
        let _ = runtime.write(|doc| {
            if let Some(saved) = doc
                .operations
                .iter_mut()
                .find(|saved| saved.operation_id == operation.operation_id)
            {
                saved.state = "rejected".into();
                saved.code = Some(code);
                saved.updated_at = now_ms();
            }
            Ok(())
        });
        return Err(denied);
    }
    let context = json!({
        "service_instance": runtime.service_instance,
        "holder_id": holder_id,
        "control_epoch": epoch,
        "grant_id": grant.grant_id,
        "grant_version": grant.grant_version,
        "policy_version": POLICY_VERSION,
        "intent_hash": binding.request_hash,
        "expires_at": if input.is_some() { grant.expires_at } else { u64::MAX },
    });
    let request = if let Some(input) = input {
        json!({"kind":"input","job_id":binding.job_id,"data_b64":base64::engine::general_purpose::STANDARD.encode(input.as_bytes()),"context":context})
    } else {
        json!({"kind":"interrupt","job_id":binding.job_id,"context":context})
    };
    let result = send_runner(runtime, &session, &request);
    let committed = result
        .as_ref()
        .ok()
        .is_some_and(|value| value.get("ok").and_then(Value::as_bool) == Some(true));
    let runner_failure = result.as_ref().ok().and_then(runner_error);
    let rejected = runner_failure.filter(|(class, ..)| *class == RunnerErrorClass::Rejection);
    let saved = runtime
        .write(|doc| {
            let saved = doc
                .operations
                .iter_mut()
                .find(|saved| saved.operation_id == operation.operation_id)
                .ok_or_else(|| DeckError::new(ErrorKind::Missing, "operation not found"))?;
            saved.state = if committed {
                "committed"
            } else if rejected.is_some() {
                "rejected"
            } else {
                "ambiguous"
            }
            .into();
            saved.code = if committed {
                None
            } else {
                Some(
                    runner_failure
                        .map(|(_, code, _)| code)
                        .unwrap_or("delivery-unknown")
                        .into(),
                )
            };
            saved.updated_at = now_ms();
            Ok(saved.clone())
        })
        .map_err(map_error)?;
    if committed {
        Ok(operation_view(&saved))
    } else if let Some((class, code, next)) = runner_failure {
        Err(error_value(
            code,
            if class == RunnerErrorClass::Rejection {
                "managed runner rejected the job side effect"
            } else {
                "Deck cannot confirm the job side effect"
            },
            next,
        ))
    } else {
        Err(error_value(
            "OPERATION_AMBIGUOUS",
            "Deck cannot confirm the job side effect",
            "Read the job and operation before deciding what to do next.",
        ))
    }
}
