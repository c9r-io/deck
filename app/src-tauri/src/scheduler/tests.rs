//! Scheduler unit tests: the tick/selection/delivery contract with fake
//! probe/fire/persist hooks. Shared fixtures live at the top.

use super::*;
use crate::context::ProbeResult;
use crate::error::DeckError;

fn qi(id: &str, mode: &str) -> QueueItem {
    QueueItem {
        id: id.into(),
        session: "s".into(),
        card_id: "card-s".into(),
        dir: String::new(),
        cmd: String::new(),
        text: "x".into(),
        mode: mode.into(),
        at: None,
        added: 0,
        every: None,
        win_from: None,
        win_to: None,
        until_n: None,
        until_at: None,
        fired: 0,
        paused: false,
        last: None,
        state: default_state(),
        attempts: 0,
        last_error: None,
        last_attempt_at: None,
        steps: Vec::new(),
        tpl: None,
        tpl_idx: None,
        tpl_total: None,
        group: None,
        seq: None,
        rule: None,
        delivery: None,
        expected_process: Some("codex".into()),
        binding: None,
        last_context: None,
        revision: 0,
    }
}

fn rule(every: u64) -> QueueItem {
    let mut i = qi("t", "every");
    i.every = Some(every);
    i
}

/// Build a QueueState the way load_queue does: adjacency-derived groups
/// (a chain joins the previous non-rule item of its session).
fn qs(items: Vec<QueueItem>) -> QueueState {
    let mut q = QueueState {
        items,
        last_fired: HashMap::new(),
        deliveries: Vec::new(),
        pending: Vec::new(),
        cancelled: Vec::new(),
    };
    migrate_groups(&mut q);
    q
}

fn ids(v: &[QueueItem]) -> Vec<&str> {
    v.iter().map(|i| i.id.as_str()).collect()
}

const NOW: u64 = 1_000_000;

#[test]
fn at_fires_only_when_due() {
    let mut a = qi("a", "at");
    a.at = Some(NOW - 1);
    let mut b = qi("b", "at");
    b.session = "other".into();
    b.at = Some(NOW + 100);
    let q = qs(vec![a, b]);
    assert_eq!(ids(&select_due(&q, NOW, 720, &HashMap::new())), ["a"]);
}

#[test]
fn future_at_does_not_block_due_at() {
    // the future item was queued FIRST — it must not own a head slot
    let mut fut = qi("fut", "at");
    fut.at = Some(NOW + 3600);
    let mut due = qi("due", "at");
    due.at = Some(NOW - 1);
    due.added = 10;
    let q = qs(vec![fut, due]);
    assert_eq!(ids(&select_due(&q, NOW, 720, &HashMap::new())), ["due"]);
}

#[test]
fn two_due_ats_same_session_one_per_tick() {
    let mut a1 = qi("a1", "at");
    a1.at = Some(NOW - 50);
    let mut a2 = qi("a2", "at");
    a2.at = Some(NOW - 5);
    let q = qs(vec![a2.clone(), a1.clone()]);
    // exactly one candidate, deterministically the earliest-due
    assert_eq!(ids(&select_due(&q, NOW, 720, &HashMap::new())), ["a1"]);
}

#[test]
fn two_every_rules_same_session_one_per_tick() {
    let mut r1 = rule(300);
    r1.id = "r1".into();
    let mut r2 = rule(300);
    r2.id = "r2".into();
    let q = qs(vec![r1, r2]);
    assert_eq!(ids(&select_due(&q, NOW, 720, &HashMap::new())), ["r1"]);
}

#[test]
fn every_plus_at_both_due_picks_the_at() {
    let mut a = qi("a", "at");
    a.at = Some(NOW - 1);
    let q = qs(vec![rule(300), a]);
    assert_eq!(ids(&select_due(&q, NOW, 720, &HashMap::new())), ["a"]);
}

#[test]
fn pause_after_selection_is_honored_before_send() {
    let mut a = qi("a", "at");
    a.at = Some(NOW - 1);
    let mut q = qs(vec![a]);
    // tick-start selection sees it...
    assert_eq!(ids(&select_due(&q, NOW, 720, &HashMap::new())), ["a"]);
    // ...user pauses before the send: the loop's FRESH per-send
    // selection (select_for_session) must come up empty
    q.items[0].paused = true;
    assert!(select_for_session(&q, "s", NOW, 720, &HashMap::new()).is_none());
}

#[test]
fn update_after_selection_sends_the_new_text() {
    let mut a = qi("a", "at");
    a.at = Some(NOW - 1);
    let mut q = qs(vec![a]);
    assert_eq!(select_due(&q, NOW, 720, &HashMap::new())[0].text, "x");
    q.items[0].text = "edited".into();
    let fresh = select_for_session(&q, "s", NOW, 720, &HashMap::new()).unwrap();
    assert_eq!(fresh.text, "edited");
}

#[test]
fn failed_head_blocks_group_until_user_skips_or_retries() {
    let mut c1 = qi("c1", "chain");
    c1.state = "failed".into();
    c1.attempts = MAX_ATTEMPTS; // dead: attempts exhausted
    let c2 = qi("c2", "chain"); // adjacency → same group as c1
    let mut q = qs(vec![c1, c2]);
    let quiet: HashMap<String, u64> = [("s".into(), NOW - 400)].into();
    // the dead head blocks its group — nothing fires on its own
    assert!(select_due(&q, NOW, 720, &quiet).is_empty());
    // user skip (= remove the failed step) unblocks the successor
    let mut skipped = q.clone();
    skipped.items.retain(|i| i.id != "c1");
    assert_eq!(ids(&select_due(&skipped, NOW, 720, &quiet)), ["c2"]);
    // user retry re-arms the failed step itself instead
    q.items[0].state = default_state();
    q.items[0].attempts = 0;
    assert_eq!(ids(&select_due(&q, NOW, 720, &quiet)), ["c1"]);
}

#[test]
fn failed_item_backs_off_then_retries_with_priority() {
    let mut c1 = qi("c1", "chain");
    c1.state = "failed".into();
    c1.attempts = 1;
    c1.last_attempt_at = Some(NOW - 10);
    let mut a = qi("a", "at");
    a.at = Some(NOW - 1);
    a.added = 10; // separate group (not chained after c1? adjacency: at starts its own group)
    let mut q = qs(vec![c1, a]);
    let quiet: HashMap<String, u64> = [("s".into(), NOW - 400)].into();
    // 10s after 1st failure: backoff (20s) holds the retry; the due at runs
    assert_eq!(ids(&select_due(&q, NOW, 720, &quiet)), ["a"]);
    // backoff elapsed → the retry outranks even a due at
    q.items[0].last_attempt_at = Some(NOW - 30);
    assert_eq!(ids(&select_due(&q, NOW, 720, &quiet)), ["c1"]);
    assert_eq!(backoff_secs(1), 20);
    assert_eq!(backoff_secs(20), 1800, "backoff is capped");
}

#[test]
fn session_min_gap_applies_to_every_mode() {
    let mut a = qi("a", "at");
    a.at = Some(NOW - 1);
    let mut q = qs(vec![a]);
    q.last_fired.insert("s".into(), NOW - 10);
    assert!(select_due(&q, NOW, 720, &HashMap::new()).is_empty());
    q.last_fired
        .insert("s".into(), NOW - SESSION_MIN_GAP_SECS - 1);
    assert_eq!(ids(&select_due(&q, NOW, 720, &HashMap::new())), ["a"]);
}

#[test]
fn chain_respects_order_quiet_and_gap() {
    let c1 = qi("c1", "chain");
    let c2 = qi("c2", "chain");
    let mut q = qs(vec![c1, c2]);
    // quiet session, no prior fire → only the HEAD chain step fires
    let quiet: HashMap<String, u64> = [("s".into(), NOW - 400)].into();
    assert_eq!(ids(&select_due(&q, NOW, 720, &quiet)), ["c1"]);
    // recent activity → nothing
    let busy: HashMap<String, u64> = [("s".into(), NOW - 10)].into();
    assert!(select_due(&q, NOW, 720, &busy).is_empty());
    // fired 10s ago → min-gap blocks even a quiet session
    q.last_fired.insert("s".into(), NOW - 10);
    assert!(select_due(&q, NOW, 720, &quiet).is_empty());
}

#[test]
fn sessions_stay_parallel_one_candidate_each() {
    let mut a = qi("a", "at");
    a.at = Some(NOW - 1);
    let mut b = qi("b", "at");
    b.session = "other".into();
    b.at = Some(NOW - 1);
    let q = qs(vec![a, b]);
    let due = select_due(&q, NOW, 720, &HashMap::new());
    assert_eq!(due.len(), 2);
    let mut sessions: Vec<_> = due.iter().map(|i| i.session.as_str()).collect();
    sessions.sort_unstable();
    assert_eq!(sessions, ["other", "s"]);
}

#[test]
fn recurring_iterations_do_not_interleave() {
    let mut r = rule(300);
    r.steps = vec!["s2".into()];
    let mut q = qs(vec![r]);
    // first fire spawns the iteration's follow-up step
    finalize_delivery(&mut q, "t", "d1", NOW, false);
    assert_eq!(q.items.len(), 2, "rule + spawned step");
    // cadence elapsed again, session quiet — but the previous iteration
    // still has a live step, so the rule must NOT fire
    let quiet: HashMap<String, u64> = [("s".into(), NOW + 600 - 400)].into();
    let later = NOW + 600;
    let due = select_due(&q, later, 720, &quiet);
    // the only candidate can be the iteration's chain step, never the rule
    assert!(due.iter().all(|i| i.mode != "every"));
    // step done → next iteration may start
    let step_id = q
        .items
        .iter()
        .find(|i| i.mode == "chain")
        .unwrap()
        .id
        .clone();
    finalize_delivery(&mut q, &step_id, "d2", later, false);
    let quiet2: HashMap<String, u64> = [("s".into(), later + 300 - 400)].into();
    let due = select_due(&q, later + 300, 720, &quiet2);
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].mode, "every");
}

#[test]
fn paused_rule_skipped_resume_restores() {
    let mut r = rule(300);
    r.paused = true;
    let mut q = qs(vec![r]);
    assert!(select_due(&q, NOW, 720, &HashMap::new()).is_empty());
    q.items[0].paused = false;
    assert_eq!(select_due(&q, NOW, 720, &HashMap::new()).len(), 1);
}

#[test]
fn migration_derives_groups_from_legacy_adjacency() {
    let mut a = qi("a", "at");
    a.at = Some(NOW + 10);
    let c = qi("c", "chain");
    let mut b_other = qi("co", "chain");
    b_other.session = "other".into();
    let r = rule(300);
    let q = qs(vec![a, c, r, b_other]); // qs() runs migrate_groups
    let g = |id: &str| {
        q.items
            .iter()
            .find(|i| i.id == id)
            .map(|i| (i.group.clone(), i.seq))
            .unwrap()
    };
    assert_eq!(g("a"), (Some("a".into()), Some(1)));
    assert_eq!(
        g("c"),
        (Some("a".into()), Some(2)),
        "chain joins the at's group"
    );
    assert_eq!(
        g("co"),
        (Some("co".into()), Some(1)),
        "other session: own group"
    );
    assert_eq!(q.items.iter().find(|i| i.id == "t").unwrap().group, None);
}

// ---------- finalize / ambiguous crash recovery ----------

#[test]
fn finalize_is_idempotent_per_delivery() {
    let mut r = rule(300);
    r.steps = vec!["s2".into(), "s3".into()];
    r.tpl = Some("tp".into());
    r.tpl_idx = Some(1);
    r.tpl_total = Some(3);
    let mut q = qs(vec![r]);
    finalize_delivery(&mut q, "t", "d1", NOW, false);
    finalize_delivery(&mut q, "t", "d1", NOW, false); // re-run: no-op
    let rule_item = q.items.iter().find(|i| i.id == "t").unwrap();
    assert_eq!(rule_item.fired, 1, "no double count");
    let steps: Vec<_> = q.items.iter().filter(|i| i.mode == "chain").collect();
    assert_eq!(steps.len(), 2, "iteration steps spawned exactly once");
    assert_eq!(steps[0].group.as_deref(), Some("d1"));
    assert_eq!(steps[0].seq, Some(2));
    assert_eq!(steps[0].rule.as_deref(), Some("t"));
    assert_eq!(steps[0].tpl_idx, Some(2));
    assert_eq!(q.deliveries.len(), 1);
    assert_eq!(q.last_fired.get("s"), Some(&NOW));
}

#[test]
fn crash_after_intent_becomes_ambiguous_and_never_auto_resends() {
    let mut once = qi("o", "at");
    once.state = "firing".into();
    once.delivery = Some("dA".into());
    once.last_attempt_at = Some(NOW - 5);
    let mut r = rule(300);
    r.state = "firing".into();
    r.delivery = Some("dB".into());
    r.last_attempt_at = Some(NOW - 5);
    r.steps = vec!["s2".into()];
    let mut q = qs(vec![once, r]);
    let notes = recover_interrupted(&mut q);
    assert_eq!(notes.len(), 2);
    let once = q.items.iter().find(|i| i.id == "o").unwrap();
    assert_eq!(once.state, "ambiguous");
    let rl = q.items.iter().find(|i| i.id == "t").unwrap();
    assert_eq!(
        (rl.fired, rl.last, rl.state.as_str()),
        (0, None, "ambiguous")
    );
    assert!(q.deliveries.is_empty(), "recovery never claims success");
    assert_eq!(q.items.iter().filter(|i| i.mode == "chain").count(), 0);
    assert!(select_due(&q, NOW + 100_000, 720, &HashMap::new()).is_empty());
}

#[test]
fn repeated_recovery_is_idempotent() {
    let mut r = rule(300);
    r.state = "firing".into();
    r.delivery = Some("dB".into());
    r.last_attempt_at = Some(NOW - 5);
    r.steps = vec!["s2".into()];
    let mut q = qs(vec![r]);
    recover_interrupted(&mut q);
    let snapshot = serde_json::to_string(&q).unwrap();
    recover_interrupted(&mut q);
    assert_eq!(serde_json::to_string(&q).unwrap(), snapshot);
}

#[test]
fn acknowledge_after_crash_matches_live_result_and_is_idempotent() {
    let mut r = rule(300);
    r.steps = vec!["s2".into()];
    r.state = "firing".into();
    r.delivery = Some("dX".into());
    r.last_attempt_at = Some(NOW);
    let pre = qs(vec![r]);
    let mut live = pre.clone();
    finalize_delivery(&mut live, "t", "dX", NOW, false);
    let mut rec = pre.clone();
    recover_interrupted(&mut rec);
    acknowledge_ambiguous(&mut rec, "t").unwrap();
    acknowledge_ambiguous(&mut rec, "t").unwrap();
    let strip = |q: &QueueState| {
        let mut v: Vec<_> = q
            .items
            .iter()
            .map(|i| (i.text.clone(), i.mode.clone(), i.group.clone(), i.seq))
            .collect();
        v.sort();
        (v, q.items.iter().find(|i| i.id == "t").map(|i| i.fired))
    };
    assert_eq!(strip(&live), strip(&rec));
}

#[test]
fn stop_after_n_stays_accurate_across_recovery() {
    let mut r = rule(300);
    r.until_n = Some(2);
    r.fired = 1;
    r.state = "firing".into();
    r.delivery = Some("dZ".into());
    r.last_attempt_at = Some(NOW);
    let mut q = qs(vec![r]);
    recover_interrupted(&mut q);
    assert!(q
        .items
        .iter()
        .any(|i| i.id == "t" && i.state == "ambiguous"));
    acknowledge_ambiguous(&mut q, "t").unwrap();
    acknowledge_ambiguous(&mut q, "t").unwrap();
    // second (and last) fire counted exactly once after acknowledgement
    assert!(!q.items.iter().any(|i| i.id == "t"));
    assert_eq!(q.deliveries.len(), 1);
}

#[test]
fn legacy_firing_item_without_delivery_id_still_recovers_once() {
    let mut once = qi("o", "at");
    once.state = "firing".into(); // pre-delivery-id queue file
    once.last_attempt_at = Some(NOW - 5);
    let mut q = qs(vec![once]);
    recover_interrupted(&mut q);
    assert_eq!(q.items[0].state, "ambiguous");
    acknowledge_ambiguous(&mut q, "o").unwrap();
    assert!(q.items.is_empty());
    acknowledge_ambiguous(&mut q, "o").unwrap();
    recover_interrupted(&mut q);
    assert_eq!(q.deliveries.len(), 1, "synthetic id keeps it idempotent");
}

#[test]
fn delivery_audit_is_capped() {
    let mut q = qs(vec![]);
    for k in 0..(MAX_DELIVERIES + 10) {
        let mut a = qi(&format!("a{k}"), "at");
        a.at = Some(NOW);
        q.items.push(a);
        finalize_delivery(&mut q, &format!("a{k}"), &format!("d{k}"), NOW, false);
    }
    assert_eq!(q.deliveries.len(), MAX_DELIVERIES);
    assert_eq!(
        q.deliveries.last().unwrap().id,
        format!("d{}", MAX_DELIVERIES + 9)
    );
}

// ---------- misc invariants (kept from round one) ----------

#[test]
fn queue_ids_never_collide() {
    let existing = vec![qi("q1-0", "at")];
    let a = next_queue_id(&existing);
    let b = next_queue_id(&existing);
    assert_ne!(a, b);
    assert!(!existing.iter().any(|i| i.id == a || i.id == b));
}

#[test]
fn expired_rules_purge() {
    let mut r = rule(300);
    r.until_at = Some(NOW - 1);
    let mut q = qs(vec![r, qi("keep", "chain")]);
    assert!(purge_expired(&mut q, NOW));
    assert_eq!(q.items.len(), 1);
    assert_eq!(q.items[0].id, "keep");
}

#[test]
fn add_validation_rejects_bad_combinations() {
    let base = || QueueAddArgs {
        session: "s".into(),
        card_id: "card-s".into(),
        dir: String::new(),
        cmd: String::new(),
        text: "x".into(),
        mode: "at".into(),
        at: Some(NOW),
        every: None,
        win_from: None,
        win_to: None,
        until_n: None,
        until_at: None,
        steps: None,
        tpl: None,
        tpl_idx: None,
        tpl_total: None,
    };
    assert!(validate_add(&base()).is_ok());
    let mut a = base();
    a.at = None;
    assert!(validate_add(&a).is_err(), "at without a time");
    let mut a = base();
    a.mode = "every".into();
    a.every = Some(30);
    assert!(validate_add(&a).is_err(), "sub-minute interval");
    let mut a = base();
    a.mode = "every".into();
    a.every = Some(300);
    a.win_from = Some(480);
    assert!(validate_add(&a).is_err(), "one-sided window");
    a.win_to = Some(2000);
    assert!(validate_add(&a).is_err(), "window past 24h");
    let mut a = base();
    a.mode = "chain".into();
    a.at = None;
    a.steps = Some(vec!["y".into()]);
    assert!(validate_add(&a).is_err(), "steps on a non-rule");
    let mut a = base();
    a.mode = "yearly".into();
    assert!(validate_add(&a).is_err(), "unknown mode");
}

#[test]
fn every_rule_due_logic() {
    let now = 1_000_000;
    // never fired → due immediately
    assert!(every_due(&rule(1800), now, 720));
    // fired 10 min ago on a 30-min cadence → not due; after 30 min → due
    let mut r = rule(1800);
    r.last = Some(now - 600);
    assert!(!every_due(&r, now, 720));
    r.last = Some(now - 1800);
    assert!(every_due(&r, now, 720));
    // paused wins over everything
    r.paused = true;
    assert!(!every_due(&r, now, 720));
    r.paused = false;
    // outside the 08:00–18:00 window (22:00) → sleeping
    r.win_from = Some(480);
    r.win_to = Some(1080);
    assert!(!every_due(&r, now, 22 * 60));
    assert!(every_due(&r, now, 9 * 60));
    // stop instant passed → never due again
    r.until_at = Some(now - 1);
    assert!(!every_due(&r, now, 9 * 60));
}

// ---------- round 3: firing contract, delivery ledger, send_one ----------

#[test]
fn user_mutations_conflict_while_firing() {
    let mut a = qi("a", "at");
    a.at = Some(NOW - 1);
    a.state = "firing".into();
    let mut q = qs(vec![a]);
    assert!(update_text(&mut q, "a", "edited".into()).is_err());
    assert!(remove_item(&mut q, "a").is_err());
    assert!(pause_item(&mut q, "a", true).is_err());
    assert!(retry_item(&mut q, "a").is_err());
    // the item is untouched by all four refused operations
    assert_eq!(q.items.len(), 1);
    assert_eq!(q.items[0].state, "firing");
    assert_eq!(q.items[0].text, "x");
    assert!(!q.items[0].paused);
    // once the send finalized (state left "firing") the same ops work
    q.items[0].state = default_state();
    assert!(remove_item(&mut q, "a").unwrap());
    assert!(q.items.is_empty());
}

// ---------- round 4: deleting a card cancels its schedule for good ----------

#[test]
fn deleting_a_card_empties_its_queue_and_spares_other_sessions() {
    let mut a = qi("a", "at");
    a.at = Some(NOW);
    let b = qi("b", "chain");
    let mut r = rule(300);
    r.id = "r".into();
    let mut c = qi("c", "at");
    c.session = "other".into();
    c.at = Some(NOW);
    let mut q = qs(vec![a, b, r, c]);
    q.last_fired.insert("s".into(), NOW - 10);
    clear_session_items(&mut q, "s");
    let left: Vec<&str> = q.items.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(left, ["c"], "every item of the deleted card is gone");
    assert!(!q.last_fired.contains_key("s"), "send-gap entry cleared");
    assert!(is_cancelled(&q, "s"));
    assert!(!is_cancelled(&q, "other"));
    // idempotent: clearing again is a no-op, never an error
    let snapshot = serde_json::to_string(&q).unwrap();
    clear_session_items(&mut q, "s");
    assert_eq!(q.cancelled.len(), 1, "one tombstone per session");
    assert_eq!(
        serde_json::to_string(&q)
            .unwrap()
            .replace(&format!("\"at\":{}", q.cancelled[0].at), "\"at\":T"),
        snapshot.replace(&format!("\"at\":{}", q.cancelled[0].at), "\"at\":T")
    );
}

#[test]
fn a_deleted_recurring_rule_never_becomes_a_candidate_again() {
    let mut r = rule(300);
    r.steps = vec!["s2".into()];
    let mut q = qs(vec![r]);
    assert_eq!(select_due(&q, NOW, 720, &HashMap::new()).len(), 1);
    clear_session_items(&mut q, "s");
    assert!(q.items.is_empty());
    // even if a rule for that session somehow reappears (hand-edited
    // file, stale queue from another deck), it is not schedulable
    q.items.push(rule(300));
    let later = NOW + 10_000;
    assert!(select_due(&q, later, 720, &HashMap::new()).is_empty());
    assert!(select_for_session(&q, "s", later, 720, &HashMap::new()).is_none());
}

#[test]
fn a_delete_during_a_send_audits_the_delivery_but_revives_nothing() {
    // recurring rule mid-send: a live result still finishes its audit,
    // but the deleted card and its future schedule never return
    let mut r = rule(300);
    r.steps = vec!["s2".into(), "s3".into()];
    r.state = "firing".into();
    r.delivery = Some("dX".into());
    let mut q = qs(vec![r]);
    q.pending.push(PendingDelivery {
        id: "dX".into(),
        snapshot: q.items[0].clone(),
    });
    clear_session_items(&mut q, "s"); // the user deletes the card now
    finalize_delivery(&mut q, "t", "dX", NOW, false); // the send lands
    assert_eq!(q.deliveries.len(), 1, "delivery audited");
    assert!(q.items.is_empty(), "no rule restored, no steps spawned");
    assert!(q.pending.is_empty(), "ledger consumed");
    assert!(
        !q.last_fired.contains_key("s"),
        "no cadence bookkeeping for a session that no longer exists"
    );
    // and nothing can fire for that session afterwards, ever
    assert!(select_due(&q, NOW + 100_000, 720, &HashMap::new()).is_empty());
}

#[test]
fn a_crash_right_after_a_delete_does_not_revive_anything() {
    let mut r = rule(300);
    r.steps = vec!["s2".into()];
    r.state = "firing".into();
    r.delivery = Some("dX".into());
    r.last_attempt_at = Some(NOW);
    let mut q = qs(vec![r]);
    q.pending.push(PendingDelivery {
        id: "dX".into(),
        snapshot: q.items[0].clone(),
    });
    clear_session_items(&mut q, "s");
    // …deck dies here; this is exactly what is on disk
    let on_disk = serde_json::to_string(&q).unwrap();
    let mut booted: QueueState = serde_json::from_str(&on_disk).unwrap();
    let notes = recover_interrupted(&mut booted);
    assert!(notes.is_empty(), "a cancelled delivery needs no decision");
    assert!(booted.items.is_empty(), "nothing revived");
    assert!(booted.pending.is_empty());
    assert!(
        is_cancelled(&booted, "s"),
        "the tombstone survived the crash"
    );
    assert!(select_due(&booted, NOW + 100_000, 720, &HashMap::new()).is_empty());
    // repeated recovery still changes nothing
    let after = serde_json::to_string(&booted).unwrap();
    recover_interrupted(&mut booted);
    assert_eq!(serde_json::to_string(&booted).unwrap(), after);
}

#[test]
fn deleting_a_project_clears_every_one_of_its_sessions_at_once() {
    let mk = |id: &str, session: &str| {
        let mut i = qi(id, "at");
        i.session = session.into();
        i.at = Some(NOW);
        i
    };
    let mut q = qs(vec![
        mk("a", "p-one"),
        mk("b", "p-two"),
        mk("c", "p-two"),
        mk("keep", "other-project"),
    ]);
    clear_sessions(&mut q, &["p-one".to_string(), "p-two".to_string()]);
    let left: Vec<&str> = q.items.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(left, ["keep"], "other projects untouched");
    assert!(is_cancelled(&q, "p-one") && is_cancelled(&q, "p-two"));
    assert!(!is_cancelled(&q, "other-project"));
    assert_eq!(ids(&select_due(&q, NOW, 720, &HashMap::new())), ["keep"]);
}

#[test]
fn a_deleted_session_never_reaches_the_send_hook() {
    let qm = Mutex::new(qs(vec![due_at("a", "s")]));
    with_queue(&qm, &ok_persist, |q| {
        clear_session_items(q, "s");
        Ok(())
    })
    .unwrap();
    let res = send_test(
        &qm,
        &AtomicBool::new(false),
        "s",
        720,
        &HashMap::new(),
        &|_: &QueueItem| panic!("a deleted card must never start or feed a session"),
        &ok_persist,
    );
    assert_eq!(res, SendResult::Nothing);
}

#[test]
fn a_card_deleted_mid_send_leaves_no_session_behind() {
    // the worker holds no lock while injecting, so the delete can land
    // between the intent and the send — fire_item may just have started
    // the session, which must not outlive the card
    let qm = Mutex::new(qs(vec![due_at("a", "s")]));
    let killed: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let fire = |_: &QueueItem| {
        // the user deletes the card at exactly this instant
        with_queue(&qm, &ok_persist, |q| {
            clear_session_items(q, "s");
            Ok(())
        })
        .unwrap();
        Ok(())
    };
    let kill = |s: &str| killed.lock().unwrap().push(s.to_string());
    let res = send_one(
        &qm,
        &AtomicBool::new(false),
        "s",
        720,
        &HashMap::new(),
        &SendHooks {
            fire: &fire,
            persist: &ok_persist,
            kill: &kill,
        },
    );
    assert!(matches!(res, SendResult::Sent { .. }));
    assert_eq!(killed.lock().unwrap().as_slice(), ["s"], "session reaped");
    let q = qm.lock().unwrap();
    assert_eq!(q.deliveries.len(), 1, "the delivery is still audited");
    assert!(q.items.is_empty() && q.pending.is_empty());
}

#[test]
fn scheduling_for_a_session_again_clears_its_tombstone() {
    // a NEW card that happens to reuse a name must schedule normally
    let mut q = qs(vec![]);
    clear_session_items(&mut q, "s");
    add_item(&mut q, add_args("s", "hello"), "hello".into()).unwrap();
    assert!(!is_cancelled(&q, "s"));
    let quiet: HashMap<String, u64> = [("s".into(), NOW - 400)].into();
    assert_eq!(select_due(&q, NOW, 720, &quiet).len(), 1);
}

#[test]
fn tombstones_are_capped() {
    let mut q = qs(vec![]);
    for k in 0..(MAX_TOMBSTONES + 5) {
        clear_session_items(&mut q, &format!("s{k}"));
    }
    assert_eq!(q.cancelled.len(), MAX_TOMBSTONES);
    assert!(is_cancelled(&q, &format!("s{}", MAX_TOMBSTONES + 4)));
    assert!(!is_cancelled(&q, "s0"), "oldest dropped first");
}

#[test]
fn finalize_without_item_still_audits_via_ledger() {
    let mut a = qi("a", "at");
    a.state = "firing".into();
    a.delivery = Some("d1".into());
    let mut q = qs(vec![a.clone()]);
    q.pending.push(PendingDelivery {
        id: "d1".into(),
        snapshot: q.items[0].clone(),
    });
    q.items.clear(); // the item vanished mid-flight
    finalize_delivery(&mut q, "a", "d1", NOW, false);
    assert_eq!(q.deliveries.len(), 1, "delivery audited from the snapshot");
    assert_eq!(q.deliveries[0].session, "s");
    assert_eq!(q.last_fired.get("s"), Some(&NOW), "session gap updated");
    assert!(q.pending.is_empty(), "ledger entry consumed");
    assert!(q.items.is_empty(), "no item resurrected, no steps spawned");
    // idempotent replay with the item still missing
    finalize_delivery(&mut q, "a", "d1", NOW, false);
    assert_eq!(q.deliveries.len(), 1);
}

#[test]
fn vanished_rule_delivery_spawns_no_steps() {
    let mut r = rule(300);
    r.steps = vec!["s2".into()];
    r.state = "firing".into();
    r.delivery = Some("d1".into());
    let mut q = qs(vec![r]);
    q.pending.push(PendingDelivery {
        id: "d1".into(),
        snapshot: q.items[0].clone(),
    });
    q.items.clear();
    finalize_delivery(&mut q, "t", "d1", NOW, false);
    assert!(q.items.is_empty(), "removed rule must not respawn steps");
    assert_eq!(q.deliveries.len(), 1);
}

#[test]
fn note_failed_drops_the_ledger_entry() {
    let mut a = qi("a", "at");
    a.state = "firing".into();
    a.delivery = Some("d1".into());
    a.attempts = 1;
    let mut q = qs(vec![a]);
    q.pending.push(PendingDelivery {
        id: "d1".into(),
        snapshot: q.items[0].clone(),
    });
    note_failed(&mut q, "a", "d1", "tmux send-keys failed");
    assert!(q.pending.is_empty(), "not-sent leaves nothing to recover");
    assert_eq!(q.items[0].state, "failed");
    assert!(q.items[0].delivery.is_none());
    assert!(q.deliveries.is_empty(), "a refused send is never audited");
}

#[test]
fn recovery_restores_orphaned_ledger_entries_as_ambiguous() {
    let mut a = qi("a", "at");
    a.state = "firing".into();
    a.delivery = Some("dX".into());
    a.last_attempt_at = Some(NOW - 5);
    let mut q = qs(vec![]);
    q.pending.push(PendingDelivery {
        id: "dX".into(),
        snapshot: a,
    });
    let notes = recover_interrupted(&mut q);
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("choose acknowledge or retry"));
    assert!(q.deliveries.is_empty());
    assert!(q.last_fired.is_empty());
    assert_eq!(q.items[0].state, "ambiguous");
    // repeated recovery changes nothing
    let snapshot = serde_json::to_string(&q).unwrap();
    recover_interrupted(&mut q);
    assert_eq!(serde_json::to_string(&q).unwrap(), snapshot);
}

#[test]
fn boot_persist_failure_keeps_ambiguous_memory_dirty_until_flush() {
    let mut firing = due_at("a", "s");
    firing.state = "firing".into();
    firing.delivery = Some("d1".into());
    let loaded = qs(vec![firing]);
    let fail = AtomicBool::new(true);
    let disk = Mutex::new(String::new());
    let persist = |q: &QueueState| {
        if fail.load(AtomicOrdering::Relaxed) {
            Err("disk unavailable".into())
        } else {
            *disk.lock().unwrap() = serde_json::to_string(q).unwrap();
            Ok(())
        }
    };
    let queues = boot_queues_with(loaded, &persist);
    {
        let q = queues.q.lock().unwrap();
        assert_eq!(q.items[0].state, "ambiguous");
        assert!(select_due(&q, NOW + 100_000, 720, &HashMap::new()).is_empty());
    }
    assert!(queues.dirty.load(AtomicOrdering::Relaxed));
    assert!(!flush_dirty(&queues.q, &queues.dirty, &persist));
    assert!(queues.dirty.load(AtomicOrdering::Relaxed));
    fail.store(false, AtomicOrdering::Relaxed);
    assert!(flush_dirty(&queues.q, &queues.dirty, &persist));
    assert!(!queues.dirty.load(AtomicOrdering::Relaxed));
    let saved: QueueState = serde_json::from_str(&disk.lock().unwrap()).unwrap();
    assert_eq!(saved.items[0].state, "ambiguous");
}

#[test]
fn orphan_ledger_boot_failure_is_immediately_decidable_and_ack_is_transactional() {
    let mut snapshot = due_at("a", "s");
    snapshot.state = "firing".into();
    snapshot.delivery = Some("d1".into());
    let mut loaded = qs(vec![]);
    loaded.pending.push(PendingDelivery {
        id: "d1".into(),
        snapshot,
    });
    let queues = boot_queues_with(loaded, &|_| Err("read only".into()));
    let before = serde_json::to_string(&*queues.q.lock().unwrap()).unwrap();
    assert!(before.contains("ambiguous"));
    assert!(
        with_queue(&queues.q, &|_| Err("still read only".into()), |q| {
            acknowledge_ambiguous(q, "a")
        })
        .is_err()
    );
    assert_eq!(
        serde_json::to_string(&*queues.q.lock().unwrap()).unwrap(),
        before
    );
}

// ---------- send_one: the full firing state machine with fakes ----------

fn ok_persist(_: &QueueState) -> Result<(), DeckError> {
    Ok(())
}

/// send_one with a no-op kill hook — keeps the state-machine tests about
/// the state machine. The kill hook has its own test below.
fn send_test(
    qm: &Mutex<QueueState>,
    dirty: &AtomicBool,
    session: &str,
    now_min: u32,
    activity: &HashMap<String, u64>,
    fire: &(dyn Fn(&QueueItem) -> Result<(), DeckError> + Sync),
    persist: &(dyn Fn(&QueueState) -> Result<(), DeckError> + Sync),
) -> SendResult {
    let kill = |_: &str| {};
    send_one(
        qm,
        dirty,
        session,
        now_min,
        activity,
        &SendHooks {
            fire,
            persist,
            kill: &kill,
        },
    )
}

fn due_at(id: &str, session: &str) -> QueueItem {
    let mut a = qi(id, "at");
    a.session = session.into();
    a.at = Some(1); // long past — always due against the real clock
    a
}

#[test]
fn send_one_success_runs_the_full_cycle() {
    use std::sync::atomic::{AtomicU32, Ordering};
    let qm = Mutex::new(qs(vec![due_at("a", "s")]));
    let fired = AtomicU32::new(0);
    let res = send_test(
        &qm,
        &AtomicBool::new(false),
        "s",
        720,
        &HashMap::new(),
        &|i: &QueueItem| {
            fired.fetch_add(1, Ordering::SeqCst);
            assert_eq!(i.text, "x", "worker sends the snapshot text");
            Ok(())
        },
        &ok_persist,
    );
    assert_eq!(
        res,
        SendResult::Sent {
            session: "s".into()
        }
    );
    assert_eq!(fired.load(Ordering::SeqCst), 1);
    let q = qm.lock().unwrap();
    assert!(q.items.is_empty(), "once-item consumed");
    assert_eq!(q.deliveries.len(), 1);
    assert!(!q.deliveries[0].assumed);
    assert!(q.pending.is_empty());
    assert!(q.last_fired.contains_key("s"));
}

#[test]
fn send_one_failure_is_retryable_and_never_audited() {
    let qm = Mutex::new(qs(vec![due_at("a", "s")]));
    let res = send_test(
        &qm,
        &AtomicBool::new(false),
        "s",
        720,
        &HashMap::new(),
        &|_: &QueueItem| Err("injection refused".into()),
        &ok_persist,
    );
    assert_eq!(
        res,
        SendResult::Failed {
            session: "s".into(),
            gave_up: false
        }
    );
    let q = qm.lock().unwrap();
    assert_eq!(q.items[0].state, "failed");
    assert_eq!(q.items[0].attempts, 1);
    assert!(q.items[0].delivery.is_none());
    assert!(q.pending.is_empty());
    assert!(q.deliveries.is_empty(), "refused send is not a delivery");
    assert!(q.last_fired.is_empty(), "no gap update for a refused send");
}

#[test]
fn retry_after_failure_sends_the_full_text_exactly_once() {
    use std::sync::atomic::{AtomicU32, Ordering};
    let qm = Mutex::new(qs(vec![due_at("a", "s")]));
    let _ = send_test(
        &qm,
        &AtomicBool::new(false),
        "s",
        720,
        &HashMap::new(),
        &|_: &QueueItem| Err("refused".into()),
        &ok_persist,
    );
    qm.lock().unwrap().items[0].last_attempt_at = Some(0); // backoff elapsed
    let sent = AtomicU32::new(0);
    let res = send_test(
        &qm,
        &AtomicBool::new(false),
        "s",
        720,
        &HashMap::new(),
        &|i: &QueueItem| {
            sent.fetch_add(1, Ordering::SeqCst);
            assert_eq!(i.text, "x", "retry re-sends the WHOLE text once");
            Ok(())
        },
        &ok_persist,
    );
    assert!(matches!(res, SendResult::Sent { .. }));
    assert_eq!(sent.load(Ordering::SeqCst), 1);
    assert_eq!(qm.lock().unwrap().deliveries.len(), 1, "one audit total");
}

#[test]
fn send_one_persist_failure_rolls_back_the_intent() {
    let qm = Mutex::new(qs(vec![due_at("a", "s")]));
    let res = send_test(
        &qm,
        &AtomicBool::new(false),
        "s",
        720,
        &HashMap::new(),
        &|_: &QueueItem| panic!("must not inject when the intent never hit disk"),
        &|_: &QueueState| Err("disk full".into()),
    );
    assert_eq!(res, SendResult::NotPersisted);
    let q = qm.lock().unwrap();
    assert_eq!(q.items[0].state, "pending");
    assert_eq!(q.items[0].attempts, 0);
    assert!(q.items[0].delivery.is_none());
    assert!(q.pending.is_empty());
}

#[test]
fn send_one_honors_a_pause_that_landed_after_the_tick() {
    let mut a = due_at("a", "s");
    a.paused = true; // user paused between candidate pass and worker
    let qm = Mutex::new(qs(vec![a]));
    let res = send_test(
        &qm,
        &AtomicBool::new(false),
        "s",
        720,
        &HashMap::new(),
        &|_: &QueueItem| panic!("paused item must not fire"),
        &ok_persist,
    );
    assert_eq!(res, SendResult::Nothing);
}

fn pane(n: u32) -> PaneIdentity {
    PaneIdentity {
        server_pid: 90 + n,
        session_id: format!("${n}"),
        window_id: format!("@{n}"),
        pane_id: format!("%{n}"),
        pane_pid: 100 + n,
    }
}

fn probe_result(status: ContextStatus, code: ContextCode, identity: u32) -> ProbeResult {
    ProbeResult {
        status,
        code,
        identity: Some(pane(identity)),
        current_process: Some("codex".into()),
    }
}

fn send_safe_test(
    qm: &Mutex<QueueState>,
    fire: &(dyn Fn(&QueueItem) -> Result<(), DeckError> + Sync),
    prepare: &(dyn Fn(&QueueItem, &dyn Fn() -> bool) -> ProbeResult + Sync),
    final_probe: &(dyn Fn(&QueueItem) -> ProbeResult + Sync),
) -> SendResult {
    let kill = |_: &str| {};
    send_one_safe(
        qm,
        &AtomicBool::new(false),
        "s",
        720,
        &HashMap::new(),
        &SendHooks {
            fire,
            persist: &ok_persist,
            kill: &kill,
        },
        &ContextHooks {
            prepare,
            final_probe,
        },
    )
}

#[test]
fn safe_ready_context_sends_exactly_once() {
    use std::sync::atomic::{AtomicU32, Ordering};
    let qm = Mutex::new(qs(vec![due_at("a", "s")]));
    let sends = AtomicU32::new(0);
    let ready = |_: &QueueItem, _: &dyn Fn() -> bool| {
        probe_result(ContextStatus::Ready, ContextCode::ProcessMatched, 1)
    };
    let final_ready = |i: &QueueItem| {
        assert_eq!(i.binding.as_ref(), Some(&pane(1)));
        probe_result(ContextStatus::Ready, ContextCode::ProcessMatched, 1)
    };
    let result = send_safe_test(
        &qm,
        &|i: &QueueItem| {
            assert_eq!(i.binding.as_ref(), Some(&pane(1)));
            sends.fetch_add(1, Ordering::SeqCst);
            Ok(())
        },
        &ready,
        &final_ready,
    );
    assert!(matches!(result, SendResult::Sent { .. }));
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    assert_eq!(qm.lock().unwrap().deliveries.len(), 1);
}

#[test]
fn unsafe_contexts_block_without_attempt_or_ledger() {
    for (status, code) in [
        (
            ContextStatus::ForegroundDifferent,
            ContextCode::ForegroundDifferent,
        ),
        (ContextStatus::Unavailable, ContextCode::ProbeFailed),
    ] {
        let qm = Mutex::new(qs(vec![due_at("a", "s")]));
        let prepare = move |_: &QueueItem, _: &dyn Fn() -> bool| probe_result(status, code, 1);
        let result = send_safe_test(
            &qm,
            &|_: &QueueItem| panic!("blocked context must never send"),
            &prepare,
            &|_: &QueueItem| panic!("blocked context has no final probe"),
        );
        assert!(matches!(result, SendResult::Blocked { status: s, .. } if s == status));
        let q = qm.lock().unwrap();
        assert_eq!(q.items[0].attempts, 0);
        assert_eq!(q.items[0].state, "pending");
        assert!(q.pending.is_empty() && q.deliveries.is_empty());
        assert_eq!(q.items[0].last_context.as_ref().unwrap().status, status);
    }
}

/// An upgrade, a tmux crash or a reboot replaces the whole server, so the
/// card's pane comes back under the same deck-owned name with an entirely
/// new generation. That stale binding is adopted from the readiness probe
/// and used for the atomic paste guard — it never blocks delivery.
#[test]
fn a_new_tmux_generation_under_the_same_name_is_adopted_not_blocked() {
    use std::sync::atomic::{AtomicU32, Ordering};
    let mut stale = due_at("a", "s");
    stale.binding = Some(pane(1));
    let qm = Mutex::new(qs(vec![stale]));
    let sends = AtomicU32::new(0);
    let result = send_safe_test(
        &qm,
        &|i: &QueueItem| {
            assert_eq!(i.binding.as_ref(), Some(&pane(2)));
            sends.fetch_add(1, Ordering::SeqCst);
            Ok(())
        },
        &|_: &QueueItem, _: &dyn Fn() -> bool| {
            probe_result(ContextStatus::Ready, ContextCode::ProcessMatched, 2)
        },
        &|i: &QueueItem| {
            assert_eq!(i.binding.as_ref(), Some(&pane(2)));
            probe_result(ContextStatus::Ready, ContextCode::ProcessMatched, 2)
        },
    );
    assert!(matches!(result, SendResult::Sent { .. }));
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    let q = qm.lock().unwrap();
    assert_eq!(q.items.len(), 0);
    assert_eq!(q.deliveries.len(), 1);
}

#[test]
fn replacement_between_probe_and_send_is_rejected_without_attempt() {
    let qm = Mutex::new(qs(vec![due_at("a", "s")]));
    let result = send_safe_test(
        &qm,
        &|_: &QueueItem| panic!("replacement must never receive input"),
        &|_: &QueueItem, _: &dyn Fn() -> bool| {
            probe_result(ContextStatus::Ready, ContextCode::ProcessMatched, 1)
        },
        &|_: &QueueItem| {
            probe_result(
                ContextStatus::SessionReplaced,
                ContextCode::IdentityChanged,
                2,
            )
        },
    );
    assert!(matches!(
        result,
        SendResult::Blocked {
            status: ContextStatus::SessionReplaced,
            ..
        }
    ));
    let q = qm.lock().unwrap();
    assert_eq!(q.items[0].attempts, 0);
    assert!(q.pending.is_empty());
}

#[test]
fn foreground_change_between_probe_and_send_is_rejected_without_attempt() {
    let qm = Mutex::new(qs(vec![due_at("a", "s")]));
    let result = send_safe_test(
        &qm,
        &|_: &QueueItem| panic!("changed foreground must never receive input"),
        &|_: &QueueItem, _: &dyn Fn() -> bool| {
            probe_result(ContextStatus::Ready, ContextCode::ProcessMatched, 1)
        },
        &|_: &QueueItem| {
            probe_result(
                ContextStatus::ForegroundDifferent,
                ContextCode::ForegroundDifferent,
                1,
            )
        },
    );
    assert!(matches!(
        result,
        SendResult::Blocked {
            status: ContextStatus::ForegroundDifferent,
            ..
        }
    ));
    let q = qm.lock().unwrap();
    assert_eq!(q.items[0].attempts, 0);
    assert!(q.pending.is_empty());
}

#[test]
fn pause_edit_and_delete_during_probe_cancel_the_worker() {
    for action in ["pause", "edit", "delete"] {
        let qm = Mutex::new(qs(vec![due_at("a", "s")]));
        let prepare = |_: &QueueItem, _: &dyn Fn() -> bool| {
            let mut q = qm.lock().unwrap();
            match action {
                "pause" => q.items[0].paused = true,
                "edit" => {
                    q.items[0].text = "changed".into();
                    q.items[0].revision += 1;
                }
                "delete" => clear_session_items(&mut q, "s"),
                _ => unreachable!(),
            }
            probe_result(ContextStatus::Ready, ContextCode::ProcessMatched, 1)
        };
        let result = send_safe_test(
            &qm,
            &|_: &QueueItem| panic!("stale worker must not send"),
            &prepare,
            &|_: &QueueItem| panic!("stale worker has no final probe"),
        );
        assert_eq!(result, SendResult::Nothing, "{action}");
        assert!(qm
            .lock()
            .unwrap()
            .items
            .first()
            .is_none_or(|i| i.attempts == 0));
    }
}

#[test]
fn delete_during_probe_reaps_a_session_the_worker_may_have_started() {
    use std::sync::atomic::{AtomicU32, Ordering};

    let qm = Mutex::new(qs(vec![due_at("a", "s")]));
    let kills = AtomicU32::new(0);
    let kill = |session: &str| {
        assert_eq!(session, "s");
        kills.fetch_add(1, Ordering::SeqCst);
    };
    let result = send_one_safe(
        &qm,
        &AtomicBool::new(false),
        "s",
        720,
        &HashMap::new(),
        &SendHooks {
            fire: &|_: &QueueItem| panic!("deleted prompt must not send"),
            persist: &ok_persist,
            kill: &kill,
        },
        &ContextHooks {
            prepare: &|_: &QueueItem, _: &dyn Fn() -> bool| {
                clear_session_items(&mut qm.lock().unwrap(), "s");
                probe_result(
                    ContextStatus::Unavailable,
                    ContextCode::CancelledOrRevised,
                    1,
                )
            },
            final_probe: &|_: &QueueItem| panic!("deleted prompt has no final probe"),
        },
    );
    assert_eq!(result, SendResult::Nothing);
    assert_eq!(kills.load(Ordering::SeqCst), 1);
}

#[test]
fn startup_poll_succeeds_times_out_and_detects_replacement_deterministically() {
    let mut calls = 0;
    let success = poll_readiness(
        3,
        &|| false,
        &mut |_| {
            calls += 1;
            if calls == 3 {
                probe_result(ContextStatus::Ready, ContextCode::ProcessMatched, 1)
            } else {
                probe_result(
                    ContextStatus::ForegroundDifferent,
                    ContextCode::ForegroundDifferent,
                    1,
                )
            }
        },
        &mut || {},
    );
    assert!(success.is_ready());
    assert_eq!(calls, 3);

    let timeout = poll_readiness(
        2,
        &|| false,
        &mut |_| {
            probe_result(
                ContextStatus::ForegroundDifferent,
                ContextCode::ForegroundDifferent,
                1,
            )
        },
        &mut || {},
    );
    assert_eq!(timeout.status, ContextStatus::ForegroundDifferent);
    assert_eq!(timeout.code, ContextCode::StartupTimeout);

    let mut calls = 0;
    let replaced = poll_readiness(
        4,
        &|| false,
        &mut |bound| {
            calls += 1;
            if bound.is_some() {
                probe_result(
                    ContextStatus::SessionReplaced,
                    ContextCode::IdentityChanged,
                    2,
                )
            } else {
                probe_result(
                    ContextStatus::ForegroundDifferent,
                    ContextCode::ForegroundDifferent,
                    1,
                )
            }
        },
        &mut || {},
    );
    assert_eq!(replaced.status, ContextStatus::SessionReplaced);
    assert_eq!(calls, 2);
}

#[test]
fn one_sessions_context_wait_does_not_block_another_session() {
    use std::sync::Barrier;
    let qm = Mutex::new(qs(vec![due_at("a", "slow"), due_at("b", "fast")]));
    let entered = Barrier::new(2);
    let release = Barrier::new(2);
    std::thread::scope(|scope| {
        let qm_ref = &qm;
        let (entered_ref, release_ref) = (&entered, &release);
        scope.spawn(move || {
            let kill = |_: &str| {};
            let result = send_one_safe(
                qm_ref,
                &AtomicBool::new(false),
                "slow",
                720,
                &HashMap::new(),
                &SendHooks {
                    fire: &|_: &QueueItem| Ok(()),
                    persist: &ok_persist,
                    kill: &kill,
                },
                &ContextHooks {
                    prepare: &|_: &QueueItem, _: &dyn Fn() -> bool| {
                        entered_ref.wait();
                        release_ref.wait();
                        probe_result(ContextStatus::Ready, ContextCode::ProcessMatched, 1)
                    },
                    final_probe: &|_: &QueueItem| {
                        probe_result(ContextStatus::Ready, ContextCode::ProcessMatched, 1)
                    },
                },
            );
            assert!(matches!(result, SendResult::Sent { .. }));
        });
        entered.wait();
        let kill = |_: &str| {};
        let fast = send_one_safe(
            &qm,
            &AtomicBool::new(false),
            "fast",
            720,
            &HashMap::new(),
            &SendHooks {
                fire: &|_: &QueueItem| Ok(()),
                persist: &ok_persist,
                kill: &kill,
            },
            &ContextHooks {
                prepare: &|_: &QueueItem, _: &dyn Fn() -> bool| {
                    probe_result(ContextStatus::Ready, ContextCode::ProcessMatched, 2)
                },
                final_probe: &|_: &QueueItem| {
                    probe_result(ContextStatus::Ready, ContextCode::ProcessMatched, 2)
                },
            },
        );
        assert!(matches!(fast, SendResult::Sent { .. }));
        assert!(!qm.lock().unwrap().items.iter().any(|i| i.session == "fast"));
        release.wait();
    });
    assert_eq!(qm.lock().unwrap().deliveries.len(), 2);
}

#[test]
fn legacy_policy_variants_are_ignored_and_cleaned() {
    for policy in ["agent-ready", "foreground-match", "force-generic"] {
        let mut value = serde_json::to_value(qs(vec![due_at("a", "s")])).unwrap();
        let item = value["items"][0].as_object_mut().unwrap();
        item.remove("expected_process");
        item.insert("safety_policy".into(), serde_json::json!(policy));
        item.insert("expected_agent".into(), serde_json::json!("codex"));
        item.insert(
            "last_context".into(),
            serde_json::json!({"status":"working","code":"hook-working","checked_at":1}),
        );
        item.insert("cmd".into(), serde_json::json!("codex --full-auto"));
        let mut loaded: QueueState = serde_json::from_value(value).unwrap();
        migrate_context(&mut loaded);
        let item = &loaded.items[0];
        assert_eq!(item.expected_process.as_deref(), Some("codex"), "{policy}");
        assert!(item.last_context.is_none(), "{policy}");
        let saved = serde_json::to_string(&loaded).unwrap();
        assert!(!saved.contains("safety_policy"), "{policy}");
        assert!(!saved.contains("expected_agent"), "{policy}");
        assert!(!saved.contains("hook-working"), "{policy}");
    }
}

#[test]
fn manual_now_bypasses_only_time_not_ordering_or_gap() {
    let mut future = due_at("future", "s");
    future.at = Some(u64::MAX);
    let mut q = qs(vec![future]);
    assert!(select_for_session(&q, "s", NOW, 720, &HashMap::new()).is_none());
    assert_eq!(
        select_requested(&q, "s", "future", NOW).unwrap().id,
        "future"
    );
    let mut tail = qi("tail", "chain");
    tail.group = Some("g".into());
    tail.seq = Some(2);
    let mut head = qi("head", "chain");
    head.group = Some("g".into());
    head.seq = Some(1);
    q.items.extend([head, tail]);
    assert!(select_requested(&q, "s", "tail", NOW).is_none());
    q.last_fired.insert("s".into(), NOW - 1);
    assert!(select_requested(&q, "s", "future", NOW).is_none());
}

#[test]
fn a_firing_item_is_never_selected_again() {
    let mut a = due_at("a", "s");
    a.state = "firing".into();
    let q = qs(vec![a]);
    assert!(
        select_for_session(&q, "s", NOW, 720, &HashMap::new()).is_none(),
        "a second worker on the same session finds nothing"
    );
}

#[test]
fn session_claim_is_exclusive_and_releasable() {
    let busy = Mutex::new(HashSet::new());
    assert!(claim_session(&busy, "s"));
    assert!(!claim_session(&busy, "s"), "second worker refused");
    assert!(claim_session(&busy, "other"), "other sessions independent");
    release_session(&busy, "s");
    assert!(claim_session(&busy, "s"), "released slot reusable");
}

/// A slow send on one session must not delay another session — proven
/// with barriers (deterministic sync points), not sleeps.
#[test]
fn sessions_progress_independently_during_a_slow_send() {
    use std::sync::Barrier;
    let qm = Mutex::new(qs(vec![due_at("a", "slow"), due_at("b", "fast")]));
    let entered = Barrier::new(2); // slow worker is inside fire()
    let release = Barrier::new(2); // let the slow send finish
    std::thread::scope(|s| {
        let qref = &qm;
        let (er, rl) = (&entered, &release);
        s.spawn(move || {
            let res = send_test(
                qref,
                &AtomicBool::new(false),
                "slow",
                720,
                &HashMap::new(),
                &|_: &QueueItem| {
                    er.wait(); // signal: mid-send, queue lock NOT held
                    rl.wait(); // block until the main thread saw "fast" done
                    Ok(())
                },
                &ok_persist,
            );
            assert!(matches!(res, SendResult::Sent { .. }));
        });
        entered.wait();
        // while "slow" is stalled inside its injection, "fast" completes
        let res = send_test(
            &qm,
            &AtomicBool::new(false),
            "fast",
            720,
            &HashMap::new(),
            &|_: &QueueItem| Ok(()),
            &ok_persist,
        );
        assert!(matches!(res, SendResult::Sent { .. }));
        {
            let q = qm.lock().unwrap();
            assert!(
                !q.items.iter().any(|i| i.session == "fast"),
                "fast session progressed while slow was mid-send"
            );
            assert!(
                q.items.iter().any(|i| i.session == "slow"),
                "slow send still in flight"
            );
        }
        release.wait();
    });
    let q = qm.lock().unwrap();
    assert_eq!(q.deliveries.len(), 2, "both sessions delivered");
    assert!(q.pending.is_empty());
}

// ---------- round 4: persist-then-commit transactions ----------

/// A persist that can be switched to failing, recording every state it
/// was asked to write (the "disk").
struct FakeDisk {
    fail: AtomicBool,
    writes: Mutex<Vec<String>>,
}
impl FakeDisk {
    fn new(initial: &QueueState) -> Self {
        FakeDisk {
            fail: AtomicBool::new(false),
            writes: Mutex::new(vec![serde_json::to_string(initial).unwrap()]),
        }
    }
    fn persist(&self, q: &QueueState) -> Result<(), DeckError> {
        if self.fail.load(AtomicOrdering::Relaxed) {
            return Err("No space left on device (os error 28)".into());
        }
        self.writes
            .lock()
            .unwrap()
            .push(serde_json::to_string(q).unwrap());
        Ok(())
    }
    /// what a fresh deck would load right now
    fn on_disk(&self) -> String {
        self.writes.lock().unwrap().last().unwrap().clone()
    }
}

fn add_args(session: &str, text: &str) -> QueueAddArgs {
    QueueAddArgs {
        session: session.into(),
        card_id: format!("card-{session}"),
        dir: String::new(),
        cmd: String::new(),
        text: text.into(),
        mode: "chain".into(),
        at: None,
        every: None,
        win_from: None,
        win_to: None,
        until_n: None,
        until_at: None,
        steps: None,
        tpl: None,
        tpl_idx: None,
        tpl_total: None,
    }
}

/// Every user-driven mutation, run twice: once against a healthy disk
/// (change visible in memory AND on disk) and once against a failing one
/// (error returned, memory byte-identical, disk untouched).
#[test]
fn every_queue_mutation_is_all_or_nothing() {
    let base = || {
        let mut a = qi("a", "at");
        a.at = Some(NOW - 1);
        let mut b = qi("b", "chain");
        b.text = "second".into();
        let mut f = qi("f", "chain");
        f.state = "failed".into();
        f.attempts = MAX_ATTEMPTS;
        let mut other = qi("o", "at");
        other.session = "other".into();
        qs(vec![a, b, f, other])
    };
    type Mutation = (&'static str, fn(&mut QueueState) -> Result<(), DeckError>);
    let mutations: Vec<Mutation> = vec![
        ("add", |q| {
            add_item(q, add_args("s", "fresh"), "fresh".into())
        }),
        ("update", |q| update_text(q, "a", "edited".into())),
        ("remove", |q| remove_item(q, "a").map(|_| ())),
        ("pause", |q| pause_item(q, "a", true)),
        ("retry", |q| retry_item(q, "f")),
        ("skip", |q| remove_item(q, "f").map(|_| ())),
        ("clear-session", |q| {
            clear_session_items(q, "s");
            Ok(())
        }),
        ("expiry-purge", |q| {
            q.items[0].mode = "every".into();
            q.items[0].until_at = Some(NOW - 1);
            purge_expired(q, NOW);
            Ok(())
        }),
    ];
    for (name, mutate) in mutations {
        // healthy disk: the change lands in memory and on disk together
        let qm = Mutex::new(base());
        let disk = FakeDisk::new(&base());
        let before = serde_json::to_string(&*qm.lock().unwrap()).unwrap();
        with_queue(&qm, &|q| disk.persist(q), mutate).unwrap_or_else(|e| panic!("{name}: {e}"));
        let after = serde_json::to_string(&*qm.lock().unwrap()).unwrap();
        assert_ne!(before, after, "{name}: mutation had no effect");
        assert_eq!(after, disk.on_disk(), "{name}: memory and disk agree");

        // failing disk: same mutation, nothing changes anywhere
        let qm = Mutex::new(base());
        let disk = FakeDisk::new(&base());
        disk.fail.store(true, AtomicOrdering::Relaxed);
        let disk_before = disk.on_disk();
        let err = with_queue(&qm, &|q| disk.persist(q), mutate)
            .expect_err(&format!("{name}: failed save must be an error"));
        assert_eq!(err.code(), "disk-full", "{name}: {err}");
        assert_eq!(
            serde_json::to_string(&*qm.lock().unwrap()).unwrap(),
            before,
            "{name}: shared memory must be byte-identical after a failed save"
        );
        assert_eq!(disk.on_disk(), disk_before, "{name}: disk untouched");
    }
}

#[test]
fn a_rejected_mutation_never_reaches_the_disk() {
    // the firing contract rejects before any write is attempted
    let mut a = qi("a", "at");
    a.state = "firing".into();
    let qm = Mutex::new(qs(vec![a]));
    let disk = FakeDisk::new(&qm.lock().unwrap().clone());
    let writes0 = disk.writes.lock().unwrap().len();
    assert!(with_queue(&qm, &|q| disk.persist(q), |q| update_text(
        q,
        "a",
        "edited".into()
    ))
    .is_err());
    assert_eq!(disk.writes.lock().unwrap().len(), writes0, "no save tried");
    assert_eq!(qm.lock().unwrap().items[0].text, "x");
}

#[test]
fn a_failed_retry_save_keeps_the_item_out_of_the_candidate_set() {
    let mut f = qi("f", "chain");
    f.state = "failed".into();
    f.attempts = MAX_ATTEMPTS;
    let qm = Mutex::new(qs(vec![f]));
    let disk = FakeDisk::new(&qm.lock().unwrap().clone());
    disk.fail.store(true, AtomicOrdering::Relaxed);
    assert!(with_queue(&qm, &|q| disk.persist(q), |q| retry_item(q, "f")).is_err());
    let q = qm.lock().unwrap();
    assert!(item_dead(&q.items[0]), "still dead in memory");
    let quiet: HashMap<String, u64> = [("s".into(), NOW - 400)].into();
    assert!(
        select_due(&q, NOW, 720, &quiet).is_empty(),
        "a retry the user was told failed must not re-enter the schedule"
    );
}

#[test]
fn a_failed_pre_fire_save_sends_nothing_and_changes_nothing() {
    let qm = Mutex::new(qs(vec![due_at("a", "s")]));
    let before = serde_json::to_string(&*qm.lock().unwrap()).unwrap();
    let dirty = AtomicBool::new(false);
    let res = send_test(
        &qm,
        &dirty,
        "s",
        720,
        &HashMap::new(),
        &|_: &QueueItem| panic!("must not inject when the intent never hit disk"),
        &|_: &QueueState| Err("No space left on device".into()),
    );
    assert_eq!(res, SendResult::NotPersisted);
    assert_eq!(
        serde_json::to_string(&*qm.lock().unwrap()).unwrap(),
        before,
        "intent rolled back completely"
    );
    assert!(!dirty.load(AtomicOrdering::Relaxed), "nothing owed to disk");
}

#[test]
fn a_failed_post_send_save_keeps_memory_authoritative_and_retries() {
    // the prompt really went out: memory MUST take the finalized state
    // (automatic re-sending could duplicate it), and the write is retried
    let qm = Mutex::new(qs(vec![due_at("a", "s")]));
    let disk = FakeDisk::new(&qm.lock().unwrap().clone());
    let dirty = AtomicBool::new(false);
    let persist = |q: &QueueState| disk.persist(q);
    let res = send_test(
        &qm,
        &dirty,
        "s",
        720,
        &HashMap::new(),
        &|_: &QueueItem| {
            disk.fail.store(true, AtomicOrdering::Relaxed); // disk dies mid-send
            Ok(())
        },
        &persist,
    );
    assert!(matches!(res, SendResult::Sent { .. }));
    assert!(dirty.load(AtomicOrdering::Relaxed), "write still owed");
    {
        let q = qm.lock().unwrap();
        assert!(q.items.is_empty(), "delivery finalized in memory");
        assert_eq!(q.deliveries.len(), 1);
        assert!(q.pending.is_empty());
    }
    // still failing: nothing changes, the flag stays up
    assert!(!flush_dirty(&qm, &dirty, &persist));
    assert!(dirty.load(AtomicOrdering::Relaxed));
    // disk recovers: the retry lands and the flag clears
    disk.fail.store(false, AtomicOrdering::Relaxed);
    assert!(flush_dirty(&qm, &dirty, &persist));
    assert!(!dirty.load(AtomicOrdering::Relaxed));
    assert_eq!(
        disk.on_disk(),
        serde_json::to_string(&*qm.lock().unwrap()).unwrap()
    );
    assert!(!flush_dirty(&qm, &dirty, &persist), "nothing owed anymore");
}

#[test]
fn a_definitively_refused_send_that_cannot_be_saved_is_retried_not_forgotten() {
    let qm = Mutex::new(qs(vec![due_at("a", "s")]));
    let disk = FakeDisk::new(&qm.lock().unwrap().clone());
    let dirty = AtomicBool::new(false);
    let persist = |q: &QueueState| disk.persist(q);
    let res = send_test(
        &qm,
        &dirty,
        "s",
        720,
        &HashMap::new(),
        &|_: &QueueItem| {
            disk.fail.store(true, AtomicOrdering::Relaxed);
            Err("tmux send-keys failed: can't find session: x".into())
        },
        &persist,
    );
    assert!(matches!(res, SendResult::Failed { .. }));
    {
        let q = qm.lock().unwrap();
        assert_eq!(q.items[0].state, "failed", "not sent — retryable");
        assert!(q.pending.is_empty(), "no delivery to recover");
        assert!(q.deliveries.is_empty(), "a refused send is never audited");
    }
    assert!(dirty.load(AtomicOrdering::Relaxed));
    disk.fail.store(false, AtomicOrdering::Relaxed);
    assert!(flush_dirty(&qm, &dirty, &persist));
    // the persisted state carries the "not sent" truth, so a restart
    // resumes the retry instead of counting the prompt as delivered
    let recovered: QueueState = serde_json::from_str(&disk.on_disk()).unwrap();
    assert_eq!(recovered.items[0].state, "failed");
    assert!(recovered.pending.is_empty());
    assert!(recovered.deliveries.is_empty());
}

#[test]
fn crash_before_dirty_flush_recovers_old_firing_disk_as_ambiguous() {
    // Exact crash window from the regression: intent save succeeds, the
    // tmux injection explicitly refuses, the post-failure save fails,
    // then the process disappears WITHOUT flush_dirty.
    let initial = qs(vec![due_at("a", "s")]);
    let qm = Mutex::new(initial.clone());
    let disk = FakeDisk::new(&initial);
    let dirty = AtomicBool::new(false);
    let persist = |q: &QueueState| disk.persist(q);
    let res = send_test(
        &qm,
        &dirty,
        "s",
        720,
        &HashMap::new(),
        &|_: &QueueItem| {
            disk.fail.store(true, AtomicOrdering::Relaxed);
            Err("tmux send-keys refused".into())
        },
        &persist,
    );
    assert!(matches!(res, SendResult::Failed { .. }));
    assert!(dirty.load(AtomicOrdering::Relaxed));

    let mut restarted: QueueState = serde_json::from_str(&disk.on_disk()).unwrap();
    assert_eq!(restarted.items[0].state, "firing", "old disk is the intent");
    recover_interrupted(&mut restarted);
    assert_eq!(restarted.items[0].state, "ambiguous");
    assert!(restarted.deliveries.is_empty());
    assert!(select_due(&restarted, NOW + 100_000, 720, &HashMap::new()).is_empty());

    retry_item(&mut restarted, "a").unwrap();
    retry_item(&mut restarted, "a").unwrap();
    assert_eq!(restarted.items[0].state, "pending");
    assert!(restarted.pending.is_empty());
}

#[test]
fn ambiguous_acknowledgement_accounts_once_for_once_rule_and_template_chain() {
    let mut once = due_at("once", "once-session");
    once.state = "firing".into();
    once.delivery = Some("do".into());
    once.last_attempt_at = Some(NOW);

    let mut recurring = rule(300);
    recurring.id = "rule".into();
    recurring.session = "rule-session".into();
    recurring.until_n = Some(2);
    recurring.fired = 1;
    recurring.steps = vec!["step two".into(), "step three".into()];
    recurring.state = "firing".into();
    recurring.delivery = Some("dr".into());
    recurring.last_attempt_at = Some(NOW);

    let mut head = due_at("head", "chain-session");
    head.group = Some("g".into());
    head.seq = Some(1);
    head.state = "firing".into();
    head.delivery = Some("dh".into());
    head.last_attempt_at = Some(NOW);
    let mut tail = qi("tail", "chain");
    tail.session = "chain-session".into();
    tail.group = Some("g".into());
    tail.seq = Some(2);

    let mut q = qs(vec![once, recurring, head, tail]);
    recover_interrupted(&mut q);
    for id in ["once", "rule", "head"] {
        acknowledge_ambiguous(&mut q, id).unwrap();
        acknowledge_ambiguous(&mut q, id).unwrap();
    }
    assert!(!q
        .items
        .iter()
        .any(|i| i.id == "once" || i.id == "rule" || i.id == "head"));
    assert!(q.items.iter().any(|i| i.id == "tail"));
    assert_eq!(
        q.items
            .iter()
            .filter(|i| i.rule.as_deref() == Some("rule"))
            .count(),
        2
    );
    assert_eq!(q.deliveries.len(), 3);
    assert!(q.deliveries.iter().all(|d| d.assumed));
}

#[test]
fn window_plain_and_midnight_wrap() {
    // 08:00–18:00
    assert!(in_window(8 * 60, Some(480), Some(1080)));
    assert!(in_window(17 * 60 + 59, Some(480), Some(1080)));
    assert!(!in_window(18 * 60, Some(480), Some(1080)));
    assert!(!in_window(3 * 60, Some(480), Some(1080)));
    // 20:00–08:00 wraps midnight
    assert!(in_window(23 * 60, Some(1200), Some(480)));
    assert!(in_window(2 * 60, Some(1200), Some(480)));
    assert!(!in_window(12 * 60, Some(1200), Some(480)));
    // no / degenerate window = always
    assert!(in_window(0, None, None));
    assert!(in_window(700, Some(600), Some(600)));
}
