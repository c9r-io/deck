# deck — development notes

Kanban over tmux sessions. Small by design: tmux owns the shells; deck is only the
board projection plus metadata. Resist adding session management features that tmux
already provides. **No automatic card movement, ever** — the board surfaces
information; the user makes every placement decision.

This file is an index plus the rules that cut across modules. Each subsystem's
full contract lives in the `//!` (Rust) or leading `//` (JS) header of the module
that owns it — read that header before changing the module, and update it in the
same commit as the behaviour it describes.

## Hard rules

- **No automatic card movement.** The board never moves a card; not on agent
  state, not on inbound events, not on delivery.
- **deck is EDR-QUIET by rule** (a corporate EDR flagged it and IT demanded the
  app be stopped; `tests/edr_quiet.rs` enforces each point):
  never touches launchd — no `launchctl` (not even a one-shot `submit`), no
  LaunchAgents/LaunchDaemons, no login items; post-update relaunch is a
  `setsid`-detached waiter (`relaunch.rs`) that waits for the old PID and
  `open -n`s the installed bundle. Never spawns `ps`, `date`, `osascript`
  or a shell: process facts (pid/ppid/RSS/tty/foreground group/argv[0])
  come from libproc + `KERN_PROCARGS2` in `procinfo.rs`, local time from
  `localtime_r`, and a duplicate instance just logs and exits. Never
  writes an executable under `~`: hook commands name the helper INSIDE the
  signed bundle (see agent hooks). Remaining spawns are low-frequency,
  fixed-argument system tools (`open`, `plutil`, `pbcopy`, `defaults`,
  `sw_vers`, `uname`) plus the bundled tmux.
- **Never create a public candidate, Stable tag, feed update or promotion
  without the user's explicit authorization.** Release operation is documented
  in `docs/release-channels.md`. `scripts/release-version` synchronizes the three numeric source/lock entries;
  `scripts/release_channels.py` is the shared manifest/hash/provenance validator;
  `scripts/check-workflows` runs checksum-pinned actionlint. `nightly.yml`
  builds a signed/notarized immutable candidate and updates `nightly-feed` last.
  `promote.yml` is copy-only: its static gate rejects application build
  commands, and Stable `latest.json` is the last completeness asset.
- **Privacy by construction.** The whole `~/.deck` tree is 0700/0600; app.log is
  structured and sanitized (`applog.rs`, `redact.rs`, `diagnostics.rs`); never add a
  free-form frontend log channel (`tests/log_privacy.rs` and the formatter
  contract test in `diagnostics.rs` enforce this). Credentials live only in the
  macOS Keychain (`keychain.rs`).
- **Persistence never guesses.** A load FAILURE is surfaced, never treated as a
  first run; the UI must never auto-save defaults over an existing file; every
  Board mutation goes through the one transaction queue (`persistence.js`);
  future schema versions are refused untouched (`storage.rs`).
- **The signed `deck-app` binary is never a pane executable**, and no
  `/bin/sh -c`, script or shell argv appears on the shell-restore path
  (`commands::restore_start_args`, `tests/edr_quiet.rs`).
- **Never write current tmux server metadata onto an unknown existing server**;
  session creation holds `session_creation_guard`; increment `SERVER_PROTOCOL`
  only for a true compatibility break (`tmux_lifecycle.rs`).
- **Selection never touches xterm internals**: no `._core`, no transparent
  textarea, no `disableStdin` (`ui/js/check.mjs`, `selection.js`).
- **NEVER run the bare binary from a background shell**: outside the GUI login
  session the process can't reach macOS text-input services — window and mouse
  work, keyboard is silently dead. Use `app/run.sh`.

## App (v0.2, primary) — `app/`

Tauri 2 macOS app. Frontend `app/ui/` is a no-build set of native ES modules
loaded by `ui/index.html`; xterm.js vendored in `app/ui/vendor/`. Backend
`app/src-tauri/src/`. The contract for each area is in the named module header.

| Area | Module (contract in its header) |
|---|---|
| Board state, one persist-before-commit transaction queue | `ui/js/persistence.js`, `state.js` (shared slots), `pure.js` (DOM-free logic) |
| Typed documents, envelope, quarantine-first recovery | `storage.rs`, `documents.rs` |
| Private data dir (0700/0600 by construction), atomic writes, pruning | `datadir.rs` |
| One error type: closed `ErrorKind` + message, string on the wire | `error.rs` |
| app.log writer, session tags, log migration | `applog.rs` |
| Log redaction scanner (`sanitize_log`, `redact_credentials`) | `redact.rs` |
| Single-instance flock, launch flags / debug-only smoke args | `instance_lock.rs`, `launch_args.rs` |
| Session start/kill, poll (status, RSS, preview rows), clipboard write | `commands.rs` |
| tmux sidecar, socket, server conf | `tmux.rs` |
| Server lifecycle: protocol metadata, reuse/replace, restart transaction, channel sockets | `tmux_lifecycle.rs` (+ `docs/tmux-server-lifecycle.md`) |
| PTY attach bridge with end-to-end flow control | `pty.rs` |
| Terminal scroll + token-bound selection lease commands | `terminal.rs`, `terminal_selection.rs`, `terminal_scroll.rs` |
| Pointer/selection authority, overlay, wheel routing (frontend) | `ui/js/selection.js`, `layout.js` |
| Completion bar, links, context menus | `ui/js/terminal.js`, `links.rs` |
| Scheduled prompts: queue model, selection, delivery state machine, tick | `scheduler/` (+ `docs/scheduler-context-safety.md`), `context.rs`, `ui/js/scheduler.js` |
| Prompt templates | `ui/js/templates.js` |
| Agent status hooks (closed state words, bundled helper) | `agent_status.rs`, `src-tauri/status-helper/` |
| Auto-respond (inbound sources, dispatcher, Keychain) | `inbound.rs`, `inbound_slack.rs`, `keychain.rs`, `ui/js/inbound.js` (+ `docs/auto-respond.md`) |
| Shell restart recovery (bounded transcript projection) | `shell_state.rs` |
| Updates (closed stable/nightly, one endpoint each) | `updater.rs`, `relaunch.rs` |
| Structured diagnostics, ui_event whitelist, exports | `diagnostics.rs` |
| File drop / image paste | `drops.rs` |
| Process facts without spawning `ps` | `procinfo.rs` |
| Debug/isolated-smoke fault injection | `smoke_faults.rs` |

Status semantics (card colour) are documented on `effectiveCardStatus` in
`pure.js`: agent state outranks the 15s output heuristic.

### Run and gates

- Run: `app/run.sh` — builds, wraps the binary in a minimal .app, launches via
  `open`. `~/.deck/app.log` (0600) collects backend + frontend diagnostics;
  maintainer-only verbose frontend events are enabled with
  `app/run.sh --debug-logging` (no user setting). Production debug:
  `tmux -L deck ls`; source bundles use `tmux -L deck-dev ls`.
- Frontend gates: `node --check` · `scripts/ui-tests` (node:test + coverage
  thresholds; WKWebView-bound modules are excluded and covered by the smoke) ·
  `node ui/js/check.mjs` (unresolved identifiers; forbids xterm `._core`).
  Backend gates: `cargo fmt`, `cargo clippy --workspace -D warnings`,
  `cargo test --workspace` (unit + `tests/tmux_contract.rs` against the
  bundled tmux + `tests/log_privacy.rs` + `tests/edr_quiet.rs` + the status
  helper). `src-tauri/Cargo.toml` is the one workspace root; the compiler
  is pinned by `rust-toolchain.toml` at the repository root and the same
  version in every workflow (`scripts/test_release_tools.py` checks).
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

### WKWebView / Tauri gotchas (each cost a real bug)

- Force Touch can open macOS Look Up on button text despite `user-select:none`.
  `app.js` cancels `webkitmouseforcewillbegin` for buttons and their descendants;
  keep ordinary pointer/click events and editable/terminal text untouched.
  App-surface `contextmenu` cancels WebKit's browser menu without stopping
  propagation to Deck's own card/project context-menu handlers. Real form
  fields retain native editing menus; xterm's hidden textarea does not.
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
  build.rs stages `ui/` into the gitignored `src-tauri/ui-dist/` that
  `frontendDist` points at; release profiles leave `ui/test` out of the
  bundle, debug profiles keep it for the WKWebView smoke. Never edit `ui-dist`.
  The tauri CLI refuses to start cargo when `frontendDist` is missing, so
  `beforeBuildCommand` is a bare `mkdir -p` of it (bit us: the first 0.5.15
  nightly build failed on a clean runner while `app/run.sh`, which calls
  cargo directly, never hit it).
- Tauri v2 requires `src-tauri/capabilities/default.json` granting `core:default`
  or `event.listen` is REFUSED with a silent promise rejection — invoke (JS→Rust)
  works, events (Rust→JS) never arrive, so the terminal receives no output while
  everything else looks healthy. The boot-time deck-ping self-test in ui/index.html
  catches this class of failure; keep it. Always `.catch(ulog)` on listen().

## Invariants

- Sessions are started with a plain shell + `send-keys` of the command, NOT by exec'ing
  the command, so the session survives agent exit and scrollback stays inspectable.
- Use harmless card commands (e.g. `while true; do date; sleep 1; done`) when
  testing — a card whose command is `claude` will really launch Claude Code.
