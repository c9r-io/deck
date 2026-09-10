use super::*;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

pub(crate) fn probe() -> RawProbe {
    RawProbe {
        identity: PaneIdentity {
            server_pid: 123,
            session_id: "$1".into(),
            window_id: "@2".into(),
            pane_id: "%3".into(),
            pane_pid: 456,
        },
        foreground: Some("zsh".into()),
        foreground_argv: None,
    }
}
#[derive(Default)]
pub(crate) struct FakeTransport {
    pub calls: RefCell<Vec<Vec<String>>>,
    pub replies: RefCell<VecDeque<Result<String, DeckError>>>,
    pub observed: RefCell<Option<RawProbe>>,
    pub fail_probe: Cell<bool>,
    pub waits: RefCell<Vec<Duration>>,
}
impl Transport for FakeTransport {
    fn probe(&self, _: &str) -> Result<RawProbe, DeckError> {
        if self.fail_probe.get() {
            return Err(DeckError::new(ErrorKind::Missing, "probe unavailable"));
        }
        Ok(self.observed.borrow().clone().unwrap_or_else(probe))
    }
    fn run(&self, args: &[String]) -> Result<String, DeckError> {
        self.calls.borrow_mut().push(args.to_vec());
        self.replies
            .borrow_mut()
            .pop_front()
            .unwrap_or_else(|| Ok(String::new()))
    }
    fn pause(&self, duration: Duration) {
        self.waits.borrow_mut().push(duration);
    }
}
fn request(pane: &PaneIdentity) -> LiteralRequest<'_> {
    LiteralRequest {
        session: "deck-test",
        pane,
        expected_process: Some("zsh"),
        delivery: "voice-1",
        text: "a;b #{x} 'quoted'",
        submit: true,
        require_bracketed: true,
    }
}

#[test]
fn insert_is_byte_literal_and_never_waits_or_sends_enter() {
    let io = FakeTransport::default();
    let pane = probe().identity;
    let mut req = request(&pane);
    req.submit = false;
    assert_eq!(deliver_with(req, &io).unwrap(), LiteralOutcome::Inserted);
    let calls = io.calls.borrow();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0][3], "a;b #{x} 'quoted'");
    assert!(calls[0][9].contains("#{pane_in_mode},0"));
    assert!(calls[0][10].starts_with("paste-buffer -p "));
    assert!(io.waits.borrow().is_empty());
}
#[test]
fn submit_waits_then_reuses_the_full_atomic_guard_for_enter() {
    let io = FakeTransport::default();
    let pane = probe().identity;
    let mut req = request(&pane);
    req.text = "first\nsecond";
    assert_eq!(deliver_with(req, &io).unwrap(), LiteralOutcome::Submitted);
    let calls = io.calls.borrow();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0][9], calls[1][4]);
    for atom in [
        "123:$1:@2:%3:456",
        "#{pane_current_command},zsh",
        "#{bracketed_paste_flag},1",
        "#{pane_in_mode},0",
    ] {
        assert!(calls[0][9].contains(atom), "missing {atom}");
    }
    assert_eq!(calls[1][5], "send-keys -t %3 Enter");
    assert_eq!(*io.waits.borrow(), [Duration::from_millis(600)]);
}
#[test]
fn scheduler_compatibility_omits_interactive_guards_and_supports_argv_alias() {
    let io = FakeTransport::default();
    let pane = probe().identity;
    let mut raw = probe();
    raw.foreground = Some("2.1.259".into());
    raw.foreground_argv = Some("claude".into());
    *io.observed.borrow_mut() = Some(raw);
    let mut req = request(&pane);
    req.expected_process = Some("claude");
    req.require_bracketed = false;
    deliver_with(req, &io).unwrap();
    let guard = io.calls.borrow()[0][9].clone();
    assert!(guard.contains("#{pane_current_command},2.1.259"));
    assert!(!guard.contains("pane_in_mode"));
    assert!(!guard.contains("bracketed_paste_flag"));
    let mut req = request(&pane);
    req.expected_process = None;
    deliver_with(req, &io).unwrap();
    assert!(!io.calls.borrow()[2][9].contains("pane_current_command"));
}
#[test]
fn refusals_and_unknown_paste_failures_cleanup_without_enter() {
    for refused in [true, false] {
        let io = FakeTransport::default();
        let pane = probe().identity;
        io.replies.borrow_mut().push_back(if refused {
            Ok("deck-context-refused\n".into())
        } else {
            Err(DeckError::new(ErrorKind::Other, "transport"))
        });
        let error = deliver_with(request(&pane), &io).unwrap_err();
        assert_eq!(
            error.to_string(),
            if refused {
                "target-changed"
            } else {
                "delivery-unknown"
            }
        );
        assert_eq!(
            io.calls.borrow()[1],
            ["delete-buffer", "-b", "deck-send-voice-1"]
        );
        assert!(io.waits.borrow().is_empty());
    }
}
#[test]
fn enter_refusal_and_transport_failure_do_not_paste_again() {
    for refused in [true, false] {
        let io = FakeTransport::default();
        let pane = probe().identity;
        io.replies.borrow_mut().extend([
            Ok(String::new()),
            if refused {
                Ok("deck-context-refused".into())
            } else {
                Err(DeckError::new(ErrorKind::Other, "transport"))
            },
        ]);
        assert_eq!(
            deliver_with(request(&pane), &io).unwrap(),
            LiteralOutcome::EnterRefused
        );
        assert_eq!(io.calls.borrow().len(), 2);
    }
}
#[test]
fn invalid_ids_cannot_reach_transport_and_failed_probe_keeps_expected_guard() {
    let io = FakeTransport::default();
    let pane = probe().identity;
    for id in ["", "x;send-keys", "x y"] {
        let mut req = request(&pane);
        req.delivery = id;
        assert_eq!(deliver_with(req, &io).unwrap_err().code(), "invalid");
    }
    assert!(io.calls.borrow().is_empty());
    io.fail_probe.set(true);
    deliver_with(request(&pane), &io).unwrap();
    assert!(io.calls.borrow()[0][9].contains("#{pane_current_command},zsh"));
}

// Exercise the actual production guard against the bundled tmux on a private
// throwaway socket. No deck/dev server, microphone, or real user input is used.
struct IsolatedTmux(String);
impl Transport for IsolatedTmux {
    fn probe(&self, _: &str) -> Result<RawProbe, DeckError> {
        Ok(probe())
    }
    fn run(&self, args: &[String]) -> Result<String, DeckError> {
        let out = std::process::Command::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/binaries/tmux-aarch64-apple-darwin"
        ))
        .args(["-f", "/dev/null", "-L", &self.0])
        .args(args)
        .output()?;
        if !out.status.success() {
            return Err(DeckError::new(
                ErrorKind::Tmux,
                "isolated tmux command failed",
            ));
        }
        Ok(String::from_utf8(out.stdout)?)
    }
    fn pause(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}
impl Drop for IsolatedTmux {
    fn drop(&mut self) {
        let _ = self.run(&["kill-server".into()]);
    }
}
#[test]
fn production_paste_guards_reject_changed_generation_foreground_multiline_and_copy_mode() {
    let io = IsolatedTmux(format!("deck-test-voice-delivery-{}", std::process::id()));
    let run = |args: &[&str]| {
        io.run(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
            .unwrap()
    };
    run(&["new-session", "-d", "-s", "t", "/bin/cat"]);
    let raw = run(&[
        "display-message",
        "-p",
        "-t",
        "t",
        "#{pid}:#{session_id}:#{window_id}:#{pane_id}:#{pane_pid}",
    ]);
    let fields: Vec<_> = raw.trim().split(':').collect();
    let pane = PaneIdentity {
        server_pid: fields[0].parse().unwrap(),
        session_id: fields[1].into(),
        window_id: fields[2].into(),
        pane_id: fields[3].into(),
        pane_pid: fields[4].parse().unwrap(),
    };
    for case in ["generation", "foreground", "multiline", "copy"] {
        let mut identity = pane.clone();
        if case == "generation" {
            identity.server_pid += 1;
        }
        if case == "copy" {
            run(&["copy-mode", "-t", "t"]);
        }
        let mut req = request(&identity);
        req.submit = false;
        req.expected_process = None;
        req.text = if case == "multiline" {
            "must-not-land\nsecond"
        } else {
            "must-not-land"
        };
        if case == "foreground" {
            req.expected_process = Some("never-running");
        }
        assert_eq!(
            deliver_with(req, &io).unwrap_err().to_string(),
            "target-changed",
            "{case}"
        );
        assert!(!run(&["capture-pane", "-p", "-t", "t"]).contains("must-not-land"));
        assert!(!run(&["list-buffers"]).contains("deck-send-voice-1"));
        if case == "copy" {
            run(&["send-keys", "-t", "t", "-X", "cancel"]);
        }
    }
    let mut req = request(&pane);
    req.submit = false;
    req.expected_process = None;
    req.text = "DECK_LITERAL_OK";
    assert_eq!(deliver_with(req, &io).unwrap(), LiteralOutcome::Inserted);
    for _ in 0..40 {
        if run(&["capture-pane", "-p", "-t", "t"]).contains("DECK_LITERAL_OK") {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("literal bytes did not reach the isolated pane");
}
