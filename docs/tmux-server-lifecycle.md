# tmux server lifecycle

deck treats the tmux server as a persistent process boundary, not as part of
the GUI process. Quitting, hiding, crashing, or reopening the same build must
therefore leave the server and its sessions alone. Installing a different app
build is a separate lifecycle event: a server that continues executing an old
helper also continues carrying that old code identity and must not be reused
indefinitely. One exit stops a server: a launch that only delivered a
notification answer gives back the empty server it started itself (see Boot
and update behavior).

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

Boot runs before a launch can know what it is, so a launch the system makes
only to deliver a notification answer (the response-only launch,
`docs/card-reminders.md`) starts a server like any other when none exists.
That launch ends within a second and never has a session. On its way out it
stops the server its own boot started, so a “not now” on a notification
leaves no process behind that was not there before. The stop is one tmux
command that decides inside the server (`if-shell -F` on the PID, the start
time and a session count of zero, then `kill-server`); the socket file tmux
leaves is then removed under the same device/inode check a restart uses
(`tmux_lifecycle::retire_boot_server`). A server that was already running, a
server with a session, and every other exit are untouched, and a stop that
fails leaves the server running. The launch still runs the boot inspection
and the webview's start-up tmux queries before it knows it is response-only.

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

Every Deck server also installs a global `alert-bell` hook (`bell.rs`), by
the same two routes. Its whole body is one `set-option` that appends
`b1|$session|@window|%pane|time;` to the server option `@deck_bells`, bounded
where it is written like the exit ledger and read on the same server line of
the Board poll's snapshot. It starts no process and removes nothing. It is an
attention observation only (a program rang the terminal bell; see
`docs/notifications.md`) and never evidence for retirement or delivery. The
hook is additive: an older Deck reusing the server never reads the option, so
the server protocol version is unchanged.

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

## Reopen Agent session (optional post phase of an explicit restart)

**Status: implemented and enabled in code.** Ordinary tests use a stand-in
client signed ad hoc as a test product.

**No release is named.** A client is admitted by its code identity, not by
a list: the adapter states Claude Code's code requirement (Anthropic's
signing identity, no release, version or digest), and Security.framework
validates it inside Deck's process — `codesign`, `spctl` and the client's
own `--help`/`--version` are never run — in two different ways that are not
merged. On DISK, the file's signature is validated in full, every page of
the executable included. For the RUNNING image, the process is validated
dynamically against that file's code directory: the kernel's record of what
the process executes must be what the file holds. A client counts only
while its exact generation is bound to the frozen reference — its pid and
birth instant, the kernel's image path, the file's state and that dynamic
validation, read before and after. That is what refuses an old process
whose path already holds another validly signed build, and an exec into
another image under the same pid and birth instant (which the birth instant
does not notice). The file's SHA-256 is this transaction's proof that the
content stayed the same; it is compared only with itself and never looked
up. So a release this build has never seen is admitted on the same facts as
any other, under either policy. A change of the signing identity or of the
narrow native contract (the resume flag, the bypass flag before it, the
session id and the start source / end reason of the lifecycle hooks, hooks
run as the client's direct child) is an adapter change; a release that kept
those shapes and changed their meaning cannot be told apart by any
interface this feature may use — an accepted residual, not a reason to list
releases again.

The reads and validations cannot be interrupted — the dynamic validation
of a running image included. Three rules hold for each of them, on every
path:

- **An owner.** Each runs under a deadline its caller owns: the preview's
  ten-second share, the restart's preparation, a lifecycle receipt's 400 ms,
  the post share (asked again after the re-read, before a shell is created)
  and the confirmation's own one-second share after it. A dynamic
  validation nobody owns is not made.
- **A late answer is no answer.** The deadline is asked after the call
  returned, and a positive answer that arrived late is dropped like any
  other: no candidate, nothing frozen (the restart ends as its own timeout
  before any exit key), no receipt recorded, no entry confirmed. What was
  read before such a call — the process table, the pane's foreground — is
  read again after it, before anything is committed.
- **The caller is answered anyway.** A deadline does not shorten the call,
  so each caller stops waiting through a worker: the preview on one worker
  that holds the preview's turn while it reads and nothing else; the restart
  on its worker,
  which keeps the operation gate, the activity guard and the plan; the
  receipts on ONE receipt worker beside the status listener, behind a
  bounded queue — the listener itself, every agent's Signal path, never
  makes or awaits that call. A stalled thread keeps what it owns until its
  call returns and its answer then reaches nobody; nothing is retried and no
  second worker starts beside it.

Three things are each fixed to one instant, so that an answer that is late
for its own boundary never arrives at the next one:

- **A receipt belongs to one operation and one arming.** A lifecycle line is
  a claim on the operation in flight when it was read. Where the fact is
  written — under the mailbox lock, not only when the claim was queued — the
  mailbox must be that very operation (a number this process gives each
  operation and never uses again), the lease that very arming, and the time
  inside both the receipt's own 400 ms and the phase the lease was armed
  for. A claim that outlived its operation in the queue or inside its
  validation is refused there even when the next operation armed the same
  card, pane, shell and client. Nothing of this is written to disk.
- **The receipts end at the post deadline.** A launch lease stops taking
  facts at that instant by the clock, sealed or not, and every entry is
  sealed together before the first check of a started client. The
  confirmation's one-second share is time for those checks alone: it never
  lengthens the time a receipt may arrive in, and an entry is confirmed from
  the sealed set. A receipt that comes later — also a conflicting one —
  changes nothing that was settled and leads to no second request.
- **A preview is published under its turn.** One preview reads at a time,
  and what it read becomes the preview that can be confirmed at one instant:
  one write, made by the caller that was answered in time, while it still
  holds the turn it read under. So previews are published in the order they
  were read, and a result that was read earlier never replaces a later one.
  A caller waits for the turn no longer than its own ten seconds, counted
  from when it asked, however many previews follow each other; a worker that
  outlived its caller publishes nothing and gives back only its own turn.

A check that did not finish is reported as `check-incomplete` before a
launch and `context-unverified` after one — never as a candidate, a
confirmation or a changed program (`executable-unavailable` and
`context-changed` are the positive findings). A validation that never
returns therefore costs one thread and, for a restart in flight, its gate
and guard until the process ends: no guard is released under a running
operation. The cost on the 240 MB client, measured in the release profile
with the file in the page cache: about 0.42 s for the digest, 0.09 s for the
full signature validation, and under 2 ms for each dynamic binding (3.5 ms
for the first one this process makes). A debug build needs its SHA-256
dependency optimized (`[profile.dev.package.sha2]` in `Cargo.toml`) to stay
inside the post phase, and then measures the same.

Evidence for the identity checks: tests on real processes with ad-hoc
signed fixtures (the mechanism), and one explicit read-only run of the
production requirement against three installed releases — one of them
never listed anywhere — two other Developer ID programs and one running
client. Representative P3 validation of the new mechanism completed on
2026-10-11 with real Claude 2.1.295, whose digest was never in that list:

| Policy | Test controller calling the production transaction | Isolated real application, UI and IPC |
| --- | --- | --- |
| A | PASS: one transaction, two cards, no bypass override | PASS: one transaction, one card, no bypass override |
| B | PASS: one transaction, three cards, two bypass overrides | PASS: one transaction, three cards, two bypass overrides |

Both B samples included a bypass launch lowered to plan before the restart.
The application used an ad-hoc debug carrier, private data/socket, real HOME
and project lifecycle hooks. Its earlier B preparation stopped on a CUA
timeout before any Claude launch or restart; that record is retained.
Requests and new generations were sampled independently; exact final-session
receipt attribution also relies on the production confirmation checks, not
an exported wire trace. Native permission indicators are operator observations,
not Deck's independent verification of effective permissions.

P3 is complete with evidence limits (PASS WITH OPEN QUESTIONS). macOS 26
remains UNRUN. The application samples had empty queues; the controller's
nonempty paused-queue evidence is separate. Stale terminal frames and an
invalid independent policy-persistence probe are recorded, not counted as
passes. Existing automated tests cover resetting the policy for a new preview.
The 2026-10-11 close-out added, on a private Developer-ID-signed release
carrier and a debug carrier under a relocated home: the launch-time hook
migration (an install moved to another location is rewritten, a user with
the status hooks alone is not enrolled, a user's own hooks are untouched,
nothing happens away from a stable location) and both Settings switches in
the real window. Its follow-up ran the transaction itself in a real window
with real Claude Code 2.1.296 clients, in a debug carrier at a stable
location under a relocated home with that home's hook file: Policy B, the
review after it, a relaunch, Policy A, a queued prompt that stayed paused,
and the session after `/clear`. The final round of that close-out ran the
transaction once more in a Developer-ID-signed release carrier at the same
kind of location, with a person operating its window: Policy B, then the
ordinary policy after the application had been started again. Still open,
each on its own: anything at `/Applications/deck.app`, where nothing ran at
all; the Settings switches in a release window; an unknown or unconfirmed
outcome in a real application; notarization and Gatekeeper acceptance; and
on macOS 26, native identity of a real client and a transaction in a real
window (the automated gate ran there in CI, and closes only what its jobs
executed). See the
[acceptance summary](evidence/closeout-20261011/reentry-acceptance.md)
for the matrix, source provenance and the limits of this validation.
This is representative validation, not a version allowlist or release approval.

The native evidence that follows is historical. It was gathered while
admission was still a list of release digests, each stage run once by a
test controller that calls the production transaction, and it says nothing
about the identity checks. On the real Claude 2.1.294:
how the production checks read a bare and a bypass-flag launch (stages S and
B), and ONE Policy A restart transaction over two such clients (stage M1: both
entries confirmed). On the real Claude 2.1.296: the same launch readings, ONE
Policy A transaction (M1, with a `/clear`), ONE Policy B transaction over three
clients (M2: the two bypass launches asked with the override, the bare one
without it, all confirmed; the client lowered to plan mode before the restart
showed bypass after it, as read from its pane by the experiment's operator)
and ONE transaction whose new shell exported a refused name (M3: one request,
not confirmed, the client kept). Local isolated application validation on
2.1.296 passed in follow-up 33: real UI → IPC → production assembly → restart
→ real Claude/helper → post-check → UI result, ONE successful Policy A
transaction. It used an ad-hoc debug carrier, private data/socket, real HOME
and an explicitly validated project lifecycle configuration. Only the debug
SHA-256 dependency was optimized; deadlines and identity checks were unchanged.
Not verified: formal release installation and global hook opt-in/migration,
Policy B and the refused-name path on 2.1.294, anything Deck itself knows of the
effective permission after a re-entry, Developer ID, notarization, Gatekeeper
and the published carrier. The WKWebView
`reentry` smoke paints the dialog from scripted answers. As an ad-hoc signed
debug carrier on the test machine it failed its layout checkpoint twice (the
dialog's checkboxes and radios took the general `input` width, and the
section overflowed sideways) and, with that rule repaired, passed once: 15 of 15
checkpoints, English and Chinese, the default 1280×800 window. One passing
run says nothing about other window sizes, the larger font scale, the signed
carrier or the transaction itself. A further run on 2026-10-11 passed 15/15
with the current A/B result wording; see
[the acceptance summary](evidence/closeout-20261011/reentry-acceptance.md#the-dialog-in-the-real-window).
The module header of `agent_reentry/claude.rs` retains the historical native
campaigns; the acceptance summary above records the current P3 evidence.

**Launch context: limited checks before, confirmation after** (the user's
ruling). The environment a new shell's startup files leave behind cannot be
read before the request is typed, by any interface Deck may use. So:

- *Before the one request* a bounded set of conditions must hold: the exact
  card, pane, server, pid and birth; the frozen executable and a working
  directory that still exists; the old client gone; the ticked cards and the
  chosen policy as bound now; a supported launch profile of the OLD client;
  the exact input edge of the new shell; and a supported baseline — what Deck
  hands its server and the one home variable tmux gives the pane. That
  variable is read by name and answers one of four things (set, removed,
  absent, unavailable); the global scope is asked only when the session scope
  positively does not mention it, and a removal, a failure, a timeout or an
  ambiguous answer refuses the launch.
- *The accepted window*: the startup files may have changed the environment,
  and the request may already have run in it — with native effects — when
  that is found out. Nothing is typed to probe it, no environment is copied
  or assigned on the command line, and no launcher is added.
- *After the launch* an entry is confirmed only when the receipt arrived, the
  reporting client is still the pane's foreground generation, and that exact
  process executes the frozen file, was started with exactly the request this
  operation built, in a launch environment with the expected home and no
  refused name, in the frozen directory. The image and the directory are
  facts of the instant they are read; arguments and environment are the
  launch image the kernel kept from the exec. None of this is evidence of the
  configuration the client loaded or of its effective permission.

A missing client, an unreadable launch image, a mismatch, or a missing or
conflicting receipt is `unconfirmed` with its own reason
(`launch-unconfirmed`, `context-unverified`, `context-changed`). That is a
statement about confirmation only — not that nothing happened and not a
rollback. The started client is left for the user: not killed, not typed
into again, not retried. The server replacement stays settled.

A restart the user confirms may also reopen selected Claude sessions. This is
an addition to the transaction above, never a change of it:

1. **Preview** (`agent_reentry_preview`, read-only): every live card session
   is classified from its live foreground process — a shell, an unsupported
   foreground, or a candidate (a direct client whose running code is the
   product's and is bound to its file, in a one-window, one-pane session,
   with its launch profile `bypass` or `ordinary`). Clients that share a
   binary read and validate it once; each is bound to it on its own.
   Candidates are unticked by default; the default policy is A. The reply
   carries card id, session, closed codes and a single-use token; no native
   id, pid, path, digest, signer or release.
2. **Freeze** (inside the restart's guards, before any exit key): the token
   is consumed and each ticked card is read again. A card, pane, server,
   foreground generation, executable or working directory that is not the
   previewed one refuses the whole restart as `tmux-server-impact-changed`;
   an approval is never rebound, and an executable that changed is never
   frozen again under it. The previewed reference is reused only for that
   exact file state; each client is bound to it again.
3. **Exit**: the frozen panes take the existing graceful-exit path and its
   shared two seconds, including a client that runs under its release's
   file name. An entry continues only with BOTH its final receipt
   (`SessionEnd`, `prompt-input-exit`, attributed by the kernel to that exact
   client while it still executes the frozen executable) and its old client
   generation gone. A missing receipt skips that
   card. A client that does not exit — including one that gave the terminal
   back to its shell but is still alive, or whose exit cannot be proven — is
   the existing `AgentTimeout` for the whole restart: no pause, no kill.
4. **Replacement**: unchanged, including the durable queue pause before the
   kill and the content-free intent. Its read-back settles "the shell service
   restarted" and nothing later rewrites that.
5. **Post phase** (one shared share, an implementation default to be
   validated, not a per-card budget or a promise): for each sealed entry a
   command-less shell is created in the client's working directory with the
   ordinary transcript restore; its input edge is positively verified; then,
   under the Board writer's fence, the card's binding is read again, the
   one launch slot is consumed and one request is pasted and submitted, with
   the exact shell generation asked of the kernel at the paste and again at
   the Enter; a
   `SessionStart` receipt with source `resume` and the same id from the new
   foreground client, recorded before this share ended, together with the
   post-launch check of that client described above, confirms the entry.
   Everything else is a per-card
   result: not attempted, refused before launch, known not launched, or
   unconfirmed. Nothing is retried.

Policy A adds nothing to `--resume <id>`. Policy B is that restart's explicit
reauthorization: the bypass flag is added only for ticked cards whose client
was shown as a bypass launch and is read as one again at freeze. It applies
again even if the user lowered permissions while the session ran — the dialog
says so before confirmation — and it is not stored. Both policies rest on
the same identity and contract facts: there is no list of releases for the
override either, and the client's last permission mode is read by nothing.
A confirmed Policy B entry says that the session was re-entered and that
the bypass flag was sent as authorized; Deck does not read the permission
in effect and says so. An entry that is not confirmed says only that
re-entry was attempted.

What it is not: the task is not resumed by Deck, no prompt is sent, model and
added directories are left to the Agent, queues stay paused, no readiness,
Signal or authority is created, no card moves, and a client the user opened
elsewhere with the same session is outside every check. After a crash nothing
re-enters: the plan, the ids and the launch slots were process memory.

## Security and privacy boundary of re-entry

The lifecycle helper is a second bundled sidecar with its own closed wire; it
forwards a native session id and one closed word, never a payload, prompt,
path or model, and logs nothing. The host attributes a receipt by kernel
facts while the helper is connected and keeps it only for the armed lease of
the running operation. Logs and replies carry closed codes, counts and the
existing hashed session tag. The executable and working-directory references
never leave the process, and the started client's arguments and environment
are compared inside `procinfo` against Deck's own expectations: only a closed
answer leaves it. The card binding is read from the Board this process
committed last as authority (`documents::board_authority`), under the Board
writer's fence; a Board answered from a backup names no card. Same-user code inside the client's own process tree
can still misreport; that residual is accepted for this one user-confirmed
side effect and extends to nothing else.
