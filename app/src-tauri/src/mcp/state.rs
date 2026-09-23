//! Durable MCP state: the disk document, its records, the in-process runtime, load/validate/compact/save.
//!
//! Split out of the one-file `mcp.rs` on 2026-09-23; the contract stays in
//! `mcp/mod.rs`.

use super::*;

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Config {
    pub(super) enabled: bool,
    pub(super) clients: Vec<Client>,
    #[serde(default = "default_output_retention_ms")]
    pub(super) output_retention_ms: u64,
}

pub(super) fn default_output_retention_ms() -> u64 {
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
pub(super) struct Client {
    pub(super) id: String,
    pub(super) name: String,
    #[serde(default)]
    pub(super) credential_hash: String,
    #[serde(default)]
    pub(super) credential_version: u64,
    pub(super) revoked_at: Option<u64>,
    pub(super) allow_create: bool,
    pub(super) projects: Vec<ProjectScope>,
    /// Creates this client has had accepted. A create must name the current
    /// value, so a create whose record was retired can never be accepted again.
    #[serde(default)]
    pub(super) create_sequence: u64,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ProjectScope {
    pub(super) project_id: String,
    pub(super) roots: Vec<String>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ManagedSession {
    pub(super) session_id: String,
    pub(super) card_id: String,
    pub(super) tmux_session: String,
    pub(super) project_id: String,
    pub(super) title: String,
    pub(super) cwd: String,
    pub(super) generation: String,
    pub(super) runner_socket: String,
    pub(super) owner_client_id: String,
    pub(super) control_owner: Option<String>,
    #[serde(default)]
    pub(super) control_holder: Option<String>,
    pub(super) control_epoch: u64,
    pub(super) lease_expires_at: Option<u64>,
    pub(super) human_lock: bool,
    #[serde(default)]
    pub(super) output_shared: bool,
    #[serde(default)]
    pub(super) closing: bool,
    pub(super) created_at: u64,
    /// Accepted MCP control changes (request, renew, release). A control
    /// request must name the current value; see `session_control`.
    #[serde(default)]
    pub(super) control_sequence: u64,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Operation {
    pub(super) operation_id: String,
    pub(super) client_id: String,
    pub(super) request_id: String,
    pub(super) request_hash: String,
    pub(super) kind: String,
    pub(super) state: String,
    pub(super) code: Option<String>,
    pub(super) result: Option<Value>,
    pub(super) accepted_at: u64,
    pub(super) updated_at: u64,
    #[serde(default)]
    pub(super) admission_hash: Option<String>,
    /// Session this request targeted (the new session for a create).
    #[serde(default)]
    pub(super) session_id: Option<String>,
    /// Control epoch the request was bound to. A terminal record whose epoch
    /// is older than the session's current epoch can be retired: any replay
    /// of it fails `check_control` deterministically.
    #[serde(default)]
    pub(super) control_epoch: Option<u64>,
    /// For a control record: the session's control sequence this change
    /// produced. A record whose sequence is no longer current is superseded.
    #[serde(default)]
    pub(super) control_sequence: Option<u64>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct JobBinding {
    pub(super) job_id: String,
    pub(super) client_id: String,
    pub(super) session_id: String,
    pub(super) session_generation: String,
    pub(super) request_hash: String,
    pub(super) operation_id: String,
    #[serde(default)]
    pub(super) grant_id: String,
    #[serde(default)]
    pub(super) grant_version: u64,
    #[serde(default)]
    pub(super) allow_output: bool,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ExecutionGrant {
    pub(super) grant_id: String,
    pub(super) client_id: String,
    pub(super) credential_version: u64,
    pub(super) project_id: String,
    pub(super) session_id: String,
    pub(super) session_generation: String,
    pub(super) profile: String,
    pub(super) environment_profile: String,
    pub(super) environment_profile_version: u32,
    pub(super) issued_at: u64,
    pub(super) expires_at: u64,
    pub(super) issued_monotonic_ms: u64,
    pub(super) duration_ms: u64,
    pub(super) allow_stdin: bool,
    pub(super) allow_output: bool,
    /// Accepted only so v6 files written before shell fallback was removed
    /// continue to load. It is ignored and omitted on the next save.
    #[serde(default, rename = "allowShell", skip_serializing)]
    pub(super) _legacy_allow_shell: Option<bool>,
    pub(super) grant_version: u64,
    pub(super) revocation_version: u64,
    pub(super) service_instance: String,
    pub(super) revoked_at: Option<u64>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct AuditEvent {
    pub(super) event_id: String,
    pub(super) at: u64,
    pub(super) kind: String,
    pub(super) principal_id: Option<String>,
    pub(super) session_id: Option<String>,
    pub(super) operation_id: Option<String>,
    pub(super) job_id: Option<String>,
    pub(super) grant_id: Option<String>,
    pub(super) reason_code: Option<String>,
    pub(super) policy_version: u32,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct DiskDoc {
    pub(super) version: u32,
    pub(super) config: Config,
    pub(super) sessions: Vec<ManagedSession>,
    pub(super) operations: Vec<Operation>,
    pub(super) jobs: Vec<JobBinding>,
    #[serde(default)]
    pub(super) execution_grants: Vec<ExecutionGrant>,
    #[serde(default)]
    pub(super) audit: Vec<AuditEvent>,
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
pub(super) struct AuditLink<'a> {
    pub(super) principal_id: Option<&'a str>,
    pub(super) session_id: Option<&'a str>,
    pub(super) operation_id: Option<&'a str>,
    pub(super) job_id: Option<&'a str>,
    pub(super) grant_id: Option<&'a str>,
    pub(super) reason_code: Option<&'a str>,
}

pub(super) fn audit(doc: &mut DiskDoc, kind: &str, link: AuditLink<'_>) -> Result<(), DeckError> {
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

pub(super) struct Runtime {
    pub(super) app: Option<AppHandle>,
    pub(super) path: PathBuf,
    pub(super) socket: PathBuf,
    pub(super) doc: Mutex<Result<DiskDoc, DeckError>>,
    pub(super) io: Mutex<()>,
    /// Fences every terminal/Board side-effect dispatch against control
    /// transfer, revoke, disable, and close admission.
    pub(super) delivery: Mutex<()>,
    pub(super) emergency: Mutex<EmergencyFences>,
    pub(super) service_instance: String,
    /// Per-runner authentication keys exist only in this Deck process, each
    /// kept with the pane PID the runner was verified to run as.
    pub(super) runner_auth: Mutex<HashMap<String, RunnerAuth>>,
    pub(super) started: Instant,
}

#[derive(Default)]
pub(super) struct EmergencyFences {
    pub(super) disabled: bool,
    pub(super) clients: HashSet<String>,
    pub(super) human_sessions: HashSet<String>,
    /// Execution revocations that have set their fence and not yet finished
    /// their locked section, counted per session (two may overlap). Only the
    /// revocation itself lowers its count, so an approval that ran while it
    /// was pending cannot lift it.
    pub(super) execution_pending: HashMap<String, usize>,
    /// Execution revocations whose persistence failed. Only a later local
    /// approval — ordered after it by the delivery lock — lifts this.
    pub(super) execution_unpersisted: HashSet<String>,
}

impl EmergencyFences {
    pub(super) fn execution_fenced(&self, session_id: &str) -> bool {
        self.execution_pending
            .get(session_id)
            .is_some_and(|count| *count > 0)
            || self.execution_unpersisted.contains(session_id)
    }
}

/// What a side effect needs besides client, feature and human fences.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Admission {
    /// Control, read, interrupt and close: an execution window is not part of
    /// their authority, so its revocation does not refuse them.
    Control,
    /// exec and stdin: a pending execution revocation also refuses them.
    Execution,
}

impl Runtime {
    pub(super) fn monotonic_ms(&self) -> u64 {
        self.started.elapsed().as_millis().min(u64::MAX as u128) as u64
    }
}

#[cfg(test)]
pub(super) fn pause_point(runtime: &Runtime, point: &'static str) {
    pause::reach(&runtime.path, point);
}

#[cfg(not(test))]
pub(super) fn pause_point(_runtime: &Runtime, _point: &'static str) {}

pub(super) fn emit_changed(runtime: &Runtime) {
    if let Some(app) = &runtime.app {
        let _ = app.emit("mcp-changed", ());
    }
}

pub(super) static RUNTIME: OnceLock<Arc<Runtime>> = OnceLock::new();

pub(super) fn load(path: &Path) -> Result<DiskDoc, DeckError> {
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
pub(super) fn compact(doc: &mut DiskDoc, current_service: Option<&str>) -> bool {
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

pub(super) fn validate_doc(doc: &DiskDoc) -> Result<(), DeckError> {
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

pub(super) fn save(path: &Path, doc: &DiskDoc) -> Result<(), DeckError> {
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
    pub(super) fn read<T>(&self, read: impl FnOnce(&DiskDoc) -> T) -> Result<T, DeckError> {
        let doc = self.doc.lock_or_recover();
        match &*doc {
            Ok(doc) => Ok(read(doc)),
            Err(error) => Err(error.clone()),
        }
    }

    pub(super) fn write<T>(
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

/// Deterministic interleaving for tests: a test arms a named point of one
/// runtime (keyed by its state path), the production path parks there until
/// released. Compiled out of every non-test build.
#[cfg(test)]
pub(super) mod pause {
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
    pub(in crate::mcp) fn arm(path: &Path, point: &'static str) -> (Receiver<()>, Sender<()>) {
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
