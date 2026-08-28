# deck — development notes

Kanban over tmux sessions. Small by design: tmux owns the shells; deck is only the
board projection plus metadata. Resist adding session management features that tmux
already provides. **No automatic card movement, ever** — the board surfaces
information; the user makes every placement decision.

## App (v0.2, primary) — `app/`

Tauri 2 macOS app. Frontend `app/ui/` is a no-build set of native ES modules
(`ui/js/state.js` shared slots/helpers · `pure.js` DOM-free logic, node-tested ·
`board.js` · `layout.js` splits/terminal host · `terminal.js` completion/ghost ·
`scheduler.js` queue UI · `dialogs.js` · `persistence.js` · `app.js` boot),
loaded by `ui/index.html`; xterm.js vendored in `app/ui/vendor/`. Backend
`app/src-tauri/src/` is modular: `main.rs` (wiring) · `commands.rs` ·
`scheduler.rs` · `storage.rs` · `pty.rs` · `tmux.rs` · `history.rs`.
Frontend gates: `node --check` (syntax) · `ui/test/pure.test.mjs` (node:test)
· `ui/js/check.mjs` (unresolved identifiers; forbids xterm `._core`).

- Run: `app/run.sh` — builds, wraps the binary in a minimal .app, launches via
  `open`. NEVER run the bare binary from a background shell: outside the GUI
  login session the process can't reach macOS text-input services (TSM/IMK) —
  window and mouse work, keyboard is silently dead. `~/.deck/app.log` (0600)
  collects backend + frontend diagnostics. Frontend logging is STRUCTURED
  ONLY: the `ui_event` command takes a whitelisted code + short slug + two
  ints and rejects everything else — never add a free-form frontend log
  channel (log_privacy tests enforce this).
- PTY smoke test (headless): `cargo run --example pty_smoke`
- Board persistence: `~/.deck/deck.json` (frontend owns the state, saves wholesale,
  debounced). storage.rs is TYPED and durable for all four data files
  (deck/queue/history/settings): JSON + version envelope + business-structure
  validation on load; damaged main quarantined to a unique `.corrupt-<ts>`
  BEFORE the fully-validated `.bak` is tried; recovery warnings returned
  in-band (`LoadedDoc {data, source, warning}`); future schema versions
  refused untouched (save refuses to overwrite them too); recovery never
  writes; a load FAILURE is surfaced, never treated as a first run — the UI
  must never auto-save defaults over an existing file. Writes: unique temp +
  fsync + rename + parent-dir fsync, `.bak` written the same way.
- One poll command (`poll_sessions`) returns liveness + `#{window_activity}` recency +
  process-tree RSS (pane_pid → ps tree walk) + tail previews. Frontend polls every
  2.5s and diffs into granular UI events (status/mem/output) — never full re-renders
  on output.
- Status semantics: green = output <15s ago; amber "waiting" = alive but quiet ≥15s
  (honest heuristic — may be waiting for input, may be a silent build); gray = no
  session.
- tmux ships INSIDE the app: a statically linked binary (see
  `binaries/build-tmux.sh`, committed as `binaries/tmux-aarch64-apple-darwin`,
  bundled+signed via tauri `externalBin`). `tmux_bin()` prefers the sidecar,
  then Homebrew/MacPorts probes. deck talks to its OWN server (`-L deck`
  socket) — never version-clashes with a user tmux, and deck sessions don't
  appear in the user's `tmux ls`. Debug: `tmux -L deck ls`.
- Attach = `tmux attach` inside a portable-pty, bytes streamed as base64 over the
  `pty-data` event to xterm.js; detach kills only the tmux *client*. Reader threads
  carry a generation counter so a stale thread never removes a newer attachment.
  Flow control is END-TO-END: pty-data events carry `gen`+`seq`; the frontend
  ACKs (`pty_ack`) only after xterm's write callback, and the emitter never
  runs more than MAX_INFLIGHT_BATCHES (4 × ≤256KB) past the last ACK — past
  that it waits on the attachment's AckGate (closed by detach/re-attach, which
  is what releases a stalled emitter; stalls are logged). A wedged webview
  therefore stalls emitter → bounded channel → kernel PTY → tmux client, with
  memory bounded at ~1.5MB per attachment. The frontend drops (without ACKing)
  events whose gen is older than the current attachment, and accepts+adopts a
  NEWER gen (the first event can beat the attach invoke's resolution).
- Scheduled prompts: Rust-side scheduler thread (NOT webview timers — App Nap
  freezes those), 20s tick, queue persisted at `~/.deck/queue.json` and loaded at
  boot. Injection = `tmux send-keys -l` (literal) + Enter, no attach needed; dead
  sessions are started first. "chain" mode fires after `window_activity` has been
  quiet ≥180s (a permission prompt also counts as quiet — documented behavior;
  quiet NEVER means "the agent finished"). Round-2 semantics (scheduler.rs is
  the reference, all unit-tested):
  - at most ONE candidate per session per tick, ≥60s between any two
    injections into the same session; sessions are independent;
  - deterministic priority: backoff-elapsed retry → earliest-due `at` →
    cadence-due `every` → chain; a future `at` never blocks a due one;
  - each send re-selects from fresh state under the lock (pause/edit/remove
    races are closed); chain steps carry explicit `group`/`seq` (legacy
    files migrate from array adjacency);
  - a step that exhausts its 8 attempts BLOCKS its group until the user
    retries/skips/removes it (queue_retry / queue_skip commands);
  - a recurring rule has at most one active iteration (its spawned steps,
    keyed `rule`/`group`=delivery id) — iterations never interleave;
  - delivery is at-most-once: firing intent + delivery id persisted BEFORE
    injection; success and crash recovery share one idempotent
    `finalize_delivery` (fired count, until-N retirement, template-step
    spawn, audit record — `deliveries`, capped 200). The crash window
    between persist-intent and Enter cannot be closed: recovery assumes
    sent, never re-sends.
- The GUI design reference (mock, same UI with fake data) lives in `gui/index.html`.

### WKWebView / Tauri gotchas (each cost a real bug)

- `body { user-select: none }` makes WebKit refuse keyboard input in any
  textarea/input that inherits it — including xterm's hidden helper textarea
  (symptom: terminal renders, chips inject fine, typing does nothing). Keep the
  `input, textarea { -webkit-user-select: text }` override.
- macOS windows must set `"dragDropEnabled": false` in tauri.conf.json or the
  native file-drop handler swallows all HTML5 drag events (symptom: cards can't
  be dragged between boards). Chromium-based testing of the mock never catches
  either issue — verify webview-specific behavior in the real app.
- A menu-less macOS app gets no standard Edit actions, so ⌘C/⌘V are implemented
  inside xterm's attachCustomKeyEventHandler via navigator.clipboard.
- The frontend is embedded at COMPILE time and cargo does not track it: without
  the `cargo:rerun-if-changed=../ui/...` lines in build.rs, UI-only edits build
  in 1s as a no-op and the app silently runs the previous UI. (Bit us: command
  capture appeared broken because the binary shipped a stale frontend.)
- Tauri v2 requires `src-tauri/capabilities/default.json` granting `core:default`
  or `event.listen` is REFUSED with a silent promise rejection — invoke (JS→Rust)
  works, events (Rust→JS) never arrive, so the terminal receives no output while
  everything else looks healthy. The boot-time deck-ping self-test in ui/index.html
  catches this class of failure; keep it. Always `.catch(ulog)` on listen().

## TUI (v0.1, legacy) — `src/`

- `src/model.rs` — `Board`/`Card` structs, JSON persistence in `~/.deck/board.json`
  (atomic write via tmp+rename), id/session-name generation. Columns are the fixed
  `COLUMNS` array; a card's `column` is an index into it.
- `src/tmux.rs` — all tmux subprocess calls. Every call captures stdout/stderr
  (never inherit — stray tmux stderr corrupts the TUI). Pane-level targets
  (`send-keys`, `capture-pane`) need `=name:`; session-level targets need `=name`
  (tmux 3.7 parses a bare `=name` pane target as a lookup failure).
- `src/app.rs` — state machine. `Mode` (Normal / Input / Confirm) drives key routing.
  Actions that need the real terminal (attach, $EDITOR) are queued as `Action` and
  executed by the main loop, which suspends/reinits ratatui around them.
- `src/ui.rs` — rendering only, no state changes.
- `src/main.rs` — event loop; polls keys at 200ms, refreshes tmux state at 1s.

## Invariants

- Sessions are started with a plain shell + `send-keys` of the command, NOT by exec'ing
  the command, so the session survives agent exit and scrollback stays inspectable.
- Deleted cards are moved to `board.archived`, never dropped.
- `crossterm` comes via `ratatui::crossterm` re-export — don't add a separate
  crossterm dependency (version-mismatch hazard).

## Testing

Headless smoke test: run deck itself inside tmux and drive it with send-keys:

```bash
tmux new-session -d -s deck-test -x 180 -y 40 target/debug/deck
tmux send-keys -t deck-test s        # press a key
tmux capture-pane -p -t deck-test    # read the screen
tmux send-keys -t deck-test q && tmux kill-session -t deck-test
```

Use harmless card commands (e.g. `while true; do date; sleep 1; done`) when testing —
a card whose command is `claude` will really launch Claude Code.
