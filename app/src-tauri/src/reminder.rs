//! Card Reminder projection, never a Board writer or execution authority.
//! Committed Board intent owns identity/revision/time; UNUserNotificationCenter
//! owns scheduled delivery even after quit. Async results are version fenced.
//! Responses use a bounded durable inbox until the WebView can transact them.
//! Notes never cross the bridge. Agent identifiers/removal remain separate.
//!
//! What runs the projection: every committed Board (`observe_committed`), a
//! system wake or clock change, and the webview's reconcile. `reminder_status`
//! is therefore not a plain read: it projects and refreshes the Dock. The
//! webview reconciles at boot, on every return to its window, and every 2 s
//! while the Board has a reminder (`reminderTick` in reminder-model.js; a
//! Board without one makes no periodic call). That tick is the clock for the
//! due latch, for the Dock at a due instant, for another try at a failed
//! registration and for a response that arrives while deck is in the
//! background: `action_callback` stores the response and signals nobody.
use crate::error::{DeckError, ErrorKind};
use crate::sync::LockRecover;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

pub(crate) const NOTE_BYTES: usize = 280;
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Reminder {
    pub id: String,
    pub revision: u32,
    pub due_at: u64,
    pub time_zone: String,
    pub note: String,
    pub in_app_only: bool,
    pub due: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Claim {
    pub card_id: String,
    pub id: String,
    pub revision: u32,
}
fn invalid() -> DeckError {
    DeckError::new(ErrorKind::InvalidDoc, "invalid card reminder")
}
fn hex_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}
pub(crate) fn validate(reminder: &Reminder) -> Result<(), DeckError> {
    if !hex_id(&reminder.id)
        || reminder.revision == 0
        || reminder.revision > 1_000_000
        || reminder.due_at == 0
        || reminder.due_at > 253_402_300_799_000
        || reminder.time_zone.is_empty()
        || reminder.time_zone.len() > 64
        || !reminder
            .time_zone
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"/_+-".contains(&c))
        || reminder.note.len() > NOTE_BYTES
        || reminder.note.contains(['\r', '\n', '\0'])
    {
        return Err(invalid());
    }
    #[cfg(target_os = "macos")]
    {
        extern "C" {
            fn deck_reminder_zone_valid(raw: *const std::ffi::c_char) -> i32;
        }
        let zone = std::ffi::CString::new(reminder.time_zone.clone()).map_err(|_| invalid())?;
        // SAFETY: a NUL-terminated zone name; Foundation validates without UN.
        if unsafe { deck_reminder_zone_valid(zone.as_ptr()) } != 1 {
            return Err(invalid());
        }
    }
    Ok(())
}
pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}
pub(crate) fn request_id(card: &str, reminder: &Reminder) -> String {
    // Card IDs may contain legacy characters. Hex encoding is injective.
    let card: String = card
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!(
        "deck.reminder.{card}.{}.{revision}",
        reminder.id,
        revision = reminder.revision
    )
}
fn parse_request(id: &str) -> Option<Claim> {
    let mut parts = id.strip_prefix("deck.reminder.")?.split('.');
    let card = parts.next()?;
    let identity = parts.next()?;
    let revision = parts.next()?.parse::<u32>().ok()?;
    if parts.next().is_some()
        || card.is_empty()
        || card.len() > 1024
        || card.len() % 2 != 0
        || !hex_id(identity)
        || revision == 0
        || revision > 1_000_000
    {
        return None;
    }
    let bytes: Option<Vec<u8>> = card
        .as_bytes()
        .chunks_exact(2)
        .map(|v| {
            std::str::from_utf8(v)
                .ok()
                .and_then(|v| u8::from_str_radix(v, 16).ok())
        })
        .collect();
    Some(Claim {
        card_id: String::from_utf8(bytes?).ok()?,
        id: identity.into(),
        revision,
    })
}
/// Changes/removals require the exact revision observed by the user. Generic
/// Board callers cannot silently drop protection. The frontend transaction
/// checks this before kill; this native fence checks the committed document.
pub(crate) fn validate_changes(
    old: &serde_json::Value,
    new: &serde_json::Value,
    claims: &[Claim],
) -> Result<(), DeckError> {
    let cards = |value: &serde_json::Value| {
        value
            .get("cards")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
    };
    let old_cards = cards(old);
    let next = cards(new);
    for card in &next {
        let Some(value) = card.get("reminder") else {
            continue;
        };
        let previous = old_cards
            .iter()
            .find(|old| old.get("id") == card.get("id"))
            .and_then(|old| old.get("reminder"));
        let changed: Reminder = serde_json::from_value(value.clone()).map_err(|_| invalid())?;
        if previous.and_then(|r| r.get("dueAt")) != value.get("dueAt") && changed.due_at <= now_ms()
        {
            // A delayed genuine system response still means one hour from
            // its actual action time, even if processing resumes later.
            let valid_snooze = inbox()?.actions.iter().any(|action| {
                action.kind == "snooze"
                    && card.get("id").and_then(|v| v.as_str()) == Some(&action.card_id)
                    && changed.id == action.id
                    && changed.revision == action.revision + 1
                    && changed.due_at == action.acted_at.saturating_add(3_600_000)
            });
            if !valid_snooze {
                return Err(DeckError::new(
                    ErrorKind::InvalidDoc,
                    "reminder time must be in the future",
                ));
            }
        }
    }
    for card in cards(old) {
        let Some(reminder) = card.get("reminder") else {
            continue;
        };
        let id = card
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(invalid)?;
        let replacement = next
            .iter()
            .find(|c| c.get("id").and_then(|v| v.as_str()) == Some(id));
        if replacement.and_then(|c| c.get("reminder")) == Some(reminder) {
            continue;
        }
        let current: Reminder = serde_json::from_value(reminder.clone()).map_err(|_| invalid())?;
        if !claims.iter().any(|claim| {
            claim.card_id == id && claim.id == current.id && claim.revision == current.revision
        }) {
            return Err(DeckError::new(
                ErrorKind::InvalidDoc,
                "reminder changed: explicit current-version intent required",
            ));
        }
        if let Some(value) = replacement.and_then(|c| c.get("reminder")) {
            let changed: Reminder = serde_json::from_value(value.clone()).map_err(|_| invalid())?;
            if changed.id != current.id
                || !(changed.revision == current.revision + 1
                    || (changed.revision == current.revision
                        && !current.due
                        && changed.due
                        && Reminder {
                            due: true,
                            ..current.clone()
                        } == changed))
                || (current.due && changed.due_at == current.due_at && !changed.due)
            {
                return Err(invalid());
            }
        }
    }
    Ok(())
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Action {
    pub card_id: String,
    pub id: String,
    pub revision: u32,
    pub kind: String,
    pub acted_at: u64,
    pub request: String,
}
#[derive(Default, Deserialize, Serialize)]
#[serde(try_from = "InboxRaw")]
struct Inbox {
    actions: Vec<Action>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InboxRaw {
    actions: Vec<Action>,
}
impl TryFrom<InboxRaw> for Inbox {
    type Error = DeckError;
    fn try_from(raw: InboxRaw) -> Result<Self, Self::Error> {
        if raw.actions.len() > 128
            || raw.actions.iter().any(|a| {
                !matches!(a.kind.as_str(), "open" | "snooze")
                    || parse_request(&a.request).is_none_or(|claim| {
                        claim.card_id != a.card_id
                            || claim.id != a.id
                            || claim.revision != a.revision
                    })
            })
        {
            return Err(invalid());
        }
        Ok(Self {
            actions: raw.actions,
        })
    }
}
static INBOX: Mutex<()> = Mutex::new(());
static ACTION_FAILURE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static WEBVIEW_READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static BACKGROUND_LAUNCH: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static RESULTS: LazyLock<Mutex<HashMap<String, &'static str>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static PROJECT: Mutex<()> = Mutex::new(());
static COMMITTED: Mutex<Option<serde_json::Value>> = Mutex::new(None);
type BadgeCards = Vec<(String, String, u64, bool)>;
static BADGE: LazyLock<Mutex<BadgeCards>> = LazyLock::new(|| Mutex::new(Vec::new()));
pub(crate) fn badge_keys(
    sessions: impl Iterator<Item = String>,
) -> std::collections::HashSet<String> {
    let cards = BADGE.lock_or_recover();
    let mut keys: std::collections::HashSet<String> = cards
        .iter()
        .filter(|(_, _, time, due)| *due || *time <= now_ms())
        .map(|(id, _, _, _)| id.clone())
        .collect();
    for session in sessions {
        keys.insert(
            cards
                .iter()
                .find(|(_, s, _, _)| *s == session)
                .map(|(id, _, _, _)| id.clone())
                .unwrap_or(session),
        );
    }
    keys
}
fn inbox_path() -> std::path::PathBuf {
    crate::datadir::deck_dir().join("reminder-actions.json")
}
fn inbox() -> Result<Inbox, DeckError> {
    let loaded = crate::storage::load_typed::<Inbox>(&inbox_path())?;
    loaded
        .map(|doc| serde_json::from_str(&doc.payload).map_err(|_| invalid()))
        .unwrap_or_else(|| Ok(Inbox::default()))
}
#[cfg(target_os = "macos")]
extern "C" {
    fn deck_reminder_init(
        action: extern "C" fn(*const std::ffi::c_char, i32, u64),
        result: extern "C" fn(*const std::ffi::c_char, i32),
    );
    fn deck_reminder_project(json: *const std::ffi::c_char);
}
#[cfg(target_os = "macos")]
extern "C" fn action_callback(raw: *const std::ffi::c_char, kind: i32, acted_at: u64) {
    if raw.is_null() || !matches!(kind, 1 | 2) {
        return;
    }
    // SAFETY: bridge lends a NUL-terminated identifier for this call.
    let request = unsafe { std::ffi::CStr::from_ptr(raw) }
        .to_string_lossy()
        .into_owned();
    let Some(claim) = parse_request(&request) else {
        return;
    };
    if kind == 2 && !WEBVIEW_READY.load(std::sync::atomic::Ordering::Acquire) {
        BACKGROUND_LAUNCH.store(true, std::sync::atomic::Ordering::Release);
    }
    let _lock = INBOX.lock_or_recover();
    let Ok(mut inbox) = inbox() else {
        ACTION_FAILURE.store(true, std::sync::atomic::Ordering::Release);
        return;
    };
    if inbox
        .actions
        .iter()
        .any(|a| a.request == request && a.kind == if kind == 1 { "open" } else { "snooze" })
    {
        return;
    }
    if inbox.actions.len() >= 128 {
        ACTION_FAILURE.store(true, std::sync::atomic::Ordering::Release);
        return;
    }
    inbox.actions.push(Action {
        card_id: claim.card_id,
        id: claim.id,
        revision: claim.revision,
        kind: if kind == 1 { "open" } else { "snooze" }.into(),
        acted_at,
        request,
    });
    if let Ok(json) = serde_json::to_string(&inbox) {
        if crate::storage::save_typed::<Inbox>(&inbox_path(), &json).is_err() {
            ACTION_FAILURE.store(true, std::sync::atomic::Ordering::Release);
        } else if cfg!(debug_assertions) && crate::smoke_faults::enabled() {
            // Preserve actual system callbacks through ACK for isolated evidence.
            // No response injection seam; only this native delegate writes it.
            let directory = crate::datadir::deck_dir().join("evidence");
            if crate::datadir::create_private_dir(&directory).is_ok() {
                let path = directory.join("reminder-native-actions.json");
                let mut values: Vec<Action> = std::fs::read(&path)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                    .unwrap_or_default();
                if let Some(action) = inbox.actions.last() {
                    values.push(action.clone());
                }
                if values.len() <= 128 {
                    if let Ok(bytes) = serde_json::to_vec(&values) {
                        let _ = crate::datadir::write_private(&path, &bytes);
                    }
                }
            }
        }
    }
}
#[cfg(target_os = "macos")]
extern "C" fn result_callback(raw: *const std::ffi::c_char, code: i32) {
    if code == 3 {
        reconcile();
        return;
    }
    if raw.is_null() {
        return;
    }
    // SAFETY: bridge lends a NUL-terminated identifier for this call.
    let request = unsafe { std::ffi::CStr::from_ptr(raw) }
        .to_string_lossy()
        .into_owned();
    if let Some(value) = RESULTS.lock_or_recover().get_mut(&request) {
        *value = if code == 1 {
            "scheduled"
        } else {
            "registration-failed"
        };
    }
}
pub(crate) fn init() {
    #[cfg(target_os = "macos")]
    // SAFETY: static callback functions live for the entire process.
    unsafe {
        deck_reminder_init(action_callback, result_callback);
    }
}

/// Only the authoritative load/save door supplies this read-only mirror.
/// Projection must never run storage recovery ahead of the Board loader.
pub(crate) fn observe_committed(payload: &str) {
    if let Ok(board) = serde_json::from_str(payload) {
        {
            let _fence = PROJECT.lock_or_recover();
            *COMMITTED.lock_or_recover() = Some(board);
        }
        reconcile();
    }
}
pub(crate) fn committed_payload() -> Option<String> {
    COMMITTED
        .lock_or_recover()
        .as_ref()
        .and_then(|board| serde_json::to_string(board).ok())
}
pub(crate) fn reconcile() {
    let _lock = PROJECT.lock_or_recover();
    let Some(board) = COMMITTED.lock_or_recover().clone() else {
        return;
    };
    let now = now_ms();
    let mut projection = Vec::new();
    let mut badges = Vec::new();
    for card in board
        .get("cards")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
    {
        if let (Some(id), Some(session)) = (
            card.get("id").and_then(|v| v.as_str()),
            card.get("session").and_then(|v| v.as_str()),
        ) {
            badges.push((
                id.to_string(),
                session.to_string(),
                card.get("reminder")
                    .and_then(|r| r.get("dueAt"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(u64::MAX),
                card.get("reminder")
                    .and_then(|r| r.get("due"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
            ));
        }
    }
    {
        let mut previous = BADGE.lock_or_recover();
        let current = badges.clone();
        for (id, session, _, _) in &current {
            if let Some((_, old_session, _, _)) = previous
                .iter()
                .find(|(old_id, old_session, _, _)| old_id == id && old_session != session)
            {
                badges.push((id.clone(), old_session.clone(), u64::MAX, false));
            }
        }
        *previous = badges;
    }
    let mut results = RESULTS.lock_or_recover();
    let mut current = HashMap::new();
    for card in board
        .get("cards")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
    {
        let (Some(id), Some(value)) = (
            card.get("id").and_then(|v| v.as_str()),
            card.get("reminder"),
        ) else {
            continue;
        };
        let Ok(reminder) = serde_json::from_value::<Reminder>(value.clone()) else {
            continue;
        };
        let request = request_id(id, &reminder);
        current.insert(request.clone(), *results.get(&request).unwrap_or(&"saved"));
        if reminder.in_app_only {
            continue;
        }
        let project = board
            .get("projects")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .find(|p| p.get("id") == card.get("projectId"))
            .and_then(|p| p.get("name"))
            .cloned()
            .unwrap_or_default();
        projection.push(serde_json::json!({"identifier":request,"title":card.get("title"),"project":project,"dueAt":reminder.due_at,"due":reminder.due || reminder.due_at <= now,"sound":crate::notify::reminder_sound(),"locale":crate::documents::locale_setting()}));
    }
    *results = current;
    drop(results);
    #[cfg(target_os = "macos")]
    if let Ok(json) = std::ffi::CString::new(serde_json::to_string(&projection).unwrap_or_default())
    {
        // SAFETY: the bridge copies JSON before returning; it contains labels
        // and closed scheduling fields, never reminder notes or terminal data.
        unsafe {
            deck_reminder_project(json.as_ptr());
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = projection;
    crate::notify::refresh_badge();
}
#[tauri::command]
pub(crate) fn reminder_status() -> HashMap<String, &'static str> {
    reconcile();
    RESULTS.lock_or_recover().clone()
}
#[tauri::command]
pub(crate) fn reminder_actions() -> Result<Vec<Action>, DeckError> {
    if ACTION_FAILURE.swap(false, std::sync::atomic::Ordering::AcqRel) {
        return Err(DeckError::new(
            ErrorKind::Other,
            "reminder notification action was not saved",
        ));
    }
    let _lock = INBOX.lock_or_recover();
    Ok(inbox()?.actions)
}
#[tauri::command]
pub(crate) fn reminder_ack(request: String, kind: String) -> Result<(), DeckError> {
    if parse_request(&request).is_none() || !matches!(kind.as_str(), "open" | "snooze") {
        return Err(invalid());
    }
    let _lock = INBOX.lock_or_recover();
    let mut inbox = inbox()?;
    if kind == "snooze" {
        BACKGROUND_LAUNCH.store(false, std::sync::atomic::Ordering::Release);
    }
    inbox
        .actions
        .retain(|a| a.request != request || a.kind != kind);
    crate::storage::save_typed::<Inbox>(
        &inbox_path(),
        &serde_json::to_string(&inbox).map_err(|_| invalid())?,
    )
}
#[tauri::command]
pub(crate) fn reminder_show(app: tauri::AppHandle) {
    use tauri::Manager;
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[tauri::command]
pub(crate) fn reminder_request_permission() -> String {
    crate::notify::reminder_request_permission()
}

pub(crate) fn background_launch() -> bool {
    BACKGROUND_LAUNCH.load(std::sync::atomic::Ordering::Acquire)
}
#[tauri::command]
pub(crate) fn reminder_launch_visible() -> bool {
    WEBVIEW_READY.store(true, std::sync::atomic::Ordering::Release);
    !BACKGROUND_LAUNCH.load(std::sync::atomic::Ordering::Acquire)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> Reminder {
        Reminder {
            id: "0123456789abcdef0123456789abcdef".into(),
            revision: 1,
            due_at: 123,
            time_zone: "Asia/Tokyo".into(),
            note: "private note".into(),
            in_app_only: false,
            due: false,
        }
    }
    #[test]
    fn identity_round_trips_without_widening_session_names() {
        let r = sample();
        let id = request_id("card.☃", &r);
        let c = parse_request(&id).unwrap();
        assert_eq!(c.card_id, "card.☃");
        assert_eq!(c.revision, 1);
        for bad in [
            "session",
            "deck.reminder.gg.0123456789abcdef0123456789abcdef.1",
            "deck.reminder.61.0123456789abcdef0123456789abcdef.0",
        ] {
            assert!(parse_request(bad).is_none());
        }
    }
    #[test]
    fn old_confirmation_cannot_cancel_a_new_revision() {
        let r = sample();
        let old = serde_json::json!({"cards":[{"id":"a","reminder":r}]});
        let new = serde_json::json!({"cards":[]});
        assert!(validate_changes(&old, &new, &[]).is_err());
        let mut claim = Claim {
            card_id: "a".into(),
            id: sample().id,
            revision: 2,
        };
        assert!(validate_changes(&old, &new, &[claim.clone()]).is_err());
        claim.revision = 1;
        assert!(validate_changes(&old, &new, &[claim]).is_ok());
    }
    #[test]
    fn notes_are_bounded_single_line_and_not_projection_content() {
        let mut r = sample();
        assert!(validate(&r).is_ok());
        r.note = "x".repeat(NOTE_BYTES + 1);
        assert!(validate(&r).is_err());
        r.note = "line\nbreak".into();
        assert!(validate(&r).is_err());
    }
}
