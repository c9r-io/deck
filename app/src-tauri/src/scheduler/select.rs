//! Candidate selection: deterministic priority (backoff retry → due `at`
//! → cadence `every` → chain), one candidate per session per tick, and
//! expiry.
//!
//! # Agent hold
//! The agent hook's closed state word (`agent_status`) can only HOLD a row,
//! never release one, and it holds automatic selection only (`agent_holds`):
//! - no row of any mode is selected while the session's agent reports
//!   `needs-input` (an input request) — the pasted text and its Enter would
//!   answer the agent's question or permission prompt (typically accepting
//!   the highlighted "Yes") instead of reaching the prompt box;
//! - an `external` follow-up row (chain) is never selected automatically
//!   unless it carries content authority (`authority.rs`). Quiet alone
//!   cannot tell a finished turn from a permission prompt, and `turn-done`
//!   only says an interaction ended: the agent may still own background
//!   work and resume on its own, so no hook word is readiness or approval
//!   for text that arrived from outside deck. Such a row waits for the
//!   user's send-now — or, when the user approved this exact version of a
//!   Slack badge automation's steps, it is selected like an owner row and
//!   every hold here still applies (needs-input, Codex trust, the
//!   first-interaction gate). Authority is durable and checked at
//!   admission; readiness is not: an approval never substitutes for
//!   interaction evidence, and Signal never creates or restores approval.
//!
//! - no row of any mode is selected while the session's Signal target runs
//!   Codex in the foreground and Codex Signal is not `Trusted` for that
//!   process generation (`agent_status::CodexSignalTrust`): `Unknown` (no
//!   proof yet) and `Unavailable` (its hooks are refused as
//!   `terminal-discontinuity`, e.g. Codex 0.157's shared daemon) both hold.
//!   Without a trusted hook Deck cannot see a Codex permission prompt, and
//!   "no agent word" must not fall back to the quiet-only rule for it. The
//!   gate applies to an existing session whose Signal target has a trust
//!   proof for its current generation (whatever the foreground's name), is
//!   literally `codex`, or whose row is configured for Codex
//!   (`expected_process`, covering `node`/wrapper launches); every other
//!   session is unchanged. A failed listing proves nothing, so that tick
//!   selects nothing (`tick_selection`).
//! - First-interaction gate (Agent Bootstrap Input Safety): no automatic
//!   row for a recognized interactive agent (`admission::interactive_agent`
//!   on the row's `expected_process`: Claude or Codex) is selected into an
//!   EXISTING session until that session's current foreground generation
//!   has `AgentInteractionEstablished` evidence — Codex `Trusted`, or an
//!   accepted Claude interaction word (`agent_status::Evidence`). A process
//!   in the foreground, a quiet pane, bracketed paste or elapsed time is not
//!   evidence: a startup dialog (update, trust, first-run setup, MCP or
//!   hooks review) may own Enter. Without the Agent Status integration the
//!   gate never clears and the row waits for send-now — an intentional,
//!   documented degradation. A new generation and a Deck restart both start
//!   without evidence. The evidence is only this prerequisite; every other
//!   hold here still applies once it exists. Other process-bound rows
//!   (`expected_process` naming any other program) are unchanged.
//! - A session a SUCCESSFUL listing proves absent is not gated: the row is
//!   selected so the worker may START it, but for a recognized agent
//!   starting is not delivery (`delivery::prepare_context_with` returns
//!   `StartedAwaitingInteraction`): no prompt, no Enter, no attempt, no
//!   ledger; the row stays pending and the next tick holds it at stage
//!   `first-send`.
//!
//! Owner rows keep the quiet-only rule (plus the holds above). Manual
//! send-now (`select_for_request`) is the user acting while looking at the
//! pane and is not held. A stale `needs-input` (a question dismissed with
//! Esc fires no Stop hook) holds until the next hook word, or until the
//! poll reconciliation sees the agent leave the foreground. The hold is a
//! signal consumer pinned by `tests/signal_census.rs`.

use std::collections::HashMap;

use super::*;
use crate::agent_status;
use crate::tmux::PaneRow;

/// One tick's view of a session: its pane's last output instant and the
/// agent hook's closed state word projected from the session's Signal
/// target pane (`agent_status::signal_targets`: the active pane of its
/// current window, the pane delivery pastes into), if one reported it.
#[derive(Clone, Copy, Default)]
pub(crate) struct Observed {
    pub(crate) activity: u64,
    pub(crate) agent: Option<&'static str>,
    /// Codex Signal trust for the Signal target's current foreground
    /// generation (`agent_status::generation_evidence`): its proof when a matching
    /// trust record exists, whatever the executable name (a wrapper,
    /// `node`); else `Unknown` when the foreground is literally `codex`;
    /// else `None`. `agent_holds` also treats a row configured for Codex
    /// (`expected_process`) in an existing session as `Unknown` here. A name
    /// only decides that a hold applies — never proof.
    pub(crate) codex: Option<agent_status::CodexSignalTrust>,
    /// An accepted Claude interaction word came from the Signal target's
    /// current foreground generation (`agent_status::Evidence`).
    pub(crate) claude_interaction: bool,
    /// Not Signal: a tick-wide fact copied to every session so selection
    /// stays pure — this tick could not read the automation-authority source
    /// (settings), so no row may be sent automatically on its approval
    /// (`mark_authority_unverified`, `authority.rs`).
    pub(crate) authority_unverified: bool,
}

/// The tick could not revalidate approvals: every session's observation
/// holds rows that rely on one (`Hold::AuthorityUnverified`), without
/// touching the rows or their stored authority.
pub(crate) fn mark_authority_unverified(seen: &mut Observations) {
    for observed in seen.values_mut() {
        observed.authority_unverified = true;
    }
}

/// Session name → observation, one snapshot per tick. `activity` keeps the
/// first listed pane (unchanged quiet-time semantics); the agent word never
/// falls back to it. No process-table scan here: the Board poll reconciles
/// generations, and a word that outlived its process can only HOLD. Codex
/// trust is re-bound to the live foreground generation with two point reads
/// per Codex target (`agent_status::live_generation`), so a proof never
/// releases a later process.
pub(crate) type Observations = HashMap<String, Observed>;

pub(crate) fn observe(rows: Vec<PaneRow>) -> Observations {
    observe_with(rows, crate::agent_status::live_generation)
}

/// `observe` with the foreground-generation reader injected (the Signal
/// Trace harness passes its synthetic world's).
pub(crate) fn observe_with(
    rows: Vec<PaneRow>,
    generation_now: impl Fn(u32) -> Option<agent_status::ForegroundGeneration>,
) -> Observations {
    let agents = crate::agent_status::projections(&rows);
    let evidence = crate::agent_status::generation_evidence(&rows, generation_now);
    let mut seen = Observations::new();
    for row in rows {
        let activity = row.window_activity;
        seen.entry(row.session_name)
            .or_insert_with_key(|session| Observed {
                activity,
                agent: agents.get(session).map(|o| o.state),
                codex: evidence.get(session).and_then(|e| e.codex),
                claude_interaction: evidence.get(session).is_some_and(|e| e.claude_interaction),
                authority_unverified: false,
            });
    }
    seen
}

/// Why an automatic row is held (module header). Each is a closed plan
/// stage (`review.rs` `plan_item`) so the panel can say why a row waits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hold {
    /// the agent reported an input request (plan stage `agent`)
    NeedsInput,
    /// an external follow-up without content authority: send-now only
    /// (plan stage `external`)
    External,
    /// Codex Signal is `Unavailable` for this generation (plan stage
    /// `codex-signal`)
    CodexUnavailable,
    /// a recognized agent's current generation has no
    /// `AgentInteractionEstablished` evidence yet (plan stage `first-send`)
    FirstInteraction,
    /// the row relies on an approval that could not be revalidated this
    /// tick (plan stage `authority-unverified`); send-now still works
    AuthorityUnverified,
}

/// The recognized interactive agent a row is configured for, if any.
pub(crate) fn row_agent(i: &QueueItem) -> Option<&'static str> {
    i.expected_process
        .as_deref()
        .and_then(crate::admission::interactive_agent)
}

/// The agent hold (module header): why the hook state and interaction
/// evidence forbid an automatic paste of `i` into its session, if they do.
/// `None` for an absent session (no observation): it may be started, and
/// starting a recognized agent never delivers.
pub(crate) fn hold_reason(i: &QueueItem, seen: Option<&Observed>) -> Option<Hold> {
    let agent = seen.and_then(|o| o.agent);
    if agent == Some(agent_status::NEEDS_INPUT) {
        return Some(Hold::NeedsInput);
    }
    // provenance stays external; only the row's own content authority
    // (`authority.rs`, verified at admission, swept on revocation) lifts
    // this one hold — never a hook word, quiet time or elapsed time
    if i.external && i.mode == "chain" && i.authority.is_none() {
        return Some(Hold::External);
    }
    let o = seen?;
    // a failed read of the authority source is no proof the approval
    // still stands (and no proof it was revoked): hold, keep everything
    if o.authority_unverified && relies_on_authority(i) {
        return Some(Hold::AuthorityUnverified);
    }
    let configured = row_agent(i);
    // Codex: the target's proof or literal `codex` foreground, or a row
    // configured for Codex (a wrapper or `node` foreground) — `Trusted` is
    // its interaction evidence
    let codex = o
        .codex
        .or((configured == Some("codex")).then_some(agent_status::CodexSignalTrust::Unknown));
    match codex {
        Some(agent_status::CodexSignalTrust::Unavailable) => return Some(Hold::CodexUnavailable),
        Some(agent_status::CodexSignalTrust::Unknown) => return Some(Hold::FirstInteraction),
        _ => {}
    }
    if configured == Some("claude") && !o.claude_interaction {
        return Some(Hold::FirstInteraction);
    }
    None
}

/// Whether any hold applies (`hold_reason`).
pub(crate) fn agent_holds(i: &QueueItem, seen: Option<&Observed>) -> bool {
    hold_reason(i, seen).is_some()
}

/// Deterministic candidate order within a session: retries whose backoff
/// elapsed, then the earliest-due at, then a cadence-due rule, then chains.
fn priority(i: &QueueItem) -> u8 {
    if i.state == ItemState::Failed {
        return 0;
    }
    match i.mode.as_str() {
        "at" => 1,
        "every" => 2,
        _ => 3,
    }
}

/// The first live step of an item's group (lowest seq still queued). Only
/// the head step may fire; a dead head therefore stalls the whole group.
fn group_head<'a>(q: &'a QueueState, i: &QueueItem) -> Option<&'a QueueItem> {
    let g = i.group.as_deref()?;
    q.items
        .iter()
        .filter(|x| x.group.as_deref() == Some(g) && x.state != ItemState::ReviewApproved)
        .min_by_key(|x| (x.seq.unwrap_or(1), x.added))
}

fn eligible(
    q: &QueueState,
    i: &QueueItem,
    now: u64,
    now_min: u32,
    activity: &Observations,
) -> bool {
    if is_review(i)
        || !review_allows(q, i)
        || i.paused
        || i.state.blocks_firing()
        || item_dead(i)
        || !retry_ok(i, now)
    {
        return false;
    }
    // the card (or its project) was deleted: nothing of this session ever
    // fires again, and in particular fire_item never restarts its tmux
    // session. Belt and braces — clear_session_items already dropped the
    // items; this also covers a file hand-edited between runs.
    if is_cancelled(q, &i.session) {
        return false;
    }
    // one prompt per session at a time: every mode honors the send gap
    if q.last_fired
        .get(&i.session)
        .is_some_and(|t| now < t + SESSION_MIN_GAP_SECS)
    {
        return false;
    }
    // group discipline: only the head step of a group is ever a candidate
    if i.mode != "every" {
        if let Some(h) = group_head(q, i) {
            if h.id != i.id {
                return false;
            }
        }
    }
    if agent_holds(i, activity.get(&i.session)) {
        return false;
    }
    match i.mode.as_str() {
        "at" => i.at.map(|t| now >= t).unwrap_or(false),
        "chain" => activity
            .get(&i.session)
            .map(|o| now >= o.activity + i.quiet_secs.unwrap_or(CHAIN_QUIET_SECS))
            .unwrap_or(true), // dead session = quiet; fire_item restarts it
        "every" => {
            // a rule may not start a new iteration while any item of its
            // previous one is still live (queued, retrying or blocked)
            every_due(i, now, now_min)
                && !q
                    .items
                    .iter()
                    .any(|x| x.rule.as_deref() == Some(i.id.as_str()))
        }
        _ => false,
    }
}

/// The ONE item this session may fire now — recomputed from fresh state
/// immediately before every send, never from a stale tick-wide snapshot.
pub(crate) fn select_for_session(
    q: &QueueState,
    session: &str,
    now: u64,
    now_min: u32,
    activity: &Observations,
) -> Option<QueueItem> {
    q.items
        .iter()
        .filter(|i| i.session == session && eligible(q, i, now, now_min, activity))
        .min_by_key(|i| {
            let class_time = match (i.state, i.mode.as_str()) {
                (ItemState::Failed, _) => i.last_attempt_at.unwrap_or(0),
                (_, "at") => i.at.unwrap_or(0),
                _ => i.added,
            };
            (priority(i), class_time, i.added)
        })
        .cloned()
}

/// Explicit manual-now selection skips only the schedule now_min/quiet/window
/// and the agent hold (the user is acting while looking at the pane).
/// It retains every ordering and exclusivity invariant: pause/ambiguous/dead,
/// tombstone, session gap, group head and one-active-rule-iteration.
pub(super) fn select_requested(
    q: &QueueState,
    session: &str,
    id: &str,
    now: u64,
) -> Option<QueueItem> {
    let item = q
        .items
        .iter()
        .find(|i| i.id == id && i.session == session)?;
    if is_review(item)
        || !review_allows(q, item)
        || item.paused
        || item.state.blocks_firing()
        || item_dead(item)
        || is_cancelled(q, session)
        || q.last_fired
            .get(session)
            .is_some_and(|t| now < t + SESSION_MIN_GAP_SECS)
    {
        return None;
    }
    if item.mode != "every" && group_head(q, item).is_some_and(|head| head.id != item.id) {
        return None;
    }
    if item.mode == "every"
        && q.items
            .iter()
            .any(|other| other.rule.as_deref() == Some(item.id.as_str()))
    {
        return None;
    }
    Some(item.clone())
}

pub(super) fn select_for_request(
    q: &QueueState,
    session: &str,
    now: u64,
    now_min: u32,
    activity: &Observations,
    requested: Option<&str>,
) -> Option<QueueItem> {
    match requested {
        Some(id) => select_requested(q, session, id, now),
        None => select_for_session(q, session, now, now_min, activity),
    }
}

/// Pure per-tick candidate selection (unit-tested; the thread only adds IO):
/// at most ONE candidate per session, sessions independent of each other.
pub(crate) fn select_due(
    q: &QueueState,
    now: u64,
    now_min: u32,
    activity: &Observations,
) -> Vec<QueueItem> {
    let mut sessions: Vec<&str> = q.items.iter().map(|i| i.session.as_str()).collect();
    sessions.sort_unstable();
    sessions.dedup();
    sessions
        .into_iter()
        .filter_map(|s| select_for_session(q, s, now, now_min, activity))
        .collect()
}

/// The tick's automatic candidates. `listing` is `None` when the pane
/// listing failed: that proves no session absent (a live Codex may be
/// waiting on a permission prompt), so nothing is selected this tick and the
/// next tick simply tries again. A Deck server positively proven absent or
/// empty is NOT a failure but an empty listing
/// (`tmux_lifecycle::scheduler_pane_listing`): nothing exists to protect,
/// so a due row may be selected and its session started — and starting a
/// recognized agent never delivers (`StartedAwaitingInteraction`).
pub(crate) fn tick_selection(
    q: &QueueState,
    now: u64,
    now_min: u32,
    listing: Option<&Observations>,
) -> Vec<QueueItem> {
    listing.map_or_else(Vec::new, |activity| select_due(q, now, now_min, activity))
}

/// A rule whose stop instant passed (while deck slept, typically).
pub(crate) fn expired(i: &QueueItem, now: u64) -> bool {
    i.mode == "every" && i.until_at.map(|t| now >= t).unwrap_or(false)
}

/// Expired rules die quietly (their stop instant passed while sleeping).
pub(crate) fn purge_expired(q: &mut QueueState, now: u64) -> bool {
    let n0 = q.items.len();
    q.items.retain(|i| !expired(i, now));
    q.items.len() != n0
}
