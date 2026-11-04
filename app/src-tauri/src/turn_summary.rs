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
    if accepted.state != crate::agent_status::TURN_DONE || accepted.class != SignalClass::Claude {
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

/// The final message of a turn `agent_status` accepted (`dispatch`): kept
/// as it is when short, else summarized off this thread. When the card has
/// follow-ups (`documents::card_follow_ups`) the same one call also asks
/// which of them fits, so a short message is sent too, for that alone.
pub(crate) fn offer(accepted: &Accepted, reply: String) {
    let has_key = crate::model_api::has_key();
    let decision = decide(accepted, &reply, has_key);
    let follow_ups = if has_key && decision != Decision::Skip {
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
}
