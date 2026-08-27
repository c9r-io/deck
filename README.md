# deck

**A native macOS command center for terminal agent sessions** — Claude Code,
Codex, or any long-running CLI. Every card on the board is a real, persistent
terminal session; the board tells you where your attention is needed.

```
┌ Attention ──────┐┌ Working ─────────┐┌ Queued ─────────┐┌ Parked ─────────┐
│● fix flaky test ││● refactor auth   ││○ migration plan ││○ changelog pass │
│  ⧗ waiting for  ││  ⏺ cargo test …  ││                 ││                 │
│    your input   ││            812M  ││                 ││                 │
└─────────────────┘└──────────────────┘└─────────────────┘└─────────────────┘
```

## Install

Download the latest `.dmg` from [Releases](https://github.com/c9r-io/deck/releases),
drag to Applications, open. That's it:

- **Zero dependencies** — a statically linked tmux ships inside the app
- **Signed & notarized** — no Gatekeeper prompts
- **Self-updating** — new versions appear as a button in the sidebar

Apple Silicon only for now.

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

**A real terminal.** Full xterm with truecolor, ⌘C/⌘V, and clickable file
paths / URLs (open in editor, reveal in Finder, open in browser).

**Split view.** Watch several agents at once: drag a card from the sidebar
onto a pane edge, or hit ⌘D / ⌘⇧D (or the ◧ ⬓ buttons) and pick a session.
Splits nest freely, dividers drag to resize, closing a pane never kills the
session.

**Command completion, Warp-style.** deck records the commands you run in its
shells (agent prompts are never recorded) and suggests as you type: the first
match appears as gray ghost text at the cursor — **Tab or →** applies it; more
candidates sit in a bar below.

**Scheduled prompts.** The quota-window workflow: queue prompts on a session
and have them typed in later — at a set time ("in 5 h, when my Claude window
resets") or chained ("after the previous one goes quiet for 3 minutes" — quiet
means no output; it may also be a prompt waiting for you). Works while detached; dead sessions are started automatically; the
queue survives restarts. The app must be running for prompts to fire.

**Recurring rules.** A prompt can repeat — *every 5/15/30 min, 1/2 h* —
optionally only inside a daily time window ("only 09:00–18:00"; 20:00–08:00
wraps midnight, so night-only works too). Rules keep firing until you pause
(⏸ keeps the settings) or remove them, or stop themselves after N times /
at a set time. Outside the window a rule sleeps and resumes by itself.

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
| Rename / describe | double-click titles · right-click card |
| Split view | drag a card onto a pane edge · ⌘D right / ⌘⇧D down |
| Schedule prompts | ⏱ in the session header; 📋 for templates |
| Collapse sidebar | ⌘B |

## Data

Everything lives in `~/.deck/` as plain JSON you can inspect or edit:
`deck.json` (boards & cards) · `queue.json` (scheduled prompts) ·
`history.json` (command history) · `app.log` (diagnostics).

Sessions live on a dedicated tmux socket: `tmux -L deck ls` shows them,
`tmux -L deck attach -t <name>` attaches from any terminal — deck never
touches your personal tmux server.

## Building from source

```bash
app/run.sh                                # build + launch the dev bundle
cd app/src-tauri && cargo run --example pty_smoke   # headless PTY test
app/src-tauri/binaries/build-tmux.sh      # rebuild the static tmux sidecar
```

Requires a Rust toolchain. The frontend (`app/ui/`) is a single static
HTML file — no Node, no bundler.

Releases: push a `v*` tag — CI builds, signs, notarizes, and publishes the
dmg plus in-app-update artifacts. An hourly scheduled run rebuilds the newest
tag if a release is missing or incomplete.

## License

MIT
