//! Local MCP control service and durable authorization/operation ledger.
//!
//! This is deliberately separate from Phone Connector: its socket, client
//! records, scopes, operation ids, sessions, and jobs are disjoint. The
//! listener is disabled by default and is a 0600 Unix socket under Deck's
//! private data directory; every connection must also have Deck's effective
//! uid. The thin `deck-mcp` sidecar provides MCP STDIO and never receives a
//! Phone token or unrestricted backend credential.
//!
//! Board creation and close intents are journaled here, then handed to the
//! webview's one serialized Board transaction (`mcp.js`). The managed runner
//! reports job exit independently from that control-operation state. Scripts
//! are never persisted or logged: only their SHA-256 request hash and bounded
//! runner output exist. On restart, an accepted operation is not replayed;
//! it becomes ambiguous unless the deterministic result can be observed.

use base64::Engine;
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

use crate::error::{DeckError, ErrorKind};
use crate::sync::LockRecover;

const VERSION: u32 = 1;
const MAX_STATE_BYTES: usize = 8 * 1024 * 1024;
const MAX_REQUEST_BYTES: usize = 256 * 1024;
const MAX_RESPONSE_BYTES: usize = 128 * 1024;
const MAX_CLIENTS: usize = 32;
const MAX_PROJECTS_PER_CLIENT: usize = 64;
const MAX_ROOTS_PER_PROJECT: usize = 16;
const MAX_SESSIONS: usize = 64;
const MAX_OPERATIONS: usize = 2000;
const MAX_JOBS: usize = 1000;
const MAX_SCRIPT_BYTES: usize = 128 * 1024;
const MAX_INPUT_BYTES: usize = 32 * 1024;
const DEFAULT_LEASE_MS: u64 = 60_000;
const MAX_LEASE_MS: u64 = 5 * 60_000;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis() as u64)
        .unwrap_or(0)
}

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
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

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Config {
    enabled: bool,
    clients: Vec<Client>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Client {
    id: String,
    name: String,
    revoked_at: Option<u64>,
    allow_create: bool,
    projects: Vec<ProjectScope>,
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
    control_epoch: u64,
    lease_expires_at: Option<u64>,
    human_lock: bool,
    #[serde(default)]
    closing: bool,
    created_at: u64,
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
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DiskDoc {
    version: u32,
    config: Config,
    sessions: Vec<ManagedSession>,
    operations: Vec<Operation>,
    jobs: Vec<JobBinding>,
}

impl Default for DiskDoc {
    fn default() -> Self {
        Self {
            version: VERSION,
            config: Config::default(),
            sessions: Vec::new(),
            operations: Vec::new(),
            jobs: Vec::new(),
        }
    }
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
}

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
    validate_doc(&doc)?;
    let mut changed = false;
    for operation in &mut doc.operations {
        if matches!(operation.state.as_str(), "accepted" | "executing")
            && matches!(operation.kind.as_str(), "session-create" | "session-close")
        {
            // The webview can reconcile deterministic card ids through the
            // one Board transaction after restart; it will not repeat an
            // already-visible card or close.
            operation.state = "accepted".into();
            operation.updated_at = now_ms();
            changed = true;
        } else if matches!(operation.state.as_str(), "accepted" | "executing") {
            operation.state = "ambiguous".into();
            operation.code = Some("deck-restarted".into());
            operation.updated_at = now_ms();
            changed = true;
        }
    }
    for session in &mut doc.sessions {
        if session.control_owner.is_some() {
            session.control_owner = None;
            session.control_epoch = session.control_epoch.saturating_add(1);
            session.lease_expires_at = None;
            changed = true;
        }
    }
    if changed {
        save(path, &doc)?;
    }
    Ok(doc)
}

fn validate_doc(doc: &DiskDoc) -> Result<(), DeckError> {
    let mut client_ids = HashSet::new();
    let clients_valid = doc.version == VERSION
        && doc.config.clients.len() <= MAX_CLIENTS
        && doc.config.clients.iter().all(|client| {
            valid_id(&client.id)
                && client_ids.insert(client.id.clone())
                && valid_title(&client.name)
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
                && Path::new(&session.cwd).is_absolute()
                && Path::new(&session.runner_socket).is_absolute()
        });
    let mut operation_ids = HashSet::new();
    let operations_valid = doc.operations.len() <= MAX_OPERATIONS
        && doc.operations.iter().all(|operation| {
            valid_id(&operation.operation_id)
                && operation_ids.insert(operation.operation_id.clone())
                && valid_id(&operation.client_id)
                && valid_id(&operation.request_id)
                && operation.request_hash.len() == 64
                && matches!(
                    operation.state.as_str(),
                    "accepted" | "executing" | "committed" | "rejected" | "ambiguous"
                )
        });
    let mut job_ids = HashSet::new();
    let jobs_valid = doc.jobs.len() <= MAX_JOBS
        && doc.jobs.iter().all(|job| {
            valid_id(&job.job_id)
                && job_ids.insert(job.job_id.clone())
                && valid_id(&job.client_id)
                && session_ids.contains(&job.session_id)
                && operation_ids.contains(&job.operation_id)
        });
    if clients_valid && sessions_valid && operations_valid && jobs_valid {
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
) -> Result<(), DeckError> {
    if session.generation != generation {
        return Err(DeckError::new(
            ErrorKind::ContextChanged,
            "session generation changed",
        ));
    }
    if session.human_lock
        || session.control_owner.as_deref() != Some(client_id)
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

fn error_value(code: &str, message: &str, next_action: &str) -> Value {
    json!({"ok":false,"error":{"code":code,"message":message,"nextAction":next_action}})
}

fn map_error(error: DeckError) -> Value {
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
            "Resolve or clear old MCP operations in Deck before retrying.",
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

fn request_hash(tool: &str, arguments: &Value) -> String {
    let mut bytes = tool.as_bytes().to_vec();
    bytes.push(0);
    bytes.extend(serde_json::to_vec(arguments).unwrap_or_default());
    sha(&bytes)
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

fn operation_view(operation: &Operation) -> Value {
    json!({
        "ok": true,
        "operationId": operation.operation_id,
        "kind": operation.kind,
        "state": operation.state,
        "code": operation.code,
        "result": operation.result,
        "acceptedAt": operation.accepted_at,
        "updatedAt": operation.updated_at,
        "shellJobComplete": false
    })
}

fn send_runner(session: &ManagedSession, request: &Value) -> Result<Value, DeckError> {
    let mut stream = UnixStream::connect(&session.runner_socket)
        .map_err(|_| DeckError::new(ErrorKind::Missing, "managed runner is unavailable"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(7)))
        .map_err(DeckError::from)?;
    stream
        .set_write_timeout(Some(Duration::from_secs(7)))
        .map_err(DeckError::from)?;
    serde_json::to_writer(&mut stream, request)
        .map_err(|_| DeckError::new(ErrorKind::Other, "runner request encoding failed"))?;
    stream.write_all(b"\n").map_err(DeckError::from)?;
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
    if value.get("generation").and_then(Value::as_str) != Some(&session.generation) {
        return Err(DeckError::new(
            ErrorKind::ContextChanged,
            "managed runner generation changed",
        ));
    }
    Ok(value)
}

fn runner_socket_matches(socket: &str, generation: &str) -> bool {
    let Ok(mut stream) = UnixStream::connect(socket) else {
        return false;
    };
    if stream.write_all(b"{\"kind\":\"ping\"}\n").is_err() {
        return false;
    }
    let mut bytes = Vec::new();
    BufReader::new(stream)
        .take((MAX_RESPONSE_BYTES + 1) as u64)
        .read_until(b'\n', &mut bytes)
        .is_ok()
        && bytes.len() <= MAX_RESPONSE_BYTES
        && serde_json::from_slice::<Value>(&bytes)
            .ok()
            .is_some_and(|value| {
                value.get("ok").and_then(Value::as_bool) == Some(true)
                    && value.get("generation").and_then(Value::as_str) == Some(generation)
            })
}

fn runner_error(value: &Value) -> Option<(&'static str, &'static str)> {
    match value.get("error").and_then(Value::as_str)? {
        "session-busy" => Some((
            "SESSION_BUSY",
            "Read or interrupt the active job before retrying.",
        )),
        "control-revoked" => Some((
            "CONTROL_REVOKED",
            "Inspect control state and wait for the local user to return control.",
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
        "invalid-cwd" | "invalid-script" | "invalid-request" => Some((
            "INVALID_ARGUMENTS",
            "Correct the rejected arguments before retrying.",
        )),
        _ => None,
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WireRequest {
    version: u32,
    client_id: String,
    tool: String,
    arguments: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateArgs {
    request_id: String,
    project_id: String,
    cwd: String,
    #[serde(default)]
    title: Option<String>,
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
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ControlAction {
    Request,
    Renew,
    Release,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlArgs {
    request_id: String,
    session_id: String,
    expected_generation: String,
    action: ControlAction,
    #[serde(default)]
    control_epoch: Option<u64>,
    #[serde(default)]
    lease_ms: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecArgs {
    request_id: String,
    session_id: String,
    expected_generation: String,
    control_epoch: u64,
    script: String,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    wait_ms: Option<u64>,
    #[serde(default)]
    execution_timeout_ms: Option<u64>,
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InputArgs {
    request_id: String,
    job_id: String,
    session_generation: String,
    control_epoch: u64,
    input: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InterruptArgs {
    request_id: String,
    job_id: String,
    session_generation: String,
    control_epoch: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CloseArgs {
    request_id: String,
    session_id: String,
    expected_generation: String,
    control_epoch: u64,
    #[serde(default)]
    confirm_running: bool,
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

fn capabilities(runtime: &Runtime, client_id: &str, arguments: Value) -> Result<Value, Value> {
    parse::<Empty>(arguments)?;
    // Keep the control process free of an executable shell path. The runner
    // alone owns the reviewed zsh spawn boundary.
    let shell = format!("{} -d -f", Path::new("/bin").join("zsh").display());
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
                "adapterVersion": "0.1.0",
                "deckConnection": "connected",
                "protocolVersion": 1,
                "executionMode": "trusted-host",
                "realOsSandbox": false,
                "shellSemantics": {
                    "shell": shell,
                    "perJobShellState": true,
                    "filesystemChangesPersist": true,
                    "cdExportAliasFunctionPersistAcrossExec": false,
                    "outputKind": "pty_combined"
                },
                "limits": {
                    "scriptBytes": MAX_SCRIPT_BYTES,
                    "inputBytes": MAX_INPUT_BYTES,
                    "readBytesDefault": 32 * 1024,
                    "readBytesMax": 64 * 1024,
                    "waitMsMax": 5000,
                    "retainedOutputBytesPerJob": 1024 * 1024,
                    "retainedOutputBytesPerSession": 16 * 1024 * 1024,
                    "retainedOutputBytesGlobal": MAX_SESSIONS * 16 * 1024 * 1024,
                    "managedSessions": MAX_SESSIONS,
                    "operations": MAX_OPERATIONS
                },
                "interactiveInput": true,
                "reliableJobExitTracking": true,
                "authorizedWorkspaces": client.projects,
                "mayCreateSession": client.allow_create,
                "featureEnabled": doc.config.enabled,
                "manualActionRequired": if doc.config.enabled { Value::Null } else { json!("Open Deck Settings and enable MCP terminal control.") },
                "tools": ["deck_capabilities","deck_sessions_list","deck_session_create","deck_operation_get","deck_session_inspect","deck_session_control","deck_exec","deck_job_read","deck_job_input","deck_job_interrupt","deck_session_close"]
            }))
        })
        .map_err(map_error)?
        .map_err(map_error)
}

fn sessions_list(runtime: &Runtime, client_id: &str, arguments: Value) -> Result<Value, Value> {
    parse::<Empty>(arguments)?;
    let sessions = runtime
        .read(|doc| {
            client(doc, client_id)?;
            Ok(doc
                .sessions
                .iter()
                .filter(|session| session.owner_client_id == client_id)
                .cloned()
                .collect::<Vec<_>>())
        })
        .map_err(map_error)?
        .map_err(map_error)?;
    let values = sessions
        .into_iter()
        .map(|session| {
            let runner = send_runner(&session, &json!({"kind":"ping"})).ok();
            json!({
                "sessionId": session.session_id,
                "cardId": session.card_id,
                "projectId": session.project_id,
                "title": session.title,
                "sessionGeneration": session.generation,
                "controlOwner": session.control_owner,
                "controlEpoch": session.control_epoch,
                "activeJob": runner.as_ref().and_then(|value| value.get("job")).cloned(),
                "foreground": runner.as_ref().and_then(|value| value.get("job")).is_some().then_some("managed-job"),
                "readiness": "unknown",
                "readinessConfidence": "not-inferred-from-quiet",
                "stale": runner.is_none()
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({"ok":true,"sessions":values}))
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
    let hash = request_hash("deck_session_create", &arguments);
    let existing = runtime
        .read(|doc| {
            client(doc, client_id)?;
            Ok(existing_operation(doc, client_id, &args.request_id, &hash)?.cloned())
        })
        .map_err(map_error)?
        .map_err(map_error)?;
    if let Some(operation) = existing {
        return Ok(operation_view(&operation));
    }
    let title = args.title.unwrap_or_else(|| "MCP shell".into());
    let operation = runtime
        .write(|doc| {
            if doc.operations.len() >= MAX_OPERATIONS || doc.sessions.len() >= MAX_SESSIONS {
                return Err(DeckError::new(
                    ErrorKind::DiskFull,
                    "MCP operation capacity reached",
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
                "sessionId": session_id,
                "projectId": args.project_id,
                "title": title,
                "cwd": cwd,
                "generation": generation,
                "runnerSocket": socket
            });
            let operation = Operation {
                operation_id,
                client_id: client_id.into(),
                request_id: args.request_id,
                request_hash: hash,
                kind: "session-create".into(),
                state: "accepted".into(),
                code: None,
                result: Some(result),
                accepted_at: now_ms(),
                updated_at: now_ms(),
            };
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
    let runner = send_runner(&session, &json!({"kind":"ping"}));
    let terminal = crate::tmux::tmux(&[
        "capture-pane",
        "-p",
        "-S",
        "-40",
        "-t",
        &crate::tmux::pane_target(&session.tmux_session),
    ])
    .ok()
    .map(|value| {
        if value.len() > 16 * 1024 {
            String::from_utf8_lossy(&value.as_bytes()[value.len() - 16 * 1024..]).into_owned()
        } else {
            value
        }
    });
    Ok(json!({
        "ok": true,
        "sessionId": session.session_id,
        "sessionGeneration": session.generation,
        "controlOwner": session.control_owner,
        "controlEpoch": session.control_epoch,
        "leaseExpiresAt": session.lease_expires_at,
        "humanLock": session.human_lock,
        "activeJob": runner.as_ref().ok().and_then(|value| value.get("job")).cloned(),
        "foreground": runner.as_ref().ok().and_then(|value| value.get("job")).is_some().then_some("managed-job"),
        "readiness": "unknown",
        "terminalContext": terminal,
        "terminalContextBounded": true,
        "stale": runner.is_err(),
        "mayStartNextJob": runner.as_ref().ok().is_some_and(|value| value.get("job").is_none()) && session.control_owner.as_deref() == Some(client_id) && !session.human_lock
    }))
}

fn session_control(runtime: &Runtime, client_id: &str, arguments: Value) -> Result<Value, Value> {
    let _delivery = runtime.delivery.lock_or_recover();
    let args: ControlArgs = parse(arguments.clone())?;
    if !valid_id(&args.request_id) {
        return Err(error_value(
            "INVALID_ARGUMENTS",
            "request id is invalid",
            "Use a stable opaque request id.",
        ));
    }
    let hash = request_hash("deck_session_control", &arguments);
    let operation = runtime
        .write(|doc| {
            client(doc, client_id)?;
            if let Some(existing) = existing_operation(doc, client_id, &args.request_id, &hash)? {
                return Ok(existing.clone());
            }
            if doc.operations.len() >= MAX_OPERATIONS {
                return Err(DeckError::new(ErrorKind::DiskFull, "MCP operation capacity reached"));
            }
            let session = doc
                .sessions
                .iter_mut()
                .find(|session| session.session_id == args.session_id && session.owner_client_id == client_id)
                .ok_or_else(|| DeckError::new(ErrorKind::Missing, "session not found"))?;
            if session.generation != args.expected_generation {
                return Err(DeckError::new(ErrorKind::ContextChanged, "session generation changed"));
            }
            match args.action {
                ControlAction::Request => {
                    if session.human_lock || session.control_owner.as_deref().is_some_and(|owner| owner != client_id) {
                        return Err(DeckError::new(ErrorKind::Perm, "user owns terminal control"));
                    }
                    session.control_epoch = session.control_epoch.saturating_add(1);
                    session.control_owner = Some(client_id.into());
                    session.lease_expires_at = Some(now_ms() + args.lease_ms.unwrap_or(DEFAULT_LEASE_MS).clamp(1_000, MAX_LEASE_MS));
                }
                ControlAction::Renew => {
                    check_control(session, client_id, &args.expected_generation, args.control_epoch.unwrap_or(0))?;
                    session.lease_expires_at = Some(now_ms() + args.lease_ms.unwrap_or(DEFAULT_LEASE_MS).clamp(1_000, MAX_LEASE_MS));
                }
                ControlAction::Release => {
                    check_control(session, client_id, &args.expected_generation, args.control_epoch.unwrap_or(0))?;
                    session.control_owner = None;
                    session.lease_expires_at = None;
                    session.control_epoch = session.control_epoch.saturating_add(1);
                }
            }
            let result = json!({"sessionId":session.session_id,"sessionGeneration":session.generation,"controlOwner":session.control_owner,"controlEpoch":session.control_epoch,"leaseExpiresAt":session.lease_expires_at});
            let operation = Operation { operation_id: random_id("op_")?, client_id: client_id.into(), request_id: args.request_id, request_hash: hash, kind: "session-control".into(), state: "committed".into(), code: None, result: Some(result), accepted_at: now_ms(), updated_at: now_ms() };
            doc.operations.push(operation.clone());
            Ok(operation)
        })
        .map_err(map_error)?;
    Ok(operation_view(&operation))
}

fn exec(runtime: &Runtime, client_id: &str, arguments: Value) -> Result<Value, Value> {
    let _delivery = runtime.delivery.lock_or_recover();
    let args: ExecArgs = parse(arguments.clone())?;
    if !valid_id(&args.request_id)
        || args.script.is_empty()
        || args.script.len() > MAX_SCRIPT_BYTES
        || args.wait_ms.unwrap_or(1_000) > 5_000
        || args
            .execution_timeout_ms
            .is_some_and(|value| !(100..=24 * 60 * 60 * 1000).contains(&value))
    {
        return Err(error_value(
            "INVALID_ARGUMENTS",
            "exec arguments exceed their bounds",
            "Use a non-empty script within the advertised limits.",
        ));
    }
    let hash = request_hash("deck_exec", &arguments);
    let prepared = runtime
        .write(|doc| {
            let client = client(doc, client_id)?.clone();
            if let Some(existing) = existing_operation(doc, client_id, &args.request_id, &hash)? {
                let job = existing.result.as_ref().and_then(|result| result.get("jobId")).and_then(Value::as_str).and_then(|job_id| doc.jobs.iter().find(|job| job.job_id == job_id)).cloned();
                return Ok((existing.clone(), job, None));
            }
            if doc.operations.len() >= MAX_OPERATIONS || doc.jobs.len() >= MAX_JOBS {
                return Err(DeckError::new(ErrorKind::DiskFull, "MCP operation capacity reached"));
            }
            let session = doc.sessions.iter().find(|session| session.session_id == args.session_id && session.owner_client_id == client_id).cloned().ok_or_else(|| DeckError::new(ErrorKind::Missing, "session not found"))?;
            check_control(&session, client_id, &args.expected_generation, args.control_epoch)?;
            if session.closing {
                return Err(DeckError::new(ErrorKind::Locked, "session is closing"));
            }
            let project = scoped_project(&client, &session.project_id)?;
            let cwd = canonical_scope(args.cwd.as_deref().unwrap_or(&session.cwd), &project.roots)?;
            let operation_id = random_id("op_")?;
            let job_id = random_id("job_")?;
            let binding = JobBinding { job_id: job_id.clone(), client_id: client_id.into(), session_id: session.session_id.clone(), session_generation: session.generation.clone(), request_hash: hash.clone(), operation_id: operation_id.clone() };
            let operation = Operation { operation_id, client_id: client_id.into(), request_id: args.request_id.clone(), request_hash: hash.clone(), kind: "exec".into(), state: "accepted".into(), code: None, result: Some(json!({"jobId":job_id,"sessionId":session.session_id,"sessionGeneration":session.generation})), accepted_at: now_ms(), updated_at: now_ms() };
            doc.jobs.push(binding.clone());
            doc.operations.push(operation.clone());
            Ok((operation, Some(binding), Some((session, cwd))))
        })
        .map_err(map_error)?;
    let (mut operation, binding, dispatch) = prepared;
    let Some(binding) = binding else {
        return Ok(operation_view(&operation));
    };
    let Some((session, cwd)) = dispatch else {
        let runner = send_runner(
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
    let runner = send_runner(
        &session,
        &json!({"kind":"exec","job_id":binding.job_id,"request_hash":hash,"script":args.script,"cwd":cwd,"wait_ms":args.wait_ms.unwrap_or(1_000),"timeout_ms":args.execution_timeout_ms}),
    );
    let committed = runner
        .as_ref()
        .ok()
        .is_some_and(|value| value.get("ok").and_then(Value::as_bool) == Some(true));
    let rejected = runner.as_ref().ok().and_then(runner_error);
    let _ = runtime.write(|doc| {
        if let Some(saved) = doc
            .operations
            .iter_mut()
            .find(|saved| saved.operation_id == operation.operation_id)
        {
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
                        .unwrap_or("dispatch-unknown")
                        .into(),
                )
            };
            saved.updated_at = now_ms();
            operation = saved.clone();
        }
        Ok(())
    });
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
    let cursor = decode_cursor(args.cursor, &binding)?;
    let max_bytes = args.max_bytes.unwrap_or(32 * 1024);
    if !(4..=64 * 1024).contains(&max_bytes) || args.wait_ms.unwrap_or(0) > 5_000 {
        return Err(error_value(
            "INVALID_ARGUMENTS",
            "read limits are invalid",
            "Use max_bytes 4..65536 and wait_ms up to 5000.",
        ));
    }
    let value = send_runner(&session, &json!({"kind":"read","job_id":binding.job_id,"cursor":cursor,"max_bytes":max_bytes,"wait_ms":args.wait_ms.unwrap_or(0)})).map_err(map_error)?;
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
        json!({"ok":true,"sessionId":session.session_id,"sessionGeneration":session.generation,"jobId":binding.job_id,"state":value.get("job").and_then(|job| job.get("state")).cloned().unwrap_or(json!("unknown")),"exitCode":value.get("job").and_then(|job| job.get("exitCode")).cloned().unwrap_or(Value::Null),"terminationSignal":value.get("job").and_then(|job| job.get("terminationSignal")).cloned().unwrap_or(Value::Null),"interruptRequested":value.get("job").and_then(|job| job.get("interruptRequested")).cloned().unwrap_or(json!(false)),"timeoutRequested":value.get("job").and_then(|job| job.get("timeoutRequested")).cloned().unwrap_or(json!(false)),"output":value.get("output").cloned().unwrap_or(json!("")),"outputKind":"pty_combined","nextCursor":format!("{}:{}:{}",session.generation,binding.job_id,next),"truncated":value.get("gap").cloned().unwrap_or(json!(false)),"gap":value.get("gap").cloned().unwrap_or(json!(false)),"droppedBytes":value.get("droppedBytes").cloned().unwrap_or(json!(0)),"startedAt":value.get("job").and_then(|job| job.get("startedAt")).cloned().unwrap_or(Value::Null),"endedAt":value.get("job").and_then(|job| job.get("endedAt")).cloned().unwrap_or(Value::Null),"nextAction":if value.get("job").and_then(|job| job.get("state")).and_then(Value::as_str)==Some("exited") {"Evaluate the exit code and output; exit 0 alone does not prove the requested work is correct."} else {"Continue reading with nextCursor; do not submit the original command again."}}),
    )
}

fn job_side_effect(
    runtime: &Runtime,
    client_id: &str,
    tool: &str,
    arguments: Value,
) -> Result<Value, Value> {
    let _delivery = runtime.delivery.lock_or_recover();
    let (request_id, job_id, generation, epoch, input) = if tool == "deck_job_input" {
        let args: InputArgs = parse(arguments.clone())?;
        if args.input.len() > MAX_INPUT_BYTES {
            return Err(error_value(
                "INVALID_ARGUMENTS",
                "input exceeds its bound",
                "Send a smaller input chunk.",
            ));
        }
        (
            args.request_id,
            args.job_id,
            args.session_generation,
            args.control_epoch,
            Some(args.input),
        )
    } else {
        let args: InterruptArgs = parse(arguments.clone())?;
        (
            args.request_id,
            args.job_id,
            args.session_generation,
            args.control_epoch,
            None,
        )
    };
    if !valid_id(&request_id) {
        return Err(error_value(
            "INVALID_ARGUMENTS",
            "request id is invalid",
            "Use a stable opaque request id.",
        ));
    }
    let hash = request_hash(tool, &arguments);
    let prepared = runtime
        .write(|doc| {
            client(doc, client_id)?;
            if let Some(existing) = existing_operation(doc, client_id, &request_id, &hash)? {
                return Ok((existing.clone(), None));
            }
            if doc.operations.len() >= MAX_OPERATIONS {
                return Err(DeckError::new(
                    ErrorKind::DiskFull,
                    "MCP operation capacity reached",
                ));
            }
            let binding = doc
                .jobs
                .iter()
                .find(|job| job.job_id == job_id && job.client_id == client_id)
                .cloned()
                .ok_or_else(|| DeckError::new(ErrorKind::Missing, "job not found"))?;
            let session = authorized_session(doc, client_id, &binding.session_id)?.clone();
            check_control(&session, client_id, &generation, epoch)?;
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
            };
            doc.operations.push(operation.clone());
            Ok((operation, Some((binding, session))))
        })
        .map_err(map_error)?;
    let (operation, target) = prepared;
    let Some((binding, session)) = target else {
        return Ok(operation_view(&operation));
    };
    let request = if let Some(input) = input {
        json!({"kind":"input","job_id":binding.job_id,"data_b64":base64::engine::general_purpose::STANDARD.encode(input.as_bytes())})
    } else {
        json!({"kind":"interrupt","job_id":binding.job_id})
    };
    let result = send_runner(&session, &request);
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
    let _delivery = runtime.delivery.lock_or_recover();
    let args: CloseArgs = parse(arguments.clone())?;
    if !valid_id(&args.request_id) {
        return Err(error_value(
            "INVALID_ARGUMENTS",
            "request id is invalid",
            "Use a stable opaque request id.",
        ));
    }
    let hash = request_hash("deck_session_close", &arguments);
    let operation = runtime.write(|doc| {
        client(doc, client_id)?;
        if let Some(existing) = existing_operation(doc, client_id, &args.request_id, &hash)? { return Ok(existing.clone()); }
        if doc.operations.len() >= MAX_OPERATIONS {
            return Err(DeckError::new(ErrorKind::DiskFull, "MCP operation capacity reached"));
        }
        let session = authorized_session(doc, client_id, &args.session_id)?.clone();
        check_control(&session, client_id, &args.expected_generation, args.control_epoch)?;
        let active = send_runner(&session, &json!({"kind":"ping"}))
            .ok()
            .and_then(|value| value.get("job").cloned())
            .is_some_and(|job| !job.is_null());
        if active && !args.confirm_running { return Err(DeckError::new(ErrorKind::Locked, "session has a running job")); }
        let managed = doc.sessions.iter_mut().find(|item| item.session_id == session.session_id).ok_or_else(|| DeckError::new(ErrorKind::Missing, "session not found"))?;
        managed.closing = true;
        let operation = Operation { operation_id:random_id("op_")?, client_id:client_id.into(), request_id:args.request_id, request_hash:hash, kind:"session-close".into(), state:"accepted".into(), code:None, result:Some(json!({"sessionId":session.session_id,"cardId":session.card_id,"confirmRunning":args.confirm_running})), accepted_at:now_ms(), updated_at:now_ms() };
        doc.operations.push(operation.clone());
        Ok(operation)
    }).map_err(map_error)?;
    emit_changed(runtime);
    Ok(operation_view(&operation))
}

fn route(runtime: &Runtime, request: WireRequest) -> Value {
    if request.version != VERSION || !valid_id(&request.client_id) {
        return error_value(
            "AUTH_REQUIRED",
            "invalid MCP control request",
            "Use the bundled deck-mcp adapter and an authorized client id.",
        );
    }
    let result = match request.tool.as_str() {
        "deck_capabilities" => capabilities(runtime, &request.client_id, request.arguments),
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

fn handle_connection(runtime: Arc<Runtime>, mut stream: UnixStream) {
    if !same_uid(&stream) {
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
    });
    let _ = RUNTIME.set(runtime.clone());
    std::thread::Builder::new()
        .name("deck-mcp-control".into())
        .spawn(move || {
            let _ = std::fs::remove_file(&socket);
            let listener = match UnixListener::bind(&socket) {
                Ok(listener) => listener,
                Err(_) => return,
            };
            if std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).is_err() {
                let _ = std::fs::remove_file(&socket);
                return;
            }
            for stream in listener.incoming().flatten() {
                let runtime = runtime.clone();
                std::thread::spawn(move || handle_connection(runtime, stream));
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
    runtime.read(|doc| StatusView {
        enabled: doc.config.enabled,
        socket_ready: runtime.socket.exists(),
        clients: doc
            .config
            .clients
            .iter()
            .map(|client| ClientView {
                id: client.id.clone(),
                name: client.name.clone(),
                revoked: client.revoked_at.is_some(),
                allow_create: client.allow_create,
                projects: client.projects.clone(),
            })
            .collect(),
    })
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

#[tauri::command]
pub(crate) fn mcp_enable() -> Result<(), DeckError> {
    runtime()?.write(|doc| {
        doc.config.enabled = true;
        Ok(())
    })
}

#[tauri::command]
pub(crate) fn mcp_disable() -> Result<(), DeckError> {
    let runtime = runtime()?;
    let _delivery = runtime.delivery.lock_or_recover();
    let sessions = runtime.write(|doc| {
        doc.config.enabled = false;
        let sessions = doc.sessions.clone();
        let closing = doc
            .operations
            .iter()
            .filter(|operation| operation.state == "accepted" && operation.kind == "session-close")
            .filter_map(|operation| operation.result.as_ref()?.get("sessionId")?.as_str())
            .map(str::to_owned)
            .collect::<HashSet<_>>();
        for session in &mut doc.sessions {
            session.control_owner = None;
            session.control_epoch = session.control_epoch.saturating_add(1);
            session.lease_expires_at = None;
            session.human_lock = true;
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
        Ok(sessions)
    })?;
    for session in sessions {
        let _ = send_runner(&session, &json!({"kind":"control","mode":"human"}));
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn mcp_client_add(
    name: String,
    projects: Vec<ProjectScopeInput>,
) -> Result<ClientView, DeckError> {
    if !valid_title(&name) || projects.is_empty() || projects.len() > MAX_PROJECTS_PER_CLIENT {
        return Err(DeckError::new(
            ErrorKind::Invalid,
            "invalid MCP client scope",
        ));
    }
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
            .map(|root| {
                std::fs::canonicalize(root)
                    .map(|path| path.display().to_string())
                    .map_err(|_| {
                        DeckError::new(ErrorKind::NotDir, "authorized root is unavailable")
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        scopes.push(ProjectScope {
            project_id: project.project_id,
            roots,
        });
    }
    runtime()?.write(|doc| {
        if doc.config.clients.len() >= MAX_CLIENTS {
            return Err(DeckError::new(
                ErrorKind::DiskFull,
                "MCP client capacity reached",
            ));
        }
        let client = Client {
            id: random_id("client_")?,
            name,
            revoked_at: None,
            allow_create: true,
            projects: scopes,
        };
        doc.config.clients.push(client.clone());
        Ok(ClientView {
            id: client.id,
            name: client.name,
            revoked: false,
            allow_create: client.allow_create,
            projects: client.projects,
        })
    })
}

#[tauri::command]
pub(crate) fn mcp_client_revoke(client_id: String) -> Result<(), DeckError> {
    let runtime = runtime()?;
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
            let sessions = doc
                .sessions
                .iter()
                .filter(|session| session.owner_client_id == client_id)
                .cloned()
                .collect::<Vec<_>>();
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
            for session in doc
                .sessions
                .iter_mut()
                .filter(|session| session.owner_client_id == client_id)
            {
                session.control_owner = None;
                session.control_epoch = session.control_epoch.saturating_add(1);
                session.lease_expires_at = None;
                session.human_lock = true;
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
            Ok(sessions)
        })?;
    for session in sessions {
        let _ = send_runner(&session, &json!({"kind":"control","mode":"human"}));
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
    runtime()?.write(|doc| {
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
    runtime()?.read(|doc| {
        if !doc.config.enabled {
            return Err(DeckError::new(ErrorKind::Perm, "MCP control is disabled"));
        }
        let operation = doc
            .operations
            .iter()
            .find(|operation| {
                operation.operation_id == operation_id && operation.state == "executing"
            })
            .ok_or_else(|| {
                DeckError::new(
                    ErrorKind::ContextChanged,
                    "MCP operation is no longer executable",
                )
            })?;
        client(doc, &operation.client_id).map(|_| ())
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
        Ok::<Value, DeckError>(operation.result.clone().unwrap_or(Value::Null))
    })??;
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
    if crate::tmux::tmux(&["has-session", "-t", &crate::tmux::session_target(&name)]).is_ok() {
        if runner_socket_matches(socket, generation) {
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
    if !matches!(state.as_str(), "committed" | "rejected" | "ambiguous") {
        return Err(DeckError::new(
            ErrorKind::Invalid,
            "invalid MCP operation result",
        ));
    }
    let runtime = runtime()?;
    let _delivery = runtime.delivery.lock_or_recover();
    let switch_to_human = runtime.write(|doc| {
        let index = doc
            .operations
            .iter()
            .position(|operation| {
                operation.operation_id == operation_id && operation.state == "executing"
            })
            .ok_or_else(|| DeckError::new(ErrorKind::Missing, "MCP operation not executing"))?;
        let operation = doc.operations[index].clone();
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
                control_owner: authorized.then(|| operation.client_id.clone()),
                control_epoch: 1,
                lease_expires_at: authorized.then(|| now_ms() + DEFAULT_LEASE_MS),
                human_lock: !authorized,
                closing: false,
                created_at: now_ms(),
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
        Ok(created_session)
    })?;
    if let Some(session) = switch_to_human {
        let _ = send_runner(&session, &json!({"kind":"control","mode":"human"}));
    }
    Ok(())
}

/// Reconcile an explicit local Board close that did not originate as an MCP
/// operation. The card is already durably absent when this is called.
#[tauri::command]
pub(crate) fn mcp_card_closed(card_id: String) -> Result<(), DeckError> {
    runtime()?.write(|doc| {
        let removed = doc
            .sessions
            .iter()
            .filter(|session| session.card_id == card_id)
            .map(|session| session.session_id.clone())
            .collect::<HashSet<_>>();
        doc.sessions.retain(|session| session.card_id != card_id);
        doc.jobs.retain(|job| !removed.contains(&job.session_id));
        Ok(())
    })
}

#[tauri::command]
pub(crate) fn mcp_takeover(session_id: String) -> Result<(), DeckError> {
    let runtime = runtime()?;
    let _delivery = runtime.delivery.lock_or_recover();
    let session = runtime.write(|doc| {
        let session = doc
            .sessions
            .iter_mut()
            .find(|session| session.session_id == session_id || session.card_id == session_id)
            .ok_or_else(|| DeckError::new(ErrorKind::Missing, "MCP session not found"))?;
        session.control_owner = None;
        session.control_epoch = session.control_epoch.saturating_add(1);
        session.lease_expires_at = None;
        session.human_lock = true;
        Ok(session.clone())
    })?;
    let response = send_runner(&session, &json!({"kind":"control","mode":"human"}))?;
    if response.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(())
    } else {
        Err(DeckError::new(
            ErrorKind::ContextChanged,
            "MCP writes are fenced but terminal handoff is uncertain",
        ))
    }
}

#[tauri::command]
pub(crate) fn mcp_return_control(session_id: String) -> Result<(), DeckError> {
    let runtime = runtime()?;
    let _delivery = runtime.delivery.lock_or_recover();
    let session = runtime.read(|doc| {
        let session = doc
            .sessions
            .iter()
            .find(|session| session.session_id == session_id || session.card_id == session_id)
            .cloned()
            .ok_or_else(|| DeckError::new(ErrorKind::Missing, "MCP session not found"))?;
        client(doc, &session.owner_client_id)?;
        Ok::<ManagedSession, DeckError>(session)
    })??;
    let response = send_runner(&session, &json!({"kind":"control","mode":"mcp"}))?;
    if response.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(DeckError::new(
            ErrorKind::Locked,
            "managed job is still running",
        ));
    }
    runtime.write(|doc| {
        let session = doc
            .sessions
            .iter_mut()
            .find(|item| item.card_id == session.card_id && item.generation == session.generation)
            .ok_or_else(|| {
                DeckError::new(ErrorKind::ContextChanged, "MCP session generation changed")
            })?;
        session.human_lock = false;
        session.control_epoch = session.control_epoch.saturating_add(1);
        session.control_owner = Some(session.owner_client_id.clone());
        session.lease_expires_at = Some(now_ms() + DEFAULT_LEASE_MS);
        Ok(())
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionUiView {
    managed: bool,
    human_control: bool,
    control_owner: Option<String>,
    control_epoch: u64,
    active_job: bool,
    client_name: Option<String>,
    job_state: Option<String>,
    recent_error: Option<String>,
}

#[tauri::command]
pub(crate) fn mcp_session_ui(card_id: String) -> Result<SessionUiView, DeckError> {
    let session = runtime()?.read(|doc| {
        doc.sessions
            .iter()
            .find(|session| session.card_id == card_id)
            .cloned()
    })?;
    let Some(session) = session else {
        return Ok(SessionUiView {
            managed: false,
            human_control: false,
            control_owner: None,
            control_epoch: 0,
            active_job: false,
            client_name: None,
            job_state: None,
            recent_error: None,
        });
    };
    let (client_name, recent_error) = runtime()?.read(|doc| {
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
        (name, error)
    })?;
    let runner = send_runner(&session, &json!({"kind":"ping"})).ok();
    let job_state = runner
        .as_ref()
        .and_then(|value| value.get("job"))
        .and_then(|job| job.get("state"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok(SessionUiView {
        managed: true,
        human_control: session.human_lock,
        control_owner: session.control_owner,
        control_epoch: session.control_epoch,
        active_job: job_state.is_some(),
        client_name,
        job_state,
        recent_error,
    })
}

/// All ordinary Deck terminal-input paths call this before writing. A managed
/// runner also discards pane stdin while MCP owns control, closing the check /
/// write race for keyboard input already queued at takeover.
pub(crate) fn guard_terminal_input(tmux_session: &str) -> Result<(), DeckError> {
    let Some(runtime) = RUNTIME.get() else {
        return Ok(());
    };
    runtime.read(|doc| {
        if let Some(session) = doc
            .sessions
            .iter()
            .find(|session| session.tmux_session == tmux_session)
        {
            if !session.human_lock {
                return Err(DeckError::new(ErrorKind::Perm, "MCP owns terminal control"));
            }
        }
        Ok(())
    })?
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

    struct FakeRunner {
        socket: PathBuf,
        stop: Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl FakeRunner {
        fn start(root: &Path, generation: &str) -> Self {
            let socket = root.join("runner.sock");
            let listener = UnixListener::bind(&socket).unwrap();
            listener.set_nonblocking(true).unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let thread_stop = stop.clone();
            let generation = generation.to_owned();
            let thread = std::thread::spawn(move || {
                while !thread_stop.load(Ordering::SeqCst) {
                    let Ok((mut stream, _)) = listener.accept() else {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    };
                    let mut line = String::new();
                    if BufReader::new(stream.try_clone().unwrap())
                        .read_line(&mut line)
                        .is_err()
                    {
                        continue;
                    }
                    let request: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
                    let kind = request.get("kind").and_then(Value::as_str).unwrap_or("");
                    let job_id = request
                        .get("job_id")
                        .and_then(Value::as_str)
                        .unwrap_or("job_a");
                    let response = match kind {
                        "ping" => json!({"ok":true,"generation":generation,"job":null}),
                        "read" => json!({
                            "ok":true,
                            "generation":generation,
                            "job":{"jobId":job_id,"state":"exited","exitCode":0,
                                "terminationSignal":null,"interruptRequested":false,
                                "timeoutRequested":false,"startedAt":1,"endedAt":2},
                            "output":"done\n","nextCursor":5,"gap":false,"droppedBytes":0
                        }),
                        "exec" => json!({
                            "ok":true,
                            "generation":generation,
                            "job":{"jobId":job_id,"state":"exited","exitCode":0}
                        }),
                        "input" | "interrupt" | "control" => {
                            json!({"ok":true,"generation":generation})
                        }
                        _ => json!({"ok":false,"generation":generation,"error":"invalid-request"}),
                    };
                    serde_json::to_writer(&mut stream, &response).unwrap();
                    stream.write_all(b"\n").unwrap();
                }
            });
            Self {
                socket,
                stop,
                thread: Some(thread),
            }
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
        let root = std::env::temp_dir().join(format!(
            "deck-mcp-{tag}-{}-{}",
            std::process::id(),
            now_ms()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn client_record(root: &Path) -> Client {
        Client {
            id: "client_a".into(),
            name: "Client A".into(),
            revoked_at: None,
            allow_create: true,
            projects: vec![ProjectScope {
                project_id: "P1".into(),
                roots: vec![root.display().to_string()],
            }],
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
            control_epoch: 1,
            lease_expires_at: Some(now_ms() + 60_000),
            human_lock: false,
            closing: false,
            created_at: now_ms(),
        }
    }

    fn request(tool: &str, arguments: Value) -> WireRequest {
        WireRequest {
            version: VERSION,
            client_id: "client_a".into(),
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
        }
    }

    #[test]
    fn fresh_state_is_disabled_and_strictly_bounded() {
        let doc = DiskDoc::default();
        assert!(!doc.config.enabled);
        validate_doc(&doc).unwrap();
        assert_eq!(MAX_SCRIPT_BYTES, 128 * 1024);
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
            control_epoch: 2,
            lease_expires_at: None,
            human_lock: true,
            closing: false,
            created_at: 1,
        };
        let error = check_control(&session, "client_a", "g_a", 1).unwrap_err();
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
            revoked_at: None,
            allow_create: true,
            projects: vec![ProjectScope {
                project_id: "P1".into(),
                roots: vec![root.clone()],
            }],
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
            control_epoch: 2,
            lease_expires_at: Some(now_ms() + 10_000),
            human_lock: false,
            closing: false,
            created_at: 1,
        });
        assert!(authorized_session(&doc, "client_missing", "mcp_a").is_err());
        let session = authorized_session(&doc, "client_a", "mcp_a").unwrap();
        assert!(check_control(session, "client_a", "g_other", 2).is_err());
        assert!(check_control(session, "client_a", "g_a", 1).is_err());
        assert!(check_control(session, "client_a", "g_a", 2).is_ok());
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
        });
        assert!(RUNTIME.set(runtime.clone()).is_ok());

        let status = mcp_status().unwrap();
        assert!(status.enabled);
        assert!(!status.socket_ready);
        assert_eq!(status.clients.len(), 1);
        assert!(mcp_adapter_path().is_err());
        assert!(mcp_client_add("".into(), vec![]).is_err());
        assert!(mcp_client_add(
            "Bad scope".into(),
            vec![ProjectScopeInput {
                project_id: "bad id".into(),
                roots: vec![root.display().to_string()],
            }],
        )
        .is_err());
        let added = mcp_client_add(
            "Client B".into(),
            vec![ProjectScopeInput {
                project_id: "P2".into(),
                roots: vec![root.display().to_string()],
            }],
        )
        .unwrap();
        assert_eq!(added.name, "Client B");

        let unmanaged = mcp_session_ui("missing".into()).unwrap();
        assert!(!unmanaged.managed);
        let managed = mcp_session_ui("M1".into()).unwrap();
        assert!(managed.managed);
        assert!(!managed.human_control);
        assert_eq!(managed.client_name.as_deref(), Some("Client A"));
        assert!(guard_terminal_input("deck-mcp-test").is_err());
        assert!(guard_server_restart().is_err());
        assert!(runner_socket_matches(
            runner.socket.to_str().unwrap(),
            "g_a"
        ));
        assert!(!runner_socket_matches(
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
            json!({"request_id":"command_create","project_id":"P1","cwd":root.display().to_string()}),
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
                "confirm_running":false
            }),
        )
        .unwrap();
        let close_id = close["operationId"].as_str().unwrap().to_owned();
        mcp_claim(close_id.clone()).unwrap();
        mcp_validate(close_id.clone()).unwrap();
        mcp_complete(close_id, "committed".into(), None, None).unwrap();

        assert!(mcp_claim("missing".into()).is_err());
        assert!(mcp_validate("missing".into()).is_err());
        assert!(mcp_start_session(
            "missing".into(),
            "deck-valid".into(),
            root.display().to_string()
        )
        .is_err());
        mcp_client_revoke(added.id).unwrap();
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
        });

        assert_eq!(
            route(
                &runtime,
                WireRequest {
                    version: 99,
                    client_id: "client_a".into(),
                    tool: "deck_capabilities".into(),
                    arguments: json!({}),
                }
            )["error"]["code"],
            "AUTH_REQUIRED"
        );
        assert_eq!(
            route(&runtime, request("unknown", json!({})))["error"]["code"],
            "UNSUPPORTED"
        );
        assert!(route(&runtime, request("deck_capabilities", json!({})))["ok"] == true);
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
                "lease_ms":2_000
            });
            if let Some(epoch) = epoch {
                value["control_epoch"] = json!(epoch);
            }
            route(&runtime, request("deck_session_control", value))
        };
        assert_eq!(
            control("control_request", "request", None)["state"],
            "committed"
        );
        assert_eq!(
            control("control_renew", "renew", Some(2))["state"],
            "committed"
        );
        assert_eq!(
            control("control_release", "release", Some(2))["state"],
            "committed"
        );
        assert_eq!(
            control("control_again", "request", None)["state"],
            "committed"
        );

        let exec_arguments = json!({
            "request_id":"exec_a",
            "session_id":"mcp_a",
            "expected_generation":"g_a",
            "control_epoch":4,
            "script":"printf 'done\\n'",
            "cwd":root.display().to_string(),
            "wait_ms":10,
            "execution_timeout_ms":1_000
        });
        let executed = route(&runtime, request("deck_exec", exec_arguments.clone()));
        assert_eq!(executed["state"], "exited");
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
                    json!({"request_id":"input_a","job_id":job_id,"session_generation":"g_a","control_epoch":4,"input":"yes\n"}),
                ),
            )["state"],
            "committed"
        );
        assert_eq!(
            route(
                &runtime,
                request(
                    "deck_job_interrupt",
                    json!({"request_id":"interrupt_a","job_id":job_id,"session_generation":"g_a","control_epoch":4}),
                ),
            )["state"],
            "committed"
        );

        let create = route(
            &runtime,
            request(
                "deck_session_create",
                json!({"request_id":"create_a","project_id":"P1","cwd":root.display().to_string(),"title":"Visible shell"}),
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
                json!({"request_id":"close_a","session_id":"mcp_a","expected_generation":"g_a","control_epoch":4,"confirm_running":false}),
            ),
        );
        assert_eq!(close["state"], "accepted", "{close}");

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

        drop(runtime);
        drop(runner);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn validation_and_error_mapping_cover_rejected_boundaries() {
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
        assert_eq!(load(&root.join("absent.json")).unwrap().version, VERSION);

        let mut invalid = DiskDoc {
            version: VERSION + 1,
            ..DiskDoc::default()
        };
        assert!(validate_doc(&invalid).is_err());
        invalid.version = VERSION;
        invalid.config.clients.push(Client {
            id: "bad id".into(),
            name: "Bad".into(),
            revoked_at: None,
            allow_create: false,
            projects: vec![],
        });
        assert!(validate_doc(&invalid).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
