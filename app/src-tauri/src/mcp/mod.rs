//! Local MCP structured-read/control service and durable authorization ledger.
//!
//! This is deliberately separate from Phone Connector: its socket, client
//! records, project scopes, short-lived execution grants, operation ids,
//! sessions, and jobs are disjoint. Creating a session never grants execution;
//! only the local Tauri command may create or expand an execution window.
//! Authorization roots are canonicalized directories before the UI confirms
//! them. Client deletion is available only after revocation has fenced live
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
//! reports its session's grant `revoked` from the fence on (read before the
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
//! The control socket is bound under a private temporary name, made 0600, and
//! atomically renamed into place; Deck never changes its process-wide umask.
//!
//! Layout (one contract, one file per concern; every file starts with
//! `use super::*` and exposes its items `pub(super)`): `state` holds the disk
//! document, its records, the runtime and load/validate/compact/save;
//! `grants` the client/scope/execution-grant checks; `operations` the
//! operation records, stable error codes, reservation and emergency fences;
//! `runner` the pane runner's private control channel; `wire` the request
//! envelope and argument shapes; `project` list/read/search; `session` the
//! session-level requests including close; `jobs` exec, job reads and every
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

#[cfg(test)]
mod admission_tests;
mod commands;
mod control;
mod grants;
mod jobs;
mod operations;
mod project;
mod runner;
mod session;
mod state;
#[cfg(test)]
mod tests;
mod wire;

pub(crate) use commands::*;
pub(crate) use control::*;
use grants::*;
use jobs::*;
use operations::*;
use project::*;
use runner::*;
use session::*;
use state::*;
use wire::*;

const STATE_VERSION: u32 = 6;
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
const ENVIRONMENT_PROFILE: &str = "developer-sanitized-v1";

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
