# Phone Connector

Phone Connector is disabled by default. When explicitly enabled in **Settings → Integrations & automation**, deck listens on the selected private-network address over HTTPS. Pairing uses a five-minute, single-use QR descriptor. The TLS private key remains in the macOS Keychain. The host journal stores only each device token's hash; the raw token exists on the host only in the one-time in-memory pairing response and is stored by the phone in the iOS Keychain. The desktop UI receives only the generated SVG, expiry, public fingerprint, and device summaries.

Changing to an address outside the current certificate requires **Reset and re-pair** while Connector is disabled. Reset removes the Connector identity, paired devices, and its remote-command journal. Revoking one device prevents its pending commands from running. Disabling Connector stops accepting and claiming remote work; it does not clear ordinary deck queues.

Projects can define up to 50 phone task presets under **Project defaults**. A preset fixes its group, card title, directory, Codex or Claude launch command, and up to 20 initial steps on the Mac. The phone receives only each preset's ID and name and cannot supply executable text, paths, or command flags.

Remote commands first enter a bounded native journal and are then handled by the same serialized Board writer as desktop actions. Buffer edits use the visible buffer revision and manual-entry rules. Queue requests persist immutable copies before scheduler admission and retain deterministic operation IDs. Task creation starts the deterministic session before committing a card with its frozen initial plan; an unknown matching session is left as an ambiguous orphan and is never adopted.

Phone scratchpad queueing is offered only when the saved card command is a recognized Codex or Claude launch configuration and the snapshot reports `canQueue`. Other cards still support notes but reject remote queue requests. The phone cannot replace the saved command.

Connector uses the existing local network, including a VPN that already provides direct reachability; it does not create a network, public relay, or APNs path. The iOS app refreshes when opened or returned to the foreground and does not promise background delivery. See the [iOS client README](../connector/ios/README.md) for the required Xcode, Simulator, signing, and physical-device gates.

The host retains at most 2,000 command identities in a 16 MiB journal and does not recycle old IDs. Buffer limits are 256 entries, 256 copies, 32 KiB per text, 1 MiB aggregate text, and 2 MiB serialized JSON. HTTP requests are limited to 256 KiB and encoded responses to 4 MiB. A result marked ambiguous is not retried automatically.
