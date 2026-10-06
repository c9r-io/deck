//! Verified shell-exit evidence: the ONE source of automatic card retirement
//! for an ordinary shell (Ctrl+D, `exit N`, the owning shell ending on its own).
//!
//! # Contract
//! Absence is not deletion authority. A card's session missing from a
//! successful listing says nothing about WHY it is gone: the shell exited, the
//! tmux server was replaced or crashed, someone ran `kill-session`, or Deck is
//! looking at a fresh server. `poll_sessions` therefore reports three states
//! per session — `Alive`, `ExitedNormally`, `Missing` — and a failed listing
//! is `Unavailable` (the whole poll errors). Only `ExitedNormally` may lead to
//! retirement, and only through the frontend's ordinary durable close.
//!
//! The evidence is written by tmux itself, at the moment the pane's process
//! ends, never reconstructed by Deck. Every Deck server runs with
//! `remain-on-exit on` and a global `pane-died` hook (`server_setup`,
//! `conf_lines`) that, in one server command list,
//!   1. appends one closed-format record to the server option `@deck_exits`:
//!      `x1|<server pid>|<server start_time>|$<session>|%<pane>|
//!      <window_panes>|<session_windows>|<exit status>|<exit signal>;`
//!   2. `kill-pane`s the dead pane, so the session is destroyed exactly as it
//!      was without `remain-on-exit`.
//!
//! The dead pane is never observable to a client: tmux runs the global
//! notification queue (the hook) before any client's command queue in the
//! same server loop, so a listing, `send-keys` or guarded paste either runs
//! before the death or after the pane is gone (`tests/tmux_contract.rs`
//! hammers this). `prompt_delivery` additionally requires `pane_dead == 0`
//! inside its atomic guard.
//!
//! The ledger is append-only and bounded AT ITS SOURCE: the hook rewrites the
//! option as the last `LEDGER_LIMIT` characters of its old value plus the new
//! record (`#{=-N:…}`). Deck only ever READS it (a `display-message` line in
//! the same tmux command list as the pane listing, `SERVER_FORMAT`), so no
//! consumer can erase a concurrent append; there is no clear step to race.
//! When the last session ended, the server is empty and `list-panes -a`
//! fails; `tmux::snapshot_or_empty_with` then proves a reachable server with
//! zero sessions and still delivers its server line, so that exit counts.
//! Overflow drops the oldest records and truncates at most the first one,
//! which fails the strict parse: lost evidence means a stopped card, never a
//! deletion. Record fields are tmux-generated numbers, ids and a signal name;
//! no session name or other user text enters the ledger.
//!
//! Identity: a record authorizes nothing by itself. `ExitEvidence` remembers
//! the identity Deck last POSITIVELY observed for each requested session name
//! — server pid + server start time + `$session` + its `%panes` — and a
//! missing name is `ExitedNormally` only when a record matches that identity
//! exactly, names one of its observed panes, was the LAST pane of the session
//! (`window_panes == 1 && session_windows == 1`: a non-final pane dying is not
//! the session ending), carries an exit status and no signal (`exit 7` is a
//! normal exit; a signal death is not), and the session is not MCP-managed
//! (a runner ending is a job end, never a card retirement). A name reused by
//! a new session replaces the identity, so old evidence can never match it.
//! Never observed alive in this process (a session created and ended between
//! two polls, a Deck restart) means `Missing`. `forget_all` drops every
//! identity; an intentional service restart calls it before it sends any key,
//! so a shell that ends during the restart transaction stays stopped.
//!
//! Not evidence, ever: `pty-exit` (the attach client's stream ended), tmux's
//! `[exited]` text, `alive=false`, the last key typed, the foreground process
//! name, or elapsed time. This file is dependency-free so the tmux contract
//! suite includes it (`#[path]`) and drives the production hook and parser
//! against the bundled tmux.

use std::collections::{BTreeMap, BTreeSet};

/// The server option holding the exit ledger.
pub(crate) const LEDGER_OPTION: &str = "@deck_exits";
/// Characters of older records the hook keeps before appending a new one.
pub(crate) const LEDGER_LIMIT: usize = 4096;
const RECORD_TAG: &str = "x1";
/// Marks the server-identity line of a pane snapshot.
pub(crate) const SERVER_TAG: &str = "deck-exits";
/// `display-message` format of the snapshot's server line: server identity
/// and the whole ledger, read in the same command list as the pane listing.
/// The last field is another module's ledger (`bell.rs`), carried here raw
/// and never read by this file: one server line serves both.
pub(crate) const SERVER_FORMAT: &str =
    "deck-exits\t#{pid}\t#{start_time}\t#{@deck_exits}\t#{@deck_bells}";

/// The `pane-died` hook body: append one record, then remove the dead pane.
pub(crate) fn pane_died_hook() -> String {
    format!(
        "set-option -gF {LEDGER_OPTION} \"#{{=-{LEDGER_LIMIT}:{LEDGER_OPTION}}}{RECORD_TAG}|#{{pid}}|#{{start_time}}|#{{session_id}}|#{{pane_id}}|#{{window_panes}}|#{{session_windows}}|#{{pane_dead_status}}|#{{pane_dead_signal}};\" ; kill-pane"
    )
}

/// The two server settings, in order, as tmux argv. The hook comes first: a
/// server must never keep dead panes without the hook that removes them, so
/// callers apply `remain-on-exit` only after the hook was installed.
pub(crate) fn server_setup() -> [Vec<String>; 2] {
    [
        vec![
            "set-hook".into(),
            "-g".into(),
            "pane-died".into(),
            pane_died_hook(),
        ],
        vec![
            "set-option".into(),
            "-g".into(),
            "remain-on-exit".into(),
            "on".into(),
        ],
    ]
}

/// The same settings as tmux.conf lines, hook first.
pub(crate) fn conf_lines() -> String {
    format!(
        "set-hook -g pane-died '{}'\nset -g remain-on-exit on\n",
        pane_died_hook()
    )
}

/// One strictly parsed ledger record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ExitRecord {
    pub(crate) server_pid: u32,
    pub(crate) server_start: u64,
    pub(crate) session_id: String,
    pub(crate) pane_id: String,
    pub(crate) window_panes: u32,
    pub(crate) session_windows: u32,
    /// Exit status of a process that exited; `None` when a signal ended it.
    pub(crate) status: Option<u8>,
    pub(crate) signal: Option<String>,
}

impl ExitRecord {
    /// The session ended with this pane, and its process exited by itself.
    fn normal_final_exit(&self) -> bool {
        self.window_panes == 1
            && self.session_windows == 1
            && self.status.is_some()
            && self.signal.is_none()
    }
}

fn tmux_id(value: &str, prefix: char) -> bool {
    value
        .strip_prefix(prefix)
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

fn number<T: std::str::FromStr>(value: &str) -> Option<T> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

/// Strict: exactly the nine fields, the tag first, ids in their tmux shapes,
/// and exactly one of status and signal. Anything else is `None`.
pub(crate) fn parse_record(text: &str) -> Option<ExitRecord> {
    let fields: Vec<&str> = text.split('|').collect();
    let [tag, pid, start, session, pane, panes, windows, status, signal] = fields[..] else {
        return None;
    };
    if tag != RECORD_TAG || !tmux_id(session, '$') || !tmux_id(pane, '%') {
        return None;
    }
    let server_pid: u32 = number(pid)?;
    let server_start: u64 = number(start)?;
    let window_panes: u32 = number(panes)?;
    let session_windows: u32 = number(windows)?;
    let status = if status.is_empty() {
        None
    } else {
        Some(number::<u8>(status)?)
    };
    let signal = if signal.is_empty() {
        None
    } else if signal.len() <= 16
        && signal
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    {
        Some(signal.to_string())
    } else {
        return None;
    };
    if server_pid == 0 || status.is_some() == signal.is_some() {
        return None;
    }
    Some(ExitRecord {
        server_pid,
        server_start,
        session_id: session.into(),
        pane_id: pane.into(),
        window_panes,
        session_windows,
        status,
        signal,
    })
}

/// Every well-formed record of a ledger value. A record is complete only
/// when its terminating `;` was written, so the unterminated tail and every
/// malformed chunk (the one source truncation cut) are skipped.
pub(crate) fn parse_ledger(ledger: &str) -> Vec<ExitRecord> {
    let complete = ledger.rsplit_once(';').map_or("", |(head, _)| head);
    complete.split(';').filter_map(parse_record).collect()
}

/// The server line of one pane snapshot: identity plus the raw ledger.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ServerLedger {
    pub(crate) server_pid: u32,
    pub(crate) server_start: u64,
    pub(crate) records: Vec<ExitRecord>,
    /// the raw bell ledger (`bell.rs` parses it); empty when absent
    pub(crate) bells: String,
}

/// Parse the body of a `SERVER_FORMAT` line (framing already removed).
pub(crate) fn parse_server_line(body: &str) -> Option<ServerLedger> {
    let mut fields = body.splitn(5, '\t');
    if fields.next()? != SERVER_TAG {
        return None;
    }
    let server_pid: u32 = number(fields.next()?)?;
    let server_start: u64 = number(fields.next()?)?;
    let ledger = fields.next()?;
    // absent on a line cut after the exit ledger: no bell, never a reason
    // to lose the exit evidence beside it
    let bells = fields.next().unwrap_or("");
    if server_pid == 0 {
        return None;
    }
    Some(ServerLedger {
        server_pid,
        server_start,
        records: parse_ledger(ledger),
        bells: bells.to_owned(),
    })
}

/// One live pane of a snapshot, reduced to identity.
pub(crate) struct LivePane<'a> {
    pub(crate) server_pid: u32,
    pub(crate) session_name: &'a str,
    pub(crate) session_id: &'a str,
    pub(crate) pane_id: &'a str,
}

/// What one poll may say about one session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    Alive,
    /// The owning shell of the identity Deck observed ended by itself.
    ExitedNormally,
    /// Absent for a reason Deck cannot prove. Never authority.
    Missing,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Observed {
    server_pid: u32,
    server_start: u64,
    session_id: String,
    panes: BTreeSet<String>,
}

/// Last positively observed identity per requested session name.
#[derive(Debug, Default)]
pub(crate) struct ExitEvidence {
    observed: BTreeMap<String, Observed>,
}

impl ExitEvidence {
    pub(crate) const fn new() -> Self {
        Self {
            observed: BTreeMap::new(),
        }
    }

    /// Classify every requested name against one snapshot. `server` is the
    /// snapshot's server line (`None` when it was absent or malformed, which
    /// disables both observing and authorizing). `managed` names sessions an
    /// MCP runner owns; it must answer `true` when it cannot tell.
    pub(crate) fn classify(
        &mut self,
        names: &[String],
        panes: &[LivePane<'_>],
        server: Option<&ServerLedger>,
        managed: &dyn Fn(&str) -> bool,
    ) -> Vec<Verdict> {
        self.observed.retain(|name, _| names.contains(name));
        // Rows and the server line come from one command list on one server;
        // a disagreement means the snapshot cannot vouch for any identity.
        let server = server.filter(|s| panes.iter().all(|p| p.server_pid == s.server_pid));
        names
            .iter()
            .map(|name| {
                let rows: Vec<&LivePane> =
                    panes.iter().filter(|p| p.session_name == name).collect();
                if !rows.is_empty() {
                    self.observe(name, &rows, server);
                    return Verdict::Alive;
                }
                let Some(server) = server else {
                    return Verdict::Missing;
                };
                let Some(seen) = self.observed.get(name) else {
                    return Verdict::Missing;
                };
                if seen.server_pid != server.server_pid || seen.server_start != server.server_start
                {
                    // another server instance: nothing here can be about it
                    self.observed.remove(name);
                    return Verdict::Missing;
                }
                let exited = server.records.iter().any(|record| {
                    record.server_pid == seen.server_pid
                        && record.server_start == seen.server_start
                        && record.session_id == seen.session_id
                        && seen.panes.contains(&record.pane_id)
                        && record.normal_final_exit()
                });
                if exited && !managed(name) {
                    Verdict::ExitedNormally
                } else {
                    Verdict::Missing
                }
            })
            .collect()
    }

    fn observe(&mut self, name: &str, rows: &[&LivePane<'_>], server: Option<&ServerLedger>) {
        let session_id = rows[0].session_id;
        let Some(server) = server.filter(|_| rows.iter().all(|r| r.session_id == session_id))
        else {
            self.observed.remove(name);
            return;
        };
        let identity = Observed {
            server_pid: server.server_pid,
            server_start: server.server_start,
            session_id: session_id.to_string(),
            panes: rows.iter().map(|r| r.pane_id.to_string()).collect(),
        };
        match self.observed.get_mut(name) {
            // the same session: remember every pane it has shown, so the
            // record of a pane that became the last one still matches
            Some(seen)
                if seen.server_pid == identity.server_pid
                    && seen.server_start == identity.server_start
                    && seen.session_id == identity.session_id =>
            {
                seen.panes.extend(identity.panes);
            }
            _ => {
                self.observed.insert(name.to_string(), identity);
            }
        }
    }

    /// Closed, generation-bound retirement key; never content or authority.
    pub(crate) fn lifecycle(&self, name: &str) -> Option<String> {
        self.observed.get(name).map(|seen| {
            format!(
                "{}:{}:{}",
                seen.server_pid, seen.server_start, seen.session_id
            )
        })
    }

    /// Drop every identity: nothing that ends before a fresh observation can
    /// authorize retirement.
    pub(crate) fn forget_all(&mut self) {
        self.observed.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(session: &str, pane: &str, wp: u32, sw: u32, status: &str, signal: &str) -> String {
        format!("x1|100|5000|{session}|{pane}|{wp}|{sw}|{status}|{signal};")
    }
    fn server(ledger: &str) -> ServerLedger {
        parse_server_line(&format!("deck-exits\t100\t5000\t{ledger}")).unwrap()
    }
    fn pane<'a>(name: &'a str, session: &'a str, pane: &'a str) -> LivePane<'a> {
        LivePane {
            server_pid: 100,
            session_name: name,
            session_id: session,
            pane_id: pane,
        }
    }
    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }
    const ORDINARY: &dyn Fn(&str) -> bool = &|_| false;

    #[test]
    fn the_hook_is_tmux_only_bounded_and_installed_before_remain_on_exit() {
        let hook = pane_died_hook();
        assert!(hook.starts_with("set-option -gF @deck_exits \"#{=-4096:@deck_exits}x1|"));
        assert!(hook.ends_with(";\" ; kill-pane"));
        for forbidden in ["run-shell", "#(", "pipe-pane", "session_name"] {
            assert!(!hook.contains(forbidden), "{forbidden}");
        }
        let [hook_argv, remain] = server_setup();
        assert_eq!(hook_argv[..3], ["set-hook", "-g", "pane-died"]);
        assert_eq!(remain, ["set-option", "-g", "remain-on-exit", "on"]);
        let conf = conf_lines();
        assert!(conf.find("set-hook").unwrap() < conf.find("remain-on-exit").unwrap());
    }

    #[test]
    fn records_parse_strictly_and_truncated_or_malformed_chunks_are_ignored() {
        let good = record("$1", "%2", 1, 1, "0", "");
        assert_eq!(parse_ledger(&good).len(), 1);
        assert_eq!(parse_ledger(&good)[0].status, Some(0));
        let signal = parse_ledger(&record("$1", "%2", 1, 1, "", "kill"));
        assert_eq!(signal[0].signal.as_deref(), Some("kill"));
        for bad in [
            "1|100|5000|$1|%2|1|1|0|",        // truncated tag
            "x1|100|5000|$1|%2|1|1||",        // neither status nor signal
            "x1|100|5000|$1|%2|1|1|0|kill",   // both
            "x1|100|5000|1|%2|1|1|0|",        // bad session id
            "x1|100|5000|$1|2|1|1|0|",        // bad pane id
            "x1|0|5000|$1|%2|1|1|0|",         // zero pid
            "x1|100|5000|$1|%2|1|1|256|",     // status out of range
            "x1|100|5000|$1|%2|1|1|-1|",      // not a number
            "x1|100|5000|$1|%2|1|1||KILL",    // not a tmux signal name
            "x1|100|5000|$1|%2|1|1|0||extra", // extra field
        ] {
            assert_eq!(parse_record(bad), None, "{bad}");
        }
        // an unterminated tail is incomplete even if its fields look whole
        assert!(parse_ledger("x1|100|5000|$1|%2|1|1|0|").is_empty());
        // a front fragment cut by the source bound is skipped, later ones kept
        let ledger = format!("00|5000|$1|%2|1|1|0|;{good}");
        assert_eq!(parse_ledger(&ledger).len(), 1);
        assert_eq!(parse_server_line("deck-exits\t0\t1\t"), None);
        assert_eq!(parse_server_line("other\t1\t1\t"), None);
        assert_eq!(parse_server_line("deck-exits\t1\tx\t"), None);
    }

    /// Observe `name` alive as `$1/%2`, then report it missing with `ledger`.
    fn after_exit(ledger: &str, managed: &dyn Fn(&str) -> bool) -> Verdict {
        let mut evidence = ExitEvidence::default();
        let all = names(&["card"]);
        let live = [pane("card", "$1", "%2")];
        assert_eq!(
            evidence.classify(&all, &live, Some(&server("")), ORDINARY),
            [Verdict::Alive]
        );
        evidence.classify(&all, &[], Some(&server(ledger)), managed)[0]
    }

    #[test]
    fn only_a_normal_final_exit_of_the_observed_identity_authorizes() {
        let exit0 = record("$1", "%2", 1, 1, "0", "");
        assert_eq!(after_exit(&exit0, ORDINARY), Verdict::ExitedNormally);
        assert_eq!(
            after_exit(&record("$1", "%2", 1, 1, "7", ""), ORDINARY),
            Verdict::ExitedNormally,
            "exit N is a normal termination"
        );
        assert_eq!(
            after_exit(&record("$1", "%2", 1, 1, "", "kill"), ORDINARY),
            Verdict::Missing,
            "a signal death is not"
        );
        assert_eq!(after_exit("", ORDINARY), Verdict::Missing, "absence alone");
        assert_eq!(
            after_exit(&record("$1", "%2", 2, 1, "0", ""), ORDINARY),
            Verdict::Missing,
            "a non-final pane"
        );
        assert_eq!(
            after_exit(&record("$1", "%2", 1, 2, "0", ""), ORDINARY),
            Verdict::Missing,
            "another window remained"
        );
        assert_eq!(
            after_exit(&record("$9", "%2", 1, 1, "0", ""), ORDINARY),
            Verdict::Missing,
            "another session id"
        );
        assert_eq!(
            after_exit(&record("$1", "%9", 1, 1, "0", ""), ORDINARY),
            Verdict::Missing,
            "a pane never observed"
        );
        assert_eq!(
            after_exit(&exit0.replace("x1|100|5000", "x1|100|5001"), ORDINARY),
            Verdict::Missing,
            "a record from another server instance"
        );
        assert_eq!(
            after_exit(&exit0, &|_| true),
            Verdict::Missing,
            "an MCP-managed session"
        );
        assert_eq!(
            after_exit(&exit0.replace('|', ":"), ORDINARY),
            Verdict::Missing,
            "malformed evidence"
        );
    }

    #[test]
    fn a_name_never_observed_or_on_another_server_is_missing() {
        let exit0 = record("$1", "%2", 1, 1, "0", "");
        let mut evidence = ExitEvidence::default();
        let all = names(&["card"]);
        assert_eq!(
            evidence.classify(&all, &[], Some(&server(&exit0)), ORDINARY),
            [Verdict::Missing],
            "Deck restarted: no identity was observed in this process"
        );
        evidence.classify(
            &all,
            &[pane("card", "$1", "%2")],
            Some(&server("")),
            ORDINARY,
        );
        let replaced = parse_server_line(&format!("deck-exits\t100\t6000\t{exit0}")).unwrap();
        assert_eq!(
            evidence.classify(&all, &[], Some(&replaced), ORDINARY),
            [Verdict::Missing],
            "the same pid with another start time is another server"
        );
        // and the stale identity is gone for good
        assert_eq!(
            evidence.classify(&all, &[], Some(&server(&exit0)), ORDINARY),
            [Verdict::Missing]
        );
        evidence.classify(
            &all,
            &[pane("card", "$1", "%2")],
            Some(&server("")),
            ORDINARY,
        );
        assert_eq!(
            evidence.classify(&all, &[], None, ORDINARY),
            [Verdict::Missing],
            "no server line, no authority"
        );
        let rows = [pane("card", "$1", "%2")];
        assert_eq!(
            evidence.classify(&all, &rows, None, ORDINARY),
            [Verdict::Alive]
        );
        assert_eq!(
            evidence.classify(&all, &[], Some(&server(&exit0)), ORDINARY),
            [Verdict::Missing],
            "an observation without a server line recorded no identity"
        );
    }

    #[test]
    fn a_reused_name_never_consumes_old_evidence() {
        let old = record("$1", "%2", 1, 1, "0", "");
        let mut evidence = ExitEvidence::default();
        let all = names(&["card"]);
        evidence.classify(
            &all,
            &[pane("card", "$1", "%2")],
            Some(&server("")),
            ORDINARY,
        );
        // the old shell exited; before any poll the card was reopened
        let reopened = [pane("card", "$4", "%5")];
        assert_eq!(
            evidence.classify(&all, &reopened, Some(&server(&old)), ORDINARY),
            [Verdict::Alive]
        );
        // the new session disappears without a record of its own
        assert_eq!(
            evidence.classify(&all, &[], Some(&server(&old)), ORDINARY),
            [Verdict::Missing]
        );
    }

    #[test]
    fn the_last_of_several_observed_panes_ending_is_the_session_ending() {
        let mut evidence = ExitEvidence::default();
        let all = names(&["card"]);
        let two = [pane("card", "$1", "%2"), pane("card", "$1", "%3")];
        evidence.classify(&all, &two, Some(&server("")), ORDINARY);
        let first = record("$1", "%3", 2, 1, "0", "");
        assert_eq!(
            evidence.classify(
                &all,
                &[pane("card", "$1", "%2")],
                Some(&server(&first)),
                ORDINARY
            ),
            [Verdict::Alive]
        );
        let last = format!("{first}{}", record("$1", "%2", 1, 1, "0", ""));
        assert_eq!(
            evidence.classify(&all, &[], Some(&server(&last)), ORDINARY),
            [Verdict::ExitedNormally]
        );
    }

    #[test]
    fn forgetting_and_unrequested_names_drop_identities() {
        let exit0 = record("$1", "%2", 1, 1, "0", "");
        let mut evidence = ExitEvidence::default();
        let all = names(&["card"]);
        evidence.classify(
            &all,
            &[pane("card", "$1", "%2")],
            Some(&server("")),
            ORDINARY,
        );
        evidence.forget_all();
        assert_eq!(
            evidence.classify(&all, &[], Some(&server(&exit0)), ORDINARY),
            [Verdict::Missing]
        );
        evidence.classify(
            &all,
            &[pane("card", "$1", "%2")],
            Some(&server("")),
            ORDINARY,
        );
        evidence.classify(&[], &[], Some(&server("")), ORDINARY);
        assert_eq!(
            evidence.classify(&all, &[], Some(&server(&exit0)), ORDINARY),
            [Verdict::Missing],
            "a card that stopped being requested was closed"
        );
    }

    #[test]
    fn rows_from_another_server_than_the_server_line_vouch_for_nothing() {
        let exit0 = record("$1", "%2", 1, 1, "0", "");
        let mut evidence = ExitEvidence::default();
        let all = names(&["card"]);
        let foreign = [LivePane {
            server_pid: 999,
            ..pane("card", "$1", "%2")
        }];
        evidence.classify(&all, &foreign, Some(&server("")), ORDINARY);
        assert_eq!(
            evidence.classify(&all, &[], Some(&server(&exit0)), ORDINARY),
            [Verdict::Missing]
        );
    }
}
