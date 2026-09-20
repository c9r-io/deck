# Deck MCP terminal control architecture

Status: accepted MVP ADR, 2026-09-20.

## Decision

Deck uses a thin Rust STDIO MCP adapter, a private in-process Deck control
service, and a signed `deck-mcp-runner` sidecar that is the pane process of
each MCP-managed tmux session.

The adapter owns only MCP framing, strict schemas, annotations, and conversion
to structured results. Deck owns feature enablement, client and project scope,
Board operations, idempotency, control epochs, generations, and the job
ledger. The runner owns one visible session's actual processes, bounded output,
stdin binding, process-group interruption, and exit status. Phone Connector
tokens, routes, kinds, and journals are not accepted by any MCP interface.

## Considered execution mechanisms

### A. Persistent shell integration

A hook-enabled interactive shell could delimit commands and report a marker.
It would preserve `cd`, environment, aliases, and functions. It also makes a
normal program able to imitate the terminal marker, couples correctness to
prompt/hook configuration, and risks modifying user shell startup files.
Reliable stdin and process identity around command completion are difficult.

### B. Managed execution unit in the visible pane (chosen)

The signed runner is the tmux pane process. Each `deck_exec` starts a fresh
`/bin/zsh -d -f` child in that same pane. Script bytes arrive over a 0600 Unix
socket and an inherited pipe, never argv, environment, or a plaintext script
file. stdout/stderr are mirrored to the pane and retained as bounded combined
PTY output; `Child` and its private process group supply exit and interrupt
identity. stdin is a job-owned pipe, so input after exit is refused rather
than falling through to a parent shell.

This preserves visible execution and file changes with a smaller contract than
shell hooks. It intentionally does not preserve `cd`, `export`, aliases, or
functions across calls. Use `cwd` and put dependent commands in one script.

## Board transaction

`deck_session_create` persists an accepted operation first. `ui/js/mcp.js`
claims it and calls `provider.createStarted`, the existing authoritative Board
transaction. The runner/tmux session starts inside `beforePersist`; the card is
then persisted. A failed Board write kills only the session created by that
transaction. Recovery adopts an orphan only when its private runner socket
reports the exact recorded generation; an unrelated same-name tmux session is
never adopted.

Close uses `provider.close`, including scheduler cancellation and the ordinary
Board persistence path. MCP never writes `deck.json`.

## State and fencing

Control operations use `accepted`, `executing`, `committed`, `rejected`, and
`ambiguous`. Jobs separately use `starting`, `running`, `exited`, and `lost`.
Terminal text is context, never completion evidence.

Every write verifies the principal, active grant, project/session scope,
session generation, control epoch, lease, job binding, and current target. A
single delivery fence serializes dispatch with revoke, disable, close,
takeover, and return. Human takeover advances the epoch before enabling Deck
keyboard input. The runner discards ordinary pane input while MCP owns control
and routes MCP input only to the named running child's stdin.

Side-effect request ids are retained in `mcp.json` and are never silently
recycled. Equal request id and arguments return the recorded operation;
different arguments return `REQUEST_ID_CONFLICT`. The fixed ledger bounds are
2,000 operations and 1,000 jobs. Reaching a bound fails closed instead of
evicting replay protection. Scripts and input bytes are not persisted.

After Deck crashes, deterministic Board operations can be reconciled. An
accepted/executing non-Board side effect becomes `ambiguous`; it is never
automatically replayed. Control ownership is cleared and its epoch advanced.

## Security boundary

This is trusted-host execution, not a sandbox. An authorized script has the
macOS account's permissions and may access paths outside its initial cwd.
Canonical root checks prevent accidental selection of another workspace; they
do not create filesystem isolation. Code run by package managers, builds, and
tests is equally privileged. Same-UID malicious code is outside the protection
provided by a 0600 socket and 0600 state file.

The local service accepts only same-effective-UID Unix-socket peers. The
adapter presents a random per-client id created in Deck. It cannot expand its
recorded project roots. Diagnostics record only closed status/error codes;
terminal output, scripts, and stdin are returned through the functional MCP
channel and excluded from `app.log`.

No launchd job, login item, PATH tmux fallback, home-directory executable,
LLM API, coding-agent process, or background model loop is introduced. The
runner and adapter are signed bundle sidecars. Deck must be running for new
control requests; closing the GUI removes the control service, while tmux and
already-started processes may continue.
