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
        struct Fixture: Decodable { let pairingURI: String; let cardId: String; let shellCardId: String; let scope: String? }
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
        let shell = model.snapshot?.cards.first(where: { $0.id == fixture.shellCardId })
        if fixture.scope == "boardMetadata" {
            let shell = try XCTUnwrap(shell)
            XCTAssertFalse(shell.maySend || shell.canReadOutput || shell.canEditNotes || shell.mayQueue)
            await model.loadDetails(card: shell)
            XCTAssertNil(model.outputs[shell.id])
            XCTAssertNil(model.buffers[shell.id])
            XCTAssertEqual(model.outputUnavailable[shell.id], .unsupportedTarget)
        } else { XCTAssertNil(shell, "The default restricted scope must exclude ordinary shell cards.") }
        XCTAssertFalse(card.canSend)
        XCTAssertTrue(card.canQueue)
        #if DEBUG && targetEnvironment(simulator)
        let savedCredential = try XCTUnwrap(KeychainCredentialStore().load())
        let diagnosticClient = try DeckHTTPClient(credential: savedCredential)
        do {
            _ = try await diagnosticClient.output(cardID: fixture.shellCardId)
            XCTFail("Metadata sharing must never permit shell output.")
        } catch { XCTAssertEqual(error as? ConnectorError, .unsupportedTarget) }
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

private actor TestJournalStorage: JournalStorage {
    var value: JournalSnapshot?
    func load() async throws -> JournalSnapshot? { value }
    func save(_ snapshot: JournalSnapshot) async throws { value = snapshot }
}

private actor ControlledConnector: ConnectorTransport {
    nonisolated let hostID = "test-host"
    nonisolated func invalidate() {}
    var board: Snapshot
    var text = "A"
    var holdOutputs = false
    var holdQueries = false
    var outputCount = 0
    var queryCount = 0
    var postCount = 0
    var postState = "accepted"
    var reads: [Int: CheckedContinuation<TerminalOutput, Error>] = [:]
    var queries: [Int: CheckedContinuation<CommandResult, Error>] = [:]
    init() throws { board = try Self.snapshotValue(generation: "g1") }
    static func snapshotValue(generation: String, removed: Bool = false, notes: Bool = false) throws -> Snapshot {
        let card: [String: Any] = ["id": "C", "projectId": "P", "columnId": "K", "title": "Card", "status": "running", "generation": generation, "canSend": true, "canQueue": notes, "buffer": ["revision": 1, "collecting": false, "entryCount": 0]]
        return try JSONDecoder().decode(Snapshot.self, from: JSONSerialization.data(withJSONObject: ["version": 1, "hostId": "test-host", "revision": generation, "capturedAt": 1, "projects": [], "cards": removed ? [] : [card], "queue": []]))
    }
    static func outputValue(_ text: String, generation: String, cardID: String = "C") throws -> TerminalOutput {
        try JSONDecoder().decode(TerminalOutput.self, from: JSONSerialization.data(withJSONObject: ["cardId": cardID, "generation": generation, "revision": text, "capturedAt": 1, "text": text, "truncated": false]))
    }
    func snapshot() async throws -> Snapshot { board }
    func setBoard(_ value: Snapshot) { board = value }
    func setText(_ value: String) { text = value }
    func applyPostsImmediately() { postState = "applied" }
    func finishOutputs() {
        holdOutputs = false
        reads.values.forEach { $0.resume(throwing: URLError(.cancelled)) }; reads = [:]
    }
    func blockOutputs() { holdOutputs = true }
    func blockQueries() { holdQueries = true }
    func output(cardID: String) async throws -> TerminalOutput {
        outputCount += 1
        if holdOutputs { return try await withCheckedThrowingContinuation { reads[outputCount] = $0 } }
        return try Self.outputValue(text, generation: board.cards.first?.generation ?? "none")
    }
    func finishOutput(_ index: Int, text: String, generation: String) throws {
        reads.removeValue(forKey: index)?.resume(returning: try Self.outputValue(text, generation: generation))
    }
    func buffer(cardID: String) async throws -> CardBuffer {
        try JSONDecoder().decode(CardBuffer.self, from: Data(#"{"revision":1,"collecting":false,"entries":[]}"#.utf8))
    }
    func post(command: CommandRequest) async throws -> CommandResult {
        postCount += 1
        return try JSONDecoder().decode(CommandResult.self, from: JSONSerialization.data(withJSONObject: ["id": command.id, "state": postState]))
    }
    func query(id: String) async throws -> CommandResult {
        queryCount += 1
        if holdQueries { return try await withCheckedThrowingContinuation { queries[queryCount] = $0 } }
        return try JSONDecoder().decode(CommandResult.self, from: JSONSerialization.data(withJSONObject: ["id": id, "state": "delivered"]))
    }
    func finishQuery(_ index: Int) { queries.removeValue(forKey: index)?.resume(throwing: URLError(.cancelled)) }
    func finishQueries() {
        holdQueries = false
        queries.values.forEach { $0.resume(throwing: URLError(.cancelled)) }; queries = [:]
    }
}

extension AppModelTests {
    private func isolatedModel(_ client: ControlledConnector) async throws -> (AppModel, CommandJournal) {
        let journal = try await CommandJournal.open(storage: TestJournalStorage(), binding: JournalBinding(hostID: "test-host", deviceID: "test-device"))
        return (AppModel(client: client, journal: journal), journal)
    }

    func testLateOldGenerationCannotReplaceNewContent() async throws {
        let client = try ControlledConnector()
        let (model, _) = try await isolatedModel(client)
        await model.refresh()
        await client.blockOutputs()
        let firstCard = try XCTUnwrap(model.snapshot?.cards.first)
        let first = Task { await model.loadDetails(card: firstCard) }
        for _ in 0..<200 { if await client.outputCount == 1 { break }; try await Task.sleep(for: .milliseconds(5)) }
        assertCount( await client.outputCount, 1)
        let secondSnapshot = try ControlledConnector.snapshotValue(generation: "g2")
        model.applySnapshot(secondSnapshot)
        let second = Task { await model.loadDetails(card: secondSnapshot.cards[0]) }
        for _ in 0..<200 { if await client.outputCount == 2 { break }; try await Task.sleep(for: .milliseconds(5)) }
        try await client.finishOutput(2, text: "NEW", generation: "g2")
        await second.value
        try await client.finishOutput(1, text: "OLD", generation: "g1")
        await first.value
        XCTAssertEqual(model.outputs["C"]?.text, "NEW")
        model.setForeground(false)
    }

    func testPendingRecoveryDoesNotBlockSnapshotAndNeverReposts() async throws {
        let client = try ControlledConnector()
        let (model, journal) = try await isolatedModel(client)
        _ = try await journal.submit(CommandRequest(id: "original", kind: "send-message", cardId: "C", expectedGeneration: "g1", payload: ["text": .string("hello")]), draftCardID: nil, using: client)
        await client.blockQueries()
        await model.refresh()
        XCTAssertEqual(model.connection, .online)
        XCTAssertEqual(model.snapshot?.cards.first?.id, "C")
        assertCount( await client.postCount, 1)
        model.setForeground(false)
        await client.finishQueries()
    }

    func testResolvedNoteDoesNotEraseNewerEditWaitingForPersistence() async throws {
        let client = try ControlledConnector()
        let (model, journal) = try await isolatedModel(client)
        let submitted = NoteDraft(cardID: "C", entryID: nil, text: "same text")
        try await journal.saveNoteDraft(submitted)
        _ = try await journal.submit(CommandRequest(id: "note", kind: "buffer-add", cardId: "C", expectedRevision: "1", payload: ["text": .string(submitted.text)]), draftCardID: nil, noteDraftRevision: submitted.revision, using: client)
        _ = await journal.recover(using: client)
        let newer = NoteDraft(cardID: "C", entryID: nil, text: submitted.text)
        // The editor publishes synchronously; its coordinator can still be waiting
        // for the journal transaction when a resolved receipt refreshes the UI.
        model.noteDrafts[newer.storageKey] = newer
        await model.refresh()
        XCTAssertEqual(model.noteDrafts[newer.storageKey], newer)
        model.setForeground(false)
    }

    func testRestoredNoteCanBeSavedWithoutAnotherEdit() async throws {
        let client = try ControlledConnector()
        await client.setBoard(try ControlledConnector.snapshotValue(generation: "g1", notes: true))
        let (model, journal) = try await isolatedModel(client)
        let draft = NoteDraft(cardID: "C", entryID: nil, text: "Restored draft")
        try await journal.saveNoteDraft(draft)
        await model.refresh()
        let card = try XCTUnwrap(model.snapshot?.cards.first)
        await model.loadDetails(card: card)
        let restored = await model.loadNoteDraft(cardID: "C")
        XCTAssertEqual(restored, draft)
        let outcome = await model.bufferAdd(card: card, text: draft.text)
        if case let .failed(message) = outcome { XCTFail(message) }
        assertCount(await client.postCount, 1)
        await model.checkOriginalOperations()
        XCTAssertNil(model.noteDrafts[draft.storageKey])
        model.setForeground(false)
    }

    func testAppliedRestoredNoteSettlesWhileVisibleOutputReadIsSuspended() async throws {
        let client = try ControlledConnector()
        await client.setBoard(try ControlledConnector.snapshotValue(generation: "g1", notes: true))
        await client.applyPostsImmediately()
        let (model, journal) = try await isolatedModel(client)
        let draft = NoteDraft(cardID: "C", entryID: nil, text: "Restored while output is slow")
        try await journal.saveNoteDraft(draft)
        await model.refresh()
        let card = try XCTUnwrap(model.snapshot?.cards.first)
        await model.loadDetails(card: card)
        _ = await model.loadNoteDraft(cardID: "C")
        model.showDetails("C")
        await client.blockOutputs()
        let before = await client.outputCount
        let refresh = Task { await model.refresh() }
        for _ in 0..<200 {
            if await client.outputCount > before { break }
            try await Task.sleep(for: .milliseconds(5))
        }
        assertCount(await client.outputCount, before + 1)
        var outcome: AppModel.MutationOutcome?
        let save = Task { outcome = await model.bufferAdd(card: card, text: draft.text) }
        for _ in 0..<200 {
            if outcome != nil { break }
            try await Task.sleep(for: .milliseconds(5))
        }
        XCTAssertEqual(outcome, .applied, "An acknowledged save cannot wait for an unrelated remote read.")
        XCTAssertNil(model.noteDrafts[draft.storageKey])
        XCTAssertFalse(model.busyCards.contains(card.id))
        XCTAssertTrue(model.refreshing, "The read must still be suspended when the save settles.")
        assertCount(await client.postCount, 1)
        model.setForeground(false)
        await client.finishOutputs()
        await refresh.value
        await save.value
    }

    func testBackgroundSaveResumesConfirmationOnForegroundWithoutReposting() async throws {
        let client = try ControlledConnector()
        await client.setBoard(try ControlledConnector.snapshotValue(generation: "g1", notes: true))
        let (model, journal) = try await isolatedModel(client)
        await model.refresh()
        let card = try XCTUnwrap(model.snapshot?.cards.first)
        await model.loadDetails(card: card)
        let draft = NoteDraft(cardID: "C", entryID: nil, text: "Background receipt")
        try await journal.saveNoteDraft(draft)
        _ = await model.loadNoteDraft(cardID: "C")
        model.setForeground(false)
        let queriesBefore = await client.queryCount
        let outcome = await model.bufferAdd(card: card, text: draft.text)
        guard case .pending = outcome else { XCTFail("Expected an accepted operation."); return }
        try await Task.sleep(for: .milliseconds(30))
        assertCount(await client.queryCount, queriesBefore)
        XCTAssertNotNil(model.noteDrafts[draft.storageKey])
        model.setForeground(true)
        for _ in 0..<200 {
            if model.noteDrafts[draft.storageKey] == nil { break }
            try await Task.sleep(for: .milliseconds(5))
        }
        XCTAssertNil(model.noteDrafts[draft.storageKey])
        assertCount(await client.postCount, 1)
        assertCount(await client.queryCount, queriesBefore + 1)
        model.setForeground(false)
    }

    func testForegroundRefreshUpdatesOpenCardAndRemovedCardClearsContent() async throws {
        let client = try ControlledConnector()
        let (model, _) = try await isolatedModel(client)
        model.showDetails("C")
        await model.refresh()
        XCTAssertEqual(model.outputs["C"]?.text, "A")
        model.setForeground(false)
        await client.setText("B")
        model.setForeground(true)
        for _ in 0..<200 { if model.outputs["C"]?.text == "B" { break }; try await Task.sleep(for: .milliseconds(5)) }
        XCTAssertEqual(model.outputs["C"]?.text, "B")
        XCTAssertNotNil(model.lastSyncedAt)
        model.applySnapshot(try ControlledConnector.snapshotValue(generation: "g2", removed: true))
        XCTAssertNil(model.outputs["C"])
        model.setForeground(false)
    }

    func testCancelledRecoveryCannotClearReplacementRecovery() async throws {
        let client = try ControlledConnector()
        let (model, journal) = try await isolatedModel(client)
        _ = try await journal.submit(CommandRequest(id: "original", kind: "send-message", cardId: "C", expectedGeneration: "g1", payload: ["text": .string("hello")]), draftCardID: nil, using: client)
        await client.blockQueries()
        await model.refresh()
        for _ in 0..<200 { if await client.queryCount == 1 { break }; try await Task.sleep(for: .milliseconds(5)) }
        model.setForeground(false)
        model.setForeground(true)
        for _ in 0..<200 { if await client.queryCount == 2 { break }; try await Task.sleep(for: .milliseconds(5)) }
        await client.finishQuery(1)
        try await Task.sleep(for: .milliseconds(20))
        for _ in 0..<3 { await model.refresh() }
        assertCount(await client.queryCount, 2)
        assertCount(await client.postCount, 1)
        model.setForeground(false)
        await client.finishQueries()
    }

    func testDuplicateDetailLoadsShareOneRequest() async throws {
        let client = try ControlledConnector()
        let (model, _) = try await isolatedModel(client)
        await model.refresh()
        await client.blockOutputs()
        let card = try XCTUnwrap(model.snapshot?.cards.first)
        let first = Task { await model.loadDetails(card: card) }
        for _ in 0..<200 { if await client.outputCount == 1 { break }; try await Task.sleep(for: .milliseconds(5)) }
        let second = Task { await model.loadDetails(card: card) }
        try await Task.sleep(for: .milliseconds(20))
        assertCount(await client.outputCount, 1)
        try await client.finishOutput(1, text: "shared", generation: "g1")
        await first.value; await second.value
        XCTAssertEqual(model.outputs["C"]?.text, "shared")
        model.setForeground(false)
    }

    func testPairingTransitionClearsOldHostContentAndDraftMemory() async throws {
        let client = try ControlledConnector()
        let (model, _) = try await isolatedModel(client)
        model.showDetails("C"); await model.refresh()
        model.stageNoteDraft(cardID: "C", text: "old host note")
        XCTAssertNotNil(model.outputs["C"])
        await model.pair(descriptor: "invalid-pairing")
        XCTAssertNil(model.snapshot)
        XCTAssertTrue(model.outputs.isEmpty)
        XCTAssertTrue(model.noteDrafts.isEmpty)
        XCTAssertEqual(model.connection, .unpaired)
    }

    func testNetworkFailureMarksStaleButAuthorityFailureClearsOutput() throws {
        model.receiveOutput(cardID: "C", result: .success(try ControlledConnector.outputValue("A", generation: "g1")))
        model.receiveOutput(cardID: "C", result: .failure(URLError(.networkConnectionLost)))
        XCTAssertEqual(model.outputs["C"]?.text, "A")
        XCTAssertTrue(model.staleOutputs.contains("C"))
        model.receiveOutput(cardID: "C", result: .failure(ConnectorError.outputResponse(status: 409, code: "mcp-controlled")))
        XCTAssertNil(model.outputs["C"])
        XCTAssertEqual(model.outputUnavailable["C"], .controlled)
        model.receiveOutput(cardID: "C", result: .success(try ControlledConnector.outputValue("wrong card", generation: "g1", cardID: "OTHER")))
        XCTAssertNil(model.outputs["C"])
    }

    private func assertCount(_ awaitCount: Int, _ expected: Int, file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertTrue(awaitCount == expected, "Expected count \(expected), got \(awaitCount)", file: file, line: line)
    }
}
