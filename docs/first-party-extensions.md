# First-party extensions (phase 1)

Deck's service knowledge, agent-specific adapter declarations and optional
local capability are organised behind one narrow, static composition point:
`app/src-tauri/src/extensions.rs`. This is an internal architecture seam. It
is not a plugin system and adds nothing a user has to learn.

## What it is

Three typed tables, compiled into the app, with no shared interface:

| Kind | Table | Entry | Today |
|---|---|---|---|
| Service integration | `SERVICES` | `extensions::ServiceIntegration` | `slack::INTEGRATION` |
| Agent adapter | `AGENT_ADAPTERS` | `agent_status::AgentAdapter` | `CLAUDE_CODE`, `CODEX` |
| Local capability provider | `TRANSLATION_PROVIDERS` + `TRANSLATION_ENGINE` | `intelligence::provider::TranslationProviderEntry` | `BERGAMOT` |

Rules the module header states and its tests pin:

- An entry is source code. No setting, environment variable, path or download
  adds one. The only override is a `cfg(test)` thread-local table.
- Registered is not enabled, connected, loaded, trusted or authorized. A
  lookup starts nothing and adds no polling, connection or model load.
- An identity is used only when exactly one entry carries it, at every
  consumer and not only at the lookups; a clash has no winner, first or
  last. Unknown or doubly registered ids fail closed at the caller. A
  service id two entries share takes no part in start, wake,
  re-announcement, source contribution or credential ownership. A trigger
  source is polled only when one contributed source carries its name, the
  dispatcher's closed `inbound::SOURCES` names it, and it is not the clock
  the dispatcher appends itself. A clash removes only the identities in it;
  a malformed table also starts no connection owner. This is a rule about
  compiled-in tables: the shipped tables have no clash, and nothing a user
  does can create one.
- Extensions run with the application's own trust. Module visibility is an
  architecture constraint, not a security boundary: there is no sandbox, no
  dynamic loading, and no compatibility with any plugin ecosystem.

## Production call paths

| Kind | Registered entry | Production callers | Decisions the core keeps |
|---|---|---|---|
| Service | `slack::INTEGRATION` (`slack.rs`) | `main.rs` setup → `extensions::start_services` (the only start of `slack_transport::spawn`, which refuses a second owner); `inbound::spawn_inbound` → `extensions::service_sources` (+ the built-in clock); `inbound_status` (credential presence rows, live connection); `inbound::set_secret` → `extensions::secret_owner` → `store_secret`; `inbound_setup` → `setup_url`; `inbound_check_now` → `wake_services`; the poll loop → `reannounce_services` | Board creation and persistence; dedupe ledger, pending and ACK-to-webview order; provenance, content authority, first-send permission and readiness (`scheduler/authority.rs`, `first_send.rs`, `channel_first_send.rs`, `documents.rs`); settings shape (`inbound::validate_settings`, including the channel rules); final input delivery and attention routing |
| Agent | `agent_status::CLAUDE_CODE`, `CODEX` | `parse_event` (accepted source words), `ingest` (proof class), `agent_hooks_set`, `agent_hooks_status`, `migrate_hooks_on_boot` — all through `extensions::agent` / `agents` | Pane, ancestry, foreground-generation and interaction admission; `CodexSignalTrust`; every Signal consumer (`tests/signal_census.rs`); remote admission (`admission.rs`, which does not read the tables) |
| Provider | `provider::BERGAMOT` | `translation::Coordinator::from_registry` → `extensions::translation_provider`, held as `Box<dyn TranslationProvider>` | Enablement, pack download/verification/deletion, limits per mode, language gating, cancellation (a request that left `active` publishes nothing), unload; the Lens, pasteboard gate, selection ownership and where a result is shown |

## What moved, what was reused, what stays coupled

Reused as they were: the dispatcher's `Source` trait, the
`TranslationProvider` trait, the shared hook merge engine and its spec
tables, the Keychain's closed slots, every IPC command name and reply shape.

New: `extensions.rs`, `slack.rs` (one descriptor; the credential
verify-then-store adaptation moved here from `inbound.rs` unchanged),
`AgentAdapter` (replaces the parallel `SOURCES` list and `agent_modules()`
tuple array with one declaration per agent), `TranslationProviderEntry`,
and a one-owner guard in `slack_transport::spawn`.

Kept coupled on purpose:

- Slack-specific grant and first-send code stays in the core
  (`scheduler/authority.rs`, `scheduler/channel_first_send.rs`,
  `documents.rs`, `inbound.rs` `AutoSend`). An integration must not vouch
  for its own authority.
- `inbound::validate_settings` still calls
  `inbound_channel::validate_settings`: the settings document is one
  persisted schema that the core validates on load and save.
- `inbound::SOURCES` (`slack`, `clock`) stays the closed, persisted source
  vocabulary; the clock stays the dispatcher's built-in source.
- `agent_hooks_status` keeps its two fixed reply keys (`claude`, `codex`),
  and the frontend Settings sections, Slack connection panel and Lens are
  unchanged. No frontend registry exists: the backend IPC contract did not
  change, so a frontend table would only be a second description.
- `admission::interactive_agent` (executable names) is a separate closed
  list from the adapters' source words.
- `scheduler` and `commands.rs` still read `agent_status` directly, and
  `generation_evidence` still names the `codex` executable for the hold.

## Adding a future extension of an existing kind (not done here)

- A service: its own module with a `ServiceIntegration` descriptor and one
  entry in `SERVICES`; a new trigger source also needs its name in
  `inbound::SOURCES` and `limits.json` (a persisted vocabulary), a Keychain
  slot in `keychain.rs`, its rule shape in settings validation, and its
  Settings/drawer UI. Authority for its events is a core decision.
- An agent: one `AgentAdapter` constant and its entry in `AGENT_ADAPTERS`,
  a Settings toggle and a reply key in `AgentHooksStatus`. Signal trust and
  remote admission need their own, separate decisions.
- A translation engine: a `TranslationProvider` implementation, its entry,
  and changing `TRANSLATION_ENGINE` (one engine per build).

## Optional re-entry capability of an agent adapter

`AgentAdapter` has one more narrow, optional, static field: `reentry`
(`agent_reentry::ReentryCapability`), `Some` for `CLAUDE_CODE` and `None`
for `CODEX`. It holds the product's code requirement — whose code a client
must be, with no release, version or digest in it — the supported launch
grammar, the login shell the launch sink is written against and a pure
constructor of the exact-id request. Like
every entry here it is source code and registered is not enabled, approved
or trusted: the adapter has no pane, PTY, queue, Board or authority handle,
cannot approve an entry, cannot choose the bypass override and cannot launch.
The core (`agent_reentry/mod.rs`) owns eligibility, the user's per-restart
approval, the once-only launch and every result. The lifecycle hooks an
adapter declares (`lifecycle_hooks`) are a second ownership family beside
its Signal hooks, installed only by their own explicit opt-in.

A new release of the product needs no change here. A client is admitted by
its code identity — the requirement above, validated by Security.framework
against the running image and its file (`procinfo.rs`,
`procinfo/code_identity.rs`) — and by what the transaction itself observes
of the narrow native contract: the old client's launch image, its end
receipt, the new client's resume receipt and launch image. There is no list
of releases, no version range, no downloaded or learned compatibility data
and no probe of a client's `--help` or `--version`. What does need an
adapter change is a change of the product's signing identity or of that
contract; such a change shows as clients that are not offered, or as a
request that is typed once and not confirmed. The releases that were
exercised natively are evidence, not a runtime condition. The header of
`agent_reentry/claude.rs` records the historical campaigns; the current
[acceptance summary](../reports/agent-reentry-forward-compat/acceptance.md)
records the new identity mechanism's completed representative P3 on Claude
2.1.295: A/B through both the controller and the isolated real application.
That result does not certify untested platforms or a published carrier.

## Compatibility and withdrawal

No settings key, source id, rule id, queue, inbox or ledger field changed;
no schema version moved; nothing is migrated and no "installed extension"
state is persisted. Existing approvals are neither invalidated nor reissued.
Reverting the phase-1 commits restores the previous wiring with no data
step in either direction.

Evidence: `docs/evidence/first-party-extensions-20261009/README.md`.
