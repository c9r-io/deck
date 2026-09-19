import Foundation
import Testing
@testable import DeckConnectorCore

private enum DraftFailure: Error { case disk }

private actor ControlledDraftSink: MessageDraftSink {
    var saved: [MessageDraft] = []
    var shouldFail = false
    var blockNext = false
    private var blocked = false
    private var observers: [CheckedContinuation<Void, Never>] = []
    private var release: CheckedContinuation<Void, Never>?

    func saveDraft(_ draft: MessageDraft) async throws {
        if shouldFail { throw DraftFailure.disk }
        if blockNext {
            blockNext = false; blocked = true
            observers.forEach { $0.resume() }; observers = []
            await withCheckedContinuation { release = $0 }
            blocked = false
        }
        saved.append(draft)
    }
    func arrangeBlock() { blockNext = true }
    func arrangeFailure(_ value: Bool) { shouldFail = value }
    func waitUntilBlocked() async { if blocked { return }; await withCheckedContinuation { observers.append($0) } }
    func releaseSave() { release?.resume(); release = nil }
}

private func versioned(_ text: String, _ version: UInt64, card: String = "card") -> VersionedMessageDraft {
    VersionedMessageDraft(draft: MessageDraft(cardID: card, text: text, expectedGeneration: "g"), version: version)
}

@Test func rapidDraftWritesStayOrderedAndSupersedeOlderVersion() async throws {
    let sink = ControlledDraftSink()
    let coordinator = DraftSaveCoordinator(sink: sink)
    _ = try await coordinator.persist(versioned("new", 2))
    #expect(try await coordinator.persist(versioned("old", 1)) == .superseded(2))
    #expect(await sink.saved.map(\.text) == ["new"])
}

@Test func delayedWriteSerializesNewerEditAndFailureCanRetry() async throws {
    let sink = ControlledDraftSink()
    let coordinator = DraftSaveCoordinator(sink: sink)
    await sink.arrangeBlock()
    let first = Task { try await coordinator.persist(versioned("one", 1)) }
    await sink.waitUntilBlocked()
    let second = Task { try await coordinator.persist(versioned("two", 2)) }
    await sink.releaseSave()
    _ = try await first.value; _ = try await second.value
    #expect(await sink.saved.map(\.text) == ["one", "two"])

    await sink.arrangeFailure(true)
    await #expect(throws: DraftFailure.disk) { try await coordinator.persist(versioned("three", 3)) }
    await sink.arrangeFailure(false)
    #expect(try await coordinator.persist(versioned("three", 3)) == .saved(3))
}

@Test func dirtyComposerIgnoresRefreshAndSendSnapshotIsExact() async throws {
    var state = ComposerDraftState(cardID: "card", remote: MessageDraft(cardID: "card", text: "remote", expectedGeneration: "g"), fallbackGeneration: "g")
    state.edit("visible")
    state.merge(remote: MessageDraft(cardID: "card", text: "stale refresh", expectedGeneration: "g"), fallbackGeneration: "g")
    #expect(state.text == "visible")

    let sink = ControlledDraftSink()
    let coordinator = DraftSaveCoordinator(sink: sink)
    let visible = state.snapshot
    _ = try await coordinator.persistForSend(visible)
    state.edit("newer while sending")
    state.clearIfUnchanged(sentVersion: visible.version, fallbackGeneration: "g")
    #expect(state.text == "newer while sending")
    #expect(await sink.saved.last?.text == "visible")
}

@Test func coordinatorsDoNotCrossCredentialBinding() async throws {
    let oldSink = ControlledDraftSink(), newSink = ControlledDraftSink()
    let old = DraftSaveCoordinator(sink: oldSink), new = DraftSaveCoordinator(sink: newSink)
    _ = try await old.persist(versioned("old binding", 1))
    _ = try await new.persist(versioned("new binding", 1))
    #expect(await oldSink.saved.map(\.text) == ["old binding"])
    #expect(await newSink.saved.map(\.text) == ["new binding"])
}
