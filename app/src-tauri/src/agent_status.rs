//! agent_status.rs — modular, content-free agent state for cards.
//!
//! Claude Code and Codex run hooks inside deck's tmux panes. Pi reports
//! through its bundled observer on the same socket. Each hook invokes the bundled
//! `deck-status-helper`, which forwards ONE closed status word plus the pane
//! identity it inherited from `$TMUX`/`$TMUX_PANE` to a local unix socket in
//! the deck data dir. This module owns that socket, the closed vocabulary,
//! the pane-generation store, the ONE selector of a session's Signal pane,
//! and the reconciliation that clears state when its generation ends.
//! Configuration editing, installation, migration and adapter declarations
//! live in `agent_status/hooks.rs`; its header owns that contract.
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
//! Adding an agent module = one `AgentAdapter` declaration + its entry in
//! the static extension tables (`extensions.rs`): the adapter names the
//! source word, how it is installed (`Install`: a hooks document with its
//! spec table, style and file, or Pi's one extension entry) and a closed
//! proof class. The socket and the store are shared. A registered adapter
//! is only a recognized source word: it earns no Signal, trust, readiness
//! or remote admission by being listed.
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
//! error and never touch agent config.
//! Explicit debug re-entry experiments may instead read/write their bound
//! project Lifecycle hook file. Signal/global installation and boot migration
//! keep the ordinary location restrictions; observation remains file-derived.
//! `migrate_agents_on_boot` (release
//! installs only) rewrites installed entries that are not what the CURRENT
//! spec describes — `hooks_are_current` compares deck's WHOLE hook and the
//! matcher of its group (event, matcher, helper path, style, args) and
//! rejects a deck hook left under a retired event, while what the user added
//! to that group is not deck's to judge. So a spec change (a narrowed
//! matcher, a moved bundle, the legacy copy) reaches users who enabled the
//! toggle under an older version
//! without them touching the switch; installed-ness alone (`hooks_installed`)
//! cannot see that. It also deletes the legacy copy once nothing references
//! it. Install takes deck's hooks out document-wide, hook by hook, before
//! writing the specs: a hook the user keeps in the same group stays in it,
//! and an EMPTY array the user wrote is left exactly as written; uninstall
//! prunes only what taking deck's hooks out emptied. The file is never
//! serialized again: what is written is its owner's text with deck's hooks
//! cut out and deck's groups put in, every other byte (the order of the
//! keys, the indentation, the spelling of a number or an escape) where it
//! was, and only when that text parses back to exactly the value the
//! install or uninstall decided; a text deck cannot follow is refused and
//! the file left alone. Nothing is written when the file already is what
//! was asked for. The write replaces the file the path NAMES
//! (`datadir::atomic_write_through_link`): a symlinked settings file stays
//! a link. Hooks inherit `$TMUX`/`$TMUX_PANE`; the helper
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
//! helper's parent is that leader for every hook word. Finally the chain
//! from the helper to that leader must stay on the pane's terminal, apart
//! from the one terminal-less process group the agent spawns the hook in
//! (`terminal-discontinuity`, `terminal_continuous`). Shared-daemon Signal
//! can become Trusted only when an event is verifiably bound to a specific
//! interactive client and its CURRENT pane foreground generation. This is
//! a provenance rule, not a version allowlist. Certifications: Codex 0.157.1
//! BLOCKED; 0.160.0 BLOCKED (2026-10-02), documented in
//! `docs/codex-signal-certification-20261002.md`. In both tested releases,
//! the shared background app-server (feature `daemon_auto_start`) spawns
//! clients' hooks with the environment of the client that started it, so
//! `$TMUX_PANE` names the starter's pane for all of them; while the starter
//! TUI lives in pane P, another client's hook passes `foreign-pane` and the
//! generation check for P. The daemon is a second terminal-less group in the
//! chain, so those events are refused. Without trustworthy client/pane
//! binding, shared-daemon Signal remains unavailable and scheduler/readiness
//! gates stay in place. Embedded Codex (`--no-daemon`) attribution topology
//! can satisfy this model; that does not certify full attention coverage.
//! Claude Code keeps its existing admission. Inherited `$TMUX_PANE`, cwd,
//! timing, executable names, transcript paths and session/thread/turn IDs
//! are never pane ownership proof.
//! The Board receives generation-bound Codex coverage separately from the
//! observation: a quiet diagnostic can explain missing Signal without
//! creating an attention episode or changing any automation hold.
//! `poll_sessions` reconciles with one process-table snapshot so the state dies with the
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
//! early. Adding an agent module = one `AgentAdapter` (registered in
//! `extensions.rs`) whose spec table calls the same helper with its own
//! source word, behind its own Settings toggle.
//!
//! # A second, separate family on the same socket and the same edit engine
//! Re-entry (`agent_reentry`) needs a native session id, which the status
//! helper's payload contract must never carry. So it does not extend that
//! helper: a second bundled helper, `deck-lifecycle-helper`, speaks its own
//! closed protocol family (`agent_reentry::lifecycle`), and `dispatch`
//! tells the families apart before the Signal parser sees a byte — a lifecycle
//! line never reaches `parse_event`, `ingest`, the store, interactions,
//! episodes, notifications or any Signal consumer, and a Signal line never
//! reaches the lifecycle mailbox. An older app that does not know the
//! family refuses the line as an unknown Signal state.
//! The two families share the accept loop, the bounded read of a
//! connection's first line and the dispatcher (the listener section below),
//! and nothing after it. A lifecycle
//! receipt is verified by the kernel's view of the client's running code,
//! a call that cannot be interrupted; the dispatcher is every agent's
//! Signal path, so it never makes that call: it names the lease (one short
//! lock) and hands the connection to ONE receipt worker over a bounded
//! queue without waiting (`Receipts`). A receipt that takes long, or never
//! returns, delays no Signal and starts no further thread; receipts that
//! pile up behind it are refused — a full queue at once, a waited one when
//! its own deadline has passed — and a refused receipt is only an entry
//! that is not confirmed.
//! Hook ownership is per `Family` (`Signal`: `deck-status-helper`,
//! `Lifecycle`: `deck-lifecycle-helper`, each with its own marker). Install,
//! currency check, uninstall and boot migration act on ONE family's hooks:
//! the status toggle never adds or removes `SessionStart`/`SessionEnd`,
//! `agent_lifecycle_hooks_set` (the separate, explicit "Claude session
//! re-entry observation" opt-in) never touches the status hooks, and boot
//! migration repairs the lifecycle hooks only where they already are
//! installed — a user of the status hooks is never enrolled. The same
//! release-location rule, byte-preserving text edit, mode and symlink
//! handling apply to both. An adapter's optional `reentry` capability is
//! static data: registered is not enabled, approved or trusted.
//!
//! # A third family: a Pi observer's messages
//! Pi runs no helper per event. Its observer is an extension inside the Pi
//! process that speaks to this socket itself, one message per connection
//! (`agent_pi`: the wire, the settings entry, the transport). `dispatch`
//! asks for that family FIRST — the lifecycle family would take any line
//! with a `protocol` key — and hands the connection to the bounded Pi
//! transport without waiting; nothing of a Pi message is verified or
//! answered on the accept loop or the dispatcher, and its lines reach
//! neither the hook parser nor the lifecycle mailbox. The reverse door is closed too: a hook line
//! (wire v1 or v2) that names source `pi` is refused, `pi-observer-required`
//! — Pi has no interaction id of its own, and a number an observer counts
//! must never look like one.
//! What a Pi observer proved is written here by four functions and nothing
//! else. `pi_observed` records, for a pane's foreground generation, that a
//! Pi observer was proven there and what may be said about its Signal
//! (`PiCoverage`: `connected` is LIMITED coverage — work started, an
//! interaction ended, never an input request; `unavailable` claims no
//! cause; `incompatible` is a release this build does not know).
//! A generation is a pid and a birth instant, and an exec inside that pid
//! keeps both: what an attest proved is therefore kept with a watch of the
//! process's EXECUTION INSTANCE (`procinfo::InstanceWatch`), and every
//! reader — the poll's evidence, the scheduler's two questions, the restart
//! review, the projection of a word a Pi observer wrote — asks that watch
//! before it says anything. A process that executed another image is no
//! recognized Pi and has no word, coverage or session, whatever the
//! program before it proved; a kernel that cannot be asked leaves the
//! recognition (it only ever adds a hold) and no word.
//! With the separate lifecycle capability the same observer may also say
//! which native session its runtime is in; that goes to `agent_reentry::
//! lifecycle::observe_pi` and never into this store. All a card learns of
//! it is the closed word `Evidence::pi_session` (`observed`): no id, no
//! state, no episode, no hold lifted.
//! `pi_signal` writes one run boundary — `working` or `turn-done`, the only
//! two words the closed `agent_pi::RunState` has — into the SAME store,
//! under `InteractionKey::PiRun { lease, run }`, with the same episode,
//! viewed and projection rules as a hook word. `pi_lost` withdraws exactly
//! the observation one lease wrote, with its episode (`notify::
//! withdraw_episode`), and turns `connected` into `unavailable`; a lost
//! observer is never a turn that ended. The same call serves a lease that
//! only lost its Signal capability and goes on observing sessions
//! (`agent_pi::transport`, "Capabilities narrow"). `pi_waits_for_reload`
//! remembers, for a recognized Pi whose observer was ended for its
//! capabilities, that this runtime does not connect again by itself: the
//! Settings row then says `needs-reload` for as long as that Pi runs
//! (`pi_reload_pending`), instead of calling the entry current. Coverage
//! is explanatory: it creates no episode, joins no badge and lifts no
//! hold. Recognition — a proven observer of the generation, or Pi's own
//! name on the foreground (`pi_named`: tmux's name, or the name the leader
//! gives itself, since an npm install is `node` to tmux) — only ever ADDS
//! the scheduler's Pi hold (`Evidence::pi`, `pi_foreground`); a name proves
//! nothing and admits nothing.

mod hooks;
// Keep the existing IPC and adapter paths; hooks owns configuration only.
pub(crate) use hooks::*;

use std::collections::HashMap;

use crate::tmux::PaneRow;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::applog;
use crate::error::DeckError;
use crate::procinfo::{Continuity, InstanceWatch};
use crate::sync::LockRecover;

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
    pub(crate) fn of(row: &PaneRow) -> Self {
        Self {
            server_pid: row.server_pid,
            pane_id: row.pane_id.clone(),
        }
    }

    /// Whether `row` is this pane of this server generation.
    pub(crate) fn names(&self, row: &PaneRow) -> bool {
        self.server_pid == row.server_pid && self.pane_id == row.pane_id
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

impl ForegroundGeneration {
    /// The generation a process is, by its pid and birth instant as
    /// `procinfo` read them. Nothing is proven by building one: a caller
    /// compares it with what the kernel says about a pane's foreground.
    pub(crate) fn of(pid: u32, birth: (u64, u32)) -> Self {
        Self {
            pid,
            start_seconds: birth.0,
            start_micros: birth.1,
        }
    }

    pub(crate) fn pid(&self) -> u32 {
        self.pid
    }
    /// The leader's birth instant, as `procinfo::process_start` reads it.
    pub(crate) fn birth(&self) -> (u64, u32) {
        (self.start_seconds, self.start_micros)
    }
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
    last: Option<(InteractionKey, &'static str)>,
    viewed: bool,
    /// The last accepted word came from Codex (the only source whose
    /// observation an `Unavailable` trust proof clears).
    codex: bool,
    /// The last accepted word came through this Pi observer lease
    /// (`agent_pi::transport`): the one observation that lease's loss
    /// withdraws. `None` for a word a hook reported.
    pi: Option<PiWriter>,
}

/// The Pi observer lease that wrote an observation, and the execution
/// instance its process was when the lease was bound.
struct PiWriter {
    lease: u64,
    instance: Arc<InstanceWatch>,
}

/// What makes two accepted words the same observation (FR-SI-05). A closed
/// set, one variant per way an interaction is identified, so one kind can
/// never be read as another: a hook word without an id, a source's own
/// interaction id (wire v2), and a Pi observer's run. The last is NOT a
/// native id: Pi has none that spans a run, so the observer counts its runs
/// inside one connection and the backend names that connection (`lease`).
/// Only `pi_signal` builds it, from a lease the Pi transport verified.
#[derive(Clone, Debug, PartialEq, Eq)]
enum InteractionKey {
    Word,
    NativeUuid(String),
    PiRun { lease: u64, run: u64 },
}

impl Entry {
    /// Still the observation of `row`'s pane: the same session and the same
    /// pane process.
    fn of_pane(&self, row: &PaneRow) -> bool {
        self.session_id == row.session_id && self.pane_pid == row.pane_pid
    }

    /// Whether `generation`, the pane's foreground generation as read now,
    /// is the one that reported this observation; `None` when it could not
    /// be read (no proof either way — each caller says what that means).
    fn reported_by(&self, generation: Option<ForegroundGeneration>) -> Option<bool> {
        if !self.speaks() {
            return Some(false);
        }
        generation.map(|now| now == self.generation)
    }

    /// Whether the program that reported this still runs as it did. A hook
    /// word is bound by its generation alone; a Pi observer's word also by
    /// its process's execution instance, which an exec inside the same pid
    /// ends while pid and birth instant stay (`procinfo::InstanceWatch`). A
    /// kernel that cannot be asked proves nothing, so that word is not
    /// spoken either.
    fn speaks(&self) -> bool {
        self.pi
            .as_ref()
            .is_none_or(|writer| writer.instance.continuity() == Continuity::Same)
    }
}

/// Deck-local, process-local attention episode (FR-SI-05): "is this the
/// same accepted observation that was already surfaced or viewed?". An
/// opaque counter — never a source id, never Agent truth, never side-effect
/// authority. Only an ACCEPTED observation allocates one: the same (source
/// interaction, word) — or, for v1, the same word — keeps its episode; any
/// other accepted change, or a new foreground generation, gets a new one.
pub(crate) type EpisodeId = u64;

static NEXT_EPISODE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// The episode the next accepted observation gets: a test asks whether any
/// was allocated in between, whatever became of it afterwards.
#[cfg(test)]
pub(crate) fn next_episode_for_tests() -> EpisodeId {
    NEXT_EPISODE.load(std::sync::atomic::Ordering::Relaxed)
}

/// How often a Pi observer's proof was written into this module's stores —
/// recognized, a word, withdrawn, waiting for a reload — whatever became of
/// it afterwards: a test asks whether ANYTHING was written, not only what
/// is left.
#[cfg(test)]
static PI_WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[cfg(test)]
pub(crate) fn pi_writes_for_tests() -> u64 {
    PI_WRITES.load(std::sync::atomic::Ordering::Relaxed)
}

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

/// Whether Codex Signal can be trusted for one pane's foreground generation
/// (the Codex shared-daemon FR). NOT an agent state and never shown as one:
/// the scheduler's agent hold reads it (`select::agent_holds`), and the
/// Board projects it separately as diagnostic coverage (never an agent
/// state or an attention episode), for
/// an existing session whose Signal target's foreground is literally
/// `codex`, whose current foreground generation has a matching trust
/// record whatever its executable name (a wrapper, `node`), or whose queue
/// row is configured for Codex (`expected_process`), where anything short
/// of `Trusted` holds. Executable names only decide that a hold applies;
/// they never prove anything. Evidence comes only from
/// Codex-sourced hooks and only degrades toward safety within one
/// generation — `Unknown → Trusted`, `Unknown → Unavailable`,
/// `Trusted → Unavailable`, never back — and a new generation starts
/// `Unknown`:
/// - `Trusted`: a Codex hook of this generation was ACCEPTED — it proved
///   itself pane-bound through every admission rule;
/// - `Unavailable`: a hook whose ancestry reaches this pane's own process
///   and current foreground leader (so it passed `foreign-pane` and the
///   generation check) was refused for `terminal-discontinuity`: this
///   generation's hooks are not provably its own (e.g. a shared daemon
///   without trustworthy interactive-client/pane binding);
/// - `Unknown`: no evidence yet (fresh process, or Deck restarted).
///
/// A discontinuity proves that events can now claim this pane without
/// trustworthy attribution, so it dominates an earlier `Trusted`: the
/// generation's Codex observation is cleared at once (no stale attention
/// or notification), and its later Codex hooks are refused
/// (`ambiguous-generation`) even when otherwise admissible — one good hook
/// never heals an ambiguous generation. This may drop a valid observation
/// because another client shared the channel; that is the intended failure
/// direction. A refused hook can only mark the generation its ancestry
/// proves, never the pane its inherited `$TMUX_PANE` merely names, so a
/// client sharing another pane's daemon marks only the daemon's starter.
/// Claude-sourced events neither set nor obey it. Never inferred from a
/// Codex version.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum CodexSignalTrust {
    #[default]
    Unknown,
    Trusted,
    Unavailable,
}

/// The interaction words a Claude hook may prove `AgentInteractionEstablished`
/// with: each comes from an interaction-level hook (UserPromptSubmit,
/// the permission Notification, Stop). Closed on purpose — a future
/// startup/session lifecycle word accepted by the transport must NOT count
/// merely because it was admitted.
const CLAUDE_INTERACTION_WORDS: &[&str] = &[WORKING, NEEDS_INPUT, TURN_DONE];

/// Evidence Deck holds about ONE pane foreground generation (same identity
/// as `Entry`: pane, leader pid, birth instant), in memory only — a Deck
/// restart starts without any, and nothing is persisted or reconstructed.
///
/// - `codex`: `CodexSignalTrust` for this generation, `None` until a
///   Codex-sourced hook proved or disproved it (see `prove_trust`).
/// - `claude_interaction`: an ACCEPTED Claude interaction word
///   (`CLAUDE_INTERACTION_WORDS`) came from this generation.
///
/// Together they define `AgentInteractionEstablished` for the scheduler's
/// first-interaction gate (`scheduler/select.rs`): Codex `Trusted`, or a
/// Claude interaction. It means only "this exact process generation has
/// produced at least one real agent interaction through Deck's admission
/// path" — never a current agent state, never "the editor is ready now",
/// and never lifecycle, close or external-chain authority.
struct GenerationEvidence {
    session_id: String,
    pane_pid: u32,
    generation: ForegroundGeneration,
    codex: Option<CodexSignalTrust>,
    claude_interaction: bool,
    /// A Pi observer of this generation said hello and the kernel proved it
    /// to be the pane's foreground program (`agent_pi::transport`). `Some`
    /// is what recognizes the generation as Pi; `PiSeen::now` is what Deck
    /// may say about its Signal coverage. Never interaction evidence.
    pi: Option<PiSeen>,
}

/// What was proven of one Pi observer, and of which execution instance.
///
/// A generation is a pid and a birth instant, and an exec keeps both. What
/// a hello proved was proven of the PROGRAM that ran then, so it is kept
/// with a watch of that process (`procinfo::InstanceWatch`) and read only
/// through `now`: once the process executed another image or is gone,
/// nothing here describes what runs in that pid.
struct PiSeen {
    coverage: PiCoverage,
    instance: Arc<InstanceWatch>,
    /// This runtime's observer was ended for a capability, and its code
    /// does not connect again before Pi loads its extensions anew
    /// (`agent_pi::config`, `needs-reload`).
    reload: bool,
}

impl PiSeen {
    /// The coverage that may be reported NOW; `None` when the process is no
    /// longer the instance that was proven. A kernel that cannot be asked
    /// leaves the recognition — it only ever adds a hold — and never a
    /// `Connected`.
    fn now(&self) -> Option<PiCoverage> {
        match self.instance.continuity() {
            Continuity::Same => Some(self.coverage),
            Continuity::Unreadable => Some(match self.coverage {
                PiCoverage::Connected => PiCoverage::Unavailable,
                other => other,
            }),
            Continuity::Executed | Continuity::Exited => None,
        }
    }

    /// Still the instance that was proven, by a kernel that answered.
    fn continuous(&self) -> bool {
        self.instance.continuity() == Continuity::Same
    }
}

/// What Deck may say about a recognized Pi's Signal coverage. Presentation
/// only: not an agent state, not an attention episode, and no hold depends
/// on which of the three it is. `Connected` is LIMITED coverage — the start
/// of work and the end of an interaction; Pi gives Deck no reliable way to
/// observe that it waits for input, so that is never reported.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum PiCoverage {
    /// An observer with the signal capability is bound to this generation.
    Connected,
    /// No such observer is bound now. Why is not known and not guessed:
    /// never loaded, turned off, disconnected, or reloading.
    Unavailable,
    /// The observer's Pi release is outside the range this build supports.
    Incompatible,
}

static EVIDENCE: Mutex<Option<HashMap<PaneKey, GenerationEvidence>>> = Mutex::new(None);

fn with_evidence<R>(f: impl FnOnce(&mut HashMap<PaneKey, GenerationEvidence>) -> R) -> R {
    let mut guard = EVIDENCE.lock_or_recover();
    f(guard.get_or_insert_with(HashMap::new))
}

/// Apply `update` to this exact pane generation's evidence, starting a
/// fresh record when the pane has none or another generation's.
fn with_generation(
    row: &PaneRow,
    generation: ForegroundGeneration,
    update: impl FnOnce(&mut GenerationEvidence),
) {
    with_evidence(|records| {
        let record = records
            .entry(PaneKey::of(row))
            .or_insert_with(|| GenerationEvidence {
                session_id: row.session_id.clone(),
                pane_pid: row.pane_pid,
                generation,
                codex: None,
                claude_interaction: false,
                pi: None,
            });
        if record.session_id != row.session_id
            || record.pane_pid != row.pane_pid
            || record.generation != generation
        {
            *record = GenerationEvidence {
                session_id: row.session_id.clone(),
                pane_pid: row.pane_pid,
                generation,
                codex: None,
                claude_interaction: false,
                pi: None,
            };
        }
        update(record);
    });
}

/// Record Codex `trust` for the pane generation, only ever toward safety:
/// within one generation `Unavailable` replaces anything and nothing
/// replaces `Unavailable`.
fn prove_trust(row: &PaneRow, generation: ForegroundGeneration, trust: CodexSignalTrust) {
    with_generation(row, generation, |record| {
        if record.codex.is_none() || trust == CodexSignalTrust::Unavailable {
            record.codex = Some(trust);
        }
    });
}

/// Record that an accepted Claude interaction word came from this pane
/// generation.
fn prove_claude_interaction(row: &PaneRow, generation: ForegroundGeneration) {
    with_generation(row, generation, |record| record.claude_interaction = true);
}

/// Whether this exact pane generation was proven Codex-`Unavailable`.
fn ambiguous(row: &PaneRow, generation: ForegroundGeneration) -> bool {
    with_evidence(|records| {
        records.get(&PaneKey::of(row)).is_some_and(|r| {
            r.session_id == row.session_id
                && r.pane_pid == row.pane_pid
                && r.generation == generation
                && r.codex == Some(CodexSignalTrust::Unavailable)
        })
    })
}

/// What the scheduler may know about one session's Signal target's CURRENT
/// foreground generation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Evidence {
    /// Codex Signal trust: the generation's proof whatever the foreground's
    /// name (`node`, a wrapper), else `Unknown` when the foreground is
    /// literally `codex`, else `None`.
    pub(crate) codex: Option<CodexSignalTrust>,
    /// An accepted Claude interaction came from this generation.
    pub(crate) claude_interaction: bool,
    /// The target is a recognized Pi: an observer of this generation was
    /// proven (whatever the foreground's name — `node` for an npm install),
    /// or the foreground is literally `pi`. Recognition only ever ADDS the
    /// scheduler's Pi hold; a name proves nothing and admits nothing.
    pub(crate) pi: bool,
    /// What may be said about that Pi's Signal coverage (`None`: not a
    /// recognized Pi). A name alone reads `Unavailable`.
    pub(crate) pi_signal: Option<PiCoverage>,
    /// A live observer of this generation reported which native session its
    /// runtime is in (`agent_reentry::lifecycle::pi_session`). An
    /// observation with Signal's trust ceiling: it explains, and it makes
    /// Pi neither re-enterable nor ready for input.
    pub(crate) pi_session: Option<crate::agent_reentry::lifecycle::PiSession>,
}

/// Evidence per session's Signal target, bound to its CURRENT foreground
/// generation via `generation_now` (point reads in production,
/// `live_generation`), so evidence never outlives the process that earned
/// it, even between Board polls. The executable name only decides that a
/// hold applies — it never proves anything. `name_now` is the name a
/// process gives itself (`foreground_name`): an npm-installed Pi is `node`
/// to tmux and `pi` by its own argv, and either name recognizes it.
pub(crate) fn generation_evidence(
    rows: &[PaneRow],
    generation_now: impl Fn(u32) -> Option<ForegroundGeneration>,
    name_now: impl Fn(u32) -> Option<String>,
) -> HashMap<String, Evidence> {
    let targets = signal_targets(rows);
    with_evidence(|records| {
        targets
            .into_iter()
            .map(|(session, row)| {
                let now = generation_now(row.pane_pid);
                let proven = records.get(&PaneKey::of(row)).filter(|r| {
                    r.session_id == row.session_id
                        && r.pane_pid == row.pane_pid
                        && now == Some(r.generation)
                });
                let seen = proven.and_then(|r| r.pi.as_ref());
                let observed = seen.and_then(PiSeen::now);
                let pi = observed.is_some() || pi_named(&row.command, now, &name_now);
                let evidence = Evidence {
                    codex: proven
                        .and_then(|r| r.codex)
                        .or((row.command == "codex").then_some(CodexSignalTrust::Unknown)),
                    claude_interaction: proven.is_some_and(|r| r.claude_interaction),
                    pi,
                    pi_signal: pi.then_some(observed.unwrap_or(PiCoverage::Unavailable)),
                    // a session is said only by a lease whose process is
                    // still the instance it was bound as
                    pi_session: now
                        .filter(|_| seen.is_some_and(PiSeen::continuous))
                        .and_then(|generation| {
                            crate::agent_reentry::lifecycle::pi_session(
                                &PaneKey::of(row),
                                generation,
                            )
                        }),
                };
                (session, evidence)
            })
            .collect()
    })
}

/// Whether a pane's foreground carries Pi's name: tmux's name for it, or
/// the name the foreground leader gives itself. A shell in front is never
/// asked. A name recognizes — it adds the scheduler's Pi hold and the card's
/// coverage note — and proves nothing: no Signal, target or admission
/// follows from it.
fn pi_named(
    command: &str,
    generation: Option<ForegroundGeneration>,
    name_now: &impl Fn(u32) -> Option<String>,
) -> bool {
    command == crate::agent_pi::PROCESS
        || (!crate::context::shell_process(Some(command))
            && generation
                .and_then(|generation| name_now(generation.pid))
                .as_deref()
                == Some(crate::agent_pi::PROCESS))
}

/// The name a process gives itself: the sanitized basename of its argv[0],
/// one kernel read (`procinfo::argv0`). Production's `name_now`.
pub(crate) fn foreground_name(pid: u32) -> Option<String> {
    crate::procinfo::argv0(pid).and_then(|name| crate::context::sanitize_process(&name))
}

/// The foreground generation of `pane_pid`'s terminal from two point reads
/// (the pane process for its tty's foreground group, then that group's
/// leader) — the same facts `foreground_generation` takes from a snapshot,
/// without a table scan on the scheduler tick.
pub(crate) fn live_generation(pane_pid: u32) -> Option<ForegroundGeneration> {
    let pane = crate::procinfo::process(pane_pid)?;
    let leader = crate::procinfo::process(pane.tty_pgid)?;
    (pane.tty != 0
        && leader.tty == pane.tty
        && leader.pgid == leader.pid
        && leader.start_seconds != 0)
        .then_some(ForegroundGeneration {
            pid: leader.pid,
            start_seconds: leader.start_seconds,
            start_micros: leader.start_micros,
        })
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
        .filter(|s| crate::extensions::agent(s).is_some())
        .ok_or("unknown-source")?;
    // Pi has no hook helper and no native interaction id: its words exist
    // only as messages of an observer the Pi transport verified. A v1 or v2
    // line that names it is refused whatever else it says.
    if crate::extensions::agent(source).is_some_and(|adapter| adapter.class == SignalClass::Pi) {
        return Err("pi-observer-required");
    }
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
/// without one (the scheduler tick) only ever get a word that can hold. A
/// reader for which a stale word would do more than hold asks `reporting`.
pub(crate) fn projected(target: &PaneRow) -> Option<Observation> {
    with_agents(|agents| {
        agents
            .get(&PaneKey::of(target))
            .filter(|entry| entry.of_pane(target) && entry.speaks())
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

/// session → whether its Signal target's stored observation still speaks
/// for the pane NOW, for a reader that runs between polls and so after no
/// `reconcile` (the Board poll's idle fallback, `commands::poll_idle_tick`).
/// The same two checks `reconcile` makes, without retiring anything:
/// `Some(true)` when the observation's pane identity holds and
/// `generation_now` says its foreground generation is still the one that
/// reported; `Some(false)` when that generation is another one (the agent
/// exited or was replaced under the same shell); `None` when the generation
/// cannot be read, which proves neither. A session with no observation for
/// its target pane is absent. Executable names play no part.
pub(crate) fn reporting(
    rows: &[PaneRow],
    generation_now: impl Fn(u32) -> Option<ForegroundGeneration>,
) -> HashMap<String, Option<bool>> {
    let targets = signal_targets(rows);
    with_agents(|agents| {
        targets
            .into_iter()
            .filter_map(|(session, row)| {
                let entry = agents
                    .get(&PaneKey::of(row))
                    .filter(|entry| entry.of_pane(row))?;
                Some((session, entry.reported_by(generation_now(row.pane_pid))))
            })
            .collect()
    })
}

/// session → projected observation, for every session with a Signal target.
pub(crate) fn projections(rows: &[PaneRow]) -> HashMap<String, Observation> {
    signal_targets(rows)
        .into_iter()
        .filter_map(|(session, target)| Some((session, projected(target)?)))
        .collect()
}

use crate::agent_origin::{terminal_continuous, ORIGIN_HOPS};
// Re-exported: existing callers and tests name the origin through this module.
pub(crate) use crate::agent_origin::Origin;

/// Validate one wire line and commit it to the store. Admission, in order:
/// a kernel peer (`no-peer`), a pane of this server generation
/// (`no-such-pane`), the pane's own process in the peer's ancestry
/// (`foreign-pane`), no shell in the foreground (`shell-foreground`), an
/// establishable foreground generation (`no-generation`) whose leader is
/// also in the peer's ancestry (`generation-mismatch`: the reporter is not
/// the pane's current foreground program — a late event of an exited or
/// replaced agent), and an unbroken terminal from the helper to that leader
/// (`terminal-discontinuity`, see `terminal_continuous`); a Codex event of
/// a generation already proven ambiguous is refused too
/// (`ambiguous-generation`, `CodexSignalTrust`). Runtime proof
/// (Claude Code 2.1.282, Codex 0.156.1): the helper's parent IS the
/// foreground leader; no fixed depth is required, so a wrapper leader is
/// accepted.
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
    // The adapter names only which proof class its source belongs to;
    // whether this event is accepted, and what it proves, is decided here.
    let class = crate::extensions::agent(&event.source)
        .ok_or("unknown-source")?
        .class;
    let codex = class == SignalClass::Codex;
    if !terminal_continuous(&origin.table, &chain, generation.pid, row.pane_pid) {
        if codex {
            prove_trust(row, generation, CodexSignalTrust::Unavailable);
            // the generation's Codex observation stops presenting at once
            let key = PaneKey::of(row);
            let cleared = with_agents(|agents| {
                let stale = agents.get(&key).is_some_and(|entry| {
                    entry.codex
                        && entry.session_id == row.session_id
                        && entry.pane_pid == row.pane_pid
                        && entry.generation == generation
                });
                stale && agents.remove(&key).is_some()
            });
            if cleared {
                renotify(&rows);
            }
        }
        return Err("terminal-discontinuity");
    }
    if codex && ambiguous(row, generation) {
        return Err("ambiguous-generation");
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
                    codex,
                    pi: None,
                },
            );
        }
        let entry = agents.get_mut(&key).ok_or("no-such-pane")?;
        entry
            .interactions
            .admit(event.state, event.interaction.as_deref())?;
        entry.state = event.state;
        entry.codex = codex;
        entry.pi = None;
        // only an accepted observation allocates or changes an episode
        let sameness = (
            event
                .interaction
                .clone()
                .map_or(InteractionKey::Word, InteractionKey::NativeUuid),
            event.state,
        );
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
    if codex {
        prove_trust(row, generation, CodexSignalTrust::Trusted);
    } else if class == SignalClass::Claude && CLAUDE_INTERACTION_WORDS.contains(&event.state) {
        prove_claude_interaction(row, generation);
    }
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

// ---------- Pi observer ingress ------------------------------------------------
//
// Pi's words do not arrive as hook lines: an observer inside the Pi process
// makes a connection to the status socket for each message it sends, and
// `agent_pi::transport` verifies every one (kernel peer, pane, foreground
// generation, lease, order) before anything below is called. These functions
// are the ONLY writers of a Pi observation into the shared store, and they
// take what the transport proved — never a wire line. The store, the
// projection, episodes and every consumer are the same as for a hook word.

/// A proven Pi observer said hello for this pane generation: the generation
/// is recognized as Pi from here on, and `coverage` is what may be said
/// about its Signal — for as long as its process stays the execution
/// instance `instance` watches. Recognition is not interaction evidence.
pub(crate) fn pi_observed(
    row: &PaneRow,
    generation: ForegroundGeneration,
    coverage: PiCoverage,
    instance: &Arc<InstanceWatch>,
) {
    #[cfg(test)]
    PI_WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    with_generation(row, generation, |record| {
        record.pi = Some(PiSeen {
            coverage,
            instance: instance.clone(),
            reload: false,
        });
    });
}

/// The observer of this pane generation was ended for a capability the
/// settings do not enable, and its code does not connect again by itself:
/// the settings entry may say what it likes, this Pi follows only when it
/// loads its extensions again. Forgotten with the generation, the instance
/// or the next proven hello.
pub(crate) fn pi_waits_for_reload(key: &PaneKey, generation: ForegroundGeneration) {
    #[cfg(test)]
    PI_WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    with_evidence(|records| {
        if let Some(seen) = records
            .get_mut(key)
            .filter(|record| record.generation == generation)
            .and_then(|record| record.pi.as_mut())
        {
            seen.reload = true;
        }
    });
}

/// Whether a recognized Pi that still runs waits for such a reload.
pub(crate) fn pi_reload_pending() -> bool {
    with_evidence(|records| {
        records
            .values()
            .filter_map(|record| record.pi.as_ref())
            .any(|seen| seen.reload && seen.continuous())
    })
}

/// Whether a Pi observer was verified for exactly this pane and foreground
/// generation (the restart review's label for a card it will not reopen).
/// A name is not asked: this is what was proven, not what something is
/// called.
pub(crate) fn pi_proven(row: &PaneRow, generation: ForegroundGeneration) -> bool {
    with_evidence(|records| {
        records.get(&PaneKey::of(row)).is_some_and(|r| {
            r.session_id == row.session_id
                && r.pane_pid == row.pane_pid
                && r.generation == generation
                && r.pi.as_ref().and_then(PiSeen::now).is_some()
        })
    })
}

/// Whether a pane's CURRENT foreground is a recognized Pi, for a reader
/// that holds the pane's identity and no listing (the scheduler's pre-fire
/// check): a proven observer of the live generation, or Pi's own name on
/// that generation's leader. Everything is asked now, through
/// `generation_now` and `name_now`; a foreground that cannot be read is not
/// recognized by this — recognition needs a fact.
pub(crate) fn pi_foreground(
    server_pid: u32,
    pane_id: &str,
    pane_pid: u32,
    generation_now: impl Fn(u32) -> Option<ForegroundGeneration>,
    name_now: impl Fn(u32) -> Option<String>,
) -> bool {
    let Some(generation) = generation_now(pane_pid) else {
        return false;
    };
    let key = PaneKey {
        server_pid,
        pane_id: pane_id.to_string(),
    };
    name_now(generation.pid).as_deref() == Some(crate::agent_pi::PROCESS)
        || with_evidence(|records| {
            records.get(&key).is_some_and(|r| {
                r.pane_pid == pane_pid
                    && r.generation == generation
                    && r.pi.as_ref().and_then(PiSeen::now).is_some()
            })
        })
}

/// One run boundary of a proven Pi observer. `lease` and `run` name the
/// observer's lease and its run inside it (`InteractionKey::PiRun`);
/// the word is one of the two the closed `RunState` has, so a Pi input
/// request cannot be written here at all. Stored under its pane like any
/// word; only the session's Signal target reaches the attention layer.
pub(crate) fn pi_signal(
    rows: &[PaneRow],
    row: &PaneRow,
    generation: ForegroundGeneration,
    lease: u64,
    instance: &Arc<InstanceWatch>,
    run: u64,
    state: crate::agent_pi::RunState,
) {
    #[cfg(test)]
    PI_WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let state = match state {
        crate::agent_pi::RunState::Working => WORKING,
        crate::agent_pi::RunState::TurnDone => TURN_DONE,
    };
    let key = PaneKey::of(row);
    let observation = with_agents(|agents| {
        // the same pane generation keeps its entry; anything else (a new
        // foreground generation, a replaced session or pane process) starts
        // fresh, exactly as a hook word does
        let fresh = || Entry {
            state,
            session: row.session_name.clone(),
            session_id: row.session_id.clone(),
            pane_pid: row.pane_pid,
            generation,
            interactions: Interactions::default(),
            episode: 0,
            last: None,
            viewed: false,
            codex: false,
            pi: None,
        };
        let entry = agents.entry(key.clone()).or_insert_with(&fresh);
        if entry.session_id != row.session_id
            || entry.pane_pid != row.pane_pid
            || entry.generation != generation
        {
            *entry = fresh();
        }
        entry.state = state;
        entry.codex = false;
        entry.pi = Some(PiWriter {
            lease,
            instance: instance.clone(),
        });
        let sameness = (InteractionKey::PiRun { lease, run }, state);
        if entry.last.as_ref() != Some(&sameness) {
            entry.last = Some(sameness);
            entry.episode = NEXT_EPISODE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            entry.viewed = false;
        }
        Observation {
            state: entry.state,
            episode: entry.episode,
            viewed: entry.viewed,
        }
    });
    let targeted = signal_targets(rows)
        .get(&row.session_name)
        .is_some_and(|target| target.pane_id == row.pane_id);
    applog(&format!(
        "[agent-status] pi {} s={} target={} v=pi e={}",
        state,
        crate::applog::session_tag(&row.session_name),
        u8::from(targeted),
        observation.episode
    ));
    if targeted {
        crate::notify::observe(&row.session_name, observation);
    }
}

/// A Pi observer lease is over (a reset, a frame that was refused, a
/// timeout, a turned-off capability, another foreground or image): the
/// observation that lease wrote is withdrawn at once, with its episode, and
/// the generation's coverage no longer reads `Connected`. Nothing else is
/// touched: an observation a hook wrote for the same pane stays, another
/// pane's stays, and nothing is reported in its place — a lost observer is
/// not a turn that ended.
pub(crate) fn pi_lost(key: &PaneKey, generation: ForegroundGeneration, lease: u64) {
    #[cfg(test)]
    PI_WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let withdrawn = with_agents(|agents| {
        let owned = agents.get(key).is_some_and(|entry| {
            entry.generation == generation
                && entry
                    .pi
                    .as_ref()
                    .is_some_and(|writer| writer.lease == lease)
        });
        owned
            .then(|| agents.remove(key))
            .flatten()
            .map(|entry| (entry.session, entry.episode))
    });
    with_evidence(|records| {
        if let Some(record) = records.get_mut(key).filter(|r| r.generation == generation) {
            if let Some(seen) = record.pi.as_mut() {
                if seen.coverage == PiCoverage::Connected {
                    seen.coverage = PiCoverage::Unavailable;
                }
            }
        }
    });
    if let Some((session, episode)) = withdrawn {
        crate::notify::withdraw_episode(&session, episode);
    }
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
    // a Pi observer lease whose pane or foreground generation is no longer
    // the one it was bound to ends here, before its observation is judged
    crate::agent_pi::transport::reconcile(rows);
    with_agents(|agents| {
        agents.retain(|key, entry| {
            rows.iter()
                .find(|row| PaneKey::of(row) == *key)
                .is_some_and(|row| {
                    entry.of_pane(row)
                        && entry.reported_by(foreground_generation(table, row.pane_pid))
                            == Some(true)
                })
        });
    });
    with_evidence(|records| {
        records.retain(|key, record| {
            rows.iter()
                .find(|row| PaneKey::of(row) == *key)
                .is_some_and(|row| {
                    row.session_id == record.session_id
                        && row.pane_pid == record.pane_pid
                        && foreground_generation(table, row.pane_pid) == Some(record.generation)
                })
        });
        // what a hello proved of a program that has since executed another
        // image, in the same pid, is proven of nothing that runs there
        for record in records.values_mut() {
            if record.pi.as_ref().is_some_and(|seen| seen.now().is_none()) {
                record.pi = None;
            }
        }
    });
    renotify(rows);
}

/// Bring the notification layer to the projected Signal of every session:
/// a session whose target changed hears its new target's word, a session
/// without one is forgotten.
fn renotify(rows: &[PaneRow]) {
    let projected = projections(rows);
    for (session, observation) in &projected {
        crate::notify::observe(session, *observation);
    }
    crate::notify::retain(&projected.into_keys().collect());
}

#[cfg(test)]
pub(crate) fn reset_for_tests() {
    with_agents(|agents| agents.clear());
    with_evidence(|records| records.clear());
}

// ---------- socket listener --------------------------------------------------

// The accept loop never reads a byte. Every agent's Signal, every re-entry
// receipt and every Pi message arrives on this one socket, and a client that
// connects and then says nothing, or a byte now and then, must not hold any
// of them up. So a connection's FIRST LINE is read by a task of the
// application's async runtime, under an absolute deadline counted from the
// accept (`FIRST_LINE_BUDGET`: a byte that arrives resets nothing), by at
// most `MAX_FIRST_LINES` connections at a time; one more is closed at once,
// unread. A whole first line goes to ONE dispatcher thread over a queue of
// `MAX_DISPATCH`; a full queue closes the connection too. No listener, no
// thread per connection and no unbounded task or buffer is added, and a
// refusal is never a promise that everything is accepted under any load.
//
// Every accepted connection gets its PLACE: a number, counted by the one
// thread that accepts, in the order in which the kernel handed the
// connections over — which is the order in which they were made (xnu puts a
// connection at the end of the listener's queue when its `connect`
// completes, and `accept` takes the first; measured). The place says
// nothing about a client and decides nothing here. The Pi transport uses it
// to tell a connection that was made after it looked at a process from one
// that existed before (`agent_pi::transport`, "Whose bytes").
//
// The dispatcher is what the accept loop used to be after its read, and
// keeps its order for every family: it asks for the Pi family first, then
// for the lifecycle family, then takes the line as a hook word; a hook
// helper is read from the kernel while it is still connected and released
// only then, before the pane listing; a lifecycle receipt keeps its
// connection until the receipt worker settled it, with a deadline counted
// from the instant its line was read. One arrival is handled after the
// other, so an agent's words are committed in the order their lines
// completed — the helper of one word is released before that agent can
// start the next.
//
// What a client sent behind its first line is never read and thrown away.
// All three families are one message per connection — a hook word, a
// lifecycle receipt, a Pi observer's line — so more bytes mean that the
// client did not speak any of them, and the message is refused
// (`trailing-bytes`): for a Pi line this is also what keeps a second
// writer's bytes from riding behind the first (`agent_pi::transport`,
// "Whose bytes").

/// The longest first line of a connection, in bytes.
const MAX_LINE: usize = 8 * 1024;
/// How long a connection has, from its accept, for its whole first line.
const FIRST_LINE_BUDGET: Duration = Duration::from_secs(2);
/// The most connections whose first line is being read at one time.
pub(crate) const MAX_FIRST_LINES: usize = 32;
/// The most connections that were read and wait for the dispatcher.
const MAX_DISPATCH: usize = 32;

/// A connection whose first line was read.
struct Arrival {
    stream: UnixStream,
    /// The connection's place among the accepted ones.
    place: u64,
    line: String,
    /// What was read behind the first line.
    rest: Vec<u8>,
    /// When the line was complete: a lifecycle receipt's budget starts here.
    read_at: std::time::Instant,
}

/// One of the `MAX_FIRST_LINES` slots for a connection that has not
/// delivered its first line yet.
struct FirstLine(Arc<std::sync::atomic::AtomicUsize>);

impl FirstLine {
    fn take(reading: &Arc<std::sync::atomic::AtomicUsize>) -> Option<Self> {
        use std::sync::atomic::Ordering;
        reading
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |held| {
                (held < MAX_FIRST_LINES).then_some(held + 1)
            })
            .ok()
            .map(|_| Self(reading.clone()))
    }
}

impl Drop for FirstLine {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

/// Read one connection's first line, no later than `deadline`. `Err` is the
/// closed reason it is dropped for, or `None` for a client that connected
/// and left without a byte (ordinary, and not worth a line).
async fn first_line(
    stream: UnixStream,
    place: u64,
    deadline: std::time::Instant,
) -> Result<Arrival, Option<&'static str>> {
    let stream = tokio::net::UnixStream::from_std(stream).map_err(|_| None)?;
    let mut buffer = Vec::new();
    let read = async {
        let mut chunk = [0u8; 1024];
        loop {
            if stream.readable().await.is_err() {
                return Ok(false);
            }
            match stream.try_read(&mut chunk) {
                // the client closed its side: what it sent is its line
                Ok(0) => return Ok(false),
                Ok(count) => {
                    buffer.extend_from_slice(&chunk[..count]);
                    if buffer.contains(&b'\n') {
                        return Ok(true);
                    }
                    if buffer.len() > MAX_LINE {
                        return Err(Some("first-line-oversize"));
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => return Ok(false),
            }
        }
    };
    let ended = tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), read)
        .await
        .map_err(|_| Some("first-line-late"))??;
    let read_at = std::time::Instant::now();
    let (line, rest) = match buffer.iter().position(|byte| *byte == b'\n') {
        Some(end) if ended => (buffer[..end].to_vec(), buffer[end + 1..].to_vec()),
        _ => (buffer, Vec::new()),
    };
    if line.is_empty() && rest.is_empty() {
        return Err(None);
    }
    if line.len() > MAX_LINE {
        return Err(Some("first-line-oversize"));
    }
    let line = String::from_utf8(line).map_err(|_| Some("bad-json"))?;
    let stream = stream.into_std().map_err(|_| None)?;
    Ok(Arrival {
        stream,
        place,
        line,
        rest,
        read_at,
    })
}

/// The bounded way from the accept loop to the dispatcher.
struct Intake {
    arrivals: std::sync::mpsc::SyncSender<Arrival>,
    reading: Arc<std::sync::atomic::AtomicUsize>,
    /// How many connections were accepted: the last one's place.
    accepted: std::sync::atomic::AtomicU64,
    drops: Arc<Mutex<Drops>>,
}

impl Intake {
    /// Start the ONE dispatcher thread, which owns the receipt queue and the
    /// Pi service for as long as arrivals can come. A thread that cannot be
    /// started takes the queue's other end with it: every connection is
    /// then refused at `take`.
    fn start(
        drops: Arc<Mutex<Drops>>,
        receipts: Receipts,
        pi: crate::agent_pi::transport::Service,
    ) -> (Self, Option<std::thread::JoinHandle<()>>) {
        let (arrivals, waiting) = std::sync::mpsc::sync_channel::<Arrival>(MAX_DISPATCH);
        let dispatcher = std::thread::Builder::new()
            .name("deck-status-dispatch".into())
            .spawn({
                let drops = drops.clone();
                move || {
                    for arrival in waiting {
                        dispatch(arrival, &drops, &receipts, &pi);
                    }
                }
            })
            .ok();
        let intake = Self {
            arrivals,
            reading: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            accepted: std::sync::atomic::AtomicU64::new(0),
            drops,
        };
        (intake, dispatcher)
    }

    /// An intake whose queue nobody empties: what a dispatcher that is busy
    /// for long looks like to the accept loop.
    #[cfg(test)]
    fn undrained(drops: Arc<Mutex<Drops>>) -> (Self, std::sync::mpsc::Receiver<Arrival>) {
        let (arrivals, waiting) = std::sync::mpsc::sync_channel::<Arrival>(MAX_DISPATCH);
        let intake = Self {
            arrivals,
            reading: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            accepted: std::sync::atomic::AtomicU64::new(0),
            drops,
        };
        (intake, waiting)
    }

    /// Take one accepted connection, in the order the accept loop got them:
    /// it is numbered here, before anything else. Never reads and never
    /// waits: one of the first-line slots is taken or the connection is
    /// closed, and the rest happens in the connection's own task.
    fn take(&self, stream: UnixStream) {
        let place = self
            .accepted
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
            + 1;
        let deadline = std::time::Instant::now() + FIRST_LINE_BUDGET;
        let Some(slot) = FirstLine::take(&self.reading) else {
            note(&self.drops, Some("listener-busy"));
            return;
        };
        if stream.set_nonblocking(true).is_err() {
            return;
        }
        let (arrivals, drops) = (self.arrivals.clone(), self.drops.clone());
        tauri::async_runtime::spawn(async move {
            let arrival = first_line(stream, place, deadline).await;
            // the slot is free before the dispatcher is asked
            drop(slot);
            match arrival {
                Ok(arrival) => {
                    if arrivals.try_send(arrival).is_err() {
                        note(&drops, Some("listener-busy"));
                    }
                }
                Err(Some(reason)) => note(&drops, Some(reason)),
                Err(None) => {}
            }
        });
    }
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
    "terminal-discontinuity",
    "ambiguous-generation",
    "stale-interaction",
    "interaction-mismatch",
    "duplicate-interaction",
    "identity-downgrade",
    // the listener's own bounds, before any family is known
    "listener-busy",
    "first-line-late",
    "first-line-oversize",
    "trailing-bytes",
    // the lifecycle family (`agent_reentry::lifecycle`), same closed log
    "lifecycle-oversize",
    "lifecycle-bad-json",
    "lifecycle-bad-family",
    "lifecycle-bad-version",
    "lifecycle-unknown-source",
    "lifecycle-bad-id",
    "lifecycle-bad-field",
    "lifecycle-no-peer",
    "lifecycle-foreign-helper",
    "lifecycle-foreign-client",
    "lifecycle-foreign-pane",
    "lifecycle-terminal-discontinuity",
    "lifecycle-late",
    "lifecycle-busy",
    "lifecycle-overflow",
    // a hook line that names Pi, and the Pi observer family (`agent_pi`)
    "pi-observer-required",
    "pi-bad-frame",
    "pi-bad-version",
    "pi-bad-mode",
    "pi-unsupported-pi-version",
    "pi-other-server",
    "pi-no-peer",
    "pi-not-foreground",
    "pi-no-such-pane",
    "pi-foreign-pane",
    "pi-terminal-discontinuity",
    "pi-no-generation",
    "pi-generation-mismatch",
    "pi-no-instance",
    "pi-peer-executed",
    "pi-pane-changed",
    "pi-duplicate-observer",
    "pi-capability-disabled",
    "pi-busy",
    "pi-bad-seq",
    "pi-bad-run",
    "pi-bad-lifecycle",
    "pi-bad-native-id",
    "pi-late",
    "pi-stale",
    "pi-no-challenge",
    "pi-sender-mismatch",
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

/// Count one handled line, and log what is due for it.
fn note(drops: &Mutex<Drops>, refused: Option<&'static str>) {
    let lines = drops
        .lock_or_recover()
        .observe(refused, std::time::Instant::now());
    for line in lines {
        applog(&line);
    }
}

/// How many lifecycle receipts may wait for the receipt worker: as many as
/// one lease keeps facts. Each is one accepted connection and one claim; one
/// more is dropped, not queued.
const RECEIPT_QUEUE: usize = 64;

/// One lifecycle receipt on its way to the receipt worker.
struct Receipt {
    claim: crate::agent_reentry::lifecycle::Claim,
    /// Open until the receipt is settled: its close is the helper's ACK.
    stream: UnixStream,
    peer: Option<u32>,
    /// The instant the fact must be written by, counted from the line.
    deadline: std::time::Instant,
}

impl Receipt {
    /// Settle this receipt on the worker: verify and record it, or refuse
    /// it, under its own deadline (`lifecycle::receive`). Whether its claim
    /// is still the operation's — the one it was taken from, the arming it
    /// named, inside its budget and its phase — is asked there, under the
    /// lock that writes, however long the receipt waited in the queue.
    fn settle(self, kernel: &crate::agent_reentry::lifecycle::Kernel) -> Result<(), &'static str> {
        let Receipt {
            claim,
            stream,
            peer,
            deadline,
        } = self;
        let result = {
            let _deadline = crate::session_runtime::Deadline::until(deadline);
            crate::agent_reentry::lifecycle::receive(claim, peer, deadline, kernel)
        };
        // recorded or refused: only now is the helper released
        drop(stream);
        result
    }
}

/// The ONE worker that verifies lifecycle receipts, and the bounded queue in
/// front of it.
///
/// Verifying a receipt ends in a dynamic validation of the client's running
/// code, and that call cannot be interrupted. The status listener is every
/// agent's Signal path, so it never makes that call and never waits for it:
/// it hands the connection over without blocking (`hand`) and goes back to
/// accepting. However long one validation takes, what it holds is this one
/// thread: receipts behind it wait in a queue of `RECEIPT_QUEUE`, a full
/// queue refuses the next one at once, and every receipt that waited past
/// its own deadline is refused unread when the worker reaches it
/// (`lifecycle::receive`). No thread is started per receipt, nothing is
/// retried, and no identity is remembered from one receipt to the next.
struct Receipts {
    queue: std::sync::mpsc::SyncSender<Receipt>,
}

impl Receipts {
    /// Start the worker. Its handle is for an orderly end (tests); dropped,
    /// the thread lives as long as this queue does.
    fn start(drops: Arc<Mutex<Drops>>) -> (Self, Option<std::thread::JoinHandle<()>>) {
        use crate::agent_reentry::lifecycle;
        Self::start_with(move |receipt| {
            note(&drops, receipt.settle(&lifecycle::Kernel::live()).err());
        })
    }

    /// The queue and its one worker, which settles each receipt with
    /// `settle`, one after the other.
    fn start_with(
        settle: impl Fn(Receipt) + Send + 'static,
    ) -> (Self, Option<std::thread::JoinHandle<()>>) {
        let (queue, waiting) = std::sync::mpsc::sync_channel::<Receipt>(RECEIPT_QUEUE);
        // A worker that cannot be started takes the queue's other end with
        // it: every receipt is then refused at `hand`.
        let worker = std::thread::Builder::new()
            .name("deck-lifecycle-receipts".into())
            .spawn(move || {
                for receipt in waiting {
                    settle(receipt);
                }
            })
            .ok();
        (Self { queue }, worker)
    }

    /// Hand one receipt to the worker without waiting for it. A refused
    /// receipt is dropped with its connection: nothing was recorded for it.
    fn hand(&self, receipt: Receipt) -> Result<(), &'static str> {
        self.queue.try_send(receipt).map_err(|_| "lifecycle-busy")
    }
}

/// One connection whose first line was read, on the dispatcher thread.
fn dispatch(
    arrival: Arrival,
    drops: &Mutex<Drops>,
    receipts: &Receipts,
    pi: &crate::agent_pi::transport::Service,
) {
    let Arrival {
        stream,
        place,
        line,
        rest,
        read_at,
    } = arrival;
    // Every family is one message per connection: a client that sent more
    // spoke none of them, and an empty first line is no message at all —
    // bytes behind it are not a later one.
    if !rest.is_empty() {
        note(drops, Some("trailing-bytes"));
        return;
    }
    if line.is_empty() {
        return;
    }
    // A third closed family on the same socket: a Pi observer's message.
    // Verifying it asks the kernel and tmux, so nothing of it is done on
    // this thread: the connection goes to the bounded Pi transport, which
    // never blocks here and answers it. The line reaches neither the hook
    // parser nor the lifecycle mailbox.
    if crate::agent_pi::is_family(&line) {
        pi.hand(stream, line, place);
        return;
    }
    // A separate closed protocol family on the same socket: a lifecycle
    // receipt of the re-entry operation in flight. It never reaches the
    // Signal parser, store or any consumer, and it is not verified on this
    // thread: the connection goes to the receipt worker, which records the
    // fact — or refuses it — while the helper is still connected, and whose
    // close of the stream is the helper's ACK.
    if crate::agent_reentry::lifecycle::is_family(&line) {
        // Without an operation every client's start and end lands here all
        // day long: that ordinary case is dropped without a line.
        if let Some(reason) = lifecycle_receipt(&line, stream, receipts, read_at)
            .err()
            .filter(|reason| *reason != "lifecycle-no-lease")
        {
            note(drops, Some(reason));
        }
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
    note(drops, result.err());
}

/// The production first-line read and the dispatcher's step, one after the
/// other on the calling thread: a test hands ONE connection through both
/// and keeps its own receipt queue and Pi service.
#[cfg(test)]
fn handle_stream(
    stream: UnixStream,
    drops: &Mutex<Drops>,
    receipts: &Receipts,
    pi: &crate::agent_pi::transport::Service,
) {
    stream.set_nonblocking(true).expect("a nonblocking stream");
    let deadline = std::time::Instant::now() + FIRST_LINE_BUDGET;
    // one connection, handed over by itself: it has no place among others
    match tauri::async_runtime::block_on(first_line(stream, 0, deadline)) {
        Ok(arrival) => dispatch(arrival, drops, receipts, pi),
        Err(Some(reason)) => note(drops, Some(reason)),
        Err(None) => {}
    }
}

/// One lifecycle line, as far as the listener itself goes: nothing unless a
/// lease is armed; with one, the closed parse, the lease the line names (one
/// short mailbox lock) and the hand-over. No kernel read and no validation
/// happens here, so this returns in bounded time whatever a client's
/// program does. `Ok` means handed over, not recorded.
fn lifecycle_receipt(
    line: &str,
    stream: UnixStream,
    receipts: &Receipts,
    read_at: std::time::Instant,
) -> Result<(), &'static str> {
    use crate::agent_reentry::lifecycle;
    // the receipt's whole budget starts where its line was read
    let deadline = read_at + lifecycle::RECEIPT_BUDGET;
    if !lifecycle::armed() {
        return Err("lifecycle-no-lease");
    }
    // a source may send receipts only when its adapter's re-entry
    // capability declares what they mean
    let wire = lifecycle::parse(line, |source| {
        crate::extensions::agent(source)
            .and_then(|adapter| adapter.reentry)
            .map(|capability| capability.receipts)
    })?;
    let claim = lifecycle::claim(wire)?;
    let peer = crate::procinfo::peer_pid(&stream);
    receipts.hand(Receipt {
        claim,
        stream,
        peer,
        deadline,
    })
}

/// The Pi transport's way into the bounded drop diagnostics: it names a
/// closed reason, and this module counts and logs it like every other.
fn pi_reporter(drops: &Arc<Mutex<Drops>>) -> crate::agent_pi::transport::Reporter {
    let drops = drops.clone();
    Arc::new(move |reason| note(&drops, Some(reason)))
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

/// What the bounded drop diagnostics counted so far, by closed reason: a
/// test's view of what production writes to app.log.
#[cfg(test)]
#[derive(Clone)]
pub(crate) struct DropCounts(Arc<Mutex<Drops>>);

#[cfg(test)]
impl DropCounts {
    pub(crate) fn new() -> Self {
        Self(Arc::new(Mutex::new(Drops::new(std::time::Instant::now()))))
    }

    pub(crate) fn counts(&self) -> std::collections::BTreeMap<&'static str, u32> {
        self.0.lock_or_recover().counts.clone()
    }

    /// The way a Pi service a test starts itself reports into these counts.
    pub(crate) fn reporter(&self) -> crate::agent_pi::transport::Reporter {
        pi_reporter(&self.0)
    }
}

/// The production accept, first-line read and dispatch on a test socket:
/// the re-entry integration tests feed real helper connections through it.
#[cfg(test)]
pub(crate) fn serve_for_tests(
    listener: UnixListener,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    serve_counting_for_tests(listener, stop, &DropCounts::new());
}

/// `serve_for_tests`, with the drop diagnostics where the caller can read
/// them (the native Pi run, `agent_pi::tests::native`).
#[cfg(test)]
pub(crate) fn serve_counting_for_tests(
    listener: UnixListener,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    counted: &DropCounts,
) {
    let status = listener
        .local_addr()
        .ok()
        .and_then(|address| address.as_pathname().map(Path::to_path_buf))
        .expect("a listener on a path");
    let pi = crate::agent_pi::transport::Service::start(pi_reporter(&counted.0), &status);
    let open = std::sync::atomic::AtomicI64::new(-1);
    serve_pi_for_tests(listener, stop, &open, counted, pi);
}

/// The same with a Pi service the caller started (a test's own tmux server
/// and settings), and a switch. `pass` is how many connections may still be
/// accepted: a negative number is any, and at zero nothing is accepted, so
/// what clients connect and write meanwhile waits in the kernel, unread, in
/// the order they connected — a test decides what has happened by the time
/// Deck reads a byte, and which connection it reads first.
#[cfg(test)]
pub(crate) fn serve_pi_for_tests(
    listener: UnixListener,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pass: &std::sync::atomic::AtomicI64,
    counted: &DropCounts,
    pi: crate::agent_pi::transport::Service,
) {
    listener
        .set_nonblocking(true)
        .expect("nonblocking listener");
    let drops = counted.0.clone();
    let (receipts, worker) = Receipts::start(drops.clone());
    let (intake, dispatcher) = Intake::start(drops, receipts, pi);
    while !stop.load(std::sync::atomic::Ordering::Acquire) {
        let left = pass.load(std::sync::atomic::Ordering::Acquire);
        if left == 0 {
            std::thread::sleep(Duration::from_millis(2));
            continue;
        }
        match listener.accept() {
            Ok((stream, _)) => {
                if left > 0 {
                    pass.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
                }
                intake.take(stream)
            }
            Err(_) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    // An orderly end: a first line still being read arrives or runs out of
    // its budget, the dispatcher handles what it was handed and lets go of
    // the receipt queue, and the worker settles what waits there.
    drop(intake);
    if let Some(dispatcher) = dispatcher {
        let _ = dispatcher.join();
    }
    if let Some(worker) = worker {
        let _ = worker.join();
    }
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
        let drops = Arc::new(Mutex::new(Drops::new(std::time::Instant::now())));
        // one receipt worker, for as long as connections are accepted
        let (receipts, _) = Receipts::start(drops.clone());
        // the one bounded service that answers every Pi observer's message
        let pi = crate::agent_pi::transport::Service::start(pi_reporter(&drops), &path);
        // and the one dispatcher both are asked from; this loop only accepts
        let (intake, _) = Intake::start(drops, receipts, pi);
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => intake.take(stream),
                Err(_) => std::thread::sleep(Duration::from_secs(1)),
            }
        }
    });
}

// ---------- tests -------------------------------------------------------------

/// Tests that touch the process-wide agent store run one at a time, in this
/// module and in any other whose code reaches `reconcile`/`ingest`.
#[cfg(test)]
pub(crate) static STORE_TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// What a client wrote as its first line, read on the test's own thread
    /// (the production read is `first_line`, on the runtime).
    fn read_first_line(stream: &mut UnixStream) -> Option<String> {
        use std::io::BufRead;
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let mut line = String::new();
        std::io::BufReader::new(stream).read_line(&mut line).ok()?;
        Some(line.trim_end_matches('\n').to_string())
    }

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
    fn a_source_word_is_recognized_only_through_the_registered_adapter() {
        let _guard = STORE_TEST_LOCK.lock_or_recover();
        reset_for_tests();
        const FAKE: AgentAdapter = AgentAdapter {
            source: "fake-agent",
            ..CODEX
        };
        let builtin = event_line("turn-done", "%3");
        let fake = builtin.replace("claude-code", "fake-agent");
        // production tables: the two supported words, nothing else
        assert!(parse_event(&builtin).is_ok());
        assert_eq!(parse_event(&fake), Err("unknown-source"));
        assert_eq!(
            agent_hooks_set("fake-agent".into(), true)
                .unwrap_err()
                .message(),
            "unknown agent"
        );
        assert_eq!(
            agent_hooks_set("claude".into(), true)
                .unwrap_err()
                .message(),
            "unknown agent",
            "an executable name is not an adapter"
        );
        {
            let _table = crate::extensions::testing::with_agents(&[FAKE]);
            assert!(
                parse_event(&fake).is_ok(),
                "the parser asks the tables for the word"
            );
            // negative control: a built-in word is not accepted by a
            // hard-coded list once the tables do not carry it
            assert_eq!(parse_event(&builtin), Err("unknown-source"));
            // being registered earns nothing: the same pane, ancestry and
            // generation checks refuse the registered source
            let rows = [row("deck-card-ab12", "$1", "%3", 300, true, "claude")];
            let foreign = with_helper(
                world(&[pane(300, 7, 310, 2000), pane(400, 8, 410, 2000)]),
                420,
                410,
            );
            let ingest_fake = |pane_id: &str, peer: Option<u32>, table: &ProcessTable| {
                let origin = Origin {
                    peer,
                    table: table.clone(),
                };
                ingest(&fake.replace("%3", pane_id), &origin, || {
                    Some(rows.to_vec())
                })
            };
            assert_eq!(ingest_fake("%3", Some(420), &foreign), Err("foreign-pane"));
            assert_eq!(ingest_fake("%9", Some(420), &foreign), Err("no-such-pane"));
            assert_eq!(ingest_fake("%3", None, &foreign), Err("no-peer"));
            assert!(projections(&rows).is_empty(), "refusals store nothing");
            // and it is not remote admission either
            assert_eq!(crate::admission::interactive_agent("fake-agent"), None);
            assert_eq!(crate::admission::channel_agent_command("fake-agent"), None);
        }
        {
            // a doubly registered word is no word
            let _table = crate::extensions::testing::with_agents(&[CODEX, CODEX]);
            assert_eq!(
                parse_event(&builtin.replace("claude-code", "codex")),
                Err("unknown-source")
            );
        }
        // admission reads its own closed list of executables, not sources
        assert_eq!(crate::admission::interactive_agent("claude-code"), None);
        assert_eq!(
            crate::admission::interactive_agent("claude"),
            Some("claude")
        );
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

    /// The runtime shape of a hook (Claude Code 2.1.283, Codex 0.157.0,
    /// probed): the agent spawns it in a fresh session, so the hook's `sh`
    /// (`group`) has no terminal and leads its own group, and the helper
    /// (`group + 1`) is in that group. Returns the helper.
    fn detached_hook(table: &mut ProcessTable, group: u32, parent: u32) -> u32 {
        table.insert(group, process(group, parent, 0, 0, 5000));
        let helper = ProcessInfo {
            pgid: group,
            ..process(group + 1, group, 0, 0, 5000)
        };
        table.insert(helper.pid, helper);
        helper.pid
    }

    /// The blocked shared app-server topology certified in Codex 0.157.1
    /// and 0.160.0, started by the TUI `starter`:
    /// its own group, no terminal.
    fn daemon(table: &mut ProcessTable, pid: u32, starter: u32) {
        table.insert(pid, process(pid, starter, 0, 0, 4000));
    }

    fn signal(rows: &[PaneRow], session: &str) -> Option<(&'static str, EpisodeId)> {
        projections(rows).get(session).map(|o| (o.state, o.episode))
    }

    /// Embedded Codex, Claude and a same-terminal wrapper keep their Signal
    /// with the real detached hook shape; nothing but the one hook group may
    /// be off the pane's terminal.
    #[test]
    fn a_detached_hook_under_the_pane_s_terminal_is_continuous() {
        let _guard = STORE_TEST_LOCK.lock_or_recover();
        reset_for_tests();
        let rows = [row("deck-card-ab12", "$1", "%3", 300, true, "codex")];
        // embedded Codex: TUI 310 → hook sh 330 → helper 331
        let mut embedded = world(&[pane(300, 7, 310, 2000)]);
        let helper = detached_hook(&mut embedded, 330, 310);
        assert_eq!(
            report("working", "%3", Some(helper), &embedded, &rows),
            Ok(())
        );
        // Claude's exec form: the helper alone leads the hook group
        reset_for_tests();
        let mut claude = world(&[pane(300, 7, 310, 2000)]);
        claude.insert(335, process(335, 310, 0, 0, 5000));
        assert_eq!(report("working", "%3", Some(335), &claude, &rows), Ok(()));
        // `caffeinate claude`: wrapper leader 310, agent 311 on the same tty
        reset_for_tests();
        let mut wrapped = world(&[pane(300, 7, 310, 2000)]);
        wrapped.insert(
            311,
            ProcessInfo {
                pgid: 310,
                ..process(311, 310, 7, 310, 2001)
            },
        );
        let helper = detached_hook(&mut wrapped, 330, 311);
        assert_eq!(
            report("working", "%3", Some(helper), &wrapped, &rows),
            Ok(())
        );
        // the hook run must be terminal-less, ONE group, ending at its leader
        let mut other_tty = wrapped.clone();
        other_tty.get_mut(&330).unwrap().tty = 9;
        let mut split_group = wrapped.clone();
        split_group.get_mut(&331).unwrap().pgid = 331;
        let mut no_leader = wrapped.clone();
        no_leader.get_mut(&330).unwrap().pgid = 331;
        no_leader.get_mut(&331).unwrap().pgid = 331;
        let mut gap = wrapped.clone();
        gap.get_mut(&311).unwrap().tty = 0; // the agent itself left the terminal
        for broken in [other_tty, split_group, no_leader, gap] {
            reset_for_tests();
            assert_eq!(
                report("working", "%3", Some(331), &broken, &rows),
                Err("terminal-discontinuity")
            );
            assert_eq!(signal(&rows, "deck-card-ab12"), None);
        }
        reset_for_tests();
    }

    /// The blocked shared app-server topology, as probed: every client's hook
    /// is spawned by the daemon and inherits the STARTER's `$TMUX_PANE`, so
    /// all of them name the starter's pane — and pass `foreign-pane` and the
    /// generation check while the starter TUI lives there. Terminal
    /// continuity refuses them all; the starter's own earlier observation is
    /// withdrawn (the channel is now ambiguous) and no other card gains one.
    #[test]
    fn shared_codex_daemon_hooks_are_refused_and_contaminate_no_pane() {
        let _guard = STORE_TEST_LOCK.lock_or_recover();
        reset_for_tests();
        crate::notify::reset_for_tests();
        let rows = [
            row("deck-card-aaaa", "$1", "%3", 300, true, "codex"),
            row("deck-card-bbbb", "$2", "%4", 400, true, "codex"),
        ];
        let panes = world(&[pane(300, 7, 310, 2000), pane(400, 8, 410, 2000)]);

        // A started the daemon; A's own hook comes through it
        let mut from_a = panes.clone();
        daemon(&mut from_a, 350, 310);
        let a_hook = detached_hook(&mut from_a, 360, 350);
        assert!(
            ancestry_contains(&from_a, a_hook, 300),
            "passes foreign-pane"
        );
        assert_eq!(
            codex_report("working", "%3", Some(a_hook), &from_a, &rows),
            Err("terminal-discontinuity")
        );
        assert_eq!(signal(&rows, "deck-card-aaaa"), None);

        // the latent misattribution: A holds a proven observation (an
        // embedded-shaped hook of the same generation); B's turn, spawned by
        // A's daemon, claims A's inherited pane and must not become A's
        // (a fresh start: A's own daemon hook above already made this
        // generation ambiguous, which would refuse the seed)
        reset_for_tests();
        let mut seeded = panes.clone();
        let own = detached_hook(&mut seeded, 330, 310);
        assert_eq!(
            codex_report("needs-input", "%3", Some(own), &seeded, &rows),
            Ok(())
        );
        let before = signal(&rows, "deck-card-aaaa");
        let notified_before = notified("deck-card-aaaa");
        let b_hook = detached_hook(&mut from_a, 370, 350);
        for word in ["working", "turn-done"] {
            assert_eq!(
                codex_report(word, "%3", Some(b_hook), &from_a, &rows),
                Err("terminal-discontinuity")
            );
        }
        assert!(before.is_some() && notified_before.is_some());
        assert_eq!(
            signal(&rows, "deck-card-aaaa"),
            None,
            "withdrawn, never B's word"
        );
        assert_eq!(
            notified("deck-card-aaaa"),
            None,
            "no attention attributed to A"
        );
        assert_eq!(signal(&rows, "deck-card-bbbb"), None);
        assert_eq!(notified("deck-card-bbbb"), None);

        // the daemon restarted from B: now EVERY hook names B's pane
        reset_for_tests();
        let mut from_b = panes.clone();
        daemon(&mut from_b, 450, 410);
        let a_again = detached_hook(&mut from_b, 460, 450);
        let b_again = detached_hook(&mut from_b, 470, 450);
        for helper in [a_again, b_again] {
            assert_eq!(
                codex_report("turn-done", "%4", Some(helper), &from_b, &rows),
                Err("terminal-discontinuity")
            );
        }
        assert_eq!(
            signal(&rows, "deck-card-bbbb"),
            None,
            "B is not contaminated"
        );
        assert_eq!(signal(&rows, "deck-card-aaaa"), None);
        // and a daemon-inherited claim of the OTHER pane is still foreign
        assert_eq!(
            codex_report("turn-done", "%3", Some(a_again), &from_b, &rows),
            Err("foreign-pane")
        );
        reset_for_tests();
    }

    /// `report` with a Codex-sourced line.
    fn codex_report(
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
        let line = event_line(state, pane).replace("\"claude-code\"", "\"codex\"");
        ingest(&line, &origin, || Some(rows.to_vec()))
    }

    /// Every session's Codex trust against the snapshot `table`.
    fn trust(rows: &[PaneRow], table: &ProcessTable) -> HashMap<String, CodexSignalTrust> {
        generation_evidence(rows, |pane| foreground_generation(table, pane), |_| None)
            .into_iter()
            .filter_map(|(session, evidence)| evidence.codex.map(|trust| (session, trust)))
            .collect()
    }

    /// The scheduler's automatic hold for an owner row of `session`, from
    /// the Signal projection and Codex trust against `table`.
    fn owner_held(rows: &[PaneRow], table: &ProcessTable, session: &str) -> bool {
        let owner: crate::scheduler::QueueItem = serde_json::from_value(serde_json::json!({
            "id": "o", "session": session, "card_id": "c", "dir": "", "cmd": "",
            "text": "x", "mode": "chain", "added": 0
        }))
        .unwrap();
        let seen = crate::scheduler::Observed {
            authority_unverified: false,
            board_unverified: false,
            channel_unverified: false,
            activity: 0,
            agent: projections(rows).get(session).map(|o| o.state),
            codex: trust(rows, table).get(session).copied(),
            claude_interaction: false,
            pi: false,
        };
        crate::scheduler::agent_holds(&owner, Some(&seen))
    }

    /// Codex trust per foreground generation only degrades toward safety:
    /// an accepted Codex hook proves Trusted; a discontinuity whose ancestry
    /// reaches the pane's own generation makes it Unavailable, over an
    /// earlier Trusted, and clears the generation's Codex observation; no
    /// later hook heals it; a new process starts Unknown again. Only Codex
    /// is gated, and Claude-sourced events neither set nor obey it.
    #[test]
    fn codex_trust_is_proven_per_generation_and_only_degrades() {
        use CodexSignalTrust::{Trusted, Unavailable, Unknown};
        let _guard = STORE_TEST_LOCK.lock_or_recover();
        reset_for_tests();
        crate::notify::reset_for_tests();
        let rows = [row("deck-card-aaaa", "$1", "%3", 300, true, "codex")];
        let mut table = world(&[pane(300, 7, 310, 2000)]);
        // a fresh process: no evidence, held although quiet with no word
        assert_eq!(trust(&rows, &table).get("deck-card-aaaa"), Some(&Unknown));
        assert!(owner_held(&rows, &table, "deck-card-aaaa"));
        // an accepted hook that is not Codex's proves nothing about Codex
        let other = detached_hook(&mut table, 320, 310);
        assert_eq!(
            report("turn-done", "%3", Some(other), &table, &rows),
            Ok(())
        );
        assert_eq!(trust(&rows, &table).get("deck-card-aaaa"), Some(&Unknown));
        reset_for_tests();
        // an accepted embedded Codex hook: Trusted, existing semantics
        let helper = detached_hook(&mut table, 330, 310);
        assert_eq!(
            codex_report("turn-done", "%3", Some(helper), &table, &rows),
            Ok(())
        );
        assert_eq!(trust(&rows, &table).get("deck-card-aaaa"), Some(&Trusted));
        assert!(
            !owner_held(&rows, &table, "deck-card-aaaa"),
            "turn-done: quiet rule"
        );
        assert_eq!(notified("deck-card-aaaa"), Some("turn-done"));
        // Trusted → later discontinuity, same generation → Unavailable: the
        // turn-done stops presenting (Signal, attention, notification) and
        // automatic rows are held
        daemon(&mut table, 350, 310);
        let through = detached_hook(&mut table, 360, 350);
        assert_eq!(
            codex_report("working", "%3", Some(through), &table, &rows),
            Err("terminal-discontinuity")
        );
        assert_eq!(
            trust(&rows, &table).get("deck-card-aaaa"),
            Some(&Unavailable)
        );
        assert_eq!(signal(&rows, "deck-card-aaaa"), None);
        assert_eq!(notified("deck-card-aaaa"), None, "no stale attention");
        assert!(owner_held(&rows, &table, "deck-card-aaaa"));
        // Unavailable → a later, otherwise admissible Codex hook of the same
        // generation is refused and heals nothing
        let again = detached_hook(&mut table, 340, 310);
        assert_eq!(
            codex_report("turn-done", "%3", Some(again), &table, &rows),
            Err("ambiguous-generation")
        );
        assert_eq!(
            trust(&rows, &table).get("deck-card-aaaa"),
            Some(&Unavailable)
        );
        assert_eq!(signal(&rows, "deck-card-aaaa"), None);
        assert!(owner_held(&rows, &table, "deck-card-aaaa"));
        // a new Codex process in the pane: Unknown again, even before a
        // Board poll reconciles, and still held until it proves itself
        let mut relaunched = world(&[pane(300, 7, 311, 3000)]);
        assert_eq!(
            trust(&rows, &relaunched).get("deck-card-aaaa"),
            Some(&Unknown)
        );
        assert!(owner_held(&rows, &relaunched, "deck-card-aaaa"));
        let fresh = detached_hook(&mut relaunched, 370, 311);
        assert_eq!(
            codex_report("working", "%3", Some(fresh), &relaunched, &rows),
            Ok(())
        );
        assert_eq!(
            trust(&rows, &relaunched).get("deck-card-aaaa"),
            Some(&Trusted)
        );
        // reconciliation drops a proof whose generation is gone
        reconcile(&rows, &world(&[pane(300, 7, 312, 4000)]));
        assert!(with_evidence(|records| records.is_empty()));
        reset_for_tests();
        crate::notify::reset_for_tests();
    }

    /// A Claude foreground is never gated and a Codex-sourced discontinuity
    /// never clears a Claude observation.
    #[test]
    fn claude_is_unchanged_by_codex_trust() {
        let _guard = STORE_TEST_LOCK.lock_or_recover();
        reset_for_tests();
        let rows = [row("deck-card-aaaa", "$1", "%3", 300, true, "claude")];
        let mut table = world(&[pane(300, 7, 310, 2000)]);
        let hook = detached_hook(&mut table, 330, 310);
        assert_eq!(
            report("needs-input", "%3", Some(hook), &table, &rows),
            Ok(())
        );
        assert!(trust(&rows, &table).is_empty(), "no Codex gate");
        // Codex's daemon nested under the Claude pane (e.g. run by a tool)
        daemon(&mut table, 350, 310);
        let nested = detached_hook(&mut table, 360, 350);
        assert_eq!(
            codex_report("working", "%3", Some(nested), &table, &rows),
            Err("terminal-discontinuity")
        );
        assert_eq!(
            signal(&rows, "deck-card-aaaa").map(|s| s.0),
            Some("needs-input")
        );
        let later = detached_hook(&mut table, 340, 310);
        assert_eq!(
            report("turn-done", "%3", Some(later), &table, &rows),
            Ok(())
        );
        assert_eq!(
            signal(&rows, "deck-card-aaaa").map(|s| s.0),
            Some("turn-done")
        );
        reset_for_tests();
    }

    /// Codex launched through `node` or a wrapper: tmux does not name the
    /// foreground `codex`, yet a proof for its current generation is still
    /// what the scheduler sees.
    #[test]
    fn a_trust_proof_is_visible_whatever_the_foreground_name() {
        use CodexSignalTrust::Trusted;
        let _guard = STORE_TEST_LOCK.lock_or_recover();
        reset_for_tests();
        let rows = [row("deck-card-aaaa", "$1", "%3", 300, true, "node")];
        let mut table = world(&[pane(300, 7, 310, 2000)]);
        assert!(
            trust(&rows, &table).is_empty(),
            "no proof, no name: the row config decides"
        );
        let hook = detached_hook(&mut table, 330, 310);
        assert_eq!(
            codex_report("turn-done", "%3", Some(hook), &table, &rows),
            Ok(())
        );
        assert_eq!(trust(&rows, &table).get("deck-card-aaaa"), Some(&Trusted));
        // an owner row configured for Codex resumes its normal semantics
        let owner: crate::scheduler::QueueItem = serde_json::from_value(serde_json::json!({
            "id": "o", "session": "deck-card-aaaa", "card_id": "c", "dir": "", "cmd": "codex",
            "text": "x", "mode": "chain", "added": 0, "expected_process": "codex"
        }))
        .unwrap();
        let seen = |codex| crate::scheduler::Observed {
            authority_unverified: false,
            board_unverified: false,
            channel_unverified: false,
            activity: 0,
            agent: Some("turn-done"),
            codex,
            claude_interaction: false,
            pi: false,
        };
        assert!(!crate::scheduler::agent_holds(
            &owner,
            Some(&seen(Some(Trusted)))
        ));
        assert!(crate::scheduler::agent_holds(&owner, Some(&seen(None))));
        reset_for_tests();
    }

    /// The demonstrated multi-client misattribution cannot reach another
    /// pane: B's hooks run inside A's daemon and name A's pane; the only pane
    /// they can ever mark is the one their ancestry proves (A, which started
    /// the daemon) — where the ambiguity correctly dominates A's own proof —
    /// and B's own pane gains nothing.
    #[test]
    fn a_shared_daemon_moves_no_other_pane_s_trust_or_hold() {
        use CodexSignalTrust::{Trusted, Unavailable, Unknown};
        let _guard = STORE_TEST_LOCK.lock_or_recover();
        reset_for_tests();
        crate::notify::reset_for_tests();
        let rows = [
            row("deck-card-aaaa", "$1", "%3", 300, true, "codex"),
            row("deck-card-bbbb", "$2", "%4", 400, true, "codex"),
            row("deck-card-cccc", "$3", "%5", 500, true, "codex"),
        ];
        let mut table = world(&[
            pane(300, 7, 310, 2000),
            pane(400, 8, 410, 2000),
            pane(500, 9, 510, 2000),
        ]);
        // C is an unrelated, proven embedded Codex
        let c_hook = detached_hook(&mut table, 520, 510);
        assert_eq!(
            codex_report("turn-done", "%5", Some(c_hook), &table, &rows),
            Ok(())
        );
        // A proved itself, then started a daemon that B attached to
        let own = detached_hook(&mut table, 330, 310);
        assert_eq!(
            codex_report("turn-done", "%3", Some(own), &table, &rows),
            Ok(())
        );
        daemon(&mut table, 350, 310);
        for (group, word) in [(360, "working"), (370, "needs-input"), (380, "turn-done")] {
            let b_hook = detached_hook(&mut table, group, 350);
            assert_eq!(
                codex_report(word, "%3", Some(b_hook), &table, &rows),
                Err("terminal-discontinuity")
            );
        }
        let seen = trust(&rows, &table);
        assert_eq!(
            seen.get("deck-card-aaaa"),
            Some(&Unavailable),
            "the ambiguity dominates"
        );
        assert_eq!(
            seen.get("deck-card-bbbb"),
            Some(&Unknown),
            "B gains nothing"
        );
        assert_eq!(seen.get("deck-card-cccc"), Some(&Trusted), "C untouched");
        for held in ["deck-card-aaaa", "deck-card-bbbb"] {
            assert!(owner_held(&rows, &table, held), "{held}");
        }
        assert!(!owner_held(&rows, &table, "deck-card-cccc"));
        // no Signal, attention or notification lands on A or B; C keeps its own
        assert_eq!(
            signal(&rows, "deck-card-aaaa"),
            None,
            "B's needs-input is not A's"
        );
        assert_eq!(signal(&rows, "deck-card-bbbb"), None);
        assert_eq!(
            signal(&rows, "deck-card-cccc").map(|s| s.0),
            Some("turn-done")
        );
        assert_eq!(notified("deck-card-aaaa"), None);
        assert_eq!(notified("deck-card-bbbb"), None);
        assert_eq!(notified("deck-card-cccc"), Some("turn-done"));

        // the daemon restarted from B: every hook names B; B started it, so
        // B's generation is the one proven Unavailable; A's new process is
        // untouched (Unknown until it proves itself)
        reset_for_tests();
        let mut table = world(&[pane(300, 7, 311, 3000), pane(400, 8, 410, 2000)]);
        daemon(&mut table, 450, 410);
        for group in [460, 470] {
            let hook = detached_hook(&mut table, group, 450);
            assert_eq!(
                codex_report("needs-input", "%4", Some(hook), &table, &rows),
                Err("terminal-discontinuity")
            );
        }
        let seen = trust(&rows, &table);
        assert_eq!(seen.get("deck-card-bbbb"), Some(&Unavailable));
        assert_eq!(seen.get("deck-card-aaaa"), Some(&Unknown));
        assert!(projections(&rows).is_empty(), "no Signal anywhere");
        reset_for_tests();
        crate::notify::reset_for_tests();
    }

    /// The scheduler's view of one session, as `scheduler::observe_with`
    /// builds it against `table`.
    fn observed(
        rows: &[PaneRow],
        table: &ProcessTable,
        session: &str,
    ) -> crate::scheduler::Observed {
        crate::scheduler::observe_with(
            rows.to_vec(),
            |pane| foreground_generation(table, pane),
            |_| None,
        )
        .remove(session)
        .expect("listed session")
    }

    fn claude_row_held(rows: &[PaneRow], table: &ProcessTable, session: &str) -> bool {
        let row: crate::scheduler::QueueItem = serde_json::from_value(serde_json::json!({
            "id": "o", "session": session, "card_id": "c", "dir": "", "cmd": "claude",
            "text": "x", "mode": "chain", "added": 0, "expected_process": "claude"
        }))
        .unwrap();
        crate::scheduler::agent_holds(&row, Some(&observed(rows, table, session)))
    }

    /// `AgentInteractionEstablished` for Claude: an ACCEPTED interaction word
    /// of the exact current foreground generation. A new generation and a
    /// Deck restart (the in-memory store reset) start without it; a manual
    /// send through the scheduler never writes it — only hook admission does.
    #[test]
    fn claude_interaction_evidence_is_per_generation_and_only_from_hooks() {
        let _guard = STORE_TEST_LOCK.lock_or_recover();
        reset_for_tests();
        let rows = [row("deck-card-aaaa", "$1", "%3", 300, true, "claude")];
        let mut gen_a = world(&[pane(300, 7, 310, 2000)]);
        // a live Claude with no accepted interaction (startup dialog, fresh
        // process, hooks off): held
        assert!(!observed(&rows, &gen_a, "deck-card-aaaa").claude_interaction);
        assert!(claude_row_held(&rows, &gen_a, "deck-card-aaaa"));
        // a delivery through the scheduler (the path send-now shares) writes
        // no interaction evidence: only hook admission does
        let item: crate::scheduler::QueueItem = serde_json::from_value(serde_json::json!({
            "id": "o", "session": "deck-card-aaaa", "card_id": "c", "dir": "", "cmd": "claude",
            "text": "x", "mode": "at", "at": 1, "added": 0, "expected_process": "claude"
        }))
        .unwrap();
        let queue: crate::scheduler::QueueState =
            serde_json::from_value(serde_json::json!({ "items": [item], "last_fired": {} }))
                .unwrap();
        let queue = Mutex::new(queue);
        let ready = || crate::context::ProbeResult {
            status: crate::context::ContextStatus::Ready,
            code: crate::context::ContextCode::ProcessMatched,
            identity: Some(crate::context::PaneIdentity {
                server_pid: 42,
                session_id: "$1".into(),
                window_id: "@1".into(),
                pane_id: "%3".into(),
                pane_pid: 300,
            }),
            current_process: Some("claude".into()),
        };
        let fired = std::sync::atomic::AtomicBool::new(false);
        let mut established = observed(&rows, &gen_a, "deck-card-aaaa");
        established.claude_interaction = true; // let the send through
        let sent = crate::scheduler::send_one_safe(
            &queue,
            &std::sync::atomic::AtomicBool::new(false),
            "deck-card-aaaa",
            720,
            &HashMap::from([("deck-card-aaaa".to_string(), established)]),
            &crate::scheduler::SendHooks {
                fire: &|_| {
                    fired.store(true, std::sync::atomic::Ordering::SeqCst);
                    Ok(())
                },
                persist: &|_| Ok(()),
                kill: &|_| {},
                board: &|| None,
                authority: &|| None,
            },
            &crate::scheduler::ContextHooks {
                prepare: &|_, _| crate::scheduler::Prepared::Probe(ready()),
                final_probe: &|_| ready(),
            },
        );
        assert!(fired.load(std::sync::atomic::Ordering::SeqCst), "{sent:?}");
        assert!(!observed(&rows, &gen_a, "deck-card-aaaa").claude_interaction);
        assert!(claude_row_held(&rows, &gen_a, "deck-card-aaaa"));
        // a Codex-sourced accepted hook is not Claude evidence
        let codex_hook = detached_hook(&mut gen_a, 320, 310);
        assert_eq!(
            codex_report("turn-done", "%3", Some(codex_hook), &gen_a, &rows),
            Ok(())
        );
        assert!(!observed(&rows, &gen_a, "deck-card-aaaa").claude_interaction);
        reset_for_tests();
        // an accepted Claude interaction establishes it; ordinary rules resume
        let hook = detached_hook(&mut gen_a, 330, 310);
        assert_eq!(report("turn-done", "%3", Some(hook), &gen_a, &rows), Ok(()));
        assert!(observed(&rows, &gen_a, "deck-card-aaaa").claude_interaction);
        assert!(!claude_row_held(&rows, &gen_a, "deck-card-aaaa"));
        // generation B (same executable name, new process) inherits nothing
        let gen_b = world(&[pane(300, 7, 311, 3000)]);
        assert!(!observed(&rows, &gen_b, "deck-card-aaaa").claude_interaction);
        assert!(claude_row_held(&rows, &gen_b, "deck-card-aaaa"));
        // a Deck restart: the still-running generation A has no evidence
        // until a new interaction arrives (nothing is persisted)
        reset_for_tests();
        assert!(claude_row_held(&rows, &gen_a, "deck-card-aaaa"));
        let again = detached_hook(&mut gen_a, 340, 310);
        assert_eq!(report("working", "%3", Some(again), &gen_a, &rows), Ok(()));
        assert!(!claude_row_held(&rows, &gen_a, "deck-card-aaaa"));
        // a refused event proves nothing
        reset_for_tests();
        let foreign = with_helper(
            world(&[pane(300, 7, 310, 2000), pane(400, 8, 410, 2000)]),
            420,
            410,
        );
        assert_eq!(
            report("turn-done", "%3", Some(420), &foreign, &rows),
            Err("foreign-pane")
        );
        assert!(!observed(&rows, &foreign, "deck-card-aaaa").claude_interaction);
        reset_for_tests();
    }

    /// Tripwire: every accepted Claude word counts as interaction evidence
    /// today because every word comes from an interaction-level hook. A new
    /// word (e.g. a future startup/session hook) must be classified here on
    /// purpose, never counted automatically.
    #[test]
    fn claude_interaction_words_are_a_reviewed_closed_list() {
        assert_eq!(
            CLAUDE_INTERACTION_WORDS, STATES,
            "classify the new agent word: is it real interaction evidence?"
        );
    }

    fn ancestry_contains(table: &ProcessTable, pid: u32, want: u32) -> bool {
        crate::procinfo::ancestry_in(table, pid, ORIGIN_HOPS).contains(&want)
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
            Some(g) if !chain.contains(&g.pid) => Err("generation-mismatch"),
            Some(g) if !terminal_continuous(&origin.table, &chain, g.pid, me) => {
                Err("terminal-discontinuity")
            }
            Some(_) => Ok(()),
        };
        assert_eq!(ingest(&read, &origin, || Some(pane_is(me))), expected);
        drop(listener);
        let _ = std::fs::remove_dir_all(&dir);
        reset_for_tests();
    }

    /// A throwaway bundled-tmux server for the real-process tests:
    /// (socket name, tmux binary, socket path captured while alive)
    struct Server(String, PathBuf, Option<PathBuf>);
    impl Server {
        fn run(&self, args: &[&str]) -> String {
            let out = std::process::Command::new(&self.1)
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
                    && std::fs::symlink_metadata(&path)
                        .is_ok_and(|m| std::os::unix::fs::FileTypeExt::is_socket(&m.file_type()))
                {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
    }

    /// A fresh throwaway server and a status listener in its own temp dir:
    /// (server, listener, dir, listener socket path).
    fn throwaway(kind: &str) -> (Server, UnixListener, PathBuf, PathBuf) {
        let bin =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("binaries/tmux-aarch64-apple-darwin");
        // one per run and per call: the sequence keeps a second use of this
        // fixture in the same process off the first one's socket and server
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("deck-status-{kind}-{}-{seq}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("status.sock");
        let listener = listen_at(&path).unwrap();
        let server = Server(
            format!("deck-test-status-{}-{seq}", std::process::id()),
            bin,
            None,
        );
        (server, listener, dir, path)
    }

    /// End to end against a throwaway tmux server: a client started INSIDE
    /// the pane (a descendant of `#{pane_pid}`) is accepted, the same line
    /// from this test process (outside the pane) is refused.
    #[test]
    fn a_pane_s_own_descendant_reports_and_an_outsider_is_refused() {
        let _guard = STORE_TEST_LOCK.lock_or_recover();
        reset_for_tests();
        let (mut server, listener, dir, path) = throwaway("e2e");
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

    /// Real processes, real `setsid()`: two panes of a throwaway server run
    /// a stand-in agent (perl, the pane's own foreground program). Each
    /// reports the way Claude Code 2.1.283 and Codex 0.157.0 were probed to
    /// spawn hooks — in a fresh terminal-less session — and builds its line
    /// from the INHERITED `$TMUX`/`$TMUX_PANE`, like the helper. Pane `emb`
    /// spawns the hook itself (embedded Codex): accepted. Pane `dmn` first
    /// starts a detached "daemon" that spawns the hook: the hook names the
    /// starter's own pane and its chain reaches that pane and its foreground
    /// leader, yet the daemon breaks terminal continuity: refused.
    #[test]
    fn a_real_detached_daemon_breaks_terminal_continuity() {
        let _guard = STORE_TEST_LOCK.lock_or_recover();
        reset_for_tests();
        let (mut server, listener, dir, path) = throwaway("tty");
        let script = dir.join("agent.pl");
        std::fs::write(
            &script,
            r#"use POSIX (); use IO::Socket::UNIX;
            my ($mode, $sock, $name) = @ARGV;
            sub hook {
              POSIX::setsid();
              my (undef, $server) = split /,/, $ENV{TMUX};
              my $s = IO::Socket::UNIX->new(Peer => $sock) or exit 1;
              print $s qq({"v":1,"source":"codex","state":"working","socket":"$name","server_pid":$server,"pane":"$ENV{TMUX_PANE}"}\n);
              1 while <$s>;
              exit 0;
            }
            if (fork() == 0) {
              if ($mode eq "daemon") { POSIX::setsid(); if (fork() == 0) { hook() } wait(); exit 0 }
              hook();
            }
            sleep 30;"#,
        )
        .unwrap();
        let agent = |mode: &str| {
            format!(
                "exec /usr/bin/perl '{}' {mode} '{}' {}",
                script.display(),
                path.display(),
                crate::tmux::socket()
            )
        };
        let (embedded, daemon) = (agent("embedded"), agent("daemon"));
        server.run(&[
            "start-server",
            ";",
            "new-session",
            "-d",
            "-s",
            "emb",
            "-x",
            "80",
            "-y",
            "12",
            &embedded,
            ";",
            "new-session",
            "-d",
            "-s",
            "dmn",
            "-x",
            "80",
            "-y",
            "12",
            &daemon,
        ]);
        let socket = server.run(&["display-message", "-p", "#{socket_path}"]);
        server.2 = Some(PathBuf::from(socket.trim()));
        listener.set_nonblocking(true).unwrap();
        let mut reports = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while reports.len() < 2 {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false).unwrap();
                    let line = read_first_line(&mut stream).unwrap();
                    // identity read while the reporter is still connected
                    let origin = Origin {
                        peer: crate::procinfo::peer_pid(&stream),
                        table: crate::procinfo::processes(),
                    };
                    reports.push((line, origin));
                }
                Err(_) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                Err(e) => panic!("both panes report: {e}"),
            }
        }
        let rows: Vec<PaneRow> = server
            .run(&["list-panes", "-a", "-F", crate::tmux::PANE_FORMAT])
            .lines()
            .filter_map(crate::tmux::parse_pane_row)
            .collect();
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert!(rows.iter().all(|r| r.command == "perl"), "{rows:?}");
        for (line, origin) in &reports {
            let row = rows
                .iter()
                .find(|r| line.contains(&format!("\"pane\":\"{}\"", r.pane_id)))
                .expect("the hook named one of the panes");
            let chain = crate::procinfo::ancestry_in(&origin.table, origin.peer.unwrap(), 32);
            let hook = &origin.table[&chain[0]];
            assert_eq!((hook.tty, hook.pgid), (0, hook.pid), "a detached hook");
            // both pass the pre-existing rules for the pane they name
            assert!(chain.contains(&row.pane_pid), "not foreign");
            let generation = foreground_generation(&origin.table, row.pane_pid).unwrap();
            assert!(chain.contains(&generation.pid), "the pane's generation");
            let want = if row.session_name == "emb" {
                Ok(())
            } else {
                Err("terminal-discontinuity")
            };
            assert_eq!(
                ingest(line, origin, || Some(rows.clone())),
                want,
                "{}",
                row.session_name
            );
        }
        let states = projections(&rows);
        assert_eq!(states.get("emb").map(|o| o.state), Some("working"));
        assert_eq!(states.get("dmn").map(|o| o.state), None);
        drop(reports);
        drop(server);
        drop(listener);
        let _ = std::fs::remove_dir_all(&dir);
        reset_for_tests();
    }

    /// The hand-over to the receipt worker never waits and never grows.
    /// While the one worker is inside a verification that does not return,
    /// exactly `RECEIPT_QUEUE` further receipts wait; the next one is
    /// refused at once and its connection is closed; nothing else runs
    /// beside the one verification. When it returns, the same worker settles
    /// the queue in order.
    #[test]
    fn the_receipt_queue_is_bounded_and_the_hand_over_never_waits() {
        use crate::agent_reentry::lifecycle;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Instant;
        let _lock = crate::agent_reentry::TEST_LOCK.lock_or_recover();
        crate::agent_reentry::reset_for_tests();
        lifecycle::begin("deck", Path::new("/nonexistent/deck-lifecycle-helper"));
        lifecycle::arm(
            0,
            lifecycle::Lease::exit(
                4242,
                "%7",
                (1, (1, 1)),
                (2, (2, 2)),
                crate::procinfo::ExecutableRef::of_this_test(),
            ),
        );
        let line = concat!(
            r#"{"protocol":"agent-lifecycle","v":1,"source":"claude-code","#,
            r#""event":"session-end","nativeSessionId":"aaaaaaaa-0000-4000-8000-000000000001","#,
            r#""endReason":"prompt-input-exit","socket":"deck","server_pid":4242,"pane":"%7"}"#
        );
        // a receipt and the helper's end of its connection
        let receipt = || {
            let (ours, theirs) = UnixStream::pair().expect("a connection");
            // set while both ends are open: macOS refuses it afterwards
            theirs
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("a read timeout");
            let wire = lifecycle::parse(line, |_| Some(&crate::agent_reentry::claude::RECEIPTS))
                .expect("a valid line");
            let receipt = Receipt {
                claim: lifecycle::claim(wire).expect("the armed lease"),
                stream: ours,
                peer: None,
                deadline: Instant::now() + Duration::from_secs(60),
            };
            (receipt, theirs)
        };
        let (entered, at_work) = std::sync::mpsc::channel();
        let (release, held) = std::sync::mpsc::channel::<()>();
        let settled = Arc::new(AtomicUsize::new(0));
        let threads = Arc::new(Mutex::new(std::collections::HashSet::new()));
        let (receipts, worker) = Receipts::start_with({
            let (settled, threads) = (settled.clone(), threads.clone());
            let held = Mutex::new(held);
            move |receipt| {
                let _ = entered.send(());
                // does not return until the test lets it
                let _ = held.lock_or_recover().recv();
                threads
                    .lock_or_recover()
                    .insert(std::thread::current().id());
                settled.fetch_add(1, Ordering::SeqCst);
                drop(receipt);
            }
        });
        let receipts = Arc::new(receipts);
        let mut helpers = Vec::new();
        let (first, helper) = receipt();
        helpers.push(helper);
        assert_eq!(receipts.hand(first), Ok(()));
        at_work
            .recv_timeout(Duration::from_secs(5))
            .expect("the worker took the first receipt");
        for _ in 0..RECEIPT_QUEUE {
            let (queued, helper) = receipt();
            helpers.push(helper);
            assert_eq!(receipts.hand(queued), Ok(()), "a waiting receipt");
        }
        // one more than the queue holds: asked from another thread, so that
        // a hand-over that waited would be seen instead of hanging the test
        let (extra, mut refused) = receipt();
        let (answered, answer) = std::sync::mpsc::channel();
        let asking = std::thread::spawn({
            let receipts = receipts.clone();
            move || {
                let _ = answered.send(receipts.hand(extra));
            }
        });
        let verdict = answer.recv_timeout(Duration::from_secs(2));
        assert_eq!(
            verdict,
            Ok(Err("lifecycle-busy")),
            "a full queue refuses at once; the hand-over never waits"
        );
        assert_eq!(
            std::io::Read::read(&mut refused, &mut [0u8; 1]).expect("the refused connection"),
            0,
            "a refused receipt's connection is closed"
        );
        assert_eq!(
            settled.load(Ordering::SeqCst),
            0,
            "nothing ran beside the one verification"
        );
        // the verification returns: the same worker settles what waited
        drop(release);
        asking.join().expect("the hand-over thread ended");
        let receipts = Arc::try_unwrap(receipts)
            .ok()
            .expect("the hand-over thread is done");
        drop(receipts);
        worker
            .expect("the worker was started")
            .join()
            .expect("the worker ended");
        assert_eq!(settled.load(Ordering::SeqCst), RECEIPT_QUEUE + 1);
        assert_eq!(threads.lock_or_recover().len(), 1, "one worker thread");
        drop(helpers);
        lifecycle::end();
    }

    /// The production receipt worker (`Receipts`, `Receipt::settle`,
    /// `lifecycle::receive`) on a kernel that says yes to everything about
    /// the certified topology, and whose every validation waits until the
    /// test lets it answer: the one call that cannot be interrupted, with
    /// the test deciding what happens while it runs.
    struct HeldWorker {
        receipts: Receipts,
        worker: Option<std::thread::JoinHandle<()>>,
        /// One message per validation that began.
        validating: std::sync::mpsc::Receiver<()>,
        /// One message lets one validation answer.
        answer: std::sync::mpsc::Sender<()>,
        /// What each receipt was settled as, in the order they were settled.
        settled: std::sync::mpsc::Receiver<Result<(), &'static str>>,
        /// The helpers' ends of the connections, kept open.
        helpers: Vec<UnixStream>,
    }

    const LEASED_SHELL: (u32, (u64, u32)) = (10, (100, 1));
    const LEASED_CLIENT: (u32, (u64, u32)) = (20, (200, 2));
    const LEASED_HELPER: u32 = 30;

    impl HeldWorker {
        fn start() -> Self {
            use crate::agent_reentry::lifecycle;
            let (began, validating) = std::sync::mpsc::channel();
            let (answer, answered) = std::sync::mpsc::channel::<()>();
            let (results, settled) = std::sync::mpsc::channel();
            let (receipts, worker) = Receipts::start_with(move |receipt: Receipt| {
                // the pane's shell, the client in the terminal's foreground,
                // and the helper as the client's direct child
                let process = |pid: u32, ppid: u32, tty: u32, foreground: u32| ProcessInfo {
                    pid,
                    ppid,
                    pgid: pid,
                    tty,
                    tty_pgid: foreground,
                    start_seconds: u64::from(pid) * 10,
                    start_micros: pid / 10,
                };
                let table: ProcessTable = [
                    process(LEASED_SHELL.0, 1, 7, LEASED_CLIENT.0),
                    process(LEASED_CLIENT.0, LEASED_SHELL.0, 7, LEASED_CLIENT.0),
                    process(LEASED_HELPER, LEASED_CLIENT.0, 0, 0),
                ]
                .into_iter()
                .map(|info| (info.pid, info))
                .collect();
                let result = receipt.settle(&lifecycle::Kernel {
                    snapshot: &|| table.clone(),
                    runs_helper: &|_, _| true,
                    at_executable: &|_, _| true,
                    runs_executable: &|_, _| {
                        let _ = began.send(());
                        let _ = answered.recv();
                        true
                    },
                });
                let _ = results.send(result);
            });
            Self {
                receipts,
                worker,
                validating,
                answer,
                settled,
                helpers: Vec::new(),
            }
        }

        /// One receipt of the operation in flight — its line read now, its
        /// claim taken now, a budget of its own that no test here reaches —
        /// handed to the worker.
        fn hand(&mut self, line: &str) {
            use crate::agent_reentry::lifecycle;
            let (ours, theirs) = UnixStream::pair().expect("a connection");
            let wire = lifecycle::parse(line, |_| Some(&crate::agent_reentry::claude::RECEIPTS))
                .expect("a valid line");
            let receipt = Receipt {
                claim: lifecycle::claim(wire).expect("an armed lease"),
                stream: ours,
                peer: Some(LEASED_HELPER),
                deadline: std::time::Instant::now() + Duration::from_secs(60),
            };
            self.helpers.push(theirs);
            assert_eq!(self.receipts.hand(receipt), Ok(()));
        }

        fn validation_began(&self) {
            self.validating
                .recv_timeout(Duration::from_secs(10))
                .expect("the worker is inside a validation");
        }

        fn let_answer(&self) {
            self.answer.send(()).expect("the worker is running");
        }

        fn next_settled(&self) -> Result<(), &'static str> {
            self.settled
                .recv_timeout(Duration::from_secs(10))
                .expect("the worker settled a receipt")
        }

        fn stop(self) {
            drop(self.receipts);
            if let Some(worker) = self.worker {
                worker.join().expect("the worker ended");
            }
        }
    }

    fn lifecycle_line(event: &str, id: &str) -> String {
        format!(
            r#"{{"protocol":"agent-lifecycle","v":1,"source":"claude-code",{event},"nativeSessionId":"{id}","socket":"deck","server_pid":4242,"pane":"%7"}}"#
        )
    }

    /// One operation's claims are no other operation's. Two receipts of an
    /// operation are with the worker — one inside its validation, one
    /// waiting in the queue — when that operation ends and the next begins,
    /// arming the same entry for the same pane, shell and client with the
    /// same serial (every operation counts its armings from one). Both old
    /// receipts are verified in full, positively, inside their budgets; both
    /// are refused where the fact would be written, and the next operation's
    /// mailbox holds only what its own receipt recorded.
    #[test]
    fn a_receipt_of_an_ended_operation_is_refused_from_the_queue_and_from_its_validation() {
        use crate::agent_reentry::lifecycle;
        let _lock = crate::agent_reentry::TEST_LOCK.lock_or_recover();
        crate::agent_reentry::reset_for_tests();
        let begin_operation = || {
            lifecycle::end();
            lifecycle::begin("deck", Path::new("/nonexistent/deck-lifecycle-helper"));
            lifecycle::arm(
                0,
                lifecycle::Lease::exit(
                    4242,
                    "%7",
                    LEASED_SHELL,
                    LEASED_CLIENT,
                    crate::procinfo::ExecutableRef::of_this_test(),
                ),
            );
        };
        let exit = |id: &str| {
            lifecycle_line(
                r#""event":"session-end","endReason":"prompt-input-exit""#,
                id,
            )
        };
        const VALIDATING: &str = "aaaaaaaa-0000-4000-8000-000000000001";
        const QUEUED: &str = "bbbbbbbb-0000-4000-8000-000000000002";
        const OWN: &str = "cccccccc-0000-4000-8000-000000000003";
        begin_operation();
        let mut worker = HeldWorker::start();
        worker.hand(&exit(VALIDATING));
        worker.validation_began();
        worker.hand(&exit(QUEUED));
        // the operation ends; the next one begins, arms and is reported to
        begin_operation();
        worker.hand(&exit(OWN));
        // the old claim that was inside its validation
        worker.let_answer();
        assert_eq!(worker.next_settled(), Err("lifecycle-no-lease"));
        // the old claim that waited in the queue: verified, and refused
        worker.validation_began();
        assert!(
            lifecycle::peek(0).is_empty(),
            "nothing of the ended operation reached the next one's mailbox"
        );
        worker.let_answer();
        assert_eq!(worker.next_settled(), Err("lifecycle-no-lease"));
        assert!(lifecycle::peek(0).is_empty());
        // the next operation's own receipt, behind them in the same queue
        worker.validation_began();
        worker.let_answer();
        assert_eq!(worker.next_settled(), Ok(()));
        let facts = lifecycle::seal(0);
        assert_eq!(facts.len(), 1, "exactly its own fact");
        assert_eq!(
            crate::agent_reentry::claude::final_session(&facts, &[]).as_deref(),
            Some(OWN)
        );
        worker.stop();
        lifecycle::end();
    }

    /// A receipt is handed to the worker before the post share ends and its
    /// validation answers yes after it — the lease still armed, nobody having
    /// sealed it, the receipt's own budget far from over. Nothing is
    /// written, and what is sealed afterwards holds nothing of it. Inside
    /// its phase the same receipt is a fact.
    #[test]
    fn a_receipt_validated_across_the_post_deadline_is_not_written() {
        use crate::agent_reentry::lifecycle;
        use std::time::Instant;
        let _lock = crate::agent_reentry::TEST_LOCK.lock_or_recover();
        crate::agent_reentry::reset_for_tests();
        // read before any phase's end is fixed: the first reference a test
        // process makes is a full read of its own executable
        let executable = crate::procinfo::ExecutableRef::of_this_test();
        let arm_until = |closes: Instant| {
            lifecycle::end();
            lifecycle::begin("deck", Path::new("/nonexistent/deck-lifecycle-helper"));
            lifecycle::arm(
                0,
                lifecycle::Lease::launch(4242, "%7", LEASED_SHELL, executable.clone(), closes),
            );
        };
        const ID: &str = "aaaaaaaa-0000-4000-8000-000000000001";
        let resume = lifecycle_line(r#""event":"session-start","startSource":"resume""#, ID);
        // long enough for the worker to be inside the validation before it
        let closes = Instant::now() + Duration::from_secs(1);
        arm_until(closes);
        let mut worker = HeldWorker::start();
        worker.hand(&resume);
        worker.validation_began();
        assert!(Instant::now() < closes, "handed over inside the phase");
        while Instant::now() < closes {
            std::thread::sleep(Duration::from_millis(5));
        }
        // the post share is over; the validation answers yes now
        worker.let_answer();
        assert_eq!(worker.next_settled(), Err("lifecycle-late"));
        assert!(lifecycle::armed(), "the lease was never sealed");
        assert!(
            lifecycle::seal_all().is_empty(),
            "the sealed set holds nothing of it"
        );
        // the same receipt inside its phase
        arm_until(Instant::now() + Duration::from_secs(60));
        worker.hand(&resume);
        worker.validation_began();
        worker.let_answer();
        assert_eq!(worker.next_settled(), Ok(()));
        let sealed = lifecycle::seal_all();
        assert_eq!(
            crate::agent_reentry::claude::entered(&sealed[&0], ID),
            lifecycle::Receipt::Entered(LEASED_CLIENT)
        );
        worker.stop();
        lifecycle::end();
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

    /// Pi has no hook helper and no interaction id of its own: a status
    /// line that names it is refused by the parser, whatever its version,
    /// word or id, and whoever sent it. Nothing an observer counts can be
    /// dressed up as a native v2 id.
    #[test]
    fn a_hook_line_that_names_pi_is_refused_whatever_else_it_says() {
        let _guard = STORE_TEST_LOCK.lock_or_recover();
        reset_for_tests();
        let line = |version: u32, state: &str, interaction: Option<&str>| {
            let mut line = serde_json::json!({
                "v": version, "source": "pi", "state": state,
                "socket": crate::tmux::socket(), "server_pid": 42, "pane": "%3"
            });
            if let Some(id) = interaction {
                line["interaction"] = id.into();
            }
            line.to_string()
        };
        let id = "731c5908-251a-4388-b3a3-4719ad442bce";
        for state in STATES {
            assert_eq!(
                parse_event(&line(1, state, None)),
                Err("pi-observer-required")
            );
            assert_eq!(
                parse_event(&line(2, state, Some(id))),
                Err("pi-observer-required")
            );
        }
        // the same line from the pane's own foreground program, with every
        // admission fact in order: still refused, and nothing is stored
        let rows = [row("deck-card-ab12", "$1", "%3", 300, true, "pi")];
        let table = with_helper(world(&[pane(300, 7, 310, 2000)]), 320, 310);
        for text in [line(1, "working", None), line(2, "turn-done", Some(id))] {
            assert_eq!(
                report_line(&text, Some(320), &table, &rows),
                Err("pi-observer-required")
            );
        }
        assert!(projections(&rows).is_empty());
        assert!(
            !generation_evidence(&rows, |pane| foreground_generation(&table, pane), |_| None)
                ["deck-card-ab12"]
                .claude_interaction
        );
        // counted in the bounded drop diagnostics under its own reason
        assert!(DROP_REASONS.contains(&"pi-observer-required"));
        // the other two sources are parsed as they were
        assert!(
            parse_event(&line(1, "working", None).replace("\"pi\"", "\"claude-code\"")).is_ok()
        );
        assert!(parse_event(&line(2, "working", Some(id)).replace("\"pi\"", "\"codex\"")).is_ok());
        // every refusal the Pi family can send has its drop reason here
        for code in crate::agent_pi::REFUSALS {
            let reason = crate::agent_pi::drop_reason(code);
            assert!(DROP_REASONS.contains(&reason), "{reason}");
        }
    }

    fn report_line(
        line: &str,
        peer: Option<u32>,
        table: &ProcessTable,
        rows: &[PaneRow],
    ) -> Result<(), &'static str> {
        let origin = Origin {
            peer,
            table: table.clone(),
        };
        ingest(line, &origin, || Some(rows.to_vec()))
    }

    /// The two long-lived workers beside the accept loop do not wait for each
    /// other. While the receipt worker is inside a Claude code validation
    /// that does not return, a Pi message is taken and answered by the Pi
    /// transport; while the Pi admission is stuck in a pane listing, a
    /// lifecycle receipt is still handed over and settled. Neither holds the
    /// accept loop.
    #[test]
    fn a_held_receipt_validation_and_a_stuck_pi_admission_do_not_wait_for_each_other() {
        use crate::agent_pi::transport::{Bounds, Service, World};
        use crate::agent_reentry::lifecycle;
        use std::io::{BufRead, Write};
        use std::time::Instant;
        let _lock = crate::agent_reentry::TEST_LOCK.lock_or_recover();
        let _guard = STORE_TEST_LOCK.lock_or_recover();
        crate::agent_reentry::reset_for_tests();
        reset_for_tests();
        let executable = crate::procinfo::ExecutableRef::of_this_test();
        lifecycle::begin("deck", Path::new("/nonexistent/deck-lifecycle-helper"));
        lifecycle::arm(
            0,
            lifecycle::Lease::launch(
                4242,
                "%7",
                LEASED_SHELL,
                executable,
                Instant::now() + Duration::from_secs(60),
            ),
        );
        const ID: &str = "aaaaaaaa-0000-4000-8000-000000000001";
        let resume = lifecycle_line(r#""event":"session-start","startSource":"resume""#, ID);
        let hello = pi_hello();
        let drops = Mutex::new(Drops::new(Instant::now()));
        let answered = |service: &Service, receipts: &Receipts| -> (serde_json::Value, Duration) {
            let (mut ours, theirs) = UnixStream::pair().expect("a socket pair");
            ours.write_all(format!("{hello}\n").as_bytes()).unwrap();
            let from = Instant::now();
            handle_stream(theirs, &drops, receipts, service);
            let handed = from.elapsed();
            ours.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut reply = String::new();
            std::io::BufReader::new(&ours)
                .read_line(&mut reply)
                .unwrap();
            (serde_json::from_str(&reply).expect("a reply line"), handed)
        };
        let quick = || Bounds {
            answer: Duration::from_secs(2),
            stale: Duration::from_secs(15),
            challenge: Duration::from_secs(5),
        };
        let watched = || -> Box<dyn Fn(u32) -> Option<Arc<InstanceWatch>> + Send + Sync> {
            Box::new(|_| Some(InstanceWatch::stand_in().0))
        };

        // 1. the receipt worker is held inside a validation that does not return
        let mut worker = HeldWorker::start();
        worker.hand(&resume);
        worker.validation_began();
        let pi = Service::start_in(
            Arc::new(|_| {}),
            World {
                process: Box::new(|_| None),
                listing: Box::new(|| None),
                enabled: Box::new(|| crate::agent_pi::Capabilities::NONE),
                watch: watched(),
                nonce: Box::new(crate::agent_pi::Nonce::fresh),
                mark: Box::new(|| Some(1)),
                socket: "deck".into(),
            },
            quick(),
        );
        let (reply, handed) = answered(&pi, &worker.receipts);
        assert_eq!(reply["protocol"], "pi-observer");
        assert!(reply["nonce"].is_string(), "answered by the Pi transport");
        assert!(
            handed < Duration::from_millis(500),
            "the accept loop did not wait"
        );
        // the validation is still held: nothing of it was needed for that answer
        worker.let_answer();
        assert_eq!(worker.next_settled(), Ok(()));

        // 2. the Pi admission is stuck in a listing that does not return
        let (entered, inside) = std::sync::mpsc::channel::<()>();
        let (free, wait) = std::sync::mpsc::channel::<()>();
        let wait = Mutex::new(wait);
        let table = {
            // a peer that leads its terminal's foreground, so the admission reaches the listing
            let me = std::process::id();
            move |pid: u32| {
                (pid == me).then_some(crate::procinfo::ProcessInfo {
                    pid: me,
                    ppid: 1,
                    pgid: me,
                    tty: 7,
                    tty_pgid: me,
                    start_seconds: 1000,
                    start_micros: 0,
                })
            }
        };
        let held = Service::start_in(
            Arc::new(|_| {}),
            World {
                process: Box::new(table),
                listing: Box::new(move || {
                    let _ = entered.send(());
                    let _ = wait.lock_or_recover().recv();
                    None
                }),
                enabled: Box::new(|| crate::agent_pi::Capabilities::NONE),
                watch: watched(),
                nonce: Box::new(crate::agent_pi::Nonce::fresh),
                mark: Box::new(|| Some(1)),
                socket: "deck".into(),
            },
            quick(),
        );
        // a hello is answered with a nonce; the attest that carries it is
        // what reaches the listing
        let (ours, theirs) = UnixStream::pair().expect("a socket pair");
        ours.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        held.hand_as(theirs, hello.clone(), Some(std::process::id()), 1);
        let mut given = String::new();
        std::io::BufReader::new(&ours)
            .read_line(&mut given)
            .unwrap();
        let given: serde_json::Value = serde_json::from_str(&given).expect("a reply line");
        let attest = serde_json::json!({
            "protocol": "pi-observer", "v": 2, "source": "pi", "nonce": given["nonce"],
            "socket": "deck", "server_pid": 42, "pane": "%3", "observerVersion": 2,
            "piVersion": "1.1.0", "mode": "tui",
            "capabilities": { "signal": true, "lifecycle": false }
        })
        .to_string();
        let (ours, theirs) = UnixStream::pair().expect("a socket pair");
        // on a connection that was made after the hello's look
        held.hand_as(theirs, attest, Some(std::process::id()), 2);
        inside
            .recv_timeout(Duration::from_secs(5))
            .expect("the Pi admission is inside its listing");
        // a receipt meanwhile: handed over at once and settled by its own worker
        let from = Instant::now();
        worker.hand(&resume);
        assert!(from.elapsed() < Duration::from_millis(500));
        worker.validation_began();
        worker.let_answer();
        let _ = worker.next_settled();
        assert!(
            from.elapsed() < Duration::from_secs(2),
            "the receipt did not wait for the Pi admission"
        );
        let _ = free.send(());
        drop(ours);
        worker.stop();
        lifecycle::end();
        crate::agent_reentry::reset_for_tests();
    }

    /// The listener's dispatch: a `pi-observer` line goes to the Pi
    /// transport and to nothing else — not the lifecycle mailbox, which
    /// would take any line with a `protocol` key, and not the hook parser.
    #[test]
    fn the_listener_hands_a_pi_message_to_the_pi_transport_and_to_nothing_else() {
        use crate::agent_pi::transport::{Bounds, Service, World};
        use std::io::{BufRead, Write};
        // the mailbox is one per process: this test begins and ends an
        // operation, so it holds the lock every receipt test holds
        let _operation = crate::agent_reentry::TEST_LOCK.lock_or_recover();
        let _guard = STORE_TEST_LOCK.lock_or_recover();
        reset_for_tests();
        let hello = pi_hello();
        // both family tests take the line: the order of the dispatch decides
        assert!(crate::agent_pi::is_family(&hello));
        assert!(crate::agent_reentry::lifecycle::is_family(&hello));
        assert!(!crate::agent_pi::is_family(
            "{\"protocol\":\"agent-lifecycle\",\"v\":1}"
        ));
        assert!(!crate::agent_pi::is_family(
            "{\"v\":1,\"source\":\"pi\",\"state\":\"working\"}"
        ));
        let reasons = Arc::new(Mutex::new(Vec::new()));
        let drops = Mutex::new(Drops::new(std::time::Instant::now()));
        let settled = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (receipts, worker) = Receipts::start_with({
            let settled = settled.clone();
            move |_receipt: Receipt| {
                settled.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        });
        // an operation is in flight, so a lifecycle line WOULD be claimed
        crate::agent_reentry::lifecycle::begin("deck", Path::new("/nonexistent/helper"));
        let pi = Service::start_in(
            {
                let reasons = reasons.clone();
                Arc::new(move |reason| reasons.lock_or_recover().push(reason))
            },
            World {
                process: Box::new(|_| None),
                listing: Box::new(|| None),
                enabled: Box::new(|| crate::agent_pi::Capabilities::NONE),
                // the kernel gives no watch of this process: nothing is asked of it
                watch: Box::new(|_| None),
                nonce: Box::new(crate::agent_pi::Nonce::fresh),
                mark: Box::new(|| Some(1)),
                socket: "deck".into(),
            },
            Bounds {
                answer: Duration::from_secs(2),
                stale: Duration::from_secs(15),
                challenge: Duration::from_secs(5),
            },
        );
        let (mut ours, theirs) = UnixStream::pair().expect("a socket pair");
        ours.write_all(format!("{hello}\n").as_bytes()).unwrap();
        let started = std::time::Instant::now();
        handle_stream(theirs, &drops, &receipts, &pi);
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "the listener does not wait for the answer"
        );
        // the Pi transport answered in its own protocol: it could not watch
        // the process that said hello, so it asked nothing of it
        ours.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut reply = String::new();
        std::io::BufReader::new(&ours)
            .read_line(&mut reply)
            .unwrap();
        let reply: serde_json::Value = serde_json::from_str(&reply).expect("a reply line");
        assert_eq!(reply["protocol"], "pi-observer");
        assert_eq!(reply["refused"], "no-instance");
        assert_eq!(*reasons.lock_or_recover(), ["pi-no-instance"]);
        // nothing of it reached the lifecycle mailbox or the hook parser
        drop(receipts);
        worker.expect("the receipt worker").join().unwrap();
        assert_eq!(settled.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(
            drops.lock_or_recover().counts.is_empty(),
            "no hook or lifecycle drop"
        );
        crate::agent_reentry::lifecycle::end();
        // and the order is what the source says: Pi before the lifecycle family
        let source = include_str!("agent_status.rs");
        let at = source.find("fn dispatch(").unwrap();
        let body = &source[at..at + source[at..].find("\n}\n").unwrap()];
        let pi_at = body.find("crate::agent_pi::is_family(&line)").unwrap();
        let lifecycle_at = body
            .find("crate::agent_reentry::lifecycle::is_family(&line)")
            .unwrap();
        assert!(pi_at < lifecycle_at);
    }

    // ---------- the listener's own bounds ----------------------------------------

    /// A status socket of this test's own, served by the production accept
    /// step, first-line read, dispatcher, receipt worker and Pi service.
    struct Listening {
        dir: PathBuf,
        path: PathBuf,
        stop: Arc<std::sync::atomic::AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
        counts: DropCounts,
    }

    impl Listening {
        fn start(label: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("deck-l-{label}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("status.sock");
            let listener = listen_at(&path).unwrap();
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let counts = DropCounts::new();
            let thread = std::thread::spawn({
                let (stop, counts) = (stop.clone(), counts.clone());
                move || serve_counting_for_tests(listener, stop, &counts)
            });
            Self {
                dir,
                path,
                stop,
                thread: Some(thread),
                counts,
            }
        }

        fn connect(&self) -> UnixStream {
            let stream = UnixStream::connect(&self.path).expect("a connection");
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream
        }

        fn counted(&self, reason: &str) -> u32 {
            self.counts.counts().get(reason).copied().unwrap_or(0)
        }

        fn until_counted(&self, reason: &str, count: u32) {
            let end = std::time::Instant::now() + Duration::from_secs(10);
            while self.counted(reason) < count {
                assert!(
                    std::time::Instant::now() < end,
                    "timed out waiting for {count} x {reason}: {:?}",
                    self.counts.counts()
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    impl Drop for Listening {
        fn drop(&mut self) {
            self.stop.store(true, std::sync::atomic::Ordering::Release);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// A Pi observer's first message: it asks for a nonce and claims nothing.
    fn pi_hello() -> String {
        serde_json::json!({ "protocol": "pi-observer", "v": 2, "source": "pi" }).to_string()
    }

    /// The answer of the Pi transport to a hello, and how long it took.
    fn pi_answer(listening: &Listening) -> (serde_json::Value, Duration) {
        use std::io::{BufRead, Write};
        let from = std::time::Instant::now();
        let mut stream = listening.connect();
        stream
            .write_all(format!("{}\n", pi_hello()).as_bytes())
            .unwrap();
        let mut reply = String::new();
        std::io::BufReader::new(&stream)
            .read_line(&mut reply)
            .unwrap();
        (
            serde_json::from_str(&reply).expect("a reply line"),
            from.elapsed(),
        )
    }

    /// One client that never finishes its first line — silent, half a line,
    /// or a byte now and then — holds nobody up: while all three are
    /// connected, a Pi message, a hook word and a lifecycle receipt are each
    /// dispatched at once. And each of the three runs out of ITS OWN time,
    /// counted from its accept: a byte that arrives resets nothing, and a
    /// line that never ended is not parsed.
    #[test]
    fn a_silent_or_dripping_client_holds_nobody_up_and_runs_out_of_its_own_time() {
        use crate::agent_reentry::lifecycle;
        use std::io::Write;
        use std::time::Instant;
        let _lock = crate::agent_reentry::TEST_LOCK.lock_or_recover();
        let _guard = STORE_TEST_LOCK.lock_or_recover();
        crate::agent_reentry::reset_for_tests();
        reset_for_tests();
        // an operation in flight, so that a lifecycle line has a family to reach
        lifecycle::begin("deck", Path::new("/nonexistent/deck-lifecycle-helper"));
        lifecycle::arm(
            0,
            lifecycle::Lease::launch(
                4242,
                "%7",
                LEASED_SHELL,
                crate::procinfo::ExecutableRef::of_this_test(),
                Instant::now() + Duration::from_secs(60),
            ),
        );
        let listening = Listening::start("slow");
        let opened = Instant::now();
        let mut silent = listening.connect();
        let mut partial = listening.connect();
        partial
            .write_all(br#"{"v":1,"source":"claude-code","sta"#)
            .unwrap();
        let drip = listening.connect();
        let dripping = std::thread::spawn({
            let mut drip = drip.try_clone().unwrap();
            move || {
                let mut sent = 0;
                while drip.write_all(b" ").is_ok() {
                    sent += 1;
                    std::thread::sleep(Duration::from_millis(250));
                }
                sent
            }
        });
        std::thread::sleep(Duration::from_millis(500));

        // a Pi message: taken and answered by the Pi transport
        let (reply, took) = pi_answer(&listening);
        assert_eq!(reply["protocol"], "pi-observer");
        assert!(reply["nonce"].is_string(), "{reply}");
        assert!(took < Duration::from_millis(1000), "Pi waited {took:?}");
        // a hook word, Claude's and Codex's: handled (here refused, for the
        // server it names) while its helper is still connected
        for source in ["claude-code", "codex"] {
            let from = Instant::now();
            let mut hook = listening.connect();
            let line = event_line("working", "%3")
                .replace("claude-code", source)
                .replace(crate::tmux::socket(), "deck-other");
            writeln!(hook, "{line}").unwrap();
            hook.shutdown(std::net::Shutdown::Write).unwrap();
            listening.until_counted("other-server", if source == "codex" { 2 } else { 1 });
            assert!(
                from.elapsed() < Duration::from_millis(1000),
                "{source} waited {:?}",
                from.elapsed()
            );
            // released by the dispatcher: its close is the helper's end
            assert_eq!(std::io::Read::read(&mut hook, &mut [0u8; 1]).ok(), Some(0));
        }
        // a lifecycle receipt of the operation in flight: reaches its family
        let from = Instant::now();
        let mut receipt = listening.connect();
        let line = lifecycle_line(
            r#""event":"session-start","startSource":"resume""#,
            "aaaaaaaa-0000-4000-8000-000000000001",
        )
        .replace("claude-code", "mystery-agent");
        writeln!(receipt, "{line}").unwrap();
        listening.until_counted("lifecycle-unknown-source", 1);
        assert!(from.elapsed() < Duration::from_millis(1000));
        lifecycle::end();
        assert!(
            opened.elapsed() < FIRST_LINE_BUDGET,
            "all of that while the three were still inside their time: {:?}",
            opened.elapsed()
        );
        assert_eq!(listening.counted("first-line-late"), 0);

        // their own time, from their accept
        listening.until_counted("first-line-late", 3);
        let took = opened.elapsed();
        assert!(
            took >= FIRST_LINE_BUDGET - Duration::from_millis(50)
                && took < FIRST_LINE_BUDGET + Duration::from_millis(1500),
            "an absolute deadline: {took:?}"
        );
        for stream in [&mut silent, &mut partial] {
            assert_eq!(
                std::io::Read::read(stream, &mut [0u8; 1]).ok(),
                Some(0),
                "closed by the listener"
            );
        }
        assert!(
            dripping.join().unwrap() >= 5,
            "its bytes kept arriving, and bought it no time"
        );
        assert_eq!(
            listening.counted("bad-json"),
            0,
            "a line that never ended is not parsed"
        );
        crate::agent_reentry::reset_for_tests();
    }

    /// Every accepted connection has its place: the order in which the
    /// connections were MADE — whatever the order in which their lines
    /// came, and whether or not they ever said anything. Deck's own line is
    /// how the Pi transport asks for one (`agent_pi::transport::mark_at`).
    #[test]
    fn a_connection_s_place_is_the_order_in_which_it_was_made() {
        use std::io::{BufRead, Write};
        let _guard = STORE_TEST_LOCK.lock_or_recover();
        reset_for_tests();
        let listening = Listening::start("place");
        let ask = format!("{}\n", crate::agent_pi::place_line());
        let asked = |mut stream: UnixStream| -> u64 {
            stream.write_all(ask.as_bytes()).unwrap();
            let mut reply = String::new();
            std::io::BufReader::new(&stream)
                .read_line(&mut reply)
                .unwrap();
            crate::agent_pi::placed(&reply).unwrap_or_else(|| panic!("no place: {reply:?}"))
        };
        // four connections, one after the other; one never says a word, and
        // the others speak in the opposite order
        let first = listening.connect();
        let silent = listening.connect();
        let second = listening.connect();
        let third = listening.connect();
        let third = asked(third);
        let second = asked(second);
        let first = asked(first);
        assert_eq!((second, third), (first + 2, first + 3));
        drop(silent);
        // one that is made later is behind them all, the transport's too
        assert_eq!(asked(listening.connect()), first + 4);
        assert_eq!(
            crate::agent_pi::transport::mark_at(&listening.path),
            Some(first + 5)
        );
        assert!(listening.counts.counts().is_empty(), "nothing was refused");
    }

    /// A first line is at most `MAX_LINE` bytes. What a client sent behind
    /// its first line is never read and thrown away: every family is one
    /// message per connection, so it means that the message is none of
    /// theirs — a Pi observer's line as well, where it is also what keeps a
    /// second writer's bytes from riding behind the first. A line may also
    /// end with the client's end of its side, as before.
    #[test]
    fn a_first_line_is_bounded_and_what_follows_it_is_never_thrown_away() {
        use std::io::Write;
        let _guard = STORE_TEST_LOCK.lock_or_recover();
        reset_for_tests();
        let listening = Listening::start("line");
        let foreign = event_line("working", "%3").replace(crate::tmux::socket(), "deck-other");

        // more than a line, without an end
        let mut big = listening.connect();
        let _ = big.write_all(&vec![b'x'; MAX_LINE + 2048]);
        listening.until_counted("first-line-oversize", 1);
        assert_eq!(std::io::Read::read(&mut big, &mut [0u8; 1]).ok(), Some(0));

        // a hook word with more behind it is no hook word
        let mut more = listening.connect();
        more.write_all(format!("{foreign}\n{foreign}\n").as_bytes())
            .unwrap();
        listening.until_counted("trailing-bytes", 1);
        assert_eq!(
            listening.counted("other-server"),
            0,
            "neither line was taken"
        );

        // a line that ends with the client's end of its side
        let mut ended = listening.connect();
        ended.write_all(foreign.as_bytes()).unwrap();
        ended.shutdown(std::net::Shutdown::Write).unwrap();
        listening.until_counted("other-server", 1);

        // a Pi line alone is the Pi transport's ...
        let (reply, _) = pi_answer(&listening);
        assert_eq!(reply["protocol"], "pi-observer");
        assert_eq!(listening.counted("trailing-bytes"), 1);
        // ... and with anything behind it — a second line, half of one, a
        // single byte — it is nobody's: not answered, and nothing of it
        // reaches the transport
        for behind in [
            format!("{}\n", pi_hello()),
            "{\"protocol\"".to_string(),
            "x".to_string(),
        ] {
            let mut two = listening.connect();
            two.write_all(format!("{}\n{behind}", pi_hello()).as_bytes())
                .unwrap();
            assert_eq!(
                std::io::Read::read(&mut two, &mut [0u8; 1]).ok(),
                Some(0),
                "closed without an answer"
            );
        }
        listening.until_counted("trailing-bytes", 4);

        // an empty first line with bytes behind it: said, not swallowed
        let mut blank = listening.connect();
        blank
            .write_all(format!("\n{foreign}\n").as_bytes())
            .unwrap();
        listening.until_counted("trailing-bytes", 5);
        assert_eq!(
            listening.counted("other-server"),
            1,
            "that line was not taken"
        );

        // a client that connects and leaves without a byte is nothing
        drop(listening.connect());
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(listening.counted("bad-json"), 0);
        assert_eq!(listening.counted("trailing-bytes"), 5);
    }

    /// At most `MAX_FIRST_LINES` connections are read for their first line
    /// at one time. The next one is closed at once, unread — not queued, not
    /// waited for — and when the time of the others is up their places are
    /// free again.
    #[test]
    fn a_bounded_number_of_first_lines_is_read_and_the_next_connection_is_closed_at_once() {
        use std::time::Instant;
        let _guard = STORE_TEST_LOCK.lock_or_recover();
        reset_for_tests();
        let listening = Listening::start("full");
        let held: Vec<UnixStream> = (0..MAX_FIRST_LINES).map(|_| listening.connect()).collect();
        let opened = Instant::now();
        let mut extra = listening.connect();
        let from = Instant::now();
        assert_eq!(
            std::io::Read::read(&mut extra, &mut [0u8; 1]).ok(),
            Some(0),
            "closed, unread"
        );
        assert!(from.elapsed() < Duration::from_millis(1000));
        listening.until_counted("listener-busy", 1);
        assert!(
            opened.elapsed() < FIRST_LINE_BUDGET,
            "inside the others' time"
        );
        // their time is up: every place is free again, and the next is served
        listening.until_counted("first-line-late", MAX_FIRST_LINES as u32);
        let (reply, took) = pi_answer(&listening);
        assert_eq!(reply["protocol"], "pi-observer");
        assert!(took < Duration::from_millis(1000));
        drop(held);
    }

    /// The queue in front of the dispatcher holds `MAX_DISPATCH` arrivals. A
    /// dispatcher that is busy for long makes the next connection a refusal,
    /// never a longer queue.
    #[test]
    fn a_full_dispatch_queue_refuses_the_next_connection_instead_of_growing() {
        use std::io::Write;
        let drops = Arc::new(Mutex::new(Drops::new(std::time::Instant::now())));
        let (intake, undrained) = Intake::undrained(drops.clone());
        let mut clients = Vec::new();
        for _ in 0..MAX_DISPATCH + 1 {
            let (mut ours, theirs) = UnixStream::pair().expect("a socket pair");
            ours.write_all(b"{}\n").unwrap();
            intake.take(theirs);
            clients.push(ours);
            // one after the other, so that only the queue is ever full
            std::thread::sleep(Duration::from_millis(10));
        }
        let end = std::time::Instant::now() + Duration::from_secs(10);
        while drops.lock_or_recover().counts.get("listener-busy") != Some(&1) {
            assert!(std::time::Instant::now() < end, "no refusal");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(undrained.try_iter().count(), MAX_DISPATCH);
    }
}
