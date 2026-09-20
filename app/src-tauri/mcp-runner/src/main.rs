//! Visible execution host for one Deck MCP tmux pane.
//!
//! The runner is a signed bundle sidecar and is started only as the pane
//! process of an explicitly created MCP session. It accepts user scripts over
//! a user-only Unix socket, executes each job in a fresh zsh process, mirrors
//! combined output into the real pane, and retains a bounded copy for cursor
//! reads. Script bytes never appear in argv, the environment, or a temporary
//! file. The runner has no network, model, persistence, or Board authority.

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
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const PROTOCOL: u32 = 1;
const MAX_REQUEST: usize = 256 * 1024;
const MAX_SCRIPT: usize = 128 * 1024;
const MAX_INPUT: usize = 32 * 1024;
const RETAINED_OUTPUT: usize = 1024 * 1024;
const RETAINED_OUTPUT_PER_SESSION: usize = 16 * 1024 * 1024;
const MAX_JOBS: usize = 256;

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
    },
    Interrupt {
        job_id: String,
    },
    Control {
        mode: ControlMode,
    },
    Shutdown,
}

fn default_read() -> usize {
    32 * 1024
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
}

struct Job {
    id: String,
    request_hash: String,
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
        }
    }
}

struct Inner {
    jobs: HashMap<String, Job>,
    order: VecDeque<String>,
    active: Option<String>,
    control: ControlMode,
    stopping: bool,
    retained_output: usize,
}

struct Shared {
    generation: String,
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
    // SAFETY: pipe returned two new owned descriptors.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
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
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // SAFETY: only async-signal-safe dup2/setpgid/close calls run after fork.
    unsafe {
        command.pre_exec(move || {
            if libc::dup2(read_fd, 3) < 0 || libc::setpgid(0, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if read_fd != 3 {
                libc::close(read_fd);
            }
            Ok(())
        });
    }
    let mut child = command.spawn().map_err(|_| "spawn-failed")?;
    drop(script_read);
    script_write
        .write_all(script.as_bytes())
        .and_then(|_| script_write.flush())
        .map_err(|_| "dispatch-unknown")?;
    drop(script_write);
    let stdout = child.stdout.take().ok_or("spawn-failed")?;
    let stderr = child.stderr.take().ok_or("spawn-failed")?;
    let stdin = child.stdin.take().ok_or("spawn-failed")?;
    let child = Arc::new(Mutex::new(child));
    {
        let mut inner = shared.inner.lock().recover();
        let Some(job) = inner.jobs.get_mut(job_id) else {
            return Err("job-not-found");
        };
        job.state = JobState::Running;
        job.child = Some(child.clone());
        job.stdin = Some(stdin);
        inner.active = Some(job_id.into());
        shared.changed.notify_all();
    }
    mirror(shared.clone(), job_id.into(), stdout, false);
    mirror(shared.clone(), job_id.into(), stderr, true);

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
    Ok(())
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
        } => {
            if !valid_id(&job_id) || !valid_id(&request_hash) {
                return Response::error(&shared.generation, "invalid-request");
            }
            {
                let mut inner = shared.inner.lock().recover();
                if inner.control != ControlMode::Mcp {
                    return Response::error(&shared.generation, "control-revoked");
                }
                if let Some(existing) = inner.jobs.get(&job_id) {
                    if existing.request_hash != request_hash {
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
                    },
                );
            }
            if let Err(error) = spawn_job(shared, &job_id, &script, &cwd, timeout_ms) {
                let mut inner = shared.inner.lock().recover();
                if let Some(job) = inner.jobs.get_mut(&job_id) {
                    job.state = JobState::Lost;
                    job.ended_at = Some(now_ms());
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
            if !valid_id(&job_id) || !(1..=64 * 1024).contains(&max_bytes) {
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
            let inner = shared.inner.lock().recover();
            let Some(job) = inner.jobs.get(&job_id) else {
                return Response::error(&shared.generation, "job-not-found");
            };
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
            response
        }
        Request::Input { job_id, data_b64 } => {
            if !valid_id(&job_id) || data_b64.len() > MAX_INPUT.saturating_mul(2) {
                return Response::error(&shared.generation, "invalid-request");
            }
            let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data_b64) else {
                return Response::error(&shared.generation, "invalid-request");
            };
            if bytes.len() > MAX_INPUT {
                return Response::error(&shared.generation, "invalid-request");
            }
            let mut inner = shared.inner.lock().recover();
            if inner.control != ControlMode::Mcp || inner.active.as_deref() != Some(&job_id) {
                return Response::error(&shared.generation, "job-not-running");
            }
            let Some(job) = inner.jobs.get_mut(&job_id) else {
                return Response::error(&shared.generation, "job-not-found");
            };
            if job.state != JobState::Running {
                return Response::error(&shared.generation, "job-not-running");
            }
            let Some(stdin) = job.stdin.as_mut() else {
                return Response::error(&shared.generation, "job-not-running");
            };
            if stdin.write_all(&bytes).and_then(|_| stdin.flush()).is_err() {
                return Response::error(&shared.generation, "job-state-unknown");
            }
            Response::empty(&shared.generation)
        }
        Request::Interrupt { job_id } => {
            let child = {
                let mut inner = shared.inner.lock().recover();
                if inner.control != ControlMode::Mcp || inner.active.as_deref() != Some(&job_id) {
                    return Response::error(&shared.generation, "job-not-running");
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
        Request::Control { mode } => {
            let human_job = {
                let mut inner = shared.inner.lock().recover();
                if mode == ControlMode::Mcp && inner.active.is_some() {
                    return Response::error(&shared.generation, "session-busy");
                }
                inner.control = mode;
                if mode == ControlMode::Human && inner.active.is_none() {
                    let id = format!("human_{}", now_ms());
                    inner.order.push_back(id.clone());
                    inner.jobs.insert(
                        id.clone(),
                        Job {
                            id: id.clone(),
                            request_hash: "human-control".into(),
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
            } else {
                "human"
            });
            response
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
    if let Ok(bytes) = serde_json::to_vec(&response) {
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

fn parse_args() -> Option<(PathBuf, String)> {
    let mut args = std::env::args_os().skip(1);
    let mut socket = None;
    let mut generation = None;
    while let Some(arg) = args.next() {
        match arg.to_str()? {
            "--socket" => socket = args.next().map(PathBuf::from),
            "--generation" => generation = args.next()?.into_string().ok(),
            _ => return None,
        }
    }
    let socket = socket?;
    let generation = generation?;
    if !socket.is_absolute() || !valid_id(&generation) {
        return None;
    }
    Some((socket, generation))
}

fn main() {
    let Some((socket, generation)) = parse_args() else {
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
        initial_cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
        inner: Mutex::new(Inner {
            jobs: HashMap::new(),
            order: VecDeque::new(),
            active: None,
            control: ControlMode::Mcp,
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
