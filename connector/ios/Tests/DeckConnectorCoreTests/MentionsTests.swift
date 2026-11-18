import Foundation
import Testing
@testable import DeckConnectorCore

private func mentionsFixture() throws -> Data {
    let url = try #require(Bundle.module.url(forResource: "mentions", withExtension: "json", subdirectory: "Fixtures"))
    return try Data(contentsOf: url)
}

@Test func theHostsMentionsListDecodesWithEverythingTheScreenShows() throws {
    let list = try JSONDecoder().decode(MentionsList.self, from: mentionsFixture())
    try WireValidator.validate(list)
    #expect(list.isCurrent)
    #expect(list.items.map(\.key) == ["C1/1794989000.000200", "C1/1794980000.000100"])
    let first = try #require(list.items.first)
    #expect(first.context?.thread == true)
    #expect(first.context?.before.first?.files == ["log.txt"])
    #expect(list.items[1].mark?.mark == "for-you")
    #expect(list.items[1].mark?.by == "Orchestrator")
    #expect(list.handed.first?.rule == "Triage")
    #expect(list.routes.map(\.leadsTo) == [.auto, .gated, .manual, .manual])
    #expect(list.routes.map(\.canBeChosen) == [true, true, true, false])
    #expect(list.routes.last?.blocked == "command")
}

@Test func aListThatMayBeStaleCarriesOnlyItsState() throws {
    for word in ["slack-off", "not-searched", "rate-limited"] {
        let raw = #"{"state":"\#(word)","lastPoll":null,"items":[],"handed":[],"routes":[],"more":0}"#
        let list = try JSONDecoder().decode(MentionsList.self, from: Data(raw.utf8))
        try WireValidator.validate(list)
        #expect(!list.isCurrent)
    }
    let lying = #"{"state":"slack-off","lastPoll":null,"items":[{"key":"C1/1.2","text":"x","from":"a","where":"dev","link":"","at":1,"context":null}],"handed":[],"routes":[],"more":0}"#
    let list = try JSONDecoder().decode(MentionsList.self, from: Data(lying.utf8))
    #expect(throws: ConnectorError.invalidResponse) { try WireValidator.validate(list) }
}

@Test func rangesCountByTheMessagesOwnTime() throws {
    let list = try JSONDecoder().decode(MentionsList.self, from: mentionsFixture())
    let now = Date(timeIntervalSince1970: 1_794_989_000 + 3_600)
    #expect(list.items(within: .day, now: now).count == 2)
    let later = Date(timeIntervalSince1970: 1_794_980_000 + 86_400 + 60)
    #expect(list.items(within: .day, now: later).map(\.key) == ["C1/1794989000.000200"])
    #expect(list.items(within: .threeDays, now: later).count == 2)
    #expect(list.handed(within: .week, now: later).count == 1)
    #expect(list.handed(within: .day, now: later).isEmpty)
}

@Test func anUnknownOutcomeReadsAsTheCautiousOne() throws {
    let raw = #"{"id":"r","label":"x","outcome":"something-new"}"#
    let route = try JSONDecoder().decode(MentionRoute.self, from: Data(raw.utf8))
    #expect(route.leadsTo == .manual)
    #expect(route.canBeChosen)
}

@Test func slackMarkupReadsAsPlainText() {
    #expect(MentionText.plain("<@U1|me> see <#C2|dev> and <https://x.test/a|the doc>") == "@me see #dev and the doc (https://x.test/a)")
    #expect(MentionText.plain("<@U9> <https://x.test> <!here>") == "@U9 https://x.test @here")
    #expect(MentionText.plain("a &lt; b &amp;&amp; c &gt; d") == "a < b && c > d")
    #expect(MentionText.plain("if a < b then <not a link> ok") == "if a < b then <not a link> ok")
    #expect(MentionText.plain("dangling < bracket") == "dangling < bracket")
}

@Test func mentionCommandsNameAKeyAndARuleAndNothingElse() throws {
    let route = MentionCommand.route(id: "op-1", key: "C1/1794989000.000200", ruleID: "r-eyes")
    try WireValidator.validate(route)
    #expect(route.cardId == nil && route.expectedRevision == nil && route.expectedGeneration == nil)
    #expect(route.payload == ["key": .string("C1/1794989000.000200"), "ruleId": .string("r-eyes")])
    try WireValidator.validate(MentionCommand.dismiss(id: "op-2", key: "C1/1794989000.000200"))
    #expect(!MentionCommand.validKey("C1/17 94"))
    #expect(!MentionCommand.validKey(""))
    #expect(!MentionCommand.validKey(String(repeating: "a", count: 129)))
    let withText = CommandRequest(id: "op-3", kind: "mention-route", payload: ["key": .string("C1/1.2"), "ruleId": .string("r"), "text": .string("do this")])
    #expect(throws: (any Error).self) { try WireValidator.validate(withText) }
    let withCard = CommandRequest(id: "op-4", kind: "mention-dismiss", cardId: "C1", payload: ["key": .string("C1/1.2")])
    #expect(throws: (any Error).self) { try WireValidator.validate(withCard) }
    let noRule = CommandRequest(id: "op-5", kind: "mention-route", payload: ["key": .string("C1/1.2")])
    #expect(throws: (any Error).self) { try WireValidator.validate(noRule) }
    // a mention result carries a state and at most a closed code
    let applied = CommandResult(id: "op-1", state: .applied, code: nil, result: nil)
    try WireValidator.validate(applied, for: route)
}

@Test func theSnapshotSaysWhetherThereIsAListAndHowManyWait() throws {
    let on = try JSONDecoder().decode(MentionsSummary.self, from: Data(#"{"state":"ok","count":3}"#.utf8))
    #expect(on == MentionsSummary(state: "ok", count: 3))
    let off = try JSONDecoder().decode(MentionsSummary.self, from: Data(#"{"state":"slack-off"}"#.utf8))
    #expect(off.count == nil)
}
