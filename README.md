# deck

[简体中文入口](docs/zh-Hans.md) · English documentation is the canonical technical specification.

**A native macOS command center for terminal agent sessions** — Claude Code,
Codex, or any long-running CLI. Every card on the board is a real, persistent
terminal session; the board shows which sessions have recent output, are quiet,
or have stopped, so you decide where to look next.

```
┌ Attention ──────┐┌ Working ─────────┐┌ Queued ─────────┐┌ Parked ─────────┐
│● fix flaky test ││● refactor auth   ││○ migration plan ││○ changelog pass │
│  ⧗ quiet 3m     ││  ⏺ cargo test …  ││                 ││                 │
│  inspect next   ││            812M  ││                 ││                 │
└─────────────────┘└──────────────────┘└─────────────────┘└─────────────────┘
```

## Install

Download the latest `.dmg` from [Releases](https://github.com/c9r-io/deck/releases),
drag to Applications, open. That's it:

- **Zero dependencies** — a statically linked tmux ships inside the app
- **Signed & notarized** — no Gatekeeper prompts
- **Self-updating** — new versions appear as a button in the sidebar

Apple Silicon only for now.

### Update channels

Stable is the default and reads only the latest non-prerelease Release.
Maintainers and testers can explicitly opt into **Nightly** in Settings to
exercise a signed and notarized candidate through its separate feed. Nightly
uses the same app identity and `~/.deck` data, so it replaces Stable rather
than installing beside it. Back up important data first and do not run Stable
and Nightly at the same time. Switching back to Stable affects future checks
but does not downgrade a newer Nightly; reinstall the Stable DMG if a downgrade
is required. See [release channels](docs/release-channels.md) for the complete
operator and recovery guide.

## What it does

**Boards express attention, not workflow.** Default boards per project:
*Attention* (things you want to deal with next) · *Working* (agents running
autonomously) · *Queued* · *Parked*. Cards never move on their own — status
dots and amber highlights carry the information; you make the calls. Boards
are per-project (tabs), and fully customizable: add, rename, delete, drag
cards anywhere.

**Sessions outlive the app.** deck runs its own private tmux server, so
quitting deck (or it crashing) never kills your agents. Reopen and everything
is exactly where you left it. Closing a card (corner ✕, or Ctrl+D in the
shell) is the only way a session ends.

**Upgrades have an explicit process boundary.** Reopening the same deck build
keeps the same tmux server PID and every running process. After deck itself is
upgraded, an empty old server is replaced automatically; if it still owns
sessions, deck asks before restarting the background shell service and shows
what will be affected. You can choose Later and keep using existing sessions,
but finishing the restart ends their commands and agents—Unix processes cannot
be migrated into a new tmux server. The pending action remains in the sidebar
and Settings.

**A real terminal.** Full xterm with truecolor, ⌘C/⌘V, clickable existing
local file paths, and complete HTTP(S) URLs even across terminal soft wraps.
Drag directly over terminal cells; holding at either vertical
edge continuously extends the same selection through tmux history, including
reverse shrinking across screens. A path can open in the editor, reveal in
Finder, open its parent in the configured editor, or start a new session in
that parent directory.

**Split view.** Watch several agents at once: drag a card from the sidebar
onto a pane edge, or hit ⌘D / ⌘⇧D (or the ◧ ⬓ buttons) and pick a session.
Splits nest freely, dividers drag to resize, closing a pane never kills the
session.

**Complete, safe themes.** Settings offers Deck Dark (the compatible default),
Light, Follow System, and High Contrast, plus reviewed teal, blue, purple, and
orange accents. A switch updates the whole app, every open xterm pane, ANSI
palette, cursor, selection, focus and tmux copy-mode highlight immediately;
new splits inherit it. Follow System reacts live to macOS appearance changes.
Only the closed presets can be selected—there is no free-form color input—and
the previous palette is restored if settings cannot be saved.

**Board-grouped sidebar.** Sessions in the current project are grouped under
their Board names, in Board order, with counts in each group. Sessions retain
their durable card order within a Board even as live/quiet/stopped status
changes, so navigation never moves merely because a session was opened.

**Command completion, Warp-style.** deck records the commands you run in its
shells (agent prompts are never recorded) and suggests as you type: the first
match appears as gray ghost text at the cursor — **Tab or →** applies it; more
candidates sit in a reserved row below. Only the focused pane gives up that
row, and xterm plus the underlying PTY are refit together.

**Bounded shell recovery.** This opt-in feature is off by default. While a pane
is back at a shell prompt, deck checkpoints its current directory and up to
256 KB of recent plain output after redacting common credential shapes.
After a machine/tmux restart, opening the card starts a new shell in that
directory and places the old text directly in that pane's tmux scrollback,
above a clear restart boundary and the new live prompt. The text is written
only to pane output, never shell input, so commands are not replayed; processes,
jobs, environment variables and agent TUIs are not restored. Snapshots are
private 0600 files, have no backup copy, expire after seven days, and are
cleared when recovery is disabled. Redaction is best-effort, so the consent
prompt still warns that terminal output may contain secrets.

**Scheduled prompts.** The quota-window workflow: queue prompts on a session
and have them typed in later — at a set time ("in 5 h, when my Claude window
resets") or chained ("after the previous one goes quiet for 3 minutes" — quiet
means no output, not that the program is ready). Before delivery deck
resolves the pane the card owns and pins the exact tmux
server/session/window/pane/process generation it just read — a pane that came
back with a new generation (after an update, a crash or a reboot) is adopted
automatically, so a schedule never needs re-pointing. When the
card launch command identifies a program, deck also waits for that executable
to return to the foreground; otherwise it sends to the same pane in
compatibility mode, where input may be interpreted by a shell. Context waiting
does not consume a delivery attempt. Works while detached; dead sessions are
started and probed with a bounded wait; the queue survives restarts. The app
must be running for prompts to fire.

**Recurring rules.** A prompt can repeat — *every 5/15/30 min, 1/2 h* —
optionally only inside a daily time window ("only 09:00–18:00"; 20:00–08:00
wraps midnight, so night-only works too). Rules keep firing until you pause
(⏸ keeps the settings) or remove them, or stop themselves after N times /
at a set time. Outside the window a rule sleeps and resumes by itself.

**Delivery you can reason about.** One prompt per session at a time, at least
a minute apart; different sessions run independently (a session that needs a
startup wait never delays another session's prompt). Immediately before
delivery, deck rechecks the automatically captured target identity and optional
foreground executable. Prompt + Enter are literal-pasted only if both still
match. If deck crashes in the narrow delivery window,
the queue shows the prompt as **ambiguous** instead of claiming success or
silently sending it again: acknowledge it as sent, or explicitly retry while
accepting the possible duplicate. While a prompt is mid-send (a window of
seconds), editing/pausing/removing it is refused with a clear message
instead of racing the delivery. If a step permanently fails to
send (its session can't start, say), the later steps of its group **wait** —
the queue shows ⚠ with retry ↻ and skip ⏭ buttons, and nothing runs past a
failed step until you decide.

**Prompt templates.** Save a queue of prompts as a named, per-project
template (📋 in the scheduler panel). Inserting a template queues all its
steps in order — your schedule applies to the first step, the rest follow
"after previous". Combine with a recurring rule and the whole template
re-runs on cadence.

**Honest signals.** Green = output in the last 15 s. Amber = quiet, may be
waiting for you. Memory chips show the *whole process tree* of a session
(shell + agent + everything it spawned), not just the shell.

## Day-to-day

| Action | How |
| --- | --- |
| New session | ＋ New session → you're in a shell (`$HOME`); recent commands offered as chips |
| Target board for new sessions | click a board's empty area (accent edge marks it) |
| Enter / leave a session | click card · back button (shows the board name) or Esc |
| Move cards | drag & drop (or the board dropdown inside a session) |
| Close | card ✕ / Ctrl+D in shell (instant) · in-session Close (confirms) |
| Copy terminal text | drag directly in the terminal (hold at an edge to cross screens) · ⌘C |
| Rename / describe | double-click titles · right-click card |
| Split view | drag a card onto a pane edge · ⌘D right / ⌘⇧D down |
| Schedule prompts | ⏱ in the session header; 📋 for templates |
| Collapse sidebar | ⌘B |

## Data

Everything lives in `~/.deck/` as plain JSON you can inspect or edit:
`deck.json` (boards, cards, and each live pane's latest directory) · `queue.json` (scheduled prompts, incl. card/tmux
identity, an optional sanitized executable basename, a content-free last
context result, and a short
delivery audit) · `history.json` (command history; wipeable from
Settings) · `shell-state/*.json` (opt-in, redacted, seven-day shell transcript
snapshots; wipeable or disableable from Settings) · `settings.json` (including locale, theme,
accent, shell recovery and update channel) · `app.log` (diagnostics — event codes and
counts only, never what you type; errors appear as categories, never as
raw paths, and session names as a per-run tag rather than the name itself).
Every line is redacted again as it is written—including assignment/JSON/
quoted/ANSI-wrapped paths, URLs and credential shapes—and logs or exports an
older deck left behind are cleaned up in place at first launch.

The whole directory is readable only by you: `~/.deck` is 0700 and every
file — including backups, quarantined corrupt files, logs and exports — is
created 0600 from its first byte; deck re-restricts anything an older
version left more open at every launch.

Every file keeps a `.bak` of its previous good version. If a file is
damaged, deck sets the damaged bytes aside as `<file>.corrupt-<timestamp>`,
restores from the backup, and tells you — it never silently replaces your
data with an empty default. A file written by a NEWER deck (or one whose
version header deck cannot read) is left byte-for-byte alone instead of
being overwritten.

Terminal drag selection has one explicit owner. tmux copy-mode tracks the
anchor and active endpoint while the pointer is down; at pointerup deck freezes
the exact tmux bytes and content coordinates under a generation token. A small
public-geometry overlay then follows those content rows while tmux scrolls the
viewport, so scrolling cannot move the completed range. Holding a drag at the
pane's top or bottom edge continuously crosses screens without leaving the
terminal. ⌘C waits for the freeze and copies only that immutable logical text
through the native macOS clipboard; with no selection it leaves the clipboard
untouched.
Hard newlines and real blank lines are retained, soft wraps are rejoined, and
ANSI drawing sequences are excluded. Each pane keeps a 50,000-row tmux history;
deck reports when that reachable history limit is hit and refuses a clipboard
payload above 64 MiB instead of silently truncating the highlighted selection.

Closing a card — or deleting a project, or letting its shell exit —
permanently cancels every scheduled prompt for that session, and the card
only leaves the board once that cancellation is on disk, its tmux session is
stopped, and the resulting Board is durably saved. A kill or save failure keeps
the cards visible, manageable, and retryable. Nothing deck schedules can
outlive the card it belongs to.

All Board changes share one serial persist-before-commit transaction stream.
A later close, rename, move, project edit, or debounced description is computed
from the latest committed state when its turn begins, so concurrent UI actions
cannot resurrect a card or silently overwrite each other.

Production sessions live on a dedicated tmux socket: `tmux -L deck ls` shows them,
`tmux -L deck attach -t <name>` attaches from any terminal — deck never
touches your personal tmux server. Source builds use `deck-dev`; packaged smoke
tests require a `deck-smoke*` socket, so they cannot attach to production by
accident.

## Building from source

```bash
app/run.sh                                # build + launch the dev bundle
app/run.sh --debug-logging                # add structured verbose diagnostics
cd app/src-tauri && cargo run --example pty_smoke   # headless PTY test
app/src-tauri/binaries/build-tmux.sh      # rebuild the static tmux sidecar
```

Verbose frontend diagnostics are maintainer-only and remain structured and
redacted. For an installed build, quit deck first and launch it with
`open -n /Applications/deck.app --args --debug-logging`; omitting the flag
keeps verbose events off.

Requires a Rust toolchain. The frontend (`app/ui/`) is plain HTML + native
ES modules — no Node runtime, no bundler (Node is used only for dev-time
checks: `node --check`, `node --test app/ui/test/*.test.mjs`,
`node app/ui/js/check.mjs`).

Releases use strict numeric versions. `scripts/release-version set X.Y.Z`
prepares a version commit. The manual `nightly` workflow builds and verifies an
immutable prerelease; the `promote` workflow copies that exact tested DMG and
updater archive into Stable without rebuilding. Directly pushing a strict
`vX.Y.Z` tag remains the emergency source-build path. The hourly resolver only
considers Stable tags and never deletes an incomplete Release. Full procedures
are in [docs/release-channels.md](docs/release-channels.md).

## Feedback

Use the [feedback issue form](https://github.com/c9r-io/deck/issues/new?template=feedback.yml)
to tell us what became easier or what got in your way. GitHub issues are public:
do not include prompts, commands, terminal output, secrets, private paths,
session names, project names, or repository names.

## License

MIT
