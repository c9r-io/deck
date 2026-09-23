//! Ignored real-integration release gate for tunnel-client v0.0.14.
//!
//! This module is compiled only by `cargo test`; it adds no release CLI. The
//! Runtime API key comes only from this helper's Keychain namespace. Output is
//! deliberately limited to non-secret lifecycle facts.

use crate::identity;
use crate::keychain;
use crate::process;
use crate::secret_file::SecretFile;
use crate::tunnel_client::encode_command;
use crate::valid_client_id;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

const CLIENT_ENV: &str = "DECK_TUNNEL_LIFECYCLE_CLIENT_ID";
const TUNNEL_ENV: &str = "DECK_TUNNEL_LIFECYCLE_TUNNEL_ID";

struct Secret(Vec<u8>);
impl Drop for Secret {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

struct RuntimeCleanup {
    executable: PathBuf,
    alias: String,
    created: bool,
}
impl Drop for RuntimeCleanup {
    fn drop(&mut self) {
        if !self.created {
            return;
        }
        let _ = process::output(
            Command::new(&self.executable).args(["runtimes", "stop", &self.alias, "--json"]),
            Duration::from_secs(10),
        );
        let _ = process::output(
            Command::new(&self.executable).args(["runtimes", "rm", &self.alias, "--json"]),
            Duration::from_secs(10),
        );
    }
}

#[derive(Debug)]
struct Snapshot {
    process_running: bool,
    healthy: bool,
    ready: bool,
    stale: bool,
    runtime_state: String,
    poll_state: String,
    tmux_session: Option<String>,
    log_path: Option<PathBuf>,
    health_url_file: Option<String>,
}

struct Captures(Vec<Vec<u8>>);
impl Captures {
    fn new() -> Self {
        Self(Vec::new())
    }

    fn keep(&mut self, output: &Output) {
        self.0.push(output.stdout.clone());
        self.0.push(output.stderr.clone());
    }

    fn contains(&self, needle: &[u8]) -> bool {
        self.0.iter().any(|bytes| contains_bytes(bytes, needle))
    }
}

#[test]
#[ignore = "requires a dedicated Tunnel, restricted Runtime API key in helper Keychain, and explicit local execution"]
fn real_file_secret_lifecycle() {
    assert!(std::env::var_os("DECK_TUNNELCTL_TUNNEL_CLIENT").is_none());
    assert!(std::env::var_os("DECK_TUNNELCTL_ADAPTER_PATH").is_none());

    let client_id = std::env::var(CLIENT_ENV).expect("missing non-secret lifecycle client id");
    let tunnel_id = std::env::var(TUNNEL_ENV).expect("missing non-secret lifecycle Tunnel ID");
    assert!(valid_client_id(&client_id), "invalid lifecycle client id");
    assert!(valid_tunnel_id(&tunnel_id), "invalid lifecycle Tunnel ID");

    let executable = identity::tunnel_client().expect("supported tunnel-client is required");
    let adapter = identity::adapter().expect("signed installed deck-mcp is required");
    let alias = lifecycle_alias(&client_id);
    let mut captures = Captures::new();

    let list = run(
        &executable,
        ["runtimes", "list", "--json"],
        Duration::from_secs(5),
        &mut captures,
    );
    assert!(list.status.success(), "runtime list failed");
    assert!(
        !list_contains(&list.stdout, &alias),
        "dedicated alias already exists; refusing cleanup"
    );
    assert!(
        keychain::has(&client_id).expect("Keychain presence check failed"),
        "helper Keychain credential is absent"
    );

    println!("tunnel_client_version=0.0.14");
    println!(
        "tunnel_client_sha256=309fd85da5a8c2ca8dae920deea8ac10a4d7934ed18ac46e7df0c200139cc9c5"
    );
    println!("test_alias={alias}");
    println!("tunnel_id={tunnel_id}");
    println!("runtime_initially_present=false");
    println!("secret_file_pattern=private-random-file");

    let secret = Secret(
        keychain::get(&client_id)
            .expect("Keychain read failed")
            .expect("Keychain credential is absent"),
    );
    let mut cleanup = RuntimeCleanup {
        executable: executable.clone(),
        alias: alias.clone(),
        created: false,
    };
    let command_string = encode_command(&[
        adapter.to_string_lossy().as_ref(),
        "--client-id",
        &client_id,
    ]);
    assert!(!contains_bytes(command_string.as_bytes(), &secret.0));

    let first_file =
        SecretFile::create(&secret.0).expect("first secure secret file creation failed");
    assert_private_secret(&first_file);
    let first_path = first_file.path().to_path_buf();
    assert!(!contains_bytes(
        first_file.reference().as_bytes(),
        &secret.0
    ));
    println!("secret_file_created=true");
    // The alias was absent immediately before this point, so any local state
    // left by even a failed connect belongs to this test and may be removed.
    cleanup.created = true;
    let first_connect = connect(
        &executable,
        &alias,
        &tunnel_id,
        &command_string,
        &first_file.reference(),
        &mut captures,
    );
    assert!(
        first_connect.status.success(),
        "first runtimes connect failed"
    );
    let initial = wait_ready(&executable, &alias, &mut captures, Duration::from_secs(120));
    report("initial", &initial);

    drop(first_file);
    assert!(!first_path.exists(), "first secret file still exists");
    println!("first_secret_file_removed=true");

    let mut post_delete = Vec::new();
    // v0.0.14 reports a 30 second long-poll timeout. Four observations over
    // 120 seconds span multiple full poll deadlines without requiring traffic.
    for _ in 0..4 {
        std::thread::sleep(Duration::from_secs(30));
        let (snapshot, successful_poll) = status_with_poll(&executable, &alias, &mut captures);
        report("post_delete", &snapshot);
        assert!(
            snapshot.process_running,
            "runtime stopped after secret deletion"
        );
        assert!(
            snapshot.healthy,
            "runtime health failed after secret deletion"
        );
        assert!(
            !snapshot.stale,
            "runtime became stale after secret deletion"
        );
        assert!(successful_poll, "control-plane poll failed after deletion");
        post_delete.push(snapshot);
    }
    assert!(post_delete
        .iter()
        .all(|snapshot| snapshot.process_running && snapshot.healthy));

    let live = post_delete.last().unwrap();
    let session = live
        .tmux_session
        .as_deref()
        .expect("status did not identify the test runtime tmux session");
    assert!(
        valid_test_session(session, &alias),
        "status returned an ambiguous runtime session"
    );
    let tmux = Path::new("/opt/homebrew/bin/tmux")
        .canonicalize()
        .expect("tmux canonical path missing");
    let killed = process::output(
        Command::new(&tmux).args(["kill-session", "-t", session]),
        Duration::from_secs(5),
    )
    .expect("exact test runtime termination failed");
    captures.keep(&killed);
    assert!(
        killed.status.success(),
        "exact test runtime termination was refused"
    );
    println!("unexpected_child_termination_target_verified=true");

    let mut automatic_restart = false;
    let mut restarted = None;
    for _ in 0..6 {
        std::thread::sleep(Duration::from_secs(10));
        let (snapshot, successful_poll) = status_with_poll(&executable, &alias, &mut captures);
        if snapshot.process_running {
            automatic_restart = true;
            restarted = Some((snapshot, successful_poll));
            break;
        }
    }
    println!("automatic_restart={automatic_restart}");
    if let Some((snapshot, successful_poll)) = restarted {
        report("automatic_restart", &snapshot);
        assert!(
            snapshot.healthy && !snapshot.stale,
            "automatic restart did not recover health without the file"
        );
        assert!(
            successful_poll,
            "automatic restart did not recover control-plane polling"
        );
    }

    let stopped = run(
        &executable,
        ["runtimes", "stop", &alias, "--json"],
        Duration::from_secs(10),
        &mut captures,
    );
    assert!(stopped.status.success(), "explicit stop failed");
    let stopped_status = status(&executable, &alias, &mut captures);
    assert!(
        !stopped_status.process_running,
        "runtime still running after stop"
    );
    println!("explicit_stop_verified=true");

    assert!(
        !first_path.exists(),
        "old secret file unexpectedly returned"
    );
    let missing_ref = format!("file:{}", first_path.display());
    let old_file_connect = connect(
        &executable,
        &alias,
        &tunnel_id,
        &command_string,
        &missing_ref,
        &mut captures,
    );
    assert!(
        !old_file_connect.status.success(),
        "connect unexpectedly accepted the missing old secret file"
    );
    println!("missing_old_secret_fails_closed=true");

    let second_file =
        SecretFile::create(&secret.0).expect("second secure secret file creation failed");
    assert_private_secret(&second_file);
    let second_path = second_file.path().to_path_buf();
    assert_ne!(
        first_path, second_path,
        "explicit Start reused a secret path"
    );
    assert!(!contains_bytes(
        second_file.reference().as_bytes(),
        &secret.0
    ));
    let second_connect = connect(
        &executable,
        &alias,
        &tunnel_id,
        &command_string,
        &second_file.reference(),
        &mut captures,
    );
    assert!(
        second_connect.status.success(),
        "second runtimes connect failed"
    );
    let second_ready = wait_ready(&executable, &alias, &mut captures, Duration::from_secs(120));
    report("second_start", &second_ready);
    drop(second_file);
    assert!(!second_path.exists(), "second secret file still exists");
    std::thread::sleep(Duration::from_secs(10));
    let (second_post_delete, second_successful_poll) =
        status_with_poll(&executable, &alias, &mut captures);
    report("second_post_delete", &second_post_delete);
    assert!(
        second_post_delete.process_running
            && second_post_delete.healthy
            && !second_post_delete.stale
    );
    assert!(second_successful_poll);

    if let Some(path) = second_post_delete.log_path.as_deref() {
        assert_file_does_not_contain(path, &secret.0);
    }

    let final_stop = run(
        &executable,
        ["runtimes", "stop", &alias, "--json"],
        Duration::from_secs(10),
        &mut captures,
    );
    assert!(final_stop.status.success(), "final stop failed");
    let remove = run(
        &executable,
        ["runtimes", "rm", &alias, "--json"],
        Duration::from_secs(10),
        &mut captures,
    );
    assert!(remove.status.success(), "local runtime removal failed");
    cleanup.created = false;
    let final_list = run(
        &executable,
        ["runtimes", "list", "--json"],
        Duration::from_secs(5),
        &mut captures,
    );
    assert!(final_list.status.success() && !list_contains(&final_list.stdout, &alias));
    println!("local_runtime_removed=true");

    leakage_sweep(&secret.0, &captures, [&first_path, &second_path]);
    println!("secret_leakage_sweep=true");
}

#[test]
#[ignore = "Step C only: requires the dedicated lifecycle client, Tunnel, and helper Keychain credential"]
fn step_c_extended_readiness_diagnostic() {
    const OBSERVATION_BUDGET: Duration = Duration::from_secs(120);
    const OBSERVATION_INTERVAL: Duration = Duration::from_secs(10);

    assert!(std::env::var_os("DECK_TUNNELCTL_TUNNEL_CLIENT").is_none());
    assert!(std::env::var_os("DECK_TUNNELCTL_ADAPTER_PATH").is_none());

    let client_id = std::env::var(CLIENT_ENV).expect("missing non-secret lifecycle client id");
    let tunnel_id = std::env::var(TUNNEL_ENV).expect("missing non-secret lifecycle Tunnel ID");
    assert!(valid_client_id(&client_id), "invalid lifecycle client id");
    assert!(valid_tunnel_id(&tunnel_id), "invalid lifecycle Tunnel ID");

    let executable = identity::tunnel_client().expect("supported tunnel-client is required");
    let adapter = identity::adapter().expect("signed installed deck-mcp is required");
    let alias = lifecycle_alias(&client_id);
    let mut captures = Captures::new();
    let list = run(
        &executable,
        ["runtimes", "list", "--json"],
        Duration::from_secs(5),
        &mut captures,
    );
    assert!(list.status.success(), "runtime list failed");
    assert!(
        !list_contains(&list.stdout, &alias),
        "dedicated alias already exists; refusing cleanup"
    );
    assert!(
        keychain::has(&client_id).expect("Keychain presence check failed"),
        "helper Keychain credential is absent"
    );

    let secret = Secret(
        keychain::get(&client_id)
            .expect("Keychain read failed")
            .expect("Keychain credential is absent"),
    );
    let secret_file = SecretFile::create(&secret.0).expect("secure secret file creation failed");
    assert_private_secret(&secret_file);
    let secret_path = secret_file.path().to_path_buf();
    let command_string = encode_command(&[
        adapter.to_string_lossy().as_ref(),
        "--client-id",
        &client_id,
    ]);
    assert!(!contains_bytes(command_string.as_bytes(), &secret.0));
    assert!(!contains_bytes(
        secret_file.reference().as_bytes(),
        &secret.0
    ));

    // Declared after the secret file so unwind cleanup stops/removes the
    // runtime before SecretFile::drop removes the still-live credential file.
    let mut cleanup = RuntimeCleanup {
        executable: executable.clone(),
        alias: alias.clone(),
        created: true,
    };

    println!(
        "observation_budget_seconds={}",
        OBSERVATION_BUDGET.as_secs()
    );
    println!(
        "observation_interval_seconds={}",
        OBSERVATION_INTERVAL.as_secs()
    );
    println!("secret_file_created=true");
    let connect_started = Instant::now();
    let connect_output = connect(
        &executable,
        &alias,
        &tunnel_id,
        &command_string,
        &secret_file.reference(),
        &mut captures,
    );
    assert!(connect_output.status.success(), "runtimes connect failed");
    let observation_started = Instant::now();
    println!("connect_command_succeeded=true");
    println!(
        "connect_elapsed_ms={}",
        connect_started.elapsed().as_millis()
    );

    let mut reached_stable_ready = false;
    let mut auth_failure = false;
    let mut local_mcp_failure = false;
    let mut transport_failure = false;
    let mut remote_reported = false;
    let mut optional_endpoints_reported = false;
    let mut last_log_path = None;

    loop {
        let elapsed = observation_started.elapsed();
        let output = run(
            &executable,
            ["runtimes", "status", &alias, "--json"],
            Duration::from_secs(5),
            &mut captures,
        );
        assert!(output.status.success(), "runtime status failed");
        let value: Value = serde_json::from_slice(&output.stdout).expect("status JSON malformed");
        let current = snapshot(&output.stdout);
        last_log_path = current.log_path.clone().or(last_log_path);
        report_timeline(elapsed, &current);

        if !remote_reported {
            report_remote_metadata(&value);
            remote_reported = true;
        }

        if let Some(health_url_file) = value
            .get("health_url_file")
            .and_then(Value::as_str)
            .filter(|path| !path.is_empty())
        {
            let health = run_health_probe(&executable, health_url_file, &mut captures);
            report_component_health(elapsed, &health);
            auth_failure |= health_auth_failure(&health);
            local_mcp_failure |= health_local_mcp_failure(&health);
            transport_failure |= health_transport_failure(&health);

            if !optional_endpoints_reported {
                if let Some(base_url) = health.get("base_url").and_then(Value::as_str) {
                    report_optional_health_endpoints(base_url);
                    optional_endpoints_reported = true;
                }
            }

            let poll_ok = health
                .pointer("/control_plane_poll/ok")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            reached_stable_ready = current.process_running
                && current.healthy
                && current.ready
                && !current.stale
                && poll_ok;
        }

        if reached_stable_ready || auth_failure || local_mcp_failure {
            break;
        }
        if elapsed >= OBSERVATION_BUDGET {
            break;
        }
        std::thread::sleep(OBSERVATION_INTERVAL);
    }

    if let Some(path) = last_log_path.as_deref() {
        let log = classify_structured_log(path);
        auth_failure |= log.auth_failure;
        local_mcp_failure |= log.local_mcp_failure;
        transport_failure |= log.transport_failure;
        println!("log.auth_failure={}", log.auth_failure);
        println!("log.local_mcp_failure={}", log.local_mcp_failure);
        println!("log.transport_failure={}", log.transport_failure);
        assert_file_does_not_contain(path, &secret.0);
    }
    assert!(
        !captures.contains(&secret.0),
        "secret appeared in bounded subprocess output"
    );

    let classification = if reached_stable_ready {
        "PREVIOUS_30S_DEADLINE_WAS_TOO_SHORT"
    } else if auth_failure {
        "CONTROL_PLANE_AUTH_FAILURE"
    } else if local_mcp_failure {
        "LOCAL_MCP_RUNTIME_FAILURE"
    } else if transport_failure {
        "CONTROL_PLANE_TRANSPORT_FAILURE"
    } else {
        "READINESS_STUCK_UNKNOWN"
    };
    println!("classification={classification}");

    let stop = run(
        &executable,
        ["runtimes", "stop", &alias, "--json"],
        Duration::from_secs(10),
        &mut captures,
    );
    assert!(stop.status.success(), "Step C cleanup stop failed");
    let remove = run(
        &executable,
        ["runtimes", "rm", &alias, "--json"],
        Duration::from_secs(10),
        &mut captures,
    );
    assert!(remove.status.success(), "Step C cleanup remove failed");
    cleanup.created = false;
    drop(cleanup);
    drop(secret_file);
    assert!(!secret_path.exists(), "Step C secret file survived cleanup");
    let final_list = run(
        &executable,
        ["runtimes", "list", "--json"],
        Duration::from_secs(5),
        &mut captures,
    );
    assert!(final_list.status.success() && !list_contains(&final_list.stdout, &alias));
    println!("cleanup.test_alias_removed=true");
    println!("cleanup.temporary_secret_removed=true");

    assert_eq!(classification, "PREVIOUS_30S_DEADLINE_WAS_TOO_SHORT");
}

#[test]
#[ignore = "read-only auth split probe using the dedicated lifecycle Runtime key"]
fn runtime_key_tunnel_metadata_probe() {
    assert!(std::env::var_os("DECK_TUNNELCTL_TUNNEL_CLIENT").is_none());

    let client_id = std::env::var(CLIENT_ENV).expect("missing non-secret lifecycle client id");
    let tunnel_id = std::env::var(TUNNEL_ENV).expect("missing non-secret lifecycle Tunnel ID");
    assert!(valid_client_id(&client_id), "invalid lifecycle client id");
    assert!(valid_tunnel_id(&tunnel_id), "invalid lifecycle Tunnel ID");

    let executable = identity::tunnel_client().expect("supported tunnel-client is required");
    let secret = Secret(
        keychain::get(&client_id)
            .expect("Keychain read failed")
            .expect("Keychain credential is absent"),
    );
    let secret_text = std::str::from_utf8(&secret.0).expect("Keychain credential is not UTF-8");

    // Test-only exception: the read-only metadata command has no file:
    // credential interface. Clear the inherited environment so an admin or
    // ordinary API key cannot win credential precedence, then give only this
    // child the exact Runtime key from the helper-owned Keychain item.
    let mut command = Command::new(&executable);
    command
        .args(["admin", "--json", "tunnels", "get", &tunnel_id])
        .env_clear()
        .env("CONTROL_PLANE_API_KEY", secret_text);
    let output = process::output(&mut command, Duration::from_secs(30))
        .expect("bounded metadata probe failed to execute");

    assert!(
        !contains_bytes(&output.stdout, &secret.0) && !contains_bytes(&output.stderr, &secret.0),
        "Runtime key appeared in metadata probe output"
    );
    let diagnosis = metadata_probe_diagnosis(&output, &tunnel_id);
    println!("exit_code={}", output.status.code().unwrap_or(-1));
    println!("http_status={}", diagnosis.http_status);
    println!("normalized_error_code={}", diagnosis.error_code);
    println!(
        "metadata_lookup_success={}",
        diagnosis.metadata_lookup_success
    );
    println!("tunnel_id={}", diagnosis.tunnel_id);
    println!(
        "organization_ids={}",
        serde_json::to_string(&diagnosis.organization_ids).unwrap()
    );
    println!(
        "workspace_ids={}",
        serde_json::to_string(&diagnosis.workspace_ids).unwrap()
    );
    println!("classification={}", diagnosis.classification);
}

struct MetadataProbeDiagnosis {
    http_status: i64,
    error_code: String,
    metadata_lookup_success: bool,
    tunnel_id: String,
    organization_ids: Vec<String>,
    workspace_ids: Vec<String>,
    classification: &'static str,
}

fn metadata_probe_diagnosis(output: &Output, expected_tunnel_id: &str) -> MetadataProbeDiagnosis {
    let stdout_json = serde_json::from_slice::<Value>(&output.stdout).ok();
    let stderr_json = serde_json::from_slice::<Value>(&output.stderr).ok();
    let value = stdout_json.as_ref().or(stderr_json.as_ref());
    let http_status = value
        .and_then(find_http_status)
        .or_else(|| byte_status(&output.stderr))
        .or_else(|| byte_status(&output.stdout))
        .unwrap_or(0);
    let error_code = value
        .and_then(find_error_code)
        .unwrap_or_else(|| match http_status {
            401 => "unauthorized".to_owned(),
            403 => "forbidden".to_owned(),
            _ if output.status.success() => "none".to_owned(),
            _ => "unclassified".to_owned(),
        });

    let tunnel = stdout_json
        .as_ref()
        .and_then(|json| json.get("tunnel").or(Some(json)))
        .filter(|_| output.status.success());
    let tunnel_id = tunnel
        .and_then(|json| json.get("id").or_else(|| json.get("tunnel_id")))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let organization_ids = tunnel
        .and_then(|json| json.get("organization_ids"))
        .and_then(string_array)
        .unwrap_or_default();
    let workspace_ids = tunnel
        .and_then(|json| json.get("workspace_ids"))
        .and_then(string_array)
        .unwrap_or_default();
    let metadata_lookup_success = output.status.success() && tunnel_id == expected_tunnel_id;
    let classification = if metadata_lookup_success {
        "METADATA_GET_PASS"
    } else if http_status == 401 || is_auth_error(&error_code) {
        "RUNTIME_KEY_AUTHENTICATION_FAILURE"
    } else if http_status == 403 {
        "RUNTIME_KEY_READ_OR_ASSOCIATION_FAILURE"
    } else {
        "METADATA_GET_UNCLASSIFIED_FAILURE"
    };

    MetadataProbeDiagnosis {
        http_status,
        error_code,
        metadata_lookup_success,
        tunnel_id,
        organization_ids,
        workspace_ids,
        classification,
    }
}

fn find_http_status(value: &Value) -> Option<i64> {
    ["status", "status_code", "http_status"]
        .iter()
        .find_map(|key| value.get(key).and_then(Value::as_i64))
        .or_else(|| value.get("error").and_then(find_http_status))
}

fn find_error_code(value: &Value) -> Option<String> {
    ["code", "error_code", "type"]
        .iter()
        .find_map(|key| value.get(key).and_then(Value::as_str))
        .map(normalize_error_code)
        .or_else(|| value.get("error").and_then(find_error_code))
}

fn normalize_error_code(value: &str) -> String {
    let normalized: String = value
        .chars()
        .take(96)
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    if normalized.is_empty() {
        "unclassified".to_owned()
    } else {
        normalized
    }
}

fn byte_status(bytes: &[u8]) -> Option<i64> {
    if contains_bytes(bytes, b"401") {
        Some(401)
    } else if contains_bytes(bytes, b"403") {
        Some(403)
    } else {
        None
    }
}

fn string_array(value: &Value) -> Option<Vec<String>> {
    Some(
        value
            .as_array()?
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
    )
}

fn report_timeline(elapsed: Duration, snapshot: &Snapshot) {
    println!(
        "timeline.t_ms={}.process_running={}.healthy={}.ready={}.stale={}.control_plane_poll_health={}.status={}",
        elapsed.as_millis(),
        snapshot.process_running,
        snapshot.healthy,
        snapshot.ready,
        snapshot.stale,
        snapshot.poll_state,
        snapshot.runtime_state,
    );
}

fn run_health_probe(executable: &Path, url_file: &str, captures: &mut Captures) -> Value {
    let output = run(
        executable,
        [
            "health",
            "--url-file",
            url_file,
            "--require-control-plane-poll",
            "--json",
        ],
        Duration::from_secs(5),
        captures,
    );
    serde_json::from_slice(&output.stdout).expect("health JSON malformed")
}

fn report_component_health(elapsed: Duration, health: &Value) {
    println!(
        "component.t_ms={}.healthz_status={}.healthz_ok={}.readyz_status={}.readyz_ok={}.poll_attempted={}.successful_poll={}.poll_last_success={}",
        elapsed.as_millis(),
        json_u64(health, "/healthz/status"),
        json_bool(health, "/healthz/ok"),
        json_u64(health, "/readyz/status"),
        json_bool(health, "/readyz/ok"),
        health.get("control_plane_poll").is_some(),
        json_bool(health, "/control_plane_poll/ok"),
        json_u64(health, "/control_plane_poll/value"),
    );
}

fn report_remote_metadata(status: &Value) {
    let remote = status.get("remote").filter(|value| !value.is_null());
    println!(
        "remote.lookup_attempted={}.exists={}.organization_count={}.workspace_count={}.tenant_count={}.error_category={}",
        status
            .get("remote_lookup_attempted")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        remote.is_some(),
        remote
            .and_then(|value| value.get("organization_ids"))
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
        remote
            .and_then(|value| value.get("workspace_ids"))
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
        remote
            .and_then(|value| value.get("tenant_ids"))
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
        error_category(
            status
                .get("remote_error")
                .and_then(Value::as_str)
                .unwrap_or_default()
        ),
    );
}

fn report_optional_health_endpoints(base_url: &str) {
    for path in [
        "/readyz",
        "/health?details=true",
        "/health/control-plane",
        "/health/mcp",
    ] {
        let status = loopback_http_status(base_url, path).unwrap_or(0);
        println!("endpoint.{}.status={status}", endpoint_label(path));
    }
}

fn loopback_http_status(base_url: &str, path: &str) -> std::io::Result<u16> {
    let authority = base_url
        .strip_prefix("http://127.0.0.1:")
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "not loopback"))?;
    let port = authority
        .trim_end_matches('/')
        .parse::<u16>()
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid port"))?;
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(1))?;
    stream.set_read_timeout(Some(Duration::from_secs(1)))?;
    stream.set_write_timeout(Some(Duration::from_secs(1)))?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
    )?;
    let mut response = Vec::new();
    stream.take(65_536).read_to_end(&mut response)?;
    let first_line = response
        .split(|byte| *byte == b'\n')
        .next()
        .unwrap_or_default();
    let text = std::str::from_utf8(first_line)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid HTTP"))?;
    text.split_ascii_whitespace()
        .nth(1)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "missing status"))?
        .parse::<u16>()
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid status"))
}

fn endpoint_label(path: &str) -> &'static str {
    match path {
        "/readyz" => "readyz",
        "/health?details=true" => "health_details",
        "/health/control-plane" => "health_control_plane",
        "/health/mcp" => "health_mcp",
        _ => "unknown",
    }
}

fn json_bool(value: &Value, pointer: &str) -> bool {
    value
        .pointer(pointer)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn json_u64(value: &Value, pointer: &str) -> u64 {
    value
        .pointer(pointer)
        .and_then(Value::as_f64)
        .unwrap_or_default()
        .max(0.0) as u64
}

fn health_auth_failure(health: &Value) -> bool {
    [
        "/healthz/error",
        "/readyz/error",
        "/control_plane_poll/error",
    ]
    .iter()
    .filter_map(|pointer| health.pointer(pointer).and_then(Value::as_str))
    .any(is_auth_error)
}

fn health_local_mcp_failure(health: &Value) -> bool {
    health
        .pointer("/readyz/body")
        .and_then(Value::as_str)
        .map(|body| {
            let body = body.to_ascii_lowercase();
            body.contains("mcp probe failed") || body.contains("mcp startup wait failed")
        })
        .unwrap_or(false)
}

fn health_transport_failure(health: &Value) -> bool {
    health
        .pointer("/control_plane_poll/error")
        .and_then(Value::as_str)
        .map(|error| !error.is_empty() && !is_auth_error(error))
        .unwrap_or(false)
}

#[derive(Default)]
struct LogDiagnosis {
    auth_failure: bool,
    local_mcp_failure: bool,
    transport_failure: bool,
}

fn classify_structured_log(path: &Path) -> LogDiagnosis {
    let Ok(file) = File::open(path) else {
        return LogDiagnosis::default();
    };
    let mut result = LogDiagnosis::default();
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let status = value.get("status_code").and_then(Value::as_u64);
        let code = value
            .get("error_code")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let message = value.get("msg").and_then(Value::as_str).unwrap_or_default();
        let error = value
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or_default();
        result.auth_failure |=
            matches!(status, Some(401 | 403)) || is_auth_error(code) || is_auth_error(error);
        let lower_message = message.to_ascii_lowercase();
        result.local_mcp_failure |= lower_message.contains("mcp")
            && (lower_message.contains("failed") || lower_message.contains("error"));
        result.transport_failure |= (lower_message == "poll failed; backing off"
            || lower_message == "poll timed out; backing off")
            && !matches!(status, Some(401 | 403));
    }
    result
}

fn is_auth_error(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    value.contains("unauthorized")
        || value.contains("forbidden")
        || value.contains("permission")
        || value.contains("invalid_api_key")
        || value.contains("tunnel_use_forbidden")
}

fn error_category(value: &str) -> &'static str {
    if value.is_empty() {
        "none"
    } else if is_auth_error(value) {
        "auth"
    } else if value.to_ascii_lowercase().contains("timeout") {
        "timeout"
    } else {
        "other"
    }
}

fn run<const N: usize>(
    executable: &Path,
    args: [&str; N],
    timeout: Duration,
    captures: &mut Captures,
) -> Output {
    let output = process::output(Command::new(executable).args(args), timeout)
        .expect("bounded tunnel-client subprocess failed");
    captures.keep(&output);
    output
}

fn connect(
    executable: &Path,
    alias: &str,
    tunnel_id: &str,
    command_string: &str,
    secret_ref: &str,
    captures: &mut Captures,
) -> Output {
    let argv = [
        "runtimes",
        "connect",
        "--alias",
        alias,
        "--mcp-command",
        command_string,
        "--runtime-api-key",
        secret_ref,
        "--tunnel-id",
        tunnel_id,
        "--json",
    ];
    assert!(!argv.iter().any(|arg| arg.as_bytes().starts_with(b"sk-")));
    run(executable, argv, Duration::from_secs(30), captures)
}

fn status(executable: &Path, alias: &str, captures: &mut Captures) -> Snapshot {
    let output = run(
        executable,
        ["runtimes", "status", alias, "--json"],
        Duration::from_secs(5),
        captures,
    );
    assert!(output.status.success(), "runtime status failed");
    snapshot(&output.stdout)
}

fn status_with_poll(executable: &Path, alias: &str, captures: &mut Captures) -> (Snapshot, bool) {
    let current = status(executable, alias, captures);
    let successful_poll = current
        .health_url_file
        .as_deref()
        .map(|path| run_health_probe(executable, path, captures))
        .map(|health| json_bool(&health, "/control_plane_poll/ok"))
        .unwrap_or(false);
    (current, successful_poll)
}

fn wait_ready(
    executable: &Path,
    alias: &str,
    captures: &mut Captures,
    budget: Duration,
) -> Snapshot {
    let deadline = std::time::Instant::now() + budget;
    loop {
        let (current, successful_poll) = status_with_poll(executable, alias, captures);
        if current.process_running
            && current.healthy
            && current.ready
            && !current.stale
            && successful_poll
        {
            return current;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "runtime did not reach stable ready state"
        );
        std::thread::sleep(Duration::from_secs(2));
    }
}

fn snapshot(bytes: &[u8]) -> Snapshot {
    let value: Value = serde_json::from_slice(bytes).expect("status JSON malformed");
    let poll = value
        .get("control_plane_poll_health")
        .and_then(Value::as_object)
        .expect("control_plane_poll_health missing");
    Snapshot {
        process_running: bool_field(&value, "process_running"),
        healthy: bool_field(&value, "healthy"),
        ready: bool_field(&value, "ready"),
        stale: bool_field(&value, "stale"),
        runtime_state: string_field(&value, "runtime_state"),
        poll_state: poll
            .get("state")
            .and_then(Value::as_str)
            .expect("poll state missing")
            .to_string(),
        tmux_session: value
            .pointer("/process/session_name")
            .and_then(Value::as_str)
            .map(str::to_string),
        log_path: value
            .pointer("/process/log_path")
            .and_then(Value::as_str)
            .map(PathBuf::from),
        health_url_file: value
            .get("health_url_file")
            .and_then(Value::as_str)
            .map(str::to_owned),
    }
}

fn report(prefix: &str, snapshot: &Snapshot) {
    println!("{prefix}.process_running={}", snapshot.process_running);
    println!("{prefix}.healthy={}", snapshot.healthy);
    println!("{prefix}.ready={}", snapshot.ready);
    println!("{prefix}.stale={}", snapshot.stale);
    println!("{prefix}.runtime_state={}", snapshot.runtime_state);
    println!("{prefix}.control_plane_poll_health={}", snapshot.poll_state);
}

fn bool_field(value: &Value, name: &str) -> bool {
    value
        .get(name)
        .and_then(Value::as_bool)
        .unwrap_or_else(|| panic!("{name} missing"))
}
fn string_field(value: &Value, name: &str) -> String {
    value
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{name} missing"))
        .to_string()
}
fn list_contains(bytes: &[u8], alias: &str) -> bool {
    let value: Value = serde_json::from_slice(bytes).expect("runtime list JSON malformed");
    value
        .get("aliases")
        .and_then(Value::as_array)
        .expect("aliases missing")
        .iter()
        .any(|entry| entry.get("alias").and_then(Value::as_str) == Some(alias))
}

fn lifecycle_alias(client_id: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(b"deck-tunnelctl:file-lifecycle-test:v1\0");
    hash.update(client_id.as_bytes());
    let digest = hash.finalize();
    let short = digest[..12]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("deck-lifecycle-{short}")
}

fn valid_tunnel_id(value: &str) -> bool {
    value.starts_with("tunnel_")
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn valid_test_session(session: &str, alias: &str) -> bool {
    session.starts_with("tunnel-mcp__")
        && session.contains(alias)
        && session.len() <= 160
        && session
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn assert_private_secret(secret: &SecretFile) {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let metadata = fs::symlink_metadata(secret.path()).expect("secret metadata missing");
    assert!(metadata.is_file() && !metadata.file_type().is_symlink());
    assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    assert_eq!(metadata.uid(), unsafe { libc::geteuid() });
    assert_eq!(metadata.nlink(), 1, "secret file has a hard-link ambiguity");

    let parent = secret.path().parent().expect("secret parent missing");
    let parent_metadata = fs::symlink_metadata(parent).expect("secret parent metadata missing");
    assert!(parent_metadata.is_dir() && !parent_metadata.file_type().is_symlink());
    assert_eq!(parent_metadata.permissions().mode() & 0o777, 0o700);
    assert_eq!(parent_metadata.uid(), unsafe { libc::geteuid() });
}

fn leakage_sweep(secret: &[u8], captures: &Captures, secret_paths: [&Path; 2]) {
    assert!(
        !captures.contains(secret),
        "secret appeared in bounded subprocess output"
    );
    for path in secret_paths {
        assert!(!path.exists(), "secret path survived cleanup");
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .expect("home missing");
    for path in [home.join(".deck/app.log")] {
        if path.exists() {
            assert_file_does_not_contain(&path, secret);
        }
    }
    let diff = process::output(
        Command::new("/usr/bin/git").args(["diff", "--no-ext-diff", "--binary"]),
        Duration::from_secs(5),
    )
    .expect("repository diff scan failed");
    assert!(diff.status.success());
    assert!(
        !contains_bytes(&diff.stdout, secret) && !contains_bytes(&diff.stderr, secret),
        "secret appeared in repository diff"
    );
}

fn assert_file_does_not_contain(path: &Path, needle: &[u8]) {
    let file = File::open(path).expect("local log could not be opened for leakage scan");
    assert!(
        !reader_contains(file, needle).expect("local log leakage scan failed"),
        "secret appeared in a local log"
    );
}

fn reader_contains(mut reader: impl Read, needle: &[u8]) -> std::io::Result<bool> {
    if needle.is_empty() {
        return Ok(false);
    }
    let mut chunk = [0_u8; 8192];
    let mut overlap = Vec::with_capacity(needle.len().saturating_sub(1));
    loop {
        let read = reader.read(&mut chunk)?;
        if read == 0 {
            return Ok(false);
        }
        overlap.extend_from_slice(&chunk[..read]);
        if contains_bytes(&overlap, needle) {
            return Ok(true);
        }
        let keep = needle.len().saturating_sub(1).min(overlap.len());
        overlap.drain(..overlap.len() - keep);
    }
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_alias_is_dedicated_deterministic_and_bounded() {
        let alias = lifecycle_alias("client_test");
        assert_eq!(alias, lifecycle_alias("client_test"));
        assert_ne!(alias, crate::runtime_alias("client_test").unwrap());
        assert!(alias.starts_with("deck-lifecycle-"));
        assert_eq!(alias.len(), 39);
    }

    #[test]
    fn release_gate_records_completed_real_lifecycle_verification() {
        assert_eq!(crate::cli::require_verified_secret_lifecycle(), Ok(()));
    }

    #[test]
    fn streaming_leakage_scan_finds_matches_across_chunk_boundaries() {
        let mut bytes = vec![b'x'; 8191];
        bytes.extend_from_slice(b"secret-value");
        assert!(reader_contains(bytes.as_slice(), b"secret-value").unwrap());
        assert!(!reader_contains(bytes.as_slice(), b"not-present").unwrap());
    }
}
