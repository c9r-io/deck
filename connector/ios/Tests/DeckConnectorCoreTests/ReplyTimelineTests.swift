import Foundation
import Testing
@testable import DeckConnectorCore

private func reply(_ seconds: Int64, _ text: String = "r", cut: Bool = false) -> AgentReply {
    AgentReply(at: seconds * 1000, text: text, cut: cut)
}

private func sent(_ id: String, _ seconds: TimeInterval) -> SentMessage {
    SentMessage(id: id, at: Date(timeIntervalSince1970: seconds), text: id, confirmed: true)
}

@Test func messagesSitBetweenRepliesByTime() {
    let items = ReplyTimeline.merge(replies: [reply(10_000), reply(20_000)], messages: [sent("m1", 9_000), sent("m2", 15_000), sent("m3", 25_000)])
    #expect(items.map(\.id) == ["message-m1", "reply-10000000", "message-m2", "reply-20000000", "message-m3"])
}

@Test func messagesLongBeforeTheOldestReplyAndAListWithoutRepliesAreLeftOut() {
    let items = ReplyTimeline.merge(replies: [reply(10_000)], messages: [sent("old", 10_000 - 3_601), sent("near", 10_000 - 3_599)])
    #expect(items.map(\.id) == ["message-near", "reply-10000000"])
    #expect(ReplyTimeline.merge(replies: [], messages: [sent("m", 1)]).isEmpty)
}

@Test func onlyTheNewestMessagesAreKept() {
    let messages = (0..<30).map { sent("m\($0)", TimeInterval(10_000 + $0)) }
    let items = ReplyTimeline.merge(replies: [reply(10_000)], messages: messages)
    #expect(items.count == 1 + ReplyTimeline.maxMessages)
    #expect(items.last?.id == "message-m29")
}

@Test func sentMessagesComeFromTheJournalWithoutRefusedOnes() throws {
    func record(_ id: String, card: String?, kind: String = "send-message", state: CommandResult.State?, local: CommandRecord.LocalState) throws -> CommandRecord {
        let result = try state.map { try JSONDecoder().decode(CommandResult.self, from: Data(#"{"id":"\#(id)","state":"\#($0.rawValue)"}"#.utf8)) }
        return CommandRecord(request: CommandRequest(id: id, kind: kind, cardId: card, payload: ["text": .string("t-\(id)")]),
                             localState: local, result: result, createdAt: Date(timeIntervalSince1970: 1), updatedAt: Date(timeIntervalSince1970: 1))
    }
    let byCard = SentMessage.fromJournal([
        try record("a", card: "C", state: .delivered, local: .resolved),
        try record("b", card: "C", state: nil, local: .unknown),
        try record("c", card: "C", state: .rejected, local: .resolved),
        try record("d", card: "C", kind: "buffer-add", state: .applied, local: .resolved),
        try record("e", card: "D", state: .delivered, local: .resolved),
    ])
    #expect(byCard["C"]?.map(\.id) == ["a", "b"])
    #expect(byCard["C"]?.map(\.confirmed) == [true, false])
    #expect(byCard["D"]?.first?.text == "t-e")
}

@Test func repliesSplitIntoProseHeadingsCodeAndTables() {
    let text = """
    ## Result
    Fixed **two** tests.
    - one
    - two

    ```
    cargo test
    ok
    ```
    | a | b |
    |---|---|
    after
    """
    #expect(ReplyText.blocks(text, cut: false) == [
        .heading("Result"),
        .prose("Fixed **two** tests.\n- one\n- two"),
        .code("cargo test\nok"),
        .code("| a | b |\n|---|---|"),
        .prose("after"),
    ])
    #expect(ReplyText.blocks("```\nopen", cut: false) == [.code("open")])
    #expect(ReplyText.blocks("#tag stays text\n### Third level", cut: false) == [.prose("#tag stays text"), .heading("Third level")])
}

@Test func aCutReplyMarksWhereItsMiddleWas() {
    let text = "head" + ReplyText.cutMark + "tail"
    #expect(ReplyText.blocks(text, cut: true) == [.prose("head"), .omitted, .prose("tail")])
    #expect(ReplyText.blocks(text, cut: false) == [.prose("head\n…\ntail")])
}

@Test func onlyLongRepliesCollapse() {
    let short = (1...14).map(String.init).joined(separator: "\n")
    #expect(ReplyText.preview(short) == nil)
    let long = (1...15).map(String.init).joined(separator: "\n")
    #expect(ReplyText.preview(long) == (1...12).map(String.init).joined(separator: "\n"))
}

@Test func aSnapshotOffersRepliesForClaudeOrKeptReplies() throws {
    func card(agent: String?, count: Int?) -> CardSummary {
        CardSummary(id: "c", projectId: "p", columnId: "k", title: "t", status: "running", generation: nil, canSend: false, canQueue: false,
                    buffer: BufferSummary(revision: WireRevision("0"), collecting: false, entryCount: 0), agent: agent, replyCount: count)
    }
    func snapshot(_ replies: String?) -> Snapshot {
        Snapshot(version: 1, hostId: "h", hostName: "Mac", scope: nil, revision: WireRevision("r"), capturedAt: .integer(1),
                 projects: [], cards: [], queue: [], replies: replies)
    }
    #expect(snapshot("on").offersReplies(card(agent: "claude", count: 0)))
    #expect(snapshot("off").offersReplies(card(agent: "claude", count: nil)))
    #expect(snapshot("on").offersReplies(card(agent: nil, count: 2)))
    #expect(!snapshot("on").offersReplies(card(agent: "codex", count: 0)))
    #expect(!snapshot(nil).offersReplies(card(agent: "claude", count: 3)))
}
