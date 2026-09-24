//! notify.rs — away notifications and the Dock badge, the desktop side of
//! the attention loop: a macOS notification when an agent needs the user
//! while the deck window is not in front, and a badge with the number of
//! cards waiting. Both are derived from the agent-status hook words only.
//!
//! # Contract
//! - **Two closed signals, one switch.** A notification is posted for a
//!   transition INTO `needs-input`, or INTO `turn-done` (which is unread
//!   until the card is viewed), and only while the main window is not
//!   focused (`FOCUSED`, fed by `WindowEvent::Focused`; a hidden window is
//!   unfocused). The badge counts sessions in `needs-input` plus sessions
//!   in `turn-done` that are still unread — the same rows the Needs
//!   attention list shows, minus manual follow-up — and is kept whether or
//!   not the window is focused. Both exist only while the
//!   `notifyAway` setting is on (off by default; `notifySound` adds the
//!   default sound). Nothing is posted for `working`, for the 15 s output
//!   heuristic (no hook state is no state), for a star, or for a stop.
//! - **Triggered from Rust, not the webview.** `agent_status::ingest`
//!   calls `observe` on the listener thread and `reconcile` calls `retain`,
//!   so a notification is posted even while App Nap has frozen the
//!   webview's poll. The webview supplies what Rust cannot know: card
//!   labels (`notify_cards`: session → title + project, memory only), the
//!   fact that a `turn-done` was viewed (`notify_dismiss`), and the
//!   settings (`notify_configure`).
//! - **Content is a closed set.** The title is the card's own title, the
//!   body is one of two fixed phrases (`body_text`, en / zh-Hans, following
//!   the locale setting) prefixed by the project name. No prompt, output,
//!   path or free text ever reaches the system, and nothing but closed
//!   codes reaches app.log (`tests/log_privacy.rs`). A session without a
//!   label is never announced.
//! - **Once per transition; withdrawn when handled.** A repeated word is
//!   ignored; `working` after a notified state withdraws the notification
//!   and clears the unread mark; viewing the card (`dismiss`) withdraws it;
//!   a session that left the hook store is forgotten. The system's own
//!   identifier is the tmux session name, so one card has at most one
//!   notification.
//! - **Native surface.** `native/NotificationBridge.swift` is compiled into
//!   the same static library as the speech bridge and is a no-op outside a
//!   real .app bundle (`status()` reports `unsupported`). Authorization is
//!   requested only when the user turns the switch on; `denied` is shown as
//!   a fixed status, never retried or worked around. A click hands back the
//!   identifier; deck shows and focuses its window and emits `notify-open`
//!   with the session, and the webview opens that card. The badge goes
//!   through Tauri's `set_badge_count`. No process is spawned and no file
//!   is written by this module.

use std::collections::{HashMap, HashSet};
use std::ffi::{c_char, CStr, CString};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex, OnceLock};

use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::applog::applog;
use crate::error::{DeckError, ErrorKind};
use crate::sync::LockRecover;

/// Closed authorization words, in the order of the bridge's status codes.
pub(crate) const STATUS_WORDS: [&str; 5] = [
    "unsupported",
    "not-determined",
    "denied",
    "authorized",
    "provisional",
];

const NEEDS_INPUT: &str = "needs-input";
const TURN_DONE: &str = "turn-done";

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(crate) struct CardLabel {
    pub(crate) session: String,
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) project: String,
}

#[derive(Default)]
pub(crate) struct Notify {
    enabled: bool,
    sound: bool,
    labels: HashMap<String, CardLabel>,
    /// Last hook word seen per session, as agent_status reported it.
    states: HashMap<String, &'static str>,
    /// Sessions whose `turn-done` has not been viewed.
    unread: HashSet<String>,
    /// Sessions with a notification handed to the system.
    posted: HashSet<String>,
}

/// What the module asks of the platform; `SystemNative` is the bridge and
/// the tests record calls instead.
pub(crate) trait Native {
    fn status(&self) -> &'static str;
    fn request(&self);
    fn post(&self, session: &str, title: &str, body: &str, sound: bool) -> bool;
    fn remove(&self, session: &str);
    fn badge(&self, count: usize);
}

static NOTIFY: LazyLock<Mutex<Notify>> = LazyLock::new(|| Mutex::new(Notify::default()));
/// The main window is in front. Starts true: the window opens focused.
static FOCUSED: AtomicBool = AtomicBool::new(true);
static APP: OnceLock<AppHandle> = OnceLock::new();

pub(crate) fn set_focused(focused: bool) {
    FOCUSED.store(focused, Ordering::Release);
}

fn away() -> bool {
    !FOCUSED.load(Ordering::Acquire)
}

/// The two phrases the system ever shows, per locale, after the project.
pub(crate) fn body_text(locale: &str, state: &str, project: &str) -> String {
    let phrase = match (locale == "zh-Hans", state == NEEDS_INPUT) {
        (true, true) => "需要你的输入",
        (true, false) => "一轮已结束",
        (false, true) => "needs your input",
        (false, false) => "a turn has ended",
    };
    if project.is_empty() {
        phrase.to_string()
    } else {
        format!("{project} · {phrase}")
    }
}

fn can_post(native: &dyn Native) -> bool {
    matches!(native.status(), "authorized" | "provisional")
}

fn badge_count(n: &Notify) -> usize {
    n.states
        .iter()
        .filter(|(session, state)| {
            **state == NEEDS_INPUT || (**state == TURN_DONE && n.unread.contains(*session))
        })
        .count()
}

fn push_badge(n: &Notify, native: &dyn Native) {
    native.badge(if n.enabled { badge_count(n) } else { 0 });
}

fn withdraw(n: &mut Notify, native: &dyn Native, session: &str) {
    if n.posted.remove(session) {
        native.remove(session);
    }
}

/// One hook word for one session, from `agent_status::ingest`.
pub(crate) fn observe_with(
    n: &mut Notify,
    native: &dyn Native,
    session: &str,
    state: &'static str,
    away: bool,
    locale: &str,
) {
    let previous = n.states.insert(session.to_string(), state);
    if previous == Some(state) {
        return;
    }
    let announces = state == NEEDS_INPUT || state == TURN_DONE;
    if !announces {
        n.unread.remove(session);
        withdraw(n, native, session);
        push_badge(n, native);
        return;
    }
    if state == TURN_DONE {
        n.unread.insert(session.to_string());
    }
    // a changed state replaces the previous notification (the identifier
    // is the session), so an unread `turn-done` is not left under a newer
    // `needs-input` and vice versa
    withdraw(n, native, session);
    if n.enabled && away && can_post(native) {
        if let Some(label) = n.labels.get(session) {
            if native.post(
                session,
                &label.title,
                &body_text(locale, state, &label.project),
                n.sound,
            ) {
                n.posted.insert(session.to_string());
                applog(&format!(
                    "[notify] posted {} s={}",
                    state,
                    crate::applog::session_tag(session)
                ));
            }
        }
    }
    push_badge(n, native);
}

/// Sessions that agent_status still tracks; every other one is forgotten.
pub(crate) fn retain_with(n: &mut Notify, native: &dyn Native, alive: &HashSet<String>) {
    let gone: Vec<String> = n
        .states
        .keys()
        .filter(|session| !alive.contains(*session))
        .cloned()
        .collect();
    if gone.is_empty() {
        return;
    }
    for session in gone {
        n.states.remove(&session);
        n.unread.remove(&session);
        withdraw(n, native, &session);
    }
    push_badge(n, native);
}

/// The card was viewed: its `turn-done` is read and its notification gone.
pub(crate) fn dismiss_with(n: &mut Notify, native: &dyn Native, session: &str) {
    let changed = n.unread.remove(session) | n.posted.contains(session);
    withdraw(n, native, session);
    if changed {
        push_badge(n, native);
    }
}

/// The switch. Turning it on asks for authorization once (`request`)
/// when the user did it (not at boot); turning it off withdraws every
/// notification and clears the badge.
pub(crate) fn configure_with(
    n: &mut Notify,
    native: &dyn Native,
    enabled: bool,
    sound: bool,
    request: bool,
) -> &'static str {
    n.enabled = enabled;
    n.sound = sound;
    if enabled && request && native.status() == "not-determined" {
        native.request();
    }
    if !enabled {
        let posted: Vec<String> = n.posted.iter().cloned().collect();
        for session in posted {
            withdraw(n, native, &session);
        }
    }
    push_badge(n, native);
    native.status()
}

pub(crate) fn set_labels_with(n: &mut Notify, labels: Vec<CardLabel>) -> Result<(), DeckError> {
    if labels.len() > 4096 {
        return Err(DeckError::new(ErrorKind::Invalid, "too many card labels"));
    }
    let mut map = HashMap::with_capacity(labels.len());
    for label in labels {
        if crate::tmux::validate_session_name(&label.session).is_err() || label.title.len() > 512 {
            return Err(DeckError::new(
                ErrorKind::Invalid,
                "card label out of bounds",
            ));
        }
        map.insert(label.session.clone(), label);
    }
    n.labels = map;
    Ok(())
}

// ---------- the platform ------------------------------------------------------

#[cfg(target_os = "macos")]
extern "C" {
    fn deck_notify_init(callback: extern "C" fn(*const c_char)) -> i32;
    fn deck_notify_request();
    fn deck_notify_status() -> i32;
    fn deck_notify_post(
        id: *const c_char,
        title: *const c_char,
        body: *const c_char,
        sound: i32,
    ) -> i32;
    fn deck_notify_remove(id: *const c_char);
}

struct SystemNative;

fn c_string(value: &str) -> Option<CString> {
    CString::new(value.replace('\0', "")).ok()
}

impl Native for SystemNative {
    fn status(&self) -> &'static str {
        #[cfg(target_os = "macos")]
        {
            // SAFETY: a plain call into the bridge; it returns a small code.
            let code = unsafe { deck_notify_status() };
            STATUS_WORDS
                .get(usize::try_from(code).unwrap_or(0))
                .copied()
                .unwrap_or("unsupported")
        }
        #[cfg(not(target_os = "macos"))]
        {
            "unsupported"
        }
    }
    fn request(&self) {
        #[cfg(target_os = "macos")]
        // SAFETY: no arguments; the bridge owns the asynchronous dialog.
        unsafe {
            deck_notify_request()
        }
    }
    fn post(&self, session: &str, title: &str, body: &str, sound: bool) -> bool {
        let (Some(id), Some(title), Some(body)) =
            (c_string(session), c_string(title), c_string(body))
        else {
            return false;
        };
        #[cfg(target_os = "macos")]
        {
            // SAFETY: three NUL-terminated strings that outlive the call;
            // the bridge copies them into the notification content.
            unsafe {
                deck_notify_post(id.as_ptr(), title.as_ptr(), body.as_ptr(), i32::from(sound)) == 1
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (id, title, body, sound);
            false
        }
    }
    fn remove(&self, session: &str) {
        if let Some(id) = c_string(session) {
            #[cfg(target_os = "macos")]
            // SAFETY: one NUL-terminated string that outlives the call.
            unsafe {
                deck_notify_remove(id.as_ptr())
            }
            #[cfg(not(target_os = "macos"))]
            let _ = id;
        }
    }
    fn badge(&self, count: usize) {
        if let Some(window) = APP.get().and_then(|app| app.get_webview_window("main")) {
            let value = (count > 0).then(|| i64::try_from(count).unwrap_or(i64::MAX));
            let _ = window.set_badge_count(value);
        }
    }
}

extern "C" fn opened_callback(id: *const c_char) {
    if id.is_null() {
        return;
    }
    // SAFETY: the bridge lends a valid NUL-terminated identifier for this call.
    let session = unsafe { CStr::from_ptr(id) }.to_string_lossy().into_owned();
    opened(&session);
}

/// A click on the notification: the window comes to the front and the
/// webview is told which session to open.
pub(crate) fn opened(session: &str) {
    if crate::tmux::validate_session_name(session).is_err() {
        return;
    }
    let Some(app) = APP.get() else {
        return;
    };
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
    applog("[notify] opened");
    let _ = app.emit("notify-open", serde_json::json!({ "session": session }));
}

/// Boot: remember the handle, install the click delegate, and apply the
/// saved switch without asking for authorization (that is the user's click
/// in Settings). Runs on the main thread from `setup`.
pub(crate) fn init(app: AppHandle, enabled: bool, sound: bool) {
    let _ = APP.set(app);
    #[cfg(target_os = "macos")]
    {
        // SAFETY: registers a static callback; returns 0 outside a bundle.
        let bundled = unsafe { deck_notify_init(opened_callback) };
        if bundled == 0 {
            applog("[notify] unsupported outside a bundle");
        }
    }
    let status = configure_with(
        &mut NOTIFY.lock_or_recover(),
        &SystemNative,
        enabled,
        sound,
        false,
    );
    applog(&format!("[notify] boot {status}"));
}

pub(crate) fn observe(session: &str, state: &'static str) {
    let locale = crate::documents::locale_setting();
    observe_with(
        &mut NOTIFY.lock_or_recover(),
        &SystemNative,
        session,
        state,
        away(),
        &locale,
    );
}

pub(crate) fn retain(alive: &HashSet<String>) {
    retain_with(&mut NOTIFY.lock_or_recover(), &SystemNative, alive);
}

// ---------- commands ----------------------------------------------------------

#[tauri::command]
pub(crate) fn notify_configure(enabled: bool, sound: bool, request: bool) -> String {
    let status = configure_with(
        &mut NOTIFY.lock_or_recover(),
        &SystemNative,
        enabled,
        sound,
        request,
    );
    applog(&format!("[notify] configured {status}"));
    status.to_string()
}

#[tauri::command]
pub(crate) fn notify_status() -> String {
    SystemNative.status().to_string()
}

#[tauri::command]
pub(crate) fn notify_cards(cards: Vec<CardLabel>) -> Result<(), DeckError> {
    set_labels_with(&mut NOTIFY.lock_or_recover(), cards)
}

#[tauri::command]
pub(crate) fn notify_dismiss(session: String) -> Result<(), DeckError> {
    crate::tmux::validate_session_name(&session)?;
    dismiss_with(&mut NOTIFY.lock_or_recover(), &SystemNative, &session);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Fake {
        status: &'static str,
        calls: RefCell<Vec<String>>,
        badge: RefCell<Option<usize>>,
        refuse: bool,
    }
    impl Fake {
        fn new(status: &'static str) -> Self {
            Fake {
                status,
                ..Fake::default()
            }
        }
        fn calls(&self) -> Vec<String> {
            self.calls.borrow().clone()
        }
        fn clear(&self) {
            self.calls.borrow_mut().clear();
        }
    }
    impl Native for Fake {
        fn status(&self) -> &'static str {
            self.status
        }
        fn request(&self) {
            self.calls.borrow_mut().push("request".into());
        }
        fn post(&self, session: &str, title: &str, body: &str, sound: bool) -> bool {
            self.calls
                .borrow_mut()
                .push(format!("post {session} [{title}] [{body}] sound={sound}"));
            !self.refuse
        }
        fn remove(&self, session: &str) {
            self.calls.borrow_mut().push(format!("remove {session}"));
        }
        fn badge(&self, count: usize) {
            *self.badge.borrow_mut() = Some(count);
        }
    }

    fn label(session: &str, title: &str, project: &str) -> CardLabel {
        CardLabel {
            session: session.into(),
            title: title.into(),
            project: project.into(),
        }
    }

    fn ready() -> (Notify, Fake) {
        let mut n = Notify::default();
        let fake = Fake::new("authorized");
        set_labels_with(
            &mut n,
            vec![
                label("deck-card-ab12", "Fix the parser", "deck"),
                label("deck-card-cd34", "Write docs", ""),
            ],
        )
        .unwrap();
        assert_eq!(
            configure_with(&mut n, &fake, true, false, false),
            "authorized"
        );
        fake.clear();
        (n, fake)
    }

    #[test]
    fn a_transition_into_a_waiting_state_is_announced_once_while_away() {
        let (mut n, fake) = ready();
        observe_with(&mut n, &fake, "deck-card-ab12", "working", true, "en");
        assert!(fake.calls().is_empty(), "working is never announced");
        observe_with(&mut n, &fake, "deck-card-ab12", NEEDS_INPUT, true, "en");
        assert_eq!(
            fake.calls(),
            ["post deck-card-ab12 [Fix the parser] [deck · needs your input] sound=false"]
        );
        assert_eq!(*fake.badge.borrow(), Some(1));
        fake.clear();
        // the same word again: nothing
        observe_with(&mut n, &fake, "deck-card-ab12", NEEDS_INPUT, true, "en");
        assert!(fake.calls().is_empty());
        // the turn ends: the input notification is replaced, unread counts
        observe_with(&mut n, &fake, "deck-card-ab12", TURN_DONE, true, "zh-Hans");
        assert_eq!(
            fake.calls(),
            [
                "remove deck-card-ab12",
                "post deck-card-ab12 [Fix the parser] [deck · 一轮已结束] sound=false"
            ]
        );
        assert_eq!(*fake.badge.borrow(), Some(1));
        fake.clear();
        // a new turn starts: withdrawn, unread cleared, badge empty
        observe_with(&mut n, &fake, "deck-card-ab12", "working", true, "en");
        assert_eq!(fake.calls(), ["remove deck-card-ab12"]);
        assert_eq!(*fake.badge.borrow(), Some(0));
    }

    #[test]
    fn nothing_is_posted_while_focused_disabled_unauthorized_or_unlabelled() {
        let (mut n, fake) = ready();
        observe_with(&mut n, &fake, "deck-card-ab12", NEEDS_INPUT, false, "en");
        assert!(fake.calls().is_empty(), "focused: the board is in front");
        assert_eq!(*fake.badge.borrow(), Some(1), "but the badge still counts");
        observe_with(&mut n, &fake, "deck-card-zz99", NEEDS_INPUT, true, "en");
        assert!(fake.calls().is_empty(), "no label, no announcement");
        assert_eq!(
            *fake.badge.borrow(),
            Some(2),
            "the badge counts hook state, not labels"
        );

        let denied = Fake::new("denied");
        let mut m = Notify::default();
        set_labels_with(&mut m, vec![label("deck-card-ab12", "t", "p")]).unwrap();
        configure_with(&mut m, &denied, true, true, true);
        assert!(denied.calls().is_empty(), "denied is never re-requested");
        observe_with(&mut m, &denied, "deck-card-ab12", TURN_DONE, true, "en");
        assert!(denied.calls().is_empty());

        let off = Fake::new("authorized");
        let mut o = Notify::default();
        set_labels_with(&mut o, vec![label("deck-card-ab12", "t", "p")]).unwrap();
        configure_with(&mut o, &off, false, false, false);
        observe_with(&mut o, &off, "deck-card-ab12", NEEDS_INPUT, true, "en");
        assert!(off.calls().is_empty());
        assert_eq!(
            *off.badge.borrow(),
            Some(0),
            "the switch off keeps the badge empty"
        );
    }

    #[test]
    fn viewing_dismisses_and_a_vanished_session_is_forgotten() {
        let (mut n, fake) = ready();
        observe_with(&mut n, &fake, "deck-card-ab12", TURN_DONE, true, "en");
        observe_with(&mut n, &fake, "deck-card-cd34", NEEDS_INPUT, true, "en");
        assert_eq!(*fake.badge.borrow(), Some(2));
        fake.clear();
        dismiss_with(&mut n, &fake, "deck-card-ab12");
        assert_eq!(fake.calls(), ["remove deck-card-ab12"]);
        assert_eq!(
            *fake.badge.borrow(),
            Some(1),
            "read turn-done leaves the badge"
        );
        fake.clear();
        dismiss_with(&mut n, &fake, "deck-card-ab12");
        assert!(fake.calls().is_empty(), "a second view changes nothing");
        // viewing a needs-input keeps it counted: the question is still open
        dismiss_with(&mut n, &fake, "deck-card-cd34");
        assert_eq!(fake.calls(), ["remove deck-card-cd34"]);
        assert_eq!(*fake.badge.borrow(), Some(1));
        fake.clear();
        let alive: HashSet<String> = ["deck-card-ab12".to_string()].into_iter().collect();
        retain_with(&mut n, &fake, &alive);
        assert_eq!(*fake.badge.borrow(), Some(0));
        assert!(!n.states.contains_key("deck-card-cd34"));
        retain_with(&mut n, &fake, &alive);
        assert!(fake.calls().is_empty(), "nothing to forget is silent");
    }

    #[test]
    fn the_switch_requests_once_when_the_user_turns_it_on_and_withdraws_when_off() {
        let fake = Fake::new("not-determined");
        let mut n = Notify::default();
        assert_eq!(
            configure_with(&mut n, &fake, true, false, false),
            "not-determined"
        );
        assert!(fake.calls().is_empty(), "boot never asks");
        assert_eq!(
            configure_with(&mut n, &fake, true, true, true),
            "not-determined"
        );
        assert_eq!(fake.calls(), ["request"]);
        fake.clear();
        let granted = Fake::new("authorized");
        set_labels_with(&mut n, vec![label("deck-card-ab12", "t", "p")]).unwrap();
        observe_with(&mut n, &granted, "deck-card-ab12", NEEDS_INPUT, true, "en");
        assert_eq!(granted.calls().len(), 1);
        granted.clear();
        configure_with(&mut n, &granted, false, false, false);
        assert_eq!(granted.calls(), ["remove deck-card-ab12"]);
        assert_eq!(*granted.badge.borrow(), Some(0));
    }

    #[test]
    fn a_refused_post_is_not_remembered_and_labels_are_bounded() {
        let mut n = Notify::default();
        let fake = Fake {
            status: "provisional",
            refuse: true,
            ..Fake::default()
        };
        set_labels_with(&mut n, vec![label("deck-card-ab12", "t", "p")]).unwrap();
        configure_with(&mut n, &fake, true, true, false);
        observe_with(&mut n, &fake, "deck-card-ab12", NEEDS_INPUT, true, "en");
        assert_eq!(
            fake.calls(),
            ["post deck-card-ab12 [t] [p · needs your input] sound=true"]
        );
        assert!(n.posted.is_empty());
        assert!(set_labels_with(&mut n, vec![label("bad name", "t", "p")]).is_err());
        assert!(
            set_labels_with(&mut n, vec![label("deck-card-ab12", &"x".repeat(513), "p")]).is_err()
        );
        assert!(set_labels_with(&mut n, vec![label("deck-card-ab12", "ok", "p")]).is_ok());
    }

    #[test]
    fn the_body_is_one_of_two_phrases_per_locale() {
        assert_eq!(body_text("en", NEEDS_INPUT, ""), "needs your input");
        assert_eq!(
            body_text("en", TURN_DONE, "deck"),
            "deck · a turn has ended"
        );
        assert_eq!(
            body_text("zh-Hans", NEEDS_INPUT, "deck"),
            "deck · 需要你的输入"
        );
        assert_eq!(body_text("zh-Hans", TURN_DONE, ""), "一轮已结束");
        assert_eq!(STATUS_WORDS.len(), 5);
    }

    #[test]
    fn system_native_refuses_interior_nuls_and_reports_unsupported_headless() {
        assert!(c_string("a\0b").is_some_and(|c| c.as_bytes() == b"ab"));
        // no bundle in a test binary: the bridge answers unsupported
        assert_eq!(SystemNative.status(), "unsupported");
        assert!(!SystemNative.post("deck-card-ab12", "t", "b", false));
        SystemNative.remove("deck-card-ab12");
        SystemNative.badge(1);
        opened("deck-card-ab12");
        opened("bad name");
    }
}
