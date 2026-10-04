//! Card Reminder projection, never a Board writer or execution authority.
//! Committed Board intent owns identity/revision/time; UNUserNotificationCenter
//! owns scheduled delivery even after quit. Async results are version fenced.
//! Responses use a bounded durable inbox until the WebView can transact them.
//! Notes never cross the bridge. Agent identifiers/removal remain separate.
//!
//! What runs the projection: every committed Board (`observe_committed`, the
//! observer `init` registers with the Board door), a system wake or clock
//! change, and the webview's reconcile. `reminder_status`
//! is therefore not a plain read: it projects and refreshes the Dock. The
//! webview reconciles at boot, on every return to its window, and every 2 s
//! while the Board has a reminder (`reminderTick` in reminder-model.js; a
//! Board without one makes no periodic call). That tick is the clock for the
//! due latch, for the Dock at a due instant, for another try at a failed
//! registration and for a response that arrives while deck is in the
//! background: `action_callback` stores the response and signals nobody.
//!
//! A launch the system made to deliver "remind in 1 hour" while deck was not
//! running is RESPONSE-ONLY (`LAUNCH`): a Snooze that arrives before the
//! webview has asked `reminder_launch_visible`. The user answered "not now"
//! and did not ask for deck, so that launch does one thing. The window stays
//! unrevealed, the app is hidden at once so the application in use keeps the
//! keyboard, and what `main.rs` handed to `defer_automatic_work` (scheduler,
//! inbound sources, Slack transport, Connector, MCP) does not start. The
//! webview transacts the answer and calls `reminder_response_finish`, which
//! ends the process only when the inbox is empty and the system has confirmed
//! the request of every Snooze it transacted (`finish_decision`). Everything
//! else turns the launch into an ordinary, visible one (`stay`): the user
//! asked for deck meanwhile (Dock, or a click on a banner), the answer could
//! not be saved or registered, or the webview did not finish within
//! `RESPONSE_ONLY_LIMIT`. No path leaves deck running without a window. An
//! ordinary launch starts the deferred work when the webview asks, and a
//! Snooze answered while deck runs changes nothing here.
use crate::applog::applog;
use crate::error::{DeckError, ErrorKind};
use crate::sync::LockRecover;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant};

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
static ACTION_FAILURE: AtomicBool = AtomicBool::new(false);
/// What this launch is. Only an undecided launch can become response-only
/// (an early Snooze) or ordinary (the webview asked first, or anything else
/// arrived first); a response-only launch then ends or becomes ordinary.
static LAUNCH: AtomicU8 = AtomicU8::new(UNDECIDED);
const UNDECIDED: u8 = 0;
const ORDINARY: u8 = 1;
const RESPONSE_ONLY: u8 = 2;
const ENDING: u8 = 3;
/// The user asked for deck (Dock icon) during a response-only launch.
static REOPENED: AtomicBool = AtomicBool::new(false);
/// A notification answer could not be stored in this launch. Unlike
/// `ACTION_FAILURE`, which the webview consumes to say so once, this stays:
/// a launch that lost an answer never ends as if it had transacted it.
static ANSWER_LOST: AtomicBool = AtomicBool::new(false);
/// Requests the Snoozes transacted by a response-only launch must see
/// registered before the process may end.
static EXPECTED: Mutex<Vec<String>> = Mutex::new(Vec::new());
type Deferred = Box<dyn FnOnce() + Send>;
static DEFERRED: Mutex<Option<Deferred>> = Mutex::new(None);
static APP: OnceLock<tauri::AppHandle> = OnceLock::new();
/// How long `reminder_response_finish` waits for the system to confirm a
/// registration, and how long a response-only launch may last at all. The
/// second is below the point where the system suspends a webview whose
/// window was never shown (about 7.5 s, measured 2026-10-04).
const REGISTRATION_WAIT: Duration = Duration::from_secs(4);
const RESPONSE_ONLY_LIMIT: Duration = Duration::from_secs(6);
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
fn answer_not_saved() {
    ACTION_FAILURE.store(true, Ordering::Release);
    ANSWER_LOST.store(true, Ordering::Release);
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
    if response_arrived(kind) {
        response_only_began();
    }
    let _lock = INBOX.lock_or_recover();
    let Ok(mut inbox) = inbox() else {
        answer_not_saved();
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
        answer_not_saved();
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
            answer_not_saved();
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
    // every Board the door commits is projected (documents.rs `commit_board`)
    crate::documents::set_commit_observer(observe_committed);
    #[cfg(target_os = "macos")]
    // SAFETY: static callback functions live for the entire process.
    unsafe {
        deck_reminder_init(action_callback, result_callback);
    }
}

/// The Board door's commit observer. Only that authoritative load/save door
/// supplies this copy, the projection's input; the door keeps its own and
/// never reads this one. Projection must never run storage recovery ahead of
/// the Board loader.
fn observe_committed(payload: &str) {
    if let Ok(board) = serde_json::from_str(payload) {
        {
            let _fence = PROJECT.lock_or_recover();
            *COMMITTED.lock_or_recover() = Some(board);
        }
        reconcile();
    }
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
    if ACTION_FAILURE.swap(false, Ordering::AcqRel) {
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
    inbox
        .actions
        .retain(|a| a.request != request || a.kind != kind);
    crate::storage::save_typed::<Inbox>(
        &inbox_path(),
        &serde_json::to_string(&inbox).map_err(|_| invalid())?,
    )?;
    expect_registration(&request, &kind);
    Ok(())
}
/// The webview locates the card of an Open. That is the user asking for
/// deck: a response-only launch becomes an ordinary one first.
#[tauri::command]
pub(crate) fn reminder_show(app: tauri::AppHandle) {
    stay("opened");
    show_window(&app);
}
fn show_window(app: &tauri::AppHandle) {
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

/// Deck's automatic work waits here until the launch is an ordinary one
/// (`main.rs` setup). A response-only launch that ends never runs it.
pub(crate) fn defer_automatic_work(app: tauri::AppHandle, start: impl FnOnce() + Send + 'static) {
    let _ = APP.set(app);
    defer_start(start);
}
fn defer_start(start: impl FnOnce() + Send + 'static) {
    *DEFERRED.lock_or_recover() = Some(Box::new(start));
}
fn start_deferred() {
    let start = DEFERRED.lock_or_recover().take();
    if let Some(start) = start {
        start();
    }
}
/// A notification answer reached the delegate. True when it makes this
/// launch response-only: a Snooze, before anything else decided the launch.
fn response_arrived(kind: i32) -> bool {
    let to = if kind == 2 { RESPONSE_ONLY } else { ORDINARY };
    LAUNCH
        .compare_exchange(UNDECIDED, to, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
        && to == RESPONSE_ONLY
}
/// The system made deck the frontmost application when it launched it.
/// Hiding hands the keyboard back to the application the user is in; the
/// bound below makes sure the launch never lingers without a window.
fn response_only_began() {
    applog("[reminder] response-only launch");
    if let Some(app) = APP.get() {
        #[cfg(target_os = "macos")]
        let _ = app.hide();
        let _ = app;
    }
    std::thread::spawn(|| {
        std::thread::sleep(RESPONSE_ONLY_LIMIT);
        stay("unfinished");
    });
}
/// A response-only launch becomes an ordinary one, shown. False when the
/// launch is not response-only (any more).
fn stay(reason: &'static str) -> bool {
    if LAUNCH
        .compare_exchange(RESPONSE_ONLY, ORDINARY, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return false;
    }
    applog(match reason {
        "opened" => "[reminder] response-only launch stays: a card was opened",
        "reopened" => "[reminder] response-only launch stays: deck was asked for",
        "answer-pending" => "[reminder] response-only launch stays: the answer is not transacted",
        "registration-failed" => "[reminder] response-only launch stays: registration failed",
        "registration-unconfirmed" => {
            "[reminder] response-only launch stays: registration not confirmed"
        }
        _ => "[reminder] response-only launch stays: not finished in time",
    });
    start_deferred();
    if let Some(app) = APP.get() {
        show_window(app);
    }
    true
}
/// The Dock icon was clicked while the launch is response-only: the user
/// wants deck. The finish turns the launch into an ordinary one.
pub(crate) fn reopen_requested() {
    REOPENED.store(true, Ordering::Release);
}
/// The Snooze just acknowledged was transacted by a response-only launch:
/// its next revision's request is what the system must confirm before the
/// process may end.
fn expect_registration(request: &str, kind: &str) {
    if kind != "snooze" || LAUNCH.load(Ordering::Acquire) != RESPONSE_ONLY {
        return;
    }
    let (Some(claim), Some((head, _))) = (parse_request(request), request.rsplit_once('.')) else {
        return;
    };
    EXPECTED
        .lock_or_recover()
        .push(format!("{head}.{}", claim.revision + 1));
}
/// What the projection last recorded for each expected request; `None` for
/// one the committed Board no longer has (the answer changed nothing).
fn expected_states() -> Vec<Option<&'static str>> {
    let results = RESULTS.lock_or_recover();
    EXPECTED
        .lock_or_recover()
        .iter()
        .map(|request| results.get(request).copied())
        .collect()
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Finish {
    Wait,
    End,
    Stay(&'static str),
}
struct FinishFacts {
    response_only: bool,
    reopened: bool,
    failed: bool,
    inbox_empty: bool,
    registrations: Vec<Option<&'static str>>,
    waited: bool,
}
/// The process ends only on positive proof that the answer is done: nothing
/// left in the inbox, nothing failed, and every Snooze this launch transacted
/// has its new request registered. The user asking for deck outranks all.
fn finish_decision(facts: &FinishFacts) -> Finish {
    if !facts.response_only {
        return Finish::Stay("ordinary");
    }
    if facts.reopened {
        return Finish::Stay("reopened");
    }
    if facts.failed || !facts.inbox_empty {
        return Finish::Stay("answer-pending");
    }
    if facts.registrations.contains(&Some("registration-failed")) {
        return Finish::Stay("registration-failed");
    }
    if facts.registrations.contains(&Some("saved")) {
        return if facts.waited {
            Finish::Stay("registration-unconfirmed")
        } else {
            Finish::Wait
        };
    }
    Finish::End
}
fn finish_facts(waited: bool) -> FinishFacts {
    let inbox_empty = {
        let _lock = INBOX.lock_or_recover();
        inbox().is_ok_and(|inbox| inbox.actions.is_empty())
    };
    FinishFacts {
        response_only: LAUNCH.load(Ordering::Acquire) == RESPONSE_ONLY,
        reopened: REOPENED.load(Ordering::Acquire),
        failed: ANSWER_LOST.load(Ordering::Acquire),
        inbox_empty,
        registrations: expected_states(),
        waited,
    }
}
/// The webview transacted what a response-only launch was made for. False:
/// the process ends. True: deck stays as an ordinary launch and the boot
/// goes on. A bounded wait covers the system's asynchronous registration.
#[tauri::command]
pub(crate) async fn reminder_response_finish(app: tauri::AppHandle) -> bool {
    tauri::async_runtime::spawn_blocking(move || {
        let deadline = Instant::now() + REGISTRATION_WAIT;
        loop {
            match finish_decision(&finish_facts(Instant::now() >= deadline)) {
                Finish::Wait => std::thread::sleep(Duration::from_millis(50)),
                Finish::Stay(reason) => {
                    stay(reason);
                    return true;
                }
                Finish::End => {
                    let ended = LAUNCH.compare_exchange(
                        RESPONSE_ONLY,
                        ENDING,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    );
                    if ended.is_ok() {
                        applog("[reminder] response-only launch: answer transacted, exiting");
                        app.exit(0);
                    }
                    return ended.is_err();
                }
            }
        }
    })
    .await
    .unwrap_or(true)
}
/// The launch is response-only (or ending): the Dock's Reopen must not
/// reveal the window by itself (`main.rs`).
pub(crate) fn background_launch() -> bool {
    matches!(LAUNCH.load(Ordering::Acquire), RESPONSE_ONLY | ENDING)
}
/// The webview asks whether to reveal its window. An undecided launch is an
/// ordinary one from here on, and what waited for that starts.
#[tauri::command]
pub(crate) fn reminder_launch_visible() -> bool {
    let _ = LAUNCH.compare_exchange(UNDECIDED, ORDINARY, Ordering::AcqRel, Ordering::Acquire);
    let visible = LAUNCH.load(Ordering::Acquire) == ORDINARY;
    if visible {
        start_deferred();
    }
    visible
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

    // ---- a launch the system made for a notification answer ----
    static LAUNCH_TESTS: Mutex<()> = Mutex::new(());
    /// The launch state is process-wide: one test at a time, from a fresh one.
    fn fresh_launch() -> std::sync::MutexGuard<'static, ()> {
        let guard = LAUNCH_TESTS.lock_or_recover();
        LAUNCH.store(UNDECIDED, Ordering::Release);
        REOPENED.store(false, Ordering::Release);
        ANSWER_LOST.store(false, Ordering::Release);
        EXPECTED.lock_or_recover().clear();
        *DEFERRED.lock_or_recover() = None;
        guard
    }
    fn counted() -> (
        std::sync::Arc<std::sync::atomic::AtomicUsize>,
        impl FnOnce() + Send + 'static,
    ) {
        let runs = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = runs.clone();
        (runs, move || {
            seen.fetch_add(1, Ordering::SeqCst);
        })
    }
    #[test]
    fn an_early_snooze_makes_the_launch_response_only_and_starts_nothing() {
        let _serial = fresh_launch();
        let (runs, start) = counted();
        defer_start(start);
        assert!(response_arrived(2));
        assert!(!reminder_launch_visible(), "the window stays unrevealed");
        assert!(background_launch());
        assert_eq!(runs.load(Ordering::SeqCst), 0, "no automatic work starts");
        // a second early Snooze belongs to the same launch
        assert!(!response_arrived(2));
        assert!(background_launch());
    }
    #[test]
    fn an_ordinary_launch_starts_the_deferred_work_exactly_once() {
        let _serial = fresh_launch();
        let (runs, start) = counted();
        defer_start(start);
        assert!(reminder_launch_visible());
        assert!(reminder_launch_visible());
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        // a Snooze answered once deck is up changes nothing about the launch
        assert!(!response_arrived(2));
        assert!(!background_launch());
        assert!(reminder_launch_visible());
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn a_click_on_the_banner_itself_is_an_ordinary_launch() {
        let _serial = fresh_launch();
        let (runs, start) = counted();
        defer_start(start);
        assert!(!response_arrived(1));
        // a Snooze after it no longer makes the launch response-only
        assert!(!response_arrived(2));
        assert!(!background_launch());
        assert!(reminder_launch_visible());
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn staying_starts_the_deferred_work_and_ends_the_response_only_launch() {
        let _serial = fresh_launch();
        let (runs, start) = counted();
        defer_start(start);
        assert!(response_arrived(2));
        assert!(!reminder_launch_visible());
        assert!(stay("reopened"));
        assert_eq!(runs.load(Ordering::SeqCst), 1, "staying itself starts it");
        assert!(!background_launch());
        assert!(reminder_launch_visible());
        assert!(!stay("reopened"), "only a response-only launch is turned");
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn an_ordinary_launch_cannot_be_ended_as_response_only() {
        let _serial = fresh_launch();
        assert!(reminder_launch_visible());
        assert!(!stay("unfinished"));
        assert_eq!(
            finish_decision(&finish_facts(false)),
            Finish::Stay("ordinary")
        );
    }
    #[test]
    fn the_launch_ends_only_when_the_answer_is_transacted_and_registered() {
        let decide = |change: fn(&mut FinishFacts)| {
            let mut facts = FinishFacts {
                response_only: true,
                reopened: false,
                failed: false,
                inbox_empty: true,
                registrations: vec![Some("scheduled")],
                waited: false,
            };
            change(&mut facts);
            finish_decision(&facts)
        };
        assert_eq!(decide(|_| {}), Finish::End);
        // an answer that no longer applied to anything registers nothing
        assert_eq!(decide(|f| f.registrations.clear()), Finish::End);
        assert_eq!(decide(|f| f.registrations = vec![None]), Finish::End);
        assert_eq!(
            decide(|f| f.response_only = false),
            Finish::Stay("ordinary")
        );
        assert_eq!(decide(|f| f.reopened = true), Finish::Stay("reopened"));
        assert_eq!(decide(|f| f.failed = true), Finish::Stay("answer-pending"));
        assert_eq!(
            decide(|f| f.inbox_empty = false),
            Finish::Stay("answer-pending")
        );
        assert_eq!(
            decide(|f| f.registrations = vec![Some("scheduled"), Some("registration-failed")]),
            Finish::Stay("registration-failed")
        );
        // not confirmed yet: wait, and when the wait is over do not end
        assert_eq!(
            decide(|f| f.registrations = vec![Some("scheduled"), Some("saved")]),
            Finish::Wait
        );
        assert_eq!(
            decide(|f| {
                f.registrations = vec![Some("saved")];
                f.waited = true;
            }),
            Finish::Stay("registration-unconfirmed")
        );
        // the user asking for deck outranks every other reason
        assert_eq!(
            decide(|f| {
                f.reopened = true;
                f.inbox_empty = false;
                f.registrations = vec![Some("registration-failed")];
            }),
            Finish::Stay("reopened")
        );
    }
    #[test]
    fn an_acked_snooze_names_the_request_the_end_waits_for() {
        let _serial = fresh_launch();
        let request = request_id("card", &sample());
        let next = request_id(
            "card",
            &Reminder {
                revision: 2,
                ..sample()
            },
        );
        // deck already running: nothing is expected
        assert!(reminder_launch_visible());
        expect_registration(&request, "snooze");
        assert!(EXPECTED.lock_or_recover().is_empty());
        drop(_serial);
        let _serial = fresh_launch();
        assert!(response_arrived(2));
        expect_registration(&request, "open");
        expect_registration("not a request", "snooze");
        assert!(EXPECTED.lock_or_recover().is_empty());
        expect_registration(&request, "snooze");
        assert_eq!(*EXPECTED.lock_or_recover(), vec![next.clone()]);
        // what the end reads is the state the projection last recorded
        RESULTS.lock_or_recover().remove(&next);
        assert_eq!(expected_states(), vec![None]);
        RESULTS.lock_or_recover().insert(next.clone(), "saved");
        assert_eq!(expected_states(), vec![Some("saved")]);
        RESULTS.lock_or_recover().insert(next.clone(), "scheduled");
        assert_eq!(expected_states(), vec![Some("scheduled")]);
        RESULTS.lock_or_recover().remove(&next);
    }
    #[test]
    fn an_answer_that_could_not_be_stored_keeps_deck_even_after_the_webview_was_told() {
        let _serial = fresh_launch();
        assert!(response_arrived(2));
        answer_not_saved();
        // the webview's read consumes the one-time notice, not the fact
        assert!(ACTION_FAILURE.swap(false, Ordering::AcqRel));
        let facts = finish_facts(false);
        assert!(facts.failed && facts.inbox_empty);
        assert_eq!(finish_decision(&facts), Finish::Stay("answer-pending"));
    }
    #[test]
    fn the_dock_during_a_response_only_launch_keeps_deck() {
        let _serial = fresh_launch();
        assert!(response_arrived(2));
        assert!(!finish_facts(false).reopened);
        reopen_requested();
        let facts = finish_facts(false);
        assert!(facts.response_only && facts.reopened);
        assert_eq!(finish_decision(&facts), Finish::Stay("reopened"));
    }
}
