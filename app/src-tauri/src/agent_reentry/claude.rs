//! Claude Code's re-entry knowledge: the compatibility facts (rows of
//! version and digest; what each row is backed by is stated below), the
//! native session id shape, the supported launch grammar, the closed
//! lifecycle words and the PURE construction of an exact-id request.
//!
//! # Contract
//! Everything here is static data or a pure function. This module holds no
//! pane, PTY, queue, Board or authority handle; it never decides that an
//! entry may launch and never chooses the bypass override — the core hands
//! it that decision (`agent_reentry::decide`) and gets argument words back.
//! A request is rebuilt from structured fields only: `--resume <exact id>`,
//! preceded by the bypass flag when the core says so. No model, directory,
//! prompt, `--continue`, `--fork-session`, permission-mode or environment
//! word can be produced, and no observed argv is ever replayed.
//!
//! Compatibility facts are a short list of (version, SHA-256) pairs. What
//! each row is backed by is three different things and must not be merged,
//! and the native evidence of one row is never the evidence of another:
//! - **Native evidence for 2.1.294**, four pieces that must not be merged
//!   with each other either:
//!   - a probe carrier, not Deck (`reports/agent-reentry-native-probe/
//!     batch-294-20261009`): the exit receipt chain, an exact-id resume
//!     WITHOUT any override, and that a resume without an override does not
//!     keep bypass;
//!   - stage S (`reports/agent-reentry-implementation/followup-12.md`):
//!     native launch metadata was readable and matched the production
//!     checks (`procinfo::qualify_client`, the preview classification,
//!     `procinfo::check_launch`) for that binary under a normal HOME, in a
//!     bare launch after the user confirmed the session view. User shell
//!     startup files were isolated;
//!   - stage B (`reports/agent-reentry-implementation/followup-13.md`): the
//!     same for a launch with only the bypass flag, which read as the
//!     `Bypass` launch profile and matched. That is how the client was
//!     started, not its effective permission: the user read the native
//!     bypass indicator once, and that reading is not Deck's observation;
//!   - stage M1 (`reports/agent-reentry-implementation/followup-17-m1.md`):
//!     ONE restart transaction under Policy A, run once. A test controller
//!     (`tests.rs`, `native_m1_transact`) called the production preview and
//!     the production restart transaction, with the real operation and
//!     activity gates, over two such clients under a normal HOME: one
//!     started bare that had switched session once, one started with the
//!     bypass flag. Both entries ended `Confirmed`. Each request was the
//!     frozen executable, `--resume` and one id — the session that client
//!     exited in, for the first one not the session it started in — and
//!     neither carried the bypass flag. Deck's helper ran from project-level
//!     hooks as the client's direct child, and the exit and entry receipts
//!     were attributed by `lifecycle::receive`.
//! - **Native evidence for 2.1.296**
//!   (`reports/agent-reentry-implementation/followup-22.md`), every stage
//!   run once by the same test controller, which was handed a capability
//!   for exactly this digest; the row entered the table below only after
//!   all of them:
//!   - launch metadata: six clients started outside Deck, bare or with only
//!     the bypass flag, read as `Ordinary` / `Bypass` and matched the
//!     production checks after the start and at the session stage — one of
//!     them again after it had been lowered to plan mode while running: a
//!     launch profile is how a client was started, whatever it shows later;
//!   - stage M1: ONE Policy A transaction over two clients (one switched
//!     session with `/clear`, one started with the bypass flag). Both
//!     `Confirmed`; each request the frozen executable, `--resume` and the
//!     session that client exited in; neither carried the bypass flag;
//!   - stage M2: ONE Policy B transaction over three clients — started with
//!     the bypass flag and left so; started with it and lowered to plan
//!     mode while running; started bare. All three `Confirmed`. The two
//!     bypass launches were asked with `--dangerously-skip-permissions
//!     --resume <id>`, in that order, and the started clients' launch images
//!     matched it; the bare launch was asked without the flag. Policy B is
//!     a REAUTHORIZATION, not a kept state: the client that showed plan
//!     mode before the restart showed bypass after it. What the panes
//!     showed afterwards (bypass, bypass, and the bare launch's default
//!     mode) was read by the experiment's operator; it is not Deck's
//!     observation and gates nothing;
//!   - stage M3: ONE transaction in which the new shell's startup file
//!     exported a refused name. One request was typed; the entry ended
//!     `Unconfirmed`, `context-changed`; the started client was left
//!     running.
//!
//!   The experiment's servers of the 2.1.296 stages handed their panes
//!   five variables of the user's login session and the client's own switch
//!   against update checks beside what the 2.1.294 stages had. That is a
//!   fact about the experiment's environment (its servers start under
//!   `env -i`), not a configuration this feature requires: with that group
//!   added the client's official status command saw the user's login.
//!   Which of the five is needed was not isolated, and the observation
//!   explains nothing about any run of 2.1.294.
//!
//!   Stages S and B are discrete read-only samples of a client started
//!   outside Deck; neither is a transaction. Every transaction above ran
//!   once, not repeated, and what ran it was this feature's code inside a
//!   test binary, not the application.
//! - **Local isolated application evidence for 2.1.296**
//!   (`reports/agent-reentry-implementation/followup-33.md`): real Deck UI,
//!   IPC and production assembly completed ONE successful Policy A re-entry
//!   after a separate debug carrier attempt ended without launching a
//!   replacement. The successful carrier retained debug assertions, optimized
//!   only its SHA-256 dependency, and used an explicitly validated project
//!   lifecycle configuration, real HOME, private data/socket and its own
//!   ad-hoc signed bundled helper. No controller or scripted answer drove the
//!   transaction. This is not release installation or global hook migration.
//! - **Fake integration** (`agent_reentry/tests.rs`, a stand-in client):
//!   Deck's own helper, lease attribution, launch sink and both request
//!   shapes, including `--dangerously-skip-permissions --resume <id>`.
//! - **Not certified yet**:
//!   - on 2.1.294: the request WITH the override and any Policy B
//!     transaction, and a refused environment name — those ran on 2.1.296
//!     only;
//!   - on either release: a launch image that cannot be read (such an entry
//!     ends `Unconfirmed`, `context-unverified`, never confirmed; no natural
//!     way to produce it was found);
//!   - formal release installation and its global user-level hook opt-in and
//!     migration; the local debug evidence above uses a project-only entry;
//!   - the effective permission of a re-entered session as something Deck
//!     knows: Deck does not read it;
//!   - Developer ID, notarization, Gatekeeper and the published carrier.
//!
//!   The WKWebView `reentry` smoke paints the restart dialog from scripted
//!   answers. It has run three times, as an ad-hoc signed debug carrier on
//!   the test machine. The first two runs FAILED its layout checkpoint
//!   (`followup-17-wk.md`, `followup-19.md`): the dialog's checkboxes and
//!   radios took the stylesheet's general `input` width and the section
//!   overflowed sideways. With that one rule repaired the third run PASSED
//!   all 15 checkpoints, in English and Chinese, at the default 1280×800
//!   window (`followup-20.md`). That is one passing run of the dialog's
//!   presentation: no restart, no client, no other window size or font
//!   scale, and not the signed carrier.
//!
//! So a row means "this exact binary has the native evidence listed for it
//! above", not more. Adding a release is adding one row after its own
//! native evidence, never a version range and never a lookup through
//! `PATH`. The final permission mode is NOT observable on 2.1.294
//! (`SessionEnd` carries no such field; not examined on 2.1.296); that
//! absence gates nothing.

use super::{CertifiedFact, ReentryCapability};
use crate::procinfo::LaunchGrammar;

/// Releases this feature is written against, each with its own native
/// evidence (see the module header for what that evidence is per row, what
/// is fake integration and what is not certified yet). Two rows today.
const CERTIFIED: &[CertifiedFact] = &[
    CertifiedFact {
        version: "2.1.294",
        sha256: "def0d15e64dd7d89621f88d28214f885b1c38b0ddd69762fb8593e34915d6d53",
    },
    CertifiedFact {
        version: "2.1.296",
        sha256: "c9b5341637becbd423ddffc5b254afb645682a3868cb708bbc6cc0e7bb419937",
    },
];

const BYPASS: &str = "--dangerously-skip-permissions";
const RESUME: &str = "--resume";

/// The launch shapes a direct native client is supported in. Anything else —
/// a wrapper, an unknown flag, a positional prompt, `--continue`,
/// `--allow-dangerously-skip-permissions`, `--permission-mode
/// bypassPermissions`, another config root, a provider or credential
/// override — classifies as unsupported and is never re-entered
/// automatically.
const GRAMMAR: LaunchGrammar = LaunchGrammar {
    bypass: BYPASS,
    resume: RESUME,
    resume_value: native_id_ok,
    mode: "--permission-mode",
    ordinary_modes: &["default", "plan", "acceptEdits"],
    dropped: &["--model"],
    directories: "--add-dir",
    refused_env: &["CLAUDE_CONFIG_DIR"],
    refused_env_prefixes: &["ANTHROPIC_", "CLAUDE_CODE_USE_", "AWS_BEARER_TOKEN_"],
};

/// `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`, lowercase hex: the one shape a
/// native session id may take anywhere in this feature.
pub(crate) fn native_id_ok(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_digit() || (b'a'..=b'f').contains(&b),
        })
}

/// The argument words of an exact-id request, in the one order this build
/// produces. Without the override, that request ran against both real
/// releases (stage M1 of each, module header). With it, it ran once against
/// 2.1.296 (stage M2); on 2.1.294 that order is exercised by the fake
/// integration only. `None` for anything that is not a native session id.
fn request(native_id: &str, bypass: bool) -> Option<Vec<String>> {
    if !native_id_ok(native_id) {
        return None;
    }
    let mut words = Vec::with_capacity(3);
    if bypass {
        words.push(BYPASS.to_string());
    }
    words.push(RESUME.to_string());
    words.push(native_id.to_string());
    Some(words)
}

pub(crate) const CAPABILITY: ReentryCapability = ReentryCapability {
    facts: CERTIFIED,
    launcher: "claude",
    grammar: &GRAMMAR,
    login_shell: "zsh",
    request,
};

/// Native `SessionStart` sources as the lifecycle wire spells them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StartSource {
    Startup,
    Resume,
    Clear,
    Compact,
    /// Any source this build does not name: it confirms nothing.
    Other,
}

/// Native `SessionEnd` reasons as the lifecycle wire spells them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EndReason {
    Clear,
    Resume,
    Logout,
    /// The client left at its input prompt: the only reason that can name
    /// the final session of an exiting client.
    PromptInputExit,
    /// Any reason this build does not name: it finalizes nothing.
    Other,
}

pub(crate) fn start_source(word: &str) -> Option<StartSource> {
    Some(match word {
        "startup" => StartSource::Startup,
        "resume" => StartSource::Resume,
        "clear" => StartSource::Clear,
        "compact" => StartSource::Compact,
        "other" => StartSource::Other,
        _ => return None,
    })
}

pub(crate) fn end_reason(word: &str) -> Option<EndReason> {
    Some(match word {
        "clear" => EndReason::Clear,
        "resume" => EndReason::Resume,
        "logout" => EndReason::Logout,
        "prompt-input-exit" => EndReason::PromptInputExit,
        "other" => EndReason::Other,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "731c5908-251a-4388-b3a3-4719ad442bce";

    #[test]
    fn a_request_is_only_the_exact_id_and_the_optional_bypass_flag() {
        assert_eq!(request(ID, false).unwrap(), ["--resume", ID]);
        assert_eq!(
            request(ID, true).unwrap(),
            ["--dangerously-skip-permissions", "--resume", ID]
        );
        for words in [request(ID, false).unwrap(), request(ID, true).unwrap()] {
            for banned in [
                "--permission-mode",
                "default",
                "--model",
                "--add-dir",
                "--continue",
                "--fork-session",
                "-p",
            ] {
                assert!(!words.iter().any(|word| word == banned), "{banned}");
            }
        }
    }

    #[test]
    fn only_a_lowercase_native_uuid_can_be_requested() {
        for bad in [
            "",
            "731C5908-251a-4388-b3a3-4719ad442bce",
            "731c5908-251a-4388-b3a3-4719ad442bc",
            "731c5908-251a-4388-b3a3-4719ad442bce ",
            "731c5908-251a-4388-b3a3-4719ad442bce; rm -rf",
            "--dangerously-skip-permissions",
            "latest",
        ] {
            assert!(!native_id_ok(bad), "{bad:?}");
            assert!(request(bad, true).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn the_certified_facts_are_the_two_releases_with_native_evidence() {
        assert_eq!(CERTIFIED.len(), 2);
        assert_eq!(CERTIFIED[0].version, "2.1.294");
        assert_eq!(CERTIFIED[1].version, "2.1.296");
        assert!(CERTIFIED.iter().all(|fact| fact.sha256.len() == 64));
        assert_ne!(CERTIFIED[0].sha256, CERTIFIED[1].sha256);
        // a digest names its own release and no other
        for (digest, version) in [
            (
                "def0d15e64dd7d89621f88d28214f885b1c38b0ddd69762fb8593e34915d6d53",
                "2.1.294",
            ),
            (
                "c9b5341637becbd423ddffc5b254afb645682a3868cb708bbc6cc0e7bb419937",
                "2.1.296",
            ),
        ] {
            assert_eq!(
                CAPABILITY.certified(digest).map(|fact| fact.version),
                Some(version)
            );
        }
        assert!(CAPABILITY
            .certified("def0d15e64dd7d89621f88d28214f885b1c38b0ddd69762fb8593e34915d6d53")
            .is_some());
        // same version, another digest: not certified
        assert!(CAPABILITY.certified(&"0".repeat(64)).is_none());
        assert!(CAPABILITY.certified("2.1.294").is_none());
        assert!(CAPABILITY.certified("2.1.296").is_none());
    }

    #[test]
    fn unknown_lifecycle_words_are_not_facts() {
        assert_eq!(start_source("resume"), Some(StartSource::Resume));
        assert_eq!(start_source("fork"), None);
        assert_eq!(
            end_reason("prompt-input-exit"),
            Some(EndReason::PromptInputExit)
        );
        assert_eq!(end_reason("prompt_input_exit"), None);
        assert_eq!(end_reason("exit"), None);
    }
}
