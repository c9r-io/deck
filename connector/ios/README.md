# Deck Connector for iOS

Native iOS 17 SwiftUI client for the local Deck Connector protocol. It contains no shell, tmux, raw PTY, background daemon, public relay, or terminal write API. Terminal output is a bounded read-only snapshot.

Scratchpad queueing is available only when the host reports `canQueue` for a recognized desktop-saved Codex or Claude launch configuration; a missing capability fails closed and the phone never supplies a shell command.

## Layout

- `Sources/DeckConnectorCore`: Foundation/Security wire models, pairing parser, exact-origin HTTP client, pinned TLS, one-value Keychain credential, bounded response loader, and durable command journal.
- `DeckConnectorApp`: SwiftUI task/scratchpad/queue UI plus AVFoundation QR scanner. The system keyboard microphone supplies dictation; the app does not request microphone access or run a Speech service.
- `Tests/DeckConnectorCoreTests`: macOS-runnable protocol and recovery tests.
- `DeckConnectorTests`: app-hosted XCTest coverage for the production `AppModel`, Simulator Keychain, and an opt-in real Deck loopback host.
- `DeckConnector.xcodeproj`: iOS app project with a shared `DeckConnector` scheme and local package dependency.

The app reconnects and refreshes snapshots when it returns to the foreground. It does not promise background connectivity. Pairing descriptors remain in memory only. The saved Keychain value binds origin, certificate pin, host ID, device ID, and bearer token with `WhenUnlockedThisDeviceOnly` accessibility. HTTP uses an ephemeral session with cookies, redirects, disk cache, and credential storage disabled. The self-signed leaf must match the pinned DER SHA-256 and also pass hostname and validity evaluation as the sole trust anchor.

Message drafts preserve the target generation they were created against. A refresh never silently changes that binding; if the desktop target changes, the user must explicitly accept the current generation before sending. Each command's immutable ID and body is atomically persisted before POST. An unknown result is recovered by querying that same ID and is never automatically retried under a new ID. Pending sends are visible and block another send for the card.

The journal is one atomic snapshot scoped by the Keychain credential's host ID and device ID. Unpairing leaves that protected directory as an archive and a new pairing receives a different scope, so old operations cannot be queried against another host or device. It keeps at most 1,000 local records in a 2 MiB file, reserves terminal-result space before POST, and only prunes locally confirmed resolved records; unresolved IDs are never evicted and the host retains its IDs independently. HTTP permits at most eight concurrent requests, cancels the underlying URLSession task with its Swift task, uses a 45-second resource timeout, and caps requests at 256 KiB and responses at 4 MiB. Pair descriptors are capped at 8 KiB before decoding, command text at 32 KiB UTF-8, serialized buffer responses at 2 MiB, and retained buffer text plus queued copies at 1 MiB.

## Checks available on this machine

```sh
cd connector/ios
swift test
```

The installed Command Line Tools 6.4 build intermittently omits its bundled `TestingMacros` plugin during automatic discovery. The verified fallback explicitly loads that same toolchain plugin:

```sh
swift test -j 1 \
  -Xswiftc -Xfrontend -Xswiftc -load-plugin-library \
  -Xswiftc -Xfrontend \
  -Xswiftc /Library/Developer/CommandLineTools/usr/lib/swift/host/plugins/testing/libTestingMacros.dylib
```

### Opt-in real Deck loopback transport smoke

This macOS-only package test is skipped unless an isolated Deck smoke fixture has already been authorized and launched. From the isolated smoke WK bridge, call `connector_smoke_transport({cardId})`; after it returns, use its `{path}` result. The mode-0600 private JSON contains exactly `{ "pairingURI": "deck-connector://pair?...", "cardId": "..." }`. Its card must be stopped, use `cmd: ""`, report `canQueue: false`, and belong to disposable smoke data. The test additionally rejects any fixture whose parsed origin host is not exactly `127.0.0.1`. The returned credential remains in memory and is never written to Keychain.

```sh
DECK_CONNECTOR_SMOKE_FIXTURE=/absolute/private/path/fixture.json \
swift test -j 1 --filter realDeckLoopbackHTTPSAndWKBridgeTransport \
  -Xswiftc -Xfrontend -Xswiftc -load-plugin-library \
  -Xswiftc -Xfrontend \
  -Xswiftc /Library/Developer/CommandLineTools/usr/lib/swift/host/plugins/testing/libTestingMacros.dylib
```

The fixture call returns only after the listener is bound and the one-use five-minute pairing is installed. Afterwards call `connector_disable()` and close only that exact isolated app, socket, session, and data root. The test uses a 25-second command deadline and verifies real loopback TLS/pinning, pairing, snapshot/output/buffer decoding, a 32 KiB escaped-Unicode note, idempotent replay, stale-revision rejection, and fail-closed buffer queueing. It does not log the QR payload, token, or note contents. It is not an iOS UI, Keychain, LAN, or device test.

## Xcode and Simulator checks

If Xcode is outside the default path, keep its selection scoped to the current shell:

```sh
export DEVELOPER_DIR=/path/to/Xcode.app/Contents/Developer
xcrun simctl list devices available
export DECK_SIMULATOR_ID='<UDID_OF_A_DISPOSABLE_BOOTED_SIMULATOR>'
xcodebuild -project connector/ios/DeckConnector.xcodeproj \
  -scheme DeckConnector \
  -destination "platform=iOS Simulator,id=$DECK_SIMULATOR_ID" \
  -derivedDataPath /tmp/deck-ios-derived-tests \
  test
xcodebuild -project connector/ios/DeckConnector.xcodeproj \
  -scheme DeckConnector -configuration Release -sdk iphoneos \
  -destination 'generic/platform=iOS' \
  -derivedDataPath /tmp/deck-ios-derived-device \
  CODE_SIGNING_ALLOWED=NO build
```

Use normal local signing for Simulator tests; `CODE_SIGNING_ALLOWED=NO` prevents the app from receiving the Simulator Keychain entitlement. The Debug configuration uses Xcode's standard `ONLY_ACTIVE_ARCH=YES` setting, which avoids asking a locally resolved Swift package product for an unused x86_64 slice on an Apple Silicon Simulator. It does not exclude an architecture from Release or device builds. A concrete disposable destination is required for app-hosted tests.

The app-hosted real-host test is skipped by default. First build the test runner and copy its generated test plan outside the repository:

```sh
xcodebuild -project connector/ios/DeckConnector.xcodeproj \
  -scheme DeckConnector \
  -destination "platform=iOS Simulator,id=$DECK_SIMULATOR_ID" \
  -derivedDataPath /tmp/deck-ios-derived-tests \
  build-for-testing
export DECK_XCTESTRUN_DIR=/tmp/deck-ios-derived-tests/Build/Products
export DECK_XCTESTRUN_SOURCE="$(find "$DECK_XCTESTRUN_DIR" -name '*.xctestrun' ! -name 'DeckConnector-smoke.xctestrun' -print -quit)"
cp "$DECK_XCTESTRUN_SOURCE" "$DECK_XCTESTRUN_DIR/DeckConnector-smoke.xctestrun"
```

After an authorized isolated Deck smoke host returns its private mode-0600 fixture path, inject only that path into the copied test plan and run the selected test before the five-minute pairing expires:

```sh
export DECK_CONNECTOR_SMOKE_FIXTURE=/absolute/private/path/connector-smoke-transport.json
python3 - "$DECK_CONNECTOR_SMOKE_FIXTURE" <<'PY'
import plistlib
import sys

path = "/tmp/deck-ios-derived-tests/Build/Products/DeckConnector-smoke.xctestrun"
with open(path, "rb") as source:
    plan = plistlib.load(source)
if "TestConfigurations" in plan:
    targets = [
        target
        for configuration in plan["TestConfigurations"]
        for target in configuration["TestTargets"]
    ]
else:
    targets = [
        target for name, target in plan.items()
        if name != "__xctestrun_metadata__" and isinstance(target, dict)
    ]
matches = [target for target in targets if target.get("BlueprintName") == "DeckConnectorTests"]
if len(matches) != 1:
    raise SystemExit("Expected exactly one DeckConnectorTests target in copied xctestrun")
matches[0].setdefault("EnvironmentVariables", {})["DECK_CONNECTOR_SMOKE_FIXTURE"] = sys.argv[1]
with open(path, "wb") as destination:
    plistlib.dump(plan, destination)
PY
xcodebuild -xctestrun "$DECK_XCTESTRUN_DIR/DeckConnector-smoke.xctestrun" \
  -destination "platform=iOS Simulator,id=$DECK_SIMULATOR_ID" \
  -only-testing:DeckConnectorTests/AppModelTests/testOptInRealHostPairingBufferCASAndCredentialLifecycle \
  test-without-building
```

Delete the private copied test plan after a pass or failure. The repository scheme remains secret-free, and no persistent Simulator service environment is changed.

The test rejects a fixture whose parsed HTTPS origin is not exactly `127.0.0.1` before opening a network request. It exercises production pairing, pinned HTTPS, snapshot and buffer decoding, buffer add/edit/delete, stale-revision rejection, original-operation recovery, Keychain restore into a new `AppModel`, and unpair. It never logs the descriptor, token, or note value. Use a disposable Simulator because setup clears that app's Deck pairing.

Simulator verification covers local signing, Simulator Keychain access, and loopback TLS through production code. It does not establish physical-device Keychain protection, camera behavior, LAN permission, or real Wi-Fi connectivity.

## Required physical-device gate

On a Mac with an assigned Development Team, install a signed development build on a physical iPhone and verify:

1. camera denial still leaves paste pairing usable;
2. correct host pairing succeeds, while wrong pin, expired QR, hostname/SAN mismatch, redirect, changed address, and revoked device fail closed;
3. the Keychain item is unavailable while locked and does not migrate to another device;
4. local-network permission denial and desktop sleep/offline states are actionable;
5. Codex and Claude Code tasks show bounded read-only output; system keyboard dictation remains editable before send;
6. restart/generation conflicts, stale buffer revisions, concurrent note edits, queue pause/cancel, unknown delivery recovery, foreground reconnect, and host removal never cause silent loss or duplicate send.

Do not add an ATS exception that permits arbitrary/self-signed certificates. The host certificate must include the paired LAN IP or DNS name in SAN; an address outside SAN requires desktop reset and re-pairing.

### Opt-in paired-device UI smoke

The shared `DeckConnectorUITests` scheme contains only the UI test target; it does not run `DeckConnectorTests` or reset the app's Keychain pairing, drafts, or data. The read-only output test opens one explicitly named card, checks that its output accessibility label contains a non-secret marker, exercises the Latest output control, backgrounds and reactivates the app, repeats the check, and attaches one card-detail screenshot. The separate note mutation test below requires an additional opt-in. A skipped opt-in test is not a pass—confirm that the named test executed.

The verified rerun uses a full Xcode `DEVELOPER_DIR`, builds once, copies the generated test plan beside the original under `Build/Products` so its `__TESTROOT__` paths remain valid, injects the three opt-in values only into the `DeckConnectorUITests` target's `EnvironmentVariables`, and runs without rebuilding. Inspect the generated `.xctestrun` before editing it because its plist shape varies by Xcode; shell variables passed only to `xcodebuild` are not a substitute for test-runner environment injection.

```sh
export DEVELOPER_DIR=/path/to/Xcode.app/Contents/Developer
export DECK_DEVICE_ID='<paired iPhone UDID>'
export DECK_UI_DERIVED=/tmp/deck-ios-ui-derived
export DECK_UI_PRODUCTS="$DECK_UI_DERIVED/Build/Products"

xcodebuild -project connector/ios/DeckConnector.xcodeproj \
  -scheme DeckConnectorUITests -configuration Debug \
  -destination "id=$DECK_DEVICE_ID" \
  -derivedDataPath "$DECK_UI_DERIVED" \
  build-for-testing

export DECK_UI_SOURCE="$(find "$DECK_UI_PRODUCTS" -name 'DeckConnectorUITests*.xctestrun' ! -name 'DeckConnectorUITests-paired-*.xctestrun' -print -quit)"
export DECK_UI_RUN="$DECK_UI_PRODUCTS/DeckConnectorUITests-paired-$(date +%Y%m%d-%H%M%S).xctestrun"
cp "$DECK_UI_SOURCE" "$DECK_UI_RUN"

# In DECK_UI_RUN, set these only for the DeckConnectorUITests target:
# DECK_UI_PAIRED_SMOKE=1
# DECK_UI_CARD_TITLE=<exact non-secret card title>
# DECK_UI_EXPECTED_OUTPUT=<non-secret expected marker>

export DECK_UI_RESULT="/tmp/DeckConnectorUITests-$(date +%Y%m%d-%H%M%S).xcresult"
xcodebuild -xctestrun "$DECK_UI_RUN" \
  -destination "id=$DECK_DEVICE_ID" \
  -only-testing:DeckConnectorUITests/ConnectorUITests/testPairedCardOutputSurvivesForegroundCycle \
  -resultBundlePath "$DECK_UI_RESULT" \
  test-without-building
```

Use a new result-bundle path for every run; do not pre-delete an existing result. Add `-allowProvisioningUpdates` to the build step only when the already authorized Team/device needs normal profile resolution—do not use this procedure to register additional devices by default. The verified run completed in 21.393 seconds with 1 test passed, 0 skipped, and 0 failed; require those exact counts rather than treating an opt-in skip as success. Its single card-detail screenshot supports visual review, while the assertion proves only that the complete accessibility label contains the marker, not that the marker's text range is visible inside the clipped viewport.

The separate `testPairedScratchpadNoteCRUD` test is an explicitly destructive sandbox-card check. In the copied UI-test plan, also set `DECK_UI_ALLOW_NOTE_MUTATION=1` and a fresh `DECK_UI_NOTE_MARKER=DECK-UI-NOTE-<unique-run>`, then select only `DeckConnectorUITests/ConnectorUITests/testPairedScratchpadNoteCRUD`. It adds that marker once, waits for mutation controls to unlock and checks the original add operation at most once when needed, proves the edit caret is at the end with an unsaved probe, clears the bounded printable-ASCII marker, accepts either an absent or empty accessibility value for the cleared SwiftUI editor, enters the marker plus `-edited`, and guards that exact value before saving. It then deletes only the matching row. A pending delete also uses Check original operation once; a pending edit acknowledges the sheet, performs one background/activate cycle, and uses the editor's own Check original operation control if confirmation has not yet dismissed it, without cancelling or saving again. On failure, do not rerun with the same marker or delete it blindly; leave it for host-side investigation and use a fresh marker only after explicit authorization. Require 1 passed, 0 skipped, and 0 failed.

To continue from an investigated add-only failure without creating another note, run only `DeckConnectorUITests/ConnectorUITests/testPairedScratchpadResumeEditDeleteTestNote` with the exact confirmed original marker. It refuses to edit unless that original is unique and its `-edited` form is absent, then applies the same exact-value checks before one edit save and one delete.

To remove a specifically investigated leftover test note, run only `DeckConnectorUITests/ConnectorUITests/testPairedScratchpadDeleteTestNote` with the same mutation opt-in values and the exact confirmed marker. This cleanup test never creates or edits a note. It refuses to act unless exactly one loaded scratchpad row matches the `DECK-UI-NOTE-<run>` value.
