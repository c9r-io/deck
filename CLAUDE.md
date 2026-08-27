# deck — development notes

Kanban over tmux sessions. Small by design: tmux owns the shells; deck is only the
board projection plus metadata. Resist adding session management features that tmux
already provides. **No automatic card movement, ever** — the board surfaces
information; the user makes every placement decision.

## App (v0.2, primary) — `app/`

Tauri 2 macOS app. Frontend `app/ui/` is a single static index.html (no build step,
xterm.js vendored in `app/ui/vendor/`); backend `app/src-tauri/src/main.rs`.

- Run: `app/run.sh` — builds, wraps the binary in a minimal .app, launches via
  `open`. NEVER run the bare binary from a background shell: outside the GUI
  login session the process can't reach macOS text-input services (TSM/IMK) —
  window and mouse work, keyboard is silently dead. `~/.deck/app.log` collects
  backend + frontend (`ui_log`) diagnostics.
- PTY smoke test (headless): `cargo run --example pty_smoke`
- Board persistence: `~/.deck/deck.json` (frontend owns the state, saves wholesale,
  debounced; backend does atomic tmp+rename writes)
- One poll command (`poll_sessions`) returns liveness + `#{window_activity}` recency +
  process-tree RSS (pane_pid → ps tree walk) + tail previews. Frontend polls every
  2.5s and diffs into granular UI events (status/mem/output) — never full re-renders
  on output.
- Status semantics: green = output <15s ago; amber "waiting" = alive but quiet ≥15s
  (honest heuristic — may be waiting for input, may be a silent build); gray = no
  session.
- Attach = `tmux attach` inside a portable-pty, bytes streamed as base64 over the
  `pty-data` event to xterm.js; detach kills only the tmux *client*. Reader threads
  carry a generation counter so a stale thread never removes a newer attachment.
- Scheduled prompts: Rust-side scheduler thread (NOT webview timers — App Nap
  freezes those), 20s tick, queue persisted at `~/.deck/queue.json` and loaded at
  boot. Injection = `tmux send-keys -l` (literal) + Enter, no attach needed; dead
  sessions are started first. "chain" mode fires after `window_activity` has been
  quiet ≥180s (a permission prompt also counts as quiet — documented behavior).
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
