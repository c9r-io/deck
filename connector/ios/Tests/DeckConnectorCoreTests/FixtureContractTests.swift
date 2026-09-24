import Foundation
import Testing
@testable import DeckConnectorCore

// Golden wire fixtures shared with the Rust host (app/src-tauri/src/connector/
// tests.rs reads the same files with include_str!). The host produces or
// accepts each file; these tests decode and validate the same bytes. A
// protocol change updates the fixture, and both sides follow or fail.

private func fixture(_ name: String) throws -> Data {
    let parts = name.split(separator: ".", maxSplits: 1).map(String.init)
    let url = try #require(Bundle.module.url(forResource: parts[0], withExtension: parts[1], subdirectory: "Fixtures"))
    return try Data(contentsOf: url)
}

private func fixtureJSON(_ name: String) throws -> [String: Any] {
    try #require(JSONSerialization.jsonObject(with: fixture(name)) as? [String: Any])
}

private func requests(_ name: String) throws -> [[String: Any]] {
    try #require(fixtureJSON(name)["requests"] as? [[String: Any]])
}

@Test func limitsMatchTheHost() throws {
    let limits = try fixtureJSON("limits.json")
    let expected: [(String, Int)] = [
        ("request_bytes", ConnectorLimits.requestBytes),
        ("response_bytes", ConnectorLimits.responseBytes),
        ("command_text_utf8_bytes", ConnectorLimits.commandTextUTF8Bytes),
        ("output_text_utf8_bytes", ConnectorLimits.terminalTextUTF8Bytes),
        ("command_result_bytes", ConnectorLimits.commandResultBytes),
        ("pairing_descriptor_bytes", ConnectorLimits.pairingDescriptorBytes),
        ("buffer_entries", ConnectorLimits.bufferEntries),
        ("buffer_copies", ConnectorLimits.bufferCopies),
        ("buffer_entry_utf8_bytes", ConnectorLimits.bufferEntryUTF8Bytes),
        ("buffer_total_utf8_bytes", ConnectorLimits.bufferTotalUTF8Bytes),
        ("serialized_buffer_bytes", ConnectorLimits.serializedBufferBytes),
    ]
    for (key, value) in expected {
        #expect(limits[key] as? Int == value, "limits.json \(key)")
    }
    #expect(limits.count == expected.count, "every fixture limit is checked")
}

@Test func everyValidHostRequestDecodesValidatesAndRoundTrips() throws {
    let list = try requests("requests-valid.json")
    #expect(Set(list.compactMap { $0["kind"] as? String }) == ["send-message", "buffer-add", "buffer-edit", "buffer-delete", "buffer-queue", "task-create", "queue-pause", "queue-cancel"])
    for raw in list {
        let command = try JSONDecoder().decode(CommandRequest.self, from: JSONSerialization.data(withJSONObject: raw))
        try WireValidator.validate(command)
        #expect(try JSONDecoder().decode(CommandRequest.self, from: JSONEncoder().encode(command)) == command)
    }
}

@Test func invalidHostRequestsTheHostRefusesAreRefusedLocallyWhenThePhoneCanTell() throws {
    for entry in try requests("requests-invalid.json") {
        let reason = entry["reason"] as? String ?? "?"
        let raw = try #require(entry["request"] as? [String: Any])
        guard let command = try? JSONDecoder().decode(CommandRequest.self, from: JSONSerialization.data(withJSONObject: raw)) else {
            Issue.record("the phone cannot express: \(reason)")
            continue
        }
        if entry["phoneRejects"] as? Bool == true {
            #expect(throws: ConnectorError.invalidCommandText, "\(reason)") { try WireValidator.validate(command) }
        }
    }
}

@Test func hostSnapshotDecodesWithTheStatusWordsItEmits() throws {
    let snapshot = try JSONDecoder().decode(Snapshot.self, from: fixture("snapshot.json"))
    #expect(snapshot.cards.map(\.status) == ["running", "stopped", "unknown"])
    #expect(snapshot.cards.map(\.isStopped) == [false, true, false])
    let emitted = Set(snapshot.cards.map(\.status))
    #expect(CardSummary.stoppedStatuses.isSubset(of: emitted), "the phone special-cases a status the host never sends")
    #expect(snapshot.cards[1].generation == nil)
    #expect(snapshot.queue.first?.revision.value == "5")
}

@Test func hostBufferAndOutputDecodeAndValidate() throws {
    let buffer = try JSONDecoder().decode(CardBuffer.self, from: fixture("buffer.json"))
    try WireValidator.validate(buffer)
    #expect(buffer.entries.flatMap { $0.copies.map(\.state) } == ["delivered", "uncertain"])
    #expect(buffer.entries.last?.source?.type == "slack")
    let output = try JSONDecoder().decode(TerminalOutput.self, from: fixture("output.json"))
    try WireValidator.validate(output)
    #expect(output.text == "secret")
}

@Test func hostPairingDescriptorAndPairResponseParse() throws {
    let descriptor = try #require(String(data: fixture("pairing-descriptor.txt"), encoding: .utf8))
    let payload = try PairingDescriptor.parse(descriptor, now: Date(timeIntervalSince1970: 1_000))
    #expect(payload == PairingPayload(version: 1, hostId: "host-1", hostName: "Studio Mac", origin: "https://192.168.1.4:47631", fingerprint: String(repeating: "a", count: 64), code: "one-time-code", expiresAt: 2_000))
    let pair = try JSONDecoder().decode(PairResponse.self, from: fixture("pair-response.json"))
    #expect(pair.hostId == "host-1" && pair.deviceId == "device-1" && pair.token == "token-1" && pair.version == 1)
    let envelope = try JSONDecoder().decode(ErrorEnvelope.self, from: fixture("error-envelope.json"))
    #expect(envelope.error.code == "context-changed")
}

@Test func resultCodesFollowTheHostRule() throws {
    let codes = try fixtureJSON("result-codes.json")
    let request = CommandRequest(id: "s", kind: "send-message", cardId: "c", expectedGeneration: "g", payload: ["text": .string("hi")])
    for code in try #require(codes["valid"] as? [String]) {
        #expect(throws: Never.self, "\(code)") { try WireValidator.validate(CommandResult(id: "s", state: .rejected, code: code, result: nil), for: request) }
    }
    for code in try #require(codes["invalid"] as? [String]) {
        #expect(throws: ConnectorError.invalidResponse, "\(code)") { try WireValidator.validate(CommandResult(id: "s", state: .rejected, code: code, result: nil), for: request) }
    }
}
