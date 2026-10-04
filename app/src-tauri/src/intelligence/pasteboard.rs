//! Clipboard authority: automatic reads require an armed, focused Lens and
//! a post-baseline change. Native AppKit focus checks are repeated at read
//! time. Focus loss clears ownership; a new arm only records changeCount.
//! There is no command that reads the clipboard's existing content: text
//! copied before the Copied-text tab armed (or while Deck was away) is
//! never read. Deck's own Lens copies go through `translation_clipboard_write`,
//! whose native write returns the changeCount it produced; the gate excludes
//! exactly that version (never text that resembles an old result), and the
//! receipt dies with the armed cycle. A version its writer marked confidential
//! or momentary (`org.nspasteboard.ConcealedType` / `TransientType`, looked
//! for natively before and after the read) is never read: the gate consumes
//! it without publishing text or an error, so the Lens keeps what it had.
//! The markers are the writer's choice; a copy without one is ordinary text
//! here. Debug smoke builds may point this same gate at a named test
//! pasteboard (`native/SmokeBridge.swift`); the gate logic is unchanged.
use crate::error::{DeckError, ErrorKind};
use crate::sync::LockRecover;
use std::ffi::{c_char, CStr};
use std::sync::{Mutex, OnceLock};

#[derive(Default, Clone, Copy)]
struct Gate {
    armed: bool,
    baseline: i64,
    generation: u64,
    /// The one version Deck's Lens itself wrote in this armed cycle (the
    /// writer's receipt). Only that exact changeCount is excluded; a later
    /// version is a real copy even when its text is identical.
    own: Option<i64>,
}
impl Gate {
    fn arm(&mut self, count: i64) {
        self.armed = true;
        self.baseline = count;
        self.own = None;
        self.generation = self.generation.wrapping_add(1);
    }
    fn disarm(&mut self) {
        self.armed = false;
        self.own = None;
        self.generation = self.generation.wrapping_add(1);
    }
    fn wrote(&mut self, count: i64) {
        if self.armed && count > self.baseline {
            self.own = Some(count);
        }
    }
    fn changed(&mut self, count: i64, generation: u64) -> bool {
        if !self.armed || self.generation != generation || count <= self.baseline {
            return false;
        }
        self.baseline = count;
        self.own.take() != Some(count)
    }
}
static GATE: OnceLock<Mutex<Gate>> = OnceLock::new();
fn gate() -> &'static Mutex<Gate> {
    GATE.get_or_init(|| Mutex::new(Gate::default()))
}
fn locked() -> std::sync::MutexGuard<'static, Gate> {
    gate().lock_or_recover()
}
fn error(code: &'static str) -> DeckError {
    DeckError::new(ErrorKind::Other, code)
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn deck_pasteboard_focused_count() -> i64;
    fn deck_pasteboard_read_text(withheld: *mut i32) -> *mut c_char;
    fn deck_pasteboard_free(text: *mut c_char);
    fn deck_pasteboard_write_text(text: *const c_char) -> i64;
}
fn focused_count() -> Result<i64, DeckError> {
    #[cfg(target_os = "macos")]
    let count = unsafe { deck_pasteboard_focused_count() };
    #[cfg(not(target_os = "macos"))]
    let count = -1;
    if count < 0 {
        Err(error("clipboard-not-focused"))
    } else {
        Ok(count)
    }
}
/// One read of the focused pasteboard.
#[derive(Debug, PartialEq)]
enum Read {
    Text(String),
    /// The writer marked this version confidential or momentary: its text
    /// was not read (`native/PasteboardBridge.swift`).
    Withheld,
}
fn read_text() -> Result<Read, DeckError> {
    #[cfg(target_os = "macos")]
    {
        let mut withheld = 0;
        let ptr = unsafe { deck_pasteboard_read_text(&mut withheld) };
        if ptr.is_null() {
            return if withheld != 0 {
                Ok(Read::Withheld)
            } else {
                Err(error("clipboard-not-text"))
            };
        }
        let text = unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned();
        unsafe {
            deck_pasteboard_free(ptr);
        }
        if text.len() > crate::documents::local_translation_settings().1 {
            return Err(error("text-too-large"));
        }
        if text.trim().is_empty() {
            return Err(error("text-empty"));
        }
        Ok(Read::Text(text))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(error("clipboard-unavailable"))
    }
}

pub(crate) fn focus_changed(focused: bool) {
    if !focused {
        locked().disarm();
    }
}

#[tauri::command]
pub(crate) fn translation_clipboard_arm() -> Result<(), DeckError> {
    if !crate::documents::local_translation_settings().0 {
        return Err(error("translation-disabled"));
    }
    let count = focused_count()?; // baseline only; never read content here
    locked().arm(count);
    Ok(())
}

#[tauri::command]
pub(crate) fn translation_clipboard_disarm() {
    locked().disarm();
}

/// Deck's own Lens copy (Copy Translation, Copy Source, Cmd+C inside the
/// result). The native write returns its receipt, recorded before this
/// command returns; sync commands run on the main thread, so a poll cannot
/// publish the write in between. Only that version is excluded from
/// Copied-text observation.
#[tauri::command]
pub(crate) fn translation_clipboard_write(text: String) -> Result<i64, DeckError> {
    if !crate::documents::local_translation_settings().0 {
        return Err(error("translation-disabled"));
    }
    let input = std::ffi::CString::new(text).map_err(|_| error("clipboard-unavailable"))?;
    #[cfg(target_os = "macos")]
    let count = unsafe { deck_pasteboard_write_text(input.as_ptr()) };
    #[cfg(not(target_os = "macos"))]
    let count = {
        let _ = input;
        -1
    };
    if count < 0 {
        return Err(error("clipboard-unavailable"));
    }
    locked().wrote(count);
    Ok(count)
}

#[tauri::command]
pub(crate) fn translation_clipboard_poll() -> Result<Option<String>, DeckError> {
    if !crate::documents::local_translation_settings().0 {
        locked().disarm();
        return Ok(None);
    }
    let (armed, baseline, generation) = {
        let state = locked();
        (state.armed, state.baseline, state.generation)
    };
    if !armed {
        return Ok(None);
    }
    let count = focused_count()?;
    if count <= baseline {
        return Ok(None);
    }
    let read = read_text();
    // A second native focus/count probe closes a race with focus loss and a
    // second clipboard write while the AppKit read was in flight.
    let recheck = focused_count()?;
    publish(&mut locked(), count, recheck, generation, read)
}

/// A read taken between two native probes is published only when both saw
/// the same change, the gate is still the one armed before the read, and
/// that change is past the baseline. A change during the read leaves the
/// baseline untouched, so the next poll reads the newer text instead.
fn accept_read(state: &mut Gate, count: i64, recheck: i64, generation: u64) -> bool {
    recheck == count && state.changed(count, generation)
}

/// What one poll hands to the Lens. A version the gate accepts is consumed
/// whatever the read found: its text is published, a failed read is
/// reported, and a version withheld for a privacy marker is neither: the
/// Lens keeps what it had, and the next copy is read as usual.
fn publish(
    state: &mut Gate,
    count: i64,
    recheck: i64,
    generation: u64,
    read: Result<Read, DeckError>,
) -> Result<Option<String>, DeckError> {
    if !accept_read(state, count, recheck, generation) {
        return Ok(None);
    }
    match read? {
        Read::Text(text) => Ok(Some(text)),
        Read::Withheld => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn focus_loss_revokes_old_baseline() {
        let mut state = locked();
        state.armed = true;
        state.baseline = 7;
        drop(state);
        focus_changed(false);
        let state = locked();
        assert!(!state.armed);
        assert_eq!(state.baseline, 7); // inert until a new arm replaces it
    }
    #[test]
    fn baseline_requires_new_change_and_generation() {
        let mut state = Gate::default();
        state.arm(12); // enter mode: baseline only
        let generation = state.generation;
        assert!(!state.changed(12, generation));
        assert!(state.changed(13, generation));
        assert!(!state.changed(13, generation));
        state.disarm(); // focus lost: unrelated copy must remain unread
        assert!(!state.changed(14, generation));
        state.arm(14); // focus regained: baseline without reading
        assert!(!state.changed(14, state.generation));
        assert!(state.changed(15, state.generation));
    }
    #[test]
    fn change_during_read_is_not_published_and_the_newer_change_is() {
        let mut state = Gate::default();
        state.arm(20);
        let generation = state.generation;
        assert!(!accept_read(&mut state, 21, 22, generation)); // copied again mid-read
        assert_eq!(
            state.baseline, 20,
            "baseline stays before the unstable read"
        );
        assert!(accept_read(&mut state, 22, 22, generation)); // next poll: newest text
        assert!(
            !accept_read(&mut state, 22, 22, generation),
            "published once"
        );
        state.arm(22); // the Lens was re-armed (mode/epoch change) during a read
        assert!(
            !accept_read(&mut state, 23, 23, generation),
            "a stale generation never publishes"
        );
        let current = state.generation;
        assert!(accept_read(&mut state, 23, 23, current));
    }
    /// One poll whose two native probes agree on `count`.
    fn stable(
        state: &mut Gate,
        count: i64,
        read: Result<Read, DeckError>,
    ) -> Result<Option<String>, DeckError> {
        let generation = state.generation;
        publish(state, count, count, generation, read)
    }
    fn text(value: &str) -> Result<Read, DeckError> {
        Ok(Read::Text(value.into()))
    }
    fn not_text() -> Result<Read, DeckError> {
        Err(error("clipboard-not-text"))
    }
    #[test]
    fn a_marked_version_is_consumed_and_nothing_is_published() {
        let mut state = Gate::default();
        state.arm(40);
        let withheld = stable(&mut state, 41, Ok(Read::Withheld));
        assert_eq!(withheld, Ok(None), "neither text nor an error");
        assert_eq!(state.baseline, 41, "the withheld version is consumed");
        let again = stable(&mut state, 41, text("x"));
        assert_eq!(again, Ok(None), "and never looked at again");
        let next = stable(&mut state, 42, text("next"));
        assert_eq!(next, Ok(Some("next".into())), "the next copy is read");
    }
    #[test]
    fn a_failed_read_is_reported_once_and_an_unstable_one_never() {
        let mut state = Gate::default();
        state.arm(50);
        let generation = state.generation;
        // copied again mid-read: nothing is published, whatever the read found
        for read in [Ok(Read::Withheld), text("stale"), not_text()] {
            assert_eq!(publish(&mut state, 51, 52, generation, read), Ok(None));
        }
        assert_eq!(state.baseline, 50, "an unstable read consumes nothing");
        let failed = stable(&mut state, 52, not_text());
        assert_eq!(failed, Err(error("clipboard-not-text")));
        let again = stable(&mut state, 52, not_text());
        assert_eq!(again, Ok(None), "the failed version is consumed too");
        // Deck's own receipted write stays excluded, whatever was read
        state.wrote(53);
        assert_eq!(stable(&mut state, 53, text("own")), Ok(None));
    }
    #[test]
    fn self_write_rebaseline_excludes_own_change() {
        let mut state = Gate::default();
        state.arm(4);
        let old_generation = state.generation;
        state.disarm();
        state.arm(5); // Deck wrote the translation while disarmed
        assert!(!state.changed(5, state.generation));
        assert!(!state.changed(6, old_generation));
        assert!(state.changed(6, state.generation));
    }
    #[test]
    fn g04_only_the_receipted_self_write_is_excluded() {
        let mut state = Gate::default();
        state.arm(10);
        let generation = state.generation;
        state.wrote(11); // Copy Translation
        assert!(!state.changed(11, generation), "own write never feeds back");
        assert!(state.changed(12, generation), "the next real copy is read");
    }
    #[test]
    fn g06_a_real_copy_right_after_a_self_write_is_not_swallowed() {
        let mut state = Gate::default();
        state.arm(20);
        let generation = state.generation;
        state.wrote(21);
        // an external copy landed before the poll: the poll sees 22
        assert!(state.changed(22, generation));
        assert!(!state.changed(22, generation));
        // the stale receipt cannot hide any later version either
        assert!(state.changed(23, generation));
    }
    #[test]
    fn g07_receipts_never_cross_an_armed_cycle() {
        let mut state = Gate::default();
        state.arm(30);
        state.wrote(31);
        state.disarm(); // mode switch, close, focus loss
        state.arm(31); // new cycle: baseline covers the old version, nothing read
        let generation = state.generation;
        assert!(state.own.is_none());
        assert!(state.changed(32, generation), "a new-cycle copy is read");
        let mut unarmed = Gate::default();
        unarmed.wrote(5); // Live mode copy: not observing, nothing recorded
        assert!(unarmed.own.is_none());
    }
}
