//! Backend ↔ frontend mirrored constants: the Rust half of the mirror whose
//! JS half is `ui/test/limits.test.mjs`. Both compare their own constants
//! with the one list in `ui/test/fixtures/limits.json`; neither parses the
//! other's source. The backend stays authoritative (it refuses on save).
//! Scheduler item state words are deliberately absent (FR-3 turns them into
//! an enum); MCP local command errors are checked in `mcp/tests.rs`, where
//! their constants are visible.

use crate::context::ContextStatus;
use serde_json::Value;

fn limits() -> Value {
    serde_json::from_str(include_str!("../../ui/test/fixtures/limits.json")).unwrap()
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
