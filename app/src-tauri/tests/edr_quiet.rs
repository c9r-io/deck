//! EDR-quiet tripwires. A corporate EDR once flagged deck and IT demanded the
//! app be stopped, so the process surface is a closed allowlist enforced here
//! over PRODUCTION source (only a file's trailing `#[cfg(test)] mod tests`
//! and dedicated `*/tests.rs` files declared `#[cfg(test)]` are left out).
//! This header is the contract; CLAUDE.md only points here.
//!
//! What deck never does:
//! - touch launchd — no `launchctl` (not even a one-shot `submit`), no
//!   LaunchAgents/LaunchDaemons, no login items; the post-update relaunch is
//!   a `setsid`-detached waiter (`relaunch.rs`) that waits for the old PID
//!   and `open -n`s the installed bundle;
//! - spawn `ps`, `date`, `osascript` or a shell: process facts (pid, ppid,
//!   footprint, tty, foreground group, argv[0]) come from libproc and
//!   `KERN_PROCARGS2` in `procinfo.rs`, local time from `localtime_r`, and a
//!   duplicate instance just logs and exits; default paths, structured
//!   reads, metadata and readiness queries never start a shell; human
//!   takeover of an MCP session starts no shell;
//! - build a shell path at runtime, or hide a spawn behind concatenation,
//!   an alias or a variable-level allowlist entry;
//! - write an executable under `~`: agent hook commands name the helper
//!   INSIDE the signed bundle;
//! - run any tmux but the signed sidecar next to its own executable
//!   (`tmux::tmux_program()`), never Homebrew/MacPorts or a PATH lookup
//!   (`/usr/local/bin` is user-writable on many Macs, and every deck session
//!   descends from that binary);
//! - open a listener other than the disabled-by-default Connector's one
//!   inbound HTTPS listener on the selected private IPv4 address (RFC1918,
//!   169.254/16 on `bridge*` or 100.64/10 on `utun*` — `docs/connector.md`;
//!   never public or 0.0.0.0).
//!
//! - let a dependency spawn on its behalf: the updater plugin's macOS
//!   installer runs an admin AppleScript (OSAKit) when the bundle is not
//!   writable and a PATH `touch` after every install, so deck calls only
//!   its download/verify half, installs the verified archive itself
//!   (`updater::install_bundle`) after a no-spawn writability check, and
//!   pins the plugin version here so a bump re-reads that path. The
//!   strings stay linked in the binary; `scripts/check-edr-binary` names
//!   them as gated rather than forbidden.
//!
//! - post anything but a card title and one of two fixed phrases as a
//!   macOS notification (`notify.rs` + `native/NotificationBridge.swift`,
//!   in-process, UNUserNotificationCenter only, no-op outside a bundle).
//!
//! What deck may spawn: low-frequency, fixed-argument system tools named by
//! absolute `/usr/bin` path (`open`, `plutil`, `pbcopy`, `sw_vers`, `uname`;
//! never a PATH lookup), the bundled tmux, and — for a locally approved,
//! unexpired trusted-host MCP job — the signed `deck-mcp-runner`, which
//! starts the requested absolute executable path and exact argv at its
//! single fixed entry (`spawn_job`); bare names never resolve through the
//! child PATH. Execution authority permits any program, including
//! interpreters and shells; it is not a sandbox, `-d -f` does not skip
//! `/etc/zshenv`, and nothing here promises EDR invisibility. The ONE
//! executable outside the bundle is the optional, separately installed and
//! signed Deck Tunnel Helper, only at `/Applications/Deck Tunnel
//! Helper.app/Contents/MacOS/deck-tunnelctl` after its Team ID/identifier
//! signature and file-identity checks, with closed argv (`tunnel_helper.rs`;
//! protocol once per session, status cached). The helper in turn runs only
//! the hash-pinned external `tunnel-client` (census in
//! `tools/deck-tunnelctl/src/lib.rs`; `tunnel-helper.yml` runs
//! `check-edr-binary` on it). That `tunnel-client` keeps an outbound HTTPS
//! connection in its own tmux session, outlives deck until explicitly
//! stopped or the Mac restarts (nothing relaunches it), and is OUTSIDE
//! deck's EDR-quiet promise. Isolated debug/smoke servers must be retired
//! after evidence capture: `scripts/edr_runtime.py` inventories only
//! reviewed `deck-dev` / `deck-smoke-*` identities and cleanup is explicit;
//! release workflows run `scripts/check-edr-binary` over the packaged app.
//!
//! What this file enforces:
//! 1. every `Command::new(...)` (whitespace-tolerant) names a fixed,
//!    low-frequency system tool at a reviewed file, or a reviewed computed
//!    executable at an exact file AND enclosing function; every allowlist
//!    entry must still be used, and the total site count is pinned;
//! 2. aliases and lower-level spawns (`Command as`, `type X = Command`,
//!    `posix_spawn`, `libc::exec*`, `libc::system`, ...) are forbidden, as
//!    are tmux verbs that run a shell (`run-shell`, `pipe-pane`,
//!    `display-popup`/`popup`, the `#(...)` format job) and any `if-shell`
//!    without `-F`;
//! 3. deck-app never constructs a shell path at runtime (`"/bin"`,
//!    `join("zsh")`, ...), and the MCP runner has no fixed shell spawn;
//! 4. nothing touches launchd, login items or `~/.deck/bin`;
//! 5. the shell-restore path carries no script, shell argv or deck-as-pane
//!    bootstrap (`commands::restore_start_args` pins the positive shape).
//! 6. the disabled-by-default Connector owns the only production TCP bind,
//!    and its enable path must call the private/local IPv4 validator first;
//! 7. updates are installed by deck, never by the plugin (no
//!    `download_and_install` / `install(`), behind the writability guard,
//!    and the reviewed plugin version is pinned in `Cargo.lock`.
//!
//! The scanner is a pure function over `(relative path, source)`, so the
//! negative tests below feed it in-memory samples. It is a review tripwire,
//! not a sandbox: it keeps the spawn vocabulary from growing unreviewed.
//! Behavioural facts (libproc process facts, `localtime_r`, the tmux
//! sequences) are unit-tested in their modules.

mod source_scan;
use source_scan::{
    all_sources, enclosing_function, is_declared_test_file, is_ident, manifest, production_region,
    production_sources,
};

#[derive(Debug, Eq, PartialEq)]
struct SpawnSite {
    function: String,
    argument: String,
}

fn skip_ws(text: &str) -> &str {
    text.trim_start_matches(char::is_whitespace)
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

/// tmux verbs that make the server itself run a shell command, plus the
/// `#(...)` format job, which tmux expands through `/bin/sh` in any format.
/// Runtime values entering a format stay restricted to `sanitize_process`'s
/// character set (`restart.rs`, `context.rs`).
const FORBIDDEN_TMUX_VERBS: &[&str] =
    &["run-shell", "pipe-pane", "display-popup", "\"popup\"", "#("];

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
    ("links.rs", "/usr/bin/open"),
    ("diagnostics.rs", "/usr/bin/open"),
    ("diagnostics.rs", "/usr/bin/sw_vers"),
    ("diagnostics.rs", "/usr/bin/uname"),
    ("relaunch.rs", "/usr/bin/open"),
    ("relaunch.rs", "/usr/bin/plutil"),
    ("commands.rs", "/usr/bin/pbcopy"),
    ("inbound.rs", "/usr/bin/open"),
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
    // Optional Tunnel integration executes only the separately signed helper
    // selected by resolve_helper(); no webview-supplied executable or argv.
    // The helper's own spawn census (one site: the hash-pinned tunnel-client)
    // is `production_spawn_census_is_one_verified_tunnel_client_site` in
    // tools/deck-tunnelctl/src/lib.rs, run by tunnel-helper.yml.
    ("tunnel_helper.rs", "invoke", "&helper.path"),
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

fn tcp_bind_count(source: &str) -> usize {
    let compact = source
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    ["TcpListener::bind(", "TcpSocket::bind(", "Server::bind("]
        .iter()
        .map(|token| compact.matches(token).count())
        .sum()
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
        "fn f() { tmux(&[\"display-message\", \"-p\", \"#(id)\"]); }\n",
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
        "fn f() { Command::new(\"/usr/bin/open\").status(); }\n",
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

/// The notification bridge talks to UNUserNotificationCenter and nothing
/// else: no process, no network, no file, no log line, and every entry is
/// a no-op outside a bundle. notify.rs hands the system only a card title
/// and one of two fixed phrases, from exactly one call site.
#[test]
fn native_notifications_are_in_process_and_content_closed() {
    let swift = std::fs::read_to_string(manifest("native/NotificationBridge.swift")).unwrap();
    for forbidden in [
        "Process(",
        "NSTask",
        "URLSession",
        "FileManager",
        "print(",
        "NSLog(",
        "launchctl",
        "osascript",
        "userInfo",
        "attachments",
    ] {
        assert!(
            !swift.contains(forbidden),
            "native notifications must not introduce {forbidden}"
        );
    }
    assert!(swift.contains("guard deckNotifyBundled() else"));
    assert_eq!(
        swift.matches("guard deckNotifyBundled() else").count(),
        5,
        "every C entry is guarded"
    );
    let notify = std::fs::read_to_string(manifest("src/notify.rs")).unwrap();
    let notify = production_region(&notify);
    assert_eq!(
        notify.matches("deck_notify_post(").count(),
        2,
        "one declaration, one call"
    );
    let declared = "fn deck_notify_post(";
    let declaration = notify.find(declared).expect("declared");
    let after = declaration + declared.len();
    let call = notify[after..]
        .find("deck_notify_post(")
        .map(|at| at + after)
        .expect("the one post call");
    assert!(
        notify[..call].contains("impl Native for SystemNative"),
        "the call is inside SystemNative::post"
    );
    // the body comes from body_text and nowhere else
    assert_eq!(notify.matches("&body_text(").count(), 1);
    for phrase in [
        "asked for your input",
        "a turn has ended",
        "请求了你的输入",
        "一轮已结束",
    ] {
        assert_eq!(
            notify.matches(phrase).count(),
            1,
            "{phrase} is defined once"
        );
    }
    let build = std::fs::read_to_string(manifest("build.rs")).unwrap();
    assert!(build.contains("native/NotificationBridge.swift"));
    assert!(build.contains("\"UserNotifications\","));
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
    // a bare name resolves through PATH, which may hold a user-writable dir
    for (file, tool) in ALLOWED_LITERAL_SITES {
        assert!(
            tool.starts_with("/usr/bin/"),
            "({file}, {tool}) must name the absolute system tool"
        );
    }
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

/// A pane program must not be able to write the macOS clipboard through
/// tmux: every `set-clipboard` the backend sends turns it OFF (the contract
/// suite proves what `on` would let through). `external` is not enough —
/// tmux would still push each of deck's own selection snapshots through the
/// PTY as OSC 52.
#[test]
fn the_tmux_server_never_enables_the_terminal_clipboard() {
    let mut sites = 0;
    for (name, src) in production_sources() {
        for (at, _) in src.match_indices("set-clipboard") {
            let line_start = src[..at].rfind('\n').map_or(0, |i| i + 1);
            if src[line_start..at].trim_start().starts_with("//") {
                continue; // prose in a module header, not a tmux option
            }
            let rest = src[at + "set-clipboard".len()..]
                .trim_start_matches(|c: char| c == '"' || c == ',' || c.is_whitespace());
            assert!(
                rest.starts_with("off"),
                "{name}: set-clipboard must be off, found {:?}",
                rest.chars().take(12).collect::<String>()
            );
            sites += 1;
        }
    }
    assert_eq!(sites, 2, "tmux.rs sets the option in its conf and on reuse");
}

/// tauri-plugin-updater 2.10.1 `src/updater.rs:1274-1305`: on a
/// permission error the installer runs `do shell script … with
/// administrator privileges` through OSAKit, and after every successful
/// install spawns a PATH-resolved `touch`. deck never reaches that code:
/// it downloads and verifies through the plugin, checks writability first
/// (no spawn) and swaps the bundle itself. A plugin bump must re-read that
/// installer before the pin below moves.
#[test]
fn updates_are_installed_by_deck_not_by_the_plugin() {
    let updater = std::fs::read_to_string(manifest("src/updater.rs")).unwrap();
    // code only: the module header names the plugin calls it avoids
    let updater: String = production_region(&updater)
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    for forbidden in ["download_and_install", ".install("] {
        assert!(
            !updater.contains(forbidden),
            "updater.rs reaches the plugin installer via {forbidden}"
        );
    }
    let guard = updater
        .find("writable_bundle()?")
        .expect("the writability guard is called");
    let flag = updater
        .find("begin_app_update_install()?")
        .expect("the lifecycle flag is raised");
    let download = updater
        .find(".download(")
        .expect("the plugin download is used");
    assert!(
        guard < flag && flag < download,
        "guard, then lifecycle flag, then download"
    );
    assert!(
        updater.contains("libc::access("),
        "writability is asked with access(2)"
    );

    let lock = std::fs::read_to_string(manifest("Cargo.lock")).unwrap();
    assert!(
        lock.contains("name = \"tauri-plugin-updater\"\nversion = \"2.10.1\""),
        "tauri-plugin-updater moved off 2.10.1: re-read its macOS installer \
         (admin AppleScript, PATH touch) and update updater.rs and this pin"
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
fn connector_owns_the_only_production_tcp_listener() {
    let sites = production_sources()
        .into_iter()
        .filter_map(|(name, source)| {
            let count = tcp_bind_count(&source);
            (count > 0).then_some((name, count))
        })
        .collect::<Vec<_>>();
    assert_eq!(sites, vec![("connector/server.rs".into(), 1)]);
    for sidecar in [
        "mcp-adapter/src/main.rs",
        "mcp-runner/src/main.rs",
        "status-helper/src/main.rs",
    ] {
        let source = std::fs::read_to_string(manifest(sidecar)).unwrap();
        assert_eq!(
            tcp_bind_count(production_region(&source)),
            0,
            "{sidecar} introduced a TCP listener"
        );
    }

    let network = std::fs::read_to_string(manifest("src/connector/network.rs")).unwrap();
    assert!(production_region(&network).contains("fn validate_connector_listener("));
    let commands = std::fs::read_to_string(manifest("src/connector/commands.rs")).unwrap();
    let production = production_region(&commands);
    let enable = production
        .split("pub(crate) async fn connector_enable")
        .nth(1)
        .and_then(|tail| tail.split("#[tauri::command]").next())
        .expect("connector_enable production body");
    assert!(enable.contains("validate_connector_listener("));

    for sample in [
        "std::net::TcpListener::bind(\"127.0.0.1:0\")",
        "tokio::net::TcpListener :: bind(addr).await",
        "hyper::Server::bind(&addr)",
        "TcpSocket::bind(addr)",
    ] {
        assert_eq!(tcp_bind_count(sample), 1, "scanner missed {sample}");
    }
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
    // The one reviewed trusted-host spawn site lives in spawn_job and uses the
    // locally authorized executable verbatim. Human takeover starts no process.
    let production = production_region(&runner);
    assert_eq!(
        constructor_sites(production, "Command"),
        vec![SpawnSite {
            function: "spawn_job".into(),
            argument: "executable".into(),
        }]
    );
    assert_eq!(
        production.matches("/bin/zsh").count(),
        0,
        "the runner must not retain the removed fixed-shell launch point"
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
