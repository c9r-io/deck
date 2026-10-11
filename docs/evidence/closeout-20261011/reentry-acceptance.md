# Claude session re-entry: acceptance summary

State of the evidence on 2026-10-11. This file replaces the links tracked
documents used to carry into `reports/`, a git-ignored working directory no
checkout has. It is a summary: the raw logs, screenshots and run records stay
outside the repository.

**Verdict: PASS WITH OPEN QUESTIONS.** The feature's transaction passed its
representative validation on a real Claude Code (2.1.295, a release no
earlier table ever named) through the test controller and through the real
application, under both policies. The installed path — a release build at a
stable location with the global hook file — is verified for the hook
families and their migration, and is **not** verified for a transaction with
a real client. macOS 26, notarization and a published carrier are unverified.

Wording used here, and what it does not mean:

- **confirmed** — the production chain of conditions confirmed that the
  requested client entered the named Claude session. It does not mean the
  task continued, that the model is ready, that a queue was released or that
  Deck verified the effective permission.
- **Policy B result** — "the bypass flag was sent as authorized for this
  restart". Deck does not read Claude's effective permission afterwards.
- **re-entry capability** — one optional capability of the statically
  registered Claude adapter (`docs/first-party-extensions.md`). Nothing here
  is an installable plugin.

## What ran, by kind of evidence

Four kinds, never merged:

| Kind | What it is |
| --- | --- |
| **T** automated tests | The gate's coverage step (`cargo llvm-cov --workspace --all-targets --all-features … -- --test-threads=1`) and the frontend suite, on the commit that carries this file. Stand-in agent, real bundled helper, real bundled tmux. |
| **C** controller + real Claude | A test controller calling the production transaction with real Claude 2.1.295 clients, real home, project-level hooks. 2026-10-11, before this round. Reused, not rerun. |
| **A** real application + real Claude | An ad-hoc signed DEBUG bundle in a private directory, isolated data directory and socket, project-level hooks (the debug-only experiment configuration), real home, operated through its window. 2026-10-11, before this round. Reused, not rerun. |
| **R** release carrier / stable location | This round. A private Developer-ID-signed release carrier and a debug carrier, each at `<home>/Applications/deck.app` under a home relocated into `/private/tmp` on the test machine. No real Claude, no person's home. |

## The transaction (C and A, reused)

Real Claude 2.1.295, macOS 27.0.1, arm64.

| Policy | C: controller calling the production transaction | A: real application, window and IPC |
| --- | --- | --- |
| A (no bypass reapplied) | PASS — one transaction, two cards, no bypass flag in either request; one card resumed the session it was in after `/clear` | PASS — one transaction, one card, no bypass flag |
| B (bypass reapplied for this restart) | PASS — one transaction, three cards; the flag before `--resume` for the two launched with bypass, one of them lowered to plan mode while running; none for the third | PASS — one transaction, three cards, same shape |

Limits recorded with those runs, unchanged:

- Requests and client generations were sampled independently; the exact final
  session id and its attribution also rest on the reviewed production
  confirmation chain. No wire trace was exported.
- The permission indicator after a Policy B re-entry is an operator's reading
  of Claude's own screen, not a Deck verification.
- The application samples had EMPTY queues.
- An earlier Policy B preparation stopped on a tool timeout before any Claude
  was started; it is kept as a stopped preparation, not as a product failure
  and not as a pass.
- The real home changed in ways those reports did not fully attribute
  (Claude's own project records, shell and launcher metadata).

## The dialog in the real window

The `reentry` smoke mode paints the restart dialog from scripted backend
answers in a real WKWebView (debug carrier, test machine, default 1280×800
window): PASS, 15 of 15 checkpoints, once, on 2026-10-11 — 12 asserted
checkpoints, 2 layout metrics that only have to appear, and the terminal one.
It asserts the default policy A with nothing ticked, the Policy B count and
its warning sentence literally in English and Chinese, the exact confirmed-B
result sentence and its detail line, that unconfirmed rows never mention a
flag or a permission, one request with keys `cards, policy, token`, and no
retry control. It starts no Claude and its restart never reaches the backend.
One passing run says nothing about other window sizes, the larger font scale,
a signed carrier or the transaction.

## This round (R)

Source and artifacts are identified in `README.md` of this directory.

### A session that cannot be chosen because observation is off

Found while reading the installed path: with re-entry observation off (the
default) the backend turned every candidate into "unsupported —
re-entry context could not be verified", so the dialog's own "observation is
off, open Settings" hint could never appear against the real backend.
`agent_reentry::read_preview` now keeps such an entry a candidate; the view
says `lifecycleObservation: false`, nothing is selectable, no token is kept,
and an approval for it is still refused (`reentry-preview-expired`). An
unsupported baseline or login shell still makes the entry unsupported.
Pinned by `only_a_direct_client_of_the_product_in_a_supported_launch_shape_is_a_candidate`
(T). Not seen in a release window (no UI operator, below).

### The private release carrier

The tauri bundler's release bundle of the working tree, bundle identifier
changed to `io.c9r.deck.smoke`, signed inside-out with the Developer ID
identity of the build machine, hardened runtime, no secure timestamp.

| Check | Result |
| --- | --- |
| `codesign --verify --strict` on each of the six executables by itself | exit 0 each |
| leaf authority of each is a Developer ID Application certificate; team equals the formally installed Deck's team | yes |
| `codesign --verify --deep --strict` on the bundle | exit 0 |
| control: helper with one changed byte | fails by itself and fails the bundle `--deep`; the bundle NOT `--deep` still exits 0 (a non-deep check does not read a nested helper) |
| `scripts/sidecar_provenance.py check --profile release --bundle` | exit 0: 6 files are this checkout's release build |
| control: a debug adapter put into the carrier | the provenance check fails |
| Gatekeeper, `spctl -a -vv -t exec` | **rejected, source=Unnotarized Developer ID** |
| `xcrun stapler validate` | no ticket |

So: release profile PASS, Developer ID signature PASS, **notarization UNRUN**
(nothing was sent to Apple; it needs credentials and an authorization this
round did not have), Gatekeeper acceptance not claimed, public release not
attempted.

### Launch-time migration, release carrier, no click

`migrate_hooks_on_boot` runs in the application's setup. The carrier was
started through LaunchServices with `HOME` and `TMUX_TMPDIR` pointing into a
private `/private/tmp` root, from `<that home>/Applications/deck.app` — a
stable location by the product's own rule (`stable_installed_bundle`), under
a relocated home. Each scenario wrote one `~/.claude/settings.json` into that
home: three hooks and two keys of "the user's", plus Deck's entries as an
install at `/Applications/deck.app` would have written them.

| Scenario | Before | After one launch | Result |
| --- | --- | --- | --- |
| M1 moved install | lifecycle (2) and status (3) entries naming `/Applications/deck.app` | all five name the helper beside the running carrier; events and arguments unchanged; user hooks and keys unchanged; log `lifecycle entries rewritten to the current spec` | PASS |
| M2 status hooks only, one in the older shell-string form | 3 status entries | 3 status entries in the current form; **no lifecycle entry**; no lifecycle line in the log | PASS |
| M3 user hooks only | — | file byte for byte the same; no hook work logged | PASS |
| M4 same carrier and M1's file, at `<home>/elsewhere/deck.app` | as M1 | byte for byte the same; log `transient release source` | PASS |

### The Settings switches in the real window

`hooks-live` (new smoke mode, debug carrier at `<home>/Applications/deck.app`
under a relocated home, same test machine): the page clicks the two Claude
switches and their confirmations; the operator copies the global settings
file after every checkpoint. `scripts/smoke-verdict … hooks-live`: PASS, 8
of 8.

| Step | File afterwards |
| --- | --- |
| open Settings; decline the re-entry question | byte for byte the user's file |
| re-entry observation ON (confirmed) | + exactly `SessionStart` and `SessionEnd` entries of `deck-lifecycle-helper` (this carrier's path, args `claude-code session-start` / `session-end`, timeout 1); no status entry; status switch still off |
| status hooks ON (confirmed) | + `UserPromptSubmit`, `Notification` (matcher `permission_prompt`), `Stop` entries of `deck-status-helper`; lifecycle entries unchanged |
| re-entry observation OFF (asks nothing) | lifecycle entries gone; status entries unchanged |
| close and reopen Settings | switches show what the file says |
| status hooks OFF | byte for byte the user's file |

The user's three hooks and two other keys were identical at every step.

### Identity on this OS

The identity mechanism has no version conditional (`procinfo/code_identity.rs`
has only `cfg(target_os = "macos")`). Its tests run in the gate on macOS
27.0.1 (this round) with C fixtures signed at test time, `/bin/ls` as an
Apple-signed negative and the test binary itself. **macOS 26: UNRUN** — the
two machines available are 27.0.1; the gate's `macos-26` runner needs a push,
which this round was not authorized to make.

## The eleven acceptance items

`T` `C` `A` `R` as above. "formal" = a release build at
`/Applications/deck.app` with the person's real home and global hook file.

| # | Item | Evidence | Formal |
| --- | --- | --- | --- |
| 1 | Global lifecycle hook opt-in / opt-out in Settings | T; **R real window PASS** (debug carrier, stable location under a relocated home) | BLOCKED |
| 2 | Upgrade or path migration of existing lifecycle hooks | T (edit engine); **R release carrier PASS** (M1, M2 older form, M4) | BLOCKED at `/Applications` |
| 3 | User hooks intact; the two switches independent | T; **R PASS** (every step and scenario) | BLOCKED |
| 4 | A user without lifecycle hooks is not enrolled by migration | T; **R release carrier PASS** (M2, M3) | BLOCKED at `/Applications` |
| 5 | Policy A sends no bypass | T; C PASS; A PASS | UNRUN |
| 6 | Policy B sends bypass only to chosen, eligible launches, including bypass-then-plan; accurate wording | T; C PASS; A PASS; wording in the real window (scripted) PASS | UNRUN |
| 7 | The exact final session, also after `/clear` | T; C PASS (one card after `/clear`); A PASS without `/clear` | UNRUN |
| 8 | One start per entry; an unknown outcome is not resent | T (`unknown_outcomes_are_never_retried` and the source scan in `tests/external_admission.rs`); no second request in C or A | UNRUN |
| 9 | A non-empty queue stays paused across restart and re-entry | T (`a_waiting_queue_row_is_paused_on_disk_before_the_kill_and_stays_paused_after_reentry`, crash matrix). The earlier acceptance records a controller sample with a non-empty paused queue; it was not re-examined this round | **UNRUN in any real application** (A had empty queues) |
| 10 | The next preview and a relaunch do not inherit Policy B | T (frontend: a new preview always starts at A with nothing ticked; backend: the lifecycle document holds no policy). Real file: the application log of A's Policy B run holds one line naming bypass, a count (`frozen entries=3 eligible=3 bypass=2`), and no policy word | UNRUN: a second preview and a relaunch after B in a real application |
| 11 | Unsupported and unconfirmable cases said as they are; a restarted service not reported as a failure | T; the observation-off presentation was wrong and is fixed (above); result title separates "Shell service restarted" from per-card rows (T, scripted window) | UNRUN |

Where the policy could be stored, checked against the code rather than
against a file that does not exist: a restart with re-entry writes
`tmux-lifecycle.json` (restart intent: pids, socket identity, counts),
`queue.json` (paused flags), `deck.json` (`launched`), `app.log` (counts and
closed codes) and, with shell recovery on, `shell-state/`. The policy arrives
inside one request, is read once and is kept in no document; results live in
process memory for at most ten minutes. An earlier "persistence probe" that
looked at a settings file the isolated root never had is not counted.

## Why the formal path is blocked

A release build ignores every isolation argument: it uses `~/.deck`, the tmux
socket `deck` and the global `~/.claude/settings.json`, and a transaction
needs a real, signed-in Claude under that same home.

- Both machines available have the owner's Deck installed at
  `/Applications/deck.app` and in use; replacing it means ending its sessions
  and parking its data, which is the owner's decision.
- The test machine's instructions forbid an executable under its real home,
  so `~/Applications/deck.app` is not available there either, and its Claude
  was not signed in when last asked.
- A relocated home (this round's method) gives the release build a stable
  location and its own files, but a real Claude under it has no account.
- A release build has no smoke driver, and this round had no desktop UI
  tool: its switches and dialog need a person or a UI operator.

## Accepted residual risks

A silent change of meaning in a native field of unchanged shape; the window
between validation and execution; the availability cost of Security calls
that cannot be cancelled. None restores a version list.
