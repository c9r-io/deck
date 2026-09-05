//! Closed, debug-only fault injection for the packaged WKWebView gate.
//! Hooks arm only when both smoke arguments select an isolated data root;
//! normal and release launches cannot trigger them.

use crate::sync::LockRecover;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

const KINDS: &[&str] = &[
    "board-save",
    "settings-save",
    "queue-save",
    "queue-cancel",
    "tmux-after-stop",
    "tmux-after-socket",
    "tmux-before-start",
    "tmux-after-metadata",
];
static COUNTS: LazyLock<Mutex<HashMap<&'static str, u8>>> = LazyLock::new(|| {
    let mut counts = HashMap::new();
    if let Some(kind) = crate::storage::debug_arg("--smoke-fault").and_then(|v| canonical(&v)) {
        counts.insert(kind, 1);
    }
    Mutex::new(counts)
});

pub(crate) fn enabled() -> bool {
    cfg!(debug_assertions)
        && crate::storage::debug_arg("--smoke-wkwebview").is_some()
        && crate::storage::debug_arg("--smoke-data-dir")
            .is_some_and(|root| std::path::Path::new(&root).is_absolute())
}

fn canonical(kind: &str) -> Option<&'static str> {
    KINDS.iter().copied().find(|candidate| *candidate == kind)
}

pub(crate) fn take(kind: &str) -> bool {
    if !enabled() {
        return false;
    }
    let Some(kind) = canonical(kind) else {
        return false;
    };
    let mut counts = COUNTS.lock_or_recover();
    let count = counts.entry(kind).or_default();
    if *count == 0 {
        return false;
    }
    *count -= 1;
    true
}

#[derive(Debug, Serialize)]
pub(crate) struct SmokeFaultState {
    kind: String,
    remaining: u8,
}

#[derive(Debug, Serialize)]
pub(crate) struct SmokeClipboardMetrics {
    bytes: usize,
    newlines: usize,
    hash: String,
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

#[tauri::command]
pub(crate) fn smoke_fault_set(kind: String, count: u8) -> Result<SmokeFaultState, String> {
    if !enabled() {
        return Err("smoke fault hooks are unavailable".into());
    }
    let kind = canonical(&kind).ok_or("unknown smoke fault")?;
    if count > 8 {
        return Err("smoke fault count must be between 0 and 8".into());
    }
    COUNTS.lock_or_recover().insert(kind, count);
    Ok(SmokeFaultState {
        kind: kind.to_string(),
        remaining: count,
    })
}

#[tauri::command]
pub(crate) fn smoke_clipboard_metrics() -> Result<SmokeClipboardMetrics, String> {
    if !enabled() {
        return Err("smoke clipboard metrics are unavailable".into());
    }
    let out = std::process::Command::new("pbpaste")
        .output()
        .map_err(|_| "clipboard reader unavailable")?;
    if !out.status.success() {
        return Err("clipboard reader failed".into());
    }
    Ok(SmokeClipboardMetrics {
        bytes: out.stdout.len(),
        newlines: out.stdout.iter().filter(|byte| **byte == b'\n').count(),
        hash: format!("{:016x}", fnv1a64(&out.stdout)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fault_protocol_is_a_closed_enum_with_a_small_count() {
        assert_eq!(canonical("board-save"), Some("board-save"));
        assert_eq!(canonical("settings-save"), Some("settings-save"));
        assert_eq!(canonical("tmux-after-stop"), Some("tmux-after-stop"));
        assert_eq!(
            canonical("tmux-after-metadata"),
            Some("tmux-after-metadata")
        );
        assert_eq!(canonical("shell"), None);
        assert_eq!(canonical("/tmp/file"), None);
        assert_eq!(canonical("queue-save; rm"), None);
    }

    #[test]
    fn clipboard_hash_is_stable_and_byte_oriented() {
        assert_eq!(fnv1a64(b"DEFG"), 0xbab10472a66bfe51);
        assert_ne!(fnv1a64("中".as_bytes()), fnv1a64(b"?"));
    }

    #[test]
    fn normal_test_process_cannot_arm_or_consume_packaged_smoke_hooks() {
        assert!(!enabled());
        for kind in KINDS {
            assert!(!take(kind));
            assert_eq!(
                smoke_fault_set((*kind).to_string(), 1).unwrap_err(),
                "smoke fault hooks are unavailable"
            );
        }
        assert!(!take("unknown"));
        assert_eq!(
            smoke_clipboard_metrics().unwrap_err(),
            "smoke clipboard metrics are unavailable"
        );
    }
}
