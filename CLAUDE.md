# deck — development notes

Kanban over tmux sessions. Small by design: tmux owns the shells; deck is only the
board projection plus metadata. Resist adding session management features that tmux
already provides. **No automatic card movement, ever** — the board surfaces
information; the user makes every placement decision.

## App (v0.2, primary) — `app/`

Tauri 2 macOS app. Frontend `app/ui/` is a no-build set of native ES modules
(`ui/js/state.js` shared slots/helpers · `pure.js` DOM-free logic, node-tested ·
`board.js` · `layout.js` splits/terminal host · `selection.js` tmux-owned
terminal selection · `terminal.js` completion/ghost ·
`scheduler.js` queue UI · `dialogs.js` · `persistence.js` · `app.js` boot),
loaded by `ui/index.html`; xterm.js vendored in `app/ui/vendor/`. Backend
`app/src-tauri/src/` is modular: `main.rs` (wiring) · `commands.rs` ·
`scheduler.rs` · `storage.rs` · `pty.rs` · `tmux.rs` · `history.rs` ·
`smoke_faults.rs` (debug/isolated-smoke only).
Frontend gates: `node --check` (syntax) · `ui/test/*.mjs` (node:test)
· `ui/js/check.mjs` (unresolved identifiers; forbids xterm `._core`).

- Run: `app/run.sh` — builds, wraps the binary in a minimal .app, launches via
  `open`. NEVER run the bare binary from a background shell: outside the GUI
  login session the process can't reach macOS text-input services (TSM/IMK) —
  window and mouse work, keyboard is silently dead. `~/.deck/app.log` (0600)
  collects backend + frontend diagnostics. Frontend logging is STRUCTURED
  ONLY: the `ui_event` command takes a whitelisted code + a detail vetted by
  that code's OWN closed policy (enum values / version pattern — no generic
  slug rule) + two ints, and redacts everything else — never add a free-form
  frontend log channel (log_privacy tests enforce this). Backend log lines
  never interpolate raw error Display text or a raw session NAME:
  `storage::err_code()` maps errors to stable path-free categories (the full
  error goes only to the operation's caller) and `storage::session_tag()`
  gives a per-RUN, non-reversible tag. Every line is redacted again by
  `sanitize_log` on its way to disk (absolute paths, `~/`, any `scheme://`,
  credential prefixes, long opaque tokens, session-name shapes →
  `<redacted>`); exports sanitize their own header AND body instead of
  trusting app.log; `sanitize_existing_logs` migrates logs/exports an older
  deck wrote, in place, at boot (atomic, 0600, no raw copy kept). The
  runtime privacy tests write REAL files through `applog_to` into temp dirs
  — never stub the writer and call it proven. The whole `~/.deck` tree is private by construction
  (dir 0700, every file created 0600 — atomic-write temps, `.bak`,
  `.corrupt-*`, log, exports); `harden_data_dir()` re-migrates legacy modes
  at every boot.
- PTY smoke test (headless): `cargo run --example pty_smoke`
- WKWebView release regression (debug bundles only): launch with a fresh
  absolute `DECK_SMOKE_DATA_DIR`, unique `DECK_SMOKE_TMUX_SOCKET`, and
  `DECK_SMOKE_WKWEBVIEW=1 app/run.sh`. The production modules run inside the
  real bundled WKWebView; results are closed numeric `smoke-check` events in
  that isolated directory's `app.log`. Release builds ignore these debug-only
  arguments, and the harness must never point at `~/.deck`.
- Board persistence: `~/.deck/deck.json` (frontend owns the state). EVERY
  mutation enters one global persist-before-commit transaction queue and builds
  its candidate from the latest committed Board only when it reaches the head;
  debounced mutations enter that same queue before an immediate-operation
  barrier. A rejected mutation or failed write cannot poison the following
  transaction, resurrect a removed card, or overwrite a concurrent rename/move.
  Runtime-only card fields are merged from the newest live state at commit.
  `storage.rs` is TYPED and durable for all four data files
  (deck/queue/history/settings): JSON + version envelope + business-structure
  validation on load — BoardDoc/SettingsDoc validate via `try_from`
  (referential rules: unique ids, cards reference an existing project and a
  column of that project, ≥1 column per project, runtime fields present,
  session names by the same tmux rule the runtime enforces), and
  save_board/save_settings run the SAME validation before touching disk;
  unknown extension fields round-trip untouched. Damaged main quarantined to
  a unique `.corrupt-<ts>`
  BEFORE the fully-validated `.bak` is tried; recovery warnings returned
  in-band (`LoadedDoc {data, source, warning}`); future schema versions
  refused untouched (save refuses to overwrite them too); recovery never
  writes; a load FAILURE is surfaced, never treated as a first run — the UI
  must never auto-save defaults over an existing file. Writes: unique temp +
  fsync + rename + parent-dir fsync, `.bak` written the same way. The
  envelope is validated STRICTLY: only a document carrying neither
  `schema_version` nor `data` is legacy v0; once either appears the file
  must be a COMPLETE envelope with a non-negative INTEGER version (string /
  fractional / negative / null version, or a version without data, is
  damage → recovery), and `save` refuses to overwrite a malformed or future
  envelope.
- Terminal selection has one authority: tmux copy-mode owns anchor, active
  endpoint, history position, highlight and hard/soft-wrap semantics; PTY
  repaint makes that highlight visible in xterm. `selection.js` owns a primary
  gesture at pointerdown (blocking its compatible mouse sequence), promotes
  drags to tmux, and replays only a sub-threshold click to xterm. It translates
  pointer coordinates using only the public `.xterm-screen` rectangle and
  public xterm cols/rows, coalesces IPC updates, holds pointer capture through
  edge scrolling, and cancels on Escape, blur, visibility, pane detach or
  disposal. tmux endpoints are directional and end-exclusive; cell columns
  must be converted to grapheme steps before cursor movement. Never add a
  mirror document, transparent textarea, or xterm `._core` dependency. ⌘C
  asks tmux for exactly the current logical selection
  and writes it through native `pbcopy`; no selection is a clipboard no-op.
  History is 50,000 rows and clipboard extraction is explicitly capped at
  64 MiB without truncation. During selection tmux freezes the reading frame
  while the PTY stream continues through its bounded ACK gate.
- The completion bar is a real flex row inside one pane, never an overlay.
  Its generation-based transition is: detach/hide old owner, refit old owner,
  mount new owner, refit new owner. Stale RAF work must not resize a newer
  owner, and each pane preserves its own bottom-follow/scrollback position.
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
  is what releases a stalled emitter; stalls are logged). The gate tracks an
  emitted HIGH-WATER mark: an ACK counts only for acked < seq ≤ emitted on an
  open gate, so a buggy/hostile webview ACKing sequences never sent cannot
  widen the window; a failed app.emit ends the pump and closes the gate (the
  webview can never ACK an event it never received); seq overflow ends the
  stream cleanly. A wedged webview
  therefore stalls emitter → bounded channel → kernel PTY → tmux client, with
  memory bounded at ~1.5MB per attachment. The frontend drops (without ACKing)
  events whose gen is older than the current attachment, and accepts+adopts a
  NEWER gen (the first event can beat the attach invoke's resolution).
- Scheduled prompts: Rust-side scheduler thread (NOT webview timers — App Nap
  freezes those), 20s tick, queue persisted at `~/.deck/queue.json` and loaded at
  boot. Injection = ONE atomic tmux command: literal text + trailing CR in a
  single `send-keys -l` (no "text landed but Enter didn't" window), no attach
  needed; dead sessions are started first. "chain" mode fires after
  `window_activity` has been quiet ≥180s (a permission prompt also counts as
  quiet — documented behavior; quiet NEVER means "the agent finished").
  Round-2/3 semantics (scheduler.rs is the reference, all unit-tested):
  - at most ONE candidate per session per tick, ≥60s between any two
    injections into the same session; each due session gets its own
    short-lived worker thread claimed via a busy-set, so sessions are truly
    independent (a 2.5s session-boot wait delays only its own session), the
    same session never has two concurrent sends, and a worker outliving its
    tick can't collide with the next tick;
  - deterministic priority: backoff-elapsed retry → earliest-due `at` →
    cadence-due `every` → chain; a future `at` never blocks a due one;
  - each worker re-selects from fresh state under the lock (`send_one` is
    the whole firing state machine, testable with fake fire/persist);
    chain steps carry explicit `group`/`seq` (legacy files migrate from
    array adjacency);
  - the firing contract: while an item is mid-send, queue remove/update/
    pause/retry/skip return a conflict error (UI toasts it) and the item
    survives until finalize;
  - EVERY user-driven mutation goes through `with_queue` (persist-then-
    commit): clone the state, mutate the CANDIDATE, persist, only then swap
    it in — a rejected mutation or a failed save leaves memory byte-identical
    to disk, so the scheduler never acts on a change the user was told
    failed. The two POST-send transitions are deliberately the opposite: the
    injection cannot be rolled back, so memory takes the new state and a
    failed write sets `Queues.dirty`, warns the user, and is retried by
    `flush_dirty` every tick — that retry is what stops a definitively
    NOT-sent prompt from being counted as delivered after a restart;
  - deleting a card/project is PERMANENT cancellation: `queue_clear_session(s)`
    tombstones the session (`cancelled`, capped 500) and drops ALL its items
    INCLUDING one mid-send — that delivery still finalizes from the
    pending-ledger snapshot (the audit completes), but no rule is restored,
    no template step spawned, no cadence and no send-gap entry left behind,
    and a tombstoned session is never eligible again (so `fire_item` cannot
    restart it). A delete landing while a worker is inside its injection is
    reaped afterwards (`SendHooks.kill`); scheduling for a session again
    clears its tombstone. The frontend removes a card ONLY after that
    cancellation is on disk — close, project delete (one atomic
    `queue_clear_sessions`) and the shell-exited auto-retire share the path,
    and a failure keeps the card with an explicit toast. The frontend then
    kills every tmux session (already-missing is success) and persists the
    candidate Board BEFORE committing removal; any kill/save failure keeps
    the card/project and pane visible for retry;
  - a step that exhausts its 8 attempts BLOCKS its group until the user
    retries/skips/removes it (queue_retry / queue_skip commands);
  - a recurring rule has at most one active iteration (its spawned steps,
    keyed `rule`/`group`=delivery id) — iterations never interleave;
  - firing intent + delivery id + a full item snapshot (the `pending` ledger)
    is persisted BEFORE injection. A live confirmed success uses idempotent
    `finalize_delivery` (fired count, until-N retirement, template-step spawn,
    audit record — `deliveries`, capped 200). A persisted `firing` found after
    a crash becomes `ambiguous`: it is never auto-retried or silently counted.
    User acknowledge finalizes it once; risk-accepting retry clears the ledger
    and re-arms it, both persist-then-commit and idempotent. A definitively
    refused atomic injection becomes retryable `failed`; if that post-failure
    save and the process both fail, the old disk intent is honestly ambiguous.
    Boot recovery installs every persisted `firing`/orphan ledger entry as
    in-memory `ambiguous` BEFORE attempting the repair write. If that write
    fails, the ambiguous actions remain available and unschedulable while
    `dirty` retries the exact snapshot. `flush_dirty` runs before the
    empty-queue fast path.
- File drop / image paste into a terminal pane (Warp-style): WKWebView
  surfaces external files as CONTENT with no path, so the frontend reads the
  bytes and `save_dropped_file` persists them 0600 under `~/.deck/drops`
  (0700, week-old entries pruned at boot); the returned path is typed into
  the session shell-quoted, no Enter. Card/pane DnD is distinguished by the
  `text/deck-session` payload; file drags show a plain accent outline, not
  the split dropzone.
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
- Inline rename is a one-shot edit lifecycle: Enter commits once (and removes
  the editor before async persistence), Escape restores without persistence,
  blur commits once, and composition/`keyCode 229` Enter is ignored. Persistence
  failure rolls every visible title back with an explicit toast.
- Terminal path menus resolve parent directories in Rust without invoking a
  shell: quoted/relative/absolute/Unicode paths and optional `:line[:column]`
  suffixes are normalized against the canonical session cwd, but a literal
  existing colon-number filename wins. New-session-in-parent starts tmux before
  the Board transaction and kills only that new session on save failure.
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

- `src/model.rs` — `Board`/`Card` structs, JSON persistence in `~/.deck/board.json`,
  id/session-name generation. Columns are the fixed `COLUMNS` array; a card's
  `column` is an index into it. The TUI holds the SAME privacy/durability
  contract as the app backend (its own small copy, not a dependency): dirs
  0700 and files 0600 at CREATION, boot-time idempotent migration
  (`harden_data_dir`), atomic + durable saves (unique temp → fsync → rename →
  parent-dir fsync) keeping `board.json.bak`, `.bak` fallback for a damaged
  main file, and notes created by deck — never by `$EDITOR` under the ambient
  umask — behind a card-id alphabet check so a note can never escape
  `notes/`. Path-taking variants (`load_from` / `save_to` / `notes_path_in` /
  `prepare_notes`) exist so no test touches the real `~/.deck`.
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
