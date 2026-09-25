//! Signal consumer census (Signal Integrity FR-SI-01). The agent hook words
//! are INTERACTION observations (`agent_status.rs` header): `working` = an
//! interaction is active, `needs-input` = the agent requested input,
//! `turn-done` = an interaction boundary was observed. None of them is
//! lifecycle or readiness authority. Regression: a clock automation with
//! "close the card" retired a live Claude Code session ~7 s after its first
//! `turn-done` and killed a background command the agent was still running
//! (and would have resumed from); an external follow-up row was released by
//! the same word.
//!
//! Every production site that reads a word or the agent observation is
//! pinned here to (file, function, token, count) and classified:
//!
//! - `Producer` — defines, parses, forwards or mirrors the vocabulary;
//! - `Presentation` — status colour, labels, text;
//! - `Attention` — unread/viewed, the attention list, badges, notifications;
//! - `Hold` — a side-effect path that may only WITHHOLD an automatic action
//!   (the scheduler's agent hold, the automation finish reading);
//! - `NotSignal` — the same spelling with another meaning (a Board column
//!   semantic, an agent CLI name, a recognized process).
//!
//! Rules: an unreviewed site or a stale entry fails; no `Hold` entry may read
//! `turn-done` (an interaction boundary never releases or retires anything);
//! a `Presentation`/`Attention` function never reaches a side-effect sink.
//! A new side-effect consumer needs a `Hold` entry here AND a contract test
//! beside the code (`pure.test.mjs` finish rule, `scheduler/tests.rs` agent
//! hold). The string scan is a boundary census, not the correctness proof.

mod source_scan;
use source_scan::{
    code_only, enclosing_function, is_declared_test_file, is_ident, js_declaration, js_ident,
    js_sources, production_sources,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    Producer,
    Presentation,
    Attention,
    Hold,
    NotSignal,
}
use Class::*;

/// (file, function, token) → count.
type Census = Vec<(String, String, String, usize)>;

fn bump(out: &mut Census, file: &str, function: String, token: &str) {
    match out
        .iter_mut()
        .find(|(f, n, t, _)| f == file && *n == function && t == token)
    {
        Some(entry) => entry.3 += 1,
        None => out.push((file.to_string(), function, token.to_string(), 1)),
    }
}

/// Offsets of `token` not followed by an identifier character (`.agent`
/// must not match `.agent_target`).
fn sites(source: &str, token: &str, ident: fn(char) -> bool) -> Vec<usize> {
    source
        .match_indices(token)
        .map(|(at, _)| at)
        .filter(|&at| {
            !source[at + token.len()..]
                .chars()
                .next()
                .is_some_and(|c| token.chars().last().is_some_and(ident) && ident(c))
        })
        .collect()
}

// ------------------------------------------------------------------ Rust

const RUST_TOKENS: &[&str] = &[
    "TURN_DONE",
    "NEEDS_INPUT",
    "\"turn-done\"",
    "\"needs-input\"",
    "\"working\"",
    "agent_status::projected",
    "agent_status::projections",
    "agent_holds(",
    "notify::observe(",
    ".agent",
];

/// Byte span of the brace-balanced body opened after `from` (string and
/// char literals skipped).
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

/// The innermost `fn` whose body holds `at`; "" at item scope (a const or
/// a spec table), where `enclosing_function` would name an earlier fn.
fn rust_function_at(source: &str, at: usize) -> String {
    source
        .match_indices("fn ")
        .filter(|(from, _)| !source[..*from].chars().next_back().is_some_and(is_ident))
        .filter(|(from, _)| *from < at)
        .filter_map(|(from, _)| {
            let (open, close) = body_span(source, from)?;
            (open < at && at <= close).then(|| enclosing_function(source, open))
        })
        .last()
        .unwrap_or_default()
}

fn rust_census() -> Census {
    let mut out = Census::new();
    for (file, source) in production_sources() {
        for token in RUST_TOKENS {
            for at in sites(&source, token, is_ident) {
                let line_start = source[..at].rfind('\n').map_or(0, |n| n + 1);
                if source[line_start..at].trim_start().starts_with("//") {
                    continue;
                }
                bump(&mut out, &file, rust_function_at(&source, at), token);
            }
        }
    }
    out
}

const RUST: &[(&str, &str, &str, usize, Class)] = &[
    // the closed vocabulary, the parser and the hook spec tables
    ("agent_status.rs", "", "TURN_DONE", 2, Producer),
    ("agent_status.rs", "", "NEEDS_INPUT", 2, Producer),
    ("agent_status.rs", "", "\"turn-done\"", 4, Producer),
    ("agent_status.rs", "", "\"needs-input\"", 3, Producer),
    ("agent_status.rs", "", "\"working\"", 3, Producer),
    // FR-SI-04: the interaction tracker admits words per source interaction
    // (refuses late/ended/mismatched ones); it grants nothing
    ("agent_status.rs", "admit", "NEEDS_INPUT", 4, Producer),
    // ingest forwards a word to the desktop attention loop only from the
    // session's Signal target pane; reconcile re-projects on target changes
    ("agent_status.rs", "ingest", "notify::observe(", 1, Producer),
    (
        "agent_status.rs",
        "reconcile",
        "notify::observe(",
        1,
        Producer,
    ),
    // the poll hands the word to the webview (status, attention, finish)
    (
        "commands.rs",
        "poll_from_listing",
        "agent_status::projections",
        1,
        Producer,
    ),
    // notification body, Dock badge, unread mark
    ("notify.rs", "", "\"needs-input\"", 1, Attention),
    ("notify.rs", "", "\"turn-done\"", 1, Attention),
    ("notify.rs", "", "NEEDS_INPUT", 1, Attention),
    ("notify.rs", "", "TURN_DONE", 1, Attention),
    ("notify.rs", "body_text", "NEEDS_INPUT", 1, Attention),
    ("notify.rs", "badge_count", "NEEDS_INPUT", 1, Attention),
    ("notify.rs", "observe_with", "NEEDS_INPUT", 1, Attention),
    // FR-SI-05: unread = a turn-done episode not yet viewed; the viewed
    // truth is recorded for exactly one live turn-done episode
    ("notify.rs", "unread", "TURN_DONE", 1, Attention),
    ("notify.rs", "dismiss_with", "TURN_DONE", 1, Attention),
    ("agent_status.rs", "mark_viewed", "TURN_DONE", 1, Attention),
    ("notify.rs", "observe_with", "TURN_DONE", 2, Attention),
    // the scheduler's agent hold: may only withhold an automatic paste
    (
        "scheduler/select.rs",
        "observe",
        "agent_status::projections",
        1,
        Hold,
    ),
    ("scheduler/select.rs", "", "agent_holds(", 1, Hold),
    ("scheduler/select.rs", "agent_holds", "NEEDS_INPUT", 1, Hold),
    ("scheduler/select.rs", "agent_holds", ".agent", 1, Hold),
    ("scheduler/select.rs", "eligible", "agent_holds(", 1, Hold),
    // the panel plan names the hold (stage `agent`)
    (
        "scheduler/review.rs",
        "plan_item",
        "agent_holds(",
        1,
        Presentation,
    ),
    // Connector: a recognized foreground agent PROCESS (`connector_probe`)
    (
        "connector/native.rs",
        "execute_native",
        ".agent",
        1,
        NotSignal,
    ),
    (
        "connector/projection.rs",
        "snapshot_in",
        ".agent",
        1,
        NotSignal,
    ),
    (
        "connector/projection.rs",
        "output_with",
        ".agent",
        2,
        NotSignal,
    ),
];

// -------------------------------------------------------------------- JS

const JS_TOKENS: &[&str] = &[
    "finish_fg",
    "'turn-done'",
    "'needs-input'",
    "'working'",
    ".agent",
    "effectiveCardStatus(",
    "runFinishHolds(",
];

/// A `//` line or a block comment whose first line starts with `/*` (the
/// frontend's comment style); code sharing a line with a comment counts.
fn js_comment_lines(source: &str) -> Vec<bool> {
    let mut inside = false;
    source
        .lines()
        .map(|line| {
            let t = line.trim_start();
            if inside {
                inside = !line.contains("*/");
                return true;
            }
            if t.starts_with("/*") {
                inside = !t.contains("*/");
                return true;
            }
            t.starts_with("//") || t.starts_with('*')
        })
        .collect()
}

/// The declaration a JS site belongs to: the nearest function (per
/// `js_declaration`) or top-level `const`/`let` at or above its line.
fn js_scope(source: &str, at: usize) -> String {
    let line_end = source[at..].find('\n').map_or(source.len(), |n| at + n);
    source[..line_end]
        .lines()
        .rev()
        .find_map(|line| {
            js_declaration(line).or_else(|| {
                let t = line.strip_prefix("export ").unwrap_or(line);
                let rest = t
                    .strip_prefix("const ")
                    .or_else(|| t.strip_prefix("let "))?;
                let name: String = rest.chars().take_while(|c| js_ident(*c)).collect();
                (!name.is_empty()).then_some(name)
            })
        })
        .unwrap_or_default()
}

/// The source text from a scope's declaration line to the next one.
fn js_scope_body<'a>(source: &'a str, scope: &str) -> Option<&'a str> {
    let mut start = None;
    let mut offset = 0;
    for line in source.split_inclusive('\n') {
        let declared = js_declaration(line);
        if let Some(from) = start {
            if declared.is_some() {
                return Some(&source[from..offset]);
            }
        } else if declared.as_deref() == Some(scope) {
            start = Some(offset);
        }
        offset += line.len();
    }
    start.map(|from| &source[from..])
}

fn js_census() -> Census {
    let mut out = Census::new();
    for (file, source) in js_sources() {
        if file.starts_with("i18n") {
            continue;
        }
        let comments = js_comment_lines(&source);
        for token in JS_TOKENS {
            for at in sites(&source, token, js_ident) {
                let line = source[..at].matches('\n').count();
                if comments[line] {
                    continue;
                }
                bump(&mut out, &file, js_scope(&source, at), token);
            }
        }
    }
    out
}

const JS: &[(&str, &str, &str, usize, Class)] = &[
    // the closed vocabulary mirror and the status colour
    ("pure.js", "AGENT_STATES", "'turn-done'", 1, Producer),
    ("pure.js", "AGENT_STATES", "'needs-input'", 1, Producer),
    ("pure.js", "AGENT_STATES", "'working'", 1, Producer),
    (
        "pure.js",
        "effectiveCardStatus",
        "effectiveCardStatus(",
        1,
        Presentation,
    ),
    (
        "pure.js",
        "effectiveCardStatus",
        "'turn-done'",
        1,
        Presentation,
    ),
    (
        "pure.js",
        "effectiveCardStatus",
        "'needs-input'",
        1,
        Presentation,
    ),
    (
        "pure.js",
        "effectiveCardStatus",
        "'working'",
        1,
        Presentation,
    ),
    // a Board column semantic, not an agent word
    ("pure.js", "newSessionColumn", "'working'", 1, NotSignal),
    ("app.js", "boot", "'working'", 2, NotSignal),
    (
        "board-defaults.js",
        "DEFAULT_BOARD_SEMANTICS",
        "'working'",
        1,
        NotSignal,
    ),
    (
        "board-defaults.js",
        "migrateColumnSemantics",
        "'working'",
        1,
        NotSignal,
    ),
    // the automation finish reading: agent words may only hold the close
    ("pure.js", "runFinishHolds", "runFinishHolds(", 1, Hold),
    ("board.js", "observeRunFinish", "runFinishHolds(", 1, Hold),
    ("board.js", "observeRunFinish", ".agent", 1, Hold),
    // FR-SI-03/03.1: the finish rule's foreground is the Signal target
    // pane's, and only for a single-pane session (`finish_foregrounds`)
    ("board.js", "observeRunFinish", "finish_fg", 1, Hold),
    // the card's status colour on every poll (exempt from the sink check:
    // see POLL_EXEMPTION)
    (
        "board.js",
        "pollSessionsNow",
        "effectiveCardStatus(",
        1,
        Presentation,
    ),
    ("board.js", "pollSessionsNow", ".agent", 1, Presentation),
    // runtime snapshots, categories, counts, badge
    (
        "attention-model.js",
        "createAttentionTracker",
        ".agent",
        3,
        Attention,
    ),
    (
        "attention-model.js",
        "createAttentionTracker",
        "'needs-input'",
        1,
        Attention,
    ),
    (
        "attention-model.js",
        "createAttentionTracker",
        "'turn-done'",
        1,
        Attention,
    ),
    ("attention-model.js", "record", ".agent", 3, Attention),
    ("attention-model.js", "record", "'turn-done'", 1, Attention),
    (
        "attention-model.js",
        "record",
        "'needs-input'",
        1,
        Attention,
    ),
    ("attention-model.js", "record", "'working'", 1, Attention),
    (
        "attention-model.js",
        "record",
        "effectiveCardStatus(",
        1,
        Attention,
    ),
    (
        "attention.js",
        "attentionStatusText",
        ".agent",
        3,
        Presentation,
    ),
    (
        "attention.js",
        "attentionStatusText",
        "'needs-input'",
        1,
        Presentation,
    ),
    (
        "attention.js",
        "attentionStatusText",
        "'turn-done'",
        1,
        Presentation,
    ),
    (
        "attention.js",
        "attentionStatusText",
        "'working'",
        1,
        Presentation,
    ),
    ("attention.js", "sourceText", ".agent", 1, Presentation),
    ("notify-model.js", "seenDismissals", ".agent", 1, Attention),
    (
        "notify-model.js",
        "seenDismissals",
        "'turn-done'",
        1,
        Attention,
    ),
    // the queue panel's hook observation label
    (
        "queue-review.js",
        "executionPlan",
        "'needs-input'",
        1,
        Presentation,
    ),
    (
        "queue-review.js",
        "executionPlan",
        "'turn-done'",
        1,
        Presentation,
    ),
    (
        "queue-review.js",
        "executionPlan",
        ".agent",
        1,
        Presentation,
    ),
    // an i18n key (`queue.stage.agent`) and an agent CLI name
    ("queue-review.js", "stageKeys", ".agent", 1, NotSignal),
    ("resume-model.js", "resumeCommands", ".agent", 2, NotSignal),
];

/// `pollSessionsNow` projects every card's status AND retires a card whose
/// shell died (`!info.alive`, liveness, not a hook word) in the same loop.
/// Its agent read is pinned to exactly one `effectiveCardStatus` call; the
/// finish decision is handed to `observeRunFinish` (Hold).
const POLL_EXEMPTION: (&str, &str) = ("board.js", "pollSessionsNow");

/// Calls that change the Board, a pane or the queue. A Presentation or
/// Attention scope reaching one would be a side effect in disguise.
const JS_SINKS: &[&str] = &[
    "provider.close(",
    "closePaneBySid(",
    "runRetirement.observe(",
    "exitRetirement.observe(",
    "'queue_send_now'",
    "channel_queue_add",
    "'kill_session'",
];

// ----------------------------------------------------------------- checks

const TURN_DONE_TOKENS: &[&str] = &["TURN_DONE", "\"turn-done\"", "'turn-done'"];

fn assert_pinned(what: &str, found: &Census, allowed: &[(&str, &str, &str, usize, Class)]) {
    let mut violations = Vec::new();
    for (file, function, token, count) in found {
        match allowed
            .iter()
            .find(|(f, n, t, _, _)| f == file && n == function && t == token)
        {
            Some((_, _, _, expected, _)) if expected == count => {}
            Some((_, _, _, expected, class)) => violations.push(format!(
                "{what}: {file} `{function}` has {count} `{token}` site(s), reviewed {expected} ({class:?})"
            )),
            None => violations.push(format!(
                "{what}: unreviewed `{token}` in {file} `{function}` — classify it here; a side \
                 effect may only HOLD and needs a contract test beside the code"
            )),
        }
    }
    for (file, function, token, count, class) in allowed {
        if *count > 0
            && !found
                .iter()
                .any(|(f, n, t, _)| f == file && n == function && t == token)
        {
            violations.push(format!(
                "{what}: stale entry ({file}, {function}, {token}, {class:?})"
            ));
        }
    }
    assert!(violations.is_empty(), "{violations:#?}");
}

#[test]
fn every_rust_signal_site_is_classified() {
    assert_pinned("rust", &rust_census(), RUST);
}

#[test]
fn every_frontend_signal_site_is_classified() {
    assert_pinned("js", &js_census(), JS);
}

/// The P0 invariant: `turn-done` never enters a side-effect path.
#[test]
fn an_interaction_boundary_is_never_side_effect_authority() {
    for (file, function, token, _, class) in RUST.iter().chain(JS) {
        assert!(
            !(*class == Hold && TURN_DONE_TOKENS.contains(token)),
            "{file} `{function}` reads `turn-done` on a side-effect path"
        );
    }
    // the two decisions themselves, read directly: neither names a word
    // that could release or retire
    let pure = js_sources()
        .into_iter()
        .find(|(f, _)| f == "pure.js")
        .unwrap()
        .1;
    let finish = js_scope_body(&pure, "runFinishHolds").expect("runFinishHolds");
    for word in ["'turn-done'", "'working'", "'needs-input'"] {
        assert!(!finish.contains(word), "runFinishHolds reads {word}");
    }
    assert!(
        finish.contains("!agent &&"),
        "the finish reading requires the ABSENCE of agent state"
    );
    // FR-SI-03: `agent` and the foreground it is paired with come from the
    // same pane (poll_sessions `finish_fg`), never the representative `fg`
    let board = js_sources()
        .into_iter()
        .find(|(f, _)| f == "board.js")
        .unwrap()
        .1;
    let observe = js_scope_body(&board, "observeRunFinish").expect("observeRunFinish");
    assert!(
        observe.contains("agent: info.agent, fg: info.finish_fg"),
        "the finish rule pairs agent with the Signal target's foreground"
    );
    assert!(
        !observe.contains("info.fg"),
        "never the representative pane's fg"
    );
    // FR-SI-03.1: retirement evidence exists only for a single-pane
    // session — a pane-local shell never retires a session another pane's
    // agent lives in — and the poll takes it from nowhere else
    let status = production_sources()
        .into_iter()
        .find(|(f, _)| f == "agent_status.rs")
        .unwrap()
        .1;
    let at = status
        .find("fn finish_foregrounds(")
        .expect("finish_foregrounds");
    let (open, close) = body_span(&status, at).unwrap();
    assert!(
        status[open..=close].contains("== Some(&1)"),
        "finish_foregrounds requires exactly one pane per session"
    );
    let commands = production_sources()
        .into_iter()
        .find(|(f, _)| f == "commands.rs")
        .unwrap()
        .1;
    assert_eq!(
        commands.matches("finish_fg:").count(),
        2,
        "declared once, filled once"
    );
    assert!(commands.contains("finish_fg: pane.and_then(|_| finish.remove(&name))"));
    assert!(commands.contains("crate::agent_status::finish_foregrounds(&rows)"));
    let select = production_sources()
        .into_iter()
        .find(|(f, _)| f == "scheduler/select.rs")
        .unwrap()
        .1;
    let at = select.find("fn agent_holds(").expect("agent_holds");
    let (open, close) = body_span(&select, at).unwrap();
    let hold = &select[open..=close];
    for word in TURN_DONE_TOKENS.iter().chain(&["WORKING", "\"working\""]) {
        assert!(!hold.contains(word), "agent_holds reads {word}");
    }
}

#[test]
fn presentation_and_attention_never_reach_a_side_effect() {
    let sources = js_sources();
    let mut violations = Vec::new();
    for (file, function, _, _, class) in JS {
        if !matches!(class, Presentation | Attention)
            || function.is_empty()
            || (*file, *function) == POLL_EXEMPTION
        {
            continue;
        }
        let source = &sources.iter().find(|(f, _)| f == file).unwrap().1;
        let Some(body) = js_scope_body(source, function) else {
            continue; // a top-level const: no calls
        };
        for sink in JS_SINKS {
            if body.contains(sink) {
                violations.push(format!("{file} `{function}` ({class:?}) calls {sink}"));
            }
        }
    }
    assert!(violations.is_empty(), "{violations:#?}");
}

/// The census boundary must be harder to bypass than the code it governs
/// (FR-SI-05.1): only a REAL `#[cfg(test)]` declaration makes a module
/// test-only — never the same text in a comment, block comment, string or
/// raw string, never while an ungated declaration also exists, and a
/// top-level module only when pinned in `TEST_ONLY_TOP_LEVEL`.
#[test]
fn only_a_real_cfg_test_declaration_hides_a_module_from_the_censuses() {
    let classify = |name: &str, parent: &str, text: &str| {
        is_declared_test_file(name, &[(parent.to_string(), text.to_string())])
    };
    let real = "mod a;\n#[cfg(test)]\nmod signal_trace;\nfn main() {}\n";
    assert!(classify("signal_trace.rs", "main.rs", real));
    assert!(classify(
        "signal_trace.rs",
        "main.rs",
        "#[cfg(test)]   mod   signal_trace;"
    ));
    let disguised = [
        "// #[cfg(test)]\n// mod signal_trace;\nmod signal_trace;\n",
        "/* #[cfg(test)]\nmod signal_trace; */\nmod signal_trace;\n",
        "/* outer /* nested */ #[cfg(test)]\nmod signal_trace; */\nmod signal_trace;\n",
        "const X: &str = \"#[cfg(test)]\\nmod signal_trace;\";\nmod signal_trace;\n",
        "const X: &str = r#\"#[cfg(test)]\nmod signal_trace;\"#;\nmod signal_trace;\n",
        "const Y: &str = r##\"x\"# #[cfg(test)]\nmod signal_trace;\"##;\nmod signal_trace;\n",
        // gated once, but ALSO declared ungated: production
        "#[cfg(test)]\nmod signal_trace;\nmod signal_trace;\n",
        // only ever mentioned, never declared
        "// #[cfg(test)]\n// mod signal_trace;\n",
    ];
    for text in disguised {
        assert!(!classify("signal_trace.rs", "main.rs", text), "{text:?}");
    }
    // a top-level module that is not pinned is never test-only
    assert!(!classify("tool.rs", "main.rs", "#[cfg(test)]\nmod tool;"));
    // the dir/tests.rs rule is held to the same standard
    assert!(classify(
        "scheduler/tests.rs",
        "scheduler/mod.rs",
        "#[cfg(test)]\nmod tests;"
    ));
    assert!(!classify(
        "scheduler/tests.rs",
        "scheduler/mod.rs",
        "// #[cfg(test)]\n// mod tests;\nmod tests;"
    ));
    // char literals and lifetimes are code, not string starts
    assert_eq!(
        code_only("let q = '\"'; fn f<'a>(x: &'a str) {} // #[cfg(test)]"),
        "let q = ; fn f<'a>(x: &'a str) {}"
    );
    // the real tree: the trace harness is excluded, production modules are not
    let names: Vec<String> = production_sources().into_iter().map(|(n, _)| n).collect();
    assert!(!names.iter().any(|n| n == "signal_trace.rs"));
    for production in ["agent_status.rs", "notify.rs", "commands.rs", "main.rs"] {
        assert!(names.iter().any(|n| n == production), "{production}");
    }
}
