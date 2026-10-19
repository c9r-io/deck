# deck — development notes

Kanban over tmux sessions. Small by design: tmux owns the shells; deck is only the
board projection plus metadata. Resist adding session management features that tmux
already provides. **No automatic card movement, ever** — the board surfaces
information; the user makes every placement decision.

This file is an index plus the rules that cut across modules. Each subsystem's
full contract lives in the `//!` (Rust) or leading `//` (JS) header of the module
that owns it — read that header before changing the module, and update it in the
same commit as the behaviour it describes.

## Product north star and boundaries

Deck manages human attention around interactive CLI work. Its goal is to reduce
total attention-management cost: unnecessary interruptions from misleading signals,
manual checking from missed or unavailable signals, effort spent understanding
Deck's internal state, and the times a user must come back to re-engage, find the
work and rebuild its context. It does not aim to minimize notifications or maximize
conservatism at any cost. Automation is core when it reduces that cost without
hiding uncertainty or taking ownership of the work, within honest safety boundaries.

**Optimize for reliable progress without repeated human intervention, not merely
the shortest unattended latency.** Within existing authority, safety and explicit
risk-acceptance boundaries, a brief, bounded background wait nobody has to watch
can beat a faster flow that sends the user back to confirm, check or nudge; a few
more machine seconds are not automatically worse than one more human action. When
intent was stated once and the user moved on, reliable later progress usually beats
making them return, locate the work and repeat the step; a genuinely new decision
or risk still needs them. This never excuses unbounded or silent stalls: a wait is
not readiness evidence, real failures and needed decisions still surface promptly
and accurately, and interaction is never reduced by hiding failure, suppressing
signal or bypassing authority.

**Deck owns attention around work. The Agent owns the work.** Deck may
coordinate an Agent session; it does not own Agent execution. An Agent manages
its own work. Deck manages the human's attention across many pieces of work:
Agents, ordinary shells, automations and other long-running interactive work.
Deck does not manage Agents; it brings work requiring human attention into
one coherent surface, so the user need not patrol every session.

Agent-agnostic design is a strategic choice. Agent vendors control model behavior,
training/inference, planning, task context, tools, permissions, background work,
turn semantics and native UI. They can improve when their Agent continues,
asks, interrupts or presents progress, including below the UI layer. Deck should
not try to outsmart or out-manage one Agent by reconstructing those semantics.
The Agent answers “What does this Agent need from the human?”; Deck answers
“Which work deserves the human's attention now?” Deck need not beat Claude Code
or Codex at managing one session. Better autonomy, recovery and signaling should
reduce Deck's work inside each session and increase the value of its durable
cross-work attention layer, which no individual Agent vendor naturally owns.

- Deck owns: session/work identity, session lifecycle coordination, the
  Board projection, observable Agent Signal, attention routing, safe prompt
  delivery, automation trigger/admission/authority, persistence and
  recovery, external integrations.
- The Agent owns: task semantics, planning, reasoning, tool execution, its
  internal context, subagents, worktrees, Git/PR workflow, and whether a
  task is done or succeeded.
- Agent-specific Signal is a narrow, deliberate exception to Agent
  agnosticism: Deck consumes it only to protect its own concerns (attention
  integrity and coverage, input safety, safe degradation), never to understand or steer
  the work. Keep adapters thin: Agent → authoritative/narrow observation →
  Deck adapter → generic Deck observation → cross-work attention routing.
  Consume useful vendor-native facts; do not infer Agent internals to reproduce
  workflow semantics or orchestrate Agent execution. Integration depth alone is
  not the boundary; ownership of meaning and work is.
- Authority stays separate: Signal may inform and HOLD ≠ Signal may
  authorize; external provenance ≠ content authority; authorization ≠ input
  readiness. The mechanics live in the owning module headers and docs
  (`docs/scheduler-context-safety.md`, `docs/auto-respond.md`).

**Prefer authoritative upstream facts over downstream inference.** Trustworthy
Codex client provenance should replace process-ancestry guesses, terminal-title
inference, screen parsing or quiet-time heuristics; an authoritative Claude
input-needed event should replace reconstruction from terminal presentation.
Fallback observations may aid presentation or degraded coverage, with an explicit
trust ceiling. When a better official primitive arrives, prefer replacing the
heuristic over retaining a parallel inference system. Temporary API gaps and useful
prototype observations do not by themselves justify permanent product complexity.

**Signal Integrity ≠ Signal Suppression. Signal may inform.** Trusted Signal
should actively route attention when it offers a useful, honest observation:
`needs-input` is actionable; `turn-done` is an interaction boundary worth surfacing,
not task completion, Agent idleness, permission to close, or authority to send another prompt.

**Attention should surface actionable truth. Authority should require positive proof.**
Uncertain authority or readiness must block automatic side effects: do not type into
an uncertain target, auto-send unverified external content, or release automation
using unattributable Signal. Attention has a different trade-off: neither interrupt
when the user is not needed nor stay silent when silence forces them to babysit the Board.
Attention integrity needs both precision (signals mean what Deck says) and coverage
(observe and surface states the user should reasonably know about). Even technically
correct signals fail the product goal if blind spots teach users to keep polling cards.

Unknown ≠ safe: unknown authority remains fail-closed; unknown attention coverage
must not become an invented Agent state. An honest, low-interruption indication such
as “Agent status unavailable” may help, but is not automatically “needs attention”
or an urgent notification. Expose blind spots without making each one an interruption.
For Codex's shared-daemon case, unattributable Signal means no trusted Signal and no
authority from it: safety is preserved, attention coverage is degraded, not fully solved.
Shared-daemon Signal can become Trusted only when each event is verifiably bound
to a specific interactive client and its current pane foreground generation.
Inherited `TMUX_PANE`, cwd, timing, executable name, transcript path, and
session/thread/turn IDs are not pane ownership proof. This is a provenance rule,
not a version allowlist: 0.157.1 and 0.160.0 remain BLOCKED; `--no-daemon`
embedded attribution topology can satisfy the existing model. See
`docs/codex-signal-certification-20261002.md`; current `CodexSignalTrust::Unavailable`
and scheduler/readiness gates remain unchanged.
Prefer a future official client-provenance primitive to a permanent substitute
protocol; expose the gap honestly while authoritative attribution is unavailable.

As a product design test, aspire to let users think: “I can start several pieces
of work and go do something else. Each Agent can manage itself. When work needs
me, Deck brings the right work back to my attention; otherwise it stays out of the way.”
This evaluates the experience, not a guaranteed technical SLA.

Before a substantial feature, ask:

1. What human attention cost does it remove or protect?
2. Does it reduce unnecessary interruption, manual checking, or both? For a latency
   change: does it save machine time or human attention? Never trade verified
   reliability for a shorter clock, or hand checking and recovery back to the user.
3. What new authority would Deck acquire?
4. Does it require Deck to understand Agent task semantics or own how work is performed?
5. Can the same value be reached while staying Agent-agnostic?
6. Does it improve trustworthy attention coverage, or merely add state/mechanisms?
7. Must users understand Deck's internal safety model during normal use?
8. Does this coordinate attention across work or improve one Agent's internal behavior?
9. Does the Agent/vendor own this semantic more authoritatively; can Deck consume its fact?
10. If the upstream API improves, can this adapter disappear cleanly?
11. Does this improve Deck across Agents or deepen dependence on one Agent's internals?

These are scope heuristics: Agent-specific work can serve cross-work attention well.

Good features should generally make normal use require less thought. Complexity can
be justified where errors harm Deck's unique responsibility: cross-work identity,
attention routing, authority boundaries, input safety, persistence, side-effect
fencing and honest degradation. This is product positioning, not just engineering
preference. Stay simple outside its ownership: task meaning, Agent progress semantics,
planning, tool strategy, worktrees, Git/PR semantics, completion judgment and internal workflow.
**Deck can be a strict small system; it should not become a smart large system.**
Count persistent mental-model concepts, not just lines of code. Prefer reusing
Observation, Authority, Readiness and Lifecycle; before adding a security/state
primitive, ask whether it is independent or the existing model is being misused.
This is a complexity-budget heuristic, not an absolute architectural prohibition.

A feature that mainly expands Deck's ownership of how work is performed is
scope expansion needing exceptional, explicit justification — not the
product's default direction. Headless Agent transports, Agent pools, Agent
task orchestration, worktree ownership, Git/PR orchestration and
task-success judgment should all trigger this scrutiny. This is not a ban on
evolution; it makes scope expansion deliberate.

## Hard rules

- **No automatic card movement.** The board never moves a card; not on agent
  state, not on inbound events, not on delivery. There are exactly TWO
  sanctioned automatic retirements, both through the ordinary durable
  `provider.close` path. (1) A verified normal exit of an ordinary card's
  owning shell (Ctrl+D, `exit N`): tmux's own `pane-died` hook recorded an
  exit status and no signal for the last pane of the exact session identity
  Deck observed alive (`shell_exit.rs`, poll `exited_normally`, `board.js`
  exit retirement); never for an MCP-origin card or a retained buffer.
  **Absence is not deletion authority**: `pty-exit`, tmux's `[exited]`,
  a session missing from a listing, `kill-session`, server loss or
  replacement, and a signal death grant no retirement — the card stays
  stopped. (2) An automation run whose rule says "close the
  card" (`runFinishHolds` in `pure.js`, driven by the `board.js` poll):
  the user chose it per rule, it never fires while a pane shows the card,
  it goes through the same close path as a click, and it reads only
  process-level evidence: no agent state and a shell back in the pane's
  foreground (the agent program exited). **An agent hook word is never
  side-effect authority**: `turn-done` is an interaction boundary, not the
  end of the agent's work (Claude Code resumes on its own after background
  work; a Codex interrupt leaves background terminals running), so it
  neither retires a run nor releases an external follow-up row
  (`scheduler/select.rs` agent hold). `tests/signal_census.rs` pins every
  consumer of the words as presentation, attention or side effect. Other non-click card
  creation/retirement paths are closed: an explicit MCP request (`mcp.js`,
  `mcp/`) names one target; create needs the locally authorized client's
  create permission, close needs its current control generation/epoch/holder
  and is refused under human lock or while any pane shows the card; both are
  idempotent by request id and go through `provider.createStarted` /
  `provider.close`, and a Deck restart never replays either (they become
  ambiguous). A paired Connector `task-create` (`connector.js`) may create
  one card from a locally saved project preset, and the first message matched
  by a saved Slack channel rule (`inbound.js`) may create that rule's
  collection card; both use `provider.createStarted`, and neither moves or
  retires a card. An item a delegated MCP client hands to an Agent rule
  (`mcp/automation.rs` → `inbound_agent.rs` → the ordinary dispatcher)
  creates that rule's card exactly as a badge does. Nothing else retires or
  relocates a card on its own.
- **Deck may coordinate an Agent session ≠ Deck owns Agent execution.**
  Deck owns attention around work; the Agent owns the work. No headless
  agent transport, `-p`/`exec`/SDK orchestration, agent pools, worktrees or
  permission auto-approval. Three row facts stay separate
  (`scheduler/authority.rs`): external **provenance** (`external`, never
  cleared) ≠ **content authority** (`authority`: the user's explicit,
  revision-bound approval of a Slack badge automation's, an Agent
  automation's or a phone task preset's exact steps, checked by the backend
  at admission and revoked by the tick; being paired is never prompt
  authority, text the phone supplies is never approved, and an Agent
  rule's approval also rests on the user's standing delegation to the MCP
  client that handed the item over — withdrawn under the settings fence,
  `mcp/automation.rs`) ≠ **input readiness**
  (every scheduler hold, including the first-interaction gate). An approval
  releases only the external-follow-up hold; the fresh-start pass never types
  into an interactive agent. A separately enabled, native-verified Slack badge, clock, Agent or phone task
  preset first-send override may send the head row on a later pass after bounded compatibility
  stabilization and current fences; it is not Agent readiness evidence.
  A channel rule may separately grant fully revocable permission for future
  scoped events to send a new run's first step. A native-proven channel head
  retains its authorization constraint after withdrawal or text edits;
  existing interaction evidence never bypasses that constraint. This does
  not expand later-step or tool authority, and `TriggerClass` stays the
  closed list of three (Slack badge, phone task preset, delegated Agent
  client), each tied to its own revocation source; a fourth is a hard-rule
  change. No Signal word, quiet
  time or activity ever creates, restores or upgrades authority
  (`tests/signal_census.rs`).
- **deck is EDR-QUIET by rule** (a corporate EDR flagged it and IT demanded the
  app be stopped). The whole process-surface contract — every allowed spawn
  and its exact file, the launchd/login-item ban, the bundled-tmux-only
  rule, the shell-free restore path, the disabled-by-default Connector
  listener on a private IPv4 address, the smoke-server cleanup, and the one
  outside-bundle exception (the signed Deck Tunnel Helper, whose hash-pinned
  `tunnel-client` is OUTSIDE the EDR-quiet promise) — lives in the header
  of `tests/edr_quiet.rs`, which enforces each point over production
  source; `scripts/check-edr-binary` scans the packaged app and the release
  helper. Read that header before adding any `Command::new`, tmux verb,
  listener, shell path or file under `~`. This is not an OS sandbox and
  carries no promise of EDR invisibility.
- **Never create a public candidate, Stable tag, feed update or promotion
  without the user's explicit authorization.** Release operation is documented
  in `docs/release-channels.md`. `scripts/release-version` synchronizes the five application/Adapter source and lock entries;
  `scripts/release_channels.py` is the shared manifest/hash/provenance validator;
  `scripts/check-workflows` runs checksum-pinned actionlint. `gate.yml` is the
  ONE test gate (`test.yml` runs it; `nightly.yml` and `release.yml` call it on
  the resolved commit before any build). `nightly.yml`
  builds a signed/notarized immutable candidate and updates `nightly-feed` last.
  `promote.yml` is copy-only: its static gate rejects application build
  commands, and Stable `latest.json` is the last completeness asset.
  `release.yml` is the emergency source build for a strict Stable tag no
  candidate covers (tag push or manual dispatch, never a schedule).
- **Privacy by construction.** The whole `~/.deck` tree is 0700/0600; app.log is
  structured and sanitized (`applog.rs`, `redact.rs`, `diagnostics.rs`); never add a
  free-form frontend log channel (`tests/log_privacy.rs` and the formatter
  contract test in `diagnostics.rs` enforce this). Credentials live only in the
  macOS Keychain (`keychain.rs`).
- **Persistence never guesses.** A load FAILURE is surfaced, never treated as a
  first run; the UI must never auto-save defaults over an existing file; every
  Board mutation goes through the one transaction queue (`persistence.js`);
  future schema versions are refused untouched (`storage.rs`). Inspection opt-in uses
  sticky v2 envelopes on queue.json and settings.json plus a settings barrier.
  A retained card buffer, channel/Connector run journal, frozen inbound plan, or project task preset upgrades deck.json to sticky v3, idempotent buffer
  queue operations upgrade queue.json to sticky v3, and enabled channel
  connections/rules upgrade settings.json to sticky v3; ordinary data stays v1. Clock first-send settings/queue origins upgrade to sticky
  v4 so older readers refuse them untouched. Card Reminder intent and blocked
  retirement identities upgrade deck.json to sticky v5. A phone task preset's
  first-send choice or approval (deck.json) and the queue origins they admit
  (queue.json) upgrade to sticky v6. Channel first-step permission,
  frozen targets/claims, queue constraints,
  snapshots and audit semantics require sticky v7. A clock schedule that
  repeats every N > 1 units from an anchor date upgrades settings.json to
  sticky v8. An Agent trigger rule (settings.json) and the queue origins it
  admits (queue.json) upgrade to sticky v9, and an MCP ledger holding an
  automation delegation or a trigger record is written at its own sticky
  v7. Channel inbox v2 protects
  native staged grant references; v1 entries gain no new permission. A
  settings backup is handed to the webview without channel grants. Native
  template edits retire affected grants before the Board write, under
  settings fence → Board fence → queue lock → storage save lock. A rule's
  choices are backed by settings
  and a preset's by the Board; for both a backup is recovery material, never
  authority: a Board answered from deck.json.bak, or taken as the way out of
  a lost Board, is handed to the webview without its presets' choices, so no
  later save can make a withdrawn one current; it admits nothing and the
  steps still waiting lose theirs. Native notifications
  project committed Board intent; their responses return through the same writer.
  Reminders grant no input/start authority and protect pending/due cards from
  non-cancelling deletion, including deferred retirement after ending Reminder.
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

| Area | Modules |
|---|---|
| Board state, the one transaction queue | `ui/js/persistence.js`, `state.js`, `pure.js` |
| Board card presentation and gesture routing | `ui/js/board-card.js`; action owners remain in `board.js`, `layout.js`, `terminal.js` |
| Typed documents, envelopes, recovery | `storage.rs`, `documents.rs` |
| Private data dir, atomic writes; bounded durable state | `datadir.rs`, `ledger.rs` |
| Error type | `error.rs` |
| app.log, redaction, diagnostics | `applog.rs`, `redact.rs`, `diagnostics.rs` |
| Single instance, launch flags | `instance_lock.rs`, `launch_args.rs` |
| Session start/kill, poll | `commands.rs` |
| tmux sidecar and long-lived clients | `tmux.rs`, `tmux_clients.rs` |
| tmux server lifecycle, restart, verified shell exit, restart recovery | `tmux_lifecycle.rs`, `restart.rs`, `session_runtime.rs`, `shell_exit.rs`, `shell_state.rs` (+ `docs/tmux-server-lifecycle.md`) |
| PTY attach bridge | `pty.rs` |
| MCP terminal control and automation delegation | `mcp/` (contract in `mcp/mod.rs`; delegation in `mcp/automation.rs`), `mcp-adapter/`, `mcp-runner/`, `mcp-fixtures/`, `ui/js/mcp.js` (+ `docs/mcp.md`, `docs/mcp-architecture.md`) |
| Controlled Execution calibration (CE1) | `scripts/ce/`, `scripts/ce_parity.py`, `scripts/ce_verdict.py`, `mcp/tests.rs` `ce1_probe_*` (+ `docs/ce-calibration.md`) |
| Voice input, shared literal paste | `native/SpeechBridge.swift`, `voice.rs`, `prompt_delivery.rs`, `ui/js/voice*.js` (+ `docs/voice-input.md`) |
| Local translation, pasteboard observation, Lens | `native/BergamotBridge.cpp`, `native/PasteboardBridge.swift`, `vendor/bergamot/`, `intelligence/`, `ui/js/local-intelligence.js`, `translation-lens*.js` (+ `docs/translation-lens.md`) |
| Terminal scroll, selection, links, clipboard | `terminal.rs`, `terminal_selection.rs`, `terminal_scroll.rs`, `links.rs`, `ui/js/selection.js`, `layout.js`, `terminal-links*.js`, `terminal-clipboard.js`, `terminal-bytes.js`, `terminal-ime-probe.js` (always-on content-free IME commit probe) |
| Completion bar, context menus, the ONE new-session path | `ui/js/terminal.js` (`newSession`), `board.js` (`provider.createStarted`), `pure.js` (`newSessionPlan`) |
| Settings (the ONE settings writer), dialogs, dropdowns | `ui/js/settings.js`, `settings-search-model.js`, `dialogs.js`, `dropdown.js` |
| Lists (the ⏱ panel), delivery state machine | `scheduler/`, `context.rs`, `ui/js/scheduler.js`, `queue-review.js` (+ `docs/scheduler-context-safety.md`) |
| Automation authority and first-send overrides | `scheduler/authority.rs`, `first_send.rs`, `channel_first_send.rs`, `inbound_channel.rs`, `ui/js/automation-model.js`, `connector-model.js`, `channel-model.js` (+ `docs/channel-monitor.md`) |
| Automations: inbound sources, drawer, templates, Keychain | `inbound.rs`, `inbound_slack.rs`, `inbound_clock.rs`, `inbound_agent.rs` (the Agent handover inbox), `keychain.rs`, `ui/js/inbound.js`, `automation.js`, `templates.js` (+ `docs/auto-respond.md`) |
| External admission policy | `admission.rs` (pinned by `tests/external_admission.rs`) |
| Needs-attention view, away notifications, Dock badge | `ui/js/attention.js`, `attention-model.js`, `notify.rs`, `native/NotificationBridge.swift`, `ui/js/notify-model.js` (+ `docs/notifications.md`) |
| Card Reminders | `reminder.rs`, `ui/js/reminder-model.js` (+ `docs/card-reminders.md`) |
| Agent status, attribution and listener | `agent_status.rs`, `agent_origin.rs`, `src-tauri/status-helper/` |
| Agent hooks configuration, installation and migration | `agent_status/hooks.rs` (contract), `agent_status/hooks/tests.rs` |
| Pi agent adapter: registration, the one settings entry, observer transport | `agent_pi.rs` (contract), `agent_pi/config.rs`, `agent_pi/transport.rs` (the sender proof, wire v2), `resources/pi/` (the observer extension), `scripts/pi_native.py`, `scripts/pi-live-verify.py` |
| Agent re-entry on a shell-service restart | `agent_reentry/` (contract in `mod.rs`; adapters `claude.rs`, `codex.rs`), `src-tauri/lifecycle-helper/`, `procinfo.rs`, `procinfo/code_identity.rs`, `resume.rs` (`printed`, the one pane-text reader), `ui/js/reentry-model.js`, `restart-reentry.js` |
| First-party extension composition, access startup and Settings controllers | `extensions.rs`, `app/extensions/*.json`, `scripts/extension_catalog.py`, `ui/js/settings-extensions.js`, `ui/js/extension-*.js` (Agent, Slack, translation, Connector, MCP/Tunnel, voice preferences; + `docs/first-party-extensions.md`) |
| Updates | `updater.rs`, `relaunch.rs` |
| File drop / image paste | `drops.rs` |
| Smoke fault injection, native driver (debug profiles only) | `smoke_faults.rs`, `smoke_native.rs`, `native/SmokeBridge.swift` |

Card colour: `effectiveCardStatus` in `pure.js` (agent state outranks the 15s
output heuristic).

### Run and gates

- Run: `app/run.sh` (`--debug-logging` for verbose frontend events). Log:
  `~/.deck/app.log`. tmux: `tmux -L deck ls` (production), `-L deck-dev`
  (source bundles).
- Extension declarations: `python3 scripts/extension_catalog.py --check` (Cargo refreshes the checked-in catalogs before compilation).
- Frontend gates: `python3 scripts/check-js-syntax.py` · `scripts/ui-tests` · `node ui/js/check.mjs`.
- Backend gates: `cargo fmt`, `cargo clippy --workspace --all-targets
  --all-features -D warnings` (the CI form), `cargo test --workspace`.
  `src-tauri/Cargo.toml` is the one workspace root; the compiler is pinned by
  `rust-toolchain.toml`.
- Tests leave nothing behind: a Rust unit test takes its files from
  `test_dir::TestDir` (removed when it drops), a Python test registers
  `addCleanup`. The gate runs both suites in an empty `TMPDIR` and
  `scripts/check-test-tmpdir` fails on any leftover.
- Mirrored frontend/backend constants are one list,
  `ui/test/fixtures/limits.json`.
- The vendored xterm carries exactly ONE pinned upstream change
  (`docs/vendored-xterm.md`); any further vendor edit needs separate
  authorization and its own pin.
- WKWebView smoke (debug bundles only): fresh absolute `DECK_SMOKE_DATA_DIR`,
  unique `DECK_SMOKE_TMUX_SOCKET`, `DECK_SMOKE_WKWEBVIEW=1 app/run.sh`; judge
  with `scripts/smoke-verdict <data-dir> <mode>` (exit 0 = green). Never point
  it at `~/.deck`.

Recent validation scope, inherited native evidence and remaining gaps are indexed
in `docs/verification-status.md`; a green gate does not close a native or
release-install scenario it did not execute.

### WKWebView / Tauri gotchas (each cost a real bug)

Chromium testing of the mock catches none of these; verify in the real app.

- `body { user-select: none }` makes WebKit refuse keyboard input in inheriting
  fields, xterm's hidden textarea included (terminal renders, typing does
  nothing). Keep the `input, textarea { -webkit-user-select: text }` override.
- `"dragDropEnabled": false` in `tauri.conf.json`, or the native file-drop
  handler swallows HTML5 drag events (cards can't be dragged).
- A menu-less macOS app has no Edit actions: ⌘C/⌘V live in xterm's
  `attachCustomKeyEventHandler`.
- The frontend is embedded at compile time and cargo does not track it: keep
  the whole-directory `cargo:rerun-if-changed=../ui` in `build.rs`, or UI edits
  silently ship the previous UI. `build.rs` stages `ui/` into the gitignored
  `src-tauri/ui-dist/`; never edit it. `beforeBuildCommand` must `mkdir -p` it
  or the tauri CLI fails on a clean runner.
- `src-tauri/binaries/` and `src-tauri/ui-dist/` are ONE location each for
  every build context of a checkout (debug, release, `cargo llvm-cov`), and
  each context produces its own sidecars and frontend. `build.rs` writes both
  only through `build/shared_output.rs` (contract in its header: a plain copy
  keeps the source's modification time and cargo then never sees the change,
  which packaged coverage-instrumented helpers and debug frontends). Every
  path that packages checks what it packages with
  `scripts/sidecar_provenance.py`; a new packaging path must too.
- `src-tauri/capabilities/default.json` must grant `core:default`, or
  `event.listen` is silently refused: invoke works, events never arrive, the
  terminal gets no output. Keep the boot-time deck-ping self-test; always
  `.catch(ulog)` on `listen()`.

## Invariants

- **Minimum window 1280×800 points** (`tauri.conf.json`, pinned by
  `ui/test/static.test.mjs`). Smaller sizes are unsupported: no layout work,
  no smoke.
- Ordinary sessions start a plain shell and `send-keys` the command, never
  exec it, so the session survives agent exit. Exception: an MCP-managed
  session's pane executable is the signed `deck-mcp-runner`, never `deck-app`.
- **A launch command is sent once** (card flag `launched`; `pure.js`
  `startCommand`). Reopening a stopped card starts the shell only. The
  scheduler's delivery start is the one path that restarts a dead session
  with its command. A manual card's command comes only from `project.cmd`;
  "New shell only" and every context entry create shells.
- **Agent re-entry is the one other program start, and never reads the card's
  command.** On a confirmed shell-service restart with ticked cards it types
  one request per entry: the frozen executable with its adapter's exact-id
  resume words (Claude `--resume <native session>`; Codex `resume
  --no-daemon <native session>`), plus the bypass override only under that
  restart's Policy B, or the words that ask Codex for the ordinary profile
  under Policy A on a bypass launch. One attempt, never retried; a client is
  admitted by verified code identity, never a release list; a confirmed or
  started entry is not readiness, authority or task progress. Codex's final
  session is its receipts crossed with the resume hint its pane printed at
  the exit; pane text alone never names a session, and a Codex entry is at
  best `started` (no entry receipt before its first turn). Contract:
  `agent_reentry/mod.rs`; evidence and limits:
  `docs/evidence/closeout-20261011/reentry-acceptance.md`,
  `docs/evidence/codex-reentry-20261018/README.md`.
- **A card is persisted only after its session started**
  (`provider.createStarted` kills the session if the write fails; a missing
  directory asks the user, never leaves a stopped card).
- **MCP shell authority is separate from Phone Connector**: own state, grants
  and epochs; never a Phone token. See `mcp/mod.rs`, `docs/mcp-architecture.md`.
- Test with harmless card commands (`while true; do date; sleep 1; done`): a
  card whose command is `claude` really launches Claude Code.
