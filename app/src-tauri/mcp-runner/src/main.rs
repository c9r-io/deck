//! Visible execution host for one Deck MCP tmux pane.
//!
//! The runner is a signed bundle sidecar and is started only as the pane
//! process of an explicitly created MCP session. It accepts structured direct
//! launches over a user-only Unix socket, executes each job in a fresh process, mirrors
//! combined output into the real pane, and retains a bounded copy for cursor
//! reads. Fresh jobs receive the versioned sanitized developer environment,
//! not the runner's complete host environment. Process exit and each output
//! pipe's EOF are reported separately. The runner has no model, persistence,
//! or Board authority; trusted-host jobs retain the user's ordinary OS/network
//! permissions and the environment profile is not a sandbox.
//!
//! Process ownership: tmux execs the runner as the pane's session leader and
//! foreground process group. Each direct executable is in its OWN process
//! group (pid == pgid) with piped stdio, so the job is a background group of
//! the same terminal. The runner never starts any other shell: human takeover
//! only changes who may type into the active job's stdin.
//!
//! Signals: SIGINT/SIGQUIT/SIGTSTP/SIGHUP/SIGTERM are blocked in every runner
//! thread and consumed by one `sigwait` thread, so terminal keys never kill or
//! stop the runner itself. In human mode a terminal ^C becomes
//! `killpg(active job, SIGINT)` — the local stop key. SIGHUP (tmux
//! kill-session) and SIGTERM make the runner `killpg(SIGKILL)` every live job
//! group before exiting. The `stop` request (sent by Deck before a close)
//! escalates SIGINT → SIGTERM → SIGKILL with bounded waits and reports whether
//! every job leader was reaped; an expired execution timeout applies the same
//! escalation to its one job group. Jobs start with default dispositions and an
//! empty signal mask. These guarantees cover only jobs that stay in their
//! process group: a descendant that calls setsid/setpgid, or a group member
//! that outlives the leader, is outside the runner's reach (trusted-host is
//! not an OS sandbox). A job stopped by SIGTTIN/SIGTTOU (it touched the tty
//! from the background) is reported as `stopped`, never as `running`.
//!
//! Connections: the accept loop closes every peer whose kernel-reported PID
//! is not the launching Deck process before it takes one of the bounded
//! connection slots, so a same-uid job cannot hold the slots and starve
//! Deck's `control`/`stop` requests. Deck is the runner's only client.
//!
//! Authentication: every runner generates an independent 256-bit random key
//! in memory. The Deck process whose PID was fixed at launch may retrieve it
//! exactly once; the kernel-reported Unix-socket peer PID, not the claimed
//! JSON value, authorizes that bootstrap. Every later request carries the key
//! and is compared in constant time. It is never argv, environment, a tmux
//! option, or a file. A Deck restart cannot reclaim an old runner; closing
//! the tmux pane remains the cleanup boundary and its SIGHUP path kills jobs.
//!
//! Every control/exec/input/interrupt/grant request also names Deck's service
//! instance. A mismatch means Deck restarted after this runner was created.
//! Control changes are authenticated and advance to any strictly greater
//! epoch; gaps absorb Deck state changes that were persisted without a runner
//! acknowledgement, while equality and rollback are rejected. Exec contexts
//! never create grants or advance control and must match the current epoch.
//! The socket directory is created 0700 in one operation (or an existing
//! directory is accepted only at exactly 0700). The socket is bound under a
//! private temporary name, made 0600, and atomically renamed into place, so a
//! process-wide umask is never changed and the published path is never lax.
//! An idle runner holds no timer: the accept loop blocks, `Shutdown` exits
//! from the connection that answered it, and each job's reaper blocks in
//! `waitid(WNOWAIT)` until the leader has an event.

use base64::Engine;
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const BUILD: Option<&str> = match option_env!("DECK_BUILD_SHA") {
    Some(value) => Some(value),
    None => option_env!("GITHUB_SHA"),
};
const MAX_CONNECTIONS: usize = 16;
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);
/// Bounded escalation used by `stop`: each signal gets this long to reap.
const STOP_STEP: Duration = Duration::from_millis(1_000);
const MAX_REQUEST: usize = 256 * 1024;
const MAX_EXECUTABLE: usize = 4 * 1024;
const MAX_ARGUMENTS: usize = 256;
const MAX_ARGUMENT_BYTES: usize = 64 * 1024;
const MAX_READ: usize = 16 * 1024;
/// One response line including its trailing newline.
const MAX_RESPONSE: usize = 128 * 1024;
const MAX_INPUT: usize = 32 * 1024;
const RETAINED_OUTPUT: usize = 1024 * 1024;
const RETAINED_OUTPUT_PER_SESSION: usize = 16 * 1024 * 1024;
/// This runner's own job cap, independent of Deck's per-session binding
/// bound (64): each side retires its oldest finished entries against its own.
const MAX_JOBS: usize = 256;
const MAX_WAIT_MS: u64 = 5_000;
const MIN_OUTPUT_RETENTION_MS: u64 = 60_000;
const MAX_OUTPUT_RETENTION_MS: u64 = 7 * 24 * 60 * 60_000;
const INPUT_WRITE_TIMEOUT: Duration = Duration::from_secs(1);

/// Every error string this runner puts on the wire. `ALL` is held to
/// `mcp-fixtures/runner-errors.json`, where Deck's class for each one is
/// recorded (rejection: no process started and no byte was written;
/// ambiguous: a process may have started). Deck's `RUNNER_ERRORS` table is
/// held to the same file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RunnerError {
    AuthenticationFailed,
    CapacityExceeded,
    ControlRevoked,
    DispatchContextInvalid,
    HolderConflict,
    InternalError,
    InvalidCwd,
    InvalidExecutable,
    InvalidRequest,
    JobNotFound,
    JobNotRunning,
    JobStateUnknown,
    OutputCursorInvalid,
    RequestIdConflict,
    ResponseTooLarge,
    RunnerStale,
    SessionBusy,
    SpawnFailed,
    StopUnconfirmed,
}

impl RunnerError {
    #[cfg(test)]
    const ALL: [Self; 19] = [
        Self::AuthenticationFailed,
        Self::CapacityExceeded,
        Self::ControlRevoked,
        Self::DispatchContextInvalid,
        Self::HolderConflict,
        Self::InternalError,
        Self::InvalidCwd,
        Self::InvalidExecutable,
        Self::InvalidRequest,
        Self::JobNotFound,
        Self::JobNotRunning,
        Self::JobStateUnknown,
        Self::OutputCursorInvalid,
        Self::RequestIdConflict,
        Self::ResponseTooLarge,
        Self::RunnerStale,
        Self::SessionBusy,
        Self::SpawnFailed,
        Self::StopUnconfirmed,
    ];

    fn as_str(self) -> &'static str {
        match self {
            Self::AuthenticationFailed => "authentication-failed",
            Self::CapacityExceeded => "capacity-exceeded",
            Self::ControlRevoked => "control-revoked",
            Self::DispatchContextInvalid => "dispatch-context-invalid",
            Self::HolderConflict => "holder-conflict",
            Self::InternalError => "internal-error",
            Self::InvalidCwd => "invalid-cwd",
            Self::InvalidExecutable => "invalid-executable",
            Self::InvalidRequest => "invalid-request",
            Self::JobNotFound => "job-not-found",
            Self::JobNotRunning => "job-not-running",
            Self::JobStateUnknown => "job-state-unknown",
            Self::OutputCursorInvalid => "output-cursor-invalid",
            Self::RequestIdConflict => "request-id-conflict",
            Self::ResponseTooLarge => "response-too-large",
            Self::RunnerStale => "runner-stale",
            Self::SessionBusy => "session-busy",
            Self::SpawnFailed => "spawn-failed",
            Self::StopUnconfirmed => "stop-unconfirmed",
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis() as u64)
        .unwrap_or(0)
}

/// `lock()` that recovers a poisoned mutex instead of taking down every
/// thread that touches it after one panicked while holding it — the same
/// policy as deck-app's `src/sync.rs`. `clippy.toml` refuses a raw
/// `Mutex::lock` anywhere else in the workspace.
trait LockRecover<T> {
    fn lock_or_recover(&self) -> MutexGuard<'_, T>;
}
impl<T> LockRecover<T> for Mutex<T> {
    #[allow(clippy::disallowed_methods)] // the recovering lock itself
    fn lock_or_recover(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(|error| error.into_inner())
    }
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum Request {
    Ping {
        /// When present, the response says whether this runner still belongs
        /// to that Deck service instance (`serviceCurrent`).
        #[serde(default)]
        service_instance: Option<String>,
    },
    Exec {
        job_id: String,
        request_hash: String,
        executable: String,
        #[serde(default)]
        args: Vec<String>,
        cwd: String,
        #[serde(default)]
        wait_ms: u64,
        #[serde(default)]
        timeout_ms: Option<u64>,
        context: DispatchContext,
    },
    Read {
        job_id: String,
        #[serde(default)]
        cursor: Option<u64>,
        #[serde(default = "default_read")]
        max_bytes: usize,
        #[serde(default)]
        wait_ms: u64,
    },
    Input {
        job_id: String,
        data_b64: String,
        context: DispatchContext,
    },
    Interrupt {
        job_id: String,
        context: DispatchContext,
    },
    Control {
        mode: ControlMode,
        service_instance: String,
        control_epoch: u64,
        #[serde(default)]
        holder_id: Option<String>,
    },
    Retention {
        service_instance: String,
        output_retention_ms: u64,
    },
    RevokeGrant {
        service_instance: String,
        grant_id: String,
        grant_version: u64,
    },
    AuthorizeGrant {
        service_instance: String,
        grant_id: String,
        grant_version: u64,
        policy_version: u64,
        expires_at: u64,
    },
    /// Fence control and terminate every live job group with bounded
    /// escalation. The launching Deck process authenticates this request;
    /// after its restart, pane SIGHUP supplies the cleanup boundary instead.
    Stop {
        generation: String,
    },
    Shutdown,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaimRequest {
    kind: String,
    service_instance: String,
    generation: String,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct DispatchContext {
    service_instance: String,
    holder_id: String,
    control_epoch: u64,
    grant_id: String,
    grant_version: u64,
    policy_version: u64,
    intent_hash: String,
    expires_at: u64,
}

fn default_read() -> usize {
    MAX_READ
}

/// Consume complete UTF-8 units while allowing genuinely invalid PTY bytes to
/// advance lossily. An incomplete final unit is deferred to the next cursor
/// read instead of being replaced twice across the boundary.
fn complete_utf8_prefix(bytes: &[u8]) -> usize {
    let mut consumed = 0;
    while consumed < bytes.len() {
        match std::str::from_utf8(&bytes[consumed..]) {
            Ok(_) => return bytes.len(),
            Err(error) => {
                consumed += error.valid_up_to();
                match error.error_len() {
                    Some(length) => consumed += length,
                    None => return consumed,
                }
            }
        }
    }
    consumed
}

#[derive(Clone, Copy, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
enum ControlMode {
    Mcp,
    Human,
    Fenced,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Response {
    ok: bool,
    generation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    job: Option<JobView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_cursor: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gap: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dropped_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    control: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    deletion_reason: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    service_current: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    runner_version: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    runner_build: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    auth: Option<String>,
}

impl Response {
    fn error(generation: &str, error: RunnerError) -> Self {
        Self {
            ok: false,
            error: Some(error.as_str()),
            ..Self::empty(generation)
        }
    }
    fn empty(generation: &str) -> Self {
        Self {
            ok: true,
            generation: generation.into(),
            error: None,
            job: None,
            output: None,
            next_cursor: None,
            gap: None,
            dropped_bytes: None,
            control: None,
            deletion_reason: None,
            service_current: None,
            runner_version: None,
            runner_build: None,
            auth: None,
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct JobView {
    job_id: String,
    state: &'static str,
    exit_code: Option<i32>,
    termination_signal: Option<i32>,
    started_at: u64,
    ended_at: Option<u64>,
    interrupt_requested: bool,
    timeout_requested: bool,
    base_cursor: u64,
    end_cursor: u64,
    stdout_eof: bool,
    stderr_eof: bool,
    output_complete: bool,
}

struct Job {
    id: String,
    request_hash: String,
    holder_id: String,
    control_epoch: u64,
    intent_hash: String,
    grant_id: String,
    grant_version: u64,
    state: JobState,
    exit_code: Option<i32>,
    signal: Option<i32>,
    started_at: u64,
    ended_at: Option<u64>,
    interrupt_requested: bool,
    timeout_requested: bool,
    output: VecDeque<u8>,
    base_cursor: u64,
    /// Unreaped job leader; also its process-group id. Cleared under the
    /// `Inner` lock in the same critical section that reaps the leader, so a
    /// signal sent while holding that lock can never reach a reused PID.
    pid: Option<i32>,
    stdin: Option<ChildStdin>,
    stdout_eof: bool,
    stderr_eof: bool,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum JobState {
    Starting,
    Running,
    /// Stopped by a job-control signal (typically SIGTTIN/SIGTTOU).
    Stopped,
    Exited,
    Lost,
}

impl Job {
    fn new(id: String, request_hash: String, context: &DispatchContext) -> Self {
        Self {
            id,
            request_hash,
            holder_id: context.holder_id.clone(),
            control_epoch: context.control_epoch,
            intent_hash: context.intent_hash.clone(),
            grant_id: context.grant_id.clone(),
            grant_version: context.grant_version,
            state: JobState::Starting,
            exit_code: None,
            signal: None,
            started_at: now_ms(),
            ended_at: None,
            interrupt_requested: false,
            timeout_requested: false,
            output: VecDeque::new(),
            base_cursor: 0,
            pid: None,
            stdin: None,
            stdout_eof: false,
            stderr_eof: false,
        }
    }

    fn live(&self) -> bool {
        matches!(
            self.state,
            JobState::Starting | JobState::Running | JobState::Stopped
        )
    }

    fn view(&self) -> JobView {
        JobView {
            job_id: self.id.clone(),
            state: match self.state {
                JobState::Starting => "starting",
                JobState::Running => "running",
                JobState::Stopped => "stopped",
                JobState::Exited => "exited",
                JobState::Lost => "lost",
            },
            exit_code: self.exit_code,
            termination_signal: self.signal,
            started_at: self.started_at,
            ended_at: self.ended_at,
            interrupt_requested: self.interrupt_requested,
            timeout_requested: self.timeout_requested,
            base_cursor: self.base_cursor,
            end_cursor: self.base_cursor + self.output.len() as u64,
            stdout_eof: self.stdout_eof,
            stderr_eof: self.stderr_eof,
            output_complete: self.stdout_eof && self.stderr_eof,
        }
    }
}

struct Inner {
    jobs: HashMap<String, Job>,
    order: VecDeque<String>,
    active: Option<String>,
    control: ControlMode,
    control_epoch: u64,
    holder_id: Option<String>,
    revoked_grants: HashMap<String, u64>,
    authorized_grants: HashMap<String, AuthorizedGrant>,
    stopping: bool,
    retained_output: usize,
}

struct AuthorizedGrant {
    version: u64,
    policy_version: u64,
    expires_at: u64,
}

struct Shared {
    generation: String,
    service_instance: String,
    output_retention_ms: AtomicU64,
    inner: Mutex<Inner>,
    changed: Condvar,
    connections: AtomicUsize,
    auth_key: [u8; 32],
    deck_pid: libc::pid_t,
    claimed: std::sync::atomic::AtomicBool,
}

/// Send `signal` to every live job group. Must be called with `inner` held so
/// the PIDs cannot be reaped (and reused) concurrently.
fn signal_live_groups(inner: &Inner, signal: i32) -> usize {
    let mut sent = 0;
    for job in inner.jobs.values() {
        if let Some(pid) = job.pid {
            // SAFETY: `pid` is an unreaped leader of its own process group;
            // the waiter clears it under this same lock before reaping.
            if unsafe { libc::killpg(pid, signal) } == 0 {
                sent += 1;
            }
            if signal != libc::SIGKILL && job.state == JobState::Stopped {
                // A stopped group cannot act on INT/TERM until continued.
                unsafe { libc::killpg(pid, libc::SIGCONT) };
            }
        }
    }
    sent
}

/// Signals that must never stop or kill the runner through its terminal.
fn runner_signal_set() -> libc::sigset_t {
    // SAFETY: sigemptyset/sigaddset initialize a local sigset_t.
    unsafe {
        let mut set = std::mem::zeroed::<libc::sigset_t>();
        libc::sigemptyset(&mut set);
        for signal in [
            libc::SIGINT,
            libc::SIGQUIT,
            libc::SIGTSTP,
            libc::SIGHUP,
            libc::SIGTERM,
        ] {
            libc::sigaddset(&mut set, signal);
        }
        set
    }
}

/// Consume terminal/lifecycle signals on one thread. Called after the main
/// thread blocked `runner_signal_set` so every thread inherits the mask.
fn signal_thread(shared: Arc<Shared>, socket: PathBuf) {
    std::thread::spawn(move || {
        let set = runner_signal_set();
        loop {
            let mut signal = 0;
            // SAFETY: `set` is initialized and every member is blocked.
            if unsafe { libc::sigwait(&set, &mut signal) } != 0 {
                continue;
            }
            match signal {
                libc::SIGINT => {
                    // The local stop key: only the human may use it, and only
                    // against the one active job group.
                    let mut inner = shared.inner.lock_or_recover();
                    if inner.control != ControlMode::Human {
                        continue;
                    }
                    let Some(id) = inner.active.clone() else {
                        continue;
                    };
                    if let Some(job) = inner.jobs.get_mut(&id) {
                        if let Some(pid) = job.pid {
                            job.interrupt_requested = true;
                            // SAFETY: see `signal_live_groups`.
                            unsafe { libc::killpg(pid, libc::SIGINT) };
                            if job.state == JobState::Stopped {
                                unsafe { libc::killpg(pid, libc::SIGCONT) };
                            }
                        }
                    }
                }
                libc::SIGHUP | libc::SIGTERM => {
                    let inner = shared.inner.lock_or_recover();
                    signal_live_groups(&inner, libc::SIGKILL);
                    let _ = std::fs::remove_file(&socket);
                    std::process::exit(0);
                }
                // ^\ and ^Z are swallowed: they must not stop the runner.
                _ => {}
            }
        }
    });
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn admit_exec_context(
    shared: &Shared,
    inner: &mut Inner,
    context: &DispatchContext,
) -> Result<(), RunnerError> {
    if context.service_instance != shared.service_instance {
        return Err(RunnerError::RunnerStale);
    }
    if !valid_id(&context.holder_id)
        || !valid_id(&context.grant_id)
        || !valid_hash(&context.intent_hash)
        || context.grant_version == 0
        || context.policy_version == 0
        || context.expires_at <= now_ms()
        || inner
            .revoked_grants
            .get(&context.grant_id)
            .is_some_and(|version| *version >= context.grant_version)
    {
        return Err(RunnerError::DispatchContextInvalid);
    }
    let Some(grant) = inner.authorized_grants.get(&context.grant_id) else {
        return Err(RunnerError::DispatchContextInvalid);
    };
    if grant.version != context.grant_version
        || grant.policy_version != context.policy_version
        || grant.expires_at != context.expires_at
    {
        return Err(RunnerError::DispatchContextInvalid);
    }
    if inner.control != ControlMode::Mcp || context.control_epoch != inner.control_epoch {
        return Err(RunnerError::ControlRevoked);
    }
    if inner.holder_id.as_deref() != Some(&context.holder_id) {
        return Err(RunnerError::HolderConflict);
    }
    Ok(())
}

fn check_job_context(
    shared: &Shared,
    inner: &Inner,
    job: &Job,
    context: &DispatchContext,
    allow_expired_or_revoked: bool,
) -> Result<(), RunnerError> {
    if context.service_instance != shared.service_instance {
        return Err(RunnerError::RunnerStale);
    }
    if (!allow_expired_or_revoked && context.expires_at <= now_ms())
        || context.control_epoch != inner.control_epoch
        || context.control_epoch != job.control_epoch
        || inner.holder_id.as_deref() != Some(&context.holder_id)
        || job.holder_id != context.holder_id
        || job.intent_hash != context.intent_hash
        || job.grant_id != context.grant_id
        || job.grant_version != context.grant_version
        || (!allow_expired_or_revoked
            && inner
                .revoked_grants
                .get(&context.grant_id)
                .is_some_and(|version| *version >= context.grant_version))
    {
        return Err(RunnerError::ControlRevoked);
    }
    Ok(())
}

fn append_output(shared: &Arc<Shared>, job_id: &str, bytes: &[u8]) {
    let mut inner = shared.inner.lock_or_recover();
    {
        let Some(job) = inner.jobs.get_mut(job_id) else {
            return;
        };
        for byte in bytes {
            job.output.push_back(*byte);
        }
        while job.output.len() > RETAINED_OUTPUT {
            job.output.pop_front();
            job.base_cursor = job.base_cursor.saturating_add(1);
        }
    }
    inner.retained_output = inner.jobs.values().map(|job| job.output.len()).sum();
    while inner.retained_output > RETAINED_OUTPUT_PER_SESSION {
        let victim = inner
            .order
            .iter()
            .find(|id| {
                inner
                    .jobs
                    .get(*id)
                    .is_some_and(|job| !job.output.is_empty())
            })
            .cloned();
        let Some(victim) = victim else { break };
        if let Some(job) = inner.jobs.get_mut(&victim) {
            job.output.pop_front();
            job.base_cursor = job.base_cursor.saturating_add(1);
            inner.retained_output -= 1;
        }
    }
    shared.changed.notify_all();
}

fn mirror(
    shared: Arc<Shared>,
    job_id: String,
    mut input: impl Read + Send + 'static,
    stderr: bool,
) {
    std::thread::spawn(move || {
        let mut output: Box<dyn Write + Send> = if stderr {
            Box::new(std::io::stderr())
        } else {
            Box::new(std::io::stdout())
        };
        let mut buffer = [0u8; 8192];
        loop {
            match input.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    let bytes = &buffer[..count];
                    let _ = output.write_all(bytes);
                    let _ = output.flush();
                    append_output(&shared, &job_id, bytes);
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        let mut inner = shared.inner.lock_or_recover();
        if let Some(job) = inner.jobs.get_mut(&job_id) {
            if stderr {
                job.stderr_eof = true;
            } else {
                job.stdout_eof = true;
            }
        }
        shared.changed.notify_all();
    });
}

fn wait_writable(fd: i32, deadline: Instant) -> std::io::Result<()> {
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "write deadline",
            ));
        }
        let remaining = deadline.saturating_duration_since(now).as_millis().min(100) as i32;
        let mut descriptor = libc::pollfd {
            fd,
            events: libc::POLLOUT,
            revents: 0,
        };
        let result = unsafe { libc::poll(&mut descriptor, 1, remaining.max(1)) };
        if result > 0 {
            return Ok(());
        }
        if result < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return Err(std::io::Error::last_os_error());
        }
    }
}

fn bounded_job_input(
    shared: &Shared,
    job_id: &str,
    context: &DispatchContext,
    stdin: &mut ChildStdin,
    bytes: &[u8],
) -> std::io::Result<()> {
    let deadline = Instant::now() + INPUT_WRITE_TIMEOUT;
    let mut offset = 0;
    while offset < bytes.len() {
        {
            let inner = shared.inner.lock_or_recover();
            let current = inner
                .jobs
                .get(job_id)
                .filter(|job| job.state == JobState::Running);
            if inner.control != ControlMode::Mcp
                || inner.active.as_deref() != Some(job_id)
                || current.is_none_or(|job| {
                    check_job_context(shared, &inner, job, context, false).is_err()
                })
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "input fenced",
                ));
            }
        }
        let end = (offset + 4096).min(bytes.len());
        match stdin.write(&bytes[offset..end]) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "short write",
                ))
            }
            Ok(count) => offset += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                wait_writable(stdin.as_raw_fd(), deadline)?;
            }
            Err(error) => return Err(error),
        }
    }
    stdin.flush()
}

enum Launch<'a> {
    Direct {
        executable: &'a str,
        args: &'a [String],
    },
}

enum OwnedLaunch {
    Direct {
        executable: String,
        args: Vec<String>,
    },
}

fn spawn_job(
    shared: &Arc<Shared>,
    job_id: &str,
    launch: Launch<'_>,
    cwd: &str,
    timeout_ms: Option<u64>,
) -> Result<(), RunnerError> {
    if !Path::new(cwd).is_absolute() || !Path::new(cwd).is_dir() {
        return Err(RunnerError::InvalidCwd);
    }
    let mut command = match launch {
        Launch::Direct { executable, args } => {
            if executable.is_empty()
                || executable.len() > MAX_EXECUTABLE
                || executable.chars().any(char::is_control)
                || !Path::new(executable).is_absolute()
                || args.len() > MAX_ARGUMENTS
                || args.iter().any(|value| value.as_bytes().contains(&0))
                || args.iter().map(String::len).sum::<usize>() > MAX_ARGUMENT_BYTES
            {
                return Err(RunnerError::InvalidExecutable);
            }
            let mut command = Command::new(executable);
            command.args(args);
            command
        }
    };
    command
        .current_dir(cwd)
        .env_clear()
        .env(
            "PATH",
            "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Preserve only ordinary developer/runtime coordinates. Authentication
    // variables, agent sockets and application-specific secrets are excluded.
    for key in [
        "HOME", "USER", "LOGNAME", "TMPDIR", "LANG", "LC_ALL", "TERM",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    // SAFETY: only async-signal-safe setpgid/signal/sigprocmask
    // calls run after fork.
    unsafe {
        command.pre_exec(move || {
            if libc::setpgid(0, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            // The runner blocks terminal signals for its sigwait thread; a
            // job must start with ordinary dispositions and an empty mask.
            for signal in [
                libc::SIGINT,
                libc::SIGQUIT,
                libc::SIGTSTP,
                libc::SIGHUP,
                libc::SIGTERM,
            ] {
                libc::signal(signal, libc::SIG_DFL);
            }
            let mut empty = std::mem::zeroed::<libc::sigset_t>();
            libc::sigemptyset(&mut empty);
            if libc::sigprocmask(libc::SIG_SETMASK, &empty, std::ptr::null_mut()) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    // `spawn-failed` means exactly this: the process never started.
    let mut child = command.spawn().map_err(|_| RunnerError::SpawnFailed)?;
    let pid = child.id() as i32;
    // From here on the process exists. A failure below cannot prove it did
    // nothing, so it is `job-state-unknown` (Deck journals it ambiguous), and
    // its group is killed and reaped first so no untracked job survives.
    let pipes = (child.stdout.take(), child.stderr.take(), child.stdin.take());
    let mut abandon = move || {
        // SAFETY: `pid` is this runner's unreaped child and its own group.
        unsafe { libc::killpg(pid, libc::SIGKILL) };
        let _ = child.wait();
        RunnerError::JobStateUnknown
    };
    let (Some(stdout), Some(stderr), Some(stdin)) = pipes else {
        return Err(abandon());
    };
    let stdin_flags = unsafe { libc::fcntl(stdin.as_raw_fd(), libc::F_GETFL) };
    if stdin_flags < 0
        || unsafe {
            libc::fcntl(
                stdin.as_raw_fd(),
                libc::F_SETFL,
                stdin_flags | libc::O_NONBLOCK,
            )
        } != 0
    {
        return Err(abandon());
    }
    {
        let mut inner = shared.inner.lock_or_recover();
        let Some(job) = inner.jobs.get_mut(job_id) else {
            drop(inner);
            return Err(abandon());
        };
        job.state = JobState::Running;
        job.pid = Some(pid);
        job.stdin = Some(stdin);
        shared.changed.notify_all();
    }
    // The std Child handle is dropped without waiting: the waiter below owns
    // reaping through waitpid so PID clearing and reaping share one lock.
    drop(abandon);
    mirror(shared.clone(), job_id.into(), stdout, false);
    mirror(shared.clone(), job_id.into(), stderr, true);
    let waiter = shared.clone();
    let waiter_id = job_id.to_string();
    std::thread::spawn(move || reap_job(&waiter, &waiter_id, pid));

    if let Some(timeout) = timeout_ms.filter(|value| *value > 0) {
        let timed = shared.clone();
        let timed_id = job_id.to_string();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(timeout));
            let mut inner = timed.inner.lock_or_recover();
            let Some(job) = inner.jobs.get_mut(&timed_id) else {
                return;
            };
            if job.pid.is_none() {
                return;
            }
            job.timeout_requested = true;
            escalate_job(&timed, inner, &timed_id);
        });
    }
    Ok(())
}

/// The execution timeout: the same SIGINT → SIGTERM → SIGKILL escalation and
/// `STOP_STEP` waits as `stop`, for one job group. Each step is skipped once
/// the leader has been reaped, so a job that honours SIGINT sees only that.
fn escalate_job(shared: &Arc<Shared>, mut inner: MutexGuard<'_, Inner>, job_id: &str) {
    for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGKILL] {
        let Some(job) = inner.jobs.get_mut(job_id) else {
            return;
        };
        let Some(pid) = job.pid else {
            return;
        };
        job.interrupt_requested = true;
        let stopped = job.state == JobState::Stopped;
        // SAFETY: `pid` is unreaped while the lock is held.
        unsafe { libc::killpg(pid, signal) };
        if signal != libc::SIGKILL && stopped {
            // A stopped group cannot act on INT/TERM until continued.
            unsafe { libc::killpg(pid, libc::SIGCONT) };
        }
        inner = shared
            .changed
            .wait_timeout_while(inner, STOP_STEP, |state| {
                state.jobs.get(job_id).is_some_and(|job| job.pid.is_some())
            })
            .unwrap_or_else(|error| error.into_inner())
            .0;
    }
}

/// Follow one job leader until it is reaped. Stop/continue events update the
/// reported state; exit reaps the leader and clears its PID inside one `Inner`
/// critical section, so signal senders (who also hold `Inner`) never target a
/// reused PID. Between events the thread blocks in `waitid(WNOWAIT)` without
/// the lock: WNOWAIT leaves the event (and the zombie) in place for the
/// WNOHANG `waitpid` below, so the PID stays reserved until that reap.
fn reap_job(shared: &Arc<Shared>, job_id: &str, pid: i32) {
    // A `waitid` wake-up that `waitpid` then finds nothing for must not spin.
    let mut spurious = false;
    loop {
        if spurious {
            std::thread::sleep(Duration::from_millis(25));
        }
        wait_for_child_event(pid);
        {
            let mut inner = shared.inner.lock_or_recover();
            let mut status = 0;
            // SAFETY: `pid` is this runner's unreaped child; WNOHANG keeps the
            // critical section short.
            let result = unsafe {
                libc::waitpid(
                    pid,
                    &mut status,
                    libc::WNOHANG | libc::WUNTRACED | libc::WCONTINUED,
                )
            };
            spurious = result == 0;
            if result == pid {
                let terminal = !libc::WIFSTOPPED(status) && !libc::WIFCONTINUED(status);
                if let Some(job) = inner.jobs.get_mut(job_id) {
                    if libc::WIFSTOPPED(status) {
                        job.state = JobState::Stopped;
                    } else if libc::WIFCONTINUED(status) {
                        job.state = JobState::Running;
                    } else {
                        job.state = JobState::Exited;
                        job.exit_code = libc::WIFEXITED(status).then(|| libc::WEXITSTATUS(status));
                        job.signal = libc::WIFSIGNALED(status).then(|| libc::WTERMSIG(status));
                        job.ended_at = Some(now_ms());
                        job.stdin = None;
                        job.pid = None;
                    }
                }
                if terminal {
                    if inner.active.as_deref() == Some(job_id) {
                        inner.active = None;
                    }
                    shared.changed.notify_all();
                    return;
                }
                shared.changed.notify_all();
            } else if result < 0
                && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
            {
                if let Some(job) = inner.jobs.get_mut(job_id) {
                    job.state = JobState::Lost;
                    job.ended_at = Some(now_ms());
                    job.stdin = None;
                    job.pid = None;
                }
                if inner.active.as_deref() == Some(job_id) {
                    inner.active = None;
                }
                shared.changed.notify_all();
                return;
            }
        }
    }
}

/// Block until `pid` has a reportable exit/stop/continue event, without
/// consuming it. Falls back to a short sleep if `waitid` fails for a reason
/// other than EINTR, so the caller's WNOHANG `waitpid` still decides.
fn wait_for_child_event(pid: i32) {
    loop {
        // SAFETY: `info` is valid writable storage; `pid` is this runner's
        // unreaped child, and WNOWAIT never reaps it.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WSTOPPED | libc::WCONTINUED | libc::WNOWAIT,
            )
        };
        if result == 0 {
            return;
        }
        if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            std::thread::sleep(Duration::from_millis(25));
            return;
        }
    }
}

/// Make room for one new job by forgetting the oldest finished jobs. A live
/// job is never evicted; Deck retires the matching bindings in the same order.
fn evict_finished_jobs(inner: &mut Inner) -> bool {
    while inner.jobs.len() >= MAX_JOBS {
        let Some(position) = inner.order.iter().position(|id| {
            inner.active.as_deref() != Some(id.as_str())
                && inner.jobs.get(id).is_none_or(|job| !job.live())
        }) else {
            return false;
        };
        if let Some(id) = inner.order.remove(position) {
            if let Some(job) = inner.jobs.remove(&id) {
                inner.retained_output = inner.retained_output.saturating_sub(job.output.len());
            }
        }
    }
    true
}

/// Fence control, then escalate SIGINT → SIGTERM → SIGKILL against every live
/// job group, waiting a bounded time after each step. Returns true only when
/// every job leader has been reaped.
fn stop_all_jobs(shared: &Arc<Shared>) -> bool {
    let mut inner = shared.inner.lock_or_recover();
    inner.control = ControlMode::Fenced;
    inner.holder_id = None;
    for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGKILL] {
        if inner.jobs.values().all(|job| job.pid.is_none()) {
            return true;
        }
        for job in inner.jobs.values_mut() {
            if job.pid.is_some() {
                job.interrupt_requested = true;
            }
        }
        signal_live_groups(&inner, signal);
        let (guard, _) = shared
            .changed
            .wait_timeout_while(inner, STOP_STEP, |state| {
                state.jobs.values().any(|job| job.pid.is_some())
            })
            .unwrap_or_else(|error| error.into_inner());
        inner = guard;
    }
    inner.jobs.values().all(|job| job.pid.is_none())
}

fn wait_for_job(shared: &Arc<Shared>, job_id: &str, wait_ms: u64) {
    let limit = wait_ms.min(MAX_WAIT_MS);
    if limit == 0 {
        return;
    }
    let inner = shared.inner.lock_or_recover();
    let _ = shared
        .changed
        .wait_timeout_while(inner, Duration::from_millis(limit), |state| {
            state.jobs.get(job_id).is_some_and(Job::live)
        });
}

fn handle(shared: &Arc<Shared>, request: Request) -> Response {
    match request {
        Request::Ping { service_instance } => {
            let inner = shared.inner.lock_or_recover();
            let mut response = Response::empty(&shared.generation);
            response.service_current =
                service_instance.map(|value| value == shared.service_instance);
            response.runner_version = Some(VERSION);
            response.runner_build = BUILD;
            response.control = Some(if inner.control == ControlMode::Mcp {
                "mcp"
            } else if inner.control == ControlMode::Fenced {
                "fenced"
            } else {
                "human"
            });
            if let Some(active) = inner.active.as_ref().and_then(|id| inner.jobs.get(id)) {
                response.job = Some(active.view());
            }
            response
        }
        request @ Request::Exec { .. } => {
            let (job_id, request_hash, launch, cwd, wait_ms, timeout_ms, context) = match request {
                Request::Exec {
                    job_id,
                    request_hash,
                    executable,
                    args,
                    cwd,
                    wait_ms,
                    timeout_ms,
                    context,
                } => (
                    job_id,
                    request_hash,
                    OwnedLaunch::Direct { executable, args },
                    cwd,
                    wait_ms,
                    timeout_ms,
                    context,
                ),
                _ => unreachable!(),
            };
            if !valid_id(&job_id) || !valid_id(&request_hash) {
                return Response::error(&shared.generation, RunnerError::InvalidRequest);
            }
            {
                let mut inner = shared.inner.lock_or_recover();
                if let Err(error) = admit_exec_context(shared, &mut inner, &context) {
                    return Response::error(&shared.generation, error);
                }
                if let Some(existing) = inner.jobs.get(&job_id) {
                    if existing.request_hash != request_hash
                        || existing.intent_hash != context.intent_hash
                        || existing.control_epoch != context.control_epoch
                        || existing.holder_id != context.holder_id
                    {
                        return Response::error(&shared.generation, RunnerError::RequestIdConflict);
                    }
                    drop(inner);
                    wait_for_job(shared, &job_id, wait_ms);
                    let inner = shared.inner.lock_or_recover();
                    let mut response = Response::empty(&shared.generation);
                    response.job = inner.jobs.get(&job_id).map(Job::view);
                    return response;
                }
                if inner.active.is_some() {
                    return Response::error(&shared.generation, RunnerError::SessionBusy);
                }
                if !evict_finished_jobs(&mut inner) {
                    return Response::error(&shared.generation, RunnerError::CapacityExceeded);
                }
                inner.order.push_back(job_id.clone());
                inner.jobs.insert(
                    job_id.clone(),
                    Job::new(job_id.clone(), request_hash, &context),
                );
                // Starting is an active reservation, not an idle session.
                inner.active = Some(job_id.clone());
            }
            let borrowed = match &launch {
                OwnedLaunch::Direct { executable, args } => Launch::Direct { executable, args },
            };
            if let Err(error) = spawn_job(shared, &job_id, borrowed, &cwd, timeout_ms) {
                // No live process remains either way (never started, or
                // killed and reaped by `spawn_job`).
                let mut inner = shared.inner.lock_or_recover();
                if let Some(job) = inner.jobs.get_mut(&job_id) {
                    job.state = JobState::Lost;
                    job.ended_at = Some(now_ms());
                }
                if inner.active.as_deref() == Some(&job_id) {
                    inner.active = None;
                }
                return Response::error(&shared.generation, error);
            }
            wait_for_job(shared, &job_id, wait_ms);
            let inner = shared.inner.lock_or_recover();
            let mut response = Response::empty(&shared.generation);
            response.job = inner.jobs.get(&job_id).map(Job::view);
            response
        }
        Request::Read {
            job_id,
            cursor,
            max_bytes,
            wait_ms,
        } => {
            if !valid_id(&job_id) || !(1..=MAX_READ).contains(&max_bytes) {
                return Response::error(&shared.generation, RunnerError::InvalidRequest);
            }
            if wait_ms > 0 {
                let limit = wait_ms.min(MAX_WAIT_MS);
                let initial_end = {
                    let inner = shared.inner.lock_or_recover();
                    let Some(job) = inner.jobs.get(&job_id) else {
                        return Response::error(&shared.generation, RunnerError::JobNotFound);
                    };
                    job.base_cursor + job.output.len() as u64
                };
                let inner = shared.inner.lock_or_recover();
                let _ = shared.changed.wait_timeout_while(
                    inner,
                    Duration::from_millis(limit),
                    |state| {
                        state.jobs.get(&job_id).is_some_and(|job| {
                            job.live() && job.base_cursor + job.output.len() as u64 == initial_end
                        })
                    },
                );
            }
            let mut inner = shared.inner.lock_or_recover();
            let Some(job) = inner.jobs.get_mut(&job_id) else {
                return Response::error(&shared.generation, RunnerError::JobNotFound);
            };
            let expired = job.ended_at.is_some_and(|ended| {
                now_ms().saturating_sub(ended) >= shared.output_retention_ms.load(Ordering::SeqCst)
            });
            if expired && !job.output.is_empty() {
                let removed = job.output.len();
                job.output.clear();
                job.base_cursor = job.base_cursor.saturating_add(removed as u64);
                inner.retained_output = inner.retained_output.saturating_sub(removed);
            }
            let job = inner.jobs.get(&job_id).expect("job retained");
            let requested = cursor.unwrap_or(job.base_cursor);
            let gap = requested < job.base_cursor;
            if requested > job.base_cursor + job.output.len() as u64 {
                return Response::error(&shared.generation, RunnerError::OutputCursorInvalid);
            }
            let start = requested.max(job.base_cursor);
            let offset = (start - job.base_cursor) as usize;
            let candidate: Vec<u8> = job
                .output
                .iter()
                .skip(offset)
                .take(max_bytes)
                .copied()
                .collect();
            let terminal = matches!(job.state, JobState::Exited | JobState::Lost);
            let final_chunk = offset + candidate.len() == job.output.len();
            let consumed = if terminal && final_chunk {
                candidate.len()
            } else {
                complete_utf8_prefix(&candidate)
            };
            let bytes = &candidate[..consumed];
            let mut response = Response::empty(&shared.generation);
            response.job = Some(job.view());
            response.output = Some(String::from_utf8_lossy(bytes).into_owned());
            response.next_cursor = Some(start + bytes.len() as u64);
            response.gap = Some(gap);
            response.dropped_bytes = Some(start.saturating_sub(requested));
            response.deletion_reason = expired.then_some("retention-expired");
            response
        }
        Request::Input {
            job_id,
            data_b64,
            context,
        } => {
            if !valid_id(&job_id) || data_b64.len() > MAX_INPUT.saturating_mul(2) {
                return Response::error(&shared.generation, RunnerError::InvalidRequest);
            }
            let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data_b64) else {
                return Response::error(&shared.generation, RunnerError::InvalidRequest);
            };
            if bytes.len() > MAX_INPUT {
                return Response::error(&shared.generation, RunnerError::InvalidRequest);
            }
            if context.service_instance != shared.service_instance {
                return Response::error(&shared.generation, RunnerError::RunnerStale);
            }
            let mut stdin = {
                let mut inner = shared.inner.lock_or_recover();
                if inner.control != ControlMode::Mcp || inner.active.as_deref() != Some(&job_id) {
                    return Response::error(&shared.generation, RunnerError::JobNotRunning);
                }
                if let Some(job) = inner.jobs.get(&job_id) {
                    if let Err(error) = check_job_context(shared, &inner, job, &context, false) {
                        return Response::error(&shared.generation, error);
                    }
                }
                let Some(job) = inner.jobs.get_mut(&job_id) else {
                    return Response::error(&shared.generation, RunnerError::JobNotFound);
                };
                if job.state != JobState::Running {
                    return Response::error(&shared.generation, RunnerError::JobNotRunning);
                }
                let Some(stdin) = job.stdin.take() else {
                    return Response::error(&shared.generation, RunnerError::JobNotRunning);
                };
                stdin
            };
            let wrote = bounded_job_input(shared, &job_id, &context, &mut stdin, &bytes).is_ok();
            let mut inner = shared.inner.lock_or_recover();
            let still_bound = inner.control == ControlMode::Mcp
                && inner.active.as_deref() == Some(&job_id)
                && inner.control_epoch == context.control_epoch
                && inner.holder_id.as_deref() == Some(&context.holder_id)
                && inner
                    .jobs
                    .get(&job_id)
                    .is_some_and(|job| job.state == JobState::Running);
            if !wrote || !still_bound {
                return Response::error(&shared.generation, RunnerError::JobStateUnknown);
            }
            if let Some(job) = inner.jobs.get_mut(&job_id) {
                job.stdin = Some(stdin);
            }
            Response::empty(&shared.generation)
        }
        Request::Interrupt { job_id, context } => {
            let mut inner = shared.inner.lock_or_recover();
            if context.service_instance != shared.service_instance {
                return Response::error(&shared.generation, RunnerError::RunnerStale);
            }
            if inner.control != ControlMode::Mcp || inner.active.as_deref() != Some(&job_id) {
                return Response::error(&shared.generation, RunnerError::JobNotRunning);
            }
            if let Some(job) = inner.jobs.get(&job_id) {
                if let Err(error) = check_job_context(shared, &inner, job, &context, true) {
                    return Response::error(&shared.generation, error);
                }
            }
            let Some(job) = inner.jobs.get_mut(&job_id) else {
                return Response::error(&shared.generation, RunnerError::JobNotFound);
            };
            if !matches!(job.state, JobState::Running | JobState::Stopped) {
                return Response::error(&shared.generation, RunnerError::JobNotRunning);
            }
            let Some(pid) = job.pid else {
                return Response::error(&shared.generation, RunnerError::JobStateUnknown);
            };
            job.interrupt_requested = true;
            let stopped = job.state == JobState::Stopped;
            // SAFETY: `pid` is unreaped while `inner` is held (see reap_job).
            let sent = unsafe { libc::killpg(pid, libc::SIGINT) } == 0;
            if stopped {
                unsafe { libc::killpg(pid, libc::SIGCONT) };
            }
            if sent {
                Response::empty(&shared.generation)
            } else {
                Response::error(&shared.generation, RunnerError::JobStateUnknown)
            }
        }
        Request::Control {
            mode,
            service_instance,
            control_epoch,
            holder_id,
        } => {
            // Human mode only re-routes the pane keyboard to the active job's
            // stdin and enables the ^C stop key. It never starts a shell.
            {
                let mut inner = shared.inner.lock_or_recover();
                if service_instance != shared.service_instance {
                    return Response::error(&shared.generation, RunnerError::RunnerStale);
                }
                if control_epoch <= inner.control_epoch {
                    return Response::error(
                        &shared.generation,
                        RunnerError::DispatchContextInvalid,
                    );
                }
                if mode == ControlMode::Mcp && inner.active.is_some() {
                    return Response::error(&shared.generation, RunnerError::SessionBusy);
                }
                inner.control = mode;
                inner.control_epoch = control_epoch;
                inner.holder_id = holder_id;
            }
            let mut response = Response::empty(&shared.generation);
            response.control = Some(if mode == ControlMode::Mcp {
                "mcp"
            } else if mode == ControlMode::Fenced {
                "fenced"
            } else {
                "human"
            });
            response
        }
        Request::Retention {
            service_instance,
            output_retention_ms,
        } => {
            if service_instance != shared.service_instance {
                return Response::error(&shared.generation, RunnerError::RunnerStale);
            }
            if !(MIN_OUTPUT_RETENTION_MS..=MAX_OUTPUT_RETENTION_MS).contains(&output_retention_ms) {
                return Response::error(&shared.generation, RunnerError::DispatchContextInvalid);
            }
            shared
                .output_retention_ms
                .store(output_retention_ms, Ordering::SeqCst);
            Response::empty(&shared.generation)
        }
        Request::RevokeGrant {
            service_instance,
            grant_id,
            grant_version,
        } => {
            if service_instance != shared.service_instance {
                return Response::error(&shared.generation, RunnerError::RunnerStale);
            }
            if !valid_id(&grant_id) {
                return Response::error(&shared.generation, RunnerError::DispatchContextInvalid);
            }
            let mut inner = shared.inner.lock_or_recover();
            inner
                .revoked_grants
                .entry(grant_id)
                .and_modify(|version| *version = (*version).max(grant_version))
                .or_insert(grant_version);
            Response::empty(&shared.generation)
        }
        Request::AuthorizeGrant {
            service_instance,
            grant_id,
            grant_version,
            policy_version,
            expires_at,
        } => {
            if service_instance != shared.service_instance {
                return Response::error(&shared.generation, RunnerError::RunnerStale);
            }
            if !valid_id(&grant_id)
                || grant_version == 0
                || policy_version == 0
                || expires_at <= now_ms()
            {
                return Response::error(&shared.generation, RunnerError::DispatchContextInvalid);
            }
            let mut inner = shared.inner.lock_or_recover();
            if inner
                .revoked_grants
                .get(&grant_id)
                .is_some_and(|version| *version >= grant_version)
            {
                return Response::error(&shared.generation, RunnerError::DispatchContextInvalid);
            }
            if inner.authorized_grants.len() >= MAX_JOBS
                && !inner.authorized_grants.contains_key(&grant_id)
            {
                return Response::error(&shared.generation, RunnerError::CapacityExceeded);
            }
            inner.authorized_grants.insert(
                grant_id,
                AuthorizedGrant {
                    version: grant_version,
                    policy_version,
                    expires_at,
                },
            );
            Response::empty(&shared.generation)
        }
        Request::Stop { generation } => {
            if generation != shared.generation {
                return Response::error(&shared.generation, RunnerError::InvalidRequest);
            }
            let stopped = stop_all_jobs(shared);
            let mut response = if stopped {
                Response::empty(&shared.generation)
            } else {
                Response::error(&shared.generation, RunnerError::StopUnconfirmed)
            };
            response.control = Some("fenced");
            response
        }
        Request::Shutdown => {
            let mut inner = shared.inner.lock_or_recover();
            if inner.active.is_some() {
                return Response::error(&shared.generation, RunnerError::SessionBusy);
            }
            inner.stopping = true;
            Response::empty(&shared.generation)
        }
    }
}

/// Serve one request. Every accepted stream is forced to blocking I/O with
/// bounded timeouts before it is read: macOS hands a listener's O_NONBLOCK to
/// accepted sockets, and a non-blocking stream failed any request larger than
/// one socket buffer with WouldBlock.
fn serve_connection(shared: Arc<Shared>, mut stream: UnixStream) {
    if stream.set_nonblocking(false).is_err()
        || stream.set_read_timeout(Some(CONNECTION_TIMEOUT)).is_err()
        || stream.set_write_timeout(Some(CONNECTION_TIMEOUT)).is_err()
    {
        return;
    }
    let limited = stream
        .try_clone()
        .map(|copy| copy.take((MAX_REQUEST + 1) as u64));
    let response = match limited {
        Ok(reader) => {
            let mut reader = BufReader::new(reader);
            let mut line = Vec::new();
            match reader.read_until(b'\n', &mut line) {
                Ok(size) if size > 0 && size <= MAX_REQUEST && line.ends_with(b"\n") => {
                    authenticate_request(&shared, &stream, &line)
                }
                _ => Response::error(&shared.generation, RunnerError::InvalidRequest),
            }
        }
        Err(_) => Response::error(&shared.generation, RunnerError::InternalError),
    };
    if let Ok(mut bytes) = serde_json::to_vec(&response) {
        if bytes.len() + 1 > MAX_RESPONSE {
            bytes = serde_json::to_vec(&Response::error(
                &shared.generation,
                RunnerError::ResponseTooLarge,
            ))
            .unwrap_or_default();
        }
        let _ = stream.write_all(&bytes);
        let _ = stream.write_all(b"\n");
        let _ = stream.flush();
    }
}

fn auth_matches(expected: &[u8; 32], encoded: &str) -> bool {
    let Ok(actual) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(encoded) else {
        return false;
    };
    actual.len() == expected.len() && expected.ct_eq(actual.as_slice()).into()
}

#[cfg(target_os = "macos")]
fn peer_pid(stream: &UnixStream) -> Option<libc::pid_t> {
    let mut pid: libc::pid_t = 0;
    let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    // SAFETY: `pid` and `len` point to valid storage for LOCAL_PEERPID.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&mut pid as *mut libc::pid_t).cast(),
            &mut len,
        )
    };
    (result == 0 && len as usize == std::mem::size_of::<libc::pid_t>()).then_some(pid)
}

#[cfg(target_os = "linux")]
fn peer_pid(stream: &UnixStream) -> Option<libc::pid_t> {
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `credentials` and `len` point to valid storage for SO_PEERCRED.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    (result == 0 && len as usize == std::mem::size_of::<libc::ucred>()).then_some(credentials.pid)
}

fn authenticate_request(shared: &Arc<Shared>, stream: &UnixStream, line: &[u8]) -> Response {
    if let Ok(claim) = serde_json::from_slice::<ClaimRequest>(line) {
        if claim.kind == "claim"
            && claim.service_instance == shared.service_instance
            && claim.generation == shared.generation
            && peer_pid(stream) == Some(shared.deck_pid)
            && shared
                .claimed
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        {
            let mut response = Response::empty(&shared.generation);
            response.auth =
                Some(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(shared.auth_key));
            return response;
        }
    }
    let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(line) else {
        return Response::error(&shared.generation, RunnerError::AuthenticationFailed);
    };
    let Some(fields) = value.as_object_mut() else {
        return Response::error(&shared.generation, RunnerError::AuthenticationFailed);
    };
    let Some(auth) = fields
        .remove("auth")
        .and_then(|value| value.as_str().map(str::to_owned))
    else {
        return Response::error(&shared.generation, RunnerError::AuthenticationFailed);
    };
    if !auth_matches(&shared.auth_key, &auth) {
        return Response::error(&shared.generation, RunnerError::AuthenticationFailed);
    }
    match serde_json::from_value::<Request>(value) {
        Ok(request) => handle(shared, request),
        Err(_) => Response::error(&shared.generation, RunnerError::InvalidRequest),
    }
}

/// Pane keyboard → active job stdin, only in human mode. A terminal EOF (^D at
/// the start of a line) is not the end of the pane: it closes the active
/// job's stdin in human mode, and the forwarder keeps reading afterwards.
fn stdin_forwarder(shared: Arc<Shared>) {
    std::thread::spawn(move || {
        let mut input = std::io::stdin();
        let mut buffer = [0u8; 4096];
        loop {
            let count = match input.read(&mut buffer) {
                Ok(count) => count,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return,
            };
            if count == 0 {
                {
                    let mut inner = shared.inner.lock_or_recover();
                    if inner.control == ControlMode::Human {
                        if let Some(id) = inner.active.clone() {
                            if let Some(job) = inner.jobs.get_mut(&id) {
                                job.stdin = None;
                            }
                        }
                    }
                }
                // A closed (non-tty) stdin keeps returning EOF; back off.
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
            let mut inner = shared.inner.lock_or_recover();
            if inner.control != ControlMode::Human {
                continue;
            }
            let Some(id) = inner.active.clone() else {
                continue;
            };
            if let Some(stdin) = inner.jobs.get_mut(&id).and_then(|job| job.stdin.as_mut()) {
                let _ = stdin.write_all(&buffer[..count]);
                let _ = stdin.flush();
            }
        }
    });
}

type RunnerArgs = (PathBuf, String, String, u64, libc::pid_t);

fn parse_args() -> Option<RunnerArgs> {
    parse_args_from(std::env::args_os().skip(1))
}

/// Exactly the flags in `mcp-fixtures/runner-argv.json`, each once with a
/// value; anything else refuses to start.
fn parse_args_from(args: impl IntoIterator<Item = std::ffi::OsString>) -> Option<RunnerArgs> {
    let mut args = args.into_iter();
    let mut socket = None;
    let mut generation = None;
    let mut service_instance = None;
    let mut output_retention_ms = None;
    let mut deck_pid = None;
    while let Some(arg) = args.next() {
        match arg.to_str()? {
            "--socket" => socket = args.next().map(PathBuf::from),
            "--generation" => generation = args.next()?.into_string().ok(),
            "--service-instance" => service_instance = args.next()?.into_string().ok(),
            "--output-retention-ms" => {
                output_retention_ms = args.next()?.to_str()?.parse::<u64>().ok()
            }
            "--deck-pid" => deck_pid = args.next()?.to_str()?.parse::<libc::pid_t>().ok(),
            _ => return None,
        }
    }
    let socket = socket?;
    let generation = generation?;
    let service_instance = service_instance?;
    let output_retention_ms = output_retention_ms?;
    let deck_pid = deck_pid.filter(|pid| *pid > 0)?;
    if !socket.is_absolute() || !valid_id(&generation) || !valid_id(&service_instance) {
        return None;
    }
    if !(MIN_OUTPUT_RETENTION_MS..=MAX_OUTPUT_RETENTION_MS).contains(&output_retention_ms) {
        return None;
    }
    Some((
        socket,
        generation,
        service_instance,
        output_retention_ms,
        deck_pid,
    ))
}

fn ensure_private_dir(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_dir() || metadata.permissions().mode() & 0o777 != 0o700 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "runner directory is not private",
                ));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = std::fs::DirBuilder::new();
            builder.recursive(true).mode(0o700).create(path)?;
            let metadata = std::fs::symlink_metadata(path)?;
            if !metadata.file_type().is_dir() || metadata.permissions().mode() & 0o777 != 0o700 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "runner directory is not private",
                ));
            }
        }
        Err(error) => return Err(error),
    }
    Ok(())
}

fn bind_private_socket(socket: &Path) -> std::io::Result<UnixListener> {
    let parent = socket.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "socket has no parent")
    })?;
    // Keep below macOS SUN_LEN while allowing concurrent runners in the
    // shared private directory. Production names begin with a random
    // generation, so this bounded prefix is independently collision-resistant.
    let prefix = socket
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("runner")
        .chars()
        .take(12)
        .collect::<String>();
    let temporary = parent.join(format!(".{prefix}"));
    let _ = std::fs::remove_file(&temporary);
    let listener = UnixListener::bind(&temporary)?;
    if let Err(error) = std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))
        .and_then(|_| std::fs::rename(&temporary, socket))
    {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(listener)
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;

    fn shared() -> Arc<Shared> {
        Arc::new(Shared {
            generation: "g_test".into(),
            service_instance: "svc_current".into(),
            output_retention_ms: AtomicU64::new(60_000),
            connections: AtomicUsize::new(0),
            inner: Mutex::new(Inner {
                jobs: HashMap::new(),
                order: VecDeque::new(),
                active: None,
                control: ControlMode::Fenced,
                control_epoch: 0,
                holder_id: None,
                revoked_grants: HashMap::new(),
                authorized_grants: HashMap::from([(
                    "grant_test".into(),
                    AuthorizedGrant {
                        version: 1,
                        policy_version: 2,
                        expires_at: u64::MAX,
                    },
                )]),
                stopping: false,
                retained_output: 0,
            }),
            changed: Condvar::new(),
            auth_key: [7; 32],
            deck_pid: std::process::id() as libc::pid_t,
            claimed: std::sync::atomic::AtomicBool::new(false),
        })
    }

    #[test]
    fn private_directory_creation_never_repairs_a_lax_existing_directory() {
        let root = std::env::temp_dir().join(format!(
            "deck-mcp-runner-private-dir-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        ensure_private_dir(&root).unwrap();
        assert_eq!(
            std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            ensure_private_dir(&root).unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::remove_dir(&root).unwrap();
    }

    fn context(service: &str, epoch: u64, holder: &str) -> DispatchContext {
        DispatchContext {
            service_instance: service.into(),
            holder_id: holder.into(),
            control_epoch: epoch,
            grant_id: "grant_test".into(),
            grant_version: 1,
            policy_version: 2,
            intent_hash: "a".repeat(64),
            expires_at: u64::MAX,
        }
    }

    #[test]
    fn stale_epoch_and_old_service_are_fenced_before_spawn() {
        let shared = shared();
        assert!(
            handle(
                &shared,
                Request::Control {
                    mode: ControlMode::Mcp,
                    service_instance: "svc_current".into(),
                    control_epoch: 1,
                    holder_id: Some("holder_new".into()),
                }
            )
            .ok
        );
        let stale = handle(
            &shared,
            Request::Exec {
                job_id: "job_stale".into(),
                request_hash: "b".repeat(64),
                executable: "/usr/bin/true".into(),
                args: vec![],
                cwd: "/tmp".into(),
                wait_ms: 0,
                timeout_ms: None,
                context: context("svc_current", 0, "holder_old"),
            },
        );
        assert_eq!(stale.error, Some("control-revoked"));
        let old_service = handle(
            &shared,
            Request::Exec {
                job_id: "job_old_service".into(),
                request_hash: "c".repeat(64),
                executable: "/usr/bin/true".into(),
                args: vec![],
                cwd: "/tmp".into(),
                wait_ms: 0,
                timeout_ms: None,
                context: context("svc_previous", 1, "holder_new"),
            },
        );
        assert_eq!(old_service.error, Some("runner-stale"));
        assert!(
            handle(
                &shared,
                Request::RevokeGrant {
                    service_instance: "svc_current".into(),
                    grant_id: "grant_test".into(),
                    grant_version: 1,
                }
            )
            .ok
        );
        let revoked = handle(
            &shared,
            Request::Exec {
                job_id: "job_revoked".into(),
                request_hash: "e".repeat(64),
                executable: "/usr/bin/true".into(),
                args: vec![],
                cwd: "/tmp".into(),
                wait_ms: 0,
                timeout_ms: None,
                context: context("svc_current", 1, "holder_new"),
            },
        );
        assert_eq!(revoked.error, Some("dispatch-context-invalid"));
        assert!(shared.inner.lock_or_recover().jobs.is_empty());
    }

    #[test]
    fn human_takeover_never_starts_a_shell_job() {
        let shared = shared();
        let human = handle(
            &shared,
            Request::Control {
                mode: ControlMode::Human,
                service_instance: "svc_current".into(),
                control_epoch: 1,
                holder_id: None,
            },
        );
        assert!(human.ok);
        let inner = shared.inner.lock_or_recover();
        assert!(inner.jobs.is_empty(), "takeover must not create a job");
        assert!(inner.active.is_none());
        assert!(inner.control == ControlMode::Human);
    }

    #[test]
    fn control_epoch_starts_at_zero_and_only_moves_forward() {
        let shared = shared();
        assert_eq!(shared.inner.lock_or_recover().control_epoch, 0);
        let forward = handle(
            &shared,
            Request::Control {
                mode: ControlMode::Mcp,
                service_instance: "svc_current".into(),
                control_epoch: 4,
                holder_id: Some("holder_new".into()),
            },
        );
        assert!(forward.ok);
        for epoch in [4, 3, 0] {
            let rejected = handle(
                &shared,
                Request::Control {
                    mode: ControlMode::Human,
                    service_instance: "svc_current".into(),
                    control_epoch: epoch,
                    holder_id: None,
                },
            );
            assert_eq!(rejected.error, Some("dispatch-context-invalid"));
        }
        let inner = shared.inner.lock_or_recover();
        assert_eq!(inner.control_epoch, 4);
        assert!(inner.control == ControlMode::Mcp);
        assert_eq!(inner.holder_id.as_deref(), Some("holder_new"));
    }

    #[test]
    fn a_foreign_service_instance_is_reported_stale() {
        let shared = shared();
        let control = handle(
            &shared,
            Request::Control {
                mode: ControlMode::Mcp,
                service_instance: "svc_restarted".into(),
                control_epoch: 9,
                holder_id: Some("holder_a".into()),
            },
        );
        assert_eq!(control.error, Some("runner-stale"));
        let retention = handle(
            &shared,
            Request::Retention {
                service_instance: "svc_restarted".into(),
                output_retention_ms: 60_000,
            },
        );
        assert_eq!(retention.error, Some("runner-stale"));
        assert!(shared.inner.lock_or_recover().control == ControlMode::Fenced);
    }

    #[test]
    fn finished_jobs_are_evicted_but_live_jobs_are_not() {
        let shared = shared();
        let mut inner = shared.inner.lock_or_recover();
        for index in 0..MAX_JOBS {
            let id = format!("job_{index}");
            let mut job = Job::new(id.clone(), "hash".into(), &context("svc_current", 1, "h"));
            job.state = if index == 0 {
                JobState::Running
            } else {
                JobState::Exited
            };
            inner.order.push_back(id.clone());
            inner.jobs.insert(id, job);
        }
        inner.active = Some("job_0".into());
        assert!(evict_finished_jobs(&mut inner));
        assert!(inner.jobs.contains_key("job_0"), "the live job survives");
        assert!(
            !inner.jobs.contains_key("job_1"),
            "the oldest finished job goes"
        );
        for job in inner.jobs.values_mut() {
            job.state = JobState::Running;
        }
        assert_eq!(inner.jobs.len(), MAX_JOBS - 1);
        let id = "job_extra".to_string();
        inner.order.push_back(id.clone());
        inner.jobs.insert(
            id,
            Job::new(
                "job_extra".into(),
                "hash".into(),
                &context("svc_current", 1, "h"),
            ),
        );
        assert!(
            !evict_finished_jobs(&mut inner),
            "live jobs are never evicted"
        );
    }

    #[test]
    fn duplicate_dispatch_returns_one_job() {
        let shared = shared();
        assert!(
            handle(
                &shared,
                Request::Control {
                    mode: ControlMode::Mcp,
                    service_instance: "svc_current".into(),
                    control_epoch: 1,
                    holder_id: Some("holder_a".into()),
                }
            )
            .ok
        );
        let context = context("svc_current", 1, "holder_a");
        let request = || Request::Exec {
            job_id: "job_once".into(),
            request_hash: "d".repeat(64),
            executable: "/usr/bin/true".into(),
            args: vec![],
            cwd: "/tmp".into(),
            wait_ms: 1_000,
            timeout_ms: None,
            context: context.clone(),
        };
        let first = handle(&shared, request());
        assert!(first.ok, "{:?}", first.error);
        let second = handle(&shared, request());
        assert!(second.ok, "{:?}", second.error);
        assert_eq!(shared.inner.lock_or_recover().jobs.len(), 1);
    }

    #[test]
    fn relative_executables_are_rejected_before_spawn() {
        let shared = shared();
        assert_eq!(
            spawn_job(
                &shared,
                "job_relative",
                Launch::Direct {
                    executable: "git",
                    args: &[],
                },
                "/tmp",
                None,
            ),
            Err(RunnerError::InvalidExecutable)
        );
    }

    /// `spawn-failed` is reported only when the process never started, which
    /// is what lets Deck journal it as a plain rejection.
    #[test]
    fn a_missing_executable_is_spawn_failed_before_any_process() {
        let shared = shared();
        assert_eq!(
            spawn_job(
                &shared,
                "job_missing",
                Launch::Direct {
                    executable: "/nonexistent/deck-runner-test",
                    args: &[],
                },
                "/tmp",
                None,
            ),
            Err(RunnerError::SpawnFailed)
        );
    }

    #[test]
    fn expired_output_reports_a_gap_without_deleting_job_metadata() {
        let shared = shared();
        shared.output_retention_ms.store(1, Ordering::SeqCst);
        {
            let mut inner = shared.inner.lock_or_recover();
            inner.jobs.insert(
                "job_expired".into(),
                Job {
                    id: "job_expired".into(),
                    request_hash: "request".into(),
                    holder_id: "holder_a".into(),
                    control_epoch: 1,
                    intent_hash: "a".repeat(64),
                    grant_id: "grant_test".into(),
                    grant_version: 1,
                    state: JobState::Exited,
                    exit_code: Some(0),
                    signal: None,
                    started_at: now_ms().saturating_sub(10),
                    ended_at: Some(now_ms().saturating_sub(10)),
                    interrupt_requested: false,
                    timeout_requested: false,
                    output: VecDeque::from(b"secret tail".to_vec()),
                    base_cursor: 0,
                    pid: None,
                    stdin: None,
                    stdout_eof: true,
                    stderr_eof: true,
                },
            );
            inner.retained_output = 11;
        }
        let response = handle(
            &shared,
            Request::Read {
                job_id: "job_expired".into(),
                cursor: Some(0),
                max_bytes: 100,
                wait_ms: 0,
            },
        );
        assert!(response.ok);
        assert_eq!(response.output.as_deref(), Some(""));
        assert_eq!(response.gap, Some(true));
        assert_eq!(response.deletion_reason, Some("retention-expired"));
        assert!(response.job.is_some());
    }

    fn finished_job(id: &str, state: JobState, output: &[u8]) -> Job {
        Job {
            id: id.into(),
            request_hash: "request".into(),
            holder_id: "holder_a".into(),
            control_epoch: 1,
            intent_hash: "a".repeat(64),
            grant_id: "grant_test".into(),
            grant_version: 1,
            state,
            exit_code: None,
            signal: None,
            started_at: now_ms(),
            ended_at: None,
            interrupt_requested: false,
            timeout_requested: false,
            output: VecDeque::from(output.to_vec()),
            base_cursor: 0,
            pid: None,
            stdin: None,
            stdout_eof: true,
            stderr_eof: true,
        }
    }

    /// A cursor read hands out whole UTF-8 units only: an incomplete final
    /// unit waits for the next page, a genuinely invalid byte is consumed so
    /// the stream still advances.
    #[test]
    fn utf8_prefix_defers_an_incomplete_unit_and_advances_past_invalid_bytes() {
        assert_eq!(complete_utf8_prefix(b"abc"), 3);
        assert_eq!(complete_utf8_prefix(b""), 0);
        assert_eq!(complete_utf8_prefix(b"ab\xe2\x82"), 2);
        assert_eq!(complete_utf8_prefix(b"a\xffb"), 3);
        assert_eq!(complete_utf8_prefix(b"\xff\xfe\xe2\x82"), 2);
        assert_eq!(default_read(), MAX_READ);
    }

    /// The per-runner key is compared in constant time and only as a whole,
    /// and the signals a pane can raise never stop or kill the runner.
    #[test]
    fn runner_auth_accepts_only_the_exact_key_and_the_signal_set_covers_terminal_stops() {
        let key = [7u8; 32];
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key);
        assert!(auth_matches(&key, &encoded));
        assert!(!auth_matches(&[8u8; 32], &encoded));
        assert!(!auth_matches(&key, "not base64!"));
        assert!(!auth_matches(
            &key,
            &base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([7u8; 16])
        ));
        let set = runner_signal_set();
        for signal in [
            libc::SIGINT,
            libc::SIGQUIT,
            libc::SIGTSTP,
            libc::SIGHUP,
            libc::SIGTERM,
        ] {
            // SAFETY: `set` was initialized by `runner_signal_set`.
            assert_eq!(unsafe { libc::sigismember(&set, signal) }, 1, "{signal}");
        }
        // SAFETY: as above.
        assert_eq!(unsafe { libc::sigismember(&set, libc::SIGUSR1) }, 0);
    }

    /// The runner socket is bound under a private temporary name, made 0600
    /// and renamed into place, so no peer ever sees a permissive socket.
    #[test]
    fn private_socket_is_published_0600_under_its_final_name_only() {
        // Short names: the published path must stay below macOS SUN_LEN
        // under a long TMPDIR, as production runner names do.
        let root = std::env::temp_dir().join(format!("deck-mcpr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        ensure_private_dir(&root).unwrap();
        let socket = root.join("gen-abcdefghijklmnop.sock");
        let listener = bind_private_socket(&socket).unwrap();
        assert_eq!(
            std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let names: Vec<String> = std::fs::read_dir(&root)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            ["gen-abcdefghijklmnop.sock"],
            "no temporary name survives"
        );
        assert!(UnixStream::connect(&socket).is_ok());
        drop(listener);
        assert_eq!(
            bind_private_socket(Path::new("/")).unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Every request is fenced by service instance, control epoch and job
    /// state before it has any effect, and the runner reports exactly what it
    /// applied.
    #[test]
    fn requests_are_fenced_by_service_epoch_and_job_state_before_any_side_effect() {
        let shared = shared();
        let ping = handle(
            &shared,
            Request::Ping {
                service_instance: Some("svc_current".into()),
            },
        );
        assert!(ping.ok);
        assert_eq!(ping.service_current, Some(true));
        assert_eq!(ping.control, Some("fenced"));
        assert_eq!(ping.runner_version, Some(VERSION));
        assert!(ping.job.is_none());
        assert_eq!(
            handle(
                &shared,
                Request::Ping {
                    service_instance: Some("svc_old".into()),
                },
            )
            .service_current,
            Some(false)
        );
        assert_eq!(
            handle(
                &shared,
                Request::Ping {
                    service_instance: None,
                },
            )
            .service_current,
            None
        );

        let retention = |service: &str, ms: u64| {
            handle(
                &shared,
                Request::Retention {
                    service_instance: service.into(),
                    output_retention_ms: ms,
                },
            )
        };
        assert_eq!(retention("svc_old", 60_000).error, Some("runner-stale"));
        assert_eq!(
            retention("svc_current", 59_999).error,
            Some("dispatch-context-invalid")
        );
        assert!(retention("svc_current", 120_000).ok);
        assert_eq!(shared.output_retention_ms.load(Ordering::SeqCst), 120_000);

        let revoke = |service: &str, grant: &str, version: u64| {
            handle(
                &shared,
                Request::RevokeGrant {
                    service_instance: service.into(),
                    grant_id: grant.into(),
                    grant_version: version,
                },
            )
        };
        assert_eq!(
            revoke("svc_old", "grant_old", 1).error,
            Some("runner-stale")
        );
        assert_eq!(
            revoke("svc_current", "bad id", 1).error,
            Some("dispatch-context-invalid")
        );
        assert!(revoke("svc_current", "grant_old", 3).ok);
        assert!(revoke("svc_current", "grant_old", 2).ok);
        assert_eq!(
            shared.inner.lock_or_recover().revoked_grants["grant_old"],
            3,
            "a revocation never lowers the fenced version"
        );
        let authorize = |service: &str, grant: &str, version: u64, policy: u64| {
            handle(
                &shared,
                Request::AuthorizeGrant {
                    service_instance: service.into(),
                    grant_id: grant.into(),
                    grant_version: version,
                    policy_version: policy,
                    expires_at: u64::MAX,
                },
            )
        };
        assert_eq!(
            authorize("svc_old", "grant_test", 4, 2).error,
            Some("runner-stale")
        );
        assert_eq!(
            authorize("svc_current", "grant_old", 3, 2).error,
            Some("dispatch-context-invalid"),
            "a revoked version is never re-authorized"
        );
        assert_eq!(
            authorize("svc_current", "grant_new", 1, 0).error,
            Some("dispatch-context-invalid")
        );
        assert!(authorize("svc_current", "grant_test", 4, 2).ok);
        assert_eq!(
            shared.inner.lock_or_recover().authorized_grants["grant_test"].version,
            4
        );

        let read = |job: &str, cursor: Option<u64>, max_bytes: usize, wait_ms: u64| {
            handle(
                &shared,
                Request::Read {
                    job_id: job.into(),
                    cursor,
                    max_bytes,
                    wait_ms,
                },
            )
        };
        assert_eq!(read("bad id", None, 10, 0).error, Some("invalid-request"));
        assert_eq!(read("job_x", None, 0, 0).error, Some("invalid-request"));
        assert_eq!(read("job_x", None, 10, 0).error, Some("job-not-found"));
        assert_eq!(
            read("job_x", None, 10, 50).error,
            Some("job-not-found"),
            "a wait on an unknown job returns at once"
        );
        shared.inner.lock_or_recover().jobs.insert(
            "job_done".into(),
            finished_job("job_done", JobState::Exited, b"done\n"),
        );
        assert_eq!(
            read("job_done", Some(99), 10, 0).error,
            Some("output-cursor-invalid")
        );
        let first = read("job_done", None, 2, 0);
        assert!(first.ok);
        assert_eq!(first.output.as_deref(), Some("do"));
        assert_eq!(first.next_cursor, Some(2));
        assert_eq!(first.gap, Some(false));
        let rest = read("job_done", Some(2), 10, 20);
        assert_eq!(rest.output.as_deref(), Some("ne\n"));
        assert_eq!(rest.next_cursor, Some(5));
        assert_eq!(rest.job.as_ref().map(|job| job.state), Some("exited"));
        assert!(rest.job.as_ref().is_some_and(|job| job.output_complete));

        let input = |job: &str, data: &str, service: &str| {
            handle(
                &shared,
                Request::Input {
                    job_id: job.into(),
                    data_b64: data.into(),
                    context: context(service, 1, "holder_a"),
                },
            )
        };
        let encoded = base64::engine::general_purpose::STANDARD.encode(b"x");
        assert_eq!(
            input("job_done", "***", "svc_current").error,
            Some("invalid-request")
        );
        assert_eq!(
            input("job_done", &encoded, "svc_old").error,
            Some("runner-stale")
        );
        assert_eq!(
            input("job_done", &encoded, "svc_current").error,
            Some("job-not-running"),
            "fenced control accepts no stdin"
        );
        let interrupt = |job: &str, service: &str| {
            handle(
                &shared,
                Request::Interrupt {
                    job_id: job.into(),
                    context: context(service, 1, "holder_a"),
                },
            )
        };
        assert_eq!(interrupt("job_done", "svc_old").error, Some("runner-stale"));
        assert_eq!(
            interrupt("job_done", "svc_current").error,
            Some("job-not-running")
        );

        let control = |mode: ControlMode, service: &str, epoch: u64| {
            handle(
                &shared,
                Request::Control {
                    mode,
                    service_instance: service.into(),
                    control_epoch: epoch,
                    holder_id: Some("holder_a".into()),
                },
            )
        };
        assert_eq!(
            control(ControlMode::Mcp, "svc_old", 1).error,
            Some("runner-stale")
        );
        let human = control(ControlMode::Human, "svc_current", 1);
        assert!(human.ok);
        assert_eq!(human.control, Some("human"));
        assert_eq!(
            control(ControlMode::Mcp, "svc_current", 1).error,
            Some("dispatch-context-invalid"),
            "an equal epoch never re-enters"
        );
        {
            let mut inner = shared.inner.lock_or_recover();
            inner.control = ControlMode::Mcp;
            inner.active = Some("job_done".into());
        }
        assert_eq!(
            control(ControlMode::Mcp, "svc_current", 2).error,
            Some("session-busy")
        );
        assert_eq!(
            handle(&shared, Request::Shutdown).error,
            Some("session-busy")
        );
        assert_eq!(
            input("job_done", &encoded, "svc_current").error,
            Some("job-not-running"),
            "an exited job takes no stdin even while it is the active reservation"
        );
        assert_eq!(
            interrupt("job_done", "svc_current").error,
            Some("job-not-running")
        );
        assert_eq!(
            interrupt("job_other", "svc_current").error,
            Some("job-not-running"),
            "only the active job can be interrupted"
        );
        {
            let mut inner = shared.inner.lock_or_recover();
            inner.jobs.insert(
                "job_live".into(),
                finished_job("job_live", JobState::Running, b""),
            );
            inner.active = Some("job_live".into());
        }
        assert_eq!(
            interrupt("job_live", "svc_current").error,
            Some("job-state-unknown"),
            "a running job without a group cannot be signalled"
        );
        assert_eq!(
            input("job_live", &encoded, "svc_current").error,
            Some("job-not-running"),
            "a running job whose stdin is closed takes no more input"
        );
        assert_eq!(
            handle(
                &shared,
                Request::Interrupt {
                    job_id: "job_live".into(),
                    context: context("svc_current", 1, "holder_b"),
                },
            )
            .error,
            Some("control-revoked")
        );
        shared.inner.lock_or_recover().active = None;

        assert_eq!(
            handle(
                &shared,
                Request::Stop {
                    generation: "g_other".into(),
                },
            )
            .error,
            Some("invalid-request")
        );
        let stop = handle(
            &shared,
            Request::Stop {
                generation: "g_test".into(),
            },
        );
        assert!(stop.ok);
        assert_eq!(stop.control, Some("fenced"));
        {
            let inner = shared.inner.lock_or_recover();
            assert!(inner.control == ControlMode::Fenced);
            assert_eq!(inner.holder_id, None);
            assert!(!inner.stopping);
        }
        assert!(handle(&shared, Request::Shutdown).ok);
        assert!(shared.inner.lock_or_recover().stopping);
    }

    /// A job's dispatch context must match the job and the session on every
    /// axis; only reads and interrupts tolerate an expired or revoked grant.
    #[test]
    fn job_context_checks_every_binding_axis_and_views_report_each_state() {
        let shared = shared();
        {
            let mut inner = shared.inner.lock_or_recover();
            inner.control = ControlMode::Mcp;
            inner.control_epoch = 1;
            inner.holder_id = Some("holder_a".into());
            inner.revoked_grants.insert("grant_test".into(), 1);
        }
        let inner = shared.inner.lock_or_recover();
        let job = finished_job("job_ctx", JobState::Running, b"");
        let current = context("svc_current", 1, "holder_a");
        assert_eq!(
            check_job_context(&shared, &inner, &job, &current, false),
            Err(RunnerError::ControlRevoked),
            "a revoked grant blocks stdin"
        );
        assert_eq!(
            check_job_context(&shared, &inner, &job, &current, true),
            Ok(()),
            "reads and interrupts still reach a job whose grant was revoked"
        );
        let mut expired = current.clone();
        expired.expires_at = 1;
        assert_eq!(
            check_job_context(&shared, &inner, &job, &expired, true),
            Ok(())
        );
        assert_eq!(
            check_job_context(&shared, &inner, &job, &expired, false),
            Err(RunnerError::ControlRevoked)
        );
        assert_eq!(
            check_job_context(
                &shared,
                &inner,
                &job,
                &context("svc_stale", 1, "holder_a"),
                true
            ),
            Err(RunnerError::RunnerStale)
        );
        for foreign in [
            context("svc_current", 2, "holder_a"),
            context("svc_current", 1, "holder_b"),
            DispatchContext {
                intent_hash: "b".repeat(64),
                ..current.clone()
            },
            DispatchContext {
                grant_version: 2,
                ..current.clone()
            },
        ] {
            assert_eq!(
                check_job_context(&shared, &inner, &job, &foreign, true),
                Err(RunnerError::ControlRevoked)
            );
        }
        assert_eq!(signal_live_groups(&inner, libc::SIGINT), 0);
        drop(inner);

        for (state, word, live) in [
            (JobState::Starting, "starting", true),
            (JobState::Running, "running", true),
            (JobState::Stopped, "stopped", true),
            (JobState::Exited, "exited", false),
            (JobState::Lost, "lost", false),
        ] {
            let job = finished_job("job_view", state, b"abc");
            let view = job.view();
            assert_eq!(view.state, word);
            assert_eq!(job.live(), live);
            assert_eq!((view.base_cursor, view.end_cursor), (0, 3));
        }

        // Retained output is bounded per job: the oldest bytes fall off the
        // front and the base cursor records how many did.
        shared.inner.lock_or_recover().jobs.insert(
            "job_big".into(),
            finished_job("job_big", JobState::Running, b""),
        );
        append_output(&shared, "job_big", &vec![b'x'; RETAINED_OUTPUT]);
        append_output(&shared, "job_big", b"yz");
        append_output(&shared, "job_missing", b"ignored");
        let inner = shared.inner.lock_or_recover();
        let job = &inner.jobs["job_big"];
        assert_eq!(job.output.len(), RETAINED_OUTPUT);
        assert_eq!(job.base_cursor, 2);
        assert_eq!(job.output.back(), Some(&b'z'));
        assert_eq!(inner.retained_output, RETAINED_OUTPUT);
    }

    macro_rules! mcp_fixture {
        ($name:literal) => {
            serde_json::from_str::<serde_json::Value>(include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../mcp-fixtures/",
                $name
            )))
            .unwrap()
        };
    }

    /// `mcp-fixtures/` is shared with Deck and the adapter; this runner
    /// compares it with its own constants only.
    #[test]
    fn limits_match_the_shared_fixture() {
        let limits = mcp_fixture!("limits.json");
        for (key, value) in [
            ("max_request_bytes", MAX_REQUEST as u64),
            ("max_response_bytes", MAX_RESPONSE as u64),
            ("max_executable", MAX_EXECUTABLE as u64),
            ("max_arguments", MAX_ARGUMENTS as u64),
            ("max_argument_bytes", MAX_ARGUMENT_BYTES as u64),
            ("max_read_bytes", MAX_READ as u64),
            ("max_input_bytes", MAX_INPUT as u64),
            ("wait_ms_max", MAX_WAIT_MS),
            ("output_retention_ms_min", MIN_OUTPUT_RETENTION_MS),
            ("output_retention_ms_max", MAX_OUTPUT_RETENTION_MS),
        ] {
            assert_eq!(limits[key].as_u64(), Some(value), "limits.json {key}");
        }
        assert_eq!(limits["max_response_bytes_includes_newline"], true);
    }

    #[test]
    fn every_emitted_error_is_listed_in_the_shared_fixture() {
        let fixture = mcp_fixture!("runner-errors.json");
        let mut listed: Vec<&str> = fixture["errors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["error"].as_str().unwrap())
            .collect();
        let mut emitted: Vec<&str> = RunnerError::ALL.iter().map(|e| e.as_str()).collect();
        listed.sort_unstable();
        emitted.sort_unstable();
        assert_eq!(emitted, listed);
    }

    #[test]
    fn parse_args_accepts_exactly_the_fixture_flags() {
        let fixture = mcp_fixture!("runner-argv.json");
        let flags: Vec<&str> = fixture["flags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|flag| flag.as_str().unwrap())
            .collect();
        let value = |flag: &str| match flag {
            "--socket" => "/tmp/deck-runner.sock",
            "--deck-pid" => "42",
            "--output-retention-ms" => "60000",
            _ => "valid_id",
        };
        let argv = |extra: &[&str]| -> Vec<std::ffi::OsString> {
            flags
                .iter()
                .flat_map(|flag| [*flag, value(flag)])
                .chain(extra.iter().copied())
                .map(Into::into)
                .collect()
        };
        assert!(parse_args_from(argv(&[])).is_some());
        assert!(parse_args_from(argv(&["--extra", "x"])).is_none());
        for missing in &flags {
            let partial: Vec<std::ffi::OsString> = flags
                .iter()
                .filter(|flag| flag != &missing)
                .flat_map(|flag| [*flag, value(flag)])
                .map(Into::into)
                .collect();
            assert!(parse_args_from(partial).is_none(), "{missing} is required");
        }
    }
}

fn main() {
    let Some((socket, generation, service_instance, output_retention_ms, deck_pid)) = parse_args()
    else {
        std::process::exit(64);
    };
    let Some(parent) = socket.parent() else {
        std::process::exit(64);
    };
    if ensure_private_dir(parent).is_err() {
        std::process::exit(73);
    }
    let _ = std::fs::remove_file(&socket);
    let listener = match bind_private_socket(&socket) {
        Ok(listener) => listener,
        Err(_) => std::process::exit(73),
    };
    // Block terminal/lifecycle signals before any thread exists so every
    // runner thread inherits the mask; `signal_thread` consumes them.
    let signals = runner_signal_set();
    // SAFETY: `signals` is initialized; this runs before any thread spawn.
    if unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &signals, std::ptr::null_mut()) } != 0 {
        let _ = std::fs::remove_file(&socket);
        std::process::exit(71);
    }
    let mut auth_key = [0u8; 32];
    if SystemRandom::new().fill(&mut auth_key).is_err() {
        let _ = std::fs::remove_file(&socket);
        std::process::exit(71);
    }
    let shared = Arc::new(Shared {
        generation,
        service_instance,
        output_retention_ms: AtomicU64::new(output_retention_ms),
        connections: AtomicUsize::new(0),
        inner: Mutex::new(Inner {
            jobs: HashMap::new(),
            order: VecDeque::new(),
            active: None,
            control: ControlMode::Fenced,
            control_epoch: 0,
            holder_id: None,
            revoked_grants: HashMap::new(),
            authorized_grants: HashMap::new(),
            stopping: false,
            retained_output: 0,
        }),
        changed: Condvar::new(),
        auth_key,
        deck_pid,
        claimed: std::sync::atomic::AtomicBool::new(false),
    });
    signal_thread(shared.clone(), socket.clone());
    stdin_forwarder(shared.clone());
    println!("Deck MCP managed shell ready");
    // Blocking accept: an idle runner sleeps in the kernel instead of waking
    // on a timer. `Shutdown` is answered on its own connection, which then
    // ends the process itself (the accept below never returns for it).
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                // Only the launching Deck process is a control client. Any
                // other peer (for example a same-uid job of this pane) is
                // closed before it can occupy a connection slot, so it can
                // never starve Deck's `control`/`stop` requests.
                if peer_pid(&stream) != Some(shared.deck_pid) {
                    drop(stream);
                    continue;
                }
                // Bounded concurrency: excess connections are closed at once.
                if shared.connections.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                    shared.connections.fetch_sub(1, Ordering::SeqCst);
                    drop(stream);
                    continue;
                }
                let state = shared.clone();
                let socket = socket.clone();
                std::thread::spawn(move || {
                    serve_connection(state.clone(), stream);
                    state.connections.fetch_sub(1, Ordering::SeqCst);
                    if state.inner.lock_or_recover().stopping {
                        let _ = std::fs::remove_file(&socket);
                        std::process::exit(0);
                    }
                });
            }
            // EINTR/ECONNABORTED and friends: accept again after a short
            // pause so a persistent error cannot spin.
            Err(_) => std::thread::sleep(Duration::from_millis(250)),
        }
    }
}
