// Exercise the notification bridge's pure helpers and its no-bundle guard
// without a notification center: this test binary is not a .app bundle, so
// every C entry must return the "unsupported" code and do nothing.
import Foundation
import UserNotifications

@main struct NotificationTests {
    static func main() {
        precondition(deckNotifyStatusCode(.notDetermined) == 1)
        precondition(deckNotifyStatusCode(.denied) == 2)
        precondition(deckNotifyStatusCode(.authorized) == 3)
        precondition(deckNotifyStatusCode(.provisional) == 4)

        precondition(deckNotifyIdentifierOk("deck-card-ab12"))
        precondition(deckNotifyIdentifierOk("t"))
        precondition(!deckNotifyIdentifierOk(""))
        precondition(!deckNotifyIdentifierOk("has space"))
        precondition(!deckNotifyIdentifierOk("has:colon"))
        precondition(!deckNotifyIdentifierOk("日本語"))
        precondition(!deckNotifyIdentifierOk(String(repeating: "x", count: 65)))

        precondition(!deckNotifyBundled(), "a test executable is not a bundle")
        precondition(deckNotifyInit({ _ in }) == 0)
        precondition(deckNotifyStatus() == 0)
        precondition(deckNotifyPost("deck-card-ab12", "title", "body", 1) == 0)
        deckNotifyRequest()
        deckNotifyRemove("deck-card-ab12")
    }
}
