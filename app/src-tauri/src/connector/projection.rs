//! Board projection for the phone: saved agent cards, snapshot, buffer and bounded output.
//!
//! Split out of the one-file `connector/mod.rs` on 2026-09-23; the contract
//! stays in `connector/mod.rs`. `snapshot` and `buffer` read the committed
//! board and the managed queues, then delegate to the pure `snapshot_in`
//! (with the pane probe injected) and `buffer_in`, which tests feed directly.

use super::*;

#[derive(Clone)]
pub(super) struct InternalCard {
    pub(super) id: String,
    pub(super) session: String,
    /// The card's SAVED command is Codex or Claude. Phone text and phone
    /// output reads are limited to such cards; a live foreground agent in an
    /// ordinary shell card does not qualify.
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
) -> Result<(), DeckError> {
    require_queue_target(card)?;
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

pub(super) fn snapshot(app: &AppHandle) -> Result<Value, DeckError> {
    let (revision, b) = board_value()?;
    let host_id = rt()?.read(|d| d.host_id.clone())?;
    let queue = app.state::<Queues>();
    Ok(snapshot_in(
        &host_id,
        &revision,
        &b,
        &queue,
        crate::context::connector_probe,
    ))
}

/// The phone snapshot of one committed board (`revision` is its hash) and
/// the live queues; `probe` answers each saved agent card's session.
pub(super) fn snapshot_in(
    host_id: &str,
    revision: &str,
    b: &Value,
    queue: &Queues,
    probe: impl Fn(&str) -> Result<crate::context::ConnectorProbe, DeckError>,
) -> Value {
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
        .filter(|card| queue_target_supported(card))
        .filter_map(|c| {
            let id = c.get("id")?.as_str()?;
            let session = c.get("session")?.as_str()?;
            let (status, probe) = probe_status(probe(session));
            let buffer = c.get("buffer");
            Some(json!({
                "id":id,
                "projectId":c.get("projectId")?.as_str()?,
                "columnId":c.get("columnId")?.as_str()?,
                "title":c.get("title")?.as_str()?,
                "status":status,
                "generation":probe.as_ref().map(|p|p.generation.clone()),
                "canSend":queue_target_supported(c) && probe.as_ref().is_some_and(|p|p.agent.is_some()),
                "canQueue":true,
                "buffer":{
                    "revision":buffer.and_then(|v|v.get("revision")).and_then(Value::as_u64).unwrap_or(0),
                    "collecting":buffer.and_then(|v|v.get("collecting")).and_then(Value::as_bool).unwrap_or(false),
                    "entryCount":buffer.and_then(|v|v.get("entries")).and_then(Value::as_array).map(|a|a.len()).unwrap_or(0)
                }
            }))
        })
        .collect::<Vec<_>>();
    json!({"version":1,"hostId":host_id,"revision":revision,"capturedAt":now(),"projects":projects,"cards":cards,"queue":items})
}

pub(super) fn probe_status<T>(probe: Result<T, DeckError>) -> (&'static str, Option<T>) {
    match probe {
        Ok(value) => ("running", Some(value)),
        Err(error) if error.kind() == ErrorKind::NoSession => ("stopped", None),
        Err(_) => ("unknown", None),
    }
}

pub(super) fn buffer(app: &AppHandle, card_id: &str) -> Result<Value, DeckError> {
    let (_, b) = board_value()?;
    let queues = app.state::<Queues>();
    let (_, _, ops) = crate::scheduler::connector::snapshot(&queues, |_| true);
    buffer_in(&b, card_id, &ops)
}

/// One saved agent card's buffer from a committed board, each copy stamped
/// with its queue operation's state (`uncertain` when the queue lost it).
pub(super) fn buffer_in(
    b: &Value,
    card_id: &str,
    ops: &[crate::scheduler::connector::OperationDto],
) -> Result<Value, DeckError> {
    let c = b
        .get("cards")
        .and_then(Value::as_array)
        .and_then(|a| {
            a.iter()
                .find(|c| c.get("id").and_then(Value::as_str) == Some(card_id))
        })
        .ok_or_else(|| DeckError::new(ErrorKind::Missing, "card not found"))?;
    require_queue_target(c)?;
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
    fn card(&self, id: &str) -> Result<InternalCard, DeckError>;
    fn probe(&self, session: &str) -> Result<crate::context::ConnectorProbe, DeckError>;
    fn tmux(&self, args: &[String]) -> Result<String, DeckError>;
    /// Refuses while MCP owns the session's terminal control.
    fn mcp_fence(&self, session: &str) -> Result<(), DeckError>;
}

pub(super) struct LiveOutput;

impl OutputIo for LiveOutput {
    fn card(&self, id: &str) -> Result<InternalCard, DeckError> {
        committed_card(id)
    }
    fn probe(&self, session: &str) -> Result<crate::context::ConnectorProbe, DeckError> {
        crate::context::connector_probe(session)
    }
    fn tmux(&self, args: &[String]) -> Result<String, DeckError> {
        crate::tmux::tmux_owned(args)
    }
    fn mcp_fence(&self, session: &str) -> Result<(), DeckError> {
        crate::mcp::guard_terminal_input(session)
    }
}

pub(super) fn output(card_id: &str) -> Result<Value, DeckError> {
    output_with(&LiveOutput, card_id)
}

/// Phone output reads are limited to cards with a saved Codex/Claude command
/// and a live Codex/Claude foreground process. Both are rechecked after the
/// capture, so a pane that has fallen back to a shell is never read. The
/// capture is the pane's last 200 lines while the agent runs; those can still
/// hold shell output from before the agent started (documented limit,
/// `docs/connector.md`). A session under MCP control is never read: MCP
/// output sharing is consent for the MCP client, not for a phone. The fence
/// is checked before any pane access and again after the capture.
pub(super) fn output_with(io: &dyn OutputIo, card_id: &str) -> Result<Value, DeckError> {
    let card = io.card(card_id)?;
    require_agent_card(&card)?;
    let unsupported = |_| DeckError::new(ErrorKind::Invalid, "unsupported-target");
    io.mcp_fence(&card.session).map_err(unsupported)?;
    let before = io.probe(&card.session)?;
    if before.agent.is_none() {
        return Err(DeckError::new(ErrorKind::Invalid, "unsupported-target"));
    }
    let history_size = io
        .tmux(&[
            "display-message".into(),
            "-p".into(),
            "-t".into(),
            before.identity.pane_id.clone(),
            "#{history_size}".into(),
        ])?
        .trim()
        .parse::<usize>()
        .map_err(|_| DeckError::new(ErrorKind::Tmux, "output-history-unavailable"))?;
    let text = io.tmux(&[
        "capture-pane".into(),
        "-p".into(),
        "-t".into(),
        before.identity.pane_id.clone(),
        "-S".into(),
        "-200".into(),
    ])?;
    let after = io.probe(&card.session)?;
    let still = io.card(card_id)?;
    io.mcp_fence(&card.session).map_err(unsupported)?;
    if after.agent.is_none() || !still.agent_target {
        return Err(DeckError::new(ErrorKind::Invalid, "unsupported-target"));
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
