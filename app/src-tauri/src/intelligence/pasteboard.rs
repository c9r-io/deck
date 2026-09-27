//! Clipboard authority: automatic reads require an armed, focused Lens and
//! a post-baseline change. Native AppKit focus checks are repeated at read
//! time. Focus loss clears ownership; a new arm only records changeCount.
use crate::error::{DeckError, ErrorKind};
use crate::sync::LockRecover;
use std::ffi::{c_char, CStr};
use std::sync::{Mutex, OnceLock};

#[derive(Default, Clone, Copy)]
struct Gate {
    armed: bool,
    baseline: i64,
    generation: u64,
}
impl Gate {
    fn arm(&mut self, count: i64) {
        self.armed = true;
        self.baseline = count;
        self.generation = self.generation.wrapping_add(1);
    }
    fn disarm(&mut self) {
        self.armed = false;
        self.generation = self.generation.wrapping_add(1);
    }
    fn changed(&mut self, count: i64, generation: u64) -> bool {
        if !self.armed || self.generation != generation || count <= self.baseline {
            return false;
        }
        self.baseline = count;
        true
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
    fn deck_pasteboard_read_text() -> *mut c_char;
    fn deck_pasteboard_free(text: *mut c_char);
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
fn read_text() -> Result<String, DeckError> {
    #[cfg(target_os = "macos")]
    {
        let ptr = unsafe { deck_pasteboard_read_text() };
        if ptr.is_null() {
            return Err(error("clipboard-not-text"));
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
        Ok(text)
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
    let text = read_text();
    // A second native focus/count probe closes a race with focus loss and a
    // second clipboard write while the AppKit read was in flight.
    let stable = focused_count()? == count;
    let mut state = locked();
    if !stable || !state.changed(count, generation) {
        return Ok(None);
    }
    text.map(Some)
}

#[tauri::command]
pub(crate) fn translation_clipboard_current() -> Result<String, DeckError> {
    if !crate::documents::local_translation_settings().0 {
        return Err(error("translation-disabled"));
    }
    let generation = {
        let state = locked();
        if !state.armed {
            return Err(error("clipboard-unavailable"));
        }
        state.generation
    };
    let before = focused_count()?;
    let text = read_text()?;
    if focused_count()? != before {
        return Err(error("clipboard-unavailable"));
    }
    let state = locked();
    if !state.armed || state.generation != generation {
        return Err(error("clipboard-unavailable"));
    }
    Ok(text)
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
}
