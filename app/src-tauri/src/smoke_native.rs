//! Debug-only native driver for the isolated Local Translation smoke.
//! Every command refuses unless `smoke_faults::enabled()` (a debug build
//! launched with `--smoke-wkwebview` and an absolute `--smoke-data-dir`);
//! a release build has no native implementation at all
//! (`native/SmokeBridge.swift` is compiled only into debug profiles).
//!
//! It acts only on Deck's own process: AppKit events are handed to Deck's
//! own window (never posted to the system or another process, so no
//! Accessibility/TCC grant is used), snapshots come from Deck's own
//! WKWebView into the isolated data directory, the window's enforced minimum
//! size is read from Deck's own NSWindow (`window_min`), and a pasteboard guard
//! (general, or a test-owned named board) refuses every test write unless it
//! holds a stable backup and the board is still at its owned version; see the
//! contract in `native/SmokeBridge.swift`. Settling always disarms Copied-text
//! observation first, so a restored original is never read by the Lens.
//! A watcher thread lets the driver settle the guard (`settle-request` →
//! `settle-result` in the fixture directory) before it terminates this
//! process. Nothing here spawns a process, reads content back to JS, or logs
//! text.
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

/// Debug-only LaunchServices inventory/withdrawal, before Board initialization.
/// The same signed test identity observes UN state without rearming requests.
pub(crate) fn reminder_maintenance() -> bool {
    if !cfg!(debug_assertions) || gate().is_err() {
        return false;
    }
    let scenario = std::fs::read_to_string(fixture_dir().join("scenario")).unwrap_or_default();
    if !matches!(scenario.trim(), "native-inventory" | "native-cleanup") {
        return false;
    }
    #[cfg(all(debug_assertions, target_os = "macos"))]
    unsafe {
        extern "C" {
            fn deck_smoke_reminder_inventory() -> *mut std::ffi::c_char;
            fn deck_smoke_reminder_withdraw() -> i32;
        }
        let cleanup = scenario.trim() == "native-cleanup";
        if cleanup {
            deck_smoke_reminder_withdraw();
        }
        for _ in 0..20 {
            let ptr = deck_smoke_reminder_inventory();
            if ptr.is_null() {
                break;
            }
            let bytes = std::ffi::CStr::from_ptr(ptr).to_bytes().to_vec();
            libc::free(ptr.cast());
            if let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                value["observedAt"] = serde_json::json!(crate::reminder::now_ms());
                value["boardLoaded"] = serde_json::json!(false);
                let empty = value["pending"].as_array().is_some_and(Vec::is_empty)
                    && value["delivered"].as_array().is_some_and(Vec::is_empty);
                let directory = crate::datadir::deck_dir().join("evidence");
                let _ = crate::datadir::create_private_dir(&directory);
                let _ = crate::datadir::write_private(
                    &directory.join("reminder-maintenance.json"),
                    &serde_json::to_vec(&value).unwrap_or_default(),
                );
                if !cleanup || empty {
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    true
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
        pub fn deck_smoke_roman_input() -> i32;
        pub fn deck_smoke_snapshot(path: *const c_char) -> i32;
        pub fn deck_smoke_window_min(out: *mut f64) -> i32;
        pub fn deck_smoke_pb_guard_begin(board: i32) -> i32;
        pub fn deck_smoke_pb_write(text: *const c_char) -> i64;
        pub fn deck_smoke_pb_permit() -> i64;
        pub fn deck_smoke_pb_adopt(receipt: i64) -> i64;
        pub fn deck_smoke_pb_guard_end() -> i32;
        pub fn deck_smoke_pb_state() -> i32;
        pub fn deck_smoke_pb_audit() -> *mut c_char;
        pub fn deck_smoke_pb_named(enable: i32) -> i32;
        pub fn deck_smoke_pb_named_write(kind: i32, text: *const c_char) -> i64;
        pub fn deck_smoke_pb_count(board: i32) -> i64;
        pub fn deck_smoke_pb_fail_restores(count: i32) -> i32;
        pub fn deck_smoke_pb_fail_next_fill() -> i32;
        pub fn deck_pasteboard_free(text: *mut c_char);
    }
}

/// Debug-only: the minimum content size AppKit enforces on a user resize of
/// Deck's window and its current content size, in points
/// `[min_w, min_h, w, h]` (`deck_smoke_window_min`); `None` outside an
/// isolated smoke or before the window exists.
pub(crate) fn window_min() -> Option<[f64; 4]> {
    gate().ok()?;
    #[cfg(all(debug_assertions, target_os = "macos"))]
    {
        let mut out = [0.0_f64; 4];
        // SAFETY: the bridge writes exactly four doubles into `out`.
        let code = unsafe { native::deck_smoke_window_min(out.as_mut_ptr()) };
        (code == 0).then_some(out)
    }
    #[cfg(not(all(debug_assertions, target_os = "macos")))]
    None
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

/// One native input step (closed `kind`; CSS coordinates in the webview).
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NativeInput {
    kind: String,
    x: Option<f64>,
    y: Option<f64>,
    dy: Option<i32>,
    text: Option<String>,
    key_code: Option<u16>,
    modifiers: Option<Vec<String>>,
    viewport: Option<f64>,
}

const MODIFIERS: &[(&str, u64)] = &[
    ("shift", 1 << 17),
    ("control", 1 << 18),
    ("option", 1 << 19),
    ("command", 1 << 20),
];

#[tauri::command]
pub(crate) async fn smoke_native_input(input: NativeInput) -> Result<i32, DeckError> {
    let NativeInput {
        kind,
        x,
        y,
        dy,
        text,
        key_code,
        modifiers,
        viewport,
    } = input;
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
                "roman" => native::deck_smoke_roman_input(),
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

/// Settle the guard after disarming Copied-text observation (so a restored
/// original is never read). Returns the bridge's result code.
#[cfg(all(debug_assertions, target_os = "macos"))]
fn settle() -> i64 {
    crate::intelligence::pasteboard::translation_clipboard_disarm();
    i64::from(unsafe { native::deck_smoke_pb_guard_end() })
}

#[cfg(all(debug_assertions, target_os = "macos"))]
fn audit() -> String {
    unsafe {
        let ptr = native::deck_smoke_pb_audit();
        if ptr.is_null() {
            return String::new();
        }
        let text = std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned();
        native::deck_pasteboard_free(ptr);
        text
    }
}

fn fixture_dir() -> std::path::PathBuf {
    crate::datadir::deck_dir().join("translation-fixture")
}

/// Test-only delay of the next driver settlement reply (named-board fault
/// probes: a reply that is not yet available must not be taken as absent).
static DELAY_NEXT_SETTLE_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Driver-side settlement: while this process lives, a `settle-request`
/// file (holding the driver's nonce) makes it settle the guard and write
/// `settle-result` as `<nonce>|<code>|<audit>` (numbers only), so a stale
/// reply can never stand in for this request. The driver waits for it before
/// it may terminate this process.
#[cfg(all(debug_assertions, target_os = "macos"))]
fn start_settle_watcher() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        std::thread::spawn(|| loop {
            std::thread::sleep(std::time::Duration::from_millis(100));
            let request = fixture_dir().join("settle-request");
            let Ok(nonce) = std::fs::read_to_string(&request) else {
                continue;
            };
            let _ = std::fs::remove_file(&request);
            let delay = DELAY_NEXT_SETTLE_MS.swap(0, std::sync::atomic::Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(delay));
            let code = settle();
            let nonce: String = nonce
                .chars()
                .filter(char::is_ascii_alphanumeric)
                .take(32)
                .collect();
            let staged = fixture_dir().join("settle-result.tmp");
            if std::fs::write(&staged, format!("{nonce}|{code}|{}", audit())).is_ok() {
                let _ = std::fs::rename(&staged, fixture_dir().join("settle-result"));
            }
        });
    });
}

/// Pasteboard guard and the test-owned named pasteboard. Returns the
/// bridge's closed numeric result; never any pasteboard content.
#[tauri::command]
pub(crate) async fn smoke_pasteboard(
    action: String,
    text: Option<String>,
    board: Option<i32>,
    receipt: Option<i64>,
) -> Result<i64, DeckError> {
    blocking(move || {
        let text = c_text(text.as_deref().unwrap_or(""))?;
        #[cfg(all(debug_assertions, target_os = "macos"))]
        unsafe {
            start_settle_watcher();
            let board = board.unwrap_or(0).clamp(0, 1);
            Ok(match action.as_str() {
                "guard-begin" => i64::from(native::deck_smoke_pb_guard_begin(board)),
                "write" => native::deck_smoke_pb_write(text.as_ptr()),
                "permit" => native::deck_smoke_pb_permit(),
                "adopt" => native::deck_smoke_pb_adopt(receipt.unwrap_or(-1)),
                "guard-end" => settle(),
                "state" => i64::from(native::deck_smoke_pb_state()),
                "count" => native::deck_smoke_pb_count(board),
                "named-on" => i64::from(native::deck_smoke_pb_named(1)),
                "named-off" => i64::from(native::deck_smoke_pb_named(0)),
                "named-text" => native::deck_smoke_pb_named_write(0, text.as_ptr()),
                "named-data" => native::deck_smoke_pb_named_write(1, text.as_ptr()),
                "named-empty" => native::deck_smoke_pb_named_write(2, text.as_ptr()),
                "named-multi" => native::deck_smoke_pb_named_write(3, text.as_ptr()),
                "named-lazy" => native::deck_smoke_pb_named_write(4, text.as_ptr()),
                "named-clear" => native::deck_smoke_pb_named_write(5, text.as_ptr()),
                // named-board faults; the bridge refuses them while a general guard is active
                "fail-restores" => i64::from(native::deck_smoke_pb_fail_restores(
                    receipt.unwrap_or(1).clamp(0, 99) as i32,
                )),
                "fail-next-fill" => i64::from(native::deck_smoke_pb_fail_next_fill()),
                "delay-next-settle" => {
                    let ms = receipt.unwrap_or(0).clamp(0, 10_000) as u64;
                    DELAY_NEXT_SETTLE_MS.store(ms, std::sync::atomic::Ordering::SeqCst);
                    0
                }
                _ => return Err(unavailable()),
            })
        }
        #[cfg(not(all(debug_assertions, target_os = "macos")))]
        {
            let _ = (action, text, board, receipt);
            Err(unavailable())
        }
    })
    .await
}

/// Content-free guard audit: `id,board,writes,rejects,reason,result;…`.
#[tauri::command]
pub(crate) async fn smoke_pasteboard_audit() -> Result<String, DeckError> {
    blocking(|| {
        #[cfg(all(debug_assertions, target_os = "macos"))]
        {
            Ok(audit())
        }
        #[cfg(not(all(debug_assertions, target_os = "macos")))]
        {
            Err(unavailable())
        }
    })
    .await
}

/// The /copy fixture's (or driver's) write receipt: the changeCount its own
/// clearContents() returned, or -1 when it refused or wrote nothing. Taking
/// it removes it, so a receipt is used once.
#[tauri::command]
pub(crate) fn smoke_native_fixture_receipt() -> Result<i64, DeckError> {
    gate()?;
    let path = fixture_dir().join("copy-receipt");
    let value = std::fs::read_to_string(&path).ok();
    let _ = std::fs::remove_file(&path);
    Ok(value
        .and_then(|text| text.trim().parse::<i64>().ok())
        .unwrap_or(-1))
}

/// The driver's closed scenario word for this launch (`translation-fixture/
/// scenario`), or "" — the guard mode's fault probes read it.
#[tauri::command]
pub(crate) fn smoke_native_scenario() -> Result<String, DeckError> {
    gate()?;
    let text = std::fs::read_to_string(fixture_dir().join("scenario")).unwrap_or_default();
    Ok(text
        .trim()
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-' || *c == ':')
        .take(64)
        .collect())
}

/// Where the isolated driver placed the deterministic `/copy` CLI fixture.
#[tauri::command]
pub(crate) fn smoke_native_fixture() -> Result<String, DeckError> {
    gate()?;
    let path = fixture_dir().join("copy_agent.py");
    if !path.is_file() {
        return Err(unavailable());
    }
    path.to_str().map(str::to_owned).ok_or_else(unavailable)
}

/// Actual system inventory, scoped to the dedicated Reminder carrier. A
/// request or delivered item is not evidence that a user saw a banner.
#[tauri::command]
pub(crate) async fn smoke_reminder_inventory() -> Result<serde_json::Value, DeckError> {
    blocking(|| {
        #[cfg(all(debug_assertions, target_os = "macos"))]
        unsafe {
            extern "C" {
                fn deck_smoke_reminder_inventory() -> *mut std::ffi::c_char;
            }
            let ptr = deck_smoke_reminder_inventory();
            if ptr.is_null() {
                return Err(unavailable());
            }
            let bytes = std::ffi::CStr::from_ptr(ptr).to_bytes().to_vec();
            libc::free(ptr.cast());
            let mut value: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
            value["observedAt"] = serde_json::json!(crate::reminder::now_ms());
            let home = crate::datadir::deck_dir().join("home");
            value["privateEnvironment"] = serde_json::json!({
                "home": std::env::var_os("HOME").is_some_and(|p| p == home),
                "claude": std::env::var_os("CLAUDE_CONFIG_DIR").is_some_and(|p| p == home.join(".claude")),
                "codex": std::env::var_os("CODEX_HOME").is_some_and(|p| p == home.join(".codex")),
            });
            let directory = crate::datadir::deck_dir().join("evidence");
            crate::datadir::create_private_dir(&directory)?;
            crate::datadir::write_private(
                &directory.join("reminder-system-inventory.json"),
                &serde_json::to_vec(&value).map_err(|_| unavailable())?,
            )?;
            Ok(value)
        }
        #[cfg(not(all(debug_assertions, target_os = "macos")))]
        Err(unavailable())
    })
    .await
}
#[tauri::command]
pub(crate) async fn smoke_reminder_withdraw() -> Result<i32, DeckError> {
    blocking(|| {
        #[cfg(all(debug_assertions, target_os = "macos"))]
        unsafe {
            extern "C" {
                fn deck_smoke_reminder_withdraw() -> i32;
            }
            Ok(deck_smoke_reminder_withdraw())
        }
        #[cfg(not(all(debug_assertions, target_os = "macos")))]
        Err(unavailable())
    })
    .await
}

#[cfg(test)]
mod tests {
    #[test]
    fn refuses_outside_an_isolated_smoke_launch() {
        assert!(super::gate().is_err());
        assert!(super::smoke_native_fixture().is_err());
    }
}
