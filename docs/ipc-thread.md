# What runs on the window's main thread (IPC-thread census)

A webview's IPC is answered on the window's main thread. An ordinary
(non-async) Tauri command runs there, inside WebKit's URL-scheme callback, and
while it runs nothing else of the window does: no event, no redraw, no other
request, and — through the input method, whose last active client Deck may be —
possibly not other applications' text input either. Two freezes came from
exactly this: on 2026-11-26 a stopped tmux server held `scroll_session` in
`Command::output`; on 2026-11-27 a stalled process creation held
`attach_session` in portable-pty's fork for 88 s
(`docs/evidence/deck-freeze-20261126/`, `docs/evidence/deck-freeze-20261127/`).

The rule: **a command that may wait on a process, tmux, another process's
service, a lock a worker may hold, the Keychain, or a path outside `~/.deck`
does not run on the IPC thread.** `tests/ipc_thread.rs` lists every
synchronous command with its class and the risky tokens its body contains; a
new synchronous command, or a body that gains or loses one of those tokens,
fails that test until it is reviewed here and there.

## How a command leaves the IPC thread

| Mechanism | Where | What the caller gets |
|---|---|---|
| `session_runtime::off_ipc_thread(op, budget, work)` | most moved commands | the work's answer, or `terminal-timeout` at `budget`; the work runs under a deadline scope, so every tmux call in it is bounded by `bounded_output` and every `OperationLock` wait ends with it |
| `session_runtime::off_ipc_answer(op, budget, work)` | `queue_send_now` | the same answer bound, without a deadline scope: the send runs to its end exactly as on a scheduler worker (its firing window is never cut) |
| `pty.rs` `attach_within` | `attach_session` | the attach lifecycle (in flight / delivered / abandoned / superseded) under the attachment table's lock; a late attach is retired, never inserted, never emitted |
| `commands.rs` `session_op` | `start_session`, `kill_session`, `mcp_start_session` | `terminal-timeout` (nothing irreversible was done that the caller does not know about), `start-unknown` (the launch command may have been typed), or the worker's own answer when it beat the budget; a late start is reconciled (`reconcile_late_start`) |

Bounds on stuck workers: at most `MAX_ATTACH_WORKERS` (4) attach workers and
`MAX_SESSION_WORKERS` (4) start/kill workers (plus 4 managed starts) run at
once; past that the next request is refused `terminal-busy` at once. Reaper
threads that cannot reap their client are counted and logged
(`client not reaped … (reapers=N)`).

## Budgets and why

| Command | Budget | Basis |
|---|---|---|
| `attach_session` | 10 s (raised from 5 s: an endpoint-security agent taxes every exec and can slow a shell's start); "Connecting terminal…" after 1 s; "shows nothing yet" after 3 s without a first frame | app.log 2026-11-27, 321 attaches: attach → first read p50 6 ms, p99 32 ms, max 212 ms |
| `start_session`, `mcp_start_session` | 10 s | 197 starts: total p50 30 ms, p99 137 ms, max 153 ms; room for a first Keychain read of a project's variables and an endpoint-security exec tax |
| `kill_session` | 5 s | one tmux call (an MCP pane: one bounded runner stop first) |
| `write_clipboard` | 3 s | one `pbcopy`; past it pbcopy is killed and reaped, never left to write an old copy over a newer one |
| link commands, `voice_open_settings` | 5 s | a stat/canonicalize of a card's directory (often an external volume) and one `open`; a timeout is the closed `link-open-failed` |
| `queue_send_now` | 15 s | the scheduler's own send path, including a dead session's boot wait |
| everything else moved | 5 s (10 s for the Keychain and the relaunch) | metadata reads and config writes |

The shared waits are one list with the frontend (`ui/test/fixtures/limits.json`
`terminal_waits`; `ui/js/pane-wait-model.js`).

## Moved off the IPC thread (2026-11-27)

`attach_session`, `start_session`, `kill_session`, `mcp_start_session`,
`tmux_available` (a `true` is remembered for the run), `write_clipboard`,
`export_logs` (and the menu's export, on its own thread), `resolve_parent_dir`,
`open_target`, `link_remote`, `open_remote_target`, `voice_open_settings`,
`mcp_scope_preview`, `agent_hooks_status`, `agent_hooks_set`,
`agent_lifecycle_hooks_set`, `codex_replies_set`, `turn_summary_set`,
`turn_summary_status`, `agent_pi_replies_set` (Agent config roots may be on any
volume), `project_env_names`/`_set`/`_clear` (Keychain), `queue_send_now`,
`queue_probe_context`, `queue_review_preview`, `queue_review_confirm`,
`queue_list` (its view reads a pane listing — found by the watchdog's stack
sample when the `ipc-stall` smoke stopped the tmux server),
`tmux_server_status`, `defer_tmux_restart` (tmux probes), `mcp_close_admit`,
`mcp_complete`, `mcp_client_delete`, `mcp_output_retention` (they take the MCP
delivery lock, which MCP control threads hold across runner-socket exchanges),
`inbound_setup` (`open`), `relaunch_after_update` (starts the relaunch waiter).

`mcp_start_session` no longer holds the MCP delivery lock across tmux or the
runner's readiness wait: it reads the plan and checks the fences under the
lock, releases it, creates the session, and re-checks the fences under the lock
afterwards (a fence raised meanwhile kills the new session).

## Still on the IPC thread, and why

| Class | Commands | Why it may stay |
|---|---|---|
| Memory | `ping_event`, `pty_write`, `pty_ack`, `detach_session`, `notify_cards`, `voice_snapshot`, … | in-memory state and events; their locks are held by short in-memory sections, never across a wait. `detach_session` waits for no process: the reaper thread kills and reaps |
| DeckFiles | `load_board`, `save_board`, `load_settings`, `save_settings`, queue edits, history, inbound/channel/connector ledgers, … | Deck's own files under `~/.deck` (internal disk), bounded |
| InternalPaths | `detect_editors`, `recent_commands` | fixed paths of the system volume (`/Applications`, the home's shell history), never a card's directory |
| MainThread | `input_source_snapshot`, `set_native_locale` (menu titles), `translation_clipboard_arm`/`_poll`/`_write`, `notify_configure`, `notify_dismiss`, `notify_status`, `reminder_show`, `reminder_request_permission`, `voice_start`/`_stop`/`_cancel` | AppKit/TIS/notification-centre work that must run on the main thread. The pasteboard bridge hops to the main thread itself (`onMain`), so a pool thread would not take the read off it; a pasteboard read can wait for the data's provider (another application) — kept, and only while the Lens's Copied-text tab is armed and focused. The notification bridge only enqueues; its waits (≤5 s) are on its own background queue. Voice dispatches to its own queue. TIS reads in-process input-source state; a stalled input method was not tested |
| Keychain | `connector_reset_identity`, `inbound_status`, `channel_status`, `mcp_client_add`, `model_api_status` | securityd over XPC, normally fast. **Follow-up**: move them off like `project_env_*` |
| Smoke | `smoke_*`, `connector_smoke_*`, `channel_smoke_*` | debug smoke instrumentation, refused outside an isolated smoke launch (`connector_smoke_transport` spawns and calls tmux; smoke only) |

## Limits of the census

`tests/ipc_thread.rs` reads each command's OWN body; a wait reached through
a callee is the class's reviewed claim, not a checked fact. `queue_list`
reached tmux that way (`queue_view` → `scheduler_pane_listing`) and was
found only by the watchdog below; a name-based call graph over the crate was
tried and is too coarse to use (nearly every command "reaches" every helper).
The watchdog is the backstop: a command that holds the main thread is named
in app.log with its stack.

## What the main-thread watchdog says

`main_watch.rs` (contract in its header) logs, by command NAME only:

- `[main] command X held the main thread Nms` — a synchronous command took
  ≥500 ms (rate-limited per command);
- `[main] no main-thread turn for Nms (command=X)` — a heartbeat waited ≥2 s;
  `command=none` means the main thread was busy outside every Deck command
  (AppKit, WebKit, the input method);
- `[main] main thread stack after Nms (command=X): image:symbol < …` — once per
  stall past 5 s, an in-process sample (no process started);
- `[main] main thread back after Nms (command=X)`.
