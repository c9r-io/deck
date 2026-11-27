use super::*;
use serde_json::json;

fn rule(id: &str, extra: Value) -> Rule {
    let mut v = json!({"id": id, "source": "local", "badge": id, "projectId": "p1",
        "columnId": "c1", "cmd": "claude", "template": "Triage",
        "localMatch": {"producer": "ci", "auto": true}});
    if let (Some(base), Some(more)) = (v.as_object_mut(), extra.as_object()) {
        for (k, val) in more {
            base.insert(k.clone(), val.clone());
        }
    }
    serde_json::from_value(v).unwrap()
}

fn cfg(rules: Vec<Rule>) -> Config {
    Config {
        slack_enabled: false,
        local_enabled: true,
        rules,
    }
}

fn note(event: &str, extra: Value) -> Value {
    let mut v = json!({"version": 1, "producer": "ci", "event_id": event, "title": "Build failed",
        "body": "step 3\nfailed"});
    if let (Some(base), Some(more)) = (v.as_object_mut(), extra.as_object()) {
        for (k, val) in more {
            base.insert(k.clone(), val.clone());
        }
    }
    v
}

fn rev(_: &Rule) -> Option<String> {
    Some("r".repeat(64))
}

fn only_notice() -> Notice {
    with_store(|s| Ok(s.doc.notices[0].clone())).unwrap()
}

#[test]
fn accepted_is_on_disk_before_the_receipt() {
    with_test_inbox(|path| {
        let (r, changed) = publish(&note("e1", json!({})), 100).unwrap();
        assert_eq!(r.status, "accepted");
        assert!(r.applied && changed);
        assert_eq!(
            r.notification_key, "e1",
            "the object key defaults to the event id"
        );
        let disk: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(disk["events"][0]["eventId"], "e1");
        assert_eq!(disk["notices"][0]["title"], "Build failed");
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    });
}

#[test]
fn a_failed_write_accepts_nothing() {
    with_test_inbox(|path| {
        FAIL_WRITE.with(|f| f.set(true));
        let err = publish(&note("e1", json!({})), 100).unwrap_err();
        FAIL_WRITE.with(|f| f.set(false));
        assert_eq!(err.code, "INTERNAL");
        assert!(err.retryable());
        assert!(!path.exists());
        // the retry with the same id is the first acceptance
        assert_eq!(
            publish(&note("e1", json!({})), 101).unwrap().0.status,
            "accepted"
        );
    });
}

#[test]
fn same_event_is_a_duplicate_and_other_content_a_conflict() {
    with_test_inbox(|_| {
        let first = publish(&note("e1", json!({})), 100).unwrap().0;
        // a retry after a lost receipt: same id, same content
        let again = publish(&note("e1", json!({})), 200).unwrap().0;
        assert_eq!(again.status, "duplicate");
        assert_eq!(again.received_at, first.received_at);
        // normalization happens before the fingerprint: trailing spaces and
        // CRLF are the same content
        let spaced = publish(
            &note(
                "e1",
                json!({"title": "  Build failed ", "body": "step 3\r\nfailed\n"}),
            ),
            300,
        )
        .unwrap()
        .0;
        assert_eq!(spaced.status, "duplicate");
        let other = publish(&note("e1", json!({"title": "Other"})), 400).unwrap_err();
        assert_eq!(other.code, "CONFLICT");
        assert!(!other.retryable());
        // another producer's equal event id is its own
        let mine = publish(&note("e1", json!({"producer": "ci-2"})), 500)
            .unwrap()
            .0;
        assert_eq!(mine.status, "accepted");
        assert_eq!(with_store(|s| Ok(s.doc.notices.len())).unwrap(), 2);
    });
}

#[test]
fn input_is_closed_and_bounded() {
    with_test_inbox(|_| {
        for (bad, code) in [
            (
                json!({"producer": "ci", "event_id": "e", "title": "t"}),
                "INVALID_INPUT",
            ),
            (note("e", json!({"version": 2})), "UNSUPPORTED_VERSION"),
            (note("e", json!({"version": "1"})), "INVALID_INPUT"),
            (note("e", json!({"target": "card"})), "INVALID_INPUT"),
            (note("e", json!({"callback": "x"})), "INVALID_INPUT"),
            (note("e", json!({"cmd": "rm -rf"})), "INVALID_INPUT"),
            (note("e", json!({"producer": "Slack"})), "INVALID_INPUT"),
            (note("e", json!({"event_id": "has space"})), "INVALID_INPUT"),
            (note("e", json!({"title": " \u{200B} "})), "INVALID_INPUT"),
            (note("e", json!({"title": "x".repeat(600)})), "TOO_LARGE"),
            (
                note("e", json!({"body": "x".repeat(MAX_BODY_BYTES + 1)})),
                "TOO_LARGE",
            ),
            (
                note("e", json!({"url": "javascript:alert(1)"})),
                "INVALID_INPUT",
            ),
            (note("e", json!({"url": "https://a b"})), "INVALID_INPUT"),
            (note("e", json!({"operation": "delete"})), "INVALID_INPUT"),
            (note("e", json!({"operation": "resolve"})), "INVALID_INPUT"),
            (
                note("e", json!({"labels": {"Bad Key": "v"}})),
                "INVALID_INPUT",
            ),
        ] {
            assert_eq!(normalize(&bad).unwrap_err().code, code, "{bad}");
        }
        let n = normalize(&note(
            "e",
            json!({"title": "a\tb\u{202E}c\nd", "body": "x\u{0007}y\n\tz",
                "labels": {"env": " dev "}, "url": "https://ci.example/1"}),
        ))
        .unwrap();
        assert_eq!(n.title, "a bc d");
        assert_eq!(n.body, "xy\n\tz");
        assert_eq!(n.labels["env"], "dev");
        assert_eq!(n.kind, "notification");
        assert_eq!(n.key, "e");
    });
}

#[test]
fn revisions_order_updates_and_stale_ones_change_nothing() {
    with_test_inbox(|_| {
        let k = json!({"notification_key": "build-7"});
        let with = |rev: u64, title: &str| {
            let mut v = note(&format!("e{rev}-{title}"), k.clone());
            v["revision"] = json!(rev);
            v["title"] = json!(title);
            v
        };
        publish(&with(2, "two"), 100).unwrap();
        let old = publish(&with(1, "one"), 101).unwrap().0;
        assert_eq!(old.status, "accepted");
        assert!(!old.applied);
        assert_eq!(old.reason.as_deref(), Some("stale-revision"));
        assert_eq!(only_notice().title, "two");
        publish(&with(3, "three"), 102).unwrap();
        assert_eq!(only_notice().title, "three");
        assert_eq!(only_notice().revision, 3);
        // an unversioned event cannot overwrite a versioned object
        let plain = publish(&note("plain", k.clone()), 103).unwrap().0;
        assert!(!plain.applied);
        // unversioned producers update in arrival order
        publish(
            &note("a", json!({"notification_key": "k2", "title": "A"})),
            104,
        )
        .unwrap();
        publish(
            &note("b", json!({"notification_key": "k2", "title": "B"})),
            105,
        )
        .unwrap();
        let titles: Vec<String> =
            with_store(|s| Ok(s.doc.notices.iter().map(|n| n.title.clone()).collect())).unwrap();
        assert!(titles.contains(&"B".to_string()));
    });
}

#[test]
fn resolve_and_a_new_occurrence() {
    with_test_inbox(|_| {
        let k = json!({"notification_key": "job"});
        publish(&note("e1", k.clone()), 100).unwrap();
        let resolve = json!({"version": 1, "producer": "ci", "event_id": "e2",
            "notification_key": "job", "operation": "resolve"});
        let r = publish(&resolve, 101).unwrap().0;
        assert_eq!(r.notification.external, Some(External::Resolved));
        assert_eq!(only_notice().occurrence, 1);
        // failing again after the resolve is the next occurrence, open again
        let (again, changed) = publish(&note("e3", k.clone()), 102).unwrap();
        assert!(changed);
        assert_eq!(again.notification.occurrence, Some(2));
        assert_eq!(again.notification.external, Some(External::Active));
        assert_eq!(again.notification.attention, Some(Attention::Open));
        // a withdraw of a notification Deck never saw is recorded, applies nothing
        let w = json!({"version": 1, "producer": "ci", "event_id": "w1",
            "notification_key": "never", "operation": "withdraw"});
        let w = publish(&w, 103).unwrap().0;
        assert!(!w.applied);
        assert_eq!(w.reason.as_deref(), Some("unknown-notification"));
    });
}

#[test]
fn a_rule_claims_once_and_never_reaches_back() {
    with_test_inbox(|_| {
        publish(&note("old", json!({})), 100).unwrap();
        let config = cfg(vec![rule("ra", json!({"since": 150}))]);
        assert!(
            auto_route(&config, rev, 160).unwrap().is_empty(),
            "before since"
        );
        publish(&note("new", json!({})), 200).unwrap();
        let events = auto_route(&config, rev, 201).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].source, "local");
        assert_eq!(events[0].badge, "ra");
        assert_eq!(events[0].text, "Build failed\nstep 3\nfailed");
        assert_eq!(events[0].from, "ci");
        assert_eq!(events[0].where_, "notification");
        assert!(
            auto_route(&config, rev, 202).unwrap().is_empty(),
            "claimed once"
        );
        // the pending claim is offered on every poll until it is handled
        assert_eq!(pending_events(&config, 203).unwrap(), events);
    });
}

#[test]
fn rules_match_in_settings_order_and_by_pattern() {
    with_test_inbox(|_| {
        publish(
            &note(
                "e1",
                json!({"type": "build.failed", "labels": {"env": "prod"}}),
            ),
            100,
        )
        .unwrap();
        let narrow = rule(
            "rb",
            json!({"localMatch": {"producer": "c*", "types": ["build.*"], "labels": {"env": "prod"}, "auto": true}}),
        );
        let wrong_label = rule(
            "rc",
            json!({"localMatch": {"producer": "*", "labels": {"env": "dev"}, "auto": true}}),
        );
        let broad = rule("rd", json!({"localMatch": {"producer": "*", "auto": true}}));
        let config = cfg(vec![wrong_label, narrow, broad]);
        let events = auto_route(&config, rev, 101).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].badge, "rb", "the first rule in order that fits");
        // a rule without auto never takes anything by itself
        publish(&note("e2", json!({})), 102).unwrap();
        let manual_only = cfg(vec![rule("re", json!({"localMatch": {"producer": "*"}}))]);
        assert!(auto_route(&manual_only, rev, 103).unwrap().is_empty());
    });
}

#[test]
fn a_click_and_a_rule_cannot_both_take_one_occurrence() {
    with_test_inbox(|_| {
        publish(&note("e1", json!({})), 100).unwrap();
        let n = only_notice();
        let r = rule("ra", json!({}));
        let manual = claim_manual(&n.id, 1, 0, &r, "x", 101).unwrap();
        assert_eq!(manual.badge, "ra");
        let config = cfg(vec![rule("rb", json!({}))]);
        assert!(auto_route(&config, rev, 102).unwrap().is_empty());
        assert_eq!(
            claim_manual(&n.id, 1, 0, &rule("rb", json!({})), "x", 103).unwrap_err(),
            ClaimRefusal::Taken,
            "the exclusive key is the occurrence, not the rule"
        );
        // the user saw an older revision: refused, nothing claimed
        publish(
            &note("e2", json!({"notification_key": "k", "revision": 4})),
            104,
        )
        .unwrap();
        let k = with_store(|s| Ok(s.doc.notices.iter().find(|n| n.key == "k").unwrap().clone()))
            .unwrap();
        assert_eq!(
            claim_manual(&k.id, 1, 3, &r, "x", 105).unwrap_err(),
            ClaimRefusal::Stale
        );
    });
}

#[test]
fn concurrent_claims_make_one() {
    with_test_inbox(|_| {
        publish(&note("e1", json!({})), 100).unwrap();
        let id = only_notice().id;
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let id = id.clone();
                std::thread::spawn(move || {
                    let r = rule(&format!("r{i}"), json!({}));
                    claim_manual(&id, 1, 0, &r, "x", 101).is_ok()
                })
            })
            .collect();
        let won = handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .filter(|ok| *ok)
            .count();
        assert_eq!(won, 1);
        assert_eq!(only_notice().claims.len(), 1);
    });
}

#[test]
fn handled_is_a_consumption_fact_that_survives_a_restart() {
    with_test_inbox(|_| {
        publish(&note("e1", json!({})), 100).unwrap();
        let config = cfg(vec![rule("ra", json!({}))]);
        let ev = auto_route(&config, rev, 101).unwrap().remove(0);
        // a restart between claim and card: the claim is offered again, with
        // the same key, so the webview's origin check finds a card it made
        *STORE.lock_or_recover() = None;
        assert_eq!(pending_events(&config, 102).unwrap(), vec![ev.clone()]);
        handled(&ev.key, "done", Some("S1"), None, 103).unwrap();
        // idempotent, and the first outcome stands
        handled(&ev.key, "skipped", None, Some("blocked"), 104).unwrap();
        *STORE.lock_or_recover() = None;
        let n = only_notice();
        assert_eq!(n.claims[0].state, ClaimState::Done);
        assert_eq!(n.claims[0].card.as_deref(), Some("S1"));
        assert_eq!(n.attention(), Attention::Routed);
        // nothing is offered again, and no rule takes it again
        assert!(pending_events(&config, 105).unwrap().is_empty());
        assert!(auto_route(&config, rev, 106).unwrap().is_empty());
        // an update while active refreshes the view and starts nothing
        let (_, changed) = publish(
            &note("e2", json!({"notification_key": "e1", "title": "worse"})),
            107,
        )
        .unwrap();
        assert!(!changed);
        assert!(auto_route(&config, rev, 108).unwrap().is_empty());
        // the frozen copy is the content the run was made from
        assert_eq!(only_notice().claims[0].frozen.title, "Build failed");
        assert_eq!(only_notice().title, "worse");
    });
}

#[test]
fn a_skipped_claim_gives_the_notification_back_without_another_automatic_try() {
    with_test_inbox(|_| {
        publish(&note("e1", json!({})), 100).unwrap();
        let config = cfg(vec![rule("ra", json!({}))]);
        let ev = auto_route(&config, rev, 101).unwrap().remove(0);
        handled(&ev.key, "skipped", None, Some("no-template"), 102).unwrap();
        assert_eq!(only_notice().attention(), Attention::Open);
        assert!(auto_route(&config, rev, 103).unwrap().is_empty());
        // the user may route it again; the new claim has a new key
        let again = claim_manual(&only_notice().id, 1, 0, &config.rules[0], "x", 104).unwrap();
        assert_ne!(again.key, ev.key);
    });
}

#[test]
fn a_deleted_rule_retires_its_pending_claim() {
    with_test_inbox(|_| {
        publish(&note("e1", json!({})), 100).unwrap();
        let config = cfg(vec![rule("ra", json!({}))]);
        auto_route(&config, rev, 101).unwrap();
        assert!(pending_events(&cfg(vec![]), 102).unwrap().is_empty());
        let n = only_notice();
        assert_eq!(n.claims[0].reason.as_deref(), Some("no-rule"));
        assert_eq!(n.attention(), Attention::Open);
    });
}

#[test]
fn dismissed_stays_dismissed_until_the_next_occurrence() {
    with_test_inbox(|_| {
        publish(&note("e1", json!({"notification_key": "k"})), 100).unwrap();
        let id = only_notice().id;
        assert_eq!(dismiss(&[(id.clone(), 1)], 101).unwrap(), 1);
        publish(
            &note("e2", json!({"notification_key": "k", "title": "again"})),
            102,
        )
        .unwrap();
        assert_eq!(only_notice().attention(), Attention::Dismissed);
        let v = view(true, 103);
        assert!(v.items[0].updated, "shown as updated, not counted again");
        let config = cfg(vec![rule("ra", json!({}))]);
        assert!(
            auto_route(&config, rev, 104).unwrap().is_empty(),
            "a rule never takes a dismissed one"
        );
        let resolve = json!({"version": 1, "producer": "ci", "event_id": "e3",
            "notification_key": "k", "operation": "resolve"});
        publish(&resolve, 105).unwrap();
        publish(&note("e4", json!({"notification_key": "k"})), 106).unwrap();
        assert_eq!(only_notice().attention(), Attention::Open);
        assert_eq!(auto_route(&config, rev, 107).unwrap().len(), 1);
    });
}

#[test]
fn a_full_inbox_refuses_and_open_ones_are_never_evicted() {
    with_test_inbox(|_| {
        with_store(|s| {
            s.mutate(|doc| {
                for i in 0..MAX_NOTICES {
                    let key = format!("k{i}");
                    doc.notices.push(Notice {
                        id: notice_id("ci", &key),
                        producer: "ci".into(),
                        key,
                        kind: "notification".into(),
                        title: "t".into(),
                        body: String::new(),
                        url: String::new(),
                        labels: BTreeMap::new(),
                        revision: 0,
                        occurrence: 1,
                        occurrence_at: 1,
                        reported: External::Active,
                        first_at: 1,
                        changed_at: 1,
                        occurred_at: None,
                        dismissed: None,
                        auto_tried: 0,
                        claims: Vec::new(),
                    });
                }
                Ok(())
            })
        })
        .unwrap();
        let far = 1 + RETENTION_SECS * 2;
        let err = publish(&note("new", json!({})), far).unwrap_err();
        assert_eq!(err.code, "CAPACITY");
        assert!(err.retryable());
        assert_eq!(
            with_store(|s| Ok(s.doc.notices.len())).unwrap(),
            MAX_NOTICES
        );
    });
}

#[test]
fn old_settled_notifications_and_their_events_leave_after_retention() {
    with_test_inbox(|_| {
        publish(&note("e1", json!({})), 100).unwrap();
        dismiss(&[(only_notice().id, 1)], 101).unwrap();
        let later = 101 + RETENTION_SECS + 10;
        publish(&note("e2", json!({"notification_key": "fresh"})), later).unwrap();
        let (notices, events) =
            with_store(|s| Ok((s.doc.notices.len(), s.doc.events.len()))).unwrap();
        assert_eq!((notices, events), (1, 1));
        // the dedupe window is over: the old id is a new event now
        assert_eq!(
            publish(&note("e1", json!({})), later + 1).unwrap().0.status,
            "accepted"
        );
    });
}

#[test]
fn status_reports_routing_apart_from_acceptance() {
    with_test_inbox(|_| {
        assert!(status("ci", "e1").unwrap().is_none());
        publish(&note("e1", json!({})), 100).unwrap();
        let s = status("ci", "e1").unwrap().unwrap();
        assert_eq!(s.status, "found");
        assert!(s.notification.routing.is_none());
        let config = cfg(vec![rule("ra", json!({}))]);
        let ev = auto_route(&config, rev, 101).unwrap().remove(0);
        handled(&ev.key, "done", Some("S9"), None, 102).unwrap();
        let s = status("ci", "e1").unwrap().unwrap();
        let routing = s.notification.routing.unwrap();
        assert_eq!(routing.state, ClaimState::Done);
        assert_eq!(routing.card.as_deref(), Some("S9"));
        assert_eq!(routing.by, ClaimBy::Auto);
    });
}

#[test]
fn the_wire_refuses_disabled_protocol_and_garbage() {
    with_test_inbox(|_| {
        let req = |v: Value| serde_json::to_vec(&v).unwrap();
        let (v, _) = answer(b"not json", true, 1);
        assert_eq!(v["code"], "INVALID_INPUT");
        let (v, _) = answer(&req(json!({"protocol": 9, "op": "publish"})), true, 1);
        assert_eq!(v["code"], "UNSUPPORTED_PROTOCOL");
        let (v, _) = answer(
            &req(json!({"protocol": 1, "op": "publish", "notification": note("e1", json!({}))})),
            false,
            1,
        );
        assert_eq!(v["code"], "DISABLED");
        assert_eq!(v["retryable"], true);
        let (v, routable) = answer(
            &req(json!({"protocol": 1, "op": "publish", "notification": note("e1", json!({}))})),
            true,
            1,
        );
        assert_eq!(v["status"], "accepted");
        assert!(routable);
        let (v, _) = answer(
            &req(json!({"protocol": 1, "op": "status", "producer": "ci", "event_id": "e1"})),
            true,
            2,
        );
        assert_eq!(v["status"], "found");
        let (v, _) = answer(
            &req(json!({"protocol": 1, "op": "status", "producer": "ci", "event_id": "zz"})),
            true,
            2,
        );
        assert_eq!(v["status"], "not-found");
        let (v, _) = answer(&req(json!({"protocol": 1, "op": "route"})), true, 2);
        assert_eq!(v["code"], "INVALID_INPUT");
    });
}

#[test]
fn the_socket_serves_only_its_own_user_and_complete_lines() {
    with_test_inbox(|path| {
        let socket = path.with_file_name("notify.sock");
        let listener = bind_notify_socket(&socket).unwrap();
        assert_eq!(
            std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777,
            0o600
        );
        listener.set_nonblocking(false).unwrap();
        let server = std::thread::spawn(move || {
            for _ in 0..2 {
                let (stream, _) = listener.accept().unwrap();
                handle(stream, true);
            }
        });
        let ask = |bytes: &[u8]| {
            let mut c = UnixStream::connect(&socket).unwrap();
            c.write_all(bytes).unwrap();
            let mut out = String::new();
            c.read_to_string(&mut out).unwrap();
            serde_json::from_str::<Value>(out.trim()).unwrap()
        };
        let line = serde_json::to_string(
            &json!({"protocol": 1, "op": "publish", "notification": note("w1", json!({}))}),
        )
        .unwrap();
        assert_eq!(ask(format!("{line}\n").as_bytes())["status"], "accepted");
        let big = vec![b'x'; MAX_REQUEST_BYTES + 10];
        assert_eq!(ask(&big)["code"], "TOO_LARGE");
        server.join().unwrap();
        let _ = std::fs::remove_file(&socket);
    });
}

#[test]
fn an_ack_whose_inbox_write_fails_leaves_the_item_pending_to_be_offered_again() {
    crate::inbound::with_fresh_runtime(|_| {
        with_test_inbox(|_| {
            publish(&note("e1", json!({})), 100).unwrap();
            let config = cfg(vec![rule("ra", json!({}))]);
            let ev = auto_route(&config, rev, 101).unwrap().remove(0);
            let app = tauri::test::mock_app();
            assert_eq!(
                crate::inbound::offer(app.handle(), &config, vec![ev.clone()], true),
                1
            );
            let id = crate::inbound::inbound_pending()
                .into_iter()
                .find(|p| p.event.key == ev.key)
                .unwrap()
                .id;
            // the inbox cannot be written: the ack is refused, nothing is marked
            FAIL_WRITE.with(|f| f.set(true));
            let refused = crate::inbound::inbound_ack(id, "done".into(), Some("S1".into()), None);
            FAIL_WRITE.with(|f| f.set(false));
            assert!(refused.is_err());
            assert_eq!(only_notice().claims[0].state, ClaimState::Pending);
            assert!(crate::inbound::inbound_pending().iter().any(|p| p.id == id));
            // offered again it stays one item; the next ack records it
            assert_eq!(
                crate::inbound::offer(app.handle(), &config, vec![ev.clone()], true),
                0
            );
            crate::inbound::inbound_ack(id, "done".into(), Some("S1".into()), None).unwrap();
            assert_eq!(only_notice().claims[0].state, ClaimState::Done);
            assert_eq!(only_notice().claims[0].card.as_deref(), Some("S1"));
            assert!(crate::inbound::inbound_pending().is_empty());
        })
    });
}
