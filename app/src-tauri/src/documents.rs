//! Typed board/settings documents: `BoardDoc` and `SettingsDoc` validate
//! business structure via `try_from` (the SAME rules on load and save), and
//! the load/save commands plus the settings readers other modules use.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

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
    type Error = String;
    fn try_from(raw: BoardDocRaw) -> Result<Self, String> {
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
    /// runtime fields the UI cannot operate a card without
    #[allow(dead_code)]
    cmd: String,
    #[allow(dead_code)]
    dir: String,
    session: String,
}

/// The referential rules a usable board must satisfy. Errors carry ids
/// (deck-generated), never titles/commands/paths — they end up in recovery
/// warnings.
fn validate_board(b: &BoardDocRaw) -> Result<(), String> {
    let mut project_ids = HashSet::new();
    for p in &b.projects {
        if p.id.trim().is_empty() {
            return Err("a project has an empty id".into());
        }
        if !project_ids.insert(p.id.as_str()) {
            return Err(format!("duplicate project id {}", p.id));
        }
        if p.columns.is_empty() {
            return Err(format!("project {} has no columns", p.id));
        }
        let mut col_ids = HashSet::new();
        for c in &p.columns {
            if c.id.trim().is_empty() {
                return Err(format!("project {} has a column with an empty id", p.id));
            }
            if !col_ids.insert(c.id.as_str()) {
                return Err(format!("duplicate column id {} in project {}", c.id, p.id));
            }
        }
    }
    let mut card_ids = HashSet::new();
    let mut sessions = HashSet::new();
    for c in &b.cards {
        if c.id.trim().is_empty() {
            return Err("a card has an empty id".into());
        }
        if !card_ids.insert(c.id.as_str()) {
            return Err(format!("duplicate card id {}", c.id));
        }
        // the SAME session-name rule the runtime enforces on start/attach
        crate::tmux::validate_session_name(&c.session)
            .map_err(|e| format!("card {}: {e}", c.id))?;
        if !sessions.insert(c.session.as_str()) {
            return Err(format!("card {}: session name is already used", c.id));
        }
        let Some(project) = b.projects.iter().find(|p| p.id == c.project_id) else {
            return Err(format!("card {} references a missing project", c.id));
        };
        if !project.columns.iter().any(|col| col.id == c.column_id) {
            return Err(format!(
                "card {} references a column that is not in its project",
                c.id
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

#[derive(serde::Deserialize)]
pub(crate) struct SettingsDocRaw {
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
    type Error = String;
    fn try_from(raw: SettingsDocRaw) -> Result<Self, String> {
        if let Some(e) = &raw.editor {
            if e.len() > 200 {
                return Err("editor name is unreasonably long".into());
            }
        }
        if let Some(locale) = &raw.locale {
            if !matches!(locale.as_str(), "system" | "en" | "zh-Hans") {
                return Err("locale must be system, en, or zh-Hans".into());
            }
        }
        if let Some(theme) = &raw.theme {
            if !matches!(
                theme.as_str(),
                "deck-dark" | "light" | "system" | "high-contrast"
            ) {
                return Err("theme must be deck-dark, light, system, or high-contrast".into());
            }
        }
        if let Some(accent) = &raw.accent {
            if !matches!(accent.as_str(), "teal" | "blue" | "purple" | "orange") {
                return Err("accent must be teal, blue, purple, or orange".into());
            }
        }
        if let Some(scale) = raw.font_scale {
            if !scale.is_finite() || !(0.5..=1.6).contains(&scale) {
                return Err("fontScale must be between 0.5 and 1.6".into());
            }
        }
        if let Some(shortcuts) = &raw.shortcuts {
            if shortcuts.len() > 64 {
                return Err("too many shortcut entries".into());
            }
            if shortcuts
                .iter()
                .any(|(key, value)| key.is_empty() || key.len() > 64 || value.len() > 64)
            {
                return Err("shortcut names and bindings must be bounded strings".into());
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
    storage::deck_dir().join("deck.json")
}

#[tauri::command]
pub(crate) fn load_board() -> Result<LoadedDoc, String> {
    Ok(to_loaded(storage::load_typed::<BoardDoc>(&board_path())?))
}

/// The same full business validation as load, BEFORE anything touches disk:
/// an invalid document never overwrites the main file or rotates the .bak.
pub(crate) fn save_validated<T: serde::de::DeserializeOwned>(
    path: &std::path::Path,
    data: &str,
    what: &str,
) -> Result<(), String> {
    serde_json::from_str::<T>(data).map_err(|e| format!("refusing to save invalid {what}: {e}"))?;
    storage::save_typed::<T>(path, data)
}

#[tauri::command]
pub(crate) fn save_board(data: String) -> Result<(), String> {
    if crate::smoke_faults::take("board-save") {
        return Err("injected board save failure".into());
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
    storage::deck_dir().join("settings.json")
}

#[tauri::command]
pub(crate) fn load_settings() -> Result<LoadedDoc, String> {
    Ok(to_loaded(storage::load_typed::<SettingsDoc>(
        &settings_path(),
    )?))
}

#[tauri::command]
pub(crate) fn save_settings(data: String) -> Result<(), String> {
    if crate::smoke_faults::take("settings-save") {
        return Err("injected settings save failure".into());
    }
    validate_saved_update_channel(&data)?;
    save_validated::<SettingsDoc>(&settings_path(), &data, "settings")
}

fn validate_saved_update_channel(data: &str) -> Result<(), String> {
    let value: serde_json::Value =
        serde_json::from_str(data).map_err(|_| "settings must be valid JSON".to_string())?;
    match value.get("updateChannel") {
        None => Ok(()),
        Some(serde_json::Value::String(channel))
            if matches!(channel.as_str(), "stable" | "nightly") =>
        {
            Ok(())
        }
        Some(_) => Err("updateChannel must be stable or nightly".into()),
    }
}

pub(crate) fn editor_app() -> Option<String> {
    let raw = storage::load_typed::<SettingsDoc>(&settings_path())
        .ok()??
        .payload;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let e = v.get("editor")?.as_str()?.trim().to_string();
    if e.is_empty() {
        None
    } else {
        Some(e)
    }
}

pub(crate) fn locale_setting() -> String {
    let raw = storage::load_typed::<SettingsDoc>(&settings_path())
        .ok()
        .flatten()
        .map(|o| o.payload);
    raw.and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("locale")?.as_str().map(str::to_owned))
        .filter(|v| matches!(v.as_str(), "system" | "en" | "zh-Hans"))
        .unwrap_or_else(|| "system".into())
}

pub(crate) fn update_channel_setting() -> String {
    let raw = storage::load_typed::<SettingsDoc>(&settings_path())
        .ok()
        .flatten()
        .map(|outcome| outcome.payload);
    raw.and_then(|value| serde_json::from_str::<serde_json::Value>(&value).ok())
        .and_then(|value| value.get("updateChannel")?.as_str().map(str::to_owned))
        .filter(|value| matches!(value.as_str(), "stable" | "nightly"))
        .unwrap_or_else(|| "stable".into())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(err.contains("refusing to save"), "{err}");
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
