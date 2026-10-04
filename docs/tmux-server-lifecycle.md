# tmux server lifecycle

deck treats the tmux server as a persistent process boundary, not as part of
the GUI process. Quitting, hiding, crashing, or reopening the same build must
therefore leave the server and its sessions alone. Installing a different app
build is a separate lifecycle event: a server that continues executing an old
helper also continues carrying that old code identity and must not be reused
indefinitely.

## Root cause

The macOS Tauri updater installs an archive by renaming the running app bundle
to a temporary `tauri_current_app/.../current_app` backup and moving the new
bundle into the original install location. A tmux server that was launched
from the old bundle keeps its already-open executable image after the rename
and after the temporary backup is deleted. Its command line can still contain
the original `/Applications/deck.app` path while the kernel's open image is
the deleted updater backup.

Before this change, every build used the same `tmux -L deck` socket and startup
only re-applied global options. The server carried no creator/build metadata,
so the relaunched app had no way to distinguish the old helper and silently
attached to it. This preserved sessions but also preserved the old helper's
code/signing identity, which is the wrong boundary for upgrades and macOS
Local Network Privacy.

### Restored-pane Local Network attribution

Server identity was not the whole Local Network Privacy failure. After a full
machine restart, both a failing restored shell and a working fresh shell were
created by the same production socket, tmux 3.7c process and bundled image.
Their decisive difference was `pane_start_command`: the restored pane started
the signed `deck-app --deck-shell-bootstrap ...` helper and that helper called
`exec` into the login shell, while the fresh pane was created directly by tmux.
The macOS privacy log then recorded a local-network block for Deck's bundle
identity. An `exec` changes the executable but does not reliably erase the
responsible-code attribution inherited from the signed app, so `kubectl` and
other descendants in only the restored pane failed with `no route to host`.

Shell recovery therefore has a separate process-boundary invariant: Deck may
prepare inert history, but it must never be a pane executable. The current
path sends sanitized bounded bytes to the private tmux server over client
stdin, using one command batch to load a uniquely named buffer, create the pane
as an ordinary clean shell, write that buffer to the new pane's tty with tmux
`save-buffer`, and delete the buffer. No script or shell argv carries the
payload. This preserves the visible history and restart boundary without
replaying commands, placing history in shell stdin or argv, creating a restore
payload file, or putting Deck in the pane's responsible process chain. Fresh
and restored sessions now have the same tmux-to-system-shell trust boundary.

## Identity and compatibility

The authoritative identity is a versioned JSON value in a tmux global server
option. It exists exactly as long as that server and records:

- schema and lifecycle protocol versions;
- product channel and bundle identifier;
- app semantic version and immutable build commit;
- bundled tmux version;
- server creation time and source category (`installed`, `development`, or
  `smoke`).

The app compares that value through one state machine:

- `CompatibleCurrentBuild`: exact current release build and protocol;
- `CompatibleDifferentBuild`: a development rebuild using the same explicit
  lifecycle protocol (avoids restarting for every local compile);
- `RestartRequired`: a different release build, protocol/helper mismatch,
  product mismatch, or a server created from an unacceptable source;
- `LegacyUnknown`: a reachable pre-metadata server;
- `CorruptOrUnreachable`: metadata cannot be decoded or a server cannot be
  inspected reliably.

Stable and Nightly intentionally share the production product identity and
`deck` socket: Nightly promotion copies the exact candidate bytes without a
rebuild. Development uses `deck-dev`; packaged smoke tests use a unique
`deck-smoke-*` socket and separate bundle identifier. Changing compatibility
semantics requires incrementing the lifecycle protocol, independently of the
marketing version or Mach-O UUID.

## Boot and update behavior

Boot inspects the server before any session start or attach. An exact current
server is reused without changing its PID. An incompatible, legacy, or corrupt
empty server is replaced automatically. A server with any user session is
left running and recorded as pending until the user explicitly confirms the
destructive restart. Choosing “later” is persisted per current build so normal
refreshes and relaunches keep a discoverable status without repeating the
modal.

Production builds may create a server only when the running app bundle is in a
stable Applications location. A process launched from an updater temporary
directory, DMG, App Translocation, or another transient location can inspect
an existing server but cannot become the creator of a new long-lived one.

## Board query channel

The Board's high-frequency pane inventory uses one serialized tmux control
client after an initial one-shot discovery. The client attaches with
`ignore-size,no-output` and receives only Deck's compiled snapshot — one
`list-panes -a -F` query plus one `display-message -p` server line (server
PID, start time and the shell-exit ledger), each framed by the query's nonce;
user, project, session and prompt values can never become control
commands. Replies are correlated by tmux command ID and have a 1.5 second
deadline plus a 2 MiB output bound. Malformed, nested, mismatched, failed or
exited frames destroy the channel.

The client is deliberately not read-only. tmux gives every one-shot command
an ambient target client (the most recently active one), and while no pane is
attached that is this client; `send-keys` without `-X` refuses when its
target client is read-only. A `-r` query client made every launch command and
delivery Enter fail whenever the Board showed no terminal (0.7.1–0.7.6).
Its argv, and a visible pane's PTY attach argv, live in `tmux_clients.rs`;
`tests/tmux_contract.rs` attaches exactly those clients and runs every
client-sensitive contract with no client, with the query client, and with
the query client plus a pane client.

A query client never outlives Deck by design, but tmux can strand one: a
control client exits only after the server has delivered its pending output,
and once Deck is gone that output (notifications `no-output` does not
suppress) can never be delivered. The stranded client stays connected to the
server, drops out of `list-clients`, and waits forever. Two layers prevent
it. SIGTERM, SIGINT and SIGHUP take the normal quit path, which closes the
client's stdin and reaps it (`exit_on_termination_signals`). For what cannot
be caught (SIGKILL, a crash), every new channel first SIGTERMs orphans (ppid
1, same uid) whose exec path and argv are exactly this bundle's tmux with
this socket's query-client arguments (`reap_orphaned_query_clients`, via
libproc and `kill(2)`; nothing is spawned).

Discovery and the first failure of a channel generation use the existing
one-shot query as an oracle. The client is attached to one session, the
smallest name at discovery, and tmux ends it with that session
(`detach-on-destroy`): closing that card, or its shell exiting, fails the
channel while the server is fine. A generation that served ten seconds and
whose oracle read succeeds is therefore replaced by the next poll's
discovery, and no poll fails. A generation that fails sooner, or whose
server the oracle cannot read, is followed by a ten-second cooldown that
fails closed instead of creating a new process on every Board poll. Either
way a failed channel is never replaced within ten seconds of its own attach
(`tmux::query_snapshot_with`). No sentinel session is created:
an empty server has no persistent query client. On such a server `list-panes
-a` fails with exactly `no current target`; only then one more command list —
the server line followed by `list-sessions` — may prove a reachable server
with zero sessions, and the poll reads an empty snapshot that still carries
the server identity and exit ledger (`tmux::snapshot_or_empty_with`, the
`probe_server_on` emptiness criterion read atomically). No server, a session
in the proof, or a missing, malformed or foreign-nonce server line keeps the
original failure. Every write, PTY, stdin,
session mutation and lifecycle operation remains on the existing one-shot
path.

A control client increments tmux's attached-client count. Deck therefore
remembers its child PID, server PID and attached session, then subtracts one
client from lifecycle impact only after `list-clients` independently confirms
the exact PID, control mode, no-output/ignore-size flags and session
on the same server generation. Missing, duplicate or malformed evidence fails
closed. A server with no sessions has nothing to subtract from, so the probe
does not ask: the remembered client exits with the last session but its
record lasts until the channel is next polled, and tmux answers
`list-clients` on an empty server with an error ("no current target") that
would otherwise read as an unreachable server and refuse every new session.
The channel is stopped before a server restart and on app exit.

## Shell exit evidence

Exit evidence is not absence. Without it, a shell ending and a session lost
to `kill-session`, a crash or a replaced server look identical to Deck: the
name is missing from a successful listing (`alive=false`), and the attach
client's `pty-exit` and tmux's `[exited]` text are the same for a shell exit
and an external `kill-session`. So neither is ever retirement authority;
`pty-exit` only wakes the poll.

The evidence is recorded by tmux at the moment the pane's process ends. Every
Deck server (`tmux.conf` and the reuse defaults) installs a global
`pane-died` hook and then sets `remain-on-exit on` — hook first, so a server
never keeps a dead pane nothing removes. The hook, in one server command
list, appends `x1|pid|start_time|$session|%pane|window_panes|session_windows|
status|signal;` to the server option `@deck_exits` and `kill-pane`s the dead
pane, so the session is destroyed exactly as before. tmux runs the
notification queue before any client command in the same server loop, so no
client — listing, delivery, input — ever sees the dead pane; the literal
delivery guard also requires `pane_dead == 0`.

The ledger is append-only and bounded where it is written (the hook keeps the
last 4096 characters and appends). Deck never writes it, so a concurrent
append cannot be erased by a reader; overflow drops the oldest records and at
most truncates one, which the strict parser rejects. `kill-session` and
`kill-server` run no hook and record nothing.

`poll_sessions` classifies each card session as Alive, ExitedNormally or
Missing (a failed listing is Unavailable). ExitedNormally requires a record
matching the identity Deck last observed alive in this process — server PID
and start time, session id, one of its pane ids — for the session's last pane
(`window_panes = 1`, `session_windows = 1`), with an exit status and no
signal, for a session no MCP runner owns. A reused name replaces the
identity; a Deck restart starts with none; the restart transaction forgets
all identities before it sends its first key. Everything else is Missing and
only marks the card stopped. The last session ending leaves an empty server;
the poll still reads its ledger through the proven empty snapshot (Board query
channel above), so that exit is verified too. Contract: `src/shell_exit.rs`,
`tests/tmux_contract.rs` (real-tmux matrix, bound, concurrent reads and
dead-pane observability).

## Restart transaction and recovery

The backend owns one serialized restart operation:

1. snapshot server PID, sessions, panes, attached clients, activity and
   foreground-process presence;
2. after UI confirmation, lock session creation/updater installation and
   re-check PID, server start time, session and pane counts; a reachable server
   with zero sessions has an empty pane set even though tmux's `list-panes -a`
   reports `no current target`. Deck accepts that response only after another
   probe confirms the same server is still reachable and empty;
3. persist a content-free restart intent (written once, before the stop;
   no progress phase is recorded — recovery never needs one);
4. request `kill-server`, wait for exit, and remove only a validated stale
   socket belonging to this deck socket name;
5. start the server with the current bundled helper and write metadata;
6. read back PID and metadata, require a new PID and current compatibility;
7. clear the intent and pending marker.

The persisted intent contains counts, identity, PID/start time and the old
socket device/inode only—never a socket/project path, session name, command,
terminal output or prompt. Device/inode is used only to prove that a residual
socket is the one captured before `kill-server`; it is not a build identity or
compatibility input. If deck stops at any point after the intent was written,
the next boot resumes only when the observed old PID and start time still
match the confirmed transaction (and probes what is actually running); it does
not need to know which step was reached. Files written by earlier builds also
carry a `phase` key, which is ignored.
A different unexpected server or socket is preserved and returned to the
pending/diagnostic path.

The frontend marks ordinary cards stopped after replacement succeeds, or after
an error when a fresh status shows the server identity changed (or replacement
began and status cannot be read), and before resuming polling. A refused
restart leaves their live presentation intact. Intentional server replacement
cannot be mistaken for individual shell exits and delete cards: the old
server's exit evidence dies with it, identities are forgotten when the
transaction starts, and a fresh server can never match an old identity. Cards, boards,
queue records, and bounded shell
snapshots remain; Unix processes inside the old tmux server do not migrate and
are described honestly as terminated.

MCP-managed runners cannot use ordinary shell restoration. Restart status lists
their blocking card and session identifiers before the user confirms. Deck can
close these cards through the normal local Board close path, then verifies each
close and refreshes server impact before attempting the existing restart. Active
managed jobs stop with their cards. Control leases, execution grants, runner
authentication and job bindings are not restored. A failed or uncertain close,
new blocker, or unexpected server identity change stops the transaction. The
backend still checks the durable MCP ledger under the lifecycle gate immediately
before any server replacement side effect.

## Security and privacy boundary

Lifecycle logs use closed event/reason codes plus counts and PIDs. They never
include session names, commands, pane text, prompt data, or user paths. The
Local Network usage string says that terminal tools may access services the
user chooses; deck does not claim to scan the network and does not reset or
bypass macOS privacy controls. Signed updater tests must verify both the new
server metadata and the helper's kernel-loaded image after replacement.
