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

@Test func textTheHostWouldRefuseIsRejectedBeforeJournalingOrSending() async throws {
    let delivered = CommandResult(id: "op-1", state: .delivered, code: nil, result: nil)
    let transport = MockTransport(post: .success(delivered), query: .success(delivered))
    let journal = try await CommandJournal.open(storage: ControlledStorage(), binding: binding)
    let pasted = CommandRequest(id: "op-1", kind: "send-message", cardId: "card", expectedGeneration: "g", payload: ["text": .string("pasted\r\nline")])
    await #expect(throws: ConnectorError.invalidCommandText) { try await journal.submit(pasted, draftCardID: "card", using: transport) }
    #expect(await transport.posted.isEmpty)
    #expect(await journal.allRecords().isEmpty)
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
    let sequences = await transport.posted.map(\.seq)
    #expect(sequences.first != nil && sequences.first! != nil)
    #expect(Set(sequences).count == 1, "a retry reuses the original admission sequence")
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

@Test func submitPersistsAnAdmissionSequenceBeforeTheFirstPost() async throws {
    let storage = ControlledStorage()
    let transport = MockTransport(post: .failure(MockFailure.offline), query: .failure(MockFailure.offline))
    let journal = try await CommandJournal.open(storage: storage, binding: binding)
    let clock = UInt64(Date().timeIntervalSince1970 * 1000)
    do { _ = try await journal.submit(request("op-1"), draftCardID: "card", using: transport) } catch { }
    do { _ = try await journal.submit(request("op-2"), draftCardID: "card", using: transport) } catch { }
    let posted = await transport.posted
    let first = try #require(posted.first?.seq)
    let second = try #require(posted.last?.seq)
    #expect(first >= clock, "sequences stay above an earlier installation's")
    #expect(second > first)
    let saved = try #require(await storage.snapshot)
    #expect(saved.version == 3)
    #expect(saved.nextSequence == second + 1)
    #expect(saved.records.map(\.request.seq) == [first, second], "persisted before the POST")
    // The caller's unsequenced body still identifies the same operation.
    await #expect(throws: ConnectorError.transport("The original operation is pending recovery.")) {
        try await journal.submit(request("op-1"), draftCardID: "card", using: transport)
    }
    await #expect(throws: ConnectorError.conflict("operation-id-reused")) {
        try await journal.submit(request("op-1", text: "changed"), draftCardID: "card", using: transport)
    }
    #expect(await transport.posted.count == 2)
}

@Test func versionOneJournalUpgradesWithoutRewritingItsRecords() async throws {
    let old = CommandRecord(request: request("legacy"), localState: .unknown, result: nil, createdAt: Date(), updatedAt: Date())
    var encoded = try JSONSerialization.jsonObject(with: JSONEncoder().encode(JournalSnapshot(binding: binding, records: [old]))) as! [String: Any]
    encoded["version"] = 1
    encoded.removeValue(forKey: "nextSequence")
    let decoder = JSONDecoder(); decoder.dateDecodingStrategy = .deferredToDate
    let stored = try decoder.decode(JournalSnapshot.self, from: JSONSerialization.data(withJSONObject: encoded))
    #expect(stored.version == 1 && stored.nextSequence == 1)
    let storage = ControlledStorage(snapshot: stored)
    let journal = try await CommandJournal.open(storage: storage, binding: binding)
    #expect(await journal.allRecords().first?.request.seq == nil, "an old body stays immutable")
    let transport = MockTransport(post: .success(CommandResult(id: "next", state: .accepted, code: nil, result: nil)), query: .failure(MockFailure.offline))
    _ = try await journal.submit(request("next"), draftCardID: "card", using: transport)
    #expect(await storage.snapshot?.version == 3)
    #expect(await transport.posted.first?.seq != nil)
}

// ---- Legacy (version-1) records and 410 expiry ----

/// Answers like the host: 404 until admitted, then the admitted state; logs
/// what the journal had persisted when each POST started.
private actor RecordingHost: CommandTransport {
    let storage: ControlledStorage
    var queryError: ConnectorError? = .commandNotFound
    var postError: ConnectorError?
    var queried: [String] = []
    var posted: [CommandRequest] = []
    var persistedAtPost: [CommandRecord?] = []
    init(storage: ControlledStorage) { self.storage = storage }
    func setQueryError(_ error: ConnectorError?) { queryError = error }
    func setPostError(_ error: ConnectorError?) { postError = error }
    func query(id: String) async throws -> CommandResult {
        queried.append(id)
        if let queryError { throw queryError }
        return CommandResult(id: id, state: .accepted, code: nil, result: nil)
    }
    func post(command: CommandRequest) async throws -> CommandResult {
        posted.append(command)
        persistedAtPost.append(await storage.snapshot?.records.first { $0.id == command.id })
        if let postError { throw postError }
        return CommandResult(id: command.id, state: .accepted, code: nil, result: nil)
    }
}

private func legacyJournal(_ ids: [String], state: CommandRecord.LocalState = .unknown) -> JournalSnapshot {
    let records = ids.map { CommandRecord(request: request($0), localState: state, result: nil, createdAt: Date(), updatedAt: Date()) }
    return JournalSnapshot(binding: binding, records: records)
}

@Test func legacyRecordGetsASequenceOnlyFromAHost404AndPersistsItBeforeTheRetry() async throws {
    let storage = ControlledStorage(snapshot: legacyJournal(["legacy"]))
    let host = RecordingHost(storage: storage)
    let journal = try await CommandJournal.open(storage: storage, binding: binding)
    _ = await journal.recover(using: host)
    let stored = try #require(await storage.snapshot?.records.first)
    let seq = try #require(stored.request.seq)
    #expect(stored.localState == .notFound)
    #expect(stored.request.sequenced(nil) == request("legacy"), "same id and body")
    #expect(await storage.snapshot?.nextSequence == seq + 1)
    #expect(await storage.snapshot?.version == 3, "persisted as the v3 local schema")
    #expect(await host.posted.isEmpty)
    _ = try await journal.retryNotFound(id: "legacy", using: host)
    #expect(await host.posted.map(\.seq) == [seq])
    let atPost = try #require(await host.persistedAtPost.first ?? nil)
    #expect(atPost.request.seq == seq && atPost.localState == .prepared, "persisted before the POST")
    // Reopened (crash after the POST): recovered by query, never re-sequenced.
    let reopened = try await CommandJournal.open(storage: storage, binding: binding)
    await host.setQueryError(nil)
    _ = await reopened.recover(using: host)
    #expect(await storage.snapshot?.records.first?.request.seq == seq)
    #expect(await storage.snapshot?.nextSequence == seq + 1)
    #expect(await host.posted.count == 1)
}

@Test func legacyRecordIsNeverSequencedWithoutAHost404() async throws {
    for failure in [ConnectorError.transport("offline"), .upgradeRequired, .commandExpired] {
        let storage = ControlledStorage(snapshot: legacyJournal(["legacy", "stale"]))
        let host = RecordingHost(storage: storage)
        await host.setQueryError(failure)
        let journal = try await CommandJournal.open(storage: storage, binding: binding)
        _ = await journal.recover(using: host)
        #expect(await storage.snapshot?.records.allSatisfy { $0.request.seq == nil } ?? true, "\(failure)")
        await #expect(throws: ConnectorError.conflict("operation-not-confirmed-missing")) {
            try await journal.retryNotFound(id: "legacy", using: host)
        }
        #expect(await host.posted.isEmpty)
    }
    // A 404 recorded by an older build is re-queried, never POSTed unsequenced.
    let storage = ControlledStorage(snapshot: legacyJournal(["stale"], state: .notFound))
    let host = RecordingHost(storage: storage)
    let journal = try await CommandJournal.open(storage: storage, binding: binding)
    await #expect(throws: ConnectorError.conflict("operation-not-confirmed-missing")) {
        try await journal.retryNotFound(id: "stale", using: host)
    }
    _ = await journal.recover(using: host)
    #expect(await host.queried == ["stale"])
    #expect(await storage.snapshot?.records.first?.request.seq != nil)
}

@Test func a426RetryKeepsTheOneSequence() async throws {
    let storage = ControlledStorage(snapshot: legacyJournal(["legacy"]))
    let host = RecordingHost(storage: storage)
    let journal = try await CommandJournal.open(storage: storage, binding: binding)
    _ = await journal.recover(using: host)
    let seq = try #require(await storage.snapshot?.records.first?.request.seq)
    await host.setPostError(.upgradeRequired)
    for _ in 0..<2 {
        await #expect(throws: ConnectorError.upgradeRequired) { try await journal.retryNotFound(id: "legacy", using: host) }
        #expect(await storage.snapshot?.records.first?.localState == .unknown)
        _ = await journal.recover(using: host)
    }
    #expect(await host.posted.map(\.seq) == [seq, seq])
    #expect(await storage.snapshot?.nextSequence == seq + 1)
}

@Test func severalLegacyRecordsGetUniqueIncreasingSequences() async throws {
    let storage = ControlledStorage(snapshot: legacyJournal(["l1", "l2", "l3"]))
    let host = RecordingHost(storage: storage)
    _ = try await CommandJournal.open(storage: storage, binding: binding).recover(using: host)
    let seqs = try #require(await storage.snapshot?.records.map(\.request.seq)).compactMap { $0 }
    #expect(seqs.count == 3 && seqs == seqs.sorted() && Set(seqs).count == 3)
    _ = try await CommandJournal.open(storage: storage, binding: binding).recover(using: host)
    #expect(try #require(await storage.snapshot?.records.map(\.request.seq)).compactMap { $0 } == seqs)
    #expect(await host.queried.count == 3, "a sequenced notFound is not re-queried")
}

@Test func a410IsATerminalExpiredStateAcrossReload() async throws {
    let storage = ControlledStorage()
    let host = RecordingHost(storage: storage)
    await host.setPostError(.transport("offline"))
    let journal = try await CommandJournal.open(storage: storage, binding: binding)
    do { _ = try await journal.submit(request(), draftCardID: "card", using: host) } catch { }
    let seq = await storage.snapshot?.records.first?.request.seq
    await host.setQueryError(.commandExpired)
    _ = await journal.recover(using: host)
    _ = await journal.recover(using: host)
    let record = try #require(await journal.allRecords().first)
    #expect(record.localState == .expired)
    #expect(record.localState != .resolved && record.localState != .notFound && record.result == nil)
    #expect(record.request.seq == seq)
    #expect(await storage.snapshot?.version == 3, "`expired` is only ever written as v3")
    let reopened = try await CommandJournal.open(storage: storage, binding: binding)
    #expect(await reopened.allRecords().first?.localState == .expired)
    _ = await reopened.recover(using: host)
    #expect(await host.queried == ["op-1"], "one GET, none after expiry or reload")
    #expect(await host.posted.count == 1)
    await #expect(throws: ConnectorError.conflict("operation-not-confirmed-missing")) {
        try await reopened.retryNotFound(id: "op-1", using: host)
    }
}

@Test func expiredRecordsLeaveTheReserveAndArePrunedButAmbiguousStays() async throws {
    let now = Date()
    var records = [CommandRecord(request: request("ambiguous"), localState: .expired, result: CommandResult(id: "ambiguous", state: .ambiguous, code: "delivery-unknown", result: nil), createdAt: now, updatedAt: now)]
    records += (1..<ConnectorLimits.commandJournalEntries).map {
        CommandRecord(request: request("expired-\($0)"), localState: .expired, result: nil, createdAt: now, updatedAt: now)
    }
    let storage = ControlledStorage(snapshot: JournalSnapshot(binding: binding, records: records))
    let journal = try await CommandJournal.open(storage: storage, binding: binding)
    _ = try await journal.submit(request("new"), draftCardID: nil, using: RecordingHost(storage: storage))
    let saved = await journal.allRecords()
    #expect(saved.count == ConnectorLimits.commandJournalEntries)
    #expect(saved.contains { $0.id == "new" } && saved.contains { $0.id == "ambiguous" })
    #expect(!saved.contains { $0.id == "expired-1" })
}

// ---- Local journal schema v3 (the file an older build wrote, on disk) ----

private func scratchJournalDirectory() throws -> URL {
    let url = FileManager.default.temporaryDirectory.appendingPathComponent("deck-journal-\(UUID().uuidString)", isDirectory: true)
    try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    return url
}

/// A record exactly as `JSONEncoder.deck` wrote it (sorted keys, ISO-8601).
private func recordJSON(_ id: String, _ state: String, seq: UInt64?, result: String? = nil) -> String {
    let seqField = seq.map { #","seq":\#($0)"# } ?? ""
    let resultField = result.map { #""result":\#($0),"# } ?? ""
    return #"{"createdAt":"2026-09-01T00:00:00Z","localState":"\#(state)","request":{"cardId":"card","expectedGeneration":"generation-1","id":"\#(id)","kind":"send-message","payload":{"text":"hello"}\#(seqField)},\#(resultField)"updatedAt":"2026-09-01T00:00:01Z"}"#
}

private func journalJSON(version: Int, records: [String], nextSequence: UInt64?) -> String {
    let next = nextSequence.map { #","nextSequence":\#($0)"# } ?? ""
    return #"{"binding":{"deviceID":"device","hostID":"host"},"drafts":{"card":{"cardID":"card","expectedGeneration":"generation-1","text":"draft","updatedAt":"2026-09-01T00:00:00Z"}}\#(next),"records":[\#(records.joined(separator: ","))],"version":\#(version)}"#
}

private func writeJournal(_ json: String, in directory: URL) throws -> Data {
    let data = Data(json.utf8)
    try data.write(to: directory.appendingPathComponent("journal.json"))
    return data
}

private func fileBytes(_ directory: URL) throws -> Data { try Data(contentsOf: directory.appendingPathComponent("journal.json")) }

private func fileVersion(_ directory: URL) throws -> Int {
    let object = try JSONSerialization.jsonObject(with: fileBytes(directory)) as? [String: Any]
    guard let version = object?["version"] as? Int else { throw ConnectorError.invalidResponse }
    return version
}

private let ambiguousResult = #"{"code":"delivery-unknown","id":"ambiguous","state":"ambiguous"}"#
private let appliedResult = #"{"id":"resolved","state":"applied"}"#

@Test func versionOneFileMigratesLazilyToVersionThree() async throws {
    let directory = try scratchJournalDirectory()
    defer { try? FileManager.default.removeItem(at: directory) }
    let original = try writeJournal(journalJSON(version: 1, records: [
        recordJSON("legacy", "unknown", seq: nil),
        recordJSON("resolved", "resolved", seq: nil, result: appliedResult),
    ], nextSequence: nil), in: directory)
    let storage = FileJournalStorage(directory: directory)
    let journal = try await CommandJournal.open(storage: storage, binding: binding)
    // Lazy: opening reads and upgrades in memory, and writes nothing.
    #expect(try fileBytes(directory) == original)
    let records = await journal.allRecords()
    #expect(records.map(\.id) == ["legacy", "resolved"])
    #expect(records.map(\.localState) == [.unknown, .resolved])
    #expect(records.allSatisfy { $0.request.seq == nil }, "a v1 body stays unsequenced")
    #expect(records[0].request == request("legacy"))
    #expect(records[1].result?.state == .applied)
    #expect(await journal.draft(for: "card")?.text == "draft")
    // The next save writes version 3 with the records unchanged.
    try await journal.saveDraft(MessageDraft(cardID: "other", text: "x", expectedGeneration: nil))
    #expect(try fileVersion(directory) == 3)
    let saved = try #require(try await storage.load())
    #expect(saved.version == 3 && saved.records == records)
    #expect(saved.nextSequence == 1)
}

@Test func versionTwoFileMigratesWithoutChangingAnyRecord() async throws {
    let directory = try scratchJournalDirectory()
    defer { try? FileManager.default.removeItem(at: directory) }
    let original = try writeJournal(journalJSON(version: 2, records: [
        recordJSON("prepared", "prepared", seq: 4001),
        recordJSON("awaiting", "awaitingFinal", seq: 4002),
        recordJSON("unknown", "unknown", seq: 4003),
        recordJSON("ambiguous", "ambiguous", seq: 4004, result: ambiguousResult),
        recordJSON("notFound", "notFound", seq: 4005),
        recordJSON("resolved", "resolved", seq: 4006, result: appliedResult),
        recordJSON("legacy", "unknown", seq: nil),
    ], nextSequence: 5000), in: directory)
    let storage = FileJournalStorage(directory: directory)
    let journal = try await CommandJournal.open(storage: storage, binding: binding)
    #expect(try fileBytes(directory) == original, "opening never rewrites")
    let records = await journal.allRecords()
    #expect(records.map(\.id) == ["prepared", "awaiting", "unknown", "ambiguous", "notFound", "resolved", "legacy"])
    #expect(records.map(\.localState) == [.prepared, .awaitingFinal, .unknown, .ambiguous, .notFound, .resolved, .unknown])
    #expect(records.map(\.request.seq) == [4001, 4002, 4003, 4004, 4005, 4006, nil])
    #expect(!records.contains { $0.localState == .expired }, "nothing becomes expired by migration")
    #expect(records[3].result?.state == .ambiguous && records[3].result?.code == "delivery-unknown")
    #expect(records.allSatisfy { $0.request.sequenced(nil) == request($0.id) }, "ids and bodies unchanged")
    try await journal.saveDraft(MessageDraft(cardID: "other", text: "x", expectedGeneration: nil))
    #expect(try fileVersion(directory) == 3)
    let saved = try #require(try await storage.load())
    #expect(saved.records == records && saved.nextSequence == 5000)
    #expect(saved.drafts["card"]?.text == "draft")

    // Recovery after migration: a host 404 sequences only the unsequenced
    // legacy record; ambiguous stays ambiguous with its own sequence.
    let host = RecordingHost(storage: ControlledStorage())
    _ = await journal.recover(using: host)
    let recovered = try #require(try await storage.load())
    let byID = Dictionary(uniqueKeysWithValues: recovered.records.map { ($0.id, $0) })
    #expect(byID["ambiguous"]?.localState == .ambiguous && byID["ambiguous"]?.request.seq == 4004)
    #expect(byID["ambiguous"]?.result?.state == .ambiguous)
    #expect(byID["resolved"]?.localState == .resolved && byID["resolved"]?.request.seq == 4006)
    for id in ["prepared", "awaiting", "unknown"] { #expect(byID[id]?.localState == .notFound, "\(id)") }
    #expect(["prepared", "awaiting", "unknown", "notFound"].map { byID[$0]?.request.seq } == [4001, 4002, 4003, 4005])
    let legacySeq = try #require(byID["legacy"]?.request.seq)
    #expect(legacySeq >= 5000 && recovered.nextSequence == legacySeq + 1)
    #expect(!recovered.records.contains { $0.localState == .expired })
    #expect(recovered.version == 3)
    #expect(await host.posted.isEmpty)
}

@Test func versionThreeExpiredReloadsWithoutAnyRequest() async throws {
    let directory = try scratchJournalDirectory()
    defer { try? FileManager.default.removeItem(at: directory) }
    let original = try writeJournal(journalJSON(version: 3, records: [recordJSON("gone", "expired", seq: 4100)], nextSequence: 4101), in: directory)
    let storage = FileJournalStorage(directory: directory)
    let journal = try await CommandJournal.open(storage: storage, binding: binding)
    let host = RecordingHost(storage: ControlledStorage())
    _ = await journal.recover(using: host)
    await #expect(throws: ConnectorError.conflict("operation-not-confirmed-missing")) {
        try await journal.retryNotFound(id: "gone", using: host)
    }
    #expect(await host.queried.isEmpty, "no GET")
    #expect(await host.posted.isEmpty, "no POST")
    let record = try #require(await journal.allRecords().first)
    #expect(record.localState == .expired && record.request.seq == 4100)
    #expect(try fileBytes(directory) == original, "nothing to persist, no new seq")
}

@Test func aNewerJournalVersionIsRefusedBeforeItsRecordsAndLeftUntouched() async throws {
    for version in [0, 4, 99] {
        for state in ["unknown", "superseded"] {
            let directory = try scratchJournalDirectory()
            defer { try? FileManager.default.removeItem(at: directory) }
            let original = try writeJournal(journalJSON(version: version, records: [recordJSON("op", state, seq: 7)], nextSequence: 8), in: directory)
            await #expect(throws: ConnectorError.conflict("journal-version-unsupported"), "v\(version) \(state)") {
                _ = try await CommandJournal.open(storage: FileJournalStorage(directory: directory), binding: binding)
            }
            #expect(try fileBytes(directory) == original)
        }
    }
}

@Test func anUnknownOrOutOfVersionStateFailsClosed() async throws {
    // v3 with a state it does not define; v1/v2 never wrote `expired`.
    for (version, state) in [(3, "vanished"), (2, "expired"), (1, "expired")] {
        let directory = try scratchJournalDirectory()
        defer { try? FileManager.default.removeItem(at: directory) }
        let original = try writeJournal(journalJSON(version: version, records: [recordJSON("keep", "unknown", seq: 6), recordJSON("odd", state, seq: 7)], nextSequence: 8), in: directory)
        var refused = false
        do { _ = try await CommandJournal.open(storage: FileJournalStorage(directory: directory), binding: binding) } catch { refused = true }
        #expect(refused, "v\(version) \(state) was opened")
        #expect(try fileBytes(directory) == original, "no record was dropped or rewritten")
    }
}

@Test func aFailedMigrationWriteLeavesTheVersionTwoFileIntact() async throws {
    let directory = try scratchJournalDirectory()
    defer {
        try? FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: directory.path)
        try? FileManager.default.removeItem(at: directory)
    }
    let original = try writeJournal(journalJSON(version: 2, records: [recordJSON("pending", "unknown", seq: 4003)], nextSequence: 5000), in: directory)
    let storage = FileJournalStorage(directory: directory)
    let journal = try await CommandJournal.open(storage: storage, binding: binding)
    // The atomic replacement cannot land in a read-only directory.
    try FileManager.default.setAttributes([.posixPermissions: 0o500], ofItemAtPath: directory.path)
    await #expect(throws: (any Error).self) {
        try await journal.saveDraft(MessageDraft(cardID: "other", text: "x", expectedGeneration: nil))
    }
    #expect(try fileBytes(directory) == original, "still the whole v2 file")
    #expect(await journal.draft(for: "other") == nil, "memory did not commit")
    let entries = try FileManager.default.contentsOfDirectory(atPath: directory.path)
    #expect(entries == ["journal.json"], "no partial file left behind: \(entries)")
    try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: directory.path)
    try await journal.saveDraft(MessageDraft(cardID: "other", text: "x", expectedGeneration: nil))
    #expect(try fileVersion(directory) == 3)
    #expect(try await storage.load()?.records.first?.request.seq == 4003)
}
