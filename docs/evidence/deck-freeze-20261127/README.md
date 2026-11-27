# Deck freeze of 2026-11-27 ~19:00 JST — what the logs show

Installed Deck 0.8.43 (build `01ff6dbf`, contains the 2026-11-26 fix
`1a02e0d4`), main process 59610, started 01:11. The user did not force quit;
the window came back by itself. Memory was flat. Read-only investigation of
this Mac's logs; nothing was changed. Times are JST.

Files: `app-log-excerpt.txt` (app.log 19:00:20–19:02:40, epoch rendered),
`webkit-ipc-timing.txt` (every Tauri invoke as a WebKit URL-scheme task, with
start → complete, and every Tauri emit as a JS evaluation),
`system-log-excerpt.txt` (kernel APFS lines for the external disk today, and
the other processes' markers). The kernel lines rotate out within days (those
of 2026-11-26 01:4x were already gone), which is why they are kept here.

## Timeline

| Time | What | Source |
|---|---|---|
| 18:55–19:00 | external disk `disk7` (USB SSD; volumes Yotta, c9r-io, YottaUnity) already slow: transaction flushes of 1.2–3.7 s | kernel apfs |
| 19:00:26 | orchestrator starts worker card `sess-477a1` (Claude) | app.log |
| 19:00:42.6 | `disk7` transaction 473193 cannot open; 473192 cannot finish flushing | kernel apfs (`took 100351431 us to close`, `took 100998577 us to flush`) |
| 19:00:49.4 | user opens `sess-477a1`: `attached … gen=261 … client started`; **no `first read` ever** (the card stayed blank) | app.log |
| 19:00:50.9–51.3 | 17 wheel requests (`scroll_session`), ~11 ms each: tmux still answered | WebKit tasks |
| 19:00:52.6 | input method (SCIM_Extension) main thread serves Deck's activation; its last line until 19:02:23.568 | unified log |
| 19:00:54.554 | Board poll, 4 ms (normal 2.5 s cadence) | WebKit tasks |
| 19:00:55.534–.540 | card re-open: `detach_session` and `mcp_session_ui` answered at .540; the third request, `attach_session` (sent .536), does not return | WebKit tasks; layout.js `openSession` order |
| 19:00:57.057 | next Board poll sent, unanswered until 19:02:23.767; no JS evaluation (emit) reaches the page after 19:00:52.613 | WebKit tasks |
| 19:00:58.4 | spindump starts watching (Deck unresponsive) | spindump |
| 19:01:08 | `webview poll idle`: background threads alive (scheduler tick ran) | app.log |
| 19:01:19.1 / 19:01:28.6 | user switches to Brave / Terminal; spindump attaches ~2 s after each | launchservicesd SETFRONT, spindump |
| 19:02:23.028 / .556 | `disk7` transaction closes (100.35 s) / flushes (101.0 s) | kernel apfs |
| 19:02:23.548 | Spotlight `mdworker_shared` killed by mds after 113.9 s | launchd |
| ≈19:02:23.564 | `attach_session`'s spawn returns: `attached … gen=262 … client started` is written after the spawn; its `first emit … after 219ms` is the JS evaluation at 19:02:23.783 | app.log + WebKit |
| 19:02:23.568–.570 | input method `Activate Server`; Deck main thread's first line (queued occlusion events) | unified log |
| 19:02:23.596 | `Brave Browser: slow hid response (61.4s)` | spindump |
| 19:02:24 | the delivery to `sess-14a67`, stabilized at 19:00:54, is sent | app.log |

## What this establishes

1. Deck's main thread was blocked for 88 s (19:00:55.54 → 19:02:23.57).
   Every invoke queued; the Board poll stopped; emits stopped.
2. The blocked call was `attach_session` re-attaching the card. The request
   order is fixed by `openSession`/`leaveSessionView` (detach, then
   `mcp_session_ui`, then attach). Both requests before the attach were
   answered at .540, and the spawn returned at ≈19:02:23.564. The attach line
   is logged only after portable-pty's `spawn_command` returns, so the wait
   was inside `openpty` + `spawn_command`. The map lock was free (the
   scheduler took it at 19:01:08). The activity guard is `try_read`.
3. The wait ended 8 ms after the external disk's stalled transaction flushed.
   This was not the 2026-11-26 defect: those terminal commands now run off
   the IPC thread and none of them was involved.
4. The stall was system-wide. Brave froze 61.4 s and Terminal froze when
   brought to the front. The input method's main thread was silent.
   Spotlight's worker hung. Everything resumed within ~40 ms of the flush.
5. The external disk stalled repeatedly today: 12:47–12:52, 15:31–15:34
   (173 s), 16:57–16:59, 17:00–17:01, 18:10–18:13, 19:00–19:03. Deck kept
   polling through the earlier ones (no `poll idle`). A stall alone does not
   freeze Deck. An attach started during one did.

## Not established

- What inside the spawn waited on the stalled disk. No stack was sampled:
  spindump wrote no report for Deck. Today Deck holds no file on that disk.
  The tmux binary and the client's cwd (home) are on the internal disk.
  portable-pty forks Deck, then the child runs setsid and `TIOCSCTTY`, closes
  every inherited fd above 2 (`close_random_fds`) and execs. Because that loop
  also closes std's exec-status pipe, the parent's `spawn` returns as soon as
  the child reaches it, before exec. So `client started` means "the child got
  through close_random_fds", not "tmux is running". That reading would put the
  19:00:49 client (no output ever) stuck at or after exec. It would put the
  19:00:55 one stuck before closing the pipe. Both are inferences.
- Why the 19:00:49 client never drew. It is the same symptom as the 11-26
  incident (§1 there: "newly attached tmux clients did not get their first
  redraw" while short commands answered). The kernel lines for 11-26 had
  rotated out, so a disk stall there can be neither shown nor excluded.
- Whether Brave and Terminal waited on the input method, which waited on
  Deck as its last active client, or on the disk through another path.
- Why the SSD stalled. The processes with files open on it include tart VMs,
  Unity's dotnet, flow-music-mcp and cargo builds. No per-process I/O was
  recorded at the time.

## Same class, still on the IPC (main) thread

`attach_session`, `detach_session`, `start_session`, `kill_session`,
`mcp_start_session`, `tmux_available`, `write_clipboard` and `export_logs`
are synchronous Tauri commands. Each of them starts or waits on a process. A
stall in process creation therefore freezes the window, and through the
input method possibly other apps. The 2026-11-26 fix moved only the terminal
commands and `pty_resize` off that thread.

## Investigation, completed (after the read-only round)

Each item: conclusion, evidence, what is still missing.

1. **What the spawn waited on.** Not established at the syscall level.
   - Established: the stall held process creation in general, not only
     portable-pty's fork. `posix_spawn`-based tmux calls answered in 11 ms at
     19:00:50.9–51.3 (17 `scroll_session`), i.e. after the 19:00:49 fork of
     gen=261. From ≈19:00:54 a scheduler worker whose remaining steps were
     only tmux execs and libproc reads (`final_context_probe`, `fire_item`)
     did not finish until 19:02:24 (`sent to sess-14a67`). The main thread's
     fork for gen=262 returned ≈19:02:23.564, 8 ms after the disk7 flush.
   - Checked and not supported: an Endpoint Security / third-party kext gate
     (none installed: `systemextensionsctl list`, `kextstat`); amfid waiting
     on the stalled volume — amfid has no line between 19:00:05 and
     19:02:23.711, but its first validations afterwards name internal-volume
     files, the first external-disk binary appears only at 19:03:19, and
     Deck's fork returned ~150 ms before amfid's first line
     (`amfid-summary.txt`). Inconclusive either way.
   - Candidate kernel paths (not shown): `vm_map_fork` waiting on a VM object
     with paging in progress against disk7, or exec image activation waiting
     on a vnode/mount of disk7; `close_random_fds` closing an inherited
     vnode under `P_LADVLOCK` is unlikely (the flag is not inherited at fork).
   - Missing evidence: a stack of the waiting thread at the time. Deck now
     samples its own main thread after 5 s of a stall (`main_watch.rs`) and
     logs the symbol names, which separates `openpty` / `fork` /
     `__posix_spawn` next time; a kernel stack still needs a spindump.
   - Decision: the pane client is started by Deck's own spawn
     (`pane_client.rs`): std `Command` with a `pre_exec` that runs setsid
     and `TIOCSCTTY` and only MARKS inherited descriptors close-on-exec, so
     std's exec-status pipe lives until exec — `spawn` returns once tmux
     runs, and an exec failure is reported. `posix_spawn` was tried first
     and REJECTED: macOS gives a controlling terminal only through
     `TIOCSCTTY`, not by opening the slave, and `posix_spawn` has no action
     for it; such a tmux client had no controlling tty (`ps`: tty `??`,
     tpgid 0), never got SIGWINCH, and resized panes kept their old size —
     the WK `run`/`ux-layers` regressions below, reproduced by the unit test
     `a_real_tmux_client_follows_a_resize`. It still forks Deck; the move
     off the IPC thread is what keeps the window alive (the scheduler's
     `posix_spawn`-based tmux calls waited too).
2. **Why gen=261 drew nothing.** Not established. Under the old fork path
   `client started` meant only "the child passed close_random_fds", so the
   client may have been held at or after exec. With the new spawn
   `client started` now means tmux is running, so a repeat separates
   "exec never returned" (`attach … still spawning after Nms`) from "tmux
   running, no frame" (`no first read … within 3000ms`). Either way the
   pane now says "The terminal shows nothing yet" after 3 s with
   [Reconnect] (detach, then a new attach and generation).
3. **The delivery to sess-14a67 (stabilized 19:00:54, sent 19:02:24).** Not a
   designed wait: after its final observation the worker's only steps are
   tmux execs and libproc reads, and it is a per-session worker (only that
   session waited). The scheduler tick's pane listing had no deadline at
   all; it now runs under 5 s (`thread.rs` `bounded_listing`; a cut listing
   selects nothing). Recorded, not changed (bigger than this round): the
   per-session send workers run their tmux calls without a deadline by
   design of the firing window; MCP control threads hold the delivery lock
   across runner-socket exchanges (the IPC-thread commands that take that
   lock now run off the IPC thread instead).
4. **Input-method knock-on (optional).** UNRUN.
5. **Simulating a stalled volume without root (optional).** UNRUN: there is
   no WebDAV server here without installing software, and no macFUSE; the
   debug stall faults and a SIGSTOPped tmux server stand in.
6. **Budgets.** From this Mac's app.log (2026-11-27): attach → first read
   p50 6 ms / p99 32 ms / max 212 ms (321 attaches); start total p50 30 ms /
   p99 137 ms / max 153 ms (197 starts). Attach 5 s (notice at 1 s,
   first-frame notice at 3 s), start 10 s, kill 5 s, clipboard 3 s, links
   5 s, send-now 15 s (`docs/ipc-thread.md`).

## The fix

Rules and the census: `docs/ipc-thread.md` (enforced by `tests/ipc_thread.rs`).

- **Watchdog** (`main_watch.rs`): the one wrapper around `invoke_handler`
  records the NAME of the synchronous command on the main thread; a
  background thread posts a heartbeat every 250 ms with `run_on_main_thread`.
  `[main] no main-thread turn for Nms (command=X|none)` after 2 s, one
  in-process main-thread stack sample after 5 s (`thread_suspend` +
  frame-pointer walk through `mach_vm_read_overwrite`, symbols named later
  by `dladdr` on its own thread; image basenames and symbol names only, no
  process started), `[main] main thread back after Nms`, and `[main] command
  X held the main thread Nms` for a command ≥500 ms. Kept on purpose: the
  sample is what was missing on 11-26 and 11-27, and it found `queue_list`
  in its first run (below).
- **Attach** (`pty.rs`, header "Attach lifecycle"): `attach_session` is async;
  the blocking part runs on a pool thread and the caller is answered within
  5 s. Each request is InFlight → Delivered / Abandoned / Superseded under
  the attachment table's one lock; a late attach is retired (its client
  killed and reaped, master closed), never inserted, never emitted; a
  delivery that beats the timeout decision is a success with exactly the
  inserted generation; at most 4 attach workers (`terminal-busy`); detach and
  `detach_all` supersede the attach in flight. `attached()` does not count
  an attach in flight (a bell then is not one the person saw); the frontend's
  `hasPane` counts the pane from its creation. Activity guard: held for the
  whole worker (an attach never races a server replacement; a restart is
  refused busy while a stalled spawn holds one). Reapers that cannot reap
  are counted and logged.
- **Pane client** (`pane_client.rs`): std `Command` + `pre_exec` (signals to
  default, setsid, `TIOCSCTTY`, inherited fds marked close-on-exec without
  allocating); same program, argv, environment and cwd as before; its one
  `Command::new` is a reviewed site in `tests/edr_quiet.rs`. The reaper now
  closes the master 300 ms after the kill if the client has not gone (a
  session leader exits only after its tty's output drained; nothing reads
  the master after retire — found by the same unit test).
- **Start / kill** (`commands.rs` `session_op`): 10 s / 5 s; `terminal-timeout`
  when nothing irreversible the caller does not know about happened (the
  launch command is then WITHHELD), `start-unknown` once the command was
  being typed (the frontend marks the card launched). A late or
  deadline-cut start is reconciled under the lifecycle operation lock: a
  `cardless` start (`createStarted`) → the session is an orphan and is killed
  with its checkpoint; a reopened stopped card → kept. "tmux did not answer"
  is re-checked every 5 s for up to 5 min (a stopped server may still run
  the cut client's request after it continues). A late kill keeps the card
  and its recovery material. `mcp_start_session` holds the MCP delivery lock
  only to read the plan and check the fences, and re-checks them afterwards.
- **Moved off the IPC thread**: the full list is in `docs/ipc-thread.md`
  (clipboard, links, hooks, project environment, queue send-now/probes and
  `queue_list`, tmux status, the MCP delivery-lock commands, export, …).
  The scheduler tick's pane listing now has a 5 s deadline.
- **Frontend** (`pane-wait-model.js`, `layout.js` header): "Connecting
  terminal…" after 1 s; "The terminal did not start within 5 s" + [Retry];
  "The terminal shows nothing yet" + [Reconnect] 3 s after an attach without
  a first frame; "Starting…" / "The start did not finish in time" + [Retry]
  for a stopped card; toasts for a new card's start and a close that did not
  answer. No toast for the pane waits; six languages.

## Verification

| What | Where / build | Result |
|---|---|---|
| Unit: attach lifecycle (late retired, superseded by attach/detach/`detach_all`, delivery-vs-timeout race, worker bound, input bound to a timed-out attach never reaches the successor), pane client (ctty, SIGWINCH, env, exec error, reap), `session_op` (timeout / unknown / answered, worker bound), late start reconcile, late kill keeps material, deadline-cut start, clipboard budget, watchdog lines and stack sample | `cargo test`, this Mac | PASS |
| Frontend: waits mirror the backend, notices and actions, retry never for a dead/closing/replaced pane or during a restart, keys typed while connecting are cancelled and never reach the retry | `scripts/ui-tests` | PASS 1051/1051 |
| Census: every synchronous command classified, none spawns or calls tmux, the moved commands stay async | `tests/ipc_thread.rs` | PASS |
| WK `ipc-stall`, run 1 | air2, macOS 27.2.1, debug carrier | FAIL, and a finding: with the smoke's tmux server SIGSTOPped, `queue_list` held the main thread 195 s; the watchdog logged `no main-thread turn … (command=queue_list)`, the stack (`queue_view → scheduler_pane_listing → tmux → Command::output`), `held the main thread 195668ms` and `back after 195558ms`. Fixed (moved off). The page could not continue the server itself; it was continued by hand after checking the pid, start time, program and socket against the experiment registry. |
| WK `ipc-stall`, run 4 (final code) | air2, same | **PASS 13/13**: attach timeout notice at 5040 ms, the window answered (worst IPC 2 ms) and card B echoed while A's spawn was held; the late attach retired (0 pane clients, 0 workers, 0 reapers); [Retry] attached and drew; keys typed while connecting reached no pane; a held start: no card, answered at 10006 ms, nothing left after release; a held kill: card kept at 5019 ms, closed on the second try; a held pbcopy: failed at 3004 ms; stopped tmux server: new card refused at 10004 ms, close refused and kept, window answering; after continue no orphan session, card closed. No `[main]` stall line, no `poll idle`. Registry cleanup exit 0, nothing left, machine state identical before/after. Runs 2–3 failed only on the carrier typing into zsh during the split's resize redraw (fixed in the carrier). |
| WK regression, candidate with the posix_spawn pane client | air2 | FAIL vs baseline: `split-picker-layout`, `completion`, `completion-owner`, `selection-drag-overlay`, `selection-up` (`run`) and `ux-terminal-click`/`-drag`/`-link` (`ux-layers`; real clicks in the focused window, `terminal-click stale`) — tmux pane size ≠ xterm size. Cause: no controlling tty (above); the pane client was changed to Deck's own fork + `TIOCSCTTY` |
| WK baseline HEAD `a254b81e` (no change) | air2, same | `run` fails `path-editor` (no editor on air2), `input-isolation`, `completion-gap`, `ime-routing`; `ux-layers` fails `ux-spacing` — pre-existing on this machine |
| WK final build: `ipc-stall`, `terminal-input` | air2 | **PASS**, **PASS** |
| WK final build: `run`, `ux-layers` | air2 | only the baseline's own failures (`path-editor`, `input-isolation`, `completion-gap`; `ux-spacing`); `ime-routing` passed this time; the real-click checks `ux-terminal-click`, `-drag`, `-link`, `-shell` pass. No `[main]` stall line and no `poll idle` in any mode; every registry cleanup exit 0 with nothing left and air2's own Deck, `~/.deck` and `deck` server unchanged |

Smoke summaries (verdict, checks, `[main]`/attach/start/close lines; no
content): `smoke/`. Runner: `~/c9r-io/deck-freeze-fix-20261127/mini_run_smoke.py`
(registry-based, as in AGENTS.md).

Not run: the input-method knock-on on air2 (optional); a real stalled
volume (not reproducible without root/macFUSE/WebDAV server); the release
carrier; the first-frame notice ("shows nothing yet") in the real webview
(no fault produces an attach without a first frame; covered by the pane
model test and the backend `no first read` line only).

## Commit, gates, remaining

Gates on this Mac (final code): `cargo fmt --check`, clippy (CI form), the CI coverage step (deck-app 1525 passed, 0 failed; lines 85.05%, functions 82.47%; nothing left in TMPDIR), `scripts/ui-tests` 1051/1051, `check-js-syntax.py`, `check.mjs`, `extension_catalog.py --check`. See also (`docs/verification-status.md`,
2026-11-27 entry). Remaining risk and decisions:

- The root cause — which kernel path made process creation wait on disk7 —
  is not established; next time `[main] main thread stack …` names the user
  stack of the waiting call.
- Keychain status readers (`connector_reset_identity`, `inbound_status`,
  `channel_status`, `mcp_client_add`, `model_api_status`) and the AppKit
  pasteboard reads of the Lens still run on the main thread
  (`docs/ipc-thread.md`). The census checks command bodies only; the
  watchdog is the backstop.
- While a stalled spawn holds an attach or start worker, a tmux restart is
  refused as busy and other session creations get the lifecycle lock's
  "restart in progress" refusal, until that spawn returns.
- Per-session scheduler send workers still run their tmux calls without a
  deadline (the firing window); only that session waits.
- Not pushed, not released; shipping in a Nightly needs the user's
  authorization. The in-process stack sampler is kept (a decision for the
  user: it suspends the main thread for microseconds once per stall ≥5 s).
