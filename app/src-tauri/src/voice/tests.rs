use super::*;
use crate::prompt_delivery::tests::{probe, FakeTransport};
use std::cell::RefCell;

#[derive(Default)]
struct FakeSpeech(RefCell<Vec<(String, u64)>>);
impl Speech for FakeSpeech {
    fn start(&self, id: u64, locale: &str) {
        self.0.borrow_mut().push((locale.into(), id));
    }
    fn stop(&self, id: u64) {
        self.0.borrow_mut().push(("stop".into(), id));
    }
    fn cancel(&self, id: u64) {
        self.0.borrow_mut().push(("cancel".into(), id));
    }
}
fn binding(state: &Mutex<Voice>, io: &FakeTransport) -> u64 {
    bind_with(state, "deck-voice-test".into(), io).unwrap().id
}
fn snapshot(state: &Mutex<Voice>, id: u64, status: &str, text: &str, code: &str) {
    let status = CString::new(status).unwrap();
    let text = CString::new(text).unwrap();
    let code = CString::new(code).unwrap();
    accept_snapshot(state, id, status.as_ptr(), text.as_ptr(), code.as_ptr());
}
#[test]
fn recording_lifecycle_accepts_current_callbacks_and_cannot_revive_cancelled_capture() {
    let state = Mutex::new(Voice::default());
    let io = FakeTransport::default();
    let speech = FakeSpeech::default();
    let target = binding(&state, &io);
    let id = start_with(&state, target, "ja-JP", &speech).unwrap();
    assert_eq!(snapshot_with(&state, id).unwrap().status, "preparing");
    snapshot(&state, id + 1, "recording", "stale", "");
    accept_snapshot(
        &state,
        id,
        std::ptr::null(),
        std::ptr::null(),
        std::ptr::null(),
    );
    assert!(snapshot_with(&state, id).unwrap().text.is_empty());
    for phase in ["downloading", "recording", "stopping"] {
        snapshot(&state, id, phase, "部分文字", "");
        assert_eq!(snapshot_with(&state, id).unwrap().status, phase);
    }
    stop_with(&state, id + 1, &speech);
    stop_with(&state, id, &speech);
    assert_eq!(speech.0.borrow().len(), 2);
    cancel_with(&state, id + 1, &speech);
    assert_eq!(snapshot_with(&state, id).unwrap().text, "部分文字");
    cancel_with(&state, id, &speech);
    snapshot(&state, id, "recording", "late", "");
    assert_eq!(snapshot_with(&state, id).unwrap().status, "cancelled");
    assert!(snapshot_with(&state, id).unwrap().text.is_empty());
    assert!(snapshot_with(&state, id + 1).is_err());
    let next = start_with(&state, target, "en-US", &speech).unwrap();
    snapshot(&state, id, "error", "old error", "recognition-failed");
    snapshot(&state, next, "error", "retained", "microphone-unavailable");
    let result = snapshot_with(&state, next).unwrap();
    assert_eq!(result.text, "retained");
    assert_eq!(result.code, "microphone-unavailable");
    snapshot(&state, next, "recording", "late", "");
    assert_eq!(snapshot_with(&state, next).unwrap().status, "error");
    cancel_with(&state, 0, &speech);
    assert!(snapshot_with(&state, next).unwrap().text.is_empty());
}
#[test]
fn invalid_or_busy_capture_never_opens_the_microphone_or_replaces_binding() {
    let state = Mutex::new(Voice::default());
    let io = FakeTransport::default();
    let speech = FakeSpeech::default();
    assert!(bind_with(&state, "not a session".into(), &io).is_err());
    io.fail_probe.set(true);
    assert!(bind_with(&state, "deck-a".into(), &io).is_err());
    io.fail_probe.set(false);
    let mut missing = probe();
    missing.foreground = None;
    *io.observed.borrow_mut() = Some(missing);
    assert!(bind_with(&state, "deck-a".into(), &io).is_err());
    *io.observed.borrow_mut() = None;
    assert_eq!(
        start_with(&state, 42, "en-US", &speech)
            .unwrap_err()
            .to_string(),
        "target-unavailable"
    );
    let target = binding(&state, &io);
    assert_eq!(
        start_with(&state, target, "unknown", &speech)
            .unwrap_err()
            .code(),
        "invalid"
    );
    assert!(speech.0.borrow().is_empty());
    start_with(&state, target, "en-US", &speech).unwrap();
    assert_eq!(
        start_with(&state, target, "en-US", &speech)
            .unwrap_err()
            .to_string(),
        "voice-busy"
    );
    assert!(bind_with(&state, "deck-b".into(), &io).is_err());
    assert_eq!(speech.0.borrow().len(), 1);
    assert_eq!(state.lock_or_recover().targets.len(), 1);
}
#[test]
fn delivery_validates_text_and_shares_the_scheduler_exclusion() {
    let state = Mutex::new(Voice::default());
    let io = FakeTransport::default();
    let busy = Mutex::new(HashSet::new());
    let target = binding(&state, &io);
    for text in [
        "".into(),
        " \r\n\t".into(),
        "\u{1b}[31m".into(),
        "界".repeat(22000),
    ] {
        assert_eq!(
            deliver_with(&state, &busy, target, text, true, &io)
                .unwrap_err()
                .code(),
            "invalid"
        );
    }
    assert!(io.calls.borrow().is_empty());
    assert!(scheduler::claim_session(&busy, "deck-voice-test"));
    assert_eq!(
        deliver_with(&state, &busy, target, "test".into(), true, &io)
            .unwrap_err()
            .to_string(),
        "delivery-busy"
    );
    assert!(busy.lock_or_recover().contains("deck-voice-test"));
    scheduler::release_session(&busy, "deck-voice-test");
    assert!(deliver_with(&state, &busy, target + 10, "test".into(), true, &io).is_err());
    state.lock_or_recover().snapshot.status = "recording".into();
    assert_eq!(
        deliver_with(&state, &busy, target, "test".into(), true, &io)
            .unwrap_err()
            .to_string(),
        "voice-busy"
    );
    assert!(busy.lock_or_recover().is_empty());
}
#[test]
fn generation_expiry_is_distinct_from_transient_refusals_and_always_releases_busy() {
    let state = Mutex::new(Voice::default());
    let io = FakeTransport::default();
    let busy = Mutex::new(HashSet::new());
    let target = binding(&state, &io);
    let mut changed = probe();
    changed.identity.server_pid += 1;
    *io.observed.borrow_mut() = Some(changed);
    assert_eq!(
        deliver_with(&state, &busy, target, "test".into(), true, &io)
            .unwrap_err()
            .to_string(),
        "target-expired"
    );
    assert!(busy.lock_or_recover().is_empty());
    assert!(io.calls.borrow().is_empty());
    let rebound = binding(&state, &io);
    assert_eq!(
        deliver_with(&state, &busy, rebound, "test".into(), false, &io).unwrap(),
        Delivery::Inserted
    );
    assert!(busy.lock_or_recover().is_empty());
    io.fail_probe.set(true);
    assert_eq!(
        deliver_with(&state, &busy, rebound, "test".into(), true, &io)
            .unwrap_err()
            .to_string(),
        "target-changed"
    );
    io.fail_probe.set(false);
    let mut missing = io.observed.borrow().clone().unwrap();
    missing.foreground = None;
    *io.observed.borrow_mut() = Some(missing);
    assert_eq!(
        deliver_with(&state, &busy, rebound, "test".into(), true, &io)
            .unwrap_err()
            .to_string(),
        "target-unavailable"
    );
    assert!(busy.lock_or_recover().is_empty());
}
#[test]
fn delivery_outcomes_preserve_ambiguity_and_multiline_requires_paste_mode() {
    for (replies, expected) in [
        (vec![Ok("0".into())], Err("multiline-unsupported")),
        (
            vec![Ok("1".into()), Ok(String::new()), Ok(String::new())],
            Ok(Delivery::Submitted),
        ),
        (
            vec![Ok("1".into()), Ok("deck-context-refused".into())],
            Err("target-changed"),
        ),
        (
            vec![
                Ok("1".into()),
                Err(DeckError::new(ErrorKind::Other, "lost reply")),
            ],
            Ok(Delivery::Ambiguous),
        ),
        (
            vec![
                Ok("1".into()),
                Ok(String::new()),
                Ok("deck-context-refused".into()),
            ],
            Ok(Delivery::EnterRefused),
        ),
    ] {
        let state = Mutex::new(Voice::default());
        let io = FakeTransport::default();
        let busy = Mutex::new(HashSet::new());
        let target = binding(&state, &io);
        io.replies.borrow_mut().extend(replies);
        assert_eq!(
            deliver_with(&state, &busy, target, "first\r\nsecond".into(), true, &io)
                .map_err(|e| e.to_string()),
            expected.map_err(str::to_string)
        );
        assert!(busy.lock_or_recover().is_empty());
        if io.calls.borrow().len() > 1 {
            assert_eq!(*io.inputs.borrow(), [b"first\nsecond".to_vec()]);
            assert!(!io
                .calls
                .borrow()
                .iter()
                .flatten()
                .any(|arg| arg.contains("first")));
        }
    }
}
