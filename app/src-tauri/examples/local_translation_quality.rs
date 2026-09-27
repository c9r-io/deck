//! Isolated manual quality carrier using the production provider, scanner and
//! splitter. It reads one explicit stdin document and returns JSON on stdout.
//! No text is logged, saved or sent over a network transport.
#![cfg(not(test))]
#[path = "../src/error.rs"]
#[allow(dead_code)]
mod error;
#[path = "../src/intelligence/protected.rs"]
#[allow(dead_code)]
mod protected;
#[path = "../src/intelligence/provider.rs"]
#[allow(dead_code)]
mod provider;

use provider::{BergamotProvider, TranslationProvider};
use std::io::Read;
use std::path::Path;

fn run(model: &Path, source: &str) -> serde_json::Value {
    if source.trim().is_empty() {
        return serde_json::json!({"ok": false, "error": "text-empty"});
    }
    if source.len() > 16384 {
        return serde_json::json!({"ok": false, "error": "text-too-large"});
    }
    let pieces = protected::split_document(source);
    if pieces.concat() != source {
        return serde_json::json!({"ok": false, "error": "translation-failed"});
    }
    let spans = protected::scan(source);
    let mut engine = BergamotProvider::default();
    if engine.load(model).is_err() {
        return serde_json::json!({"ok": false, "error": "translation-model-missing"});
    }
    let started = std::time::Instant::now();
    let mut result = String::new();
    let mut translated = 0;
    let mut protected_spans = 0;
    for piece in &pieces {
        if piece.trim().is_empty() {
            result.push_str(piece);
            continue;
        }
        let (carrier, originals) = protected::carrier(piece);
        protected_spans += originals.len();
        let Ok(raw) = engine.translate(&carrier) else {
            return serde_json::json!({"ok": false, "error": "translation-failed"});
        };
        let Ok(restored) = protected::restore(&raw, &originals) else {
            return serde_json::json!({"ok": false, "error": "protected-restoration-failed"});
        };
        result.push_str(&restored);
        translated += 1;
    }
    serde_json::json!({
        "ok": true, "text": result, "sourceBytes": source.len(),
        "segments": translated, "protectedSpans": protected_spans,
        "durationSeconds": started.elapsed().as_secs_f64(),
        "spans": spans.iter().map(|span| serde_json::json!({
            "start": span.start, "end": span.end, "kind": span.kind
        })).collect::<Vec<_>>()
    })
}

fn main() {
    let Some(model_dir) = std::env::args_os().nth(1) else {
        println!(
            "{}",
            serde_json::json!({"ok": false, "error": "translation-model-missing"})
        );
        return;
    };
    let mut bytes = Vec::new();
    if std::io::stdin()
        .take(16385)
        .read_to_end(&mut bytes)
        .is_err()
    {
        println!(
            "{}",
            serde_json::json!({"ok": false, "error": "translation-failed"})
        );
        return;
    }
    let response = match String::from_utf8(bytes) {
        Ok(source) => run(Path::new(&model_dir), &source),
        Err(_) => serde_json::json!({"ok": false, "error": "translation-failed"}),
    };
    println!("{response}");
}
