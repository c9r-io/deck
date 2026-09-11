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
    pub inputs: RefCell<Vec<Vec<u8>>>,
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
    fn run_with_stdin(&self, args: &[String], input: &[u8]) -> Result<String, DeckError> {
        self.inputs.borrow_mut().push(input.to_vec());
        self.run(args)
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
    assert_eq!(calls[0][0], "load-buffer");
    assert_eq!(calls[0][3], "-");
    assert_eq!(*io.inputs.borrow(), [b"a;b #{x} 'quoted'".to_vec()]);
    assert!(!calls.iter().flatten().any(|arg| arg.contains("a;b")));
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
        .env("LANG", "en_US.UTF-8")
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
    fn run_with_stdin(&self, args: &[String], input: &[u8]) -> Result<String, DeckError> {
        let mut command = std::process::Command::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/binaries/tmux-aarch64-apple-darwin"
        ));
        command.args(["-f", "/dev/null", "-L", &self.0]);
        let args: Vec<_> = args.iter().map(String::as_str).collect();
        crate::tmux::command_with_stdin(&mut command, &args, input)
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
    for case in ["generation", "foreground", "multiline", "copy", "missing"] {
        let mut identity = pane.clone();
        if case == "generation" {
            identity.server_pid += 1;
        }
        if case == "missing" {
            identity.pane_id = "%999999999".into();
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
    run(&["resize-window", "-t", "t", "-x", "240"]);
    for (text, submit, require_bracketed) in [
        (
            "-DECK_LITERAL_OK 中文 'quoted' \"double\" #{pid} $(literal) \\ ;",
            false,
            true,
        ),
        ("DECK_SCHEDULED_FIRST\nDECK_SCHEDULED_SECOND", true, false),
    ] {
        let mut req = request(&pane);
        req.submit = submit;
        req.require_bracketed = require_bracketed;
        req.expected_process = None;
        req.text = text;
        assert_eq!(
            deliver_with(req, &io).unwrap(),
            if submit {
                LiteralOutcome::Submitted
            } else {
                LiteralOutcome::Inserted
            }
        );
        let mut landed = false;
        for _ in 0..40 {
            if run(&["capture-pane", "-p", "-t", "t"]).contains(text) {
                landed = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(landed, "literal bytes did not reach the isolated pane");
        assert!(!run(&["list-buffers"]).contains("deck-send-voice-1"));
    }
}

// Inspect only children of this test process. The image includes argv and
// environment, so the assertion also catches accidental environment transport.
#[cfg(target_os = "macos")]
fn child_process_image(pid: u32) -> Option<Vec<u8>> {
    let mut image = vec![0u8; 1024 * 1024];
    let mut len = image.len();
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as libc::c_int];
    // SAFETY: sysctl receives the allocated buffer and its actual byte capacity.
    let result = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as libc::c_uint,
            image.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if result != 0 {
        return None;
    }
    image.truncate(len);
    Some(image)
}

#[test]
#[cfg(target_os = "macos")]
fn stdin_pipe_keeps_full_prompt_out_of_live_argv_and_preserves_every_byte() {
    let io = IsolatedTmux(format!("deck-test-prompt-privacy-{}", std::process::id()));
    io.run(&[
        "new-session".into(),
        "-d".into(),
        "-s".into(),
        "t".into(),
        "/bin/cat".into(),
    ])
    .unwrap();
    let marker = "DECK_SYNTHETIC_PRIVATE_PROMPT";
    let mut text = format!("-{marker} 中文 日本語 'quoted' \"double\" \\ #{{pid}} $(literal);\n");
    text.push_str(&"x".repeat(65_536 - text.len() - 1));
    text.push(';');
    let contains =
        |bytes: &[u8], needle: &[u8]| bytes.windows(needle.len()).any(|part| part == needle);
    std::thread::scope(|scope| {
        // Hold this client's lifetime open after loading, so process inspection
        // does not depend on catching a short-lived client. Signal even on timeout.
        let worker = scope.spawn(|| {
            io.run_with_stdin(
                &[
                    "load-buffer".into(),
                    "-b".into(),
                    "deck-send-privacy".into(),
                    "-".into(),
                    ";".into(),
                    "wait-for".into(),
                    "privacy-release".into(),
                    ";".into(),
                    "save-buffer".into(),
                    "-b".into(),
                    "deck-send-privacy".into(),
                    "-".into(),
                    ";".into(),
                    "delete-buffer".into(),
                    "-b".into(),
                    "deck-send-privacy".into(),
                ],
                text.as_bytes(),
            )
        });
        let mut observed = None;
        for _ in 0..100 {
            observed = crate::procinfo::processes()
                .values()
                .filter(|process| process.ppid == std::process::id())
                .filter_map(|process| child_process_image(process.pid))
                .find(|image| {
                    contains(image, io.0.as_bytes()) && contains(image, b"privacy-release")
                });
            if observed.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let release = io.run(&["wait-for".into(), "-S".into(), "privacy-release".into()]);
        if release.is_err() {
            let _ = io.run(&["kill-server".into()]);
        }
        let output = worker.join().unwrap();
        release.unwrap();
        let image = observed.expect("the isolated stdin client must be observable");
        assert!(
            !contains(&image, marker.as_bytes()),
            "prompt leaked to process arguments/environment"
        );
        assert_eq!(output.unwrap().as_bytes(), text.as_bytes());
    });
    assert!(!io
        .run(&["list-buffers".into()])
        .unwrap()
        .contains("deck-send-privacy"));
}
