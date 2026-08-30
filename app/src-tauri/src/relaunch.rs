//! macOS updater re-entry.
//!
//! Tauri's in-process restart starts the replacement executable inside the
//! old application's process group. macOS can retain that group's responsible
//! code identity for descendants, so terminal TCP connections may be charged
//! to an executable UUID that disappeared during the update. A launchd-owned
//! helper waits for the old process to exit and asks LaunchServices to open the
//! installed bundle. The resulting GUI process has a fresh process group and
//! responsible-code boundary.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tauri::AppHandle;

use crate::storage::applog;

const HELPER_FLAG: &str = "--deck-launchd-relauncher";
const LABEL_PREFIX: &str = "io.c9r.deck.relaunch.";
const WAIT_ATTEMPTS: usize = 600;
const WAIT_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Clone, Debug, PartialEq, Eq)]
struct RelaunchTarget {
    app_bundle: PathBuf,
    preferred_executable: OsString,
    preserved_args: Vec<OsString>,
}

#[derive(Debug, PartialEq, Eq)]
struct HelperRequest {
    old_pid: u32,
    label: String,
    app_bundle: PathBuf,
    preserved_args: Vec<OsString>,
}

static TARGET: OnceLock<Option<RelaunchTarget>> = OnceLock::new();

fn app_bundle_for_executable(executable: &Path) -> Option<PathBuf> {
    let macos = executable.parent()?;
    if macos.file_name()? != "MacOS" {
        return None;
    }
    let contents = macos.parent()?;
    if contents.file_name()? != "Contents" {
        return None;
    }
    let bundle = contents.parent()?;
    if bundle.extension()? != "app" || !bundle.is_absolute() {
        return None;
    }
    Some(bundle.to_path_buf())
}

fn valid_smoke_data_dir(value: &OsStr) -> bool {
    Path::new(value).is_absolute()
}

fn valid_smoke_socket(value: &OsStr) -> bool {
    value
        .to_str()
        .is_some_and(|value| value.starts_with("deck-smoke") && value.len() <= 128)
}

/// Preserve only diagnostics and the two isolation arguments needed to test a
/// signed updater without touching the user's real data or production socket.
fn preserved_app_args(args: impl IntoIterator<Item = OsString>) -> Vec<OsString> {
    let mut args = args.into_iter();
    let _executable = args.next();
    let mut preserved = Vec::new();
    while let Some(arg) = args.next() {
        if arg == "--debug-logging" {
            preserved.push(arg);
            continue;
        }
        if arg == "--smoke-data-dir" {
            if let Some(value) = args.next().filter(|value| valid_smoke_data_dir(value)) {
                preserved.push(arg);
                preserved.push(value);
            }
            continue;
        }
        if arg == "--smoke-tmux-socket" {
            if let Some(value) = args.next().filter(|value| valid_smoke_socket(value)) {
                preserved.push(arg);
                preserved.push(value);
            }
        }
    }
    preserved
}

fn valid_preserved_args(args: &[OsString]) -> bool {
    let mut index = 0;
    while index < args.len() {
        match args[index].to_str() {
            Some("--debug-logging") => index += 1,
            Some("--smoke-data-dir")
                if args
                    .get(index + 1)
                    .is_some_and(|value| valid_smoke_data_dir(value)) =>
            {
                index += 2;
            }
            Some("--smoke-tmux-socket")
                if args
                    .get(index + 1)
                    .is_some_and(|value| valid_smoke_socket(value)) =>
            {
                index += 2;
            }
            _ => return false,
        }
    }
    true
}

fn capture_target_from(
    executable: PathBuf,
    args: impl IntoIterator<Item = OsString>,
) -> Option<RelaunchTarget> {
    Some(RelaunchTarget {
        app_bundle: app_bundle_for_executable(&executable)?,
        preferred_executable: executable.file_name()?.to_os_string(),
        preserved_args: preserved_app_args(args),
    })
}

/// Capture the original bundle path before the updater moves/replaces it.
pub(crate) fn capture_current_target() {
    TARGET.get_or_init(|| {
        std::env::current_exe()
            .ok()
            .and_then(|executable| capture_target_from(executable, std::env::args_os()))
    });
}

fn parse_helper_request(
    args: impl IntoIterator<Item = OsString>,
) -> Result<Option<HelperRequest>, ()> {
    let mut args = args.into_iter();
    let _executable = args.next();
    if args.next().as_deref() != Some(OsStr::new(HELPER_FLAG)) {
        return Ok(None);
    }
    let old_pid = args
        .next()
        .and_then(|value| value.to_str().and_then(|value| value.parse::<u32>().ok()))
        .filter(|pid| *pid > 1)
        .ok_or(())?;
    let label = args
        .next()
        .and_then(|value| value.into_string().ok())
        .filter(|value| {
            value.starts_with(LABEL_PREFIX)
                && value.len() <= 128
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
        })
        .ok_or(())?;
    let app_bundle = args
        .next()
        .map(PathBuf::from)
        .filter(|path| {
            path.is_absolute() && path.extension().is_some_and(|extension| extension == "app")
        })
        .ok_or(())?;
    let preserved_args: Vec<_> = args.collect();
    if !valid_preserved_args(&preserved_args) {
        return Err(());
    }
    Ok(Some(HelperRequest {
        old_pid,
        label,
        app_bundle,
        preserved_args,
    }))
}

#[cfg(target_os = "macos")]
fn process_exists(pid: u32) -> bool {
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(target_os = "macos"))]
fn process_exists(_pid: u32) -> bool {
    false
}

fn cleanup_job(label: &str) {
    let _ = Command::new("/bin/launchctl")
        .args(["remove", label])
        .status();
}

fn run_helper(request: HelperRequest) -> i32 {
    for _ in 0..WAIT_ATTEMPTS {
        if !process_exists(request.old_pid) {
            let mut open = Command::new("/usr/bin/open");
            open.arg("-n").arg(&request.app_bundle);
            if !request.preserved_args.is_empty() {
                open.arg("--args").args(&request.preserved_args);
            }
            let success = open.status().is_ok_and(|status| status.success());
            cleanup_job(&request.label);
            return if success { 0 } else { 1 };
        }
        std::thread::sleep(WAIT_INTERVAL);
    }
    cleanup_job(&request.label);
    1
}

/// The launchd helper uses the same signed executable but exits before Tauri,
/// storage, the instance lock, or tmux are initialized.
pub(crate) fn run_helper_from_args() -> Option<i32> {
    match parse_helper_request(std::env::args_os()) {
        Ok(Some(request)) => Some(run_helper(request)),
        Ok(None) => None,
        Err(()) => Some(2),
    }
}

fn resolved_installed_executable(target: &RelaunchTarget) -> Option<PathBuf> {
    let macos = target.app_bundle.join("Contents/MacOS");
    let preferred = macos.join(&target.preferred_executable);
    if preferred.is_file() {
        return Some(preferred);
    }
    let plist = target.app_bundle.join("Contents/Info.plist");
    let output = Command::new("/usr/bin/plutil")
        .args(["-extract", "CFBundleExecutable", "raw", "-o", "-"])
        .arg(plist)
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    let name = String::from_utf8(output.stdout).ok()?;
    let name = name.trim();
    if name.is_empty() || name.contains('/') || name.contains('\0') {
        return None;
    }
    let executable = macos.join(name);
    executable.is_file().then_some(executable)
}

fn unique_label(pid: u32) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{LABEL_PREFIX}{pid}.{nanos}")
}

fn schedule_clean_relaunch() -> Result<(), String> {
    let target = TARGET
        .get()
        .and_then(Option::as_ref)
        .ok_or_else(|| "installed application path is unavailable".to_string())?;
    let executable = resolved_installed_executable(target)
        .ok_or_else(|| "installed application executable is unavailable".to_string())?;
    let pid = std::process::id();
    let label = unique_label(pid);
    let mut command = Command::new("/bin/launchctl");
    command
        .args(["submit", "-l", &label, "--"])
        .arg(executable)
        .arg(HELPER_FLAG)
        .arg(pid.to_string())
        .arg(&label)
        .arg(&target.app_bundle)
        .args(&target.preserved_args);
    let status = command
        .status()
        .map_err(|_| "clean application relaunch could not be scheduled".to_string())?;
    if !status.success() {
        return Err("clean application relaunch could not be scheduled".into());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn process_group_needs_reentry(pid: u32, process_group: i32) -> bool {
    process_group > 1 && process_group as u32 != pid
}

#[cfg(not(target_os = "macos"))]
fn process_group_needs_reentry(_pid: u32, _process_group: i32) -> bool {
    false
}

/// Heal the first upgrade from a release that still used Tauri's in-process
/// restart. LaunchServices GUI applications normally lead their own process
/// group; inheriting another group is the observable signature of that path.
pub(crate) fn heal_inherited_process_group() -> bool {
    #[cfg(target_os = "macos")]
    let process_group = unsafe { libc::getpgrp() };
    #[cfg(not(target_os = "macos"))]
    let process_group = std::process::id() as i32;

    if !process_group_needs_reentry(std::process::id(), process_group) {
        return false;
    }
    match schedule_clean_relaunch() {
        Ok(()) => {
            applog("[update] inherited process identity detected; clean relaunch scheduled");
            true
        }
        Err(error) => {
            applog(&format!(
                "[update] clean relaunch unavailable ({})",
                crate::storage::err_code(&error)
            ));
            false
        }
    }
}

/// Future updates never use Tauri's child-process restart. The command is
/// accepted only after this process successfully installed a verified update.
#[tauri::command]
pub(crate) fn relaunch_after_update(app: AppHandle) -> Result<(), String> {
    if !crate::tmux_lifecycle::app_update_installing() {
        return Err("no installed update is awaiting relaunch".into());
    }
    schedule_clean_relaunch()?;
    applog("[update] clean relaunch scheduled");
    app.exit(0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(value: &str) -> OsString {
        OsString::from(value)
    }

    #[test]
    fn derives_only_absolute_app_bundle_layouts() {
        assert_eq!(
            app_bundle_for_executable(Path::new("/Applications/deck.app/Contents/MacOS/deck-app")),
            Some(PathBuf::from("/Applications/deck.app"))
        );
        assert_eq!(
            app_bundle_for_executable(Path::new("target/debug/deck-app")),
            None
        );
        assert_eq!(
            app_bundle_for_executable(Path::new("/tmp/deck/Contents/Other/deck-app")),
            None
        );
    }

    #[test]
    fn preserves_only_bounded_diagnostic_and_isolation_arguments() {
        let args = vec![
            os("deck"),
            os("--debug-logging"),
            os("--smoke-data-dir"),
            os("/tmp/deck smoke"),
            os("--smoke-tmux-socket"),
            os("deck-smoke-update"),
            os("--smoke-fault"),
            os("tmux-after-stop"),
            os("--untrusted"),
        ];
        assert_eq!(
            preserved_app_args(args),
            vec![
                os("--debug-logging"),
                os("--smoke-data-dir"),
                os("/tmp/deck smoke"),
                os("--smoke-tmux-socket"),
                os("deck-smoke-update")
            ]
        );
    }

    #[test]
    fn helper_arguments_are_closed_and_validated() {
        let valid = vec![
            os("deck-app"),
            os(HELPER_FLAG),
            os("123"),
            os("io.c9r.deck.relaunch.123.456"),
            os("/Applications/deck.app"),
            os("--smoke-data-dir"),
            os("/tmp/deck-smoke"),
            os("--smoke-tmux-socket"),
            os("deck-smoke-update"),
        ];
        let parsed = parse_helper_request(valid).unwrap().unwrap();
        assert_eq!(parsed.old_pid, 123);
        assert_eq!(parsed.app_bundle, PathBuf::from("/Applications/deck.app"));

        let invalid = vec![
            os("deck-app"),
            os(HELPER_FLAG),
            os("123"),
            os("io.c9r.deck.relaunch.123.456"),
            os("/Applications/deck.app"),
            os("--arbitrary"),
        ];
        assert_eq!(parse_helper_request(invalid), Err(()));
    }

    #[test]
    fn inherited_group_requires_clean_reentry() {
        assert!(!process_group_needs_reentry(500, 500));
        assert!(process_group_needs_reentry(501, 500));
        assert!(!process_group_needs_reentry(501, 0));
    }
}
