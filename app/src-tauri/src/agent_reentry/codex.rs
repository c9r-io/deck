//! Codex's re-entry knowledge: whose code a client must be, the supported
//! launch grammar of an embedded (`--no-daemon`) interactive client, the
//! closed lifecycle words with what a record of them means (`RECEIPTS`),
//! the exit hint the client prints, and the PURE construction of an
//! exact-id request.
//!
//! # Contract
//! Everything here is static data or a pure function, as in `claude.rs`:
//! no pane, PTY, queue, Board or authority handle; nothing here decides that
//! an entry may launch. The core hands it the grant it decided
//! (`agent_reentry::decide`, `Grant`) and gets argument words back. A
//! request is rebuilt from structured fields only: `resume --no-daemon`,
//! then the words of the grant, then the exact id. No model, directory,
//! prompt, `-c`/`-C` override, profile or environment word can be produced,
//! and no observed argv is ever replayed.
//!
//! # What Codex gives, and what it does not (0.163.0, read in source and
//! run in an isolated experiment on 2026-10-17; the screenshots of
//! 2026-10-18)
//! - **The final session is the hook AND the exit hint, crossed.** Codex's
//!   `SessionEnd` carries no "current" mark, its reason is the constant
//!   `other`, and a client that switched threads (`/new`, `/resume`) sends
//!   one `SessionEnd` per loaded root at its exit, in no order. What names
//!   the input-target thread is the exit summary the TUI prints to the pane
//!   as it leaves — `To continue this session, run: codex resume <id>` — the
//!   one moment that thread leaves the TUI's memory. So `final_session`
//!   takes two things: the facts of the exit lease (kernel-attributed
//!   receipts of the frozen client) and the ids the pane printed between the
//!   freeze and the seal (`exit_hint`, read by the core as the delta of two
//!   captures of the same pane). The hook proves whose ids they are; the
//!   printed line says which of them is the current one. Rules: a printed
//!   id must be among the receipts' ids, and the last printed wins; with
//!   nothing printed exactly one receipt id is taken; anything else names
//!   nothing. Pane text alone never names a session.
//! - **No entry receipt before the first turn.** Codex queues `SessionStart`
//!   at session init and runs it inside the first turn only; a resume with
//!   no prompt shows nothing within the post share (0 of 15 windows). The
//!   capability says so (`entry_receipt: false`): a started client whose
//!   launch image, directory and environment match the request is
//!   `Started`, never `Confirmed` — unless a `resume` receipt does arrive in
//!   the share (the user typed at once, or a goal or queued input started a
//!   turn by itself), which confirms it by the same rule as Claude's.
//! - **A bare resume keeps bypass.** Codex restores every permission field
//!   the request omits from the thread's last saved settings, bypass
//!   included; `--yolo` sets exactly sandbox `danger-full-access` and
//!   approval `never`. Policy A on a bypass launch therefore asks for the
//!   two words that undo those two fields (`Grant::Lower`); the saved
//!   reviewer, roots and the rest stay the session's own, as they do for
//!   Claude. Policy B is `--yolo` again. An ordinary launch is a bare
//!   resume under either policy. Each resume writes back what it applied.
//! - **Hooks run through the user's shell** (`$SHELL -c`); the helper is
//!   the client's direct child only because zsh and bash exec a simple
//!   command. Deck's hook is one simple command, and a receipt whose helper
//!   has another parent is refused by the kernel attribution as before.
//! - **Topology.** Only a client started with `--no-daemon` is supported:
//!   the shared daemon's receipts cannot be bound to a pane
//!   (`docs/codex-signal-certification-20261002.md`). The flag is required
//!   by the grammar, and the request carries it again.
//!
//! None of this names a release: the product requirement is OpenAI's
//! signing identity for the `codex` executable, and the contract above is
//! what THIS transaction observes of the client.

use super::lifecycle::{self, Event, Fact, Receipt, ReceiptContract, Words};
use super::{claude, Grant, ReentryCapability, Request};
use crate::procinfo::{LaunchGrammar, ProductIdentity};

/// Codex CLI as OpenAI signs it: the `codex` identifier, a chain to Apple's
/// root through the Developer ID authority, a Developer ID Application leaf
/// and OpenAI's Team. No release, digest or leaf certificate.
const PRODUCT: ProductIdentity = ProductIdentity {
    requirement: "identifier \"codex\" and anchor apple generic \
        and certificate 1[field.1.2.840.113635.100.6.2.6] exists \
        and certificate leaf[field.1.2.840.113635.100.6.1.13] exists \
        and certificate leaf[subject.OU] = \"2DC432GLL2\"",
};

const BYPASS: &str = "--yolo";
const RESUME: &str = "resume";
const EMBEDDED: &str = "--no-daemon";
/// The two words that undo what `--yolo` sets (Policy A on a bypass launch).
const LOWER: [&str; 4] = [
    "--sandbox",
    "workspace-write",
    "--ask-for-approval",
    "on-request",
];

/// The launch shapes a direct embedded client is supported in: `--no-daemon`
/// exactly once, `--yolo` or not, the resume word alone (the native picker)
/// or with one id, a model, directories. Anything else — a prompt, `exec`,
/// `--ask-for-approval`, `--full-auto`, a profile, a `-c` override, the
/// hook-trust bypass, a daemon client — classifies as unsupported and is
/// never re-entered automatically.
const GRAMMAR: LaunchGrammar = LaunchGrammar {
    bypass: BYPASS,
    resume: RESUME,
    resume_value: claude::native_id_ok,
    resume_optional: true,
    required: &[EMBEDDED],
    mode: "--sandbox",
    ordinary_modes: &["workspace-write", "read-only"],
    dropped: &["--model", "-m"],
    directories: "--add-dir",
    refused_env: &["CODEX_HOME"],
    refused_env_prefixes: &["OPENAI_"],
};

/// The argument words of an exact-id request: `resume --no-daemon`, the
/// grant's words, the id. `None` for anything that is not a native id.
fn request(native_id: &str, grant: Grant) -> Option<Request> {
    if !claude::native_id_ok(native_id) {
        return None;
    }
    let mut words = vec![RESUME.to_string(), EMBEDDED.to_string()];
    let lowered = match grant {
        Grant::Plain => false,
        Grant::Reapply => {
            words.push(BYPASS.to_string());
            false
        }
        Grant::Lower => {
            words.extend(LOWER.iter().map(|word| word.to_string()));
            true
        }
    };
    words.push(native_id.to_string());
    Some(Request { words, lowered })
}

pub(crate) const CAPABILITY: ReentryCapability = ReentryCapability {
    source: "codex",
    product: &PRODUCT,
    launcher: "codex",
    grammar: &GRAMMAR,
    request,
    receipts: &RECEIPTS,
    exit_hint: Some("codex"),
    entry_receipt: false,
};

pub(crate) const RECEIPTS: ReceiptContract = ReceiptContract {
    decode,
    final_session,
    entered,
};

/// One receipt of Codex: a native `SessionStart` or `SessionEnd`. The
/// closed words are the lifecycle helper's, the same for both sources.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Told {
    Start(claude::StartSource),
    End(claude::EndReason),
}

pub(crate) fn started(source: claude::StartSource) -> Event {
    Event::Codex(Told::Start(source))
}

pub(crate) fn ended(reason: claude::EndReason) -> Event {
    Event::Codex(Told::End(reason))
}

/// One line's words as the helper writes them for Codex: one lowercase
/// UUID, closed words, no field of the other event, no final permission.
fn decode(words: &Words) -> Result<(Event, String), &'static str> {
    let native_id = words
        .native_id
        .filter(|id| claude::native_id_ok(id))
        .ok_or("lifecycle-bad-id")?;
    if words.final_permission.is_some() {
        return Err("lifecycle-bad-field");
    }
    let event = match (words.event, words.start_source, words.end_reason) {
        (Some("session-start"), Some(source), None) => {
            started(claude::start_source(source).ok_or("lifecycle-bad-field")?)
        }
        (Some("session-end"), None, Some(reason)) => {
            ended(claude::end_reason(reason).ok_or("lifecycle-bad-field")?)
        }
        _ => return Err("lifecycle-bad-field"),
    };
    Ok((event, native_id.to_owned()))
}

/// The final native session of an exited client: the receipts' end ids
/// crossed with the ids the pane printed at the exit (module header). A
/// printed id must be one the client itself reported, and the last printed
/// one is the current thread; with nothing printed, exactly one reported end
/// names it. An incomplete record, no end, a printed id the client did not
/// report, or several ends with nothing printed names nothing.
pub(crate) fn final_session(facts: &[Fact], printed: &[String]) -> Option<String> {
    if !lifecycle::complete(facts) {
        return None;
    }
    let ends: Vec<&str> = facts
        .iter()
        .filter(|fact| matches!(fact.event, Event::Codex(Told::End(_))))
        .map(|fact| fact.native_id.as_str())
        .collect();
    match (ends.as_slice(), printed.last()) {
        ([], _) => None,
        (_, Some(current)) => ends.contains(&current.as_str()).then(|| current.clone()),
        ([only], None) => Some((*only).to_string()),
        _ => None,
    }
}

/// What the launch-phase facts say about entering `wanted`: nothing yet
/// (the usual case, see the header), or exactly one client whose every
/// report is a resume of that session.
pub(crate) fn entered(facts: &[Fact], wanted: &str) -> Receipt {
    let Some(first) = facts.first() else {
        return Receipt::Pending;
    };
    let entered = lifecycle::complete(facts)
        && facts.iter().all(|fact| {
            fact.client == first.client
                && fact.native_id == wanted
                && fact.event == started(claude::StartSource::Resume)
        });
    if entered {
        Receipt::Entered(first.client)
    } else {
        Receipt::Conflict
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use claude::{EndReason, StartSource};

    const ID: &str = "01a14e8b-684f-71c3-89c7-81d71e7c7082";
    const OTHER: &str = "01a149c9-c0ae-7230-9c7d-642f3644ec16";

    #[test]
    fn a_request_is_resume_the_embedded_flag_the_grants_words_and_the_exact_id() {
        let words = |grant| request(ID, grant).unwrap();
        assert_eq!(words(Grant::Plain).words, ["resume", "--no-daemon", ID]);
        assert!(!words(Grant::Plain).lowered);
        assert_eq!(
            words(Grant::Reapply).words,
            ["resume", "--no-daemon", "--yolo", ID]
        );
        assert!(!words(Grant::Reapply).lowered);
        assert_eq!(
            words(Grant::Lower).words,
            [
                "resume",
                "--no-daemon",
                "--sandbox",
                "workspace-write",
                "--ask-for-approval",
                "on-request",
                ID
            ]
        );
        assert!(words(Grant::Lower).lowered);
        for grant in [Grant::Plain, Grant::Reapply, Grant::Lower] {
            for banned in [
                "-c",
                "-C",
                "--cd",
                "--profile",
                "-p",
                "--model",
                "--last",
                "exec",
            ] {
                assert!(
                    !words(grant).words.iter().any(|word| word == banned),
                    "{banned}"
                );
            }
        }
        for bad in ["latest", &ID.to_uppercase(), "", "01a14e8b", "../x"] {
            assert!(request(bad, Grant::Plain).is_none(), "{bad}");
        }
    }

    #[test]
    fn the_grammar_requires_the_embedded_flag_and_refuses_the_daemon_shape() {
        assert_eq!(GRAMMAR.required, [EMBEDDED]);
        assert_eq!(
            (GRAMMAR.resume_optional, GRAMMAR.bypass, GRAMMAR.resume),
            (true, "--yolo", "resume")
        );
        assert_eq!(
            (
                CAPABILITY.exit_hint,
                CAPABILITY.entry_receipt,
                CAPABILITY.source
            ),
            (Some("codex"), false, "codex")
        );
    }

    fn end(id: &str) -> Fact {
        Fact {
            event: ended(EndReason::Other),
            native_id: id.to_string(),
            client: (7, (1, 2)),
        }
    }

    fn start(id: &str, source: StartSource) -> Fact {
        Fact {
            event: started(source),
            native_id: id.to_string(),
            client: (9, (3, 4)),
        }
    }

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|id| id.to_string()).collect()
    }

    /// The final session needs the client's own receipt; the printed line
    /// only chooses among what was reported.
    #[test]
    fn the_final_session_is_the_printed_id_among_the_reported_ends() {
        // one end, nothing printed (the hint was erased, or never read)
        assert_eq!(final_session(&[end(ID)], &[]), Some(ID.to_string()));
        // one end, the same id printed
        assert_eq!(final_session(&[end(ID)], &ids(&[ID])), Some(ID.to_string()));
        // two ends (a client that switched threads): the printed one
        assert_eq!(
            final_session(&[end(OTHER), end(ID)], &ids(&[ID])),
            Some(ID.to_string())
        );
        assert_eq!(
            final_session(&[end(ID), end(OTHER)], &ids(&[ID])),
            Some(ID.to_string())
        );
        // the last printed decides when several were printed
        assert_eq!(
            final_session(&[end(ID), end(OTHER)], &ids(&[ID, OTHER])),
            Some(OTHER.to_string())
        );
        // two ends and nothing printed: nobody knows which is current
        assert_eq!(final_session(&[end(ID), end(OTHER)], &[]), None);
        // a printed id the client never reported is somebody else's text
        assert_eq!(final_session(&[end(ID)], &ids(&[OTHER])), None);
        assert_eq!(final_session(&[], &ids(&[ID])), None);
        assert_eq!(final_session(&[], &[]), None);
        // a start is not an end; an incomplete record names nothing
        assert_eq!(
            final_session(&[start(ID, StartSource::Startup)], &ids(&[ID])),
            None
        );
        let mut overflowed = vec![end(ID)];
        overflowed.push(Fact {
            event: Event::Overflow,
            native_id: ID.to_string(),
            client: (7, (1, 2)),
        });
        assert_eq!(final_session(&overflowed, &ids(&[ID])), None);
    }

    #[test]
    fn an_entry_is_pending_without_a_receipt_and_confirmed_only_by_a_resume_of_the_wanted_id() {
        assert_eq!(entered(&[], ID), Receipt::Pending);
        assert_eq!(
            entered(&[start(ID, StartSource::Resume)], ID),
            Receipt::Entered((9, (3, 4)))
        );
        assert_eq!(
            entered(&[start(OTHER, StartSource::Resume)], ID),
            Receipt::Conflict
        );
        assert_eq!(
            entered(&[start(ID, StartSource::Startup)], ID),
            Receipt::Conflict
        );
        assert_eq!(
            entered(&[start(ID, StartSource::Resume), end(ID)], ID),
            Receipt::Conflict
        );
    }

    #[test]
    fn decode_reads_the_helpers_closed_words_and_no_final_permission() {
        let words = |event, start, end| Words {
            event: Some(event),
            native_id: Some(ID),
            start_source: start,
            end_reason: end,
            final_permission: None,
        };
        assert_eq!(
            decode(&words("session-start", Some("resume"), None)),
            Ok((started(StartSource::Resume), ID.to_string()))
        );
        assert_eq!(
            decode(&words("session-end", None, Some("other"))),
            Ok((ended(EndReason::Other), ID.to_string()))
        );
        assert_eq!(
            decode(&words("session-end", Some("resume"), Some("other"))),
            Err("lifecycle-bad-field")
        );
        assert_eq!(
            decode(&words("session-start", Some("fork"), None)),
            Err("lifecycle-bad-field")
        );
        let mut with_permission = words("session-end", None, Some("other"));
        with_permission.final_permission = Some("unknown");
        assert_eq!(decode(&with_permission), Err("lifecycle-bad-field"));
        let mut bad_id = words("session-end", None, Some("other"));
        bad_id.native_id = Some("latest");
        assert_eq!(decode(&bad_id), Err("lifecycle-bad-id"));
    }
}
