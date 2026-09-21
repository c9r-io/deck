//! EDR-quiet tripwires. A corporate EDR once flagged deck and IT demanded the
//! app be stopped, so the process surface is a closed allowlist enforced here
//! over PRODUCTION source (only a file's trailing `#[cfg(test)] mod tests`
//! and dedicated `*/tests.rs` files declared `#[cfg(test)]` are left out):
//!
//! 1. every `Command::new(...)` (whitespace-tolerant) names a fixed,
//!    low-frequency system tool at a reviewed file, or a reviewed computed
//!    executable at an exact file AND enclosing function; every allowlist
//!    entry must still be used, and the total site count is pinned;
//! 2. aliases and lower-level spawns (`Command as`, `type X = Command`,
//!    `posix_spawn`, `libc::exec*`, `libc::system`, ...) are forbidden, as
//!    are tmux verbs that run a shell (`run-shell`, `pipe-pane`,
//!    `display-popup`/`popup`) and any `if-shell` without `-F`;
//! 3. deck-app never constructs a shell path at runtime (`"/bin"`,
//!    `join("zsh")`, ...): the ONE shell spawn is the MCP runner's literal
//!    `/bin/zsh` inside `spawn_job`, for a locally approved trusted-host job;
//! 4. nothing touches launchd, login items or `~/.deck/bin`;
//! 5. the shell-restore path carries no script, shell argv or deck-as-pane
//!    bootstrap (`commands::restore_start_args` pins the positive shape).
//!
//! The scanner is a pure function over `(relative path, source)`, so the
//! negative tests below feed it in-memory samples. It is a review tripwire,
//! not a sandbox: it keeps the spawn vocabulary from growing unreviewed.
//! Behavioural facts (libproc process facts, `localtime_r`, the tmux
//! sequences) are unit-tested in their modules.

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

const TEST_MODULE: &str = "#[cfg(test)]\nmod tests";

/// The production part of one file: only a TRAILING `#[cfg(test)] mod tests`
/// block is removed, i.e. one after which nothing at column 0 follows except
/// its closing brace. An early or empty test module never hides the
/// production code after it.
fn production_region(source: &str) -> &str {
    let Some(at) = source.rfind(TEST_MODULE) else {
        return source;
    };
    let mut closed = false;
    for line in source[at..].lines().skip(2).filter(|line| !line.is_empty()) {
        if closed {
            return source;
        }
        if line == "}" {
            closed = true;
        } else if !line.starts_with(char::is_whitespace) {
            return source;
        }
    }
    if closed {
        &source[..at]
    } else {
        source
    }
}

/// A dedicated `dir/tests.rs` is test-only when its parent module declares
/// it under `#[cfg(test)]`.
fn is_declared_test_file(name: &str, sources: &[(String, String)]) -> bool {
    let Some(dir) = name.strip_suffix("/tests.rs") else {
        return false;
    };
    let parents = [format!("{dir}.rs"), format!("{dir}/mod.rs")];
    sources.iter().any(|(parent, text)| {
        parents.contains(parent)
            && (text.contains("#[cfg(test)]\nmod tests;")
                || text.contains("#[cfg(test)]\npub(crate) mod tests;"))
    })
}

fn production_sources() -> Vec<(String, String)> {
    let sources = all_sources();
    sources
        .iter()
        .filter(|(name, _)| !is_declared_test_file(name, &sources))
        .map(|(name, text)| (name.clone(), production_region(text).to_string()))
        .collect()
}

#[derive(Debug, Eq, PartialEq)]
struct SpawnSite {
    function: String,
    argument: String,
}

fn is_ident(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

fn skip_ws(text: &str) -> &str {
    text.trim_start_matches(char::is_whitespace)
}

/// The name of the last `fn` declared before `at` (empty at file scope).
fn enclosing_function(source: &str, at: usize) -> String {
    let before = &source[..at];
    let mut found = String::new();
    for (index, _) in before.match_indices("fn ") {
        if before[..index].chars().next_back().is_some_and(is_ident) {
            continue;
        }
        let name: String = before[index + 3..]
            .chars()
            .take_while(|character| is_ident(*character))
            .collect();
        if !name.is_empty() {
            found = name;
        }
    }
    found
}

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

/// Every `<type_name>::new(` call, tolerant of whitespace around `::`, `new`
/// and `(`. `type_name` must not be the tail of a longer identifier, and
/// `CommandBuilder` is not `Command`.
fn constructor_sites(source: &str, type_name: &str) -> Vec<SpawnSite> {
    let mut sites = Vec::new();
    for (at, _) in source.match_indices(type_name) {
        if source[..at].chars().next_back().is_some_and(is_ident) {
            continue;
        }
        let rest = &source[at + type_name.len()..];
        let Some(rest) = skip_ws(rest).strip_prefix("::") else {
            continue;
        };
        let Some(rest) = skip_ws(rest).strip_prefix("new") else {
            continue;
        };
        if rest.chars().next().is_some_and(is_ident) {
            continue;
        }
        let Some(rest) = skip_ws(rest).strip_prefix('(') else {
            continue;
        };
        sites.push(SpawnSite {
            function: enclosing_function(source, at),
            argument: call_argument(rest).to_string(),
        });
    }
    sites
}

/// Constructs that would spawn a process past the `Command::new` census.
const FORBIDDEN_SPAWN_TOKENS: &[&str] = &[
    "Command as",
    "= Command;",
    "= std::process::Command;",
    "= process::Command;",
    "posix_spawn",
    "libc::exec",
    "libc::system",
    "libc::fork",
    "libc::popen",
];

/// tmux verbs that make the server itself run a shell command.
const FORBIDDEN_TMUX_VERBS: &[&str] = &["run-shell", "pipe-pane", "display-popup", "\"popup\""];

/// Spawn-vocabulary violations in one production file (empty when clean).
fn spawn_vocabulary_violations(name: &str, source: &str) -> Vec<String> {
    let mut violations = Vec::new();
    for token in FORBIDDEN_SPAWN_TOKENS.iter().chain(FORBIDDEN_TMUX_VERBS) {
        if source.contains(token) {
            violations.push(format!("{name}: forbidden {token}"));
        }
    }
    for (at, _) in source.match_indices("if-shell") {
        let rest = &source[at + "if-shell".len()..];
        let next = if let Some(literal_tail) = rest.strip_prefix('"') {
            // `"if-shell".into(), "-F".into()`: the next string literal.
            literal_tail.split('"').nth(1).unwrap_or("")
        } else if rest.starts_with(' ') {
            rest.split_whitespace().next().unwrap_or("")
        } else {
            ""
        };
        if next != "-F" {
            violations.push(format!("{name}: if-shell without -F"));
        }
    }
    violations
}

/// Runtime construction of a shell path in deck-app itself.
const FORBIDDEN_SHELL_PATHS: &[&str] = &[
    "\"/bin\"",
    "\"/bin/\"",
    "join(\"zsh\")",
    "join(\"bash\")",
    "join(\"sh\")",
    "\"/bin/zsh\"",
    "\"/bin/bash\"",
    "\"/bin/sh\"",
];

fn shell_path_violations(name: &str, source: &str) -> Vec<String> {
    FORBIDDEN_SHELL_PATHS
        .iter()
        .filter(|token| source.contains(**token))
        .map(|token| format!("{name}: constructs a shell path with {token}"))
        .collect()
}

/// Fixed-argument system tools deck may spawn, at reviewed files (exact
/// relative path). A new `open` in an unrelated module is not accepted
/// merely because another feature already uses it.
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

/// Computed executables, each reviewed at an exact file AND enclosing
/// function: the bundled tmux sidecar and the relaunch waiter (the installed
/// deck bundle itself, in helper mode). A new `Command::new(program)`
/// anywhere else in `tmux.rs` is an unreviewed site.
const ALLOWED_EXPRESSIONS: &[(&str, &str, &str)] = &[
    // tmux_program() is the one gate: it resolves ONLY the sidecar inside
    // this build's bundle, never Homebrew/MacPorts and never a PATH lookup
    ("tmux.rs", "tmux", "tmux_program()?"),
    ("tmux.rs", "tmux_with_stdin", "tmux_program()?"),
    ("tmux.rs", "tmux_owned", "tmux_program()?"),
    ("tmux.rs", "tmux_batch", "tmux_sidecar"),
    // Private control-channel seam: production passes tmux_program(); the
    // injectable argument exists only so an isolated bundled tmux can test
    // the real protocol without touching the deck/deck-dev sockets.
    ("tmux.rs", "connect_with", "program"),
    ("commands.rs", "tmux_available", "tmux_sidecar"),
    ("tmux_lifecycle.rs", "helper_version", "tmux_sidecar"),
    ("relaunch.rs", "helper_command", "executable"),
];

/// Debug-only smoke instrumentation may read the pasteboard back; it is
/// compiled into isolated smoke builds only.
const DEBUG_ONLY_LITERALS: &[(&str, &str)] = &[("smoke_faults.rs", "pbpaste")];

const EXPECTED_COMMAND_SITES: usize = 23;

type UsedLiteral = (String, String);
type UsedExpression = (String, String, String);

/// Allowlist violations for one file's `Command::new` sites. The used
/// entries are recorded so stale allowlist entries can be rejected.
fn allowlist_violations(
    name: &str,
    source: &str,
    used_literals: &mut Vec<UsedLiteral>,
    used_expressions: &mut Vec<UsedExpression>,
) -> (usize, Vec<String>) {
    let sites = constructor_sites(source, "Command");
    let mut violations = Vec::new();
    for site in &sites {
        let arg = site.argument.as_str();
        if let Some(literal) = arg.strip_prefix('"').and_then(|a| a.strip_suffix('"')) {
            let allowed = ALLOWED_LITERAL_SITES
                .iter()
                .chain(DEBUG_ONLY_LITERALS)
                .find(|(file, tool)| *file == name && *tool == literal);
            match allowed {
                Some((file, tool)) => used_literals.push((file.to_string(), tool.to_string())),
                None => violations.push(format!(
                    "{name}: spawns {literal:?} at an unreviewed call site"
                )),
            }
        } else {
            let allowed = ALLOWED_EXPRESSIONS.iter().find(|(file, function, expr)| {
                *file == name && *function == site.function && *expr == arg
            });
            match allowed {
                Some((file, function, expr)) => used_expressions.push((
                    file.to_string(),
                    function.to_string(),
                    expr.to_string(),
                )),
                None => violations.push(format!(
                    "{name}: spawns a computed executable {arg:?} in fn {} that has not been reviewed",
                    site.function
                )),
            }
        }
    }
    (sites.len(), violations)
}

fn scan_one(name: &str, source: &str) -> Vec<String> {
    let mut violations = spawn_vocabulary_violations(name, source);
    violations.extend(shell_path_violations(name, source));
    violations.extend(allowlist_violations(name, source, &mut Vec::new(), &mut Vec::new()).1);
    violations
}

#[test]
fn scanner_rejects_aliases_whitespace_and_indirect_spawns() {
    let rejected = [
        "use std::process::Command as C;\nfn f() { C::new(\"x\"); }\n",
        "type Spawn = std::process::Command;\n",
        "fn f() { Command::new (\"curl\").status(); }\n",
        "fn f() { Command :: new(\"curl\").status(); }\n",
        "fn f() { unsafe { libc::posix_spawn(p, a, b, c, d, e) }; }\n",
        "fn f() { unsafe { libc::execv(p, a) }; }\n",
        "fn f() { unsafe { libc::system(p) }; }\n",
        "fn f() { tmux(&[\"run-shell\", \"id\"]); }\n",
        "fn f() { tmux(&[\"pipe-pane\", \"-o\", \"cat\"]); }\n",
        "fn f() { tmux(&[\"display-popup\", \"sh\"]); }\n",
        "fn f() { args.extend([\"if-shell\".into(), \"-t\".into()]); }\n",
        "fn f() { format!(\"if-shell -t {t} 'true' ''\"); }\n",
        "fn f() { let s = Path::new(\"/bin\").join(\"zsh\"); }\n",
    ];
    for sample in rejected {
        assert!(
            !scan_one("links.rs", sample).is_empty(),
            "scanner accepted {sample:?}"
        );
    }
    for accepted in [
        "fn f() { args.extend([\"if-shell\".into(), \"-F\".into()]); }\n",
        "fn f() { format!(\"if-shell -F -t {t} '#{{x}}' 'a' ''\"); }\n",
        "fn f() { CommandBuilder::new(tmux_program()?); }\n",
        "fn f() { Command::new(\"open\").status(); }\n",
    ] {
        assert!(
            scan_one("links.rs", accepted).is_empty(),
            "scanner rejected {accepted:?}: {:?}",
            scan_one("links.rs", accepted)
        );
    }
}

#[test]
fn scanner_binds_sites_to_exact_paths_and_functions() {
    let reviewed = "fn connect_with(program: &str) { Command::new(program); }\n";
    assert!(scan_one("tmux.rs", reviewed).is_empty());
    // `foo_tmux.rs` and `x/tmux.rs` are not `tmux.rs`.
    assert!(!scan_one("foo_tmux.rs", reviewed).is_empty());
    assert!(!scan_one("x/tmux.rs", reviewed).is_empty());
    // The reviewed argument in any other function of tmux.rs is rejected.
    let elsewhere = "fn spawn_anything(program: &str) { Command::new(program); }\n";
    assert!(!scan_one("tmux.rs", elsewhere).is_empty());
    // A reviewed literal in an unreviewed file is rejected.
    assert!(!scan_one("drops.rs", "fn f() { Command::new(\"open\"); }\n").is_empty());
}

#[test]
fn only_a_trailing_test_module_is_left_out_of_the_scan() {
    let early = "#[cfg(test)]\nmod tests {}\n\nfn f() { Command::new(\"curl\"); }\n";
    assert_eq!(production_region(early), early);
    assert!(!scan_one("links.rs", production_region(early)).is_empty());

    let trailing =
        "fn f() {}\n\n#[cfg(test)]\nmod tests {\n    fn t() { Command::new(\"curl\"); }\n}\n";
    assert_eq!(production_region(trailing), "fn f() {}\n\n");

    let item_after = "fn f() {}\n\n#[cfg(test)]\nmod tests {\n    fn t() {}\n}\n\nfn g() { Command::new(\"curl\"); }\n";
    assert_eq!(production_region(item_after), item_after);

    let sources = vec![
        ("a.rs".to_string(), "#[cfg(test)]\nmod tests;\n".to_string()),
        ("a/tests.rs".to_string(), String::new()),
        ("b.rs".to_string(), "mod tests;\n".to_string()),
        ("b/tests.rs".to_string(), String::new()),
    ];
    assert!(is_declared_test_file("a/tests.rs", &sources));
    assert!(!is_declared_test_file("b/tests.rs", &sources));
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
    let mut violations = Vec::new();
    let mut used_literals = Vec::new();
    let mut used_expressions = Vec::new();
    for (name, src) in production_sources() {
        violations.extend(spawn_vocabulary_violations(&name, &src));
        let (count, found) =
            allowlist_violations(&name, &src, &mut used_literals, &mut used_expressions);
        seen += count;
        violations.extend(found);
    }
    assert!(violations.is_empty(), "{violations:#?}");
    for (file, tool) in ALLOWED_LITERAL_SITES.iter().chain(DEBUG_ONLY_LITERALS) {
        assert!(
            used_literals.contains(&(file.to_string(), tool.to_string())),
            "stale allowlist entry ({file}, {tool})"
        );
    }
    for (file, function, expr) in ALLOWED_EXPRESSIONS {
        assert!(
            used_expressions.contains(&(file.to_string(), function.to_string(), expr.to_string())),
            "stale allowlist entry ({file}, {function}, {expr})"
        );
    }
    assert_eq!(
        seen, EXPECTED_COMMAND_SITES,
        "the production process surface changed and needs EDR review"
    );
}

#[test]
fn deck_app_never_constructs_a_shell_path() {
    let violations: Vec<_> = production_sources()
        .iter()
        .flat_map(|(name, src)| shell_path_violations(name, src))
        .collect();
    assert!(violations.is_empty(), "{violations:#?}");
}

#[test]
fn pty_launches_only_the_reviewed_bundled_tmux() {
    let mut sites = Vec::new();
    for (name, src) in production_sources() {
        for site in constructor_sites(&src, "CommandBuilder") {
            sites.push((name.clone(), site.argument));
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
    assert!(constructor_sites(&helper, "Command").is_empty());
    assert!(spawn_vocabulary_violations("status-helper", &helper).is_empty());
}

#[test]
fn mcp_sidecars_keep_the_reviewed_process_boundary() {
    let adapter = std::fs::read_to_string(manifest("mcp-adapter/src/main.rs")).unwrap();
    assert!(constructor_sites(production_region(&adapter), "Command").is_empty());
    for forbidden in ["launchctl", "LaunchAgents", "LaunchDaemons", "osascript"] {
        assert!(
            !adapter.contains(forbidden),
            "MCP adapter introduced {forbidden}"
        );
    }

    let runner = std::fs::read_to_string(manifest("mcp-runner/src/main.rs")).unwrap();
    for (name, source) in [("mcp-adapter", &adapter), ("mcp-runner", &runner)] {
        let violations = spawn_vocabulary_violations(name, production_region(source));
        assert!(violations.is_empty(), "{violations:#?}");
    }
    // The ONE shell spawn: the runner's literal /bin/zsh inside spawn_job,
    // used only for a locally approved trusted-host job. Human takeover
    // starts no shell, so no other /bin/zsh appears in production code.
    let production = production_region(&runner);
    assert_eq!(
        constructor_sites(production, "Command"),
        vec![SpawnSite {
            function: "spawn_job".into(),
            argument: "\"/bin/zsh\"".into(),
        }]
    );
    assert_eq!(
        production.matches("/bin/zsh").count(),
        1,
        "the runner starts a shell only at its reviewed spawn_job entry"
    );
    for forbidden in [
        "launchctl",
        "LaunchAgents",
        "LaunchDaemons",
        "osascript",
        "Command::new(\"sh\")",
        "Command::new(\"bash\")",
    ] {
        assert!(
            !runner.contains(forbidden),
            "MCP runner introduced {forbidden}"
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
        for site in constructor_sites(&src, "Command") {
            for spawn in [
                "\"ps\"",
                "\"date\"",
                "\"osascript\"",
                "\"sh\"",
                "\"bash\"",
                "\"zsh\"",
            ] {
                assert_ne!(
                    site.argument, spawn,
                    "{name}: process facts come from libproc/sysctl; no shell or AppleScript"
                );
            }
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
