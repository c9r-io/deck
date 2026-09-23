# deck-tunnelctl

`deck-tunnelctl` is an optional, independently built Deck Secure Tunnel
lifecycle helper. It is not a Cargo workspace member, Tauri sidecar, Deck
startup dependency, MCP client, or MCP authorization component.

Commands form a closed protocol:

```text
deck-tunnelctl protocol --json
deck-tunnelctl status --client-id client_xxx --json
deck-tunnelctl start --client-id client_xxx --json
deck-tunnelctl stop --client-id client_xxx --json
deck-tunnelctl remove --client-id client_xxx --json
deck-tunnelctl setup --client-id client_xxx
```

`setup` is interactive because Deck must never receive the OpenAI Runtime API
key. The key belongs to Keychain service
`io.c9r.deck-tunnelctl.runtime`, account `<client_id>`. No key is accepted in
argv, JSON, an environment variable, or persistent helper state.

The helper currently recognizes the official macOS arm64 `tunnel-client`
0.0.14 binary whose hash was checked against the OpenAI release. Homebrew's
shell wrapper is only a discovery path; the helper canonicalizes and executes
the validated Mach-O target. It re-compares the file identity (device, inode,
size, mtime, ctime) before every run and re-hashes the file before
`runtimes connect`. Updating this policy requires a helper release, not a
Deck release.

The helper's only process spawn is that verified `tunnel-client`
(`Client::spawn`); `production_spawn_census_is_one_verified_tunnel_client_site`
in `src/lib.rs` fails on any other `Command::new`, shell, lower-level spawn
or persistence vocabulary. Each command has one time budget below Deck's
kill timeout (see `src/cli.rs`). `tunnel-client`'s runtime and its outbound
connection outlive the helper until explicitly stopped and are outside Deck's
EDR-quiet promise (`docs/mcp-tunnel-helper.md`).

Build and test independently:

```text
cargo fmt --manifest-path tools/deck-tunnelctl/Cargo.toml -- --check
cargo clippy --all-targets --locked --manifest-path tools/deck-tunnelctl/Cargo.toml -- -D warnings
cargo test --all-targets --locked --manifest-path tools/deck-tunnelctl/Cargo.toml
cargo build --release --locked --manifest-path tools/deck-tunnelctl/Cargo.toml
scripts/check-edr-binary tools/deck-tunnelctl/target/release/deck-tunnelctl
```

Production packaging is a separate signed and notarized
`Deck Tunnel Helper.app` whose CLI executable has signing identifier
`io.c9r.deck-tunnelctl`. It creates no Login Item, LaunchAgent, LaunchDaemon,
cron entry, daemon, or updater.

## Verified Runtime-key lifecycle

The live v0.0.14 lifecycle gate completed on 2026-09-23. `setup` and `start`
read the key from the helper-owned Keychain item, create a new random 0600 file
inside a private 0700 directory, invoke `runtimes connect` with a `file:`
reference, wait for a successful control-plane poll within the command's
120-second budget, and then delete the file. SIGINT/SIGTERM/SIGHUP delete it
too; a file left by SIGKILL is swept by the next helper command. Every explicit start generates a different file. There
is no plaintext retention, literal-argv fallback, or environment fallback.

The controlled verification observed four later polling windows after deleting
the live file, an unexpected runtime-child termination, an explicit stop, a
fail-closed reconnect with the missing old file, and a successful reconnect
with a newly generated file. v0.0.14 did not automatically restart the
terminated child, so no absent `file:` reference was reread in the background.

### Ignored real-lifecycle harness

The test-only `real_file_secret_lifecycle` harness records this release-gate
evidence. It is ignored by default and is absent from release binaries. Before
re-running it, create a dedicated Deck MCP client and place a
restricted, disposable Runtime API key in macOS Keychain under the service
above and that dedicated client ID as the account. Enter the key only through
Keychain Access or another approved interactive Keychain boundary; never put
it in this command or an environment variable.

The only environment values accepted by the harness are the non-secret,
dedicated client and Tunnel IDs:

```text
DECK_TUNNEL_LIFECYCLE_CLIENT_ID=client_xxx \
DECK_TUNNEL_LIFECYCLE_TUNNEL_ID=tunnel_xxx \
cargo test --manifest-path tools/deck-tunnelctl/Cargo.toml \
  real_file_secret_lifecycle::real_file_secret_lifecycle -- \
  --ignored --exact --nocapture
```

The harness refuses a pre-existing deterministic test alias, uses exact argv
only, observes multiple v0.0.14 polling windows, targets only the exact tmux
session reported for that alias during the unexpected-death check, regenerates
a second secret file for reconnect, scans bounded captures and local logs for
the in-memory secret, and removes only the runtime it created. A future
tunnel-client version still requires its own helper compatibility and
lifecycle validation.

This is software and credential ownership isolation, not an OS sandbox. The
helper and `tunnel-client` run as the logged-in user. A compromised
`tunnel-client` can start the configured `deck-mcp` for an authorized client
and try to exercise that client's existing authority. Deck's project/root
scope, execution grants, epochs, takeover, stdin/output gates, revoke, and
disable remain authoritative.
