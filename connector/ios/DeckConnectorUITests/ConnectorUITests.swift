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

    func testPairedCardOutputSurvivesProcessTermination() throws {
        let environment = ProcessInfo.processInfo.environment
        guard environment["DECK_UI_PAIRED_SMOKE"] == "1" else {
            throw XCTSkip("Set DECK_UI_PAIRED_SMOKE=1 to run against an already paired device.")
        }
        let cardTitle = environment["DECK_UI_CARD_TITLE"]?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        guard !cardTitle.isEmpty else { throw XCTSkip("DECK_UI_CARD_TITLE is required.") }
        let expectedNote = environment["DECK_UI_EXPECTED_NOTE"]?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        guard !expectedNote.isEmpty else { throw XCTSkip("DECK_UI_EXPECTED_NOTE is required.") }

        let app = XCUIApplication()
        app.launch()
        openCard(named: cardTitle, in: app)
        XCTAssertTrue(
            noteText(expectedNote, in: app).waitForExistence(timeout: 15),
            "Expected the paired scratchpad note before process termination."
        )

        app.terminate()
        app.launch()
        openCard(named: cardTitle, in: app)
        XCTAssertTrue(
            noteText(expectedNote, in: app).waitForExistence(timeout: 15),
            "Expected the paired scratchpad note after process termination and Keychain restore."
        )

        let attachment = XCTAttachment(screenshot: app.screenshot())
        attachment.name = "Paired card detail after process termination"
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    func testPairedScratchpadNoteCRUD() throws {
        let environment = ProcessInfo.processInfo.environment
        guard environment["DECK_UI_PAIRED_SMOKE"] == "1" else {
            throw XCTSkip("Set DECK_UI_PAIRED_SMOKE=1 to run against an already paired device.")
        }
        guard environment["DECK_UI_ALLOW_NOTE_MUTATION"] == "1" else {
            throw XCTSkip("Set DECK_UI_ALLOW_NOTE_MUTATION=1 to permit one scratchpad CRUD cycle.")
        }
        let cardTitle = environment["DECK_UI_CARD_TITLE"]?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        guard !cardTitle.isEmpty else { throw XCTSkip("DECK_UI_CARD_TITLE is required.") }
        let marker = environment["DECK_UI_NOTE_MARKER"]?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        guard !marker.isEmpty else { throw XCTSkip("DECK_UI_NOTE_MARKER is required.") }
        guard marker.hasPrefix("DECK-UI-NOTE-"), marker.count > "DECK-UI-NOTE-".count else {
            XCTFail("DECK_UI_NOTE_MARKER must use a unique DECK-UI-NOTE-<run> value.")
            return
        }
        let editedMarker = marker + "-edited"

        let app = XCUIApplication()
        app.launch()
        openCard(named: cardTitle, in: app)
        let bufferLoaded = app.buttons["deck.note.queue-selected"]
        XCTAssertTrue(bufferLoaded.waitForExistence(timeout: 15), "Expected the scratchpad buffer to finish loading before checking the marker.")
        XCTAssertFalse(noteText(marker, in: app).exists, "The run marker already exists; investigate it instead of mutating again.")
        XCTAssertFalse(noteText(editedMarker, in: app).exists, "The edited run marker already exists; investigate it instead of mutating again.")

        let newNote = app.descendants(matching: .any)["deck.note.new"]
        XCTAssertTrue(revealBySwipingDown(newNote, in: app), "Expected the new scratchpad note editor.")
        newNote.tap()
        newNote.typeText(marker)
        let keyboardDone = app.buttons["deck.keyboard.done"]
        XCTAssertTrue(keyboardDone.waitForExistence(timeout: 5) && keyboardDone.isHittable, "Expected the keyboard Done control.")
        keyboardDone.tap()

        let saveNote = app.buttons["deck.note.save"]
        XCTAssertTrue(saveNote.waitForExistence(timeout: 5) && saveNote.isHittable && saveNote.isEnabled, "Expected an enabled Save note control.")
        saveNote.tap()

        let addedNote = noteText(marker, in: app)
        if !addedNote.waitForExistence(timeout: 12) {
            checkOriginalNoteOperationOnce(in: app)
        }
        XCTAssertTrue(addedNote.waitForExistence(timeout: 12), "The added note did not reach its applied state; leave its marker for investigation.")
        guard editOnlyNote(marker, to: editedMarker, in: app) else { return }
        guard deleteOnlyNote(editedMarker, in: app) else { return }
    }

    func testPairedScratchpadResumeEditDeleteTestNote() throws {
        let environment = ProcessInfo.processInfo.environment
        guard environment["DECK_UI_PAIRED_SMOKE"] == "1" else {
            throw XCTSkip("Set DECK_UI_PAIRED_SMOKE=1 to run against an already paired device.")
        }
        guard environment["DECK_UI_ALLOW_NOTE_MUTATION"] == "1" else {
            throw XCTSkip("Set DECK_UI_ALLOW_NOTE_MUTATION=1 to permit editing and deleting one test note.")
        }
        let cardTitle = environment["DECK_UI_CARD_TITLE"]?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        guard !cardTitle.isEmpty else { throw XCTSkip("DECK_UI_CARD_TITLE is required.") }
        let marker = environment["DECK_UI_NOTE_MARKER"]?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        guard !marker.isEmpty else { throw XCTSkip("DECK_UI_NOTE_MARKER is required.") }
        guard marker.hasPrefix("DECK-UI-NOTE-"), marker.count > "DECK-UI-NOTE-".count else {
            XCTFail("DECK_UI_NOTE_MARKER must identify an exact DECK-UI-NOTE-<run> value.")
            return
        }
        let editedMarker = marker + "-edited"

        let app = XCUIApplication()
        app.launch()
        openCard(named: cardTitle, in: app)
        let bufferLoaded = app.buttons["deck.note.queue-selected"]
        XCTAssertTrue(bufferLoaded.waitForExistence(timeout: 15), "Expected the scratchpad buffer to finish loading before finding the marker.")
        guard editOnlyNote(marker, to: editedMarker, in: app) else { return }
        guard deleteOnlyNote(editedMarker, in: app) else { return }
    }

    func testPairedScratchpadDeleteTestNote() throws {
        let environment = ProcessInfo.processInfo.environment
        guard environment["DECK_UI_PAIRED_SMOKE"] == "1" else {
            throw XCTSkip("Set DECK_UI_PAIRED_SMOKE=1 to run against an already paired device.")
        }
        guard environment["DECK_UI_ALLOW_NOTE_MUTATION"] == "1" else {
            throw XCTSkip("Set DECK_UI_ALLOW_NOTE_MUTATION=1 to permit deleting one test note.")
        }
        let cardTitle = environment["DECK_UI_CARD_TITLE"]?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        guard !cardTitle.isEmpty else { throw XCTSkip("DECK_UI_CARD_TITLE is required.") }
        let marker = environment["DECK_UI_NOTE_MARKER"]?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        guard !marker.isEmpty else { throw XCTSkip("DECK_UI_NOTE_MARKER is required.") }
        guard marker.hasPrefix("DECK-UI-NOTE-"), marker.count > "DECK-UI-NOTE-".count else {
            XCTFail("DECK_UI_NOTE_MARKER must identify an exact DECK-UI-NOTE-<run> value.")
            return
        }

        let app = XCUIApplication()
        app.launch()
        openCard(named: cardTitle, in: app)
        let bufferLoaded = app.buttons["deck.note.queue-selected"]
        XCTAssertTrue(bufferLoaded.waitForExistence(timeout: 15), "Expected the scratchpad buffer to finish loading before finding the marker.")
        guard deleteOnlyNote(marker, in: app) else { return }
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

    private func noteText(_ text: String, in app: XCUIApplication) -> XCUIElement {
        noteTexts(text, in: app).firstMatch
    }

    private func noteTexts(_ text: String, in app: XCUIApplication) -> XCUIElementQuery {
        app.staticTexts.matching(NSPredicate(format: "identifier == %@ AND label == %@", "deck.note.text", text))
    }

    private func noteRow(_ text: String, in app: XCUIApplication) -> XCUIElement? {
        let rows = app.descendants(matching: .any).matching(identifier: "deck.note.row")
        for index in 0..<rows.count {
            let row = rows.element(boundBy: index)
            let note = row.staticTexts.matching(identifier: "deck.note.text").firstMatch
            if note.exists && note.label == text { return row }
        }
        return nil
    }

    private func checkOriginalNoteOperationOnce(in app: XCUIApplication) {
        let check = app.buttons["deck.note.check-original"]
        XCTAssertTrue(revealBySwipingDown(check, in: app), "Expected Check original operation for the pending note mutation.")
        check.tap()
    }

    private func waitForNoteMutationControls(
        for text: String,
        in app: XCUIApplication,
        timeout: TimeInterval = 15
    ) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        let check = app.buttons["deck.note.check-original"]
        var checkedOriginalOperation = false

        while Date() < deadline {
            if let row = noteRow(text, in: app) {
                let edit = row.buttons["deck.note.edit"]
                if edit.exists && edit.isEnabled { return true }
            }

            if !checkedOriginalOperation,
               revealBySwipingDown(check, in: app),
               check.isHittable,
               check.isEnabled {
                check.tap()
                checkedOriginalOperation = true
            }
            RunLoop.current.run(until: Date().addingTimeInterval(0.25))
        }

        XCTFail("The note mutation controls remained disabled after checking the original operation at most once. No new mutation was attempted.")
        return false
    }

    private func editOnlyNote(_ original: String, to edited: String, in app: XCUIApplication) -> Bool {
        let originals = noteTexts(original, in: app)
        guard originals.count == 1 else {
            XCTFail("Expected exactly one note matching the original marker; found \(originals.count). No edit was attempted.")
            return false
        }
        guard noteTexts(edited, in: app).count == 0 else {
            XCTFail("The edited marker already exists. No edit was attempted.")
            return false
        }
        guard waitForNoteMutationControls(for: original, in: app) else { return false }
        let originalNote = originals.firstMatch
        guard reveal(originalNote, in: app), let row = noteRow(original, in: app) else {
            XCTFail("Could not identify the unique original marker row. No edit was attempted.")
            return false
        }
        let edit = row.buttons["deck.note.edit"]
        guard edit.exists, edit.isHittable else {
            XCTFail("Expected the edit control only for the original marker row.")
            return false
        }
        edit.tap()

        let editor = app.descendants(matching: .any)["deck.note.edit.text"]
        guard editor.waitForExistence(timeout: 5), editor.isHittable else {
            XCTFail("Expected the note editor.")
            return false
        }
        guard replaceAllText(in: editor, expectedOriginal: original, with: edited) else { return false }
        guard editor.value as? String == edited else {
            XCTFail("The edit text must exactly match the expected marker before saving. The edit was not saved.")
            return false
        }
        let save = app.buttons["deck.note.edit.save"]
        guard save.waitForExistence(timeout: 5), save.isHittable, save.isEnabled else {
            XCTFail("Expected the edit Save control only after exact replacement.")
            return false
        }
        save.tap()

        let pendingAlert = app.alerts["Note not confirmed"]
        if pendingAlert.waitForExistence(timeout: 2) {
            let ok = pendingAlert.buttons["OK"]
            guard ok.isHittable else {
                XCTFail("Expected the pending edit acknowledgement.")
                return false
            }
            ok.tap()
            XCUIDevice.shared.press(.home)
            app.activate()
            if app.navigationBars["Edit note"].waitForExistence(timeout: 3) {
                let check = app.buttons["deck.note.edit.check-original"]
                guard check.waitForExistence(timeout: 3), check.isHittable, check.isEnabled else {
                    XCTFail("Expected an in-editor check for the already submitted operation; the edit was not submitted again.")
                    return false
                }
                check.tap()
            }
        }
        guard app.navigationBars["Edit note"].waitForNonExistence(timeout: 15) else {
            XCTFail("The edit did not reach its applied state; do not submit it again.")
            return false
        }
        let editedNote = noteText(edited, in: app)
        guard editedNote.waitForExistence(timeout: 12) else {
            XCTFail("Expected the exact edited marker.")
            return false
        }
        guard waitForNoteMutationControls(for: edited, in: app) else { return false }
        XCTAssertFalse(noteText(original, in: app).exists, "The original marker remained after editing.")
        return !noteText(original, in: app).exists
    }

    private func deleteOnlyNote(_ text: String, in app: XCUIApplication) -> Bool {
        let matches = noteTexts(text, in: app)
        guard matches.count == 1 else {
            XCTFail("Expected exactly one note matching the explicit marker; found \(matches.count). No delete was attempted.")
            return false
        }
        guard waitForNoteMutationControls(for: text, in: app) else { return false }
        let note = matches.firstMatch
        guard reveal(note, in: app) else {
            XCTFail("Expected the exact marker note to be visible. No delete was attempted.")
            return false
        }
        guard let row = noteRow(text, in: app) else {
            XCTFail("Could not identify the unique row belonging to the explicit marker. No delete was attempted.")
            return false
        }
        row.swipeLeft()
        let delete = app.buttons["deck.note.delete"]
        guard delete.waitForExistence(timeout: 5), delete.isHittable else {
            XCTFail("Expected Delete only for the explicit marker row.")
            return false
        }
        delete.tap()
        if !note.waitForNonExistence(timeout: 12) {
            checkOriginalNoteOperationOnce(in: app)
        }
        XCTAssertTrue(note.waitForNonExistence(timeout: 12), "The delete did not reach its applied state; leave the marker for investigation.")
        return !note.exists
    }

    private func replaceAllText(in editor: XCUIElement, expectedOriginal: String, with replacement: String) -> Bool {
        guard editor.value as? String == expectedOriginal else {
            XCTFail("The editor did not contain the exact original marker. No edit was attempted.")
            return false
        }
        guard isPrintableASCII(expectedOriginal), isPrintableASCII(replacement) else {
            XCTFail("The physical-device replacement helper only accepts printable ASCII test markers. The edit was not saved.")
            return false
        }

        // XCTest does not expose the selection range, and physical-device software keyboards
        // need not honor Command-A. Prove the caret is at the end with an unsaved probe before
        // deleting a bounded, known ASCII value.
        let probe = "X"
        editor.coordinate(withNormalizedOffset: CGVector(dx: 0.9, dy: 0.8)).tap()
        editor.typeText(probe)
        guard waitForValue(expectedOriginal + probe, in: editor, timeout: 2) else {
            XCTFail("The end-of-text probe did not append exactly. The edit was not saved.")
            return false
        }
        editor.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: expectedOriginal.count + probe.count))
        guard waitForEmptyValue(in: editor, timeout: 2) else {
            XCTFail("The bounded delete sequence did not clear the complete editor. The edit was not saved.")
            return false
        }
        editor.typeText(replacement)
        guard waitForValue(replacement, in: editor, timeout: 2) else {
            XCTFail("The editor did not contain the exact replacement. The edit was not saved.")
            return false
        }
        return editor.value as? String == replacement
    }

    private func isPrintableASCII(_ value: String) -> Bool {
        !value.isEmpty && value.unicodeScalars.allSatisfy { $0.isASCII && (0x20...0x7e).contains($0.value) }
    }

    private func waitForValue(_ value: String, in editor: XCUIElement, timeout: TimeInterval) -> Bool {
        let predicate = NSPredicate(format: "value == %@", value)
        let expectation = XCTNSPredicateExpectation(predicate: predicate, object: editor)
        return XCTWaiter.wait(for: [expectation], timeout: timeout) == .completed
    }

    private func waitForEmptyValue(in editor: XCUIElement, timeout: TimeInterval) -> Bool {
        // A cleared SwiftUI TextEditor can omit its AX value instead of exposing an empty string.
        let predicate = NSPredicate(format: "value == nil OR value == ''")
        let expectation = XCTNSPredicateExpectation(predicate: predicate, object: editor)
        return XCTWaiter.wait(for: [expectation], timeout: timeout) == .completed
    }

    private func reveal(_ element: XCUIElement, in app: XCUIApplication) -> Bool {
        if element.waitForExistence(timeout: 2), element.isHittable { return true }
        for _ in 0..<8 {
            app.swipeUp()
            if element.waitForExistence(timeout: 1), element.isHittable { return true }
        }
        return false
    }

    private func revealBySwipingDown(_ element: XCUIElement, in app: XCUIApplication) -> Bool {
        if element.waitForExistence(timeout: 2), element.isHittable { return true }
        for _ in 0..<8 {
            app.swipeDown()
            if element.waitForExistence(timeout: 1), element.isHittable { return true }
        }
        return false
    }
}
