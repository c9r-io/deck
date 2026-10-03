# deck release smoke checklist

Every isolated run must be retired after its evidence is captured. From the
repository root, first audit with `scripts/edr_runtime.py`; then explicitly run
`scripts/edr_runtime.py --cleanup --socket deck-smoke-UNIQUE`. Cleanup validates
the server's `source=smoke` metadata and never targets production `-L deck`. It
refuses a non-shell foreground process unless the operator deliberately adds
`--include-foreground`. A final read-only audit must print nothing and exit 0.

## Opt-in real Claude first-send compatibility matrix

This local integration test is ignored by ordinary CI and requires an
authenticated, disposable Claude Code configuration. It exercises the real
scheduler start pass, immediate follow-up selection, worker stabilization,
ledger, and `prompt_delivery::deliver_with` on a unique detached bundled-tmux
socket. Only the tmux transport and settings persistence are redirected to
test-owned resources; it never reads `~/.deck` or the normal tmux server.

1. Create a private `/tmp/deck-firstsend-<unique>` directory with `claude`,
   `work`, and `evidence` subdirectories. Authenticate Claude with
   `CLAUDE_CONFIG_DIR=<root>/claude` from `<root>/work`; complete login and
   trust for this harmless test directory through Claude's normal flow.
   Do not copy an existing `~/.claude` tree. The test installs one
   `UserPromptSubmit` observer into this private config. It records only a
   trial ID and timestamp, never hook payload or prompt text.
2. Run baseline behavior first, then the candidate, from this repository:

   ```sh
   DECK_FIRST_SEND_REAL_ROOT=<root> DECK_FIRST_SEND_REAL_MODE=baseline \
     cargo test --manifest-path app/src-tauri/Cargo.toml --bin deck-app \
     opt_in_real_claude_first_send_matrix -- --ignored --nocapture
   DECK_FIRST_SEND_REAL_ROOT=<root> DECK_FIRST_SEND_REAL_MODE=candidate \
     cargo test --manifest-path app/src-tauri/Cargo.toml --bin deck-app \
     opt_in_real_claude_first_send_matrix -- --ignored --nocapture
   ```

   Baseline mode deliberately omits the new wait while retaining the same
   production selection and delivery path. Candidate mode defaults to 20
   fresh detached launches at 0, 150, 400, 900 and 1500 ms after process
   binding. Each trial requires a real Claude `UserPromptSubmit` and a
   unique assistant response in the isolated transcript. Results are
   appended to `<root>/evidence/{baseline,candidate}-matrix.jsonl`; a failed
   candidate assertion preserves that trial's bounded evidence.
3. Record the Claude version and exact candidate diff/build identity.
   Report the actual number and conditions tested. A successful finite
   matrix does not prove readiness for future versions or startup dialogs.
   Stop only the test-owned tmux server and Agent, then move the disposable
   root to a uniquely named Trash location after retaining bounded evidence.

## Manual physical selection evidence

The `selection-events` mode keeps an isolated terminal open for actual mouse
input. Run with a fresh absolute data directory, a unique `deck-smoke-*`
socket and `--debug-logging`:

```sh
DECK_SMOKE_DATA_DIR="$(mktemp -d /tmp/deck-selection-events.XXXXXX)" \
DECK_SMOKE_TMUX_SOCKET=deck-smoke-selection-events-UNIQUE \
DECK_SMOKE_WKWEBVIEW=selection-events app/run.sh --debug-logging
```

In that terminal, perform a plain click, a short horizontal drag, a drag
across at least three rows, a drag followed immediately by Cmd+C, and a
double click followed by Cmd+C. `scripts/smoke-verdict <data-dir>
selection-events` verifies the isolated carrier is ready;
`node scripts/selection-events-verdict.mjs <data-dir>` verifies the five
physical gesture and copy categories using only closed event names, bounded
relative cell deltas and numeric IDs. This is manual input evidence, not an
automated WK regression. Stop only the isolated app and clean its socket as
described above after capturing the verdict.

Resume completion has an isolated scenario:
`DECK_SMOKE_DATA_DIR="$(mktemp -d /tmp/deck-resume.XXXXXX)" DECK_SMOKE_TMUX_SOCKET=deck-smoke-resume-unique DECK_SMOKE_WKWEBVIEW=resume app/run.sh`.
It checks wrapped exit hints through real tmux/IPC, candidate priority, ghost
acceptance without execution, recovery across an isolated service restart,
pane isolation, agent suppression and clearing.
Before narrowing each pane to 32 columns the smoke types a short prompt
(`PS1='$ '; PROMPT='$ '; RPROMPT=''`) into it: with a prompt wider than the
pane, zsh's redraw when the pane is widened again erases the last hint line
(a known limitation recorded in `resume.rs`), which made the run depend on
the operator's own prompt. The shell and its rc files are still the user's.

Voice input integration has a separate isolated scenario:
`DECK_SMOKE_DATA_DIR=/tmp/deck-voice-unique DECK_SMOKE_TMUX_SOCKET=deck-smoke-voice-unique DECK_SMOKE_WKWEBVIEW=voice app/run.sh`.
It checks the header button, the byte-literal typing path via real IPC (no
Enter, separators kept, any foreground program, dead targets), themes and
saved language preferences without opening the microphone. See
[voice input](../docs/voice-input.md) for native audio and permission checks.

**Judging an automated mode.** When a mode has finished, run
`scripts/smoke-verdict <DECK_SMOKE_DATA_DIR> <mode>` from the repository
root; it must exit 0. It compares that root's `app.log` with the mode's
entry in `app/ui/test/fixtures/smoke-manifest.json` and lists every
missing, negative or unexpected checkpoint. The modes are `run` (the
default; `DECK_SMOKE_WKWEBVIEW=1`), `settings`, `attention`, `resume`,
`review` → `review-restart`, `voice`, `buffer`, `channel`, `connector`,
`channel-fault` (real Slack sandbox tokens), `connector-transport` (stays
open for the Swift transport test; see `connector/ios/README.md`), and two
that relaunch a finished `run` root: `ambiguous` (relaunch with
`app/run.sh --smoke-fault queue-save` so the boot repair write fails) and
`restart`. A relaunch appends to the same `app.log`; judge each mode before
relaunching the next. `authority-live` only prepares the live-agent sequence
below; its verdict covers that setup, not the agents. `empty-start` begins on
a never-used isolated socket (no server, no session — never add a helper
session): a clock-style automation card with one owner `:` no-op must be
started by the scheduler and delivered (`scripts/smoke-verdict <root>
empty-start`). It is the regression for the zero-session deadlock, where a
failed `list-panes -a` on an empty server selected nothing forever.
`board-lost` runs on a root seeded, before launch, so that the Board cannot
be loaded, and follows the one way out deck offers for it. A damaged
`deck.json` with no backup (`printf '{damaged' > "$ROOT/deck.json"`) is a new
Board: every checkpoint reports a=2. A healthy `deck.json` copied in as
`deck.corrupt-1700000000`, with no `deck.json` and no `.bak`, is a restore:
a=1, and `board-lost-exit` b is the number of cards restored. The carrier
cancels the dialog, makes a change that is refused, takes the exit when the
dialog comes back, and requires the chosen Board on disk and usable
(`scripts/smoke-verdict <root> board-lost`); the copy that was set aside
must still be there afterwards. The restore root may also carry a clock rule
that is due at launch, to check that inbound triggers wait for the user's
Board: a `settings.json` whose `inbound.rules` holds one enabled `clock`
rule (`schedule` `{"unit":"day","minute":0}`, `graceMin` 1440, `since` 0, an
empty `cmd`, a `name`) pointing at a project, a group and a one-row template
(`:`) of the kept Board. While the exit is on offer its slot must stay
pending in the backend, with no run recorded and no notice
(`board-lost-offer` then reports b = 10 + the number of buttons), and after
the exit its run must be on the restored Board, queued and acknowledged
(`board-lost-exit` b counts that card too).

**Window size.** Every mode runs in the default window, which is the minimum
supported window size: 1280×800 points (`tauri.conf.json` `minWidth` /
`minHeight`, enforced only by the window manager). Sizes below 1280×800 are
outside the supported product envelope; no mode certifies them. `buffer-narrow`
reads the minimum AppKit holds Deck's own window to on a user resize and the
window's content size (`window-min-clamp`: a = enforced minimum width, which
must be 1280; b = enforced minimum height × 10000 + content height); its
narrow workspace comes from 160% text, not from a smaller window. A
programmatic resize is not bound by the minimum, so no smoke shrinks the
window.


Everything below is a **live** checklist of WKWebView/xterm integration
behaviours that cannot be tested headless: Chromium-based harnesses pass while
the real webview fails (that is how every regression in this list originally
shipped). Run through it in the `app/run.sh` build before tagging a release.
Dated run logs and release evidence do not live here; they belong in the
release's GitHub run and in git history, not in the checklist.

`cargo test` covers the tmux contracts (scroll model, clear-history, literal
injection, poll formats); `scripts/ui-tests` covers the DOM-free modules.

Current state: as of 2026-09-25 (FR-3 A, the scheduler state types) 13 of
the 14 smoke modes — run, ambiguous, restart, settings, attention, voice,
buffer, channel, connector, resume, connector-transport, review and
review-restart — pass `scripts/smoke-verdict` on fresh isolated roots on an
idle Mac. The 14th, `channel-fault`, is a manual item: it needs real Slack
sandbox tokens pasted into the isolated window, so no automated run covers
it; run and judge it by hand before a release that touches the channel
transport. Run the smoke with no other build in progress: under a concurrent
`cargo test` build the timing-bound checks (`link-activate`, `completion*`,
`selection-native-scroll`, `selection-resize`, `scroll-frame`) fail. The five
`selection-*-range/expect/copy/scroll` lines are closed diagnostics and
always report positive. Red or green is decided by `scripts/smoke-verdict`,
not by reading the log.

The real WKWebView smoke needs an unlocked Mac with the smoke window's page
visible and the desktop left alone while it runs. A hidden page (locked
screen, covered window, another Space, or any cause) has its timers
throttled and the product rightly cancels a held press or drag; such runs
used to fail `link-repaint` and the timing checks above with no stated cause.
`page-visible` now reports any hidden time as an environment red. The
link-repaint and selection stages hold trusted OS input and window focus
changes, which `input-isolation` counts; `input-isolation-proof` injects real
AppKit input last, bringing the window to the front for a moment, then hides
the app.

## Human inspection checkpoints (05 C v01)

Run `DECK_SMOKE_DATA_DIR="$(mktemp -d /tmp/deck-review.XXXXXX)" DECK_SMOKE_TMUX_SOCKET=deck-smoke-review-UNIQUE DECK_SMOKE_WKWEBVIEW=review app/run.sh`.
This mode uses the real queue, storage, WKWebView and a fresh empty-command
shell. It sends only three `:` no-ops labelled 修改代码 → 运行测试 → 整理结果,
observing the real minimum 60-second gap (allow about two minutes). It checks
default opt-out, atomic enqueue, no manual-send bypass, failed-save rollback,
replayed confirmations, edit revocation, synthetic hook/quiet/permission states,
confirmation cancellation, locale/theme/font layout, second delivery and the
last row still held. The debug strip labels synthetic observations explicitly;
no hook or agent is installed. Run `scripts/smoke-verdict <root> review`;
exit 0.

Quit only this smoke instance, then relaunch with the **same isolated root and
socket** and `DECK_SMOKE_WKWEBVIEW=review-restart`, then run
`scripts/smoke-verdict <root> review-restart`; exit 0: the last checkpoint and separate delivery/inspection ledgers
survive. Inspect and capture the plan, independent observations, records and
last-row confirmation. Do not confirm it until persistence evidence is captured.
The full release checklist and its known selection baseline remain separate.

## Automation authority: live agents

Run `DECK_SMOKE_DATA_DIR="$(mktemp -d /tmp/deck-authz.XXXXXX)" DECK_SMOKE_TMUX_SOCKET=deck-smoke-authz-UNIQUE DECK_SMOKE_WKWEBVIEW=authority-live app/run.sh`
with the Agent Status hooks installed and Claude Code and Codex trusting
`/tmp`. It approves three Slack badge rules (`claude`, `codex`,
`codex --no-daemon`) through the settings writer and queues each run's frozen
three-step FIXED plan through the real `queueInboundPlan` → `channel_queue_add`
→ native claim check (`scripts/smoke-verdict <root> authority-live`: all nine
rows `external` with fixed authority). No Slack event exists, so a bounded
step is not exercised live. Then, as the user, in the panes:

1. Deck starts each agent from the empty isolated server and types nothing
   (`[queue] started … waits`, no `sent to`).
2. Interact once with an agent yourself (type a prompt into its EMPTY input
   box; never press Enter on a dialog). After its real hook, approved steps
   continue on their own (`sent to … approved fixed step`); step 2 asks the
   agent to run a command.
3. Where the agent requests permission (`needs-input`), no approved step is
   sent for as long as it waits; decline with Esc, and the next real hook
   word releases the run.
4. A Codex client sharing a managed daemon started elsewhere gets no trusted
   Signal: its approved rows stay pending (0 attempts) however long you wait.

Judge the bootstrap windows with `scripts/signal-candidate`
(`claude-bootstrap-waits` / `codex-bootstrap-waits`). Record CLI versions
before and after; quit only this instance and stop its server.

## Signal integrity: automation finish (FR-SI-01)

Run `DECK_SMOKE_DATA_DIR="$(mktemp -d /tmp/deck-signal.XXXXXX)" DECK_SMOKE_TMUX_SOCKET=deck-smoke-signal-UNIQUE DECK_SMOKE_WKWEBVIEW=signal-finish app/run.sh`.
`run.sh` also builds the debug example `signal_fixture` (never bundled), a
fake agent that refuses to run outside this data dir and `deck-smoke*`
socket. A clock rule with "close the card" launches it in a real pane; it
reports through the bundle's real `deck-status-helper`: prompt delivered →
`working` → a background command writes `STARTED` → `turn-done` while that
command runs. The run must stay on the Board for ten polls (every one reading
`turn-done`, queue drained) until the command writes `COMPLETED`; the fixture
then resumes (`working`, `turn-done`), exits, and only then does the existing
finish path close the card and the run (outcome `closed`). Both fixture
processes and the session must be gone. Every fixture hook pipes a Claude
Code-shaped payload into the real helper (FR-SI-04): the isolated app.log
must show four `[agent-status] … v=2` lines (two interactions) and none of
the fixture's decoy text or fake id. The carrier forces polls
(`pollNow`), so a regression that trusts `turn-done` fails within about a
second: that is deterministic regression timing, not production timing
(the reported real-agent case closed ~7 s after the first `turn-done`);
real-agent timing and semantics belong to the candidate step below. About a
minute. Run
`scripts/smoke-verdict <root> signal-finish`; exit 0. Then
`scripts/edr_runtime.py --cleanup --socket deck-smoke-signal-UNIQUE`.

Candidate step with a real agent (release-location install, Claude Code
status integration on — dev/smoke bundles cannot install hooks): create a
clock automation with "close the card" whose template asks Claude Code to
run `sleep 30 && touch /tmp/deck-bg-COMPLETED` in the background and then
stop. The automation starts Claude but never sends it its first prompt
(Agent Bootstrap Input Safety): the row waits at "Waiting for first agent
interaction"; open the card's ⏱ panel and use send now once (do not type
anything else). After the turn ends the card must stay for as
long as `claude` runs; `/tmp/deck-bg-COMPLETED` appears, Claude resumes on its
own, and the card is never closed while `claude` is in front. Record the
Claude Code version, the Deck build and pass/fail; `/exit` must then let the
run close. Repeat with Codex: Esc-interrupt while a background terminal
runs; the card stays.

## Signal candidate (installed build, manual)

Required before a release that touches agent hooks, the status helper,
process identity, attention, notifications, scheduler signal admission or
automation finish. On the INSTALLED Deck with both agent integrations on,
in a scratch project, run each case, then note the card's session tag
(`sess-…` in `~/.deck/app.log`) and the case's start/end epoch seconds in a
plan file:

- `claude-normal`, `claude-permission`, `claude-restart` (quit and relaunch
  Deck between the prompt and its end), `claude-background-resume` (an
  automation with "close the card" whose prompt runs a background command
  and ends the turn; Claude resumes by itself; the card must stay — its
  first row is released with one send now, see below), and
  `claude-bootstrap-waits`: a clock automation whose command is `claude`
  and whose directory has never been trusted, so Claude opens on its
  folder-trust dialog; the row must stay at "Waiting for first agent
  interaction", the dialog must receive no keystroke (Claude is still on it
  a minute later) and `[queue] started sess-… — its first prompt waits for
  an agent interaction` must be the only queue line for that card. Esc out
  of the dialog afterwards; never answer it with Enter;
- Codex in two modes, certified separately:
  Shared-daemon Signal becomes Trusted only with verifiable binding to the
  specific interactive client and its current pane foreground generation.
  Inherited `TMUX_PANE`, cwd, timing, executable name, transcript path and
  session/thread/turn IDs are not pane ownership proof. This is not a version
  allowlist: 0.157.1 — BLOCKED; 0.160.0 — BLOCKED (2026-10-02). See
  [certification record](../docs/codex-signal-certification-20261002.md).
  `CodexSignalTrust::Unavailable` and scheduler/readiness gates remain unchanged.

  - **embedded (functional)** — Codex running without its shared background
    service. Launch each test Codex with `--no-daemon`, whose attribution
    topology can satisfy the existing model (verified through 0.160.0). It
    stays embedded even if a shared daemon already exists. Never stop a
    user daemon for certification. This is a test topology and optional
    compatibility path, never a product requirement. Cases: `codex-normal`, `codex-permission`, `codex-interrupt`
    (Esc), `codex-background-interrupt` (automation with "close the card";
    release its first row with one send now, then Esc while a background
    terminal runs; the card must stay), `codex-rapid` (a second prompt right
    after the first ends), and `codex-bootstrap-waits` (a clock automation
    starting `codex`: its row stays at "Waiting for first agent
    interaction" and nothing is typed into Codex until send now). Each must pass with v2
    identity; the stale-interaction and cross-pane regressions are pinned by
    the unit and trace suites.
  - **shared daemon (safety)** — `codex-daemon-refused`: at least two Codex
    clients in two cards sharing ONE daemon (default launch), each running a
    normal turn, a permission request and an interrupt. Its plan's
    "session" lists every client card's tag. It passes only when the log
    shows `terminal-discontinuity` refusals and not one accepted event or
    notification for those cards; also check by eye that no card (the
    daemon starter's included) shows agent status, Needs Attention or a Dock
    count, that a due owner-list row for each card stays in the ⏱ panel —
    the daemon starter at the agent hold (Codex Unavailable), the other
    clients at "Waiting for first agent interaction" (no trusted evidence
    can ever arrive) — never pasted, and that the row's send-now still
    sends.
    This is a safe-degradation pass, not a functional one.

Then `scripts/signal-candidate --log ~/.deck/app.log --plan plan.json
--deck-version … --deck-build … --claude-version … --codex-version …`
prints a `deck-signal-candidate/1` verdict and exits 0 only when every case
is `pass`; `fail` and `insufficient-evidence` both block. It reads only
Deck's content-free log lines, never an agent transcript. `[inbound] run
closed` carries no session tag, so the two auto-close cases treat ANY run
closing inside their window as premature: run them with no other automation
active (an unrelated close gives a false fail, never a false pass).

## Local Translation (unattended)

One command from the repository root, no manual step:

    python3 scripts/translation-lens-verify.py

It needs an unlocked GUI session (BLOCKED otherwise); the smoke window
restricts only its own input context to Roman input sources. It creates `/tmp/deck-tl-verify-*` (0700), a bundle with
its own path and `io.c9r.deck.smoke.tl*` identifier, one `deck-smoke-*`
socket and one data root per mode and run, verifies the pinned model pack
(read-only cache or a download into the run root), builds, runs L1, the
baseline negative control, then `DECK_TL_RUNS` (default 3) serial runs of the
`translation` and `translation-native` WKWebView modes, judges each with
`scripts/smoke-verdict`, and cleans up only what it started. The report is
`<run root>/report.json`; screenshots and per-run checkpoint lines are under
`<run root>/evidence/`. Each run starts with the `translation-guard` mode
(harness-safety negatives on a test-owned named pasteboard); the
general-pasteboard mode of that run is not started when it fails, nor when one of the driver fault probes (the real driver as a child, failing inside `run_mode` while a named guard holds modified content) fails. The
general pasteboard is used only inside the gated sections described in
`docs/translation-lens.md`; the report gives functional, harness-safety,
process-cleanup and shared-resource results separately.

## Away notifications (manual)

Release-location or `app/run.sh` bundle, an Agent status integration on,
**Settings → Agents & notifications → Notify me when away** on (accept
the macOS permission dialog; the status row must read "Notifications
allowed"). Start a `claude` card, ask it a question that needs a tool
approval, hide deck with ⌘W: a macOS notification titled with the card
title and "<project> · asked for your input" appears; the Dock badge shows 1.
Click the notification: deck comes to the front on that session. Answer,
let the turn end while deck is hidden: a "· a turn has ended" notification
appears; open the card: it is withdrawn and the badge drops. Leave deck in
the background for ten minutes before one of these transitions to confirm
App Nap does not delay it (the trigger is the Rust listener, not the poll).
Turn the switch off: badge cleared, no further notifications.

## Cross-project attention (B v01)

Run `DECK_SMOKE_DATA_DIR="$(mktemp -d /tmp/deck-attention.XXXXXX)" DECK_SMOKE_TMUX_SOCKET=deck-smoke-attention-UNIQUE DECK_SMOKE_WKWEBVIEW=attention app/run.sh`.
This fresh-root mode uses the shared 12-card fixture, real production modules,
and real empty-command shells/PTY attachments. Only poll statuses and explicit
poll/attach failures are injected in the debug test carrier; no agent or hook is
installed. It checks the uncluttered Board with all cards draggable and only the sidebar
attention entry, unchanged placement, keyed row focus,
pointer reconciliation, success/failure read receipts (including splits),
cross-project back navigation, persistent manual follow-up (including failed-save
rollback, unfollowing, viewed cards and unknown-state counts), stale/partial snapshots, locate-only stopped
entries, keyboard focus surviving a row reorder, and three lifecycle races:
a click on a pane whose shell exited only focuses it (no re-attach, no
restart), an exit that lands before its attach reply leaves the pane detached
and unseen, and a poll requested mid-flight runs again afterwards and hands
every waiter the follow-up. Twelve locale/theme/font combinations run in the actual WKWebView.
Run `scripts/smoke-verdict <root> attention`; exit 0. The same run
carries the 06 B v01 entry-point gate (`entry-*`, nine checks): one
persistent New session ▾ split button and no standing Automations/Templates
buttons; the empty-project start block (and no per-column attention text)
on a project without cards; the ▾ menu with arrow/Escape keyboard handling
and focus return; a click elsewhere or the global Escape clearing that menu's
keyboard handler; the Slack-preset editor opened from that menu; the project
tab menu opening both managers with focus returning to the tab; the ↻ chip
appearing only while a rule exists and opening the drawer; Templates…
reachable from the menu and from a list's 📋 with focus returning to the
opener; and one name per object in both dictionaries. The same run also
carries the 04 A v01 project-defaults gate (`defaults-*`, ten checks): the ▾
menu without and with defaults (first-item hint, "New shell only", "Project
defaults…" with its summary, the ＋ tooltip); ＋ with defaults starting a real
session in the default directory with the command sent once and the card
marked launched; "New shell only" keeping the directory with the fresh-shell
flag set and an injected recent-command chip rendered (the flag is set after
focusPane since 04; it had been reset there since v0.4.0); a missing default directory raising the three-choice
dialog with nothing written (cancel keeps it so, the home choice starts a
shell, the edit choice opens the defaults dialog); a context entry staying a
shell and its missing directory raising the two-choice dialog; a new
automation rule prefilled from the defaults while an existing rule keeps its
own; the defaults dialog (values, Save, Escape, focus return, the tab menu
entry); the empty project's link and promise text; and cleared defaults
leaving the project byte-identical; and a new shell whose poll turns dead
retiring its card and pane through the real close (the harness's synthetic
poll only knows the fixture cards, so a card you create by hand in the
left-over instance is NOT polled — press the strip's 真实轮询 first to hand
the page back to the real backend before testing exits). A debug-only strip
leaves Board / Empty project / Needs attention / Update failed available for
screenshots; the strip and fixture never ship in release bundles. Also inspect at the
minimum supported window size, 1280×800: controls must remain reachable and long names
must wrap. Sizes below 1280×800 are outside the supported product envelope.

## Settings navigation and diagnostic log reset

Run `DECK_SMOKE_DATA_DIR="$(mktemp -d /tmp/deck-settings.XXXXXX)" DECK_SMOKE_TMUX_SOCKET=deck-smoke-settings-UNIQUE DECK_SMOKE_WKWEBVIEW=settings app/run.sh`.
This isolated mode checks all eight categories, setting-level search (Slack,
and 通知 showing only the notification group), Enter locating the first match,
empty results, Escape/focus restoration, cancellation and acceptance of log
reset, and 32 layout combinations (English/Chinese, 100%/160% font, eight
categories) in a 680 × 400 settings frame. It leaves Data & privacy open for
visual inspection. Run `scripts/smoke-verdict <root> settings`; exit 0.
Reset runs before smoke results are written, so it cannot erase test evidence.

## Upgrade-aware tmux lifecycle

All automated tests use `deck-smoke*` sockets. Never point a smoke command at
`-L deck`, never delete `~/.deck`, and do not use a production card/session as
a fixture.

### Graceful agent exit and restart timing

- [ ] In an isolated build, enable terminal output recovery and create test
      Claude Code and Codex sessions. Restart while idle, while running, with
      draft input, in copy mode, and with a slow exit hook. Verify real CLI
      versions separately: raw-mode fixture tests do not certify CLI behavior.
- [ ] Successful exit returns to shell, stable resume hints are saved, and
      opening retained cards starts only a shell. With recovery disabled,
      the modal discloses that Deck will not save exit hints.
- [ ] A slow/refused exit or failed snapshot aborts before kill-server. Some
      agents may already have exited. The service PID remains unchanged.
- [ ] All agents share a 2s exit budget; preparation has 3s and the backend
      response has 8s. After 300ms the modal shows phase/progress. A blocked
      filesystem may retain the backend locks after the timeout response;
      when it returns it must not proceed to a late kill.
- [ ] Queued prompts for affected sessions remain paused after replacement
      and after app relaunch. An in-flight delivery makes restart return busy.
      Old poll/attach replies do not reopen, delete or revive cards.
- [ ] Inspect content-free `[tmux-restart]` anchors: `begin`, `validated`,
      `classified`, `exit-request`, `agents-exited`/`exit-timeout`,
      `output-stable`, `snapshot-saved`/`snapshot-failed`, `prepared`,
      `queue-paused`, `stopping`, `stopped`, `starting`, `verified`, `finish`.
      A hard response timeout adds `watchdog-timeout`. Logs contain durations,
      counts and hashed session tags, never prompt/resume text or paths.

### Isolated same-build and failure recovery

- [ ] Launch a debug smoke bundle with a fresh absolute data directory and a
      unique `DECK_SMOKE_TMUX_SOCKET=deck-smoke-lifecycle-...`. Create a shell
      running a harmless counter. Record server PID, pane PID and metadata:

      ```sh
      tmux -L deck-smoke-lifecycle-UNIQUE display -p '#{pid} #{start_time} #{socket_path}'
      tmux -L deck-smoke-lifecycle-UNIQUE show -gqv @deck-server-metadata
      tmux -L deck-smoke-lifecycle-UNIQUE list-panes -a -F '#{session_name} #{pane_pid} #{pane_current_command}'
      ```

- [ ] Quit/reopen the same bundle, then force-quit it once and reopen. The
      server PID, pane PID, session and counter continue; no upgrade modal or
      pending sidebar action appears.
- [ ] On the isolated server only, replace the metadata option with a fixture
      carrying an older release identity. With a live session, relaunch does
      not kill it. “Later” closes the modal, existing sessions remain usable,
      the sidebar/Settings still say Restart required, refreshing the UI does
      not re-open the modal, and another app relaunch remembers the deferral.
- [ ] Remove the metadata option to model legacy, then repeat with malformed
      JSON. A live session is preserved and reported as Legacy/unknown or
      unavailable; an empty server is safely replaced and reports a new PID
      with current metadata.
- [ ] In the confirmation window, create another session before clicking
      restart. The backend refuses the stale confirmation and returns an
      updated affected list. Double-click Restart and try opening a new card
      during the operation; there is one replacement server and no new session
      can enter the old generation.
- [ ] In a disposable WK smoke run, arm `smoke_fault_set` in turn with
      `tmux-after-stop`, `tmux-after-socket`, `tmux-before-start`, and
      `tmux-after-metadata`, then confirm restart. Reopen after each injected
      interruption: a matching
      persisted intent resumes to one current server; an unexpected different
      PID is never killed under the prior confirmation and requires review.
      `tmux-lifecycle.json` contains PID/start/socket device+inode/count/
      build fields only—no socket path, session names, commands, prompts,
      terminal text, or project paths.

### Slack channel transport faults

- [ ] Start only an isolated data root that already has the sandbox channel
      credentials and rule, with `DECK_SMOKE_WKWEBVIEW=channel-fault`. The
      runner arms the closed debug-only `channel-network` and `channel-scope`
      faults in turn; it does not disable Wi-Fi/VPN or write to Slack.
- [ ] For each fault, Settings shows not connected/no backfill, an unresolved
      gap, and the exact `network` or `scope` code. Clearing the fault restores
      the real Socket Mode connection while the explicit gap remains.
- [ ] Treat `scope` as deterministic application-path acceptance paired with
      Slack's live configuration constraint and the Web API
      `missing_scope -> scope` contract test. Do not report it as a successful
      live scope removal when Slack requires the scope for a configured event.

### Connector post-accept recovery

- [ ] Launch the isolated `connector-transport` runner with
      `--smoke-fault connector-after-accept`, then run the opt-in Simulator
      AppModel host test with `DECK_CONNECTOR_EXPECT_POST_ACCEPT_FAILURE=1`.
- [ ] The first command is durably accepted but its POST returns `timeout`.
      The app must retain the immutable operation ID, query that same operation,
      reach `applied`, and find exactly one matching note before continuing the
      existing edit/delete, stale-revision, Keychain restore and unpair checks.

### Signed updater and responsible-code gate

- [ ] Use two authorized, increasing, signed/notarized candidate builds from
      `/Applications/deck.app`. Start sessions on the first and record app
      version/commit, tmux PID, `codesign -dv --verbose=4` identifier/Team ID,
      and `otool -l`/`dwarfdump --uuid` UUIDs for the main executable and
      bundled tmux. Confirm there is one main executable and the helper is
      inside the signed app.
- [ ] Install the second through deck's updater. Before confirmation, `ps`
      may still show the original launch argument, but `lsof -p OLD_TMUX_PID`
      is the kernel-image authority and may show the deleted
      `tauri_current_app/.../current_app` image. The new deck must report the
      old build as Restart required and must not create another server from
      that backup, `/tmp`, a DMG mount or App Translocation.
- [ ] Exercise an update between two fixed candidates. After each update
      settles, verify the final deck process has `PPID=1` and `PGID=PID`; the
      prior deck PID/PGID is gone, no `deck-app --deck-relauncher` waiter
      remains in `ps`, `launchctl list` shows only the ordinary
      `application.io.c9r.deck.*` entry (deck never submits a launchd job), and
      `app.log` contains the clean-relaunch event. A transient intermediate
      process must not reach tmux/session creation.
- [ ] Choose Later and verify the old process/session continues and the prompt
      does not loop. Then save work and confirm restart. Observe: old PID exits;
      the socket is usable; new PID differs; `show -gqv
      @deck-server-metadata` matches the installed version/commit/helper/
      protocol/source; `lsof -p NEW_TMUX_PID` resolves the executable image
      under the final installed `deck.app`, not a deleted updater directory.
      Cards remain stopped/restartable, while old shell/agent PIDs are gone.
- [ ] Repeat the update with no sessions. Replacement is automatic, produces a
      current PID/metadata and only a non-blocking result toast.
- [ ] Verify Settings shows current deck identity, server identity, PID/start
      time and Current/Restart required/Legacy status. Manual Restart uses the
      same destructive copy and safe default focus as the upgrade path.

### Local Network Privacy

- [ ] Confirm the signed app's final `Info.plist` contains the exact
      `NSLocalNetworkUsageDescription` explaining that terminal tools may
      access user-chosen local services; it must not claim deck scans the
      network. Do not run `tccutil reset`, modify privacy databases, or add a
      system route/privileged daemon for this test.
- [ ] From a session owned by the new server, access a user-controlled LAN test
      service directly. If macOS prompts, the prompt belongs to the installed
      current deck identity. After allowing it, direct access works without an
      SSH/loopback workaround and `lsof` attributes the connection to the new
      helper image. Record OS version, app/helper UUID, Team ID/CDHash, server
      PID/metadata and prompt ownership; do not record service addresses,
      commands, terminal output or project/session names.
- [ ] Create the LAN-test session through the signed deck UI/backend command,
      never through an external tmux client. Check the macOS Local Network log
      around creation and the TCP probe: it must not report the prior deck PID,
      prior executable UUID, `bundle_id: (null)`, or a blocked notification.
- [ ] Check Launch Services does not retain newly generated debug/smoke apps
      under `io.c9r.deck`: dev is `io.c9r.deck.dev`, smoke is
      `io.c9r.deck.smoke`. Stable and Nightly still replace one installed app
      and keep the same Developer ID identity.

## Input & rename
- [ ] New session → type `ls` → characters echo, Enter runs it (TSM/IMK alive)
- [ ] Chinese IME: type 中文, composition window appears, Enter commits
- [ ] With Simplified Chinese Pinyin and ABC in turn, type `[ ] ? ( ) { } < >`,
      shifted variants and full-width punctuation; no key silently disappears.
      Verify Enter, Escape, Backspace and arrows during preedit. This is a
      physical gate: never mark it from synthetic composition events.
- [ ] ⌘V pastes into the shell; ⌘C copies a selection out
- [ ] Rename a non-active session from the sidebar: Enter removes the editor
      immediately, persists exactly once, and updates Board/sidebar/open-pane
      titles. Reopen the app and confirm the title remains.
- [ ] Rename again: Escape restores without a write; click away commits once;
      Chinese IME Enter commits composition first and does not end editing.

## Scrolling & selection
- [ ] Fresh shell: trackpad scroll does nothing (no pull-down, no copy-mode badge)
- [ ] `seq 200` → scroll up reaches history, scroll to bottom auto-returns live
- [ ] Compare slow trackpad movement, fast swipes, inertia tails and direction
      reversal with Terminal.app/Warp: updates follow display frames without
      the old 50ms stepping, and sub-line input is not dropped.
- [ ] Scroll up and STOP: an accent "⤓ scrollback" chip appears in the pane
      header within the gesture (view is frozen history — an agent TUI must
      never look silently hung); clicking the chip OR typing returns live
      and the chip disappears
- [ ] Inside `claude`: long output scrollable; typing still reaches the agent
      (typing while scrolled first leaves copy-mode, so keys are never eaten
      as copy-mode commands)
- [ ] `terminal-paint` in the automated WK run compares xterm's public buffer
      with its rendered DOM after `onRender`, wheel up/down, and new output
      after reattach. This catches a consumed-but-unpainted terminal; manual
      screenshots remain necessary to check WebView compositing/capture.
- [ ] Start default `codex` (no `--no-alt-screen`) in an isolated instance,
      request 80 numbered lines without tools, and wheel up/down. Codex's own
      transcript must scroll and return to bottom without changing its composer.
      Then exit Codex and verify shell history and local word/line selection.
      Mouse negotiation must be on before the app starts; an already-running
      app may retain the old policy until it restores its terminal screen.
- [ ] Drag-select multiple lines → ⌘C → paste elsewhere matches
- [ ] Double-click a word / triple-click a line, keep the last press held,
      and drag up, down, then reverse across the original unit. The original
      word/line stays included and endpoint granularity matches native xterm.
      Copy matches byte-for-byte, including mixed Unicode. A wheel frame while
      held does not freeze the native gesture; blur ends an unfinished gesture
      but preserves a finished selection (`selection-multiclick-drag` smoke).
- [ ] Without cancelling that selection first, immediately drag-select a
      different multi-line range. The new range replaces it without a
      "session changed" error or leaving the pane in copy-mode.
- [ ] Produce at least 2,500 deterministic rows containing Chinese, emoji,
      combining/ZWJ characters, blank lines, fenced-code markers, tabs,
      trailing spaces and a line wider than the pane. Drag directly on xterm
      cells into the top edge and hold: highlight and anchor remain continuous
      while tmux crosses multiple screens. Repeat downward from history.
- [ ] After crossing multiple screens, reverse direction within the same drag:
      the selection shrinks without duplicating, dropping or reversing rows.
      ⌘C copies only that logical selection; with no selection, the existing
      clipboard remains byte-identical.
- [ ] Paste into an external text target and verify start/end markers, order,
      hard blank lines, joined soft wraps, Unicode byte count and summary.
- [ ] Repeat in horizontal and vertical splits. Only the gesture's pane may
      scroll or highlight; its sibling keeps focus, viewport and xterm/PTY rows.
- [ ] While holding the selection, generate live output and resize the window,
      sidebar and divider. Then test Escape, pointer cancel, app blur, pane
      switch and detach: each stops edge work immediately and restores input.
- [ ] Select beyond 20,000 rows and at the 50,000-row history boundary. The UI
      announces the reachable limit; clipboard requests over 64 MiB fail
      explicitly and never return a truncated highlighted range.

## Board & cards
- [ ] Drag a card between boards (native drop must not swallow HTML5 DnD)
- [ ] Double-click board title renames (no render() mid-dblclick regression)
- [ ] Card ✕ closes instantly; in-session Close shows the custom confirm
      (window.confirm is a silent no-op in WKWebView — never use it)
- [ ] In an isolated profile, restart the shell service with two ordinary
      cards. Both show stopped/old snapshot; Close one and verify it disappears
      from Board and sidebar without an error, while the other remains. Start
      a fresh ordinary card and Close it to verify the live path.
- [ ] With delayed persistence, overlap two card closes; close+rename/move;
      project delete+unrelated create/rename; and a failed first write followed
      by a successful second mutation. Reload `deck.json`: it must exactly equal
      the final visible Board, with no resurrection or lost unrelated change.
- [ ] Verified shell exit: Ctrl+D, `exit`, or `exit 7` at the prompt of an
      ordinary card's owning shell retires the card through the durable close
      transaction (queue cancelled, Board saved, pane closed) with one
      "closed — shell exited" toast. With queue-cancel or Board-save failure the
      stopped card and pane stay visible, the retire error toasts once, and a
      later poll retries; after writes are restored it retires exactly once.
- [ ] Unexplained session loss keeps the card stopped/restartable with its
      queued prompts: `tmux -L deck kill-session -t =<session>`, `kill -9` of the
      pane's shell, `tmux -L deck kill-server`, a service restart, or an
      MCP-origin card whose runner ends. `claude`/`codex` exiting back to the
      shell never retires anything. Explicit Close with queue-cancel or
      Board-save failure keeps the card and pane; retry after restoring writes
      closes them only after durable success.

## Completion & separators
- [ ] Second command typed shows gray ghost; Tab applies remainder only
- [ ] Test a fresh shell prompt, a scrolled-history prompt, a prompt on the
      last visible row, a long wrapped command, rapid input, pane resize, and
      horizontal/vertical/nested splits. The candidates occupy real reserved
      space and never cover any terminal row; only the focused pane shrinks.
- [ ] While candidates show, compare xterm rows and `tmux display -p
      '#{pane_width} #{pane_height}'`; they agree. Hide candidates and confirm
      both grow back, the prompt/cursor remains visible, and no extra jump or
      blank row is introduced.
- [ ] Separator lines appear between shell commands, none inside `claude`

## File drop & image paste (Warp-style path insertion)
- [ ] Take a screenshot (⌘⇧4) → drag its floating thumbnail onto a terminal
      pane → the pane outlines in accent, and on drop a quoted path under
      `~/.deck/drops/` is typed at the cursor (no Enter); the agent/shell can
      read that file
- [ ] Drag a file from Finder onto a pane → same path insertion; dragging a
      CARD between boards still works (file drags must not break card DnD)
- [ ] ⌃⌘⇧4 (screenshot to clipboard) → ⌘V in a pane → same: file saved,
      path typed; plain TEXT ⌘V still pastes as text
- [ ] `ls -l ~/.deck/drops` → files 0600, dir 0700; relaunch after 7 days
      (or backdate with touch) → old drops pruned

## Scheduler deletion (release gate — orphan sessions)
- [ ] Card with a recurring rule ("every 1 min") → close the card → the queue
      panel loses its rows, and after several minutes NO tmux session comes
      back: `tmux -L deck ls` shows nothing for it and `~/.deck/queue.json`
      lists the session under `cancelled`
- [ ] Same, but close the card in the second the prompt fires (rule due, hit
      ✕): the send may still land, `deliveries` records it, and still nothing
      re-arms or restarts
- [ ] Delete a whole project holding 2–3 scheduled cards → every one of their
      queue rows is gone at once, other projects untouched
- [ ] Ctrl+D a shell that has queued prompts → card retires itself and its
      queue rows go with it
- [ ] `kill-session` (or kill-server) under a card that has queued prompts →
      card remains stopped and its queue rows remain; explicit Close removes
      the card and cancels its queue
- [ ] `chmod 400 ~/.deck/queue.json` → close a card → an explicit toast, the
      card STAYS on the board (never a silent delete with a live schedule);
      `chmod 600` back → closing works
- [ ] Start from a persisted `firing` item and force the boot repair save to
      fail. The UI still exposes acknowledge/retry immediately; the item stays
      ambiguous and cannot fire. Restore writes: the exact in-memory snapshot
      is flushed, remains ambiguous after restart, and the dirty flag clears.

## File-path menu

- [ ] Print `说明(/tmp/a.txt)`, `[说明](src/main.rs)` and `file(1).txt`.
      Menus show only the path, preserving brackets that belong to filenames.
      Press a path while its row repaints unchanged: one menu opens on release.
      Changing the target, resizing, losing focus or dragging cancels the click.
      A click without prior hover still opens the menu. The isolated run covers
      these through `link-repaint` modes 0–7 (including OSC 8 priority) and bounds the former quadratic
      scans through `link-scan-bounded` (median under 50ms at 6,400 characters).
      Production `terminal-link` events contain only run/pane/attempt IDs,
      closed outcomes and counts/durations; slow scans are limited to one per
      pane per five seconds. Action events use codes 1/copy, 2/URL, 3/editor,
      4/editor-parent, 5/session-parent, 6/reveal. No path or content is logged.

- [ ] Print relative and absolute paths containing spaces, Chinese and emoji,
      plus `:line[:column]`. Keyboard-open the menu: URL entries remain only
      Open/Copy; file entries include Open, Reveal, Copy, Open parent folder in
      editor, and New session in parent folder. Arrow/Home/End/Escape navigation
      and focus restoration work.
- [ ] Print a nonexistent log token (`memcache.go:265`), an IPv4 address with
      port, and a long HTTP(S) URL that soft-wraps through its `/api` segment.
      The missing file remains a candidate (actions report failure); the IP
      has no link. Every wrapped URL row resolves to one exact
      URL value and never exposes `/api` as a file path.
- [ ] Open parent uses the configured editor with the directory as an argument;
      New session starts in the canonical parent and follows the normal project/
      Board placement rules. Repeated clicks create at most one session. Missing,
      unreadable or stale paths show a safe error and create no ghost card/session.

## Splits
- [ ] Both toolbar buttons and ⌘D / ⌘⇧D open the session picker. Open panes
      are excluded; “new shell here” works even when no other cards exist.
      Create a shell and select an existing session in each direction.
      Typing goes to the FOCUSED pane; no reflow jitter from the completion
      bar; divider drags.
      The isolated WKWebView run covers these entry points with the six
      `split-picker-*` checks, once per direction (a=1/right, a=2/down).
      They verify creation, attachment, geometry, PTY row sizing and a real
      shell input/output round trip; `scripts/smoke-verdict <root> run`
      exits 0.

## PTY flow control
- [ ] `seq 1 500000` (or `yes | head -2000000`) → output streams smoothly to
      the end, scrollback intact at the tail (ACK window at work: no dropped
      or reordered bytes, no beachball)
- [ ] While it streams, close the pane mid-flood → no hang, no crash
      (detach closes the AckGate and releases the emitter); reopen the card
      → terminal repaints correctly (fresh generation, stale tail dropped)
- [ ] After heavy output, `grep "ack stall" ~/.deck/app.log` — a stall line
      is fine (it means the window did its job); the app must have stayed
      responsive throughout

## Update & settings
- [ ] Settings → editor list shows installed editors; file link opens there
- [ ] Theme and Accent switch immediately; every already-open split pane and a
      newly created split use the same xterm background/cursor/selection/ANSI
      palette. Force a settings-save failure and confirm the prior palette and
      selectors return with an explicit toast.
- [ ] Select Follow System, toggle macOS Light/Dark, and confirm live switching;
      select a fixed theme and confirm later macOS changes are ignored.
- [ ] deck menu → Check for Updates… reports up-to-date (or offers install)
- [ ] Existing/missing/corrupt channel settings start on Stable. Opt into
      Nightly only after the risk confirmation; the version label shows
      `vX.Y.Z · Nightly · commit`. Switch back and confirm the no-downgrade
      explanation, then restart and verify the Stable preference persisted.
- [ ] Make the Nightly feed unavailable or malformed: the check fails visibly
      and never queries Stable or another URL. Test a deliberately invalidly
      signed fixture only in an isolated feed/release: Tauri refuses install.

## Dropdowns
- [ ] Every select in the list form, the automation editor and Settings opens
      deck's own listbox (the `run` mode's `dropdown` check, judged by
      `scripts/smoke-verdict`): the button label
      follows a programmatic value, a choice fires `change`, Escape closes and
      returns focus WITHOUT leaving the session view, and a hidden select hides
      its wrapper. The 31-day day-of-month list scrolls inside the menu.

## Lists & templates
- [ ] Start a list not before 1 min out on a harmless shell card whose launch command
      is empty → deck automatically binds the exact pane and fires once in
      compatibility mode; its row disappears (there is no "fired" UI; the send is
      recorded in queue.json's `deliveries` audit list, capped at 200 entries)
- [ ] Start a Codex/Claude/OpenCode card with an explicit launch command, queue
      a prompt, then put another program in the foreground → the row says it is
      waiting for the expected executable and attempts remain 0. Return that
      executable to the foreground → exactly one send lands. Do not configure
      any tmux pane hook for this test.
- [ ] Queue while a manually started agent is already foregrounded on a card
      with an empty launch command → deck captures that executable and waits if
      it later changes. Queue before the agent starts on the same kind of card
      → no process is captured; the prompt still sends to the exact same pane.
- [ ] Kill a scheduled session before it is due. On the next tick deck starts
      it and polls pane/process metadata rather than sleeping a fixed 2.5s. The
      expected executable appearing succeeds; a mismatch reaches the bounded
      timeout and remains blocked.
      While polling, separately pause, edit and delete items; each stops without
      a firing intent, delivery attempt, or ambiguous record.
- [ ] With one session blocked in boot readiness, a due prompt on a second
      session still advances independently.
- [ ] Start a list, then add a row from its footer → they fire in order, the
      row only after the first target went quiet (~3 min; "quiet" ≠ "done" —
      the UI must say quiet) and ≥60s after the first send (per-session min
      gap). With two lists on a card, a row added to the OLDER list joins it,
      not the newest.
- [ ] Two prompts due at once on the SAME session → they arrive one per
      20s-tick, a minute apart — never both in one tick
- [ ] Schedule onto a stopped card whose directory was deleted → the row shows
      context unavailable/waiting, attempts do not increase, no firing intent
      is created, and a queued follow-up does not run past it. Separately force
      a real post-intent tmux refusal to retain the existing backoff/gave-up,
      retry ↻, skip ⏭ and group-blocking behavior.
- [ ] For a process-mismatch row choose keep waiting and cancel in turn.
      “Send now…” shows the expected and current processes; Enter and blur
      cannot accept its high-risk dialog, while an explicit click performs one
      process-only override without changing the saved expected executable.
- [ ] Replace a scheduled session/pane under the same name (kill the tmux
      server, or update the app, then reopen the card) → the next pass adopts
      the new identity, persists it, and delivers without any user action; a
      card whose launch command names an agent still waits for that executable.
- [ ] Type a prompt with several lines into the queue field: Enter adds a
      newline (it does not queue), ⌘↵ queues it, and the pasted prompt reaches
      the agent's input as those same lines with a single submit at the end.
      The row shows only the first line plus `⏎N`; its chevron opens the rest
      and clicking the text edits the whole prompt
- [ ] A template row written over several lines survives a save/reopen and a
      Slack-badge automation fills `{{msg.text}}` into it with the message
      flattened to one line while the row keeps its own
- [ ] Save a template from a list's 📋 → start a list from it on another
      card, and insert it into an existing list (its rows join that list)
- [ ] A repeating list: pause ⏸ → skipped while paused; resume → fires again;
      add a row from its footer and delete one → both survive the next fire
- [ ] Automations drawer: an automation with the Slack-badge trigger appears
      beside the clock ones; Settings shows only the Slack connection

## Data durability
- [ ] Quit deck → corrupt `~/.deck/deck.json` (truncate mid-JSON) → relaunch:
      board restores from `.bak`, a toast explains, the corrupt file is set
      aside as `.corrupt-<ts>` — NEVER silently replaced with an empty board
- [ ] Valid-JSON corruption too: replace deck.json's contents with `{"x":1}`
      → same recovery path (typed validation, not just a JSON parse)
- [ ] Delete `.bak` as well (no intact `.corrupt-<ts>` left from earlier) →
      relaunch: no toast; a dialog offers ONLY "Start a new Board", naming
      its three consequences, with Cancel focused. Cancel → the in-memory
      board stays, a change is refused and no file is written; the first
      refused change offers the dialog once more, later ones do not. "Start
      a new Board" → the default project is saved, deck.json exists again,
      every `.corrupt-<ts>` is untouched
- [ ] Quit → rename a healthy `deck.json` to `deck.corrupt-1700000000` and
      delete `.bak` → relaunch: the dialog offers ONLY "Restore the kept
      Board", naming the copy's date and card count; Restore → the cards are
      back, deck.json is written again, the kept `.corrupt-<ts>` copy
      remains. (`chmod 000` cannot stage this: launch re-restricts every
      data file to 0600 before the Board loads.)
- [ ] Set `"schema_version": 99` in deck.json → toast says update deck; the
      file is left byte-identical (no .corrupt, no overwrite on save)
- [ ] Set `"schema_version": "1"` (a STRING) → treated as damage: recovery
      from `.bak` + `.corrupt-<ts>` kept, never read as a legacy file
- [ ] Delete the `data` key but keep `schema_version` → same recovery path
- [ ] Same truncate drill for `queue.json`
- [ ] Launch a second deck instance → alert "deck is already running", no
      data raced

## Privacy (release gate)
- [ ] `rm ~/.deck/app.log`, then: type a command with a distinctive marker
      string into a session, schedule a prompt containing the marker, export
      logs. `grep <marker> ~/.deck/app.log ~/.deck/exports/*` → ZERO hits.
      Bytes/counts/session names in logs are fine; user content is not.
- [ ] Relaunch with `app/run.sh --debug-logging`, then repeat — including
      ⌘V-pasting the marker into the shell (bracketed paste) and typing it through the IME:
      marker still absent (debug adds volume, never content; the frontend
      can only emit whitelisted event codes, per-code closed detail values
      and numbers)
- [ ] `grep -E '/Users/|file://' ~/.deck/app.log ~/.deck/exports/*` → zero
      hits (errors are logged as category codes; the tmux binary is logged
      as sidecar/missing, never as a path; storage recovery logs name
      files, never absolute paths)
- [ ] `grep -E 'deck-[a-z0-9]+-[a-z0-9-]+' ~/.deck/app.log ~/.deck/exports/*`
      → zero hits: sessions appear as `sess-xxxxx` tags, never by name
- [ ] Migration of what an OLDER deck left: append a fake legacy line
      (`echo "1 [pty] attached deck-my-card-ab12 /Users/$USER/secret" >>
      ~/.deck/app.log`), relaunch deck → the line is still there structurally
      but the name and path read `<redacted>`, the file is still 0600, and no
      `.bak` copy of the raw line exists anywhere in `~/.deck`
- [ ] Permissions: `ls -ld ~/.deck ~/.deck/exports` → `drwx------` (0700);
      `ls -l ~/.deck/*.json ~/.deck/*.json.bak ~/.deck/*.corrupt-* \
      ~/.deck/app.log ~/.deck/exports/*` → everything `-rw-------` (0600),
      including deck.json, queue.json, settings.json, history.json, every
      `.bak`, every quarantined `.corrupt-*` and every export
- [ ] `chmod 644 ~/.deck/deck.json; chmod 755 ~/.deck` → relaunch deck →
      both are back to 0600/0700 (boot-time migration)

## MCP-managed session blocks shell restart

- [ ] MCP-managed session blocks shell restart: with an isolated test profile,
      create a visible idle managed card and make shell restart required. Open
      restart review: it names the card and explains why it cannot be restored
      as an ordinary shell. Ordinary Restart must not fail once before showing
      this explanation. Cancel: card and service remain. Confirm “Close MCP
      sessions and restart”: Board closes the card, then the service restarts;
      ordinary cards retain their existing stopped/restartable behavior and the
      MCP card is not restored as an ordinary shell. Repeat with an active job:
      the warning explains termination and the job stops before restart.
      Inject a rejected or uncertain close: service PID remains unchanged and
      unrelated ordinary cards never show a false stopped state.
- [ ] Repeat with only one managed card and no ordinary tmux session. Quit and
      relaunch Deck without replacing its shell service, so the managed runner
      is stale. Close that card from restart review: the still-running empty
      server must be replaced (new PID and current build metadata), with no
      dummy or restored MCP session.

## Security baseline
- [ ] `app.log` contains no `CSP` violation lines after a full session of use
      (the securitypolicyviolation listener logs any)
- [ ] A `file:///…` or non-http link printed in a terminal does NOT open on
      click (only http/https leave the app)

## Release channels (explicit authorization only)

Nothing in this section is authorized merely by the checklist; see
`docs/release-channels.md`.

- [ ] With explicit release authorization, publish two increasing, signed and
      notarized Nightly candidates. Install the first DMG, verify Gatekeeper and
      real data/tmux/scheduler/terminal/i18n survival, then verify Nightly
      self-update to the second while a Stable client sees neither prerelease.
- [ ] With explicit promotion authorization and production Environment review,
      promote the tested candidate. Record candidate/Stable URLs and workflow
      runs; compare DMG/archive/signature SHA-256 byte-for-byte; update an older
      Stable through `/releases/latest/`.

## Autonomous real-clock first-step acceptance (isolated debug candidate)

`clock-live` is a setup-only WKWebView driver. It saves harmless templates
through the production provider and rules through the automation editor,
exercises decline/enable/disable/re-enable confirmation, and arms legal future
local-minute slots. It never creates cards, queue rows, native events or Signal.
Before configuring rules it validates native hook attribution with one separate
manual-input setup card, using normal context guards and explicit isolated risk
confirmation, then closes that generation. These setup actions are outside all
unattended windows. After `clock-live-armed`, it performs no input, focus, redraw
or mutation.
The real native clock scheduler creates every run.

Prepare a disposable private root with `home`, `work`, `claude`, `codex`, `data`
and `evidence` directories (0700). Use only existing authorized authentication,
selectively provision necessary credential records (0600), and complete the
CLIs' ordinary folder/hook trust flow in separate setup generations. Never copy
normal configuration trees, bypass permissions or bootstrap the test targets.
Install the production-shaped status-helper hooks only into those private CLI
configs, referring to the candidate bundle helper. Disable the test CLI's updater.
Launch the candidate with LaunchServices and a private HOME, CLAUDE_CONFIG_DIR,
CODEX_HOME and PATH, in addition to absolute smoke data and a unique deck-smoke
socket. Do not use the normal `app/run.sh` path without its isolation variables.
A build-only preparatory smoke launch must be retired before this private-env
launch; never replace the normal application.

Run `--smoke-wkwebview clock-live` on the isolated candidate. The default driver
configures A (OFF/fresh Claude), B (ON/one reply), C1–C3 (three independent fresh
Claude runs/three owner steps/review OFF), and D (review ON). Slots are one minute
apart; chain rows retain the ordinary 180-second quiet and 60-second send gap.
The separate Codex track uses a fresh private data directory/socket with the
closed scenario `codex` in `translation-fixture/scenario`; it configures E only,
with `codex --no-daemon`. A Claude result never certifies shared-daemon Codex.

Once setup reports armed, before the earliest due time, run:

```sh
python3 scripts/clock-unattended-observe.py --root <private-root> \
  --data <private-root>/data --socket deck-smoke-<unique> \
  --evidence <sanitized-evidence-directory>
```

The observer freezes deadlines before the trigger (90-second A observation,
180-second single-step/control budget, 780-second three-step budget including
ordinary follow-up waits). It only reads test-owned state/transcripts and passively
captures panes. Expected markers are constructed by the Agent from separate
pieces, absent verbatim from the input; only exact assistant-role messages count.
It preserves native slot, row/binding transitions, delivery audits, actual replies,
review state and stabilization log intervals. No timeout authorizes resubmission.
Record candidate diff digest, binary hash, Agent versions, all setup failures,
intervention counts and cleanup in `unattended-acceptance.json`. Setup verdicts
from `scripts/smoke-verdict <data> clock-live` certify configuration only.

Save sanitized evidence first, then retire only the exact test-owned GUI/socket
with `scripts/edr_runtime.py --cleanup --socket deck-smoke-<unique>
--include-foreground`. Remove private credentials and configurations; retain the
harness and evidence. A final read-only resource audit must be empty. Never use
broad process-name cleanup or normal `~/.deck` as a fallback.

## Card Reminder unattended verification

Run `python3 scripts/reminder-verify.py` against the live dedicated Mac mini.
The `reminder` WK mode performs real own-window configuration and actual shell
exit/restart/retirement controls in private carriers. It waits for a short real
in-app due instant and checks the Dock. The fixed A1–E6 matrix and cleanup are
recorded in `unattended-acceptance.json`. System permission, visible notification,
quit delivery, Notification Center clicks and buttons require a legally
authorized remote system UI channel and remain BLOCKED when unavailable;
never substitute handler injection. See `docs/reminder-verification.md`.


The Reminder verifier also supports `--target local|macmini|auto` for the
explicitly authorized local smoke fallback. Local system actions use the
connected agent CUA channel and bounded `ui-request.json` receipts; actual
UN callbacks and inventory, not receipts alone, certify each action. The
signed carrier has an early `native-inventory`/`native-cleanup` mode which
never loads Board or rearms notifications. See `docs/reminder-verification.md`.
