# Changelog

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
