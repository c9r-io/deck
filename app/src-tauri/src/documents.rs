//! Typed board/settings documents: `BoardDoc` and `SettingsDoc` validate
//! business structure via `try_from` (the SAME rules on load and save), and
//! the load/save commands plus the settings readers other modules use.
//!
//! # Contract
//! - One rule set, two doors. `BoardDoc` / `SettingsDoc` deserialize through
//!   `try_from`, so `storage::load_typed` (quarantine-first recovery on
//!   failure) and `save_board` / `save_settings` (reject before touching
//!   disk) run identical checks: non-empty unique project, column and card
//!   ids; a card's project and column exist; one tmux session per card with
//!   a name the runtime would accept; settings values from closed sets
//!   (locale, theme, accent, update channel, voice languages) or bounded ranges (font scale,
//!   editor name, shortcut table), with `inbound` handed to
//!   `inbound::validate_settings`. A violation is an `InvalidDoc` error with
//!   its rule's message; the file is never rewritten to make it pass.
//! - The typed structs are parse-only. The webview owns the documents and
//!   `save_*` persists the ORIGINAL string, so unknown extension fields
//!   round-trip untouched and the `#[allow(dead_code)]` fields exist to be
//!   validated, not read; `launched` defaults to true so a board written
//!   before the field never re-runs a command.
//! - `LoadedDoc` carries the text, its source and at most one `UiNotice`: a
//!   closed code (`storage.privacy`, `queue.persist`, `queue.load`,
//!   `history.load`, `queue.interrupted`, `storage.recovered`) the webview
//!   translates; a recovery note never carries a path. `storage_warnings`
//!   drains the boot-time notes once, for the first Board render.
//! - The settings readers (`editor_app`, `locale_setting`,
//!   `update_channel_setting`) load the file through the same typed door on
//!   every call, never a cache, and fall back (None / "system" / "stable")
//!   when the file or field is absent or malformed: a broken settings file
//!   must not take the editor menu, the locale or the updater down with it.
//! - `save_settings` refuses an unknown `updateChannel` before disk
//!   (`validate_saved_update_channel`) so a build can never be pointed at an
//!   endpoint deck does not ship.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::error::{DeckError, ErrorKind};
use crate::storage;
use crate::sync::LockRecover;

// ---------- board persistence ------------------------------------------------

/// Business-structure validation for deck.json — ONE rule set shared by
/// load and save: `BoardDoc` deserializes via `try_from`, so
/// `storage::load_typed::<BoardDoc>` (quarantine/backup recovery on
/// failure) and `save_board` (reject before touching disk) both run the
/// full referential checks below. Unknown extension fields are tolerated
/// (serde ignores them; save persists the original string, so they
/// round-trip untouched).
#[derive(serde::Deserialize)]
pub(crate) struct BoardDocRaw {
    projects: Vec<BoardProject>,
    cards: Vec<BoardCard>,
}

#[derive(serde::Deserialize)]
#[serde(try_from = "BoardDocRaw")]
pub(crate) struct BoardDoc(#[allow(dead_code)] BoardDocRaw);

impl TryFrom<BoardDocRaw> for BoardDoc {
    type Error = DeckError;
    fn try_from(raw: BoardDocRaw) -> Result<Self, DeckError> {
        validate_board(&raw)?;
        Ok(BoardDoc(raw))
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct BoardProject {
    id: String,
    #[allow(dead_code)]
    name: String,
    #[serde(default)]
    columns: Vec<BoardColumn>,
    /// Optional project defaults for the Board's own creation paths (04): a
    /// default directory and a default launch command. Absent means a shell
    /// in $HOME; present values must be strings.
    #[allow(dead_code)]
    #[serde(default)]
    dir: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    cmd: Option<String>,
    #[serde(default)]
    presets: Vec<TaskPreset>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskPreset {
    id: String,
    name: String,
    column_id: String,
    title: String,
    dir: String,
    cmd: String,
    steps: Vec<String>,
}
#[derive(serde::Deserialize)]
pub(crate) struct BoardColumn {
    id: String,
    #[allow(dead_code)]
    name: String,
}
#[derive(serde::Deserialize)]
pub(crate) struct BoardCard {
    id: String,
    #[serde(rename = "projectId")]
    project_id: String,
    #[serde(rename = "columnId")]
    column_id: String,
    #[allow(dead_code)]
    title: String,
    #[allow(dead_code)]
    #[serde(default)]
    pinned: bool,
    /// The launch command was sent once. Absent on boards written before the
    /// field, which must read as launched: an upgrade never re-runs commands.
    #[allow(dead_code)]
    #[serde(default = "launched_default")]
    launched: bool,
    /// runtime fields the UI cannot operate a card without
    #[allow(dead_code)]
    cmd: String,
    #[allow(dead_code)]
    dir: String,
    session: String,
    /// Optional card-local scratchpad. Its text is independent from desc and
    /// queued prompts; old boards have no field and therefore an empty buffer.
    #[serde(default)]
    buffer: Option<CardBuffer>,
    #[serde(default, rename = "channelRun")]
    channel_run: Option<ChannelRun>,
    #[serde(default, rename = "connectorRun")]
    connector_run: Option<ConnectorRun>,
}

const BUFFER_MAX_ENTRIES: usize = 256;
const BUFFER_MAX_COPIES: usize = 256;
const BUFFER_MAX_ENTRY_BYTES: usize = 32 * 1024;
const BUFFER_MAX_BYTES: usize = 1024 * 1024;
const BUFFER_MAX_SERIALIZED_BYTES: usize = 2 * 1024 * 1024;

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct CardBuffer {
    #[serde(default)]
    revision: u64,
    #[serde(default)]
    collecting: bool,
    #[serde(default)]
    entries: Vec<BufferEntry>,
    #[serde(default, flatten)]
    extra: HashMap<String, serde_json::Value>,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct BufferEntry {
    id: String,
    kind: String,
    text: String,
    revision: u64,
    created_at: u64,
    updated_at: u64,
    #[serde(default)]
    source: Option<BufferSource>,
    #[serde(default)]
    copies: Vec<BufferCopy>,
    #[serde(default, flatten)]
    extra: HashMap<String, serde_json::Value>,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct BufferSource {
    #[serde(rename = "type")]
    source_type: String,
    event_id: String,
    #[serde(default)]
    connection: Option<String>,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    rule: Option<String>,
    #[serde(default)]
    at: Option<u64>,
    #[serde(default)]
    links: Vec<String>,
    #[serde(default)]
    workspace_id: Option<String>,
    #[serde(default)]
    message_ts: Option<String>,
    #[serde(default)]
    thread_ts: Option<String>,
    #[serde(default)]
    sender_user_id: Option<String>,
    #[serde(default)]
    sender_bot_id: Option<String>,
    #[serde(default, flatten)]
    extra: HashMap<String, serde_json::Value>,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct BufferCopy {
    operation_id: String,
    entry_revision: u64,
    text: String,
    created_at: u64,
    state: String,
    #[serde(default, flatten)]
    extra: HashMap<String, serde_json::Value>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChannelRun {
    group_key: String,
    first_event_id: String,
    connection_id: String,
    workspace_id: String,
    channel_id: String,
    rule_id: String,
    last_collected_at: u64,
    idle_minutes: u32,
    collecting: bool,
    #[serde(default)]
    initial_steps: Vec<ChannelStep>,
    #[serde(default)]
    initial_queued: bool,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChannelStep {
    operation_id: String,
    text: String,
    mode: String,
    #[serde(default)]
    at: Option<u64>,
    tpl: String,
    tpl_idx: usize,
    tpl_total: usize,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConnectorRun {
    handle: String,
    preset_id: String,
    #[serde(default)]
    initial_steps: Vec<ChannelStep>,
    #[serde(default)]
    initial_queued: bool,
}

fn validate_connector_run(card_id: &str, run: &ConnectorRun) -> Result<(), DeckError> {
    let valid = run.handle.len() == 64
        && run.handle.chars().all(|c| c.is_ascii_hexdigit())
        && bounded_buffer_id(&run.preset_id)
        && run.initial_steps.len() <= 20
        && run.initial_steps.iter().enumerate().all(|(index, step)| {
            bounded_buffer_id(&step.operation_id)
                && !step.text.is_empty()
                && step.text.len() <= 2000
                && matches!(step.mode.as_str(), "at" | "chain")
                && (step.mode == "at") == step.at.is_some()
                && step.tpl == run.preset_id
                && step.tpl_idx == index + 1
                && step.tpl_total == run.initial_steps.len()
        });
    let _ = run.initial_queued;
    if valid {
        Ok(())
    } else {
        Err(DeckError::new(
            ErrorKind::InvalidDoc,
            format!("card {card_id}: invalid connector run"),
        ))
    }
}

fn validate_channel_run(
    card_id: &str,
    run: &ChannelRun,
    buffer: Option<&CardBuffer>,
) -> Result<(), DeckError> {
    let valid = run.group_key.len() <= 1024
        && run.group_key.starts_with("default/")
        && !run.first_event_id.is_empty()
        && run.first_event_id.len() <= 128
        && run.connection_id == "default"
        && run.workspace_id.starts_with('T')
        && (run.channel_id.starts_with('C') || run.channel_id.starts_with('G'))
        && bounded_buffer_id(&run.rule_id)
        && run.last_collected_at > 0
        && run.idle_minutes <= 7 * 24 * 60
        && run.initial_steps.len() <= BUFFER_MAX_COPIES
        && buffer.is_some_and(|value| value.collecting == run.collecting)
        && run.initial_steps.iter().enumerate().all(|(index, step)| {
            bounded_buffer_id(&step.operation_id)
                && !step.text.is_empty()
                && step.text.len() <= BUFFER_MAX_ENTRY_BYTES
                && matches!(step.mode.as_str(), "at" | "chain")
                && (step.mode == "at") == step.at.is_some()
                && !step.tpl.is_empty()
                && step.tpl.len() <= 120
                && step.tpl_idx == index + 1
                && step.tpl_total == run.initial_steps.len()
        });
    let _ = (run.initial_queued, &run.workspace_id);
    if valid {
        Ok(())
    } else {
        Err(DeckError::new(
            ErrorKind::InvalidDoc,
            format!("card {card_id}: invalid channel run"),
        ))
    }
}

fn bounded_buffer_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
}

fn validate_buffer(card_id: &str, buffer: &CardBuffer) -> Result<(), DeckError> {
    if buffer.entries.len() > BUFFER_MAX_ENTRIES {
        return Err(DeckError::new(
            ErrorKind::InvalidDoc,
            format!("card {card_id}: too many buffer entries"),
        ));
    }
    let mut ids = HashSet::new();
    let mut operations = HashSet::new();
    let mut copies = 0usize;
    let mut bytes = 0usize;
    for entry in &buffer.entries {
        if !bounded_buffer_id(&entry.id) || !ids.insert(entry.id.as_str()) {
            return Err(DeckError::new(
                ErrorKind::InvalidDoc,
                format!("card {card_id}: invalid or duplicate buffer entry id"),
            ));
        }
        if !matches!(entry.kind.as_str(), "manual" | "external")
            || entry.revision == 0
            || entry.created_at == 0
            || entry.updated_at < entry.created_at
            || entry.text.len() > BUFFER_MAX_ENTRY_BYTES
        {
            return Err(DeckError::new(
                ErrorKind::InvalidDoc,
                format!("card {card_id}: invalid buffer entry"),
            ));
        }
        match (&entry.kind[..], &entry.source) {
            ("manual", None) => {}
            ("external", Some(source))
                if !source.event_id.is_empty()
                    && source.event_id.len() <= 256
                    && !source.event_id.chars().any(char::is_control)
                    && !source.source_type.is_empty()
                    && source.source_type.len() <= 64
                    && source.connection.as_ref().is_none_or(|v| v.len() <= 128)
                    && source.channel.as_ref().is_none_or(|v| v.len() <= 128)
                    && source.rule.as_ref().is_none_or(|v| v.len() <= 128)
                    && source.at.is_none_or(|at| at > 0)
                    && source.links.len() <= 16
                    && source.links.iter().all(|v| {
                        v.len() <= 2048 && (v.starts_with("https://") || v.starts_with("http://"))
                    })
                    && source
                        .workspace_id
                        .as_ref()
                        .is_none_or(|v| v.starts_with('T') && v.len() <= 64)
                    && source.message_ts.as_ref().is_none_or(|v| v.len() <= 32)
                    && source.thread_ts.as_ref().is_none_or(|v| v.len() <= 32)
                    && source.sender_user_id.as_ref().is_none_or(|v| v.len() <= 64)
                    && source.sender_bot_id.as_ref().is_none_or(|v| v.len() <= 64) => {}
            _ => {
                return Err(DeckError::new(
                    ErrorKind::InvalidDoc,
                    format!("card {card_id}: invalid buffer source"),
                ))
            }
        }
        bytes = bytes.saturating_add(entry.text.len());
        copies += entry.copies.len();
        for copy in &entry.copies {
            if !bounded_buffer_id(&copy.operation_id)
                || !operations.insert(copy.operation_id.as_str())
                || copy.entry_revision == 0
                || copy.created_at == 0
                || copy.text.len() > BUFFER_MAX_ENTRY_BYTES
                || !matches!(
                    copy.state.as_str(),
                    "queued" | "delivered" | "canceled" | "uncertain"
                )
            {
                return Err(DeckError::new(
                    ErrorKind::InvalidDoc,
                    format!("card {card_id}: invalid buffer queue copy"),
                ));
            }
            bytes = bytes.saturating_add(copy.text.len());
        }
    }
    let serialized = serde_json::to_vec(buffer)
        .map(|value| value.len())
        .unwrap_or(usize::MAX);
    if copies > BUFFER_MAX_COPIES
        || bytes > BUFFER_MAX_BYTES
        || serialized > BUFFER_MAX_SERIALIZED_BYTES
    {
        return Err(DeckError::new(
            ErrorKind::InvalidDoc,
            format!("card {card_id}: buffer capacity exceeded"),
        ));
    }
    let _ = (buffer.revision, buffer.collecting);
    Ok(())
}

fn launched_default() -> bool {
    true
}

/// The referential rules a usable board must satisfy. Errors carry ids
/// (deck-generated), never titles/commands/paths — they end up in recovery
/// warnings.
fn validate_board(b: &BoardDocRaw) -> Result<(), DeckError> {
    let mut project_ids = HashSet::new();
    for p in &b.projects {
        if p.id.trim().is_empty() {
            return Err(DeckError::new(
                ErrorKind::InvalidDoc,
                "a project has an empty id",
            ));
        }
        if !project_ids.insert(p.id.as_str()) {
            return Err(DeckError::new(
                ErrorKind::InvalidDoc,
                format!("duplicate project id {}", p.id),
            ));
        }
        if p.columns.is_empty() {
            return Err(DeckError::new(
                ErrorKind::InvalidDoc,
                format!("project {} has no columns", p.id),
            ));
        }
        let mut col_ids = HashSet::new();
        for c in &p.columns {
            if c.id.trim().is_empty() {
                return Err(DeckError::new(
                    ErrorKind::InvalidDoc,
                    format!("project {} has a column with an empty id", p.id),
                ));
            }
            if !col_ids.insert(c.id.as_str()) {
                return Err(DeckError::new(
                    ErrorKind::InvalidDoc,
                    format!("duplicate column id {} in project {}", c.id, p.id),
                ));
            }
        }
        if p.presets.len() > 50 {
            return Err(DeckError::new(
                ErrorKind::InvalidDoc,
                format!("project {} has too many task presets", p.id),
            ));
        }
        let mut preset_ids = HashSet::new();
        for preset in &p.presets {
            let supported = crate::inbound_channel::channel_agent_command(&preset.cmd).is_some();
            if !bounded_buffer_id(&preset.id)
                || !preset_ids.insert(preset.id.as_str())
                || preset.name.is_empty()
                || preset.name.len() > 120
                || preset.title.is_empty()
                || preset.title.len() > 120
                || preset.dir.is_empty()
                || preset.dir.len() > 1024
                || preset.dir.chars().any(char::is_control)
                || preset.cmd.is_empty()
                || preset.cmd.len() > 200
                || !supported
                || !col_ids.contains(preset.column_id.as_str())
                || preset.steps.len() > 20
                || preset
                    .steps
                    .iter()
                    .any(|step| step.is_empty() || step.len() > 2000)
            {
                return Err(DeckError::new(
                    ErrorKind::InvalidDoc,
                    format!("project {} has an invalid task preset", p.id),
                ));
            }
        }
    }
    let mut card_ids = HashSet::new();
    let mut sessions = HashSet::new();
    for c in &b.cards {
        if c.id.trim().is_empty() {
            return Err(DeckError::new(
                ErrorKind::InvalidDoc,
                "a card has an empty id",
            ));
        }
        if !card_ids.insert(c.id.as_str()) {
            return Err(DeckError::new(
                ErrorKind::InvalidDoc,
                format!("duplicate card id {}", c.id),
            ));
        }
        // the SAME session-name rule the runtime enforces on start/attach
        crate::tmux::validate_session_name(&c.session)
            .map_err(|e| DeckError::classified(format!("card {}: {e}", c.id)))?;
        if !sessions.insert(c.session.as_str()) {
            return Err(DeckError::new(
                ErrorKind::InvalidDoc,
                format!("card {}: session name is already used", c.id),
            ));
        }
        if let Some(buffer) = &c.buffer {
            validate_buffer(&c.id, buffer)?;
        }
        if let Some(run) = &c.channel_run {
            validate_channel_run(&c.id, run, c.buffer.as_ref())?;
        }
        if let Some(run) = &c.connector_run {
            validate_connector_run(&c.id, run)?;
        }
        let Some(project) = b.projects.iter().find(|p| p.id == c.project_id) else {
            return Err(DeckError::new(
                ErrorKind::InvalidDoc,
                format!("card {} references a missing project", c.id),
            ));
        };
        if !project.columns.iter().any(|col| col.id == c.column_id) {
            return Err(DeckError::new(
                ErrorKind::InvalidDoc,
                format!(
                    "card {} references a column that is not in its project",
                    c.id
                ),
            ));
        }
    }
    Ok(())
}

/// Settings must be a JSON object; individual keys are optional but must
/// have the right type when present. Same try_from sharing as BoardDoc.
fn deserialize_present_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    String::deserialize(deserializer).map(Some)
}

// Voice preferences are optional for old settings, but a present value must
// select at least one supported language and a default from that set/system.
#[derive(serde::Deserialize)]
struct VoicePreferencesDoc {
    languages: Vec<String>,
    #[serde(rename = "defaultLanguage")]
    default_language: String,
}
fn deserialize_voice_preferences<'de, D>(
    deserializer: D,
) -> Result<Option<VoicePreferencesDoc>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    VoicePreferencesDoc::deserialize(deserializer).map(Some)
}

#[derive(serde::Deserialize)]
pub(crate) struct SettingsDocRaw {
    #[serde(default, deserialize_with = "deserialize_voice_preferences")]
    voice: Option<VoicePreferencesDoc>,
    #[serde(default)]
    editor: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    // Legacy compatibility only. The frontend removes this retired user
    // setting; verbose diagnostics now use the --debug-logging launch flag.
    debug: Option<bool>,
    #[serde(default)]
    #[serde(rename = "sessionRestore")]
    #[allow(dead_code)]
    session_restore: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_present_string")]
    locale: Option<String>,
    #[serde(default, deserialize_with = "deserialize_present_string")]
    theme: Option<String>,
    #[serde(default, deserialize_with = "deserialize_present_string")]
    accent: Option<String>,
    #[serde(default)]
    #[serde(rename = "fontScale")]
    font_scale: Option<f64>,
    #[serde(default)]
    shortcuts: Option<HashMap<String, String>>,
    // Deliberately accept any JSON value on load: older/corrupt/unknown values
    // migrate to Stable in the frontend rather than making all settings
    // unreadable. Every deck-authored save serializes the closed enum.
    #[serde(default)]
    #[serde(rename = "updateChannel")]
    #[allow(dead_code)]
    update_channel: Option<serde_json::Value>,
    // Validated structurally by the inbound module (closed source names,
    // bounded rule fields, one rule per badge); referential checks against
    // the Board are the webview's.
    #[serde(default)]
    inbound: Option<serde_json::Value>,
}

#[derive(serde::Deserialize)]
#[serde(try_from = "SettingsDocRaw")]
pub(crate) struct SettingsDoc(#[allow(dead_code)] SettingsDocRaw);

impl TryFrom<SettingsDocRaw> for SettingsDoc {
    type Error = DeckError;
    fn try_from(raw: SettingsDocRaw) -> Result<Self, DeckError> {
        if let Some(e) = &raw.editor {
            if e.len() > 200 {
                return Err(DeckError::new(
                    ErrorKind::InvalidDoc,
                    "editor name is unreasonably long",
                ));
            }
        }
        if let Some(locale) = &raw.locale {
            if !matches!(locale.as_str(), "system" | "en" | "zh-Hans") {
                return Err(DeckError::new(
                    ErrorKind::InvalidDoc,
                    "locale must be system, en, or zh-Hans",
                ));
            }
        }
        if let Some(theme) = &raw.theme {
            if !matches!(
                theme.as_str(),
                "deck-dark" | "light" | "system" | "high-contrast"
            ) {
                return Err(DeckError::new(
                    ErrorKind::InvalidDoc,
                    "theme must be deck-dark, light, system, or high-contrast",
                ));
            }
        }
        if let Some(accent) = &raw.accent {
            if !matches!(accent.as_str(), "teal" | "blue" | "purple" | "orange") {
                return Err(DeckError::new(
                    ErrorKind::InvalidDoc,
                    "accent must be teal, blue, purple, or orange",
                ));
            }
        }
        if let Some(scale) = raw.font_scale {
            if !scale.is_finite() || !(0.5..=1.6).contains(&scale) {
                return Err(DeckError::new(
                    ErrorKind::InvalidDoc,
                    "fontScale must be between 0.5 and 1.6",
                ));
            }
        }
        if let Some(shortcuts) = &raw.shortcuts {
            if shortcuts.len() > 64 {
                return Err(DeckError::new(
                    ErrorKind::InvalidDoc,
                    "too many shortcut entries",
                ));
            }
            if shortcuts
                .iter()
                .any(|(key, value)| key.is_empty() || key.len() > 64 || value.len() > 64)
            {
                return Err(DeckError::new(
                    ErrorKind::InvalidDoc,
                    "shortcut names and bindings must be bounded strings",
                ));
            }
        }
        if let Some(voice) = &raw.voice {
            let unique: HashSet<_> = voice.languages.iter().collect();
            if voice.languages.is_empty()
                || unique.len() != voice.languages.len()
                || voice
                    .languages
                    .iter()
                    .any(|language| !crate::voice::SUPPORTED_LANGUAGES.contains(&language.as_str()))
                || (voice.default_language != "system"
                    && !voice.languages.contains(&voice.default_language))
            {
                return Err(DeckError::new(
                    ErrorKind::InvalidDoc,
                    "voice languages must be supported, unique, non-empty, and include the default",
                ));
            }
        }
        if let Some(inbound) = &raw.inbound {
            crate::inbound::validate_settings(inbound)?;
        }
        Ok(SettingsDoc(raw))
    }
}

/// What a typed load hands the frontend: the payload, where it came from
/// ("main" | "backup" | "none" for a first run), and — when recovery
/// happened — a warning the UI must show. A rejected promise here is a HARD
/// error (nothing loadable): the UI must surface it, never treat it as a
/// first run.
#[derive(Serialize)]
pub(crate) struct LoadedDoc {
    data: String,
    source: String,
    warning: Option<UiNotice>,
}

#[derive(Serialize)]
pub(crate) struct UiNotice {
    code: &'static str,
}

fn notice_from(note: &str) -> UiNotice {
    let code = if note.contains("privacy hardening") {
        "storage.privacy"
    } else if note.contains("scheduled prompts could not be saved") {
        "queue.persist"
    } else if note.contains("scheduled prompts could not be loaded") {
        "queue.load"
    } else if note.contains("command history could not be loaded") {
        "history.load"
    } else if note.contains("interrupted deliveries") || note.contains("delivery") {
        "queue.interrupted"
    } else {
        "storage.recovered"
    };
    UiNotice { code }
}

fn to_loaded(o: Option<storage::LoadOutcome>) -> LoadedDoc {
    match o {
        Some(o) => LoadedDoc {
            data: o.payload,
            source: o.source.into(),
            warning: o.warning.as_deref().map(notice_from),
        },
        None => LoadedDoc {
            data: String::new(),
            source: "none".into(),
            warning: None,
        },
    }
}

pub(crate) fn board_path() -> PathBuf {
    crate::datadir::deck_dir().join("deck.json")
}

#[tauri::command]
pub(crate) fn load_board() -> Result<LoadedDoc, DeckError> {
    Ok(to_loaded(storage::load_typed::<BoardDoc>(&board_path())?))
}

/// Connector read seam: the returned bytes are the committed, fully typed
/// Board payload selected by normal recovery. Callers project closed DTOs;
/// they never receive a mutable document handle.
pub(crate) fn connector_board_payload() -> Result<String, DeckError> {
    storage::load_typed::<BoardDoc>(&board_path())?
        .map(|loaded| loaded.payload)
        .ok_or_else(|| DeckError::new(ErrorKind::Missing, "board is not initialized"))
}

/// Check a project against the currently committed, fully validated Board.
/// This is used immediately before creating a new authorization; it does not
/// mutate existing authorizations when a project is later removed.
pub(crate) fn board_project_exists(project_id: &str) -> Result<bool, DeckError> {
    let payload = connector_board_payload()?;
    let board = serde_json::from_str::<BoardDoc>(&payload)
        .map_err(|error| DeckError::classified(format!("invalid committed board: {error}")))?;
    Ok(board
        .0
        .projects
        .iter()
        .any(|project| project.id == project_id))
}

/// The same full business validation as load, BEFORE anything touches disk:
/// an invalid document never overwrites the main file or rotates the .bak.
pub(crate) fn save_validated<T: serde::de::DeserializeOwned>(
    path: &std::path::Path,
    data: &str,
    what: &str,
) -> Result<(), DeckError> {
    serde_json::from_str::<T>(data)
        .map_err(|e| DeckError::classified(format!("refusing to save invalid {what}: {e}")))?;
    storage::save_typed::<T>(path, data)
}

#[tauri::command]
pub(crate) fn save_board(data: String) -> Result<(), DeckError> {
    if crate::smoke_faults::take("board-save") {
        return Err(DeckError::new(
            ErrorKind::Other,
            "injected board save failure",
        ));
    }
    save_validated::<BoardDoc>(&board_path(), &data, "board")
}

/// Boot-time storage notices (corruption recovered from .bak, etc.) for the
/// frontend to surface as toasts.
#[tauri::command]
pub(crate) fn storage_warnings() -> Vec<UiNotice> {
    std::mem::take(&mut *storage::WARNINGS.lock_or_recover())
        .iter()
        .map(|note| notice_from(note))
        .collect()
}

// ---------- settings ------------------------------------------------------------

pub(crate) fn settings_path() -> PathBuf {
    crate::datadir::deck_dir().join("settings.json")
}

#[tauri::command]
pub(crate) fn load_settings() -> Result<LoadedDoc, DeckError> {
    Ok(to_loaded(storage::load_typed::<SettingsDoc>(
        &settings_path(),
    )?))
}

#[tauri::command]
pub(crate) fn save_settings(data: String) -> Result<(), DeckError> {
    if crate::smoke_faults::take("settings-save") {
        return Err(DeckError::new(
            ErrorKind::Other,
            "injected settings save failure",
        ));
    }
    validate_saved_update_channel(&data)?;
    save_validated::<SettingsDoc>(&settings_path(), &data, "settings")
}

fn validate_saved_update_channel(data: &str) -> Result<(), DeckError> {
    let value: serde_json::Value = serde_json::from_str(data)
        .map_err(|_| DeckError::new(ErrorKind::InvalidDoc, "settings must be valid JSON"))?;
    match value.get("updateChannel") {
        None => Ok(()),
        Some(serde_json::Value::String(channel))
            if matches!(channel.as_str(), "stable" | "nightly") =>
        {
            Ok(())
        }
        Some(_) => Err(DeckError::new(
            ErrorKind::InvalidDoc,
            "updateChannel must be stable or nightly",
        )),
    }
}

/// The settings document as loose JSON, or None when it is absent or
/// unreadable. Every reader below tolerates a missing/foreign value: settings
/// are advisory, and a bad file must never stop the app from booting.
fn settings_value() -> Option<serde_json::Value> {
    let raw = storage::load_typed::<SettingsDoc>(&settings_path())
        .ok()??
        .payload;
    serde_json::from_str(&raw).ok()
}

fn editor_from(settings: Option<&serde_json::Value>) -> Option<String> {
    let e = settings?.get("editor")?.as_str()?.trim().to_string();
    if e.is_empty() {
        None
    } else {
        Some(e)
    }
}

fn locale_from(settings: Option<&serde_json::Value>) -> String {
    settings
        .and_then(|v| v.get("locale")?.as_str().map(str::to_owned))
        .filter(|v| matches!(v.as_str(), "system" | "en" | "zh-Hans"))
        .unwrap_or_else(|| "system".into())
}

fn update_channel_from(settings: Option<&serde_json::Value>) -> String {
    settings
        .and_then(|v| v.get("updateChannel")?.as_str().map(str::to_owned))
        .filter(|v| matches!(v.as_str(), "stable" | "nightly"))
        .unwrap_or_else(|| "stable".into())
}

pub(crate) fn editor_app() -> Option<String> {
    editor_from(settings_value().as_ref())
}

pub(crate) fn locale_setting() -> String {
    locale_from(settings_value().as_ref())
}

pub(crate) fn update_channel_setting() -> String {
    update_channel_from(settings_value().as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- settings readers: closed values, advisory file ----------

    /// Each reader accepts only its closed alphabet and falls back to the
    /// default for a missing file, a missing key, a foreign type or an
    /// unknown value — a hand-edited settings.json can never select an
    /// endpoint, locale or editor deck does not know.
    #[test]
    fn settings_readers_accept_only_closed_values_and_default_otherwise() {
        let v = |json: &str| serde_json::from_str::<serde_json::Value>(json).unwrap();
        assert_eq!(update_channel_from(None), "stable", "no settings file");
        assert_eq!(update_channel_from(Some(&v("{}"))), "stable");
        assert_eq!(
            update_channel_from(Some(&v(r#"{"updateChannel":"nightly"}"#))),
            "nightly"
        );
        assert_eq!(
            update_channel_from(Some(&v(r#"{"updateChannel":"https://evil"}"#))),
            "stable",
            "an unknown channel can never reach the updater"
        );
        assert_eq!(
            update_channel_from(Some(&v(r#"{"updateChannel":1}"#))),
            "stable"
        );

        assert_eq!(locale_from(None), "system");
        assert_eq!(locale_from(Some(&v(r#"{"locale":"zh-Hans"}"#))), "zh-Hans");
        assert_eq!(locale_from(Some(&v(r#"{"locale":"en"}"#))), "en");
        assert_eq!(locale_from(Some(&v(r#"{"locale":"fr"}"#))), "system");

        assert_eq!(editor_from(None), None);
        assert_eq!(
            editor_from(Some(&v(r#"{"editor":"  Zed "}"#))),
            Some("Zed".into())
        );
        assert_eq!(
            editor_from(Some(&v(r#"{"editor":"   "}"#))),
            None,
            "blank is unset"
        );
        assert_eq!(editor_from(Some(&v(r#"{"editor":3}"#))), None);
    }

    #[test]
    fn saving_settings_refuses_an_unknown_update_channel_before_disk() {
        assert!(validate_saved_update_channel(r#"{"editor":"Zed"}"#).is_ok());
        assert!(validate_saved_update_channel(r#"{"updateChannel":"stable"}"#).is_ok());
        assert!(validate_saved_update_channel(r#"{"updateChannel":"nightly"}"#).is_ok());
        let e = validate_saved_update_channel(r#"{"updateChannel":"beta"}"#).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::InvalidDoc);
        assert_eq!(
            validate_saved_update_channel("not json")
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidDoc
        );
    }

    // ---------- board / settings business validation ----------

    /// A minimal valid board matching what persistence.js actually writes.
    fn board(cards: &str) -> String {
        format!(
            r#"{{"projects":[{{"id":"P1","name":"main","columns":[
                 {{"id":"C1","name":"Attention"}},{{"id":"C2","name":"Working"}}]}},
                 {{"id":"P2","name":"side","columns":[{{"id":"C9","name":"Only"}}]}}],
               "cards":[{cards}]}}"#
        )
    }
    fn card(id: &str, project: &str, column: &str, session: &str) -> String {
        format!(
            r#"{{"id":"{id}","projectId":"{project}","columnId":"{column}",
                 "title":"t","desc":"","cmd":"claude","dir":"~/w","session":"{session}"}}"#
        )
    }

    /// The one Board document both sides pin. `dom.test.mjs` proves the
    /// fixture is exactly what `persistence.js` writes; this test proves
    /// what `BoardCard` requires of it and names every key it merely
    /// tolerates. A new persisted key changes the fixture (the frontend test
    /// forces that) and then fails here until it is either declared in
    /// `BoardCard` or added to the tolerated list on purpose.
    #[test]
    fn board_fixture_pins_the_schema_on_both_sides() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../ui/test/fixtures/board.json");
        let raw = std::fs::read_to_string(path).expect("shared Board fixture");
        assert!(
            serde_json::from_str::<BoardDoc>(&raw).is_ok(),
            "the frontend's shape loads"
        );
        let doc: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let card = doc["cards"][0].as_object().unwrap();
        let mut required = Vec::new();
        let mut tolerated = Vec::new();
        for key in card.keys() {
            let mut without = doc.clone();
            without["cards"][0].as_object_mut().unwrap().remove(key);
            if serde_json::from_value::<BoardDoc>(without).is_ok() {
                tolerated.push(key.as_str());
            } else {
                required.push(key.as_str());
            }
        }
        assert_eq!(
            required,
            [
                "cmd",
                "columnId",
                "dir",
                "id",
                "projectId",
                "session",
                "title"
            ],
            "every key BoardCard requires is one persistence.js always writes"
        );
        assert_eq!(
            tolerated,
            ["desc", "launched", "origin", "pinned"],
            "pinned defaults to false, launched to true; desc and origin are the frontend's alone"
        );
        for (key, wrong) in [
            ("title", serde_json::json!(1)),
            ("pinned", serde_json::json!("yes")),
            ("launched", serde_json::json!("yes")),
        ] {
            let mut typed = doc.clone();
            typed["cards"][0][key] = wrong;
            assert!(
                serde_json::from_value::<BoardDoc>(typed).is_err(),
                "{key} is typed"
            );
        }
    }

    #[test]
    fn board_validation_accepts_real_shape_and_unknown_extensions() {
        let ok = board(&card("s1", "P1", "C1", "deck-t-ab12"));
        assert!(serde_json::from_str::<BoardDoc>(&ok).is_ok());
        // The persisted important-card mark is optional for legacy boards;
        // unrelated future extension fields anywhere must not break loading.
        let extended = ok
            .replacen(
                "{\"projects\"",
                "{\"futureTopLevel\":{\"x\":1},\"projects\"",
                1,
            )
            .replacen(
                "\"title\":\"t\"",
                "\"title\":\"t\",\"pinned\":true,\"futureCard\":true",
                1,
            );
        assert!(
            serde_json::from_str::<BoardDoc>(&extended).is_ok(),
            "unknown fields are tolerated"
        );
        // empty board is a valid first save
        assert!(serde_json::from_str::<BoardDoc>(r#"{"projects":[],"cards":[]}"#).is_ok());
    }

    #[test]
    fn board_buffer_is_optional_bounded_and_validated_without_losing_old_boards() {
        let legacy = board(&card("s1", "P1", "C1", "deck-t-ab12"));
        assert!(serde_json::from_str::<BoardDoc>(&legacy).is_ok());
        let mut value: serde_json::Value = serde_json::from_str(&legacy).unwrap();
        value["cards"][0]["buffer"] = serde_json::json!({
            "revision": 2, "collecting": false, "entries": [{
                "id": "N1", "kind": "manual", "text": "keep me", "revision": 1,
                "createdAt": 1, "updatedAt": 1, "copies": [{
                    "operationId": "B1", "entryRevision": 1, "text": "keep me",
                    "createdAt": 2, "state": "queued"
                }]
            }]
        });
        assert!(serde_json::from_value::<BoardDoc>(value.clone()).is_ok());
        value["cards"][0]["buffer"]["entries"][0]["text"] =
            serde_json::json!("x".repeat(BUFFER_MAX_ENTRY_BYTES + 1));
        assert!(serde_json::from_value::<BoardDoc>(value).is_err());

        let entries: Vec<_> = (0..32)
            .map(|i| {
                serde_json::json!({
                    "id":format!("N{i}"),"kind":"manual","text":"\n".repeat(BUFFER_MAX_ENTRY_BYTES),
                    "revision":1,"createdAt":1,"updatedAt":1,"copies":[]
                })
            })
            .collect();
        let escaped: CardBuffer = serde_json::from_value(serde_json::json!({
            "revision":1,"collecting":false,"entries":entries
        }))
        .unwrap();
        assert!(
            validate_buffer("s1", &escaped).is_err(),
            "2 MiB serialized cap counts JSON escaping"
        );
        let extended: CardBuffer = serde_json::from_value(serde_json::json!({
            "revision":1,"collecting":false,"entries":[{
                "id":"N1","kind":"manual","text":"small","revision":1,
                "createdAt":1,"updatedAt":1,"copies":[],
                "futureMetadata":"x".repeat(BUFFER_MAX_SERIALIZED_BYTES)
            }]
        }))
        .unwrap();
        assert!(
            validate_buffer("s1", &extended).is_err(),
            "unknown nested metadata is preserved in the serialized capacity measurement"
        );
    }

    #[test]
    fn board_channel_run_requires_a_matching_collecting_buffer_and_bounded_frozen_plan() {
        let mut value: serde_json::Value =
            serde_json::from_str(&board(&card("s1", "P1", "C1", "deck-t-ab12"))).unwrap();
        value["cards"][0]["buffer"] =
            serde_json::json!({"revision":1,"collecting":true,"entries":[]});
        value["cards"][0]["channelRun"] = serde_json::json!({
            "groupKey":"default/T1/C1/R1","firstEventId":"Ev1","connectionId":"default",
            "workspaceId":"T1","channelId":"C1","ruleId":"R1","lastCollectedAt":10,
            "idleMinutes":30,"collecting":true,"initialQueued":false,
            "initialSteps":[{"operationId":"B1","text":"frozen","mode":"at","at":10,
                "tpl":"triage","tplIdx":1,"tplTotal":1}]
        });
        assert!(serde_json::from_value::<BoardDoc>(value.clone()).is_ok());
        value["cards"][0]["buffer"]["collecting"] = serde_json::json!(false);
        assert!(serde_json::from_value::<BoardDoc>(value).is_err());
    }

    #[test]
    fn board_connector_run_keeps_a_bounded_frozen_initial_plan() {
        let mut value: serde_json::Value =
            serde_json::from_str(&board(&card("s1", "P1", "C1", "deck-t-ab12"))).unwrap();
        value["cards"][0]["connectorRun"] = serde_json::json!({
            "handle":"a".repeat(64),"presetId":"R1","initialQueued":false,
            "initialSteps":[{"operationId":"B1","text":"frozen","mode":"at","at":10,
                "tpl":"R1","tplIdx":1,"tplTotal":1}]
        });
        assert!(serde_json::from_value::<BoardDoc>(value.clone()).is_ok());
        value["cards"][0]["connectorRun"]["initialSteps"][0]["text"] =
            serde_json::json!("x".repeat(2001));
        assert!(serde_json::from_value::<BoardDoc>(value).is_err());
    }

    /// Project defaults (04) are optional strings: a board without them is
    /// what every earlier version wrote, and a wrong type is refused rather
    /// than guessed at.
    #[test]
    fn project_defaults_are_optional_typed_strings() {
        let plain = board(&card("s1", "P1", "C1", "deck-t-ab12"));
        assert!(serde_json::from_str::<BoardDoc>(&plain).is_ok());
        let with_defaults = plain.replacen(
            "\"name\":\"main\"",
            "\"name\":\"main\",\"dir\":\"~/work/atlas\",\"cmd\":\"claude\"",
            1,
        );
        assert!(
            serde_json::from_str::<BoardDoc>(&with_defaults).is_ok(),
            "a project may carry a default directory and command"
        );
        for wrong in [
            "\"dir\":1",
            "\"cmd\":[\"claude\"]",
            "\"dir\":{\"path\":\"x\"}",
        ] {
            let typed = plain.replacen(
                "\"name\":\"main\"",
                &format!("\"name\":\"main\",{wrong}"),
                1,
            );
            assert!(
                serde_json::from_str::<BoardDoc>(&typed).is_err(),
                "{wrong} is not a string"
            );
        }
        let null_defaults = plain.replacen(
            "\"name\":\"main\"",
            "\"name\":\"main\",\"dir\":null,\"cmd\":null",
            1,
        );
        assert!(
            serde_json::from_str::<BoardDoc>(&null_defaults).is_ok(),
            "null reads as no default"
        );
    }

    #[test]
    fn project_task_presets_are_bounded_and_reference_a_real_column() {
        let plain = board(&card("s1", "P1", "C1", "deck-t-ab12"));
        let with_preset = plain.replacen(
            "\"name\":\"main\"",
            "\"name\":\"main\",\"presets\":[{\"id\":\"R1\",\"name\":\"Fix\",\"columnId\":\"C1\",\"title\":\"Remote task\",\"dir\":\"~/work\",\"cmd\":\"codex\",\"steps\":[\"inspect\",\"fix\"]}]",
            1,
        );
        assert!(serde_json::from_str::<BoardDoc>(&with_preset).is_ok());
        assert!(
            serde_json::from_str::<BoardDoc>(&with_preset.replace("\"codex\"", "\"bash\""))
                .is_err()
        );
        assert!(serde_json::from_str::<BoardDoc>(
            &with_preset.replace("\"codex\"", "\"codex --full-auto\"")
        )
        .is_err());
        assert!(serde_json::from_str::<BoardDoc>(
            &with_preset.replace("\"columnId\":\"C1\"", "\"columnId\":\"missing\"")
        )
        .is_err());
    }

    #[test]
    fn board_validation_rejects_broken_documents() {
        let fail = |doc: &str, why: &str, needle: &str| {
            let e = match serde_json::from_str::<BoardDoc>(doc) {
                Err(e) => e.to_string(),
                Ok(_) => panic!("{why}: invalid document was accepted"),
            };
            assert!(e.contains(needle), "{why}: wrong error {e}");
        };
        // missing runtime field (no session)
        let no_session =
            board(r#"{"id":"s1","projectId":"P1","columnId":"C1","title":"t","cmd":"","dir":""}"#);
        fail(&no_session, "missing session", "session");
        let bad_pinned = board(&card("s1", "P1", "C1", "deck-a-1111").replacen(
            "\"title\":\"t\"",
            "\"title\":\"t\",\"pinned\":\"yes\"",
            1,
        ));
        fail(&bad_pinned, "non-boolean important mark", "boolean");
        // duplicate project id
        let dup_proj = r#"{"projects":[
            {"id":"P1","name":"a","columns":[{"id":"C1","name":"x"}]},
            {"id":"P1","name":"b","columns":[{"id":"C2","name":"y"}]}],"cards":[]}"#;
        fail(dup_proj, "dup project", "duplicate project id");
        // duplicate column id within a project
        let dup_col = r#"{"projects":[{"id":"P1","name":"a","columns":[
            {"id":"C1","name":"x"},{"id":"C1","name":"y"}]}],"cards":[]}"#;
        fail(dup_col, "dup column", "duplicate column id");
        // a project with no columns cannot hold cards
        let no_cols = r#"{"projects":[{"id":"P1","name":"a","columns":[]}],"cards":[]}"#;
        fail(no_cols, "no columns", "no columns");
        // duplicate card ids
        let dup_card = board(&format!(
            "{},{}",
            card("s1", "P1", "C1", "deck-a-1111"),
            card("s1", "P1", "C2", "deck-b-2222")
        ));
        fail(&dup_card, "dup card", "duplicate card id");
        // dangling project reference
        fail(
            &board(&card("s1", "PX", "C1", "deck-a-1111")),
            "dangling project",
            "missing project",
        );
        // column exists but belongs to ANOTHER project
        fail(
            &board(&card("s1", "P1", "C9", "deck-a-1111")),
            "wrong-project column",
            "not in its project",
        );
        // session name breaking the runtime rule (tmux target separators)
        fail(
            &board(&card("s1", "P1", "C1", "has:colon")),
            "illegal session",
            "session name",
        );
        // two cards sharing one tmux session
        let dup_sess = board(&format!(
            "{},{}",
            card("s1", "P1", "C1", "deck-a-1111"),
            card("s2", "P1", "C2", "deck-a-1111")
        ));
        fail(&dup_sess, "dup session", "already used");
    }

    #[test]
    fn voice_preferences_require_supported_unique_languages_and_enabled_default() {
        assert!(serde_json::from_str::<SettingsDoc>(r#"{}"#).is_ok());
        for value in [
            r#"{"languages":["zh-CN","en-US","ja-JP"],"defaultLanguage":"system"}"#,
            r#"{"languages":["ja-JP"],"defaultLanguage":"ja-JP","future":true}"#,
        ] {
            assert!(
                serde_json::from_str::<SettingsDoc>(&format!(r#"{{"voice":{value}}}"#)).is_ok()
            );
        }
        for value in [
            "null",
            "[]",
            "false",
            "{}",
            r#"{"languages":[],"defaultLanguage":"system"}"#,
            r#"{"languages":["en-US","en-US"],"defaultLanguage":"system"}"#,
            r#"{"languages":["unknown"],"defaultLanguage":"system"}"#,
            r#"{"languages":["en-US"],"defaultLanguage":"ja-JP"}"#,
            r#"{"languages":["en-US"],"defaultLanguage":null}"#,
        ] {
            assert!(
                serde_json::from_str::<SettingsDoc>(&format!(r#"{{"voice":{value}}}"#)).is_err()
            );
        }
    }

    #[test]
    fn settings_validation_type_checks_optional_keys() {
        assert!(serde_json::from_str::<SettingsDoc>(r#"{}"#).is_ok());
        assert!(
            serde_json::from_str::<SettingsDoc>(r#"{"editor":"Zed","debug":true,"future":1}"#)
                .is_ok()
        );
        assert!(serde_json::from_str::<SettingsDoc>(r#"{"editor":123}"#).is_err());
        assert!(serde_json::from_str::<SettingsDoc>(r#"{"debug":"yes"}"#).is_err());
        assert!(serde_json::from_str::<SettingsDoc>(r#"{"sessionRestore":true}"#).is_ok());
        assert!(serde_json::from_str::<SettingsDoc>(r#"{"sessionRestore":false}"#).is_ok());
        assert!(serde_json::from_str::<SettingsDoc>(r#"{"sessionRestore":"yes"}"#).is_err());
        for locale in ["system", "en", "zh-Hans"] {
            assert!(
                serde_json::from_str::<SettingsDoc>(&format!(r#"{{"locale":"{locale}"}}"#)).is_ok()
            );
        }
        assert!(serde_json::from_str::<SettingsDoc>(r#"{"locale":"zh-CN"}"#).is_err());
        assert!(serde_json::from_str::<SettingsDoc>(r#"{"locale":false}"#).is_err());
        assert!(serde_json::from_str::<SettingsDoc>(r#"{"locale":null}"#).is_err());
        for theme in ["deck-dark", "light", "system", "high-contrast"] {
            assert!(
                serde_json::from_str::<SettingsDoc>(&format!(r#"{{"theme":"{theme}"}}"#)).is_ok()
            );
        }
        for accent in ["teal", "blue", "purple", "orange"] {
            assert!(
                serde_json::from_str::<SettingsDoc>(&format!(r#"{{"accent":"{accent}"}}"#)).is_ok()
            );
        }
        assert!(serde_json::from_str::<SettingsDoc>(r#"{"theme":"midnight"}"#).is_err());
        assert!(serde_json::from_str::<SettingsDoc>(r#"{"theme":false}"#).is_err());
        assert!(serde_json::from_str::<SettingsDoc>(r#"{"accent":"red"}"#).is_err());
        assert!(serde_json::from_str::<SettingsDoc>(r#"{"accent":null}"#).is_err());
        for scale in [0.5, 1.0, 1.6] {
            assert!(
                serde_json::from_str::<SettingsDoc>(&format!(r#"{{"fontScale":{scale}}}"#)).is_ok()
            );
        }
        assert!(serde_json::from_str::<SettingsDoc>(r#"{"fontScale":"large"}"#).is_err());
        assert!(serde_json::from_str::<SettingsDoc>(r#"{"fontScale":0.4}"#).is_err());
        assert!(serde_json::from_str::<SettingsDoc>(r#"{"fontScale":1.7}"#).is_err());
        assert!(serde_json::from_str::<SettingsDoc>(
            r#"{"shortcuts":{"newSession":"Meta+KeyN","fontIncrease":""}}"#
        )
        .is_ok());
        assert!(serde_json::from_str::<SettingsDoc>(r#"{"shortcuts":[]}"#).is_err());
        assert!(serde_json::from_str::<SettingsDoc>(r#"{"shortcuts":{"x":1}}"#).is_err());
        assert!(serde_json::from_str::<SettingsDoc>(r#"[1,2]"#).is_err());
        for channel in [
            r#""stable""#,
            r#""nightly""#,
            r#""unknown""#,
            "false",
            "null",
        ] {
            let document = format!(r#"{{"updateChannel":{channel}}}"#);
            assert!(
                serde_json::from_str::<SettingsDoc>(&document).is_ok(),
                "unknown/damaged channel must reach the safe Stable migration"
            );
        }
    }

    #[test]
    fn settings_save_persists_only_the_closed_update_channel_enum() {
        for valid in [
            r#"{}"#,
            r#"{"updateChannel":"stable"}"#,
            r#"{"updateChannel":"nightly"}"#,
        ] {
            assert!(validate_saved_update_channel(valid).is_ok());
        }
        for invalid in [
            r#"{"updateChannel":"beta"}"#,
            r#"{"updateChannel":false}"#,
            r#"{"updateChannel":null}"#,
            r#"{"updateChannel":"https://example.com/latest.json"}"#,
        ] {
            assert!(validate_saved_update_channel(invalid).is_err());
        }
    }

    #[test]
    fn locale_setting_persists_with_unknown_fields_and_rejects_atomically() {
        let d = std::env::temp_dir().join(format!("deck-settings-locale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("settings.json");
        let good = r#"{"editor":"Zed","debug":true,"locale":"zh-Hans","future":{"kept":1}}"#;
        save_validated::<SettingsDoc>(&p, good, "settings").unwrap();
        let loaded = storage::load_typed::<SettingsDoc>(&p)
            .unwrap()
            .unwrap()
            .payload;
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&loaded).unwrap(),
            serde_json::from_str::<serde_json::Value>(good).unwrap()
        );
        let before = std::fs::read_to_string(&p).unwrap();
        assert!(save_validated::<SettingsDoc>(
            &p,
            r#"{"locale":"zh-CN","future":{"kept":2}}"#,
            "settings"
        )
        .is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), before);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn save_rejection_touches_neither_main_nor_backup() {
        let d = std::env::temp_dir().join(format!("deck-savereject-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("deck.json");
        let good = board(&card("s1", "P1", "C1", "deck-t-ab12"));
        save_validated::<BoardDoc>(&p, &good, "board").unwrap();
        let before = std::fs::read_to_string(&p).unwrap();

        let bad = board(&card("s1", "PX", "C1", "deck-t-ab12")); // dangling ref
        let err = save_validated::<BoardDoc>(&p, &bad, "board").unwrap_err();
        assert!(err.message().contains("refusing to save"), "{err}");
        assert_eq!(
            std::fs::read_to_string(&p).unwrap(),
            before,
            "main untouched"
        );
        let mut bak = p.as_os_str().to_owned();
        bak.push(".bak");
        assert!(
            !std::path::PathBuf::from(bak).exists(),
            "backup not rotated by a rejected save"
        );
        // a valid save afterwards still works (rejection left no debris)
        save_validated::<BoardDoc>(&p, &good, "board").unwrap();
    }

    #[test]
    fn command_adapters_preserve_closed_status_and_notice_models() {
        let none = to_loaded(None);
        assert_eq!(none.data, "");
        assert_eq!(none.source, "none");
        assert!(none.warning.is_none());

        let recovered = to_loaded(Some(storage::LoadOutcome {
            payload: "{\"ok\":true}".into(),
            source: "backup",
            warning: Some("interrupted deliveries were recovered".into()),
        }));
        assert_eq!(recovered.source, "backup");
        assert_eq!(recovered.warning.unwrap().code, "queue.interrupted");

        let notices = [
            ("privacy hardening failed", "storage.privacy"),
            ("scheduled prompts could not be saved", "queue.persist"),
            ("scheduled prompts could not be loaded", "queue.load"),
            ("command history could not be loaded", "history.load"),
            ("ordinary recovery", "storage.recovered"),
        ];
        for (note, code) in notices {
            assert_eq!(notice_from(note).code, code);
        }

        storage::WARNINGS.lock().unwrap().clear();
        storage::warn("privacy hardening failed".into());
        let drained = storage_warnings();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].code, "storage.privacy");
        assert!(storage_warnings().is_empty());
    }
}
