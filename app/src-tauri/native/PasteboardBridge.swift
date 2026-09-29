// Main-thread AppKit pasteboard authority and one-shot source-language fact.
// No clipboard text is read by a baseline/count probe. No Translation APIs.
import Foundation
import AppKit
import NaturalLanguage

// Debug smoke builds compile native/SmokeBridge.swift, which may point the
// same gate at a test-owned named pasteboard. Release never sets it.
var deckSmokeTranslationPasteboard: NSPasteboard? = nil
func deckTranslationPasteboard() -> NSPasteboard { deckSmokeTranslationPasteboard ?? .general }

func onMain<T>(_ body: () -> T) -> T {
    if Thread.isMainThread { return body() }
    return DispatchQueue.main.sync(execute: body)
}

@_cdecl("deck_pasteboard_focused_count")
public func deckPasteboardFocusedCount() -> Int64 {
    onMain {
        guard NSApp.isActive, NSApp.keyWindow != nil else { return -1 }
        return Int64(deckTranslationPasteboard().changeCount)
    }
}

@_cdecl("deck_pasteboard_read_text")
public func deckPasteboardReadText() -> UnsafeMutablePointer<CChar>? {
    onMain {
        guard NSApp.isActive, NSApp.keyWindow != nil,
              let text = deckTranslationPasteboard().string(forType: .string),
              !text.contains("\0") else { return nil }
        return strdup(text)
    }
}

// Deck's own Lens copy (Copy Translation, Copy Source, Cmd+C in the result).
// Returns the changeCount that clearContents() produced for THIS write: the
// receipt the clipboard gate uses to exclude exactly this version from
// Copied-text observation. -1 when the write failed.
@_cdecl("deck_pasteboard_write_text")
public func deckPasteboardWriteText(_ text: UnsafePointer<CChar>?) -> Int64 {
    onMain {
        guard let text else { return -1 }
        let board = deckTranslationPasteboard()
        let count = board.clearContents()
        return board.setString(String(cString: text), forType: .string) ? Int64(count) : -1
    }
}

@_cdecl("deck_pasteboard_free")
public func deckPasteboardFree(_ text: UnsafeMutablePointer<CChar>?) { free(text) }

// 1 = dominant English, 2 = dominant Chinese, 0 = unsupported/unknown.
@_cdecl("deck_translation_source_kind")
public func deckTranslationSourceKind(_ source: UnsafePointer<CChar>?) -> Int32 {
    guard let source else { return 0 }
    let recognizer = NLLanguageRecognizer()
    recognizer.processString(String(cString: source))
    switch recognizer.dominantLanguage {
    case .english: return 1
    case .simplifiedChinese, .traditionalChinese: return 2
    default: return 0
    }
}
