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

    @Published var connection: ConnectionState = .loading
    @Published var snapshot: Snapshot?
    @Published var outputs: [String: TerminalOutput] = [:]
    @Published var buffers: [String: CardBuffer] = [:]
    @Published var drafts: [String: MessageDraft] = [:]
    @Published var outputUnavailable: [String: String] = [:]
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

    private let credentialStore = KeychainCredentialStore()
    private var client: DeckHTTPClient?
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

    func start() async {
        bindingEpoch &+= 1
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
        bindingEpoch &+= 1
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
        } catch {
            guard epoch == bindingEpoch else { return }
            connection = .unpaired
            message = error.localizedDescription
        }
    }

    func unpair() {
        bindingEpoch &+= 1
        do {
            try credentialStore.delete()
            client?.invalidate()
            client = nil
            journal = nil
            draftCoordinator = nil
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
        guard let client, let journal else { if connection != .loading { connection = .unpaired }; return }
        let epoch = bindingEpoch
        refreshSequence &+= 1
        let sequence = refreshSequence
        connection = .connecting
        do {
            _ = await journal.recover(using: client)
            guard isCurrent(client: client, journal: journal, epoch: epoch), sequence == refreshSequence else { return }
            let newSnapshot = try await client.snapshot()
            guard isCurrent(client: client, journal: journal, epoch: epoch), sequence == refreshSequence else { return }
            guard newSnapshot.hostId == client.hostID else { throw ConnectorError.invalidResponse }
            snapshot = newSnapshot
            for card in newSnapshot.cards {
                drafts[card.id] = await journal.draft(for: card.id)
                guard isCurrent(client: client, journal: journal, epoch: epoch), sequence == refreshSequence else { return }
            }
            await refreshJournalStatus(journal: journal, epoch: epoch)
            guard isCurrent(client: client, journal: journal, epoch: epoch), sequence == refreshSequence else { return }
            connection = .online
        } catch ConnectorError.revoked {
            guard isCurrent(client: client, journal: journal, epoch: epoch), sequence == refreshSequence else { return }
            connection = .revoked
        } catch {
            guard isCurrent(client: client, journal: journal, epoch: epoch), sequence == refreshSequence else { return }
            connection = .offline(error.localizedDescription)
        }
    }

    func loadDetails(card: CardSummary) async {
        guard let client, let journal else { return }
        let epoch = bindingEpoch
        async let outputLoad: Void = loadOutput(cardID: card.id, client: client, journal: journal, epoch: epoch)
        async let bufferLoad: Void = loadBuffer(cardID: card.id, client: client, journal: journal, epoch: epoch)
        _ = await (outputLoad, bufferLoad)
    }

    private func loadOutput(cardID: String, client: DeckHTTPClient, journal: CommandJournal, epoch: UInt64) async {
        do {
            let output = try await client.output(cardID: cardID)
            guard isCurrent(client: client, journal: journal, epoch: epoch) else { return }
            outputs[cardID] = output
            outputUnavailable.removeValue(forKey: cardID)
        } catch {
            guard isCurrent(client: client, journal: journal, epoch: epoch) else { return }
            outputs.removeValue(forKey: cardID)
            outputUnavailable[cardID] = error.localizedDescription
        }
    }

    private func loadBuffer(cardID: String, client: DeckHTTPClient, journal: CommandJournal, epoch: UInt64) async {
        do {
            let buffer = try await client.buffer(cardID: cardID)
            guard isCurrent(client: client, journal: journal, epoch: epoch) else { return }
            buffers[cardID] = buffer
        } catch { if isCurrent(client: client, journal: journal, epoch: epoch) { setError(error) } }
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
        await refresh()
    }

    func retryOriginal(_ record: CommandRecord) async {
        guard record.localState == .notFound, let client, let journal else { return }
        let epoch = bindingEpoch
        do {
            _ = try await journal.retryNotFound(id: record.id, using: client)
            guard isCurrent(client: client, journal: journal, epoch: epoch) else { return }
            await refresh()
            guard isCurrent(client: client, journal: journal, epoch: epoch) else { return }
            _ = await journal.canonicalResult(id: record.id, consume: true)
            await refreshJournalStatus(journal: journal, epoch: epoch)
        } catch { if isCurrent(client: client, journal: journal, epoch: epoch) { setError(error) } }
    }

    func bufferAdd(card: CardSummary, text: String) async -> MutationOutcome {
        guard pendingCardCommands[card.id]?.isEmpty ?? true else { return .pending(id: pendingCardCommands[card.id]?.first?.id, state: "original operation") }
        guard let revision = buffers[card.id]?.revision.value else { return .failed("Scratchpad is unavailable.") }
        return await execute(kind: "buffer-add", card: card, expectedRevision: revision, payload: ["text": .string(text)])
    }

    func bufferEdit(card: CardSummary, entry: BufferEntry, text: String) async -> MutationOutcome {
        guard pendingCardCommands[card.id]?.isEmpty ?? true else { return .pending(id: pendingCardCommands[card.id]?.first?.id, state: "original operation") }
        guard entry.kind == "manual", let revision = buffers[card.id]?.revision.value else { return .failed("Only manual notes can be edited.") }
        return await execute(kind: "buffer-edit", card: card, expectedRevision: revision, payload: ["entryId": .string(entry.id), "text": .string(text)])
    }

    func bufferDelete(card: CardSummary, entry: BufferEntry) async -> MutationOutcome {
        guard pendingCardCommands[card.id]?.isEmpty ?? true else { return .pending(id: pendingCardCommands[card.id]?.first?.id, state: "original operation") }
        guard let revision = buffers[card.id]?.revision.value else { return .failed("Scratchpad is unavailable.") }
        return await execute(kind: "buffer-delete", card: card, expectedRevision: revision, payload: ["entryId": .string(entry.id)])
    }

    func bufferQueue(card: CardSummary, entryIDs: [String]) async -> MutationOutcome {
        guard card.canQueue else { return .failed("Queueing requires a desktop-saved Codex or Claude launch configuration for this task.") }
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
        _ = await execute(kind: "queue-pause", card: card, expectedGeneration: card.generation, payload: ["itemId": .string(item.id), "paused": .bool(paused), "revision": .string(item.revision.value)])
    }

    func cancelQueue(card: CardSummary, item: QueueItem) async {
        _ = await execute(kind: "queue-cancel", card: card, expectedGeneration: card.generation, payload: ["itemId": .string(item.id), "revision": .string(item.revision.value)])
    }

    private func execute(kind: String, card: CardSummary, expectedGeneration: String? = nil, expectedRevision: String? = nil, payload: [String: JSONValue]) async -> MutationOutcome {
        guard !busyCards.contains(card.id), pendingCardCommands[card.id]?.isEmpty ?? true else { return .pending(id: pendingCardCommands[card.id]?.first?.id, state: "original operation") }
        busyCards.insert(card.id)
        let epoch = bindingEpoch
        defer { if epoch == bindingEpoch { busyCards.remove(card.id) } }
        let command = CommandRequest(id: UUID().uuidString.lowercased(), kind: kind, cardId: card.id, expectedGeneration: expectedGeneration, expectedRevision: expectedRevision, payload: payload)
        return await execute(command)
    }

    private func execute(_ command: CommandRequest, draftCardID: String? = nil, client explicitClient: DeckHTTPClient? = nil, journal explicitJournal: CommandJournal? = nil) async -> MutationOutcome {
        guard let client = explicitClient ?? client, let journal = explicitJournal ?? journal else { return .failed("Deck is offline.") }
        let epoch = bindingEpoch
        do {
            let submittedResult = try await journal.submit(command, draftCardID: draftCardID, using: client)
            guard isCurrent(client: client, journal: journal, epoch: epoch) else { return .failed("Pairing changed.") }
            await refresh()
            guard isCurrent(client: client, journal: journal, epoch: epoch) else { return .failed("Pairing changed.") }
            let canonical = await journal.canonicalResult(id: command.id) ?? submittedResult
            let terminal = [.applied, .delivered, .rejected].contains(canonical.state)
            let result = await journal.canonicalResult(id: command.id, consume: terminal) ?? canonical
            await refreshJournalStatus(journal: journal, epoch: epoch)
            if let cardID = command.cardId {
                let currentDraft = await journal.draft(for: cardID)
                guard isCurrent(client: client, journal: journal, epoch: epoch) else { return .failed("Pairing changed.") }
                drafts[cardID] = currentDraft
            }
            guard isCurrent(client: client, journal: journal, epoch: epoch) else { return .failed("Pairing changed.") }
            if result.state == .rejected || result.state == .ambiguous {
                message = result.code.map { "Command \(result.state.rawValue): \($0)" } ?? "Command \(result.state.rawValue)."
            }
            if let cardID = command.cardId, let card = snapshot?.cards.first(where: { $0.id == cardID }) { await loadDetails(card: card) }
            switch result.state {
            case .applied, .delivered: return .applied
            case .accepted, .ambiguous: return .pending(id: command.id, state: result.state.rawValue)
            case .rejected: return .failed(result.code ?? "rejected")
            }
        } catch {
            guard isCurrent(client: client, journal: journal, epoch: epoch) else { return .failed("Pairing changed.") }
            setError(error)
            await refreshJournalStatus(journal: journal, epoch: epoch)
            return .failed(error.localizedDescription)
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
            $0.request.kind == "send-message" && [.prepared, .unknown, .notFound, .awaitingFinal, .ambiguous].contains($0.localState)
        }
        pendingSends = Dictionary(unresolved.compactMap { record in record.request.cardId.map { ($0, record) } }, uniquingKeysWith: { _, newest in newest })
        pendingCardCommands = Dictionary(grouping: records.filter {
            $0.request.kind != "send-message" && [.prepared, .unknown, .notFound, .awaitingFinal, .ambiguous].contains($0.localState) && $0.request.cardId != nil
        }, by: { $0.request.cardId! })
        pendingTaskCreate = records.last {
            $0.request.kind == "task-create" && [.prepared, .unknown, .notFound, .awaitingFinal, .ambiguous].contains($0.localState)
        }
        hasPendingTaskCreate = pendingTaskCreate != nil
        operationReceipts = Dictionary(uniqueKeysWithValues: records.compactMap { record in
            guard let result = record.result, [.resolved, .ambiguous].contains(record.localState) else { return nil }
            return (record.id, result)
        })
        for record in records where record.localState == .resolved {
            _ = await journal.canonicalResult(id: record.id, consume: true)
        }
    }

    private func setError(_ error: Error) {
        if error as? ConnectorError == .revoked { connection = .revoked }
        message = error.localizedDescription
    }

    private func isCurrent(client expectedClient: DeckHTTPClient? = nil, journal expectedJournal: CommandJournal, epoch: UInt64) -> Bool {
        epoch == bindingEpoch && journal === expectedJournal && (expectedClient == nil || client === expectedClient)
    }
}
