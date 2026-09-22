//! Optional closed bridge to the separately installed Deck Tunnel Helper.
//!
//! This module knows only the helper identity and protocol. It does not know
//! tunnel-client paths, versions, hashes, configuration, or credentials. A
//! missing, invalid, incompatible, or hung helper never affects MCP authority.

use crate::error::{DeckError, ErrorKind};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{ErrorKind as IoKind, Read};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const HELPER_PATH: &str = "/Applications/Deck Tunnel Helper.app/Contents/MacOS/deck-tunnelctl";
const PROTOCOL_VERSION: u32 = 1;
const OUTPUT_LIMIT: usize = 64 * 1024;

#[derive(Debug, Serialize)]
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
    modified_seconds: i64,
    modified_nanoseconds: i64,
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
    if let Err(code) = verify_protocol(&helper) {
        return protocol_unavailable(code, helper.development);
    }
    let output = match invoke(
        &helper,
        &["status", "--client-id", client_id, "--json"],
        Duration::from_secs(5),
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
    action_worker("start", client_id, Duration::from_secs(45)).await
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
    )
    .map_err(error)?;
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
    Ok(())
}

fn resolve_helper() -> Result<ResolvedHelper, &'static str> {
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os("DECK_TUNNEL_HELPER_PATH") {
        let path = PathBuf::from(path);
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
        modified_seconds: metadata.mtime(),
        modified_nanoseconds: metadata.mtime_nsec(),
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
    let result = (|| {
        let mut stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        for fd in [stdout.as_raw_fd(), stderr.as_raw_fd()] {
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
            {
                return Err("helper_io_failed");
            }
        }
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut exited = None;
        loop {
            if Instant::now() >= deadline {
                return Err("helper_timeout");
            }
            let mut caught_up = true;
            for (pipe, bytes) in [
                (&mut stdout as &mut dyn Read, &mut out),
                (&mut stderr as &mut dyn Read, &mut err),
            ] {
                let mut buffer = [0u8; 8192];
                loop {
                    match pipe.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(count) => {
                            bytes.extend_from_slice(&buffer[..count]);
                            if bytes.len() > OUTPUT_LIMIT {
                                return Err("helper_output_too_large");
                            }
                        }
                        Err(error) if error.kind() == IoKind::WouldBlock => {
                            caught_up = false;
                            break;
                        }
                        Err(error) if error.kind() == IoKind::Interrupted => continue,
                        Err(_) => return Err("helper_io_failed"),
                    }
                }
            }
            if let Some(status) = exited.filter(|_| caught_up) {
                return Ok(Output {
                    status,
                    stdout: out,
                    stderr: err,
                });
            }
            exited = child.try_wait().map_err(|_| "helper_io_failed")?;
            std::thread::sleep(Duration::from_millis(2));
        }
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
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
    fn setup_command_quoting_is_literal() {
        assert_eq!(shell_quote("a'b c"), "'a'\\''b c'");
    }

    #[test]
    fn debug_helper_protocol_is_strict_and_missing_helper_is_optional() {
        let _guard = ENV_LOCK.lock().unwrap();
        let base =
            std::env::temp_dir().join(format!("deck-helper-protocol-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir(&base).unwrap();
        let helper = base.join("helper");
        fs::write(&helper, b"#!/bin/sh\nif [ \"$1\" = protocol ]; then echo '{\"protocolVersion\":1,\"toolVersion\":\"test\",\"capabilities\":[\"status\",\"start\",\"stop\",\"setup\",\"remove\"]}'; else echo '{\"protocolVersion\":1,\"state\":\"ready\",\"runtimeAlias\":\"deck-0123456789abcdef0123456789abcdef\",\"runtimeExists\":true,\"tunnelId\":\"tunnel_test\"}'; fi\n").unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
        std::env::set_var("DECK_TUNNEL_HELPER_PATH", &helper);
        let status = helper_status("client_test");
        assert_eq!(status.helper_state, "installed");
        assert_eq!(status.tunnel_state.as_deref(), Some("ready"));
        assert!(status.runtime_exists);

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
