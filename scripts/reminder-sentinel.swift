// Test-owned foreground sentinel; no global input or accessibility calls.
import AppKit

let output = CommandLine.arguments[1]
let app = NSApplication.shared
app.setActivationPolicy(.regular)
let window = NSWindow(contentRect: NSRect(x: 100, y: 100, width: 360, height: 160), styleMask: [.titled, .closable], backing: .buffered, defer: false)
window.title = "Deck Reminder Test Sentinel"
window.makeKeyAndOrderFront(nil)
app.activate(ignoringOtherApps: true)
var samples = [[String: Any]]()
let timer = Timer.scheduledTimer(withTimeInterval: 0.25, repeats: true) { _ in
    samples.append(["at": Int64(Date().timeIntervalSince1970 * 1000), "sentinelFrontmost": NSWorkspace.shared.frontmostApplication?.processIdentifier == ProcessInfo.processInfo.processIdentifier, "pid": ProcessInfo.processInfo.processIdentifier])
    if samples.count > 2400 { samples.removeFirst() }
    if let bytes = try? JSONSerialization.data(withJSONObject: samples) { try? bytes.write(to: URL(fileURLWithPath: output), options: .atomic) }
}
app.run()
