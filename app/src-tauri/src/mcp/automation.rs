//! Automation delegation: the two tools a delegated client uses to name one
//! of the user's Agent rules and hand over one item, the acceptance
//! transaction with the agent inbox (`inbound_agent.rs`), the outcome
//! write-back, and the delegation set the scheduler's authority reads.
//!
//! The contract lives in `mcp/mod.rs` ("Automation delegation"). In short:
//! a client may call these only with `allow_automations` (a separate,
//! revocable local choice, never implied by session creation); a trigger
//! names an Agent rule by its automation id and the rule's current
//! `revision`, and is refused (`RULE_CHANGED`, with the current revision)
//! when the rule or its template moved; the inbox entry is written before
//! the operation and the client's `trigger_sequence` are persisted, and
//! marked committed after; a replay returns the recorded operation; the
//! dispatcher creates the card and the webview's acknowledgment reaches
//! `automation_outcome`. A trigger operation's truth after a Deck restart is
//! its inbox entry, so `load` leaves it `accepted` and `reconcile_inbox`
//! commits or settles it from the inbox.

use super::*;

/// Closed message constants the webview and the adapter map to one code each.
pub(super) const AUTOMATION_NOT_FOUND: &str = "mcp-automation-not-found";
pub(super) const AUTOMATION_UNAVAILABLE: &str = "mcp-automation-unavailable";
pub(super) const TRIGGER_KIND: &str = "automation-trigger";

/// The rules a delegated client may name now: enabled Agent rules whose
/// command passes the remote admission, each with its current revision
/// (`None` when the Board or the template cannot answer, in which case the
/// rule is listed but cannot be triggered).
fn automations_now(config: &crate::inbound::Config) -> Vec<Value> {
    config
        .rules
        .iter()
        .filter(|rule| {
            rule.source == crate::inbound_agent::SOURCE
                && rule.enabled
                && crate::admission::channel_agent_command(&rule.cmd).is_some()
        })
        .map(|rule| {
            json!({
                "id": rule.badge,
                "name": rule.name,
                "projectId": rule.project_id,
                "revision": crate::inbound_agent::current_revision(rule),
            })
        })
        .collect()
}

fn delegated<'a>(doc: &'a DiskDoc, client_id: &str) -> Result<&'a Client, DeckError> {
    let client = client(doc, client_id)?;
    if !client.allow_automations {
        return Err(DeckError::new(
            ErrorKind::Perm,
            "client may not trigger automations",
        ));
    }
    Ok(client)
}

/// The current settings, the one source of the rules (`None`: unreadable).
fn automation_config() -> Option<crate::inbound::Config> {
    #[cfg(test)]
    if let Some(forced) = CONFIG_FOR_TESTS.with(|cell| cell.borrow().clone()) {
        return forced;
    }
    crate::inbound::read_config_strict()
}

#[cfg(test)]
thread_local! {
    /// A test's settings in place of the settings file (`Some(answer)`).
    pub(super) static CONFIG_FOR_TESTS: std::cell::RefCell<Option<Option<crate::inbound::Config>>> =
        const { std::cell::RefCell::new(None) };
}

pub(super) fn automation_list(
    runtime: &Runtime,
    client_id: &str,
    arguments: Value,
) -> Result<Value, Value> {
    parse::<Empty>(arguments)?;
    let next = runtime
        .read(|doc| delegated(doc, client_id).map(|client| client.trigger_sequence))
        .map_err(map_error)?
        .map_err(map_error)?;
    let config = automation_config().ok_or_else(|| {
        map_error(DeckError::new(
            ErrorKind::ContextChanged,
            AUTOMATION_UNAVAILABLE,
        ))
    })?;
    Ok(json!({
        "ok": true,
        "automations": automations_now(&config),
        "inputFields": ["text", "from", "where", "link"],
        "nextTriggerSequence": next
    }))
}

fn sha_hex(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub(super) fn automation_trigger(
    runtime: &Runtime,
    client_id: &str,
    arguments: Value,
) -> Result<Value, Value> {
    let args: TriggerArgs = parse(arguments)?;
    if !valid_id(&args.request_id)
        || !crate::inbound::valid_badge(&args.automation_id)
        || !sha_hex(&args.expected_revision)
        || args.text.trim().is_empty()
        || args.text.len() > crate::inbound_agent::MAX_TEXT_BYTES
        || [&args.from, &args.where_, &args.link].iter().any(|field| {
            field
                .as_ref()
                .is_some_and(|f| f.len() > crate::inbound_agent::MAX_FIELD_BYTES)
        })
    {
        return Err(error_value(
            "INVALID_ARGUMENTS",
            "request id, automation id, revision or item is out of shape or bounds",
            "Use an automation id and revision from deck_automation_list; text at most 16 KiB, each context field at most 1 KiB.",
        ));
    }
    let hash = request_hash("deck_automation_trigger", &args);
    // A replay returns the recorded operation; its inbox entry is committed
    // on the way in case the crash was between the two writes.
    let replay = runtime
        .read(|doc| {
            delegated(doc, client_id)?;
            Ok(existing_operation(doc, client_id, &args.request_id, &hash)?.cloned())
        })
        .map_err(map_error)?
        .map_err(map_error)?;
    if let Some(operation) = replay {
        settle_committed(client_id, &args.request_id, &operation.operation_id);
        return Ok(operation_view(&operation));
    }
    // Pre-checks under the read lock; the authoritative sequence check is
    // in the write below.
    let stale = runtime
        .read(|doc| {
            delegated(doc, client_id).map(|client| client.trigger_sequence != args.trigger_sequence)
        })
        .map_err(map_error)?
        .map_err(map_error)?;
    if stale {
        return Err(map_error(DeckError::new(
            ErrorKind::ContextChanged,
            STALE_REQUEST,
        )));
    }
    let config = automation_config().ok_or_else(|| {
        map_error(DeckError::new(
            ErrorKind::ContextChanged,
            AUTOMATION_UNAVAILABLE,
        ))
    })?;
    let rule = config
        .rules
        .iter()
        .find(|rule| {
            rule.source == crate::inbound_agent::SOURCE
                && rule.badge == args.automation_id
                && rule.enabled
                && crate::admission::channel_agent_command(&rule.cmd).is_some()
        })
        .ok_or_else(|| map_error(DeckError::new(ErrorKind::Missing, AUTOMATION_NOT_FOUND)))?;
    let revision = crate::inbound_agent::current_revision(rule).ok_or_else(|| {
        map_error(DeckError::new(
            ErrorKind::ContextChanged,
            AUTOMATION_UNAVAILABLE,
        ))
    })?;
    if revision != args.expected_revision {
        return Err(json!({"ok": false, "error": {
            "code": "RULE_CHANGED",
            "message": "the automation's rule or template is not the revision this request expected; nothing was accepted",
            "nextAction": "Read deck_automation_list again and submit a NEW request with the current revision only if the item still applies.",
            "currentRevision": revision
        }}));
    }
    let minted = random_id("op_", 16).map_err(map_error)?;
    let now = crate::datadir::now_epoch();
    let (operation_id, _) = crate::inbound_agent::stage(
        crate::inbound_agent::Submission {
            client: client_id.into(),
            request_id: args.request_id.clone(),
            request_hash: hash.clone(),
            operation_id: minted,
            rule: args.automation_id.clone(),
            revision: revision.clone(),
            text: args.text.clone(),
            from: args.from.clone().unwrap_or_default(),
            where_: args.where_.clone().unwrap_or_default(),
            link: args.link.clone().unwrap_or_default(),
        },
        now,
    )
    .map_err(map_error)?;
    let operation = runtime
        .write(|doc| {
            delegated(doc, client_id)?;
            if let Some(existing) = existing_operation(doc, client_id, &args.request_id, &hash)? {
                return Ok(existing.clone());
            }
            if args.trigger_sequence != delegated(doc, client_id)?.trigger_sequence {
                return Err(DeckError::new(ErrorKind::ContextChanged, STALE_REQUEST));
            }
            reserve_operation(runtime, doc, client_id, Slot::Ordinary)?;
            let operation = Operation {
                operation_id: operation_id.clone(),
                client_id: client_id.into(),
                request_id: args.request_id.clone(),
                request_hash: hash.clone(),
                kind: TRIGGER_KIND.into(),
                state: "accepted".into(),
                code: None,
                result: Some(json!({
                    "automationId": args.automation_id,
                    "revision": revision,
                })),
                accepted_at: now_ms(),
                updated_at: now_ms(),
                admission_hash: None,
                session_id: None,
                control_epoch: None,
                control_sequence: None,
            };
            if let Some(client) = doc
                .config
                .clients
                .iter_mut()
                .find(|client| client.id == client_id)
            {
                client.trigger_sequence = client.trigger_sequence.saturating_add(1);
            }
            doc.operations.push(operation.clone());
            Ok(operation)
        })
        .map_err(map_error)?;
    crate::inbound_agent::commit(client_id, &args.request_id, &operation.operation_id)
        .map_err(map_error)?;
    crate::inbound::inbound_check_now();
    Ok(operation_view(&operation))
}

/// Best effort after a replay: the entry of a request whose operation is on
/// disk is committed (a crash between the two writes is healed here).
fn settle_committed(client_id: &str, request_id: &str, operation_id: &str) {
    if let Err(error) = crate::inbound_agent::commit(client_id, request_id, operation_id) {
        if error.kind() != ErrorKind::Missing {
            crate::applog::applog(&format!(
                "[mcp] automation inbox not settled ({})",
                error.code()
            ));
        }
    }
}

/// The dispatcher's acknowledgment for a trigger operation: `committed`
/// with the card it created, or `rejected` with a closed code. Idempotent;
/// a terminal record is left as it is.
pub(crate) fn automation_outcome(
    operation_id: &str,
    state: &str,
    code: Option<&str>,
    card: Option<&str>,
) -> Result<(), DeckError> {
    let Some(runtime) = RUNTIME.get() else {
        return Ok(());
    };
    outcome_in(runtime, operation_id, state, code, card)
}

pub(super) fn outcome_in(
    runtime: &Runtime,
    operation_id: &str,
    state: &str,
    code: Option<&str>,
    card: Option<&str>,
) -> Result<(), DeckError> {
    runtime.write(|doc| {
        let Some(operation) = doc.operations.iter_mut().find(|operation| {
            operation.operation_id == operation_id && operation.kind == TRIGGER_KIND
        }) else {
            return Ok(());
        };
        if !matches!(operation.state.as_str(), "accepted" | "ambiguous") {
            return Ok(());
        }
        operation.state = state.into();
        operation.code = code.map(str::to_owned);
        if let Some(card) = card {
            let mut result = operation.result.take().unwrap_or_else(|| json!({}));
            result["cardId"] = json!(card);
            operation.result = Some(result);
        }
        operation.updated_at = now_ms();
        Ok(())
    })
}

/// The clients whose delegation stands now, for the scheduler's authority
/// (`scheduler/first_send.rs` `delegations`): `Some(empty)` when MCP never
/// started, is disabled or has no delegated client, `None` only when the
/// ledger cannot be read (nothing is known, so nothing is granted and
/// nothing is withdrawn).
pub(crate) fn automation_delegations() -> Option<HashSet<String>> {
    let Some(runtime) = RUNTIME.get() else {
        return Some(HashSet::new());
    };
    let emergency = runtime.emergency.lock_or_recover();
    if emergency.disabled {
        return Some(HashSet::new());
    }
    let fenced = emergency.clients.clone();
    drop(emergency);
    runtime
        .read(|doc| {
            if !doc.config.enabled {
                return HashSet::new();
            }
            doc.config
                .clients
                .iter()
                .filter(|client| {
                    client.revoked_at.is_none()
                        && client.allow_automations
                        && !fenced.contains(&client.id)
                })
                .map(|client| client.id.clone())
                .collect()
        })
        .ok()
}

/// After the ledger loaded: every accepted trigger operation's inbox entry
/// is committed (a crash between the two acceptance writes), and one whose
/// entry is already handled takes the entry's outcome. Nothing is created
/// and nothing is replayed.
pub(super) fn reconcile_inbox(runtime: &Runtime) {
    let accepted = runtime
        .read(|doc| {
            doc.operations
                .iter()
                .filter(|operation| operation.kind == TRIGGER_KIND && operation.state == "accepted")
                .map(|operation| {
                    (
                        operation.client_id.clone(),
                        operation.request_id.clone(),
                        operation.operation_id.clone(),
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut committed = 0usize;
    for (client, request, operation) in &accepted {
        match crate::inbound_agent::commit(client, request, operation) {
            Ok(()) => committed += 1,
            Err(error) if error.kind() == ErrorKind::Missing => {
                // the inbox lost the item: the operation cannot be judged
                let _ = runtime.write(|doc| {
                    if let Some(op) = doc
                        .operations
                        .iter_mut()
                        .find(|op| &op.operation_id == operation && op.state == "accepted")
                    {
                        op.state = "ambiguous".into();
                        op.code = Some("inbox-lost".into());
                        op.updated_at = now_ms();
                    }
                    Ok(())
                });
            }
            Err(_) => {}
        }
    }
    if committed > 0 {
        crate::applog::applog(&format!(
            "[mcp] {committed} automation trigger(s) reconciled"
        ));
    }
}

/// Withdraw or restore one client's automation delegation. Withdrawing is
/// ordered against the scheduler's firing-intent transaction by the
/// settings fence: a row that relies on this client's delegation re-reads
/// it inside that transaction, so once this returns no automatic send can
/// still begin on the old delegation.
pub(super) fn client_automations(
    runtime: &Runtime,
    client_id: &str,
    allowed: bool,
) -> Result<(), DeckError> {
    let _fence = crate::storage::settings_fence();
    let _delivery = runtime.delivery.lock_or_recover();
    runtime.write(|doc| {
        let client = doc
            .config
            .clients
            .iter_mut()
            .find(|client| client.id == client_id && client.revoked_at.is_none())
            .ok_or_else(|| DeckError::new(ErrorKind::Missing, "MCP client not found"))?;
        client.allow_automations = allowed;
        if !allowed {
            doc.reject_pending(
                |operation| operation.client_id == client_id && operation.kind == TRIGGER_KIND,
                "client-revoked",
            );
        }
        audit(
            doc,
            if allowed {
                "automations-allowed"
            } else {
                "automations-withdrawn"
            },
            AuditLink {
                principal_id: Some(client_id),
                ..Default::default()
            },
        )
    })?;
    if !allowed {
        retire_inbox_for(client_id);
    }
    Ok(())
}

/// The inbox side of a withdrawal: the client's unhandled items will not be
/// offered. Also run by `client_revoke`.
pub(super) fn retire_inbox_for(client_id: &str) {
    match crate::inbound_agent::reject_client(client_id, crate::datadir::now_epoch()) {
        Ok(ids) if !ids.is_empty() => {
            crate::applog::applog(&format!("[mcp] {} automation item(s) retired", ids.len()));
        }
        Ok(_) => {}
        Err(error) => crate::applog::applog(&format!(
            "[mcp] automation inbox not retired ({})",
            error.code()
        )),
    }
}

/// The inbox side of disabling the feature.
pub(super) fn retire_inbox() {
    match crate::inbound_agent::reject_all(crate::datadir::now_epoch()) {
        Ok(ids) if !ids.is_empty() => {
            crate::applog::applog(&format!("[mcp] {} automation item(s) retired", ids.len()));
        }
        Ok(_) => {}
        Err(error) => crate::applog::applog(&format!(
            "[mcp] automation inbox not retired ({})",
            error.code()
        )),
    }
}

/// Whether the ledger holds anything a v6 reader would misread: a delegated
/// client, a trigger sequence, or a trigger record. Such a ledger is written
/// at `STATE_VERSION_AUTOMATIONS` (sticky), which older builds refuse.
pub(super) fn uses_automations(doc: &DiskDoc) -> bool {
    doc.config
        .clients
        .iter()
        .any(|client| client.allow_automations || client.trigger_sequence > 0)
        || doc
            .operations
            .iter()
            .any(|operation| operation.kind == TRIGGER_KIND)
}
