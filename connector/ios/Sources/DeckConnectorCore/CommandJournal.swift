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

public struct JournalSnapshot: Codable, Equatable, Sendable {
    /// The phone's local file only, not the Connector wire or host journal
    /// format. 2 adds `nextSequence`; 3 adds the `expired` record state. A
    /// version-1 or -2 journal is upgraded in memory on open, with every record
    /// unchanged, and written as version 3 by its next save (atomically, so a
    /// failed save leaves the old file whole). Any other version is refused
    /// before its records are read, and the file is left untouched.
    public static let currentVersion = 3
    public let version: Int
    public let binding: JournalBinding
    public var records: [CommandRecord]
    public var drafts: [String: MessageDraft]
    /// Lower bound for the next command's admission sequence.
    public var nextSequence: UInt64

    public init(binding: JournalBinding, records: [CommandRecord] = [], drafts: [String: MessageDraft] = [:], nextSequence: UInt64 = 1) {
        self.version = Self.currentVersion
        self.binding = binding
        self.records = records
        self.drafts = drafts
        self.nextSequence = nextSequence
    }

    private enum CodingKeys: String, CodingKey { case version, binding, records, drafts, nextSequence }

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
            let upgraded = JournalSnapshot(binding: loaded.binding, records: loaded.records, drafts: loaded.drafts, nextSequence: loaded.nextSequence)
            return CommandJournal(storage: storage, state: upgraded)
        }
        return CommandJournal(storage: storage, state: JournalSnapshot(binding: binding))
    }

    public func draft(for cardID: String) async -> MessageDraft? {
        await acquireTransaction()
        defer { releaseTransaction() }
        return state.drafts[cardID]
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

    public func submit(_ request: CommandRequest, draftCardID: String?, using transport: any CommandTransport) async throws -> CommandResult {
        // Text the host would refuse never enters the journal or takes a
        // sequence: the user fixes it and sends again.
        try WireValidator.validate(request)
        let prepared = try await prepare(request)
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

    public func recover(using transport: any CommandTransport) async -> [String: Result<CommandResult, Error>] {
        let pending = await recoverableRecords()
        var outcomes: [String: Result<CommandResult, Error>] = [:]
        for record in pending {
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
        return outcomes
    }

    /// Records a new command with its admission sequence before any POST, or
    /// answers an existing one. The sequence is `max(nextSequence, now in ms)`:
    /// strictly increasing within this journal, and still above an earlier
    /// installation's sequences if the journal file was lost while the
    /// device credential survived.
    private func prepare(_ request: CommandRequest) async throws -> (existing: CommandResult?, request: CommandRequest) {
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
        candidate.records.append(CommandRecord(request: sequenced, localState: .prepared, result: nil, createdAt: now, updatedAt: now))
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

        if let result, (result.state == .applied || result.state == .delivered),
           let draftCardID,
           case let .string(sentText)? = candidate.records[index].request.payload["text"],
           let currentDraft = candidate.drafts[draftCardID],
           currentDraft.text == sentText,
           currentDraft.expectedGeneration == candidate.records[index].request.expectedGeneration {
            candidate.drafts.removeValue(forKey: draftCardID)
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
