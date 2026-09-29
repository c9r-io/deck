//! Debug-only native driver for the isolated Local Translation smoke.
//! Every command refuses unless `smoke_faults::enabled()` (a debug build
//! launched with `--smoke-wkwebview` and an absolute `--smoke-data-dir`);
//! a release build has no native implementation at all
//! (`native/SmokeBridge.swift` is compiled only into debug profiles).
//!
//! It acts only on Deck's own process: AppKit events are handed to Deck's
//! own window (never posted to the system or another process, so no
//! Accessibility/TCC grant is used), snapshots come from Deck's own
//! WKWebView into the isolated data directory, and the shared general
//! pasteboard is guarded — its original items stay in process memory and
//! are restored only while the current change is still test-owned. Nothing
//! here spawns a process, reads content back to JS, or logs text.
use crate::error::{DeckError, ErrorKind};

fn unavailable() -> DeckError {
    DeckError::new(ErrorKind::Other, "smoke native driver is unavailable")
}
fn gate() -> Result<(), DeckError> {
    if crate::smoke_faults::enabled() {
        Ok(())
    } else {
        Err(unavailable())
    }
}

#[cfg(all(debug_assertions, target_os = "macos"))]
mod native {
    use std::ffi::c_char;
    unsafe extern "C" {
        pub fn deck_smoke_mouse(kind: i32, x: f64, y: f64, clicks: i32) -> i32;
        pub fn deck_smoke_scroll(x: f64, y: f64, dy: i32) -> i32;
        pub fn deck_smoke_key(chars: *const c_char, key_code: u16, modifiers: u64) -> i32;
        pub fn deck_smoke_app(action: i32) -> i32;
        pub fn deck_smoke_viewport(height: f64);
        pub fn deck_smoke_snapshot(path: *const c_char) -> i32;
        pub fn deck_smoke_pb_guard_begin() -> i32;
        pub fn deck_smoke_pb_write(text: *const c_char) -> i64;
        pub fn deck_smoke_pb_claim(expected: *const c_char) -> i32;
        pub fn deck_smoke_pb_guard_end() -> i32;
        pub fn deck_smoke_pb_named(enable: i32) -> i32;
        pub fn deck_smoke_pb_named_write(kind: i32, text: *const c_char) -> i64;
    }
}

fn c_text(text: &str) -> Result<std::ffi::CString, DeckError> {
    std::ffi::CString::new(text).map_err(|_| unavailable())
}

/// Runs a native call off the IPC thread; AppKit work hops to the main
/// thread inside the bridge.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, DeckError> + Send + 'static,
) -> Result<T, DeckError> {
    gate()?;
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| unavailable())?
}

const MODIFIERS: &[(&str, u64)] = &[
    ("shift", 1 << 17),
    ("control", 1 << 18),
    ("option", 1 << 19),
    ("command", 1 << 20),
];

#[tauri::command]
pub(crate) async fn smoke_native_input(
    kind: String,
    x: Option<f64>,
    y: Option<f64>,
    dy: Option<i32>,
    text: Option<String>,
    key_code: Option<u16>,
    modifiers: Option<Vec<String>>,
    viewport: Option<f64>,
) -> Result<i32, DeckError> {
    blocking(move || {
        let (x, y) = (x.unwrap_or(0.0), y.unwrap_or(0.0));
        let flags = modifiers
            .unwrap_or_default()
            .iter()
            .map(|name| {
                MODIFIERS
                    .iter()
                    .find(|(known, _)| *known == name)
                    .map(|(_, bit)| *bit)
                    .ok_or_else(unavailable)
            })
            .sum::<Result<u64, DeckError>>()?;
        let text = c_text(text.as_deref().unwrap_or(""))?;
        #[cfg(all(debug_assertions, target_os = "macos"))]
        unsafe {
            if let Some(height) = viewport {
                native::deck_smoke_viewport(height);
            }
            Ok(match kind.as_str() {
                "down" => native::deck_smoke_mouse(0, x, y, 1),
                "drag" => native::deck_smoke_mouse(1, x, y, 1),
                "up" => native::deck_smoke_mouse(2, x, y, 1),
                "scroll" => native::deck_smoke_scroll(x, y, dy.unwrap_or(0)),
                "key" => native::deck_smoke_key(text.as_ptr(), key_code.unwrap_or(0), flags),
                "hide" => native::deck_smoke_app(0),
                "state" => native::deck_smoke_app(1),
                "activate" => native::deck_smoke_app(2),
                _ => return Err(unavailable()),
            })
        }
        #[cfg(not(all(debug_assertions, target_os = "macos")))]
        {
            let _ = (kind, x, y, dy, text, key_code, flags, viewport);
            Err(unavailable())
        }
    })
    .await
}

/// Writes `<smoke data dir>/evidence/<name>.png` (name: `[a-z0-9-]{1,48}`).
#[tauri::command]
pub(crate) async fn smoke_native_snapshot(name: String) -> Result<i32, DeckError> {
    if name.is_empty()
        || name.len() > 48
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(unavailable());
    }
    blocking(move || {
        let dir = crate::datadir::deck_dir().join("evidence");
        crate::datadir::create_private_dir(&dir)?;
        let path = dir.join(format!("{name}.png"));
        let path = c_text(path.to_str().ok_or_else(unavailable)?)?;
        #[cfg(all(debug_assertions, target_os = "macos"))]
        unsafe {
            Ok(native::deck_smoke_snapshot(path.as_ptr()))
        }
        #[cfg(not(all(debug_assertions, target_os = "macos")))]
        {
            let _ = path;
            Err(unavailable())
        }
    })
    .await
}

/// Pasteboard guard and the test-owned named pasteboard. Returns the
/// bridge's closed numeric result; never any pasteboard content.
#[tauri::command]
pub(crate) async fn smoke_pasteboard(
    action: String,
    text: Option<String>,
) -> Result<i64, DeckError> {
    blocking(move || {
        let text = c_text(text.as_deref().unwrap_or(""))?;
        #[cfg(all(debug_assertions, target_os = "macos"))]
        unsafe {
            Ok(match action.as_str() {
                "guard-begin" => i64::from(native::deck_smoke_pb_guard_begin()),
                "write" => native::deck_smoke_pb_write(text.as_ptr()),
                "claim" => i64::from(native::deck_smoke_pb_claim(text.as_ptr())),
                "guard-end" => i64::from(native::deck_smoke_pb_guard_end()),
                "named-on" => i64::from(native::deck_smoke_pb_named(1)),
                "named-off" => i64::from(native::deck_smoke_pb_named(0)),
                "named-text" => native::deck_smoke_pb_named_write(0, text.as_ptr()),
                "named-data" => native::deck_smoke_pb_named_write(1, text.as_ptr()),
                "named-empty" => native::deck_smoke_pb_named_write(2, text.as_ptr()),
                _ => return Err(unavailable()),
            })
        }
        #[cfg(not(all(debug_assertions, target_os = "macos")))]
        {
            let _ = (action, text);
            Err(unavailable())
        }
    })
    .await
}

/// Where the isolated driver placed the deterministic `/copy` CLI fixture.
#[tauri::command]
pub(crate) fn smoke_native_fixture() -> Result<String, DeckError> {
    gate()?;
    let path = crate::datadir::deck_dir().join("translation-fixture/copy_agent.py");
    if !path.is_file() {
        return Err(unavailable());
    }
    path.to_str().map(str::to_owned).ok_or_else(unavailable)
}

#[cfg(test)]
mod tests {
    #[test]
    fn refuses_outside_an_isolated_smoke_launch() {
        assert!(super::gate().is_err());
        assert!(super::smoke_native_fixture().is_err());
    }
}
