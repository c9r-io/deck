//! agy (Google Antigravity CLI) adapter: its one hooks entry and what its
//! two words mean.
//!
//! # Contract
//! - **Two words, from two hooks, no third.** agy 1.5.14 has six hook
//!   events and none of them says it waits for the user: a permission
//!   question or a question to the user runs no hook. So an agy card is
//!   `working` from the first model call of a turn (`PreInvocation` with
//!   `invocationNum` 0: the helper's `--turn-start` filter, so one line per
//!   turn instead of one per model call) until the turn's `Stop`
//!   (`turn-done`), and while agy asks for permission it still reads
//!   `working`. Deck never guesses `needs-input` for it (no quiet-time, no
//!   screen, no "a tool started and did not finish"): the card's status
//!   says that agy does not report its questions (`ui/js/extension-agy.js`,
//!   the coverage note), and that is all. Runtime proof (agy 1.5.14,
//!   2026-11-23): the hook's `sh` is agy's direct child on agy's terminal,
//!   agy the foreground group's leader, so the ordinary kernel admission
//!   (`agent_status::ingest_accepted`) applies unchanged.
//! - **Never PreToolUse, never output.** agy reads a hook's stdout as its
//!   answer; for `PreToolUse` an empty object DENIES the tool (probed), so
//!   Deck writes no such hook. For `PreInvocation` and `Stop` an empty
//!   stdout is no answer and changes nothing (probed); the helper writes
//!   nothing, ever.
//! - **v1 lines.** agy carries only a conversation id, the same for every
//!   turn, and counters; none is a per-turn id, and none is synthesized
//!   (`deck-status-helper` `identity_field`: none for `agy`).
//! - **Known gaps.** Esc (during a model call or on a permission question)
//!   runs no `Stop`: the card stays `working` until the next turn's first
//!   model call. `Stop`'s `fullyIdle` (background commands still running) is
//!   not read: `turn-done` never meant idle, and agy starts a turn of its own
//!   when the background work ends (probed), which reads `working` again.
//! - **One entry in agy's global file.** `~/.gemini/config/hooks.json` is an
//!   object of NAMED hooks. Deck owns exactly the member `deck-status`, and
//!   only while a command in it is the bundled status helper
//!   (`agent_status::status_helper_command`); a `deck-status` that is not
//!   Deck's is `agy-entry-foreign` and is never replaced. Every other member
//!   and every other byte stays as written: the text is edited in place
//!   with the span tools of `agent_status::hooks` and used only when it
//!   parses back to exactly the decided value. A file that is not one JSON
//!   object, or repeats a top-level key, is `agy-hooks-unreadable` and left
//!   alone. agy's config directory missing is "never ran": nothing is
//!   created.
//! - **The user's own disabling wins.** `"enabled": false` in Deck's entry
//!   (agy's own switch) reads as off; turning the switch on is refused
//!   (`agy-disabled-by-user`) and the boot migration leaves it.
//! - **Only from a stable install**, as every hook write
//!   (`agent_status::installed_helper_path`). The boot migration rewrites an
//!   entry that is there to this bundle's helper and enrolls nobody.
//! - **Folder trust refuses, never releases** (approved 2026-11-23). agy
//!   asks "Do you trust the contents of this project?" the first time it
//!   runs in a directory — trust is kept per EXACT directory
//!   (`trustedWorkspaces` in `~/.gemini/antigravity-cli/settings.json`; a
//!   trusted parent does not cover its children, probed on 1.5.14 and
//!   1.5.15) — and Enter there answers "Yes, I trust this folder": a typed
//!   line is dropped and its Enter grants trust (seen on a worker card,
//!   2026-11-23). So before agy has reported a turn, a row for an agy
//!   whose working directory (the pane's current path) is not in that list
//!   holds at `Hold::AgyUntrusted` (`scheduler/select.rs`), whatever
//!   first-send override it carries; the user answers agy in the card and
//!   the row goes on its own. The list is read only to refuse: unreadable,
//!   unparsable or missing reads as untrusted, and a trusted directory is
//!   no readiness evidence — it only returns the row to the gates that
//!   held it before. Deck never writes that file.
//! - **Recognition only.** An accepted agy word marks the pane generation
//!   `agy_interaction` (`agent_status::GenerationEvidence`), the evidence the
//!   scheduler's first-interaction gate asks for an agy row
//!   (`scheduler/select.rs`). It is not readiness, authority or task
//!   progress, and remote admission (`admission::interactive_agent`) does
//!   not list agy.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::agent_status::{
    items, layout, read_settings_text, rebuilt, skip_space, value_end, written, AgentAdapter,
    Install, SignalClass,
};
use crate::applog::applog;
use crate::error::{DeckError, ErrorKind};

/// The wire source word, also the Settings toggle's id.
pub(crate) const SOURCE: &str = "agy";
/// agy's executable basename (a worker type's command, a row's
/// `expected_process`).
pub(crate) const PROCESS: &str = "agy";
/// The one member of agy's hooks file that is Deck's.
const ENTRY: &str = "deck-status";
/// agy's default is 30 s and its hooks block its loop; the helper is done
/// in milliseconds and gives up after 2 s.
const HOOK_TIMEOUT_SECS: u64 = 5;
const NEVER_RAN: &str = "agy has never run on this Mac — nothing to configure";

pub(crate) const AGY: AgentAdapter = AgentAdapter {
    source: SOURCE,
    class: SignalClass::Agy,
    install: Install::AgyHooks,
    reentry: None,
};

fn refused(kind: ErrorKind, code: &'static str) -> DeckError {
    DeckError::new(kind, code)
}

/// `~/.gemini/config/hooks.json`, agy's global customization root.
fn config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".gemini").join("config").join("hooks.json"))
}

/// agy's own settings, where it records the directories the user trusted.
fn agy_settings_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| {
        home.join(".gemini")
            .join("antigravity-cli")
            .join("settings.json")
    })
}

/// The directories agy was told to trust, canonical where they still
/// exist (`/tmp` is `/private/tmp`). `None` when the file is missing,
/// unreadable or not the shape agy writes: nothing is trusted then.
pub(crate) fn trusted_folders() -> Option<Vec<PathBuf>> {
    let text = std::fs::read_to_string(agy_settings_path()?).ok()?;
    trusted_from(&text)
}

fn trusted_from(text: &str) -> Option<Vec<PathBuf>> {
    let value: Value = serde_json::from_str(text).ok()?;
    let list = value.get("trustedWorkspaces")?.as_array()?;
    Some(
        list.iter()
            .filter_map(Value::as_str)
            .filter(|entry| Path::new(entry).is_absolute())
            .map(|entry| std::fs::canonicalize(entry).unwrap_or_else(|_| PathBuf::from(entry)))
            .collect(),
    )
}

/// Whether agy trusts exactly `dir` (a pane's current path): the same
/// directory, never a parent of it.
pub(crate) fn trusts(trusted: &[PathBuf], dir: &str) -> bool {
    let dir = Path::new(dir);
    if !dir.is_absolute() {
        return false;
    }
    let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    trusted.contains(&dir)
}

/// Deck's entry for `helper`: the turn's first model call and its end.
fn entry(helper: &str) -> Value {
    let command = |words: &str| format!("\"{helper}\" {SOURCE} {words}");
    json!({
        "PreInvocation": [
            {"type": "command", "command": command("working --turn-start"), "timeout": HOOK_TIMEOUT_SECS}
        ],
        "Stop": [
            {"type": "command", "command": command("turn-done"), "timeout": HOOK_TIMEOUT_SECS}
        ]
    })
}

/// What the file holds under `deck-status`.
#[derive(Debug, PartialEq, Eq)]
enum Held {
    Absent,
    /// Deck's: `current` is exactly this bundle's entry, `disabled` is agy's
    /// own `"enabled": false` on it.
    Ours {
        current: bool,
        disabled: bool,
    },
    Foreign,
}

/// Every `command` string anywhere in `value`.
fn commands(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, inner) in map {
                match (key.as_str(), inner) {
                    ("command", Value::String(command)) => out.push(command.clone()),
                    _ => commands(inner, out),
                }
            }
        }
        Value::Array(list) => list.iter().for_each(|inner| commands(inner, out)),
        _ => {}
    }
}

fn held(document: &Value, helper: Option<&str>) -> Held {
    let Some(value) = document.get(ENTRY) else {
        return Held::Absent;
    };
    let mut found = Vec::new();
    commands(value, &mut found);
    if !found
        .iter()
        .any(|command| crate::agent_status::status_helper_command(command))
    {
        return Held::Foreign;
    }
    Held::Ours {
        current: helper.is_some_and(|helper| *value == entry(helper)),
        disabled: value.get("enabled") == Some(&Value::Bool(false)),
    }
}

/// The document a hooks text holds: one JSON object without a repeated
/// top-level key. No file is an empty object.
fn document(text: Option<&str>) -> Result<Value, DeckError> {
    let unreadable = || refused(ErrorKind::InvalidDoc, "agy-hooks-unreadable");
    let Some(text) = text else {
        return Ok(json!({}));
    };
    let value: Value = serde_json::from_str(text).map_err(|_| unreadable())?;
    if !value.is_object() {
        return Err(unreadable());
    }
    let bytes = text.as_bytes();
    let start = skip_space(bytes, 0);
    let root = start..value_end(bytes, start).ok_or_else(unreadable)?;
    let members = items(text, &root).ok_or_else(unreadable)?;
    let mut keys = std::collections::HashSet::new();
    if !members.iter().all(|member| {
        member
            .key
            .as_ref()
            .is_some_and(|key| keys.insert(key.clone()))
    }) {
        return Err(unreadable());
    }
    Ok(value)
}

/// `text` with Deck's member set to `target` (added after the last member
/// when absent) or, for `None`, taken out; every other byte where it was.
/// `None` when the span tools cannot follow the text or the result does not
/// parse back to exactly the decided document.
fn edited(text: Option<&str>, document: &Value, target: Option<&Value>) -> Option<String> {
    let mut decided = document.clone();
    let object = decided.as_object_mut()?;
    match target {
        Some(value) => {
            object.insert(ENTRY.to_string(), value.clone());
        }
        None => {
            object.remove(ENTRY);
        }
    }
    let Some(text) = text else {
        let mut out = serde_json::to_string_pretty(&decided).ok()?;
        out.push('\n');
        return Some(out);
    };
    let bytes = text.as_bytes();
    let start = skip_space(bytes, 0);
    let root = start..value_end(bytes, start)?;
    let members = items(text, &root)?;
    let lead = layout(text, &root, &members).lead;
    let mine = members
        .iter()
        .position(|member| member.key.as_deref() == Some(ENTRY));
    let value_text = match target {
        Some(value) => Some(written(value, text, &lead)?),
        None => None,
    };
    let body = match (mine, value_text) {
        (Some(at), Some(value)) => {
            let span = &members[at].value;
            format!("{}{value}{}", &text[..span.start], &text[span.end..])
        }
        (at, value) => {
            let kept: Vec<Option<String>> = members
                .iter()
                .enumerate()
                .map(|(index, member)| {
                    (Some(index) != at).then(|| text[member.whole.clone()].to_string())
                })
                .collect();
            let added: Vec<String> = value
                .map(|value| format!("\"{ENTRY}\": {value}"))
                .into_iter()
                .collect();
            format!(
                "{}{}{}",
                &text[..root.start],
                rebuilt(text, &root, &members, &kept, &added),
                &text[root.end..]
            )
        }
    };
    (serde_json::from_str::<Value>(&body).ok()? == decided).then_some(body)
}

/// One switch or migration step.
#[derive(Clone, Copy)]
enum Edit<'a> {
    Install(&'a str),
    Uninstall,
    Migrate(&'a str),
}

/// Apply `edit` to the file at `path`. Says whether it was written.
fn apply(path: &Path, edit: Edit) -> Result<bool, DeckError> {
    let text = read_settings_text(path)
        .map_err(|_| refused(ErrorKind::InvalidDoc, "agy-hooks-unreadable"))?;
    let document = document(text.as_deref())?;
    let helper = match edit {
        Edit::Install(helper) | Edit::Migrate(helper) => Some(helper),
        Edit::Uninstall => None,
    };
    let target = match (edit, held(&document, helper)) {
        (Edit::Install(_), Held::Foreign) => {
            return Err(refused(ErrorKind::Other, "agy-entry-foreign"))
        }
        (Edit::Install(_), Held::Ours { disabled: true, .. }) => {
            return Err(refused(ErrorKind::Other, "agy-disabled-by-user"))
        }
        (Edit::Install(_) | Edit::Migrate(_), Held::Ours { current: true, .. }) => {
            return Ok(false)
        }
        (Edit::Install(helper), _) => Some(entry(helper)),
        (
            Edit::Migrate(helper),
            Held::Ours {
                disabled: false, ..
            },
        ) => Some(entry(helper)),
        (Edit::Migrate(_), _) => return Ok(false),
        (Edit::Uninstall, Held::Ours { .. }) => None,
        (Edit::Uninstall, _) => return Ok(false),
    };
    let next = edited(text.as_deref(), &document, target.as_ref())
        .ok_or_else(|| refused(ErrorKind::InvalidDoc, "agy-hooks-unreadable"))?;
    crate::agent_status::write_agent_config(path, next.as_bytes(), NEVER_RAN)?;
    Ok(true)
}

/// Whether the switch is on: Deck's entry is there and agy has not been
/// told to skip it. Read from the file every time; nothing else stores it.
pub(crate) fn enabled() -> bool {
    config_path()
        .and_then(|path| read_settings_text(&path).ok())
        .and_then(|text| document(text.as_deref()).ok())
        .is_some_and(|document| {
            matches!(
                held(&document, None),
                Held::Ours {
                    disabled: false,
                    ..
                }
            )
        })
}

/// The Settings switch (`agent_status::agent_hooks_set`).
pub(crate) fn set(enable: bool) -> Result<(), DeckError> {
    let path = config_path().ok_or(DeckError::new(ErrorKind::Other, "no home directory"))?;
    let result = if enable {
        crate::agent_status::installed_helper_path()
            .and_then(|helper| apply(&path, Edit::Install(&helper)))
    } else {
        apply(&path, Edit::Uninstall)
    };
    applog(&format!(
        "[agent-hooks] agy {} {}",
        if enable { "install" } else { "remove" },
        result.as_ref().map_or_else(|e| e.code(), |_| "ok")
    ));
    result.map(|_written| ())
}

/// The boot migration (stable installs only): Deck's entry, when there and
/// not disabled, is rewritten to this bundle's helper; nothing else.
pub(crate) fn migrate_on_boot(helper: &str) {
    let Some(path) = config_path() else {
        return;
    };
    match apply(&path, Edit::Migrate(helper)) {
        Ok(false) => {}
        Ok(true) => applog("[agent-hooks] agy entry rewritten to the current spec"),
        Err(e) => applog(&format!(
            "[agent-hooks] agy migration FAILED ({})",
            e.code()
        )),
    }
}

#[cfg(test)]
mod tests;
