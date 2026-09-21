# Deck MCP terminal control architecture

Status: control-protocol-v5 / state-schema-v6 / runner-protocol-4 scheme-B
ADR, 2026-09-21. The control protocol is Deck's own Adapter↔app protocol, not
the MCP standard version (negotiated separately by the SDK); an Adapter built
for protocol 4 gets `PROTOCOL_MISMATCH` and fails closed. The current protocol
exposes one structured execution tool; legacy state-schema-v6 `allowShell`
fields are ignored, and runner protocol 4 accepts only direct launch requests.

## Decision

Deck uses a thin Rust STDIO MCP adapter, a private in-process Deck control
service, and a signed `deck-mcp-runner` sidecar that is the pane process of
each MCP-managed tmux session.

The adapter owns only MCP framing, strict schemas, annotations, and conversion
to structured results. Deck owns feature enablement, client and project scope,
Board operations, idempotency, control epochs, generations, and the job
ledger. `mcp_fs.rs` owns descriptor-relative structured reads without child
processes: non-blocking `O_NOFOLLOW` opens with a type check on the same
descriptor, one name policy shared by list/read/identity/search, and a root
policy that refuses `/`, the account home and its ancestors, and excluded
directories. That policy governs structured reads only; it is not file
isolation for trusted-host jobs. The runner owns one visible session's actual processes, bounded output,
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

The signed runner is the tmux pane process. `deck_exec` starts the requested
absolute executable path with an exact argument vector in that same pane; bare
names and relative paths are refused, and no implicit shell
parses it. The executable may itself be an interpreter or shell. Execution
approval therefore permits arbitrary programs with the logged-in user's
permissions and is not a sandbox. argv remains visible in normal host process
metadata and must not carry secrets. stdout/stderr are mirrored to the pane and retained as bounded combined
PTY output; the job's private process group (pid == pgid, a background group
of the pane's terminal) supplies exit and interrupt identity, and the runner
reaps the leader itself so no signal can reach a reused PID. stdin is a
job-owned pipe, so input after exit is refused rather than falling through to
a parent shell.

Process and signal ownership. The runner is the pane's session leader and
foreground process group. It blocks SIGINT/SIGQUIT/SIGTSTP/SIGHUP/SIGTERM in
all threads and consumes them on one `sigwait` thread: terminal keys never
kill or stop the runner. In human mode a terminal ^C becomes
`killpg(job, SIGINT)` — the local stop key; in MCP/fenced mode it is ignored
(Deck blocks the keyboard anyway). SIGHUP (tmux kill-session) and SIGTERM make
the runner SIGKILL every live job group before exiting. Before a card close in
the launching Deck process, Deck sends authenticated `stop`: SIGINT → SIGTERM
→ SIGKILL with bounded waits, confirmed by reaping. After a Deck restart the
key is gone and the tmux pane's SIGHUP cleanup supplies the close boundary.
Jobs start with default dispositions and an empty mask. A job stopped by
SIGTTIN/SIGTTOU is reported `stopped`. Descendants that call setsid/setpgid,
or group members that outlive the leader, are outside these guarantees.

This preserves visible execution and file changes with a smaller contract than
shell hooks. It intentionally does not preserve `cd`, `export`, aliases, or
functions across calls. Use `cwd`; invoke a shell explicitly through
`deck_exec` only when composition is genuinely required.

## Board transaction

`deck_session_create` persists an accepted operation first. `ui/js/mcp.js`
claims it and calls `provider.createStarted`, the existing authoritative Board
transaction. The runner/tmux session starts inside `beforePersist`; the card is
then persisted. A failed Board write kills only the session created by that
transaction. Recovery adopts an orphan only when its private runner socket
reports the exact recorded generation; an unrelated same-name tmux session is
never adopted.

Close uses `provider.close`, including scheduler cancellation and the ordinary
Board persistence path. While holding the Board mutation slot it exchanges the
executing close plan for a target-bound admission token immediately before the
first queue-cancellation side effect. Queue cancellation and tmux termination
both verify that token. A takeover ordered before admission rejects the close;
a revocation ordered after admission does not falsely claim that prior effects
were rolled back. After admission the only outcomes are `committed` (the Board
write finished) or `ambiguous` (a side effect may have run); `closing` is
cleared on every outcome, and a repeated completion with the same result is a
no-op. A remote close is refused (`card-shown`) while any pane shows the card.
MCP never writes `deck.json`.

## State and fencing

Control operations use `accepted`, `executing`, `admitted` (close only),
`committed`, `rejected`, and `ambiguous`. Jobs separately use `starting`,
`running`, `stopped`, `exited`, and `lost`. Terminal text is never returned
and never completion evidence.

Every exec verifies the authenticated adapter principal, holder, execution-grant and policy versions,
service-start identity, project/session scope, canonical cwd, generation,
control epoch, lease, environment profile, script digest/length, timeout and
request identity. A
single delivery fence serializes dispatch with revoke, disable, close,
takeover, and return. Emergency actions (takeover, revoke, disable, execution
revoke) set their in-memory fence BEFORE waiting for that lock, and every
dispatch re-checks the fences after acquiring it, immediately before the
runner call; a dispatch already on the wire completes and is then fenced by
the runner epoch. These local commands run off the UI thread. Human takeover
advances the epoch, pauses output sharing and closes every existing job
binding's output before enabling Deck keyboard input; it starts no shell.
Execution revoke changes only execution authority: it leaves the session's
output-sharing switch as it was and interrupts nothing. The
runner discards ordinary pane input while MCP owns control and routes MCP
input only to the named running child's stdin. Return to MCP needs no
execution grant: it persists a new epoch with no holder first, then tells the
runner; a runner failure re-fences.

Runner dispatch carries the service-start identity, generation (fixed by the
runner process), holder/epoch, grant and policy versions, expiry, and intent
hash. The runner starts fenced, answers `runner-stale` to an old service
identity, rejects stale epochs, deduplicates job ids, and applies explicit
grant authorization and revocation barriers. An exec grant must first have
been registered by the authenticated Deck process with the same version,
policy version and expiry; an exec request never creates a grant or advances
the runner epoch. A missing barrier acknowledgement is reported as uncertain
even though the in-process admission gate is already closed. Runner
connections are switched to blocking I/O with 5-second timeouts and capped at
16; the control socket likewise has timeouts and a cap of 32.

### Runner control authentication

tmux `new-session` sends a command to an already-running tmux server, which
later creates the pane process; an anonymous fd inherited by the Deck client
is therefore not inherited by that pane. Passing a key in the pane command,
environment, tmux options, or a file would expose it to other same-user
processes and was rejected.

Runner protocol 4 instead makes each runner generate an independent 256-bit
CSPRNG key after binding its private socket. The pane argv contains only the
launching Deck PID. Deck makes a one-time claim over the socket; the runner
accepts it only when the kernel-reported peer PID (`LOCAL_PEERPID` on macOS,
`SO_PEERCRED` in the Linux test build) equals that launch PID and the service
instance and generation match. The PID is public but cannot be chosen by the
connecting process. The key is returned once, retained only in the two
processes' memory, and never enters argv, environment, a tmux option, or disk.
Every later runner request, including ping/read/stop/shutdown, carries the key
and uses a constant-time comparison; missing and incorrect keys receive the
same `authentication-failed` response.

Socket paths are never published with permissive creation modes. The runner
creates a missing socket directory with mode 0700 in the creation operation
and refuses an existing directory unless it is exactly 0700. Both the runner
socket and Deck's control socket are bound under private temporary names,
changed to 0600, and only then atomically renamed to their public paths. This
avoids changing Deck's process-wide umask while other threads may create files.

Only authenticated control requests may change the runner epoch, and they
must advance it by exactly one. Renewals do not call the runner because they
do not change the epoch. Exec/input/interrupt contexts must equal the current
epoch and holder and cannot move either value. Thus a pane process cannot
raise the epoch, invent a grant, or take MCP control back after Human/Fenced.

A Deck restart intentionally loses the in-memory keys and cannot reclaim an
old runner. Such a runner is stale and the new app cannot ping, read, or stop
it through the control socket; closing its tmux session remains safe because
the pane SIGHUP path SIGKILLs its live job groups. This is the restart boundary
required to keep the key off disk.

Side-effect request ids are fingerprinted over the parsed arguments (null ≡
omitted). Equal request id and arguments return the recorded operation
without repeating any side effect (including runner control); different
arguments return `REQUEST_ID_CONFLICT`. Each record carries the session and
control epoch it was bound to, and a control record also the control
sequence it produced. A terminal record is retired once its session is gone,
its epoch is no longer current, or (control) a later change superseded it — a
replay then fails the generation, epoch or sequence check (`STALE_REQUEST`),
so retirement never turns an old request into a new one. A sequence is replay
identity only: it is not an execution grant, and advancing it never creates
or extends one; an exact renew replay returns its record and never extends the
lease again. Creates are bound to
the client's create sequence the same way; creates/closes stay queryable in a
32-per-client window and a close is kept while its session is at the epoch it
named. Control records (one per session) never draw from the ordinary pool,
so release/request always fit; each client may hold 500 ordinary records;
64 slots beyond the ordinary pool are reserved for interrupts (16 per
client). Under pressure, epochs whose lease lapsed are closed first. Job bindings are capped at 64 per session (the runner retires its
oldest finished jobs the same way), and execution grants at the newest per
session plus those still referenced by a binding. Nonterminal and ambiguous
records of a live session are never retired. Scripts and input bytes are not
persisted.

Execution grants use a monotonic in-process deadline plus a wall-clock display
deadline and are bound to a random service-start identity. Restart never
restores them, and control Request/Renew cannot extend them. After Deck
restarts, every accepted/executing/admitted operation — Board create and close
included — becomes `ambiguous` (`deck-restarted`); nothing is replayed.
Control ownership is cleared and its epoch advanced, and `closing` flags are
cleared.

## Security boundary

This is trusted-host execution, not a sandbox. An authorized script has the
macOS account's permissions and may access paths outside its initial cwd.
Canonical root checks prevent accidental selection of another workspace; they
do not create filesystem isolation. Code run by package managers, builds, and
tests is equally privileged. Same-UID malicious code is outside the protection
provided by a 0600 socket and 0600 state file.

The local service accepts only same-effective-UID Unix-socket peers. The
adapter presents a random public per-client id plus a separate bearer read from
the login Keychain. The bearer is neither argv nor display state and is never
forwarded to the runner/job. The adapter resolves the socket from the account
database (not `$HOME`) and sends the bearer only after `getpeereid` shows the
peer runs as the same user; it does not verify the peer's code signature, so a
same-UID process that already replaced Deck's socket remains out of scope.
Socket, environment and credential-FD overrides are compiled into debug builds
only, for synthetic isolated harnesses. The adapter cannot expand its recorded
project roots. Diagnostics record only closed status/error codes;
terminal output, scripts, and stdin are returned through the functional MCP
channel and excluded from `app.log`.

No launchd job, login item, PATH tmux fallback, home-directory executable,
LLM API, coding-agent process, or background model loop is introduced. The
runner and adapter are signed bundle sidecars. Deck must be running for new
control requests; closing the GUI removes the control service, while tmux and
already-started processes may continue.
