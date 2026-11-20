//! handover.rs — a worker card's line given to the Pi in its pane by Deck's
//! own observer (approved 2026-11-20; design
//! `docs/design/pi-worker-delivery-20261120.md`).
//!
//! Deck never types into a Pi: Pi gives no reliable sign that it waits at
//! its own editor with nothing over it (`agent_pi.rs`), and a keystroke
//! into a dialog answers it. A worker card whose type starts `pi` gets its
//! lines another way. The scheduler's delivery (`scheduler/delivery.rs`
//! `fire_item`) puts ONE line in this mailbox for the card's pane and waits;
//! the observer of that pane asks Deck every few seconds (a `ping`), and the
//! answer to a frame of ITS lease — the lease the kernel proved to be that
//! pane's foreground Pi, between two runs — carries the line (`take_for`,
//! `agent_pi::ack_line_with`). The observer gives it to Pi with
//! `pi.sendUserMessage` only while Pi is idle, never as a command or a
//! template, and says so in its next frame (`receipt`). The line goes
//! through Pi's own message path, not the keyboard: it can land in no
//! dialog and leaves the user's half-typed draft alone.
//!
//! # Contract
//! - One line per pane at a time; a second one is refused (`NotAsked`).
//! - Each line has a fresh id (`h_` + 16 random bytes in hex). A receipt
//!   counts only from the lease the line was offered to and for that id.
//! - Outcomes, which the scheduler maps onto its own states: `Delivered`
//!   (sent); `Declined` (not sent: Pi was busy or refused the call — tried
//!   again later); `NotAsked` (no observer of that pane asked within the
//!   budget: not sent); `Unconfirmed` (the line went out in an answer and no
//!   receipt came: it may have reached Pi, so it is NEVER offered again —
//!   the row is left for the user, as a paste whose Enter was refused is).
//! - The text lives here only while its line waits, in memory, and never
//!   in a log line or a `Debug`.

use crate::agent_pi::Decline;
use crate::sync::LockRecover;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

/// How long a line waits for the pane's observer to ask: two pings and the
/// answer of a third.
pub(crate) const ASK_BUDGET: Duration = Duration::from_secs(12);
/// How long a line that went out waits for its receipt: the observer sends
/// it as its next frame, and an `again` may delay it a few times.
pub(crate) const RECEIPT_BUDGET: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Waiting,
    Offered { lease: u64 },
    Delivered,
    Declined(Decline),
}

struct Offer {
    id: String,
    server_pid: u32,
    pane_id: String,
    pane_pid: u32,
    text: String,
    stage: Stage,
}

/// What became of one line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Delivered,
    Declined(Decline),
    NotAsked,
    Unconfirmed,
}

static MAILBOX: Mutex<Vec<Offer>> = Mutex::new(Vec::new());
static CHANGED: Condvar = Condvar::new();

/// The pane a line is for: tmux server, pane id and the pane's own process.
pub(crate) struct Pane<'a> {
    pub(crate) server_pid: u32,
    pub(crate) pane_id: &'a str,
    pub(crate) pane_pid: u32,
}

/// Put `text` in the mailbox for `pane` and wait for what became of it.
pub(crate) fn hand_over(pane: Pane<'_>, text: &str) -> Outcome {
    hand_over_within(pane, text, ASK_BUDGET, RECEIPT_BUDGET)
}

pub(crate) fn hand_over_within(
    pane: Pane<'_>,
    text: &str,
    ask: Duration,
    receipt: Duration,
) -> Outcome {
    let Ok(id) = crate::ledger::random_id("h_", 16) else {
        return Outcome::NotAsked;
    };
    let mut mailbox = MAILBOX.lock_or_recover();
    if mailbox
        .iter()
        .any(|o| o.server_pid == pane.server_pid && o.pane_id == pane.pane_id)
    {
        return Outcome::NotAsked;
    }
    mailbox.push(Offer {
        id: id.clone(),
        server_pid: pane.server_pid,
        pane_id: pane.pane_id.to_string(),
        pane_pid: pane.pane_pid,
        text: text.to_string(),
        stage: Stage::Waiting,
    });
    let stage = |mailbox: &Vec<Offer>| mailbox.iter().find(|o| o.id == id).map(|o| o.stage);
    let asked_by = Instant::now() + ask;
    let mut receipt_by = None;
    let outcome = loop {
        let now = Instant::now();
        match stage(&mailbox) {
            Some(Stage::Delivered) => break Outcome::Delivered,
            Some(Stage::Declined(why)) => break Outcome::Declined(why),
            Some(Stage::Offered { .. }) => {
                let by = *receipt_by.get_or_insert(now + receipt);
                if now >= by {
                    break Outcome::Unconfirmed;
                }
                mailbox = crate::sync::wait_timeout_or_recover(&CHANGED, mailbox, by - now);
            }
            Some(Stage::Waiting) => {
                if now >= asked_by {
                    break Outcome::NotAsked;
                }
                mailbox = crate::sync::wait_timeout_or_recover(&CHANGED, mailbox, asked_by - now);
            }
            None => break Outcome::NotAsked,
        }
    };
    mailbox.retain(|o| o.id != id);
    outcome
}

/// The waiting line of the pane `lease` is bound to, now offered to that
/// lease: its id and text. The transport asks this while it answers a frame
/// of a lease the kernel proved to be the pane's foreground Pi, outside a
/// run (`transport::committed`).
pub(crate) fn take_for(
    lease: u64,
    server_pid: u32,
    pane_id: &str,
    pane_pid: u32,
) -> Option<(String, String)> {
    let mut mailbox = MAILBOX.lock_or_recover();
    let offer = mailbox.iter_mut().find(|o| {
        o.stage == Stage::Waiting
            && o.server_pid == server_pid
            && o.pane_id == pane_id
            && o.pane_pid == pane_pid
    })?;
    offer.stage = Stage::Offered { lease };
    let taken = (offer.id.clone(), offer.text.clone());
    drop(mailbox);
    CHANGED.notify_all();
    Some(taken)
}

/// The observer's receipt for line `id`. Counted only from the lease it was
/// offered to; anything else (a late receipt of a line the scheduler gave
/// up on, another lease) changes nothing. Whether it was counted.
pub(crate) fn receipt(lease: u64, id: &str, outcome: Result<(), Decline>) -> bool {
    let mut mailbox = MAILBOX.lock_or_recover();
    let Some(offer) = mailbox
        .iter_mut()
        .find(|o| o.id == id && o.stage == Stage::Offered { lease })
    else {
        return false;
    };
    offer.stage = match outcome {
        Ok(()) => Stage::Delivered,
        Err(why) => Stage::Declined(why),
    };
    drop(mailbox);
    CHANGED.notify_all();
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    fn pane(pane_id: &str) -> Pane<'_> {
        Pane {
            server_pid: 7,
            pane_id,
            pane_pid: 70,
        }
    }

    #[test]
    fn a_line_goes_to_the_lease_that_asked_and_counts_only_its_receipt() {
        let worker = thread::spawn(|| {
            hand_over_within(
                pane("%41"),
                "fix the test",
                Duration::from_secs(5),
                Duration::from_secs(5),
            )
        });
        let taken = loop {
            if let Some(taken) = take_for(3, 7, "%41", 70) {
                break taken;
            }
            assert!(
                take_for(3, 7, "%41", 71).is_none(),
                "another pane process gets nothing"
            );
            thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(taken.1, "fix the test");
        assert!(taken.0.starts_with("h_") && crate::agent_pi::handover_id(&taken.0));
        assert!(take_for(3, 7, "%41", 70).is_none(), "offered once");
        assert!(
            !receipt(4, &taken.0, Ok(())),
            "another lease's receipt does not count"
        );
        assert!(!receipt(3, "h_other", Ok(())));
        assert!(receipt(3, &taken.0, Ok(())));
        assert_eq!(worker.join().unwrap(), Outcome::Delivered);
        assert!(!receipt(3, &taken.0, Ok(())), "late: the line is gone");
    }

    #[test]
    fn a_line_nobody_asked_for_was_not_sent_and_one_without_a_receipt_is_unconfirmed() {
        let short = Duration::from_millis(60);
        assert_eq!(
            hand_over_within(pane("%42"), "a", short, short),
            Outcome::NotAsked
        );
        let worker = thread::spawn(move || {
            hand_over_within(pane("%43"), "b", Duration::from_secs(5), short)
        });
        while take_for(5, 7, "%43", 70).is_none() {
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(worker.join().unwrap(), Outcome::Unconfirmed);
        let worker = thread::spawn(|| {
            hand_over_within(
                pane("%44"),
                "c",
                Duration::from_secs(5),
                Duration::from_secs(5),
            )
        });
        let id = loop {
            if let Some((id, _)) = take_for(6, 7, "%44", 70) {
                break id;
            }
            thread::sleep(Duration::from_millis(5));
        };
        assert!(receipt(6, &id, Err(Decline::Busy)));
        assert_eq!(worker.join().unwrap(), Outcome::Declined(Decline::Busy));
    }

    #[test]
    fn one_line_per_pane_at_a_time() {
        let worker = thread::spawn(|| {
            hand_over_within(
                pane("%45"),
                "first",
                Duration::from_millis(300),
                Duration::from_millis(50),
            )
        });
        while waiting_for("%45") == 0 {
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            hand_over_within(
                pane("%45"),
                "second",
                Duration::from_secs(1),
                Duration::from_secs(1)
            ),
            Outcome::NotAsked
        );
        assert_eq!(worker.join().unwrap(), Outcome::NotAsked);
    }

    fn waiting_for(pane_id: &str) -> usize {
        MAILBOX
            .lock_or_recover()
            .iter()
            .filter(|o| o.pane_id == pane_id)
            .count()
    }
}
