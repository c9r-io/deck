# Close-out evidence, 2026-10-11

Three things were closed out in one round on top of `a0fe806`:

- **A. Shared build outputs** — the sidecar pollution: a coverage run left
  instrumented helpers in `binaries/` and an ordinary build went on packaging
  them. Fixed, with standing checks.
- **B. The first-party Slack integration in the real window** — a sandbox
  Slack app, its real Socket Mode connection and real messages, through the
  registered integration, in a real WKWebView.
- **C. Claude session re-entry on the installed path** — the hook families,
  their migration and a private signed release carrier. The transaction with
  a real client at the formal location stays open. Its acceptance summary is
  [reentry-acceptance.md](reentry-acceptance.md).

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

## Source and artifacts

| | |
| --- | --- |
| Base commit | `a0fe80685fb8f684e4658238cfa3a8c8fa018abb` (`main`, clean at the start) |
| Commit carrying this round | the commit that adds this file (`git log -1 -- docs/evidence/closeout-20261011/README.md`); not pushed, not tagged, not released |
| Machines | build and Slack driver: macOS 27.0.1 (26A434), arm64. Real windows: the Mac mini test machine, macOS 27.0.1 (26A434), arm64 |
| Toolchain | cargo 1.97.1, cargo-llvm-cov 0.8.5, tauri-cli 2.11.4 |
| Debug carriers | `target/debug/deck-app` of the working tree in an ad-hoc signed bundle, identifier `io.c9r.deck.smoke`; plain sidecars (checked before every assembly) |
| Release carrier | the tauri bundler's release bundle, identifier changed to `io.c9r.deck.smoke`, Developer ID signed, not notarized; plain release sidecars, no `ui/test` inside |

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

## Verdict

| Part | Verdict |
| --- | --- |
| A. Shared build outputs | **PASS** — reproduced, fixed, sequence 68 of 68 checks, packaging guarded |
| B. Slack in the real window | **PASS** on macOS 27 with a debug carrier — all six items; limits in B |
| C. Re-entry, hook families and migration | **PASS** for what ran (release carrier and real window under a relocated home) |
| C. Re-entry, transaction at the formal location | **BLOCKED** — owner's installed Deck in the way on both machines, no UI operator, no signed-in Claude under an isolated home |
| C. macOS 26 | **BLOCKED** — no machine; the gate's runner needs a push that was not authorized |
| C. Notarization, Gatekeeper acceptance, public carrier | **UNRUN** — not authorized; Gatekeeper's verdict on the carrier is recorded as "rejected, unnotarized" |
| Architecture and regression checks | **PASS** — final gate below |

One incident happened on the test machine during C; it is recorded in
"Incident" and is not softened by anything above.

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
  have run the coverage run's own instrumented sidecars).
- Tests: `tests/shared_output.rs` (13: the rule against cargo's
  modification-time check, and a source pin that `build.rs` writes through
  the module), `scripts/test_sidecar_provenance.py` (18, in the gate's
  unittest step).

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
evidence, this round's release carrier and its signature matrix, the
launch-time migration scenarios M1–M4, the Settings switches in the real
window, the eleven items one by one, and why the formal path is blocked.

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

## Incident

At 14:12 on the test machine, the cleanup of the migration runner ran
`tmux -L deck kill-server` with the carrier's tmux but WITHOUT the run's own
socket directory. A release build always names its socket `deck`, so the
command reached the socket of the Deck formally installed on that machine
and stopped its shell service (the server started on Oct 10 16:55). Its
sessions ended and cannot be brought back; the application itself kept
running and its `~/.deck` was not opened. The run's "state as before" line
was wrong: it recorded the application process but not its tmux server.
The owner was told at once. The runners now stop only a server whose
command path is under the run's root, with the socket directory in the same
command, and record every process of the installed bundle.

## Resources and cleanup

Created this round and removed or left as stated:

| Resource | End state |
| --- | --- |
| Slack test app and its tokens, private test channel (sandbox workspace) | app deleted (its bot token then answered `account_inactive`); channel archived, which is reversible; the workspace's three existing apps were not opened and its sidebar is as it was |
| Separate browser profile holding the owner's Slack sign-in | signed out of the workspace; that browser instance quit by its own pid; profile directory removed |
| Test machine: three smoke roots, one relocated-home root per C run, their tmux servers | servers stopped; roots moved to that machine's Trash (recoverable); no process left |
| Test machine: Claude's own record of the directories the Slack rule started it in | not looked for beyond a count of matching project directories (0) |
| Build machine: carrier staging directories, private token files, isolated cargo home, probe directories | removed by exact path |
| `app/src-tauri/target/` (release, debug and coverage products) | left; `binaries/` and `ui-dist/` are the ordinary debug build |

## Open items

| Item | Why it is open | Smallest thing that closes it |
| --- | --- | --- |
| Re-entry transaction (items 5–11) in a release build at `/Applications/deck.app` | owner's Deck installed and in use on both machines; no UI operator; needs a signed-in Claude under the same home | the owner frees one machine's installed Deck for a bounded window, signs Claude in there, and either operates the dialog or provides a UI operator |
| A non-empty queue across a restart in a real application; a second preview and a relaunch after Policy B | same | part of the same window |
| macOS 26 | no machine | a push so the gate runs on its `macos-26` runner, or a macOS 26 machine |
| Notarization and Gatekeeper acceptance of a carrier | not authorized; needs Apple credentials | an authorization to submit a private carrier |
| Slack in a release build; the `channel-scope` fault and a pending event across a quit in the window | no smoke driver in release; not part of this run | an operator pasting tokens into a release window; a later run |
