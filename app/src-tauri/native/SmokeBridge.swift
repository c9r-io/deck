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

private func smokeWindow() -> NSWindow? {
    NSApp.keyWindow ?? NSApp.mainWindow ?? NSApp.windows.first { $0.isVisible }
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

/* ----- shared general pasteboard guard ----- */
private var guardItems: [[(NSPasteboard.PasteboardType, Data)]]? = nil
private var guardOwned = -1
private var guardExternal = false

// >= 0: item count captured; -1 already active; -2 an item type is not
// preservable (lazy/promised data), so the shared-pasteboard cases must stop.
@_cdecl("deck_smoke_pb_guard_begin")
public func deckSmokePbGuardBegin() -> Int32 {
    onMain {
        guard guardItems == nil else { return -1 }
        let board = NSPasteboard.general
        var items: [[(NSPasteboard.PasteboardType, Data)]] = []
        for item in board.pasteboardItems ?? [] {
            var entry: [(NSPasteboard.PasteboardType, Data)] = []
            for type in item.types {
                guard let data = item.data(forType: type) else { return -2 }
                entry.append((type, data))
            }
            items.append(entry)
        }
        guardItems = items; guardOwned = board.changeCount; guardExternal = false
        return Int32(min(items.count, 10_000))
    }
}

// Test-owned text write; returns the new changeCount (or -1 without a guard).
@_cdecl("deck_smoke_pb_write")
public func deckSmokePbWrite(_ text: UnsafePointer<CChar>?) -> Int64 {
    onMain {
        guard guardItems != nil, let text else { return -1 }
        let board = NSPasteboard.general
        board.clearContents()
        guard board.setString(String(cString: text), forType: .string) else { return -2 }
        guardOwned = board.changeCount
        return Int64(guardOwned)
    }
}

// Claim the current change as test-owned: the count advanced past the last
// test-owned change AND the text is what this test's action wrote (a
// fixture's pbcopy, Deck's own copy, a native Cmd+C), compared after line-end
// and trailing-space normalisation. Otherwise it is an external change and
// restoration is refused. 0 owned; 1 external.
private func normalized(_ text: String) -> String {
    text.replacingOccurrences(of: "\r\n", with: "\n").split(separator: "\n", omittingEmptySubsequences: false)
        .map { $0.replacingOccurrences(of: "\u{00a0}", with: " ").trimmingCharacters(in: .whitespaces) }
        .joined(separator: "\n").trimmingCharacters(in: .whitespacesAndNewlines)
}
@_cdecl("deck_smoke_pb_claim")
public func deckSmokePbClaim(_ expected: UnsafePointer<CChar>?) -> Int32 {
    onMain {
        guard guardItems != nil, let expected else { return -1 }
        let board = NSPasteboard.general
        guard board.changeCount > guardOwned,
              normalized(board.string(forType: .string) ?? "") == normalized(String(cString: expected)) else {
            guardExternal = true; return 1
        }
        guardOwned = board.changeCount
        return 0
    }
}

// 0 restored and verified; 1 no guard; 2 external change (not restored);
// 3 write failed; 4 restored content differs.
@_cdecl("deck_smoke_pb_guard_end")
public func deckSmokePbGuardEnd() -> Int32 {
    onMain {
        guard let items = guardItems else { return 1 }
        defer { guardItems = nil; guardOwned = -1; guardExternal = false }
        let board = NSPasteboard.general
        if guardExternal || board.changeCount != guardOwned { return 2 }
        board.clearContents()
        if !items.isEmpty {
            let objects = items.map { entry -> NSPasteboardItem in
                let item = NSPasteboardItem()
                for (type, data) in entry { item.setData(data, forType: type) }
                return item
            }
            if !board.writeObjects(objects) { return 3 }
        }
        let now = (board.pasteboardItems ?? []).map { item in item.types.map { ($0, item.data(forType: $0)) } }
        guard now.count == items.count else { return 4 }
        for (restored, original) in zip(now, items) {
            guard restored.count == original.count else { return 4 }
            for ((type, data), (otype, odata)) in zip(restored, original) where type != otype || data != odata {
                return 4
            }
        }
        return 0
    }
}

/* ----- test-owned named pasteboard for boundary and race cases ----- */
@_cdecl("deck_smoke_pb_named")
public func deckSmokePbNamed(_ enable: Int32) -> Int32 {
    onMain {
        if enable != 0 {
            let board = NSPasteboard(name: NSPasteboard.Name("io.c9r.deck.smoke.translation.\(getpid())"))
            board.clearContents()
            deckSmokeTranslationPasteboard = board
        } else {
            deckSmokeTranslationPasteboard?.releaseGlobally()
            deckSmokeTranslationPasteboard = nil
        }
        return 0
    }
}

// kind 0 text, 1 non-text (PNG bytes), 2 empty string; returns changeCount.
@_cdecl("deck_smoke_pb_named_write")
public func deckSmokePbNamedWrite(_ kind: Int32, _ text: UnsafePointer<CChar>?) -> Int64 {
    onMain {
        guard let board = deckSmokeTranslationPasteboard else { return -1 }
        board.clearContents()
        switch kind {
        case 0: board.setString(text.map { String(cString: $0) } ?? "", forType: .string)
        case 1: board.setData(Data([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), forType: .png)
        default: board.setString("", forType: .string)
        }
        return Int64(board.changeCount)
    }
}
