//! macOS updater re-entry.
//!
//! Tauri's in-process restart starts the replacement executable inside the
//! old application's process group. macOS can retain that group's responsible
//! code identity for descendants, so terminal TCP connections may be charged
//! to an executable UUID that disappeared during the update. Instead, the
//! exiting process detaches one short-lived waiter (this same signed
//! executable in helper mode, in its own session via `setsid`) that waits for
//! the old process to exit and asks LaunchServices to open the installed
//! bundle. LaunchServices starts the GUI process itself, so the new deck is
//! never a child of the waiter or of the replaced process and leads its own
//! process group.
//!
//! deck never registers anything with the system service manager: no
//! submitted jobs, no agent/daemon plists, no login items. Endpoint security
//! tooling treats a third-party job submission as a persistence signature, and
//! a corporate EDR alert on that path is what removed it (CLAUDE.md keeps the
//! rule; the static test forbids the strings).

use std::ffi::{OsStr, OsString};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

use tauri::AppHandle;

use crate::applog::applog;
use crate::error::{DeckError, ErrorKind};

const HELPER_FLAG: &str = "--deck-relauncher";
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

fn run_helper(request: HelperRequest) -> i32 {
    for _ in 0..WAIT_ATTEMPTS {
        if !process_exists(request.old_pid) {
            let mut open = Command::new("/usr/bin/open");
            open.arg("-n").arg(&request.app_bundle);
            if !request.preserved_args.is_empty() {
                open.arg("--args").args(&request.preserved_args);
            }
            let success = open.status().is_ok_and(|status| status.success());
            return if success { 0 } else { 1 };
        }
        std::thread::sleep(WAIT_INTERVAL);
    }
    1
}

/// The waiter uses the same signed executable but exits before Tauri,
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

/// Build the detached waiter command: the installed executable in helper
/// mode, in its own session (`setsid`), with no inherited stdio.
fn helper_command(executable: PathBuf, target: &RelaunchTarget, pid: u32) -> Command {
    let mut command = Command::new(executable);
    command
        .arg(HELPER_FLAG)
        .arg(pid.to_string())
        .arg(&target.app_bundle)
        .args(&target.preserved_args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: setsid is async-signal-safe and touches no Rust state.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command
}

fn schedule_clean_relaunch() -> Result<(), DeckError> {
    let target = TARGET.get().and_then(Option::as_ref).ok_or_else(|| {
        DeckError::new(
            ErrorKind::Other,
            "installed application path is unavailable",
        )
    })?;
    let executable = resolved_installed_executable(target).ok_or_else(|| {
        DeckError::new(
            ErrorKind::Other,
            "installed application executable is unavailable",
        )
    })?;
    let pid = std::process::id();
    // The child is deliberately not waited on: once this process exits it is
    // reparented to launchd and finishes on its own.
    helper_command(executable, target, pid)
        .spawn()
        .map(drop)
        .map_err(|_| {
            DeckError::new(
                ErrorKind::Other,
                "clean application relaunch could not be scheduled",
            )
        })
}

/// Future updates never use Tauri's child-process restart. The command is
/// accepted only after this process successfully installed a verified update.
#[tauri::command]
pub(crate) fn relaunch_after_update(app: AppHandle) -> Result<(), DeckError> {
    if !crate::tmux_lifecycle::app_update_installing() {
        return Err(DeckError::new(
            ErrorKind::Other,
            "no installed update is awaiting relaunch",
        ));
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
            os("/Applications/deck.app"),
            os("--arbitrary"),
        ];
        assert_eq!(parse_helper_request(invalid), Err(()));
    }

    #[test]
    fn preserved_arguments_reject_partial_relative_and_unbounded_values() {
        assert!(valid_smoke_data_dir(OsStr::new("/tmp/deck-smoke")));
        assert!(!valid_smoke_data_dir(OsStr::new("relative")));
        assert!(valid_smoke_socket(OsStr::new("deck-smoke-safe")));
        assert!(!valid_smoke_socket(OsStr::new("deck-dev")));
        assert!(!valid_smoke_socket(OsStr::new(&format!(
            "deck-smoke-{}",
            "x".repeat(129)
        ))));

        assert!(valid_preserved_args(&[
            os("--debug-logging"),
            os("--smoke-data-dir"),
            os("/tmp/deck-smoke"),
            os("--smoke-tmux-socket"),
            os("deck-smoke-safe"),
        ]));
        for bad in [
            vec![os("--smoke-data-dir")],
            vec![os("--smoke-data-dir"), os("relative")],
            vec![os("--smoke-tmux-socket"), os("deck-dev")],
            vec![os("--unknown")],
        ] {
            assert!(!valid_preserved_args(&bad));
        }

        let filtered = preserved_app_args([
            os("deck"),
            os("--smoke-data-dir"),
            os("relative"),
            os("--smoke-tmux-socket"),
            os("deck-dev"),
            os("--debug-logging"),
        ]);
        assert_eq!(filtered, vec![os("--debug-logging")]);
    }

    #[test]
    fn helper_parser_distinguishes_normal_launches_from_malformed_helpers() {
        assert_eq!(
            parse_helper_request([os("deck"), os("--debug-logging")]),
            Ok(None)
        );
        let base = |pid: &str, bundle: &str| vec![os("deck"), os(HELPER_FLAG), os(pid), os(bundle)];
        for bad in [
            base("0", "/Applications/deck.app"),
            base("1", "/Applications/deck.app"),
            base("nope", "/Applications/deck.app"),
            base("12", "relative.app"),
            base("12", "/Applications/deck.txt"),
        ] {
            assert_eq!(parse_helper_request(bad), Err(()));
        }
    }

    #[test]
    fn captured_and_resolved_targets_stay_inside_the_original_bundle() {
        let executable = PathBuf::from("/Applications/deck.app/Contents/MacOS/deck-app");
        let target = capture_target_from(
            executable.clone(),
            [os("deck"), os("--debug-logging"), os("--ignored")],
        )
        .unwrap();
        assert_eq!(target.app_bundle, PathBuf::from("/Applications/deck.app"));
        assert_eq!(target.preferred_executable, os("deck-app"));
        assert_eq!(target.preserved_args, vec![os("--debug-logging")]);
        assert!(capture_target_from(PathBuf::from("relative/deck"), [os("deck")]).is_none());

        let dir = std::env::temp_dir().join(format!("deck-relaunch-{}", std::process::id()));
        let bundle = dir.join("deck.app");
        let macos = bundle.join("Contents/MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        let preferred = macos.join("deck-app");
        std::fs::write(&preferred, "binary").unwrap();
        let local = RelaunchTarget {
            app_bundle: bundle,
            preferred_executable: os("deck-app"),
            preserved_args: Vec::new(),
        };
        assert_eq!(resolved_installed_executable(&local), Some(preferred));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn helper_command_is_the_installed_executable_with_closed_arguments() {
        let target = RelaunchTarget {
            app_bundle: PathBuf::from("/Applications/deck.app"),
            preferred_executable: os("deck-app"),
            preserved_args: vec![os("--debug-logging")],
        };
        let executable = PathBuf::from("/Applications/deck.app/Contents/MacOS/deck-app");
        let command = helper_command(executable.clone(), &target, 42);
        assert_eq!(command.get_program(), executable.as_os_str());
        let args: Vec<_> = command.get_args().map(OsStr::to_os_string).collect();
        assert_eq!(
            args,
            vec![
                os(HELPER_FLAG),
                os("42"),
                os("/Applications/deck.app"),
                os("--debug-logging")
            ]
        );
    }

    #[test]
    fn missing_install_state_fails_closed() {
        assert!(process_exists(std::process::id()));
        assert!(!process_exists(2_000_000_000));
        assert_eq!(
            schedule_clean_relaunch().unwrap_err(),
            "installed application path is unavailable"
        );
    }
}
