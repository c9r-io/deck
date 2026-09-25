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
//! - **Attention, not authority.** The two words are interaction
//!   observations: `needs-input` = the agent requested input (it may have
//!   moved on since), `turn-done` = an interaction ended (not task
//!   completion, an idle agent or a finished program). The two phrases in
//!   `body_text` say exactly that and no more, the unread
//!   mark is attention bookkeeping, and nothing here causes a side effect
//!   beyond the notification and badge (`tests/signal_census.rs`). A task
//!   whose agent resumes after background work ends a second turn and is
//!   announced again; no dwell or debounce is applied — a delay would
//!   change when an ending is shown, not what it means.
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
//! - **Once per episode; withdrawn when superseded or viewed** — never
//!   a claim that the cause was resolved. Input is the session's PROJECTED
//!   observation (`agent_status::Observation`: word, Deck-local episode,
//!   viewed), from `ingest` for the Signal target pane and from `reconcile`
//!   when the projection changes. The same episode again is not a
//!   transition (pane switches, re-projection, repeated words); `working`
//!   withdraws; a `turn-done` episode is unread until `agent_status` records
//!   it viewed — that viewed flag is the one authoritative truth, so an
//!   already viewed episode projected again is suppressed, never re-armed
//!   (FR-SI-05). `notify_dismiss(session, episode)` marks exactly that
//!   episode; a stale episode is a no-op. A session that left the hook store
//!   is forgotten. The system's own identifier is the tmux session name, so
//!   one card has at most one notification.
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
    /// The last projected observation per session, as agent_status
    /// reported it; unread = a `turn-done` whose episode is not viewed.
    states: HashMap<String, crate::agent_status::Observation>,
    /// Sessions with a notification handed to the system.
    posted: HashSet<String>,
    /// Announcing transitions per session (a notification was due).
    #[cfg(test)]
    announced: HashMap<String, u32>,
}

fn unread(seen: &crate::agent_status::Observation) -> bool {
    seen.state == TURN_DONE && !seen.viewed
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
        (true, true) => "请求了你的输入",
        (true, false) => "一轮已结束",
        (false, true) => "asked for your input",
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
        .filter(|(_, seen)| seen.state == NEEDS_INPUT || unread(seen))
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

/// One projected observation for one session (module header).
pub(crate) fn observe_with(
    n: &mut Notify,
    native: &dyn Native,
    session: &str,
    seen: crate::agent_status::Observation,
    away: bool,
    locale: &str,
) {
    let previous = n.states.insert(session.to_string(), seen);
    if previous.is_some_and(|p| p.state == seen.state && p.episode == seen.episode) {
        // the same episode: only its viewed flag can have changed
        if previous.is_some_and(|p| unread(&p)) && !unread(&seen) {
            withdraw(n, native, session);
            push_badge(n, native);
        }
        return;
    }
    let announces = seen.state == NEEDS_INPUT || seen.state == TURN_DONE;
    // a changed episode replaces the previous notification (the identifier
    // is the session), so an unread `turn-done` is not left under a newer
    // `needs-input` and vice versa
    withdraw(n, native, session);
    if !announces {
        push_badge(n, native);
        return;
    }
    if seen.state == TURN_DONE && seen.viewed {
        // an already viewed ending projected again (a pane switch back)
        applog(&format!(
            "[notify] suppressed viewed-episode s={} e={}",
            crate::applog::session_tag(session),
            seen.episode
        ));
        push_badge(n, native);
        return;
    }
    #[cfg(test)]
    {
        *n.announced.entry(session.to_string()).or_default() += 1;
    }
    if n.enabled && away && can_post(native) {
        if let Some(label) = n.labels.get(session) {
            if native.post(
                session,
                &label.title,
                &body_text(locale, seen.state, &label.project),
                n.sound,
            ) {
                n.posted.insert(session.to_string());
                applog(&format!(
                    "[notify] posted {} s={} e={}",
                    seen.state,
                    crate::applog::session_tag(session),
                    seen.episode
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
        withdraw(n, native, &session);
    }
    push_badge(n, native);
}

/// `episode` of `session` was viewed (agent_status already recorded it):
/// if it is the session's projected episode its notification goes and the
/// badge drops. Any other episode changes nothing here.
pub(crate) fn dismiss_with(
    n: &mut Notify,
    native: &dyn Native,
    session: &str,
    episode: crate::agent_status::EpisodeId,
) {
    let Some(seen) = n.states.get_mut(session) else {
        return;
    };
    if seen.episode != episode || seen.state != TURN_DONE {
        return;
    }
    seen.viewed = true;
    withdraw(n, native, session);
    push_badge(n, native);
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

pub(crate) fn observe(session: &str, seen: crate::agent_status::Observation) {
    // The same observation again is a no-op (`observe_with`'s first rule).
    // Reconcile re-projects every session on every poll, so answer that
    // case before the locale read, which reads the settings document.
    if NOTIFY.lock_or_recover().states.get(session) == Some(&seen) {
        return;
    }
    let locale = crate::documents::locale_setting();
    observe_with(
        &mut NOTIFY.lock_or_recover(),
        &SystemNative,
        session,
        seen,
        away(),
        &locale,
    );
}

pub(crate) fn retain(alive: &HashSet<String>) {
    retain_with(&mut NOTIFY.lock_or_recover(), &SystemNative, alive);
}

/// The global notification state as tests see it: session → last word,
/// the unread sessions and the Dock count the badge would show. Tests that
/// read it hold `agent_status::STORE_TEST_LOCK` (the only writer in tests).
#[cfg(test)]
pub(crate) fn snapshot_for_tests() -> (HashMap<String, &'static str>, HashSet<String>, usize) {
    let n = NOTIFY.lock_or_recover();
    (
        n.states
            .iter()
            .map(|(s, seen)| (s.clone(), seen.state))
            .collect(),
        n.states
            .iter()
            .filter(|(_, seen)| unread(seen))
            .map(|(s, _)| s.clone())
            .collect(),
        badge_count(&n),
    )
}

/// A fresh notification state for one trace (test isolation).
#[cfg(test)]
pub(crate) fn reset_for_tests() {
    *NOTIFY.lock_or_recover() = Notify::default();
}

/// How many announcing transitions (a notification was due) each session
/// has had — the notification eligibility the trace harness asserts.
#[cfg(test)]
pub(crate) fn announced_for_tests() -> HashMap<String, u32> {
    NOTIFY.lock_or_recover().announced.clone()
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

/// The webview displayed `episode` of `session` (FR-SI-05). agent_status
/// records exactly that live `turn-done` episode as viewed — the one
/// authoritative viewed truth, projected back to every surface — and the
/// notification layer follows. A stale or unknown episode is a no-op; it is
/// never translated into "dismiss whatever is current".
#[tauri::command]
pub(crate) fn notify_dismiss(
    session: String,
    episode: crate::agent_status::EpisodeId,
) -> Result<(), DeckError> {
    crate::tmux::validate_session_name(&session)?;
    if crate::agent_status::mark_viewed(&session, episode) {
        dismiss_with(
            &mut NOTIFY.lock_or_recover(),
            &SystemNative,
            &session,
            episode,
        );
        applog(&format!(
            "[notify] viewed s={} e={episode}",
            crate::applog::session_tag(&session)
        ));
    }
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

    use crate::agent_status::Observation;

    /// v1-like episodes for the word-level tests: the same word keeps the
    /// session's episode, a changed word gets a new one.
    fn observe_word(
        n: &mut Notify,
        native: &dyn Native,
        session: &str,
        state: &'static str,
        away: bool,
        locale: &str,
    ) {
        let episode = match n.states.get(session) {
            Some(seen) if seen.state == state => seen.episode,
            Some(seen) => seen.episode + 100,
            None => 1,
        };
        let seen = Observation {
            state,
            episode,
            viewed: false,
        };
        observe_with(n, native, session, seen, away, locale);
    }

    /// The webview viewed the session's current episode (what agent_status
    /// `mark_viewed` then confirms for a live `turn-done`).
    fn dismiss_current(n: &mut Notify, native: &dyn Native, session: &str) {
        if let Some(episode) = n.states.get(session).map(|seen| seen.episode) {
            dismiss_with(n, native, session, episode);
        }
    }

    #[test]
    fn a_transition_into_a_waiting_state_is_announced_once_while_away() {
        let (mut n, fake) = ready();
        observe_word(&mut n, &fake, "deck-card-ab12", "working", true, "en");
        assert!(fake.calls().is_empty(), "working is never announced");
        observe_word(&mut n, &fake, "deck-card-ab12", NEEDS_INPUT, true, "en");
        assert_eq!(
            fake.calls(),
            ["post deck-card-ab12 [Fix the parser] [deck · asked for your input] sound=false"]
        );
        assert_eq!(*fake.badge.borrow(), Some(1));
        fake.clear();
        // the same word again: nothing
        observe_word(&mut n, &fake, "deck-card-ab12", NEEDS_INPUT, true, "en");
        assert!(fake.calls().is_empty());
        // the turn ends: the input notification is replaced, unread counts
        observe_word(&mut n, &fake, "deck-card-ab12", TURN_DONE, true, "zh-Hans");
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
        observe_word(&mut n, &fake, "deck-card-ab12", "working", true, "en");
        assert_eq!(fake.calls(), ["remove deck-card-ab12"]);
        assert_eq!(*fake.badge.borrow(), Some(0));
    }

    #[test]
    fn nothing_is_posted_while_focused_disabled_unauthorized_or_unlabelled() {
        let (mut n, fake) = ready();
        observe_word(&mut n, &fake, "deck-card-ab12", NEEDS_INPUT, false, "en");
        assert!(fake.calls().is_empty(), "focused: the board is in front");
        assert_eq!(*fake.badge.borrow(), Some(1), "but the badge still counts");
        observe_word(&mut n, &fake, "deck-card-zz99", NEEDS_INPUT, true, "en");
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
        observe_word(&mut m, &denied, "deck-card-ab12", TURN_DONE, true, "en");
        assert!(denied.calls().is_empty());

        let off = Fake::new("authorized");
        let mut o = Notify::default();
        set_labels_with(&mut o, vec![label("deck-card-ab12", "t", "p")]).unwrap();
        configure_with(&mut o, &off, false, false, false);
        observe_word(&mut o, &off, "deck-card-ab12", NEEDS_INPUT, true, "en");
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
        observe_word(&mut n, &fake, "deck-card-ab12", TURN_DONE, true, "en");
        observe_word(&mut n, &fake, "deck-card-cd34", NEEDS_INPUT, true, "en");
        assert_eq!(*fake.badge.borrow(), Some(2));
        fake.clear();
        dismiss_current(&mut n, &fake, "deck-card-ab12");
        assert_eq!(fake.calls(), ["remove deck-card-ab12"]);
        assert_eq!(
            *fake.badge.borrow(),
            Some(1),
            "read turn-done leaves the badge"
        );
        fake.clear();
        dismiss_current(&mut n, &fake, "deck-card-ab12");
        assert!(fake.calls().is_empty(), "a second view changes nothing");
        // viewing a needs-input changes nothing: the question is still open
        // and viewing is not answering (only a turn-done episode is viewed)
        dismiss_current(&mut n, &fake, "deck-card-cd34");
        assert!(fake.calls().is_empty());
        assert_eq!(*fake.badge.borrow(), Some(1));
        fake.clear();
        let alive: HashSet<String> = ["deck-card-ab12".to_string()].into_iter().collect();
        retain_with(&mut n, &fake, &alive);
        assert_eq!(*fake.badge.borrow(), Some(0));
        assert!(!n.states.contains_key("deck-card-cd34"));
        assert_eq!(
            fake.calls(),
            ["remove deck-card-cd34"],
            "its notification goes with it"
        );
        fake.clear();
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
        observe_word(&mut n, &granted, "deck-card-ab12", NEEDS_INPUT, true, "en");
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
        observe_word(&mut n, &fake, "deck-card-ab12", NEEDS_INPUT, true, "en");
        assert_eq!(
            fake.calls(),
            ["post deck-card-ab12 [t] [p · asked for your input] sound=true"]
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
        assert_eq!(body_text("en", NEEDS_INPUT, ""), "asked for your input");
        assert_eq!(
            body_text("en", TURN_DONE, "deck"),
            "deck · a turn has ended"
        );
        assert_eq!(
            body_text("zh-Hans", NEEDS_INPUT, "deck"),
            "deck · 请求了你的输入"
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

    fn ended(episode: u64, viewed: bool) -> Observation {
        Observation {
            state: TURN_DONE,
            episode,
            viewed,
        }
    }

    /// FR-SI-05: the same episode projected again (a pane switch away and
    /// back) is not a transition; once viewed, it is suppressed — no
    /// re-armed unread, badge or notification. A new episode re-arms.
    #[test]
    fn a_viewed_episode_projected_again_is_not_re_armed() {
        let (mut n, fake) = ready();
        let s = "deck-card-ab12";
        observe_with(&mut n, &fake, s, ended(7, false), true, "en");
        assert_eq!(*fake.badge.borrow(), Some(1));
        dismiss_with(&mut n, &fake, s, 7);
        assert_eq!(*fake.badge.borrow(), Some(0));
        fake.clear();
        // the other pane becomes the target, then this one again
        observe_with(
            &mut n,
            &fake,
            s,
            Observation {
                state: "working",
                episode: 8,
                viewed: false,
            },
            true,
            "en",
        );
        observe_with(&mut n, &fake, s, ended(7, true), true, "en");
        assert!(
            !fake.calls().iter().any(|c| c.starts_with("post")),
            "{:?}",
            fake.calls()
        );
        assert_eq!(*fake.badge.borrow(), Some(0));
        assert_eq!(
            n.announced.get(s),
            Some(&1),
            "announced once, for the first projection only"
        );
        // a genuinely new ending re-arms
        fake.clear();
        observe_with(&mut n, &fake, s, ended(9, false), true, "en");
        assert!(fake.calls().iter().any(|c| c.starts_with("post")));
        assert_eq!(*fake.badge.borrow(), Some(1));
    }

    /// The dismiss race: the webview rendered e3, the backend advanced to
    /// e5; a dismiss for e3 must leave e5 unread.
    #[test]
    fn a_stale_dismiss_never_dismisses_the_current_episode() {
        let (mut n, fake) = ready();
        let s = "deck-card-ab12";
        observe_with(&mut n, &fake, s, ended(3, false), true, "en");
        observe_with(
            &mut n,
            &fake,
            s,
            Observation {
                state: "working",
                episode: 4,
                viewed: false,
            },
            true,
            "en",
        );
        observe_with(&mut n, &fake, s, ended(5, false), true, "en");
        fake.clear();
        dismiss_with(&mut n, &fake, s, 3);
        assert!(fake.calls().is_empty());
        assert_eq!(n.states[s], ended(5, false), "e5 stays unread");
        assert_eq!(badge_count(&n), 1);
        // the same observation re-projected with its viewed flag set by
        // agent_status converges too
        observe_with(&mut n, &fake, s, ended(5, true), true, "en");
        assert_eq!(badge_count(&n), 0);
    }
}
