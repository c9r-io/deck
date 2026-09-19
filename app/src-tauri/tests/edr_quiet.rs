//! EDR-quiet tripwires. A corporate EDR once flagged deck and IT demanded the
//! app be stopped, so the process surface is a closed allowlist enforced here
//! over PRODUCTION source (unit-test regions are cut off before scanning):
//!
//! 1. every literal `Command::new(...)` names a fixed, low-frequency system
//!    tool; the only computed executables are the bundled tmux and the
//!    relaunch waiter (deck's own installed bundle);
//! 2. nothing touches launchd, login items or `~/.deck/bin`;
//! 3. the shell-restore path carries no script, shell argv or deck-as-pane
//!    bootstrap (`commands::restore_start_args` pins the positive shape).
//!
//! Behavioural facts (libproc process facts, `localtime_r`, the tmux
//! sequences) are unit-tested in their modules; this file only guards the
//! spawn vocabulary from growing without a review.

use std::path::{Path, PathBuf};

fn manifest(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// (relative path, contents) of every backend module, recursively.
fn all_sources() -> Vec<(String, String)> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).expect("src") {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, root, out);
            } else if path.extension().is_some_and(|x| x == "rs") {
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                out.push((rel, std::fs::read_to_string(&path).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    walk(&manifest("src"), &manifest("src"), &mut out);
    assert!(out.len() >= 20, "all backend modules found: {}", out.len());
    out.sort();
    out
}

/// The same files with their `#[cfg(test)] mod tests` tail removed and the
/// dedicated test modules dropped, so test-only spawns (a `touch` to age a
/// fixture, a `python3` flock probe) never count as production surface.
fn production_sources() -> Vec<(String, String)> {
    all_sources()
        .into_iter()
        .filter(|(name, _)| !name.ends_with("/tests.rs"))
        .map(|(name, text)| {
            let cut = text
                .find("#[cfg(test)]\nmod tests")
                .map(|at| text[..at].to_string())
                .unwrap_or(text);
            (name, cut)
        })
        .collect()
}

/// Fixed-argument system tools deck may spawn, at reviewed module boundaries.
/// Keeping the file in this key means a new `open` in an unrelated module is
/// not silently accepted merely because another feature already uses it.
const ALLOWED_LITERAL_SITES: &[(&str, &str)] = &[
    ("links.rs", "open"),
    ("links.rs", "/usr/bin/open"),
    ("diagnostics.rs", "open"),
    ("diagnostics.rs", "sw_vers"),
    ("diagnostics.rs", "uname"),
    ("relaunch.rs", "/usr/bin/open"),
    ("relaunch.rs", "/usr/bin/plutil"),
    ("commands.rs", "/usr/bin/pbcopy"),
    ("inbound.rs", "open"),
    ("inbound_channel.rs", "open"),
];

/// Computed executables, each reviewed: the bundled tmux sidecar and the
/// relaunch waiter (the installed deck bundle itself, in helper mode).
const ALLOWED_EXPRESSIONS: &[(&str, &str)] = &[
    // tmux_program() is the one gate: it resolves ONLY the sidecar inside
    // this build's bundle, never Homebrew/MacPorts and never a PATH lookup
    ("tmux.rs", "tmux_program()?"),
    ("tmux.rs", "tmux_sidecar"),
    ("commands.rs", "tmux_sidecar"),
    ("tmux_lifecycle.rs", "tmux_sidecar"),
    ("tmux_lifecycle.rs", "&self.binary"),
    ("relaunch.rs", "executable"),
];

/// Debug-only smoke instrumentation may read the pasteboard back; it is
/// compiled into isolated smoke builds only.
const DEBUG_ONLY_LITERALS: &[(&str, &str)] = &[("smoke_faults.rs", "pbpaste")];

const EXPECTED_COMMAND_SITES: usize = 22;

fn call_argument(rest: &str) -> &str {
    let mut depth = 0usize;
    let end = rest
        .char_indices()
        .find_map(|(index, character)| match character {
            '(' => {
                depth += 1;
                None
            }
            ')' if depth == 0 => Some(index),
            ')' => {
                depth -= 1;
                None
            }
            _ => None,
        })
        .expect("balanced process constructor argument");
    rest[..end].trim()
}

#[test]
fn native_speech_is_in_process_local_and_content_free() {
    let swift = std::fs::read_to_string(manifest("native/SpeechBridge.swift")).unwrap();
    for forbidden in [
        "Process(",
        "NSTask",
        "URLSession",
        "AVAudioFile",
        "print(",
        "NSLog(",
        "launchctl",
        "osascript",
    ] {
        assert!(
            !swift.contains(forbidden),
            "native Speech must not introduce {forbidden}"
        );
    }
    assert!(swift.contains("recognizer.supportsOnDeviceRecognition"));
    assert!(swift.contains("request.requiresOnDeviceRecognition = true"));
    assert!(swift.contains("engine.inputNode.removeTap(onBus: 0)"));
    let config = std::fs::read_to_string(manifest("tauri.conf.json")).unwrap();
    assert!(config.contains("Entitlements.plist"));
    let entitlement = std::fs::read_to_string(manifest("Entitlements.plist")).unwrap();
    assert!(entitlement.contains("com.apple.security.device.audio-input"));
    for forbidden in [
        "disable-library-validation",
        "allow-unsigned-executable-memory",
        "allow-jit",
    ] {
        assert!(!entitlement.contains(forbidden));
    }
}

#[test]
fn every_production_spawn_is_on_the_allowlist() {
    let mut seen = 0;
    for (name, src) in production_sources() {
        for (at, _) in src.match_indices("Command::new(") {
            seen += 1;
            let rest = &src[at + "Command::new(".len()..];
            let arg = call_argument(rest);
            if let Some(literal) = arg.strip_prefix('"').and_then(|a| a.strip_suffix('"')) {
                let debug = DEBUG_ONLY_LITERALS.contains(&(name.as_str(), literal));
                assert!(
                    ALLOWED_LITERAL_SITES
                        .iter()
                        .any(|(file, tool)| name.ends_with(file) && *tool == literal)
                        || debug,
                    "{name}: spawns {literal:?} at an unreviewed call site"
                );
            } else {
                assert!(
                    ALLOWED_EXPRESSIONS
                        .iter()
                        .any(|(file, expr)| name.ends_with(file) && *expr == arg),
                    "{name}: spawns a computed executable {arg:?} that has not been reviewed"
                );
            }
        }
    }
    assert_eq!(
        seen, EXPECTED_COMMAND_SITES,
        "the production process surface changed and needs EDR review"
    );
}

#[test]
fn pty_launches_only_the_reviewed_bundled_tmux() {
    let mut sites = Vec::new();
    for (name, src) in production_sources() {
        for (at, _) in src.match_indices("CommandBuilder::new(") {
            let rest = &src[at + "CommandBuilder::new(".len()..];
            sites.push((name.clone(), call_argument(rest).to_string()));
        }
    }
    assert_eq!(sites, vec![("pty.rs".into(), "tmux_program()?".into())]);
}

#[test]
fn bundled_status_helper_has_no_process_or_persistence_surface() {
    let helper = std::fs::read_to_string(manifest("status-helper/src/main.rs")).unwrap();
    for forbidden in [
        "Command::new(",
        "CommandBuilder::new(",
        "Process(",
        "NSTask",
        "launchctl",
        "LaunchAgents",
        "LaunchDaemons",
        "SMAppService",
    ] {
        assert!(
            !helper.contains(forbidden),
            "status helper introduced {forbidden}"
        );
    }
}

#[test]
fn nothing_touches_launchd_login_items_or_a_home_executable() {
    for (name, src) in all_sources() {
        for token in ["launchctl", "LaunchAgents", "LaunchDaemons", "SMAppService"] {
            assert!(
                !src.contains(token),
                "{name}: {token} — deck never registers with launchd"
            );
        }
        assert!(
            !src.contains("fn install_helper_binary"),
            "{name}: deck never drops an executable under the home directory"
        );
    }
    for (name, src) in production_sources() {
        for spawn in [
            "\"ps\"",
            "\"date\"",
            "\"osascript\"",
            "\"sh\"",
            "\"bash\"",
            "\"zsh\"",
        ] {
            assert!(
                !src.contains(&format!("Command::new({spawn})")),
                "{name}: process facts come from libproc/sysctl; no shell or AppleScript"
            );
        }
    }
}

#[test]
fn the_restore_path_has_no_script_shell_argv_or_deck_bootstrap() {
    let restore: String = production_sources()
        .into_iter()
        .filter(|(name, _)| ["commands.rs", "shell_state.rs", "main.rs"].contains(&name.as_str()))
        .map(|(_, text)| text)
        .collect();
    for token in [
        "shell_restore",
        "RESTORE_SCRIPT",
        "\"/bin/sh\"",
        "\"-sh\"",
        "login_shell",
        "BOOTSTRAP_ARG",
        "--deck-shell-bootstrap",
        "maybe_run_bootstrap",
        "bootstrap.executable",
        "bootstrap.payload",
        "recovered_prefixes",
        "merge_transcripts",
    ] {
        assert!(!restore.contains(token), "restore path regressed: {token}");
    }
}
