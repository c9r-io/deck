//! Reopen Agent session: re-entering the exact native Claude session a
//! client was in when the user's own shell-service restart ended it.
//!
//! Deck owns attention around work; the Agent owns the work. This module
//! coordinates one thing — user restart → the managed old client ends → the
//! server is replaced → the SAME native session is entered once in the new
//! shell generation. It sends no prompt, no "continue" and no task
//! instruction, moves or retires no card, and a confirmed re-entry means
//! only "this client entered that session at the receipt": never task
//! resumed, input ready, tool authority or task success.
//!
//! # Contract
//! - **Transaction memory only.** Preview, frozen plan, native ids, launch
//!   slots and results live in this process and die with it. Nothing here is
//!   written to disk: no registry, recovery queue or replay journal, and the
//!   lifecycle file (`tmux_lifecycle::RestartIntent`) carries no Agent data.
//!   After a crash nothing re-enters; the user's manual resume hint remains.
//! - **States.** Preview → Frozen → Exiting → Prepared → Replacing →
//!   ShellsReopening → Launching → Confirming → Complete. A preview issues a
//!   single-use opaque token, and only for an answer its caller received in
//!   time (`preview_bounded`). It becomes the preview that can be confirmed
//!   at ONE instant — one registry write under the preview's turn
//!   (`Turn::publish`) — so previews are published in the order they were
//!   read, and the one that can be confirmed is the one read last. `freeze`
//!   (inside the restart's guards)
//!   re-reads every approved target and refuses the whole restart
//!   (`ImpactChanged`) when a card, pane, server, foreground generation,
//!   executable or working directory is not the one previewed; it never
//!   rebinds an approval to a new target. Exit leases are armed before the
//!   first exit key. `seal_exits` needs BOTH the applicable final receipt
//!   (`SessionEnd`, `prompt-input-exit`) and the old client generation gone;
//!   neither stands in for the other, and they fail differently: an old
//!   exact generation that is still alive, or whose exit cannot be proven,
//!   fails the WHOLE restart before the pause and the kill (`AgentTimeout`);
//!   a client that is gone without a valid final receipt skips that card
//!   alone. `reopen` runs only after the server replacement was read back.
//! - **Card scope through the side effect.** The card ↔ session binding is
//!   read from the Board this process committed last, as authority
//!   (`documents::board_authority`: the user's current version; a Board
//!   answered from a backup names no card) — at freeze, at seal, before the
//!   shell is created, and once more under the Board writer's fence
//!   (`Env::fenced`, `documents::board_fence`) in the same fenced section
//!   that consumes the launch slot and makes the input. A card deleted or rebound meanwhile is seen (nothing is typed)
//!   or its commit lands after the input. Lock order: the restart's
//!   operation gate and exclusive activity guard (both try-locks) → the
//!   Board fence, held only for that section; no queue, settings or storage
//!   lock is taken while it is held, and nothing holding the Board fence
//!   waits for either guard. A disabled frontend control is not this fence.
//! - **Launch context: limited checks before, confirmation after.** Three
//!   things are kept apart.
//!   1. *Required before the one request* (any of them missing means
//!      nothing is typed): the exact card, pane, server, pid and birth; the
//!      frozen executable and a working directory that still exists; the
//!      old client generation gone; the cards the user ticked and the
//!      policy they chose, as bound right now; a supported launch profile
//!      of the OLD client (its own launch environment had the expected home
//!      and no refused name); the exact input edge of the new shell; and a
//!      supported baseline — what Deck hands its server
//!      (`procinfo::own_baseline_supported`), the one home variable tmux
//!      gives the pane (`launch::baseline_home`) and the system's own login
//!      shell. A baseline that is known to be unsupported is shown in the
//!      preview and refused at freeze.
//!   2. *The accepted window.* The new shell runs the user's startup files
//!      before it reads the request, and what they export cannot be read
//!      in advance by any interface Deck may use. The user accepted this:
//!      the request may run in an environment those files changed, and it
//!      may already have had native effects (a hook, a prompt of its own)
//!      when that is found out. Nothing here claims the whole environment
//!      was proven, and nothing is typed to probe it.
//!   3. *Required for `Confirmed`, after the launch*: the applicable
//!      receipt, the reporting client still the pane's foreground
//!      generation, and `launch::actual_context` — that exact process
//!      executes the frozen file, was started with exactly the request this
//!      operation built, in a launch environment with the expected home and
//!      no refused name, in the frozen directory. That check must answer
//!      inside the confirmation's share, and the client must still be the
//!      pane's foreground generation when it has. A missing client, an
//!      unreadable launch image, a check that answered late, a mismatch or
//!      a missing or conflicting
//!      receipt is `Unconfirmed` with its own reason. That is a statement
//!      about confirmation only: the started client is left as it is for
//!      the user — never killed, re-typed, re-identified or retried — and
//!      the server replacement stays settled.
//!
//!      The receipts and the confirmation are two shares, and the second
//!      never lengthens the first. A receipt counts only when it was
//!      recorded before the post share ended: every launch lease stops
//!      taking facts at that instant by the clock (`lifecycle`), and all of
//!      them are sealed together before the first check of a started client
//!      (`lifecycle::seal_all`). The confirmation's share is time for those
//!      checks alone; what it confirms, it confirms from the sealed set.
//! - **Policy.** A (`Ordinary`, the default): every approved entry is
//!   `<frozen executable> --resume <exact id>` and nothing else — no bypass,
//!   no `--permission-mode default`. B (`ReapplyBypass`) is THIS restart's
//!   explicit reauthorization: the override is added only for
//!   selected ∩ previewed-as-bypass ∩ frozen-as-bypass entries of the same
//!   exact client (`decide`). It is not "preserve the final state": a client
//!   started with bypass and lowered to plan while running gets bypass again
//!   when the user picks B after the stated warning. Ordinary and unknown
//!   profiles never get it; an unsupported launch is never re-entered. The
//!   final permission mode is read by nothing here and gates nothing. The
//!   choice is not stored and reaches no other card or restart.
//! - **`LaunchProfile`** is how the exact foreground client was started
//!   (`procinfo::qualify_client`: the product's own code in a direct launch,
//!   exact pid + birth, bounded argv reduction inside procinfo). A saved
//!   card command, shell history, terminal text, `PATH`, an earlier
//!   observation or a session id never decides it, and no observed argv is
//!   replayed.
//! - **No release is named.** A client is admitted by what its running code
//!   IS, not by a list: the adapter states the product's code requirement
//!   (`ReentryCapability::product`), and Security.framework validates it
//!   against the running image and its file. A release this build has never
//!   seen is admitted on the same facts as any other, under either policy;
//!   a file name, a version, a path, a signature that validates on disk, a
//!   lifecycle receipt or the user's approval never stands in for that
//!   identity, and none of them refuses a client either. Five things stay
//!   apart and none replaces another: executable identity, the native
//!   protocol as THIS transaction observes it (launch grammar, end receipt,
//!   resume receipt), the launch profile, the user's authorization, and the
//!   confirmation. What the product's requirement and the native contract
//!   are is `claude.rs`; how identity is proven is `procinfo`.
//! - **Executable reference.** A reference is one file state — path,
//!   device, inode, size, modification and change instants, SHA-256 — and
//!   the code identity its signature validated as, in full, under the
//!   product's requirement. The digest proves that the content is still the
//!   same inside THIS transaction; it is compared only with itself and
//!   never looked up. A client counts only while its exact generation is
//!   BOUND to the reference (`procinfo::binding`: generation, the kernel's
//!   image path, the file's state, and a dynamic validation of the running
//!   image against that code identity) — at the preview, at the freeze, when
//!   its exit receipt is attributed, and again for the client that was
//!   started. An old image whose path already holds another validly signed
//!   build, and an exec into another image under the same pid and birth
//!   instant, are not bound. The preview reads and validates a binary once
//!   for all clients that share it and binds each client on its own; the
//!   freeze reuses the previewed reference only for that exact file state;
//!   the post phase re-reads the content in full once per distinct binary
//!   (not per card) and every entry checks the state again inside the
//!   fenced section. A changed executable is never frozen again under the
//!   same approval. Nothing is cached beyond the plan.
//! - **Bounded, and honest about what did not finish.** The full reads, the
//!   signature validations and the dynamic validations of a running image
//!   cannot be interrupted. Three rules hold for each of them on every
//!   path: the preview, the freeze (also when the previewed reference is
//!   reused and only the binding is asked), the attribution of a receipt,
//!   and the started client's final check.
//!   1. *An owner.* It runs under a deadline its caller owns: the preview's
//!      share (`PREVIEW_BUDGET`), the restart's preparation, a receipt's
//!      budget (`lifecycle::RECEIPT_BUDGET`), the post share (which the
//!      entry asks again after the re-read, before its first side effect),
//!      and the confirmation's own share after it (`CONFIRM_SHARE`). A
//!      dynamic validation nobody owns is not made (`procinfo::validated`).
//!   2. *A late answer is no answer.* The deadline is asked after the call
//!      returned, and a positive answer that arrived late is dropped like
//!      any other: it names no candidate; it freezes nothing — the restart
//!      then ends as its own timeout, before a lease is armed and before
//!      any exit key; it records no receipt; it confirms no entry. What was
//!      read before such a call — a process table, the pane's foreground —
//!      is read again after it, before anything is committed on it.
//!   3. *The caller is answered anyway.* A deadline does not shorten the
//!      call, so every caller that must answer stops waiting through a
//!      worker (`session_runtime::bounded_worker`): the preview on its one
//!      worker, which holds the preview's turn while it reads and nothing
//!      else (`Turn`: the caller's own limit also bounds its wait for that
//!      turn, whoever else asks); the restart
//!      on its worker, which goes on holding the operation gate, the
//!      activity guard and the plan; the receipts on the status listener's
//!      one receipt worker. The stalled thread keeps what it owns until
//!      its call returns — no guard is released under an operation that is
//!      still running — and what it returns then reaches nobody. Nothing is
//!      retried, no second worker starts beside it (the turn, the operation
//!      gate, the one receipt queue), and a launch slot stays consumed.
//!
//!   A check that did not finish is `CheckIncomplete` before a launch —
//!   never a candidate, never launched — and `ContextUnverified` after one.
//!   Neither is ever reported as a program that changed
//!   (`ExecutableUnavailable` and `ContextChanged` are the positive
//!   findings).
//! - **Once only.** An entry has one launch slot, consumed BEFORE any
//!   transport side effect. A timeout, a missing receipt, a refused Enter, a
//!   lost reply or a crash never leads to a second request; after transport
//!   begins the outcome is `Unconfirmed` unless the atomic guard proved that
//!   nothing reached the pane. A request id names at most one operation: a
//!   repeated invoke reads the stored result and executes nothing.
//! - **Independent conclusions.** The server replacement is settled first
//!   and never rewritten by what follows; every Agent result is a per-card
//!   addition (`EntryView`). Queues stay paused; no readiness, Signal,
//!   interaction or authority is created, restored or released here.
//! - **Privacy.** Views and logs carry card id, session, closed codes and
//!   counts — never a native id, pid, path, digest, argv or environment.
//!
//! Vendor knowledge is `claude.rs` (static, pure); lifecycle wire parsing,
//! kernel attribution and the transaction mailbox are `lifecycle.rs`; the
//! one terminal-input site is `launch.rs`.

pub(crate) mod claude;
pub(crate) mod launch;
pub(crate) mod lifecycle;
#[cfg(test)]
mod tests;

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::applog::{applog, session_tag};
use crate::error::{DeckError, ErrorKind, RestartFailure};
use crate::procinfo::{
    ClientRead, ClientSpec, DirRef, ExecutableRef, LaunchGrammar, LaunchProfile, NotProduct,
    ProductIdentity, Unavailable,
};
use crate::sync::LockRecover;
use crate::tmux::{same_pane, PaneRow};

/// The optional re-entry capability of one agent adapter: static facts and
/// pure functions. It cannot approve, launch, type, unlock or confirm, and
/// it names no release: a client is admitted by what its running code is
/// (`product`), never by a version, a file name or a digest.
pub(crate) struct ReentryCapability {
    /// Whose code a client must be: the product's code requirement, checked
    /// against the running image and its file (`procinfo::qualify_client`).
    pub(crate) product: &'static ProductIdentity,
    /// The launcher's own file name: a hint for wording a refusal about a
    /// process that carries it. It admits nothing and refuses nothing — a
    /// client that runs under its release's file name is the product all
    /// the same.
    pub(crate) launcher: &'static str,
    pub(crate) grammar: &'static LaunchGrammar,
    /// The file name of the one login shell whose line editor the launch
    /// sink is written against (the system's own build of it).
    pub(crate) login_shell: &'static str,
    /// Argument words for (exact id, bypass override decided by the core).
    pub(crate) request: fn(&str, bool) -> Option<Vec<String>>,
}

// ---------- closed vocabulary -------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Policy {
    /// A: exact resume only.
    Ordinary,
    /// B: this restart's explicit bypass reauthorization.
    ReapplyBypass,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Kind {
    Candidate,
    Shell,
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Outcome {
    NotAttempted,
    RefusedBeforeLaunch,
    Confirmed,
    Unconfirmed,
    KnownNoLaunchFailure,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Reason {
    UnsupportedAgent,
    MultiplePanes,
    IdentityUnavailable,
    ExecutableUnavailable,
    FinalSessionUnconfirmed,
    LaunchRefused,
    LaunchUnconfirmed,
    UserActionRequired,
    /// A check of the client's program did not finish — its phase ended, or
    /// the file, the kernel or Security gave no answer. Not a statement
    /// that anything changed.
    CheckIncomplete,
    /// The started client is not the one this operation asked for: another
    /// request, launch environment, directory or executable.
    ContextChanged,
    /// The started client's launch facts could not be read.
    ContextUnverified,
}

impl Reason {
    fn code(self) -> &'static str {
        match self {
            Reason::UnsupportedAgent => "unsupported-agent",
            Reason::MultiplePanes => "multiple-panes",
            Reason::IdentityUnavailable => "identity-unavailable",
            Reason::ExecutableUnavailable => "executable-unavailable",
            Reason::FinalSessionUnconfirmed => "final-session-unconfirmed",
            Reason::LaunchRefused => "launch-refused",
            Reason::LaunchUnconfirmed => "launch-unconfirmed",
            Reason::UserActionRequired => "user-action-required",
            Reason::CheckIncomplete => "check-incomplete",
            Reason::ContextChanged => "context-changed",
            Reason::ContextUnverified => "context-unverified",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Warning {
    BypassNotReapplied,
    LaunchProfileUnconfirmed,
    BypassReauthorized,
    AdditionalDirsNotRestored,
}

fn profile_word(profile: LaunchProfile) -> &'static str {
    match profile {
        LaunchProfile::Bypass => "bypass",
        LaunchProfile::Ordinary => "ordinary",
        LaunchProfile::Unknown | LaunchProfile::Unsupported => "unknown",
    }
}

// ---------- the override decision (pure) ---------------------------------------

/// Everything the bypass decision may read. Nothing else exists to consult:
/// no saved command, no final permission observation, no earlier operation.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Scope {
    pub(crate) policy: Policy,
    /// The user ticked this card in THIS preview.
    pub(crate) selected: bool,
    /// The profile the confirmed preview showed for this exact client.
    pub(crate) preview: LaunchProfile,
    /// The profile read again inside the restart's guards.
    pub(crate) frozen: LaunchProfile,
    /// Card, pane, server, foreground generation, executable and working
    /// directory are the previewed ones.
    pub(crate) identity_unchanged: bool,
    /// The old client's launch image was read in full and its environment
    /// is a supported one (expected home, no refused name), independently
    /// of the profile. About the OLD client only: nothing here is a claim
    /// about the environment the new shell will hand on.
    pub(crate) context_proven: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Decision {
    /// Not ticked: no part of this operation.
    NotApproved,
    /// The approval names a target that no longer exists: preview again.
    Repreview,
    Skip(Reason),
    Reenter {
        bypass: bool,
        warnings: Vec<Warning>,
    },
}

/// The ONE place a request gains the bypass override: Policy B, a ticked
/// card, the unchanged exact client, bypass in the preview the user saw AND
/// bypass when read again at freeze. Every other combination re-enters
/// without it or does not re-enter.
pub(crate) fn decide(scope: Scope) -> Decision {
    if !scope.selected {
        return Decision::NotApproved;
    }
    if !scope.identity_unchanged {
        return Decision::Repreview;
    }
    let reapply = scope.policy == Policy::ReapplyBypass;
    match scope.frozen {
        LaunchProfile::Unsupported => Decision::Skip(Reason::IdentityUnavailable),
        LaunchProfile::Unknown if !scope.context_proven => {
            Decision::Skip(Reason::IdentityUnavailable)
        }
        LaunchProfile::Unknown => Decision::Reenter {
            bypass: false,
            warnings: if reapply {
                vec![Warning::LaunchProfileUnconfirmed]
            } else {
                Vec::new()
            },
        },
        LaunchProfile::Bypass | LaunchProfile::Ordinary => {
            let bypass = reapply
                && scope.preview == LaunchProfile::Bypass
                && scope.frozen == LaunchProfile::Bypass;
            // B was asked for and a bypass profile was shown or is now read,
            // yet the intersection is empty: say that it was not reapplied.
            let dropped = reapply
                && !bypass
                && (scope.preview == LaunchProfile::Bypass
                    || scope.frozen == LaunchProfile::Bypass);
            Decision::Reenter {
                bypass,
                warnings: if dropped {
                    vec![Warning::BypassNotReapplied]
                } else {
                    Vec::new()
                },
            }
        }
    }
}

// ---------- views ---------------------------------------------------------------

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreviewEntryView {
    card_id: String,
    session: String,
    kind: Kind,
    reason: Option<Reason>,
    launch_profile: Option<&'static str>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreviewView {
    token: String,
    lifecycle_observation: bool,
    entries: Vec<PreviewEntryView>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EntryView {
    pub(crate) card_id: String,
    pub(crate) session: String,
    pub(crate) outcome: Outcome,
    pub(crate) reason: Option<Reason>,
    pub(crate) warnings: Vec<Warning>,
    /// A live shell was created for this card by this operation.
    pub(crate) shell: bool,
    /// A new client process was observed in that shell.
    pub(crate) client_observed: bool,
    /// This entry's outcome is final. `false` while the operation is still
    /// running for it: not a result, and never to be shown as one.
    pub(crate) settled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum OperationState {
    Running,
    Complete,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReentryResult {
    pub(crate) request_id: String,
    pub(crate) state: OperationState,
    pub(crate) server_replaced: bool,
    pub(crate) entries: Vec<EntryView>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OperationView {
    known: bool,
    result: Option<ReentryResult>,
}

/// The user's confirmation for one restart.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Approval {
    pub(crate) token: String,
    pub(crate) cards: Vec<String>,
    pub(crate) policy: Policy,
}

// ---------- environment -----------------------------------------------------------

/// A tmux call that carries bytes on stdin.
pub(crate) type TmuxStdin<'a> = &'a dyn Fn(&[&str], &[u8]) -> Result<String, DeckError>;

/// Everything the coordinator reaches outside this process's memory.
/// One assembly builds it (`tmux_lifecycle::assemble_reentry_env`): for
/// production from deck's own server, for the integration tests from a
/// throwaway bundled-tmux socket — with the same Board reader, Board fence,
/// shell creator and login shell.
pub(crate) struct Env<'a> {
    pub(crate) tmux: &'a dyn Fn(&[&str]) -> Result<String, DeckError>,
    pub(crate) tmux_stdin: TmuxStdin<'a>,
    /// The tmux socket name lifecycle claims must carry.
    pub(crate) socket: &'a str,
    /// (card id, session) of every card on the committed Board; `None` when
    /// the Board cannot be read.
    pub(crate) cards: &'a dyn Fn() -> Option<Vec<(String, String)>>,
    pub(crate) capability: &'a ReentryCapability,
    /// The bundled lifecycle helper a receipt's reporter must be executing.
    pub(crate) helper: &'a Path,
    /// The home directory a supported client and its new shell run under.
    pub(crate) home: &'a Path,
    /// The user's login shell as the account database names it.
    pub(crate) login_shell: &'a Path,
    /// The lifecycle hooks are installed (the user's explicit opt-in).
    pub(crate) observation: bool,
    /// What Deck handed to the server it starts carries the expected home
    /// and no refused name (`procinfo::own_baseline_supported`). A baseline
    /// only: not what a shell's startup files leave behind.
    pub(crate) baseline: bool,
    /// Run `work` while holding the Board writer's fence
    /// (`documents::board_fence`): no Board commit can land between the
    /// binding check and the irreversible input made inside it.
    pub(crate) fenced: &'a dyn Fn(&mut dyn FnMut()),
    /// Create one command-less shell for `session` in exactly `dir`, with
    /// the ordinary transcript restore when `restore` is set. Called under
    /// the restart's guards; must take none of them itself.
    pub(crate) create_shell: &'a dyn Fn(&str, &Path, bool) -> Result<(), DeckError>,
}

impl Env<'_> {
    fn rows(&self) -> Result<Vec<PaneRow>, DeckError> {
        crate::tmux::list_panes_with(self.tmux)
    }
}

// ---------- preview ---------------------------------------------------------------

/// A birth instant as procinfo reads it.
type Birth = (u64, u32);

/// One exact client as qualified at one instant.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Client {
    pid: u32,
    birth: Birth,
    executable: ExecutableRef,
    profile: LaunchProfile,
    additional_dirs: bool,
    cwd: DirRef,
}

#[derive(Clone, Debug)]
struct Inspected {
    kind: Kind,
    reason: Option<Reason>,
    shell_birth: Option<Birth>,
    client: Option<Client>,
}

impl Inspected {
    fn refused(kind: Kind, reason: Reason) -> Self {
        Self {
            kind,
            reason: Some(reason),
            shell_birth: None,
            client: None,
        }
    }
}

/// Why a client's program is not available as a reference, as a reason.
fn unavailable(why: Unavailable) -> Reason {
    match why {
        Unavailable::Changed => Reason::ExecutableUnavailable,
        Unavailable::Unverified => Reason::CheckIncomplete,
    }
}

/// What one pane of a one-pane session is, for re-entry. The foreground is
/// read from the kernel; a saved command plays no part. `known` holds the
/// executable references this same operation already validated: a client
/// that runs one of those very file states is bound to it without the file
/// being read again.
fn inspect(env: &Env, row: &PaneRow, panes: usize, known: &[ExecutableRef]) -> Inspected {
    if panes != 1 {
        return Inspected::refused(Kind::Unsupported, Reason::MultiplePanes);
    }
    let table = crate::procinfo::processes();
    let Some(shell) = table.get(&row.pane_pid) else {
        return Inspected::refused(Kind::Unsupported, Reason::IdentityUnavailable);
    };
    let shell_birth = (shell.start_seconds, shell.start_micros);
    let Some(foreground) = crate::agent_status::foreground_generation(&table, row.pane_pid) else {
        return Inspected::refused(Kind::Unsupported, Reason::IdentityUnavailable);
    };
    if foreground.pid() == row.pane_pid {
        return Inspected {
            kind: Kind::Shell,
            reason: None,
            shell_birth: Some(shell_birth),
            client: None,
        };
    }
    let capability = env.capability;
    let read = crate::procinfo::qualify_client(
        foreground.pid(),
        foreground.birth(),
        &ClientSpec {
            product: capability.product,
            launcher: capability.launcher,
            grammar: capability.grammar,
            home: env.home,
        },
        known,
    );
    let qualified = match read {
        ClientRead::Gone => {
            return Inspected::refused(Kind::Unsupported, Reason::IdentityUnavailable)
        }
        // Nothing is known about what it runs: the question was not answered,
        // or not inside this phase. That is a check that did not finish —
        // not a shell, not an unsupported program and not a changed one —
        // whatever the process is called.
        ClientRead::NotProduct {
            why: NotProduct::Unreadable,
            ..
        } => return Inspected::refused(Kind::Unsupported, Reason::CheckIncomplete),
        // Some other program: a shell, or a foreground this feature does
        // not know.
        ClientRead::NotProduct { named: false, .. } => {
            return if crate::context::shell_process(Some(&row.command)) {
                Inspected {
                    kind: Kind::Shell,
                    reason: None,
                    shell_birth: Some(shell_birth),
                    client: None,
                }
            } else {
                Inspected::refused(Kind::Unsupported, Reason::UnsupportedAgent)
            }
        }
        // It carries the launcher's name and its running code was shown not
        // to be the product's: never a candidate, whatever it reports later.
        // The name only chooses the words; it was not what refused it.
        ClientRead::NotProduct { named: true, why } => {
            return Inspected::refused(
                Kind::Unsupported,
                match why {
                    NotProduct::Other => Reason::IdentityUnavailable,
                    NotProduct::Diverged => Reason::ExecutableUnavailable,
                    NotProduct::Unreadable => Reason::CheckIncomplete,
                },
            )
        }
        ClientRead::Product(qualified) => qualified,
    };
    let executable = match qualified.executable {
        Ok(executable) => executable,
        Err(why) => return Inspected::refused(Kind::Unsupported, unavailable(why)),
    };
    // a direct foreground launch: the pane's own shell is the client's parent
    let direct = table
        .get(&foreground.pid())
        .is_some_and(|client| client.ppid == row.pane_pid);
    let Some(cwd) = qualified.cwd.filter(DirRef::is_dir) else {
        return Inspected::refused(Kind::Unsupported, Reason::IdentityUnavailable);
    };
    if !direct
        || !matches!(
            qualified.profile,
            LaunchProfile::Bypass | LaunchProfile::Ordinary
        )
    {
        return Inspected::refused(Kind::Unsupported, Reason::IdentityUnavailable);
    }
    Inspected {
        kind: Kind::Candidate,
        reason: None,
        shell_birth: Some(shell_birth),
        client: Some(Client {
            pid: foreground.pid(),
            birth: foreground.birth(),
            executable,
            profile: qualified.profile,
            additional_dirs: qualified.additional_dirs,
            cwd,
        }),
    }
}

struct PreviewEntry {
    card_id: String,
    row: PaneRow,
    inspected: Inspected,
}

struct Preview {
    token: String,
    made: Instant,
    entries: Vec<PreviewEntry>,
}

/// A preview nobody confirmed stops being confirmable after this long.
const PREVIEW_TTL: Duration = Duration::from_secs(300);
/// One preview's whole share for reading and validating the clients'
/// programs (an implementation default, not a promise). A program whose
/// check does not finish inside it is shown as not checked — never as
/// changed, never as a candidate.
pub(crate) const PREVIEW_BUDGET: Duration = Duration::from_secs(10);

/// One preview at a time: the instant the one that has the turn must be over
/// by, or `None`.
static PREVIEW_TURN: Mutex<Option<Instant>> = Mutex::new(None);
/// Signalled when a turn is given back.
static PREVIEW_FREED: Condvar = Condvar::new();
/// How many previews are waiting for the turn right now. Tests order their
/// threads by it; nothing else reads it.
#[cfg(test)]
static TURN_WAITERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The turn of one preview: the right to read the clients' programs and to
/// publish what was read, until `end`.
///
/// A check that is running cannot be interrupted, so a second request never
/// starts the same reads beside it. And what a read found is published under
/// the turn it was read in, so two previews are never published in another
/// order than they were read:
///
/// - **One owner.** This value IS the turn. It is moved, never copied: from
///   the caller into its worker, and back to the caller with the answer.
///   Dropping it — after the publication, on any early return, on a panic,
///   or on the worker when nobody waits for its answer any more — gives the
///   turn back, and nothing else does. A worker that outlived its caller
///   therefore holds its OWN turn until its read returns: it can give back no
///   other, and it has nothing to publish with.
/// - **Waiting has the caller's own limit.** `take` waits while the holder
///   can still answer in time and never past `end`, the instant the caller
///   itself must be answered by. That limit is fixed when the caller asked;
///   whoever takes the turn next, however many come, moves it for nobody. A
///   holder past its own deadline is stalled, and waiting for it is waiting
///   for nothing: the next request is refused at once.
/// - **Publication is one instant** (`publish`).
struct Turn {
    end: Instant,
}

impl Turn {
    /// Take the turn for a preview that must be over by `end`. Waits on the
    /// thread that asked, starting nothing.
    fn take(end: Instant) -> Result<Self, DeckError> {
        let mut held = PREVIEW_TURN.lock_or_recover();
        loop {
            let now = Instant::now();
            // The caller's own limit decides first: a turn it could no
            // longer use is not taken, and no other holder's deadline is
            // waited for past it.
            if now >= end {
                return Err(preview_busy());
            }
            match *held {
                None => {
                    *held = Some(end);
                    return Ok(Self { end });
                }
                Some(over) if now >= over => return Err(preview_busy()),
                Some(over) => {
                    #[cfg(test)]
                    TURN_WAITERS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    held = crate::sync::wait_timeout_or_recover(
                        &PREVIEW_FREED,
                        held,
                        over.min(end) - now,
                    );
                    #[cfg(test)]
                    TURN_WAITERS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                }
            }
        }
    }

    /// Publish what this turn read. This is the ONE instant a preview becomes
    /// the preview that can be confirmed: one write of the registry, made by
    /// the holder of the turn, with the turn's own deadline asked under the
    /// lock that writes. `freeze` takes the preview under that same lock, so
    /// a publication and a confirmation are always one before the other.
    ///
    /// Reads are one at a time and each is published before its turn is
    /// given back, so publications follow the reads: the preview that can be
    /// confirmed is always the one that was read last, and a result that was
    /// read earlier can never replace it. Past the deadline nothing is
    /// written — the earlier preview is neither replaced nor revived — and
    /// the caller is told that this one timed out. Whatever happens, the turn
    /// is given back when this returns.
    fn publish(self, previewed: Previewed) -> Result<PreviewView, DeckError> {
        let Previewed { view, kept } = previewed;
        let published = with_registry(|registry| {
            let in_time = Instant::now() < self.end;
            if in_time {
                // replaces any earlier one: only the latest can be confirmed
                registry.preview = kept;
            }
            in_time
        });
        if !published {
            return Err(preview_timeout());
        }
        applog(&format!(
            "[agent-reentry] preview sessions={} candidates={}",
            view.entries.len(),
            view.entries
                .iter()
                .filter(|entry| entry.kind == Kind::Candidate)
                .count()
        ));
        Ok(view)
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        *PREVIEW_TURN.lock_or_recover() = None;
        PREVIEW_FREED.notify_all();
    }
}

fn preview_busy() -> DeckError {
    DeckError::new(ErrorKind::Other, "reentry-preview-busy")
}

/// A preview that was not answered, or not published, inside its limit.
fn preview_timeout() -> DeckError {
    applog("[agent-reentry] preview-timeout nothing-kept");
    DeckError::new(ErrorKind::Other, "reentry-preview-timeout")
}
/// How long a finished operation's result stays readable.
const RESULT_TTL: Duration = Duration::from_secs(600);
const RESULTS_KEPT: usize = 4;

#[derive(Default)]
struct Registry {
    preview: Option<Preview>,
    active: Option<Arc<Mutex<ReentryResult>>>,
    finished: VecDeque<(Instant, ReentryResult)>,
}

static REGISTRY: Mutex<Option<Registry>> = Mutex::new(None);

fn with_registry<R>(f: impl FnOnce(&mut Registry) -> R) -> R {
    f(REGISTRY
        .lock_or_recover()
        .get_or_insert_with(Registry::default))
}

fn opaque_token() -> Result<String, DeckError> {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; 16];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| DeckError::new(ErrorKind::Other, "reentry-token-unavailable"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Each card's session that is live, and what it is for re-entry.
fn previewed(env: &Env) -> Result<Vec<PreviewEntry>, DeckError> {
    let rows = env.rows()?;
    let cards = (env.cards)().unwrap_or_default();
    let mut entries: Vec<PreviewEntry> = Vec::new();
    // The references this preview has validated so far. Clients that share
    // one binary read and validate it once; each is still bound to it on its
    // own. The list lives for this call only.
    let mut known: Vec<ExecutableRef> = Vec::new();
    for (card_id, session) in &cards {
        // a session two cards claim is nobody's exact target
        if cards.iter().filter(|(_, other)| other == session).count() != 1 {
            continue;
        }
        let panes: Vec<&PaneRow> = rows
            .iter()
            .filter(|row| row.session_name == *session)
            .collect();
        let Some(row) = panes.first() else {
            continue; // a stopped card is never started
        };
        let inspected = inspect(env, row, panes.len(), &known);
        if let Some(client) = &inspected.client {
            if !known.contains(&client.executable) {
                known.push(client.executable.clone());
            }
        }
        entries.push(PreviewEntry {
            card_id: card_id.clone(),
            row: (*row).clone(),
            inspected,
        });
    }
    Ok(entries)
}

/// What one preview read, before anybody was answered with it. Nothing of it
/// can be confirmed: only `Turn::publish` makes it the preview a token names.
pub(crate) struct Previewed {
    view: PreviewView,
    kept: Option<Preview>,
}

/// Read the provisional candidates for the restart dialog. Reads only, and
/// installs nothing: what it returns is not a preview anybody can confirm
/// until the caller that was answered in time publishes it. Every check of a
/// client's program runs under the calling thread's deadline, which the
/// caller owns (`preview_bounded`).
pub(crate) fn read_preview(env: &Env) -> Result<Previewed, DeckError> {
    let entries = previewed(env)?;
    let token = opaque_token()?;
    let baseline = launch::supported_before_restart(env);
    let launchable = env.observation && baseline;
    let view = PreviewView {
        token: token.clone(),
        lifecycle_observation: env.observation,
        entries: entries
            .iter()
            .map(|entry| {
                // Conditions known before the restart are shown now, not
                // discovered after the client was exited, and each as what
                // it is. Without a supported baseline and the supported
                // login shell nothing would be typed: the entry is not a
                // candidate. Observation that is merely off is the user's
                // own setting, not something wrong with the session: the
                // entry stays a candidate and `lifecycle_observation: false`
                // says why none of them can be chosen, so the dialog can
                // name the setting instead of an unverifiable session.
                // Either way nothing is kept below: no token, no approval.
                let (kind, reason) = match entry.inspected.kind {
                    Kind::Candidate if !baseline => {
                        (Kind::Unsupported, Some(Reason::IdentityUnavailable))
                    }
                    kind => (kind, entry.inspected.reason),
                };
                PreviewEntryView {
                    card_id: entry.card_id.clone(),
                    session: entry.row.session_name.clone(),
                    kind,
                    reason,
                    launch_profile: entry
                        .inspected
                        .client
                        .as_ref()
                        .filter(|_| kind == Kind::Candidate)
                        .map(|client| profile_word(client.profile)),
                }
            })
            .collect(),
    };
    Ok(Previewed {
        view,
        kept: launchable.then_some(Preview {
            token,
            made: Instant::now(),
            entries,
        }),
    })
}

/// The provisional candidates for the restart dialog, answered in bounded
/// time whatever the checks do.
///
/// The caller has ONE limit, fixed when it asked: `PREVIEW_BUDGET` from now.
/// Waiting for the turn, the read and the publication all end by it.
///
/// The checks of a client's program end in calls that cannot be interrupted,
/// so a deadline scope alone bounds nothing for the caller. `read` — the
/// environment's assembly and `read_preview` — therefore runs on ONE worker
/// thread, under the preview's turn, and this caller waits for it no longer
/// than its limit:
///
/// - answered in time, the turn comes back with the answer and THIS thread
///   publishes under it (`Turn::publish`): a token exists only for an answer
///   its caller received, and no other preview reads or publishes between
///   the read and its publication;
/// - not answered in time, the caller gets `reentry-preview-timeout` and the
///   worker goes on until its call returns. It holds its own turn and
///   nothing else — no restart guard, no Board fence, no lease — and what it
///   returns then has no receiver: nothing is published, no token exists
///   for it, and an earlier preview is neither replaced nor revived;
/// - while that worker holds the turn no other preview starts a thread or a
///   read (`Turn::take`), so a stalled check costs one thread, once. Nothing
///   is asked again on anybody's behalf.
pub(crate) fn preview_bounded(
    read: impl FnOnce() -> Result<Previewed, DeckError> + Send + 'static,
) -> Result<PreviewView, DeckError> {
    preview_within(PREVIEW_BUDGET, read)
}

/// `preview_bounded` with the preview's share as an argument: production's
/// is `PREVIEW_BUDGET`, a test's is shorter.
fn preview_within(
    budget: Duration,
    read: impl FnOnce() -> Result<Previewed, DeckError> + Send + 'static,
) -> Result<PreviewView, DeckError> {
    let (previewed, turn) = read_in_turn(Instant::now() + budget, read)?;
    turn.publish(previewed)
}

/// What one preview read, and the turn it was read under: for a caller that
/// was answered before `end`, its own limit. Nothing is published here.
fn read_in_turn(
    end: Instant,
    read: impl FnOnce() -> Result<Previewed, DeckError> + Send + 'static,
) -> Result<(Previewed, Turn), DeckError> {
    use crate::session_runtime::{bounded_worker, Unanswered};
    let turn = Turn::take(end)?;
    let answer = bounded_worker("deck-reentry-preview", end, move || {
        let read = read();
        // The turn goes back with the answer, for the caller to publish
        // under. When nobody waits for the answer any more, both are dropped
        // on this thread: the turn is then over when the read returned,
        // however late, and not an instant before.
        (read, turn)
    });
    match answer {
        Ok((read, turn)) => read.map(|previewed| (previewed, turn)),
        Err(Unanswered::Late) => Err(preview_timeout()),
        Err(Unanswered::Failed) => Err(DeckError::new(ErrorKind::Other, "reentry-preview-failed")),
    }
}

/// `read_preview` and its publication on the calling thread, under the turn
/// and its deadline: the same preview without the worker (tests that hold
/// the world on their own thread).
#[cfg(test)]
pub(crate) fn preview(env: &Env) -> Result<PreviewView, DeckError> {
    preview_by(env, Instant::now() + PREVIEW_BUDGET)
}

/// `preview` for a caller whose own limit is `end`.
#[cfg(test)]
pub(crate) fn preview_by(env: &Env, end: Instant) -> Result<PreviewView, DeckError> {
    let turn = Turn::take(end)?;
    let _deadline = crate::session_runtime::Deadline::until(turn.end);
    turn.publish(read_preview(env)?)
}

// ---------- operation registry ------------------------------------------------------

fn expired_preview() -> DeckError {
    DeckError::new(ErrorKind::ContextChanged, "reentry-preview-expired")
}

/// The stored result of `request_id`: a running operation's known facts or
/// a finished one's result. Read-only.
pub(crate) fn known(request_id: &str) -> Option<ReentryResult> {
    with_registry(|registry| {
        registry
            .finished
            .retain(|(at, _)| at.elapsed() < RESULT_TTL);
        registry
            .active
            .as_ref()
            .map(|shared| shared.lock_or_recover().clone())
            .filter(|result| result.request_id == request_id)
            .or_else(|| {
                registry
                    .finished
                    .iter()
                    .find(|(_, result)| result.request_id == request_id)
                    .map(|(_, result)| result.clone())
            })
    })
}

pub(crate) fn operation(request_id: &str) -> OperationView {
    let result = known(request_id);
    OperationView {
        known: result.is_some(),
        result,
    }
}

// ---------- the frozen plan ---------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    /// The one launch this entry may ever make is still unused.
    Armed,
    /// Consumed: a transport side effect may have happened. Never re-armed.
    Consumed,
}

struct Entry {
    view: EntryView,
    old: PaneRow,
    shell_birth: Birth,
    client: Client,
    bypass: bool,
    /// The sealed final native session id. Never leaves this module except
    /// inside `request`.
    final_id: Option<String>,
    /// The argument words of the one request this entry typed. They go to
    /// the launch sink as text and to `procinfo::check_launch` as the
    /// expectation the started client is compared with; nowhere else.
    request: Vec<String>,
    slot: Slot,
    new: Option<PaneRow>,
    /// The request was submitted and a receipt is awaited.
    awaiting: bool,
}

/// One restart's frozen re-entry plan. Dropping it — on completion, on any
/// early return, on a panic — discards the leases and publishes what is
/// known as the operation's final result.
pub(crate) struct Plan {
    shared: Arc<Mutex<ReentryResult>>,
    entries: Vec<Entry>,
    /// Executable references whose content was re-read in full during this
    /// post phase. Entries that share a binary read it once; each still
    /// checks the file's state (`ExecutableRef::same_state`) right before
    /// its own input. Lives and dies with the plan.
    reread: Vec<ExecutableRef>,
}

/// A whole-restart share for shell creation, launch and receipts
/// (implementation default, to be validated; not a per-card budget and not
/// a product promise).
pub(crate) const POST_BUDGET: Duration = Duration::from_secs(15);
/// The confirmation's own share, after the post share: what the checks of
/// the started clients may take, together, before an entry is settled. It
/// follows `POST_BUDGET` instead of being cut out of it, so that waiting for
/// one receipt until the post share ends leaves the others their check; an
/// answer that arrives after it confirms nothing (implementation default).
pub(crate) const CONFIRM_SHARE: Duration = Duration::from_millis(1000);
/// The least remaining share an entry needs to be started at all.
const ATTEMPT_FLOOR: Duration = Duration::from_millis(1500);
/// How long after the last awaited receipt the facts stay open.
const SETTLE: Duration = Duration::from_millis(400);
/// How long one new shell may take to show a positively verified prompt.
const READY_BUDGET: Duration = Duration::from_secs(6);

/// Freeze the user's approval inside the restart's guards: consume the
/// token, re-read every approved target against `rows` (the restart's own
/// validated listing), decide each entry and arm its exit lease.
pub(crate) fn freeze(
    env: &Env,
    request_id: &str,
    approval: &Approval,
    rows: &[PaneRow],
) -> Result<Plan, DeckError> {
    if !launch::supported_before_restart(env) {
        return Err(expired_preview());
    }
    // The token is consumed whatever follows: an approval is used once.
    let preview = with_registry(|registry| {
        if registry.active.is_some() {
            return Err(DeckError::new(
                ErrorKind::Tmux,
                "tmux-server-restart-in-progress",
            ));
        }
        registry
            .preview
            .take()
            .filter(|preview| {
                preview.token == approval.token && preview.made.elapsed() < PREVIEW_TTL
            })
            .ok_or_else(expired_preview)
    })?;
    let mut seen = std::collections::HashSet::new();
    if approval.cards.is_empty() || !approval.cards.iter().all(|card| seen.insert(card.as_str())) {
        return Err(expired_preview());
    }
    let cards = (env.cards)().ok_or_else(|| DeckError::restart(RestartFailure::ImpactChanged))?;
    let changed = || DeckError::restart(RestartFailure::ImpactChanged);
    let mut entries = Vec::new();
    for card in &approval.cards {
        // only a candidate the user was shown can be approved
        let shown = preview
            .entries
            .iter()
            .find(|entry| entry.card_id == *card && entry.inspected.kind == Kind::Candidate)
            .ok_or_else(expired_preview)?;
        let (Some(shown_client), Some(shown_shell)) =
            (shown.inspected.client.as_ref(), shown.inspected.shell_birth)
        else {
            return Err(expired_preview());
        };
        let session = &shown.row.session_name;
        // the card still names this session, and nothing else does
        let bound = cards.iter().filter(|(_, s)| s == session).count() == 1
            && cards.iter().any(|(id, s)| id == card && s == session);
        let panes: Vec<&PaneRow> = rows
            .iter()
            .filter(|row| row.session_name == *session)
            .collect();
        let same_row = panes.len() == 1
            && same_pane(panes[0], &shown.row)
            && panes[0].command == shown.row.command;
        if !bound || !same_row {
            return Err(changed());
        }
        let now = inspect(
            env,
            panes[0],
            panes.len(),
            std::slice::from_ref(&shown_client.executable),
        );
        // That check ends in a validation that cannot be interrupted. One
        // that returned after the preparation ended froze nothing, whatever
        // it answered: the restart ends here as its own timeout — before a
        // lease is armed, before any exit key — and not as a target that
        // changed.
        crate::session_runtime::check_deadline()?;
        // The exact client: generation, executable and working directory.
        // The profile is compared by `decide`, not here.
        let identity_unchanged = now.shell_birth == Some(shown_shell)
            && now.client.as_ref().is_some_and(|client| {
                client.pid == shown_client.pid
                    && client.birth == shown_client.birth
                    && client.executable == shown_client.executable
                    && client.cwd == shown_client.cwd
            });
        let frozen = match (&now.client, now.reason) {
            (Some(client), _) => client.profile,
            // The check of this target did not finish: nothing is known to
            // have changed, and the approval cannot be confirmed now. Refused
            // before any exit key, as an approval that is no longer current.
            (None, Some(Reason::CheckIncomplete)) => return Err(expired_preview()),
            // A client that is the same generation, still bound to the
            // previewed executable, but no longer classifies as a supported
            // launch keeps its identity and loses its qualification.
            (None, Some(Reason::IdentityUnavailable)) => {
                use crate::procinfo::Binding;
                let bound = crate::procinfo::binding(
                    shown_client.pid,
                    shown_client.birth,
                    &shown_client.executable,
                );
                crate::session_runtime::check_deadline()?;
                match bound {
                    Binding::Bound => LaunchProfile::Unsupported,
                    // no answer about the exact client is not a change
                    Binding::Unreadable => return Err(expired_preview()),
                    Binding::Gone | Binding::Other => return Err(changed()),
                }
            }
            (None, _) => return Err(changed()),
        };
        let decision = decide(Scope {
            policy: approval.policy,
            selected: true,
            preview: shown_client.profile,
            frozen,
            identity_unchanged: identity_unchanged || now.client.is_none(),
            // argv and environment come from one read: an unreadable
            // profile leaves the old client's environment unread as well
            context_proven: now.client.is_some(),
        });
        let mut view = EntryView {
            card_id: card.clone(),
            session: session.clone(),
            outcome: Outcome::NotAttempted,
            reason: None,
            warnings: Vec::new(),
            shell: false,
            client_observed: false,
            settled: false,
        };
        let (bypass, eligible) = match decision {
            Decision::NotApproved | Decision::Repreview => return Err(changed()),
            Decision::Skip(reason) => {
                view.reason = Some(reason);
                (false, false)
            }
            Decision::Reenter { bypass, warnings } => {
                view.warnings = warnings;
                (bypass, true)
            }
        };
        let client = now.client.unwrap_or_else(|| shown_client.clone());
        if client.additional_dirs {
            view.warnings.push(Warning::AdditionalDirsNotRestored);
        }
        entries.push((
            Entry {
                view,
                old: panes[0].clone(),
                shell_birth: shown_shell,
                client,
                bypass,
                final_id: None,
                request: Vec::new(),
                slot: Slot::Armed,
                new: None,
                awaiting: false,
            },
            eligible,
        ));
    }
    let shared = Arc::new(Mutex::new(ReentryResult {
        request_id: request_id.to_string(),
        state: OperationState::Running,
        server_replaced: false,
        entries: entries
            .iter()
            .map(|(entry, _)| entry.view.clone())
            .collect(),
    }));
    with_registry(|registry| registry.active = Some(shared.clone()));
    lifecycle::begin(env.socket, env.helper);
    for (index, (entry, eligible)) in entries.iter().enumerate() {
        if *eligible {
            lifecycle::arm(
                index,
                lifecycle::Lease::exit(
                    entry.old.server_pid,
                    &entry.old.pane_id,
                    (entry.old.pane_pid, entry.shell_birth),
                    (entry.client.pid, entry.client.birth),
                    entry.client.executable.clone(),
                ),
            );
        }
    }
    applog(&format!(
        "[agent-reentry] frozen entries={} eligible={} bypass={}",
        entries.len(),
        entries.iter().filter(|(_, eligible)| *eligible).count(),
        entries.iter().filter(|(entry, _)| entry.bypass).count()
    ));
    Ok(Plan {
        shared,
        entries: entries.into_iter().map(|(entry, _)| entry).collect(),
        reread: Vec::new(),
    })
}

impl Plan {
    fn publish(&self) {
        self.shared.lock_or_recover().entries = self
            .entries
            .iter()
            .map(|entry| entry.view.clone())
            .collect();
    }

    /// The frozen clients' panes: the restart's graceful-exit path treats
    /// exactly these rows as the client, whatever tmux names their program.
    /// Each was qualified and frozen as the product by its running code;
    /// nothing here is taken from the name tmux shows.
    pub(crate) fn frozen_rows(&self) -> Vec<PaneRow> {
        self.entries.iter().map(|entry| entry.old.clone()).collect()
    }

    /// Whether `entry`'s card still names its session and nothing else
    /// does, on the committed Board as read now.
    fn bound(env: &Env, entry: &Entry) -> bool {
        (env.cards)().is_some_and(|cards| {
            let session = &entry.view.session;
            cards.iter().filter(|(_, s)| s == session).count() == 1
                && cards
                    .iter()
                    .any(|(id, s)| *id == entry.view.card_id && s == session)
        })
    }

    /// After every agent returned to its shell.
    ///
    /// An old exact client generation that is still alive — or whose exit
    /// cannot be proven — fails the WHOLE restart here, before the queue
    /// pause and before any kill: a shell in the foreground is not the
    /// client's exit, and replacing the server under a live client is not
    /// this feature's to decide. It keeps the existing `AgentTimeout`
    /// contract and budget; nothing is waited for longer and nothing is
    /// killed. With the generation gone, an entry keeps going only with its
    /// applicable final receipt; without one that card alone is skipped.
    pub(crate) fn seal_exits(&mut self, env: &Env) -> Result<(), DeckError> {
        use crate::procinfo::Liveness;
        let liveness: Vec<Liveness> = self
            .entries
            .iter()
            .map(|entry| crate::procinfo::generation_liveness(entry.client.pid, entry.client.birth))
            .collect();
        if liveness.iter().any(|state| *state != Liveness::Gone) {
            applog(&format!(
                "[agent-reentry] old-client-alive alive={} unknown={}",
                liveness.iter().filter(|s| **s == Liveness::Alive).count(),
                liveness.iter().filter(|s| **s == Liveness::Unknown).count()
            ));
            for index in 0..self.entries.len() {
                lifecycle::seal(index);
            }
            return Err(DeckError::restart(RestartFailure::AgentTimeout));
        }
        for (index, entry) in self.entries.iter_mut().enumerate() {
            let facts = lifecycle::seal(index);
            if entry.view.reason.is_some() {
                continue;
            }
            entry.final_id = lifecycle::final_session(&facts);
            if entry.final_id.is_none() {
                entry.view.reason = Some(Reason::FinalSessionUnconfirmed);
            } else if !entry.client.cwd.is_dir() {
                entry.final_id = None;
                entry.view.reason = Some(Reason::UserActionRequired);
            } else if !Self::bound(env, entry) {
                // the card was deleted or rebound after the freeze
                entry.final_id = None;
                entry.view.reason = Some(Reason::IdentityUnavailable);
            }
        }
        // One operation launches a native session at most once: entries
        // that sealed the same id are both left to the user, with no winner.
        let ids: Vec<Option<String>> = self.entries.iter().map(|e| e.final_id.clone()).collect();
        for entry in &mut self.entries {
            let Some(id) = &entry.final_id else { continue };
            if ids
                .iter()
                .filter(|other| other.as_ref() == Some(id))
                .count()
                > 1
            {
                entry.final_id = None;
                entry.view.reason = Some(Reason::IdentityUnavailable);
            }
        }
        applog(&format!(
            "[agent-reentry] sealed final={} of={}",
            self.entries.iter().filter(|e| e.final_id.is_some()).count(),
            self.entries.len()
        ));
        self.publish();
        Ok(())
    }

    /// The server replacement was read back. From here on nothing in this
    /// plan can turn the restart into a failure.
    pub(crate) fn server_replaced(&mut self) {
        self.shared.lock_or_recover().server_replaced = true;
    }

    fn settle(entry: &mut Entry, outcome: Outcome, reason: Option<Reason>) {
        entry.view.outcome = outcome;
        entry.view.reason = reason;
        entry.view.settled = true;
        entry.awaiting = false;
        applog(&format!(
            "[agent-reentry] entry {} outcome={} reason={}",
            session_tag(&entry.view.session),
            match outcome {
                Outcome::NotAttempted => "not-attempted",
                Outcome::RefusedBeforeLaunch => "refused-before-launch",
                Outcome::Confirmed => "confirmed",
                Outcome::Unconfirmed => "unconfirmed",
                Outcome::KnownNoLaunchFailure => "known-no-launch-failure",
            },
            reason.map_or("none", Reason::code)
        ));
    }

    /// The post phase: for each sealed entry create its shell, verify it,
    /// consume the launch slot, send the one request, then wait together
    /// for the receipts until `end`. `end` is where the receipts end —
    /// nothing is recorded for a launch lease from that instant on — and
    /// the confirmation's own share (`CONFIRM_SHARE`) starts there: it is
    /// time for the checks of the started clients, never more time for a
    /// receipt. `new_server` is the replaced server's pid as read back.
    pub(crate) fn reopen(
        &mut self,
        env: &Env,
        new_server: u32,
        restore_shells: bool,
        end: Instant,
        held: &std::sync::RwLockWriteGuard<'static, ()>,
        progress: &dyn Fn(&str, usize, usize),
    ) {
        let total = self
            .entries
            .iter()
            .filter(|entry| entry.final_id.is_some())
            .count();
        let settled = |entries: &[Entry]| {
            entries
                .iter()
                .filter(|entry| entry.final_id.is_some() && !entry.awaiting)
                .filter(|entry| entry.slot == Slot::Consumed || entry.view.reason.is_some())
                .count()
        };
        // Launches and receipts share the post share, and nothing of them
        // outlives it.
        let launching = crate::session_runtime::Deadline::until(end);
        progress("reopening", 0, total);
        for entry in &mut self.entries {
            entry.view.settled = entry.final_id.is_none();
        }
        self.publish();
        for index in 0..self.entries.len() {
            if self.entries[index].final_id.is_none() {
                continue;
            }
            self.launch(env, index, new_server, restore_shells, end, held);
            self.publish();
            progress("reopening", settled(&self.entries), total);
        }
        // Receipts are awaited together; a receipt wakes the wait.
        loop {
            let mut pending = false;
            for (index, entry) in self.entries.iter_mut().enumerate() {
                if !entry.awaiting {
                    continue;
                }
                let facts = lifecycle::peek(index);
                let wanted = entry.final_id.as_deref().unwrap_or_default();
                match lifecycle::reentry(&facts, wanted) {
                    lifecycle::Receipt::Pending => pending = true,
                    // Settled as confirmed only below, from the sealed
                    // facts, so a conflicting fact that arrives while other
                    // entries are still awaited counts against it.
                    lifecycle::Receipt::Entered(_) => {}
                    lifecycle::Receipt::Conflict => {
                        Self::settle(entry, Outcome::Unconfirmed, Some(Reason::LaunchUnconfirmed));
                    }
                }
            }
            let now = Instant::now();
            if !pending || now >= end {
                break;
            }
            lifecycle::wait((end - now).min(Duration::from_millis(50)));
        }
        // A client that enters and at once switches or leaves must not read
        // as an unambiguous entry: its next receipt gets one short, bounded
        // moment to arrive before the facts are sealed.
        if self.entries.iter().any(|entry| entry.awaiting) {
            std::thread::sleep(SETTLE.min(end.saturating_duration_since(Instant::now())));
        }
        drop(launching);
        // The receipts end here, for every entry at one instant and before
        // the first check below. Those checks cannot be interrupted, and a
        // lease left armed behind one of them would go on taking receipts
        // after the post share: the confirmation's share would then have
        // lengthened the receipts' own. Two things close them, and neither
        // waits for the other: each launch lease stops taking facts at `end`
        // by the clock (`Lease::launch`), however late this thread gets
        // here; and this seal removes them all together, also when the
        // receipts were complete before `end`. What is confirmed below is
        // confirmed from this set alone.
        #[cfg(test)]
        tests::before_seal();
        let mut sealed = lifecycle::seal_all();
        // The confirmation has a share of its own. The receipts may have
        // been waited for until the post share ended — one entry that never
        // reports must not cost every other entry its confirmation — and
        // the checks below end in validations that cannot be interrupted, so
        // they are bounded here like every other phase: an entry is
        // confirmed only by a check that answered inside this share.
        let confirm_end = end + CONFIRM_SHARE;
        let _confirming = crate::session_runtime::Deadline::until(confirm_end);
        for (index, entry) in self.entries.iter_mut().enumerate() {
            let facts = sealed.remove(&index).unwrap_or_default();
            if entry.slot == Slot::Consumed {
                entry.view.client_observed = entry.new.as_ref().is_some_and(|row| {
                    crate::agent_status::live_generation(row.pane_pid)
                        .is_some_and(|foreground| foreground.pid() != row.pane_pid)
                });
            }
            if !entry.awaiting {
                continue;
            }
            let wanted = entry.final_id.as_deref().unwrap_or_default();
            let entered = match lifecycle::reentry(&facts, wanted) {
                lifecycle::Receipt::Entered(client) => Some(client),
                _ => None,
            };
            // A receipt is history. At completion the client that reported
            // must still be alive and still be the pane's foreground
            // generation: one that already exited or was replaced — even
            // with its end receipt lost — is not shown as an entry.
            let foreground_is = |client: &(u32, Birth)| {
                entry.new.as_ref().is_some_and(|row| {
                    crate::agent_status::live_generation(row.pane_pid)
                        .is_some_and(|foreground| (foreground.pid(), foreground.birth()) == *client)
                })
            };
            let present = entered.filter(&foreground_is);
            // The started client against what this operation asked for,
            // now that it exists (`launch::actual_context`). Whatever the
            // answer, the client is left alone: an unconfirmed entry is a
            // statement about confirmation, not an undo.
            use crate::procinfo::LaunchCheck;
            let checked = present.map(|client| {
                let context = launch::actual_context(
                    env,
                    client,
                    &entry.client.executable,
                    &entry.request,
                    &entry.client.cwd,
                );
                (client, context)
            });
            let reason = match checked {
                // The check took as long as its validations took. A match
                // that came back after the confirmation's share is not a
                // confirmation — the client did start, and its context was
                // not verified in time — and it is not a change either.
                Some((_, LaunchCheck::Matches)) if Instant::now() >= confirm_end => {
                    Some(Reason::ContextUnverified)
                }
                // What was read before that check is not evidence of now:
                // the client must be the pane's foreground generation at the
                // instant the entry is confirmed.
                Some((client, LaunchCheck::Matches)) if !foreground_is(&client) => {
                    Some(Reason::LaunchUnconfirmed)
                }
                Some((_, LaunchCheck::Matches)) => None,
                Some((_, LaunchCheck::Unreadable)) => Some(Reason::ContextUnverified),
                Some((
                    _,
                    LaunchCheck::OtherImage
                    | LaunchCheck::OtherRequest
                    | LaunchCheck::OtherEnvironment
                    | LaunchCheck::OtherDirectory,
                )) => Some(Reason::ContextChanged),
                // no receipt, a conflicting one, or the client is gone
                Some((_, LaunchCheck::Gone)) | None => Some(Reason::LaunchUnconfirmed),
            };
            match reason {
                None => {
                    if entry.bypass {
                        entry.view.warnings.push(Warning::BypassReauthorized);
                    }
                    Self::settle(entry, Outcome::Confirmed, None);
                }
                reason => Self::settle(entry, Outcome::Unconfirmed, reason),
            }
        }
        self.publish();
        progress("reopening", total, total);
    }

    /// One entry up to and including its single transport attempt.
    fn launch(
        &mut self,
        env: &Env,
        index: usize,
        new_server: u32,
        restore_shells: bool,
        end: Instant,
        held: &std::sync::RwLockWriteGuard<'static, ()>,
    ) {
        let entry = &mut self.entries[index];
        let refuse =
            |entry: &mut Entry, outcome, reason| Self::settle(entry, outcome, Some(reason));
        if end.saturating_duration_since(Instant::now()) < ATTEMPT_FLOOR {
            return refuse(entry, Outcome::NotAttempted, Reason::LaunchRefused);
        }
        if !Self::bound(env, entry) {
            return refuse(entry, Outcome::NotAttempted, Reason::IdentityUnavailable);
        }
        // The frozen executable itself, re-read in full: never PATH, never a
        // link that moved, never another build of the same version. Entries
        // that share one binary read it once per post phase; every entry
        // still checks the file's exact state right before its own input.
        let reread = self.reread.contains(&entry.client.executable);
        if !(reread && entry.client.executable.same_state()) {
            // a read that did not finish is not a file that changed
            if let Err(why) = entry.client.executable.unchanged() {
                return refuse(entry, Outcome::NotAttempted, unavailable(why));
            }
        }
        if !reread {
            self.reread.push(entry.client.executable.clone());
        }
        // The read above cannot be interrupted. When it returned late, what
        // is left of the share decides again — before the first side effect
        // of this entry, its shell.
        if end.saturating_duration_since(Instant::now()) < ATTEMPT_FLOOR {
            return refuse(entry, Outcome::NotAttempted, Reason::LaunchRefused);
        }
        if !entry.client.cwd.is_dir() {
            return refuse(entry, Outcome::NotAttempted, Reason::UserActionRequired);
        }
        let session = entry.view.session.clone();
        let listed = |env: &Env| -> Option<Vec<PaneRow>> {
            match env.rows() {
                Ok(rows) => Some(rows),
                // a reachable server with no session has no pane to list
                Err(error) if error.message() == "tmux list-panes failed: no current target" => {
                    Some(Vec::new())
                }
                Err(_) => None,
            }
        };
        // The old server and pane are gone, and no shell of this name exists
        // yet: a shell somebody else created is theirs and is not borrowed.
        let Some(before) = listed(env) else {
            return refuse(entry, Outcome::NotAttempted, Reason::LaunchRefused);
        };
        if before
            .iter()
            .any(|row| row.server_pid != new_server || row.session_name == session)
        {
            entry.view.shell = before.iter().any(|row| row.session_name == session);
            return refuse(entry, Outcome::NotAttempted, Reason::LaunchRefused);
        }
        if crate::procinfo::process_start(entry.client.pid) == Some(entry.client.birth) {
            return refuse(entry, Outcome::NotAttempted, Reason::IdentityUnavailable);
        }
        let created = (env.create_shell)(&session, entry.client.cwd.path(), restore_shells);
        let mine: Vec<PaneRow> = listed(env)
            .unwrap_or_default()
            .into_iter()
            .filter(|row| row.session_name == session)
            .collect();
        entry.view.shell = !mine.is_empty();
        let [row] = mine.as_slice() else {
            return refuse(entry, Outcome::NotAttempted, Reason::LaunchRefused);
        };
        if created.is_err() || row.server_pid != new_server {
            return refuse(entry, Outcome::NotAttempted, Reason::LaunchRefused);
        }
        entry.new = Some(row.clone());
        let Some(shell_birth) = crate::procinfo::process_start(row.pane_pid) else {
            return refuse(entry, Outcome::RefusedBeforeLaunch, Reason::LaunchRefused);
        };
        let ready_end = end.min(Instant::now() + READY_BUDGET);
        let cwd = entry.client.cwd.clone();
        let target = launch::Target {
            row,
            shell_birth,
            cwd: &cwd,
        };
        if !launch::wait_ready(env, &target, ready_end) {
            return refuse(entry, Outcome::RefusedBeforeLaunch, Reason::LaunchRefused);
        }
        // Rebuilt from structured fields: the exact frozen executable, the
        // sealed id, and the override only when `decide` granted it.
        let words = entry
            .final_id
            .as_deref()
            .and_then(|id| (env.capability.request)(id, entry.bypass));
        let line = words
            .as_deref()
            .and_then(|words| launch::command_line(entry.client.executable.path(), words));
        let (Some(words), Some(line)) = (words, line) else {
            return refuse(entry, Outcome::RefusedBeforeLaunch, Reason::LaunchRefused);
        };
        entry.request = words;
        if end.saturating_duration_since(Instant::now()) < ATTEMPT_FLOOR {
            return refuse(entry, Outcome::RefusedBeforeLaunch, Reason::LaunchRefused);
        }
        // The lease takes receipts until the post share ends and not an
        // instant longer, whenever it is sealed.
        let lease = lifecycle::Lease::launch(
            row.server_pid,
            &row.pane_id,
            (row.pane_pid, shell_birth),
            entry.client.executable.clone(),
            end,
        );
        // From here to the end of the transport the Board writer is held
        // out: the card's binding is read under its fence and the input is
        // made before the fence is released, so a delete or rebind either
        // is seen here (nothing is typed) or commits after the input.
        let mut sent = None;
        let mut refused = None;
        (env.fenced)(&mut || {
            if !Self::bound(env, entry) {
                refused = Some(Reason::IdentityUnavailable);
                return;
            }
            if !entry.client.executable.same_state() {
                refused = Some(Reason::ExecutableUnavailable);
                return;
            }
            // CONSUME BEFORE TRANSPORT. From this line on the entry can
            // never send again, whatever the transport reports.
            if entry.slot != Slot::Armed {
                return;
            }
            lifecycle::arm(index, lease.clone());
            entry.slot = Slot::Consumed;
            entry.view.outcome = Outcome::Unconfirmed;
            entry.view.reason = Some(Reason::LaunchUnconfirmed);
            sent = Some(launch::send_once(env, &target, index, &line, held));
        });
        if let Some(reason) = refused {
            return refuse(entry, Outcome::RefusedBeforeLaunch, reason);
        }
        match sent {
            None => {}
            // The transport ran and its guards positively refused the
            // paste: known not to have launched, and not retried.
            Some(launch::Sent::Refused) => {
                lifecycle::seal(index);
                refuse(entry, Outcome::KnownNoLaunchFailure, Reason::LaunchRefused)
            }
            Some(launch::Sent::Unknown) => {
                refuse(entry, Outcome::Unconfirmed, Reason::LaunchUnconfirmed)
            }
            Some(launch::Sent::Submitted) => entry.awaiting = true,
        }
    }
}

impl Drop for Plan {
    fn drop(&mut self) {
        lifecycle::end();
        let mut result = {
            let mut shared = self.shared.lock_or_recover();
            shared.entries = self
                .entries
                .iter()
                .map(|entry| entry.view.clone())
                .collect();
            shared.state = OperationState::Complete;
            for view in &mut shared.entries {
                view.settled = true;
            }
            shared.clone()
        };
        // an entry still awaiting when the plan ends was never confirmed
        for (view, entry) in result.entries.iter_mut().zip(&self.entries) {
            if entry.awaiting {
                view.outcome = Outcome::Unconfirmed;
                view.reason = Some(Reason::LaunchUnconfirmed);
            }
        }
        applog(&format!(
            "[agent-reentry] complete replaced={} entries={} confirmed={}",
            result.server_replaced,
            result.entries.len(),
            result
                .entries
                .iter()
                .filter(|entry| entry.outcome == Outcome::Confirmed)
                .count()
        ));
        with_registry(|registry| {
            registry.active = None;
            registry.finished.push_back((Instant::now(), result));
            while registry.finished.len() > RESULTS_KEPT {
                registry.finished.pop_front();
            }
        });
    }
}

#[cfg(test)]
pub(crate) fn reset_for_tests() {
    lifecycle::end();
    *REGISTRY.lock_or_recover() = None;
}

/// Tests that use the process-wide preview/operation registry and lifecycle
/// mailbox run one at a time.
#[cfg(test)]
pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());
