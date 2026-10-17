import Foundation
import XCTest

final class ConnectorUITests: XCTestCase {
    override func setUpWithError() throws {
        continueAfterFailure = false
        XCUIDevice.shared.orientation = .portrait
    }

    func testIsolatedStoppedAgentVisibilityAndOutput() throws {
        let app = XCUIApplication()
        app.launchArguments.append("--deck-diagnostic")
        app.launchArguments += ["-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
        for round in 1...3 {
            app.launch()
            let target = app.staticTexts["connector transport"].firstMatch
            _ = app.staticTexts["Tasks"].waitForExistence(timeout: 20)
            XCTAssertTrue(reveal(target, in: app), "Round \(round): saved agent card must appear.")
            XCTAssertEqual(app.staticTexts["connector ordinary shell"].exists,
                           ProcessInfo.processInfo.environment["DECK_DIAG_SCOPE"] != "agentCards",
                           "Round \(round): list must match the locally authorized scope.")
            target.tap()
            let unavailable = app.staticTexts["deck.output.unavailable"].firstMatch
            XCTAssertTrue(reveal(unavailable, in: app),
                          "Round \(round): stopped output area must be reachable in the normal detail layout.")
            XCTAssertEqual(unavailable.label, Self.sessionUnavailableLabel,
                           "Round \(round): missing session must have its own output state.")
            app.segmentedControls.buttons["Notes"].tap()
            XCTAssertTrue(revealBySwipingDown(app.textViews["deck.note.new"].firstMatch, in: app),
                          "Scratchpad remains available without a readable session.")
            app.segmentedControls.buttons["Output"].tap()
            let send = app.buttons["deck.message.send"].firstMatch
            XCTAssertTrue(reveal(send, in: app), "The send control must remain reachable.")
            XCTAssertFalse(send.isEnabled,
                           "Missing session must not allow sending.")
            XCTAssertTrue(reveal(unavailable, in: app), "The output state must remain reachable.")
            let attachment = XCTAttachment(screenshot: app.screenshot())
            attachment.name = "Stopped agent round \(round)"
            attachment.lifetime = .keepAlways
            add(attachment)
            if round == 1 {
                XCUIDevice.shared.press(.home)
                app.activate()
                XCTAssertTrue(reveal(unavailable, in: app), "The output state must survive foregrounding.")
            }
            app.terminate()
        }
    }

    func testIsolatedLiveCodexOutput() throws {
        guard let marker = ProcessInfo.processInfo.environment["DECK_DIAG_EXPECTED_MARKER"],
              marker.hasPrefix("deckprobe") else {
            throw XCTSkip("The isolated real Codex did not produce its expected marker.")
        }
        let app = XCUIApplication()
        app.launchArguments += ["--deck-diagnostic", "-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
        app.launch()
        openCard(named: "connector live codex", in: app)
        let output = app.descendants(matching: .any)["deck.output.text"].firstMatch
        if !reveal(output, in: app) {
            let unavailable = app.staticTexts["deck.output.unavailable"].firstMatch
            if unavailable.exists {
                XCTFail("Real Codex foreground/output failed: \(unavailable.label)")
                return
            }
            XCTFail("Live Codex card did not show output or an error.")
            return
        }
        XCTAssertTrue(app.buttons["deck.output.refresh"].exists)
        XCTAssertTrue(app.descendants(matching: .any)["deck.output.captured"].firstMatch.exists)
        XCTAssertTrue(output.label.contains(marker),
                      "The pinned host output must contain the isolated Agent's response marker.")
        let captured = app.descendants(matching: .any)["deck.output.captured"].firstMatch
        let previousCapture = captured.label
        XCUIDevice.shared.press(.home)
        Thread.sleep(forTimeInterval: 2)
        app.activate()
        let refreshed = XCTNSPredicateExpectation(predicate: NSPredicate(format: "label != %@", previousCapture), object: captured)
        XCTAssertEqual(XCTWaiter.wait(for: [refreshed], timeout: 20), .completed,
                       "Foregrounding must recapture the currently open card without a manual refresh.")
        XCTAssertTrue(output.label.contains(marker))
        let attachment = XCTAttachment(screenshot: app.screenshot())
        attachment.name = "Live output first screen"; attachment.lifetime = .keepAlways; add(attachment)
        app.terminate()
    }

    func testIsolatedCodexExitToShellRejectsOutputAndSend() throws {
        guard let marker = ProcessInfo.processInfo.environment["DECK_DIAG_EXITED_MARKER"],
              marker.hasPrefix("shellmarker") else {
            throw XCTSkip("The isolated Agent-to-shell transition was not prepared.")
        }
        let app = XCUIApplication()
        app.launchArguments += ["--deck-diagnostic", "-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
        app.launch()
        openCard(named: "connector live codex", in: app)
        let draft = app.textViews["deck.message.draft"].firstMatch
        XCTAssertTrue(reveal(draft, in: app), "The normal message composer must be reachable.")
        draft.tap()
        draft.typeText("unsent-shell-test")
        let done = app.buttons["deck.keyboard.done"].firstMatch
        XCTAssertTrue(done.waitForExistence(timeout: 5), "The normal keyboard toolbar must offer Done.")
        done.tap()
        let send = app.buttons["deck.message.send"].firstMatch
        XCTAssertTrue(reveal(send, in: app), "The send control must be reachable.")
        XCTAssertFalse(send.isEnabled, "An Agent card whose foreground returned to shell must not allow sending.")
        if ProcessInfo.processInfo.environment["DECK_DIAG_SCOPE"] == "boardAccess" {
            let output = app.descendants(matching: .any)["deck.output.text"].firstMatch
            XCTAssertTrue(reveal(output, in: app), "Board Access must keep the card's bounded output readable after Agent exit.")
            XCTAssertTrue(output.label.contains(marker), "The post-Agent shell marker must appear in Board Access output.")
        } else {
            let unavailable = app.staticTexts["deck.output.unavailable"].firstMatch
            XCTAssertTrue(reveal(unavailable, in: app), "The normal output area must be reachable after Agent exit.")
            XCTAssertEqual(unavailable.label,
                           "Output unavailable: A supported agent is not in the foreground, so its output cannot be read.")
            XCTAssertFalse(app.descendants(matching: .any)["deck.output.text"].exists,
                           "The host must not return the shell's \(marker) output as Agent output.")
        }
        app.terminate()
    }

    func testIsolatedSharedShellIsMetadataOnly() throws {
        let app = XCUIApplication()
        app.launchArguments += ["--deck-diagnostic", "-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
        app.launch()
        openCard(named: "connector ordinary shell", in: app)
        let unavailable = app.staticTexts["deck.output.unavailable"].firstMatch
        XCTAssertTrue(unavailable.waitForExistence(timeout: 10))
        // Summaries-only sharing names the Mac action that would add output.
        XCTAssertTrue(unavailable.label.contains("are not shared") && unavailable.label.contains("“Trust this device…”"),
                      unavailable.label)
        XCTAssertFalse(app.descendants(matching: .any)["deck.output.text"].exists)
        XCTAssertFalse(app.buttons["deck.message.send"].isEnabled)
        app.segmentedControls.buttons["Notes"].tap()
        XCTAssertFalse(app.buttons["deck.note.save"].isEnabled)
        app.terminate()
    }

    func testIsolatedBoardAccessManualAgentAndShell() throws {
        let environment = ProcessInfo.processInfo.environment
        guard environment["DECK_DIAG_SCOPE"] == "boardAccess" else {
            throw XCTSkip("Run the isolated diagnostic with Board Access sharing.")
        }
        guard let marker = environment["DECK_DIAG_EXPECTED_MARKER"], marker.hasPrefix("deckprobe") else {
            throw XCTSkip("The isolated real Codex did not produce its expected marker.")
        }

        let app = XCUIApplication()
        app.launchArguments += ["--deck-diagnostic", "-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
        app.launch()

        openCard(named: "connector ordinary shell", in: app)
        let shellOutput = app.descendants(matching: .any)["deck.output.text"].firstMatch
        XCTAssertTrue(reveal(shellOutput, in: app), "Board Access must expose the shell card's bounded output snapshot.")
        XCTAssertFalse(app.staticTexts["deck.output.unavailable"].exists)

        app.segmentedControls.buttons["Notes"].tap()
        let shellNote = app.textViews["deck.note.new"].firstMatch
        XCTAssertTrue(revealBySwipingDown(shellNote, in: app), "Board Access must expose the shell card's scratchpad.")
        XCTAssertTrue(shellNote.isEnabled)
        let noteProbe = "board-access-note-" + UUID().uuidString
        shellNote.tap(); shellNote.typeText(noteProbe)
        app.buttons["deck.keyboard.done"].tap()
        let saveNote = app.buttons["deck.note.save"].firstMatch
        XCTAssertTrue(reveal(saveNote, in: app) && saveNote.isEnabled,
                      "A shell note must be editable without granting queue access.")
        XCTAssertTrue(app.staticTexts["Queueing requires a Codex or Claude launch configuration saved on the desktop."].exists,
                      "Queueing must still name its saved-launch requirement.")
        saveNote.tap()
        let savedNote = noteText(noteProbe, in: app)
        if !savedNote.waitForExistence(timeout: 12) { checkOriginalNoteOperationOnce(in: app) }
        XCTAssertTrue(savedNote.waitForExistence(timeout: 12),
                      "A Board Access shell note must round-trip through the desktop scratchpad.")
        let shellAttachment = XCTAttachment(screenshot: app.screenshot())
        shellAttachment.name = "Board Access shell note saved"; shellAttachment.lifetime = .keepAlways; add(shellAttachment)
        guard deleteOnlyNote(noteProbe, in: app) else { return }

        app.segmentedControls.buttons["Output"].tap()
        let shellDraft = app.textViews["deck.message.draft"].firstMatch
        XCTAssertTrue(reveal(shellDraft, in: app))
        let messageProbe = "board-access-message-probe"
        shellDraft.tap(); shellDraft.typeText(messageProbe)
        app.buttons["deck.keyboard.done"].tap()
        let shellSend = app.buttons["deck.message.send"].firstMatch
        XCTAssertTrue(reveal(shellSend, in: app))
        XCTAssertFalse(shellSend.isEnabled, "Board Access must not turn shell input into a phone message target.")
        guard replaceAllText(in: shellDraft, expectedOriginal: messageProbe, with: "") else { return }
        app.buttons["deck.keyboard.done"].tap()
        XCTAssertTrue(waitForEmptyValue(in: shellDraft, timeout: 3))

        app.navigationBars.buttons.element(boundBy: 0).tap()
        openCard(named: "connector live codex", in: app)
        let agentOutput = app.descendants(matching: .any)["deck.output.text"].firstMatch
        XCTAssertTrue(reveal(agentOutput, in: app))
        XCTAssertTrue(agentOutput.label.contains(marker),
                      "The manually started verified Codex must expose its bounded output snapshot.")
        let agentDraft = app.textViews["deck.message.draft"].firstMatch
        XCTAssertTrue(reveal(agentDraft, in: app))
        let responseSuffix = UUID().uuidString.replacingOccurrences(of: "-", with: "").lowercased()
        let responseMarker = "phoneaccess" + responseSuffix
        let prompt = "Reply only with the concatenation of these two chunks, without spaces: phoneaccess and \(responseSuffix)"
        agentDraft.tap(); agentDraft.typeText(prompt)
        app.buttons["deck.keyboard.done"].tap()
        let agentSend = app.buttons["deck.message.send"].firstMatch
        XCTAssertTrue(reveal(agentSend, in: app) && agentSend.isEnabled,
                      "A current verified Codex foreground remains directly writable after its saved command is cleared.")
        agentSend.tap()
        XCTAssertTrue(waitForEmptyValue(in: agentDraft, timeout: 20),
                      "A confirmed direct send must clear the exact submitted draft.")
        let response = XCTNSPredicateExpectation(predicate: NSPredicate { _, _ in
            agentOutput.label.contains(responseMarker)
        }, object: nil)
        XCTAssertEqual(XCTWaiter.wait(for: [response], timeout: 45), .completed,
                       "Auto-refresh must show the concatenated marker that was absent from the submitted prompt.")

        let messageAttachment = XCTAttachment(screenshot: app.screenshot())
        messageAttachment.name = "Board Access manual Agent reply"; messageAttachment.lifetime = .keepAlways; add(messageAttachment)

        app.segmentedControls.buttons["Notes"].tap()
        XCTAssertTrue(app.staticTexts["Queueing requires a Codex or Claude launch configuration saved on the desktop."].waitForExistence(timeout: 10),
                      "The manually started agent must not gain queued delivery without a saved launch configuration.")
        app.terminate()
    }

    func testIsolatedDraftsSurviveNavigationAndRelaunch() throws {
        let app = XCUIApplication()
        app.launchArguments += ["--deck-diagnostic", "-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
        app.launch()
        openCard(named: "connector transport", in: app)
        selectDetailTab("Notes", in: app)
        let note = app.textViews["deck.note.new"].firstMatch
        XCTAssertTrue(revealBySwipingDown(note, in: app))
        let marker = "local-note-" + UUID().uuidString
        note.tap(); note.typeText(marker)
        app.buttons["deck.keyboard.done"].tap()
        app.navigationBars.buttons.element(boundBy: 0).tap()
        openCard(named: "connector transport", in: app)
        selectDetailTab("Notes", in: app)
        XCTAssertEqual(note.value as? String, marker)
        app.terminate(); app.launch()
        openCard(named: "connector transport", in: app)
        selectDetailTab("Notes", in: app)
        XCTAssertTrue(revealBySwipingDown(note, in: app))
        XCTAssertEqual(note.value as? String, marker, "An unsaved note must survive app termination without a desktop write.")
        let saveNew = app.buttons["deck.note.save"]
        XCTAssertTrue(reveal(saveNew, in: app)); saveNew.tap()
        XCTAssertTrue(revealBySwipingDown(note, in: app), "Bring the editor back onscreen before checking its cleared value.")
        XCTAssertTrue(waitForEmptyValue(in: note, timeout: 20),
                       "A restored note must save without needing another edit.")
        func openSavedNoteEditor() throws {
            XCTAssertTrue(reveal(noteText(marker, in: app), in: app))
            let row = try XCTUnwrap(noteRow(marker, in: app))
            let edit = row.buttons["deck.note.edit"]
            XCTAssertTrue(reveal(edit, in: app)); edit.tap()
        }
        try openSavedNoteEditor()
        let editor = app.textViews["deck.note.edit.text"].firstMatch
        XCTAssertTrue(editor.waitForExistence(timeout: 5))
        editor.tap(); editor.typeText(" unsaved-edit")
        let edited = editor.value as? String
        app.buttons["Cancel"].firstMatch.tap()
        try openSavedNoteEditor()
        XCTAssertEqual(editor.value as? String, edited, "Closing an edit must preserve its local draft.")
        app.terminate(); app.launch()
        openCard(named: "connector transport", in: app)
        selectDetailTab("Notes", in: app)
        try openSavedNoteEditor()
        XCTAssertTrue(editor.waitForExistence(timeout: 5))
        XCTAssertEqual(editor.value as? String, edited)
        app.buttons["deck.note.edit.save"].tap()
        let savedEdit = XCTNSPredicateExpectation(predicate: NSPredicate(format: "exists == false"), object: editor)
        XCTAssertEqual(XCTWaiter.wait(for: [savedEdit], timeout: 20), .completed,
                       "A restored edit must save without needing another edit.")
        if let edited { XCTAssertTrue(reveal(app.staticTexts[edited].firstMatch, in: app)) }
        app.terminate()
    }

    func testIsolatedChineseLargeText() throws {
        defer { XCUIDevice.shared.orientation = .portrait }
        let app = XCUIApplication()
        app.launchArguments += ["--deck-diagnostic", "-AppleLanguages", "(zh-Hans)", "-AppleLocale", "zh_CN", "-UIPreferredContentSizeCategoryName", "UICTContentSizeCategoryAccessibilityXXXL"]
        app.launch()
        let card = app.staticTexts["connector transport"].firstMatch
        _ = card.waitForExistence(timeout: 15)
        XCTAssertTrue(reveal(card, in: app))
        let listAttachment = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        listAttachment.name = "Chinese large text card list"
        listAttachment.lifetime = .keepAlways; add(listAttachment)
        openCard(named: "connector transport", in: app)
        XCTAssertTrue(app.buttons["deck.output.refresh"].waitForExistence(timeout: 10))
        XCTAssertEqual(app.buttons["deck.output.refresh"].label, "刷新输出")
        let unavailable = app.staticTexts["deck.output.unavailable"].firstMatch
        XCTAssertTrue(reveal(unavailable, in: app))
        XCTAssertTrue(unavailable.label.contains("此卡片目前没有可读取的会话"))
        for orientation in [UIDeviceOrientation.portrait, .landscapeLeft] {
            XCUIDevice.shared.orientation = orientation
            let landscape = orientation == .landscapeLeft
            let settled = XCTNSPredicateExpectation(predicate: NSPredicate { _, _ in
                let frame = app.windows.firstMatch.frame
                return (frame.width > frame.height) == landscape
            }, object: nil)
            XCTAssertEqual(XCTWaiter.wait(for: [settled], timeout: 10), .completed,
                           "Wait for the window to finish changing orientation.")
            XCTAssertTrue(reveal(app.buttons["deck.output.refresh"], in: app))
            XCTAssertTrue(reveal(unavailable, in: app))
            let screenshot = XCUIScreen.main.screenshot()
            XCTAssertEqual(screenshot.image.size.width > screenshot.image.size.height, landscape,
                           "The visual evidence must have the settled screen orientation.")
            let attachment = XCTAttachment(screenshot: screenshot)
            attachment.name = "Chinese large text \(orientation.rawValue)"
            attachment.lifetime = .keepAlways; add(attachment)
        }
        XCUIDevice.shared.orientation = .portrait
        app.terminate()
    }

    private static let sessionUnavailableLabel = "Output unavailable: There is no readable session for this card right now."

    func testPairedCardOutputSurvivesForegroundCycle() throws {
        let environment = ProcessInfo.processInfo.environment
        guard environment["DECK_UI_PAIRED_SMOKE"] == "1" else {
            throw XCTSkip("Set DECK_UI_PAIRED_SMOKE=1 to run against an already paired device.")
        }
        let cardTitle = environment["DECK_UI_CARD_TITLE"]?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        guard !cardTitle.isEmpty else { throw XCTSkip("DECK_UI_CARD_TITLE is required.") }
        let expectedOutput = environment["DECK_UI_EXPECTED_OUTPUT"]?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        guard !expectedOutput.isEmpty else { throw XCTSkip("DECK_UI_EXPECTED_OUTPUT is required.") }

        let app = pairedApp()
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

        let app = pairedApp()
        app.launch()
        openCard(named: cardTitle, in: app)
        app.segmentedControls.buttons["Notes"].tap()
        XCTAssertTrue(
            noteText(expectedNote, in: app).waitForExistence(timeout: 15),
            "Expected the paired scratchpad note before process termination."
        )

        app.terminate()
        app.launch()
        openCard(named: cardTitle, in: app)
        app.segmentedControls.buttons["Notes"].tap()
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

        let app = pairedApp()
        app.launch()
        openCard(named: cardTitle, in: app)
        app.segmentedControls.buttons["Notes"].tap()
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

        let app = pairedApp()
        app.launch()
        openCard(named: cardTitle, in: app)
        app.segmentedControls.buttons["Notes"].tap()
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

        let app = pairedApp()
        app.launch()
        openCard(named: cardTitle, in: app)
        app.segmentedControls.buttons["Notes"].tap()
        let bufferLoaded = app.buttons["deck.note.queue-selected"]
        XCTAssertTrue(bufferLoaded.waitForExistence(timeout: 15), "Expected the scratchpad buffer to finish loading before finding the marker.")
        guard deleteOnlyNote(marker, in: app) else { return }
    }

    /// The paired tests address English labels; DECK_UI_LANGUAGE=en pins the
    /// app language when the device itself uses another one.
    private func pairedApp() -> XCUIApplication {
        let app = XCUIApplication()
        if ProcessInfo.processInfo.environment["DECK_UI_LANGUAGE"] == "en" {
            app.launchArguments += ["-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
        }
        return app
    }

    private func selectDetailTab(_ title: String, in app: XCUIApplication) {
        let tab = app.segmentedControls.buttons[title]
        XCTAssertTrue(revealBySwipingDown(tab, in: app), "The detail tab must be visible after navigation settles.")
        tab.tap()
        let selected = XCTNSPredicateExpectation(predicate: NSPredicate(format: "selected == true"), object: tab)
        XCTAssertEqual(XCTWaiter.wait(for: [selected], timeout: 5), .completed,
                       "The requested tab must actually be selected before looking for its lazy List cells.")
    }

    private func openCard(named title: String, in app: XCUIApplication) {
        let card = app.staticTexts[title].firstMatch
        _ = card.waitForExistence(timeout: 15)
        XCTAssertTrue(reveal(card, in: app), "Expected reachable paired test card named \(title).")
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
        guard isPrintableASCII(expectedOriginal), replacement.isEmpty || isPrintableASCII(replacement) else {
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
        if replacement.isEmpty { return true }
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
        reveal(element, in: app, searchDown: false)
    }

    private func revealBySwipingDown(_ element: XCUIElement, in app: XCUIApplication) -> Bool {
        reveal(element, in: app, searchDown: true)
    }

    private func visible(_ element: XCUIElement, in app: XCUIApplication) -> Bool {
        guard element.exists else { return false }
        let frame = element.frame
        guard !frame.isNull, !frame.isInfinite, frame.width > 0, frame.height > 0 else { return false }
        var window = app.windows.firstMatch.frame
        if app.navigationBars.firstMatch.exists {
            let top = max(window.minY, app.navigationBars.firstMatch.frame.maxY)
            window = CGRect(x: window.minX, y: top, width: window.width, height: max(0, window.maxY - top))
        }
        // Static labels have no activation target. Validate their visible geometry
        // instead of asking XCTest for an interaction point during foregrounding.
        // A long terminal snapshot can extend beyond its own scroll viewport.
        // Reading it requires a visible portion, not its offscreen center point.
        if element.elementType == .staticText {
            let portion = frame.intersection(window)
            return !portion.isNull && portion.width > 0 && portion.height >= min(24, frame.height)
        }
        let center = CGPoint(x: frame.midX, y: frame.midY)
        guard window.contains(center) else { return false }
        return element.isHittable
    }

    private func reveal(_ element: XCUIElement, in app: XCUIApplication, searchDown: Bool) -> Bool {
        _ = element.waitForExistence(timeout: 2)
        if visible(element, in: app) { return true }
        // List cells leave the accessibility tree when scrolled offscreen. A
        // missing cell does not tell us which side it is on, particularly after
        // rotation changes the viewport. Search both directions, boundedly.
        for down in [searchDown, !searchDown] {
            for _ in 0..<8 {
                let list = app.collectionViews.firstMatch
                let surface = list.exists ? list : app
                let window = app.windows.firstMatch
                if window.frame.width > window.frame.height {
                    // A full inertial swipe can skip an entire accessible row
                    // in the short landscape viewport. Drag a quarter-screen
                    // and hold before releasing so every intervening row is read.
                    let start = window.coordinate(withNormalizedOffset: CGVector(dx: 0.96, dy: down ? 0.47 : 0.72))
                    let end = window.coordinate(withNormalizedOffset: CGVector(dx: 0.96, dy: down ? 0.72 : 0.47))
                    start.press(forDuration: 0.05, thenDragTo: end, withVelocity: .slow, thenHoldForDuration: 0.2)
                } else if down { surface.swipeDown() } else { surface.swipeUp() }
                _ = element.waitForExistence(timeout: 1)
                if visible(element, in: app) { return true }
            }
        }
        return false
    }

}

// Physical-device acceptance against an isolated Deck host on the private LAN.
// Every test is skipped unless DECK_DEVICE_ACCEPTANCE=1 is injected into this
// runner; the external driver (scripts/connector-device-acceptance.py) pairs
// through the product's paste field, watches the `DECKSTEP <name>` activities
// below in the live xcodebuild log, and answers by changing the isolated host
// through its production Settings handlers. The phone side waits only on what
// the app itself shows. Nothing here writes Keychain or seeds a snapshot.
extension ConnectorUITests {
    private var deviceEnvironment: [String: String] { ProcessInfo.processInfo.environment }

    private func requireDeviceAcceptance(_ keys: [String] = []) throws -> [String: String] {
        let environment = deviceEnvironment
        guard environment["DECK_DEVICE_ACCEPTANCE"] == "1" else {
            throw XCTSkip("Set DECK_DEVICE_ACCEPTANCE=1 in the copied device test plan.")
        }
        for key in keys where (environment[key] ?? "").isEmpty {
            XCTFail("\(key) is required for the device acceptance run.")
        }
        return environment
    }

    private func chineseApp(largeText: Bool = false) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments += ["-AppleLanguages", "(zh-Hans)", "-AppleLocale", "zh_CN"]
        if largeText { app.launchArguments += ["-UIPreferredContentSizeCategoryName", "UICTContentSizeCategoryAccessibilityXL"] }
        return app
    }

    private func deckStep(_ name: String) {
        XCTContext.runActivity(named: "DECKSTEP \(name)") { _ in }
    }

    private func keepScreenshot(_ name: String, _ app: XCUIApplication) {
        let attachment = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    /// Answers a system permission alert by one of its localized button titles.
    @discardableResult
    private func answerSystemAlert(_ titles: [String], timeout: TimeInterval = 8) -> String? {
        let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        let alert = springboard.alerts.firstMatch
        guard alert.waitForExistence(timeout: timeout) else { return nil }
        for title in titles {
            let button = alert.buttons[title]
            if button.exists { button.tap(); return title }
        }
        XCTFail("Unexpected system alert buttons: \(alert.buttons.allElementsBoundByIndex.map(\.label))")
        return nil
    }

    private func waitUntil(_ timeout: TimeInterval, _ condition: @escaping () -> Bool) -> Bool {
        let expectation = XCTNSPredicateExpectation(predicate: NSPredicate { _, _ in condition() }, object: nil)
        return XCTWaiter.wait(for: [expectation], timeout: timeout) == .completed
    }

    private func outputText(_ app: XCUIApplication) -> XCUIElement {
        app.descendants(matching: .any)["deck.output.text"].firstMatch
    }

    private func pairField(_ app: XCUIApplication) -> XCUIElement {
        let identified = app.textFields["deck.pair.descriptor"]
        return identified.exists ? identified : app.textFields.firstMatch
    }

    /// Camera denial first, then the product's paste entry and the system
    /// Local Network prompt. Works for the older build too (label fallback).
    private func pairByPaste(_ uri: String, host: String, in app: XCUIApplication, cameraDenial: Bool) {
        let scan = app.buttons["deck.pair.scan"].exists ? app.buttons["deck.pair.scan"] : app.buttons["扫描二维码"]
        XCTAssertTrue(scan.waitForExistence(timeout: 20), "The unpaired app must offer pairing.")
        keepScreenshot("unpaired", app)
        if cameraDenial {
            scan.tap()
            let answered = answerSystemAlert(["不允许", "Don’t Allow", "Don't Allow"])
            deckStep("camera-alert-\(answered == nil ? "absent" : "denied")")
            let unavailable = app.staticTexts.matching(NSPredicate(format: "label CONTAINS[c] %@ OR label CONTAINS %@", "Camera access", "相机")).firstMatch
            XCTAssertTrue(unavailable.waitForExistence(timeout: 8), "A denied camera must explain the paste alternative.")
            keepScreenshot("camera-denied", app)
            app.buttons["取消"].firstMatch.tap()
        }
        let field = pairField(app)
        XCTAssertTrue(field.waitForExistence(timeout: 10))
        field.tap()
        field.typeText(uri)
        let submit = app.buttons["deck.pair.submit"].exists ? app.buttons["deck.pair.submit"] : app.buttons["配对"]
        XCTAssertTrue(submit.isEnabled)
        submit.tap()
        let localNetwork = answerSystemAlert(["允许", "Allow"], timeout: 15)
        deckStep("local-network-\(localNetwork == nil ? "no-prompt" : "allowed")")
        XCTAssertTrue(app.staticTexts[host].firstMatch.waitForExistence(timeout: 45),
                      "Pairing must reach the isolated host over the private LAN and show its name.")
    }

    private func pairingScan(_ app: XCUIApplication) -> XCUIElement {
        app.buttons["deck.pair.scan"].exists ? app.buttons["deck.pair.scan"] : app.buttons["扫描二维码"]
    }

    /// iOS keeps an app's Keychain item across uninstall, so a reinstalled
    /// test variant can wake up with an earlier run's pairing. Remove it the
    /// way a user would (revoked screen or menu → 移除配对 → confirm).
    /// Returns whether a pairing was present.
    @discardableResult
    private func removePairingIfPresent(_ app: XCUIApplication) -> Bool {
        if pairingScan(app).waitForExistence(timeout: 6) { return false }
        answerSystemAlert(["允许", "Allow"], timeout: 3)
        let remove = app.buttons["移除配对"].firstMatch
        if !remove.waitForExistence(timeout: 5) {
            let menu = app.navigationBars.buttons.allElementsBoundByIndex.last
            XCTAssertNotNil(menu, "The paired list must offer its menu."); menu?.tap()
            XCTAssertTrue(remove.waitForExistence(timeout: 5), "The menu must offer Remove pairing.")
        }
        remove.tap()
        let confirm = app.sheets.buttons["移除配对"].exists ? app.sheets.buttons["移除配对"] : app.buttons.matching(identifier: "移除配对").element(boundBy: max(0, app.buttons.matching(identifier: "移除配对").count - 1))
        XCTAssertTrue(confirm.waitForExistence(timeout: 5)); confirm.tap()
        XCTAssertTrue(pairingScan(app).waitForExistence(timeout: 15), "Removing the pairing must return to the pairing screen.")
        return true
    }

    // Z1: cleanup — removes this test variant's pairing through the app so no
    // credential is left in the phone's Keychain when the variant is uninstalled.
    func testDeviceZ1RemovePairing() throws {
        _ = try requireDeviceAcceptance()
        let app = chineseApp()
        app.launch()
        let removed = removePairingIfPresent(app)
        deckStep(removed ? "pairing-removed" : "no-pairing")
        XCTAssertTrue(pairingScan(app).exists)
        keepScreenshot("unpaired-for-cleanup", app)
        app.terminate()
    }

    // A0: proves the runner controls the app on this phone (launch, query,
    // screenshot) before any pairing exists.
    func testDeviceA0ControlLaunch() throws {
        _ = try requireDeviceAcceptance()
        let app = chineseApp()
        app.launch()
        let scan = app.buttons["deck.pair.scan"].exists ? app.buttons["deck.pair.scan"] : app.buttons["扫描二维码"]
        XCTAssertTrue(scan.waitForExistence(timeout: 20), "A fresh test variant must open on the pairing screen.")
        keepScreenshot("control-launch", app)
        deckStep("control-ok")
        app.terminate()
    }

    // R1: the published 1.0 (3) source, built as the test variant, against the
    // kept summaries-only scope reproduces the field screenshot exactly.
    func testDeviceR1OldBuildReproducesFieldDenial() throws {
        let environment = try requireDeviceAcceptance(["DECK_DEVICE_PAIRING_URI", "DECK_DEVICE_HOST", "DECK_DEVICE_AGENT_TITLE"])
        let app = chineseApp()
        app.launch()
        deckStep("control-ok")
        if removePairingIfPresent(app) { deckStep("stale-pairing-removed") }
        pairByPaste(environment["DECK_DEVICE_PAIRING_URI"]!, host: environment["DECK_DEVICE_HOST"]!, in: app, cameraDenial: true)
        let legacy = "此卡片未共享这项访问权限。请检查 Mac 上的 Phone Connector 共享范围。"
        let rowReason = app.staticTexts[legacy].firstMatch
        XCTAssertTrue(rowReason.waitForExistence(timeout: 20), "The old build must show the field's generic denial on the list.")
        keepScreenshot("old-build-list", app)
        openCard(named: environment["DECK_DEVICE_AGENT_TITLE"]!, in: app)
        let unavailable = app.staticTexts["deck.output.unavailable"].firstMatch
        XCTAssertTrue(reveal(unavailable, in: app))
        XCTAssertTrue(unavailable.label.contains(legacy), "Old detail: \(unavailable.label)")
        let send = app.buttons["deck.message.send"].firstMatch
        XCTAssertTrue(reveal(send, in: app))
        XCTAssertFalse(send.isEnabled)
        keepScreenshot("old-build-detail", app)
        deckStep("old-build-reproduced")
    }

    // U1: the new build installed over the paired old build keeps the pairing,
    // names the exact Mac setting, converges after the explicit grant without
    // re-pairing, and delivers a phone message to a manually started Claude.
    func testDeviceU1UpgradeGuidesGrantAndDelivers() throws {
        let environment = try requireDeviceAcceptance(["DECK_DEVICE_HOST", "DECK_DEVICE_AGENT_TITLE", "DECK_DEVICE_AGENT_MARKER", "DECK_DEVICE_NONCE"])
        let host = environment["DECK_DEVICE_HOST"]!
        let agent = environment["DECK_DEVICE_AGENT_TITLE"]!
        let app = chineseApp()
        app.launch()
        XCTAssertTrue(app.staticTexts[host].firstMatch.waitForExistence(timeout: 30), "Upgrade must keep the existing pairing.")
        XCTAssertFalse(app.textFields["deck.pair.descriptor"].exists, "Upgrade must not ask to pair again.")
        let setup = app.staticTexts["deck.sharing.setup"].firstMatch
        XCTAssertTrue(setup.waitForExistence(timeout: 20), "The kept summaries-only scope must lead with setup guidance.")
        XCTAssertTrue(setup.label.contains("「\(host)」") && setup.label.contains("「信任此设备」") && setup.label.contains("「设置 → 远程访问」")
                      && setup.label.contains("下次回到") && setup.label.contains("无需重新配对"),
                      "Setup must name the host and the exact desktop action: \(setup.label)")
        // Compact enough that the first card stays on the first screen.
        XCTAssertTrue(app.staticTexts[environment["DECK_DEVICE_AGENT_TITLE"]!].firstMatch.isHittable,
                      "The setup note must not push the cards off the first screen.")
        XCTAssertTrue(app.staticTexts["输出和消息尚未共享"].firstMatch.exists)
        XCTAssertTrue(app.staticTexts["输出未共享"].firstMatch.exists, "Each affected card carries one short marker.")
        keepScreenshot("new-build-list-setup", app)
        // Keeping the current sharing collapses the guidance for this host and
        // scope across relaunch; it can be shown again.
        app.buttons["deck.sharing.keep"].tap()
        let collapsed = app.staticTexts["deck.sharing.collapsed"].firstMatch
        XCTAssertTrue(collapsed.waitForExistence(timeout: 5)); XCTAssertFalse(setup.exists)
        app.terminate(); app.launch()
        XCTAssertTrue(collapsed.waitForExistence(timeout: 30), "The kept choice survives relaunch.")
        XCTAssertFalse(setup.exists)
        keepScreenshot("new-build-list-collapsed", app)
        app.buttons["deck.sharing.show"].tap()
        XCTAssertTrue(setup.waitForExistence(timeout: 5))
        app.descendants(matching: .any)["deck.sharing.details"].firstMatch.tap()
        // The disclosure's own identifier masks its rows' identifiers; match the visible text.
        let version = app.staticTexts.matching(NSPredicate(format: "label CONTAINS %@", "Deck Connector 1.0 (4)")).firstMatch
        XCTAssertTrue(version.waitForExistence(timeout: 5), "The phone shows its own build for diagnosis.")
        keepScreenshot("new-build-version", app)
        deckStep("upgrade-kept-pairing")

        openCard(named: agent, in: app)
        let unavailable = app.staticTexts["deck.output.unavailable"].firstMatch
        XCTAssertTrue(reveal(unavailable, in: app))
        XCTAssertTrue(unavailable.label.contains("「\(host)」") && unavailable.label.contains("「信任此设备」"), unavailable.label)
        let send = app.buttons["deck.message.send"].firstMatch
        XCTAssertTrue(reveal(send, in: app)); XCTAssertFalse(send.isEnabled)
        // Like the field screenshot: a draft typed while output is not shared.
        let draft = app.textViews["deck.message.draft"].firstMatch
        XCTAssertTrue(reveal(draft, in: app))
        clearDraft(draft, in: app)
        draft.tap(); draft.typeText("test")
        app.buttons["deck.keyboard.done"].tap()
        keepScreenshot("new-build-detail-before-grant", app)

        // The driver trusts this device on the Mac only after this step.
        deckStep("ready-trust")
        let output = outputText(app)
        XCTAssertTrue(waitUntil(60) { output.exists && output.label.contains(environment["DECK_DEVICE_AGENT_MARKER"]!) },
                      "The open detail must converge to the granted output without re-pairing or relaunch.")
        XCTAssertFalse(app.staticTexts["deck.output.unavailable"].exists)
        keepScreenshot("after-grant-output", app)
        deckStep("granted-output-visible")
        // The pre-grant draft never silently retargets: it asks for explicit acceptance.
        let unconfirmed = app.staticTexts["deck.message.targetChanged"].firstMatch
        XCTAssertTrue(reveal(unconfirmed, in: app))
        XCTAssertTrue(unconfirmed.label.contains("目标可确认之前"), unconfirmed.label)
        XCTAssertFalse(app.buttons["deck.message.send"].firstMatch.isEnabled)
        keepScreenshot("pre-grant-draft-needs-acceptance", app)
        app.buttons["使用当前目标"].firstMatch.tap()
        XCTAssertTrue(replaceAllText(in: draft, expectedOriginal: "test", with: ""))
        app.buttons["deck.keyboard.done"].tap()

        sendAndAwaitReply(nonce: environment["DECK_DEVICE_NONCE"]!, tag: "first", in: app)

        // Response loss: the host applies the send but answers 504. The phone
        // must confirm the original operation, never post a second one.
        deckStep("ready-fault")
        XCTAssertTrue(waitUntil(5) { true })
        sendAndAwaitReply(nonce: environment["DECK_DEVICE_NONCE"]! + "r", tag: "lost-response", in: app)
        app.terminate()
    }

    /// Removes a leftover durable draft of any content from an earlier run.
    private func clearDraft(_ draft: XCUIElement, in app: XCUIApplication) {
        guard let value = draft.value as? String, !value.isEmpty else { return }
        draft.tap()
        // The short editor shows only the first lines; delete from the last
        // visible caret position until nothing remains (bounded).
        for _ in 0..<6 {
            guard let remaining = draft.value as? String, !remaining.isEmpty else { break }
            draft.coordinate(withNormalizedOffset: CGVector(dx: 0.98, dy: 0.98)).tap()
            draft.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: remaining.count + 2))
        }
        XCTAssertTrue(waitForEmptyValue(in: draft, timeout: 3), "A leftover draft must be cleared before this run.")
        app.buttons["deck.keyboard.done"].tap()
    }

    private func sendAndAwaitReply(nonce: String, tag: String, in app: XCUIApplication) {
        let draft = app.textViews["deck.message.draft"].firstMatch
        XCTAssertTrue(reveal(draft, in: app))
        let first = "DQA"
        let expected = first + nonce
        draft.tap()
        draft.typeText("只回复把 \(first) 和 \(nonce) 直接拼接后的一个词，不要加空格，也不要调用任何工具。")
        app.buttons["deck.keyboard.done"].tap()
        let send = app.buttons["deck.message.send"].firstMatch
        XCTAssertTrue(reveal(send, in: app) && send.isEnabled, "A manually started Claude foreground must accept a phone message.")
        send.tap()
        deckStep("sent-\(tag)")
        XCTAssertTrue(waitForEmptyValue(in: draft, timeout: 45), "The send must be confirmed and its exact draft cleared (\(tag)).")
        let output = outputText(app)
        XCTAssertTrue(waitUntil(90) { output.exists && output.label.contains(expected) },
                      "The agent's reply must contain the concatenated marker absent from the prompt (\(tag)).")
        keepScreenshot("reply-\(tag)", app)
        deckStep("reply-\(tag)")
    }

    // S1: under Board access a plain shell's output is readable, sending stays
    // disabled, and a stopped card says so; then sharing changes take effect
    // on an open detail and regrant revives no old operation.
    func testDeviceS1ShellStoppedAndWithdrawal() throws {
        let environment = try requireDeviceAcceptance(["DECK_DEVICE_SHELL_TITLE", "DECK_DEVICE_SHELL_MARKER", "DECK_DEVICE_STOPPED_TITLE", "DECK_DEVICE_HOST"])
        let app = chineseApp()
        app.launch()
        openCard(named: environment["DECK_DEVICE_STOPPED_TITLE"]!, in: app)
        let stopped = app.staticTexts["deck.output.unavailable"].firstMatch
        XCTAssertTrue(reveal(stopped, in: app))
        XCTAssertTrue(stopped.label.contains("没有可读取的会话") || stopped.label.contains("会话"), stopped.label)
        let stoppedSend = app.buttons["deck.message.send"].firstMatch
        XCTAssertTrue(reveal(stoppedSend, in: app)); XCTAssertFalse(stoppedSend.isEnabled)
        keepScreenshot("stopped-card", app)
        app.navigationBars.buttons.element(boundBy: 0).tap()

        openCard(named: environment["DECK_DEVICE_SHELL_TITLE"]!, in: app)
        let output = outputText(app)
        XCTAssertTrue(waitUntil(30) { output.exists && output.label.contains(environment["DECK_DEVICE_SHELL_MARKER"]!) },
                      "Board access must expose a plain shell's bounded output.")
        let draft = app.textViews["deck.message.draft"].firstMatch
        XCTAssertTrue(reveal(draft, in: app))
        draft.tap(); draft.typeText("echo must-not-run")
        app.buttons["deck.keyboard.done"].tap()
        let send = app.buttons["deck.message.send"].firstMatch
        XCTAssertTrue(reveal(send, in: app)); XCTAssertFalse(send.isEnabled, "A phone must never send shell input.")
        keepScreenshot("shell-output-send-disabled", app)
        XCTAssertTrue(replaceAllText(in: draft, expectedOriginal: "echo must-not-run", with: ""))
        app.buttons["deck.keyboard.done"].tap()
        deckStep("shell-checked")

        deckStep("ready-downgrade")
        XCTAssertTrue(waitUntil(30) { !self.outputText(app).exists && app.staticTexts["deck.output.unavailable"].exists },
                      "Narrowing sharing must remove the open shell output promptly.")
        keepScreenshot("after-downgrade", app)
        deckStep("downgrade-observed")
        deckStep("ready-regrant")
        XCTAssertTrue(waitUntil(30) { self.outputText(app).exists }, "Regranting must restore output for the open card.")
        deckStep("regrant-observed")

        deckStep("ready-disable")
        XCTAssertTrue(waitUntil(30) { app.descendants(matching: .any).containing(NSPredicate(format: "label CONTAINS %@", "离线")).firstMatch.exists
                                      || app.images["wifi.slash"].exists },
                      "Disabling Connector must be visible as a connection problem, not as a sharing choice.")
        keepScreenshot("connector-disabled", app)
        deckStep("disable-observed")
        deckStep("ready-enable")
        XCTAssertTrue(waitUntil(45) { self.outputText(app).exists && !app.images["wifi.slash"].exists }, "Re-enabling must recover without re-pairing.")
        deckStep("enable-observed")
        app.terminate()
    }

    // E1: after the Agent exits to its shell, Board access still reads output
    // and sending is disabled.
    func testDeviceE1AgentExitToShell() throws {
        let environment = try requireDeviceAcceptance(["DECK_DEVICE_AGENT_TITLE", "DECK_DEVICE_EXIT_MARKER"])
        let app = chineseApp()
        app.launch()
        openCard(named: environment["DECK_DEVICE_AGENT_TITLE"]!, in: app)
        let output = outputText(app)
        XCTAssertTrue(waitUntil(45) { output.exists && output.label.contains(environment["DECK_DEVICE_EXIT_MARKER"]!) },
                      "Shell output after the Agent exited stays readable under Board access.")
        let send = app.buttons["deck.message.send"].firstMatch
        XCTAssertTrue(reveal(send, in: app)); XCTAssertFalse(send.isEnabled)
        keepScreenshot("agent-exited-to-shell", app)
        app.terminate()
    }

    // D1: message and note drafts survive navigation, background and process
    // termination; the restored note saves without another edit.
    func testDeviceD1DraftsSurviveNavigationBackgroundAndRelaunch() throws {
        let environment = try requireDeviceAcceptance(["DECK_DEVICE_SHELL_TITLE", "DECK_DEVICE_NONCE"])
        let title = environment["DECK_DEVICE_SHELL_TITLE"]!
        let app = chineseApp()
        app.launch()
        openCard(named: title, in: app)
        let message = "draft-\(environment["DECK_DEVICE_NONCE"]!)"
        let draft = app.textViews["deck.message.draft"].firstMatch
        XCTAssertTrue(reveal(draft, in: app)); draft.tap(); draft.typeText(message)
        app.buttons["deck.keyboard.done"].tap()
        selectDetailTab("便签", in: app)
        let note = app.textViews["deck.note.new"].firstMatch
        XCTAssertTrue(revealBySwipingDown(note, in: app))
        let noteMarker = "DQA-NOTE-DRAFT-\(environment["DECK_DEVICE_NONCE"]!)"
        note.tap(); note.typeText(noteMarker)
        app.buttons["deck.keyboard.done"].tap()
        app.navigationBars.buttons.element(boundBy: 0).tap()
        XCUIDevice.shared.press(.home)
        app.activate()
        openCard(named: title, in: app)
        XCTAssertTrue(reveal(draft, in: app)); XCTAssertEqual(draft.value as? String, message)
        app.terminate(); app.launch()
        openCard(named: title, in: app)
        XCTAssertTrue(reveal(draft, in: app)); XCTAssertEqual(draft.value as? String, message, "A message draft must survive termination.")
        selectDetailTab("便签", in: app)
        XCTAssertTrue(revealBySwipingDown(note, in: app)); XCTAssertEqual(note.value as? String, noteMarker)
        keepScreenshot("drafts-restored", app)
        let save = app.buttons["deck.note.save"]
        XCTAssertTrue(reveal(save, in: app) && save.isEnabled); save.tap()
        deckStep("draft-note-saved")
        let saved = noteText(noteMarker, in: app)
        if !saved.waitForExistence(timeout: 15) { checkOriginalNoteOperationOnce(in: app) }
        XCTAssertTrue(saved.waitForExistence(timeout: 15), "The restored note must save once without another edit.")
        guard deleteOnlyNote(noteMarker, in: app) else { return }
        selectDetailTab("输出", in: app)
        XCTAssertTrue(replaceAllText(in: draft, expectedOriginal: message, with: ""))
        app.buttons["deck.keyboard.done"].tap()
        deckStep("drafts-done")
        app.terminate()
    }

    // V1: Chinese at an accessibility text size in portrait with the keyboard
    // open keeps the output and the send area reachable.
    func testDeviceV1ChineseLargeTextKeyboardPortrait() throws {
        let environment = try requireDeviceAcceptance(["DECK_DEVICE_SHELL_TITLE"])
        XCUIDevice.shared.orientation = .portrait
        let app = chineseApp(largeText: true)
        app.launch()
        openCard(named: environment["DECK_DEVICE_SHELL_TITLE"]!, in: app)
        XCTAssertTrue(reveal(outputText(app), in: app), "Output must be reachable.")
        let draft = app.textViews["deck.message.draft"].firstMatch
        XCTAssertTrue(reveal(draft, in: app)); draft.tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 5))
        keepScreenshot("large-text-keyboard-portrait", app)
        app.buttons["deck.keyboard.done"].tap()
        XCTAssertTrue(reveal(app.buttons["deck.output.refresh"], in: app))
        app.terminate()
    }

    // V2: the same in landscape. It fails rather than passing in portrait
    // when the phone does not rotate (for example under rotation lock).
    func testDeviceV2ChineseLargeTextLandscape() throws {
        let environment = try requireDeviceAcceptance(["DECK_DEVICE_SHELL_TITLE"])
        defer { XCUIDevice.shared.orientation = .portrait }
        let app = chineseApp(largeText: true)
        app.launch()
        openCard(named: environment["DECK_DEVICE_SHELL_TITLE"]!, in: app)
        XCUIDevice.shared.orientation = .landscapeLeft
        let rotated = waitUntil(5) { let size = XCUIScreen.main.screenshot().image.size; return size.width > size.height }
        keepScreenshot("large-text-landscape-attempt", app)
        XCTAssertTrue(rotated, "The phone did not rotate; landscape cannot be judged on this device state.")
        guard rotated else { return }
        XCTAssertTrue(outputText(app).waitForExistence(timeout: 20), "Output must be present in landscape.")
        // On a physical device the coordinate drags in `reveal` do not follow
        // the rotation; scroll the list itself, boundedly.
        let draft = app.textViews["deck.message.draft"].firstMatch
        let list = app.collectionViews.firstMatch
        for _ in 0..<30 where !(draft.exists && draft.isHittable) { list.swipeUp() }
        XCTAssertTrue(draft.exists && draft.isHittable, "The composer must be reachable in landscape at accessibility text size.")
        draft.tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 5))
        keepScreenshot("large-text-keyboard-landscape", app)
        app.buttons["deck.keyboard.done"].tap()
        app.terminate()
    }

    // L1: a real device lock (not app termination) with a detail open; after
    // waking, the same pairing resumes and the output is captured anew.
    func testDeviceL1LockAndUnlockResumes() throws {
        let environment = try requireDeviceAcceptance(["DECK_DEVICE_SHELL_TITLE"])
        let app = chineseApp()
        app.launch()
        openCard(named: environment["DECK_DEVICE_SHELL_TITLE"]!, in: app)
        let captured = app.descendants(matching: .any)["deck.output.captured"].firstMatch
        XCTAssertTrue(waitUntil(30) { captured.exists })
        let before = captured.label
        XCUIDevice.shared.perform(NSSelectorFromString("pressLockButton"))
        deckStep("locked")
        Thread.sleep(forTimeInterval: 20)
        XCUIDevice.shared.press(.home)
        XCUIDevice.shared.press(.home)
        app.activate()
        deckStep("woken")
        XCTAssertTrue(app.navigationBars[environment["DECK_DEVICE_SHELL_TITLE"]!].waitForExistence(timeout: 30),
                      "After unlocking, the app must return to the open card without re-pairing.")
        XCTAssertFalse(app.textFields["deck.pair.descriptor"].exists)
        XCTAssertTrue(waitUntil(30) { captured.exists && captured.label != before }, "Output must be captured again after unlocking.")
        keepScreenshot("after-unlock", app)
        app.terminate()
    }

    /// Sets this test variant's Local Network permission in the Settings app.
    /// Returns false (without guessing) when the expected rows are not found.
    private func setLocalNetwork(_ enabled: Bool, appName: String) -> Bool {
        let settings = XCUIApplication(bundleIdentifier: "com.apple.Preferences")
        settings.terminate(); settings.launch()
        func open(_ titles: [String]) -> Bool {
            for _ in 0..<12 {
                for title in titles {
                    let cell = settings.staticTexts[title].firstMatch
                    if cell.exists && cell.isHittable { cell.tap(); return true }
                }
                settings.swipeUp()
            }
            return false
        }
        // Privacy & Security → Local Network lists one switch per app, labeled
        // with the app's name. Only the exact test variant's switch is touched;
        // the row text opens a detail page, so tap the switch end itself.
        guard open(["隐私与安全性", "Privacy & Security"]), open(["本地网络", "Local Network"]) else { return false }
        let toggle = settings.switches.matching(NSPredicate(format: "label == %@", appName)).firstMatch
        guard toggle.waitForExistence(timeout: 5) else { return false }
        let isOn = (toggle.value as? String) == "1"
        if isOn != enabled { toggle.coordinate(withNormalizedOffset: CGVector(dx: 0.93, dy: 0.5)).tap() }
        let settled = waitUntil(5) { ((toggle.value as? String) == "1") == enabled }
        keepScreenshot("local-network-\(enabled ? "allowed" : "denied")", settings)
        return settled
    }

    // N1: with Local Network access turned off for this app, the phone says
    // where to look instead of showing a raw error; turning it back on recovers.
    func testDeviceN1LocalNetworkDeniedIsActionable() throws {
        let environment = try requireDeviceAcceptance(["DECK_DEVICE_HOST", "DECK_DEVICE_ADDRESS"])
        let host = environment["DECK_DEVICE_HOST"]!
        defer { _ = setLocalNetwork(true, appName: "Deck QA") }
        XCTAssertTrue(setLocalNetwork(false, appName: "Deck QA"), "Could not turn off Local Network access in Settings.")
        deckStep("local-network-denied")
        let app = chineseApp()
        app.launch()
        // A cold launch has no snapshot yet, so the hint names the paired address.
        let address = environment["DECK_DEVICE_ADDRESS"]!
        let hint = app.staticTexts.matching(NSPredicate(format: "(label CONTAINS %@ OR label CONTAINS %@) AND label CONTAINS %@",
                                                        "「\(host)」", "「\(address)」", "本地网络")).firstMatch
        XCTAssertTrue(hint.waitForExistence(timeout: 60), "A denied Local Network permission must lead to the actionable checklist.")
        keepScreenshot("local-network-denied-app", app)
        XCTAssertTrue(setLocalNetwork(true, appName: "Deck QA"), "Could not restore Local Network access.")
        deckStep("local-network-restored")
        app.activate()
        XCTAssertTrue(waitUntil(60) { !hint.exists && app.staticTexts[host].firstMatch.exists }, "Restoring access must recover without re-pairing.")
        keepScreenshot("local-network-restored-app", app)
        app.terminate()
    }

    // X1: revoking this device on the Mac while a detail is open removes its
    // content and leaves an unpaired, explained state.
    func testDeviceX1RevokeWhileOpen() throws {
        let environment = try requireDeviceAcceptance(["DECK_DEVICE_SHELL_TITLE"])
        let app = chineseApp()
        app.launch()
        openCard(named: environment["DECK_DEVICE_SHELL_TITLE"]!, in: app)
        XCTAssertTrue(waitUntil(30) { self.outputText(app).exists })
        deckStep("ready-revoke")
        XCTAssertTrue(waitUntil(45) { !self.outputText(app).exists }, "Revocation must remove shared output from the open detail.")
        keepScreenshot("after-revoke", app)
        deckStep("revoke-observed")
        app.terminate()
    }
}

