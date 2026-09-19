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
        struct Fixture: Decodable { let pairingURI: String; let cardId: String }
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
        XCTAssertFalse(card.canSend)
        XCTAssertFalse(card.canQueue)
        await model.loadDetails(card: card)
        let before = try XCTUnwrap(model.buffers[card.id])

        let note = "simulator-smoke-\(UUID().uuidString.lowercased())"
        let addOutcome = await model.bufferAdd(card: card, text: note)
        let addResult = try await awaitTerminal(addOutcome)
        XCTAssertTrue(addResult == nil || addResult?.state == .applied)
        await model.loadDetails(card: card)
        let added = try XCTUnwrap(model.buffers[card.id]?.entries.first(where: { $0.text == note }))
        let editedText = note + "-edited"
        let editOutcome = await model.bufferEdit(card: card, entry: added, text: editedText)
        let editResult = try await awaitTerminal(editOutcome)
        XCTAssertTrue(editResult == nil || editResult?.state == .applied)
        await model.loadDetails(card: card)
        let edited = try XCTUnwrap(model.buffers[card.id]?.entries.first(where: { $0.id == added.id }))
        XCTAssertEqual(edited.text, editedText)

        model.buffers[card.id] = before
        let staleText = "stale-write-must-not-appear"
        let staleOutcome = await model.bufferAdd(card: card, text: staleText)
        let staleResult = try await awaitTerminal(staleOutcome)
        XCTAssertEqual(staleResult?.state, .rejected)
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
