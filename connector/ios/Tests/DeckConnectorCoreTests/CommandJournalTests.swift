import Foundation
import Testing
@testable import DeckConnectorCore

private actor ControlledStorage: JournalStorage {
    var snapshot: JournalSnapshot?
    var saveCalls = 0
    var failOnCall: Int?
    var blockNext = false
    private var blocked = false
    private var blockedWaiters: [CheckedContinuation<Void, Never>] = []
    private var releaseSave: CheckedContinuation<Void, Never>?

    init(snapshot: JournalSnapshot? = nil) { self.snapshot = snapshot }
    func load() async throws -> JournalSnapshot? { snapshot }
    func save(_ snapshot: JournalSnapshot) async throws {
        saveCalls += 1
        if failOnCall == saveCalls { throw MockFailure.storage }
        if blockNext {
            blockNext = false
            blocked = true
            blockedWaiters.forEach { $0.resume() }
            blockedWaiters = []
            await withCheckedContinuation { releaseSave = $0 }
            blocked = false
        }
        self.snapshot = snapshot
    }
    func arrangeBlockedSave() { blockNext = true }
    func arrangeFailure(call: Int) { failOnCall = call }
    func waitUntilBlocked() async { if blocked { return }; await withCheckedContinuation { blockedWaiters.append($0) } }
    func releaseBlockedSave() { releaseSave?.resume(); releaseSave = nil }
}

private enum MockFailure: Error { case offline, storage }

private actor MockTransport: CommandTransport {
    var postResult: Result<CommandResult, Error>
    var queryResult: Result<CommandResult, Error>
    var posted: [CommandRequest] = []
    var queried: [String] = []
    init(post: Result<CommandResult, Error>, query: Result<CommandResult, Error>) { postResult = post; queryResult = query }
    func post(command: CommandRequest) async throws -> CommandResult { posted.append(command); return try postResult.get() }
    func query(id: String) async throws -> CommandResult { queried.append(id); return try queryResult.get() }
    func setPost(_ result: Result<CommandResult, Error>) { postResult = result }
}

private actor DelayedPostTransport: CommandTransport {
    private var postWaiter: CheckedContinuation<CommandResult, Error>?
    private var posted = false
    private var observers: [CheckedContinuation<Void, Never>] = []
    let queryResult: Result<CommandResult, Error>
    init(queryResult: Result<CommandResult, Error>) { self.queryResult = queryResult }
    func post(command: CommandRequest) async throws -> CommandResult {
        posted = true; observers.forEach { $0.resume() }; observers = []
        return try await withCheckedThrowingContinuation { postWaiter = $0 }
    }
    func query(id: String) async throws -> CommandResult { try queryResult.get() }
    func waitUntilPosted() async { if posted { return }; await withCheckedContinuation { observers.append($0) } }
    func finish(_ result: CommandResult) { postWaiter?.resume(returning: result); postWaiter = nil }
}

private actor BlockingTransport: CommandTransport {
    private var postWaiter: CheckedContinuation<CommandResult, Error>?
    private var observed = false
    private var observers: [CheckedContinuation<Void, Never>] = []
    func post(command: CommandRequest) async throws -> CommandResult {
        observed = true; observers.forEach { $0.resume() }; observers = []
        return try await withCheckedThrowingContinuation { postWaiter = $0 }
    }
    func query(id: String) async throws -> CommandResult { throw MockFailure.offline }
    func waitUntilPosted() async { if observed { return }; await withCheckedContinuation { observers.append($0) } }
    func finish(_ result: CommandResult) { postWaiter?.resume(returning: result); postWaiter = nil }
}

private let binding = JournalBinding(hostID: "host", deviceID: "device")
private func request(_ id: String = "op-1", text: String = "hello") -> CommandRequest {
    CommandRequest(id: id, kind: "send-message", cardId: "card", expectedGeneration: "generation-1", payload: ["text": .string(text)])
}

@Test func stateWritesStayOrderedAcrossSuspendingStorage() async throws {
    let storage = ControlledStorage()
    let journal = try await CommandJournal.open(storage: storage, binding: binding)
    await storage.arrangeBlockedSave()
    let first = Task { try await journal.saveDraft(MessageDraft(cardID: "card", text: "first", expectedGeneration: "g")) }
    await storage.waitUntilBlocked()
    let second = Task { try await journal.saveDraft(MessageDraft(cardID: "card", text: "second", expectedGeneration: "g")) }
    await Task.yield()
    #expect(await storage.saveCalls == 1)
    await storage.releaseBlockedSave()
    try await first.value
    try await second.value
    #expect(await journal.draft(for: "card")?.text == "second")
    #expect(await storage.snapshot?.drafts["card"]?.text == "second")
}

@Test func failedPersistenceNeverCommitsMemory() async throws {
    let storage = ControlledStorage()
    await storage.arrangeFailure(call: 1)
    let journal = try await CommandJournal.open(storage: storage, binding: binding)
    do { try await journal.saveDraft(MessageDraft(cardID: "card", text: "lost", expectedGeneration: "g")); Issue.record("Expected storage failure") } catch { }
    #expect(await journal.draft(for: "card") == nil)
}

@Test func failedResultPersistenceLeavesPreparedIDForQueryRecovery() async throws {
    let storage = ControlledStorage()
    await storage.arrangeFailure(call: 2)
    let delivered = CommandResult(id: "op-1", state: .delivered, code: nil, result: nil)
    let transport = MockTransport(post: .success(delivered), query: .success(delivered))
    let journal = try await CommandJournal.open(storage: storage, binding: binding)
    do { _ = try await journal.submit(request(), draftCardID: "card", using: transport); Issue.record("Expected result persistence failure") } catch { }
    #expect(await journal.allRecords().first?.id == "op-1")
    #expect(await journal.allRecords().first?.localState == .prepared)
    await storage.arrangeFailure(call: -1)
    _ = await journal.recover(using: transport)
    #expect(await transport.posted.count == 1)
    #expect(await transport.queried == ["op-1"])
    #expect(await journal.allRecords().first?.localState == .resolved)
}

@Test func concurrentDraftEditDuringPendingPostSurvivesDeliveredReply() async throws {
    let storage = ControlledStorage()
    let transport = BlockingTransport()
    let journal = try await CommandJournal.open(storage: storage, binding: binding)
    try await journal.saveDraft(MessageDraft(cardID: "card", text: "first", expectedGeneration: "generation-1"))
    let submission = Task { try await journal.submit(request(text: "first"), draftCardID: "card", using: transport) }
    await transport.waitUntilPosted()
    try await journal.saveDraft(MessageDraft(cardID: "card", text: "second", expectedGeneration: "generation-1"))
    await transport.finish(CommandResult(id: "op-1", state: .delivered, code: nil, result: nil))
    _ = try await submission.value
    #expect(await journal.draft(for: "card")?.text == "second")
}

@Test func wrongResponseIDNeverUpdatesAnotherRecord() async throws {
    let storage = ControlledStorage()
    let wrong = CommandResult(id: "other", state: .delivered, code: nil, result: nil)
    let transport = MockTransport(post: .success(wrong), query: .success(wrong))
    let journal = try await CommandJournal.open(storage: storage, binding: binding)
    do { _ = try await journal.submit(request(), draftCardID: "card", using: transport); Issue.record("Expected invalid response") }
    catch let error as ConnectorError { #expect(error == .invalidResponse) }
    #expect(await journal.allRecords().map(\.id) == ["op-1"])
    #expect(await journal.allRecords().first?.localState == .unknown)
}

@Test func failedSecondPrepareCannotRemoveFirstRequestWhileItsPostIsRunning() async throws {
    let storage = ControlledStorage()
    await storage.arrangeFailure(call: 2)
    let firstTransport = BlockingTransport()
    let unusedTransport = MockTransport(post: .failure(MockFailure.offline), query: .failure(MockFailure.offline))
    let journal = try await CommandJournal.open(storage: storage, binding: binding)
    let first = Task { try await journal.submit(request("first"), draftCardID: "card", using: firstTransport) }
    await firstTransport.waitUntilPosted()
    do { _ = try await journal.submit(request("second"), draftCardID: "card", using: unusedTransport); Issue.record("Expected second prepare failure") } catch { }
    await firstTransport.finish(CommandResult(id: "first", state: .delivered, code: nil, result: nil))
    _ = try await first.value
    #expect(await journal.allRecords().map(\.id) == ["first"])
    #expect(await unusedTransport.posted.isEmpty)
}

@Test func journalRejectsDifferentCredentialBinding() async throws {
    let stored = JournalSnapshot(binding: binding)
    let storage = ControlledStorage(snapshot: stored)
    await #expect(throws: ConnectorError.conflict("journal-binding-mismatch")) {
        try await CommandJournal.open(storage: storage, binding: JournalBinding(hostID: "other", deviceID: "device"))
    }
}

@Test func delayedAcceptedPostCannotOverwriteDeliveredQuery() async throws {
    let delivered = CommandResult(id: "op-1", state: .delivered, code: nil, result: nil)
    let transport = DelayedPostTransport(queryResult: .success(delivered))
    let journal = try await CommandJournal.open(storage: ControlledStorage(), binding: binding)
    let submission = Task { try await journal.submit(request(), draftCardID: "card", using: transport) }
    await transport.waitUntilPosted()
    _ = await journal.recover(using: transport)
    await transport.finish(CommandResult(id: "op-1", state: .accepted, code: nil, result: nil))
    #expect(try await submission.value.state == .delivered)
    #expect(await journal.allRecords().first?.result?.state == .delivered)
}

@Test func failedQueryDoesNotEraseKnownAmbiguousResult() async throws {
    let ambiguous = CommandResult(id: "op-1", state: .ambiguous, code: "delivery-unknown", result: nil)
    let transport = MockTransport(post: .success(ambiguous), query: .failure(MockFailure.offline))
    let journal = try await CommandJournal.open(storage: ControlledStorage(), binding: binding)
    _ = try await journal.submit(request(), draftCardID: "card", using: transport)
    _ = await journal.recover(using: transport)
    #expect(await journal.allRecords().first?.result == ambiguous)
    #expect(await journal.allRecords().first?.localState == .ambiguous)
}

@Test func retryRequiresProvenNotFoundAndReusesImmutableRequest() async throws {
    let transport = MockTransport(post: .failure(MockFailure.offline), query: .failure(ConnectorError.commandNotFound))
    let journal = try await CommandJournal.open(storage: ControlledStorage(), binding: binding)
    do { _ = try await journal.submit(request(), draftCardID: "card", using: transport) } catch { }
    _ = await journal.recover(using: transport)
    #expect(await journal.allRecords().first?.localState == .notFound)
    await transport.setPost(.success(CommandResult(id: "op-1", state: .accepted, code: nil, result: nil)))
    _ = try await journal.retryNotFound(id: "op-1", using: transport)
    #expect(await transport.posted.map(\.id) == ["op-1", "op-1"])
}

@Test func fullArchivePrunesOnlyConfirmedResolvedRecord() async throws {
    let now = Date()
    var records = [CommandRecord(request: request("resolved"), localState: .resolved, result: CommandResult(id: "resolved", state: .delivered, code: nil, result: nil), createdAt: now, updatedAt: now)]
    records += (1..<ConnectorLimits.commandJournalEntries).map { index in
        CommandRecord(request: request("pending-\(index)"), localState: .unknown, result: nil, createdAt: now, updatedAt: now)
    }
    let storage = ControlledStorage(snapshot: JournalSnapshot(binding: binding, records: records))
    let transport = MockTransport(post: .failure(MockFailure.offline), query: .failure(MockFailure.offline))
    let journal = try await CommandJournal.open(storage: storage, binding: binding)
    do { _ = try await journal.submit(request("new"), draftCardID: "card", using: transport) } catch { }
    let saved = await journal.allRecords()
    #expect(saved.count == ConnectorLimits.commandJournalEntries)
    #expect(!saved.contains { $0.id == "resolved" })
    #expect(saved.contains { $0.id == "new" })
    #expect(saved.filter { $0.localState != .resolved }.count == ConnectorLimits.commandJournalEntries)
}

@Test func acceptedThenRecoveredAppliedUsesCanonicalResult() async throws {
    let accepted = CommandResult(id: "op-1", state: .accepted, code: nil, result: nil)
    let applied = CommandResult(id: "op-1", state: .applied, code: nil, result: nil)
    let transport = MockTransport(post: .success(accepted), query: .success(applied))
    let journal = try await CommandJournal.open(storage: ControlledStorage(), binding: binding)
    #expect(try await journal.submit(request(), draftCardID: "card", using: transport).state == .accepted)
    _ = await journal.recover(using: transport)
    #expect(await journal.canonicalResult(id: "op-1", consume: true)?.state == .applied)
}

@Test func recoveredAppliedClearsOnlyUnchangedSubmittedDraft() async throws {
    let accepted = CommandResult(id: "op-1", state: .accepted, code: nil, result: nil)
    let applied = CommandResult(id: "op-1", state: .applied, code: nil, result: nil)
    let transport = MockTransport(post: .success(accepted), query: .success(applied))
    let journal = try await CommandJournal.open(storage: ControlledStorage(), binding: binding)
    try await journal.saveDraft(MessageDraft(cardID: "card", text: "submitted", expectedGeneration: "generation-1"))
    _ = try await journal.submit(request(text: "submitted"), draftCardID: "card", using: transport)
    try await journal.saveDraft(MessageDraft(cardID: "card", text: "newer edit", expectedGeneration: "generation-1"))
    _ = await journal.recover(using: transport)
    #expect(await journal.canonicalResult(id: "op-1")?.state == .applied)
    #expect(await journal.draft(for: "card")?.text == "newer edit")
}

@Test func byteFullArchivePrunesResolvedAndReservesFinalResult() async throws {
    let now = Date()
    let largeText = String(repeating: "x", count: 30 * 1024)
    let records = (0..<70).map { index in
        CommandRecord(request: request("resolved-\(index)", text: largeText), localState: .resolved, result: CommandResult(id: "resolved-\(index)", state: .delivered, code: nil, result: nil), createdAt: now, updatedAt: now)
    }
    let storage = ControlledStorage(snapshot: JournalSnapshot(binding: binding, records: records))
    let applied = CommandResult(id: "new", state: .applied, code: nil, result: ["cardId": .string(String(repeating: "c", count: 128))])
    let transport = MockTransport(post: .success(applied), query: .success(applied))
    let journal = try await CommandJournal.open(storage: storage, binding: binding)
    _ = try await journal.submit(CommandRequest(id: "new", kind: "task-create", expectedRevision: "1", payload: ["projectId": .string("p"), "presetId": .string("x")]), draftCardID: nil, using: transport)
    let saved = try #require(await storage.snapshot)
    let encoder = JSONEncoder(); encoder.dateEncodingStrategy = .iso8601; encoder.outputFormatting = [.sortedKeys]
    #expect(try encoder.encode(saved).count <= ConnectorLimits.journalBytes)
    #expect(saved.records.count < records.count + 1)
    #expect(saved.records.last?.id == "new")
}

@Test func oversizedClosedResultIsRejectedWithoutLosingPreparedRequest() async throws {
    let bad = CommandResult(id: "op-1", state: .applied, code: nil, result: ["revision": .string(String(repeating: "r", count: 129))])
    let transport = MockTransport(post: .success(bad), query: .success(bad))
    let journal = try await CommandJournal.open(storage: ControlledStorage(), binding: binding)
    await #expect(throws: ConnectorError.invalidResponse) { try await journal.submit(request(), draftCardID: "card", using: transport) }
    #expect(await journal.allRecords().first?.localState == .prepared)
    #expect(await journal.allRecords().first?.result == nil)
}
