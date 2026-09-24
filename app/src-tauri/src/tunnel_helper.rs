//! Optional closed bridge to the separately installed Deck Tunnel Helper.
//!
//! This module knows only the helper identity and protocol. It does not know
//! tunnel-client paths, versions, hashes, configuration, or credentials. A
//! missing, invalid, incompatible, or hung helper never affects MCP authority.
//!
//! Process boundary (EDR): the only spawn is `invoke`, which runs the helper
//! at its one fixed path (or the debug-only override) after the signature,
//! no-symlink and file-identity checks, with closed argv and no shell. The
//! identity (device, inode, size, mtime, ctime) is re-compared immediately
//! before every spawn. Spawns are kept rare: the protocol handshake runs once
//! per app session per helper identity, and status queries are serialized
//! and answered from a 3-second per-client cache (invalidated by every
//! action), so a Settings render costs at most one helper run per client and
//! never a parallel burst. Waiting blocks in poll(2) on the output pipes
//! under one deadline. Deck's kill timeouts (status 12s, stop/remove 10s,
//! start 135s) sit above the helper's own budgets (10s, 8s, 120s, in
//! tools/deck-tunnelctl/src/cli.rs), so the helper normally finishes and
//! removes its temporary Runtime-key file itself.

use crate::error::{DeckError, ErrorKind};
use crate::sync::LockRecover;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{ErrorKind as IoKind, Read};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const HELPER_PATH: &str = "/Applications/Deck Tunnel Helper.app/Contents/MacOS/deck-tunnelctl";
const PROTOCOL_VERSION: u32 = 1;
const OUTPUT_LIMIT: usize = 64 * 1024;
const STATUS_CACHE_TTL: Duration = Duration::from_secs(3);

/// The helper (path + identity) whose protocol handshake already succeeded.
static VERIFIED_PROTOCOL: Mutex<Option<(PathBuf, FileIdentity)>> = Mutex::new(None);

type StatusCache = HashMap<String, (Instant, u64, PathBuf, FileIdentity, HelperStatus)>;

/// Recent status per client id. The mutex is held across the helper run, so
/// concurrent row renders run one helper at a time and reuse the result.
static STATUS_CACHE: Mutex<Option<StatusCache>> = Mutex::new(None);

/// Bumped when any action finishes; older cached statuses no longer match.
static STATUS_GENERATION: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HelperStatus {
    helper_state: &'static str,
    development_helper: bool,
    tunnel_state: Option<String>,
    runtime_exists: bool,
    runtime_alias: Option<String>,
    tunnel_id: Option<String>,
    error_code: Option<&'static str>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProtocolResponse {
    protocol_version: u32,
    tool_version: String,
    capabilities: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StatusResponse {
    protocol_version: u32,
    state: String,
    runtime_alias: String,
    runtime_exists: bool,
    tunnel_id: Option<String>,
    error_code: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ActionResponse {
    protocol_version: u32,
    ok: bool,
    state: String,
    runtime_alias: String,
    error_code: Option<String>,
}

#[derive(Clone)]
struct ResolvedHelper {
    path: PathBuf,
    development: bool,
    identity: FileIdentity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
    size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

#[tauri::command]
pub(crate) async fn tunnel_helper_status(client_id: String) -> HelperStatus {
    tauri::async_runtime::spawn_blocking(move || helper_status(&client_id))
        .await
        .unwrap_or_else(|_| unavailable("helper_error", "helper_worker_failed"))
}

fn helper_status(client_id: &str) -> HelperStatus {
    if !valid_client_id(client_id) {
        return unavailable("helper_error", "invalid_client_id");
    }
    let helper = match resolve_helper() {
        Ok(helper) => helper,
        Err(code) => return unavailable(code, code),
    };
    let generation = STATUS_GENERATION.load(Ordering::SeqCst);
    let mut cache = STATUS_CACHE.lock_or_recover();
    let cache = cache.get_or_insert_with(HashMap::new);
    cache.retain(|_, entry| entry.0.elapsed() < STATUS_CACHE_TTL);
    if let Some((_, cached_generation, path, identity, status)) = cache.get(client_id) {
        if *cached_generation == generation && *path == helper.path && *identity == helper.identity
        {
            return status.clone();
        }
    }
    let status = query_status(&helper, client_id);
    cache.insert(
        client_id.to_string(),
        (
            Instant::now(),
            generation,
            helper.path.clone(),
            helper.identity,
            status.clone(),
        ),
    );
    status
}

fn query_status(helper: &ResolvedHelper, client_id: &str) -> HelperStatus {
    if let Err(code) = verify_protocol(helper) {
        return protocol_unavailable(code, helper.development);
    }
    let output = match invoke(
        helper,
        &["status", "--client-id", client_id, "--json"],
        Duration::from_secs(12),
    ) {
        Ok(output) if output.status.success() => output,
        Ok(_) => return unavailable_with_dev("helper_error", "helper_nonzero", helper.development),
        Err(code) => return unavailable_with_dev("helper_error", code, helper.development),
    };
    let response: StatusResponse = match parse_json(&output.stdout) {
        Ok(response) => response,
        Err(code) => return unavailable_with_dev("helper_error", code, helper.development),
    };
    if response.protocol_version != PROTOCOL_VERSION
        || !valid_state(&response.state)
        || !valid_alias(&response.runtime_alias)
        || response.error_code.is_some()
    {
        return unavailable_with_dev("helper_incompatible", "helper_schema", helper.development);
    }
    HelperStatus {
        helper_state: "installed",
        development_helper: helper.development,
        tunnel_state: Some(response.state),
        runtime_exists: response.runtime_exists,
        runtime_alias: Some(response.runtime_alias),
        tunnel_id: response.tunnel_id.filter(|id| valid_tunnel_id(id)),
        error_code: None,
    }
}

#[tauri::command]
pub(crate) async fn tunnel_helper_start(client_id: String) -> Result<HelperStatus, DeckError> {
    action_worker("start", client_id, Duration::from_secs(135)).await
}

#[tauri::command]
pub(crate) async fn tunnel_helper_stop(client_id: String) -> Result<HelperStatus, DeckError> {
    action_worker("stop", client_id, Duration::from_secs(10)).await
}

#[tauri::command]
pub(crate) async fn tunnel_helper_remove(client_id: String) -> Result<HelperStatus, DeckError> {
    action_worker("remove", client_id, Duration::from_secs(10)).await
}

#[tauri::command]
pub(crate) async fn tunnel_helper_setup_command(client_id: String) -> Result<String, DeckError> {
    tauri::async_runtime::spawn_blocking(move || setup_command(&client_id))
        .await
        .map_err(|_| error("helper_worker_failed"))?
}

async fn action_worker(
    command: &'static str,
    client_id: String,
    timeout: Duration,
) -> Result<HelperStatus, DeckError> {
    tauri::async_runtime::spawn_blocking(move || action(command, &client_id, timeout))
        .await
        .map_err(|_| error("helper_worker_failed"))?
}

fn setup_command(client_id: &str) -> Result<String, DeckError> {
    if !valid_client_id(client_id) {
        return Err(error("invalid_client_id"));
    }
    let helper = resolve_helper().map_err(error)?;
    verify_protocol(&helper).map_err(error)?;
    Ok(format!(
        "{} setup --client-id {}",
        shell_quote(helper.path.to_string_lossy().as_ref()),
        shell_quote(client_id)
    ))
}

fn action(
    command: &'static str,
    client_id: &str,
    timeout: Duration,
) -> Result<HelperStatus, DeckError> {
    if !valid_client_id(client_id) {
        return Err(error("invalid_client_id"));
    }
    let helper = resolve_helper().map_err(error)?;
    verify_protocol(&helper).map_err(error)?;
    let output = invoke(
        &helper,
        &[command, "--client-id", client_id, "--json"],
        timeout,
    );
    STATUS_GENERATION.fetch_add(1, Ordering::SeqCst);
    let output = output.map_err(error)?;
    if !output.status.success() {
        return Err(error("helper_action_failed"));
    }
    let response: ActionResponse = parse_json(&output.stdout).map_err(error)?;
    if response.protocol_version != PROTOCOL_VERSION
        || !response.ok
        || !valid_state(&response.state)
        || !valid_alias(&response.runtime_alias)
        || response.error_code.is_some()
    {
        return Err(error("helper_schema"));
    }
    Ok(HelperStatus {
        helper_state: "installed",
        development_helper: helper.development,
        tunnel_state: Some(response.state),
        runtime_exists: command != "remove",
        runtime_alias: Some(response.runtime_alias),
        tunnel_id: None,
        error_code: None,
    })
}

fn verify_protocol(helper: &ResolvedHelper) -> Result<(), &'static str> {
    let verified = Some((helper.path.clone(), helper.identity));
    if *VERIFIED_PROTOCOL.lock_or_recover() == verified {
        return Ok(());
    }
    let output = invoke(helper, &["protocol", "--json"], Duration::from_secs(3))?;
    if !output.status.success() {
        return Err("helper_incompatible");
    }
    let response: ProtocolResponse = parse_json(&output.stdout)?;
    let required = ["status", "start", "stop", "setup", "remove"];
    if response.protocol_version != PROTOCOL_VERSION
        || response.tool_version.is_empty()
        || !required
            .iter()
            .all(|item| response.capabilities.iter().any(|value| value == item))
    {
        return Err("helper_incompatible");
    }
    *VERIFIED_PROTOCOL.lock_or_recover() = verified;
    Ok(())
}

fn resolve_helper() -> Result<ResolvedHelper, &'static str> {
    if let Some(path) = development_helper_path() {
        validate_file(&path)?;
        return Ok(ResolvedHelper {
            identity: file_identity(&path)?,
            path,
            development: true,
        });
    }
    let path = PathBuf::from(HELPER_PATH);
    if !path.exists() {
        return Err("helper_missing");
    }
    validate_production_helper(&path, validate_helper_signature)?;
    Ok(ResolvedHelper {
        identity: file_identity(&path)?,
        path,
        development: false,
    })
}

#[cfg(debug_assertions)]
fn development_helper_path() -> Option<PathBuf> {
    std::env::var_os("DECK_TUNNEL_HELPER_PATH").map(PathBuf::from)
}

#[cfg(not(debug_assertions))]
fn development_helper_path() -> Option<PathBuf> {
    None
}

fn validate_production_helper(
    path: &Path,
    signature: impl FnOnce(&Path) -> Result<(), &'static str>,
) -> Result<(), &'static str> {
    validate_no_symlink_components(path)?;
    validate_file(path)?;
    signature(path).map_err(|_| "helper_untrusted")
}

fn validate_no_symlink_components(path: &Path) -> Result<(), &'static str> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        let metadata = fs::symlink_metadata(&current).map_err(|_| "helper_untrusted")?;
        if metadata.file_type().is_symlink() {
            return Err("helper_untrusted");
        }
    }
    Ok(())
}

fn validate_file(path: &Path) -> Result<(), &'static str> {
    let metadata = fs::symlink_metadata(path).map_err(|_| "helper_missing")?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.permissions().mode() & 0o111 == 0
    {
        return Err("helper_untrusted");
    }
    Ok(())
}

fn file_identity(path: &Path) -> Result<FileIdentity, &'static str> {
    let metadata = fs::symlink_metadata(path).map_err(|_| "helper_untrusted")?;
    Ok(FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        size: metadata.size(),
        modified: (metadata.mtime(), metadata.mtime_nsec()),
        // A same-size overwrite can restore mtime; ctime cannot be set.
        changed: (metadata.ctime(), metadata.ctime_nsec()),
    })
}

#[cfg(target_os = "macos")]
fn validate_helper_signature(path: &Path) -> Result<(), &'static str> {
    use core_foundation::url::CFURL;
    use security_framework::os::macos::code_signing::{Flags, SecRequirement, SecStaticCode};
    use std::str::FromStr;
    let url = CFURL::from_path(path, false).ok_or("helper_untrusted")?;
    let code = SecStaticCode::from_path(&url, Flags::NONE).map_err(|_| "helper_untrusted")?;
    let requirement = SecRequirement::from_str(
        "identifier \"io.c9r.deck-tunnelctl\" and anchor apple generic and certificate leaf[subject.OU] = \"Y8ZG3D692W\"",
    )
    .map_err(|_| "helper_untrusted")?;
    code.check_validity(
        Flags::STRICT_VALIDATE | Flags::CHECK_ALL_ARCHITECTURES,
        &requirement,
    )
    .map_err(|_| "helper_untrusted")
}

#[cfg(not(target_os = "macos"))]
fn validate_helper_signature(_: &Path) -> Result<(), &'static str> {
    Err("helper_untrusted")
}

fn invoke(
    helper: &ResolvedHelper,
    args: &[&str],
    timeout: Duration,
) -> Result<Output, &'static str> {
    // Recheck the final path immediately before spawn. Static signature
    // validation plus this check narrows, but cannot eliminate, path-based
    // validate-to-exec replacement by the current macOS user.
    validate_file(&helper.path)?;
    if file_identity(&helper.path)? != helper.identity {
        return Err("helper_replaced");
    }
    bounded_output(Command::new(&helper.path).args(args), timeout)
}

fn bounded_output(command: &mut Command, timeout: Duration) -> Result<Output, &'static str> {
    let deadline = Instant::now() + timeout;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| "helper_spawn_failed")?;
    let result = collect(&mut child, deadline);
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

fn remaining(deadline: Instant) -> Result<Duration, &'static str> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Err("helper_timeout");
    }
    Ok(left)
}

/// Blocks in poll(2) until output, EOF or the deadline (no periodic wake).
/// After both pipes reach EOF the child is exiting; only that short window
/// is awaited with a capped backoff.
fn collect(child: &mut Child, deadline: Instant) -> Result<Output, &'static str> {
    let mut stdout = child.stdout.take().ok_or("helper_io_failed")?;
    let mut stderr = child.stderr.take().ok_or("helper_io_failed")?;
    let mut captured = [Vec::new(), Vec::new()];
    let mut open = [true, true];
    while open[0] || open[1] {
        let millis = remaining(deadline)?
            .as_millis()
            .clamp(1, libc::c_int::MAX as u128) as libc::c_int;
        let mut fds = [stdout.as_raw_fd(), stderr.as_raw_fd()].map(|fd| libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        });
        for (fd, open) in fds.iter_mut().zip(open) {
            if !open {
                fd.fd = -1; // poll(2) ignores negative descriptors
            }
        }
        // SAFETY: `fds` is a live array of two pollfd values.
        if unsafe { libc::poll(fds.as_mut_ptr(), 2, millis) } < 0 {
            if std::io::Error::last_os_error().kind() == IoKind::Interrupted {
                continue;
            }
            return Err("helper_io_failed");
        }
        for index in 0..2 {
            if fds[index].revents == 0 {
                continue;
            }
            let pipe: &mut dyn Read = if index == 0 { &mut stdout } else { &mut stderr };
            let mut buffer = [0u8; 8192];
            match pipe.read(&mut buffer) {
                Ok(0) => open[index] = false,
                Ok(count) => {
                    captured[index].extend_from_slice(&buffer[..count]);
                    if captured[index].len() > OUTPUT_LIMIT {
                        return Err("helper_output_too_large");
                    }
                }
                Err(error) if error.kind() == IoKind::Interrupted => {}
                Err(_) => return Err("helper_io_failed"),
            }
        }
    }
    let mut pause = Duration::from_millis(1);
    loop {
        if let Some(status) = child.try_wait().map_err(|_| "helper_io_failed")? {
            let [stdout, stderr] = captured;
            return Ok(Output {
                status,
                stdout,
                stderr,
            });
        }
        std::thread::sleep(pause.min(remaining(deadline)?));
        pause = (pause * 2).min(Duration::from_millis(50));
    }
}

fn parse_json<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, &'static str> {
    if bytes.len() > OUTPUT_LIMIT {
        return Err("helper_output_too_large");
    }
    serde_json::from_slice(bytes).map_err(|_| "helper_malformed")
}

fn valid_client_id(value: &str) -> bool {
    value.starts_with("client_")
        && value.len() > 7
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}
fn valid_alias(value: &str) -> bool {
    value.starts_with("deck-")
        && value.len() == 37
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
fn valid_tunnel_id(value: &str) -> bool {
    value.starts_with("tunnel_")
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}
fn valid_state(value: &str) -> bool {
    matches!(
        value,
        "tunnel_client_missing"
            | "not_configured"
            | "key_missing"
            | "stopped"
            | "starting"
            | "ready"
            | "unhealthy"
            | "stale"
            | "error"
    )
}
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn unavailable(helper_state: &'static str, error_code: &'static str) -> HelperStatus {
    unavailable_with_dev(helper_state, error_code, false)
}
fn unavailable_with_dev(
    helper_state: &'static str,
    error_code: &'static str,
    development: bool,
) -> HelperStatus {
    HelperStatus {
        helper_state,
        development_helper: development,
        tunnel_state: None,
        runtime_exists: false,
        runtime_alias: None,
        tunnel_id: None,
        error_code: Some(error_code),
    }
}
fn protocol_unavailable(code: &'static str, development: bool) -> HelperStatus {
    let state = if matches!(code, "helper_incompatible" | "helper_malformed") {
        "helper_incompatible"
    } else {
        "helper_error"
    };
    unavailable_with_dev(state, code, development)
}
fn error(code: &'static str) -> DeckError {
    DeckError::new(ErrorKind::Other, code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn inputs_and_wire_values_are_closed() {
        assert!(valid_client_id("client_abc-123_DEF"));
        assert!(!valid_client_id("client_a;rm"));
        assert!(valid_alias("deck-0123456789abcdef0123456789abcdef"));
        assert!(!valid_alias("deck-user-input"));
        assert!(valid_state("ready"));
        assert!(!valid_state("connected_to_chatgpt"));
    }

    #[test]
    fn helper_validation_rejects_symlinks_and_non_regular_files() {
        let base = std::env::temp_dir().join(format!("deck-helper-policy-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir(&base).unwrap();
        let target = base.join("target");
        fs::write(&target, b"fixture").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(validate_file(&target).is_ok());
        let link = base.join("link");
        symlink(&target, &link).unwrap();
        assert_eq!(validate_file(&link), Err("helper_untrusted"));
        assert_eq!(validate_file(&base), Err("helper_untrusted"));
        assert_eq!(
            validate_production_helper(&target, |_| Err("wrong_identity")),
            Err("helper_untrusted")
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn strict_json_rejects_unknown_fields_and_oversize() {
        let valid = br#"{"protocolVersion":1,"toolVersion":"0.1","capabilities":["status"]}"#;
        assert!(parse_json::<ProtocolResponse>(valid).is_ok());
        let unknown =
            br#"{"protocolVersion":1,"toolVersion":"0.1","capabilities":[],"secret":"x"}"#;
        assert!(parse_json::<ProtocolResponse>(unknown).is_err());
        assert_eq!(
            parse_json::<ProtocolResponse>(&vec![b'x'; OUTPUT_LIMIT + 1]).unwrap_err(),
            "helper_output_too_large"
        );
    }

    #[test]
    fn same_size_overwrite_with_restored_mtime_changes_identity() {
        let base = std::env::temp_dir().join(format!("deck-helper-ctime-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir(&base).unwrap();
        let target = base.join("helper");
        fs::write(&target, b"fixture-a").unwrap();
        let before = file_identity(&target).unwrap();
        let modified = fs::metadata(&target).unwrap().modified().unwrap();
        std::thread::sleep(Duration::from_millis(5));
        fs::write(&target, b"fixture-b").unwrap();
        fs::File::options()
            .write(true)
            .open(&target)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        let after = file_identity(&target).unwrap();
        assert_eq!(after.modified, before.modified);
        assert_ne!(after, before);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn setup_command_quoting_is_literal() {
        assert_eq!(shell_quote("a'b c"), "'a'\\''b c'");
    }

    #[cfg(debug_assertions)]
    #[test]
    fn debug_helper_protocol_is_strict_and_missing_helper_is_optional() {
        let _guard = ENV_LOCK.lock().unwrap();
        let base =
            std::env::temp_dir().join(format!("deck-helper-protocol-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir(&base).unwrap();
        let helper = base.join("helper");
        let log = base.join("calls.log");
        fs::write(&helper, format!("#!/bin/sh\necho \"$1\" >> '{}'\nif [ \"$1\" = protocol ]; then echo '{{\"protocolVersion\":1,\"toolVersion\":\"test\",\"capabilities\":[\"status\",\"start\",\"stop\",\"setup\",\"remove\"]}}'; else echo '{{\"protocolVersion\":1,\"state\":\"ready\",\"runtimeAlias\":\"deck-0123456789abcdef0123456789abcdef\",\"runtimeExists\":true,\"tunnelId\":\"tunnel_test\"}}'; fi\n", log.display())).unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
        std::env::set_var("DECK_TUNNEL_HELPER_PATH", &helper);
        let status = helper_status("client_test");
        assert_eq!(status.helper_state, "installed");
        assert_eq!(status.tunnel_state.as_deref(), Some("ready"));
        assert!(status.runtime_exists);
        // A second render reuses the cached status; after an action only the
        // status is re-queried, never the per-session protocol handshake.
        assert_eq!(helper_status("client_test").helper_state, "installed");
        assert_eq!(fs::read_to_string(&log).unwrap(), "protocol\nstatus\n");
        STATUS_GENERATION.fetch_add(1, Ordering::SeqCst);
        assert_eq!(helper_status("client_test").helper_state, "installed");
        assert_eq!(
            fs::read_to_string(&log).unwrap(),
            "protocol\nstatus\nstatus\n"
        );

        fs::write(&helper, b"#!/bin/sh\necho '{\"protocolVersion\":99,\"toolVersion\":\"test\",\"capabilities\":[]}'\n").unwrap();
        let incompatible = helper_status("client_test");
        assert_eq!(incompatible.helper_state, "helper_incompatible");

        fs::write(&helper, b"#!/bin/sh\necho 'not-json'\n").unwrap();
        let malformed = helper_status("client_test");
        assert_eq!(malformed.helper_state, "helper_incompatible");

        fs::write(&helper, b"#!/bin/sh\nexit 7\n").unwrap();
        let nonzero = helper_status("client_test");
        assert_eq!(nonzero.helper_state, "helper_incompatible");

        std::env::set_var("DECK_TUNNEL_HELPER_PATH", base.join("missing"));
        let missing = helper_status("client_test");
        assert_eq!(missing.helper_state, "helper_missing");
        std::env::remove_var("DECK_TUNNEL_HELPER_PATH");
        fs::remove_dir_all(base).unwrap();
    }

    #[cfg(not(debug_assertions))]
    #[test]
    fn release_build_ignores_development_helper_override() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("DECK_TUNNEL_HELPER_PATH", "/tmp/untrusted-helper");
        assert!(development_helper_path().is_none());
        std::env::remove_var("DECK_TUNNEL_HELPER_PATH");
    }

    #[cfg(all(not(debug_assertions), target_os = "macos"))]
    #[test]
    #[ignore = "requires an explicitly installed Phase C helper at the fixed production path"]
    fn release_fixed_path_real_identity_gate() {
        let expected = std::env::var("DECK_HELPER_PHASE_C_EXPECT")
            .expect("set DECK_HELPER_PHASE_C_EXPECT to trusted, untrusted, or missing");
        assert!(development_helper_path().is_none());
        let client_id = std::env::var("DECK_HELPER_PHASE_C_CLIENT_ID")
            .unwrap_or_else(|_| "client_phase_c_identity_gate".to_string());
        let status = helper_status(&client_id);
        match expected.as_str() {
            "trusted" => {
                assert_eq!(status.helper_state, "installed");
                assert!(!status.development_helper);
                assert!(status.tunnel_state.is_some());
            }
            "untrusted" => assert_eq!(status.helper_state, "helper_untrusted"),
            "missing" => assert_eq!(status.helper_state, "helper_missing"),
            _ => panic!("unsupported Phase C identity expectation"),
        }
    }

    #[test]
    fn wire_value_filters_and_unavailable_shapes_are_closed() {
        assert!(valid_tunnel_id("tunnel_abc-123_X"));
        for id in [
            "tunnel_a b",
            "tun_x",
            &format!("tunnel_{}", "x".repeat(122)),
        ] {
            assert!(!valid_tunnel_id(id), "{id:?}");
        }
        assert!(!valid_client_id("client_"));
        assert!(!valid_client_id(&format!("client_{}", "a".repeat(122))));
        assert!(!valid_alias("deck-0123456789ABCDEF0123456789abcdef"));

        let incompatible = protocol_unavailable("helper_malformed", true);
        assert_eq!(incompatible.helper_state, "helper_incompatible");
        assert!(incompatible.development_helper);
        assert_eq!(incompatible.error_code, Some("helper_malformed"));
        let errored = protocol_unavailable("helper_timeout", false);
        assert_eq!(errored.helper_state, "helper_error");
        assert_eq!(errored.error_code, Some("helper_timeout"));
        let wire = serde_json::to_value(unavailable("helper_missing", "helper_missing")).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({
                "helperState":"helper_missing","developmentHelper":false,"tunnelState":null,
                "runtimeExists":false,"runtimeAlias":null,"tunnelId":null,
                "errorCode":"helper_missing"
            })
        );
        assert_eq!(error("helper_schema").message(), "helper_schema");
        assert_eq!(error("helper_schema").kind(), ErrorKind::Other);
        assert_eq!(remaining(Instant::now()), Err("helper_timeout"));
        assert!(remaining(Instant::now() + Duration::from_secs(60)).is_ok());
    }

    #[test]
    fn commands_refuse_an_invalid_client_id_before_looking_for_the_helper() {
        let status = tauri::async_runtime::block_on(tunnel_helper_status("client;x".into()));
        assert_eq!(status.helper_state, "helper_error");
        assert_eq!(status.error_code, Some("invalid_client_id"));
        assert!(status.tunnel_state.is_none());
        for result in [
            tauri::async_runtime::block_on(tunnel_helper_start("nope".into())),
            tauri::async_runtime::block_on(tunnel_helper_stop("nope".into())),
            tauri::async_runtime::block_on(tunnel_helper_remove("nope".into())),
        ] {
            assert_eq!(result.err().unwrap().message(), "invalid_client_id");
        }
        assert_eq!(
            tauri::async_runtime::block_on(tunnel_helper_setup_command("nope".into()))
                .unwrap_err()
                .message(),
            "invalid_client_id"
        );
    }

    #[test]
    fn helper_identity_and_path_are_rechecked_before_every_spawn() {
        let base =
            std::env::temp_dir().join(format!("deck-helper-identity-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("real")).unwrap();
        // The temp dir itself sits behind a symlink on macOS (/var).
        let base = fs::canonicalize(&base).unwrap();
        let helper = base.join("real/helper");
        fs::write(&helper, b"#!/bin/sh\necho '{}'\n").unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(validate_no_symlink_components(&helper).is_ok());
        symlink(base.join("real"), base.join("alias")).unwrap();
        assert_eq!(
            validate_no_symlink_components(&base.join("alias/helper")),
            Err("helper_untrusted"),
            "a symlinked directory on the way is untrusted"
        );
        assert_eq!(
            validate_no_symlink_components(&base.join("absent/helper")),
            Err("helper_untrusted")
        );

        let resolved = ResolvedHelper {
            identity: file_identity(&helper).unwrap(),
            path: helper.clone(),
            development: true,
        };
        let output = invoke(&resolved, &["protocol"], Duration::from_secs(3)).unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"{}\n");
        std::thread::sleep(Duration::from_millis(5));
        fs::write(&helper, b"#!/bin/sh\necho '{\"replaced\":true}'\n").unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            invoke(&resolved, &["protocol"], Duration::from_secs(3)).unwrap_err(),
            "helper_replaced",
            "a rewritten helper is not run under the verified identity"
        );
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            invoke(&resolved, &["protocol"], Duration::from_secs(3)).unwrap_err(),
            "helper_untrusted"
        );
        fs::remove_file(&helper).unwrap();
        assert_eq!(
            invoke(&resolved, &["protocol"], Duration::from_secs(3)).unwrap_err(),
            "helper_missing"
        );
        assert_eq!(file_identity(&helper), Err("helper_untrusted"));
        fs::remove_dir_all(base).unwrap();
    }

    /// A debug helper whose `protocol` answer is fixed and whose other
    /// answers come from `reply` / `code` files, so the helper file (and
    /// its verified identity) never changes between scenarios.
    #[cfg(debug_assertions)]
    fn scripted_helper(tag: &str) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("deck-helper-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir(&base).unwrap();
        let helper = base.join("helper");
        let reply = base.join("reply");
        let code = base.join("code");
        fs::write(&helper, format!("#!/bin/sh\nif [ \"$1\" = protocol ]; then echo '{{\"protocolVersion\":1,\"toolVersion\":\"test\",\"capabilities\":[\"status\",\"start\",\"stop\",\"setup\",\"remove\"]}}'; exit 0; fi\ncat '{}'\nexit $(cat '{}')\n", reply.display(), code.display())).unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(&code, b"0").unwrap();
        (base, helper, reply, code)
    }

    #[cfg(debug_assertions)]
    #[test]
    fn debug_helper_actions_report_closed_state_and_invalidate_cached_status() {
        let _guard = ENV_LOCK.lock().unwrap();
        let (base, helper, reply, code) = scripted_helper("actions");
        std::env::set_var("DECK_TUNNEL_HELPER_PATH", &helper);
        let alias = "deck-0123456789abcdef0123456789abcdef";

        fs::write(&reply, format!("{{\"protocolVersion\":1,\"ok\":true,\"state\":\"ready\",\"runtimeAlias\":\"{alias}\",\"errorCode\":null}}")).unwrap();
        let before = STATUS_GENERATION.load(Ordering::SeqCst);
        let started = action("start", "client_act", Duration::from_secs(5)).unwrap();
        assert_eq!(started.helper_state, "installed");
        assert!(started.development_helper);
        assert_eq!(started.tunnel_state.as_deref(), Some("ready"));
        assert!(started.runtime_exists);
        assert_eq!(started.runtime_alias.as_deref(), Some(alias));
        assert!(started.tunnel_id.is_none());
        assert_eq!(
            STATUS_GENERATION.load(Ordering::SeqCst),
            before + 1,
            "an action invalidates every cached status"
        );
        fs::write(&reply, format!("{{\"protocolVersion\":1,\"ok\":true,\"state\":\"not_configured\",\"runtimeAlias\":\"{alias}\",\"errorCode\":null}}")).unwrap();
        let removed = action("remove", "client_act", Duration::from_secs(5)).unwrap();
        assert!(!removed.runtime_exists);
        assert_eq!(removed.tunnel_state.as_deref(), Some("not_configured"));
        assert_eq!(
            setup_command("client_act").unwrap(),
            format!("'{}' setup --client-id 'client_act'", helper.display())
        );

        fs::write(&reply, format!("{{\"protocolVersion\":1,\"ok\":false,\"state\":\"error\",\"runtimeAlias\":\"{alias}\",\"errorCode\":null}}")).unwrap();
        assert_eq!(
            action("stop", "client_act", Duration::from_secs(5))
                .unwrap_err()
                .message(),
            "helper_schema"
        );
        fs::write(&reply, b"not json").unwrap();
        assert_eq!(
            action("stop", "client_act", Duration::from_secs(5))
                .unwrap_err()
                .message(),
            "helper_malformed"
        );
        fs::write(&code, b"3").unwrap();
        assert_eq!(
            action("stop", "client_act", Duration::from_secs(5))
                .unwrap_err()
                .message(),
            "helper_action_failed"
        );

        // Status: a non-zero helper is an error (not incompatible), a schema
        // violation is incompatible, and a foreign tunnel id is dropped.
        let nonzero = helper_status("client_nonzero");
        assert_eq!(nonzero.helper_state, "helper_error");
        assert_eq!(nonzero.error_code, Some("helper_nonzero"));
        fs::write(&code, b"0").unwrap();
        fs::write(&reply, format!("{{\"protocolVersion\":1,\"state\":\"ready\",\"runtimeAlias\":\"{alias}\",\"runtimeExists\":true,\"tunnelId\":\"bogus id\",\"errorCode\":\"boom\"}}")).unwrap();
        let schema = helper_status("client_schema");
        assert_eq!(schema.helper_state, "helper_incompatible");
        assert_eq!(schema.error_code, Some("helper_schema"));
        fs::write(&reply, format!("{{\"protocolVersion\":1,\"state\":\"ready\",\"runtimeAlias\":\"{alias}\",\"runtimeExists\":false,\"tunnelId\":\"bogus id\",\"errorCode\":null}}")).unwrap();
        let foreign = helper_status("client_foreign");
        assert_eq!(foreign.helper_state, "installed");
        assert!(!foreign.runtime_exists);
        assert!(
            foreign.tunnel_id.is_none(),
            "an invalid tunnel id is dropped"
        );

        // Missing and non-executable helpers are reported without a spawn.
        std::env::set_var("DECK_TUNNEL_HELPER_PATH", base.join("absent"));
        assert_eq!(
            action("start", "client_act", Duration::from_secs(5))
                .unwrap_err()
                .message(),
            "helper_missing"
        );
        assert_eq!(
            setup_command("client_act").unwrap_err().message(),
            "helper_missing"
        );
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o600)).unwrap();
        std::env::set_var("DECK_TUNNEL_HELPER_PATH", &helper);
        assert_eq!(
            helper_status("client_plain").helper_state,
            "helper_untrusted"
        );
        std::env::remove_var("DECK_TUNNEL_HELPER_PATH");
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn helper_timeout_and_large_output_are_bounded_and_killed() {
        let started = Instant::now();
        assert_eq!(
            bounded_output(
                Command::new("/bin/sleep").arg("10"),
                Duration::from_millis(40)
            )
            .unwrap_err(),
            "helper_timeout"
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(
            bounded_output(&mut Command::new("/usr/bin/yes"), Duration::from_secs(1)).unwrap_err(),
            "helper_output_too_large"
        );
    }
}
