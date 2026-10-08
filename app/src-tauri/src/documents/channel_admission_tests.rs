//! Disk-backed admission regressions run in their own test process so native
//! singleton inbox/Board/settings state cannot interfere with other tests.
use super::*;
use crate::inbound_channel as channel;
use crate::scheduler::{channel_first_send, QueueAddArgs};
use serde_json::{json, Value};

#[test]
fn native_channel_admission_binds_source_scope_frozen_intent_and_retirement() {
    const CHILD: &str = "DECK_CHANNEL_AUTHORITY_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "documents::channel_admission_tests::native_channel_admission_binds_source_scope_frozen_intent_and_retirement", "--nocapture"])
            .env(CHILD, "1").output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let root = crate::datadir::deck_dir();
    assert!(root
        .to_string_lossy()
        .contains(&format!("deck-test-{}", std::process::id())));
    std::fs::create_dir_all(&root).unwrap();
    exercise(&root);
    std::fs::remove_dir_all(root).unwrap();
}

fn exercise(root: &std::path::Path) {
    let dir = root.canonicalize().unwrap().to_string_lossy().into_owned();
    let mut board = json!({"projects":[{"id":"P1","name":"main","columns":[{"id":"C1","name":"Attention"}],
        "templates":[{"name":"triage","steps":["Inspect {{msg.text}}","Later"]}]}],"cards":[]});
    commit_board(&board.to_string(), BoardStanding::Current);
    let identity = channel::Identity {
        team_id: "T1".into(),
        own_user_id: "UOWN".into(),
        own_bot_id: "BOWN".into(),
        app_id: "A1".into(),
    };
    channel::set_current_identity(Some(identity.clone()));
    let settings = json!({"inbound":{"channelConnection":{"enabled":true,"connectionId":"default"},"channelRules":[{
        "id":"R1","enabled":true,"firstSend":true,"connectionId":"default","channelIds":["C1"],
        "senderUserIds":["UEXTERNAL"],"senderBotIds":[],"match":{"kind":"contains","value":"INCIDENT"},
        "includeThreads":true,"projectId":"P1","columnId":"C1","dir":dir,"cmd":"claude","template":"triage","idleMinutes":15
    }]}});
    let saved = save_settings(
        settings.to_string(),
        Some(vec![channel::ChannelFirstSendRequest {
            rule_id: "R1".into(),
            external: true,
            identity: None,
        }]),
    )
    .unwrap();
    let saved: Value = serde_json::from_str(&saved).unwrap();
    let micros = now_micros().max(
        saved["inbound"]["channelRules"][0]["firstSendGrant"]["issuedAtMicros"]
            .as_u64()
            .unwrap()
            + 1,
    );
    let now = micros / 1_000_000;
    let envelope = json!({"envelope_id":"ENV1","type":"events_api","payload":{"team_id":"T1","api_app_id":"A1","event_id":"E1","event_time":now,
        "event":{"type":"message","channel":"C1","user":"UEXTERNAL","ts":format!("{}.{:06}",now,micros%1_000_000),"text":"INCIDENT fixture"}}});
    let event = channel::stage_channel_authority_fixture(&envelope.to_string()).unwrap();
    let grant = event.first_send_grant.as_ref().unwrap();
    let op = channel_first_send::operation_id(&event.id);
    let claim = json!({"inboxId":event.id,"grantId":grant.id,"grantDigest":grant.digest,"skeleton":grant.skeleton});
    board["cards"] = json!([{"id":"S1","projectId":"P1","columnId":"C1","title":"fixture","desc":"","dir":dir,"cmd":"claude","session":"deck-test-channel",
        "origin":{"source":"channel","key":event.operation_key,"badge":"R1"},"buffer":{"revision":1,"collecting":true,"entries":[]},
        "channelRun":{"groupKey":event.group_key,"firstEventId":"E1","connectionId":"default","workspaceId":"T1","channelId":"C1","ruleId":"R1",
            "lastCollectedAt":now,"idleMinutes":15,"collecting":true,"initialQueued":false,"target":{"projectId":"P1","dir":dir,"cmd":"claude","session":"deck-test-channel"},
            "firstSend":claim,"initialSteps":[{"operationId":op,"text":"Inspect INCIDENT fixture","mode":"at","at":now,"tpl":"triage","tplIdx":1,"tplTotal":2},
            {"operationId":"Blater","text":"Later","mode":"chain","tpl":"triage","tplIdx":2,"tplTotal":2}]}}]);
    serde_json::from_value::<BoardDoc>(board.clone()).unwrap();
    reconcile_channel_board_evidence(None, &board.to_string()).unwrap();
    commit_board(&board.to_string(), BoardStanding::Current);
    let args_value = json!({"session":"deck-test-channel","cardId":"S1","operationId":op,"dir":dir,"cmd":"claude","text":"Inspect INCIDENT fixture",
        "mode":"at","at":now,"tpl":"triage","tplIdx":1,"tplTotal":2,"channelFirstSend":claim});
    let args: QueueAddArgs = serde_json::from_value(args_value.clone()).unwrap();
    assert!(
        channel_first_send::admit(&args, now)
            .unwrap()
            .unwrap()
            .authorized
    );
    assert!(
        !channel_first_send::admit(&args, now + 901)
            .unwrap()
            .unwrap()
            .authorized,
        "freshness is first-admission only"
    );
    assert!(
        !channel_first_send::admit(&args, now + 900)
            .unwrap()
            .unwrap()
            .authorized,
        "whole-second admission uses the conservative end of the interval"
    );
    assert!(
        channel_first_send::rollback_uncertain(&args, Some(event.staged_at)).unwrap(),
        "a backup predating the first operation cannot establish that the head was never sent"
    );
    assert!(
        !channel_first_send::rollback_uncertain(&args, Some(event.staged_at - 1)).unwrap(),
        "events natively staged after recovery are not part of the rollback"
    );
    for (key, value) in [
        ("text", json!("Forged")),
        ("operationId", json!("Bother")),
        ("cmd", json!("codex --no-daemon")),
        ("session", json!("deck-test-other")),
        ("mode", json!("chain")),
        ("tplIdx", json!(2)),
        ("reviewEach", json!(true)),
        ("group", json!("forged")),
    ] {
        let mut changed = args_value.clone();
        changed[key] = value;
        assert!(
            channel_first_send::admit(&serde_json::from_value(changed).unwrap(), now).is_err(),
            "forged {key}"
        );
    }
    let mut omitted = args_value.clone();
    omitted.as_object_mut().unwrap().remove("channelFirstSend");
    let omitted_args: QueueAddArgs = serde_json::from_value(omitted.clone()).unwrap();
    assert!(channel_first_send::admit(&omitted_args, now).is_err());
    assert!(channel_first_send::reject_reviewed_list(&omitted_args).is_err());
    let mut stripped = board.clone();
    stripped["cards"][0]
        .as_object_mut()
        .unwrap()
        .remove("origin");
    commit_board(&stripped.to_string(), BoardStanding::Current);
    assert!(
        channel_first_send::admit(&omitted_args, now).is_err(),
        "native source survives missing frontend origin"
    );
    omitted["session"] = json!("deck-test-other");
    assert!(
        channel_first_send::admit(&serde_json::from_value(omitted).unwrap(), now).is_err(),
        "changed session cannot choose legacy"
    );
    commit_board(&board.to_string(), BoardStanding::Current);
    let mut later = args_value.clone();
    later.as_object_mut().unwrap().remove("channelFirstSend");
    later["operationId"] = json!("Blater");
    later["text"] = json!("Later");
    later["mode"] = json!("chain");
    later["tplIdx"] = json!(2);
    later.as_object_mut().unwrap().remove("at");
    assert!(
        channel_first_send::admit(&serde_json::from_value(later).unwrap(), now)
            .unwrap()
            .is_none(),
        "later chain gets no extra authority"
    );
    let mut foreign = identity.clone();
    foreign.team_id = "T2".into();
    channel::set_current_identity(Some(foreign));
    assert!(
        !channel_first_send::admit(&args, now)
            .unwrap()
            .unwrap()
            .authorized,
        "first admission checks current workspace"
    );
    channel::set_current_identity(Some(identity));
    let recovered = commit_board(&board.to_string(), BoardStanding::Recovered).unwrap();
    assert!(
        reconcile_channel_board_evidence(Some(&recovered), &board.to_string()).is_err(),
        "a frontend request cannot erase native recovery uncertainty"
    );
    assert!(
        channel_first_send::admit(&args, now).is_err(),
        "a backup cannot authorize first admission"
    );
    commit_board(&board.to_string(), BoardStanding::Current);
    assert!(
        channel_first_send::admit(&args, now)
            .unwrap()
            .unwrap()
            .authorized
    );
    let saved_path = settings_path().with_extension("fixture");
    std::fs::rename(settings_path(), &saved_path).unwrap();
    assert!(
        channel_first_send::admit(&args, now).is_err(),
        "unknown settings preserves intent for retry"
    );
    std::fs::rename(&saved_path, settings_path()).unwrap();
    assert!(
        channel_first_send::admit(&args, now)
            .unwrap()
            .unwrap()
            .authorized
    );
    for (field, value) in [("workspace", "T2"), ("channel", "C2"), ("sender", "UOTHER")] {
        let mut forged = envelope.clone();
        forged["payload"]["event_id"] = json!("EOTHER");
        match field {
            "workspace" => forged["payload"]["team_id"] = json!(value),
            "channel" => forged["payload"]["event"]["channel"] = json!(value),
            _ => forged["payload"]["event"]["user"] = json!(value),
        }
        assert!(
            channel::stage_channel_authority_fixture(&forged.to_string()).is_err(),
            "unmatched native {field}"
        );
    }
    let mut disabled = saved.clone();
    disabled["inbound"]["channelRules"][0]["firstSend"] = json!(false);
    save_settings(disabled.to_string(), None).unwrap();
    assert!(
        !channel_first_send::admit(&args, now)
            .unwrap()
            .unwrap()
            .authorized
    );
    save_settings(
        saved.to_string(),
        Some(vec![channel::ChannelFirstSendRequest {
            rule_id: "R1".into(),
            external: true,
            identity: None,
        }]),
    )
    .unwrap();
    assert!(
        !channel_first_send::admit(&args, now)
            .unwrap()
            .unwrap()
            .authorized,
        "new grant never revives old pending"
    );
    {
        let _settings_fence = storage::settings_fence();
        let _board_fence = board_fence();
        let mut later = board.clone();
        later["projects"][0]["templates"][0]["steps"][1] = json!("Changed follow-up");
        assert!(!retire_channel_grants_locked(&later.to_string()).unwrap());
        assert!(channel::read_config_strict_result().unwrap().rules[0]
            .first_send_grant
            .is_some());
        let mut changed = board.clone();
        changed["projects"][0]["templates"][0]["steps"][0] = json!("A different head");
        assert!(retire_channel_grants_locked(&changed.to_string()).unwrap());
        assert!(!retire_channel_grants_locked(&board.to_string()).unwrap());
        assert!(
            channel::read_config_strict_result().unwrap().rules[0]
                .first_send_grant
                .is_none(),
            "changing the head back before any tick cannot revive its grant"
        );
    }
    channel::channel_ack(event.id.clone()).unwrap();
    channel::channel_ack(event.id.clone()).unwrap();
    assert!(
        channel_first_send::admit(&args, now).is_err(),
        "ACKed proof cannot create another first operation"
    );
    assert!(channel::channel_ack("default/T1/UNKNOWN/R1".into()).is_err());
    let mut disabled_fence = saved.clone();
    disabled_fence["inbound"]["channelRules"][0]["firstSend"] = json!(false);
    disabled_fence["inbound"]["channelRules"][0]
        .as_object_mut()
        .unwrap()
        .remove("firstSendGrant");
    crate::scheduler::verify_native_channel_fences(
        &saved.to_string(),
        &disabled_fence.to_string(),
        &|| {
            save_settings_locked_at(&settings_path(), &disabled_fence.to_string()).unwrap();
        },
    );
}

// ---------- independent review closure (F1, F2, F4, F5) --------------------
//
// Each test runs in a child process of its own: the inbox store, the
// committed Board and the Slack identity are process singletons, and a test
// that damages the inbox must do it before the store's one initialization.

use crate::scheduler::QueueProbe;

fn in_child(test: &str, body: impl FnOnce(&std::path::Path)) {
    const CHILD: &str = "DECK_CHANNEL_CLOSURE_TEST_CHILD";
    if std::env::var(CHILD).ok().as_deref() != Some(test) {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                &format!("documents::channel_admission_tests::{test}"),
                "--nocapture",
            ])
            .env(CHILD, test)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let root = crate::datadir::deck_dir();
    assert!(root
        .to_string_lossy()
        .contains(&format!("deck-test-{}", std::process::id())));
    std::fs::create_dir_all(&root).unwrap();
    body(&root);
    let _ = std::fs::remove_dir_all(root);
}

fn closure_identity() -> channel::Identity {
    channel::Identity {
        team_id: "T1".into(),
        own_user_id: "UOWN".into(),
        own_bot_id: "BOWN".into(),
        app_id: "A1".into(),
    }
}

fn closure_board(steps: Value) -> Value {
    json!({"projects":[{"id":"P1","name":"main","columns":[{"id":"C1","name":"Attention"}],
        "templates":[{"name":"triage","steps":steps}]}],"cards":[]})
}

fn channel_rule(dir: &str, first_send: bool) -> Value {
    let mut rule = json!({
        "id":"R1","enabled":true,"connectionId":"default","channelIds":["C1"],
        "senderUserIds":["UEXTERNAL"],"senderBotIds":[],"match":{"kind":"contains","value":"INCIDENT"},
        "includeThreads":true,"projectId":"P1","columnId":"C1","dir":dir,"cmd":"claude","template":"triage","idleMinutes":15
    });
    if first_send {
        rule["firstSend"] = json!(true);
    }
    rule
}

fn channel_settings(dir: &str, first_send: bool) -> Value {
    json!({"inbound":{"channelConnection":{"enabled":true,"connectionId":"default"},
        "channelRules":[channel_rule(dir, first_send)]}})
}

fn first_send_request() -> Option<Vec<channel::ChannelFirstSendRequest>> {
    Some(vec![channel::ChannelFirstSendRequest {
        rule_id: "R1".into(),
        external: true,
        identity: None,
    }])
}

/// One matching message through the production parser, matcher and staging.
fn stage(event_id: &str, after_micros: u64) -> (channel::PendingChannelEvent, u64) {
    let micros = now_micros().max(after_micros + 1);
    let now = micros / 1_000_000;
    let envelope = json!({"envelope_id":format!("ENV{event_id}"),"type":"events_api","payload":{
        "team_id":"T1","api_app_id":"A1","event_id":event_id,"event_time":now,
        "event":{"type":"message","channel":"C1","user":"UEXTERNAL",
            "ts":format!("{}.{:06}",now,micros%1_000_000),"text":"INCIDENT fixture"}}});
    (
        channel::stage_channel_authority_fixture(&envelope.to_string()).unwrap(),
        now,
    )
}

fn issued_at(saved: &str) -> u64 {
    serde_json::from_str::<Value>(saved).unwrap()["inbound"]["channelRules"][0]["firstSendGrant"]
        ["issuedAtMicros"]
        .as_u64()
        .unwrap()
}

/// A channel card as inbound.js freezes it, for a plan of `texts`.
fn channel_card(
    id: &str,
    session: &str,
    dir: &str,
    event: &channel::PendingChannelEvent,
    now: u64,
    texts: &[&str],
) -> Value {
    let head = channel_first_send::operation_id(&event.id);
    let steps: Vec<Value> = texts
        .iter()
        .enumerate()
        .map(|(index, text)| {
            let mut step = json!({"operationId": if index == 0 { head.clone() } else { format!("Bstep{index}") },
                "text":text,"mode": if index == 0 { "at" } else { "chain" },
                "tpl":"triage","tplIdx":index + 1,"tplTotal":texts.len()});
            if index == 0 {
                step["at"] = json!(now);
            }
            step
        })
        .collect();
    let mut card = json!({"id":id,"projectId":"P1","columnId":"C1","title":"fixture","desc":"","dir":dir,"cmd":"claude","session":session,
        "origin":{"source":"channel","key":event.operation_key,"badge":"R1"},"buffer":{"revision":1,"collecting":true,"entries":[]},
        "channelRun":{"groupKey":event.group_key,"firstEventId":event.event_id,"connectionId":"default","workspaceId":"T1","channelId":"C1","ruleId":"R1",
            "lastCollectedAt":now,"idleMinutes":15,"collecting":true,"initialQueued":false,
            "target":{"projectId":"P1","dir":dir,"cmd":"claude","session":session},"initialSteps":steps}});
    if let Some(grant) = event.first_send_grant.as_ref() {
        card["channelRun"]["firstSend"] = json!({"inboxId":event.id,"grantId":grant.id,
            "grantDigest":grant.digest,"skeleton":grant.skeleton});
    }
    card
}

/// The request `queueChannelPlan` sends for step `index` of `card`.
fn step_request(card: &Value, index: usize) -> Value {
    let run = &card["channelRun"];
    let step = &run["initialSteps"][index];
    let target = if run["target"].is_object() {
        &run["target"]
    } else {
        card
    };
    let mut request = json!({"session":target["session"],"cardId":card["id"],"operationId":step["operationId"],
        "dir":target["dir"],"cmd":target["cmd"],"text":step["text"],"mode":step["mode"],
        "tpl":step["tpl"],"tplIdx":step["tplIdx"],"tplTotal":step["tplTotal"]});
    if !step["at"].is_null() {
        request["at"] = step["at"].clone();
    }
    if index == 0 && run["firstSend"].is_object() {
        request["channelFirstSend"] = run["firstSend"].clone();
    }
    request
}

fn commit_current(board: &Value) {
    serde_json::from_value::<BoardDoc>(board.clone()).unwrap();
    commit_board(&board.to_string(), BoardStanding::Current);
}

// ---------- F5 ---------------------------------------------------------------

#[test]
fn a_follow_up_step_may_repeat_the_head_text() {
    in_child("a_follow_up_step_may_repeat_the_head_text", |root| {
        let dir = root.canonicalize().unwrap().to_string_lossy().into_owned();
        let same = "Check the current state.";
        let mut board = closure_board(json!([same, same, same]));
        commit_current(&board);
        channel::set_current_identity(Some(closure_identity()));
        let saved = save_settings(
            channel_settings(&dir, true).to_string(),
            first_send_request(),
        )
        .unwrap();
        let (event, now) = stage("E1", issued_at(&saved));
        let card = channel_card(
            "S1",
            "deck-test-same",
            &dir,
            &event,
            now,
            &[same, same, same],
        );
        board["cards"] = json!([card]);
        reconcile_channel_board_evidence(None, &board.to_string()).unwrap();
        commit_current(&board);
        let probe = QueueProbe::new();
        for index in 0..3 {
            assert!(
                probe
                    .external_add(step_request(&card, index))
                    .unwrap_or_else(|e| panic!("step {index}: {e:?}")),
                "step {index} is queued"
            );
        }
        let rows = probe.rows();
        assert_eq!(rows.len(), 3);
        assert_eq!(
            rows[0].constraint,
            Some(true),
            "the head alone is authorized"
        );
        assert_eq!(rows[0].mode, "at");
        for row in &rows[1..] {
            assert_eq!((row.constraint, row.mode.as_str()), (None, "chain"));
            assert!(!row.authority && !row.readiness_override);
        }
        assert!(rows.iter().all(|row| row.external), "every row is external");
        let operations: std::collections::HashSet<_> =
            rows.iter().map(|row| row.operation.clone()).collect();
        assert_eq!(operations.len(), 3, "each step has an operation of its own");
        for index in 0..3 {
            assert!(
                !probe.external_add(step_request(&card, index)).unwrap(),
                "replay of step {index} adds nothing"
            );
        }
        assert_eq!(probe.rows().len(), 3);

        // the checks that tell a head from a follow-up are as strict as before
        let fresh = QueueProbe::new();
        let head = step_request(&card, 0);
        let later = step_request(&card, 1);
        let mut head_as_later = head.clone();
        head_as_later
            .as_object_mut()
            .unwrap()
            .remove("channelFirstSend");
        head_as_later.as_object_mut().unwrap().remove("at");
        head_as_later["mode"] = json!("chain");
        head_as_later["tplIdx"] = json!(2);
        assert!(
            fresh.external_add(head_as_later).is_err(),
            "head as a follow-up"
        );
        let mut head_without_claim = head.clone();
        head_without_claim
            .as_object_mut()
            .unwrap()
            .remove("channelFirstSend");
        assert!(
            fresh.external_add(head_without_claim).is_err(),
            "claim omitted"
        );
        let mut later_as_head = later.clone();
        later_as_head["channelFirstSend"] = card["channelRun"]["firstSend"].clone();
        assert!(
            fresh.external_add(later_as_head).is_err(),
            "follow-up as head"
        );
        for (key, value) in [
            ("operationId", json!("Bforged")),
            ("tplIdx", json!(3)),
            ("tplIdx", json!(1)),
            ("mode", json!("at")),
            ("text", json!("Something else")),
            ("session", json!("deck-test-other")),
            ("cmd", json!("codex --no-daemon")),
        ] {
            let mut forged = later.clone();
            forged[key] = value;
            if key == "mode" {
                forged["at"] = json!(now);
            }
            assert!(
                fresh.external_add(forged).is_err(),
                "forged follow-up {key}"
            );
        }
        assert!(fresh.rows().is_empty());

        // ruled 2026-10-07: text a phone queues onto a permitted channel card
        // stays refused, before the run's own rows exist and after them
        let phone = json!({"session":card["session"],"cardId":card["id"],"operationId":"Bphone",
            "dir":card["dir"],"cmd":card["cmd"],"text":"From the phone","mode":"at","at":now});
        for queue in [&fresh, &probe] {
            let refused = queue.external_add(phone.clone()).unwrap_err();
            assert!(
                format!("{refused:?}").contains("channel first-send claim is missing"),
                "{refused:?}"
            );
        }
        assert!(fresh.rows().is_empty());
        assert_eq!(probe.rows().len(), 3);
    });
}

// ---------- F2---------------------------------------------------------------

fn other_sources_survive_inbox_failure(test: &str, damage: fn(&std::path::Path)) {
    in_child(test, |root| {
        let dir = root.canonicalize().unwrap().to_string_lossy().into_owned();
        // the damage precedes the store's one initialization in this process
        let inbox = root.join("channel-inbox.json");
        damage(&inbox);
        let card = |id: &str, extra: Value| {
            let mut card = json!({"id":id,"projectId":"P1","columnId":"C1","title":id,"desc":"",
                "dir":dir,"cmd":"claude","session":format!("deck-test-{id}")});
            for (key, value) in extra.as_object().unwrap() {
                card[key] = value.clone();
            }
            card
        };
        let run = json!({"groupKey":"default/T1/C1/R1","firstEventId":"E1","connectionId":"default","workspaceId":"T1",
            "channelId":"C1","ruleId":"R1","lastCollectedAt":10,"idleMinutes":15,"collecting":true,"initialQueued":false,
            "target":{"projectId":"P1","dir":dir,"cmd":"claude","session":"deck-test-channel"},
            "firstSend":{"inboxId":"default/T1/E1/R1","grantId":"g1","grantDigest":"a".repeat(64),"skeleton":"Inspect"},
            "initialSteps":[{"operationId":channel_first_send::operation_id("default/T1/E1/R1"),"text":"Inspect","mode":"at","at":10,
                "tpl":"triage","tplIdx":1,"tplTotal":2},
                {"operationId":"Bstep1","text":"Later","mode":"chain","tpl":"triage","tplIdx":2,"tplTotal":2}]});
        let buffer = json!({"revision":1,"collecting":true,"entries":[]});
        let mut board = closure_board(json!(["Inspect", "Later"]));
        board["cards"] = json!([
            card(
                "badge",
                json!({"origin":{"source":"slack","key":"slack:C1:1.2:eyes","badge":"eyes"}})
            ),
            card(
                "phone",
                json!({"origin":{"source":"connector","key":"h1","badge":"p1"}})
            ),
            card("manual", json!({})),
            card(
                "channel",
                json!({"origin":{"source":"channel","key":"channel:default/T1/E1/R1","badge":"R1"},
                "buffer":buffer,"channelRun":run})
            ),
            card("stripped", json!({"buffer":buffer,"channelRun":run})),
            card(
                "relabeled",
                json!({"origin":{"source":"slack","key":"slack:C1:1.3:eyes","badge":"eyes"},
                "buffer":buffer,"channelRun":run})
            ),
        ]);
        commit_current(&board);
        channel::set_current_identity(Some(closure_identity()));
        assert!(
            channel::channel_pending().is_err(),
            "the inbox really failed to initialize"
        );
        let request = |id: &str, operation: &str| {
            json!({"session":format!("deck-test-{id}"),"cardId":id,"operationId":operation,"dir":dir,
                "cmd":"claude","text":"Triage this","mode":"at","at":10})
        };
        let probe = QueueProbe::new();
        // other sources keep their own admission, whatever the inbox says
        assert!(probe.external_add(request("badge", "Bbadge1")).unwrap());
        let mut chained = request("badge", "Bbadge2");
        chained["mode"] = json!("chain");
        chained.as_object_mut().unwrap().remove("at");
        assert!(probe.external_add(chained).unwrap());
        assert!(probe.external_add(request("phone", "Bphone1")).unwrap());
        let mut verbatim = request("manual", "Bmanual1");
        verbatim["externalText"] = json!(true);
        assert!(probe.external_add(verbatim).unwrap());
        let mut reviewed = request("badge", "Bbadge3");
        reviewed["reviewEach"] = json!(true);
        probe.external_reviewed(reviewed).unwrap();
        let rows = probe.rows();
        assert_eq!(rows.len(), 4);
        for row in &rows {
            // provenance stays, and the failure grants nothing
            assert!(row.external);
            assert_eq!(
                (row.authority, row.readiness_override, row.constraint),
                (false, false, None)
            );
        }
        // a claim another source's row cannot back is still refused
        let mut shell = request("badge", "Bbadge4");
        shell["cmd"] = json!("zsh");
        assert!(probe.external_add(shell).is_err(), "agent-only admission");

        // a channel head never passes while its proof cannot be read
        let head = step_request(&board["cards"][3], 0);
        assert!(probe.external_add(head.clone()).is_err(), "claimed head");
        let mut omitted = head.clone();
        omitted.as_object_mut().unwrap().remove("channelFirstSend");
        assert!(
            probe.external_add(omitted.clone()).is_err(),
            "claim omitted"
        );
        assert!(probe.external_reviewed(omitted.clone()).is_err());
        assert!(probe.external_reviewed(head.clone()).is_err());
        for (key, value) in [
            ("operationId", json!("Bother")),
            ("mode", json!("chain")),
            ("text", json!("Other text")),
            ("cardId", json!("stripped")),
            ("cardId", json!("relabeled")),
            ("cardId", json!("unknown-card")),
        ] {
            let mut changed = omitted.clone();
            changed[key] = value.clone();
            if key == "mode" {
                changed.as_object_mut().unwrap().remove("at");
            }
            if key == "cardId" {
                changed["session"] = json!(format!("deck-test-{}", value.as_str().unwrap()));
            }
            assert!(
                probe.external_add(changed).is_err(),
                "{key}={value} cannot leave the channel path"
            );
        }
        // the follow-up of a constrained run waits with its head
        assert!(probe
            .external_add(step_request(&board["cards"][3], 1))
            .is_err());
        // without a current Board nothing names the card another source's
        commit_board(&board.to_string(), BoardStanding::Recovered);
        assert!(probe.external_add(request("badge", "Bbadge5")).is_err());
        commit_current(&board);
        assert!(probe.external_add(request("badge", "Bbadge5")).unwrap());
        assert_eq!(probe.rows().len(), 5);
    });
}

#[test]
fn a_damaged_channel_inbox_does_not_block_other_external_sources() {
    other_sources_survive_inbox_failure(
        "a_damaged_channel_inbox_does_not_block_other_external_sources",
        |inbox| std::fs::write(inbox, b"{\"version\":2,\"pending\":[{").unwrap(),
    );
}

#[test]
fn an_unreadable_channel_inbox_does_not_block_other_external_sources() {
    other_sources_survive_inbox_failure(
        "an_unreadable_channel_inbox_does_not_block_other_external_sources",
        |inbox| {
            use std::os::unix::fs::PermissionsExt;
            std::fs::write(inbox, b"{\"version\":2}").unwrap();
            std::fs::set_permissions(inbox, std::fs::Permissions::from_mode(0o000)).unwrap();
        },
    );
}

// ---------- F4 ---------------------------------------------------------------

#[test]
fn a_run_acknowledged_by_an_older_deck_finishes_its_plan() {
    in_child(
        "a_run_acknowledged_by_an_older_deck_finishes_its_plan",
        |root| {
            let dir = root.canonicalize().unwrap().to_string_lossy().into_owned();
            let now = crate::datadir::now_epoch();
            // what the previous release left: the event acknowledged BEFORE its
            // plan was queued, a version-1 inbox whose handled entries are an id
            // and a time, and a frozen run with neither target nor grant
            let old_inbox = json!({"version":1,"pending":[],"lastConnected":now - 50,
            "handled":[{"id":"default/T1/EOLD/R1","at":now - 100},{"id":"default/T1/EPART/R1","at":now - 90},
                {"id":"default/T1/ESENT/R1","at":now - 80}]});
            std::fs::write(root.join("channel-inbox.json"), old_inbox.to_string()).unwrap();
            let legacy = |id: &str, event: &str| {
                json!({"id":id,"projectId":"P1","columnId":"C1","title":id,"desc":"triage","dir":dir,"cmd":"claude",
                "session":format!("deck-test-{id}"),
                "origin":{"source":"channel","key":format!("channel:default/T1/{event}/R1"),"badge":"R1"},
                "buffer":{"revision":1,"collecting":true,"entries":[]},
                "channelRun":{"groupKey":"default/T1/C1/R1","firstEventId":event,"connectionId":"default","workspaceId":"T1",
                    "channelId":"C1","ruleId":"R1","lastCollectedAt":now - 100,"idleMinutes":0,"collecting":true,"initialQueued":false,
                    "initialSteps":[
                        {"operationId":format!("B{event}0"),"text":"Inspect INCIDENT fixture","mode":"at","at":now - 100,"tpl":"triage","tplIdx":1,"tplTotal":2},
                        {"operationId":format!("B{event}1"),"text":"Later","mode":"chain","tpl":"triage","tplIdx":2,"tplTotal":2}]}})
            };
            let mut board = closure_board(json!(["Inspect {{msg.text}}", "Later"]));
            board["cards"] = json!([
                legacy("old", "EOLD"),
                legacy("part", "EPART"),
                legacy("sent", "ESENT")
            ]);
            commit_current(&board);
            channel::set_current_identity(Some(closure_identity()));
            // the user turned the new option on after upgrading: nothing below
            // may gain from it
            let saved = save_settings(
                channel_settings(&dir, true).to_string(),
                first_send_request(),
            )
            .unwrap();

            // 1. acknowledged, nothing queued: the whole plan is recovered
            let probe = QueueProbe::new();
            let old = &board["cards"][0];
            assert!(probe.external_add(step_request(old, 0)).unwrap());
            assert!(probe.external_add(step_request(old, 1)).unwrap());
            // 2. acknowledged, head queued before the upgrade: only the rest
            let part = &board["cards"][1];
            let old_request = step_request(part, 0);
            assert!(
                old_request.get("channelFirstSend").is_none(),
                "an old request carries no new field, so its fingerprint is unchanged"
            );
            // 3. head already sent: its operation record answers the replay
            let sent = &board["cards"][2];
            let partial = QueueProbe::new();
            partial.seed_legacy(step_request(part, 0));
            assert!(!partial.external_add(step_request(part, 0)).unwrap());
            assert!(partial.external_add(step_request(part, 1)).unwrap());
            assert_eq!(partial.rows().len(), 2);
            let done = QueueProbe::new();
            let head_operation = sent["channelRun"]["initialSteps"][0]["operationId"]
                .as_str()
                .unwrap();
            done.seed_legacy(step_request(sent, 0));
            done.sent(head_operation);
            assert!(!done.external_add(step_request(sent, 0)).unwrap());
            assert!(done.external_add(step_request(sent, 1)).unwrap());
            assert_eq!(done.rows().len(), 1, "the sent head is not queued again");
            for row in probe
                .rows()
                .iter()
                .chain(partial.rows().iter())
                .chain(done.rows().iter())
            {
                assert!(row.external);
                assert_eq!(
                    (row.constraint, row.authority, row.readiness_override),
                    (None, false, false),
                    "an old run gains nothing from the new option"
                );
            }
            // an old run cannot ask for the new permission either
            let mut asking = step_request(old, 0);
            asking["operationId"] = json!("Basking");
            asking["channelFirstSend"] = json!({"inboxId":"default/T1/EOLD/R1","grantId":"g",
            "grantDigest":"a".repeat(64),"skeleton":"Inspect {{msg.text}}"});
            assert!(QueueProbe::new().external_add(asking).is_err());

            // 4. a run of the new path never becomes an old one by losing things
            let (event, staged) = stage("ENEW", issued_at(&saved));
            assert!(event.first_send_grant.is_some());
            let card = channel_card(
                "new",
                "deck-test-new",
                &dir,
                &event,
                staged,
                &["Inspect INCIDENT fixture", "Later"],
            );
            let prior = board.to_string();
            board["cards"].as_array_mut().unwrap().push(card.clone());
            reconcile_channel_board_evidence(Some(&prior), &board.to_string()).unwrap();
            commit_current(&board);
            let strict = QueueProbe::new();
            let mut omitted = step_request(&card, 0);
            omitted.as_object_mut().unwrap().remove("channelFirstSend");
            assert!(
                strict.external_add(omitted.clone()).is_err(),
                "claim omitted"
            );
            // the Board loses the frozen claim as well
            let mut bare = board.clone();
            bare["cards"][3]["channelRun"]
                .as_object_mut()
                .unwrap()
                .remove("firstSend");
            commit_current(&bare);
            assert!(
                strict.external_add(omitted.clone()).is_err(),
                "claim and Board proof"
            );
            // and the native event is consumed: its handled record keeps the grant
            commit_current(&board);
            channel::cancel_event(&event.id).unwrap();
            assert!(strict.external_add(omitted.clone()).is_err(), "consumed");
            commit_current(&bare);
            assert!(
                strict.external_add(omitted.clone()).is_err(),
                "consumed, bare Board"
            );
            let mut renamed = omitted.clone();
            renamed["operationId"] = json!("Brenamed");
            assert!(strict.external_add(renamed).is_err());
            assert!(strict.rows().is_empty());
            // a run nothing native remembers is not an old one either
            let mut unknown = bare.clone();
            unknown["cards"][0]["origin"]["key"] = json!("channel:default/T1/ENEVER/R1");
            unknown["cards"][0]["channelRun"]["firstEventId"] = json!("ENEVER");
            commit_current(&unknown);
            assert!(QueueProbe::new()
                .external_add(step_request(&unknown["cards"][0], 0))
                .is_err());
        },
    );
}

// ---------- F1 ---------------------------------------------------------------

fn settings_files(root: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(root)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("settings.json"))
        .collect();
    names.sort();
    names
}

fn forget_settings(root: &std::path::Path) {
    for name in settings_files(root) {
        std::fs::remove_file(root.join(name)).unwrap();
    }
}

fn loaded_value(loaded: &LoadedDoc) -> Value {
    serde_json::from_str(&loaded.data).unwrap()
}

#[test]
fn settings_recovery_keeps_withdrawn_authority_withdrawn_and_saves_working() {
    in_child(
        "settings_recovery_keeps_withdrawn_authority_withdrawn_and_saves_working",
        |root| {
            let dir = root.canonicalize().unwrap().to_string_lossy().into_owned();
            commit_current(&closure_board(json!(["Inspect {{msg.text}}", "Later"])));
            channel::set_current_identity(Some(closure_identity()));
            let main = settings_path();
            let badge = |accepted: bool| {
                let mut rule = json!({"id":"b1","source":"slack","badge":"eyes","projectId":"P1","columnId":"C1",
                    "cmd":"claude","template":"triage"});
                if accepted {
                    rule["firstSendWithoutReadiness"] = json!(true);
                }
                rule
            };
            let with_badge = |accepted: bool, first_send: bool| {
                let mut settings = channel_settings(&dir, first_send);
                settings["inbound"]["sources"] = json!({"slack":{"enabled":true}});
                settings["inbound"]["rules"] = json!([badge(accepted)]);
                settings
            };
            let badge_accepts = || {
                crate::inbound::read_config_strict().map(|config| {
                    config
                        .rules
                        .iter()
                        .any(|rule| rule.first_send_without_readiness)
                })
            };

            // 1. the backup holds what the last save withdrew, for a badge
            //    rule AND for the channel rule; recovering it restores neither
            let a =
                save_settings(with_badge(true, true).to_string(), first_send_request()).unwrap();
            assert!(a.contains("firstSendGrant"));
            assert_eq!(badge_accepts(), Some(true));
            save_settings(with_badge(false, false).to_string(), None).unwrap();
            assert_eq!(badge_accepts(), Some(false), "B withdrew the badge choice");
            std::fs::write(&main, b"{ damaged").unwrap();
            let loaded = load_settings().unwrap();
            assert_eq!(loaded.source, "backup");
            let shown = loaded_value(&loaded);
            assert!(shown["inbound"]["channelRules"][0]
                .get("firstSend")
                .is_none());
            assert!(shown["inbound"]["channelRules"][0]
                .get("firstSendGrant")
                .is_none());
            assert_eq!(
                storage::read_typed::<SettingsDoc>(&main)
                    .unwrap()
                    .map(|outcome| outcome.source),
                Some("backup"),
                "loading never makes the backup the current settings"
            );
            assert_eq!(
                badge_accepts(),
                None,
                "the withdrawn badge choice is not current authority again"
            );
            assert!(channel::read_config_strict_result().is_err());
            // a template-head edit is still saved while only the backup
            // answers: no grant is current, so there is none to retire
            {
                let _settings_fence = storage::settings_fence();
                let _board_fence = board_fence();
                let changed = closure_board(json!(["A different head", "Later"]));
                assert!(!retire_channel_grants_locked(&changed.to_string()).unwrap());
            }
            // a restart loads the same backup, again without channel choices
            let again = loaded_value(&load_settings().unwrap());
            assert!(again["inbound"]["channelRules"][0]
                .get("firstSendGrant")
                .is_none());
            assert_eq!(badge_accepts(), None);

            // 3. an unrelated save of what was loaded, then a restart
            let mut edited = shown.clone();
            edited["locale"] = json!("en");
            let saved = save_settings(edited.to_string(), None).unwrap();
            assert!(!saved.contains("firstSendGrant") && !saved.contains("\"firstSend\":true"));
            let restarted = load_settings().unwrap();
            assert_eq!(restarted.source, "main");
            let current = channel::read_config_strict_result().unwrap();
            assert!(!current.rules[0].first_send && current.rules[0].first_send_grant.is_none());
            // a stale page that still holds the old grant cannot bring it back
            forget_settings(root);
            save_settings(with_badge(true, true).to_string(), first_send_request()).unwrap();
            let stale = std::fs::read_to_string(&main).unwrap();
            let stale: Value = serde_json::from_str::<Value>(&stale).unwrap();
            let stale = stale.get("data").cloned().unwrap_or(stale);
            assert!(stale["inbound"]["channelRules"][0]["firstSendGrant"].is_object());
            save_settings(with_badge(false, false).to_string(), None).unwrap();
            std::fs::write(&main, b"{ damaged").unwrap();
            assert_eq!(load_settings().unwrap().source, "backup");
            let replayed = save_settings(stale.to_string(), None).unwrap();
            assert!(
                !replayed.contains("firstSendGrant") && !replayed.contains("\"firstSend\":true")
            );
            let current = channel::read_config_strict_result().unwrap();
            assert!(!current.rules[0].first_send && current.rules[0].first_send_grant.is_none());
            // recovered once more, the older copies still restore nothing
            std::fs::write(&main, b"{ damaged").unwrap();
            let once_more = load_settings().unwrap();
            let once_more = loaded_value(&once_more);
            for rule in once_more["inbound"]["channelRules"].as_array().unwrap() {
                assert!(rule.get("firstSend").is_none() && rule.get("firstSendGrant").is_none());
            }
            // a new explicit choice after recovery is honoured in one save
            let granted =
                save_settings(with_badge(false, true).to_string(), first_send_request()).unwrap();
            assert!(granted.contains("firstSendGrant"));
            assert!(channel::read_config_strict_result().unwrap().rules[0]
                .first_send_grant
                .is_some());

            // 2. settings that never used the channel option: recovered, then saved
            forget_settings(root);
            save_settings(json!({"locale":"en"}).to_string(), None).unwrap();
            save_settings(json!({"locale":"system"}).to_string(), None).unwrap();
            std::fs::write(&main, b"{ damaged").unwrap();
            let loaded = load_settings().unwrap();
            assert_eq!(loaded.source, "backup");
            assert_eq!(loaded_value(&loaded)["locale"], "en");
            assert!(!main.exists(), "nothing was written for the owner");
            save_settings(json!({"locale":"zh-Hans"}).to_string(), None)
                .expect("recovered settings can be saved");
            let reloaded = load_settings().unwrap();
            assert_eq!(reloaded.source, "main");
            assert_eq!(loaded_value(&reloaded)["locale"], "zh-Hans");
            // damaged while deck runs, and with no usable backup at all
            std::fs::write(&main, b"{ damaged").unwrap();
            save_settings(json!({"locale":"en"}).to_string(), None)
                .expect("a file damaged while running never refuses the owner");
            forget_settings(root);
            save_settings(json!({"locale":"en"}).to_string(), None).unwrap();
            std::fs::write(&main, b"{ damaged").unwrap();
            save_settings(json!({"locale":"system"}).to_string(), None)
                .expect("no backup: the damaged file is set aside and the save goes ahead");
            assert_eq!(loaded_value(&load_settings().unwrap())["locale"], "system");

            // 4. a file that cannot be READ is unknown, not damaged: a save
            //    writes nothing over it, and the choice stands once it reads
            use std::os::unix::fs::PermissionsExt;
            forget_settings(root);
            let granted =
                save_settings(with_badge(true, true).to_string(), first_send_request()).unwrap();
            // an ordinary later save carries the exact current grant along
            let mut later: Value = serde_json::from_str(&granted).unwrap();
            later["locale"] = json!("en");
            assert!(save_settings(later.to_string(), None)
                .unwrap()
                .contains("firstSendGrant"));
            let bytes = std::fs::read(&main).unwrap();
            std::fs::set_permissions(&main, std::fs::Permissions::from_mode(0o000)).unwrap();
            assert!(save_settings(json!({}).to_string(), None).is_err());
            assert!(save_settings(with_badge(false, false).to_string(), None).is_err());
            assert!(
                channel::read_config_strict_result().is_err(),
                "unknown to the readers: a queued head waits, it is not withdrawn"
            );
            std::fs::set_permissions(&main, std::fs::Permissions::from_mode(0o600)).unwrap();
            assert_eq!(std::fs::read(&main).unwrap(), bytes, "left untouched");
            assert_eq!(load_settings().unwrap().source, "main");
            assert!(channel::read_config_strict_result().unwrap().rules[0]
                .first_send_grant
                .is_some());
            assert_eq!(badge_accepts(), Some(true));
            // the owner's LOAD of an unreadable file keeps its own contract
            // (set aside, answered from the backup, never from defaults) and
            // the backup still comes without the channel choice
            std::fs::set_permissions(&main, std::fs::Permissions::from_mode(0o000)).unwrap();
            let loaded = load_settings().unwrap();
            assert_eq!(loaded.source, "backup");
            let shown = loaded_value(&loaded);
            assert_eq!(shown["inbound"]["rules"][0]["id"], "b1", "not defaults");
            assert!(shown["inbound"]["channelRules"][0]
                .get("firstSendGrant")
                .is_none());
            assert!(channel::read_config_strict_result().is_err());
            assert_eq!(badge_accepts(), None);
        },
    );
}

// ---------- F3 ---------------------------------------------------------------

/// A loopback stand-in for Slack: the Web API methods the transport calls and
/// one Socket Mode endpoint. Nothing leaves this machine, and the only
/// credentials are the fixture strings the test puts in the in-memory test
/// Keychain cache.
struct SlackFixture {
    state: std::sync::Arc<std::sync::Mutex<FixtureState>>,
}

struct FixtureState {
    /// the workspace `auth.test` answers for; `None` answers HTTP 500
    team: Option<&'static str>,
    /// whether `hello` names the app
    hello_app: bool,
    opens: usize,
    auth_tests: usize,
    sockets: usize,
    outbox: Vec<String>,
    acks: Vec<String>,
}

impl SlackFixture {
    fn start() -> Self {
        use std::io::{Read, Write};
        use std::sync::{Arc, Mutex};
        use std::time::Duration;
        let state = Arc::new(Mutex::new(FixtureState {
            team: Some("T1"),
            hello_app: true,
            opens: 0,
            auth_tests: 0,
            sockets: 0,
            outbox: Vec::new(),
            acks: Vec::new(),
        }));
        let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let socket_url = format!("ws://{}", socket.local_addr().unwrap());
        let shared = state.clone();
        std::thread::spawn(move || {
            for stream in socket.incoming().flatten() {
                let shared = shared.clone();
                std::thread::spawn(move || {
                    let Ok(mut ws) = tungstenite::accept(stream) else {
                        return;
                    };
                    let _ = ws
                        .get_mut()
                        .set_read_timeout(Some(Duration::from_millis(50)));
                    let hello = {
                        let mut state = shared.lock_or_recover();
                        state.sockets += 1;
                        if state.hello_app {
                            json!({"type":"hello","connection_info":{"app_id":"A1"}})
                        } else {
                            json!({"type":"hello"})
                        }
                    };
                    if ws
                        .send(tungstenite::Message::Text(hello.to_string().into()))
                        .is_err()
                    {
                        return;
                    }
                    loop {
                        let next = shared.lock_or_recover().outbox.pop();
                        if let Some(text) = next {
                            if ws.send(tungstenite::Message::Text(text.into())).is_err() {
                                return;
                            }
                        }
                        match ws.read() {
                            Ok(tungstenite::Message::Text(text)) => {
                                shared.lock_or_recover().acks.push(text.to_string())
                            }
                            Ok(tungstenite::Message::Close(_)) => return,
                            Ok(_) => {}
                            Err(tungstenite::Error::Io(error))
                                if matches!(
                                    error.kind(),
                                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                                ) => {}
                            Err(_) => return,
                        }
                    }
                });
            }
        });
        let api = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", api.local_addr().unwrap());
        let shared = state.clone();
        std::thread::spawn(move || {
            for mut stream in api.incoming().flatten() {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let mut raw = Vec::new();
                let mut chunk = [0u8; 4096];
                while let Ok(n) = stream.read(&mut chunk) {
                    if n == 0 {
                        break;
                    }
                    raw.extend_from_slice(&chunk[..n]);
                    let Some(end) = raw.windows(4).position(|w| w == b"\r\n\r\n") else {
                        continue;
                    };
                    let head = String::from_utf8_lossy(&raw[..end]).to_ascii_lowercase();
                    let length = head
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .and_then(|value| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if raw.len() >= end + 4 + length {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&raw).into_owned();
                let (status, body) = {
                    let mut state = shared.lock_or_recover();
                    match state.team {
                        None => (500, json!({"ok":false})),
                        Some(team) if request.starts_with("POST /auth.test") => {
                            state.auth_tests += 1;
                            (
                                200,
                                json!({"ok":true,"team_id":team,"user_id":"UOWN","bot_id":"BOWN"}),
                            )
                        }
                        Some(_) if request.starts_with("POST /apps.connections.open") => {
                            state.opens += 1;
                            (200, json!({"ok":true,"url":socket_url}))
                        }
                        Some(_) => (404, json!({"ok":false})),
                    }
                };
                let body = body.to_string();
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nX-OAuth-Scopes: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    crate::slack_api::BOT_SCOPES.join(","),
                    body.len()
                );
            }
        });
        *crate::slack_api::TEST_API.lock_or_recover() = Some(base);
        crate::slack_transport::LOOPBACK_SOCKET.store(true, std::sync::atomic::Ordering::SeqCst);
        Self { state }
    }

    fn set(&self, change: impl FnOnce(&mut FixtureState)) {
        change(&mut self.state.lock_or_recover());
    }

    fn read<T>(&self, read: impl FnOnce(&FixtureState) -> T) -> T {
        read(&self.state.lock_or_recover())
    }
}

fn eventually(what: &str, seconds: u64, mut done: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    while !done() {
        assert!(std::time::Instant::now() < deadline, "timed out: {what}");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

#[test]
fn the_first_first_send_rule_is_saved_by_one_save_from_zero_rules() {
    use crate::keychain::{self, Slot};
    use crate::slack_transport as transport;
    use std::time::Duration;
    in_child(
        "the_first_first_send_rule_is_saved_by_one_save_from_zero_rules",
        |root| {
            let dir = root.canonicalize().unwrap().to_string_lossy().into_owned();
            let slack = SlackFixture::start();
            commit_current(&closure_board(json!(["Inspect {{msg.text}}", "Later"])));
            // credentials configured, the channel connection switched on,
            // and not one channel rule yet
            keychain::set(Slot::SlackBotToken, "xoxb-fixture").unwrap();
            keychain::set(Slot::SlackAppToken, "xapp-fixture").unwrap();
            let no_rules = json!({"inbound":{"channelConnection":{"enabled":true,"connectionId":"default"},"channelRules":[]}});
            save_settings(no_rules.to_string(), None).unwrap();
            std::thread::spawn(|| {
                transport::socket_loop(transport::Consumers {
                    channel: Box::new(|identity, text| {
                        channel::stage_message(identity, text).map(|_| ())
                    }),
                    reaction: Box::new(|_, _, _| {}),
                })
            });
            let with_rule = channel_settings(&dir, true).to_string();
            let request = |identity: Option<String>| {
                Some(vec![channel::ChannelFirstSendRequest {
                    rule_id: "R1".into(),
                    external: true,
                    identity,
                }])
            };
            let rules_saved = || channel::read_config_strict_result().unwrap().rules.len();
            let unplanned = |slack: &SlackFixture| {
                eventually("the connection is dropped", 12, || {
                    channel::current_identity().is_none()
                });
                let opens = slack.read(|s| s.opens);
                std::thread::sleep(Duration::from_millis(600));
                assert_eq!(slack.read(|s| s.opens), opens, "nothing asks for a ticket");
                assert!(channel::current_identity().is_none());
            };

            // with no rule the transport verifies nothing, so the save that
            // would create the first rule has no identity to bind
            std::thread::sleep(Duration::from_millis(400));
            assert!(channel::current_identity().is_none());
            assert_eq!(slack.read(|s| (s.opens, s.sockets)), (0, 0));
            assert!(
                save_settings(with_rule.clone(), request(None)).is_err(),
                "the grant is never issued without a verified identity"
            );
            assert_eq!(rules_saved(), 0);

            // 1. ONE save: prepare the identity through the real handshake,
            //    then save the rule with its grant
            let ready = transport::prepare_channel_identity(Duration::from_secs(10))
                .expect("the transport verifies the identity for the waiting save");
            assert_eq!(
                ready.identity,
                channel::identity_digest(&closure_identity()),
                "auth.test and the socket's hello produced it, nothing was injected"
            );
            assert_eq!(slack.read(|s| (s.opens, s.sockets)), (1, 1));
            let saved = save_settings(with_rule.clone(), request(Some(ready.identity.clone())))
                .expect("the same save creates the rule and its grant");
            assert!(saved.contains("firstSendGrant"));
            assert_eq!(rules_saved(), 1);
            // the save is over: its hold is released, and what every inbound
            // save does next (`inbound_check_now`) re-plans for the saved rule
            transport::slack_channel_prepare_cancel();
            transport::wake();
            eventually("reconnected for the saved rule", 10, || {
                channel::current_identity().is_some() && slack.read(|s| s.sockets) >= 2
            });
            // a matching message now freezes that grant at native staging
            let micros = now_micros().max(issued_at(&saved) + 1);
            let now = micros / 1_000_000;
            slack.set(|s| {
                s.outbox.push(
                    json!({"envelope_id":"ENV1","type":"events_api","payload":{
                "team_id":"T1","api_app_id":"A1","event_id":"E1","event_time":now,
                "event":{"type":"message","channel":"C1","user":"UEXTERNAL",
                    "ts":format!("{}.{:06}",now,micros%1_000_000),"text":"INCIDENT fixture"}}})
                    .to_string(),
                )
            });
            eventually("staged, then acknowledged", 10, || {
                slack.read(|s| s.acks.iter().any(|ack| ack.contains("ENV1")))
            });
            let pending = channel::channel_pending().unwrap();
            assert_eq!(pending.len(), 1);
            assert!(pending[0].first_send_grant.is_some());

            // 5. with the connection up, a later save asks and waits for nothing
            let (opens, sockets) = slack.read(|s| (s.opens, s.sockets));
            let started = std::time::Instant::now();
            let again = transport::prepare_channel_identity(Duration::from_secs(10)).unwrap();
            assert!(started.elapsed() < Duration::from_millis(200));
            assert_eq!(again.identity, ready.identity);
            let mut edited: Value = serde_json::from_str(&saved).unwrap();
            edited["inbound"]["channelRules"][0]["channelIds"] = json!(["C1", "C2"]);
            assert!(
                save_settings(edited.to_string(), request(Some(again.identity)))
                    .unwrap()
                    .contains("firstSendGrant")
            );
            transport::slack_channel_prepare_cancel();
            std::thread::sleep(Duration::from_millis(300));
            assert!(
                channel::current_identity().is_some(),
                "the connection was not restarted"
            );
            assert_eq!(slack.read(|s| (s.opens, s.sockets)), (opens, sockets));

            // back to zero rules for the failure cases
            let reset = |slack: &SlackFixture| {
                save_settings(no_rules.to_string(), None).unwrap();
                transport::wake();
                unplanned(slack);
            };
            reset(&slack);

            // 2. a short outage: the same request is asked again and the
            //    save completes, with nothing accepted a second time
            slack.set(|s| s.team = None);
            assert_eq!(
                transport::prepare_channel_identity(Duration::from_millis(1500)).err(),
                Some("pending")
            );
            assert_eq!(rules_saved(), 0, "nothing was saved or granted meanwhile");
            slack.set(|s| s.team = Some("T1"));
            let ready = transport::prepare_channel_identity(Duration::from_secs(10))
                .expect("the retry completes once Slack answers");
            assert!(
                save_settings(with_rule.clone(), request(Some(ready.identity.clone())))
                    .unwrap()
                    .contains("firstSendGrant")
            );
            transport::slack_channel_prepare_cancel();
            reset(&slack);

            // 4a. another workspace became the verified one after the choice
            slack.set(|s| s.team = Some("T2"));
            let other = transport::prepare_channel_identity(Duration::from_secs(10)).unwrap();
            assert_ne!(other.identity, ready.identity);
            let before = std::fs::read(settings_path()).unwrap();
            assert!(
                save_settings(with_rule.clone(), request(Some(ready.identity.clone()))).is_err(),
                "an identity that is not the one the save was made against"
            );
            assert_eq!(std::fs::read(settings_path()).unwrap(), before);
            assert_eq!(rules_saved(), 0);
            transport::slack_channel_prepare_cancel();
            unplanned(&slack);
            slack.set(|s| s.team = Some("T1"));

            // 4b. a socket that does not name its app verifies nothing
            slack.set(|s| s.hello_app = false);
            assert_eq!(
                transport::prepare_channel_identity(Duration::from_millis(1500)).err(),
                Some("pending")
            );
            assert!(channel::current_identity().is_none());
            assert!(save_settings(with_rule.clone(), request(None)).is_err());
            transport::slack_channel_prepare_cancel();
            slack.set(|s| s.hello_app = true);
            unplanned(&slack);

            // 4c. the user gives the save up while it waits
            slack.set(|s| s.team = None);
            let waiting = std::thread::spawn(|| {
                transport::prepare_channel_identity(Duration::from_secs(10)).err()
            });
            std::thread::sleep(Duration::from_millis(400));
            transport::slack_channel_prepare_cancel();
            assert_eq!(waiting.join().unwrap(), Some("canceled"));
            slack.set(|s| s.team = Some("T1"));
            unplanned(&slack);
            assert_eq!(rules_saved(), 0, "a canceled save grants nothing later");

            // 4d. the connection is switched off while the save waits
            slack.set(|s| s.team = None);
            let waiting = std::thread::spawn(|| {
                transport::prepare_channel_identity(Duration::from_millis(2500)).err()
            });
            std::thread::sleep(Duration::from_millis(300));
            let off = json!({"inbound":{"channelConnection":{"enabled":false,"connectionId":"default"},"channelRules":[]}});
            save_settings(off.to_string(), None).unwrap();
            transport::wake();
            slack.set(|s| s.team = Some("T1"));
            assert_eq!(waiting.join().unwrap(), Some("pending"));
            assert!(channel::current_identity().is_none());
            assert_eq!(
                transport::prepare_channel_identity(Duration::from_secs(1)).err(),
                Some("disabled")
            );
            // and without credentials nothing is attempted either
            save_settings(no_rules.to_string(), None).unwrap();
            keychain::clear(Slot::SlackAppToken).unwrap();
            let opens = slack.read(|s| s.opens);
            assert_eq!(
                transport::prepare_channel_identity(Duration::from_secs(1)).err(),
                Some("no-token")
            );
            assert_eq!(slack.read(|s| s.opens), opens);
        },
    );
}
