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
