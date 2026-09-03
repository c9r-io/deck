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

- Updates have a closed `stable | nightly` setting; missing, unknown or damaged
  values normalize to Stable. The webview owns no updater capability or URL.
  `commands.rs` maps the enum to exactly one compiled HTTPS endpoint and uses
  `UpdaterExt::updater_builder().endpoints(vec![endpoint])`; a Nightly failure
  never falls back. Tauri 2.10.1 still owns semver comparison, archive download,
  minisign verification and install. Build identity is only numeric version +
  a bounded hex commit from `build.rs`.
- `tmux_lifecycle.rs` owns the server boundary. It inspects a versioned JSON
  server option before scheduler/webview startup, reuses only a compatible
  server, automatically replaces an empty old/legacy server, and persists a
  content-free pending/restart transaction when sessions exist. Never write
  current metadata onto an unknown existing server: that would relabel old
  code as current. Session creation must hold `session_creation_guard`; attach
  to an existing pending session remains allowed. The restart command rechecks
  PID/start-time/session/pane counts under the same gate, detaches PTYs, kills
  and waits, validates a stale socket against its captured device/inode, starts
  from the current sidecar, then requires a new PID and read-back identity.
  The updater takes the same gate before setting its creation embargo. Cards
  are marked stopped before polling so a
  whole-server restart is not mistaken for natural card exits.
- Production Stable/Nightly intentionally share socket `deck` because
  promotion copies identical candidate bytes. Debug development uses
  `deck-dev` and bundle ID `io.c9r.deck.dev`; smoke requires `deck-smoke*` and
  `io.c9r.deck.smoke`. Release creation is allowed only from
  `/Applications/deck.app` or `~/Applications/deck.app` with the adjacent
  bundled helper. Updater installation sets a process-local creation embargo
  before Tauri renames the running app into `tauri_current_app`; a failed
  install clears it, a successful install exits/relaunches from the stable app.
  Increment `SERVER_PROTOCOL` only for a true compatibility break.
- Release operation is documented in `docs/release-channels.md`.
  `scripts/release-version` synchronizes the three numeric source/lock entries;
  `scripts/release_channels.py` is the shared manifest/hash/provenance validator;
  `scripts/check-workflows` runs checksum-pinned actionlint. `nightly.yml`
  builds a signed/notarized immutable candidate and updates `nightly-feed` last.
  `promote.yml` is copy-only: its static gate rejects application build
  commands, and Stable `latest.json` is the last completeness asset. Never
  create a public candidate, Stable tag, feed update or promotion without the
  user's explicit authorization.

- Run: `app/run.sh` — builds, wraps the binary in a minimal .app, launches via
  `open`. NEVER run the bare binary from a background shell: outside the GUI
  login session the process can't reach macOS text-input services (TSM/IMK) —
  window and mouse work, keyboard is silently dead. `~/.deck/app.log` (0600)
  collects backend + frontend diagnostics. Maintainer-only verbose frontend
  events are enabled at launch with `app/run.sh --debug-logging`; there is no
  user setting for them. Frontend logging is STRUCTURED
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
- Clipboard diagnostics are always structured and content-free. Copy records
  terminal key capture, Deck/native/no-selection routing, snapshot loss and
  the `pbcopy`/Web Clipboard writer result. Text paste records the closed chain
  key capture → xterm key handler → native paste event → xterm `onData` → PTY
  write, with bounded missing-stage timers. Only fixed labels and character or
  file counts enter `app.log`; clipboard text, errors and session names never
  do. Per-pane timers are disposed with the pane.
  ⌘C can only report what it FOUND, so `terminal-selection` records the
  selection's own life: `promote` / `start-ok` / `finish-ok` (or
  `start-failed` / `update-failed` / `finish-failed` / `freeze-failed`,
  which previously cancelled behind nothing but a toast), plus one
  `cancel-<reason>` naming every revoke — pointer, pointer-cancel, blur,
  hidden, input, escape, focus, live, exit, leave, dispose. That is what
  separates a `terminal-copy keydown-none` caused by a drag that never
  promoted from one caused by a live selection something took away. A cancel
  with nothing to destroy stays silent, so ordinary clicks do not flood the
  log; a caller that already logged a specific failure passes a null reason
  instead of a second anonymous line. The two integers are a per-label count
  (rows spanned, or 1 when a FROZEN selection died) and the selection's age in
  milliseconds — never text, coordinates of content, or an error string.
  Three forensic labels attribute the dominant field failure (a completed
  selection revoked before ⌘C arrives): `revoker-<class>` pairs with
  `cancel-pointer` and classifies the destroying pointerdown by provenance
  (trusted pointerType mouse/touch/pen/unknown, or synthetic when isTrusted
  is false; its ints are click count and ms since the last pointerup — the
  one label whose `b` is not selection age); `native-cleared` marks an xterm
  selection appearing while Deck owned the drag (WKWebView's late
  compatibility-mouse replay); and `terminal-copy keydown-elsewhere` replaces
  `keydown-none` when another pane still holds a live Deck selection (count +
  its age), separating "revoked" from "⌘C reached the wrong pane".
- PTY smoke test (headless): `cargo run --example pty_smoke`
- The unchanged vendored terminal is `@xterm/xterm` 5.5.0 (the local
  `xterm.js` is byte-identical to the published 5.5.0 artifact, SHA-256
  `1f991ac3b4b283ebf96e60ae23a00a52765dd3a2e46fa6fdda9f1aab032f7495`).
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
  `storage.rs` is TYPED and durable for every persistent JSON document
  (deck/queue/history/settings and per-session shell snapshots): JSON + version envelope + business-structure
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
- Shell restart recovery is deliberately a bounded projection, not process
  serialization. `poll_sessions` returns `pane_current_path` so the frontend
  persists cwd changes into the card. `shell_state.rs` checkpoints only panes
  whose foreground process is a shell, at most every 15s and two panes per
  pass, into separate 0600 typed files (≤256 KiB / 3000 plain-text lines;
  control characters stripped). On a user-opened command-less card,
  `start_session` may use the saved cwd and submits one tmux batch that starts
  an empty server if needed, loads sanitized bytes from Deck's stdin into a
  uniquely named private tmux buffer, then creates the pane with `/bin/sh`.
  That system-shell bootstrap writes the buffer to the NEW pane's stdout,
  deletes it, and execs the user's login shell. The signed `deck-app` binary
  must NEVER be a pane executable: after reboot macOS Local Network Privacy
  can otherwise attribute the exec-replaced shell and all of its descendants
  to Deck while a fresh tmux-created shell works. The text is ordinary tmux
  history (not an xterm/DOM overlay), never argv, a new temp payload, or shell
  stdin; no command, environment, job, process or agent TUI is restored.
  Startup failure degrades to a clean shell, and boot still removes legacy
  `.restore-*` payloads left by deck ≤0.5.1.
  Later checkpoints capture the restored pane output directly—never merge an
  out-of-band prefix, which would duplicate it. Closing a card removes
  main/backup/quarantine/temporary copies; Settings can disable capture and
  clear all snapshots, with an epoch+IO lock preventing an in-flight writer
  from resurrecting cleared data. Never turn this into command replay or raw
  PTY recording.
- Terminal gesture and selection authority is explicit. A sub-threshold
  physical gesture stays on xterm's trusted mouse/link path; no synthetic
  compatibility click is replayed. Crossing the threshold transfers the drag
  to `selection.js`/tmux and clears speculative xterm selection. tmux owns the
  directional, end-exclusive endpoints only while dragging. Endpoints are
  placed with `top-line` + `cursor-down` + `cursor-right` ONLY
  (`copy_cursor_moves`): `start-of-line`/`end-of-line`/`back-to-indentation`
  walk to the ends of the WRAPPED logical line and `cursor-left` lands on a
  wide grapheme's trailing column, so all four leave the visible row. Because
  `cursor-down` snaps the column to a line end until the walk first steps off
  a NON-EMPTY line, the plan descends to the last blank row above the frame's
  first text, wraps out of it with one `cursor-right`, then descends the rest
  — without that, a full-screen agent frame (blank rows on top) selected rows
  the pointer never touched while a shell pane looked fine. Pointerup queues
  the final update, atomically snapshots tmux into a unique buffer, validates
  the pane token, clears tmux's cursor-bound highlight without moving the
  viewport, and installs one immutable backend lease (bytes + absolute content
  coordinates). A plain overlay derived from public `.xterm-screen`, cols and
  rows renders that lease; selection wheel commands only update viewport
  status, so endpoints and copy bytes cannot drift. Pointerup positions the
  final cell with edge scrolling disabled. Every pointer coordinate uses a
  frontend grid confirmed by `pty_resize`; the backend serializes resize reflow
  with selection operations and rejects stale dimensions instead of clamping.
  A completed-selection scroll treats tmux status and xterm `onWriteParsed` as
  unordered: if the frame arrived first, the status completion renders; if the
  status arrived first, the next parsed frame renders. Because the immutable
  lease no longer depends on tmux's copy cursor, scrolling re-anchors that
  cursor to the live input row and publishes `cursor_visible`; xterm removes
  its cursor marker once the input row is outside the viewport instead of
  leaving it fixed on the selected cell. While Deck owns a
  promoted drag, `onSelectionChange` clears any late compatibility-mouse xterm
  selection so a second viewport-fixed highlight cannot survive.
  ⌘C waits for the whole
  chain and reads only the current token. Escape, input/composition, blur,
  visibility, focus change, detach and disposal revoke the lease. Never add a
  transparent textarea or xterm `._core` dependency.
  Gesture promotion is based on crossing a public terminal cell, never an
  arbitrary CSS-pixel distance, and pointerup rechecks the final cell because
  WebKit may coalesce the last pointermove. This is what keeps short one-row
  drags on the same tmux/overlay path as multi-row drags while same-cell and
  double/triple clicks remain native xterm operations. If a native xterm
  word/line range survives until the first wheel frame, read only its public
  `getSelectionPosition()` coordinates, convert visible absolute buffer rows
  with `terminalNativeSelectionCells`, and freeze it in tmux before scrolling.
  Wheel routing keeps an existing Deck token authoritative, adopts an idle
  native range, and otherwise uses ordinary session scrolling.
  Selection never sets `disableStdin`; composition/dead-key events bypass all
  Deck shortcuts, and `macOptionIsMeta` is false so Option remains owned by
  macOS text input. Codex/Claude Up-arrow compatibility is narrowly armed by a
  history recall at the agent prompt and requires a visible continuation row
  located from the first five public xterm cells; it re-enters `term.input` and
  must never capture shell/editor keys or terminal text.
  Terminal links use `tokenizeTerminalLinks`, not an overlapping global regex:
  an HTTP(S) URL consumes its whole logical-line interval before path candidates
  are considered. `terminal_paths_exist` then resolves candidates against the
  pane cwd in one bounded backend call; nonexistent or inaccessible local paths
  never become interactive. Link actions resolve again before opening.
  History is 50,000 rows and clipboard extraction is explicitly capped at
  64 MiB without truncation. During selection tmux freezes the reading frame
  while the PTY stream continues through its bounded ACK gate.
- The completion bar is a real flex row inside one pane, never an overlay.
  Its generation-based transition is: detach/hide old owner, refit old owner,
  mount new owner, refit new owner. Stale RAF work must not resize a newer
  owner, and each pane preserves its own bottom-follow/scrollback position.
- One poll command (`poll_sessions`) returns liveness + `#{window_activity}` recency +
  process-tree RSS (pane_pid → ps tree walk) + the last six non-empty pane rows
  for fixed-height, bottom-aligned card previews. Frontend polls every 2.5s and
  diffs into granular UI events (status/mem/output) — never full re-renders on
  output.
- Status semantics: green = output <15s ago; amber "waiting" = alive but quiet ≥15s
  (honest heuristic — may be waiting for input, may be a silent build); gray = no
  session.
- Agent status hooks (`agent_status.rs`, opt-in): agent CLIs report a CLOSED
  state word (`working | needs-input | turn-done`) via the bundled
  `deck-status-helper` (`app/status-helper/`, standalone zero-dep crate;
  `build.rs` builds it into `binaries/` for tauri `externalBin` on every
  build, and it is copied to `~/.deck/bin` at enable/boot so hook entries
  reference one stable path). Hooks inherit `$TMUX`/`$TMUX_PANE`; the helper
  drains and DISCARDS the hook stdin payload, charset-validates every field,
  and writes one JSON line to the instance's `status.sock` (0600) — routed
  per pane by `DECK_STATUS_SOCK`, which each deck exports into its own tmux
  server env (tmux.rs), falling back to `~/.deck/status.sock`; so an
  isolated/smoke instance receives its own events and never production's.
  The helper can never carry content, exits 0 always, and silently does
  nothing outside a deck tmux pane. The backend listener validates source/state against the module
  registry, requires the event's socket name AND tmux server pid (generation
  stamp — restarted servers reuse pane ids) before resolving pane→session,
  refuses shell-foreground panes, and records the observed foreground
  executable; `poll_sessions` reconciles so the state dies with the process
  that reported it — no TTLs and no per-agent executable lists. Frontend:
  `effectiveCardStatus` (pure.js) — agent state OUTRANKS the 15s heuristic
  (card statuses `attention`/`done`; a working agent never shows amber). The
  Settings toggle is the ONLY writer of `~/.claude/settings.json` (three
  entries: UserPromptSubmit→working, Notification matcher
  `permission_prompt|idle_prompt`→needs-input, Stop→turn-done); install and
  uninstall touch only entries containing `.deck/bin/deck-status-helper`,
  preserve everything else including file mode, and never modify a malformed
  file. The toggle state is DERIVED from that file — never stored twice.
  Claude Code's Stop does not fire on Esc-interrupt; foreground
  reconciliation and the next UserPromptSubmit heal that. The Codex module
  uses lifecycle hooks in `$CODEX_HOME/hooks.json` (same document shape as
  Claude's, so ONE marker-based JSON merge engine serves both via per-agent
  spec tables): UserPromptSubmit→working, PermissionRequest→needs-input,
  Stop→turn-done, Interrupt→turn-done, all `"async": true` so the helper can
  never block a turn. Multiple hooks per event coexist, so this never
  conflicts with the user's own hooks or `notify` program — the earlier
  notify/`config.toml` route was DROPPED for exactly that conflict (Codex
  allows one notify program only; chain-forwarding it was rejected as too
  much surface). Known caveat: Codex's Stop fires when the model ATTEMPTS to
  stop, so another Stop hook forcing continuation makes "done" slightly
  early. Adding an agent module = one `SOURCES` entry + its own installer
  spec calling the same helper with its own source word, behind its own
  Settings toggle.
- tmux ships INSIDE the app: a statically linked binary (see
  `binaries/build-tmux.sh`, committed as `binaries/tmux-aarch64-apple-darwin`,
  bundled+signed via tauri `externalBin`). `tmux_bin()` prefers the sidecar,
  then Homebrew/MacPorts probes. deck talks to its OWN server (`-L deck`
  socket) — never version-clashes with a user tmux, and deck sessions don't
  appear in the user's `tmux ls`. Production debug: `tmux -L deck ls`;
  source bundles use `tmux -L deck-dev ls`.
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
  boot. Every item persists its card id, optional full tmux
  server/session/window/pane/pid binding, optional sanitized executable
  basename, revision and a closed last-context result (never terminal text,
  arguments or paths). Context protection is automatic and has no saved/UI/API
  policy: an executable derived from the explicit card launch command is
  required in the foreground, otherwise a live non-shell foreground is
  captured at creation, otherwise same-pane compatibility delivery is
  allowed. Hooks, agent class, output activity and
  quiet time never gate delivery. Legacy policy/AgentClass/hook fields are
  ignored and cleaned on the next save without changing schedule/delivery
  state. `context.rs` owns metadata-only probing and sanitization.
  Injection loads literal text + trailing CR into a uniquely named tmux buffer,
  then one synchronous tmux command queue compares the full generation plus
  optional foreground executable and byte-literal-pastes only on a match (no
  attach and no "text landed but Enter didn't" window). The persisted binding
  is a GENERATION STAMP that must hold within ONE delivery, never a permanent
  target: `current_context_probe` re-observes the card's own pane (deck's own
  name on deck's own socket, and a deleted card tombstones its items), the
  readiness probe persists whatever generation it finds, and startup polling,
  the final probe and the atomic paste then require THAT generation — numeric
  tmux ids alone are insufficient there because a restarted server reuses
  them, so server pid is part of the stamp. What decides whether the target is
  the right one is `expected_process`, not the stamp. Never restore a hard
  block on a changed generation: every production upgrade replaces the tmux
  server, so it fired on every item after every update, stalled whole chain
  groups behind their head, rewrote queue.json on each tick and taught the
  user to click a rebind button that only ever confirmed the pane deck had
  already picked. Manual immediate delivery may pointer-confirm a one-shot
  process mismatch bypass; the process comparison is the only thing it can
  bypass. "chain" mode fires after
  `window_activity` has been quiet ≥180s (a permission prompt also counts as
  quiet — documented behavior; quiet NEVER means "the agent finished").
  Round-2/3 semantics (scheduler.rs is the reference, all unit-tested):
  - at most ONE candidate per session per tick, ≥60s between any two
    injections into the same session; each due session gets its own
    short-lived worker thread claimed via a busy-set, so sessions are truly
    independent (a startup wait delays only its own session), the
    same session never has two concurrent sends, and a worker outliving its
    tick can't collide with the next tick;
  - deterministic priority: backoff-elapsed retry → earliest-due `at` →
    cadence-due `every` → chain; a future `at` never blocks a due one;
  - each worker re-selects from fresh state under the lock (`send_one` is
    the delivery state machine and `send_one_safe` is its context-safe front
    half, both testable with fake probe/fire/persist);
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
    is persisted only AFTER readiness, binding persistence, fresh re-selection
    and a final probe, and still BEFORE injection. Blocked context never
    increments attempts, creates a ledger or becomes ambiguous. A live confirmed success uses idempotent
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
