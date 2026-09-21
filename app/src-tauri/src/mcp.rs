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
//! enabled and is removed on disable. The thin `deck-mcp` sidecar maps an
//! absent control socket to `FEATURE_DISABLED`; when connected, the service
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
//! credentials. Closing its tmux pane invokes the runner's SIGHUP cleanup.
//! Local-command failures are stable machine codes (`mcp-*`) the webview maps
//! to one sentence each.
//! The control socket is bound under a private temporary name, made 0600, and
//! atomically renamed into place; Deck never changes its process-wide umask.

use base64::Engine;
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

use crate::error::{DeckError, ErrorKind};
use crate::sync::LockRecover;

const STATE_VERSION: u32 = 6;
const CONTROL_PROTOCOL: u32 = 5;
const DECK_VERSION: &str = env!("CARGO_PKG_VERSION");
const DECK_BUILD: Option<&str> = match option_env!("DECK_BUILD_SHA") {
    Some(value) => Some(value),
    None => option_env!("GITHUB_SHA"),
};
const MAX_STATE_BYTES: usize = 8 * 1024 * 1024;
const MAX_REQUEST_BYTES: usize = 256 * 1024;
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
/// Job bindings kept per session; the runner retires jobs the same way.
const MAX_JOBS_PER_SESSION: usize = 64;
const MAX_JOBS: usize = MAX_SESSIONS * MAX_JOBS_PER_SESSION;
const MAX_GRANTS: usize = MAX_SESSIONS + MAX_JOBS;
const MAX_CONNECTIONS: usize = 32;
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_AUDIT_EVENTS: usize = 2_000;
const AUDIT_RETENTION_MS: u64 = 30 * 24 * 60 * 60_000;
const MAX_EXECUTABLE_BYTES: usize = 4 * 1024;
const MAX_ARGUMENTS: usize = 256;
const MAX_ARGUMENT_BYTES: usize = 64 * 1024;
const MAX_READ_BYTES: usize = 16 * 1024;
const MAX_INPUT_BYTES: usize = 32 * 1024;
const DEFAULT_LEASE_MS: u64 = 60_000;
const MAX_LEASE_MS: u64 = 5 * 60_000;
const DEFAULT_EXECUTION_GRANT_MS: u64 = 15 * 60_000;
const MAX_EXECUTION_GRANT_MS: u64 = 8 * 60 * 60_000;
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

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
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

fn random_id(prefix: &str) -> Result<String, DeckError> {
    let mut bytes = [0u8; 16];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| DeckError::new(ErrorKind::Other, "secure random unavailable"))?;
    Ok(format!(
        "{prefix}{}",
        bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    ))
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

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Config {
    enabled: bool,
    clients: Vec<Client>,
    #[serde(default = "default_output_retention_ms")]
    output_retention_ms: u64,
}

fn default_output_retention_ms() -> u64 {
    DEFAULT_OUTPUT_RETENTION_MS
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: false,
            clients: Vec::new(),
            output_retention_ms: DEFAULT_OUTPUT_RETENTION_MS,
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Client {
    id: String,
    name: String,
    #[serde(default)]
    credential_hash: String,
    #[serde(default)]
    credential_version: u64,
    revoked_at: Option<u64>,
    allow_create: bool,
    projects: Vec<ProjectScope>,
    /// Creates this client has had accepted. A create must name the current
    /// value, so a create whose record was retired can never be accepted again.
    #[serde(default)]
    create_sequence: u64,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProjectScope {
    project_id: String,
    roots: Vec<String>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ManagedSession {
    session_id: String,
    card_id: String,
    tmux_session: String,
    project_id: String,
    title: String,
    cwd: String,
    generation: String,
    runner_socket: String,
    owner_client_id: String,
    control_owner: Option<String>,
    #[serde(default)]
    control_holder: Option<String>,
    control_epoch: u64,
    lease_expires_at: Option<u64>,
    human_lock: bool,
    #[serde(default)]
    output_shared: bool,
    #[serde(default)]
    closing: bool,
    created_at: u64,
    /// Accepted MCP control changes (request, renew, release). A control
    /// request must name the current value; see `session_control`.
    #[serde(default)]
    control_sequence: u64,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Operation {
    operation_id: String,
    client_id: String,
    request_id: String,
    request_hash: String,
    kind: String,
    state: String,
    code: Option<String>,
    result: Option<Value>,
    accepted_at: u64,
    updated_at: u64,
    #[serde(default)]
    admission_hash: Option<String>,
    /// Session this request targeted (the new session for a create).
    #[serde(default)]
    session_id: Option<String>,
    /// Control epoch the request was bound to. A terminal record whose epoch
    /// is older than the session's current epoch can be retired: any replay
    /// of it fails `check_control` deterministically.
    #[serde(default)]
    control_epoch: Option<u64>,
    /// For a control record: the session's control sequence this change
    /// produced. A record whose sequence is no longer current is superseded.
    #[serde(default)]
    control_sequence: Option<u64>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct JobBinding {
    job_id: String,
    client_id: String,
    session_id: String,
    session_generation: String,
    request_hash: String,
    operation_id: String,
    #[serde(default)]
    grant_id: String,
    #[serde(default)]
    grant_version: u64,
    #[serde(default)]
    allow_output: bool,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExecutionGrant {
    grant_id: String,
    client_id: String,
    credential_version: u64,
    project_id: String,
    session_id: String,
    session_generation: String,
    profile: String,
    environment_profile: String,
    environment_profile_version: u32,
    issued_at: u64,
    expires_at: u64,
    issued_monotonic_ms: u64,
    duration_ms: u64,
    allow_stdin: bool,
    allow_output: bool,
    /// Accepted only so v6 files written before shell fallback was removed
    /// continue to load. It is ignored and omitted on the next save.
    #[serde(default, rename = "allowShell", skip_serializing)]
    _legacy_allow_shell: Option<bool>,
    grant_version: u64,
    revocation_version: u64,
    service_instance: String,
    revoked_at: Option<u64>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AuditEvent {
    event_id: String,
    at: u64,
    kind: String,
    principal_id: Option<String>,
    session_id: Option<String>,
    operation_id: Option<String>,
    job_id: Option<String>,
    grant_id: Option<String>,
    reason_code: Option<String>,
    policy_version: u32,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DiskDoc {
    version: u32,
    config: Config,
    sessions: Vec<ManagedSession>,
    operations: Vec<Operation>,
    jobs: Vec<JobBinding>,
    #[serde(default)]
    execution_grants: Vec<ExecutionGrant>,
    #[serde(default)]
    audit: Vec<AuditEvent>,
}

impl Default for DiskDoc {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            config: Config::default(),
            sessions: Vec::new(),
            operations: Vec::new(),
            jobs: Vec::new(),
            execution_grants: Vec::new(),
            audit: Vec::new(),
        }
    }
}

#[derive(Default)]
struct AuditLink<'a> {
    principal_id: Option<&'a str>,
    session_id: Option<&'a str>,
    operation_id: Option<&'a str>,
    job_id: Option<&'a str>,
    grant_id: Option<&'a str>,
    reason_code: Option<&'a str>,
}

fn audit(doc: &mut DiskDoc, kind: &str, link: AuditLink<'_>) -> Result<(), DeckError> {
    let cutoff = now_ms().saturating_sub(AUDIT_RETENTION_MS);
    doc.audit.retain(|event| event.at >= cutoff);
    if doc.audit.len() >= MAX_AUDIT_EVENTS {
        doc.audit.remove(0);
    }
    doc.audit.push(AuditEvent {
        event_id: random_id("audit_")?,
        at: now_ms(),
        kind: kind.into(),
        principal_id: link.principal_id.map(str::to_owned),
        session_id: link.session_id.map(str::to_owned),
        operation_id: link.operation_id.map(str::to_owned),
        job_id: link.job_id.map(str::to_owned),
        grant_id: link.grant_id.map(str::to_owned),
        reason_code: link.reason_code.map(str::to_owned),
        policy_version: POLICY_VERSION,
    });
    Ok(())
}

struct Runtime {
    app: Option<AppHandle>,
    path: PathBuf,
    socket: PathBuf,
    doc: Mutex<Result<DiskDoc, DeckError>>,
    io: Mutex<()>,
    /// Fences every terminal/Board side-effect dispatch against control
    /// transfer, revoke, disable, and close admission.
    delivery: Mutex<()>,
    emergency: Mutex<EmergencyFences>,
    service_instance: String,
    /// Per-runner authentication keys exist only in this Deck process.
    runner_auth: Mutex<HashMap<String, String>>,
    started: Instant,
}

#[derive(Default)]
struct EmergencyFences {
    disabled: bool,
    clients: HashSet<String>,
    human_sessions: HashSet<String>,
    /// Execution revocations that have set their fence and not yet finished
    /// their locked section, counted per session (two may overlap). Only the
    /// revocation itself lowers its count, so an approval that ran while it
    /// was pending cannot lift it.
    execution_pending: HashMap<String, usize>,
    /// Execution revocations whose persistence failed. Only a later local
    /// approval — ordered after it by the delivery lock — lifts this.
    execution_unpersisted: HashSet<String>,
}

impl EmergencyFences {
    fn execution_fenced(&self, session_id: &str) -> bool {
        self.execution_pending
            .get(session_id)
            .is_some_and(|count| *count > 0)
            || self.execution_unpersisted.contains(session_id)
    }
}

/// What a side effect needs besides client, feature and human fences.
#[derive(Clone, Copy, PartialEq)]
enum Admission {
    /// Control, read, interrupt and close: an execution window is not part of
    /// their authority, so its revocation does not refuse them.
    Control,
    /// exec and stdin: a pending execution revocation also refuses them.
    Execution,
}

impl Runtime {
    fn monotonic_ms(&self) -> u64 {
        self.started.elapsed().as_millis().min(u64::MAX as u128) as u64
    }
}

/// Deterministic interleaving for tests: a test arms a named point of one
/// runtime (keyed by its state path), the production path parks there until
/// released. Compiled out of every non-test build.
#[cfg(test)]
mod pause {
    use crate::sync::LockRecover;
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::sync::{Mutex, OnceLock};
    use std::time::Duration;

    type Slot = (Sender<()>, Receiver<()>);

    fn points() -> &'static Mutex<HashMap<(PathBuf, &'static str), Slot>> {
        static POINTS: OnceLock<Mutex<HashMap<(PathBuf, &'static str), Slot>>> = OnceLock::new();
        POINTS.get_or_init(Default::default)
    }

    /// Park the next arrival at `point`; returns (entered, release).
    pub(super) fn arm(path: &Path, point: &'static str) -> (Receiver<()>, Sender<()>) {
        let (entered_tx, entered_rx) = channel();
        let (release_tx, release_rx) = channel();
        points()
            .lock_or_recover()
            .insert((path.to_owned(), point), (entered_tx, release_rx));
        (entered_rx, release_tx)
    }

    pub(super) fn reach(path: &Path, point: &'static str) {
        let slot = points().lock_or_recover().remove(&(path.to_owned(), point));
        if let Some((entered, release)) = slot {
            entered.send(()).unwrap();
            release
                .recv_timeout(Duration::from_secs(10))
                .expect("paused point was never released");
        }
    }
}

#[cfg(test)]
fn pause_point(runtime: &Runtime, point: &'static str) {
    pause::reach(&runtime.path, point);
}

#[cfg(not(test))]
fn pause_point(_runtime: &Runtime, _point: &'static str) {}

fn emit_changed(runtime: &Runtime) {
    if let Some(app) = &runtime.app {
        let _ = app.emit("mcp-changed", ());
    }
}

static RUNTIME: OnceLock<Arc<Runtime>> = OnceLock::new();

fn load(path: &Path) -> Result<DiskDoc, DeckError> {
    let mut bytes = Vec::new();
    match std::fs::File::open(path) {
        Ok(file) => {
            file.take((MAX_STATE_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
                .map_err(|error| {
                    DeckError::new(ErrorKind::io(error.kind()), "MCP state could not be read")
                })?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(DiskDoc::default()),
        Err(error) => {
            return Err(DeckError::new(
                ErrorKind::io(error.kind()),
                "MCP state could not be read",
            ));
        }
    }
    if bytes.len() > MAX_STATE_BYTES {
        return Err(DeckError::new(
            ErrorKind::Recovery,
            "MCP state exceeds its bound",
        ));
    }
    let mut doc: DiskDoc = serde_json::from_slice(&bytes)
        .map_err(|_| DeckError::new(ErrorKind::Recovery, "MCP state is unreadable"))?;
    if doc.version == 1 {
        // v1 clients combined project access and host execution. Preserve the
        // non-secret display records for local review, but fail closed: no old
        // client, lease, pending write, or session receives a v2 execution
        // window implicitly.
        doc.version = STATE_VERSION;
        doc.config.enabled = false;
        for client in &mut doc.config.clients {
            client.revoked_at.get_or_insert_with(now_ms);
        }
        for session in &mut doc.sessions {
            session.control_owner = None;
            session.control_holder = None;
            session.control_epoch = session.control_epoch.saturating_add(1);
            session.lease_expires_at = None;
            session.human_lock = true;
        }
        for operation in &mut doc.operations {
            if matches!(operation.state.as_str(), "accepted" | "executing") {
                operation.state = "ambiguous".into();
                operation.code = Some("v2-reauthorization-required".into());
                operation.updated_at = now_ms();
            }
        }
        doc.execution_grants.clear();
        validate_doc(&doc)?;
        save(path, &doc)?;
        return Ok(doc);
    }
    if doc.version == 2 {
        doc.version = STATE_VERSION;
        doc.config.enabled = false;
        for client in &mut doc.config.clients {
            client.revoked_at.get_or_insert_with(now_ms);
            client.credential_hash.clear();
            client.credential_version = 0;
        }
        for session in &mut doc.sessions {
            session.control_owner = None;
            session.control_holder = None;
            session.control_epoch = session.control_epoch.saturating_add(1);
            session.lease_expires_at = None;
            session.human_lock = true;
        }
        for operation in &mut doc.operations {
            if matches!(
                operation.state.as_str(),
                "accepted" | "executing" | "admitted"
            ) {
                operation.state = "ambiguous".into();
                operation.code = Some("v3-reauthorization-required".into());
                operation.updated_at = now_ms();
            }
        }
        doc.execution_grants.clear();
        validate_doc(&doc)?;
        save(path, &doc)?;
        return Ok(doc);
    }
    let mut changed = false;
    if doc.version == 3 {
        // v4 adds per-operation session/epoch binding (absent on v3 records,
        // which are retired only when their session closes) and canonical
        // request fingerprints. The bump is sticky: v3 builds refuse v4 state
        // untouched instead of dropping the binding fields.
        doc.version = STATE_VERSION;
        changed = true;
    }
    if doc.version == 4 {
        // v5 adds the per-session control sequence and per-client create
        // sequence (both start at 0; nothing else changes). Sticky: a v4
        // build refuses v5 state untouched, because dropping the sequences
        // would let a retired request be accepted again.
        doc.version = STATE_VERSION;
        changed = true;
    }
    if doc.version == 5 {
        // v6 used to add a separate arbitrary-shell permission. The fallback
        // has since been removed; the sticky version remains so older builds
        // still refuse newer state, and legacy allowShell fields are ignored.
        doc.version = STATE_VERSION;
        changed = true;
    }
    validate_doc(&doc)?;
    for operation in &mut doc.operations {
        // Nothing accepted before a restart is replayed — not even a Board
        // create or close. Its outcome is unknown, so it is ambiguous; the
        // client inspects state and decides under a new request id.
        if matches!(
            operation.state.as_str(),
            "accepted" | "executing" | "admitted"
        ) {
            operation.state = "ambiguous".into();
            operation.code = Some("deck-restarted".into());
            operation.updated_at = now_ms();
            changed = true;
        }
    }
    for session in &mut doc.sessions {
        if session.closing {
            session.closing = false;
            changed = true;
        }
    }
    for session in &mut doc.sessions {
        if session.control_owner.is_some() {
            session.control_owner = None;
            session.control_holder = None;
            session.control_epoch = session.control_epoch.saturating_add(1);
            session.lease_expires_at = None;
            changed = true;
        }
    }
    changed |= compact(&mut doc, None);
    if changed {
        save(path, &doc)?;
    }
    Ok(doc)
}

/// Retire journal records whose replay can no longer have an effect, and
/// execution grants that can no longer authorize anything. Never touches a
/// non-terminal operation. Returns whether anything was removed.
///
/// Request identity (the protocol promise, see `docs/mcp.md`): a request is
/// (client, request_id) plus its canonical argument fingerprint, and every
/// side effect is bound to a server-issued value that only moves forward:
/// * exec, input, interrupt, renew, release and close name the session's
///   control epoch; a record is kept while its session exists and that epoch
///   is current, and once the epoch advances (or the session closes) a replay
///   fails the epoch/generation check — it is never executed again;
/// * control request/renew/release name the session's control sequence;
///   only the record of the latest change is kept, and a replay of an older
///   one fails the sequence check (`STALE_REQUEST`);
/// * a create names the client's create sequence; the last
///   `CREATE_REPLAY_WINDOW` results per client stay queryable, and an older
///   create fails the sequence check;
/// * a close is also kept while its session exists at the epoch it named,
///   whatever the window, so an ambiguous close is never re-admitted.
///
/// `current_service` (when known) also retires grants issued by an earlier
/// Deck process: they can never be active again.
fn compact(doc: &mut DiskDoc, current_service: Option<&str>) -> bool {
    let before = (
        doc.operations.len(),
        doc.execution_grants.len(),
        doc.jobs.len(),
    );
    let sessions = doc
        .sessions
        .iter()
        .map(|session| {
            (
                session.session_id.clone(),
                (session.control_epoch, session.control_sequence),
            )
        })
        .collect::<HashMap<_, _>>();
    let terminal = |operation: &Operation| {
        matches!(
            operation.state.as_str(),
            "committed" | "rejected" | "ambiguous"
        )
    };
    let bound_live = |operation: &Operation| {
        operation
            .session_id
            .as_deref()
            .and_then(|id| sessions.get(id))
            .is_some_and(|(epoch, _)| operation.control_epoch.is_some_and(|bound| bound >= *epoch))
    };
    let mut creates_kept = HashMap::<String, usize>::new();
    let mut keep = vec![true; doc.operations.len()];
    for (index, operation) in doc.operations.iter().enumerate().rev() {
        if !terminal(operation) {
            continue;
        }
        if matches!(operation.kind.as_str(), "session-create" | "session-close") {
            // Board results stay queryable for a bounded per-client window,
            // even after their session is gone.
            let kept = creates_kept
                .entry(format!("{}\0{}", operation.client_id, operation.kind))
                .or_default();
            *kept += 1;
            keep[index] = *kept <= CREATE_REPLAY_WINDOW
                || (operation.kind == "session-close" && bound_live(operation));
            continue;
        }
        let Some(session_id) = operation.session_id.as_deref() else {
            // Pre-v4 record: retire only once its named session is gone; a
            // record that names no session is kept (it cannot be judged).
            let target = operation
                .result
                .as_ref()
                .and_then(|result| result.get("sessionId"))
                .and_then(Value::as_str);
            keep[index] = target.is_none_or(|id| sessions.contains_key(id));
            continue;
        };
        keep[index] = match sessions.get(session_id) {
            None => false,
            Some((epoch, sequence)) => {
                operation.control_epoch.is_none_or(|bound| bound >= *epoch)
                    && operation
                        .control_sequence
                        .is_none_or(|produced| produced >= *sequence)
            }
        };
    }
    let mut flags = keep.into_iter();
    doc.operations.retain(|_| flags.next().unwrap_or(true));
    doc.jobs
        .retain(|job| sessions.contains_key(&job.session_id));
    let referenced = doc
        .jobs
        .iter()
        .map(|job| job.grant_id.clone())
        .collect::<HashSet<_>>();
    let mut newest = HashSet::new();
    let mut keep_grants = vec![true; doc.execution_grants.len()];
    for (index, grant) in doc.execution_grants.iter().enumerate().rev() {
        let latest = newest.insert(grant.session_id.clone());
        let live_service = current_service.is_none_or(|service| grant.service_instance == service);
        keep_grants[index] = sessions.contains_key(&grant.session_id)
            && live_service
            && (latest || referenced.contains(&grant.grant_id));
    }
    let mut flags = keep_grants.into_iter();
    doc.execution_grants
        .retain(|_| flags.next().unwrap_or(true));
    before
        != (
            doc.operations.len(),
            doc.execution_grants.len(),
            doc.jobs.len(),
        )
}

fn validate_doc(doc: &DiskDoc) -> Result<(), DeckError> {
    let mut client_ids = HashSet::new();
    let clients_valid = doc.version == STATE_VERSION
        && (60_000..=MAX_OUTPUT_RETENTION_MS).contains(&doc.config.output_retention_ms)
        && doc.config.clients.len() <= MAX_CLIENTS
        && doc.config.clients.iter().all(|client| {
            valid_id(&client.id)
                && client_ids.insert(client.id.clone())
                && valid_title(&client.name)
                && ((client.credential_version > 0 && client.credential_hash.len() == 64)
                    || (client.revoked_at.is_some()
                        && client.credential_version == 0
                        && client.credential_hash.is_empty()))
                && client.projects.len() <= MAX_PROJECTS_PER_CLIENT
                && client.projects.iter().all(|project| {
                    valid_id(&project.project_id)
                        && project.roots.len() <= MAX_ROOTS_PER_PROJECT
                        && project
                            .roots
                            .iter()
                            .all(|root| Path::new(root).is_absolute())
                })
        });
    let mut session_ids = HashSet::new();
    let sessions_valid = doc.sessions.len() <= MAX_SESSIONS
        && doc.sessions.iter().all(|session| {
            valid_id(&session.session_id)
                && session_ids.insert(session.session_id.clone())
                && valid_id(&session.card_id)
                && valid_id(&session.project_id)
                && valid_id(&session.generation)
                && valid_id(&session.owner_client_id)
                && session
                    .control_owner
                    .as_ref()
                    .is_none_or(|owner| valid_id(owner))
                && session
                    .control_holder
                    .as_ref()
                    .is_none_or(|holder| valid_id(holder))
                && Path::new(&session.cwd).is_absolute()
                && Path::new(&session.runner_socket).is_absolute()
        });
    let mut operation_ids = HashSet::new();
    let mut request_keys = HashSet::new();
    let operations_valid = doc.operations.len() <= MAX_OPERATIONS
        && doc.operations.iter().all(|operation| {
            valid_id(&operation.operation_id)
                && operation_ids.insert(operation.operation_id.clone())
                && request_keys.insert((operation.client_id.clone(), operation.request_id.clone()))
                && valid_id(&operation.client_id)
                && valid_id(&operation.request_id)
                && operation.session_id.as_deref().is_none_or(valid_id)
                && operation.request_hash.len() == 64
                && matches!(
                    operation.state.as_str(),
                    "accepted" | "executing" | "admitted" | "committed" | "rejected" | "ambiguous"
                )
        });
    let mut job_ids = HashSet::new();
    let jobs_valid = doc.jobs.len() <= MAX_JOBS
        && doc.jobs.iter().all(|job| {
            valid_id(&job.job_id)
                && job_ids.insert(job.job_id.clone())
                && valid_id(&job.client_id)
                && session_ids.contains(&job.session_id)
                // The exec record may already be retired (older epoch); the
                // binding stays readable for its session's lifetime.
                && valid_id(&job.operation_id)
        });
    let mut grant_ids = HashSet::new();
    let grants_valid = doc.execution_grants.len() <= MAX_GRANTS
        && doc.execution_grants.iter().all(|grant| {
            valid_id(&grant.grant_id)
                && grant_ids.insert(grant.grant_id.clone())
                && valid_id(&grant.client_id)
                && valid_id(&grant.project_id)
                && valid_id(&grant.session_id)
                && valid_id(&grant.session_generation)
                && valid_id(&grant.service_instance)
                && grant.duration_ms <= MAX_EXECUTION_GRANT_MS
                && grant.expires_at >= grant.issued_at
                && grant.environment_profile == ENVIRONMENT_PROFILE
        });
    let audit_valid = doc.audit.len() <= MAX_AUDIT_EVENTS
        && doc.audit.iter().all(|event| {
            valid_id(&event.event_id)
                && valid_id(&event.kind)
                && event.principal_id.as_deref().is_none_or(valid_id)
                && event.session_id.as_deref().is_none_or(valid_id)
                && event.operation_id.as_deref().is_none_or(valid_id)
                && event.job_id.as_deref().is_none_or(valid_id)
                && event.grant_id.as_deref().is_none_or(valid_id)
                && event.reason_code.as_deref().is_none_or(valid_id)
        });
    if clients_valid
        && sessions_valid
        && operations_valid
        && jobs_valid
        && grants_valid
        && audit_valid
    {
        Ok(())
    } else {
        Err(DeckError::new(ErrorKind::Recovery, "MCP state is invalid"))
    }
}

fn save(path: &Path, doc: &DiskDoc) -> Result<(), DeckError> {
    validate_doc(doc)?;
    let bytes = serde_json::to_vec(doc)
        .map_err(|_| DeckError::new(ErrorKind::Other, "MCP state encoding failed"))?;
    if bytes.len() > MAX_STATE_BYTES {
        return Err(DeckError::new(
            ErrorKind::DiskFull,
            "MCP state capacity reached",
        ));
    }
    crate::datadir::atomic_write(path, &bytes)
}

impl Runtime {
    fn read<T>(&self, read: impl FnOnce(&DiskDoc) -> T) -> Result<T, DeckError> {
        let doc = self.doc.lock_or_recover();
        match &*doc {
            Ok(doc) => Ok(read(doc)),
            Err(error) => Err(error.clone()),
        }
    }

    fn write<T>(
        &self,
        mutate: impl FnOnce(&mut DiskDoc) -> Result<T, DeckError>,
    ) -> Result<T, DeckError> {
        let _io = self.io.lock_or_recover();
        let mut guard = self.doc.lock_or_recover();
        let doc = match &*guard {
            Ok(doc) => doc,
            Err(error) => return Err(error.clone()),
        };
        let mut candidate: DiskDoc = serde_json::from_value(
            serde_json::to_value(doc)
                .map_err(|_| DeckError::new(ErrorKind::Other, "MCP state clone failed"))?,
        )
        .map_err(|_| DeckError::new(ErrorKind::Other, "MCP state clone failed"))?;
        let result = mutate(&mut candidate)?;
        save(&self.path, &candidate)?;
        *guard = Ok(candidate);
        Ok(result)
    }
}

fn runtime() -> Result<&'static Arc<Runtime>, DeckError> {
    RUNTIME
        .get()
        .ok_or_else(|| DeckError::new(ErrorKind::Other, "MCP control is not initialized"))
}

fn runner_program() -> Result<PathBuf, DeckError> {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join("deck-mcp-runner")))
        .filter(|path| path.is_file())
        .ok_or_else(|| DeckError::new(ErrorKind::Missing, "MCP runner is not bundled"))
}

fn canonical_scope(path: &str, roots: &[String]) -> Result<PathBuf, DeckError> {
    let path = std::fs::canonicalize(path)
        .map_err(|_| DeckError::new(ErrorKind::NotDir, "working directory is unavailable"))?;
    if !path.is_dir() {
        return Err(DeckError::new(
            ErrorKind::NotDir,
            "working directory is unavailable",
        ));
    }
    let allowed = roots.iter().any(|root| {
        std::fs::canonicalize(root)
            .ok()
            .is_some_and(|root| path.starts_with(root))
    });
    if allowed {
        Ok(path)
    } else {
        Err(DeckError::new(
            ErrorKind::Perm,
            "working directory is outside the authorized roots",
        ))
    }
}

fn client<'a>(doc: &'a DiskDoc, client_id: &str) -> Result<&'a Client, DeckError> {
    if !doc.config.enabled {
        return Err(DeckError::new(ErrorKind::Perm, "MCP control is disabled"));
    }
    doc.config
        .clients
        .iter()
        .find(|client| client.id == client_id && client.revoked_at.is_none())
        .ok_or_else(|| DeckError::new(ErrorKind::Perm, "MCP client is not authorized"))
}

fn scoped_project<'a>(client: &'a Client, project_id: &str) -> Result<&'a ProjectScope, DeckError> {
    client
        .projects
        .iter()
        .find(|project| project.project_id == project_id)
        .ok_or_else(|| DeckError::new(ErrorKind::Perm, "project is not authorized"))
}

fn authorized_session<'a>(
    doc: &'a DiskDoc,
    client_id: &str,
    session_id: &str,
) -> Result<&'a ManagedSession, DeckError> {
    client(doc, client_id)?;
    doc.sessions
        .iter()
        .find(|session| session.session_id == session_id && session.owner_client_id == client_id)
        .ok_or_else(|| DeckError::new(ErrorKind::Missing, "session not found"))
}

fn check_control(
    session: &ManagedSession,
    client_id: &str,
    generation: &str,
    epoch: u64,
    holder_id: &str,
) -> Result<(), DeckError> {
    if session.generation != generation {
        return Err(DeckError::new(
            ErrorKind::ContextChanged,
            "session generation changed",
        ));
    }
    if session.human_lock
        || session.control_owner.as_deref() != Some(client_id)
        || session.control_holder.as_deref() != Some(holder_id)
        || session.control_epoch != epoch
        || session
            .lease_expires_at
            .is_none_or(|lease| lease <= now_ms())
    {
        return Err(DeckError::new(
            ErrorKind::ControlRevoked,
            "session control was revoked",
        ));
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum ExecutionAuthorization<'a> {
    None,
    Active(&'a ExecutionGrant),
    Expired(&'a ExecutionGrant),
    Revoked(&'a ExecutionGrant),
}

fn execution_authorization<'a>(
    runtime: &Runtime,
    doc: &'a DiskDoc,
    client_id: &str,
    session: &ManagedSession,
) -> ExecutionAuthorization<'a> {
    let elapsed = runtime.monotonic_ms();
    let Some(grant) = doc.execution_grants.iter().rev().find(|grant| {
        grant.client_id == client_id
            && grant.project_id == session.project_id
            && grant.session_id == session.session_id
            && grant.session_generation == session.generation
    }) else {
        return ExecutionAuthorization::None;
    };
    if grant.revoked_at.is_some() || grant.grant_version <= grant.revocation_version {
        return ExecutionAuthorization::Revoked(grant);
    }
    let credential_current = doc
        .config
        .clients
        .iter()
        .find(|client| client.id == client_id)
        .is_some_and(|client| client.credential_version == grant.credential_version);
    if !credential_current || grant.service_instance != runtime.service_instance {
        return ExecutionAuthorization::Revoked(grant);
    }
    if elapsed < grant.issued_monotonic_ms
        || elapsed.saturating_sub(grant.issued_monotonic_ms) >= grant.duration_ms
        || now_ms() >= grant.expires_at
    {
        return ExecutionAuthorization::Expired(grant);
    }
    ExecutionAuthorization::Active(grant)
}

fn active_execution_grant<'a>(
    runtime: &Runtime,
    doc: &'a DiskDoc,
    client_id: &str,
    session: &ManagedSession,
    require_stdin: bool,
) -> Result<&'a ExecutionGrant, DeckError> {
    match execution_authorization(runtime, doc, client_id, session) {
        ExecutionAuthorization::Active(grant) if !require_stdin || grant.allow_stdin => Ok(grant),
        _ => Err(DeckError::new(
            ErrorKind::Perm,
            if require_stdin {
                "interactive stdin is not locally authorized"
            } else {
                "a local execution grant is required"
            },
        )),
    }
}

fn record_expired_grants(runtime: &Runtime) -> Result<(), DeckError> {
    let wall = now_ms();
    let elapsed = runtime.monotonic_ms();
    let expired = runtime.read(|doc| {
        doc.execution_grants
            .iter()
            .filter(|grant| {
                grant.service_instance == runtime.service_instance
                    && grant.revoked_at.is_none()
                    && (wall >= grant.expires_at
                        || elapsed.saturating_sub(grant.issued_monotonic_ms) >= grant.duration_ms)
                    && !doc.audit.iter().any(|event| {
                        event.kind == "grant-expired"
                            && event.grant_id.as_deref() == Some(&grant.grant_id)
                    })
            })
            .map(|grant| {
                (
                    grant.client_id.clone(),
                    grant.session_id.clone(),
                    grant.grant_id.clone(),
                )
            })
            .collect::<Vec<_>>()
    })?;
    if expired.is_empty() {
        return Ok(());
    }
    runtime.write(|doc| {
        for (principal_id, session_id, grant_id) in &expired {
            if doc.audit.iter().any(|event| {
                event.kind == "grant-expired"
                    && event.grant_id.as_deref() == Some(grant_id.as_str())
            }) {
                continue;
            }
            audit(
                doc,
                "grant-expired",
                AuditLink {
                    principal_id: Some(principal_id),
                    session_id: Some(session_id),
                    grant_id: Some(grant_id),
                    reason_code: Some("deadline-reached"),
                    ..Default::default()
                },
            )?;
        }
        Ok(())
    })
}

fn error_value(code: &str, message: &str, next_action: &str) -> Value {
    json!({"ok":false,"error":{"code":code,"message":message,"nextAction":next_action}})
}

/// Stable machine codes carried as the DeckError message on local Tauri
/// commands. The webview maps each to one localized sentence; no free text.
const RUNNER_STALE: &str = "mcp-runner-stale";
const SESSION_BUSY: &str = "mcp-session-busy";
const CLIENT_REVOKED: &str = "mcp-client-revoked";
const FEATURE_DISABLED: &str = "mcp-feature-disabled";
const RUNNER_UNCONFIRMED: &str = "mcp-runner-unconfirmed";
const FENCE_UNPERSISTED: &str = "mcp-fence-unpersisted";
/// A request bound to a control or create sequence that is no longer current.
const STALE_REQUEST: &str = "mcp-stale-request";

fn map_error(error: DeckError) -> Value {
    if error.message() == STALE_REQUEST {
        return error_value(
            "STALE_REQUEST",
            "the request names a control or create sequence that is no longer current; it was not applied",
            "Do not resubmit it unchanged. Inspect the current state (deck_session_inspect, deck_capabilities); send a NEW request with a new request_id and the current sequence only if the effect is still wanted.",
        );
    }
    if error.message() == RUNNER_STALE {
        return error_value(
            "RUNNER_STALE",
            "Deck restarted after this managed session was created",
            "Ask the local Deck user to close this session, then create a new one.",
        );
    }
    if error.kind() == ErrorKind::Perm && error.message().contains("execution grant") {
        return error_value(
            "EXECUTION_GRANT_REQUIRED",
            error.message(),
            "Ask the local Deck user to approve a short trusted-host execution window.",
        );
    }
    if error.kind() == ErrorKind::Perm && error.message().contains("stdin") {
        return error_value(
            "STDIN_NOT_AUTHORIZED",
            error.message(),
            "Ask the local Deck user to approve interactive stdin for this execution window.",
        );
    }
    let (code, next) = match error.kind() {
        ErrorKind::Perm => (
            "PERMISSION_DENIED",
            "Open Deck MCP settings and grant or restore the required scope.",
        ),
        ErrorKind::Missing | ErrorKind::NoSession => {
            ("SESSION_NOT_FOUND", "Refresh the authorized session list.")
        }
        ErrorKind::NotDir => (
            "CONTEXT_CHANGED",
            "Choose an existing authorized working directory.",
        ),
        ErrorKind::ContextChanged => (
            "CONTEXT_CHANGED",
            "Inspect the session and use its current generation and control epoch.",
        ),
        ErrorKind::ControlRevoked => (
            "CONTROL_REVOKED",
            "Stop writing, inspect the session, and wait for the local user to return control.",
        ),
        ErrorKind::RequestConflict => (
            "REQUEST_ID_CONFLICT",
            "Reuse a request id only with byte-equivalent validated arguments.",
        ),
        ErrorKind::DiskFull => (
            "CAPACITY_EXCEEDED",
            "Release control and request it again: control changes never need an ordinary slot, and the new epoch retires this session's older records (their request ids then stay rejected). A lapsed lease is closed automatically when capacity is short.",
        ),
        ErrorKind::Locked => (
            "SESSION_BUSY",
            "Inspect the active job or finish human control before retrying.",
        ),
        _ => (
            "INTERNAL_ERROR",
            "Inspect Deck status and logs, then retry only after checking the request id.",
        ),
    };
    error_value(code, error.message(), next)
}

/// Fingerprint the PARSED arguments, not the raw JSON: an optional field sent
/// as null and the same field omitted are the same request, and key order
/// never matters.
fn request_hash<T: Serialize>(tool: &str, arguments: &T) -> String {
    let mut bytes = b"deck-request-v4\0".to_vec();
    bytes.extend(tool.as_bytes());
    bytes.push(0);
    bytes.extend(
        serde_json::to_value(arguments)
            .and_then(|value| serde_json::to_vec(&value))
            .unwrap_or_default(),
    );
    sha(&bytes)
}

/// Which journal pool a new record draws from.
#[derive(Clone, Copy, PartialEq)]
enum Slot {
    Ordinary,
    /// `deck_job_interrupt`: the ordinary pool first, then the interrupt
    /// reserve (bounded per client).
    Interrupt,
    /// A control change. Called after the change is applied to the candidate
    /// document, so compaction has already retired what it supersedes.
    Control,
}

fn is_control_record(operation: &Operation) -> bool {
    operation.kind == "session-control" && operation.control_sequence.is_some()
}

/// A lease that has run out can never become valid again at its epoch
/// (renew and release need a live lease; a new request opens a new epoch),
/// so under capacity pressure its epoch is closed: holder cleared, epoch
/// advanced. The records bound to it are then retired by `compact`. A
/// human-locked or closing session is left alone.
fn close_lapsed_leases(doc: &mut DiskDoc) {
    let now = now_ms();
    for session in &mut doc.sessions {
        if session.control_owner.is_some()
            && !session.human_lock
            && !session.closing
            && session.lease_expires_at.is_none_or(|lease| lease <= now)
        {
            session.control_owner = None;
            session.control_holder = None;
            session.lease_expires_at = None;
            session.control_epoch = session.control_epoch.saturating_add(1);
        }
    }
}

/// Retire what can be retired, then reserve one journal slot for `client_id`
/// in `slot`'s pool. Control records never count against the ordinary pool,
/// so release/request (which retire an epoch's records) always fit.
fn reserve_operation(
    runtime: &Runtime,
    doc: &mut DiskDoc,
    client_id: &str,
    slot: Slot,
) -> Result<(), DeckError> {
    compact(doc, Some(&runtime.service_instance));
    let fits = |doc: &DiskDoc| {
        let control = doc
            .operations
            .iter()
            .filter(|op| is_control_record(op))
            .count();
        if slot == Slot::Control {
            return control < CONTROL_RESERVE;
        }
        let others = doc.operations.len() - control;
        let own = doc
            .operations
            .iter()
            .filter(|op| op.client_id == client_id && !is_control_record(op))
            .count();
        if others < ORDINARY_OPERATIONS && own < MAX_OPERATIONS_PER_CLIENT {
            return true;
        }
        slot == Slot::Interrupt
            && others < MAX_OPERATIONS - CONTROL_RESERVE
            && doc
                .operations
                .iter()
                .filter(|op| op.client_id == client_id && op.kind == "job-interrupt")
                .count()
                < INTERRUPT_RESERVE_PER_CLIENT
    };
    if fits(doc) {
        return Ok(());
    }
    if slot != Slot::Control {
        close_lapsed_leases(doc);
        compact(doc, Some(&runtime.service_instance));
        if fits(doc) {
            return Ok(());
        }
    }
    Err(DeckError::new(
        ErrorKind::DiskFull,
        "MCP operation capacity reached",
    ))
}

/// Emergency fences are in-memory and set BEFORE the local command waits for
/// the delivery lock. Every side-effect path re-checks them while holding
/// that lock, as its last step before the side effect (runner I/O or Board
/// admission).
///
/// Linearization: a local command whose fence is visible to that check wins
/// and nothing is dispatched. A fence set after the check belongs to a local
/// command that is still waiting for the same delivery lock; it takes effect
/// only after the in-flight dispatch finished, and is ordered after it (its
/// audit event follows the dispatch's). Holding the fence mutex across runner
/// I/O would make an emergency stop wait for that I/O, so it is not done.
fn emergency_denial(
    runtime: &Runtime,
    client_id: &str,
    session_id: &str,
    admission: Admission,
) -> Option<Value> {
    let emergency = runtime.emergency.lock_or_recover();
    if emergency.disabled || emergency.clients.contains(client_id) {
        return Some(error_value(
            "AUTH_REQUIRED",
            "MCP authority was revoked locally",
            "Reauthorize this integration locally in Deck.",
        ));
    }
    if emergency.human_sessions.contains(session_id) {
        return Some(error_value(
            "HUMAN_CONTROL",
            "local takeover has fenced remote access",
            "Wait for the local user to explicitly return control.",
        ));
    }
    if admission == Admission::Execution && emergency.execution_fenced(session_id) {
        return Some(error_value(
            "EXECUTION_GRANT_REQUIRED",
            "local execution authority was revoked",
            "Ask the local Deck user to approve a new execution window.",
        ));
    }
    None
}

/// The emergency fences as a local-command error, for Board admissions
/// (create start, close validation and admission). Checked while holding the
/// delivery lock, before the state write (never inside it: the fence mutex is
/// taken before the state lock elsewhere).
fn emergency_fence_error(
    runtime: &Runtime,
    client_id: &str,
    session_id: &str,
) -> Option<DeckError> {
    let emergency = runtime.emergency.lock_or_recover();
    if emergency.disabled {
        return Some(DeckError::new(ErrorKind::Perm, FEATURE_DISABLED));
    }
    if emergency.clients.contains(client_id) {
        return Some(DeckError::new(ErrorKind::Perm, CLIENT_REVOKED));
    }
    if emergency.human_sessions.contains(session_id) {
        return Some(DeckError::new(
            ErrorKind::ControlRevoked,
            "local takeover has fenced remote access",
        ));
    }
    None
}

fn existing_operation<'a>(
    doc: &'a DiskDoc,
    client_id: &str,
    request_id: &str,
    hash: &str,
) -> Result<Option<&'a Operation>, DeckError> {
    if let Some(operation) = doc
        .operations
        .iter()
        .find(|operation| operation.client_id == client_id && operation.request_id == request_id)
    {
        if operation.request_hash != hash {
            return Err(DeckError::new(
                ErrorKind::RequestConflict,
                "request id was already used with different arguments",
            ));
        }
        return Ok(Some(operation));
    }
    Ok(None)
}

/// Operation state is the control decision only; process completion and
/// output EOF are reported by `deck_job_read`. Internal plan fields (the
/// runner socket path) never leave Deck.
fn operation_view(operation: &Operation) -> Value {
    let mut result = operation.result.clone();
    if let Some(Value::Object(fields)) = result.as_mut() {
        fields.remove("runnerSocket");
    }
    json!({
        "ok": true,
        "operationId": operation.operation_id,
        "kind": operation.kind,
        "state": operation.state,
        "code": operation.code,
        "result": result,
        "acceptedAt": operation.accepted_at,
        "updatedAt": operation.updated_at
    })
}

fn runner_exchange(socket: &str, generation: &str, request: &Value) -> Result<Value, DeckError> {
    let mut stream = UnixStream::connect(socket)
        .map_err(|_| DeckError::new(ErrorKind::Missing, "managed runner is unavailable"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(7)))
        .map_err(DeckError::from)?;
    stream
        .set_write_timeout(Some(Duration::from_secs(7)))
        .map_err(DeckError::from)?;
    let mut frame = serde_json::to_vec(request)
        .map_err(|_| DeckError::new(ErrorKind::Other, "runner request encoding failed"))?;
    frame.push(b'\n');
    if frame.len() > MAX_REQUEST_BYTES {
        return Err(DeckError::new(
            ErrorKind::Invalid,
            "encoded runner request exceeds its bound",
        ));
    }
    stream.write_all(&frame).map_err(DeckError::from)?;
    stream.flush().map_err(DeckError::from)?;
    let mut bytes = Vec::new();
    BufReader::new(stream)
        .take((MAX_RESPONSE_BYTES + 1) as u64)
        .read_until(b'\n', &mut bytes)
        .map_err(DeckError::from)?;
    if bytes.len() > MAX_RESPONSE_BYTES || !bytes.ends_with(b"\n") {
        return Err(DeckError::new(
            ErrorKind::Other,
            "runner response exceeded its bound",
        ));
    }
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| DeckError::new(ErrorKind::Other, "runner response is invalid"))?;
    if value.get("generation").and_then(Value::as_str) != Some(generation) {
        return Err(DeckError::new(
            ErrorKind::ContextChanged,
            "managed runner generation changed",
        ));
    }
    Ok(value)
}

fn runner_auth(runtime: &Runtime, session: &ManagedSession) -> Result<String, DeckError> {
    let mut keys = runtime.runner_auth.lock_or_recover();
    if let Some(key) = keys.get(&session.runner_socket) {
        return Ok(key.clone());
    }
    let response = runner_exchange(
        &session.runner_socket,
        &session.generation,
        &json!({
            "kind":"claim",
            "service_instance":runtime.service_instance,
            "generation":session.generation,
        }),
    )?;
    let key = response
        .get("auth")
        .and_then(Value::as_str)
        .filter(|encoded| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(encoded)
                .is_ok_and(|bytes| bytes.len() == 32)
        })
        .ok_or_else(|| DeckError::new(ErrorKind::Perm, "managed runner authentication failed"))?
        .to_owned();
    keys.insert(session.runner_socket.clone(), key.clone());
    Ok(key)
}

fn send_runner(
    runtime: &Runtime,
    session: &ManagedSession,
    request: &Value,
) -> Result<Value, DeckError> {
    let auth = runner_auth(runtime, session)?;
    let mut authenticated = request.clone();
    authenticated
        .as_object_mut()
        .ok_or_else(|| DeckError::new(ErrorKind::Invalid, "runner request must be an object"))?
        .insert("auth".into(), Value::String(auth));
    runner_exchange(&session.runner_socket, &session.generation, &authenticated)
}

fn send_runner_control(
    runtime: &Runtime,
    session: &ManagedSession,
    mode: &str,
) -> Result<Value, DeckError> {
    send_runner(
        runtime,
        session,
        &json!({
            "kind": "control",
            "mode": mode,
            "service_instance": runtime.service_instance,
            "control_epoch": session.control_epoch,
            "holder_id": session.control_holder,
        }),
    )
}

/// Ping a runner and learn whether it still belongs to this Deck process.
/// `None` means unreachable or unparseable.
struct RunnerProbe {
    current: bool,
    job: Value,
    version: Option<String>,
}

fn probe_runner(runtime: &Runtime, session: &ManagedSession) -> Option<RunnerProbe> {
    let value = send_runner(
        runtime,
        session,
        &json!({"kind":"ping","service_instance":runtime.service_instance}),
    )
    .ok()?;
    if value.get("ok").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    Some(RunnerProbe {
        // A runner that predates `serviceCurrent` cannot prove it is current.
        current: value.get("serviceCurrent").and_then(Value::as_bool) == Some(true),
        job: value.get("job").cloned().unwrap_or(Value::Null),
        version: value
            .get("runnerVersion")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

/// Classify a runner control reply for local commands.
fn runner_control_result(response: Result<Value, DeckError>) -> Result<(), DeckError> {
    match response {
        Ok(value) if value.get("ok").and_then(Value::as_bool) == Some(true) => Ok(()),
        Ok(value) => Err(match value.get("error").and_then(Value::as_str) {
            Some("runner-stale") => DeckError::new(ErrorKind::ContextChanged, RUNNER_STALE),
            Some("session-busy") => DeckError::new(ErrorKind::Locked, SESSION_BUSY),
            _ => DeckError::new(ErrorKind::ContextChanged, RUNNER_UNCONFIRMED),
        }),
        Err(_) => Err(DeckError::new(
            ErrorKind::ContextChanged,
            RUNNER_UNCONFIRMED,
        )),
    }
}

fn runner_socket_matches(runtime: &Runtime, socket: &str, generation: &str) -> bool {
    let session = ManagedSession {
        session_id: String::new(),
        card_id: String::new(),
        tmux_session: String::new(),
        project_id: String::new(),
        title: String::new(),
        cwd: String::new(),
        generation: generation.into(),
        runner_socket: socket.into(),
        owner_client_id: String::new(),
        control_owner: None,
        control_holder: None,
        control_epoch: 0,
        lease_expires_at: None,
        human_lock: true,
        output_shared: false,
        closing: false,
        created_at: 0,
        control_sequence: 0,
    };
    send_runner(runtime, &session, &json!({"kind":"ping"}))
        .ok()
        .is_some_and(|value| value.get("ok").and_then(Value::as_bool) == Some(true))
}

fn runner_error(value: &Value) -> Option<(&'static str, &'static str)> {
    match value.get("error").and_then(Value::as_str)? {
        "session-busy" => Some((
            "SESSION_BUSY",
            "Read or interrupt the active job before retrying.",
        )),
        "runner-stale" => Some((
            "RUNNER_STALE",
            "Deck restarted after this session was created; ask the local user to close it and create a new session.",
        )),
        "control-revoked" => Some((
            "CONTROL_REVOKED",
            "Inspect control state and wait for the local user to return control.",
        )),
        "dispatch-context-invalid" | "holder-conflict" => Some((
            "CONTROL_REVOKED",
            "Acquire current control and submit a new request under the active service context.",
        )),
        "job-not-found" => Some(("JOB_NOT_FOUND", "Refresh the authorized job state.")),
        "job-not-running" => Some((
            "JOB_NOT_RUNNING",
            "Read the job; input and interrupt are accepted only while it is running.",
        )),
        "request-id-conflict" => Some((
            "REQUEST_ID_CONFLICT",
            "Do not reuse the identifier with different arguments.",
        )),
        "capacity-exceeded" => Some((
            "CAPACITY_EXCEEDED",
            "Close old managed sessions before retrying.",
        )),
        "invalid-cwd" | "invalid-script" | "invalid-executable" | "invalid-request" => Some((
            "INVALID_ARGUMENTS",
            "Correct the rejected arguments before retrying.",
        )),
        "response-too-large" => Some((
            "RESPONSE_TOO_LARGE",
            "Read the existing job again with a smaller max_bytes value; do not re-execute it.",
        )),
        _ => None,
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WireRequest {
    version: u32,
    client_id: String,
    credential: String,
    tool: String,
    arguments: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CreateArgs {
    request_id: String,
    project_id: String,
    cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    create_sequence: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OperationArgs {
    operation_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionArgs {
    session_id: String,
    #[serde(default)]
    holder_id: Option<String>,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum ControlAction {
    Request,
    Renew,
    Release,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ControlArgs {
    request_id: String,
    session_id: String,
    expected_generation: String,
    action: ControlAction,
    holder_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    control_epoch: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lease_ms: Option<u64>,
    control_sequence: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecCommon {
    request_id: String,
    session_id: String,
    expected_generation: String,
    control_epoch: u64,
    holder_id: String,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    wait_ms: Option<u64>,
    #[serde(default)]
    execution_timeout_ms: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DirectExecArgs {
    #[serde(flatten)]
    common: ExecCommon,
    executable: String,
    #[serde(default)]
    args: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
    job_id: String,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    max_bytes: Option<usize>,
    #[serde(default)]
    wait_ms: Option<u64>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct InputArgs {
    request_id: String,
    job_id: String,
    session_generation: String,
    control_epoch: u64,
    holder_id: String,
    input: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct InterruptArgs {
    request_id: String,
    job_id: String,
    session_generation: String,
    control_epoch: u64,
    holder_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CloseArgs {
    request_id: String,
    session_id: String,
    expected_generation: String,
    control_epoch: u64,
    holder_id: String,
    #[serde(default)]
    confirm_running: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectPathArgs {
    project_id: String,
    #[serde(default)]
    root_index: usize,
    #[serde(default)]
    path: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileReadArgs {
    project_id: String,
    #[serde(default)]
    root_index: usize,
    path: String,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    max_bytes: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
    project_id: String,
    #[serde(default)]
    root_index: usize,
    #[serde(default)]
    path: String,
    query: String,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    max_results: Option<usize>,
}

fn parse<T: for<'de> Deserialize<'de>>(value: Value) -> Result<T, Value> {
    serde_json::from_value(value).map_err(|_| {
        error_value(
            "INVALID_ARGUMENTS",
            "arguments do not match the tool schema",
            "Correct the arguments and retry only after checking any prior request id.",
        )
    })
}

fn read_root(
    runtime: &Runtime,
    client_id: &str,
    project_id: &str,
    root_index: usize,
) -> Result<(PathBuf, Vec<String>), Value> {
    runtime
        .read(|doc| {
            let project = scoped_project(client(doc, client_id)?, project_id)?;
            let root = project
                .roots
                .get(root_index)
                .ok_or_else(|| DeckError::new(ErrorKind::Perm, "read root is not authorized"))?;
            Ok((PathBuf::from(root), project.roots.clone()))
        })
        .map_err(map_error)?
        .map_err(map_error)
}

fn recheck_read(
    runtime: &Runtime,
    client_id: &str,
    project_id: &str,
    roots: &[String],
) -> Result<(), Value> {
    runtime
        .read(|doc| {
            let project = scoped_project(client(doc, client_id)?, project_id)?;
            if project.roots != roots {
                return Err(DeckError::new(
                    ErrorKind::ContextChanged,
                    "project read authorization changed during the request",
                ));
            }
            Ok(())
        })
        .map_err(map_error)?
        .map_err(map_error)
}

fn page_cursor(
    runtime: &Runtime,
    client_id: &str,
    project_id: &str,
    root_index: usize,
    query: &str,
    snapshot: &str,
    offset: usize,
) -> String {
    let payload = format!(
        "{}\0{client_id}\0{project_id}\0{root_index}\0{query}\0{snapshot}\0{offset}",
        runtime.service_instance
    );
    format!("{offset}.{}", sha(payload.as_bytes()))
}

fn cursor_offset(
    runtime: &Runtime,
    cursor: Option<&str>,
    client_id: &str,
    project_id: &str,
    root_index: usize,
    query: &str,
    snapshot: &str,
) -> Result<usize, Value> {
    let Some(cursor) = cursor else { return Ok(0) };
    let (offset, _) = cursor.split_once('.').ok_or_else(|| {
        error_value(
            "OUTPUT_CURSOR_INVALID",
            "read cursor is invalid",
            "Restart the read without a cursor.",
        )
    })?;
    let offset = offset.parse::<usize>().map_err(|_| {
        error_value(
            "OUTPUT_CURSOR_INVALID",
            "read cursor is invalid",
            "Restart the read without a cursor.",
        )
    })?;
    if page_cursor(
        runtime, client_id, project_id, root_index, query, snapshot, offset,
    ) != cursor
    {
        return Err(error_value(
            "CONTENT_CHANGED",
            "the target or authorization changed since the cursor was issued",
            "Restart the read or search from the beginning.",
        ));
    }
    Ok(offset)
}

/// One closed code per structured-read failure class. Absent and excluded
/// paths share READ_DENIED so a denial never reveals out-of-scope names.
fn fs_error(error: crate::mcp_fs::FsError, denied_action: &str) -> Value {
    use crate::mcp_fs::FsErrorKind;
    let (code, next_action) = match error.kind {
        FsErrorKind::Invalid => (
            "INVALID_ARGUMENTS",
            "Use a relative path without traversal and arguments within the documented bounds.",
        ),
        FsErrorKind::Denied => ("READ_DENIED", denied_action),
        FsErrorKind::Limit => (
            "READ_LIMIT",
            "The target exceeds the structured read size bound; choose a smaller file.",
        ),
        FsErrorKind::Changed => (
            "CONTENT_CHANGED",
            "Restart the read or search without a cursor.",
        ),
        FsErrorKind::Cancelled => (
            "CONTEXT_CHANGED",
            "Call deck_capabilities to confirm the current project scope before retrying.",
        ),
    };
    error_value(code, error.message, next_action)
}

fn project_list(runtime: &Runtime, client_id: &str, arguments: Value) -> Result<Value, Value> {
    let args: ProjectPathArgs = parse(arguments)?;
    let (root, roots) = read_root(runtime, client_id, &args.project_id, args.root_index)?;
    let listing = crate::mcp_fs::list(&root, &args.path).map_err(|error| {
        fs_error(
            error,
            "Choose an existing, non-sensitive directory inside the approved root.",
        )
    })?;
    recheck_read(runtime, client_id, &args.project_id, &roots)?;
    Ok(
        json!({"ok":true,"projectId":args.project_id,"rootIndex":args.root_index,"path":args.path,"entries":listing.entries,"version":listing.version,"truncated":listing.truncated}),
    )
}

fn project_read(runtime: &Runtime, client_id: &str, arguments: Value) -> Result<Value, Value> {
    let args: FileReadArgs = parse(arguments)?;
    let (root, roots) = read_root(runtime, client_id, &args.project_id, args.root_index)?;
    // Read metadata/content once at offset zero to obtain the descriptor-bound
    // version. Subsequent cursor validation binds that version before slicing.
    let file_version = crate::mcp_fs::identity(&root, &args.path).map_err(|error| {
        fs_error(
            error,
            "Choose a regular UTF-8, non-sensitive file inside the approved root.",
        )
    })?;
    let snapshot = sha(&serde_json::to_vec(&file_version).unwrap_or_default());
    let offset = cursor_offset(
        runtime,
        args.cursor.as_deref(),
        client_id,
        &args.project_id,
        args.root_index,
        &args.path,
        &snapshot,
    )?;
    let (content, next, truncated, version) = crate::mcp_fs::read(
        &root,
        &args.path,
        offset as u64,
        args.max_bytes.unwrap_or(MAX_READ_BYTES),
    )
    .map_err(|error| {
        fs_error(
            error,
            "Choose a regular UTF-8, non-sensitive file inside the approved root.",
        )
    })?;
    if version != file_version {
        return Err(error_value(
            "CONTENT_CHANGED",
            "the file changed during the read",
            "Restart reading without a cursor.",
        ));
    }
    recheck_read(runtime, client_id, &args.project_id, &roots)?;
    let cursor = truncated.then(|| {
        page_cursor(
            runtime,
            client_id,
            &args.project_id,
            args.root_index,
            &args.path,
            &snapshot,
            next as usize,
        )
    });
    Ok(
        json!({"ok":true,"projectId":args.project_id,"rootIndex":args.root_index,"path":args.path,"content":content,"version":version,"nextCursor":cursor,"truncated":truncated}),
    )
}

fn project_search(runtime: &Runtime, client_id: &str, arguments: Value) -> Result<Value, Value> {
    let args: SearchArgs = parse(arguments)?;
    let limit = args.max_results.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return Err(error_value(
            "INVALID_ARGUMENTS",
            "search result limit is invalid",
            "Use max_results from 1 through 100.",
        ));
    }
    let (root, roots) = read_root(runtime, client_id, &args.project_id, args.root_index)?;
    let deadline = Instant::now() + Duration::from_millis(750);
    let outcome = crate::mcp_fs::search_controlled(&root, &args.path, &args.query, || {
        if Instant::now() >= deadline {
            return crate::mcp_fs::SearchControl::Deadline;
        }
        let authorized = runtime
            .read(|doc| {
                scoped_project(client(doc, client_id)?, &args.project_id)
                    .map(|project| project.roots == roots)
            })
            .ok()
            .and_then(Result::ok)
            .unwrap_or(false);
        if authorized {
            crate::mcp_fs::SearchControl::Continue
        } else {
            crate::mcp_fs::SearchControl::Cancelled
        }
    })
    .map_err(|error| {
        fs_error(
            error,
            "Choose an existing, non-sensitive file or directory inside the approved root.",
        )
    })?;
    let results = outcome.matches;
    let search_complete = outcome.complete;
    let stop_reason = outcome.stop_reason;
    let skipped = outcome.skipped;
    let snapshot = sha(&serde_json::to_vec(&results).unwrap_or_default());
    let key = format!("{}\0{}", args.path, args.query);
    let offset = cursor_offset(
        runtime,
        args.cursor.as_deref(),
        client_id,
        &args.project_id,
        args.root_index,
        &key,
        &snapshot,
    )?;
    if offset > results.len() {
        return Err(error_value(
            "CONTENT_CHANGED",
            "search results changed",
            "Restart the search without a cursor.",
        ));
    }
    let mut end = offset.saturating_add(limit).min(results.len());
    recheck_read(runtime, client_id, &args.project_id, &roots)?;
    loop {
        let cursor = (search_complete && end < results.len()).then(|| {
            page_cursor(
                runtime,
                client_id,
                &args.project_id,
                args.root_index,
                &key,
                &snapshot,
                end,
            )
        });
        let response = json!({"ok":true,"projectId":args.project_id,"path":args.path,"query":args.query,"matches":&results[offset..end],"nextCursor":cursor,"truncated":!search_complete || end < results.len(),"complete":search_complete && end == results.len(),"stopReason":stop_reason,"skipped":skipped});
        if serde_json::to_vec(&response).is_ok_and(|bytes| bytes.len() < MAX_RESPONSE_BYTES) {
            return Ok(response);
        }
        if end == offset {
            return Err(error_value(
                "RESPONSE_TOO_LARGE",
                "one encoded search result exceeds the response budget",
                "Narrow the search path or query.",
            ));
        }
        end -= 1;
    }
}

fn capabilities(runtime: &Runtime, client_id: &str, arguments: Value) -> Result<Value, Value> {
    parse::<Empty>(arguments)?;
    runtime
        .read(|doc| {
            let client = doc
                .config
                .clients
                .iter()
                .find(|client| client.id == client_id && client.revoked_at.is_none())
                .ok_or_else(|| DeckError::new(ErrorKind::Perm, "MCP client is not authorized"))?;
            Ok(json!({
                "ok": true,
                "deckConnection": "connected",
                "controlProtocolVersion": CONTROL_PROTOCOL,
                "stateSchemaVersion": STATE_VERSION,
                "deckVersion": DECK_VERSION,
                "deckBuild": DECK_BUILD,
                "executionMode": "structured-direct-default",
                "realOsSandbox": false,
                "directExecution": {
                    "default": true,
                    "arbitraryPrograms": true,
                    "executableResolution": "absolute-path-only",
                    "argumentsVisibleInProcessMetadata": true,
                    "path": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
                    "pathPurpose": "child-process-environment",
                    "outputKind": "pty_combined"
                },
                "limits": {
                    "executableBytes": MAX_EXECUTABLE_BYTES,
                    "argumentCount": MAX_ARGUMENTS,
                    "argumentBytes": MAX_ARGUMENT_BYTES,
                    "inputBytes": MAX_INPUT_BYTES,
                    "readBytesDefault": MAX_READ_BYTES,
                    "readBytesMax": MAX_READ_BYTES,
                    "waitMsMax": 5000,
                    "retainedOutputBytesPerJob": 1024 * 1024,
                    "retainedOutputBytesPerSession": 16 * 1024 * 1024,
                    "retainedOutputBytesGlobal": MAX_SESSIONS * 16 * 1024 * 1024,
                    "managedSessions": MAX_SESSIONS,
                    "operations": MAX_OPERATIONS
                },
                "outputRetentionMs": doc.config.output_retention_ms,
                "interactiveInput": true,
                "structuredProjectRead": true,
                "reliableJobExitTracking": true,
                "authorizedWorkspaces": client.projects,
                "mayCreateSession": client.allow_create,
                "nextCreateSequence": client.create_sequence
            }))
        })
        .map_err(map_error)?
        .map_err(map_error)
}

fn sessions_list(runtime: &Runtime, client_id: &str, arguments: Value) -> Result<Value, Value> {
    parse::<Empty>(arguments)?;
    let (sessions, next_create_sequence) = runtime
        .read(|doc| {
            let create_sequence = client(doc, client_id)?.create_sequence;
            Ok((
                doc.sessions
                    .iter()
                    .filter(|session| session.owner_client_id == client_id)
                    .cloned()
                    .collect::<Vec<_>>(),
                create_sequence,
            ))
        })
        .map_err(map_error)?
        .map_err(map_error)?;
    let values = sessions
        .into_iter()
        .map(|session| {
            let runner = probe_runner(runtime, &session);
            json!({
                "sessionId": session.session_id,
                "cardId": session.card_id,
                "projectId": session.project_id,
                "title": session.title,
                "sessionGeneration": session.generation,
                "controlOwner": session.control_owner,
                "controlHolder": session.control_holder,
                "controlEpoch": session.control_epoch,
                "controlSequence": session.control_sequence,
                "activeJob": runner.as_ref().map(|probe| probe.job.clone()),
                "foreground": runner.as_ref().is_some_and(|probe| !probe.job.is_null()).then_some("managed-job"),
                "stale": runner.as_ref().is_none_or(|probe| !probe.current)
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({"ok":true,"sessions":values,"nextCreateSequence":next_create_sequence}))
}

fn session_create(runtime: &Runtime, client_id: &str, arguments: Value) -> Result<Value, Value> {
    let args: CreateArgs = parse(arguments.clone())?;
    if !valid_id(&args.request_id)
        || !valid_id(&args.project_id)
        || args
            .title
            .as_deref()
            .is_some_and(|title| !valid_title(title))
    {
        return Err(error_value(
            "INVALID_ARGUMENTS",
            "request, project, or title is invalid",
            "Use ids returned by Deck and a short printable title.",
        ));
    }
    let hash = request_hash("deck_session_create", &args);
    let title = args.title.clone().unwrap_or_else(|| "MCP shell".into());
    let operation = runtime
        .write(|doc| {
            client(doc, client_id)?;
            // Request-key lookup, fingerprint comparison, capacity reservation,
            // and insertion are one persisted transaction. A separate read here
            // allowed two concurrent callers to reserve two create plans.
            if let Some(existing) = existing_operation(doc, client_id, &args.request_id, &hash)? {
                return Ok(existing.clone());
            }
            // A create must name the client's current create sequence, which
            // advances on every accepted create: a create whose record left
            // the result window can never be accepted again.
            if args.create_sequence != client(doc, client_id)?.create_sequence {
                return Err(DeckError::new(ErrorKind::ContextChanged, STALE_REQUEST));
            }
            reserve_operation(runtime, doc, client_id, Slot::Ordinary)?;
            if doc.sessions.len() >= MAX_SESSIONS {
                return Err(DeckError::new(
                    ErrorKind::DiskFull,
                    "MCP operation capacity reached",
                ));
            }
            // Accepted creates reserve a future managed-session slot.
            let reserved = doc
                .operations
                .iter()
                .filter(|operation| {
                    operation.kind == "session-create"
                        && matches!(operation.state.as_str(), "accepted" | "executing")
                })
                .count();
            if doc.sessions.len().saturating_add(reserved) >= MAX_SESSIONS {
                return Err(DeckError::new(
                    ErrorKind::DiskFull,
                    "MCP managed-session capacity reached",
                ));
            }
            let client = client(doc, client_id)?;
            if !client.allow_create {
                return Err(DeckError::new(
                    ErrorKind::Perm,
                    "client may not create sessions",
                ));
            }
            let project = scoped_project(client, &args.project_id)?;
            let cwd = canonical_scope(&args.cwd, &project.roots)?;
            let operation_id = random_id("op_")?;
            let card_id = random_id("M")?;
            let session_id = random_id("mcp_")?;
            let generation = random_id("g_")?;
            let socket = crate::datadir::deck_dir()
                .join("mcp-runners")
                .join(format!("{generation}.sock"));
            let result = json!({
                "cardId": card_id,
                "sessionId": &session_id,
                "projectId": args.project_id,
                "title": title,
                "cwd": cwd,
                "generation": generation,
                "controlSequence": 0,
                "runnerSocket": socket
            });
            let operation = Operation {
                operation_id,
                client_id: client_id.into(),
                request_id: args.request_id.clone(),
                request_hash: hash,
                kind: "session-create".into(),
                state: "accepted".into(),
                code: None,
                result: Some(result),
                accepted_at: now_ms(),
                updated_at: now_ms(),
                admission_hash: None,
                session_id: Some(session_id),
                control_epoch: None,
                control_sequence: None,
            };
            if let Some(client) = doc
                .config
                .clients
                .iter_mut()
                .find(|client| client.id == client_id)
            {
                client.create_sequence = client.create_sequence.saturating_add(1);
            }
            doc.operations.push(operation.clone());
            Ok(operation)
        })
        .map_err(map_error)?;
    emit_changed(runtime);
    Ok(operation_view(&operation))
}

fn operation_get(runtime: &Runtime, client_id: &str, arguments: Value) -> Result<Value, Value> {
    let args: OperationArgs = parse(arguments)?;
    runtime
        .read(|doc| {
            client(doc, client_id)?;
            doc.operations
                .iter()
                .find(|operation| {
                    operation.operation_id == args.operation_id && operation.client_id == client_id
                })
                .map(operation_view)
                .ok_or_else(|| DeckError::new(ErrorKind::Missing, "operation not found"))
        })
        .map_err(map_error)?
        .map_err(map_error)
}

fn inspect(runtime: &Runtime, client_id: &str, arguments: Value) -> Result<Value, Value> {
    let args: SessionArgs = parse(arguments)?;
    let session = runtime
        .read(|doc| authorized_session(doc, client_id, &args.session_id).cloned())
        .map_err(map_error)?
        .map_err(map_error)?;
    let runner = probe_runner(runtime, &session);
    // The execution fence is read BEFORE the grants: a revocation that has
    // fenced but not yet persisted already refuses exec and stdin, so its
    // grant reads `revoked` here, and once it persists the grant itself
    // reads `revoked` — the view never returns to `active` in between.
    let (emergency_human, execution_fenced) = {
        let emergency = runtime.emergency.lock_or_recover();
        (
            emergency.human_sessions.contains(&session.session_id),
            emergency.execution_fenced(&session.session_id),
        )
    };
    let (authorization_status, authorization_expiry, stdin_approved) = runtime
        .read(|doc| {
            Ok(
                match execution_authorization(runtime, doc, client_id, &session) {
                    ExecutionAuthorization::None => ("none", None, false),
                    ExecutionAuthorization::Active(grant) if execution_fenced => {
                        ("revoked", Some(grant.expires_at), false)
                    }
                    ExecutionAuthorization::Active(grant) => {
                        ("active", Some(grant.expires_at), grant.allow_stdin)
                    }
                    ExecutionAuthorization::Expired(grant) => {
                        ("expired", Some(grant.expires_at), false)
                    }
                    ExecutionAuthorization::Revoked(grant) => {
                        ("revoked", Some(grant.expires_at), false)
                    }
                },
            )
        })
        .map_err(map_error)?
        .map_err(map_error)?;
    let job = runner
        .as_ref()
        .map(|probe| probe.job.clone())
        .unwrap_or(Value::Null);
    let denial = if runner.is_none() {
        Some("RUNNER_UNAVAILABLE")
    } else if runner.as_ref().is_some_and(|probe| !probe.current) {
        Some("RUNNER_STALE")
    } else if session.closing {
        Some("TARGET_CLOSING")
    } else if session.human_lock || emergency_human {
        Some("HUMAN_CONTROL")
    } else if session.control_owner.as_deref() != Some(client_id) {
        Some("CONTROL_REVOKED")
    } else if args.holder_id.as_deref() != session.control_holder.as_deref() {
        Some("HOLDER_MISMATCH")
    } else if session
        .lease_expires_at
        .is_none_or(|deadline| deadline <= now_ms())
    {
        Some("CONTROL_LEASE_EXPIRED")
    } else if !job.is_null() {
        Some("SESSION_BUSY")
    } else if authorization_status != "active" {
        Some("EXECUTION_GRANT_REQUIRED")
    } else {
        None
    };
    // Terminal screen content is deliberately NOT returned: output reaches a
    // client only through deck_job_read, gated per job binding.
    Ok(json!({
        "ok": true,
        "sessionId": session.session_id,
        "sessionGeneration": session.generation,
        "controlOwner": session.control_owner,
        "controlHolder": session.control_holder,
        "controlEpoch": session.control_epoch,
        "controlSequence": session.control_sequence,
        "leaseExpiresAt": session.lease_expires_at,
        "humanLock": session.human_lock || emergency_human,
        "executionAuthorization": {
            "status": authorization_status,
            "active": authorization_status == "active",
            "expiresAtUnixMs": authorization_expiry,
            "stdinApprovedForActiveGrant": stdin_approved
        },
        "outputSharing": {
            "sessionGateOpen": session.output_shared && !session.human_lock && !emergency_human,
            "independentOfExecutionAuthorization": true
        },
        "activeJob": job,
        "foreground": (!job.is_null()).then_some("managed-job"),
        "stale": runner.as_ref().is_none_or(|probe| !probe.current),
        "runnerVersion": runner.as_ref().and_then(|probe| probe.version.clone()),
        "mayStartNextJob": denial.is_none(),
        "mayStartNextJobReason": denial,
    }))
}

fn session_control(runtime: &Runtime, client_id: &str, arguments: Value) -> Result<Value, Value> {
    let args: ControlArgs = parse(arguments)?;
    if !valid_id(&args.request_id) || !valid_id(&args.holder_id) {
        return Err(error_value(
            "INVALID_ARGUMENTS",
            "request id is invalid",
            "Use a stable opaque request id.",
        ));
    }
    if let Some(lease_ms) = args.lease_ms {
        if !(1_000..=MAX_LEASE_MS).contains(&lease_ms) {
            return Err(error_value(
                "INVALID_ARGUMENTS",
                "lease_ms is outside the allowed range",
                "Use an integer from 1000 through 300000 milliseconds.",
            ));
        }
    }
    match args.action {
        ControlAction::Request if args.control_epoch.is_some() => {
            return Err(error_value(
                "INVALID_ARGUMENTS",
                "control_epoch is not allowed for request",
                "Omit control_epoch when requesting a new control lease.",
            ));
        }
        ControlAction::Renew | ControlAction::Release if args.control_epoch.is_none() => {
            return Err(error_value(
                "INVALID_ARGUMENTS",
                "control_epoch is required for this action",
                "Use the current epoch returned by a successful control response.",
            ));
        }
        ControlAction::Release if args.lease_ms.is_some() => {
            return Err(error_value(
                "INVALID_ARGUMENTS",
                "lease_ms is not allowed for release",
                "Omit lease_ms when releasing control.",
            ));
        }
        _ => {}
    }
    let hash = request_hash("deck_session_control", &args);
    let action = args.action;
    let _delivery = runtime.delivery.lock_or_recover();
    if let Some(denied) = emergency_denial(runtime, client_id, &args.session_id, Admission::Control)
    {
        return Err(denied);
    }
    // (operation, session, replayed)
    let (operation, session, replayed) = runtime
        .write(|doc| {
            client(doc, client_id)?;
            if let Some(existing) = existing_operation(doc, client_id, &args.request_id, &hash)? {
                // An exact replay returns the recorded decision only. It never
                // re-sends a runner control side effect: after a holder change
                // that would re-install a stale control context.
                let session = doc
                    .sessions
                    .iter()
                    .find(|session| session.session_id == args.session_id)
                    .cloned();
                return Ok((existing.clone(), session, true));
            }
            let session = doc
                .sessions
                .iter_mut()
                .find(|session| session.session_id == args.session_id && session.owner_client_id == client_id)
                .ok_or_else(|| DeckError::new(ErrorKind::Missing, "session not found"))?;
            if session.generation != args.expected_generation {
                return Err(DeckError::new(ErrorKind::ContextChanged, "session generation changed"));
            }
            // Every accepted change advances the sequence, and a request must
            // name the current value. A retry of a change whose record was
            // superseded or retired therefore fails here — it is never
            // applied a second time (a renew included: it cannot extend the
            // lease again).
            if args.control_sequence != session.control_sequence {
                return Err(DeckError::new(ErrorKind::ContextChanged, STALE_REQUEST));
            }
            match args.action {
                ControlAction::Request => {
                    let lease_active = session.lease_expires_at.is_some_and(|lease| lease > now_ms());
                    if session.human_lock
                        || (lease_active
                            && (session.control_owner.as_deref() != Some(client_id)
                                || session.control_holder.as_deref() != Some(&args.holder_id)))
                    {
                        return Err(DeckError::new(ErrorKind::Perm, "user owns terminal control"));
                    }
                    if !lease_active {
                        session.control_epoch = session.control_epoch.saturating_add(1);
                    }
                    session.control_owner = Some(client_id.into());
                    session.control_holder = Some(args.holder_id.clone());
                    session.lease_expires_at = Some(now_ms() + args.lease_ms.unwrap_or(DEFAULT_LEASE_MS));
                }
                ControlAction::Renew => {
                    // Changes only the lease deadline: never the epoch, and
                    // never an execution grant.
                    check_control(session, client_id, &args.expected_generation, args.control_epoch.unwrap_or(0), &args.holder_id)?;
                    session.lease_expires_at = Some(now_ms() + args.lease_ms.unwrap_or(DEFAULT_LEASE_MS));
                }
                ControlAction::Release => {
                    check_control(session, client_id, &args.expected_generation, args.control_epoch.unwrap_or(0), &args.holder_id)?;
                    session.control_owner = None;
                    session.control_holder = None;
                    session.lease_expires_at = None;
                    session.control_epoch = session.control_epoch.saturating_add(1);
                }
            }
            session.control_sequence = session.control_sequence.saturating_add(1);
            let result = json!({"sessionId":session.session_id,"sessionGeneration":session.generation,"controlOwner":session.control_owner,"controlHolder":session.control_holder,"controlEpoch":session.control_epoch,"controlSequence":session.control_sequence,"leaseExpiresAt":session.lease_expires_at});
            let session = session.clone();
            // The change is applied first: compaction then retires what it
            // supersedes (older control records, and an advanced epoch's
            // records), and the control pool is never taken by ordinary work.
            reserve_operation(runtime, doc, client_id, Slot::Control)?;
            let operation = Operation { operation_id: random_id("op_")?, client_id: client_id.into(), request_id: args.request_id.clone(), request_hash: hash.clone(), kind: "session-control".into(), state: "committed".into(), code: None, result: Some(result), accepted_at: now_ms(), updated_at: now_ms(), admission_hash: None, session_id: Some(session.session_id.clone()), control_epoch: Some(session.control_epoch), control_sequence: Some(session.control_sequence) };
            doc.operations.push(operation.clone());
            audit(doc, "control-changed", AuditLink { principal_id: Some(client_id), session_id: Some(&session.session_id), operation_id: Some(&operation.operation_id), ..Default::default() })?;
            Ok((operation, Some(session), false))
        })
        .map_err(map_error)?;
    // A renew changes only Deck's lease deadline; the runner's fence (epoch,
    // holder) is unchanged, so nothing is sent.
    if replayed || matches!(action, ControlAction::Renew) {
        return Ok(operation_view(&operation));
    }
    let Some(session) = session else {
        return Ok(operation_view(&operation));
    };
    let mode = if matches!(action, ControlAction::Release) {
        "fenced"
    } else {
        "mcp"
    };
    match runner_control_result(send_runner_control(runtime, &session, mode)) {
        Ok(()) => Ok(operation_view(&operation)),
        Err(error) if error.message() == RUNNER_STALE => Err(map_error(error)),
        Err(_) => Err(error_value(
            "OPERATION_AMBIGUOUS",
            "the control decision is durable but the runner fence is unconfirmed",
            "Inspect the session locally; do not assume the prior holder can still run or is stopped.",
        )),
    }
}

fn valid_direct_launch(executable: &str, arguments: &[String]) -> bool {
    !executable.is_empty()
        && executable.len() <= MAX_EXECUTABLE_BYTES
        && !executable.chars().any(char::is_control)
        && Path::new(executable).is_absolute()
        && arguments.len() <= MAX_ARGUMENTS
        && arguments.iter().all(|value| !value.as_bytes().contains(&0))
        && arguments.iter().map(String::len).sum::<usize>() <= MAX_ARGUMENT_BYTES
}

fn exec(runtime: &Runtime, client_id: &str, arguments: Value) -> Result<Value, Value> {
    let direct: DirectExecArgs = parse(arguments)?;
    let args = direct.common;
    let executable = direct.executable;
    let argv = direct.args;
    if !valid_id(&args.request_id)
        || !valid_direct_launch(&executable, &argv)
        || args.wait_ms.unwrap_or(1_000) > 5_000
        || args
            .execution_timeout_ms
            .is_some_and(|value| !(100..=24 * 60 * 60 * 1000).contains(&value))
    {
        return Err(error_value(
            "INVALID_ARGUMENTS",
            "exec arguments exceed their bounds",
            "Use a bounded executable and exact argument vector.",
        ));
    }
    // Fingerprinting is independent of JSON object key order. Raw arguments
    // are never journaled; only bounded metadata and digests
    // participate in the stable request fingerprint.
    let executable_digest = sha(executable.as_bytes());
    let argument_digest = sha(&serde_json::to_vec(&argv).unwrap_or_default());
    let argument_bytes = argv.iter().map(String::len).sum::<usize>();
    let launch_fingerprint = json!([
        "direct",
        executable_digest,
        argument_digest,
        argv.len(),
        argument_bytes
    ]);
    let launch_metadata = json!({"launchKind":"direct","executableDigest":executable_digest,"argumentDigest":argument_digest,"argumentCount":argv.len(),"argumentBytes":argument_bytes});
    let hash = sha(serde_json::to_vec(&json!([
        "deck-exec-v3",
        &args.request_id,
        &args.session_id,
        &args.expected_generation,
        args.control_epoch,
        &args.holder_id,
        &args.cwd,
        args.wait_ms,
        args.execution_timeout_ms,
        launch_fingerprint,
        ENVIRONMENT_PROFILE,
        POLICY_VERSION
    ]))
    .unwrap_or_default()
    .as_slice());
    let _delivery = runtime.delivery.lock_or_recover();
    if let Some(denied) =
        emergency_denial(runtime, client_id, &args.session_id, Admission::Execution)
    {
        return Err(denied);
    }
    let prepared = runtime
        .write(|doc| {
            let client = client(doc, client_id)?.clone();
            if let Some(existing) = existing_operation(doc, client_id, &args.request_id, &hash)? {
                let job = existing.result.as_ref().and_then(|result| result.get("jobId")).and_then(Value::as_str).and_then(|job_id| doc.jobs.iter().find(|job| job.job_id == job_id)).cloned();
                return Ok((existing.clone(), job, None));
            }
            reserve_operation(runtime, doc, client_id, Slot::Ordinary)?;
            let session = doc.sessions.iter().find(|session| session.session_id == args.session_id && session.owner_client_id == client_id).cloned().ok_or_else(|| DeckError::new(ErrorKind::Missing, "session not found"))?;
            // Keep a bounded window of bindings per session. The runner holds
            // at most one live job and retires finished jobs oldest-first, so
            // the oldest binding here is never the live one.
            let bound = doc.jobs.iter().filter(|job| job.session_id == session.session_id).count();
            if bound >= MAX_JOBS_PER_SESSION {
                if let Some(oldest) = doc.jobs.iter().position(|job| job.session_id == session.session_id) {
                    doc.jobs.remove(oldest);
                }
            }
            if doc.jobs.len() >= MAX_JOBS {
                return Err(DeckError::new(ErrorKind::DiskFull, "MCP operation capacity reached"));
            }
            check_control(&session, client_id, &args.expected_generation, args.control_epoch, &args.holder_id)?;
            let grant = active_execution_grant(runtime, doc, client_id, &session, false)?.clone();
            if session.closing {
                return Err(DeckError::new(ErrorKind::Locked, "session is closing"));
            }
            let project = scoped_project(&client, &session.project_id)?;
            let cwd = canonical_scope(args.cwd.as_deref().unwrap_or(&session.cwd), &project.roots)?;
            let operation_id = random_id("op_")?;
            let job_id = random_id("job_")?;
            let binding = JobBinding { job_id: job_id.clone(), client_id: client_id.into(), session_id: session.session_id.clone(), session_generation: session.generation.clone(), request_hash: hash.clone(), operation_id: operation_id.clone(), grant_id: grant.grant_id.clone(), grant_version: grant.grant_version, allow_output: grant.allow_output };
            let operation = Operation { operation_id, client_id: client_id.into(), request_id: args.request_id.clone(), request_hash: hash.clone(), kind: "exec".into(), state: "accepted".into(), code: None, result: Some(json!({"jobId":job_id,"sessionId":session.session_id,"sessionGeneration":session.generation,"executionGrantId":grant.grant_id,"executionGrantVersion":grant.grant_version,"policyVersion":POLICY_VERSION,"environmentProfile":ENVIRONMENT_PROFILE,"launch":launch_metadata.clone(),"cwd":cwd})), accepted_at: now_ms(), updated_at: now_ms(), admission_hash: None, session_id: Some(session.session_id.clone()), control_epoch: Some(args.control_epoch), control_sequence: None };
            doc.jobs.push(binding.clone());
            doc.operations.push(operation.clone());
            audit(doc, "exec-intent", AuditLink { principal_id: Some(client_id), session_id: Some(&session.session_id), operation_id: Some(&operation.operation_id), job_id: Some(&binding.job_id), grant_id: Some(&grant.grant_id), reason_code: None })?;
            Ok((operation, Some(binding), Some((session, cwd, grant))))
        })
        .map_err(map_error)?;
    let (mut operation, binding, dispatch) = prepared;
    let Some(binding) = binding else {
        return Ok(operation_view(&operation));
    };
    let Some((session, cwd, _accepted_grant)) = dispatch else {
        let runner = send_runner(
            runtime,
            &runtime
                .read(|doc| authorized_session(doc, client_id, &binding.session_id).cloned())
                .map_err(map_error)?
                .map_err(map_error)?,
            &json!({"kind":"read","job_id":binding.job_id,"max_bytes":1,"wait_ms":0}),
        );
        if let Ok(value) = runner {
            return Ok(
                json!({"ok":true,"operationId":operation.operation_id,"jobId":binding.job_id,"state":value.get("job").and_then(|job| job.get("state")).cloned().unwrap_or(json!("unknown")),"initialOutput":"","outputCursor":format!("{}:{}:0",binding.session_generation,binding.job_id)}),
            );
        }
        return Ok(operation_view(&operation));
    };
    pause_point(runtime, "exec-final");
    // Final admission: the last step before the runner, still under the
    // delivery lock (see `emergency_denial` for the linearization). Control,
    // the SAME execution grant this intent was accepted under (a re-approval
    // never adopts an older intent), and every emergency fence — a pending
    // execution revocation included — must hold. Otherwise nothing is sent
    // and the intent is rejected with the refusal's code.
    let admitted = runtime
        .read(|doc| {
            let current = authorized_session(doc, client_id, &session.session_id)?;
            check_control(
                current,
                client_id,
                &args.expected_generation,
                args.control_epoch,
                &args.holder_id,
            )?;
            let grant = active_execution_grant(runtime, doc, client_id, current, false)?;
            if grant.grant_id != binding.grant_id || grant.grant_version != binding.grant_version {
                return Err(DeckError::new(
                    ErrorKind::Perm,
                    "the execution window changed; a local execution grant is required",
                ));
            }
            Ok(grant.clone())
        })
        .map_err(map_error)
        .and_then(|admitted| admitted.map_err(map_error))
        .and_then(|grant| {
            match emergency_denial(
                runtime,
                client_id,
                &session.session_id,
                Admission::Execution,
            ) {
                Some(denied) => Err(denied),
                None => Ok(grant),
            }
        });
    let runner = admitted.as_ref().ok().map(|grant| {
        let context = json!({
            "service_instance": runtime.service_instance,
            "holder_id": args.holder_id,
            "control_epoch": args.control_epoch,
            "grant_id": grant.grant_id,
            "grant_version": grant.grant_version,
            "policy_version": POLICY_VERSION,
            "intent_hash": hash,
            "expires_at": grant.expires_at,
        });
        let authorization = json!({
            "kind":"authorize-grant",
            "service_instance":runtime.service_instance,
            "grant_id":grant.grant_id,
            "grant_version":grant.grant_version,
            "policy_version":POLICY_VERSION,
            "expires_at":grant.expires_at,
        });
        let authorized = send_runner(runtime, &session, &authorization)?;
        if authorized.get("ok").and_then(Value::as_bool) != Some(true) {
            return Ok(authorized);
        }
        let request = json!({"kind":"exec","job_id":binding.job_id,"request_hash":hash,"executable":executable,"args":argv,"cwd":cwd,"wait_ms":args.wait_ms.unwrap_or(1_000),"timeout_ms":args.execution_timeout_ms,"context":context});
        send_runner(runtime, &session, &request)
    });
    let reply = runner.as_ref().and_then(|runner| runner.as_ref().ok());
    let committed =
        reply.is_some_and(|value| value.get("ok").and_then(Value::as_bool) == Some(true));
    let rejected = reply.and_then(runner_error);
    let refused = admitted.as_ref().err().map(|denied| {
        denied["error"]["code"]
            .as_str()
            .unwrap_or("ADMISSION_REFUSED")
            .to_owned()
    });
    let audit_grant_id = operation
        .result
        .as_ref()
        .and_then(|result| result.get("executionGrantId"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let journaled = runtime.write(|doc| {
        if let Some(saved) = doc
            .operations
            .iter_mut()
            .find(|saved| saved.operation_id == operation.operation_id)
        {
            saved.state = if committed {
                "committed"
            } else if rejected.is_some() || refused.is_some() {
                "rejected"
            } else {
                "ambiguous"
            }
            .into();
            saved.code = if committed {
                None
            } else if let Some(code) = &refused {
                Some(code.clone())
            } else {
                Some(
                    rejected
                        .map(|(code, _)| code)
                        .unwrap_or("dispatch-unknown")
                        .into(),
                )
            };
            saved.updated_at = now_ms();
            operation = saved.clone();
        }
        if rejected.is_some() || refused.is_some() {
            // The runner never created this job: drop its binding.
            doc.jobs.retain(|job| job.job_id != binding.job_id);
        }
        audit(
            doc,
            "exec-dispatch",
            AuditLink {
                principal_id: Some(client_id),
                session_id: Some(&session.session_id),
                operation_id: Some(&operation.operation_id),
                job_id: Some(&binding.job_id),
                grant_id: audit_grant_id.as_deref(),
                reason_code: (!committed).then_some(if refused.is_some() {
                    "admission-refused"
                } else if rejected.is_some() {
                    "runner-rejected"
                } else {
                    "dispatch-unknown"
                }),
            },
        )?;
        Ok(())
    });
    if journaled.is_err() {
        if let Err(denied) = admitted {
            // Nothing was dispatched; the unrecorded intent turns ambiguous
            // on restart and is never replayed.
            return Err(denied);
        }
        return Err(error_value(
            "OPERATION_AMBIGUOUS",
            "job dispatch occurred but its audit result could not be persisted",
            "Inspect the live job; do not re-execute it.",
        ));
    }
    let runner = match admitted {
        Err(denied) => return Err(denied),
        Ok(_) => runner.unwrap_or_else(|| Err(DeckError::new(ErrorKind::Other, "not dispatched"))),
    };
    match runner {
        Ok(value) if committed => Ok(
            json!({"ok":true,"operationId":operation.operation_id,"jobId":binding.job_id,"sessionId":session.session_id,"sessionGeneration":session.generation,"state":value.get("job").and_then(|job| job.get("state")).cloned().unwrap_or(json!("unknown")),"initialOutput":"","outputCursor":format!("{}:{}:0",session.generation,binding.job_id)}),
        ),
        Ok(value) if rejected.is_some() => {
            let (code, next) = runner_error(&value).unwrap();
            Err(error_value(code, "managed runner rejected the job", next))
        }
        _ => Err(error_value(
            "OPERATION_AMBIGUOUS",
            "Deck cannot prove whether job dispatch completed",
            "Inspect the operation and job; do not resubmit the script under a new request id.",
        )),
    }
}

fn decode_cursor(cursor: Option<String>, binding: &JobBinding) -> Result<u64, Value> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    let expected = format!("{}:{}:", binding.session_generation, binding.job_id);
    cursor
        .strip_prefix(&expected)
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| {
            error_value(
                "OUTPUT_CURSOR_INVALID",
                "cursor does not belong to this job generation",
                "Restart reading with no cursor; a gap may be reported.",
            )
        })
}

fn job_read(runtime: &Runtime, client_id: &str, arguments: Value) -> Result<Value, Value> {
    let args: ReadArgs = parse(arguments)?;
    // The same output gate runs before the runner read and again after it
    // returns (the read may wait up to 5 s). Bytes read across a takeover,
    // sharing pause, revocation or generation change are dropped. The gate is
    // the job binding plus the session's independent sharing switch — never
    // the execution grant.
    let gate = || -> Result<(JobBinding, ManagedSession), Value> {
        let (binding, session) = runtime
            .read(|doc| {
                client(doc, client_id)?;
                let binding = doc
                    .jobs
                    .iter()
                    .find(|job| job.job_id == args.job_id && job.client_id == client_id)
                    .cloned()
                    .ok_or_else(|| DeckError::new(ErrorKind::Missing, "job not found"))?;
                let session = authorized_session(doc, client_id, &binding.session_id)?.clone();
                Ok((binding, session))
            })
            .map_err(map_error)?
            .map_err(map_error)?;
        if let Some(denied) =
            emergency_denial(runtime, client_id, &session.session_id, Admission::Control)
        {
            return Err(denied);
        }
        if session.human_lock {
            return Err(error_value(
                "HUMAN_CONTROL",
                "the local user controls this session",
                "Wait for the local user to explicitly return control.",
            ));
        }
        if session.generation != binding.session_generation {
            return Err(map_error(DeckError::new(
                ErrorKind::ContextChanged,
                "session generation changed",
            )));
        }
        if !binding.allow_output || !session.output_shared {
            return Err(map_error(DeckError::new(
                ErrorKind::Perm,
                "job output sharing is paused",
            )));
        }
        Ok((binding, session))
    };
    let (binding, session) = gate()?;
    let cursor = decode_cursor(args.cursor, &binding)?;
    let max_bytes = args.max_bytes.unwrap_or(MAX_READ_BYTES);
    if !(4..=MAX_READ_BYTES).contains(&max_bytes) || args.wait_ms.unwrap_or(0) > 5_000 {
        return Err(error_value(
            "INVALID_ARGUMENTS",
            "read limits are invalid",
            "Use max_bytes 4..16384 and wait_ms up to 5000.",
        ));
    }
    let value = send_runner(runtime, &session, &json!({"kind":"read","job_id":binding.job_id,"cursor":cursor,"max_bytes":max_bytes,"wait_ms":args.wait_ms.unwrap_or(0)})).map_err(map_error)?;
    // Linearization: authority is judged at return time. Output already
    // delivered by an earlier call cannot be recalled.
    gate()?;
    if value.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(error_value(
            "JOB_STATE_UNKNOWN",
            "managed runner could not read the job",
            "Inspect the session; do not re-execute the command.",
        ));
    }
    let next = value
        .get("nextCursor")
        .and_then(Value::as_u64)
        .unwrap_or(cursor);
    Ok(
        json!({"ok":true,"sessionId":session.session_id,"sessionGeneration":session.generation,"jobId":binding.job_id,"state":value.get("job").and_then(|job| job.get("state")).cloned().unwrap_or(json!("unknown")),"exitCode":value.get("job").and_then(|job| job.get("exitCode")).cloned().unwrap_or(Value::Null),"terminationSignal":value.get("job").and_then(|job| job.get("terminationSignal")).cloned().unwrap_or(Value::Null),"interruptRequested":value.get("job").and_then(|job| job.get("interruptRequested")).cloned().unwrap_or(json!(false)),"timeoutRequested":value.get("job").and_then(|job| job.get("timeoutRequested")).cloned().unwrap_or(json!(false)),"processComplete":matches!(value.get("job").and_then(|job| job.get("state")).and_then(Value::as_str),Some("exited" | "lost")),"stdoutEof":value.get("job").and_then(|job| job.get("stdoutEof")).cloned().unwrap_or(json!(false)),"stderrEof":value.get("job").and_then(|job| job.get("stderrEof")).cloned().unwrap_or(json!(false)),"outputComplete":value.get("job").and_then(|job| job.get("outputComplete")).cloned().unwrap_or(json!(false)),"output":value.get("output").cloned().unwrap_or(json!("")),"outputKind":"pty_combined","nextCursor":format!("{}:{}:{}",session.generation,binding.job_id,next),"truncated":value.get("gap").cloned().unwrap_or(json!(false)),"gap":value.get("gap").cloned().unwrap_or(json!(false)),"droppedBytes":value.get("droppedBytes").cloned().unwrap_or(json!(0)),"startedAt":value.get("job").and_then(|job| job.get("startedAt")).cloned().unwrap_or(Value::Null),"endedAt":value.get("job").and_then(|job| job.get("endedAt")).cloned().unwrap_or(Value::Null),"nextAction":if value.get("job").and_then(|job| job.get("outputComplete")).and_then(Value::as_bool)==Some(true) {"Evaluate the exit code and output; exit 0 alone does not prove the requested work is correct."} else {"Continue reading with nextCursor; process exit does not imply that all output has reached EOF."}}),
    )
}

fn job_side_effect(
    runtime: &Runtime,
    client_id: &str,
    tool: &str,
    arguments: Value,
) -> Result<Value, Value> {
    let (request_id, job_id, generation, epoch, holder_id, input, hash) =
        if tool == "deck_job_input" {
            let args: InputArgs = parse(arguments)?;
            if args.input.len() > MAX_INPUT_BYTES {
                return Err(error_value(
                    "INVALID_ARGUMENTS",
                    "input exceeds its bound",
                    "Send a smaller input chunk.",
                ));
            }
            let hash = request_hash(tool, &args);
            (
                args.request_id,
                args.job_id,
                args.session_generation,
                args.control_epoch,
                args.holder_id,
                Some(args.input),
                hash,
            )
        } else {
            let args: InterruptArgs = parse(arguments)?;
            let hash = request_hash(tool, &args);
            (
                args.request_id,
                args.job_id,
                args.session_generation,
                args.control_epoch,
                args.holder_id,
                None,
                hash,
            )
        };
    if !valid_id(&request_id) {
        return Err(error_value(
            "INVALID_ARGUMENTS",
            "request id is invalid",
            "Use a stable opaque request id.",
        ));
    }
    let _delivery = runtime.delivery.lock_or_recover();
    let prepared = runtime
        .write(|doc| {
            client(doc, client_id)?;
            if let Some(existing) = existing_operation(doc, client_id, &request_id, &hash)? {
                return Ok((existing.clone(), None));
            }
            // Interrupt may use the reserved slots: stopping work must not
            // fail because ordinary requests filled the journal.
            reserve_operation(
                runtime,
                doc,
                client_id,
                if input.is_none() {
                    Slot::Interrupt
                } else {
                    Slot::Ordinary
                },
            )?;
            let binding = doc
                .jobs
                .iter()
                .find(|job| job.job_id == job_id && job.client_id == client_id)
                .cloned()
                .ok_or_else(|| DeckError::new(ErrorKind::Missing, "job not found"))?;
            let session = authorized_session(doc, client_id, &binding.session_id)?.clone();
            check_control(&session, client_id, &generation, epoch, &holder_id)?;
            let grant = doc
                .execution_grants
                .iter()
                .find(|grant| {
                    grant.grant_id == binding.grant_id
                        && grant.grant_version == binding.grant_version
                        && grant.client_id == client_id
                        && grant.session_id == session.session_id
                        && grant.session_generation == session.generation
                        && grant.service_instance == runtime.service_instance
                })
                .cloned()
                .ok_or_else(|| DeckError::new(ErrorKind::ContextChanged, "job grant changed"))?;
            if input.is_some()
                && (grant.revoked_at.is_some()
                    || grant.grant_version <= grant.revocation_version
                    || !grant.allow_stdin
                    || now_ms() >= grant.expires_at
                    || runtime
                        .monotonic_ms()
                        .saturating_sub(grant.issued_monotonic_ms)
                        >= grant.duration_ms)
            {
                return Err(DeckError::new(
                    ErrorKind::Perm,
                    "interactive stdin is not locally authorized",
                ));
            }
            let operation = Operation {
                operation_id: random_id("op_")?,
                client_id: client_id.into(),
                request_id,
                request_hash: hash,
                kind: if input.is_some() {
                    "job-input"
                } else {
                    "job-interrupt"
                }
                .into(),
                state: "accepted".into(),
                code: None,
                result: Some(json!({"jobId":job_id,"sessionId":session.session_id})),
                accepted_at: now_ms(),
                updated_at: now_ms(),
                admission_hash: None,
                session_id: Some(session.session_id.clone()),
                control_epoch: Some(epoch),
                control_sequence: None,
            };
            doc.operations.push(operation.clone());
            Ok((operation, Some((binding, session, grant))))
        })
        .map_err(map_error)?;
    let (operation, target) = prepared;
    let Some((binding, session, grant)) = target else {
        return Ok(operation_view(&operation));
    };
    pause_point(runtime, "side-effect-final");
    // Final admission under the delivery lock (see `emergency_denial`): stdin
    // is refused by a pending execution revocation; an interrupt is not — it
    // stops work and needs only its own client, control and job authority.
    let admission = if input.is_some() {
        Admission::Execution
    } else {
        Admission::Control
    };
    if let Some(denied) = emergency_denial(runtime, client_id, &session.session_id, admission) {
        let code = denied["error"]["code"]
            .as_str()
            .unwrap_or("ADMISSION_REFUSED")
            .to_owned();
        let _ = runtime.write(|doc| {
            if let Some(saved) = doc
                .operations
                .iter_mut()
                .find(|saved| saved.operation_id == operation.operation_id)
            {
                saved.state = "rejected".into();
                saved.code = Some(code);
                saved.updated_at = now_ms();
            }
            Ok(())
        });
        return Err(denied);
    }
    let context = json!({
        "service_instance": runtime.service_instance,
        "holder_id": holder_id,
        "control_epoch": epoch,
        "grant_id": grant.grant_id,
        "grant_version": grant.grant_version,
        "policy_version": POLICY_VERSION,
        "intent_hash": binding.request_hash,
        "expires_at": if input.is_some() { grant.expires_at } else { u64::MAX },
    });
    let request = if let Some(input) = input {
        json!({"kind":"input","job_id":binding.job_id,"data_b64":base64::engine::general_purpose::STANDARD.encode(input.as_bytes()),"context":context})
    } else {
        json!({"kind":"interrupt","job_id":binding.job_id,"context":context})
    };
    let result = send_runner(runtime, &session, &request);
    let committed = result
        .as_ref()
        .ok()
        .is_some_and(|value| value.get("ok").and_then(Value::as_bool) == Some(true));
    let rejected = result.as_ref().ok().and_then(runner_error);
    let saved = runtime
        .write(|doc| {
            let saved = doc
                .operations
                .iter_mut()
                .find(|saved| saved.operation_id == operation.operation_id)
                .ok_or_else(|| DeckError::new(ErrorKind::Missing, "operation not found"))?;
            saved.state = if committed {
                "committed"
            } else if rejected.is_some() {
                "rejected"
            } else {
                "ambiguous"
            }
            .into();
            saved.code = if committed {
                None
            } else {
                Some(
                    rejected
                        .map(|(code, _)| code)
                        .unwrap_or("delivery-unknown")
                        .into(),
                )
            };
            saved.updated_at = now_ms();
            Ok(saved.clone())
        })
        .map_err(map_error)?;
    if committed {
        Ok(operation_view(&saved))
    } else if let Ok(value) = result {
        if let Some((code, next)) = runner_error(&value) {
            return Err(error_value(
                code,
                "managed runner rejected the job side effect",
                next,
            ));
        }
        Err(error_value(
            "OPERATION_AMBIGUOUS",
            "Deck cannot confirm the job side effect",
            "Read the job and operation before deciding what to do next.",
        ))
    } else {
        Err(error_value(
            "OPERATION_AMBIGUOUS",
            "Deck cannot confirm the job side effect",
            "Read the job and operation before deciding what to do next.",
        ))
    }
}

fn session_close(runtime: &Runtime, client_id: &str, arguments: Value) -> Result<Value, Value> {
    let args: CloseArgs = parse(arguments)?;
    if !valid_id(&args.request_id) {
        return Err(error_value(
            "INVALID_ARGUMENTS",
            "request id is invalid",
            "Use a stable opaque request id.",
        ));
    }
    let hash = request_hash("deck_session_close", &args);
    let _delivery = runtime.delivery.lock_or_recover();
    if let Some(denied) = emergency_denial(runtime, client_id, &args.session_id, Admission::Control)
    {
        return Err(denied);
    }
    // Replay, authorization and control are checked first without I/O. The
    // runner ping happens OUTSIDE the state write: the doc lock also guards
    // every terminal keystroke (`guard_terminal_input`). No job can start
    // meanwhile because exec needs the delivery lock held here.
    let session = runtime
        .read(|doc| {
            client(doc, client_id)?;
            if let Some(existing) = existing_operation(doc, client_id, &args.request_id, &hash)? {
                return Ok(Err(existing.clone()));
            }
            let session = authorized_session(doc, client_id, &args.session_id)?.clone();
            check_control(
                &session,
                client_id,
                &args.expected_generation,
                args.control_epoch,
                &args.holder_id,
            )?;
            Ok(Ok(session))
        })
        .map_err(map_error)?
        .map_err(map_error)?;
    let session = match session {
        Ok(session) => session,
        Err(existing) => return Ok(operation_view(&existing)),
    };
    let runner = send_runner(runtime, &session, &json!({"kind":"ping"})).map_err(|_| {
        map_error(DeckError::new(
            ErrorKind::ContextChanged,
            "managed runner state is unknown; remote close is refused",
        ))
    })?;
    let active = runner.get("job").is_some_and(|job| !job.is_null());
    if active && !args.confirm_running {
        return Err(map_error(DeckError::new(
            ErrorKind::Locked,
            "session has a running job",
        )));
    }
    let operation = runtime.write(|doc| {
        client(doc, client_id)?;
        if let Some(existing) = existing_operation(doc, client_id, &args.request_id, &hash)? { return Ok(existing.clone()); }
        reserve_operation(runtime, doc, client_id, Slot::Ordinary)?;
        let current = authorized_session(doc, client_id, &args.session_id)?.clone();
        check_control(&current, client_id, &args.expected_generation, args.control_epoch, &args.holder_id)?;
        let managed = doc.sessions.iter_mut().find(|item| item.session_id == session.session_id).ok_or_else(|| DeckError::new(ErrorKind::Missing, "session not found"))?;
        managed.closing = true;
        let operation = Operation { operation_id:random_id("op_")?, client_id:client_id.into(), request_id:args.request_id.clone(), request_hash:hash.clone(), kind:"session-close".into(), state:"accepted".into(), code:None, result:Some(json!({"sessionId":session.session_id,"cardId":session.card_id,"sessionGeneration":session.generation,"controlEpoch":session.control_epoch,"holderId":args.holder_id,"confirmRunning":args.confirm_running})), accepted_at:now_ms(), updated_at:now_ms(), admission_hash:None, session_id: Some(session.session_id.clone()), control_epoch: Some(args.control_epoch), control_sequence: None };
        doc.operations.push(operation.clone());
        Ok(operation)
    }).map_err(map_error)?;
    emit_changed(runtime);
    Ok(operation_view(&operation))
}

fn route(runtime: &Runtime, request: WireRequest) -> Value {
    if request.version != CONTROL_PROTOCOL {
        return error_value(
            "PROTOCOL_MISMATCH",
            "the adapter and Deck speak different control protocol versions",
            "Use the deck-mcp adapter bundled with the running Deck build.",
        );
    }
    if !valid_id(&request.client_id) || request.credential.len() > 128 {
        return error_value(
            "AUTH_REQUIRED",
            "invalid MCP control request",
            "Use the bundled deck-mcp adapter and an authorized client id.",
        );
    }
    // The socket always listens (it is private and same-uid only). While the
    // feature is off every tool — including capabilities — is refused with
    // this distinct code; it discloses nothing about clients or scopes.
    let disabled = runtime.emergency.lock_or_recover().disabled
        || !runtime.read(|doc| doc.config.enabled).unwrap_or(false);
    if disabled {
        return error_value(
            "FEATURE_DISABLED",
            "MCP terminal control is disabled in Deck",
            "Ask the local Deck user to enable MCP terminal control in Settings.",
        );
    }
    let emergency_auth_blocked = runtime
        .emergency
        .lock_or_recover()
        .clients
        .contains(&request.client_id);
    let authenticated = !emergency_auth_blocked
        && runtime
            .read(|doc| {
                doc.config.enabled
                    && doc.config.clients.iter().any(|client| {
                        client.id == request.client_id
                            && client.revoked_at.is_none()
                            && client.credential_version > 0
                            && secret_hash_matches(
                                &client.credential_hash,
                                &sha(request.credential.as_bytes()),
                            )
                    })
            })
            .unwrap_or(false);
    if !authenticated {
        return error_value(
            "AUTH_REQUIRED",
            "MCP credential is invalid or revoked",
            "Reauthorize this integration locally in Deck.",
        );
    }
    let request_session = request
        .arguments
        .get("session_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            let job_id = request.arguments.get("job_id")?.as_str()?;
            runtime
                .read(|doc| {
                    doc.jobs
                        .iter()
                        .find(|job| job.job_id == job_id && job.client_id == request.client_id)
                        .map(|job| job.session_id.clone())
                })
                .ok()
                .flatten()
        });
    if let Some(session_id) = request_session.as_deref() {
        let emergency = runtime.emergency.lock_or_recover();
        if emergency.human_sessions.contains(session_id)
            && matches!(
                request.tool.as_str(),
                "deck_session_control"
                    | "deck_exec"
                    | "deck_job_read"
                    | "deck_job_input"
                    | "deck_session_close"
            )
        {
            return error_value(
                "HUMAN_CONTROL",
                "local takeover has fenced remote access",
                "Wait for the local user to explicitly return control.",
            );
        }
        if emergency.execution_fenced(session_id)
            && matches!(request.tool.as_str(), "deck_exec" | "deck_job_input")
        {
            return error_value(
                "EXECUTION_GRANT_REQUIRED",
                "local execution authority was revoked",
                "Ask the local Deck user to approve a new execution window.",
            );
        }
    }
    let audit_tool = request.tool.clone();
    let audit_principal = request.client_id.clone();
    let result = match request.tool.as_str() {
        "deck_capabilities" => capabilities(runtime, &request.client_id, request.arguments),
        "deck_project_list" => project_list(runtime, &request.client_id, request.arguments),
        "deck_project_read" => project_read(runtime, &request.client_id, request.arguments),
        "deck_project_search" => project_search(runtime, &request.client_id, request.arguments),
        "deck_sessions_list" => sessions_list(runtime, &request.client_id, request.arguments),
        "deck_session_create" => session_create(runtime, &request.client_id, request.arguments),
        "deck_operation_get" => operation_get(runtime, &request.client_id, request.arguments),
        "deck_session_inspect" => inspect(runtime, &request.client_id, request.arguments),
        "deck_session_control" => session_control(runtime, &request.client_id, request.arguments),
        "deck_exec" => exec(runtime, &request.client_id, request.arguments),
        "deck_job_read" => job_read(runtime, &request.client_id, request.arguments),
        "deck_job_input" | "deck_job_interrupt" => job_side_effect(
            runtime,
            &request.client_id,
            &request.tool,
            request.arguments,
        ),
        "deck_session_close" => session_close(runtime, &request.client_id, request.arguments),
        _ => Err(error_value(
            "UNSUPPORTED",
            "unknown Deck MCP tool",
            "Refresh tools/list and use an advertised tool.",
        )),
    };
    if let Err(error) = &result {
        let kind = if audit_tool.starts_with("deck_project_") {
            Some("read-denied")
        } else if matches!(
            audit_tool.as_str(),
            "deck_exec" | "deck_job_input" | "deck_session_create" | "deck_session_close"
        ) {
            Some("request-denied")
        } else {
            None
        };
        if let Some(kind) = kind {
            let reason = error
                .get("error")
                .and_then(|value| value.get("code"))
                .and_then(Value::as_str)
                .unwrap_or("rejected");
            let _ = runtime.write(|doc| {
                audit(
                    doc,
                    kind,
                    AuditLink {
                        principal_id: Some(&audit_principal),
                        reason_code: Some(reason),
                        ..Default::default()
                    },
                )
            });
        }
    }
    result.unwrap_or_else(|error| error)
}

#[cfg(target_os = "macos")]
fn same_uid(stream: &UnixStream) -> bool {
    let mut uid = 0;
    let mut gid = 0;
    // SAFETY: getpeereid writes two scalar outputs for this connected socket.
    unsafe {
        libc::getpeereid(std::os::fd::AsRawFd::as_raw_fd(stream), &mut uid, &mut gid) == 0
            && uid == libc::geteuid()
    }
}

#[cfg(not(target_os = "macos"))]
fn same_uid(_stream: &UnixStream) -> bool {
    false
}

/// Live control connections; excess connections are closed immediately.
static CONNECTIONS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// Serializes bind/remove decisions with enable and disable. The listener
/// itself remains owned by the one control thread.
static SOCKET_LIFECYCLE: Mutex<()> = Mutex::new(());

fn handle_connection(runtime: Arc<Runtime>, mut stream: UnixStream) {
    if !same_uid(&stream) {
        return;
    }
    // A same-uid peer that connects and never sends a full request cannot pin
    // a thread forever; neither can one that never reads its response.
    if stream.set_read_timeout(Some(CONNECTION_TIMEOUT)).is_err()
        || stream.set_write_timeout(Some(CONNECTION_TIMEOUT)).is_err()
    {
        return;
    }
    let cloned = stream.try_clone();
    let response = match cloned {
        Ok(cloned) => {
            let mut line = Vec::new();
            match BufReader::new(cloned.take((MAX_REQUEST_BYTES + 1) as u64))
                .read_until(b'\n', &mut line)
            {
                Ok(size) if size > 0 && size <= MAX_REQUEST_BYTES && line.ends_with(b"\n") => {
                    serde_json::from_slice::<WireRequest>(&line)
                        .map(|request| route(&runtime, request))
                        .unwrap_or_else(|_| {
                            error_value(
                                "INVALID_ARGUMENTS",
                                "invalid local control request",
                                "Use the bundled deck-mcp adapter.",
                            )
                        })
                }
                _ => error_value(
                    "INVALID_ARGUMENTS",
                    "invalid local control request",
                    "Use the bundled deck-mcp adapter.",
                ),
            }
        }
        Err(_) => error_value(
            "INTERNAL_ERROR",
            "control connection failed",
            "Reconnect the adapter.",
        ),
    };
    if let Ok(mut bytes) = serde_json::to_vec(&response) {
        if bytes.len() > MAX_RESPONSE_BYTES {
            bytes = serde_json::to_vec(&error_value(
                "INTERNAL_ERROR",
                "response exceeded its bound",
                "Use a smaller read request.",
            ))
            .unwrap_or_default();
        }
        let _ = stream.write_all(&bytes);
        let _ = stream.write_all(b"\n");
        let _ = stream.flush();
    }
}

fn bind_private_socket(socket: &Path) -> Result<UnixListener, DeckError> {
    let parent = socket
        .parent()
        .ok_or_else(|| DeckError::new(ErrorKind::Invalid, "MCP socket path is invalid"))?;
    // The private Deck directory and single-instance lock make this name
    // exclusive; keeping it short also preserves macOS SUN_LEN headroom.
    let temporary = parent.join(".mcp-bind");
    let _ = std::fs::remove_file(&temporary);
    let listener = UnixListener::bind(&temporary).map_err(DeckError::from)?;
    if let Err(error) = std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))
        .and_then(|_| std::fs::rename(&temporary, socket))
    {
        let _ = std::fs::remove_file(&temporary);
        return Err(DeckError::from(error));
    }
    Ok(listener)
}

fn socket_should_listen(runtime: &Runtime) -> bool {
    runtime.read(|doc| doc.config.enabled).unwrap_or(false)
        && !runtime.emergency.lock_or_recover().disabled
}

fn reconcile_control_socket(
    runtime: &Runtime,
    listener: &mut Option<UnixListener>,
) -> Result<(), DeckError> {
    let _lifecycle = SOCKET_LIFECYCLE.lock_or_recover();
    if !socket_should_listen(runtime) {
        *listener = None;
        match std::fs::remove_file(&runtime.socket) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(DeckError::from(error)),
        }
        return Ok(());
    }
    if listener.is_some() {
        return Ok(());
    }
    let _ = std::fs::remove_file(&runtime.socket);
    let bound = bind_private_socket(&runtime.socket)?;
    bound.set_nonblocking(true).map_err(DeckError::from)?;
    *listener = Some(bound);
    Ok(())
}

pub(crate) fn spawn(app: AppHandle) {
    let dir = crate::datadir::deck_dir();
    let path = dir.join("mcp.json");
    let socket = dir.join("mcp-control.sock");
    let runtime = Arc::new(Runtime {
        app: Some(app),
        path: path.clone(),
        socket: socket.clone(),
        doc: Mutex::new(load(&path)),
        io: Mutex::new(()),
        delivery: Mutex::new(()),
        emergency: Mutex::new(EmergencyFences::default()),
        service_instance: random_id("svc_").unwrap_or_else(|_| "svc_unavailable".into()),
        runner_auth: Mutex::new(HashMap::new()),
        started: Instant::now(),
    });
    let _ = RUNTIME.set(runtime.clone());
    std::thread::Builder::new()
        .name("deck-mcp-control".into())
        .spawn(move || {
            let mut listener = None;
            loop {
                let _ = reconcile_control_socket(&runtime, &mut listener);
                if let Some(bound) = &listener {
                    match bound.accept() {
                        Ok((stream, _)) => {
                            use std::sync::atomic::Ordering;
                            if CONNECTIONS.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                                CONNECTIONS.fetch_sub(1, Ordering::SeqCst);
                                continue;
                            }
                            let runtime = runtime.clone();
                            std::thread::spawn(move || {
                                handle_connection(runtime, stream);
                                CONNECTIONS.fetch_sub(1, Ordering::SeqCst);
                            });
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                        Err(_) => listener = None,
                    }
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        })
        .ok();
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StatusView {
    enabled: bool,
    socket_ready: bool,
    clients: Vec<ClientView>,
    output_retention_ms: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClientView {
    id: String,
    name: String,
    revoked: bool,
    allow_create: bool,
    projects: Vec<ProjectScope>,
}

#[tauri::command]
pub(crate) fn mcp_status() -> Result<StatusView, DeckError> {
    let runtime = runtime()?;
    let emergency = runtime.emergency.lock_or_recover();
    runtime.read(|doc| StatusView {
        enabled: doc.config.enabled && !emergency.disabled,
        socket_ready: runtime.socket.exists(),
        output_retention_ms: doc.config.output_retention_ms,
        clients: doc
            .config
            .clients
            .iter()
            .map(|client| ClientView {
                id: client.id.clone(),
                name: client.name.clone(),
                revoked: client.revoked_at.is_some() || emergency.clients.contains(&client.id),
                allow_create: client.allow_create,
                projects: client.projects.clone(),
            })
            .collect(),
    })
}

#[tauri::command]
pub(crate) fn mcp_output_retention(duration_ms: u64) -> Result<(), DeckError> {
    if !(60_000..=MAX_OUTPUT_RETENTION_MS).contains(&duration_ms) {
        return Err(DeckError::new(
            ErrorKind::Invalid,
            "MCP output retention must be between one minute and seven days",
        ));
    }
    let runtime = runtime()?;
    let _delivery = runtime.delivery.lock_or_recover();
    let sessions = runtime.write(|doc| {
        doc.config.output_retention_ms = duration_ms;
        Ok(doc.sessions.clone())
    })?;
    let mut uncertain = false;
    for session in sessions {
        let applied = send_runner(
            runtime,
            &session,
            &json!({"kind":"retention","service_instance":runtime.service_instance,"output_retention_ms":duration_ms}),
        )
        .ok()
        .is_some_and(|value| value.get("ok").and_then(Value::as_bool) == Some(true));
        uncertain |= !applied;
    }
    if uncertain {
        return Err(DeckError::new(
            ErrorKind::ContextChanged,
            "retention is saved for new runners but one or more live runners did not confirm it",
        ));
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn mcp_adapter_path() -> Result<String, DeckError> {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join("deck-mcp")))
        .filter(|path| path.is_file())
        .map(|path| path.display().to_string())
        .ok_or_else(|| DeckError::new(ErrorKind::Missing, "MCP adapter is not bundled"))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ProjectScopeInput {
    project_id: String,
    roots: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScopePreview {
    ok: bool,
    root: Option<String>,
    error: Option<&'static str>,
}

fn canonical_project_root(root: &str) -> Result<String, DeckError> {
    canonical_project_root_with_access(root, |path| std::fs::read_dir(path).map(|_| ()))
}

fn canonical_project_root_with_access(
    root: &str,
    access: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<String, DeckError> {
    let path = std::fs::canonicalize(root).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => {
            DeckError::new(ErrorKind::Missing, "authorized root does not exist")
        }
        std::io::ErrorKind::PermissionDenied => {
            DeckError::new(ErrorKind::Perm, "authorized root is not accessible")
        }
        std::io::ErrorKind::NotADirectory => {
            DeckError::new(ErrorKind::NotDir, "authorized root is not a directory")
        }
        _ => DeckError::new(ErrorKind::Other, "authorized root could not be resolved"),
    })?;
    if !path.is_dir() {
        return Err(DeckError::new(
            ErrorKind::NotDir,
            "authorized root is not a directory",
        ));
    }
    access(&path).map_err(|error| match error.kind() {
        std::io::ErrorKind::PermissionDenied => {
            DeckError::new(ErrorKind::Perm, "authorized root is not accessible")
        }
        std::io::ErrorKind::NotFound => {
            DeckError::new(ErrorKind::Missing, "authorized root does not exist")
        }
        std::io::ErrorKind::NotADirectory => {
            DeckError::new(ErrorKind::NotDir, "authorized root is not a directory")
        }
        _ => DeckError::new(ErrorKind::Other, "authorized root could not be inspected"),
    })?;
    // `/`, the account home, its ancestors and excluded directories would turn
    // a project scope into account-wide structured reads.
    crate::mcp_fs::root_policy(&path)
        .map_err(|error| DeckError::new(ErrorKind::Invalid, error.message))?;
    Ok(path.display().to_string())
}

#[tauri::command]
pub(crate) fn mcp_scope_preview(root: String) -> ScopePreview {
    match canonical_project_root(&root) {
        Ok(root) => ScopePreview {
            ok: true,
            root: Some(root),
            error: None,
        },
        Err(error) => ScopePreview {
            ok: false,
            root: None,
            error: Some(match error.kind() {
                ErrorKind::Missing => "not_found",
                ErrorKind::NotDir => "not_directory",
                ErrorKind::Perm => "not_accessible",
                ErrorKind::Invalid => "too_broad",
                _ => "unavailable",
            }),
        },
    }
}

#[tauri::command]
pub(crate) fn mcp_enable() -> Result<(), DeckError> {
    let runtime = runtime()?;
    {
        let _lifecycle = SOCKET_LIFECYCLE.lock_or_recover();
        runtime.write(|doc| {
            doc.config.enabled = true;
            Ok(())
        })?;
        runtime.emergency.lock_or_recover().disabled = false;
    }
    Ok(())
}

/// Disable fences every client in memory BEFORE waiting for the delivery lock,
/// then persists and hands every managed pane to the local user (human mode:
/// the keyboard and the ^C stop key reach the job; MCP reaches nothing).
#[tauri::command(async)]
pub(crate) fn mcp_disable() -> Result<(), DeckError> {
    disable(runtime()?)
}

fn disable(runtime: &Runtime) -> Result<(), DeckError> {
    runtime.emergency.lock_or_recover().disabled = true;
    {
        let _lifecycle = SOCKET_LIFECYCLE.lock_or_recover();
        match std::fs::remove_file(&runtime.socket) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(DeckError::from(error)),
        }
    }
    let _delivery = runtime.delivery.lock_or_recover();
    let sessions = runtime.write(|doc| {
        doc.config.enabled = false;
        for grant in &mut doc.execution_grants {
            if grant.revoked_at.is_none() {
                grant.revoked_at = Some(now_ms());
                grant.revocation_version = grant.grant_version;
            }
        }
        let closing = doc
            .operations
            .iter()
            .filter(|operation| operation.state == "accepted" && operation.kind == "session-close")
            .filter_map(|operation| operation.result.as_ref()?.get("sessionId")?.as_str())
            .map(str::to_owned)
            .collect::<HashSet<_>>();
        for session in &mut doc.sessions {
            session.control_owner = None;
            session.control_holder = None;
            session.control_epoch = session.control_epoch.saturating_add(1);
            session.lease_expires_at = None;
            session.human_lock = true;
        }
        // The human now owns every pane; no earlier binding reads what follows.
        for job in &mut doc.jobs {
            job.allow_output = false;
        }
        for operation in &mut doc.operations {
            if operation.state == "accepted" {
                operation.state = "rejected".into();
                operation.code = Some("feature-disabled".into());
                operation.updated_at = now_ms();
            }
        }
        for session in &mut doc.sessions {
            if closing.contains(&session.session_id) {
                session.closing = false;
            }
        }
        Ok(doc.sessions.clone())
    })?;
    let mut uncertain = false;
    for session in sessions {
        uncertain |=
            runner_control_result(send_runner_control(runtime, &session, "human")).is_err();
    }
    if uncertain {
        return Err(DeckError::new(
            ErrorKind::ContextChanged,
            "MCP is disabled locally but one or more runner fences are unconfirmed",
        ));
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn mcp_client_add(
    name: String,
    projects: Vec<ProjectScopeInput>,
    allow_create: bool,
) -> Result<ClientView, DeckError> {
    if !valid_title(&name) || projects.is_empty() || projects.len() > MAX_PROJECTS_PER_CLIENT {
        return Err(DeckError::new(
            ErrorKind::Invalid,
            "invalid MCP client scope",
        ));
    }
    let runtime = runtime()?;
    let enforce_projects = runtime.app.is_some();
    let scopes = validate_new_client_scopes(projects, enforce_projects, |project_id| {
        crate::documents::board_project_exists(project_id)
    })?;
    let client_id = random_id("client_")?;
    let credential = random_id("mcp_")?;
    if runtime.app.is_some() {
        crate::keychain::set_mcp_credential(&client_id, &credential)?;
    }
    let result = runtime.write(|doc| {
        if doc.config.clients.len() >= MAX_CLIENTS {
            return Err(DeckError::new(
                ErrorKind::DiskFull,
                "MCP client capacity reached",
            ));
        }
        let client = Client {
            id: client_id.clone(),
            name,
            credential_hash: sha(credential.as_bytes()),
            credential_version: 1,
            revoked_at: None,
            allow_create,
            projects: scopes,
            create_sequence: 0,
        };
        doc.config.clients.push(client.clone());
        Ok(ClientView {
            id: client.id,
            name: client.name,
            revoked: false,
            allow_create: client.allow_create,
            projects: client.projects,
        })
    });
    if result.is_err() && runtime.app.is_some() {
        let _ = crate::keychain::clear_mcp_credential(&client_id);
    }
    result
}

fn validate_new_client_scopes(
    projects: Vec<ProjectScopeInput>,
    enforce_projects: bool,
    mut project_exists: impl FnMut(&str) -> Result<bool, DeckError>,
) -> Result<Vec<ProjectScope>, DeckError> {
    let mut scopes = Vec::new();
    for project in projects {
        if !valid_id(&project.project_id)
            || project.roots.is_empty()
            || project.roots.len() > MAX_ROOTS_PER_PROJECT
        {
            return Err(DeckError::new(
                ErrorKind::Invalid,
                "invalid MCP project scope",
            ));
        }
        let roots = project
            .roots
            .into_iter()
            .map(|root| canonical_project_root(&root))
            .collect::<Result<Vec<_>, _>>()?;
        if enforce_projects && !project_exists(&project.project_id)? {
            return Err(DeckError::new(
                ErrorKind::Missing,
                "MCP project no longer exists",
            ));
        }
        scopes.push(ProjectScope {
            project_id: project.project_id,
            roots,
        });
    }
    Ok(scopes)
}

/// Revocation fences the client in memory BEFORE waiting for the delivery
/// lock, persists, then hands the client's panes to the local user.
#[tauri::command(async)]
pub(crate) fn mcp_client_revoke(client_id: String) -> Result<(), DeckError> {
    client_revoke(runtime()?, client_id)
}

fn client_revoke(runtime: &Runtime, client_id: String) -> Result<(), DeckError> {
    runtime
        .read(|doc| {
            doc.config
                .clients
                .iter()
                .any(|client| client.id == client_id)
        })?
        .then_some(())
        .ok_or_else(|| DeckError::new(ErrorKind::Missing, "MCP client not found"))?;
    runtime
        .emergency
        .lock_or_recover()
        .clients
        .insert(client_id.clone());
    let _delivery = runtime.delivery.lock_or_recover();
    let sessions =
        runtime.write(|doc| {
            let client = doc
                .config
                .clients
                .iter_mut()
                .find(|client| client.id == client_id)
                .ok_or_else(|| DeckError::new(ErrorKind::Missing, "MCP client not found"))?;
            client.revoked_at = Some(now_ms());
            let closing = doc
                .operations
                .iter()
                .filter(|operation| {
                    operation.client_id == client_id
                        && operation.state == "accepted"
                        && operation.kind == "session-close"
                })
                .filter_map(|operation| operation.result.as_ref()?.get("sessionId")?.as_str())
                .map(str::to_owned)
                .collect::<HashSet<_>>();
            for grant in doc
                .execution_grants
                .iter_mut()
                .filter(|grant| grant.client_id == client_id && grant.revoked_at.is_none())
            {
                grant.revoked_at = Some(now_ms());
                grant.revocation_version = grant.grant_version;
            }
            for session in doc
                .sessions
                .iter_mut()
                .filter(|session| session.owner_client_id == client_id)
            {
                session.control_owner = None;
                session.control_holder = None;
                session.control_epoch = session.control_epoch.saturating_add(1);
                session.lease_expires_at = None;
                session.human_lock = true;
            }
            for job in doc.jobs.iter_mut().filter(|job| job.client_id == client_id) {
                job.allow_output = false;
            }
            for operation in doc.operations.iter_mut().filter(|operation| {
                operation.client_id == client_id && operation.state == "accepted"
            }) {
                operation.state = "rejected".into();
                operation.code = Some("client-revoked".into());
                operation.updated_at = now_ms();
            }
            for session in &mut doc.sessions {
                if closing.contains(&session.session_id) {
                    session.closing = false;
                }
            }
            audit(
                doc,
                "client-revoked",
                AuditLink {
                    principal_id: Some(&client_id),
                    ..Default::default()
                },
            )?;
            Ok(doc
                .sessions
                .iter()
                .filter(|session| session.owner_client_id == client_id)
                .cloned()
                .collect::<Vec<_>>())
        })?;
    let mut uncertain = false;
    for session in sessions {
        uncertain |=
            runner_control_result(send_runner_control(runtime, &session, "human")).is_err();
    }
    if runtime.app.is_some() {
        crate::keychain::clear_mcp_credential(&client_id)?;
    }
    if uncertain {
        return Err(DeckError::new(
            ErrorKind::ContextChanged,
            "client is revoked locally but one or more runner fences are unconfirmed",
        ));
    }
    Ok(())
}

/// Remove a local authorization display record after revocation has already
/// fenced its sessions and pending side effects. Historical sessions,
/// operations, jobs, grants, and audit links intentionally retain the opaque
/// client id so deleting a client cannot erase the security ledger.
#[tauri::command]
pub(crate) fn mcp_client_delete(client_id: String) -> Result<(), DeckError> {
    if !valid_id(&client_id) {
        return Err(DeckError::new(ErrorKind::Invalid, "invalid MCP client id"));
    }
    let runtime = runtime()?;
    let _delivery = runtime.delivery.lock_or_recover();
    runtime
        .read(|doc| {
            doc.config
                .clients
                .iter()
                .find(|client| client.id == client_id)
                .map(|client| client.revoked_at.is_some())
        })?
        .ok_or_else(|| DeckError::new(ErrorKind::Missing, "MCP client not found"))?
        .then_some(())
        .ok_or_else(|| {
            DeckError::new(ErrorKind::Perm, "revoke the MCP client before deleting it")
        })?;
    if runtime.app.is_some() {
        crate::keychain::clear_mcp_credential(&client_id)?;
    }
    runtime.write(|doc| {
        let index = doc
            .config
            .clients
            .iter()
            .position(|client| client.id == client_id && client.revoked_at.is_some())
            .ok_or_else(|| DeckError::new(ErrorKind::Missing, "revoked MCP client not found"))?;
        doc.config.clients.remove(index);
        audit(
            doc,
            "client-deleted",
            AuditLink {
                principal_id: Some(&client_id),
                ..Default::default()
            },
        )?;
        Ok(())
    })?;
    runtime
        .emergency
        .lock_or_recover()
        .clients
        .remove(&client_id);
    Ok(())
}

/// Create a short-lived trusted-host execution window. This command is only
/// exposed to the local Tauri UI; there is intentionally no MCP route for it.
#[tauri::command(async)]
pub(crate) fn mcp_execution_grant(
    session_id: String,
    duration_ms: Option<u64>,
    allow_stdin: bool,
    allow_output: bool,
) -> Result<(), DeckError> {
    execution_grant(
        runtime()?,
        session_id,
        duration_ms,
        allow_stdin,
        allow_output,
    )
}

fn execution_grant(
    runtime: &Runtime,
    session_id: String,
    duration_ms: Option<u64>,
    allow_stdin: bool,
    allow_output: bool,
) -> Result<(), DeckError> {
    let _delivery = runtime.delivery.lock_or_recover();
    let duration = duration_ms
        .unwrap_or(DEFAULT_EXECUTION_GRANT_MS)
        .clamp(60_000, MAX_EXECUTION_GRANT_MS);
    let service_instance = runtime.service_instance.clone();
    let monotonic = runtime.monotonic_ms();
    let (session, revoked) =
        runtime.write(|doc| {
            let session = doc
                .sessions
                .iter()
                .find(|session| session.session_id == session_id || session.card_id == session_id)
                .cloned()
                .ok_or_else(|| DeckError::new(ErrorKind::Missing, "MCP session not found"))?;
            let session_for_runner = session.clone();
            let credential_version = client(doc, &session.owner_client_id)?.credential_version;
            let version = doc
                .execution_grants
                .iter()
                .filter(|grant| grant.session_id == session.session_id)
                .map(|grant| grant.grant_version)
                .max()
                .unwrap_or(0)
                .saturating_add(1);
            let mut revoked = Vec::new();
            for grant in doc.execution_grants.iter_mut().filter(|grant| {
                grant.session_id == session.session_id && grant.revoked_at.is_none()
            }) {
                grant.revoked_at = Some(now_ms());
                grant.revocation_version = grant.grant_version;
                revoked.push((grant.grant_id.clone(), grant.grant_version));
            }
            let granted_session_id = session.session_id.clone();
            let principal_id = session.owner_client_id.clone();
            let grant_id = random_id("grant_")?;
            // Bounded by construction: superseded, closed-session and
            // earlier-process grants are retired before a new one is added,
            // so a later write (a takeover) can never fail validation.
            compact(doc, Some(&runtime.service_instance));
            if doc.execution_grants.len() >= MAX_GRANTS {
                return Err(DeckError::new(
                    ErrorKind::DiskFull,
                    "MCP execution grant capacity reached",
                ));
            }
            doc.execution_grants.push(ExecutionGrant {
                grant_id: grant_id.clone(),
                client_id: session.owner_client_id,
                credential_version,
                project_id: session.project_id,
                session_id: session.session_id,
                session_generation: session.generation,
                profile: "trusted-host-v1".into(),
                environment_profile: ENVIRONMENT_PROFILE.into(),
                environment_profile_version: 1,
                issued_at: now_ms(),
                expires_at: now_ms().saturating_add(duration),
                issued_monotonic_ms: monotonic,
                duration_ms: duration,
                allow_stdin,
                allow_output,
                _legacy_allow_shell: None,
                grant_version: version,
                revocation_version: version.saturating_sub(1),
                service_instance,
                revoked_at: None,
            });
            if let Some(managed) = doc
                .sessions
                .iter_mut()
                .find(|managed| managed.session_id == granted_session_id)
            {
                managed.output_shared = allow_output;
            }
            audit(
                doc,
                "grant-approved",
                AuditLink {
                    principal_id: Some(&principal_id),
                    session_id: Some(&granted_session_id),
                    grant_id: Some(&grant_id),
                    ..Default::default()
                },
            )?;
            Ok((session_for_runner, revoked))
        })?;
    let mut uncertain = false;
    for (grant_id, grant_version) in revoked {
        uncertain |= send_runner(
            runtime,
            &session,
            &json!({"kind":"revoke-grant","service_instance":runtime.service_instance,"grant_id":grant_id,"grant_version":grant_version}),
        )
        .ok()
        .is_none_or(|value| value.get("ok").and_then(Value::as_bool) != Some(true));
    }
    if uncertain {
        return Err(DeckError::new(
            ErrorKind::ContextChanged,
            "the new grant is saved but an older runner grant fence is unconfirmed",
        ));
    }
    // An approval lifts only a revocation that failed to persist BEFORE it
    // (both ran under the delivery lock, so that one is ordered earlier and
    // this approval's write revoked its grants durably). A revocation that is
    // still pending keeps its own count.
    runtime
        .emergency
        .lock_or_recover()
        .execution_unpersisted
        .remove(&session.session_id);
    Ok(())
}

#[tauri::command(async)]
pub(crate) fn mcp_execution_revoke(session_id: String) -> Result<(), DeckError> {
    execution_revoke(runtime()?, &session_id)
}

fn execution_revoke(runtime: &Runtime, session_id: &str) -> Result<(), DeckError> {
    let fenced_session_id = resolve_session_id(runtime, session_id)?;
    *runtime
        .emergency
        .lock_or_recover()
        .execution_pending
        .entry(fenced_session_id.clone())
        .or_default() += 1;
    pause_point(runtime, "revoke-fenced");
    let _delivery = runtime.delivery.lock_or_recover();
    let persisted =
        runtime.write(|doc| {
            let session = doc
                .sessions
                .iter()
                .find(|session| session.session_id == session_id || session.card_id == session_id)
                .cloned()
                .ok_or_else(|| DeckError::new(ErrorKind::Missing, "MCP session not found"))?;
            let mut revoked = Vec::new();
            for grant in doc.execution_grants.iter_mut().filter(|grant| {
                grant.session_id == session.session_id && grant.revoked_at.is_none()
            }) {
                grant.revoked_at = Some(now_ms());
                grant.revocation_version = grant.grant_version;
                revoked.push((grant.grant_id.clone(), grant.grant_version));
            }
            audit(
                doc,
                "grant-revoked",
                AuditLink {
                    principal_id: Some(&session.owner_client_id),
                    session_id: Some(&session.session_id),
                    ..Default::default()
                },
            )?;
            // Output sharing is a separate local switch: revoking execution
            // leaves it exactly as it was.
            Ok((session, revoked))
        });
    {
        // Still under the delivery lock: the persisted revocation (or, when it
        // failed, the sticky fence) replaces this revocation's pending count
        // before any waiting request or approval can run.
        let mut emergency = runtime.emergency.lock_or_recover();
        if let Some(count) = emergency.execution_pending.get_mut(&fenced_session_id) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                emergency.execution_pending.remove(&fenced_session_id);
            }
        }
        if persisted.is_err() {
            emergency.execution_unpersisted.insert(fenced_session_id);
        }
    }
    let (session, revoked) = persisted?;
    let mut uncertain = false;
    for (grant_id, grant_version) in revoked {
        uncertain |= send_runner(
            runtime,
            &session,
            &json!({"kind":"revoke-grant","service_instance":runtime.service_instance,"grant_id":grant_id,"grant_version":grant_version}),
        )
        .ok()
        .is_none_or(|value| value.get("ok").and_then(Value::as_bool) != Some(true));
    }
    if uncertain {
        return Err(DeckError::new(
            ErrorKind::ContextChanged,
            "execution is revoked locally but the runner revocation fence is unconfirmed",
        ));
    }
    Ok(())
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PendingView {
    operation_id: String,
    kind: String,
    result: Value,
}

#[tauri::command]
pub(crate) fn mcp_pending() -> Result<Vec<PendingView>, DeckError> {
    runtime()?.read(|doc| {
        if !doc.config.enabled {
            Vec::new()
        } else {
            doc.operations
                .iter()
                .filter(|operation| {
                    operation.state == "accepted"
                        && matches!(operation.kind.as_str(), "session-create" | "session-close")
                })
                .filter_map(|operation| {
                    operation.result.clone().map(|result| PendingView {
                        operation_id: operation.operation_id.clone(),
                        kind: operation.kind.clone(),
                        result,
                    })
                })
                .collect()
        }
    })
}

#[tauri::command]
pub(crate) fn mcp_claim(operation_id: String) -> Result<PendingView, DeckError> {
    claim(runtime()?, &operation_id)
}

fn claim(runtime: &Runtime, operation_id: &str) -> Result<PendingView, DeckError> {
    runtime.write(|doc| {
        if !doc.config.enabled {
            return Err(DeckError::new(ErrorKind::Perm, "MCP control is disabled"));
        }
        let operation = doc
            .operations
            .iter_mut()
            .find(|operation| {
                operation.operation_id == operation_id && operation.state == "accepted"
            })
            .ok_or_else(|| DeckError::new(ErrorKind::Missing, "MCP operation not pending"))?;
        operation.state = "executing".into();
        operation.updated_at = now_ms();
        Ok(PendingView {
            operation_id: operation.operation_id.clone(),
            kind: operation.kind.clone(),
            result: operation.result.clone().unwrap_or(Value::Null),
        })
    })
}

#[tauri::command]
pub(crate) fn mcp_validate(operation_id: String) -> Result<(), DeckError> {
    validate(runtime()?, &operation_id)
}

fn validate(runtime: &Runtime, operation_id: &str) -> Result<(), DeckError> {
    let _delivery = runtime.delivery.lock_or_recover();
    let target = runtime.read(|doc| {
        doc.operations
            .iter()
            .find(|operation| operation.operation_id == operation_id)
            .map(|operation| {
                (
                    operation.client_id.clone(),
                    operation
                        .result
                        .as_ref()
                        .and_then(|result| result.get("sessionId"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                )
            })
    })?;
    if let Some((client_id, session_id)) = &target {
        if let Some(error) = emergency_fence_error(runtime, client_id, session_id) {
            return Err(error);
        }
    }
    runtime.read(|doc| {
        if !doc.config.enabled {
            return Err(DeckError::new(ErrorKind::Perm, "MCP control is disabled"));
        }
        let operation = doc
            .operations
            .iter()
            .find(|operation| {
                operation.operation_id == operation_id
                    && matches!(operation.state.as_str(), "executing" | "admitted")
            })
            .ok_or_else(|| {
                DeckError::new(
                    ErrorKind::ContextChanged,
                    "MCP operation is no longer executable",
                )
            })?;
        client(doc, &operation.client_id)?;
        if operation.kind == "session-close" {
            let result = operation.result.as_ref().ok_or_else(|| {
                DeckError::new(ErrorKind::ContextChanged, "MCP close plan is missing")
            })?;
            let session_id = result
                .get("sessionId")
                .and_then(Value::as_str)
                .unwrap_or("");
            let generation = result
                .get("sessionGeneration")
                .and_then(Value::as_str)
                .unwrap_or("");
            let epoch = result
                .get("controlEpoch")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let holder = result.get("holderId").and_then(Value::as_str).unwrap_or("");
            let session = authorized_session(doc, &operation.client_id, session_id)?;
            check_control(session, &operation.client_id, generation, epoch, holder)?;
            if !session.closing {
                return Err(DeckError::new(
                    ErrorKind::ContextChanged,
                    "MCP close plan is no longer current",
                ));
            }
        }
        Ok(())
    })?
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CloseAdmissionView {
    admission: String,
}

/// Linearize a remote close inside the Board transaction, immediately before
/// its first durable queue-cancellation side effect. Once admitted, later
/// revocation does not pretend that the already-admitted close never started.
#[tauri::command]
pub(crate) fn mcp_close_admit(operation_id: String) -> Result<CloseAdmissionView, DeckError> {
    let runtime = runtime()?;
    close_admit(runtime, &operation_id)
}

fn close_admit(runtime: &Runtime, operation_id: &str) -> Result<CloseAdmissionView, DeckError> {
    let _delivery = runtime.delivery.lock_or_recover();
    pause_point(runtime, "close-admit");
    // Emergency fences first, under the delivery lock: a takeover, client
    // revocation or disable that fenced while this admission waited wins.
    let target = runtime.read(|doc| {
        doc.operations
            .iter()
            .find(|operation| operation.operation_id == operation_id)
            .map(|operation| {
                (
                    operation.client_id.clone(),
                    operation
                        .result
                        .as_ref()
                        .and_then(|result| result.get("sessionId"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                )
            })
    })?;
    if let Some((client_id, session_id)) = &target {
        if let Some(error) = emergency_fence_error(runtime, client_id, session_id) {
            return Err(error);
        }
    }
    let admission = random_id("close_")?;
    let admission_hash = sha(admission.as_bytes());
    runtime.write(|doc| {
        if !doc.config.enabled {
            return Err(DeckError::new(ErrorKind::Perm, "MCP control is disabled"));
        }
        let index = doc
            .operations
            .iter()
            .position(|operation| {
                operation.operation_id == operation_id
                    && operation.kind == "session-close"
                    && operation.state == "executing"
            })
            .ok_or_else(|| {
                DeckError::new(
                    ErrorKind::ContextChanged,
                    "MCP close operation is no longer admissible",
                )
            })?;
        let operation = doc.operations[index].clone();
        client(doc, &operation.client_id)?;
        let result = operation.result.as_ref().ok_or_else(|| {
            DeckError::new(ErrorKind::ContextChanged, "MCP close plan is missing")
        })?;
        let session_id = result
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or("");
        let generation = result
            .get("sessionGeneration")
            .and_then(Value::as_str)
            .unwrap_or("");
        let epoch = result
            .get("controlEpoch")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let holder = result.get("holderId").and_then(Value::as_str).unwrap_or("");
        let session = authorized_session(doc, &operation.client_id, session_id)?;
        check_control(session, &operation.client_id, generation, epoch, holder)?;
        if !session.closing {
            return Err(DeckError::new(
                ErrorKind::ContextChanged,
                "MCP close plan is no longer current",
            ));
        }
        let saved = &mut doc.operations[index];
        saved.state = "admitted".into();
        saved.admission_hash = Some(admission_hash);
        saved.updated_at = now_ms();
        Ok(())
    })?;
    Ok(CloseAdmissionView { admission })
}

/// Validate an already-linearized close at each native side-effect boundary.
/// This does not re-run revocation checks: revocation ordered after admission
/// cannot roll back queue cancellation that may already have committed.
pub(crate) fn validate_close_admission(
    admission: Option<&str>,
    tmux_sessions: &[String],
) -> Result<(), DeckError> {
    validate_close_admission_with(runtime()?, admission, tmux_sessions)
}

fn validate_close_admission_with(
    runtime: &Runtime,
    admission: Option<&str>,
    tmux_sessions: &[String],
) -> Result<(), DeckError> {
    let Some(admission) = admission else {
        return Ok(());
    };
    let hash = sha(admission.as_bytes());
    runtime.read(|doc| {
        let operation = doc
            .operations
            .iter()
            .find(|operation| {
                operation.kind == "session-close"
                    && operation.state == "admitted"
                    && operation.admission_hash.as_deref() == Some(&hash)
            })
            .ok_or_else(|| {
                DeckError::new(ErrorKind::ContextChanged, "MCP close admission is invalid")
            })?;
        let session_id = operation
            .result
            .as_ref()
            .and_then(|result| result.get("sessionId"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let session = doc
            .sessions
            .iter()
            .find(|session| session.session_id == session_id)
            .ok_or_else(|| DeckError::new(ErrorKind::ContextChanged, "MCP close target changed"))?;
        if tmux_sessions.len() != 1 || tmux_sessions[0] != session.tmux_session {
            return Err(DeckError::new(
                ErrorKind::ContextChanged,
                "MCP close admission target changed",
            ));
        }
        Ok(())
    })?
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StartResult {
    created: bool,
}

#[tauri::command]
pub(crate) fn mcp_start_session(
    operation_id: String,
    name: String,
    dir: String,
) -> Result<StartResult, DeckError> {
    let runtime = runtime()?;
    let _delivery = runtime.delivery.lock_or_recover();
    crate::tmux::validate_session_name(&name)?;
    let plan = runtime.read(|doc| {
        let operation = doc
            .operations
            .iter()
            .find(|operation| {
                operation.operation_id == operation_id
                    && operation.kind == "session-create"
                    && operation.state == "executing"
            })
            .ok_or_else(|| {
                DeckError::new(
                    ErrorKind::ContextChanged,
                    "MCP create operation is no longer executable",
                )
            })?;
        client(doc, &operation.client_id)?;
        Ok::<(Value, String), DeckError>((
            operation.result.clone().unwrap_or(Value::Null),
            operation.client_id.clone(),
        ))
    })??;
    let (plan, client_id) = plan;
    // A client revocation or disable that fenced while this start waited for
    // the delivery lock wins (checked outside the state lock).
    if let Some(error) = emergency_fence_error(runtime, &client_id, "") {
        return Err(error);
    }
    if plan.get("cwd").and_then(Value::as_str) != Some(&dir) {
        return Err(DeckError::new(
            ErrorKind::ContextChanged,
            "MCP create context changed",
        ));
    }
    let socket = plan
        .get("runnerSocket")
        .and_then(Value::as_str)
        .ok_or_else(|| DeckError::new(ErrorKind::Invalid, "MCP create plan is invalid"))?;
    let generation = plan
        .get("generation")
        .and_then(Value::as_str)
        .ok_or_else(|| DeckError::new(ErrorKind::Invalid, "MCP create plan is invalid"))?;
    let output_retention_ms = runtime.read(|doc| doc.config.output_retention_ms)?;
    if crate::tmux::tmux(&["has-session", "-t", &crate::tmux::session_target(&name)]).is_ok() {
        if runner_socket_matches(runtime, socket, generation) {
            return Ok(StartResult { created: true });
        }
        return Err(DeckError::new(
            ErrorKind::ContextChanged,
            "unmanaged matching session already exists",
        ));
    }
    let _activity = crate::session_runtime::activity_guard()?;
    let _creation = crate::tmux_lifecycle::session_creation_guard()?;
    let runner = runner_program()?;
    let args = vec![
        "new-session".into(),
        "-d".into(),
        "-s".into(),
        name.clone(),
        "-c".into(),
        dir,
        runner.display().to_string(),
        "--socket".into(),
        socket.into(),
        "--generation".into(),
        generation.into(),
        "--service-instance".into(),
        runtime.service_instance.clone(),
        "--deck-pid".into(),
        std::process::id().to_string(),
        "--output-retention-ms".into(),
        output_retention_ms.to_string(),
    ];
    crate::tmux::tmux_owned(&args)?;
    let ready = (0..40).any(|_| {
        if UnixStream::connect(socket).is_ok() {
            true
        } else {
            std::thread::sleep(Duration::from_millis(50));
            false
        }
    });
    if !ready {
        let _ = crate::tmux::tmux(&["kill-session", "-t", &crate::tmux::session_target(&name)]);
        return Err(DeckError::new(
            ErrorKind::Other,
            "MCP runner did not become ready",
        ));
    }
    Ok(StartResult { created: true })
}

#[tauri::command]
pub(crate) fn mcp_complete(
    operation_id: String,
    state: String,
    code: Option<String>,
    tmux_session: Option<String>,
) -> Result<(), DeckError> {
    complete(runtime()?, operation_id, state, code, tmux_session)
}

fn complete(
    runtime: &Runtime,
    operation_id: String,
    state: String,
    code: Option<String>,
    tmux_session: Option<String>,
) -> Result<(), DeckError> {
    if !matches!(state.as_str(), "committed" | "rejected" | "ambiguous") {
        return Err(DeckError::new(
            ErrorKind::Invalid,
            "invalid MCP operation result",
        ));
    }
    let _delivery = runtime.delivery.lock_or_recover();
    let switch_to_human = runtime.write(|doc| {
        let index = doc
            .operations
            .iter()
            .position(|operation| operation.operation_id == operation_id)
            .ok_or_else(|| DeckError::new(ErrorKind::Missing, "MCP operation not found"))?;
        let operation = doc.operations[index].clone();
        // Transitions: executing → committed|rejected|ambiguous; a close that
        // was admitted (its first Board side effect may have run) →
        // committed|ambiguous only. Repeating the recorded terminal state is
        // an idempotent no-op; anything else is refused.
        match (operation.state.as_str(), state.as_str()) {
            ("executing", _) => {}
            ("admitted", "committed" | "ambiguous") if operation.kind == "session-close" => {}
            (recorded, requested) if recorded == requested => return Ok(None),
            _ => {
                return Err(DeckError::new(
                    ErrorKind::ContextChanged,
                    "MCP operation cannot take that result",
                ))
            }
        }
        let authorized = doc.config.enabled
            && doc
                .config
                .clients
                .iter()
                .any(|client| client.id == operation.client_id && client.revoked_at.is_none());
        let mut created_session = None;
        if operation.kind == "session-create" && state == "committed" {
            let result = operation
                .result
                .as_ref()
                .ok_or_else(|| DeckError::new(ErrorKind::Invalid, "MCP create result missing"))?;
            let tmux_session = tmux_session
                .clone()
                .ok_or_else(|| DeckError::new(ErrorKind::Invalid, "MCP tmux session missing"))?;
            let get = |key: &str| {
                result
                    .get(key)
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| DeckError::new(ErrorKind::Invalid, "MCP create result invalid"))
            };
            let session = ManagedSession {
                session_id: get("sessionId")?,
                card_id: get("cardId")?,
                tmux_session,
                project_id: get("projectId")?,
                title: get("title")?,
                cwd: get("cwd")?,
                generation: get("generation")?,
                runner_socket: get("runnerSocket")?,
                owner_client_id: operation.client_id.clone(),
                control_owner: None,
                control_holder: None,
                control_epoch: 1,
                lease_expires_at: None,
                human_lock: !authorized,
                output_shared: false,
                closing: false,
                created_at: now_ms(),
                control_sequence: 0,
            };
            doc.sessions.push(session.clone());
            created_session = (!authorized).then_some(session);
        }
        if operation.kind == "session-close" && state == "committed" {
            if let Some(id) = operation
                .result
                .as_ref()
                .and_then(|value| value.get("sessionId"))
                .and_then(Value::as_str)
            {
                doc.sessions.retain(|session| session.session_id != id);
                doc.jobs.retain(|job| job.session_id != id);
            }
        } else if operation.kind == "session-close" {
            if let Some(id) = operation
                .result
                .as_ref()
                .and_then(|value| value.get("sessionId"))
                .and_then(Value::as_str)
            {
                if let Some(session) = doc
                    .sessions
                    .iter_mut()
                    .find(|session| session.session_id == id)
                {
                    session.closing = false;
                }
            }
        }
        let saved = &mut doc.operations[index];
        saved.state = state;
        saved.code = code;
        saved.updated_at = now_ms();
        compact(doc, Some(&runtime.service_instance));
        Ok(created_session)
    })?;
    if let Some(session) = switch_to_human {
        let _ = send_runner_control(runtime, &session, "human");
    }
    Ok(())
}

/// Reconcile an explicit local Board close that did not originate as an MCP
/// operation. The card is already durably absent when this is called.
#[tauri::command]
pub(crate) fn mcp_card_closed(card_id: String) -> Result<(), DeckError> {
    let runtime = runtime()?;
    let removed = runtime.write(|doc| {
        let removed = doc
            .sessions
            .iter()
            .filter(|session| session.card_id == card_id)
            .map(|session| session.session_id.clone())
            .collect::<HashSet<_>>();
        doc.sessions.retain(|session| session.card_id != card_id);
        doc.jobs.retain(|job| !removed.contains(&job.session_id));
        // Records and grants of a closed session can no longer act.
        compact(doc, Some(&runtime.service_instance));
        Ok(removed)
    })?;
    let mut emergency = runtime.emergency.lock_or_recover();
    for session_id in removed {
        emergency.human_sessions.remove(&session_id);
        emergency.execution_unpersisted.remove(&session_id);
    }
    Ok(())
}

fn resolve_session_id(runtime: &Runtime, session_id: &str) -> Result<String, DeckError> {
    runtime
        .read(|doc| {
            doc.sessions
                .iter()
                .find(|session| session.session_id == session_id || session.card_id == session_id)
                .map(|session| session.session_id.clone())
        })?
        .ok_or_else(|| DeckError::new(ErrorKind::Missing, "MCP session not found"))
}

/// Local takeover. The in-memory fence is set FIRST — before waiting for the
/// delivery lock that an in-flight exec may hold — so no new remote request
/// passes route or dispatch after this point. Then the takeover is persisted
/// (new epoch, holder/lease cleared, output sharing off, and every existing
/// job binding's output closed to MCP for good) and the runner hands the pane
/// keyboard and the ^C stop key to the human. Errors are stable codes; the
/// fence holds whatever happens afterwards.
#[tauri::command(async)]
pub(crate) fn mcp_takeover(session_id: String) -> Result<(), DeckError> {
    takeover(runtime()?, &session_id)
}

fn takeover(runtime: &Runtime, session_id: &str) -> Result<(), DeckError> {
    let fenced_session_id = resolve_session_id(runtime, session_id)?;
    runtime
        .emergency
        .lock_or_recover()
        .human_sessions
        .insert(fenced_session_id.clone());
    let _delivery = runtime.delivery.lock_or_recover();
    let persisted = runtime.write(|doc| {
        let session = doc
            .sessions
            .iter_mut()
            .find(|session| session.session_id == fenced_session_id)
            .ok_or_else(|| DeckError::new(ErrorKind::Missing, "MCP session not found"))?;
        session.control_owner = None;
        session.control_holder = None;
        session.control_epoch = session.control_epoch.saturating_add(1);
        session.lease_expires_at = None;
        session.human_lock = true;
        session.output_shared = false;
        let session = session.clone();
        // Output produced from here on may include the human's own typing
        // into the job: no pre-takeover binding may ever read it.
        for job in doc
            .jobs
            .iter_mut()
            .filter(|job| job.session_id == session.session_id)
        {
            job.allow_output = false;
        }
        audit(
            doc,
            "human-takeover",
            AuditLink {
                principal_id: Some(&session.owner_client_id),
                session_id: Some(&session.session_id),
                ..Default::default()
            },
        )?;
        Ok(session)
    });
    let session = match &persisted {
        Ok(session) => session.clone(),
        Err(_) => {
            // Still hand the keyboard over under a higher epoch; the
            // in-memory fence keeps MCP out for this Deck run.
            let mut session = runtime
                .read(|doc| {
                    doc.sessions
                        .iter()
                        .find(|session| session.session_id == fenced_session_id)
                        .cloned()
                })?
                .ok_or_else(|| DeckError::new(ErrorKind::Missing, "MCP session not found"))?;
            session.control_epoch = session.control_epoch.saturating_add(1);
            session.control_holder = None;
            session
        }
    };
    runner_control_result(send_runner_control(runtime, &session, "human"))?;
    if persisted.is_err() {
        return Err(DeckError::new(ErrorKind::Other, FENCE_UNPERSISTED));
    }
    Ok(())
}

/// Local return of a human-locked session to MCP. It needs no execution
/// grant, creates or extends none, restores no holder, lease or output
/// sharing, and never revives an old epoch: the persisted state moves to a
/// NEW epoch with no holder, then the runner is told. A runner failure
/// re-fences (human lock back on, another new epoch). Machine codes:
/// SESSION_BUSY (a job still runs; stop it locally first), RUNNER_STALE
/// (Deck restarted since the session was created; close it),
/// CLIENT_REVOKED / FEATURE_DISABLED (nothing to return to).
#[tauri::command(async)]
pub(crate) fn mcp_return_control(session_id: String) -> Result<(), DeckError> {
    return_control(runtime()?, &session_id)
}

fn return_control(runtime: &Runtime, session_id: &str) -> Result<(), DeckError> {
    let _delivery = runtime.delivery.lock_or_recover();
    let session = runtime.read(|doc| {
        let session = doc
            .sessions
            .iter()
            .find(|session| session.session_id == session_id || session.card_id == session_id)
            .cloned()
            .ok_or_else(|| DeckError::new(ErrorKind::Missing, "MCP session not found"))?;
        if !doc.config.enabled {
            return Err(DeckError::new(ErrorKind::Perm, FEATURE_DISABLED));
        }
        if client(doc, &session.owner_client_id).is_err() {
            return Err(DeckError::new(ErrorKind::Perm, CLIENT_REVOKED));
        }
        Ok(session)
    })??;
    {
        let emergency = runtime.emergency.lock_or_recover();
        if emergency.disabled {
            return Err(DeckError::new(ErrorKind::Perm, FEATURE_DISABLED));
        }
        if emergency.clients.contains(&session.owner_client_id) {
            return Err(DeckError::new(ErrorKind::Perm, CLIENT_REVOKED));
        }
    }
    match probe_runner(runtime, &session) {
        None => {
            return Err(DeckError::new(
                ErrorKind::ContextChanged,
                RUNNER_UNCONFIRMED,
            ))
        }
        Some(probe) if !probe.current => {
            return Err(DeckError::new(ErrorKind::ContextChanged, RUNNER_STALE))
        }
        Some(probe) if !probe.job.is_null() => {
            return Err(DeckError::new(ErrorKind::Locked, SESSION_BUSY))
        }
        Some(_) => {}
    }
    let returned = runtime.write(|doc| {
        let current = doc
            .sessions
            .iter_mut()
            .find(|item| {
                item.session_id == session.session_id && item.generation == session.generation
            })
            .ok_or_else(|| {
                DeckError::new(ErrorKind::ContextChanged, "MCP session generation changed")
            })?;
        current.human_lock = false;
        current.control_epoch = current.control_epoch.saturating_add(1);
        current.control_owner = None;
        current.control_holder = None;
        current.lease_expires_at = None;
        let current = current.clone();
        audit(
            doc,
            "human-return",
            AuditLink {
                principal_id: Some(&current.owner_client_id),
                session_id: Some(&current.session_id),
                ..Default::default()
            },
        )?;
        Ok(current)
    })?;
    if let Err(error) = runner_control_result(send_runner_control(runtime, &returned, "mcp")) {
        // Re-fence: the persisted state must not claim MCP control that the
        // runner never confirmed. The in-memory fence is still in place.
        let refenced = runtime.write(|doc| {
            Ok(doc
                .sessions
                .iter_mut()
                .find(|item| item.session_id == returned.session_id)
                .map(|current| {
                    current.human_lock = true;
                    current.control_epoch = current.control_epoch.saturating_add(1);
                    current.control_owner = None;
                    current.control_holder = None;
                    current.lease_expires_at = None;
                    current.clone()
                }))
        });
        if let Ok(Some(current)) = refenced {
            let _ = send_runner_control(runtime, &current, "human");
        }
        return Err(error);
    }
    runtime
        .emergency
        .lock_or_recover()
        .human_sessions
        .remove(&session.session_id);
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionUiView {
    managed: bool,
    /// The runner is unreachable or belongs to an earlier Deck process: it can
    /// only be closed.
    stale: bool,
    human_control: bool,
    control_owner: Option<String>,
    control_holder: Option<String>,
    control_epoch: u64,
    active_job: bool,
    client_name: Option<String>,
    job_state: Option<String>,
    recent_error: Option<String>,
    execution_grant_active: bool,
    execution_expires_at: Option<u64>,
    stdin_allowed: bool,
    output_shared: bool,
}

#[tauri::command(async)]
pub(crate) fn mcp_session_ui(card_id: String) -> Result<SessionUiView, DeckError> {
    let runtime = runtime()?;
    record_expired_grants(runtime)?;
    let session = runtime.read(|doc| {
        doc.sessions
            .iter()
            .find(|session| session.card_id == card_id)
            .cloned()
    })?;
    let Some(session) = session else {
        return Ok(SessionUiView {
            managed: false,
            stale: false,
            human_control: false,
            control_owner: None,
            control_holder: None,
            control_epoch: 0,
            active_job: false,
            client_name: None,
            job_state: None,
            recent_error: None,
            execution_grant_active: false,
            execution_expires_at: None,
            stdin_allowed: false,
            output_shared: false,
        });
    };
    let (client_name, recent_error, grant) = runtime.read(|doc| {
        let name = doc
            .config
            .clients
            .iter()
            .find(|client| client.id == session.owner_client_id)
            .map(|client| client.name.clone());
        let error = doc
            .operations
            .iter()
            .rev()
            .find(|operation| {
                operation.client_id == session.owner_client_id
                    && operation.code.is_some()
                    && operation
                        .result
                        .as_ref()
                        .and_then(|result| result.get("sessionId"))
                        .and_then(Value::as_str)
                        == Some(&session.session_id)
            })
            .and_then(|operation| operation.code.clone());
        let grant = doc
            .execution_grants
            .iter()
            .rev()
            .find(|grant| {
                grant.session_id == session.session_id
                    && grant.service_instance == runtime.service_instance
                    && grant.revoked_at.is_none()
                    && now_ms() < grant.expires_at
                    && runtime
                        .monotonic_ms()
                        .saturating_sub(grant.issued_monotonic_ms)
                        < grant.duration_ms
            })
            .cloned();
        (name, error, grant)
    })?;
    let runner = probe_runner(runtime, &session);
    let emergency_human = runtime
        .emergency
        .lock_or_recover()
        .human_sessions
        .contains(&session.session_id);
    let job_state = runner
        .as_ref()
        .and_then(|probe| probe.job.get("state"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok(SessionUiView {
        managed: true,
        stale: runner.as_ref().is_none_or(|probe| !probe.current),
        human_control: session.human_lock || emergency_human,
        control_owner: session.control_owner,
        control_holder: session.control_holder,
        control_epoch: session.control_epoch,
        active_job: job_state.is_some(),
        client_name,
        job_state,
        recent_error,
        execution_grant_active: grant.is_some(),
        execution_expires_at: grant.as_ref().map(|grant| grant.expires_at),
        stdin_allowed: grant.as_ref().is_some_and(|grant| grant.allow_stdin),
        output_shared: session.output_shared && !emergency_human,
    })
}

/// All ordinary Deck terminal-input paths call this before writing. A managed
/// runner also discards pane stdin while MCP owns control, closing the check /
/// write race for keyboard input already queued at takeover.
pub(crate) fn guard_terminal_input(tmux_session: &str) -> Result<(), DeckError> {
    let Some(runtime) = RUNTIME.get() else {
        return Ok(());
    };
    let session = runtime.read(|doc| {
        doc.sessions
            .iter()
            .find(|session| session.tmux_session == tmux_session)
            .map(|session| (session.session_id.clone(), session.human_lock))
    })?;
    match session {
        None | Some((_, true)) => Ok(()),
        // A takeover whose state write failed still hands the keyboard over
        // for this Deck run: the in-memory fence is authoritative.
        Some((session_id, false))
            if runtime
                .emergency
                .lock_or_recover()
                .human_sessions
                .contains(&session_id) =>
        {
            Ok(())
        }
        Some(_) => Err(DeckError::new(ErrorKind::Perm, "MCP owns terminal control")),
    }
}

/// Close path hook (`commands::kill_session`): before tmux kills a managed
/// pane, ask its runner to stop every job group with bounded escalation
/// (SIGINT → SIGTERM → SIGKILL). Best effort by design — after a Deck restart
/// the in-memory runner key is unavailable, and the runner's SIGHUP handler
/// still SIGKILLs live job groups when the pane dies.
pub(crate) fn stop_managed_jobs(tmux_session: &str) {
    let Some(runtime) = RUNTIME.get() else {
        return;
    };
    let Ok(Some(session)) = runtime.read(|doc| {
        doc.sessions
            .iter()
            .find(|session| session.tmux_session == tmux_session)
            .cloned()
    }) else {
        return;
    };
    let _ = send_runner(
        runtime,
        &session,
        &json!({"kind":"stop","generation":session.generation}),
    );
}

/// Managed runner panes cannot be reconstructed by the ordinary shell
/// restart transaction. The user must explicitly close them first.
pub(crate) fn guard_server_restart() -> Result<(), DeckError> {
    let Some(runtime) = RUNTIME.get() else {
        return Ok(());
    };
    runtime.read(|doc| {
        if doc.sessions.is_empty() {
            Ok(())
        } else {
            Err(DeckError::new(
                ErrorKind::Locked,
                "tmux-restart-mcp-managed-sessions",
            ))
        }
    })?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// In-process runner double. Every connection is served on its own
    /// thread so a held request never blocks unrelated ones.
    struct FakeRunner {
        socket: PathBuf,
        stop: Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
        /// Every request received, in order.
        seen: Arc<Mutex<Vec<Value>>>,
        /// A managed job is live: ping reports it and control mcp is busy.
        busy: Arc<AtomicBool>,
        /// Requests of this kind park until `release` is sent.
        hold: Arc<Mutex<Option<HeldKind>>>,
        /// Runner control state uses the production monotonic epoch contract.
        control: Arc<Mutex<FakeControl>>,
        /// Reject one control request without advancing the runner epoch.
        fail_next_control: Arc<AtomicBool>,
    }

    struct FakeControl {
        epoch: u64,
        mode: String,
        holder: Option<String>,
    }

    struct HeldKind {
        kind: String,
        entered: std::sync::mpsc::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
    }

    impl FakeRunner {
        fn start(root: &Path, generation: &str) -> Self {
            let socket = root.join("runner.sock");
            let listener = UnixListener::bind(&socket).unwrap();
            listener.set_nonblocking(true).unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let seen = Arc::new(Mutex::new(Vec::new()));
            let busy = Arc::new(AtomicBool::new(false));
            let hold: Arc<Mutex<Option<HeldKind>>> = Arc::new(Mutex::new(None));
            let control = Arc::new(Mutex::new(FakeControl {
                epoch: 0,
                mode: "fenced".into(),
                holder: None,
            }));
            let fail_next_control = Arc::new(AtomicBool::new(false));
            let thread_stop = stop.clone();
            let generation = generation.to_owned();
            let (thread_seen, thread_busy, thread_hold, thread_control, thread_fail_control) = (
                seen.clone(),
                busy.clone(),
                hold.clone(),
                control.clone(),
                fail_next_control.clone(),
            );
            let thread = std::thread::spawn(move || {
                while !thread_stop.load(Ordering::SeqCst) {
                    let Ok((stream, _)) = listener.accept() else {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    };
                    let (generation, seen, busy, hold, control, fail_control) = (
                        generation.clone(),
                        thread_seen.clone(),
                        thread_busy.clone(),
                        thread_hold.clone(),
                        thread_control.clone(),
                        thread_fail_control.clone(),
                    );
                    std::thread::spawn(move || {
                        Self::serve(
                            stream,
                            &generation,
                            &seen,
                            &busy,
                            &hold,
                            &control,
                            &fail_control,
                        )
                    });
                }
            });
            Self {
                socket,
                stop,
                thread: Some(thread),
                seen,
                busy,
                hold,
                control,
                fail_next_control,
            }
        }

        fn serve(
            mut stream: UnixStream,
            generation: &str,
            seen: &Mutex<Vec<Value>>,
            busy: &AtomicBool,
            hold: &Mutex<Option<HeldKind>>,
            control: &Mutex<FakeControl>,
            fail_next_control: &AtomicBool,
        ) {
            stream.set_nonblocking(false).unwrap();
            let mut line = String::new();
            if BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut line)
                .is_err()
            {
                return;
            }
            let request: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
            seen.lock().unwrap().push(request.clone());
            let kind = request.get("kind").and_then(Value::as_str).unwrap_or("");
            let held = {
                let mut slot = hold.lock().unwrap();
                if slot.as_ref().is_some_and(|held| held.kind == kind) {
                    slot.take()
                } else {
                    None
                }
            };
            if let Some(held) = held {
                held.entered.send(()).unwrap();
                held.release
                    .recv_timeout(Duration::from_secs(10))
                    .expect("held runner request was never released");
            }
            let job_id = request
                .get("job_id")
                .and_then(Value::as_str)
                .unwrap_or("job_a");
            let service = request
                .get("service_instance")
                .or_else(|| {
                    request
                        .get("context")
                        .and_then(|context| context.get("service_instance"))
                })
                .and_then(Value::as_str)
                .unwrap_or("");
            let stale = service == "svc_stale";
            let live = busy.load(Ordering::SeqCst);
            let dispatch_matches = || {
                let current = control.lock().unwrap();
                let context = request.get("context").unwrap_or(&Value::Null);
                current.mode == "mcp"
                    && context.get("control_epoch").and_then(Value::as_u64) == Some(current.epoch)
                    && context.get("holder_id").and_then(Value::as_str) == current.holder.as_deref()
            };
            let response = match kind {
                "claim" => json!({"ok":true,"generation":generation,
                    "auth":base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([7u8;32])}),
                "ping" => json!({"ok":true,"generation":generation,
                    "job": if live { json!({"jobId":"job_live","state":"running"}) } else { Value::Null },
                    "serviceCurrent":!service.is_empty() && !stale,"runnerVersion":"0.1.0"}),
                _ if stale => json!({"ok":false,"generation":generation,"error":"runner-stale"}),
                "control" if fail_next_control.swap(false, Ordering::SeqCst) => {
                    json!({"ok":false,"generation":generation,"error":"dispatch-context-invalid"})
                }
                "control" if live && request["mode"] == "mcp" => {
                    json!({"ok":false,"generation":generation,"error":"session-busy"})
                }
                "control" => {
                    let epoch = request["control_epoch"].as_u64().unwrap_or(0);
                    let mut current = control.lock().unwrap();
                    if epoch <= current.epoch {
                        json!({"ok":false,"generation":generation,"error":"dispatch-context-invalid"})
                    } else {
                        current.epoch = epoch;
                        current.mode = request["mode"].as_str().unwrap_or("fenced").into();
                        current.holder = request["holder_id"].as_str().map(str::to_owned);
                        json!({"ok":true,"generation":generation})
                    }
                }
                "read" => json!({
                    "ok":true,
                    "generation":generation,
                    "job":{"jobId":job_id,"state":"exited","exitCode":0,
                        "terminationSignal":null,"interruptRequested":false,
                        "timeoutRequested":false,"startedAt":1,"endedAt":2},
                    "output":"done\n","nextCursor":5,"gap":false,"droppedBytes":0
                }),
                "exec" if dispatch_matches() => json!({
                    "ok":true,
                    "generation":generation,
                    "job":{"jobId":job_id,"state":"exited","exitCode":0}
                }),
                "exec" | "input" | "interrupt" if !dispatch_matches() => {
                    json!({"ok":false,"generation":generation,"error":"control-revoked"})
                }
                "input" | "interrupt" | "retention" | "revoke-grant" | "authorize-grant"
                | "stop" => {
                    json!({"ok":true,"generation":generation})
                }
                _ => json!({"ok":false,"generation":generation,"error":"invalid-request"}),
            };
            if serde_json::to_writer(&mut stream, &response).is_ok() {
                let _ = stream.write_all(b"\n");
            }
        }

        fn count(&self, kind: &str) -> usize {
            self.seen
                .lock()
                .unwrap()
                .iter()
                .filter(|request| request["kind"] == kind)
                .count()
        }

        fn last(&self, kind: &str) -> Option<Value> {
            self.seen
                .lock()
                .unwrap()
                .iter()
                .rev()
                .find(|request| request["kind"] == kind)
                .cloned()
        }

        fn set_control(&self, epoch: u64, mode: &str, holder: Option<&str>) {
            *self.control.lock().unwrap() = FakeControl {
                epoch,
                mode: mode.into(),
                holder: holder.map(str::to_owned),
            };
        }

        fn control_epoch(&self) -> u64 {
            self.control.lock().unwrap().epoch
        }

        fn fail_next_control(&self) {
            self.fail_next_control.store(true, Ordering::SeqCst);
        }

        /// Park the next request of `kind`; returns (entered, release).
        fn hold(&self, kind: &str) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
            let (entered_tx, entered_rx) = std::sync::mpsc::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            *self.hold.lock().unwrap() = Some(HeldKind {
                kind: kind.into(),
                entered: entered_tx,
                release: release_rx,
            });
            (entered_rx, release_tx)
        }
    }

    impl Drop for FakeRunner {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            let _ = UnixStream::connect(&self.socket);
            if let Some(thread) = self.thread.take() {
                thread.join().unwrap();
            }
            let _ = std::fs::remove_file(&self.socket);
        }
    }

    fn test_root(tag: &str) -> PathBuf {
        // A per-process counter instead of a timestamp keeps runner socket
        // paths below SUN_LEN under a long macOS TMPDIR.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "deck-mcp-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn control_socket_is_private_before_its_public_name_exists() {
        let root = test_root("private-control-socket");
        let socket = root.join("mcp-control.sock");
        let listener = bind_private_socket(&socket).unwrap();
        assert_eq!(
            std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::read_dir(&root)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name() == ".mcp-bind")
                .count(),
            0
        );
        drop(listener);
        std::fs::remove_file(socket).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    fn client_record(root: &Path) -> Client {
        Client {
            id: "client_a".into(),
            name: "Client A".into(),
            credential_hash: sha(b"mcp_test"),
            credential_version: 1,
            revoked_at: None,
            allow_create: true,
            projects: vec![ProjectScope {
                project_id: "P1".into(),
                roots: vec![root.display().to_string()],
            }],
            create_sequence: 0,
        }
    }

    fn session_record(root: &Path, runner: &FakeRunner) -> ManagedSession {
        ManagedSession {
            session_id: "mcp_a".into(),
            card_id: "M1".into(),
            tmux_session: "deck-mcp-test".into(),
            project_id: "P1".into(),
            title: "MCP shell".into(),
            cwd: root.display().to_string(),
            generation: "g_a".into(),
            runner_socket: runner.socket.display().to_string(),
            owner_client_id: "client_a".into(),
            control_owner: Some("client_a".into()),
            control_holder: Some("holder_a".into()),
            control_epoch: 1,
            lease_expires_at: Some(now_ms() + 60_000),
            human_lock: false,
            output_shared: true,
            closing: false,
            created_at: now_ms(),
            control_sequence: 0,
        }
    }

    fn execution_grant(session: &ManagedSession) -> ExecutionGrant {
        ExecutionGrant {
            grant_id: "grant_test".into(),
            client_id: session.owner_client_id.clone(),
            credential_version: 1,
            project_id: session.project_id.clone(),
            session_id: session.session_id.clone(),
            session_generation: session.generation.clone(),
            profile: "trusted-host-v1".into(),
            environment_profile: ENVIRONMENT_PROFILE.into(),
            environment_profile_version: 1,
            issued_at: now_ms(),
            expires_at: now_ms() + 60_000,
            issued_monotonic_ms: 0,
            duration_ms: 60_000,
            allow_stdin: true,
            allow_output: true,
            _legacy_allow_shell: None,
            grant_version: 1,
            revocation_version: 0,
            service_instance: "svc_test".into(),
            revoked_at: None,
        }
    }

    #[test]
    fn inspect_separates_execution_authorization_from_output_sharing() {
        let root = test_root("inspect");
        let runner = FakeRunner::start(&root, "g_a");
        let session = session_record(&root, &runner);
        let mut doc = DiskDoc::default();
        doc.config.enabled = true;
        doc.config.clients.push(client_record(&root));
        doc.sessions.push(session.clone());
        let runtime = Runtime {
            app: None,
            path: root.join("mcp.json"),
            socket: root.join("control.sock"),
            doc: Mutex::new(Ok(doc)),
            io: Mutex::new(()),
            delivery: Mutex::new(()),
            emergency: Mutex::new(EmergencyFences::default()),
            service_instance: "svc_test".into(),
            runner_auth: Mutex::new(HashMap::new()),
            started: Instant::now(),
        };
        let view = || inspect(&runtime, "client_a", json!({"session_id":"mcp_a"})).unwrap();

        let none = view();
        assert_eq!(none["executionAuthorization"]["status"], "none");
        assert_eq!(none["outputSharing"]["sessionGateOpen"], true);

        runtime
            .write(|doc| {
                doc.execution_grants.push(execution_grant(&session));
                Ok(())
            })
            .unwrap();
        let active = view();
        assert_eq!(active["executionAuthorization"]["status"], "active");
        assert_eq!(
            active["executionAuthorization"]["stdinApprovedForActiveGrant"],
            true
        );
        assert!(active["executionAuthorization"]
            .get("shellApprovedForActiveGrant")
            .is_none());
        runtime
            .write(|doc| {
                doc.sessions[0].control_owner = None;
                doc.sessions[0].lease_expires_at = None;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            view()["executionAuthorization"]["status"],
            "active",
            "control lease is independent"
        );
        runtime
            .write(|doc| {
                doc.sessions[0].human_lock = true;
                Ok(())
            })
            .unwrap();
        let human = view();
        assert_eq!(human["executionAuthorization"]["status"], "active");
        assert_eq!(human["outputSharing"]["sessionGateOpen"], false);
        assert!(
            human.get("controlEpoch").is_some(),
            "control metadata remains visible during takeover"
        );
        runtime
            .write(|doc| {
                doc.sessions[0].human_lock = false;
                Ok(())
            })
            .unwrap();

        runtime
            .write(|doc| {
                doc.execution_grants[0].expires_at = doc.execution_grants[0].issued_at;
                Ok(())
            })
            .unwrap();
        let expired = view();
        assert_eq!(expired["executionAuthorization"]["status"], "expired");
        assert_eq!(
            expired["outputSharing"]["sessionGateOpen"], true,
            "retained output sharing is independent"
        );

        runtime
            .write(|doc| {
                doc.execution_grants[0].revoked_at = Some(now_ms());
                doc.execution_grants[0].revocation_version = 1;
                Ok(())
            })
            .unwrap();
        assert_eq!(view()["executionAuthorization"]["status"], "revoked");

        runtime
            .write(|doc| {
                doc.sessions[0].output_shared = false;
                Ok(())
            })
            .unwrap();
        let paused = view();
        assert_eq!(paused["executionAuthorization"]["status"], "revoked");
        assert_eq!(paused["outputSharing"]["sessionGateOpen"], false);
        assert!(
            paused.get("controlEpoch").is_some(),
            "control metadata remains visible"
        );

        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn control_socket_exists_only_while_mcp_is_enabled() {
        let root = test_root("conditional-control-socket");
        let runtime = Runtime {
            app: None,
            path: root.join("mcp.json"),
            socket: root.join("mcp-control.sock"),
            doc: Mutex::new(Ok(DiskDoc::default())),
            io: Mutex::new(()),
            delivery: Mutex::new(()),
            emergency: Mutex::new(EmergencyFences::default()),
            service_instance: "svc_socket".into(),
            runner_auth: Mutex::new(HashMap::new()),
            started: Instant::now(),
        };
        let mut listener = None;
        reconcile_control_socket(&runtime, &mut listener).unwrap();
        assert!(listener.is_none());
        assert!(!runtime.socket.exists());

        runtime
            .doc
            .lock_or_recover()
            .as_mut()
            .unwrap()
            .config
            .enabled = true;
        reconcile_control_socket(&runtime, &mut listener).unwrap();
        assert!(listener.is_some());
        assert_eq!(
            std::fs::metadata(&runtime.socket)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        runtime
            .doc
            .lock_or_recover()
            .as_mut()
            .unwrap()
            .config
            .enabled = false;
        reconcile_control_socket(&runtime, &mut listener).unwrap();
        assert!(listener.is_none());
        assert!(!runtime.socket.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scope_preview_classifies_paths_and_final_scope_check_rejects_stale_project() {
        let root = test_root("scope-preview");
        let missing = mcp_scope_preview(root.join("missing").display().to_string());
        assert!(!missing.ok);
        assert_eq!(missing.error, Some("not_found"));
        let file = root.join("file.txt");
        std::fs::write(&file, b"file").unwrap();
        let not_directory = mcp_scope_preview(file.display().to_string());
        assert!(!not_directory.ok);
        assert_eq!(not_directory.error, Some("not_directory"));
        let valid = mcp_scope_preview(root.display().to_string());
        assert!(valid.ok);
        let whole_disk = mcp_scope_preview("/".into());
        assert!(!whole_disk.ok);
        assert_eq!(whole_disk.error, Some("too_broad"));
        let home = crate::mcp_fs::home_directory().unwrap();
        assert_eq!(
            mcp_scope_preview(home.display().to_string()).error,
            Some("too_broad")
        );
        let inaccessible = canonical_project_root_with_access(root.to_str().unwrap(), |_| {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "synthetic access denial",
            ))
        })
        .unwrap_err();
        assert_eq!(inaccessible.kind(), ErrorKind::Perm);

        let stale = match validate_new_client_scopes(
            vec![ProjectScopeInput {
                project_id: "P1".into(),
                roots: vec![root.display().to_string()],
            }],
            true,
            |_| Ok(false),
        ) {
            Err(error) => error,
            Ok(_) => panic!("stale project was accepted"),
        };
        assert_eq!(stale.kind(), ErrorKind::Missing);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn request(tool: &str, arguments: Value) -> WireRequest {
        WireRequest {
            version: CONTROL_PROTOCOL,
            client_id: "client_a".into(),
            credential: "mcp_test".into(),
            tool: tool.into(),
            arguments,
        }
    }

    fn operation(kind: &str, state: &str) -> Operation {
        Operation {
            operation_id: "op_a".into(),
            client_id: "client_a".into(),
            request_id: "request_a".into(),
            request_hash: "a".repeat(64),
            kind: kind.into(),
            state: state.into(),
            code: None,
            result: None,
            accepted_at: 1,
            updated_at: 1,
            admission_hash: None,
            session_id: None,
            control_epoch: None,
            control_sequence: None,
        }
    }

    #[test]
    fn fresh_state_is_disabled_and_strictly_bounded() {
        let doc = DiskDoc::default();
        assert!(!doc.config.enabled);
        validate_doc(&doc).unwrap();
        assert_eq!(MAX_RESPONSE_BYTES, 128 * 1024);
    }

    #[test]
    fn cursor_is_bound_to_job_and_generation() {
        let binding = JobBinding {
            job_id: "job_a".into(),
            client_id: "client_a".into(),
            session_id: "mcp_a".into(),
            session_generation: "g_a".into(),
            request_hash: "a".repeat(64),
            operation_id: "op_a".into(),
            grant_id: "grant_a".into(),
            grant_version: 1,
            allow_output: true,
        };
        assert_eq!(
            decode_cursor(Some("g_a:job_a:42".into()), &binding).unwrap(),
            42
        );
        assert!(decode_cursor(Some("g_b:job_a:42".into()), &binding).is_err());
        assert!(decode_cursor(Some("g_a:job_b:42".into()), &binding).is_err());
    }

    #[test]
    fn request_id_conflict_is_distinct_and_never_reused() {
        let mut doc = DiskDoc::default();
        doc.operations.push(operation("exec", "committed"));
        assert!(
            existing_operation(&doc, "client_a", "request_a", &"a".repeat(64))
                .unwrap()
                .is_some()
        );
        let error = match existing_operation(&doc, "client_a", "request_a", &"b".repeat(64)) {
            Err(error) => error,
            Ok(_) => panic!("conflicting request id was accepted"),
        };
        assert_eq!(error.kind(), ErrorKind::RequestConflict);
    }

    #[test]
    fn revoked_control_has_its_own_protocol_error() {
        let session = ManagedSession {
            session_id: "mcp_a".into(),
            card_id: "M1".into(),
            tmux_session: "deck-mcp-a".into(),
            project_id: "P1".into(),
            title: "MCP shell".into(),
            cwd: "/tmp".into(),
            generation: "g_a".into(),
            runner_socket: "/tmp/runner.sock".into(),
            owner_client_id: "client_a".into(),
            control_owner: None,
            control_holder: None,
            control_epoch: 2,
            lease_expires_at: None,
            human_lock: true,
            output_shared: false,
            closing: false,
            created_at: 1,
            control_sequence: 0,
        };
        let error = check_control(&session, "client_a", "g_a", 1, "holder_a").unwrap_err();
        assert_eq!(error.kind(), ErrorKind::ControlRevoked);
        assert_eq!(map_error(error)["error"]["code"], "CONTROL_REVOKED");
    }

    #[test]
    fn authorization_is_client_project_session_and_control_scoped() {
        let root = std::fs::canonicalize("/tmp").unwrap().display().to_string();
        let mut doc = DiskDoc::default();
        doc.config.clients.push(Client {
            id: "client_a".into(),
            name: "Client A".into(),
            credential_hash: sha(b"mcp_test"),
            credential_version: 1,
            revoked_at: None,
            allow_create: true,
            projects: vec![ProjectScope {
                project_id: "P1".into(),
                roots: vec![root.clone()],
            }],
            create_sequence: 0,
        });
        let disabled = match client(&doc, "client_a") {
            Err(error) => error,
            Ok(_) => panic!("disabled MCP state authorized a client"),
        };
        assert_eq!(disabled.kind(), ErrorKind::Perm);
        doc.config.enabled = true;
        assert!(client(&doc, "client_missing").is_err());
        assert!(scoped_project(client(&doc, "client_a").unwrap(), "P2").is_err());
        assert_eq!(
            canonical_scope("/tmp", &[root]).unwrap(),
            std::fs::canonicalize("/tmp").unwrap()
        );

        doc.sessions.push(ManagedSession {
            session_id: "mcp_a".into(),
            card_id: "M1".into(),
            tmux_session: "deck-mcp-a".into(),
            project_id: "P1".into(),
            title: "MCP shell".into(),
            cwd: "/tmp".into(),
            generation: "g_a".into(),
            runner_socket: "/tmp/runner.sock".into(),
            owner_client_id: "client_a".into(),
            control_owner: Some("client_a".into()),
            control_holder: Some("holder_a".into()),
            control_epoch: 2,
            lease_expires_at: Some(now_ms() + 10_000),
            human_lock: false,
            output_shared: true,
            closing: false,
            created_at: 1,
            control_sequence: 0,
        });
        assert!(authorized_session(&doc, "client_missing", "mcp_a").is_err());
        let session = authorized_session(&doc, "client_a", "mcp_a").unwrap();
        assert!(check_control(session, "client_a", "g_other", 2, "holder_a").is_err());
        assert!(check_control(session, "client_a", "g_a", 1, "holder_a").is_err());
        assert!(check_control(session, "client_a", "g_a", 2, "other_holder").is_err());
        assert!(check_control(session, "client_a", "g_a", 2, "holder_a").is_ok());
    }

    #[test]
    fn restart_never_replays_an_accepted_exec() {
        let root = std::env::temp_dir().join(format!("deck-mcp-load-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).unwrap();
        let path = root.join("mcp.json");
        let mut doc = DiskDoc::default();
        doc.operations.push(operation("exec", "accepted"));
        save(&path, &doc).unwrap();
        let recovered = load(&path).unwrap();
        assert_eq!(recovered.operations[0].state, "ambiguous");
        assert_eq!(
            recovered.operations[0].code.as_deref(),
            Some("deck-restarted")
        );
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn grant_expiry_is_audited_once_without_reviving_authority() {
        let root = test_root("grant-expiry-audit");
        let path = root.join("mcp.json");
        let mut doc = DiskDoc::default();
        doc.config.enabled = true;
        doc.config.clients.push(client_record(&root));
        let session = ManagedSession {
            session_id: "mcp_a".into(),
            card_id: "M1".into(),
            tmux_session: "deck-mcp-test".into(),
            project_id: "P1".into(),
            title: "MCP shell".into(),
            cwd: root.display().to_string(),
            generation: "g_a".into(),
            runner_socket: root.join("missing.sock").display().to_string(),
            owner_client_id: "client_a".into(),
            control_owner: Some("client_a".into()),
            control_holder: Some("holder_a".into()),
            control_epoch: 1,
            lease_expires_at: Some(now_ms() + 60_000),
            human_lock: false,
            output_shared: true,
            closing: false,
            created_at: now_ms(),
            control_sequence: 0,
        };
        let mut grant = execution_grant(&session);
        grant.issued_at = now_ms().saturating_sub(1_000);
        grant.expires_at = now_ms().saturating_sub(1);
        grant.duration_ms = 100;
        doc.execution_grants.push(grant);
        doc.sessions.push(session.clone());
        save(&path, &doc).unwrap();
        let runtime = Runtime {
            app: None,
            path,
            socket: root.join("control.sock"),
            doc: Mutex::new(Ok(doc)),
            io: Mutex::new(()),
            delivery: Mutex::new(()),
            emergency: Mutex::new(EmergencyFences::default()),
            service_instance: "svc_test".into(),
            runner_auth: Mutex::new(HashMap::new()),
            started: Instant::now(),
        };

        record_expired_grants(&runtime).unwrap();
        record_expired_grants(&runtime).unwrap();
        runtime
            .read(|doc| {
                assert!(
                    active_execution_grant(&runtime, doc, "client_a", &session, false).is_err()
                );
                assert_eq!(
                    doc.audit
                        .iter()
                        .filter(|event| event.kind == "grant-expired")
                        .count(),
                    1
                );
            })
            .unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn emergency_fence_survives_state_write_failure() {
        let root = test_root("ef");
        let runner = FakeRunner::start(&root, "g_a");
        let mut doc = DiskDoc::default();
        doc.config.enabled = true;
        doc.config.clients.push(client_record(&root));
        let session = session_record(&root, &runner);
        doc.sessions.push(session.clone());
        let runtime = Runtime {
            app: None,
            path: root.join("missing-parent/state.json"),
            socket: root.join("control.sock"),
            doc: Mutex::new(Ok(doc)),
            io: Mutex::new(()),
            delivery: Mutex::new(()),
            emergency: Mutex::new(EmergencyFences::default()),
            service_instance: "svc_test".into(),
            runner_auth: Mutex::new(HashMap::new()),
            started: Instant::now(),
        };
        runtime
            .emergency
            .lock_or_recover()
            .human_sessions
            .insert(session.session_id.clone());
        assert!(runtime.write(|_| Ok(())).is_err());

        let inspect = route(
            &runtime,
            request(
                "deck_session_inspect",
                json!({"session_id":"mcp_a","holder_id":"holder_a"}),
            ),
        );
        assert_eq!(inspect["humanLock"], true);
        assert!(inspect["terminalContext"].is_null());
        assert_eq!(
            route(
                &runtime,
                request(
                    "deck_exec",
                    json!({"session_id":"mcp_a","request_id":"blocked"}),
                ),
            )["error"]["code"],
            "HUMAN_CONTROL"
        );
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_or_extended_state_fails_closed() {
        let root = std::env::temp_dir().join(format!("deck-mcp-corrupt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).unwrap();
        let path = root.join("mcp.json");
        std::fs::write(&path, br#"{"version":1,"config":{"enabled":true,"clients":[]},"sessions":[],"operations":[],"jobs":[],"unexpected":true}"#).unwrap();
        let error = match load(&path) {
            Err(error) => error,
            Ok(_) => panic!("extended state was accepted"),
        };
        assert_eq!(error.kind(), ErrorKind::Recovery);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn command_surface_exercises_local_authorization_and_board_reconciliation() {
        let root = test_root("commands");
        let runner = FakeRunner::start(&root, "g_a");
        let mut doc = DiskDoc::default();
        doc.config.enabled = true;
        doc.config.clients.push(client_record(&root));
        let session = session_record(&root, &runner);
        doc.execution_grants.push(execution_grant(&session));
        doc.sessions.push(session);
        let path = root.join("mcp.json");
        save(&path, &doc).unwrap();
        let runtime = Arc::new(Runtime {
            app: None,
            path,
            socket: root.join("control.sock"),
            doc: Mutex::new(Ok(doc)),
            io: Mutex::new(()),
            delivery: Mutex::new(()),
            emergency: Mutex::new(EmergencyFences::default()),
            service_instance: "svc_test".into(),
            runner_auth: Mutex::new(HashMap::new()),
            started: Instant::now(),
        });
        assert!(RUNTIME.set(runtime.clone()).is_ok());

        let status = mcp_status().unwrap();
        assert!(status.enabled);
        assert!(!status.socket_ready);
        assert_eq!(status.clients.len(), 1);
        assert!(mcp_adapter_path().is_err());
        assert!(mcp_client_add("".into(), vec![], false).is_err());
        assert!(mcp_client_add(
            "Bad scope".into(),
            vec![ProjectScopeInput {
                project_id: "bad id".into(),
                roots: vec![root.display().to_string()],
            }],
            false,
        )
        .is_err());
        let added = mcp_client_add(
            "Client B".into(),
            vec![ProjectScopeInput {
                project_id: "P2".into(),
                roots: vec![root.display().to_string()],
            }],
            true,
        )
        .unwrap();
        assert_eq!(added.name, "Client B");
        let preview = mcp_scope_preview(root.display().to_string());
        assert!(preview.ok);
        assert_eq!(
            preview.root.unwrap(),
            std::fs::canonicalize(&root).unwrap().display().to_string()
        );
        assert_eq!(
            mcp_client_delete(added.id.clone()).unwrap_err().kind(),
            ErrorKind::Perm
        );

        let unmanaged = mcp_session_ui("missing".into()).unwrap();
        assert!(!unmanaged.managed);
        let managed = mcp_session_ui("M1".into()).unwrap();
        assert!(managed.managed);
        assert!(!managed.human_control);
        assert_eq!(managed.client_name.as_deref(), Some("Client A"));
        assert!(guard_terminal_input("deck-mcp-test").is_err());
        assert!(guard_server_restart().is_err());
        assert!(runner_socket_matches(
            &runtime,
            runner.socket.to_str().unwrap(),
            "g_a"
        ));
        assert!(!runner_socket_matches(
            &runtime,
            runner.socket.to_str().unwrap(),
            "g_other"
        ));

        mcp_takeover("mcp_a".into()).unwrap();
        assert!(guard_terminal_input("deck-mcp-test").is_ok());
        mcp_return_control("M1".into()).unwrap();
        assert!(guard_terminal_input("deck-mcp-test").is_err());

        mcp_disable().unwrap();
        assert!(!mcp_status().unwrap().enabled);
        assert!(mcp_pending().unwrap().is_empty());
        mcp_enable().unwrap();

        let create = session_create(
            &runtime,
            "client_a",
            json!({"request_id":"command_create","project_id":"P1","cwd":root.display().to_string(),"create_sequence":0}),
        )
        .unwrap();
        let create_id = create["operationId"].as_str().unwrap().to_owned();
        assert_eq!(mcp_pending().unwrap().len(), 1);
        let claimed = mcp_claim(create_id.clone()).unwrap();
        assert_eq!(claimed.kind, "session-create");
        mcp_validate(create_id.clone()).unwrap();
        assert!(mcp_complete(create_id.clone(), "invalid".into(), None, None).is_err());
        mcp_complete(
            create_id,
            "committed".into(),
            None,
            Some("deck-created".into()),
        )
        .unwrap();

        let created_session = runtime
            .read(|doc| {
                doc.sessions
                    .iter()
                    .find(|session| session.tmux_session == "deck-created")
                    .cloned()
                    .unwrap()
            })
            .unwrap();
        let close = session_close(
            &runtime,
            "client_a",
            json!({
                "request_id":"command_close",
                "session_id":created_session.session_id,
                "expected_generation":created_session.generation,
                "control_epoch":1,
                "holder_id":"holder_a",
                "confirm_running":false
            }),
        );
        assert_eq!(close.unwrap_err()["error"]["code"], "CONTROL_REVOKED");
        mcp_card_closed(created_session.card_id).unwrap();

        assert!(mcp_claim("missing".into()).is_err());
        assert!(mcp_validate("missing".into()).is_err());
        assert!(mcp_start_session(
            "missing".into(),
            "deck-valid".into(),
            root.display().to_string()
        )
        .is_err());
        mcp_client_revoke(added.id.clone()).unwrap();
        mcp_client_delete(added.id.clone()).unwrap();
        assert!(mcp_status()
            .unwrap()
            .clients
            .iter()
            .all(|client| client.id != added.id));
        assert!(runtime
            .read(|doc| doc.audit.iter().any(|event| {
                event.kind == "client-deleted"
                    && event.principal_id.as_deref() == Some(added.id.as_str())
            }))
            .unwrap());
        mcp_card_closed("M1".into()).unwrap();
        assert!(guard_server_restart().is_ok());
        assert!(guard_terminal_input("ordinary-session").is_ok());

        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn production_routes_cover_authorized_job_and_control_lifecycle() {
        let root = test_root("routes");
        let runner = FakeRunner::start(&root, "g_a");
        let mut doc = DiskDoc::default();
        doc.config.enabled = true;
        doc.config.clients.push(client_record(&root));
        let mut session = session_record(&root, &runner);
        session.control_owner = None;
        session.control_holder = None;
        session.lease_expires_at = None;
        doc.sessions.push(session);
        let path = root.join("mcp.json");
        save(&path, &doc).unwrap();
        let runtime = Arc::new(Runtime {
            app: None,
            path,
            socket: root.join("control.sock"),
            doc: Mutex::new(Ok(doc)),
            io: Mutex::new(()),
            delivery: Mutex::new(()),
            emergency: Mutex::new(EmergencyFences::default()),
            service_instance: "svc_test".into(),
            runner_auth: Mutex::new(HashMap::new()),
            started: Instant::now(),
        });

        assert_eq!(
            route(
                &runtime,
                WireRequest {
                    version: 99,
                    client_id: "client_a".into(),
                    credential: "mcp_test".into(),
                    tool: "deck_capabilities".into(),
                    arguments: json!({}),
                }
            )["error"]["code"],
            "PROTOCOL_MISMATCH",
            "an adapter/app version skew is not reported as an auth failure"
        );
        let mut wrong_credential = request("deck_capabilities", json!({}));
        wrong_credential.credential = "mcp_wrong".into();
        assert_eq!(
            route(&runtime, wrong_credential)["error"]["code"],
            "AUTH_REQUIRED"
        );
        assert_eq!(
            route(&runtime, request("unknown", json!({})))["error"]["code"],
            "UNSUPPORTED"
        );
        assert!(route(&runtime, request("deck_capabilities", json!({})))["ok"] == true);
        assert_eq!(
            route(
                &runtime,
                request(
                    "deck_project_read",
                    json!({"project_id":"P1","path":".env"}),
                ),
            )["error"]["code"],
            "READ_DENIED"
        );
        std::fs::write(root.join("target.txt"), "DECK_SYNTHETIC_ROUTE_MARKER\n").unwrap();
        let file_search = route(
            &runtime,
            request(
                "deck_project_search",
                json!({"project_id":"P1","path":"target.txt","query":"DECK_SYNTHETIC_ROUTE_MARKER"}),
            ),
        );
        assert_eq!(file_search["matches"].as_array().unwrap().len(), 1);
        assert_eq!(file_search["complete"], true);
        assert_eq!(
            route(
                &runtime,
                request(
                    "deck_project_search",
                    json!({"project_id":"P1","path":"../outside","query":"x"}),
                ),
            )["error"]["code"],
            "INVALID_ARGUMENTS"
        );
        assert_eq!(
            route(&runtime, request("deck_sessions_list", json!({})))["sessions"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            route(
                &runtime,
                request("deck_session_inspect", json!({"session_id":"mcp_a"}))
            )["sessionGeneration"],
            "g_a"
        );

        let control = |request_id: &str, action: &str, epoch: Option<u64>| {
            let mut value = json!({
                "request_id":request_id,
                "session_id":"mcp_a",
                "expected_generation":"g_a",
                "action":action,
                "holder_id":"holder_a"
            });
            if action != "release" {
                value["lease_ms"] = json!(2_000);
            }
            if let Some(epoch) = epoch {
                value["control_epoch"] = json!(epoch);
            }
            value["control_sequence"] = json!(session_state(&runtime).control_sequence);
            route(&runtime, request("deck_session_control", value))
        };
        let operation_count = runtime.read(|doc| doc.operations.len()).unwrap();
        for arguments in [
            json!({"request_id":"bad_lease_low","session_id":"mcp_a","expected_generation":"g_a","action":"request","holder_id":"holder_a","lease_ms":999,"control_sequence":0}),
            json!({"request_id":"bad_request_epoch","session_id":"mcp_a","expected_generation":"g_a","action":"request","holder_id":"holder_a","control_epoch":1,"control_sequence":0}),
            json!({"request_id":"bad_renew_epoch","session_id":"mcp_a","expected_generation":"g_a","action":"renew","holder_id":"holder_a","control_sequence":0}),
            json!({"request_id":"bad_release_lease","session_id":"mcp_a","expected_generation":"g_a","action":"release","holder_id":"holder_a","control_epoch":1,"lease_ms":1000,"control_sequence":0}),
        ] {
            assert_eq!(
                route(&runtime, request("deck_session_control", arguments))["error"]["code"],
                "INVALID_ARGUMENTS"
            );
        }
        assert_eq!(
            runtime.read(|doc| doc.operations.len()).unwrap(),
            operation_count
        );
        assert_eq!(
            control("control_request", "request", None)["state"],
            "committed"
        );
        assert_eq!(
            route(
                &runtime,
                request(
                    "deck_session_control",
                    json!({"request_id":"holder_conflict","session_id":"mcp_a","expected_generation":"g_a","action":"request","holder_id":"holder_other","control_sequence":session_state(&runtime).control_sequence}),
                ),
            )["error"]["code"],
            "PERMISSION_DENIED"
        );
        assert_eq!(
            control("control_renew", "renew", Some(2))["state"],
            "committed"
        );
        assert_eq!(
            control("control_release", "release", Some(2))["state"],
            "committed"
        );
        let control_again = control("control_again", "request", None);
        assert_eq!(control_again["state"], "committed", "{control_again}");

        let denied = route(
            &runtime,
            request(
                "deck_exec",
                json!({"request_id":"exec_without_grant","session_id":"mcp_a","expected_generation":"g_a","control_epoch":4,"holder_id":"holder_a","executable":"/usr/bin/printf","args":["forbidden"],"cwd":root.display().to_string()}),
            ),
        );
        assert_eq!(denied["error"]["code"], "EXECUTION_GRANT_REQUIRED");
        runtime
            .write(|doc| {
                let session = doc.sessions[0].clone();
                doc.execution_grants.push(execution_grant(&session));
                Ok(())
            })
            .unwrap();
        let grant_expiry = runtime
            .read(|doc| doc.execution_grants.last().unwrap().expires_at)
            .unwrap();
        assert_eq!(
            control("control_after_grant", "renew", Some(4))["state"],
            "committed"
        );
        assert_eq!(
            runtime
                .read(|doc| doc.execution_grants.last().unwrap().expires_at)
                .unwrap(),
            grant_expiry
        );

        let exec_arguments = json!({
            "request_id":"exec_a",
            "session_id":"mcp_a",
            "expected_generation":"g_a",
            "control_epoch":4,
            "holder_id":"holder_a",
            "executable":"/usr/bin/printf",
            "args":["done\\n"],
            "cwd":root.display().to_string(),
            "wait_ms":10,
            "execution_timeout_ms":1_000
        });
        let executed = route(&runtime, request("deck_exec", exec_arguments.clone()));
        assert_eq!(executed["state"], "exited", "{executed}");
        let persisted_launch = runtime
            .read(|doc| {
                doc.operations
                    .iter()
                    .find(|operation| operation.request_id == "exec_a")
                    .and_then(|operation| operation.result.clone())
                    .unwrap()
            })
            .unwrap();
        assert_eq!(persisted_launch["launch"]["launchKind"], "direct");
        let persisted_text = persisted_launch.to_string();
        assert!(!persisted_text.contains("/usr/bin/printf"));
        assert!(!persisted_text.contains("done\\n"));
        let job_id = executed["jobId"].as_str().unwrap().to_owned();
        assert_eq!(
            route(&runtime, request("deck_exec", exec_arguments))["jobId"],
            job_id
        );

        let read = route(
            &runtime,
            request(
                "deck_job_read",
                json!({"job_id":job_id,"max_bytes":1024,"wait_ms":0}),
            ),
        );
        assert_eq!(read["exitCode"], 0);
        assert_eq!(read["output"], "done\n");
        assert_eq!(
            route(
                &runtime,
                request(
                    "deck_job_input",
                    json!({"request_id":"input_a","job_id":job_id,"session_generation":"g_a","control_epoch":4,"holder_id":"holder_a","input":"yes\n"}),
                ),
            )["state"],
            "committed"
        );
        assert_eq!(
            route(
                &runtime,
                request(
                    "deck_job_interrupt",
                    json!({"request_id":"interrupt_a","job_id":job_id,"session_generation":"g_a","control_epoch":4,"holder_id":"holder_a"}),
                ),
            )["state"],
            "committed"
        );

        let create = route(
            &runtime,
            request(
                "deck_session_create",
                json!({"request_id":"create_a","project_id":"P1","cwd":root.display().to_string(),"title":"Visible shell","create_sequence":0}),
            ),
        );
        assert_eq!(create["state"], "accepted");
        let operation_id = create["operationId"].as_str().unwrap();
        assert_eq!(
            route(
                &runtime,
                request("deck_operation_get", json!({"operation_id":operation_id}),),
            )["kind"],
            "session-create"
        );

        let close = route(
            &runtime,
            request(
                "deck_session_close",
                json!({"request_id":"close_a","session_id":"mcp_a","expected_generation":"g_a","control_epoch":4,"holder_id":"holder_a","confirm_running":false}),
            ),
        );
        assert_eq!(close["state"], "accepted", "{close}");
        let close_id = close["operationId"].as_str().unwrap().to_owned();
        runtime
            .write(|doc| {
                doc.operations
                    .iter_mut()
                    .find(|operation| operation.operation_id == close_id)
                    .unwrap()
                    .state = "executing".into();
                let session = doc
                    .sessions
                    .iter_mut()
                    .find(|session| session.session_id == "mcp_a")
                    .unwrap();
                session.human_lock = true;
                session.control_owner = None;
                session.control_epoch += 1;
                Ok(())
            })
            .unwrap();
        assert!(close_admit(&runtime, &close_id).is_err());

        runtime
            .write(|doc| {
                let operation = doc
                    .operations
                    .iter_mut()
                    .find(|operation| operation.operation_id == close_id)
                    .unwrap();
                operation.state = "executing".into();
                let epoch = operation.result.as_ref().unwrap()["controlEpoch"]
                    .as_u64()
                    .unwrap();
                let session = doc
                    .sessions
                    .iter_mut()
                    .find(|session| session.session_id == "mcp_a")
                    .unwrap();
                session.human_lock = false;
                session.control_owner = Some("client_a".into());
                session.control_epoch = epoch;
                Ok(())
            })
            .unwrap();
        let admission = close_admit(&runtime, &close_id).unwrap().admission;
        runtime
            .write(|doc| {
                doc.config.clients[0].revoked_at = Some(now_ms());
                Ok(())
            })
            .unwrap();
        validate_close_admission_with(&runtime, Some(&admission), &["deck-mcp-test".into()])
            .unwrap();
        runtime
            .write(|doc| {
                doc.config.clients[0].revoked_at = None;
                Ok(())
            })
            .unwrap();

        assert_eq!(
            route(&runtime, request("deck_exec", json!({})))["error"]["code"],
            "INVALID_ARGUMENTS"
        );
        assert_eq!(
            route(
                &runtime,
                request(
                    "deck_job_read",
                    json!({"job_id":job_id,"cursor":"wrong:cursor:1"}),
                ),
            )["error"]["code"],
            "OUTPUT_CURSOR_INVALID"
        );
        let audit_kinds = runtime
            .read(|doc| {
                doc.audit
                    .iter()
                    .map(|event| event.kind.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap();
        assert!(audit_kinds.iter().any(|kind| kind == "control-changed"));
        assert!(audit_kinds.iter().any(|kind| kind == "exec-intent"));
        assert!(audit_kinds.iter().any(|kind| kind == "exec-dispatch"));
        assert!(audit_kinds.iter().any(|kind| kind == "request-denied"));
        assert!(audit_kinds.iter().any(|kind| kind == "read-denied"));

        drop(runtime);
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn validation_and_error_mapping_cover_rejected_boundaries() {
        assert!(!valid_direct_launch("git", &[]));
        assert!(!valid_direct_launch("./tool", &[]));
        assert!(valid_direct_launch("/usr/bin/git", &[]));
        assert!(!valid_id(""));
        assert!(!valid_id("bad id"));
        assert!(valid_id("good_ID-1"));
        assert!(!valid_title("\n"));
        assert!(!valid_title(&"x".repeat(121)));
        assert_eq!(sha(b"same"), sha(b"same"));
        assert!(random_id("test_").unwrap().starts_with("test_"));
        assert!(parse::<Empty>(json!({"extra":true})).is_err());
        assert!(runner_error(&json!({"error":"session-busy"})).is_some());
        assert!(runner_error(&json!({"error":"control-revoked"})).is_some());
        assert!(runner_error(&json!({"error":"job-not-found"})).is_some());
        assert!(runner_error(&json!({"error":"job-not-running"})).is_some());
        assert!(runner_error(&json!({"error":"request-id-conflict"})).is_some());
        assert!(runner_error(&json!({"error":"capacity-exceeded"})).is_some());
        assert!(runner_error(&json!({"error":"invalid-cwd"})).is_some());
        assert!(runner_error(&json!({"error":"other"})).is_none());

        for (kind, code) in [
            (ErrorKind::Perm, "PERMISSION_DENIED"),
            (ErrorKind::Missing, "SESSION_NOT_FOUND"),
            (ErrorKind::NotDir, "CONTEXT_CHANGED"),
            (ErrorKind::ContextChanged, "CONTEXT_CHANGED"),
            (ErrorKind::ControlRevoked, "CONTROL_REVOKED"),
            (ErrorKind::RequestConflict, "REQUEST_ID_CONFLICT"),
            (ErrorKind::DiskFull, "CAPACITY_EXCEEDED"),
            (ErrorKind::Locked, "SESSION_BUSY"),
            (ErrorKind::Other, "INTERNAL_ERROR"),
        ] {
            assert_eq!(
                map_error(DeckError::new(kind, "test"))["error"]["code"],
                code
            );
        }

        let root = test_root("validation");
        let missing = root.join("missing");
        assert!(canonical_scope(missing.to_str().unwrap(), &[]).is_err());
        assert!(canonical_scope(root.to_str().unwrap(), &["/elsewhere".into()]).is_err());
        assert_eq!(
            load(&root.join("absent.json")).unwrap().version,
            STATE_VERSION
        );

        let mut invalid = DiskDoc {
            version: STATE_VERSION + 1,
            ..DiskDoc::default()
        };
        assert!(validate_doc(&invalid).is_err());
        invalid.version = STATE_VERSION;
        invalid.config.clients.push(Client {
            id: "bad id".into(),
            name: "Bad".into(),
            credential_hash: sha(b"mcp_test"),
            credential_version: 1,
            revoked_at: None,
            allow_create: false,
            projects: vec![],
            create_sequence: 0,
        });
        assert!(validate_doc(&invalid).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn v2_state_migration_revokes_clients_and_never_restores_execution() {
        let root = test_root("v2-migration");
        let path = root.join("mcp.json");
        let runner = FakeRunner::start(&root, "g_a");
        let mut doc = DiskDoc {
            version: 2,
            ..DiskDoc::default()
        };
        doc.config.enabled = true;
        doc.config.clients.push(client_record(&root));
        let session = session_record(&root, &runner);
        doc.execution_grants.push(execution_grant(&session));
        doc.sessions.push(session);
        doc.operations.push(operation("exec", "accepted"));
        std::fs::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();

        let migrated = load(&path).unwrap();
        assert_eq!(migrated.version, STATE_VERSION);
        assert!(!migrated.config.enabled);
        assert!(migrated.config.clients[0].revoked_at.is_some());
        assert_eq!(migrated.config.clients[0].credential_version, 0);
        assert!(migrated.execution_grants.is_empty());
        assert_eq!(migrated.operations[0].state, "ambiguous");
        assert_eq!(
            migrated.operations[0].code.as_deref(),
            Some("v3-reauthorization-required")
        );
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn encoded_frames_fit_the_advertised_budget() {
        let hostile = "\"\\\n\r\t\u{0001}".repeat(MAX_ARGUMENT_BYTES / 6);
        let request = json!({"kind":"exec","executable":"/usr/bin/printf","args":[hostile],"context":{"intent_hash":"a".repeat(64)}});
        assert!(serde_json::to_vec(&request).unwrap().len() < MAX_REQUEST_BYTES);
        let output = "\"\\\n\r\t\u{0001}".repeat(MAX_READ_BYTES / 6);
        let response = json!({"ok":true,"output":output});
        assert!(serde_json::to_vec(&response).unwrap().len() < MAX_RESPONSE_BYTES);
    }

    #[test]
    fn concurrent_create_reserves_one_operation_and_one_plan() {
        let root = test_root("concurrent-create");
        let mut doc = DiskDoc::default();
        doc.config.enabled = true;
        doc.config.clients.push(client_record(&root));
        let runtime = Arc::new(Runtime {
            app: None,
            path: root.join("mcp.json"),
            socket: root.join("control.sock"),
            doc: Mutex::new(Ok(doc)),
            io: Mutex::new(()),
            delivery: Mutex::new(()),
            emergency: Mutex::new(EmergencyFences::default()),
            service_instance: "svc_concurrent".into(),
            runner_auth: Mutex::new(HashMap::new()),
            started: Instant::now(),
        });
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let mut threads = Vec::new();
        for _ in 0..2 {
            let runtime = runtime.clone();
            let barrier = barrier.clone();
            let cwd = root.display().to_string();
            threads.push(std::thread::spawn(move || {
                barrier.wait();
                session_create(
                    &runtime,
                    "client_a",
                    json!({"request_id":"same_create","project_id":"P1","cwd":cwd,"create_sequence":0}),
                )
                .unwrap()["operationId"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            }));
        }
        barrier.wait();
        let ids = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(ids[0], ids[1]);
        runtime
            .read(|doc| {
                assert_eq!(doc.operations.len(), 1);
                assert_eq!(
                    doc.operations
                        .iter()
                        .filter(|operation| operation.kind == "session-create")
                        .count(),
                    1
                );
            })
            .unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Runtime + fake runner + one managed session (`mcp_a`, card `M1`).
    fn fixture(tag: &str, service: &str) -> (Arc<Runtime>, FakeRunner, PathBuf) {
        let root = test_root(tag);
        let runner = FakeRunner::start(&root, "g_a");
        // This fixture represents an already-established session. Tests that
        // exercise creation use a fresh runner at epoch zero instead.
        runner.set_control(1, "mcp", Some("holder_a"));
        let mut doc = DiskDoc::default();
        doc.config.enabled = true;
        doc.config.clients.push(client_record(&root));
        doc.sessions.push(session_record(&root, &runner));
        let path = root.join("mcp.json");
        save(&path, &doc).unwrap();
        let runtime = Arc::new(Runtime {
            app: None,
            path,
            socket: root.join("control.sock"),
            doc: Mutex::new(Ok(doc)),
            io: Mutex::new(()),
            delivery: Mutex::new(()),
            emergency: Mutex::new(EmergencyFences::default()),
            service_instance: service.into(),
            runner_auth: Mutex::new(HashMap::new()),
            started: Instant::now(),
        });
        (runtime, runner, root)
    }

    fn session_state(runtime: &Runtime) -> ManagedSession {
        runtime.read(|doc| doc.sessions[0].clone()).unwrap()
    }

    #[test]
    fn authorized_create_control_job_takeover_return_and_release_stay_in_sync() {
        let root = test_root("authorized-create-epoch");
        let runner = FakeRunner::start(&root, "g_a");
        let mut doc = DiskDoc::default();
        doc.config.enabled = true;
        doc.config.clients.push(client_record(&root));
        let path = root.join("mcp.json");
        let runtime = Arc::new(Runtime {
            app: None,
            path,
            socket: root.join("control.sock"),
            doc: Mutex::new(Ok(doc)),
            io: Mutex::new(()),
            delivery: Mutex::new(()),
            emergency: Mutex::new(EmergencyFences::default()),
            service_instance: "svc_test".into(),
            runner_auth: Mutex::new(HashMap::new()),
            started: Instant::now(),
        });
        let created = session_create(
            &runtime,
            "client_a",
            json!({"request_id":"create_epoch","project_id":"P1","cwd":root.display().to_string(),"create_sequence":0}),
        )
        .unwrap();
        let operation_id = created["operationId"].as_str().unwrap().to_owned();
        runtime
            .write(|doc| {
                let operation = doc
                    .operations
                    .iter_mut()
                    .find(|operation| operation.operation_id == operation_id)
                    .unwrap();
                operation.state = "executing".into();
                let result = operation.result.as_mut().unwrap();
                result["sessionId"] = json!("mcp_created");
                result["cardId"] = json!("Mcreated");
                result["generation"] = json!("g_a");
                result["runnerSocket"] = json!(runner.socket.display().to_string());
                operation.session_id = Some("mcp_created".into());
                Ok(())
            })
            .unwrap();
        complete(
            &runtime,
            operation_id,
            "committed".into(),
            None,
            Some("deck-created".into()),
        )
        .unwrap();
        assert_eq!(
            runner.control_epoch(),
            0,
            "authorized create sends no control"
        );

        let request_control = |request_id: &str, sequence: u64| {
            route(
                &runtime,
                request(
                    "deck_session_control",
                    json!({"request_id":request_id,"session_id":"mcp_created","expected_generation":"g_a","action":"request","holder_id":"holder_created","control_sequence":sequence}),
                ),
            )
        };
        let first = request_control("request_first", 0);
        assert_eq!(first["state"], "committed", "{first}");
        assert_eq!(first["result"]["controlEpoch"], 2);
        assert_eq!(runner.control_epoch(), 2);

        super::execution_grant(&runtime, "mcp_created".into(), Some(60_000), true, true).unwrap();
        let exec = route(
            &runtime,
            request(
                "deck_exec",
                json!({"request_id":"exec_created","session_id":"mcp_created","expected_generation":"g_a","control_epoch":2,"holder_id":"holder_created","cwd":root.display().to_string(),"executable":"/usr/bin/true","args":[],"wait_ms":0}),
            ),
        );
        assert_eq!(exec["state"], "exited", "{exec}");

        takeover(&runtime, "mcp_created").unwrap();
        assert_eq!(runner.control_epoch(), 3);
        return_control(&runtime, "mcp_created").unwrap();
        assert_eq!(runner.control_epoch(), 4);

        let second = request_control("request_second", 1);
        assert_eq!(second["state"], "committed", "{second}");
        assert_eq!(second["result"]["controlEpoch"], 5);
        let released = route(
            &runtime,
            request(
                "deck_session_control",
                json!({"request_id":"release_created","session_id":"mcp_created","expected_generation":"g_a","action":"release","holder_id":"holder_created","control_epoch":5,"control_sequence":2}),
            ),
        );
        assert_eq!(released["state"], "committed", "{released}");
        assert_eq!(runner.control_epoch(), 6);
        drop(runtime);
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn lapsed_lease_epoch_gap_is_resynchronized_by_next_request() {
        let (runtime, runner, root) = fixture("lapsed-epoch-gap", "svc_test");
        runtime
            .write(|doc| {
                doc.sessions[0].lease_expires_at = Some(now_ms().saturating_sub(1));
                close_lapsed_leases(doc);
                Ok(())
            })
            .unwrap();
        assert_eq!(session_state(&runtime).control_epoch, 2);
        assert_eq!(runner.control_epoch(), 1);
        let requested = route(
            &runtime,
            request(
                "deck_session_control",
                json!({"request_id":"request_after_lapse","session_id":"mcp_a","expected_generation":"g_a","action":"request","holder_id":"holder_a","control_sequence":0}),
            ),
        );
        assert_eq!(requested["state"], "committed", "{requested}");
        assert_eq!(requested["result"]["controlEpoch"], 3);
        assert_eq!(runner.control_epoch(), 3);
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_runner_control_does_not_poison_the_next_takeover() {
        let (runtime, runner, root) = fixture("failed-control-gap", "svc_test");
        runner.fail_next_control();
        assert_eq!(
            takeover(&runtime, "mcp_a").unwrap_err().message(),
            RUNNER_UNCONFIRMED
        );
        assert_eq!(session_state(&runtime).control_epoch, 2);
        assert_eq!(runner.control_epoch(), 1);

        takeover(&runtime, "mcp_a").unwrap();
        assert_eq!(session_state(&runtime).control_epoch, 3);
        assert_eq!(runner.control_epoch(), 3);
        assert_eq!(runner.last("control").unwrap()["mode"], "human");
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn execution_grant_allows_any_program_including_shells() {
        let (runtime, runner, root) = fixture("shell-boundary", "svc_test");
        runtime
            .write(|doc| {
                doc.execution_grants.push(execution_grant(&doc.sessions[0]));
                Ok(())
            })
            .unwrap();
        let direct = json!({"request_id":"direct_shell","session_id":"mcp_a","expected_generation":"g_a","control_epoch":1,"holder_id":"holder_a","cwd":root.display().to_string(),"executable":"/bin/zsh","args":["-c","true"]});
        assert_eq!(route(&runtime, request("deck_exec", direct))["ok"], true);
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn expired_grant(session: &ManagedSession) -> ExecutionGrant {
        let mut grant = execution_grant(session);
        grant.issued_at = now_ms().saturating_sub(120_000);
        grant.expires_at = now_ms().saturating_sub(60_000);
        grant
    }

    #[test]
    fn return_after_takeover_needs_no_grant_and_restores_nothing() {
        // The acceptance failure: epoch 2 → takeover → grant already expired →
        // return was refused with "a local execution grant is required".
        let (runtime, runner, root) = fixture("return-expired", "svc_test");
        runtime
            .write(|doc| {
                let grant = expired_grant(&doc.sessions[0]);
                doc.execution_grants.push(grant);
                doc.sessions[0].control_epoch = 2;
                Ok(())
            })
            .unwrap();
        takeover(&runtime, "M1").unwrap();
        let taken = session_state(&runtime);
        assert!(taken.human_lock);
        assert_eq!(taken.control_epoch, 3);
        assert!(taken.control_owner.is_none() && taken.control_holder.is_none());
        assert!(taken.lease_expires_at.is_none());
        assert!(!taken.output_shared);
        assert_eq!(runner.last("control").unwrap()["mode"], "human");
        let grants_before = runtime.read(|doc| doc.execution_grants.len()).unwrap();

        return_control(&runtime, "M1").unwrap();
        let returned = session_state(&runtime);
        assert!(!returned.human_lock);
        assert_eq!(returned.control_epoch, 4, "a new epoch, never the old one");
        assert!(returned.control_owner.is_none() && returned.control_holder.is_none());
        assert!(returned.lease_expires_at.is_none());
        assert!(!returned.output_shared, "sharing is not restored");
        let control = runner.last("control").unwrap();
        assert_eq!(control["mode"], "mcp");
        assert_eq!(control["control_epoch"], 4);
        assert!(control["holder_id"].is_null());
        runtime
            .read(|doc| {
                assert_eq!(
                    doc.execution_grants.len(),
                    grants_before,
                    "no grant created"
                );
                assert!(matches!(
                    execution_authorization(&runtime, doc, "client_a", &doc.sessions[0]),
                    ExecutionAuthorization::Expired(_)
                ));
            })
            .unwrap();
        assert!(!runtime
            .emergency
            .lock_or_recover()
            .human_sessions
            .contains("mcp_a"));
        // The pre-takeover holder's epoch is dead.
        assert!(check_control(&returned, "client_a", "g_a", 2, "holder_a").is_err());
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn return_is_refused_with_stable_codes_and_changes_nothing() {
        let (runtime, runner, root) = fixture("return-refused", "svc_test");
        takeover(&runtime, "mcp_a").unwrap();
        let fenced = session_state(&runtime);

        runner.busy.store(true, Ordering::SeqCst);
        let busy = return_control(&runtime, "M1").unwrap_err();
        assert_eq!(busy.message(), SESSION_BUSY);
        assert_eq!(session_state(&runtime).control_epoch, fenced.control_epoch);
        assert!(session_state(&runtime).human_lock);
        runner.busy.store(false, Ordering::SeqCst);

        runtime
            .write(|doc| {
                doc.config.clients[0].revoked_at = Some(now_ms());
                Ok(())
            })
            .unwrap();
        assert_eq!(
            return_control(&runtime, "M1").unwrap_err().message(),
            CLIENT_REVOKED
        );
        runtime
            .write(|doc| {
                doc.config.clients[0].revoked_at = None;
                doc.config.enabled = false;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            return_control(&runtime, "M1").unwrap_err().message(),
            FEATURE_DISABLED
        );
        let after = session_state(&runtime);
        assert!(after.human_lock);
        assert_eq!(after.control_epoch, fenced.control_epoch);
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_runner_from_before_a_restart_is_stale_but_still_fenced_and_stoppable() {
        let (runtime, runner, root) = fixture("stale", "svc_stale");
        let error = takeover(&runtime, "M1").unwrap_err();
        assert_eq!(error.message(), RUNNER_STALE);
        assert!(session_state(&runtime).human_lock, "the fence is durable");
        assert_eq!(
            return_control(&runtime, "M1").unwrap_err().message(),
            RUNNER_STALE
        );
        let inspect = route(
            &runtime,
            request("deck_session_inspect", json!({"session_id":"mcp_a"})),
        );
        assert_eq!(inspect["stale"], true);
        assert_eq!(inspect["mayStartNextJobReason"], "RUNNER_STALE");
        // `stop` is bound to the generation only: a stale runner still stops.
        assert_eq!(runner.count("stop"), 0);
        let session = session_state(&runtime);
        send_runner(
            &runtime,
            &session,
            &json!({"kind":"stop","generation":session.generation}),
        )
        .unwrap();
        assert_eq!(runner.count("stop"), 1);
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn takeover_fences_before_waiting_for_an_in_flight_dispatch() {
        let (runtime, runner, root) = fixture("fence-first", "svc_test");
        // An exec holds the delivery lock across runner I/O.
        let delivery = runtime.delivery.lock_or_recover();
        let local = runtime.clone();
        let takeover_thread = std::thread::spawn(move || takeover(&local, "M1"));
        let deadline = Instant::now() + Duration::from_secs(5);
        while !runtime
            .emergency
            .lock_or_recover()
            .human_sessions
            .contains("mcp_a")
        {
            assert!(Instant::now() < deadline, "fence waited for the lock");
            std::thread::yield_now();
        }
        // While the lock is still held, every new remote request is refused.
        let exec = route(
            &runtime,
            request(
                "deck_exec",
                json!({"request_id":"late","session_id":"mcp_a","expected_generation":"g_a","control_epoch":1,"holder_id":"holder_a","executable":"/usr/bin/true","args":[]}),
            ),
        );
        assert_eq!(exec["error"]["code"], "HUMAN_CONTROL");
        assert!(emergency_denial(&runtime, "client_a", "mcp_a", Admission::Control).is_some());
        drop(delivery);
        takeover_thread.join().unwrap().unwrap();
        assert!(session_state(&runtime).human_lock);
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn job_read_drops_output_when_a_takeover_lands_during_the_wait() {
        let (runtime, runner, root) = fixture("read-recheck", "svc_test");
        runtime
            .write(|doc| {
                let grant = execution_grant(&doc.sessions[0]);
                doc.jobs.push(JobBinding {
                    job_id: "job_a".into(),
                    client_id: "client_a".into(),
                    session_id: "mcp_a".into(),
                    session_generation: "g_a".into(),
                    request_hash: "a".repeat(64),
                    operation_id: "op_exec".into(),
                    grant_id: grant.grant_id.clone(),
                    grant_version: 1,
                    allow_output: true,
                });
                doc.execution_grants.push(grant);
                Ok(())
            })
            .unwrap();
        let read = || {
            route(
                &runtime,
                request(
                    "deck_job_read",
                    json!({"job_id":"job_a","max_bytes":1024,"wait_ms":5000}),
                ),
            )
        };
        assert_eq!(read()["output"], "done\n", "baseline read is allowed");

        let (entered, release) = runner.hold("read");
        let reader = {
            let runtime = runtime.clone();
            std::thread::spawn(move || {
                route(
                    &runtime,
                    request(
                        "deck_job_read",
                        json!({"job_id":"job_a","max_bytes":1024,"wait_ms":5000}),
                    ),
                )
            })
        };
        entered.recv_timeout(Duration::from_secs(5)).unwrap();
        // Takeover while the runner read is in flight. The runner control
        // call is served on its own connection, so this completes.
        takeover(&runtime, "M1").unwrap();
        release.send(()).unwrap();
        let late = reader.join().unwrap();
        assert_eq!(late["error"]["code"], "HUMAN_CONTROL", "{late}");
        assert!(late.get("output").is_none(), "no bytes after takeover");

        // Return + a fresh grant does not reopen the pre-takeover job.
        return_control(&runtime, "M1").unwrap();
        runtime
            .write(|doc| {
                doc.sessions[0].output_shared = true;
                Ok(())
            })
            .unwrap();
        let reopened = read();
        assert_eq!(reopened["error"]["code"], "PERMISSION_DENIED", "{reopened}");
        let inspect = route(
            &runtime,
            request("deck_session_inspect", json!({"session_id":"mcp_a"})),
        );
        assert!(inspect.get("terminalContext").is_none());
        assert!(inspect.get("readiness").is_none());
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn close_plan(runtime: &Runtime) -> String {
        let close = route(
            runtime,
            request(
                "deck_session_close",
                json!({"request_id":format!("close_{}", now_ms()),"session_id":"mcp_a","expected_generation":"g_a","control_epoch":1,"holder_id":"holder_a"}),
            ),
        );
        assert_eq!(close["state"], "accepted", "{close}");
        close["operationId"].as_str().unwrap().to_owned()
    }

    #[test]
    fn a_remote_close_commits_after_admission_and_never_sticks() {
        let (runtime, runner, root) = fixture("close-a", "svc_test");
        let operation_id = close_plan(&runtime);
        assert!(session_state(&runtime).closing);
        claim(&runtime, &operation_id).unwrap();
        let admission = close_admit(&runtime, &operation_id).unwrap().admission;
        validate_close_admission_with(&runtime, Some(&admission), &["deck-mcp-test".into()])
            .unwrap();
        // After admission a side effect may have run: rejected is not allowed.
        assert!(complete(
            &runtime,
            operation_id.clone(),
            "rejected".into(),
            None,
            None
        )
        .is_err());
        complete(
            &runtime,
            operation_id.clone(),
            "committed".into(),
            None,
            None,
        )
        .unwrap();
        runtime
            .read(|doc| assert!(doc.sessions.is_empty(), "committed close removes it"))
            .unwrap();
        // A repeated completion is idempotent; a different one is refused.
        complete(
            &runtime,
            operation_id.clone(),
            "committed".into(),
            None,
            None,
        )
        .unwrap();
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();

        // Failure after admission: ambiguous, and `closing` is cleared.
        let (runtime, runner, root) = fixture("close-b", "svc_test");
        let operation_id = close_plan(&runtime);
        claim(&runtime, &operation_id).unwrap();
        close_admit(&runtime, &operation_id).unwrap();
        complete(
            &runtime,
            operation_id.clone(),
            "ambiguous".into(),
            Some("close-failed".into()),
            None,
        )
        .unwrap();
        assert!(!session_state(&runtime).closing, "closing never sticks");
        let view = route(
            &runtime,
            request("deck_operation_get", json!({"operation_id":operation_id})),
        );
        assert_eq!(view["state"], "ambiguous");
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restart_turns_pending_board_operations_ambiguous() {
        let root = test_root("restart-board");
        let path = root.join("mcp.json");
        let runner = FakeRunner::start(&root, "g_a");
        let mut doc = DiskDoc::default();
        doc.config.enabled = true;
        doc.config.clients.push(client_record(&root));
        let mut session = session_record(&root, &runner);
        session.closing = true;
        doc.sessions.push(session);
        for (id, kind, state) in [
            ("op_create", "session-create", "accepted"),
            ("op_close", "session-close", "admitted"),
            ("op_exec", "session-create", "executing"),
        ] {
            let mut item = operation(kind, state);
            item.operation_id = id.into();
            item.request_id = format!("request_{id}");
            item.result = Some(json!({"sessionId":"mcp_a"}));
            doc.operations.push(item);
        }
        save(&path, &doc).unwrap();
        let recovered = load(&path).unwrap();
        for item in &recovered.operations {
            assert_eq!(item.state, "ambiguous", "{}", item.operation_id);
            assert_eq!(item.code.as_deref(), Some("deck-restarted"));
        }
        assert!(!recovered.sessions[0].closing);
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_control_replay_never_resends_a_runner_side_effect() {
        let (runtime, runner, root) = fixture("control-replay", "svc_test");
        runtime
            .write(|doc| {
                let session = &mut doc.sessions[0];
                session.control_owner = None;
                session.control_holder = None;
                session.lease_expires_at = None;
                Ok(())
            })
            .unwrap();
        let control =
            |arguments: Value| route(&runtime, request("deck_session_control", arguments));
        let first = control(
            json!({"request_id":"req_a","session_id":"mcp_a","expected_generation":"g_a","action":"request","holder_id":"holder_a","control_sequence":0}),
        );
        assert_eq!(first["state"], "committed", "{first}");
        let epoch = first["result"]["controlEpoch"].as_u64().unwrap();
        let release = json!({"request_id":"rel_a","session_id":"mcp_a","expected_generation":"g_a","action":"release","holder_id":"holder_a","control_epoch":epoch,"control_sequence":1});
        assert_eq!(control(release.clone())["state"], "committed");
        let taken = control(
            json!({"request_id":"req_b","session_id":"mcp_a","expected_generation":"g_a","action":"request","holder_id":"holder_b","control_sequence":2}),
        );
        assert_eq!(taken["state"], "committed");
        let controls = runner.count("control");
        // Replaying A's old release must not fence B's runner context.
        // A's release was superseded by B's request: it is refused as stale
        // (its sequence is no longer current), never applied again.
        let replay = control(release);
        assert_eq!(replay["error"]["code"], "STALE_REQUEST", "{replay}");
        assert_eq!(
            runner.count("control"),
            controls,
            "replay sent a runner control"
        );
        assert_eq!(
            session_state(&runtime).control_holder.as_deref(),
            Some("holder_b")
        );
        // null and omitted are the same request, not a conflict.
        let with_null = control(
            json!({"request_id":"req_b","session_id":"mcp_a","expected_generation":"g_a","action":"request","holder_id":"holder_b","lease_ms":null,"control_epoch":null,"control_sequence":2}),
        );
        assert_eq!(
            with_null["operationId"], taken["operationId"],
            "{with_null}"
        );
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Grant a fresh window directly (the local UI path) for route tests.
    fn grant_window(runtime: &Runtime) {
        super::execution_grant(runtime, "mcp_a".into(), Some(60_000), true, true).unwrap();
    }

    fn control_request(runtime: &Runtime, request_id: &str) -> u64 {
        let value = route(
            runtime,
            request(
                "deck_session_control",
                json!({"request_id":request_id,"session_id":"mcp_a","expected_generation":"g_a","action":"request","holder_id":"holder_a","control_sequence":session_state(runtime).control_sequence}),
            ),
        );
        assert_eq!(value["state"], "committed", "{value}");
        value["result"]["controlEpoch"].as_u64().unwrap()
    }

    fn release(runtime: &Runtime, request_id: &str, epoch: u64) {
        let value = route(
            runtime,
            request(
                "deck_session_control",
                json!({"request_id":request_id,"session_id":"mcp_a","expected_generation":"g_a","action":"release","holder_id":"holder_a","control_epoch":epoch,"control_sequence":session_state(runtime).control_sequence}),
            ),
        );
        assert_eq!(value["state"], "committed", "{value}");
    }

    fn exec_request(request_id: &str, epoch: u64) -> WireRequest {
        request(
            "deck_exec",
            json!({"request_id":request_id,"session_id":"mcp_a","expected_generation":"g_a","control_epoch":epoch,"holder_id":"holder_a","executable":"/usr/bin/true","args":[],"wait_ms":0}),
        )
    }

    fn unowned(runtime: &Runtime) {
        runtime
            .write(|doc| {
                let session = &mut doc.sessions[0];
                session.control_owner = None;
                session.control_holder = None;
                session.lease_expires_at = None;
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn compaction_keeps_three_thousand_epochs_bounded() {
        // Pure ledger model of 3000 request/exec/release cycles: each cycle
        // journals three records bound to its epoch.
        let root = test_root("compact-model");
        let runner = FakeRunner::start(&root, "g_a");
        let mut doc = DiskDoc::default();
        doc.sessions.push(session_record(&root, &runner));
        for cycle in 0..3_000u64 {
            let epoch = cycle * 2 + 1;
            doc.sessions[0].control_epoch = epoch;
            for kind in ["session-control", "exec", "session-control"] {
                let mut record = operation(kind, "committed");
                record.operation_id = format!("op_{}", doc.operations.len() + cycle as usize * 3);
                record.request_id = format!("{kind}_{cycle}_{}", doc.operations.len());
                record.session_id = Some("mcp_a".into());
                record.control_epoch = Some(epoch);
                doc.operations.push(record);
            }
            doc.sessions[0].control_epoch = epoch + 1;
            compact(&mut doc, Some("svc_test"));
            assert!(
                doc.operations.len() <= 3,
                "cycle {cycle}: {}",
                doc.operations.len()
            );
        }
        validate_doc(&doc).unwrap();
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sustained_use_stays_within_the_journal() {
        // End-to-end through the routes. 700 cycles journal 2100 records —
        // past the pre-fix global limit of 2000 (it failed at cycle 667).
        // Each debug-build write serializes the audit-capped document, so the
        // 3000-cycle bound is covered by the pure model test above.
        let (runtime, runner, root) = fixture("sustained", "svc_test");
        unowned(&runtime);
        super::execution_grant(
            &runtime,
            "mcp_a".into(),
            Some(MAX_EXECUTION_GRANT_MS),
            true,
            true,
        )
        .unwrap();
        for cycle in 0..700 {
            let epoch = control_request(&runtime, &format!("req_{cycle}"));
            let executed = route(&runtime, exec_request(&format!("exec_{cycle}"), epoch));
            assert_eq!(executed["state"], "exited", "cycle {cycle}: {executed}");
            release(&runtime, &format!("rel_{cycle}"), epoch);
        }
        runtime
            .read(|doc| {
                assert!(
                    doc.operations.len() < 16,
                    "journal grew: {}",
                    doc.operations.len()
                );
                assert!(doc.jobs.len() <= MAX_JOBS_PER_SESSION);
                assert!(doc.execution_grants.len() <= 2);
            })
            .unwrap();
        assert_eq!(runner.count("exec"), 700);
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn many_grants_never_block_a_takeover() {
        let (runtime, runner, root) = fixture("grants", "svc_test");
        for _ in 0..2_001 {
            grant_window(&runtime);
        }
        assert!(runtime.read(|doc| doc.execution_grants.len()).unwrap() <= 2);
        takeover(&runtime, "M1").unwrap();
        assert!(session_state(&runtime).human_lock, "takeover persisted");
        assert_eq!(runner.last("control").unwrap()["mode"], "human");
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_retired_request_id_is_rejected_never_reexecuted() {
        let (runtime, runner, root) = fixture("retired", "svc_test");
        unowned(&runtime);
        grant_window(&runtime);
        let epoch = control_request(&runtime, "req_old");
        let first = route(&runtime, exec_request("exec_once", epoch));
        assert_eq!(first["state"], "exited");
        // Exact replay inside the window: same job, no second dispatch.
        let again = route(&runtime, exec_request("exec_once", epoch));
        assert_eq!(again["jobId"], first["jobId"]);
        assert_eq!(runner.count("exec"), 1);
        release(&runtime, "rel_old", epoch);
        let next = control_request(&runtime, "req_new");
        // Force retirement, then replay the old request id verbatim.
        runtime
            .write(|doc| {
                compact(doc, Some("svc_test"));
                assert!(doc
                    .operations
                    .iter()
                    .all(|operation| operation.request_id != "exec_once"));
                Ok(())
            })
            .unwrap();
        let replay = route(&runtime, exec_request("exec_once", epoch));
        assert_eq!(replay["error"]["code"], "CONTROL_REVOKED", "{replay}");
        assert_eq!(runner.count("exec"), 1, "a retired request was re-executed");
        assert!(next > epoch);
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_full_journal_still_admits_an_interrupt_and_names_a_real_recovery() {
        let (runtime, runner, root) = fixture("reserve", "svc_test");
        grant_window(&runtime);
        let executed = route(&runtime, exec_request("exec_live", 1));
        let job_id = executed["jobId"].as_str().unwrap().to_owned();
        runtime
            .write(|doc| {
                while doc.operations.len() < ORDINARY_OPERATIONS {
                    let mut filler = operation("exec", "committed");
                    filler.operation_id = format!("op_fill_{}", doc.operations.len());
                    filler.request_id = format!("fill_{}", doc.operations.len());
                    filler.client_id = format!("client_fill_{}", doc.operations.len() % 8);
                    filler.session_id = Some("mcp_a".into());
                    filler.control_epoch = Some(1);
                    doc.operations.push(filler);
                }
                Ok(())
            })
            .unwrap();
        let blocked = route(&runtime, exec_request("exec_full", 1));
        assert_eq!(blocked["error"]["code"], "CAPACITY_EXCEEDED");
        assert!(blocked["error"]["nextAction"]
            .as_str()
            .unwrap()
            .contains("Release control"));
        let interrupt = route(
            &runtime,
            request(
                "deck_job_interrupt",
                json!({"request_id":"stop_now","job_id":job_id,"session_generation":"g_a","control_epoch":1,"holder_id":"holder_a"}),
            ),
        );
        assert_eq!(interrupt["state"], "committed", "{interrupt}");
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn v3_state_upgrades_stickily_and_future_state_is_refused() {
        let root = test_root("v4");
        let path = root.join("mcp.json");
        let mut doc = DiskDoc {
            version: 3,
            ..DiskDoc::default()
        };
        doc.config.clients.push(client_record(&root));
        let mut old = operation("session-control", "committed");
        old.result = Some(json!({"sessionId":"mcp_gone"}));
        doc.operations.push(old);
        // A v3 file never contains the v4 fields.
        let mut bytes = serde_json::to_value(&doc).unwrap();
        for operation in bytes["operations"].as_array_mut().unwrap() {
            let fields = operation.as_object_mut().unwrap();
            fields.remove("sessionId");
            fields.remove("controlEpoch");
        }
        std::fs::write(&path, serde_json::to_vec(&bytes).unwrap()).unwrap();
        let upgraded = load(&path).unwrap();
        assert_eq!(upgraded.version, STATE_VERSION);
        assert!(
            upgraded.operations.is_empty(),
            "a v3 record of a closed session is retired"
        );
        let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            saved["version"], STATE_VERSION,
            "the upgrade is written back (sticky)"
        );
        // The v3 build accepted exactly version 3; the same rule refuses a
        // future schema here.
        let mut future = saved.clone();
        future["version"] = json!(STATE_VERSION + 1);
        std::fs::write(&path, serde_json::to_vec(&future).unwrap()).unwrap();
        assert_eq!(load(&path).err().unwrap().kind(), ErrorKind::Recovery);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            serde_json::to_vec(&future).unwrap(),
            "refused state is left untouched"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_allow_shell_is_ignored_and_not_reserialized() {
        let root = test_root("v5-shell-grant");
        let runner = FakeRunner::start(&root, "g_a");
        let session = session_record(&root, &runner);
        let mut value = serde_json::to_value(execution_grant(&session)).unwrap();
        value["allowShell"] = json!(true);
        let migrated: ExecutionGrant = serde_json::from_value(value).unwrap();
        assert_eq!(migrated._legacy_allow_shell, Some(true));
        assert!(serde_json::to_value(migrated)
            .unwrap()
            .get("allowShell")
            .is_none());
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn protocol_views_are_honest_about_state_and_identity() {
        let (runtime, runner, root) = fixture("views", "svc_test");
        let capabilities = route(&runtime, request("deck_capabilities", json!({})));
        assert_eq!(capabilities["deckVersion"], DECK_VERSION);
        assert_eq!(capabilities["stateSchemaVersion"], STATE_VERSION);
        assert_eq!(capabilities["executionMode"], "structured-direct-default");
        assert_eq!(capabilities["directExecution"]["arbitraryPrograms"], true);
        assert!(capabilities.get("shellFallback").is_none());
        assert!(capabilities.get("featureEnabled").is_none());
        let create = route(
            &runtime,
            request(
                "deck_session_create",
                json!({"request_id":"create_view","project_id":"P1","cwd":root.display().to_string(),"create_sequence":0}),
            ),
        );
        assert!(create.get("shellJobComplete").is_none());
        assert!(create["result"].get("runnerSocket").is_none());
        assert!(create["result"].get("sessionId").is_some());
        let listed = route(&runtime, request("deck_sessions_list", json!({})));
        assert!(listed["sessions"][0].get("readiness").is_none());
        runtime
            .write(|doc| {
                doc.config.enabled = false;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            route(&runtime, request("deck_capabilities", json!({})))["error"]["code"],
            "FEATURE_DISABLED"
        );
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    // ---- F1: execution revocation reaches the final admission ----

    fn wait_until(mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready() {
            assert!(Instant::now() < deadline, "condition never became true");
            std::thread::yield_now();
        }
    }

    fn execution_fenced(runtime: &Runtime, session_id: &str) -> bool {
        runtime
            .emergency
            .lock_or_recover()
            .execution_fenced(session_id)
    }

    fn operation_by_request(runtime: &Runtime, request_id: &str) -> Option<Operation> {
        runtime
            .read(|doc| {
                doc.operations
                    .iter()
                    .find(|operation| operation.request_id == request_id)
                    .cloned()
            })
            .unwrap()
    }

    fn input_request(request_id: &str, job_id: &str) -> WireRequest {
        request(
            "deck_job_input",
            json!({"request_id":request_id,"job_id":job_id,"session_generation":"g_a","control_epoch":1,"holder_id":"holder_a","input":"y\n"}),
        )
    }

    fn interrupt_request(request_id: &str, job_id: &str) -> WireRequest {
        request(
            "deck_job_interrupt",
            json!({"request_id":request_id,"job_id":job_id,"session_generation":"g_a","control_epoch":1,"holder_id":"holder_a"}),
        )
    }

    #[test]
    fn f1_exec_past_route_is_stopped_by_a_pending_execution_revoke() {
        let (runtime, runner, root) = fixture("f1-exec", "svc_test");
        grant_window(&runtime);
        // Baseline: the same production path dispatches when nothing revokes.
        let baseline = route(&runtime, exec_request("exec_base", 1));
        assert_eq!(baseline["state"], "exited", "{baseline}");
        assert_eq!(runner.count("exec"), 1);
        // The request passes route, persists its intent and parks right
        // before its final admission.
        let (entered, release) = pause::arm(&runtime.path, "exec-final");
        let racing = runtime.clone();
        let exec_thread = std::thread::spawn(move || route(&racing, exec_request("exec_raced", 1)));
        entered.recv_timeout(Duration::from_secs(10)).unwrap();
        // The local user revokes: the fence is set, persistence waits for the
        // delivery lock the parked request holds.
        let revoking = runtime.clone();
        let revoke_thread = std::thread::spawn(move || execution_revoke(&revoking, "M1"));
        wait_until(|| execution_fenced(&runtime, "mcp_a"));
        release.send(()).unwrap();
        let raced = exec_thread.join().unwrap();
        revoke_thread.join().unwrap().unwrap();
        assert_eq!(
            runner.count("exec"),
            1,
            "an exec admitted after the revocation fence reached the runner: {raced}"
        );
        assert_eq!(
            raced["error"]["code"], "EXECUTION_GRANT_REQUIRED",
            "{raced}"
        );
        let record = operation_by_request(&runtime, "exec_raced").unwrap();
        assert_eq!(record.state, "rejected");
        let job_id = record.result.unwrap()["jobId"].as_str().unwrap().to_owned();
        runtime
            .read(|doc| assert!(doc.jobs.iter().all(|job| job.job_id != job_id)))
            .unwrap();
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn f1_stdin_past_route_delivers_no_bytes_after_a_pending_execution_revoke() {
        let (runtime, runner, root) = fixture("f1-input", "svc_test");
        grant_window(&runtime);
        let executed = route(&runtime, exec_request("exec_job", 1));
        let job_id = executed["jobId"].as_str().unwrap().to_owned();
        let baseline = route(&runtime, input_request("input_base", &job_id));
        assert_eq!(baseline["state"], "committed", "{baseline}");
        assert_eq!(runner.count("input"), 1);
        let (entered, release) = pause::arm(&runtime.path, "side-effect-final");
        let racing = runtime.clone();
        let racing_job = job_id.clone();
        let input_thread =
            std::thread::spawn(move || route(&racing, input_request("input_raced", &racing_job)));
        entered.recv_timeout(Duration::from_secs(10)).unwrap();
        let revoking = runtime.clone();
        let revoke_thread = std::thread::spawn(move || execution_revoke(&revoking, "M1"));
        wait_until(|| execution_fenced(&runtime, "mcp_a"));
        release.send(()).unwrap();
        let raced = input_thread.join().unwrap();
        revoke_thread.join().unwrap().unwrap();
        assert_eq!(
            runner.count("input"),
            1,
            "stdin bytes reached the runner after the revocation fence: {raced}"
        );
        assert_eq!(
            raced["error"]["code"], "EXECUTION_GRANT_REQUIRED",
            "{raced}"
        );
        assert_eq!(
            operation_by_request(&runtime, "input_raced").unwrap().state,
            "rejected"
        );
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn f1_close_admission_rechecks_takeover_revocation_and_disable() {
        for fence in ["takeover", "client-revoke", "disable"] {
            let (runtime, runner, root) = fixture(&format!("f1-close-{fence}"), "svc_test");
            let operation_id = close_plan(&runtime);
            claim(&runtime, &operation_id).unwrap();
            let (entered, release) = pause::arm(&runtime.path, "close-admit");
            let admitting = runtime.clone();
            let admitted_id = operation_id.clone();
            let admit_thread =
                std::thread::spawn(move || close_admit(&admitting, &admitted_id).map(|_| ()));
            entered.recv_timeout(Duration::from_secs(10)).unwrap();
            let fencing = runtime.clone();
            let fence_thread = std::thread::spawn(move || match fence {
                "takeover" => takeover(&fencing, "M1"),
                "client-revoke" => client_revoke(&fencing, "client_a".into()),
                _ => disable(&fencing),
            });
            wait_until(|| {
                let emergency = runtime.emergency.lock_or_recover();
                emergency.disabled
                    || emergency.clients.contains("client_a")
                    || emergency.human_sessions.contains("mcp_a")
            });
            release.send(()).unwrap();
            let admitted = admit_thread.join().unwrap();
            fence_thread.join().unwrap().unwrap();
            assert!(admitted.is_err(), "{fence}: close admitted across a fence");
            let record = runtime
                .read(|doc| {
                    doc.operations
                        .iter()
                        .find(|operation| operation.operation_id == operation_id)
                        .cloned()
                })
                .unwrap()
                .unwrap();
            assert!(
                !matches!(record.state.as_str(), "admitted" | "committed"),
                "{fence}: {}",
                record.state
            );
            // The Board reports the refusal; the card and session remain.
            complete(
                &runtime,
                operation_id.clone(),
                "rejected".into(),
                None,
                None,
            )
            .unwrap();
            let session = session_state(&runtime);
            assert_eq!(session.session_id, "mcp_a", "{fence}: session was removed");
            assert!(!session.closing, "{fence}: closing stuck");
            drop(runner);
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn f1_revocation_after_the_commit_point_is_ordered_after_the_dispatch() {
        let (runtime, runner, root) = fixture("f1-inflight", "svc_test");
        grant_window(&runtime);
        // The exec is at the runner: it crossed the admission boundary.
        let (entered, release) = runner.hold("exec");
        let racing = runtime.clone();
        let exec_thread =
            std::thread::spawn(move || route(&racing, exec_request("exec_inflight", 1)));
        entered.recv_timeout(Duration::from_secs(10)).unwrap();
        let revoking = runtime.clone();
        let revoke_thread = std::thread::spawn(move || execution_revoke(&revoking, "M1"));
        wait_until(|| execution_fenced(&runtime, "mcp_a"));
        release.send(()).unwrap();
        let inflight = exec_thread.join().unwrap();
        revoke_thread.join().unwrap().unwrap();
        assert_eq!(inflight["state"], "exited", "{inflight}");
        assert_eq!(runner.count("exec"), 1);
        assert_eq!(
            operation_by_request(&runtime, "exec_inflight")
                .unwrap()
                .state,
            "committed"
        );
        // The revocation is linearized after the dispatch it waited for.
        runtime
            .read(|doc| {
                let position = |kind: &str| {
                    doc.audit
                        .iter()
                        .position(|event| event.kind == kind)
                        .unwrap()
                };
                assert!(position("exec-dispatch") < position("grant-revoked"));
            })
            .unwrap();
        let later = route(&runtime, exec_request("exec_after", 1));
        assert_eq!(
            later["error"]["code"], "EXECUTION_GRANT_REQUIRED",
            "{later}"
        );
        assert_eq!(runner.count("exec"), 1);
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn f1_a_pending_execution_revoke_leaves_reads_and_interrupt_alone() {
        let (runtime, runner, root) = fixture("f1-read", "svc_test");
        grant_window(&runtime);
        let executed = route(&runtime, exec_request("exec_job", 1));
        let job_id = executed["jobId"].as_str().unwrap().to_owned();
        let (entered, release) = pause::arm(&runtime.path, "revoke-fenced");
        let revoking = runtime.clone();
        let revoke_thread = std::thread::spawn(move || execution_revoke(&revoking, "M1"));
        entered.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(execution_fenced(&runtime, "mcp_a"));
        let read = route(
            &runtime,
            request("deck_job_read", json!({"job_id":job_id,"max_bytes":64})),
        );
        assert_eq!(read["ok"], true, "{read}");
        let interrupt = route(&runtime, interrupt_request("stop_job", &job_id));
        assert_eq!(interrupt["state"], "committed", "{interrupt}");
        assert_eq!(runner.count("interrupt"), 1);
        let exec = route(&runtime, exec_request("exec_blocked", 1));
        assert_eq!(exec["error"]["code"], "EXECUTION_GRANT_REQUIRED", "{exec}");
        release.send(()).unwrap();
        revoke_thread.join().unwrap().unwrap();
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn f1_an_approval_never_clears_a_revocation_that_persists_after_it() {
        let (runtime, runner, root) = fixture("f1-approve", "svc_test");
        grant_window(&runtime);
        let (entered, release) = pause::arm(&runtime.path, "revoke-fenced");
        let revoking = runtime.clone();
        let revoke_thread = std::thread::spawn(move || execution_revoke(&revoking, "M1"));
        entered.recv_timeout(Duration::from_secs(10)).unwrap();
        // An approval runs to completion while that revocation is pending.
        grant_window(&runtime);
        let exec = route(&runtime, exec_request("exec_between", 1));
        assert_eq!(exec["error"]["code"], "EXECUTION_GRANT_REQUIRED", "{exec}");
        assert_eq!(
            runner.count("exec"),
            0,
            "the approval erased a pending revocation"
        );
        release.send(()).unwrap();
        revoke_thread.join().unwrap().unwrap();
        let after = route(&runtime, exec_request("exec_after", 1));
        assert_eq!(
            after["error"]["code"], "EXECUTION_GRANT_REQUIRED",
            "{after}"
        );
        assert_eq!(runner.count("exec"), 0);
        // Only a later explicit approval opens a new window.
        grant_window(&runtime);
        let fresh = route(&runtime, exec_request("exec_fresh", 1));
        assert_eq!(fresh["state"], "exited", "{fresh}");
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn inspect_view(runtime: &Runtime) -> Value {
        let value = route(
            runtime,
            request(
                "deck_session_inspect",
                json!({"session_id":"mcp_a","holder_id":"holder_a"}),
            ),
        );
        assert_eq!(value["ok"], true, "{value}");
        value
    }

    #[test]
    fn inspect_reports_a_pending_execution_revoke_as_revoked() {
        let (runtime, runner, root) = fixture("inspect-fence", "svc_test");
        grant_window(&runtime);
        let executed = route(&runtime, exec_request("exec_job", 1));
        let job_id = executed["jobId"].as_str().unwrap().to_owned();
        let before = inspect_view(&runtime);
        assert_eq!(before["executionAuthorization"]["active"], true, "{before}");

        // C1/C2: the fence is set, the grant revocation is not persisted yet.
        let (entered, release) = pause::arm(&runtime.path, "revoke-fenced");
        let revoking = runtime.clone();
        let revoke_thread = std::thread::spawn(move || execution_revoke(&revoking, "M1"));
        entered.recv_timeout(Duration::from_secs(10)).unwrap();
        runtime
            .read(|doc| assert!(doc.execution_grants[0].revoked_at.is_none()))
            .unwrap();
        let pending = inspect_view(&runtime);
        let authorization = &pending["executionAuthorization"];
        assert_eq!(authorization["status"], "revoked", "{pending}");
        assert_eq!(authorization["active"], false);
        assert_eq!(authorization["stdinApprovedForActiveGrant"], false);
        assert_eq!(
            authorization["expiresAtUnixMs"],
            before["executionAuthorization"]["expiresAtUnixMs"]
        );
        assert_eq!(
            pending["mayStartNextJobReason"], "EXECUTION_GRANT_REQUIRED",
            "{pending}"
        );
        assert_eq!(pending["outputSharing"], before["outputSharing"]);
        assert_eq!(pending["activeJob"], before["activeJob"]);
        assert_eq!(pending["foreground"], before["foreground"]);
        let exec = route(&runtime, exec_request("exec_pending", 1));
        assert_eq!(exec["error"]["code"], "EXECUTION_GRANT_REQUIRED", "{exec}");
        let input = route(&runtime, input_request("input_pending", &job_id));
        assert_eq!(
            input["error"]["code"], "EXECUTION_GRANT_REQUIRED",
            "{input}"
        );
        assert_eq!(runner.count("interrupt"), 0, "nothing was interrupted");

        // C3: persisted, fence lowered — still revoked, never active again.
        release.send(()).unwrap();
        revoke_thread.join().unwrap().unwrap();
        assert!(!execution_fenced(&runtime, "mcp_a"));
        let persisted = inspect_view(&runtime);
        assert_eq!(persisted["executionAuthorization"]["status"], "revoked");
        assert_eq!(persisted["executionAuthorization"]["active"], false);
        assert_eq!(persisted["outputSharing"], before["outputSharing"]);
        assert_eq!(persisted["outputSharing"]["sessionGateOpen"], true);

        // C4: a later local approval is a new, active grant.
        grant_window(&runtime);
        let renewed = inspect_view(&runtime);
        assert_eq!(renewed["executionAuthorization"]["status"], "active");
        assert_eq!(
            renewed["executionAuthorization"]["stdinApprovedForActiveGrant"],
            true
        );
        let fresh = route(&runtime, exec_request("exec_fresh", 1));
        assert_eq!(fresh["state"], "exited", "{fresh}");

        // C5: a naturally expired grant stays `expired` under a pending fence.
        runtime
            .write(|doc| {
                for grant in &mut doc.execution_grants {
                    grant.expires_at = grant.issued_at;
                }
                Ok(())
            })
            .unwrap();
        let (entered, release) = pause::arm(&runtime.path, "revoke-fenced");
        let revoking = runtime.clone();
        let revoke_thread = std::thread::spawn(move || execution_revoke(&revoking, "M1"));
        entered.recv_timeout(Duration::from_secs(10)).unwrap();
        let expired = inspect_view(&runtime);
        assert_eq!(expired["executionAuthorization"]["status"], "expired");
        assert_eq!(expired["executionAuthorization"]["active"], false);
        release.send(()).unwrap();
        revoke_thread.join().unwrap().unwrap();
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn read_job(runtime: &Runtime, job_id: &str) -> Value {
        route(
            runtime,
            request("deck_job_read", json!({"job_id":job_id,"max_bytes":1024})),
        )
    }

    #[test]
    fn execution_revoke_leaves_output_sharing_and_a_running_job_alone() {
        let (runtime, runner, root) = fixture("revoke-sharing", "svc_test");
        grant_window(&runtime);
        let executed = route(&runtime, exec_request("exec_job", 1));
        let job_id = executed["jobId"].as_str().unwrap().to_owned();
        // The job is running at the runner, and stdin reaches it: the later
        // denial is the revocation, not JOB_NOT_RUNNING.
        runner.busy.store(true, Ordering::SeqCst);
        let baseline = route(&runtime, input_request("input_base", &job_id));
        assert_eq!(baseline["state"], "committed", "{baseline}");
        let before = inspect_view(&runtime);
        assert_eq!(before["executionAuthorization"]["active"], true);
        assert_eq!(before["outputSharing"]["sessionGateOpen"], true);
        assert_eq!(before["activeJob"]["state"], "running", "{before}");
        let (exec_count, input_count) = (runner.count("exec"), runner.count("input"));

        execution_revoke(&runtime, "M1").unwrap();

        let after = inspect_view(&runtime);
        let authorization = &after["executionAuthorization"];
        assert_eq!(authorization["status"], "revoked", "{after}");
        assert_eq!(authorization["active"], false);
        assert_eq!(authorization["stdinApprovedForActiveGrant"], false);
        assert_eq!(after["outputSharing"], before["outputSharing"], "{after}");
        assert!(session_state(&runtime).output_shared);
        // A1: the job's output is read through its binding, not the grant.
        runtime
            .read(|doc| {
                let binding = doc.jobs.iter().find(|job| job.job_id == job_id).unwrap();
                let grant = doc
                    .execution_grants
                    .iter()
                    .find(|grant| grant.grant_id == binding.grant_id)
                    .unwrap();
                assert!(grant.revoked_at.is_some(), "its grant is revoked");
            })
            .unwrap();
        let read = read_job(&runtime, &job_id);
        assert_eq!(read["ok"], true, "{read}");
        assert_eq!(read["output"], "done\n", "{read}");
        assert_eq!(read["gap"], false);
        assert_eq!(read["droppedBytes"], 0);
        // A2/A3: sharing grants no execution authority.
        let exec = route(&runtime, exec_request("exec_after", 1));
        assert_eq!(exec["error"]["code"], "EXECUTION_GRANT_REQUIRED", "{exec}");
        // Once persisted, stdin is refused by the job's own (revoked) grant.
        let input = route(&runtime, input_request("input_after", &job_id));
        assert_eq!(input["error"]["code"], "STDIN_NOT_AUTHORIZED", "{input}");
        assert_eq!(runner.count("exec"), exec_count);
        assert_eq!(runner.count("input"), input_count);
        // A4: revocation is not an interrupt.
        assert_eq!(runner.count("interrupt"), 0);
        assert_eq!(runner.count("stop"), 0);
        assert_eq!(after["activeJob"], before["activeJob"]);
        assert_eq!(after["foreground"], "managed-job");
        runner.busy.store(false, Ordering::SeqCst);
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn execution_revoke_never_opens_output_sharing() {
        let (runtime, runner, root) = fixture("revoke-unshared", "svc_test");
        super::execution_grant(&runtime, "mcp_a".into(), Some(60_000), true, false).unwrap();
        let executed = route(&runtime, exec_request("exec_job", 1));
        let job_id = executed["jobId"].as_str().unwrap().to_owned();
        assert_eq!(
            inspect_view(&runtime)["outputSharing"]["sessionGateOpen"],
            false
        );
        execution_revoke(&runtime, "M1").unwrap();
        let after = inspect_view(&runtime);
        assert_eq!(after["executionAuthorization"]["status"], "revoked");
        assert_eq!(after["outputSharing"]["sessionGateOpen"], false, "{after}");
        assert!(!session_state(&runtime).output_shared);
        let read = read_job(&runtime, &job_id);
        assert_eq!(read["error"]["code"], "PERMISSION_DENIED", "{read}");
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn f1_a_job_bound_to_a_revoked_grant_never_revives_under_a_new_grant() {
        let (runtime, runner, root) = fixture("f1-revive", "svc_test");
        grant_window(&runtime);
        let executed = route(&runtime, exec_request("exec_old", 1));
        let job_id = executed["jobId"].as_str().unwrap().to_owned();
        execution_revoke(&runtime, "M1").unwrap();
        grant_window(&runtime);
        let input = route(&runtime, input_request("input_old_grant", &job_id));
        assert_eq!(input["ok"], false, "{input}");
        assert_eq!(runner.count("input"), 0);
        // The replayed exec returns its record and is never re-dispatched.
        let replay = route(&runtime, exec_request("exec_old", 1));
        assert_eq!(replay["jobId"], executed["jobId"], "{replay}");
        assert_eq!(runner.count("exec"), 1);
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    // ---- F2 / C1 / F3: request identity, renew replay, capacity recovery ----

    fn control(
        runtime: &Runtime,
        request_id: &str,
        action: &str,
        epoch: Option<u64>,
        sequence: u64,
    ) -> Value {
        let mut arguments = json!({"request_id":request_id,"session_id":"mcp_a","expected_generation":"g_a","action":action,"holder_id":"holder_a","control_sequence":sequence});
        if let Some(epoch) = epoch {
            arguments["control_epoch"] = json!(epoch);
        }
        route(runtime, request("deck_session_control", arguments))
    }

    fn create(
        runtime: &Runtime,
        root: &Path,
        request_id: &str,
        sequence: u64,
        title: &str,
    ) -> Value {
        route(
            runtime,
            request(
                "deck_session_create",
                json!({"request_id":request_id,"project_id":"P1","cwd":root.display().to_string(),"title":title,"create_sequence":sequence}),
            ),
        )
    }

    /// A new Deck process over the same state file (restart).
    fn reload(runtime: &Runtime, service: &str) -> Arc<Runtime> {
        Arc::new(Runtime {
            app: None,
            path: runtime.path.clone(),
            socket: runtime.socket.clone(),
            doc: Mutex::new(load(&runtime.path)),
            io: Mutex::new(()),
            delivery: Mutex::new(()),
            emergency: Mutex::new(EmergencyFences::default()),
            service_instance: service.into(),
            runner_auth: Mutex::new(HashMap::new()),
            started: Instant::now(),
        })
    }

    /// Terminal exec records of `client_id`, bound to `session_id`/`epoch`.
    fn fill(
        runtime: &Runtime,
        client_id: &str,
        session_id: &str,
        epoch: u64,
        count: usize,
        tag: &str,
    ) {
        runtime
            .write(|doc| {
                for index in 0..count {
                    let mut filler = operation("exec", "committed");
                    filler.operation_id = format!("op_{tag}_{index}");
                    filler.request_id = format!("{tag}_{index}");
                    filler.client_id = client_id.into();
                    filler.session_id = Some(session_id.into());
                    filler.control_epoch = Some(epoch);
                    doc.operations.push(filler);
                }
                Ok(())
            })
            .unwrap();
    }

    fn ordinary_count(runtime: &Runtime) -> usize {
        runtime
            .read(|doc| {
                doc.operations
                    .iter()
                    .filter(|op| !is_control_record(op))
                    .count()
            })
            .unwrap()
    }

    fn own_count(runtime: &Runtime, client_id: &str) -> usize {
        runtime
            .read(|doc| {
                doc.operations
                    .iter()
                    .filter(|op| op.client_id == client_id && !is_control_record(op))
                    .count()
            })
            .unwrap()
    }

    #[test]
    fn f2_a_retired_control_request_is_never_applied_again() {
        let (runtime, runner, root) = fixture("f2-control", "svc_test");
        unowned(&runtime);
        let first = control(&runtime, "req_first", "request", None, 0);
        assert_eq!(first["state"], "committed", "{first}");
        let epoch = first["result"]["controlEpoch"].as_u64().unwrap();
        let released = control(&runtime, "rel_first", "release", Some(epoch), 1);
        assert_eq!(released["state"], "committed", "{released}");
        // The first request's record is retired by the release.
        assert!(operation_by_request(&runtime, "req_first").is_none());
        let before = session_state(&runtime);
        assert!(!before.human_lock && before.control_owner.is_none());
        let controls = runner.count("control");
        let replay = control(&runtime, "req_first", "request", None, 0);
        assert_eq!(replay["error"]["code"], "STALE_REQUEST", "{replay}");
        let after = session_state(&runtime);
        assert_eq!(after.control_owner, None);
        assert_eq!(after.control_holder, None);
        assert_eq!(after.lease_expires_at, None);
        assert_eq!(after.control_epoch, before.control_epoch);
        assert_eq!(after.control_sequence, before.control_sequence);
        assert_eq!(
            runner.count("control"),
            controls,
            "a replay reached the runner"
        );
        // Same after a local takeover and an explicit return.
        takeover(&runtime, "M1").unwrap();
        return_control(&runtime, "M1").unwrap();
        let controls = runner.count("control");
        let replay = control(&runtime, "req_first", "request", None, 0);
        assert_eq!(replay["error"]["code"], "STALE_REQUEST", "{replay}");
        assert_eq!(session_state(&runtime).control_owner, None);
        assert_eq!(runner.count("control"), controls);
        // Same after a restart.
        let reloaded = reload(&runtime, "svc_reloaded");
        let replay = control(&reloaded, "req_first", "request", None, 0);
        assert_eq!(replay["error"]["code"], "STALE_REQUEST", "{replay}");
        assert_eq!(session_state(&reloaded).control_owner, None);
        assert_eq!(runner.count("control"), controls);
        // New requests are never locked out.
        let sequence = session_state(&reloaded).control_sequence;
        let fresh = control(&reloaded, "req_fresh", "request", None, sequence);
        assert_eq!(fresh["state"], "committed", "{fresh}");
        // Same identity, different arguments: a conflict while the record is
        // kept; once superseded, stale (never re-accepted, never compared).
        let conflict = control(&reloaded, "req_fresh", "request", None, sequence + 7);
        assert_eq!(
            conflict["error"]["code"], "REQUEST_ID_CONFLICT",
            "{conflict}"
        );
        let moved = control(
            &reloaded,
            "req_moved",
            "renew",
            Some(fresh["result"]["controlEpoch"].as_u64().unwrap()),
            sequence + 1,
        );
        assert_eq!(moved["state"], "committed", "{moved}");
        let stale = control(&reloaded, "req_fresh", "request", None, sequence + 7);
        assert_eq!(stale["error"]["code"], "STALE_REQUEST", "{stale}");
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn f2_a_retired_create_is_never_accepted_again() {
        let (runtime, runner, root) = fixture("f2-create", "svc_test");
        let first = create(&runtime, &root, "create_first", 0, "First");
        assert_eq!(first["state"], "accepted", "{first}");
        let first_id = first["operationId"].as_str().unwrap().to_owned();
        claim(&runtime, &first_id).unwrap();
        complete(
            &runtime,
            first_id,
            "committed".into(),
            None,
            Some("deck-mcp-first".into()),
        )
        .unwrap();
        // Push it out of the result window with later, finished creates.
        for index in 1..=CREATE_REPLAY_WINDOW as u64 {
            let later = create(&runtime, &root, &format!("create_{index}"), index, "Later");
            assert_eq!(later["state"], "accepted", "{later}");
            let id = later["operationId"].as_str().unwrap().to_owned();
            claim(&runtime, &id).unwrap();
            complete(
                &runtime,
                id,
                "rejected".into(),
                Some("create-failed".into()),
                None,
            )
            .unwrap();
        }
        assert!(operation_by_request(&runtime, "create_first").is_none());
        let sessions = runtime.read(|doc| doc.sessions.len()).unwrap();
        let operations = runtime.read(|doc| doc.operations.len()).unwrap();
        let pending = || {
            runtime
                .read(|doc| {
                    doc.operations
                        .iter()
                        .filter(|op| op.state == "accepted")
                        .count()
                })
                .unwrap()
        };
        let replay = create(&runtime, &root, "create_first", 0, "First");
        assert_eq!(replay["error"]["code"], "STALE_REQUEST", "{replay}");
        assert_eq!(runtime.read(|doc| doc.sessions.len()).unwrap(), sessions);
        assert_eq!(
            runtime.read(|doc| doc.operations.len()).unwrap(),
            operations
        );
        assert_eq!(pending(), 0, "a retired create was accepted again");
        // A different body under the retired identity is stale too — Deck
        // does not pretend to have compared it with deleted arguments.
        let changed = create(&runtime, &root, "create_first", 0, "Changed");
        assert_eq!(changed["error"]["code"], "STALE_REQUEST", "{changed}");
        // Inside the window the same identity with other arguments conflicts.
        let last = format!("create_{}", CREATE_REPLAY_WINDOW);
        let conflict = create(&runtime, &root, &last, CREATE_REPLAY_WINDOW as u64, "Other");
        assert_eq!(
            conflict["error"]["code"], "REQUEST_ID_CONFLICT",
            "{conflict}"
        );
        let reloaded = reload(&runtime, "svc_reloaded");
        let replay = create(&reloaded, &root, "create_first", 0, "First");
        assert_eq!(replay["error"]["code"], "STALE_REQUEST", "{replay}");
        // A new create at the current sequence still works.
        let next = CREATE_REPLAY_WINDOW as u64 + 1;
        let fresh = create(&reloaded, &root, "create_fresh", next, "Fresh");
        assert_eq!(fresh["state"], "accepted", "{fresh}");
        let listed = route(&reloaded, request("deck_sessions_list", json!({})));
        assert_eq!(listed["nextCreateSequence"], next + 1);
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn f2_an_ambiguous_exec_is_never_dispatched_again() {
        let (runtime, runner, root) = fixture("f2-unknown", "svc_test");
        grant_window(&runtime);
        // The runner vanishes: Deck cannot prove whether dispatch happened.
        drop(runner);
        let lost = route(&runtime, exec_request("exec_lost", 1));
        assert_eq!(lost["error"]["code"], "OPERATION_AMBIGUOUS", "{lost}");
        assert_eq!(
            operation_by_request(&runtime, "exec_lost").unwrap().state,
            "ambiguous"
        );
        let runner = FakeRunner::start(&root, "g_a");
        let replay = route(&runtime, exec_request("exec_lost", 1));
        assert_eq!(replay["ok"], true, "{replay}");
        assert_eq!(
            runner.count("exec"),
            0,
            "an ambiguous exec was dispatched again"
        );
        // Restart: the record's epoch is closed, the replay is refused.
        let reloaded = reload(&runtime, "svc_reloaded");
        let replay = route(&reloaded, exec_request("exec_lost", 1));
        assert_eq!(replay["error"]["code"], "CONTROL_REVOKED", "{replay}");
        assert_eq!(runner.count("exec"), 0);
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn f2_an_ambiguous_close_outlives_the_result_window() {
        let (runtime, runner, root) = fixture("f2-close", "svc_test");
        let close = |request_id: &str| {
            route(
                &runtime,
                request(
                    "deck_session_close",
                    json!({"request_id":request_id,"session_id":"mcp_a","expected_generation":"g_a","control_epoch":1,"holder_id":"holder_a"}),
                ),
            )
        };
        let unknown = close("close_unknown");
        let unknown_id = unknown["operationId"].as_str().unwrap().to_owned();
        claim(&runtime, &unknown_id).unwrap();
        close_admit(&runtime, &unknown_id).unwrap();
        complete(
            &runtime,
            unknown_id.clone(),
            "ambiguous".into(),
            Some("close-failed".into()),
            None,
        )
        .unwrap();
        for index in 0..CREATE_REPLAY_WINDOW {
            let later = close(&format!("close_{index}"));
            let id = later["operationId"].as_str().unwrap().to_owned();
            claim(&runtime, &id).unwrap();
            complete(
                &runtime,
                id,
                "rejected".into(),
                Some("close-failed".into()),
                None,
            )
            .unwrap();
        }
        let replay = close("close_unknown");
        assert_eq!(replay["operationId"], unknown_id.as_str(), "{replay}");
        assert_eq!(replay["state"], "ambiguous");
        runtime
            .read(|doc| {
                assert!(
                    doc.operations.iter().all(|op| op.state != "accepted"),
                    "re-admitted"
                );
                assert!(!doc.sessions[0].closing);
            })
            .unwrap();
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn c1_an_exact_renew_replay_never_extends_the_lease() {
        let (runtime, runner, root) = fixture("c1-renew", "svc_test");
        unowned(&runtime);
        grant_window(&runtime);
        let grant = runtime
            .read(|doc| doc.execution_grants.last().unwrap().clone())
            .unwrap();
        let requested = control(&runtime, "req_a", "request", None, 0);
        let epoch = requested["result"]["controlEpoch"].as_u64().unwrap();
        let renew = control(&runtime, "renew_a", "renew", Some(epoch), 1);
        assert_eq!(renew["state"], "committed", "{renew}");
        assert!(
            renew["operationId"].is_string(),
            "a renew has a queryable operation"
        );
        let first_deadline = renew["result"]["leaseExpiresAt"].as_u64().unwrap();
        test_clock::advance(30_000);
        let replay = control(&runtime, "renew_a", "renew", Some(epoch), 1);
        assert_eq!(replay["operationId"], renew["operationId"], "{replay}");
        assert_eq!(replay["result"]["leaseExpiresAt"], first_deadline);
        assert_eq!(
            session_state(&runtime).lease_expires_at,
            Some(first_deadline),
            "the replay extended the lease"
        );
        let queried = route(
            &runtime,
            request(
                "deck_operation_get",
                json!({"operation_id":renew["operationId"]}),
            ),
        );
        assert_eq!(queried["kind"], "session-control");
        // A new logical renew extends it.
        let renewed = control(&runtime, "renew_b", "renew", Some(epoch), 2);
        let second_deadline = renewed["result"]["leaseExpiresAt"].as_u64().unwrap();
        assert!(second_deadline >= first_deadline + 30_000);
        // The superseded renew is stale now and changes nothing.
        let stale = control(&runtime, "renew_a", "renew", Some(epoch), 1);
        assert_eq!(stale["error"]["code"], "STALE_REQUEST", "{stale}");
        assert_eq!(
            session_state(&runtime).lease_expires_at,
            Some(second_deadline)
        );
        // The execution window never moves with the lease.
        let current = runtime
            .read(|doc| doc.execution_grants.last().unwrap().clone())
            .unwrap();
        assert_eq!(
            (current.grant_id.clone(), current.expires_at),
            (grant.grant_id.clone(), grant.expires_at)
        );
        // Sustained renewals keep one control record per session.
        let mut sequence = 3;
        let mut last = Value::Null;
        for index in 0..200 {
            let value = control(
                &runtime,
                &format!("renew_many_{index}"),
                "renew",
                Some(epoch),
                sequence,
            );
            assert_eq!(value["state"], "committed", "{value}");
            sequence += 1;
            last = value;
        }
        assert_eq!(
            runtime
                .read(|doc| doc
                    .operations
                    .iter()
                    .filter(|op| is_control_record(op))
                    .count())
                .unwrap(),
            1
        );
        // After the context ends, the latest renew cannot restore control.
        takeover(&runtime, "M1").unwrap();
        return_control(&runtime, "M1").unwrap();
        let replay = control(
            &runtime,
            "renew_many_199",
            "renew",
            Some(epoch),
            sequence - 1,
        );
        // Either the recorded (historical) receipt of that very renew, or a
        // stale refusal once it is retired — never a new effect.
        assert!(
            replay["operationId"] == last["operationId"]
                || replay["error"]["code"] == "STALE_REQUEST",
            "{replay}"
        );
        assert_eq!(
            session_state(&runtime).control_owner,
            None,
            "a renew replay restored control"
        );
        assert_eq!(session_state(&runtime).lease_expires_at, None);
        let reloaded = reload(&runtime, "svc_reloaded");
        let replay = control(&reloaded, "renew_a", "renew", Some(epoch), 1);
        assert_eq!(replay["error"]["code"], "STALE_REQUEST", "{replay}");
        assert_eq!(session_state(&reloaded).lease_expires_at, None);
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn f3_a_full_client_quota_recovers_by_release_and_request() {
        let (runtime, runner, root) = fixture("f3-client", "svc_test");
        unowned(&runtime);
        grant_window(&runtime);
        let epoch = control_request(&runtime, "req_a");
        let before = route(&runtime, exec_request("exec_before", epoch));
        assert_eq!(before["state"], "exited", "{before}");
        let own = own_count(&runtime, "client_a");
        fill(
            &runtime,
            "client_a",
            "mcp_a",
            epoch,
            MAX_OPERATIONS_PER_CLIENT - own,
            "own",
        );
        let blocked = route(&runtime, exec_request("exec_blocked", epoch));
        assert_eq!(blocked["error"]["code"], "CAPACITY_EXCEEDED", "{blocked}");
        assert_eq!(runner.count("exec"), 1);
        // The recovery the error names, performed for real.
        release(&runtime, "rel_a", epoch);
        let next = control_request(&runtime, "req_b");
        assert!(
            own_count(&runtime, "client_a") < 8,
            "the old epoch was not retired"
        );
        let after = route(&runtime, exec_request("exec_after", next));
        assert_eq!(after["state"], "exited", "{after}");
        assert_eq!(runner.count("exec"), 2);
        let replay = route(&runtime, exec_request("exec_before", epoch));
        assert_eq!(replay["error"]["code"], "CONTROL_REVOKED", "{replay}");
        assert_eq!(runner.count("exec"), 2, "a retired exec ran again");
        // The boundary survives a restart; new work continues.
        let reloaded = reload(&runtime, "svc_reloaded");
        for (request_id, bound) in [("exec_before", epoch), ("exec_after", next)] {
            let replay = route(&reloaded, exec_request(request_id, bound));
            assert_eq!(replay["error"]["code"], "CONTROL_REVOKED", "{replay}");
        }
        assert_eq!(runner.count("exec"), 2);
        super::execution_grant(&reloaded, "mcp_a".into(), Some(60_000), true, true).unwrap();
        let epoch = control_request(&reloaded, "req_c");
        let resumed = route(&reloaded, exec_request("exec_resumed", epoch));
        assert_eq!(resumed["state"], "exited", "{resumed}");
        assert_eq!(runner.count("exec"), 3);
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// A second managed session owned by another client, for global tests.
    fn add_other_session(runtime: &Runtime, lease_expires_at: Option<u64>) {
        runtime
            .write(|doc| {
                let mut other = doc.sessions[0].clone();
                other.session_id = "mcp_b".into();
                other.card_id = "M2".into();
                other.owner_client_id = "client_b".into();
                other.control_owner = Some("client_b".into());
                other.control_holder = Some("holder_b".into());
                other.lease_expires_at = lease_expires_at;
                doc.sessions.push(other);
                Ok(())
            })
            .unwrap();
    }

    fn fill_others(runtime: &Runtime, session_id: &str, mut count: usize) {
        let mut client = 0;
        while count > 0 {
            let chunk = count.min(MAX_OPERATIONS_PER_CLIENT);
            fill(
                runtime,
                &format!("client_fill_{client}"),
                session_id,
                1,
                chunk,
                &format!("fill{client}"),
            );
            count -= chunk;
            client += 1;
        }
    }

    #[test]
    fn f3_a_full_global_pool_recovers_by_release_and_request() {
        let (runtime, runner, root) = fixture("f3-global", "svc_test");
        unowned(&runtime);
        grant_window(&runtime);
        add_other_session(&runtime, Some(now_ms() + 600_000));
        let epoch = control_request(&runtime, "req_a");
        let before = route(&runtime, exec_request("exec_before", epoch));
        assert_eq!(before["state"], "exited", "{before}");
        fill(&runtime, "client_a", "mcp_a", epoch, 300, "own");
        fill_others(
            &runtime,
            "mcp_b",
            ORDINARY_OPERATIONS - ordinary_count(&runtime),
        );
        assert_eq!(ordinary_count(&runtime), ORDINARY_OPERATIONS);
        assert!(own_count(&runtime, "client_a") < MAX_OPERATIONS_PER_CLIENT);
        let blocked = route(&runtime, exec_request("exec_blocked", epoch));
        assert_eq!(blocked["error"]["code"], "CAPACITY_EXCEEDED", "{blocked}");
        release(&runtime, "rel_a", epoch);
        let next = control_request(&runtime, "req_b");
        let after = route(&runtime, exec_request("exec_after", next));
        assert_eq!(after["state"], "exited", "{after}");
        assert_eq!(runner.count("exec"), 2);
        let replay = route(&runtime, exec_request("exec_before", epoch));
        assert_eq!(replay["error"]["code"], "CONTROL_REVOKED", "{replay}");
        assert_eq!(runner.count("exec"), 2);
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();

        // Another client whose lease lapsed holds the whole pool: its dead
        // epoch is closed under pressure and this client proceeds.
        let (runtime, runner, root) = fixture("f3-lapsed", "svc_test");
        unowned(&runtime);
        grant_window(&runtime);
        add_other_session(&runtime, Some(now_ms().saturating_sub(1)));
        let epoch = control_request(&runtime, "req_a");
        fill_others(
            &runtime,
            "mcp_b",
            ORDINARY_OPERATIONS - ordinary_count(&runtime),
        );
        let proceeded = route(&runtime, exec_request("exec_after_lapse", epoch));
        assert_eq!(proceeded["state"], "exited", "{proceeded}");
        runtime
            .read(|doc| {
                let other = doc
                    .sessions
                    .iter()
                    .find(|s| s.session_id == "mcp_b")
                    .unwrap();
                assert_eq!(other.control_owner, None);
                assert_eq!(other.control_epoch, 2);
                assert!(doc.operations.len() < 16);
            })
            .unwrap();
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn f3_a_lapsed_lease_at_full_capacity_recovers_by_request_without_a_new_grant() {
        let (runtime, runner, root) = fixture("f3-lapse", "svc_test");
        unowned(&runtime);
        super::execution_grant(
            &runtime,
            "mcp_a".into(),
            Some(MAX_EXECUTION_GRANT_MS),
            true,
            true,
        )
        .unwrap();
        let grant = runtime
            .read(|doc| doc.execution_grants.last().unwrap().clone())
            .unwrap();
        let epoch = control_request(&runtime, "req_a");
        let own = own_count(&runtime, "client_a");
        fill(
            &runtime,
            "client_a",
            "mcp_a",
            epoch,
            MAX_OPERATIONS_PER_CLIENT - own,
            "own",
        );
        // The lease runs out naturally.
        test_clock::advance(DEFAULT_LEASE_MS + 1);
        let blocked = route(&runtime, exec_request("exec_blocked", epoch));
        assert_eq!(blocked["ok"], false, "{blocked}");
        let released = control(
            &runtime,
            "rel_late",
            "release",
            Some(epoch),
            session_state(&runtime).control_sequence,
        );
        assert_eq!(released["error"]["code"], "CONTROL_REVOKED", "{released}");
        let next = control_request(&runtime, "req_b");
        assert!(next > epoch);
        let current = runtime
            .read(|doc| doc.execution_grants.last().unwrap().clone())
            .unwrap();
        assert_eq!(
            (current.grant_id.clone(), current.expires_at),
            (grant.grant_id.clone(), grant.expires_at),
            "control changed the execution window"
        );
        let after = route(&runtime, exec_request("exec_after", next));
        assert_eq!(after["state"], "exited", "{after}");
        assert_eq!(runner.count("exec"), 1);
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn f3_interrupt_reserve_survives_a_full_pool_and_one_greedy_client() {
        let (runtime, runner, root) = fixture("f3-interrupt", "svc_test");
        grant_window(&runtime);
        let executed = route(&runtime, exec_request("exec_live", 1));
        let job_id = executed["jobId"].as_str().unwrap().to_owned();
        // A second client with its own session, runner and job.
        let root_b = test_root("f3-interrupt-b");
        let runner_b = FakeRunner::start(&root_b, "g_b");
        runner_b.set_control(1, "mcp", Some("holder_b"));
        runtime
            .write(|doc| {
                let mut client_b = client_record(&root);
                client_b.id = "client_b".into();
                client_b.credential_hash = sha(b"mcp_test_b");
                doc.config.clients.push(client_b);
                let mut session_b = session_record(&root, &runner_b);
                session_b.session_id = "mcp_b".into();
                session_b.card_id = "M2".into();
                session_b.generation = "g_b".into();
                session_b.owner_client_id = "client_b".into();
                session_b.control_owner = Some("client_b".into());
                session_b.control_holder = Some("holder_b".into());
                doc.sessions.push(session_b.clone());
                let mut grant_b = execution_grant(&session_b);
                grant_b.grant_id = "grant_b".into();
                doc.execution_grants.push(grant_b);
                doc.jobs.push(JobBinding {
                    job_id: "job_b".into(),
                    client_id: "client_b".into(),
                    session_id: "mcp_b".into(),
                    session_generation: "g_b".into(),
                    request_hash: "b".repeat(64),
                    operation_id: "op_b".into(),
                    grant_id: "grant_b".into(),
                    grant_version: 1,
                    allow_output: true,
                });
                Ok(())
            })
            .unwrap();
        fill_others(
            &runtime,
            "mcp_a",
            ORDINARY_OPERATIONS - ordinary_count(&runtime),
        );
        let blocked = route(&runtime, exec_request("exec_full", 1));
        assert_eq!(blocked["error"]["code"], "CAPACITY_EXCEEDED", "{blocked}");
        let first = route(&runtime, interrupt_request("stop_0", &job_id));
        assert_eq!(first["state"], "committed", "{first}");
        // Accepted is only a request: the job state comes from deck_job_read.
        assert!(first.get("processComplete").is_none());
        // An exact repeat uses no slot and sends nothing.
        let interrupts = runner.count("interrupt");
        let repeated = route(&runtime, interrupt_request("stop_0", &job_id));
        assert_eq!(repeated["operationId"], first["operationId"]);
        assert_eq!(runner.count("interrupt"), interrupts);
        for index in 1..INTERRUPT_RESERVE_PER_CLIENT {
            let value = route(
                &runtime,
                interrupt_request(&format!("stop_{index}"), &job_id),
            );
            assert_eq!(value["state"], "committed", "{value}");
        }
        let greedy = route(&runtime, interrupt_request("stop_greedy", &job_id));
        assert_eq!(greedy["error"]["code"], "CAPACITY_EXCEEDED", "{greedy}");
        // The other client can still stop its own work.
        let other = route(
            &runtime,
            WireRequest {
                version: CONTROL_PROTOCOL,
                client_id: "client_b".into(),
                credential: "mcp_test_b".into(),
                tool: "deck_job_interrupt".into(),
                arguments: json!({"request_id":"stop_b","job_id":"job_b","session_generation":"g_b","control_epoch":1,"holder_id":"holder_b"}),
            },
        );
        assert_eq!(other["state"], "committed", "{other}");
        assert_eq!(runner_b.count("interrupt"), 1);
        drop(runner_b);
        drop(runner);
        std::fs::remove_dir_all(root_b).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
