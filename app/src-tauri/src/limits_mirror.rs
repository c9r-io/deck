//! Backend ↔ frontend mirrored constants: the Rust half of the mirror whose
//! JS half is `ui/test/limits.test.mjs`. Both compare their own constants
//! with the one list in `ui/test/fixtures/limits.json`; neither parses the
//! other's source. The backend stays authoritative (it refuses on save).
//! Scheduler item state words are deliberately absent (FR-3 turns them into
//! an enum); MCP local command errors are checked in `mcp/tests.rs`, where
//! their constants are visible.
//!
//! A length bound's key says its unit (`_bytes`, `_chars`). `unit_gaps`
//! lists the bounds that carry the same number in two units: the editors
//! and the settings validation count characters, the Board (`documents.rs`)
//! counts bytes, so a name of 41 to 120 CJK characters passes the former
//! and is refused by the latter. The gap is known and left open (relaxing
//! the Board needs a sticky schema version first); each half of the mirror
//! holds its own side to the list, so changing either unit breaks a test
//! and has to be a decision.

use crate::context::ContextStatus;
use serde_json::Value;

fn limits() -> Value {
    serde_json::from_str(include_str!("../../ui/test/fixtures/limits.json")).unwrap()
}

#[test]
fn translation_input_bound_matches_frontend() {
    assert_eq!(
        number(&limits()["translation_max_bytes"]),
        crate::intelligence::pack::SELECTION_BYTES as u64
    );
    assert_eq!(
        number(&limits()["translation_live_bytes"]),
        crate::intelligence::pack::LIVE_BYTES as u64
    );
    assert_eq!(
        number(&limits()["translation_document_choices"][0]),
        crate::intelligence::pack::DOCUMENT_CHOICES[0] as u64
    );
    assert_eq!(
        number(&limits()["translation_document_choices"][1]),
        crate::intelligence::pack::DOCUMENT_CHOICES[1] as u64
    );
}

fn words(value: &Value) -> Vec<&str> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|word| word.as_str().unwrap())
        .collect()
}

fn number(value: &Value) -> u64 {
    value.as_u64().unwrap()
}

#[test]
fn scheduler_limits_match_the_fixture() {
    let s = &limits()["scheduler"];
    assert_eq!(
        number(&s["chain_quiet_secs_default"]),
        crate::scheduler::CHAIN_QUIET_SECS
    );
    assert_eq!(
        number(&s["quiet_secs_min"]),
        crate::scheduler::MIN_QUIET_SECS
    );
    assert_eq!(
        number(&s["quiet_secs_max"]),
        crate::scheduler::MAX_QUIET_SECS
    );
    assert_eq!(
        number(&s["item_max_attempts"]),
        crate::scheduler::MAX_ATTEMPTS as u64
    );
}

#[test]
fn inbound_rule_limits_match_the_fixture() {
    use crate::inbound::*;
    let i = &limits()["inbound"];
    assert_eq!(words(&i["sources"]), SOURCES);
    assert_eq!(words(&i["schedule_units"]), SCHEDULE_UNITS);
    assert_eq!(words(&i["finish_modes"]), FINISH_MODES);
    assert_eq!(number(&i["grace_min_default"]), DEFAULT_GRACE_MIN as u64);
    assert_eq!(number(&i["grace_min_max"]), MAX_GRACE_MIN as u64);
    assert_eq!(number(&i["rules_max"]), MAX_RULES as u64);
    assert_eq!(number(&i["rule_id_max"]), RULE_ID_MAX as u64);
    assert_eq!(
        number(&i["rule_name_max_chars"]),
        RULE_NAME_MAX_CHARS as u64
    );
    assert_eq!(number(&i["rule_cmd_max_chars"]), RULE_CMD_MAX_CHARS as u64);
    assert_eq!(
        number(&i["auto_send_max_steps"]),
        AUTO_SEND_MAX_STEPS as u64
    );
    assert_eq!(words(&i["auto_send_classes"]), AUTO_SEND_CLASSES);
    assert_eq!(
        number(&i["template_name_max_chars"]),
        TEMPLATE_NAME_MAX_CHARS as u64
    );
}

/// The frontend holds regexes, the backend byte predicates: the same
/// vectors must classify the same way on both sides.
#[test]
fn identifier_and_badge_rules_classify_the_fixture_vectors() {
    use crate::inbound::{bounded_id, valid_badge, REF_ID_MAX};
    let l = limits();
    assert_eq!(number(&l["local_id"]["max"]), REF_ID_MAX as u64);
    for value in words(&l["local_id"]["valid"]) {
        assert!(bounded_id(value, REF_ID_MAX), "id {value:?}");
    }
    for value in words(&l["local_id"]["invalid"]) {
        assert!(!bounded_id(value, REF_ID_MAX), "id {value:?}");
    }
    for value in words(&l["badge"]["valid"]) {
        assert!(valid_badge(value), "badge {value:?}");
    }
    for value in words(&l["badge"]["invalid"]) {
        assert!(!valid_badge(value), "badge {value:?}");
    }
}

#[test]
fn channel_buffer_preset_and_drop_limits_match_the_fixture() {
    use crate::documents::*;
    use crate::inbound_channel::{
        MAX_CHANNELS, MAX_IDLE_MINUTES, MAX_KEYWORDS, MAX_KEYWORD_CHARS, MAX_RULES,
    };
    let l = limits();
    let c = &l["channel"];
    assert_eq!(number(&c["rules_max"]), MAX_RULES as u64);
    assert_eq!(number(&c["channel_ids_max"]), MAX_CHANNELS as u64);
    assert_eq!(number(&c["keywords_max"]), MAX_KEYWORDS as u64);
    assert_eq!(number(&c["keyword_max_chars"]), MAX_KEYWORD_CHARS as u64);
    assert_eq!(number(&c["idle_minutes_max"]), MAX_IDLE_MINUTES as u64);
    let b = &l["buffer"];
    assert_eq!(number(&b["max_entries"]), BUFFER_MAX_ENTRIES as u64);
    assert_eq!(number(&b["max_copies"]), BUFFER_MAX_COPIES as u64);
    assert_eq!(number(&b["max_entry_bytes"]), BUFFER_MAX_ENTRY_BYTES as u64);
    assert_eq!(number(&b["max_bytes"]), BUFFER_MAX_BYTES as u64);
    assert_eq!(
        number(&b["max_serialized_bytes"]),
        BUFFER_MAX_SERIALIZED_BYTES as u64
    );
    assert_eq!(number(&l["presets_max"]), PRESETS_MAX as u64);
    assert_eq!(
        number(&l["drop_max_bytes"]),
        crate::drops::MAX_DROP_BYTES as u64
    );
}

#[test]
fn settings_limits_match_the_fixture() {
    use crate::documents::*;
    let s = &limits()["settings"];
    assert_eq!(s["font_scale_min"].as_f64(), Some(FONT_SCALE_MIN));
    assert_eq!(s["font_scale_max"].as_f64(), Some(FONT_SCALE_MAX));
    assert_eq!(words(&s["themes"]), THEMES);
    assert_eq!(words(&s["accents"]), ACCENTS);
    assert_eq!(words(&s["locales"]), LOCALES);
    assert_eq!(number(&s["shortcut_max_len"]), SHORTCUT_MAX_LEN as u64);
    for channel in words(&s["update_channels"]) {
        assert!(crate::updater::update_source(channel).is_ok(), "{channel}");
    }
    assert!(crate::updater::update_source("beta").is_err());
}

#[test]
fn closed_status_vocabularies_match_the_fixture() {
    let l = limits();
    assert_eq!(words(&l["agent_states"]), crate::agent_status::STATES);
    use crate::agent_status::CodexSignalTrust::{Trusted, Unavailable, Unknown};
    assert_eq!(
        serde_json::to_value([Unknown, Trusted, Unavailable]).unwrap(),
        l["codex_signal_trust"]
    );
    let notices: Vec<&str> = crate::storage::StorageNotice::ALL
        .iter()
        .map(|notice| notice.code())
        .collect();
    assert_eq!(words(&l["storage_notices"]), notices);
    assert_eq!(
        words(&l["notify_status_words"]),
        crate::notify::STATUS_WORDS
    );
    // Every ContextStatus variant, spelled as it goes on the wire. Adding a
    // variant breaks this exhaustive match until it is listed here and in
    // the fixture.
    let spell = |status: ContextStatus| {
        match status {
            ContextStatus::Ready
            | ContextStatus::ForegroundDifferent
            | ContextStatus::SessionReplaced
            | ContextStatus::Unavailable
            | ContextStatus::Starting
            | ContextStatus::Unknown => {}
        }
        serde_json::to_value(status)
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned()
    };
    let all = [
        ContextStatus::Ready,
        ContextStatus::ForegroundDifferent,
        ContextStatus::SessionReplaced,
        ContextStatus::Unavailable,
        ContextStatus::Starting,
        ContextStatus::Unknown,
    ];
    let spelled: Vec<String> = all.into_iter().map(spell).collect();
    assert_eq!(spelled, words(&l["context_statuses"]));
}

#[test]
fn reminder_note_bound_matches_frontend() {
    assert_eq!(
        number(&limits()["reminder_note_bytes"]),
        crate::reminder::NOTE_BYTES as u64
    );
}

/// A Board with one project and, optionally, one preset and one card field.
fn board_accepts(preset: Option<Value>, card_field: Option<(&str, Value)>) -> bool {
    let mut board = serde_json::json!({
        "projects": [{"id": "P1", "name": "p", "columns": [{"id": "C1", "name": "c"}]}],
        "cards": []
    });
    if let Some(preset) = preset {
        board["projects"][0]["presets"] = serde_json::json!([preset]);
    }
    if let Some((field, value)) = card_field {
        let mut card = serde_json::json!({
            "id": "s1", "projectId": "P1", "columnId": "C1", "title": "t", "desc": "",
            "cmd": "claude", "dir": "~/w", "session": "deck-t-ab12"
        });
        if field == "channelRun" {
            card["buffer"] = serde_json::json!({"revision": 1, "collecting": true, "entries": []});
        }
        card[field] = value;
        board["cards"] = serde_json::json!([card]);
    }
    serde_json::from_value::<crate::documents::BoardDoc>(board).is_ok()
}

fn preset_with(field: &str, value: Value) -> Option<Value> {
    let mut preset = serde_json::json!({
        "id": "R1", "name": "n", "columnId": "C1", "title": "t", "dir": "~/w", "cmd": "codex",
        "steps": ["s"]
    });
    preset[field] = value;
    Some(preset)
}

fn frozen_steps(count: usize, tpl: &str) -> Value {
    (0..count)
        .map(|index| {
            serde_json::json!({
                "operationId": format!("B{index}"), "text": "frozen",
                "mode": if index == 0 { "at" } else { "chain" },
                "at": if index == 0 { serde_json::json!(10) } else { Value::Null },
                "tpl": tpl, "tplIdx": index + 1, "tplTotal": count
            })
        })
        .collect()
}

fn inbound_plan(count: usize, tpl: &str) -> Option<(&'static str, Value)> {
    Some((
        "inboundPlan",
        serde_json::json!({
            "operationId": "B", "reviewEach": false, "initialQueued": false,
            "initialSteps": frozen_steps(count, tpl)
        }),
    ))
}

fn channel_run(tpl: &str) -> Option<(&'static str, Value)> {
    Some((
        "channelRun",
        serde_json::json!({
            "groupKey": "default/T1/C1/R1", "firstEventId": "Ev1", "connectionId": "default",
            "workspaceId": "T1", "channelId": "C1", "ruleId": "R1", "lastCollectedAt": 10,
            "idleMinutes": 30, "collecting": true, "initialQueued": true,
            "initialSteps": frozen_steps(1, tpl)
        }),
    ))
}

/// The Board's own bounds (`documents.rs`): each number, and that a length is
/// counted in BYTES — a value at the bound passes and one byte more is
/// refused, however few characters it has.
#[test]
fn board_bounds_match_the_fixture_and_count_bytes() {
    use crate::documents::*;
    let b = &limits()["board"];
    let fixture = [
        ("plan_steps_max", PLAN_STEPS_MAX),
        ("plan_template_name_max_bytes", PLAN_TEMPLATE_NAME_MAX_BYTES),
        ("preset_name_max_bytes", PRESET_NAME_MAX_BYTES),
        ("preset_title_max_bytes", PRESET_TITLE_MAX_BYTES),
        ("preset_dir_max_bytes", PRESET_DIR_MAX_BYTES),
        ("preset_cmd_max_bytes", PRESET_CMD_MAX_BYTES),
        ("preset_steps_max", PRESET_STEPS_MAX),
        ("preset_step_max_bytes", PRESET_STEP_MAX_BYTES),
    ];
    assert_eq!(
        b.as_object().unwrap().len(),
        fixture.len(),
        "every key is held"
    );
    for (key, constant) in fixture {
        assert_eq!(number(&b[key]), constant as u64, "{key}");
    }
    // three bytes a character: the bound is reached at a third of the count
    let cjk = |bytes: usize| "模".repeat(bytes / 3);
    let ascii = |bytes: usize| "a".repeat(bytes);
    for (field, max) in [
        ("name", PRESET_NAME_MAX_BYTES),
        ("title", PRESET_TITLE_MAX_BYTES),
    ] {
        for at in [ascii(max), cjk(max)] {
            assert_eq!(at.len(), max);
            assert!(
                board_accepts(preset_with(field, at.into()), None),
                "{field} at the bound"
            );
        }
        for over in [ascii(max + 1), cjk(max + 3)] {
            assert!(
                !board_accepts(preset_with(field, over.into()), None),
                "{field} over"
            );
        }
    }
    let dir = |bytes: usize| format!("/{}", ascii(bytes - 1));
    assert!(board_accepts(
        preset_with("dir", dir(PRESET_DIR_MAX_BYTES).into()),
        None
    ));
    assert!(!board_accepts(
        preset_with("dir", dir(PRESET_DIR_MAX_BYTES + 1).into()),
        None
    ));
    let cjk_dir = format!("/{}", "模".repeat(PRESET_DIR_MAX_BYTES / 3 + 1));
    assert!(cjk_dir.chars().count() < PRESET_DIR_MAX_BYTES && cjk_dir.len() > PRESET_DIR_MAX_BYTES);
    assert!(
        !board_accepts(preset_with("dir", cjk_dir.into()), None),
        "dir counts bytes"
    );
    let cmd = |bytes: usize| format!("codex {}", ascii(bytes - 6));
    assert!(board_accepts(
        preset_with("cmd", cmd(PRESET_CMD_MAX_BYTES).into()),
        None
    ));
    assert!(!board_accepts(
        preset_with("cmd", cmd(PRESET_CMD_MAX_BYTES + 1).into()),
        None
    ));
    let steps = |count: usize, text: String| serde_json::json!(vec![text; count]);
    assert!(board_accepts(
        preset_with("steps", steps(PRESET_STEPS_MAX, "s".into())),
        None
    ));
    assert!(!board_accepts(
        preset_with("steps", steps(PRESET_STEPS_MAX + 1, "s".into())),
        None
    ));
    assert!(board_accepts(
        preset_with("steps", steps(1, ascii(PRESET_STEP_MAX_BYTES))),
        None
    ));
    assert!(!board_accepts(
        preset_with("steps", steps(1, ascii(PRESET_STEP_MAX_BYTES + 1))),
        None
    ));
    assert!(!board_accepts(
        preset_with("steps", steps(1, cjk(PRESET_STEP_MAX_BYTES + 1))),
        None
    ));
    // a frozen plan: its step count and the template name every step carries
    assert!(board_accepts(None, inbound_plan(PLAN_STEPS_MAX, "t")));
    assert!(!board_accepts(None, inbound_plan(PLAN_STEPS_MAX + 1, "t")));
    for at in [
        ascii(PLAN_TEMPLATE_NAME_MAX_BYTES),
        cjk(PLAN_TEMPLATE_NAME_MAX_BYTES),
    ] {
        assert!(
            board_accepts(None, inbound_plan(1, &at)),
            "plan template name at the bound"
        );
        assert!(
            board_accepts(None, channel_run(&at)),
            "channel template name at the bound"
        );
    }
    for over in [
        ascii(PLAN_TEMPLATE_NAME_MAX_BYTES + 1),
        cjk(PLAN_TEMPLATE_NAME_MAX_BYTES + 3),
    ] {
        assert!(
            !board_accepts(None, inbound_plan(1, &over)),
            "plan template name over"
        );
        assert!(
            !board_accepts(None, channel_run(&over)),
            "channel template name over"
        );
    }
}

/// The unit gaps the fixture lists, on this side: the two keys of a pair
/// carry the same number, the sample has no more characters than it and more
/// bytes, the settings validation (characters) accepts the sample as a
/// template name, and the Board (bytes) refuses it in the field the pair
/// names. Known and left open: `documents::PLAN_TEMPLATE_NAME_MAX_BYTES`
/// says why counting characters there is not a local change.
#[test]
fn the_listed_unit_gaps_are_real_on_the_board_side() {
    let l = limits();
    let at = |key: &str| {
        let (section, name) = key.split_once('.').unwrap();
        number(&l[section][name]) as usize
    };
    let sample = l["unit_gaps"]["sample"].as_str().unwrap();
    let pairs = l["unit_gaps"]["pairs"].as_array().unwrap();
    assert_eq!(pairs.len(), 3, "a new pair needs its refusal named below");
    for pair in pairs {
        let (chars_key, bytes_key) = (pair[0].as_str().unwrap(), pair[1].as_str().unwrap());
        assert!(
            chars_key.ends_with("_chars") && bytes_key.ends_with("_bytes"),
            "{pair}"
        );
        assert_eq!(
            at(chars_key),
            at(bytes_key),
            "{pair}: one number, two units"
        );
        assert!(
            sample.chars().count() <= at(chars_key),
            "{pair}: within the characters"
        );
        assert!(sample.len() > at(bytes_key), "{pair}: over the bytes");
        let refused = match bytes_key {
            "board.plan_template_name_max_bytes" => {
                !board_accepts(None, inbound_plan(1, sample))
                    && !board_accepts(None, channel_run(sample))
            }
            "board.preset_name_max_bytes" => {
                !board_accepts(preset_with("name", sample.into()), None)
            }
            "board.preset_title_max_bytes" => {
                !board_accepts(preset_with("title", sample.into()), None)
            }
            other => panic!("unit gap {other} has no Board check"),
        };
        assert!(refused, "{pair}: the Board refuses what the editors accept");
    }
    // the settings validation is on the editors' side of the first pair
    let rule = serde_json::json!({"rules": [{
        "id": "R-deck", "source": "slack", "badge": "deck", "projectId": "P1", "columnId": "C1",
        "cmd": "claude", "template": sample
    }]});
    assert!(
        crate::inbound::validate_settings(&rule).is_ok(),
        "settings count characters"
    );
}

#[test]
fn notify_label_bounds_match_the_fixture() {
    let n = &limits()["notify"];
    assert_eq!(number(&n["labels_max"]), crate::notify::LABELS_MAX as u64);
    assert_eq!(
        number(&n["label_title_max_bytes"]),
        crate::notify::LABEL_TITLE_MAX_BYTES as u64
    );
    assert_eq!(n.as_object().unwrap().len(), 2, "every key is held");
}
