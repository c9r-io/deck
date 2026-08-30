# Changelog

## 0.5.4 — 2026-08-30

- Separate Stable and Nightly updater trust roots. Nightly signing now runs in
  a read-only secret-bearing job and hands verified artifacts to a no-secret
  publisher; promotion keeps tested DMG/archive bytes and creates a new Stable
  detached signature inside the protected production environment. v0.5.4 is
  the single legacy-key bootstrap candidate for existing clients.
- Pin every GitHub Action to a full commit SHA. Scope Cloudflare deployment
  credentials to `website-production`, remove its GitHub token, and keep the
  site job read-only toward the repository.
- Make shell recovery opt-in, redact common credential values and private-key
  blocks, expire snapshots after seven days, and stop creating transcript
  backups. Existing explicit preferences are preserved and disabling remains
  a race-safe wipe of all recovery files.
- Bind destructive tmux restart confirmation to a backend-generated token over
  the exact server socket, session identities, pane IDs/PIDs and foreground
  commands, so equal counts cannot authorize a replacement session.
- Clarify that shell commands, CLIs and agents launched or restored by deck
  inherit its macOS Local Network permission.

## 0.5.2 — 2026-08-30

- Fix restored shell sessions losing access to local-network destinations
  after a machine or tmux-server restart. The old recovery path made the
  signed `deck-app` executable the pane bootstrap, so macOS Local Network
  Privacy could keep attributing the replacement login shell and commands such
  as `kubectl` to Deck even though a fresh session on the same server worked.
- Preserve the same bounded history display without putting Deck in the pane
  process chain: recovery now streams sanitized output through a one-use
  private tmux buffer, a system shell writes it only to pane stdout, deletes
  the buffer, and execs the user's login shell. No history is replayed, exposed
  in argv, or written to a new temporary payload file.
- Add a real-tmux regression contract that starts from an empty private server
  and proves restored text becomes scrollback, command-shaped text stays
  inert, the buffer is deleted, and `pane_start_command` contains no
  `deck-app` executable.

## 0.5.1 — 2026-08-30

- Keep sidebar navigation stable: Boards remain grouped in Board order, while
  sessions retain durable card order instead of moving whenever polling flips
  them between live, quiet and stopped states. Focus and status changes now
  update the existing sidebar entries in place.
- Remove verbose diagnostics from user Settings. Maintainers can launch with
  `--debug-logging`; structured event allowlists, redaction, private log files
  and ordinary always-on diagnostics remain unchanged.

## 0.5.0 — 2026-08-30

- Stop restored, history-heavy panes from flashing through tmux's intermediate
  copy-mode frames while dragging a selection. tmux remains the exact byte and
  history-coordinate authority, while deck paints only each settled range in
  one stable overlay and hides the internal drag cursor.
- Keep Settings inside the window at every supported size: the dialog is wider
  when space allows, vertically centered, bounded by the viewport, and scrolls
  its own contents when the full localized form is taller than the window.

## 0.4.42 — 2026-08-30

- Restore bounded shell output directly into the new pane's real tmux
  scrollback instead of blocking the live terminal with a read-only overlay.
  A one-use private bootstrap writes only to pane stdout, marks the restart
  boundary, then execs the login shell; command-shaped text never reaches
  stdin, and ordinary tmux scrolling, selection and copy work immediately.
- Keep the terminal cursor attached to the agent's live input row while a
  frozen text selection is scrolled. Once that row leaves the viewport the
  cursor is hidden, instead of remaining fixed on an unrelated selected cell.

## 0.4.41 — 2026-08-30

- Add an upgrade-aware lifecycle for deck's private tmux server. Quitting,
  crashing, and reopening the same build still reuse the existing PID and
  processes; a different release build now detects the old helper before
  attaching indefinitely.
- Store versioned creator/build/helper/protocol/source metadata in the tmux
  server, classify current/different/legacy/corrupt states centrally, and keep
  Stable/Nightly, development, and smoke sockets from accidentally crossing.
- Automatically replace only an empty incompatible server. An occupied old or
  legacy server stays usable until the user reviews the affected session/pane
  counts and explicitly confirms that restarting the background shell service
  ends its commands and agents. “Later” is durable and remains discoverable in
  the sidebar and Settings without repeated prompts.
- Make replacement a serialized, recoverable transaction with a fresh impact
  check, PTY detach, orderly stop, bounded wait, validated stale-socket cleanup,
  current-helper start, metadata read-back, PID/identity verification and
  content-free diagnostics. Cards and bounded shell snapshots remain, but
  running Unix processes are never described as migrated.
- Prevent the updater's relocated old process and release apps running from
  transient/DMG paths from creating a long-lived server. Add an accurate Local
  Network usage description for user-chosen services reached by terminal tools.
- Add isolated real-tmux lifecycle contracts plus manual signed updater,
  responsible-code and Local Network Privacy smoke steps.
- Keep terminal wheel work armed on every display frame while preserving the
  single in-flight tmux mutation. Real bundled WKWebView verification improves
  sustained scroll updates from about 40 Hz to 60 Hz without losing fractional
  trackpad deltas, inertia tails, direction reversals or tmux scroll authority.
- Synchronize frozen terminal selections across either tmux-status/xterm-frame
  ordering, promote drags by terminal-cell movement rather than a CSS-pixel
  threshold, and recheck the pointerup cell. On the first wheel frame, native
  xterm word/line selections are adopted into the same immutable tmux range
  and overlay used by multi-row drags. Even a one-cell horizontal selection
  now follows its text while its coordinates and clipboard bytes stay frozen.
- Let Codex/Claude history recall yield to editing a visible multi-row prompt:
  after recalling a long entry, Up moves into its preceding visual row first;
  ordinary single-line history, modifiers, shells, editors and other TUIs keep
  their existing keys. The check reads only five public xterm cells per row.
- Fix terminal drag selection inside full-screen agent panes (Claude Code,
  Codex), where the highlight and the copied bytes landed on rows the pointer
  never touched while the identical drag in a shell pane was correct.
- Place copy-mode endpoints with `top-line`/`cursor-down`/`cursor-right` only:
  `start-of-line`, `end-of-line` and `back-to-indentation` walk to the ends of
  the wrapped logical line and `cursor-left` lands on a wide grapheme's
  trailing column, so all four leave the visible row. `cursor-down` snaps the
  column to a line end until the walk first steps off a non-empty line, which
  is why a frame that opens with blank rows selected the wrong rows.
- Measure a row's end the way tmux does, so a pointer past a row's trailing
  blanks clamps to the line end instead of wrapping onto the next row, and read
  the frame once per placement instead of once per endpoint.
- Cover the fix with real-tmux contract tests over alternate-screen frames
  (blank top rows, wide characters, wrapped rows, styled blanks, scrolled
  viewports), asserting endpoints and copied bytes; the contract suite now
  drives the production placement instead of its own copy of the rules.
## 0.4.38 — Nightly candidate, 2026-08-29

- Publish a Nightly-only version bump for validating the Stable-to-Nightly
  updater path after `v0.4.37`; application behavior is otherwise unchanged.

## 0.4.37 — 2026-08-29

- Add opt-in Stable/Nightly update channels with Stable-safe settings
  migration, one-time Nightly risk confirmation, fixed backend-owned endpoints,
  Tauri-native signature verification/install, and visible version/channel/
  commit identity.
- Add deterministic numeric version tooling, candidate manifest/hash/provenance
  validation, and fixture coverage for version, tag, asset, signature,
  manifest, Release-state and copy-only promotion failures.
- Add a protected Nightly candidate workflow with the full test/sign/notarize/
  staple/Gatekeeper gate and a last-step rolling feed update that preserves the
  prior verified pointer on ordinary failures.
- Add a production-approved Stable promotion workflow that copies the exact
  candidate DMG/updater/signature, re-verifies every byte and Apple/minisign
  identity, creates the Stable tag at the same commit, and publishes Stable
  `latest.json` last without rebuilding the application.
- Tighten the legacy Stable resolver to strict tags, per-version concurrency,
  draft-until-complete publication and non-destructive incomplete-Release
  handling; promoted candidate commits cannot trigger a duplicate source build.

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
