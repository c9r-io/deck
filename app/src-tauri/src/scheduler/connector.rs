//! Closed Connector projection and revision-checked queue mutations. Snapshot
//! callers supply the eligible card predicate; items for every other card are
//! omitted so the phone cannot discover queues for unsupported targets.

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{remove_item, save_queue, with_queue, Queues};
use crate::error::{DeckError, ErrorKind};
use crate::sync::LockRecover;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QueueDto {
    pub(crate) id: String,
    pub(crate) card_id: String,
    pub(crate) mode: String,
    pub(crate) state: String,
    pub(crate) paused: bool,
    pub(crate) revision: String,
}

#[derive(Clone)]
pub(crate) struct OperationDto {
    pub(crate) id: String,
    pub(crate) state: String,
}

pub(crate) fn snapshot(
    state: &Queues,
    include_card: impl Fn(&str) -> bool,
) -> (String, Vec<QueueDto>, Vec<OperationDto>) {
    let q = state.q.lock_or_recover();
    let bytes = serde_json::to_vec(&*q).unwrap_or_default();
    let revision = format!("{:x}", Sha256::digest(&bytes));
    let items = q
        .items
        .iter()
        .filter(|item| include_card(&item.card_id))
        .map(|i| QueueDto {
            id: i.id.clone(),
            card_id: i.card_id.clone(),
            mode: i.mode.clone(),
            state: i.state.as_str().into(),
            paused: i.paused,
            revision: i.revision.to_string(),
        })
        .collect();
    let operations = q
        .operations
        .iter()
        .map(|o| OperationDto {
            id: o.id.clone(),
            state: o.state.as_str().into(),
        })
        .collect();
    (revision, items, operations)
}

pub(crate) fn mutate(
    state: &Queues,
    card_id: &str,
    card_session: &str,
    item_id: &str,
    expected_revision: u64,
    pause: Option<bool>,
    validate_target: impl FnOnce() -> Result<(), DeckError>,
) -> Result<(), DeckError> {
    mutate_with_persist(
        state,
        &save_queue,
        card_id,
        card_session,
        item_id,
        expected_revision,
        pause,
        validate_target,
    )
}

// The closed mutation proof fields and injected persistence seam are kept
// explicit so production and failure-path tests exercise the same operation.
#[allow(clippy::too_many_arguments)]
fn mutate_with_persist(
    state: &Queues,
    persist: &dyn Fn(&super::QueueState) -> Result<(), DeckError>,
    card_id: &str,
    card_session: &str,
    item_id: &str,
    expected_revision: u64,
    pause: Option<bool>,
    validate_target: impl FnOnce() -> Result<(), DeckError>,
) -> Result<(), DeckError> {
    with_queue(&state.q, persist, |q| {
        // Authorization and session generation are checked while the queue
        // transaction is locked, before revision validation and persistence.
        validate_target()?;
        let item = q
            .items
            .iter()
            .find(|i| i.id == item_id)
            .ok_or_else(|| DeckError::new(ErrorKind::Missing, "scheduled prompt not found"))?;
        if item.card_id != card_id
            || item.session != card_session
            || item.revision != expected_revision
        {
            return Err(DeckError::new(
                ErrorKind::ContextChanged,
                "revision-changed",
            ));
        }
        match pause {
            Some(paused) => super::pause_item(q, item_id, paused),
            None => {
                if remove_item(q, item_id)? {
                    Ok(())
                } else {
                    Err(DeckError::new(
                        ErrorKind::Missing,
                        "scheduled prompt not found",
                    ))
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn queues() -> Queues {
        let queue = serde_json::from_value(json!({
            "items":[{
                "id":"Q1","session":"S1","card_id":"C1","dir":"","cmd":"",
                "text":"note","mode":"at","at":1,"added":1,"revision":7
            }],
            "last_fired":{}
        }))
        .unwrap();
        Queues::new(queue)
    }

    #[test]
    fn queue_mutation_checks_card_session_revision_inside_transaction() {
        let queues = queues();
        let saves = AtomicUsize::new(0);
        let persist = |_: &super::super::QueueState| {
            saves.fetch_add(1, Ordering::SeqCst);
            Ok(())
        };
        assert_eq!(
            mutate_with_persist(
                &queues,
                &persist,
                "C1",
                "wrong-session",
                "Q1",
                7,
                Some(true),
                || Ok(())
            )
            .unwrap_err()
            .kind(),
            ErrorKind::ContextChanged
        );
        assert_eq!(saves.load(Ordering::SeqCst), 0);
        mutate_with_persist(
            &queues,
            &persist,
            "C1",
            "S1",
            "Q1",
            7,
            Some(true),
            || Ok(()),
        )
        .unwrap();
        assert!(snapshot(&queues, |_| true).1[0].paused);
        assert_eq!(saves.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn snapshot_omits_queue_items_for_ineligible_cards() {
        let queues = queues();
        assert_eq!(snapshot(&queues, |card_id| card_id == "C1").1.len(), 1);
        assert!(snapshot(&queues, |card_id| card_id == "other").1.is_empty());
    }
}
