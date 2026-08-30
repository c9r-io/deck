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

## Restart transaction and recovery

The backend owns one serialized restart operation:

1. snapshot server PID, sessions, panes, attached clients, activity and
   foreground-process presence;
2. after UI confirmation, lock session creation/updater installation and
   re-check PID, server start time, session and pane counts;
3. persist a content-free restart intent and phase;
4. request `kill-server`, wait for exit, and remove only a validated stale
   socket belonging to this deck socket name;
5. start the server with the current bundled helper and write metadata;
6. read back PID and metadata, require a new PID and current compatibility;
7. clear the intent and pending marker.

The persisted intent contains counts, identity, PID/start time and the old
socket device/inode only—never a socket/project path, session name, command,
terminal output or prompt. Device/inode is used only to prove that a residual
socket is the one captured before `kill-server`; it is not a build identity or
compatibility input. If deck stops between phases, the next boot resumes only
when the observed old PID and start time still match the confirmed transaction.
A different unexpected server or socket is preserved and returned to the
pending/diagnostic path.

The frontend detaches PTY clients and marks cards stopped before resuming
polling, so intentional server replacement cannot be mistaken for individual
shell exits and delete cards. Cards, boards, queue records, and bounded shell
snapshots remain; Unix processes inside the old tmux server do not migrate and
are described honestly as terminated.

## Security and privacy boundary

Lifecycle logs use closed event/reason codes plus counts and PIDs. They never
include session names, commands, pane text, prompt data, or user paths. The
Local Network usage string says that terminal tools may access services the
user chooses; deck does not claim to scan the network and does not reset or
bypass macOS privacy controls. Signed updater tests must verify both the new
server metadata and the helper's kernel-loaded image after replacement.
