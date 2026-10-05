//! First-send readiness override: the explicit acceptance, per Slack badge
//! rule, clock rule or phone task preset, that the FIRST step of its runs
//! may be sent to a freshly started agent without first-interaction
//! evidence.
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
//! - **the override** — `Rule.first_send_without_readiness` (settings) or a
//!   task preset's `firstSend` (the Board), and its row copy
//!   `QueueItem.readiness_override`: readiness is UNKNOWN and the user
//!   accepted the startup-dialog risk for this rule's or preset's first send.
//!
//! Scope, closed: verified Slack badge runs use external admission; verified
//! clock runs use owner admission and remain external=false; a verified
//! phone task (Connector `task-create`) uses external admission and stays
//! external. Only the head of a frozen run may carry this policy, for Claude
//! or literal Codex --no-daemon. This separate FirstSendOrigin never expands
//! TriggerClass or content authority. Channel monitors, every other
//! Connector command, MCP, manual lists and later steps never obtain it.
//!
//! Two sources, never mixed: a Slack or clock override is backed by
//! settings.json, a phone task override by the Board (`Backing`). Each row
//! is admitted, swept and fenced against its own source alone, and each
//! source being unreadable holds only the rows it backs.
//!
//! - Admission: CURRENT settings and the backend's pending native event must
//!   match {rule,event}. Clock owner admission additionally checks the saved
//!   card's unique origin, session/target, frozen first text and operation ID.
//!   Missing native proof admits no override; replay is operation-idempotent.
//!   The pending event cannot be manufactured by a claimed source/timestamp.
//!   Clock target policy freezes rule project/directory; disabling, changing
//!   source/command/target or deleting the rule withdraws it. Schedule/template
//!   edits affect future runs, not frozen prompt bytes or this policy.
//! - Phone task admission (`verify_connector`): the option is the preset's,
//!   set on this Mac only; the phone sends a project and a preset id and
//!   never this choice. The CURRENT Board (`documents::board_authority`, so
//!   never one answered from its backup) must hold the preset with
//!   `firstSend`, a supported command equal to the row's, and exactly one
//!   card made from that command handle whose frozen, still unqueued run
//!   froze the choice and whose head is this row. The Connector's own
//!   journal must say that handle is an applied `task-create` naming that
//!   card, from a device that is still paired (`connector::task_proof`).
//!   Unticking, deleting the preset or its project, or changing the
//!   preset's command or directory withdraws it.
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
//!   the file, and a damaged main file is unreadable settings even when its
//!   backup loads: the backup is the previous save and could bring back a
//!   choice the last save withdrew, as for the approval (`authority.rs`).
//!   A phone task override is swept against the Board the same way and
//!   fenced under `documents::board_fence`, which every Board commit takes;
//!   a Board that is absent or stands as recovered is an unreadable source.
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
//!   A preset with `firstSend` (deck.json) and a phone task override
//!   (queue.json) upgrade to sticky v6 for the same reason; an older build
//!   would otherwise drop the preset's choice on its next Board save.
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
    /// Present exactly for `FirstSendOrigin::Connector`, where `rule` is the
    /// preset id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) connector: Option<ConnectorTarget>,
}

/// Readiness origins are independent of Slack content-authority triggers.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum FirstSendOrigin {
    SlackBadge,
    Clock,
    Connector,
}

/// Which document backs an override: the one it is admitted, swept and
/// fenced against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Backing {
    Settings,
    Board,
}

impl FirstSendOrigin {
    pub(crate) fn backing(self) -> Backing {
        match self {
            Self::SlackBadge | Self::Clock => Backing::Settings,
            Self::Connector => Backing::Board,
        }
    }
}

/// What an override's row is checked against, each `None` when that source
/// could not be read as current.
#[derive(Clone, Copy, Default)]
pub(crate) struct Sources<'a> {
    pub(crate) settings: Option<&'a Config>,
    pub(crate) board: Option<&'a serde_json::Value>,
}

impl<'a> Sources<'a> {
    #[cfg(test)]
    pub(crate) fn settings(config: &'a Config) -> Self {
        Self {
            settings: Some(config),
            board: None,
        }
    }
}

/// The preset a phone task override came from, frozen at admission: its
/// project, its directory as saved, and the paired device whose command made
/// the run.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct ConnectorTarget {
    pub(crate) project: String,
    pub(crate) dir: String,
    pub(crate) device: String,
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
    /// Present for a phone task alone: `rule` is then a task preset of this
    /// project and `event` the Connector command handle. Omitted otherwise
    /// so older operation fingerprints stay identical.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) preset_project: Option<String>,
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
        connector: None,
    })
}

/// `dir` as a card's directory: a leading `~` is the home directory.
fn expand_home(dir: &str) -> Option<String> {
    let home = dirs::home_dir().map(|p| p.to_string_lossy().into_owned());
    let dir = dir.trim();
    if dir.is_empty() || dir == "~" {
        home
    } else if let Some(rest) = dir.strip_prefix("~/") {
        home.map(|h| format!("{}/{rest}", h.trim_end_matches('/')))
    } else {
        Some(dir.to_owned())
    }
}

/// Preset `id` of project `project` on `board`.
fn preset<'a>(
    board: &'a serde_json::Value,
    project: &str,
    id: &str,
) -> Option<&'a serde_json::Value> {
    board
        .get("projects")?
        .as_array()?
        .iter()
        .find(|p| p["id"] == project)?
        .get("presets")?
        .as_array()?
        .iter()
        .find(|p| p["id"] == id)
}

/// Whether `preset` currently allows the override for a row launched with
/// `cmd` from a run frozen with directory `dir`.
fn preset_allows(preset: &serde_json::Value, cmd: &str, dir: &str) -> bool {
    preset["firstSend"] == true
        && preset["cmd"] == cmd
        && preset["dir"] == dir
        && supported_command(cmd)
}

/// Check a phone task head row's claim (module header). `board` is the
/// current Board or nothing, `proof` the Connector journal's own record of
/// the claimed command handle.
pub(crate) fn verify_connector(
    board: Option<&serde_json::Value>,
    claim: &FirstSendClaim,
    args: &super::ops::QueueAddArgs,
    proof: Option<&crate::connector::TaskProof>,
) -> Result<ReadinessOverride, Refusal> {
    if args.external_text {
        return Err("verbatim");
    }
    if args.mode != "at" || args.review_each || args.group.is_some() {
        return Err("step");
    }
    let project = claim.preset_project.as_deref().ok_or("no-preset")?;
    let board = board.ok_or("board-unreadable")?;
    let preset = preset(board, project, &claim.rule).ok_or("no-preset")?;
    if preset["firstSend"] != true {
        return Err("off");
    }
    if preset["cmd"] != args.cmd.as_str() {
        return Err("command");
    }
    if !supported_command(&args.cmd) {
        return Err("agent");
    }
    let proof = proof.ok_or("no-event")?;
    if proof.card_id != args.card_id {
        return Err("event");
    }
    let dir = preset["dir"].as_str().unwrap_or_default();
    if !connector_head_matches(board, args, project, &claim.rule, &claim.event, dir) {
        return Err("head");
    }
    Ok(ReadinessOverride {
        rule: claim.rule.clone(),
        trigger: FirstSendOrigin::Connector,
        clock_target: None,
        connector: Some(ConnectorTarget {
            project: project.to_owned(),
            dir: dir.to_owned(),
            device: proof.device_id.clone(),
        }),
    })
}

/// The Board's side of a phone task claim: exactly one card was made from
/// command `handle` by preset `preset`, it is this row's card, and its
/// frozen run is still unqueued, froze the choice and opens with this row.
fn connector_head_matches(
    board: &serde_json::Value,
    args: &super::ops::QueueAddArgs,
    project: &str,
    preset: &str,
    handle: &str,
    preset_dir: &str,
) -> bool {
    let Some(cards) = board.get("cards").and_then(|v| v.as_array()) else {
        return false;
    };
    let matching: Vec<_> = cards
        .iter()
        .filter(|c| {
            c["origin"]["source"] == "connector"
                && c["origin"]["key"] == handle
                && c["origin"]["badge"] == preset
        })
        .collect();
    let [card] = matching.as_slice() else {
        return false;
    };
    let run = &card["connectorRun"];
    let head = &run["initialSteps"][0];
    expand_home(preset_dir).as_deref() == Some(args.dir.as_str())
        && card["id"] == args.card_id
        && card["session"] == args.session
        && card["projectId"] == project
        && card["cmd"] == args.cmd
        && card["dir"] == args.dir
        && run["handle"] == handle
        && run["presetId"] == preset
        && run["initialQueued"] == false
        && run["firstSend"] == true
        && head["operationId"].as_str() == args.operation_id.as_deref()
        && args.operation_id.is_some()
        && head["mode"] == "at"
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
    let configured_dir = expand_home(&rule.dir);
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

/// The source that backs this row's override, when it carries one.
pub(crate) fn backing(i: &QueueItem) -> Option<Backing> {
    i.readiness_override.as_ref().map(|o| o.trigger.backing())
}

/// Whether the row's override is still backed by its rule (settings).
fn rule_still_allows(config: &Config, i: &QueueItem, o: &ReadinessOverride) -> bool {
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
                            && o.clock_target
                                .as_ref()
                                .is_some_and(|t| t.project == rule.project_id && t.dir == rule.dir)
                    }
                    FirstSendOrigin::Connector => false,
                }
        })
}

/// Whether the row's override is still backed by its preset (the Board).
fn preset_still_allows(board: &serde_json::Value, i: &QueueItem, o: &ReadinessOverride) -> bool {
    o.connector.as_ref().is_some_and(|target| {
        preset(board, &target.project, &o.rule)
            .is_some_and(|preset| preset_allows(preset, &i.cmd, &target.dir))
    })
}

/// Whether the row's override still stands: `None` when the source that
/// backs it could not be read, which proves nothing either way.
fn standing(sources: Sources<'_>, i: &QueueItem) -> Option<bool> {
    let o = i.readiness_override.as_ref()?;
    match o.trigger.backing() {
        Backing::Settings => sources.settings.map(|c| rule_still_allows(c, i, o)),
        Backing::Board => sources.board.map(|b| preset_still_allows(b, i, o)),
    }
}

/// The override check immediately before the irreversible boundary, for an
/// automatic send that relies on it (`select::relies_on_readiness_override`).
pub(crate) fn fence(i: &QueueItem, sources: Sources<'_>) -> Fence {
    match standing(sources, i) {
        None => Fence::Unverified,
        Some(true) => Fence::Clear,
        Some(false) => Fence::Revoked,
    }
}

fn revocable(i: &QueueItem) -> bool {
    i.readiness_override.is_some() && matches!(i.state, ItemState::Pending | ItemState::Failed)
}

/// Whether any unsent row carries an override backed by `source` (the tick
/// reads that source only then, or for `authority::any_authority`).
pub(crate) fn any_override(q: &QueueState, source: Backing) -> bool {
    q.items
        .iter()
        .any(|i| revocable(i) && backing(i) == Some(source))
}

/// Strip the override from every unsent row whose rule or preset no longer
/// allows it; the number of rows that lost it. A row whose source is
/// unreadable is left alone.
pub(crate) fn revoke_stale(q: &mut QueueState, sources: Sources<'_>) -> usize {
    let mut n = 0;
    for item in q.items.iter_mut().filter(|i| revocable(i)) {
        if standing(sources, item) == Some(false) {
            item.readiness_override = None;
            item.revision = item.revision.wrapping_add(1);
            n += 1;
        }
    }
    n
}
