# MCP validation record

Date: 2026-09-21. Status words below are evidence labels, not forecasts.

Historical signed-smoke results below apply to protocol v1. They are not proof
that the current control-protocol-v3 scheme-B UI has been exercised in an
installed signed app.

## Scheme-B remaining-requirement matrix

The current source uses control protocol v3 and state schema v3. The MCP
standard protocol version is negotiated independently by the SDK.

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
- Revoke/takeover closes the in-memory gate before runner fencing. A missing
  runner acknowledgement returns an uncertainty error and never claims that a
  running process stopped.
- Output reads authenticate before lookup and recheck sharing/read authority
  before return. Retention gaps are explicit; transmitted bytes cannot be
  recalled.

## Automated evidence

- **PASS** — official-SDK STDIO adapter initializes, lists all 14 tools,
  advertises strict schemas and annotations, rejects unknown arguments, calls
  a separate mock Deck service, returns structured content, and emits only MCP
  JSON on stdout: `cargo test -p deck-mcp --test stdio`.
- **PASS** — production runner executes a real zsh child, ignores a forged
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

## Current protocol-v3 workspace verification

The 2026-09-21 isolated rerun used synthetic principals, private temporary
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
   and subsequent old-epoch MCP writes fail. Exit the human shell, choose
   **Return to MCP**, and continue with the new epoch.
9. Close the test card, revoke the client, and remove only the disposable test
   project. Confirm no Codex CLI, Claude Code, LLM API, or agent process ran in
   the managed session.

Expected report: each step is `PASS`, `FAIL`, or `MANUAL_PENDING`; do not treat
a Codex smoke, API Playground run, or unchanged usage display as proof of a
successful ordinary ChatGPT Chat or of any quota behavior.
