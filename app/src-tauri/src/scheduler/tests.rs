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
        operation_id: None,
        dir: String::new(),
        cmd: String::new(),
        text: "x".into(),
        mode: mode.into(),
        at: None,
        added: 0,
        quiet_secs: None,
        review_each: false,
        every: None,
        not_before: None,
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
        // a process-bound row; Codex-configured rows (which the Codex trust
        // gate applies to) are built explicitly by their own tests
        expected_process: Some("claude".into()),
        binding: None,
        last_context: None,
        revision: 0,
        review: None,
        external: false,
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
        reviews: Vec::new(),
        review_completed: HashSet::new(),
        operations: Vec::new(),
    };
    migrate_groups(&mut q);
    q
}

/// Session "s" last produced output at `activity`; no agent hook word. Its
/// Claude generation has already established an interaction (the generic
/// scheduling tests model an agent past its bootstrap; the
/// first-interaction gate has its own tests with `unestablished`).
fn seen(activity: u64) -> Observations {
    HashMap::from([(
        "s".to_string(),
        Observed {
            activity,
            claude_interaction: true,
            ..Observed::default()
        },
    )])
}

/// Session "s" quiet since `activity`, with the agent hook reporting `agent`
/// (so its Claude generation has an accepted interaction).
fn seen_agent(activity: u64, agent: &'static str) -> Observations {
    HashMap::from([(
        "s".to_string(),
        Observed {
            activity,
            agent: Some(agent),
            claude_interaction: true,
            ..Observed::default()
        },
    )])
}

/// Session "s" exists, quiet since `activity`, and its current foreground
/// generation has NO interaction evidence (a fresh or restarted agent, a
/// startup dialog, hooks off).
fn unestablished(activity: u64) -> Observations {
    HashMap::from([(
        "s".to_string(),
        Observed {
            activity,
            ..Observed::default()
        },
    )])
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
    let quiet = seen(NOW - 400);
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
    let quiet = seen(NOW - 400);
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
    let quiet = seen(NOW - 400);
    assert_eq!(ids(&select_due(&q, NOW, 720, &quiet)), ["c1"]);
    // recent activity → nothing
    let busy = seen(NOW - 10);
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
    let quiet = seen(NOW + 600 - 400);
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
    let quiet2 = seen(later + 300 - 400);
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

#[test]
fn a_named_list_is_joined_and_an_unknown_one_falls_back() {
    // two lists on one session: an "at" head each, the newest last
    let mut q = qs(vec![]);
    let mut first = add_args("s", "first");
    first.mode = "at".into();
    first.at = Some(NOW + 10);
    add_item(&mut q, first, "first".into()).unwrap();
    let mut second = add_args("s", "second");
    second.mode = "at".into();
    second.at = Some(NOW + 20);
    add_item(&mut q, second, "second".into()).unwrap();
    let first_group = q.items[0].group.clone().unwrap();
    let second_group = q.items[1].group.clone().unwrap();
    assert_ne!(first_group, second_group);
    // a row that names the OLDER list joins it, after its head
    let mut row = add_args("s", "row");
    row.mode = "chain".into();
    row.group = Some(first_group.clone());
    add_item(&mut q, row, "row".into()).unwrap();
    let joined = q.items.iter().find(|i| i.text == "row").unwrap();
    assert_eq!(joined.group.as_deref(), Some(first_group.as_str()));
    assert_eq!(joined.seq, Some(2));
    // a row naming a list this session does not have falls back to the
    // list of the most recently added row (the pre-list behaviour)
    let mut stray = add_args("s", "stray");
    stray.mode = "chain".into();
    stray.group = Some("nope".into());
    add_item(&mut q, stray, "stray".into()).unwrap();
    let stray = q.items.iter().find(|i| i.text == "stray").unwrap();
    assert_eq!(stray.group.as_deref(), Some(first_group.as_str()));
    assert_eq!(stray.seq, Some(3));
    let _ = second_group;
    // a list of another session is never joined, even by name
    let mut other = add_args("o", "other");
    other.mode = "chain".into();
    other.group = Some(first_group.clone());
    add_item(&mut q, other, "other".into()).unwrap();
    let other = q.items.iter().find(|i| i.text == "other").unwrap();
    assert_ne!(other.group.as_deref(), Some(first_group.as_str()));
    // the field is a chain-only field
    let mut timed = add_args("s", "timed");
    timed.mode = "at".into();
    timed.at = Some(NOW + 30);
    timed.group = Some(first_group);
    assert!(validate_add(&timed).is_err());
}

#[test]
fn a_rules_follow_up_rows_are_replaced_wholesale_and_only_on_a_rule() {
    let mut r = rule(300);
    r.steps = vec!["s2".into()];
    let mut q = qs(vec![r, qi("c", "chain")]);
    update_steps(&mut q, "t", vec!["a".into(), "b".into()]).unwrap();
    let rule_item = q.items.iter().find(|i| i.id == "t").unwrap();
    assert_eq!(rule_item.steps, vec!["a".to_string(), "b".to_string()]);
    assert_eq!(rule_item.revision, 1);
    update_steps(&mut q, "t", vec![]).unwrap();
    assert!(q
        .items
        .iter()
        .find(|i| i.id == "t")
        .unwrap()
        .steps
        .is_empty());
    assert!(
        update_steps(&mut q, "c", vec!["x".into()]).is_err(),
        "a one-shot row never holds embedded steps"
    );
    assert!(
        update_steps(&mut q, "missing", vec![]).is_ok(),
        "gone is a no-op"
    );
    q.items[0].state = "firing".into();
    assert!(update_steps(&mut q, "t", vec![]).is_err(), "never mid-send");
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
        operation_id: None,
        session: "s".into(),
        card_id: "card-s".into(),
        dir: String::new(),
        cmd: String::new(),
        text: "x".into(),
        mode: "at".into(),
        at: Some(NOW),
        quiet_secs: None,
        review_each: false,
        every: None,
        not_before: None,
        win_from: None,
        win_to: None,
        until_n: None,
        until_at: None,
        steps: None,
        tpl: None,
        tpl_idx: None,
        tpl_total: None,
        group: None,
        external_text: false,
        channel_path: false,
    };
    assert!(validate_add(&base()).is_ok());
    let mut a = base();
    a.at = None;
    assert_eq!(
        validate_add(&a).unwrap_err().kind(),
        crate::error::ErrorKind::Invalid,
        "at without a time is an invalid argument, not an anonymous failure"
    );
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
    // a start instant still ahead → not yet; reached → due
    r.until_at = None;
    r.not_before = Some(now + 1);
    assert!(!every_due(&r, now, 9 * 60));
    r.not_before = Some(now);
    assert!(every_due(&r, now, 9 * 60));
}

#[test]
fn chain_quiet_time_is_per_item() {
    let mut c = qi("c", "chain");
    c.quiet_secs = Some(30);
    let q = qs(vec![c]);
    let act = |ago: u64| seen(NOW - ago);
    assert!(select_due(&q, NOW, 720, &act(29)).is_empty());
    assert_eq!(ids(&select_due(&q, NOW, 720, &act(30))), ["c"]);
    // unset = the default
    let q = qs(vec![qi("d", "chain")]);
    assert!(select_due(&q, NOW, 720, &act(CHAIN_QUIET_SECS - 1)).is_empty());
    assert_eq!(
        ids(&select_due(&q, NOW, 720, &act(CHAIN_QUIET_SECS))),
        ["d"]
    );
}

#[test]
fn add_validation_covers_quiet_and_start() {
    let base = || QueueAddArgs {
        operation_id: None,
        session: "s".into(),
        card_id: "card-s".into(),
        dir: String::new(),
        cmd: String::new(),
        text: "x".into(),
        mode: "chain".into(),
        at: None,
        quiet_secs: None,
        review_each: false,
        every: None,
        not_before: None,
        win_from: None,
        win_to: None,
        until_n: None,
        until_at: None,
        steps: None,
        tpl: None,
        tpl_idx: None,
        tpl_total: None,
        group: None,
        external_text: false,
        channel_path: false,
    };
    let mut a = base();
    a.quiet_secs = Some(MIN_QUIET_SECS);
    assert!(validate_add(&a).is_ok());
    a.quiet_secs = Some(MIN_QUIET_SECS - 1);
    assert!(validate_add(&a).is_err(), "quiet below the floor");
    a.quiet_secs = Some(MAX_QUIET_SECS + 1);
    assert!(validate_add(&a).is_err(), "quiet above the ceiling");
    let mut a = base();
    a.mode = "at".into();
    a.at = Some(NOW);
    a.quiet_secs = Some(60);
    assert!(validate_add(&a).is_err(), "quiet time on a timed prompt");
    let mut a = base();
    a.mode = "every".into();
    a.every = Some(300);
    a.not_before = Some(NOW);
    a.until_at = Some(NOW + 1);
    assert!(validate_add(&a).is_ok());
    a.until_at = Some(NOW);
    assert!(validate_add(&a).is_err(), "stops before it starts");
    let mut a = base();
    a.not_before = Some(NOW);
    assert!(validate_add(&a).is_err(), "start instant on a chain item");
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
    let kill = |s: &str| killed.lock_or_recover().push(s.to_string());
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
    assert_eq!(killed.lock_or_recover().as_slice(), ["s"], "session reaped");
    let q = qm.lock_or_recover();
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
    let quiet = seen(NOW - 400);
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
            Err(DeckError::classified("disk unavailable"))
        } else {
            *disk.lock_or_recover() = serde_json::to_string(q).unwrap();
            Ok(())
        }
    };
    let queues = boot_queues_with(loaded, &persist);
    {
        let q = queues.q.lock_or_recover();
        assert_eq!(q.items[0].state, "ambiguous");
        assert!(select_due(&q, NOW + 100_000, 720, &HashMap::new()).is_empty());
    }
    assert!(queues.dirty.load(AtomicOrdering::Relaxed));
    assert!(!flush_dirty(&queues.q, &queues.dirty, &persist));
    assert!(queues.dirty.load(AtomicOrdering::Relaxed));
    fail.store(false, AtomicOrdering::Relaxed);
    assert!(flush_dirty(&queues.q, &queues.dirty, &persist));
    assert!(!queues.dirty.load(AtomicOrdering::Relaxed));
    let saved: QueueState = serde_json::from_str(&disk.lock_or_recover()).unwrap();
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
    let queues = boot_queues_with(loaded, &|_| Err(DeckError::classified("read only")));
    let before = serde_json::to_string(&*queues.q.lock_or_recover()).unwrap();
    assert!(before.contains("ambiguous"));
    assert!(with_queue(
        &queues.q,
        &|_| Err(DeckError::classified("still read only")),
        |q| { acknowledge_ambiguous(q, "a") }
    )
    .is_err());
    assert_eq!(
        serde_json::to_string(&*queues.q.lock_or_recover()).unwrap(),
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
    activity: &Observations,
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
    let q = qm.lock_or_recover();
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
        &|_: &QueueItem| Err(DeckError::classified("injection refused")),
        &ok_persist,
    );
    assert_eq!(
        res,
        SendResult::Failed {
            session: "s".into(),
            gave_up: false
        }
    );
    let q = qm.lock_or_recover();
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
        &|_: &QueueItem| Err(DeckError::classified("refused")),
        &ok_persist,
    );
    qm.lock_or_recover().items[0].last_attempt_at = Some(0); // backoff elapsed
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
    assert_eq!(qm.lock_or_recover().deliveries.len(), 1, "one audit total");
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
        &|_: &QueueState| Err(DeckError::classified("disk full")),
    );
    assert_eq!(res, SendResult::NotPersisted);
    let q = qm.lock_or_recover();
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
            prepare: &|item: &QueueItem, cancelled: &dyn Fn() -> bool| {
                Prepared::Probe(prepare(item, cancelled))
            },
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
    assert_eq!(qm.lock_or_recover().deliveries.len(), 1);
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
        let q = qm.lock_or_recover();
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
    let q = qm.lock_or_recover();
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
    let q = qm.lock_or_recover();
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
    let q = qm.lock_or_recover();
    assert_eq!(q.items[0].attempts, 0);
    assert!(q.pending.is_empty());
}

#[test]
fn pause_edit_and_delete_during_probe_cancel_the_worker() {
    for action in ["pause", "edit", "delete"] {
        let qm = Mutex::new(qs(vec![due_at("a", "s")]));
        let prepare = |_: &QueueItem, _: &dyn Fn() -> bool| {
            let mut q = qm.lock_or_recover();
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
            .lock_or_recover()
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
                clear_session_items(&mut qm.lock_or_recover(), "s");
                Prepared::Probe(probe_result(
                    ContextStatus::Unavailable,
                    ContextCode::CancelledOrRevised,
                    1,
                ))
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
                        Prepared::Probe(probe_result(
                            ContextStatus::Ready,
                            ContextCode::ProcessMatched,
                            1,
                        ))
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
                    Prepared::Probe(probe_result(
                        ContextStatus::Ready,
                        ContextCode::ProcessMatched,
                        2,
                    ))
                },
                final_probe: &|_: &QueueItem| {
                    probe_result(ContextStatus::Ready, ContextCode::ProcessMatched, 2)
                },
            },
        );
        assert!(matches!(fast, SendResult::Sent { .. }));
        assert!(!qm
            .lock_or_recover()
            .items
            .iter()
            .any(|i| i.session == "fast"));
        release.wait();
    });
    assert_eq!(qm.lock_or_recover().deliveries.len(), 2);
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
            let q = qm.lock_or_recover();
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
    let q = qm.lock_or_recover();
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
            return Err(DeckError::classified(
                "No space left on device (os error 28)",
            ));
        }
        self.writes
            .lock_or_recover()
            .push(serde_json::to_string(q).unwrap());
        Ok(())
    }
    /// what a fresh deck would load right now
    fn on_disk(&self) -> String {
        self.writes.lock_or_recover().last().unwrap().clone()
    }
}

fn add_args(session: &str, text: &str) -> QueueAddArgs {
    QueueAddArgs {
        operation_id: None,
        session: session.into(),
        card_id: format!("card-{session}"),
        dir: String::new(),
        cmd: String::new(),
        text: text.into(),
        mode: "chain".into(),
        at: None,
        quiet_secs: None,
        review_each: false,
        every: None,
        not_before: None,
        win_from: None,
        win_to: None,
        until_n: None,
        until_at: None,
        steps: None,
        tpl: None,
        tpl_idx: None,
        tpl_total: None,
        group: None,
        external_text: false,
        channel_path: false,
    }
}

#[test]
fn buffer_queue_operation_is_durable_idempotent_evidence() {
    let mut q = qs(Vec::new());
    let mut args = add_args("s", "immutable copy");
    args.operation_id = Some("Bcopy1".into());
    add_item(&mut q, args.clone(), normalize_prompt(&args.text)).unwrap();
    let item = q.items[0].id.clone();
    add_item(&mut q, args.clone(), normalize_prompt(&args.text)).unwrap();
    assert_eq!(
        q.items.len(),
        1,
        "a repeated operation cannot enqueue twice"
    );
    assert_eq!(q.operations.len(), 1);

    let mut conflict = args.clone();
    conflict.text = "different text".into();
    assert!(add_item(&mut q, conflict.clone(), normalize_prompt(&conflict.text)).is_err());
    finalize_delivery(&mut q, &item, "delivery-copy1", NOW, false);
    q.deliveries.clear(); // the ordinary 200-row audit may rotate
    assert_eq!(q.operations[0].state, "delivered");
    assert!(q.items.is_empty());

    let mut queued = add_args("s", "cancel me");
    queued.operation_id = Some("Bcopy2".into());
    add_item(&mut q, queued.clone(), normalize_prompt(&queued.text)).unwrap();
    let queued_id = q.items[0].id.clone();
    remove_item(&mut q, &queued_id).unwrap();
    assert_eq!(
        q.operations
            .iter()
            .find(|op| op.id == "Bcopy2")
            .unwrap()
            .state,
        "canceled"
    );
    clear_session_items(&mut q, "s");
    add_item(&mut q, queued.clone(), normalize_prompt(&queued.text)).unwrap();
    assert!(
        q.items.is_empty(),
        "a canceled operation stays reserved after session/card cleanup"
    );
    let mut changed_old = queued.clone();
    changed_old.text = "replayed with changed intent".into();
    assert!(add_item(
        &mut q,
        changed_old.clone(),
        normalize_prompt(&changed_old.text)
    )
    .is_err());
    let seed = q.operations[0].clone();
    while q.operations.len() < MAX_QUEUE_OPERATIONS {
        let mut operation = seed.clone();
        operation.id = format!("Bfill{}", q.operations.len());
        q.operations.push(operation);
    }
    add_item(&mut q, args.clone(), normalize_prompt(&args.text)).unwrap();
    let mut full = add_args("s", "new after full");
    full.operation_id = Some("BnewAfterFull".into());
    assert!(add_item(&mut q, full.clone(), normalize_prompt(&full.text)).is_err());
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
        let before = serde_json::to_string(&*qm.lock_or_recover()).unwrap();
        with_queue(&qm, &|q| disk.persist(q), mutate).unwrap_or_else(|e| panic!("{name}: {e}"));
        let after = serde_json::to_string(&*qm.lock_or_recover()).unwrap();
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
            serde_json::to_string(&*qm.lock_or_recover()).unwrap(),
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
    let disk = FakeDisk::new(&qm.lock_or_recover().clone());
    let writes0 = disk.writes.lock_or_recover().len();
    assert!(with_queue(&qm, &|q| disk.persist(q), |q| update_text(
        q,
        "a",
        "edited".into()
    ))
    .is_err());
    assert_eq!(
        disk.writes.lock_or_recover().len(),
        writes0,
        "no save tried"
    );
    assert_eq!(qm.lock_or_recover().items[0].text, "x");
}

#[test]
fn a_failed_retry_save_keeps_the_item_out_of_the_candidate_set() {
    let mut f = qi("f", "chain");
    f.state = "failed".into();
    f.attempts = MAX_ATTEMPTS;
    let qm = Mutex::new(qs(vec![f]));
    let disk = FakeDisk::new(&qm.lock_or_recover().clone());
    disk.fail.store(true, AtomicOrdering::Relaxed);
    assert!(with_queue(&qm, &|q| disk.persist(q), |q| retry_item(q, "f")).is_err());
    let q = qm.lock_or_recover();
    assert!(item_dead(&q.items[0]), "still dead in memory");
    let quiet = seen(NOW - 400);
    assert!(
        select_due(&q, NOW, 720, &quiet).is_empty(),
        "a retry the user was told failed must not re-enter the schedule"
    );
}

#[test]
fn a_failed_pre_fire_save_sends_nothing_and_changes_nothing() {
    let qm = Mutex::new(qs(vec![due_at("a", "s")]));
    let before = serde_json::to_string(&*qm.lock_or_recover()).unwrap();
    let dirty = AtomicBool::new(false);
    let res = send_test(
        &qm,
        &dirty,
        "s",
        720,
        &HashMap::new(),
        &|_: &QueueItem| panic!("must not inject when the intent never hit disk"),
        &|_: &QueueState| Err(DeckError::classified("No space left on device")),
    );
    assert_eq!(res, SendResult::NotPersisted);
    assert_eq!(
        serde_json::to_string(&*qm.lock_or_recover()).unwrap(),
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
    let disk = FakeDisk::new(&qm.lock_or_recover().clone());
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
        let q = qm.lock_or_recover();
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
        serde_json::to_string(&*qm.lock_or_recover()).unwrap()
    );
    assert!(!flush_dirty(&qm, &dirty, &persist), "nothing owed anymore");
}

#[test]
fn a_definitively_refused_send_that_cannot_be_saved_is_retried_not_forgotten() {
    let qm = Mutex::new(qs(vec![due_at("a", "s")]));
    let disk = FakeDisk::new(&qm.lock_or_recover().clone());
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
            Err(DeckError::classified(
                "tmux send-keys failed: can't find session: x",
            ))
        },
        &persist,
    );
    assert!(matches!(res, SendResult::Failed { .. }));
    {
        let q = qm.lock_or_recover();
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
            Err(DeckError::classified("tmux send-keys refused"))
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

/// A prompt is pasted byte for byte, so the only rewrite that matters is the
/// one that keeps the burst from submitting itself. This must stay in step
/// with `normalizeTemplateStep` in ui/js/pure.js — the same prompt reaches
/// the queue from a template and from the panel's own field.
#[test]
fn normalize_prompt_keeps_the_lines_and_folds_every_carriage_return() {
    use super::ops::normalize_prompt;

    // newlines are content; a CR in any spelling becomes one
    assert_eq!(normalize_prompt("one\ntwo"), "one\ntwo");
    assert_eq!(normalize_prompt("one\r\ntwo\rthree"), "one\ntwo\nthree");
    assert!(!normalize_prompt("a\r\nb\rc").contains('\r'));

    // indentation is what the user typed, and stays
    assert_eq!(
        normalize_prompt("review\n  - file:line\n  - the fix"),
        "review\n  - file:line\n  - the fix"
    );

    // only the invisible whitespace goes: trailing spaces and the edges
    assert_eq!(
        normalize_prompt("keep   \n\n  \nthese\n\n"),
        "keep\n\n\nthese"
    );
    assert_eq!(normalize_prompt("a\tb   c"), "a b   c");
    assert_eq!(normalize_prompt("   "), "");
    assert_eq!(normalize_prompt("\n\n  \n"), "");
}

// C v01: a checkpoint is a durable order barrier, independent of hook/quiet.
fn reviewed_pair() -> QueueState {
    let mut first = qi("inspect-a", "at");
    first.at = Some(NOW - 10);
    first.review_each = true;
    first.binding = Some(pane(1));
    let mut next = qi("inspect-b", "chain");
    next.review_each = true;
    let mut q = qs(vec![first, next]);
    finalize_delivery(&mut q, "inspect-a", "delivery-a", NOW, false);
    q
}

#[test]
fn review_wait_survives_restart_quiet_manual_now_and_old_done_cannot_authorize_it() {
    let q = reviewed_pair();
    let raw = serde_json::to_string(&q).unwrap();
    let mut q: QueueState = serde_json::from_str(&raw).unwrap();
    recover_interrupted(&mut q);
    assert!(select_for_session(&q, "s", NOW + 99_999, 0, &HashMap::new()).is_none());
    assert!(select_requested(&q, "s", "inspect-a", NOW + 99_999).is_none());
    assert!(select_requested(&q, "s", "inspect-b", NOW + 99_999).is_none());
    assert_eq!(q.deliveries.len(), 1);
    assert_eq!(
        q.items.iter().find(|i| i.id == "inspect-a").unwrap().state,
        "review"
    );
    assert!(retry_item(&mut q, "inspect-a").is_err());
    assert!(remove_item(&mut q, "inspect-a").is_err());
    assert!(update_text(&mut q, "inspect-a", "changed".into()).is_err());
}

#[test]
fn human_inspection_releases_only_its_successor_with_gap_and_quiet_still_required() {
    let mut q = reviewed_pair();
    let d = review::decision_for(&q, "inspect-a", pane(1)).unwrap();
    confirm_review(&mut q, &d, pane(1), NOW + 1).unwrap();
    confirm_review(&mut q, &d, pane(1), NOW + 2).unwrap();
    assert_eq!(q.reviews.len(), 1);
    assert!(select_for_session(&q, "s", NOW + 30, 0, &HashMap::new()).is_none());
    assert!(select_for_session(&q, "s", NOW + 61, 0, &seen(NOW)).is_none());
    assert_eq!(
        select_for_session(&q, "s", NOW + 181, 0, &HashMap::new())
            .unwrap()
            .id,
        "inspect-b"
    );
    finalize_delivery(&mut q, "inspect-b", "delivery-b", NOW + 181, false);
    assert!(!q.items.iter().any(|i| i.id == "inspect-a"));
    assert_eq!(q.items[0].state, "review");
    assert!(!q.review_completed.contains("s"));
    let last = review::decision_for(&q, "inspect-b", pane(1)).unwrap();
    confirm_review(&mut q, &last, pane(1), NOW + 182).unwrap();
    assert!(q.items.is_empty());
    assert!(q.review_completed.contains("s"));
    confirm_review(&mut q, &last, pane(1), NOW + 183).unwrap();
    assert_eq!(q.reviews.len(), 2);
    assert_eq!(q.deliveries.len(), 2);
}

#[test]
fn inspection_save_failure_leaves_memory_and_disk_decision_unreleased() {
    let q = reviewed_pair();
    let d = review::decision_for(&q, "inspect-a", pane(1)).unwrap();
    let before = serde_json::to_string(&q).unwrap();
    let qm = Mutex::new(q);
    let failed = |_: &QueueState| Err(DeckError::new(ErrorKind::Other, "test save rejected"));
    assert!(with_queue(&qm, &failed, |q| confirm_review(q, &d, pane(1), NOW)).is_err());
    assert_eq!(
        serde_json::to_string(&*qm.lock_or_recover()).unwrap(),
        before
    );
}

#[test]
fn inspection_preview_is_invalidated_by_successor_edit_removal_target_change_and_cancel() {
    for change in 0..4 {
        let mut q = reviewed_pair();
        let d = review::decision_for(&q, "inspect-a", pane(1)).unwrap();
        match change {
            0 => update_text(&mut q, "inspect-b", "different".into()).unwrap(),
            1 => {
                remove_item(&mut q, "inspect-b").unwrap();
            }
            2 => {}
            _ => clear_session_items(&mut q, "s"),
        }
        assert!(
            confirm_review(&mut q, &d, if change == 2 { pane(2) } else { pane(1) }, NOW).is_err()
        );
        assert!(q.reviews.is_empty());
    }
}

#[test]
fn unused_inspection_permission_is_revoked_by_edit_retry_and_target_generation() {
    for change in 0..3 {
        let mut q = reviewed_pair();
        let old = review::decision_for(&q, "inspect-a", pane(1)).unwrap();
        confirm_review(&mut q, &old, pane(1), NOW).unwrap();
        match change {
            0 => update_text(&mut q, "inspect-b", "different".into()).unwrap(),
            1 => retry_item(&mut q, "inspect-b").unwrap(),
            _ => {
                assert!(invalidate_review_target(
                    &mut q,
                    "inspect-b",
                    Some(&pane(2))
                ));
            }
        }
        assert!(select_requested(&q, "s", "inspect-b", NOW + 9999).is_none());
        // Replaying the old decision cannot release the new revision.
        confirm_review(&mut q, &old, pane(1), NOW).unwrap();
        assert_eq!(
            q.items.iter().find(|i| i.id == "inspect-a").unwrap().state,
            "review"
        );
        let new = review::decision_for(&q, "inspect-a", pane(2)).unwrap();
        confirm_review(&mut q, &new, pane(2), NOW + 1).unwrap();
        assert_eq!(q.reviews.len(), 2);
    }
}

#[test]
fn reviewed_repeating_single_row_and_last_iteration_wait_for_last_inspection() {
    for last in [false, true] {
        let mut i = rule(60);
        i.review_each = true;
        i.until_n = last.then_some(1);
        let mut q = qs(vec![i]);
        finalize_delivery(&mut q, "t", "iteration-1", NOW, false);
        let cp = q.items.iter().find(|i| is_review(i)).unwrap().id.clone();
        assert!(select_for_session(&q, "s", NOW + 9999, 0, &HashMap::new()).is_none());
        let d = review::decision_for(&q, &cp, pane(1)).unwrap();
        confirm_review(&mut q, &d, pane(1), NOW + 9999).unwrap();
        assert_eq!(
            select_for_session(&q, "s", NOW + 9999, 0, &HashMap::new()).is_some(),
            !last
        );
        if !last {
            finalize_delivery(&mut q, "t", "iteration-2", NOW + 9999, false);
            assert!(q
                .items
                .iter()
                .any(|i| is_review(i) && i.review.as_ref().unwrap().delivery == "iteration-2"));
        }
    }
}

#[test]
fn ambiguous_acknowledgement_creates_inspection_not_a_successful_business_result() {
    let mut i = qi("a", "at");
    i.review_each = true;
    i.at = Some(NOW);
    i.state = "firing".into();
    i.delivery = Some("uncertain".into());
    let mut q = qs(vec![i]);
    recover_interrupted(&mut q);
    assert_eq!(q.items[0].state, "ambiguous");
    acknowledge_ambiguous(&mut q, "a").unwrap();
    acknowledge_ambiguous(&mut q, "a").unwrap();
    assert_eq!(q.deliveries.len(), 1);
    assert!(q.deliveries[0].assumed);
    assert_eq!(q.items[0].state, "review");
    assert!(q.reviews.is_empty());
}

#[test]
fn checkpoint_blocks_only_its_group_and_cancel_does_not_mark_it_inspected() {
    let mut q = reviewed_pair();
    let mut other = qi("other-list", "at");
    other.at = Some(NOW);
    other.group = Some("other-group".into());
    q.items.push(other);
    assert_eq!(
        select_for_session(&q, "s", NOW + 181, 0, &HashMap::new())
            .unwrap()
            .id,
        "other-list"
    );
    cancel_list(&mut q, "inspect-a").unwrap();
    assert_eq!(ids(&q.items), vec!["other-list"]);
    assert_eq!(q.deliveries.len(), 1);
    assert!(q.reviews.is_empty());
}

#[test]
fn opting_out_never_removes_an_existing_checkpoint_and_firing_refuses_mode_changes() {
    let mut q = reviewed_pair();
    set_review_mode(&mut q, "inspect-a", false).unwrap();
    assert!(q.items.iter().any(is_review));
    assert!(select_requested(&q, "s", "inspect-b", NOW + 9999).is_none());
    assert!(
        !q.items
            .iter()
            .find(|i| i.id == "inspect-b")
            .unwrap()
            .review_each
    );
    q.items
        .iter_mut()
        .find(|i| i.id == "inspect-b")
        .unwrap()
        .state = "firing".into();
    assert!(set_review_mode(&mut q, "inspect-a", true).is_err());
    assert!(cancel_list(&mut q, "inspect-a").is_err());
}

#[test]
fn observed_replacement_revokes_inspection_before_any_injection() {
    let q = reviewed_pair();
    let qm = Mutex::new(q);
    {
        let mut q = qm.lock_or_recover();
        let d = review::decision_for(&q, "inspect-a", pane(1)).unwrap();
        confirm_review(&mut q, &d, pane(1), NOW).unwrap();
        q.last_fired.clear();
    }
    let dirty = AtomicBool::new(false);
    let result = send_one_safe(
        &qm,
        &dirty,
        "s",
        0,
        &HashMap::new(),
        &SendHooks {
            fire: &|_| panic!("stale permission must never inject"),
            persist: &ok_persist,
            kill: &|_| {},
        },
        &ContextHooks {
            prepare: &|_, _| {
                Prepared::Probe(probe_result(
                    ContextStatus::Ready,
                    ContextCode::CompatibilityTarget,
                    2,
                ))
            },
            final_probe: &|_| panic!("revoked before final probe"),
        },
    );
    assert!(matches!(result, SendResult::Nothing));
    let q = qm.lock_or_recover();
    assert!(q.pending.is_empty());
    assert_eq!(
        q.items.iter().find(|i| i.id == "inspect-a").unwrap().state,
        "review"
    );
}

#[test]
fn inspecting_one_list_never_marks_another_pending_list_inspected() {
    let mut q = reviewed_pair();
    let mut other = qi("other-review", "at");
    other.group = Some("other-group".into());
    other.review_each = true;
    q.items.push(other);
    finalize_delivery(&mut q, "other-review", "other-delivery", NOW, false);
    let d = review::decision_for(&q, "other-review", pane(1)).unwrap();
    confirm_review(&mut q, &d, pane(1), NOW + 1).unwrap();
    assert!(!q.review_completed.contains("s"));
    // Cancel is not a last-inspection receipt, including after a prior one.
    q.review_completed.insert("s".into());
    cancel_list(&mut q, "inspect-a").unwrap();
    assert!(!q.review_completed.contains("s"));
    assert!(q.items.is_empty());
}

#[test]
fn restart_pause_survives_reload_and_save_failure_preserves_the_queue() {
    let mut other = qi("other", "at");
    other.session = "untouched".into();
    let original = qs(vec![
        qi("first", "at"),
        qi("next", "chain"),
        rule(60),
        other,
    ]);
    let qm = Mutex::new(original.clone());
    let fail = |_: &QueueState| {
        Err(crate::error::DeckError::new(
            crate::error::ErrorKind::DiskFull,
            "fixture",
        ))
    };
    assert!(with_queue(&qm, &fail, |q| pause_restart_sessions(q, &["s".into()])).is_err());
    assert_eq!(
        serde_json::to_value(&*qm.lock_or_recover()).unwrap(),
        serde_json::to_value(&original).unwrap()
    );
    let disk = std::cell::RefCell::new(String::new());
    let save = |q: &QueueState| {
        *disk.borrow_mut() = serde_json::to_string(q).unwrap();
        Ok(())
    };
    assert_eq!(
        with_queue(&qm, &save, |q| pause_restart_sessions(q, &["s".into()])).unwrap(),
        3
    );
    let restored: QueueState = serde_json::from_str(&disk.borrow()).unwrap();
    assert!(restored
        .items
        .iter()
        .filter(|i| i.session == "s")
        .all(|i| i.paused));
    assert!(
        !restored
            .items
            .iter()
            .find(|i| i.id == "other")
            .unwrap()
            .paused
    );
    assert!(select_due(&restored, NOW, 720, &HashMap::new())
        .iter()
        .all(|i| i.session == "untouched"));
}

#[test]
fn process_bound_delivery_requires_paste_mode_and_compatibility_does_not() {
    let pane = crate::context::PaneIdentity {
        server_pid: 1,
        session_id: "$1".into(),
        window_id: "@1".into(),
        pane_id: "%1".into(),
        pane_pid: 2,
    };
    let bound = qi("a", "at");
    let request = delivery::literal_request(&bound, &pane, "d1");
    assert_eq!(request.expected_process, Some("claude"));
    assert!(request.require_paste_mode);
    let mut compatibility = qi("b", "at");
    compatibility.expected_process = None;
    let request = delivery::literal_request(&compatibility, &pane, "d2");
    assert!(request.expected_process.is_none());
    assert!(!request.require_paste_mode);
}

#[test]
fn external_rows_are_admitted_only_for_agent_commands_with_simple_arguments() {
    for cmd in [
        "claude",
        "codex",
        "codex --yolo",
        "claude --dangerously-skip-permissions",
    ] {
        let mut args = add_args("s", "external");
        args.cmd = cmd.into();
        assert!(ops::require_channel_agent(&args).is_ok(), "{cmd}");
    }
    for cmd in [
        "",
        "zsh",
        "claude;zsh",
        "/usr/local/bin/claude",
        "FOO=1 codex",
    ] {
        let mut args = add_args("s", "external");
        args.cmd = cmd.into();
        assert_eq!(
            ops::require_channel_agent(&args).unwrap_err().kind(),
            ErrorKind::Invalid,
            "{cmd:?}"
        );
    }
}

#[test]
fn external_admission_refuses_a_plain_shell_card_and_marks_what_it_admits() {
    for cmd in ["zsh", "", "bash -l", "claude;zsh", "codex; sh"] {
        let mut args = add_args("s", "rm -rf ~");
        args.cmd = cmd.into();
        assert_eq!(
            ops::admit_external(&mut args).unwrap_err().kind(),
            ErrorKind::Invalid,
            "{cmd:?}"
        );
        assert!(!args.channel_path, "a refused row is never marked admitted");
    }
    let mut args = add_args("s", "please look at INC-42");
    args.mode = "at".into();
    args.at = Some(NOW);
    args.cmd = "claude --dangerously-skip-permissions".into();
    ops::admit_external(&mut args).unwrap();
    assert!(args.channel_path);
    let mut q = qs(Vec::new());
    add_item(&mut q, args, "please look at INC-42".into()).unwrap();
    assert!(q.items[0].external);
    assert_eq!(q.items[0].expected_process.as_deref(), Some("claude"));
}

// ---------- agent hold (select.rs header) ----------

#[test]
fn needs_input_holds_every_automatic_mode() {
    let mut a = qi("a", "at");
    a.at = Some(NOW - 1);
    let c = qi("c", "chain");
    let r = rule(300);
    for item in [a, c, r] {
        let id = item.id.clone();
        let q = qs(vec![item]);
        let quiet = NOW - 400;
        assert!(
            select_due(&q, NOW, 720, &seen_agent(quiet, "needs-input")).is_empty(),
            "{id}: a permission prompt must not receive a paste and Enter"
        );
        for agent in ["working", "turn-done"] {
            assert_eq!(
                ids(&select_due(&q, NOW, 720, &seen_agent(quiet, agent))),
                [id.as_str()],
                "{id}/{agent}: only needs-input holds an owner row"
            );
        }
        assert_eq!(ids(&select_due(&q, NOW, 720, &seen(quiet))), [id.as_str()]);
    }
}

#[test]
fn external_follow_up_row_is_never_released_by_a_hook_word() {
    let mut c = qi("c", "chain");
    c.external = true;
    let q = qs(vec![c]);
    let quiet = NOW - 400;
    // `turn-done` ends an interaction; the agent may still own background
    // work and resume on its own, so it is no readiness for external text
    for agent in ["working", "needs-input", "turn-done"] {
        assert!(
            select_due(&q, NOW, 720, &seen_agent(quiet, agent)).is_empty(),
            "{agent}"
        );
    }
    // no hook word: quiet alone cannot tell a finished turn from a prompt
    assert!(select_due(&q, NOW, 720, &seen(quiet)).is_empty());
    // a dead session has no hook word either
    assert!(select_due(&q, NOW, 720, &HashMap::new()).is_empty());
    // the user's send-now remains the release, whatever the hook said
    for agent in ["working", "turn-done"] {
        assert_eq!(
            select_for_request(&q, "s", NOW, 720, &seen_agent(quiet, agent), Some("c"))
                .unwrap()
                .id,
            "c",
            "{agent}"
        );
    }
    // an owner row keeps the quiet-only rule, dead session and every
    // non-input word included
    let q = qs(vec![qi("o", "chain")]);
    assert_eq!(ids(&select_due(&q, NOW, 720, &HashMap::new())), ["o"]);
    for agent in ["working", "turn-done"] {
        assert_eq!(
            ids(&select_due(&q, NOW, 720, &seen_agent(quiet, agent))),
            ["o"],
            "{agent}"
        );
    }
    // an owner row still waits for its quiet time
    assert!(select_due(&q, NOW, 720, &seen_agent(NOW - 10, "turn-done")).is_empty());
    // and an input request holds an owner row too
    assert!(select_due(&q, NOW, 720, &seen_agent(quiet, "needs-input")).is_empty());
}

#[test]
fn external_first_row_is_not_held_without_hooks() {
    // a fresh channel card's first row: its agent has had no turn yet
    let mut a = qi("a", "at");
    a.at = Some(NOW - 1);
    a.external = true;
    let q = qs(vec![a]);
    assert_eq!(ids(&select_due(&q, NOW, 720, &HashMap::new())), ["a"]);
    assert!(select_due(&q, NOW, 720, &seen_agent(NOW - 400, "needs-input")).is_empty());
}

#[test]
fn manual_send_now_is_not_held_by_the_agent() {
    let mut c = qi("c", "chain");
    c.external = true;
    let q = qs(vec![c]);
    let held = seen_agent(NOW - 400, "needs-input");
    assert!(select_for_request(&q, "s", NOW, 720, &held, None).is_none());
    assert_eq!(
        select_for_request(&q, "s", NOW, 720, &held, Some("c"))
            .unwrap()
            .id,
        "c"
    );
}

/// Session "s" quiet since `activity`, Codex in the foreground of its
/// Signal target with `trust`, and the agent hook reporting `agent`.
fn seen_codex(
    activity: u64,
    agent: Option<&'static str>,
    trust: crate::agent_status::CodexSignalTrust,
) -> Observations {
    HashMap::from([(
        "s".to_string(),
        Observed {
            activity,
            agent,
            codex: Some(trust),
            claude_interaction: false,
        },
    )])
}

/// Codex shared-daemon FR: without a trusted Codex Signal, "no agent word"
/// must not fall back to the quiet-only rule — a Codex permission prompt
/// is quiet too.
#[test]
fn a_codex_foreground_without_trusted_signal_holds_every_automatic_row() {
    use crate::agent_status::CodexSignalTrust::{Trusted, Unavailable, Unknown};
    let quiet = NOW - 400;
    let mut a = qi("a", "at");
    a.at = Some(NOW - 1);
    for mut item in [a, qi("c", "chain"), rule(300)] {
        item.expected_process = Some("codex".into());
        let id = item.id.clone();
        let q = qs(vec![item]);
        // configured for Codex: without a proof the row is held even when
        // tmux does not name the foreground `codex`
        assert!(select_due(&q, NOW, 720, &seen(quiet)).is_empty(), "{id}");
        for trust in [Unknown, Unavailable] {
            for agent in [None, Some("working"), Some("turn-done")] {
                assert!(
                    select_due(&q, NOW, 720, &seen_codex(quiet, agent, trust)).is_empty(),
                    "{id}/{trust:?}/{agent:?}: an unproven Codex gets no automatic paste"
                );
            }
            // the user's send-now is not held
            assert_eq!(
                select_for_request(
                    &q,
                    "s",
                    NOW,
                    720,
                    &seen_codex(quiet, None, trust),
                    Some(&id)
                )
                .map(|i| i.id),
                Some(id.clone()),
                "{id}/{trust:?}: manual send-now"
            );
        }
        // Trusted: exactly the existing semantics
        for agent in [None, Some("working"), Some("turn-done")] {
            assert_eq!(
                ids(&select_due(
                    &q,
                    NOW,
                    720,
                    &seen_codex(quiet, agent, Trusted)
                )),
                [id.as_str()],
                "{id}/{agent:?}: a trusted Codex keeps the quiet-only rule"
            );
        }
        assert!(
            select_due(
                &q,
                NOW,
                720,
                &seen_codex(quiet, Some("needs-input"), Trusted)
            )
            .is_empty(),
            "{id}: a trusted input request holds"
        );
    }
    // a trusted owner chain still waits for its quiet time
    let q = qs(vec![qi("o", "chain")]);
    assert!(select_due(&q, NOW, 720, &seen_codex(NOW - 10, None, Trusted)).is_empty());
    // the external follow-up rule is unchanged: held even when trusted
    let mut ext = qi("e", "chain");
    ext.external = true;
    let q = qs(vec![ext]);
    assert!(select_due(&q, NOW, 720, &seen_codex(quiet, Some("turn-done"), Trusted)).is_empty());
    // the plan names the hold
    let q = qs(vec![qi("o", "chain")]);
    let stage = serde_json::to_value(plan_item(
        &q,
        &q.items[0],
        NOW,
        720,
        Some(&seen_codex(quiet, None, Unavailable)),
    ))
    .unwrap()["stage"]
        .clone();
    assert_eq!(stage, "agent");
}

/// A row configured for Codex (`expected_process`) is gated in an EXISTING
/// session even when tmux names the foreground `node` or a wrapper, and
/// released by a trust proof; a session a successful listing proves absent
/// may still bootstrap. Rows configured for anything else are unchanged.
#[test]
fn a_codex_configured_row_is_gated_whatever_the_foreground_name() {
    use crate::agent_status::CodexSignalTrust::{Trusted, Unavailable};
    let quiet = NOW - 400;
    let mut row = qi("o", "chain");
    row.cmd = "codex".into();
    row.expected_process = Some("codex".into());
    let q = qs(vec![row]);
    // existing session, `node` foreground, no proof: codex trust is absent
    // from the observation, the configuration alone holds
    assert!(select_due(&q, NOW, 720, &seen(quiet)).is_empty());
    assert!(select_due(&q, NOW, 720, &seen_agent(quiet, "turn-done")).is_empty());
    let plan =
        serde_json::to_value(plan_item(&q, &q.items[0], NOW, 720, Some(&seen(quiet)))).unwrap();
    assert_eq!(plan["stage"], "first-send");
    // a pane-bound Codex hook proved this generation: normal semantics
    assert_eq!(
        ids(&select_due(&q, NOW, 720, &seen_codex(quiet, None, Trusted))),
        ["o"]
    );
    assert!(select_due(
        &q,
        NOW,
        720,
        &seen_codex(quiet, Some("needs-input"), Trusted)
    )
    .is_empty());
    assert!(select_due(&q, NOW, 720, &seen_codex(quiet, None, Unavailable)).is_empty());
    // a session the listing proves absent: the first row starts Codex
    assert_eq!(ids(&select_due(&q, NOW, 720, &HashMap::new())), ["o"]);
    // send-now is never held
    assert!(select_for_request(&q, "s", NOW, 720, &seen(quiet), Some("o")).is_some());
    // rows configured for another program keep the quiet-only rule
    for other in [Some("claude"), Some("zsh"), None] {
        let mut row = qi("o", "chain");
        row.expected_process = other.map(str::to_string);
        let q = qs(vec![row]);
        assert_eq!(
            ids(&select_due(&q, NOW, 720, &seen(quiet))),
            ["o"],
            "{other:?}"
        );
    }
}

/// A failed pane listing proves no session absent: a live Codex may be
/// waiting on a permission prompt. Nothing is selected that tick; a
/// successful listing that shows the session absent still bootstraps.
#[test]
fn a_failed_pane_listing_selects_nothing() {
    let mut owner = qi("o", "chain");
    owner.cmd = "codex".into();
    owner.expected_process = Some("codex".into());
    let mut at = qi("a", "at");
    at.at = Some(NOW - 1);
    let mut plain = qi("p", "chain");
    plain.session = "t".into();
    let q = qs(vec![owner, at, plain]);
    assert!(tick_selection(&q, NOW, 720, None).is_empty());
    let absent = Observations::new();
    let mut due = ids(&tick_selection(&q, NOW, 720, Some(&absent)))
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    due.sort();
    assert_eq!(
        due,
        ["a", "p"],
        "one per session; the at row leads session s"
    );
}

#[test]
fn plan_reports_the_agent_hold() {
    let mut c = qi("c", "chain");
    c.external = true;
    let q = qs(vec![c]);
    let stage = |obs: Option<&Observations>| {
        serde_json::to_value(plan_item(&q, &q.items[0], NOW, 720, obs)).unwrap()["stage"].clone()
    };
    assert_eq!(stage(Some(&seen(NOW - 400))), "agent");
    assert_eq!(
        stage(Some(&seen_agent(NOW - 400, "turn-done"))),
        "agent",
        "an interaction boundary does not release an external follow-up"
    );
    assert_eq!(stage(None), "unknown");
}

#[test]
fn spawned_iteration_rows_inherit_the_external_mark() {
    let mut r = rule(300);
    r.steps = vec!["s2".into()];
    r.external = true;
    let mut q = qs(vec![r]);
    finalize_delivery(&mut q, "t", "d1", NOW, false);
    let step = q.items.iter().find(|i| i.mode == "chain").unwrap();
    assert!(step.external);
}

#[test]
fn external_mark_is_omitted_when_false() {
    let owner = serde_json::to_value(qi("o", "at")).unwrap();
    assert!(owner.get("external").is_none());
    let mut ext = qi("e", "chain");
    ext.external = true;
    let raw = serde_json::to_string(&ext).unwrap();
    let back: QueueItem = serde_json::from_str(&raw).unwrap();
    assert!(back.external);
}

#[test]
fn leading_command_skips_whitespace_and_format_characters() {
    for text in [
        "/clear",
        "!rm -rf ~",
        "# remember this",
        "  \n\t/compact",
        "\u{00A0}!x",
        "\u{0085}/x",
        "\u{3000}#x",
        "\u{200B}/x",
        "\u{200D}!x",
        "\u{180E}/x",
        "\u{2060}#x",
        "\u{FEFF}/x",
        "\u{00AD}!x",
        "\u{202E}/x",
        "\u{E0041}/x",
    ] {
        assert!(ops::leading_command(text), "{text:?}");
    }
    for text in [
        "",
        "   ",
        "please run /clear",
        "<@U123> /clear",
        "@user hi",
        "x # y",
        "\u{200B}hello",
    ] {
        assert!(!ops::leading_command(text), "{text:?}");
    }
}

#[test]
fn external_text_rows_are_refused_with_a_leading_command() {
    let mut args = add_args("s", "\u{200B} /clear");
    args.mode = "at".into();
    args.at = Some(NOW);
    args.cmd = "claude".into();
    args.external_text = true;
    assert_eq!(validate_add(&args).unwrap_err().kind(), ErrorKind::Invalid);
    // the same text from the owner is theirs to queue
    args.external_text = false;
    assert!(validate_add(&args).is_ok());
    // a plain external message passes, but only for an exact agent command
    args.external_text = true;
    args.text = "please look at INC-42".into();
    assert!(validate_add(&args).is_ok());
    args.cmd = "zsh".into();
    assert_eq!(validate_add(&args).unwrap_err().kind(), ErrorKind::Invalid);
}

#[test]
fn external_rows_carry_the_mark_and_keep_old_fingerprints() {
    let mut q = qs(Vec::new());
    let mut args = add_args("s", "hello");
    args.mode = "at".into();
    args.at = Some(NOW);
    args.cmd = "claude".into();
    let before = serde_json::to_value(&args).unwrap();
    assert!(before.get("externalText").is_none());
    assert!(before.get("channelPath").is_none());
    args.channel_path = true;
    assert_eq!(serde_json::to_value(&args).unwrap(), before);
    add_item(&mut q, args.clone(), "hello".into()).unwrap();
    assert!(q.items[0].external);
    args.channel_path = false;
    add_item(&mut q, args.clone(), "hello".into()).unwrap();
    assert!(!q.items[1].external);
    args.external_text = true;
    add_item(&mut q, args, "hello".into()).unwrap();
    assert!(q.items[2].external);
    // a caller cannot set the channel path itself
    let parsed: QueueAddArgs = serde_json::from_value(serde_json::json!({
        "session": "s", "dir": "", "cmd": "claude", "text": "x", "mode": "at", "at": 1,
        "every": null, "winFrom": null, "winTo": null, "untilN": null, "untilAt": null,
        "steps": null, "tpl": null, "tplIdx": null, "tplTotal": null, "channelPath": true
    }))
    .unwrap();
    assert!(!parsed.channel_path);
}

// ---------- ops.rs cores: edit, reviewed list, delivery-state edges ----------

#[test]
fn reviewed_external_list_replays_each_row_under_the_original_group() {
    let mut q = qs(Vec::new());
    let mut args = add_args("s", "first");
    args.mode = "at".into();
    args.at = Some(NOW);
    args.cmd = "codex --yolo".into();
    args.review_each = true;
    args.channel_path = true;
    args.operation_id = Some("Blist".into());
    let texts = vec!["first".into(), "second".into()];
    let creation = context::CreationContext {
        binding: None,
        expected_process: Some("codex".into()),
    };
    ops::add_reviewed_rows(&mut q, &args, &texts, &creation).unwrap();
    let first = q.items[0].id.clone();
    assert_eq!(
        q.items
            .iter()
            .map(|i| i.operation_id.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("Blist-0"), Some("Blist-1")]
    );
    assert!(q
        .items
        .iter()
        .all(|i| i.external && i.group.as_deref() == Some(first.as_str())));

    q.items.pop();
    q.operations.pop();
    ops::add_reviewed_rows(&mut q, &args, &texts, &creation).unwrap();
    assert_eq!(
        q.items.len(),
        2,
        "a partial commit gets only its missing row"
    );
    assert_eq!(q.items[1].group.as_deref(), Some(first.as_str()));

    q.items.remove(0);
    ops::add_reviewed_rows(&mut q, &args, &texts, &creation).unwrap();
    assert_eq!(
        q.items.len(),
        1,
        "a delivered first row is never queued again"
    );
    assert_eq!(q.items[0].group.as_deref(), Some(first.as_str()));
}

#[test]
fn edit_item_takes_exactly_one_of_text_or_steps() {
    let mut a = qi("a", "at");
    a.text = "original".into();
    let mut r = rule(300);
    r.steps = vec!["old".into()];
    let mut q = qs(vec![a, r]);
    for (text, steps) in [(None, None), (Some("x".to_string()), Some(vec![]))] {
        let err = edit_item(&mut q, "a", text, steps).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Invalid);
        assert_eq!(err.message(), "queue_update takes a text or a step list");
    }
    let err = edit_item(&mut q, "a", Some(" \r\n\t ".into()), None).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Invalid);
    assert_eq!(err.message(), "empty prompt");
    assert_eq!(
        q.items[0].text, "original",
        "a refused edit changes nothing"
    );
    assert_eq!(q.items[0].revision, 0);

    edit_item(&mut q, "a", Some("one\r\ntwo  \n".into()), None).unwrap();
    assert_eq!(q.items[0].text, "one\ntwo");
    assert_eq!(
        q.items[0].revision, 1,
        "an edit invalidates a readiness wait"
    );

    edit_item(
        &mut q,
        "t",
        None,
        Some(vec!["  ".into(), "step\ttwo\r".into(), "".into()]),
    )
    .unwrap();
    assert_eq!(q.items[1].steps, vec!["step two".to_string()]);
    assert_eq!(q.items[1].revision, 1);
    let err = edit_item(&mut q, "a", None, Some(vec!["y".into()])).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Invalid, "only a rule holds steps");
    // the firing contract applies to both shapes
    q.items[0].state = "firing".into();
    assert!(edit_item(&mut q, "a", Some("late".into()), None).is_err());
    assert_eq!(q.items[0].text, "one\ntwo");
}

#[test]
fn retry_and_acknowledge_resolve_every_delivery_state_exactly_once() {
    let odd = qi("odd", "at");
    let mut amb = qi("amb", "at");
    amb.state = "ambiguous".into();
    amb.delivery = Some("d-amb".into());
    amb.attempts = 3;
    amb.last_error = Some("refused".into());
    amb.last_attempt_at = Some(NOW);
    amb.operation_id = Some("Bamb".into());
    let mut plain = qi("plain", "at");
    plain.at = Some(NOW);
    let mut q = qs(vec![odd, amb.clone(), plain]);
    q.pending.push(PendingDelivery {
        id: "d-amb".into(),
        snapshot: amb,
    });
    q.operations.push(QueueOperation {
        id: "Bamb".into(),
        item: "amb".into(),
        session: "s".into(),
        card_id: "card-s".into(),
        fingerprint: "f".into(),
        state: "uncertain".into(),
    });

    // An unknown lifecycle word can no longer reach a row: queue.json that
    // carries one is refused when read (never guessed), so retry has no
    // "unknown delivery state" case left to reject.
    assert!(serde_json::from_value::<ItemState>(serde_json::json!("review-pending-typo")).is_err());

    retry_item(&mut q, "amb").unwrap();
    let re_armed = q.items.iter().find(|i| i.id == "amb").unwrap();
    assert_eq!(re_armed.state, "pending");
    assert_eq!(re_armed.attempts, 0);
    assert!(re_armed.last_error.is_none() && re_armed.last_attempt_at.is_none());
    assert!(re_armed.delivery.is_none());
    assert!(
        q.pending.is_empty(),
        "the ledger entry of the retried send is gone"
    );
    assert_eq!(q.operations[0].state, "queued");

    // acknowledge: only an ambiguous item is a decision
    let err = acknowledge_ambiguous(&mut q, "plain").unwrap_err();
    assert_eq!(
        err.message(),
        "this prompt is not awaiting an ambiguous-delivery decision"
    );
    let err = acknowledge_ambiguous(&mut q, "ghost").unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Missing);
    assert!(
        retry_item(&mut q, "ghost").is_ok(),
        "an unknown retry is a no-op"
    );
    assert_eq!(q.items.len(), 3);

    // a consumed once item: its receipt is the proof, replays are no-ops
    q.deliveries.push(DeliveryRecord {
        id: "d-gone".into(),
        item: "gone".into(),
        session: "s".into(),
        mode: "at".into(),
        at: NOW,
        assumed: false,
        operation_id: None,
    });
    assert!(acknowledge_ambiguous(&mut q, "gone").is_ok());
    assert!(retry_item(&mut q, "gone").is_ok());
    assert_eq!(q.deliveries.len(), 1, "no second receipt");
    // a delivered item that was re-added under the same id, delivery cleared
    q.deliveries.push(DeliveryRecord {
        id: "d-plain".into(),
        item: "plain".into(),
        session: "s".into(),
        mode: "at".into(),
        at: NOW,
        assumed: false,
        operation_id: None,
    });
    assert!(acknowledge_ambiguous(&mut q, "plain").is_ok());
    assert!(
        q.items.iter().any(|i| i.id == "plain"),
        "nothing consumed twice"
    );
}

#[test]
fn clearing_a_session_tombstones_it_and_cancels_only_its_open_operations() {
    let mut delivered = qi("done", "at");
    delivered.session = "gone".into();
    let mut other = qi("keep", "at");
    other.session = "other".into();
    let mut q = qs(vec![delivered, other]);
    q.last_fired.insert("gone".into(), NOW);
    q.review_completed.insert("gone".into());
    let op = |id: &str, session: &str, state: &str| QueueOperation {
        id: id.into(),
        item: format!("i-{id}"),
        session: session.into(),
        card_id: "c".into(),
        fingerprint: "f".into(),
        state: state.into(),
    };
    q.operations = vec![
        op("B1", "gone", "queued"),
        op("B2", "gone", "delivered"),
        op("B3", "gone", "uncertain"),
        op("B4", "other", "queued"),
    ];
    assert!(!is_cancelled(&q, "gone"));
    clear_session_items(&mut q, "gone");
    assert!(is_cancelled(&q, "gone"));
    assert!(!is_cancelled(&q, "other"));
    assert_eq!(ids(&q.items), ["keep"]);
    assert!(!q.last_fired.contains_key("gone"));
    assert!(!q.review_completed.contains("gone"));
    let states: Vec<&str> = q.operations.iter().map(|o| o.state.as_str()).collect();
    assert_eq!(states, ["canceled", "delivered", "canceled", "queued"]);
    // clearing again refreshes the tombstone instead of adding one
    q.cancelled[0].at = 0;
    clear_session_items(&mut q, "gone");
    assert_eq!(q.cancelled.len(), 1);
    assert!(q.cancelled[0].at > 0);
}

#[test]
fn add_validation_bounds_identities_counts_and_list_membership() {
    let base = || {
        let mut a = add_args("s", "x");
        a.mode = "at".into();
        a.at = Some(NOW);
        a
    };
    let refused = |a: QueueAddArgs, expect: &str| {
        let err = validate_add(&a).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Invalid, "{expect}");
        assert_eq!(err.message(), expect);
    };
    let mut a = base();
    a.session = "bad name".into();
    refused(a, "session name may only contain letters, digits, _ - @");
    let mut a = base();
    a.session = String::new();
    refused(a, "session name must be 1–64 characters");
    for card in ["", "bad card!", &"c".repeat(129)] {
        let mut a = base();
        a.card_id = card.into();
        refused(a, "scheduled prompt needs a valid card identity");
    }
    for op in ["", "op id", &"o".repeat(129)] {
        let mut a = base();
        a.operation_id = Some(op.into());
        refused(a, "invalid queue operation identity");
    }
    let mut a = base();
    a.operation_id = Some("Bcopy-1_2".into());
    assert!(validate_add(&a).is_ok());
    let mut a = base();
    a.mode = "every".into();
    refused(a, "a recurring rule needs an interval");
    let mut a = base();
    a.mode = "every".into();
    a.every = Some(59);
    refused(a, "recurring interval must be at least 1 minute");
    let mut a = base();
    a.group = Some("list".into());
    refused(a, "only a follow-up row joins a list");
    let mut a = base();
    a.mode = "chain".into();
    a.at = None;
    a.group = Some("list".into());
    assert!(validate_add(&a).is_ok(), "a chain row names its list");
    let mut a = base();
    a.until_n = Some(0);
    refused(a, "stop-after count must be at least 1");
    let mut a = base();
    a.until_n = Some(1);
    a.win_from = Some(0);
    a.win_to = Some(1439);
    assert!(validate_add(&a).is_ok(), "a full-day window and one fire");
    let mut a = base();
    a.win_from = Some(1440);
    a.win_to = Some(10);
    refused(a, "time-window minutes must be below 24h");
    let mut a = base();
    a.win_to = Some(10);
    refused(a, "a time window needs both ends");
}

#[test]
fn state_only_commands_fail_closed_without_the_smoke_hooks_or_the_item() {
    use tauri::Manager;
    let app = tauri::test::mock_app();
    let mut a = qi("a", "at");
    a.at = Some(NOW);
    app.manage(Queues::new(qs(vec![a])));
    let state = app.state::<Queues>();
    let err = queue_probe_context(state.clone(), "missing".into())
        .err()
        .expect("no probe without the item");
    assert_eq!(err.kind(), ErrorKind::Missing);
    assert_eq!(err.message(), "scheduled prompt not found");
    let seed = smoke_seed_ambiguous(state.clone()).unwrap_err();
    let disk = smoke_queue_state(state.clone())
        .err()
        .expect("smoke state is unavailable");
    let flush = smoke_flush_queue(state.clone()).unwrap_err();
    for err in [seed, disk, flush] {
        assert_eq!(err.kind(), ErrorKind::Other);
        assert_eq!(err.message(), "smoke queue hooks are unavailable");
    }
    let q = state.q.lock_or_recover();
    assert_eq!(q.items[0].state, "pending", "nothing was seeded");
    assert!(q.pending.is_empty());
    assert!(!state.dirty.load(AtomicOrdering::Relaxed));
}

/// queue.json bytes are unchanged by the typed states: the fixture is what
/// the string-typed scheduler serialized (all six row states, all four
/// operation states), and the enum round trip reproduces it byte for byte.
#[test]
fn typed_states_keep_queue_json_byte_identical() {
    let old = include_str!("queue-all-states.json");
    let q: QueueState = serde_json::from_str(old).unwrap();
    assert_eq!(serde_json::to_string(&q).unwrap(), old);
    let rows: Vec<&str> = q.items.iter().map(|i| i.state.as_str()).collect();
    assert_eq!(
        rows,
        [
            "pending",
            "firing",
            "failed",
            "ambiguous",
            "review",
            "review-approved"
        ]
    );
    let operations: Vec<&str> = q.operations.iter().map(|o| o.state.as_str()).collect();
    assert_eq!(operations, ["queued", "delivered", "canceled", "uncertain"]);
    // a missing row state still reads as pending (the old default)
    let mut value: serde_json::Value = serde_json::from_str(old).unwrap();
    value["items"][1].as_object_mut().unwrap().remove("state");
    let q: QueueState = serde_json::from_value(value).unwrap();
    assert_eq!(q.items[1].state, ItemState::Pending);
}

/// A transaction that finds nothing to do is a value, not an error: nothing
/// is persisted and memory is untouched; a real change persists then commits.
#[test]
fn with_queue_opt_never_persists_a_noop() {
    let qm = Mutex::new(qs(vec![qi("a", "at")]));
    let writes = std::cell::Cell::new(0);
    let persist = |_: &QueueState| -> Result<(), DeckError> {
        writes.set(writes.get() + 1);
        Ok(())
    };
    let none: Option<()> = with_queue_opt(&qm, &persist, |q| {
        q.items.clear(); // a candidate edit that must be dropped
        Ok(None)
    })
    .unwrap();
    assert!(none.is_none());
    assert_eq!(writes.get(), 0);
    assert_eq!(qm.lock_or_recover().items.len(), 1);
    let some = with_queue_opt(&qm, &persist, |q| {
        q.items.clear();
        Ok(Some(7))
    })
    .unwrap();
    assert_eq!(some, Some(7));
    assert_eq!(writes.get(), 1);
    assert!(qm.lock_or_recover().items.is_empty());
}

// ---------- Agent Bootstrap Input Safety (select.rs first-interaction gate) --

/// A due `at` row configured for `process`.
fn bootstrap_row(process: &str) -> QueueItem {
    let mut row = qi("b", "at");
    row.at = Some(NOW - 1);
    row.cmd = process.into();
    row.expected_process = Some(process.into());
    row
}

/// An existing recognized-agent session without interaction evidence in
/// its current generation is held at `first-send`, whatever the reason
/// (startup dialog, fresh process, Deck restarted, hooks off); evidence
/// resumes the ordinary rules; send-now is never held; other process-bound
/// programs are unchanged.
#[test]
fn a_recognized_agent_needs_current_generation_interaction_evidence() {
    use crate::agent_status::CodexSignalTrust::{Trusted, Unavailable, Unknown};
    let quiet = NOW - 400;
    let stage = |q: &QueueState, obs: &Observations| {
        serde_json::to_value(plan_item(q, &q.items[0], NOW, 720, Some(obs))).unwrap()["stage"]
            .clone()
    };
    // Claude: no evidence → held at first-send (hooks off stays like this)
    let q = qs(vec![bootstrap_row("claude")]);
    assert!(select_due(&q, NOW, 720, &unestablished(quiet)).is_empty());
    assert_eq!(stage(&q, &unestablished(quiet)), "first-send");
    assert_eq!(
        hold_reason(&q.items[0], unestablished(quiet).get("s")),
        Some(Hold::FirstInteraction)
    );
    // evidence (an accepted Claude interaction) → ordinary rules resume
    assert_eq!(ids(&select_due(&q, NOW, 720, &seen(quiet))), ["b"]);
    assert!(select_due(&q, NOW, 720, &seen_agent(quiet, "needs-input")).is_empty());
    // send-now bypasses the automatic hold
    assert!(select_for_request(&q, "s", NOW, 720, &unestablished(quiet), Some("b")).is_some());
    // Codex: Trusted is its evidence; Unknown is first-send; Unavailable is
    // the (unchanged) agent hold
    let q = qs(vec![bootstrap_row("codex")]);
    assert_eq!(stage(&q, &seen_codex(quiet, None, Unknown)), "first-send");
    assert_eq!(stage(&q, &unestablished(quiet)), "first-send");
    assert_eq!(stage(&q, &seen_codex(quiet, None, Unavailable)), "agent");
    assert_eq!(
        ids(&select_due(&q, NOW, 720, &seen_codex(quiet, None, Trusted))),
        ["b"]
    );
    // a Claude interaction is not Codex evidence (and vice versa)
    assert!(select_due(&q, NOW, 720, &seen(quiet)).is_empty());
    let q = qs(vec![bootstrap_row("claude")]);
    assert!(select_due(&q, NOW, 720, &seen_codex(quiet, None, Trusted)).is_empty());
    // a session a successful listing proves absent is selected: the worker
    // may start it (and prepare_context_with never delivers to it)
    assert_eq!(ids(&select_due(&q, NOW, 720, &HashMap::new())), ["b"]);
    // any other process-bound program keeps its existing semantics
    for other in ["python3", "pyapp", "node"] {
        let q = qs(vec![bootstrap_row(other)]);
        assert_eq!(
            ids(&select_due(&q, NOW, 720, &unestablished(quiet))),
            ["b"],
            "{other}"
        );
        assert_eq!(
            hold_reason(&q.items[0], unestablished(quiet).get("s")),
            None
        );
    }
}

/// Fake start/probe/sleep for `prepare_context_with`: the session is absent
/// until started; every probe after the start is ready on pane 1.
struct FakeStart {
    started: AtomicBool,
    sleeps: Mutex<Vec<u64>>,
}

impl FakeStart {
    fn new() -> Self {
        Self {
            started: AtomicBool::new(false),
            sleeps: Mutex::new(Vec::new()),
        }
    }

    fn prepare(&self, item: &QueueItem) -> Prepared {
        prepare_context_with(
            item,
            &|| false,
            &StartOps {
                exists: &|_| self.started.load(std::sync::atomic::Ordering::SeqCst),
                start: &|_| {
                    self.started
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                    Ok(())
                },
                probe: &|_, _| probe_result(ContextStatus::Ready, ContextCode::ProcessMatched, 1),
                sleep: &|d| self.sleeps.lock_or_recover().push(d.as_millis() as u64),
            },
        )
    }
}

/// Starting a recognized agent is not delivery: bound at once, no settle
/// delay, `StartedAwaitingInteraction`. Any other process-bound program keeps
/// the fresh-start settle and is prepared for delivery as before.
#[test]
fn starting_a_recognized_agent_never_prepares_a_delivery() {
    for agent in ["claude", "codex"] {
        let fake = FakeStart::new();
        let prepared = fake.prepare(&bootstrap_row(agent));
        assert!(
            fake.started.load(std::sync::atomic::Ordering::SeqCst),
            "{agent}: the session is started"
        );
        assert!(
            matches!(&prepared, Prepared::StartedAwaitingInteraction(p) if p.is_ready()),
            "{agent}: {prepared:?}"
        );
        assert!(
            !fake
                .sleeps
                .lock_or_recover()
                .contains(&FRESH_START_SETTLE_MS),
            "{agent}: elapsed time never authorizes a first send"
        );
        // an existing session is only probed
        assert!(matches!(
            fake.prepare(&bootstrap_row(agent)),
            Prepared::Probe(_)
        ));
    }
    let fake = FakeStart::new();
    let prepared = fake.prepare(&bootstrap_row("pyapp"));
    assert!(matches!(&prepared, Prepared::Probe(p) if p.is_ready()));
    assert!(
        fake.sleeps
            .lock_or_recover()
            .contains(&FRESH_START_SETTLE_MS),
        "unchanged"
    );
}

/// The start-only outcome leaves NO delivery bookkeeping: the row stays
/// queued with no attempt, ledger, gap, fired count, group advance or
/// spawned step, and nothing is fired; the pane binding is recorded.
#[test]
fn a_started_agent_row_stays_pending_without_delivery_bookkeeping() {
    for agent in ["claude", "codex"] {
        let mut row = bootstrap_row(agent);
        row.tpl = Some("t".into());
        row.tpl_idx = Some(1);
        row.tpl_total = Some(2);
        row.steps = vec!["second".into()];
        let before = row.clone();
        let qm = Mutex::new(qs(vec![row]));
        let result = send_one_safe(
            &qm,
            &AtomicBool::new(false),
            "s",
            720,
            &HashMap::new(),
            &SendHooks {
                fire: &|_: &QueueItem| panic!("{agent}: a started agent is never typed into"),
                persist: &ok_persist,
                kill: &|_: &str| {},
            },
            &ContextHooks {
                prepare: &|_: &QueueItem, _: &dyn Fn() -> bool| {
                    Prepared::StartedAwaitingInteraction(probe_result(
                        ContextStatus::Ready,
                        ContextCode::ProcessMatched,
                        1,
                    ))
                },
                final_probe: &|_: &QueueItem| panic!("{agent}: no final probe"),
            },
        );
        assert_eq!(
            result,
            SendResult::StartedAwaitingInteraction {
                session: "s".into()
            }
        );
        let q = qm.lock_or_recover();
        assert_eq!(
            q.items.len(),
            1,
            "{agent}: consumed nothing, spawned nothing"
        );
        let it = &q.items[0];
        assert_eq!(it.state, before.state);
        assert_eq!(it.attempts, 0);
        assert_eq!(it.last_attempt_at, None);
        assert_eq!(it.delivery, None);
        assert_eq!(it.fired, 0);
        assert_eq!(it.steps, before.steps);
        assert!(q.pending.is_empty(), "{agent}: no delivery ledger");
        assert!(q.deliveries.is_empty(), "{agent}: no delivery record");
        assert!(!q.last_fired.contains_key("s"), "{agent}: no send gap");
        assert_eq!(
            it.binding,
            Some(pane(1)),
            "{agent}: the started pane is bound"
        );
    }
}

// ---------- real processes: a fake startup modal owns Enter ----------------

/// A throwaway bundled-tmux server (socket name, tmux binary).
struct ModalServer(String, std::path::PathBuf);

impl ModalServer {
    fn run(&self, args: &[&str]) -> Result<String, DeckError> {
        let out = std::process::Command::new(&self.1)
            .args(["-f", "/dev/null", "-L", &self.0])
            .args(args)
            .output()
            .map_err(DeckError::from)?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(DeckError::new(crate::error::ErrorKind::Tmux, "tmux failed"))
        }
    }
}

impl Drop for ModalServer {
    fn drop(&mut self) {
        let _ = self.run(&["kill-server"]);
    }
}

/// Run one automatic send of a due row configured for `argv0` into a session
/// the listing proves absent, on a throwaway server whose pane program is a
/// fake startup modal (perl under `exec -a <argv0>`): it enables bracketed
/// paste like an agent TUI, draws an update-style menu and records every
/// byte its stdin receives. The delivery path is the real one
/// (`send_one_safe` → `prepare_context_with` → start, readiness probe of the
/// real process, settle, final probe) with the throwaway server swapped in
/// for Deck's socket; `fire` pastes the text and sends Enter the way
/// `prompt_delivery` does. Returns the send result and the recorded bytes.
fn send_into_fake_modal(argv0: &str) -> (SendResult, String) {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("deck-modal-{}-{seq}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("stdin.hex");
    std::fs::write(&log, "").unwrap();
    let script = dir.join("modal.pl");
    std::fs::write(
        &script,
        r#"$| = 1; my $log = shift;
print "\e[?2004h", "Update available\r\n> 1. Update now\r\n  2. Skip\r\nenter continue\r\n";
system("stty raw -echo");
open(my $l, ">>", $log) or die; select($l); $| = 1;
while (sysread(STDIN, my $b, 1)) { printf $l "%02x", ord($b); }
"#,
    )
    .unwrap();
    let server = ModalServer(
        format!("deck-test-modal-{}-{seq}", std::process::id()),
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("binaries/tmux-aarch64-apple-darwin"),
    );
    let program = format!(
        "exec -a {argv0} /usr/bin/perl '{}' '{}'",
        script.display(),
        log.display()
    );
    let rows =
        || crate::tmux::list_panes_with(&|args: &[&str]| server.run(args)).unwrap_or_default();
    let probe = |item: &QueueItem, identity: Option<&PaneIdentity>| match rows()
        .into_iter()
        .find(|row| row.session_name == item.session)
    {
        Some(row) => crate::context::evaluate(
            &crate::context::raw_probe_of_row(&row),
            identity,
            item.expected_process.as_deref(),
        ),
        None => ProbeResult::blocked(ContextStatus::Unavailable, ContextCode::SessionMissing),
    };
    let ops = StartOps {
        exists: &|item| {
            server
                .run(&["has-session", "-t", &format!("={}", item.session)])
                .is_ok()
        },
        start: &|item| {
            server
                .run(&[
                    "new-session",
                    "-d",
                    "-s",
                    &item.session,
                    "-x",
                    "80",
                    "-y",
                    "12",
                    &program,
                ])
                .map(|_| ())
        },
        probe: &probe,
        // keep the real ordering, compress the waits
        sleep: &|d| std::thread::sleep(d.min(std::time::Duration::from_millis(300))),
    };
    let mut row = bootstrap_row(argv0);
    row.session = format!("modal-{seq}");
    let session = row.session.clone();
    let qm = Mutex::new(qs(vec![row]));
    let fire = |item: &QueueItem| -> Result<(), DeckError> {
        let target = format!("={}:", item.session);
        server.run(&["send-keys", "-t", &target, "-l", &item.text])?;
        server
            .run(&["send-keys", "-t", &target, "Enter"])
            .map(|_| ())
    };
    let result = send_one_safe(
        &qm,
        &AtomicBool::new(false),
        &session,
        720,
        &HashMap::new(),
        &SendHooks {
            fire: &fire,
            persist: &ok_persist,
            kill: &|_: &str| {},
        },
        &ContextHooks {
            prepare: &|item: &QueueItem, cancelled: &dyn Fn() -> bool| {
                prepare_context_with(item, cancelled, &ops)
            },
            final_probe: &|item: &QueueItem| probe(item, item.binding.as_ref()),
        },
    );
    // give any byte that was sent time to reach the program
    std::thread::sleep(std::time::Duration::from_millis(700));
    let bytes = std::fs::read_to_string(&log).unwrap();
    drop(server);
    let _ = std::fs::remove_dir_all(&dir);
    (result, bytes)
}

/// The real regression: a freshly started Codex/Claude whose startup modal
/// owns Enter receives ZERO bytes and no Enter from the automatic path. The
/// same harness with a non-agent program still delivers (text + Enter), so
/// the harness can see bytes and non-agent semantics are unchanged.
#[test]
fn a_fresh_agent_s_startup_modal_receives_no_automatic_bytes() {
    for agent in ["codex", "claude"] {
        let (result, bytes) = send_into_fake_modal(agent);
        assert_eq!(
            bytes, "",
            "{agent}: no prompt bytes and no Enter reached the modal"
        );
        assert!(
            matches!(result, SendResult::StartedAwaitingInteraction { .. }),
            "{agent}: {result:?}"
        );
    }
    let (result, bytes) = send_into_fake_modal("pyapp");
    assert!(matches!(result, SendResult::Sent { .. }), "{result:?}");
    assert!(
        bytes.ends_with("0d"),
        "the text and its Enter arrived: {bytes}"
    );
    assert!(bytes.len() > 2);
}
