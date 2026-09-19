import Foundation

public struct VersionedMessageDraft: Equatable, Sendable {
    public let draft: MessageDraft
    public let version: UInt64
    public init(draft: MessageDraft, version: UInt64) { self.draft = draft; self.version = version }
}

public enum DraftSaveReceipt: Equatable, Sendable { case saved(UInt64), superseded(UInt64) }

public protocol MessageDraftSink: Sendable {
    func saveDraft(_ draft: MessageDraft) async throws
}

extension CommandJournal: MessageDraftSink {}

public actor DraftSaveCoordinator {
    private let sink: any MessageDraftSink
    private var savedVersions: [String: UInt64] = [:]
    private var busy = false
    private var waiters: [CheckedContinuation<Void, Never>] = []

    public init(sink: any MessageDraftSink) { self.sink = sink }

    public func persist(_ value: VersionedMessageDraft) async throws -> DraftSaveReceipt {
        await acquire()
        defer { release() }
        let saved = savedVersions[value.draft.cardID] ?? 0
        guard value.version > saved else { return .superseded(saved) }
        try await sink.saveDraft(value.draft)
        savedVersions[value.draft.cardID] = value.version
        return .saved(value.version)
    }

    /// Used immediately before POST. It writes the exact visible snapshot even if a newer edit
    /// arrived while the caller was waiting; the newer edit remains queued and is never cleared.
    public func persistForSend(_ value: VersionedMessageDraft) async throws -> DraftSaveReceipt {
        await acquire()
        defer { release() }
        try await sink.saveDraft(value.draft)
        savedVersions[value.draft.cardID] = max(savedVersions[value.draft.cardID] ?? 0, value.version)
        return .saved(value.version)
    }

    private func acquire() async {
        if !busy { busy = true; return }
        await withCheckedContinuation { waiters.append($0) }
    }

    private func release() {
        if waiters.isEmpty { busy = false } else { waiters.removeFirst().resume() }
    }
}

public struct ComposerDraftState: Equatable, Sendable {
    public private(set) var cardID: String
    public private(set) var text: String
    public private(set) var expectedGeneration: String?
    public private(set) var version: UInt64
    public private(set) var persistedVersion: UInt64
    public private(set) var persistenceError: String?
    private var hasLocalEdits: Bool

    public init(cardID: String, remote: MessageDraft?, fallbackGeneration: String?) {
        self.cardID = cardID
        self.text = remote?.text ?? ""
        self.expectedGeneration = remote?.expectedGeneration ?? fallbackGeneration
        self.version = 0
        self.persistedVersion = 0
        self.persistenceError = nil
        self.hasLocalEdits = false
    }

    public var isDirty: Bool { version > persistedVersion }
    public var snapshot: VersionedMessageDraft {
        VersionedMessageDraft(draft: MessageDraft(cardID: cardID, text: text, expectedGeneration: expectedGeneration), version: version)
    }

    public mutating func edit(_ value: String) { version &+= 1; text = value; persistenceError = nil; hasLocalEdits = true }
    public mutating func acceptGeneration(_ generation: String?) { version &+= 1; expectedGeneration = generation; persistenceError = nil; hasLocalEdits = true }

    public mutating func merge(remote: MessageDraft?, fallbackGeneration: String?) {
        guard !hasLocalEdits else { return }
        text = remote?.text ?? ""
        expectedGeneration = remote?.expectedGeneration ?? fallbackGeneration
    }

    public mutating func saved(version savedVersion: UInt64) {
        persistedVersion = max(persistedVersion, savedVersion)
        if savedVersion >= version { persistenceError = nil }
    }

    public mutating func failed(version failedVersion: UInt64, message: String) {
        if failedVersion == version { persistenceError = message }
    }

    public mutating func clearIfUnchanged(sentVersion: UInt64, fallbackGeneration: String?) {
        guard version == sentVersion else { return }
        version &+= 1
        persistedVersion = version
        text = ""
        expectedGeneration = fallbackGeneration
        persistenceError = nil
        hasLocalEdits = true
    }
}
