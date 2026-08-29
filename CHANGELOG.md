# Changelog

## 0.4.36 — 2026-08-29

- Add one closed theme registry for application CSS, native window chrome,
  xterm cursor/selection and ANSI colors, and tmux copy-mode highlighting.
  Deck Dark remains the default; Light, live system appearance, High Contrast,
  and reviewed teal/blue/purple/orange accents are available in Settings.
- Apply the persisted theme before revealing the hidden window, update every
  existing terminal pane in place, make new splits inherit it, and roll the UI
  back with an explicit message when settings persistence fails.
- Add typed Rust and frontend validation/migration, complete-token and
  WCAG-contrast checks for every theme/accent pair, fixed-vs-system listener
  tests, DOM rollback coverage, and real-WK smoke stages for switch/rollback.
- Make native xterm word/line selections use the same prevented, case-insensitive
  Command-C clipboard path as tmux drag selections.
- Freeze a drag at the pointer cell without one final edge-scroll step, and
  synchronize/validate xterm and tmux dimensions so resize reflow cannot shift
  selection endpoints by characters or rows.
- Stabilize the real-WK link-classifier smoke by waiting for the synchronized
  terminal grid and selecting fixture output rather than the shell's echoed
  `printf` command. Two consecutive isolated production-module runs pass.

## 0.4.35 — 2026-08-29

- Add complete English and Simplified Chinese UI localization with immediate
  runtime switching, system-locale detection, native menu localization and
  strict dictionary/static coverage.
- Automatically bind scheduled work to its card and full tmux
  server/session/window/pane/pid identity, with an optional foreground
  executable derived from the launch command or live non-shell pane.
- Remove readiness hooks and user-configured safety policies: an exact pane is
  always required, an automatically captured process must match, and items
  without one retain same-pane compatibility delivery.
- Replace the fixed boot sleep with bounded cancellable target polling and
  atomically guard both identity and optional process at literal paste time.
  Context waiting consumes no attempts; process mismatch has a one-shot
  pointer-confirmed send, while identity replacement requires explicit rebind.

- Route printable macOS IME `keyCode=229` events through the final native
  `InputEvent` path, avoiding xterm's deferred keydown fallback that could
  drop the first Pinyin punctuation press in WKWebView.
- Keep modifier-only Shift keydowns out of xterm's byte path so its transient
  keydown flag cannot suppress the first Pinyin InputEvent of each Shift chord.
- Replace the overlapping terminal-link regex with a single-pass tokenizer:
  complete HTTP(S) URLs own their interval across soft wraps and full-width
  tmux redraw rows, while path-like tokens only become links after the backend
  resolves a real local target in the pane cwd. Menus visibly wrap the complete
  value; IPv4 addresses, nonexistent log filenames and URL `/api` fragments no
  longer open path menus.
- Keep real pointer clicks on xterm's trusted link state machine, remove
  synthetic click replay, prevent the compatibility click after mouseup from
  immediately closing the path menu, and recognize wrapped/Unicode/quoted,
  absolute, relative and `file:line[:column]` paths with public buffer APIs.
- Freeze every completed drag into a generation-bound tmux snapshot and native
  clipboard route. Immediate Command-C waits for the final pointer update;
  stale tokens and disappeared selections fail with closed diagnostic stages.
- Decouple completed selection endpoints from tmux's viewport cursor. A
  public-geometry overlay follows immutable content rows while scrolling, and
  clipboard bytes remain identical before and after the scroll.
- Remove pointer-time `disableStdin`, let composition/Process/Dead/Compose
  events bypass Deck shortcuts, and leave macOS Option/dead-key processing to
  the input method (`macOptionIsMeta: false`).
- Add exact path grammar, overlay geometry, IME routing, frozen-scroll tmux
  contract and real-WKWebView provider/clipboard/scroll smoke coverage.
## 0.4.34 — 2026-08-29

- Make repeated terminal drags replace the prior tmux selection reliably,
  accept the valid zero-cell anchor state, and invalidate late gesture replies.
- Coalesce each burst of PTY repaint bytes before handing it to xterm so tmux
  selection and scroll redraws cannot expose a partially painted frame.
- Drive terminal wheel input at display-frame cadence, preserve fractional
  trackpad deltas, serialize requests, and execute each tmux scroll as one
  server command list instead of several subprocess round trips.
- Add real-WKWebView release regressions for immediate repeated selection and
  fractional pixel/line-mode wheel routing.

## 0.4.33 — 2026-08-29

- Snapshot terminal selections through tmux's native copy buffer so ongoing
  pane output cannot drift stale `capture-pane` coordinates and silently copy
  the wrong rows.
- Add a deterministic contract that grows history before every legacy capture
  step and verifies the production snapshot byte-for-byte against tmux's
  selection, including buffer cleanup and disappeared-selection handling.

## 0.4.32 — 2026-08-29

- Add direct, tmux-owned terminal drag selection that continuously crosses
  screens in either direction, supports reverse shrinking and split isolation,
  and copies exact logical text with explicit 50,000-row/64 MiB boundaries.
- Give every primary-pointer gesture one owner from pointerdown, replay only
  sub-threshold clicks to xterm, and honor tmux's exclusive end column and
  Unicode cell/grapheme boundaries without an extra copied character.
- Remove the auxiliary long-scrollback copying surface and all of its entry
  points, shortcuts, backend capture protocol, styles, listeners and tests.
- Make completion ownership transitions refit both the old and new split panes,
  cancel stale animation frames, and keep xterm and PTY rows synchronized.
- Give natural-exit retirement per-session destructive ownership so overlapping
  polls or manual close operations cannot duplicate close/success callbacks,
  while unrelated sessions continue progressing.
- Add debug-only, isolated WKWebView fault hooks and production-path smoke gates
  for Board persistence recovery, ambiguous delivery repair, natural-exit
  failures, completion ownership, and direct terminal selection.
- Raise tmux history to 50,000 rows and add deterministic Unicode/wrap selection
  contracts at 2,500 and beyond 20,000 rows.

## 0.4.31 — 2026-08-28

- Serialize every Board mutation through one persist-before-commit transaction
  queue, including debounced edits, destructive closes, project deletion and
  natural shell exit; failed writes remain visible and retryable without
  dropping later mutations.
- Keep crash-recovered scheduler deliveries visibly ambiguous in memory even
  when the recovery write fails, and retry that dirty snapshot without ever
  making it schedulable again.
- Make inline rename IME-safe and single-commit across Enter, Escape and blur,
  with durable rollback and immediate updates in Board, sidebar and pane titles.
- Improve edge scrolling in the then-existing auxiliary scrollback view
  (the auxiliary view was retired in 0.4.32).
- Expand file-path actions with safe parent-directory resolution, opening the
  parent in the configured editor and creating a session there without a shell.
- Reserve real pane layout space for completion candidates, refit xterm and PTY
  rows, and keep adjacent split panes unaffected.
- Add production-module DOM regressions and an isolated, in-app WKWebView smoke
  harness covering concurrent Board mutations, rename, copy, paths and layout.

## 0.4.30 — 2026-08-28

- Replace silent assumed-sent crash recovery with an explicit ambiguous
  delivery state and idempotent acknowledge/retry decisions.
- Keep dirty queue persistence retries alive after the last once item leaves
  the in-memory queue.
- Make card/project deletion contingent on successful tmux shutdown and a
  durable Board save; failures keep sessions visible and retryable.
- Fix sidebar Rename for non-active sessions and group sidebar sessions by
  Board, with Board order, counts, and stable waiting/running/stopped order.
- Improve native clipboard and scrollback-capture exactness for the
  then-existing auxiliary view (retired in 0.4.32).
- Redact assignment, JSON, quoted and ANSI-wrapped paths, URLs and credentials
  throughout app logs, migrations and exports.
- Refuse to overwrite invalid JSON, malformed envelopes, wrong typed structure,
  or unreadable existing data; never poison a valid backup with damaged main
  bytes.

## 0.4.29 — 2026-08-28

- Added an auxiliary scrollback view (retired in 0.4.32), strict schema-envelope checks,
  durable queue mutations, completion-bar placement fixes, and expanded log
  privacy protections.
