//! Settings summary (`deck_settings_read`): a client the user lets draft
//! configuration (`allow_drafts`) may read a content-free summary of Deck's
//! settings, so its Agent can tell the user why something does not work and
//! where to change it. It writes nothing and grants nothing.
//!
//! # Contract
//! - Read only, under the draft choice (`Choice::Drafts`); no new choice, no
//!   ledger write, no state version. At most `READS_PER_MINUTE` per client.
//! - Content-free by construction: a fact is a boolean, a count, a closed
//!   word Deck itself defines (a status word, an enum the webview
//!   normalizes to: a settings word is passed only when it has the shape of
//!   one, `word`), a version or a time. Never a template step, a rule's or
//!   channel's text, a workspace, device or editor name, a path, a command,
//!   a project environment variable (not even its name: `project_env.rs`
//!   keeps every trace of them out of MCP views) or a secret.
//! - Starts no process, makes no network call and takes no secret out of
//!   the Keychain: Slack and model keys are `keychain::has` only, and the
//!   readers that verify (`slack_connection_status`, `tmux_server_status`,
//!   the translation pack's hash) are never called — the pack is reported
//!   as present on disk, a check as its last recorded result.
//! - Persistence never guesses: an unreadable settings file is
//!   `SETTINGS_UNAVAILABLE`, never defaults; a backup answers with
//!   `source: "backup"`; no file yet answers `source: "none"` (Deck's
//!   defaults are then the truth). No current Board: `boardUnavailable`
//!   and no projects.
//! - Each item carries its stable settings id and, in the UI language, the
//!   section and setting titles (`ui/test/fixtures/settings-items.json`,
//!   held equal to `settings-search-model.js` by `settings-items.test.mjs`);
//!   typing the id into Deck's Settings search shows exactly that setting.
//!   Each also carries a fixed note on what it depends on; Deck draws no
//!   conclusion, the Agent does.

use super::*;

pub(super) const READS_PER_MINUTE: usize = 30;

const ITEMS_FIXTURE: &str = include_str!("../../../ui/test/fixtures/settings-items.json");

/// What the summary reads outside settings.json and the Board.
#[derive(Clone, Debug, Default)]
pub(super) struct Live {
    /// macOS notification permission (`notify::STATUS_WORDS`).
    pub(super) os_notifications: String,
    /// `agent_hooks_status().agents`: per Agent source, closed booleans and
    /// Pi's installation word.
    pub(super) agents: Value,
    /// `turn_summary_status()`: `{hooks, on}`.
    pub(super) summaries: Value,
    /// `model_api_status()`: per provider `saved` and the last check.
    pub(super) model_api: Value,
    /// Slack user, bot and App token: present in the Keychain.
    pub(super) slack_tokens: [bool; 3],
    pub(super) slack_connected: bool,
    /// Phone Connector: enabled, running, trusted devices; None unreadable.
    pub(super) connector: Option<(bool, bool, usize)>,
    pub(super) pack_on_disk: bool,
    pub(super) update_failure: Option<&'static str>,
}

/// `documents::settings_for_summary`'s answer.
pub(super) type SettingsAnswer = Result<(Option<Value>, &'static str), ()>;

#[cfg(test)]
thread_local! {
    /// A test's settings answer and live facts in place of the real readers.
    pub(super) static SOURCES_FOR_TESTS: std::cell::RefCell<Option<(SettingsAnswer, Live)>> =
        const { std::cell::RefCell::new(None) };
}

fn settings_answer() -> SettingsAnswer {
    #[cfg(test)]
    if let Some((answer, _)) = SOURCES_FOR_TESTS.with(|cell| cell.borrow().clone()) {
        return answer;
    }
    crate::documents::settings_for_summary()
}

fn live() -> Live {
    #[cfg(test)]
    if let Some((_, live)) = SOURCES_FOR_TESTS.with(|cell| cell.borrow().clone()) {
        return live;
    }
    use crate::keychain::{has, Slot};
    let connector = crate::connector::connector_status()
        .ok()
        .and_then(|status| serde_json::to_value(status).ok())
        .map(|status| {
            let trusted = status["devices"]
                .as_array()
                .map_or(0, |d| d.iter().filter(|x| x["trusted"] == true).count());
            (
                status["enabled"] == true,
                status["running"] == true,
                trusted,
            )
        });
    Live {
        os_notifications: crate::notify::notify_status(),
        agents: serde_json::to_value(crate::agent_status::agent_hooks_status())
            .map(|v| v["agents"].clone())
            .unwrap_or(Value::Null),
        summaries: serde_json::to_value(crate::agent_status::turn_summary_status())
            .unwrap_or(Value::Null),
        model_api: crate::model_api::model_api_status(),
        slack_tokens: [
            has(Slot::SlackUserToken),
            has(Slot::SlackBotToken),
            has(Slot::SlackAppToken),
        ],
        slack_connected: crate::slack_transport::connected(),
        connector,
        pack_on_disk: crate::intelligence::pack::on_disk(),
        update_failure: crate::updater::last_check_failure(),
    }
}

fn within_rate(client: &str) -> bool {
    static WINDOW: OnceLock<Mutex<HashMap<String, Vec<Instant>>>> = OnceLock::new();
    let mut window = WINDOW.get_or_init(Default::default).lock_or_recover();
    let recent = window.entry(client.to_string()).or_default();
    recent.retain(|at| at.elapsed() < Duration::from_secs(60));
    if recent.len() >= READS_PER_MINUTE {
        return false;
    }
    recent.push(Instant::now());
    true
}

/// The fixed notes: what each setting depends on. Facts, never a verdict.
fn note(id: &str) -> &'static str {
    match id {
        "language" => "The UI language; system follows macOS.",
        "appearance" => "Theme, accent and text size of Deck's window.",
        "voice" => "Voice input languages; dictation also needs macOS speech recognition and microphone permission.",
        "local-translation" => "The Translation Lens works only while enabled AND the language pack is downloaded (packOnDisk; Deck verifies it when it loads).",
        "editor" => "Which editor opens file links from a terminal; unchosen means macOS's default app.",
        "session-restore" => "When on, Deck restores each card's shell after the shell service restarts.",
        "agent-status" => "A card shows its Agent's state (working, needs input, turn ended) only when that Agent's signal hooks are installed (signal: true) in the Agent's own settings. Codex Signal is also unavailable for cards started without --no-daemon (Deck cannot attribute the shared daemon's events to a pane); that is a known limit, not a misconfiguration. turnSummaries.on needs Claude Code's hooks; long final messages are condensed with a model API key.",
        "claude-reentry" => "When observed (lifecycle: true on claude-code), Deck can resume Claude sessions after the shell service restarts or is lost.",
        "codex-reentry" => "When observed (lifecycle: true on codex), Deck can resume Codex sessions after the shell service restarts.",
        "away-notifications" => "Notifies only while Deck's window is not in front, and only when macOS allows Deck to notify (osPermission authorized).",
        "slack-reactions" => "Badge rules need badgesEnabled, the user token and a live connection; channel rules need channelEnabled, the bot and App tokens. Token validity is not checked here.",
        "model-api" => "Used by Draft title and turn summaries. check is the last recorded verification (none since Deck started: null).",
        "connector" => "The Phone Connector serves paired, trusted devices only while enabled and running.",
        "updates" => "The update channel and this build's version; lastCheckFailure is the last check's error, null when it answered.",
        _ => "",
    }
}

fn paths() -> &'static HashMap<String, (String, Value)> {
    static PATHS: OnceLock<HashMap<String, (String, Value)>> = OnceLock::new();
    PATHS.get_or_init(|| {
        let fixture: Value = serde_json::from_str(ITEMS_FIXTURE).unwrap_or(Value::Null);
        fixture["items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| {
                Some((
                    item["id"].as_str()?.to_string(),
                    (item["section"].as_str()?.to_string(), item["path"].clone()),
                ))
            })
            .collect()
    })
}

/// A settings word the webview normalizes to a closed list, passed only
/// when it has the shape of one; anything else is `fallback`.
fn word(settings: Option<&Value>, pointer: &str, fallback: &'static str) -> Value {
    settings
        .and_then(|s| s.pointer(pointer))
        .and_then(Value::as_str)
        .filter(|w| {
            !w.is_empty()
                && w.len() <= 32
                && w.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
        .map_or(json!(fallback), |w| json!(w))
}

fn flag(settings: Option<&Value>, pointer: &str) -> bool {
    settings.and_then(|s| s.pointer(pointer)) == Some(&Value::Bool(true))
}

fn count(settings: Option<&Value>, pointer: &str) -> usize {
    settings
        .and_then(|s| s.pointer(pointer))
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

fn present(yes: bool) -> &'static str {
    if yes {
        "present"
    } else {
        "absent"
    }
}

/// The items in Settings order, each with its facts.
pub(super) fn items(settings: Option<&Value>, live: &Live, locale: &str) -> Vec<Value> {
    let s = settings;
    let lifecycle = |source: &str| live.agents[source]["lifecycle"].clone();
    let font_scale = s
        .and_then(|s| s.get("fontScale"))
        .and_then(Value::as_f64)
        .filter(|f| (0.5..=3.0).contains(f))
        .unwrap_or(1.0);
    let voice_languages: Vec<Value> = (0..count(s, "/voice/languages").min(16))
        .map(|at| word(s, &format!("/voice/languages/{at}"), ""))
        .filter(|language| language != "")
        .collect();
    let facts: Vec<(&str, Value)> = vec![
        ("language", json!({"locale": word(s, "/locale", "system")})),
        (
            "appearance",
            json!({
                "theme": word(s, "/theme", "deck-dark"),
                "accent": word(s, "/accent", "teal"),
                "fontScale": font_scale,
            }),
        ),
        (
            "voice",
            json!({
                "defaultLanguage": word(s, "/voice/defaultLanguage", "system"),
                "languages": voice_languages,
            }),
        ),
        (
            "local-translation",
            json!({
                "enabled": flag(s, "/localIntelligence/translation/enabled"),
                "packOnDisk": live.pack_on_disk,
            }),
        ),
        (
            "editor",
            json!({"chosen": s
                .and_then(|s| s.get("editor"))
                .and_then(Value::as_str)
                .is_some_and(|e| !e.trim().is_empty())}),
        ),
        (
            "session-restore",
            json!({"enabled": flag(s, "/sessionRestore")}),
        ),
        (
            "agent-status",
            json!({"agents": live.agents, "turnSummaries": live.summaries}),
        ),
        (
            "claude-reentry",
            json!({"lifecycle": lifecycle("claude-code")}),
        ),
        ("codex-reentry", json!({"lifecycle": lifecycle("codex")})),
        (
            "away-notifications",
            json!({
                "enabled": flag(s, "/notifyAway"),
                "sound": flag(s, "/notifySound"),
                "osPermission": live.os_notifications,
            }),
        ),
        (
            "slack-reactions",
            json!({
                "userToken": present(live.slack_tokens[0]),
                "botToken": present(live.slack_tokens[1]),
                "appToken": present(live.slack_tokens[2]),
                "connected": live.slack_connected,
                "badgesEnabled": flag(s, "/inbound/sources/slack/enabled"),
                "channelEnabled": flag(s, "/inbound/channelConnection/enabled"),
                "channelRules": count(s, "/inbound/channelRules"),
            }),
        ),
        ("model-api", live.model_api.clone()),
        (
            "connector",
            match live.connector {
                Some((enabled, running, trusted)) => {
                    json!({"enabled": enabled, "running": running, "trustedDevices": trusted})
                }
                None => json!({"unavailable": true}),
            },
        ),
        (
            "updates",
            json!({
                "channel": match s.and_then(|s| s.get("updateChannel")).and_then(Value::as_str) {
                    Some("nightly") => "nightly",
                    _ => "stable",
                },
                "version": env!("CARGO_PKG_VERSION"),
                "lastCheckFailure": live.update_failure,
            }),
        ),
    ];
    facts
        .into_iter()
        .map(|(id, facts)| {
            let (section, path) = paths()
                .get(id)
                .map(|(section, path)| {
                    let localized = path
                        .get(locale)
                        .or_else(|| path.get("en"))
                        .cloned()
                        .unwrap_or(Value::Null);
                    (json!(section), localized)
                })
                .unwrap_or((Value::Null, Value::Null));
            json!({
                "id": id,
                "where": {"section": section, "path": path},
                "facts": facts,
                "note": note(id),
            })
        })
        .collect()
}

/// One project's defaults and its rules, counted.
fn project_view(
    project: &Value,
    project_id: &str,
    config: Option<&crate::inbound::Config>,
) -> Value {
    let text_set = |key: &str| {
        if project[key].as_str().is_some_and(|v| !v.trim().is_empty()) {
            "set"
        } else {
            "unset"
        }
    };
    let columns = project["columns"].as_array().cloned().unwrap_or_default();
    let selected = project["selected"].as_str();
    let new_card_column = columns
        .iter()
        .find(|c| selected.is_some() && c["id"].as_str() == selected)
        .or_else(|| columns.iter().find(|c| c["semantic"] == "working"))
        .or_else(|| columns.get(1))
        .or_else(|| columns.first())
        .map(|c| c["id"].clone())
        .unwrap_or(Value::Null);
    let rules = config.map(|config| {
        let of = |source: &str| {
            config
                .rules
                .iter()
                .filter(|r| r.source == source && r.project_id == project_id)
                .collect::<Vec<_>>()
        };
        let slack = of("slack");
        let agent = of(crate::inbound_agent::SOURCE);
        let clock = of(TRIGGER_CLOCK);
        json!({
            "slackBadge": {"total": slack.len(), "approved": slack.iter().filter(|r| r.auto_send.is_some()).count()},
            "agent": {"total": agent.len(), "approved": agent.iter().filter(|r| r.auto_send.is_some()).count()},
            "clock": {"total": clock.len(), "enabled": clock.iter().filter(|r| r.enabled).count()},
        })
    });
    json!({
        "project_id": project_id,
        "name": project["name"],
        "defaultCmd": text_set("cmd"),
        "defaultDir": text_set("dir"),
        "newCardColumn": new_card_column,
        "templates": project["templates"].as_array().map_or(0, Vec::len),
        "taskPresets": project["presets"].as_array().map_or(0, Vec::len),
        "rules": rules,
    })
}

pub(super) fn settings_read(
    runtime: &Runtime,
    client_id: &str,
    arguments: Value,
) -> Result<Value, Value> {
    parse::<Empty>(arguments)?;
    let client = runtime
        .read(|doc| permitted(doc, client_id, Choice::Drafts).cloned())
        .map_err(map_error)?
        .map_err(map_error)?;
    if !within_rate(client_id) {
        return Err(error_value(
            "CAPACITY_EXCEEDED",
            "at most 30 settings reads a minute",
            "Read once and reason over the answer; it changes only when the user changes a setting.",
        ));
    }
    let Ok((settings, source)) = settings_answer() else {
        return Err(error_value(
            "SETTINGS_UNAVAILABLE",
            "Deck cannot read its settings file right now",
            "Tell the user Deck cannot read its own settings (Settings may say why); do not assume defaults.",
        ));
    };
    let locale = match crate::documents::native_locale().as_str() {
        "system" => "en".to_string(),
        other => other.to_string(),
    };
    let live = live();
    let board = crate::documents::board_authority();
    let config = super::automation::automation_config();
    let projects: Vec<Value> = board
        .as_ref()
        .map(|board| {
            client_scopes(&client)
                .iter()
                .filter_map(|scope| {
                    let project = board["projects"]
                        .as_array()?
                        .iter()
                        .find(|p| p["id"].as_str() == Some(scope.project_id.as_str()))?;
                    Some(project_view(project, &scope.project_id, config.as_ref()))
                })
                .collect()
        })
        .unwrap_or_default();
    let mut answer = json!({
        "ok": true,
        "source": source,
        "readAt": now_ms(),
        "locale": locale,
        "items": items(settings.as_ref(), &live, &locale),
        "projects": projects,
        "boardUnavailable": board.is_none(),
        "rulesUnavailable": config.is_none(),
        "howToLocate": "Tell the user the item's where.path, or to type its id into the search field of Deck's Settings: that shows exactly this setting. You cannot change settings; the user does. Your own MCP permissions are in deck_capabilities.",
    });
    if source == "backup" {
        answer["sourceNote"] = json!(
            "Deck is running on its settings backup (the main file could not be read); tell the user. Approvals read from a backup do not count."
        );
    }
    Ok(answer)
}
