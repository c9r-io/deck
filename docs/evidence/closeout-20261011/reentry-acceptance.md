# Claude session re-entry: acceptance summary

State of the evidence on 2026-10-11. This file replaces the links tracked
documents used to carry into `reports/`, a git-ignored working directory no
checkout has. It is a summary: the raw logs, screenshots and run records stay
outside the repository.

**Verdict: PASS WITH OPEN QUESTIONS, for what ran.** The feature's
transaction passed its representative validation on a real Claude Code
(2.1.295, a release no earlier table ever named) through the test controller
and through the real application, under both policies: a debug bundle in a
private directory, project-level hooks. In a follow-up the same day it passed
again on Claude Code 2.1.296 in a real window: a DEBUG carrier at a stable
location under a relocated home, with the hook file of that home and no debug
exemption, both policies, a queued prompt and a relaunch in between (kind
**L** below). Under a relocated home the hook families were exercised too:
the Settings switches in a debug carrier, the launch-time migration in a
Developer-ID-signed RELEASE carrier. Last, the transaction ran once in such
a release carrier under a relocated home, with the owner operating its
window (kind **Lr**): Policy B, then the ordinary policy after the
application had been started again.

What did not run, each a separate item below: **anything at
`/Applications/deck.app`**; the Settings switches in a release window; on
macOS 26, native identity of a real client and a transaction in a real
window (the automated gate did run there, in CI); notarization and
Gatekeeper acceptance.

Three places are kept apart throughout (`README.md`, "Scope words"): a
**private directory**; a **relocated home**, `<home>/Applications/deck.app`
with `<home>` under `/private/tmp` — a stable location by the product's own
rule, but not a person's home; and **`/Applications/deck.app`**, the
installed path, with a person's home, their real hook file and their login
keychain.

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

Seven kinds, never merged. The place and the build are part of each.

| Kind | Place | Build | What it is |
| --- | --- | --- | --- |
| **T** automated tests | — | test binaries | The gate's coverage step (`cargo llvm-cov --workspace --all-targets --all-features … -- --test-threads=1`) and the frontend suite, on the commit that carries this file. Stand-in agent, real bundled helper, real bundled tmux. |
| **C** controller + real Claude | — | test controller | A test controller calling the production transaction with real Claude 2.1.295 clients, real home, project-level hooks. 2026-10-11, before this round. Reused, not rerun. |
| **A** real application + real Claude | private directory | debug, ad-hoc signed | Isolated data directory and socket, project-level hooks (the debug-only experiment configuration), real home, operated through its window. 2026-10-11, before this round. Reused, not rerun. |
| **Rr** release carrier | relocated home | RELEASE, Developer ID signed, not notarized | This round. The launch-time migration, with no click. No real Claude, no window operation. |
| **Rd** debug carrier, the Settings switches | relocated home | debug, ad-hoc signed | This round. The page clicks the two Claude switches in the real window (`hooks-live`). No real Claude. |
| **L** live window + real Claude | relocated home | debug, ad-hoc signed | This round's follow-up, on the test machine (macOS 27.0.1). The hooks are that home's `.claude/settings.json`, written by the Settings switches, with no project-level file and no debug experiment configuration. Real Claude Code 2.1.296, signed in under that home by the owner. The page clicks the real dialog (`reentry-live`, `reentry-live-relaunch` in `app/SMOKE.md`); a runner outside reads the process table. |

| **Lr** release window + real Claude | relocated home | RELEASE, Developer ID signed, not notarized | The final round, on the test machine (macOS 27.0.1). The hooks are that home's `.claude/settings.json`, as the carrier's launch-time migration rewrote them. Real Claude Code 2.1.296, signed in under that home by the owner. A PERSON, the owner, made the cards, queued a prompt and operated the restart review twice; a runner outside typed into the cards through the carrier's tmux server, read the process table, and ended and reopened the application once. No smoke driver exists in a release build. |

No kind ran at `/Applications/deck.app`.

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

## This round (Rr, Rd)

Source and artifacts are identified in `README.md` of this directory.

### A session that cannot be chosen because observation is off

Found while reading the code behind the Settings switch: with re-entry
observation off (the default) the backend turned every candidate into "unsupported —
re-entry context could not be verified", so the dialog's own "observation is
off, open Settings" hint could never appear against the real backend.
`agent_reentry::read_preview` now keeps such an entry a candidate; the view
says `lifecycleObservation: false`, nothing is selectable, no token is kept,
and an approval for it is still refused (`reentry-preview-expired`). An
unsupported baseline or login shell still makes the entry unsupported.
Pinned by `only_a_direct_client_of_the_product_in_a_supported_launch_shape_is_a_candidate`
(T). Seen in the real window of a debug carrier in the follow-up (L,
`relive-hint`); not seen in a release window (no one to operate it, below).

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

Four properties, none standing for another:

| Property | Status | Basis |
| --- | --- | --- |
| Release profile | **PASS** | the provenance check: all six executables are this checkout's release build; the executable names no `ui/test` asset |
| Developer ID signature | **PASS** | each executable verifies strictly by itself, the bundle verifies deep and strict, the leaf authority is a Developer ID Application certificate of the installed Deck's team; hardened runtime; signed on the build machine without a secure timestamp |
| Notarization | **UNRUN** | nothing was sent to Apple; it needs credentials and an authorization this round did not have; no ticket is stapled |
| Gatekeeper | **assessed: rejected** ("Unnotarized Developer ID") | `spctl`. Acceptance is UNRUN. The carrier ran on the test machine all the same because a file copied over SSH carries no quarantine attribute: no launch in this document was a Gatekeeper decision |

A public release was not attempted. The follow-up's live run (L) used a debug
carrier with an ad-hoc signature: none of the four rows is about it. The
final round's release carrier (Lr) was built from `18e38bb` and signed the
same way; the same checks gave the same four answers for it.

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

### Identity, and macOS 26 as three separate items

The identity mechanism has no version conditional (`procinfo/code_identity.rs`
has only `cfg(target_os = "macos")`). On macOS 27.0.1 its tests ran in the
gate (C fixtures signed at test time, `/bin/ls` as an Apple-signed negative
and the test binary itself), and it admitted real Claude Code 2.1.295 and
2.1.296 clients in the runs above. Both machines available are 27.0.1.
macOS 26 is three items, not one, and only the first has run:

| Item on macOS 26 | Status | What would close it, and what would not |
| --- | --- | --- |
| The automated gate | **PASS** | CI run 38139504866 on `2c2a455`, after the owner authorized the push: `checks`, `rust` and `release` on hosted `macos-26` runners. It closes what those jobs executed: the test suites (81 `agent_reentry` tests and the identity mechanism's own tests with their test-time fixtures among them), clippy, the release build and its checks. The first run failed because the hosted runner's account has bash as its login shell and the re-entry tests run the production assembly, which supports zsh; the gate now gives the account the macOS default first (`README.md`, "CI on the pushed commits"). It closes nothing below |
| Native identity of a real client | **BLOCKED** | the production check, Security.framework validating a running Claude Code image and its file against the product's requirement, on macOS 26. The gate runs no real client. It needs a macOS 26 machine with Claude Code on it |
| The transaction in a real window with a real client | **BLOCKED** | no macOS 26 machine. The debug-carrier run of kind L, on such a machine |

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

One column per place and build. "—" means that kind of evidence does not
speak to the item; UNRUN means nothing stood in the way but it was not run;
BLOCKED means something outside the code stands in the way.

| # | Item | Tests (T) | Private directory, debug (C, A) | Relocated home, DEBUG carrier (Rd, L) | Relocated home, RELEASE carrier (Rr) | `/Applications/deck.app`, release |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | Global lifecycle hook opt-in / opt-out in Settings | pass | — (project-level hooks there) | **PASS** (Rd: 8 of 8; L switched both on through the page) | UNRUN: Lr used the migration, not the switch | BLOCKED |
| 2 | Upgrade or path migration of existing lifecycle hooks | pass (edit engine) | — | not run | **PASS** (Rr: M1, M2 older form, M4; Lr: entries naming another installation rewritten at launch, then used by real clients) | BLOCKED |
| 3 | User hooks intact; the two switches independent | pass | — | **PASS** (every step of Rd; L kept the client's own keys) | **PASS** for the migration (Rr: user hooks and keys unchanged in M1–M4; Lr: the client's own keys unchanged); the switches UNRUN | BLOCKED |
| 4 | A user without lifecycle hooks is not enrolled by migration | pass | — | not run | **PASS** (Rr: M2, M3) | BLOCKED |
| 5 | Policy A sends no bypass | pass | C PASS; A PASS | **L PASS** (after a relaunch, on clients that had carried the flag) | **Lr PASS** (after the application was started again; b had carried the flag since restart 1) | BLOCKED |
| 6 | Policy B sends bypass only to chosen, eligible launches, including bypass-then-plan; accurate wording | pass | C PASS; A PASS | **L PASS** (flag on B and C, not on A; the real result rows) | **Lr PASS** for what was sent (flag on b, not on a; `bypass=1`). No bypass-then-plan card in Lr; the wording was read by the owner, not recorded | BLOCKED |
| 7 | The exact final session, also after `/clear` | pass | C PASS (one card after `/clear`); A PASS without `/clear` | **L PASS** (A resumed in the second of its two sessions) | **Lr PASS** (a resumed in the second of its two own sessions; b in its one) | BLOCKED |
| 8 | One start per entry; an unknown outcome is not resent | pass (`unknown_outcomes_are_never_retried` and the source scan in `tests/external_admission.rs`) | no second request in C or A | **L**: one result line and one new client per entry in both restarts | **Lr**: one result line and one new client per entry in both restarts | BLOCKED |
| 9 | A non-empty queue stays paused across restart and re-entry | pass (`a_waiting_queue_row_is_paused_on_disk_before_the_kill_and_stays_paused_after_reentry`, crash matrix) | — (A's queues were empty) | **L PASS** (one queued prompt, paused by restart 1, still paused and unsent after restart 2 and a relaunch) | **Lr PASS** (one queued prompt, `queue-paused count=1` at restart 1, still paused, never attempted and unsent after restart 2) | BLOCKED |
| 10 | The next preview and a relaunch do not inherit Policy B | pass | — | **L PASS** (the review after Policy B, and the review after a relaunch, both start from the ordinary policy with nothing ticked; no document holds the policy) | **Lr PASS** across a restart of the application (the review was left as it came up and the transaction carried `bypass=0`; no document holds the policy). The review directly after Policy B was not opened | BLOCKED |
| 11 | Unsupported and unconfirmable cases said as they are; a restarted service not reported as a failure | pass; the observation-off presentation was wrong and is fixed (above) | — | **L PASS** for the cases it staged (observation off; a foreground program that is no Agent; a program Deck cannot read; "Shell service restarted" above the rows) | not staged in Lr (both restarts: `finish result=ok`, every entry confirmed) | BLOCKED |

In no place and no build did a real run have an unknown outcome, a timeout
or an unconfirmed entry: for item 8's second half and for those cases of
item 11 the tests are the only evidence.

The release column's Lr entries are one run, described under "The
transaction in a release carrier (Lr)". What its window said was read by the
owner and not recorded; the entries rest on what was sent and what ran.

Where the policy could be stored, checked against the code rather than
against a file that does not exist: a restart with re-entry writes
`tmux-lifecycle.json` (restart intent: pids, socket identity, counts),
`queue.json` (paused flags), `deck.json` (`launched`), `app.log` (counts and
closed codes) and, with shell recovery on, `shell-state/`. The policy arrives
inside one request, is read once and is kept in no document; results live in
process memory for at most ten minutes. An earlier "persistence probe" that
looked at a settings file the isolated root never had is not counted.

## Why nothing ran at `/Applications/deck.app`

A release build ignores every isolation argument: it uses `~/.deck`, the tmux
socket `deck` and the global `~/.claude/settings.json`, and a transaction
needs a real, signed-in Claude under that same home.

- Both machines available have the owner's Deck installed at
  `/Applications/deck.app` and in use; replacing it means ending its sessions
  and parking its data, which is the owner's decision.
- The test machine's instructions forbid an executable under its real home,
  so `~/Applications/deck.app` is not available there either.
- A relocated home (this round's method) gives a build a stable location and
  its own files without touching either. It is a different place, and its
  results are recorded as such.

## The transaction in a release carrier (Lr)

One run in the final round, 2026-10-11 20:54–21:00, after the cleanup
mechanism was in place (`README.md`, "Final bounded round"). Every check of
the runner's judgement held.

It could not be run from a build session alone. A release build has no smoke
driver, so its switches and its dialog need clicks, and nothing available
there can make them: asked without prompting, `AXIsProcessTrusted`,
`CGPreflightPostEventAccess`, `CGPreflightListenEventAccess` and
`CGPreflightScreenCaptureAccess` all answered false, on the build machine
and over SSH on the test machine. The owner operated the window and asked
for that part to be as small as it could be. Who did what:

**The carrier (built, signed and placed from the build machine).**

1. The release profile of commit `18e38bb`: `cargo build --release --locked`,
   then `tauri build --bundles app` with updater artifacts off, then
   `scripts/sidecar_provenance.py check --profile release --bundle`.
2. A private bundle identifier, and an `LSEnvironment` of three values in
   its `Info.plist`: `HOME=<root>/home`, `TMUX_TMPDIR=<root>/t`, and
   `DISABLE_AUTOUPDATER=1` for the clients its panes start. Signed inside-out
   with a Developer ID identity (LaunchServices refuses `LSEnvironment` in an
   unsigned bundle) and verified piece by piece there and on the test
   machine. The executables are the release build's, unchanged; this is not
   the published bundle and not a switch in the product.
3. Placed at `<root>/home/Applications/deck.app` on the test machine, where
   `<root>` is a short directory under `/private/tmp` with a registry
   (`scripts/edr_runtime.py experiment init`). One background trial launch:
   the environment read from the kernel was those three values, the data and
   socket were inside the root, and it was stopped through the registry.

**The client.**

4. A keychain of its own for that home (not named `login.keychain-db`; the
   home needs a `Library/Preferences` directory for the search list to be
   saved), unlocked in the GUI session; the client's sign-in in a shell that
   has `USER` and `LOGNAME`. **The owner authorized it in a browser.**
5. Trust for the two working directories and the bypass warning, accepted
   once in a client outside the carrier, which was then closed.

**Observation.**

6. The hook file was given the two lifecycle entries as an installation at
   `/Applications/deck.app` would have left them. The carrier's next launch
   logged `lifecycle entries rewritten to the current spec` and the file then
   named this carrier's helper; the client's own two keys were unchanged. No
   client ran between the two. The Settings switch was not used.

**The window: the owner, four steps.**

7. Two new cards.
8. One prompt queued in a card's Lists panel, "not before" the next day.
9. Settings → Terminal & sessions → "Shell service" → "Restart service"; in
   the review "Select all candidates", "Reapply bypass to 1 session(s)
   launched with bypass", then "Restart and reopen 2 session(s)".
10. After the application had been started again: the same restart with the
    policy left as the review showed it.

**From outside, between those steps.** A runner typed into the two cards
through the carrier's own tmux server, by its full socket path, what a
person would type there: `cd ~/work/a && claude` and, in the other,
`cd ~/work/b && claude --dangerously-skip-permissions`; one one-word prompt
in each; in the first then `/clear` and a second prompt. It looked at the
pane before every keystroke, typed only at a shell prompt or at the client's
ready screen, and answered no dialog. After restart 1 it ended the
application through the registry and opened the carrier again, once. It kept
the registry current: each application process and each of the three server
generations was registered when first seen.

| Step | Who | What the process table, the session files and the carrier's own log said |
| --- | --- | --- |
| Observation on | the carrier's launch | the hook file had been given the two lifecycle entries as an installation at `/Applications/deck.app` would have left them; the launch logged `lifecycle entries rewritten to the current spec` and the file then named this carrier's helper. The client's own two keys were unchanged. No client ran in between |
| Two cards | the owner, in the window | two sessions on the carrier's server |
| Clients and prompts | the driver, through the carrier's tmux server | card a: `claude`, one prompt, `/clear`, one prompt: two sessions of its own. Card b: `claude --dangerously-skip-permissions`, one prompt: one session |
| A queued prompt, "not before" the next day | the owner, in the window | `queue.json`: one item, pending |
| Review and restart 1, **Policy B** | the owner, in the window | log: `preview sessions=2 candidates=2`, `frozen entries=2 eligible=2 bypass=1`, `sealed final=2 of=2`, `queue-paused count=1`, two `outcome=confirmed`, `complete replaced=true entries=2 confirmed=2`, `finish result=ok` after 4.0 s. After: a `<absolute program> --resume <id>`; b `<absolute program> --dangerously-skip-permissions --resume <id>`. Each id is the newest session of that card's directory, and a's is the second of its two (the one after `/clear`) |
| The application ended and started again | the driver: stopped through the registry, one LaunchServices open | another process; the same server and the same two clients |
| Review and restart 2, **the ordinary policy**, left as the review showed it | the owner, in the window | log: `preview sessions=2 candidates=2`, `frozen entries=2 eligible=2 bypass=0`, `queue-paused count=0`, two `outcome=confirmed`, `complete replaced=true entries=2 confirmed=2`. After: a and b each one new client, `--resume <id>`, and neither carries the bypass flag |
| The queued prompt | — | still one item: `paused: true`, `attempts: 0`, no delivery record; no send in the log |
| The policy | — | no document of the data directory holds it; the log holds the two counts |

Read from outside, typing into nothing after the prompts: the process table
(each pane's client and its arguments), the names of the session files per
directory, the carrier's own `app.log`, and its documents. Card a's
directory also held the sessions of the two clients that had run there
before the cards existed (the acceptance of the trust question, and the
GUI-session check); card a's own two are the ones made after the runner
began, and it was resumed in the later of them.

What this run is and is not:

- A RELEASE build, Developer ID signed, at a stable location under a
  relocated home, with that home's hook file and a real, signed-in client.
  Not `/Applications/deck.app`, not a person's home, not notarized; no
  Gatekeeper decision was involved.
- The typing came from outside, not through the window. The clients are
  direct children of the cards' shells either way.
- The Settings switch was not used; observation came from the migration.
- What the review and the result rows said was the owner's reading and was
  not recorded. Nothing was typed into a re-entered client, and nothing here
  says its work continued.
- The review directly after Policy B was not opened before the application
  was started again.
- The clients ran on the owner's account: four one-word prompts in all.

The end: everything registered was stopped through the registry (exit 0),
the client signed out, the keychain deleted, the root removed (not moved to
the Trash: it held a credential store). The test machine's installed Deck,
its files, the hook file digest and the keychain list were the same before
and after.

## Accepted residual risks

A silent change of meaning in a native field of unchanged shape; the window
between validation and execution; the availability cost of Security calls
that cannot be cancelled. None restores a version list.
