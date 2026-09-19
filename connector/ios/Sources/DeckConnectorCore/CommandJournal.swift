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
    public enum LocalState: String, Codable, Sendable { case prepared, unknown, notFound, awaitingFinal, resolved, ambiguous }
    public let request: CommandRequest
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
    public let version: Int
    public let binding: JournalBinding
    public var records: [CommandRecord]
    public var drafts: [String: MessageDraft]

    public init(binding: JournalBinding, records: [CommandRecord] = [], drafts: [String: MessageDraft] = [:]) {
        self.version = 1
        self.binding = binding
        self.records = records
        self.drafts = drafts
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
        let attributes = try FileManager.default.attributesOfItem(atPath: file.path)
        guard let size = attributes[.size] as? NSNumber, size.intValue <= ConnectorLimits.journalBytes else { throw ConnectorError.responseTooLarge }
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
            guard loaded.version == 1, loaded.binding == binding else { throw ConnectorError.conflict("journal-binding-mismatch") }
            guard loaded.records.count <= ConnectorLimits.commandJournalEntries else { throw ConnectorError.capacityExceeded }
            return CommandJournal(storage: storage, state: loaded)
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
        if let existing = try await prepare(request) {
            return existing
        }

        let result: CommandResult
        do {
            result = try await transport.post(command: request)
        } catch {
            try? await persistState(id: request.id, localState: .unknown, result: nil, draftCardID: nil)
            throw error
        }
        guard result.id == request.id else {
            try? await persistState(id: request.id, localState: .unknown, result: nil, draftCardID: nil)
            throw ConnectorError.invalidResponse
        }
        try await persistState(id: request.id, localState: localState(for: result), result: result, draftCardID: draftCardID)
        return await record(id: request.id)?.result ?? result
    }

    /// Explicitly retries an operation only after the host has proved that its original ID is absent.
    public func retryNotFound(id: String, using transport: any CommandTransport) async throws -> CommandResult {
        guard let record = await record(id: id), record.localState == .notFound else {
            throw ConnectorError.conflict("operation-not-confirmed-missing")
        }
        let result = try await transport.post(command: record.request)
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
                if error as? ConnectorError == .commandNotFound {
                    try? await persistState(id: record.id, localState: .notFound, result: nil, draftCardID: nil)
                }
                outcomes[record.id] = .failure(error)
            }
        }
        return outcomes
    }

    private func prepare(_ request: CommandRequest) async throws -> CommandResult? {
        await acquireTransaction()
        defer { releaseTransaction() }
        if let existing = state.records.first(where: { $0.id == request.id }) {
            guard existing.request == request else { throw ConnectorError.conflict("operation-id-reused") }
            if let result = existing.result { protectedResultIDs.insert(request.id); return result }
            throw ConnectorError.transport("The original operation is pending recovery.")
        }
        let now = Date()
        var candidate = state
        candidate.records.append(CommandRecord(request: request, localState: .prepared, result: nil, createdAt: now, updatedAt: now))
        protectedResultIDs.insert(request.id)
        do { try await persistAndCommit(candidate, protecting: request.id) }
        catch { protectedResultIDs.remove(request.id); throw error }
        return nil
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
        return state.records.filter { [.prepared, .unknown, .awaitingFinal, .ambiguous].contains($0.localState) }
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
                $0.localState == .resolved && $0.id != id && !protectedResultIDs.contains($0.id)
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
            case .resolved, .ambiguous: total
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
