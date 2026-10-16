import CryptoKit
import Foundation

public struct JournalBinding: Codable, Equatable, Sendable {
    public let hostID: String
    public let deviceID: String
    public init(hostID: String, deviceID: String) { self.hostID = hostID; self.deviceID = deviceID }

    public var storageKey: String {
        SHA256.hash(data: Data("\(hostID)\u{0}\(deviceID)".utf8)).map { String(format: "%02x", $0) }.joined()
    }
}

public struct CommandRecord: Codable, Equatable, Sendable, Identifiable {
    /// `expired`: the host answered 410 — the outcome left its retained
    /// history. Terminal: never queried, re-sent or re-sequenced, and it says
    /// nothing about whether the command ran. Only a version-3 journal may
    /// hold it (see `JournalSnapshot.currentVersion`).
    public enum LocalState: String, Codable, Sendable { case prepared, unknown, notFound, awaitingFinal, resolved, ambiguous, expired }
    /// Immutable except for one step: a version-1 record without `seq` gets
    /// one when the host proves its id absent (see `CommandJournal.recover`).
    public internal(set) var request: CommandRequest
    public var localState: LocalState
    public var result: CommandResult?
    public let createdAt: Date
    public var updatedAt: Date
    /// Local-only identity of the exact note edit present when a buffer command
    /// was first prepared. Older journal records decode this as nil and never
    /// gain authority to clear a note draft.
    public internal(set) var noteDraftRevision: UUID? = nil
    public var id: String { request.id }
}

public struct MessageDraft: Codable, Equatable, Sendable {
    public let cardID: String
    public var text: String
    public let expectedGeneration: String?
    public var updatedAt: Date

    public init(cardID: String, text: String, expectedGeneration: String?, updatedAt: Date = Date()) {
        self.cardID = cardID
        self.text = text
        self.expectedGeneration = expectedGeneration
        self.updatedAt = updatedAt
    }
}

/// An unsaved scratchpad edit. The journal binding supplies host/device scope;
/// `entryID == nil` identifies the one new-note composer for a card, while an
/// entry ID identifies that existing note's editor. Note drafts deliberately
/// do not share storage or clearing behavior with message drafts.
public struct NoteDraft: Codable, Equatable, Sendable {
    public let cardID: String
    public let entryID: String?
    public let text: String
    public let updatedAt: Date
    /// Stable for one edit snapshot and persisted without relying on Date's
    /// lossy ISO-8601 representation.
    public let revision: UUID

    public init(cardID: String, entryID: String?, text: String, updatedAt: Date = Date(), revision: UUID = UUID()) {
        self.cardID = cardID
        self.entryID = entryID
        self.text = text
        self.updatedAt = updatedAt
        self.revision = revision
    }

    public static func storageKey(cardID: String, entryID: String?) -> String {
        let entry = entryID ?? ""
        return "\(cardID.utf8.count):\(cardID):\(entryID == nil ? -1 : entry.utf8.count):\(entry)"
    }

    public var storageKey: String {
        Self.storageKey(cardID: cardID, entryID: entryID)
    }
}

public struct JournalSnapshot: Codable, Equatable, Sendable {
    /// The phone's local file only, not the Connector wire or host journal
    /// format. 2 adds `nextSequence`; 3 adds the `expired` record state; 4 adds
    /// note drafts in a namespace separate from message drafts. A version-1,
    /// -2 or -3 journal is upgraded in memory on open, with every record and
    /// message draft unchanged, and written as version 4 by its next save (atomically, so a
    /// failed save leaves the old file whole). Any other version is refused
    /// before its records are read, and the file is left untouched.
    public static let currentVersion = 4
    public let version: Int
    public let binding: JournalBinding
    public var records: [CommandRecord]
    public var drafts: [String: MessageDraft]
    public var noteDrafts: [String: NoteDraft]
    /// Lower bound for the next command's admission sequence.
    public var nextSequence: UInt64

    public init(binding: JournalBinding, records: [CommandRecord] = [], drafts: [String: MessageDraft] = [:], noteDrafts: [String: NoteDraft] = [:], nextSequence: UInt64 = 1) {
        self.version = Self.currentVersion
        self.binding = binding
        self.records = records
        self.drafts = drafts
        self.noteDrafts = noteDrafts
        self.nextSequence = nextSequence
    }

    private enum CodingKeys: String, CodingKey { case version, binding, records, drafts, noteDrafts, nextSequence }

    public init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        version = try values.decode(Int.self, forKey: .version)
        guard (1...Self.currentVersion).contains(version) else { throw ConnectorError.conflict("journal-version-unsupported") }
        binding = try values.decode(JournalBinding.self, forKey: .binding)
        records = try values.decode([CommandRecord].self, forKey: .records)
        if version < 3, records.contains(where: { $0.localState == .expired }) {
            throw DecodingError.dataCorruptedError(forKey: .records, in: values, debugDescription: "expired before journal version 3")
        }
        drafts = try values.decode([String: MessageDraft].self, forKey: .drafts)
        if version >= 4 {
            noteDrafts = try values.decode([String: NoteDraft].self, forKey: .noteDrafts)
            guard noteDrafts.allSatisfy({ $0.key == $0.value.storageKey }) else {
                throw DecodingError.dataCorruptedError(forKey: .noteDrafts, in: values, debugDescription: "note draft key does not match its identity")
            }
        } else {
            noteDrafts = [:]
        }
        nextSequence = try values.decodeIfPresent(UInt64.self, forKey: .nextSequence) ?? 1
    }
}

public protocol JournalStorage: Sendable {
    func load() async throws -> JournalSnapshot?
    func save(_ snapshot: JournalSnapshot) async throws
}

public struct FileJournalStorage: JournalStorage, Sendable {
    public let directory: URL
    public init(directory: URL) { self.directory = directory }
    private var file: URL { directory.appendingPathComponent("journal.json", isDirectory: false) }

    public func load() async throws -> JournalSnapshot? {
        guard FileManager.default.fileExists(atPath: file.path) else { return nil }
        let handle = try FileHandle(forReadingFrom: file)
        defer { try? handle.close() }
        let data = try handle.read(upToCount: ConnectorLimits.journalBytes + 1) ?? Data()
        guard data.count <= ConnectorLimits.journalBytes else { throw ConnectorError.responseTooLarge }
        return try JSONDecoder.deck.decode(JournalSnapshot.self, from: data)
    }

    public func save(_ snapshot: JournalSnapshot) async throws {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        let data = try JSONEncoder.deck.encode(snapshot)
        guard data.count <= ConnectorLimits.journalBytes else { throw ConnectorError.capacityExceeded }
        #if os(iOS)
        try data.write(to: file, options: [.atomic, .completeFileProtection])
        #else
        try data.write(to: file, options: .atomic)
        #endif
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: file.path)
    }
}

public actor CommandJournal {
    private let storage: any JournalStorage
    private var state: JournalSnapshot
    private var protectedResultIDs = Set<String>()
    /// Process-local scheduling only. A failed GET creates no durable command
    /// fact, but it must not starve later pending IDs in bounded recovery.
    private var lastRecoveryQueries: [String: Date] = [:]
    private var transactionBusy = false
    private var transactionWaiters: [CheckedContinuation<Void, Never>] = []

    private init(storage: any JournalStorage, state: JournalSnapshot) {
        self.storage = storage
        self.state = state
    }

    public static func open(storage: any JournalStorage, binding: JournalBinding) async throws -> CommandJournal {
        let loaded = try await storage.load()
        if let loaded {
            guard (1...JournalSnapshot.currentVersion).contains(loaded.version) else { throw ConnectorError.conflict("journal-version-unsupported") }
            guard loaded.binding == binding else { throw ConnectorError.conflict("journal-binding-mismatch") }
            guard loaded.records.count <= ConnectorLimits.commandJournalEntries else { throw ConnectorError.capacityExceeded }
            // Version 1 records carry no sequence and keep their immutable
            // bodies; the upgrade only adds the counter and the version.
            let upgraded = JournalSnapshot(binding: loaded.binding, records: loaded.records, drafts: loaded.drafts, noteDrafts: loaded.noteDrafts, nextSequence: loaded.nextSequence)
            return CommandJournal(storage: storage, state: upgraded)
        }
        return CommandJournal(storage: storage, state: JournalSnapshot(binding: binding))
    }

    public func draft(for cardID: String) async -> MessageDraft? {
        await acquireTransaction()
        defer { releaseTransaction() }
        return state.drafts[cardID]
    }

    public func noteDraft(cardID: String, entryID: String?) async -> NoteDraft? {
        await acquireTransaction()
        defer { releaseTransaction() }
        return state.noteDrafts[NoteDraft.storageKey(cardID: cardID, entryID: entryID)]
    }

    public func allRecords() async -> [CommandRecord] {
        await acquireTransaction()
        defer { releaseTransaction() }
        return state.records
    }

    public func canonicalResult(id: String, consume: Bool = false) async -> CommandResult? {
        await acquireTransaction()
        defer { releaseTransaction() }
        let result = state.records.first(where: { $0.id == id })?.result
        if consume, result != nil { protectedResultIDs.remove(id) }
        return result
    }

    public func saveDraft(_ draft: MessageDraft) async throws {
        guard draft.text.utf8.count <= ConnectorLimits.draftUTF8Bytes else { throw ConnectorError.requestTooLarge }
        await acquireTransaction()
        defer { releaseTransaction() }
        var candidate = state
        candidate.drafts[draft.cardID] = draft
        try await persistAndCommit(candidate)
    }

    public func discardDraft(cardID: String) async throws {
        await acquireTransaction()
        defer { releaseTransaction() }
        var candidate = state
        candidate.drafts.removeValue(forKey: cardID)
        try await persistAndCommit(candidate)
    }

    public func saveNoteDraft(_ draft: NoteDraft) async throws {
        guard draft.text.utf8.count <= ConnectorLimits.draftUTF8Bytes else { throw ConnectorError.requestTooLarge }
        await acquireTransaction()
        defer { releaseTransaction() }
        var candidate = state
        candidate.noteDrafts[draft.storageKey] = draft
        try await persistAndCommit(candidate)
    }

    /// Removes a note draft only when it is still the exact snapshot whose
    /// save succeeded. A newer edit therefore survives a late host response.
    @discardableResult
    public func discardNoteDraft(ifUnchanged draft: NoteDraft) async throws -> Bool {
        await acquireTransaction()
        defer { releaseTransaction() }
        guard state.noteDrafts[draft.storageKey] == draft else { return false }
        var candidate = state
        candidate.noteDrafts.removeValue(forKey: draft.storageKey)
        try await persistAndCommit(candidate)
        return true
    }

    public func submit(
        _ request: CommandRequest,
        draftCardID: String?,
        noteDraftRevision: UUID? = nil,
        using transport: any CommandTransport
    ) async throws -> CommandResult {
        // Text the host would refuse never enters the journal or takes a
        // sequence: the user fixes it and sends again.
        try WireValidator.validate(request)
        let prepared = try await prepare(request, noteDraftRevision: noteDraftRevision)
        if let existing = prepared.existing {
            return existing
        }
        let request = prepared.request

        let result: CommandResult
        do {
            result = try await transport.post(command: request)
        } catch {
            try? await recordPostFailure(id: request.id, error: error)
            throw error
        }
        guard result.id == request.id else {
            try? await persistState(id: request.id, localState: .unknown, result: nil, draftCardID: nil)
            throw ConnectorError.invalidResponse
        }
        try await persistState(id: request.id, localState: localState(for: result), result: result, draftCardID: draftCardID)
        return await record(id: request.id)?.result ?? result
    }

    /// Explicitly retries an operation only after the host has proved that its
    /// original ID is absent, with its original ID, body and sequence. The
    /// record is `prepared` again before the POST, so a crash after it is
    /// recovered by query, never by another POST.
    public func retryNotFound(id: String, using transport: any CommandTransport) async throws -> CommandResult {
        guard let record = await record(id: id), record.localState == .notFound, record.request.seq != nil else {
            throw ConnectorError.conflict("operation-not-confirmed-missing")
        }
        try await persistState(id: id, localState: .prepared, result: nil, draftCardID: nil)
        let result: CommandResult
        do {
            result = try await transport.post(command: record.request)
        } catch {
            try? await recordPostFailure(id: id, error: error)
            throw error
        }
        guard result.id == id else { throw ConnectorError.invalidResponse }
        try await persistState(id: id, localState: localState(for: result), result: result, draftCardID: record.request.cardId)
        return await self.record(id: id)?.result ?? result
    }

    public func recover(using transport: any CommandTransport, maximumQueries: Int? = nil) async -> [String: Result<CommandResult, Error>] {
        let recoverable = await recoverableRecords()
        let recoverableIDs = Set(recoverable.map(\.id))
        lastRecoveryQueries = lastRecoveryQueries.filter { recoverableIDs.contains($0.key) }
        let ordered = recoverable.sorted {
            let leftQuery = lastRecoveryQueries[$0.id]
            let rightQuery = lastRecoveryQueries[$1.id]
            if (leftQuery == nil) != (rightQuery == nil) { return leftQuery == nil }
            let left = leftQuery ?? $0.updatedAt
            let right = rightQuery ?? $1.updatedAt
            if left != right { return left < right }
            if $0.createdAt != $1.createdAt { return $0.createdAt < $1.createdAt }
            return $0.id < $1.id
        }
        let pending = maximumQueries.map { Array(ordered.prefix(max(0, $0))) } ?? ordered
        var outcomes: [String: Result<CommandResult, Error>] = [:]
        for record in pending {
            if Task.isCancelled { break }
            lastRecoveryQueries[record.id] = Date()
            do {
                let result = try await transport.query(id: record.id)
                guard result.id == record.id else { throw ConnectorError.invalidResponse }
                try await persistState(id: record.id, localState: localState(for: result), result: result, draftCardID: record.request.cardId)
                outcomes[record.id] = .success(result)
            } catch {
                switch error as? ConnectorError {
                case .commandNotFound: try? await persistNotFound(id: record.id)
                case .commandExpired: try? await persistExpired(id: record.id)
                default: break
                }
                outcomes[record.id] = .failure(error)
            }
        }
        let remainingIDs = Set(await recoverableRecords().map(\.id))
        lastRecoveryQueries = lastRecoveryQueries.filter { remainingIDs.contains($0.key) }
        return outcomes
    }

    /// Records a new command with its admission sequence before any POST, or
    /// answers an existing one. The sequence is `max(nextSequence, now in ms)`:
    /// strictly increasing within this journal, and still above an earlier
    /// installation's sequences if the journal file was lost while the
    /// device credential survived.
    private func prepare(_ request: CommandRequest, noteDraftRevision: UUID?) async throws -> (existing: CommandResult?, request: CommandRequest) {
        await acquireTransaction()
        defer { releaseTransaction() }
        if let existing = state.records.first(where: { $0.id == request.id }) {
            guard existing.request.sequenced(nil) == request.sequenced(nil) else { throw ConnectorError.conflict("operation-id-reused") }
            if let result = existing.result { protectedResultIDs.insert(request.id); return (result, existing.request) }
            throw ConnectorError.transport("The original operation is pending recovery.")
        }
        let now = Date()
        var candidate = state
        let sequenced = request.sequenced(Self.allocateSequence(&candidate, now: now))
        candidate.records.append(CommandRecord(
            request: sequenced,
            localState: .prepared,
            result: nil,
            createdAt: now,
            updatedAt: now,
            noteDraftRevision: Self.matchingNoteDraftRevision(noteDraftRevision, for: sequenced, in: candidate)
        ))
        protectedResultIDs.insert(request.id)
        do { try await persistAndCommit(candidate, protecting: request.id) }
        catch { protectedResultIDs.remove(request.id); throw error }
        return (nil, sequenced)
    }

    private static func allocateSequence(_ snapshot: inout JournalSnapshot, now: Date) -> UInt64 {
        let clock = UInt64(max(0, (now.timeIntervalSince1970 * 1000).rounded(.down)))
        let seq = max(snapshot.nextSequence, clock)
        snapshot.nextSequence = seq + 1
        return seq
    }

    private static func matchingNoteDraftRevision(_ intendedRevision: UUID?, for request: CommandRequest, in snapshot: JournalSnapshot) -> UUID? {
        guard let intendedRevision,
              ["buffer-add", "buffer-edit"].contains(request.kind),
              let cardID = request.cardId,
              case let .string(text)? = request.payload["text"] else { return nil }
        let entryID: String?
        if request.kind == "buffer-edit" {
            guard case let .string(value)? = request.payload["entryId"] else { return nil }
            entryID = value
        } else {
            entryID = nil
        }
        let draft = snapshot.noteDrafts[NoteDraft.storageKey(cardID: cardID, entryID: entryID)]
        return draft?.revision == intendedRevision && draft?.text == text ? intendedRevision : nil
    }

    /// A 404 from the host proves the id was never admitted. A version-1
    /// record has no `seq` (the host refuses it with 426), so this — and only
    /// this — is where it gets one: in the same persisted write as `notFound`,
    /// before any retry can POST it. ID and body stay the same.
    private func persistNotFound(id: String) async throws {
        await acquireTransaction()
        defer { releaseTransaction() }
        var candidate = state
        guard let index = candidate.records.firstIndex(where: { $0.id == id }) else { throw ConnectorError.invalidResponse }
        let record = candidate.records[index]
        if [.resolved, .expired].contains(record.localState) || record.result?.state == .ambiguous { return }
        if record.request.seq == nil {
            candidate.records[index].request = record.request.sequenced(Self.allocateSequence(&candidate, now: Date()))
        }
        candidate.records[index].localState = .notFound
        candidate.records[index].result = nil
        candidate.records[index].updatedAt = Date()
        try await persistAndCommit(candidate, protecting: id)
    }

    /// A 410 from the host: terminal, keeping whatever result was known.
    private func persistExpired(id: String) async throws {
        await acquireTransaction()
        defer { releaseTransaction() }
        var candidate = state
        guard let index = candidate.records.firstIndex(where: { $0.id == id }) else { throw ConnectorError.invalidResponse }
        if [.resolved, .expired].contains(candidate.records[index].localState) { return }
        candidate.records[index].localState = .expired
        candidate.records[index].updatedAt = Date()
        try await persistAndCommit(candidate, protecting: id)
    }

    private func recordPostFailure(id: String, error: Error) async throws {
        if error as? ConnectorError == .commandExpired { try await persistExpired(id: id) }
        else { try await persistState(id: id, localState: .unknown, result: nil, draftCardID: nil) }
    }

    private func persistState(id: String, localState: CommandRecord.LocalState, result: CommandResult?, draftCardID: String?) async throws {
        await acquireTransaction()
        defer { releaseTransaction() }
        var candidate = state
        guard let index = candidate.records.firstIndex(where: { $0.id == id }) else { throw ConnectorError.invalidResponse }
        if candidate.records[index].localState == .resolved { return }
        if candidate.records[index].result?.state == .ambiguous,
           result == nil || result?.state == .accepted { return }
        if let result { try WireValidator.validate(result, for: candidate.records[index].request) }
        candidate.records[index].localState = localState
        candidate.records[index].result = result
        candidate.records[index].updatedAt = Date()

        let record = candidate.records[index]
        if let result, (result.state == .applied || result.state == .delivered),
           record.request.kind == "send-message",
           let draftCardID,
           case let .string(sentText)? = record.request.payload["text"],
           let currentDraft = candidate.drafts[draftCardID],
           currentDraft.text == sentText,
           currentDraft.expectedGeneration == record.request.expectedGeneration {
            candidate.drafts.removeValue(forKey: draftCardID)
        }
        if let result, (result.state == .applied || result.state == .delivered),
           ["buffer-add", "buffer-edit"].contains(record.request.kind),
           let cardID = record.request.cardId,
           case let .string(sentText)? = record.request.payload["text"] {
            let entryID: String?
            if record.request.kind == "buffer-edit" {
                guard case let .string(value)? = record.request.payload["entryId"] else {
                    throw ConnectorError.invalidResponse
                }
                entryID = value
            } else {
                entryID = nil
            }
            let key = NoteDraft.storageKey(cardID: cardID, entryID: entryID)
            if let currentDraft = candidate.noteDrafts[key],
               let submittedRevision = record.noteDraftRevision,
               currentDraft.revision == submittedRevision,
               currentDraft.text == sentText {
                candidate.noteDrafts.removeValue(forKey: key)
            }
        }
        try await persistAndCommit(candidate, protecting: id)
    }

    private func recoverableRecords() async -> [CommandRecord] {
        await acquireTransaction()
        defer { releaseTransaction() }
        // A `notFound` without `seq` is a version-1 record, or one whose 404
        // came from an older build: it is queried again for fresh proof.
        return state.records.filter {
            [.prepared, .unknown, .awaitingFinal, .ambiguous].contains($0.localState)
                || ($0.localState == .notFound && $0.request.seq == nil)
        }
    }

    private func record(id: String) async -> CommandRecord? {
        await acquireTransaction()
        defer { releaseTransaction() }
        return state.records.first(where: { $0.id == id })
    }

    private func localState(for result: CommandResult) -> CommandRecord.LocalState {
        switch result.state {
        case .accepted: .awaitingFinal
        case .ambiguous: .ambiguous
        case .applied, .delivered, .rejected: .resolved
        }
    }

    private func persistAndCommit(_ proposed: JournalSnapshot, protecting id: String? = nil) async throws {
        var candidate = proposed
        while !fits(candidate) {
            guard let index = candidate.records.firstIndex(where: {
                [.resolved, .expired].contains($0.localState) && $0.result?.state != .ambiguous
                    && $0.id != id && !protectedResultIDs.contains($0.id)
            }) else { throw ConnectorError.capacityExceeded }
            candidate.records.remove(at: index)
        }
        try await storage.save(candidate)
        state = candidate
    }

    private func fits(_ candidate: JournalSnapshot) -> Bool {
        guard candidate.records.count <= ConnectorLimits.commandJournalEntries,
              let bytes = (try? JSONEncoder.deck.encode(candidate))?.count else { return false }
        let reserved = candidate.records.reduce(0) { total, record in
            switch record.localState {
            case .prepared, .unknown, .notFound, .awaitingFinal: total + ConnectorLimits.terminalResultReserveBytes
            case .resolved, .ambiguous, .expired: total
            }
        }
        return bytes + reserved <= ConnectorLimits.journalBytes
    }

    private func acquireTransaction() async {
        if !transactionBusy { transactionBusy = true; return }
        await withCheckedContinuation { transactionWaiters.append($0) }
    }

    private func releaseTransaction() {
        if transactionWaiters.isEmpty { transactionBusy = false }
        else { transactionWaiters.removeFirst().resume() }
    }
}

private extension JSONEncoder {
    static var deck: JSONEncoder { let encoder = JSONEncoder(); encoder.dateEncodingStrategy = .iso8601; encoder.outputFormatting = [.sortedKeys]; return encoder }
}

private extension JSONDecoder {
    static var deck: JSONDecoder { let decoder = JSONDecoder(); decoder.dateDecodingStrategy = .iso8601; return decoder }
}
