//! project_env.rs — a project's environment variables: set in the project
//! defaults dialog, kept only in the login Keychain, handed to every shell
//! Deck creates for a card of that project.
//!
//! # Contract
//! - Storage: one Keychain item per project (`keychain::ENV_SERVICE`, the
//!   account the project id), its data the encoded ordered list. Nothing
//!   about it enters `~/.deck`: no name, no value, no flag, so `deck.json`,
//!   its backups, the MCP and Connector views of a project and diagnostics
//!   carry nothing of it, and the Board schema is unchanged. The Keychain is
//!   the encryption; Deck adds no cipher of its own.
//! - Write-only toward the webview: `project_env_names` returns NAMES only.
//!   `project_env_set` takes the whole list in order; an entry without a
//!   value keeps the stored value of that name (refused when there is none),
//!   an empty list deletes the item. No value ever travels back, enters a
//!   log line or an error message.
//! - Shape: at most `MAX_VARS` variables; a name is a POSIX identifier of at
//!   most `MAX_NAME` bytes, unique; `DECK_*` and `TMUX*` are refused (they
//!   are Deck's and tmux's own, and overriding them would cut a card off
//!   its Signal or its server). A value holds no control character (so it
//!   can never end a line of the tmux script) and is at most `MAX_VALUE`
//!   bytes; the whole list at most `MAX_TOTAL`. The same check runs on what
//!   the Keychain returns: a malformed item is an error, never "none".
//! - Injection (`commands::create_session`): the variables become the new
//!   session's own environment (`new-session -e NAME=VALUE`), so its login
//!   shell and everything it starts inherit them and no other session sees
//!   them. The `new-session` line is handed to tmux on STDIN
//!   (`start-server ; source-file -`), never as argv: an exec's arguments
//!   are recorded by the endpoint-security agent and visible in the process
//!   table. Every word of that line is single-quoted (`quote`), so tmux
//!   expands nothing in it ($, ~, formats, braces, `;` all stay literal).
//!   They are never typed into the pane (scrollback, shell history and the
//!   shell-state transcript stay free of them). A project without variables
//!   creates its sessions exactly as before.
//! - Which project: `start_session` is told it by the webview (a new card is
//!   persisted only after its session started). The scheduler's delivery
//!   start and the restart re-entry shell resolve it from the committed
//!   Board by card id, else by session name (`for_card`). A project that
//!   cannot be placed starts no shell while any project has variables: a
//!   start without them is never a guess Deck makes silently.
//! - Scope and limits: applied when a shell is CREATED. A running card keeps
//!   the environment it started with; a stopped card reopened later gets the
//!   current list. Shell rc files may still override a variable. Same-user
//!   processes can read a live session's environment (`ps -E`, `tmux
//!   show-environment`); this protects values at rest, not from the user's
//!   own processes or the Agent that is meant to use them. MCP-managed shells
//!   (`deck-mcp-runner`) are not given them.
//! - A list that cannot be read stops the project's new shells (a start
//!   without it would be a silent guess) and is never a dead end: its read
//!   fails with one of two fixed messages, `UNAVAILABLE` (the Keychain
//!   denied it or is locked) or `INVALID` (the stored bytes do not decode),
//!   which the webview turns into the reason a card did not start
//!   (`project-env-model.js` `envStartFailureKey`); the defaults dialog,
//!   whose read failed too, then offers clearing the list
//!   (`project_env_clear`, two clicks) so it can be set again.

use crate::error::{DeckError, ErrorKind};
use crate::sync::LockRecover;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

pub(crate) const MAX_VARS: usize = 64;
pub(crate) const MAX_NAME: usize = 128;
pub(crate) const MAX_VALUE: usize = 8 * 1024;
pub(crate) const MAX_TOTAL: usize = 32 * 1024;
const VERSION: u64 = 1;

/// Ordered (name, value) pairs, in the order the user listed them.
pub(crate) type Vars = Vec<(String, String)>;

/// Serializes read-modify-write of one project's item.
static WRITE: Mutex<()> = Mutex::new(());

fn invalid(message: &str) -> DeckError {
    DeckError::new(ErrorKind::Invalid, message.to_string())
}

/// Why a name is refused, or `None`.
pub(crate) fn name_problem(name: &str) -> Option<&'static str> {
    let mut bytes = name.bytes();
    let shape = name.len() <= MAX_NAME
        && bytes
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_');
    if !shape {
        return Some("name");
    }
    let upper = name.to_ascii_uppercase();
    if upper.starts_with("DECK_") || upper.starts_with("TMUX") {
        return Some("reserved");
    }
    None
}

fn value_ok(value: &str) -> bool {
    value.len() <= MAX_VALUE && !value.chars().any(char::is_control)
}

/// The whole list's shape (module header). Messages name the rule broken,
/// never a value.
pub(crate) fn validate(vars: &[(String, String)]) -> Result<(), DeckError> {
    if vars.len() > MAX_VARS {
        return Err(invalid("too many environment variables"));
    }
    let mut seen = HashSet::new();
    let mut total = 0usize;
    for (name, value) in vars {
        match name_problem(name) {
            Some("reserved") => {
                return Err(invalid(
                    "DECK_* and TMUX* environment variables are reserved",
                ))
            }
            Some(_) => return Err(invalid("invalid environment variable name")),
            None => {}
        }
        if !seen.insert(name.as_str()) {
            return Err(invalid("duplicate environment variable name"));
        }
        if !value_ok(value) {
            return Err(invalid(
                "an environment variable value is too long or holds a control character",
            ));
        }
        total += name.len() + value.len() + 2;
    }
    if total > MAX_TOTAL {
        return Err(invalid("environment variables are too large"));
    }
    Ok(())
}

fn encode(vars: &[(String, String)]) -> String {
    serde_json::json!({ "v": VERSION, "vars": vars }).to_string()
}

fn decode(data: &str) -> Result<Vars, DeckError> {
    #[derive(Deserialize)]
    struct Stored {
        v: u64,
        vars: Vars,
    }
    let bad = || DeckError::new(ErrorKind::Recovery, INVALID);
    let stored: Stored = serde_json::from_str(data).map_err(|_| bad())?;
    if stored.v != VERSION || validate(&stored.vars).is_err() {
        return Err(bad());
    }
    Ok(stored.vars)
}

/// The message of a list the Keychain would not hand over (denied, locked).
/// It and decode's "project environment is invalid" are the two the webview
/// recognises (`project-env-model.js` `envStartFailureKey`) to say why a
/// card of the project did not start, and where to clear the list.
pub(crate) const UNAVAILABLE: &str = "project environment unavailable";
/// The message of a stored list Deck cannot decode.
pub(crate) const INVALID: &str = "project environment is invalid";

/// A project's variables; empty when it has none.
pub(crate) fn for_project(project_id: &str) -> Result<Vars, DeckError> {
    let stored = crate::keychain::env_get(project_id).map_err(|error| match error.kind() {
        // the item is there but its bytes are not text: the list is broken
        ErrorKind::Recovery => DeckError::new(ErrorKind::Recovery, INVALID),
        kind => DeckError::new(kind, UNAVAILABLE),
    })?;
    match stored {
        Some(data) => decode(&data),
        None => Ok(Vars::new()),
    }
}

/// The project a card belongs to, from a Board's cards: by card id, else by
/// session name.
fn card_project(
    cards: &HashMap<String, crate::documents::CardTarget>,
    card_id: &str,
    session: &str,
) -> Option<String> {
    cards
        .get(card_id)
        .filter(|_| !card_id.is_empty())
        .or_else(|| cards.values().find(|card| card.session == session))
        .map(|card| card.project_id.clone())
        .filter(|id| !id.is_empty())
}

/// The variables for a shell the backend starts on its own (scheduler
/// delivery, restart re-entry): see the module header.
pub(crate) fn for_card(card_id: &str, session: &str) -> Result<Vars, DeckError> {
    resolve_for_card(
        crate::documents::card_targets(false).as_ref(),
        crate::keychain::env_any,
        card_id,
        session,
        &for_project,
    )
}

fn resolve_for_card(
    cards: Option<&HashMap<String, crate::documents::CardTarget>>,
    any: impl Fn() -> bool,
    card_id: &str,
    session: &str,
    load: &dyn Fn(&str) -> Result<Vars, DeckError>,
) -> Result<Vars, DeckError> {
    match cards.and_then(|cards| card_project(cards, card_id, session)) {
        Some(project) => load(&project),
        None if any() => Err(DeckError::new(
            ErrorKind::Missing,
            "the card's project is unknown, so its environment variables cannot be applied",
        )),
        None => Ok(Vars::new()),
    }
}

/// One word of a tmux command line, taken literally: single quotes, inside
/// which tmux expands nothing; an embedded `'` closes the quote, is given
/// double-quoted, and the quote reopens (tmux joins adjacent quoted parts
/// into one word, as sh does).
pub(crate) fn quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\"'\"'"))
}

/// The one `new-session` line `create_session` hands tmux on stdin.
/// `print_format`, when given, adds `-P -F <format>` (the restore path reads
/// the new pane's tty from it). Callers validated `vars`; name and dir are
/// checked again here because the line must stay ONE line.
pub(crate) fn new_session_script(
    name: &str,
    dir: &str,
    print_format: Option<&str>,
    vars: &[(String, String)],
) -> Result<String, DeckError> {
    if name.chars().chain(dir.chars()).any(char::is_control) {
        return Err(invalid(
            "session name or directory holds a control character",
        ));
    }
    validate(vars)?;
    let mut line = String::from("new-session -d");
    if let Some(format) = print_format {
        line.push_str(" -P -F ");
        line.push_str(&quote(format));
    }
    line.push_str(" -s ");
    line.push_str(&quote(name));
    line.push_str(" -c ");
    line.push_str(&quote(dir));
    for (key, value) in vars {
        line.push_str(" -e ");
        line.push_str(&quote(&format!("{key}={value}")));
    }
    line.push('\n');
    Ok(line)
}

/// The argv that carries `new_session_script` (module header).
pub(crate) const SCRIPT_ARGS: [&str; 4] = ["start-server", ";", "source-file", "-"];

/// An env-carrying start failed: tmux's own text may quote the script, so it
/// never leaves; the kind stays.
pub(crate) fn scrub(error: DeckError) -> DeckError {
    DeckError::new(
        error.kind(),
        "tmux could not create the session with the project's environment variables",
    )
}

// ---------- commands -------------------------------------------------------

#[derive(Deserialize)]
pub(crate) struct EnvEntry {
    name: String,
    /// `None` keeps the stored value of this name.
    value: Option<String>,
}

/// The names of a project's variables, in order. Reads the item's data (the
/// first read in a process may raise macOS's access prompt).
#[tauri::command]
pub(crate) fn project_env_names(project_id: String) -> Result<Vec<String>, DeckError> {
    Ok(for_project(&project_id)?
        .into_iter()
        .map(|(name, _)| name)
        .collect())
}

/// Replace a project's list (module header). Returns the stored names.
#[tauri::command]
pub(crate) fn project_env_set(
    project_id: String,
    entries: Vec<EnvEntry>,
) -> Result<Vec<String>, DeckError> {
    let _write = WRITE.lock_or_recover();
    let needs_stored = entries.iter().any(|entry| entry.value.is_none());
    let stored: HashMap<String, String> = if needs_stored {
        for_project(&project_id)?.into_iter().collect()
    } else {
        HashMap::new()
    };
    let vars = merge(entries, &stored)?;
    validate(&vars)?;
    if vars.is_empty() {
        crate::keychain::env_delete(&project_id)?;
    } else {
        crate::keychain::env_set(&project_id, &encode(&vars))?;
    }
    crate::applog::applog(&format!("[project-env] saved count={}", vars.len()));
    Ok(vars.into_iter().map(|(name, _)| name).collect())
}

fn merge(entries: Vec<EnvEntry>, stored: &HashMap<String, String>) -> Result<Vars, DeckError> {
    entries
        .into_iter()
        .map(|entry| {
            let name = entry.name.trim().to_string();
            let value = match entry.value {
                Some(value) => value,
                None => stored
                    .get(&name)
                    .cloned()
                    .ok_or_else(|| invalid("a kept environment variable has no stored value"))?,
            };
            Ok((name, value))
        })
        .collect()
}

/// Delete a project's item (the project was removed). Absent is success.
#[tauri::command]
pub(crate) fn project_env_clear(project_id: String) -> Result<(), DeckError> {
    let _write = WRITE.lock_or_recover();
    crate::keychain::env_delete(&project_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::CardTarget;

    fn vars(list: &[(&str, &str)]) -> Vars {
        list.iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn names_are_posix_identifiers_and_deck_or_tmux_ones_are_reserved() {
        assert_eq!(name_problem("ANTHROPIC_API_KEY"), None);
        assert_eq!(name_problem("_x1"), None);
        assert_eq!(name_problem("1X"), Some("name"));
        assert_eq!(name_problem(""), Some("name"));
        assert_eq!(name_problem("A-B"), Some("name"));
        assert_eq!(name_problem("A B"), Some("name"));
        assert_eq!(name_problem("Ä"), Some("name"));
        assert_eq!(name_problem(&"A".repeat(MAX_NAME + 1)), Some("name"));
        assert_eq!(name_problem("DECK_STATUS_SOCK"), Some("reserved"));
        assert_eq!(name_problem("deck_x"), Some("reserved"));
        assert_eq!(name_problem("TMUX"), Some("reserved"));
        assert_eq!(name_problem("TMUX_PANE"), Some("reserved"));
        assert_eq!(name_problem("DECKX"), None);
    }

    #[test]
    fn the_list_shape_is_bounded_and_values_hold_no_control_character() {
        assert!(validate(&vars(&[("A", "x y $HOME 'q' \"d\" ~ ;{}"), ("B", "")])).is_ok());
        assert!(validate(&vars(&[("A", "1"), ("A", "2")])).is_err());
        assert!(validate(&vars(&[("A", "a\nb")])).is_err());
        assert!(validate(&vars(&[("A", "a\tb")])).is_err());
        assert!(validate(&vars(&[("A", "a\u{0}b")])).is_err());
        assert!(validate(&vars(&[("A", &"x".repeat(MAX_VALUE + 1))])).is_err());
        let many: Vars = (0..=MAX_VARS)
            .map(|i| (format!("V{i}"), String::new()))
            .collect();
        assert!(validate(&many).is_err());
        let big: Vars = (0..5)
            .map(|i| (format!("V{i}"), "x".repeat(MAX_VALUE)))
            .collect();
        assert!(validate(&big).is_err());
        let error = validate(&vars(&[("A", "secret\n")])).unwrap_err();
        assert!(!error.message().contains("secret"));
    }

    #[test]
    fn stored_data_round_trips_and_malformed_data_is_an_error() {
        let list = vars(&[("B", "2"), ("A", "it's")]);
        assert_eq!(decode(&encode(&list)).unwrap(), list);
        assert!(decode("{}").is_err());
        assert!(decode("not json").is_err());
        assert!(decode(r#"{"v":2,"vars":[]}"#).is_err());
        assert!(decode(r#"{"v":1,"vars":[["TMUX","x"]]}"#).is_err());
        // a broken stored list says so in the one message the webview reads,
        // and clearing it is the way out
        crate::keychain::env_set("Ptest_broken", "not json").unwrap();
        assert_eq!(for_project("Ptest_broken").unwrap_err().message(), INVALID);
        assert!(project_env_names("Ptest_broken".into()).is_err());
        project_env_clear("Ptest_broken".into()).unwrap();
        assert!(project_env_names("Ptest_broken".into()).unwrap().is_empty());
    }

    #[test]
    fn quoting_keeps_every_word_literal_and_on_one_line() {
        assert_eq!(quote("plain"), "'plain'");
        assert_eq!(quote("it's"), "'it'\"'\"'s'");
        let script = new_session_script(
            "deck-1",
            "/tmp/a b",
            Some("#{pane_tty}"),
            &vars(&[("K", "v'1")]),
        )
        .unwrap();
        assert_eq!(
            script,
            "new-session -d -P -F '#{pane_tty}' -s 'deck-1' -c '/tmp/a b' -e 'K=v'\"'\"'1'\n"
        );
        assert!(new_session_script("x", "/tmp/a\nb", None, &[]).is_err());
        assert!(new_session_script("x", "/tmp", None, &vars(&[("K", "a\nb")])).is_err());
    }

    #[test]
    fn set_keeps_named_values_and_an_empty_list_deletes() {
        let project = "Ptest-project-env-set";
        let entry = |name: &str, value: Option<&str>| EnvEntry {
            name: name.into(),
            value: value.map(str::to_string),
        };
        // keeping a value that was never stored is refused
        assert!(project_env_set(project.into(), vec![entry("A", None)]).is_err());
        let names = project_env_set(
            project.into(),
            vec![entry(" A ", Some("one")), entry("B", Some("two"))],
        )
        .unwrap();
        assert_eq!(names, ["A", "B"]);
        let names = project_env_set(
            project.into(),
            vec![entry("B", None), entry("C", Some("three"))],
        )
        .unwrap();
        assert_eq!(names, ["B", "C"]);
        assert_eq!(
            for_project(project).unwrap(),
            vars(&[("B", "two"), ("C", "three")])
        );
        assert_eq!(project_env_names(project.into()).unwrap(), ["B", "C"]);
        assert!(project_env_set(project.into(), vec![entry("TMUX_X", Some("1"))]).is_err());
        // a refused write leaves the stored list as it was
        assert_eq!(project_env_names(project.into()).unwrap(), ["B", "C"]);
        project_env_set(project.into(), Vec::new()).unwrap();
        assert!(for_project(project).unwrap().is_empty());
        project_env_set(project.into(), vec![entry("A", Some("1"))]).unwrap();
        project_env_clear(project.into()).unwrap();
        assert!(for_project(project).unwrap().is_empty());
        project_env_clear(project.into()).unwrap();
    }

    #[test]
    fn a_card_is_placed_by_id_then_session_and_an_unplaced_one_fails_closed() {
        let card = |session: &str, project: &str| CardTarget {
            session: session.into(),
            title: String::new(),
            dir: String::new(),
            cmd: String::new(),
            project_id: project.into(),
            column_id: String::new(),
            reminder: false,
        };
        let cards: HashMap<String, CardTarget> = [
            ("c1".to_string(), card("s1", "P1")),
            ("c2".to_string(), card("s2", "P2")),
            ("c3".to_string(), card("s3", "")),
        ]
        .into_iter()
        .collect();
        let load = |project: &str| Ok(vars(&[("PROJECT", project)]));
        let got =
            |cards: Option<&HashMap<String, CardTarget>>, any: bool, id: &str, session: &str| {
                resolve_for_card(cards, || any, id, session, &load)
            };
        assert_eq!(
            got(Some(&cards), true, "c1", "zzz").unwrap(),
            vars(&[("PROJECT", "P1")])
        );
        assert_eq!(
            got(Some(&cards), true, "", "s2").unwrap(),
            vars(&[("PROJECT", "P2")])
        );
        assert_eq!(
            got(Some(&cards), true, "gone", "s2").unwrap(),
            vars(&[("PROJECT", "P2")])
        );
        // no project anywhere has variables: nothing to apply
        assert!(got(Some(&cards), false, "gone", "nope").unwrap().is_empty());
        assert!(got(None, false, "c1", "s1").unwrap().is_empty());
        // some project has variables and this card cannot be placed: refuse
        assert!(got(Some(&cards), true, "gone", "nope").is_err());
        assert!(got(Some(&cards), true, "c3", "s3").is_err());
        assert!(got(None, true, "c1", "s1").is_err());
    }
}
