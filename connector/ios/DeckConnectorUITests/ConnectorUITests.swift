import Foundation
import XCTest

final class ConnectorUITests: XCTestCase {
    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    func testPairedCardOutputSurvivesForegroundCycle() throws {
        let environment = ProcessInfo.processInfo.environment
        guard environment["DECK_UI_PAIRED_SMOKE"] == "1" else {
            throw XCTSkip("Set DECK_UI_PAIRED_SMOKE=1 to run against an already paired device.")
        }
        let cardTitle = environment["DECK_UI_CARD_TITLE"]?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        guard !cardTitle.isEmpty else { throw XCTSkip("DECK_UI_CARD_TITLE is required.") }
        let expectedOutput = environment["DECK_UI_EXPECTED_OUTPUT"]?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        guard !expectedOutput.isEmpty else { throw XCTSkip("DECK_UI_EXPECTED_OUTPUT is required.") }

        let app = XCUIApplication()
        app.launch()
        openCard(named: cardTitle, in: app)
        assertLatestOutput(expectedOutput, in: app)

        XCUIDevice.shared.press(.home)
        app.activate()
        if !app.navigationBars[cardTitle].waitForExistence(timeout: 5) {
            openCard(named: cardTitle, in: app)
        }
        assertLatestOutput(expectedOutput, in: app)

        let attachment = XCTAttachment(screenshot: app.screenshot())
        attachment.name = "Paired card detail"
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    private func openCard(named title: String, in app: XCUIApplication) {
        let card = app.staticTexts[title].firstMatch
        XCTAssertTrue(card.waitForExistence(timeout: 15), "Expected paired test card named \(title).")
        card.tap()
        XCTAssertTrue(app.navigationBars[title].waitForExistence(timeout: 10), "Expected card detail for \(title).")
    }

    private func assertLatestOutput(_ expected: String, in app: XCUIApplication) {
        let latest = app.buttons["deck.output.latest"]
        XCTAssertTrue(reveal(latest, in: app), "Expected the Latest output control on the card detail.")
        latest.tap()

        let output = app.descendants(matching: .any)["deck.output.text"]
        XCTAssertTrue(output.waitForExistence(timeout: 10), "Expected the terminal output text.")
        XCTAssertTrue(output.label.contains(expected), "Expected the terminal output accessibility label to contain the requested marker.")
        // AX exposes the complete Text label, but not a substring's clipped on-screen geometry.
        // Tapping Latest output validates the scroll action without claiming that range is visually provable here.
    }

    private func reveal(_ element: XCUIElement, in app: XCUIApplication) -> Bool {
        if element.waitForExistence(timeout: 2), element.isHittable { return true }
        for _ in 0..<8 {
            app.swipeUp()
            if element.waitForExistence(timeout: 1), element.isHittable { return true }
        }
        return false
    }
}
