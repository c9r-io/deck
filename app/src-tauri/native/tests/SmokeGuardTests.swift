// Headless tests of the smoke pasteboard guard (native/SmokeBridge.swift),
// run ONLY on the test-owned named pasteboard: the general pasteboard is
// never touched. scripts/test-smoke-guard also compiles SMOKE_MUTANT_* builds
// and requires each of them to fail these tests (negative control).
import AppKit

@_silgen_name("deck_smoke_pb_guard_begin") func begin(_ board: Int32) -> Int32
@_silgen_name("deck_smoke_pb_write") func write(_ text: UnsafePointer<CChar>?) -> Int64
@_silgen_name("deck_smoke_pb_permit") func permit() -> Int64
@_silgen_name("deck_smoke_pb_adopt") func adopt(_ receipt: Int64) -> Int64
@_silgen_name("deck_smoke_pb_guard_end") func end() -> Int32
@_silgen_name("deck_smoke_pb_state") func state() -> Int32
@_silgen_name("deck_smoke_pb_named_write") func external(_ kind: Int32, _ text: UnsafePointer<CChar>?) -> Int64
@_silgen_name("deck_smoke_pb_count") func count(_ board: Int32) -> Int64
@_silgen_name("deck_smoke_pb_fail_next_restore") func tFailNextRestore()
@_silgen_name("deck_smoke_pb_fail_next_fill") func tFailNextFill() -> Int32
@_silgen_name("deck_smoke_pb_audit") func tAudit() -> UnsafeMutablePointer<CChar>?

let board = NSPasteboard(name: NSPasteboard.Name("io.c9r.deck.smoke.translation.\(getpid())"))
var failures: [String] = []
func check(_ ok: Bool, _ name: String) { if !ok { failures.append(name) } }
func text() -> String? { board.string(forType: .string) }
let T_RESTORED: Int32 = 11, T_EXTERNAL_KEPT: Int32 = 12, T_NOT_WRITTEN: Int32 = 10, T_RESTORE_FAILED: Int32 = 13

@main struct SmokeGuardTests {
    static func main() {
        // F02: no guard -> refused, untouched
        _ = external(0, "original")
        var before = count(1)
        check(write("test") < 0 && count(1) == before && text() == "original", "F02 write without a guard")
        check(permit() < 0, "F02 permit without a guard")
        // F07: empty board, multi-item multi-type, non-text: restored exactly
        for (kind, name) in [(Int32(5), "empty"), (3, "multi-item"), (1, "non-text")] {
            _ = external(kind, nil)
            let original = (board.pasteboardItems ?? []).map { item in item.types.map { ($0, item.data(forType: $0)) } }
            check(begin(1) > 0, "F07 begin \(name)")
            check(write("synthetic test write") > 0, "F07 write \(name)")
            check(end() == T_RESTORED, "F07 restored \(name)")
            let now = (board.pasteboardItems ?? []).map { item in item.types.map { ($0, item.data(forType: $0)) } }
            check(now.count == original.count && zip(now, original).allSatisfy { a, b in
                a.count == b.count && zip(a, b).allSatisfy { $0.0 == $1.0 && $0.1 == $1.1 } }, "F07 exact \(name)")
        }
        // F02: an ended guard refuses writes
        before = count(1)
        check(write("late") < 0 && count(1) == before, "F02 write after end")
        // F01: lazy/promised data refuses the guard; nothing can be written
        _ = external(4, nil)
        before = count(1)
        check(begin(1) == -2, "F01 lazy data refuses begin")
        check(write("x") < 0 && permit() < 0 && count(1) == before, "F01 no write after a refused begin")
        // F02: version changed before the write -> refused, board untouched, conflicted
        _ = external(0, "original")
        check(begin(1) > 0, "F02 begin")
        _ = external(0, "external")
        before = count(1)
        check(write("test") < 0 && count(1) == before && text() == "external", "F02 version check before write")
        check(state() == 2 && write("again") < 0, "F02 conflicted stops later writes")
        check(end() == T_EXTERNAL_KEPT && text() == "external", "F02 external kept")
        // F03: an external write with the test's exact (or normalized-equal) text is not adopted
        for sample in ["The expected test text.", "The expected test text.\n "] {
            _ = external(0, "original")
            check(begin(1) > 0, "F03 begin")
            check(permit() >= 0, "F03 permit")
            let receipt = external(0, sample) // written by someone else
            check(adopt(-1) < 0, "F03 no receipt, no adoption")
            _ = receipt
            check(end() == T_EXTERNAL_KEPT && text() == sample, "F03 external content kept")
        }
        // receipts: a permitted writer's own receipt is adopted
        _ = external(0, "original")
        check(begin(1) > 0, "receipt begin")
        check(permit() >= 0, "receipt permit")
        let receipt = external(0, "fixture copy") // stands in for the fixture writer
        check(adopt(receipt) == 0, "receipt adopted")
        check(end() == T_RESTORED && text() == "original", "receipt write restored")
        // F04: takeover after a test write -> later writes refused, backup NOT restored over it
        _ = external(0, "original")
        check(begin(1) > 0 && write("test one") > 0, "F04 first write")
        _ = external(0, "user copy")
        check(write("test two") < 0 && text() == "user copy", "F04 later write refused")
        check(end() == T_EXTERNAL_KEPT && text() == "user copy", "F04 external version kept")
        // NOT_WRITTEN: nothing to undo, nothing written
        _ = external(0, "original")
        before = count(1)
        check(begin(1) > 0 && end() == T_NOT_WRITTEN && count(1) == before, "not written leaves the board alone")
        // F05/F06: a failed restore keeps the backup; a later settle completes it
        _ = external(3, nil)
        check(begin(1) > 0 && write("test") > 0, "restore-failed setup")
        tFailNextRestore()
        check(end() == T_RESTORE_FAILED, "restore failure reported")
        check(write("more") < 0, "no writes while restore is pending")
        check(end() == T_RESTORED && (board.pasteboardItems ?? []).count == 2, "backup kept and restored later")
        // F07/F05 fill failure after a successful clear: the guard's own clear is
        // recorded at once, so settle neither reports "not written" nor "external".
        func items() -> [[(NSPasteboard.PasteboardType, Data?)]] {
            (board.pasteboardItems ?? []).map { item in item.types.map { ($0, item.data(forType: $0)) } }
        }
        func exact(_ a: [[(NSPasteboard.PasteboardType, Data?)]], _ b: [[(NSPasteboard.PasteboardType, Data?)]]) -> Bool {
            a.count == b.count && zip(a, b).allSatisfy { x, y in x.count == y.count && zip(x, y).allSatisfy { $0.0 == $1.0 && $0.1 == $1.1 } }
        }
        func lastModified() -> Bool {
            guard let ptr = tAudit() else { return false }
            let last = String(cString: ptr).split(separator: ";").last.map(String.init) ?? ""
            free(ptr)
            return last.hasSuffix(",1")
        }
        for (kind, name) in [(Int32(3), "multi-item"), (5, "empty"), (1, "non-text")] {
            _ = external(kind, nil)
            let original = items()
            check(begin(1) > 0, "fill-fail begin \(name)")
            check(tFailNextFill() == 0, "fill-fail armed \(name)")
            check(write("never filled") < 0, "fill-fail write refused \(name)")
            check(lastModified(), "fill-fail recorded as modified \(name)")
            check(write("again") < 0 && state() == 2, "fill-fail stops later writes \(name)")
            let code = end()
            check(code == T_RESTORED, "fill-fail restored, not \(code) \(name)")
            check(exact(items(), original), "fill-fail exact snapshot \(name)")
        }
        // fill failure, then a real external write: the external version stays
        _ = external(3, nil)
        check(begin(1) > 0 && tFailNextFill() == 0 && write("x") < 0, "fill-fail external setup")
        _ = external(0, "external after the failed fill")
        check(end() == T_EXTERNAL_KEPT && text() == "external after the failed fill", "fill-fail external kept")
        // fill failure, then the first restore fails: backup and fact kept, retry restores
        _ = external(3, nil)
        let original = items()
        check(begin(1) > 0 && tFailNextFill() == 0 && write("x") < 0, "fill-fail restore-retry setup")
        tFailNextRestore()
        check(end() == T_RESTORE_FAILED, "fill-fail first restore fails")
        check(end() == T_RESTORED && exact(items(), original), "fill-fail retry restores exactly")
        board.releaseGlobally()
        if failures.isEmpty { print("smoke guard: ok") } else { print("smoke guard FAILED: \(failures)"); exit(1) }
    }
}
