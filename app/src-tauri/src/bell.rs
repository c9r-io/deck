//! Terminal bells as an attention observation: a program in a session that
//! reports no agent state rang the bell while nobody was looking at it.
//!
//! # Contract
//! The one upstream fact Deck consumes for a session without hook state. A
//! program rings the bell because it wants a person (`make; printf '\a'`, a
//! tool configured to ring); Deck forwards exactly that and infers nothing:
//! not that a command ended, not that it succeeded, not that the session is
//! idle. Quiet time, output matching, command names, the foreground process,
//! `alert-activity` and `alert-silence` are never read here, and OSC 9 / 777
//! / 133 never reach Deck at all (tmux drops them).
//!
//! The evidence is written by tmux itself. Every Deck server has a global
//! `alert-bell` hook (`conf_lines` in tmux.conf for a new server,
//! `server_setup` through `tmux::init_deck_server_with` for a reused one)
//! whose whole body is one
//! `set-option`: it appends `b1|$<session>|@<window>|%<pane>|<time>;` to the
//! server option `@deck_bells`, where `<time>` is the window's activity
//! time, which the bell's own output just set. It starts no process, and
//! tmux fires it for every bell whether or not a client is attached (a
//! burst inside one write is coalesced by tmux). The ledger is bounded AT
//! ITS SOURCE like the exit ledger (`shell_exit.rs`): the hook keeps the last
//! `LEDGER_LIMIT` characters of the old value. Deck only ever READS it, in
//! the Board poll's own command list (`shell_exit::SERVER_FORMAT`), so there
//! is no clear step to race an append. Overflow drops the oldest records and
//! truncates at most the first, which then fails the strict parse: a lost
//! record is a bell nobody is told about, nothing else. Fields are
//! tmux-generated ids and a number; no user text enters the ledger. The
//! hook is purely additive: an older Deck reusing this server never reads
//! the option, so `SERVER_PROTOCOL` is unchanged.
//!
//! `Bells::observe` (one call per successful Board poll) turns the ledger
//! into "rang, unseen" per session:
//! - a bell counts only for a session whose current tmux `$id` the record
//!   names (a reused name never inherits a bell) and which has NO agent hook
//!   state: an agent that reports its state has the better signal, and its
//!   bells are marked seen so they never surface later;
//! - it does not count while the session is WATCHED — the Deck window is in
//!   front and a pane shows the card. That is where a person's own keys ring
//!   (a failed completion, the end of a pager), and what they see needs no
//!   notice. A pane left open while Deck is in the background is not being
//!   watched, so `make; printf '\a'` in it counts;
//! - one episode until viewed: further bells while one is unseen change
//!   nothing, so a program that keeps ringing costs one notice;
//! - viewed = the session is observed watched; nothing else clears it, and
//!   a session that is gone is forgotten;
//! - what was already in the ledger when this process first read a server
//!   (a Deck restart, a replaced server) is the baseline and never surfaces:
//!   Deck starting must not announce hours-old bells to a user sitting at
//!   it. Bells rung while Deck was not running are therefore not reported.
//!   Nothing here is persisted.
//!
//! Attention only, never authority: the result is a list row, a Dock count
//! and an away notification (`notify.rs`, its fourth source). It releases,
//! holds, retires and moves nothing, is not a card status, and no scheduler
//! or lifecycle code reads it (`tests/signal_census.rs`). An unreadable or
//! malformed server line is simply no bell.
//!
//! Cadence: the Board poll drives this, so a bell is noticed at the next
//! poll (the webview's timer, which macOS slows while Deck is hidden), not
//! at the instant it rings.
//!
//! This file is dependency-free so the tmux contract suite includes it
//! (`#[path]`) and drives the production hook and parser against the
//! bundled tmux.

use std::collections::HashMap;

/// The server option holding the bell ledger.
pub(crate) const LEDGER_OPTION: &str = "@deck_bells";
/// Characters of older records the hook keeps before appending a new one.
pub(crate) const LEDGER_LIMIT: usize = 2048;
const RECORD_TAG: &str = "b1";

/// The `alert-bell` hook body: append one record.
pub(crate) fn alert_bell_hook() -> String {
    format!(
        "set-option -gF {LEDGER_OPTION} \"#{{=-{LEDGER_LIMIT}:{LEDGER_OPTION}}}{RECORD_TAG}|#{{session_id}}|#{{window_id}}|#{{pane_id}}|#{{window_activity}};\""
    )
}

/// The one server setting, as tmux argv.
pub(crate) fn server_setup() -> Vec<String> {
    vec![
        "set-hook".into(),
        "-g".into(),
        "alert-bell".into(),
        alert_bell_hook(),
    ]
}

/// The same setting as a tmux.conf line.
pub(crate) fn conf_lines() -> String {
    format!("set-hook -g alert-bell '{}'\n", alert_bell_hook())
}

/// One bell: the session it rang in and when (epoch seconds).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BellRecord {
    pub(crate) session_id: String,
    pub(crate) at: u64,
}

fn id(value: &str, sigil: char) -> bool {
    value
        .strip_prefix(sigil)
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

fn record(raw: &str) -> Option<BellRecord> {
    let mut fields = raw.split('|');
    if fields.next()? != RECORD_TAG {
        return None;
    }
    let (session, window, pane, at) = (
        fields.next()?,
        fields.next()?,
        fields.next()?,
        fields.next()?,
    );
    if fields.next().is_some()
        || !id(session, '$')
        || !id(window, '@')
        || !id(pane, '%')
        || at.is_empty()
        || !at.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    Some(BellRecord {
        session_id: session.to_owned(),
        at: at.parse().ok()?,
    })
}

/// Every well-formed record of a ledger value, oldest first. Anything else
/// (the record a bounded append cut in half, foreign text) is skipped.
pub(crate) fn parse_ledger(ledger: &str) -> Vec<BellRecord> {
    ledger.split(';').filter_map(record).collect()
}

/// One live session of a poll and what Deck knows about it right now.
pub(crate) struct Session<'a> {
    pub(crate) name: &'a str,
    /// tmux `$N`
    pub(crate) id: &'a str,
    /// an agent hook state is projected for it (its bells are ignored)
    pub(crate) reports: bool,
    /// the Deck window is in front and a pane shows this session
    pub(crate) watched: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Mark {
    id: String,
    at: u64,
}

/// Runtime-only bell bookkeeping for one Deck process (module header).
#[derive(Default)]
pub(crate) struct Bells {
    /// the server whose ledger the marks below describe
    server: Option<(u32, u64)>,
    /// bells at or before this were in the ledger when it was first read
    baseline: u64,
    /// per session name: the newest bell already seen or ignored
    seen: HashMap<String, Mark>,
    /// per session name: the bell that rang unseen
    rung: HashMap<String, Mark>,
}

impl Bells {
    /// One successful poll. Returns the sessions with an unseen bell, each
    /// with an opaque key for that episode.
    pub(crate) fn observe(
        &mut self,
        server: (u32, u64),
        ledger: &[BellRecord],
        sessions: &[Session<'_>],
    ) -> HashMap<String, String> {
        if self.server != Some(server) {
            *self = Self {
                server: Some(server),
                baseline: ledger.iter().map(|r| r.at).max().unwrap_or(0),
                ..Self::default()
            };
        }
        // a name that now belongs to another session keeps nothing; a
        // session missing from this poll stops ringing but stays seen, so a
        // poll that leaves one out never brings a viewed bell back
        self.seen
            .retain(|name, mark| !sessions.iter().any(|s| s.name == name && s.id != mark.id));
        self.rung
            .retain(|name, mark| sessions.iter().any(|s| s.name == name && s.id == mark.id));
        for session in sessions {
            let newest = ledger
                .iter()
                .filter(|r| r.session_id == session.id && r.at > self.baseline)
                .map(|r| r.at)
                .max();
            if session.reports || session.watched {
                // ignored, or viewed: nothing of this session is pending,
                // and what rang so far never surfaces later
                self.rung.remove(session.name);
                if let Some(at) = newest {
                    self.seen.insert(
                        session.name.to_owned(),
                        Mark {
                            id: session.id.to_owned(),
                            at,
                        },
                    );
                }
                continue;
            }
            let Some(at) = newest else { continue };
            let seen = self.seen.get(session.name).map_or(0, |mark| mark.at);
            if at > seen && !self.rung.contains_key(session.name) {
                self.rung.insert(
                    session.name.to_owned(),
                    Mark {
                        id: session.id.to_owned(),
                        at,
                    },
                );
            }
        }
        self.rung()
    }

    /// The sessions with an unseen bell as last observed, each with an
    /// opaque key for that episode: what a poll that could not read the
    /// ledger keeps saying.
    pub(crate) fn rung(&self) -> HashMap<String, String> {
        self.rung
            .iter()
            .map(|(name, mark)| (name.clone(), format!("{}:{}", mark.id, mark.at)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERVER: (u32, u64) = (100, 1_000);

    fn bell(session: &str, at: u64) -> BellRecord {
        BellRecord {
            session_id: session.into(),
            at,
        }
    }

    fn shell<'a>(name: &'a str, id: &'a str) -> Session<'a> {
        Session {
            name,
            id,
            reports: false,
            watched: false,
        }
    }

    fn rang(bells: &mut Bells, ledger: &[BellRecord], sessions: &[Session<'_>]) -> Vec<String> {
        let mut names: Vec<String> = bells
            .observe(SERVER, ledger, sessions)
            .into_keys()
            .collect();
        names.sort();
        names
    }

    #[test]
    fn the_hook_appends_one_bounded_record_and_starts_nothing() {
        let hook = alert_bell_hook();
        assert_eq!(
            hook,
            "set-option -gF @deck_bells \"#{=-2048:@deck_bells}b1|#{session_id}|#{window_id}|#{pane_id}|#{window_activity};\""
        );
        for verb in [
            "run-shell",
            "if-shell",
            "pipe-pane",
            "respawn",
            "new-",
            "send-keys",
            ";",
        ] {
            assert!(!hook.replace(";\"", "\"").contains(verb), "{verb}");
        }
        assert_eq!(server_setup()[..3], ["set-hook", "-g", "alert-bell"]);
        assert_eq!(server_setup()[3], hook);
        assert_eq!(conf_lines(), format!("set-hook -g alert-bell '{hook}'\n"));
        assert!(!hook.contains('\''), "safe inside the conf line's quotes");
    }

    #[test]
    fn the_ledger_parse_is_strict_and_skips_what_it_cannot_read() {
        assert_eq!(
            parse_ledger("b1|$1|@1|%1|1700000000;b1|$12|@3|%40|1700000009;"),
            [bell("$1", 1_700_000_000), bell("$12", 1_700_000_009)]
        );
        // the head a bounded append cut, another tag, wrong ids, extra or
        // missing fields, a non-number, an overflowing number
        for bad in [
            "|%1|1700000000;",
            "1|$1|@1|%1|5;",
            "x1|$1|@1|%1|5;",
            "b1|1|@1|%1|5;",
            "b1|$|@1|%1|5;",
            "b1|$1|1|%1|5;",
            "b1|$1|@1|1|5;",
            "b1|$1|@1|%1;",
            "b1|$1|@1|%1|5|6;",
            "b1|$1|@1|%1|;",
            "b1|$1|@1|%1|-5;",
            "b1|$1|@1|%1|99999999999999999999999;",
            "b1|$1 |@1|%1|5;",
            "",
        ] {
            assert!(parse_ledger(bad).is_empty(), "{bad:?}");
        }
        assert_eq!(
            parse_ledger("%1|17;b1|$2|@2|%2|20;garbage;b1|$3|@3|%3|21"),
            [bell("$2", 20), bell("$3", 21)]
        );
    }

    #[test]
    fn a_bell_in_an_unwatched_shell_rings_once_until_it_is_viewed() {
        let mut bells = Bells::default();
        let shells = || [shell("a", "$1"), shell("b", "$2")];
        assert!(rang(&mut bells, &[], &shells()).is_empty());
        let ledger = [bell("$1", 10)];
        assert_eq!(rang(&mut bells, &ledger, &shells()), ["a"]);
        let key = bells.observe(SERVER, &ledger, &shells())["a"].clone();
        // it keeps ringing: the same episode, the same key
        let ledger = [bell("$1", 10), bell("$1", 11), bell("$1", 30)];
        assert_eq!(bells.observe(SERVER, &ledger, &shells())["a"], key);
        // viewed: the window is in front and a pane shows it
        let watched = [
            Session {
                watched: true,
                ..shell("a", "$1")
            },
            shell("b", "$2"),
        ];
        assert!(rang(&mut bells, &ledger, &watched).is_empty());
        // nothing already seen comes back, and a bell while watched is not one
        assert!(rang(&mut bells, &ledger, &shells()).is_empty());
        let ledger = [bell("$1", 30), bell("$1", 31)];
        assert!(rang(&mut bells, &ledger, &watched).is_empty());
        assert!(rang(&mut bells, &ledger, &shells()).is_empty());
        // a later one is a new episode with a new key
        let ledger = [bell("$1", 31), bell("$1", 40)];
        assert_eq!(rang(&mut bells, &ledger, &shells()), ["a"]);
        assert_ne!(bells.observe(SERVER, &ledger, &shells())["a"], key);
    }

    #[test]
    fn a_session_that_reports_agent_state_never_rings() {
        let mut bells = Bells::default();
        let agent = [Session {
            reports: true,
            ..shell("a", "$1")
        }];
        assert!(rang(&mut bells, &[], &agent).is_empty());
        let ledger = [bell("$1", 10)];
        assert!(rang(&mut bells, &ledger, &agent).is_empty());
        // losing the hook state later does not surface the old bell
        assert!(rang(&mut bells, &ledger, &[shell("a", "$1")]).is_empty());
        // and gaining it withdraws one that was ringing
        let ledger = [bell("$1", 10), bell("$1", 20)];
        assert_eq!(rang(&mut bells, &ledger, &[shell("a", "$1")]), ["a"]);
        assert!(rang(&mut bells, &ledger, &agent).is_empty());
    }

    #[test]
    fn what_was_in_the_ledger_first_is_the_baseline() {
        let mut bells = Bells::default();
        let ledger = [bell("$1", 10), bell("$2", 12)];
        let shells = [shell("a", "$1"), shell("b", "$2")];
        assert!(rang(&mut bells, &ledger, &shells).is_empty());
        let ledger = [bell("$1", 10), bell("$2", 12), bell("$2", 13)];
        assert_eq!(rang(&mut bells, &ledger, &shells), ["b"]);
        // a replaced server starts over: its first ledger is a baseline too
        let ledger = [bell("$1", 50)];
        assert!(bells.observe((200, 2_000), &ledger, &shells).is_empty());
        let ledger = [bell("$1", 50), bell("$1", 51)];
        assert_eq!(
            bells
                .observe((200, 2_000), &ledger, &shells)
                .into_keys()
                .collect::<Vec<_>>(),
            ["a"]
        );
    }

    #[test]
    fn a_bell_belongs_to_the_session_id_it_rang_in() {
        let mut bells = Bells::default();
        assert!(rang(&mut bells, &[], &[shell("a", "$1")]).is_empty());
        let ledger = [bell("$1", 10)];
        assert_eq!(rang(&mut bells, &ledger, &[shell("a", "$1")]), ["a"]);
        // the name now belongs to a new session: the old bell is not its own
        assert!(rang(&mut bells, &ledger, &[shell("a", "$7")]).is_empty());
        // a session missing from a poll does not ring; back with the same
        // id, its unseen bell is still unseen and its viewed one stays viewed
        assert_eq!(
            rang(&mut bells, &[bell("$7", 11)], &[shell("a", "$7")]),
            ["a"]
        );
        assert!(rang(&mut bells, &[bell("$7", 11)], &[]).is_empty());
        assert_eq!(
            rang(&mut bells, &[bell("$7", 11)], &[shell("a", "$7")]),
            ["a"]
        );
        let watched = [Session {
            watched: true,
            ..shell("a", "$7")
        }];
        assert!(rang(&mut bells, &[bell("$7", 11)], &watched).is_empty());
        assert!(rang(&mut bells, &[bell("$7", 11)], &[]).is_empty());
        assert!(rang(&mut bells, &[bell("$7", 11)], &[shell("a", "$7")]).is_empty());
        // records lost to the bound do not end an unseen episode
        let mut bells = Bells::default();
        assert!(rang(&mut bells, &[], &[shell("a", "$1")]).is_empty());
        assert_eq!(
            rang(&mut bells, &[bell("$1", 10)], &[shell("a", "$1")]),
            ["a"]
        );
        assert_eq!(rang(&mut bells, &[], &[shell("a", "$1")]), ["a"]);
    }
}
