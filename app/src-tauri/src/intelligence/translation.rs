//! Typed local translation coordinator. One in-process Bergamot instance,
//! one request at a time. Cancellation removes the request's authority to
//! publish; a running native segment finishes before the next begins.
use super::{
    pack, protected,
    provider::{BergamotProvider, TranslationProvider},
};
use crate::documents;
use crate::error::{DeckError, ErrorKind};
use crate::sync::LockRecover;
use serde::Serialize;
use std::collections::HashSet;
use std::ffi::{c_char, CString};
use std::sync::{Mutex, OnceLock};

static PROVIDER: OnceLock<Mutex<BergamotProvider>> = OnceLock::new();
static ACTIVE: OnceLock<Mutex<HashSet<u64>>> = OnceLock::new();
fn provider() -> &'static Mutex<BergamotProvider> {
    PROVIDER.get_or_init(|| Mutex::new(BergamotProvider::default()))
}
fn active() -> &'static Mutex<HashSet<u64>> {
    ACTIVE.get_or_init(|| Mutex::new(HashSet::new()))
}
fn error(code: &'static str) -> DeckError {
    DeckError::new(ErrorKind::Other, code)
}
fn still_active(id: u64) -> bool {
    active().lock_or_recover().contains(&id)
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn deck_translation_source_kind(text: *const c_char) -> i32;
}
fn source_kind(text: &str) -> i32 {
    #[cfg(target_os = "macos")]
    {
        let Ok(input) = CString::new(text) else {
            return 0;
        };
        unsafe { deck_translation_source_kind(input.as_ptr()) }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = text;
        0
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Capability {
    available: bool,
    enabled: bool,
    installed: bool,
    loaded: bool,
}
#[tauri::command]
pub(crate) fn translation_capability() -> Capability {
    let (enabled, _) = documents::local_translation_settings();
    let installed = enabled && pack::status().installed;
    let supported = provider().lock_or_recover().capabilities();
    Capability {
        available: enabled && installed && cfg!(target_os = "macos"),
        enabled,
        installed,
        loaded: provider().lock_or_recover().is_loaded() && supported.target == "zh-Hans",
    }
}

#[tauri::command]
pub(crate) fn translation_pack_status() -> pack::PackStatus {
    pack::status()
}

#[tauri::command]
pub(crate) async fn translation_pack_install() -> Result<pack::PackStatus, DeckError> {
    // Invoked only after Settings' explicit download-and-enable confirmation.
    pack::install().await
}

#[tauri::command]
pub(crate) async fn translation_pack_delete() -> Result<(), DeckError> {
    if documents::local_translation_settings().0 {
        return Err(error("translation-disabled"));
    }
    translation_unload().await?;
    pack::delete()
}

#[tauri::command]
pub(crate) async fn translation_unload() -> Result<(), DeckError> {
    active().lock_or_recover().clear();
    super::pasteboard::translation_clipboard_disarm();
    tokio::task::spawn_blocking(|| provider().lock_or_recover().unload())
        .await
        .map_err(|_| error("translation-failed"))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TranslationResult {
    pub request_id: u64,
    pub text: String,
    pub source_language: &'static str,
    pub target_language: &'static str,
    pub segments: usize,
}

fn limit_for(mode: &str, configured_document_bytes: usize) -> Result<usize, DeckError> {
    match mode {
        "live" => Ok(pack::LIVE_BYTES),
        "selection" => Ok(pack::SELECTION_BYTES),
        "clipboard" => Ok(configured_document_bytes),
        _ => Err(error("translation-failed")),
    }
}
fn validate(text: &str, mode: &str, configured_document_bytes: usize) -> Result<(), DeckError> {
    if text.trim().is_empty() {
        return Err(error("text-empty"));
    }
    let limit = limit_for(mode, configured_document_bytes)?;
    if text.len() > limit {
        return Err(error(if mode == "live" {
            "view-too-large"
        } else {
            "text-too-large"
        }));
    }
    if text.contains('\0') {
        return Err(error("translation-failed"));
    }
    Ok(())
}

fn translate_owned(id: u64, source: String) -> Result<TranslationResult, DeckError> {
    if !still_active(id) {
        return Err(error("request-cancelled"));
    }
    pack::verify()?;
    if !documents::local_translation_settings().0 {
        return Err(error("translation-disabled"));
    }
    let detected = source_kind(&source);
    if detected == 2 {
        return Ok(TranslationResult {
            request_id: id,
            text: source,
            source_language: "zh-Hans",
            target_language: "zh-Hans",
            segments: 0,
        });
    }
    if detected != 1 {
        return Err(error("source-language-unsupported"));
    }
    let pieces = protected::split_document(&source);
    if pieces.concat() != source {
        return Err(error("translation-failed"));
    }
    let mut engine = provider().lock_or_recover();
    if !still_active(id) {
        return Err(error("request-cancelled"));
    }
    if !engine.is_loaded() {
        engine.load(&pack::directory())?;
    }
    let mut result = String::new();
    let mut translated = 0;
    for piece in pieces {
        if !still_active(id) || !documents::local_translation_settings().0 {
            return Err(error("request-cancelled"));
        }
        if !piece.trim().is_empty() {
            let (html, originals) = protected::carrier(piece);
            let translated_html = engine.translate(&html)?;
            let restored = protected::restore(&translated_html, &originals).map_err(error)?;
            if restored.trim().is_empty() {
                return Err(error("translation-failed"));
            }
            result.push_str(&restored);
            translated += 1;
        } else {
            result.push_str(piece);
        }
    }
    if !still_active(id) {
        return Err(error("request-cancelled"));
    }
    Ok(TranslationResult {
        request_id: id,
        text: result,
        source_language: "en",
        target_language: "zh-Hans",
        segments: translated,
    })
}

#[tauri::command]
pub(crate) async fn translation_translate(
    request_id: u64,
    text: String,
    target_language: String,
    strategy: String,
) -> Result<TranslationResult, DeckError> {
    if !documents::local_translation_settings().0 {
        return Err(error("translation-disabled"));
    }
    if target_language != "zh-Hans" {
        return Err(error("source-language-unsupported"));
    }
    let (_, document_limit) = documents::local_translation_settings();
    validate(&text, &strategy, document_limit)?;
    if request_id == 0 || !active().lock_or_recover().insert(request_id) {
        return Err(error("translation-failed"));
    }
    let outcome = tokio::task::spawn_blocking(move || translate_owned(request_id, text))
        .await
        .map_err(|_| error("translation-failed"));
    active().lock_or_recover().remove(&request_id);
    outcome?
}

#[tauri::command]
pub(crate) fn translation_cancel(request_id: u64) {
    active().lock_or_recover().remove(&request_id);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn certified_bounds() {
        assert!(validate(&"a".repeat(4096), "live", 16384).is_ok());
        assert!(validate(&"a".repeat(4097), "live", 16384).is_err());
        assert!(validate(&"a".repeat(16384), "selection", 8192).is_ok());
        assert!(validate(&"a".repeat(8193), "clipboard", 8192).is_err());
        assert!(validate(" ", "clipboard", 16384).is_err());
        assert!(limit_for("best", 16384).is_err());
    }
    #[test]
    fn cancellation_removes_authority() {
        active().lock_or_recover().insert(987_654_321);
        translation_cancel(987_654_321);
        assert!(!still_active(987_654_321));
    }
    #[test]
    fn dominant_language_is_closed() {
        #[cfg(target_os = "macos")]
        {
            assert_eq!(
                source_kind(
                    "The build completed successfully. The tests passed and the report is ready."
                ),
                1
            );
            assert_eq!(source_kind("构建已经成功完成，所有测试均已通过。"), 2);
        }
    }
}
