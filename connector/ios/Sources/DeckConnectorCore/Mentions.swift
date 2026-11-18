import Foundation

// The Mac's "Slack mentions" list as a paired phone reads it (host
// `slack_mentions.rs` "The phone reader", GET /v1/mentions). The phone names
// one listed mention and, for a route, one of the user's Slack badge
// automations; it never supplies text and never talks to Slack. What a route
// sends is decided on the Mac by that automation's own approval.

/// The snapshot's word about the list: `state` is `ok`, `slack-off`,
/// `not-searched` or the last search's failure; `count` (only beside `ok`)
/// is how many mentions of the last 24 hours are open.
public struct MentionsSummary: Codable, Equatable, Sendable {
    public let state: String
    public let count: Int?
    public init(state: String, count: Int? = nil) { self.state = state; self.count = count }
}

public struct MentionContextMessage: Codable, Equatable, Sendable {
    public let from: String
    public let text: String
    public let at: Int64
    public let files: [String]
}

/// What the reader needs to judge a mention without opening Slack: its text
/// with names in place of ids, its files, and the messages before it (a
/// thread's first message and the replies just before, or the conversation's
/// last messages).
public struct MentionContext: Codable, Equatable, Sendable {
    public let from: String
    public let thread: Bool
    public let before: [MentionContextMessage]
    public let text: String
    public let files: [String]
}

/// A mark an MCP orchestrator set on a mention: presentation only.
public struct MentionMark: Codable, Equatable, Sendable {
    public let mark: String
    public let by: String
    public let cardTitle: String?
    public let cardClosed: Bool?
    public let note: String?
}

public struct Mention: Codable, Identifiable, Equatable, Sendable {
    public let key: String
    public let text: String
    public let from: String
    public let `where`: String
    public let link: String
    /// The message's own time, epoch seconds.
    public let at: Int64
    public let context: MentionContext?
    public let mark: MentionMark?
    /// Only on a handed-over mention: the Agent automation that took it.
    public let rule: String?
    public var id: String { key }
    public var date: Date { Date(timeIntervalSince1970: TimeInterval(at)) }
}

/// One Slack badge automation as a route. `outcome` says what choosing it
/// leads to; `blocked` why it cannot be chosen. Both are explanations the
/// Mac computed, never authority.
public struct MentionRoute: Codable, Identifiable, Equatable, Sendable {
    public enum Outcome: Equatable, Sendable { case auto, gated, manual }
    public let id: String
    public let label: String
    public let outcome: String
    public let blocked: String?

    /// An outcome word this build does not know reads as the most cautious one.
    public var leadsTo: Outcome {
        switch outcome { case "auto": .auto; case "gated": .gated; default: .manual }
    }
    public var canBeChosen: Bool { blocked == nil }
}

public struct MentionsList: Codable, Equatable, Sendable {
    public let state: String
    public let lastPoll: Int64?
    public let items: [Mention]
    public let handed: [Mention]
    public let routes: [MentionRoute]
    /// Open mentions beyond the ones this read carries.
    public let more: Int?

    public var isCurrent: Bool { state == "ok" }
}

public enum MentionRange: Int, CaseIterable, Identifiable, Sendable {
    case day = 1, threeDays = 3, week = 7
    public var id: Int { rawValue }
}

public enum MentionLimits {
    public static let items = 200
    public static let routes = 32
    public static let keyUTF8Bytes = 128
    public static let ruleIDUTF8Bytes = 64
    public static let labelUTF8Bytes = 300
}

public extension MentionsList {
    /// The open mentions sent within `range` of `now`, newest first as sent.
    func items(within range: MentionRange, now: Date) -> [Mention] {
        let since = Int64(now.timeIntervalSince1970) - Int64(range.rawValue) * 86_400
        return items.filter { $0.at >= since }
    }

    /// The list without mention `key` (it was just routed or set aside).
    func removing(_ key: String) -> MentionsList {
        MentionsList(state: state, lastPoll: lastPoll, items: items.filter { $0.key != key }, handed: handed, routes: routes, more: more)
    }

    func handed(within range: MentionRange, now: Date) -> [Mention] {
        let since = Int64(now.timeIntervalSince1970) - Int64(range.rawValue) * 86_400
        return handed.filter { $0.at >= since }
    }
}

public enum MentionText {
    /// Slack's markup as plain reading text: `<@U1|name>` → `@name`,
    /// `<#C1|name>` → `#name`, `<url|label>` → `label (url)`, `<url>` → `url`,
    /// and the three escaped characters. Anything else is left as written.
    public static func plain(_ text: String) -> String {
        var out = ""
        var rest = Substring(text)
        while let open = rest.firstIndex(of: "<") {
            out += rest[..<open]
            let after = rest[rest.index(after: open)...]
            guard let close = after.firstIndex(of: ">") else { out += rest[open...]; rest = ""; break }
            let body = after[..<close]
            let parts = body.split(separator: "|", maxSplits: 1, omittingEmptySubsequences: false)
            let target = String(parts.first ?? "")
            let label = parts.count > 1 ? String(parts[1]) : nil
            if target.hasPrefix("@") { out += "@" + (label ?? String(target.dropFirst())) }
            else if target.hasPrefix("#") { out += "#" + (label ?? String(target.dropFirst())) }
            else if target.hasPrefix("!") { out += "@" + (label ?? String(target.dropFirst())) }
            else if target.contains("://") || target.hasPrefix("mailto:") {
                if let label, label != target { out += "\(label) (\(target))" } else { out += target }
            } else { out += "<" + body + ">" }
            rest = after[after.index(after: close)...]
        }
        out += rest
        return out.replacingOccurrences(of: "&lt;", with: "<")
            .replacingOccurrences(of: "&gt;", with: ">")
            .replacingOccurrences(of: "&amp;", with: "&")
    }
}

public enum MentionCommand {
    /// The host's key rule (`slack_mentions::key_shape`): `channel/ts`.
    public static func validKey(_ key: String) -> Bool {
        !key.isEmpty && key.utf8.count <= MentionLimits.keyUTF8Bytes
            && key.unicodeScalars.allSatisfy { $0.isASCII && (CharacterSet.alphanumerics.contains($0) || $0 == "/" || $0 == ".") }
    }

    public static func route(id: String, key: String, ruleID: String) -> CommandRequest {
        CommandRequest(id: id, kind: "mention-route", payload: ["key": .string(key), "ruleId": .string(ruleID)])
    }

    public static func dismiss(id: String, key: String) -> CommandRequest {
        CommandRequest(id: id, kind: "mention-dismiss", payload: ["key": .string(key)])
    }
}

public extension WireValidator {
    static func validate(_ list: MentionsList) throws {
        guard list.items.count <= MentionLimits.items, list.handed.count <= MentionLimits.items,
              list.routes.count <= MentionLimits.routes,
              (list.items + list.handed).allSatisfy({ MentionCommand.validKey($0.key) }),
              list.routes.allSatisfy({ !$0.id.isEmpty && $0.id.utf8.count <= MentionLimits.ruleIDUTF8Bytes
                  && !$0.label.isEmpty && $0.label.utf8.count <= MentionLimits.labelUTF8Bytes }),
              list.isCurrent || (list.items.isEmpty && list.routes.isEmpty)
        else { throw ConnectorError.invalidResponse }
    }
}

public extension ConnectorTransport {
    /// A transport without the list (a test double, an older build's fake).
    func mentions() async throws -> MentionsList { throw ConnectorError.transport("mentions unavailable") }
}
