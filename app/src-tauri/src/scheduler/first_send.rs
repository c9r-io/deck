//! First-send readiness override: a Slack badge rule's explicit, per-rule
//! acceptance that the FIRST step of its runs may be sent to a freshly
//! started agent without first-interaction evidence.
//!
//! # Contract
//! A temporary compatibility escape hatch for a missing upstream primitive:
//! neither Claude Code nor Codex exposes a fact that proves a fresh
//! interactive TUI is ready for its first typed prompt (a startup dialog —
//! trust, update, onboarding, a hooks or MCP review — may own Enter). So by
//! default the first-interaction gate (`select.rs`) holds that first prompt
//! until the agent proves an interaction, and the user's Send now is the
//! readiness confirmation. The user's Slack reaction already approved the
//! run; the remaining click is not an approval. This override lets the user
//! remove it for ONE rule, knowingly. Four facts stay separate:
//!
//! - **run approval** — the reaction on that message (the run's head row is
//!   never held as external, `select.rs`);
//! - **content authority** — `authority.rs` (`StepAuthority`), untouched
//!   here: this module never makes unauthorized text sendable;
//! - **readiness evidence** — `agent_status::Evidence`, never written or
//!   faked here: the generation stays "no interaction" after the send, and
//!   no Signal word, quiet time, title or timer creates the override;
//! - **the override** — `Rule.first_send_without_readiness` (settings) and
//!   its row copy `QueueItem.readiness_override`: readiness is UNKNOWN and
//!   the user accepted the startup-dialog risk for this rule's first send.
//!
//! Scope, closed: only a Slack badge rule (`TriggerClass::SlackBadge`), only
//! the head row (`at`) of a run that rule created, only on the external
//! admission path, only a supported agent command — Claude, or Codex with a
//! literal `--no-daemon` (the shared app-server daemon, Codex's default,
//! gives Deck no attributable Signal for that process at all; the override
//! does not reach it). Channel monitors (a passive event, no per-message
//! human action), Connector rows, clock and manual lists, MCP and every
//! later step never carry it: `verify` is the one writer and refuses them.
//!
//! - Admission (`verify`): the webview's claim `{rule, event}` for text 0
//!   of a badge run is checked against the CURRENT settings and the
//!   backend's own copy of the inbound event (`inbound::pending_event`: a
//!   Slack event of that rule, same badge). A refused claim admits the row
//!   without the override — the ordinary gate holds it. A replay after a
//!   restart before the event is announced again admits it without.
//! - Selection (`select::hold_reason`): the override lifts ONLY the
//!   first-interaction hold (Claude without an interaction word, Codex
//!   `Unknown`) and only while this tick could read settings. Needs-input,
//!   Codex `Unavailable`, the external and authority holds, pause, review,
//!   group order, time, send gap, target identity, expected process and the
//!   bracketed-paste check all still apply. Starting an absent session is
//!   unchanged (`StartedAwaitingInteraction`: no bytes); the worker then
//!   wakes the scheduler once (`thread::start_wake_due`, at most once per
//!   session per tick), and that ordinary pass sends the row into the
//!   now-existing session through the existing-session probe — no settle
//!   delay and no readiness claim.
//! - Revocation: the sweep (`revoke_stale`) strips the override from every
//!   unsent row whose rule no longer allows it (unticked, deleted, command
//!   changed, trigger changed); the pre-fire `fence` re-reads settings under
//!   `storage::settings_fence` in the transaction that persists the firing
//!   intent, exactly like the authority fence. Unreadable settings: nothing
//!   is stripped, and nothing is sent on the override (the row falls back to
//!   the ordinary first-interaction hold). Send-now never consults it.
//! - Durable vs transient: the rule flag and the row copy survive restarts;
//!   interaction evidence does not and is never invented. After a Deck
//!   restart a still-permitted pending head row may still be sent on it.
//! - Audit: a delivery that relied on the override records
//!   `readiness_overridden` (closed flag, no text) — "the rule allowed a
//!   first send without readiness", never "the agent was ready".
//! - Compatibility: every field can only withdraw the override on an older
//!   reader (it ignores the rule flag and the row field and holds the row).

use serde::{Deserialize, Serialize};

use super::*;
use crate::inbound::{Config, Event, Rule};

/// The durable row fact: this row is the head of a run created by Slack
/// badge rule `rule`, which allowed a first send without readiness when the
/// row was admitted. Ids and a closed word only.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct ReadinessOverride {
    pub(crate) rule: String,
    pub(crate) trigger: TriggerClass,
}

/// The frozen plan's statement that this call's first text is the head row
/// of the badge run made from inbound event `event` by rule `rule`. A
/// request only: `verify` decides.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FirstSendClaim {
    pub(crate) rule: String,
    pub(crate) event: String,
}

/// Whether the override can reach this agent command: Claude, or Codex
/// forced out of the shared daemon by a literal `--no-daemon` argument.
pub(crate) fn supported_command(cmd: &str) -> bool {
    match crate::admission::channel_agent_command(cmd) {
        Some("claude") => true,
        Some("codex") => cmd.split(' ').any(|word| word == "--no-daemon"),
        _ => false,
    }
}

/// Whether `rule` currently allows the override for a row launched with
/// `cmd`.
fn allows(rule: &Rule, cmd: &str) -> bool {
    rule.source == "slack"
        && rule.first_send_without_readiness
        && rule.cmd == cmd
        && supported_command(cmd)
}

/// Check a head row's claim (module header). `mode` is the row's mode,
/// `event` the backend's own copy of the claimed inbound event.
pub(crate) fn verify(
    config: Option<&Config>,
    claim: &FirstSendClaim,
    cmd: &str,
    mode: &str,
    external_text: bool,
    event: Option<&Event>,
) -> Result<ReadinessOverride, Refusal> {
    if external_text {
        return Err("verbatim");
    }
    if mode != "at" {
        return Err("step");
    }
    let config = config.ok_or("settings-unreadable")?;
    let rule = config
        .rules
        .iter()
        .find(|r| r.id == claim.rule)
        .ok_or("no-rule")?;
    if rule.source != "slack" {
        return Err("trigger");
    }
    if !rule.first_send_without_readiness {
        return Err("off");
    }
    if rule.cmd != cmd {
        return Err("command");
    }
    if !supported_command(cmd) {
        return Err("agent");
    }
    let event = event.ok_or("no-event")?;
    if event.source != "slack" || event.badge != rule.badge || event.key != claim.event {
        return Err("event");
    }
    Ok(ReadinessOverride {
        rule: rule.id.clone(),
        trigger: TriggerClass::SlackBadge,
    })
}

/// Whether the row's override is still backed by its rule.
fn still_allowed(config: &Config, i: &QueueItem) -> bool {
    i.readiness_override.as_ref().is_some_and(|o| {
        config
            .rules
            .iter()
            .find(|r| r.id == o.rule)
            .is_some_and(|rule| allows(rule, &i.cmd))
    })
}

/// The override check immediately before the irreversible boundary, for an
/// automatic send that relies on it (`select::relies_on_readiness_override`).
pub(crate) fn fence(i: &QueueItem, config: Option<&Config>) -> Fence {
    match config {
        None => Fence::Unverified,
        Some(config) if still_allowed(config, i) => Fence::Clear,
        Some(_) => Fence::Revoked,
    }
}

fn revocable(i: &QueueItem) -> bool {
    i.readiness_override.is_some() && matches!(i.state, ItemState::Pending | ItemState::Failed)
}

/// Whether any unsent row carries the override (the tick reads settings
/// only then, or for `authority::any_authority`).
pub(crate) fn any_override(q: &QueueState) -> bool {
    q.items.iter().any(revocable)
}

/// Strip the override from every unsent row whose rule no longer allows it;
/// the number of rows that lost it.
pub(crate) fn revoke_stale(q: &mut QueueState, config: &Config) -> usize {
    let mut n = 0;
    for item in q.items.iter_mut().filter(|i| revocable(i)) {
        if !still_allowed(config, item) {
            item.readiness_override = None;
            item.revision = item.revision.wrapping_add(1);
            n += 1;
        }
    }
    n
}
