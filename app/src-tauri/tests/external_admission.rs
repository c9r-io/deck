//! External-text admission census. Regression ba60adc: Slack badge rules were
//! a second entry path that queued someone else's message through the owner
//! queue, so it could be pasted into zsh and run. Text that did not come from
//! the local user's own keyboard, voice or saved configuration reaches a
//! terminal only through ONE admission (`scheduler::ops::admit_external`: the
//! card command is exactly `claude`/`codex`, the row is marked `external`),
//! a verbatim external entry through `validate_add`'s `external_text` gate
//! (same agent check plus the leading `!`/`/`/`#` refusal), and the phone's
//! direct send-message through the same agent predicate
//! (`connector::require_agent_card`). This file makes a new path fail CI:
//!
//! 1. every production site that creates a queue item (`QueueItem {`,
//!    `items.push(`...) is pinned to an exact file + function + count and
//!    labelled: the one admission core, a row derived from an existing row
//!    (inherits its mark), or crash recovery of a persisted snapshot;
//! 2. the admission core's callers, `admit_external`, `validate_add` and the
//!    writers of a row's origin fields are pinned, and each external entry
//!    must reach the chokepoint;
//! 3. every `#[tauri::command]` whose parameters can carry text is pinned
//!    with a role (external / owner queue / owner terminal / not terminal);
//! 4. every backend terminal-input site (literal delivery, tmux paste/load
//!    buffers, text `send-keys`, the PTY writer) is pinned and labelled;
//! 5. every frontend call of those commands is pinned per JS file and
//!    function; plain `queue_add*` next to external text must carry its
//!    guard (`clock ?`, `externalText`).
//!
//! Unused entries fail, like `edr_quiet.rs`. Scope decisions: clock rules
//! are OWNER text (the template and command are the rule owner's; a clock
//! event's `msg.text` is the rule's own name). MCP is out of scope for this
//! admission: its input reaches only the signed runner's job stdin under the
//! local user's per-client execution and stdin grants (`mcp/`, a separate
//! authority), and census 1–5 would flag it the moment `mcp/`/`mcp.js`
//! queued text or wrote to a pane. A card's launch command is configuration
//! (the owner's rule, preset or card), not message text. The Connector
//! phone is the owner's paired device; its rows still take the external
//! path, and a buffer entry it wrote as `manual` is owner text once the
//! desktop user queues it.

mod source_scan;
use source_scan::{enclosing_function, is_ident, manifest, production_sources};

// ---------------------------------------------------------------- scanning

/// Byte offsets of `token` in `source`, skipping `//` comment lines.
fn token_sites(source: &str, token: &str) -> Vec<usize> {
    source
        .match_indices(token)
        .map(|(at, _)| at)
        .filter(|&at| {
            let line_start = source[..at].rfind('\n').map_or(0, |n| n + 1);
            !source[line_start..at].trim_start().starts_with("//")
        })
        .collect()
}

/// Byte span of the brace-balanced body opened after `from` (string and
/// char literals skipped; a lifetime is not a char literal).
fn body_span(source: &str, from: usize) -> Option<(usize, usize)> {
    let open = from + source[from..].find('{')?;
    let bytes = source.as_bytes();
    let (mut depth, mut i) = (0usize, open);
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
            }
            b'\'' if bytes.get(i + 1) == Some(&b'\\') => {
                i += 2;
                while i < bytes.len() && bytes[i] != b'\'' {
                    i += 1;
                }
            }
            b'\'' if bytes.get(i + 2) == Some(&b'\'') => i += 2,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((open, i));
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Every `fn` declaration: (name, body span).
fn functions(source: &str) -> Vec<(String, (usize, usize))> {
    source
        .match_indices("fn ")
        .filter(|(at, _)| !source[..*at].chars().next_back().is_some_and(is_ident))
        .filter_map(|(at, _)| {
            let name: String = source[at + 3..]
                .chars()
                .take_while(|c| is_ident(*c))
                .collect();
            let span = body_span(source, at)?;
            (!name.is_empty()).then_some((name, span))
        })
        .collect()
}

/// The innermost `fn` whose body holds `at`: unlike `enclosing_function`,
/// a nested helper (`impl Drop { fn drop }`) that closed before the site
/// does not capture it.
fn function_at(source: &str, at: usize) -> String {
    functions(source)
        .into_iter()
        .rfind(|(_, (open, close))| *open < at && at <= *close)
        .map(|(name, _)| name)
        .unwrap_or_else(|| enclosing_function(source, at))
}

/// The body of `fn name`.
fn function_body<'a>(source: &'a str, name: &str) -> Option<&'a str> {
    functions(source)
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, (open, close))| &source[open..=close])
}

fn body(file: &str, function: &str) -> String {
    let sources = production_sources();
    let (_, source) = sources
        .iter()
        .find(|(name, _)| name == file)
        .unwrap_or_else(|| panic!("{file} is a production source"));
    function_body(source, function)
        .unwrap_or_else(|| panic!("fn {function} exists in {file}"))
        .to_string()
}

/// (file, function, token) → count over every production source.
type Census = Vec<(String, String, String, usize)>;

fn census(tokens: &[&str], skip: impl Fn(&str, usize) -> bool) -> Census {
    let mut out: Census = Vec::new();
    for (file, source) in production_sources() {
        for token in tokens {
            for at in token_sites(&source, token) {
                if skip(&source, at) {
                    continue;
                }
                let function = function_at(&source, at);
                match out
                    .iter_mut()
                    .find(|(f, n, t, _)| *f == file && *n == function && t == token)
                {
                    Some(entry) => entry.3 += 1,
                    None => out.push((file.clone(), function, token.to_string(), 1)),
                }
            }
        }
    }
    out
}

/// Every census site must be an allowlist entry with the exact count, and
/// every allowlist entry must still be used.
fn assert_pinned<R: std::fmt::Debug>(
    what: &str,
    found: &Census,
    allowed: &[(&str, &str, &str, usize, R)],
) {
    let mut violations = Vec::new();
    for (file, function, token, count) in found {
        match allowed
            .iter()
            .find(|(f, n, t, _, _)| f == file && n == function && t == token)
        {
            Some((_, _, _, expected, _)) if expected == count => {}
            Some((_, _, _, expected, role)) => violations.push(format!(
                "{what}: {file} fn {function} has {count} `{token}` site(s), reviewed {expected} ({role:?})"
            )),
            None => violations.push(format!(
                "{what}: unreviewed `{token}` in {file} fn {function} — route external text \
                 through `admit_external` / `channel_queue_add*` and add a labelled entry here"
            )),
        }
    }
    for (file, function, token, _, role) in allowed {
        if !found
            .iter()
            .any(|(f, n, t, _)| f == file && n == function && t == token)
        {
            violations.push(format!(
                "{what}: stale entry ({file}, {function}, {token}, {role:?})"
            ));
        }
    }
    assert!(violations.is_empty(), "{violations:#?}");
}

// ------------------------------------------------------- 1. queue creation

#[derive(Debug)]
enum Creation {
    /// `add_item_bound`: sets `external` from the admission marks.
    Admission,
    /// A row spawned from, or a checkpoint copied from, an existing row.
    Inherits,
    /// Crash recovery re-installs a persisted ledger snapshot.
    Restores,
}

const CREATION_TOKENS: &[&str] = &[
    "QueueItem {",
    "items.push(",
    "items.insert(",
    "items.extend(",
    "items.append(",
    "items.splice(",
];

const QUEUE_CREATION: &[(&str, &str, &str, usize, Creation)] = &[
    (
        "scheduler/ops.rs",
        "add_item_bound",
        "QueueItem {",
        1,
        Creation::Admission,
    ),
    (
        "scheduler/ops.rs",
        "add_item_bound",
        "items.push(",
        1,
        Creation::Admission,
    ),
    (
        "scheduler/delivery.rs",
        "finalize_delivery",
        "QueueItem {",
        1,
        Creation::Inherits,
    ),
    (
        "scheduler/delivery.rs",
        "finalize_delivery",
        "items.push(",
        2,
        Creation::Inherits,
    ),
    (
        "scheduler/delivery.rs",
        "recover_interrupted",
        "items.push(",
        1,
        Creation::Restores,
    ),
];

#[test]
fn every_queue_item_creation_site_is_reviewed() {
    let found = census(CREATION_TOKENS, |source, at| {
        source[..at].ends_with("struct ")
    });
    assert_pinned("queue creation", &found, QUEUE_CREATION);
    for (file, function, _, _, role) in QUEUE_CREATION {
        let text = body(file, function);
        let required = match role {
            Creation::Admission => "external: args.channel_path || args.external_text",
            Creation::Inherits => "external: item.external",
            Creation::Restores => "pending.snapshot",
        };
        assert!(
            text.contains(required),
            "{file} fn {function} ({role:?}) must keep `{required}`"
        );
    }
}

// ----------------------------------------------------- 2. the chokepoint

#[derive(Debug)]
enum Entry {
    /// A `#[tauri::command]` owner path: must run `validate_add`, which
    /// carries the verbatim `external_text` gate.
    Owner,
    /// Compiled for unit tests only.
    TestOnly,
    /// The one external admission.
    Admission,
    /// `validate_add`'s verbatim-entry gate.
    VerbatimGate,
    /// Crash-safe migration: fills a MISSING expected process only.
    Migration,
}

const ADMISSION_SITES: &[(&str, &str, &str, usize, Entry)] = &[
    (
        "scheduler/ops.rs",
        "add_item",
        "add_item_bound(",
        1,
        Entry::TestOnly,
    ),
    (
        "scheduler/ops.rs",
        "queue_add",
        "add_item_bound(",
        1,
        Entry::Owner,
    ),
    (
        "scheduler/ops.rs",
        "queue_add_reviewed_list",
        "add_item_bound(",
        1,
        Entry::Owner,
    ),
    (
        "scheduler/ops.rs",
        "admit_external",
        "require_channel_agent(",
        1,
        Entry::Admission,
    ),
    (
        "scheduler/ops.rs",
        "validate_add",
        "require_channel_agent(",
        1,
        Entry::VerbatimGate,
    ),
    (
        "scheduler/ops.rs",
        "admit_external",
        "channel_path = true",
        1,
        Entry::Admission,
    ),
    (
        "scheduler/mod.rs",
        "migrate_context",
        "expected_process = ",
        1,
        Entry::Migration,
    ),
];

#[test]
fn external_text_reaches_the_one_admission() {
    let found = census(
        &[
            "add_item_bound(",
            "require_channel_agent(",
            "channel_path = true",
            "expected_process = ",
            "expected_process: None",
            ".external = ",
        ],
        |source, at| source[..at].ends_with("fn ") || source[..at].ends_with("let "),
    );
    assert_pinned("admission", &found, ADMISSION_SITES);

    let ops = production_sources()
        .into_iter()
        .find(|(name, _)| name == "scheduler/ops.rs")
        .unwrap()
        .1;
    assert!(
        ops.contains("#[cfg(test)]\npub(crate) fn add_item("),
        "add_item stays test-only"
    );
    for owner in ["queue_add", "queue_add_reviewed_list"] {
        assert!(
            body("scheduler/ops.rs", owner).contains("validate_add("),
            "{owner} must run validate_add (the externalText gate) before the core"
        );
    }
    let admit = body("scheduler/ops.rs", "admit_external");
    assert!(admit.contains("require_channel_agent(args)?;"));
    assert!(body("scheduler/ops.rs", "require_channel_agent")
        .contains("crate::admission::channel_agent_command(&args.cmd).is_none()"));
    let validate = body("scheduler/ops.rs", "validate_add");
    let gate = validate
        .split("if a.external_text {")
        .nth(1)
        .expect("validate_add gates external_text");
    assert!(
        gate.contains("require_channel_agent(a)?;") && gate.contains("leading_command(&a.text)")
    );
    assert!(body("scheduler/mod.rs", "migrate_context")
        .contains("if item.expected_process.is_none() {"));
}

// ------------------------------------------- 3. text-carrying Tauri commands

#[derive(Debug, PartialEq)]
enum Command {
    /// Must call `admit_external` before the owner core.
    External,
    /// Owner text into the queue; a verbatim entry declares `externalText`.
    OwnerQueue,
    /// Edits an existing row's text; its origin marks stay.
    OwnerEdit,
    /// The local user's keystrokes, voice, or saved launch command.
    OwnerTerminal,
    /// Carries text that never reaches a pane.
    NotTerminal,
}

const TEXT_PARAMS: &[&str] = &[
    "QueueAddArgs",
    "text:",
    "texts:",
    "steps:",
    "data:",
    "data_b64:",
    "cmd:",
    "prompt:",
    "message:",
    "input:",
    "keys:",
];

const TEXT_COMMANDS: &[(&str, &str, Command)] = &[
    ("scheduler/ops.rs", "channel_queue_add", Command::External),
    (
        "scheduler/ops.rs",
        "channel_queue_add_reviewed_list",
        Command::External,
    ),
    ("scheduler/ops.rs", "queue_add", Command::OwnerQueue),
    (
        "scheduler/ops.rs",
        "queue_add_reviewed_list",
        Command::OwnerQueue,
    ),
    ("scheduler/ops.rs", "queue_update", Command::OwnerEdit),
    ("pty.rs", "pty_write", Command::OwnerTerminal),
    ("voice.rs", "voice_deliver", Command::OwnerTerminal),
    ("commands.rs", "start_session", Command::OwnerTerminal),
    ("commands.rs", "write_clipboard", Command::NotTerminal),
    ("history.rs", "record_command", Command::NotTerminal),
    ("drops.rs", "save_dropped_file", Command::NotTerminal),
    ("documents.rs", "save_board", Command::NotTerminal),
    ("documents.rs", "save_settings", Command::NotTerminal),
    ("diagnostics.rs", "ui_event", Command::NotTerminal),
];

/// (file, name, parameter list) of every production `#[tauri::command]`.
fn tauri_commands() -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for (file, source) in production_sources() {
        for (at, _) in source.match_indices("#[tauri::command]") {
            let rest = &source[at..];
            let fn_at = rest.find("fn ").expect("command fn");
            let name: String = rest[fn_at + 3..]
                .chars()
                .take_while(|c| is_ident(*c))
                .collect();
            let params = &rest[fn_at..fn_at + rest[fn_at..].find('{').expect("body")];
            out.push((file.clone(), name, params.to_string()));
        }
    }
    out
}

#[test]
fn every_text_carrying_tauri_command_is_reviewed() {
    let mut violations = Vec::new();
    let mut seen = Vec::new();
    for (file, name, params) in tauri_commands() {
        if !TEXT_PARAMS.iter().any(|p| params.contains(p)) {
            continue;
        }
        match TEXT_COMMANDS
            .iter()
            .find(|(f, n, _)| *f == file && *n == name)
        {
            Some(entry) => seen.push(entry),
            None => violations.push(format!(
                "unreviewed text-carrying command {file} fn {name}: label it here; external \
                 text must enter through `admit_external`"
            )),
        }
    }
    for entry in TEXT_COMMANDS {
        if !seen.iter().any(|s| std::ptr::eq(*s, entry)) {
            violations.push(format!("stale command entry {entry:?}"));
        }
    }
    assert!(violations.is_empty(), "{violations:#?}");
    for (file, name, role) in TEXT_COMMANDS {
        let text = body(file, name);
        match role {
            Command::External => assert!(
                text.contains("admit_external(&mut args)?;"),
                "{name} must pass admit_external first"
            ),
            Command::OwnerQueue => assert!(text.contains("validate_add(")),
            _ => assert!(
                !text.contains("admit_external(") && !text.contains("channel_path"),
                "{name} is not an admission path"
            ),
        }
    }
}

// ----------------------------------------------- 4. terminal-input sites

#[derive(Debug)]
enum Input {
    /// The scheduler pasting an already admitted row.
    Queue,
    /// The phone's direct send-message: the channel agent predicate plus the
    /// live agent foreground and the MCP fence.
    External,
    /// The local user's voice or keystrokes.
    Owner,
    /// A saved launch command, or deck's own fixed key.
    Config,
    /// The shared literal-paste primitive every row above uses.
    Primitive,
    /// Restored transcript bytes written to the pane as OUTPUT.
    Display,
}

const INPUT_TOKENS: &[&str] = &[
    "prompt_delivery::deliver(",
    "prompt_delivery::deliver_with(",
    "paste-buffer",
    "load-buffer",
    "set-buffer",
    "send-prefix",
    "send-keys",
    "take_writer(",
    ".write_all(",
];

const TERMINAL_INPUT: &[(&str, &str, &str, usize, Input)] = &[
    (
        "scheduler/delivery.rs",
        "fire_item",
        "prompt_delivery::deliver(",
        1,
        Input::Queue,
    ),
    (
        "connector/native.rs",
        "execute_native",
        "prompt_delivery::deliver_with(",
        1,
        Input::External,
    ),
    (
        "voice.rs",
        "deliver_with",
        "prompt_delivery::deliver_with(",
        1,
        Input::Owner,
    ),
    ("pty.rs", "attach_session", "take_writer(", 1, Input::Owner),
    ("pty.rs", "pty_write", ".write_all(", 1, Input::Owner),
    (
        "prompt_delivery.rs",
        "deliver_with",
        "paste-buffer",
        1,
        Input::Primitive,
    ),
    (
        "prompt_delivery.rs",
        "deliver_with",
        "load-buffer",
        1,
        Input::Primitive,
    ),
    (
        "prompt_delivery.rs",
        "deliver_with",
        "send-keys",
        1,
        Input::Primitive,
    ),
    (
        "commands.rs",
        "start_session",
        "send-keys",
        1,
        Input::Config,
    ),
    ("restart.rs", "exit_keys", "send-keys", 1, Input::Config),
    (
        "commands.rs",
        "restore_start_args",
        "load-buffer",
        1,
        Input::Display,
    ),
];

/// `send-keys -X` drives copy mode (scroll, selection) and types nothing:
/// `-X` must follow within the same argument list (up to its `]`) or the
/// same tmux command (up to the next `send-keys`).
fn copy_mode_keys(source: &str, at: usize) -> bool {
    let rest = &source[at + "send-keys".len()..];
    let window = &rest[..rest.len().min(200)];
    let window = window.split("send-keys").next().unwrap_or(window);
    let window = window.split(']').next().unwrap_or(window);
    window.contains("-X")
}

#[test]
fn every_terminal_input_site_is_reviewed() {
    let found = census(INPUT_TOKENS, |source, at| {
        copy_mode_keys(source, at)
            // `.write_all(` is a terminal write only on the PTY writer.
            || (source[at..].starts_with(".write_all(") && !source.contains("take_writer("))
    });
    assert_pinned("terminal input", &found, TERMINAL_INPUT);
    // The phone's direct path uses the same agent predicate as the queue.
    let native = body("connector/native.rs", "execute_native");
    let send = native
        .split("\"send-message\" =>")
        .nth(1)
        .and_then(|tail| tail.split("prompt_delivery::deliver_with(").next())
        .expect("send-message delivery");
    for required in [
        "require_agent_card(&card)",
        "guard_terminal_input(&card.session)",
        ".agent",
    ] {
        assert!(
            send.contains(required),
            "Connector send-message must check {required} before delivery"
        );
    }
    assert!(native.contains("expected_process: Some(agent)"));
    assert!(body("connector/projection.rs", "queue_target_supported")
        .contains("crate::admission::channel_agent_command"));
    assert!(body("scheduler/delivery.rs", "fire_item").contains("literal_request(item"));
    // No import may alias the delivery entry points past the census.
    for (file, source) in production_sources() {
        for (at, _) in source.match_indices("use crate::prompt_delivery::") {
            let names = source[at + "use crate::prompt_delivery::".len()..]
                .split(';')
                .next()
                .unwrap_or("");
            assert!(
                !names
                    .split(|c: char| !is_ident(c))
                    .any(|name| name == "deliver" || name == "deliver_with"),
                "{file}: import prompt_delivery by module, not `{names}`"
            );
        }
    }
}

// --------------------------------------------------- 5. frontend call sites

const FRONTEND_COMMANDS: &[&str] = &[
    "channel_queue_add",
    "channel_queue_add_reviewed_list",
    "queue_add",
    "queue_add_reviewed_list",
    "queue_update",
    "pty_write",
    "voice_deliver",
];

/// (file, function, command, count, guard): every call's statement must
/// contain `guard` when one is given.
type FrontendSite = (
    &'static str,
    &'static str,
    &'static str,
    usize,
    &'static str,
);

const FRONTEND_SITES: &[FrontendSite] = &[
    // Slack channel rule and Connector task runs: template rows around
    // external text; the whole run takes the external path.
    ("board.js", "queueChannelPlan", "channel_queue_add", 1, ""),
    ("board.js", "queueConnectorPlan", "channel_queue_add", 1, ""),
    // Desktop scratchpad: an external entry declares itself verbatim.
    (
        "board.js",
        "queueBufferEntries",
        "queue_add",
        1,
        "...(copy.external ? { externalText: true } : {})",
    ),
    // Phone scratchpad: always external, verbatim entries also declared.
    (
        "connector.js",
        "queueBuffer",
        "channel_queue_add",
        1,
        "...(item.external ? { externalText: true } : {})",
    ),
    // Badge rules are external; only a clock rule takes the owner command.
    (
        "inbound.js",
        "handleInbound",
        "queue_add_reviewed_list",
        1,
        "clock ? 'queue_add_reviewed_list' : 'channel_queue_add_reviewed_list'",
    ),
    (
        "inbound.js",
        "handleInbound",
        "channel_queue_add_reviewed_list",
        1,
        "clock ? 'queue_add_reviewed_list' : 'channel_queue_add_reviewed_list'",
    ),
    (
        "inbound.js",
        "handleInbound",
        "queue_add",
        1,
        "clock ? 'queue_add' : 'channel_queue_add'",
    ),
    (
        "inbound.js",
        "handleInbound",
        "channel_queue_add",
        1,
        "clock ? 'queue_add' : 'channel_queue_add'",
    ),
    // The ⏱ panel and saved templates: the user's own rows.
    ("scheduler.js", "appendRows", "queue_add", 1, ""),
    ("scheduler.js", "withSteps", "queue_update", 1, ""),
    ("scheduler.js", "rowEl", "queue_update", 1, ""),
    ("scheduler-model.js", "listStartCalls", "queue_add", 2, ""),
    (
        "scheduler-model.js",
        "listStartCalls",
        "queue_add_reviewed_list",
        1,
        "",
    ),
    // Keystrokes, drops, completions and voice: the local user.
    ("layout.js", "insertDroppedFiles", "pty_write", 1, ""),
    ("layout.js", "wireTerminalInput", "pty_write", 1, ""),
    ("terminal.js", "acceptSuggestion", "pty_write", 1, ""),
    ("terminal.js", "acceptGhost", "pty_write", 1, ""),
    ("scheduler.js", "initScheduler", "pty_write", 1, ""),
    ("voice-model.js", "type", "voice_deliver", 1, ""),
    ("voice-model.js", "typeText", "voice_deliver", 1, ""),
];

/// Calls a pinned function must keep before it queues external text.
const FRONTEND_ADMISSION: &[(&str, &str, &str)] = &[
    ("inbound.js", "handleInbound", "channelBlockReason("),
    ("board.js", "queueChannelPlan", "channelAgentCommand("),
];

fn js_sources() -> Vec<(String, String)> {
    let root = manifest("../ui/js");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&root).expect("ui/js") {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|x| x == "js") {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            out.push((name, std::fs::read_to_string(&path).unwrap()));
        }
    }
    assert!(out.len() >= 20, "frontend modules found: {}", out.len());
    out.sort();
    out
}

fn js_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// The function a JS line declares: `function name(`, an object method
/// `async name(a, b) {` (a plain parameter list, so `listen('x', () => {` is
/// a call, not a method), or a top-level `const name = (...) =>` /
/// `= async` / `= function`. Anything nested deeper than one closure level
/// belongs to the function around it.
fn js_declaration(line: &str) -> Option<String> {
    let indent = line.len() - line.trim_start().len();
    if indent > 4 {
        return None;
    }
    let t = line.trim_start();
    let t = t.strip_prefix("export ").unwrap_or(t);
    let t = t.strip_prefix("default ").unwrap_or(t);
    let t = t.strip_prefix("async ").unwrap_or(t);
    let head = |s: &str| -> String { s.chars().take_while(|c| js_ident(*c)).collect() };
    if let Some(rest) = t.strip_prefix("function") {
        let name = head(rest.trim_start_matches(['*', ' ']));
        return (!name.is_empty()).then_some(name);
    }
    if let Some(rest) = t
        .strip_prefix("const ")
        .or_else(|| t.strip_prefix("let "))
        .filter(|_| indent == 0)
    {
        let name = head(rest);
        let rhs = rest[name.len()..]
            .trim_start()
            .strip_prefix('=')?
            .trim_start();
        let callee = head(rhs);
        let arrow = rhs.starts_with("async")
            || rhs.starts_with("function")
            || (rhs.starts_with('(') && line.contains("=>"))
            || (!callee.is_empty() && rhs[callee.len()..].trim_start().starts_with("=>"));
        return (arrow && !name.is_empty()).then_some(name);
    }
    let name = head(t);
    let keyword = [
        "if", "for", "while", "switch", "catch", "return", "else", "do", "try",
    ];
    let params = t[name.len()..]
        .strip_prefix('(')
        .and_then(|rest| rest.trim_end().strip_suffix('{'))
        .and_then(|rest| rest.trim_end().strip_suffix(')'))?;
    (!name.is_empty()
        && !keyword.contains(&name.as_str())
        && !params.contains(['\'', '"', '`', '(', ')', '>']))
    .then_some(name)
}

fn js_enclosing(source: &str, at: usize) -> String {
    source[..at]
        .lines()
        .rev()
        .find_map(js_declaration)
        .unwrap_or_default()
}

/// Text of the function `name` declares: its line up to the next declaration.
fn js_function<'a>(source: &'a str, name: &str) -> &'a str {
    let mut start = None;
    let mut offset = 0;
    for line in source.split_inclusive('\n') {
        let declared = js_declaration(line);
        match (&start, declared) {
            (None, Some(n)) if n == name => start = Some(offset),
            (Some(s), Some(_)) => return &source[*s..offset],
            _ => {}
        }
        offset += line.len();
    }
    start.map_or("", |s| &source[s..])
}

#[test]
fn frontend_queue_and_terminal_calls_are_pinned() {
    let mut found: Census = Vec::new();
    let mut violations = Vec::new();
    for (file, source) in js_sources() {
        for command in FRONTEND_COMMANDS {
            for quote in ['\'', '"'] {
                let literal = format!("{quote}{command}{quote}");
                for at in token_sites(&source, &literal) {
                    let function = js_enclosing(&source, at);
                    let line_start = source[..at].rfind('\n').map_or(0, |n| n + 1);
                    let statement_end = at + source[at..].find(';').unwrap_or(source.len() - at);
                    let statement = &source[line_start..statement_end];
                    if let Some((.., guard)) = FRONTEND_SITES
                        .iter()
                        .find(|(f, n, c, ..)| *f == file && *n == function && c == command)
                    {
                        if !guard.is_empty() && !statement.contains(guard) {
                            violations.push(format!(
                                "{file} fn {function}: `{command}` lost its guard `{guard}`"
                            ));
                        }
                    }
                    match found
                        .iter_mut()
                        .find(|(f, n, c, _)| *f == file && *n == function && c == command)
                    {
                        Some(entry) => entry.3 += 1,
                        None => found.push((file.clone(), function, command.to_string(), 1)),
                    }
                }
            }
        }
    }
    assert!(violations.is_empty(), "{violations:#?}");
    let allowed: Vec<(&str, &str, &str, usize, &str)> = FRONTEND_SITES.to_vec();
    assert_pinned("frontend", &found, &allowed);
    let sources = js_sources();
    for (file, function, required) in FRONTEND_ADMISSION {
        let source = &sources.iter().find(|(n, _)| n == file).unwrap().1;
        assert!(
            js_function(source, function).contains(required),
            "{file} fn {function} must keep {required}"
        );
    }
}

// ----------------------------------------------------- scanner self-tests

#[test]
fn scanner_binds_sites_and_bodies() {
    let source = "fn a() { let s = \"}\"; let c = '{'; x.items.push(1); }\nfn b<'t>() { y }\n";
    assert_eq!(
        function_body(source, "a"),
        Some("{ let s = \"}\"; let c = '{'; x.items.push(1); }")
    );
    assert_eq!(function_body(source, "b"), Some("{ y }"));
    assert!(function_body(source, "c").is_none());
    let at = token_sites(source, "items.push(")[0];
    assert_eq!(enclosing_function(source, at), "a");
    assert!(token_sites("// items.push(x)\n", "items.push(").is_empty());
    assert!(copy_mode_keys("send-keys -t {t} -X cancel", 0));
    assert!(!copy_mode_keys(
        "'send-keys -X -t {id} cancel' ''; send-keys -t {id} C-d",
        31
    ));
    for (line, name) in [
        (
            "export async function handleInbound(item) {",
            Some("handleInbound"),
        ),
        (
            "  async queueChannelPlan(sid, key) {",
            Some("queueChannelPlan"),
        ),
        (
            "export const queueBufferEntry = (sid, id) =>",
            Some("queueBufferEntry"),
        ),
        ("const complete = async handle => {", Some("complete")),
        (
            "    const card = draft.cards.find(value => value.id === sid);",
            None,
        ),
        ("  if (clock) {", None),
        ("      for (const step of run.initialSteps || []) {", None),
    ] {
        assert_eq!(js_declaration(line).as_deref(), name, "{line}");
    }
}
