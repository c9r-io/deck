//! Agent hooks configuration and adapter declarations.
//!
//! Owns the Signal and Lifecycle hook families: lossless JSON editing,
//! installed-helper paths, registry-shaped opt-in reads, Settings commands and
//! the shared agent boot-migration phase.
//! Each family touches only its own helper markers, preserves other bytes and
//! follows settings symlinks through the one atomic writer. A malformed or
//! unreadable file is refused, never replaced by defaults. Toggle state is
//! derived from the file; no second settings store exists.
//!
//! Only a stable installed bundle may write global hooks. The explicit debug
//! re-entry experiment may edit its bound project Lifecycle hooks only.
//! Migration visits only unambiguous registered adapters and updates an already
//! installed family; it never enrolls another. The status reply likewise reads
//! only those adapters and projects its legacy fixed keys from the same generic
//! per-source observations.
//! Helpers stay in the bundle, and only the unreferenced legacy copy is retired.
//!
//! The adapter declarations describe installation and proof class; they grant
//! no Signal, pane ownership, input readiness or authority. Live admission,
//! observations and socket dispatch remain in the parent module. Pi owns its
//! extension entry in `agent_pi::config` and shares the byte-preserving JSON
//! primitives and atomic config writer here, not the hook ownership rules.
//! Parent re-exports preserve existing Rust and Tauri IPC entry paths.

use crate::applog::applog;
use crate::error::{DeckError, ErrorKind};
use std::path::{Path, PathBuf};

// ---------- hook installation (shared JSON machinery) -------------------------
//
// Claude Code (~/.claude/settings.json) and Codex ($CODEX_HOME/hooks.json)
// use the same hooks document shape: hooks → EventName →
// [{matcher?, hooks: [{type: "command", command, …}]}]. One marker-based
// merge engine serves both; each agent contributes only its spec table.

/// Markers every deck-authored hook command carries; install/uninstall touch
/// only the hooks carrying one. A group (`{matcher?, hooks: [...]}`) may also
/// hold hooks the user put there, and those are never deck's to remove.
/// Everything else in the file keeps its bytes (`hooks_file_edited`).
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

/// The lifecycle helper's marker: a second, separate ownership family.
const LIFECYCLE_HELPER_MARKER: &str = "deck.app/Contents/MacOS/deck-lifecycle-helper";
const LIFECYCLE_HELPER_NAME: &str = "deck-lifecycle-helper";
/// Read by `scripts/check-edr-binary`: an application that carries this
/// expects the lifecycle helper beside it, so a bundle without the helper is
/// rejected, while a bundle built before the helper existed (no such bytes)
/// stays valid. `#[used]` asks the compiler to keep this in the object file
/// although no code reads it; the language does not promise that a linker
/// keeps it. It was found in a local release build (lto, strip) and is a
/// second piece of evidence only: the scan already finds the helper's name
/// in `LIFECYCLE_HELPER_NAME`, which live code reads.
#[used]
static BUNDLE_REQUIRES_LIFECYCLE_HELPER: [u8; 42] = *b"deck-bundle-requires:deck-lifecycle-helper";

/// Which of deck's two hook families an edit owns. Each family installs,
/// updates, removes and migrates ONLY hooks carrying its own helper marker:
/// turning the status hooks off never removes the lifecycle hooks, turning
/// re-entry observation off never removes the status hooks, and neither
/// touches a hook the user wrote. Having one family never enrolls the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Family {
    /// `deck-status-helper`: the closed Signal words.
    Signal,
    /// `deck-lifecycle-helper`: native session start/end receipts.
    Lifecycle,
}

impl Family {
    fn owns(self, command: &str) -> bool {
        match self {
            Family::Signal => {
                command.contains(HELPER_MARKER) || command.contains(LEGACY_HELPER_MARKER)
            }
            Family::Lifecycle => command.contains(LIFECYCLE_HELPER_MARKER),
        }
    }

    fn helper_name(self) -> &'static str {
        match self {
            Family::Signal => HELPER_NAME,
            Family::Lifecycle => LIFECYCLE_HELPER_NAME,
        }
    }
}

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
/// Claude Code lifecycle receipts for re-entry (the Lifecycle family): the
/// third field is the helper's event word, not a Signal state.
const CLAUDE_LIFECYCLE_HOOKS: &[HookSpec] = &[
    ("SessionStart", None, "session-start"),
    ("SessionEnd", None, "session-end"),
];

const CODEX_HOOKS: &[HookSpec] = &[
    ("UserPromptSubmit", None, "working"),
    ("PermissionRequest", None, "needs-input"),
    ("Stop", None, "turn-done"),
    ("Interrupt", None, "turn-done"),
];

/// Codex lifecycle receipts for re-entry (the Lifecycle family), the same
/// two events and helper words as Claude's, written in Codex's shell form
/// (`ShellBrief`): one simple command, so the user's shell execs the helper
/// and it is the client's direct child. Codex asks the user to trust the
/// two entries in its `/hooks` view once.
const CODEX_LIFECYCLE_HOOKS: &[HookSpec] = &[
    ("SessionStart", None, "session-start"),
    ("SessionEnd", None, "session-end"),
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
    /// Exec form with a one-second hook timeout: a lifecycle receipt is
    /// synchronous with the client's exit and must never stretch it.
    ExecBrief,
    /// Shell form with a one-second timeout and no `async`: the lifecycle
    /// receipt of an agent whose hooks have no exec form (Codex). One
    /// simple command, nothing the shell would keep running itself.
    ShellBrief,
}

/// The style the Lifecycle family is written in for an agent whose Signal
/// hooks are written in `style`: the brief form of the same shape.
fn lifecycle_style(style: HookStyle) -> HookStyle {
    match style {
        HookStyle::Exec | HookStyle::ExecBrief => HookStyle::ExecBrief,
        HookStyle::ShellAsync | HookStyle::ShellBrief => HookStyle::ShellBrief,
    }
}

/// One deck hook object in the agent's document shape.
fn hook_value(style: HookStyle, helper: &str, source: &str, state: &str) -> serde_json::Value {
    match style {
        HookStyle::Exec => serde_json::json!({
            "type": "command", "command": helper, "args": [source, state], "timeout": 10
        }),
        HookStyle::ExecBrief => serde_json::json!({
            "type": "command", "command": helper, "args": [source, state], "timeout": 1
        }),
        HookStyle::ShellAsync => serde_json::json!({
            "type": "command", "command": format!("\"{helper}\" {source} {state}"),
            "timeout": 10, "async": true
        }),
        HookStyle::ShellBrief => serde_json::json!({
            "type": "command", "command": format!("\"{helper}\" {source} {state}"),
            "timeout": 1
        }),
    }
}

fn hook_is_ours(family: Family, hook: &serde_json::Value) -> bool {
    hook.get("command")
        .and_then(|c| c.as_str())
        .is_some_and(|command| family.owns(command))
}

/// The hooks of one group.
fn entry_hooks(entry: &serde_json::Value) -> impl Iterator<Item = &serde_json::Value> {
    entry
        .get("hooks")
        .and_then(|h| h.as_array())
        .into_iter()
        .flatten()
}

/// The group carries a deck hook (it may carry the user's as well).
fn entry_is_ours(family: Family, entry: &serde_json::Value) -> bool {
    entry_hooks(entry).any(|hook| hook_is_ours(family, hook))
}

/// Take deck's hooks out of one event's list, hook by hook: a hook the user
/// keeps in the same group stays where it is, and a group goes only when
/// taking deck's hooks out left it without any.
fn strip_ours(family: Family, list: &mut Vec<serde_json::Value>) {
    list.retain_mut(|entry| {
        let Some(hooks) = entry.get_mut("hooks").and_then(|h| h.as_array_mut()) else {
            return true;
        };
        let before = hooks.len();
        hooks.retain(|hook| !hook_is_ours(family, hook));
        hooks.len() == before || !hooks.is_empty()
    });
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

/// deck's hooks in this document are EXACTLY what the current specs
/// describe: one hook per spec event, in a group with the spec's matcher,
/// with the identical helper path, style and arguments, and no deck hook
/// left under an event the specs no longer name. What else its group holds
/// is the user's and is not looked at: a hook they added beside deck's does
/// not make deck's stale. `hooks_installed` only asks whether SOME deck hook
/// exists per event, so it cannot see a spec change — a narrowed matcher, a
/// moved bundle, a legacy `~/.deck/bin` path, a shell-form hook where exec
/// form is expected. This is the predicate boot migration repairs against.
#[cfg(test)]
pub(crate) fn hooks_are_current(
    root: &serde_json::Value,
    specs: &[HookSpec],
    source: &str,
    style: HookStyle,
    helper: &str,
) -> bool {
    hooks_are_current_of(Family::Signal, root, specs, source, style, helper)
}

/// `hooks_are_current` for one ownership family.
fn hooks_are_current_of(
    family: Family,
    root: &serde_json::Value,
    specs: &[HookSpec],
    source: &str,
    style: HookStyle,
    helper: &str,
) -> bool {
    let Some(hooks) = root.get("hooks").and_then(|h| h.as_object()) else {
        return false;
    };
    // deck's hooks under one event, each with the matcher of its group
    let ours = |list: &serde_json::Value| -> Vec<(Option<serde_json::Value>, serde_json::Value)> {
        list.as_array()
            .into_iter()
            .flatten()
            .flat_map(|entry| {
                entry_hooks(entry)
                    .filter(|hook| hook_is_ours(family, hook))
                    .map(|hook| (entry.get("matcher").cloned(), hook.clone()))
            })
            .collect()
    };
    // nothing of ours under a retired event
    if hooks
        .iter()
        .any(|(event, list)| !specs.iter().any(|(e, _, _)| e == event) && !ours(list).is_empty())
    {
        return false;
    }
    specs.iter().all(|(event, matcher, state)| {
        let mine = hooks.get(*event).map(&ours).unwrap_or_default();
        let [(group_matcher, hook)] = mine.as_slice() else {
            return false;
        };
        *group_matcher == matcher.map(serde_json::Value::from)
            && *hook == hook_value(style, helper, source, state)
    })
}

/// Add deck's hook groups to a parsed hooks document. Everything the user
/// wrote — other hooks, hooks in a group deck's hook shares, unknown keys,
/// other events — is preserved; every hook carrying the helper marker is
/// taken out first (document-wide, so an event a previous deck version
/// registered and this one no longer does cannot linger) and the current
/// specs are written, each in a group of its own, in the agent's `style`.
#[cfg(test)]
pub(crate) fn hooks_with_install(
    root: serde_json::Value,
    specs: &[HookSpec],
    source: &str,
    style: HookStyle,
    helper: &str,
) -> Result<serde_json::Value, DeckError> {
    hooks_with_install_of(Family::Signal, root, specs, source, style, helper)
}

/// `hooks_with_install` for one ownership family: only that family's hooks
/// are taken out before its specs are written.
fn hooks_with_install_of(
    family: Family,
    mut root: serde_json::Value,
    specs: &[HookSpec],
    source: &str,
    style: HookStyle,
    helper: &str,
) -> Result<serde_json::Value, DeckError> {
    let obj = root.as_object_mut().ok_or(DeckError::new(
        ErrorKind::Other,
        "settings file is not a JSON object",
    ))?;
    let hooks = obj
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or(DeckError::new(
            ErrorKind::Other,
            "the hooks key is not a JSON object",
        ))?;
    let mut emptied = Vec::new();
    for (event, list) in hooks.iter_mut() {
        let Some(list) = list.as_array_mut() else {
            continue;
        };
        let before = list.len();
        strip_ours(family, list);
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
            .ok_or(DeckError::new(
                ErrorKind::Other,
                "a hook event entry is not a JSON array",
            ))?;
        list.push(spec_entry(style, helper, source, spec));
    }
    Ok(root)
}

/// Remove every deck-authored hook, and with it the group, the event list
/// and the `hooks` key that this left empty. An empty list or an empty
/// `hooks` the user wrote is theirs and stays.
#[cfg(test)]
pub(crate) fn hooks_with_uninstall(root: serde_json::Value) -> serde_json::Value {
    hooks_with_uninstall_of(Family::Signal, root)
}

/// `hooks_with_uninstall` for one ownership family.
fn hooks_with_uninstall_of(family: Family, mut root: serde_json::Value) -> serde_json::Value {
    let mut emptied_all = false;
    if let Some(hooks) = root.get_mut("hooks").and_then(|h| h.as_object_mut()) {
        let mut emptied = Vec::new();
        for (event, list) in hooks.iter_mut() {
            let Some(list) = list.as_array_mut() else {
                continue;
            };
            let before = list.len();
            strip_ours(family, list);
            if list.is_empty() && before > 0 {
                emptied.push(event.clone());
            }
        }
        for event in &emptied {
            hooks.remove(event);
        }
        emptied_all = !emptied.is_empty() && hooks.is_empty();
    }
    if emptied_all {
        if let Some(obj) = root.as_object_mut() {
            obj.remove("hooks");
        }
    }
    root
}

/// Installed = every deck hook event carries a marker hook; a partial
/// install reads as OFF so re-enabling repairs it.
pub(crate) fn hooks_installed(root: &serde_json::Value, specs: &[HookSpec]) -> bool {
    hooks_installed_of(Family::Signal, root, specs)
}

/// `hooks_installed` for one ownership family.
fn hooks_installed_of(family: Family, root: &serde_json::Value, specs: &[HookSpec]) -> bool {
    let Some(hooks) = root.get("hooks").and_then(|h| h.as_object()) else {
        return false;
    };
    specs.iter().all(|(event, _, _)| {
        hooks
            .get(*event)
            .and_then(|l| l.as_array())
            .is_some_and(|list| list.iter().any(|entry| entry_is_ours(family, entry)))
    })
}

// ---------- writing only what is deck's ----------------------------------------
//
// The file belongs to the agent CLI and to whoever edits it: the order of its
// keys, its indentation and the way it spells a number or an escape are
// theirs. So the document is never serialized again. The functions above
// decide WHAT the result is, on the parsed value; the ones below produce it
// as the original text with deck's hooks cut out and deck's groups put in,
// every other byte where it was — and that text is used only when it parses
// back to exactly the decided value.

/// One edit of a hooks document.
#[derive(Clone, Copy)]
enum HookEdit<'a> {
    Install {
        specs: &'a [HookSpec],
        source: &'a str,
        style: HookStyle,
        helper: &'a str,
    },
    Uninstall,
}

/// The half-open byte range of a JSON value in the document text.
pub(crate) type Span = std::ops::Range<usize>;

/// One element of an array or one member of an object: `whole` runs from the
/// member's key (or the element's start) to the end of the value, `value` is
/// the value alone.
pub(crate) struct Item {
    pub(crate) key: Option<String>,
    pub(crate) whole: Span,
    pub(crate) value: Span,
}

pub(crate) fn skip_space(text: &[u8], mut at: usize) -> usize {
    while matches!(text.get(at), Some(b' ' | b'\t' | b'\n' | b'\r')) {
        at += 1;
    }
    at
}

/// Where the JSON value that starts at `at` ends. serde_json has accepted
/// the text already, so this only finds boundaries; whatever it does not
/// expect is `None`, and the caller leaves the file alone.
pub(crate) fn value_end(text: &[u8], at: usize) -> Option<usize> {
    match *text.get(at)? {
        b'"' => {
            let mut at = at + 1;
            loop {
                match *text.get(at)? {
                    b'"' => return Some(at + 1),
                    b'\\' => at += 2,
                    _ => at += 1,
                }
            }
        }
        b'{' | b'[' => {
            let mut depth = 0usize;
            let mut at = at;
            loop {
                match *text.get(at)? {
                    b'"' => {
                        at = value_end(text, at)?;
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(at + 1);
                        }
                    }
                    _ => {}
                }
                at += 1;
            }
        }
        _ => {
            let end = (at..text.len())
                .find(|&i| matches!(text[i], b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r'))
                .unwrap_or(text.len());
            (end > at).then_some(end)
        }
    }
}

/// The items of the object or array at `span`.
pub(crate) fn items(text: &str, span: &Span) -> Option<Vec<Item>> {
    let bytes = text.as_bytes();
    let (object, close) = match *bytes.get(span.start)? {
        b'{' => (true, b'}'),
        b'[' => (false, b']'),
        _ => return None,
    };
    let mut found = Vec::new();
    let mut at = skip_space(bytes, span.start + 1);
    if *bytes.get(at)? == close {
        return (at + 1 == span.end).then_some(found);
    }
    loop {
        let start = at;
        let key = if object {
            let end = value_end(bytes, at)?;
            let key = serde_json::from_str::<String>(text.get(at..end)?).ok()?;
            at = skip_space(bytes, end);
            if *bytes.get(at)? != b':' {
                return None;
            }
            at = skip_space(bytes, at + 1);
            Some(key)
        } else {
            None
        };
        let end = value_end(bytes, at)?;
        found.push(Item {
            key,
            whole: start..end,
            value: at..end,
        });
        at = skip_space(bytes, end);
        match *bytes.get(at)? {
            b',' => at = skip_space(bytes, at + 1),
            byte if byte == close => return (at + 1 == span.end).then_some(found),
            _ => return None,
        }
    }
}

/// One step of the document's indentation: what its first key is indented
/// by, two spaces when it shows none.
fn indent_unit(text: &str) -> String {
    let space = [' ', '\t', '\n', '\r'];
    let inside = text.trim_start_matches(space).get(1..).unwrap_or("");
    let lead = &inside[..inside.len() - inside.trim_start_matches(space).len()];
    match lead.rfind('\n') {
        Some(at) if at + 1 < lead.len() => lead[at + 1..].to_string(),
        _ => "  ".to_string(),
    }
}

/// What stands around the items of one container: after the opening bracket,
/// between two items (the comma included), before the closing bracket.
pub(crate) struct Layout {
    pub(crate) lead: String,
    between: String,
    tail: String,
}

/// The container's own layout when it has items; the document's, one step
/// deeper than the line the container opens on, when it has none.
pub(crate) fn layout(text: &str, span: &Span, old: &[Item]) -> Layout {
    let Some(first) = old.first() else {
        let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
        let line = text[..span.start].rfind('\n').map_or(0, |at| at + 1);
        let base: String = text[line..span.start]
            .chars()
            .take_while(|c| matches!(c, ' ' | '\t'))
            .collect();
        let unit = indent_unit(text);
        return Layout {
            lead: format!("{newline}{base}{unit}"),
            between: format!(",{newline}{base}{unit}"),
            tail: format!("{newline}{base}"),
        };
    };
    let lead = text[span.start + 1..first.whole.start].to_string();
    let between = match old.get(1) {
        Some(second) => text[first.whole.end..second.whole.start].to_string(),
        None => format!(",{lead}"),
    };
    let last = old.last().unwrap_or(first);
    Layout {
        tail: text[last.whole.end..span.end - 1].to_string(),
        lead,
        between,
    }
}

/// The text of the container at `span` with items dropped or replaced
/// (`kept[i]`: `None` drops item i) and `added` at its end. Every piece
/// between the items is the container's own: an item that stays keeps the
/// separator that stood before it, so the text changes only where an item
/// did.
pub(crate) fn rebuilt(
    text: &str,
    span: &Span,
    old: &[Item],
    kept: &[Option<String>],
    added: &[String],
) -> String {
    let Layout {
        lead,
        between,
        tail,
    } = layout(text, span, old);
    let mut out = String::from(&text[span.start..span.start + 1]);
    let mut any = false;
    for (index, item) in kept.iter().enumerate() {
        let Some(item) = item else { continue };
        if any {
            out.push_str(&text[old[index - 1].whole.end..old[index].whole.start]);
        } else {
            out.push_str(&lead);
        }
        out.push_str(item);
        any = true;
    }
    for item in added {
        out.push_str(if any { &between } else { &lead });
        out.push_str(item);
        any = true;
    }
    if any {
        out.push_str(&tail);
    }
    out.push_str(&text[span.end - 1..span.end]);
    out
}

/// A value deck writes, as text for a container whose items follow `lead`:
/// serde's pretty form re-indented in the document's own unit, or its
/// compact form when the container is written on one line.
pub(crate) fn written(value: &serde_json::Value, text: &str, lead: &str) -> Option<String> {
    let Some(at) = lead.rfind('\n') else {
        return serde_json::to_string(value).ok();
    };
    let newline = if lead[..at].ends_with('\r') {
        "\r\n"
    } else {
        "\n"
    };
    let (indent, unit) = (&lead[at + 1..], indent_unit(text));
    let pretty = serde_json::to_string_pretty(value).ok()?;
    let mut out = String::new();
    for (index, line) in pretty.lines().enumerate() {
        let body = line.trim_start_matches(' ');
        if index > 0 {
            out.push_str(newline);
            out.push_str(indent);
            out.push_str(&unit.repeat((line.len() - body.len()) / 2));
        }
        out.push_str(body);
    }
    Some(out)
}

/// A container after deck's hooks were taken out of it.
enum Stripped {
    Same,
    Gone,
    Changed(String),
}

/// One group without deck's hooks: the user's hooks stay, as written, in
/// what is left of it (`strip_ours`, on the text).
fn group_stripped(family: Family, text: &str, group: &Item) -> Option<Stripped> {
    let value: serde_json::Value = serde_json::from_str(text.get(group.value.clone())?).ok()?;
    let ours: Vec<bool> = entry_hooks(&value)
        .map(|hook| hook_is_ours(family, hook))
        .collect();
    if !ours.contains(&true) {
        return Some(Stripped::Same);
    }
    if !ours.contains(&false) {
        return Some(Stripped::Gone);
    }
    let members = items(text, &group.value)?;
    // serde_json keeps the LAST of two equal keys; so does this
    let list = members
        .iter()
        .rev()
        .find(|member| member.key.as_deref() == Some("hooks"))?;
    let hooks = items(text, &list.value)?;
    if hooks.len() != ours.len() {
        return None;
    }
    let kept: Vec<Option<String>> = hooks
        .iter()
        .zip(&ours)
        .map(|(hook, ours)| (!ours).then(|| text[hook.whole.clone()].to_string()))
        .collect();
    Some(Stripped::Changed(format!(
        "{}{}{}",
        &text[group.value.start..list.value.start],
        rebuilt(text, &list.value, &hooks, &kept, &[]),
        &text[list.value.end..group.value.end]
    )))
}

/// One event's list without deck's hooks and, on install, with deck's group
/// at its end. `Gone`: taking deck's hooks out emptied the list, so the
/// event goes with it.
fn list_edited(
    family: Family,
    text: &str,
    list: &Span,
    add: Option<&serde_json::Value>,
) -> Option<Stripped> {
    if text.as_bytes().get(list.start) != Some(&b'[') {
        // not a list: nothing of deck's is in it, and the value engine has
        // already refused to add to it
        return add.is_none().then_some(Stripped::Same);
    }
    let old = items(text, list)?;
    let mut kept = Vec::new();
    let mut changed = false;
    for group in &old {
        kept.push(match group_stripped(family, text, group)? {
            Stripped::Same => Some(text[group.whole.clone()].to_string()),
            Stripped::Changed(group) => {
                changed = true;
                Some(group)
            }
            Stripped::Gone => {
                changed = true;
                None
            }
        });
    }
    let mut added = Vec::new();
    if let Some(group) = add {
        added.push(written(group, text, &layout(text, list, &old).lead)?);
    }
    Some(if added.is_empty() && !changed {
        Stripped::Same
    } else if added.is_empty() && kept.iter().all(Option::is_none) {
        Stripped::Gone
    } else {
        Stripped::Changed(rebuilt(text, list, &old, &kept, &added))
    })
}

/// The document's text after `edit`. `None` when the text is not laid out
/// the way this reads it; the caller then leaves the file alone.
#[cfg(test)]
fn hooks_text_edited(text: &str, edit: HookEdit) -> Option<String> {
    hooks_text_edited_of(Family::Signal, text, edit)
}

/// `hooks_text_edited` for one ownership family.
fn hooks_text_edited_of(family: Family, text: &str, edit: HookEdit) -> Option<String> {
    let bytes = text.as_bytes();
    let start = skip_space(bytes, 0);
    if bytes.get(start) != Some(&b'{') {
        return None;
    }
    let root = start..value_end(bytes, start)?;
    let members = items(text, &root)?;
    let specs: &[HookSpec] = match edit {
        HookEdit::Install { specs, .. } => specs,
        HookEdit::Uninstall => &[],
    };
    let group = |spec: &HookSpec| match edit {
        HookEdit::Install {
            source,
            style,
            helper,
            ..
        } => Some(spec_entry(style, helper, source, spec)),
        HookEdit::Uninstall => None,
    };
    let own = |item: &Item| Some(text[item.whole.clone()].to_string());
    let replaced =
        |span: &Span, with: String| format!("{}{with}{}", &text[..span.start], &text[span.end..]);

    let Some(hooks) = members
        .iter()
        .rev()
        .find(|member| member.key.as_deref() == Some("hooks"))
    else {
        // no hooks key: uninstall has nothing to do, install adds the key
        // after the last one the document has
        if specs.is_empty() {
            return Some(text.to_string());
        }
        let mut events = serde_json::Map::new();
        for spec in specs {
            events.insert(spec.0.to_string(), serde_json::json!([group(spec)?]));
        }
        let lead = layout(text, &root, &members).lead;
        let member = format!(
            "\"hooks\": {}",
            written(&serde_json::Value::Object(events), text, &lead)?
        );
        let kept: Vec<Option<String>> = members.iter().map(own).collect();
        return Some(replaced(
            &root,
            rebuilt(text, &root, &members, &kept, &[member]),
        ));
    };
    let events = items(text, &hooks.value)?;
    let mut kept = Vec::new();
    let mut changed = false;
    for event in &events {
        let name = event.key.as_deref()?;
        let add = specs.iter().find(|spec| spec.0 == name).and_then(&group);
        kept.push(
            match list_edited(family, text, &event.value, add.as_ref())? {
                Stripped::Same => own(event),
                Stripped::Changed(list) => {
                    changed = true;
                    Some(format!(
                        "{}{list}",
                        &text[event.whole.start..event.value.start]
                    ))
                }
                Stripped::Gone => {
                    changed = true;
                    None
                }
            },
        );
    }
    let lead = layout(text, &hooks.value, &events).lead;
    let mut added = Vec::new();
    for spec in specs {
        if events
            .iter()
            .any(|event| event.key.as_deref() == Some(spec.0))
        {
            continue;
        }
        added.push(format!(
            "{}: {}",
            serde_json::to_string(spec.0).ok()?,
            written(&serde_json::json!([group(spec)?]), text, &lead)?
        ));
    }
    if added.is_empty() && !changed {
        return Some(text.to_string());
    }
    if added.is_empty() && kept.iter().all(Option::is_none) {
        // taking deck's hooks out emptied `hooks` itself: the key goes
        let rest: Vec<Option<String>> = members
            .iter()
            .map(|member| {
                (member.whole != hooks.whole).then(|| text[member.whole.clone()].to_string())
            })
            .collect();
        return Some(replaced(&root, rebuilt(text, &root, &members, &rest, &[])));
    }
    Some(replaced(
        &hooks.value,
        rebuilt(text, &hooks.value, &events, &kept, &added),
    ))
}

/// The text to write for `edit`, or `None` when the file already is what the
/// edit asks for: a toggle that changes nothing writes nothing. The value
/// engine decides the result; the text is the owner's original with deck's
/// part changed, and it must parse back to exactly that result or the file
/// is left alone. A file that does not exist yet has no text to keep.
#[cfg(test)]
fn hooks_file_edited(original: Option<&str>, edit: HookEdit) -> Result<Option<String>, DeckError> {
    hooks_file_edited_of(Family::Signal, original, edit)
}

/// `hooks_file_edited` for one ownership family.
fn hooks_file_edited_of(
    family: Family,
    original: Option<&str>,
    edit: HookEdit,
) -> Result<Option<String>, DeckError> {
    let value = settings_value(original)?;
    let next = match edit {
        HookEdit::Install {
            specs,
            source,
            style,
            helper,
        } => {
            if hooks_are_current_of(family, &value, specs, source, style, helper) {
                return Ok(None);
            }
            hooks_with_install_of(family, value.clone(), specs, source, style, helper)?
        }
        HookEdit::Uninstall => hooks_with_uninstall_of(family, value.clone()),
    };
    if next == value {
        return Ok(None);
    }
    let Some(original) = original else {
        let pretty = serde_json::to_string_pretty(&next).map_err(DeckError::from)?;
        return Ok(Some(format!("{pretty}\n")));
    };
    hooks_text_edited_of(family, original, edit)
        .filter(|edited| {
            serde_json::from_str::<serde_json::Value>(edited).is_ok_and(|value| value == next)
        })
        .map(Some)
        .ok_or(DeckError::new(
            ErrorKind::Other,
            "the agent settings file is laid out in a way deck cannot edit in place — not modifying it",
        ))
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

fn not_json() -> DeckError {
    DeckError::new(
        ErrorKind::Other,
        "the agent settings file is not valid JSON — not modifying it",
    )
}

/// The settings file as its owner wrote it; `None` when there is none.
pub(crate) fn read_settings_text(path: &Path) -> Result<Option<String>, DeckError> {
    match std::fs::read(path) {
        Ok(bytes) => String::from_utf8(bytes).map(Some).map_err(|_| not_json()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(DeckError::new(
            ErrorKind::io(e.kind()),
            format!("could not read agent settings ({})", e.kind()),
        )),
    }
}

/// The document a settings text holds; no file reads as an empty one.
fn settings_value(text: Option<&str>) -> Result<serde_json::Value, DeckError> {
    match text {
        Some(text) => serde_json::from_str(text).map_err(|_| not_json()),
        None => Ok(serde_json::json!({})),
    }
}

fn read_settings_value(path: &Path) -> Result<serde_json::Value, DeckError> {
    settings_value(read_settings_text(path)?.as_deref())
}

/// Atomic replace of a file that belongs to the agent CLI, not deck
/// (`datadir::atomic_write_through_link`): a symlinked settings file stays a
/// link and its target gets the bytes, the file keeps its own mode (0600 is
/// only the default for a file deck itself creates), a link to nothing and
/// a file deck may not write are refused, and a failure leaves the file as
/// it was with nothing beside it. Refuses to create the agent's config
/// DIRECTORY: a missing one means the agent never ran on this Mac.
pub(crate) fn write_agent_config(
    path: &Path,
    bytes: &[u8],
    never_ran: &str,
) -> Result<(), DeckError> {
    if let Some(dir) = path.parent() {
        if !dir.exists() {
            return Err(DeckError::new(ErrorKind::Missing, never_ran));
        }
    }
    crate::datadir::atomic_write_through_link(path, bytes)
}

/// The helper a hook command may name: the sidecar inside `bundle`, which
/// must exist and must be quotable as one double-quoted shell word.
#[cfg(test)]
fn helper_path_in(bundle: &Path) -> Result<String, DeckError> {
    family_helper_path_in(bundle, Family::Signal)
}

fn family_helper_path_in(bundle: &Path, family: Family) -> Result<String, DeckError> {
    let helper = bundle
        .join("Contents")
        .join("MacOS")
        .join(family.helper_name());
    if !helper.is_file() {
        return Err(DeckError::new(
            ErrorKind::Other,
            "the bundled status helper is missing from this build",
        ));
    }
    let text = helper.to_str().ok_or(DeckError::new(
        ErrorKind::Other,
        "the application path is not valid UTF-8",
    ))?;
    if text
        .bytes()
        .any(|b| b < 0x20 || b == 0x7f || matches!(b, b'"' | b'$' | b'`' | b'\\' | b'!'))
    {
        return Err(DeckError::new(
            ErrorKind::Other,
            "the application path contains characters a hook command cannot quote",
        ));
    }
    Ok(text.to_string())
}

/// The helper of THIS install, only when deck runs from a release location
/// (`/Applications/deck.app` or `~/Applications/deck.app`). Development and
/// smoke bundles live at temporary paths and are ad-hoc signed; a hook that
/// named one would break when the path vanished and would make the agent
/// CLI execute an unsigned binary. Such builds cannot install hooks and
/// never touch the agent's config at boot.
fn installed_helper_path() -> Result<String, DeckError> {
    installed_family_helper_path(Family::Signal)
}

fn installed_family_helper_path(family: Family) -> Result<String, DeckError> {
    if matches!(family, Family::Lifecycle) {
        if let Some(_config) = crate::launch_args::reentry_project_config()
            .map_err(|reason| DeckError::new(ErrorKind::Other, reason))?
        {
            let exe = std::env::current_exe().map_err(DeckError::from)?;
            let bundle = crate::tmux_lifecycle::app_bundle_root(&exe)
                .ok_or(DeckError::new(ErrorKind::Other, "missing isolated carrier"))?;
            return family_helper_path_in(bundle, family);
        }
    }
    let exe = std::env::current_exe().map_err(DeckError::from)?;
    let bundle = crate::tmux_lifecycle::app_bundle_root(&exe)
        .filter(|bundle| crate::tmux_lifecycle::stable_installed_bundle(bundle))
        .ok_or(DeckError::new(ErrorKind::Other, "agent status hooks can only be installed from /Applications/deck.app or ~/Applications/deck.app"))?;
    family_helper_path_in(bundle, family)
}

/// The lifecycle helper a receipt's reporter must be executing: the sidecar
/// beside this very executable (the installed bundle in a release, the
/// development bundle otherwise). Only its file identity is ever compared.
pub(crate) fn bundled_lifecycle_helper() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join(LIFECYCLE_HELPER_NAME)).filter(|helper| helper.is_file())
}

/// The explicit project-bound experiment's observation, when that mode is
/// configured. `None` means ordinary global settings; `Err` preserves the
/// existing fail-closed answer for an invalid experiment argument.
fn project_lifecycle_observation() -> Result<Option<bool>, ()> {
    match crate::launch_args::reentry_project_config() {
        Ok(Some(path)) => Ok(Some(read_settings_value(&path).ok().is_some_and(|value| {
            bundled_lifecycle_helper().is_some_and(|helper| {
                hooks_are_current_of(
                    Family::Lifecycle,
                    &value,
                    CLAUDE_LIFECYCLE_HOOKS,
                    CLAUDE_CODE.source,
                    HookStyle::ExecBrief,
                    &helper.to_string_lossy(),
                ) && hooks_installed_of(Family::Lifecycle, &value, CLAUDE_LIFECYCLE_HOOKS)
            })
        }))),
        Ok(None) => Ok(None),
        Err(_) => Err(()),
    }
}

/// Whether the user has opted in to re-entry observation for `source`: the
/// Lifecycle family's hooks are in that agent's hooks file. Derived from
/// that file, never stored twice. Installed is not "taking effect": only a
/// receipt of the real exit ever counts. Claude's answer honors the
/// project-bound experiment's alternate location.
pub(crate) fn lifecycle_observation_installed_for(source: &str) -> bool {
    if source == CLAUDE_CODE.source {
        match project_lifecycle_observation() {
            Ok(Some(installed)) => return installed,
            Err(()) => return false,
            Ok(None) => {}
        }
    }
    crate::extensions::agent(source)
        .and_then(AgentAdapter::hook_install)
        .is_some_and(|install| {
            !install.lifecycle_hooks.is_empty()
                && (install.config_path)()
                    .and_then(|path| read_settings_value(&path).ok())
                    .is_some_and(|value| {
                        hooks_installed_of(Family::Lifecycle, &value, install.lifecycle_hooks)
                    })
        })
}

/// Which core-owned generation proof an ACCEPTED event of a source feeds.
/// A closed pair: the adapter states its class and nothing else. It cannot
/// declare an event accepted, a generation trusted or a pane ready; `ingest`
/// applies the pane, ancestry, generation and interaction checks to every
/// source alike and only then records the proof of that class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SignalClass {
    /// An accepted interaction word proves a Claude interaction.
    Claude,
    /// Terminal continuity decides `CodexSignalTrust` for the generation.
    Codex,
    /// Pi: its words arrive only through the `pi-observer` family
    /// (`agent_pi`), never through the hook parser, and prove no interaction
    /// evidence for the scheduler. A recognized Pi only ever adds a hold.
    Pi,
}

/// The hook-based installation of one agent: its Signal hook spec table and
/// style, where its hooks document lives, and the Lifecycle family's hooks.
pub(crate) struct HookInstall {
    hooks: &'static [HookSpec],
    style: HookStyle,
    config_path: fn() -> Option<PathBuf>,
    never_ran: &'static str,
    /// The Lifecycle family's hooks (empty: none). Installed only by the
    /// separate re-entry observation opt-in, never with the Signal hooks.
    lifecycle_hooks: &'static [HookSpec],
}

/// How one agent's integration is installed. A closed pair, not an
/// interface: the hook merge engine below serves `Hooks`, and Pi's one
/// bundled extension entry is written by `agent_pi::config`. Neither can
/// stand in for the other.
pub(crate) enum Install {
    Hooks(HookInstall),
    PiExtension,
}

/// One supported agent: its wire source word (also the Settings toggle's
/// id), how its integration is installed, and its proof class. The static
/// extension tables (`extensions.rs`) list these; event parsing, the
/// installer, the boot migration and the status view all select through
/// `extensions::agent` / `extensions::agents`.
pub(crate) struct AgentAdapter {
    pub(crate) source: &'static str,
    pub(crate) class: SignalClass,
    pub(crate) install: Install,
    /// Optional static re-entry capability. `None`: this agent is never
    /// re-entered automatically. `Some` is not enabled, approved or trusted.
    pub(crate) reentry: Option<&'static crate::agent_reentry::ReentryCapability>,
}

impl AgentAdapter {
    /// The hook-based installation, when this agent has one.
    fn hook_install(&self) -> Option<&HookInstall> {
        match &self.install {
            Install::Hooks(install) => Some(install),
            Install::PiExtension => None,
        }
    }
}

pub(crate) const CLAUDE_CODE: AgentAdapter = AgentAdapter {
    source: "claude-code",
    class: SignalClass::Claude,
    install: Install::Hooks(HookInstall {
        hooks: CLAUDE_HOOKS,
        style: HookStyle::Exec,
        config_path: claude_settings_path,
        never_ran: "Claude Code has never run on this Mac — nothing to configure",
        lifecycle_hooks: CLAUDE_LIFECYCLE_HOOKS,
    }),
    reentry: Some(&crate::agent_reentry::claude::CAPABILITY),
};

pub(crate) const CODEX: AgentAdapter = AgentAdapter {
    source: "codex",
    class: SignalClass::Codex,
    install: Install::Hooks(HookInstall {
        hooks: CODEX_HOOKS,
        style: HookStyle::ShellAsync,
        config_path: codex_hooks_path,
        never_ran: "Codex has never run on this Mac — nothing to configure",
        lifecycle_hooks: CODEX_LIFECYCLE_HOOKS,
    }),
    reentry: Some(&crate::agent_reentry::codex::CAPABILITY),
};

/// Boot migration, the one write outside the Settings toggle: when hooks are
/// installed but are not what the CURRENT spec describes — a helper other
/// than this install's (the legacy `~/.deck/bin` copy, or a bundle that
/// moved), a matcher this deck version narrowed, an event it retired —
/// rewrite ONLY deck's hooks, every other byte of the file where it was,
/// then delete the legacy copy once nothing references it. Comparing deck's
/// whole hook and the matcher of its group is what lets a spec change
/// (`permission_prompt|idle_prompt` → `permission_prompt`) reach users who
/// enabled the toggle under an older version without touching it again.
/// A non-release build returns before reading anything.
fn migrate_hook_adapter_on_boot(adapter: &AgentAdapter, helper: &str) -> bool {
    let Some(install) = adapter.hook_install() else {
        return false;
    };
    let mut legacy_referenced = false;
    let (specs, source, style, never_ran) = (
        install.hooks,
        adapter.source,
        install.style,
        install.never_ran,
    );
    let Some(path) = (install.config_path)() else {
        return false;
    };
    let Ok(value) = read_settings_value(&path) else {
        return false;
    };
    // The Lifecycle family is migrated only where the user already
    // enabled it: a user with the status hooks alone is never enrolled.
    // A missing lifecycle helper leaves those entries exactly as they are.
    if !install.lifecycle_hooks.is_empty()
        && hooks_installed_of(Family::Lifecycle, &value, install.lifecycle_hooks)
    {
        let migrated = installed_family_helper_path(Family::Lifecycle).and_then(|helper| {
            let edit = HookEdit::Install {
                specs: install.lifecycle_hooks,
                source,
                style: lifecycle_style(install.style),
                helper: &helper,
            };
            hooks_set_of(Family::Lifecycle, &path, edit, never_ran)
        });
        match migrated {
            Ok(false) => {}
            Ok(true) => applog(&format!(
                "[agent-hooks] {source} lifecycle entries rewritten to the current spec"
            )),
            Err(e) => applog(&format!(
                "[agent-hooks] {source} lifecycle migration FAILED ({})",
                e.code()
            )),
        }
    }
    if !hooks_installed(&value, specs) {
        return false;
    }
    let edit = HookEdit::Install {
        specs,
        source,
        style,
        helper,
    };
    match hooks_set(&path, edit, never_ran) {
        // already what the current spec describes: the file is not written
        Ok(false) => {}
        Ok(true) => applog(&format!(
            "[agent-hooks] {source} entries rewritten to the current spec"
        )),
        Err(e) => {
            legacy_referenced =
                serde_json::to_string(&value).is_ok_and(|text| text.contains(LEGACY_HELPER_MARKER));
            applog(&format!(
                "[agent-hooks] {source} migration FAILED ({})",
                e.code()
            ));
        }
    }
    legacy_referenced
}

/// One registry-backed boot migration phase. Registration selects which
/// settings documents are worth reading; the closed install kind selects the
/// existing byte-preserving implementation. A duplicate source is skipped by
/// `unambiguous_agents`, so it causes no config read or migration side effect.
fn migrate_registered_agents(
    helper: Option<&str>,
    mut migrate_hook: impl FnMut(&AgentAdapter, &str) -> bool,
    retire_legacy: impl FnOnce(),
    mut migrate_pi: impl FnMut(),
) {
    if let Some(helper) = helper {
        let mut legacy_referenced = false;
        let mut hooks_visited = 0usize;
        for adapter in crate::extensions::unambiguous_agents() {
            if matches!(adapter.install, Install::Hooks(_)) {
                hooks_visited += 1;
                legacy_referenced |= migrate_hook(adapter, helper);
            }
        }
        if hooks_visited > 0 && !legacy_referenced {
            retire_legacy();
        }
    }
    // This second registry pass preserves the old ordering: hook migration
    // and legacy-copy retirement complete before Pi's existing-entry move.
    for adapter in crate::extensions::unambiguous_agents() {
        if matches!(adapter.install, Install::PiExtension) {
            migrate_pi();
        }
    }
}

pub(crate) fn migrate_agents_on_boot() {
    let helper = installed_helper_path().ok();
    migrate_registered_agents(
        helper.as_deref(),
        migrate_hook_adapter_on_boot,
        retire_legacy_helper_copy,
        crate::agent_pi::config::migrate_on_boot,
    );
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
            crate::error::err_code(&e.to_string())
        )),
    }
}

/// Apply one edit to an agent's hooks file. Says whether the file was
/// written: it is not when it already is what the edit asks for.
fn hooks_set(path: &Path, edit: HookEdit, never_ran: &str) -> Result<bool, DeckError> {
    hooks_set_of(Family::Signal, path, edit, never_ran)
}

/// `hooks_set` for one ownership family.
fn hooks_set_of(
    family: Family,
    path: &Path,
    edit: HookEdit,
    never_ran: &str,
) -> Result<bool, DeckError> {
    let original = read_settings_text(path)?;
    match hooks_file_edited_of(family, original.as_deref(), edit)? {
        Some(text) => write_agent_config(path, text.as_bytes(), never_ran).map(|()| true),
        None => Ok(false),
    }
}

#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub(crate) struct AgentSettingsStatus {
    signal: bool,
    lifecycle: Option<bool>,
    installation: Option<crate::agent_pi::config::Installation>,
}

#[derive(serde::Serialize)]
pub(crate) struct AgentHooksStatus {
    agents: std::collections::BTreeMap<&'static str, AgentSettingsStatus>,
    claude: bool,
    codex: bool,
    /// The separate re-entry observation opt-in (Lifecycle family).
    claude_reentry: bool,
    /// Pi's two switches and the state of its one extension entry, all
    /// derived from Pi's own settings file (`agent_pi::config`).
    pi: bool,
    pi_lifecycle: bool,
    pi_installation: crate::agent_pi::config::Installation,
}

#[tauri::command]
pub(crate) fn agent_hooks_status() -> AgentHooksStatus {
    let mut agents = std::collections::BTreeMap::new();
    for adapter in crate::extensions::unambiguous_agents() {
        let status = match &adapter.install {
            Install::Hooks(install) => {
                let value =
                    (install.config_path)().and_then(|path| read_settings_value(&path).ok());
                AgentSettingsStatus {
                    signal: value
                        .as_ref()
                        .is_some_and(|value| hooks_installed(value, install.hooks)),
                    lifecycle: (!install.lifecycle_hooks.is_empty()).then(|| {
                        if adapter.source == CLAUDE_CODE.source {
                            // Preserve the explicit project-bound re-entry
                            // experiment's alternate settings location while
                            // reusing the normal config read above.
                            match project_lifecycle_observation() {
                                Ok(Some(installed)) => installed,
                                Err(()) => false,
                                Ok(None) => value.as_ref().is_some_and(|value| {
                                    hooks_installed_of(
                                        Family::Lifecycle,
                                        value,
                                        install.lifecycle_hooks,
                                    )
                                }),
                            }
                        } else {
                            value.as_ref().is_some_and(|value| {
                                hooks_installed_of(
                                    Family::Lifecycle,
                                    value,
                                    install.lifecycle_hooks,
                                )
                            })
                        }
                    }),
                    installation: None,
                }
            }
            Install::PiExtension => {
                let status = crate::agent_pi::config::status();
                AgentSettingsStatus {
                    signal: status.signal,
                    lifecycle: Some(status.lifecycle),
                    installation: Some(status.installation),
                }
            }
        };
        agents.insert(adapter.source, status);
    }
    // Compatibility keys are projections of the same observations above;
    // they never perform a second config read.
    let claude = agents.get(CLAUDE_CODE.source).copied().unwrap_or_default();
    let codex = agents.get(CODEX.source).copied().unwrap_or_default();
    let pi = agents
        .get(crate::agent_pi::SOURCE)
        .copied()
        .unwrap_or_default();
    AgentHooksStatus {
        agents,
        claude: claude.signal,
        codex: codex.signal,
        claude_reentry: claude.lifecycle.unwrap_or(false),
        pi: pi.signal,
        pi_lifecycle: pi.lifecycle.unwrap_or(false),
        pi_installation: pi.installation.unwrap_or_default(),
    }
}

/// The user-driven writer of the agent CLI config files, from an explicit
/// Settings toggle. The only other writer is `migrate_agents_on_boot`, which
/// rewrites deck's own entries and nothing else.
#[tauri::command]
pub(crate) fn agent_hooks_set(agent: String, enable: bool) -> Result<(), DeckError> {
    if crate::launch_args::debug_arg("--smoke-reentry-config").is_some() {
        return Err(DeckError::new(
            ErrorKind::Other,
            "Signal hook editing is unavailable in this experiment",
        ));
    }
    let adapter = crate::extensions::agent(&agent)
        .ok_or(DeckError::new(ErrorKind::Other, "unknown agent"))?;
    let install = match &adapter.install {
        Install::Hooks(install) => install,
        // Pi has no hooks document: its one extension entry carries the
        // capability, and its own module writes it and says what happened.
        Install::PiExtension => {
            return crate::agent_pi::config::set(crate::agent_pi::Capability::Signal, enable)
        }
    };
    let (specs, source, style, never_ran) = (
        install.hooks,
        adapter.source,
        install.style,
        install.never_ran,
    );
    let path =
        (install.config_path)().ok_or(DeckError::new(ErrorKind::Other, "no home directory"))?;
    let result = if enable {
        installed_helper_path().and_then(|helper| {
            let edit = HookEdit::Install {
                specs,
                source,
                style,
                helper: &helper,
            };
            hooks_set(&path, edit, never_ran)
        })
    } else {
        hooks_set(&path, HookEdit::Uninstall, never_ran)
    }
    .map(|_written| ());
    match &result {
        Ok(()) => applog(&format!(
            "[agent-hooks] {agent} {}",
            if enable { "installed" } else { "removed" }
        )),
        Err(e) => applog(&format!(
            "[agent-hooks] {agent} {} FAILED ({})",
            if enable { "install" } else { "remove" },
            e.code()
        )),
    }
    result
}

/// The user's explicit opt-in to one agent's session re-entry observation
/// (`source`; Claude Code when none is named): adds or removes ONLY the
/// Lifecycle family's `SessionStart`/`SessionEnd` hooks in that agent's
/// hooks file. The status hooks, the user's hooks and every other byte
/// stay; a release install is required exactly as for the status hooks, so
/// a development or smoke build never edits the agent's settings.
#[tauri::command]
pub(crate) fn agent_lifecycle_hooks_set(
    enable: bool,
    source: Option<String>,
) -> Result<(), DeckError> {
    let source = source.as_deref().unwrap_or(CLAUDE_CODE.source);
    let adapter = crate::extensions::agent(source)
        .filter(|adapter| adapter.reentry.is_some())
        .ok_or(DeckError::new(ErrorKind::Other, "unknown agent"))?;
    let install = adapter
        .hook_install()
        .filter(|install| !install.lifecycle_hooks.is_empty())
        .ok_or(DeckError::new(ErrorKind::Other, "unknown agent"))?;
    let project = if source == CLAUDE_CODE.source {
        crate::launch_args::reentry_project_config()
            .map_err(|reason| DeckError::new(ErrorKind::Other, reason))?
    } else {
        None
    };
    let path = match project {
        Some(path) => path,
        None => {
            (install.config_path)().ok_or(DeckError::new(ErrorKind::Other, "no home directory"))?
        }
    };
    let result = if enable {
        installed_family_helper_path(Family::Lifecycle).and_then(|helper| {
            let edit = HookEdit::Install {
                specs: install.lifecycle_hooks,
                source: adapter.source,
                style: lifecycle_style(install.style),
                helper: &helper,
            };
            hooks_set_of(Family::Lifecycle, &path, edit, install.never_ran)
        })
    } else {
        hooks_set_of(
            Family::Lifecycle,
            &path,
            HookEdit::Uninstall,
            install.never_ran,
        )
    }
    .map(|_written| ());
    applog(&format!(
        "[agent-hooks] {source} lifecycle {} {}",
        if enable { "install" } else { "remove" },
        result.as_ref().map_or_else(|e| e.code(), |()| "ok")
    ));
    result
}

#[cfg(test)]
mod tests;
