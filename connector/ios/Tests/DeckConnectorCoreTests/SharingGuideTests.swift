import Foundation
import Testing
@testable import DeckConnectorCore

private func card(_ id: String, read: CardCapability, send: CardCapability, notes: CardCapability, queue: CardCapability) -> CardSummary {
    CardSummary(id: id, projectId: "p", columnId: "c", title: id, status: "running", generation: nil, canSend: false, canQueue: false,
                buffer: BufferSummary(revision: WireRevision("0"), collecting: false, entryCount: 0),
                capabilities: CardCapabilities(readOutput: read, sendMessage: send, editNotes: notes, queue: queue))
}

private func snapshot(scope: String?, cards: [CardSummary]) -> Snapshot {
    Snapshot(version: 1, hostId: "h", hostName: "Mac", scope: scope, revision: WireRevision("r"), capturedAt: .integer(1),
             projects: [], cards: cards, queue: [])
}

private let denied = CardCapability(allowed: false, reason: "unsupported-target")
private let allowed = CardCapability(allowed: true, reason: nil)

@Test func legacyScopeDenialPointsToFullAccessButQueueNeedsLaunchCommand() {
    for scope in ["boardMetadata", "agentCards"] {
        for kind in [CardCapabilityKind.readOutput, .sendMessage, .editNotes] {
            #expect(SharingGuide.remedy(reason: "unsupported-target", kind: kind, scope: scope) == .chooseFullAccess)
        }
        #expect(SharingGuide.remedy(reason: "unsupported-target", kind: .queue, scope: scope) == .savedLaunchCommand)
    }
    // Full access never makes the sharing choice the remedy again.
    #expect(SharingGuide.remedy(reason: "unsupported-target", kind: .queue, scope: "boardAccess") == .savedLaunchCommand)
    #expect(SharingGuide.remedy(reason: "unsupported-target", kind: .readOutput, scope: "boardAccess") == .generic)
    // Older hosts and future scopes keep the generic explanation.
    #expect(SharingGuide.remedy(reason: "unsupported-target", kind: .readOutput, scope: nil) == .generic)
    #expect(SharingGuide.remedy(reason: "unsupported-target", kind: .readOutput, scope: "futureScope") == .generic)
    // Other reasons are not sharing problems.
    for reason in ["session-unavailable", "agent-not-in-foreground", "mcp-controlled", "target-unavailable", nil] {
        #expect(SharingGuide.remedy(reason: reason, kind: .readOutput, scope: "boardMetadata") == nil)
    }
}

@Test func setupBannerAppearsOnlyWhenTheSharingChoiceIsTheCause() {
    let summaryOnly = card("shell", read: denied, send: denied, notes: denied, queue: denied)
    let savedAgent = card("agent", read: allowed, send: allowed, notes: allowed, queue: allowed)
    let queueOnly = card("manual", read: allowed, send: allowed, notes: allowed, queue: denied)
    let stopped = card("stopped", read: CardCapability(allowed: false, reason: "session-unavailable"),
                       send: CardCapability(allowed: false, reason: "session-unavailable"), notes: allowed, queue: denied)
    #expect(SharingGuide.needsFullAccessSetup(snapshot(scope: "boardMetadata", cards: [savedAgent, summaryOnly])))
    #expect(!SharingGuide.needsFullAccessSetup(snapshot(scope: "boardMetadata", cards: [savedAgent])))
    #expect(!SharingGuide.needsFullAccessSetup(snapshot(scope: "boardAccess", cards: [queueOnly, stopped])))
    #expect(!SharingGuide.needsFullAccessSetup(snapshot(scope: nil, cards: [summaryOnly])))
}
