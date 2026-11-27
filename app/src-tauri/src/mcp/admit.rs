//! Worker lane admission: how a start, line, interjected line or close of
//! one orchestrator card's lane (`origin.rs` `Client.lanes`, or the
//! client's own lane for a caller from no card) is bound so that a request
//! runs at most once, also after its journal record was retired.
//!
//! # Contract
//! - **Two bindings, exactly one per request.** `send_sequence` (the
//!   original binding, unchanged): Deck accepts the request only when it
//!   names the lane's current sequence, and acceptance moves the sequence
//!   on. Requests of one lane under it go one at a time: a request whose
//!   precondition fails leaves the sequence where it was, so the next one
//!   prepared for `n + 1` is refused too. `lane_epoch` (approved
//!   2026-11-28): the request names the lane's current epoch
//!   (`laneEpoch` in `deck_worker_list` / `deck_worker_wait` and in every
//!   receipt) and needs no guess of what was accepted before it, so
//!   independent requests of one lane may be sent at once, in any order,
//!   and one refused for its own reason refuses nothing else. A request
//!   naming both or neither is `INVALID_ARGUMENTS`; the binding is part of
//!   the request's identity (`request_hash`), so one request id never
//!   names two bindings, and Deck never moves an old request to another
//!   sequence or epoch.
//! - **Why a retired request never runs again.** Under `send_sequence`:
//!   the request's own acceptance moved the sequence past it, so once its
//!   record is retired (`state::compact`, the per-client window) its replay
//!   fails the sequence check. Under `lane_epoch`: every acceptance leaves
//!   a tombstone (`Admitted`: client, lane, epoch, request id, request
//!   hash, operation id), written in the SAME ledger write as the
//!   operation and before any side effect. Deck accepts a request only at
//!   the lane's current epoch `E` or the one before it (`E - 1`, so
//!   requests in flight when the epoch moves are not refused), and for
//!   those two epochs the tombstones are complete: a tombstone is dropped
//!   only together with its whole epoch, when the lane moves past it. So a
//!   request id at `E` or `E - 1` without a tombstone was never accepted,
//!   one with a tombstone is answered from it (the operation it became,
//!   never a second run; a different hash is `REQUEST_ID_CONFLICT`), and a
//!   request at an older epoch is refused `STALE_REQUEST`
//!   (`relation: "retired"`) with no claim about whether it ran: Deck no
//!   longer knows. An epoch Deck never issued (`> E`) is refused
//!   (`relation: "ahead"`).
//! - **Bounded.** A lane moves to the next epoch after `EPOCH_SPAN`
//!   acceptances in its current one; a client keeps at most
//!   `MAX_ADMITTED_PER_CLIENT` tombstones and the ledger `MAX_ADMITTED`, and
//!   past either bound the lane holding the oldest tombstone moves on
//!   (its older epoch, then if need be its current one, retire whole).
//!   Tombstones of a lane `origin.rs` dropped (its card left the Board)
//!   leave with it.
//! - **Nothing else changes.** Every precondition of the tool (the
//!   worker choice, the card, the episode, the human hold, `needs-input`,
//!   capacity, the rate, the unsent bound) is checked as before, for each
//!   request on its own; the epoch is never a precondition the caller's
//!   state must match beyond the lane itself, and Deck never updates
//!   `after_episode` or anything else for the caller. An accepted close
//!   still frees no capacity until its card is gone.
//! - **Receipts.** Every answer of these tools carries `lane`: the binding
//!   it was checked under and the lane's `nextSendSequence` and `laneEpoch`
//!   after it, so a caller never guesses whether a sequence moved. A
//!   refusal of the binding is `STALE_REQUEST` with `scope:
//!   "worker-lane"`, what was received and expected, the relation
//!   (`ahead`, `behind`, `retired`), `accepted: false` for THIS attempt, and
//!   the worker tools to read the lane from — never the session or create
//!   sequences.
//! - A ledger holding a tombstone or an epoch is written at
//!   `STATE_VERSION_LANE_EPOCHS` (sticky).

use super::*;

/// Acceptances in one lane's epoch before the lane moves to the next.
pub(super) const EPOCH_SPAN: usize = 64;
/// Tombstones one client keeps (all its lanes).
pub(super) const MAX_ADMITTED_PER_CLIENT: usize = 192;
/// Tombstones the ledger keeps.
pub(super) const MAX_ADMITTED: usize = 2048;

/// One request accepted under `lane_epoch`.
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Admitted {
    pub(super) client_id: String,
    /// The orchestrator card of the lane; `None` for the client's own lane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) lane: Option<String>,
    pub(super) epoch: u64,
    pub(super) request_id: String,
    pub(super) request_hash: String,
    pub(super) operation_id: String,
    pub(super) kind: String,
    pub(super) at: u64,
}

/// How a worker request is bound (exactly one).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Binding {
    Sequence(u64),
    Epoch(u64),
}

impl Binding {
    pub(super) fn of(send_sequence: Option<u64>, lane_epoch: Option<u64>) -> Result<Self, Value> {
        match (send_sequence, lane_epoch) {
            (Some(n), None) => Ok(Binding::Sequence(n)),
            (None, Some(e)) => Ok(Binding::Epoch(e)),
            _ => Err(error_value(
                "INVALID_ARGUMENTS",
                "name exactly one of send_sequence and lane_epoch",
                "Use lane_epoch (laneEpoch from deck_worker_list or deck_worker_wait) to send several requests at once; send_sequence binds one request at a time.",
            )),
        }
    }
}

pub(super) fn uses_lane_epochs(doc: &DiskDoc) -> bool {
    !doc.admitted.is_empty()
        || doc
            .config
            .clients
            .iter()
            .any(|c| c.send_epoch > 0 || c.lanes.values().any(|l| l.epoch > 0))
}

pub(super) fn valid_admitted(doc: &DiskDoc) -> bool {
    doc.admitted.len() <= MAX_ADMITTED
        && doc.admitted.iter().all(|a| {
            valid_id(&a.client_id)
                && a.lane.as_deref().is_none_or(valid_id)
                && valid_id(&a.request_id)
                && a.request_hash.len() == 64
                && valid_id(&a.operation_id)
                && valid_id(&a.kind)
        })
}

/// The lane's current epoch.
pub(super) fn epoch(client: &Client, lane: Option<&str>) -> u64 {
    match lane {
        Some(card) => client.lanes.get(card).map_or(0, |l| l.epoch),
        None => client.send_epoch,
    }
}

/// The lane as a receipt says it: the binding checked and where the lane
/// stands now.
pub(super) fn lane_view(client: &Client, lane: Option<&str>, binding: Option<Binding>) -> Value {
    let mut view = json!({
        "nextSendSequence": super::origin::send_sequence(client, lane),
        "laneEpoch": epoch(client, lane),
    });
    match binding {
        Some(Binding::Sequence(n)) => {
            view["binding"] = json!("send_sequence");
            view["sendSequence"] = json!(n);
        }
        Some(Binding::Epoch(e)) => {
            view["binding"] = json!("lane_epoch");
            view["epoch"] = json!(e);
        }
        None => {}
    }
    view
}

/// The tombstone of this client's request id, if Deck keeps one: `Err` when
/// it was accepted with other arguments (or another binding).
pub(super) fn tombstone<'a>(
    doc: &'a DiskDoc,
    client_id: &str,
    request_id: &str,
    hash: &str,
) -> Result<Option<&'a Admitted>, DeckError> {
    match doc
        .admitted
        .iter()
        .find(|a| a.client_id == client_id && a.request_id == request_id)
    {
        Some(a) if a.request_hash != hash => Err(DeckError::new(
            ErrorKind::RequestConflict,
            "request id was already used with different arguments",
        )),
        found => Ok(found),
    }
}

/// The answer to a request Deck accepted once and whose record it retired:
/// the operation it became, never a second run.
pub(super) fn retired_view(a: &Admitted) -> Value {
    json!({
        "ok": true,
        "operationId": a.operation_id,
        "kind": a.kind,
        "state": "retired",
        "code": null,
        "result": null,
        "acceptedAt": a.at,
        "updatedAt": a.at,
        "note": "Deck accepted this request once and no longer keeps its result; it will never run again. Read the card's current state with deck_worker_list."
    })
}

/// Whether the binding admits a request now (inside the acceptance write,
/// and before it as an early answer).
pub(super) fn check(client: &Client, lane: Option<&str>, binding: Binding) -> Result<(), Value> {
    match binding {
        Binding::Sequence(n) => {
            let expected = super::origin::send_sequence(client, lane);
            if n == expected {
                return Ok(());
            }
            let ahead = n > expected;
            Err(stale(
                client,
                lane,
                binding,
                if ahead { "ahead" } else { "behind" },
                if ahead {
                    "the request names a send sequence Deck has not reached: an earlier request of this lane was not accepted (refused for its own reason) or has not arrived yet; this one was not applied"
                } else {
                    "the request names a send sequence this lane already used; this attempt was not applied (Deck keeps no record of this request id, so if it was accepted once its record is retired, and it cannot run again)"
                },
            ))
        }
        Binding::Epoch(e) => {
            let current = epoch(client, lane);
            if e == current || e.saturating_add(1) == current {
                return Ok(());
            }
            Err(if e > current {
                stale(
                    client,
                    lane,
                    binding,
                    "ahead",
                    "the request names a lane epoch Deck never issued; it was not applied",
                )
            } else {
                stale(
                    client,
                    lane,
                    binding,
                    "retired",
                    "the request names a lane epoch Deck retired: it was not applied now, and Deck no longer knows whether this request id was accepted in that epoch",
                )
            })
        }
    }
}

fn stale(
    client: &Client,
    lane: Option<&str>,
    binding: Binding,
    relation: &str,
    message: &str,
) -> Value {
    let (received, expected) = match binding {
        Binding::Sequence(n) => (
            json!({"sendSequence": n}),
            json!({"sendSequence": super::origin::send_sequence(client, lane)}),
        ),
        Binding::Epoch(e) => (
            json!({"laneEpoch": e}),
            json!({"laneEpoch": epoch(client, lane), "alsoAccepted": epoch(client, lane).checked_sub(1)}),
        ),
    };
    json!({"ok": false, "error": {
        "code": "STALE_REQUEST",
        "message": message,
        "scope": "worker-lane",
        "relation": relation,
        "received": received,
        "expected": expected,
        "accepted": false,
        "lane": lane_view(client, lane, Some(binding)),
        "nextAction": "Read the lane from deck_worker_list or deck_worker_wait (nextSendSequence, laneEpoch), decide whether the effect is still wanted, and only then send it as a NEW request with a new request_id. Never resend an unchanged request. To send several requests of one lane at once, bind each with lane_epoch instead of send_sequence.",
    }})
}

/// Record the acceptance of a request in the same write as its operation:
/// the sequence moves on, or the tombstone is kept (and the lane's epoch
/// moves on when it is full, `tidy`).
pub(super) fn accept(
    doc: &mut DiskDoc,
    client_id: &str,
    lane: Option<&str>,
    binding: Binding,
    operation: &Operation,
) {
    let Some(client) = doc.config.clients.iter_mut().find(|c| c.id == client_id) else {
        return;
    };
    match binding {
        Binding::Sequence(_) => super::origin::bump_send(client, lane),
        Binding::Epoch(e) => {
            // the lane exists from its first acceptance on (`origin::lane`)
            super::origin::touch_lane(client, lane);
            doc.admitted.push(Admitted {
                client_id: client_id.into(),
                lane: lane.map(str::to_owned),
                epoch: e,
                request_id: operation.request_id.clone(),
                request_hash: operation.request_hash.clone(),
                operation_id: operation.operation_id.clone(),
                kind: operation.kind.clone(),
                at: operation.accepted_at,
            });
            let current = epoch(client, lane);
            let in_current = doc
                .admitted
                .iter()
                .filter(|a| {
                    a.client_id == client_id && a.lane.as_deref() == lane && a.epoch == current
                })
                .count();
            if in_current >= EPOCH_SPAN {
                advance(doc, client_id, lane);
            }
            tidy(doc);
        }
    }
}

/// Move a lane to its next epoch: the epoch before the old one retires
/// whole (the old one stays accepted as `E - 1`).
fn advance(doc: &mut DiskDoc, client_id: &str, lane: Option<&str>) {
    let Some(client) = doc.config.clients.iter_mut().find(|c| c.id == client_id) else {
        return;
    };
    let next = epoch(client, lane).saturating_add(1);
    match lane {
        Some(card) => {
            if let Some(l) = client.lanes.get_mut(card) {
                l.epoch = next;
            }
        }
        None => client.send_epoch = next,
    }
    doc.admitted.retain(|a| {
        !(a.client_id == client_id && a.lane.as_deref() == lane && a.epoch.saturating_add(1) < next)
    });
}

/// Keep the bounds: drop the tombstones of lanes that are gone, then, while
/// a client or the ledger holds too many, move on the lane holding the
/// oldest one.
pub(super) fn tidy(doc: &mut DiskDoc) {
    let lanes: HashSet<(String, Option<String>)> = doc
        .config
        .clients
        .iter()
        .flat_map(|c| {
            std::iter::once((c.id.clone(), None))
                .chain(c.lanes.keys().map(|k| (c.id.clone(), Some(k.clone()))))
        })
        .collect();
    doc.admitted
        .retain(|a| lanes.contains(&(a.client_id.clone(), a.lane.clone())));
    // every pass retires at least one epoch of one lane: bounded
    for _ in 0..(MAX_ADMITTED * 2) {
        let over_client = doc.config.clients.iter().map(|c| c.id.clone()).find(|id| {
            doc.admitted.iter().filter(|a| &a.client_id == id).count() > MAX_ADMITTED_PER_CLIENT
        });
        let oldest = match over_client {
            Some(id) => doc
                .admitted
                .iter()
                .filter(|a| a.client_id == id)
                .min_by_key(|a| a.at)
                .cloned(),
            None if doc.admitted.len() > MAX_ADMITTED => {
                doc.admitted.iter().min_by_key(|a| a.at).cloned()
            }
            None => return,
        };
        let Some(oldest) = oldest else { return };
        advance(doc, &oldest.client_id, oldest.lane.as_deref());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(id: &str) -> Client {
        let mut doc = DiskDoc::default();
        doc.config.clients.push(
            serde_json::from_value(json!({
                "id": id, "name": id, "credentialHash": "0".repeat(64), "credentialVersion": 1,
                "revokedAt": null, "allowCreate": false, "projects": []
            }))
            .unwrap(),
        );
        doc.config.clients.remove(0)
    }

    fn doc_with(ids: &[&str]) -> DiskDoc {
        let mut doc = DiskDoc::default();
        for id in ids {
            doc.config.clients.push(client(id));
        }
        doc
    }

    fn op(n: usize) -> Operation {
        Operation::new(
            format!("op_{n}"),
            "c1",
            format!("req_{n}"),
            sha(n.to_string().as_bytes()),
            "worker-close",
            "accepted",
        )
    }

    fn accept_n(doc: &mut DiskDoc, from: usize, to: usize, lane: Option<&str>) {
        for n in from..to {
            let e = epoch(&doc.config.clients[0], lane);
            accept(doc, "c1", lane, Binding::Epoch(e), &op(n));
        }
    }

    #[test]
    fn an_epoch_is_accepted_with_the_one_before_it_and_its_tombstones_are_complete() {
        let mut doc = doc_with(&["c1"]);
        let c = |doc: &DiskDoc| doc.config.clients[0].clone();
        assert!(check(&c(&doc), None, Binding::Epoch(0)).is_ok());
        assert_eq!(
            check(&c(&doc), None, Binding::Epoch(1)).unwrap_err()["error"]["relation"],
            "ahead"
        );
        accept_n(&mut doc, 0, EPOCH_SPAN, None);
        assert_eq!(epoch(&c(&doc), None), 1, "a full epoch moves the lane on");
        // a request prepared for epoch 0 and still in flight is accepted
        assert!(check(&c(&doc), None, Binding::Epoch(0)).is_ok());
        assert!(tombstone(&doc, "c1", "req_0", &sha(b"0"))
            .unwrap()
            .is_some());
        accept_n(&mut doc, EPOCH_SPAN, 2 * EPOCH_SPAN, None);
        assert_eq!(epoch(&c(&doc), None), 2);
        // epoch 0 retired whole: its ids are gone and the epoch is refused
        assert!(tombstone(&doc, "c1", "req_0", &sha(b"0"))
            .unwrap()
            .is_none());
        let refused = check(&c(&doc), None, Binding::Epoch(0)).unwrap_err();
        assert_eq!(refused["error"]["relation"], "retired");
        assert_eq!(refused["error"]["accepted"], false);
        assert!(refused["error"]["message"]
            .as_str()
            .unwrap()
            .contains("no longer knows"));
        // epoch 1's are all still there: none of them can run twice
        for n in EPOCH_SPAN..2 * EPOCH_SPAN {
            let id = format!("req_{n}");
            assert!(
                tombstone(&doc, "c1", &id, &sha(n.to_string().as_bytes()))
                    .unwrap()
                    .is_some(),
                "{id}"
            );
        }
        assert!(
            tombstone(&doc, "c1", "req_70", "f".repeat(64).as_str()).is_err(),
            "a different hash conflicts"
        );
    }

    #[test]
    fn lanes_are_independent_and_the_client_bound_retires_the_oldest_lane_first() {
        let mut doc = doc_with(&["c1"]);
        doc.config.clients[0]
            .lanes
            .insert("A".into(), Default::default());
        doc.config.clients[0]
            .lanes
            .insert("B".into(), Default::default());
        accept_n(&mut doc, 0, 10, Some("A"));
        accept_n(&mut doc, 10, 20, Some("B"));
        assert_eq!(epoch(&doc.config.clients[0], Some("A")), 0);
        assert_eq!(epoch(&doc.config.clients[0], Some("B")), 0);
        // fill the client's bound from lane B: lane A (the oldest) moves on
        accept_n(&mut doc, 20, 20 + MAX_ADMITTED_PER_CLIENT, Some("B"));
        let count = doc.admitted.iter().filter(|a| a.client_id == "c1").count();
        assert!(count <= MAX_ADMITTED_PER_CLIENT, "{count}");
        // whatever was retired, every request still at an accepted epoch
        // of its lane has its tombstone
        for a in &doc.admitted {
            let e = epoch(&doc.config.clients[0], a.lane.as_deref());
            assert!(a.epoch == e || a.epoch + 1 == e);
        }
        // a lane that left drops its tombstones
        doc.config.clients[0].lanes.remove("A");
        tidy(&mut doc);
        assert!(doc.admitted.iter().all(|a| a.lane.as_deref() != Some("A")));
        assert!(valid_admitted(&doc));
    }

    #[test]
    fn the_sequence_binding_says_ahead_or_behind() {
        let mut c = client("c1");
        c.send_sequence = 3;
        assert!(check(&c, None, Binding::Sequence(3)).is_ok());
        let ahead = check(&c, None, Binding::Sequence(4)).unwrap_err();
        assert_eq!(ahead["error"]["relation"], "ahead");
        assert_eq!(ahead["error"]["expected"]["sendSequence"], 3);
        let behind = check(&c, None, Binding::Sequence(2)).unwrap_err();
        assert_eq!(behind["error"]["relation"], "behind");
        assert!(behind["error"]["message"]
            .as_str()
            .unwrap()
            .contains("cannot run again"));
        assert!(Binding::of(Some(1), Some(1)).is_err());
        assert!(Binding::of(None, None).is_err());
    }
}
