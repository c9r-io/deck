# Pi v0.8.0 implementation: evidence

> **更正（Follow-up 01，2026-10-12）。** 独立验收（`../pi-acceptance-20261012/`，负责人本地文档，不在版本控制内）未通过本轮实现。
> 本文件是当时的记录，保留原样供对照，但其中有两处判定是**错误的**，已在原位改正：
> A12（同 PID `exec` 当时只用合成的内核事实测过 PID 复用，真实 `exec` 会让旧 lease 继续可信）
> 和 B02（257 字节的 Session ID 被 ACK 后静默丢弃，同一 lease 随后还能接受另一个 ID）。
> 本文件所说的“final tree”也没有可复核的构建标识：`native-result.json` 记录的
> `deck_head` 是 `812b786`，没有测试可执行文件的哈希和当时工作区的差异（F05）。
> 六个问题的根因、修复、重新生成的证据，以及当时未运行的 G06/C03/B12/A26 布局门禁，
> 见 [`../pi-followup-01-20261012/README.md`](../pi-followup-01-20261012/README.md)。

What was implemented against `docs/design/pi-v0.8.0/` (the design lead's local
documents, not in version control), what was run, on what,
and what it does and does not show. One row per acceptance item, with the
place and the build it ran on. Nothing here is a release result: every run
used this checkout's debug build.

| Scope | Verdict |
|---|---|
| P1a: run-boundary Signal (`working`, `turn-done`, limited coverage) | implemented; evidence below |
| Full P1: a reliable input request for Pi | **BLOCKED** upstream (unchanged); the reorder was reproduced again (A19) |
| P2: native session observation | implemented; evidence below |
| P3: automatic re-entry of Pi | **BLOCKED** (unchanged); only the refusal is implemented (C01–C03) |

The negative tests of the two blocked scopes pass. That is not those scopes
passing.

## Builds and artifacts

- Base `1bdcf57`. Commits: `ea6d3fe` (P1a), `812b786` (P2 and Claude's
  receipt contract), then the P3 refusal and this evidence (see `git log`).
- macOS 27.0 (arm64) on the development machine; macOS 27.0.1 (arm64) on the
  Mac mini test machine for the WKWebView runs. Rust 1.97.1 (pinned), Node
  v24.19.0, cargo-llvm-cov 0.8.5, bundled tmux (sha256 `887791ec…913ac0`).
- Pi 1.1.0, two runtimes:
  - npm install on Node: package 1.1.0, `cli.js` sha256
    `e79626f2…366926774`; the running image is `node` (tmux calls the pane's
    program `node`, the process calls itself `pi`).
  - official standalone build: `pi-darwin-arm64.tar.gz` of release v1.1.0,
    sha256 `3455b13d…e90d63c7` (matches the release's `SHA256SUMS`); `pi`
    sha256 `daea10db…1cd6a6501`.
- The model is a deterministic local provider (`scripts/pi/fixture.ts`, TEST
  ONLY, never packaged). No model account, no credential and no setting of
  the account was read. Pi itself retried, queued, continued, settled, raised
  its dialogs, reloaded, switched sessions and quit.

## Kinds of evidence

U unit · I integration (real sockets, real processes, real tmux, fixtures
allowed) · N native (real interactive Pi against production's listener,
transport, store, poll body and Settings commands: `scripts/pi_native.py`
with `agent_pi::tests::native::tap`) · W real WKWebView with the production
backend and a real Pi (`pi-live`, `scripts/pi-live-verify.py`) · L layout or
mock · S safety scan or regression.

The N harness runs production code in a test executable placed as a stand-in
bundle under a relocated home. It is not the application: app.log is not
written there, and the window does not exist. W is the application.

## Where each run is

| Run | Result | File |
|---|---|---|
| N, both runtimes, all scenarios, final build | see `native-result.json` | summary only; the trace is private |
| W, npm runtime, Mac mini | verdict PASS 13/13, 14 operator checks | `wk-pi-live-npm.json`, `.verdict.txt`, `.applog.txt` |
| W, standalone runtime, Mac mini | verdict PASS 13/13, 14 operator checks | `wk-pi-live-standalone.json`, `.verdict.txt`, `.applog.txt` |
| Gates on the final tree | see `gates.json` | commands and exit codes |
| Code identity of the two runtime images | `identity.json` | public signing identities only |

`*.applog.txt` holds the production's own closed lines of the W run (session
tags and closed words). The private traces (pane captures, Pi's event log
with native session ids) stay in the experiment roots and are not committed.

## P1a

| AC | Kind | What ran | Verdict |
|---|---|---|---|
| A01 | U/S | `agent_pi::tests::pi_is_registered_without_a_hook_family_or_a_reentry_capability`, `extensions` table tests, `external_admission::pi_is_no_remote_agent_and_its_adapter_reaches_no_terminal_or_queue` | PASS |
| A02 | U/I | `agent_status::tests::a_hook_line_that_names_pi_is_refused_whatever_else_it_says` (v1 and v2, with every admission fact in order); the Claude and Codex vectors are unchanged | PASS |
| A03 | U/I | `agent_pi::tests` (hello, frames, numbers, foreign fields, repeated keys, oversize); `transport::tests::a_line_that_breaks_the_wire_is_refused_and_ends_the_stream`; the listener returns in under 500 ms (`the_listener_hands_a_pi_stream_to_the_pi_transport_and_to_nothing_else`) | PASS |
| A04 | U/I/N | `agent_pi::config::tests` (every layout back to its bytes, mode, symlink, stable-install rule, this build's own files only, two switches as one entry); `edr_quiet::the_pi_observer_s_files_are_one_closed_list_everywhere`; N `settings_loop` | PASS |
| A05 | U/I/N/W | config tests (bytes outside the array, malformed, repeated key, non-array untouched); N and W byte comparisons, including after Pi rewrote its own file | PASS |
| A06 | U/I/N | `the_boot_migration_moves_an_entry_and_enrolls_nobody`, `the_user_s_own_disabling_entry_wins`; N migration and negative-entry checks | PASS |
| A07 | U/I | `another_writer_s_change_is_taken_once_and_never_overwritten_twice`, `several_entries_of_deck_s_are_a_conflict_until_one_carries_them` | PASS; the window between the last read and the rename is not closed (residual, as designed) |
| A08 | N/W | N `settings_loop`, `start_both`, `disable`; W `pi-clean`, `pi-enable`, `pi-connected`, `pi-reload` on both runtimes | PASS |
| A09 | I/N | N `start_both`: peer is the terminal's foreground leader, a child of the pane's shell, on its terminal, for both runtimes; `transport::tests::real_processes_bind_only_as_their_own_pane_s_foreground_program` | PASS |
| A10 | I/N | `two_panes_of_one_project_never_share_a_state`; N `together`, `inactive_pane` | PASS |
| A11 | I/S | `only_the_pane_s_own_foreground_program_may_bind`; the real-process test (another pane's name, a background child with the inherited environment, a wrapper's child); non-TUI modes refused by the wire and never connected by the observer | PASS |
| A12 | I/S | `every_frame_is_judged_by_what_the_kernel_says_now`, `a_foreground_that_changes_during_the_listing_writes_nothing`, `an_answer_that_outlasts_its_budget_is_late_and_commits_nothing_that_stays` | ~~PASS~~ **FAIL（更正）**：这些测试用合成的内核事实模拟的是 PID 复用，不是同 PID 的 `exec`；独立验收用真实进程证明 `exec` 之后旧 lease 仍被 ACK、卡片仍显示 `connected`。修复与真实 `exec` 证据见 Follow-up 01 的 F01。 |
| A13 | U/I | `frames_are_numbered_without_a_gap_and_runs_never_go_back`, `one_observer_per_foreground_generation_and_the_first_stays`, `ending_a_lease_withdraws_its_own_observation_and_nothing_else` | PASS |
| A14 | N | `boundaries`, ordinary run, both runtimes | PASS |
| A15 | N | retry, queued follow-up, continuation before the settlement, both runtimes (provider asked twice, settled once, one ending) | PASS |
| A16 | N | a run started by the settlement handler: done(A), working(B), done(B), both runtimes | PASS |
| A17 | N | abort (Escape) and provider error, both runtimes | PASS |
| A18 | N/S | confirm, input, select, editor, custom, a timed dialog and two at once, both runtimes: no card change; `a_pi_run_can_never_say_that_input_is_needed`; no word but `working` and `turn-done` in any backend's whole life | PASS |
| A19 | N/S | `out_of_order`: a later extension hears a dialog's end before its start, and a start after the run it preceded had settled; both runtimes | reproduced: this is the evidence for BLOCKED full P1, not a pass of it |
| A20 | N | `/model` and `/resume`: no prompt event, no card change, both runtimes | PASS for those two; startup trust, login, update and external-editor dialogs UNRUN |
| A21 | I/N | `not_loaded`: `--no-extensions`, the user's disabling entry, a missing file, an entry that throws; both runtimes | PASS |
| A22 | I/N | observer test (callback error, lost connection, unanswered frame, full queue); N `frozen` (fifteen seconds of silence), `reload`, quit, a killed Pi; `a_stream_that_floods_or_stalls_is_ended_inside_its_bounds` | PASS |
| A23 | I/N | N `restart`: idle not replayed, an active run reported again as working on a new lease, its ending once | PASS |
| A24 | I | `the_service_holds_a_bounded_number_of_streams_and_never_makes_the_listener_wait`, `pending_lines_are_counted_and_bounded`, the flood test; `a_held_receipt_validation_and_a_stuck_pi_admission_do_not_wait_for_each_other` | PASS (not staged with a real hostile client) |
| A25 | W/N | W `pi-disabled` on both runtimes; N `disable` (withdrawn within a second, an old Pi adds nothing) | PASS |
| A26 | W/N | W `pi-turn-done` (viewed), `pi-attention` (unread once), `pi-connected` (coverage is no attention); N `viewed`; `notify::tests::a_withdrawn_episode_takes_its_notification_and_its_count_with_it` | PASS for card, attention list and viewed episode. A posted away notification and the Dock badge with a Pi episode: UNRUN (the carrier is ad-hoc signed and may not post) |

Found while testing, upstream behaviour of Pi 1.1.0 on both runtimes:

- An extension that fails to load (a throw at load, a throwing factory, a
  syntax error) stops Pi from starting ("Failed to load extension … Start
  without extensions using pi -ne"). So the observer's factory cannot throw
  into Pi (`pi-observer.test.mjs`), a path is written only for files with this
  build's bytes, and a missing file is not fatal (A21). A named import Pi
  does not export does not throw under Pi's loader.
- Two dialogs raised at once by one extension: Pi announces one start, no
  end after both were dismissed, and the command that raised them does not
  return. Deck's card did not change.
- Pi rewrites its own settings file (it added `lastChangelogVersion`). Deck's
  entry stayed, and turning the switch off removed exactly that entry.

## Automation safety

| AC | Kind | What ran | Verdict |
|---|---|---|---|
| Q01 | U/I/N | `scheduler::tests::{no_automatic_row_is_selected_into_a_recognized_pi, a_pi_target_is_recognized_from_facts_read_now, the_pre_fire_transaction_holds_a_pi_the_tick_did_not_observe}`; N: recognized by evidence and before firing, in every coverage state, both runtimes | PASS |
| Q02 | U/I/W | `no_approval_or_override_exempts_a_pi_target`; N (unavailable and switched-off Pis are still Pi); W `pi-hold`: through several scheduler passes the row is pending, zero attempts, no text in the pane, provider never asked | PASS |
| Q03 | U/I/W | the same scheduler test (clock, approved follow-up, first-send override, channel head); W: a real clock rule's first step is the held row | PASS; Slack and phone sources at U/I only |
| Q04 | I/W | `a_fresh_pi_is_started_and_never_sent_to`; W `pi-run-created` then `pi-hold` | PASS |
| Q05 | U/S/N | `external_admission` test; N: a plain `node` process is not recognized | PASS |
| Q06 | W | W `pi-send-now`: the probe is ready, the row's text arrives once | PASS; the existing wrong-target, foreground and paste-edge refusals are the unchanged regression suite, not re-staged for Pi |
| Q07 | S | `tests/signal_census.rs` (9 tests), diff review | PASS |

A Pi reached only through a wrapper whose own process leads the terminal (for
example `npx`) is neither bound nor recognized by name. The design accepts
that residual; nothing is granted by it.

## P2

| AC | Kind | What ran | Verdict |
|---|---|---|---|
| B01 | U/I/N/W | scanner pin of the one guarded read; `pi-observer.test.mjs` (the session manager is never asked without the capability); `session_frames_need_their_own_capability_and_are_gone_with_the_lease`; N `native_sessions` (signal alone names no session, session alone reports no run, both on one stream); W `pi-session` | PASS |
| B02 | U/I/N | `a_native_id_is_pi_s_own_shape_and_never_cut_short`; N: an id of the user's own (`--session-id`) kept as it is; 257 bytes acknowledged and not kept | ~~PASS~~ **FAIL（更正）**：“ACK 后不保留”正是缺陷——超限 ID 被静默丢弃且清空了一致性记录，同一 lease 之后可以接受另一个 ID，前端也没有 unsupported 状态。修复见 Follow-up 01 的 F02。 |
| B03 | N | startup, `/fx-new`, `/fx-fork`, in-process resume by path, with Pi's own reasons and the id Pi names | PASS on the npm runtime; the in-process changes were not repeated on the standalone build |
| B04 | N | a session named on the command line starts as `startup`; a path that does not exist creates a session; an id that does not exist starts no Pi; `--session-id` | PASS |
| B05 | N | `/reload`: the same id, another lease, the old one ended by its reset | PASS (npm) |
| B06 | I/N | one session in two panes: two leases, two records | PASS |
| B07 | N/I | quit ends the lease by its reset (`reset-quit`); a killed Pi ends it as `eof` with no teardown; `frames_sent_just_before_the_stream_ends_are_answered_before_the_end` | PASS |
| B08 | I/N/W | N: a restarted Deck knows no session and gets a `snapshot`; off withdraws at once; no native id in Deck's data directory, Pi's settings or the summaries (N and W) | PASS |
| B09 | U/I/S | `lifecycle::tests::pi::an_observation_reaches_no_mailbox_and_pi_has_no_receipt_contract` (armed lease for the same pane, source check of both halves) | PASS |
| B10 | U/I/S | the Claude re-entry suite (`agent_reentry::`, 115 passing tests in the gate run) unchanged in what it asserts: its test edits are the new contract lookup, the tagged event constructors and one added capability field; `lifecycle::parse` takes a contract | PASS |
| B11 | I | `a_held_receipt_validation_and_a_stuck_pi_admission_do_not_wait_for_each_other` | PASS |
| B12 | W | W `pi-session`: the second switch through its confirmation, `needs-reload` until Pi's own reload, then `observed` in the payload and no other value of the session; `[agent-pi] session start-reload` in app.log and no id | PASS |

## P3: the refusal only

| AC | Kind | What ran | Verdict |
|---|---|---|---|
| C01 | N/S | `identity.json`: Node's Developer ID names Node, not Pi's JavaScript; the standalone build is ad-hoc signed with no team and fails strict verification | negative evidence recorded; no digest table exists |
| C02 | U/I | `agent_reentry::tests::a_verified_pi_is_named_refused_and_never_started_whatever_is_approved`: Pi alone, Pi beside the Claude card, both policies: refused whole, nothing replaced, started or typed, the Pi not asked to exit | PASS |
| C03 | W/U | W `pi-review` on both runtimes: the real backend names the card as Pi with `client-identity-unverifiable`, not selectable, in no request, also while its session is observed; `reentry-model.test.mjs` in both languages | PASS; screenshots of the dialog were not taken, and a real Claude beside it needs an account (BLOCKED for this run; the mixed case is C02 with the stand-in product) |
| C04 | S | the whole `agent_reentry::` and `procinfo::` suites in the gate run | PASS |

## Global

| AC | What ran | Verdict |
|---|---|---|
| G01 | `gates.json`: fmt, clippy, the gate's coverage run | see file |
| G02 | `node --check` on every module, `scripts/ui-tests`, `check.mjs`; `pi-observer.test.mjs` loads the four observer files as they are | see file |
| G03 | `edr_quiet`, `log_privacy`, `signal_census`, `external_admission`, sidecar provenance | see file |
| G04 | Claude and Codex suites in the gate run | PASS; live Claude and Codex with accounts: BLOCKED (no credentials), unchanged from before |
| G05 | W on the Mac mini, both runtimes: Settings install, status, attention, hold, Send now, disable, session observation, the review | PASS; a restart that reopens a real Claude beside Pi: BLOCKED (account) |
| G06 | layout at 1280×800, both languages, font scale, long titles for the new Settings rows and the review row | UNRUN. DOM tests exist (`dom.test.mjs`); no screenshot or layout run was made |
| G07 | `test_sidecar_provenance.py` (the debug carrier holds exactly the four files, byte for byte), `test_release_tools.py` (`check-edr-binary` on a bundle), the release executable scan in `gates.json` | see file; a signed, notarized release bundle: UNRUN |
| G08 | every run inside one experiment root under `/private/tmp`; the account's Pi, Claude and Codex settings were not modified (their modification times predate this work) | PASS. The account's own Deck was running throughout and wrote its own files; nothing of this work wrote under its data directory |
| G09 | every run ended with `edr_runtime.py experiment cleanup` exit 0, no process from the root, no socket left; on the Mac mini the root was moved to the Trash and its own Deck's processes and file names were the same before and after | PASS |
| G10 | no push, tag, release, message or deployment; `CLAUDE.md` and `AGENTS.md` untouched | PASS |

## Failures on the way, kept

Product defects found by the tests and fixed:

1. A quitting Pi's last frames arrived together with the end of the stream
   and were dropped: the lease ended as `eof` instead of by its reset. Found
   in the first W run; fixed in the transport; pinned by
   `frames_sent_just_before_the_stream_ends_are_answered_before_the_end`.
2. The observer's factory could throw into Pi if registering a callback
   failed, and Pi does not start then. Fixed; pinned by the observer test.
3. The review payload's new field was first named `agent`, which is the
   Signal word's field: the gate's census refused it. Renamed `source`.

Test and harness mistakes of this work (not product defects), each fixed:

- `test_sidecar_provenance.py`: three failures after `bundle.sh` began to
  copy the Pi files and the test's tree did not have them. The fixture got
  the files and two new tests.
- A listener test of this work began and ended a re-entry operation without
  the lock every receipt test holds, and made one of them fail once.
- Clippy: two findings in new test code; later two unused helpers and some
  boolean asserts.
- The native driver: a continuation fixture that asked for a continuation
  without a context entry; a rule command longer than 200 characters; a
  wrong key for the queue plan; a scenario named like an attribute; expected
  bytes that ignored Pi's own rewrite of its settings.
- W on the development machine: the screen was locked, the page's timers
  stopped and the run stalled after two checkpoints. It was cleaned up and
  the runs moved to the Mac mini.
- One W run loaded a page with a syntax error of this work and reported
  nothing but `done`; rerun after the fix.

## Design deviations and open questions

1. A missing Pi agent directory is not created: the switch is refused
   ("Pi has not been run on this Mac yet"). The design mentions creating it.
2. Frames are told apart by their exact key sets; there is no `type` key.
3. One pane listing per batch of waiting frames, none for a heartbeat. Every
   frame still gets its own kernel reads.
4. Turning a capability off ends a stream that carries it, including its
   other capability, until Pi reloads. Narrowing the stream instead would
   keep work status alive when only session observation is turned off.
5. Any `-<path>` entry of the user's for one of Deck's files keeps both
   switches off and is never repaired.
6. A Pi is recognized for the hold by a verified observer or by the name the
   foreground gives itself (an npm install is `node` to tmux).
7. The review's label is the field `source` (`claude-code`, `pi`), and the
   Pi reason sentence carries the label; other rows have no visible label.
8. An id longer than 256 bytes is acknowledged and not kept, rather than
   refusing the stream (a refusal would repeat at every reconnect).
   **（更正：这条偏差未被接受，已撤销。Follow-up 01 的 F02 改为显式、粘性的
   `unsupported`，见该轮证据。第 4 条“关一个能力会结束整条流”同样已由 F03 改掉。）**
9. `CLAUDE.md`'s module index does not list `agent_pi` (the file may not be
   edited in this work).

## What would lift the two blocks

Full P1: an upstream interface that numbers or identifies a prompt when its
state changes and cannot be overtaken by another extension, or an
authoritative snapshot of what Pi currently waits for. P3: a supplier
identity for the running Pi code that Security can validate, an exact
persisted target, a final exit receipt and a supported launch grammar. Until
then `PI.reentry` stays `None`.
