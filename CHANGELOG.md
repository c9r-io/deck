# Changelog

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
- Add continuous up/down edge auto-scroll to native long-output selection while
  keeping selection copy separate from verified `Copy all`.
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
- Route long-output copying through the native macOS clipboard, preserve exact
  text, report verified failures, prevent stale captures, and expose reliable
  20,000-row truncation metadata.
- Redact assignment, JSON, quoted and ANSI-wrapped paths, URLs and credentials
  throughout app logs, migrations and exports.
- Refuse to overwrite invalid JSON, malformed envelopes, wrong typed structure,
  or unreadable existing data; never poison a valid backup with damaged main
  bytes.

## 0.4.29 — 2026-08-28

- Added the long-output scrollback copy panel, strict schema-envelope checks,
  durable queue mutations, completion-bar placement fixes, and expanded log
  privacy protections.
