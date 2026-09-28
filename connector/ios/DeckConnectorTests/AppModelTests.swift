import XCTest
import DeckConnectorCore
@testable import DeckConnector

@MainActor
final class AppModelTests: XCTestCase {
    private var model: AppModel!

    override func setUp() async throws {
        #if !targetEnvironment(simulator)
        throw XCTSkip("AppModel host tests require a disposable iOS Simulator and never touch a physical-device Keychain.")
        #endif
        model = AppModel()
        model.unpair()
        XCTAssertEqual(model.connection, .unpaired)
    }

    override func tearDown() async throws {
        model?.unpair()
        model = nil
    }

    func testOutputIssueRecoversWhenAReadableOutputArrives() throws {
        model.receiveOutput(cardID: "C1", result: .failure(ConnectorError.outputResponse(status: 503, code: "session-unavailable")))
        XCTAssertEqual(model.outputUnavailable["C1"], .sessionUnavailable)
        model.receiveOutput(cardID: "C1", result: .failure(ConnectorError.outputResponse(status: 503, code: "unavailable")))
        XCTAssertEqual(model.outputUnavailable["C1"], .unknown,
                       "An old host's unavailable code must not be inferred as a missing session.")
        let json = """
        {"capturedAt":1789776000,"cardId":"C1","generation":"g","revision":"r","text":"live marker","truncated":false}
        """
        let output = try JSONDecoder().decode(TerminalOutput.self, from: Data(json.utf8))
        model.receiveOutput(cardID: "C1", result: .success(output))
        XCTAssertNil(model.outputUnavailable["C1"])
        XCTAssertEqual(model.outputs["C1"]?.text, "live marker")
        model.receiveOutput(cardID: "C1", result: .failure(ConnectorError.outputResponse(status: 409, code: "agent-not-in-foreground")))
        XCTAssertEqual(model.outputUnavailable["C1"], .agentNotInForeground)
        XCTAssertNil(model.outputs["C1"], "A later unreadable target cannot retain captured output.")
    }

    private func awaitTerminal(
        _ outcome: AppModel.MutationOutcome,
        timeout: Duration = .seconds(15)
    ) async throws -> CommandResult? {
        switch outcome {
        case .applied:
            return nil
        case let .failed(message):
            XCTFail("Command failed before acceptance: \(message)")
            return nil
        case let .pending(id, _):
            let operationID = try XCTUnwrap(id)
            let clock = ContinuousClock()
            let deadline = clock.now.advanced(by: timeout)
            while clock.now < deadline {
                await model.checkOriginalOperations()
                if let result = model.operationReceipts[operationID],
                   [.applied, .delivered, .rejected].contains(result.state) {
                    XCTAssertEqual(result.id, operationID)
                    return result
                }
                try await Task.sleep(for: .milliseconds(100))
            }
            XCTFail("Original operation did not reach a terminal state.")
            return nil
        }
    }

    func testFreshStartRemainsUnpaired() async {
        await model.start()
        XCTAssertEqual(model.connection, .unpaired)
        XCTAssertNil(model.snapshot)
    }

    func testMalformedPairingDescriptorFailsClosedAndRemainsActionable() async {
        await model.pair(descriptor: "not-a-deck-pairing-descriptor")
        XCTAssertEqual(model.connection, .unpaired)
        XCTAssertNotNil(model.message)
        XCTAssertNil(model.snapshot)
    }

    func testOptInRealHostPairingBufferCASAndCredentialLifecycle() async throws {
        guard let fixturePath = ProcessInfo.processInfo.environment["DECK_CONNECTOR_SMOKE_FIXTURE"],
              !fixturePath.isEmpty, fixturePath != "$(DECK_CONNECTOR_SMOKE_FIXTURE)" else {
            throw XCTSkip("Set DECK_CONNECTOR_SMOKE_FIXTURE in the disposable Simulator's launch environment.")
        }
        struct Fixture: Decodable { let pairingURI: String; let cardId: String; let shellCardId: String }
        // Xcode may reset the app container after installing the test host.
        for _ in 0..<100 where !FileManager.default.fileExists(atPath: fixturePath) {
            try await Task.sleep(for: .milliseconds(100))
        }
        let attributes = try FileManager.default.attributesOfItem(atPath: fixturePath)
        guard let size = attributes[.size] as? NSNumber, size.intValue <= ConnectorLimits.pairingDescriptorBytes * 2 else {
            throw ConnectorError.invalidPairingDescriptor
        }
        let data = try Data(contentsOf: URL(fileURLWithPath: fixturePath), options: [.mappedIfSafe])
        let fixture = try JSONDecoder().decode(Fixture.self, from: data)
        let pairing = try PairingDescriptor.parse(fixture.pairingURI)
        guard try HTTPSOrigin(pairing.origin).host == "127.0.0.1" else { throw ConnectorError.invalidOrigin }

        await model.pair(descriptor: fixture.pairingURI)
        XCTAssertEqual(model.connection, .online)
        let card = try XCTUnwrap(model.snapshot?.cards.first(where: { $0.id == fixture.cardId }))
        XCTAssertFalse(model.snapshot?.cards.contains(where: { $0.id == fixture.shellCardId }) ?? true,
                       "The persisted ordinary shell card must be filtered from the host snapshot.")
        XCTAssertFalse(card.canSend)
        XCTAssertTrue(card.canQueue)
        #if DEBUG && targetEnvironment(simulator)
        let savedCredential = try XCTUnwrap(KeychainCredentialStore().load())
        let diagnosticClient = try DeckHTTPClient(credential: savedCredential)
        let outputHTTP = try await diagnosticClient.diagnosticOutputHTTPResult(cardID: card.id)
        XCTAssertEqual(outputHTTP.status, 503, "Stopped fixture card must return HTTP 503.")
        XCTAssertEqual(outputHTTP.code, "session-unavailable", "Stopped fixture card must carry the session code.")
        #endif
        await model.loadDetails(card: card)
        XCTAssertEqual(model.outputUnavailable[card.id], .sessionUnavailable,
                       "The same fixture card's HTTP error must reach AppModel as a structured state.")
        let before = try XCTUnwrap(model.buffers[card.id])

        let note = "simulator-smoke-\(UUID().uuidString.lowercased())"
        let addOutcome = await model.bufferAdd(card: card, text: note)
        if ProcessInfo.processInfo.environment["DECK_CONNECTOR_EXPECT_POST_ACCEPT_FAILURE"] == "1" {
            guard case .failed = addOutcome else {
                XCTFail("The post-accept fault must make the original response unavailable.")
                return
            }
            let operationID = try XCTUnwrap(model.pendingCardCommands[card.id]?.first?.id)
            let clock = ContinuousClock(), deadline = clock.now.advanced(by: .seconds(15))
            while clock.now < deadline, model.operationReceipts[operationID]?.state != .applied {
                await model.checkOriginalOperations()
                try await Task.sleep(for: .milliseconds(100))
            }
            XCTAssertEqual(model.operationReceipts[operationID]?.state, .applied)
            XCTAssertTrue(model.pendingCardCommands[card.id]?.isEmpty ?? true)
        } else {
            let addResult = try await awaitTerminal(addOutcome)
            XCTAssertTrue(addResult == nil || addResult?.state == .applied)
        }
        await model.loadDetails(card: card)
        let addedMatches = model.buffers[card.id]?.entries.filter { $0.text == note } ?? []
        XCTAssertEqual(addedMatches.count, 1, "Recovering the original operation must not duplicate its note.")
        let added = try XCTUnwrap(addedMatches.first)
        let editedText = note + "-edited"
        let editOutcome = await model.bufferEdit(card: card, entry: added, text: editedText)
        let editResult = try await awaitTerminal(editOutcome)
        XCTAssertTrue(editResult == nil || editResult?.state == .applied)
        await model.loadDetails(card: card)
        let edited = try XCTUnwrap(model.buffers[card.id]?.entries.first(where: { $0.id == added.id }))
        XCTAssertEqual(edited.text, editedText)

        await model.checkOriginalOperations()
        XCTAssertTrue(model.pendingCardCommands[card.id]?.isEmpty ?? true,
                      "The prior note edit must settle before the stale-revision probe.")
        model.buffers[card.id] = before
        let staleText = "stale-write-must-not-appear"
        let staleOutcome = await model.bufferAdd(card: card, text: staleText)
        switch staleOutcome {
        case let .failed(staleCode):
            XCTAssertEqual(staleCode, "revision-changed")
            XCTAssertEqual(model.message, "revision-changed")
        case .pending:
            let staleResult = try await awaitTerminal(staleOutcome)
            XCTAssertEqual(staleResult?.state, .rejected)
            XCTAssertEqual(staleResult?.code, "revision-changed")
        case .applied:
            XCTFail("A stale scratchpad revision must never be applied.")
        }
        await model.loadDetails(card: card)
        XCTAssertFalse(model.buffers[card.id]?.entries.contains(where: { $0.text == staleText }) ?? true)

        let currentEdited = try XCTUnwrap(model.buffers[card.id]?.entries.first(where: { $0.id == edited.id }))
        let deleteOutcome = await model.bufferDelete(card: card, entry: currentEdited)
        let deleteResult = try await awaitTerminal(deleteOutcome)
        XCTAssertTrue(deleteResult == nil || deleteResult?.state == .applied)
        await model.loadDetails(card: card)
        XCTAssertFalse(model.buffers[card.id]?.entries.contains(where: { $0.id == added.id }) ?? true)
        let finalCount = model.buffers[card.id]?.entries.count
        await model.checkOriginalOperations()
        await model.loadDetails(card: try XCTUnwrap(model.snapshot?.cards.first(where: { $0.id == card.id })))
        XCTAssertEqual(model.buffers[card.id]?.entries.count, finalCount)

        let restored = AppModel()
        await restored.start()
        XCTAssertEqual(restored.connection, .online)
        XCTAssertEqual(restored.snapshot?.hostId, pairing.hostId)
        restored.unpair()
        let afterUnpair = AppModel()
        await afterUnpair.start()
        XCTAssertEqual(afterUnpair.connection, .unpaired)
    }
}
