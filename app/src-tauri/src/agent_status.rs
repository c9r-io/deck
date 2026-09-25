//! agent_status.rs — modular, content-free agent state for cards.
//!
//! Agent CLIs (Claude Code today; Codex etc. as later modules) run hooks
//! inside deck's tmux panes. Each hook invokes the bundled
//! `deck-status-helper`, which forwards ONE closed status word plus the pane
//! identity it inherited from `$TMUX`/`$TMUX_PANE` to a local unix socket in
//! the deck data dir. This module owns that socket, the closed vocabulary,
//! the pane-generation store, the ONE selector of a session's Signal pane,
//! and the reconciliation that clears state when its generation ends.
//!
//! Design rules (mirror the scheduler's context philosophy):
//! - The state is a CLOSED enum. Prompt text, notification messages and hook
//!   payloads never enter this module — the helper already discarded them.
//! - Identity (FR-SI-03). An observation is stored per pane generation
//!   (`PaneKey`: tmux server pid + pane id) and bound to the pane's session
//!   id, pane process and FOREGROUND GENERATION — the tty foreground
//!   process-group leader's pid and birth instant (`ForegroundGeneration`).
//!   It lives exactly as long as all of those are unchanged; executable
//!   names play no part, and there is never a per-agent executable list.
//!   Admission requires the leader in the reporting helper's ancestry,
//!   read from the same process-table snapshot (`ingest`).
//! - Projection. A card's Signal is the observation of ONE pane: the active
//!   pane of its session's current window (`signal_targets`) — where deck
//!   delivers input. No first-pane fallback: without a unique target there
//!   is no Signal. An event from any other pane is stored under that pane
//!   and reaches no card-level surface (status, attention, notification,
//!   Dock, scheduler hold) until its pane becomes the target.
//! - Interaction identity (FR-SI-04). Wire v1 `{v:1, source, state,
//!   socket, server_pid, pane}`; v2 adds `interaction`, the SOURCE's own id
//!   (Codex `turn_id`, Claude Code `prompt_id`; runtime-proven, see
//!   `deck-status-helper`) as a validated lowercase UUID — never
//!   synthesized. Both are accepted; an older, v1-only backend refuses v2
//!   as `bad-version` (fail closed: no Signal), which can happen briefly in
//!   an in-place update's replacement → relaunch window. Inside one pane
//!   generation an
//!   `Interactions` tracker keeps the current id and the recently ended ones:
//!   `working` may start a new interaction (not a recently ended one),
//!   `needs-input` belongs to the current interaction (or bootstraps one
//!   when there is none), `turn-done` ends the current one, and anything of
//!   another or an ended interaction is refused without touching the
//!   observation — so a late Stop(A) can never release B's input request.
//!   Once a generation has shown a v2 id, a v1 word is refused
//!   (`identity-downgrade`). No ordering is inferred (UUIDv7 time, lexical
//!   order and arrival order are unused), so a late `working(A)` arriving
//!   after `working(B)` but before A's Stop still becomes current — the one
//!   documented unsolved class. A paired ending is still only an interaction
//!   ending: no side-effect authority follows from identity.
//! - Attention episodes (FR-SI-05). Each accepted observation carries a
//!   Deck-local opaque `EpisodeId` (never a source id, never Agent truth,
//!   never authority): the same (interaction, word) — or, for v1, the same
//!   word — keeps it; any other accepted change or a new generation gets a
//!   new one; a refused event allocates none. `viewed` on the live entry is
//!   the ONE authoritative "this turn ending was viewed" truth, set only by
//!   `mark_viewed` for that exact episode (`notify_dismiss`), projected to
//!   every surface through `poll_sessions` (`episode`, `episode_viewed`)
//!   and the notification layer, and gone with the observation — no
//!   history, no cap, nothing persisted. Drop diagnostics: the first 20
//!   refusals one by one, then a closed-reason summary at most every ten
//!   minutes; `identity-absent` once per source when a new generation of a
//!   source that sent v2 sends v1 only (legacy advisory mode, no allowlist).
//!   The cross-layer contract is replayed by the Signal Trace harness
//!   (`signal_trace.rs` + `ui/test/signal-trace.test.mjs`).
//! - No automatic card movement. Backend readers: the poll merge in
//!   `commands.rs` (the webview's status dot, attention and run-finish
//!   reading), the scheduler's agent hold (`scheduler::observe`), which may
//!   only DELAY a queued row — never release, target or move anything — and
//!   `notify.rs` (a macOS notification while the window is not in front, and
//!   the Dock badge; `ingest` calls `notify::observe`, `reconcile` calls
//!   `notify::retain`). Every consumer, backend and webview, is pinned and
//!   classified by `tests/signal_census.rs`.
//! - The words are INTERACTION observations: `working` = an interaction is
//!   active, `needs-input` = the agent requested input, `turn-done` = the
//!   adapter observed an interaction boundary. `turn-done` does not mean the
//!   agent is idle, its process finished, no background work remains or the
//!   task succeeded (Claude Code resumes by itself when a background command
//!   finishes; a Codex interrupt leaves background terminals running). No
//!   word authorizes a side effect: not closing a card, not releasing
//!   external text into the pane.
//!
//! Adding an agent module = one entry in `SOURCES` + an installer that
//! registers that agent's own hook/notify config to call the same helper
//! with its own source word. The socket protocol and store are shared.
//!
//! # Contract
//! Agent status hooks (`agent_status.rs`, opt-in): agent CLIs report a CLOSED
//! state word (`working | needs-input | turn-done`) via the bundled
//! `deck-status-helper` (`app/status-helper/`, standalone zero-dep crate;
//! `build.rs` builds it into `binaries/` for tauri `externalBin` on every
//! build). Hook entries name the helper INSIDE the installed bundle
//! (`/Applications/deck.app/Contents/MacOS/deck-status-helper` or the
//! `~/Applications` twin) — never a copy under `~/.deck/bin`: that copy was
//! an EDR persistence signature, and a dev build once overwrote it with an
//! ad-hoc-signed binary that Claude Code then executed on every event. Only
//! a release-location install can enable hooks; dev/smoke builds get an
//! error and never touch agent config. `migrate_hooks_on_boot` (release
//! installs only) rewrites installed entries that are not what the CURRENT
//! spec describes — `hooks_are_current` compares the WHOLE entry (event,
//! matcher, helper path, style, args) and rejects a deck entry left under a
//! retired event, so a spec change (a narrowed matcher, a moved bundle, the
//! legacy copy) reaches users who enabled the toggle under an older version
//! without them touching the switch; installed-ness alone (`hooks_installed`)
//! cannot see that. It also deletes the legacy copy once nothing references
//! it. Install strips deck's entries document-wide before writing the specs,
//! but an EMPTY array the user wrote is left exactly as written. Hooks inherit `$TMUX`/`$TMUX_PANE`; the helper
//! drains the hook stdin payload and discards it after reading at most ONE
//! allowlisted top-level field (the source's interaction id, below),
//! charset-validates every field,
//! and writes one JSON line to the instance's `status.sock` (0600) — routed
//! per pane by `DECK_STATUS_SOCK`, which each deck exports into its own tmux
//! server env (tmux.rs), falling back to `~/.deck/status.sock`; so an
//! isolated/smoke instance receives its own events and never production's.
//! The helper can never carry content, exits 0 always, and silently does
//! nothing outside a deck tmux pane. The backend listener validates source/state against the module
//! registry, requires the event's socket name AND tmux server pid (generation
//! stamp — restarted servers reuse pane ids) before resolving the pane,
//! refuses shell-foreground panes, and records the pane's foreground
//! generation. An event is bound to the pane it names: the listener takes
//! the connecting process from the kernel (`procinfo::peer_pid`), walks its
//! parent chain (`procinfo::ancestry_in`, while the helper is still connected —
//! the helper waits for deck to close the stream before exiting) and refuses
//! the event unless the pane's own process (`#{pane_pid}`) is in that chain
//! (`foreign-pane`; `no-peer` without a kernel pid). Every pane has
//! `DECK_STATUS_SOCK`, so without this any program in any pane could paint
//! another card's status and attention state. Accepted residual: a program
//! in a pane can still misreport its OWN pane; since no word carries
//! side-effect authority, that is a presentation error only. Codex `async` hooks are spawned by the agent and stay in its tree;
//! an agent that detached its hooks into another session would have its
//! events refused, and that would show as the toggle reporting nothing. The
//! reporter must also descend from the pane's CURRENT foreground leader
//! (`generation-mismatch` otherwise; `no-generation` when it cannot be
//! read): proven at runtime for Claude Code 2.1.282 and Codex 0.156.1, whose
//! helper's parent is that leader for every hook word. `poll_sessions`
//! reconciles with one process-table snapshot so the state dies with the
//! pane or process generation that reported it — no TTLs. Frontend:
//! `effectiveCardStatus` (pure.js) — agent state OUTRANKS the 15s heuristic
//! (card statuses `attention`/`done`; a working agent never shows amber). The
//! Settings toggle is the user-driven writer of `~/.claude/settings.json`
//! (three entries: UserPromptSubmit→working, Notification matcher
//! `permission_prompt` ONLY→needs-input, Stop→turn-done, written in
//! Claude Code's EXEC form — bare helper path in `command`, words in
//! `args` — so Claude Code spawns the signed helper directly and no `sh -c`
//! runs per event; Codex stays shell-form + `async`, exec form being
//! undocumented there), and the
//! release-only boot migration is the sole other one; install, migrate and
//! uninstall touch only entries containing `deck.app/Contents/MacOS/deck-status-helper` (or the legacy `.deck/bin/deck-status-helper`),
//! preserve everything else including file mode, and never modify a malformed
//! file. The toggle state is DERIVED from that file — never stored twice.
//! Claude Code's Stop does not fire on Esc-interrupt; foreground
//! reconciliation and the next UserPromptSubmit heal that. `idle_prompt` is
//! deliberately NOT matched: Claude Code fires it ~60s after the prompt goes
//! idle, so EVERY finished card decayed from `done` into `attention` a minute
//! later and the attention colour stopped meaning anything. needs-input means
//! a question raised DURING a turn; an idle prompt after a turn is `turn-done`,
//! which is already on screen. The Codex module
//! uses lifecycle hooks in `$CODEX_HOME/hooks.json` (same document shape as
//! Claude's, so ONE marker-based JSON merge engine serves both via per-agent
//! spec tables): UserPromptSubmit→working, PermissionRequest→needs-input,
//! Stop→turn-done, Interrupt→turn-done, all `"async": true` so the helper can
//! never block a turn. Multiple hooks per event coexist, so this never
//! conflicts with the user's own hooks or `notify` program — the earlier
//! notify/`config.toml` route was DROPPED for exactly that conflict (Codex
//! allows one notify program only; chain-forwarding it was rejected as too
//! much surface). Known caveat: Codex's Stop fires when the model ATTEMPTS to
//! stop, so another Stop hook forcing continuation makes "done" slightly
//! early. Adding an agent module = one `SOURCES` entry + its own installer
//! spec calling the same helper with its own source word, behind its own
//! Settings toggle.

use std::collections::HashMap;

use crate::tmux::PaneRow;
use std::io::Read;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use crate::applog;
use crate::error::{DeckError, ErrorKind};
use crate::sync::LockRecover;

/// Registered agent modules.
const SOURCES: &[&str] = &["claude-code", "codex"];

/// Closed state vocabulary — the only words that ever reach the frontend.
pub(crate) const WORKING: &str = "working";
pub(crate) const NEEDS_INPUT: &str = "needs-input";
pub(crate) const TURN_DONE: &str = "turn-done";
pub(crate) const STATES: &[&str] = &[WORKING, NEEDS_INPUT, TURN_DONE];

#[derive(Debug, PartialEq)]
pub(crate) struct Event {
    source: String,
    state: &'static str,
    socket: String,
    server_pid: u32,
    pane: String,
    /// v2 only: the source's own interaction id (Codex `turn_id`, Claude
    /// Code `prompt_id`), a lowercase UUID the helper validated.
    interaction: Option<String>,
}

/// `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`, lowercase hex — the one shape an
/// interaction id may take on the wire (mirrors the helper's `uuid_ok`).
fn interaction_ok(s: &str) -> bool {
    s.len() == 36
        && s.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_digit() || (b'a'..=b'f').contains(&b),
        })
}

/// A pane of one tmux server generation. tmux never reuses a pane id within
/// a server's life; a restarted server has a new pid, so an old key can
/// never name a new pane.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PaneKey {
    server_pid: u32,
    pane_id: String,
}

impl PaneKey {
    fn of(row: &PaneRow) -> Self {
        Self {
            server_pid: row.server_pid,
            pane_id: row.pane_id.clone(),
        }
    }
}

/// The tty foreground process group that owned the reporting helper: its
/// leader's pid and birth instant. A wrapper (`caffeinate claude`) may be
/// the leader; that is still one execution generation. pid + start tells a
/// relaunched agent (same executable name, new process) and a reused pid
/// apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ForegroundGeneration {
    pid: u32,
    start_seconds: u64,
    start_micros: u32,
}

/// One accepted observation, bound to the pane generation that reported it.
struct Entry {
    state: &'static str,
    /// The session name the pane belonged to when it reported (the key of
    /// every card-level surface; `notify_dismiss` names it).
    session: String,
    session_id: String,
    pane_pid: u32,
    generation: ForegroundGeneration,
    /// Interaction tracking inside this generation (FR-SI-04); a new
    /// generation starts a fresh tracker.
    interactions: Interactions,
    /// The attention episode of the current observation (FR-SI-05), its
    /// private sameness key, and whether the user has viewed it.
    episode: EpisodeId,
    last: Option<(Option<String>, &'static str)>,
    viewed: bool,
}

/// Deck-local, process-local attention episode (FR-SI-05): "is this the
/// same accepted observation that was already surfaced or viewed?". An
/// opaque counter — never a source id, never Agent truth, never side-effect
/// authority. Only an ACCEPTED observation allocates one: the same (source
/// interaction, word) — or, for v1, the same word — keeps its episode; any
/// other accepted change, or a new foreground generation, gets a new one.
pub(crate) type EpisodeId = u64;

static NEXT_EPISODE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// What a card-level surface may read about one pane's observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Observation {
    pub(crate) state: &'static str,
    pub(crate) episode: EpisodeId,
    /// The user has viewed this exact `turn-done` episode
    /// (`mark_viewed`). The ONE authoritative viewed truth: it lives with
    /// the live observation and dies with it — no history, no cap.
    pub(crate) viewed: bool,
}

/// CLI drift (FR-SI-05): true exactly once per source, the first time a NEW
/// foreground generation of a source that has sent v2 in this Deck process
/// reports v1 only. Deck then runs that generation in legacy advisory mode
/// (it cannot know better); the line makes the drift visible. No version
/// list, no network check.
fn identity_absent(source: &str, v2: bool, fresh_generation: bool) -> bool {
    static SOURCES_SEEN: Mutex<(Vec<String>, Vec<String>)> = Mutex::new((Vec::new(), Vec::new()));
    let mut guard = SOURCES_SEEN.lock_or_recover();
    let (with_identity, warned) = &mut *guard;
    if v2 {
        if !with_identity.iter().any(|s| s == source) {
            with_identity.push(source.to_string());
        }
        return false;
    }
    if !fresh_generation {
        return false;
    }
    if with_identity.iter().any(|s| s == source) && !warned.iter().any(|s| s == source) {
        warned.push(source.to_string());
        return true;
    }
    false
}

/// How many ended interaction ids a pane generation remembers.
const ENDED_CAP: usize = 8;

/// Whether this generation has shown a source interaction id yet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Mode {
    /// Only v1 events so far: generation-bound words, today's semantics.
    #[default]
    Legacy,
    /// A v2 event was seen: every later event must carry its id. A v1 event
    /// can no longer change anything (no silent downgrade on a source that
    /// lost its id field for one event).
    IdentityBound,
}

/// Per pane generation: which interaction is current and which recently
/// ended, by the SOURCE's own id. There is no ordering metadata in either
/// source's hooks, so this only separates interactions; it never orders
/// them (UUIDv7 time, lexical order and arrival order are deliberately
/// unused). Known unsolved class: `working(B)` then a late `working(A)`
/// that arrives before `Stop(A)` has marked A ended makes A current.
#[derive(Default)]
struct Interactions {
    mode: Mode,
    current: Option<String>,
    ended: std::collections::VecDeque<String>,
}

impl Interactions {
    fn has_ended(&self, id: &str) -> bool {
        self.ended.iter().any(|ended| ended == id)
    }

    fn end(&mut self, id: &str) {
        if !self.has_ended(id) {
            self.ended.push_back(id.to_string());
            if self.ended.len() > ENDED_CAP {
                self.ended.pop_front();
            }
        }
    }

    /// Admit one word. `Ok` = the pane's observation becomes `state`; `Err`
    /// = a categorized refusal that changes no observation (the tracker may
    /// still learn that an id ended). None of this is side-effect
    /// authority: a paired turn ending is still only an interaction ending.
    fn admit(
        &mut self,
        state: &'static str,
        interaction: Option<&str>,
    ) -> Result<(), &'static str> {
        let Some(id) = interaction else {
            return match self.mode {
                Mode::Legacy => Ok(()),
                Mode::IdentityBound => Err("identity-downgrade"),
            };
        };
        self.mode = Mode::IdentityBound;
        let current = self.current.as_deref() == Some(id);
        match state {
            WORKING if self.has_ended(id) => Err("stale-interaction"),
            // a new interaction may begin before a late Stop of the old one
            WORKING => {
                self.current = Some(id.to_string());
                Ok(())
            }
            NEEDS_INPUT if self.has_ended(id) => Err("stale-interaction"),
            NEEDS_INPUT if current => Ok(()),
            // missing start (or a Deck restart mid-interaction): bootstrap
            NEEDS_INPUT if self.current.is_none() => {
                self.current = Some(id.to_string());
                Ok(())
            }
            NEEDS_INPUT => Err("interaction-mismatch"),
            _ if self.has_ended(id) => Err("duplicate-interaction"),
            _ if current => {
                self.end(id);
                self.current = None;
                Ok(())
            }
            // an unpaired boundary (its start was missed): advisory only
            _ if self.current.is_none() => {
                self.end(id);
                Ok(())
            }
            // a late ending of another interaction: remembered, not applied
            _ => {
                self.end(id);
                Err("interaction-mismatch")
            }
        }
    }
}

static AGENTS: Mutex<Option<HashMap<PaneKey, Entry>>> = Mutex::new(None);

fn with_agents<R>(f: impl FnOnce(&mut HashMap<PaneKey, Entry>) -> R) -> R {
    let mut guard = AGENTS.lock_or_recover();
    f(guard.get_or_insert_with(HashMap::new))
}

/// One process-table snapshot (`procinfo::processes`).
pub(crate) type ProcessTable = HashMap<u32, crate::procinfo::ProcessInfo>;

// ---------- event parsing (closed validation at the trust boundary) ---------

/// Tiny hand validation instead of serde structs: every field is checked
/// against a closed shape, and anything else — extra fields, wrong types,
/// content — is a categorized drop.
pub(crate) fn parse_event(line: &str) -> Result<Event, &'static str> {
    let value: serde_json::Value = serde_json::from_str(line).map_err(|_| "bad-json")?;
    let obj = value.as_object().ok_or("bad-json")?;
    let version = obj.get("v").and_then(|v| v.as_u64());
    if !matches!(version, Some(1 | 2)) {
        return Err("bad-version");
    }
    // v1 carries no interaction; v2 carries exactly one valid id
    let interaction = match (version, obj.get("interaction")) {
        (Some(1), None) => None,
        (Some(2), Some(id)) => Some(
            id.as_str()
                .filter(|id| interaction_ok(id))
                .ok_or("bad-interaction")?
                .to_string(),
        ),
        _ => return Err("bad-interaction"),
    };
    let source = obj
        .get("source")
        .and_then(|v| v.as_str())
        .filter(|s| SOURCES.contains(s))
        .ok_or("unknown-source")?;
    let state = STATES
        .iter()
        .find(|s| Some(**s) == obj.get("state").and_then(|v| v.as_str()))
        .ok_or("unknown-state")?;
    let socket = obj
        .get("socket")
        .and_then(|v| v.as_str())
        .filter(|s| *s == crate::tmux::socket())
        .ok_or("other-server")?;
    let server_pid = obj
        .get("server_pid")
        .and_then(|v| v.as_u64())
        .and_then(|v| u32::try_from(v).ok())
        .ok_or("bad-pid")?;
    let pane = obj
        .get("pane")
        .and_then(|v| v.as_str())
        .filter(|p| {
            p.len() >= 2
                && p.len() <= 10
                && p.starts_with('%')
                && p[1..].bytes().all(|b| b.is_ascii_digit())
        })
        .ok_or("bad-pane")?;
    Ok(Event {
        source: source.to_string(),
        state,
        socket: socket.to_string(),
        server_pid,
        pane: pane.to_string(),
        interaction,
    })
}

// ---------- identity: foreground generation and the card's pane ---------------

/// The foreground generation of the tty `pane_pid` controls, read from ONE
/// process-table snapshot: the tty's foreground process group, its leader
/// (pid == pgid, same tty) and the leader's birth instant. `None` when any
/// of it cannot be established — callers fail closed.
pub(crate) fn foreground_generation(
    table: &ProcessTable,
    pane_pid: u32,
) -> Option<ForegroundGeneration> {
    let tty = table.get(&pane_pid)?.tty;
    let leader = crate::procinfo::foreground_leader(table, tty)?;
    let info = table.get(&leader)?;
    (info.start_seconds != 0).then_some(ForegroundGeneration {
        pid: leader,
        start_seconds: info.start_seconds,
        start_micros: info.start_micros,
    })
}

/// The ONE selector of the pane whose observation is a session's Signal:
/// the active pane of the session's current window (`window_active &&
/// pane_active`) — the pane `pane_target(session)` reaches, i.e. where deck
/// delivers input. Exactly one per session or none: a session whose active
/// target cannot be established uniquely has no Signal. Never the first
/// listed pane. The poll (status, attention, run finish), the scheduler's
/// agent hold and notifications all read through this.
pub(crate) fn signal_targets(rows: &[PaneRow]) -> HashMap<String, &PaneRow> {
    let mut targets: HashMap<String, Option<&PaneRow>> = HashMap::new();
    for row in rows
        .iter()
        .filter(|row| row.window_active && row.pane_active)
    {
        targets
            .entry(row.session_name.clone())
            .and_modify(|seen| *seen = None) // a second marked pane: ambiguous
            .or_insert(Some(row));
    }
    targets
        .into_iter()
        .filter_map(|(session, row)| Some((session, row?)))
        .collect()
}

/// Retirement evidence (FR-SI-03.1): the foreground an automation's
/// "close the card" finish rule may pair with an absent agent word ("the
/// agent program exited, a shell is in front"). It exists ONLY for a
/// session with exactly one pane — the one Deck created — and is that
/// pane's foreground. A pane-local fact must never become session-lifecycle
/// authority: in a manually split session an active shell pane says nothing
/// about a live agent in another pane, and retiring the card would kill the
/// whole tmux session. Multi-pane sessions therefore have no automatic
/// retirement at all, whichever pane is active; the Signal projection
/// (`signal_targets`) is unaffected.
pub(crate) fn finish_foregrounds(rows: &[PaneRow]) -> HashMap<String, String> {
    let mut panes: HashMap<&str, usize> = HashMap::new();
    for row in rows {
        *panes.entry(row.session_name.as_str()).or_default() += 1;
    }
    signal_targets(rows)
        .into_iter()
        .filter(|(session, _)| panes.get(session.as_str()) == Some(&1))
        .map(|(session, target)| (session, target.command.clone()))
        .collect()
}

/// The observation stored for `target` (a pane from `signal_targets`), if
/// it still belongs to that pane's session and process. The foreground
/// generation is checked by `reconcile` against a process table; readers
/// without one (the scheduler tick) only ever get a word that can hold.
pub(crate) fn projected(target: &PaneRow) -> Option<Observation> {
    with_agents(|agents| {
        agents
            .get(&PaneKey::of(target))
            .filter(|entry| {
                entry.session_id == target.session_id && entry.pane_pid == target.pane_pid
            })
            .map(|entry| Observation {
                state: entry.state,
                episode: entry.episode,
                viewed: entry.viewed,
            })
    })
}

/// The user viewed `episode` of `session` (`notify_dismiss`). Marks exactly
/// that live `turn-done` episode — also on an inactive pane of the session,
/// whose episode may be projected again — and nothing else: a stale episode
/// that no longer exists is a no-op, never "whatever is current". Returns
/// whether an episode was (or already had been) marked.
pub(crate) fn mark_viewed(session: &str, episode: EpisodeId) -> bool {
    with_agents(|agents| {
        agents
            .values_mut()
            .find(|entry| {
                entry.session == session && entry.episode == episode && entry.state == TURN_DONE
            })
            .map(|entry| entry.viewed = true)
            .is_some()
    })
}

/// session → projected observation, for every session with a Signal target.
pub(crate) fn projections(rows: &[PaneRow]) -> HashMap<String, Observation> {
    signal_targets(rows)
        .into_iter()
        .filter_map(|(session, target)| Some((session, projected(target)?)))
        .collect()
}

/// How far up from the reporting process the pane's process may be. A hook
/// is the agent's child (or a `sh -c` wrapper's grandchild) under the
/// pane's shell; 32 hops is far beyond any real layering.
const ORIGIN_HOPS: usize = 32;

/// What the listener read while the helper was still connected: the
/// kernel's peer pid and one process-table snapshot. The peer's ancestry,
/// the pane's foreground leader and its birth instant all come from this one
/// snapshot, so the compared facts describe the same instant.
pub(crate) struct Origin {
    pub(crate) peer: Option<u32>,
    pub(crate) table: ProcessTable,
}

/// Validate one wire line and commit it to the store. Admission, in order:
/// a kernel peer (`no-peer`), a pane of this server generation
/// (`no-such-pane`), the pane's own process in the peer's ancestry
/// (`foreign-pane`), no shell in the foreground (`shell-foreground`), an
/// establishable foreground generation (`no-generation`) whose leader is
/// also in the peer's ancestry (`generation-mismatch`: the reporter is not
/// the pane's current foreground program — a late event of an exited or
/// replaced agent). Runtime proof (Claude Code 2.1.282, Codex 0.156.1): the
/// helper's parent IS the foreground leader; no fixed depth is required, so
/// a wrapper leader is accepted.
///
/// The observation is stored under its pane. Only an event from the pane
/// that is its session's Signal target reaches the notification layer;
/// any other pane's event waits in the store until that pane becomes the
/// target (`reconcile` projects it then). `listing` is injected so tests
/// run the full path without a live tmux server.
pub(crate) fn ingest(
    line: &str,
    origin: &Origin,
    listing: impl FnOnce() -> Option<Vec<PaneRow>>,
) -> Result<(), &'static str> {
    let event = parse_event(line)?;
    let chain = origin
        .peer
        .map(|peer| crate::procinfo::ancestry_in(&origin.table, peer, ORIGIN_HOPS))
        .unwrap_or_default();
    if chain.is_empty() {
        return Err("no-peer");
    }
    let rows = listing().ok_or("no-such-pane")?;
    let row = rows
        .iter()
        .find(|row| row.pane_id == event.pane && row.server_pid == event.server_pid)
        .filter(|row| crate::tmux::validate_session_name(&row.session_name).is_ok())
        .ok_or("no-such-pane")?;
    if !chain.contains(&row.pane_pid) {
        return Err("foreign-pane");
    }
    // An agent hook while a plain shell owns the pane foreground has no
    // process to bind the state's lifetime to — refuse rather than flicker.
    if crate::context::shell_process(Some(&row.command)) {
        return Err("shell-foreground");
    }
    let generation = foreground_generation(&origin.table, row.pane_pid).ok_or("no-generation")?;
    if !chain.contains(&generation.pid) {
        return Err("generation-mismatch");
    }
    let key = PaneKey::of(row);
    let observation = with_agents(|agents| -> Result<Observation, &'static str> {
        // the same pane generation keeps its interaction tracker; anything
        // else (a new foreground generation, a replaced session or pane
        // process) starts fresh
        let same_generation = agents.get(&key).is_some_and(|entry| {
            entry.session_id == row.session_id
                && entry.pane_pid == row.pane_pid
                && entry.generation == generation
        });
        if identity_absent(&event.source, event.interaction.is_some(), !same_generation) {
            applog(&format!("[agent-status] {} identity-absent", event.source));
        }
        if !same_generation {
            agents.insert(
                key.clone(),
                Entry {
                    state: event.state,
                    session: row.session_name.clone(),
                    session_id: row.session_id.clone(),
                    pane_pid: row.pane_pid,
                    generation,
                    interactions: Interactions::default(),
                    episode: 0,
                    last: None,
                    viewed: false,
                },
            );
        }
        let entry = agents.get_mut(&key).ok_or("no-such-pane")?;
        entry
            .interactions
            .admit(event.state, event.interaction.as_deref())?;
        entry.state = event.state;
        // only an accepted observation allocates or changes an episode
        let sameness = (event.interaction.clone(), event.state);
        if entry.last.as_ref() != Some(&sameness) {
            entry.last = Some(sameness);
            entry.episode = NEXT_EPISODE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            entry.viewed = false;
        }
        Ok(Observation {
            state: entry.state,
            episode: entry.episode,
            viewed: entry.viewed,
        })
    })?;
    let targeted = signal_targets(&rows)
        .get(&row.session_name)
        .is_some_and(|target| target.pane_id == row.pane_id);
    applog(&format!(
        "[agent-status] {} {} s={} target={} v={} e={}",
        event.source,
        event.state,
        crate::applog::session_tag(&row.session_name),
        u8::from(targeted),
        if event.interaction.is_some() { 2 } else { 1 },
        observation.episode
    ));
    // the desktop attention loop (notification while away, Dock badge)
    // hears only the session's Signal target
    if targeted {
        crate::notify::observe(&row.session_name, observation);
    }
    Ok(())
}

// ---------- poll integration ------------------------------------------------

/// Called from every `poll_sessions` with the whole pane listing and the
/// poll's one process-table snapshot. An observation survives only while
/// its exact pane (`server_pid`, `pane_id`) still exists with the same
/// session id and pane process, AND the pane's foreground generation (leader
/// pid + birth instant) is the one that reported it; anything that cannot
/// be re-established is removed. Executable names play no part. Then the
/// notification layer is brought to the projected Signal of every session:
/// a session whose target changed hears its new target's word, a session
/// without one is forgotten.
pub(crate) fn reconcile(rows: &[PaneRow], table: &ProcessTable) {
    with_agents(|agents| {
        agents.retain(|key, entry| {
            rows.iter()
                .find(|row| PaneKey::of(row) == *key)
                .is_some_and(|row| {
                    row.session_id == entry.session_id
                        && row.pane_pid == entry.pane_pid
                        && foreground_generation(table, row.pane_pid) == Some(entry.generation)
                })
        });
    });
    let projected = projections(rows);
    for (session, observation) in &projected {
        crate::notify::observe(session, *observation);
    }
    crate::notify::retain(&projected.into_keys().collect());
}

#[cfg(test)]
pub(crate) fn reset_for_tests() {
    with_agents(|agents| agents.clear());
}

// ---------- socket listener --------------------------------------------------

const MAX_LINE: usize = 8 * 1024;

fn read_first_line(stream: &mut UnixStream) -> Option<String> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() > MAX_LINE {
                    return None;
                }
                if buf.contains(&b'\n') {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let line = buf.split(|b| *b == b'\n').next().unwrap_or(&[]);
    String::from_utf8(line.to_vec()).ok()
}

/// Every reason `ingest` can refuse an event with — the closed vocabulary
/// of the drop diagnostics (anything else is counted as `other`).
const DROP_REASONS: &[&str] = &[
    "bad-json",
    "bad-version",
    "bad-interaction",
    "unknown-source",
    "unknown-state",
    "other-server",
    "bad-pid",
    "bad-pane",
    "no-peer",
    "no-such-pane",
    "foreign-pane",
    "shell-foreground",
    "no-generation",
    "generation-mismatch",
    "stale-interaction",
    "interaction-mismatch",
    "duplicate-interaction",
    "identity-downgrade",
];

/// How often the drop summary may be written.
const DROP_SUMMARY_EVERY: Duration = Duration::from_secs(600);

/// Drop diagnostics (FR-SI-05): the first 20 refusals are logged one by
/// one; every refusal is also counted by its closed reason, and the counts
/// are written as ONE summary line at most every ten minutes, then reset —
/// so "why was this Signal rejected?" stays answerable in a long session
/// without letting a hostile local writer grow app.log.
struct Drops {
    detailed: u32,
    counts: std::collections::BTreeMap<&'static str, u32>,
    since: std::time::Instant,
}

impl Drops {
    fn new(now: std::time::Instant) -> Self {
        Self {
            detailed: 0,
            counts: std::collections::BTreeMap::new(),
            since: now,
        }
    }

    /// The lines to log for one refusal (`Some`) or one accepted event.
    fn observe(&mut self, reason: Option<&'static str>, now: std::time::Instant) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(reason) = reason {
            let reason = DROP_REASONS
                .iter()
                .find(|known| **known == reason)
                .copied()
                .unwrap_or("other");
            *self.counts.entry(reason).or_default() += 1;
            self.detailed += 1;
            if self.detailed <= 20 {
                lines.push(format!("[agent-status] dropped ({reason})"));
            } else if self.detailed == 21 {
                lines.push("[agent-status] further drops summarized every 10 min".into());
            }
        }
        if !self.counts.is_empty() && now.duration_since(self.since) >= DROP_SUMMARY_EVERY {
            let summary: Vec<String> = self
                .counts
                .iter()
                .map(|(r, n)| format!("{r}={n}"))
                .collect();
            lines.push(format!("[agent-status] drops {}", summary.join(" ")));
            self.counts.clear();
            self.since = now;
        }
        lines
    }
}

fn handle_stream(mut stream: UnixStream, drops: &mut Drops) {
    let Some(line) = read_first_line(&mut stream) else {
        return;
    };
    if line.is_empty() {
        return;
    }
    // The helper stays connected until deck closes the stream, so its
    // ancestry and the pane's foreground generation are read from one
    // snapshot while it is alive; the (slow) pane listing runs after the
    // helper has been released.
    let origin = Origin {
        peer: crate::procinfo::peer_pid(&stream),
        table: crate::procinfo::processes(),
    };
    drop(stream);
    // categorized, content-free, and bounded — a hostile local writer
    // must not be able to grow app.log without limit
    let result = ingest(&line, &origin, || crate::tmux::list_panes().ok());
    for line in drops.observe(result.err(), std::time::Instant::now()) {
        applog(&line);
    }
}

pub(crate) fn listen_at(path: &Path) -> Result<UnixListener, DeckError> {
    let _ = std::fs::remove_file(path); // stale socket from a previous run
    let listener =
        UnixListener::bind(path).map_err(|e| DeckError::classified(format!("bind failed: {e}")))?;
    // the data dir is already 0700; keep the socket itself private too
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    Ok(listener)
}

/// Accept loop for the status socket. One deck instance owns the data dir
/// (instance lock), so one listener owns this socket.
pub(crate) fn spawn_listener() {
    let path = crate::datadir::deck_dir().join("status.sock");
    std::thread::spawn(move || {
        let listener = match listen_at(&path) {
            Ok(l) => l,
            Err(e) => {
                applog(&format!("[agent-status] socket unavailable ({})", e.code()));
                return;
            }
        };
        let mut drops = Drops::new(std::time::Instant::now());
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => handle_stream(stream, &mut drops),
                Err(_) => std::thread::sleep(Duration::from_secs(1)),
            }
        }
    });
}

// ---------- hook installation (shared JSON machinery) -------------------------
//
// Claude Code (~/.claude/settings.json) and Codex ($CODEX_HOME/hooks.json)
// use the same hooks document shape: hooks → EventName →
// [{matcher?, hooks: [{type: "command", command, …}]}]. One marker-based
// merge engine serves both; each agent contributes only its spec table.

/// Markers every deck-authored hook command carries; install/uninstall touch
/// only entries containing one, byte-preserving everything else in the file.
/// Current entries run the helper INSIDE the installed, signed, notarized
/// bundle — deck never drops an executable into the home directory (an app
/// writing a binary under `~` and registering it in another program's hook
/// config is an endpoint-security persistence signature, and a development
/// build once overwrote that copy with an ad-hoc-signed one). The legacy
/// marker is the pre-0.5.12 `~/.deck/bin` copy, still recognized so
/// uninstall and boot migration can retire it.
const HELPER_MARKER: &str = "deck.app/Contents/MacOS/deck-status-helper";
const LEGACY_HELPER_MARKER: &str = ".deck/bin/deck-status-helper";

const HELPER_NAME: &str = "deck-status-helper";

/// One deck hook: (event, optional matcher, state word).
type HookSpec = (&'static str, Option<&'static str>, &'static str);

/// Claude Code: `Notification` matches ONLY `permission_prompt` — a question
/// raised in the middle of a turn, which is the whole meaning of
/// "needs-input". `idle_prompt` is deliberately NOT matched: Claude Code
/// fires it ~60s after the prompt goes idle, so every finished card decayed
/// from "done" into "needs-input" a minute later and the attention colour
/// stopped meaning anything. An idle prompt after a turn is `turn-done`,
/// which is already on screen. `Stop` does not fire on user interrupt
/// (documented behavior) — foreground reconciliation and the next
/// `UserPromptSubmit` heal that gap.
const CLAUDE_HOOKS: &[HookSpec] = &[
    ("UserPromptSubmit", None, "working"),
    ("Notification", Some("permission_prompt"), "needs-input"),
    ("Stop", None, "turn-done"),
];

/// Codex lifecycle hooks (hooks.json; several hooks per event may coexist,
/// so this never conflicts with a user's own hooks or `notify` program).
/// `Stop` fires when the model attempts to stop — another Stop hook may
/// force continuation, so "done" can be slightly early; `Interrupt` maps to
/// turn-done so an Esc-interrupted card settles instead of staying "working".
const CODEX_HOOKS: &[HookSpec] = &[
    ("UserPromptSubmit", None, "working"),
    ("PermissionRequest", None, "needs-input"),
    ("Stop", None, "turn-done"),
    ("Interrupt", None, "turn-done"),
];

/// How an agent runs a hook command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HookStyle {
    /// Claude Code exec form: with `args` present the helper is spawned
    /// directly, so no `sh -c` runs per event (one process less for
    /// endpoint security to look at).
    Exec,
    /// Codex shell form with `"async": true` so the fire-and-forget helper
    /// never blocks a turn; exec form is not documented there.
    ShellAsync,
}

/// One deck hook object in the agent's document shape.
fn hook_value(style: HookStyle, helper: &str, source: &str, state: &str) -> serde_json::Value {
    match style {
        HookStyle::Exec => serde_json::json!({
            "type": "command", "command": helper, "args": [source, state], "timeout": 10
        }),
        HookStyle::ShellAsync => serde_json::json!({
            "type": "command", "command": format!("\"{helper}\" {source} {state}"),
            "timeout": 10, "async": true
        }),
    }
}

fn command_is_ours(command: &str) -> bool {
    command.contains(HELPER_MARKER) || command.contains(LEGACY_HELPER_MARKER)
}

fn entry_commands(entry: &serde_json::Value) -> impl Iterator<Item = &str> {
    entry
        .get("hooks")
        .and_then(|h| h.as_array())
        .into_iter()
        .flatten()
        .filter_map(|hook| hook.get("command").and_then(|c| c.as_str()))
}

fn entry_is_ours(entry: &serde_json::Value) -> bool {
    entry_commands(entry).any(command_is_ours)
}

/// The complete document entry one spec must produce.
fn spec_entry(
    style: HookStyle,
    helper: &str,
    source: &str,
    (_, matcher, state): &HookSpec,
) -> serde_json::Value {
    let mut entry = serde_json::json!({ "hooks": [hook_value(style, helper, source, state)] });
    if let Some(matcher) = matcher {
        entry["matcher"] = serde_json::json!(matcher);
    }
    entry
}

/// deck's entries in this document are EXACTLY what the current specs
/// describe: one entry per spec event, byte-identical matcher, helper path,
/// style and arguments, and no deck entry left under an event the specs no
/// longer name. `hooks_installed` only asks whether SOME deck entry exists
/// per event, so it cannot see a spec change — a narrowed matcher, a moved
/// bundle, a legacy `~/.deck/bin` path, a shell-form entry where exec form
/// is expected. This is the predicate boot migration repairs against.
pub(crate) fn hooks_are_current(
    root: &serde_json::Value,
    specs: &[HookSpec],
    source: &str,
    style: HookStyle,
    helper: &str,
) -> bool {
    let Some(hooks) = root.get("hooks").and_then(|h| h.as_object()) else {
        return false;
    };
    let ours = |list: &serde_json::Value| {
        list.as_array()
            .into_iter()
            .flatten()
            .filter(|entry| entry_is_ours(entry))
            .count()
    };
    // nothing of ours under a retired event
    if hooks
        .iter()
        .any(|(event, list)| !specs.iter().any(|(e, _, _)| e == event) && ours(list) > 0)
    {
        return false;
    }
    specs.iter().all(|spec| {
        let Some(list) = hooks.get(spec.0).and_then(|l| l.as_array()) else {
            return false;
        };
        let mut mine = list.iter().filter(|entry| entry_is_ours(entry));
        let Some(entry) = mine.next() else {
            return false;
        };
        mine.next().is_none() && *entry == spec_entry(style, helper, source, spec)
    })
}

/// Add deck's hook entries to a parsed hooks document. Everything the user
/// wrote — other hooks, unknown keys, other events — is preserved; every
/// entry carrying the helper marker is dropped first (document-wide, so an
/// event a previous deck version registered and this one no longer does
/// cannot linger) and the current specs are written in the agent's `style`.
pub(crate) fn hooks_with_install(
    mut root: serde_json::Value,
    specs: &[HookSpec],
    source: &str,
    style: HookStyle,
    helper: &str,
) -> Result<serde_json::Value, DeckError> {
    let obj = root.as_object_mut().ok_or(DeckError::new(
        ErrorKind::Other,
        "settings file is not a JSON object",
    ))?;
    let hooks = obj
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or(DeckError::new(
            ErrorKind::Other,
            "the hooks key is not a JSON object",
        ))?;
    let mut emptied = Vec::new();
    for (event, list) in hooks.iter_mut() {
        let Some(list) = list.as_array_mut() else {
            continue;
        };
        let before = list.len();
        list.retain(|entry| !entry_is_ours(entry));
        if list.is_empty() && before > 0 {
            emptied.push(event.clone());
        }
    }
    // an event only this document's deck entry populated leaves no husk —
    // but an empty array the USER wrote is left exactly as written
    for event in emptied {
        hooks.remove(&event);
    }
    for spec in specs {
        let list = hooks
            .entry(spec.0)
            .or_insert_with(|| serde_json::json!([]))
            .as_array_mut()
            .ok_or(DeckError::new(
                ErrorKind::Other,
                "a hook event entry is not a JSON array",
            ))?;
        list.push(spec_entry(style, helper, source, spec));
    }
    Ok(root)
}

/// Remove every deck-authored entry, pruning emptied arrays/objects.
pub(crate) fn hooks_with_uninstall(mut root: serde_json::Value) -> serde_json::Value {
    if let Some(hooks) = root.get_mut("hooks").and_then(|h| h.as_object_mut()) {
        for (_, list) in hooks.iter_mut() {
            if let Some(list) = list.as_array_mut() {
                list.retain(|entry| !entry_is_ours(entry));
            }
        }
        hooks.retain(|_, list| list.as_array().map(|l| !l.is_empty()).unwrap_or(true));
    }
    if root
        .get("hooks")
        .and_then(|h| h.as_object())
        .is_some_and(|h| h.is_empty())
    {
        if let Some(obj) = root.as_object_mut() {
            obj.remove("hooks");
        }
    }
    root
}

/// Installed = every deck hook event carries a marker entry; a partial
/// install reads as OFF so re-enabling repairs it.
pub(crate) fn hooks_installed(root: &serde_json::Value, specs: &[HookSpec]) -> bool {
    let Some(hooks) = root.get("hooks").and_then(|h| h.as_object()) else {
        return false;
    };
    specs.iter().all(|(event, _, _)| {
        hooks
            .get(*event)
            .and_then(|l| l.as_array())
            .is_some_and(|list| list.iter().any(entry_is_ours))
    })
}

fn claude_settings_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".claude").join("settings.json"))
}

/// Codex hooks live in a dedicated `$CODEX_HOME/hooks.json` (same document
/// shape as Claude's). Several hooks per event may coexist, so deck's
/// entries never conflict with the user's own hooks or `notify` program.
/// `CODEX_HOME` is honored when visible; a GUI launch usually doesn't see a
/// shell-exported value, which matches Codex's own default of `~/.codex`.
fn codex_hooks_path() -> Option<PathBuf> {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| dirs::home_dir().map(|home| home.join(".codex")))
        .map(|dir| dir.join("hooks.json"))
}

fn read_settings_value(path: &Path) -> Result<serde_json::Value, DeckError> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| {
            DeckError::new(
                ErrorKind::Other,
                "the agent settings file is not valid JSON — not modifying it",
            )
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(serde_json::json!({})),
        Err(e) => Err(DeckError::new(
            ErrorKind::io(e.kind()),
            format!("could not read agent settings ({})", e.kind()),
        )),
    }
}

/// Atomic replace that PRESERVES the file's existing permissions (these
/// config files belong to the agent CLI, not deck — 0600 is only the default
/// for a file deck itself creates). Refuses to create the agent's config
/// DIRECTORY: a missing one means the agent never ran on this Mac.
fn write_agent_config(path: &Path, bytes: &[u8], never_ran: &str) -> Result<(), DeckError> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o777)
        .unwrap_or(0o600);
    if let Some(dir) = path.parent() {
        if !dir.exists() {
            return Err(DeckError::new(ErrorKind::Missing, never_ran));
        }
    }
    crate::datadir::atomic_write(path, bytes)?;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    Ok(())
}

/// The helper a hook command may name: the sidecar inside `bundle`, which
/// must exist and must be quotable as one double-quoted shell word.
fn helper_path_in(bundle: &Path) -> Result<String, DeckError> {
    let helper = bundle.join("Contents").join("MacOS").join(HELPER_NAME);
    if !helper.is_file() {
        return Err(DeckError::new(
            ErrorKind::Other,
            "the bundled status helper is missing from this build",
        ));
    }
    let text = helper.to_str().ok_or(DeckError::new(
        ErrorKind::Other,
        "the application path is not valid UTF-8",
    ))?;
    if text
        .bytes()
        .any(|b| b < 0x20 || b == 0x7f || matches!(b, b'"' | b'$' | b'`' | b'\\' | b'!'))
    {
        return Err(DeckError::new(
            ErrorKind::Other,
            "the application path contains characters a hook command cannot quote",
        ));
    }
    Ok(text.to_string())
}

/// The helper of THIS install, only when deck runs from a release location
/// (`/Applications/deck.app` or `~/Applications/deck.app`). Development and
/// smoke bundles live at temporary paths and are ad-hoc signed; a hook that
/// named one would break when the path vanished and would make the agent
/// CLI execute an unsigned binary. Such builds cannot install hooks and
/// never touch the agent's config at boot.
fn installed_helper_path() -> Result<String, DeckError> {
    let exe = std::env::current_exe().map_err(DeckError::from)?;
    let bundle = crate::tmux_lifecycle::app_bundle_root(&exe)
        .filter(|bundle| crate::tmux_lifecycle::stable_installed_bundle(bundle))
        .ok_or(DeckError::new(ErrorKind::Other, "agent status hooks can only be installed from /Applications/deck.app or ~/Applications/deck.app"))?;
    helper_path_in(bundle)
}

fn write_hooks(path: &Path, next: &serde_json::Value, never_ran: &str) -> Result<(), DeckError> {
    let bytes = format!(
        "{}\n",
        serde_json::to_string_pretty(next).map_err(DeckError::from)?
    );
    write_agent_config(path, bytes.as_bytes(), never_ran)
}

/// Each agent module: config path, spec table, source word, hook style,
/// never-ran message.
type AgentModule = (
    Option<PathBuf>,
    &'static [HookSpec],
    &'static str,
    HookStyle,
    &'static str,
);

fn agent_modules() -> [AgentModule; 2] {
    [
        (
            claude_settings_path(),
            CLAUDE_HOOKS,
            "claude-code",
            HookStyle::Exec,
            "Claude Code has never run on this Mac — nothing to configure",
        ),
        (
            codex_hooks_path(),
            CODEX_HOOKS,
            "codex",
            HookStyle::ShellAsync,
            "Codex has never run on this Mac — nothing to configure",
        ),
    ]
}

/// Boot migration, the one write outside the Settings toggle: when hooks are
/// installed but are not what the CURRENT spec describes — a helper other
/// than this install's (the legacy `~/.deck/bin` copy, or a bundle that
/// moved), a matcher this deck version narrowed, an event it retired —
/// rewrite ONLY deck's entries, then delete the legacy copy once nothing
/// references it. Comparing the whole entry is what lets a spec change
/// (`permission_prompt|idle_prompt` → `permission_prompt`) reach users who
/// enabled the toggle under an older version without touching it again.
/// A non-release build returns before reading anything.
pub(crate) fn migrate_hooks_on_boot() {
    let Ok(helper) = installed_helper_path() else {
        return;
    };
    let mut legacy_referenced = false;
    for (path, specs, source, style, never_ran) in agent_modules() {
        let Some(path) = path else { continue };
        let Ok(value) = read_settings_value(&path) else {
            continue;
        };
        if !hooks_installed(&value, specs) {
            continue;
        }
        if hooks_are_current(&value, specs, source, style, &helper) {
            continue;
        }
        let result = hooks_with_install(value.clone(), specs, source, style, &helper)
            .and_then(|next| write_hooks(&path, &next, never_ran));
        match result {
            Ok(()) => applog(&format!(
                "[agent-hooks] {source} entries rewritten to the current spec"
            )),
            Err(e) => {
                legacy_referenced |= serde_json::to_string(&value)
                    .is_ok_and(|text| text.contains(LEGACY_HELPER_MARKER));
                applog(&format!(
                    "[agent-hooks] {source} migration FAILED ({})",
                    e.code()
                ));
            }
        }
    }
    if !legacy_referenced {
        retire_legacy_helper_copy();
    }
}

/// Remove the pre-0.5.12 `~/.deck/bin/deck-status-helper` copy (and the
/// directory if that left it empty). Nothing references it any more.
fn retire_legacy_helper_copy() {
    let Some(bin) = dirs::home_dir().map(|home| home.join(".deck").join("bin")) else {
        return;
    };
    let file = bin.join(HELPER_NAME);
    if !file.exists() {
        return;
    }
    match std::fs::remove_file(&file) {
        Ok(()) => {
            let _ = std::fs::remove_dir(&bin);
            applog("[agent-hooks] legacy helper copy removed");
        }
        Err(e) => applog(&format!(
            "[agent-hooks] legacy helper copy removal FAILED ({})",
            crate::error::err_code(&e.to_string())
        )),
    }
}

/// Shared enable/disable for one agent's hooks document.
fn hooks_set(
    path: &Path,
    specs: &[HookSpec],
    source: &str,
    style: HookStyle,
    enable: bool,
    never_ran: &str,
) -> Result<(), DeckError> {
    let value = read_settings_value(path)?;
    let next = if enable {
        let helper = installed_helper_path()?;
        hooks_with_install(value, specs, source, style, &helper)?
    } else {
        if !path.exists() {
            return Ok(());
        }
        hooks_with_uninstall(value)
    };
    write_hooks(path, &next, never_ran)
}

#[derive(serde::Serialize)]
pub(crate) struct AgentHooksStatus {
    claude: bool,
    codex: bool,
}

#[tauri::command]
pub(crate) fn agent_hooks_status() -> AgentHooksStatus {
    let installed = |path: Option<PathBuf>, specs: &[HookSpec]| {
        path.and_then(|p| read_settings_value(&p).ok())
            .map(|v| hooks_installed(&v, specs))
            .unwrap_or(false)
    };
    AgentHooksStatus {
        claude: installed(claude_settings_path(), CLAUDE_HOOKS),
        codex: installed(codex_hooks_path(), CODEX_HOOKS),
    }
}

/// The user-driven writer of the agent CLI config files, from an explicit
/// Settings toggle. The only other writer is `migrate_hooks_on_boot`, which
/// rewrites deck's own entries and nothing else.
#[tauri::command]
pub(crate) fn agent_hooks_set(agent: String, enable: bool) -> Result<(), DeckError> {
    let module = agent_modules()
        .into_iter()
        .find(|(_, _, source, _, _)| *source == agent)
        .ok_or(DeckError::new(ErrorKind::Other, "unknown agent"))?;
    let (path, specs, source, style, never_ran) = module;
    let result = hooks_set(
        &path.ok_or(DeckError::new(ErrorKind::Other, "no home directory"))?,
        specs,
        source,
        style,
        enable,
        never_ran,
    );
    match &result {
        Ok(()) => applog(&format!(
            "[agent-hooks] {agent} {}",
            if enable { "installed" } else { "removed" }
        )),
        Err(e) => applog(&format!(
            "[agent-hooks] {agent} {} FAILED ({})",
            if enable { "install" } else { "remove" },
            e.code()
        )),
    }
    result
}

// ---------- tests -------------------------------------------------------------

/// Tests that touch the process-wide agent store run one at a time, in this
/// module and in any other whose code reaches `reconcile`/`ingest`.
#[cfg(test)]
pub(crate) static STORE_TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    const HELPER: &str = "/Applications/deck.app/Contents/MacOS/deck-status-helper";

    fn event_line(state: &str, pane: &str) -> String {
        format!(
            "{{\"v\":1,\"source\":\"claude-code\",\"state\":\"{state}\",\"socket\":\"{}\",\"server_pid\":42,\"pane\":\"{pane}\"}}",
            crate::tmux::socket()
        )
    }

    #[test]
    fn events_are_validated_as_a_closed_shape() {
        assert!(parse_event(&event_line("working", "%3")).is_ok());
        assert_eq!(parse_event("not json"), Err("bad-json"));
        assert_eq!(parse_event("[1,2]"), Err("bad-json"));
        assert_eq!(
            parse_event(&event_line("working", "%3").replace("\"v\":1", "\"v\":3")),
            Err("bad-version")
        );
        assert_eq!(
            parse_event(&event_line("working", "%3").replace("claude-code", "mystery-agent")),
            Err("unknown-source")
        );
        assert_eq!(
            parse_event(&event_line("working", "%3").replace("working", "exfiltrate")),
            Err("unknown-state")
        );
        // an event from another deck server generation/socket is not ours
        assert_eq!(
            parse_event(&event_line("working", "%3").replace(crate::tmux::socket(), "deck-other")),
            Err("other-server")
        );
        assert_eq!(parse_event(&event_line("working", "%x")), Err("bad-pane"));
        assert_eq!(parse_event(&event_line("working", "3")), Err("bad-pane"));
    }

    // ---------- synthetic worlds: tmux server 42, one process table ------

    use crate::procinfo::ProcessInfo;

    fn process(pid: u32, ppid: u32, tty: u32, tty_pgid: u32, start: u64) -> ProcessInfo {
        ProcessInfo {
            pid,
            ppid,
            pgid: pid, // every process here leads its own group
            tty,
            tty_pgid,
            start_seconds: start,
            start_micros: 7,
        }
    }

    /// A pane: its shell `pane_pid` on `tty` (a child of the tmux server
    /// 42), the tty foreground group led by `leader` (born at `start`, a
    /// child of the shell) unless the leader is the shell itself, and a hook
    /// helper `helper` whose parent is `parent`.
    fn pane(pane_pid: u32, tty: u32, leader: u32, start: u64) -> Vec<ProcessInfo> {
        let mut out = vec![process(pane_pid, 42, tty, leader, 1000)];
        if leader != pane_pid {
            out.push(process(leader, pane_pid, tty, leader, start));
        }
        out
    }

    fn world(parts: &[Vec<ProcessInfo>]) -> ProcessTable {
        let mut table: ProcessTable = parts
            .iter()
            .flatten()
            .map(|info| (info.pid, *info))
            .collect();
        table.insert(42, process(42, 1, 0, 0, 1));
        table
    }

    fn with_helper(mut table: ProcessTable, helper: u32, parent: u32) -> ProcessTable {
        let tty = table.get(&parent).map_or(0, |p| p.tty);
        let fg = table.get(&parent).map_or(0, |p| p.tty_pgid);
        table.insert(helper, process(helper, parent, tty, fg, 5000));
        table
    }

    fn row(
        session: &str,
        session_id: &str,
        pane: &str,
        pane_pid: u32,
        active: bool,
        fg: &str,
    ) -> PaneRow {
        PaneRow {
            server_pid: 42,
            session_id: session_id.into(),
            session_name: session.into(),
            window_id: "@1".into(),
            pane_id: pane.into(),
            pane_pid,
            window_active: true,
            pane_active: active,
            command: fg.into(),
            ..PaneRow::default()
        }
    }

    fn report(
        state: &str,
        pane: &str,
        peer: Option<u32>,
        table: &ProcessTable,
        rows: &[PaneRow],
    ) -> Result<(), &'static str> {
        let origin = Origin {
            peer,
            table: table.clone(),
        };
        ingest(&event_line(state, pane), &origin, || Some(rows.to_vec()))
    }

    fn projection(rows: &[PaneRow], session: &str) -> Option<&'static str> {
        projections(rows).get(session).map(|o| o.state)
    }

    fn notified(session: &str) -> Option<&'static str> {
        crate::notify::snapshot_for_tests().0.get(session).copied()
    }

    #[test]
    fn the_signal_target_is_the_unique_active_pane_never_the_first() {
        let a = row("s", "$1", "%1", 100, false, "zsh");
        let b = row("s", "$1", "%2", 200, true, "claude");
        let listing = [a.clone(), b.clone()];
        let targets = signal_targets(&listing);
        assert_eq!(targets.get("s").map(|t| t.pane_id.as_str()), Some("%2"));
        // no marked pane → no target, NOT the first listed pane
        let unmarked = PaneRow {
            pane_active: false,
            ..b.clone()
        };
        assert!(signal_targets(&[a.clone(), unmarked]).is_empty());
        // an active pane of an inactive window is not the target
        let other_window = PaneRow {
            window_active: false,
            ..b.clone()
        };
        assert!(signal_targets(&[a.clone(), other_window]).is_empty());
        // two marked panes in one session → ambiguous → none
        let both = [
            PaneRow {
                pane_active: true,
                ..a.clone()
            },
            b.clone(),
        ];
        assert!(signal_targets(&both).is_empty());
        // sessions are independent
        let c = row("t", "$2", "%3", 300, true, "codex");
        let listing = [a, b, c];
        let targets = signal_targets(&listing);
        assert_eq!(targets.len(), 2);
        assert_eq!(targets.get("t").map(|t| t.pane_id.as_str()), Some("%3"));
    }

    #[test]
    fn admission_binds_the_reporter_to_the_pane_s_foreground_generation() {
        let _guard = STORE_TEST_LOCK.lock_or_recover();
        reset_for_tests();
        let rows = [row("deck-card-ab12", "$1", "%3", 300, true, "claude")];
        let table = with_helper(world(&[pane(300, 7, 310, 2000)]), 320, 310);
        // the helper's parent is the foreground leader (runtime-proven shape)
        assert_eq!(
            report("needs-input", "%3", Some(320), &table, &rows),
            Ok(())
        );
        assert_eq!(projection(&rows, "deck-card-ab12"), Some("needs-input"));
        // a wrapper leader (`caffeinate claude`): the leader is higher up
        reset_for_tests();
        let mut wrapped = world(&[pane(300, 7, 310, 2000)]);
        wrapped.insert(311, process(311, 310, 7, 310, 2001)); // agent under the wrapper
        let wrapped = with_helper(wrapped, 320, 311);
        assert_eq!(report("working", "%3", Some(320), &wrapped, &rows), Ok(()));
        assert_eq!(projection(&rows, "deck-card-ab12"), Some("working"));
        reset_for_tests();
    }

    #[test]
    fn admission_refusals_are_categorized_and_store_nothing() {
        let _guard = STORE_TEST_LOCK.lock_or_recover();
        reset_for_tests();
        let rows = [row("deck-card-ab12", "$1", "%3", 300, true, "claude")];
        let base = world(&[pane(300, 7, 310, 2000)]);
        let good = with_helper(base.clone(), 320, 310);
        // no kernel peer, or a peer the snapshot does not know
        assert_eq!(
            report("turn-done", "%3", None, &good, &rows),
            Err("no-peer")
        );
        assert_eq!(
            report("turn-done", "%3", Some(999), &good, &rows),
            Err("no-peer")
        );
        // a pane this server generation does not have
        assert_eq!(
            report("turn-done", "%9", Some(320), &good, &rows),
            Err("no-such-pane")
        );
        let restarted = [PaneRow {
            server_pid: 43,
            ..rows[0].clone()
        }];
        assert_eq!(
            report("turn-done", "%3", Some(320), &good, &restarted),
            Err("no-such-pane"),
            "a restarted server reused the pane id"
        );
        let bad_name = [row("bad name", "$1", "%3", 300, true, "claude")];
        assert_eq!(
            report("turn-done", "%3", Some(320), &good, &bad_name),
            Err("no-such-pane")
        );
        // a reporter outside the pane (another pane's process tree)
        let foreign = with_helper(
            world(&[pane(300, 7, 310, 2000), pane(400, 8, 410, 2000)]),
            420,
            410,
        );
        assert_eq!(
            report("turn-done", "%3", Some(420), &foreign, &rows),
            Err("foreign-pane")
        );
        // a shell owns the pane foreground
        let shell_rows = [row("deck-card-ab12", "$1", "%3", 300, true, "zsh")];
        assert_eq!(
            report("turn-done", "%3", Some(320), &good, &shell_rows),
            Err("shell-foreground")
        );
        // the foreground generation cannot be established
        let mut no_tty = good.clone();
        no_tty.get_mut(&300).unwrap().tty = 0;
        assert_eq!(
            report("turn-done", "%3", Some(320), &no_tty, &rows),
            Err("no-generation")
        );
        let mut no_leader = good.clone();
        no_leader.remove(&310);
        no_leader.insert(320, process(320, 300, 7, 310, 5000)); // helper reparented to the shell
        assert_eq!(
            report("turn-done", "%3", Some(320), &no_leader, &rows),
            Err("no-generation")
        );
        let mut unborn = good.clone();
        unborn.get_mut(&310).unwrap().start_seconds = 0;
        assert_eq!(
            report("turn-done", "%3", Some(320), &unborn, &rows),
            Err("no-generation")
        );
        assert_eq!(projection(&rows, "deck-card-ab12"), None);
        reset_for_tests();
    }

    /// Agent generation A reported; A exited and B (same executable name)
    /// took the foreground. A late event from A's tree is refused, and A's
    /// stored observation dies at the next reconciliation.
    #[test]
    fn an_old_foreground_generation_can_neither_report_nor_survive() {
        let _guard = STORE_TEST_LOCK.lock_or_recover();
        reset_for_tests();
        let rows = [row("deck-card-ab12", "$1", "%3", 300, true, "claude")];
        let gen_a = with_helper(world(&[pane(300, 7, 310, 2000)]), 320, 310);
        assert_eq!(report("working", "%3", Some(320), &gen_a, &rows), Ok(()));
        // B leads the foreground now; A lingers outside it (still a child
        // of the shell) and its late turn-done arrives
        let mut gen_b = world(&[pane(300, 7, 311, 3000)]);
        gen_b.insert(310, process(310, 300, 7, 311, 2000));
        let late = with_helper(gen_b.clone(), 330, 310);
        assert_eq!(
            report("turn-done", "%3", Some(330), &late, &rows),
            Err("generation-mismatch")
        );
        assert_eq!(
            projection(&rows, "deck-card-ab12"),
            Some("working"),
            "B's view untouched"
        );
        // same executable name, new foreground pid → A's observation ends
        reconcile(&rows, &gen_b);
        assert_eq!(projection(&rows, "deck-card-ab12"), None);
        // pid reuse: the same pid with another birth instant is another process
        assert_eq!(report("working", "%3", Some(320), &gen_a, &rows), Ok(()));
        let mut reused = gen_a.clone();
        reused.get_mut(&310).unwrap().start_seconds = 2999;
        reconcile(&rows, &reused);
        assert_eq!(projection(&rows, "deck-card-ab12"), None);
        // the unchanged generation survives any number of polls
        assert_eq!(report("working", "%3", Some(320), &gen_a, &rows), Ok(()));
        for _ in 0..3 {
            reconcile(&rows, &gen_a);
        }
        assert_eq!(projection(&rows, "deck-card-ab12"), Some("working"));
        // a foreground that fell back to the shell ends it too
        reconcile(&rows, &world(&[pane(300, 7, 300, 1000)]));
        assert_eq!(projection(&rows, "deck-card-ab12"), None);
        reset_for_tests();
    }

    #[test]
    fn server_session_or_pane_replacement_invalidates_the_observation() {
        let _guard = STORE_TEST_LOCK.lock_or_recover();
        reset_for_tests();
        let rows = [row("deck-card-ab12", "$1", "%3", 300, true, "claude")];
        let table = with_helper(world(&[pane(300, 7, 310, 2000)]), 320, 310);
        let replaced = |rows: &[PaneRow]| {
            reset_for_tests();
            assert_eq!(
                report(
                    "working",
                    "%3",
                    Some(320),
                    &table,
                    &[row("deck-card-ab12", "$1", "%3", 300, true, "claude")]
                ),
                Ok(())
            );
            reconcile(rows, &table);
            projection(rows, "deck-card-ab12")
        };
        // tmux server restarted and reused the pane id
        assert_eq!(
            replaced(&[PaneRow {
                server_pid: 43,
                ..rows[0].clone()
            }]),
            None
        );
        // the session was replaced under the same name (new session id)
        assert_eq!(
            replaced(&[PaneRow {
                session_id: "$9".into(),
                ..rows[0].clone()
            }]),
            None
        );
        // the pane's process was replaced
        assert_eq!(
            replaced(&[PaneRow {
                pane_pid: 301,
                ..rows[0].clone()
            }]),
            None
        );
        // the pane is gone
        assert_eq!(replaced(&[]), None);
        // nothing can be re-established without a process table
        assert_eq!(replaced(&rows), Some("working"));
        reconcile(&rows, &ProcessTable::new());
        assert_eq!(projection(&rows, "deck-card-ab12"), None);
        reset_for_tests();
    }

    /// The Stage A defect: pane A working, pane B (same session) reports
    /// turn-done. B's event is stored under B and reaches no card-level
    /// surface while A is the session's Signal target — not the Board
    /// projection, not the notification state, not the unread set, not the
    /// Dock count.
    #[test]
    fn an_inactive_pane_cannot_contaminate_the_session_signal() {
        let _guard = STORE_TEST_LOCK.lock_or_recover();
        reset_for_tests();
        let session = "deck-card-split";
        let table = with_helper(
            with_helper(
                world(&[pane(300, 7, 310, 2000), pane(400, 8, 410, 2000)]),
                320,
                310,
            ),
            420,
            410,
        );
        let a_active = [
            row(session, "$1", "%3", 300, true, "claude"),
            row(session, "$1", "%4", 400, false, "claude"),
        ];
        crate::notify::retain(&std::collections::HashSet::new());
        let (_, _, dock_before) = crate::notify::snapshot_for_tests();
        assert_eq!(
            report("working", "%3", Some(320), &table, &a_active),
            Ok(())
        );
        assert_eq!(notified(session), Some("working"));
        assert_eq!(
            report("turn-done", "%4", Some(420), &table, &a_active),
            Ok(())
        );
        assert_eq!(projection(&a_active, session), Some("working"), "A stays A");
        let (states, unread, dock) = crate::notify::snapshot_for_tests();
        assert_eq!(
            states.get(session),
            Some(&"working"),
            "no notification from B"
        );
        assert!(!unread.contains(session), "no unread ending from B");
        assert_eq!(dock, dock_before, "the Dock count is untouched");
        reconcile(&a_active, &table);
        assert_eq!(projection(&a_active, session), Some("working"));
        assert_eq!(notified(session), Some("working"));

        // B becomes the active pane: the card now projects B's valid
        // observation (a turn ended; still no side-effect authority — SI-01)
        let b_active = [
            row(session, "$1", "%3", 300, false, "claude"),
            row(session, "$1", "%4", 400, true, "claude"),
        ];
        reconcile(&b_active, &table);
        assert_eq!(projection(&b_active, session), Some("turn-done"));
        assert_eq!(notified(session), Some("turn-done"));
        assert!(crate::notify::snapshot_for_tests().1.contains(session));
        // a target pane without an observation projects nothing, and the
        // session leaves the notification layer
        let c_active = [
            row(session, "$1", "%3", 300, false, "claude"),
            row(session, "$1", "%4", 400, false, "claude"),
            row(session, "$1", "%5", 500, true, "zsh"),
        ];
        reconcile(&c_active, &table);
        assert_eq!(projection(&c_active, session), None);
        assert_eq!(notified(session), None);
        // back to A: A's generation is still valid
        reconcile(&a_active, &table);
        assert_eq!(projection(&a_active, session), Some("working"));
        reset_for_tests();
        crate::notify::retain(&std::collections::HashSet::new());
    }

    /// An agent only in a non-first pane: its observation is validated
    /// against ITS pane, never cleared or kept because of the first one.
    #[test]
    fn an_agent_in_a_non_first_pane_is_judged_by_its_own_pane() {
        let _guard = STORE_TEST_LOCK.lock_or_recover();
        reset_for_tests();
        let session = "deck-card-second";
        let table = with_helper(
            world(&[pane(300, 7, 300, 1000), pane(400, 8, 410, 2000)]),
            420,
            410,
        );
        let rows = [
            row(session, "$1", "%3", 300, false, "zsh"),
            row(session, "$1", "%4", 400, true, "claude"),
        ];
        assert_eq!(
            report("needs-input", "%4", Some(420), &table, &rows),
            Ok(())
        );
        assert_eq!(notified(session), Some("needs-input"));
        reconcile(&rows, &table);
        assert_eq!(projection(&rows, session), Some("needs-input"));
        // the markers vanish (ambiguous listing): no Signal, never pane %3's
        let unmarked = [
            row(session, "$1", "%3", 300, false, "zsh"),
            row(session, "$1", "%4", 400, false, "claude"),
        ];
        reconcile(&unmarked, &table);
        assert_eq!(projection(&unmarked, session), None);
        assert_eq!(notified(session), None);
        reset_for_tests();
        crate::notify::retain(&std::collections::HashSet::new());
    }

    #[test]
    fn the_scheduler_hold_reads_the_same_projection() {
        let _guard = STORE_TEST_LOCK.lock_or_recover();
        reset_for_tests();
        let session = "deck-card-hold";
        let table = with_helper(
            with_helper(
                world(&[pane(300, 7, 310, 2000), pane(400, 8, 410, 2000)]),
                320,
                310,
            ),
            420,
            410,
        );
        let mut rows = vec![
            row(session, "$1", "%3", 300, false, "claude"),
            row(session, "$1", "%4", 400, true, "claude"),
        ];
        rows[0].window_activity = 11;
        assert_eq!(
            report("needs-input", "%3", Some(320), &table, &rows),
            Ok(())
        );
        let seen = crate::scheduler::observe(rows.clone());
        assert_eq!(
            seen[session].agent, None,
            "the inactive pane's input request is not the target's"
        );
        assert_eq!(
            seen[session].activity, 11,
            "activity keeps its first-pane semantics"
        );
        assert_eq!(report("working", "%4", Some(420), &table, &rows), Ok(()));
        assert_eq!(
            crate::scheduler::observe(rows.clone())[session].agent,
            Some("working")
        );
        rows[0].pane_active = true;
        rows[1].pane_active = false;
        assert_eq!(
            crate::scheduler::observe(rows.clone())[session].agent,
            Some("needs-input")
        );
        rows[0].pane_active = false;
        assert_eq!(
            crate::scheduler::observe(rows)[session].agent,
            None,
            "no target, no fallback"
        );
        reset_for_tests();
        crate::notify::retain(&std::collections::HashSet::new());
    }

    // ---------- FR-SI-04: protocol v2 and the interaction tracker ----------

    const A: &str = "0199aaaa-bbbb-7ccc-8ddd-00000000000a";
    const B: &str = "0199aaaa-bbbb-7ccc-8ddd-00000000000b";

    fn event_v2(state: &str, pane: &str, id: &str) -> String {
        event_line(state, pane)
            .replace("\"v\":1", "\"v\":2")
            .replace("}", &format!(",\"interaction\":\"{id}\"}}"))
    }

    #[test]
    fn v2_carries_exactly_one_validated_interaction_and_v1_none() {
        let parsed = parse_event(&event_v2("working", "%3", A)).unwrap();
        assert_eq!(parsed.interaction.as_deref(), Some(A));
        assert_eq!(
            parse_event(&event_line("working", "%3"))
                .unwrap()
                .interaction,
            None
        );
        // v2 without, or with an invalid, id; v1 with one
        let no_id = event_line("working", "%3").replace("\"v\":1", "\"v\":2");
        assert_eq!(parse_event(&no_id), Err("bad-interaction"));
        for bad in ["0199AAAA-BBBB-7CCC-8DDD-00000000000A", "not-a-uuid", ""] {
            assert_eq!(
                parse_event(&event_v2("working", "%3", bad)),
                Err("bad-interaction"),
                "{bad}"
            );
        }
        let numeric = event_line("working", "%3")
            .replace("\"v\":1", "\"v\":2")
            .replace("}", ",\"interaction\":7}");
        assert_eq!(parse_event(&numeric), Err("bad-interaction"));
        let v1_with_id = event_v2("working", "%3", A).replace("\"v\":2", "\"v\":1");
        assert_eq!(parse_event(&v1_with_id), Err("bad-interaction"));
    }

    /// Mixed versions: a Deck built before FR-SI-04 gates on `v == 1` (the
    /// exact pre-v2 check, quoted below), so a v2 line is refused there as
    /// `bad-version` — it fails closed (no Signal) instead of being
    /// half-read. The helper and backend ship in one bundle and the updater
    /// relaunches after replacing it, so an old backend meets a new helper
    /// only transiently — in the short bundle-replacement → relaunch window
    /// of an in-place update, or from a second, older Deck (a dev build).
    /// Either way the Signal is briefly absent, never wrong.
    #[test]
    fn a_v1_only_parser_refuses_a_v2_line() {
        let v1_only = |line: &str| -> Result<(), &'static str> {
            let value: serde_json::Value = serde_json::from_str(line).map_err(|_| "bad-json")?;
            let obj = value.as_object().ok_or("bad-json")?;
            // pre-FR-SI-04 agent_status::parse_event, verbatim:
            if obj.get("v").and_then(|v| v.as_u64()) != Some(1) {
                return Err("bad-version");
            }
            Ok(())
        };
        assert_eq!(v1_only(&event_v2("turn-done", "%3", A)), Err("bad-version"));
        assert_eq!(v1_only(&event_line("turn-done", "%3")), Ok(()));
    }

    #[test]
    fn the_interaction_tracker_separates_but_never_orders() {
        let mut t = Interactions::default();
        // working(A), working(B), late Stop(A) → B stays current
        assert_eq!(t.admit(WORKING, Some(A)), Ok(()));
        assert_eq!(t.admit(WORKING, Some(B)), Ok(()));
        assert_eq!(t.admit(TURN_DONE, Some(A)), Err("interaction-mismatch"));
        assert_eq!(t.current.as_deref(), Some(B));
        // … and A is remembered as ended: its late start is stale
        assert_eq!(t.admit(WORKING, Some(A)), Err("stale-interaction"));
        assert_eq!(t.admit(NEEDS_INPUT, Some(A)), Err("stale-interaction"));
        // the current interaction's input request and ending are accepted
        assert_eq!(t.admit(NEEDS_INPUT, Some(B)), Ok(()));
        assert_eq!(t.admit(TURN_DONE, Some(B)), Ok(()));
        assert_eq!(t.current, None);
        // Stop(B) again: a duplicate, not a new boundary
        assert_eq!(t.admit(TURN_DONE, Some(B)), Err("duplicate-interaction"));
        // Stop(B), late needs-input(B): stale
        assert_eq!(t.admit(NEEDS_INPUT, Some(B)), Err("stale-interaction"));

        // current = None: needs-input bootstraps, turn-done is an unpaired boundary
        let mut t = Interactions::default();
        assert_eq!(t.admit(NEEDS_INPUT, Some(A)), Ok(()));
        assert_eq!(t.current.as_deref(), Some(A));
        let mut t = Interactions::default();
        assert_eq!(t.admit(TURN_DONE, Some(A)), Ok(()));
        assert_eq!(t.current, None);
        assert!(t.has_ended(A));

        // no downgrade: once an id was seen, a v1 word changes nothing
        let mut t = Interactions::default();
        assert_eq!(t.admit(WORKING, None), Ok(()), "legacy v1 is accepted");
        assert_eq!(t.admit(WORKING, Some(A)), Ok(()));
        for state in [WORKING, NEEDS_INPUT, TURN_DONE] {
            assert_eq!(t.admit(state, None), Err("identity-downgrade"), "{state}");
        }
        assert_eq!(t.current.as_deref(), Some(A));

        // documented UNSOLVED class: working(B), then a late working(A)
        // before Stop(A) marked A ended — no source ordering exists, so A
        // becomes current (and B's own Stop is then refused as a mismatch)
        let mut t = Interactions::default();
        assert_eq!(t.admit(WORKING, Some(B)), Ok(()));
        assert_eq!(t.admit(WORKING, Some(A)), Ok(()));
        assert_eq!(
            t.current.as_deref(),
            Some(A),
            "not guessed from UUID time or arrival"
        );

        // the ended memory is bounded
        let mut t = Interactions::default();
        for i in 0..20 {
            t.end(&format!("0199aaaa-bbbb-7ccc-8ddd-{i:012}"));
        }
        assert_eq!(t.ended.len(), ENDED_CAP);
    }

    /// The side-effect-relevant regression: B asks for input, then a late
    /// Stop of the previous interaction A arrives. B stays `needs-input` —
    /// the Board, the scheduler hold (an owner row is NOT released), the
    /// notification state, the unread set and the Dock count all keep B.
    #[test]
    fn a_late_ending_cannot_release_the_current_input_request() {
        let _guard = STORE_TEST_LOCK.lock_or_recover();
        reset_for_tests();
        crate::notify::retain(&std::collections::HashSet::new());
        let session = "deck-card-v2";
        let rows = [row(session, "$1", "%3", 300, true, "codex")];
        let table = with_helper(world(&[pane(300, 7, 310, 2000)]), 320, 310);
        let send = |line: String| {
            ingest(
                &line,
                &Origin {
                    peer: Some(320),
                    table: table.clone(),
                },
                || Some(rows.to_vec()),
            )
        };
        assert_eq!(send(event_v2("working", "%3", A)), Ok(()));
        assert_eq!(send(event_v2("working", "%3", B)), Ok(()));
        assert_eq!(send(event_v2("needs-input", "%3", B)), Ok(()));
        let (_, _, dock_before) = crate::notify::snapshot_for_tests();
        assert_eq!(
            send(event_v2("turn-done", "%3", A)),
            Err("interaction-mismatch")
        );
        assert_eq!(projection(&rows, session), Some("needs-input"));
        let (states, unread, dock) = crate::notify::snapshot_for_tests();
        assert_eq!(states.get(session), Some(&"needs-input"));
        assert!(!unread.contains(session), "no false turn-ended unread");
        assert_eq!(dock, dock_before);
        // the scheduler still holds an automatic owner row
        let seen = crate::scheduler::observe(rows.to_vec());
        let owner: crate::scheduler::QueueItem = serde_json::from_value(serde_json::json!({
            "id": "o", "session": session, "card_id": "c", "dir": "", "cmd": "",
            "text": "x", "mode": "chain", "added": 0
        }))
        .unwrap();
        assert!(crate::scheduler::agent_holds(&owner, seen.get(session)));

        // Stop(B) ends it; a late needs-input(B) is stale: no re-arm
        assert_eq!(send(event_v2("turn-done", "%3", B)), Ok(()));
        let (_, unread_after_end, dock_after_end) = crate::notify::snapshot_for_tests();
        assert!(unread_after_end.contains(session));
        assert_eq!(
            send(event_v2("needs-input", "%3", B)),
            Err("stale-interaction")
        );
        assert_eq!(projection(&rows, session), Some("turn-done"));
        let (states, _, dock) = crate::notify::snapshot_for_tests();
        assert_eq!(
            states.get(session),
            Some(&"turn-done"),
            "no input request re-armed"
        );
        assert_eq!(dock, dock_after_end);
        // a late working(A) of an ended interaction is refused too
        assert_eq!(send(event_v2("working", "%3", A)), Err("stale-interaction"));
        assert_eq!(projection(&rows, session), Some("turn-done"));

        // same generation, a v1 word after v2: refused, state untouched
        assert_eq!(
            send(event_v2(
                "working",
                "%3",
                "0199aaaa-bbbb-7ccc-8ddd-00000000000c"
            )),
            Ok(())
        );
        assert_eq!(
            send(event_line("turn-done", "%3")),
            Err("identity-downgrade")
        );
        assert_eq!(projection(&rows, session), Some("working"));

        // a NEW foreground generation starts a fresh (legacy) tracker
        let next = with_helper(world(&[pane(300, 7, 311, 3000)]), 321, 311);
        let origin = Origin {
            peer: Some(321),
            table: next.clone(),
        };
        assert_eq!(
            ingest(&event_line("turn-done", "%3"), &origin, || Some(
                rows.to_vec()
            )),
            Ok(())
        );
        reconcile(&rows, &next);
        assert_eq!(projection(&rows, session), Some("turn-done"));
        reset_for_tests();
        crate::notify::retain(&std::collections::HashSet::new());
    }

    /// Protocol v2 changes nothing in the installed hook commands: the
    /// helper reads the payload it already received on stdin, so there is
    /// no hook migration. The exact pre-v2 command shapes, pinned.
    #[test]
    fn hook_commands_are_unchanged_by_protocol_v2() {
        assert_eq!(
            hook_value(HookStyle::Exec, HELPER, "claude-code", "turn-done"),
            serde_json::json!({"type": "command", "command": HELPER, "args": ["claude-code", "turn-done"], "timeout": 10})
        );
        assert_eq!(
            hook_value(HookStyle::ShellAsync, HELPER, "codex", "turn-done"),
            serde_json::json!({"type": "command", "command": format!("\"{HELPER}\" codex turn-done"), "timeout": 10, "async": true})
        );
    }

    /// FR-SI-05 diagnostics: the first 20 refusals one by one, every one
    /// counted by its closed reason, one summary per ten minutes, reset.
    #[test]
    fn drop_diagnostics_are_closed_bounded_and_summarized() {
        let t0 = std::time::Instant::now();
        let mut drops = Drops::new(t0);
        assert_eq!(
            drops.observe(Some("generation-mismatch"), t0),
            ["[agent-status] dropped (generation-mismatch)"]
        );
        assert_eq!(
            drops.observe(Some("not-a-code"), t0),
            ["[agent-status] dropped (other)"],
            "closed vocabulary"
        );
        for _ in 0..18 {
            drops.observe(Some("stale-interaction"), t0);
        }
        assert_eq!(
            drops.observe(Some("stale-interaction"), t0),
            ["[agent-status] further drops summarized every 10 min"]
        );
        assert!(
            drops.observe(Some("interaction-mismatch"), t0).is_empty(),
            "no line per drop past 20"
        );
        assert!(
            drops
                .observe(None, t0 + Duration::from_secs(599))
                .is_empty(),
            "not before ten minutes"
        );
        assert_eq!(
            drops.observe(None, t0 + DROP_SUMMARY_EVERY),
            ["[agent-status] drops generation-mismatch=1 interaction-mismatch=1 other=1 stale-interaction=19"]
        );
        assert!(
            drops.observe(None, t0 + DROP_SUMMARY_EVERY * 3).is_empty(),
            "counts were reset"
        );
        for reason in DROP_REASONS {
            assert!(
                reason.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'),
                "{reason}"
            );
        }
    }

    #[test]
    fn identity_absence_is_reported_once_per_source_on_a_fresh_generation() {
        let source = "drift-test-source";
        assert!(
            !identity_absent(source, false, true),
            "never seen with identity: legacy, silent"
        );
        assert!(!identity_absent(source, true, false));
        assert!(
            !identity_absent(source, false, false),
            "same generation: the tracker refuses it instead"
        );
        assert!(
            identity_absent(source, false, true),
            "a fresh generation without identity"
        );
        assert!(!identity_absent(source, false, true), "once");
    }

    #[test]
    fn codex_events_are_a_registered_source() {
        assert!(
            parse_event(&event_line("turn-done", "%3").replace("claude-code", "codex")).is_ok()
        );
    }

    #[test]
    fn hook_status_command_returns_both_closed_agent_fields() {
        let status = serde_json::to_value(agent_hooks_status()).unwrap();
        let object = status.as_object().unwrap();
        assert_eq!(object.len(), 2);
        assert!(object
            .get("claude")
            .is_some_and(serde_json::Value::is_boolean));
        assert!(object
            .get("codex")
            .is_some_and(serde_json::Value::is_boolean));
    }

    #[test]
    fn codex_hooks_install_covers_all_events_async_and_coexists() {
        // a user hooks.json with its own Stop hook and a description key
        let user = serde_json::json!({
            "description": "my hooks",
            "hooks": {
                "Stop": [
                    { "hooks": [{ "type": "command", "command": "terminal-notifier -message done" }] }
                ]
            }
        });
        let installed = hooks_with_install(
            user.clone(),
            CODEX_HOOKS,
            "codex",
            HookStyle::ShellAsync,
            HELPER,
        )
        .unwrap();
        assert!(hooks_installed(&installed, CODEX_HOOKS));
        assert_eq!(installed["description"], "my hooks");
        // the user's own Stop hook coexists with ours — no notify-style conflict
        assert_eq!(installed["hooks"]["Stop"].as_array().unwrap().len(), 2);
        let permission = &installed["hooks"]["PermissionRequest"][0]["hooks"][0];
        assert_eq!(
            permission["command"].as_str().unwrap(),
            "\"/Applications/deck.app/Contents/MacOS/deck-status-helper\" codex needs-input"
        );
        // fire-and-forget: every deck entry is async so it can never block a turn
        assert_eq!(permission["async"], serde_json::json!(true));
        assert_eq!(
            installed["hooks"]["Interrupt"][0]["hooks"][0]["command"]
                .as_str()
                .unwrap(),
            "\"/Applications/deck.app/Contents/MacOS/deck-status-helper\" codex turn-done"
        );

        // idempotent install, exact uninstall
        let twice = hooks_with_install(
            installed,
            CODEX_HOOKS,
            "codex",
            HookStyle::ShellAsync,
            HELPER,
        )
        .unwrap();
        assert_eq!(twice["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(hooks_with_uninstall(twice), user);

        // claude's spec set does not read as installed for codex and vice versa
        let claude_only = hooks_with_install(
            serde_json::json!({}),
            CLAUDE_HOOKS,
            "claude-code",
            HookStyle::Exec,
            HELPER,
        )
        .unwrap();
        assert!(!hooks_installed(&claude_only, CODEX_HOOKS));
    }

    /// The real socket path through the kernel: the peer's pid and one
    /// process-table snapshot decide. This test process is "the pane" only
    /// when the listing names it (or an ancestor) as the pane's process and
    /// its tty's foreground leader is in its chain.
    #[test]
    fn the_listener_binds_a_real_peer_to_its_pane() {
        use std::io::Write;
        let _guard = STORE_TEST_LOCK.lock_or_recover();
        reset_for_tests();
        let dir = std::env::temp_dir().join(format!("deck-status-peer-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("status.sock");
        let listener = listen_at(&path).unwrap();
        let line = event_line("needs-input", "%3");
        let mut client = UnixStream::connect(&path).unwrap();
        writeln!(client, "{line}").unwrap();
        let (mut stream, _) = listener.accept().unwrap();
        let read = read_first_line(&mut stream).unwrap();
        assert_eq!(read, line);
        let origin = Origin {
            peer: crate::procinfo::peer_pid(&stream),
            table: crate::procinfo::processes(),
        };
        drop(client);
        let me = std::process::id();
        assert_eq!(origin.peer, Some(me), "the kernel names this process");
        let chain = crate::procinfo::ancestry_in(&origin.table, me, ORIGIN_HOPS);
        assert!(chain.len() >= 2, "and its parent: {chain:?}");
        let pane_is = |pid: u32| vec![row("deck-card-ab12", "$1", "%3", pid, true, "claude")];
        // a pane process outside this chain → foreign
        assert_eq!(
            ingest(&read, &origin, || Some(pane_is(u32::MAX - 1))),
            Err("foreign-pane")
        );
        // this process as the pane: accepted exactly when its tty's
        // foreground leader is in the chain (a test run may have no tty)
        let generation = foreground_generation(&origin.table, me);
        let expected = match generation {
            None => Err("no-generation"),
            Some(g) if chain.contains(&g.pid) => Ok(()),
            Some(_) => Err("generation-mismatch"),
        };
        assert_eq!(ingest(&read, &origin, || Some(pane_is(me))), expected);
        drop(listener);
        let _ = std::fs::remove_dir_all(&dir);
        reset_for_tests();
    }

    /// End to end against a throwaway tmux server: a client started INSIDE
    /// the pane (a descendant of `#{pane_pid}`) is accepted, the same line
    /// from this test process (outside the pane) is refused.
    #[test]
    fn a_pane_s_own_descendant_reports_and_an_outsider_is_refused() {
        use std::process::Command;
        let _guard = STORE_TEST_LOCK.lock_or_recover();
        reset_for_tests();
        /// (socket name, tmux binary, socket path captured while alive)
        struct Server(String, PathBuf, Option<PathBuf>);
        impl Server {
            fn run(&self, args: &[&str]) -> String {
                let out = Command::new(&self.1)
                    .args(["-f", "/dev/null", "-L", &self.0])
                    .args(args)
                    .output()
                    .expect("tmux spawn");
                String::from_utf8_lossy(&out.stdout).into_owned()
            }
        }
        impl Drop for Server {
            fn drop(&mut self) {
                // like the contract suite's guard: kill the server and
                // remove exactly its socket file, which tmux leaves behind.
                // The path was captured while the server was alive: once
                // nc exits the empty server exits by itself and can no
                // longer be asked.
                let _ = self.run(&["kill-server"]);
                if let Some(path) = self.2.take() {
                    if path.file_name().and_then(|n| n.to_str()) == Some(&self.0)
                        && std::fs::symlink_metadata(&path).is_ok_and(|m| {
                            std::os::unix::fs::FileTypeExt::is_socket(&m.file_type())
                        })
                    {
                        let _ = std::fs::remove_file(path);
                    }
                }
            }
        }
        let bin =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("binaries/tmux-aarch64-apple-darwin");
        // one per run and per call: the sequence keeps a second use of this
        // fixture in the same process off the first one's socket and server
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("deck-status-e2e-{}-{seq}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("status.sock");
        let listener = listen_at(&path).unwrap();
        let mut server = Server(
            format!("deck-test-status-{}-{seq}", std::process::id()),
            bin,
            None,
        );
        // The pane program IS the client: `exec` makes nc the pane's own
        // process (same pid tmux recorded as #{pane_pid}) and its
        // foreground, so a shell never owns the pane while it reports —
        // exactly the shape of an agent hook. The event line is typed into
        // nc's stdin (the pane tty) once the pane's identity is known.
        let client = format!("exec /usr/bin/nc -U '{}'", path.display());
        server.run(&[
            "start-server",
            ";",
            "new-session",
            "-d",
            "-s",
            "t",
            "-x",
            "80",
            "-y",
            "12",
            &client,
        ]);
        let socket = server.run(&["display-message", "-p", "#{socket_path}"]);
        server.2 = Some(PathBuf::from(socket.trim()));
        let mut rows: Vec<PaneRow> = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            rows = server
                .run(&["list-panes", "-a", "-F", crate::tmux::PANE_FORMAT])
                .lines()
                .filter_map(crate::tmux::parse_pane_row)
                .collect();
            if rows.len() == 1 && rows[0].command == "nc" {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(rows.len(), 1, "one pane whose foreground is nc: {rows:?}");
        assert_eq!(rows[0].command, "nc");
        let line = format!(
            "{{\"v\":1,\"source\":\"claude-code\",\"state\":\"working\",\"socket\":\"{}\",\"server_pid\":{},\"pane\":\"{}\"}}",
            crate::tmux::socket(),
            rows[0].server_pid,
            rows[0].pane_id
        );
        server.run(&["send-keys", "-t", "t", "-l", &line]);
        server.run(&["send-keys", "-t", "t", "Enter"]);
        listener.set_nonblocking(true).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(_) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                Err(e) => panic!("no connection from the pane: {e}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        let read = read_first_line(&mut stream).unwrap();
        assert_eq!(read, line);
        let origin = Origin {
            peer: crate::procinfo::peer_pid(&stream),
            table: crate::procinfo::processes(),
        };
        drop(stream);
        assert_eq!(
            origin.peer,
            Some(rows[0].pane_pid),
            "nc is the pane process itself"
        );
        assert_eq!(ingest(&line, &origin, || Some(rows.clone())), Ok(()));
        assert_eq!(
            projections(&rows).get("t").map(|o| o.state),
            Some("working")
        );
        // the generation recorded is the one the admission snapshot saw
        reconcile(&rows, &origin.table);
        assert_eq!(
            projections(&rows).get("t").map(|o| o.state),
            Some("working")
        );
        reset_for_tests();
        // the same bytes from outside the pane
        let outsider = Origin {
            peer: Some(std::process::id()),
            table: crate::procinfo::processes(),
        };
        assert_eq!(
            ingest(&line, &outsider, || Some(rows.clone())),
            Err("foreign-pane")
        );
        assert_eq!(projections(&rows).get("t").map(|o| o.state), None);
        drop(server);
        drop(listener);
        let _ = std::fs::remove_dir_all(&dir);
        reset_for_tests();
    }

    #[test]
    fn listener_accepts_a_real_socket_line() {
        use std::io::Write;
        let _guard = STORE_TEST_LOCK.lock_or_recover();
        reset_for_tests();
        let dir = std::env::temp_dir().join(format!("deck-status-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("status.sock");
        let listener = listen_at(&path).unwrap();
        let line = event_line("turn-done", "%3");
        let mut client = UnixStream::connect(&path).unwrap();
        writeln!(client, "{line}").unwrap();
        // like the helper, the client stays connected until deck has read
        // its identity: the kernel reports no peer for a closed one
        client.shutdown(std::net::Shutdown::Write).unwrap();
        let (stream, _) = listener.accept().unwrap();
        // the real read path; a fake resolver stands in for the tmux server
        let mut stream = stream;
        let read = read_first_line(&mut stream).unwrap();
        assert_eq!(read, line);
        let peer = crate::procinfo::peer_pid(&stream);
        drop(client);
        assert_eq!(
            peer,
            Some(std::process::id()),
            "the kernel names the writer"
        );
        // rebinding over a stale socket file must work (previous run crashed)
        drop(listener);
        let listener2 = listen_at(&path).unwrap();
        drop(listener2);
        let _ = std::fs::remove_dir_all(&dir);
        reset_for_tests();
    }

    #[test]
    fn hook_install_is_idempotent_and_preserves_foreign_config() {
        let user = serde_json::json!({
            "model": "opus",
            "hooks": {
                "Stop": [
                    { "hooks": [{ "type": "command", "command": "afplay /System/Library/Sounds/Glass.aiff" }] }
                ],
                "PreToolUse": [
                    { "matcher": "Bash", "hooks": [{ "type": "command", "command": "my-guard" }] }
                ]
            }
        });
        let installed = hooks_with_install(
            user.clone(),
            CLAUDE_HOOKS,
            "claude-code",
            HookStyle::Exec,
            HELPER,
        )
        .unwrap();
        assert!(hooks_installed(&installed, CLAUDE_HOOKS));
        assert_eq!(installed["model"], "opus");
        // the user's own Stop hook and PreToolUse guard survive
        assert_eq!(installed["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(
            installed["hooks"]["PreToolUse"].as_array().unwrap().len(),
            1
        );
        // exec form: the helper path alone in `command`, words in `args`,
        // so Claude Code spawns the helper without a shell
        let submit = &installed["hooks"]["UserPromptSubmit"][0]["hooks"][0];
        assert_eq!(submit["command"], HELPER);
        assert_eq!(
            submit["args"],
            serde_json::json!(["claude-code", "working"])
        );
        assert_eq!(submit["type"], "command");
        assert!(submit.get("async").is_none());
        // only a mid-turn permission question is "needs-input"; an idle
        // prompt after a finished turn stays turn-done
        assert_eq!(
            installed["hooks"]["Notification"][0]["matcher"],
            "permission_prompt"
        );

        // installing twice does not duplicate
        let twice = hooks_with_install(
            installed.clone(),
            CLAUDE_HOOKS,
            "claude-code",
            HookStyle::Exec,
            HELPER,
        )
        .unwrap();
        assert_eq!(twice["hooks"]["Stop"].as_array().unwrap().len(), 2);

        // uninstall restores the user's document exactly
        let removed = hooks_with_uninstall(twice);
        assert!(!hooks_installed(&removed, CLAUDE_HOOKS));
        assert_eq!(removed, user);

        // uninstalling a never-installed file is a no-op
        let empty = hooks_with_uninstall(serde_json::json!({ "model": "opus" }));
        assert_eq!(empty, serde_json::json!({ "model": "opus" }));

        // a hooks-only file empties back to no hooks key at all
        let only_ours = hooks_with_install(
            serde_json::json!({}),
            CLAUDE_HOOKS,
            "claude-code",
            HookStyle::Exec,
            HELPER,
        )
        .unwrap();
        assert_eq!(hooks_with_uninstall(only_ours), serde_json::json!({}));
    }

    #[test]
    fn hook_install_refuses_malformed_documents() {
        let install =
            |v| hooks_with_install(v, CLAUDE_HOOKS, "claude-code", HookStyle::Exec, HELPER);
        assert!(install(serde_json::json!([1, 2])).is_err());
        assert!(install(serde_json::json!({ "hooks": "nope" })).is_err());
        assert!(install(serde_json::json!({ "hooks": { "Stop": "nope" } })).is_err());
        // partial installs read as OFF so re-enabling repairs them
        let partial = serde_json::json!({
            "hooks": { "Stop": [{ "hooks": [{ "type": "command",
                "command": "\"$HOME/.deck/bin/deck-status-helper\" claude-code turn-done" }] }] }
        });
        assert!(!hooks_installed(&partial, CLAUDE_HOOKS));
    }

    #[test]
    fn stale_entries_are_ours_and_get_rewritten_to_the_current_spec() {
        let legacy_command =
            |state: &str| format!("\"$HOME/.deck/bin/deck-status-helper\" claude-code {state}");
        let legacy = serde_json::json!({
            "model": "opus",
            "hooks": {
                "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": legacy_command("working"), "timeout": 10 }] }],
                "Notification": [{ "matcher": "permission_prompt|idle_prompt", "hooks": [{ "type": "command", "command": legacy_command("needs-input"), "timeout": 10 }] }],
                "Stop": [
                    { "hooks": [{ "type": "command", "command": "afplay /System/Library/Sounds/Glass.aiff" }] },
                    { "hooks": [{ "type": "command", "command": legacy_command("turn-done"), "timeout": 10 }] }
                ]
            }
        });
        // a legacy install still reads as installed, but not as current:
        // the helper path is stale AND the matcher predates the narrowing
        assert!(hooks_installed(&legacy, CLAUDE_HOOKS));
        assert!(!hooks_are_current(
            &legacy,
            CLAUDE_HOOKS,
            "claude-code",
            HookStyle::Exec,
            HELPER
        ));

        // migration = the ordinary install over the legacy document
        let migrated = hooks_with_install(
            legacy.clone(),
            CLAUDE_HOOKS,
            "claude-code",
            HookStyle::Exec,
            HELPER,
        )
        .unwrap();
        assert!(hooks_are_current(
            &migrated,
            CLAUDE_HOOKS,
            "claude-code",
            HookStyle::Exec,
            HELPER
        ));
        assert_eq!(migrated["model"], "opus");
        assert_eq!(migrated["hooks"]["Stop"].as_array().unwrap().len(), 2);
        let text = serde_json::to_string(&migrated).unwrap();
        assert!(!text.contains(".deck/bin"), "no legacy path survives");
        assert!(text.contains(HELPER));
        // the stale wide matcher is gone — an idle prompt no longer decays a
        // finished card into "needs-input"
        assert_eq!(
            migrated["hooks"]["Notification"][0]["matcher"],
            "permission_prompt"
        );

        // a bundle that moved is equally not current
        let moved = "/Users/x/Applications/deck.app/Contents/MacOS/deck-status-helper";
        assert!(!hooks_are_current(
            &migrated,
            CLAUDE_HOOKS,
            "claude-code",
            HookStyle::Exec,
            moved
        ));
        assert!(hooks_are_current(
            &hooks_with_install(
                migrated.clone(),
                CLAUDE_HOOKS,
                "claude-code",
                HookStyle::Exec,
                moved
            )
            .unwrap(),
            CLAUDE_HOOKS,
            "claude-code",
            HookStyle::Exec,
            moved
        ));

        // a shell-form entry with the right path is not current for Claude
        // Code (exec form expected) but is for Codex
        let shell_form = hooks_with_install(
            serde_json::json!({}),
            CLAUDE_HOOKS,
            "claude-code",
            HookStyle::ShellAsync,
            HELPER,
        )
        .unwrap();
        assert!(!hooks_are_current(
            &shell_form,
            CLAUDE_HOOKS,
            "claude-code",
            HookStyle::Exec,
            HELPER
        ));
        assert!(hooks_are_current(
            &shell_form,
            CLAUDE_HOOKS,
            "claude-code",
            HookStyle::ShellAsync,
            HELPER
        ));
        let exec_form = hooks_with_install(
            shell_form,
            CLAUDE_HOOKS,
            "claude-code",
            HookStyle::Exec,
            HELPER,
        )
        .unwrap();
        assert!(hooks_are_current(
            &exec_form,
            CLAUDE_HOOKS,
            "claude-code",
            HookStyle::Exec,
            HELPER
        ));
        assert!(!serde_json::to_string(&exec_form).unwrap().contains("\\\""));

        // uninstall removes legacy and current entries alike
        let removed = hooks_with_uninstall(legacy);
        assert_eq!(removed["hooks"]["Stop"].as_array().unwrap().len(), 1);
        assert!(!hooks_installed(&removed, CLAUDE_HOOKS));
    }

    /// Boot migration rewrites whenever the document is not current, so
    /// install's OWN output must be current for every module — otherwise
    /// deck would rewrite the user's agent config on every launch. One
    /// spec event carrying two specs is exactly how that would happen.
    #[test]
    fn installing_any_module_leaves_a_document_the_migration_will_not_touch() {
        for (specs, source, style) in [
            (CLAUDE_HOOKS, "claude-code", HookStyle::Exec),
            (CODEX_HOOKS, "codex", HookStyle::ShellAsync),
        ] {
            let mut events: Vec<&str> = specs.iter().map(|(e, _, _)| *e).collect();
            events.sort_unstable();
            let unique = events.len();
            events.dedup();
            assert_eq!(events.len(), unique, "{source}: one spec per event");

            let doc = hooks_with_install(
                serde_json::json!({ "model": "opus" }),
                specs,
                source,
                style,
                HELPER,
            )
            .unwrap();
            assert!(hooks_installed(&doc, specs), "{source}");
            assert!(
                hooks_are_current(&doc, specs, source, style, HELPER),
                "{source}: a fresh install must not be stale"
            );
        }
    }

    /// The migration's whole point after 0.5.13: a user who enabled the
    /// toggle under an older spec gets the narrowed matcher without touching
    /// the Settings switch, and a retired event leaves nothing behind.
    #[test]
    fn a_spec_change_alone_makes_installed_hooks_stale() {
        let current = hooks_with_install(
            serde_json::json!({}),
            CLAUDE_HOOKS,
            "claude-code",
            HookStyle::Exec,
            HELPER,
        )
        .unwrap();
        let is_current = |doc: &serde_json::Value| {
            hooks_are_current(doc, CLAUDE_HOOKS, "claude-code", HookStyle::Exec, HELPER)
        };
        assert!(is_current(&current));

        // same helper, same events, only the matcher predates the narrowing
        let mut wide = current.clone();
        wide["hooks"]["Notification"][0]["matcher"] =
            serde_json::json!("permission_prompt|idle_prompt");
        assert!(hooks_installed(&wide, CLAUDE_HOOKS), "still installed");
        assert!(!is_current(&wide), "a matcher change is a stale install");
        assert!(is_current(
            &hooks_with_install(wide, CLAUDE_HOOKS, "claude-code", HookStyle::Exec, HELPER)
                .unwrap()
        ));

        // an event this deck version no longer registers: not current, and
        // install drops the husk instead of leaving a dead hook behind
        let mut retired = current.clone();
        retired["hooks"]["SessionStart"] = serde_json::json!([spec_entry(
            HookStyle::Exec,
            HELPER,
            "claude-code",
            &("SessionStart", None, "working")
        )]);
        assert!(!is_current(&retired));
        let repaired = hooks_with_install(
            retired,
            CLAUDE_HOOKS,
            "claude-code",
            HookStyle::Exec,
            HELPER,
        )
        .unwrap();
        assert!(is_current(&repaired));
        assert!(repaired["hooks"].get("SessionStart").is_none());
        assert_eq!(repaired, current);

        // a duplicate deck entry (a hand-edited file) is stale, and install
        // collapses it back to exactly one
        let mut doubled = current.clone();
        let entry = doubled["hooks"]["Stop"][0].clone();
        doubled["hooks"]["Stop"].as_array_mut().unwrap().push(entry);
        assert!(!is_current(&doubled));
        assert_eq!(
            hooks_with_install(
                doubled,
                CLAUDE_HOOKS,
                "claude-code",
                HookStyle::Exec,
                HELPER
            )
            .unwrap(),
            current
        );

        // an EMPTY array the user wrote is theirs — install never prunes it
        let user_empty = hooks_with_install(
            serde_json::json!({ "hooks": { "PreToolUse": [] } }),
            CLAUDE_HOOKS,
            "claude-code",
            HookStyle::Exec,
            HELPER,
        )
        .unwrap();
        assert_eq!(user_empty["hooks"]["PreToolUse"], serde_json::json!([]));
    }

    #[test]
    fn helper_path_requires_a_present_quotable_sidecar_in_a_release_location() {
        let dir = std::env::temp_dir().join(format!("deck-hooks-{}", std::process::id()));
        let bundle = dir.join("deck.app");
        let macos = bundle.join("Contents").join("MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        assert!(helper_path_in(&bundle).is_err(), "missing sidecar");
        std::fs::write(macos.join(HELPER_NAME), "binary").unwrap();
        let helper = helper_path_in(&bundle).unwrap();
        assert!(helper.ends_with("/deck.app/Contents/MacOS/deck-status-helper"));
        assert!(helper.contains(HELPER_MARKER));
        assert_eq!(
            hook_value(HookStyle::ShellAsync, &helper, "codex", "working")["command"],
            format!("\"{helper}\" codex working")
        );
        assert_eq!(
            hook_value(HookStyle::Exec, &helper, "claude-code", "working")["command"],
            helper
        );

        let quoted = dir.join("we\"ird").join("deck.app");
        let quoted_macos = quoted.join("Contents").join("MacOS");
        std::fs::create_dir_all(&quoted_macos).unwrap();
        std::fs::write(quoted_macos.join(HELPER_NAME), "binary").unwrap();
        assert!(helper_path_in(&quoted).is_err(), "unquotable path");
        let _ = std::fs::remove_dir_all(&dir);

        // the test binary is not a release install: no hook may name it
        let error = installed_helper_path().unwrap_err();
        assert!(error.message().contains("/Applications/deck.app"));
    }
}
