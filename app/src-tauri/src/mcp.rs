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
//! `mcp_fs.rs` and never start a shell or repository helper. The
//! listener is disabled by default and is a 0600 Unix socket under Deck's
//! private data directory; every connection must also have Deck's effective
//! uid. The thin `deck-mcp` sidecar provides MCP STDIO and never receives a
//! Phone token or unrestricted backend credential.
//!
//! Board creation and close intents are journaled here, then handed to the
//! webview's one serialized Board transaction (`mcp.js`). The managed runner
//! reports process exit and output EOF independently from that control-operation state. Scripts
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
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

use crate::error::{DeckError, ErrorKind};
use crate::sync::LockRecover;

const STATE_VERSION: u32 = 3;
const CONTROL_PROTOCOL: u32 = 3;
const MAX_STATE_BYTES: usize = 8 * 1024 * 1024;
const MAX_REQUEST_BYTES: usize = 256 * 1024;
const MAX_RESPONSE_BYTES: usize = 128 * 1024;
const MAX_CLIENTS: usize = 32;
const MAX_PROJECTS_PER_CLIENT: usize = 64;
const MAX_ROOTS_PER_PROJECT: usize = 16;
const MAX_SESSIONS: usize = 64;
const MAX_OPERATIONS: usize = 2000;
const MAX_JOBS: usize = 1000;
const MAX_AUDIT_EVENTS: usize = 2_000;
const AUDIT_RETENTION_MS: u64 = 30 * 24 * 60 * 60_000;
const MAX_SCRIPT_BYTES: usize = 32 * 1024;
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
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis() as u64)
        .unwrap_or(0)
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
    started: Instant,
}

#[derive(Default)]
struct EmergencyFences {
    disabled: bool,
    clients: HashSet<String>,
    human_sessions: HashSet<String>,
    execution_sessions: HashSet<String>,
}

impl Runtime {
    fn monotonic_ms(&self) -> u64 {
        self.started.elapsed().as_millis().min(u64::MAX as u128) as u64
    }
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
            session.control_holder = None;
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
                && operation_ids.contains(&job.operation_id)
        });
    let mut grant_ids = HashSet::new();
    let grants_valid = doc.execution_grants.len() <= MAX_OPERATIONS
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

fn map_error(error: DeckError) -> Value {
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
    if value.get("generation").and_then(Value::as_str) != Some(&session.generation) {
        return Err(DeckError::new(
            ErrorKind::ContextChanged,
            "managed runner generation changed",
        ));
    }
    Ok(value)
}

fn send_runner_control(
    runtime: &Runtime,
    session: &ManagedSession,
    mode: &str,
) -> Result<Value, DeckError> {
    send_runner(
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
        "invalid-cwd" | "invalid-script" | "invalid-request" => Some((
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
    #[serde(default)]
    holder_id: Option<String>,
}

#[derive(Clone, Copy, Deserialize)]
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
    holder_id: String,
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
    holder_id: String,
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
    holder_id: String,
    input: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InterruptArgs {
    request_id: String,
    job_id: String,
    session_generation: String,
    control_epoch: u64,
    holder_id: String,
}

#[derive(Deserialize)]
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

fn project_list(runtime: &Runtime, client_id: &str, arguments: Value) -> Result<Value, Value> {
    let args: ProjectPathArgs = parse(arguments)?;
    let (root, roots) = read_root(runtime, client_id, &args.project_id, args.root_index)?;
    let (entries, version) = crate::mcp_fs::list(&root, &args.path).map_err(|message| {
        error_value(
            "READ_DENIED",
            &message,
            "Choose a regular, non-sensitive path inside the approved root.",
        )
    })?;
    recheck_read(runtime, client_id, &args.project_id, &roots)?;
    Ok(
        json!({"ok":true,"projectId":args.project_id,"rootIndex":args.root_index,"path":args.path,"entries":entries,"version":version,"truncated":false}),
    )
}

fn project_read(runtime: &Runtime, client_id: &str, arguments: Value) -> Result<Value, Value> {
    let args: FileReadArgs = parse(arguments)?;
    let (root, roots) = read_root(runtime, client_id, &args.project_id, args.root_index)?;
    // Read metadata/content once at offset zero to obtain the descriptor-bound
    // version. Subsequent cursor validation binds that version before slicing.
    let file_version = crate::mcp_fs::identity(&root, &args.path).map_err(|message| {
        error_value(
            "READ_DENIED",
            &message,
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
    .map_err(|message| {
        error_value(
            "READ_DENIED",
            &message,
            "Restart if the file changed, or choose a smaller regular UTF-8 file.",
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
    .map_err(|message| {
        error_value(
            "SEARCH_LIMIT_OR_DENIED",
            &message,
            "Narrow the literal query or search path.",
        )
    })?;
    let results = outcome.matches;
    let search_complete = outcome.complete;
    let stop_reason = outcome.stop_reason;
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
        let response = json!({"ok":true,"projectId":args.project_id,"path":args.path,"query":args.query,"matches":&results[offset..end],"nextCursor":cursor,"truncated":!search_complete || end < results.len(),"complete":search_complete && end == results.len(),"stopReason":stop_reason});
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
                "deckConnection": "connected",
                "controlProtocolVersion": CONTROL_PROTOCOL,
                "stateSchemaVersion": STATE_VERSION,
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
                "featureEnabled": doc.config.enabled,
                "manualActionRequired": if doc.config.enabled { Value::Null } else { json!("Open Deck Settings and enable MCP terminal control.") }
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
                "controlHolder": session.control_holder,
                "controlEpoch": session.control_epoch,
                "activeJob": runner.as_ref().and_then(|value| value.get("job")).cloned(),
                "foreground": runner.as_ref().and_then(|value| value.get("job")).is_some_and(|job| !job.is_null()).then_some("managed-job"),
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
    let title = args.title.unwrap_or_else(|| "MCP shell".into());
    let operation = runtime
        .write(|doc| {
            client(doc, client_id)?;
            // Request-key lookup, fingerprint comparison, capacity reservation,
            // and insertion are one persisted transaction. A separate read here
            // allowed two concurrent callers to reserve two create plans.
            if let Some(existing) = existing_operation(doc, client_id, &args.request_id, &hash)? {
                return Ok(existing.clone());
            }
            if doc.operations.len() >= MAX_OPERATIONS || doc.sessions.len() >= MAX_SESSIONS {
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
                admission_hash: None,
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
    let emergency_human = runtime
        .emergency
        .lock_or_recover()
        .human_sessions
        .contains(&session.session_id);
    let (authorization_status, authorization_expiry, stdin_approved) = runtime
        .read(|doc| {
            Ok(
                match execution_authorization(runtime, doc, client_id, &session) {
                    ExecutionAuthorization::None => ("none", None, false),
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
    let denial = if runner.is_err() {
        Some("RUNNER_UNAVAILABLE")
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
    } else if runner
        .as_ref()
        .ok()
        .and_then(|value| value.get("job"))
        .is_some_and(|job| !job.is_null())
    {
        Some("SESSION_BUSY")
    } else {
        runtime
            .read(|doc| active_execution_grant(runtime, doc, client_id, &session, false).is_err())
            .map_err(map_error)?
            .then_some("EXECUTION_GRANT_REQUIRED")
    };
    let terminal = (session.output_shared && !session.human_lock && !emergency_human)
        .then(|| {
            crate::tmux::tmux(&[
                "capture-pane",
                "-p",
                "-S",
                "-40",
                "-t",
                &crate::tmux::pane_target(&session.tmux_session),
            ])
        })
        .and_then(Result::ok)
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
        "controlHolder": session.control_holder,
        "controlEpoch": session.control_epoch,
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
        "activeJob": runner.as_ref().ok().and_then(|value| value.get("job")).cloned(),
        "foreground": runner.as_ref().ok().and_then(|value| value.get("job")).is_some().then_some("managed-job"),
        "readiness": "unknown",
        "terminalContext": terminal,
        "terminalContextBounded": true,
        "stale": runner.is_err(),
        "mayStartNextJob": denial.is_none(),
        "mayStartNextJobReason": denial,
    }))
}

fn session_control(runtime: &Runtime, client_id: &str, arguments: Value) -> Result<Value, Value> {
    let _delivery = runtime.delivery.lock_or_recover();
    let args: ControlArgs = parse(arguments.clone())?;
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
    let hash = request_hash("deck_session_control", &arguments);
    let action = args.action;
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
            let result = json!({"sessionId":session.session_id,"sessionGeneration":session.generation,"controlOwner":session.control_owner,"controlHolder":session.control_holder,"controlEpoch":session.control_epoch,"leaseExpiresAt":session.lease_expires_at});
            let session_id = session.session_id.clone();
            let operation = Operation { operation_id: random_id("op_")?, client_id: client_id.into(), request_id: args.request_id, request_hash: hash, kind: "session-control".into(), state: "committed".into(), code: None, result: Some(result), accepted_at: now_ms(), updated_at: now_ms(), admission_hash: None };
            doc.operations.push(operation.clone());
            audit(doc, "control-changed", AuditLink { principal_id: Some(client_id), session_id: Some(&session_id), operation_id: Some(&operation.operation_id), ..Default::default() })?;
            Ok(operation)
        })
        .map_err(map_error)?;
    let session_id = operation
        .result
        .as_ref()
        .and_then(|result| result.get("sessionId"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let session = runtime
        .read(|doc| authorized_session(doc, client_id, session_id).cloned())
        .map_err(map_error)?
        .map_err(map_error)?;
    let mode = if matches!(action, ControlAction::Release) {
        "fenced"
    } else {
        "mcp"
    };
    let fenced = send_runner_control(runtime, &session, mode)
        .ok()
        .is_some_and(|value| value.get("ok").and_then(Value::as_bool) == Some(true));
    if !fenced {
        return Err(error_value(
            "OPERATION_AMBIGUOUS",
            "the control decision is durable but the runner fence is unconfirmed",
            "Inspect the session locally; do not assume the prior holder can still run or is stopped.",
        ));
    }
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
    // Protocol-v2 fingerprinting is independent of JSON object key order.
    // The script itself is never journaled; only its byte length and digest
    // participate in the stable request fingerprint.
    let script_digest = sha(args.script.as_bytes());
    let hash = sha(serde_json::to_vec(&json!([
        "deck-exec-v2",
        &args.request_id,
        &args.session_id,
        &args.expected_generation,
        args.control_epoch,
        &args.holder_id,
        &args.cwd,
        args.wait_ms,
        args.execution_timeout_ms,
        args.script.len(),
        &script_digest,
        ENVIRONMENT_PROFILE,
        POLICY_VERSION
    ]))
    .unwrap_or_default()
    .as_slice());
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
            let operation = Operation { operation_id, client_id: client_id.into(), request_id: args.request_id.clone(), request_hash: hash.clone(), kind: "exec".into(), state: "accepted".into(), code: None, result: Some(json!({"jobId":job_id,"sessionId":session.session_id,"sessionGeneration":session.generation,"executionGrantId":grant.grant_id,"executionGrantVersion":grant.grant_version,"policyVersion":POLICY_VERSION,"environmentProfile":ENVIRONMENT_PROFILE,"scriptDigest":script_digest,"scriptLength":args.script.len(),"cwd":cwd})), accepted_at: now_ms(), updated_at: now_ms(), admission_hash: None };
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
    // Final admission is deliberately adjacent to runner dispatch. Revocation,
    // takeover, expiry, generation changes, and policy changes after the
    // durable intent was accepted prevent the side effect.
    let grant = runtime
        .read(|doc| {
            let current = authorized_session(doc, client_id, &session.session_id)?;
            check_control(
                current,
                client_id,
                &args.expected_generation,
                args.control_epoch,
                &args.holder_id,
            )?;
            Ok::<ExecutionGrant, DeckError>(
                active_execution_grant(runtime, doc, client_id, current, false)?.clone(),
            )
        })
        .map_err(map_error)?
        .map_err(map_error)?;
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
    let runner = send_runner(
        &session,
        &json!({"kind":"exec","job_id":binding.job_id,"request_hash":hash,"script":args.script,"cwd":cwd,"wait_ms":args.wait_ms.unwrap_or(1_000),"timeout_ms":args.execution_timeout_ms,"context":context}),
    );
    let committed = runner
        .as_ref()
        .ok()
        .is_some_and(|value| value.get("ok").and_then(Value::as_bool) == Some(true));
    let rejected = runner.as_ref().ok().and_then(runner_error);
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
        audit(
            doc,
            "exec-dispatch",
            AuditLink {
                principal_id: Some(client_id),
                session_id: Some(&session.session_id),
                operation_id: Some(&operation.operation_id),
                job_id: Some(&binding.job_id),
                grant_id: audit_grant_id.as_deref(),
                reason_code: (!committed).then_some(if rejected.is_some() {
                    "runner-rejected"
                } else {
                    "dispatch-unknown"
                }),
            },
        )?;
        Ok(())
    });
    if journaled.is_err() {
        return Err(error_value(
            "OPERATION_AMBIGUOUS",
            "job dispatch occurred but its audit result could not be persisted",
            "Inspect the live job; do not re-execute it.",
        ));
    }
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
            if !binding.allow_output || !session.output_shared || session.human_lock {
                return Err(DeckError::new(
                    ErrorKind::Perm,
                    "job output sharing is paused",
                ));
            }
            Ok((binding, session))
        })
        .map_err(map_error)?
        .map_err(map_error)?;
    let cursor = decode_cursor(args.cursor, &binding)?;
    let max_bytes = args.max_bytes.unwrap_or(MAX_READ_BYTES);
    if !(4..=MAX_READ_BYTES).contains(&max_bytes) || args.wait_ms.unwrap_or(0) > 5_000 {
        return Err(error_value(
            "INVALID_ARGUMENTS",
            "read limits are invalid",
            "Use max_bytes 4..16384 and wait_ms up to 5000.",
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
        json!({"ok":true,"sessionId":session.session_id,"sessionGeneration":session.generation,"jobId":binding.job_id,"state":value.get("job").and_then(|job| job.get("state")).cloned().unwrap_or(json!("unknown")),"exitCode":value.get("job").and_then(|job| job.get("exitCode")).cloned().unwrap_or(Value::Null),"terminationSignal":value.get("job").and_then(|job| job.get("terminationSignal")).cloned().unwrap_or(Value::Null),"interruptRequested":value.get("job").and_then(|job| job.get("interruptRequested")).cloned().unwrap_or(json!(false)),"timeoutRequested":value.get("job").and_then(|job| job.get("timeoutRequested")).cloned().unwrap_or(json!(false)),"processComplete":matches!(value.get("job").and_then(|job| job.get("state")).and_then(Value::as_str),Some("exited" | "lost")),"stdoutEof":value.get("job").and_then(|job| job.get("stdoutEof")).cloned().unwrap_or(json!(false)),"stderrEof":value.get("job").and_then(|job| job.get("stderrEof")).cloned().unwrap_or(json!(false)),"outputComplete":value.get("job").and_then(|job| job.get("outputComplete")).cloned().unwrap_or(json!(false)),"output":value.get("output").cloned().unwrap_or(json!("")),"outputKind":"pty_combined","nextCursor":format!("{}:{}:{}",session.generation,binding.job_id,next),"truncated":value.get("gap").cloned().unwrap_or(json!(false)),"gap":value.get("gap").cloned().unwrap_or(json!(false)),"droppedBytes":value.get("droppedBytes").cloned().unwrap_or(json!(0)),"startedAt":value.get("job").and_then(|job| job.get("startedAt")).cloned().unwrap_or(Value::Null),"endedAt":value.get("job").and_then(|job| job.get("endedAt")).cloned().unwrap_or(Value::Null),"nextAction":if value.get("job").and_then(|job| job.get("outputComplete")).and_then(Value::as_bool)==Some(true) {"Evaluate the exit code and output; exit 0 alone does not prove the requested work is correct."} else {"Continue reading with nextCursor; process exit does not imply that all output has reached EOF."}}),
    )
}

fn job_side_effect(
    runtime: &Runtime,
    client_id: &str,
    tool: &str,
    arguments: Value,
) -> Result<Value, Value> {
    let _delivery = runtime.delivery.lock_or_recover();
    let (request_id, job_id, generation, epoch, holder_id, input) = if tool == "deck_job_input" {
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
            args.holder_id,
            Some(args.input),
        )
    } else {
        let args: InterruptArgs = parse(arguments.clone())?;
        (
            args.request_id,
            args.job_id,
            args.session_generation,
            args.control_epoch,
            args.holder_id,
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
            };
            doc.operations.push(operation.clone());
            Ok((operation, Some((binding, session, grant))))
        })
        .map_err(map_error)?;
    let (operation, target) = prepared;
    let Some((binding, session, grant)) = target else {
        return Ok(operation_view(&operation));
    };
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
        check_control(&session, client_id, &args.expected_generation, args.control_epoch, &args.holder_id)?;
        let runner = send_runner(&session, &json!({"kind":"ping"})).map_err(|_| {
            DeckError::new(
                ErrorKind::ContextChanged,
                "managed runner state is unknown; remote close is refused",
            )
        })?;
        let active = runner.get("job").is_some_and(|job| !job.is_null());
        if active && !args.confirm_running { return Err(DeckError::new(ErrorKind::Locked, "session has a running job")); }
        let managed = doc.sessions.iter_mut().find(|item| item.session_id == session.session_id).ok_or_else(|| DeckError::new(ErrorKind::Missing, "session not found"))?;
        managed.closing = true;
        let operation = Operation { operation_id:random_id("op_")?, client_id:client_id.into(), request_id:args.request_id, request_hash:hash, kind:"session-close".into(), state:"accepted".into(), code:None, result:Some(json!({"sessionId":session.session_id,"cardId":session.card_id,"sessionGeneration":session.generation,"controlEpoch":session.control_epoch,"holderId":args.holder_id,"confirmRunning":args.confirm_running})), accepted_at:now_ms(), updated_at:now_ms(), admission_hash:None };
        doc.operations.push(operation.clone());
        Ok(operation)
    }).map_err(map_error)?;
    emit_changed(runtime);
    Ok(operation_view(&operation))
}

fn route(runtime: &Runtime, request: WireRequest) -> Value {
    if request.version != CONTROL_PROTOCOL
        || !valid_id(&request.client_id)
        || request.credential.len() > 128
    {
        return error_value(
            "AUTH_REQUIRED",
            "invalid MCP control request",
            "Use the bundled deck-mcp adapter and an authorized client id.",
        );
    }
    let emergency_auth_blocked = {
        let emergency = runtime.emergency.lock_or_recover();
        emergency.disabled || emergency.clients.contains(&request.client_id)
    };
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
        if emergency.execution_sessions.contains(session_id)
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
        emergency: Mutex::new(EmergencyFences::default()),
        service_instance: random_id("svc_").unwrap_or_else(|_| "svc_unavailable".into()),
        started: Instant::now(),
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
                _ => "unavailable",
            }),
        },
    }
}

#[tauri::command]
pub(crate) fn mcp_enable() -> Result<(), DeckError> {
    let runtime = runtime()?;
    runtime.write(|doc| {
        doc.config.enabled = true;
        Ok(())
    })?;
    runtime.emergency.lock_or_recover().disabled = false;
    Ok(())
}

#[tauri::command]
pub(crate) fn mcp_disable() -> Result<(), DeckError> {
    let runtime = runtime()?;
    let _delivery = runtime.delivery.lock_or_recover();
    runtime.emergency.lock_or_recover().disabled = true;
    let sessions = runtime.write(|doc| {
        doc.config.enabled = false;
        for grant in &mut doc.execution_grants {
            if grant.revoked_at.is_none() {
                grant.revoked_at = Some(now_ms());
                grant.revocation_version = grant.grant_version;
            }
        }
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
            session.control_holder = None;
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
    let mut uncertain = false;
    for session in sessions {
        uncertain |= send_runner_control(runtime, &session, "fenced")
            .ok()
            .is_none_or(|value| value.get("ok").and_then(Value::as_bool) != Some(true));
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

#[tauri::command]
pub(crate) fn mcp_client_revoke(client_id: String) -> Result<(), DeckError> {
    let runtime = runtime()?;
    let _delivery = runtime.delivery.lock_or_recover();
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
            Ok(sessions)
        })?;
    let mut uncertain = false;
    for session in sessions {
        uncertain |= send_runner_control(runtime, &session, "fenced")
            .ok()
            .is_none_or(|value| value.get("ok").and_then(Value::as_bool) != Some(true));
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
#[tauri::command]
pub(crate) fn mcp_execution_grant(
    session_id: String,
    duration_ms: Option<u64>,
    allow_stdin: bool,
    allow_output: bool,
) -> Result<(), DeckError> {
    let runtime = runtime()?;
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
    runtime
        .emergency
        .lock_or_recover()
        .execution_sessions
        .remove(&session.session_id);
    Ok(())
}

#[tauri::command]
pub(crate) fn mcp_execution_revoke(session_id: String) -> Result<(), DeckError> {
    let runtime = runtime()?;
    let _delivery = runtime.delivery.lock_or_recover();
    let fenced_session_id = runtime
        .read(|doc| {
            doc.sessions
                .iter()
                .find(|session| session.session_id == session_id || session.card_id == session_id)
                .map(|session| session.session_id.clone())
        })?
        .ok_or_else(|| DeckError::new(ErrorKind::Missing, "MCP session not found"))?;
    runtime
        .emergency
        .lock_or_recover()
        .execution_sessions
        .insert(fenced_session_id);
    let (session, revoked) =
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
            if let Some(managed) = doc
                .sessions
                .iter_mut()
                .find(|managed| managed.session_id == session.session_id)
            {
                managed.output_shared = false;
            }
            Ok((session, revoked))
        })?;
    let mut uncertain = false;
    for (grant_id, grant_version) in revoked {
        uncertain |= send_runner(
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
    let runtime = runtime()?;
    let _delivery = runtime.delivery.lock_or_recover();
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
    let output_retention_ms = runtime.read(|doc| doc.config.output_retention_ms)?;
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
        "--service-instance".into(),
        runtime.service_instance.clone(),
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
                control_owner: None,
                control_holder: None,
                control_epoch: 1,
                lease_expires_at: None,
                human_lock: !authorized,
                output_shared: false,
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
        let _ = send_runner_control(runtime, &session, "human");
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
    let fenced_session_id = runtime
        .read(|doc| {
            doc.sessions
                .iter()
                .find(|session| session.session_id == session_id || session.card_id == session_id)
                .map(|session| session.session_id.clone())
        })?
        .ok_or_else(|| DeckError::new(ErrorKind::Missing, "MCP session not found"))?;
    runtime
        .emergency
        .lock_or_recover()
        .human_sessions
        .insert(fenced_session_id);
    let session = runtime.write(|doc| {
        let session = doc
            .sessions
            .iter_mut()
            .find(|session| session.session_id == session_id || session.card_id == session_id)
            .ok_or_else(|| DeckError::new(ErrorKind::Missing, "MCP session not found"))?;
        session.control_owner = None;
        session.control_holder = None;
        session.control_epoch = session.control_epoch.saturating_add(1);
        session.lease_expires_at = None;
        session.human_lock = true;
        session.output_shared = false;
        let session = session.clone();
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
    })?;
    let response = send_runner_control(runtime, &session, "human")?;
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
        active_execution_grant(runtime, doc, &session.owner_client_id, &session, false)?;
        Ok::<ManagedSession, DeckError>(session)
    })??;
    let response = send_runner_control(runtime, &session, "mcp")?;
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
        session.control_owner = None;
        session.control_holder = None;
        session.lease_expires_at = None;
        Ok(())
    })?;
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

#[tauri::command]
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
    let runner = send_runner(&session, &json!({"kind":"ping"})).ok();
    let emergency_human = runtime
        .emergency
        .lock_or_recover()
        .human_sessions
        .contains(&session.session_id);
    let job_state = runner
        .as_ref()
        .and_then(|value| value.get("job"))
        .and_then(|job| job.get("state"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok(SessionUiView {
        managed: true,
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
                        "input" | "interrupt" | "control" | "retention" | "revoke-grant" => {
                            json!({"ok":true,"generation":generation})
                        }
                        _ => json!({"ok":false,"generation":generation,"error":"invalid-request"}),
                    };
                    if serde_json::to_writer(&mut stream, &response).is_ok() {
                        let _ = stream.write_all(b"\n");
                    }
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
            credential_hash: sha(b"mcp_test"),
            credential_version: 1,
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
            control_holder: Some("holder_a".into()),
            control_epoch: 1,
            lease_expires_at: Some(now_ms() + 60_000),
            human_lock: false,
            output_shared: true,
            closing: false,
            created_at: now_ms(),
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
        }
    }

    #[test]
    fn fresh_state_is_disabled_and_strictly_bounded() {
        let doc = DiskDoc::default();
        assert!(!doc.config.enabled);
        validate_doc(&doc).unwrap();
        assert_eq!(MAX_SCRIPT_BYTES, 32 * 1024);
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
            "AUTH_REQUIRED"
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
            route(&runtime, request("deck_session_control", value))
        };
        let operation_count = runtime.read(|doc| doc.operations.len()).unwrap();
        for arguments in [
            json!({"request_id":"bad_lease_low","session_id":"mcp_a","expected_generation":"g_a","action":"request","holder_id":"holder_a","lease_ms":999}),
            json!({"request_id":"bad_request_epoch","session_id":"mcp_a","expected_generation":"g_a","action":"request","holder_id":"holder_a","control_epoch":1}),
            json!({"request_id":"bad_renew_epoch","session_id":"mcp_a","expected_generation":"g_a","action":"renew","holder_id":"holder_a"}),
            json!({"request_id":"bad_release_lease","session_id":"mcp_a","expected_generation":"g_a","action":"release","holder_id":"holder_a","control_epoch":1,"lease_ms":1000}),
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
                    json!({"request_id":"holder_conflict","session_id":"mcp_a","expected_generation":"g_a","action":"request","holder_id":"holder_other"}),
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
                json!({"request_id":"exec_without_grant","session_id":"mcp_a","expected_generation":"g_a","control_epoch":4,"holder_id":"holder_a","script":"print forbidden","cwd":root.display().to_string()}),
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
            "script":"printf 'done\\n'",
            "cwd":root.display().to_string(),
            "wait_ms":10,
            "execution_timeout_ms":1_000
        });
        let executed = route(&runtime, request("deck_exec", exec_arguments.clone()));
        assert_eq!(executed["state"], "exited", "{executed}");
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
        let hostile = "\"\\\n\r\t\u{0001}".repeat(MAX_SCRIPT_BYTES / 6);
        let request =
            json!({"kind":"exec","script":hostile,"context":{"intent_hash":"a".repeat(64)}});
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
                    json!({"request_id":"same_create","project_id":"P1","cwd":cwd}),
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
}
