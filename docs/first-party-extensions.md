# First-party extensions: automatic composition and Settings contributions

Deck discovers first-party **source declarations at build time**, and mounts
bundled Settings contributions at runtime. All code still compiles or embeds
into the signed application. This is not an external plugin loader: no user
setting, environment variable, download or runtime directory adds an extension.
There is no plugin-management screen, sandbox or hot unload.

## One composition source

Each backend `app/extensions/<id>.json` declares exactly `id`, `kind`, `rust`, `ui`,
`order` and `selected`. A UI-only `settings` declaration omits `rust` (and
rejects that field rather than accepting a dummy implementation). The filename equals the stable id. Kinds remain
separate contracts, without a common execution interface:

| Kind | Backend descriptor | Current entries |
|---|---|---|
| `service` | `extensions::ServiceIntegration` | Slack |
| `agent` | `agent_status::AgentAdapter` | Claude Code, Codex, Pi |
| `translation` | `intelligence::provider::TranslationProviderEntry` | Bergamot |
| `access` | `extensions::AccessIntegration` | Phone Connector, MCP |
| `settings` | None; UI contribution only | Voice preferences |

`scripts/extension_catalog.py` reads only these source manifests, sorts by
explicit unique order and generates two checked-in, reviewable files:

- `app/src-tauri/src/extensions/generated.rs`: the four typed tables and the
  single selected translation engine. A contract test compares each manifest
  identity with the actual typed descriptor's identity.
- `app/ui/js/extensions-generated.js`: static ES imports and the contribution
  list; manifest/UI identity disagreement is refused before Settings mounts.

Cargo runs composition before compiling and before the existing exact-copy UI
staging step. No bundler or package manager is added; the frontend remains
native ES modules. Node-only development can refresh with
`python3 scripts/extension_catalog.py`; CI runs the same tool with `--check`
to reject stale committed output. Never edit generated files or `ui-dist`.

The generator refuses duplicate identities, implementation paths, UI modules
or order, unknown fields/kinds, unsafe module paths, missing modules/exports,
unregistered `extension-*.js` modules, symlink inputs/outputs and anything other
than exactly one selected translation provider. Source generation itself does
not install hooks, start a connection, load a model or change user data.

## Runtime boundaries

`extensions.rs` is still the one typed runtime composition boundary. Registered
is not enabled, connected, loaded, trusted, ready or authorized. A lookup starts
nothing. Duplicate identities have no winner at every consumer: malformed
service tables start no connection owner; duplicated service/source identities
contribute no polling or credential ownership; duplicated agent identities
contribute no status reads or migrations. The clock remains dispatcher-owned.

Services retain their existing routes through start/wake/reannounce, source
contribution, live status and credential ownership. Slack's Socket Mode owner
is still unique. Translation still constructs its selected provider lazily and
loads a model only when the coordinator needs it.

Access registrations own only an identity and the existing startup entry.
Boot order stays service integrations, Connector, then MCP. They do not pretend
to be inbound trigger services and need no dummy credentials or source methods.
MCP input/restart guards, its durable authorization ledger and execution fencing
remain compiled core calls, independent of whether an access feature is enabled.
Connector pairing and admission remain its native owners. Neither registration
nor opening Settings binds an opt-out listener or authorizes a client. Voice
preferences add no recorder startup, microphone permission or model download.

Agent status reads iterate unambiguous registrations. `agent_hooks_status`
includes an `agents` map keyed by source, with `signal`, nullable `lifecycle`
and nullable `installation`. The old fixed keys remain compatibility projections
of those same observations, not a second read or persistence store. Hook
adapters use their declared config/specs; Pi retains its byte-preserving one-entry
implementation. `migrate_agents_on_boot` is one registered phase, preserving
hook migration, legacy-helper retirement, Pi migration, then listener startup.
Migration updates existing opt-ins; it never enrolls an integration.

## Settings contributions

Each declared `extension-*.js` exports `CONTRIBUTION`. The common host in
`settings-extensions.js` consumes the generated directory, mounts contributions
before Settings binds controls, and coordinates shared status reads and writes.
Per-extension declarations provide control identity, group/order, translated
labels/hints/confirmations, search terms and implementation bindings. Standard
agent controls share a renderer and transaction coordination. Slack, translation,
Connector, MCP (including its optional Tunnel presentation) and voice preferences
own dedicated controllers as well as panels and event wiring.

`create(host)` constructs an inert controller; `mount()` binds it once. The
host dispatches `refresh`, `localeChanged`, `loaded`, `close` and the committed
inbound-settings refresh to controllers that implement them. A failed callback
does not prevent later contributions from running. Controllers keep their
original read generations, cancellation and pending-write rules. The host supplies
the one Settings commit/drain path, feedback, dialogs and native services; it
does not assemble feature-specific Slack/Bergamot callbacks. Compatibility exports
in `settings.js` delegate to these same controller instances for existing callers.
Search and feedback declarations travel with the contribution. There is no second
writer, new persisted document or runtime plugin lifecycle.

`settings-search-model.js` combines core groups with contributed groups. Several
agent contributions intentionally share `agent-status`: their labels and search
keywords combine under that one stable group. Existing section order, setting
ids, control ids, navigation, focus, bilingual matching and user-facing text stay
stable. There is no second manually maintained extension search directory.

Behavioral invariants are as important as markup:

- An unknown, timed-out or failed read stays visibly unknown and disabled;
  retry only reads. Older responses cannot overwrite newer choices.
- Pi's two choices share one external config entry and a write lock. They stay
  independent opt-ins; session observation never becomes Signal or re-entry.
- Confirmation, rollback, persistent inline feedback and toast behavior remain
  owned by the same user action. Mounting/importing grants nothing.
- The one Settings document writer remains in `settings.js`. Slack authority,
  credential verification and canonical grants stay in their existing backend
  owners; the contribution cannot authorize its own events.
- Translation's download/install/delete guards and the Lens entry share the
  existing implementation. Merely registering its panel never downloads a pack.

The shared Rust frontend census and JS syntax gate scan recursively, so moving a
module into a subdirectory cannot silently remove it from their checks. Generated
sources remain inside the existing source/coverage boundaries.

## Adding a first-party extension

1. Implement the appropriate typed backend descriptor and its narrow behavior.
2. Export a UI `CONTRIBUTION` from `app/ui/js/extension-<id>.js`, reusing ordinary
   controls or supplying a dedicated panel. Keep dependencies one-way; receive
   host services rather than importing `settings.js` back. A controller factory
   must not read native state, mutate the DOM or activate anything at creation.
3. Add `app/extensions/<id>.json`, run the generator and review both generated
   files. Existing-kind status/UI registration needs no central table edit.
4. Add behavior tests and run the gates and applicable real-WKWebView acceptance.

This removes composition bookkeeping, not deliberate core decisions. A new
persisted trigger source still needs its closed `inbound::SOURCES` vocabulary,
settings validation and fixtures; a new credential slot remains a Keychain
change. New Signal proof, remote admission, first-send authority or re-entry
semantics still require explicit core implementation and tests. The source
manifest cannot expand any of those contracts.

## Optional re-entry capability of an agent adapter

`AgentAdapter` has one more narrow, optional, static field: `reentry`
(`agent_reentry::ReentryCapability`), `Some` for `CLAUDE_CODE` and `None`
for `CODEX` and `PI` (neither way Pi is installed lets Deck prove whose
code a client is). What a source's lifecycle receipts mean is part of that
capability (`receipts`, a `lifecycle::ReceiptContract`): a source without
one can send no receipt. Pi's optional session observation
(`agent_pi_lifecycle_set`, `lifecycle::observe_pi`) is not a receipt and
gives Pi no contract: it says which native session a running Pi is in, in
memory only, and makes Pi no candidate for re-entry. A session id Deck does
not keep is said as that (`unsupported`), never cut short or passed over. In the restart review a
card whose foreground is a verified Pi is named as Pi and told its own
reason (`client-identity-unverifiable`); it is never a candidate, and an
approval that names it is refused whole. It holds the product's code requirement — whose code a client
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
[acceptance summary](evidence/closeout-20261011/reentry-acceptance.md)
records the new identity mechanism's completed representative P3 on Claude
2.1.295: A/B through both the controller and the isolated real application,
and later runs on 2.1.296 in a real window at a stable location under a
relocated home: in a debug carrier, and once in a Developer-ID-signed release
carrier with a person operating the window. Nothing ran at
`/Applications/deck.app`.
That result does not certify untested platforms or a published carrier.

## Compatibility and verification

No persisted settings key, source id, approval, queue or ledger schema changes.
No installed-plugin state or migration is introduced. The status IPC extension
is additive; legacy keys remain available. Unknown newer state continues to
follow the existing storage compatibility rules.

Run the ordinary gates in `CLAUDE.md` and `.github/workflows/gate.yml`, including
catalog freshness and its negative tests, frontend coverage and source census,
Rust lint/tests/coverage and the native Settings/agent/Slack/translation checks.
Node DOM tests alone do not certify WKWebView layout or input behavior.

Historical phase-1 evidence remains at
`docs/evidence/first-party-extensions-20261009/README.md`; it is not evidence that
the current composition/UI refactor passed its native scenarios.

Automatic composition evidence: [2026-10-16 registration report](evidence/first-party-autoreg-20261016/README.md).
Controller ownership, access registrations and preference contributions: [2026-10-16 governance report](evidence/first-party-governance-20261016/README.md).
