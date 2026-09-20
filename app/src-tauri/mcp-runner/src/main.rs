//! Visible execution host for one Deck MCP tmux pane.
//!
//! The runner is a signed bundle sidecar and is started only as the pane
//! process of an explicitly created MCP session. It accepts user scripts over
//! a user-only Unix socket, executes each job in a fresh zsh process, mirrors
//! combined output into the real pane, and retains a bounded copy for cursor
//! reads. Script bytes never appear in argv, the environment, or a temporary
//! file. Fresh jobs receive the versioned sanitized developer environment,
//! not the runner's complete host environment. Process exit and each output
//! pipe's EOF are reported separately. The runner has no model, persistence,
//! or Board authority; trusted-host jobs retain the user's ordinary OS/network
//! permissions and the environment profile is not a sandbox.

use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const PROTOCOL: u32 = 1;
const MAX_REQUEST: usize = 256 * 1024;
const MAX_SCRIPT: usize = 32 * 1024;
const MAX_READ: usize = 16 * 1024;
const MAX_RESPONSE: usize = 128 * 1024;
const MAX_INPUT: usize = 32 * 1024;
const RETAINED_OUTPUT: usize = 1024 * 1024;
const RETAINED_OUTPUT_PER_SESSION: usize = 16 * 1024 * 1024;
const MAX_JOBS: usize = 256;
const SCRIPT_WRITE_TIMEOUT: Duration = Duration::from_secs(2);
const INPUT_WRITE_TIMEOUT: Duration = Duration::from_secs(1);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis() as u64)
        .unwrap_or(0)
}

trait Recover<T> {
    fn recover(self) -> T;
}
impl<'a, T> Recover<MutexGuard<'a, T>> for std::sync::LockResult<MutexGuard<'a, T>> {
    fn recover(self) -> MutexGuard<'a, T> {
        self.unwrap_or_else(|error| error.into_inner())
    }
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum Request {
    Ping,
    Exec {
        job_id: String,
        request_hash: String,
        script: String,
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
    Shutdown,
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
    protocol: u32,
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
}

impl Response {
    fn error(generation: &str, error: &'static str) -> Self {
        Self {
            ok: false,
            protocol: PROTOCOL,
            generation: generation.into(),
            error: Some(error),
            job: None,
            output: None,
            next_cursor: None,
            gap: None,
            dropped_bytes: None,
            control: None,
            deletion_reason: None,
        }
    }
    fn empty(generation: &str) -> Self {
        Self {
            ok: true,
            protocol: PROTOCOL,
            generation: generation.into(),
            error: None,
            job: None,
            output: None,
            next_cursor: None,
            gap: None,
            dropped_bytes: None,
            control: None,
            deletion_reason: None,
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
    child: Option<Arc<Mutex<Child>>>,
    stdin: Option<ChildStdin>,
    stdout_eof: bool,
    stderr_eof: bool,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum JobState {
    Starting,
    Running,
    Exited,
    Lost,
}

impl Job {
    fn view(&self) -> JobView {
        JobView {
            job_id: self.id.clone(),
            state: match self.state {
                JobState::Starting => "starting",
                JobState::Running => "running",
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
    stopping: bool,
    retained_output: usize,
}

struct Shared {
    generation: String,
    service_instance: String,
    output_retention_ms: AtomicU64,
    initial_cwd: PathBuf,
    inner: Mutex<Inner>,
    changed: Condvar,
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
) -> Result<(), &'static str> {
    if context.service_instance != shared.service_instance
        || !valid_id(&context.holder_id)
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
        return Err("dispatch-context-invalid");
    }
    if inner.control != ControlMode::Mcp || context.control_epoch < inner.control_epoch {
        return Err("control-revoked");
    }
    if context.control_epoch == inner.control_epoch
        && inner
            .holder_id
            .as_deref()
            .is_some_and(|holder| holder != context.holder_id)
    {
        return Err("holder-conflict");
    }
    if context.control_epoch > inner.control_epoch || inner.holder_id.is_none() {
        inner.control_epoch = context.control_epoch;
        inner.holder_id = Some(context.holder_id.clone());
    }
    Ok(())
}

fn check_job_context(
    shared: &Shared,
    inner: &Inner,
    job: &Job,
    context: &DispatchContext,
    allow_expired_or_revoked: bool,
) -> Result<(), &'static str> {
    if context.service_instance != shared.service_instance
        || (!allow_expired_or_revoked && context.expires_at <= now_ms())
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
        return Err("control-revoked");
    }
    Ok(())
}

fn append_output(shared: &Arc<Shared>, job_id: &str, bytes: &[u8]) {
    let mut inner = shared.inner.lock().recover();
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
        let mut inner = shared.inner.lock().recover();
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

fn make_script_pipe() -> std::io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    // SAFETY: fds points at two valid integers and successful pipe ownership
    // is immediately transferred into OwnedFd.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    for fd in fds {
        // SAFETY: both descriptors were returned by pipe and remain owned
        // here. CLOEXEC prevents the child from retaining the write end;
        // dup2 in pre_exec intentionally clears it for descriptor 3.
        if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } != 0 {
            let error = std::io::Error::last_os_error();
            // SAFETY: construction of OwnedFd has not happened yet.
            unsafe {
                libc::close(fds[0]);
                libc::close(fds[1]);
            }
            return Err(error);
        }
    }
    // The parent must never block the control plane indefinitely while a
    // child refuses to consume its script pipe.
    let flags = unsafe { libc::fcntl(fds[1], libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fds[1], libc::F_SETFL, flags | libc::O_NONBLOCK) } != 0 {
        let error = std::io::Error::last_os_error();
        unsafe {
            libc::close(fds[0]);
            libc::close(fds[1]);
        }
        return Err(error);
    }
    // SAFETY: pipe returned two new owned descriptors.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
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

fn bounded_write(
    writer: &mut (impl Write + AsRawFd),
    bytes: &[u8],
    timeout: Duration,
) -> std::io::Result<()> {
    let deadline = Instant::now() + timeout;
    let mut offset = 0;
    while offset < bytes.len() {
        match writer.write(&bytes[offset..]) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "short write",
                ))
            }
            Ok(count) => offset += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                wait_writable(writer.as_raw_fd(), deadline)?;
            }
            Err(error) => return Err(error),
        }
    }
    writer.flush()
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
            let inner = shared.inner.lock().recover();
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

fn spawn_job(
    shared: &Arc<Shared>,
    job_id: &str,
    script: &str,
    cwd: &str,
    timeout_ms: Option<u64>,
) -> Result<(), &'static str> {
    if script.len() > MAX_SCRIPT || script.as_bytes().contains(&0) {
        return Err("invalid-script");
    }
    if !Path::new(cwd).is_absolute() || !Path::new(cwd).is_dir() {
        return Err("invalid-cwd");
    }
    let (script_read, script_write) = make_script_pipe().map_err(|_| "spawn-failed")?;
    let mut script_write = std::fs::File::from(script_write);
    let read_fd = script_read.as_raw_fd();
    let mut command = Command::new("/bin/zsh");
    command
        .args(["-d", "-f", "-c", "source /dev/fd/3"])
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
    // SAFETY: only async-signal-safe dup2/setpgid/close calls run after fork.
    unsafe {
        command.pre_exec(move || {
            if libc::dup2(read_fd, 3) < 0 || libc::setpgid(0, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if read_fd == 3 {
                if libc::fcntl(3, libc::F_SETFD, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
            } else {
                libc::close(read_fd);
            }
            Ok(())
        });
    }
    let mut child = command.spawn().map_err(|_| "spawn-failed")?;
    drop(script_read);
    let stdout = child.stdout.take().ok_or("spawn-failed")?;
    let stderr = child.stderr.take().ok_or("spawn-failed")?;
    let stdin = child.stdin.take().ok_or("spawn-failed")?;
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
        return Err("spawn-failed");
    }
    let child = Arc::new(Mutex::new(child));
    {
        let mut inner = shared.inner.lock().recover();
        let Some(job) = inner.jobs.get_mut(job_id) else {
            return Err("job-not-found");
        };
        job.state = JobState::Running;
        job.child = Some(child.clone());
        job.stdin = Some(stdin);
        shared.changed.notify_all();
    }
    mirror(shared.clone(), job_id.into(), stdout, false);
    mirror(shared.clone(), job_id.into(), stderr, true);
    // Start draining child output before writing a large script. A child may
    // emit startup output before consuming descriptor 3.
    let dispatch_unknown =
        bounded_write(&mut script_write, script.as_bytes(), SCRIPT_WRITE_TIMEOUT).is_err();
    drop(script_write);

    let waiter = shared.clone();
    let waiter_id = job_id.to_string();
    std::thread::spawn(move || loop {
        let status = child.lock().recover().try_wait();
        match status {
            Ok(Some(status)) => {
                use std::os::unix::process::ExitStatusExt;
                let mut inner = waiter.inner.lock().recover();
                if let Some(job) = inner.jobs.get_mut(&waiter_id) {
                    job.state = JobState::Exited;
                    job.exit_code = status.code();
                    job.signal = status.signal();
                    job.ended_at = Some(now_ms());
                    job.stdin = None;
                    job.child = None;
                }
                if inner.active.as_deref() == Some(&waiter_id) {
                    inner.active = None;
                }
                waiter.changed.notify_all();
                break;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(_) => {
                let mut inner = waiter.inner.lock().recover();
                if let Some(job) = inner.jobs.get_mut(&waiter_id) {
                    job.state = JobState::Lost;
                    job.ended_at = Some(now_ms());
                    job.stdin = None;
                    job.child = None;
                }
                if inner.active.as_deref() == Some(&waiter_id) {
                    inner.active = None;
                }
                waiter.changed.notify_all();
                break;
            }
        }
    });

    if let Some(timeout) = timeout_ms.filter(|value| *value > 0) {
        let timed = shared.clone();
        let timed_id = job_id.to_string();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(timeout));
            let child = {
                let mut inner = timed.inner.lock().recover();
                let Some(job) = inner.jobs.get_mut(&timed_id) else {
                    return;
                };
                if job.state != JobState::Running {
                    return;
                }
                job.timeout_requested = true;
                job.interrupt_requested = true;
                job.child.clone()
            };
            if let Some(child) = child {
                let child = child.lock().recover();
                if child.id() > 0 {
                    // SAFETY: this child was placed in a fresh process group
                    // whose id is its pid. No PID is used after Child reports exit.
                    unsafe { libc::kill(-(child.id() as i32), libc::SIGINT) };
                }
            }
        });
    }
    if dispatch_unknown {
        Err("dispatch-unknown")
    } else {
        Ok(())
    }
}

fn wait_for_job(shared: &Arc<Shared>, job_id: &str, wait_ms: u64) {
    let limit = wait_ms.min(5_000);
    if limit == 0 {
        return;
    }
    let inner = shared.inner.lock().recover();
    let _ = shared
        .changed
        .wait_timeout_while(inner, Duration::from_millis(limit), |state| {
            state
                .jobs
                .get(job_id)
                .is_some_and(|job| matches!(job.state, JobState::Starting | JobState::Running))
        });
}

fn handle(shared: &Arc<Shared>, request: Request) -> Response {
    match request {
        Request::Ping => {
            let inner = shared.inner.lock().recover();
            let mut response = Response::empty(&shared.generation);
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
        Request::Exec {
            job_id,
            request_hash,
            script,
            cwd,
            wait_ms,
            timeout_ms,
            context,
        } => {
            if !valid_id(&job_id) || !valid_id(&request_hash) {
                return Response::error(&shared.generation, "invalid-request");
            }
            {
                let mut inner = shared.inner.lock().recover();
                if let Err(error) = admit_exec_context(shared, &mut inner, &context) {
                    return Response::error(&shared.generation, error);
                }
                if let Some(existing) = inner.jobs.get(&job_id) {
                    if existing.request_hash != request_hash
                        || existing.intent_hash != context.intent_hash
                        || existing.control_epoch != context.control_epoch
                        || existing.holder_id != context.holder_id
                    {
                        return Response::error(&shared.generation, "request-id-conflict");
                    }
                    drop(inner);
                    wait_for_job(shared, &job_id, wait_ms);
                    let inner = shared.inner.lock().recover();
                    let mut response = Response::empty(&shared.generation);
                    response.job = inner.jobs.get(&job_id).map(Job::view);
                    return response;
                }
                if inner.active.is_some() {
                    return Response::error(&shared.generation, "session-busy");
                }
                if inner.jobs.len() >= MAX_JOBS {
                    return Response::error(&shared.generation, "capacity-exceeded");
                }
                inner.order.push_back(job_id.clone());
                inner.jobs.insert(
                    job_id.clone(),
                    Job {
                        id: job_id.clone(),
                        request_hash,
                        holder_id: context.holder_id,
                        control_epoch: context.control_epoch,
                        intent_hash: context.intent_hash,
                        grant_id: context.grant_id,
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
                        child: None,
                        stdin: None,
                        stdout_eof: false,
                        stderr_eof: false,
                    },
                );
                // Starting is an active reservation, not an idle session.
                inner.active = Some(job_id.clone());
            }
            if let Err(error) = spawn_job(shared, &job_id, &script, &cwd, timeout_ms) {
                if error != "dispatch-unknown" {
                    let mut inner = shared.inner.lock().recover();
                    if let Some(job) = inner.jobs.get_mut(&job_id) {
                        job.state = JobState::Lost;
                        job.ended_at = Some(now_ms());
                    }
                    if inner.active.as_deref() == Some(&job_id) {
                        inner.active = None;
                    }
                }
                return Response::error(&shared.generation, error);
            }
            wait_for_job(shared, &job_id, wait_ms);
            let inner = shared.inner.lock().recover();
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
                return Response::error(&shared.generation, "invalid-request");
            }
            if wait_ms > 0 {
                let limit = wait_ms.min(5_000);
                let initial_end = {
                    let inner = shared.inner.lock().recover();
                    let Some(job) = inner.jobs.get(&job_id) else {
                        return Response::error(&shared.generation, "job-not-found");
                    };
                    job.base_cursor + job.output.len() as u64
                };
                let inner = shared.inner.lock().recover();
                let _ = shared.changed.wait_timeout_while(
                    inner,
                    Duration::from_millis(limit),
                    |state| {
                        state.jobs.get(&job_id).is_some_and(|job| {
                            matches!(job.state, JobState::Starting | JobState::Running)
                                && job.base_cursor + job.output.len() as u64 == initial_end
                        })
                    },
                );
            }
            let mut inner = shared.inner.lock().recover();
            let Some(job) = inner.jobs.get_mut(&job_id) else {
                return Response::error(&shared.generation, "job-not-found");
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
                return Response::error(&shared.generation, "output-cursor-invalid");
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
                return Response::error(&shared.generation, "invalid-request");
            }
            let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data_b64) else {
                return Response::error(&shared.generation, "invalid-request");
            };
            if bytes.len() > MAX_INPUT {
                return Response::error(&shared.generation, "invalid-request");
            }
            let mut stdin = {
                let mut inner = shared.inner.lock().recover();
                if inner.control != ControlMode::Mcp || inner.active.as_deref() != Some(&job_id) {
                    return Response::error(&shared.generation, "job-not-running");
                }
                if let Some(job) = inner.jobs.get(&job_id) {
                    if let Err(error) = check_job_context(shared, &inner, job, &context, false) {
                        return Response::error(&shared.generation, error);
                    }
                }
                let Some(job) = inner.jobs.get_mut(&job_id) else {
                    return Response::error(&shared.generation, "job-not-found");
                };
                if job.state != JobState::Running {
                    return Response::error(&shared.generation, "job-not-running");
                }
                let Some(stdin) = job.stdin.take() else {
                    return Response::error(&shared.generation, "job-not-running");
                };
                stdin
            };
            let wrote = bounded_job_input(shared, &job_id, &context, &mut stdin, &bytes).is_ok();
            let mut inner = shared.inner.lock().recover();
            let still_bound = inner.control == ControlMode::Mcp
                && inner.active.as_deref() == Some(&job_id)
                && inner.control_epoch == context.control_epoch
                && inner.holder_id.as_deref() == Some(&context.holder_id)
                && inner
                    .jobs
                    .get(&job_id)
                    .is_some_and(|job| job.state == JobState::Running);
            if !wrote || !still_bound {
                return Response::error(&shared.generation, "job-state-unknown");
            }
            if let Some(job) = inner.jobs.get_mut(&job_id) {
                job.stdin = Some(stdin);
            }
            Response::empty(&shared.generation)
        }
        Request::Interrupt { job_id, context } => {
            let child = {
                let mut inner = shared.inner.lock().recover();
                if inner.control != ControlMode::Mcp || inner.active.as_deref() != Some(&job_id) {
                    return Response::error(&shared.generation, "job-not-running");
                }
                if let Some(job) = inner.jobs.get(&job_id) {
                    if let Err(error) = check_job_context(shared, &inner, job, &context, true) {
                        return Response::error(&shared.generation, error);
                    }
                }
                let Some(job) = inner.jobs.get_mut(&job_id) else {
                    return Response::error(&shared.generation, "job-not-found");
                };
                if job.state != JobState::Running {
                    return Response::error(&shared.generation, "job-not-running");
                }
                job.interrupt_requested = true;
                job.child.clone()
            };
            let Some(child) = child else {
                return Response::error(&shared.generation, "job-state-unknown");
            };
            let child = child.lock().recover();
            if child.id() == 0 {
                return Response::error(&shared.generation, "job-state-unknown");
            }
            // SAFETY: child identity and process group remain owned by the
            // active Child handle until the waiter records its exit.
            let sent = unsafe { libc::kill(-(child.id() as i32), libc::SIGINT) } == 0;
            if sent {
                Response::empty(&shared.generation)
            } else {
                Response::error(&shared.generation, "job-state-unknown")
            }
        }
        Request::Control {
            mode,
            service_instance,
            control_epoch,
            holder_id,
        } => {
            let human_job = {
                let mut inner = shared.inner.lock().recover();
                if service_instance != shared.service_instance
                    || control_epoch < inner.control_epoch
                {
                    return Response::error(&shared.generation, "dispatch-context-invalid");
                }
                if mode == ControlMode::Mcp && inner.active.is_some() {
                    return Response::error(&shared.generation, "session-busy");
                }
                inner.control = mode;
                inner.control_epoch = control_epoch;
                inner.holder_id = holder_id;
                if mode == ControlMode::Human && inner.active.is_none() {
                    let id = format!("human_{}", now_ms());
                    inner.order.push_back(id.clone());
                    inner.jobs.insert(
                        id.clone(),
                        Job {
                            id: id.clone(),
                            request_hash: "human-control".into(),
                            holder_id: "local-human".into(),
                            control_epoch,
                            intent_hash: "human-control".into(),
                            grant_id: "local-human".into(),
                            grant_version: 1,
                            state: JobState::Starting,
                            exit_code: None,
                            signal: None,
                            started_at: now_ms(),
                            ended_at: None,
                            interrupt_requested: false,
                            timeout_requested: false,
                            output: VecDeque::new(),
                            base_cursor: 0,
                            child: None,
                            stdin: None,
                            stdout_eof: false,
                            stderr_eof: false,
                        },
                    );
                    Some(id)
                } else {
                    None
                }
            };
            if let Some(job_id) = human_job {
                let cwd = shared.initial_cwd.to_string_lossy().into_owned();
                if let Err(error) = spawn_job(shared, &job_id, "exec /bin/zsh -d -f -s", &cwd, None)
                {
                    let mut inner = shared.inner.lock().recover();
                    if let Some(job) = inner.jobs.get_mut(&job_id) {
                        job.state = JobState::Lost;
                        job.ended_at = Some(now_ms());
                    }
                    return Response::error(&shared.generation, error);
                }
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
            if service_instance != shared.service_instance
                || !(60_000..=7 * 24 * 60 * 60_000).contains(&output_retention_ms)
            {
                return Response::error(&shared.generation, "dispatch-context-invalid");
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
            if service_instance != shared.service_instance || !valid_id(&grant_id) {
                return Response::error(&shared.generation, "dispatch-context-invalid");
            }
            let mut inner = shared.inner.lock().recover();
            inner
                .revoked_grants
                .entry(grant_id)
                .and_modify(|version| *version = (*version).max(grant_version))
                .or_insert(grant_version);
            Response::empty(&shared.generation)
        }
        Request::Shutdown => {
            let mut inner = shared.inner.lock().recover();
            if inner.active.is_some() {
                return Response::error(&shared.generation, "session-busy");
            }
            inner.stopping = true;
            Response::empty(&shared.generation)
        }
    }
}

fn serve_connection(shared: Arc<Shared>, mut stream: UnixStream) {
    let limited = stream
        .try_clone()
        .map(|copy| copy.take((MAX_REQUEST + 1) as u64));
    let response = match limited {
        Ok(reader) => {
            let mut reader = BufReader::new(reader);
            let mut line = Vec::new();
            match reader.read_until(b'\n', &mut line) {
                Ok(size) if size > 0 && size <= MAX_REQUEST && line.ends_with(b"\n") => {
                    match serde_json::from_slice::<Request>(&line) {
                        Ok(request) => handle(&shared, request),
                        Err(_) => Response::error(&shared.generation, "invalid-request"),
                    }
                }
                _ => Response::error(&shared.generation, "invalid-request"),
            }
        }
        Err(_) => Response::error(&shared.generation, "internal-error"),
    };
    if let Ok(mut bytes) = serde_json::to_vec(&response) {
        if bytes.len() + 1 > MAX_RESPONSE {
            bytes = serde_json::to_vec(&Response::error(&shared.generation, "response-too-large"))
                .unwrap_or_default();
        }
        let _ = stream.write_all(&bytes);
        let _ = stream.write_all(b"\n");
        let _ = stream.flush();
    }
}

fn stdin_forwarder(shared: Arc<Shared>) {
    std::thread::spawn(move || {
        let mut input = std::io::stdin();
        let mut buffer = [0u8; 4096];
        while let Ok(count) = input.read(&mut buffer) {
            if count == 0 {
                break;
            }
            let mut inner = shared.inner.lock().recover();
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

fn parse_args() -> Option<(PathBuf, String, String, u64)> {
    let mut args = std::env::args_os().skip(1);
    let mut socket = None;
    let mut generation = None;
    let mut service_instance = None;
    let mut output_retention_ms = None;
    while let Some(arg) = args.next() {
        match arg.to_str()? {
            "--socket" => socket = args.next().map(PathBuf::from),
            "--generation" => generation = args.next()?.into_string().ok(),
            "--service-instance" => service_instance = args.next()?.into_string().ok(),
            "--output-retention-ms" => {
                output_retention_ms = args.next()?.to_str()?.parse::<u64>().ok()
            }
            _ => return None,
        }
    }
    let socket = socket?;
    let generation = generation?;
    let service_instance = service_instance?;
    let output_retention_ms = output_retention_ms?;
    if !socket.is_absolute() || !valid_id(&generation) || !valid_id(&service_instance) {
        return None;
    }
    if !(60_000..=7 * 24 * 60 * 60_000).contains(&output_retention_ms) {
        return None;
    }
    Some((socket, generation, service_instance, output_retention_ms))
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
            initial_cwd: PathBuf::from("/tmp"),
            inner: Mutex::new(Inner {
                jobs: HashMap::new(),
                order: VecDeque::new(),
                active: None,
                control: ControlMode::Fenced,
                control_epoch: 0,
                holder_id: None,
                revoked_grants: HashMap::new(),
                stopping: false,
                retained_output: 0,
            }),
            changed: Condvar::new(),
        })
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
            expires_at: now_ms() + 60_000,
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
                    control_epoch: 3,
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
                script: "printf stale".into(),
                cwd: "/tmp".into(),
                wait_ms: 0,
                timeout_ms: None,
                context: context("svc_current", 2, "holder_old"),
            },
        );
        assert_eq!(stale.error, Some("control-revoked"));
        let old_service = handle(
            &shared,
            Request::Exec {
                job_id: "job_old_service".into(),
                request_hash: "c".repeat(64),
                script: "printf old".into(),
                cwd: "/tmp".into(),
                wait_ms: 0,
                timeout_ms: None,
                context: context("svc_previous", 3, "holder_new"),
            },
        );
        assert_eq!(old_service.error, Some("dispatch-context-invalid"));
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
                script: "printf revoked".into(),
                cwd: "/tmp".into(),
                wait_ms: 0,
                timeout_ms: None,
                context: context("svc_current", 3, "holder_new"),
            },
        );
        assert_eq!(revoked.error, Some("dispatch-context-invalid"));
        assert!(shared.inner.lock().recover().jobs.is_empty());
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
            script: ":".into(),
            cwd: "/tmp".into(),
            wait_ms: 1_000,
            timeout_ms: None,
            context: context.clone(),
        };
        let first = handle(&shared, request());
        assert!(first.ok, "{:?}", first.error);
        let second = handle(&shared, request());
        assert!(second.ok, "{:?}", second.error);
        assert_eq!(shared.inner.lock().recover().jobs.len(), 1);
    }

    #[test]
    fn script_pipe_write_has_a_deadline_when_reader_stalls() {
        let (_reader, writer) = make_script_pipe().unwrap();
        let mut writer = std::fs::File::from(writer);
        let error = bounded_write(
            &mut writer,
            &vec![b'x'; 1024 * 1024],
            Duration::from_millis(10),
        )
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    }

    #[test]
    fn expired_output_reports_a_gap_without_deleting_job_metadata() {
        let shared = shared();
        shared.output_retention_ms.store(1, Ordering::SeqCst);
        {
            let mut inner = shared.inner.lock().recover();
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
                    child: None,
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
}

fn main() {
    let Some((socket, generation, service_instance, output_retention_ms)) = parse_args() else {
        std::process::exit(64);
    };
    let Some(parent) = socket.parent() else {
        std::process::exit(64);
    };
    if std::fs::create_dir_all(parent).is_err()
        || std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)).is_err()
    {
        std::process::exit(73);
    }
    let _ = std::fs::remove_file(&socket);
    let listener = match UnixListener::bind(&socket) {
        Ok(listener) => listener,
        Err(_) => std::process::exit(73),
    };
    if std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).is_err() {
        let _ = std::fs::remove_file(&socket);
        std::process::exit(73);
    }
    if listener.set_nonblocking(true).is_err() {
        let _ = std::fs::remove_file(&socket);
        std::process::exit(73);
    }
    let shared = Arc::new(Shared {
        generation,
        service_instance,
        output_retention_ms: AtomicU64::new(output_retention_ms),
        initial_cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
        inner: Mutex::new(Inner {
            jobs: HashMap::new(),
            order: VecDeque::new(),
            active: None,
            control: ControlMode::Fenced,
            control_epoch: 0,
            holder_id: None,
            revoked_grants: HashMap::new(),
            stopping: false,
            retained_output: 0,
        }),
        changed: Condvar::new(),
    });
    stdin_forwarder(shared.clone());
    println!("Deck MCP managed shell ready");
    while !shared.inner.lock().recover().stopping {
        match listener.accept() {
            Ok((stream, _)) => {
                let state = shared.clone();
                std::thread::spawn(move || serve_connection(state, stream));
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(25)),
        }
    }
    let _ = std::fs::remove_file(socket);
}
