//! Claude Code's re-entry knowledge: whose code a client must be, the native
//! session id shape, the supported launch grammar, the closed lifecycle
//! words with what a record of them means (`RECEIPTS`) and the PURE
//! construction of an exact-id request.
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
//! # Compatibility: an identity and a narrow contract, not a list of releases
//! Nothing here names a release. A client is admitted by FIVE separate
//! facts, none of which stands in for another:
//! - **Executable identity** — `PRODUCT`: Claude Code as Anthropic signs it,
//!   validated by Security.framework against the running image and its file
//!   (`procinfo::qualify_client`). The file's SHA-256 is this transaction's
//!   proof that the content stayed the same; it is compared only with
//!   itself and is never looked up. A release this build has never seen is
//!   the product all the same. A signature, a file name, a version, a path,
//!   a lifecycle event or the user's approval alone is none of this.
//! - **Native protocol** — the narrow part of the public CLI and hook
//!   contract this feature uses: `--resume <id>`, the bypass flag before
//!   it, `SessionStart` (source) and `SessionEnd` (reason) with a session
//!   id, hooks run as the client's direct child. What is checked is what
//!   THIS transaction observes of it: the old client's own launch image
//!   (`GRAMMAR`), its applicable end receipt, and the new client's resume
//!   receipt and launch image. A release that changed any of that shows it
//!   there and is not launched, or is launched once and not confirmed; a
//!   release that kept the same shapes and changed their meaning cannot be
//!   told apart by any interface this feature may use. The user accepted
//!   that residual; it is not a reason to list releases again.
//! - **Launch profile** — how the old client was started (`LaunchProfile`).
//! - **User authorization** — this restart's ticked cards and policy.
//! - **Confirmation** — the consumed slot, the resume receipt of the new
//!   foreground client and the post-launch check of its image and request.
//!
//! Policy B needs nothing more than Policy A beyond a `Bypass` launch
//! profile that was previewed and read again at freeze. There is no
//! per-release list for the override either, and the final permission mode
//! gates nothing (`SessionEnd` on 2.1.294 carries no such field; it was not
//! examined on 2.1.296).
//!
//! # Historical native evidence (what was run, never a runtime gate)
//! The releases below were exercised natively while this feature was
//! written. The record says what was shown and what was not; it admits and
//! refuses nothing at runtime, and adding a release is not maintenance this
//! module needs. The `reports/…` and `followup-…` names below are those
//! rounds' working records: they were never part of the repository
//! (`reports/` is git-ignored) and a checkout does not have them. What a
//! checkout can read is the acceptance summary,
//! `docs/evidence/closeout-20261011/reentry-acceptance.md`.
//! The pieces must not be merged with each other:
//! - **2.1.294**, four pieces:
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
//! - **2.1.296** (`reports/agent-reentry-implementation/followup-22.md`),
//!   every stage run once by the same test controller:
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
//!   test binary, not the application. All of them ran while a client was
//!   still admitted by a list of release digests: they are evidence about
//!   the native contract and the transaction, not about the identity
//!   checks that replaced that list.
//! - **Local isolated application evidence for 2.1.296**
//!   (`reports/agent-reentry-implementation/followup-33.md`): real Deck UI,
//!   IPC and production assembly completed ONE successful Policy A re-entry
//!   after a separate debug carrier attempt ended without launching a
//!   replacement. The successful carrier retained debug assertions, optimized
//!   only its SHA-256 dependency, and used an explicitly validated project
//!   lifecycle configuration, real HOME, private data/socket and its own
//!   ad-hoc signed bundled helper. No controller or scripted answer drove the
//!   transaction. This is not release installation or global hook migration,
//!   and it too ran under the list of release digests.
//! - **The identity checks** (`procinfo`, `procinfo/code_identity.rs`):
//!   their mechanism is proven by tests against real processes and ad-hoc
//!   signed fixtures; that `PRODUCT` accepts Anthropic's real signature and
//!   refuses other signers is the explicit, read-only test in this file
//!   (`the_production_requirement_on_named_real_executables_and_processes`:
//!   run against three installed releases — one of them never listed — two
//!   other Developer ID programs and one running client). No restart
//!   transaction with a real client has run on them.
//! - **Fake integration** (`agent_reentry/tests.rs`, a stand-in client
//!   signed ad hoc as a test product): Deck's own helper, lease
//!   attribution, launch sink and both request shapes, including
//!   `--dangerously-skip-permissions --resume <id>`, for a build no list
//!   names.
//! - **Not shown natively**:
//!   - on 2.1.294: the request WITH the override and any Policy B
//!     transaction, and a refused environment name — those ran on 2.1.296
//!     only. Under this module's contract that absence refuses nothing;
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

use super::lifecycle::{self, Event, Fact, Receipt, ReceiptContract, Words};
use super::ReentryCapability;
use crate::procinfo::{LaunchGrammar, ProductIdentity};

/// Whose code a client must be: Claude Code as Anthropic signs it. This is
/// the designated requirement the product's own signature states — the
/// signing identifier, a chain to Apple's root through the Developer ID
/// authority, a Developer ID Application leaf, and Anthropic's Team. None of
/// it names a release, a digest or a leaf certificate, so a new release and
/// a renewed certificate satisfy it unchanged; another signer, another
/// product of the same Team, an ad-hoc or unsigned file do not. It is
/// evaluated by Security.framework against the code itself
/// (`procinfo::qualify_client`), never compared as text with a field read
/// from a file.
const PRODUCT: ProductIdentity = ProductIdentity {
    requirement: "identifier \"com.anthropic.claude-code\" and anchor apple generic \
        and certificate 1[field.1.2.840.113635.100.6.2.6] exists \
        and certificate leaf[field.1.2.840.113635.100.6.1.13] exists \
        and certificate leaf[subject.OU] = \"Q6L2SF6YDW\"",
};

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
/// produces: the override, when the core granted it, before `--resume`.
/// (What ran natively in that order is the module header's record.) `None`
/// for anything that is not a native session id.
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
    product: &PRODUCT,
    launcher: "claude",
    grammar: &GRAMMAR,
    login_shell: "zsh",
    request,
    receipts: &RECEIPTS,
};

/// What Claude Code's lifecycle receipts mean (`lifecycle::ReceiptContract`):
/// the decoding of its words and its id, and the two reducers over a
/// lease's record. `lifecycle.rs` frames, attributes, bounds and keeps the
/// record; what a fact says is read here and nowhere else.
pub(crate) const RECEIPTS: ReceiptContract = ReceiptContract {
    decode,
    final_session,
    entered,
};

/// One receipt of Claude Code: a native `SessionStart` or `SessionEnd`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Told {
    Start(StartSource),
    End(EndReason),
}

/// The fact a native `SessionStart` of this source is.
pub(crate) fn started(source: StartSource) -> Event {
    Event::Claude(Told::Start(source))
}

/// The fact a native `SessionEnd` of this source is.
pub(crate) fn ended(reason: EndReason) -> Event {
    Event::Claude(Told::End(reason))
}

/// One line's words as Claude Code writes them: one valid lowercase UUID,
/// closed words, no field of the other event. The optional final permission
/// is a closed diagnostic word: validated, then dropped — nothing in this
/// build reads it.
fn decode(words: &Words) -> Result<(Event, String), &'static str> {
    let native_id = words
        .native_id
        .filter(|id| native_id_ok(id))
        .ok_or("lifecycle-bad-id")?;
    if words
        .final_permission
        .is_some_and(|word| !matches!(word, "bypass" | "non-bypass" | "unknown"))
    {
        return Err("lifecycle-bad-field");
    }
    let event = match (words.event, words.start_source, words.end_reason) {
        (Some("session-start"), Some(source), None) if words.final_permission.is_none() => {
            started(start_source(source).ok_or("lifecycle-bad-field")?)
        }
        (Some("session-end"), None, Some(reason)) => {
            ended(end_reason(reason).ok_or("lifecycle-bad-field")?)
        }
        _ => return Err("lifecycle-bad-field"),
    };
    Ok((event, native_id.to_owned()))
}

/// The final native session of an exited client: the id of its applicable
/// end (`prompt-input-exit`), which must be the LAST fact and the only such
/// id. `clear`, `resume`, `logout` and `other` ends switch or name nothing;
/// two different final ids never resolve to the later one, and an
/// incomplete record names nothing.
pub(crate) fn final_session(facts: &[Fact]) -> Option<String> {
    let exit = ended(EndReason::PromptInputExit);
    let last = facts.last()?;
    if !lifecycle::complete(facts) || last.event != exit {
        return None;
    }
    facts
        .iter()
        .filter(|fact| fact.event == exit)
        .all(|fact| fact.native_id == last.native_id)
        .then(|| last.native_id.clone())
}

/// What the launch-phase facts say about entering `wanted`: exactly one
/// client reported, and all it reported is entering that session through a
/// resume.
pub(crate) fn entered(facts: &[Fact], wanted: &str) -> Receipt {
    let Some(first) = facts.first() else {
        return Receipt::Pending;
    };
    let entered = lifecycle::complete(facts)
        && facts.iter().all(|fact| {
            fact.client == first.client
                && fact.native_id == wanted
                && fact.event == started(StartSource::Resume)
        });
    if entered {
        Receipt::Entered(first.client)
    } else {
        Receipt::Conflict
    }
}

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

    /// The releases that were exercised natively while this feature was
    /// written (module header), as version label and file digest. A label
    /// for tests and reports: production code holds no such list.
    const HISTORICAL: &[(&str, &str)] = &[
        (
            "2.1.294",
            "def0d15e64dd7d89621f88d28214f885b1c38b0ddd69762fb8593e34915d6d53",
        ),
        (
            "2.1.296",
            "c9b5341637becbd423ddffc5b254afb645682a3868cb708bbc6cc0e7bb419937",
        ),
    ];

    /// The production half of a source file: everything before its trailing
    /// test module.
    fn production(source: &'static str) -> &'static str {
        source
            .split("#[cfg(test)]\nmod tests")
            .next()
            .expect("a source file")
    }

    /// No release is named at runtime: the capability has a product and no
    /// list, and no production source of this feature holds a historical
    /// digest, a version label as data, or a lookup of a file digest. Putting
    /// a "known digest" condition back fails here before it fails the
    /// transaction tests for a build no list names.
    #[test]
    fn no_release_is_named_at_runtime() {
        let ReentryCapability {
            product: _,
            launcher: _,
            grammar: _,
            login_shell: _,
            request: _,
            receipts: _,
        } = CAPABILITY;
        let sources = [
            ("claude.rs", production(include_str!("claude.rs"))),
            ("mod.rs", production(include_str!("mod.rs"))),
            ("launch.rs", production(include_str!("launch.rs"))),
            ("lifecycle.rs", production(include_str!("lifecycle.rs"))),
            ("procinfo.rs", production(include_str!("../procinfo.rs"))),
            (
                "code_identity.rs",
                production(include_str!("../procinfo/code_identity.rs")),
            ),
        ];
        for (name, source) in sources {
            let code: String = source
                .lines()
                .filter(|line| !line.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n");
            for (version, digest) in HISTORICAL {
                assert!(!code.contains(digest), "{name} holds a release digest");
                assert!(
                    !code.contains(&format!("\"{version}\"")),
                    "{name} holds a release label as data"
                );
            }
            // a digest is compared with itself (`unchanged`), never looked up
            for lookup in [
                "digest_hex()",
                "certified(",
                "CertifiedFact",
                "image_names(",
            ] {
                assert!(!code.contains(lookup), "{name} must not contain {lookup}");
            }
        }
        // the one definition of the digest's hex form is test-only
        let procinfo = include_str!("../procinfo.rs");
        let at = procinfo
            .find("pub(crate) fn digest_hex(&self)")
            .expect("the hex form");
        assert!(procinfo[..at].trim_end().ends_with("#[cfg(test)]"));
        assert_eq!(HISTORICAL.len(), 2);
        assert!(HISTORICAL.iter().all(|(_, digest)| digest.len() == 64));
    }

    use crate::procinfo::{
        identity_fixture as fixture, ClientRead, ClientSpec, ExecutableRef, NotProduct,
        ProductIdentity, Unavailable,
    };
    use std::path::Path;

    /// The same requirement with one clause changed: what another signer's
    /// or another product's requirement would be.
    fn variant(from: &str, to: &str) -> &'static ProductIdentity {
        assert!(PRODUCT.requirement.contains(from), "{from}");
        Box::leak(Box::new(ProductIdentity {
            requirement: Box::leak(PRODUCT.requirement.replace(from, to).into_boxed_str()),
        }))
    }

    /// What the production requirement is: the product, its signer and the
    /// chain to Apple — and no release, digest or code directory.
    #[test]
    fn the_product_requirement_names_the_product_its_signer_and_no_release() {
        let text = PRODUCT.requirement;
        for clause in [
            "identifier \"com.anthropic.claude-code\"",
            "anchor apple generic",
            "certificate 1[field.1.2.840.113635.100.6.2.6] exists",
            "certificate leaf[field.1.2.840.113635.100.6.1.13] exists",
            "certificate leaf[subject.OU] = \"Q6L2SF6YDW\"",
        ] {
            assert!(text.contains(clause), "{clause}");
        }
        assert_eq!(
            text.matches(" and ").count(),
            4,
            "five clauses, all required"
        );
        for release in ["cdhash", "2.1.", " or ", "info["] {
            assert!(!text.contains(release), "{release}");
        }
        assert!(HISTORICAL.iter().all(|(_, digest)| !text.contains(digest)));
    }

    /// Through the production functions, with no real client: nothing that
    /// Anthropic did not sign as Claude Code is the product — a file and a
    /// running process that CLAIM its signing identifier and carry its
    /// launcher's name included. (Ad-hoc fixtures and system programs; that
    /// the requirement accepts Anthropic's real signature is the explicit
    /// test below.)
    #[test]
    fn the_production_requirement_refuses_everything_anthropic_did_not_sign() {
        // these checks ask for a deadline owner; the deadline is not the subject
        let _phase = crate::session_runtime::test_phase();
        let dir = fixture::directory("production");
        let claimed = dir.join("claude");
        fixture::build(&claimed, "build-A", &[], "com.anthropic.claude-code");
        // the claim is really there, and a requirement of that identifier alone takes it
        let by_name = fixture::product("com.anthropic.claude-code");
        assert!(ExecutableRef::read(&claimed, by_name).is_ok());
        let refused = |path: &Path| ExecutableRef::read(path, &PRODUCT).err();
        assert_eq!(refused(&claimed), Some(Unavailable::Changed));
        // Apple's own signature: a valid chain to Apple's root, not this signer or product
        assert_eq!(refused(Path::new("/bin/ls")), Some(Unavailable::Changed));
        let unsigned = dir.join("unsigned");
        std::fs::copy(&claimed, &unsigned).unwrap();
        fixture::unsign(&unsigned);
        assert_eq!(refused(&unsigned), Some(Unavailable::Changed));
        // the running claim: named like the launcher, not the product
        let (process, _) = fixture::Process::start(&claimed, None).expect("the fixture runs");
        let birth = crate::procinfo::process_start(process.pid()).unwrap();
        let read = crate::procinfo::qualify_client(
            process.pid(),
            birth,
            &ClientSpec {
                product: &PRODUCT,
                launcher: CAPABILITY.launcher,
                grammar: &GRAMMAR,
                home: Path::new("/nonexistent"),
            },
            &[],
        );
        // named like the launcher — which is said, and is not what refused it
        assert!(
            matches!(
                read,
                ClientRead::NotProduct {
                    named: true,
                    why: NotProduct::Other
                }
            ),
            "{read:?}"
        );
        drop(process);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The production requirement against REAL material the caller names,
    /// read-only: files that must be the product (`DECK_REENTRY_PRODUCT_ACCEPT`,
    /// `:`-separated absolute paths), files that must not be
    /// (`DECK_REENTRY_PRODUCT_REJECT`), and one already running client
    /// (`DECK_REENTRY_PROCESS_PROBE`, a pid) that the production
    /// qualification must take for the product and bind to its file. It
    /// starts nothing, sends nothing and prints closed words, counts and
    /// milliseconds — never a path, an argument or a digest.
    #[test]
    #[ignore = "requires explicitly named real executables or processes; starts no client"]
    fn the_production_requirement_on_named_real_executables_and_processes() {
        // these checks ask for a deadline owner; the deadline is not the subject
        let _phase = crate::session_runtime::test_phase();
        let listed = |name: &str| -> Vec<std::path::PathBuf> {
            std::env::var(name)
                .map(|list| list.split(':').map(std::path::PathBuf::from).collect())
                .unwrap_or_default()
        };
        let other_team = variant("\"Q6L2SF6YDW\"", "\"AAAAAAAAAA\"");
        let other_product = variant("com.anthropic.claude-code", "com.anthropic.other-product");
        let mut failed = 0;
        for (index, path) in listed("DECK_REENTRY_PRODUCT_ACCEPT").iter().enumerate() {
            let started = std::time::Instant::now();
            let made = ExecutableRef::read(path, &PRODUCT);
            let reference_ms = started.elapsed().as_millis();
            let started = std::time::Instant::now();
            let reread = made.as_ref().ok().map(ExecutableRef::unchanged);
            let reread_ms = started.elapsed().as_millis();
            let historical = made.as_ref().is_ok_and(|made| {
                HISTORICAL
                    .iter()
                    .any(|(_, digest)| *digest == made.digest_hex())
            });
            let clauses = (
                ExecutableRef::read(path, other_team).err(),
                ExecutableRef::read(path, other_product).err(),
            );
            let ok = made.is_ok()
                && reread == Some(Ok(()))
                && clauses == (Some(Unavailable::Changed), Some(Unavailable::Changed));
            failed += usize::from(!ok);
            println!(
                "ACCEPT[{index}] product={} in_historical_table={historical} reference_ms={reference_ms} reread={reread:?} reread_ms={reread_ms} other_team={:?} other_product={:?}",
                made.as_ref().map(|_| "yes").unwrap_or("NO"),
                clauses.0,
                clauses.1,
            );
        }
        for (index, path) in listed("DECK_REENTRY_PRODUCT_REJECT").iter().enumerate() {
            let made = ExecutableRef::read(path, &PRODUCT);
            // it is somebody's valid code: only then is the refusal about WHOSE
            let own = ExecutableRef::read(path, fixture::product_of(path)).is_ok();
            failed += usize::from(made.as_ref().err() != Some(&Unavailable::Changed) || !own);
            println!(
                "REJECT[{index}] product={} valid_as_its_own={own}",
                match &made {
                    Ok(_) => "YES".to_string(),
                    Err(why) => format!("no/{why:?}"),
                }
            );
        }
        if let Some(pid) = std::env::var("DECK_REENTRY_PROCESS_PROBE")
            .ok()
            .and_then(|pid| pid.parse::<u32>().ok())
        {
            let home = std::path::PathBuf::from(
                std::env::var_os("DECK_REENTRY_PROCESS_HOME").unwrap_or_default(),
            );
            let birth = crate::procinfo::process_start(pid).expect("the named process exists");
            let spec = |product| ClientSpec {
                product,
                launcher: CAPABILITY.launcher,
                grammar: &GRAMMAR,
                home: &home,
            };
            let started = std::time::Instant::now();
            let read = crate::procinfo::qualify_client(pid, birth, &spec(&PRODUCT), &[]);
            let qualify_ms = started.elapsed().as_millis();
            let bound = match &read {
                ClientRead::Product(read) => read.executable.as_ref().ok().cloned(),
                _ => None,
            };
            let rebinding = bound.as_ref().map(|executable| {
                let started = std::time::Instant::now();
                let answers = (0..20)
                    .filter(|_| crate::procinfo::runs_executable(pid, birth, executable))
                    .count();
                (answers, started.elapsed().as_micros() / 20)
            });
            let not_it = |product| {
                matches!(
                    crate::procinfo::qualify_client(pid, birth, &spec(product), &[]),
                    ClientRead::NotProduct {
                        why: NotProduct::Other,
                        ..
                    }
                )
            };
            let others = (not_it(other_team), not_it(other_product));
            failed += usize::from(
                rebinding.map(|(bound, _)| bound) != Some(20) || others != (true, true),
            );
            println!(
                "PROCESS qualify={} qualify_ms={qualify_ms} rebinding_bound_of_20={:?} rebinding_mean_us={:?} other_team_refused={} other_product_refused={}",
                match &read {
                    ClientRead::Gone => "gone".to_string(),
                    ClientRead::NotProduct { why, .. } => format!("not-product/{why:?}"),
                    ClientRead::Product(read) => format!(
                        "product executable={} profile={:?}",
                        match &read.executable {
                            Ok(_) => "bound".to_string(),
                            Err(why) => format!("{why:?}"),
                        },
                        read.profile
                    ),
                },
                rebinding.map(|(bound, _)| bound),
                rebinding.map(|(_, mean)| mean),
                others.0,
                others.1,
            );
        }
        assert_eq!(
            failed, 0,
            "every named file and process answered as required"
        );
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
