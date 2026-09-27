import AppKit
import WebKit

final class DemoApp: NSObject, NSApplicationDelegate, NSWindowDelegate, WKScriptMessageHandler, WKNavigationDelegate {
    private var window: NSWindow!
    private var webView: WKWebView!
    private var busy = false
    private let arguments = Array(CommandLine.arguments.dropFirst())
    private var smokePath: String? { arguments.count == 5 && arguments[3] == "--smoke" ? arguments[4] : nil }

    func applicationDidFinishLaunching(_ notification: Notification) {
        guard arguments.count == 3 || smokePath != nil else { NSApp.terminate(nil); return }
        let menu = NSMenu()
        let appItem = NSMenuItem()
        menu.addItem(appItem)
        let appMenu = NSMenu()
        appMenu.addItem(withTitle: "Quit Bergamot Demo", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        appItem.submenu = appMenu
        let editItem = NSMenuItem()
        menu.addItem(editItem)
        let editMenu = NSMenu(title: "Edit")
        editMenu.addItem(withTitle: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
        editMenu.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
        editMenu.addItem(withTitle: "Select All", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
        editItem.submenu = editMenu
        NSApp.mainMenu = menu

        let controller = WKUserContentController()
        controller.add(self, name: "demo")
        let config = WKWebViewConfiguration()
        config.userContentController = controller
        webView = WKWebView(frame: .zero, configuration: config)
        webView.navigationDelegate = self
        webView.autoresizingMask = [.width, .height]

        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1160, height: 760),
                          styleMask: [.titled, .closable, .miniaturizable, .resizable],
                          backing: .buffered, defer: false)
        window.title = "Bergamot Translation Quality Lab"
        window.minSize = NSSize(width: 760, height: 520)
        window.center()
        window.delegate = self
        window.contentView = webView
        webView.frame = window.contentView!.bounds
        webView.loadFileURL(URL(fileURLWithPath: arguments[2]),
                            allowingReadAccessTo: URL(fileURLWithPath: arguments[2]).deletingLastPathComponent())
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        window.makeFirstResponder(webView)
        if smokePath != nil {
            webView.evaluateJavaScript("document.getElementById('source').value = 'The build completed successfully. Inspect AcmeWidgetQ7.'; document.getElementById('translate').click();", completionHandler: nil)
        }
    }

    func windowWillClose(_ notification: Notification) { NSApp.terminate(nil) }

    func userContentController(_ userContentController: WKUserContentController,
                               didReceive message: WKScriptMessage) {
        guard let body = message.body as? [String: Any], let action = body["action"] as? String else { return }
        if action == "copy", let text = body["text"] as? String {
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(text, forType: .string)
            return
        }
        guard action == "translate", !busy,
              let source = body["text"] as? String,
              let request = body["request"] as? Int else { return }
        if source.utf8.count > 32 * 1024 {
            deliver(["ok": false, "error": "text-too-large"], request: request)
            return
        }
        busy = true
        let binary = arguments[0]
        let model = arguments[1]
        DispatchQueue.global(qos: .userInitiated).async {
            let process = Process()
            process.executableURL = URL(fileURLWithPath: binary)
            process.arguments = [model]
            let input = Pipe()
            let output = Pipe()
            process.standardInput = input
            process.standardOutput = output
            process.standardError = FileHandle.nullDevice
            var response: [String: Any] = ["ok": false, "error": "translation-failed"]
            do {
                try process.run()
                input.fileHandleForWriting.write(source.data(using: .utf8) ?? Data())
                try? input.fileHandleForWriting.close()
                let data = output.fileHandleForReading.readDataToEndOfFile()
                process.waitUntilExit()
                if process.terminationStatus == 0,
                   let decoded = try JSONSerialization.jsonObject(with: data) as? [String: Any] {
                    response = decoded
                }
            } catch {
                response = ["ok": false, "error": "backend-start-failed"]
            }
            DispatchQueue.main.async {
                self.busy = false
                self.deliver(response, request: request)
            }
        }
    }

    private func deliver(_ response: [String: Any], request: Int) {
        var payload = response
        payload["request"] = request
        guard let data = try? JSONSerialization.data(withJSONObject: payload, options: [.fragmentsAllowed]),
              let json = String(data: data, encoding: .utf8) else { return }
        webView.evaluateJavaScript("window.deckDemoComplete(\(json))") { _, _ in
            guard let path = self.smokePath else { return }
            self.webView.evaluateJavaScript("document.getElementById('result').textContent.includes('AcmeWidgetQ7') && document.getElementById('source') instanceof HTMLTextAreaElement && !document.getElementById('source').readOnly") { value, _ in
                let passed = (value as? Bool == true) && (response["ok"] as? Bool == true)
                self.webView.takeSnapshot(with: nil) { image, _ in
                    if let data = image?.tiffRepresentation,
                       let bitmap = NSBitmapImageRep(data: data),
                       let png = bitmap.representation(using: .png, properties: [:]) {
                        try? png.write(to: URL(fileURLWithPath: path + ".png"))
                    }
                    try? (passed ? "PASS\n" : "FAIL\n").write(toFile: path, atomically: true, encoding: .utf8)
                    NSApp.terminate(nil)
                }
            }
        }
    }
}

let app = NSApplication.shared
app.setActivationPolicy(.regular)
let delegate = DemoApp()
app.delegate = delegate
app.run()
