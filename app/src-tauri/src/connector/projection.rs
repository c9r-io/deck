//! Board projection for the phone: consented Board metadata, saved agent
//! card capabilities, explicit Board access, buffer and bounded output.
//!
//! Split out of the one-file `connector/mod.rs` on 2026-09-23; the contract
//! stays in `connector/mod.rs`. `snapshot` and `buffer` read the committed
//! board and the managed queues, then delegate to the pure `snapshot_in`
//! (with the pane probe injected) and `buffer_in_scope`, which tests feed directly.
//! Output maps failures only within the read operation: a missing pane before
//! capture is unavailable, while loss during capture is a changed target.

use super::*;

#[derive(Clone)]
pub(super) struct InternalCard {
    pub(super) id: String,
    pub(super) session: String,
    /// Saved command admission for queues and legacy scopes. BoardAccess
    /// separately permits direct access without rewriting this configuration.
    pub(super) agent_target: bool,
}
pub(super) fn board_value() -> Result<(String, Value), DeckError> {
    let raw = crate::documents::connector_board_payload()?;
    let rev = sha(raw.as_bytes());
    let value = serde_json::from_str(&raw)
        .map_err(|_| DeckError::new(ErrorKind::Recovery, "board projection failed"))?;
    Ok((rev, value))
}
pub(super) fn committed_card(id: &str) -> Result<InternalCard, DeckError> {
    let (_, v) = board_value()?;
    card_in(&v, id)
}
pub(super) fn card_in(v: &Value, id: &str) -> Result<InternalCard, DeckError> {
    let c = v
        .get("cards")
        .and_then(Value::as_array)
        .and_then(|a| {
            a.iter()
                .find(|c| c.get("id").and_then(Value::as_str) == Some(id))
        })
        .ok_or_else(|| DeckError::new(ErrorKind::Missing, "card not found"))?;
    Ok(InternalCard {
        id: id.into(),
        session: c
            .get("session")
            .and_then(Value::as_str)
            .ok_or_else(|| DeckError::new(ErrorKind::InvalidDoc, "card session missing"))?
            .into(),
        agent_target: queue_target_supported(c),
    })
}

pub(super) fn queue_target_supported(card: &Value) -> bool {
    card.get("cmd")
        .and_then(Value::as_str)
        .and_then(crate::admission::channel_agent_command)
        .is_some()
}

pub(super) fn require_agent_card(card: &InternalCard) -> Result<(), DeckError> {
    if card.agent_target {
        Ok(())
    } else {
        Err(DeckError::new(ErrorKind::Invalid, "unsupported-target"))
    }
}

/// Launch configuration remains necessary for queued delivery. Direct access
/// may instead be explicitly authorized for the whole Board on this Mac.
pub(super) fn require_card_access(
    card: &InternalCard,
    scope: SnapshotScope,
) -> Result<(), DeckError> {
    if scope == SnapshotScope::BoardAccess {
        Ok(())
    } else {
        require_agent_card(card)
    }
}

/// The scope governing `device_id` (trusted devices get BoardAccess).
pub(super) fn scope_for(device_id: &str) -> Result<SnapshotScope, DeckError> {
    rt()?.read(|d| effective_scope(d, device_id))
}

pub(super) fn require_queue_target(card: &Value) -> Result<(), DeckError> {
    if queue_target_supported(card) {
        Ok(())
    } else {
        Err(DeckError::new(ErrorKind::Invalid, "unsupported-target"))
    }
}

pub(super) fn validate_buffer_target(
    request: &CommandRequest,
    card: &Value,
    scope: SnapshotScope,
) -> Result<(), DeckError> {
    if request.kind == "buffer-queue" || scope != SnapshotScope::BoardAccess {
        require_queue_target(card)?;
    }
    let current = card
        .get("buffer")
        .and_then(|buffer| buffer.get("revision"))
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .to_string();
    if request.expected_revision.as_deref() != Some(&current) {
        return Err(DeckError::new(
            ErrorKind::ContextChanged,
            "revision-changed",
        ));
    }
    Ok(())
}

pub(super) fn snapshot(app: &AppHandle, device_id: &str) -> Result<Value, DeckError> {
    let (revision, b) = board_value()?;
    let (host_id, scope) = rt()?.read(|d| (d.host_id.clone(), effective_scope(d, device_id)))?;
    let queue = app.state::<Queues>();
    Ok(snapshot_in(
        (&host_id, &host_name()),
        &revision,
        &b,
        &queue,
        scope,
        precise_connector_probe,
        crate::mcp::guard_terminal_input,
    ))
}

/// The phone snapshot of one committed board (`revision` is its hash) and
/// the live queues. `probe` and `mcp_fence` are the same native checks used
/// immediately before output reads; the capability result is explanatory and
/// never substitutes for route admission.
pub(super) fn snapshot_in(
    host: (&str, &str),
    revision: &str,
    b: &Value,
    queue: &Queues,
    scope: SnapshotScope,
    probe: impl Fn(&str) -> Result<crate::context::ConnectorProbe, DeckError>,
    mcp_fence: impl Fn(&str) -> Result<(), DeckError>,
) -> Value {
    let (host_id, host_name) = host;
    let eligible_card_ids = b
        .get("cards")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|card| queue_target_supported(card))
        .filter_map(|card| card.get("id").and_then(Value::as_str))
        .collect::<HashSet<_>>();
    let (_, items, _) =
        crate::scheduler::connector::snapshot(queue, |card_id| eligible_card_ids.contains(card_id));
    let projects = b.get("projects").and_then(Value::as_array).into_iter().flatten().filter_map(|p| {
        let columns = p.get("columns").and_then(Value::as_array).into_iter().flatten().filter_map(|c| Some(json!({"id":c.get("id")?.as_str()?,"name":c.get("name")?.as_str()?}))).collect::<Vec<_>>();
        let presets = p.get("presets").and_then(Value::as_array).into_iter().flatten().filter_map(|x| Some(json!({"id":x.get("id")?.as_str()?,"name":x.get("name")?.as_str()?}))).collect::<Vec<_>>();
        Some(json!({"id":p.get("id")?.as_str()?,"name":p.get("name")?.as_str()?,"columns":columns,"presets":presets}))
    }).collect::<Vec<_>>();
    let cards = b
        .get("cards")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter(|(_, card)| {
            scope != SnapshotScope::AgentCards || queue_target_supported(card)
        })
        .filter_map(|(order, c)| {
            let id = c.get("id")?.as_str()?;
            let session = c.get("session")?.as_str()?;
            let agent_target = queue_target_supported(c);
            let access = agent_target || scope == SnapshotScope::BoardAccess;
            let (status, probe_result, terminal) = if access {
                let (status, probe_result) = probe_status(probe(session));
                let terminal = terminal_capability(
                    true,
                    probe_result.as_ref(),
                    status,
                    scope != SnapshotScope::BoardAccess,
                    || mcp_fence(session),
                );
                (status, probe_result, terminal)
            } else {
                (
                    "unknown",
                    None,
                    capability(false, Some("unsupported-target")),
                )
            };
            let metadata_capability = if access {
                capability(true, None)
            } else {
                capability(false, Some("unsupported-target"))
            };
            // Metadata-only cards do not expose even note counts or collection
            // state. Their native buffer routes remain unsupported as well.
            let buffer = access.then(|| c.get("buffer")).flatten();
            Some(json!({
                "id":id,
                "projectId":c.get("projectId")?.as_str()?,
                "columnId":c.get("columnId")?.as_str()?,
                "title":c.get("title")?.as_str()?,
                "order":order,
                "status":status,
                "generation":access.then(|| probe_result.as_ref().map(|p|p.generation.clone())).flatten(),
                "canSend":terminal["allowed"] == true && probe_result.as_ref().is_some_and(|p|p.agent.is_some()),
                "canQueue":agent_target,
                "capabilities":{
                    "readOutput":terminal.clone(),
                    "sendMessage": if terminal["allowed"] == true && probe_result.as_ref().is_some_and(|p|p.agent.is_none()) { capability(false, Some("agent-not-in-foreground")) } else { terminal },
                    "editNotes":metadata_capability.clone(),
                    "queue":capability(agent_target, (!agent_target).then_some("unsupported-target"))
                },
                "buffer":{
                    "revision":buffer.and_then(|v|v.get("revision")).and_then(Value::as_u64).unwrap_or(0),
                    "collecting":buffer.and_then(|v|v.get("collecting")).and_then(Value::as_bool).unwrap_or(false),
                    "entryCount":buffer.and_then(|v|v.get("entries")).and_then(Value::as_array).map(|a|a.len()).unwrap_or(0)
                }
            }))
        })
        .collect::<Vec<_>>();
    json!({"version":1,"hostId":host_id,"hostName":host_name,"scope":scope,"revision":revision,"capturedAt":now(),"projects":projects,"cards":cards,"queue":items})
}

/// How many committed Board cards a paired phone cannot read output from
/// only because of the kept scope (cards without a saved Codex/Claude launch
/// command). Settings shows it next to the scope choice; it is a fact about
/// the Board, not a probe of what is running, and grants nothing.
pub(super) fn withheld_cards(b: &Value, scope: SnapshotScope) -> usize {
    if scope == SnapshotScope::BoardAccess {
        return 0;
    }
    b.get("cards")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|card| !queue_target_supported(card))
        .count()
}

fn capability(allowed: bool, reason: Option<&str>) -> Value {
    match reason {
        Some(reason) => json!({"allowed":allowed,"reason":reason}),
        None => json!({"allowed":allowed}),
    }
}

fn terminal_capability(
    agent_target: bool,
    probe: Option<&crate::context::ConnectorProbe>,
    status: &str,
    require_agent: bool,
    mcp_fence: impl FnOnce() -> Result<(), DeckError>,
) -> Value {
    if !agent_target {
        return capability(false, Some("unsupported-target"));
    }
    let Some(probe) = probe else {
        let reason = if status == "stopped" {
            "session-unavailable"
        } else {
            "target-unavailable"
        };
        return capability(false, Some(reason));
    };
    if require_agent && probe.agent.is_none() {
        return capability(false, Some("agent-not-in-foreground"));
    }
    if mcp_fence().is_err() {
        return capability(false, Some("mcp-controlled"));
    }
    capability(true, None)
}

pub(super) fn probe_status<T>(probe: Result<T, DeckError>) -> (&'static str, Option<T>) {
    match probe {
        Ok(value) => ("running", Some(value)),
        Err(error) if error.kind() == ErrorKind::NoSession => ("stopped", None),
        Err(_) => ("unknown", None),
    }
}

pub(super) fn buffer(app: &AppHandle, device_id: &str, card_id: &str) -> Result<Value, DeckError> {
    let (_, b) = board_value()?;
    let queues = app.state::<Queues>();
    let (_, _, ops) = crate::scheduler::connector::snapshot(&queues, |_| true);
    buffer_in_scope(&b, card_id, &ops, scope_for(device_id)?)
}

/// One authorized card's buffer from a committed board, each copy stamped
/// with its queue operation's state (`uncertain` when the queue lost it).
pub(super) fn buffer_in_scope(
    b: &Value,
    card_id: &str,
    ops: &[crate::scheduler::connector::OperationDto],
    scope: SnapshotScope,
) -> Result<Value, DeckError> {
    let c = b
        .get("cards")
        .and_then(Value::as_array)
        .and_then(|a| {
            a.iter()
                .find(|c| c.get("id").and_then(Value::as_str) == Some(card_id))
        })
        .ok_or_else(|| DeckError::new(ErrorKind::Missing, "card not found"))?;
    if scope != SnapshotScope::BoardAccess {
        require_queue_target(c)?;
    }
    let mut out = c
        .get("buffer")
        .cloned()
        .unwrap_or_else(|| json!({"revision":0,"collecting":false,"entries":[]}));
    if let Some(entries) = out.get_mut("entries").and_then(Value::as_array_mut) {
        for e in entries {
            if let Some(copies) = e.get_mut("copies").and_then(Value::as_array_mut) {
                for copy in copies {
                    if let Some(id) = copy.get("operationId").and_then(Value::as_str) {
                        if let Some(op) = ops.iter().find(|o| o.id == id) {
                            copy["state"] = Value::String(op.state.clone());
                        } else {
                            copy["state"] = Value::String("uncertain".into());
                        }
                    }
                }
            }
        }
    }
    Ok(out)
}

pub(super) fn bounded_output(text: String, history_size: usize) -> (String, bool) {
    let bytes_truncated = text.len() > MAX_OUTPUT_BYTES;
    let truncated = history_size > 200 || bytes_truncated;
    if !bytes_truncated {
        return (text, truncated);
    }
    let mut start = text.len() - MAX_OUTPUT_BYTES;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    (text[start..].to_string(), truncated)
}

/// The side effects of an output read, injectable so the target check is
/// provably ahead of every pane access.
pub(super) trait OutputIo {
    fn scope(&self) -> Result<SnapshotScope, DeckError>;
    fn card(&self, id: &str) -> Result<InternalCard, DeckError>;
    fn probe(&self, session: &str) -> Result<crate::context::ConnectorProbe, DeckError>;
    fn tmux(&self, args: &[String]) -> Result<String, DeckError>;
    /// Refuses while MCP owns the session's terminal control.
    fn mcp_fence(&self, session: &str) -> Result<(), DeckError>;
}

/// Live output for one requesting device (its effective scope).
pub(super) struct LiveOutput<'a> {
    pub(super) device_id: &'a str,
}

impl OutputIo for LiveOutput<'_> {
    fn scope(&self) -> Result<SnapshotScope, DeckError> {
        scope_for(self.device_id)
    }
    fn card(&self, id: &str) -> Result<InternalCard, DeckError> {
        committed_card(id)
    }
    fn probe(&self, session: &str) -> Result<crate::context::ConnectorProbe, DeckError> {
        precise_connector_probe(session)
    }
    fn tmux(&self, args: &[String]) -> Result<String, DeckError> {
        crate::tmux::tmux_owned(args)
    }
    fn mcp_fence(&self, session: &str) -> Result<(), DeckError> {
        crate::mcp::guard_terminal_input(session)
    }
}

/// Resolve connector probe failures with the same exact session/pane check
/// for snapshot capabilities and output admission. This keeps a stopped card
/// from appearing as an unknown send target solely because tmux returned a
/// malformed display row with exit 0.
fn precise_connector_probe(session: &str) -> Result<crate::context::ConnectorProbe, DeckError> {
    match crate::context::connector_probe(session) {
        Ok(probe) => Ok(probe),
        Err(error) if error.kind() == ErrorKind::Tmux => {
            // A missing target can produce a malformed display-message row
            // with exit 0. Recheck existence without parsing tmux stderr.
            let target = crate::tmux::session_target(session);
            match crate::tmux::tmux(&["has-session", "-t", &target]) {
                Err(check) if check.kind() == ErrorKind::NoSession => Err(check),
                Ok(_) => match crate::tmux::list_panes() {
                    Ok(rows)
                        if !rows.iter().any(|row| {
                            row.session_name == session && row.window_active && row.pane_active
                        }) =>
                    {
                        Err(DeckError::new(ErrorKind::NoSession, "pane unavailable"))
                    }
                    _ => Err(error),
                },
                Err(_) => Err(error),
            }
        }
        Err(error) => Err(error),
    }
}

pub(super) fn output(device_id: &str, card_id: &str) -> Result<Value, DeckError> {
    output_with(&LiveOutput { device_id }, card_id)
}

/// Legacy scopes require a saved agent command plus live agent. BoardAccess
/// permits any live pane's output, including shell history. Every scope keeps
/// the exact pane/generation and current card/scope checks before and after
/// capture, and MCP ownership always blocks reads. The HTTP layer separately
/// fences device withdrawal and scope ABA before returning captured content.
pub(super) fn output_with(io: &dyn OutputIo, card_id: &str) -> Result<Value, DeckError> {
    let started = std::time::Instant::now();
    let request = OUTPUT_REQUEST.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let card = io.card(card_id)?;
    let scope = io.scope()?;
    require_card_access(&card, scope)?;
    let mcp_controlled = |_| DeckError::new(ErrorKind::Perm, "mcp-controlled");
    io.mcp_fence(&card.session).map_err(mcp_controlled)?;
    let before = io
        .probe(&card.session)
        .map_err(|error| output_failure("probe-before", card_id, request, started, error))?;
    if scope != SnapshotScope::BoardAccess && before.agent.is_none() {
        return Err(DeckError::new(
            ErrorKind::Invalid,
            "agent-not-in-foreground",
        ));
    }
    let history_size = io
        .tmux(&[
            "display-message".into(),
            "-p".into(),
            "-t".into(),
            before.identity.pane_id.clone(),
            "#{history_size}".into(),
        ])
        .map_err(|error| output_failure("history-size", card_id, request, started, error))?
        .trim()
        .parse::<usize>()
        .map_err(|_| {
            output_failure(
                "history-size",
                card_id,
                request,
                started,
                DeckError::new(ErrorKind::Tmux, "output-history-unavailable"),
            )
        })?;
    let text = io
        .tmux(&[
            "capture-pane".into(),
            "-p".into(),
            "-t".into(),
            before.identity.pane_id.clone(),
            "-S".into(),
            "-200".into(),
        ])
        .map_err(|error| output_failure("capture-pane", card_id, request, started, error))?;
    let after = io
        .probe(&card.session)
        .map_err(|error| output_failure("probe-after", card_id, request, started, error))?;
    let still = io.card(card_id)?;
    io.mcp_fence(&card.session).map_err(mcp_controlled)?;
    require_card_access(&still, io.scope()?)?;
    if scope != io.scope()? || (scope != SnapshotScope::BoardAccess && after.agent.is_none()) {
        return Err(DeckError::new(ErrorKind::ContextChanged, "target-changed"));
    }
    if before.generation != after.generation || still.session != card.session {
        return Err(DeckError::new(ErrorKind::ContextChanged, "target-changed"));
    }
    let (text, truncated) = bounded_output(text, history_size);
    let revision = sha(format!("{}\0{text}", before.generation).as_bytes());
    Ok(
        json!({"cardId":card_id,"generation":before.generation,"revision":revision,"capturedAt":now(),"text":text,"truncated":truncated}),
    )
}

static OUTPUT_REQUEST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn output_failure(
    stage: &'static str,
    card_id: &str,
    request: u64,
    started: std::time::Instant,
    error: DeckError,
) -> DeckError {
    let kind = error.kind();
    let mapped = match kind {
        ErrorKind::NoSession if stage == "probe-before" => {
            DeckError::new(ErrorKind::NoSession, "session-unavailable")
        }
        ErrorKind::NoSession | ErrorKind::ContextChanged => {
            DeckError::new(ErrorKind::ContextChanged, "target-changed")
        }
        ErrorKind::Perm | ErrorKind::Missing => error,
        _ => DeckError::new(ErrorKind::Tmux, "output-read-failed"),
    };
    if mapped.kind() != ErrorKind::NoSession {
        crate::applog::applog(&format!(
            "[connector-output] stage={stage} kind={} card={} request={request} duration_ms={}",
            kind.as_str(),
            crate::applog::session_tag(card_id),
            started.elapsed().as_millis()
        ));
    }
    mapped
}
