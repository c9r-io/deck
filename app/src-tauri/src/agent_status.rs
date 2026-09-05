//! agent_status.rs — modular, content-free agent state for cards.
//!
//! Agent CLIs (Claude Code today; Codex etc. as later modules) run hooks
//! inside deck's tmux panes. Each hook invokes the bundled
//! `deck-status-helper`, which forwards ONE closed status word plus the pane
//! identity it inherited from `$TMUX`/`$TMUX_PANE` to a local unix socket in
//! the deck data dir. This module owns that socket, the closed vocabulary,
//! the pane→session resolution, and the reconciliation that clears state
//! when the agent process leaves the pane's foreground.
//!
//! Design rules (mirror the scheduler's context philosophy):
//! - The state is a CLOSED enum. Prompt text, notification messages and hook
//!   payloads never enter this module — the helper already discarded them.
//! - What decides whether a state is still valid is the pane's observed
//!   foreground process, captured AT EVENT TIME (`expected_fg`), never a
//!   hard-coded per-agent executable list.
//! - No automatic card movement. This is presentation-only input for the
//!   board's status dot; the poll merge in `commands.rs` is the sole reader.
//!
//! Adding an agent module = one entry in `SOURCES` + an installer that
//! registers that agent's own hook/notify config to call the same helper
//! with its own source word. The socket protocol and store are shared.
//!
//! # Contract
//! Agent status hooks (`agent_status.rs`, opt-in): agent CLIs report a CLOSED
//! state word (`working | needs-input | turn-done`) via the bundled
//! `deck-status-helper` (`app/status-helper/`, standalone zero-dep crate;
//! `build.rs` builds it into `binaries/` for tauri `externalBin` on every
//! build). Hook entries name the helper INSIDE the installed bundle
//! (`/Applications/deck.app/Contents/MacOS/deck-status-helper` or the
//! `~/Applications` twin) — never a copy under `~/.deck/bin`: that copy was
//! an EDR persistence signature, and a dev build once overwrote it with an
//! ad-hoc-signed binary that Claude Code then executed on every event. Only
//! a release-location install can enable hooks; dev/smoke builds get an
//! error and never touch agent config. `migrate_hooks_on_boot` (release
//! installs only) rewrites installed entries that are not what the CURRENT
//! spec describes — `hooks_are_current` compares the WHOLE entry (event,
//! matcher, helper path, style, args) and rejects a deck entry left under a
//! retired event, so a spec change (a narrowed matcher, a moved bundle, the
//! legacy copy) reaches users who enabled the toggle under an older version
//! without them touching the switch; installed-ness alone (`hooks_installed`)
//! cannot see that. It also deletes the legacy copy once nothing references
//! it. Install strips deck's entries document-wide before writing the specs,
//! but an EMPTY array the user wrote is left exactly as written. Hooks inherit `$TMUX`/`$TMUX_PANE`; the helper
//! drains and DISCARDS the hook stdin payload, charset-validates every field,
//! and writes one JSON line to the instance's `status.sock` (0600) — routed
//! per pane by `DECK_STATUS_SOCK`, which each deck exports into its own tmux
//! server env (tmux.rs), falling back to `~/.deck/status.sock`; so an
//! isolated/smoke instance receives its own events and never production's.
//! The helper can never carry content, exits 0 always, and silently does
//! nothing outside a deck tmux pane. The backend listener validates source/state against the module
//! registry, requires the event's socket name AND tmux server pid (generation
//! stamp — restarted servers reuse pane ids) before resolving pane→session,
//! refuses shell-foreground panes, and records the observed foreground
//! executable; `poll_sessions` reconciles so the state dies with the process
//! that reported it — no TTLs and no per-agent executable lists. Frontend:
//! `effectiveCardStatus` (pure.js) — agent state OUTRANKS the 15s heuristic
//! (card statuses `attention`/`done`; a working agent never shows amber). The
//! Settings toggle is the user-driven writer of `~/.claude/settings.json`
//! (three entries: UserPromptSubmit→working, Notification matcher
//! `permission_prompt` ONLY→needs-input, Stop→turn-done, written in
//! Claude Code's EXEC form — bare helper path in `command`, words in
//! `args` — so Claude Code spawns the signed helper directly and no `sh -c`
//! runs per event; Codex stays shell-form + `async`, exec form being
//! undocumented there), and the
//! release-only boot migration is the sole other one; install, migrate and
//! uninstall touch only entries containing `deck.app/Contents/MacOS/deck-status-helper` (or the legacy `.deck/bin/deck-status-helper`),
//! preserve everything else including file mode, and never modify a malformed
//! file. The toggle state is DERIVED from that file — never stored twice.
//! Claude Code's Stop does not fire on Esc-interrupt; foreground
//! reconciliation and the next UserPromptSubmit heal that. `idle_prompt` is
//! deliberately NOT matched: Claude Code fires it ~60s after the prompt goes
//! idle, so EVERY finished card decayed from `done` into `attention` a minute
//! later and the attention colour stopped meaning anything. needs-input means
//! a question raised DURING a turn; an idle prompt after a turn is `turn-done`,
//! which is already on screen. The Codex module
//! uses lifecycle hooks in `$CODEX_HOME/hooks.json` (same document shape as
//! Claude's, so ONE marker-based JSON merge engine serves both via per-agent
//! spec tables): UserPromptSubmit→working, PermissionRequest→needs-input,
//! Stop→turn-done, Interrupt→turn-done, all `"async": true` so the helper can
//! never block a turn. Multiple hooks per event coexist, so this never
//! conflicts with the user's own hooks or `notify` program — the earlier
//! notify/`config.toml` route was DROPPED for exactly that conflict (Codex
//! allows one notify program only; chain-forwarding it was rejected as too
//! much surface). Known caveat: Codex's Stop fires when the model ATTEMPTS to
//! stop, so another Stop hook forcing continuation makes "done" slightly
//! early. Adding an agent module = one `SOURCES` entry + its own installer
//! spec calling the same helper with its own source word, behind its own
//! Settings toggle.

use std::collections::HashMap;
use std::io::Read;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use crate::applog;
use crate::storage;

/// Registered agent modules.
const SOURCES: &[&str] = &["claude-code", "codex"];

/// Closed state vocabulary — the only words that ever reach the frontend.
const STATES: &[&str] = &["working", "needs-input", "turn-done"];

#[derive(Debug, PartialEq)]
pub(crate) struct Event {
    source: String,
    state: &'static str,
    socket: String,
    server_pid: u32,
    pane: String,
}

struct Entry {
    state: &'static str,
    /// Foreground executable observed when the event arrived. The agent
    /// process name varies by install (claude/node/bun), so the entry
    /// self-calibrates instead of trusting a per-agent list.
    expected_fg: String,
}

static AGENTS: Mutex<Option<HashMap<String, Entry>>> = Mutex::new(None);

fn with_agents<R>(f: impl FnOnce(&mut HashMap<String, Entry>) -> R) -> R {
    let mut guard = AGENTS.lock().unwrap();
    f(guard.get_or_insert_with(HashMap::new))
}

// ---------- event parsing (closed validation at the trust boundary) ---------

/// Tiny hand validation instead of serde structs: every field is checked
/// against a closed shape, and anything else — extra fields, wrong types,
/// content — is a categorized drop.
pub(crate) fn parse_event(line: &str) -> Result<Event, &'static str> {
    let value: serde_json::Value = serde_json::from_str(line).map_err(|_| "bad-json")?;
    let obj = value.as_object().ok_or("bad-json")?;
    if obj.get("v").and_then(|v| v.as_u64()) != Some(1) {
        return Err("bad-version");
    }
    let source = obj
        .get("source")
        .and_then(|v| v.as_str())
        .filter(|s| SOURCES.contains(s))
        .ok_or("unknown-source")?;
    let state = STATES
        .iter()
        .find(|s| Some(**s) == obj.get("state").and_then(|v| v.as_str()))
        .ok_or("unknown-state")?;
    let socket = obj
        .get("socket")
        .and_then(|v| v.as_str())
        .filter(|s| *s == crate::tmux::socket())
        .ok_or("other-server")?;
    let server_pid = obj
        .get("server_pid")
        .and_then(|v| v.as_u64())
        .and_then(|v| u32::try_from(v).ok())
        .ok_or("bad-pid")?;
    let pane = obj
        .get("pane")
        .and_then(|v| v.as_str())
        .filter(|p| {
            p.len() >= 2
                && p.len() <= 10
                && p.starts_with('%')
                && p[1..].bytes().all(|b| b.is_ascii_digit())
        })
        .ok_or("bad-pane")?;
    Ok(Event {
        source: source.to_string(),
        state,
        socket: socket.to_string(),
        server_pid,
        pane: pane.to_string(),
    })
}

// ---------- pane → session resolution --------------------------------------

/// Parse a `#{pid}\t#{pane_id}\t#{session_name}\t#{pane_current_command}`
/// listing and return the (session, foreground) for `pane` — but only if the
/// listing's server pid matches the event's generation stamp (a restarted
/// server reuses numeric pane ids; pid is what tells generations apart).
pub(crate) fn resolve_in(listing: &str, pane: &str, server_pid: u32) -> Option<(String, String)> {
    for line in listing.lines() {
        let mut fields = line.split('\t');
        let (Some(pid), Some(id), Some(session), Some(fg)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if id != pane {
            continue;
        }
        if pid.parse::<u32>().ok() != Some(server_pid) {
            return None;
        }
        if crate::tmux::validate_session_name(session).is_err() {
            return None;
        }
        return Some((session.to_string(), fg.to_string()));
    }
    None
}

fn tmux_resolve(pane: &str, server_pid: u32) -> Option<(String, String)> {
    let listing = crate::tmux::tmux(&[
        "list-panes",
        "-a",
        "-F",
        "#{pid}\t#{pane_id}\t#{session_name}\t#{pane_current_command}",
    ])
    .ok()?;
    resolve_in(&listing, pane, server_pid)
}

/// Validate one wire line and commit it to the store. `resolve` is injected
/// so tests exercise the full path without a live tmux server.
pub(crate) fn ingest(
    line: &str,
    resolve: impl Fn(&str, u32) -> Option<(String, String)>,
) -> Result<(), &'static str> {
    let event = parse_event(line)?;
    let (session, fg) = resolve(&event.pane, event.server_pid).ok_or("no-such-pane")?;
    // An agent hook while a plain shell owns the pane foreground has no
    // process to bind the state's lifetime to — refuse rather than flicker.
    if crate::context::shell_process(Some(&fg)) {
        return Err("shell-foreground");
    }
    applog(&format!(
        "[agent-status] {} {} s={}",
        event.source,
        event.state,
        storage::session_tag(&session)
    ));
    with_agents(|agents| {
        agents.insert(
            session,
            Entry {
                state: event.state,
                expected_fg: fg,
            },
        );
    });
    Ok(())
}

// ---------- poll integration ------------------------------------------------

/// Called from every `poll_sessions` with the fresh pane map
/// (session → (pid, activity, mode, foreground, cwd)). Clears state whose
/// session is gone or whose pane foreground no longer matches the process
/// observed when the state was reported — the agent exited or was replaced.
pub(crate) fn reconcile(panes: &HashMap<String, (u32, u64, bool, String, String)>) {
    with_agents(|agents| {
        agents.retain(|session, entry| {
            let Some((_, _, _, fg, _)) = panes.get(session) else {
                return false;
            };
            !crate::context::shell_process(Some(fg)) && *fg == entry.expected_fg
        });
    });
}

/// The state word for one session, if an agent module reported one.
pub(crate) fn current(session: &str) -> Option<&'static str> {
    with_agents(|agents| agents.get(session).map(|e| e.state))
}

#[cfg(test)]
pub(crate) fn reset_for_tests() {
    with_agents(|agents| agents.clear());
}

// ---------- socket listener --------------------------------------------------

const MAX_LINE: usize = 8 * 1024;

fn read_first_line(stream: &mut UnixStream) -> Option<String> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() > MAX_LINE {
                    return None;
                }
                if buf.contains(&b'\n') {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let line = buf.split(|b| *b == b'\n').next().unwrap_or(&[]);
    String::from_utf8(line.to_vec()).ok()
}

fn handle_stream(mut stream: UnixStream, drops: &mut u32) {
    let Some(line) = read_first_line(&mut stream) else {
        return;
    };
    if line.is_empty() {
        return;
    }
    if let Err(reason) = ingest(&line, tmux_resolve) {
        // categorized, content-free, and bounded — a hostile local writer
        // must not be able to grow app.log without limit
        *drops += 1;
        if *drops <= 20 {
            applog(&format!("[agent-status] dropped ({reason})"));
        } else if *drops == 21 {
            applog("[agent-status] further drops suppressed");
        }
    }
}

pub(crate) fn listen_at(path: &Path) -> Result<UnixListener, String> {
    let _ = std::fs::remove_file(path); // stale socket from a previous run
    let listener = UnixListener::bind(path).map_err(|e| format!("bind failed: {e}"))?;
    // the data dir is already 0700; keep the socket itself private too
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    Ok(listener)
}

/// Accept loop for the status socket. One deck instance owns the data dir
/// (instance lock), so one listener owns this socket.
pub(crate) fn spawn_listener() {
    let path = storage::deck_dir().join("status.sock");
    std::thread::spawn(move || {
        let listener = match listen_at(&path) {
            Ok(l) => l,
            Err(e) => {
                applog(&format!(
                    "[agent-status] socket unavailable ({})",
                    storage::err_code(&e)
                ));
                return;
            }
        };
        let mut drops = 0u32;
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => handle_stream(stream, &mut drops),
                Err(_) => std::thread::sleep(Duration::from_secs(1)),
            }
        }
    });
}

// ---------- hook installation (shared JSON machinery) -------------------------
//
// Claude Code (~/.claude/settings.json) and Codex ($CODEX_HOME/hooks.json)
// use the same hooks document shape: hooks → EventName →
// [{matcher?, hooks: [{type: "command", command, …}]}]. One marker-based
// merge engine serves both; each agent contributes only its spec table.

/// Markers every deck-authored hook command carries; install/uninstall touch
/// only entries containing one, byte-preserving everything else in the file.
/// Current entries run the helper INSIDE the installed, signed, notarized
/// bundle — deck never drops an executable into the home directory (an app
/// writing a binary under `~` and registering it in another program's hook
/// config is an endpoint-security persistence signature, and a development
/// build once overwrote that copy with an ad-hoc-signed one). The legacy
/// marker is the pre-0.5.12 `~/.deck/bin` copy, still recognized so
/// uninstall and boot migration can retire it.
const HELPER_MARKER: &str = "deck.app/Contents/MacOS/deck-status-helper";
const LEGACY_HELPER_MARKER: &str = ".deck/bin/deck-status-helper";

const HELPER_NAME: &str = "deck-status-helper";

/// One deck hook: (event, optional matcher, state word).
type HookSpec = (&'static str, Option<&'static str>, &'static str);

/// Claude Code: `Notification` matches ONLY `permission_prompt` — a question
/// raised in the middle of a turn, which is the whole meaning of
/// "needs-input". `idle_prompt` is deliberately NOT matched: Claude Code
/// fires it ~60s after the prompt goes idle, so every finished card decayed
/// from "done" into "needs-input" a minute later and the attention colour
/// stopped meaning anything. An idle prompt after a turn is `turn-done`,
/// which is already on screen. `Stop` does not fire on user interrupt
/// (documented behavior) — foreground reconciliation and the next
/// `UserPromptSubmit` heal that gap.
const CLAUDE_HOOKS: &[HookSpec] = &[
    ("UserPromptSubmit", None, "working"),
    ("Notification", Some("permission_prompt"), "needs-input"),
    ("Stop", None, "turn-done"),
];

/// Codex lifecycle hooks (hooks.json; several hooks per event may coexist,
/// so this never conflicts with a user's own hooks or `notify` program).
/// `Stop` fires when the model attempts to stop — another Stop hook may
/// force continuation, so "done" can be slightly early; `Interrupt` maps to
/// turn-done so an Esc-interrupted card settles instead of staying "working".
const CODEX_HOOKS: &[HookSpec] = &[
    ("UserPromptSubmit", None, "working"),
    ("PermissionRequest", None, "needs-input"),
    ("Stop", None, "turn-done"),
    ("Interrupt", None, "turn-done"),
];

/// How an agent runs a hook command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HookStyle {
    /// Claude Code exec form: with `args` present the helper is spawned
    /// directly, so no `sh -c` runs per event (one process less for
    /// endpoint security to look at).
    Exec,
    /// Codex shell form with `"async": true` so the fire-and-forget helper
    /// never blocks a turn; exec form is not documented there.
    ShellAsync,
}

/// One deck hook object in the agent's document shape.
fn hook_value(style: HookStyle, helper: &str, source: &str, state: &str) -> serde_json::Value {
    match style {
        HookStyle::Exec => serde_json::json!({
            "type": "command", "command": helper, "args": [source, state], "timeout": 10
        }),
        HookStyle::ShellAsync => serde_json::json!({
            "type": "command", "command": format!("\"{helper}\" {source} {state}"),
            "timeout": 10, "async": true
        }),
    }
}

fn command_is_ours(command: &str) -> bool {
    command.contains(HELPER_MARKER) || command.contains(LEGACY_HELPER_MARKER)
}

fn entry_commands(entry: &serde_json::Value) -> impl Iterator<Item = &str> {
    entry
        .get("hooks")
        .and_then(|h| h.as_array())
        .into_iter()
        .flatten()
        .filter_map(|hook| hook.get("command").and_then(|c| c.as_str()))
}

fn entry_is_ours(entry: &serde_json::Value) -> bool {
    entry_commands(entry).any(command_is_ours)
}

/// The complete document entry one spec must produce.
fn spec_entry(
    style: HookStyle,
    helper: &str,
    source: &str,
    (_, matcher, state): &HookSpec,
) -> serde_json::Value {
    let mut entry = serde_json::json!({ "hooks": [hook_value(style, helper, source, state)] });
    if let Some(matcher) = matcher {
        entry["matcher"] = serde_json::json!(matcher);
    }
    entry
}

/// deck's entries in this document are EXACTLY what the current specs
/// describe: one entry per spec event, byte-identical matcher, helper path,
/// style and arguments, and no deck entry left under an event the specs no
/// longer name. `hooks_installed` only asks whether SOME deck entry exists
/// per event, so it cannot see a spec change — a narrowed matcher, a moved
/// bundle, a legacy `~/.deck/bin` path, a shell-form entry where exec form
/// is expected. This is the predicate boot migration repairs against.
pub(crate) fn hooks_are_current(
    root: &serde_json::Value,
    specs: &[HookSpec],
    source: &str,
    style: HookStyle,
    helper: &str,
) -> bool {
    let Some(hooks) = root.get("hooks").and_then(|h| h.as_object()) else {
        return false;
    };
    let ours = |list: &serde_json::Value| {
        list.as_array()
            .into_iter()
            .flatten()
            .filter(|entry| entry_is_ours(entry))
            .count()
    };
    // nothing of ours under a retired event
    if hooks
        .iter()
        .any(|(event, list)| !specs.iter().any(|(e, _, _)| e == event) && ours(list) > 0)
    {
        return false;
    }
    specs.iter().all(|spec| {
        let Some(list) = hooks.get(spec.0).and_then(|l| l.as_array()) else {
            return false;
        };
        let mut mine = list.iter().filter(|entry| entry_is_ours(entry));
        let Some(entry) = mine.next() else {
            return false;
        };
        mine.next().is_none() && *entry == spec_entry(style, helper, source, spec)
    })
}

/// Add deck's hook entries to a parsed hooks document. Everything the user
/// wrote — other hooks, unknown keys, other events — is preserved; every
/// entry carrying the helper marker is dropped first (document-wide, so an
/// event a previous deck version registered and this one no longer does
/// cannot linger) and the current specs are written in the agent's `style`.
pub(crate) fn hooks_with_install(
    mut root: serde_json::Value,
    specs: &[HookSpec],
    source: &str,
    style: HookStyle,
    helper: &str,
) -> Result<serde_json::Value, String> {
    let obj = root
        .as_object_mut()
        .ok_or("settings file is not a JSON object")?;
    let hooks = obj
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or("the hooks key is not a JSON object")?;
    let mut emptied = Vec::new();
    for (event, list) in hooks.iter_mut() {
        let Some(list) = list.as_array_mut() else {
            continue;
        };
        let before = list.len();
        list.retain(|entry| !entry_is_ours(entry));
        if list.is_empty() && before > 0 {
            emptied.push(event.clone());
        }
    }
    // an event only this document's deck entry populated leaves no husk —
    // but an empty array the USER wrote is left exactly as written
    for event in emptied {
        hooks.remove(&event);
    }
    for spec in specs {
        let list = hooks
            .entry(spec.0)
            .or_insert_with(|| serde_json::json!([]))
            .as_array_mut()
            .ok_or("a hook event entry is not a JSON array")?;
        list.push(spec_entry(style, helper, source, spec));
    }
    Ok(root)
}

/// Remove every deck-authored entry, pruning emptied arrays/objects.
pub(crate) fn hooks_with_uninstall(mut root: serde_json::Value) -> serde_json::Value {
    if let Some(hooks) = root.get_mut("hooks").and_then(|h| h.as_object_mut()) {
        for (_, list) in hooks.iter_mut() {
            if let Some(list) = list.as_array_mut() {
                list.retain(|entry| !entry_is_ours(entry));
            }
        }
        hooks.retain(|_, list| list.as_array().map(|l| !l.is_empty()).unwrap_or(true));
    }
    if root
        .get("hooks")
        .and_then(|h| h.as_object())
        .is_some_and(|h| h.is_empty())
    {
        if let Some(obj) = root.as_object_mut() {
            obj.remove("hooks");
        }
    }
    root
}

/// Installed = every deck hook event carries a marker entry; a partial
/// install reads as OFF so re-enabling repairs it.
pub(crate) fn hooks_installed(root: &serde_json::Value, specs: &[HookSpec]) -> bool {
    let Some(hooks) = root.get("hooks").and_then(|h| h.as_object()) else {
        return false;
    };
    specs.iter().all(|(event, _, _)| {
        hooks
            .get(*event)
            .and_then(|l| l.as_array())
            .is_some_and(|list| list.iter().any(entry_is_ours))
    })
}

fn claude_settings_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".claude").join("settings.json"))
}

/// Codex hooks live in a dedicated `$CODEX_HOME/hooks.json` (same document
/// shape as Claude's). Several hooks per event may coexist, so deck's
/// entries never conflict with the user's own hooks or `notify` program.
/// `CODEX_HOME` is honored when visible; a GUI launch usually doesn't see a
/// shell-exported value, which matches Codex's own default of `~/.codex`.
fn codex_hooks_path() -> Option<PathBuf> {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| dirs::home_dir().map(|home| home.join(".codex")))
        .map(|dir| dir.join("hooks.json"))
}

fn read_settings_value(path: &Path) -> Result<serde_json::Value, String> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| {
            "the agent settings file is not valid JSON — not modifying it".to_string()
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(serde_json::json!({})),
        Err(e) => Err(format!("could not read agent settings ({})", e.kind())),
    }
}

/// Atomic replace that PRESERVES the file's existing permissions (these
/// config files belong to the agent CLI, not deck — 0600 is only the default
/// for a file deck itself creates). Refuses to create the agent's config
/// DIRECTORY: a missing one means the agent never ran on this Mac.
fn write_agent_config(path: &Path, bytes: &[u8], never_ran: &str) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o777)
        .unwrap_or(0o600);
    if let Some(dir) = path.parent() {
        if !dir.exists() {
            return Err(never_ran.into());
        }
    }
    storage::atomic_write(path, bytes)?;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    Ok(())
}

/// The helper a hook command may name: the sidecar inside `bundle`, which
/// must exist and must be quotable as one double-quoted shell word.
fn helper_path_in(bundle: &Path) -> Result<String, String> {
    let helper = bundle.join("Contents").join("MacOS").join(HELPER_NAME);
    if !helper.is_file() {
        return Err("the bundled status helper is missing from this build".into());
    }
    let text = helper
        .to_str()
        .ok_or("the application path is not valid UTF-8")?;
    if text
        .bytes()
        .any(|b| b < 0x20 || b == 0x7f || matches!(b, b'"' | b'$' | b'`' | b'\\' | b'!'))
    {
        return Err("the application path contains characters a hook command cannot quote".into());
    }
    Ok(text.to_string())
}

/// The helper of THIS install, only when deck runs from a release location
/// (`/Applications/deck.app` or `~/Applications/deck.app`). Development and
/// smoke bundles live at temporary paths and are ad-hoc signed; a hook that
/// named one would break when the path vanished and would make the agent
/// CLI execute an unsigned binary. Such builds cannot install hooks and
/// never touch the agent's config at boot.
fn installed_helper_path() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let bundle = crate::tmux_lifecycle::app_bundle_root(&exe)
        .filter(|bundle| crate::tmux_lifecycle::stable_installed_bundle(bundle))
        .ok_or("agent status hooks can only be installed from /Applications/deck.app or ~/Applications/deck.app")?;
    helper_path_in(bundle)
}

fn write_hooks(path: &Path, next: &serde_json::Value, never_ran: &str) -> Result<(), String> {
    let bytes = format!(
        "{}\n",
        serde_json::to_string_pretty(next).map_err(|e| e.to_string())?
    );
    write_agent_config(path, bytes.as_bytes(), never_ran)
}

/// Each agent module: config path, spec table, source word, hook style,
/// never-ran message.
type AgentModule = (
    Option<PathBuf>,
    &'static [HookSpec],
    &'static str,
    HookStyle,
    &'static str,
);

fn agent_modules() -> [AgentModule; 2] {
    [
        (
            claude_settings_path(),
            CLAUDE_HOOKS,
            "claude-code",
            HookStyle::Exec,
            "Claude Code has never run on this Mac — nothing to configure",
        ),
        (
            codex_hooks_path(),
            CODEX_HOOKS,
            "codex",
            HookStyle::ShellAsync,
            "Codex has never run on this Mac — nothing to configure",
        ),
    ]
}

/// Boot migration, the one write outside the Settings toggle: when hooks are
/// installed but are not what the CURRENT spec describes — a helper other
/// than this install's (the legacy `~/.deck/bin` copy, or a bundle that
/// moved), a matcher this deck version narrowed, an event it retired —
/// rewrite ONLY deck's entries, then delete the legacy copy once nothing
/// references it. Comparing the whole entry is what lets a spec change
/// (`permission_prompt|idle_prompt` → `permission_prompt`) reach users who
/// enabled the toggle under an older version without touching it again.
/// A non-release build returns before reading anything.
pub(crate) fn migrate_hooks_on_boot() {
    let Ok(helper) = installed_helper_path() else {
        return;
    };
    let mut legacy_referenced = false;
    for (path, specs, source, style, never_ran) in agent_modules() {
        let Some(path) = path else { continue };
        let Ok(value) = read_settings_value(&path) else {
            continue;
        };
        if !hooks_installed(&value, specs) {
            continue;
        }
        if hooks_are_current(&value, specs, source, style, &helper) {
            continue;
        }
        let result = hooks_with_install(value.clone(), specs, source, style, &helper)
            .and_then(|next| write_hooks(&path, &next, never_ran));
        match result {
            Ok(()) => applog(&format!(
                "[agent-hooks] {source} entries rewritten to the current spec"
            )),
            Err(e) => {
                legacy_referenced |= serde_json::to_string(&value)
                    .is_ok_and(|text| text.contains(LEGACY_HELPER_MARKER));
                applog(&format!(
                    "[agent-hooks] {source} migration FAILED ({})",
                    storage::err_code(&e)
                ));
            }
        }
    }
    if !legacy_referenced {
        retire_legacy_helper_copy();
    }
}

/// Remove the pre-0.5.12 `~/.deck/bin/deck-status-helper` copy (and the
/// directory if that left it empty). Nothing references it any more.
fn retire_legacy_helper_copy() {
    let Some(bin) = dirs::home_dir().map(|home| home.join(".deck").join("bin")) else {
        return;
    };
    let file = bin.join(HELPER_NAME);
    if !file.exists() {
        return;
    }
    match std::fs::remove_file(&file) {
        Ok(()) => {
            let _ = std::fs::remove_dir(&bin);
            applog("[agent-hooks] legacy helper copy removed");
        }
        Err(e) => applog(&format!(
            "[agent-hooks] legacy helper copy removal FAILED ({})",
            storage::err_code(&e.to_string())
        )),
    }
}

/// Shared enable/disable for one agent's hooks document.
fn hooks_set(
    path: &Path,
    specs: &[HookSpec],
    source: &str,
    style: HookStyle,
    enable: bool,
    never_ran: &str,
) -> Result<(), String> {
    let value = read_settings_value(path)?;
    let next = if enable {
        let helper = installed_helper_path()?;
        hooks_with_install(value, specs, source, style, &helper)?
    } else {
        if !path.exists() {
            return Ok(());
        }
        hooks_with_uninstall(value)
    };
    write_hooks(path, &next, never_ran)
}

#[derive(serde::Serialize)]
pub(crate) struct AgentHooksStatus {
    claude: bool,
    codex: bool,
}

#[tauri::command]
pub(crate) fn agent_hooks_status() -> AgentHooksStatus {
    let installed = |path: Option<PathBuf>, specs: &[HookSpec]| {
        path.and_then(|p| read_settings_value(&p).ok())
            .map(|v| hooks_installed(&v, specs))
            .unwrap_or(false)
    };
    AgentHooksStatus {
        claude: installed(claude_settings_path(), CLAUDE_HOOKS),
        codex: installed(codex_hooks_path(), CODEX_HOOKS),
    }
}

/// The user-driven writer of the agent CLI config files, from an explicit
/// Settings toggle. The only other writer is `migrate_hooks_on_boot`, which
/// rewrites deck's own entries and nothing else.
#[tauri::command]
pub(crate) fn agent_hooks_set(agent: String, enable: bool) -> Result<(), String> {
    let module = agent_modules()
        .into_iter()
        .find(|(_, _, source, _, _)| *source == agent)
        .ok_or("unknown agent")?;
    let (path, specs, source, style, never_ran) = module;
    let result = hooks_set(
        &path.ok_or("no home directory")?,
        specs,
        source,
        style,
        enable,
        never_ran,
    );
    match &result {
        Ok(()) => applog(&format!(
            "[agent-hooks] {agent} {}",
            if enable { "installed" } else { "removed" }
        )),
        Err(e) => applog(&format!(
            "[agent-hooks] {agent} {} FAILED ({})",
            if enable { "install" } else { "remove" },
            storage::err_code(e)
        )),
    }
    result
}

// ---------- tests -------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// The two tests below mutate the process-global AGENTS store; hold this
    /// across each so the default parallel test runner cannot interleave them.
    static STORE_TEST_LOCK: Mutex<()> = Mutex::new(());

    const HELPER: &str = "/Applications/deck.app/Contents/MacOS/deck-status-helper";

    fn event_line(state: &str, pane: &str) -> String {
        format!(
            "{{\"v\":1,\"source\":\"claude-code\",\"state\":\"{state}\",\"socket\":\"{}\",\"server_pid\":42,\"pane\":\"{pane}\"}}",
            crate::tmux::socket()
        )
    }

    #[test]
    fn events_are_validated_as_a_closed_shape() {
        assert!(parse_event(&event_line("working", "%3")).is_ok());
        assert_eq!(parse_event("not json"), Err("bad-json"));
        assert_eq!(parse_event("[1,2]"), Err("bad-json"));
        assert_eq!(
            parse_event(&event_line("working", "%3").replace("\"v\":1", "\"v\":2")),
            Err("bad-version")
        );
        assert_eq!(
            parse_event(&event_line("working", "%3").replace("claude-code", "mystery-agent")),
            Err("unknown-source")
        );
        assert_eq!(
            parse_event(&event_line("working", "%3").replace("working", "exfiltrate")),
            Err("unknown-state")
        );
        // an event from another deck server generation/socket is not ours
        assert_eq!(
            parse_event(&event_line("working", "%3").replace(crate::tmux::socket(), "deck-other")),
            Err("other-server")
        );
        assert_eq!(parse_event(&event_line("working", "%x")), Err("bad-pane"));
        assert_eq!(parse_event(&event_line("working", "3")), Err("bad-pane"));
    }

    #[test]
    fn pane_resolution_requires_the_server_generation() {
        let listing = "42\t%3\tdeck-card-ab12\tclaude\n42\t%5\tdeck-card-cd34\tzsh\n";
        assert_eq!(
            resolve_in(listing, "%3", 42),
            Some(("deck-card-ab12".into(), "claude".into()))
        );
        // same pane id, different server pid → a restarted server reused it
        assert_eq!(resolve_in(listing, "%3", 43), None);
        assert_eq!(resolve_in(listing, "%9", 42), None);
        // a session name outside the tmux alphabet never enters the store
        assert_eq!(resolve_in("42\t%3\tbad name\tclaude\n", "%3", 42), None);
    }

    #[test]
    fn codex_events_are_a_registered_source() {
        assert!(
            parse_event(&event_line("turn-done", "%3").replace("claude-code", "codex")).is_ok()
        );
    }

    #[test]
    fn hook_status_command_returns_both_closed_agent_fields() {
        let status = serde_json::to_value(agent_hooks_status()).unwrap();
        let object = status.as_object().unwrap();
        assert_eq!(object.len(), 2);
        assert!(object
            .get("claude")
            .is_some_and(serde_json::Value::is_boolean));
        assert!(object
            .get("codex")
            .is_some_and(serde_json::Value::is_boolean));
    }

    #[test]
    fn codex_hooks_install_covers_all_events_async_and_coexists() {
        // a user hooks.json with its own Stop hook and a description key
        let user = serde_json::json!({
            "description": "my hooks",
            "hooks": {
                "Stop": [
                    { "hooks": [{ "type": "command", "command": "terminal-notifier -message done" }] }
                ]
            }
        });
        let installed = hooks_with_install(
            user.clone(),
            CODEX_HOOKS,
            "codex",
            HookStyle::ShellAsync,
            HELPER,
        )
        .unwrap();
        assert!(hooks_installed(&installed, CODEX_HOOKS));
        assert_eq!(installed["description"], "my hooks");
        // the user's own Stop hook coexists with ours — no notify-style conflict
        assert_eq!(installed["hooks"]["Stop"].as_array().unwrap().len(), 2);
        let permission = &installed["hooks"]["PermissionRequest"][0]["hooks"][0];
        assert_eq!(
            permission["command"].as_str().unwrap(),
            "\"/Applications/deck.app/Contents/MacOS/deck-status-helper\" codex needs-input"
        );
        // fire-and-forget: every deck entry is async so it can never block a turn
        assert_eq!(permission["async"], serde_json::json!(true));
        assert_eq!(
            installed["hooks"]["Interrupt"][0]["hooks"][0]["command"]
                .as_str()
                .unwrap(),
            "\"/Applications/deck.app/Contents/MacOS/deck-status-helper\" codex turn-done"
        );

        // idempotent install, exact uninstall
        let twice = hooks_with_install(
            installed,
            CODEX_HOOKS,
            "codex",
            HookStyle::ShellAsync,
            HELPER,
        )
        .unwrap();
        assert_eq!(twice["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(hooks_with_uninstall(twice), user);

        // claude's spec set does not read as installed for codex and vice versa
        let claude_only = hooks_with_install(
            serde_json::json!({}),
            CLAUDE_HOOKS,
            "claude-code",
            HookStyle::Exec,
            HELPER,
        )
        .unwrap();
        assert!(!hooks_installed(&claude_only, CODEX_HOOKS));
    }

    #[test]
    fn ingest_reconcile_and_current_follow_the_pane_foreground() {
        let _guard = STORE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_for_tests();
        let resolve = |pane: &str, _pid: u32| {
            (pane == "%3").then(|| ("deck-card-ab12".to_string(), "claude".to_string()))
        };
        assert!(ingest(&event_line("needs-input", "%3"), resolve).is_ok());
        assert_eq!(current("deck-card-ab12"), Some("needs-input"));
        assert_eq!(current("deck-card-other"), None);
        assert_eq!(
            ingest(&event_line("working", "%9"), resolve),
            Err("no-such-pane")
        );

        let pane = |fg: &str| (7u32, 0u64, false, fg.to_string(), "/".to_string());
        // same foreground → state survives the poll
        let mut panes = HashMap::new();
        panes.insert("deck-card-ab12".to_string(), pane("claude"));
        reconcile(&panes);
        assert_eq!(current("deck-card-ab12"), Some("needs-input"));
        // foreground fell back to a shell → the agent exited → state cleared
        panes.insert("deck-card-ab12".to_string(), pane("zsh"));
        reconcile(&panes);
        assert_eq!(current("deck-card-ab12"), None);

        // a replaced foreground (different program) also clears
        assert!(ingest(&event_line("working", "%3"), resolve).is_ok());
        panes.insert("deck-card-ab12".to_string(), pane("vim"));
        reconcile(&panes);
        assert_eq!(current("deck-card-ab12"), None);

        // a vanished session clears
        assert!(ingest(&event_line("working", "%3"), resolve).is_ok());
        reconcile(&HashMap::new());
        assert_eq!(current("deck-card-ab12"), None);

        // a shell foreground at event time is refused outright
        let shell_resolve =
            |_: &str, _: u32| Some(("deck-card-ab12".to_string(), "zsh".to_string()));
        assert_eq!(
            ingest(&event_line("working", "%3"), shell_resolve),
            Err("shell-foreground")
        );
        assert_eq!(current("deck-card-ab12"), None);
        reset_for_tests();
    }

    #[test]
    fn listener_accepts_a_real_socket_line() {
        use std::io::Write;
        let _guard = STORE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_for_tests();
        let dir = std::env::temp_dir().join(format!("deck-status-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("status.sock");
        let listener = listen_at(&path).unwrap();
        let line = event_line("turn-done", "%3");
        let mut client = UnixStream::connect(&path).unwrap();
        writeln!(client, "{line}").unwrap();
        drop(client);
        let (stream, _) = listener.accept().unwrap();
        // the real read path; a fake resolver stands in for the tmux server
        let mut stream = stream;
        let read = read_first_line(&mut stream).unwrap();
        assert_eq!(read, line);
        assert!(ingest(&read, |_, _| Some((
            "deck-card-ab12".into(),
            "claude".into()
        )))
        .is_ok());
        assert_eq!(current("deck-card-ab12"), Some("turn-done"));
        // rebinding over a stale socket file must work (previous run crashed)
        drop(listener);
        let listener2 = listen_at(&path).unwrap();
        drop(listener2);
        let _ = std::fs::remove_dir_all(&dir);
        reset_for_tests();
    }

    #[test]
    fn hook_install_is_idempotent_and_preserves_foreign_config() {
        let user = serde_json::json!({
            "model": "opus",
            "hooks": {
                "Stop": [
                    { "hooks": [{ "type": "command", "command": "afplay /System/Library/Sounds/Glass.aiff" }] }
                ],
                "PreToolUse": [
                    { "matcher": "Bash", "hooks": [{ "type": "command", "command": "my-guard" }] }
                ]
            }
        });
        let installed = hooks_with_install(
            user.clone(),
            CLAUDE_HOOKS,
            "claude-code",
            HookStyle::Exec,
            HELPER,
        )
        .unwrap();
        assert!(hooks_installed(&installed, CLAUDE_HOOKS));
        assert_eq!(installed["model"], "opus");
        // the user's own Stop hook and PreToolUse guard survive
        assert_eq!(installed["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(
            installed["hooks"]["PreToolUse"].as_array().unwrap().len(),
            1
        );
        // exec form: the helper path alone in `command`, words in `args`,
        // so Claude Code spawns the helper without a shell
        let submit = &installed["hooks"]["UserPromptSubmit"][0]["hooks"][0];
        assert_eq!(submit["command"], HELPER);
        assert_eq!(
            submit["args"],
            serde_json::json!(["claude-code", "working"])
        );
        assert_eq!(submit["type"], "command");
        assert!(submit.get("async").is_none());
        // only a mid-turn permission question is "needs-input"; an idle
        // prompt after a finished turn stays turn-done
        assert_eq!(
            installed["hooks"]["Notification"][0]["matcher"],
            "permission_prompt"
        );

        // installing twice does not duplicate
        let twice = hooks_with_install(
            installed.clone(),
            CLAUDE_HOOKS,
            "claude-code",
            HookStyle::Exec,
            HELPER,
        )
        .unwrap();
        assert_eq!(twice["hooks"]["Stop"].as_array().unwrap().len(), 2);

        // uninstall restores the user's document exactly
        let removed = hooks_with_uninstall(twice);
        assert!(!hooks_installed(&removed, CLAUDE_HOOKS));
        assert_eq!(removed, user);

        // uninstalling a never-installed file is a no-op
        let empty = hooks_with_uninstall(serde_json::json!({ "model": "opus" }));
        assert_eq!(empty, serde_json::json!({ "model": "opus" }));

        // a hooks-only file empties back to no hooks key at all
        let only_ours = hooks_with_install(
            serde_json::json!({}),
            CLAUDE_HOOKS,
            "claude-code",
            HookStyle::Exec,
            HELPER,
        )
        .unwrap();
        assert_eq!(hooks_with_uninstall(only_ours), serde_json::json!({}));
    }

    #[test]
    fn hook_install_refuses_malformed_documents() {
        let install =
            |v| hooks_with_install(v, CLAUDE_HOOKS, "claude-code", HookStyle::Exec, HELPER);
        assert!(install(serde_json::json!([1, 2])).is_err());
        assert!(install(serde_json::json!({ "hooks": "nope" })).is_err());
        assert!(install(serde_json::json!({ "hooks": { "Stop": "nope" } })).is_err());
        // partial installs read as OFF so re-enabling repairs them
        let partial = serde_json::json!({
            "hooks": { "Stop": [{ "hooks": [{ "type": "command",
                "command": "\"$HOME/.deck/bin/deck-status-helper\" claude-code turn-done" }] }] }
        });
        assert!(!hooks_installed(&partial, CLAUDE_HOOKS));
    }

    #[test]
    fn stale_entries_are_ours_and_get_rewritten_to_the_current_spec() {
        let legacy_command =
            |state: &str| format!("\"$HOME/.deck/bin/deck-status-helper\" claude-code {state}");
        let legacy = serde_json::json!({
            "model": "opus",
            "hooks": {
                "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": legacy_command("working"), "timeout": 10 }] }],
                "Notification": [{ "matcher": "permission_prompt|idle_prompt", "hooks": [{ "type": "command", "command": legacy_command("needs-input"), "timeout": 10 }] }],
                "Stop": [
                    { "hooks": [{ "type": "command", "command": "afplay /System/Library/Sounds/Glass.aiff" }] },
                    { "hooks": [{ "type": "command", "command": legacy_command("turn-done"), "timeout": 10 }] }
                ]
            }
        });
        // a legacy install still reads as installed, but not as current:
        // the helper path is stale AND the matcher predates the narrowing
        assert!(hooks_installed(&legacy, CLAUDE_HOOKS));
        assert!(!hooks_are_current(
            &legacy,
            CLAUDE_HOOKS,
            "claude-code",
            HookStyle::Exec,
            HELPER
        ));

        // migration = the ordinary install over the legacy document
        let migrated = hooks_with_install(
            legacy.clone(),
            CLAUDE_HOOKS,
            "claude-code",
            HookStyle::Exec,
            HELPER,
        )
        .unwrap();
        assert!(hooks_are_current(
            &migrated,
            CLAUDE_HOOKS,
            "claude-code",
            HookStyle::Exec,
            HELPER
        ));
        assert_eq!(migrated["model"], "opus");
        assert_eq!(migrated["hooks"]["Stop"].as_array().unwrap().len(), 2);
        let text = serde_json::to_string(&migrated).unwrap();
        assert!(!text.contains(".deck/bin"), "no legacy path survives");
        assert!(text.contains(HELPER));
        // the stale wide matcher is gone — an idle prompt no longer decays a
        // finished card into "needs-input"
        assert_eq!(
            migrated["hooks"]["Notification"][0]["matcher"],
            "permission_prompt"
        );

        // a bundle that moved is equally not current
        let moved = "/Users/x/Applications/deck.app/Contents/MacOS/deck-status-helper";
        assert!(!hooks_are_current(
            &migrated,
            CLAUDE_HOOKS,
            "claude-code",
            HookStyle::Exec,
            moved
        ));
        assert!(hooks_are_current(
            &hooks_with_install(
                migrated.clone(),
                CLAUDE_HOOKS,
                "claude-code",
                HookStyle::Exec,
                moved
            )
            .unwrap(),
            CLAUDE_HOOKS,
            "claude-code",
            HookStyle::Exec,
            moved
        ));

        // a shell-form entry with the right path is not current for Claude
        // Code (exec form expected) but is for Codex
        let shell_form = hooks_with_install(
            serde_json::json!({}),
            CLAUDE_HOOKS,
            "claude-code",
            HookStyle::ShellAsync,
            HELPER,
        )
        .unwrap();
        assert!(!hooks_are_current(
            &shell_form,
            CLAUDE_HOOKS,
            "claude-code",
            HookStyle::Exec,
            HELPER
        ));
        assert!(hooks_are_current(
            &shell_form,
            CLAUDE_HOOKS,
            "claude-code",
            HookStyle::ShellAsync,
            HELPER
        ));
        let exec_form = hooks_with_install(
            shell_form,
            CLAUDE_HOOKS,
            "claude-code",
            HookStyle::Exec,
            HELPER,
        )
        .unwrap();
        assert!(hooks_are_current(
            &exec_form,
            CLAUDE_HOOKS,
            "claude-code",
            HookStyle::Exec,
            HELPER
        ));
        assert!(!serde_json::to_string(&exec_form).unwrap().contains("\\\""));

        // uninstall removes legacy and current entries alike
        let removed = hooks_with_uninstall(legacy);
        assert_eq!(removed["hooks"]["Stop"].as_array().unwrap().len(), 1);
        assert!(!hooks_installed(&removed, CLAUDE_HOOKS));
    }

    /// Boot migration rewrites whenever the document is not current, so
    /// install's OWN output must be current for every module — otherwise
    /// deck would rewrite the user's agent config on every launch. One
    /// spec event carrying two specs is exactly how that would happen.
    #[test]
    fn installing_any_module_leaves_a_document_the_migration_will_not_touch() {
        for (specs, source, style) in [
            (CLAUDE_HOOKS, "claude-code", HookStyle::Exec),
            (CODEX_HOOKS, "codex", HookStyle::ShellAsync),
        ] {
            let mut events: Vec<&str> = specs.iter().map(|(e, _, _)| *e).collect();
            events.sort_unstable();
            let unique = events.len();
            events.dedup();
            assert_eq!(events.len(), unique, "{source}: one spec per event");

            let doc = hooks_with_install(
                serde_json::json!({ "model": "opus" }),
                specs,
                source,
                style,
                HELPER,
            )
            .unwrap();
            assert!(hooks_installed(&doc, specs), "{source}");
            assert!(
                hooks_are_current(&doc, specs, source, style, HELPER),
                "{source}: a fresh install must not be stale"
            );
        }
    }

    /// The migration's whole point after 0.5.13: a user who enabled the
    /// toggle under an older spec gets the narrowed matcher without touching
    /// the Settings switch, and a retired event leaves nothing behind.
    #[test]
    fn a_spec_change_alone_makes_installed_hooks_stale() {
        let current = hooks_with_install(
            serde_json::json!({}),
            CLAUDE_HOOKS,
            "claude-code",
            HookStyle::Exec,
            HELPER,
        )
        .unwrap();
        let is_current = |doc: &serde_json::Value| {
            hooks_are_current(doc, CLAUDE_HOOKS, "claude-code", HookStyle::Exec, HELPER)
        };
        assert!(is_current(&current));

        // same helper, same events, only the matcher predates the narrowing
        let mut wide = current.clone();
        wide["hooks"]["Notification"][0]["matcher"] =
            serde_json::json!("permission_prompt|idle_prompt");
        assert!(hooks_installed(&wide, CLAUDE_HOOKS), "still installed");
        assert!(!is_current(&wide), "a matcher change is a stale install");
        assert!(is_current(
            &hooks_with_install(wide, CLAUDE_HOOKS, "claude-code", HookStyle::Exec, HELPER)
                .unwrap()
        ));

        // an event this deck version no longer registers: not current, and
        // install drops the husk instead of leaving a dead hook behind
        let mut retired = current.clone();
        retired["hooks"]["SessionStart"] = serde_json::json!([spec_entry(
            HookStyle::Exec,
            HELPER,
            "claude-code",
            &("SessionStart", None, "working")
        )]);
        assert!(!is_current(&retired));
        let repaired = hooks_with_install(
            retired,
            CLAUDE_HOOKS,
            "claude-code",
            HookStyle::Exec,
            HELPER,
        )
        .unwrap();
        assert!(is_current(&repaired));
        assert!(repaired["hooks"].get("SessionStart").is_none());
        assert_eq!(repaired, current);

        // a duplicate deck entry (a hand-edited file) is stale, and install
        // collapses it back to exactly one
        let mut doubled = current.clone();
        let entry = doubled["hooks"]["Stop"][0].clone();
        doubled["hooks"]["Stop"].as_array_mut().unwrap().push(entry);
        assert!(!is_current(&doubled));
        assert_eq!(
            hooks_with_install(
                doubled,
                CLAUDE_HOOKS,
                "claude-code",
                HookStyle::Exec,
                HELPER
            )
            .unwrap(),
            current
        );

        // an EMPTY array the user wrote is theirs — install never prunes it
        let user_empty = hooks_with_install(
            serde_json::json!({ "hooks": { "PreToolUse": [] } }),
            CLAUDE_HOOKS,
            "claude-code",
            HookStyle::Exec,
            HELPER,
        )
        .unwrap();
        assert_eq!(user_empty["hooks"]["PreToolUse"], serde_json::json!([]));
    }

    #[test]
    fn helper_path_requires_a_present_quotable_sidecar_in_a_release_location() {
        let dir = std::env::temp_dir().join(format!("deck-hooks-{}", std::process::id()));
        let bundle = dir.join("deck.app");
        let macos = bundle.join("Contents").join("MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        assert!(helper_path_in(&bundle).is_err(), "missing sidecar");
        std::fs::write(macos.join(HELPER_NAME), "binary").unwrap();
        let helper = helper_path_in(&bundle).unwrap();
        assert!(helper.ends_with("/deck.app/Contents/MacOS/deck-status-helper"));
        assert!(helper.contains(HELPER_MARKER));
        assert_eq!(
            hook_value(HookStyle::ShellAsync, &helper, "codex", "working")["command"],
            format!("\"{helper}\" codex working")
        );
        assert_eq!(
            hook_value(HookStyle::Exec, &helper, "claude-code", "working")["command"],
            helper
        );

        let quoted = dir.join("we\"ird").join("deck.app");
        let quoted_macos = quoted.join("Contents").join("MacOS");
        std::fs::create_dir_all(&quoted_macos).unwrap();
        std::fs::write(quoted_macos.join(HELPER_NAME), "binary").unwrap();
        assert!(helper_path_in(&quoted).is_err(), "unquotable path");
        let _ = std::fs::remove_dir_all(&dir);

        // the test binary is not a release install: no hook may name it
        let error = installed_helper_path().unwrap_err();
        assert!(error.contains("/Applications/deck.app"));
    }
}
