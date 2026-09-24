//! IPC command census: the JS ↔ Rust command names, held equal by a test
//! instead of by the committer remembering both sides.
//!
//! 1. every `#[tauri::command]` in `src/` is registered in `main.rs`'s
//!    `generate_handler!` exactly once, and nothing else is;
//! 2. every literal `inv('name'` / `invoke('name'` in `ui/js/*.js` and
//!    `ui/index.html` names a registered command;
//! 3. every registered command is called by a literal, produced at one of
//!    the pinned `DYNAMIC_SITES` (a name chosen by a ternary or a plan, each
//!    name still a quoted literal inside the named JS function), or is a
//!    `DEBUG_ONLY` command the WKWebView smoke / debug harness calls from
//!    `ui/test`. A new command that is none of these fails: classify it.
//!
//! Extraction is single-line literal matching only (FR-2 shape rule): a
//! call whose name cannot be read from one line is a dynamic site and is
//! listed here, never parsed.

mod source_scan;
use source_scan::{all_sources, js_enclosing, js_sources, manifest};
use std::collections::{BTreeMap, BTreeSet};

/// (JS file, enclosing function, command names it chooses between).
const DYNAMIC_SITES: &[(&str, &str, &[&str])] = &[
    (
        "board.js",
        "queueInboundPlan",
        &["channel_queue_add", "queue_add"],
    ),
    (
        "board.js",
        "queueInboundPlan",
        &["channel_queue_add_reviewed_list", "queue_add_reviewed_list"],
    ),
    (
        "scheduler-model.js",
        "listStartCalls",
        &["queue_add", "queue_add_reviewed_list"],
    ),
    (
        "layout.js",
        "initLayout",
        &["mcp_return_control", "mcp_takeover"],
    ),
    (
        "settings.js",
        "renderMcpSettings",
        &["tunnel_helper_stop", "tunnel_helper_remove"],
    ),
    (
        "settings.js",
        "renderTunnelForClient",
        &["tunnel_helper_start", "tunnel_helper_stop"],
    ),
    (
        "settings.js",
        "initSettings",
        &["mcp_disable", "mcp_enable"],
    ),
];

/// Registered for the debug-only WKWebView smoke and fault harness; called
/// only from `ui/test`, never from production frontend code.
const DEBUG_ONLY: &[&str] = &[
    "channel_smoke_seed",
    "connector_smoke_seed",
    "connector_smoke_transport",
    "connector_smoke_window",
    "smoke_clipboard_metrics",
    "smoke_fault_set",
    "smoke_flush_queue",
    "smoke_query_channel",
    "smoke_queue_state",
    "smoke_seed_ambiguous",
    "terminal_metrics",
];

fn command_name(text: &str) -> Option<&str> {
    let end = text.find(['\'', '"'])?;
    let name = &text[..end];
    (!name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'))
    .then_some(name)
}

/// Literal command names called on one line: `inv('x'` or `invoke('x'`
/// (either quote), not preceded by an identifier character.
fn literal_calls(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    for callee in ["inv(", "invoke("] {
        for (at, _) in line.match_indices(callee) {
            let before = line[..at].chars().next_back();
            if before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '$' || c == '.') {
                continue;
            }
            let rest = line[at + callee.len()..].trim_start();
            if let Some(name) = rest.strip_prefix(['\'', '"']).and_then(command_name) {
                out.push(name.to_owned());
            }
        }
    }
    out
}

fn frontend() -> Vec<(String, String)> {
    let mut files = js_sources();
    files.push((
        "index.html".into(),
        std::fs::read_to_string(manifest("../ui/index.html")).unwrap(),
    ));
    files
}

fn frontend_literals() -> BTreeMap<String, BTreeSet<String>> {
    let mut found: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (file, source) in frontend() {
        for line in source.lines() {
            for name in literal_calls(line) {
                found.entry(name).or_default().insert(file.clone());
            }
        }
    }
    found
}

/// Entries of `generate_handler![ … ]`, one `path::name,` per line.
fn registered() -> Vec<String> {
    let main = std::fs::read_to_string(manifest("src/main.rs")).unwrap();
    let start = main.find("generate_handler![").expect("generate_handler!");
    let body = &main[start..];
    let body = &body[..body.find("])").expect("end of generate_handler!")];
    body.lines()
        .skip(1)
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            let entry = line.strip_suffix(',').unwrap_or_else(|| {
                panic!("generate_handler! entry is not one `path::name,` line: {line}")
            });
            entry.rsplit("::").next().unwrap().to_owned()
        })
        .collect()
}

/// Function names under `#[tauri::command]` (attributes and comments in
/// between are skipped; the `fn` line itself is read, never parsed across).
fn declared_commands() -> Vec<String> {
    let mut out = Vec::new();
    for (file, source) in all_sources() {
        let mut lines = source.lines();
        while let Some(line) = lines.next() {
            let line = line.trim();
            if line != "#[tauri::command]" && !line.starts_with("#[tauri::command(") {
                continue;
            }
            let signature = lines
                .by_ref()
                .map(str::trim)
                .find(|l| !l.starts_with("#[") && !l.starts_with("//"))
                .unwrap_or_default();
            let name = signature
                .split_once("fn ")
                .map(|(_, rest)| rest)
                .and_then(|rest| rest.split(['(', '<']).next())
                .unwrap_or_else(|| panic!("{file}: #[tauri::command] not followed by fn"));
            out.push(name.to_owned());
        }
    }
    out.sort();
    out
}

#[test]
fn every_command_is_registered_exactly_once() {
    let mut registered = registered();
    registered.sort();
    let unique: BTreeSet<_> = registered.iter().collect();
    assert_eq!(unique.len(), registered.len(), "a command registered twice");
    let declared = declared_commands();
    let missing: Vec<_> = declared.iter().filter(|n| !unique.contains(n)).collect();
    let stray: Vec<_> = registered
        .iter()
        .filter(|n| !declared.contains(n))
        .collect();
    assert!(
        missing.is_empty() && stray.is_empty(),
        "#[tauri::command] not registered: {missing:?}; registered without #[tauri::command]: {stray:?}"
    );
    assert_eq!(
        declared.len(),
        registered.len(),
        "a #[tauri::command] name is declared twice"
    );
}

#[test]
fn frontend_literals_name_registered_commands() {
    let registered: BTreeSet<String> = registered().into_iter().collect();
    let unknown: Vec<_> = frontend_literals()
        .into_iter()
        .filter(|(name, _)| !registered.contains(name))
        .collect();
    assert!(
        unknown.is_empty(),
        "frontend calls unregistered commands: {unknown:?}"
    );
}

#[test]
fn every_registered_command_has_a_classified_caller() {
    let literals = frontend_literals();
    let sources: BTreeMap<String, String> = js_sources().into_iter().collect();
    let registered: BTreeSet<String> = registered().into_iter().collect();
    let mut problems = Vec::new();

    let mut dynamic = BTreeSet::new();
    for (file, function, names) in DYNAMIC_SITES {
        let source = &sources[*file];
        for name in *names {
            let quoted = [format!("'{name}'"), format!("\"{name}\"")];
            let at_site = quoted.iter().any(|literal| {
                source
                    .match_indices(literal.as_str())
                    .any(|(at, _)| js_enclosing(source, at) == *function)
            });
            if !at_site {
                let found: BTreeSet<String> = quoted
                    .iter()
                    .flat_map(|literal| source.match_indices(literal.as_str()))
                    .map(|(at, _)| js_enclosing(source, at))
                    .collect();
                problems.push(format!(
                    "{file} fn {function} no longer names `{name}` (found in {found:?})"
                ));
            }
            if !registered.contains(*name) {
                problems.push(format!("dynamic site names unregistered `{name}`"));
            }
            dynamic.insert(name.to_string());
        }
    }

    let tests: String = std::fs::read_dir(manifest("../ui/test"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|x| x == "mjs"))
        .map(|path| std::fs::read_to_string(path).unwrap())
        .collect();
    for name in DEBUG_ONLY {
        if !registered.contains(*name) {
            problems.push(format!("debug-only `{name}` is not registered"));
        }
        if literals.contains_key(*name) || dynamic.contains(*name) {
            problems.push(format!("debug-only `{name}` has a production caller"));
        }
        if !tests.contains(&format!("'{name}'")) && !tests.contains(&format!("\"{name}\"")) {
            problems.push(format!("debug-only `{name}` has no ui/test caller"));
        }
    }

    for name in &registered {
        if !literals.contains_key(name)
            && !dynamic.contains(name)
            && !DEBUG_ONLY.contains(&name.as_str())
        {
            problems.push(format!(
                "`{name}` has no frontend caller: call it, list its dynamic site, mark it debug-only, or remove it"
            ));
        }
    }
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn literal_extraction_reads_one_line_only() {
    assert_eq!(literal_calls("await inv('queue_add', {"), ["queue_add"]);
    assert_eq!(
        literal_calls(r#"invoke("voice_stop", { id })"#),
        ["voice_stop"]
    );
    assert!(literal_calls("await inv(command, payload)").is_empty());
    assert!(literal_calls("core.invoke(cmd, args)").is_empty());
    assert!(literal_calls("x.inv('queue_add')").is_empty());
    assert!(literal_calls("await inv(a ? 'mcp_enable' : 'mcp_disable')").is_empty());
}
