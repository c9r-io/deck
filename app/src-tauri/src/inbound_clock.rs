//! The clock trigger of automations ("自动化"): a `clock` rule's schedule
//! turns local-time slots into inbound events.
//!
//! # Contract
//! Every poll offers ONE event per enabled clock rule whose schedule has a
//! slot today that is already due and not before the rule's `since`:
//! `Event {source: "clock", key: <slot key>, badge: <rule id>, text: <rule
//! name>}` — nothing else. A slot has two facts that must not be confused:
//! - its KEY, `day_start + minute * 60`: the slot's identity, one value for
//!   the whole local day. The ledger, the run list, a card's `origin.key`
//!   and the first-send claims all hold this key, and its formula never
//!   changes — so an update cannot hand the same logical slot a second
//!   identity and run it twice.
//! - the instant it is DUE, `LocalClock::instant(minute)`: the zone's
//!   reading of that wall time on that day (`procinfo::wall_time_epoch`;
//!   the contract for skipped and repeated wall times is pinned there and
//!   in ui/test/fixtures/schedule-cases.json). Only on a DST switch day do
//!   key and instant differ, by the shift, for wall times after it. `since`
//!   and the dispatcher's grace (`inbound::slot_missed`) compare against the
//!   instant, never the key.
//!
//! The dispatcher's ledger is the only thing that makes a slot fire once
//! (the same key/badge is offered again on every poll until it is acked), so
//! this source keeps no state and never decides: a slot is offered until
//! local midnight, and whether a slot deck slept through still starts a run
//! (inside the rule's `graceMin`) or is recorded as missed is the
//! dispatcher's call. The local day comes from `procinfo::local_clock` (libc
//! `localtime_r` + `mktime`, never a spawned `date`); its `day_start` is one
//! value for the whole day even across a DST switch, so a slot never gets a
//! second key. Whether today is a slot day at all — the weekday or day of
//! month, and for an interval rule the on period counted from its anchor
//! date — is `Schedule::matches_day` (inbound.rs). Clock events are live by
//! nature: the baseline gate that protects badge sources does not apply.

use tauri::AppHandle;

use crate::inbound::{Config, Event, Rule, Source, SourceStatus};
use crate::procinfo::{local_clock, LocalClock};

#[derive(Default)]
pub(crate) struct Clock {
    last_poll: Option<u64>,
}

/// The key of the slot of `rule` that is due on the day `clock` describes,
/// if any.
pub(crate) fn due_slot(rule: &Rule, clock: &LocalClock) -> Option<u64> {
    if rule.source != "clock" || !rule.enabled {
        return None;
    }
    let schedule = rule.schedule.as_ref()?;
    if !schedule.matches_day(clock) {
        return None;
    }
    let key = clock.day_start + u64::from(schedule.minute) * 60;
    let due = clock.instant(schedule.minute);
    (clock.now >= due && due >= rule.since).then_some(key)
}

pub(crate) fn due_events(cfg: &Config, clock: &LocalClock) -> Vec<Event> {
    cfg.rules
        .iter()
        .filter_map(|rule| {
            due_slot(rule, clock).map(|slot| Event {
                source: "clock".into(),
                key: slot.to_string(),
                badge: rule.id.clone(),
                text: rule.name.clone(),
                from: String::new(),
                where_: String::new(),
                link: String::new(),
            })
        })
        .collect()
}

impl Source for Clock {
    fn id(&self) -> &'static str {
        "clock"
    }

    fn enabled(&self, cfg: &Config) -> bool {
        cfg.rules.iter().any(|r| r.source == "clock" && r.enabled)
    }

    fn poll(&mut self, cfg: &Config, _badges: &[String]) -> Result<Vec<Event>, &'static str> {
        let clock = local_clock();
        self.last_poll = Some(clock.now);
        Ok(due_events(cfg, &clock))
    }

    fn set_live(&mut self, _app: &AppHandle, _wanted: bool, _badges: &[String]) {}

    fn status(&self) -> SourceStatus {
        SourceStatus {
            live: false,
            last_poll: self.last_poll,
            last_error: None,
        }
    }

    fn polled_events_are_live(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inbound::{config_from_value, slot_missed};
    use crate::procinfo::clock_at;
    use crate::procinfo::tz_test::{TzOverride, TZ_TESTS};
    use crate::sync::LockRecover;
    use serde_json::json;

    fn cfg(rules: Vec<serde_json::Value>) -> Config {
        config_from_value(Some(&json!({ "rules": rules })))
    }

    fn rule(id: &str, schedule: serde_json::Value) -> serde_json::Value {
        json!({"id": id, "source": "clock", "badge": id, "projectId": "P1", "columnId": "C1",
               "cmd": "claude", "template": "morning", "name": "Morning", "schedule": schedule})
    }

    /// A clock at `minute` on a calendar date, under UTC so that the slot
    /// instant mktime gives is the civil clock's own `now`.
    fn at(year: i32, month: u32, day: u32, minute: u32) -> LocalClock {
        LocalClock::civil(year, month, day, minute)
    }

    #[test]
    fn a_daily_rule_is_due_from_its_minute_until_midnight() {
        let _tz = TZ_TESTS.lock_or_recover();
        let _zone = TzOverride::set("UTC");
        let c = cfg(vec![rule("r1", json!({"unit": "day", "minute": 540}))]);
        assert!(due_events(&c, &at(2026, 10, 19, 539)).is_empty());
        let due = due_events(&c, &at(2026, 10, 19, 540));
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].source, "clock");
        assert_eq!(due[0].badge, "r1");
        assert_eq!(due[0].text, "Morning");
        assert_eq!(due[0].key, at(2026, 10, 19, 540).now.to_string());
        // the same day, hours later: the same slot key is offered again
        let late = due_events(&c, &at(2026, 10, 19, 1400));
        assert_eq!(
            late[0].key, due[0].key,
            "the same slot all day — the ledger dedupes it"
        );
    }

    #[test]
    fn weekly_and_monthly_rules_match_their_days() {
        let _tz = TZ_TESTS.lock_or_recover();
        let _zone = TzOverride::set("UTC");
        let weekly = cfg(vec![rule(
            "w",
            json!({"unit": "week", "days": [1, 5], "minute": 0}),
        )]);
        assert_eq!(
            due_events(&weekly, &at(2026, 10, 19, 600)).len(),
            1,
            "Monday"
        );
        assert!(
            due_events(&weekly, &at(2026, 10, 20, 600)).is_empty(),
            "Tuesday"
        );
        assert_eq!(
            due_events(&weekly, &at(2026, 10, 23, 600)).len(),
            1,
            "Friday"
        );
        let monthly = cfg(vec![rule(
            "m",
            json!({"unit": "month", "days": [1, 31], "minute": 0}),
        )]);
        assert_eq!(due_events(&monthly, &at(2026, 11, 1, 600)).len(), 1);
        assert!(due_events(&monthly, &at(2026, 11, 15, 600)).is_empty());
        assert_eq!(
            due_events(&monthly, &at(2026, 11, 30, 600)).len(),
            1,
            "the 31st of a 30-day month is its last day"
        );
    }

    /// An interval rule has slots only in its on periods, counted from its
    /// anchor; a period that was skipped or missed does not move the phase,
    /// and the anchor is also the first day anything can fire.
    #[test]
    fn interval_rules_fire_in_their_on_periods_only() {
        let _tz = TZ_TESTS.lock_or_recover();
        let _zone = TzOverride::set("UTC");
        let sprint = cfg(vec![rule(
            "s",
            json!({"unit": "week", "every": 2, "anchor": "2026-10-19", "days": [1], "minute": 540}),
        )]);
        assert_eq!(
            due_events(&sprint, &at(2026, 10, 19, 600)).len(),
            1,
            "anchor Monday"
        );
        assert!(
            due_events(&sprint, &at(2026, 10, 26, 600)).is_empty(),
            "off week"
        );
        assert_eq!(
            due_events(&sprint, &at(2026, 11, 2, 600)).len(),
            1,
            "on week again"
        );
        assert!(
            due_events(&sprint, &at(2026, 10, 12, 600)).is_empty(),
            "before the anchor"
        );
        let midweek = cfg(vec![rule(
            "m",
            json!({"unit": "week", "every": 2, "anchor": "2026-10-21", "days": [1, 5], "minute": 540}),
        )]);
        assert!(
            due_events(&midweek, &at(2026, 10, 19, 600)).is_empty(),
            "the Monday before a Wednesday anchor"
        );
        assert_eq!(
            due_events(&midweek, &at(2026, 10, 23, 600)).len(),
            1,
            "the Friday after it"
        );
        assert!(
            due_events(&midweek, &at(2026, 10, 26, 600)).is_empty(),
            "off week"
        );
        let quarterly = cfg(vec![rule(
            "q",
            json!({"unit": "month", "every": 3, "anchor": "2026-10-01", "days": [1], "minute": 0}),
        )]);
        assert!(due_events(&quarterly, &at(2026, 11, 1, 600)).is_empty());
        assert!(due_events(&quarterly, &at(2026, 12, 1, 600)).is_empty());
        assert_eq!(due_events(&quarterly, &at(2027, 1, 1, 600)).len(), 1);
        let yearly = cfg(vec![rule(
            "y",
            json!({"unit": "month", "every": 12, "anchor": "2026-10-01", "days": [1], "minute": 0}),
        )]);
        assert!(due_events(&yearly, &at(2027, 4, 1, 600)).is_empty());
        assert_eq!(due_events(&yearly, &at(2027, 10, 1, 600)).len(), 1);
        let thirds = cfg(vec![rule(
            "d",
            json!({"unit": "day", "every": 3, "anchor": "2026-10-30", "minute": 0}),
        )]);
        assert!(due_events(&thirds, &at(2026, 10, 29, 600)).is_empty());
        assert_eq!(due_events(&thirds, &at(2026, 10, 30, 600)).len(), 1);
        assert!(due_events(&thirds, &at(2026, 11, 1, 600)).is_empty());
        assert_eq!(
            due_events(&thirds, &at(2026, 11, 2, 600)).len(),
            1,
            "across the month end"
        );
    }

    /// The zone contract on the two 2026 switch days of America/New_York: a
    /// 09:00 rule is due at 09:00 wall time (not an hour off), its key is
    /// the unchanged `day_start + minute * 60`, and the dispatcher's grace
    /// is judged from the wall time, so the autumn slot is not "missed" a
    /// quarter past nine because its key sits at eight.
    #[test]
    fn switch_days_keep_their_key_and_fire_at_wall_time() {
        let _tz = TZ_TESTS.lock_or_recover();
        let _zone = TzOverride::set("America/New_York");
        let c = cfg(vec![rule("r", json!({"unit": "day", "minute": 540}))]);
        let rule = c.rules[0].clone();
        // 2026-03-08 (spring): midnight EST 1772946000; 09:00 EDT = 1772974800
        let spring_midnight = 1_772_946_000u64;
        let spring_nine = 1_772_974_800u64;
        assert!(
            due_events(&c, &clock_at((spring_nine - 60) as libc::time_t)).is_empty(),
            "08:59 EDT: not yet"
        );
        let due = due_events(&c, &clock_at(spring_nine as libc::time_t));
        assert_eq!(due.len(), 1, "09:00 EDT: due");
        assert_eq!(
            due[0].key,
            (spring_midnight + 540 * 60).to_string(),
            "the key is still midnight + minute (10:00 EDT as an instant)"
        );
        assert!(!slot_missed(&due[0].key, &rule, spring_nine + 15 * 60));
        assert!(slot_missed(&due[0].key, &rule, spring_nine + 15 * 60 + 1));
        // 2026-11-01 (autumn): midnight EDT 1793505600; 09:00 EST = 1793541600
        let fall_midnight = 1_793_505_600u64;
        let fall_nine = 1_793_541_600u64;
        assert!(
            due_events(&c, &clock_at((fall_midnight + 8 * 3600) as libc::time_t)).is_empty(),
            "08:00 EST, where the old formula fired: not yet"
        );
        assert!(
            due_events(&c, &clock_at((fall_nine - 60) as libc::time_t)).is_empty(),
            "08:59 EST"
        );
        let due = due_events(&c, &clock_at(fall_nine as libc::time_t));
        assert_eq!(due.len(), 1, "09:00 EST: due");
        assert_eq!(due[0].key, (fall_midnight + 540 * 60).to_string());
        assert!(
            !slot_missed(&due[0].key, &rule, fall_nine + 10 * 60),
            "09:10 EST is inside the grace of a 09:00 slot"
        );
        assert!(slot_missed(&due[0].key, &rule, fall_nine + 15 * 60 + 1));
        // `since` compares the wall-time instant too: a schedule saved at
        // 08:30 EST on the autumn day still fires its 09:00 slot
        let mut saved = rule.clone();
        saved.since = fall_nine - 30 * 60;
        let c = Config {
            rules: vec![saved],
            ..Config::default()
        };
        assert_eq!(
            due_events(&c, &clock_at(fall_nine as libc::time_t)).len(),
            1
        );
        let mut later = rule.clone();
        later.since = fall_nine + 1;
        let c = Config {
            rules: vec![later],
            ..Config::default()
        };
        assert!(due_events(&c, &clock_at((fall_nine + 60) as libc::time_t)).is_empty());
    }

    #[test]
    fn paused_rules_slack_rules_and_slots_before_since_offer_nothing() {
        let _tz = TZ_TESTS.lock_or_recover();
        let _zone = TzOverride::set("UTC");
        let now = at(2026, 10, 19, 600).now;
        let mut paused = rule("p", json!({"unit": "day", "minute": 0}));
        paused["enabled"] = json!(false);
        let slack = json!({"id": "s", "source": "slack", "badge": "deck", "projectId": "P1",
                           "columnId": "C1", "cmd": "claude", "template": "triage"});
        let mut fresh = rule("f", json!({"unit": "day", "minute": 0}));
        fresh["since"] = json!(now + 1);
        let c = cfg(vec![paused, slack, fresh]);
        assert!(c.rules.len() == 3, "all three rules are valid");
        assert!(due_events(&c, &at(2026, 10, 19, 600)).is_empty());
        let mut old = rule("o", json!({"unit": "day", "minute": 0}));
        old["since"] = json!(now - 36_000);
        assert_eq!(due_events(&cfg(vec![old]), &at(2026, 10, 19, 600)).len(), 1);
    }

    #[test]
    fn the_source_is_enabled_by_any_live_clock_rule_and_needs_no_baseline() {
        let clock = Clock::default();
        assert!(clock.polled_events_are_live());
        assert_eq!(clock.id(), "clock");
        assert!(!clock.enabled(&cfg(vec![])));
        assert!(clock.enabled(&cfg(vec![rule("r", json!({"unit": "day", "minute": 0}))])));
        let mut paused = rule("r", json!({"unit": "day", "minute": 0}));
        paused["enabled"] = json!(false);
        assert!(!clock.enabled(&cfg(vec![paused])));
    }
}
