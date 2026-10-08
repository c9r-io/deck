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
| `channel` | synthetic envelope → production parser → durable inbox → Board card; `[extensions] services started (1)` in the log | PASS 3/3 |
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
