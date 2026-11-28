//! Client, project-scope and execution-grant authorization checks over the loaded document:
//! a client's separate choices (`permitted`), its project scope
//! (`scoped_project`, the all-projects resolution, `withdraw_projects`) and
//! its execution grants.
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
    Cards,
    CardEdits,
}

impl Choice {
    fn granted(self, client: &Client) -> bool {
        match self {
            Choice::Create => client.allow_create,
            Choice::Automations => client.allow_automations,
            Choice::Workers => client.allow_workers,
            Choice::Drafts => client.allow_drafts,
            Choice::Mentions => client.allow_mentions,
            Choice::Cards => client.allow_cards,
            Choice::CardEdits => client.allow_cards && client.allow_card_edits,
        }
    }

    fn refusal(self) -> &'static str {
        match self {
            Choice::Create => "client may not create sessions",
            Choice::Automations => "client may not trigger automations",
            Choice::Workers => "client may not drive worker cards",
            Choice::Drafts => "client may not draft configuration",
            Choice::Mentions => "client may not read Slack mentions",
            Choice::Cards => "client may not read the card snapshot",
            Choice::CardEdits => "client may not rename or move cards",
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
                    "session-create"
                        | super::workers::WORKER_START_KIND
                        | super::workers::WORKER_CLOSE_KIND
                        | super::cards::CARD_UPDATE_KIND
                )
                && ["projectId", "fromProjectId"].iter().any(|key| {
                    operation
                        .result
                        .as_ref()
                        .and_then(|r| r.get(*key))
                        .and_then(Value::as_str)
                        .is_some_and(|p| removed.contains(p))
                })
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

/* ---------- linked worktrees of authorized projects (approved 2026-11-28) ---------- */

/// The most bytes Deck reads of one of Git's small pointer files
/// (`.git`, `gitdir`, `commondir`): each holds one path.
const GIT_POINTER_BYTES: u64 = 4096;
/// The most directories `worktree_top` climbs looking for a `.git`.
const MAX_WORKTREE_DEPTH: usize = 64;

/// Whether the ledger holds a client with the linked-worktree switch on (a
/// v22 reader would refuse the member).
pub(super) fn uses_auto_worktrees(doc: &DiskDoc) -> bool {
    doc.config.clients.iter().any(|c| c.auto_worktrees)
}

/// Why a directory is not a linked worktree of a repository the client's
/// projects anchor, or why a project anchors no repository: closed words.
pub(super) const WORKTREE_REASONS: [&str; 8] = [
    // no `.git` above the directory: not inside any Git working tree
    "not-a-worktree",
    // the nearest `.git` is a directory: a repository's own checkout (a
    // clone, or a main checkout no project root names), not linked
    "separate-repository",
    // a linked worktree, but of a repository no reached project anchors
    "other-repository",
    // its `.git` names worktree metadata that does not name it back
    "not-registered",
    // a pointer file missing, too long, not a regular file, a link, or not
    // the shape Git writes
    "unreadable-metadata",
    // a project root that is not a working tree's top (a subdirectory of a
    // repository, or no repository): it anchors nothing
    "not-a-repository-root",
    // a project root whose `.git` names a submodule's or another layout's
    // metadata, or a bare repository: not supported as an anchor
    "unsupported-layout",
    // the worktree's top is too broad or sensitive (`mcp_fs::root_policy`)
    "path-policy",
];

/// One of Git's pointer files: `gitdir: <path>` (a worktree's `.git`) or a
/// bare path (`gitdir`, `commondir`), resolved against `base` when
/// relative. Opened with `O_NOFOLLOW` and checked on the descriptor it
/// opened (a regular file of at most `GIT_POINTER_BYTES`), so a link or a
/// file swapped in after a check is never what is read. Bounded, no spawn.
fn read_pointer(file: &Path, prefix: Option<&str>, base: &Path) -> Result<PathBuf, &'static str> {
    use std::io::Read as _;
    use std::os::fd::FromRawFd;
    use std::os::unix::ffi::OsStrExt;
    let unreadable = "unreadable-metadata";
    let path = std::ffi::CString::new(file.as_os_str().as_bytes()).map_err(|_| unreadable)?;
    // SAFETY: a NUL-terminated path; the descriptor is owned below.
    let fd = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
        )
    };
    if fd < 0 {
        return Err(unreadable);
    }
    // SAFETY: open returned a new descriptor this File now owns.
    let mut opened = unsafe { std::fs::File::from_raw_fd(fd) };
    let meta = opened.metadata().map_err(|_| unreadable)?;
    if !meta.is_file() || meta.len() > GIT_POINTER_BYTES {
        return Err(unreadable);
    }
    let mut text = String::new();
    (&mut opened)
        .take(GIT_POINTER_BYTES)
        .read_to_string(&mut text)
        .map_err(|_| unreadable)?;
    let line = text.lines().next().unwrap_or_default().trim_end();
    let named = match prefix {
        Some(prefix) => line.strip_prefix(prefix).ok_or(unreadable)?.trim_start(),
        None => line,
    };
    if named.is_empty() || named.contains('\0') {
        return Err(unreadable);
    }
    let named = Path::new(named);
    let named = if named.is_absolute() {
        named.to_path_buf()
    } else {
        base.join(named)
    };
    std::fs::canonicalize(named).map_err(|_| unreadable)
}

/// A linked worktree whose top is `top`: its `.git` FILE names
/// `<common>/worktrees/<id>`, that directory's `commondir` resolves to
/// `<common>`, and its `gitdir` names this `.git` back (git-worktree(1),
/// gitrepository-layout(5); absolute or relative paths). Returns `<common>`,
/// the repository's own Git directory. Equal remotes, branches, HEADs or
/// names prove nothing and are not read. This is a consistency check of
/// what the repository registered, not a signature: a local program able
/// to rewrite both sides can make a consistent pair.
fn linked_common(top: &Path) -> Result<PathBuf, &'static str> {
    let dot_git = top.join(".git");
    let admin = read_pointer(&dot_git, Some("gitdir:"), top)?;
    let registry = admin.parent().ok_or("unreadable-metadata")?;
    if registry.file_name() != Some(std::ffi::OsStr::new("worktrees")) {
        return Err("other-repository");
    }
    let common = registry
        .parent()
        .ok_or("unreadable-metadata")?
        .to_path_buf();
    if read_pointer(&admin.join("commondir"), None, &admin)? != common {
        return Err("other-repository");
    }
    let back = read_pointer(&admin.join("gitdir"), None, &admin)?;
    let here = std::fs::canonicalize(&dot_git).map_err(|_| "unreadable-metadata")?;
    if back != here {
        return Err("not-registered");
    }
    Ok(common)
}

/// The repository a project root anchors, from the root itself: a main
/// checkout's top (its `.git` is a directory: that directory) or a linked
/// worktree's top (verified both ways: its repository's Git directory). A
/// root that is a subdirectory of a working tree anchors nothing — only
/// what the user authorized is the anchor, never a `.git` found above it —
/// and a submodule's, a bare or another layout is not supported.
pub(super) fn repository_anchor(root: &Path) -> Result<PathBuf, &'static str> {
    match std::fs::symlink_metadata(root.join(".git")) {
        Ok(meta) if meta.is_dir() => {
            std::fs::canonicalize(root.join(".git")).map_err(|_| "unreadable-metadata")
        }
        Ok(meta) if meta.is_file() => linked_common(root).map_err(|why| match why {
            "other-repository" => "unsupported-layout",
            other => other,
        }),
        Ok(_) => Err("unreadable-metadata"),
        Err(_) => Err("not-a-repository-root"),
    }
}

/// The top of the working tree that holds `path` (the nearest directory
/// with a `.git`), and whether that `.git` is a file (a linked worktree).
fn worktree_top(path: &Path) -> Result<(PathBuf, bool), &'static str> {
    let mut top = path;
    for _ in 0..MAX_WORKTREE_DEPTH {
        match std::fs::symlink_metadata(top.join(".git")) {
            Ok(meta) if meta.is_dir() => return Ok((top.to_path_buf(), false)),
            Ok(meta) if meta.is_file() => return Ok((top.to_path_buf(), true)),
            Ok(_) => return Err("unreadable-metadata"),
            Err(_) => {}
        }
        top = top.parent().ok_or("not-a-worktree")?;
    }
    Err("not-a-worktree")
}

/// The repositories the client's reached projects anchor now
/// (`repository_anchor` of each root, read at each use: a project whose
/// directory changed anchors its new one), and why each that anchors
/// nothing does not.
pub(super) fn project_anchors(
    client: &Client,
) -> Vec<(String, String, Result<PathBuf, &'static str>)> {
    client_scopes(client)
        .into_iter()
        .flat_map(|scope| {
            let id = scope.project_id.clone();
            scope.roots.into_iter().map(move |root| {
                let anchor = std::fs::canonicalize(&root)
                    .map_err(|_| "not-a-repository-root")
                    .and_then(|r| repository_anchor(&r));
                (id.clone(), root, anchor)
            })
        })
        .collect()
}

/// The scopes the linked-worktree switch gives canonical directory `path`
/// (approved 2026-11-28, `Client.auto_worktrees`): when the switch is on
/// and `path` lies in a linked worktree (`linked_common`) of a repository a
/// project the client reaches anchors (`project_anchors`), one scope per
/// such project, its root the worktree's top — wherever that worktree is.
/// The worktree's top passes the root policy. `Err` is why not (a closed
/// word). Off, it is empty: the switch adds start directories and nothing
/// else.
pub(super) fn auto_worktree_scope(
    client: &Client,
    path: &Path,
) -> Result<Vec<ProjectScope>, &'static str> {
    if !client.auto_worktrees {
        return Ok(Vec::new());
    }
    let (top, linked) = worktree_top(path)?;
    if !linked {
        return Err("separate-repository");
    }
    let common = linked_common(&top)?;
    if crate::mcp_fs::root_policy(&top).is_err() {
        return Err("path-policy");
    }
    let root = top.display().to_string();
    let mut scopes: Vec<ProjectScope> = Vec::new();
    for (project, _, anchor) in project_anchors(client) {
        if anchor.as_ref().ok() == Some(&common) && !scopes.iter().any(|s| s.project_id == project)
        {
            scopes.push(ProjectScope {
                project_id: project,
                roots: vec![root.clone()],
            });
        }
    }
    if scopes.is_empty() {
        return Err("other-repository");
    }
    Ok(scopes)
}

/// How a start directory was authorized (`project_cwd`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CwdGrant {
    /// Inside the project's own roots.
    Roots,
    /// Inside a linked worktree of the project's repository, by the switch.
    AutoWorktree,
}

impl CwdGrant {
    pub(super) fn word(self) -> &'static str {
        match self {
            CwdGrant::Roots => "roots",
            CwdGrant::AutoWorktree => "auto-worktree",
        }
    }
}

/// The working directory a managed session or worker card of `project`
/// starts in: `path` canonicalized inside the project's roots
/// (`canonical_scope`) first, or else inside a linked worktree of the
/// project's repository the switch admits (`auto_worktree_scope`); refused
/// otherwise. The roots decide on their own: the switch off never refuses
/// what they hold.
pub(super) fn project_cwd(
    client: &Client,
    project: &ProjectScope,
    path: &str,
) -> Result<(PathBuf, CwdGrant), DeckError> {
    match canonical_scope(path, &project.roots) {
        Ok(cwd) => Ok((cwd, CwdGrant::Roots)),
        Err(error) if error.kind() == ErrorKind::Perm => {
            let canonical = std::fs::canonicalize(path).map_err(|_| error.clone())?;
            match auto_worktree_scope(client, &canonical) {
                Ok(scopes) if scopes.iter().any(|s| s.project_id == project.project_id) => {
                    Ok((canonical, CwdGrant::AutoWorktree))
                }
                _ => Err(error),
            }
        }
        Err(error) => Err(error),
    }
}

/// A directory's identity (device, inode), recorded when a start is
/// accepted and compared at its launch: the same path naming another
/// directory (replaced, moved over) is not the directory admitted.
pub(super) fn dir_identity(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path)
        .ok()
        .filter(|m| m.is_dir())
        .map(|m| (m.dev(), m.ino()))
}

/// What the capabilities say of the switch: whether it is on and, for each
/// reached project root, whether it anchors a repository (else why).
pub(super) fn auto_worktrees_view(client: &Client) -> Value {
    if !client.auto_worktrees {
        return json!({"enabled": false});
    }
    json!({
        "enabled": true,
        "note": "worker starts and managed sessions may also use any registered linked worktree of these projects' repositories, wherever it is; authorizedWorkspaces stays the file-read scope",
        "projects": project_anchors(client)
            .into_iter()
            .map(|(project, root, anchor)| match anchor {
                Ok(_) => json!({"projectId": project, "root": root, "anchored": true}),
                Err(why) => json!({"projectId": project, "root": root, "anchored": false, "reason": why}),
            })
            .collect::<Vec<_>>(),
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
