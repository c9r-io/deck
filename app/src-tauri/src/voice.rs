//! One native recorder bound to one pane; committed text is typed straight in.
//! Native snapshots are pulled only by the invoking webview: no audio files,
//! transcript logs, broadcast transcript events, subprocesses or cloud fallback.
//! A snapshot's text is the committed transcript so far (a growing prefix);
//! the webview pastes each new slice while capture is still running, so
//! delivery is allowed during a busy recording. Bind pins session/pane
//! identity, not the foreground program; every paste probes the current
//! program and pins identity + program atomically. Text is byte-literal:
//! no trimming (a slice may begin with the space between two words), never
//! an Enter. A changed generation returns target-expired; the UI ends the
//! recording rather than rebinding. Shared scheduler exclusion prevents
//! concurrent sends into one session.
use crate::context::RawProbe;
use crate::error::{DeckError, ErrorKind};
use crate::prompt_delivery::{self, LiteralOutcome, LiteralRequest};
use crate::scheduler::{self, Queues};
use crate::sync::LockRecover;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString};
use std::sync::{Mutex, OnceLock};

pub(crate) const SUPPORTED_LANGUAGES: &[&str] = &[
    "zh-CN", "zh-TW", "en-US", "ja-JP", "ko-KR", "de-DE", "fr-FR", "es-ES",
];

#[derive(Clone, Default, Serialize)]
pub(crate) struct Snapshot {
    id: u64,
    status: String,
    /// Committed transcript prefix; grows until `ready`, never revised.
    text: String,
    /// Volatile tail shown as a caption; never typed, never logged.
    preview: String,
    code: String,
}
impl Snapshot {
    fn busy(&self) -> bool {
        matches!(
            self.status.as_str(),
            "preparing" | "downloading" | "recording" | "stopping"
        )
    }
}
#[derive(Clone)]
struct Target {
    id: u64,
    session: String,
    probe: RawProbe,
}
#[derive(Default)]
struct Voice {
    serial: u64,
    snapshot: Snapshot,
    targets: HashMap<String, Target>,
}
static VOICE: OnceLock<Mutex<Voice>> = OnceLock::new();
fn voice() -> &'static Mutex<Voice> {
    VOICE.get_or_init(|| Mutex::new(Voice::default()))
}
fn failure(code: &'static str) -> DeckError {
    let kind = match code {
        "language-invalid" | "text-invalid" | "multiline-unsupported" => ErrorKind::Invalid,
        "target-changed" | "target-expired" => ErrorKind::ContextChanged,
        "target-unavailable" | "recording-expired" => ErrorKind::Missing,
        "voice-busy" | "delivery-busy" => ErrorKind::Locked,
        _ => ErrorKind::Other,
    };
    DeckError::new(kind, code)
}

#[cfg(target_os = "macos")]
extern "C" {
    fn deck_speech_start(
        id: u64,
        locale: *const std::ffi::c_char,
        callback: extern "C" fn(
            u64,
            *const std::ffi::c_char,
            *const std::ffi::c_char,
            *const std::ffi::c_char,
            *const std::ffi::c_char,
        ),
    );
    fn deck_speech_stop(id: u64);
    fn deck_speech_cancel(id: u64);
}

extern "C" fn snapshot_callback(
    id: u64,
    status: *const std::ffi::c_char,
    text: *const std::ffi::c_char,
    preview: *const std::ffi::c_char,
    code: *const std::ffi::c_char,
) {
    accept_snapshot(voice(), id, status, text, preview, code);
}
fn accept_snapshot(
    state: &Mutex<Voice>,
    id: u64,
    status: *const std::ffi::c_char,
    text: *const std::ffi::c_char,
    preview: *const std::ffi::c_char,
    code: *const std::ffi::c_char,
) {
    // Swift lends valid NUL-terminated strings for this synchronous callback.
    if status.is_null() || text.is_null() || preview.is_null() || code.is_null() {
        return;
    }
    let mut v = state.lock_or_recover();
    if v.snapshot.id != id || !v.snapshot.busy() {
        return;
    }
    unsafe {
        let status = CStr::from_ptr(status).to_string_lossy().into_owned();
        // A stop is acknowledged here before the native side reaches it; a
        // capture-phase callback in between must not revive the recording.
        if !(v.snapshot.status == "stopping"
            && matches!(status.as_str(), "preparing" | "downloading" | "recording"))
        {
            v.snapshot.status = status;
        }
        v.snapshot.text = CStr::from_ptr(text)
            .to_string_lossy()
            .chars()
            .take(65_536)
            .collect();
        v.snapshot.preview = CStr::from_ptr(preview)
            .to_string_lossy()
            .chars()
            .take(4_096)
            .collect();
        v.snapshot.code = CStr::from_ptr(code).to_string_lossy().into_owned();
    }
    // While preparing, the code names the recognizer that will run: a closed
    // word for app.log so "typed only at stop" can be told apart from a slow
    // finalization. It never reaches the snapshot as an error code.
    if v.snapshot.status == "preparing" {
        match v.snapshot.code.as_str() {
            "engine-modern" => crate::applog::applog("[voice] engine=modern"),
            "engine-legacy" => crate::applog::applog("[voice] engine=legacy"),
            _ => {}
        }
        v.snapshot.code.clear();
    }
}

#[derive(Serialize)]
pub(crate) struct Binding {
    id: u64,
    process: Option<String>,
}

#[tauri::command]
pub(crate) async fn voice_bind(name: String) -> Result<Binding, DeckError> {
    tauri::async_runtime::spawn_blocking(move || {
        bind_with(voice(), name, &prompt_delivery::TmuxTransport)
    })
    .await
    .map_err(|_| failure("target-unavailable"))?
}

fn bind_with(
    state: &Mutex<Voice>,
    name: String,
    transport: &impl prompt_delivery::Transport,
) -> Result<Binding, DeckError> {
    crate::tmux::validate_session_name(&name)?;
    let mut v = state.lock_or_recover();
    if v.snapshot.busy() {
        return Err(failure("voice-busy"));
    }
    let probe = transport
        .probe(&name)
        .map_err(|_| failure("target-unavailable"))?;
    if probe.foreground.is_none() {
        return Err(failure("target-unavailable"));
    }
    v.serial += 1;
    let result = Binding {
        id: v.serial,
        process: probe.foreground_name(),
    };
    v.targets.insert(
        name.clone(),
        Target {
            id: result.id,
            session: name,
            probe,
        },
    );
    Ok(result)
}

trait Speech {
    fn start(&self, id: u64, locale: &str);
    fn stop(&self, id: u64);
    fn cancel(&self, id: u64);
}
struct NativeSpeech;
impl Speech for NativeSpeech {
    fn start(&self, id: u64, locale: &str) {
        #[cfg(target_os = "macos")]
        unsafe {
            deck_speech_start(
                id,
                CString::new(locale).unwrap().as_ptr(),
                snapshot_callback,
            );
        }
    }
    fn stop(&self, id: u64) {
        #[cfg(target_os = "macos")]
        unsafe {
            deck_speech_stop(id);
        }
    }
    fn cancel(&self, id: u64) {
        #[cfg(target_os = "macos")]
        unsafe {
            deck_speech_cancel(id);
        }
    }
}

#[tauri::command]
pub(crate) fn voice_start(target_id: u64, locale: String) -> Result<u64, DeckError> {
    start_with(voice(), target_id, &locale, &NativeSpeech)
}
fn start_with(
    state: &Mutex<Voice>,
    target_id: u64,
    locale: &str,
    speech: &impl Speech,
) -> Result<u64, DeckError> {
    if !SUPPORTED_LANGUAGES.contains(&locale) {
        return Err(failure("language-invalid"));
    }
    let mut v = state.lock_or_recover();
    if v.snapshot.busy() {
        return Err(failure("voice-busy"));
    }
    if !v.targets.values().any(|t| t.id == target_id) {
        return Err(failure("target-unavailable"));
    }
    v.serial += 1;
    let id = v.serial;
    v.snapshot = Snapshot {
        id,
        status: "preparing".into(),
        ..Snapshot::default()
    };
    speech.start(id, locale);
    #[cfg(not(target_os = "macos"))]
    {
        v.snapshot.status = "error".into();
        v.snapshot.code = "local-unavailable".into();
    }
    Ok(id)
}

#[tauri::command]
pub(crate) fn voice_snapshot(id: u64) -> Result<Snapshot, DeckError> {
    snapshot_with(voice(), id)
}
fn snapshot_with(state: &Mutex<Voice>, id: u64) -> Result<Snapshot, DeckError> {
    let v = state.lock_or_recover();
    if v.snapshot.id != id {
        return Err(failure("recording-expired"));
    }
    Ok(v.snapshot.clone())
}

#[tauri::command]
pub(crate) fn voice_stop(id: u64) {
    stop_with(voice(), id, &NativeSpeech);
}
fn stop_with(state: &Mutex<Voice>, id: u64, speech: &impl Speech) {
    let mut v = state.lock_or_recover();
    if v.snapshot.id == id && v.snapshot.busy() {
        v.snapshot.status = "stopping".into();
        speech.stop(id);
    }
}

#[tauri::command]
pub(crate) fn voice_cancel(id: u64) {
    cancel_with(voice(), id, &NativeSpeech);
}
fn cancel_with(state: &Mutex<Voice>, id: u64, speech: &impl Speech) {
    let mut v = state.lock_or_recover();
    if id == 0 || v.snapshot.id == id {
        v.snapshot.status = "cancelled".into();
        v.snapshot.text.clear();
        v.snapshot.preview.clear();
    }
    speech.cancel(id);
}

#[tauri::command]
pub(crate) async fn voice_deliver(
    app: tauri::AppHandle,
    target_id: u64,
    text: String,
) -> Result<(), DeckError> {
    use tauri::Manager;
    tauri::async_runtime::spawn_blocking(move || {
        let queues = app.state::<Queues>();
        deliver_with(
            voice(),
            &queues.busy,
            target_id,
            text,
            &prompt_delivery::TmuxTransport,
        )
    })
    .await
    .map_err(|_| failure("delivery-unknown"))?
}

fn deliver_with(
    state: &Mutex<Voice>,
    busy: &Mutex<HashSet<String>>,
    target_id: u64,
    text: String,
    transport: &impl prompt_delivery::Transport,
) -> Result<(), DeckError> {
    if text.trim().is_empty()
        || text.len() > 65_536
        || text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err(failure("text-invalid"));
    }
    // Byte-literal apart from line endings: the webview already turned line
    // breaks into spaces, and a slice keeps its leading word separator.
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let target = {
        let v = state.lock_or_recover();
        let target = v
            .targets
            .values()
            .find(|t| t.id == target_id)
            .ok_or_else(|| failure("target-unavailable"))?;
        if !scheduler::claim_session(busy, &target.session) {
            return Err(failure("delivery-busy"));
        }
        target.clone()
    };
    // RAII releases the same exclusion used by scheduled prompt workers.
    struct Release<'a>(&'a Mutex<HashSet<String>>, &'a str);
    impl Drop for Release<'_> {
        fn drop(&mut self) {
            scheduler::release_session(self.0, self.1);
        }
    }
    let _release = Release(busy, &target.session);
    let current = transport
        .probe(&target.session)
        .map_err(|_| failure("target-changed"))?;
    if current.identity != target.probe.identity {
        return Err(failure("target-expired"));
    }
    if current.foreground.is_none() {
        return Err(failure("target-unavailable"));
    }
    if text.contains('\n') {
        let paste = transport.run(&[
            "display-message".into(),
            "-p".into(),
            "-t".into(),
            target.probe.identity.pane_id.clone(),
            "#{bracketed_paste_flag}".into(),
        ])?;
        if paste.trim() != "1" {
            return Err(failure("multiline-unsupported"));
        }
    }
    let outcome = prompt_delivery::deliver_with(
        LiteralRequest {
            session: &target.session,
            pane: &target.probe.identity,
            expected_process: current.foreground.as_deref(),
            delivery: &format!("voice-{}", scheduler::next_delivery_id()),
            text: &text,
            submit: false,
            require_bracketed: true,
        },
        transport,
    );
    // A transport failure can happen after the paste. Never auto-retry it.
    match outcome {
        Ok(LiteralOutcome::Inserted) => Ok(()),
        Ok(_) => Err(failure("delivery-unknown")),
        Err(error) if error == "target-changed" => Err(failure("target-changed")),
        Err(_) => Err(failure("delivery-unknown")),
    }
}

#[cfg(test)]
mod tests;
