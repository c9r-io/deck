# Optional Secure Tunnel helper

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

Deck rejects symlinks, non-regular/non-executable files, invalid signatures,
the wrong Team ID or signing identifier, incompatible protocol versions,
malformed JSON, output over 64 KiB, non-zero exits, and timeouts. A debug build
may use `DECK_TUNNEL_HELPER_PATH`; release builds do not compile that override.

Deck contains no tunnel-client version, hash, path, release-manifest, or CLI
policy. Those checks belong only to the helper. The initial helper recognizes
the verified official arm64 `tunnel-client` 0.0.14 binary. Its Homebrew shell
wrapper is not executed. Ambient PATH is not trusted.

Both installed locations are writable by the login user in common setups.
Static signature/hash verification followed by an immediate file recheck
narrows but does not eliminate validate-to-exec replacement. This release does
not claim an OS sandbox or race-free fd-based exec.

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
0600 inside a mode-0700 private directory, and contains no other state. Release
requires a live tunnel-client 0.0.14 lifecycle test proving when the file may
be deleted. If that cannot be proved, the helper must not retain plaintext,
fall back to a literal argv key, or silently switch to environment transport.

The checked-in implementation currently keeps this release gate closed:
`setup` and `start` return `secret_file_lifecycle_not_verified`. Enablement
requires the live test above; unit tests alone are insufficient.

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

Ready means tunnel-client reports a running process that is both healthy and
ready and is not stale. A running process alone is not Ready, and Ready does
not claim that ChatGPT is connected.

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
