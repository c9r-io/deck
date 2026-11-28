//! Deck's statusLine entry: the user's "Quota" switch (Settings › Agent).
//!
//! Claude Code (`~/.claude/settings.json`; a `CLAUDE_CONFIG_DIR` folder
//! that layers it with `--settings`, such as a second account, reads the
//! same entry) and agy (`~/.gemini/antigravity-cli/settings.json`) each
//! have ONE `statusLine` slot, `{type: "command", command, …}`, whose
//! command the agent runs through its shell with the session JSON on
//! stdin. Deck's entry is the bundled helper as a tee:
//! - no statusLine before: `'<helper>' --quota <source> --quiet` (agy also
//!   gets `stack_with_default: true`, so its built-in line stays);
//! - the user's command X before: `'<helper>' --quota <source> | X`. X runs
//!   exactly as before, in the agent's own shell, with the same stdin.
//!
//! The file is the only state: `On` when the command is Deck's form
//! (`ours`), `User` when another command holds the slot, `Off` when there
//! is none. Switching on wraps whatever is there; switching off puts X
//! back (or removes the quiet entry Deck added). A command the user changed
//! after Deck wrapped it is theirs: Deck shows the switch off and wraps it
//! again only when the user switches on. Boot migration rewrites only
//! Deck's own prefix (a moved bundle). Every edit keeps the file's other
//! bytes (`agent_status` JSON text primitives), is written only when the
//! text parses back to exactly the decided value, goes through the agent
//! config writer (symlinks followed, a missing agent folder refused), and a
//! file that is not JSON is refused untouched. Only a release-location
//! bundle may write (`installed_helper_path`), like the hooks.

use crate::agent_status::{
    installed_helper_path, items, read_settings_text, rebuilt, skip_space, value_end,
    write_agent_config, written, Span,
};
use crate::applog::applog;
use crate::error::{DeckError, ErrorKind};
use std::path::{Path, PathBuf};

const KEY: &str = "statusLine";
const HELPER_FILE: &str = "/deck-status-helper";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    Claude,
    Agy,
}

impl Target {
    const ALL: [Target; 2] = [Target::Claude, Target::Agy];

    fn source(self) -> &'static str {
        match self {
            Target::Claude => "claude-code",
            Target::Agy => "agy",
        }
    }

    fn path(self, home: &Path) -> PathBuf {
        match self {
            Target::Claude => home.join(".claude").join("settings.json"),
            Target::Agy => home
                .join(".gemini")
                .join("antigravity-cli")
                .join("settings.json"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum State {
    /// The agent's folder does not exist: it never ran here.
    Absent,
    Off,
    /// Another command holds the slot.
    User,
    On,
    /// The file is not JSON, or its statusLine is not a command.
    Unreadable,
}

/// Deck's form of a statusLine command: (helper path, the user's command
/// it pipes into). `None` for anything else.
pub(crate) fn ours(command: &str, source: &str) -> Option<(String, Option<String>)> {
    let rest = command.strip_prefix('\'')?;
    let (helper, rest) = rest.split_once('\'')?;
    if !helper.ends_with(HELPER_FILE) || !helper.starts_with('/') {
        return None;
    }
    let rest = rest.strip_prefix(&format!(" --quota {source}"))?;
    match rest {
        " --quiet" => Some((helper.to_string(), None)),
        _ => {
            let user = rest.strip_prefix(" | ")?;
            (!user.trim().is_empty()).then(|| (helper.to_string(), Some(user.to_string())))
        }
    }
}

/// Deck's command for `helper`, piping into `user` when there is one.
pub(crate) fn command_for(helper: &str, source: &str, user: Option<&str>) -> String {
    match user {
        Some(user) => format!("'{helper}' --quota {source} | {user}"),
        None => format!("'{helper}' --quota {source} --quiet"),
    }
}

/// The state of one settings text.
pub(crate) fn state_of(text: Option<&str>, target: Target) -> State {
    let Some(text) = text else {
        return State::Off;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return State::Unreadable;
    };
    let Some(root) = value.as_object() else {
        return State::Unreadable;
    };
    let Some(line) = root.get(KEY) else {
        return State::Off;
    };
    match line.get("command").and_then(|c| c.as_str()) {
        Some(command) if ours(command, target.source()).is_some() => State::On,
        Some(_) => State::User,
        None => State::Unreadable,
    }
}

fn root_span(text: &str) -> Option<Span> {
    let bytes = text.as_bytes();
    let start = skip_space(bytes, 0);
    let end = value_end(bytes, start)?;
    (bytes.get(start) == Some(&b'{') && skip_space(bytes, end) == bytes.len()).then_some(start..end)
}

/// `text` with `span` replaced by `with`.
fn spliced(text: &str, span: &Span, with: &str) -> String {
    format!("{}{with}{}", &text[..span.start], &text[span.end..])
}

/// The settings text after switching Deck's entry on (`helper` given) or
/// off (`None`): `Ok(None)` when nothing changes, `Err` (a closed reason)
/// when Deck may not edit this file.
pub(crate) fn edited(
    text: Option<&str>,
    target: Target,
    helper: Option<&str>,
) -> Result<Option<String>, &'static str> {
    let source = target.source();
    let original = text.unwrap_or("{}\n");
    let mut value: serde_json::Value = serde_json::from_str(original).map_err(|_| "not-json")?;
    let root = value.as_object_mut().ok_or("not-an-object")?;
    let span = root_span(original).ok_or("not-an-object")?;
    let members = items(original, &span).ok_or("unfollowable")?;
    let slot = members.iter().position(|m| m.key.as_deref() == Some(KEY));
    let command_now = root.get(KEY).map(|line| {
        line.get("command")
            .and_then(|c| c.as_str())
            .map(str::to_owned)
    });
    // what to do: replace the command with `Some(text)`, remove the
    // member (`Remove`), add a new member (`Add`)
    enum Edit {
        Command(String),
        Remove,
        Add(serde_json::Value),
    }
    let edit = match (helper, command_now) {
        (Some(helper), None) => {
            let mut line = serde_json::json!({"type": "command", "command": command_for(helper, source, None)});
            if target == Target::Agy {
                line["stack_with_default"] = true.into();
            }
            Edit::Add(line)
        }
        (_, Some(None)) => return Err("not-a-command"),
        (Some(helper), Some(Some(command))) => match ours(&command, source) {
            Some((current, _)) if current == helper => return Ok(None),
            Some((_, user)) => Edit::Command(command_for(helper, source, user.as_deref())),
            None => {
                let kind = root
                    .get(KEY)
                    .and_then(|l| l.get("type"))
                    .and_then(|t| t.as_str());
                if kind.is_some_and(|k| k != "command") {
                    return Err("not-a-command");
                }
                Edit::Command(command_for(helper, source, Some(&command)))
            }
        },
        (None, None) => return Ok(None),
        (None, Some(Some(command))) => match ours(&command, source) {
            None => return Ok(None),
            Some((_, Some(user))) => Edit::Command(user),
            Some((_, None)) => Edit::Remove,
        },
    };
    let text = match &edit {
        Edit::Add(line) => {
            let kept: Vec<Option<String>> = members
                .iter()
                .map(|m| Some(original[m.whole.clone()].to_string()))
                .collect();
            let layout = crate::agent_status::layout(original, &span, &members);
            let body = written(line, original, &layout.lead).ok_or("unfollowable")?;
            let added = format!("\"{KEY}\": {body}");
            spliced(
                original,
                &span,
                &rebuilt(original, &span, &members, &kept, &[added]),
            )
        }
        Edit::Remove => {
            let kept: Vec<Option<String>> = members
                .iter()
                .enumerate()
                .map(|(i, m)| (Some(i) != slot).then(|| original[m.whole.clone()].to_string()))
                .collect();
            spliced(
                original,
                &span,
                &rebuilt(original, &span, &members, &kept, &[]),
            )
        }
        Edit::Command(command) => {
            let line = &members[slot.ok_or("unfollowable")?];
            let inner = items(original, &line.value).ok_or("unfollowable")?;
            let at = inner
                .iter()
                .find(|m| m.key.as_deref() == Some("command"))
                .ok_or("unfollowable")?;
            let quoted = serde_json::to_string(command).map_err(|_| "unfollowable")?;
            spliced(original, &at.value, &quoted)
        }
    };
    // the value the edit decided, and the text must parse back to it
    match edit {
        Edit::Add(line) => {
            root.insert(KEY.into(), line);
        }
        Edit::Remove => {
            root.remove(KEY);
        }
        Edit::Command(command) => {
            root[KEY]["command"] = command.into();
        }
    }
    let back: serde_json::Value = serde_json::from_str(&text).map_err(|_| "unfollowable")?;
    if back != value {
        return Err("unfollowable");
    }
    Ok(Some(text))
}

/// A helper path the tee command can quote in single quotes.
fn quotable(helper: &str) -> Result<&str, DeckError> {
    if helper.contains('\'') {
        return Err(DeckError::new(
            ErrorKind::Other,
            "the application path contains a quote a statusLine command cannot carry",
        ));
    }
    Ok(helper)
}

fn edit_target(home: &Path, target: Target, helper: Option<&str>) -> Result<bool, DeckError> {
    let path = target.path(home);
    if !path.parent().is_some_and(Path::exists) {
        return Ok(false);
    }
    let text = read_settings_text(&path)?;
    match edited(text.as_deref(), target, helper) {
        Ok(None) => Ok(false),
        Ok(Some(text)) => {
            write_agent_config(&path, text.as_bytes(), "the agent has not run on this Mac")?;
            Ok(true)
        }
        Err(reason) => Err(DeckError::new(
            ErrorKind::Other,
            format!(
                "the {} settings file cannot take Deck's statusLine ({reason}) — not modifying it",
                target.source()
            ),
        )),
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct QuotaStatus {
    claude: State,
    agy: State,
    /// Whether this build may write the agent settings (a release bundle).
    writable: bool,
}

impl QuotaStatus {
    /// The Claude Code and agy statusLine states, as the files say now.
    #[cfg_attr(test, allow(dead_code))]
    pub(crate) fn states(&self) -> (State, State) {
        (self.claude, self.agy)
    }
}

pub(crate) fn status_now() -> QuotaStatus {
    let home = dirs::home_dir().unwrap_or_default();
    let state = |target: Target| {
        let path = target.path(&home);
        if !path.parent().is_some_and(Path::exists) {
            return State::Absent;
        }
        match read_settings_text(&path) {
            Ok(text) => state_of(text.as_deref(), target),
            Err(_) => State::Unreadable,
        }
    };
    QuotaStatus {
        claude: state(Target::Claude),
        agy: state(Target::Agy),
        writable: installed_helper_path().is_ok(),
    }
}

/// The switch: every target whose agent folder exists.
pub(crate) fn set_now(enable: bool) -> Result<(), DeckError> {
    let home = dirs::home_dir().ok_or(DeckError::new(ErrorKind::Missing, "no home folder"))?;
    let helper = installed_helper_path()?;
    let helper = quotable(&helper)?;
    let mut first_error = None;
    for target in Target::ALL {
        match edit_target(&home, target, enable.then_some(helper)) {
            Ok(changed) => {
                if changed {
                    applog(&format!(
                        "[quota] statusline {} {}",
                        target.source(),
                        if enable { "on" } else { "off" }
                    ));
                }
            }
            Err(e) => {
                first_error.get_or_insert(e);
            }
        }
    }
    first_error.map_or(Ok(()), Err)
}

#[tauri::command]
pub(crate) async fn quota_status() -> Result<QuotaStatus, DeckError> {
    crate::session_runtime::off_ipc_thread(
        "quota_status",
        std::time::Duration::from_secs(5),
        move || Ok(status_now()),
    )
    .await
}

#[tauri::command]
pub(crate) async fn quota_set(enable: bool) -> Result<QuotaStatus, DeckError> {
    crate::session_runtime::off_ipc_thread(
        "quota_set",
        std::time::Duration::from_secs(5),
        move || set_now(enable).map(|()| status_now()),
    )
    .await
}

/// Boot (release installs only): an entry that is Deck's but names another
/// helper path gets this install's; nothing else is touched.
pub(crate) fn migrate_on_boot() {
    let (Some(home), Ok(helper)) = (dirs::home_dir(), installed_helper_path()) else {
        return;
    };
    let Ok(helper) = quotable(&helper) else {
        return;
    };
    for target in Target::ALL {
        let path = target.path(&home);
        let Ok(Some(text)) = read_settings_text(&path) else {
            continue;
        };
        if state_of(Some(&text), target) != State::On {
            continue;
        }
        if let Ok(Some(next)) = edited(Some(&text), target, Some(helper)) {
            match write_agent_config(&path, next.as_bytes(), "the agent has not run on this Mac") {
                Ok(()) => applog(&format!("[quota] statusline {} migrated", target.source())),
                Err(e) => applog(&format!(
                    "[quota] statusline {} migrate failed kind={:?}",
                    target.source(),
                    e.kind()
                )),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: &str = "/Applications/deck.app/Contents/MacOS/deck-status-helper";
    const H2: &str = "/Users/u/Applications/deck.app/Contents/MacOS/deck-status-helper";

    fn on(text: Option<&str>, target: Target) -> String {
        edited(text, target, Some(H)).unwrap().unwrap()
    }

    #[test]
    fn our_form_is_recognized_exactly() {
        assert_eq!(
            ours(&command_for(H, "claude-code", None), "claude-code"),
            Some((H.into(), None))
        );
        assert_eq!(
            ours(&command_for(H, "agy", Some("~/bin/line.sh --x")), "agy"),
            Some((H.into(), Some("~/bin/line.sh --x".into())))
        );
        assert_eq!(ours(&command_for(H, "agy", None), "claude-code"), None);
        assert_eq!(
            ours("'/x/other' --quota claude-code --quiet", "claude-code"),
            None
        );
        assert_eq!(ours("~/statusline.sh", "claude-code"), None);
        assert_eq!(
            ours(&format!("'{H}' --quota claude-code | "), "claude-code"),
            None
        );
    }

    #[test]
    fn no_file_and_no_statusline_get_the_quiet_entry_and_lose_it_again() {
        let text = on(None, Target::Claude);
        assert_eq!(state_of(Some(&text), Target::Claude), State::On);
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value[KEY]["command"], command_for(H, "claude-code", None));
        let original = "{\n    \"model\": \"opus\",\n    \"hooks\": {}\n}\n";
        let text = on(Some(original), Target::Claude);
        assert!(text
            .starts_with("{\n    \"model\": \"opus\",\n    \"hooks\": {},\n    \"statusLine\": {"));
        let off = edited(Some(&text), Target::Claude, None).unwrap().unwrap();
        assert_eq!(off, original, "switching off restores the owner's bytes");
        assert_eq!(edited(Some(original), Target::Claude, None).unwrap(), None);
    }

    #[test]
    fn agy_keeps_its_built_in_line() {
        let text = on(Some("{\"trustedWorkspaces\": []}"), Target::Agy);
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value[KEY]["stack_with_default"], true);
        assert_eq!(value["trustedWorkspaces"], serde_json::json!([]));
    }

    #[test]
    fn the_users_command_is_wrapped_and_put_back_byte_for_byte() {
        let original = "{\n  \"statusLine\": {\n    \"type\": \"command\",\n    \"command\": \"~/.claude/line.sh \\\"$x\\\"\",\n    \"padding\": 1\n  }\n}\n";
        assert_eq!(state_of(Some(original), Target::Claude), State::User);
        let text = on(Some(original), Target::Claude);
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            value[KEY]["command"],
            format!("'{H}' --quota claude-code | ~/.claude/line.sh \"$x\"")
        );
        assert_eq!(value[KEY]["padding"], 1);
        assert_eq!(
            edited(Some(&text), Target::Claude, Some(H)).unwrap(),
            None,
            "already on"
        );
        let off = edited(Some(&text), Target::Claude, None).unwrap().unwrap();
        assert_eq!(off, original);
    }

    #[test]
    fn a_moved_bundle_rewrites_only_the_prefix() {
        let wrapped = on(
            Some("{\"statusLine\":{\"type\":\"command\",\"command\":\"line\"}}"),
            Target::Claude,
        );
        let moved = edited(Some(&wrapped), Target::Claude, Some(H2))
            .unwrap()
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&moved).unwrap();
        assert_eq!(
            value[KEY]["command"],
            format!("'{H2}' --quota claude-code | line")
        );
    }

    #[test]
    fn what_deck_cannot_follow_is_refused() {
        assert_eq!(
            edited(Some("not json"), Target::Claude, Some(H)),
            Err("not-json")
        );
        assert_eq!(
            edited(Some("[]"), Target::Claude, Some(H)),
            Err("not-an-object")
        );
        assert_eq!(
            edited(Some("{\"statusLine\": true}"), Target::Claude, Some(H)),
            Err("not-a-command")
        );
        assert_eq!(
            edited(
                Some("{\"statusLine\": {\"type\": \"other\", \"command\": \"x\"}}"),
                Target::Claude,
                Some(H)
            ),
            Err("not-a-command")
        );
        assert_eq!(
            state_of(Some("{\"statusLine\": 3}"), Target::Claude),
            State::Unreadable
        );
    }

    #[test]
    fn a_file_on_disk_round_trips_through_the_writer() {
        let dir = crate::test_dir::TestDir::new("quota-install");
        let home: &Path = &dir;
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        assert!(
            !edit_target(home, Target::Agy, Some(H)).unwrap(),
            "agy never ran: skipped"
        );
        assert!(edit_target(home, Target::Claude, Some(H)).unwrap());
        let text = std::fs::read_to_string(Target::Claude.path(home)).unwrap();
        assert_eq!(state_of(Some(&text), Target::Claude), State::On);
        assert!(edit_target(home, Target::Claude, None).unwrap());
        let text = std::fs::read_to_string(Target::Claude.path(home)).unwrap();
        assert_eq!(state_of(Some(&text), Target::Claude), State::Off);
    }
}
