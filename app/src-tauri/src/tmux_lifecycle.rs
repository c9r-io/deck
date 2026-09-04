//! Upgrade-aware lifecycle for deck's persistent tmux server.
//!
//! The GUI process is intentionally disposable; the server is not. This
//! module is the single authority that decides whether a reachable server can
//! be reused and the only place allowed to replace it.

use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock, TryLockError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::storage::{self, applog};
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

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ServerStatus {
    status: CompatibilityState,
    pending_restart: bool,
    should_prompt: bool,
    can_restart: bool,
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

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum RestartPhase {
    Stopping,
    Starting,
    Verifying,
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
    phase: RestartPhase,
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

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn lifecycle_path() -> PathBuf {
    storage::deck_dir().join(LIFECYCLE_FILE)
}

fn read_disk() -> LifecycleDisk {
    let Ok(raw) = std::fs::read_to_string(lifecycle_path()) else {
        return LifecycleDisk::default();
    };
    serde_json::from_str::<LifecycleDisk>(&raw)
        .ok()
        .filter(|disk| disk.schema_version == 1)
        .unwrap_or_default()
}

fn write_disk(disk: &LifecycleDisk) -> Result<(), String> {
    storage::create_private_dir(&storage::deck_dir())?;
    let bytes = serde_json::to_vec(disk).map_err(|_| "lifecycle-state-encode".to_string())?;
    storage::atomic_write(&lifecycle_path(), &bytes)
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
        if storage::debug_arg("--smoke-data-dir").is_some() {
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
            std::process::Command::new(tmux::tmux_bin())
                .arg("-V")
                .output()
                .ok()
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
                crate::commands::update_channel_setting()
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
    matches!(storage::err_code(error), "no-session" | "missing")
        || error.contains("no server running")
        || error.contains("no sessions")
}

fn probe_server() -> Probe {
    let head = match tmux(&[
        "display-message",
        "-p",
        "#{pid}\t#{start_time}\t#{socket_path}",
    ]) {
        Ok(value) => value,
        Err(error) if absent_error(&error) => return Probe::Absent,
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

    let metadata = match tmux(&["show-options", "-gqv", METADATA_OPTION]) {
        Ok(value) if value.trim().is_empty() => MetadataRead::Missing,
        Ok(value) => serde_json::from_str::<ServerMetadata>(value.trim())
            .map(MetadataRead::Present)
            .unwrap_or(MetadataRead::Corrupt),
        Err(_) => MetadataRead::Corrupt,
    };

    let session_listing = match tmux(&[
        "list-sessions",
        "-F",
        "#{session_name}\t#{session_attached}\t#{session_activity}",
    ]) {
        Ok(value) => value,
        Err(error) if absent_error(&error) => String::new(),
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

    let mut pane_identities = Vec::new();
    if !sessions.is_empty() {
        let panes = match tmux(&[
            "list-panes",
            "-a",
            "-F",
            "#{session_name}\t#{pane_id}\t#{pane_pid}\t#{pane_current_command}",
        ]) {
            Ok(value) => value,
            Err(_) => return Probe::Unreachable,
        };
        for line in panes.lines() {
            let mut fields = line.split('\t');
            let (Some(name), Some(pane_id), Some(pane_pid), Some(foreground), None) = (
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
            ) else {
                return Probe::Unreachable;
            };
            if !pane_id
                .strip_prefix('%')
                .is_some_and(|value| !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()))
            {
                return Probe::Unreachable;
            }
            let Ok(pane_pid) = pane_pid.parse::<u32>() else {
                return Probe::Unreachable;
            };
            let Some(session) = sessions.iter_mut().find(|session| session.name == name) else {
                return Probe::Unreachable;
            };
            session.pane_count = session.pane_count.saturating_add(1);
            if !is_shell_process(foreground) {
                session.has_foreground_process = true;
            }
            pane_identities.push((
                name.to_string(),
                pane_id.to_string(),
                pane_pid,
                foreground.to_string(),
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
    }))
}

fn is_shell_process(value: &str) -> bool {
    matches!(
        value,
        "sh" | "bash" | "zsh" | "fish" | "dash" | "ksh" | "tcsh" | "csh" | "nu"
    )
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

fn start_current_server(build: &CurrentBuildIdentity) -> Result<ServerSnapshot, String> {
    if !source_can_create(build) {
        return Err("tmux-server-source-unstable".into());
    }
    let metadata = serde_json::to_string(&metadata_for_current(build))
        .map_err(|_| "tmux-metadata-encode".to_string())?;
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
    tmux_owned(&args)?;
    tmux::init_deck_server();
    match probe_server() {
        Probe::Reachable(snapshot)
            if compatible_state(build, &snapshot.metadata)
                == CompatibilityState::CompatibleCurrentBuild =>
        {
            Ok(*snapshot)
        }
        _ => Err("tmux-server-verification-failed".into()),
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

fn clean_confirmed_intent_socket(intent: &RestartIntent) -> Result<(), String> {
    let path = Path::new("/tmp")
        .join(format!("tmux-{}", unsafe { libc::getuid() }))
        .join(tmux::socket());
    if !path.exists() {
        return Ok(());
    }
    if !safe_stale_socket(
        &path,
        tmux::socket(),
        intent.old_socket_device,
        intent.old_socket_inode,
    ) {
        return Err("tmux-server-socket-not-safe".into());
    }
    std::fs::remove_file(path).map_err(|_| "tmux-server-stale-socket".to_string())
}

fn wait_for_old_server_exit(old: &ServerSnapshot) -> Result<(), String> {
    for _ in 0..50 {
        match probe_server() {
            Probe::Absent => break,
            Probe::Reachable(snapshot)
                if snapshot.pid != old.pid || snapshot.started_at != old.started_at =>
            {
                return Err("tmux-server-replaced-concurrently".into())
            }
            Probe::Unreachable => {}
            Probe::Reachable(_) => {}
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if matches!(probe_server(), Probe::Reachable(snapshot) if snapshot.pid == old.pid && snapshot.started_at == old.started_at)
    {
        return Err("tmux-server-stop-timeout".into());
    }
    if old.socket_path.exists() {
        if !safe_stale_socket(
            &old.socket_path,
            tmux::socket(),
            old.socket_device,
            old.socket_inode,
        ) {
            return Err("tmux-server-socket-not-safe".into());
        }
        std::fs::remove_file(&old.socket_path)
            .map_err(|_| "tmux-server-stale-socket".to_string())?;
    }
    Ok(())
}

fn complete_restart(
    build: &CurrentBuildIdentity,
    old: &ServerSnapshot,
    notice_code: &str,
) -> Result<ServerSnapshot, String> {
    let key = build_key(build);
    let mut disk = read_disk();
    disk.operation = Some(RestartIntent {
        build_key: key.clone(),
        old_pid: old.pid,
        old_started_at: old.started_at,
        old_socket_device: old.socket_device,
        old_socket_inode: old.socket_inode,
        session_count: old.sessions.len() as u32,
        pane_count: old.pane_count(),
        impact_token: old.impact_token.clone(),
        phase: RestartPhase::Stopping,
    });
    write_disk(&disk)?;

    match tmux(&["kill-server"]) {
        Ok(_) => {}
        Err(error) if absent_error(&error) => {}
        Err(_) => return Err("tmux-server-stop-failed".into()),
    }
    if crate::smoke_faults::take("tmux-after-stop") {
        return Err("injected-tmux-after-stop".into());
    }
    wait_for_old_server_exit(old)?;
    if crate::smoke_faults::take("tmux-after-socket") {
        return Err("injected-tmux-after-socket".into());
    }

    disk.operation.as_mut().unwrap().phase = RestartPhase::Starting;
    write_disk(&disk)?;
    if crate::smoke_faults::take("tmux-before-start") {
        return Err("injected-tmux-before-start".into());
    }
    let fresh = start_current_server(build)?;

    disk.operation.as_mut().unwrap().phase = RestartPhase::Verifying;
    write_disk(&disk)?;
    if crate::smoke_faults::take("tmux-after-metadata") {
        return Err("injected-tmux-after-metadata".into());
    }
    if fresh.pid == old.pid
        || compatible_state(build, &fresh.metadata) != CompatibilityState::CompatibleCurrentBuild
    {
        return Err("tmux-server-verification-failed".into());
    }

    disk.operation = None;
    disk.deferred_build = None;
    disk.notice = Some(LifecycleNotice {
        code: notice_code.into(),
        build_key: key,
    });
    write_disk(&disk)?;
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
    let key = build_key(&build);
    let disk = read_disk();
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

fn try_operation() -> Result<MutexGuard<'static, ()>, String> {
    match OPERATION.try_lock() {
        Ok(guard) => Ok(guard),
        Err(TryLockError::WouldBlock) => Err("tmux-server-restart-in-progress".into()),
        Err(TryLockError::Poisoned(_)) => Err("tmux-server-lifecycle-unavailable".into()),
    }
}

/// Synchronous boot gate. It runs before the scheduler and before the webview
/// can create/attach sessions, which prevents an updater-relocated process
/// from winning the first-server race.
pub(crate) fn reconcile_on_boot() {
    let Ok(_guard) = OPERATION.lock() else {
        return;
    };
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
                            storage::err_code(&error)
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
                    storage::err_code(&error)
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
pub(crate) fn session_creation_guard() -> Result<MutexGuard<'static, ()>, String> {
    if APP_UPDATE_INSTALLING.load(Ordering::Acquire) {
        return Err("app-update-installing".into());
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
            CompatibilityState::SourceUnstable => return Err("tmux-server-source-unstable".into()),
            _ => return Err("tmux-server-restart-required".into()),
        },
        Probe::Unreachable => return Err("tmux-server-unreachable".into()),
    }
    Ok(guard)
}

/// The updater renames the running app bundle while installing. From this
/// point until process exit, the old process must never win a tmux-server
/// creation race, regardless of what path APIs report after the rename.
pub(crate) fn begin_app_update_install() -> Result<(), String> {
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
pub(crate) fn tmux_server_status() -> ServerStatus {
    status_from_probe(current_build(), probe_server())
}

#[tauri::command]
pub(crate) fn defer_tmux_restart() -> Result<ServerStatus, String> {
    let _guard = try_operation()?;
    if APP_UPDATE_INSTALLING.load(Ordering::Acquire) {
        return Err("app-update-installing".into());
    }
    let build = current_build();
    let mut disk = read_disk();
    disk.deferred_build = Some(build_key(&build));
    write_disk(&disk)?;
    applog("[tmux-lifecycle] restart deferred");
    Ok(status_from_probe(build, probe_server()))
}

#[tauri::command]
pub(crate) fn acknowledge_tmux_lifecycle_notice() -> Result<(), String> {
    let _guard = try_operation()?;
    let mut disk = read_disk();
    disk.notice = None;
    write_disk(&disk)
}

#[tauri::command]
pub(crate) fn restart_tmux_server(
    pty_state: tauri::State<'_, crate::pty::PtyState>,
    expected_pid: u32,
    expected_started_at: u64,
    expected_impact_token: String,
    expected_session_count: u32,
    expected_pane_count: u32,
    force: bool,
) -> Result<ServerStatus, String> {
    let _guard = try_operation()?;
    if APP_UPDATE_INSTALLING.load(Ordering::Acquire) {
        return Err("app-update-installing".into());
    }
    let build = current_build();
    if !source_can_create(&build) {
        return Err("tmux-server-source-unstable".into());
    }
    let snapshot = match probe_server() {
        Probe::Reachable(snapshot) => snapshot,
        Probe::Absent => {
            start_current_server(&build)?;
            return Ok(status_from_probe(build, probe_server()));
        }
        Probe::Unreachable => return Err("tmux-server-unreachable".into()),
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
        return Err("tmux-server-impact-changed".into());
    }
    pty_state.detach_all();
    complete_restart(&build, &snapshot, "restartCompleted")?;
    Ok(status_from_probe(build, probe_server()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

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

        fn stop(&self) {
            let _ = self.output(&["kill-server"]);
            for _ in 0..30 {
                if !self
                    .output(&["display-message", "-p", "#{pid}"])
                    .status
                    .success()
                {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
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
    fn restart_intent_is_content_free_and_all_phases_round_trip() {
        for phase in [
            RestartPhase::Stopping,
            RestartPhase::Starting,
            RestartPhase::Verifying,
        ] {
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
                    phase,
                }),
                notice: None,
            };
            let raw = serde_json::to_string(&disk).unwrap();
            assert!(!raw.contains("private-project"));
            assert!(!raw.contains("prompt"));
            assert!(!raw.contains("command"));
            assert!(!raw.contains("socket_path"));
            let decoded: LifecycleDisk = serde_json::from_str(&raw).unwrap();
            assert_eq!(decoded.operation.unwrap().phase, phase);
        }
    }

    #[test]
    fn interrupted_confirmation_resumes_only_for_the_same_pid_and_impact() {
        let snapshot = ServerSnapshot {
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
            phase: RestartPhase::Stopping,
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
}
