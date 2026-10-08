//! Channel permission and readiness are independent, with no live transports.
use super::*;
use crate::agent_status;
use crate::inbound_channel::{
    self as channel, ChannelConfig, ChannelConnection, ChannelRule, Identity,
};
use serde_json::{json, Value};

fn fixture() -> (ChannelRule, Value, Identity) {
    let rule = serde_json::from_value(json!({
        "id":"r1", "enabled":true, "firstSend":true, "connectionId":"default",
        "channelIds":["C1","C2"], "senderUserIds":["U2","U3"], "senderBotIds":[],
        "match":{"kind":"keywords","keywords":["alpha","beta"],"caseSensitive":false},
        "includeThreads":true,"projectId":"P1","columnId":"L1",
        "dir":std::env::temp_dir().to_string_lossy(), "cmd":"claude", "template":"triage","idleMinutes":15
    })).unwrap();
    let board = json!({"projects":[{"id":"P1","templates":[{"name":"triage","steps":["Inspect {{msg.text}}","Later"]}]}]});
    let identity = Identity {
        team_id: "T1".into(),
        own_user_id: "U1".into(),
        own_bot_id: "B1".into(),
        app_id: "A1".into(),
    };
    (rule, board, identity)
}

fn authorized() -> (QueueItem, ChannelConfig, Value) {
    let (mut rule, board, identity) = fixture();
    let grant =
        channel::issue_grant(&rule, &board, &identity, true, 2_000_000_000_000_001).unwrap();
    let mut row: QueueItem = serde_json::from_value(json!({
        "id":"q1", "session":"test-session", "card_id":"card1", "dir":rule.target.dir,
        "cmd":"claude", "text":"Inspect fixture", "mode":"at", "at":1,"added":1,
        "external":true,"expected_process":"claude"
    }))
    .unwrap();
    row.channel_first_send = Some(ChannelFirstSendConstraint {
        rule: rule.id.clone(),
        inbox: "default/T1/E1/r1".into(),
        grant_id: grant.id.clone(),
        grant_digest: grant.digest.clone(),
        operation: "operation1".into(),
        workspace: "T1".into(),
        authorized: true,
    });
    rule.first_send_grant = Some(grant);
    (
        row,
        ChannelConfig {
            connection: ChannelConnection {
                enabled: true,
                connection_id: "default".into(),
            },
            rules: vec![rule],
        },
        board,
    )
}

#[test]
fn channel_permission_revocation_holds_with_and_without_interaction_or_session() {
    let (mut row, _, _) = authorized();
    row.channel_first_send.as_mut().unwrap().authorized = false;
    for observed in [
        None,
        Some(Observed::default()),
        Some(Observed {
            claude_interaction: true,
            ..Observed::default()
        }),
    ] {
        assert_eq!(
            hold_reason(&row, observed.as_ref()),
            Some(Hold::ChannelPermission)
        );
        assert!(!relies_on_readiness_override(&row, observed.as_ref()));
    }
}

#[test]
fn channel_readiness_audit_counts_only_the_missing_interaction_dependency() {
    let (row, _, _) = authorized();
    assert!(relies_on_readiness_override(
        &row,
        Some(&Observed::default())
    ));
    assert!(!relies_on_readiness_override(
        &row,
        Some(&Observed {
            claude_interaction: true,
            ..Observed::default()
        })
    ));
    assert!(
        row.readiness_override.is_none(),
        "the channel source remains separate from legacy overrides"
    );
}

#[test]
fn channel_permission_does_not_lift_needs_input_codex_unavailable_or_followup_hold() {
    let (mut row, _, _) = authorized();
    assert_eq!(
        hold_reason(
            &row,
            Some(&Observed {
                agent: Some(agent_status::NEEDS_INPUT),
                ..Observed::default()
            })
        ),
        Some(Hold::NeedsInput)
    );
    row.cmd = "codex --no-daemon".into();
    assert_eq!(
        hold_reason(
            &row,
            Some(&Observed {
                codex: Some(agent_status::CodexSignalTrust::Unavailable),
                ..Observed::default()
            })
        ),
        Some(Hold::CodexUnavailable)
    );
    row.mode = "chain".into();
    assert_eq!(
        hold_reason(
            &row,
            Some(&Observed {
                claude_interaction: true,
                ..Observed::default()
            })
        ),
        Some(Hold::External)
    );
}

#[test]
fn channel_unreadability_preserves_permission_and_recovers_without_reacceptance() {
    let (row, cfg, board) = authorized();
    assert!(matches!(
        channel_first_send::standing(&row, None, Some(&board)),
        channel_first_send::Standing::Unverified
    ));
    assert!(matches!(
        channel_first_send::standing(&row, Some(&cfg), None),
        channel_first_send::Standing::Unverified
    ));
    assert_eq!(
        hold_reason(
            &row,
            Some(&Observed {
                channel_unverified: true,
                claude_interaction: true,
                ..Observed::default()
            })
        ),
        Some(Hold::ChannelUnverified)
    );
    assert!(row.channel_first_send.as_ref().unwrap().authorized);
    assert!(matches!(
        channel_first_send::standing(&row, Some(&cfg), Some(&board)),
        channel_first_send::Standing::Clear
    ));
}

#[test]
fn channel_unverified_attention_waits_for_a_lasting_hold_and_recovers() {
    let (row, _, _) = authorized();
    let queue = QueueState {
        items: vec![row],
        ..QueueState::default()
    };
    let mut observed = Observations::from([(
        "s".into(),
        Observed {
            channel_unverified: true,
            claude_interaction: true,
            ..Observed::default()
        },
    )]);
    let session = queue.items[0].session.clone();
    let fact = observed.remove("s").unwrap();
    observed.insert(session.clone(), fact);
    let mut since = UnverifiedSince::new();
    let initial = track_unverified(&queue, 100, 720, Some(&observed), &mut since);
    assert!(initial.is_empty());
    assert!(
        delivery_waits(&queue, 100, 720, Some(&observed), &HashMap::new(), &initial).is_empty()
    );
    let lasting = track_unverified(
        &queue,
        100 + UNVERIFIED_ANNOUNCE_SECS,
        720,
        Some(&observed),
        &mut since,
    );
    assert_eq!(
        delivery_waits(
            &queue,
            100 + UNVERIFIED_ANNOUNCE_SECS,
            720,
            Some(&observed),
            &HashMap::new(),
            &lasting
        )[&session]
            .stage,
        "channel-unverified"
    );
    observed.get_mut(&session).unwrap().channel_unverified = false;
    assert!(track_unverified(
        &queue,
        101 + UNVERIFIED_ANNOUNCE_SECS,
        720,
        Some(&observed),
        &mut since
    )
    .is_empty());
    assert!(since.is_empty());
    assert!(
        queue.items[0]
            .channel_first_send
            .as_ref()
            .unwrap()
            .authorized
    );
}

#[test]
fn channel_text_edit_and_revert_keep_the_constraint_withdrawn_and_revision_changed() {
    let (row, _, _) = authorized();
    let original = row.text.clone();
    let mut queue = QueueState {
        items: vec![row],
        ..QueueState::default()
    };
    ops::update_text(&mut queue, "q1", "Edited".into()).unwrap();
    ops::update_text(&mut queue, "q1", original).unwrap();
    assert_eq!(queue.items[0].revision, 2);
    assert!(
        !queue.items[0]
            .channel_first_send
            .as_ref()
            .unwrap()
            .authorized
    );
    assert!(queue.items[0].external);
}

#[test]
fn channel_legacy_rows_have_no_new_permission_or_readiness_override() {
    let (mut row, _, _) = authorized();
    row.channel_first_send = None;
    assert_eq!(
        hold_reason(&row, Some(&Observed::default())),
        Some(Hold::FirstInteraction)
    );
    assert_eq!(
        hold_reason(
            &row,
            Some(&Observed {
                claude_interaction: true,
                ..Observed::default()
            })
        ),
        None
    );
    assert!(!relies_on_readiness_override(
        &row,
        Some(&Observed::default())
    ));
}

#[test]
fn channel_grant_ids_do_not_reuse_a_second_or_a_rolled_back_timestamp() {
    let (rule, board, identity) = fixture();
    let a = channel::issue_grant(&rule, &board, &identity, true, 2_000_000_000_000_001).unwrap();
    let b = channel::issue_grant(&rule, &board, &identity, true, a.issued_at_micros).unwrap();
    let c = channel::issue_grant(&rule, &board, &identity, true, 1_000_000_000_000_001).unwrap();
    assert_ne!(a.id, b.id);
    assert_ne!(a.id, c.id);
    assert_ne!(a.digest, b.digest);
    assert_ne!(a.digest, c.digest);
}

#[test]
fn channel_grant_semantics_ignore_collection_order_columns_names_and_later_steps() {
    let (row, mut cfg, mut board) = authorized();
    let rule = &mut cfg.rules[0];
    rule.channel_ids = vec!["C2".into(), "C1".into(), "C1".into()];
    rule.sender_user_ids.reverse();
    rule.matcher.keywords.reverse();
    rule.target.column_id = "L2".into();
    board["projects"][0]["templates"][0]["steps"][1] = json!("Different later step");
    assert!(matches!(
        channel_first_send::standing(&row, Some(&cfg), Some(&board)),
        channel_first_send::Standing::Clear
    ));
    // An atomic display rename with its reference kept consistent changes no recipe.
    cfg.rules[0].target.template = "renamed".into();
    board["projects"][0]["templates"][0]["name"] = json!("renamed");
    assert!(matches!(
        channel_first_send::standing(&row, Some(&cfg), Some(&board)),
        channel_first_send::Standing::Clear
    ));
}

#[test]
fn channel_scope_target_or_effective_head_change_invalidates_current_grant() {
    let (row, cfg, board) = authorized();
    let mut changes: Vec<ChannelConfig> = Vec::new();
    let mut c = cfg.clone();
    c.connection.enabled = false;
    changes.push(c);
    let mut c = cfg.clone();
    c.rules[0].enabled = false;
    changes.push(c);
    let mut c = cfg.clone();
    c.rules[0].first_send = false;
    changes.push(c);
    let mut c = cfg.clone();
    c.rules[0].target.idle_minutes += 1;
    changes.push(c);
    let mut c = cfg.clone();
    c.rules[0].channel_ids.push("C3".into());
    changes.push(c);
    let mut c = cfg.clone();
    c.rules[0].target.cmd = "claude --verbose".into();
    changes.push(c);
    for changed in changes {
        assert!(matches!(
            channel_first_send::standing(&row, Some(&changed), Some(&board)),
            channel_first_send::Standing::Revoked
        ));
    }
    let mut changed = board.clone();
    changed["projects"][0]["templates"][0]["steps"][0] = json!("A different head");
    assert!(matches!(
        channel_first_send::standing(&row, Some(&cfg), Some(&changed)),
        channel_first_send::Standing::Revoked
    ));
}

#[test]
fn channel_new_grant_does_not_reauthorize_an_old_constrained_row() {
    let (row, mut cfg, board) = authorized();
    let (_, _, identity) = fixture();
    cfg.rules[0].first_send_grant = Some(
        channel::issue_grant(
            &cfg.rules[0],
            &board,
            &identity,
            true,
            2_000_000_000_000_001,
        )
        .unwrap(),
    );
    assert!(matches!(
        channel_first_send::standing(&row, Some(&cfg), Some(&board)),
        channel_first_send::Standing::Revoked
    ));
}

#[test]
fn channel_bounded_head_requires_external_acceptance_and_empty_head_is_not_skipped() {
    let (rule, mut board, identity) = fixture();
    assert!(channel::issue_grant(&rule, &board, &identity, false, 100).is_err());
    board["projects"][0]["templates"][0]["steps"][0] = json!(" \t ");
    assert!(channel::issue_grant(&rule, &board, &identity, true, 100).is_err());
    board["projects"][0]["templates"][0]["steps"][0] = json!("{{msg.text}} then inspect");
    assert!(channel::issue_grant(&rule, &board, &identity, true, 100).is_err());
}

#[test]
fn channel_permission_preserves_pause_ambiguity_gap_and_group_order() {
    let (row, _, _) = authorized();
    let observed = Observations::from([(row.session.clone(), Observed::default())]);
    let baseline = QueueState {
        items: vec![row],
        ..QueueState::default()
    };
    assert!(select_for_session(&baseline, "test-session", 1000, 0, &observed).is_some());
    let mut paused = baseline.clone();
    paused.items[0].paused = true;
    assert!(select_for_session(&paused, "test-session", 1000, 0, &observed).is_none());
    let mut ambiguous = baseline.clone();
    ambiguous.items[0].state = ItemState::Ambiguous;
    assert!(select_for_session(&ambiguous, "test-session", 1000, 0, &observed).is_none());
    let mut gap = baseline.clone();
    gap.last_fired.insert("test-session".into(), 1000);
    assert!(select_for_session(&gap, "test-session", 1000, 0, &observed).is_none());
    let mut grouped = baseline.clone();
    grouped.items[0].group = Some("group".into());
    grouped.items[0].seq = Some(2);
    let mut predecessor = grouped.items[0].clone();
    predecessor.id = "earlier".into();
    predecessor.seq = Some(1);
    predecessor.paused = true;
    grouped.items.push(predecessor);
    assert!(select_for_session(&grouped, "test-session", 1000, 0, &observed).is_none());
}

#[test]
fn channel_manual_selection_is_one_action_without_restoring_rule_permission() {
    let (mut row, _, _) = authorized();
    row.channel_first_send.as_mut().unwrap().authorized = false;
    let queue = QueueState {
        items: vec![row],
        ..QueueState::default()
    };
    assert!(select_for_session(&queue, "test-session", 1000, 0, &Observations::new()).is_none());
    assert!(select::select_requested(&queue, "test-session", "q1", 1000).is_some());
    assert!(
        !queue.items[0]
            .channel_first_send
            .as_ref()
            .unwrap()
            .authorized
    );
}

#[test]
fn channel_missing_startup_directory_does_not_revoke_unchanged_permission() {
    let (mut row, mut config, board) = authorized();
    let (_, _, identity) = fixture();
    let dir = std::env::temp_dir().join(crate::ledger::random_id("channel-dir-", 8).unwrap());
    std::fs::create_dir(&dir).unwrap();
    config.rules[0].target.dir = dir.to_string_lossy().into_owned();
    let grant = channel::issue_grant(&config.rules[0], &board, &identity, true, 100).unwrap();
    row.channel_first_send.as_mut().unwrap().grant_id = grant.id.clone();
    row.channel_first_send.as_mut().unwrap().grant_digest = grant.digest.clone();
    config.rules[0].first_send_grant = Some(grant);
    std::fs::remove_dir(&dir).unwrap();
    assert!(matches!(
        channel_first_send::standing(&row, Some(&config), Some(&board)),
        channel_first_send::Standing::Clear
    ));
    std::fs::create_dir(&dir).unwrap();
    assert!(matches!(
        channel_first_send::standing(&row, Some(&config), Some(&board)),
        channel_first_send::Standing::Clear
    ));
    std::fs::remove_dir(dir).unwrap();
}

/// Called by the isolated disk-backed authority subprocess. No tmux, network,
/// Agent, or clipboard is used; only the production firing transaction runs.
pub(crate) fn verify_native_channel_fences(
    active_settings: &str,
    disabled_settings: &str,
    commit_disabled_locked: &(dyn Fn() + Sync),
) {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;
    for (before, established) in [(true, false), (true, true), (false, false), (false, true)] {
        crate::documents::save_settings(
            active_settings.to_owned(),
            Some(vec![channel::ChannelFirstSendRequest {
                rule_id: "R1".into(),
                external: true,
                identity: None,
            }]),
        )
        .unwrap();
        let cfg = channel::read_config_strict_result().unwrap();
        let grant = cfg.rules[0].first_send_grant.as_ref().unwrap();
        let mut row: QueueItem = serde_json::from_value(json!({"id":"fence-row","session":"deck-test-channel","card_id":"S1",
            "dir":grant.dir,"cmd":"claude","expected_process":"claude","text":"Inspect INCIDENT fixture","mode":"at","at":1,"added":1,"external":true})).unwrap();
        row.channel_first_send = Some(ChannelFirstSendConstraint {
            rule: "R1".into(),
            inbox: "default/T1/E1/R1".into(),
            grant_id: grant.id.clone(),
            grant_digest: grant.digest.clone(),
            operation: "fence-op".into(),
            workspace: "T1".into(),
            authorized: true,
        });
        let qm = Mutex::new(QueueState {
            items: vec![row],
            ..QueueState::default()
        });
        let fired = AtomicBool::new(false);
        let dirty = AtomicBool::new(false);
        let observed = Observations::from([(
            "deck-test-channel".into(),
            Observed {
                claude_interaction: established,
                ..Observed::default()
            },
        )]);
        let fire = |_: &QueueItem| {
            assert!(!crate::storage::settings_fence_busy());
            assert!(!crate::documents::board_fence_busy());
            assert_eq!(qm.lock_or_recover().items[0].state, ItemState::Firing);
            fired.store(true, Ordering::SeqCst);
            if !before {
                crate::documents::save_settings(disabled_settings.to_owned(), None).unwrap();
            }
            Ok(())
        };
        let persist = |q: &QueueState| {
            if q.items.iter().any(|row| row.state == ItemState::Firing) {
                assert!(crate::storage::settings_fence_busy());
                assert!(crate::documents::board_fence_busy());
                let attempt = q.pending[0].channel_attempt.as_ref().unwrap();
                assert!(attempt.automatic);
                assert_eq!(attempt.readiness_overridden, !established);
            }
            save_queue(q)
        };
        let hooks = SendHooks {
            fire: &fire,
            persist: &persist,
            kill: &|_| {},
            authority: &|| None,
            board: &|| None,
        };
        let no_start = ContextHooks {
            prepare: &|_, _| {
                panic!("a denied or unverifiable channel row must not start a session")
            },
            final_probe: &|_| panic!("no context probe is allowed"),
        };
        if before && !established {
            let path = crate::documents::settings_path();
            let held_path = path.with_extension("temporarily-unreadable");
            std::fs::rename(&path, &held_path).unwrap();
            let unavailable = send_one_safe(
                &qm,
                &dirty,
                "deck-test-channel",
                0,
                &Observations::new(),
                &hooks,
                &no_start,
            );
            std::fs::rename(&held_path, &path).unwrap();
            assert_eq!(unavailable, SendResult::Nothing);
            assert!(
                qm.lock_or_recover().items[0]
                    .channel_first_send
                    .as_ref()
                    .unwrap()
                    .authorized
            );
        }
        let result = if before {
            std::thread::scope(|scope| {
                let settings_guard = crate::storage::settings_fence();
                let board_guard = crate::documents::board_fence();
                let (tx, rx) = std::sync::mpsc::sync_channel(0);
                let (queue_ref, dirty_ref, observations_ref, hooks_ref) =
                    (&qm, &dirty, &observed, &hooks);
                let worker = scope.spawn(move || {
                    tx.send(()).unwrap();
                    send_one(
                        queue_ref,
                        dirty_ref,
                        "deck-test-channel",
                        0,
                        observations_ref,
                        hooks_ref,
                    )
                });
                rx.recv().unwrap();
                commit_disabled_locked();
                assert!(!fired.load(Ordering::SeqCst));
                drop(board_guard);
                drop(settings_guard);
                worker.join().unwrap()
            })
        } else {
            send_one(&qm, &dirty, "deck-test-channel", 0, &observed, &hooks)
        };
        if before {
            assert_eq!(result, SendResult::Nothing);
            assert!(!fired.load(Ordering::SeqCst));
            assert_eq!(
                send_one_safe(
                    &qm,
                    &dirty,
                    "deck-test-channel",
                    0,
                    &Observations::new(),
                    &hooks,
                    &no_start
                ),
                SendResult::Nothing
            );
            assert!(
                !qm.lock_or_recover().items[0]
                    .channel_first_send
                    .as_ref()
                    .unwrap()
                    .authorized
            );
        } else {
            assert!(matches!(result, SendResult::Sent { .. }));
            assert!(fired.load(Ordering::SeqCst));
            let q = qm.lock_or_recover();
            assert_eq!(q.deliveries.len(), 1);
            assert!(q.deliveries[0].automatic);
            assert_eq!(q.deliveries[0].readiness_overridden, !established);
            assert!(q.deliveries[0].channel_first_send.is_some());
        }
    }
}

#[test]
fn channel_firing_audit_survives_reload_without_inventing_success() {
    for (automatic, overridden) in [(true, true), (true, false), (false, false)] {
        let (mut row, _, _) = authorized();
        row.state = ItemState::Firing;
        row.delivery = Some("uncertain-channel".into());
        let mut queue = QueueState {
            items: vec![row.clone()],
            pending: vec![PendingDelivery {
                id: "uncertain-channel".into(),
                snapshot: row.clone(),
                channel_attempt: Some(channel_first_send::ChannelAttemptAudit {
                    automatic,
                    readiness_overridden: overridden,
                }),
            }],
            ..QueueState::default()
        };
        queue = serde_json::from_value(serde_json::to_value(queue).unwrap()).unwrap();
        assert!(
            queue.deliveries.is_empty(),
            "firing is not a successful delivery"
        );
        let audit = queue.pending[0].channel_attempt.as_ref().unwrap();
        assert_eq!(
            (audit.automatic, audit.readiness_overridden),
            (automatic, overridden)
        );
        // Only an explicit user acknowledgement may account the uncertain attempt.
        finalize_delivery(&mut queue, &row.id, "uncertain-channel", 100, true);
        assert!(queue.deliveries[0].assumed);
        assert_eq!(queue.deliveries[0].automatic, automatic);
        assert_eq!(queue.deliveries[0].manual, !automatic);
        assert_eq!(queue.deliveries[0].readiness_overridden, overridden);
    }
}

/// What a disk-backed closure test may read of a row the probe admitted.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProbeRow {
    pub(crate) operation: Option<String>,
    pub(crate) mode: String,
    pub(crate) external: bool,
    pub(crate) authority: bool,
    pub(crate) readiness_override: bool,
    /// `None`: no channel constraint; `Some(authorized)` otherwise.
    pub(crate) constraint: Option<bool>,
}

/// The external queue entries, minus Tauri: each call runs the admission
/// steps of `channel_queue_add*` in the command's own order against real
/// settings, Board and inbox files, on a queue this probe owns. No tmux,
/// network, Agent or clipboard is used.
pub(crate) struct QueueProbe(std::sync::Mutex<QueueState>);

impl QueueProbe {
    pub(crate) fn new() -> Self {
        Self(std::sync::Mutex::new(QueueState::default()))
    }

    /// `channel_queue_add`: `Ok(true)` appended, `Ok(false)` an exact replay.
    pub(crate) fn external_add(&self, request: Value) -> Result<bool, DeckError> {
        let mut args: QueueAddArgs = serde_json::from_value(request).expect("queue request");
        ops::admit_external(&mut args)?;
        let text = normalize_prompt(&args.text);
        validate_add(&args)?;
        if operation_replay(&self.0, &args, &text)? {
            return Ok(false);
        }
        let fingerprint = args
            .operation_id
            .as_ref()
            .map(|_| operation_fingerprint(&args, &text));
        if args.channel_first_send.is_some() {
            args.dir = channel::normalized_dir(&args.dir)
                .ok_or_else(|| DeckError::new(ErrorKind::Invalid, "directory unavailable"))?;
        }
        ops::admit_authority(&mut args, std::slice::from_ref(&text));
        ops::admit_first_send(&mut args);
        args.channel_first_send_granted =
            channel_first_send::admit(&args, crate::datadir::now_epoch())?;
        let uncertain = channel_first_send::rollback_uncertain(&args, None)?;
        let expected = crate::context::expected_from_command(&args.cmd);
        let mut queue = self.0.lock_or_recover();
        ops::add_item_bound(
            &mut queue,
            args,
            text,
            None,
            expected,
            fingerprint,
            uncertain,
        )?;
        Ok(true)
    }

    /// `channel_queue_add_reviewed_list`, up to the owner core.
    pub(crate) fn external_reviewed(&self, request: Value) -> Result<(), DeckError> {
        let mut args: QueueAddArgs = serde_json::from_value(request).expect("queue request");
        ops::admit_external(&mut args)?;
        channel_first_send::reject_reviewed_list(&args)
    }

    pub(crate) fn rows(&self) -> Vec<ProbeRow> {
        let queue = self.0.lock_or_recover();
        queue
            .items
            .iter()
            .map(|item| ProbeRow {
                operation: item.operation_id.clone(),
                mode: item.mode.clone(),
                external: item.external,
                authority: item.authority.is_some(),
                readiness_override: item.readiness_override.is_some(),
                constraint: item.channel_first_send.as_ref().map(|c| c.authorized),
            })
            .collect()
    }

    /// A row the previous release queued: the owner core as it was, with
    /// the fingerprint of the request as sent (no channel admission existed).
    pub(crate) fn seed_legacy(&self, request: Value) {
        let mut args: QueueAddArgs = serde_json::from_value(request).expect("queue request");
        ops::admit_external(&mut args).unwrap();
        let text = normalize_prompt(&args.text);
        let fingerprint = operation_fingerprint(&args, &text);
        let expected = crate::context::expected_from_command(&args.cmd);
        let mut queue = self.0.lock_or_recover();
        ops::add_item_bound(
            &mut queue,
            args,
            text,
            None,
            expected,
            Some(fingerprint),
            false,
        )
        .unwrap();
    }

    /// The row left the queue (it was sent); its operation record stays.
    pub(crate) fn sent(&self, operation: &str) {
        let mut queue = self.0.lock_or_recover();
        queue
            .items
            .retain(|item| item.operation_id.as_deref() != Some(operation));
        assert!(queue.operations.iter().any(|op| op.id == operation));
    }
}
