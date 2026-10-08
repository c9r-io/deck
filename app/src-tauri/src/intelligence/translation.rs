//! Typed local translation coordinator. One in-process engine instance,
//! one request at a time. Cancellation removes the request's authority to
//! publish; a running native segment finishes before the next begins.
//! The engine is the build's selected entry of the static extension tables
//! (`extensions::translation_provider`), built once, on the first use, and
//! held as a `TranslationProvider` trait object: this file names no engine
//! type. Building loads nothing; an unregistered or doubly registered
//! engine leaves the coordinator without a provider and every translation
//! fails closed (`translation-failed`), never on a built-in default.
//! Enablement, pack verification, limits, language gating, cancellation and
//! where a result may be published stay here and in the Lens host; the
//! provider sees one protected segment at a time and nothing else.
use super::{pack, protected, provider::TranslationProvider};
use crate::documents;
use crate::error::{DeckError, ErrorKind};
use crate::sync::LockRecover;
use serde::Serialize;
use std::collections::HashSet;
use std::ffi::{c_char, CString};
use std::sync::{Mutex, OnceLock};

/// The one provider instance and the set of requests still allowed to
/// publish. `provider` is `None` when the tables name no usable engine.
struct Coordinator {
    provider: Option<Mutex<Box<dyn TranslationProvider>>>,
    active: Mutex<HashSet<u64>>,
}

impl Coordinator {
    fn from_registry() -> Self {
        Coordinator {
            provider: crate::extensions::translation_provider()
                .ok()
                .map(Mutex::new),
            active: Mutex::new(HashSet::new()),
        }
    }
    fn provider(&self) -> Result<&Mutex<Box<dyn TranslationProvider>>, DeckError> {
        self.provider.as_ref().ok_or(error("translation-failed"))
    }
    fn still_active(&self, id: u64) -> bool {
        self.active.lock_or_recover().contains(&id)
    }
    fn loaded(&self) -> bool {
        self.provider.as_ref().is_some_and(|provider| {
            let provider = provider.lock_or_recover();
            provider.is_loaded() && provider.capabilities().target == "zh-Hans"
        })
    }
    fn unload(&self) {
        if let Some(provider) = &self.provider {
            provider.lock_or_recover().unload();
        }
    }
    /// Translate an English document segment by segment. `enabled` is read
    /// again before every segment; a request that lost its place in
    /// `active` publishes nothing.
    fn translate(
        &self,
        id: u64,
        source: &str,
        pack: &std::path::Path,
        enabled: impl Fn() -> bool,
    ) -> Result<TranslationResult, DeckError> {
        let pieces = protected::split_document(source);
        if pieces.concat() != source {
            return Err(error("translation-failed"));
        }
        let mut engine = self.provider()?.lock_or_recover();
        if !self.still_active(id) {
            return Err(error("request-cancelled"));
        }
        if !engine.is_loaded() {
            engine.load(pack)?;
        }
        let mut result = String::new();
        let mut translated = 0;
        for piece in pieces {
            if !self.still_active(id) || !enabled() {
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
        if !self.still_active(id) {
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
}

fn coordinator() -> &'static Coordinator {
    static COORDINATOR: OnceLock<Coordinator> = OnceLock::new();
    COORDINATOR.get_or_init(Coordinator::from_registry)
}
fn active() -> &'static Mutex<HashSet<u64>> {
    &coordinator().active
}
fn error(code: &'static str) -> DeckError {
    DeckError::new(ErrorKind::Other, code)
}
fn still_active(id: u64) -> bool {
    coordinator().still_active(id)
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
    Capability {
        available: enabled && installed && cfg!(target_os = "macos"),
        enabled,
        installed,
        loaded: coordinator().loaded(),
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
    tokio::task::spawn_blocking(|| coordinator().unload())
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
    coordinator().translate(id, &source, &pack::directory(), || {
        documents::local_translation_settings().0
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
    /* ---- the coordinator's engine is the registered entry ---- */

    use crate::extensions::testing;
    use crate::intelligence::provider::{Capabilities, TranslationProviderEntry, BERGAMOT};
    use std::cell::RefCell;
    use std::path::Path;

    thread_local! {
        /// What the stand-in engine was asked to do, in order.
        static CALLS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
        /// A request id the stand-in cancels while "translating".
        static CANCEL_DURING: RefCell<Option<(&'static Coordinator, u64)>> =
            const { RefCell::new(None) };
    }
    #[derive(Default)]
    struct Recording {
        loaded: bool,
    }
    impl TranslationProvider for Recording {
        fn capabilities(&self) -> Capabilities {
            Capabilities {
                source: "en",
                target: "zh-Hans",
            }
        }
        fn load(&mut self, _: &Path) -> Result<(), DeckError> {
            CALLS.with(|calls| calls.borrow_mut().push("load".into()));
            self.loaded = true;
            Ok(())
        }
        fn translate(&mut self, segment: &str) -> Result<String, DeckError> {
            CALLS.with(|calls| calls.borrow_mut().push("translate".into()));
            if let Some((coordinator, id)) = CANCEL_DURING.with(|c| *c.borrow()) {
                coordinator.active.lock_or_recover().remove(&id);
            }
            Ok(format!("[stand-in]{segment}"))
        }
        fn unload(&mut self) {
            CALLS.with(|calls| calls.borrow_mut().push("unload".into()));
            self.loaded = false;
        }
        fn is_loaded(&self) -> bool {
            self.loaded
        }
    }
    const RECORDING: TranslationProviderEntry = TranslationProviderEntry {
        id: crate::extensions::TRANSLATION_ENGINE,
        build: || Box::new(Recording::default()),
    };
    const OTHER_ID: TranslationProviderEntry = TranslationProviderEntry {
        id: "another-engine",
        build: || Box::new(Recording::default()),
    };
    fn calls() -> Vec<String> {
        CALLS.with(|calls| calls.borrow().clone())
    }
    const TEXT: &str = "The build completed successfully.";

    #[test]
    fn the_coordinator_drives_the_registered_provider() {
        let coordinator = {
            let _table = testing::with_providers(&[RECORDING]);
            Coordinator::from_registry()
        };
        assert!(calls().is_empty(), "building the provider loads nothing");
        assert!(!coordinator.loaded());
        coordinator.active.lock_or_recover().insert(7);
        let result = coordinator
            .translate(7, TEXT, Path::new("unused"), || true)
            .unwrap();
        assert!(
            result.text.contains("[stand-in]"),
            "the text came from the registered engine: {}",
            result.text
        );
        assert_eq!((result.request_id, result.segments), (7, 1));
        assert_eq!(calls(), ["load", "translate"]);
        assert!(coordinator.loaded());
        // the one instance is reused, not rebuilt or reloaded
        coordinator
            .translate(7, TEXT, Path::new("unused"), || true)
            .unwrap();
        assert_eq!(calls(), ["load", "translate", "translate"]);
        coordinator.unload();
        assert_eq!(calls().last().map(String::as_str), Some("unload"));
        assert!(!coordinator.loaded());
    }

    #[test]
    fn an_engine_the_tables_do_not_offer_fails_closed() {
        // negative controls: the selected id is unregistered, or registered
        // twice. Neither falls back to a built-in engine.
        for table in [&[OTHER_ID][..], &[RECORDING, RECORDING][..], &[][..]] {
            let table: &'static [TranslationProviderEntry] = table;
            let coordinator = {
                let _table = testing::with_providers(table);
                Coordinator::from_registry()
            };
            coordinator.active.lock_or_recover().insert(1);
            let error = coordinator
                .translate(1, TEXT, Path::new("unused"), || true)
                .err()
                .unwrap();
            assert_eq!(error.message(), "translation-failed");
            assert!(!coordinator.loaded());
            coordinator.unload();
        }
        assert!(calls().is_empty(), "no engine was built or called");
    }

    #[test]
    fn a_cancelled_or_disabled_request_publishes_nothing() {
        let coordinator: &'static Coordinator = Box::leak(Box::new({
            let _table = testing::with_providers(&[RECORDING]);
            Coordinator::from_registry()
        }));
        // never admitted: refused before the engine is touched
        let refused = coordinator.translate(3, TEXT, Path::new("unused"), || true);
        assert_eq!(refused.err().unwrap().message(), "request-cancelled");
        assert!(calls().is_empty());
        // disabled between admission and the first segment
        coordinator.active.lock_or_recover().insert(4);
        let disabled = coordinator.translate(4, TEXT, Path::new("unused"), || false);
        assert_eq!(disabled.err().unwrap().message(), "request-cancelled");
        assert_eq!(calls(), ["load"], "loaded, but no segment was translated");
        // cancelled while the engine runs: the finished segment is dropped
        coordinator.active.lock_or_recover().insert(5);
        CANCEL_DURING.with(|c| *c.borrow_mut() = Some((coordinator, 5)));
        let late = coordinator.translate(5, TEXT, Path::new("unused"), || true);
        CANCEL_DURING.with(|c| *c.borrow_mut() = None);
        assert_eq!(late.err().unwrap().message(), "request-cancelled");
        assert_eq!(calls(), ["load", "translate"]);
    }

    #[test]
    fn the_production_engine_is_the_registered_bergamot_entry_and_starts_unloaded() {
        assert_eq!(BERGAMOT.id, crate::extensions::TRANSLATION_ENGINE);
        let coordinator = Coordinator::from_registry();
        let provider = coordinator.provider().unwrap().lock_or_recover();
        assert_eq!(
            provider.capabilities(),
            Capabilities {
                source: "en",
                target: "zh-Hans"
            }
        );
        assert!(
            !provider.is_loaded(),
            "registration and build load no model"
        );
        // this file names no engine type outside its tests
        let source = include_str!("translation.rs");
        let production = source.split("#[cfg(test)]\nmod tests").next().unwrap();
        assert!(!production.contains("Bergamot"));
        assert!(production.contains("crate::extensions::translation_provider()"));
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
