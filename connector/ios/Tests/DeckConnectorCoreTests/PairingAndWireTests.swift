import Foundation
import Testing
@testable import DeckConnectorCore

@Test func pairingDescriptorParsesAndRejectsExpiry() throws {
    let payload = PairingPayload(version: 1, hostId: "host-1", hostName: "Studio Mac", origin: "https://192.168.1.4:9443", fingerprint: String(repeating: "a", count: 64), code: "one-time-code", expiresAt: 2_000)
    let data = try JSONEncoder().encode(payload)
    let descriptor = "deck-connector://pair?data=\(data.base64URLEncodedString)"
    #expect(try PairingDescriptor.parse(descriptor, now: Date(timeIntervalSince1970: 1_000)) == payload)
    #expect(throws: ConnectorError.expiredPairingDescriptor) { try PairingDescriptor.parse(descriptor, now: Date(timeIntervalSince1970: 2_001)) }
}

@Test func pairingDescriptorRejectsHTTPAndMalformedFingerprint() throws {
    let payload = PairingPayload(version: 1, hostId: "h", hostName: "Mac", origin: "http://host:80", fingerprint: "AA", code: "c", expiresAt: 4_000)
    let data = try JSONEncoder().encode(payload)
    #expect(throws: ConnectorError.invalidPairingDescriptor) { try PairingDescriptor.parse("deck-connector://pair?data=\(data.base64URLEncodedString)", now: Date(timeIntervalSince1970: 1)) }
}

@Test func pairingDescriptorAcceptsOnlyPrivateNetworkIPv4Origins() throws {
    func descriptor(_ origin: String) throws -> String {
        let payload = PairingPayload(version: 1, hostId: "h", hostName: "Mac", origin: origin, fingerprint: String(repeating: "a", count: 64), code: "c", expiresAt: 4_000)
        return "deck-connector://pair?data=\(try JSONEncoder().encode(payload).base64URLEncodedString)"
    }
    let now = Date(timeIntervalSince1970: 1)
    for host in ["10.0.0.1", "172.16.0.1", "172.31.255.254", "192.168.31.101", "169.254.20.4", "100.64.0.1", "100.127.255.254", "127.0.0.1"] {
        #expect(throws: Never.self) { try PairingDescriptor.parse(try descriptor("https://\(host):47631"), now: now) }
    }
    for host in ["8.8.8.8", "172.32.0.1", "100.63.255.255", "100.128.0.1", "127.0.0.2", "0.0.0.0", "192.168.001.4", "deck.local", "[fd00::1]"] {
        #expect(throws: ConnectorError.invalidPairingDescriptor, "\(host)") {
            try PairingDescriptor.parse(try descriptor("https://\(host):47631"), now: now)
        }
    }
}

@Test func pairingDescriptorRejectsOversizedInputBeforeDecodeAndNonURLAlphabet() {
    let oversized = "deck-connector://pair?data=" + String(repeating: "A", count: ConnectorLimits.pairingDescriptorBytes)
    #expect(throws: ConnectorError.invalidPairingDescriptor) { try PairingDescriptor.parse(oversized) }
    #expect(throws: ConnectorError.invalidPairingDescriptor) { try PairingDescriptor.parse("deck-connector://pair?data=abcd=") }
}

@Test func fileJournalChecksFileSizeBeforeJSONDecode() async throws {
    let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString, isDirectory: true)
    defer { try? FileManager.default.removeItem(at: directory) }
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    try Data(count: ConnectorLimits.journalBytes + 1).write(to: directory.appendingPathComponent("journal.json"))
    let storage = FileJournalStorage(directory: directory)
    await #expect(throws: ConnectorError.responseTooLarge) { try await storage.load() }
}

@Test func commandTextUsesExactUTF8Limit() throws {
    let allowed = CommandRequest(id: "a", kind: "send-message", cardId: "c", expectedGeneration: "g", payload: ["text": .string(String(repeating: "é", count: ConnectorLimits.commandTextUTF8Bytes / 2))])
    try WireValidator.validate(allowed)
    let oversized = CommandRequest(id: "b", kind: "buffer-add", cardId: "c", payload: ["text": .string(String(repeating: "é", count: ConnectorLimits.commandTextUTF8Bytes / 2 + 1))])
    #expect(throws: ConnectorError.requestTooLarge) { try WireValidator.validate(oversized) }
}

@Test func rustQueueAndExecutingFixturesRemainWireCompatible() throws {
    let snapshotJSON = #"{"version":1,"hostId":"h","revision":"8","capturedAt":1789776000,"projects":[],"cards":[{"id":"C1","projectId":"P1","columnId":"K1","title":"Stopped","status":"stopped","generation":null,"canSend":false,"buffer":{"revision":1,"collecting":false,"entryCount":0}}],"queue":[{"cardId":"C1","id":"Q1","mode":"once","paused":false,"revision":"7","state":"pending"}]}"#
    let snapshot = try JSONDecoder().decode(Snapshot.self, from: Data(snapshotJSON.utf8))
    #expect(snapshot.cards[0].generation == nil)
    #expect(snapshot.queue == [QueueItem(id: "Q1", cardId: "C1", mode: "once", state: "pending", paused: false, revision: WireRevision("7"))])

    let pause = CommandRequest(id: "pause-1", kind: "queue-pause", cardId: "C1", expectedGeneration: nil, payload: ["itemId": .string("Q1"), "paused": .bool(true), "revision": .string("1")])
    let object = try #require(JSONSerialization.jsonObject(with: JSONEncoder().encode(pause)) as? [String: Any])
    #expect(object.keys.contains("expectedGeneration"))
    #expect(object["expectedGeneration"] is NSNull)

    // Rust's external_state("executing") serializes as "accepted".
    let accepted = try JSONDecoder().decode(CommandResult.self, from: Data(#"{"id":"same","state":"accepted","code":null,"result":null}"#.utf8))
    #expect(accepted.state == .accepted)
}

@Test func commandResultUsesClosedBoundedPerKindSchema() throws {
    let queue = CommandRequest(id: "q", kind: "buffer-queue", cardId: "c", expectedRevision: "1", payload: [:])
    try WireValidator.validate(CommandResult(id: "q", state: .applied, code: nil, result: ["cardId": .string("c"), "revision": .string("2"), "queued": .integer(256)]), for: queue)
    #expect(throws: ConnectorError.invalidResponse) {
        try WireValidator.validate(CommandResult(id: "q", state: .rejected, code: "Not a closed code", result: nil), for: queue)
    }
    let send = CommandRequest(id: "s", kind: "send-message", cardId: "c", expectedGeneration: "g", payload: ["text": .string("hi")])
    #expect(throws: ConnectorError.invalidResponse) {
        try WireValidator.validate(CommandResult(id: "s", state: .delivered, code: nil, result: [:]), for: send)
    }
}

@Test func originMatchesSchemeHostPortAndPreservesEncodedID() throws {
    let origin = try HTTPSOrigin("https://Deck.Local:443/")
    #expect(origin.matches(try #require(URL(string: "https://deck.local/v1/snapshot"))))
    #expect(!origin.matches(try #require(URL(string: "https://deck.local:9443/v1/snapshot"))))
    #expect(!origin.matches(try #require(URL(string: "http://deck.local/v1/snapshot"))))
    #expect(!origin.matches(try #require(URL(string: "https://deck.local.evil/v1/snapshot"))))
    let encodedURL = try origin.endpoint("/v1/cards/a%2Fb/output")
    #expect(URLComponents(url: encodedURL, resolvingAgainstBaseURL: false)?.percentEncodedPath == "/v1/cards/a%2Fb/output")
}

@Test func bufferSchemaAcceptsIntegerRevisionAndEnforcesDesktopCapacity() throws {
    let text = String(repeating: "x", count: ConnectorLimits.bufferEntryUTF8Bytes + 1)
    let entry = BufferEntry(id: "e", kind: "manual", text: text, revision: 1, createdAt: .integer(1), updatedAt: .integer(1), source: nil, copies: [])
    let buffer = CardBuffer(revision: WireRevision("3"), collecting: false, entries: [entry])
    #expect(throws: ConnectorError.responseTooLarge) { try WireValidator.validate(buffer) }
    let json = #"{"revision":3,"collecting":false,"entries":[]}"#
    #expect(try JSONDecoder().decode(CardBuffer.self, from: Data(json.utf8)).revision.value == "3")
}

@Test func wireSchemaParsesSummaryWithoutBufferContents() throws {
    let json = #"{"version":1,"hostId":"h","revision":"8","capturedAt":1789776000,"projects":[{"id":"p","name":"Project","columns":[{"id":"c","name":"Doing"}],"presets":[{"id":"x","name":"Codex"}]}],"cards":[{"id":"card","projectId":"p","columnId":"c","title":"Fix","status":"running","generation":"g1","canSend":true,"buffer":{"revision":2,"collecting":false,"entryCount":3}}],"queue":[]}"#
    let snapshot = try JSONDecoder().decode(Snapshot.self, from: Data(json.utf8))
    #expect(snapshot.revision.value == "8")
    #expect(snapshot.cards.first?.buffer.entryCount == 3)
    #expect(snapshot.cards.first?.canQueue == false)
    #expect(snapshot.projects.first?.presets.first?.name == "Codex")
}

@Test func cardQueueCapabilityUsesExactFieldAndFailsClosedWhenMissing() throws {
    let card = #"{"id":"card","projectId":"p","columnId":"c","title":"Fix","status":"stopped","generation":null,"canSend":false,"canQueue":true,"buffer":{"revision":2,"collecting":false,"entryCount":1}}"#
    #expect(try JSONDecoder().decode(CardSummary.self, from: Data(card.utf8)).canQueue)
    let missing = card.replacingOccurrences(of: ",\"canQueue\":true", with: "")
    #expect(try JSONDecoder().decode(CardSummary.self, from: Data(missing.utf8)).canQueue == false)
}
