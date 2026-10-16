// Foreground reads are coalesced independently of command recovery. Every read
// is fenced by pairing identity, current card generation and request sequence.
// Only network loss may retain a visibly stale snapshot; authority/identity
// changes clear it. Recovery queries existing immutable operations, never POSTs.
// Durable mutation receipts settle local UI before background remote refreshes;
// an unrelated slow read must not keep an acknowledged operation in "saving".
import DeckConnectorCore
import Foundation
import SwiftUI
import UIKit

@MainActor
final class AppModel: ObservableObject {
    enum ConnectionState: Equatable {
        case loading, unpaired, connecting, online, offline(String), revoked
    }
    enum MutationOutcome: Equatable { case applied, pending(id: String?, state: String), failed(String) }
    enum DraftPersistenceStatus: Equatable { case saving(UInt64), saved(UInt64), failed(UInt64, String) }
    enum OutputIssue: Equatable {
        case sessionUnavailable, agentNotInForeground, readFailed, targetChanged
        case unsupportedTarget, controlled, unknown

        init(_ error: Error) {
            switch error {
            case ConnectorError.outputResponse(503, "session-unavailable"):
                self = .sessionUnavailable
            case ConnectorError.outputResponse(409, "agent-not-in-foreground"):
                self = .agentNotInForeground
            case ConnectorError.outputResponse(503, "output-read-failed"):
                self = .readFailed
            case ConnectorError.outputResponse(409, "context-changed"):
                self = .targetChanged
            case ConnectorError.outputResponse(_, "mcp-controlled"):
                self = .controlled
            case ConnectorError.unsupportedTarget, ConnectorError.outputResponse(_, "unsupported-target"):
                self = .unsupportedTarget
            default:
                self = .unknown
            }
        }
    }

    @Published var connection: ConnectionState = .loading
    @Published var snapshot: Snapshot?
    @Published var outputs: [String: TerminalOutput] = [:]
    @Published var buffers: [String: CardBuffer] = [:]
    @Published var bufferErrors: [String: String] = [:]
    @Published var drafts: [String: MessageDraft] = [:]
    @Published var outputUnavailable: [String: OutputIssue] = [:]
    @Published var pendingSends: [String: CommandRecord] = [:]
    @Published var pendingCardCommands: [String: [CommandRecord]] = [:]
    @Published var hasPendingTaskCreate = false
    @Published var pendingTaskCreate: CommandRecord?
    @Published var operationReceipts: [String: CommandResult] = [:]
    @Published var draftPersistence: [String: DraftPersistenceStatus] = [:]
    @Published var sendingCards = Set<String>()
    @Published var busyCards = Set<String>()
    @Published var creatingTask = false
    @Published var message: String?
    @Published var refreshing = false
    @Published var loadingDetails = Set<String>()
    @Published var staleOutputs = Set<String>()
    @Published var lastSyncedAt: Date?
    @Published var noteDrafts: [String: NoteDraft] = [:]
    @Published var noteDraftErrors: [String: String] = [:]
    private var noteCoordinator: NoteDraftSaveCoordinator?
    private var noteVersions: [String: UInt64] = [:]
    private var noteTasks: [String: Task<Void, Never>] = [:]

    func loadNoteDraft(cardID: String, entryID: String? = nil) async -> NoteDraft? {
        let key = NoteDraft.storageKey(cardID: cardID, entryID: entryID)
        if let draft = noteDrafts[key] { return draft }
        guard let journal else { return nil }
        let epoch = bindingEpoch
        let draft = await journal.noteDraft(cardID: cardID, entryID: entryID)
        guard isCurrent(journal: journal, epoch: epoch) else { return nil }
        if noteDrafts[key] == nil { noteDrafts[key] = draft }
        return noteDrafts[key]
    }

    func stageNoteDraft(cardID: String, entryID: String? = nil, text: String) {
        guard let journal else { return }
        if noteCoordinator == nil { noteCoordinator = NoteDraftSaveCoordinator(sink: journal) }
        guard let coordinator = noteCoordinator else { return }
        let draft = NoteDraft(cardID: cardID, entryID: entryID, text: text)
        let key = draft.storageKey, epoch = bindingEpoch
        noteDrafts[key] = draft
        noteVersions[key, default: 0] &+= 1
        let version = noteVersions[key]!
        // No debounce: leaving the editor cannot cancel an unpersisted edit.
        noteTasks[key] = Task {
            do {
                _ = try await coordinator.persist(VersionedNoteDraft(draft: draft, version: version))
                guard self.isCurrent(journal: journal, epoch: epoch), self.noteVersions[key] == version else { return }
                self.noteDraftErrors.removeValue(forKey: key)
            } catch {
                guard self.isCurrent(journal: journal, epoch: epoch), self.noteVersions[key] == version else { return }
                self.noteDraftErrors[key] = error.localizedDescription
            }
        }
    }

    private var refreshTask: Task<Void, Never>?
    private var recoveryTask: Task<Void, Never>?
    private var recoverySequence: UInt64 = 0
    private var foregroundTask: Task<Void, Never>?
    private var detailTasks: [String: Task<Void, Never>] = [:]
    private var detailSequences: [String: UInt64] = [:]
    private var visibleCardID: String?
    private var foreground = true

    init() {}

    // Dependency injection uses the exact production scheduling/fencing paths.
    init(client: any ConnectorTransport, journal: CommandJournal) {
        self.client = client
        self.journal = journal
        self.draftCoordinator = DraftSaveCoordinator(sink: journal)
        self.connection = .connecting
    }

    func setForeground(_ active: Bool) {
        foreground = active
        traceNote("lifecycle-active-\(active)")
        foregroundTask?.cancel()
        foregroundTask = nil
        if !active {
            refreshSequence &+= 1
            refreshTask?.cancel(); refreshTask = nil
            refreshing = false
            recoverySequence &+= 1
            recoveryTask?.cancel(); recoveryTask = nil
            detailTasks.values.forEach { $0.cancel() }; detailTasks = [:]
            detailSequences = detailSequences.mapValues { $0 &+ 1 }
            loadingDetails = []
            staleOutputs.formUnion(outputs.keys)
            return
        }
        foregroundTask = Task { [weak self] in
            while !Task.isCancelled {
                guard let self else { return }
                await self.refresh()
                do { try await Task.sleep(for: .seconds(5)) } catch { return }
            }
        }
        scheduleRecovery()
    }

    func showDetails(_ cardID: String?) {
        if let old = visibleCardID, old != cardID {
            detailTasks[old]?.cancel(); detailTasks.removeValue(forKey: old)
            detailSequences[old, default: 0] &+= 1
            loadingDetails.remove(old)
        }
        visibleCardID = cardID
    }

    private func clearRemoteContent() {
        outputs = [:]; buffers = [:]; bufferErrors = [:]; outputUnavailable = [:]; staleOutputs = []
        detailTasks.values.forEach { $0.cancel() }; detailTasks = [:]
        detailSequences = detailSequences.mapValues { $0 &+ 1 }
        loadingDetails = []
    }

    func commandFailureMessage(_ code: String?) -> String {
        if code == "unsupported-target" { return ConnectorError.unsupportedTarget.localizedDescription }
        return code ?? "The command was rejected."
    }

    private let credentialStore = KeychainCredentialStore()
    private var client: (any ConnectorTransport)?
    private var journal: CommandJournal?
    private var draftCoordinator: DraftSaveCoordinator?
    private var draftSaveTasks: [String: Task<Void, Never>] = [:]
    private var latestDraftVersions: [String: UInt64] = [:]
    private var draftWriteSequences: [String: UInt64] = [:]
    private var bindingEpoch: UInt64 = 0
    private var refreshSequence: UInt64 = 0

    private var journalDirectory: URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        return base.appendingPathComponent("DeckConnector", isDirectory: true)
    }

    private func beginBinding() {
        bindingEpoch &+= 1
        let active = foreground
        setForeground(false)
        foreground = active
        client?.invalidate(); client = nil; journal = nil
        clearRemoteContent(); snapshot = nil; lastSyncedAt = nil
        draftSaveTasks.values.forEach { $0.cancel() }; draftSaveTasks = [:]
        drafts = [:]; draftPersistence = [:]; latestDraftVersions = [:]; draftWriteSequences = [:]
        draftCoordinator = nil; noteCoordinator = nil
        noteDrafts = [:]; noteDraftErrors = [:]; noteVersions = [:]; noteTasks = [:]
        pendingSends = [:]; pendingCardCommands = [:]; pendingTaskCreate = nil
        hasPendingTaskCreate = false; operationReceipts = [:]
    }

    func start() async {
        beginBinding()
        let epoch = bindingEpoch
        do {
            if let credential = try credentialStore.load() {
                let openedJournal = try await openJournal(for: credential)
                guard epoch == bindingEpoch else { return }
                journal = openedJournal
                draftCoordinator = DraftSaveCoordinator(sink: openedJournal)
                client = try DeckHTTPClient(credential: credential)
                connection = .connecting
                await refresh()
                // beginBinding cancels the old loop. Resume from the current
                // lifecycle state, which may have changed while startup awaited IO.
                if isCurrent(journal: openedJournal, epoch: epoch), foreground { setForeground(true) }
            } else {
                connection = .unpaired
            }
        } catch {
            guard epoch == bindingEpoch else { return }
            connection = .unpaired
            message = error.localizedDescription
        }
    }

    func pair(descriptor raw: String) async {
        beginBinding()
        let epoch = bindingEpoch
        connection = .connecting
        do {
            let descriptor = try PairingDescriptor.parse(raw)
            let pairingClient = try DeckHTTPClient(pairing: descriptor)
            defer { pairingClient.invalidate() }
            let response = try await pairingClient.pair(code: descriptor.code, deviceName: UIDevice.current.name)
            guard epoch == bindingEpoch else { return }
            guard response.version == 1, response.hostId == descriptor.hostId else { throw ConnectorError.invalidResponse }
            let credential = DeviceCredential(origin: descriptor.origin, fingerprint: descriptor.fingerprint, hostId: response.hostId, deviceId: response.deviceId, token: response.token)
            try credentialStore.save(credential)
            let openedJournal = try await openJournal(for: credential)
            guard epoch == bindingEpoch else { return }
            journal = openedJournal
            draftCoordinator = DraftSaveCoordinator(sink: openedJournal)
            client = try DeckHTTPClient(credential: credential)
            await refresh()
            if isCurrent(journal: openedJournal, epoch: epoch), foreground { setForeground(true) }
        } catch {
            guard epoch == bindingEpoch else { return }
            connection = .unpaired
            message = error.localizedDescription
        }
    }

    func unpair() {
        bindingEpoch &+= 1
        let active = foreground
        setForeground(false)
        foreground = active
        clearRemoteContent()
        do {
            try credentialStore.delete()
            client?.invalidate()
            client = nil
            journal = nil
            draftCoordinator = nil
            noteCoordinator = nil; noteDrafts = [:]; noteDraftErrors = [:]; noteVersions = [:]
            noteTasks = [:]
            draftSaveTasks.values.forEach { $0.cancel() }
            draftSaveTasks = [:]
            latestDraftVersions = [:]
            draftWriteSequences = [:]
            draftPersistence = [:]
            snapshot = nil
            outputs = [:]
            buffers = [:]
            drafts = [:]
            pendingSends = [:]
            pendingCardCommands = [:]
            hasPendingTaskCreate = false
            pendingTaskCreate = nil
            operationReceipts = [:]
            sendingCards = []
            busyCards = []
            creatingTask = false
            connection = .unpaired
        } catch { message = error.localizedDescription }
    }

    func refresh() async {
        if let refreshTask { await refreshTask.value; return }
        refreshSequence &+= 1
        let sequence = refreshSequence
        let task = Task { await self.performRefresh(sequence: sequence) }
        refreshTask = task
        await task.value
        if sequence == refreshSequence { refreshTask = nil }
    }

    private func performRefresh(sequence: UInt64) async {
        guard let client, let journal else { return }
        let epoch = bindingEpoch
        refreshing = true
        if snapshot == nil { connection = .connecting }
        defer { if sequence == refreshSequence { refreshing = false } }
        scheduleRecovery()
        do {
            let newSnapshot = try await client.snapshot()
            guard !Task.isCancelled, isCurrent(client: client, journal: journal, epoch: epoch), sequence == refreshSequence else { return }
            guard newSnapshot.version == 1, newSnapshot.hostId == client.hostID else { throw ConnectorError.invalidResponse }
            applySnapshot(newSnapshot)
            connection = .online
            lastSyncedAt = Date()
            // Publish the Board before local recovery/draft work and detail reads.
            for card in newSnapshot.cards {
                drafts[card.id] = await journal.draft(for: card.id)
                guard !Task.isCancelled, isCurrent(client: client, journal: journal, epoch: epoch) else { return }
            }
            await refreshJournalStatus(journal: journal, epoch: epoch)
            if let id = visibleCardID, let card = snapshot?.cards.first(where: { $0.id == id }) {
                await loadDetails(card: card)
            }
        } catch {
            guard !Task.isCancelled, isCurrent(client: client, journal: journal, epoch: epoch), sequence == refreshSequence else { return }
            if error as? ConnectorError == .revoked { clearRemoteContent(); snapshot = nil; connection = .revoked }
            else {
                if !Self.isNetworkLoss(error) { clearRemoteContent() }
                if let error = error as? ConnectorError, [.invalidResponse, .certificateMismatch, .untrustedServer, .redirected].contains(error) { snapshot = nil }
                staleOutputs.formUnion(outputs.keys)
                connection = .offline(error.localizedDescription)
            }
        }
    }

    func applySnapshot(_ value: Snapshot) {
        let current = Dictionary(value.cards.map { ($0.id, $0) }, uniquingKeysWith: { _, last in last })
        for old in snapshot?.cards ?? [] {
            let next = current[old.id]
            if next == nil || next?.generation != old.generation || next?.canReadOutput == false {
                outputs.removeValue(forKey: old.id); outputUnavailable.removeValue(forKey: old.id)
                staleOutputs.remove(old.id)
                detailTasks[old.id]?.cancel(); detailTasks.removeValue(forKey: old.id)
                detailSequences[old.id, default: 0] &+= 1
                loadingDetails.remove(old.id)
            }
            if next == nil || next?.canEditNotes == false { buffers.removeValue(forKey: old.id); bufferErrors.removeValue(forKey: old.id) }
        }
        snapshot = value
    }

    func loadDetails(card: CardSummary) async {
        if let existing = detailTasks[card.id] { await existing.value; return }
        guard let client, let journal, snapshot?.cards.contains(where: { $0.id == card.id && $0.generation == card.generation }) == true else { return }
        let epoch = bindingEpoch
        detailSequences[card.id, default: 0] &+= 1
        let sequence = detailSequences[card.id]!
        loadingDetails.insert(card.id)
        let task = Task {
            async let output: Void = self.loadOutput(card: card, client: client, journal: journal, epoch: epoch, sequence: sequence)
            if card.canEditNotes {
                async let buffer: Void = self.loadBuffer(card: card, client: client, journal: journal, epoch: epoch, sequence: sequence)
                _ = await (output, buffer)
            } else {
                if self.acceptsDetail(card: card, client: client, journal: journal, epoch: epoch, sequence: sequence) { self.buffers.removeValue(forKey: card.id) }
                _ = await output
            }
        }
        detailTasks[card.id] = task
        await task.value
        if detailSequences[card.id] == sequence {
            detailTasks.removeValue(forKey: card.id); loadingDetails.remove(card.id)
        }
    }

    private func acceptsDetail(card: CardSummary, client: any ConnectorTransport, journal: CommandJournal, epoch: UInt64, sequence: UInt64) -> Bool {
        !Task.isCancelled && isCurrent(client: client, journal: journal, epoch: epoch)
            && detailSequences[card.id] == sequence
            && snapshot?.cards.contains(where: { $0.id == card.id && $0.generation == card.generation }) == true
    }

    private func loadOutput(card: CardSummary, client: any ConnectorTransport, journal: CommandJournal, epoch: UInt64, sequence: UInt64) async {
        if !card.canReadOutput {
            guard acceptsDetail(card: card, client: client, journal: journal, epoch: epoch, sequence: sequence) else { return }
            let reason = card.capabilities?.readOutput?.reason ?? "unsupported-target"
            receiveOutput(cardID: card.id, result: .failure(ConnectorError.outputResponse(status: reason == "session-unavailable" ? 503 : 409, code: reason)))
            return
        }
        do {
            let output = try await client.output(cardID: card.id)
            guard acceptsDetail(card: card, client: client, journal: journal, epoch: epoch, sequence: sequence) else { return }
            guard output.cardId == card.id, output.generation == card.generation else {
                receiveOutput(cardID: card.id, result: .failure(ConnectorError.outputResponse(status: 409, code: "context-changed")))
                return
            }
            receiveOutput(cardID: card.id, result: .success(output))
        } catch {
            guard acceptsDetail(card: card, client: client, journal: journal, epoch: epoch, sequence: sequence) else { return }
            if error as? ConnectorError == .revoked { clearRemoteContent(); snapshot = nil; connection = .revoked }
            else { receiveOutput(cardID: card.id, result: .failure(error)) }
        }
    }

    static func isNetworkLoss(_ error: Error) -> Bool {
        guard let error = error as? URLError else { return false }
        return [.notConnectedToInternet, .networkConnectionLost, .timedOut, .cannotConnectToHost, .cannotFindHost, .dnsLookupFailed].contains(error.code)
    }

    func receiveOutput(cardID: String, result: Result<TerminalOutput, Error>) {
        switch result {
        case let .success(output):
            guard output.cardId == cardID else {
                outputs.removeValue(forKey: cardID); outputUnavailable[cardID] = .targetChanged; return
            }
            outputs[cardID] = output
            staleOutputs.remove(cardID)
            outputUnavailable.removeValue(forKey: cardID)
        case let .failure(error):
            if Self.isNetworkLoss(error), outputs[cardID] != nil { staleOutputs.insert(cardID) }
            else { outputs.removeValue(forKey: cardID); staleOutputs.remove(cardID) }
            outputUnavailable[cardID] = OutputIssue(error)
        }
    }

    private func loadBuffer(card: CardSummary, client: any ConnectorTransport, journal: CommandJournal, epoch: UInt64, sequence: UInt64) async {
        do {
            let buffer = try await client.buffer(cardID: card.id)
            guard acceptsDetail(card: card, client: client, journal: journal, epoch: epoch, sequence: sequence) else { return }
            buffers[card.id] = buffer
            bufferErrors.removeValue(forKey: card.id)
        } catch {
            guard acceptsDetail(card: card, client: client, journal: journal, epoch: epoch, sequence: sequence) else { return }
            if error as? ConnectorError == .revoked { clearRemoteContent(); snapshot = nil; connection = .revoked }
            else {
                bufferErrors[card.id] = String(localized: "Notes could not be updated. Refresh to try again.")
                if !Self.isNetworkLoss(error) { buffers.removeValue(forKey: card.id) }
            }
        }
    }

    private func scheduleRecovery() {
        traceNote("recovery-request existing=\(recoveryTask != nil)")
        guard foreground, recoveryTask == nil, let client, let journal else { return }
        let epoch = bindingEpoch
        recoverySequence &+= 1
        let sequence = recoverySequence
        recoveryTask = Task { [weak self] in
            guard let self else { return }
            defer { if self.isCurrent(client: client, journal: journal, epoch: epoch), self.recoverySequence == sequence { self.recoveryTask = nil } }
            for delay in [0, 1, 3] {
                if delay > 0 { do { try await Task.sleep(for: .seconds(delay)) } catch { return } }
                guard !Task.isCancelled, self.foreground, self.isCurrent(client: client, journal: journal, epoch: epoch) else { return }
                self.traceNote("recovery-query-start")
                let outcomes = await journal.recover(using: client, maximumQueries: 8)
                self.traceNote("recovery-query-end count=\(outcomes.count)")
                guard !Task.isCancelled, self.isCurrent(client: client, journal: journal, epoch: epoch) else { return }
                if outcomes.values.contains(where: { if case .failure(ConnectorError.revoked) = $0 { return true }; return false }) {
                    self.clearRemoteContent(); self.snapshot = nil; self.connection = .revoked; return
                }
                await self.refreshJournalStatus(journal: journal, epoch: epoch)
                if outcomes.isEmpty { return }
            }
        }
    }

    func stageDraft(_ value: VersionedMessageDraft) {
        guard let coordinator = draftCoordinator else { return }
        let epoch = bindingEpoch
        let cardID = value.draft.cardID
        let sequence = (draftWriteSequences[cardID] ?? 0) &+ 1
        draftWriteSequences[cardID] = sequence
        let persistenceValue = VersionedMessageDraft(draft: value.draft, version: sequence)
        latestDraftVersions[cardID] = max(latestDraftVersions[cardID] ?? 0, value.version)
        draftPersistence[cardID] = .saving(value.version)
        draftSaveTasks[cardID]?.cancel()
        draftSaveTasks[cardID] = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(120))
            guard !Task.isCancelled else { return }
            do {
                let receipt = try await coordinator.persist(persistenceValue)
                guard let self, epoch == self.bindingEpoch, self.draftCoordinator === coordinator,
                      self.latestDraftVersions[cardID] == value.version else { return }
                if case .saved = receipt {
                    self.drafts[cardID] = value.draft
                    self.draftPersistence[cardID] = .saved(value.version)
                }
            } catch {
                guard let self, epoch == self.bindingEpoch, self.draftCoordinator === coordinator,
                      self.latestDraftVersions[cardID] == value.version else { return }
                self.draftPersistence[cardID] = .failed(value.version, error.localizedDescription)
            }
        }
    }

    func acceptCurrentGeneration(card: CardSummary) async {
        guard let journal, let old = drafts[card.id] else { return }
        let epoch = bindingEpoch
        let draft = MessageDraft(cardID: card.id, text: old.text, expectedGeneration: card.generation)
        do { try await journal.saveDraft(draft); guard isCurrent(journal: journal, epoch: epoch) else { return }; drafts[card.id] = draft }
        catch { if isCurrent(journal: journal, epoch: epoch) { setError(error) } }
    }

    func send(card: CardSummary, visible value: VersionedMessageDraft) async -> MutationOutcome {
        guard let journal, let client, let coordinator = draftCoordinator,
              !value.draft.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return .failed("Draft is empty or Deck is offline.") }
        guard !sendingCards.contains(card.id), pendingSends[card.id] == nil else {
            message = "This draft already has a durable operation. Check that original operation before sending again."
            return .pending(id: pendingSends[card.id]?.id, state: "original operation")
        }
        guard card.maySend else { return .failed(String(localized: "This card is not available for sending.")) }
        guard let expectedGeneration = value.draft.expectedGeneration else {
            message = "The desktop has no recognized Codex or Claude target for this task."
            return .failed(message ?? "Target unavailable.")
        }
        guard value.draft.expectedGeneration == card.generation else {
            message = "This task restarted after the draft was created. Review the target and accept the current generation before sending."
            return .failed(message ?? "Target changed.")
        }
        sendingCards.insert(card.id)
        let epoch = bindingEpoch
        defer { if epoch == bindingEpoch { sendingCards.remove(card.id) } }
        let sendSequence = (draftWriteSequences[card.id] ?? 0) &+ 1
        draftWriteSequences[card.id] = sendSequence
        draftSaveTasks[card.id]?.cancel()
        if let pendingSave = draftSaveTasks[card.id] { await pendingSave.value }
        do {
            _ = try await coordinator.persistForSend(VersionedMessageDraft(draft: value.draft, version: sendSequence))
            guard isCurrent(client: client, journal: journal, epoch: epoch), draftCoordinator === coordinator else { return .failed("Pairing changed.") }
            drafts[card.id] = value.draft
            draftPersistence[card.id] = .saved(value.version)
        } catch {
            guard isCurrent(client: client, journal: journal, epoch: epoch) else { return .failed("Pairing changed.") }
            draftPersistence[card.id] = .failed(value.version, error.localizedDescription)
            message = "Message was not sent because its current draft could not be saved: \(error.localizedDescription)"
            return .failed(error.localizedDescription)
        }
        let command = CommandRequest(id: UUID().uuidString.lowercased(), kind: "send-message", cardId: card.id, expectedGeneration: expectedGeneration, payload: ["text": .string(value.draft.text)])
        return await execute(command, draftCardID: card.id, client: client, journal: journal)
    }

    func checkOriginalOperations() async {
        scheduleRecovery()
        await recoveryTask?.value
        await refresh()
    }

    func retryOriginal(_ record: CommandRecord) async {
        guard record.localState == .notFound, let client, let journal else { return }
        let epoch = bindingEpoch
        do {
            _ = try await journal.retryNotFound(id: record.id, using: client)
            guard isCurrent(client: client, journal: journal, epoch: epoch) else { return }
            _ = await journal.canonicalResult(id: record.id, consume: true)
            await refreshJournalStatus(journal: journal, epoch: epoch)
            scheduleRefreshAfterMutation(client: client, journal: journal, epoch: epoch)
        } catch { if isCurrent(client: client, journal: journal, epoch: epoch) { setError(error) } }
    }

    private func persistNoteBeforeCommand(cardID: String, entryID: String? = nil, text: String) async -> (saved: Bool, revision: UUID?) {
        let key = NoteDraft.storageKey(cardID: cardID, entryID: entryID)
        guard let draft = noteDrafts[key], draft.text == text else { return (true, nil) }
        guard let journal else { return (false, nil) }
        // A draft restored from disk can be submitted without another edit.
        if noteCoordinator == nil { noteCoordinator = NoteDraftSaveCoordinator(sink: journal) }
        guard let coordinator = noteCoordinator else { return (false, nil) }
        let epoch = bindingEpoch, version = noteVersions[key] ?? 0
        do {
            _ = try await coordinator.persist(VersionedNoteDraft(draft: draft, version: version))
            return (isCurrent(journal: journal, epoch: epoch), draft.revision)
        } catch {
            if isCurrent(journal: journal, epoch: epoch) { noteDraftErrors[key] = error.localizedDescription }
            return (false, nil)
        }
    }

    func bufferAdd(card: CardSummary, text: String) async -> MutationOutcome {
        guard card.canEditNotes else { return .failed(ConnectorError.unsupportedTarget.localizedDescription) }
        guard pendingCardCommands[card.id]?.isEmpty ?? true else { return .pending(id: pendingCardCommands[card.id]?.first?.id, state: "original operation") }
        guard let revision = buffers[card.id]?.revision.value else { return .failed("Scratchpad is unavailable.") }
        let persisted = await persistNoteBeforeCommand(cardID: card.id, text: text)
        guard persisted.saved else { return .failed(String(localized: "Save the local draft before retrying.")) }
        return await execute(kind: "buffer-add", card: card, expectedRevision: revision, noteDraftRevision: persisted.revision, payload: ["text": .string(text)])
    }

    func bufferEdit(card: CardSummary, entry: BufferEntry, text: String) async -> MutationOutcome {
        guard card.canEditNotes else { return .failed(ConnectorError.unsupportedTarget.localizedDescription) }
        guard pendingCardCommands[card.id]?.isEmpty ?? true else { return .pending(id: pendingCardCommands[card.id]?.first?.id, state: "original operation") }
        guard entry.kind == "manual", let revision = buffers[card.id]?.revision.value else { return .failed("Only manual notes can be edited.") }
        let persisted = await persistNoteBeforeCommand(cardID: card.id, entryID: entry.id, text: text)
        guard persisted.saved else { return .failed(String(localized: "Save the local draft before retrying.")) }
        return await execute(kind: "buffer-edit", card: card, expectedRevision: revision, noteDraftRevision: persisted.revision, payload: ["entryId": .string(entry.id), "text": .string(text)])
    }

    func bufferDelete(card: CardSummary, entry: BufferEntry) async -> MutationOutcome {
        guard card.canEditNotes else { return .failed(ConnectorError.unsupportedTarget.localizedDescription) }
        guard pendingCardCommands[card.id]?.isEmpty ?? true else { return .pending(id: pendingCardCommands[card.id]?.first?.id, state: "original operation") }
        guard let revision = buffers[card.id]?.revision.value else { return .failed("Scratchpad is unavailable.") }
        return await execute(kind: "buffer-delete", card: card, expectedRevision: revision, payload: ["entryId": .string(entry.id)])
    }

    func bufferQueue(card: CardSummary, entryIDs: [String]) async -> MutationOutcome {
        guard card.mayQueue else { return .failed("Queueing requires a desktop-saved Codex or Claude launch configuration for this task.") }
        guard pendingCardCommands[card.id]?.isEmpty ?? true else { return .pending(id: pendingCardCommands[card.id]?.first?.id, state: "original operation") }
        guard !entryIDs.isEmpty, let revision = buffers[card.id]?.revision.value else { return .failed("Select at least one current note.") }
        return await execute(kind: "buffer-queue", card: card, expectedRevision: revision, payload: ["entryIds": .strings(entryIDs)])
    }

    func createTask(projectID: String, presetID: String) async {
        guard !creatingTask, !hasPendingTaskCreate else { message = "Check the original task creation operation before creating another task."; return }
        guard let revision = snapshot?.revision.value else { return }
        creatingTask = true
        let epoch = bindingEpoch
        defer { if epoch == bindingEpoch { creatingTask = false } }
        let command = CommandRequest(id: UUID().uuidString.lowercased(), kind: "task-create", expectedRevision: revision, payload: ["projectId": .string(projectID), "presetId": .string(presetID)])
        _ = await execute(command)
    }

    func setQueuePaused(card: CardSummary, item: QueueItem, paused: Bool) async {
        guard card.mayQueue else { message = ConnectorError.unsupportedTarget.localizedDescription; return }
        _ = await execute(kind: "queue-pause", card: card, expectedGeneration: card.generation, payload: ["itemId": .string(item.id), "paused": .bool(paused), "revision": .string(item.revision.value)])
    }

    func cancelQueue(card: CardSummary, item: QueueItem) async {
        guard card.mayQueue else { message = ConnectorError.unsupportedTarget.localizedDescription; return }
        _ = await execute(kind: "queue-cancel", card: card, expectedGeneration: card.generation, payload: ["itemId": .string(item.id), "revision": .string(item.revision.value)])
    }

    private func execute(kind: String, card: CardSummary, expectedGeneration: String? = nil, expectedRevision: String? = nil, noteDraftRevision: UUID? = nil, payload: [String: JSONValue]) async -> MutationOutcome {
        guard !busyCards.contains(card.id), pendingCardCommands[card.id]?.isEmpty ?? true else { return .pending(id: pendingCardCommands[card.id]?.first?.id, state: "original operation") }
        busyCards.insert(card.id)
        let epoch = bindingEpoch
        defer { if epoch == bindingEpoch { busyCards.remove(card.id) } }
        let command = CommandRequest(id: UUID().uuidString.lowercased(), kind: kind, cardId: card.id, expectedGeneration: expectedGeneration, expectedRevision: expectedRevision, payload: payload)
        return await execute(command, noteDraftRevision: noteDraftRevision)
    }

    // Disposable Simulator diagnostics contain stages/counts only, never note text or credentials.
    func traceNote(_ stage: String) {
        #if DEBUG && targetEnvironment(simulator)
        guard ProcessInfo.processInfo.arguments.contains("--deck-diagnostic") else { return }
        let path = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0].appendingPathComponent("note-stages.log")
        let line = "\(Date().timeIntervalSince1970) \(stage) foreground=\(foreground) drafts=\(noteDrafts.count) busy=\(busyCards.count)\n"
        guard let data = line.data(using: .utf8) else { return }
        if !FileManager.default.fileExists(atPath: path.path) { try? data.write(to: path) }
        else if let file = try? FileHandle(forWritingTo: path) {
            defer { try? file.close() }
            if let offset = try? file.seekToEnd(), offset < 64 * 1024 { try? file.write(contentsOf: data) }
        }
        #endif
    }

    private func execute(_ command: CommandRequest, draftCardID: String? = nil, noteDraftRevision: UUID? = nil, client explicitClient: (any ConnectorTransport)? = nil, journal explicitJournal: CommandJournal? = nil) async -> MutationOutcome {
        guard let client = explicitClient ?? client, let journal = explicitJournal ?? journal else { return .failed("Deck is offline.") }
        let epoch = bindingEpoch
        do {
            traceNote("execute-start")
            let submittedResult = try await journal.submit(command, draftCardID: draftCardID, noteDraftRevision: noteDraftRevision, using: client)
            guard isCurrent(client: client, journal: journal, epoch: epoch) else { return .failed("Pairing changed.") }
            traceNote("submit-return-\(submittedResult.state.rawValue)")
            let canonical = await journal.canonicalResult(id: command.id) ?? submittedResult
            let terminal = [.applied, .delivered, .rejected].contains(canonical.state)
            let result = await journal.canonicalResult(id: command.id, consume: terminal) ?? canonical
            await refreshJournalStatus(journal: journal, epoch: epoch)
            traceNote("journal-status-return")
            if let cardID = command.cardId {
                let currentDraft = await journal.draft(for: cardID)
                guard isCurrent(client: client, journal: journal, epoch: epoch) else { return .failed("Pairing changed.") }
                drafts[cardID] = currentDraft
            }
            guard isCurrent(client: client, journal: journal, epoch: epoch) else { return .failed("Pairing changed.") }
            if result.state == .rejected || result.state == .ambiguous {
                message = commandFailureMessage(result.code)
            }
            scheduleRefreshAfterMutation(client: client, journal: journal, epoch: epoch)
            traceNote("execute-return-\(result.state.rawValue)")
            switch result.state {
            case .applied, .delivered: return .applied
            case .accepted, .ambiguous: return .pending(id: command.id, state: result.state.rawValue)
            case .rejected: return .failed(commandFailureMessage(result.code))
            }
        } catch {
            guard isCurrent(client: client, journal: journal, epoch: epoch) else { return .failed("Pairing changed.") }
            setError(error)
            await refreshJournalStatus(journal: journal, epoch: epoch)
            scheduleRecovery()
            return .failed(error.localizedDescription)
        }
    }

    private func scheduleRefreshAfterMutation(client: any ConnectorTransport, journal: CommandJournal, epoch: UInt64) {
        guard foreground, isCurrent(client: client, journal: journal, epoch: epoch) else { return }
        scheduleRecovery()
        Task { [weak self] in
            guard let self, self.foreground, self.isCurrent(client: client, journal: journal, epoch: epoch) else { return }
            await self.refresh()
        }
    }

    private func openJournal(for credential: DeviceCredential) async throws -> CommandJournal {
        let binding = credential.journalBinding
        let directory = journalDirectory.appendingPathComponent(binding.storageKey, isDirectory: true)
        return try await CommandJournal.open(storage: FileJournalStorage(directory: directory), binding: binding)
    }

    private func refreshJournalStatus(journal explicitJournal: CommandJournal? = nil, epoch explicitEpoch: UInt64? = nil) async {
        guard let journal = explicitJournal ?? journal else { pendingSends = [:]; pendingCardCommands = [:]; hasPendingTaskCreate = false; pendingTaskCreate = nil; return }
        let epoch = explicitEpoch ?? bindingEpoch
        let records = await journal.allRecords()
        guard isCurrent(journal: journal, epoch: epoch) else { return }
        let unresolved = records.filter {
            $0.request.kind == "send-message" && [.prepared, .unknown, .notFound, .awaitingFinal, .ambiguous, .expired].contains($0.localState)
        }
        pendingSends = Dictionary(unresolved.compactMap { record in record.request.cardId.map { ($0, record) } }, uniquingKeysWith: { _, newest in newest })
        pendingCardCommands = Dictionary(grouping: records.filter {
            $0.request.kind != "send-message" && [.prepared, .unknown, .notFound, .awaitingFinal, .ambiguous, .expired].contains($0.localState) && $0.request.cardId != nil
        }, by: { $0.request.cardId! })
        pendingTaskCreate = records.last {
            $0.request.kind == "task-create" && [.prepared, .unknown, .notFound, .awaitingFinal, .ambiguous, .expired].contains($0.localState)
        }
        hasPendingTaskCreate = pendingTaskCreate != nil
        for record in records where record.localState == .resolved {
            if record.request.kind == "send-message", let cardID = record.request.cardId {
                let draft = await journal.draft(for: cardID)
                guard isCurrent(journal: journal, epoch: epoch) else { return }
                drafts[cardID] = draft
            }
            if let cardID = record.request.cardId, ["buffer-add", "buffer-edit"].contains(record.request.kind) {
                let entryID: String?
                if case let .string(id)? = record.request.payload["entryId"] { entryID = id } else { entryID = nil }
                let key = NoteDraft.storageKey(cardID: cardID, entryID: entryID)
                let before = noteDrafts[key]
                let persisted = await journal.noteDraft(cardID: cardID, entryID: entryID)
                guard isCurrent(journal: journal, epoch: epoch) else { return }
                // A newer in-memory edit may still be waiting for its atomic save.
                // An older receipt can clear only the exact submitted revision.
                if noteDrafts[key] == before {
                    if before == nil { noteDrafts[key] = persisted }
                    else if persisted == nil, before?.revision == record.noteDraftRevision { noteDrafts.removeValue(forKey: key) }
                }
            }
            _ = await journal.canonicalResult(id: record.id, consume: true)
        }
        guard isCurrent(journal: journal, epoch: epoch) else { return }
        operationReceipts = Dictionary(uniqueKeysWithValues: records.compactMap { record in
            guard let result = record.result, [.resolved, .ambiguous].contains(record.localState) else { return nil }
            return (record.id, result)
        })
    }

    private func setError(_ error: Error) {
        if error as? ConnectorError == .revoked { clearRemoteContent(); snapshot = nil; connection = .revoked }
        message = error.localizedDescription
    }

    private func isCurrent(client expectedClient: (any ConnectorTransport)? = nil, journal expectedJournal: CommandJournal, epoch: UInt64) -> Bool {
        epoch == bindingEpoch && journal === expectedJournal && (expectedClient == nil || client === expectedClient)
    }
}
