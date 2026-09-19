import DeckConnectorCore
import SwiftUI

struct RootView: View {
    @EnvironmentObject private var model: AppModel
    @State private var confirmUnpair = false

    var body: some View {
        Group {
            switch model.connection {
            case .loading:
                ProgressView("Loading…")
            case .unpaired:
                PairingView()
            case .revoked:
                ContentUnavailableView("Device revoked", systemImage: "lock.slash", description: Text("Revoke status comes from the desktop host. Remove this pairing, then create a new QR code on Deck."))
                    .safeAreaInset(edge: .bottom) { Button("Remove pairing", role: .destructive) { confirmUnpair = true }.buttonStyle(.borderedProminent).padding() }
            case .connecting, .online, .offline:
                TaskListView(confirmUnpair: $confirmUnpair)
            }
        }
        .alert("Deck Connector", isPresented: Binding(get: { model.message != nil }, set: { if !$0 { model.message = nil } })) {
            Button("OK") { model.message = nil }
        } message: { Text(model.message ?? "") }
        .confirmationDialog("Remove this pairing from this phone?", isPresented: $confirmUnpair, titleVisibility: .visible) {
            Button("Remove pairing", role: .destructive) { model.unpair() }
            Button("Cancel", role: .cancel) { }
        } message: { Text("The local recovery archive and unresolved operation history remain on this device.") }
    }
}

struct PairingView: View {
    @EnvironmentObject private var model: AppModel
    @State private var descriptor = ""
    @State private var scanning = false

    var body: some View {
        NavigationStack {
            Form {
                Section("Pair with Deck") {
                    Text("On the desktop, enable Connector and create a short-lived pairing code. The phone connects only to that pinned HTTPS host.")
                    Button("Scan QR code") { scanning = true }
                    TextField("Paste deck-connector:// pairing code", text: $descriptor, axis: .vertical)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                    Button("Pair") { Task { await model.pair(descriptor: descriptor); descriptor = "" } }
                        .disabled(descriptor.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
                Section { Text("Pairing data is used once and is not stored. The host identity, certificate pin, device ID and token are stored together in Keychain.").font(.footnote).foregroundStyle(.secondary) }
            }
            .navigationTitle("Deck Connector")
            .sheet(isPresented: $scanning) {
                NavigationStack {
                    QRScannerView { value in descriptor = value; scanning = false }
                        .navigationTitle("Scan Deck QR")
                        .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Cancel") { scanning = false } } }
                }
            }
        }
    }
}

struct TaskListView: View {
    @EnvironmentObject private var model: AppModel
    @Binding var confirmUnpair: Bool

    var body: some View {
        NavigationStack {
            List {
                connectionBanner
                if let snapshot = model.snapshot {
                    ForEach(snapshot.projects) { project in
                        Section {
                            ForEach(snapshot.cards.filter { $0.projectId == project.id }) { card in
                                NavigationLink(value: card.id) {
                                    VStack(alignment: .leading) {
                                        Text(card.title)
                                        Text("\(card.status) · \(card.buffer.entryCount) notes")
                                            .font(.caption).foregroundStyle(.secondary)
                                    }
                                }
                            }
                        } header: {
                            HStack {
                                Text(project.name)
                                Spacer()
                                if !project.presets.isEmpty {
                                    Menu {
                                        ForEach(project.presets) { preset in
                                            Button(preset.name) { Task { await model.createTask(projectID: project.id, presetID: preset.id) } }
                                        }
                                    } label: { Label("New", systemImage: "plus") }
                                        .disabled(model.creatingTask || model.hasPendingTaskCreate)
                                    if let pending = model.pendingTaskCreate {
                                        Button(pending.localState == .notFound ? "Retry task creation" : "Check task creation") {
                                            Task { if pending.localState == .notFound { await model.retryOriginal(pending) } else { await model.checkOriginalOperations() } }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    queueSection(snapshot)
                } else {
                    ContentUnavailableView("No snapshot", systemImage: "desktopcomputer.trianglebadge.exclamationmark", description: Text("Deck must be awake, running, and reachable on the local network or your VPN."))
                }
            }
            .navigationTitle("Tasks")
            .navigationDestination(for: String.self) { id in
                if let card = model.snapshot?.cards.first(where: { $0.id == id }) { TaskDetailView(cardID: card.id) }
            }
            .refreshable { await model.refresh() }
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) { Menu { Button("Refresh") { Task { await model.refresh() } }; Button("Remove pairing", role: .destructive) { confirmUnpair = true } } label: { Image(systemName: "ellipsis.circle") } }
            }
        }
    }

    @ViewBuilder private var connectionBanner: some View {
        switch model.connection {
        case .connecting: HStack { ProgressView(); Text("Connecting to desktop…") }
        case let .offline(reason): Label(reason, systemImage: "wifi.slash").foregroundStyle(.orange)
        default: EmptyView()
        }
    }

    @ViewBuilder private func queueSection(_ snapshot: Snapshot) -> some View {
        if !snapshot.queue.isEmpty {
            Section("Queue") {
                ForEach(snapshot.queue) { item in
                    if let card = snapshot.cards.first(where: { $0.id == item.cardId }) {
                        HStack {
                            VStack(alignment: .leading) { Text(card.title); Text(item.state).font(.caption).foregroundStyle(.secondary) }
                            Spacer()
                            Button(item.paused ? "Resume" : "Pause") { Task { await model.setQueuePaused(card: card, item: item, paused: !item.paused) } }.buttonStyle(.bordered).disabled(model.busyCards.contains(card.id) || !(model.pendingCardCommands[card.id]?.isEmpty ?? true))
                            Button("Cancel", role: .destructive) { Task { await model.cancelQueue(card: card, item: item) } }.disabled(item.state != "pending" || model.busyCards.contains(card.id) || !(model.pendingCardCommands[card.id]?.isEmpty ?? true))
                        }
                    }
                }
            }
        }
    }
}

struct TaskDetailView: View {
    private enum NoteSaveState: Equatable {
        case idle, saving, pending, saved, savedWithNewerDraft, failed(String)
    }

    private enum FocusedField: Hashable { case note, message }

    @EnvironmentObject private var model: AppModel
    let cardID: String
    @State private var newNote = ""
    @State private var addingNote = false
    @State private var noteSaveState = NoteSaveState.idle
    @State private var selection = Set<String>()
    @State private var queueingSelection = false
    @State private var pendingAdd: (id: String, text: String)?
    @State private var pendingQueue: (id: String, selection: Set<String>)?
    @State private var editing: BufferEntry?
    @State private var composer: ComposerDraftState
    @FocusState private var focusedField: FocusedField?

    init(cardID: String) {
        self.cardID = cardID
        _composer = State(initialValue: ComposerDraftState(cardID: cardID, remote: nil, fallbackGeneration: nil))
    }

    private var card: CardSummary? { model.snapshot?.cards.first(where: { $0.id == cardID }) }

    var body: some View {
        Group {
            if let card {
                List {
                    scratchpadSection(card)
                    composerSection(card)
                    outputSection(card)
                }
                .scrollDismissesKeyboard(.interactively)
                .navigationTitle(card.title)
                .navigationBarTitleDisplayMode(.inline)
                .refreshable { await refreshCurrentTaskDetails() }
                .task { await model.loadDetails(card: card) }
                .sheet(item: $editing) { entry in EditNoteView(text: entry.text) { text in await model.bufferEdit(card: card, entry: entry, text: text) } }
                .toolbar {
                    ToolbarItemGroup(placement: .keyboard) {
                        Spacer()
                        Button(String(localized: "taskDetail.keyboard.done")) { focusedField = nil }
                            .accessibilityIdentifier("deck.keyboard.done")
                    }
                }
            } else {
                ContentUnavailableView("Task unavailable", systemImage: "rectangle.slash", description: Text("It may have been removed on the desktop."))
            }
        }
        .onChange(of: model.operationReceipts) { _, receipts in
            if let pendingAdd, let result = receipts[pendingAdd.id] {
                switch result.state {
                case .applied:
                    let hasNewerDraft = newNote != pendingAdd.text
                    if !hasNewerDraft { newNote = "" }
                    noteSaveState = hasNewerDraft ? .savedWithNewerDraft : .saved
                    self.pendingAdd = nil
                case .rejected:
                    noteSaveState = .failed(result.code ?? String(localized: "taskDetail.note.rejected"))
                    self.pendingAdd = nil
                case .accepted, .ambiguous, .delivered:
                    break
                }
            }
            if let pendingQueue, let result = receipts[pendingQueue.id], [.applied, .delivered].contains(result.state) {
                if selection == pendingQueue.selection { selection.removeAll() }
                self.pendingQueue = nil
            }
        }
    }

    @ViewBuilder private func outputSection(_ card: CardSummary) -> some View {
        Section("Read-only terminal snapshot") {
            if let output = model.outputs[card.id] {
                TerminalOutputSnapshotView(output: output)
                if output.truncated { Label("Older output was truncated by the host.", systemImage: "scissors").font(.caption).foregroundStyle(.orange) }
                Text("This is a bounded terminal snapshot, not complete chat history.").font(.caption).foregroundStyle(.secondary)
            } else if let unavailable = model.outputUnavailable[card.id] {
                Label("Output unavailable: \(unavailable)", systemImage: "terminal.fill").foregroundStyle(.secondary)
                Text("Scratchpad notes remain available for stopped tasks.").font(.caption).foregroundStyle(.secondary)
            } else { ProgressView() }
        }
    }

    @ViewBuilder private func composerSection(_ card: CardSummary) -> some View {
        let generationChanged = composer.expectedGeneration != card.generation
        let pending = model.pendingSends[card.id]
        Section(String(localized: "taskDetail.message.title")) {
            TextEditor(text: Binding(get: { composer.text }, set: { value in composer.edit(value); model.stageDraft(composer.snapshot) }))
                .frame(minHeight: 90)
                .focused($focusedField, equals: .message)
                .disabled(model.sendingCards.contains(card.id))
                .onAppear { composer.merge(remote: model.drafts[card.id], fallbackGeneration: card.generation) }
                .onChange(of: model.drafts[card.id]) { _, remote in composer.merge(remote: remote, fallbackGeneration: card.generation) }
                .onChange(of: model.draftPersistence[card.id]) { _, status in
                    switch status {
                    case let .saved(version): composer.saved(version: version)
                    case let .failed(version, message): composer.failed(version: version, message: message)
                    case .saving, .none: break
                    }
                }
            Text(String(localized: "taskDetail.message.localDraft"))
                .font(.caption).foregroundStyle(.secondary)
            Text("Use the keyboard microphone for system dictation. Review and edit the text before sending.").font(.caption).foregroundStyle(.secondary)
            draftPersistenceStatus(card)
            if let error = draftPersistenceError(card) {
                Label(String(format: String(localized: "taskDetail.message.saveFailed"), error), systemImage: "exclamationmark.triangle").font(.caption).foregroundStyle(.orange)
                Button("Retry saving draft") { model.stageDraft(composer.snapshot) }
            }
            if generationChanged {
                Label("The desktop target changed after this draft was started.", systemImage: "exclamationmark.triangle").foregroundStyle(.orange)
                Button("Use current task generation") { composer.acceptGeneration(card.generation); model.stageDraft(composer.snapshot) }
            }
            if let pending {
                Label("Original send \(pending.localState.rawValue). Operation \(pending.id)", systemImage: "clock.arrow.circlepath")
                    .font(.caption).foregroundStyle(.orange).textSelection(.enabled)
                Button("Check original operation") { Task { await refreshCurrentTaskDetails() } }
                if pending.localState == .notFound {
                    Button("Retry original ID and body") { Task { await model.retryOriginal(pending) } }
                }
            }
            Button(String(localized: "taskDetail.message.send")) {
                let visible = composer.snapshot
                Task {
                    if await model.send(card: card, visible: visible) == .applied {
                        composer.clearIfUnchanged(sentVersion: visible.version, fallbackGeneration: card.generation)
                    }
                }
            }
                .buttonStyle(.borderedProminent)
                .disabled(!card.canSend || composer.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || generationChanged || pending != nil || model.sendingCards.contains(card.id))
            if !card.canSend {
                Label(sendUnavailableReason(card), systemImage: "exclamationmark.circle")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Text("Sending never starts a stopped shell and does not change desktop permissions.").font(.caption).foregroundStyle(.secondary)
        }
    }

    @ViewBuilder private func draftPersistenceStatus(_ card: CardSummary) -> some View {
        switch model.draftPersistence[card.id] {
        case let .saving(version) where version == composer.version:
            Label(String(localized: "taskDetail.message.saving"), systemImage: "arrow.triangle.2.circlepath")
                .font(.caption).foregroundStyle(.secondary)
        case let .saved(version) where version >= composer.version:
            Label(String(localized: "taskDetail.message.saved"), systemImage: "checkmark.circle")
                .font(.caption).foregroundStyle(.green)
        default:
            EmptyView()
        }
    }

    private func draftPersistenceError(_ card: CardSummary) -> String? {
        if let error = composer.persistenceError { return error }
        if case let .failed(version, message)? = model.draftPersistence[card.id], version == composer.version { return message }
        return nil
    }

    private func sendUnavailableReason(_ card: CardSummary) -> String {
        if card.status == "dead" || card.status == "stopped" {
            return String(localized: "taskDetail.message.unavailable.stopped")
        }
        if card.status == "unknown" {
            return String(localized: "taskDetail.message.unavailable.unknown")
        }
        return String(localized: "taskDetail.message.unavailable.agent")
    }

    private func refreshCurrentTaskDetails() async {
        await model.checkOriginalOperations()
        guard let latestCard = model.snapshot?.cards.first(where: { $0.id == cardID }) else { return }
        await model.loadDetails(card: latestCard)
    }

    @ViewBuilder private func scratchpadSection(_ card: CardSummary) -> some View {
        let pending = model.pendingCardCommands[card.id] ?? []
        Section {
            Text(String(localized: "taskDetail.note.explanation"))
                .font(.caption).foregroundStyle(.secondary)
            if !pending.isEmpty {
                Label("A scratchpad or queue operation is pending. Its original ID will be queried; controls remain locked to prevent duplicates.", systemImage: "clock.arrow.circlepath")
                    .font(.caption).foregroundStyle(.orange)
                Button("Check original operation") { Task { await refreshCurrentTaskDetails() } }
                    .accessibilityIdentifier("deck.note.check-original")
                ForEach(pending.filter { $0.localState == .notFound }) { record in
                    Button("Retry original operation \(record.id)") { Task { await model.retryOriginal(record) } }
                }
            }
            TextEditor(text: Binding(get: { newNote }, set: { value in
                newNote = value
                if pendingAdd == nil && !addingNote { noteSaveState = .idle }
            }))
                .frame(minHeight: 90)
                .focused($focusedField, equals: .note)
                .accessibilityIdentifier("deck.note.new")
                .overlay(alignment: .topLeading) {
                    if newNote.isEmpty {
                        Text(String(localized: "taskDetail.note.placeholder"))
                            .foregroundStyle(.tertiary).padding(.top, 8).padding(.leading, 5)
                            .allowsHitTesting(false)
                    }
                }
            Button(String(localized: "taskDetail.note.save")) {
                let text = newNote
                addingNote = true
                noteSaveState = .saving
                Task {
                    let outcome = await model.bufferAdd(card: card, text: text)
                    switch outcome {
                    case .applied:
                        let hasNewerDraft = newNote != text
                        if !hasNewerDraft { newNote = "" }
                        noteSaveState = hasNewerDraft ? .savedWithNewerDraft : .saved
                    case let .pending(id, _):
                        if let id { pendingAdd = (id, text) }
                        noteSaveState = .pending
                    case let .failed(message):
                        noteSaveState = .failed(message)
                    }
                    addingNote = false
                }
            }
                .accessibilityIdentifier("deck.note.save")
                .buttonStyle(.borderedProminent)
                .disabled(!pending.isEmpty || pendingAdd != nil || noteSaveState == .pending || model.busyCards.contains(card.id) || addingNote || newNote.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            noteSaveStatus
            if let buffer = model.buffers[card.id] {
                ForEach(buffer.entries) { entry in
                    HStack(alignment: .top) {
                        Toggle("", isOn: Binding(get: { selection.contains(entry.id) }, set: { selected in if selected { selection.insert(entry.id) } else { selection.remove(entry.id) } })).labelsHidden()
                            .disabled(queueingSelection || !pending.isEmpty || model.busyCards.contains(card.id))
                        VStack(alignment: .leading) {
                            Text(entry.text).accessibilityIdentifier("deck.note.text")
                            Text(entry.kind == "manual" ? "Manual note" : (entry.source?.type ?? "External event")).font(.caption).foregroundStyle(.secondary)
                            if let copy = entry.copies.last { Text("Queued copy: \(copy.state)").font(.caption2).foregroundStyle(.secondary) }
                        }
                        Spacer()
                        if entry.kind == "manual" {
                            Button { editing = entry } label: { Image(systemName: "pencil") }
                                .accessibilityIdentifier("deck.note.edit")
                                .disabled(!pending.isEmpty || model.busyCards.contains(card.id))
                        }
                    }
                    .accessibilityElement(children: .contain)
                    .accessibilityIdentifier("deck.note.row")
                    .swipeActions {
                        Button("Delete", role: .destructive) { Task { _ = await model.bufferDelete(card: card, entry: entry) } }
                            .accessibilityIdentifier("deck.note.delete")
                    }
                }
                Button("Queue selected (\(selection.count))") {
                    let submittedSelection = selection
                    queueingSelection = true
                    Task {
                        let outcome = await model.bufferQueue(card: card, entryIDs: Array(submittedSelection))
                        if outcome == .applied,
                           selection == submittedSelection { selection.removeAll() }
                        if case let .pending(id?, _) = outcome { pendingQueue = (id, submittedSelection) }
                        queueingSelection = false
                    }
                }
                .accessibilityIdentifier("deck.note.queue-selected")
                .disabled(!card.canQueue || queueingSelection || !pending.isEmpty || model.busyCards.contains(card.id) || selection.isEmpty)
                if !card.canQueue {
                    Text("Queueing requires a Codex or Claude launch configuration saved on the desktop.").font(.caption).foregroundStyle(.secondary)
                }
                Text("Queueing creates a copy; notes remain in the scratchpad and later edits do not alter that copy.").font(.caption).foregroundStyle(.secondary)
            } else { ProgressView() }
        } header: { Text(String(localized: "taskDetail.scratchpad.title")) }
    }

    @ViewBuilder private var noteSaveStatus: some View {
        switch noteSaveState {
        case .idle:
            EmptyView()
        case .saving:
            Label(String(localized: "taskDetail.note.saving"), systemImage: "arrow.triangle.2.circlepath")
                .font(.caption).foregroundStyle(.secondary)
        case .pending:
            Label(String(localized: "taskDetail.note.pending"), systemImage: "clock.arrow.circlepath")
                .font(.caption).foregroundStyle(.orange)
        case .saved:
            Label(String(localized: "taskDetail.note.saved"), systemImage: "checkmark.circle")
                .font(.caption).foregroundStyle(.green)
        case .savedWithNewerDraft:
            Label(String(localized: "taskDetail.note.savedNewer"), systemImage: "checkmark.circle")
                .font(.caption).foregroundStyle(.green)
        case let .failed(message):
            Label(String(format: String(localized: "taskDetail.note.failed"), message), systemImage: "exclamationmark.triangle")
                .font(.caption).foregroundStyle(.orange)
        }
    }
}

private struct TerminalOutputSnapshotView: View {
    private static let endID = "terminal-output-end"
    let output: TerminalOutput

    var body: some View {
        ScrollViewReader { proxy in
            VStack(alignment: .leading, spacing: 8) {
                Button {
                    withAnimation { proxy.scrollTo(Self.endID, anchor: .bottomLeading) }
                } label: {
                    Label(String(localized: "taskDetail.output.latest"), systemImage: "arrow.down.to.line")
                }
                .accessibilityIdentifier("deck.output.latest")
                .buttonStyle(.bordered)
                ScrollView([.horizontal, .vertical]) {
                    VStack(alignment: .leading, spacing: 0) {
                        Text(output.text.isEmpty ? "No output" : output.text)
                            .font(.system(.caption, design: .monospaced))
                            .fixedSize(horizontal: true, vertical: true)
                            .textSelection(.enabled)
                            .accessibilityIdentifier("deck.output.text")
                        Color.clear.frame(width: 1, height: 1).id(Self.endID)
                    }
                }
                .accessibilityIdentifier("deck.output.scroll")
                .frame(height: 260)
            }
            .task(id: output.revision.value) {
                await Task.yield()
                proxy.scrollTo(Self.endID, anchor: .bottomLeading)
            }
        }
    }
}

private struct EditNoteView: View {
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var model: AppModel
    @State var text: String
    @State private var saving = false
    @State private var error: String?
    @State private var pendingOperation: (id: String, text: String)?
    let save: (String) async -> AppModel.MutationOutcome
    var body: some View {
        NavigationStack {
            Form {
                TextEditor(text: $text)
                    .frame(minHeight: 180)
                    .accessibilityIdentifier("deck.note.edit.text")
            }
                .navigationTitle("Edit note")
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                    ToolbarItem(placement: .confirmationAction) {
                        Button("Save") {
                            let submittedText = text
                            saving = true
                            Task {
                                let outcome = await save(submittedText)
                                saving = false
                                if outcome == .applied, text == submittedText { dismiss() }
                                else if outcome == .applied { error = "The submitted version was saved. Your newer edits remain in this editor." }
                                else if case let .failed(message) = outcome { error = message }
                                else if case let .pending(id, state) = outcome {
                                    if let id { pendingOperation = (id, submittedText) }
                                    error = "Operation is \(state); the note remains open until Deck confirms it."
                                }
                            }
                        }
                        .accessibilityIdentifier("deck.note.edit.save")
                        .disabled(saving || pendingOperation != nil || text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    }
                }
                .alert("Note not confirmed", isPresented: Binding(get: { error != nil }, set: { if !$0 { error = nil } })) { Button("OK") { error = nil } } message: { Text(error ?? "") }
                .onChange(of: model.operationReceipts) { _, receipts in
                    guard let pendingOperation, let result = receipts[pendingOperation.id] else { return }
                    switch result.state {
                    case .applied, .delivered:
                        self.pendingOperation = nil
                        if text == pendingOperation.text { dismiss() }
                        else { error = "The submitted version was saved. Your newer edits remain in this editor." }
                    case .rejected:
                        self.pendingOperation = nil
                        error = result.code ?? "The edit was rejected."
                    case .accepted, .ambiguous: break
                    }
                }
        }
    }
}
