//! First-send readiness override: a Slack badge or clock rule's explicit, per-rule
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
//! - **run intent** — the saved clock schedule/template or Slack reaction
//!   on that message (the run's head row is
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
//! Scope, closed: verified Slack badge runs use external admission; verified
//! clock runs use owner admission and remain external=false. Only the head
//! of a frozen run may carry this policy, for Claude or literal Codex
//! --no-daemon. This separate FirstSendOrigin never expands TriggerClass or
//! Slack content authority. Channel monitors, Connector, MCP, manual lists
//! and later steps never obtain it.
//!
//! - Admission: CURRENT settings and the backend's pending native event must
//!   match {rule,event}. Clock owner admission additionally checks the saved
//!   card's unique origin, session/target, frozen first text and operation ID.
//!   Missing native proof admits no override; replay is operation-idempotent.
//!   The pending event cannot be manufactured by a claimed source/timestamp.
//!   Clock target policy freezes rule project/directory; disabling, changing
//!   source/command/target or deleting the rule withdraws it. Schedule/template
//!   edits affect future runs, not frozen prompt bytes or this policy.
//! - Selection (`select::hold_reason`): the override lifts ONLY the
//!   first-interaction hold (Claude without an interaction word, Codex
//!   `Unknown`) and only while this tick could read settings. Needs-input,
//!   Codex `Unavailable`, the external and authority holds, pause, review,
//!   group order, time, send gap, target identity, expected process and the
//!   bracketed-paste check all still apply. Starting an absent session is
//!   unchanged (`StartedAwaitingInteraction`: no bytes); the worker then
//!   wakes the scheduler once (`thread::start_wake_due`, at most once per
//!   session per tick), and that ordinary pass sends the row into the
//!   now-existing session through the existing-session probe. When that
//!   automatic send still relies on the override, its own per-session worker
//!   waits a bounded 6 s compatibility grace before the firing intent and
//!   paste. It rechecks current Signal, settings, row and pane generation
//!   after the wait. The grace is never evidence or authority, and an Agent
//!   restart begins a new wait.
//! - Revocation: the sweep (`revoke_stale`) strips the override from every
//!   unsent row whose rule no longer allows it (unticked, deleted, command
//!   changed, trigger changed); the pre-fire `fence` re-reads settings under
//!   `storage::settings_fence` in the transaction that persists the firing
//!   intent, exactly like the authority fence. Unreadable settings: nothing
//!   is stripped, and nothing is sent on the override (the row falls back to
//!   the ordinary first-interaction hold). Settings are read without moving
//!   the file, so a damaged main file is answered from its backup: the
//!   choices of the previous save, as for the approval (`authority.rs`).
//!   Send-now never consults it.
//! - Durable vs transient: the rule flag and the row copy survive restarts;
//!   interaction evidence does not and is never invented. After a Deck
//!   restart a still-permitted pending head row may still be sent on it.
//! - Audit: a delivery that relied on the override records
//!   `readiness_overridden` (closed flag, no text) — "the rule allowed a
//!   first send without readiness", never "the agent was ready".
//! - Compatibility: Slack optional fields remain compatible. Clock-enabled
//!   settings and clock overrides upgrade to sticky schema v4: v3 refuses
//!   them untouched instead of decoding the new closed origin as damage.
//! - Exit: replaced, not removed. "Temporary" above lasts until an agent
//!   exposes an authoritative fact that a fresh interactive session accepts
//!   its first typed prompt. Selection then takes that fact as the reason
//!   the head row may go (`select::hold_reason`, where the override is
//!   honored today), and for that agent the override stops being what lifts
//!   the hold. Three things must hold across that change, each one checkable:
//!   1. A rule that has the option keeps sending its head row with no click
//!      and no new confirmation. The tests that send an overridden head stay
//!      green, or are replaced by tests that reach the same send through the
//!      readiness fact; the user is never asked to accept anything again.
//!   2. The saved flag, the row copy and sticky v4 keep loading as they are.
//!      Where readiness is proven the override is simply not consulted;
//!      nothing is stripped from settings or from rows.
//!   3. An agent without the fact keeps the override exactly as written here.
//!
//!   Deleting the flag, or leaving v4, is a later migration of its own, with
//!   its own proof that old files load and no rule needs re-confirming. Until
//!   a readiness fact exists there is nothing to replace this with, and no
//!   Signal word, quiet time, title or timer is one.

use serde::{Deserialize, Serialize};

use super::*;
use crate::inbound::{Config, Event, Rule};

/// The durable row fact: this row is the head of a run created by an automation
/// rule `rule`, which allowed a first send without readiness when the
/// row was admitted. Ids and a closed word only.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct ReadinessOverride {
    pub(crate) rule: String,
    pub(crate) trigger: FirstSendOrigin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) clock_target: Option<ClockTarget>,
}

/// Readiness origins are independent of Slack content-authority triggers.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum FirstSendOrigin {
    SlackBadge,
    Clock,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct ClockTarget {
    pub(crate) project: String,
    pub(crate) dir: String,
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
    matches!(rule.source.as_str(), "slack" | "clock")
        && (rule.source != "clock" || rule.enabled)
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
    if !matches!(rule.source.as_str(), "slack" | "clock")
        || (rule.source == "clock" && !rule.enabled)
    {
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
    if event.source != rule.source || event.badge != rule.badge || event.key != claim.event {
        return Err("event");
    }
    Ok(ReadinessOverride {
        rule: rule.id.clone(),
        trigger: if rule.source == "clock" {
            FirstSendOrigin::Clock
        } else {
            FirstSendOrigin::SlackBadge
        },
        clock_target: (rule.source == "clock").then(|| ClockTarget {
            project: rule.project_id.clone(),
            dir: rule.dir.clone(),
        }),
    })
}

/// Clock owner admission requires the committed run plan as well as the
/// native pending event. Ordinary owner rows cannot mint a run-head policy.
pub(crate) fn clock_head_matches(
    board: &serde_json::Value,
    args: &super::ops::QueueAddArgs,
    rule: &Rule,
    claim: &FirstSendClaim,
) -> bool {
    let Some(cards) = board.get("cards").and_then(|v| v.as_array()) else {
        return false;
    };
    let matching: Vec<_> = cards
        .iter()
        .filter(|c| {
            c["origin"]["source"] == "clock"
                && c["origin"]["key"] == claim.event
                && c["origin"]["badge"] == rule.id
        })
        .collect();
    let [card] = matching.as_slice() else {
        return false;
    };
    let plan = &card["inboundPlan"];
    let head = &plan["initialSteps"][0];
    let operation = if args.review_each {
        &plan["operationId"]
    } else {
        &head["operationId"]
    };
    let home = dirs::home_dir().map(|p| p.to_string_lossy().into_owned());
    let dir = rule.dir.trim();
    let configured_dir = if dir.is_empty() || dir == "~" {
        home.clone()
    } else if let Some(rest) = dir.strip_prefix("~/") {
        home.map(|h| format!("{}/{rest}", h.trim_end_matches('/')))
    } else {
        Some(dir.to_owned())
    };
    configured_dir.as_deref() == Some(args.dir.as_str())
        && card["id"] == args.card_id
        && card["session"] == args.session
        && card["projectId"] == rule.project_id
        && card["cmd"] == args.cmd
        && card["dir"] == args.dir
        && plan["initialQueued"] == false
        && plan["firstSend"]["rule"] == rule.id
        && plan["reviewEach"] == args.review_each
        && operation.as_str() == args.operation_id.as_deref()
        && args.operation_id.is_some()
        && head["mode"] == "at"
        && args.mode == "at"
        && args.group.is_none()
        && head["tplIdx"] == 1
        && args.tpl_idx == Some(1)
        && head["tpl"].as_str() == args.tpl.as_deref()
        && head["at"].as_u64() == args.at
        && head["text"]
            .as_str()
            .map(super::ops::normalize_prompt)
            .as_deref()
            == Some(super::ops::normalize_prompt(&args.text).as_str())
}

/// Whether the row's override is still backed by its rule.
fn still_allowed(config: &Config, i: &QueueItem) -> bool {
    i.readiness_override.as_ref().is_some_and(|o| {
        config
            .rules
            .iter()
            .find(|r| r.id == o.rule)
            .is_some_and(|rule| {
                allows(rule, &i.cmd)
                    && match o.trigger {
                        FirstSendOrigin::SlackBadge => rule.source == "slack",
                        FirstSendOrigin::Clock => {
                            rule.source == "clock"
                                && o.clock_target.as_ref().is_some_and(|t| {
                                    t.project == rule.project_id && t.dir == rule.dir
                                })
                        }
                    }
            })
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
