# Close-out evidence, 2026-10-11

Work on three things, on top of `f1cad64`, and where each stands. The state
is the list of separate acceptance items in "Verdict". Several of them are
BLOCKED or UNRUN, so nothing here says that the three are finished.

- **A. Shared build outputs** — the sidecar pollution: a coverage run left
  instrumented helpers in `binaries/` and an ordinary build went on packaging
  them. Fixed, with standing checks; two further gaps in the same area were
  closed later the same day.
- **B. The first-party Slack integration in the real window** — a sandbox
  Slack app, its real Socket Mode connection and real messages, through the
  registered integration, in a real WKWebView of a debug carrier.
- **C. Claude session re-entry** — the hook families, their migration and
  the transaction itself, in carriers under a relocated home: in debug
  carriers, and once, at the end, in a release carrier with the owner at the
  window. Nothing ran at `/Applications/deck.app`. Its acceptance summary is
  [reentry-acceptance.md](reentry-acceptance.md).

Three rounds are recorded, each marked where it matters:

- **the round** (13:16–14:45): A, B and C as first run;
- **the follow-up** the same evening: the owner asked whether the change was
  complete (one more gap in A was found and closed, and the committed tree
  passed the gate from a clean worktree), then asked for the re-entry
  transaction to be run without a person at the window (done in a debug
  carrier with real clients);
- **the final bounded round** (from 19:26): the verification rules of A, a
  check of the package that actually runs, the incident's guard as a tracked
  mechanism, and the scope of this document, which had claimed more than ran
  in several places ("What this document claimed too widely"); then the
  transaction in a release carrier, with the owner operating its window.

This is a summary a clean checkout can read. Raw logs, fetched documents and
run scripts are in `reports/closeout-20261011/`, a git-ignored working
directory that no checkout has; nothing below depends on opening it. No
credential, token, account name or message body is recorded here.

Statuses: **PASS** ran and held; **FAIL** ran and did not hold; **BLOCKED**
could not run for a named reason outside the code; **UNRUN** was not run.
"This round" means executed on 2026-10-11 for this document; "reused" means
evidence from an earlier round that was not executed again.

## Scope words

- The extension architecture is **static and first-party**: three tables
  compiled into the application (`extensions.rs`: one Slack service
  integration, the Claude Code and Codex agent adapters, the Bergamot
  translation provider). Registered does not mean enabled, connected,
  loaded, trusted or authorized. Nothing here is a plugin that can be
  installed, and no dynamic loading exists.
- Re-entry is one optional capability of the Claude adapter. "Confirmed"
  means the client entered the named session — not that a task continued.
- Missing evidence is written as BLOCKED or UNRUN, never as a defect of the
  feature, and an ad-hoc signature is never written as a Developer ID one.
- Three places a build ran from, or did not, are kept apart:
  - a **private directory**: a debug bundle in a build or temporary
    directory with a smoke data directory. Not a stable location;
  - a **relocated home**: `<home>/Applications/deck.app`, where `<home>` is
    a directory under `/private/tmp` handed to the process as `HOME`. That
    is a stable location by the product's own rule
    (`stable_installed_bundle`), and that home's `.claude/settings.json` is
    the global hook file the product edits. It is not a person's home and
    it is not `/Applications`;
  - **`/Applications/deck.app`**: the installed path, with a person's home,
    their real hook file and their login keychain. **Nothing in this
    document ran there.**
- Four properties of a carrier are kept apart: its build profile (debug or
  release), its signature (ad-hoc or Developer ID), notarization, and
  Gatekeeper's verdict. One of them never stands for another.
- A green CI run closes the items its jobs execute and no other.

## Source and artifacts

| | |
| --- | --- |
| Base commit | `f1cad6466da8b56035a97f0a7d138ad0cd0a746b` (`main`, clean at the start) |
| Commits | the round: `d54d7ce`, `06fb0b5`, `02fa839`, `b7518af`, `e25774d`. The follow-up: `9e0bad3`, `c9d78f5`, `0aeee77`. The final round: `8dc8778`, `39537a2`, `18e38bb`, `f70e8c5`, `60e9e35`, then `2c2a455` (the gate's workflow) and the commit that last changed this file. `b664fc6` holds the id table. Pushed to `origin/main` on 2026-10-11, when the owner authorized it: `60e9e35` at 21:12 and `2c2a455` at 21:30. This document tags and releases nothing; a Nightly candidate built from a later commit is recorded in `CHANGELOG.md` and on its own Release |
| Machines | build and Slack driver: macOS 27.0.1 (26A434), arm64. Real windows: the Mac mini test machine, macOS 27.0.1 (26A434), arm64 |
| Toolchain | cargo 1.97.1, cargo-llvm-cov 0.8.5, tauri-cli 2.11.4 |
| Debug carriers | `target/debug/deck-app` of the working tree in an ad-hoc signed bundle, identifier `io.c9r.deck.smoke`; plain sidecars (checked before every assembly) |
| Release carrier | the tauri bundler's release bundle, identifier changed to `io.c9r.deck.smoke`, Developer ID signed, not notarized; plain release sidecars, no `ui/test` inside |

The commit ids in this file are the ids of this history. At 19:21, before
anything was published, nineteen commits were recreated with identical
content to correct one commit's author identity. Evidence gathered before
that ran on commits with the same trees under earlier ids, and this file
cited those until the final round; [commit-ids.md](commit-ids.md) maps every
earlier id, for the raw records and the older documents that still use them.

Which source each piece of evidence ran on (a diff digest is the SHA-256 of
`git diff HEAD` at that moment; the untracked files it does not cover are
named):

| Evidence | Source |
| --- | --- |
| A. reproduction | base commit, no change (`build.rs` `930658bf…`) |
| A. acceptance sequence | diff `1cf334fc…`; `build.rs` `addaedb9…`, `build/shared_output.rs` `c32563c3…`, `scripts/sidecar_provenance.py` `b15dc833…`, `scripts/package-compat-candidate` `afe4a2a1…` — all four files are byte for byte the same in the commit |
| C. release carrier, its signature matrix, migration M1–M4 | diff `d9f31a04…`; production source equal to the commit except a comment in `agent_reentry/claude.rs` |
| C. `hooks-live` | the tree before the last formatting and documentation edits; production source equal to the commit except that comment |
| B. final Slack run, final gate | the committed tree. Only the two files of this directory were written after them; the final Slack carrier's executable was `db975446…` |
| Follow-up: A's second fix, the clean-worktree gate | commit `9e0bad3` (then `f9df0ca`: the same tree); the gate ran in a detached worktree of exactly that commit (cold build, its own target directories) |
| Follow-up: C's live transaction | the code of commit `c9d78f5` (then `5274067`): its tracked changes (diff `56c261b7…`) and the scenario file (`17e5336b…`) are byte for byte what the run used; `app/SMOKE.md` was written after it. Carrier executable `efbcdeaa…`, debug, ad-hoc signed |
| Follow-up: gate with the new scenario | the working tree that became `c9d78f5` |
| Final round: tests, negative controls, the launch through `app/run.sh`, the translation carrier's assembly, the gate | commit `39537a2` (which contains `8dc8778`); only documents differed from it while the gate ran |
| Final round: the release transaction | the release profile of commit `18e38bb`, working tree clean; carrier executable `9fb51c7c…`, Developer ID signed, not notarized. The registry tool as of `39537a2`, and with `f70e8c5` from the keep-awake's registration on |

## Verdict

One row per acceptance item. "Where" says which build ran from which place;
a row never speaks for a place or a build it does not name.

| Item | Status | Where it ran, and what it does not cover |
| --- | --- | --- |
| A1. Shared build outputs: the pollution reproduced and fixed | **PASS** | this checkout, macOS 27.0.1: sequence 68 of 68 checks |
| A2. The staged frontend takes no dot entry of the source tree (follow-up) | **PASS** | test and its negative control; a debug build after the fix |
| A3. Verification follows the same staging contract (final round) | **PASS** | tests on one fixture read by both sides; the real `ui-dist/` of this checkout judged by it |
| A4. A package is checked again after it is copied, before it runs (final round) | **PASS** | `app/run.sh`: a real isolated launch; the translation carrier: its real assembly, and refused twice with a foreign sidecar; negative controls in the tests. In CI the gate's `release` job packaged and checked its candidate on `macos-26`; `nightly.yml` and `release.yml` check their own bundle only when a candidate is built |
| B. Slack in the real window, six items | **PASS** | DEBUG carrier (ad-hoc signed) in a private directory, macOS 27.0.1, one sandbox workspace. A release build: UNRUN |
| C1. The two Settings switches for Claude's hooks, in the real window | **PASS** | DEBUG carrier, relocated home. A release window: UNRUN. `/Applications/deck.app`: BLOCKED |
| C2. Launch-time migration of existing hooks | **PASS** | RELEASE carrier (Developer ID signed, not notarized), relocated home, no click. `/Applications/deck.app`: BLOCKED |
| C3. The transaction in a real window with real clients (follow-up) | **PASS** | DEBUG carrier (ad-hoc signed), relocated home, that home's hook file, macOS 27.0.1, Claude Code 2.1.296 signed in there by the owner: both policies, `/clear`, a queued prompt, a second review and a relaunch |
| C4. The transaction in a RELEASE build, relocated home (final round) | **PASS** | RELEASE carrier (Developer ID signed, not notarized), relocated home, that home's hook file, macOS 27.0.1, Claude Code 2.1.296 signed in there by the owner. The owner made the two cards, queued the prompt and operated both restart reviews in the real window: Policy B, then the ordinary policy after the application had been started again. Typed into the cards from outside, not through the window; observation was on through the launch-time migration, not through the Settings switch; what the window said is the owner's reading and was not recorded |
| C5. The transaction in a RELEASE build at `/Applications/deck.app` | **BLOCKED** | the owner's installed Deck is in use on both machines; replacing it is the owner's decision. It would also need a person at the window, as C4 did |
| C6. An unknown or unconfirmed outcome, or an agent that does not exit in time, in a real application | **UNRUN** | tests only; no real run produced one |
| macOS 26: the automated gate | **PASS** | CI run 38139504866 on `2c2a455`: `checks`, `rust` and `release` on hosted `macos-26` runners, `macos14-launch` on `macos-14`. It closes what those jobs executed ("CI on the pushed commits") and nothing below. The first run, on `60e9e35`, failed |
| macOS 26: native identity of a real client | **BLOCKED** | no macOS 26 machine; the gate runs no real client, so the green gate does not close this |
| macOS 26: the transaction in a real window | **BLOCKED** | no macOS 26 machine |
| Private release carrier: release profile | **PASS** | every executable of the carrier is this checkout's release build (provenance check); no `ui/test` inside |
| Private release carrier: Developer ID signature | **PASS** | each executable verifies by itself, the bundle verifies deep and strict, hardened runtime; signed on the build machine, no secure timestamp |
| Private release carrier: notarization | **UNRUN** | nothing of this round's own carriers was sent to Apple |
| Private release carrier: Gatekeeper | **assessed, rejected** | `spctl` says "Unnotarized Developer ID". The carriers ran only because a file copied over SSH carries no quarantine attribute, so no launch here was a Gatekeeper decision. Acceptance: UNRUN |
| Experiment cleanup: only what a run registered and can prove is its own is stopped (final round) | **PASS** for what ran | tests with real tmux servers and a sentinel, negative controls, a harmless fixture on the test machine, one live cleanup on the build machine, and the release run itself: a real release carrier whose socket is named `deck`, three server generations registered, everything stopped through the registry. The two older runners moved onto it were not run again |
| The published Nightly candidate 0.8.0: signature, notarization, stapling, Gatekeeper | **PASS**, by its own workflow | built by CI from `a6e1df3`, not by this round: "The Nightly candidate". Not installed or updated to on a machine in this round |
| Architecture and regression checks | **PASS** | the gates below, on macOS 27.0.1, and the gate in CI on `macos-26` |

One incident happened on the test machine during C. It is recorded in
"Incident"; what it destroyed stays destroyed and no row above softens it.

## A. Shared build outputs

### The defect, reproduced on the base commit

`app/src-tauri/binaries/` (tauri `externalBin`) and `app/src-tauri/ui-dist/`
(`frontendDist`) are ONE location each, while debug, release and
`cargo llvm-cov` each produce their own sidecars and their own frontend
(`ui/test` only for debug). `build.rs` copied sidecars with `std::fs::copy`,
which keeps the SOURCE's modification time. Cargo decides whether a build
script runs again by modification time alone, so a sidecar another context
had built days earlier arrived looking days old and the next context saw no
change. `ui-dist/` was not an input of the build script at all.

Sequence on the unmodified tree (each step: the command, whether `build.rs`
ran, what `binaries/` then held):

| Step | Command | `build.rs` ran | `binaries/` after |
| --- | --- | --- | --- |
| s2 | `cargo llvm-cov … --no-report` (build only) | yes | coverage-instrumented |
| s3 | `cargo build` (warm) | **no** | **still instrumented**; a helper run in an empty directory left a `default_*.profraw` file there (2 helpers, 2 files) |
| s4 | `cargo build` again | no | still instrumented |
| s5 | `cargo build --release` | yes | release |
| s6 | `cargo build` | yes (only because one release sidecar had just been relinked) | debug |
| s7 | `cargo build --release` (warm) | **no** | **debug sidecars**, and the release executable was recompiled against the debug frontend: it embedded `/test/wk-smoke.mjs` and `/test/static.test.mjs` |
| s8 | `cargo build --release` again | no | debug |

### The fix

- `app/src-tauri/build/shared_output.rs` (new; contract in its header) is
  the only writer of both outputs. An output that already is what this
  context produces is left alone; otherwise it is replaced — a sidecar by a
  new file renamed over the old one — and stamped with the instant cargo
  started this build-script run. Every other context then sees a change and
  writes its own; this one does not rebuild for its own writing. `build.rs`
  names `binaries/<sidecar>` and `ui-dist` with `rerun-if-changed`.
- `scripts/sidecar_provenance.py` (new) answers from the files themselves:
  a Mach-O is instrumented when it carries `__llvm_prf*` / `__llvm_cov*`
  sections; it is this checkout's build of a context when its `LC_UUID`
  equals the product's (unchanged by code signing); a release executable
  must name no `ui/test` asset and a debug one must.
- Packaging paths now check before they copy: `app/run.sh`,
  `scripts/package-compat-candidate` (before, and on the finished bundle),
  `scripts/translation-lens-verify.py`, `nightly.yml` and `release.yml` (on
  the built bundle), and `gate.yml` after the coverage step (the tests must
  have run the coverage run's own instrumented sidecars). In this round
  `app/run.sh` and `translation-lens-verify.py` checked only BEFORE the
  copy; the final round added the check of the finished bundle.
- Tests: `tests/shared_output.rs` (13: the rule against cargo's
  modification-time check, and a source pin that `build.rs` writes through
  the module), `scripts/test_sidecar_provenance.py` (18, in the gate's
  unittest step). After the final round: 15 and 31.

What the fix cannot do, by construction: keep a package right when a
DIFFERENT context builds between a build and its packaging step (an editor
running `cargo check` during a release build). That is what the packaging
checks are for.

### Acceptance sequence (this round, 68 of 68 checks, exit 0)

| Requirement | What ran | Result |
| --- | --- | --- |
| ordinary → coverage → ordinary, warm | `cargo build`; again; `cargo llvm-cov`; `cargo build`; again; `cargo llvm-cov` (warm); `cargo build` | after each coverage run the next ordinary build ran `build.rs` and `binaries/` was the plain debug build; a helper run in an empty directory left 0 files; an immediate second build ran nothing and recompiled nothing |
| debug ↔ release, the real packaging steps | `cargo build --release`; `scripts/package-compat-candidate`; `cargo build`; a debug bundle assembled the way `app/run.sh` copies; `cargo build --release` (warm); again; `scripts/package-compat-candidate` | every package held its own profile's sidecars; the release executable named no `ui/test` asset, the debug one did; `ui-dist/test` absent after release, present after debug; the second release build ran nothing |
| profile, source and instrumentation of the executable and every sidecar | `sidecar_provenance.py check` on `binaries/`, on each assembled bundle (4 sidecars, tmux, executable) | exit 0 for the built context, exit 1 for the two others, every time |
| another context writes between a build and its package | a debug adapter copied into `binaries/` after a release build | `package-compat-candidate` exit 1, no archive; the check names `deck-mcp`; the next `cargo build --release` ran `build.rs` and packaging passed again |
| the real bundler | `tauri build --bundles app` (updater artifacts off), then the check on the bundle; again after Developer ID signing | exit 0 both times: 6 files are this checkout's release build |

### Does coverage really cover the sidecars

Per-function data of the gate's coverage run (`llvm-cov report
--show-functions` on each instrumented sidecar), function `main`:

| Sidecar | Before (first gate run of the round) | After (final gate run) |
| --- | --- | --- |
| `deck-mcp` (adapter) | 100 % of lines | 100 % |
| `deck-mcp-runner` | 80.95 % | 80.95 % |
| `deck-lifecycle-helper` | **0 %** | 93.75 % |
| `deck-status-helper` | **0 %** | 92.59 % |

The two zeros were not caused by the pollution and were not visible in the
file totals (87–90 %, from unit tests): the helpers' own process tests start
them with a cleared environment, so the instrumented helper never learned
where its counters go and wrote `default_*.profraw` into the crate directory
instead (34 such files after one gate run; none reached the report). The
status helper had no process-level test at all. Fixed in test code only:
the env-cleared launches carry `LLVM_PROFILE_FILE` when it is set, and
`status-helper/tests/cli.rs` (new, 6 tests) runs the built helper the way a
hook does. After the final gate run: 0 stray `.profraw` files in
the source tree.

### Follow-up: what a local build staged besides the frontend

Asked whether the change was complete, a check of what is ignored inside the
source tree found `app/ui/.playwright-cli/`: 38 console logs and page
snapshots a browser-automation tool had left there in August (pictures of the
UI mock; nothing sensitive, checked). `build.rs` staged ALL of `app/ui`, so
every build made on this machine — debug and release, the private carriers of
this round included — compiled those files into the executable (27 of their
names were readable in the release executable). A clean checkout never had
the directory: CI builds and every published build were not affected.

Fixed in `9e0bad3`: `stage_tree` leaves out an entry of the SOURCE tree whose
name starts with a dot, at any depth; the tree in place is still read whole,
so a `ui-dist/` staged with such entries is staged again. One new test
(`dot_entries_of_the_source_tree_are_never_staged`); with the filter removed
in a copy of the module that test fails and the other 13 pass. After the fix
a debug build stages no dot entry and the executable carries none of the
names. The provenance check does not look for this in a packaged executable
(asset names cannot be listed from the binary); the rule and its test are
the guard. The two scripts that compare `ui-dist/` with the source did not
follow this rule yet; that is item 1 of the final round.

The other things that were checked for completeness: every file of the round
is tracked and nothing it needs is ignored; no tracked document links into
`reports/`; the frontend and script gates pass in a plain export of the
commit; and the gate's coverage step passes in a detached worktree of
`9e0bad3` built from nothing (below). Not part of this round by the
repository's own convention: `CHANGELOG.md` and the version, which are
written by the release commit.

## B. The Slack integration in the real window

Three kinds of evidence, kept apart:

1. **Backend against real Slack, no window** — reused:
   `real_slack_certification_with_explicit_sandbox_credentials` passed on
   2026-10-09 (`docs/evidence/first-party-extensions-20261009/`). Not rerun.
2. **Backend and DOM without Slack** — this round, in the final gate: the
   loopback transport-fault test, the Settings rendering of the shared fault
   vector (`ui/test/fixtures/channel-fault-status.json`), the registry and
   authority tests named in "Architecture".
3. **Real window, real service** — this round, below.

### Setup

The owner signed in to Slack in a separate browser profile and chose a
sandbox workspace. In it, this round created a test app from the scopes and
events of Deck's own manifest (`slack_api::manifest`; only the display names
differ), an app-level token with `connections:write`, and a private test
channel with the bot as a member. Existing apps of that workspace were not
used: another Deck might be connected to them, and Slack hands each event to
one connection. The two tokens were written from the browser page to 0600
files without being displayed, travelled to the test machine as one 0600
file removed as soon as the application had read it, and entered the
application through the Settings fields' own handlers (a smoke launch keeps
credentials in process memory only; the login Keychain was never involved).

### The run

New debug-only smoke modes `channel-live` and `channel-live-replay`
(`app/ui/test/channel-live-smoke.mjs`, described in `app/SMOKE.md`). The page
does what a person does — Settings fields, the Automations editor, the
channel switch — against the real backend; a driver on the build machine
typed each requested message into the test channel through the signed-in
browser. Faults are the application's own debug hooks (`smoke_fault_set`):
they end or fail only that process's connection and write nothing to Slack.
The rule's command is the real `claude`; it started in the card's pane on
the test machine (2.1.296, not signed in there) and was sent nothing.

Final run (14:34–14:38 JST), `scripts/smoke-verdict`: `channel-live` **PASS 15
of 15**, `channel-live-replay` **PASS 2 of 2**; an earlier full run of the
same sequence in this round also passed 15 of 15 and 2 of 2.

| Item of the brief | Checkpoints | What was observed |
| --- | --- | --- |
| 1 event → durable → one Board card from the saved rule | `live-rule`, `live-connected`, `live-card` | rule saved by the editor (no first-send choice); Socket Mode connected within seconds; message 1 created exactly one card with `origin.source = channel`; its two template steps were queued as two external rows |
| 2 ACK, dedup, pending | `live-card`, `live-append`, `live-resume`, `live-redelivery`; documents | nothing left pending after any message; message 2 joined the same card; every Slack event is in the inbox's `applied` list exactly once with distinct event ids; with `channel-inbox-save` armed once, message 5 was refused without an ACK (socket dropped with `capacity`) and arrived again by Slack's own redelivery, once |
| 3 a fault after the connection, in real Settings | `live-fault` | under the injected `channel-network` fault the Settings texts were "Socket Mode: disconnected", "not connected (no backfill)", "unresolved message gap" and the `network` code |
| 4 recovery: pending right, gap kept, no claimed backfill | `live-recovered`, `live-late`, `live-resume` | connection back, nothing pending, the gap still said, "no backfill" wording gone with the disconnect and no backfill claimed. Message 3, sent while disconnected: in one full run of this round Slack delivered it by its own retry after the reconnect, and it joined the card once; in the two other full runs, the final one included, it had not arrived 75 seconds after the recovery. Deck asks Slack for no history either way |
| 5 no authority from fault, recovery or replay | `live-authority`, `live-replay`; documents | no `firstSend`, grant, grant activation, `authority`, `channel_first_send` or `readiness_override` in settings, inbox, queue or Board, before or after; queue rows: 2, external, 0 attempts, 0 deliveries; log has no `[queue] sent` line |
| 6 one connection owner, through the registry | log | each launch logged `[extensions] services started (1)` once; no "second transport owner", no refused registration |

Replay: the same data root and socket launched again with no credentials —
still one card with the same messages and the same two rows, nothing
connected, the gap still said, the documents identical in every id.

### Limits

- macOS 27.0.1, one debug carrier, one sandbox workspace. Not a release
  build: a release build has no smoke driver and stores credentials in the
  login Keychain.
- The injected `channel-scope` fault of the older `channel-fault` mode was
  not part of this run; a total network outage was not produced (the faults
  are in-process).
- A pending event carried across a quit was not produced in the window; the
  replay shows applied events staying applied. Durable pending work being
  re-announced is covered by tests (kind 2).
- What Slack redelivers after a gap is Slack's behaviour and varied between
  runs; nothing here is a statement about Slack's retry schedule.

## C. Re-entry

See [reentry-acceptance.md](reentry-acceptance.md): the reused transaction
evidence, this round's release carrier and its four separate properties,
the launch-time migration scenarios M1–M4, the Settings switches in the real
window, the eleven items one by one with a column per place and build, the
three macOS 26 items, and what a release transaction needs. The follow-up's
transaction with real clients in a real window is its section "The
transaction in the real window with real clients (L)".

Everything of C that ran, ran under a relocated home or in a private
directory. In the round and the follow-up the release carrier ran the
migration and nothing else, and the transaction ran in debug carriers only;
the final round added one transaction in a release carrier, with the owner
at its window.

## Architecture and regression checks

Read from the code and run in the final gate, on the commit:

- The three tables are consumed by production paths: services are started
  by `extensions::start_services` from the application's setup and nowhere
  else (one owner per process: `the_transport_has_one_owner_per_process`),
  the poll loop takes its sources from `service_sources`, a source word is
  recognized only through the registered adapter, the translation engine
  through its provider entry.
- Unknown and conflicting registrations fail closed:
  `production_tables_are_well_formed_and_closed`,
  `malformed_tables_are_named_not_tolerated`,
  `lookups_answer_only_for_exactly_one_entry`,
  `a_service_id_two_entries_share_takes_no_part_in_dispatch`.
- Re-entry enters through the adapter capability
  (`only_claude_declares_a_reentry_capability_and_lifecycle_hooks`); the
  core keeps authorization, lifecycle, input and confirmation, and
  `agent_reentry` cannot reach the scheduler (source scan in its tests).
- `tests/signal_census.rs`, `tests/external_admission.rs`,
  `tests/edr_quiet.rs`, `tests/ipc_contract.rs` unchanged and green.

### Final gate (this round)

| Command | Exit | Result |
| --- | --- | --- |
| `cargo fmt --all --check` | 0 | |
| `cargo clippy --workspace --all-targets --all-features --locked --offline -- -D warnings` | 0 | |
| `cargo llvm-cov --workspace --all-targets --all-features --locked --offline --fail-under-lines 75 --fail-under-functions 75 -- --test-threads=1` | 0 | 1416 passed, 0 failed, 12 ignored; lines 85.06 %, functions 82.90 % |
| `python3 scripts/sidecar_provenance.py check --profile debug --coverage --binaries` (the new gate step) | 0 | |
| `scripts/ui-tests` | 0 | 743 passed, 0 failed |
| `node --check` on every frontend and test module; `node app/ui/js/check.mjs` | 0 | 60 modules, identifiers resolve |
| `python3 -m unittest scripts/test_*.py` (the gate's list) | 0 | 134 passed |
| `scripts/check-workflows` | 0 | |

No assertion, threshold or exclusion was relaxed. `--offline` is added to
the CI forms; nothing else differs. Not rerun this round because nothing
they cover changed: the Swift bridge scripts, the RustSec audits, the Tunnel
Helper suite, the Full Local parity lanes, the macOS 14 launch check.

Failures met on the way, with their real exits: the first run of the new
`channel-live` mode failed at `live-params` (exit 1; the scenario closed its
reader card by session name instead of card id — a scenario defect, fixed);
a first clippy run failed (exit 101, `type_complexity` in the new helper
test — fixed); one new unit test failed once on its own fixture (fixed).

### Follow-up gates

| What | Exit | Result |
| --- | --- | --- |
| The coverage step in a detached worktree of `9e0bad3`, built from nothing | 0 | 1417 passed, 0 failed, 12 ignored; lines 85.05 %, functions 82.90 %; coverage-context provenance 0; no `.profraw` left in the tree |
| `python3 -m unittest` (the gate's list), `node --check`, `scripts/ui-tests`, `check.mjs` in a plain export of `9e0bad3` | 0 | 134 passed; the others exit 0 |
| On the tree that became `c9d78f5`: `cargo fmt --all --check`, CI-form clippy | 0 | |
| Same tree: the coverage step | 0 | 1417 passed, 0 failed, 12 ignored; lines 85.05 %, functions 82.90 % |
| Same tree: coverage-context provenance; then an ordinary debug build and the debug provenance check | 0 | the ordinary build after the coverage run put its own sidecars and frontend back |
| Same tree: `scripts/ui-tests`, `node --check`, `check.mjs`, `python3 -m unittest`, `scripts/check-workflows` | 0 | 743 passed; 134 passed |

Failures met in the follow-up, with what they were: the first live run ended
with `relive-preview` failing (the scenario expected the row of a card
running the system `top` to say "reopen manually"; `top` belongs to another
user, Deck cannot read it and says the context could not be verified — the
scenario's expectation was wrong, the transaction itself had gone through);
the client could not save its sign-in under a relocated home until that home
had a keychain of its own, and the first such keychain could not be unlocked
because of its file name (both are setup facts, written down in
`app/SMOKE.md`).

## Final bounded round

Asked for, in this order: the verification rule that still looked at dot
entries; a provenance check of the package that actually runs; the incident's
guard as a tracked, reusable mechanism; the scope of this document; and only
then the most valuable release transaction. Commits `8dc8778` and `39537a2`.

### 1. Verification follows the staging contract

`build/shared_output.rs` stages no entry of `app/ui` whose name starts with a
dot, at any depth. `scripts/translation-lens-verify.py` and
`scripts/translation-setup-verify.py` still compared every file of `app/ui`
with `ui-dist/`: one `.DS_Store` in the source made them report a frontend
mismatch, and neither noticed a file in `ui-dist/` that the source no longer
had.

`scripts/sidecar_provenance.py staged --profile debug|release` reads the
source by the same contract and reports three things apart: a file missing
from the staging, a file staged with other bytes, and anything staged that
does not belong there (a dot entry, a file the source no longer has,
`ui/test` in a release staging). Both scripts and `app/run.sh` ask it. The
consistency check was not switched off: an ordinary file that differs or is
missing still fails.

The contract is one fixture, `tests/fixtures/staging-contract.json`: twelve
source paths, among them `.DS_Store`, `test/.DS_Store`, an editor's swap file
and two nested hidden directories, and what a debug and a release staging of
them must hold. The Rust test stages the fixture with the build's own module;
the Python test judges it with the checker.

| What ran | Result |
| --- | --- |
| `tests/shared_output.rs` | 15 passed (one new: the fixture is what the module stages) |
| `scripts/test_sidecar_provenance.py`, the staging cases | 6 passed: the contract holds; a missing file, a changed file, a dot entry in the staging, a file the source no longer has, and `ui/test` in a release staging are each reported |
| Negative controls, in copies of the tree | staging dot entries in the module, and dropping the "does not belong" report in the checker, each fails a test |
| The real `ui-dist/` of this checkout | `staged frontend ok`: the debug staging of `app/ui`, 171 files |

### 2. The package that runs is checked

`app/run.sh` and `translation-lens-verify.py` checked `binaries/` before the
copy and never what was copied. An editor's `cargo check`, a release build or
a coverage run can write `binaries/` in between, and the bundle then runs
their sidecar.

- `app/bundle.sh` (new, sourced by `app/run.sh`) holds the three steps: check
  the shared outputs, copy, check the finished and signed bundle against this
  debug build. `run.sh` refuses to open a bundle that fails the last one.
- `translation-lens-verify.py` checks its assembled, signed carrier the same
  way before any run. That carrier holds two of the four built sidecars.
  `check --sidecars` names the ones a package must carry: both are checked
  with tmux and the executable, and a third sidecar that got in would be
  checked too, never skipped.
- Coverage is as it was: the gate's coverage-context check and the
  instrumented sidecars it requires did not change.

| What ran | Result |
| --- | --- |
| `scripts/test_sidecar_provenance.py`, the packaging cases | 7 passed. The negative controls put a coverage sidecar, and a release sidecar, into `binaries/` AFTER the first check passed, run the launcher's own copy function, and require the last check to refuse the bundle |
| Negative controls, in copies of the tree | removing the last check, and skipping a sidecar that was not named, each fails a test |
| `app/run.sh`, one real isolated launch on the build machine | first check ok (5 files in `binaries/`, 171 staged files); bundle signed; last check ok (6 files in the bundle); opened through LaunchServices; it booted into its own data directory |
| The translation carrier's own assembly with the real build outputs, no launch | every check ok; the bundle holds the executable, tmux and the two helpers (4 files) |
| The same assembly, with a release-profile helper and then a coverage-instrumented helper handed to the copy after the first check had passed | refused both times: "the assembled bundle does not hold this debug build" |

Not run: the translation verification's window runs, and the release
workflows.

### 3. Experiment resources: registered when started, stopped only when proven

The guard that followed the incident lived in a git-ignored runner. It is now
the tracked tool's, and the rule is in `AGENTS.md`.

- `scripts/edr_runtime.py` addresses no tmux server by its socket name. The
  reviewed development/smoke inventory resolves a name to the full path
  `tmux -L` would use from this environment and stops a server only while
  that path still answers as the very pid it inspected. The caller's own
  `TMUX` is handed to no command.
- `experiment init | add-server | add-process | show | stop | cleanup` keeps a
  registry in the experiment's root directory. A server is its full socket
  path inside the root, the pid the server itself reports there, and that
  process's start time and executable. Before anything is stopped every entry
  is verified again. A missing field, another process under the pid, another
  pid on the socket, a path or a link that leaves the root, a root that is
  not a directory of its own under a temporary directory, or a socket,
  program or root that belongs to an installed Deck or to a person's home
  refuses the WHOLE cleanup: exit 3, nothing stopped. What runs from the root
  without having been registered is reported (exit 1), not stopped. A pid
  alone, a path prefix alone, or a socket name is never enough.
- `scripts/test-compat-candidate`, the gate's macOS 14 launch check, ended
  with the same by-name kill of `deck`; the new scan over the tracked scripts
  found it. It now asks the server on the full socket path which process it
  is and stops it only when that is the candidate's own tmux.
- Two rules over the tracked scripts, as tests: none names the installed
  product's socket, and every script that stops a tmux server is on a
  register that says how it names it.

| What ran | Result |
| --- | --- |
| `scripts/test_edr_runtime.py` | 29 passed. Real tmux servers in directories of the test's own, always by full path. The registered server is stopped; a sentinel server beside it that no registry names is still there after every case; a socket outside the root, a link out of the root, a reused pid, a changed program, another pid on the socket, an incomplete or foreign registry, and what looks like an installed Deck are each refused with nothing stopped, and the last of them without one command being run |
| Negative controls, in copies of the tool | 16 guards removed one at a time; the tests failed each time |
| The tool on the test machine, read there from standard input, against a harmless fixture (a copy of the pinned tmux, a fixture server on a socket named like a release carrier's inside its root, a sentinel beside it) | registered; the sentinel's socket, a person's home as root and a missing root refused (exit 3); cleanup stopped the fixture and nothing else; the installed Deck's shell service had the same pid and start time before and after. Nothing was pointed at the installed Deck |
| The reviewed cleanup on the build machine, live, after the launch of item 2 | application and server stopped, verified by pid; the installed Deck's processes identical before and after |

The runners that started carriers on the test machine are git-ignored working
files. The two that would be used again (the release carrier's migration and
the live transaction) now put the carrier's socket directory inside the root
and stop things only through the registry; they were **not run again** after
that change. The two others refuse to run.

### 4. What this document claimed too widely

Corrected in this round, so that a reader of an earlier copy can see what
changed:

- The opening said three things "were closed out". Several items were
  BLOCKED or UNRUN then and are now.
- C was headed "on the installed path", and the acceptance summary said the
  installed path proper was "verified for the hook families and their
  migration". What ran was a release carrier and a debug carrier under a
  relocated home. Nothing ran at `/Applications/deck.app`. The eleven-item
  table had one "Formal" column; it now has a column per place and build.
- "macOS 26" was one item. It is three: the automated gate, native identity
  of a real client, and the transaction in a real window.
- "Notarization, Gatekeeper acceptance, public carrier" was one row. Release
  profile, Developer ID signature, notarization and Gatekeeper are four.
- The resources table said the test machine's own state, the installed
  Deck's shell service included, was "as before the follow-up". That is true
  of the follow-up's own baseline, which was taken after the incident. It
  undoes nothing; see "Incident".
- "Incident" said the runners now stop only their own server. That guard was
  in files no checkout has; item 3 replaces it.

### 5. The release transaction

Asked for last: a minimal Policy A / Policy B transaction in a RELEASE build
at an isolated stable location, with global hooks, real Claude and the real
UI. It ran once, 20:54–21:00, and every check of the judgement held.

What decided how it ran:

- A release build ignores every smoke argument, so its switches and its
  dialog need clicks, and nothing here can make them. Asked without
  prompting, `AXIsProcessTrusted`, `CGPreflightPostEventAccess`,
  `CGPreflightListenEventAccess` and `CGPreflightScreenCaptureAccess` all
  answered false for a process started from the build session, on the build
  machine and over SSH on the test machine, and the session had no
  desktop-control tool. The owner agreed to operate the window and asked for
  that part to be as small as possible.
- So the owner did what only the window can do: two new cards, one queued
  prompt, and the restart review twice. Everything else was done from
  outside, and is named as such below.
- The carrier: the release profile of `18e38bb`, the bundler's bundle with a
  private bundle identifier and an `LSEnvironment` of three values (`HOME`,
  `TMUX_TMPDIR` inside the root, `DISABLE_AUTOUPDATER` for the clients),
  Developer ID signed on the build machine, not notarized, at
  `<home>/Applications/deck.app` under a relocated home on the test machine.
  The executables are the release build's, unchanged: this is packaging
  around them, not a switch in the product, and not the published bundle.
  One background trial launch first: the process's environment, read from
  the kernel, was those three values; its data, socket and log were inside
  the root; it was stopped through the registry.
- The client: a keychain of its own for that home, the owner's authorization
  in a browser on the build machine, then trust for the two directories and
  the bypass warning accepted once in a client outside the carrier.

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

What this run is and is not:

- It is a RELEASE build with a Developer ID signature at a stable location
  under a relocated home, with that home's hook file and a real, signed-in
  client. It is not `/Applications/deck.app`, not a person's home, not a
  notarized bundle, and no Gatekeeper decision was involved.
- The typing into the two cards came from outside, through the carrier's own
  tmux server by its full socket path, each keystroke after a look at the
  pane. The clients are direct children of the cards' shells either way; it
  is still not typing through the window.
- The Settings switch was not used. Observation was on because the launch
  migrated entries that were put there by hand in the form of an earlier
  installation. That is the migration path (M1) once more, now followed by
  real clients using the migrated entries; the switch in a release window
  stays UNRUN.
- What the review and the result rows SAID was read by the owner and not
  recorded. The machine evidence is what was sent and what ran.
- The review directly after Policy B was not opened before the application
  was started again, so "the next review inherits nothing" is shown here
  only across that restart of the application.
- No unknown outcome, timeout or unconfirmed entry occurred.
- The clients ran on the owner's account: four one-word prompts in all, the
  one from the GUI-session check included. Nothing was typed into a
  re-entered client.

Found on the way and fixed (`f70e8c5`): the registry refused to register the
keep-awake process, because a program started by its bare name shows no
path and the tool resolved the name against the directory it ran from, a
person's home over SSH.

### CI on the pushed commits

The owner then authorized a push and a run of the gate, to close what CI
actually covers and no more. `main` was pushed as a fast-forward
(`dcc3c06..60e9e35`, 26 commits; every identity and every added line was
scanned again first).

| Run | Commit | Result |
| --- | --- | --- |
| 38138366227 | `60e9e35` | **FAIL** in `rust`. `checks`, `release` and `macos14-launch` passed |
| 38139504866 | `2c2a455` | **PASS**: `checks`, `rust`, `release` (hosted `macos-26` arm64 runners), `macos14-launch` (`macos-14`), and the summary job |
| 38141233582, and the gate inside the Nightly run 38141251083 | `d2b0ecb` | **FAIL** in `rust`, both: one test. Nothing was built or published |
| 38143516667, and the gate inside the Nightly run 38143517011 | `a6e1df3` | **PASS**, both |

Why the first run failed: 69 tests of `agent_reentry::tests` stopped at the
first line of their fixture, which requires the account's login shell to be
`/bin/zsh`. Those tests run the production assembly; it reads the login
shell from the account database, and re-entry supports zsh, the macOS
default. The hosted runner's account is created with bash. This code had
never been built in CI, so the gate had never met it. It is a precondition
of the test environment, not a failure of the feature: on a machine whose
account uses another login shell the product shows such sessions as not
reopenable, and that case has its own test. `2c2a455` gives the runner's
account the macOS default before anything is built and fails at once if the
account database does not then say so. No test was changed and none is
skipped.

Why the two runs on `d2b0ecb` failed: `unknown_outcomes_are_never_retried`
starts four entries one after another, and an entry is attempted only while
the post share still has 1.5 s left. The test gave that share 4 s, so all
four had to be started within 2.5 s. The hosted runner managed it in the
run before and not in these two; the product then refuses the last entry
before its launch, as it should, and the test, which is about entries that
were started, failed on it. It did not fail on the build machine, with or
without load. `a6e1df3` gives the test's share 8 s and moves its late
receipt and its wait with it; every assertion is as it was. This is the one
change to a test in this round, and it changes times only.

What a green run executed, and so what it closes:

| Job | Runner | What it ran |
| --- | --- | --- |
| `checks` | `macos-26` | the bundled tmux's digest, `cargo fmt`, the frontend suite and syntax checks, the identifier scan, the script tests (170, among them the registry's 30 with real tmux servers), the workflow check, the Swift bridge guards, the audits and the Tunnel Helper suite |
| `rust` | `macos-26` | CI-form clippy; the coverage step: 1418 passed, 0 failed, 12 ignored, lines 85.05 %, functions 82.86 % (81 `agent_reentry` tests, and the identity mechanism's own tests with fixtures signed at test time); the coverage-context provenance check; the Full Local parity lanes and their verdict gate |
| `release` | `macos-26` | the release build, the translation-binary and EDR scans, and `scripts/package-compat-candidate` with its provenance checks |
| `macos14-launch` | `macos-14` | `scripts/test-compat-candidate`: the packaged candidate reached UI readiness on macOS 14 |

What it does not close:

- **Native identity of a real client on macOS 26, and a transaction in a
  real window there.** The gate starts no Claude Code client; its identity
  tests validate programs the test signs itself.
- **What the new cleanup of `scripts/test-compat-candidate` did on the
  runner.** The script ran green in both runs with the new function in
  place and printed no "left running" line, which means the function met no
  server that was not the candidate's. The job asserts nothing about it;
  the function's own behaviour is shown by its fixture tests, which ran in
  `checks`.
- Notarization, Gatekeeper and a published candidate: the gate builds no
  signed bundle. That is the Nightly workflow's part, below.

### The Nightly candidate

The owner authorized a Nightly in the 0.8 series after the gate. The
candidate is not evidence gathered by this round's own runs; its workflow
is, and it is recorded here for what that workflow ran.

| | |
| --- | --- |
| Version and commit | 0.8.0, `a6e1df3` (the release commit `d2b0ecb` plus the test's times) |
| Run | Nightly 38143517011; its gate passed, the `nightly` environment was approved under that authorization, then `build-sign` and `publish` |
| Candidate | prerelease `nightly-v0.8.0-20261011-a6e1df3`: the disk image, the updater archive and its signature, the Tunnel Helper archive, `candidate.json`, `provenance.json`, `SHA256SUMS` |
| What the workflow verified on its own bundle (`provenance.json`) | the gate passed; `codesign`, notarization, stapling and Gatekeeper passed, for the application and for the Helper; updater key epoch `nightly-v1` |
| Feed | the Nightly feed's `latest.json` names 0.8.0 and that archive. Stable is untouched: `v0.7.26` |
| Before the first dispatch, on the 0.8.0 tree (`d2b0ecb`) | `cargo fmt`, CI-form clippy, the coverage step (1418 passed, lines 85.06 %, functions 82.91 %), the frontend suite (743), the script tests (170), the workflow check; and the unattended smoke matrix on the test machine, 22 modes. After the test's times were changed (`a6e1df3`): clippy, that test three times on the build machine, and the two gates in CI |

About that smoke matrix. It ran through the registry of item 3: one root, a
relocated home, the carrier's socket directory inside the root, everything
stopped through `experiment stop` and `cleanup`. 21 modes passed at once.
`run` failed 9 of its 94 checkpoints, all of them where the smoke reads the
clipboard: the application had been opened with an empty environment, and
the smoke's own reader runs `pbpaste` under the process locale, so
non-ASCII text read back shorter than the product had written it (the
product's writer pins its locale). With the locale a launching shell hands
over, and nothing else changed, `run`, `ambiguous` and `restart` passed. Not
run: the translation modes, `channel-fault`, `channel-first-send`,
`authority-live`, `clock-live`, `selection-events`, `reminder` and
`reminder-native`; `channel-live`, `hooks-live` and `reentry-live` ran
earlier the same day on the same product source.

What the candidate does not show: the private carriers of this round were
never notarized, and their rows above stay as they are. Nobody has yet
installed the candidate from its disk image or updated to it from an
installed Deck in this round.

### Gates of the final round

On commit `39537a2`, macOS 27.0.1, with the same isolation as before
(`--offline` added to the CI forms; nothing else differs):

| Command | Exit | Result |
| --- | --- | --- |
| `cargo fmt --all --check` | 0 | |
| `cargo clippy --workspace --all-targets --all-features --locked --offline -- -D warnings` | 0 | |
| `cargo llvm-cov --workspace --all-targets --all-features --locked --offline --fail-under-lines 75 --fail-under-functions 75 -- --test-threads=1` | 0 | 1418 passed, 0 failed, 12 ignored; lines 85.05 %, functions 82.90 % |
| `python3 scripts/sidecar_provenance.py check --profile debug --coverage --binaries` | 0 | the coverage run's own instrumented sidecars; no `.profraw` left in the tree |
| then `cargo build`, `sidecar_provenance.py check --profile debug --binaries`, `sidecar_provenance.py staged --profile debug` | 0 | the ordinary build put its own sidecars and frontend back: 5 files, 171 staged files |
| `scripts/ui-tests` | 0 | 743 passed, 0 failed |
| `node --check` on every frontend and test module; `node app/ui/js/check.mjs` | 0 | 153 files; 60 modules, identifiers resolve |
| `python3 -m unittest` (the gate's list) | 0 | 169 passed (134 before this round: 13 more in `test_sidecar_provenance.py`, 22 more in `test_edr_runtime.py`); 170 after `f70e8c5`, which changed only the registry tool and its test |
| `scripts/check-workflows`, `scripts/test-smoke-guard` | 0 | |

No assertion, threshold or exclusion was relaxed. The documents of this
directory and two paragraphs in `docs/` were written after the coverage step
started; no test reads them. Not run in this round because nothing they
cover changed: the Swift bridge scripts, the RustSec audits, the Tunnel
Helper suite, the Full Local parity lanes. Not runnable here: the macOS 14
launch check, whose script changed (item 3) and whose cleanup function was
exercised against fixtures only.

Failures met on the way, with what they were: the first run of the new
`test_edr_runtime.py` cases had two errors of the test's own (it copied
`/bin/sleep` with its file flags) and one true finding, the by-name kill in
`scripts/test-compat-candidate`; one negative control was not caught at first
(a root outside a temporary directory) and a test was added for it; a count
the register test kept of its own lines was wrong after a test was added.

## Incident

At 14:12 on the test machine, the cleanup of the migration runner ran
`tmux -L deck kill-server` with the carrier's tmux but WITHOUT the run's own
socket directory. A release build always names its socket `deck`, so the
command reached the socket of the Deck formally installed on that machine
and stopped its shell service (the server started on Oct 10 16:55).

**What was lost, and stays lost.** Every session of that shell service
ended: the shells, and whatever was running in them. They cannot be brought
back. The application itself kept running and its `~/.deck` was not opened,
so the Board was not changed and its cards were left stopped. Nothing later
in this document restores any of this, and no later "unchanged" line covers
it: the baselines of the follow-up and of the final round were taken AFTER
the incident, against the shell service the owner's own use started at
17:32.

The run's "state as before" line was wrong: it recorded the application
process but not its tmux server. The owner was told at once.

What changed because of it:

- The same afternoon, in the git-ignored runners: a server was stopped only
  when its command path was under the run's root, with the socket directory
  in the same command, and every process of the installed bundle was
  recorded before and after. No checkout has those files.
- In the final round, tracked: the registry and its verified cleanup in
  `scripts/edr_runtime.py`, the rule in `AGENTS.md` ("Experiment
  resources"), and the regressions in `scripts/test_edr_runtime.py`. The
  same words, `tmux -L deck kill-server`, were found in one tracked script
  and removed from it (item 3).

## Resources and cleanup

Created this round and removed or left as stated:

| Resource | End state |
| --- | --- |
| Slack test app and its tokens, private test channel (sandbox workspace) | app deleted (its bot token then answered `account_inactive`); channel archived, which is reversible; the workspace's three existing apps were not opened and its sidebar is as it was |
| Separate browser profile holding the owner's Slack sign-in | signed out of the workspace; that browser instance quit by its own pid; profile directory removed |
| Test machine: three smoke roots, one relocated-home root per C run, their tmux servers | servers stopped; roots moved to that machine's Trash (recoverable); no process of the round left. The installed Deck's shell service was NOT as before: see "Incident" |
| Test machine: Claude's own record of the directories the Slack rule started it in | not looked for beyond a count of matching project directories (0) |
| Build machine: carrier staging directories, private token files, isolated cargo home, probe directories | removed by exact path |
| `app/src-tauri/target/` (release, debug and coverage products) | left; `binaries/` and `ui-dist/` are the ordinary debug build |
| Follow-up, test machine: a private Developer-ID-signed release carrier that relocates itself (`LSEnvironment`), opened once to check that it stays inside its directory | never used by the owner; its root removed |
| Follow-up, test machine: the root of the live run (relocated home with the client's sign-in in a keychain of its own, two data directories, the debug carrier) | client signed out, both sign-in items and the keychain deleted, root removed (not moved to the Trash: it held a credential store); no process and no socket of the run left |
| Follow-up, test machine: the machine's own state | its hook file digest, keychain list, `~/.deck` and installed Deck's shell service are as they were at the START of the follow-up. That baseline was taken after the incident: the shell service it names is the one the owner's own use started at 17:32, not the one the incident stopped |
| Follow-up, build machine: a separate browser profile holding the owner's Claude authorization | that browser instance quit by its own pid; profile and the two private files (address, code) removed |
| Follow-up, build machine: detached worktree, export, isolated cargo home, carrier staging directories, probe material | removed by exact path |
| Final round, test machine: the fixture for the registry tool (a copy of the pinned tmux, a fixture server, a sentinel server, two registry files, in two directories under `/private/tmp`) | both servers stopped through the tool; both directories removed by exact path; no process of the round left |
| Final round, build machine: one isolated smoke launch through `app/run.sh` | application and server stopped with the reviewed cleanup and verified by pid; its data directory and its stale socket file removed by exact path |
| Final round, build machine: test fixtures and tree copies of the negative controls under `/private/tmp`, the isolated cargo home and temporary directory | removed by their makers or by exact path |
| Final round, test machine: the root of the release run (relocated home with the signed carrier and the client's sign-in in a keychain of its own, the carrier's socket directory, a sign-in session, the registry) | everything registered was stopped through the registry (exit 0; three generations of the carrier's server, two application processes, the sign-in session, the keep-awake); client signed out, keychain deleted, root removed (not moved to the Trash: it held a credential store). The installed Deck's shell service, its `~/.deck` files, the hook file digest and the keychain list were the same before and after this run |
| Final round, build machine: a separate browser profile holding the owner's Claude authorization; the signed carrier's staging directory | that browser instance quit by its own pid and start time; profile, the two private files (address, code) and the staging directory removed by exact path |
| Seen on the build machine and left alone | stale `deck-smoke-*` socket files of earlier rounds in the default tmux directory. None is this round's, and nothing answers on this round's names |

## Open items

| Item | Why it is open | Smallest thing that closes it |
| --- | --- | --- |
| The Settings switches in a RELEASE window; typing through the window; a record of what the review said | the release run used the migration instead of the switch, typed from outside, and has only the owner's reading of the dialog | a person at a release window once more, or a way to operate one |
| C5. The transaction in a RELEASE build at `/Applications/deck.app` | the owner's Deck is installed and in use on both machines | the owner's decision to park their installation, or a machine without one; then the same steps |
| C6. An unknown or unconfirmed outcome, or an agent that does not exit in time, in a real application | no real run produced one; tests only | a fault staged on purpose in a later run |
| macOS 26: native identity of a real client | no machine; CI runs no real client | a macOS 26 machine with Claude Code on it |
| macOS 26: the transaction in a real window | no machine | the same machine and the follow-up's debug-carrier run |
| Notarization and Gatekeeper for this round's private carriers | they were never submitted; the published candidate's own workflow notarized and assessed ITS bundle | nothing, unless a private carrier must be handed to someone |
| The candidate installed, or updated to, on a machine | the workflow verifies the bundle on its runner; no installation was made in this round | installing Nightly 0.8.0 from its disk image, and one update from an installed Deck |
| The smoke's own clipboard reader takes the launching shell's locale | found before the Nightly: a smoke application opened with an empty environment fails its clipboard checkpoints on non-ASCII text, although the product wrote the right bytes (`smoke_clipboard_metrics` runs `pbpaste` unpinned) | pin the locale for that reader, as the product's own writer does |
| The two runners moved onto the registry | not run since they were changed | the next run of either |
| The translation verification's window runs | only its bundle assembly was exercised in the final round | one run of `scripts/translation-lens-verify.py` |
| Slack in a release build; the `channel-scope` fault and a pending event across a quit in the window | no smoke driver in release; not part of this run | an operator pasting tokens into a release window; a later run |
