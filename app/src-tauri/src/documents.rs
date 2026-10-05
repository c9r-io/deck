//! Typed board/settings documents: `BoardDoc` and `SettingsDoc` validate
//! business structure via `try_from` (the SAME rules on load and save), and
//! the load/save commands plus the settings readers other modules use.
//!
//! # Contract
//! - One rule set, two doors. `BoardDoc` / `SettingsDoc` deserialize through
//!   `try_from`, so the loads (quarantine-first recovery on failure) and
//!   `save_board` / `save_settings` (reject before touching disk) run
//!   identical checks: non-empty unique project, column and card
//!   ids; a card's project and column exist; one tmux session per card with
//!   a name the runtime would accept; settings values from closed sets
//!   (locale, theme, accent, update channel, voice languages) or bounded ranges (font scale,
//!   editor name, shortcut table), with `inbound` handed to
//!   `inbound::validate_settings`. A violation is an `InvalidDoc` error with
//!   its rule's message; the file is never rewritten to make it pass.
//! - A card's `reminder` is typed, bounded Board intent (`reminder::Reminder`;
//!   a Board that holds one is a sticky v5 envelope, `storage.rs`), checked on
//!   load and save by `reminder::validate`. `save_board` also fences every
//!   removal or edit of a saved reminder against the caller's explicit
//!   current-revision claims (`reminder::validate_changes`) before anything
//!   is persisted. Those two types and two validators are all this door takes
//!   from the reminder module.
//! - The typed structs are parse-only. The webview owns the documents and
//!   `save_*` persists the ORIGINAL string, so unknown extension fields
//!   round-trip untouched and the `#[allow(dead_code)]` fields exist to be
//!   validated, not read; `launched` defaults to true so a board written
//!   before the field never re-runs a command.
//! - `LoadedDoc` carries the text, its source and at most one `UiNotice`: a
//!   closed code the webview translates, always `storage.recovered` for an
//!   in-band load warning. `storage_warnings` drains the boot-time notices
//!   once, for the first Board render; each is the `storage::StorageNotice`
//!   its emitter named (`storage.privacy`, `queue.persist`, `queue.load`,
//!   `history.load`, `queue.interrupted`, `storage.recovered`), never a code
//!   inferred from the note's wording.
//! - deck.json has one owner, the webview. `load_board`
//!   (`storage::load_as_owner`) is the one load that sets a damaged file
//!   aside, and so the one that reports the recovery — once; a Board recovered
//!   from its backup and not saved since loads the backup again without a
//!   second notice. `save_board` sets aside a main file damaged while deck
//!   ran (the committed Board is the base for the reminder checks) instead of
//!   refusing every save until a restart. The backend's Board readers
//!   (`connector_board_payload`, and `board_project_exists` through it) read
//!   with `storage::read_typed`, never moving a file. A Board that was ever set
//!   aside is never a new empty Board: with nothing loadable left the load
//!   fails, and every save is refused while this process holds no committed
//!   Board (`save_board_at`), each refusal emitting `board-lost`.
//! - The lost Board's way out is the user's explicit choice, never deck's:
//!   `board_recovery_state` answers why a load failed (`lost` with its one way
//!   out, `newer`, `other`) and `board_lost_exit` takes the way
//!   `lost_exit_at` offers now — restore the newest copy a recovery set aside
//!   that passes full validation, or, with none, commit an empty Board. Either
//!   is named only on what was read: while the backup, or a kept copy newer
//!   than any usable one, cannot be read right now, no way out is offered. The
//!   exit commits the chosen Board (lifting the fence) and moves nothing; the
//!   webview saves it through its one transaction queue.
//! - The door keeps the Board this process committed last
//!   (`COMMITTED_BOARD`): the webview's load, its save, or the user's way out
//!   of a lost Board, through `commit_board` and nowhere else. That copy is
//!   what "holds a committed Board" means above. Each commit is then told to
//!   ONE observer, registered once at boot (`set_commit_observer`, the
//!   reminder projection): on the committing thread, before the command
//!   returns, and never while the door's lock is held. The door names no
//!   feature for it, as `tmux_lifecycle` names none for its restart guard.
//! - The committed Board also knows its STANDING, for the readers that treat
//!   what a Board says as authority (a choice the user ticked on a project's
//!   task preset): `Current` when the main file loaded or the owner saved,
//!   `Recovered` when the load answered from the backup or the user took a
//!   way out of a lost Board and has not saved since. deck.json.bak is the
//!   save BEFORE the last one, so it can hold exactly the choice the last
//!   save withdrew; `board_authority` therefore answers only for a `Current`
//!   Board, and a recovered or absent one is "no proof either way" — nothing
//!   granted, nothing revoked — until the owner's next save makes what the
//!   webview holds current. This is the Board's half of the rule settings
//!   follow (`inbound::read_config_strict`, below); every other reader of
//!   the committed Board is unchanged and still sees a recovered one.
//! - `board_fence` orders an automatic send against Board writes, as
//!   `storage::settings_fence` does against settings writes: every commit of
//!   the Board (`load_board`, `save_board` across its disk write,
//!   `board_lost_exit`) is made under it, and a sender that decides on
//!   `board_authority` holds it until its firing intent is persisted. So a
//!   save that withdrew a choice and has RETURNED is always seen, and one
//!   that lands later finds the send already begun. Lock order: the settings
//!   fence, this fence, the queue lock, `SAVE_LOCK`; nothing that holds the
//!   queue lock or `SAVE_LOCK` may take it.
//! - settings.json has one owner, the webview. `load_settings`
//!   (`storage::load_as_owner`) sets a damaged file aside and reports the
//!   recovery once, in-band; `save_settings` (`storage::save_typed_as_owner`)
//!   writes it, setting aside a file damaged since the load instead of
//!   refusing the user. The one other writer is the queue's review barrier
//!   (`storage::ensure_review_schema`), which writes the same document back
//!   under a higher envelope version — on the backup itself while only the
//!   backup loads, so the barrier never makes a recovered document current.
//!   A file whose backup still loads keeps
//!   every setting across the restart and is rebuilt by the next save; one
//!   with no usable backup is a load error the first time and a first run
//!   after that, so Settings is never locked.
//! - The settings readers (`editor_app`, `locale_setting`, `notify_settings`,
//!   `update_channel_setting`, `local_translation_settings`) read the same
//!   typed, validated document on every call, never a cache, through
//!   `storage::read_typed`: they run before the webview and on every later
//!   use, so they never move or write a file. They answer from the backup
//!   while the main file is damaged, and fall back (None / "system" / off /
//!   "stable") when nothing can be read or the field is absent or foreign: a
//!   broken settings file must not take the editor menu, the locale or the
//!   updater down with it. `inbound::read_config_strict` and
//!   `inbound_channel::read_config` read through the same door
//!   (`tests/session_architecture.rs` keeps the quarantining door out of
//!   every reader) but take only the CURRENT version
//!   (`LoadOutcome::source == "main"`): rules, approvals, first-send choices
//!   and monitoring switches are authority, and the backup, one save old,
//!   may hold what the last save withdrew. Storage reports where a document
//!   came from; whether a backup may stand in is each reader's decision.
//! - `save_settings` refuses an unknown `updateChannel` before disk
//!   (`validate_saved_update_channel`) so a build can never be pointed at an
//!   endpoint deck does not ship.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

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
    #[serde(default)]
    reminder: Option<crate::reminder::Reminder>,
    #[serde(default, rename = "reminderRetirements")]
    reminder_retirements: Vec<String>,
    #[serde(default, rename = "channelRun")]
    channel_run: Option<ChannelRun>,
    #[serde(default, rename = "connectorRun")]
    connector_run: Option<ConnectorRun>,
    #[serde(default, rename = "inboundPlan")]
    inbound_plan: Option<InboundPlan>,
}

pub(crate) const BUFFER_MAX_ENTRIES: usize = 256;
pub(crate) const BUFFER_MAX_COPIES: usize = 256;
pub(crate) const BUFFER_MAX_ENTRY_BYTES: usize = 32 * 1024;
pub(crate) const BUFFER_MAX_BYTES: usize = 1024 * 1024;
pub(crate) const BUFFER_MAX_SERIALIZED_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const PRESETS_MAX: usize = 50;
// The bounds of a project task preset and of a frozen plan, as this
// validation counts them. A length here is BYTES (`str::len`), and
// `ui/test/fixtures/limits.json` carries each under a key that says so. A
// preset's steps become a Connector run's frozen steps, so the two share
// the step bounds.
pub(crate) const PRESET_NAME_MAX_BYTES: usize = 120;
pub(crate) const PRESET_TITLE_MAX_BYTES: usize = 120;
pub(crate) const PRESET_DIR_MAX_BYTES: usize = 1024;
pub(crate) const PRESET_CMD_MAX_BYTES: usize = 200;
pub(crate) const PRESET_STEPS_MAX: usize = 20;
pub(crate) const PRESET_STEP_MAX_BYTES: usize = 2000;
pub(crate) const PLAN_STEPS_MAX: usize = 20;
/// The template name a frozen step carries (`tpl`), in bytes as well.
///
/// KNOWN GAP, left open on purpose. The editors and the settings validation
/// bound the same names (a template's, a preset's name and title) at 120
/// CHARACTERS, so a name of 41 to 120 CJK characters passes them and is
/// refused here, when the run card or the project defaults are saved.
/// Counting characters here is not a local change: a Board carrying such a
/// name is damage to every build that counts bytes (it sets the file aside
/// and may go on from an older backup), so it needs a sticky schema version
/// first (`storage.rs`). `limits.json` lists the pairs under `unit_gaps`,
/// and both halves of the mirror hold their side to that list.
pub(crate) const PLAN_TEMPLATE_NAME_MAX_BYTES: usize = 120;
/// A channel collection run waits at most a week of idleness.
pub(crate) const CHANNEL_MAX_IDLE_MINUTES: u32 = 7 * 24 * 60;
pub(crate) const FONT_SCALE_MIN: f64 = 0.5;
pub(crate) const FONT_SCALE_MAX: f64 = 1.6;
pub(crate) const THEMES: &[&str] = &["deck-dark", "light", "system", "high-contrast"];
pub(crate) const ACCENTS: &[&str] = &["teal", "blue", "purple", "orange"];
pub(crate) const LOCALES: &[&str] = &["system", "en", "zh-Hans"];
pub(crate) const SHORTCUTS_MAX: usize = 64;
pub(crate) const SHORTCUT_MAX_LEN: usize = 64;

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

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct InboundPlan {
    operation_id: String,
    review_each: bool,
    initial_steps: Vec<ChannelStep>,
    initial_queued: bool,
}

fn validate_inbound_plan(card_id: &str, plan: &InboundPlan) -> Result<(), DeckError> {
    let valid = bounded_buffer_id(&plan.operation_id)
        && plan.operation_id.len() <= 120
        && (plan.initial_queued || !plan.initial_steps.is_empty())
        && plan.initial_steps.len() <= PLAN_STEPS_MAX
        && plan.initial_steps.iter().enumerate().all(|(index, step)| {
            bounded_buffer_id(&step.operation_id)
                && !step.text.is_empty()
                && step.text.len() <= BUFFER_MAX_ENTRY_BYTES
                && matches!(step.mode.as_str(), "at" | "chain")
                && (step.mode == "at") == step.at.is_some()
                && !step.tpl.is_empty()
                && step.tpl.len() <= PLAN_TEMPLATE_NAME_MAX_BYTES
                && step.tpl_idx == index + 1
                && step.tpl_total == plan.initial_steps.len()
        });
    let _ = (plan.review_each, plan.initial_queued);
    if valid {
        Ok(())
    } else {
        Err(DeckError::new(
            ErrorKind::InvalidDoc,
            format!("card {card_id}: invalid inbound plan"),
        ))
    }
}

fn validate_connector_run(card_id: &str, run: &ConnectorRun) -> Result<(), DeckError> {
    let valid = run.handle.len() == 64
        && run.handle.chars().all(|c| c.is_ascii_hexdigit())
        && bounded_buffer_id(&run.preset_id)
        && run.initial_steps.len() <= PRESET_STEPS_MAX
        && run.initial_steps.iter().enumerate().all(|(index, step)| {
            bounded_buffer_id(&step.operation_id)
                && !step.text.is_empty()
                && step.text.len() <= PRESET_STEP_MAX_BYTES
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
        && run.idle_minutes <= CHANNEL_MAX_IDLE_MINUTES
        && run.initial_steps.len() <= BUFFER_MAX_COPIES
        && buffer.is_some_and(|value| value.collecting == run.collecting)
        && run.initial_steps.iter().enumerate().all(|(index, step)| {
            bounded_buffer_id(&step.operation_id)
                && !step.text.is_empty()
                && step.text.len() <= BUFFER_MAX_ENTRY_BYTES
                && matches!(step.mode.as_str(), "at" | "chain")
                && (step.mode == "at") == step.at.is_some()
                && !step.tpl.is_empty()
                && step.tpl.len() <= PLAN_TEMPLATE_NAME_MAX_BYTES
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
        if p.presets.len() > PRESETS_MAX {
            return Err(DeckError::new(
                ErrorKind::InvalidDoc,
                format!("project {} has too many task presets", p.id),
            ));
        }
        let mut preset_ids = HashSet::new();
        for preset in &p.presets {
            let supported = crate::admission::channel_agent_command(&preset.cmd).is_some();
            if !bounded_buffer_id(&preset.id)
                || !preset_ids.insert(preset.id.as_str())
                || preset.name.is_empty()
                || preset.name.len() > PRESET_NAME_MAX_BYTES
                || preset.title.is_empty()
                || preset.title.len() > PRESET_TITLE_MAX_BYTES
                || preset.dir.is_empty()
                || preset.dir.len() > PRESET_DIR_MAX_BYTES
                || preset.dir.chars().any(char::is_control)
                || preset.cmd.is_empty()
                || preset.cmd.len() > PRESET_CMD_MAX_BYTES
                || !supported
                || !col_ids.contains(preset.column_id.as_str())
                || preset.steps.len() > PRESET_STEPS_MAX
                || preset
                    .steps
                    .iter()
                    .any(|step| step.is_empty() || step.len() > PRESET_STEP_MAX_BYTES)
            {
                return Err(DeckError::new(
                    ErrorKind::InvalidDoc,
                    format!("project {} has an invalid task preset", p.id),
                ));
            }
        }
    }
    let mut reminder_ids = HashSet::new();
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
        if let Some(reminder) = &c.reminder {
            crate::reminder::validate(reminder)?;
            if c.id.len() > 512 || !reminder_ids.insert(reminder.id.as_str()) {
                return Err(DeckError::new(
                    ErrorKind::InvalidDoc,
                    "duplicate or unaddressable card reminder",
                ));
            }
        }
        if c.reminder_retirements.len() > 2
            || c.reminder_retirements.iter().any(|key| {
                key.len() > 2048 || !key.starts_with("exit:") && !key.starts_with("run:")
            })
        {
            return Err(DeckError::new(
                ErrorKind::InvalidDoc,
                "invalid reminder retirement identity",
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
        if let Some(plan) = &c.inbound_plan {
            validate_inbound_plan(&c.id, plan)?;
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
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TranslationPreferencesDoc {
    #[serde(default)]
    #[allow(dead_code)]
    enabled: bool,
    #[serde(rename = "targetLanguage")]
    target_language: String,
    #[serde(
        default = "default_translation_document_limit",
        rename = "documentLimitBytes"
    )]
    document_limit_bytes: usize,
}
fn default_translation_document_limit() -> usize {
    16 * 1024
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalIntelligenceDoc {
    translation: TranslationPreferencesDoc,
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
    #[serde(default, rename = "localIntelligence")]
    local_intelligence: Option<LocalIntelligenceDoc>,
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
    // Away notifications (notify.rs): read at boot by `notify_settings`.
    #[serde(default)]
    #[serde(rename = "notifyAway")]
    #[allow(dead_code)]
    notify_away: Option<bool>,
    #[serde(default)]
    #[serde(rename = "notifySound")]
    #[allow(dead_code)]
    notify_sound: Option<bool>,
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
            if !LOCALES.contains(&locale.as_str()) {
                return Err(DeckError::new(
                    ErrorKind::InvalidDoc,
                    "locale must be system, en, or zh-Hans",
                ));
            }
        }
        if let Some(theme) = &raw.theme {
            if !THEMES.contains(&theme.as_str()) {
                return Err(DeckError::new(
                    ErrorKind::InvalidDoc,
                    "theme must be deck-dark, light, system, or high-contrast",
                ));
            }
        }
        if let Some(accent) = &raw.accent {
            if !ACCENTS.contains(&accent.as_str()) {
                return Err(DeckError::new(
                    ErrorKind::InvalidDoc,
                    "accent must be teal, blue, purple, or orange",
                ));
            }
        }
        if let Some(scale) = raw.font_scale {
            if !scale.is_finite() || !(FONT_SCALE_MIN..=FONT_SCALE_MAX).contains(&scale) {
                return Err(DeckError::new(
                    ErrorKind::InvalidDoc,
                    "fontScale must be between 0.5 and 1.6",
                ));
            }
        }
        if let Some(shortcuts) = &raw.shortcuts {
            if shortcuts.len() > SHORTCUTS_MAX {
                return Err(DeckError::new(
                    ErrorKind::InvalidDoc,
                    "too many shortcut entries",
                ));
            }
            if shortcuts.iter().any(|(key, value)| {
                key.is_empty() || key.len() > SHORTCUT_MAX_LEN || value.len() > SHORTCUT_MAX_LEN
            }) {
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
        if let Some(local) = &raw.local_intelligence {
            let target = &local.translation.target_language;
            if !matches!(target.as_str(), "zh-Hans" | "system") {
                return Err(DeckError::new(
                    ErrorKind::InvalidDoc,
                    "translation target language must be zh-Hans",
                ));
            }
            if !matches!(local.translation.document_limit_bytes, 8192 | 16384) {
                return Err(DeckError::new(
                    ErrorKind::InvalidDoc,
                    "translation document limit must be certified",
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

fn notice(kind: storage::StorageNotice) -> UiNotice {
    UiNotice { code: kind.code() }
}

fn to_loaded(o: Option<storage::LoadOutcome>) -> LoadedDoc {
    match o {
        Some(o) => LoadedDoc {
            data: o.payload,
            source: o.source.into(),
            // a LoadOutcome warning is always a .bak recovery, whatever it says
            warning: o.warning.map(|_| notice(storage::StorageNotice::Recovered)),
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

/// The webview's load of the Board (`storage::load_as_owner`): the one load
/// that sets a damaged main file aside, and so the one that carries the
/// recovery warning — once. A main file set aside earlier keeps loading from
/// its backup without a second notice. A Board that was ever set aside is
/// never a new empty Board, including after a restart: with nothing loadable
/// left the load fails, and so does every save until the user chooses a way
/// forward (`save_board_at`).
fn load_board_at(path: &std::path::Path) -> Result<Option<storage::LoadOutcome>, DeckError> {
    match storage::load_as_owner::<BoardDoc>(path)? {
        Some(doc) => Ok(Some(doc)),
        None if storage::was_quarantined(path)? => Err(DeckError::new(
            ErrorKind::InvalidDoc,
            "quarantined board needs recovery; refusing empty defaults",
        )),
        None => Ok(None),
    }
}

/// The Board this process committed last: what the webview loaded or saved,
/// or the way out it took from a lost Board. The door owns it. It lifts the
/// save fence over a lost Board (`board_lost_at`) and is the base of the
/// reminder checks when the main file was damaged while deck ran
/// (`save_board_at`). Read by `committed_board` and written by `commit_board`,
/// nowhere else.
static COMMITTED_BOARD: Mutex<Option<(serde_json::Value, BoardStanding)>> = Mutex::new(None);

/// Whether the committed Board is the user's current version or one a
/// recovery put in its place (module header). Kept in the same slot as the
/// Board and written with it, by `commit_board` only, so no reader can pair
/// one Board with another's standing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BoardStanding {
    /// the main file loaded, or the owner saved
    Current,
    /// the load answered from the backup, or the user took a way out of a
    /// lost Board and has not saved since
    Recovered,
}

/// The standing of a Board the webview just loaded, from where storage says
/// it came from: only the main file is the current version.
fn load_standing(source: &str) -> BoardStanding {
    if source == "main" {
        BoardStanding::Current
    } else {
        BoardStanding::Recovered
    }
}

/// Orders Board commits against an automatic send that decides on what the
/// Board says (module header).
static BOARD_FENCE: Mutex<()> = Mutex::new(());

pub(crate) fn board_fence() -> std::sync::MutexGuard<'static, ()> {
    BOARD_FENCE.lock_or_recover()
}

/// Test probe: whether the fence is held right now (by anyone).
#[cfg(test)]
pub(crate) fn board_fence_busy() -> bool {
    BOARD_FENCE.try_lock().is_err()
}

/// The committed Board as AUTHORITY: the current version, or `None` when
/// there is none or it is a recovered one (module header). A caller deciding
/// an irreversible step on it holds `board_fence` around the read and that
/// step. Its first reader is the task-preset first-send choice
/// (`scheduler/first_send.rs`); until that lands only the tests call it.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn board_authority() -> Option<serde_json::Value> {
    match COMMITTED_BOARD.lock_or_recover().as_ref() {
        Some((board, BoardStanding::Current)) => Some(board.clone()),
        _ => None,
    }
}

/// The one observer of a commit (see the header): set once at boot by the
/// feature that projects the committed Board, never replaced.
type CommitObserver = fn(&str);
static COMMIT_OBSERVER: OnceLock<CommitObserver> = OnceLock::new();

pub(crate) fn set_commit_observer(observer: CommitObserver) {
    let _ = COMMIT_OBSERVER.set(observer);
}

fn committed_board() -> Option<String> {
    COMMITTED_BOARD
        .lock_or_recover()
        .as_ref()
        .and_then(|(board, _)| serde_json::to_string(board).ok())
}

/// The door's copy is replaced first, under its own lock and nothing else;
/// the observer is told after that lock is released. The caller holds
/// `board_fence`.
fn commit_board(payload: &str, standing: BoardStanding) {
    let Ok(board) = serde_json::from_str::<serde_json::Value>(payload) else {
        return;
    };
    *COMMITTED_BOARD.lock_or_recover() = Some((board, standing));
    if let Some(observer) = COMMIT_OBSERVER.get() {
        observer(payload);
    }
}

#[tauri::command]
pub(crate) fn load_board() -> Result<LoadedDoc, DeckError> {
    let _fence = board_fence();
    let loaded = load_board_at(&board_path())?;
    if let Some(doc) = &loaded {
        commit_board(&doc.payload, load_standing(doc.source));
    }
    Ok(to_loaded(loaded))
}

/// Connector read seam: the returned bytes are the committed, fully typed
/// Board payload selected by normal recovery. Callers project closed DTOs;
/// they never receive a mutable document handle.
pub(crate) fn connector_board_payload() -> Result<String, DeckError> {
    connector_board_payload_at(&board_path())
}

/// Read, never moved (`storage::read_typed`): the Connector and the
/// first-send admission get the best validated copy — the backup while the
/// main file is damaged — and leave setting a file aside to the webview's
/// load, which is the one that tells the user.
fn connector_board_payload_at(path: &std::path::Path) -> Result<String, DeckError> {
    storage::read_typed::<BoardDoc>(path)?
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

/// Emitted when a save is refused because the Board is lost: the webview
/// offers the way out again instead of a failure with no next step.
const BOARD_LOST_EVENT: &str = "board-lost";

#[tauri::command]
pub(crate) fn save_board(
    app: tauri::AppHandle,
    data: String,
    reminder_changes: Option<Vec<crate::reminder::Claim>>,
) -> Result<(), DeckError> {
    if crate::smoke_faults::take("board-save") {
        return Err(DeckError::new(
            ErrorKind::Other,
            "injected board save failure",
        ));
    }
    let path = board_path();
    // held across the disk write and the commit: when this returns, no
    // automatic send can still begin on what the previous Board said
    let _fence = board_fence();
    let saved = save_board_at(
        &path,
        &data,
        &reminder_changes.unwrap_or_default(),
        committed_board(),
    );
    match saved {
        // the owner's save is the current version, whatever was held before
        Ok(()) => commit_board(&data, BoardStanding::Current),
        Err(_) if board_lost_at(&path, committed_board().is_some()).unwrap_or(false) => {
            use tauri::Emitter;
            let _ = app.emit(BOARD_LOST_EVENT, ());
        }
        Err(_) => {}
    }
    saved
}

/// The Board save at `path`, given the Board this process committed last.
fn save_board_at(
    path: &std::path::Path,
    data: &str,
    claims: &[crate::reminder::Claim],
    committed: Option<String>,
) -> Result<(), DeckError> {
    serde_json::from_str::<BoardDoc>(data)
        .map_err(|_| DeckError::new(ErrorKind::InvalidDoc, "invalid board"))?;
    let next: serde_json::Value = serde_json::from_str(data)
        .map_err(|_| DeckError::new(ErrorKind::InvalidDoc, "invalid board"))?;
    if board_lost_at(path, committed.is_some())? {
        return Err(DeckError::new(
            ErrorKind::InvalidDoc,
            "quarantined board needs recovery before saving",
        ));
    }
    // A main file this build cannot use, while this process holds a committed
    // Board, was damaged behind the webview's back: the committed Board is
    // the base for the reminder checks, and the owner's save sets the damaged
    // file aside instead of refusing every save until a restart. With nothing
    // committed the file is not this process's to replace: refused.
    let disk = match storage::peek_typed::<BoardDoc>(path) {
        Ok(found) => found,
        Err(error) if error.kind() == ErrorKind::InvalidDoc && committed.is_some() => None,
        Err(error) => return Err(error),
    };
    let old = disk
        .or(committed)
        .map(|payload| serde_json::from_str::<serde_json::Value>(&payload))
        .transpose()
        .map_err(|_| DeckError::new(ErrorKind::InvalidDoc, "invalid committed board"))?
        .unwrap_or_else(|| serde_json::json!({"cards":[]}));
    crate::reminder::validate_changes(&old, &next, claims)?;
    storage::save_typed_as_owner::<BoardDoc>(path, data)
}

/// The lost Board: nothing loadable is left on disk — a recovery set the
/// main file aside and no usable backup remains, or the files were removed
/// after one — and this process committed no Board. Every save is refused
/// in this state (a save would be a silent empty Board) until the user
/// chooses a way forward (`board_lost_exit`).
fn board_lost_at(path: &std::path::Path, committed: bool) -> Result<bool, DeckError> {
    Ok(!committed && !path.exists() && storage::was_quarantined(path)?)
}

/// Why the Board did not load, as a closed answer for the webview (an error
/// crosses IPC as its message only): `lost` (above) with its one way out —
/// the copy that can be restored, or none for a new Board; `newer`, written
/// by a newer deck — update deck; `other`, anything else, including a lost
/// Board whose backup or newest kept copies cannot be read right now (no way
/// out is offered: the next start looks again).
#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct BoardRecovery {
    state: &'static str,
    kept: Option<KeptBoard>,
}

/// What the dialog says about a kept copy: when it was last written (ms
/// since the epoch) and how many cards it holds — never its file name.
#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KeptBoard {
    saved_at: u64,
    cards: usize,
}

/// The one way out of the lost Board.
enum LostExit {
    /// the newest copy a recovery set aside that passes full validation now
    Restore(KeptBoard, String),
    /// nothing can be restored: an empty Board
    New,
}

/// The way out of the lost Board this process can offer now; `Ok(None)`
/// when the Board is not lost. A way out is named only on what was READ:
/// the backup (absent, or read and unusable — one that loads now is not
/// lost, the next start loads it), then the copies a recovery set aside,
/// newest first. The newest that passes full validation is restored; with
/// none, a new Board. A file that cannot be read right now may be intact
/// and newer — the saves that follow either way out would replace the
/// backup — so that is an error (unknown) and no way out, until it reads.
/// Nothing here moves or writes a file.
fn lost_exit_at(path: &std::path::Path, committed: bool) -> Result<Option<LostExit>, DeckError> {
    if !board_lost_at(path, committed)? || storage::read_typed::<BoardDoc>(path)?.is_some() {
        return Ok(None);
    }
    let Some((kept, payload)) = storage::newest_valid_copy::<BoardDoc>(path)? else {
        return Ok(Some(LostExit::New));
    };
    let cards = serde_json::from_str::<serde_json::Value>(&payload)
        .map_err(|_| DeckError::new(ErrorKind::InvalidDoc, "invalid kept board"))?["cards"]
        .as_array()
        .map_or(0, Vec::len);
    let written = std::fs::metadata(&kept)?.modified()?;
    let saved_at = written
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        });
    Ok(Some(LostExit::Restore(
        KeptBoard { saved_at, cards },
        payload,
    )))
}

fn board_recovery_at(path: &std::path::Path, committed: bool) -> BoardRecovery {
    if matches!(
        storage::read_typed::<BoardDoc>(path),
        Err(error) if error.kind() == ErrorKind::NewerSchema
    ) {
        return BoardRecovery {
            state: "newer",
            kept: None,
        };
    }
    match lost_exit_at(path, committed) {
        Ok(Some(LostExit::Restore(kept, _))) => BoardRecovery {
            state: "lost",
            kept: Some(kept),
        },
        Ok(Some(LostExit::New)) => BoardRecovery {
            state: "lost",
            kept: None,
        },
        Ok(None) | Err(_) => BoardRecovery {
            state: "other",
            kept: None,
        },
    }
}

#[tauri::command]
pub(crate) fn board_recovery_state() -> BoardRecovery {
    board_recovery_at(&board_path(), committed_board().is_some())
}

/// The Board a new start commits: no project, no card.
const NO_BOARD: &str = r#"{"projects":[],"cards":[]}"#;

/// The user's way out of the lost Board, by an explicit choice — besides a
/// load, the only thing that commits a Board and so lifts the save fence.
/// Only the way `lost_exit_at` offers now is taken: `restore` commits the
/// kept copy it validated, `new` an empty Board, so a copy that could be
/// restored is never given up by a choice the user cannot take back.
/// Nothing on disk moves — the kept copies and the backup stay; the webview
/// then saves the chosen Board through its one transaction queue.
fn board_lost_exit_at(
    path: &std::path::Path,
    action: &str,
    committed: bool,
) -> Result<String, DeckError> {
    match (action, lost_exit_at(path, committed)?) {
        ("restore", Some(LostExit::Restore(_, payload))) => Ok(payload),
        ("new", Some(LostExit::New)) => Ok(NO_BOARD.into()),
        ("restore" | "new", _) => Err(DeckError::new(
            ErrorKind::Invalid,
            "that way out of a lost Board is not open",
        )),
        _ => Err(DeckError::new(ErrorKind::Invalid, "unknown Board exit")),
    }
}

/// Takes the exit and answers with the Board it committed, for the webview
/// to hold in place of its placeholder.
#[tauri::command]
pub(crate) fn board_lost_exit(action: String) -> Result<String, DeckError> {
    let _fence = board_fence();
    let payload = board_lost_exit_at(&board_path(), &action, committed_board().is_some())?;
    // a kept copy or an empty Board the user chose, not yet their saved
    // version: the webview saves it next, and that save makes it current
    commit_board(&payload, BoardStanding::Recovered);
    Ok(payload)
}

/// Boot-time storage notices (corruption recovered from .bak, etc.) for the
/// frontend to surface as toasts.
#[tauri::command]
pub(crate) fn storage_warnings() -> Vec<UiNotice> {
    storage::take_notices().into_iter().map(notice).collect()
}

// ---------- settings ------------------------------------------------------------

pub(crate) fn settings_path() -> PathBuf {
    crate::datadir::deck_dir().join("settings.json")
}

/// The webview's load: a damaged settings file is set aside here, its
/// recovery reported once, and a file set aside earlier keeps loading from
/// its backup (`storage::load_as_owner`).
fn load_settings_at(path: &std::path::Path) -> Result<Option<storage::LoadOutcome>, DeckError> {
    storage::load_as_owner::<SettingsDoc>(path)
}

#[tauri::command]
pub(crate) fn load_settings() -> Result<LoadedDoc, DeckError> {
    Ok(to_loaded(load_settings_at(&settings_path())?))
}

/// The webview's save. The same full validation as load, before anything
/// touches disk; a main file damaged while deck was running is set aside
/// rather than refusing the user (`storage::save_typed_as_owner`).
fn save_settings_at(path: &std::path::Path, data: &str) -> Result<(), DeckError> {
    validate_saved_update_channel(data)?;
    // a revoked automation approval is committed only under the fence the
    // scheduler's pre-fire authority check holds (storage::settings_fence)
    let _fence = storage::settings_fence();
    serde_json::from_str::<SettingsDoc>(data)
        .map_err(|e| DeckError::classified(format!("refusing to save invalid settings: {e}")))?;
    storage::save_typed_as_owner::<SettingsDoc>(path, data)
}

#[tauri::command]
pub(crate) fn save_settings(data: String) -> Result<(), DeckError> {
    if crate::smoke_faults::take("settings-save") {
        return Err(DeckError::new(
            ErrorKind::Other,
            "injected settings save failure",
        ));
    }
    save_settings_at(&settings_path(), &data)
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
/// are advisory, and a bad file must never stop the app from booting. These
/// readers run before the webview loads and on every later use, so they read
/// without moving anything (`storage::read_typed`): a damaged main file is
/// answered from its backup and stays where it is for the webview to recover.
fn settings_value() -> Option<serde_json::Value> {
    settings_value_at(&settings_path())
}

fn settings_value_at(path: &std::path::Path) -> Option<serde_json::Value> {
    let raw = storage::read_typed::<SettingsDoc>(path).ok()??.payload;
    serde_json::from_str(&raw).ok()
}

/// Advisory at-rest feature setting. The native translation facade also
/// verifies the installed model before every initial load.
pub(crate) fn local_translation_settings() -> (bool, usize) {
    let value = settings_value();
    let translation = value
        .as_ref()
        .and_then(|settings| settings.get("localIntelligence"))
        .and_then(|local| local.get("translation"));
    let enabled = translation
        .and_then(|item| item.get("enabled"))
        .and_then(serde_json::Value::as_bool)
        == Some(true);
    let limit = translation
        .and_then(|item| item.get("documentLimitBytes"))
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .filter(|value| matches!(value, 8192 | 16384))
        .unwrap_or(16384);
    (enabled, limit)
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

/// The away-notification switch and its sound, both off unless saved true.
pub(crate) fn notify_settings() -> (bool, bool) {
    notify_from(settings_value().as_ref())
}

fn notify_from(settings: Option<&serde_json::Value>) -> (bool, bool) {
    let flag = |key: &str| {
        settings
            .and_then(|s| s.get(key))
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    };
    (flag("notifyAway"), flag("notifySound"))
}

pub(crate) fn update_channel_setting() -> String {
    update_channel_from(settings_value().as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same full business validation as load, BEFORE anything touches
    /// disk: an invalid document never overwrites the main file or rotates
    /// the .bak (the plain typed save the validation tests below exercise).
    fn save_validated<T: serde::de::DeserializeOwned>(
        path: &std::path::Path,
        data: &str,
        what: &str,
    ) -> Result<(), DeckError> {
        serde_json::from_str::<T>(data)
            .map_err(|e| DeckError::classified(format!("refusing to save invalid {what}: {e}")))?;
        storage::save_typed::<T>(path, data)
    }

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

    #[test]
    fn board_quarantine_survives_reload_without_empty_default_overwrite() {
        let dir =
            std::env::temp_dir().join(format!("deck-reminder-recovery-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("deck.json");
        let payload = board(&card("a", "P1", "C1", "shell"));
        storage::save_typed::<BoardDoc>(&path, &payload).unwrap();
        storage::save_typed::<BoardDoc>(&path, &payload).unwrap();
        std::fs::write(&path, "damaged").unwrap();
        assert_eq!(load_board_at(&path).unwrap().unwrap().source, "backup");
        assert!(!path.exists());
        let reloaded = load_board_at(&path).unwrap().unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&reloaded.payload).unwrap(),
            serde_json::from_str::<serde_json::Value>(&payload).unwrap()
        );
        assert_eq!(reloaded.source, "backup");
        std::fs::remove_file(dir.join("deck.json.bak")).unwrap();
        assert!(load_board_at(&path).is_err());
        assert!(!path.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    // ---------- board recovery: the state matrix ----------
    //
    // Main file × backup × "set aside before" × what this process committed,
    // one cell per test. The OWNER is the webview (`load_board_at`,
    // `save_board_at`); the READERS are the backend's (`connector_board_payload_at`).
    // `committed` stands for the reminder module's process-wide mirror,
    // passed explicitly so parallel tests never share it.

    struct BoardDir(PathBuf);

    impl BoardDir {
        fn empty(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("deck-board-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            BoardDir(dir)
        }

        /// Saved twice by this process: the main file holds `latest_board()`, the
        /// backup `older_board()`.
        fn saved_twice(tag: &str) -> Self {
            let dir = Self::empty(tag);
            save_board_at(&dir.main(), &older_board(), &[], None).unwrap();
            save_board_at(&dir.main(), &latest_board(), &[], Some(older_board())).unwrap();
            dir
        }

        fn main(&self) -> PathBuf {
            self.0.join("deck.json")
        }

        fn backup(&self) -> PathBuf {
            self.0.join("deck.json.bak")
        }

        /// Every file with its bytes: equal listings mean nothing was moved,
        /// created, removed or rewritten.
        fn files(&self) -> Vec<(String, Vec<u8>)> {
            let mut files: Vec<_> = std::fs::read_dir(&self.0)
                .unwrap()
                .flatten()
                .map(|entry| {
                    (
                        entry.file_name().to_string_lossy().into_owned(),
                        std::fs::read(entry.path()).unwrap_or_default(),
                    )
                })
                .collect();
            files.sort();
            files
        }

        /// The bytes of every file a recovery set aside.
        fn kept(&self) -> Vec<Vec<u8>> {
            self.files()
                .into_iter()
                .filter(|(name, _)| name.starts_with("deck.corrupt-"))
                .map(|(_, bytes)| bytes)
                .collect()
        }
    }

    impl Drop for BoardDir {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt;
            for entry in std::fs::read_dir(&self.0).into_iter().flatten().flatten() {
                let _ =
                    std::fs::set_permissions(entry.path(), std::fs::Permissions::from_mode(0o600));
            }
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn older_board() -> String {
        board(&card("a", "P1", "C1", "shell-a"))
    }

    fn latest_board() -> String {
        board(&format!(
            "{},{}",
            card("a", "P1", "C1", "shell-a"),
            card("b", "P1", "C2", "shell-b")
        ))
    }

    fn cards_in(payload: &str) -> usize {
        serde_json::from_str::<serde_json::Value>(payload).unwrap()["cards"]
            .as_array()
            .unwrap()
            .len()
    }

    /// Normal: the owner loads the main file and saves keep the previous
    /// version as the backup; reading changes nothing.
    #[test]
    fn board_matrix_normal_loads_the_main_file() {
        let d = BoardDir::saved_twice("normal");
        let before = d.files();
        let loaded = load_board_at(&d.main()).unwrap().unwrap();
        assert_eq!((loaded.source, loaded.warning.is_none()), ("main", true));
        assert_eq!(cards_in(&loaded.payload), 2);
        assert_eq!(cards_in(&connector_board_payload_at(&d.main()).unwrap()), 2);
        assert_eq!(d.files(), before);
        assert!(d.kept().is_empty());
    }

    /// A backend reader that gets to a damaged Board first (the Connector,
    /// the first-send admission) reads the backup and moves nothing: the
    /// webview's load is the one that sets the file aside, and the one told.
    #[test]
    fn board_matrix_a_backend_reader_never_moves_the_file() {
        let d = BoardDir::saved_twice("reader-first");
        std::fs::write(d.main(), "{damaged").unwrap();
        let before = d.files();
        for round in 0..2 {
            let read = connector_board_payload_at(&d.main()).unwrap();
            assert_eq!(cards_in(&read), 1, "read {round}: the backup answers");
        }
        assert_eq!(d.files(), before, "a backend read moved or rewrote a file");
        let loaded = load_board_at(&d.main()).unwrap().unwrap();
        assert_eq!(loaded.source, "backup");
        assert!(loaded.warning.is_some(), "the webview is told");
        assert_eq!(d.kept(), [b"{damaged".to_vec()]);
    }

    /// Recovered from the backup and not saved since: every later start
    /// loads the backup again, and says nothing more — the start that set the
    /// file aside already told the user.
    #[test]
    fn board_matrix_recovery_is_reported_once() {
        let d = BoardDir::saved_twice("told-once");
        std::fs::write(d.main(), "{damaged").unwrap();
        let first = load_board_at(&d.main()).unwrap().unwrap();
        assert_eq!((first.source, first.warning.is_some()), ("backup", true));
        for start in 0..2 {
            let again = load_board_at(&d.main()).unwrap().unwrap();
            assert_eq!(again.source, "backup", "start {start}");
            assert!(again.warning.is_none(), "start {start}: told once");
            assert_eq!(cards_in(&again.payload), 1);
        }
        // the first save puts the main file back
        save_board_at(&d.main(), &older_board(), &[], Some(older_board())).unwrap();
        let rebuilt = load_board_at(&d.main()).unwrap().unwrap();
        assert_eq!((rebuilt.source, rebuilt.warning), ("main", None));
    }

    /// A main file damaged while deck runs (the webview holds the committed
    /// Board): the save is not refused until a restart — the damaged file is
    /// set aside, the backup kept, and what the user sees is written.
    #[test]
    fn board_matrix_damage_while_running_never_refuses_the_owners_save() {
        let d = BoardDir::saved_twice("damaged-running");
        let backup = std::fs::read(d.backup()).unwrap();
        std::fs::write(d.main(), "{damaged").unwrap();
        save_board_at(&d.main(), &latest_board(), &[], Some(latest_board())).unwrap();
        let loaded = load_board_at(&d.main()).unwrap().unwrap();
        assert_eq!((loaded.source, cards_in(&loaded.payload)), ("main", 2));
        assert_eq!(d.kept(), [b"{damaged".to_vec()]);
        assert_eq!(std::fs::read(d.backup()).unwrap(), backup);
        // with nothing committed in this process the damaged file is not the
        // owner's to replace: refused, untouched
        std::fs::write(d.main(), "{damaged again").unwrap();
        let before = d.files();
        assert!(save_board_at(&d.main(), &latest_board(), &[], None).is_err());
        assert_eq!(d.files(), before);
    }

    /// Nothing loadable is left — both copies damaged, or a Board saved only
    /// once (no backup yet), or an old quarantine and the files removed —
    /// and nothing was committed: the load fails, and every save is refused
    /// until the user chooses a way forward. Never a silent empty Board.
    #[test]
    fn board_matrix_nothing_loadable_stays_lost_until_the_user_chooses() {
        let lost = |tag: &str, main: Option<&str>, backup: Option<&str>, kept: bool| {
            let d = BoardDir::empty(tag);
            if let Some(bytes) = main {
                std::fs::write(d.main(), bytes).unwrap();
            }
            if let Some(bytes) = backup {
                std::fs::write(d.backup(), bytes).unwrap();
            }
            if kept {
                std::fs::write(d.0.join("deck.corrupt-1700000000"), "long ago").unwrap();
            }
            d
        };
        for (cell, d) in [
            (
                "both damaged",
                lost("lost-both", Some("{damaged"), Some("{worse"), false),
            ),
            (
                "saved once",
                lost("lost-once", Some("{damaged"), None, false),
            ),
            (
                "removed after an old quarantine",
                lost("lost-removed", None, None, true),
            ),
        ] {
            for start in 0..2 {
                assert!(load_board_at(&d.main()).is_err(), "{cell}: start {start}");
                assert!(
                    save_board_at(&d.main(), &older_board(), &[], None).is_err(),
                    "{cell}: start {start}: a save would be a silent empty Board"
                );
            }
            assert!(!d.main().exists(), "{cell}");
            assert!(!d.kept().is_empty(), "{cell}: the damaged bytes are kept");
        }
    }

    /// Written by a newer deck: refused as it is, by every door.
    #[test]
    fn board_matrix_newer_schema_is_refused_untouched() {
        let d = BoardDir::empty("newer");
        std::fs::write(
            d.main(),
            r#"{"schema_version":99,"data":{"projects":[],"cards":[]}}"#,
        )
        .unwrap();
        let before = d.files();
        assert_eq!(
            load_board_at(&d.main()).unwrap_err().kind(),
            ErrorKind::NewerSchema
        );
        assert_eq!(
            connector_board_payload_at(&d.main()).unwrap_err().kind(),
            ErrorKind::NewerSchema
        );
        assert_eq!(
            save_board_at(&d.main(), &older_board(), &[], Some(older_board()))
                .unwrap_err()
                .kind(),
            ErrorKind::NewerSchema
        );
        assert_eq!(d.files(), before);
        // the closed answer says why, and the lost Board's exits stay shut
        assert_eq!(board_recovery_at(&d.main(), false).state, "newer");
        assert!(board_lost_exit_at(&d.main(), "new", false).is_err());
        assert_eq!(d.files(), before);
    }

    /// A Board whose one card carries a reminder in `zone`.
    fn with_reminder_zone(zone: &str) -> String {
        board(&format!(
            r#"{{"id":"a","projectId":"P1","columnId":"C1","title":"t","desc":"","cmd":"claude","dir":"~/w","session":"shell-a",
                "reminder":{{"id":"0123456789abcdef0123456789abcdef","revision":1,"dueAt":4102444800000,"timeZone":"{zone}","note":"","inAppOnly":false,"due":false}}}}"#
        ))
    }

    /// The lost Board's way out when a kept copy is intact: a Board saved
    /// once (no backup yet) whose main file could not be READ at the load
    /// was set aside intact. (Mode 000 stands in for a failing read here;
    /// in the app, launch repairs plain mode bits before the load.) The
    /// closed answer offers that copy; a new start is refused while it can
    /// be restored; restoring moves nothing on disk, and the webview's save
    /// of it then goes through.
    #[test]
    fn board_exit_restores_an_intact_kept_copy_and_offers_nothing_else() {
        use std::os::unix::fs::PermissionsExt;
        let d = BoardDir::empty("exit-restore");
        save_board_at(&d.main(), &latest_board(), &[], None).unwrap();
        std::fs::set_permissions(d.main(), std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read(d.main()).is_ok() {
            return; // a privileged test user reads through any mode
        }
        assert!(
            load_board_at(&d.main()).is_err(),
            "nothing else is loadable"
        );
        let recovery = board_recovery_at(&d.main(), false);
        assert_eq!(recovery.state, "lost");
        let kept = recovery.kept.expect("the kept copy is intact");
        assert_eq!(kept.cards, 2);
        assert!(kept.saved_at > 1_700_000_000_000, "{}", kept.saved_at);
        // the answer as the dialog reads it
        assert_eq!(
            serde_json::to_value(board_recovery_at(&d.main(), false)).unwrap(),
            serde_json::json!({"state": "lost", "kept": {"savedAt": kept.saved_at, "cards": 2}})
        );
        let before = d.files();
        assert_eq!(
            board_lost_exit_at(&d.main(), "new", false)
                .unwrap_err()
                .kind(),
            ErrorKind::Invalid,
            "a copy that can be restored is never given up"
        );
        let restored = board_lost_exit_at(&d.main(), "restore", false).unwrap();
        assert_eq!(cards_in(&restored), 2);
        assert_eq!(d.files(), before, "the exit moves and writes nothing");
        // the webview's first save of the restored Board goes through
        save_board_at(&d.main(), &restored, &[], Some(restored.clone())).unwrap();
        let loaded = load_board_at(&d.main()).unwrap().unwrap();
        assert_eq!((loaded.source, cards_in(&loaded.payload)), ("main", 2));
        assert_eq!(board_recovery_at(&d.main(), true).state, "other");
        assert!(
            board_lost_exit_at(&d.main(), "restore", true).is_err(),
            "not lost any more"
        );
        assert_eq!(d.kept().len(), 1, "the kept copy stays where it is");
    }

    /// The lost Board's way out when no copy can be restored — both copies
    /// damaged, a Board saved once, the files removed after an old
    /// quarantine, a reminder time zone this Mac does not know (the whole
    /// Board stays invalid, by decision): only a new start, which commits an
    /// empty Board; the webview's first save then goes through.
    #[test]
    fn board_exit_starts_new_only_when_nothing_can_be_restored() {
        let lost = |tag: &str, main: Option<String>, backup: Option<String>, kept: bool| {
            let d = BoardDir::empty(tag);
            if let Some(bytes) = main {
                std::fs::write(d.main(), bytes).unwrap();
            }
            if let Some(bytes) = backup {
                std::fs::write(d.backup(), bytes).unwrap();
            }
            if kept {
                std::fs::write(d.0.join("deck.corrupt-1700000000"), "long ago").unwrap();
            }
            d
        };
        let unknown_zone = with_reminder_zone("Mars/Olympus");
        assert!(serde_json::from_str::<BoardDoc>(&with_reminder_zone("Asia/Tokyo")).is_ok());
        for (cell, d) in [
            (
                "both damaged",
                lost(
                    "new-both",
                    Some("{damaged".into()),
                    Some("{worse".into()),
                    false,
                ),
            ),
            (
                "saved once",
                lost("new-once", Some("{damaged".into()), None, false),
            ),
            (
                "removed after an old quarantine",
                lost("new-removed", None, None, true),
            ),
            (
                "unknown time zone",
                lost(
                    "new-zone",
                    Some(unknown_zone.clone()),
                    Some(unknown_zone.clone()),
                    false,
                ),
            ),
        ] {
            let _ = load_board_at(&d.main());
            let recovery = board_recovery_at(&d.main(), false);
            assert_eq!((recovery.state, &recovery.kept), ("lost", &None), "{cell}");
            assert_eq!(
                board_lost_exit_at(&d.main(), "restore", false)
                    .unwrap_err()
                    .kind(),
                ErrorKind::Invalid,
                "{cell}"
            );
            let before = d.files();
            let committed = board_lost_exit_at(&d.main(), "new", false).unwrap();
            assert_eq!(committed, NO_BOARD, "{cell}");
            assert_eq!(
                d.files(),
                before,
                "{cell}: the exit moves and writes nothing"
            );
            save_board_at(&d.main(), &older_board(), &[], Some(committed)).unwrap();
            let loaded = load_board_at(&d.main()).unwrap().unwrap();
            assert_eq!(
                (loaded.source, cards_in(&loaded.payload)),
                ("main", 1),
                "{cell}"
            );
            assert!(
                !d.kept().is_empty(),
                "{cell}: the kept bytes are still there"
            );
        }
    }

    /// Several kept copies: the newest that passes full validation now is the
    /// one offered — a newer damaged copy is skipped, and copies set aside in
    /// the same second are ordered by their counter.
    #[test]
    fn board_exit_offers_the_newest_kept_copy_that_still_validates() {
        let d = BoardDir::empty("exit-newest");
        for (name, bytes) in [
            ("deck.corrupt-1700000000", latest_board()),
            ("deck.corrupt-1700000500", latest_board()),
            ("deck.corrupt-1700000500-1", older_board()),
            ("deck.corrupt-1700000900", "{damaged".to_string()),
            ("settings.corrupt-1800000000", older_board()),
        ] {
            std::fs::write(d.0.join(name), bytes).unwrap();
        }
        let recovery = board_recovery_at(&d.main(), false);
        assert_eq!(recovery.state, "lost");
        assert_eq!(recovery.kept.map(|kept| kept.cards), Some(1));
        let restored = board_lost_exit_at(&d.main(), "restore", false).unwrap();
        assert_eq!(json(&restored), json(&older_board()));
    }

    /// A file that cannot be read right now may be intact, and newer than
    /// anything deck could offer: while the backup, or a kept copy newer than
    /// any usable one, cannot be read, no way out is offered — neither a new
    /// Board nor a restore, since the saves after either would replace the
    /// backup. The next look, once it reads, finds the way. A kept copy
    /// older than the one restored never stands in its way.
    #[test]
    fn board_exit_offers_nothing_while_a_file_that_could_be_newer_cannot_be_read() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: PathBuf, bits: u32| {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(bits)).unwrap()
        };
        let no_way_out = |d: &BoardDir| {
            let before = d.files();
            assert_eq!(board_recovery_at(&d.main(), false).state, "other");
            assert!(board_lost_exit_at(&d.main(), "new", false).is_err());
            assert!(board_lost_exit_at(&d.main(), "restore", false).is_err());
            assert_eq!(d.files(), before);
        };
        // the main file damaged, its backup unreadable
        let d = BoardDir::saved_twice("unread-backup");
        std::fs::write(d.main(), "{damaged").unwrap();
        mode(d.backup(), 0o000);
        if std::fs::read(d.backup()).is_ok() {
            return; // a privileged test user reads through any mode
        }
        assert!(load_board_at(&d.main()).is_err());
        no_way_out(&d);
        mode(d.backup(), 0o600);
        let loaded = load_board_at(&d.main()).unwrap().unwrap();
        assert_eq!((loaded.source, cards_in(&loaded.payload)), ("backup", 1));

        // the newest kept copy unreadable, an older one damaged
        let d = BoardDir::empty("unread-kept");
        std::fs::write(d.0.join("deck.corrupt-1700000000"), "{damaged").unwrap();
        std::fs::write(d.0.join("deck.corrupt-1700000500"), latest_board()).unwrap();
        mode(d.0.join("deck.corrupt-1700000500"), 0o000);
        no_way_out(&d);
        mode(d.0.join("deck.corrupt-1700000500"), 0o600);
        let recovery = board_recovery_at(&d.main(), false);
        assert_eq!(recovery.state, "lost");
        assert_eq!(recovery.kept.map(|kept| kept.cards), Some(2));

        // a kept copy validates, but the backup cannot be read: not even a
        // restore; once the backup reads, the next load answers from it
        let d = BoardDir::empty("unread-backup-kept");
        std::fs::write(d.0.join("deck.corrupt-1700000000"), older_board()).unwrap();
        save_board_at(&d.backup(), &latest_board(), &[], None).unwrap();
        mode(d.backup(), 0o000);
        no_way_out(&d);
        mode(d.backup(), 0o600);
        assert_eq!(board_recovery_at(&d.main(), false).state, "other");
        let loaded = load_board_at(&d.main()).unwrap().unwrap();
        assert_eq!((loaded.source, cards_in(&loaded.payload)), ("backup", 2));

        // an unreadable kept copy OLDER than a usable one is not in the way
        let d = BoardDir::empty("unread-older-kept");
        std::fs::write(d.0.join("deck.corrupt-1700000000"), older_board()).unwrap();
        std::fs::write(d.0.join("deck.corrupt-1700000500"), latest_board()).unwrap();
        mode(d.0.join("deck.corrupt-1700000000"), 0o000);
        let recovery = board_recovery_at(&d.main(), false);
        assert_eq!(recovery.state, "lost");
        assert_eq!(recovery.kept.map(|kept| kept.cards), Some(2));
        assert!(board_lost_exit_at(&d.main(), "new", false).is_err());
        assert_eq!(
            cards_in(&board_lost_exit_at(&d.main(), "restore", false).unwrap()),
            2
        );
    }

    /// The exits are for the lost Board only, and their vocabulary is closed.
    #[test]
    fn board_exit_is_refused_unless_the_board_is_lost() {
        let d = BoardDir::saved_twice("exit-not-lost");
        let before = d.files();
        assert_eq!(board_recovery_at(&d.main(), false).state, "other");
        for action in ["restore", "new"] {
            assert_eq!(
                board_lost_exit_at(&d.main(), action, false)
                    .unwrap_err()
                    .kind(),
                ErrorKind::Invalid,
                "{action}"
            );
        }
        assert_eq!(d.files(), before);
        // set aside once, the backup loads: the next load answers from it
        std::fs::remove_file(d.main()).unwrap();
        std::fs::write(d.0.join("deck.corrupt-1700000000"), "long ago").unwrap();
        assert_eq!(board_recovery_at(&d.main(), false).state, "other");
        assert!(board_lost_exit_at(&d.main(), "new", false).is_err());
        // nothing loadable, but committed in this process: not lost either
        std::fs::remove_file(d.backup()).unwrap();
        assert_eq!(board_recovery_at(&d.main(), true).state, "other");
        assert!(board_lost_exit_at(&d.main(), "new", true).is_err());
        assert_eq!(board_recovery_at(&d.main(), false).state, "lost");
        assert_eq!(
            board_lost_exit_at(&d.main(), "start-over", false)
                .unwrap_err()
                .kind(),
            ErrorKind::Invalid
        );
    }

    // ---------- settings recovery: the state matrix ----------
    //
    // Main file × backup × "a recovery set it aside before", one cell per
    // test. The READERS are the backend's own reads of settings.json (every
    // reader in this module goes through `settings_value_at`); the OWNER is
    // the webview, through `load_settings_at` and `save_settings_at`.

    /// The generation that ends up in the backup.
    const OLDER: &str = r#"{"locale":"zh-Hans","editor":"Zed","notifyAway":true,"notifySound":true,"updateChannel":"nightly"}"#;
    /// The generation in the main file.
    const LATEST: &str = r#"{"locale":"en","editor":"Cursor","notifyAway":true,"notifySound":false,"updateChannel":"nightly"}"#;

    struct SettingsDir(PathBuf);

    impl SettingsDir {
        fn empty(tag: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("deck-settings-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            SettingsDir(dir)
        }

        /// Saved twice: the main file holds LATEST and the backup OLDER.
        fn saved_twice(tag: &str) -> Self {
            let dir = Self::empty(tag);
            save_settings_at(&dir.main(), OLDER).unwrap();
            save_settings_at(&dir.main(), LATEST).unwrap();
            dir
        }

        fn main(&self) -> PathBuf {
            self.0.join("settings.json")
        }

        fn backup(&self) -> PathBuf {
            self.0.join("settings.json.bak")
        }

        /// Every file with its bytes: equal listings mean nothing was moved,
        /// created, removed or rewritten.
        fn files(&self) -> Vec<(String, Vec<u8>)> {
            let mut files: Vec<_> = std::fs::read_dir(&self.0)
                .unwrap()
                .flatten()
                .map(|entry| {
                    (
                        entry.file_name().to_string_lossy().into_owned(),
                        std::fs::read(entry.path()).unwrap_or_default(),
                    )
                })
                .collect();
            files.sort();
            files
        }

        /// The files a recovery set aside.
        fn set_aside(&self) -> Vec<(String, Vec<u8>)> {
            self.files()
                .into_iter()
                .filter(|(name, _)| name.starts_with("settings.corrupt-"))
                .collect()
        }
    }

    impl Drop for SettingsDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// What the backend readers see: locale, editor, notify switches, channel.
    type Seen = (String, Option<String>, (bool, bool), String);

    fn seen(path: &std::path::Path) -> Option<Seen> {
        let value = settings_value_at(path)?;
        let value = Some(&value);
        Some((
            locale_from(value),
            editor_from(value),
            notify_from(value),
            update_channel_from(value),
        ))
    }

    fn older() -> Option<Seen> {
        Some((
            "zh-Hans".into(),
            Some("Zed".into()),
            (true, true),
            "nightly".into(),
        ))
    }

    fn latest() -> Option<Seen> {
        Some((
            "en".into(),
            Some("Cursor".into()),
            (true, false),
            "nightly".into(),
        ))
    }

    fn json(text: &str) -> serde_json::Value {
        serde_json::from_str(text).unwrap()
    }

    /// Normal: both doors read the main file, the backup is the previous
    /// save, and reading changes nothing on disk.
    #[test]
    fn settings_matrix_normal_reads_the_main_file_and_touches_nothing() {
        let d = SettingsDir::saved_twice("normal");
        let before = d.files();
        assert_eq!(seen(&d.main()), latest());
        let loaded = load_settings_at(&d.main()).unwrap().unwrap();
        assert_eq!((loaded.source, loaded.warning.is_none()), ("main", true));
        assert_eq!(json(&loaded.payload), json(LATEST));
        assert_eq!(d.files(), before);
        assert_eq!(
            json(&std::fs::read_to_string(d.backup()).unwrap())["data"],
            json(OLDER)
        );
        assert!(d.set_aside().is_empty());
    }

    /// Main damaged, backup good — the readers. They run before the webview
    /// at boot and on every scheduler tick: they read the backup and leave
    /// the disk exactly as they found it, however often they run.
    #[test]
    fn settings_matrix_damaged_main_readers_use_the_backup_and_move_nothing() {
        let d = SettingsDir::saved_twice("damaged-readers");
        std::fs::write(d.main(), "{damaged").unwrap();
        let before = d.files();
        for round in 0..2 {
            assert_eq!(seen(&d.main()), older(), "read {round}");
        }
        assert_eq!(d.files(), before, "a reader moved or rewrote a file");
    }

    /// Main damaged, backup good — the owner. Whoever read first, the webview
    /// learns of the recovery exactly once, keeps every setting across a
    /// restart, and its next save puts the main file back.
    #[test]
    fn settings_matrix_damaged_main_owner_recovers_tells_once_and_the_next_save_rebuilds() {
        let d = SettingsDir::saved_twice("damaged-owner");
        std::fs::write(d.main(), "{damaged").unwrap();
        let backup = std::fs::read(d.backup()).unwrap();
        assert_eq!(seen(&d.main()), older(), "a boot-time reader runs first");

        let first = load_settings_at(&d.main())
            .unwrap()
            .expect("recovered, not a first run");
        assert_eq!(first.source, "backup");
        assert_eq!(json(&first.payload), json(OLDER));
        assert_eq!(
            to_loaded(Some(first)).warning.map(|notice| notice.code),
            Some("storage.recovered"),
            "the webview is told"
        );
        assert!(!d.main().exists(), "the owner set the damaged file aside");
        let aside = d.set_aside();
        assert_eq!(aside.len(), 1);
        assert_eq!(aside[0].1, b"{damaged", "its bytes are kept");
        assert_eq!(std::fs::read(d.backup()).unwrap(), backup);

        // a restart before any save: the same settings and no second notice
        for round in 0..2 {
            let again = load_settings_at(&d.main())
                .unwrap()
                .expect("still not a first run");
            assert_eq!(again.source, "backup", "restart {round}");
            assert_eq!(json(&again.payload), json(OLDER));
            assert!(again.warning.is_none(), "told once, not on every start");
            assert_eq!(seen(&d.main()), older());
        }
        assert_eq!(d.set_aside(), aside);
        assert!(!d.main().exists(), "recovery itself never writes");

        // the user changes a setting: the main file is back, the kept bytes stay
        save_settings_at(&d.main(), LATEST).unwrap();
        let rebuilt = load_settings_at(&d.main()).unwrap().unwrap();
        assert_eq!((rebuilt.source, rebuilt.warning.is_none()), ("main", true));
        assert_eq!(seen(&d.main()), latest());
        assert_eq!(d.set_aside(), aside);
        assert_eq!(
            std::fs::read(d.backup()).unwrap(),
            backup,
            "nothing was rotated over the good copy"
        );
    }

    /// Main and backup both damaged: the readers have no answer and move
    /// nothing; the owner reports the failure once, keeps the damaged bytes,
    /// and the user can go on changing and saving settings.
    #[test]
    fn settings_matrix_both_damaged_is_reported_once_and_never_locks_settings() {
        let d = SettingsDir::saved_twice("both-damaged");
        std::fs::write(d.main(), "{damaged main").unwrap();
        std::fs::write(d.backup(), "{damaged backup").unwrap();
        let before = d.files();
        assert_eq!(seen(&d.main()), None);
        assert_eq!(d.files(), before, "a reader moved or rewrote a file");

        let failure = load_settings_at(&d.main()).unwrap_err();
        assert_eq!(
            failure.kind(),
            ErrorKind::Recovery,
            "a load failure, never a first run"
        );
        let aside = d.set_aside();
        assert_eq!(aside.len(), 1);
        assert_eq!(aside[0].1, b"{damaged main");
        assert_eq!(std::fs::read(d.backup()).unwrap(), b"{damaged backup");

        // the next start has nothing left to load and nothing new to report
        assert!(load_settings_at(&d.main()).unwrap().is_none());
        assert_eq!(seen(&d.main()), None);

        save_settings_at(&d.main(), LATEST).unwrap();
        assert_eq!(load_settings_at(&d.main()).unwrap().unwrap().source, "main");
        assert_eq!(seen(&d.main()), latest());
        assert_eq!(d.set_aside(), aside, "the damaged bytes are still there");
    }

    /// No main file. With nothing set aside this is a first run, even with a
    /// backup lying there (the user removed the file): the backup stands in
    /// only for a file a recovery moved away.
    #[test]
    fn settings_matrix_missing_main_is_a_first_run_unless_a_recovery_moved_it() {
        let d = SettingsDir::empty("missing");
        assert!(load_settings_at(&d.main()).unwrap().is_none());
        assert_eq!(seen(&d.main()), None);
        assert!(d.files().is_empty());

        let d = SettingsDir::saved_twice("missing-removed");
        std::fs::remove_file(d.main()).unwrap();
        let before = d.files();
        assert!(load_settings_at(&d.main()).unwrap().is_none());
        assert_eq!(seen(&d.main()), None);
        assert_eq!(d.files(), before);
    }

    /// Main file present but unreadable right now (permissions, descriptors,
    /// a failing disk): a reader has no answer for this call — it does not
    /// call the file damaged, move it or read around it — and reads it again
    /// as soon as it can.
    #[test]
    fn settings_matrix_unreadable_main_is_unknown_to_readers_and_moves_nothing() {
        use std::os::unix::fs::PermissionsExt;
        let d = SettingsDir::saved_twice("unreadable-readers");
        let names = |d: &SettingsDir| {
            d.files()
                .into_iter()
                .map(|(name, _)| name)
                .collect::<Vec<_>>()
        };
        let before = names(&d);
        std::fs::set_permissions(d.main(), std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read(d.main()).is_ok() {
            return; // a privileged test user reads through any mode
        }
        for round in 0..2 {
            assert_eq!(seen(&d.main()), None, "read {round}");
        }
        assert_eq!(names(&d), before, "a reader moved a file it could not read");
        std::fs::set_permissions(d.main(), std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            seen(&d.main()),
            latest(),
            "readable again: the same file, untouched"
        );
    }

    /// The owner is different: it must end up with settings the user can
    /// save. A main file it cannot read when the app starts is set aside
    /// like a damaged one and the backup stands in — waiting instead would
    /// lock a file with a lasting fault out of Settings.
    #[test]
    fn settings_matrix_unreadable_main_owner_sets_it_aside_and_uses_the_backup() {
        use std::os::unix::fs::PermissionsExt;
        let d = SettingsDir::saved_twice("unreadable-owner");
        std::fs::set_permissions(d.main(), std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read(d.main()).is_ok() {
            return; // a privileged test user reads through any mode
        }
        let loaded = load_settings_at(&d.main()).unwrap().unwrap();
        assert_eq!(loaded.source, "backup");
        assert!(loaded.warning.is_some());
        assert_eq!(json(&loaded.payload), json(OLDER));
        assert_eq!(d.set_aside().len(), 1, "kept, restricted to the user");
        save_settings_at(&d.main(), LATEST).unwrap();
        assert_eq!(seen(&d.main()), latest());
    }

    /// Written by a newer deck: refused as it is by every door — never set
    /// aside, never read around through the older backup, never saved over.
    #[test]
    fn settings_matrix_newer_schema_is_refused_untouched_by_every_door() {
        let d = SettingsDir::saved_twice("newer");
        std::fs::write(d.main(), r#"{"schema_version":99,"data":{"locale":"en"}}"#).unwrap();
        let before = d.files();
        for round in 0..2 {
            assert_eq!(seen(&d.main()), None, "read {round}");
            assert_eq!(
                load_settings_at(&d.main()).unwrap_err().kind(),
                ErrorKind::NewerSchema
            );
            assert_eq!(
                save_settings_at(&d.main(), LATEST).unwrap_err().kind(),
                ErrorKind::NewerSchema
            );
        }
        assert_eq!(d.files(), before);
    }

    /// Damaged while deck runs (the webview holds the settings in memory):
    /// the readers carry on from the backup, and the owner's next save is not
    /// refused — it sets the damaged file aside and writes what the user
    /// sees. A save deck would refuse anyway sets nothing aside.
    #[test]
    fn settings_matrix_damage_while_running_never_refuses_the_owners_save() {
        let d = SettingsDir::saved_twice("damaged-running");
        std::fs::write(d.main(), "{damaged").unwrap();
        let backup = std::fs::read(d.backup()).unwrap();
        let before = d.files();
        assert_eq!(seen(&d.main()), older());
        assert!(save_settings_at(&d.main(), r#"{"locale":"xx"}"#).is_err());
        assert_eq!(d.files(), before, "an invalid save moved or rewrote a file");

        save_settings_at(&d.main(), LATEST).unwrap();
        assert_eq!(seen(&d.main()), latest());
        let aside = d.set_aside();
        assert_eq!(aside.len(), 1);
        assert_eq!(aside[0].1, b"{damaged");
        assert_eq!(
            std::fs::read(d.backup()).unwrap(),
            backup,
            "the damaged bytes never became the backup"
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
    /// tolerates. A new key that is always written changes the fixture (the
    /// frontend test forces that) and then fails here until it is either
    /// declared in `BoardCard` or added to the tolerated list on purpose. A
    /// key written only when it has a value sits on another card of the
    /// fixture, which this test does not take apart: `tests/ipc_contract.rs`
    /// holds every key `persistence.js` can write to a declared field or a
    /// listed exception.
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

    #[test]
    fn board_inbound_plan_keeps_a_bounded_frozen_template() {
        let mut value: serde_json::Value =
            serde_json::from_str(&board(&card("s1", "P1", "C1", "deck-t-ab12"))).unwrap();
        value["cards"][0]["inboundPlan"] = serde_json::json!({
            "operationId":"B1","reviewEach":false,"initialQueued":false,
            "initialSteps":[{"operationId":"B2","text":"frozen","mode":"at","at":10,
                "tpl":"triage","tplIdx":1,"tplTotal":1}]
        });
        assert!(serde_json::from_value::<BoardDoc>(value.clone()).is_ok());
        value["cards"][0]["inboundPlan"]["initialSteps"][0]["text"] =
            serde_json::json!("x".repeat(BUFFER_MAX_ENTRY_BYTES + 1));
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
        .is_ok());
        assert!(serde_json::from_str::<BoardDoc>(
            &with_preset.replace("\"codex\"", "\"codex;zsh\"")
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
        assert!(
            serde_json::from_str::<SettingsDoc>(r#"{"notifyAway":true,"notifySound":false}"#)
                .is_ok()
        );
        assert!(serde_json::from_str::<SettingsDoc>(r#"{"notifyAway":"on"}"#).is_err());
        assert_eq!(
            notify_from(Some(
                &serde_json::json!({"notifyAway":true,"notifySound":"yes"})
            )),
            (true, false),
            "only a real true turns a flag on"
        );
        assert_eq!(notify_from(None), (false, false));
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
        assert!(serde_json::from_str::<SettingsDoc>(
            r#"{"localIntelligence":{"translation":{"targetLanguage":"zh-Hans"}}}"#
        )
        .is_ok());
        assert!(serde_json::from_str::<SettingsDoc>(
            r#"{"localIntelligence":{"translation":{"targetLanguage":"-bad"}}}"#
        )
        .is_err());
        assert!(serde_json::from_str::<SettingsDoc>(
            r#"{"localIntelligence":{"translation":{"targetLanguage":"system","sourceText":"secret"}}}"#
        ).is_err());
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

        // A load warning is a .bak recovery whatever its text says: the
        // quoted serde detail used to steer the code ("…delivery…").
        let recovered = to_loaded(Some(storage::LoadOutcome {
            payload: "{\"ok\":true}".into(),
            source: "backup",
            warning: Some("queue.json was unreadable (missing field `delivery`); recovered".into()),
        }));
        assert_eq!(recovered.source, "backup");
        assert_eq!(recovered.warning.unwrap().code, "storage.recovered");

        // Every notice drains as exactly the code its emitter named, in order,
        // and the wording of the note never matters.
        storage::take_notices();
        for kind in storage::StorageNotice::ALL {
            storage::warn(
                kind,
                "privacy hardening … interrupted deliveries … delivery".into(),
            );
        }
        let codes: Vec<&str> = storage_warnings().iter().map(|n| n.code).collect();
        let expected: Vec<&str> = storage::StorageNotice::ALL
            .iter()
            .map(|k| k.code())
            .collect();
        assert_eq!(codes, expected);
        assert_eq!(
            expected
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            expected.len(),
            "one code per notice"
        );
        assert!(storage_warnings().is_empty());
    }

    /// Every malformed buffer shape is refused with its own reason: a
    /// recovery warning names what broke without ever quoting card text.
    #[test]
    fn buffer_validation_names_each_broken_entry_shape() {
        let reason = |value: serde_json::Value| -> String {
            let buffer: CardBuffer = serde_json::from_value(value).unwrap();
            validate_buffer("s1", &buffer)
                .unwrap_err()
                .message()
                .to_owned()
        };
        let entry = |id: &str| {
            serde_json::json!({
                "id":id,"kind":"manual","text":"t","revision":1,
                "createdAt":1,"updatedAt":1,"copies":[]
            })
        };
        let too_many: Vec<_> = (0..=BUFFER_MAX_ENTRIES)
            .map(|i| entry(&format!("N{i}")))
            .collect();
        assert_eq!(
            reason(serde_json::json!({"revision":1,"entries":too_many})),
            "card s1: too many buffer entries"
        );
        assert_eq!(
            reason(serde_json::json!({"revision":1,"entries":[entry("N1"),entry("N1")]})),
            "card s1: invalid or duplicate buffer entry id"
        );
        assert_eq!(
            reason(serde_json::json!({"revision":1,"entries":[entry("bad id")]})),
            "card s1: invalid or duplicate buffer entry id"
        );

        let mut external = entry("E1");
        external["kind"] = serde_json::json!("external");
        assert_eq!(
            reason(serde_json::json!({"revision":1,"entries":[external.clone()]})),
            "card s1: invalid buffer source",
            "an external entry needs its provenance"
        );
        external["source"] = serde_json::json!({
            "type":"slack","eventId":"ev1","workspaceId":"W123","links":[]
        });
        assert_eq!(
            reason(serde_json::json!({"revision":1,"entries":[external.clone()]})),
            "card s1: invalid buffer source",
            "a Slack workspace id starts with T"
        );
        external["source"]["workspaceId"] = serde_json::json!("T123");
        external["source"]["links"] = serde_json::json!(["ftp://example.invalid"]);
        assert_eq!(
            reason(serde_json::json!({"revision":1,"entries":[external.clone()]})),
            "card s1: invalid buffer source",
            "only web links are retained"
        );
        external["source"]["links"] = serde_json::json!(["https://example.invalid/t"]);
        external["source"]["messageTs"] = serde_json::json!("1700000000.000100");
        let sound: CardBuffer =
            serde_json::from_value(serde_json::json!({"revision":1,"entries":[external]})).unwrap();
        validate_buffer("s1", &sound).unwrap();

        let mut copied = entry("N1");
        copied["copies"] = serde_json::json!([{
            "operationId":"B1","entryRevision":1,"text":"t","createdAt":1,"state":"lost"
        }]);
        assert_eq!(
            reason(serde_json::json!({"revision":1,"entries":[copied]})),
            "card s1: invalid buffer queue copy"
        );
    }

    /// Board ids are structural: blank project, column and card ids and an
    /// unbounded preset list are refused before any referential check.
    #[test]
    fn board_and_settings_validation_refuse_blank_ids_and_unbounded_lists() {
        let fail = |doc: &str, needle: &str| {
            let error = match serde_json::from_str::<BoardDoc>(doc) {
                Err(error) => error.to_string(),
                Ok(_) => panic!("{needle}: invalid document was accepted"),
            };
            assert!(error.contains(needle), "wrong error {error}");
        };
        fail(
            r#"{"projects":[{"id":" ","name":"a","columns":[{"id":"C1","name":"x"}]}],"cards":[]}"#,
            "a project has an empty id",
        );
        fail(
            r#"{"projects":[{"id":"P1","name":"a","columns":[{"id":"","name":"x"}]}],"cards":[]}"#,
            "project P1 has a column with an empty id",
        );
        let presets: Vec<String> = (0..51)
            .map(|i| {
                format!(
                    r#"{{"id":"R{i}","name":"Fix","columnId":"C1","title":"Task","dir":"~/w","cmd":"codex","steps":[]}}"#
                )
            })
            .collect();
        fail(
            &format!(
                r#"{{"projects":[{{"id":"P1","name":"a","columns":[{{"id":"C1","name":"x"}}],"presets":[{}]}}],"cards":[]}}"#,
                presets.join(",")
            ),
            "project P1 has too many task presets",
        );
        fail(
            &board(&card(" ", "P1", "C1", "deck-t-ab12")),
            "a card has an empty id",
        );

        let settings = |doc: &str| serde_json::from_str::<SettingsDoc>(doc).map(|_| ());
        let long_editor = format!(r#"{{"editor":"{}"}}"#, "e".repeat(201));
        assert!(settings(&long_editor)
            .unwrap_err()
            .to_string()
            .contains("editor name is unreasonably long"));
        let many: Vec<String> = (0..65).map(|i| format!(r#""k{i}":"Meta+KeyA""#)).collect();
        assert!(
            settings(&format!(r#"{{"shortcuts":{{{}}}}}"#, many.join(",")))
                .unwrap_err()
                .to_string()
                .contains("too many shortcut entries")
        );
        let long_binding = format!(r#"{{"shortcuts":{{"newSession":"{}"}}}}"#, "K".repeat(65));
        assert!(settings(&long_binding)
            .unwrap_err()
            .to_string()
            .contains("bounded strings"));
        assert!(settings(r#"{"inbound":[]}"#)
            .unwrap_err()
            .to_string()
            .contains("inbound must be an object"));
        settings(r#"{"inbound":{}}"#).unwrap();

        // The two documents have fixed names inside the private data
        // directory; nothing else is ever loaded as a Board or Settings.
        let dir = crate::datadir::deck_dir();
        assert_eq!(board_path(), dir.join("deck.json"));
        assert_eq!(settings_path(), dir.join("settings.json"));
    }
    /// Only the main file is the user's current Board; the backup is the
    /// save before the last one.
    #[test]
    fn a_board_loaded_from_its_backup_is_recovered_not_current() {
        assert_eq!(load_standing("main"), BoardStanding::Current);
        assert_eq!(load_standing("backup"), BoardStanding::Recovered);
        assert_eq!(load_standing("none"), BoardStanding::Recovered);
        assert_eq!(load_standing(""), BoardStanding::Recovered);
    }

    /// The committed Board answers as authority only while it is the current
    /// version: not before any load, not for a recovered Board, and again
    /// once the owner has saved. Every other reader keeps seeing it.
    #[test]
    fn the_committed_board_is_authority_only_while_current() {
        // serialize with any other commit: this test is the fence's holder
        let _fence = board_fence();
        assert!(board_fence_busy(), "a commit is made under the fence");
        let before = COMMITTED_BOARD.lock_or_recover().take();
        assert_eq!(board_authority(), None, "nothing committed: no proof");
        assert_eq!(committed_board(), None);

        let recovered =
            r#"{"projects":[{"id":"P1","presets":[{"id":"R1","firstSend":true}]}],"cards":[]}"#;
        commit_board(recovered, BoardStanding::Recovered);
        assert_eq!(board_authority(), None, "a recovered Board grants nothing");
        assert!(
            committed_board().is_some_and(|text| text.contains("R1")),
            "the other readers still see it"
        );

        let saved = r#"{"projects":[{"id":"P1","presets":[{"id":"R1"}]}],"cards":[]}"#;
        commit_board(saved, BoardStanding::Current);
        let current = board_authority().expect("the owner's save is current");
        assert!(current["projects"][0]["presets"][0]
            .get("firstSend")
            .is_none());

        // an unparsable payload commits nothing and changes no standing
        commit_board("not json", BoardStanding::Recovered);
        assert_eq!(board_authority(), Some(current));

        // a later recovery takes the standing away again, in one step
        commit_board(recovered, BoardStanding::Recovered);
        assert_eq!(board_authority(), None);
        *COMMITTED_BOARD.lock_or_recover() = before;
    }
}
