# First-party extensions phase 1 — evidence index (2026-10-09)

Architecture note: `docs/first-party-extensions.md`.
Logs, mutation output, smoke app.logs and snapshots are kept outside the
repository in `~/c9r-io/deck-first-party-extensions-20261009/` (`E/` below).
No credential and no message body is in either place.

## Identity

| | |
|---|---|
| Baseline | `dcc3c06` (main, `release: 0.7.26`); working tree had two untracked docs entries that are not part of this work |
| Branch / worktree | `feat/first-party-extensions` in `../deck-extensions` |
| Commits | `f3e9918` implementation, `de60a7b` start log line, `b1c4547` real-service certification test (then this index) |
| Tested source | `b1c4547`, clean tree (`E/logs/gates-cand-final/head.txt`, `status.txt`, `status-after.txt`) |
| Real-app binary | `target/debug/deck-app` built from `b1c4547`, SHA-256 `b1bac3bf7dc6e127e65d62863c0a853d8e3b505877e9ad3592811fa4400dfb65`; staged frontend (`ui-dist`) byte-equal to `app/ui` (`E/scripts/smoke-cand-b1c4547/result.json`) |

## Gates

| Gate | Baseline `dcc3c06` | Candidate `b1c4547` | Evidence |
|---|---|---|---|
| `cargo fmt --all --check` | — | pass | `E/logs/gates-cand-final/fmt.log` |
| `cargo clippy --workspace --all-targets --all-features --locked -D warnings` | — | pass | `clippy.log` |
| `cargo test --workspace` (isolated HOME) | 984 + 208 pass, 2 ignored | 1001 + 208 pass, 2 ignored | `E/logs/baseline-cargo-test.log`, `gates-cand-final/cargo-test.log` |
| `node --check`, `scripts/ui-tests`, `node app/ui/js/check.mjs` | 685 pass | 685 pass | `E/logs/baseline-frontend.log`, `gates-cand-final/ui-tests.log` |
| Python release-tool tests | — | 108 pass | `py-unittest.log` |

The first baseline run overlapped my first edits and was discarded
(`baseline-cargo-test.contaminated.log`); the baseline above ran in a
separate detached worktree. No baseline failure; no candidate failure.
Not run: `scripts/check-workflows` and `cargo audit` (both fetch from
GitHub, which this task forbids; no workflow or dependency changed),
`cargo llvm-cov` thresholds, CE parity, compat-candidate packaging.

The Rust contract suites (`edr_quiet`, `log_privacy`, `external_admission`,
`signal_census`, `ipc_contract`, `session_architecture`) passed unchanged:
no allow-list, count or exclusion was edited, and the new files are inside
their recursive scans.

## Seam tests and negative controls

New tests (unit level, production functions): `extensions::tests` (5),
`inbound::tests` (4: status, credential owner, setup page, source pin),
`slack::tests` (2 + the gated certification), `slack_transport::tests::
the_transport_has_one_owner_per_process`, `intelligence::translation::
tests` (4), `agent_status::tests::
a_source_word_is_recognized_only_through_the_registered_adapter`.

Nine bypass mutations, each applied to production code and reverted
(`E/scripts/mutate.py`, `E/mutations/results.json`, one log per mutant):

| Mutant | Detected by |
|---|---|
| M1 status hard-codes Slack liveness | `status_reads_liveness_…` fails |
| M2 credential owner = first service | `a_credential_is_stored_only_…` fails |
| M3 parser uses a hard-coded source list | `a_source_word_is_recognized_…` fails |
| M4 coordinator builds Bergamot directly | two coordinator tests fail |
| M5 provider lookup falls back to the first entry | `lookups_answer_only_for_exactly_one_entry` fails (`results-m5b.json`); under the translation filter the process aborted in the native engine instead of failing an assertion |
| M6 second transport owner allowed | `the_transport_has_one_owner_per_process` fails |
| M7 `main.rs` starts the transport directly | same test fails |
| M8 lookup is first-wins | `lookups_answer_only_for_exactly_one_entry` fails |
| M9 unknown contributed source is polled | `contributed_sources_outside_…` fails |

## Real application (isolated debug bundle, real WKWebView, Mac mini)

Runner `E/scripts/smoke_run.py`; per mode `app.log`, verdict and snapshots
under `E/scripts/smoke-cand-b1c4547/`. Isolation by launch arguments
(own data dir, own `deck-smoke-*` tmux socket, bundle id
`io.c9r.deck.smoke`); credentials never reach the Keychain in a smoke
launch. The machine's own `~/.deck`, `deck` socket and running
`/Applications/deck.app` were identical before and after; no leftover
process; the run root went to that machine's Trash. Each run took the
foreground of the test machine while another app was in front.

| Mode | What it shows for this change | Result |
|---|---|---|
| `settings` | Settings sections render and navigate on the candidate | PASS 7/7 |
| `channel` | channel settings and rule editor; seeded events → production parser → durable inbox → the webview's drain, which keeps them pending because the target command is runtime-blocked (no card, session or queue row); `[extensions] services started (1)` in the log. Corrected in the closure round: this mode creates no Board card | PASS 3/3 |
| `signal-finish` | harmless fixture helper → `agent_status` ingest through the adapter table → run finish | PASS 8/8 |
| `connector`, `empty-start`, `approval` | Connector, the clock route and approval unaffected | PASS 5/5, 5/5, 12/12 |
| `run`, `ambiguous`, `restart` | general regression, queue ambiguity, restart | PASS 94/94, 2/2, 2/2 |
| `translation-native` / `setup-native-install` | enable, in-app download with the pinned SHA-256 verification, real Bergamot translation shown in the Lens | PASS 12/12 |
| `translation-native` / `setup-restart-enabled`, `setup-restart-disabled` | restart with the pack installed: persisted, translates again, disable, re-enable | PASS 4/4, 11/11 |

Earlier identical passes on `f3e9918` and `de60a7b` are in
`smoke-cand-f3e9918/` and `smoke-cand-de60a7b/`.

`channel-fault` FAILED on `f3e9918` and, the same way, on the baseline
(`smoke-baseline-dcc3c06/`): the mode waits 180 s for a real Slack
connection that an operator has to supply by pasting tokens into the
window. Pre-existing precondition, not a regression. BLOCKED as an
unattended real-app check: entering a real token into the isolated window
needs the shared clipboard or synthetic keyboard input, which this task
does not allow. `channel-first-send`, `authority-live`, `clock-live`,
the pasteboard-driven `translation-native` matrix
(`scripts/translation-lens-verify.py`), voice, buffer, attention, review,
reminder and board-lost modes were not run.

## Real engine and real service outside the WKWebView

- Real Bergamot through the registered constructor:
  `native_certification_with_explicit_synthetic_pack` with the pack the
  app itself downloaded and verified in the smoke above (four SHA-256
  values equal to `pack.rs`) — pass on `de60a7b`
  (`E/logs/cert-real-model-de60a7b.log`).
- Real Slack, authorized by the user during the task (sandbox workspace,
  a test app created from Deck's manifest and deleted afterwards, a new
  channel `#deck-ext-test`): `slack::tests::
  real_slack_certification_with_explicit_sandbox_credentials` — a forged
  token refused, three real tokens verified through the integration's
  `store_secret`, the production `socket_loop` connected over real Socket
  Mode and published the channel identity, one real channel message staged
  durably exactly once, and clearing the app credential ended the
  connection with the staged event still pending
  (`E/logs/slack-cert-run1.log`, run on the tree that became `b1c4547`).
  This is a Rust-level run of the production path, not a WKWebView run,
  and its channel consumer is the durable half (`stage_message`) without
  the window event.

No real agent was run; Signal used the bundled fixture helper only.

## Closure round (2026-10-09)

Final tested source: `a869fbe5ff524e056d334c5337cb41aa828967a1`
(`test(slack): transport faults after a live connection …`). Since
`b1c4547641443a496127511a3ad2bb0089168ff2` it adds test code only:
`src/documents/channel_admission_tests.rs` (a `cfg(test)` module),
`ui/test/dom.test.mjs` and `ui/test/fixtures/channel-fault-status.json`.
`2059695` before it and the commit carrying this section are documents.

### What earlier evidence still applies

- Real WKWebView runs were made on the debug binary built from `b1c4547`
  (SHA-256 `b1bac3bf7dc6e127e65d62863c0a853d8e3b505877e9ad3592811fa4400dfb65`).
  `git diff b1c4547 a869fbe` touches no production Rust, no `ui/js`, no
  native or vendored source, no manifest or lock file. A debug bundle also
  embeds `ui/test`, so a debug binary built from `a869fbe` is not
  byte-equal to the tested one: it differs by two files the smoke runner
  never loads. The runs were not repeated and no `a869fbe` debug binary
  was built.
- The real Bergamot run was made on `de60a7b`. `git diff de60a7b a869fbe`
  is empty for `src/intelligence`, `src/extensions.rs`, `native/`,
  `vendor/`, `build.rs`, `Cargo.toml` and `Cargo.lock`; the only Rust
  change between them outside the new test is inside `slack.rs`'s test
  module.
- The nine bypass mutations edit lines unchanged since `f3e9918`.
- Rerun on `a869fbe` because test code changed: the whole Rust gate in its
  coverage form and the frontend gates (below).

### Gates run on `a869fbe` (isolated `HOME`/`TMPDIR`, offline, clean tree before and after)

Logs: `E/logs/gates-cand-a869fbe-cov/` (`summary.txt` has every exit code).

| Gate | Result |
|---|---|
| `cargo fmt --all --check` | exit 0 |
| `node --check` on `ui/js`, `ui/js/i18n`, `ui/test` | exit 0 |
| `scripts/ui-tests` (coverage thresholds and inventory) | exit 0 |
| `node app/ui/js/check.mjs` | exit 0 |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | exit 0 |
| `cargo llvm-cov --workspace --all-targets --all-features --locked --fail-under-lines 75 --fail-under-functions 75 -- --test-threads=1` (the `gate.yml` command, cargo-llvm-cov 0.8.5) | exit 0; lines 83.60 %, functions 81.52 %, regions 84.61 %; `deck-app` 1002 passed, 0 failed, 2 ignored; every other target green |

Per file in that run (lines / functions): `extensions.rs` 81.68 / 79.66,
`slack_transport.rs` 87.05 / 77.55, `inbound.rs` 93.67 / 90.60,
`agent_status.rs` 94.72 / 95.36, `intelligence/translation.rs`
70.12 / 66.67, `intelligence/provider.rs` 27.23 / 65.00 (the native engine
runs only in the opt-in certification test), `slack.rs` 36.45 / 36.84 (its
largest function is the opt-in real-service test, which returns at once
without `DECK_SLACK_CERT_DIR`). The thresholds are workspace totals; no
file was excluded and no threshold changed.

The `DECK_CE_*` variables the CI step binds were not set, so the Full
Local B-admission probe wrote no evidence. Not run at all:
`scripts/check-workflows`, `cargo audit`, the Full Local parity steps,
the Python script tests on this commit (they passed on `b1c4547`; nothing
they read changed), packaging, signing, any release or compatibility
gate. This is not a passed CI run.

### Channel transport faults, by assertion

New test: `documents::channel_admission_tests::
a_transport_fault_keeps_the_gap_and_pending_work_through_recovery`. It
runs the production `slack_transport::socket_loop` against the existing
loopback Slack stand-in (`SlackFixture`: `auth.test`,
`apps.connections.open`, one Socket Mode endpoint). Credentials are stored
through `extensions::secret_owner(slot).store_secret`; the loop is parked
with nothing to collect and leaves that wait only on the wake
`inbound::inbound_check_now` sends through `extensions::wake_services`;
liveness is read from `inbound::inbound_status`
(`extensions::service("slack").live`). The fault is a real one at the
protocol level: the stand-in drops the socket without a close frame and
the next ticket request either gets no reply (`network`) or Slack's
`missing_scope` (`scope`).

| # | Assertion | Evidence | Layer |
|---|---|---|---|
| 1 | network fault after a live connection | the test: `connected:false`, `lastError:"network"`, service not live | Rust, production loop, loopback protocol |
| 2 | scope fault after a live connection | the test, second pass: `lastError:"scope"` | same |
| 3a | production status says disconnected, the code, an unresolved gap | the test compares `slack_connection_status` and `channel_status`, as serialized for the webview, with `ui/test/fixtures/channel-fault-status.json` | same |
| 3b | Settings shows not connected (no backfill), the gap, the code | `ui/test/dom.test.mjs` renders that same file through the production `settings.js`, `slack-connection-model.js` and `i18n.js`; `i18n.test.mjs` pins the "no backfill" wording | node, production modules, fake DOM |
| 3c | the same row inside a real WKWebView with a live fault | BLOCKED | — |
| 4 | clearing the fault reconnects | the test: a new socket, `connected:true`, `lastError:null` | Rust |
| 5 | the gap stays after reconnecting | the test: `gapUnresolved:true`, `gapSince` equal to the first fault's across both faults | Rust |
| 6 | no wrong ACK, duplicate, lost pending work or new authority | the test: three envelopes acknowledged once each, a redelivered event adds no second inbox entry, one pending entry through both faults, no `firstSendGrant` on any entry or in settings, `rejectedCount` 0. A failed durable stage sends no ACK: `slack_transport::tests::fake_socket_interleaves_consumers_and_never_acks_failed_stage` and `shared_ack_contract_stages_channel_first_and_keeps_reaction_recoverable` (unchanged) | Rust |

Negative controls (`E/mutations-closure/`, each restored afterwards):

| Mutation | Result |
|---|---|
| C1 `inbound_status` reads the poll's liveness instead of the service's | test fails: `timed out: connected` |
| C2 a reconnect clears `gap_since` | test fails: `reconnecting backfills nothing` |
| C3 the socket loop does not report a drop | test fails: `timed out: disconnected with the closed code` |
| C4 `inbound_check_now` does not wake the services | test fails: `timed out: connected` |
| C5 Settings omits the gap | `dom.test.mjs` fails on `network` |

Not covered, and why:

- 3c. `LOOPBACK_SOCKET` and `TEST_API` exist only under `cfg(test)`, so the
  application binary reaches a fault state only from a real Slack
  connection. That needs real tokens typed into the isolated window (this
  round uses none), or a debug-only alternative Slack address in the
  binary, which was not added: it would be a service address setting.
- The debug-only branch `injected_channel_fault` (it returns the code from
  inside the read loop) was not executed; the test reaches the same two
  codes through real protocol failures instead.
- Board card creation from a channel event was not exercised in a window,
  with or without a fault: the real-app `channel` mode (PASS on `b1c4547`)
  seeds events whose target is runtime-blocked and checks that they stay
  pending with no card. The inbox holds one entry per event; the creation
  path (`inbound.js`, `provider.createStarted`) is unchanged and is covered
  by the frontend unit tests only.
- Read from the code, not tested: when `auth.test` fails as well (a total
  outage), the loop waits 60 s per attempt without reporting again, so the
  status keeps the socket-drop code (`socket`, `closed` or `stalled`), not
  `network`. Same in the baseline.
- The `reannounce` seam is not in this test (it needs a window handle); the
  real-app `channel` mode runs with the poll loop that calls it.

### Real-app modes not run: effect of this change

| Mode | Paths of this change it crosses | Covered by |
|---|---|---|
| `channel-first-send` | service start, channel staging, grant freeze, card creation | `channel` (real app, up to the pending drain; no card); `the_first_first_send_rule_is_saved_by_one_save_from_zero_rules` (production loop, loopback); grant code unchanged |
| `authority-live`, `clock-live` | `inbound_status`, source list (`service_sources` + Clock) | `approval`, `empty-start` (real app); `inbound::tests` registry tests |
| pasteboard-driven translation matrix | coordinator → registered provider | three `translation-native` setup scenarios (real app, real engine); pasteboard and Lens code unchanged |
| voice, buffer, attention, review, reminder, board-lost | none | not affected |

These are impact statements, not runs.

## Correction round: registration clashes and the re-announcement seam (2026-10-09)

Tested source: `ea5a065a151b7bdf75343b1140ef51392994943c`
(`fix(extensions): an identity two entries share is used by no consumer …`).
It changes production code, so the real-app evidence from the `b1c4547`
binary is not relabelled: the affected modes were run again on a binary
built from `ea5a065`.

### The gap

A contract defect in how compiled-in tables are consumed. The shipped
tables have no clash and no user action can create one; no user-reachable
fault is claimed. Reproduced with the `cfg(test)` table override and
side-effect-free counters, before the fix
(`E/logs/conflict-repro/before-fix.log`, `before-fix-each-case.log`):

| Malformed table | Before the fix | `check` said |
|---|---|---|
| one service, the same source name twice | the first was polled | `duplicate-source` |
| two services, one source name | the first was polled | `duplicate-source` |
| a service contributing `clock` | polled next to the host's own clock | nothing (`Ok`) |
| two entries under one service id | both woken; the one with a settable slot owned the credential | `duplicate-service` |

`check`'s error stopped only `start_services`; `spawn_inbound`,
`wake_services`, `reannounce_services` and `secret_owner` read the table
on their own. The re-announcement loop had the same shape as the wake
loop; it could not be called from a test then (its callback took a window
handle), so it is shown fixed, not shown broken.

### The fix (`app/src-tauri/src`)

- `extensions.rs:139` `unambiguous_services`: the services whose id exactly
  one entry carries. Used by `secret_owner` (`:144`), `service_sources`
  (`:173`), `start_services` (`:213`), `wake_services` (`:227`),
  `reannounce_services` (`:235`).
- `service_sources`: a name is kept only when one contributed source
  carries it, `inbound::SOURCES` names it and it is not in
  `inbound::BUILT_IN_SOURCES` (`inbound.rs:133`, the clock);
  `check_tables` names the last case `reserved-source` (`:279`).
- `extensions.rs:156` `unambiguous_agents`, used by the one whole-table
  agent pass, `agent_status::migrate_hooks_on_boot` (`agent_status.rs:2207`).
  That function returns early outside a release build, so only the iterator
  is tested.
- `ServiceIntegration.reannounce` is now `fn(&dyn Fn(&'static str))`
  (`extensions.rs:80`): the service names the event, the host emits it.
  `inbound_channel::reannounce_pending` (`inbound_channel.rs:1669`) keeps
  its condition (durable pending work exists) and its event
  (`channel-changed`). `reannounce_services` is generic over the runtime so
  a test can call it. No IPC, setting, event name, order or authority
  changed for the shipped tables.

A clash removes only the identities in it; a malformed table still starts
no connection owner (unchanged).

### Tests and negative controls

- `extensions::tests::a_source_name_is_polled_only_when_exactly_one_registered_source_carries_it`
  and `…::a_service_id_two_entries_share_takes_no_part_in_dispatch`:
  failed before the fix, pass after. The second also counts wake calls,
  re-announcement calls and emitted events on a mock runtime, and the agent
  pass.
- Re-announcement, in two layers:
  1. Dispatch and the existing pending behaviour, by behaviour:
     `documents::channel_admission_tests::durable_pending_channel_work_is_reannounced_through_the_registered_service`
     calls `extensions::reannounce_services` over the production table on a
     mock runtime with a listener: nothing for an empty inbox, exactly one
     `channel-changed` per pass while an event staged by the production
     `stage_message` is pending, still one with the channel connection
     switched off and no socket ever opened, nothing after `channel_ack`.
  2. That the poll thread makes that call once per tick:
     `inbound::tests::the_poll_loop_reannounces_through_the_tables_once_per_tick_and_owns_the_clock`,
     a source pin. The thread needs a real window handle, so this link is
     not exercised by behaviour.

`E/mutations-conflict/` (each restored afterwards):

| Mutation | Failing test |
|---|---|
| R1 Slack's `reannounce` is a no-op | `durable_pending_…` |
| R2 the host does not call the callback | `durable_pending_…`, `a_service_id_…` |
| R3 re-announcement requires the connection to be enabled | `durable_pending_…` |
| R4 re-announcement walks the raw table | `a_service_id_…` |
| R5 wake walks the raw table | `a_service_id_…` |
| R6 `secret_owner` ignores an id clash | `a_service_id_…` |
| R7 the clock is not reserved | `a_source_name_…` |
| R8 a source clash keeps one | `a_source_name_…` |
| R9 the poll loop drops the call | `the_poll_loop_…` (source pin) |
| R10 the agent pass walks the raw table | `a_service_id_…` |

Of the original nine, M2 and M9 edited lines this fix rewrote; R6–R8
replace them. The other seven edit lines unchanged in `ea5a065` and were
not rerun.

### Gates on `ea5a065` (`E/logs/gates-cand-ea5a065-cov/`)

fmt, `node --check`, `scripts/ui-tests`, `check.mjs`, CI-form clippy: exit
0. `cargo llvm-cov` in the `gate.yml` form: exit 0, lines 83.65 %,
functions 81.66 %, regions 84.66 %; `deck-app` 1004 passed, 0 failed, 2
ignored; `extensions.rs` lines 89.05 %, functions 87.67 %. Same deviations
as the closure round (no `DECK_CE_*`, offline, isolated `HOME`). Not run:
the Python script tests, `check-workflows`, `cargo audit`, Full Local
parity, packaging, any release gate.

### Real application on the `ea5a065` binary

Debug `deck-app` SHA-256
`05fb79429b1b42884f04e9e1d82d62f03fd52130d375002ebd7733804d1e6468`, staged
frontend equal to `app/ui`; Mac mini, run `v3c027808`
(`E/scripts/smoke-cand-ea5a065/`): `settings` 7/7, `channel` 3/3,
`empty-start` 5/5, `approval` 12/12, `signal-finish` 8/8, all PASS;
`[extensions] services started (1)` in the fetched logs; no leftover
process; that machine's `~/.deck` listing, `deck` socket and running Deck
the same before and after. The run took the foreground while another app
was in front. Not rerun on this binary: `connector`, `run`, `ambiguous`,
`restart` and the three translation scenarios. Every launch crosses the
changed start and source functions, which the five rerun modes exercised;
what those other modes check beyond launch touches no changed function.
Their evidence stays that of the `b1c4547` binary and is not evidence
about this one.
The real Bergamot and real Slack runs were not repeated: translation
source is unchanged; the Slack run's path now goes through the changed
`secret_owner` and `wake_services`, which the loopback test above and the
transport-fault test cover on `ea5a065`.

### Statement corrected: tmux

The closure round's note that no tmux was started is wrong as written.
No tmux server was started by hand, but the Rust suite starts isolated
ones itself (`tests/tmux_contract.rs`, unique `-L` names under the
isolated `TMPDIR`; each `Server` guard runs `kill-server` and removes its
socket on drop). After the closure round nobody looked for leftovers.
After this round's coverage run a process listing showed no tmux process
with the isolated `TMPDIR` in its arguments; sockets were not inspected.
