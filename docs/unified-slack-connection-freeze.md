# Unified Slack Connection freeze

The Unified Slack Connection implementation at `e221a8ec591fa407b5efa8d5bf569e011e76200c`, together with the post-merge hygiene commit `9c6d8a26d4df615306d0a01a750c12f191bb1c49`, is the frozen baseline for the next Nightly candidate. The user reported that the unified connection had passed real Slack acceptance before the hygiene work. The hygiene changes passed the local repository gates recorded below; they did not repeat live Slack acceptance.

Keep one shared Socket Mode owner. Preserve event routing, platform ACK ownership and ordering, Reaction catch-up, Channel durable inbox staging before ACK, and Channel admission. Canonical Keychain credentials remain `slack-user-token`, `slack-bot-token`, and `slack-app-token`; the old `slack-channel-bot-token` and `slack-channel-app-token` are unused by runtime and removed only by the explicit Settings action. Pending Channel inbox entries remain drainable without any Slack credentials.

The status command reads credential presence and process-local validation/connection facts without calling `apps.connections.open`. An explicit App token save verifies before replacing the Keychain value. The shared transport requests a confined WSS URL only when it is about to connect. No second WebSocket owner is permitted.

Local acceptance for the hygiene commit: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`, `cargo test --workspace --locked`, `scripts/ui-tests`, `node app/ui/js/check.mjs`, `scripts/check-workflows`, and `git diff --check` passed. WKWebView release smoke and a new live Slack run were not performed during hygiene. A Nightly candidate still needs the release preparation and pre-dispatch checks in [release-channels.md](release-channels.md).

Changes to these invariants require a directly related defect, focused evidence, and fresh acceptance. Release version metadata can advance without reopening the connection architecture.
