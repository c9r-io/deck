// Content is primary: a card's detail is its output, filling the screen
// between the navigation bar and a one-line composer; notes open as a panel
// from the navigation bar. Cached terminal output always carries capture time
// and freshness, never Agent task semantics. The output view follows new
// output only while it is scrolled to the end; scrolled up, it stays put and
// offers a jump back.
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
                    if let summary = snapshot.mentions {
                        Section {
                            NavigationLink(value: MentionsDestination.list) {
                                HStack {
                                    Label(String(localized: "mentions.title"), systemImage: "at")
                                    Spacer()
                                    if summary.state == "ok", let count = summary.count, count > 0 {
                                        Text(count.formatted()).foregroundStyle(.secondary)
                                            .accessibilityIdentifier("deck.mentions.count")
                                    }
                                }
                            }.accessibilityIdentifier("deck.mentions.entry")
                        }
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
            .navigationDestination(for: MentionsDestination.self) { destination in
                switch destination {
                case .list: MentionsView()
                case let .mention(key): MentionDetailView(key: key)
                }
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

    private enum FocusedField: Hashable { case message }

    @EnvironmentObject private var model: AppModel
    let cardID: String
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize
    @Environment(\.verticalSizeClass) private var verticalSizeClass
    /// One line of the message draft; the editor grows from one to
    /// `draftMaxLines` lines with its text.
    @ScaledMetric(relativeTo: .body) private var draftLineHeight = 22.0
    @State private var draftTextHeight: CGFloat = 0
    /// Per-phone reading preferences.
    @AppStorage("deck.output.wrap") private var wrapLines = true
    @AppStorage("deck.output.textStep") private var textStep = 0
    /// The page last chosen by hand, for every card; empty until the first
    /// choice, which then means Replies while the Mac keeps them.
    @AppStorage("deck.detail.page") private var page = ""
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
    private func offersReplies(_ card: CardSummary) -> Bool { model.snapshot?.offersReplies(card) == true }
    /// The page shown: Output where a card has no replies; else the page
    /// chosen by hand, or before any choice Replies while the Mac's switch is
    /// on (with it off, the output is what there is to read).
    private func shownPage(_ card: CardSummary) -> DetailPage {
        guard offersReplies(card) else { return .output }
        if let chosen = DetailPage(rawValue: page) { return chosen }
        return model.snapshot?.replies == "on" ? .replies : .output
    }
    private var draftMaxLines: CGFloat { dynamicTypeSize.isAccessibilitySize || verticalSizeClass == .compact ? 2 : 5 }
    private var draftEditorHeight: CGFloat {
        let inset: CGFloat = 16
        return min(max(draftTextHeight + inset, draftLineHeight + inset), draftLineHeight * draftMaxLines + inset)
    }

    var body: some View {
        Group {
            if let card {
                VStack(spacing: 0) {
                    if offersReplies(card) { pagePicker(card) }
                    statusStrip(card)
                    Divider()
                    switch shownPage(card) {
                    case .output: outputArea(card)
                    case .replies: repliesArea(card)
                    }
                }
                .navigationTitle(card.title)
                .navigationBarTitleDisplayMode(.inline)
                .task(id: cardID) { model.showDetails(cardID); await model.loadDetails(card: card) }
                .onDisappear { model.showDetails(nil) }
                .safeAreaInset(edge: .bottom) {
                    composerSection(card)
                        .padding(.horizontal)
                        .padding(.vertical, verticalSizeClass == .compact ? 4 : 8)
                        // in a short landscape window the largest sizes would
                        // leave no room for the output
                        .dynamicTypeSize(...(verticalSizeClass == .compact ? DynamicTypeSize.xxLarge : DynamicTypeSize.accessibility5))
                        .background(.bar)
                }
                .sheet(isPresented: $showingNotes) {
                    NotesSheet { focus in
                        List { scratchpadSection(card, focus: focus) }
                            .scrollDismissesKeyboard(.interactively)
                            .refreshable { await refreshCurrentTaskDetails() }
                            .sheet(item: $editing) { entry in EditNoteView(cardID: cardID, entryID: entry.id, text: entry.text) { text in await model.bufferEdit(card: card, entry: entry, text: text) } }
                    }
                    .environmentObject(model)
                }
                .toolbar {
                    ToolbarItem(placement: .principal) { titleView(card) }
                    ToolbarItemGroup(placement: .topBarTrailing) {
                        Button { showingNotes = true } label: { notesLabel(card) }
                            .accessibilityLabel(Text("Notes"))
                            .accessibilityValue(card.buffer.entryCount > 0 ? Text(verbatim: "\(card.buffer.entryCount)") : Text(verbatim: ""))
                            .accessibilityIdentifier("deck.detail.notes")
                        readingMenu
                    }
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

    /// The card title over its project and column, capped so the largest
    /// text sizes do not push the navigation buttons away.
    @ViewBuilder private func titleView(_ card: CardSummary) -> some View {
        VStack(spacing: 0) {
            Text(card.title).font(.headline).lineLimit(1)
            if let project = model.snapshot?.projects.first(where: { $0.id == card.projectId }) {
                Text("\(project.name) · \(project.columns.first(where: { $0.id == card.columnId })?.name ?? card.columnId)")
                    .font(.caption2).foregroundStyle(.secondary).lineLimit(1)
            }
        }
        .dynamicTypeSize(...DynamicTypeSize.xxLarge)
        .accessibilityElement(children: .combine)
    }

    @ViewBuilder private func notesLabel(_ card: CardSummary) -> some View {
        Image(systemName: "note.text")
            .overlay(alignment: .topTrailing) {
                if card.buffer.entryCount > 0 {
                    Text(verbatim: "\(min(card.buffer.entryCount, 99))")
                        .font(.caption2.monospacedDigit().weight(.semibold))
                        .foregroundStyle(.white)
                        .padding(.horizontal, 4)
                        .background(Capsule().fill(Color.accentColor))
                        .offset(x: 9, y: -7)
                        .dynamicTypeSize(...DynamicTypeSize.large)
                }
            }
    }

    private var readingMenu: some View {
        Menu {
            Toggle(isOn: $wrapLines) { Label("Wrap lines", systemImage: "text.alignleft") }
                .accessibilityIdentifier("deck.output.wrap")
            Section(String(localized: "taskDetail.output.textSize")) {
                Button { textStep = max(TerminalOutputView.textSteps.lowerBound, textStep - 1) } label: {
                    Label(String(localized: "taskDetail.output.smaller"), systemImage: "textformat.size.smaller")
                }
                .disabled(textStep <= TerminalOutputView.textSteps.lowerBound)
                Button { textStep = min(TerminalOutputView.textSteps.upperBound, textStep + 1) } label: {
                    Label(String(localized: "taskDetail.output.larger"), systemImage: "textformat.size.larger")
                }
                .disabled(textStep >= TerminalOutputView.textSteps.upperBound)
            }
        } label: {
            Label(String(localized: "taskDetail.output.reading"), systemImage: "ellipsis.circle")
        }
        .accessibilityIdentifier("deck.detail.more")
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
                noteSaveState = .failed(model.commandFailureMessage(result.code, commandKind: "buffer-add"))
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
                model.message = model.commandFailureMessage(result.code, commandKind: "buffer-queue")
                self.pendingQueue = nil
            case .accepted, .ambiguous:
                break
            }
        }
    }

    @ViewBuilder private func pagePicker(_ card: CardSummary) -> some View {
        Picker(String(localized: "taskDetail.page.label"), selection: Binding(get: { shownPage(card).rawValue }, set: { page = $0 })) {
            Text(String(localized: "taskDetail.page.output")).tag(DetailPage.output.rawValue)
            Group {
                if let count = card.replyCount, count > 0 {
                    Text(verbatim: "\(String(localized: "taskDetail.page.replies")) · \(count)")
                } else {
                    Text(String(localized: "taskDetail.page.replies"))
                }
            }
            .tag(DetailPage.replies.rawValue)
        }
        .pickerStyle(.segmented)
        .padding(.horizontal)
        .padding(.top, 6)
        .padding(.bottom, 2)
        .background(.bar)
        .dynamicTypeSize(...DynamicTypeSize.accessibility1)
        .accessibilityIdentifier("deck.detail.page")
    }

    @ViewBuilder private func repliesArea(_ card: CardSummary) -> some View {
        if let value = model.replies[card.id] {
            if value.replies.isEmpty {
                repliesNotice(String(localized: "taskDetail.replies.emptyTitle"), String(localized: "taskDetail.replies.emptyBody"), systemImage: "bubble.left")
            } else {
                RepliesView(items: ReplyTimeline.merge(replies: value.replies, messages: model.sentMessages[card.id] ?? []),
                            textStep: textStep) { await refreshCurrentTaskDetails() }
                    .equatable()
            }
        } else if let issue = model.repliesIssue[card.id] {
            switch issue {
            case .off:
                repliesNotice(String(localized: "taskDetail.replies.offTitle"), String(localized: "taskDetail.replies.offBody"), systemImage: "bubble.left.and.exclamationmark.bubble.right")
            case .unavailable:
                repliesNotice(String(localized: "taskDetail.replies.unavailable"), nil, systemImage: "exclamationmark.bubble")
            }
        } else {
            ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    private func repliesNotice(_ title: String, _ body: String?, systemImage: String) -> some View {
        ScrollView {
            VStack(spacing: 10) {
                Image(systemName: systemImage).font(.title2).foregroundStyle(.secondary)
                Text(title).font(.headline).multilineTextAlignment(.center)
                if let body { Text(body).font(.callout).foregroundStyle(.secondary).multilineTextAlignment(.center) }
            }
            .padding()
            .padding(.top, 32)
            .frame(maxWidth: .infinity)
            .accessibilityElement(children: .combine)
            .accessibilityIdentifier("deck.replies.notice")
        }
        .refreshable { await refreshCurrentTaskDetails() }
    }

    /// One line of freshness: capture time, staleness or the offline state,
    /// and the refresh control. Every 5 s the open card is read again anyway.
    @ViewBuilder private func statusStrip(_ card: CardSummary) -> some View {
        HStack(spacing: 8) {
            VStack(alignment: .leading, spacing: 2) {
                if case .offline = model.connection {
                    Label("Offline — showing the last available snapshot", systemImage: "wifi.slash").foregroundStyle(.orange)
                }
                if shownPage(card) == .replies {
                    if let newest = model.replies[card.id]?.replies.last {
                        HStack(spacing: 4) { Text(String(localized: "taskDetail.replies.newest")); Text(replyTime(newest.date)) }
                            .accessibilityElement(children: .combine).accessibilityIdentifier("deck.replies.newest")
                    }
                } else if let output = model.outputs[card.id] {
                    HStack(spacing: 8) {
                        if let date = output.capturedAt.date {
                            HStack(spacing: 4) { Text("Captured"); Text(date.formatted(.dateTime.hour().minute().second())) }
                                .accessibilityElement(children: .combine).accessibilityIdentifier("deck.output.captured")
                        }
                        if model.staleOutputs.contains(card.id) {
                            Label("Snapshot may be out of date", systemImage: "clock.badge.exclamationmark").foregroundStyle(.orange)
                        }
                    }
                }
            }
            Spacer(minLength: 0)
            if model.loadingDetails.contains(card.id) { ProgressView().controlSize(.small).accessibilityLabel("Updating output") }
            Button { Task { await refreshCurrentTaskDetails() } } label: {
                Label("Refresh output", systemImage: "arrow.clockwise").labelStyle(.iconOnly)
            }
            .accessibilityIdentifier("deck.output.refresh")
            .disabled(model.loadingDetails.contains(card.id))
        }
        .font(.caption)
        .foregroundStyle(.secondary)
        .padding(.horizontal)
        .padding(.vertical, 6)
        .frame(maxWidth: .infinity)
        .background(.bar)
    }

    @ViewBuilder private func outputArea(_ card: CardSummary) -> some View {
        if let output = model.outputs[card.id] {
            // Equatable: a keystroke in the composer re-renders this view's
            // parent, and laying out up to 64 KiB of text on every key drops
            // keys; the output redraws only when what it shows changed.
            TerminalOutputView(output: output, wrapLines: wrapLines, textStep: textStep) { await refreshCurrentTaskDetails() }
                .equatable()
        } else if let unavailable = model.outputUnavailable[card.id] {
            ScrollView {
                VStack(spacing: 12) {
                    Label("Output unavailable: \(outputIssueMessage(unavailable))", systemImage: "terminal.fill")
                        .foregroundStyle(.secondary)
                        .accessibilityIdentifier("deck.output.unavailable")
                    if card.canEditNotes { Text("Notes remain available when output cannot be read.").font(.caption).foregroundStyle(.secondary) }
                }
                .padding()
                .padding(.top, 32)
                .frame(maxWidth: .infinity)
            }
            .refreshable { await refreshCurrentTaskDetails() }
        } else {
            ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
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

    /// One line high until its text needs more; a notice above it appears
    /// only when something needs the user (a pending send, a changed target,
    /// a reason the card cannot take a message, a draft that failed to save).
    @ViewBuilder private func composerSection(_ card: CardSummary) -> some View {
        let generationChanged = composer.expectedGeneration != card.generation
        let pending = model.pendingSends[card.id]
        VStack(alignment: .leading, spacing: 6) {
            if let pending {
                OperationStatusView(record: pending)
            } else if generationChanged && !composer.text.isEmpty {
                HStack {
                    Text(composer.expectedGeneration == nil
                         ? String(localized: "taskDetail.message.unconfirmedTarget")
                         : String(localized: "The target changed. Review your draft before sending."))
                        .font(.caption).foregroundStyle(.orange)
                        .accessibilityIdentifier("deck.message.targetChanged")
                    Spacer(minLength: 0)
                    Button("Use current target") { composer.acceptGeneration(card.generation); model.stageDraft(composer.snapshot) }
                        .font(.caption)
                }
            } else if !card.maySend {
                Text(sendUnavailableReason(card)).font(.caption).foregroundStyle(.secondary)
                    .lineLimit(verticalSizeClass == .compact ? 1 : 3)
            }
            if let error = draftPersistenceError(card) {
                HStack {
                    Text(error).font(.caption).foregroundStyle(.orange)
                    Spacer(minLength: 0)
                    Button("Retry saving draft") { model.stageDraft(composer.snapshot) }.font(.caption)
                }
            }
            HStack(alignment: .bottom, spacing: 8) {
                TextEditor(text: Binding(get: { composer.text }, set: { value in composer.edit(value, currentGeneration: card.generation); model.stageDraft(composer.snapshot) }))
                    .frame(height: draftEditorHeight)
                    .scrollContentBackground(.hidden)
                    .background(alignment: .topLeading) {
                        // Measures the draft at the editor's width; the editor's
                        // height follows it within one to `draftMaxLines` lines.
                        Text(composer.text.isEmpty ? " " : composer.text)
                            .padding(.horizontal, 5)
                            .fixedSize(horizontal: false, vertical: true)
                            .opacity(0)
                            .accessibilityHidden(true)
                            .allowsHitTesting(false)
                            .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { draftTextHeight = $0 }
                    }
                    .overlay(alignment: .topLeading) {
                        if composer.text.isEmpty { Text("Message the agent…").foregroundStyle(.secondary).padding(.vertical, 8).padding(.horizontal, 5).allowsHitTesting(false) }
                    }
                    .padding(.horizontal, 6)
                    .background(RoundedRectangle(cornerRadius: 18, style: .continuous).fill(Color(.secondarySystemBackground)))
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
                .buttonBorderShape(.capsule)
                .accessibilityIdentifier("deck.message.send")
                .disabled(!card.maySend || composer.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || generationChanged || pending != nil || model.sendingCards.contains(card.id))
            }
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

    @ViewBuilder private func scratchpadSection(_ card: CardSummary, focus: FocusState<Bool>.Binding) -> some View {
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
                .focused(focus)
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

/// The output, filling its space. It opens at the end and follows new output
/// while it is scrolled to the end; scrolled up, it keeps its place and
/// offers a jump back that says whether new output arrived. The note on top
/// says how much of the history this snapshot holds, where a reader who
/// scrolled that far needs it. Where the scroll position cannot be read
/// (iOS 17) it never moves on its own after opening: new output only shows
/// the jump.
struct TerminalOutputView: View, Equatable {
    static let textSteps = -2...4

    static func == (lhs: Self, rhs: Self) -> Bool {
        lhs.output == rhs.output && lhs.wrapLines == rhs.wrapLines && lhs.textStep == rhs.textStep
    }
    private static let endID = "terminal-output-end"

    let output: TerminalOutput
    let wrapLines: Bool
    let textStep: Int
    let refresh: () async -> Void

    @ScaledMetric(relativeTo: .caption) private var baseSize = 12.0
    /// Whether the visible part reaches the end of the output.
    @State private var atEnd = true
    @State private var newOutput = false

    private static var tracksPosition: Bool {
        if #available(iOS 18.0, *) { return true }
        return false
    }

    /// The snapshot without the blank rows under an Agent's screen, so its
    /// end is the last line that says something.
    private var shownText: String {
        var text = Substring(output.text)
        while let last = text.last, last.isWhitespace { text.removeLast() }
        return text.isEmpty ? String(localized: "No output") : String(text)
    }

    private var fontSize: CGFloat {
        let step = CGFloat(min(max(textStep, Self.textSteps.lowerBound), Self.textSteps.upperBound))
        return baseSize * (1 + 0.15 * step)
    }

    private var showsJump: Bool { Self.tracksPosition ? !atEnd : newOutput }

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView(wrapLines ? [.vertical] : [.horizontal, .vertical]) {
                VStack(alignment: .leading, spacing: 8) {
                    Text(output.truncated
                         ? String(localized: "taskDetail.output.boundsTruncated")
                         : String(localized: "This is a bounded terminal snapshot, not complete chat history."))
                        .font(.caption2).foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                        .accessibilityIdentifier("deck.output.bounds")
                    Text(shownText)
                        .font(.system(size: fontSize, design: .monospaced))
                        .fixedSize(horizontal: !wrapLines, vertical: true)
                        .textSelection(.enabled)
                        .accessibilityIdentifier("deck.output.text")
                    Color.clear.frame(width: 1, height: 1).id(Self.endID)
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 10)
                .frame(maxWidth: wrapLines ? .infinity : nil, alignment: .leading)
            }
            .modifier(OpensAtEnd())
            .modifier(EndTracker { wasAtEnd, nowAtEnd, resized in
                // the keyboard or a rotation took space away from a reader at
                // the end: bring the end back once, never in reply to our own
                // scroll (that would chase sub-point layout changes forever)
                if resized && wasAtEnd && !nowAtEnd { scrollToEnd(proxy, animated: false) } else { atEnd = nowAtEnd }
                if nowAtEnd { newOutput = false }
            })
            .scrollDismissesKeyboard(.interactively)
            .refreshable { await refresh() }
            .accessibilityIdentifier("deck.output.scroll")
            .overlay(alignment: .bottomTrailing) {
                if showsJump {
                    Button { scrollToEnd(proxy, animated: true) } label: {
                        Label(newOutput ? String(localized: "taskDetail.output.newBelow") : String(localized: "taskDetail.output.latest"),
                              systemImage: "arrow.down.to.line")
                            .font(.caption.weight(.semibold))
                    }
                    .buttonStyle(.borderedProminent)
                    .buttonBorderShape(.capsule)
                    .controlSize(.small)
                    .padding(12)
                    .accessibilityIdentifier("deck.output.latest")
                }
            }
            .onChange(of: output.revision.value) { _, _ in
                if Self.tracksPosition && atEnd { scrollToEnd(proxy, animated: false) } else { newOutput = true }
            }
            .onChange(of: wrapLines) { _, _ in if Self.tracksPosition && atEnd { scrollToEnd(proxy, animated: false) } }
            .onChange(of: textStep) { _, _ in if Self.tracksPosition && atEnd { scrollToEnd(proxy, animated: false) } }
        }
    }

    private func scrollToEnd(_ proxy: ScrollViewProxy, animated: Bool) {
        atEnd = true
        newOutput = false
        // after this update's layout, so the end is where the new text put it
        DispatchQueue.main.async {
            if animated {
                withAnimation { proxy.scrollTo(Self.endID, anchor: .bottomLeading) }
            } else {
                proxy.scrollTo(Self.endID, anchor: .bottomLeading)
            }
        }
    }
}

/// Opens the output at its end. On iOS 18 and later only the first offset is
/// anchored, so a reader scrolled up keeps their place when new text arrives.
private struct OpensAtEnd: ViewModifier {
    func body(content: Content) -> some View {
        if #available(iOS 18.0, *) {
            content.defaultScrollAnchor(.bottomLeading, for: .initialOffset)
        } else {
            content.defaultScrollAnchor(.bottomLeading)
        }
    }
}

/// Reports, from the scroll view's own geometry (iOS 18 and later), whether
/// its visible part reaches the end of the content, and whether the visible
/// height just changed. Nothing is reported on iOS 17.
private struct EndTracker: ViewModifier {
    private struct Probe: Equatable { var atEnd: Bool; var height: CGFloat }
    /// How close to the end still counts as being at the end.
    private static let slack: CGFloat = 24
    let changed: (_ wasAtEnd: Bool, _ nowAtEnd: Bool, _ resized: Bool) -> Void

    func body(content: Content) -> some View {
        if #available(iOS 18.0, *) {
            content.onScrollGeometryChange(for: Probe.self) { geometry in
                let room = geometry.containerSize.height - geometry.contentInsets.top - geometry.contentInsets.bottom
                let fits = geometry.contentSize.height <= room + Self.slack
                let reaches = geometry.visibleRect.maxY >= geometry.contentSize.height - Self.slack
                // whole points: the keyboard and toolbars move the container
                // by fractions that are not a change of space
                return Probe(atEnd: fits || reaches, height: geometry.containerSize.height.rounded())
            } action: { old, new in
                changed(old.atEnd, new.atEnd, abs(old.height - new.height) >= 1)
            }
        } else {
            content
        }
    }
}

/// The notes panel. It owns the note editor's focus: focus does not cross
/// into a sheet from the view that presents it.
private struct NotesSheet<Content: View>: View {
    @Environment(\.dismiss) private var dismiss
    @FocusState private var noteFocused: Bool
    @ViewBuilder let content: (FocusState<Bool>.Binding) -> Content

    var body: some View {
        NavigationStack {
            content($noteFocused)
                .navigationTitle(Text("Notes"))
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .confirmationAction) {
                        Button(String(localized: "taskDetail.keyboard.done")) { dismiss() }
                            .accessibilityIdentifier("deck.notes.close")
                    }
                    ToolbarItemGroup(placement: .keyboard) {
                        Spacer()
                        Button(String(localized: "taskDetail.keyboard.done")) { noteFocused = false }
                            .accessibilityIdentifier("deck.keyboard.done")
                    }
                }
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
            error = model.commandFailureMessage(result.code, commandKind: "buffer-edit")
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

private enum DetailPage: String { case output, replies }

/// A reply's or message's time: the clock today, the date before that.
private func replyTime(_ date: Date) -> String {
    Calendar.current.isDateInToday(date)
        ? date.formatted(.dateTime.hour().minute())
        : date.formatted(.dateTime.month().day().hour().minute())
}

/// A card's Claude Code replies with this phone's messages between them,
/// newest at the bottom. It opens at the end and follows new replies while
/// the reader is there (iOS 18 and later), exactly like the output. The
/// newest reply is open; older long ones show their first lines.
struct RepliesView: View, Equatable {
    static func == (lhs: Self, rhs: Self) -> Bool { lhs.items == rhs.items && lhs.textStep == rhs.textStep }
    private static let endID = "replies-end"

    let items: [ReplyTimelineItem]
    let textStep: Int
    let refresh: () async -> Void

    @ScaledMetric(relativeTo: .body) private var baseSize = 16.0
    /// Replies whose open state differs from the default (newest open).
    @State private var toggled = Set<String>()
    @State private var atEnd = true
    @State private var newReply = false
    @State private var copiedID: String?

    private static var tracksPosition: Bool {
        if #available(iOS 18.0, *) { return true }
        return false
    }
    private var fontSize: CGFloat {
        let step = CGFloat(min(max(textStep, TerminalOutputView.textSteps.lowerBound), TerminalOutputView.textSteps.upperBound))
        return baseSize * (1 + 0.15 * step)
    }
    private var newestReplyID: String? {
        items.last(where: { if case .reply = $0 { true } else { false } })?.id
    }
    private var showsJump: Bool { Self.tracksPosition ? !atEnd : newReply }

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                VStack(alignment: .leading, spacing: 12) {
                    Text(String(localized: "taskDetail.replies.bounds"))
                        .font(.caption2).foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                    ForEach(items) { item in
                        switch item {
                        case let .reply(reply): replyBubble(reply, id: item.id)
                        case let .message(message): messageBubble(message)
                        }
                    }
                    Color.clear.frame(width: 1, height: 1).id(Self.endID)
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 10)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .modifier(OpensAtEnd())
            .modifier(EndTracker { wasAtEnd, nowAtEnd, resized in
                if resized && wasAtEnd && !nowAtEnd { scrollToEnd(proxy, animated: false) } else { atEnd = nowAtEnd }
                if nowAtEnd { newReply = false }
            })
            .scrollDismissesKeyboard(.interactively)
            .refreshable { await refresh() }
            .accessibilityIdentifier("deck.replies.scroll")
            .overlay(alignment: .bottomTrailing) {
                if showsJump {
                    Button { scrollToEnd(proxy, animated: true) } label: {
                        Label(newReply ? String(localized: "taskDetail.replies.newBelow") : String(localized: "taskDetail.replies.latest"),
                              systemImage: "arrow.down.to.line")
                            .font(.caption.weight(.semibold))
                    }
                    .buttonStyle(.borderedProminent)
                    .buttonBorderShape(.capsule)
                    .controlSize(.small)
                    .padding(12)
                    .accessibilityIdentifier("deck.replies.latest")
                }
            }
            .onChange(of: items.last?.id) { _, _ in
                if Self.tracksPosition && atEnd { scrollToEnd(proxy, animated: false) } else { newReply = true }
            }
            .onChange(of: textStep) { _, _ in if Self.tracksPosition && atEnd { scrollToEnd(proxy, animated: false) } }
        }
    }

    @ViewBuilder private func replyBubble(_ reply: AgentReply, id: String) -> some View {
        let open = (id == newestReplyID) != toggled.contains(id)
        let preview = ReplyText.preview(reply.text)
        let shown = open || preview == nil ? reply.text : preview!
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                Text(String(localized: "taskDetail.replies.claude")).font(.caption.weight(.semibold))
                Text(replyTime(reply.date)).font(.caption).foregroundStyle(.secondary)
                Spacer(minLength: 0)
                Button {
                    UIPasteboard.general.string = reply.text
                    copiedID = id
                } label: {
                    Label(copiedID == id ? String(localized: "taskDetail.replies.copied") : String(localized: "taskDetail.replies.copy"),
                          systemImage: copiedID == id ? "checkmark" : "doc.on.doc")
                        .labelStyle(.iconOnly)
                }
                .buttonStyle(.borderless)
                .accessibilityIdentifier("deck.replies.copy")
            }
            ForEach(Array(ReplyText.blocks(shown, cut: reply.cut && shown.contains(ReplyText.cutMark)).enumerated()), id: \.offset) { _, block in
                blockView(block)
            }
            if preview != nil {
                Button(open ? String(localized: "taskDetail.replies.collapse") : String(localized: "taskDetail.replies.expand")) {
                    if toggled.contains(id) { toggled.remove(id) } else { toggled.insert(id) }
                }
                .font(.callout)
                .buttonStyle(.borderless)
                .accessibilityIdentifier("deck.replies.expand")
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 12).fill(Color(.secondarySystemBackground)))
        // one container per reply: its texts and buttons stay reachable, and
        // the identifier names the reply, not each of its pieces
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("deck.replies.reply")
    }

    @ViewBuilder private func blockView(_ block: ReplyBlock) -> some View {
        switch block {
        case let .prose(text):
            Text(Self.inline(text))
                .font(.system(size: fontSize))
                .fixedSize(horizontal: false, vertical: true)
                .textSelection(.enabled)
        case let .heading(text):
            Text(Self.inline(text))
                .font(.system(size: fontSize * 1.1, weight: .semibold))
                .fixedSize(horizontal: false, vertical: true)
                .textSelection(.enabled)
        case let .code(text):
            ScrollView(.horizontal) {
                Text(text)
                    .font(.system(size: fontSize * 0.8, design: .monospaced))
                    .fixedSize()
                    .textSelection(.enabled)
                    .padding(8)
            }
            .background(RoundedRectangle(cornerRadius: 8).fill(Color(.tertiarySystemBackground)))
        case .omitted:
            Label(String(localized: "taskDetail.replies.omitted"), systemImage: "scissors")
                .font(.caption)
                .foregroundStyle(.orange)
                .accessibilityIdentifier("deck.replies.omitted")
        }
    }

    /// Bold, italic, inline code and links; anything else stays as written.
    private static func inline(_ text: String) -> AttributedString {
        (try? AttributedString(markdown: text, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)))
            ?? AttributedString(text)
    }

    @ViewBuilder private func messageBubble(_ message: SentMessage) -> some View {
        VStack(alignment: .trailing, spacing: 4) {
            HStack(spacing: 6) {
                Text(String(localized: "taskDetail.replies.you"))
                Text(replyTime(message.at))
                if !message.confirmed {
                    Image(systemName: "clock").accessibilityLabel(String(localized: "taskDetail.replies.pending"))
                }
            }
            .font(.caption).foregroundStyle(.secondary)
            Text(message.text)
                .font(.system(size: fontSize))
                .fixedSize(horizontal: false, vertical: true)
                .textSelection(.enabled)
                .padding(10)
                .background(RoundedRectangle(cornerRadius: 12).fill(Color.accentColor.opacity(0.15)))
        }
        .frame(maxWidth: .infinity, alignment: .trailing)
        .padding(.leading, 40)
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("deck.replies.message")
    }

    private func scrollToEnd(_ proxy: ScrollViewProxy, animated: Bool) {
        atEnd = true
        newReply = false
        DispatchQueue.main.async {
            if animated {
                withAnimation { proxy.scrollTo(Self.endID, anchor: .bottomLeading) }
            } else {
                proxy.scrollTo(Self.endID, anchor: .bottomLeading)
            }
        }
    }
}


// MARK: - Slack mentions

enum MentionsDestination: Hashable { case list, mention(String) }

private func mentionStateMessage(_ state: String) -> String {
    switch state {
    case "slack-off": String(localized: "mentions.state.slackOff")
    case "not-searched": String(localized: "mentions.state.notSearched")
    default: String(format: String(localized: "mentions.state.failed"), state)
    }
}

private func mentionOutcomeMessage(_ route: MentionRoute) -> String {
    switch route.blocked {
    case "project": return String(localized: "mentions.blocked.project")
    case "command": return String(localized: "mentions.blocked.command")
    case "template": return String(localized: "mentions.blocked.template")
    case .some: return String(localized: "mentions.route.blocked")
    case nil: break
    }
    switch route.leadsTo {
    case .auto: return String(localized: "mentions.outcome.auto")
    case .gated: return String(localized: "mentions.outcome.gated")
    case .manual: return String(localized: "mentions.outcome.manual")
    }
}

private func mentionMarkMessage(_ mark: MentionMark) -> String {
    let word: String
    switch mark.mark {
    case "handling": word = String(localized: mark.cardClosed == true ? "mentions.mark.closed" : "mentions.mark.handling")
    case "for-you": word = String(localized: "mentions.mark.forYou")
    case "no-action": word = String(localized: "mentions.mark.noAction")
    default: word = mark.mark
    }
    let note = (mark.note ?? "").isEmpty ? "" : " — \(mark.note ?? "")"
    return "\(mark.by): \(word)\(note)"
}

/// The Mac's Slack mentions list: the open mentions of the chosen range and,
/// apart, the ones an agent took. Reading it never touches Slack.
struct MentionsView: View {
    @EnvironmentObject private var model: AppModel
    @State private var range: MentionRange = .day

    var body: some View {
        List {
            if let issue = model.mentionsIssue {
                Section { Label(issue, systemImage: "wifi.slash").font(.caption).foregroundStyle(.orange) }
            }
            if let list = model.mentions {
                if list.isCurrent {
                    Section {
                        Picker(String(localized: "mentions.range"), selection: $range) {
                            ForEach(MentionRange.allCases) { value in
                                Text(rangeLabel(value, count: list.items(within: value, now: Date()).count)).tag(value)
                            }
                        }.pickerStyle(.segmented).accessibilityIdentifier("deck.mentions.range")
                    }
                    let items = list.items(within: range, now: Date())
                    if items.isEmpty {
                        ContentUnavailableView(String(localized: "mentions.empty"), systemImage: "at")
                    }
                    Section {
                        ForEach(items) { mention in
                            NavigationLink(value: MentionsDestination.mention(mention.key)) { MentionRow(mention: mention) }
                        }
                    } footer: {
                        if (list.more ?? 0) > 0 { Text(String(format: String(localized: "mentions.more"), list.more ?? 0)) }
                    }
                    let handed = list.handed(within: range, now: Date())
                    if !handed.isEmpty {
                        Section(String(localized: "mentions.handed")) {
                            ForEach(handed) { mention in
                                VStack(alignment: .leading, spacing: 2) {
                                    MentionRow(mention: mention)
                                    if let rule = mention.rule {
                                        Text(String(format: String(localized: "mentions.handed.to"), rule)).font(.caption).foregroundStyle(.secondary)
                                    }
                                }
                            }
                        }
                    }
                    if list.routes.isEmpty {
                        Section { Text(String(localized: "mentions.noRoutes")).font(.caption).foregroundStyle(.secondary) }
                    }
                } else {
                    ContentUnavailableView(String(localized: "mentions.unavailable"), systemImage: "at.badge.minus",
                                           description: Text(mentionStateMessage(list.state)))
                }
            } else if model.loadingMentions {
                ProgressView()
            } else if let state = model.snapshot?.mentions?.state, state != "ok" {
                ContentUnavailableView(String(localized: "mentions.unavailable"), systemImage: "at.badge.minus",
                                       description: Text(mentionStateMessage(state)))
            }
        }
        .navigationTitle(String(localized: "mentions.title"))
        .refreshable { await model.loadMentions() }
        .onAppear { model.showMentions(true) }
        .onDisappear { model.showMentions(false) }
    }

    private func rangeLabel(_ value: MentionRange, count: Int) -> String {
        let key: String.LocalizationValue = switch value {
        case .day: "mentions.range.day"
        case .threeDays: "mentions.range.threeDays"
        case .week: "mentions.range.week"
        }
        return String(format: String(localized: key), count)
    }
}

private struct MentionRow: View {
    let mention: Mention
    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack {
                Text(mention.from).font(.subheadline.weight(.semibold)).lineLimit(1)
                Text(mention.where).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                Spacer()
                Text(replyTime(mention.date)).font(.caption).foregroundStyle(.secondary)
            }
            Text(MentionText.plain(mention.text)).font(.callout).lineLimit(2)
            if let mark = mention.mark {
                Text(mentionMarkMessage(mark)).font(.caption).foregroundStyle(.secondary).lineLimit(2)
            }
        }
    }
}

/// One mention in full with the messages before it, and the three things a
/// phone may do with it: route it to one Slack badge automation, set it
/// aside, or open it in Slack.
struct MentionDetailView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.dismiss) private var dismiss
    @Environment(\.openURL) private var openURL
    let key: String
    @State private var choosing: MentionRoute?
    @State private var confirmDismiss = false

    private var mention: Mention? { model.mentions?.items.first { $0.key == key } }
    private var locked: Bool {
        model.mentionsStale || model.busyMentions.contains(key) || model.unconfirmedMentions.contains(key)
    }

    var body: some View {
        List {
            if let mention {
                if model.mentionsStale {
                    Section { Label(String(localized: "mentions.stale"), systemImage: "wifi.slash").font(.caption).foregroundStyle(.orange) }
                }
                if model.unconfirmedMentions.contains(key) {
                    Section {
                        Text(String(localized: "mentions.unconfirmed")).font(.caption).foregroundStyle(.orange)
                        Button(String(localized: "mentions.check")) { Task { await model.checkOriginalOperations(); await model.loadMentions() } }
                    }
                }
                if let context = mention.context, !context.before.isEmpty {
                    Section(String(localized: context.thread ? "mentions.context.thread" : "mentions.context.before")) {
                        ForEach(Array(context.before.enumerated()), id: \.offset) { _, message in
                            VStack(alignment: .leading, spacing: 2) {
                                HStack {
                                    Text(message.from).font(.caption.weight(.semibold))
                                    Spacer()
                                    Text(replyTime(Date(timeIntervalSince1970: TimeInterval(message.at)))).font(.caption2).foregroundStyle(.secondary)
                                }
                                Text(MentionText.plain(message.text)).font(.callout).foregroundStyle(.secondary).textSelection(.enabled)
                                ForEach(message.files, id: \.self) { Label($0, systemImage: "paperclip").font(.caption).foregroundStyle(.secondary) }
                            }
                        }
                    }
                }
                Section {
                    VStack(alignment: .leading, spacing: 4) {
                        HStack {
                            Text(mention.from).font(.subheadline.weight(.semibold))
                            Text(mention.where).font(.caption).foregroundStyle(.secondary)
                            Spacer()
                            Text(replyTime(mention.date)).font(.caption).foregroundStyle(.secondary)
                        }
                        Text(MentionText.plain(mention.text)).textSelection(.enabled)
                            .accessibilityIdentifier("deck.mention.text")
                        ForEach(mention.context?.files ?? [], id: \.self) { Label($0, systemImage: "paperclip").font(.caption).foregroundStyle(.secondary) }
                        if let mark = mention.mark {
                            Text(mentionMarkMessage(mark)).font(.caption).foregroundStyle(.secondary)
                        }
                    }
                }
                Section(String(localized: "mentions.routeTo")) {
                    let routes = model.mentions?.routes ?? []
                    if routes.isEmpty {
                        Text(String(localized: "mentions.noRoutes")).font(.caption).foregroundStyle(.secondary)
                    }
                    ForEach(routes) { route in
                        Button { choosing = route } label: {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(route.label)
                                Text(mentionOutcomeMessage(route)).font(.caption).foregroundStyle(.secondary)
                            }
                        }
                        .disabled(locked || !route.canBeChosen)
                        .accessibilityIdentifier("deck.mention.route.\(route.id)")
                    }
                }
                Section {
                    if let url = URL(string: mention.link), url.scheme == "https" {
                        Button(String(localized: "mentions.openInSlack")) { openURL(url) }
                    }
                    Button(String(localized: "mentions.dismiss"), role: .destructive) { confirmDismiss = true }
                        .disabled(locked)
                        .accessibilityIdentifier("deck.mention.dismiss")
                }
            } else {
                ContentUnavailableView(String(localized: "mentions.gone"), systemImage: "checkmark.circle")
            }
        }
        .navigationTitle(mention?.from ?? String(localized: "mentions.title"))
        .navigationBarTitleDisplayMode(.inline)
        .onAppear { model.showMentions(true) }
        .confirmationDialog(choosing.map { String(format: String(localized: "mentions.route.confirm"), $0.label) } ?? "",
                            isPresented: Binding(get: { choosing != nil }, set: { if !$0 { choosing = nil } }), titleVisibility: .visible) {
            if let route = choosing, let mention {
                Button(String(localized: "mentions.route.go")) {
                    Task { if case .applied = await model.routeMention(mention, to: route) { dismiss() } }
                }
            }
            Button("Cancel", role: .cancel) { choosing = nil }
        } message: { Text(choosing.map(mentionOutcomeMessage) ?? "") }
        .confirmationDialog(String(localized: "mentions.dismiss.confirm"), isPresented: $confirmDismiss, titleVisibility: .visible) {
            if let mention {
                Button(String(localized: "mentions.dismiss"), role: .destructive) {
                    Task { if case .applied = await model.dismissMention(mention) { dismiss() } }
                }
            }
            Button("Cancel", role: .cancel) { }
        } message: { Text(String(localized: "mentions.dismiss.note")) }
    }
}
