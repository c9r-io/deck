//! Local MCP structured-read/control service and durable authorization ledger.
//!
//! This is deliberately separate from Phone Connector: its socket, client
//! records, project scopes, short-lived execution grants, operation ids,
//! sessions, and jobs are disjoint. Creating a session never grants execution;
//! only the local Tauri command may create or expand an execution window.
//! Authorization roots are canonicalized directories before the UI confirms
//! them; they scope session creation and structured reads, never an approved
//! job's cwd, which is any existing directory (`grants::full_local_cwd`).
//! A client reaches a list of one to 64 projects, each with its roots, or,
//! with the user's choice `all_projects`, every project of the current
//! Board, those made later too, each through its own default directory
//! read and canonicalized at each use (`grants::scoped_project`; one
//! without a directory is seen, never worked in; a recovered Board grants
//! nothing). The scope is edited with the other choices
//! (`mcp_client_permissions`); a project that leaves it takes everything
//! the client had there with it (`grants::withdraw_projects`). A ledger
//! holding a client of all projects is written at
//! `STATE_VERSION_ALL_PROJECTS` (sticky).
//! Jobs run under `host-terminal-env-v1` (`ENVIRONMENT_PROFILE`): the Deck
//! session environment minus Deck's control-plane coordinates, applied by
//! the runner; a grant stored under a legacy profile loads as history and
//! never authorizes (`grants::standing_at`). Client deletion is available only after revocation has fenced live
//! authority; it removes the display authorization but retains ledger history.
//! Project list/read/search use descriptor-relative no-follow filesystem IO in
//! `mcp_fs.rs` and never start a shell or repository helper. The control
//! socket (0600, Deck's private data directory, same-effective-uid peers
//! only, bounded connections with timeouts) is bound only while the feature is
//! enabled and is removed on disable; while disabled its thread sleeps until
//! `mcp_enable` wakes it instead of polling the socket path, and while
//! enabled it blocks in `poll(2)` on the listener (1s lifecycle recheck), never
//! a short sleep loop. Accepted streams are switched back to blocking I/O
//! before their timeouts are set, and the whole request must arrive within
//! 500ms (`REQUEST_DEADLINE`), so idle peers cannot hold the slots. The thin
//! `deck-mcp` sidecar maps an absent control socket to `FEATURE_DISABLED`; when connected, the service
//! also answers `FEATURE_DISABLED` if a disable races the request. The sidecar provides MCP
//! STDIO and never receives a Phone token or unrestricted backend credential.
//!
//! Board creation and close intents are journaled here, then handed to the
//! webview's one serialized Board transaction (`mcp.js`). The managed runner
//! reports process exit and output EOF independently from that control-operation state. Direct
//! argv is never persisted or logged: only bounded
//! metadata, SHA-256 digests, and runner output exist. On restart NOTHING accepted is replayed — Board
//! creates and closes included: every accepted/executing/admitted operation
//! becomes `ambiguous` (`deck-restarted`). A close goes executing → admitted →
//! committed|ambiguous; `closing` is cleared on every outcome.
//!
//! Request identity and journal lifetime (`compact`): every side effect is
//! bound to a server-issued value that only moves forward — the session's
//! control epoch (exec, input, interrupt, close, renew, release), the
//! session's control sequence (every control action) or the client's create
//! sequence (create) — and a record is retired only once that value moved on,
//! so a replay of a retired request fails its epoch/sequence check instead of
//! executing again. Control records (one per session) sit outside the
//! ordinary pool, so release/request always fit; per-client quotas and a
//! bounded interrupt reserve keep one client from starving another or from
//! stopping work, and lapsed leases are closed under pressure. Grants are
//! bounded to the newest per session plus those a job binding references.
//!
//! Human control: takeover/revoke/disable and execution revocation set an
//! in-memory fence BEFORE waiting for the delivery lock; every side effect
//! re-checks it under that lock as its last step (`emergency_denial`,
//! `emergency_fence_error`), by operation class: an execution revocation
//! refuses exec and stdin, never reads, interrupts or closes, and inspect
//! (and the local session view's approve/revoke button) reports its
//! session's grant `revoked` from the fence on (read before the
//! grants, so the view never returns to `active` while it persists). It does
//! not itself change the session's output-sharing switch either way; reads
//! stay gated by sharing, client authorization, generation and job binding.
//! Inspect reports the session gate and open/closed binding counts separately;
//! a read distinguishes a recoverable session pause from a binding that local
//! takeover closed permanently.
//! Which tools a takeover fences is one list, `HUMAN_FENCED_TOOLS` (held equal
//! to `mcp-fixtures/tools.json` `human_fenced`), shared by `route` — which
//! refuses them before any journal slot is reserved — and `emergency_denial`;
//! `deck_job_interrupt` is on it. Whether exec may start is one derivation,
//! `grants::admit_exec` (generation, control, grant, closing, in that
//! order): exec's accept and final admissions call it, and inspect's
//! `mayStartNextJobReason` reports its first failure after the runner facts.
//! Takeover closes existing job output to MCP for good and gives the pane
//! keyboard (and the ^C stop key) to the human; it starts no shell. Return to
//! MCP needs no execution grant and restores no holder, lease or sharing: it
//! persists a new epoch first and re-fences if the runner does not confirm.
//! Authenticated runner control accepts any strictly newer Deck epoch so a
//! persisted fence, lapsed-lease compaction, or failed acknowledgement cannot
//! permanently desynchronize the two sides; equal and older epochs are
//! rejected, and exec/input/interrupt still require an exact current epoch.
//! A runner created by an earlier Deck process is `stale` (`RUNNER_STALE`)
//! and cannot be called by the new process: each runner keeps an in-memory
//! 256-bit key retrieved once by the launching Deck PID through kernel peer
//! credentials. Deck in turn talks only to a socket whose kernel-reported
//! peer is the tmux pane process (`runner_exchange`), so a pane job that
//! moves the socket aside and listens in its place never receives the key. Closing its tmux pane invokes the runner's SIGHUP cleanup.
//! Local-command failures are stable machine codes (`mcp-*`) the webview maps
//! to one sentence each.
//!
//! An unreadable ledger — damaged, written by a deck this build does not
//! understand, or one whose boot-time save failed — is an error from the one
//! load (`spawn`) to the end of the run: no socket, every tool
//! `FEATURE_DISABLED`, no write, no runner started, mcp.json untouched, and
//! `mcp_status` answers `unavailable` so Settings can say why. It must not
//! take the other terminals with it. The guards other modules call —
//! `guard_terminal_input` (every terminal-input path and the Connector's
//! pane read) and `guard_server_restart` — then decide from the panes
//! themselves (`RunnerPanes`): such a run cannot start a runner, so ONE tmux
//! pane listing finds every runner pane there will be, namely the panes
//! whose root process is the bundled runner (argv[0] through libproc, no
//! spawn). Those sessions, and any whose root process cannot be identified,
//! stay refused and are looked at again through libproc alone; every other
//! session, including one created later, is not MCP's to fence. A server
//! restart goes ahead only when none is left. Until a listing has succeeded
//! nothing is known and both guards refuse. The input guard is asked per
//! keystroke, so on one session's behalf it asks again for a listing that
//! failed at most once a second. That pause holds nobody else back: before
//! the first session exists tmux has no pane to list, and neither a status
//! read nor another session's failure there may cost the next session its
//! first input.
//!
//! Runner errors (`runner::RUNNER_ERRORS`, held to
//! `mcp-fixtures/runner-errors.json`): an error the runner gives before any
//! job process could start or any input byte could be written is a
//! REJECTION — the operation is `rejected` with a deterministic code and a
//! rejected exec keeps no job binding. Only an error given when a process may
//! already have started (or bytes been written) is AMBIGUOUS, and it keeps
//! its own code (`JOB_STATE_UNKNOWN`, `STOP_UNCONFIRMED`,
//! `RESPONSE_TOO_LARGE`); an unrecognised reply is ambiguous too
//! (`dispatch-unknown` / `delivery-unknown`, `OPERATION_AMBIGUOUS`). Limits,
//! the control protocol, runner argv and the tool list are likewise held to
//! the shared `mcp-fixtures/` by each crate's own tests; no crate parses
//! another's source.
//!
//! Control protocol skew: a request at another `CONTROL_PROTOCOL` is refused
//! with `PROTOCOL_MISMATCH` before any other check, the same for everyone,
//! and leaves no log line and no audit record. The adapter's own version
//! never reaches Deck. A long-lived adapter (the Secure Tunnel keeps one)
//! outlives a Deck update, so the local user is told where they can act: when
//! the refused request's client id and credential authenticate, the client
//! is noted in memory (`EmergencyFences::refused_adapters`) and Settings says
//! on that client's row to restart the integration (`ClientView`
//! `adapter_refused`); the client's next accepted request drops the note. An
//! unauthenticated peer leaves nothing. An adapter of the same protocol is
//! never mentioned, whatever build it came from. The protocol number is
//! raised only when an adapter of the previous protocol would be unsafe or
//! wrong against this Deck: every raise refuses every running adapter until
//! its integration is restarted (`docs/mcp-architecture.md`).
//! Build identity is diagnostic only: `deck_capabilities` carries
//! `deckVersion`/`deckBuild`, inspect carries the pane runner's
//! `runnerBuild` (the commit it was built from) and `runnerVersion`, which is
//! the runner crate's own version and does not follow Deck releases. Whether
//! a pane's runner belongs to this Deck service is `stale`, nothing else.
//! The control socket is bound under a private temporary name, made 0600, and
//! atomically renamed into place; Deck never changes its process-wide umask.
//!
//! Automation delegation (`automation.rs`, `docs/mcp.md`): a client with the
//! separate local choice `allow_automations` may list the user's Agent
//! rules (`deck_automation_list`: id, name, project, current revision) and
//! hand over one item to one of them (`deck_automation_trigger`). It is
//! not session or execution authority: no execution window, control lease
//! or managed session is involved, and the item reaches a terminal only
//! through the dispatcher's external queue path under the rule's own
//! approvals. A trigger is bound to the client's `trigger_sequence` exactly
//! as a create is bound to `create_sequence` (replay by request id first,
//! `STALE_REQUEST` once retired; records take the same per-client window),
//! and to the rule's `revision`, refused as `RULE_CHANGED` when the rule or
//! its template moved. Acceptance is three writes: the agent inbox entry
//! (`inbound_agent::stage`), the operation plus the sequence (`write`), the
//! entry's commit; a replay settles a crash between them, and `spawn`
//! reconciles accepted trigger records against the inbox instead of
//! turning them `ambiguous` on restart (their truth is the entry). The
//! dispatcher's acknowledgment settles the record (`automation_outcome`:
//! `committed` with the card, `rejected` with a closed code). Withdrawing
//! the choice, revoking the client or disabling the feature rejects the
//! client's accepted triggers and retires its inbox items, under
//! `storage::settings_fence` so the scheduler's firing-intent transaction,
//! which re-reads `automation_delegations` under the same fence, never
//! starts a send on a withdrawn delegation. A ledger holding any of this
//! is written at `STATE_VERSION_AUTOMATIONS` (sticky).
//!
//! Worker cards (`workers.rs`, `docs/mcp.md` "Worker cards"): a client with
//! the further local choice `allow_workers` may drive the cards its own
//! Agent triggers start (recorded at the trigger's commit, `record_worker`;
//! never any other card). `deck_worker_list` / `deck_worker_wait` are
//! content-free views (alive, the Agent observation's closed word and
//! episode, the human hold, the queue states of its lines); a wait is
//! bounded by `MAX_WAIT_MS` and holds no lock. `deck_worker_reply` needs
//! `allow_replies` too and returns the Claude Code final message the Stop
//! hook forwarded (memory only, `turn_summary::reply_for`).
//! `deck_worker_send` is bound to the client's `send_sequence` exactly as a
//! trigger is to its own, and refuses a line whose `after_episode` is no
//! longer the card's (`EPISODE_MOVED`): an observation may refuse a send,
//! never release one. An accepted line becomes one scheduler row through
//! `scheduler::worker_queue_add` (external admission, content class
//! `delegated`, authority resting on the client's worker key in
//! `automation_delegations`); every hold then decides when it goes, the
//! human hold included. `deck_worker_start` opens a card of one of the
//! client's worker types (the user's commands; Codex only with
//! `--no-daemon`) through a `worker-start` Board intent the webview commits;
//! its first line is the run's head row, sent without interaction evidence
//! only while the client's worker first-send choice stands. Every choice of a
//! client is saved at once (`mcp_client_permissions`, under the settings
//! fence). The user takes a card over by typing into its pane
//! (`worker_set_held`, from the webview) and hands it back by hand only.
//! Withdrawing `allow_workers`, revoking the client or disabling MCP takes
//! the key out under `storage::settings_fence`. Capacity: 8 worker cards on
//! the Board per orchestrator card (callers from no card together), 32 per
//! client (a further start is `CAPACITY_EXCEEDED`), 4
//! unsent lines per card, 30 lines a minute. A ledger holding any of this is
//! written at `STATE_VERSION_WORKERS` (sticky).
//!
//! Drafted configuration (`proposals.rs`, `docs/mcp.md` "Drafted
//! configuration"): a client with the separate local choice `allow_drafts`
//! may describe its projects' templates and Agent rules and PROPOSE a
//! template and/or one Agent handover rule (validate-only first: every
//! problem names its field and its fix). A proposal is a draft: it waits in
//! the ledger, Needs attention shows it, and only the user's click there
//! writes it through the webview's Board transaction and settings writer,
//! the click signing the rule's approval exactly as a tick in the editor
//! does. The client never approves or applies anything; it reads the
//! decision (`deck_config_proposal_get`). Withdrawing the choice or revoking
//! the client withdraws what waits. A ledger holding a proposal or the
//! choice is written at `STATE_VERSION_PROPOSALS` (sticky).
//!
//! Slack mentions (`mentions.rs`, `docs/mcp.md` "Slack mentions"): a client
//! with the separate local choice `allow_mentions` (account-wide) reads the
//! open mentions the Slack extension already keeps in memory
//! (`deck_mentions_list`, a fenced 200-character preview, a cursor and a
//! wait of at most 5 s; `deck_mention_read`, the whole text and context,
//! fenced) — Deck makes no Slack call for it and never hands out a list
//! that may be stale (`SOURCE_UNAVAILABLE`). With `allow_automations` too it
//! may hand ONE open mention to one of its Agent rules
//! (`deck_mention_handoff`): the acceptance of `deck_automation_trigger`,
//! the item filled in by Deck from the mention itself, the mention leaving
//! the user's list as handed to this client and rule (a skipped item gives
//! it back). Never a Slack badge rule, never a dismissal, nothing written to
//! Slack. A ledger holding the choice is written at
//! `STATE_VERSION_MENTIONS` (sticky).
//!
//! Orchestrator cards (`origin.rs`, `docs/mcp.md` "Orchestrator cards"):
//! several agents may share one client, each in its own Deck card. Deck
//! reads which card a call comes from out of the kernel (the adapter's peer
//! pid and one process-table snapshot, checked as an Agent Signal's origin
//! is, `agent_origin`), never from anything the client says. That card is
//! the call's orchestrator card: the worker cards it starts open in its
//! column at that moment (unless the user chose the type's or rule's column,
//! `Client.rule_column`), are recorded as its own (`Worker.orchestrator`),
//! and only it and callers from no card see and drive them; the client's
//! unowned workers stay everyone's. Trigger and send sequences are kept per
//! orchestrator card (`Client.lanes`). A worker whose orchestrator card left
//! the Board is the user's (its lines hold at `human`) until handed back,
//! which makes it unowned. The proof only groups and narrows inside the
//! client's own authority; it grants nothing and moves no card. A ledger
//! holding any of it is written at `STATE_VERSION_ORCHESTRATORS` (sticky).
//!
//! Layout (one contract, one file per concern; every file starts with
//! `use super::*` and exposes its items `pub(super)`): `state` holds the disk
//! document, its records, the runtime and load/validate/compact/save;
//! `grants` the client/scope/execution-grant checks; `operations` the
//! operation records, stable error codes, reservation and emergency fences;
//! `runner` the pane runner's private control channel; `wire` the request
//! envelope and argument shapes; `project` list/read/search; `session` the
//! session-level requests including close; `automation` the automation
//! delegation above; `workers` the worker cards; `origin` the orchestrator
//! card of a call; `proposals` the drafted
//! configuration; `jobs` exec, job reads and every
//! job side effect; `control` request routing plus the socket thread; and
//! `commands` the local Tauri commands and the guards other modules call
//! (`spawn` registers `guard_server_restart` with `tmux_lifecycle`, which
//! never names this module).
//! Limits and the small helpers below are shared by all of them.

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

use crate::error::{DeckError, ErrorKind};
use crate::ledger::{random_id, sha};
use crate::sync::LockRecover;

pub(crate) const INTEGRATION: crate::extensions::AccessIntegration =
    crate::extensions::AccessIntegration {
        id: "mcp",
        start: spawn,
    };

#[cfg(test)]
mod admission_tests;
mod automation;
mod commands;
mod control;
mod grants;
mod jobs;
mod mentions;
mod operations;
mod origin;
pub(crate) use origin::column_for_item as worker_column_for_item;
mod project;
mod proposals;
mod runner;
mod session;
mod state;
#[cfg(test)]
mod tests;
mod wire;
mod workers;

use automation::*;
pub(crate) use automation::{automation_delegations, automation_outcome};
pub(crate) use commands::*;
pub(crate) use control::*;
use grants::*;
use jobs::*;
use mentions::*;
use operations::*;
use project::*;
use proposals::*;
use runner::*;
use session::*;
use state::*;
use wire::*;
use workers::*;
pub(crate) use workers::{
    held_worker_sessions, note_worker_change, replies_wanted_any, replies_wanted_for,
    worker_card_held, worker_client_for_item, worker_delegation_key, worker_first_send_key,
    WorkerCardView, WorkerType, WORKER_TYPE_RULE_PREFIX,
};

const STATE_VERSION: u32 = 6;
/// The sticky version a ledger takes once it holds automation delegation
/// (`automation::uses_automations`): a v6 build refuses it untouched.
const STATE_VERSION_AUTOMATIONS: u32 = 7;
/// The sticky version a ledger takes once it holds worker cards or the
/// choices that drive them (`workers::uses_workers`): a v7 build refuses it.
const STATE_VERSION_WORKERS: u32 = 8;
/// The sticky version a ledger takes once it holds a drafted proposal or
/// the draft choice (`proposals::uses_proposals`): a v8 build refuses it.
const STATE_VERSION_PROPOSALS: u32 = 9;
/// The sticky version a ledger takes once a client may read Slack mentions
/// (`mentions::uses_mentions`): a v9 build refuses it.
const STATE_VERSION_MENTIONS: u32 = 10;
/// The sticky version a ledger takes once it groups worker cards by
/// orchestrator card (`origin::uses_orchestrators`): a v10 build refuses it.
const STATE_VERSION_ORCHESTRATORS: u32 = 11;
/// The sticky version a ledger takes once a client reaches all projects
/// (`grants::uses_all_projects`): a v11 build refuses it.
const STATE_VERSION_ALL_PROJECTS: u32 = 12;
const CONTROL_PROTOCOL: u32 = 5;
const DECK_VERSION: &str = env!("CARGO_PKG_VERSION");
const DECK_BUILD: Option<&str> = match option_env!("DECK_BUILD_SHA") {
    Some(value) => Some(value),
    None => option_env!("GITHUB_SHA"),
};
const MAX_STATE_BYTES: usize = 8 * 1024 * 1024;
const MAX_REQUEST_BYTES: usize = 256 * 1024;
/// One response line INCLUDING its trailing newline (adapter and runner
/// compare the same way; `mcp-fixtures/limits.json`).
const MAX_RESPONSE_BYTES: usize = 128 * 1024;
const MAX_CLIENTS: usize = 32;
const MAX_PROJECTS_PER_CLIENT: usize = 64;
const MAX_ROOTS_PER_PROJECT: usize = 16;
const MAX_SESSIONS: usize = 64;
const MAX_OPERATIONS: usize = 2000;
/// Room for control records. Only a session's latest control change is kept
/// (older ones are superseded by the control sequence), so one slot per
/// possible session always suffices: request, renew and release — the way out
/// of a full journal — never compete with ordinary records.
const CONTROL_RESERVE: usize = MAX_SESSIONS;
/// Slots only `deck_job_interrupt` may use beyond the ordinary pool: stopping
/// work does not fail because ordinary requests filled the journal.
const INTERRUPT_RESERVE: usize = 64;
/// Interrupt records one client may hold once the ordinary pool is full, so a
/// single client cannot drain the interrupt reserve of every other client.
const INTERRUPT_RESERVE_PER_CLIENT: usize = 16;
/// Ordinary records (everything except control records and reserve
/// interrupts) across all clients.
const ORDINARY_OPERATIONS: usize = MAX_OPERATIONS - CONTROL_RESERVE - INTERRUPT_RESERVE;
/// One client may hold at most this many non-control journal entries.
const MAX_OPERATIONS_PER_CLIENT: usize = 500;
/// Terminal session-create/close records kept per client so their results
/// stay queryable. Replay safety does not depend on this window: creates are
/// bound to the client's create sequence, closes to their session's epoch.
const CREATE_REPLAY_WINDOW: usize = 32;
/// Job bindings kept per session (Deck's own bound). The runner keeps its
/// own, independent per-process cap (256 jobs) and retires its oldest
/// finished jobs against that; the two numbers are not the same limit.
const MAX_JOBS_PER_SESSION: usize = 64;
const MAX_JOBS: usize = MAX_SESSIONS * MAX_JOBS_PER_SESSION;
const MAX_GRANTS: usize = MAX_SESSIONS + MAX_JOBS;
const MAX_CONNECTIONS: usize = 32;
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);
/// The whole first request must arrive within this of the connection being
/// served (the adapter writes it at once), so idle same-uid peers release
/// their connection slot quickly instead of holding it for the read timeout.
const REQUEST_DEADLINE: Duration = Duration::from_millis(500);
const MAX_AUDIT_EVENTS: usize = 2_000;
const AUDIT_RETENTION_MS: u64 = 30 * 24 * 60 * 60_000;
const MAX_EXECUTABLE_BYTES: usize = 4 * 1024;
const MAX_ARGUMENTS: usize = 256;
const MAX_ARGUMENT_BYTES: usize = 64 * 1024;
const MAX_READ_BYTES: usize = 16 * 1024;
const MAX_INPUT_BYTES: usize = 32 * 1024;
const DEFAULT_WAIT_MS: u64 = 1_000;
const MAX_WAIT_MS: u64 = 5_000;
const MIN_LEASE_MS: u64 = 1_000;
const DEFAULT_LEASE_MS: u64 = 60_000;
const MAX_LEASE_MS: u64 = 5 * 60_000;
const DEFAULT_EXECUTION_GRANT_MS: u64 = 15 * 60_000;
const MAX_EXECUTION_GRANT_MS: u64 = 8 * 60 * 60_000;
const MIN_OUTPUT_RETENTION_MS: u64 = 60_000;
const DEFAULT_OUTPUT_RETENTION_MS: u64 = 24 * 60 * 60_000;
const MAX_OUTPUT_RETENTION_MS: u64 = 7 * 24 * 60 * 60_000;
const POLICY_VERSION: u32 = 2;
/// The environment every new Full Local grant is issued under: the job
/// inherits the runner's own environment — the Deck tmux session baseline —
/// minus Deck's control-plane coordinates (`mcp-runner` `spawn_job`).
const ENVIRONMENT_PROFILE: &str = "host-terminal-env-v1";
/// Every profile a stored grant may carry (closed). `developer-sanitized-v1`
/// is the pre-CE1b sanitized environment: such grants still load as
/// history, are never rewritten and never authorize (`grants::standing_at`).
const KNOWN_ENVIRONMENT_PROFILES: [&str; 2] = [ENVIRONMENT_PROFILE, "developer-sanitized-v1"];

fn now_ms() -> u64 {
    let wall = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis() as u64)
        .unwrap_or(0);
    #[cfg(test)]
    let wall = wall.saturating_add(test_clock::skew());
    wall
}

fn secret_hash_matches(expected: &str, actual: &str) -> bool {
    if expected.len() != actual.len() {
        return false;
    }
    expected
        .bytes()
        .zip(actual.bytes())
        .fold(0u8, |difference, (left, right)| difference | (left ^ right))
        == 0
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn valid_title(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= 120
        && !value.chars().any(|character| character.is_control())
}

/// Injectable wall clock for tests: a per-thread forward skew, so a test can
/// move time without sleeping and without touching other tests.
#[cfg(test)]
mod test_clock {
    use std::cell::Cell;

    thread_local! {
        static SKEW_MS: Cell<u64> = const { Cell::new(0) };
    }

    pub(super) fn skew() -> u64 {
        SKEW_MS.with(Cell::get)
    }

    pub(super) fn advance(ms: u64) {
        SKEW_MS.with(|skew| skew.set(skew.get().saturating_add(ms)));
    }
}
