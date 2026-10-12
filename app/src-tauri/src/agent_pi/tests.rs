//! The adapter's tests: the closed wire, and (`native`) the backend half of
//! the native Pi run.

use super::*;

const NONCE: &str = "0123456789abcdef0123456789abcdef";

/// An attest: the observer's claims and the nonce they answer.
fn hello(change: impl FnOnce(&mut serde_json::Value)) -> String {
    let mut line = serde_json::json!({
        "protocol": "pi-observer", "v": 2, "source": "pi", "nonce": NONCE, "socket": "deck",
        "server_pid": 4242, "pane": "%7", "observerVersion": 2, "piVersion": "1.1.0",
        "mode": "tui", "capabilities": { "signal": true, "lifecycle": false }
    });
    change(&mut line);
    line.to_string()
}

/// A frame of lease 3, with a nonce.
fn frame(body: serde_json::Value) -> String {
    let mut line = serde_json::json!({
        "protocol": "pi-observer", "v": 2, "source": "pi", "lease": 3, "nonce": NONCE
    });
    line.as_object_mut()
        .unwrap()
        .extend(body.as_object().unwrap().clone());
    line.to_string()
}

/// The claims of a line that is an attest.
fn parse_hello(line: &str) -> Result<Hello, &'static str> {
    match parse_message(line)? {
        Message::Attest { hello, .. } => Ok(hello),
        _ => Err("bad-frame"),
    }
}

/// The frame of a line that names a lease and carries a nonce.
fn parse_frame(line: &str) -> Result<Frame, &'static str> {
    match parse_message(line)? {
        Message::Frame { frame, .. } => frame,
        _ => Err("bad-frame"),
    }
}

#[test]
fn pi_is_registered_without_a_hook_family_or_a_reentry_capability() {
    assert_eq!(PI.source, "pi");
    assert_eq!(PI.class, SignalClass::Pi);
    assert!(matches!(PI.install, Install::PiExtension));
    assert!(PI.reentry.is_none(), "Pi is never re-entered automatically");
    // the registered entry is this one, exactly once
    let registered = crate::extensions::agent(SOURCE).expect("registered");
    assert_eq!(registered.source, PI.source);
    assert!(registered.reentry.is_none());
    // an executable name is not a source word, and Pi is no remote agent
    assert_eq!(crate::admission::interactive_agent(PROCESS), None);
    assert_eq!(crate::admission::channel_agent_command("pi"), None);
    assert_eq!(
        crate::admission::channel_agent_command("pi --no-tools"),
        None
    );
    assert_eq!(crate::admission::interactive_agent("node"), None);
}

/// The three kinds of line, told apart by the lease and the nonce: a hello
/// has neither and nothing else, an attest has the nonce and the claims, a
/// frame has both — and a line that is none of them is refused. What is
/// wrong INSIDE a frame is kept as that frame's refusal: whose mistake it
/// is can only be said once the sender is proven.
#[test]
fn a_line_is_a_hello_an_attest_or_a_frame_of_a_lease() {
    let common = serde_json::json!({ "protocol": "pi-observer", "v": 2, "source": "pi" });
    let with = |more: serde_json::Value| {
        let mut line = common.clone();
        line.as_object_mut()
            .unwrap()
            .extend(more.as_object().unwrap().clone());
        line.to_string()
    };
    assert_eq!(parse_message(&common.to_string()), Ok(Message::Hello));
    assert!(is_family(&common.to_string()));
    // a hello carries nothing: not a claim, not a frame's field
    for more in [
        serde_json::json!({"pane": "%7"}),
        serde_json::json!({"seq": 1}),
        serde_json::json!({"capabilities": {"signal": true, "lifecycle": false}}),
        serde_json::json!({"run": 1, "state": "working"}),
        serde_json::json!({"title": "x"}),
    ] {
        assert_eq!(
            parse_message(&with(more.clone())),
            Err("bad-frame"),
            "{more}"
        );
    }
    // an attest is the claims and the nonce
    assert!(matches!(
        parse_message(&hello(|_| {})),
        Ok(Message::Attest { nonce, .. }) if nonce.same(&Nonce::parse(NONCE).unwrap())
    ));
    // a frame names its lease and carries a nonce
    assert!(matches!(
        parse_message(&frame(serde_json::json!({"seq": 1}))),
        Ok(Message::Frame {
            lease: 3,
            frame: Ok(Frame::Ping { seq: 1 }),
            ..
        })
    ));
    // a lease that is none, a frame without its nonce, a nonce that is none
    for more in [
        serde_json::json!({"lease": 0, "nonce": NONCE, "seq": 1}),
        serde_json::json!({"lease": -1, "nonce": NONCE, "seq": 1}),
        serde_json::json!({"lease": 1.5, "nonce": NONCE, "seq": 1}),
        serde_json::json!({"lease": "3", "nonce": NONCE, "seq": 1}),
        serde_json::json!({"lease": 9007199254740992u64, "nonce": NONCE, "seq": 1}),
        serde_json::json!({"lease": 3, "seq": 1}),
        serde_json::json!({"lease": 3, "nonce": "", "seq": 1}),
        serde_json::json!({"lease": 3, "nonce": &NONCE[1..], "seq": 1}),
        serde_json::json!({"lease": 3, "nonce": format!("{NONCE}0"), "seq": 1}),
        serde_json::json!({"lease": 3, "nonce": NONCE.to_uppercase(), "seq": 1}),
        serde_json::json!({"lease": 3, "nonce": NONCE.replace('0', "g"), "seq": 1}),
        serde_json::json!({"lease": 3, "nonce": 7, "seq": 1}),
    ] {
        assert_eq!(
            parse_message(&with(more.clone())),
            Err("bad-frame"),
            "{more}"
        );
    }
    // What is wrong inside a frame whose lease and nonce can be read — a
    // word, a key this family does not have, a key that came twice — is the
    // frame's refusal, with the lease and the nonce still there to ask by.
    for line in [
        frame(serde_json::json!({"seq": 1, "run": 1, "state": "needs-input"})),
        frame(serde_json::json!({"seq": 1, "title": "Allow?"})),
        frame(serde_json::json!({"seq": 1})).replacen("\"seq\":1", "\"seq\":1,\"seq\":2", 1),
        frame(serde_json::json!({})),
    ] {
        assert!(
            matches!(
                parse_message(&line),
                Ok(Message::Frame {
                    lease: 3,
                    frame: Err("bad-frame"),
                    ..
                })
            ),
            "{line}"
        );
    }
    assert!(matches!(
        parse_message(&frame(
            serde_json::json!({"seq": 1, "event": "snapshot", "nativeSessionId": "../x"})
        )),
        Ok(Message::Frame {
            frame: Err("bad-native-id"),
            ..
        })
    ));
    // another version of the wire is no line of this one, whatever it holds
    for line in [
        common.to_string().replace("\"v\":2", "\"v\":1"),
        hello(|l| l["v"] = 1.into()),
        frame(serde_json::json!({"seq": 1})).replace("\"v\":2", "\"v\":3"),
    ] {
        assert_eq!(parse_message(&line), Err("bad-version"), "{line}");
    }
}

#[test]
fn a_nonce_is_thirty_two_digits_and_never_printed() {
    let (a, b) = (Nonce::fresh().unwrap(), Nonce::fresh().unwrap());
    assert!(!a.same(&b), "128 bits of the system's randomness");
    assert!(a.same(&a.clone()));
    let text = a.hex();
    assert_eq!(text.len(), 32);
    assert!(text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));
    assert!(Nonce::parse(&text).unwrap().same(&a));
    assert!(Nonce::parse(NONCE).unwrap().same(&Nonce::of([
        0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd,
        0xef
    ])));
    // a failed assertion or a log line that holds one says nothing of it
    assert_eq!(format!("{a:?}"), "Nonce(..)");
    assert!(!format!(
        "{:?}",
        Message::Attest {
            nonce: a.clone(),
            hello: parse_hello(&hello(|_| {})).unwrap()
        }
    )
    .contains(&text));
}

#[test]
fn the_attest_is_one_closed_shape() {
    let parsed = parse_hello(&hello(|_| {})).expect("the canonical attest");
    assert_eq!(
        parsed,
        Hello {
            socket: "deck".into(),
            server_pid: 4242,
            pane: "%7".into(),
            version: PiVersion::Supported,
            capabilities: Capabilities {
                signal: true,
                lifecycle: false
            },
        }
    );
    assert!(is_family(&hello(|_| {})));
    // every malformed variation is refused with a closed code
    let refused = |change: &dyn Fn(&mut serde_json::Value), code: &str| {
        assert_eq!(parse_hello(&hello(change)), Err(code), "{code}");
    };
    refused(&|l| l["v"] = 3.into(), "bad-version");
    refused(&|l| l["v"] = 1.into(), "bad-version");
    refused(&|l| l["v"] = serde_json::json!(2.5), "bad-frame");
    refused(&|l| l["v"] = serde_json::json!("2"), "bad-frame");
    refused(&|l| l["observerVersion"] = 1.into(), "bad-version");
    refused(&|l| l["observerVersion"] = 3.into(), "bad-version");
    refused(&|l| l["source"] = "claude-code".into(), "bad-frame");
    refused(&|l| l["protocol"] = "agent-lifecycle".into(), "bad-frame");
    refused(&|l| l["mode"] = "rpc".into(), "bad-mode");
    refused(&|l| l["mode"] = "print".into(), "bad-mode");
    refused(&|l| l["mode"] = "json".into(), "bad-mode");
    refused(&|l| l["mode"] = "gui".into(), "bad-frame");
    refused(&|l| l["server_pid"] = 1.into(), "bad-frame");
    refused(&|l| l["server_pid"] = (-4).into(), "bad-frame");
    refused(
        &|l| l["server_pid"] = serde_json::json!(4242.5),
        "bad-frame",
    );
    refused(
        &|l| l["server_pid"] = serde_json::json!(4294967296u64),
        "bad-frame",
    );
    refused(&|l| l["pane"] = "7".into(), "bad-frame");
    refused(&|l| l["pane"] = "%".into(), "bad-frame");
    refused(&|l| l["pane"] = "%12345678901".into(), "bad-frame");
    refused(&|l| l["pane"] = "%7;x".into(), "bad-frame");
    refused(&|l| l["socket"] = "../deck".into(), "bad-frame");
    refused(&|l| l["socket"] = "".into(), "bad-frame");
    refused(&|l| l["socket"] = "d".repeat(65).into(), "bad-frame");
    refused(&|l| l["piVersion"] = "".into(), "bad-frame");
    refused(&|l| l["piVersion"] = "1".repeat(33).into(), "bad-frame");
    refused(&|l| l["piVersion"] = "1.1.0 beta".into(), "bad-frame");
    refused(&|l| l["piVersion"] = 1.into(), "bad-frame");
    refused(
        &|l| l["capabilities"] = serde_json::json!({"signal": false, "lifecycle": false}),
        "bad-frame",
    );
    refused(
        &|l| l["capabilities"] = serde_json::json!({"signal": true}),
        "bad-frame",
    );
    refused(
        &|l| l["capabilities"] = serde_json::json!({"signal": true, "lifecycle": 1}),
        "bad-frame",
    );
    refused(
        &|l| {
            l["capabilities"] =
                serde_json::json!({"signal": true, "lifecycle": true, "tools": true})
        },
        "bad-frame",
    );
    refused(&|l| l["capabilities"] = true.into(), "bad-frame");
    // an unknown key, a key of another shape, a missing key
    refused(&|l| l["title"] = "x".into(), "bad-frame");
    refused(&|l| l["prompt"] = "x".into(), "bad-frame");
    refused(&|l| l["seq"] = 1.into(), "bad-frame");
    refused(&|l| l["state"] = "working".into(), "bad-frame");
    refused(&|l| l["nativeSessionId"] = "abc".into(), "bad-frame");
    for key in [
        "socket",
        "server_pid",
        "pane",
        "observerVersion",
        "piVersion",
        "mode",
        "capabilities",
    ] {
        refused(
            &|l| {
                l.as_object_mut().unwrap().remove(key);
            },
            if key == "observerVersion" {
                "bad-version"
            } else {
                "bad-frame"
            },
        );
    }
    // a repeated key is not "the last one wins"
    let twice = hello(|_| {}).replacen("\"pane\":\"%7\"", "\"pane\":\"%7\",\"pane\":\"%8\"", 1);
    assert_eq!(parse_hello(&twice), Err("bad-frame"));
    let twice = hello(|_| {}).replacen("\"signal\":true", "\"signal\":true,\"signal\":true", 1);
    assert_eq!(parse_hello(&twice), Err("bad-frame"));
    // without its nonce the claims are no attest, and no hello either
    refused(
        &|l| {
            l.as_object_mut().unwrap().remove("nonce");
        },
        "bad-frame",
    );
    // with a lease they are no attest but a frame, and no frame either
    assert!(matches!(
        parse_message(&hello(|l| l["lease"] = 3.into())),
        Ok(Message::Frame {
            frame: Err("bad-frame"),
            ..
        })
    ));
    // not an object, trailing bytes, an oversized line
    assert_eq!(parse_hello("[]"), Err("bad-frame"));
    assert_eq!(
        parse_hello(&format!("{} x", hello(|_| {}))),
        Err("bad-frame")
    );
    let long = hello(|l| l["socket"] = "d".repeat(60).into());
    assert!(long.len() < MAX_LINE);
    let padded = format!("{}{}", " ".repeat(MAX_LINE), long);
    assert_eq!(parse_hello(&padded), Err("bad-frame"));
}

#[test]
fn the_version_is_compatibility_and_nothing_else() {
    let version =
        |text: &str| parse_hello(&hello(|l| l["piVersion"] = text.into())).map(|h| h.version);
    assert_eq!(version("1.1.0"), Ok(PiVersion::Supported));
    assert_eq!(version("1.1.7"), Ok(PiVersion::Supported));
    assert_eq!(version("1.1.12345"), Ok(PiVersion::Supported));
    for other in [
        "1.2.0",
        "1.0.9",
        "2.1.0",
        "0.87.0",
        "1.1",
        "1.1.0.1",
        "1.1.0-rc.1",
        "1.1.x",
        "1.1.123456",
    ] {
        assert_eq!(version(other), Ok(PiVersion::Unsupported), "{other}");
    }
}

/// The observer's closed word for a session id it cannot put on a line:
/// its own shape, one word, nothing of the id, and never beside another
/// shape's field.
#[test]
fn an_id_that_cannot_be_sent_is_one_closed_word_and_no_other_shape() {
    assert_eq!(
        parse_frame(&frame(
            serde_json::json!({"seq": 4, "unavailable": "unsupported-native-id"})
        )),
        Ok(Frame::Unavailable { seq: 4 })
    );
    for body in [
        serde_json::json!({"seq": 1, "unavailable": "too-long"}),
        serde_json::json!({"seq": 1, "unavailable": ""}),
        serde_json::json!({"seq": 1, "unavailable": true}),
        serde_json::json!({"seq": 1, "unavailable": "unsupported-native-id", "nativeSessionId": "a"}),
        serde_json::json!({"seq": 1, "unavailable": "unsupported-native-id", "event": "start"}),
        serde_json::json!({"seq": 1, "unavailable": "unsupported-native-id", "reason": "quit"}),
        serde_json::json!({"seq": 1, "unavailable": "unsupported-native-id", "run": 1, "state": "working"}),
        serde_json::json!({"seq": 1, "unavailable": "unsupported-native-id", "length": 600}),
        serde_json::json!({"unavailable": "unsupported-native-id"}),
    ] {
        assert_eq!(
            parse_frame(&frame(body.clone())),
            Err("bad-frame"),
            "{body}"
        );
    }
    // which capability each shape belongs to: a reset and a heartbeat to none
    use crate::agent_pi::Capability;
    let of = |body: serde_json::Value| parse_frame(&frame(body)).unwrap().capability();
    assert_eq!(
        of(serde_json::json!({"seq": 1, "run": 1, "state": "working"})),
        Some(Capability::Signal)
    );
    assert_eq!(
        of(serde_json::json!({"seq": 1, "event": "snapshot", "nativeSessionId": "a"})),
        Some(Capability::Lifecycle)
    );
    assert_eq!(
        of(serde_json::json!({"seq": 1, "unavailable": "unsupported-native-id"})),
        Some(Capability::Lifecycle)
    );
    assert_eq!(of(serde_json::json!({"seq": 1, "reason": "quit"})), None);
    assert_eq!(of(serde_json::json!({"seq": 1})), None);
}

#[test]
fn frames_are_four_closed_shapes() {
    assert_eq!(
        parse_frame(&frame(serde_json::json!({"seq": 1}))),
        Ok(Frame::Ping { seq: 1 })
    );
    assert_eq!(
        parse_frame(&frame(
            serde_json::json!({"seq": 2, "run": 1, "state": "working"})
        )),
        Ok(Frame::Signal {
            seq: 2,
            run: 1,
            state: RunState::Working
        })
    );
    assert_eq!(
        parse_frame(&frame(
            serde_json::json!({"seq": 3, "run": 1, "state": "turn-done"})
        )),
        Ok(Frame::Signal {
            seq: 3,
            run: 1,
            state: RunState::TurnDone
        })
    );
    for (word, reason) in [
        ("new", ResetReason::New),
        ("resume", ResetReason::Resume),
        ("fork", ResetReason::Fork),
        ("reload", ResetReason::Reload),
        ("quit", ResetReason::Quit),
        ("fault", ResetReason::Fault),
    ] {
        assert_eq!(
            parse_frame(&frame(serde_json::json!({"seq": 4, "reason": word}))),
            Ok(Frame::Reset { seq: 4, reason })
        );
    }
    let lifecycle = |body: serde_json::Value| parse_frame(&frame(body));
    assert_eq!(
        lifecycle(
            serde_json::json!({"seq": 5, "event": "start", "nativeSessionId": "abc", "reason": "startup"})
        ),
        Ok(Frame::Lifecycle {
            seq: 5,
            event: SessionEvent::Start(StartReason::Startup),
            native: NativeId::Kept(PiNativeId("abc".into()))
        })
    );
    assert_eq!(
        lifecycle(serde_json::json!({"seq": 5, "event": "shutdown", "nativeSessionId": "abc", "reason": "quit"}))
            .map(|f| f.seq()),
        Ok(5)
    );
    assert!(matches!(
        lifecycle(serde_json::json!({"seq": 5, "event": "snapshot", "nativeSessionId": "abc"})),
        Ok(Frame::Lifecycle {
            event: SessionEvent::Snapshot,
            ..
        })
    ));
    // a word of the other event, a reason on a snapshot, no reason on a start
    for body in [
        serde_json::json!({"seq": 5, "event": "start", "nativeSessionId": "abc", "reason": "quit"}),
        serde_json::json!({"seq": 5, "event": "shutdown", "nativeSessionId": "abc", "reason": "startup"}),
        serde_json::json!({"seq": 5, "event": "snapshot", "nativeSessionId": "abc", "reason": "startup"}),
        serde_json::json!({"seq": 5, "event": "start", "nativeSessionId": "abc"}),
        serde_json::json!({"seq": 5, "event": "end", "nativeSessionId": "abc", "reason": "quit"}),
        serde_json::json!({"seq": 5, "event": "start", "reason": "startup"}),
        serde_json::json!({"seq": 5, "nativeSessionId": "abc"}),
    ] {
        assert_eq!(lifecycle(body.clone()), Err("bad-frame"), "{body}");
    }
}

#[test]
fn a_pi_run_can_never_say_that_input_is_needed() {
    for state in ["needs-input", "idle", "done", "ready", "", "Working"] {
        assert_eq!(
            parse_frame(&frame(
                serde_json::json!({"seq": 1, "run": 1, "state": state})
            )),
            Err("bad-frame"),
            "{state}"
        );
    }
    // the closed type has two words and the store maps exactly those
    let _exhaustive = |state: RunState| match state {
        RunState::Working | RunState::TurnDone => (),
    };
}

#[test]
fn numbers_are_whole_positive_and_exact() {
    let seq = |value: serde_json::Value| parse_frame(&frame(serde_json::json!({ "seq": value })));
    assert_eq!(seq(1.into()), Ok(Frame::Ping { seq: 1 }));
    assert_eq!(
        seq(serde_json::json!(9007199254740991u64)),
        Ok(Frame::Ping {
            seq: MAX_SAFE_INTEGER
        })
    );
    for bad in [
        serde_json::json!(0),
        serde_json::json!(-1),
        serde_json::json!(1.5),
        serde_json::json!(1e3),
        serde_json::json!(9007199254740992u64),
        serde_json::json!(18446744073709551615u64),
        serde_json::json!("1"),
        serde_json::json!(null),
        serde_json::json!(true),
        serde_json::json!([1]),
    ] {
        assert_eq!(seq(bad.clone()), Err("bad-frame"), "{bad}");
    }
    // JSON has no NaN or Infinity: such a line is not JSON at all
    for text in ["NaN", "Infinity", "-Infinity", "1e400"] {
        let line = format!(
            "{{\"protocol\":\"pi-observer\",\"v\":2,\"source\":\"pi\",\"lease\":3,\"nonce\":\"{NONCE}\",\"seq\":{text}}}"
        );
        assert_eq!(parse_message(&line), Err("bad-frame"), "{text}");
    }
    let run = |value: serde_json::Value| {
        parse_frame(&frame(
            serde_json::json!({"seq": 1, "run": value, "state": "working"}),
        ))
    };
    assert!(run(1.into()).is_ok());
    for bad in [
        serde_json::json!(0),
        serde_json::json!(-2),
        serde_json::json!(2.5),
    ] {
        assert_eq!(run(bad.clone()), Err("bad-frame"), "{bad}");
    }
}

#[test]
fn a_frame_carries_no_field_of_another_shape_and_no_content() {
    for body in [
        serde_json::json!({"seq": 1, "run": 1}),
        serde_json::json!({"seq": 1, "state": "working"}),
        serde_json::json!({"seq": 1, "run": 1, "state": "working", "reason": "quit"}),
        serde_json::json!({"seq": 1, "reason": "stopped"}),
        serde_json::json!({"seq": 1, "reason": "quit", "run": 1}),
        serde_json::json!({"seq": 1, "pane": "%7"}),
        serde_json::json!({"seq": 1, "socket": "deck"}),
        serde_json::json!({"seq": 1, "mode": "tui"}),
        serde_json::json!({"seq": 1, "capabilities": {"signal": true, "lifecycle": true}}),
        serde_json::json!({"seq": 1, "title": "Allow tool?"}),
        serde_json::json!({"seq": 1, "prompt": "do it"}),
        serde_json::json!({"seq": 1, "messages": []}),
        serde_json::json!({"seq": 1, "path": "/tmp/x"}),
        serde_json::json!({"seq": 1, "aborted": true}),
        serde_json::json!({"run": 1, "state": "working"}),
        serde_json::json!({}),
    ] {
        assert_eq!(
            parse_frame(&frame(body.clone())),
            Err("bad-frame"),
            "{body}"
        );
    }
    // an attest is not a frame and a frame is not an attest
    assert_eq!(parse_frame(&hello(|_| {})), Err("bad-frame"));
    assert_eq!(
        parse_hello(&frame(serde_json::json!({"seq": 1}))),
        Err("bad-frame")
    );
    let twice =
        frame(serde_json::json!({"seq": 1})).replacen("\"seq\":1", "\"seq\":1,\"seq\":2", 1);
    assert_eq!(parse_frame(&twice), Err("bad-frame"));
    assert_eq!(
        parse_frame(&frame(serde_json::json!({"seq": 1})).replace("\"v\":2", "\"v\":3")),
        Err("bad-version")
    );
}

#[test]
fn a_native_id_is_pi_s_own_shape_and_never_cut_short() {
    let named = |id: &str| {
        parse_frame(&frame(
            serde_json::json!({"seq": 1, "event": "snapshot", "nativeSessionId": id}),
        ))
    };
    let kept = |id: &str| {
        Ok(Frame::Lifecycle {
            seq: 1,
            event: SessionEvent::Snapshot,
            native: NativeId::Kept(PiNativeId(id.to_string())),
        })
    };
    // Pi's default (a UUIDv7) and what `--session-id` accepts
    for id in [
        "0199e3a4-7c2b-7f31-9a50-3b2c1d0e4f5a",
        "probe.custom-ID_123",
        "a",
        "A9",
    ] {
        assert_eq!(named(id), kept(id), "{id}");
        // Claude's validator would refuse most of them: the two never mix
        if id != "0199e3a4-7c2b-7f31-9a50-3b2c1d0e4f5a" {
            assert!(!crate::agent_reentry::claude::native_id_ok(id), "{id}");
        }
    }
    for id in [
        "", "-a", "a-", ".a", "a.", "a b", "a/b", "../x", "a\u{e9}", "a\n", "a;b",
    ] {
        assert_eq!(named(id), Err("bad-native-id"), "{id:?}");
    }
    let longest = "a".repeat(MAX_NATIVE_ID);
    assert_eq!(named(&longest), kept(&longest));
    let longer = "a".repeat(MAX_NATIVE_ID + 1);
    assert_eq!(
        named(&longer),
        Ok(Frame::Lifecycle {
            seq: 1,
            event: SessionEvent::Snapshot,
            native: NativeId::Unsupported
        }),
        "reported as unsupported, not shortened"
    );
}

/// What Deck says: one line per connection. Every answer that is not a
/// refusal carries the nonce the next message must carry, and every answer
/// to an attest or a frame the two capability bits as they are.
#[test]
fn answers_are_six_closed_lines() {
    let nonce = Nonce::parse(NONCE).unwrap();
    let only_signal = Capabilities {
        signal: true,
        lifecycle: false,
    };
    assert_eq!(
        nonce_line(&nonce),
        format!("{{\"protocol\":\"pi-observer\",\"v\":2,\"nonce\":\"{NONCE}\"}}\n")
    );
    assert_eq!(
        accepted_line(41, &nonce, only_signal),
        format!(
            "{{\"protocol\":\"pi-observer\",\"v\":2,\"accepted\":41,\"nonce\":\"{NONCE}\",\"capabilities\":{{\"signal\":true,\"lifecycle\":false}}}}\n"
        )
    );
    assert_eq!(
        ack_line(7, &nonce, only_signal),
        format!(
            "{{\"protocol\":\"pi-observer\",\"v\":2,\"ack\":7,\"nonce\":\"{NONCE}\",\"capabilities\":{{\"signal\":true,\"lifecycle\":false}}}}\n"
        )
    );
    assert_eq!(
        revoked_line(9, &nonce, Capabilities::NONE),
        format!(
            "{{\"protocol\":\"pi-observer\",\"v\":2,\"revoked\":9,\"nonce\":\"{NONCE}\",\"capabilities\":{{\"signal\":false,\"lifecycle\":false}}}}\n"
        )
    );
    assert_eq!(
        again_line(&nonce),
        format!("{{\"protocol\":\"pi-observer\",\"v\":2,\"again\":true,\"nonce\":\"{NONCE}\"}}\n")
    );
    assert_eq!(
        refused_line("duplicate-observer"),
        "{\"protocol\":\"pi-observer\",\"v\":2,\"refused\":\"duplicate-observer\"}\n"
    );
    // each is one JSON object with exactly its keys, and only a refusal is
    // without a nonce
    for (line, key, keys) in [
        (nonce_line(&nonce), "nonce", 3),
        (accepted_line(1, &nonce, only_signal), "accepted", 5),
        (ack_line(7, &nonce, only_signal), "ack", 5),
        (revoked_line(7, &nonce, only_signal), "revoked", 5),
        (again_line(&nonce), "again", 4),
        (refused_line("busy"), "refused", 3),
    ] {
        let value: serde_json::Value = serde_json::from_str(&line).expect("JSON");
        assert_eq!(value.as_object().unwrap().len(), keys, "{line}");
        assert!(value.get(key).is_some() && line.ends_with('\n'));
        assert_eq!(value.get("nonce").is_some(), key != "refused", "{line}");
        assert_eq!(
            value.get("capabilities").is_some(),
            matches!(key, "accepted" | "ack" | "revoked"),
            "{line}"
        );
        // no answer is two of them
        let named = ["accepted", "ack", "revoked", "again", "refused"]
            .iter()
            .filter(|other| value.get(**other).is_some())
            .count();
        assert_eq!(named, usize::from(key != "nonce"), "{line}");
    }
    // a refusal code is a plain word: no quote or escape can appear
    for code in REFUSALS {
        assert!(code.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'));
    }
    // every refusal has its own drop-diagnostics name
    for code in REFUSALS {
        assert_eq!(drop_reason(code), format!("pi-{code}"));
    }
    assert_eq!(drop_reason("anything-else"), "other");
    for code in [
        "peer-executed",
        "no-instance",
        "no-challenge",
        "sender-mismatch",
    ] {
        assert!(REFUSALS.contains(&code));
    }
}

/// A line of another wire version is told so in the framing IT claims: an
/// observer reads nothing that is not of its own version, and takes this
/// refusal as final. Every other refusal is said in this version.
/// Deck's own line is one exact line that no observer's message can be: its
/// source word is not an observer's, so the parser of the family refuses it.
/// Its answer names a place and nothing else.
#[test]
fn deck_s_own_line_is_no_observer_s_and_its_answer_is_a_place() {
    let own = place_line();
    assert_eq!(
        own,
        "{\"protocol\":\"pi-observer\",\"v\":2,\"source\":\"deck\"}"
    );
    assert!(is_family(&own), "the listener hands it to this family");
    assert!(asks_place(&own));
    assert_eq!(parse_message(&own).err(), Some("bad-frame"));
    // nothing an observer sends is taken for it, and nothing like it is
    for other in [
        hello(|_| {}),
        format!("{own} "),
        format!(" {own}"),
        own.replace("deck", "pi"),
        own.replace("\"v\":2", "\"v\":1"),
        own.replace('}', ",\"lease\":1}"),
    ] {
        assert!(!asks_place(&other), "{other}");
    }
    assert_eq!(
        placed_line(41),
        "{\"protocol\":\"pi-observer\",\"v\":2,\"place\":41}\n"
    );
    assert_eq!(placed(&placed_line(41)), Some(41));
    assert_eq!(placed(&placed_line(u64::MAX)), Some(u64::MAX));
    let nonce = Nonce::parse(NONCE).unwrap();
    for other in [
        nonce_line(&nonce),
        again_line(&nonce),
        refused_line("busy"),
        "{\"protocol\":\"agent-lifecycle\",\"v\":2,\"place\":41}\n".to_string(),
        "{\"protocol\":\"pi-observer\",\"v\":1,\"place\":41}\n".to_string(),
        "{\"protocol\":\"pi-observer\",\"v\":2,\"place\":-1}\n".to_string(),
        "{\"protocol\":\"pi-observer\",\"v\":2,\"place\":\"41\"}\n".to_string(),
        String::new(),
    ] {
        assert_eq!(placed(&other), None, "{other}");
    }
}

#[test]
fn another_version_is_refused_in_its_own_framing() {
    let old = "{\"protocol\":\"pi-observer\",\"v\":1,\"source\":\"pi\",\"seq\":1}";
    assert_eq!(parse_message(old), Err("bad-version"));
    assert_eq!(
        refusal_of(old, "bad-version"),
        "{\"protocol\":\"pi-observer\",\"v\":1,\"refused\":\"bad-version\"}\n"
    );
    let newer = old.replace("\"v\":1", "\"v\":7");
    assert_eq!(
        refusal_of(&newer, "bad-version"),
        "{\"protocol\":\"pi-observer\",\"v\":7,\"refused\":\"bad-version\"}\n"
    );
    // a version that is none, or a line that is no JSON, is answered in this one
    for line in [
        old.replace("\"v\":1", "\"v\":0"),
        old.replace("\"v\":1", "\"v\":100"),
        old.replace("\"v\":1", "\"v\":\"1\""),
        "not json".to_string(),
    ] {
        assert_eq!(
            refusal_of(&line, "bad-version"),
            refused_line("bad-version"),
            "{line}"
        );
    }
    // and no other refusal borrows a framing
    assert_eq!(refusal_of(old, "bad-frame"), refused_line("bad-frame"));
    // the observer's own bad version (the wire's is this one) is this framing's
    let own = hello(|l| l["observerVersion"] = 1.into());
    assert_eq!(refusal_of(&own, "bad-version"), refused_line("bad-version"));
}

#[test]
fn capability_sets_compose_without_a_third_value() {
    let both = Capabilities {
        signal: true,
        lifecycle: true,
    };
    let signal = Capabilities::NONE.with(Capability::Signal, true);
    assert!(signal.signal && !signal.lifecycle);
    assert!(signal.has(Capability::Signal) && !signal.has(Capability::Lifecycle));
    assert!(!Capabilities::NONE.any());
    assert_eq!(both.with(Capability::Lifecycle, false), signal);
    // what two sets share: never more than either
    assert_eq!(both.and(signal), signal);
    assert_eq!(signal.and(both), signal);
    assert_eq!(
        signal.and(Capabilities::NONE.with(Capability::Lifecycle, true)),
        Capabilities::NONE
    );
}

mod native {
    //! The backend half of the native Pi run (`scripts/pi_native.py`). Test-only:
    //! one ignored test that is a long-running harness, not an assertion.
    //!
    //! # What runs here
    //! Production's own code, not a model of it:
    //! - the status listener (`agent_status::serve_counting_for_tests`: the
    //!   accept step, the bounded read of every connection's first line, the
    //!   one dispatcher and its family dispatch, the Pi transport started the
    //!   way production starts it, with `World::live` — the kernel and its
    //!   watch of every peer's execution instance, the bundled tmux beside
    //!   this executable, Pi's settings file);
    //! - the Board poll's body (`commands::poll_from_listing`: reconcile, the
    //!   projection, the coverage word) on the one-shot form of the same
    //!   snapshot, at the webview's interval;
    //! - the Settings commands (`agent_hooks_set("pi", …)`,
    //!   `agent_pi_lifecycle_set`, `agent_hooks_status`), the boot migration
    //!   and the dismiss command (`notify_dismiss`);
    //! - the session observations a lifecycle frame wrote
    //!   (`agent_reentry::lifecycle`). Their native ids appear in this
    //!   harness's trace so a run can compare them with what Pi itself
    //!   logged; that trace stays in the experiment root and is never
    //!   committed;
    //! - what the scheduler asks before it sends (`generation_evidence` by point
    //!   reads, `pi_foreground`).
    //!
    //! The harness adds a command line on standard input and, on standard
    //! output, what those returned (`@tap {json}` lines). app.log is not written
    //! in a test build, so the drop diagnostics are read where the listener
    //! counts them. Nothing here types into a pane, starts a program or a tmux
    //! server, or reads a pane's content.
    //!
    //! # Where it may run
    //! Only as an executable inside ONE experiment root that also holds its
    //! home, its temporary directory, its tmux socket directory, Pi's agent
    //! directory and the bundled tmux (`root`). Anything else is refused before
    //! a socket is bound or a file is read. The driver places this binary at
    //! `<root>/home/Applications/deck.app/Contents/MacOS/deck-app`: under that
    //! relocated home it is a stable install by the ordinary rule
    //! (`tmux_lifecycle::stable_installed_bundle`), so the Settings switch writes
    //! Pi's settings file there, and only there. No production flag, variable
    //! or path makes that possible anywhere else.
    //!
    //! The driver starts the tmux server with the server defaults this harness
    //! writes (`tmux_conf_text` for its fixed data directory, so the panes carry
    //! the status socket it listens on), registers every process with
    //! `scripts/edr_runtime.py experiment`, and stops them through it.

    use std::collections::BTreeMap;
    use std::io::{BufRead, Write};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use serde_json::{json, Value};

    use crate::agent_pi::transport::probe;
    use crate::agent_status::{self, DropCounts};
    use crate::sync::LockRecover;
    use crate::tmux::PaneRow;

    /// The webview's poll interval (`ui/js/state.js`, `POLL_MS`).
    const POLL: Duration = Duration::from_millis(2500);
    /// How often the store is looked at between polls. Every accepted change
    /// allocates the next episode number, so a change this misses still shows
    /// as a number that was skipped.
    const SAMPLE: Duration = Duration::from_millis(2);

    /// The experiment root, or `None` without the opt-in. Everything this
    /// process reads or writes outside itself must be inside it.
    fn root() -> Option<PathBuf> {
        let root = PathBuf::from(std::env::var_os("DECK_PI_NATIVE_ROOT")?);
        assert!(
            root.starts_with("/private/tmp")
                && std::fs::canonicalize(&root).is_ok_and(|real| real == root),
            "the experiment root is not a real directory under /private/tmp"
        );
        let inside = |what: &str, path: PathBuf| {
            assert!(
                path.is_absolute() && path != root && path.starts_with(&root),
                "{what} is outside the experiment root"
            );
        };
        inside(
            "the executable",
            std::env::current_exe().expect("an executable path"),
        );
        for variable in ["HOME", "TMPDIR", "TMUX_TMPDIR", "PI_CODING_AGENT_DIR"] {
            inside(
                variable,
                PathBuf::from(std::env::var_os(variable).unwrap_or_default()),
            );
        }
        inside("the home directory", dirs::home_dir().unwrap_or_default());
        inside("the data directory", crate::datadir::deck_dir());
        inside("the bundled tmux", PathBuf::from(crate::tmux::tmux_bin()));
        Some(root)
    }

    fn wall() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_millis() as u64)
    }

    /// One line to the driver.
    fn say(mut event: Value, started: Instant) {
        if let Some(object) = event.as_object_mut() {
            object.insert("t".into(), json!(started.elapsed().as_millis() as u64));
            object.insert("wall".into(), json!(wall()));
        }
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "@tap {event}");
        let _ = out.flush();
    }

    /// What the Board asked for and what the last listing held.
    #[derive(Default)]
    struct Board {
        names: Mutex<Vec<String>>,
        rows: Mutex<Vec<PaneRow>>,
        exits: Mutex<crate::shell_exit::ExitEvidence>,
    }

    /// The Signal part of one poll's payload, as the webview receives it.
    const POLL_KEYS: [&str; 11] = [
        "name",
        "alive",
        "fg",
        "agent",
        "pi_signal",
        "pi_session",
        "codex_signal",
        "episode",
        "episode_viewed",
        "finish_fg",
        "exited_normally",
    ];

    /// One Board poll: production's body over a listing read now.
    fn poll(board: &Board) -> Value {
        let mut exits = board.exits.lock_or_recover();
        let names = board.names.lock_or_recover().clone();
        let listing = crate::tmux::snapshot_or_empty_with(&crate::tmux::tmux);
        let rows = listing.as_ref().map(|snapshot| snapshot.rows.clone()).ok();
        let polled = crate::commands::poll_from_listing(
            names,
            Vec::new(),
            false,
            listing,
            crate::procinfo::processes,
            &mut exits,
            &|_| false,
        );
        let (sessions, rows) = match (polled, rows) {
            (Ok(sessions), Some(rows)) => (sessions, rows),
            (Err(error), _) => return json!({ "error": error.code() }),
            (Ok(_), None) => return json!({ "error": "no-listing" }),
        };
        let sessions: Vec<Value> = serde_json::to_value(&sessions)
            .ok()
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_default()
            .into_iter()
            .map(|session| {
                let kept: serde_json::Map<String, Value> = POLL_KEYS
                    .iter()
                    .filter_map(|key| Some(((*key).to_string(), session.get(*key)?.clone())))
                    .collect();
                Value::Object(kept)
            })
            .collect();
        let asked = scheduler(&rows);
        *board.rows.lock_or_recover() = rows;
        json!({ "sessions": sessions, "scheduler": asked })
    }

    /// What the scheduler would know about each session's Signal target right
    /// now: the tick's evidence and the pre-fire question, both by point reads.
    fn scheduler(rows: &[PaneRow]) -> Value {
        let evidence = agent_status::generation_evidence(
            rows,
            agent_status::live_generation,
            agent_status::foreground_name,
        );
        let asked: BTreeMap<String, Value> = agent_status::signal_targets(rows)
            .into_iter()
            .map(|(session, row)| {
                let seen = evidence.get(&session).copied().unwrap_or_default();
                let pre_fire = agent_status::pi_foreground(
                    row.server_pid,
                    &row.pane_id,
                    row.pane_pid,
                    agent_status::live_generation,
                    agent_status::foreground_name,
                );
                (
                    session,
                    json!({ "pi": seen.pi, "pi_signal": seen.pi_signal, "pre_fire_pi": pre_fire }),
                )
            })
            .collect();
        json!(asked)
    }

    /// What the store, the transport and the diagnostics hold right now.
    fn sample(board: &Board, drops: &DropCounts) -> Value {
        let rows = board.rows.lock_or_recover().clone();
        let store: BTreeMap<String, Value> = agent_status::projections(&rows)
            .into_iter()
            .map(|(session, seen)| {
                (
                    session,
                    json!({ "state": seen.state, "episode": seen.episode, "viewed": seen.viewed }),
                )
            })
            .collect();
        let announced: BTreeMap<String, u32> =
            crate::notify::announced_for_tests().into_iter().collect();
        let leases: Vec<Value> = probe::leases()
            .into_iter()
            .map(|seen| {
                json!({
                    "lease": seen.lease, "session": seen.session, "run": seen.run, "open": seen.open,
                    "next_seq": seen.next_seq,
                    // what the lease may still do, as the transport holds it
                    "signal": seen.usable.signal, "lifecycle": seen.usable.lifecycle,
                })
            })
            .collect();
        // every frame that was answered `revoked`, in order
        let revoked: Vec<Value> = probe::revoked()
            .into_iter()
            .map(|(session, capability)| json!({ "session": session, "capability": capability }))
            .collect();
        let ended: Vec<Value> = probe::ended()
            .into_iter()
            .map(|(session, code)| json!({ "session": session, "code": code }))
            .collect();
        let sessions: Vec<Value> = crate::agent_reentry::lifecycle::pi_sessions_for_tests()
            .into_iter()
            .map(|(lease, id, last)| json!({ "lease": lease, "id": id, "last": said(last) }))
            .collect();
        // leases whose session is not observed for an id this build does not keep
        let unsupported = crate::agent_reentry::lifecycle::pi_unsupported_for_tests();
        json!({
            "store": store,
            // how often a line was neither believed nor blamed, and how
            // many nonces wait for their attest
            "agains": probe::agains(),
            "challenges": probe::challenges(),
            "leases": leases,
            "ended": ended,
            "sessions": sessions,
            "unsupported": unsupported,
            "revoked": revoked,
            "pending": probe::pending(),
            "drops": drops.counts(),
            "announced": announced,
        })
    }

    /// Pi's part of the Settings status command's reply.
    fn settings() -> Value {
        let status = serde_json::to_value(agent_status::agent_hooks_status()).unwrap_or_default();
        json!({
            "pi": status.get("pi"),
            "pi_lifecycle": status.get("pi_lifecycle"),
            "pi_installation": status.get("pi_installation"),
        })
    }

    /// The last thing a runtime said of its session, as one closed word.
    fn said(event: crate::agent_pi::SessionEvent) -> String {
        use crate::agent_pi::SessionEvent;
        match event {
            SessionEvent::Start(reason) => format!("start-{reason:?}").to_lowercase(),
            SessionEvent::Shutdown(reason) => format!("shutdown-{reason:?}").to_lowercase(),
            SessionEvent::Snapshot => "snapshot".into(),
        }
    }

    /// One command of the driver; the reply's body.
    fn answer(command: &Value, board: &Board, drops: &DropCounts) -> Value {
        let text = |key: &str| command.get(key).and_then(Value::as_str).unwrap_or_default();
        match text("op") {
            "names" => {
                *board.names.lock_or_recover() = command
                    .get("names")
                    .and_then(Value::as_array)
                    .map(|names| {
                        names
                            .iter()
                            .filter_map(|name| name.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                json!({})
            }
            "poll" => json!({ "poll": poll(board), "now": sample(board, drops) }),
            "status" => settings(),
            "signal" => {
                let enable = command.get("enable").and_then(Value::as_bool) == Some(true);
                let result = agent_status::agent_hooks_set(crate::agent_pi::SOURCE.into(), enable);
                json!({
                    "ok": result.is_ok(),
                    "error": result.err().map(|error| error.message().to_owned()),
                    "status": settings(),
                })
            }
            "lifecycle" => {
                let enable = command.get("enable").and_then(Value::as_bool) == Some(true);
                let result = crate::agent_pi::config::agent_pi_lifecycle_set(enable);
                json!({
                    "ok": result.is_ok(),
                    "error": result.err().map(|error| error.message().to_owned()),
                    "status": settings(),
                })
            }
            "migrate" => {
                crate::agent_pi::config::migrate_on_boot();
                settings()
            }
            "viewed" => {
                let episode = command.get("episode").and_then(Value::as_u64).unwrap_or(0);
                let result = crate::notify::notify_dismiss(text("session").into(), episode);
                json!({ "ok": result.is_ok() })
            }
            _ => json!({ "error": "unknown-op" }),
        }
    }

    /// Run until the driver says `quit` or closes standard input.
    #[test]
    #[ignore = "the native Pi run's backend: started by scripts/pi_native.py inside an experiment root"]
    fn tap() {
        let Some(root) = root() else {
            eprintln!("agent_pi::tests::native::tap: no DECK_PI_NATIVE_ROOT, nothing to do");
            return;
        };
        let started = Instant::now();
        let data = root.join("deck");
        crate::datadir::create_private_dir(&data).expect("the data directory");
        let conf = data.join("tmux.conf");
        crate::datadir::write_private(&conf, crate::tmux::tmux_conf_text(&data).as_bytes())
            .expect("the server defaults");
        let socket = data.join("status.sock");
        let listener = agent_status::listen_at(&socket).expect("the status socket");
        let stop = Arc::new(AtomicBool::new(false));
        let drops = DropCounts::new();
        let board = Arc::new(Board::default());

        let listening = std::thread::spawn({
            let (stop, drops) = (stop.clone(), drops.clone());
            move || agent_status::serve_counting_for_tests(listener, stop, &drops)
        });
        let polling = std::thread::spawn({
            let (stop, board) = (stop.clone(), board.clone());
            move || {
                let mut last = Value::Null;
                let mut next = Instant::now();
                while !stop.load(Ordering::Acquire) {
                    if Instant::now() >= next {
                        next += POLL;
                        let now = poll(&board);
                        if now != last {
                            say(json!({ "poll": now }), started);
                            last = now;
                        }
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        });
        let sampling = std::thread::spawn({
            let (stop, board, drops) = (stop.clone(), board.clone(), drops.clone());
            move || {
                let mut last = Value::Null;
                while !stop.load(Ordering::Acquire) {
                    let now = sample(&board, &drops);
                    if now != last {
                        say(json!({ "now": now }), started);
                        last = now;
                    }
                    std::thread::sleep(SAMPLE);
                }
            }
        });

        say(
            json!({
                "ready": {
                    "status_sock": path_text(&socket),
                    "conf": path_text(&conf),
                    "socket": crate::tmux::socket(),
                    "tmux": crate::tmux::tmux_bin(),
                    "settings": settings(),
                }
            }),
            started,
        );
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            let Ok(command) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if command.get("op").and_then(Value::as_str) == Some("quit") {
                break;
            }
            let mut reply = answer(&command, &board, &drops);
            if let Some(object) = reply.as_object_mut() {
                object.insert("re".into(), command.get("id").cloned().unwrap_or_default());
            }
            say(reply, started);
        }
        stop.store(true, Ordering::Release);
        for thread in [listening, polling, sampling] {
            let _ = thread.join();
        }
        say(json!({ "bye": true }), started);
    }

    fn path_text(path: &Path) -> String {
        path.display().to_string()
    }
}
