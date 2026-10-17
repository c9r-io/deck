# Phone Connector

Phone Connector is disabled by default. Enabling it in **Settings → Remote access** asks for confirmation, because a paired phone can:

- read the recent output of Codex and Claude cards;
- submit prompts to Codex and Claude sessions — a prompt can lead the agent to run commands, so this is equivalent to running code under your macOS account, with whatever permissions the saved card command and the agent's own configuration grant;
- start sessions from project presets.

A paired device keeps these abilities until it is revoked. Disabling Connector stops the listener; it does not revoke devices, so their tokens work again when Connector is re-enabled on the same identity. **Revoke** a device (or **Reset and re-pair**) to end its access.

The Mac sharing selector has three explicit scopes. Existing installations keep
what they previously selected; updating never expands consent:

- **Saved Codex/Claude cards** (default): the existing saved-command policy.
- **All card summaries**: all titles, projects, columns and order; additional
  cards still have no output, messages, notes or queue permission.
- **All card output and notes** (`boardAccess`, also what a trusted device gets): all current and future Board
  cards' bounded output, including ordinary shell history, plus scratchpad
  editing. Messages can reach a current Codex/Claude foreground even when the
  card was created as a shell or uses a wrapper/path. Shell input stays blocked.

The third choice requires its own confirmation on the Mac. It works on existing
cards without restarting sessions, editing saved commands or re-pairing. Each
snapshot advertises output, message, note and queue capabilities separately.

The hint under **Phone access** always describes the scope this Mac currently
keeps: what a paired phone can do now, what it cannot, and which choice adds
output. When a listed card lacks output, messages or notes only because of the
kept scope, the phone leads its list with a setup note naming the Mac and the
exact path (Deck → Settings → Remote access → Phone Connector → Phone access →
All card output and notes), and each such card says "not shared". The wire
code stays `unsupported-target`; the phone derives the remedy from the
advertised scope and the denied capability (queueing always names the saved
launch command instead). Each device row on the Mac also shows when that device
last received a snapshot from this process and the app build its HTTP client
announced (`DeckConnector/<build>`), kept in memory only; a phone talking to
another host, or still running an older build, is visible there.

**Trust this device.** Each active paired device can be trusted on the Mac
(device row, or the offer shown by name right after it pairs). A trusted device
gets exactly the third scope's access — all card output including shell
history, notes, and messages to a current Codex/Claude foreground — whatever
the global choice is, until trust is stopped or the device is revoked; other
devices keep the global choice. Trusting asks for confirmation naming the
device; stopping trust does not. Like a scope change, a trust change discards
in-flight reads and retires that device's unresolved commands. Trust adds no
shell input, no queueing without a saved launch command and no automation or
first-send authority; the target, generation and MCP fences still apply. The
flag is stored on the device record (omitted while false); an older Deck
ignores it, which narrows access rather than widening it.

The decision is meant to happen while the user is still at the Mac. Settings
states how many Board cards an untrusted phone cannot read output from under
the kept scope, and a pairing code shown under a narrower scope says that the
new phone can be trusted right there. A phone never widens its own access. On the phone, the setup note can be
collapsed with **Keep current sharing**; it stays one line for that host and
scope (a per-phone preference) and returns when the Mac's choice changes.
Queueing still requires a saved supported launch command because it may need to
restart a stopped session. A scope change retires unresolved phone operations;
re-enabling sharing does not replay them. Reads and writes recheck current access.

When enabled, deck listens on the selected private-network address over HTTPS. Eligible IPv4 addresses are RFC1918 private space on any interface; RFC6598 100.64/10 shared space only on a VPN tunnel interface (`utun*`, for example Tailscale); and 169.254/16 link-local space only on a bridge interface (`bridge*`, a direct Thunderbolt/USB cable network). A 100.64/10 or self-assigned 169.254/16 address on Wi-Fi or Ethernet is not offered, because carrier-grade NAT and DHCP-less public networks hand those out to strangers too; public addresses are never offered or accepted. deck records the interface (for example `en0`) that carried the address when you enabled Connector. Whenever the saved listener starts again, the address must still be available on that same interface; the same private address on a different interface is treated as a different network and Connector stays off until you enable it again.

While it runs, deck re-reads the Mac's interfaces about every 5 seconds. If the address is gone, has moved to another interface, or its netmask has changed, the listener stops and Settings shows "enabled, not listening"; disable and enable Connector to listen again (a restart of deck also listens again once the recorded interface carries the address). deck cannot see *which* network an interface has joined: moving to another Wi-Fi network that hands out the same address with the same netmask on the same interface — or losing and regaining the address between two checks — is not detected. Disable Connector before joining an untrusted network. Connections that do not arrive on the configured address are dropped unread.

Pairing uses a five-minute, single-use QR descriptor. The iOS app refuses a descriptor whose address is not an IPv4 address in the ranges above, or exactly `127.0.0.1` for the isolated smoke host (it cannot see the Mac's interface, so it checks only the range). Anyone who sees the QR code and can reach the Mac during those five minutes can pair, so the desktop shows each new pairing immediately (device name and time), hides the spent code, and lists every device with the time it was paired. Device names are stripped of bidi controls, zero-width characters and Unicode tag characters before they are validated and stored. The TLS private key remains in the macOS Keychain. The host journal stores only each device token's hash; the raw token exists on the host only in the one-time in-memory pairing response and is stored by the phone in the iOS Keychain. The desktop UI receives only the generated SVG, expiry, public fingerprint, and device summaries.

Changing to an address outside the current certificate requires **Reset and re-pair** while Connector is disabled. Reset removes the Connector identity, paired devices, and its remote-command journal. Revoking one device prevents its pending commands from running and drops its command history; a revoked device's slot is reused when a new device pairs and all 32 slots are taken.

## What the phone can send

The phone never supplies a shell command line, path, or command flag. That does not mean phone text cannot cause commands to run: text is delivered as a **prompt to an agent**, and an agent may act on it.

- **Output** in the two legacy scopes requires a saved supported Codex/Claude command and a live agent foreground. In BoardAccess it works on any live card, including a plain shell or unsupported agent. Both paths check the exact pane/foreground generation before and after capture and discard changed targets. The snapshot contains at most the recent 200 history lines plus visible pane content, capped at 64 KiB, and can include earlier shell history.
- **Send message** always requires a live recognized Codex/Claude foreground and the generation selected by the phone. A draft binds to the generation visible when its first character is typed; a draft written while no target was shared asks for explicit acceptance of the current target before it can be sent. BoardAccess additionally admits manually started agents regardless of saved command. Before each write, the host checks current card/scope, command/device authorization, generation, MCP control and the existing literal paste/Enter checks. Foreground recognition uses process names and kernel process identity; it is not binary-authenticity or Agent-readiness proof.
- **MCP-controlled sessions.** While an MCP client holds terminal control of a session (see [MCP](mcp.md)), the phone can neither send to it nor read its output — MCP output sharing is consent for the MCP client, not for the phone. A refused message reports `mcp-controlled` (not `unsupported-target`), and the phone explains a rejection by the operation it was, never by the current sharing scope. Taking control back on the Mac makes the card a phone target again.
- **Scratchpad reads and note add/edit/delete** are allowed for saved agent cards, or for every Board card under BoardAccess, including stopped cards. Edits retain revision/CAS checks and the claimed command is revalidated at the Board commit boundary under the lifecycle fence.
- **Queueing and queue pause/cancel** still require a saved card command launching `codex` or `claude` with simple safe arguments. Expanded direct access grants no automatic input or first-send authority and does not rewrite commands.

`GET /v1/snapshot` preserves saved Board order and reports independent capabilities.
Every GET rechecks device/feature authorization and the scope revision after reading;
withdrawal followed by regrant still discards an in-flight response. Older desktop
readers refuse the new scope enum, leaving its file untouched; missing scope keeps
the original default. iOS reads explicit capabilities and treats unknown/missing
capability entries as denied.

The output GET distinguishes a missing or unreadable session (`503
session-unavailable`), a pane whose foreground is not a supported Agent (`409
agent-not-in-foreground`), an MCP-controlled target (`409 mcp-controlled`), a target changed during capture (`409
context-changed`), and a probe/history/capture failure (`503
output-read-failed`). None means task completion. The phone keeps the card and
scratchpad, always refuses shell input, and can refresh the snapshot and
output after the target becomes readable again. The host logs the failure
stage, closed ErrorKind, per-run card tag, request counter and duration for
real read failures, without terminal text or underlying stderr. Older hosts'
`503 unavailable` stays an unknown read failure. Output GET never uses the
command journal's `410 expired` handling; operation IDs, sequences and replay
rules remain unchanged.

Remote commands first enter a bounded native journal and are then handled by the same serialized Board writer as desktop actions. Buffer edits use the visible buffer revision and manual-entry rules. Queue requests persist immutable copies before scheduler admission and retain deterministic operation IDs. Task creation starts the deterministic session before committing a card with its frozen initial plan; an unknown matching session is left as an ambiguous orphan and is never adopted. Connector rows carry no automatic-sending approval: a paired phone is not prompt authority, so every follow-up step of a Connector-created run waits for **Send now** on the desktop (`docs/scheduler-context-safety.md`, "Automation delivery authority").

When the paired Mac cannot be reached (asleep, Deck quit or Connector disabled, another network, or iOS Local Network access turned off for the app), the phone fails fast instead of waiting for connectivity, keeps already-read content marked as stale, and lists those checks with the Mac's name or paired address; it cannot tell them apart and does not guess. Only the one-time pairing request waits for connectivity, so it survives iOS's first Local Network prompt.

Connector uses the existing local network, including a VPN that already provides direct reachability and assigns an address in the allowed ranges above; it does not create a network, public relay, or APNs path. The iOS app refreshes when opened or returned to the foreground and does not promise background delivery. See the [iOS client README](../connector/ios/README.md) for the required Xcode, Simulator, signing, and physical-device gates.

## Limits and replay

The host holds at most 2,000 unresolved (accepted or executing) commands in a 16 MiB journal. Once a command reaches a final state it is reduced to a tombstone — its id, sequence, request hash, state, code and small result, without the request body — so resolved history never exhausts capacity. Replaying a known id with the same body returns the original result; a different body is rejected (`409`). At most 4,000 tombstones are kept; the oldest is dropped first. After any of a device's tombstones have been dropped, a query for an id the host no longer holds answers `410 expired` instead of `404`, because the host can no longer prove the id was never accepted. The phone records such an operation as `expired`: a final local state that is never queried, re-sent or given a new `seq`. It means only that the outcome can no longer be recovered — not that the command did not run.

Every command also carries `seq`, a per-device number the phone assigns once, stores with the command before its first POST and reuses on every retry (it is at least the phone's clock in milliseconds, so it stays ahead even if the app's local journal is lost). Dropping a tombstone raises the device's floor to that command's `seq`. `POST /v1/commands` itself enforces this: an unknown id whose `seq` is at or below the floor is `410 expired` and is never admitted — whether or not the phone asked `GET` first — and a `seq` already held by another retained id is `409`. Any higher `seq` is admitted as usual, so new work never stops. A command without `seq` (a phone build that predates it) is `426 upgrade-required`; update the app.

Buffer limits are 256 entries, 256 copies, 32 KiB per text, 1 MiB aggregate text, and 2 MiB serialized JSON. Command requests are limited to 256 KiB and are read only after the request's device token is authorized; pairing requests are limited to 4 KiB. The listener accepts at most 16 connections, at most 4 from one source address, with a 5-second TLS handshake, a 10-second header read and a 30-second connection limit. Encoded responses are limited to 4 MiB. Message and note text is at most 32 KiB, is not empty, and contains no control character except newline and tab — a pasted carriage return is refused on the phone before anything is sent, with a message saying so. Terminal output is at most 64 KiB of text per read. Result codes are 1–64 of `a-z`, `0-9` and `-`. A result marked ambiguous is not retried automatically.

The journal file is format version 3 (v3 adds each device's admission floor and each entry's `seq`). Version 1 and 2 files are upgraded when loaded (v1 resolved entries become tombstones) and written as version 3 on the next change; an older deck refuses a version 3 file instead of reading it. Paired devices, tokens and pending commands are kept.

The phone's own local command journal has a separate schema, unrelated to the host journal format or the wire protocol; it is now version 3 (v2 added the sequence counter, v3 the `expired` state). Version 1 and 2 files open with every command, id, body, `seq` and state unchanged — nothing becomes `expired` by migration — and are written as version 3 by the next save, which replaces the file atomically. A file of any other version is refused before its commands are read and left untouched; so is a v1/v2 file that claims `expired`. An older phone build refuses a version 3 file instead of reading it. A version-1 command still pending recovery has no `seq`, so the host would refuse it with `426`; it gets one only when the host answers its id with `404` (proof it was never admitted) — stored with the record before any retry, with the same id and body. A timeout, network error, `410`, `426` or any other answer never assigns one.

## Protocol fixtures and tests

The wire contract between the host (`app/src-tauri/src/connector/`) and the
phone (`connector/ios/Sources/DeckConnectorCore/`) is pinned by golden files
in `connector/ios/Tests/DeckConnectorCoreTests/Fixtures/`: valid and invalid
requests for every command kind, a snapshot, a buffer, terminal output, the
pairing descriptor, the pair response, the error envelope, the HTTP status →
phone error map, the result-code rule and the limits. The host's tests
produce the snapshot, buffer, output, descriptor and pair response from the
production functions and compare them with these files; the phone's tests
decode and validate the same bytes. **Changing the protocol means changing
the fixture in the same commit**, and both suites then fail until both sides
follow.

The Rust side runs in the ordinary `cargo test --workspace` gate. The Swift
side is not in CI (it needs the macOS Swift toolchain); run it locally before
committing any Connector protocol change:

```sh
cd connector/ios && swift test
```

Stop rule: if the next two Connector protocol changes land without a false
alarm from these fixtures, this is the final form — no JSON Schema, no
generated Swift models, no shared definitions crate. `WireRevision`
(integer-or-string) and `ExpectedGeneration` (null vs absent) carry meaning
and stay hand-written.
