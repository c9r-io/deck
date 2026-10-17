// Explains a denied card capability in terms the Mac user can act on.
//
// The host's `unsupported-target` code means "not admitted by this Mac's
// saved configuration". Its remedy depends on the advertised sharing scope
// and on which capability was denied: under the two legacy scopes the Mac
// user can choose full output/notes sharing; under BoardAccess only queueing
// is ever refused this way, and that needs a saved launch command instead.
// This is presentation only: it never grants, infers or caches a capability.
// An unknown or missing scope (an older host) keeps the generic explanation.
import Foundation

public enum SharingScope: String, Sendable {
    case agentCards, boardMetadata, boardAccess
}

public enum CardCapabilityKind: Sendable {
    case readOutput, sendMessage, editNotes, queue
}

public enum SharingRemedy: Equatable, Sendable {
    /// The Mac shares less than this action needs; choosing full output and
    /// notes sharing on that Mac would permit it.
    case chooseFullAccess
    /// Only a saved Codex/Claude launch command admits queueing.
    case savedLaunchCommand
    /// The host did not say enough to name a precise remedy.
    case generic
}

public enum SharingGuide {
    public static func remedy(reason: String?, kind: CardCapabilityKind, scope: String?) -> SharingRemedy? {
        guard reason == "unsupported-target" else { return nil }
        switch (SharingScope(rawValue: scope ?? ""), kind) {
        case (.boardAccess, .queue): return .savedLaunchCommand
        case (.boardAccess, _): return .generic
        case (.agentCards, .queue), (.boardMetadata, .queue): return .savedLaunchCommand
        case (.agentCards, _), (.boardMetadata, _): return .chooseFullAccess
        case (nil, _): return .generic
        }
    }

    /// The capability a rejected command exercised, from its own wire kind —
    /// never from the current scope (a direct message refused under
    /// BoardAccess is not a queue limit).
    public static func capability(forCommand kind: String) -> CardCapabilityKind? {
        switch kind {
        case "send-message": .sendMessage
        case "buffer-add", "buffer-edit", "buffer-delete": .editNotes
        case "buffer-queue", "queue-pause", "queue-cancel": .queue
        default: nil
        }
    }

    /// True when at least one listed card lacks output, messages or notes only
    /// because of this Mac's sharing choice. The list then leads with setup.
    public static func needsFullAccessSetup(_ snapshot: Snapshot) -> Bool {
        snapshot.cards.contains { card in
            [card.capabilities?.readOutput, card.capabilities?.sendMessage, card.capabilities?.editNotes]
                .contains { remedy(reason: $0?.reason, kind: .readOutput, scope: snapshot.scope) == .chooseFullAccess }
        }
    }
}
