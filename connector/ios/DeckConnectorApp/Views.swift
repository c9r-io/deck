// Content is primary; notes are an explicit secondary view. Cached terminal
// output always carries capture time and freshness, never Agent task semantics.
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
                        .accessibilityIdentifier("deck.pair.scan")
                    TextField("Paste deck-connector:// pairing code", text: $descriptor, axis: .vertical)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .accessibilityIdentifier("deck.pair.descriptor")
                    Button("Pair") { Task { await model.pair(descriptor: descriptor); descriptor = "" } }
                        .disabled(descriptor.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                        .accessibilityIdentifier("deck.pair.submit")
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
    /// "<host id>|<scope>" the user chose to keep; a per-phone convenience only.
    @AppStorage("deck.sharing.kept") private var keptSharing = ""

    var body: some View {
        NavigationStack {
            List {
                connectionBanner
                if let snapshot = model.snapshot {
                    Section {
                        DisclosureGroup {
                            Text(snapshot.hostName ?? snapshot.hostId).font(.caption)
                            Text(sharingScopeMessage(snapshot.scope))
                                .font(.caption).foregroundStyle(.secondary)
                            Text(String(format: String(localized: "sharing.thisApp"), appVersionDescription))
                                .font(.caption).foregroundStyle(.secondary)
                                .accessibilityIdentifier("deck.sharing.appVersion")
                        } label: {
                            VStack(alignment: .leading) {
                                Label(snapshot.hostName ?? snapshot.hostId, systemImage: "desktopcomputer")
                                    .font(.subheadline).lineLimit(1)
                                if let date = model.lastSyncedAt {
                                    HStack { Text("Last synced"); Text(date, style: .time) }.font(.caption).foregroundStyle(.secondary)
                                }
                            }
                        }.accessibilityIdentifier("deck.sharing.details")
                    }
                    if SharingGuide.needsFullAccessSetup(snapshot) {
                        sharingSetupSection(snapshot)
                    }
                    if snapshot.cards.isEmpty {
                        ContentUnavailableView("No shared cards", systemImage: "rectangle.stack", description: Text("Check the sharing scope on your Mac. Refreshing does not change which cards are shared."))
                    }
                    ForEach(snapshot.projects) { project in
                        Section {
                            ForEach(snapshot.cards.filter { $0.projectId == project.id }) { card in
                                NavigationLink(value: card.id) {
                                    VStack(alignment: .leading) {
                                        Text(card.title)
                                        Text(project.columns.first(where: { $0.id == card.columnId })?.name ?? card.columnId)
                                            .font(.caption).foregroundStyle(.secondary)
                                        Text(cardStatus(card)).font(.caption).foregroundStyle(.secondary)
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
                TaskDetailView(cardID: id)
            }
            .refreshable { await model.refresh() }
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) { Menu { Button("Refresh") { Task { await model.refresh() } }; Button("Remove pairing", role: .destructive) { confirmUnpair = true } } label: { Image(systemName: "ellipsis.circle") } }
            }
        }
    }

    /// Setup guidance leads the list until the user chooses to keep the
    /// current sharing; it then stays one line for this host and scope and
    /// returns by itself when the Mac's choice changes.
    @ViewBuilder private func sharingSetupSection(_ snapshot: Snapshot) -> some View {
        let key = "\(snapshot.hostId)|\(snapshot.scope ?? "")"
        Section {
            if keptSharing == key {
                HStack {
                    Label(String(localized: "sharing.setup.collapsed"), systemImage: "lock")
                        .font(.footnote).foregroundStyle(.secondary)
                        .accessibilityIdentifier("deck.sharing.collapsed")
                    Spacer()
                    Button(String(localized: "sharing.setup.show")) { keptSharing = "" }
                        .font(.footnote).accessibilityIdentifier("deck.sharing.show")
                }
            } else {
                Label(String(localized: "sharing.setup.title"), systemImage: "lock.rectangle.on.rectangle")
                    .font(.subheadline.weight(.semibold))
                // One short paragraph so the cards stay on the first screen of a small phone.
                Text(String(format: String(localized: "sharing.setup.body"), snapshot.hostName ?? snapshot.hostId))
                    .font(.footnote)
                    .accessibilityIdentifier("deck.sharing.setup")
                Button(String(localized: "sharing.setup.keep")) { keptSharing = key }
                    .font(.footnote)
                    .accessibilityIdentifier("deck.sharing.keep")
            }
        }
    }

    private func cardStatus(_ card: CardSummary) -> String {
        if let reason = card.capabilities?.readOutput?.reason {
            if SharingGuide.remedy(reason: reason, kind: .readOutput, scope: model.snapshot?.scope) == .chooseFullAccess {
                return String(localized: "sharing.row.notShared")
            }
            return capabilityMessage(reason, kind: .readOutput, snapshot: model.snapshot)
        }
        if card.isStopped { return String(localized: "Session stopped") }
        return card.status == "running" ? String(localized: "Session available") : String(localized: "Session status unavailable")
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
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize
    @Environment(\.verticalSizeClass) private var verticalSizeClass
    @ScaledMetric(relativeTo: .body) private var composerHeight = 62.0
    @State private var newNote = ""
    @State private var showingNotes = false
    @State private var addingNote = false
    @State private var noteSaveState = NoteSaveState.idle
    @State private var selection = Set<String>()
    @State private var queueingSelection = false
    @State private var pendingSend: (id: String, version: UInt64)?
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
    private var inlineComposer: Bool { dynamicTypeSize.isAccessibilitySize || verticalSizeClass == .compact }

    var body: some View {
        Group {
            if let card {
                List {
                    Section {
                        if case .offline = model.connection { Label("Offline — showing the last available snapshot", systemImage: "wifi.slash").font(.caption).foregroundStyle(.orange) }
                        Picker("Card view", selection: $showingNotes) {
                            Text("Output").tag(false)
                            Text("Notes").tag(true)
                        }.pickerStyle(.segmented).accessibilityIdentifier("deck.detail.tabs")
                        if let project = model.snapshot?.projects.first(where: { $0.id == card.projectId }) {
                            Text("\(project.name) · \(project.columns.first(where: { $0.id == card.columnId })?.name ?? card.columnId)")
                                .font(.caption).foregroundStyle(.secondary)
                        }
                    }
                    if showingNotes { scratchpadSection(card) }
                    else {
                        outputSection(card)
                        if inlineComposer { Section { composerSection(card) } }
                    }
                }
                .scrollDismissesKeyboard(.interactively)
                .navigationTitle(card.title)
                .navigationBarTitleDisplayMode(.inline)
                .refreshable { await refreshCurrentTaskDetails() }
                .task(id: cardID) { model.showDetails(cardID); await model.loadDetails(card: card) }
                .onDisappear { model.showDetails(nil) }
                .safeAreaInset(edge: .bottom) {
                    if !showingNotes && !inlineComposer { composerSection(card).padding().background(.bar) }
                }
                .sheet(item: $editing) { entry in EditNoteView(cardID: cardID, entryID: entry.id, text: entry.text) { text in await model.bufferEdit(card: card, entry: entry, text: text) } }
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
        .onChange(of: model.operationReceipts) { _, receipts in reconcileReceipts(receipts) }
        .onChange(of: newNote) { _, value in model.traceNote("view-text-changed length=\(value.count)") }
        .onChange(of: addingNote) { _, value in model.traceNote("view-adding-changed value=\(value)") }
        .onChange(of: pendingAdd?.id) { _, _ in reconcileReceipts(model.operationReceipts) }
    }

    private func reconcileReceipts(_ receipts: [String: CommandResult]) {
        if let pendingSend, let receipt = receipts[pendingSend.id], [.applied, .delivered].contains(receipt.state) {
            composer.clearIfUnchanged(sentVersion: pendingSend.version, fallbackGeneration: card?.generation)
            self.pendingSend = nil
        }
        if let pendingAdd, let result = receipts[pendingAdd.id] {
            switch result.state {
            case .applied:
                let hasNewerDraft = model.noteDrafts[NoteDraft.storageKey(cardID: cardID, entryID: nil)] != nil
                if !hasNewerDraft { newNote = "" }
                noteSaveState = hasNewerDraft ? .savedWithNewerDraft : .saved
                self.pendingAdd = nil
            case .rejected:
                noteSaveState = .failed(model.commandFailureMessage(result.code))
                self.pendingAdd = nil
            case .accepted, .ambiguous, .delivered:
                break
            }
        }
        if let pendingQueue, let result = receipts[pendingQueue.id] {
            switch result.state {
            case .applied, .delivered:
                if selection == pendingQueue.selection { selection.removeAll() }
                self.pendingQueue = nil
            case .rejected:
                model.message = model.commandFailureMessage(result.code)
                self.pendingQueue = nil
            case .accepted, .ambiguous:
                break
            }
        }
    }

    @ViewBuilder private func outputSection(_ card: CardSummary) -> some View {
        Section("Current output") {
            HStack {
                Button("Refresh output") { Task { await refreshCurrentTaskDetails() } }
                    .accessibilityIdentifier("deck.output.refresh")
                    .disabled(model.loadingDetails.contains(card.id))
                Spacer()
                if model.loadingDetails.contains(card.id) { ProgressView().accessibilityLabel("Updating output") }
            }
            if let output = model.outputs[card.id] {
                if let date = output.capturedAt.date {
                    HStack { Text("Captured"); Text(date.formatted(.dateTime.hour().minute().second())) }
                        .font(.caption).foregroundStyle(.secondary).accessibilityElement(children: .combine).accessibilityIdentifier("deck.output.captured")
                }
                if model.staleOutputs.contains(card.id) {
                    Label("Snapshot may be out of date", systemImage: "clock.badge.exclamationmark").font(.caption).foregroundStyle(.orange)
                }
                TerminalOutputSnapshotView(output: output)
                if output.truncated { Label("Older output was truncated by the host.", systemImage: "scissors").font(.caption).foregroundStyle(.orange) }
                Text("This is a bounded terminal snapshot, not complete chat history.").font(.caption).foregroundStyle(.secondary)
            } else if let unavailable = model.outputUnavailable[card.id] {
                Label("Output unavailable: \(outputIssueMessage(unavailable))", systemImage: "terminal.fill")
                    .foregroundStyle(.secondary)
                    .accessibilityIdentifier("deck.output.unavailable")
                if card.canEditNotes { Text("Notes remain available when output cannot be read.").font(.caption).foregroundStyle(.secondary) }
            } else { ProgressView() }
        }
    }

    private func outputIssueMessage(_ issue: AppModel.OutputIssue) -> String {
        switch issue {
        case .sessionUnavailable: String(localized: "taskDetail.output.sessionUnavailable")
        case .agentNotInForeground: String(localized: "taskDetail.output.agentNotInForeground")
        case .readFailed: String(localized: "taskDetail.output.readFailed")
        case .targetChanged: String(localized: "taskDetail.output.targetChanged")
        case .unsupportedTarget: capabilityMessage("unsupported-target", kind: .readOutput, snapshot: model.snapshot)
        case .controlled: capabilityMessage("mcp-controlled", kind: .readOutput, snapshot: model.snapshot)
        case .unknown: String(localized: "taskDetail.output.unknown")
        }
    }

    @ViewBuilder private func composerSection(_ card: CardSummary) -> some View {
        let generationChanged = composer.expectedGeneration != card.generation
        let pending = model.pendingSends[card.id]
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .bottom) {
                TextEditor(text: Binding(get: { composer.text }, set: { value in composer.edit(value, currentGeneration: card.generation); model.stageDraft(composer.snapshot) }))
                    .frame(height: composerHeight)
                    .overlay(alignment: .topLeading) {
                        if composer.text.isEmpty { Text("Message the agent…").foregroundStyle(.secondary).padding(8).allowsHitTesting(false) }
                    }
                    .accessibilityIdentifier("deck.message.draft")
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
                Button(String(localized: "taskDetail.message.send")) {
                    let visible = composer.snapshot
                    Task {
                        let outcome = await model.send(card: card, visible: visible)
                        if outcome == .applied {
                            composer.clearIfUnchanged(sentVersion: visible.version, fallbackGeneration: card.generation)
                        } else {
                            let id: String?
                            if case let .pending(operationID, _) = outcome { id = operationID }
                            else { id = model.pendingSends[card.id]?.id }
                            if let id, let result = model.operationReceipts[id], [.applied, .delivered].contains(result.state) {
                                composer.clearIfUnchanged(sentVersion: visible.version, fallbackGeneration: card.generation)
                            } else if let id { pendingSend = (id, visible.version) }
                        }
                    }
                }
                .buttonStyle(.borderedProminent)
                .accessibilityIdentifier("deck.message.send")
                .disabled(!card.maySend || composer.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || generationChanged || pending != nil || model.sendingCards.contains(card.id))
            }
            if let pending {
                OperationStatusView(record: pending)
            } else if generationChanged && !composer.text.isEmpty {
                Text(composer.expectedGeneration == nil
                     ? String(localized: "taskDetail.message.unconfirmedTarget")
                     : String(localized: "The target changed. Review your draft before sending."))
                    .font(.caption).foregroundStyle(.orange)
                    .accessibilityIdentifier("deck.message.targetChanged")
                Button("Use current target") { composer.acceptGeneration(card.generation); model.stageDraft(composer.snapshot) }
            } else if !card.maySend {
                Text(sendUnavailableReason(card)).font(.caption).foregroundStyle(.secondary)
            } else { draftPersistenceStatus(card) }
            if let error = draftPersistenceError(card) {
                Text(error).font(.caption).foregroundStyle(.orange)
                Button("Retry saving draft") { model.stageDraft(composer.snapshot) }
            }
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
        if let reason = card.capabilities?.sendMessage?.reason { return capabilityMessage(reason, kind: .sendMessage, snapshot: model.snapshot) }
        if card.isStopped {
            return String(localized: "taskDetail.message.unavailable.stopped")
        }
        if card.status == "unknown" {
            return String(localized: "taskDetail.message.unavailable.unknown")
        }
        return String(localized: "taskDetail.message.unavailable.agent")
    }

    private func refreshCurrentTaskDetails() async {
        await model.refresh()
        // refresh also refreshes the currently visible card; no second read chain.
    }

    @ViewBuilder private func scratchpadSection(_ card: CardSummary) -> some View {
        let pending = model.pendingCardCommands[card.id] ?? []
        Section {
            Text("Unsaved edits stay on this iPhone until you save the note.").font(.caption).foregroundStyle(.secondary)
            Text(String(localized: "taskDetail.note.explanation"))
                .font(.caption).foregroundStyle(.secondary)
            if !pending.isEmpty {
                ForEach(pending) { record in OperationStatusView(record: record) }
            }
            TextEditor(text: Binding(get: { newNote }, set: { value in
                newNote = value
                model.stageNoteDraft(cardID: cardID, text: value)
                if pendingAdd == nil && !addingNote { noteSaveState = .idle }
            }))
                .frame(minHeight: 90)
                .focused($focusedField, equals: .note)
                .accessibilityIdentifier("deck.note.new")
                .task { if let draft = await model.loadNoteDraft(cardID: cardID), newNote.isEmpty { newNote = draft.text } }
                .onChange(of: model.noteDrafts[NoteDraft.storageKey(cardID: cardID, entryID: nil)]) { old, new in
                    if new == nil, old?.text == newNote { newNote = ""; noteSaveState = .saved }
                }
                .overlay(alignment: .topLeading) {
                    if newNote.isEmpty {
                        Text(String(localized: "taskDetail.note.placeholder"))
                            .foregroundStyle(.secondary).padding(.top, 8).padding(.leading, 5)
                            .allowsHitTesting(false)
                    }
                }
            Button(String(localized: "taskDetail.note.save")) {
                let text = newNote
                addingNote = true
                noteSaveState = .saving
                Task {
                    model.traceNote("view-submit-start")
                    let outcome = await model.bufferAdd(card: card, text: text)
                    model.traceNote("view-submit-return applied=\(outcome == .applied) length=\(newNote.count)")
                    switch outcome {
                    case .applied:
                        let hasNewerDraft = model.noteDrafts[NoteDraft.storageKey(cardID: cardID, entryID: nil)] != nil
                        if !hasNewerDraft { newNote = "" }
                        noteSaveState = hasNewerDraft ? .savedWithNewerDraft : .saved
                    case let .pending(id, _):
                        if let id { pendingAdd = (id, text) }
                        noteSaveState = .pending
                    case let .failed(message):
                        noteSaveState = .failed(message)
                    }
                    addingNote = false
                    model.traceNote("view-settled length=\(newNote.count) saving=\(noteSaveState == .saving)")
                }
            }
                .accessibilityIdentifier("deck.note.save")
                .buttonStyle(.borderedProminent)
                .disabled(!card.canEditNotes || !pending.isEmpty || pendingAdd != nil || noteSaveState == .pending || model.busyCards.contains(card.id) || addingNote || newNote.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            noteSaveStatus
            if let error = model.noteDraftErrors[NoteDraft.storageKey(cardID: cardID, entryID: nil)] {
                Text(error).font(.caption).foregroundStyle(.orange)
                Button("Retry saving draft") { model.stageNoteDraft(cardID: cardID, text: newNote) }
            }
            if let error = model.bufferErrors[card.id] {
                Label(error, systemImage: "exclamationmark.circle").font(.caption).foregroundStyle(.orange)
                Button("Refresh notes") { Task { await refreshCurrentTaskDetails() } }
            }
            if let buffer = model.buffers[card.id] {
                ForEach(buffer.entries) { entry in
                    HStack(alignment: .top) {
                        Toggle("", isOn: Binding(get: { selection.contains(entry.id) }, set: { selected in if selected { selection.insert(entry.id) } else { selection.remove(entry.id) } })).labelsHidden()
                            .disabled(queueingSelection || !pending.isEmpty || model.busyCards.contains(card.id))
                        VStack(alignment: .leading) {
                            Text(entry.text).accessibilityIdentifier("deck.note.text")
                            Text(entry.kind == "manual" ? String(localized: "taskDetail.note.manual") : (entry.source?.type ?? String(localized: "taskDetail.note.external"))).font(.caption).foregroundStyle(.secondary)
                            if let copy = entry.copies.last { Text(String(format: String(localized: "taskDetail.queue.copy"), copy.state)).font(.caption2).foregroundStyle(.secondary) }
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
                        Button(String(localized: "taskDetail.note.delete"), role: .destructive) { Task { _ = await model.bufferDelete(card: card, entry: entry) } }
                            .accessibilityIdentifier("deck.note.delete")
                    }
                }
                Button(String(format: String(localized: "taskDetail.queue.selected"), selection.count)) {
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
                .disabled(!card.mayQueue || queueingSelection || !pending.isEmpty || model.busyCards.contains(card.id) || selection.isEmpty)
                if !card.mayQueue {
                    Text(String(localized: "taskDetail.queue.unavailable")).font(.caption).foregroundStyle(.secondary)
                }
                Text(String(localized: "taskDetail.queue.explanation")).font(.caption).foregroundStyle(.secondary)
            } else if !card.canEditNotes {
                Label(capabilityMessage(card.capabilities?.editNotes?.reason ?? "unsupported-target", kind: .editNotes, snapshot: model.snapshot), systemImage: "exclamationmark.circle")
                    .font(.caption).foregroundStyle(.secondary)
            } else if model.bufferErrors[card.id] == nil { ProgressView() }
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
    @State private var wrapLines = true
    @State private var newOutput = false

    var body: some View {
        ScrollViewReader { proxy in
            VStack(alignment: .leading, spacing: 8) {
                Button {
                    withAnimation { proxy.scrollTo(Self.endID, anchor: .bottomLeading) }; newOutput = false
                } label: {
                    Label(String(localized: "taskDetail.output.latest"), systemImage: "arrow.down.to.line")
                }
                .accessibilityIdentifier("deck.output.latest")
                .buttonStyle(.bordered)
                Toggle("Wrap lines", isOn: $wrapLines).font(.caption)
                if newOutput { Text("New output available below").font(.caption).foregroundStyle(.secondary) }
                ScrollView(wrapLines ? [.vertical] : [.horizontal, .vertical]) {
                    VStack(alignment: .leading, spacing: 0) {
                        Text(output.text.isEmpty ? String(localized: "No output") : output.text)
                            .font(.system(.caption, design: .monospaced))
                            .fixedSize(horizontal: !wrapLines, vertical: true)
                            .textSelection(.enabled)
                            .accessibilityIdentifier("deck.output.text")
                        Color.clear.frame(width: 1, height: 1).id(Self.endID)
                    }
                }
                .accessibilityIdentifier("deck.output.scroll")
                .frame(height: 260)
            }
            .task { proxy.scrollTo(Self.endID, anchor: .bottomLeading) }
            .onChange(of: output.revision.value) { _, _ in newOutput = true }
        }
    }
}

private struct EditNoteView: View {
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var model: AppModel
    let cardID: String
    let entryID: String
    @State var text: String
    @State private var loadedDraft = false
    @State private var saving = false
    @State private var error: String?
    @State private var pendingOperation: (id: String, text: String)?
    let save: (String) async -> AppModel.MutationOutcome
    var body: some View {
        NavigationStack {
            Form {
                Text("Unsaved edits stay on this iPhone until you save the note.").font(.caption).foregroundStyle(.secondary)
                TextEditor(text: Binding(get: { text }, set: { value in text = value; model.stageNoteDraft(cardID: cardID, entryID: entryID, text: value) }))
                    .task {
                        guard !loadedDraft else { return }
                        let original = text
                        if let draft = await model.loadNoteDraft(cardID: cardID, entryID: entryID), text == original { text = draft.text }
                        loadedDraft = true
                    }
                    .frame(minHeight: 180)
                    .accessibilityIdentifier("deck.note.edit.text")
                    .onChange(of: model.noteDrafts[NoteDraft.storageKey(cardID: cardID, entryID: entryID)]) { old, new in
                        if new == nil, old?.text == text { dismiss() }
                    }
                if let error = model.noteDraftErrors[NoteDraft.storageKey(cardID: cardID, entryID: entryID)] {
                    Text(error).foregroundStyle(.orange)
                    Button("Retry saving draft") { model.stageNoteDraft(cardID: cardID, entryID: entryID, text: text) }
                }
                if pendingOperation != nil {
                    Section {
                        Label("Confirming the saved edit…", systemImage: "clock.arrow.circlepath")
                            .font(.caption)
                            .foregroundStyle(.orange)
                        Button("Check delivery") {
                            Task { await model.checkOriginalOperations() }
                        }
                        .accessibilityIdentifier("deck.note.edit.check-original")
                    }
                }
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
                                if outcome == .applied, model.noteDrafts[NoteDraft.storageKey(cardID: cardID, entryID: entryID)] == nil { dismiss() }
                                else if outcome == .applied { error = String(localized: "The submitted version was saved. Your newer edits remain in this editor.") }
                                else if case let .failed(message) = outcome { error = message }
                                else if case let .pending(id, _) = outcome {
                                    if let id { pendingOperation = (id, submittedText) }
                                    error = String(localized: "Confirming the saved edit…")
                                }
                            }
                        }
                        .accessibilityIdentifier("deck.note.edit.save")
                        .disabled(saving || pendingOperation != nil || text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    }
                }
                .alert("Note not confirmed", isPresented: Binding(get: { error != nil }, set: { if !$0 { error = nil } })) { Button("OK") { error = nil } } message: { Text(error ?? "") }
                .onChange(of: model.operationReceipts) { _, receipts in reconcileReceipt(receipts) }
                .onChange(of: pendingOperation?.id) { _, _ in reconcileReceipt(model.operationReceipts) }
        }
    }

    private func reconcileReceipt(_ receipts: [String: CommandResult]) {
        guard let pendingOperation, let result = receipts[pendingOperation.id] else { return }
        switch result.state {
        case .applied, .delivered:
            self.pendingOperation = nil
            if model.noteDrafts[NoteDraft.storageKey(cardID: cardID, entryID: entryID)] == nil { dismiss() }
            else { error = String(localized: "The submitted version was saved. Your newer edits remain in this editor.") }
        case .rejected:
            self.pendingOperation = nil
            error = model.commandFailureMessage(result.code)
        case .accepted, .ambiguous: break
        }
    }

}

/// One explanation per denied capability. A sharing denial names the Mac and
/// the exact desktop setting; a queue denial names the launch configuration.
func capabilityMessage(_ reason: String, kind: CardCapabilityKind, snapshot: Snapshot?) -> String {
    switch SharingGuide.remedy(reason: reason, kind: kind, scope: snapshot?.scope) {
    case .chooseFullAccess:
        return String(format: String(localized: "sharing.card.chooseFullAccess"), snapshot?.hostName ?? snapshot?.hostId ?? "Mac")
    case .savedLaunchCommand:
        return String(localized: "taskDetail.queue.unavailable")
    case .generic, nil:
        return legacyCapabilityMessage(reason)
    }
}

let appVersionDescription: String = {
    let info = Bundle.main.infoDictionary
    let version = info?["CFBundleShortVersionString"] as? String ?? "?"
    let build = info?["CFBundleVersion"] as? String ?? "?"
    return "\(version) (\(build))"
}()

private func legacyCapabilityMessage(_ reason: String) -> String {
    switch reason {
    case "unsupported-target": String(localized: "This access is not shared for this card. Check Phone Connector sharing on your Mac.")
    case "mcp-controlled": String(localized: "This card is controlled by another client. Phone access is unavailable.")
    case "session-unavailable": String(localized: "taskDetail.message.unavailable.stopped")
    case "agent-not-in-foreground": String(localized: "taskDetail.message.unavailable.agent")
    default: String(localized: "taskDetail.message.unavailable.unknown")
    }
}

private func sharingScopeMessage(_ scope: String?) -> String {
    switch scope {
    case "boardAccess":
        String(localized: "All card output and notes are shared by this Mac. Messages still require a current Codex or Claude foreground; shell input is unavailable, and queued delivery requires a saved launch configuration.")
    case "boardMetadata":
        String(localized: "All card summaries are shared by this Mac. Output and notes require the separate Phone Connector sharing choice on your Mac.")
    default:
        String(localized: "Only cards with a saved Codex or Claude launch command are shared. Change Phone Connector sharing on your Mac to include other cards.")
    }
}

private struct OperationStatusView: View {
    @EnvironmentObject private var model: AppModel
    let record: CommandRecord
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(status).font(.caption).foregroundStyle(.orange)
            if record.localState == .notFound {
                Button("Retry delivery") { Task { await model.retryOriginal(record) } }
            } else if ![.ambiguous, .expired].contains(record.localState) {
                Button("Check delivery") { Task { await model.checkOriginalOperations() } }
            }
            DisclosureGroup("Diagnostic details") {
                Text(record.id).textSelection(.enabled)
                Text(record.localState.rawValue)
                if let code = record.result?.code { Text(code).textSelection(.enabled) }
            }.font(.caption)
        }
    }
    private var status: String {
        switch record.localState {
        case .notFound: String(localized: "Not delivered. You can retry.")
        case .ambiguous, .expired: String(localized: "Delivery cannot be confirmed. Check this card on your Mac before sending again.")
        default: String(localized: "Confirming delivery…")
        }
    }
}
