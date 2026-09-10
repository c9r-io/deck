//! One native recorder with volatile per-session target bindings.
//! Native snapshots are pulled only by the invoking webview: no audio files,
//! transcript logs, broadcast transcript events, subprocesses or cloud fallback.
//! Bind pins session/pane identity, not the foreground program. Every explicit
//! delivery probes the current program and pins it through paste and Enter.
//! Errors retain the session binding; the UI requires a distinct retry action
//! for uncertain delivery. Shared scheduler exclusion prevents concurrent sends.
use crate::context::{self, RawProbe};
use crate::error::{DeckError, ErrorKind};
use crate::prompt_delivery::{self, LiteralOutcome, LiteralRequest};
use crate::scheduler::{self, Queues};
use crate::sync::LockRecover;
use serde::Serialize;
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::sync::{Mutex, OnceLock};

pub(crate) const SUPPORTED_LANGUAGES: &[&str] = &[
    "zh-CN", "zh-TW", "en-US", "ja-JP", "ko-KR", "de-DE", "fr-FR", "es-ES",
];

#[derive(Clone, Default, Serialize)]
pub(crate) struct Snapshot {
    id: u64,
    status: String,
    text: String,
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
        "target-changed" => ErrorKind::ContextChanged,
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
        ),
    );
    fn deck_speech_stop(id: u64);
    fn deck_speech_cancel(id: u64);
}

extern "C" fn snapshot_callback(
    id: u64,
    status: *const std::ffi::c_char,
    text: *const std::ffi::c_char,
    code: *const std::ffi::c_char,
) {
    // Swift lends valid NUL-terminated strings for this synchronous callback.
    if status.is_null() || text.is_null() || code.is_null() {
        return;
    }
    let mut v = voice().lock_or_recover();
    if v.snapshot.id != id || !v.snapshot.busy() {
        return;
    }
    unsafe {
        v.snapshot.status = CStr::from_ptr(status).to_string_lossy().into_owned();
        v.snapshot.text = CStr::from_ptr(text)
            .to_string_lossy()
            .chars()
            .take(65_536)
            .collect();
        v.snapshot.code = CStr::from_ptr(code).to_string_lossy().into_owned();
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
        crate::tmux::validate_session_name(&name)?;
        let mut v = voice().lock_or_recover();
        if v.snapshot.busy() {
            return Err(failure("voice-busy"));
        }
        let probe = context::raw_probe(&name)?;
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
    })
    .await
    .map_err(|_| failure("target-unavailable"))?
}

#[tauri::command]
pub(crate) fn voice_start(target_id: u64, locale: String) -> Result<u64, DeckError> {
    if !SUPPORTED_LANGUAGES.contains(&locale.as_str()) {
        return Err(failure("language-invalid"));
    }
    let mut v = voice().lock_or_recover();
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
    #[cfg(target_os = "macos")]
    unsafe {
        deck_speech_start(
            id,
            CString::new(locale).unwrap().as_ptr(),
            snapshot_callback,
        );
    }
    #[cfg(not(target_os = "macos"))]
    {
        v.snapshot.status = "error".into();
        v.snapshot.code = "local-unavailable".into();
    }
    Ok(id)
}

#[tauri::command]
pub(crate) fn voice_snapshot(id: u64) -> Result<Snapshot, DeckError> {
    let v = voice().lock_or_recover();
    if v.snapshot.id != id {
        return Err(failure("recording-expired"));
    }
    Ok(v.snapshot.clone())
}

#[tauri::command]
pub(crate) fn voice_stop(id: u64) {
    #[cfg(target_os = "macos")]
    unsafe {
        deck_speech_stop(id);
    }
}

#[tauri::command]
pub(crate) fn voice_cancel(id: u64) {
    let mut v = voice().lock_or_recover();
    if id == 0 || v.snapshot.id == id {
        v.snapshot.status = "cancelled".into();
        v.snapshot.text.clear();
    }
    #[cfg(target_os = "macos")]
    unsafe {
        deck_speech_cancel(id);
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Delivery {
    Inserted,
    Submitted,
    EnterRefused,
    Ambiguous,
}

#[tauri::command]
pub(crate) async fn voice_deliver(
    app: tauri::AppHandle,
    target_id: u64,
    text: String,
    submit: bool,
) -> Result<Delivery, DeckError> {
    use tauri::Manager;
    if text.trim().is_empty()
        || text.len() > 65_536
        || text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err(failure("text-invalid"));
    }
    let text = scheduler::normalize_prompt(&text);
    if text.is_empty() {
        return Err(failure("text-invalid"));
    }
    tauri::async_runtime::spawn_blocking(move || {
        let queues = app.state::<Queues>();
        let target = {
            let v = voice().lock_or_recover();
            if v.snapshot.busy() {
                return Err(failure("voice-busy"));
            }
            let target = v
                .targets
                .values()
                .find(|t| t.id == target_id)
                .ok_or_else(|| failure("target-unavailable"))?;
            if !scheduler::claim_session(&queues.busy, &target.session) {
                return Err(failure("delivery-busy"));
            }
            target.clone()
        };
        // RAII releases the same exclusion used by scheduled prompt workers.
        struct Release<'a>(&'a Queues, &'a str);
        impl Drop for Release<'_> {
            fn drop(&mut self) {
                scheduler::release_session(&self.0.busy, self.1);
            }
        }
        let _release = Release(&queues, &target.session);
        let current = context::raw_probe(&target.session).map_err(|_| failure("target-changed"))?;
        if current.identity != target.probe.identity {
            return Err(failure("target-changed"));
        }
        if current.foreground.is_none() {
            return Err(failure("target-unavailable"));
        }
        if text.contains('\n') {
            let paste = crate::tmux::tmux(&[
                "display-message",
                "-p",
                "-t",
                &target.probe.identity.pane_id,
                "#{bracketed_paste_flag}",
            ])?;
            if paste.trim() != "1" {
                return Err(failure("multiline-unsupported"));
            }
        }
        let outcome = prompt_delivery::deliver(LiteralRequest {
            session: &target.session,
            pane: &target.probe.identity,
            expected_process: current.foreground.as_deref(),
            delivery: &format!("voice-{}", scheduler::next_delivery_id()),
            text: &text,
            submit,
            require_bracketed: true,
        });
        // A transport failure can happen after the paste. Never auto-retry it.
        let result = match outcome {
            Ok(LiteralOutcome::Inserted) => Delivery::Inserted,
            Ok(LiteralOutcome::Submitted) => Delivery::Submitted,
            Ok(LiteralOutcome::EnterRefused) => Delivery::EnterRefused,
            Err(error) if error == "target-changed" => return Err(failure("target-changed")),
            Err(_) => Delivery::Ambiguous,
        };
        Ok(result)
    })
    .await
    .map_err(|_| failure("delivery-unknown"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn callbacks_cannot_revive_cancelled_or_replace_newer_recordings() {
        let mut v = voice().lock_or_recover();
        v.snapshot = Snapshot {
            id: 91,
            status: "preparing".into(),
            ..Snapshot::default()
        };
        drop(v);
        let state = CString::new("recording").unwrap();
        let text = CString::new("private draft").unwrap();
        let code = CString::new("").unwrap();
        snapshot_callback(90, state.as_ptr(), text.as_ptr(), code.as_ptr());
        assert!(voice().lock_or_recover().snapshot.text.is_empty());
        voice().lock_or_recover().snapshot.status = "cancelled".into();
        snapshot_callback(91, state.as_ptr(), text.as_ptr(), code.as_ptr());
        assert!(voice().lock_or_recover().snapshot.text.is_empty());
    }
}
