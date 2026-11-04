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
//! - Summaries live in memory only, keyed by (session, episode), at most
//!   `KEEP`; a card shows one only while its observation is still that
//!   episode (`for_episode`, read by the poll), so a new turn hides the old
//!   one at once. Nothing is written, logged (app.log gets closed words),
//!   notified, or read by attention, a hold or any side effect:
//!   Presentation only (`tests/signal_census.rs`).

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
/// as it is when short, else summarized off this thread.
pub(crate) fn offer(accepted: &Accepted, reply: String) {
    match decide(accepted, &reply, crate::model_api::has_key()) {
        Decision::Keep(line) => {
            keep(&accepted.session, accepted.episode, line);
            applog("[summary] kept as written");
        }
        Decision::Ask => {
            let job = Job {
                session: accepted.session.clone(),
                episode: accepted.episode,
                reply,
            };
            if sender().try_send(job).is_err() {
                applog("[summary] dropped (busy)");
            }
        }
        Decision::Skip => {}
    }
}

/// One turn ending waiting for the model.
struct Job {
    session: String,
    episode: EpisodeId,
    reply: String,
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

/// One model call for one job; its line is kept, a failure says its word.
fn summarize(job: Job, model: fn(&str) -> Result<(String, &'static str), &'static str>) {
    match model(&job.reply) {
        Ok((reply, provider)) => match crate::model_api::one_line(&reply, MAX_SUMMARY_CHARS) {
            Some(line) => {
                keep(&job.session, job.episode, line);
                applog(&format!("[summary] written ({provider})"));
            }
            None => applog("[summary] failed (response)"),
        },
        Err(code) => applog(&format!("[summary] failed ({code})")),
    }
}

/// One kept summary.
struct Kept {
    session: String,
    episode: EpisodeId,
    text: String,
}

static KEPT: Mutex<VecDeque<Kept>> = Mutex::new(VecDeque::new());

/// Tests that read or clear `KEPT` hold this (here and in `agent_status`).
#[cfg(test)]
pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());

fn keep(session: &str, episode: EpisodeId, text: String) {
    let mut kept = KEPT.lock_or_recover();
    kept.retain(|entry| !(entry.session == session && entry.episode == episode));
    kept.push_back(Kept {
        session: session.to_string(),
        episode,
        text,
    });
    while kept.len() > KEEP {
        kept.pop_front();
    }
}

/// The summary of `session`'s turn ending `episode`, if one was written.
pub(crate) fn for_episode(session: &str, episode: EpisodeId) -> Option<String> {
    KEPT.lock_or_recover()
        .iter()
        .rev()
        .find(|entry| entry.session == session && entry.episode == episode)
        .map(|entry| entry.text.clone())
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

    #[test]
    fn a_summary_belongs_to_its_episode_and_the_table_is_bounded() {
        let _serial = TEST_LOCK.lock_or_recover();
        KEPT.lock_or_recover().clear();
        keep("deck-a", 7, "first".into());
        keep("deck-a", 7, "again".into());
        assert_eq!(
            for_episode("deck-a", 7).as_deref(),
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
        };
        summarize(job(1), |_| {
            Ok(("\"已修好登录测试，问是否提交\"\nextra".into(), "anthropic"))
        });
        assert_eq!(
            for_episode("deck-a", 1).as_deref(),
            Some("已修好登录测试，问是否提交")
        );
        summarize(job(2), |_| Err("busy"));
        summarize(job(3), |_| Ok(("  \n".into(), "openai")));
        assert_eq!(for_episode("deck-a", 2), None);
        assert_eq!(for_episode("deck-a", 3), None);
        KEPT.lock_or_recover().clear();
    }
}
