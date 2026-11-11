//! Client, project-scope and execution-grant authorization checks over the loaded document.
//!
//! Split out of the one-file `mcp.rs` on 2026-09-23; the contract stays in
//! `mcp/mod.rs`.

use super::*;

pub(super) fn runtime() -> Result<&'static Arc<Runtime>, DeckError> {
    RUNTIME
        .get()
        .ok_or_else(|| DeckError::new(ErrorKind::Other, "MCP control is not initialized"))
}

pub(super) fn runner_program() -> Result<PathBuf, DeckError> {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join("deck-mcp-runner")))
        .filter(|path| path.is_file())
        .ok_or_else(|| DeckError::new(ErrorKind::Missing, "MCP runner is not bundled"))
}

/// The cwd of a Full Local job (`host-terminal-env-v1`): any existing
/// directory, canonicalized. The authorized project root scopes session
/// creation and structured reads (`canonical_scope`, `mcp_fs`), never where
/// an approved host-authority job may work; checking it here only made a
/// Full Local job weaker than the Terminal. Not a filesystem boundary.
pub(super) fn full_local_cwd(path: &str) -> Result<PathBuf, DeckError> {
    let path = std::fs::canonicalize(path)
        .map_err(|_| DeckError::new(ErrorKind::NotDir, "working directory is unavailable"))?;
    if !path.is_dir() {
        return Err(DeckError::new(
            ErrorKind::NotDir,
            "working directory is unavailable",
        ));
    }
    Ok(path)
}

pub(super) fn canonical_scope(path: &str, roots: &[String]) -> Result<PathBuf, DeckError> {
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

pub(super) fn client<'a>(doc: &'a DiskDoc, client_id: &str) -> Result<&'a Client, DeckError> {
    if !doc.config.enabled {
        return Err(DeckError::new(ErrorKind::Perm, "MCP control is disabled"));
    }
    doc.config
        .clients
        .iter()
        .find(|client| client.id == client_id && client.revoked_at.is_none())
        .ok_or_else(|| DeckError::new(ErrorKind::Perm, "MCP client is not authorized"))
}

/// One of a client's separate choices, each refused with its own message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Choice {
    Create,
    Automations,
    Workers,
    Drafts,
    Mentions,
}

impl Choice {
    fn granted(self, client: &Client) -> bool {
        match self {
            Choice::Create => client.allow_create,
            Choice::Automations => client.allow_automations,
            Choice::Workers => client.allow_workers,
            Choice::Drafts => client.allow_drafts,
            Choice::Mentions => client.allow_mentions,
        }
    }

    fn refusal(self) -> &'static str {
        match self {
            Choice::Create => "client may not create sessions",
            Choice::Automations => "client may not trigger automations",
            Choice::Workers => "client may not drive worker cards",
            Choice::Drafts => "client may not draft configuration",
            Choice::Mentions => "client may not read Slack mentions",
        }
    }
}

/// The authorized client, refused unless it holds `choice`.
pub(super) fn permitted<'a>(
    doc: &'a DiskDoc,
    client_id: &str,
    choice: Choice,
) -> Result<&'a Client, DeckError> {
    let client = client(doc, client_id)?;
    if !choice.granted(client) {
        return Err(DeckError::new(ErrorKind::Perm, choice.refusal()));
    }
    Ok(client)
}

/// The message a project in reach but without a default directory is
/// refused with (`all_projects`): Deck never guesses a directory.
pub(super) const NO_PROJECT_DIRECTORY: &str =
    "project has no default directory; ask the local Deck user to set one";

/// The one project scope a client reaches `project_id` through: its own
/// list, or, for a client of all projects, that project's default
/// directory on the current Board (read now), canonicalized with the
/// checks an authorization root passes (too-broad roots refused) — a
/// directory's canonical form reused for `ROOT_REUSE`, since this runs
/// inside the ledger lock and once per directory of a search.
pub(super) fn scoped_project(client: &Client, project_id: &str) -> Result<ProjectScope, DeckError> {
    if client.all_projects {
        return board_scope(project_id);
    }
    client
        .projects
        .iter()
        .find(|project| project.project_id == project_id)
        .cloned()
        .ok_or_else(|| DeckError::new(ErrorKind::Perm, "project is not authorized"))
}

/// Every project scope a client reaches now; for a client of all projects,
/// the current Board's projects whose default directory passes (the ones
/// without one are `projects_without_directory`).
pub(super) fn client_scopes(client: &Client) -> Vec<ProjectScope> {
    if !client.all_projects {
        return client.projects.clone();
    }
    board_projects()
        .into_iter()
        .filter_map(|(id, _)| board_scope(&id).ok())
        .collect()
}

/// Whether the ledger holds a client of all projects (a v11 reader would
/// see it as a client of none).
pub(super) fn uses_all_projects(doc: &DiskDoc) -> bool {
    doc.config.clients.iter().any(|c| c.all_projects)
}

/// The ids of the projects a client reaches: its own list, or every
/// project of the current Board.
pub(super) fn reach(client: &Client) -> HashSet<String> {
    if client.all_projects {
        return board_projects().into_iter().map(|(id, _)| id).collect();
    }
    client
        .projects
        .iter()
        .map(|p| p.project_id.clone())
        .collect()
}

/// The projects the ledger itself says a client holds something in: its
/// managed sessions, its execution grants and its pending operations. A
/// scope change reads these besides the reach, so what leaves the scope is
/// withdrawn even while no current Board names the projects (a client of
/// all projects reaches none then) or after a project left the Board.
pub(super) fn held_projects(doc: &DiskDoc, client_id: &str) -> HashSet<String> {
    let sessions = doc
        .sessions
        .iter()
        .filter(|s| s.owner_client_id == client_id)
        .map(|s| s.project_id.clone());
    let grants = doc
        .execution_grants
        .iter()
        .filter(|g| g.client_id == client_id && g.revoked_at.is_none())
        .map(|g| g.project_id.clone());
    let pending = doc
        .operations
        .iter()
        .filter(|o| {
            o.client_id == client_id && matches!(o.state.as_str(), "accepted" | "executing")
        })
        .filter_map(|o| {
            o.result
                .as_ref()?
                .get("projectId")?
                .as_str()
                .map(str::to_owned)
        });
    sessions.chain(grants).chain(pending).collect()
}

/// A project left a client's reach (`client_permissions`): everything it
/// had there is withdrawn, as revocation withdraws everything: its
/// execution grants there end, its managed sessions there are fenced to
/// the user (returned, for the runner hand-over after the write) and lose
/// their output, its accepted starts there are rejected, and its worker
/// cards there are the user's.
pub(super) fn withdraw_projects(
    doc: &mut DiskDoc,
    client_id: &str,
    removed: &HashSet<String>,
) -> Vec<ManagedSession> {
    if removed.is_empty() {
        return Vec::new();
    }
    for grant in doc.execution_grants.iter_mut().filter(|grant| {
        grant.client_id == client_id
            && grant.revoked_at.is_none()
            && removed.contains(&grant.project_id)
    }) {
        grant.revoked_at = Some(now_ms());
        grant.revocation_version = grant.grant_version;
    }
    let mut fenced = Vec::new();
    for session in doc
        .sessions
        .iter_mut()
        .filter(|s| s.owner_client_id == client_id && removed.contains(&s.project_id))
    {
        session.fence(FenceMode::Human);
        fenced.push(session.clone());
    }
    let ids: HashSet<String> = fenced.iter().map(|s| s.session_id.clone()).collect();
    doc.close_output(|job| job.client_id == client_id && ids.contains(&job.session_id));
    doc.reject_pending(
        |operation| {
            operation.client_id == client_id
                && matches!(
                    operation.kind.as_str(),
                    "session-create" | super::workers::WORKER_START_KIND
                )
                && operation
                    .result
                    .as_ref()
                    .and_then(|r| r.get("projectId"))
                    .and_then(Value::as_str)
                    .is_some_and(|p| removed.contains(p))
        },
        "scope-removed",
    );
    if let Some(cards) = crate::documents::card_targets(false) {
        for worker in doc.workers.iter_mut().filter(|w| {
            w.client_id == client_id
                && cards
                    .get(&w.card_id)
                    .is_some_and(|t| removed.contains(&t.project_id))
        }) {
            worker.held = true;
        }
    }
    fenced
}

/// For a client of all projects: the current Board's projects it can see
/// but not work in, for want of a default directory.
pub(super) fn projects_without_directory(client: &Client) -> Vec<String> {
    if !client.all_projects {
        return Vec::new();
    }
    board_projects()
        .into_iter()
        .filter(|(_, dir)| dir.trim().is_empty())
        .map(|(id, _)| id)
        .collect()
}

/// The current Board's projects and their default directories; empty while
/// no current Board is known (a recovered Board grants nothing).
fn board_projects() -> Vec<(String, String)> {
    #[cfg(test)]
    if let Some(forced) = BOARD_PROJECTS_FOR_TESTS.with(|cell| cell.borrow().clone()) {
        return forced;
    }
    crate::documents::project_dirs().unwrap_or_default()
}

/// How long one directory's canonical form is reused: a scope check runs
/// inside the ledger lock and, in a search, once per directory walked, so
/// the disk is asked at most this often per directory (keyed by the
/// directory as the Board names it: a changed default directory is a new
/// key, read at once).
const ROOT_REUSE: std::time::Duration = std::time::Duration::from_secs(2);
const ROOT_CACHE_MAX: usize = 256;

type RootCache = HashMap<String, (Option<String>, Instant)>;

fn root_cache() -> &'static Mutex<RootCache> {
    static ROOTS: OnceLock<Mutex<RootCache>> = OnceLock::new();
    ROOTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The canonical authorization root of a project's default directory, or
/// `None` when it cannot be one (missing, unreadable, too broad).
fn canonical_default_dir(dir: &str) -> Option<String> {
    if let Some((root, at)) = root_cache().lock_or_recover().get(dir) {
        if at.elapsed() < ROOT_REUSE {
            return root.clone();
        }
    }
    let root = super::commands::canonical_project_root(&crate::tmux::expand_tilde(dir)).ok();
    let mut cache = root_cache().lock_or_recover();
    if cache.len() >= ROOT_CACHE_MAX {
        cache.clear();
    }
    cache.insert(dir.to_owned(), (root.clone(), Instant::now()));
    root
}

#[cfg(test)]
thread_local! {
    /// A test's Board projects (id, default directory) in place of the Board.
    pub(super) static BOARD_PROJECTS_FOR_TESTS: std::cell::RefCell<Option<Vec<(String, String)>>> =
        const { std::cell::RefCell::new(None) };
}

fn board_scope(project_id: &str) -> Result<ProjectScope, DeckError> {
    let (_, dir) = board_projects()
        .into_iter()
        .find(|(id, _)| id == project_id)
        .ok_or_else(|| DeckError::new(ErrorKind::Perm, "project is not authorized"))?;
    if dir.trim().is_empty() {
        return Err(DeckError::new(ErrorKind::Perm, NO_PROJECT_DIRECTORY));
    }
    let root = canonical_default_dir(dir.trim()).ok_or_else(|| {
        DeckError::new(
            ErrorKind::Perm,
            "project default directory cannot be authorized (missing, unreadable or too broad)",
        )
    })?;
    Ok(ProjectScope {
        project_id: project_id.to_owned(),
        roots: vec![root],
    })
}

pub(super) fn authorized_session<'a>(
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

pub(super) fn check_control(
    session: &ManagedSession,
    client_id: &str,
    generation: &str,
    epoch: u64,
    holder_id: &str,
) -> Result<(), DeckError> {
    let claim = ControlClaim {
        generation,
        epoch: Some(epoch),
        holder_id: Some(holder_id),
    };
    control_reason(session, client_id, &claim).map_or(Ok(()), |reason| Err(reason.error()))
}

/// What a caller claims to hold. `exec`, stdin/interrupt, renew and release
/// claim an exact generation, epoch and holder; `inspect` claims the
/// session's own generation, no epoch, and the holder it was given (if any).
pub(super) struct ControlClaim<'a> {
    pub(super) generation: &'a str,
    pub(super) epoch: Option<u64>,
    pub(super) holder_id: Option<&'a str>,
}

/// Why `exec` would refuse to start a job, in the order it checks: the
/// session generation, then control (human lock, owner, holder, epoch,
/// lease — `exec` reports all five as one `ControlRevoked`), then the
/// execution grant, then a close in progress. `inspect`'s
/// `mayStartNextJobReason` reports the same first failure, so the advisory
/// cannot drift from the real admission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AdmissionReason {
    GenerationChanged,
    HumanControl,
    ControlRevoked,
    HolderMismatch,
    EpochMismatch,
    LeaseExpired,
    GrantRequired,
    Closing,
}

impl AdmissionReason {
    /// The error `exec` (and `check_control`) returns — unchanged from
    /// before this was one function; `map_error` turns it into the client's
    /// machine code.
    pub(super) fn error(self) -> DeckError {
        match self {
            AdmissionReason::GenerationChanged => {
                DeckError::new(ErrorKind::ContextChanged, "session generation changed")
            }
            AdmissionReason::HumanControl
            | AdmissionReason::ControlRevoked
            | AdmissionReason::HolderMismatch
            | AdmissionReason::EpochMismatch
            | AdmissionReason::LeaseExpired => {
                DeckError::new(ErrorKind::ControlRevoked, "session control was revoked")
            }
            AdmissionReason::GrantRequired => {
                DeckError::new(ErrorKind::Perm, "a local execution grant is required")
            }
            AdmissionReason::Closing => DeckError::new(ErrorKind::Locked, "session is closing"),
        }
    }

    /// `inspect`'s finer `mayStartNextJobReason` word.
    pub(super) fn inspect_code(self) -> &'static str {
        match self {
            AdmissionReason::GenerationChanged => "CONTEXT_CHANGED",
            AdmissionReason::HumanControl => "HUMAN_CONTROL",
            AdmissionReason::ControlRevoked | AdmissionReason::EpochMismatch => "CONTROL_REVOKED",
            AdmissionReason::HolderMismatch => "HOLDER_MISMATCH",
            AdmissionReason::LeaseExpired => "CONTROL_LEASE_EXPIRED",
            AdmissionReason::GrantRequired => "EXECUTION_GRANT_REQUIRED",
            AdmissionReason::Closing => "TARGET_CLOSING",
        }
    }
}

fn control_reason(
    session: &ManagedSession,
    client_id: &str,
    claim: &ControlClaim<'_>,
) -> Option<AdmissionReason> {
    if session.generation != claim.generation {
        Some(AdmissionReason::GenerationChanged)
    } else if session.human_lock {
        Some(AdmissionReason::HumanControl)
    } else if session.control_owner.as_deref() != Some(client_id) {
        Some(AdmissionReason::ControlRevoked)
    } else if session.control_holder.as_deref() != claim.holder_id {
        Some(AdmissionReason::HolderMismatch)
    } else if claim
        .epoch
        .is_some_and(|epoch| epoch != session.control_epoch)
    {
        Some(AdmissionReason::EpochMismatch)
    } else if session
        .lease_expires_at
        .is_none_or(|lease| lease <= now_ms())
    {
        Some(AdmissionReason::LeaseExpired)
    } else {
        None
    }
}

/// Where in `exec` the admission runs. `Final` (the last step before the
/// runner, under the delivery lock) re-checks control and the grant but not
/// `closing` — unchanged from before this extraction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ExecStage {
    Accept,
    Final,
}

/// The ONE exec admission: `exec` accept, `exec` final admission and
/// `inspect`'s advisory all ask this. Emergency fences stay with
/// `emergency_denial` (checked by the callers around it, unchanged).
pub(super) fn admit_exec<'a>(
    runtime: &Runtime,
    doc: &'a DiskDoc,
    client_id: &str,
    session: &ManagedSession,
    claim: &ControlClaim<'_>,
    stage: ExecStage,
) -> Result<&'a ExecutionGrant, AdmissionReason> {
    if let Some(reason) = control_reason(session, client_id, claim) {
        return Err(reason);
    }
    let grant = match execution_authorization(runtime, doc, client_id, session) {
        ExecutionAuthorization::Active(grant) => grant,
        _ => return Err(AdmissionReason::GrantRequired),
    };
    if stage == ExecStage::Accept && session.closing {
        return Err(AdmissionReason::Closing);
    }
    Ok(grant)
}

#[derive(Clone, Copy)]
pub(super) enum ExecutionAuthorization<'a> {
    None,
    Active(&'a ExecutionGrant),
    Expired(&'a ExecutionGrant),
    Revoked(&'a ExecutionGrant),
}

pub(super) fn execution_authorization<'a>(
    runtime: &Runtime,
    doc: &'a DiskDoc,
    client_id: &str,
    session: &ManagedSession,
) -> ExecutionAuthorization<'a> {
    let Some(grant) = doc.execution_grants.iter().rev().find(|grant| {
        grant.client_id == client_id
            && grant.project_id == session.project_id
            && grant.session_id == session.session_id
            && grant.session_generation == session.generation
    }) else {
        return ExecutionAuthorization::None;
    };
    match grant_standing(runtime, doc, grant) {
        GrantStanding::Revoked => ExecutionAuthorization::Revoked(grant),
        GrantStanding::Expired => ExecutionAuthorization::Expired(grant),
        GrantStanding::Active => ExecutionAuthorization::Active(grant),
    }
}

/// Whether one execution grant is usable right now, however the caller
/// selected it. The ONE derivation: `execution_authorization`, the stdin
/// check in `job_side_effect`, the Session UI and `record_expired_grants` all
/// ask this and add only their own extra condition (e.g. `allow_stdin`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GrantStanding {
    Active,
    Expired,
    Revoked,
}

pub(super) fn grant_standing(
    runtime: &Runtime,
    doc: &DiskDoc,
    grant: &ExecutionGrant,
) -> GrantStanding {
    let credential = doc
        .config
        .clients
        .iter()
        .find(|client| client.id == grant.client_id)
        .map(|client| client.credential_version);
    standing_at(
        grant,
        credential,
        &runtime.service_instance,
        runtime.monotonic_ms(),
        now_ms(),
    )
}

/// Pure core of `grant_standing`: revocation (a revoked_at stamp, a
/// revocation version at or above the grant's, a client credential that
/// moved on or vanished, another Deck service instance, or an environment
/// profile this build no longer issues) outranks expiry (the monotonic
/// window elapsed, the clock went backwards, or the wall deadline passed).
/// A legacy-profile grant is kept as history and never authorizes again:
/// its jobs ran under that environment, and a new approval issues a grant
/// under the current one.
pub(super) fn standing_at(
    grant: &ExecutionGrant,
    client_credential_version: Option<u64>,
    service_instance: &str,
    elapsed_ms: u64,
    now: u64,
) -> GrantStanding {
    if grant.revoked_at.is_some()
        || grant.grant_version <= grant.revocation_version
        || client_credential_version != Some(grant.credential_version)
        || grant.service_instance != service_instance
        || grant.environment_profile != ENVIRONMENT_PROFILE
    {
        return GrantStanding::Revoked;
    }
    if elapsed_ms < grant.issued_monotonic_ms
        || elapsed_ms.saturating_sub(grant.issued_monotonic_ms) >= grant.duration_ms
        || now >= grant.expires_at
    {
        return GrantStanding::Expired;
    }
    GrantStanding::Active
}

// Production admission is `admit_exec`; this older form stays for the
// linearization tests in `tests.rs`, which must not change.
#[cfg(test)]
pub(super) fn active_execution_grant<'a>(
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

pub(super) fn record_expired_grants(runtime: &Runtime) -> Result<(), DeckError> {
    let expired = runtime.read(|doc| {
        doc.execution_grants
            .iter()
            .filter(|grant| {
                grant_standing(runtime, doc, grant) == GrantStanding::Expired
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

#[cfg(test)]
mod tests {
    use super::*;

    fn grant() -> ExecutionGrant {
        ExecutionGrant {
            grant_id: "grant_a".into(),
            client_id: "client_a".into(),
            credential_version: 1,
            project_id: "P1".into(),
            session_id: "mcp_a".into(),
            session_generation: "g_a".into(),
            profile: "trusted-host-v1".into(),
            environment_profile: ENVIRONMENT_PROFILE.into(),
            environment_profile_version: 1,
            issued_at: 1_000,
            expires_at: 61_000,
            issued_monotonic_ms: 100,
            duration_ms: 60_000,
            allow_stdin: true,
            allow_output: true,
            _legacy_allow_shell: None,
            grant_version: 2,
            revocation_version: 1,
            service_instance: "svc".into(),
            revoked_at: None,
        }
    }

    /// The four axes of the one derivation: revocation stamp/version,
    /// client credential, Deck service instance, and time (monotonic window,
    /// clock going backwards, wall deadline). Revocation outranks expiry.
    #[test]
    fn standing_covers_every_axis_and_revocation_outranks_expiry() {
        let at =
            |grant: &ExecutionGrant,
             credential: Option<u64>,
             service: &str,
             elapsed: u64,
             now: u64| { standing_at(grant, credential, service, elapsed, now) };
        let live = grant();
        assert_eq!(at(&live, Some(1), "svc", 200, 2_000), GrantStanding::Active);

        // revocation axis
        let mut stamped = grant();
        stamped.revoked_at = Some(1_500);
        assert_eq!(
            at(&stamped, Some(1), "svc", 200, 2_000),
            GrantStanding::Revoked
        );
        let mut versioned = grant();
        versioned.revocation_version = 2;
        assert_eq!(
            at(&versioned, Some(1), "svc", 200, 2_000),
            GrantStanding::Revoked
        );
        // credential axis
        assert_eq!(
            at(&live, Some(2), "svc", 200, 2_000),
            GrantStanding::Revoked
        );
        assert_eq!(
            at(&live, None, "svc", 200, 2_000),
            GrantStanding::Revoked,
            "client gone"
        );
        // service-instance axis
        assert_eq!(
            at(&live, Some(1), "svc_old", 200, 2_000),
            GrantStanding::Revoked
        );
        // environment-profile axis: a legacy grant never authorizes, even
        // in the same service instance and window
        let mut legacy = grant();
        legacy.environment_profile = "developer-sanitized-v1".into();
        assert_eq!(
            at(&legacy, Some(1), "svc", 200, 2_000),
            GrantStanding::Revoked
        );
        // time axis
        assert_eq!(
            at(&live, Some(1), "svc", 60_100, 2_000),
            GrantStanding::Expired,
            "window elapsed"
        );
        assert_eq!(
            at(&live, Some(1), "svc", 99, 2_000),
            GrantStanding::Expired,
            "clock went backwards"
        );
        assert_eq!(
            at(&live, Some(1), "svc", 200, 61_000),
            GrantStanding::Expired,
            "wall deadline"
        );
        assert_eq!(
            at(&live, Some(1), "svc", 60_099, 60_999),
            GrantStanding::Active,
            "last usable instant"
        );
        // revocation outranks expiry on every revocation axis
        assert_eq!(
            at(&stamped, Some(1), "svc", 60_100, 61_000),
            GrantStanding::Revoked
        );
        assert_eq!(
            at(&live, Some(2), "svc", 60_100, 61_000),
            GrantStanding::Revoked
        );
        assert_eq!(
            at(&live, Some(1), "svc_old", 60_100, 61_000),
            GrantStanding::Revoked
        );
    }
}
