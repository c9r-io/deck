# Optional Secure Tunnel helper

Looking for setup instructions? See [Connect ChatGPT to Deck with Secure Tunnel](secure-tunnel.md). This page is the technical and security reference for the optional Helper.

Deck MCP works without `deck-tunnelctl`. Removing the helper does not disable
Deck MCP, change MCP authorization, or prevent Deck from starting. Settings
degrades to “optional helper not installed.”

## Boundaries

```text
Deck.app
  │ closed status/start/stop/remove protocol; client_id only
  ▼
deck-tunnelctl
  │ exact executable and argv; no shell
  ▼
OpenAI tunnel-client
  │ outbound HTTPS
  ▼
OpenAI Secure MCP Tunnel
```

- Deck owns MCP enable/disable, client authorization, project/root scope,
  session creation, execution grants, stdin/output sharing, control epochs,
  takeover, revoke, and delete.
- `deck-tunnelctl` owns helper protocol, tunnel-client verification, the
  deterministic runtime alias, Runtime API key Keychain access, secret
  transport, lifecycle commands, and status normalization.
- `tunnel-client` owns its runtime profile, process lifecycle, logs, health,
  persistence format, tunnel protocol, and network connection.
- OpenAI owns the remote tunnel and control plane.

This separation is **software and credential ownership isolation**, not
OS-enforced sandbox isolation. All three native programs run as the current
login user. In particular, a compromised `tunnel-client` can launch the
already configured `deck-mcp --client-id <authorized-client>` and attempt to
use that client's current authority. It cannot change Deck authorization,
expand project/root scope, create execution grants, bypass revoke or
generation/control epochs, override human takeover, or bypass execution,
stdin, and output-sharing gates.

## Installation and identity

The production helper is an independently signed/notarized optional artifact:

```text
/Applications/Deck Tunnel Helper.app/Contents/MacOS/deck-tunnelctl
```

For Secure Tunnel support, download the separate **Deck Tunnel Helper** asset
from the same Deck release. Extract `Deck Tunnel Helper.app` and move the app to
`/Applications/Deck Tunnel Helper.app`. Deck Settings detects the installed
helper automatically. The helper is not a daemon or Login Item; leaving it
uninstalled does not affect Deck MCP core. No shell installer, `chmod`, or
Gatekeeper bypass is required.

Deck rejects symlinks, non-regular/non-executable files, invalid signatures,
the wrong Team ID or signing identifier, incompatible protocol versions,
malformed JSON, output over 64 KiB, non-zero exits, and timeouts. A debug build
may use `DECK_TUNNEL_HELPER_PATH`; release builds do not compile that override.

Deck contains no tunnel-client version, hash, path, release-manifest, or CLI
policy. Those checks belong only to the helper. The initial helper recognizes
the verified official arm64 `tunnel-client` 0.0.14 binary by its SHA-256 pin
alone (the pin fixes the version and CLI, so no `--version`/`--help` probe
runs). Its Homebrew shell wrapper is not executed. Ambient PATH is not trusted.

File identity is device, inode, size, mtime and ctime; ctime cannot be set by
the user, so a same-size overwrite with a restored mtime is detected. Deck
records the helper's identity at validation and re-compares it immediately
before every spawn. The helper records `tunnel-client`'s identity together
with the hashed bytes, re-compares it before every run, and re-hashes the
file immediately before `runtimes connect`, the only run handed the
Runtime-key `file:` reference.

Residual risk: both installed locations are writable by the login user in
common setups, and exec is by path. A same-user attacker who swaps the file
in the instant between the last check and `exec` is not stopped. This release
does not claim an OS sandbox or race-free fd-based exec.

### Process footprint (EDR)

Deck's EDR-quiet rule covers Deck itself: the helper is the one executable
outside Deck's bundle that Deck spawns, at the fixed path above, with closed
argv and no shell. Deck runs the protocol handshake once per app session (per
helper identity) and answers repeated status queries from a serialized
3-second cache, so opening Settings costs at most one helper run per client.
The helper's only process spawn is the verified `tunnel-client` (a census
test in `tools/deck-tunnelctl/src/lib.rs` enforces it; CI also scans the
release binary with `scripts/check-edr-binary`). A status query runs
`tunnel-client` one to three times.

`tunnel-client` itself is outside that promise. Its runtime runs in its own
tmux session, keeps an outbound HTTPS connection to OpenAI, and keeps running
after Deck or the helper exits until it is explicitly stopped. It does not
survive a reboot.

## Setup and credentials

In Deck Settings choose **Set up…**, copy the exact command, and run it in a
visible Terminal. Deck has no safe generic Terminal-command primitive and does
not use AppleScript, a shell child, or a temporary script to launch setup.

The helper asks interactively for the Tunnel ID and Runtime API key. The key is
stored under:

```text
service: io.c9r.deck-tunnelctl.runtime
account: <client_id>
```

Deck never writes, reads, receives, forwards, caches, or logs this key. The
Deck MCP bearer remains separately owned by Deck under
`io.c9r.deck.mcp`; the helper never reads or receives it. A future Admin
credential must use a different service and account namespace.

The helper passes the Runtime API key using tunnel-client's `file:` reference
and never places it in argv. The file is random, create-new/no-follow, mode
0600 inside a mode-0700 private directory, and contains no other state. The
2026-09-23 live v0.0.14 verification proved that the file can be deleted after
the first successful control-plane poll: the running runtime remained healthy
through four later poll windows. The runtime manager did not automatically
restart an unexpectedly terminated child. An explicit reconnect with the
missing old file failed closed; a new Keychain read and a new temporary file
then connected, polled successfully, and remained healthy after deletion.

Production `setup` and `start` therefore use the verified bounded sequence:

```text
Keychain read
  → new private temporary file
  → re-hash tunnel-client
  → runtimes connect
  → bounded status + successful control-plane poll
  → delete temporary file
```

The whole `start` command is bounded to 120 seconds, below Deck's
135-second kill (status 10s < 12s, stop/remove 8s < 10s), so the helper
normally deletes the file itself. SIGINT, SIGTERM and SIGHUP also delete it
before the helper exits. SIGKILL cannot be caught: every later helper command
first removes `deck-tunnelctl-*` directories whose owning process is gone,
accepting only a real mode-0700 directory owned by the current user and only a
regular `runtime-key` file inside it. Key buffers read from Keychain or the
terminal are zeroed when dropped.

There is no plaintext retention, literal-argv fallback, or environment
fallback. A future tunnel-client version requires a new helper compatibility
and lifecycle decision; Deck itself contains no such policy.

## Lifecycle and reboot behavior

The helper delegates lifecycle to official commands:

```text
tunnel-client runtimes connect
tunnel-client runtimes status
tunnel-client runtimes stop
tunnel-client runtimes rm
```

It does not create a daemon, pid file, LaunchAgent, LaunchDaemon, Login Item,
cron entry, watchdog, or custom persistence. After reboot, “Tunnel stopped” is
an expected state; start it explicitly.

Ready means tunnel-client reports a running process that is healthy, ready,
not stale, and has completed a successful control-plane poll. The v0.0.14
`runtimes status` poll summary can remain `unknown`, so the helper uses the
official bounded `health --require-control-plane-poll --json` result rather
than inferring readiness from the process flag. Ready does not claim that a
ChatGPT user is currently connected.

## Revoke, disable, and delete

MCP revoke and global disable never call or wait for the helper. Deck's local
fencing completes first and remains authoritative even if the helper is
missing, replaced, hung, crashed, or offline. Version 1 does not automatically
stop a runtime on revoke, delete, or global disable.

When deleting a revoked client, Settings performs a separate optional status
lookup. If a local runtime still exists it offers Stop Tunnel, Delete Tunnel
Runtime, Delete Deck Client Anyway, and Cancel. Cleanup failure never rolls
back revoke or delete. `remove` deletes local runtime state only; it does not
delete the remote OpenAI tunnel.

## Troubleshooting

- **Optional helper not installed:** local Deck MCP is unaffected. Install the
  separately signed helper only if Secure Tunnel integration is wanted.
- **Helper unavailable or untrusted:** reinstall the signed helper. Deck does
  not fall back to PATH.
- **tunnel-client missing/untrusted:** install a helper-supported official
  tunnel-client release. Updating support requires only a helper update.
- **Runtime key missing:** run the copied interactive setup command again.
- **Stopped after reboot:** choose Start Tunnel; no automatic persistence is
  installed.
- **Unhealthy or stale:** stop it, inspect tunnel-client's own diagnostics,
  then start it explicitly. Raw subprocess stderr and log tails are not copied
  into Deck logs or UI.

Creating ChatGPT connectors and any final ChatGPT-side step remain manual.
This component does not automate browser sessions, private APIs, OAuth, or
remote tunnel deletion.
