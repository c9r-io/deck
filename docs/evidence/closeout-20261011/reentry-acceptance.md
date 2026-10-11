# Claude session re-entry: acceptance summary

State of the evidence on 2026-10-11. This file replaces the links tracked
documents used to carry into `reports/`, a git-ignored working directory no
checkout has. It is a summary: the raw logs, screenshots and run records stay
outside the repository.

**Verdict: PASS WITH OPEN QUESTIONS.** The feature's transaction passed its
representative validation on a real Claude Code (2.1.295, a release no
earlier table ever named) through the test controller and through the real
application, under both policies. In a follow-up the same day it passed
again on Claude Code 2.1.296 in a real window at a stable location, with the
hook file of the home it ran under and no debug exemption, both policies, a
queued prompt and a relaunch in between (kind **L** below). The installed
path proper — a RELEASE build at `/Applications/deck.app` — is verified for
the hook families and their migration, and is **not** verified for a
transaction with a real client. macOS 26, notarization and a published
carrier are unverified.

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
| **L** live window + real Claude | This round's follow-up. A DEBUG carrier (ad-hoc signed) at `<home>/Applications/deck.app` under a relocated home on the test machine (macOS 27.0.1), which is a stable location by the product's own rule: the hooks are that home's `.claude/settings.json`, written by the Settings switches, with no project-level file and no debug experiment configuration. Real Claude Code 2.1.296, signed in under that home by the owner. The page clicks the real dialog (`reentry-live`, `reentry-live-relaunch` in `app/SMOKE.md`); a runner outside reads the process table. Not a release build, not `/Applications`, not a person's home. |

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

## The transaction in the real window with real clients (L)

One run, 2026-10-11 18:44–18:45, about 85 seconds, `reentry-live` 14 of 14
checkpoints and `reentry-live-relaunch` 5 of 5 (`scripts/smoke-verdict`, both
exit 0), and every check of the runner's own judgement held. An earlier
attempt the same evening ended on a wrong expectation of the scenario (a card
running the system `top`, see below); its transaction had gone through the
same way and it is not counted.

Six cards, started by the product's own new-session path: E and A run
`claude`, B and C run `claude --dangerously-skip-permissions`, D runs `cat`,
T runs `top`. Each client answered one one-word prompt; A then got `/clear`
and a second prompt; C was lowered to plan mode with Shift+Tab. One prompt
was queued on A with a "not before" a day away.

| Step | What the window said | What the process table and the files said |
| --- | --- | --- |
| Observation off, one client running | the review names the setting and offers nothing to tick (`relive-hint`) | log: `preview sessions=1 candidates=1` |
| Both hook switches on | both boxes on, both questions asked and confirmed | the settings file keeps the client's own two keys; every hook command names this carrier's helpers |
| Review of restart 1 | E, A, B, C can be ticked; D: "Reopen this Agent manually."; T (a program of another user, unreadable to Deck): "The session's re-entry context could not be verified."; ordinary policy, nothing ticked | `preview sessions=6 candidates=4` |
| Restart 1, everything ticked, **Policy B** | "Shell service restarted"; A re-entered, no flag; B and C re-entered, "the bypass flag was sent as authorized", "Deck did not independently verify…"; queues remain paused | before: `claude`, `claude`, `claude --dangerously-skip-permissions` ×2. After: A `<absolute program> --resume <id>`; B and C `<absolute program> --dangerously-skip-permissions --resume <id>`; each id is the newest session of that card's directory, and A's is the second of its two sessions (the one after `/clear`). Log: `frozen entries=4 eligible=4 bypass=2`, `sealed final=4 of=4`, `queue-paused count=1`, four `outcome=confirmed`, `complete replaced=true entries=4 confirmed=4` |
| The next review | ordinary policy, nothing ticked | — |
| Application quit and started again | the review again starts from the ordinary policy with nothing ticked | the clients kept running on the same tmux server |
| Restart 2, everything ticked, **Policy A** | all re-entered, no row speaks of a flag | A, B and C each one new client, `--resume <id>` and no bypass flag (B and C had carried it since restart 1). Log: `frozen entries=4 eligible=4 bypass=0`, four `outcome=confirmed` |
| The queued prompt | still one row, paused | `queue.json`: one item, `paused: true`, never attempted, no delivery record; no send in the log; `queue-paused count=1` at restart 1 and `0` at restart 2 |
| The policy | — | no document of the data directory holds it; the log holds the two counts above |

What this run is and is not:

- It is the product's stable-location path: the carrier sits where the
  product accepts an installation, the hooks are in the hook file of the home
  it runs under, and nothing of the debug experiment configuration is used.
  It is still a DEBUG build, ad-hoc signed, under a relocated home; it says
  nothing about a release build, a Developer ID signature or `/Applications`.
- E's client was started before the hooks existed and was re-entered all the
  same: Claude Code 2.1.296 runs hooks added after it started. Recorded, not
  a requirement.
- Each entry got one result line per restart and one new client; no restart
  had an unknown outcome, so "an unknown outcome is not resent" is still
  shown by the tests only.
- The queued prompt could not have been sent in this window in any case (its
  own "not before"); what the run shows is the pause the restart put on it
  and that two restarts and a relaunch released nothing.
- "Re-entered" is the client entering the session. Nothing was typed into a
  re-entered client and nothing here says its work continued.
- The clients ran on the owner's account: twelve one-word prompts in all,
  including calibration and the first attempt.

How the client was signed in under a relocated home, because it is not
obvious: Claude Code keeps its sign-in in the keychain and does not fall back
to a file, and a relocated home has no keychain. The run gave that home a
keychain of its own (not named `login.keychain-db`: a file of that name
cannot be unlocked with its own password), unlocked in the GUI session; the
owner authorized the sign-in in a browser, twice, because the first save
found no keychain. The item first landed under the account name `unknown`
(the sign-in shell had no `USER`) and was copied to the login user's name
inside that keychain. The machine's own keychain list and files were read
before and after and did not change.

## The eleven acceptance items

`T` `C` `A` `R` as above. "formal" = a release build at
`/Applications/deck.app` with the person's real home and global hook file.

| # | Item | Evidence | Formal |
| --- | --- | --- | --- |
| 1 | Global lifecycle hook opt-in / opt-out in Settings | T; **R real window PASS** (debug carrier, stable location under a relocated home) | BLOCKED |
| 2 | Upgrade or path migration of existing lifecycle hooks | T (edit engine); **R release carrier PASS** (M1, M2 older form, M4) | BLOCKED at `/Applications` |
| 3 | User hooks intact; the two switches independent | T; **R PASS** (every step and scenario) | BLOCKED |
| 4 | A user without lifecycle hooks is not enrolled by migration | T; **R release carrier PASS** (M2, M3) | BLOCKED at `/Applications` |
| 5 | Policy A sends no bypass | T; C PASS; A PASS; **L PASS** (after a relaunch, on clients that had carried the flag) | BLOCKED |
| 6 | Policy B sends bypass only to chosen, eligible launches, including bypass-then-plan; accurate wording | T; C PASS; A PASS; **L PASS** (flag on B and C, not on A; the real result rows) | BLOCKED |
| 7 | The exact final session, also after `/clear` | T; C PASS (one card after `/clear`); A PASS without `/clear`; **L PASS** (A resumed in the second of its two sessions) | BLOCKED |
| 8 | One start per entry; an unknown outcome is not resent | T (`unknown_outcomes_are_never_retried` and the source scan in `tests/external_admission.rs`); no second request in C or A; **L**: one result line and one new client per entry in both restarts. No real run had an unknown outcome | BLOCKED |
| 9 | A non-empty queue stays paused across restart and re-entry | T (`a_waiting_queue_row_is_paused_on_disk_before_the_kill_and_stays_paused_after_reentry`, crash matrix); **L PASS** (one queued prompt, paused by restart 1, still paused and unsent after restart 2 and a relaunch) | BLOCKED |
| 10 | The next preview and a relaunch do not inherit Policy B | T; **L PASS** (the review after Policy B, and the review after a relaunch, both start from the ordinary policy with nothing ticked; no document holds the policy) | BLOCKED |
| 11 | Unsupported and unconfirmable cases said as they are; a restarted service not reported as a failure | T; the observation-off presentation was wrong and is fixed (above); **L PASS** for the cases it staged (observation off; a foreground program that is no Agent; a program Deck cannot read; "Shell service restarted" above the rows). No real run staged a timeout or an unconfirmed entry | BLOCKED |

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
- A relocated home (this round's method) gives a build a stable location and
  its own files. A real Claude under it has no account until someone signs
  it in there, which the owner did for the follow-up (kind L).
- A release build has no smoke driver, and this round had no desktop UI
  tool: its switches and dialog need a person or a UI operator. That is why
  the follow-up's transaction ran in a debug carrier, and why the release
  build's own transaction is still open. A private Developer-ID-signed
  release carrier that relocates itself through its own `LSEnvironment` was
  built, placed on the test machine and checked to stay inside its directory
  when opened like any application; nobody operated it and it was removed.

## Accepted residual risks

A silent change of meaning in a native field of unchanged shape; the window
between validation and execution; the availability cost of Security calls
that cannot be cancelled. None restores a version list.
