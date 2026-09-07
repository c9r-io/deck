//! The clock source of auto-respond ("自动化"): a `clock` rule's schedule
//! turns local-time slots into inbound events.
//!
//! # Contract
//! Every poll offers ONE event per enabled clock rule whose schedule has a
//! slot today that is already due (`now >= slot`) and not before the rule's
//! `since`: `Event {source: "clock", key: <slot epoch>, badge: <rule id>,
//! text: <rule name>}` — nothing else. The dispatcher's ledger is the only
//! thing that makes a slot fire once (the same key/badge is offered again on
//! every poll until it is acked), so this source keeps no state and never
//! decides; a slot deck slept through is offered until local midnight, then
//! disappears with the day. The local day comes from `procinfo::local_clock`
//! (libc `localtime_r` + `mktime`, never a spawned `date`); its `day_start`
//! is one value for the whole day even across a DST switch, so a slot never
//! gets a second key. Clock events are live by
//! nature: the baseline gate that protects badge sources does not apply.

use tauri::AppHandle;

use crate::inbound::{Config, Event, Rule, Source, SourceStatus};
use crate::procinfo::{local_clock, LocalClock};

#[derive(Default)]
pub(crate) struct Clock {
    last_poll: Option<u64>,
}

/// The slot of `rule` that is due on the day `clock` describes, if any.
pub(crate) fn due_slot(rule: &Rule, clock: &LocalClock) -> Option<u64> {
    if rule.source != "clock" || !rule.enabled {
        return None;
    }
    let schedule = rule.schedule.as_ref()?;
    if !schedule.matches_day(clock) {
        return None;
    }
    let slot = clock.day_start + u64::from(schedule.minute) * 60;
    (clock.now >= slot && slot >= rule.since).then_some(slot)
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
    use crate::inbound::config_from_value;
    use serde_json::json;

    fn cfg(rules: Vec<serde_json::Value>) -> Config {
        config_from_value(Some(&json!({ "rules": rules })))
    }

    fn rule(id: &str, schedule: serde_json::Value) -> serde_json::Value {
        json!({"id": id, "source": "clock", "badge": id, "projectId": "P1", "columnId": "C1",
               "cmd": "claude", "template": "morning", "name": "Morning", "schedule": schedule})
    }

    /// A clock at `min` on a given ISO weekday / day of a 30-day month.
    fn day_clock(now: u64, min: u32, wday: u32, mday: u32) -> LocalClock {
        LocalClock {
            now,
            min,
            day_start: now - u64::from(min) * 60,
            wday,
            mday,
            mdays: 30,
        }
    }

    const NOW: u64 = 1_000_000;

    #[test]
    fn a_daily_rule_is_due_from_its_minute_until_midnight() {
        let c = cfg(vec![rule("r1", json!({"unit": "day", "minute": 540}))]);
        assert!(due_events(&c, &LocalClock::synthetic(NOW, 539)).is_empty());
        let at = due_events(&c, &LocalClock::synthetic(NOW, 540));
        assert_eq!(at.len(), 1);
        assert_eq!(at[0].source, "clock");
        assert_eq!(at[0].badge, "r1");
        assert_eq!(at[0].text, "Morning");
        assert_eq!(at[0].key, (NOW - 540 * 60 + 540 * 60).to_string());
        // the same day, hours later: the same slot key is offered again
        let late = due_events(&c, &LocalClock::synthetic(NOW + (1400 - 540) * 60, 1400));
        assert_eq!(
            late[0].key, at[0].key,
            "the same slot all day — the ledger dedupes it"
        );
    }

    #[test]
    fn weekly_and_monthly_rules_match_their_days() {
        let weekly = cfg(vec![rule(
            "w",
            json!({"unit": "week", "days": [1, 5], "minute": 0}),
        )]);
        assert_eq!(
            due_events(&weekly, &day_clock(NOW, 600, 1, 7)).len(),
            1,
            "Monday"
        );
        assert!(
            due_events(&weekly, &day_clock(NOW, 600, 2, 8)).is_empty(),
            "Tuesday"
        );
        assert_eq!(
            due_events(&weekly, &day_clock(NOW, 600, 5, 11)).len(),
            1,
            "Friday"
        );
        let monthly = cfg(vec![rule(
            "m",
            json!({"unit": "month", "days": [1, 31], "minute": 0}),
        )]);
        assert_eq!(due_events(&monthly, &day_clock(NOW, 600, 4, 1)).len(), 1);
        assert!(due_events(&monthly, &day_clock(NOW, 600, 4, 15)).is_empty());
        assert_eq!(
            due_events(&monthly, &day_clock(NOW, 600, 4, 30)).len(),
            1,
            "the 31st of a 30-day month is its last day"
        );
    }

    #[test]
    fn paused_rules_slack_rules_and_slots_before_since_offer_nothing() {
        let mut paused = rule("p", json!({"unit": "day", "minute": 0}));
        paused["enabled"] = json!(false);
        let slack = json!({"id": "s", "source": "slack", "badge": "deck", "projectId": "P1",
                           "columnId": "C1", "cmd": "claude", "template": "triage"});
        let mut fresh = rule("f", json!({"unit": "day", "minute": 0}));
        fresh["since"] = json!(NOW + 1);
        let c = cfg(vec![paused, slack, fresh]);
        assert!(c.rules.len() == 3, "all three rules are valid");
        assert!(due_events(&c, &LocalClock::synthetic(NOW, 600)).is_empty());
        let mut old = rule("o", json!({"unit": "day", "minute": 0}));
        old["since"] = json!(NOW - 36_000);
        assert_eq!(
            due_events(&cfg(vec![old]), &LocalClock::synthetic(NOW, 600)).len(),
            1
        );
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
