import Foundation

// A card's reply list on the phone: the Mac's kept Claude Code replies
// (`CardReplies`), with the messages this phone sent to that card (its own
// command journal) placed between them by time. Nothing here reaches the
// Mac: the messages are what this phone already recorded when it sent them.

/// A message this phone sent to a card (a `send-message` journal record).
public struct SentMessage: Equatable, Sendable {
    public let id: String
    public let at: Date
    public let text: String
    /// The Mac confirmed delivery; otherwise it is still pending or unknown.
    public let confirmed: Bool

    public init(id: String, at: Date, text: String, confirmed: Bool) {
        self.id = id; self.at = at; self.text = text; self.confirmed = confirmed
    }

    /// The phone's sent messages per card, newest last, from its journal:
    /// every `send-message` with text, except those the Mac refused.
    public static func fromJournal(_ records: [CommandRecord]) -> [String: [SentMessage]] {
        var byCard: [String: [SentMessage]] = [:]
        for record in records where record.request.kind == "send-message" {
            guard let card = record.request.cardId, case let .string(text)? = record.request.payload["text"] else { continue }
            if record.result?.state == .rejected { continue }
            let confirmed = record.localState == .resolved && [.delivered, .applied].contains(record.result?.state)
            byCard[card, default: []].append(SentMessage(id: record.id, at: record.createdAt, text: text, confirmed: confirmed))
        }
        return byCard.mapValues { $0.sorted { $0.at < $1.at } }
    }
}

public enum ReplyTimelineItem: Equatable, Identifiable, Sendable {
    case reply(AgentReply)
    case message(SentMessage)

    public var id: String {
        switch self {
        case let .reply(reply): "reply-\(reply.at)"
        case let .message(message): "message-\(message.id)"
        }
    }

    public var date: Date {
        switch self {
        case let .reply(reply): reply.date
        case let .message(message): message.at
        }
    }
}

public enum ReplyTimeline {
    /// Sent messages older than the oldest kept reply by more than this are
    /// left out: their replies are no longer kept.
    public static let messageLead: TimeInterval = 60 * 60
    /// At most this many sent messages are shown.
    public static let maxMessages = 20

    /// Replies and sent messages in time order. With no replies there is no
    /// list: messages alone do not make one.
    public static func merge(replies: [AgentReply], messages: [SentMessage]) -> [ReplyTimelineItem] {
        guard let oldest = replies.map(\.date).min() else { return [] }
        let shown = messages.filter { $0.at >= oldest.addingTimeInterval(-messageLead) }.suffix(maxMessages)
        let items = replies.map(ReplyTimelineItem.reply) + shown.map(ReplyTimelineItem.message)
        return items.enumerated().sorted { a, b in
            a.element.date == b.element.date ? a.offset < b.offset : a.element.date < b.element.date
        }.map(\.element)
    }
}

/// A piece of a reply as the phone lays it out.
public enum ReplyBlock: Equatable, Sendable {
    /// Ordinary text; inline Markdown (bold, italic, `code`, links) applies.
    case prose(String)
    case heading(String)
    /// A fenced code block or a table: shown as written, in a fixed-width font.
    case code(String)
    /// Where the Mac kept only the head and tail of a long reply.
    case omitted
}

public enum ReplyText {
    /// The status helper's mark between a long reply's head and tail.
    public static let cutMark = "\n…\n"
    /// A collapsed reply shows this many lines.
    public static let previewLines = 12

    /// The first `previewLines` lines, when the reply has more than a couple
    /// more than that; nil when it is short enough to show whole.
    public static func preview(_ text: String) -> String? {
        let lines = text.split(separator: "\n", omittingEmptySubsequences: false)
        guard lines.count > previewLines + 2 else { return nil }
        return lines.prefix(previewLines).joined(separator: "\n")
    }

    public static func blocks(_ text: String, cut: Bool) -> [ReplyBlock] {
        if cut, let mark = text.range(of: cutMark) {
            return parse(String(text[..<mark.lowerBound])) + [.omitted] + parse(String(text[mark.upperBound...]))
        }
        return parse(text)
    }

    static func parse(_ text: String) -> [ReplyBlock] {
        var blocks: [ReplyBlock] = []
        var prose: [Substring] = []
        var code: [Substring]?
        var table: [Substring] = []
        func flushProse() {
            let joined = prose.joined(separator: "\n").trimmingCharacters(in: .newlines)
            if !joined.isEmpty { blocks.append(.prose(joined)) }
            prose = []
        }
        func flushTable() {
            if !table.isEmpty { blocks.append(.code(table.joined(separator: "\n"))) }
            table = []
        }
        for line in text.split(separator: "\n", omittingEmptySubsequences: false) {
            let trimmed = line.drop(while: { $0 == " " })
            if trimmed.hasPrefix("```") {
                if let open = code {
                    blocks.append(.code(open.joined(separator: "\n")))
                    code = nil
                } else {
                    flushProse(); flushTable()
                    code = []
                }
                continue
            }
            if code != nil { code!.append(line); continue }
            if trimmed.hasPrefix("|") { flushProse(); table.append(line); continue }
            flushTable()
            // `# Title` … `###### Title`: hashes, then a space
            let hashes = trimmed.prefix(while: { $0 == "#" }).count
            if (1...6).contains(hashes), trimmed.dropFirst(hashes).first == " " {
                let title = trimmed.dropFirst(hashes).trimmingCharacters(in: .whitespaces)
                if !title.isEmpty { flushProse(); blocks.append(.heading(title)); continue }
            }
            if trimmed.isEmpty { flushProse(); continue }
            prose.append(line)
        }
        // an unclosed fence keeps what it held
        if let open = code { blocks.append(.code(open.joined(separator: "\n"))) }
        flushTable(); flushProse()
        return blocks
    }
}
