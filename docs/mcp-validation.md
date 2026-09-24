# MCP validation record

Date: 2026-09-21. Status words below are evidence labels, not forecasts.

Historical signed-smoke results below apply to protocol v1. They are not proof
that the current control-protocol-v5 scheme-B UI has been exercised in an
installed signed app.

## Scheme-B remaining-requirement matrix

The current source uses control protocol v5, state schema v6 and runner
protocol 3. v5/v6/3 makes structured direct launch the default and gates the
arbitrary-shell fallback behind a separate local approval. The MCP standard protocol version is negotiated independently by
the SDK. Rows below describe the v3-era evidence; the 2026-09-21 remediation
section supersedes them where they differ.

| ID | Status | Implementation evidence | Automated evidence | Remaining limit |
|---|---|---|---|---|
| R01 | Implemented and verified | `mcp_close_admit`, `validate_close_admission`, `provider.close`, `queue_clear_sessions`, `kill_session` | `production_routes_cover_authorized_job_and_control_lifecycle`; `app/ui/test/mcp.test.mjs` | Signed WebView exercise remains R12. |
| R02 | Implemented and verified | runner `DispatchContext`, service identity, epoch/holder/grant fences and `RevokeGrant` | `stale_epoch_and_old_service_are_fenced_before_spawn`; `duplicate_dispatch_returns_one_job` | Cross-process loss is reported uncertain, not made transactional. |
| R03 | Implemented and verified | nonblocking script/stdin pipes, `bounded_write`, per-chunk stdin recheck | `script_pipe_write_has_a_deadline_when_reader_stalls`; runner black-box stdin test | Bytes already accepted by a pipe cannot be recalled. |
| R04 | Implemented and verified | encoded-frame checks, 32-KiB script and 16-KiB read budgets, response-sized search pages | `encoded_frames_fit_the_advertised_budget`; runner UTF-8 paging test | Transport failure remains non-retriable for side effects. |
| R05 | Implemented but pending designated-environment verification | public client id plus Keychain bearer; Adapter Keychain/credential-FD read; wire authentication | Adapter STDIO synthetic credential-FD test; bad-credential route test | A signed installed Adapter/Keychain ACL prompt is R12. The credential-FD carrier is explicit and intended for isolated harnesses; ordinary launches read Keychain. |
| R06 | Implemented and verified | `control_holder`, strict action-specific Request/Renew/Release validation and exec/stdin/close fencing | Adapter STDIO independent request samples; holder-conflict and control lifecycle assertions in `production_routes_cover_authorized_job_and_control_lifecycle` | Holder is a caller-generated candidate flow identifier, not a second human identity or evidence of granted control. |
| R07 | Implemented and verified | `inspect` derives `mayStartNextJob` and exposes separate read-only execution-authorization and session output-sharing states | `inspect_separates_execution_authorization_from_output_sharing`; production route lifecycle assertions | Inspect never acquires or renews control; the output session gate is not a job-binding decision. |
| R08 | Implemented and verified | monotonic 750-ms search budget plus per-loop authorization callback and explicit incomplete result | `controlled_search_reports_deadline_and_cancellation` | Deadline cannot cancel an already-entered uninterruptible kernel read. |
| R09 | Implemented and verified | bounded metadata-only `AuditEvent`; approval/revoke/expiry/control/intent/dispatch/takeover/denial events; memory-first emergency fences remain effective when state persistence fails | `grant_expiry_is_audited_once_without_reviving_authority`; `emergency_fence_survives_state_write_failure`; audit-kind assertions; privacy gates | A failed persistent revoke is effective only for the current service instance and is reported as unconfirmed across restart. Same-UID state is not tamper-proof audit. |
| R10 | Implemented and verified | live configurable output retention; expired bytes preserve job/idempotency metadata and report a gap | `expired_output_reports_a_gap_without_deleting_job_metadata` | tmux scrollback is intentionally separate. |
| R11 | Implemented and verified | `guard_terminal_input` at PTY and prompt-delivery boundaries; scheduler/voice/Phone reuse prompt delivery; output sharing gates inspect/job reads | repository PTY, scheduler, connector, UI and MCP tests | Full GUI/runner/tmux restart experience remains subject to R12. |
| R12 | Implemented but pending designated-environment verification | updated `scripts/mcp-e2e.mjs` uses production Adapter/control/runner route and synthetic credential FD | Script syntax/static checks only in this workspace | Requires an isolated signed app, WebView and separately authorized real Tunnel/account; not run here. |

## Side-effect linearization

- Exec is only recorded as an intent at `accepted`. Its final admission is the
  delivery-locked authorization recheck immediately before runner dispatch;
  the runner independently rejects a stale service/epoch/holder/grant context.
- Stdin is accepted in the ledger, rechecked before dispatch, and admitted in
  at most 4-KiB pipe chunks. Takeover/revoke stops remaining chunks; written
  bytes are reported as potentially effective.
- Session creation reserves request id, capacity and plan in one state write.
  The first tmux side effect is inside the Board `beforePersist` transaction.
- Remote close is merely `executing` until the Board mutation slot calls
  `mcp_close_admit`. Queue cancellation is the first effect after that point;
  both native effects validate the same target-bound token.
- Revoke/takeover/disable close the in-memory gate BEFORE waiting for the
  delivery lock, then fence the runner. A missing runner acknowledgement
  returns an uncertainty error and never claims that a running process
  stopped.
- Output reads authenticate before lookup, check the job binding and the
  session sharing gate, and run the same gate again after the runner read
  returns; a takeover, pause, revocation or generation change during the wait
  drops the bytes. (Before the remediation the second check did not exist,
  although this record claimed it.) Retention gaps are explicit; transmitted
  bytes cannot be recalled. Inspect returns no terminal content.

## Automated evidence

- **PASS** — official-SDK STDIO adapter initializes, lists all 15 tools,
  advertises strict schemas and annotations, rejects unknown arguments, calls
  a separate mock Deck service, returns structured content, and emits only MCP
  JSON on stdout: `cargo test -p deck-mcp --test stdio`.
- **PASS** — production runner executes structured direct children and an
  explicitly selected real zsh fallback, ignores a forged
  completion-looking output line, reports exit 17, routes interactive input to
  the exact job, refuses late input, sends SIGINT to the owned process group,
  and confirms signal 2: `cargo test -p deck-mcp-runner --test runner`.
- **PASS** — direct isolated runner exercise covered Unicode/multiline output,
  exit 0, exit 23, cursor read, interactive input, late-input rejection,
  interrupt, human takeover, and return.
- **PASS** — a fresh `deck-smoke-mcp-e2e-20260920` tmux server, private smoke
  data directory, disposable Git repository, production Deck WebView, bundled
  Adapter, and bundled runner completed the real protocol loop. The client
  discovered 11 tools, created a visible card despite the project's `codex`
  default, observed failing exit 1, fixed the file and observed exit 0, read
  the real diff, continued a running job by cursor, supplied bound Unicode
  stdin, refused late stdin, confirmed an interrupt-requested exit 130, and
  closed through the Board transaction. The final persisted Board had zero
  cards and the smoke socket had zero sessions; final cleanup then stopped the
  deliberately persistent empty smoke server.
- **PASS** — local UI takeover fenced epoch 1; a real Adapter call received
  `CONTROL_REVOKED`. After the human shell exited, **Return to MCP** issued
  epoch 3, a new MCP job exited 0, and the MCP close operation committed.
- **PASS** — the isolated `app.log` and `mcp.json` contained none of the test
  script, Unicode stdin, test output, or the deliberately forbidden stale
  command. Data directory/file/socket modes were 0700/0600/0600.
- **PASS** — `cargo test --workspace`, `cargo clippy --workspace --all-targets
  -- -D warnings`, `scripts/ui-tests`, and `node app/ui/js/check.mjs` passed.
- **PASS** — the checked-in independent client `scripts/mcp-e2e.mjs` repeated
  the production loop and additionally proved same-id replay returned the same
  job while changed arguments returned `REQUEST_ID_CONFLICT`.

The repository-wide gate results and any environmental blockers are recorded
in the implementation handoff for the change that introduced this document.

## Protocol-v3 workspace verification (historical, before F1–F4)

This rerun predates control protocol 4 / state schema 5; its counts are kept
as recorded. The 2026-09-21 isolated rerun used synthetic principals, private temporary
directories, independent Unix sockets and test-owned tmux servers. It did not
start the installed app or read the user's Deck state.

- **PASS** — `cargo test --workspace -- --test-threads=1`: application tests,
  EDR, log privacy, session architecture, bundled-tmux contract, Adapter,
  runner and status helper passed. A first run exposed a test-only debug
  environment credential carrier; it was removed, replaced by the explicit
  credential-FD harness, and the complete workspace rerun passed.
- **PASS** — `cargo test -p deck-app mcp::tests -- --test-threads=1`: 17 MCP
  control tests, including strict control-argument side-effect checks, scope
  preview classification, execution-authorization observation, and the
  existing expiry/denial audit assertions.
- **PASS** — `cargo clippy --workspace --all-targets -- -D warnings` and
  `cargo fmt --all`.
- **PASS** — `scripts/ui-tests`: 247 tests with the repository coverage gate;
  `node app/ui/js/check.mjs`: 40 production modules.
- **PASS** — syntax checks for changed UI modules and `scripts/mcp-e2e.mjs`;
  `python3 scripts/test_release_tools.py`: 16 tests.
- **NOT RUN** — installed signed-app/WKWebView/Keychain ACL and real Tunnel
  acceptance. Those require the separately authorized environment described
  below; source and isolated automation do not substitute for that result.

## 2026-09-21 remediation of acceptance blockers (control/runner group)

Isolated evidence only: private temporary directories, bundled tmux on unique
`deck-smoke-h1-*` sockets with `-f /dev/null`, synthetic service instances.
The installed app, `~/.deck`, the production `deck` socket and the retained
acceptance session were not touched.

- **Return to MCP after the window expired (the observed failure).** Root
  cause: `mcp_return_control` required an active execution grant. Fixed; the
  return needs no grant, persists a new epoch with no holder first, re-fences
  on runner failure, and reports stable codes. Tests:
  `return_after_takeover_needs_no_grant_and_restores_nothing`,
  `return_is_refused_with_stable_codes_and_changes_nothing`.
- **Human control / signals (runner).** Before (isolated tmux experiment,
  pre-fix runner): runner pid == pgid == tpgid (foreground), job pgid ≠ tpgid;
  in human mode `send-keys C-c` killed the RUNNER and left the job group
  (zsh + `sleep 600`) orphaned under PPID 1; `kill-session` and SIGTERM to the
  runner orphaned it the same way. After: C-c interrupted the job group and
  the runner stayed alive in human mode; `kill-session`, SIGTERM and the new
  `stop` request each left no process of the job group. No stray process or
  tmux server remained. Tests: `human_interrupt_key_reaches_the_job_group_not_the_runner`,
  `runner_termination_kills_the_live_job_group`,
  `stop_escalates_and_reaps_the_job_group`,
  `a_job_stopped_by_job_control_is_reported_and_still_stoppable`,
  `human_takeover_never_starts_a_shell_job`.
- **Large requests to the runner.** Before: 100/100 execs with a 32-KiB script
  failed (accepted socket inherited O_NONBLOCK). After: 0/100 exec and 0/100
  16-KiB read failures. Test:
  `large_scripts_and_full_reads_cross_the_accepted_socket_intact`.
- **Restart staleness.** `a_restarted_deck_is_reported_stale_but_can_still_stop`,
  `a_runner_from_before_a_restart_is_stale_but_still_fenced_and_stoppable`.
- **Emergency ordering.** `takeover_fences_before_waiting_for_an_in_flight_dispatch`.
- **Output after takeover.** `job_read_drops_output_when_a_takeover_lands_during_the_wait`.
- **Remote close.** Before: `close_admit` wrote `admitted`, `mcp_complete`
  accepted only `executing`, and the webview swallowed the error, so no
  remote close could commit. Tests:
  `a_remote_close_commits_after_admission_and_never_sticks`,
  `restart_turns_pending_board_operations_ambiguous`, and the UI tests in
  `app/ui/test/mcp.test.mjs`.
- **Journal lifetime.** `compaction_keeps_three_thousand_epochs_bounded`,
  `sustained_use_stays_within_the_journal` (700 routed cycles, beyond the
  pre-fix limit reached at cycle 667), `many_grants_never_block_a_takeover`,
  `a_retired_request_id_is_rejected_never_reexecuted`,
  `a_full_journal_still_admits_an_interrupt_and_names_a_real_recovery`,
  `v3_state_upgrades_stickily_and_future_state_is_refused`,
  `a_control_replay_never_resends_a_runner_side_effect`.
- **Adapter.** `a_lost_answer_to_a_side_effect_is_ambiguous_not_unavailable`;
  registry/annotation assertions in
  `initializes_lists_and_calls_over_stdio_without_stdout_noise`. A release
  `deck-mcp` exits 64 for `--socket` and `--credential-fd`.
- **NOT RUN** — signed app, WKWebView, Keychain and real Tunnel acceptance
  (see below), and the E2E client against an isolated app.

## 2026-09-21 F1–F4 follow-up (replay identity, revocation admission)

Source-level and isolated-test evidence only (fake runner, temporary state
directories, loopback TLS); no signed app, WebView, real tmux, Keychain,
Tunnel or phone was used. Protocol changes were approved by the maintainer.

| Finding | Change | Counterexample tests (current source) |
|---|---|---|
| F1 exec/stdin | a pending execution revocation fences exec/stdin at the final admission (counted per revocation; an approval lifts only an earlier unpersisted one); exec also requires the grant its intent was accepted under, and every final-admission refusal is journaled `rejected` | `f1_exec_past_route_is_stopped_by_a_pending_execution_revoke`, `f1_stdin_past_route_delivers_no_bytes_after_a_pending_execution_revoke`, `f1_an_approval_never_clears_a_revocation_that_persists_after_it`, `f1_revocation_after_the_commit_point_is_ordered_after_the_dispatch`, `f1_a_pending_execution_revoke_leaves_reads_and_interrupt_alone`, `f1_a_job_bound_to_a_revoked_grant_never_revives_under_a_new_grant` |
| F1 close | close admission, validation and create start re-check disable, client revocation and takeover under the delivery lock | `f1_close_admission_rechecks_takeover_revocation_and_disable` |
| F2 | control actions carry `control_sequence`, creates `create_sequence` (protocol 4, state 5); ambiguous closes outlive the result window | `f2_a_retired_control_request_is_never_applied_again`, `f2_a_retired_create_is_never_accepted_again`, `f2_an_ambiguous_exec_is_never_dispatched_again`, `f2_an_ambiguous_close_outlives_the_result_window`, `a_retired_request_id_is_rejected_never_reexecuted` |
| C1 | renewals are journaled (real `operationId`), superseded by the sequence | `c1_an_exact_renew_replay_never_extends_the_lease` |
| F3 | control pool outside the ordinary pool; lapsed epochs closed under pressure; per-client interrupt reserve cap | `f3_a_full_client_quota_recovers_by_release_and_request`, `f3_a_full_global_pool_recovers_by_release_and_request`, `f3_a_lapsed_lease_at_full_capacity_recovers_by_request_without_a_new_grant`, `f3_interrupt_reserve_survives_a_full_pool_and_one_greedy_client` |
| F4 | phone `seq` checked by POST admission against the device floor (journal format 3; 426 without `seq`) | `f4_a_retired_command_is_never_admitted_again`, `f4_concurrent_identical_posts_admit_once`, `f4_crash_boundaries_never_admit_twice_or_guess_success`, `f4_http_post_admission_refuses_a_retired_command_without_a_prior_get` |

Remaining: C2–C5 are unchanged; the iOS Swift Testing target and the app
target were not built here (no Xcode); signed-app/WebView/Keychain/Tunnel
behaviour still needs a new signed nightly.

Follow-up on the same protocol (no version change): `deck_session_inspect`
reports a fenced-but-unpersisted execution revocation as `revoked`
(`active=false`, no stdin, `EXECUTION_GRANT_REQUIRED`) —
`inspect_reports_a_pending_execution_revoke_as_revoked`. Phone side: a
version-1 journal record without `seq` gets one only from a host `404`,
persisted before its retry, and a `410` is the terminal local state
`expired` — `CommandJournalTests.swift` (Swift Testing, needs Xcode) and an
isolated logic check against a fake host through the public
`DeckConnectorCore` API.

Last pre-nightly patch (no MCP protocol, MCP state or Connector wire/host
journal change): execution revoke no longer closes the output-sharing gate —
`execution_revoke_leaves_output_sharing_and_a_running_job_alone` (sharing
unchanged, completed output still read through its binding, exec
`EXECUTION_GRANT_REQUIRED`, stdin `STDIN_NOT_AUTHORIZED`, no interrupt or
stop), `execution_revoke_never_opens_output_sharing`, and the persisted step
of `inspect_reports_a_pending_execution_revoke_as_revoked`. The phone's
local journal is schema 3 because it may now hold `expired`: v1/v2 files
open unchanged and are written as 3 by the next atomic save, any other
version is refused before its records, and v1/v2 files holding `expired` are
refused — `CommandJournalTests.swift`, run here through an executing
macro stand-in (not Swift Testing), plus the b103255 build refusing a v3 file
without touching it.

Current source also makes the two output-sharing layers observable without a
protocol or state-schema change: inspect reports open/closed retained job
binding counts beside the session gate, and job reads distinguish
`SESSION_OUTPUT_SHARING_PAUSED` from the permanent
`JOB_OUTPUT_BINDING_CLOSED`. `natural_expiry_leaves_completed_retained_output_readable`
proves expiry alone leaves retained output readable;
`reapprove_then_expiry_keeps_takeover_closed_job_distinct_from_fresh_job`
covers takeover → return → reapprove → fresh job → expiry; and
`session_pause_is_distinct_from_an_open_job_binding` binds the two denial
classes.

## 2026-09-25 isolated E2E after FR-4 (286a9c6)

A debug `deck-smoke.app` on a private data directory and the
`deck-smoke-mcp-e2e` tmux socket, a disposable Git repository, and a client
and project authorized in that instance's UI; the credential was read from
the login Keychain into a 0600 file and passed through `--credential-fd`.
A person approved the execution window in the UI.

- **PASS** — `scripts/mcp-e2e.mjs`: 14 tools, failing exit 1 then fixed
  exit 0, same-id replay returned the same job, incremental read first saw
  `running`, interactive stdin exit 0, interrupt exit 130, close
  `committed`, zero adapter stderr bytes.
- The script now requests the maximum 5-minute lease and renews it while
  waiting for the approval. The first attempt waited past the default
  60-second lease, and `mayStartNextJob` stayed false on
  `CONTROL_LEASE_EXPIRED`, as it did before FR-4.
- The data directory's path must keep `mcp-control.sock` within the macOS
  104-byte Unix socket limit; a longer one never binds the control socket.
- **Found, fixed in the next commit (`fix(tmux)`, test
  `real_tmux_probe_of_an_emptied_server_ignores_a_stale_query_client`):** once the last session on the Deck tmux server closes,
  every new session in that Deck process (MCP and ordinary cards, one
  `session_creation_guard`) fails with `tmux-server-unreachable` until
  restart. The query channel's control client has exited but
  `OWNED_CONTROL_CLIENT` still names it, so `probe_server` runs
  `list-clients`, which on an empty tmux 3.7c server exits 1 with
  `no current target`; that is not classified as absent. The front end
  reports it as `create-failed` / `rejected` because `createStarted` tags
  only object errors, not the string an `invoke` rejects with.

## Ordinary ChatGPT manual acceptance

Status: **MANUAL_PENDING**. Protocol and local execution can be automated;
account login, workspace policy, Tunnel association, model tool availability,
and ChatGPT confirmations require the user.

1. Start the installed Deck app; enable MCP and authorize only a disposable
   test project.
2. Install/start the official Secure MCP Tunnel client according to its current
   documentation. Configure it to launch Deck's copied STDIO command. Do not
   expose a public endpoint.
3. In ChatGPT, enable Developer mode under **Settings → Security and login** if
   available. Record account/workspace policy restrictions.
4. Open ChatGPT Plugins, add a Tunnel connection, select the tunnel id, review
   the discovered tools, then start a new ordinary Chat conversation with that
   connection enabled.
5. Record client/version, mode (`Chat`, not Work/Codex), model, transport,
   authorization method, write-confirmation behavior, and selected tools.
6. Ask for capabilities, then ask ChatGPT to create a disposable session,
   create a small failing program/test, obtain a real nonzero exit, inspect and
   fix it, rerun to exit zero, and show `git diff`.
7. Run a long task and continue with `deck_job_read`; test stdin and interrupt.
8. In Deck choose **Take control**. Confirm the running process remains visible
   and subsequent old-epoch MCP writes fail. Stop any running job with
   **Ctrl-C** in the pane (takeover starts no shell), choose **Return to MCP**
   — also after the execution window expired — and continue only after MCP
   requests control again under the new epoch.
9. Close the test card, revoke the client, and remove only the disposable test
   project. Confirm no Codex CLI, Claude Code, LLM API, or agent process ran in
   the managed session.

Expected report: each step is `PASS`, `FAIL`, or `MANUAL_PENDING`; do not treat
a Codex smoke, API Playground run, or unchanged usage display as proof of a
successful ordinary ChatGPT Chat or of any quota behavior.
