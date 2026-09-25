//! Upgrade-aware lifecycle for deck's persistent tmux server.
//!
//! The GUI process is intentionally disposable; the server is not. This
//! module is the single authority that decides whether a reachable server can
//! be reused and the only place allowed to replace it.
//!
//! # Contract
//! `tmux_lifecycle.rs` owns the server boundary. It inspects a versioned JSON
//! server option before scheduler/webview startup, reuses only a compatible
//! server, automatically replaces an empty old/legacy server, and persists a
//! content-free pending/restart transaction when sessions exist. Never write
//! current metadata onto an unknown existing server: that would relabel old
//! code as current. Session creation must hold `session_creation_guard`; attach
//! to an existing pending session remains allowed. The restart command rechecks
//! PID/start-time/session/pane counts under the same gate, detaches PTYs, kills
//! and waits, validates a stale socket against its captured device/inode, starts
//! from the current sidecar, then requires a new PID and read-back identity.
//! The updater takes the same gate before setting its creation embargo. Cards
//! are marked stopped after replacement is confirmed or observed and before
//! polling, so a refused restart never presents live cards as stopped.
//! A managed MCP runner cannot use ordinary shell restoration, so an explicit
//! restart is refused until its MCP cards are closed through the Board path:
//! MCP registers a display-safe constraint provider once at boot
//! (`set_restart_guard`, from `mcp::spawn`); this module names no feature module
//! itself. The
//! restart transaction runs the registered guard before any tmux impact,
//! and an unset guard means no feature objects.
//!
//! Every tmux-facing step (`probe_server_on`, `start_current_server_on`,
//! `wait_for_old_server_exit_on`, `clean_confirmed_intent_socket_on`,
//! `complete_restart_on`) and the lifecycle file (`read_disk_at`,
//! `write_disk_at`, `status_from_probe_with`) take a `ServerHandle`;
//! production builds exactly one, `deck_server()`, and the parameterless
//! wrappers used by the boot gate and commands pass it. Tests run the same
//! code against a throwaway bundled-tmux socket and a temporary file.
//!
//! Production Stable/Nightly intentionally share socket `deck` because
//! promotion copies identical candidate bytes. Debug development uses
//! `deck-dev` and bundle ID `io.c9r.deck.dev`; smoke requires `deck-smoke*` and
//! `io.c9r.deck.smoke`. Release creation is allowed only from
//! `/Applications/deck.app` or `~/Applications/deck.app` with the adjacent
//! bundled helper. Updater installation sets a process-local creation embargo
//! before Tauri renames the running app into `tauri_current_app`; a failed
//! install clears it, a successful install exits/relaunches from the stable app.
//! Increment `SERVER_PROTOCOL` only for a true compatibility break.

use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock, TryLockError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::applog::applog;
use crate::error::{DeckError, ErrorKind, RestartFailure};
use crate::sync::LockRecover;
use crate::tmux::{self, tmux, tmux_owned};

const METADATA_OPTION: &str = "@deck-server-metadata";
const METADATA_SCHEMA: u32 = 1;
/// Bump only when a server/helper compatibility boundary changes. Release
/// builds still restart across build IDs to renew their responsible-code
/// identity; debug rebuilds may share a server while this protocol matches.
pub(crate) const SERVER_PROTOCOL: u32 = 1;
const RELEASE_BUNDLE_ID: &str = "io.c9r.deck";
const DEVELOPMENT_BUNDLE_ID: &str = "io.c9r.deck.dev";
const SMOKE_BUNDLE_ID: &str = "io.c9r.deck.smoke";
const LIFECYCLE_FILE: &str = "tmux-lifecycle.json";

static OPERATION: Mutex<()> = Mutex::new(());
static APP_UPDATE_INSTALLING: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SourceCategory {
    Installed,
    Development,
    Smoke,
    Transient,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ServerMetadata {
    schema_version: u32,
    protocol_version: u32,
    channel: String,
    bundle_identifier: String,
    app_version: String,
    build_identifier: String,
    helper_version: String,
    created_at: u64,
    source: SourceCategory,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CurrentBuildIdentity {
    channel: String,
    bundle_identifier: String,
    app_version: String,
    build_identifier: String,
    helper_version: String,
    protocol_version: u32,
    source: SourceCategory,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub(crate) enum CompatibilityState {
    CompatibleCurrentBuild,
    CompatibleDifferentBuild,
    RestartRequired,
    LegacyUnknown,
    CorruptOrUnreachable,
    SourceUnstable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionImpact {
    name: String,
    pane_count: u32,
    attached_clients: u32,
    has_foreground_process: bool,
    recently_active: bool,
}

/// Feature-owned, display-safe reasons that prevent server replacement.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum RestartBlockerKind {
    ManagedSession,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RestartBlocker {
    pub(crate) kind: RestartBlockerKind,
    pub(crate) session: String,
    pub(crate) card_id: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ServerStatus {
    status: CompatibilityState,
    pending_restart: bool,
    should_prompt: bool,
    can_restart: bool,
    restart_blockers: Vec<RestartBlocker>,
    current_build: CurrentBuildIdentity,
    server_build: Option<ServerMetadata>,
    server_pid: Option<u32>,
    server_started_at: Option<u64>,
    impact_token: Option<String>,
    session_count: u32,
    pane_count: u32,
    attached_session_count: u32,
    foreground_session_count: u32,
    sessions: Vec<SessionImpact>,
    notice: Option<String>,
}

#[derive(Clone, Debug)]
struct ServerSnapshot {
    pid: u32,
    started_at: u64,
    socket_path: PathBuf,
    socket_device: u64,
    socket_inode: u64,
    metadata: MetadataRead,
    sessions: Vec<SessionImpact>,
    impact_token: String,
    panes: Vec<tmux::PaneRow>,
}

impl ServerSnapshot {
    fn pane_count(&self) -> u32 {
        self.sessions.iter().map(|session| session.pane_count).sum()
    }
}

#[derive(Clone, Debug)]
enum MetadataRead {
    Present(ServerMetadata),
    Missing,
    Corrupt,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct RestartIntent {
    build_key: String,
    old_pid: u32,
    old_started_at: u64,
    old_socket_device: u64,
    old_socket_inode: u64,
    session_count: u32,
    pane_count: u32,
    impact_token: String,
    // Files written before 2026-09-25 also carry a `phase` key; it was never
    // read and is ignored (this struct accepts unknown keys).
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct LifecycleNotice {
    code: String,
    build_key: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct LifecycleDisk {
    schema_version: u32,
    deferred_build: Option<String>,
    operation: Option<RestartIntent>,
    notice: Option<LifecycleNotice>,
}

impl Default for LifecycleDisk {
    fn default() -> Self {
        Self {
            schema_version: 1,
            deferred_build: None,
            operation: None,
            notice: None,
        }
    }
}

enum Probe {
    Absent,
    Reachable(Box<ServerSnapshot>),
    Unreachable,
}

/// The one server this module inspects and replaces. Production has exactly
/// one, `deck_server()`: deck's own server through `tmux::tmux` /
/// `tmux::tmux_owned` on `tmux::socket()`, the query client identity from
/// `tmux::owned_control_client`, and the lifecycle file under the data dir.
/// The probe, start, stop-wait, stale-socket cleanup and restart transaction
/// take the handle as an argument so tests run the same code against a
/// throwaway bundled-tmux socket and a temporary lifecycle file.
struct ServerHandle<'a> {
    run: &'a dyn Fn(&[&str]) -> Result<String, DeckError>,
    run_owned: &'a dyn Fn(&[String]) -> Result<String, DeckError>,
    owned_client: &'a dyn Fn() -> Option<(u32, u32, String)>,
    socket_name: &'a str,
    lifecycle_file: PathBuf,
}

fn deck_server() -> ServerHandle<'static> {
    ServerHandle {
        run: &tmux,
        run_owned: &tmux_owned,
        owned_client: &tmux::owned_control_client,
        socket_name: tmux::socket(),
        lifecycle_file: lifecycle_path(),
    }
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn lifecycle_path() -> PathBuf {
    crate::datadir::deck_dir().join(LIFECYCLE_FILE)
}

fn read_disk() -> LifecycleDisk {
    read_disk_at(&lifecycle_path())
}

fn read_disk_at(path: &Path) -> LifecycleDisk {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return LifecycleDisk::default();
    };
    serde_json::from_str::<LifecycleDisk>(&raw)
        .ok()
        .filter(|disk| disk.schema_version == 1)
        .unwrap_or_default()
}

fn write_disk(disk: &LifecycleDisk) -> Result<(), DeckError> {
    write_disk_at(&lifecycle_path(), disk)
}

fn write_disk_at(path: &Path, disk: &LifecycleDisk) -> Result<(), DeckError> {
    crate::session_runtime::check_deadline()?;
    if let Some(dir) = path.parent() {
        crate::datadir::create_private_dir(dir)?;
    }
    let bytes = serde_json::to_vec(disk)
        .map_err(|_| DeckError::new(ErrorKind::Other, "lifecycle-state-encode"))?;
    crate::datadir::atomic_write(path, &bytes)
}

pub(crate) fn app_bundle_root(executable: &Path) -> Option<&Path> {
    let macos = executable.parent()?;
    if macos.file_name()?.to_str()? != "MacOS" {
        return None;
    }
    let contents = macos.parent()?;
    if contents.file_name()?.to_str()? != "Contents" {
        return None;
    }
    let app = contents.parent()?;
    if app.extension()?.to_str()? != "app" {
        return None;
    }
    Some(app)
}

pub(crate) fn stable_installed_bundle(app: &Path) -> bool {
    if app == Path::new("/Applications/deck.app") {
        return true;
    }
    dirs::home_dir()
        .map(|home| app == home.join("Applications/deck.app"))
        .unwrap_or(false)
}

pub(crate) fn source_category() -> SourceCategory {
    if cfg!(debug_assertions) {
        if crate::launch_args::debug_arg("--smoke-data-dir").is_some() {
            SourceCategory::Smoke
        } else {
            SourceCategory::Development
        }
    } else {
        std::env::current_exe()
            .ok()
            .as_deref()
            .and_then(app_bundle_root)
            .filter(|app| stable_installed_bundle(app))
            .map(|_| SourceCategory::Installed)
            .unwrap_or(SourceCategory::Transient)
    }
}

fn bundle_identifier(source: SourceCategory) -> &'static str {
    match source {
        SourceCategory::Installed | SourceCategory::Transient => RELEASE_BUNDLE_ID,
        SourceCategory::Development => DEVELOPMENT_BUNDLE_ID,
        SourceCategory::Smoke => SMOKE_BUNDLE_ID,
    }
}

fn helper_version() -> String {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION
        .get_or_init(|| {
            tmux::tmux_program()
                .ok()
                .and_then(|tmux_sidecar| {
                    std::process::Command::new(tmux_sidecar)
                        .arg("-V")
                        .output()
                        .ok()
                })
                .filter(|out| out.status.success())
                .and_then(|out| String::from_utf8(out.stdout).ok())
                .map(|value| value.trim().to_string())
                .filter(|value| {
                    !value.is_empty()
                        && value.len() <= 64
                        && value.chars().all(|ch| ch.is_ascii_graphic() || ch == ' ')
                })
                .unwrap_or_else(|| "unknown".into())
        })
        .clone()
}

pub(crate) fn current_build() -> CurrentBuildIdentity {
    let source = source_category();
    CurrentBuildIdentity {
        channel: match source {
            SourceCategory::Installed | SourceCategory::Transient => {
                crate::documents::update_channel_setting()
            }
            SourceCategory::Development => "development".into(),
            SourceCategory::Smoke => "smoke".into(),
        },
        bundle_identifier: bundle_identifier(source).into(),
        app_version: env!("CARGO_PKG_VERSION").into(),
        build_identifier: env!("DECK_BUILD_COMMIT").into(),
        helper_version: helper_version(),
        protocol_version: SERVER_PROTOCOL,
        source,
    }
}

fn build_key(build: &CurrentBuildIdentity) -> String {
    format!(
        "{}:{}:{}:{}",
        build.bundle_identifier, build.app_version, build.build_identifier, build.protocol_version
    )
}

fn metadata_for_current(build: &CurrentBuildIdentity) -> ServerMetadata {
    ServerMetadata {
        schema_version: METADATA_SCHEMA,
        protocol_version: build.protocol_version,
        channel: build.channel.clone(),
        bundle_identifier: build.bundle_identifier.clone(),
        app_version: build.app_version.clone(),
        build_identifier: build.build_identifier.clone(),
        helper_version: build.helper_version.clone(),
        created_at: now_epoch(),
        source: build.source,
    }
}

fn compatible_state(build: &CurrentBuildIdentity, metadata: &MetadataRead) -> CompatibilityState {
    if build.source == SourceCategory::Transient {
        return CompatibilityState::SourceUnstable;
    }
    let MetadataRead::Present(server) = metadata else {
        return match metadata {
            MetadataRead::Missing => CompatibilityState::LegacyUnknown,
            MetadataRead::Corrupt => CompatibilityState::CorruptOrUnreachable,
            MetadataRead::Present(_) => unreachable!(),
        };
    };
    let protocol_matches = server.schema_version == METADATA_SCHEMA
        && server.protocol_version == build.protocol_version
        && server.bundle_identifier == build.bundle_identifier
        && server.helper_version == build.helper_version
        && server.source == build.source;
    if !protocol_matches {
        return CompatibilityState::RestartRequired;
    }
    let exact = server.app_version == build.app_version
        && server.build_identifier == build.build_identifier;
    if exact {
        CompatibilityState::CompatibleCurrentBuild
    } else if matches!(
        build.source,
        SourceCategory::Development | SourceCategory::Smoke
    ) {
        CompatibilityState::CompatibleDifferentBuild
    } else {
        CompatibilityState::RestartRequired
    }
}

fn should_auto_replace(state: CompatibilityState, session_count: usize) -> bool {
    session_count == 0
        && matches!(
            state,
            CompatibilityState::RestartRequired
                | CompatibilityState::LegacyUnknown
                | CompatibilityState::CorruptOrUnreachable
        )
}

fn should_prompt_for_restart(
    state: CompatibilityState,
    session_count: usize,
    deferred_build: Option<&str>,
    current_build_key: &str,
) -> bool {
    session_count > 0
        && matches!(
            state,
            CompatibilityState::RestartRequired
                | CompatibilityState::LegacyUnknown
                | CompatibilityState::CorruptOrUnreachable
        )
        && deferred_build != Some(current_build_key)
}

fn restart_intent_still_matches(intent: &RestartIntent, snapshot: &ServerSnapshot) -> bool {
    snapshot.pid == intent.old_pid
        && snapshot.started_at == intent.old_started_at
        && snapshot.socket_device == intent.old_socket_device
        && snapshot.socket_inode == intent.old_socket_inode
        && snapshot.sessions.len() as u32 == intent.session_count
        && snapshot.pane_count() == intent.pane_count
        && snapshot.impact_token == intent.impact_token
}

fn impact_token(
    pid: u32,
    started_at: u64,
    socket_device: u64,
    socket_inode: u64,
    sessions: &[SessionImpact],
    panes: &[(String, String, u32, String)],
) -> String {
    let mut hasher = DefaultHasher::new();
    "deck-tmux-impact-v1".hash(&mut hasher);
    pid.hash(&mut hasher);
    started_at.hash(&mut hasher);
    socket_device.hash(&mut hasher);
    socket_inode.hash(&mut hasher);
    let mut sorted_sessions = sessions.to_vec();
    sorted_sessions.sort_by(|a, b| a.name.cmp(&b.name));
    for session in sorted_sessions {
        session.name.hash(&mut hasher);
        session.pane_count.hash(&mut hasher);
        session.attached_clients.hash(&mut hasher);
        session.has_foreground_process.hash(&mut hasher);
    }
    let mut sorted_panes = panes.to_vec();
    sorted_panes.sort();
    sorted_panes.hash(&mut hasher);
    format!("impact-v1-{:016x}", hasher.finish())
}

fn absent_error(error: &str) -> bool {
    matches!(crate::error::err_code(error), "no-session" | "missing")
        || error.contains("no server running")
        || error.contains("no sessions")
}

fn subtract_owned_control_client(
    server_pid: u32,
    sessions: &mut [SessionImpact],
    clients: &str,
    owned: Option<(u32, u32, String)>,
) -> Result<(), ()> {
    let Some((owned_pid, owned_server_pid, owned_session)) = owned else {
        return Ok(());
    };
    if owned_server_pid != server_pid {
        return Ok(());
    }
    let mut verified = false;
    for line in clients.lines() {
        let mut fields = line.split('\t');
        let (Some(pid), Some(control), Some(flags), Some(session), None) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) else {
            return Err(());
        };
        let pid = pid.parse::<u32>().map_err(|_| ())?;
        let flags: Vec<_> = flags.split(',').collect();
        if pid == owned_pid
            && control == "1"
            && crate::tmux_clients::QUERY_CLIENT_FLAGS
                .split(',')
                .all(|flag| flags.contains(&flag))
            && session == owned_session
        {
            if verified {
                return Err(());
            }
            verified = true;
        }
    }
    if verified {
        let session = sessions
            .iter_mut()
            .find(|session| session.name == owned_session)
            .ok_or(())?;
        session.attached_clients = session.attached_clients.saturating_sub(1);
    }
    Ok(())
}

fn probe_server() -> Probe {
    probe_server_on(&deck_server())
}

fn probe_server_on(server: &ServerHandle<'_>) -> Probe {
    let head = match (server.run)(&[
        "display-message",
        "-p",
        "#{pid}\t#{start_time}\t#{socket_path}",
    ]) {
        Ok(value) => value,
        Err(error) if absent_error(error.message()) => return Probe::Absent,
        Err(_) => return Probe::Unreachable,
    };
    let mut fields = head.trim_end().split('\t');
    let (Some(pid), Some(started_at), Some(socket_path), None) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return Probe::Unreachable;
    };
    let (Ok(pid), Ok(started_at)) = (pid.parse::<u32>(), started_at.parse::<u64>()) else {
        return Probe::Unreachable;
    };
    let socket_path = PathBuf::from(socket_path);
    let Ok(socket_metadata) = std::fs::symlink_metadata(&socket_path) else {
        return Probe::Unreachable;
    };
    if !socket_metadata.file_type().is_socket() {
        return Probe::Unreachable;
    }

    let metadata = match (server.run)(&["show-options", "-gqv", METADATA_OPTION]) {
        Ok(value) if value.trim().is_empty() => MetadataRead::Missing,
        Ok(value) => serde_json::from_str::<ServerMetadata>(value.trim())
            .map(MetadataRead::Present)
            .unwrap_or(MetadataRead::Corrupt),
        Err(_) => MetadataRead::Corrupt,
    };

    let session_listing = match (server.run)(&[
        "list-sessions",
        "-F",
        "#{session_name}\t#{session_attached}\t#{session_activity}",
    ]) {
        Ok(value) => value,
        Err(error) if absent_error(error.message()) => String::new(),
        Err(_) => return Probe::Unreachable,
    };
    let mut sessions = Vec::new();
    for line in session_listing.lines() {
        let mut fields = line.split('\t');
        let (Some(name), Some(attached), Some(activity), None) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return Probe::Unreachable;
        };
        if tmux::validate_session_name(name).is_err() {
            return Probe::Unreachable;
        }
        let (Ok(attached_clients), Ok(activity)) =
            (attached.parse::<u32>(), activity.parse::<u64>())
        else {
            return Probe::Unreachable;
        };
        sessions.push(SessionImpact {
            name: name.into(),
            pane_count: 0,
            attached_clients,
            has_foreground_process: false,
            recently_active: now_epoch().saturating_sub(activity) <= 60,
        });
    }

    // The owned query client only ever reduces an existing session's attach
    // count. An empty server has none to reduce, and its record may outlive
    // the client (it exits with the last session); tmux then refuses
    // `list-clients` with "no current target", which is not unreachability.
    if let Some(owned) = (server.owned_client)().filter(|_| !sessions.is_empty()) {
        let clients = match (server.run)(&[
            "list-clients",
            "-F",
            "#{client_pid}\t#{client_control_mode}\t#{client_flags}\t#{session_name}",
        ]) {
            Ok(value) => value,
            Err(error) if absent_error(error.message()) => String::new(),
            Err(_) => return Probe::Unreachable,
        };
        if subtract_owned_control_client(pid, &mut sessions, &clients, Some(owned)).is_err() {
            return Probe::Unreachable;
        }
    }

    let mut pane_identities = Vec::new();
    let mut pane_rows = Vec::new();
    if !sessions.is_empty() {
        let Ok(rows) = tmux::list_panes_with(server.run) else {
            return Probe::Unreachable;
        };
        pane_rows = rows;
        for row in &pane_rows {
            let Some(session) = sessions
                .iter_mut()
                .find(|session| session.name == row.session_name)
            else {
                return Probe::Unreachable;
            };
            session.pane_count = session.pane_count.saturating_add(1);
            if !crate::context::shell_process(Some(&row.command)) {
                session.has_foreground_process = true;
            }
            pane_identities.push((
                row.session_name.clone(),
                row.pane_id.clone(),
                row.pane_pid,
                row.command.clone(),
            ));
        }
        if sessions.iter().any(|session| session.pane_count == 0) {
            return Probe::Unreachable;
        }
    }

    let socket_device = socket_metadata.dev();
    let socket_inode = socket_metadata.ino();
    let impact_token = impact_token(
        pid,
        started_at,
        socket_device,
        socket_inode,
        &sessions,
        &pane_identities,
    );
    Probe::Reachable(Box::new(ServerSnapshot {
        pid,
        started_at,
        socket_path,
        socket_device,
        socket_inode,
        metadata,
        sessions,
        impact_token,
        panes: pane_rows,
    }))
}

fn source_can_create(build: &CurrentBuildIdentity) -> bool {
    if build.source == SourceCategory::Transient {
        return false;
    }
    if build.source != SourceCategory::Installed {
        return true;
    }
    let Ok(executable) = std::env::current_exe() else {
        return false;
    };
    let Some(macos) = executable.parent() else {
        return false;
    };
    Path::new(tmux::tmux_bin()) == macos.join("tmux")
}

fn start_current_server(build: &CurrentBuildIdentity) -> Result<ServerSnapshot, DeckError> {
    start_current_server_on(&deck_server(), build)
}

fn start_current_server_on(
    server: &ServerHandle<'_>,
    build: &CurrentBuildIdentity,
) -> Result<ServerSnapshot, DeckError> {
    if !source_can_create(build) {
        return Err(DeckError::new(
            ErrorKind::Tmux,
            "tmux-server-source-unstable",
        ));
    }
    let metadata = serde_json::to_string(&metadata_for_current(build))
        .map_err(|_| DeckError::new(ErrorKind::Tmux, "tmux-metadata-encode"))?;
    let args = vec![
        "start-server".into(),
        ";".into(),
        "set-option".into(),
        "-g".into(),
        "exit-empty".into(),
        "off".into(),
        ";".into(),
        "set-option".into(),
        "-g".into(),
        METADATA_OPTION.into(),
        metadata,
    ];
    (server.run_owned)(&args)?;
    match probe_server_on(server) {
        Probe::Reachable(snapshot)
            if compatible_state(build, &snapshot.metadata)
                == CompatibilityState::CompatibleCurrentBuild =>
        {
            Ok(*snapshot)
        }
        _ => Err(DeckError::new(
            ErrorKind::Tmux,
            "tmux-server-verification-failed",
        )),
    }
}

fn safe_stale_socket(
    path: &Path,
    expected_socket_name: &str,
    expected_device: u64,
    expected_inode: u64,
) -> bool {
    let expected_parent = format!("tmux-{}", unsafe { libc::getuid() });
    path.file_name().and_then(|name| name.to_str()) == Some(expected_socket_name)
        && path
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            == Some(expected_parent.as_str())
        && std::fs::symlink_metadata(path)
            .map(|metadata| {
                metadata.file_type().is_socket()
                    && metadata.dev() == expected_device
                    && metadata.ino() == expected_inode
            })
            .unwrap_or(false)
}

fn clean_confirmed_intent_socket(intent: &RestartIntent) -> Result<(), DeckError> {
    clean_confirmed_intent_socket_on(&deck_server(), intent)
}

fn clean_confirmed_intent_socket_on(
    server: &ServerHandle<'_>,
    intent: &RestartIntent,
) -> Result<(), DeckError> {
    let path = Path::new("/tmp")
        .join(format!("tmux-{}", unsafe { libc::getuid() }))
        .join(server.socket_name);
    if !path.exists() {
        return Ok(());
    }
    if !safe_stale_socket(
        &path,
        server.socket_name,
        intent.old_socket_device,
        intent.old_socket_inode,
    ) {
        return Err(DeckError::new(
            ErrorKind::Tmux,
            "tmux-server-socket-not-safe",
        ));
    }
    std::fs::remove_file(path)
        .map_err(|_| DeckError::new(ErrorKind::Tmux, "tmux-server-stale-socket"))
}

fn wait_for_old_server_exit_on(
    server: &ServerHandle<'_>,
    old: &ServerSnapshot,
) -> Result<(), DeckError> {
    for _ in 0..50 {
        crate::session_runtime::check_deadline()?;
        match probe_server_on(server) {
            Probe::Absent => break,
            Probe::Reachable(snapshot)
                if snapshot.pid != old.pid || snapshot.started_at != old.started_at =>
            {
                return Err(DeckError::new(
                    ErrorKind::Tmux,
                    "tmux-server-replaced-concurrently",
                ))
            }
            Probe::Unreachable => {}
            Probe::Reachable(_) => {}
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if matches!(probe_server_on(server), Probe::Reachable(snapshot) if snapshot.pid == old.pid && snapshot.started_at == old.started_at)
    {
        return Err(DeckError::new(ErrorKind::Tmux, "tmux-server-stop-timeout"));
    }
    if old.socket_path.exists() {
        if !safe_stale_socket(
            &old.socket_path,
            server.socket_name,
            old.socket_device,
            old.socket_inode,
        ) {
            return Err(DeckError::new(
                ErrorKind::Tmux,
                "tmux-server-socket-not-safe",
            ));
        }
        std::fs::remove_file(&old.socket_path)
            .map_err(|_| DeckError::new(ErrorKind::Tmux, "tmux-server-stale-socket"))?;
    }
    Ok(())
}

fn complete_restart(
    build: &CurrentBuildIdentity,
    old: &ServerSnapshot,
    notice_code: &str,
) -> Result<ServerSnapshot, DeckError> {
    complete_restart_on(&deck_server(), build, old, notice_code)
}

fn complete_restart_on(
    server: &ServerHandle<'_>,
    build: &CurrentBuildIdentity,
    old: &ServerSnapshot,
    notice_code: &str,
) -> Result<ServerSnapshot, DeckError> {
    let key = build_key(build);
    let mut disk = read_disk_at(&server.lifecycle_file);
    disk.operation = Some(RestartIntent {
        build_key: key.clone(),
        old_pid: old.pid,
        old_started_at: old.started_at,
        old_socket_device: old.socket_device,
        old_socket_inode: old.socket_inode,
        session_count: old.sessions.len() as u32,
        pane_count: old.pane_count(),
        impact_token: old.impact_token.clone(),
    });
    write_disk_at(&server.lifecycle_file, &disk)?;

    crate::session_runtime::check_deadline()?;
    let stop_started = std::time::Instant::now();
    applog("[tmux-restart] stopping");
    match (server.run)(&["kill-server"]) {
        Ok(_) => {}
        Err(error) if absent_error(error.message()) => {}
        Err(_) => return Err(DeckError::new(ErrorKind::Tmux, "tmux-server-stop-failed")),
    }
    if crate::smoke_faults::take("tmux-after-stop") {
        return Err(DeckError::new(ErrorKind::Tmux, "injected-tmux-after-stop"));
    }
    wait_for_old_server_exit_on(server, old)?;
    applog(&format!(
        "[tmux-restart] stopped elapsed_ms={}",
        stop_started.elapsed().as_millis()
    ));
    if crate::smoke_faults::take("tmux-after-socket") {
        return Err(DeckError::new(
            ErrorKind::Tmux,
            "injected-tmux-after-socket",
        ));
    }

    crate::session_runtime::check_deadline()?;
    if crate::smoke_faults::take("tmux-before-start") {
        return Err(DeckError::new(
            ErrorKind::Tmux,
            "injected-tmux-before-start",
        ));
    }
    let start_started = std::time::Instant::now();
    applog("[tmux-restart] starting");
    let fresh = start_current_server_on(server, build)?;
    applog(&format!(
        "[tmux-restart] verified elapsed_ms={}",
        start_started.elapsed().as_millis()
    ));

    if crate::smoke_faults::take("tmux-after-metadata") {
        return Err(DeckError::new(
            ErrorKind::Tmux,
            "injected-tmux-after-metadata",
        ));
    }
    if fresh.pid == old.pid
        || compatible_state(build, &fresh.metadata) != CompatibilityState::CompatibleCurrentBuild
    {
        return Err(DeckError::new(
            ErrorKind::Tmux,
            "tmux-server-verification-failed",
        ));
    }

    disk.operation = None;
    disk.deferred_build = None;
    disk.notice = Some(LifecycleNotice {
        code: notice_code.into(),
        build_key: key,
    });
    write_disk_at(&server.lifecycle_file, &disk)?;
    applog(&format!(
        "[tmux-lifecycle] restart complete old_pid={} new_pid={} sessions={} panes={}",
        old.pid,
        fresh.pid,
        old.sessions.len(),
        old.pane_count()
    ));
    Ok(fresh)
}

fn status_from_probe(build: CurrentBuildIdentity, probe: Probe) -> ServerStatus {
    status_from_probe_with(build, probe, read_disk())
}

fn status_from_probe_with(
    build: CurrentBuildIdentity,
    probe: Probe,
    disk: LifecycleDisk,
) -> ServerStatus {
    let key = build_key(&build);
    match probe {
        Probe::Absent => ServerStatus {
            status: if build.source == SourceCategory::Transient {
                CompatibilityState::SourceUnstable
            } else {
                CompatibilityState::CorruptOrUnreachable
            },
            pending_restart: false,
            should_prompt: false,
            can_restart: source_can_create(&build),
            restart_blockers: Vec::new(),
            current_build: build,
            server_build: None,
            server_pid: None,
            server_started_at: None,
            impact_token: None,
            session_count: 0,
            pane_count: 0,
            attached_session_count: 0,
            foreground_session_count: 0,
            sessions: Vec::new(),
            notice: disk
                .notice
                .filter(|notice| notice.build_key == key)
                .map(|notice| notice.code),
        },
        Probe::Unreachable => ServerStatus {
            status: CompatibilityState::CorruptOrUnreachable,
            pending_restart: true,
            should_prompt: false,
            can_restart: false,
            restart_blockers: Vec::new(),
            current_build: build,
            server_build: None,
            server_pid: None,
            server_started_at: None,
            impact_token: None,
            session_count: 0,
            pane_count: 0,
            attached_session_count: 0,
            foreground_session_count: 0,
            sessions: Vec::new(),
            notice: None,
        },
        Probe::Reachable(snapshot) => {
            let state = compatible_state(&build, &snapshot.metadata);
            let pending = matches!(
                state,
                CompatibilityState::RestartRequired
                    | CompatibilityState::LegacyUnknown
                    | CompatibilityState::CorruptOrUnreachable
            );
            let should_prompt = should_prompt_for_restart(
                state,
                snapshot.sessions.len(),
                disk.deferred_build.as_deref(),
                &key,
            );
            let server_build = match &snapshot.metadata {
                MetadataRead::Present(metadata) => Some(metadata.clone()),
                _ => None,
            };
            ServerStatus {
                status: state,
                pending_restart: pending,
                should_prompt,
                can_restart: source_can_create(&build),
                restart_blockers: Vec::new(),
                current_build: build,
                server_build,
                server_pid: Some(snapshot.pid),
                server_started_at: Some(snapshot.started_at),
                impact_token: Some(snapshot.impact_token.clone()),
                session_count: snapshot.sessions.len() as u32,
                pane_count: snapshot.pane_count(),
                attached_session_count: snapshot
                    .sessions
                    .iter()
                    .filter(|session| session.attached_clients > 0)
                    .count() as u32,
                foreground_session_count: snapshot
                    .sessions
                    .iter()
                    .filter(|session| session.has_foreground_process)
                    .count() as u32,
                sessions: snapshot.sessions,
                notice: disk
                    .notice
                    .filter(|notice| notice.build_key == key)
                    .map(|notice| notice.code),
            }
        }
    }
}

fn try_operation() -> Result<MutexGuard<'static, ()>, DeckError> {
    match OPERATION.try_lock() {
        Ok(guard) => Ok(guard),
        Err(TryLockError::WouldBlock) => Err(DeckError::new(
            ErrorKind::Tmux,
            "tmux-server-restart-in-progress",
        )),
        Err(TryLockError::Poisoned(_)) => Err(DeckError::new(
            ErrorKind::Tmux,
            "tmux-server-lifecycle-unavailable",
        )),
    }
}

/// Synchronous boot gate. It runs before the scheduler and before the webview
/// can create/attach sessions, which prevents an updater-relocated process
/// from winning the first-server race.
pub(crate) fn reconcile_on_boot() {
    let _guard = OPERATION.lock_or_recover();
    let build = current_build();
    if build.source == SourceCategory::Transient {
        applog("[tmux-lifecycle] transient release source; server creation disabled");
        return;
    }

    let mut disk = read_disk();
    if let Some(intent) = disk.operation.clone() {
        if intent.build_key == build_key(&build) {
            match probe_server() {
                Probe::Reachable(snapshot) if restart_intent_still_matches(&intent, &snapshot) => {
                    let _ = complete_restart(&build, &snapshot, "restartCompleted");
                    return;
                }
                Probe::Absent => {
                    if let Err(error) = clean_confirmed_intent_socket(&intent) {
                        applog(&format!(
                            "[tmux-lifecycle] restart socket recovery paused ({})",
                            error.code()
                        ));
                        return;
                    }
                    if let Ok(fresh) = start_current_server(&build) {
                        disk.operation = None;
                        disk.deferred_build = None;
                        disk.notice = Some(LifecycleNotice {
                            code: "restartCompleted".into(),
                            build_key: build_key(&build),
                        });
                        let _ = write_disk(&disk);
                        applog(&format!(
                            "[tmux-lifecycle] recovered restart new_pid={}",
                            fresh.pid
                        ));
                    }
                    return;
                }
                Probe::Reachable(snapshot)
                    if compatible_state(&build, &snapshot.metadata)
                        == CompatibilityState::CompatibleCurrentBuild =>
                {
                    disk.operation = None;
                    disk.deferred_build = None;
                    let _ = write_disk(&disk);
                    return;
                }
                _ => {
                    // An unexpected replacement is never killed under an old
                    // confirmation. Drop the intent and require a fresh review.
                    disk.operation = None;
                    let _ = write_disk(&disk);
                }
            }
        } else {
            disk.operation = None;
            let _ = write_disk(&disk);
        }
    }

    match probe_server() {
        Probe::Absent => {
            if let Err(error) = start_current_server(&build) {
                applog(&format!(
                    "[tmux-lifecycle] initial server start failed ({})",
                    error.code()
                ));
            }
        }
        Probe::Reachable(snapshot) => match compatible_state(&build, &snapshot.metadata) {
            CompatibilityState::CompatibleCurrentBuild
            | CompatibilityState::CompatibleDifferentBuild => tmux::init_deck_server(),
            state if should_auto_replace(state, snapshot.sessions.len()) => {
                let _ = complete_restart(&build, &snapshot, "emptyServerReplaced");
            }
            state => {
                applog(&format!(
                    "[tmux-lifecycle] restart pending state={state:?} sessions={} panes={}",
                    snapshot.sessions.len(),
                    snapshot.pane_count()
                ));
            }
        },
        Probe::Unreachable => {
            applog("[tmux-lifecycle] server inspection unavailable");
        }
    }
}

/// Guard every server-creating path. Existing incompatible sessions remain
/// attachable after “later”, but no new session is added to the old helper.
pub(crate) fn session_creation_guard() -> Result<MutexGuard<'static, ()>, DeckError> {
    if APP_UPDATE_INSTALLING.load(Ordering::Acquire) {
        return Err(DeckError::new(ErrorKind::Other, "app-update-installing"));
    }
    let guard = try_operation()?;
    let build = current_build();
    match probe_server() {
        Probe::Absent => {
            start_current_server(&build)?;
        }
        Probe::Reachable(snapshot) => match compatible_state(&build, &snapshot.metadata) {
            CompatibilityState::CompatibleCurrentBuild
            | CompatibilityState::CompatibleDifferentBuild => {}
            state if should_auto_replace(state, snapshot.sessions.len()) => {
                complete_restart(&build, &snapshot, "emptyServerReplaced")?;
            }
            CompatibilityState::SourceUnstable => {
                return Err(DeckError::new(
                    ErrorKind::Tmux,
                    "tmux-server-source-unstable",
                ))
            }
            _ => {
                return Err(DeckError::new(
                    ErrorKind::Tmux,
                    "tmux-server-restart-required",
                ))
            }
        },
        Probe::Unreachable => {
            return Err(DeckError::new(ErrorKind::Tmux, "tmux-server-unreachable"))
        }
    }
    Ok(guard)
}

/// The updater renames the running app bundle while installing. From this
/// point until process exit, the old process must never win a tmux-server
/// creation race, regardless of what path APIs report after the rename.
pub(crate) fn begin_app_update_install() -> Result<(), DeckError> {
    let _guard = try_operation()?;
    APP_UPDATE_INSTALLING.store(true, Ordering::Release);
    Ok(())
}

pub(crate) fn cancel_app_update_install() {
    APP_UPDATE_INSTALLING.store(false, Ordering::Release);
}

pub(crate) fn app_update_installing() -> bool {
    APP_UPDATE_INSTALLING.load(Ordering::Acquire)
}

#[tauri::command]
pub(crate) fn tmux_server_status() -> Result<ServerStatus, DeckError> {
    let mut status = status_from_probe(current_build(), probe_server());
    status.restart_blockers = restart_constraints()?;
    Ok(status)
}

#[tauri::command]
pub(crate) fn defer_tmux_restart() -> Result<ServerStatus, DeckError> {
    let _guard = try_operation()?;
    if APP_UPDATE_INSTALLING.load(Ordering::Acquire) {
        return Err(DeckError::new(ErrorKind::Other, "app-update-installing"));
    }
    let build = current_build();
    let mut disk = read_disk();
    disk.deferred_build = Some(build_key(&build));
    write_disk(&disk)?;
    applog("[tmux-lifecycle] restart deferred");
    tmux_server_status()
}

#[tauri::command]
pub(crate) fn acknowledge_tmux_lifecycle_notice() -> Result<(), DeckError> {
    let _guard = try_operation()?;
    let mut disk = read_disk();
    disk.notice = None;
    write_disk(&disk)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // Preserve the existing flat IPC confirmation fields.
pub(crate) async fn restart_tmux_server(
    app: tauri::AppHandle,
    expected_pid: u32,
    expected_started_at: u64,
    expected_impact_token: String,
    expected_session_count: u32,
    expected_pane_count: u32,
    force: bool,
    restore_shells: bool,
    request_id: String,
) -> Result<ServerStatus, DeckError> {
    use tauri::{Emitter, Manager};
    // Tauri's synchronous command handler would block the webview event loop
    // during exit hooks. All IO, deadlines and guards belong to this worker.
    let started = std::time::Instant::now();
    tauri::async_runtime::spawn_blocking(move || {
        crate::session_runtime::run_bounded(started + crate::restart::TOTAL_BUDGET, move || {
            let _deadline =
                crate::session_runtime::Deadline::until(started + crate::restart::TOTAL_BUDGET);
            let progress = |phase: &str, completed: usize, total: usize| {
                let _ = app.emit(
                    "tmux-restart-progress",
                    serde_json::json!({
                        "requestId": request_id, "phase": phase, "completed": completed,
                        "total": total, "elapsedMs": started.elapsed().as_millis() as u64,
                    }),
                );
            };
            applog("[tmux-restart] begin prepare_budget_ms=3000 total_budget_ms=8000");
            let result = restart_tmux_server_inner(
                &app.state::<crate::pty::PtyState>(),
                &app.state::<crate::scheduler::Queues>(),
                expected_pid,
                expected_started_at,
                expected_impact_token,
                expected_session_count,
                expected_pane_count,
                force,
                restore_shells,
                started,
                &progress,
            );
            applog(&format!(
                "[tmux-restart] finish result={} elapsed_ms={}",
                result
                    .as_ref()
                    .map(|_| "ok")
                    .unwrap_or_else(crate::restart::failure_reason),
                started.elapsed().as_millis()
            ));
            let _ = app.emit("queue-changed", ());
            result
        })
    })
    .await
    .map_err(|_| DeckError::new(ErrorKind::Other, "tmux-restart-worker-failed"))?
}

/// The one feature check a server restart consults (see the header): set
/// once at boot by the feature that owns live sessions, never replaced.
type RestartGuard = fn() -> Result<Vec<RestartBlocker>, DeckError>;
static RESTART_GUARD: OnceLock<RestartGuard> = OnceLock::new();

pub(crate) fn set_restart_guard(guard: RestartGuard) {
    let _ = RESTART_GUARD.set(guard);
}

fn restart_constraints() -> Result<Vec<RestartBlocker>, DeckError> {
    RESTART_GUARD.get().map_or(Ok(Vec::new()), |guard| guard())
}

fn require_no_restart_blockers(blockers: &[RestartBlocker]) -> Result<(), DeckError> {
    if blockers.is_empty() {
        Ok(())
    } else {
        Err(DeckError::new(
            ErrorKind::Locked,
            "tmux-restart-mcp-managed-sessions",
        ))
    }
}

#[allow(clippy::too_many_arguments)]
fn restart_tmux_server_inner(
    pty_state: &crate::pty::PtyState,
    queues: &crate::scheduler::Queues,
    expected_pid: u32,
    expected_started_at: u64,
    expected_impact_token: String,
    expected_session_count: u32,
    expected_pane_count: u32,
    force: bool,
    restore_shells: bool,
    started: std::time::Instant,
    progress: &dyn Fn(&str, usize, usize),
) -> Result<ServerStatus, DeckError> {
    let preparation_deadline =
        crate::session_runtime::Deadline::until(started + crate::restart::PREPARE_BUDGET);
    let _guard = try_operation()?;
    let _activity = crate::session_runtime::exclusive()?;
    require_no_restart_blockers(&restart_constraints()?)?;
    // The query client is still an attached tmux client. Stop it before
    // capturing/rechecking restart impact so it cannot keep the old server
    // alive or perturb attached-client counts during replacement.
    tmux::stop_query_channel();
    if APP_UPDATE_INSTALLING.load(Ordering::Acquire) {
        return Err(DeckError::new(ErrorKind::Other, "app-update-installing"));
    }
    let build = current_build();
    if !source_can_create(&build) {
        return Err(DeckError::new(
            ErrorKind::Tmux,
            "tmux-server-source-unstable",
        ));
    }
    let snapshot = match probe_server() {
        Probe::Reachable(snapshot) => snapshot,
        Probe::Absent => {
            start_current_server(&build)?;
            return Ok(status_from_probe(build, probe_server()));
        }
        Probe::Unreachable => {
            return Err(DeckError::new(ErrorKind::Tmux, "tmux-server-unreachable"))
        }
    };
    let state = compatible_state(&build, &snapshot.metadata);
    if !force
        && matches!(
            state,
            CompatibilityState::CompatibleCurrentBuild
                | CompatibilityState::CompatibleDifferentBuild
        )
    {
        return Ok(status_from_probe(build, Probe::Reachable(snapshot)));
    }
    if snapshot.pid != expected_pid
        || snapshot.started_at != expected_started_at
        || snapshot.sessions.len() as u32 != expected_session_count
        || snapshot.pane_count() != expected_pane_count
        || snapshot.impact_token != expected_impact_token
    {
        return Err(DeckError::restart(RestartFailure::ImpactChanged));
    }
    applog(&format!(
        "[tmux-restart] validated sessions={} panes={} elapsed_ms={}",
        snapshot.sessions.len(),
        snapshot.pane_count(),
        started.elapsed().as_millis()
    ));
    pty_state.detach_all();
    let prepared_rows = crate::restart::prepare(&snapshot.panes, restore_shells, progress)?;
    // Foreground changes caused by graceful exit are expected. Refresh the
    // content-free intent so crash recovery compares the post-exit identity.
    let post_exit = match probe_server() {
        Probe::Reachable(current)
            if current.pid == snapshot.pid && current.started_at == snapshot.started_at =>
        {
            current
        }
        _ => return Err(DeckError::restart(RestartFailure::ImpactChanged)),
    };
    let checked_rows = tmux::list_panes()?;
    if !crate::tmux::unchanged_rows(&prepared_rows, &checked_rows) {
        return Err(DeckError::restart(RestartFailure::ImpactChanged));
    }
    let paused = crate::scheduler::pause_for_server_restart(
        queues,
        &snapshot
            .sessions
            .iter()
            .map(|s| s.name.clone())
            .collect::<Vec<_>>(),
    )?;
    applog(&format!(
        "[tmux-restart] queue-paused count={paused} elapsed_ms={}",
        started.elapsed().as_millis()
    ));
    crate::session_runtime::check_deadline()?;
    drop(preparation_deadline);
    let _replace_deadline = crate::session_runtime::Deadline::until(
        (std::time::Instant::now() + Duration::from_secs(5))
            .min(started + crate::restart::TOTAL_BUDGET),
    );
    progress("replacing", 0, 0);
    pty_state.detach_all();
    let fresh = complete_restart(&build, &post_exit, "restartCompleted")?;
    Ok(status_from_probe(build, Probe::Reachable(Box::new(fresh))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::FileTypeExt;
    use std::os::unix::net::UnixListener;
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
    use std::time::Instant;

    /// Release creation is allowed only from an installed `deck.app`; the
    /// shape check must reject every other layout (a bare binary in
    /// target/, a DMG mount, a renamed bundle) so a transient copy never
    /// owns the shared production server.
    #[test]
    fn only_the_installed_bundle_layout_is_a_stable_release_source() {
        let installed = Path::new("/Applications/deck.app/Contents/MacOS/deck-app");
        assert_eq!(
            app_bundle_root(installed),
            Some(Path::new("/Applications/deck.app"))
        );
        for other in [
            "/Users/x/deck/app/src-tauri/target/release/deck-app",
            "/Volumes/deck/deck.app/Contents/Resources/deck-app",
            "/Volumes/deck/deck.app/MacOS/deck-app",
            "/Applications/deck/Contents/MacOS/deck-app",
            "deck-app",
        ] {
            assert_eq!(app_bundle_root(Path::new(other)), None, "{other}");
        }
        assert!(stable_installed_bundle(Path::new("/Applications/deck.app")));
        let home = dirs::home_dir().expect("home");
        assert!(stable_installed_bundle(&home.join("Applications/deck.app")));
        assert!(!stable_installed_bundle(Path::new(
            "/Volumes/deck/deck.app"
        )));
        assert!(!stable_installed_bundle(Path::new(
            "/Applications/deck-dev.app"
        )));
    }

    static TEST_SOCKET_SEQ: AtomicU64 = AtomicU64::new(0);

    struct IsolatedServer {
        socket: String,
        binary: PathBuf,
    }

    impl IsolatedServer {
        fn new(tag: &str) -> Self {
            let seq = TEST_SOCKET_SEQ.fetch_add(1, AtomicOrdering::Relaxed);
            Self {
                socket: format!("deck-smoke-lifecycle-{tag}-{}-{seq}", std::process::id()),
                binary: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("binaries/tmux-aarch64-apple-darwin"),
            }
        }

        fn output(&self, args: &[&str]) -> std::process::Output {
            Command::new(&self.binary)
                .args(["-f", "/dev/null", "-L", &self.socket])
                .args(args)
                .output()
                .expect("run isolated tmux")
        }

        fn run(&self, args: &[&str]) -> String {
            let output = self.output(args);
            assert!(
                output.status.success(),
                "tmux {args:?} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout)
                .expect("tmux output utf8")
                .trim()
                .to_string()
        }

        fn start(&self, metadata: Option<&ServerMetadata>) {
            let mut args = vec![
                "start-server".to_string(),
                ";".into(),
                "set-option".into(),
                "-g".into(),
                "exit-empty".into(),
                "off".into(),
            ];
            if let Some(metadata) = metadata {
                args.extend([
                    ";".into(),
                    "set-option".into(),
                    "-g".into(),
                    METADATA_OPTION.into(),
                    serde_json::to_string(metadata).unwrap(),
                ]);
            }
            let refs: Vec<&str> = args.iter().map(String::as_str).collect();
            self.run(&refs);
        }

        fn pid(&self) -> u32 {
            self.run(&["display-message", "-p", "#{pid}"])
                .parse()
                .expect("numeric server pid")
        }

        fn metadata(&self) -> MetadataRead {
            let raw = self.run(&["show-options", "-gqv", METADATA_OPTION]);
            if raw.is_empty() {
                MetadataRead::Missing
            } else {
                serde_json::from_str::<ServerMetadata>(&raw)
                    .map(MetadataRead::Present)
                    .unwrap_or(MetadataRead::Corrupt)
            }
        }

        fn new_session(&self, name: &str) {
            self.run(&["new-session", "-d", "-s", name, "/bin/sleep 30"]);
        }

        /// Exactly what `tmux::tmux` does after its spawn, on this socket.
        fn tmux(&self, args: &[&str]) -> Result<String, DeckError> {
            crate::tmux::captured_output(
                Command::new(&self.binary).args(["-f", "/dev/null", "-L", &self.socket]),
                args,
            )
        }

        fn tmux_owned(&self, args: &[String]) -> Result<String, DeckError> {
            crate::tmux::captured_output(
                Command::new(&self.binary).args(["-f", "/dev/null", "-L", &self.socket]),
                args,
            )
        }

        fn is_running(&self) -> bool {
            self.output(&["display-message", "-p", "#{pid}"])
                .status
                .success()
        }

        fn stop(&self) {
            let socket_path = self
                .output(&["display-message", "-p", "#{socket_path}"])
                .stdout;
            let socket_path = PathBuf::from(String::from_utf8_lossy(&socket_path).trim());
            let _ = self.output(&["kill-server"]);
            for _ in 0..30 {
                if !self
                    .output(&["display-message", "-p", "#{pid}"])
                    .status
                    .success()
                {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            if socket_path.file_name().and_then(|name| name.to_str()) == Some(&self.socket)
                && std::fs::symlink_metadata(&socket_path)
                    .is_ok_and(|metadata| metadata.file_type().is_socket())
            {
                let _ = std::fs::remove_file(socket_path);
            }
        }
    }

    impl Drop for IsolatedServer {
        fn drop(&mut self) {
            self.stop();
        }
    }

    fn build(
        source: SourceCategory,
        version: &str,
        commit: &str,
        protocol: u32,
    ) -> CurrentBuildIdentity {
        CurrentBuildIdentity {
            channel: "stable".into(),
            bundle_identifier: bundle_identifier(source).into(),
            app_version: version.into(),
            build_identifier: commit.into(),
            helper_version: "tmux 3.7c".into(),
            protocol_version: protocol,
            source,
        }
    }

    fn metadata(build: &CurrentBuildIdentity) -> MetadataRead {
        MetadataRead::Present(metadata_for_current(build))
    }

    fn impact(name: &str, attached_clients: u32) -> SessionImpact {
        SessionImpact {
            name: name.into(),
            pane_count: 1,
            attached_clients,
            has_foreground_process: false,
            recently_active: false,
        }
    }

    #[test]
    fn only_the_verified_owned_control_client_is_removed_from_impact() {
        let mut sessions = vec![impact("alpha", 2), impact("beta", 1)];
        subtract_owned_control_client(
            77,
            &mut sessions,
            "100\t1\tignore-size,no-output\talpha\n101\t1\tcontrol-mode\talpha\n102\t0\t\tbeta\n",
            Some((100, 77, "alpha".into())),
        )
        .unwrap();
        assert_eq!(sessions[0].attached_clients, 1);
        assert_eq!(sessions[1].attached_clients, 1);

        let mut wrong_server = vec![impact("alpha", 2)];
        subtract_owned_control_client(
            77,
            &mut wrong_server,
            "100\t1\tignore-size,no-output\talpha\n",
            Some((100, 78, "alpha".into())),
        )
        .unwrap();
        assert_eq!(wrong_server[0].attached_clients, 2);

        let mut not_control = vec![impact("alpha", 2)];
        subtract_owned_control_client(
            77,
            &mut not_control,
            "100\t0\tignore-size,no-output\talpha\n",
            Some((100, 77, "alpha".into())),
        )
        .unwrap();
        assert_eq!(not_control[0].attached_clients, 2);

        let mut wrong_flags = vec![impact("alpha", 2)];
        subtract_owned_control_client(
            77,
            &mut wrong_flags,
            "100\t1\tignore-size\talpha\n",
            Some((100, 77, "alpha".into())),
        )
        .unwrap();
        assert_eq!(wrong_flags[0].attached_clients, 2);
    }

    #[test]
    fn owned_control_client_verification_fails_closed() {
        for clients in [
            "not-a-pid\t1\tignore-size,no-output\talpha\n",
            "100\t1\talpha\n",
            "100\t1\tignore-size,no-output\talpha\textra\n",
            "100\t1\tignore-size,no-output\talpha\n100\t1\tignore-size,no-output\talpha\n",
        ] {
            let mut sessions = vec![impact("alpha", 1)];
            assert!(subtract_owned_control_client(
                77,
                &mut sessions,
                clients,
                Some((100, 77, "alpha".into())),
            )
            .is_err());
        }

        let mut missing_session = vec![impact("beta", 1)];
        assert!(subtract_owned_control_client(
            77,
            &mut missing_session,
            "100\t1\tignore-size,no-output\talpha\n",
            Some((100, 77, "alpha".into())),
        )
        .is_err());
    }

    #[test]
    fn release_build_change_requires_restart_but_exact_build_does_not() {
        let current = build(SourceCategory::Installed, "0.4.41", "bbbbbbb", 1);
        assert_eq!(
            compatible_state(&current, &metadata(&current)),
            CompatibilityState::CompatibleCurrentBuild
        );
        let old = build(SourceCategory::Installed, "0.4.40", "aaaaaaa", 1);
        assert_eq!(
            compatible_state(&current, &metadata(&old)),
            CompatibilityState::RestartRequired
        );
    }

    #[test]
    fn same_version_helper_or_protocol_change_is_not_silently_compatible() {
        let current = build(SourceCategory::Installed, "0.4.41", "same", 2);
        let mut old = metadata_for_current(&current);
        old.protocol_version = 1;
        assert_eq!(
            compatible_state(&current, &MetadataRead::Present(old)),
            CompatibilityState::RestartRequired
        );
        let mut old = metadata_for_current(&current);
        old.helper_version = "tmux 3.6".into();
        assert_eq!(
            compatible_state(&current, &MetadataRead::Present(old)),
            CompatibilityState::RestartRequired
        );
    }

    #[test]
    fn development_rebuilds_share_only_an_explicit_protocol() {
        let current = build(SourceCategory::Development, "0.4.41", "bbbbbbb", 3);
        let old = build(SourceCategory::Development, "0.4.40", "aaaaaaa", 3);
        assert_eq!(
            compatible_state(&current, &metadata(&old)),
            CompatibilityState::CompatibleDifferentBuild
        );
        let old_protocol = build(SourceCategory::Development, "0.4.40", "aaaaaaa", 2);
        assert_eq!(
            compatible_state(&current, &metadata(&old_protocol)),
            CompatibilityState::RestartRequired
        );
    }

    #[test]
    fn legacy_corrupt_and_transient_states_are_explicit() {
        let current = build(SourceCategory::Installed, "0.4.41", "bbbbbbb", 1);
        assert_eq!(
            compatible_state(&current, &MetadataRead::Missing),
            CompatibilityState::LegacyUnknown
        );
        assert_eq!(
            compatible_state(&current, &MetadataRead::Corrupt),
            CompatibilityState::CorruptOrUnreachable
        );
        let transient = build(SourceCategory::Transient, "0.4.41", "bbbbbbb", 1);
        assert_eq!(
            compatible_state(&transient, &metadata(&current)),
            CompatibilityState::SourceUnstable
        );
    }

    #[test]
    fn release_creation_accepts_only_stable_applications_locations() {
        assert!(stable_installed_bundle(Path::new("/Applications/deck.app")));
        if let Some(home) = dirs::home_dir() {
            assert!(stable_installed_bundle(&home.join("Applications/deck.app")));
        }
        assert!(!stable_installed_bundle(Path::new(
            "/private/var/folders/T/tauri_current_app/current_app/deck.app"
        )));
        assert!(!stable_installed_bundle(Path::new(
            "/Volumes/deck/deck.app"
        )));
    }

    #[test]
    fn socket_cleanup_accepts_only_our_user_tmux_socket() {
        let expected_parent = format!("tmux-{}", unsafe { libc::getuid() });
        let seq = TEST_SOCKET_SEQ.fetch_add(1, AtomicOrdering::Relaxed);
        let root = Path::new("/tmp").join(format!(
            "deck-lifecycle-socket-test-{}-{seq}",
            std::process::id()
        ));
        let parent = root.join(expected_parent);
        std::fs::create_dir_all(&parent).unwrap();
        let right = parent.join("deck-test-socket");
        let listener = UnixListener::bind(&right).unwrap();
        let metadata = std::fs::symlink_metadata(&right).unwrap();
        assert!(safe_stale_socket(
            &right,
            "deck-test-socket",
            metadata.dev(),
            metadata.ino()
        ));
        assert!(!safe_stale_socket(
            &right,
            "deck-test-socket",
            metadata.dev(),
            metadata.ino().wrapping_add(1)
        ));
        assert!(!safe_stale_socket(
            &right,
            "another-socket",
            metadata.dev(),
            metadata.ino()
        ));
        drop(listener);
        std::fs::remove_file(&right).unwrap();
        std::fs::remove_dir(&parent).unwrap();
        std::fs::remove_dir(&root).unwrap();
    }

    #[test]
    fn real_tmux_same_build_reuses_pid_session_and_process() {
        let current = build(SourceCategory::Installed, "0.4.41", "bbbbbbb", 1);
        let server = IsolatedServer::new("same-build");
        server.start(Some(&metadata_for_current(&current)));
        server.new_session("same-build-session");
        let server_pid = server.pid();
        let pane_pid = server
            .run(&[
                "display-message",
                "-p",
                "-t",
                "same-build-session",
                "#{pane_pid}",
            ])
            .parse::<u32>()
            .unwrap();

        assert_eq!(
            compatible_state(&current, &server.metadata()),
            CompatibilityState::CompatibleCurrentBuild
        );
        assert_eq!(server.pid(), server_pid);
        assert_eq!(
            server
                .run(&[
                    "display-message",
                    "-p",
                    "-t",
                    "same-build-session",
                    "#{pane_pid}"
                ])
                .parse::<u32>()
                .unwrap(),
            pane_pid
        );
        assert!(Command::new("/bin/kill")
            .args(["-0", &pane_pid.to_string()])
            .status()
            .is_ok_and(|status| status.success()));
    }

    #[test]
    fn occupied_legacy_or_old_server_is_never_an_automatic_destroy_target() {
        assert!(!should_auto_replace(CompatibilityState::RestartRequired, 1));
        assert!(!should_auto_replace(CompatibilityState::LegacyUnknown, 1));
        assert!(!should_auto_replace(
            CompatibilityState::CorruptOrUnreachable,
            1
        ));
        assert!(should_auto_replace(CompatibilityState::RestartRequired, 0));
        assert!(should_auto_replace(CompatibilityState::LegacyUnknown, 0));
        assert!(should_auto_replace(
            CompatibilityState::CorruptOrUnreachable,
            0
        ));
        assert!(should_prompt_for_restart(
            CompatibilityState::RestartRequired,
            1,
            None,
            "current"
        ));
        assert!(!should_prompt_for_restart(
            CompatibilityState::RestartRequired,
            1,
            Some("current"),
            "current"
        ));
        assert!(should_prompt_for_restart(
            CompatibilityState::RestartRequired,
            1,
            Some("older-build"),
            "current"
        ));
    }

    #[test]
    fn real_tmux_restart_changes_pid_and_round_trips_new_metadata() {
        let old = build(SourceCategory::Installed, "0.4.40", "aaaaaaa", 1);
        let current = build(SourceCategory::Installed, "0.4.41", "bbbbbbb", 1);
        let server = IsolatedServer::new("restart");
        server.start(Some(&metadata_for_current(&old)));
        server.new_session("restart-session");
        let old_pid = server.pid();
        assert_eq!(
            compatible_state(&current, &server.metadata()),
            CompatibilityState::RestartRequired
        );

        server.stop();
        server.start(Some(&metadata_for_current(&current)));
        let new_pid = server.pid();
        assert_ne!(new_pid, old_pid);
        assert_eq!(
            compatible_state(&current, &server.metadata()),
            CompatibilityState::CompatibleCurrentBuild
        );
        assert!(!server
            .output(&["has-session", "-t", "restart-session"])
            .status
            .success());
        assert!(server.binary.is_absolute() && server.binary.exists());
    }

    #[test]
    fn real_tmux_legacy_and_corrupt_metadata_are_distinguishable() {
        let server = IsolatedServer::new("metadata");
        server.start(None);
        assert!(matches!(server.metadata(), MetadataRead::Missing));
        server.run(&["set-option", "-g", METADATA_OPTION, "{not-json"]);
        assert!(matches!(server.metadata(), MetadataRead::Corrupt));
    }

    #[test]
    fn real_tmux_channel_sockets_do_not_share_servers_or_sessions() {
        let build = build(SourceCategory::Smoke, "0.4.41", "bbbbbbb", 1);
        let first = IsolatedServer::new("channel-a");
        let second = IsolatedServer::new("channel-b");
        first.start(Some(&metadata_for_current(&build)));
        second.start(Some(&metadata_for_current(&build)));
        first.new_session("only-first");
        second.new_session("only-second");
        assert_ne!(first.pid(), second.pid());
        assert!(first
            .output(&["has-session", "-t", "only-first"])
            .status
            .success());
        assert!(!first
            .output(&["has-session", "-t", "only-second"])
            .status
            .success());
        assert!(second
            .output(&["has-session", "-t", "only-second"])
            .status
            .success());
        assert!(!second
            .output(&["has-session", "-t", "only-first"])
            .status
            .success());
    }

    #[test]
    fn restart_intent_is_content_free_and_round_trips() {
        {
            let disk = LifecycleDisk {
                schema_version: 1,
                deferred_build: Some("io.c9r.deck:0.4.41:bbbbbbb:1".into()),
                operation: Some(RestartIntent {
                    build_key: "io.c9r.deck:0.4.41:bbbbbbb:1".into(),
                    old_pid: 42,
                    old_started_at: 10,
                    old_socket_device: 1,
                    old_socket_inode: 2,
                    session_count: 3,
                    pane_count: 4,
                    impact_token: "impact-v1-deadbeef".into(),
                }),
                notice: None,
            };
            let raw = serde_json::to_string(&disk).unwrap();
            assert!(!raw.contains("private-project"));
            assert!(!raw.contains("prompt"));
            assert!(!raw.contains("command"));
            assert!(!raw.contains("socket_path"));
            let decoded: LifecycleDisk = serde_json::from_str(&raw).unwrap();
            assert_eq!(serde_json::to_string(&decoded).unwrap(), raw);
            assert!(!raw.contains("phase"));
        }
    }

    /// A tmux-lifecycle.json written before the unread `phase` key was
    /// dropped still loads, and its interrupted restart is still recognized.
    #[test]
    fn a_lifecycle_file_with_the_old_phase_key_still_loads() {
        for phase in ["stopping", "starting", "verifying"] {
            let raw = format!(
                r#"{{"schema_version":1,"deferred_build":null,"operation":{{"build_key":"current","old_pid":42,"old_started_at":10,"old_socket_device":1,"old_socket_inode":2,"session_count":1,"pane_count":2,"impact_token":"impact-v1-deadbeef","phase":"{phase}"}},"notice":null}}"#
            );
            let disk: LifecycleDisk = serde_json::from_str(&raw).unwrap();
            let intent = disk.operation.expect("interrupted restart kept");
            assert_eq!((intent.old_pid, intent.old_started_at), (42, 10));
            assert_eq!(intent.impact_token, "impact-v1-deadbeef");
        }
    }

    #[test]
    fn interrupted_confirmation_resumes_only_for_the_same_pid_and_impact() {
        let snapshot = ServerSnapshot {
            panes: Vec::new(),
            pid: 42,
            started_at: 10,
            socket_path: PathBuf::from("/private/tmp/tmux-501/test"),
            socket_device: 1,
            socket_inode: 2,
            metadata: MetadataRead::Missing,
            sessions: vec![SessionImpact {
                name: "test-session".into(),
                pane_count: 2,
                attached_clients: 0,
                has_foreground_process: true,
                recently_active: true,
            }],
            impact_token: "impact-v1-deadbeef".into(),
        };
        let mut intent = RestartIntent {
            build_key: "current".into(),
            old_pid: 42,
            old_started_at: 10,
            old_socket_device: 1,
            old_socket_inode: 2,
            session_count: 1,
            pane_count: 2,
            impact_token: "impact-v1-deadbeef".into(),
        };
        assert!(restart_intent_still_matches(&intent, &snapshot));
        intent.session_count = 2;
        assert!(!restart_intent_still_matches(&intent, &snapshot));
        intent.session_count = 1;
        intent.pane_count = 3;
        assert!(!restart_intent_still_matches(&intent, &snapshot));
        intent.pane_count = 2;
        intent.old_pid = 43;
        assert!(!restart_intent_still_matches(&intent, &snapshot));
        intent.old_pid = 42;
        intent.old_started_at = 11;
        assert!(!restart_intent_still_matches(&intent, &snapshot));
        intent.old_started_at = 10;
        intent.old_socket_inode = 3;
        assert!(!restart_intent_still_matches(&intent, &snapshot));
        intent.old_socket_inode = 2;
        intent.impact_token = "impact-v1-replaced".into();
        assert!(!restart_intent_still_matches(&intent, &snapshot));
    }

    #[test]
    fn impact_token_detects_identity_replacement_even_when_counts_match() {
        let sessions = vec![SessionImpact {
            name: "reviewed".into(),
            pane_count: 1,
            attached_clients: 0,
            has_foreground_process: true,
            recently_active: true,
        }];
        let reviewed = impact_token(
            42,
            10,
            1,
            2,
            &sessions,
            &[("reviewed".into(), "%1".into(), 100, "codex".into())],
        );
        let replacement = impact_token(
            42,
            10,
            1,
            2,
            &sessions,
            &[("reviewed".into(), "%2".into(), 101, "codex".into())],
        );
        assert_ne!(reviewed, replacement);
        assert_eq!(
            reviewed,
            impact_token(
                42,
                10,
                1,
                2,
                &sessions,
                &[("reviewed".into(), "%1".into(), 100, "codex".into(),)]
            )
        );
    }

    /// Tests that take or set the process-wide statics (`OPERATION`,
    /// `APP_UPDATE_INSTALLING`) serialize here; CI runs one thread, local
    /// runs may not.
    static LIFECYCLE_STATICS: Mutex<()> = Mutex::new(());

    /// A private temporary directory holding one test's lifecycle file.
    struct TestDir(PathBuf);

    impl TestDir {
        fn new(tag: &str) -> Self {
            let seq = TEST_SOCKET_SEQ.fetch_add(1, AtomicOrdering::Relaxed);
            let dir = std::env::temp_dir()
                .join(format!("deck-lifecycle-{tag}-{}-{seq}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn file(&self) -> PathBuf {
            self.0.join(LIFECYCLE_FILE)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn snapshot(metadata: MetadataRead, sessions: Vec<SessionImpact>) -> ServerSnapshot {
        ServerSnapshot {
            pid: 42,
            started_at: 10,
            socket_path: PathBuf::from("/private/tmp/tmux-501/test"),
            socket_device: 1,
            socket_inode: 2,
            metadata,
            sessions,
            impact_token: "impact-v1-deadbeef".into(),
            panes: Vec::new(),
        }
    }

    fn intent_for(old: &ServerSnapshot, build_key: &str) -> RestartIntent {
        RestartIntent {
            build_key: build_key.into(),
            old_pid: old.pid,
            old_started_at: old.started_at,
            old_socket_device: old.socket_device,
            old_socket_inode: old.socket_inode,
            session_count: old.sessions.len() as u32,
            pane_count: old.pane_count(),
            impact_token: old.impact_token.clone(),
        }
    }

    fn no_owned_client() -> Option<(u32, u32, String)> {
        None
    }

    fn never_owned(_: &[String]) -> Result<String, DeckError> {
        panic!("start-server is not part of this scenario")
    }

    /// Only a missing server or session reads as "absent" (a fresh start is
    /// allowed); any other tmux failure is "unreachable" and fails closed.
    #[test]
    fn only_a_missing_server_or_session_reads_as_absent() {
        for absent in [
            "tmux display-message failed: no server running on /private/tmp/tmux-501/deck",
            "tmux list-sessions failed: no sessions",
            "tmux has-session failed: can't find session: deck-card",
            "error connecting to socket (No such file or directory)",
        ] {
            assert!(absent_error(absent), "{absent}");
        }
        for other in [
            "tmux control timeout",
            "tmux display-message failed: permission denied",
            "tmux-restart-timeout",
            "",
        ] {
            assert!(!absent_error(other), "{other}");
        }
    }

    /// The identity this debug test binary presents: a development source
    /// on the dev bundle ID whose build key names bundle, version, commit
    /// and protocol, and whose own metadata reads back as the current build.
    #[test]
    fn this_debug_binary_is_a_development_source_with_a_stable_build_key() {
        assert_eq!(source_category(), SourceCategory::Development);
        assert_eq!(
            bundle_identifier(SourceCategory::Installed),
            RELEASE_BUNDLE_ID
        );
        assert_eq!(
            bundle_identifier(SourceCategory::Transient),
            RELEASE_BUNDLE_ID
        );
        assert_eq!(
            bundle_identifier(SourceCategory::Development),
            DEVELOPMENT_BUNDLE_ID
        );
        assert_eq!(bundle_identifier(SourceCategory::Smoke), SMOKE_BUNDLE_ID);

        let current = current_build();
        assert_eq!(current.source, SourceCategory::Development);
        assert_eq!(current.channel, "development");
        assert_eq!(current.bundle_identifier, DEVELOPMENT_BUNDLE_ID);
        assert_eq!(current.app_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(current.build_identifier, env!("DECK_BUILD_COMMIT"));
        assert_eq!(current.protocol_version, SERVER_PROTOCOL);
        // The helper version is read once from the sidecar beside the
        // executable; a test binary has none and says so instead of guessing.
        if tmux::tmux_program().is_err() {
            assert_eq!(current.helper_version, "unknown");
        }
        assert!(!current.helper_version.is_empty() && current.helper_version.len() <= 64);
        assert_eq!(helper_version(), current.helper_version);
        assert_eq!(
            build_key(&current),
            format!(
                "{DEVELOPMENT_BUNDLE_ID}:{}:{}:{SERVER_PROTOCOL}",
                current.app_version, current.build_identifier
            )
        );

        let before = now_epoch();
        let metadata = metadata_for_current(&current);
        assert_eq!(metadata.schema_version, METADATA_SCHEMA);
        assert_eq!(metadata.protocol_version, SERVER_PROTOCOL);
        assert_eq!(metadata.source, SourceCategory::Development);
        assert!(metadata.created_at >= before && metadata.created_at <= now_epoch());
        assert_eq!(
            compatible_state(&current, &MetadataRead::Present(metadata)),
            CompatibilityState::CompatibleCurrentBuild
        );

        assert!(!source_can_create(&build(
            SourceCategory::Transient,
            "0.4.41",
            "bbbbbbb",
            1
        )));
        assert!(source_can_create(&build(
            SourceCategory::Development,
            "0.4.41",
            "bbbbbbb",
            1
        )));
        assert!(source_can_create(&build(
            SourceCategory::Smoke,
            "0.4.41",
            "bbbbbbb",
            1
        )));
        // An installed release creates only with its own adjacent sidecar,
        // which is exactly when this executable resolves one.
        assert_eq!(
            source_can_create(&build(SourceCategory::Installed, "0.4.41", "bbbbbbb", 1)),
            tmux::tmux_program().is_ok()
        );
    }

    /// The lifecycle file round-trips through the private data path, an
    /// unreadable or future file reads as empty (never as an error that
    /// would block boot), and an expired restart deadline refuses to write.
    #[test]
    fn lifecycle_state_round_trips_and_unreadable_files_read_as_empty() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TestDir::new("disk");
        let path = dir.0.join("nested").join(LIFECYCLE_FILE);
        let empty = read_disk_at(&path);
        assert_eq!(empty.schema_version, 1);
        assert!(empty.deferred_build.is_none() && empty.operation.is_none());
        assert!(empty.notice.is_none());

        let disk = LifecycleDisk {
            schema_version: 1,
            deferred_build: Some("io.c9r.deck.dev:0.4.41:bbbbbbb:1".into()),
            operation: None,
            notice: Some(LifecycleNotice {
                code: "restartCompleted".into(),
                build_key: "io.c9r.deck.dev:0.4.41:bbbbbbb:1".into(),
            }),
        };
        write_disk_at(&path, &disk).expect("write creates the private directory");
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().unwrap()), 0o700);
        let back = read_disk_at(&path);
        assert_eq!(
            back.deferred_build.as_deref(),
            Some("io.c9r.deck.dev:0.4.41:bbbbbbb:1")
        );
        let notice = back.notice.expect("notice kept");
        assert_eq!(notice.code, "restartCompleted");
        assert_eq!(notice.build_key, "io.c9r.deck.dev:0.4.41:bbbbbbb:1");

        std::fs::write(&path, "{not json").unwrap();
        assert!(read_disk_at(&path).deferred_build.is_none());
        let future =
            r#"{"schema_version":2,"deferred_build":"future","operation":null,"notice":null}"#;
        std::fs::write(&path, future).unwrap();
        assert!(
            read_disk_at(&path).deferred_build.is_none(),
            "a future schema is not interpreted"
        );

        let _deadline =
            crate::session_runtime::Deadline::until(Instant::now() - Duration::from_millis(1));
        let refused = write_disk_at(&path, &disk).expect_err("expired deadline");
        assert_eq!(refused.message(), "tmux-restart-timeout");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), future);
    }

    /// One lifecycle operation at a time, and none while the updater has
    /// renamed the running bundle: every gate answers with its own code
    /// before touching the server or the data directory.
    #[test]
    fn the_operation_gate_and_the_update_embargo_refuse_concurrent_server_work() {
        let _serial = LIFECYCLE_STATICS.lock_or_recover();
        struct ClearEmbargo;
        impl Drop for ClearEmbargo {
            fn drop(&mut self) {
                cancel_app_update_install();
            }
        }
        let _clear = ClearEmbargo;
        assert!(!app_update_installing());
        {
            let _held = try_operation().expect("free gate");
            let refused = [
                ("try_operation", try_operation().map(drop).unwrap_err()),
                (
                    "begin_app_update_install",
                    begin_app_update_install().unwrap_err(),
                ),
                (
                    "defer_tmux_restart",
                    defer_tmux_restart().map(drop).unwrap_err(),
                ),
                (
                    "acknowledge_tmux_lifecycle_notice",
                    acknowledge_tmux_lifecycle_notice().unwrap_err(),
                ),
            ];
            for (name, error) in refused {
                assert_eq!(error.message(), "tmux-server-restart-in-progress", "{name}");
                assert_eq!(error.kind(), ErrorKind::Tmux, "{name}");
            }
            assert!(!app_update_installing(), "a refused begin sets no embargo");
        }
        begin_app_update_install().expect("the gate is free again");
        assert!(app_update_installing());
        let refused = session_creation_guard().map(drop).unwrap_err();
        assert_eq!(refused.message(), "app-update-installing");
        assert_eq!(refused.kind(), ErrorKind::Other);
        assert_eq!(
            defer_tmux_restart().map(drop).unwrap_err().message(),
            "app-update-installing"
        );
        cancel_app_update_install();
        assert!(!app_update_installing());
    }

    #[test]
    fn the_restart_guard_is_registered_once_and_never_replaced() {
        fn first() -> Result<Vec<RestartBlocker>, DeckError> {
            Err(DeckError::new(ErrorKind::Other, "first-guard"))
        }
        fn second() -> Result<Vec<RestartBlocker>, DeckError> {
            Ok(Vec::new())
        }
        set_restart_guard(first);
        set_restart_guard(second);
        let guard = RESTART_GUARD.get().expect("registered");
        assert_eq!(guard().unwrap_err().message(), "first-guard");
    }

    /// The status the Board reads: probe outcome, compatibility, counts,
    /// the deferral that silences the prompt for one build, and a notice
    /// that belongs to the build that wrote it.
    #[test]
    fn server_status_projects_the_probe_the_deferral_and_the_notice() {
        let current = build(SourceCategory::Development, "0.4.41", "bbbbbbb", 1);
        let key = build_key(&current);
        let disk = |deferred: Option<&str>, notice: Option<&str>| LifecycleDisk {
            schema_version: 1,
            deferred_build: deferred.map(Into::into),
            operation: None,
            notice: notice.map(|build_key| LifecycleNotice {
                code: "restartCompleted".into(),
                build_key: build_key.into(),
            }),
        };

        let absent = status_from_probe_with(current.clone(), Probe::Absent, disk(None, Some(&key)));
        assert_eq!(absent.status, CompatibilityState::CorruptOrUnreachable);
        assert!(!absent.pending_restart && !absent.should_prompt);
        assert!(absent.can_restart);
        assert!(absent.server_pid.is_none() && absent.server_build.is_none());
        assert_eq!(absent.session_count, 0);
        assert_eq!(absent.notice.as_deref(), Some("restartCompleted"));
        let other = status_from_probe_with(
            current.clone(),
            Probe::Absent,
            disk(None, Some("io.c9r.deck:0.4.40:aaaaaaa:1")),
        );
        assert_eq!(
            other.notice, None,
            "a notice belongs to the build that wrote it"
        );
        let transient = build(SourceCategory::Transient, "0.4.41", "bbbbbbb", 1);
        let unstable = status_from_probe_with(transient, Probe::Absent, disk(None, None));
        assert_eq!(unstable.status, CompatibilityState::SourceUnstable);
        assert!(!unstable.can_restart);

        let unreachable =
            status_from_probe_with(current.clone(), Probe::Unreachable, disk(None, Some(&key)));
        assert_eq!(unreachable.status, CompatibilityState::CorruptOrUnreachable);
        assert!(unreachable.pending_restart && !unreachable.should_prompt);
        assert!(!unreachable.can_restart);
        assert!(unreachable.impact_token.is_none());
        assert_eq!(unreachable.notice, None);

        let old = build(SourceCategory::Development, "0.4.40", "aaaaaaa", 2);
        let mut sessions = vec![impact("alpha", 1), impact("beta", 0)];
        sessions[1].has_foreground_process = true;
        sessions[1].pane_count = 2;
        let reachable = status_from_probe_with(
            current.clone(),
            Probe::Reachable(Box::new(snapshot(metadata(&old), sessions.clone()))),
            disk(None, Some(&key)),
        );
        assert_eq!(reachable.status, CompatibilityState::RestartRequired);
        assert!(reachable.pending_restart && reachable.should_prompt);
        assert!(reachable.can_restart);
        assert_eq!(reachable.server_pid, Some(42));
        assert_eq!(reachable.server_started_at, Some(10));
        assert_eq!(
            reachable.impact_token.as_deref(),
            Some("impact-v1-deadbeef")
        );
        assert_eq!(reachable.session_count, 2);
        assert_eq!(reachable.pane_count, 3);
        assert_eq!(reachable.attached_session_count, 1);
        assert_eq!(reachable.foreground_session_count, 1);
        assert_eq!(reachable.sessions, sessions);
        assert_eq!(
            reachable.server_build.map(|server| server.app_version),
            Some("0.4.40".into())
        );
        assert_eq!(reachable.notice.as_deref(), Some("restartCompleted"));

        let deferred = status_from_probe_with(
            current.clone(),
            Probe::Reachable(Box::new(snapshot(metadata(&old), sessions.clone()))),
            disk(Some(&key), None),
        );
        assert!(deferred.pending_restart && !deferred.should_prompt);

        let same = status_from_probe_with(
            current.clone(),
            Probe::Reachable(Box::new(snapshot(metadata(&current), Vec::new()))),
            disk(None, None),
        );
        assert_eq!(same.status, CompatibilityState::CompatibleCurrentBuild);
        assert!(!same.pending_restart && !same.should_prompt);
        assert_eq!(same.pane_count, 0);

        let legacy = status_from_probe_with(
            current,
            Probe::Reachable(Box::new(snapshot(MetadataRead::Missing, sessions))),
            disk(None, None),
        );
        assert_eq!(legacy.status, CompatibilityState::LegacyUnknown);
        assert!(legacy.server_build.is_none());
        assert!(legacy.pending_restart && legacy.should_prompt);
    }

    #[test]
    fn real_tmux_probe_reads_identity_metadata_sessions_and_pane_impact() {
        let current = build(SourceCategory::Development, "0.4.41", "bbbbbbb", 1);
        let server = IsolatedServer::new("probe");
        let dir = TestDir::new("probe");
        let run = |args: &[&str]| server.tmux(args);
        let run_owned = |args: &[String]| server.tmux_owned(args);
        let handle = ServerHandle {
            run: &run,
            run_owned: &run_owned,
            owned_client: &no_owned_client,
            socket_name: &server.socket,
            lifecycle_file: dir.file(),
        };
        assert!(
            matches!(probe_server_on(&handle), Probe::Absent),
            "no server yet"
        );

        server.start(Some(&metadata_for_current(&current)));
        let Probe::Reachable(empty) = probe_server_on(&handle) else {
            panic!("a started server is reachable");
        };
        assert_eq!(empty.pid, server.pid());
        assert!(empty.started_at > 0);
        assert_eq!(
            compatible_state(&current, &empty.metadata),
            CompatibilityState::CompatibleCurrentBuild
        );
        assert!(empty.sessions.is_empty() && empty.panes.is_empty());
        assert_eq!(empty.pane_count(), 0);
        let socket = std::fs::symlink_metadata(&empty.socket_path).unwrap();
        assert!(socket.file_type().is_socket());
        assert_eq!(
            (empty.socket_device, empty.socket_inode),
            (socket.dev(), socket.ino())
        );
        assert_eq!(
            empty.socket_path.file_name().and_then(|name| name.to_str()),
            Some(server.socket.as_str())
        );
        assert!(empty.impact_token.starts_with("impact-v1-"));

        server.new_session("alpha");
        server.run(&["new-session", "-d", "-s", "beta", "/bin/sh"]);
        let Probe::Reachable(busy) = probe_server_on(&handle) else {
            panic!("reachable with sessions");
        };
        assert_eq!((busy.pid, busy.started_at), (empty.pid, empty.started_at));
        let mut sessions = busy.sessions.clone();
        sessions.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(
            sessions,
            vec![
                SessionImpact {
                    name: "alpha".into(),
                    pane_count: 1,
                    attached_clients: 0,
                    has_foreground_process: true,
                    recently_active: true,
                },
                SessionImpact {
                    name: "beta".into(),
                    pane_count: 1,
                    attached_clients: 0,
                    has_foreground_process: false,
                    recently_active: true,
                },
            ]
        );
        assert_eq!(busy.pane_count(), 2);
        assert_eq!(busy.panes.len(), 2);
        assert!(busy.panes.iter().all(|row| row.server_pid == busy.pid));
        assert_ne!(busy.impact_token, empty.impact_token);
        let Probe::Reachable(again) = probe_server_on(&handle) else {
            panic!("still reachable");
        };
        assert_eq!(
            again.impact_token, busy.impact_token,
            "same identity, same token"
        );

        server.run(&["set-option", "-g", METADATA_OPTION, "{not-json"]);
        let Probe::Reachable(corrupt) = probe_server_on(&handle) else {
            panic!("reachable with corrupt metadata");
        };
        assert!(matches!(corrupt.metadata, MetadataRead::Corrupt));
        server.run(&["set-option", "-g", METADATA_OPTION, ""]);
        let Probe::Reachable(legacy) = probe_server_on(&handle) else {
            panic!("reachable without metadata");
        };
        assert!(matches!(legacy.metadata, MetadataRead::Missing));
        assert_eq!(
            compatible_state(&current, &legacy.metadata),
            CompatibilityState::LegacyUnknown
        );

        server.stop();
        assert!(matches!(probe_server_on(&handle), Probe::Absent));
    }

    /// The Deck query client exits with the server's last session, but its
    /// record outlives it until the channel is next polled. An empty server
    /// with such a stale record is still reachable and empty: there is no
    /// session to subtract the client from (tmux 3.7c answers `list-clients`
    /// on an empty server with "no current target").
    #[test]
    fn real_tmux_probe_of_an_emptied_server_ignores_a_stale_query_client() {
        let current = build(SourceCategory::Development, "0.4.41", "bbbbbbb", 1);
        let server = IsolatedServer::new("emptied");
        let dir = TestDir::new("emptied");
        server.start(Some(&metadata_for_current(&current)));
        server.new_session("alpha");
        let server_pid = server.pid();
        let stale = move || Some((u32::MAX, server_pid, "alpha".to_owned()));
        let run = |args: &[&str]| server.tmux(args);
        let run_owned = |args: &[String]| server.tmux_owned(args);
        let handle = ServerHandle {
            run: &run,
            run_owned: &run_owned,
            owned_client: &stale,
            socket_name: &server.socket,
            lifecycle_file: dir.file(),
        };
        let Probe::Reachable(busy) = probe_server_on(&handle) else {
            panic!("reachable with a session");
        };
        assert_eq!(busy.sessions.len(), 1);

        server.run(&["kill-session", "-t", "alpha"]);
        let Probe::Reachable(emptied) = probe_server_on(&handle) else {
            panic!("an emptied server is reachable, not unreachable");
        };
        assert_eq!(emptied.pid, server_pid);
        assert!(emptied.sessions.is_empty() && emptied.panes.is_empty());
        server.stop();
    }

    /// A probe never half-trusts a server: any answer that does not parse,
    /// name a real socket, or account for every session and pane is
    /// "unreachable", which blocks creation and restart instead of guessing.
    #[test]
    fn real_tmux_probe_fails_closed_on_any_inconsistent_answer() {
        let current = build(SourceCategory::Development, "0.4.41", "bbbbbbb", 1);
        let server = IsolatedServer::new("inconsistent");
        let dir = TestDir::new("inconsistent");
        server.start(Some(&metadata_for_current(&current)));
        server.new_session("alpha");
        let server_pid = server.pid();
        let plain = std::fs::write(dir.0.join("plain"), b"not a socket");
        assert!(plain.is_ok());
        let run_owned = |args: &[String]| server.tmux_owned(args);
        let owned_client = |pid: u32| move || Some((pid, server_pid, "alpha".to_string()));
        let probe = |run: &dyn Fn(&[&str]) -> Result<String, DeckError>,
                     owned: &dyn Fn() -> Option<(u32, u32, String)>| {
            probe_server_on(&ServerHandle {
                run,
                run_owned: &run_owned,
                owned_client: owned,
                socket_name: &server.socket,
                lifecycle_file: dir.file(),
            })
        };
        let shared: &IsolatedServer = &server;
        let answering = |verb: &'static str, answer: String| {
            move |args: &[&str]| {
                if args[0] == verb {
                    Ok(answer.clone())
                } else {
                    shared.tmux(args)
                }
            }
        };
        let failing = |verb: &'static str| {
            move |args: &[&str]| {
                if args[0] == verb {
                    Err(DeckError::new(ErrorKind::Tmux, "tmux control timeout"))
                } else {
                    shared.tmux(args)
                }
            }
        };
        let real = |args: &[&str]| server.tmux(args);
        let sessions_of = |probe: Probe| match probe {
            Probe::Reachable(snapshot) => Some(snapshot.sessions),
            Probe::Absent => panic!("the server is running"),
            Probe::Unreachable => None,
        };
        assert_eq!(
            sessions_of(probe(&real, &no_owned_client)).map(|s| s.len()),
            Some(1)
        );

        // The identity line.
        assert!(sessions_of(probe(&failing("display-message"), &no_owned_client)).is_none());
        for head in [
            "garbage".to_string(),
            format!("{server_pid}\t1"),
            format!("x\t1\t{}", dir.0.join("plain").display()),
            format!("{server_pid}\tx\t{}", dir.0.join("plain").display()),
            format!("{server_pid}\t1\t{}", dir.0.join("plain").display()),
            format!("{server_pid}\t1\t{}", dir.0.join("missing").display()),
        ] {
            assert!(
                sessions_of(probe(
                    &answering("display-message", head.clone()),
                    &no_owned_client
                ))
                .is_none(),
                "{head:?}"
            );
        }
        // Metadata that cannot be read is corrupt, not legacy.
        match probe(&failing("show-options"), &no_owned_client) {
            Probe::Reachable(snapshot) => {
                assert!(matches!(snapshot.metadata, MetadataRead::Corrupt))
            }
            _ => panic!("metadata failure keeps the server reachable"),
        }
        // The session listing.
        assert!(sessions_of(probe(&failing("list-sessions"), &no_owned_client)).is_none());
        for listing in ["alpha\t0", "alpha\tx\t0", "alpha\t0\tx", "bad name\t0\t0"] {
            assert!(
                sessions_of(probe(
                    &answering("list-sessions", listing.into()),
                    &no_owned_client
                ))
                .is_none(),
                "{listing:?}"
            );
        }
        let no_sessions = |args: &[&str]| {
            if args[0] == "list-sessions" {
                Err(DeckError::classified(
                    "tmux list-sessions failed: no sessions",
                ))
            } else {
                server.tmux(args)
            }
        };
        assert_eq!(
            sessions_of(probe(&no_sessions, &no_owned_client)),
            Some(Vec::new()),
            "an empty listing is an empty server"
        );
        // The pane listing must account for every session, and vice versa.
        assert!(sessions_of(probe(
            &answering("list-panes", String::new()),
            &no_owned_client
        ))
        .is_none());
        assert!(sessions_of(probe(&failing("list-panes"), &no_owned_client)).is_none());
        let renamed = |args: &[&str]| {
            let out = server.tmux(args)?;
            Ok(if args[0] == "list-panes" {
                out.replace("alpha", "gamma")
            } else {
                out
            })
        };
        assert!(sessions_of(probe(&renamed, &no_owned_client)).is_none());
        // The client listing is consulted only for an owned client, and then
        // it must parse.
        assert!(sessions_of(probe(&failing("list-clients"), &no_owned_client)).is_some());
        assert!(sessions_of(probe(&failing("list-clients"), &owned_client(1))).is_none());
        assert!(sessions_of(probe(
            &answering("list-clients", "x\n".into()),
            &owned_client(1)
        ))
        .is_none());
        let no_clients = |args: &[&str]| {
            if args[0] == "list-clients" {
                Err(DeckError::classified(
                    "tmux list-clients failed: no server running",
                ))
            } else {
                server.tmux(args)
            }
        };
        assert!(sessions_of(probe(&no_clients, &owned_client(1))).is_some());
    }

    #[test]
    fn real_tmux_probe_subtracts_only_the_verified_owned_query_client() {
        let current = build(SourceCategory::Development, "0.4.41", "bbbbbbb", 1);
        let server = IsolatedServer::new("owned");
        let dir = TestDir::new("owned");
        server.start(Some(&metadata_for_current(&current)));
        server.new_session("alpha");
        let server_pid = server.pid();
        struct KillOnDrop(std::process::Child);
        impl Drop for KillOnDrop {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut client = KillOnDrop(
            Command::new(&server.binary)
                .args(["-f", "/dev/null", "-L", &server.socket])
                .args(crate::tmux_clients::query_client_args("=alpha"))
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("spawn a query client"),
        );
        let pid = client.0.id();
        let listed = || {
            server
                .run(&["list-clients", "-F", "#{client_pid}"])
                .lines()
                .any(|line| line == pid.to_string())
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        while !listed() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(listed(), "the query client attached");

        let run = |args: &[&str]| server.tmux(args);
        let run_owned = |args: &[String]| server.tmux_owned(args);
        let attached =
            |owned: &dyn Fn() -> Option<(u32, u32, String)>| match probe_server_on(&ServerHandle {
                run: &run,
                run_owned: &run_owned,
                owned_client: owned,
                socket_name: &server.socket,
                lifecycle_file: dir.file(),
            }) {
                Probe::Reachable(snapshot) => snapshot
                    .sessions
                    .iter()
                    .find(|session| session.name == "alpha")
                    .map(|session| session.attached_clients),
                _ => None,
            };
        assert_eq!(attached(&no_owned_client), Some(1));
        let ours = move || Some((pid, server_pid, "alpha".to_string()));
        assert_eq!(attached(&ours), Some(0), "deck's own client is not impact");
        let other_server = move || Some((pid, server_pid.wrapping_add(1), "alpha".to_string()));
        assert_eq!(attached(&other_server), Some(1));
        let other_session = move || Some((pid, server_pid, "beta".to_string()));
        assert_eq!(attached(&other_session), Some(1));
        let other_pid = move || Some((pid.wrapping_add(1), server_pid, "alpha".to_string()));
        assert_eq!(attached(&other_pid), Some(1));

        let _ = client.0.kill();
        let _ = client.0.wait();
        let deadline = Instant::now() + Duration::from_secs(2);
        while listed() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!listed(), "the killed client detached");
        assert_eq!(
            attached(&ours),
            Some(0),
            "a remembered pid alone is never authority"
        );
    }

    #[test]
    fn real_tmux_start_writes_metadata_keeps_the_server_alive_and_verifies_it() {
        let current = build(SourceCategory::Development, "0.4.41", "bbbbbbb", 1);
        let server = IsolatedServer::new("start");
        let dir = TestDir::new("start");
        let starts = std::cell::Cell::new(0u32);
        let run = |args: &[&str]| server.tmux(args);
        let run_owned = |args: &[String]| {
            starts.set(starts.get() + 1);
            server.tmux_owned(args)
        };
        let handle = ServerHandle {
            run: &run,
            run_owned: &run_owned,
            owned_client: &no_owned_client,
            socket_name: &server.socket,
            lifecycle_file: dir.file(),
        };

        let transient = build(SourceCategory::Transient, "0.4.41", "bbbbbbb", 1);
        assert_eq!(
            start_current_server_on(&handle, &transient)
                .unwrap_err()
                .message(),
            "tmux-server-source-unstable"
        );
        assert_eq!(starts.get(), 0, "a transient source never starts a server");
        assert!(!server.is_running());

        let fresh = start_current_server_on(&handle, &current).expect("server started");
        assert_eq!(starts.get(), 1);
        assert_eq!(fresh.pid, server.pid());
        assert!(fresh.sessions.is_empty());
        assert_eq!(
            server.run(&["show-options", "-gv", "exit-empty"]),
            "off",
            "an empty server stays alive"
        );
        let MetadataRead::Present(written) = server.metadata() else {
            panic!("metadata written");
        };
        assert_eq!(written.app_version, current.app_version);
        assert_eq!(written.build_identifier, current.build_identifier);
        assert_eq!(written.bundle_identifier, DEVELOPMENT_BUNDLE_ID);
        assert_eq!(written.channel, current.channel);
        assert_eq!(written.source, SourceCategory::Development);
        assert_eq!(
            compatible_state(&current, &fresh.metadata),
            CompatibilityState::CompatibleCurrentBuild
        );
        let again = start_current_server_on(&handle, &current).expect("idempotent start");
        assert_eq!(again.pid, fresh.pid);

        let blind = |args: &[&str]| {
            if args[0] == "show-options" {
                Ok(String::new())
            } else {
                server.tmux(args)
            }
        };
        let unverifiable = ServerHandle {
            run: &blind,
            run_owned: &run_owned,
            owned_client: &no_owned_client,
            socket_name: &server.socket,
            lifecycle_file: dir.file(),
        };
        assert_eq!(
            start_current_server_on(&unverifiable, &current)
                .unwrap_err()
                .message(),
            "tmux-server-verification-failed"
        );
        let refusing = |_: &[String]| {
            Err(DeckError::new(
                ErrorKind::Tmux,
                "tmux start-server failed: refused",
            ))
        };
        let refused = ServerHandle {
            run: &run,
            run_owned: &refusing,
            owned_client: &no_owned_client,
            socket_name: &server.socket,
            lifecycle_file: dir.file(),
        };
        assert_eq!(
            start_current_server_on(&refused, &current)
                .unwrap_err()
                .message(),
            "tmux start-server failed: refused"
        );
    }

    #[test]
    fn real_tmux_managed_blocker_refusal_leaves_server_and_intent_untouched() {
        let current = build(SourceCategory::Development, "0.4.41", "bbbbbbb", 1);
        let server = IsolatedServer::new("managed-blocker");
        let dir = TestDir::new("managed-blocker");
        server.start(Some(&metadata_for_current(&current)));
        server.new_session("alpha");
        let run = |args: &[&str]| server.tmux(args);
        let run_owned = |args: &[String]| server.tmux_owned(args);
        let handle = ServerHandle {
            run: &run,
            run_owned: &run_owned,
            owned_client: &no_owned_client,
            socket_name: &server.socket,
            lifecycle_file: dir.file(),
        };
        let Probe::Reachable(old) = probe_server_on(&handle) else {
            panic!("old server reachable");
        };
        let blocker = RestartBlocker {
            kind: RestartBlockerKind::ManagedSession,
            session: "alpha".into(),
            card_id: "M1".into(),
        };
        let refused = require_no_restart_blockers(&[blocker]).and_then(|()| {
            complete_restart_on(&handle, &current, &old, "restartCompleted").map(|_| ())
        });
        assert_eq!(
            refused.unwrap_err().message(),
            "tmux-restart-mcp-managed-sessions"
        );
        assert_eq!(server.pid(), old.pid);
        assert!(server
            .output(&["has-session", "-t", "=alpha"])
            .status
            .success());
        assert!(read_disk_at(&dir.file()).operation.is_none());
        assert!(
            matches!(probe_server_on(&handle), Probe::Reachable(snapshot) if snapshot.pid == old.pid)
        );
    }

    #[test]
    fn real_tmux_restart_transaction_replaces_the_server_and_records_the_notice() {
        let old_build = build(SourceCategory::Installed, "0.4.40", "aaaaaaa", 1);
        let current = build(SourceCategory::Development, "0.4.41", "bbbbbbb", 1);
        let server = IsolatedServer::new("transaction");
        let dir = TestDir::new("transaction");
        server.start(Some(&metadata_for_current(&old_build)));
        server.new_session("alpha");
        let run = |args: &[&str]| server.tmux(args);
        let run_owned = |args: &[String]| server.tmux_owned(args);
        let handle = ServerHandle {
            run: &run,
            run_owned: &run_owned,
            owned_client: &no_owned_client,
            socket_name: &server.socket,
            lifecycle_file: dir.file(),
        };
        let Probe::Reachable(old) = probe_server_on(&handle) else {
            panic!("old server reachable");
        };
        assert_eq!(old.sessions.len(), 1);
        assert_eq!(
            compatible_state(&current, &old.metadata),
            CompatibilityState::RestartRequired
        );

        let began = Instant::now();
        let fresh =
            complete_restart_on(&handle, &current, &old, "restartCompleted").expect("restart");
        assert!(began.elapsed() < Duration::from_secs(3));
        assert_ne!(fresh.pid, old.pid);
        assert_eq!(fresh.pid, server.pid());
        assert!(fresh.sessions.is_empty());
        assert_eq!(
            compatible_state(&current, &fresh.metadata),
            CompatibilityState::CompatibleCurrentBuild
        );
        assert!(!server
            .output(&["has-session", "-t", "=alpha"])
            .status
            .success());
        assert_eq!(server.run(&["show-options", "-gv", "exit-empty"]), "off");
        let disk = read_disk_at(&dir.file());
        assert!(disk.operation.is_none(), "the transaction is closed");
        assert!(disk.deferred_build.is_none(), "a deferral is consumed");
        let notice = disk.notice.expect("notice for the next boot");
        assert_eq!(notice.code, "restartCompleted");
        assert_eq!(notice.build_key, build_key(&current));
    }

    /// A restart that fails, before or after the old server stopped, leaves
    /// its content-free intent persisted, so the next boot resumes only
    /// against the same identity; a replaced server is never killed under an
    /// old confirmation.
    #[test]
    fn real_tmux_restart_transaction_keeps_its_intent_when_it_fails() {
        let old_build = build(SourceCategory::Installed, "0.4.40", "aaaaaaa", 1);
        let current = build(SourceCategory::Development, "0.4.41", "bbbbbbb", 1);
        let server = IsolatedServer::new("phases");
        let dir = TestDir::new("phases");
        server.start(Some(&metadata_for_current(&old_build)));
        server.new_session("alpha");
        let run = |args: &[&str]| server.tmux(args);
        let run_owned = |args: &[String]| server.tmux_owned(args);
        let handle = ServerHandle {
            run: &run,
            run_owned: &run_owned,
            owned_client: &no_owned_client,
            socket_name: &server.socket,
            lifecycle_file: dir.file(),
        };
        let Probe::Reachable(old) = probe_server_on(&handle) else {
            panic!("old server reachable");
        };

        let refusing_stop = |args: &[&str]| {
            if args[0] == "kill-server" {
                Err(DeckError::new(
                    ErrorKind::Tmux,
                    "tmux kill-server failed: refused",
                ))
            } else {
                server.tmux(args)
            }
        };
        let stop_refused = ServerHandle {
            run: &refusing_stop,
            run_owned: &run_owned,
            owned_client: &no_owned_client,
            socket_name: &server.socket,
            lifecycle_file: dir.file(),
        };
        assert_eq!(
            complete_restart_on(&stop_refused, &current, &old, "restartCompleted")
                .unwrap_err()
                .message(),
            "tmux-server-stop-failed"
        );
        assert_eq!(server.pid(), old.pid, "the old server is untouched");
        let disk = read_disk_at(&dir.file());
        let intent = disk.operation.expect("intent persisted before stopping");
        assert_eq!(intent.build_key, build_key(&current));
        assert_eq!(
            (intent.old_pid, intent.old_started_at),
            (old.pid, old.started_at)
        );
        assert_eq!(
            (intent.old_socket_device, intent.old_socket_inode),
            (old.socket_device, old.socket_inode)
        );
        assert_eq!((intent.session_count, intent.pane_count), (1, 1));
        assert_eq!(intent.impact_token, old.impact_token);
        assert!(disk.notice.is_none());
        assert!(restart_intent_still_matches(&intent, &old));

        let mut replaced = (*old).clone();
        replaced.pid = old.pid.wrapping_add(1);
        assert_eq!(
            wait_for_old_server_exit_on(&handle, &replaced)
                .unwrap_err()
                .message(),
            "tmux-server-replaced-concurrently"
        );
        assert_eq!(server.pid(), old.pid);

        let refusing_start = |_: &[String]| {
            Err(DeckError::new(
                ErrorKind::Tmux,
                "tmux start-server failed: refused",
            ))
        };
        let start_refused = ServerHandle {
            run: &run,
            run_owned: &refusing_start,
            owned_client: &no_owned_client,
            socket_name: &server.socket,
            lifecycle_file: dir.file(),
        };
        assert_eq!(
            complete_restart_on(&start_refused, &current, &old, "restartCompleted")
                .unwrap_err()
                .message(),
            "tmux start-server failed: refused"
        );
        assert!(!server.is_running(), "the old server was stopped");
        assert!(!old.socket_path.exists(), "no stale socket is left behind");
        let disk = read_disk_at(&dir.file());
        assert!(disk.operation.is_some(), "intent kept for recovery");
        assert!(disk.notice.is_none());
        // The boot recovery for exactly that state: nothing stale remains.
        clean_confirmed_intent_socket_on(&handle, &intent).expect("nothing to clean");
    }

    /// Stopping the old server and recovering a confirmed restart remove a
    /// leftover socket only when it is the confirmed one (name, tmux
    /// directory, device and inode); anything else is refused and kept.
    #[test]
    fn stale_socket_cleanup_removes_only_the_confirmed_socket() {
        let seq = TEST_SOCKET_SEQ.fetch_add(1, AtomicOrdering::Relaxed);
        let name = format!("deck-test-stale-{}-{seq}", std::process::id());
        let tmux_dir = Path::new("/tmp").join(format!("tmux-{}", unsafe { libc::getuid() }));
        std::fs::create_dir_all(&tmux_dir).unwrap();
        let path = tmux_dir.join(&name);
        let _ = std::fs::remove_file(&path);
        struct RemoveOnDrop(PathBuf);
        impl Drop for RemoveOnDrop {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let _cleanup = RemoveOnDrop(path.clone());
        let listener = UnixListener::bind(&path).unwrap();
        let socket = std::fs::symlink_metadata(&path).unwrap();
        let absent = |_: &[&str]| {
            Err(DeckError::classified(
                "tmux display-message failed: no server running on /tmp/x",
            ))
        };
        let handle = ServerHandle {
            run: &absent,
            run_owned: &never_owned,
            owned_client: &no_owned_client,
            socket_name: &name,
            lifecycle_file: PathBuf::from("/dev/null"),
        };
        let mut old = snapshot(MetadataRead::Missing, Vec::new());
        old.socket_path = path.clone();
        old.socket_device = socket.dev();
        old.socket_inode = socket.ino();

        let mut foreign = old.clone();
        foreign.socket_inode = socket.ino().wrapping_add(1);
        assert_eq!(
            wait_for_old_server_exit_on(&handle, &foreign)
                .unwrap_err()
                .message(),
            "tmux-server-socket-not-safe"
        );
        assert!(path.exists(), "a foreign socket is kept");
        let mut intent = intent_for(&foreign, "current");
        assert_eq!(
            clean_confirmed_intent_socket_on(&handle, &intent)
                .unwrap_err()
                .message(),
            "tmux-server-socket-not-safe"
        );
        assert!(path.exists());

        intent.old_socket_inode = socket.ino();
        clean_confirmed_intent_socket_on(&handle, &intent).expect("confirmed socket removed");
        assert!(!path.exists());
        clean_confirmed_intent_socket_on(&handle, &intent).expect("nothing left to clean");
        drop(listener);

        let listener = UnixListener::bind(&path).unwrap();
        let socket = std::fs::symlink_metadata(&path).unwrap();
        old.socket_device = socket.dev();
        old.socket_inode = socket.ino();
        wait_for_old_server_exit_on(&handle, &old).expect("absent server, stale socket removed");
        assert!(!path.exists());
        wait_for_old_server_exit_on(&handle, &old).expect("nothing left to remove");
        drop(listener);

        // An unreadable answer is retried until the server is absent.
        let calls = std::cell::Cell::new(0u32);
        let flaky = |args: &[&str]| {
            if calls.get() == 0 {
                calls.set(1);
                return Err(DeckError::new(ErrorKind::Tmux, "tmux control timeout"));
            }
            absent(args)
        };
        let flaky_handle = ServerHandle {
            run: &flaky,
            run_owned: &never_owned,
            owned_client: &no_owned_client,
            socket_name: &name,
            lifecycle_file: PathBuf::from("/dev/null"),
        };
        let began = Instant::now();
        wait_for_old_server_exit_on(&flaky_handle, &old).expect("absent after a retry");
        assert_eq!(calls.get(), 1);
        assert!(began.elapsed() >= Duration::from_millis(100));

        let _deadline =
            crate::session_runtime::Deadline::until(Instant::now() - Duration::from_millis(1));
        assert_eq!(
            wait_for_old_server_exit_on(&handle, &old)
                .unwrap_err()
                .message(),
            "tmux-restart-timeout"
        );
    }
}
