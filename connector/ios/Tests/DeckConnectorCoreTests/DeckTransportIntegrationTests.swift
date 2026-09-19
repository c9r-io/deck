import Foundation
import Testing
@testable import DeckConnectorCore

private struct SmokeFixture: Decodable { let pairingURI: String; let cardId: String }
private let smokeFixturePath = ProcessInfo.processInfo.environment["DECK_CONNECTOR_SMOKE_FIXTURE"]

private func boundaryNote(operationID: String) -> String {
    let prefix = "ios-smoke-\(operationID)-\"\\-"
    let remaining = ConnectorLimits.commandTextUTF8Bytes - prefix.utf8.count
    return prefix + String(repeating: "界", count: remaining / 3) + String(repeating: "x", count: remaining % 3)
}

private func waitForTerminal(_ client: DeckHTTPClient, id: String) async throws -> CommandResult {
    let clock = ContinuousClock(), deadline = clock.now.advanced(by: .seconds(25))
    while clock.now < deadline {
        let result = try await client.query(id: id)
        if result.state != .accepted { return result }
        try await Task.sleep(for: .milliseconds(150))
    }
    throw ConnectorError.transport("Timed out waiting for the original smoke operation.")
}

@Test(.enabled(if: smokeFixturePath != nil, "Set DECK_CONNECTOR_SMOKE_FIXTURE to the private isolated fixture JSON to run."))
func realDeckLoopbackHTTPSAndWKBridgeTransport() async throws {
    let path = try #require(smokeFixturePath)
    let fixture = try JSONDecoder().decode(SmokeFixture.self, from: Data(contentsOf: URL(fileURLWithPath: path), options: [.mappedIfSafe]))
    let pairing = try PairingDescriptor.parse(fixture.pairingURI)
    let smokeOrigin = try HTTPSOrigin(pairing.origin)
    guard smokeOrigin.host == "127.0.0.1" else { throw ConnectorError.invalidOrigin }

    let wrongPin = PairingPayload(version: pairing.version, hostId: pairing.hostId, hostName: pairing.hostName, origin: pairing.origin, fingerprint: String(repeating: pairing.fingerprint.first == "0" ? "1" : "0", count: 64), code: pairing.code, expiresAt: pairing.expiresAt)
    let wrongClient = try DeckHTTPClient(pairing: wrongPin)
    await #expect(throws: (any Error).self) { try await wrongClient.pair(code: wrongPin.code, deviceName: "Deck iOS smoke wrong pin") }
    wrongClient.invalidate()

    let pairingClient = try DeckHTTPClient(pairing: pairing)
    let response = try await pairingClient.pair(code: pairing.code, deviceName: "Deck iOS transport smoke")
    pairingClient.invalidate()
    #expect(response.hostId == pairing.hostId)
    let client = try DeckHTTPClient(credential: DeviceCredential(origin: pairing.origin, fingerprint: pairing.fingerprint, hostId: response.hostId, deviceId: response.deviceId, token: response.token))
    defer { client.invalidate() }

    let initialSnapshot = try await client.snapshot()
    let card = try #require(initialSnapshot.cards.first(where: { $0.id == fixture.cardId }))
    #expect(!card.canQueue)
    #expect(!card.canSend)
    let initialQueue = initialSnapshot.queue.filter { $0.cardId == fixture.cardId }
    if let output = try? await client.output(cardID: fixture.cardId) {
        #expect(output.cardId == fixture.cardId)
        #expect(output.text.utf8.count <= ConnectorLimits.terminalTextUTF8Bytes)
    }

    let initialBuffer = try await client.buffer(cardID: fixture.cardId)
    let addID = UUID().uuidString.lowercased(), note = boundaryNote(operationID: addID)
    #expect(note.utf8.count == ConnectorLimits.commandTextUTF8Bytes)
    let add = CommandRequest(id: addID, kind: "buffer-add", cardId: fixture.cardId, expectedRevision: initialBuffer.revision.value, payload: ["text": .string(note)])
    _ = try await client.post(command: add)
    #expect(try await waitForTerminal(client, id: addID).state == .applied)
    let afterAdd = try await client.buffer(cardID: fixture.cardId)
    #expect(afterAdd.entries.filter { $0.text == note }.count == 1)

    _ = try await client.post(command: add)
    #expect(try await waitForTerminal(client, id: addID).state == .applied)
    #expect(try await client.buffer(cardID: fixture.cardId).entries.filter { $0.text == note }.count == 1)

    let changed = CommandRequest(id: addID, kind: "buffer-add", cardId: fixture.cardId, expectedRevision: initialBuffer.revision.value, payload: ["text": .string("changed body")])
    var changedRejected = false
    do { changedRejected = try await client.post(command: changed).state == .rejected } catch { changedRejected = true }
    #expect(changedRejected)
    #expect(try await client.query(id: addID).state == .applied)
    #expect(try await client.buffer(cardID: fixture.cardId).entries.filter { $0.text == note }.count == 1)

    let staleID = UUID().uuidString.lowercased()
    let stale = CommandRequest(id: staleID, kind: "buffer-add", cardId: fixture.cardId, expectedRevision: initialBuffer.revision.value, payload: ["text": .string("must not appear")])
    _ = try await client.post(command: stale)
    #expect(try await waitForTerminal(client, id: staleID).state == .rejected)
    #expect(!(try await client.buffer(cardID: fixture.cardId)).entries.contains { $0.text == "must not appear" })

    let current = try await client.buffer(cardID: fixture.cardId)
    let added = try #require(current.entries.first(where: { $0.text == note }))
    let queueID = UUID().uuidString.lowercased()
    let queue = CommandRequest(id: queueID, kind: "buffer-queue", cardId: fixture.cardId, expectedGeneration: nil, expectedRevision: current.revision.value, payload: ["entryIds": .strings([added.id])])
    _ = try await client.post(command: queue)
    #expect(try await waitForTerminal(client, id: queueID).state == .rejected)
    let finalBuffer = try await client.buffer(cardID: fixture.cardId)
    #expect(finalBuffer.entries.first(where: { $0.id == added.id })?.copies.count == added.copies.count)
    #expect(try await client.snapshot().queue.filter { $0.cardId == fixture.cardId } == initialQueue)
}
