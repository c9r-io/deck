//! Selected macOS input source for the session header. The Swift bridge owns
//! the one distributed notification observer; its callback carries no borrowed
//! data. This module orders native reads and emits only a copied name.

use serde::{Deserialize, Serialize};
use std::ffi::{c_char, CStr};
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Emitter};

use crate::sync::LockRecover;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InputSourceView {
    name: Option<String>,
    icon: Option<String>,
    sequence: u64,
}

#[derive(Deserialize)]
struct NativeInputSourceSnapshot {
    name: Option<String>,
    icon: Option<String>,
}

static APP: OnceLock<AppHandle> = OnceLock::new();
static SEQUENCE: Mutex<u64> = Mutex::new(0);

#[cfg(target_os = "macos")]
extern "C" {
    fn deck_input_source_init(callback: extern "C" fn()) -> i32;
    fn deck_input_source_copy_snapshot() -> *mut c_char;
    fn deck_input_source_free_snapshot(pointer: *mut c_char);
}

#[cfg(target_os = "macos")]
fn native_snapshot() -> Option<NativeInputSourceSnapshot> {
    // The returned pointer belongs to Swift until free_snapshot; no borrowed
    // pointer survives this function or enters Tauri's event queue.
    let pointer = unsafe { deck_input_source_copy_snapshot() };
    if pointer.is_null() {
        return None;
    }
    let snapshot = unsafe { CStr::from_ptr(pointer) }
        .to_str()
        .ok()
        .and_then(|json| serde_json::from_str::<NativeInputSourceSnapshot>(json).ok());
    unsafe { deck_input_source_free_snapshot(pointer) };
    snapshot
}

#[cfg(not(target_os = "macos"))]
fn native_snapshot() -> Option<NativeInputSourceSnapshot> {
    None
}

extern "C" fn changed() {
    // Notifications may arrive off the Tauri UI thread. Query and copy under
    // the same lock as commands, then send an owned payload via AppHandle.
    let view = input_source_snapshot();
    if let Some(app) = APP.get() {
        let _ = app.emit("input-source-changed", view);
    }
}

pub(crate) fn init(app: AppHandle) {
    if APP.set(app).is_err() {
        return;
    }
    #[cfg(target_os = "macos")]
    unsafe {
        let _ = deck_input_source_init(changed);
    }
}

#[tauri::command]
pub(crate) fn input_source_snapshot() -> InputSourceView {
    // Serialize the native read with callbacks: a snapshot read before a
    // change cannot be assigned a newer sequence than that change's event.
    let mut sequence = SEQUENCE.lock_or_recover();
    let native = native_snapshot();
    *sequence = sequence.saturating_add(1);
    InputSourceView {
        name: native
            .as_ref()
            .and_then(|view| view.name.clone())
            .filter(|name| !name.is_empty()),
        icon: native
            .and_then(|view| view.icon)
            .filter(|icon| icon.starts_with("data:image/png;base64,") && icon.len() <= 16_384),
        sequence: *sequence,
    }
}

pub(crate) fn resync() {
    let view = input_source_snapshot();
    if let Some(app) = APP.get() {
        let _ = app.emit("input-source-changed", view);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_advance_sequence_without_requiring_a_name() {
        let first = input_source_snapshot();
        let second = input_source_snapshot();
        assert!(second.sequence > first.sequence);
        #[cfg(not(target_os = "macos"))]
        assert!(first.name.is_none());
    }
}
