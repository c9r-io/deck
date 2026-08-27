# deck

A kanban board for terminal agent sessions — Claude Code, Codex, or any shell process.

Each card on the board **is** a tmux session. deck is deliberately not a terminal or a
session manager itself: tmux owns the shells, their persistence, and their scrollback.
deck is a *projection* — a project-management view over those sessions, with the extra
metadata (title, stage, notes) that a raw session list can't hold.

## The app (v0.2 — macOS)

`app/` is the native macOS app: Tauri 2, with the no-build-step static frontend in
`app/ui/` (vendored xterm.js, no bundler) and the Rust backend in `app/src-tauri/`.
Because tmux owns the sessions, **agents keep running when the app quits** and are
all still there when it reopens.

```bash
app/run.sh          # build + launch as a .app via LaunchServices
```

(Launching the bare binary from a background shell leaves keyboard input dead —
macOS text-input services are only reachable from the GUI login session.)

The backend exposes: board persistence (`~/.deck/deck.json`), a poll endpoint
(liveness + output recency + process-tree memory + tail previews in one call),
a PTY bridge (`tmux attach` for the one open session, streamed to xterm.js),
`open`-based path/URL actions, and command history (deck-launched commands +
the user's shell history). Status semantics are honest: green = output in the
last 15s, amber = quiet (may be waiting for input), gray = no session.

Creating a session has no form: "＋ New session" drops you straight into a
fresh shell in `$HOME` ("＋ Here" uses the current card's directory). Type the
command yourself, or click one of the recent-command chips offered above a
fresh shell; rename the card later (double-click the title anywhere). Click a
board's title to select it — an accent edge marks it, and new sessions land
there instead of the default board.

**Scheduled prompts** (⏰ in the session header): queue prompts to be typed
into a session later — built for the agent rate-limit workflow ("when my
quota window resets in 5 h, run these tasks in order"). Each entry fires
either at a set time or after the previous one finishes (session quiet for
3 minutes). Works while detached; sessions are started automatically if
needed; the queue survives app restarts (`~/.deck/queue.json`). The app must
be running for prompts to fire.

```
┌ Backlog ────────┐┌ Active ─────────┐┌ Waiting ────────┐┌ Review ─────────┐┌ Done ───────────┐
│○ refactor auth  ││● fix flaky test ││● migrate schema ││○ PR #42         ││○ docs pass      │
│                 ││● triage crash   ││                 ││                 ││                 │
└─────────────────┘└─────────────────┘└─────────────────┘└─────────────────┘└─────────────────┘
┌ detail ─────────────────────────────────────────────────────────────────────────────────────┐
│fix flaky test   running   ~/c9r-io/orchestrator  $ claude                                    │
│─ live output ─                                                                              │
│  ⏺ Running cargo test --workspace ...                                                       │
└─────────────────────────────────────────────────────────────────────────────────────────────┘
```

## Download

Grab the latest `.dmg` from [Releases](https://github.com/c9r-io/deck/releases)
(macOS, Apple Silicon; signed and notarized). Tagging `vX.Y.Z` builds and
publishes a release automatically, and installed apps self-update in-app.

**The only runtime requirement is tmux** (`brew install tmux`) — it is the
session backend that keeps your agents alive across app restarts. No Rust,
no Node, nothing else. deck finds tmux in the usual Homebrew/MacPorts
locations even when launched from Finder.

## Building from source

- Rust toolchain
- `app/run.sh` builds and launches the dev bundle

## Install

```bash
cargo install --path .
```

## Usage

Run `deck`. Cards live in five columns: Backlog → Active → Waiting → Review → Done.

Creating a card asks for a title, a command (default `claude`), and a working
directory. The card starts in Backlog with no session. Starting it creates a
detached tmux session in that directory, running a normal shell with the command
typed in — so when the agent exits, the shell and scrollback survive.

| Key | Action |
| --- | --- |
| `h j k l` / arrows | move selection |
| `[` / `]` (or `H`/`L`) | move card to previous / next column |
| `J` / `K` | reorder card within its column |
| `n` | new card |
| `Enter` | attach to the card's session (starts it first if needed) |
| `s` | start the session detached, without attaching |
| `x` | kill the card's session (card stays) |
| `d` | delete card (kills its session; card is archived in board.json) |
| `o` | open the card's notes file in `$EDITOR` |
| `e` / `c` | edit title / command |
| `r` | refresh now |
| `q` | quit |

Detach from an attached session with the normal tmux binding (`Ctrl-b d`) to get
back to the board. When deck itself runs inside tmux, attaching uses
`switch-client`, so the board keeps running in its own session.

## State

Everything lives in `~/.deck/`:

- `board.json` — cards, columns, archive
- `notes/<card-id>.md` — free-form notes per card

Both are plain files; edit them by hand or with an agent if you like.

## GUI prototype

`gui/index.html` is a self-contained, mock-only GUI prototype (no backend, no build
step — open it directly in a browser). It exists to settle the interaction design
before wiring: project tabs (each project has its own set of boards, addable /
renamable / deletable), a collapsible sidebar session list (⌘B), a kanban board
with drag & drop and right-click card actions, a full session view with a fake
streaming terminal (detected file paths / URLs are clickable), per-session
process-tree memory chips, and a new-session modal with "open here" prefill.
Stop and delete are one operation ("Close"): removing a card terminates its shell.

Default boards express attention, not workflow stage: Attention / Working /
Queued / Parked — no "Done". Placement is always manual: the board's job is to
surface information (status dots, amber highlights on sessions waiting for
input) and lower the user's decision cost, never to move cards on its own.
Custom task-style boards remain fully supported. The `MockProvider` object at
the top of the script is the seam where a real backend (WebSocket + PTY, or a
tmux bridge) plugs in.

## Non-goals (v1)

- Its own terminal emulator, panes, or scrollback — that's tmux's job
- Multi-machine / remote sessions
- Automatic agent-state detection beyond alive/dead + live pane tail
