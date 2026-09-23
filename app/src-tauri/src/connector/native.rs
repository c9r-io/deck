//! Native execution of a claimed command and the `Transport` impl behind it.
//!
//! Split out of the one-file `connector/mod.rs` on 2026-09-23; the contract
//! stays in `connector/mod.rs`.

use super::*;

pub(super) fn execute_native(
    req: &CommandRequest,
    handle: &str,
    device_id: &str,
    queues: &Queues,
) -> Result<&'static str, (&'static str, &'static str)> {
    match req.kind.as_str() {
        "send-message" => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct P {
                text: String,
            }
            let p: P = serde_json::from_value(req.payload.clone())
                .map_err(|_| ("rejected", "invalid-payload"))?;
            if p.text.is_empty()
                || p.text.len() > MAX_TEXT
                || p.text
                    .chars()
                    .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
            {
                return Err(("rejected", "invalid-text"));
            }
            let card = committed_card(req.card_id.as_deref().ok_or(("rejected", "missing-card"))?)
                .map_err(|_| ("rejected", "missing-card"))?;
            require_agent_card(&card).map_err(|_| ("rejected", "unsupported-target"))?;
            let probe = crate::context::connector_probe(&card.session)
                .map_err(|_| ("rejected", "target-changed"))?;
            if !matches!(
                &req.expected_generation,
                ExpectedGeneration::Live(expected) if expected == &probe.generation
            ) {
                return Err(("rejected", "target-changed"));
            }
            let agent = probe
                .agent
                .as_deref()
                .ok_or(("rejected", "agent-not-ready"))?;
            crate::mcp::guard_terminal_input(&card.session)
                .map_err(|_| ("rejected", "unsupported-target"))?;
            if !crate::scheduler::claim_session(&queues.busy, &card.session) {
                return Err(("rejected", "session-busy"));
            }
            let _busy = BusyClaim {
                busy: &queues.busy,
                session: &card.session,
            };
            let transport = ConnectorTransport {
                card_id: &card.id,
                session: &card.session,
                expected_generation: &probe.generation,
                device_id,
            };
            let outcome = crate::prompt_delivery::deliver_with(
                crate::prompt_delivery::LiteralRequest {
                    session: &card.session,
                    pane: &probe.identity,
                    expected_process: Some(agent),
                    delivery: &handle[..handle.len().min(48)],
                    text: &p.text,
                    submit: true,
                    require_bracketed: true,
                    require_paste_mode: false,
                },
                &transport,
            );
            match outcome {
                Ok(crate::prompt_delivery::LiteralOutcome::Submitted) => Ok("delivered"),
                Ok(_) => Err(("ambiguous", "enter-refused")),
                Err(e) if e.kind() == ErrorKind::ContextChanged => {
                    Err(("rejected", "target-changed"))
                }
                Err(_) => Err(("ambiguous", "delivery-unknown")),
            }
        }
        "queue-pause" | "queue-cancel" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct P {
                item_id: String,
                #[serde(default)]
                paused: Option<bool>,
                revision: String,
            }
            let p: P = serde_json::from_value(req.payload.clone())
                .map_err(|_| ("rejected", "invalid-payload"))?;
            let card = committed_card(req.card_id.as_deref().ok_or(("rejected", "missing-card"))?)
                .map_err(|_| ("rejected", "missing-card"))?;
            require_agent_card(&card).map_err(|_| ("rejected", "unsupported-target"))?;
            let rev = p
                .revision
                .parse()
                .map_err(|_| ("rejected", "revision-changed"))?;
            crate::scheduler::connector::mutate(
                queues,
                &card.id,
                &card.session,
                &p.item_id,
                rev,
                if req.kind == "queue-pause" {
                    Some(p.paused.ok_or(("rejected", "invalid-payload"))?)
                } else {
                    None
                },
                || {
                    let runtime = rt()?;
                    if !runtime.feature_active() {
                        return Err(DeckError::new(ErrorKind::Perm, "connector-disabled"));
                    }
                    let active = runtime.read(|d| {
                        d.devices
                            .iter()
                            .any(|x| x.id == device_id && x.revoked_at.is_none())
                    })?;
                    if !active {
                        return Err(DeckError::new(ErrorKind::Perm, "device-revoked"));
                    }
                    match crate::context::connector_probe(&card.session) {
                        Ok(probe)
                            if matches!(
                                &req.expected_generation,
                                ExpectedGeneration::Live(expected)
                                    if expected == &probe.generation
                            ) =>
                        {
                            Ok(())
                        }
                        Ok(_) => Err(DeckError::new(ErrorKind::ContextChanged, "target-changed")),
                        Err(e)
                            if matches!(&req.expected_generation, ExpectedGeneration::Stopped)
                                && e.kind() == ErrorKind::NoSession =>
                        {
                            Ok(())
                        }
                        Err(_) => Err(DeckError::new(ErrorKind::Other, "target-unknown")),
                    }
                },
            )
            .map_err(|e| {
                (
                    "rejected",
                    match e.message() {
                        "target-changed" => "target-changed",
                        "target-unknown" => "target-unknown",
                        "device-revoked" => "device-revoked",
                        "revision-changed" => "revision-changed",
                        _ => "queue-conflict",
                    },
                )
            })?;
            Ok("applied")
        }
        _ => Err(("rejected", "frontend-required")),
    }
}

pub(super) struct BusyClaim<'a> {
    pub(super) busy: &'a Mutex<HashSet<String>>,
    pub(super) session: &'a str,
}

impl Drop for BusyClaim<'_> {
    fn drop(&mut self) {
        crate::scheduler::release_session(self.busy, self.session);
    }
}

pub(super) struct ConnectorTransport<'a> {
    pub(super) card_id: &'a str,
    pub(super) session: &'a str,
    pub(super) expected_generation: &'a str,
    pub(super) device_id: &'a str,
}

impl ConnectorTransport<'_> {
    pub(super) fn guard(&self) -> Result<(), DeckError> {
        let runtime = rt()?;
        if !runtime.feature_active() {
            return Err(DeckError::new(ErrorKind::Perm, "connector unavailable"));
        }
        if committed_card(self.card_id)?.session != self.session {
            return Err(DeckError::new(ErrorKind::ContextChanged, "target-changed"));
        }
        // The same MCP control fence as every other terminal-input path
        // (`prompt_delivery::deliver`), re-run before each tmux write.
        crate::mcp::guard_terminal_input(self.session)?;
        let device_active = runtime.read(|d| {
            d.devices
                .iter()
                .any(|x| x.id == self.device_id && x.revoked_at.is_none())
        })?;
        if !device_active {
            return Err(DeckError::new(ErrorKind::Perm, "device revoked"));
        }
        let generation = crate::context::connector_probe(self.session)?.generation;
        if generation != self.expected_generation {
            return Err(DeckError::new(ErrorKind::ContextChanged, "target-changed"));
        }
        Ok(())
    }
}

impl crate::prompt_delivery::Transport for ConnectorTransport<'_> {
    fn probe(&self, session: &str) -> Result<crate::context::RawProbe, DeckError> {
        crate::prompt_delivery::Transport::probe(&crate::prompt_delivery::TmuxTransport, session)
    }

    fn run(&self, args: &[String]) -> Result<String, DeckError> {
        // Cleanup must remain possible after a target or authorization change.
        if args.first().map(String::as_str) != Some("delete-buffer") {
            self.guard()?;
        }
        crate::prompt_delivery::Transport::run(&crate::prompt_delivery::TmuxTransport, args)
    }

    fn run_with_stdin(&self, args: &[String], input: &[u8]) -> Result<String, DeckError> {
        self.guard()?;
        crate::prompt_delivery::Transport::run_with_stdin(
            &crate::prompt_delivery::TmuxTransport,
            args,
            input,
        )
    }

    fn pause(&self, duration: std::time::Duration) {
        crate::prompt_delivery::Transport::pause(&crate::prompt_delivery::TmuxTransport, duration)
    }
}
