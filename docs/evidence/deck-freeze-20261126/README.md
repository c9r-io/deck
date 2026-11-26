# Deck freeze of 2026-11-26 01:46 JST — investigation, defects, fix

What this records: what the existing evidence can and cannot say about the
freeze of the installed Deck 0.8.40 (main process 66440, build
`c7ffe19ce6bcc84ffa208273ba894d3f44ba1b42`) at 01:46 JST on 2026-11-26; two
defects that freeze the window independently of that incident, each
reproduced through the production call path in a real WKWebView; the fix; and
what was verified where. Historical attribution and defect proof are kept
apart on purpose: a reproduced defect of the same class is not the
incident's proven root cause.

All times are JST. app.log stamps are epoch seconds (1795625100 = 01:45:00).
The 12 files the previous read-only round compared were re-checked: identical
between the incident build and `51c08ce7` (0.8.42), the base of this work.

## 1. Historical attribution

### Confirmed (from the incident's own records)

| Time | Fact | Source |
|---|---|---|
| 01:45:27 | attach `sess-eaf85`, first emit 7378 B after 8 ms (normal) | app.log |
| 01:45:47.338 | last JS evaluation (Tauri `emit`, i.e. any `pty-data` or other event) delivered to the page by the old process; **none for 21.5 s** until 01:46:08.861 | unified log, `incident-webkit-ipc-timing.txt` |
| 01:45:51.65 | card switch: its requests took ~50 ms (one 50 ms SIGHUP-grace step of portable-pty's `kill`) | same |
| 01:45:52, :55, :56, 01:46:13 | four attaches with no first output (except below) | app.log |
| 01:45:55, :56, 01:46:12.97 | each card switch: 6 requests start together and all complete ~210 ms later = portable-pty `kill`'s full 200 ms SIGHUP grace, then SIGKILL: the tmux client being detached did not exit within 200 ms | unified log |
| 01:45:56–58, 01:46:00.9–01.9, 01:46:08.4–09.8 | bursts of ~60 Hz requests of ~5 ms each, the wheel's one-request-in-flight rhythm (`scroll_session`, two tmux calls): **short tmux commands were being answered** | unified log, layout.js wheel scheduler |
| 01:46:08.861 | the only JS evaluation in the gap = `first emit 2B to sess-15e37 ... after 12561ms` (attach ≈ 01:45:56.3 + 12.561 s) | unified log + app.log |
| 01:46:13.17–.19 | switch to `sess-c4040` completed (attach logged, its 5 follow-up requests completed) | both |
| 01:46:13.916–.925 | a 7th six-request group (the card-switch shape) starts; **none completes** | unified log |
| 01:46:16.4 / 23.5 | spindump `spin` / `hang likely`, both "not sampling due to conditions 0x400000000" (no stack) | unified log |
| 01:46:32 | `webview poll idle`: the scheduler tick ran and took the PTY map lock (`PtyState::attached`) before logging | app.log, scheduler/thread.rs |

### What this supports, and how far

- The main thread stopped answering at ~01:46:14 while serving the first
  request of that 01:46:13.916 group, and never answered again before the
  force quit. Which command it was is **unknown**: Tauri logs no task↔command
  mapping and no stack was sampled.
- That command was **not** holding the PTY map lock at 01:46:32 (the
  scheduler took it then). This excludes, for the final hang, a `pty_write`
  parked in `write_all` (it holds that lock; reproduced below: that variant
  never logs `poll idle`) and a detach dropping its writer under the lock.
  It is compatible with a main-thread tmux command waiting for an
  unanswering server (reproduced below: that variant logs `poll idle` ~13 s
  in, as the incident did at 18 s).
- From ~01:45:52 newly attached tmux clients did not get their first redraw.
  The 2 B that arrived after 12.6 s has exactly the shape reproduced in
  §4 (S2): with the server not servicing a new client, its tty stays in
  cooked mode and the KERNEL echoes a keystroke (`first read 1B ... after
  1233ms` in the reproduction). This makes "the tmux server stopped
  servicing attach clients (not short commands) from ~01:45:52" the leading
  **candidate**. It is not proven: the 2 B content was never logged, and
  why a server would answer `display-message` but not attach a client is not
  explained by anything recorded.
- The 21.5 s without any JS evaluation from 01:45:47 is consistent with that
  candidate (nothing reached any attached client) but is also consistent with
  an idle screen after scrolling stopped at 01:45:47.52; it is not used as
  proof.

### Candidates, not proven

1. The final hang = a synchronous terminal command (`scroll_session`,
   `click_session`, `scroll_bottom`, selection, search, ...) on the main
   thread waiting in `Command::output` on a tmux server that did not answer
   it. Defect D1 below is this path, reproduced.
2. The trigger = the tmux server degrading from ~01:45:52 (attach clients
   not serviced, then not even the short command at 01:46:14). Cause unknown.

### Evidence that does not exist

No thread stack of 66440 (spindump skipped every sample: conditions
0x400000000, a value with no reliable public definition; not interpreted
here). No WebKit task→command mapping. No tmux server log or stack. The
`deck_app-<hash>` reports later that day are test runners, not this process.
The Pi `no-such-pane` lines share the force-quit second and are not evidence
of tmux failing first. Historical search stopped here; further log mining
only repeated these facts.

## 2. Defects proven independently

All runs: this Mac, a debug smoke bundle (`app/run.sh` with
`DECK_SMOKE_DATA_DIR` under `/private/tmp`, its own `deck-smoke-*` socket),
driven with cua-driver; the fault is `SIGSTOP` of that bundle's own tmux
server (pid read from the socket and checked), released with `SIGCONT`.
Production `~/.deck`, the user's tmux server and Deck 7112 were not touched.

**D1 — terminal commands wait on tmux on the main thread, unbounded.**
Tauri 2.11 runs a non-`async` command inside WKWebView's URL-scheme callback
on the main thread (`ipc/protocol.rs` → `Webview::on_message` →
`run_invoke_handler`); `session_runtime::command_output` is bounded only
under a deadline scope, which no terminal command had. Baseline (HEAD
`51c08ce7`): server stopped, one wheel → 1457/1457 main-thread samples in
`startURLSchemeTask → on_message → scroll_session (terminal.rs:113) →
tmux_owned → command_output → Command::output → read_output`
(`baseline-main-thread-scroll_session.txt`); the window stopped answering
accessibility; 13 s later app.log said `webview poll idle`; released, it
recovered (`baseline-app-log-excerpt.txt`). Same class: `click_session`,
`scroll_bottom`, `terminal_search`, `clear_history`, all
`terminal_selection_*`, `terminal_metrics`, `set_terminal_mode_style`, and
`pty_resize` (it waits for the operation lock such a command holds).

**D2 — PTY input written blocking on the main thread under the PTY map
lock.** A macOS pty master takes 1022 unread input bytes, then `write(2)`
blocks (measured). Baseline: server stopped, ~1300 paced keystrokes → 1545/1545
main-thread samples in `pty_write → UnixMasterWriter::write_all → write`; the
scheduler thread then blocks in `PtyState::attached` on the map lock, so no
`poll idle` is ever logged (`baseline-main-thread-pty_write.txt`).

**D3 — detach/replace on the main thread wait on the client and write into
the tty.** `detach_session` / attach replacement / `detach_all` ran
portable-pty's `kill` (SIGHUP, up to 200 ms of polling, then SIGKILL, never
reaped) and dropped the entry under the map lock; portable-pty's writer drop
does a BLOCKING `write_all("\n" + VEOF)` (pinned:
`pty::tests::portable_pty_writer_drop_blocks_while_the_tty_takes_no_input`).
Every incident switch from 01:45:55 cost the main thread 210 ms this way.
The same write, delivered to a client that is still attached, is Enter +
Ctrl-D to the pane: see §4, first candidate.

## 3. The fix (commit in §5)

- `session_runtime::off_ipc_thread(op, budget, work)`: the terminal command's
  `_now` body runs on a blocking-pool thread with the `Deadline` installed on
  THAT thread, so each tmux call is `bounded_output` (its own child killed
  and reaped at the deadline); the caller gets `terminal-timeout` at
  `budget` at the latest (tokio `timeout_at`); a late worker ends at its
  deadline and its answer is dropped and logged. Budgets: 2 s for the
  interactive commands and `pty_resize`, 5 s for selection copy.
- `session_runtime::OperationLock` replaces the operation `Mutex`: exclusion
  only (no order was ever promised), its wait ends with the caller's deadline
  (`terminal-busy`), so no request queues forever behind a holder on tmux.
- PTY: the master is non-blocking; the reader and the input pump each
  read/write and `poll` through their OWN `dup` of it (the master's own
  number is closed by the reaper and may be reused); each
  attachment has its own input pump: `pty_write` proves the generation under
  the map lock and only queues (bounded 1 MiB; `input-busy` refuses a call
  whole); the pump writes in call order, waits for the tty with `poll`, and
  when its attachment ends drops what the tty still refuses (logged, never
  moved to a successor, never resent). portable-pty's writer is not used:
  the pump writes through its own `dup` (`master_input`), so ending an
  attachment types nothing. Detach, replacement and `detach_all` remove the
  entry under the lock and `retire` it outside: gate closed, pump told, and
  a reaper thread kills AND reaps the attachment's own tmux client, then
  closes its master. The shared tmux server and other sessions are never
  touched.
- The MCP input fence `pty_write` calls first lists tmux panes only when the
  MCP ledger is unreadable; that listing now runs under a 1 s deadline on the
  IPC thread and fails closed.
- Diagnostics (content-free, existing applog / `ui_event`): `[pty] attached
  … gen=N … client started`, `first read NB … after Xms`, `first emit … gen=N
  … after Xms (emit Yms)`, `[ui] pty-first a=<gen> b=<bytes>` when xterm
  consumed that generation's first frame; `[pty] input stall / resumed /
  dropped … gen=N`; `[terminal] <op> no answer within Nms (workers=K)`,
  `finished late …`, `slow …`, at most one line per op and outcome per 5 s.
  `poll idle` now says what was observed: no Board poll reached the backend
  for 10 s.

Contracts kept: input bound to a generation reaches only that attachment
(`pty_write_reaches_only_the_bound_attachment_in_call_order`, IPC argument
shapes test); call order (`queued_input_arrives_in_order_after_the_tty_drains`);
the end-to-end ACK window and stale-generation handling are unchanged; resize
still excludes selection/wheel/click/search work; a click on a grid that is
not the pane's still answers `stale`; no input is ever retried.

## 4. Verification

| What | Where / build | Result |
|---|---|---|
| D1 baseline: wheel over a stalled pane | real WKWebView, HEAD `51c08ce7` | FAIL as expected: main thread 100% in `scroll_session` `read_output`; window unresponsive; `poll idle` at +13 s; recovers on release |
| D2 baseline: ~1300 keystrokes into a stalled pane | same | FAIL as expected: main thread 100% in `write(2)`; scheduler blocked on the map lock |
| Candidate, first build | real WKWebView | stall scenario PASS; but a card switch **ended an idle zsh**: tmux's exit ledger recorded pane `%0` status 0 and Deck retired the card (portable-pty writer drop's `\n` + VEOF reached a live client). Fixed with `master_input`; regression test fails with the old writer (2 bytes in the tty) and passes now |
| S1 final build: wheel + clicks over a stalled pane, then open the Board while still stalled | real WKWebView | PASS: 0 main-thread terminal/tmux frames; `scroll_session no answer within 2000ms`, workers end at their deadline (`finished late after 2000ms`, count falling); Board opened; webview polls kept arriving (`poll-fail`, no `poll idle`); after release a wheel scroll works again, no new timeout |
| S2 final build: ~1377 keystrokes into a stalled pane, then switch to the other card while stalled | real WKWebView | PASS: no `pty_write` on the main thread; `input stall … gen=13`; on the switch `input dropped … gen=13: 356B never reached the tty (attachment ended)`; nothing of card A's input reached card B (pane captures after release) |
| S3 final build: 10 switches between two idle cards | real WKWebView | PASS: 11 attaches, 11 first emits 7–12 ms (`emit 0ms`), 11 `pty-first`; both shells alive; exit ledger empty |
| Final build re-run (after giving the reader and pump their own descriptors): 7 static switches, then stall + ~1130 keystrokes + wheel + switch + Board, release | real WKWebView | PASS: 7 first emits 7–11 ms; 0 main-thread pty/tmux frames while stalled; `input stall … gen=9`, then `input dropped … 109B` (1022 held by the kernel + 109 = what was typed); a stalled `set_terminal_mode_style` answered `no answer within 2000ms`, its worker ended at 2002 ms (`workers=0`); both shells alive, exit ledger empty, no typed byte in either pane; 0 zombies |
| Resources | real WKWebView | 0 zombie children after 16 attachments incl. stalled detaches; only the live pane client and the Board's control client remain; thread count steady (37–38) |
| Unit: `pty::`, `terminal::`, `session_runtime::` | `cargo test` | 40+ pass, incl. new: writer-drop hazard pinned, `pty_write` never waits on a full tty and refuses past the bound, order after drain, ending an attachment writes nothing, lock wait ends with the deadline, `off_ipc_thread` answers within budget and its stalled child is killed |
| Gates | this Mac | see §5 |

Clicks were sent as background events, so `forwardTerminalClick` (armed only
when the window has focus) may not have reached `click_session` in S1; that
command shares `off_ipc_thread` with `scroll_session` but its own stalled run
in a focused window was not observed.

## 5. Commit, gates, remaining

Gates on the final code (this Mac, 2026-11-26): `cargo fmt --check`; `cargo
clippy --workspace --all-targets --all-features -D warnings`; the gate's own
Rust step `cargo llvm-cov --workspace --all-targets --all-features --locked
--offline --fail-under-lines 75 --fail-under-functions 75 -- --test-threads=1`
with an isolated HOME and an empty TMPDIR: exit 0 (deck-app 1487 passed, 14
ignored; every other target passed), TOTAL regions 86.41 % / functions
83.70 % / lines 85.63 %, then `scripts/check-test-tmpdir` exit 0;
`scripts/check-js-syntax.py`, `node ui/js/check.mjs`, `scripts/ui-tests`;
`scripts/extension_catalog.py --check`. The first full gate run caught
`external_admission::every_terminal_input_site_is_reviewed`: the input
census now pins `master_input(` (the pump's handle) and skips a function's
own definition. Not run: GitHub CI, a release build, the Mac mini.

Every experiment was a debug smoke bundle with its own data directory under
`/private/tmp` and its own `deck-smoke-fz*` tmux socket; each was stopped by
its pid and its server through `scripts/edr_runtime.py --cleanup --socket`,
leftover socket files removed by exact path after checking no process held
them, and `scripts/edr_runtime.py` audited clean (exit 0) after each.

Remaining, not changed here (same class, outside the proven path):
`start_session`, `kill_session`, `mcp_start_session`, `export_logs`,
`connector_smoke_transport`, `tmux_available` and `write_clipboard` (pbcopy)
still run synchronously on the IPC thread and would freeze the window if
tmux (or pbcopy) stopped answering. `start_session`/`kill_session` need a
decision about ambiguous outcomes (a session created or killed after the
caller gave up) before they can be bounded; they are not called on an
ordinary card switch of running cards. The incident's own root cause (why
the tmux server stopped servicing attach clients) remains unknown.
Not verified: a release/signed carrier, the Mac mini, a real Agent (Claude
Code, Codex, agy, Pi) pane under the fault, macOS spindump behaviour on the
fixed build with the window in front.
