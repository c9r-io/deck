//! turn_summary.rs — one line about what Claude Code said at the end of a
//! turn, shown on its card: the opt-in "summarize turn endings" switch
//! (Settings › Integrations › Model APIs).
//!
//! # Contract
//! - The switch IS the Stop hook: while it is on, Claude Code's Stop hook
//!   runs the status helper with `--reply` (`agent_status/hooks.rs` writes
//!   that only on the user's toggle, and reads the state back from the
//!   hooks file), and the helper forwards Claude's final message of the
//!   turn (`last_assistant_message`, its head and tail) on a v3 line.
//!   Without the flag nothing of the message leaves Claude Code. An older
//!   deck, which knows no v3, refuses such a line (`bad-version`: no Signal
//!   for that turn ending) until its own boot migration rewrites the hooks
//!   to its spec, without `--reply`.
//! - `split` takes `reply` off a v3 line BEFORE the status parser sees it:
//!   what remains is the ordinary v1/v2 line, so `agent_status` stays
//!   content-free. `offer` receives the text only for a `turn-done` of a
//!   Claude Code pane that `agent_status` ACCEPTED for its current
//!   generation, with that observation's episode; a refused event's text
//!   is dropped unread.
//! - A message that is one short line is its own summary: nothing is sent.
//!   Otherwise, with a model key stored, `model_api::summarize_turn` sends
//!   the message once, off the status dispatcher (at most `WORKERS` calls
//!   at a time and `QUEUE` waiting; more are dropped); without a key
//!   nothing is sent and no summary appears.
//! - A card with follow-ups (the fixed replies its run froze,
//!   `documents::card_follow_ups`): with a key stored, that same one call
//!   carries them and the answer also names the one that fits
//!   (`parse_pick`), so a short message is sent too, for the pick alone.
//!   The pick (a number in range and a one-line reason) only marks a
//!   button: nothing queues, sends, holds or notifies on it, and text from
//!   outside can sway which button is marked, never what a click queues.
//! - Worker replies (`mcp/workers.rs`): the final message of a worker
//!   card's Claude Code turn ending is also kept as it came (head and tail),
//!   in memory only, at most `KEEP_REPLIES`, while a client that may read
//!   that card's replies exists (`mcp::replies_wanted_for`), for that client
//!   alone (`reply_for`). While the Stop hook forwards the message for
//!   worker replies only (`--reply workers`, `agent_status::summaries_wanted`
//!   false) nothing else happens to it: no summary, no model call.
//! - Codex: its Stop hook forwards the final message in one fixed form
//!   (`agent_status/hooks.rs` `CODEX_HOOKS_WITH_REPLY`). A Codex turn
//!   ending accepted for its pane (only `--no-daemon` is ever accepted)
//!   feeds the three readers below exactly as a Claude one does
//!   (`has_final_message`); it is never summarized, and a message no reader
//!   wants is dropped where it arrives.
//! - Pi: an observer loaded from the reply entry (`agent_pi`, the user's
//!   "forward Pi's final message" switch) sends the final message of a run
//!   before that run's ending; the transport hands it over with the
//!   ending's episode (`offer_pi`). It feeds the readers below as the other
//!   two do. A short one is the card's line as written, whatever the
//!   summary switch says; a long one is summarized only while that switch
//!   is on and a key is stored. No follow-up is picked for a Pi card:
//!   nothing is ever sent to a Pi automatically.
//! - Card snapshot replies (`mcp/cards.rs`): while some MCP client may read
//!   the card snapshot (`mcp::card_readers_any`), the newest final message
//!   of each Claude Code or Codex session is kept as it came, one per session, at
//!   most `KEEP_LAST`, memory only; once none may, it is forgotten
//!   (`forget_card_replies`). `card_reply` reads it, or a worker or phone
//!   reply of the same turn ending, for that client's snapshot only. Like
//!   worker replies, the forward-only Stop hook form makes no summary.
//! - Phone replies (`connector/projection.rs` `replies`): while the user's
//!   "Claude Code replies on the phone" switch is on (Settings › Phone
//!   Connector, `connector_set_phone_replies`), the final message of every
//!   Claude Code turn ending is also kept as it came, per card session, at
//!   most `PHONE_PER_SESSION` each and `PHONE_TOTAL` in all, in memory only;
//!   turning the switch off forgets them all, and the phone snapshot drops
//!   the sessions no longer on the Board. A paired phone reads them only
//!   through the same card checks as its output read. The switch alone (no
//!   summaries) forwards the message with `--reply workers`, so nothing is
//!   summarized or sent to a model for it.
//! - Notes (the summary and the pick) live in memory only, keyed by
//!   (session, episode), at most `KEEP`; a card shows one only while its
//!   observation is still that episode (`for_episode`, read by the poll),
//!   so a new turn hides the old one at once. Nothing is written, logged
//!   (app.log gets closed words), notified, or read by attention, a hold or
//!   any side effect: Presentation only (`tests/signal_census.rs`).

use crate::agent_status::{Accepted, EpisodeId, SignalClass};
use crate::applog::applog;
use crate::sync::LockRecover;
use serde_json::Value;
use std::collections::VecDeque;
use std::sync::{mpsc, Arc, Mutex, OnceLock};

/// The longest reply a v3 line may carry: the helper sends at most 2,000
/// bytes of head, 4,000 of tail and the cut mark between them.
const MAX_REPLY_BYTES: usize = 6_100;
/// A message of one line this short is shown as it is.
const SHORT_CHARS: usize = 80;
/// A summary is cut to this many characters.
const MAX_SUMMARY_CHARS: usize = 120;
/// The reason given for a picked follow-up is cut to this many.
const MAX_REASON_CHARS: usize = 80;
/// Summaries kept, oldest dropped first.
const KEEP: usize = 64;
/// Model calls at a time, and turn endings waiting for one.
const WORKERS: usize = 2;
const QUEUE: usize = 16;

/// A line of the status socket without its reply, and the reply. Only a
/// v3 line carries one: it comes back as the v1 or v2 line the status
/// parser knows (v2 when it names an interaction), with the reply when that
/// is a non-empty string of at most `MAX_REPLY_BYTES`. Any other line
/// passes through untouched.
pub(crate) fn split(line: &str) -> (String, Option<String>) {
    let Ok(Value::Object(mut fields)) = serde_json::from_str::<Value>(line) else {
        return (line.to_string(), None);
    };
    if fields.get("v").and_then(Value::as_u64) != Some(3) {
        return (line.to_string(), None);
    }
    let reply = match fields.remove("reply") {
        Some(Value::String(text)) if text.len() <= MAX_REPLY_BYTES && !text.trim().is_empty() => {
            Some(text)
        }
        _ => None,
    };
    let version = if fields.contains_key("interaction") {
        2
    } else {
        1
    };
    fields.insert("v".into(), version.into());
    (Value::Object(fields).to_string(), reply)
}

/// What becomes of a turn ending's final message.
#[derive(Debug, PartialEq)]
enum Decision {
    /// One short line: shown as it is, nothing sent.
    Keep(String),
    /// Sent to the model for one line.
    Ask,
    /// Not a Claude Code turn ending, or no key to ask with.
    Skip,
}

fn decide(accepted: &Accepted, reply: &str, has_key: bool) -> Decision {
    if accepted.state != crate::agent_status::TURN_DONE
        || !matches!(accepted.class, SignalClass::Claude | SignalClass::Pi)
    {
        return Decision::Skip;
    }
    let text = reply.trim();
    if !text.contains('\n') && text.chars().count() <= SHORT_CHARS {
        if let Some(line) = crate::model_api::one_line(text, MAX_SUMMARY_CHARS) {
            return Decision::Keep(line);
        }
    }
    if has_key {
        Decision::Ask
    } else {
        Decision::Skip
    }
}

/// The sources whose accepted turn ending carries a final message the
/// readers may keep: Claude Code, Codex (`agent_status/hooks.rs`
/// `CODEX_HOOKS_WITH_REPLY`) and Pi (`agent_pi`, the reply entry). A Codex
/// event is accepted only when it is bound to its pane (`--no-daemon`) and
/// a Pi frame only from the proven observer of its pane's foreground
/// program, so the message is that card's.
fn has_final_message(class: SignalClass) -> bool {
    matches!(
        class,
        SignalClass::Claude | SignalClass::Codex | SignalClass::Pi
    )
}

/// The final message of a Pi run whose ending the transport committed
/// (`agent_pi/transport.rs`, "The final message"), with the episode that
/// ending was given: the readers of every agent's final message, and the
/// card's one line.
pub(crate) fn offer_pi(session: &str, episode: EpisodeId, reply: String) {
    offer(
        &Accepted {
            session: session.to_string(),
            episode,
            state: crate::agent_status::TURN_DONE,
            class: SignalClass::Pi,
        },
        reply,
    );
}

/// The final message of a turn `agent_status` accepted (`dispatch`): kept
/// as it is when short, else summarized off this thread. When the card has
/// follow-ups (`documents::card_follow_ups`) the same one call also asks
/// which of them fits, so a short message is sent too, for that alone.
pub(crate) fn offer(accepted: &Accepted, reply: String) {
    // a worker card's final message, for the orchestrator the user let
    // read its replies (`mcp/workers.rs`); kept before any summary choice
    if accepted.state == crate::agent_status::TURN_DONE
        && has_final_message(accepted.class)
        && crate::mcp::replies_wanted_for(&accepted.session)
    {
        remember_reply(&accepted.session, accepted.episode, &reply);
    }
    // the newest final message of each Claude Code or Codex card, while some MCP
    // client may read the card snapshot (`mcp/cards.rs`)
    if accepted.state == crate::agent_status::TURN_DONE
        && has_final_message(accepted.class)
        && crate::mcp::card_readers_any()
    {
        remember_last(&accepted.session, accepted.episode, &reply);
    }
    // a paired phone's reply list (`connector`), when the user turned it on
    if accepted.state == crate::agent_status::TURN_DONE
        && has_final_message(accepted.class)
        && phone_replies_on()
    {
        remember_phone_reply(&accepted.session, accepted.episode, &reply, now_ms());
    }
    // a Codex message is never summarized (its Stop hook forwards for the
    // readers above only; nobody read it: it is dropped here)
    if accepted.class == SignalClass::Codex {
        return;
    }
    // the Stop hook forwards the message for worker replies alone: no
    // summary, no model call (`agent_status/hooks.rs` `ReplyHook::Workers`)
    let summaries = crate::agent_status::summaries_wanted();
    let pi = accepted.class == SignalClass::Pi;
    if !summaries && !pi {
        return;
    }
    // Pi's switch is the user's choice to see what Pi said: a short message
    // is shown as written whatever the summary switch says; a long one goes
    // to a model only while that switch is on, as for Claude Code
    let has_key = summaries && crate::model_api::has_key();
    let decision = decide(accepted, &reply, has_key);
    // no automatic prompt is ever sent to a Pi (`Hold::PiInputUnverified`):
    // nothing is picked for a card whose follow-up could not be delivered
    let follow_ups = if has_key && !pi && decision != Decision::Skip {
        crate::documents::card_follow_ups(&accepted.session)
    } else {
        Vec::new()
    };
    let summarize = match decision {
        Decision::Keep(line) => {
            keep(&accepted.session, accepted.episode, line);
            applog("[summary] kept as written");
            false
        }
        Decision::Ask => true,
        Decision::Skip => return,
    };
    if !summarize && follow_ups.is_empty() {
        return;
    }
    let job = Job {
        session: accepted.session.clone(),
        episode: accepted.episode,
        reply,
        follow_ups,
        summarize,
    };
    if sender().try_send(job).is_err() {
        applog("[summary] dropped (busy)");
    }
}

/// One turn ending waiting for the model.
struct Job {
    session: String,
    episode: EpisodeId,
    reply: String,
    /// the card's follow-ups, to pick from; empty asks for a summary alone
    follow_ups: Vec<String>,
    /// whether the summary comes from the model (a short message is its own)
    summarize: bool,
}

/// The queue to the `WORKERS` threads, started on first use.
fn sender() -> &'static mpsc::SyncSender<Job> {
    static SENDER: OnceLock<mpsc::SyncSender<Job>> = OnceLock::new();
    SENDER.get_or_init(|| {
        let (sender, jobs) = mpsc::sync_channel::<Job>(QUEUE);
        let jobs = Arc::new(Mutex::new(jobs));
        for worker in 0..WORKERS {
            let jobs = jobs.clone();
            let _ = std::thread::Builder::new()
                .name(format!("deck-turn-summary-{worker}"))
                .spawn(move || loop {
                    let Ok(job) = jobs.lock_or_recover().recv() else {
                        return;
                    };
                    summarize(job, crate::model_api::summarize_turn);
                });
        }
        sender
    })
}

/// The model call `summarize` makes: the message, the follow-ups to pick
/// from (none asks for a summary alone); the raw reply and its provider.
type Model = fn(&str, &[String]) -> Result<(String, &'static str), &'static str>;

/// One model call for one job; what it answered is kept, a failure says
/// its word.
fn summarize(job: Job, model: Model) {
    let (reply, provider) = match model(&job.reply, &job.follow_ups) {
        Ok(answer) => answer,
        Err(code) => return applog(&format!("[summary] failed ({code})")),
    };
    if job.follow_ups.is_empty() {
        return match crate::model_api::one_line(&reply, MAX_SUMMARY_CHARS) {
            Some(line) => {
                keep(&job.session, job.episode, line);
                applog(&format!("[summary] written ({provider})"));
            }
            None => applog("[summary] failed (response)"),
        };
    }
    let Some(answer) = parse_pick(&reply, job.follow_ups.len()) else {
        return applog("[summary] failed (response)");
    };
    if job.summarize {
        if let Some(line) = answer.summary {
            keep(&job.session, job.episode, line);
        }
    }
    match answer.pick {
        Some(pick) => {
            keep_pick(&job.session, job.episode, pick);
            applog(&format!("[summary] picked ({provider})"));
        }
        None => applog(&format!("[summary] no pick ({provider})")),
    }
}

/// The model's answer when the card has follow-ups: one JSON object with
/// `summary`, `choice` (a follow-up's number from 1, or null) and `reason`
/// (`model_api::FOLLOW_UP_INSTRUCTIONS`), read as a `Note`. `None` when it
/// is not that object; a choice outside `1..=count` is no pick.
fn parse_pick(reply: &str, count: usize) -> Option<Note> {
    let start = reply.find('{')?;
    let end = reply.rfind('}')?;
    let Ok(Value::Object(answer)) = serde_json::from_str::<Value>(reply.get(start..=end)?) else {
        return None;
    };
    let summary = answer
        .get("summary")
        .and_then(Value::as_str)
        .and_then(|text| crate::model_api::one_line(text, MAX_SUMMARY_CHARS));
    let pick = answer
        .get("choice")
        .and_then(Value::as_u64)
        .filter(|choice| (1..=count as u64).contains(choice))
        .map(|choice| {
            let reason = answer
                .get("reason")
                .and_then(Value::as_str)
                .and_then(|text| crate::model_api::one_line(text, MAX_REASON_CHARS))
                .unwrap_or_default();
            (choice as u8, reason)
        });
    Some(Note { summary, pick })
}

/// One turn ending's note: its summary and, for a card with follow-ups,
/// the model's pick (a follow-up's number from 1, and why).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Note {
    pub(crate) summary: Option<String>,
    pub(crate) pick: Option<(u8, String)>,
}

/// One kept note.
struct Kept {
    session: String,
    episode: EpisodeId,
    note: Note,
}

static KEPT: Mutex<VecDeque<Kept>> = Mutex::new(VecDeque::new());

/// Tests that read or clear `KEPT` hold this (here and in `agent_status`).
#[cfg(test)]
pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());

/// Change the note of `session`'s turn ending `episode`, made the newest.
fn update(session: &str, episode: EpisodeId, change: impl FnOnce(&mut Note)) {
    let mut kept = KEPT.lock_or_recover();
    let mut note = kept
        .iter()
        .position(|entry| entry.session == session && entry.episode == episode)
        .and_then(|at| kept.remove(at))
        .map(|entry| entry.note)
        .unwrap_or_default();
    change(&mut note);
    kept.push_back(Kept {
        session: session.to_string(),
        episode,
        note,
    });
    while kept.len() > KEEP {
        kept.pop_front();
    }
}

fn keep(session: &str, episode: EpisodeId, text: String) {
    update(session, episode, |note| note.summary = Some(text));
}

fn keep_pick(session: &str, episode: EpisodeId, pick: (u8, String)) {
    update(session, episode, |note| note.pick = Some(pick));
}

/// The note of `session`'s turn ending `episode`, if one was written.
pub(crate) fn for_episode(session: &str, episode: EpisodeId) -> Option<Note> {
    KEPT.lock_or_recover()
        .iter()
        .rev()
        .find(|entry| entry.session == session && entry.episode == episode)
        .map(|entry| entry.note.clone())
}

/// Worker replies kept, oldest dropped first.
const KEEP_REPLIES: usize = 32;

/// One worker card's final message of one turn ending, as the helper
/// forwarded it (head and tail, `REPLY_CUT` between them when cut).
struct Reply {
    session: String,
    episode: EpisodeId,
    text: String,
}

static REPLIES: Mutex<VecDeque<Reply>> = Mutex::new(VecDeque::new());

pub(crate) fn remember_reply(session: &str, episode: EpisodeId, text: &str) {
    let mut replies = REPLIES.lock_or_recover();
    replies.retain(|r| !(r.session == session && r.episode == episode));
    replies.push_back(Reply {
        session: session.to_string(),
        episode,
        text: text.to_string(),
    });
    while replies.len() > KEEP_REPLIES {
        replies.pop_front();
    }
}

/// The helper's mark between the head and the tail of a cut message
/// (`status-helper` `REPLY_CUT`).
pub(crate) const REPLY_CUT: &str = "\n…\n";

/// `session`'s final message of turn ending `episode`, while it is still
/// kept (memory only: a Deck restart or 32 newer ones drop it).
pub(crate) fn reply_for(session: &str, episode: EpisodeId) -> Option<String> {
    REPLIES
        .lock_or_recover()
        .iter()
        .rev()
        .find(|r| r.session == session && r.episode == episode)
        .map(|r| r.text.clone())
}

/// Card snapshot replies kept, one per session, oldest dropped first.
const KEEP_LAST: usize = 256;

/// The newest final message of each Claude Code session, kept while some
/// MCP client may read the card snapshot (`mcp/cards.rs`).
static LAST: Mutex<VecDeque<Reply>> = Mutex::new(VecDeque::new());

fn remember_last(session: &str, episode: EpisodeId, text: &str) {
    let mut kept = LAST.lock_or_recover();
    kept.retain(|r| r.session != session);
    kept.push_back(Reply {
        session: session.to_string(),
        episode,
        text: text.to_string(),
    });
    while kept.len() > KEEP_LAST {
        kept.pop_front();
    }
}

/// Once no client may read the card snapshot, forget what it kept.
pub(crate) fn forget_card_replies() {
    LAST.lock_or_recover().clear();
}

/// `session`'s final message of turn ending `episode`, from whatever Deck
/// still keeps of it: the card snapshot's own, a worker reply or a phone
/// reply (each kept for its own reader; this reads, never keeps).
pub(crate) fn card_reply(session: &str, episode: EpisodeId) -> Option<String> {
    let found = LAST
        .lock_or_recover()
        .iter()
        .find(|r| r.session == session && r.episode == episode)
        .map(|r| r.text.clone());
    found.or_else(|| reply_for(session, episode)).or_else(|| {
        PHONE
            .lock_or_recover()
            .iter()
            .rev()
            .find(|e| e.session == session && e.episode == episode)
            .map(|e| e.reply.text.clone())
    })
}

#[cfg(test)]
pub(crate) fn keep_card_reply_for_tests(session: &str, episode: EpisodeId, text: &str) {
    remember_last(session, episode, text);
}

/// Phone replies kept for one card session, and in all.
pub(crate) const PHONE_PER_SESSION: usize = 20;
const PHONE_TOTAL: usize = 400;

/// The user's "Claude Code replies on the phone" switch, as the Connector
/// document holds it (`connector::set_phone_replies_state`).
static PHONE_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// One Claude Code turn ending's final message for a paired phone.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PhoneReply {
    /// When Deck accepted the turn ending, in milliseconds.
    pub(crate) at: u64,
    pub(crate) text: String,
    /// The helper kept only its head and tail (`REPLY_CUT` between them).
    pub(crate) cut: bool,
}

struct PhoneEntry {
    session: String,
    episode: EpisodeId,
    reply: PhoneReply,
}

static PHONE: Mutex<VecDeque<PhoneEntry>> = Mutex::new(VecDeque::new());

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Follow the switch: off forgets every kept phone reply at once.
pub(crate) fn set_phone_replies(on: bool) {
    PHONE_ON.store(on, std::sync::atomic::Ordering::Relaxed);
    if !on {
        PHONE.lock_or_recover().clear();
    }
}

pub(crate) fn phone_replies_on() -> bool {
    PHONE_ON.load(std::sync::atomic::Ordering::Relaxed)
}

/// Whether `text` is a message the helper cut (`status-helper`
/// `bounded_reply`): 2,000 bytes of head (less up to 3 for a character
/// boundary), the mark, then the 4,000-byte tail. A message that was never
/// cut is at most 6,000 bytes, so it matches only if it holds the mark at
/// exactly that place and is that long.
fn helper_cut(text: &str) -> bool {
    text.len() >= 1_997 + REPLY_CUT.len() + 3_997
        && (1_997..=2_000).any(|at| text.get(at..at + REPLY_CUT.len()) == Some(REPLY_CUT))
}

fn remember_phone_reply(session: &str, episode: EpisodeId, text: &str, at: u64) {
    push_phone(&mut PHONE.lock_or_recover(), session, episode, text, at);
}

fn push_phone(
    kept: &mut VecDeque<PhoneEntry>,
    session: &str,
    episode: EpisodeId,
    text: &str,
    at: u64,
) {
    kept.retain(|e| !(e.session == session && e.episode == episode));
    kept.push_back(PhoneEntry {
        session: session.to_string(),
        episode,
        reply: PhoneReply {
            at,
            text: text.to_string(),
            cut: helper_cut(text),
        },
    });
    let mine = kept.iter().filter(|e| e.session == session).count();
    if mine > PHONE_PER_SESSION {
        if let Some(oldest) = kept.iter().position(|e| e.session == session) {
            kept.remove(oldest);
        }
    }
    while kept.len() > PHONE_TOTAL {
        kept.pop_front();
    }
}

/// Debug smoke only (`connector/smoke.rs`): keep `text` as a turn ending
/// of `session`, `ago_ms` before now, as if Claude Code had ended it.
pub(crate) fn smoke_phone_reply(session: &str, episode: EpisodeId, text: &str, ago_ms: u64) {
    if crate::smoke_faults::enabled() {
        remember_phone_reply(session, episode, text, now_ms().saturating_sub(ago_ms));
    }
}

/// `session`'s kept phone replies, oldest first.
pub(crate) fn phone_replies(session: &str) -> Vec<PhoneReply> {
    PHONE
        .lock_or_recover()
        .iter()
        .filter(|e| e.session == session)
        .map(|e| e.reply.clone())
        .collect()
}

/// How many phone replies `session` has and when the newest came.
pub(crate) fn phone_reply_count(session: &str) -> (usize, Option<u64>) {
    let kept = PHONE.lock_or_recover();
    let mut mine = kept.iter().filter(|e| e.session == session);
    let count = mine.clone().count();
    (count, mine.next_back().map(|e| e.reply.at))
}

/// Drop the phone replies of every session `keep` refuses (no longer a
/// card on the Board).
pub(crate) fn phone_replies_retain(keep: impl Fn(&str) -> bool) {
    PHONE.lock_or_recover().retain(|e| keep(&e.session));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accepted(state: &'static str, class: SignalClass, episode: EpisodeId) -> Accepted {
        Accepted {
            session: "deck-a".into(),
            episode,
            state,
            class,
        }
    }

    #[test]
    fn the_reply_comes_off_a_v3_line_and_the_rest_is_the_line_the_parser_knows() {
        let id = "0199aaaa-bbbb-7ccc-8ddd-eeeeffff0000";
        let v3 = format!(
            r#"{{"v":3,"source":"claude-code","state":"turn-done","socket":"deck","server_pid":9,"pane":"%1","interaction":"{id}","reply":"Fixed.\nCommit?"}}"#
        );
        let (line, reply) = split(&v3);
        assert_eq!(reply.as_deref(), Some("Fixed.\nCommit?"));
        let rest: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(rest["v"], 2);
        assert!(rest.get("reply").is_none());
        assert_eq!(rest["interaction"], id);
        // the remaining line is what the status parser accepts as v2
        assert!(!line.contains("Fixed"));
        // without an interaction: v1
        let (line, reply) = split(
            r#"{"v":3,"source":"claude-code","state":"turn-done","socket":"deck","server_pid":9,"pane":"%1","reply":"ok"}"#,
        );
        assert_eq!(serde_json::from_str::<Value>(&line).unwrap()["v"], 1);
        assert_eq!(reply.as_deref(), Some("ok"));
        // a reply that is not a string, empty or too long goes; the line stays
        for bad in [
            r#"42"#,
            r#""  ""#,
            &format!("{:?}", "x".repeat(MAX_REPLY_BYTES + 1)),
        ] {
            let (line, reply) = split(&format!(
                r#"{{"v":3,"source":"claude-code","state":"turn-done","socket":"deck","server_pid":9,"pane":"%1","reply":{bad}}}"#
            ));
            assert_eq!(reply, None, "{bad}");
            assert_eq!(serde_json::from_str::<Value>(&line).unwrap()["v"], 1);
        }
        // v1, v2 and anything else pass through untouched
        for other in [
            r#"{"v":1,"source":"claude-code","state":"working","socket":"deck","server_pid":9,"pane":"%1"}"#,
            r#"{"v":2,"reply":"not taken"}"#,
            "not json",
        ] {
            assert_eq!(split(other), (other.to_string(), None));
        }
    }

    #[test]
    fn only_a_claude_turn_ending_counts_and_a_short_one_is_its_own_summary() {
        let done = accepted(crate::agent_status::TURN_DONE, SignalClass::Claude, 1);
        assert_eq!(
            decide(&done, "  Fixed the login test. Commit?  ", false),
            Decision::Keep("Fixed the login test. Commit?".into())
        );
        assert_eq!(decide(&done, "Fixed.\nCommit?", true), Decision::Ask);
        assert_eq!(
            decide(&done, &"x".repeat(SHORT_CHARS + 1), true),
            Decision::Ask
        );
        assert_eq!(
            decide(&done, "Fixed.\nCommit?", false),
            Decision::Skip,
            "no key, nothing sent"
        );
        // another word or another source: nothing
        assert_eq!(
            decide(&accepted("working", SignalClass::Claude, 1), "ok", true),
            Decision::Skip
        );
        assert_eq!(
            decide(
                &accepted(crate::agent_status::TURN_DONE, SignalClass::Codex, 1),
                "ok",
                true
            ),
            Decision::Skip
        );
    }

    fn summary_of(session: &str, episode: EpisodeId) -> Option<String> {
        for_episode(session, episode).and_then(|note| note.summary)
    }

    #[test]
    fn a_summary_belongs_to_its_episode_and_the_table_is_bounded() {
        let _serial = TEST_LOCK.lock_or_recover();
        KEPT.lock_or_recover().clear();
        keep("deck-a", 7, "first".into());
        keep("deck-a", 7, "again".into());
        assert_eq!(
            summary_of("deck-a", 7).as_deref(),
            Some("again"),
            "one per episode"
        );
        assert_eq!(
            for_episode("deck-a", 8),
            None,
            "a new turn shows nothing old"
        );
        assert_eq!(for_episode("deck-b", 7), None);
        for episode in 100..100 + KEEP as u64 + 5 {
            keep("deck-c", episode, "x".into());
        }
        assert_eq!(KEPT.lock_or_recover().len(), KEEP);
        assert_eq!(for_episode("deck-a", 7), None, "the oldest went first");
        KEPT.lock_or_recover().clear();
    }

    #[test]
    fn a_model_reply_is_kept_as_one_line_and_a_failure_keeps_nothing() {
        let _serial = TEST_LOCK.lock_or_recover();
        KEPT.lock_or_recover().clear();
        let job = |episode| Job {
            session: "deck-a".into(),
            episode,
            reply: "long message".into(),
            follow_ups: Vec::new(),
            summarize: true,
        };
        summarize(job(1), |_, _| {
            Ok(("\"已修好登录测试，问是否提交\"\nextra".into(), "anthropic"))
        });
        assert_eq!(
            summary_of("deck-a", 1).as_deref(),
            Some("已修好登录测试，问是否提交")
        );
        summarize(job(2), |_, _| Err("busy"));
        summarize(job(3), |_, _| Ok(("  \n".into(), "openai")));
        assert_eq!(for_episode("deck-a", 2), None);
        assert_eq!(for_episode("deck-a", 3), None);
        KEPT.lock_or_recover().clear();
    }

    #[test]
    fn the_pick_is_one_of_the_follow_ups_and_anything_else_is_no_pick() {
        // a fenced answer whose reason holds an escaped line break (a
        // test-module line never starts at column 0: source_scan)
        let answer = |choice: &str| {
            [
                "```json".to_string(),
                format!(
                    r#"{{"summary":"Fixed the test; asks to commit","choice":{choice},"reason":"It asks\nwhether to commit"}}"#
                ),
                "```".to_string(),
            ]
            .join("\n")
        };
        // fenced or not; a reason is one line, as a summary is (its first)
        assert_eq!(
            parse_pick(&answer("2"), 3),
            Some(Note {
                summary: Some("Fixed the test; asks to commit".into()),
                pick: Some((2, "It asks".into()))
            })
        );
        for none in ["null", "0", "4", "-1", "1.5", "\"2\""] {
            assert_eq!(
                parse_pick(&answer(none), 3).map(|note| note.pick),
                Some(None),
                "{none}"
            );
        }
        // a reason that is missing or blank is an empty one, never a refusal
        assert_eq!(
            parse_pick(r#"{"summary":"s","choice":1}"#, 1),
            Some(Note {
                summary: Some("s".into()),
                pick: Some((1, String::new()))
            })
        );
        for bad in ["plain words", "{not json}", "[1, 2]", ""] {
            assert_eq!(parse_pick(bad, 3), None, "{bad}");
        }
    }

    #[test]
    fn with_follow_ups_the_one_answer_keeps_a_pick_beside_the_summary_it_was_asked_for() {
        let _serial = TEST_LOCK.lock_or_recover();
        KEPT.lock_or_recover().clear();
        let job = |episode, summarize| Job {
            session: "deck-f".into(),
            episode,
            reply: "Fixed it. Commit?".into(),
            follow_ups: vec!["Commit and push.".into(), "Stop here.".into()],
            summarize,
        };
        const ANSWER: &str = r#"{"summary":"Model line","choice":1,"reason":"asks to commit"}"#;
        // asked for both: both are kept
        summarize(job(1, true), |text, follow_ups| {
            assert_eq!(text, "Fixed it. Commit?");
            assert_eq!(follow_ups.len(), 2, "the follow-ups go with the message");
            Ok((ANSWER.into(), "anthropic"))
        });
        assert_eq!(
            for_episode("deck-f", 1),
            Some(Note {
                summary: Some("Model line".into()),
                pick: Some((1, "asks to commit".into()))
            })
        );
        // a short message was its own summary: the answer adds the pick only
        keep("deck-f", 2, "Fixed it. Commit?".into());
        summarize(job(2, false), |_, _| Ok((ANSWER.into(), "openai")));
        assert_eq!(
            for_episode("deck-f", 2),
            Some(Note {
                summary: Some("Fixed it. Commit?".into()),
                pick: Some((1, "asks to commit".into()))
            })
        );
        // no fitting follow-up, an answer that is not the object, a failure
        summarize(job(3, true), |_, _| {
            Ok((
                r#"{"summary":"Only a line","choice":null,"reason":""}"#.into(),
                "openai",
            ))
        });
        assert_eq!(
            for_episode("deck-f", 3),
            Some(Note {
                summary: Some("Only a line".into()),
                pick: None
            })
        );
        summarize(job(4, true), |_, _| Ok(("Just a line".into(), "openai")));
        summarize(job(5, true), |_, _| Err("busy"));
        assert_eq!(for_episode("deck-f", 4), None);
        assert_eq!(for_episode("deck-f", 5), None);
        KEPT.lock_or_recover().clear();
    }

    #[test]
    fn phone_replies_keep_twenty_per_session_and_the_newest() {
        let mut kept = VecDeque::new();
        for n in 0..25u64 {
            push_phone(&mut kept, "deck-a", n, &format!("a{n}"), n);
        }
        push_phone(&mut kept, "deck-b", 1, "b1", 100);
        let a: Vec<_> = kept.iter().filter(|e| e.session == "deck-a").collect();
        assert_eq!(a.len(), PHONE_PER_SESSION);
        assert_eq!(a.first().unwrap().reply.text, "a5");
        assert_eq!(a.last().unwrap().reply.text, "a24");
        // the same turn ending again replaces its entry, it does not add one
        push_phone(&mut kept, "deck-b", 1, "b1 again", 101);
        let b: Vec<_> = kept.iter().filter(|e| e.session == "deck-b").collect();
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].reply.text, "b1 again");
    }

    #[test]
    fn phone_replies_stop_at_the_total() {
        let mut kept = VecDeque::new();
        for n in 0..(PHONE_TOTAL as u64 + 30) {
            push_phone(&mut kept, &format!("deck-{}", n % 40), n, "x", n);
        }
        assert_eq!(kept.len(), PHONE_TOTAL);
        assert!(kept.iter().all(|e| e.episode >= 30));
    }

    #[test]
    fn a_cut_reply_is_told_from_a_whole_one() {
        let long = format!("{}{}", "h".repeat(9_000), "t".repeat(9_000));
        let cut = format!(
            "{}{REPLY_CUT}{}",
            &long[..2_000],
            &long[long.len() - 4_000..]
        );
        assert!(helper_cut(&cut));
        // a multibyte head cut back to a character boundary
        let wide = format!("{}{REPLY_CUT}{}", "字".repeat(666), "t".repeat(4_000));
        assert!(helper_cut(&wide));
        assert!(!helper_cut("short\n…\nreply"));
        let whole = format!("{}{REPLY_CUT}{}", "h".repeat(2_000), "t".repeat(100));
        assert!(!helper_cut(&whole));
    }
}
