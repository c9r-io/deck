// DEBUG-ONLY smoke driver (build.rs compiles this file for non-release
// profiles only; smoke_native.rs refuses unless the isolated smoke launch
// arguments are present). It acts on Deck's OWN process: AppKit events are
// handed to Deck's own window with NSApp.sendEvent (no CGEventPost, no
// Accessibility, no other process), snapshots come from Deck's own
// WKWebView, and the shared general pasteboard is guarded: its original
// items stay in this process's memory only and are restored only while the
// current change is still one this test wrote or verified.
import Foundation
import AppKit
import WebKit
import UserNotifications
import ApplicationServices

private func smokeWindow() -> NSWindow? {
    // Startup windows may still be hidden; select only this process's WK window.
    NSApp.keyWindow ?? NSApp.mainWindow ?? NSApp.windows.first { findWebView($0.contentView) != nil }
}
private func findWebView(_ view: NSView?) -> WKWebView? {
    guard let view else { return nil }
    if let web = view as? WKWebView { return web }
    for child in view.subviews { if let web = findWebView(child) { return web } }
    return nil
}
// CSS viewport point -> window point. The page viewport (innerHeight) can be
// shorter than the webview bounds: a full-size-content title bar covers the
// top strip, so the CSS origin sits `bounds.height - innerHeight` down.
private var smokeViewportHeight: Double = 0
private func windowPoint(_ web: WKWebView, _ x: Double, _ y: Double) -> NSPoint {
    let height = smokeViewportHeight > 0 ? smokeViewportHeight : Double(web.bounds.height)
    let local = web.isFlipped ? NSPoint(x: x, y: Double(web.bounds.height) - height + y)
                              : NSPoint(x: x, y: height - y)
    return web.convert(local, to: nil)
}
@_cdecl("deck_smoke_viewport")
public func deckSmokeViewport(_ height: Double) { onMain { smokeViewportHeight = height } }
private var smokeEventNumber = 0

// kind: 0 down, 1 dragged, 2 up. CSS point in the webview.
@_cdecl("deck_smoke_mouse")
public func deckSmokeMouse(_ kind: Int32, _ x: Double, _ y: Double, _ clicks: Int32) -> Int32 {
    onMain {
        guard let window = smokeWindow(), let web = findWebView(window.contentView) else { return -1 }
        let type: NSEvent.EventType = kind == 0 ? .leftMouseDown : kind == 1 ? .leftMouseDragged : .leftMouseUp
        if kind == 0 { smokeEventNumber += 1 } // drag/up belong to the same click
        guard let event = NSEvent.mouseEvent(with: type, location: windowPoint(web, x, y),
                                             modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
                                             windowNumber: window.windowNumber, context: nil,
                                             eventNumber: smokeEventNumber, clickCount: Int(max(clicks, 1)),
                                             pressure: kind == 2 ? 0 : 1) else { return -2 }
        NSApp.sendEvent(event)
        return 0
    }
}

// One pixel-unit wheel event at a CSS point (positive dy scrolls up, AppKit
// sign). An NSEvent built from a CGEvent has no window, so its
// locationInWindow is its (Cocoa) screen location: place that location at
// the window point and hand the event to Deck's own window.
@_cdecl("deck_smoke_scroll")
public func deckSmokeScroll(_ x: Double, _ y: Double, _ dy: Int32) -> Int32 {
    onMain {
        guard let window = smokeWindow(), let web = findWebView(window.contentView) else { return -1 }
        let point = windowPoint(web, x, y)
        let height = NSScreen.screens.first?.frame.height ?? 0
        guard let cg = CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 1,
                               wheel1: dy, wheel2: 0, wheel3: 0) else { return -2 }
        cg.location = CGPoint(x: point.x, y: height - point.y)
        guard let event = NSEvent(cgEvent: cg) else { return -3 }
        window.sendEvent(event)
        return 0
    }
}

// A key down/up pair; modifiers are NSEvent.ModifierFlags raw bits.
@_cdecl("deck_smoke_key")
public func deckSmokeKey(_ chars: UnsafePointer<CChar>?, _ keyCode: UInt16, _ modifiers: UInt64) -> Int32 {
    onMain {
        guard let window = smokeWindow() else { return -1 }
        let text = chars.map { String(cString: $0) } ?? ""
        let flags = NSEvent.ModifierFlags(rawValue: UInt(modifiers))
        for type in [NSEvent.EventType.keyDown, .keyUp] {
            guard let event = NSEvent.keyEvent(with: type, location: .zero, modifierFlags: flags,
                                               timestamp: ProcessInfo.processInfo.systemUptime,
                                               windowNumber: window.windowNumber, context: nil,
                                               characters: text, charactersIgnoringModifiers: text.lowercased(),
                                               isARepeat: false, keyCode: keyCode) else { return -2 }
            NSApp.sendEvent(event)
        }
        return 0
    }
}

// 0 = hide Deck (a real resign-active); 1 = state bits only; 2 = bring
// Deck's own window forward (may be refused by the system).
// bit 1 active, bit 2 key window, bit 4 hidden.
@_cdecl("deck_smoke_app")
public func deckSmokeApp(_ action: Int32) -> Int32 {
    onMain {
        if action == 0 { NSApp.hide(nil) }
        if action == 2 {
            NSApp.unhide(nil)
            NSApp.activate(ignoringOtherApps: true)
            smokeWindow()?.makeKeyAndOrderFront(nil)
        }
        return (NSApp.isActive ? 1 : 0) | (NSApp.keyWindow != nil ? 2 : 0) | (NSApp.isHidden ? 4 : 0)
    }
}

// Restrict Deck's OWN webview input context to Roman input sources, so native
// test keystrokes are not composed by a CJK input method. App-scoped: the
// user's input-source setting is not changed and it ends with this process.
@_cdecl("deck_smoke_roman_input")
public func deckSmokeRomanInput() -> Int32 {
    onMain {
        guard let window = smokeWindow(), let web = findWebView(window.contentView),
              let context = web.inputContext else { return -1 }
        context.allowedInputSourceLocales = [NSAllRomanInputSourcesLocaleIdentifier]
        return 0
    }
}

// The content size AppKit holds Deck's own window to on a user resize (Tauri
// installs tauri.conf.json minWidth/minHeight as NSWindow.minSize, a frame
// size) and the current content size, in points: out[0..4] = min w, min h,
// w, h. Programmatic setContentSize is NOT bound by this minimum.
@_cdecl("deck_smoke_window_min")
public func deckSmokeWindowMin(_ out: UnsafeMutablePointer<Double>?) -> Int32 {
    guard let out else { return -1 }
    return onMain {
        guard let window = smokeWindow() else { return -2 }
        let fromFrame = window.contentRect(forFrameRect: NSRect(origin: .zero, size: window.minSize)).size
        let content = window.contentRect(forFrameRect: window.frame).size
        out[0] = Double(max(fromFrame.width, window.contentMinSize.width))
        out[1] = Double(max(fromFrame.height, window.contentMinSize.height))
        out[2] = Double(content.width)
        out[3] = Double(content.height)
        return 0
    }
}

// Must be called off the main thread; writes a PNG of Deck's own WKWebView.
@_cdecl("deck_smoke_snapshot")
public func deckSmokeSnapshot(_ path: UnsafePointer<CChar>?) -> Int32 {
    guard let path, !Thread.isMainThread else { return -1 }
    let target = String(cString: path)
    let done = DispatchSemaphore(value: 0)
    var code: Int32 = -2
    DispatchQueue.main.async {
        guard let window = smokeWindow(), let web = findWebView(window.contentView) else { done.signal(); return }
        web.takeSnapshot(with: nil) { image, _ in
            if let tiff = image?.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff),
               let png = rep.representation(using: .png, properties: [:]) {
                code = FileManager.default.createFile(atPath: target, contents: png,
                                                      attributes: [.posixPermissions: 0o600]) ? 0 : -3
            } else { code = -4 }
            done.signal()
        }
    }
    return done.wait(timeout: .now() + 10) == .success ? code : -5
}

/* ----- pasteboard guard (general or the test-owned named board) -----
   Contract (translation-smoke.mjs `withGuard`, smoke_native.rs):
   - begin captures every item/type/data and requires the changeCount to be
     stable across the capture; lazy/promised data refuses the guard.
   - every test write needs an active guard AND the board still at the last
     version this guard owns; otherwise it is refused BEFORE touching the
     board and the guard becomes conflicted (all later writes refused).
   - writes this process does not perform (a fixture's /copy, Deck's own copy
     button, the driver) need a permit first (same version check) and are
     adopted only by a writer RECEIPT equal to the current changeCount —
     never by comparing text.
   - settle restores only while the board is still at the owned version and
     keeps the backup when a restore fails, so a later settle can retry.
   NSPasteboard offers no cross-process compare-and-swap: between a version
   check and clearContents() another process can still write. That window is
   narrowed, not closed; cases that need it closed use the named board.
   Nothing here returns, logs or stores pasteboard content outside memory. */
private let namedBoardName = NSPasteboard.Name("io.c9r.deck.smoke.translation.\(getpid())")
private func testBoard() -> NSPasteboard { NSPasteboard(name: namedBoardName) }
// `writes` counts complete test writes; `modified` is set the moment the guard
// itself changed the board (a successful clearContents), even if the fill that
// follows fails — nothing else may then report "not written" or "external".
private struct GuardRecord { var id: Int; var board: Int; var writes = 0; var rejects = 0; var reason = 0; var result = 0
                             var modified = false }
private enum Phase { case idle, active, conflicted, settled }
private var phase = Phase.idle
private var guardItems: [[(NSPasteboard.PasteboardType, Data)]] = []
private var guardOwned = -1
private var permitFrom = -1
private var guardBoard: NSPasteboard? = nil
private var records: [GuardRecord] = []
// result codes
private let NOT_WRITTEN: Int32 = 10, RESTORED: Int32 = 11, EXTERNAL_KEPT: Int32 = 12, RESTORE_FAILED: Int32 = 13,
            BEGIN_REFUSED: Int32 = 15
// refusal reasons
private let R_NO_GUARD = 1, R_VERSION = 2, R_SETTLED = 3, R_RECEIPT = 4, R_NO_PERMIT = 5, R_FILL = 8

private func reject(_ reason: Int) -> Int64 {
    if !records.isEmpty && phase != .idle {
        records[records.count - 1].rejects += 1
        if records[records.count - 1].reason == 0 { records[records.count - 1].reason = reason }
    }
    if phase == .active { phase = .conflicted }
    return Int64(-reason)
}
private func snapshot(_ board: NSPasteboard) -> [[(NSPasteboard.PasteboardType, Data)]]? {
    var items: [[(NSPasteboard.PasteboardType, Data)]] = []
    for item in board.pasteboardItems ?? [] {
        var entry: [(NSPasteboard.PasteboardType, Data)] = []
        for type in item.types {
            guard let data = item.data(forType: type) else { return nil }
            entry.append((type, data))
        }
        items.append(entry)
    }
    return items
}
private func same(_ a: [[(NSPasteboard.PasteboardType, Data)]], _ b: [[(NSPasteboard.PasteboardType, Data)]]) -> Bool {
    guard a.count == b.count else { return false }
    for (x, y) in zip(a, b) {
        guard x.count == y.count else { return false }
        for ((t1, d1), (t2, d2)) in zip(x, y) where t1 != t2 || d1 != d2 { return false }
    }
    return true
}
// Is the board still exactly at the version this guard owns?
private func owned(_ board: NSPasteboard) -> Bool {
    #if SMOKE_MUTANT_NO_PRECHECK
    return true
    #else
    return board.changeCount == guardOwned
    #endif
}

// board 0 general, 1 named test board. >= 0: guard id; -1 already active;
// -2 lazy/promised data; -3 the board changed during the capture.
@_cdecl("deck_smoke_pb_guard_begin")
public func deckSmokePbGuardBegin(_ boardKind: Int32) -> Int32 {
    onMain {
        guard phase == .idle || phase == .settled else { return -1 }
        let board = boardKind == 1 ? testBoard() : NSPasteboard.general
        let id = records.count + 1
        let before = board.changeCount
        guard let items = snapshot(board) else {
            records.append(GuardRecord(id: id, board: Int(boardKind), reason: 6, result: Int(BEGIN_REFUSED))); return -2
        }
        guard board.changeCount == before, let again = snapshot(board), same(items, again) else {
            records.append(GuardRecord(id: id, board: Int(boardKind), reason: 7, result: Int(BEGIN_REFUSED))); return -3
        }
        guardItems = items; guardOwned = before; permitFrom = -1; guardBoard = board; phase = .active
        records.append(GuardRecord(id: id, board: Int(boardKind)))
        return Int32(id)
    }
}

// Test-owned text write on the guarded board; the new changeCount, or a
// negative refusal reason (the board is untouched).
@_cdecl("deck_smoke_pb_write")
public func deckSmokePbWrite(_ text: UnsafePointer<CChar>?) -> Int64 {
    onMain {
        guard phase == .active, let board = guardBoard, let text else {
            return reject(phase == .idle ? R_NO_GUARD : R_SETTLED)
        }
        guard owned(board) else { return reject(R_VERSION) }
        let count = board.clearContents()
        #if !SMOKE_MUTANT_LATE_ACCOUNTING
        // The clear already changed the board: record its receipt and the fact
        // before the fill, which may fail.
        guardOwned = count
        #if !SMOKE_MUTANT_VERSION_ONLY
        records[records.count - 1].modified = true
        #endif
        #endif
        let failFill = failNextFill && board.name == namedBoardName
        failNextFill = false
        guard !failFill, board.setString(String(cString: text), forType: .string) else { return reject(R_FILL) }
        #if SMOKE_MUTANT_LATE_ACCOUNTING
        guardOwned = count; records[records.count - 1].modified = true
        #endif
        records[records.count - 1].writes += 1
        return Int64(count)
    }
}

// Permit for a write another path performs: the version it must start from.
@_cdecl("deck_smoke_pb_permit")
public func deckSmokePbPermit() -> Int64 {
    onMain {
        guard phase == .active, let board = guardBoard else { return reject(phase == .idle ? R_NO_GUARD : R_SETTLED) }
        guard owned(board) else { return reject(R_VERSION) }
        permitFrom = guardOwned
        return Int64(guardOwned)
    }
}

// Adopt the write a permitted path performed, by that writer's receipt
// (the changeCount its clearContents() returned). 0 adopted; < 0 refused.
@_cdecl("deck_smoke_pb_adopt")
public func deckSmokePbAdopt(_ receipt: Int64) -> Int64 {
    onMain {
        guard phase == .active, let board = guardBoard else { return reject(phase == .idle ? R_NO_GUARD : R_SETTLED) }
        guard permitFrom >= 0 else { return reject(R_NO_PERMIT) }
        #if SMOKE_MUTANT_TEXT_CLAIM
        guardOwned = board.changeCount; permitFrom = -1; records[records.count - 1].writes += 1; return 0
        #else
        guard receipt > Int64(permitFrom), receipt == Int64(board.changeCount) else { return reject(R_RECEIPT) }
        guardOwned = Int(receipt); permitFrom = -1; records[records.count - 1].writes += 1
        records[records.count - 1].modified = true
        return 0
        #endif
    }
}

// Settle the guard. NOT_WRITTEN (nothing to undo), RESTORED (verified),
// EXTERNAL_KEPT (a newer external version stays), RESTORE_FAILED (the backup
// is KEPT so a later settle can retry), 1 no guard.
@_cdecl("deck_smoke_pb_guard_end")
public func deckSmokePbGuardEnd() -> Int32 {
    onMain {
        guard phase == .active || phase == .conflicted, let board = guardBoard else { return 1 }
        let index = records.count - 1
        let finish = { (code: Int32) -> Int32 in
            records[index].result = Int(code)
            #if SMOKE_MUTANT_EARLY_DISCARD
            phase = .settled; guardItems = []; guardBoard = nil
            #else
            if code != RESTORE_FAILED { phase = .settled; guardItems = []; guardBoard = nil }
            #endif
            return code
        }
        if !records[index].modified && records[index].writes == 0 && board.changeCount == guardOwned {
            return finish(NOT_WRITTEN)
        }
        #if !SMOKE_MUTANT_NO_PRECHECK
        guard board.changeCount == guardOwned else { return finish(EXTERNAL_KEPT) }
        #endif
        guardOwned = board.clearContents() // our own clear: a retry still owns the board
        if !guardItems.isEmpty {
            let objects = guardItems.map { entry -> NSPasteboardItem in
                let item = NSPasteboardItem()
                for (type, data) in entry { item.setData(data, forType: type) }
                return item
            }
            let failNow = failRestores > 0 && board.name == namedBoardName
            if failNow { failRestores -= 1 }
            guard !failNow, board.writeObjects(objects) else { phase = .conflicted; return finish(RESTORE_FAILED) }
        }
        guard let now = snapshot(board), same(now, guardItems) else { phase = .conflicted; return finish(RESTORE_FAILED) }
        return finish(RESTORED)
    }
}

// Faults for the NAMED test board only (they never apply to the general
// board): the next N restores fail; the next test fill fails after its clear.
// Arming is refused while a guard on the general board is active.
private var failRestores = 0
private var failNextFill = false
@_cdecl("deck_smoke_pb_fail_next_restore")
public func deckSmokePbFailNextRestore() { onMain { if guardBoard?.name != NSPasteboard.general.name { failRestores += 1 } } }
@_cdecl("deck_smoke_pb_fail_restores")
public func deckSmokePbFailRestores(_ count: Int32) -> Int32 {
    onMain {
        if guardBoard?.name == NSPasteboard.general.name { return -1 }
        failRestores = Int(max(0, count)); return 0
    }
}
@_cdecl("deck_smoke_pb_fail_next_fill")
public func deckSmokePbFailNextFill() -> Int32 {
    onMain {
        if guardBoard?.name == NSPasteboard.general.name { return -1 }
        failNextFill = true; return 0
    }
}

// 0 idle/settled (writes refused), 1 active, 2 conflicted.
@_cdecl("deck_smoke_pb_state")
public func deckSmokePbState() -> Int32 {
    onMain { phase == .active ? 1 : phase == .conflicted ? 2 : 0 }
}

// Content-free audit: "id,board,writes,rejects,reason,result,modified;..."
@_cdecl("deck_smoke_pb_audit")
public func deckSmokePbAudit() -> UnsafeMutablePointer<CChar>? {
    onMain {
        strdup(records.map { "\($0.id),\($0.board),\($0.writes),\($0.rejects),\($0.reason),\($0.result),\($0.modified ? 1 : 0)" }
            .joined(separator: ";"))
    }
}

/* ----- test-owned named pasteboard: seeding and simulated external writers ----- */
@_cdecl("deck_smoke_pb_named")
public func deckSmokePbNamed(_ enable: Int32) -> Int32 {
    onMain {
        if enable != 0 {
            let board = testBoard()
            board.clearContents()
            deckSmokeTranslationPasteboard = board
        } else {
            deckSmokeTranslationPasteboard = nil
            testBoard().releaseGlobally()
        }
        return 0
    }
}

private final class NoData: NSObject, NSPasteboardItemDataProvider {
    func pasteboard(_ pasteboard: NSPasteboard?, item: NSPasteboardItem, provideDataForType type: NSPasteboard.PasteboardType) {}
}
private let noData = NoData()

// Writes the NAMED test board only, bypassing the guard (an "external" writer
// for negatives, or seeding). kind 0 text, 1 non-text (PNG bytes), 2 empty
// string, 3 multi-item multi-type, 4 lazy/promised item, 5 clear.
// Returns the changeCount clearContents() produced (the writer's receipt).
@_cdecl("deck_smoke_pb_named_write")
public func deckSmokePbNamedWrite(_ kind: Int32, _ text: UnsafePointer<CChar>?) -> Int64 {
    onMain {
        let board = testBoard()
        let count = board.clearContents()
        let value = text.map { String(cString: $0) } ?? ""
        switch kind {
        case 0: board.setString(value, forType: .string)
        case 1: board.setData(Data([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), forType: .png)
        case 2: board.setString("", forType: .string)
        case 3:
            let first = NSPasteboardItem()
            first.setString("synthetic multi-item text", forType: .string)
            first.setString("<b>synthetic</b>", forType: .html)
            first.setData(Data([1, 2, 3, 4]), forType: NSPasteboard.PasteboardType("io.c9r.deck.smoke.custom"))
            let second = NSPasteboardItem()
            second.setData(Data([0x89, 0x50, 0x4e, 0x47]), forType: .png)
            board.writeObjects([first, second])
        case 4:
            let item = NSPasteboardItem()
            item.setDataProvider(noData, forTypes: [.string])
            board.writeObjects([item])
        default: break
        }
        return Int64(count)
    }
}

// changeCount of a board (0 general, 1 named) — a version number, no content.
@_cdecl("deck_smoke_pb_count")
public func deckSmokePbCount(_ boardKind: Int32) -> Int64 {
    onMain { Int64((boardKind == 1 ? testBoard() : NSPasteboard.general).changeCount) }
}

// Read-only UN inventory for the isolated Reminder carrier. It intentionally
// has no API to inject responses or claim that a physical banner was shown.
@_cdecl("deck_smoke_reminder_inventory")
public func deckSmokeReminderInventory() -> UnsafeMutablePointer<CChar>? {
    guard Bundle.main.bundleIdentifier?.hasPrefix("io.c9r.deck.reminder.smoke") == true else { return nil }
    // Inventory-only LaunchServices observers do not load a Board or project
    // reminders; initialize AppKit only for their own Dock observation.
    if NSApp == nil { _ = NSApplication.shared }
    let center = UNUserNotificationCenter.current()
    let group = DispatchGroup(); let lock = NSLock()
    var pending = [[String: Any]](); var delivered = [[String: Any]](); var status = -1
    group.enter(); center.getPendingNotificationRequests { requests in
        lock.lock(); pending = requests.map { ["identifier": $0.identifier,
            "dueAt": (($0.trigger as? UNCalendarNotificationTrigger)?.nextTriggerDate()?.timeIntervalSince1970 ?? 0) * 1000] }; lock.unlock(); group.leave()
    }
    group.enter(); center.getDeliveredNotifications { notifications in
        lock.lock(); delivered = notifications.map { ["identifier": $0.request.identifier, "deliveredAt": $0.date.timeIntervalSince1970 * 1000] }; lock.unlock(); group.leave()
    }
    group.enter(); center.getNotificationSettings { settings in lock.lock(); status = Int(deckNotifyStatusCode(settings.authorizationStatus)); lock.unlock(); group.leave() }
    guard group.wait(timeout: .now() + 5) == .success else { return nil }
    lock.lock(); defer { lock.unlock() }
    guard let data = try? JSONSerialization.data(withJSONObject: ["pending": pending, "delivered": delivered, "authorization": status, "permissionRequest": deckNotifyPermissionDiagnostics(), "appFrontmost": onMain { NSWorkspace.shared.frontmostApplication?.processIdentifier == ProcessInfo.processInfo.processIdentifier }, "accessibilityTrusted": AXIsProcessTrusted(), "dockBadge": onMain { NSApp.dockTile.badgeLabel ?? "" }]), let json = String(data: data, encoding: .utf8) else { return nil }
    return strdup(json)
}
@_cdecl("deck_smoke_reminder_withdraw")
public func deckSmokeReminderWithdraw() -> Int32 {
    guard Bundle.main.bundleIdentifier?.hasPrefix("io.c9r.deck.reminder.smoke") == true else { return -1 }
    let center = UNUserNotificationCenter.current()
    // The carrier owns its whole application notification namespace. No
    // production bundle, user notification or shared Agent is addressed.
    center.removeAllPendingNotificationRequests(); center.removeAllDeliveredNotifications()
    return 0
}
