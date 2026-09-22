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
the validated Mach-O target. Updating this policy requires a helper release,
not a Deck release.

Build and test independently:

```text
cargo fmt --manifest-path tools/deck-tunnelctl/Cargo.toml -- --check
cargo clippy --all-targets --locked --manifest-path tools/deck-tunnelctl/Cargo.toml -- -D warnings
cargo test --all-targets --locked --manifest-path tools/deck-tunnelctl/Cargo.toml
```

Production packaging is a separate signed and notarized
`Deck Tunnel Helper.app` whose CLI executable has signing identifier
`io.c9r.deck-tunnelctl`. It creates no Login Item, LaunchAgent, LaunchDaemon,
cron entry, daemon, or updater.

## Current release gate

`setup` and `start` currently fail closed with
`secret_file_lifecycle_not_verified`. They must not be enabled until the live
v0.0.14 test in `docs/mcp-tunnel-helper.md` proves when a `file:` secret may be
deleted. No plaintext retention, literal-argv fallback, or unverified
environment fallback is permitted.

This is software and credential ownership isolation, not an OS sandbox. The
helper and `tunnel-client` run as the logged-in user. A compromised
`tunnel-client` can start the configured `deck-mcp` for an authorized client
and try to exercise that client's existing authority. Deck's project/root
scope, execution grants, epochs, takeover, stdin/output gates, revoke, and
disable remain authoritative.
