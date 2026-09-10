// In-process, on-device speech only. C entry points enqueue work on the main
// actor; snapshots cross a synchronous callback and never enter logs/files.
// One capture owner, bounded recording/input buffer, token-scoped cancellation.
import Foundation
import AppKit
@preconcurrency import AVFoundation
import Speech

public typealias DeckSpeechCallback = @convention(c) (UInt64, UnsafePointer<CChar>, UnsafePointer<CChar>, UnsafePointer<CChar>) -> Void

@available(macOS 12.0, *)
@MainActor private var current: Capture?

@available(macOS 12.0, *)
@MainActor private final class Capture {
    let id: UInt64
    let callback: DeckSpeechCallback
    let engine = AVAudioEngine()
    var task: Task<Void, Never>?
    var results: Task<Void, Never>?
    var deadline: Task<Void, Never>?
    var stopRecognition: (() -> Void)?
    var cancelRecognition: (() -> Void)?
    var tapInstalled = false
    var observers: [NSObjectProtocol] = []
    var ended = false
    var stopping = false
    var text = ""
    var stable = ""

    init(id: UInt64, callback: @escaping DeckSpeechCallback) {
        self.id = id
        self.callback = callback
        // Observe preparation too: hiding while a model/permission is pending
        // must not let that completion start a hidden microphone later.
        for name in [NSApplication.didHideNotification, NSWindow.didMiniaturizeNotification] {
            observers.append(NotificationCenter.default.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in
                Task { @MainActor in self?.stop() }
            })
        }
    }

    var active: Bool { !ended && current === self }

    func publish(_ state: String, _ code: String = "") {
        guard current === self else { return }
        state.withCString { s in text.withCString { t in code.withCString { c in callback(id, s, t, c) } } }
    }

    func releaseMic() {
        engine.stop()
        if tapInstalled { engine.inputNode.removeTap(onBus: 0); tapInstalled = false }
    }

    func finish(_ state: String, _ code: String = "") {
        guard active else { return }
        ended = true
        releaseMic()
        deadline?.cancel()
        task?.cancel()
        results?.cancel()
        cancelRecognition?()
        cancelRecognition = nil
        stopRecognition = nil
        for observer in observers { NotificationCenter.default.removeObserver(observer) }
        observers.removeAll()
        publish(state, code)
    }

    func stop() {
        guard active, !stopping else { return }
        // Stopping while permission/model preparation is pending cancels it.
        guard let stopRecognition else { finish("ready"); return }
        stopping = true
        releaseMic()
        publish("stopping")
        deadline?.cancel()
        deadline = Task { [weak self] in
            try? await Task.sleep(nanoseconds: 15_000_000_000)
            guard !Task.isCancelled else { return }
            self?.finish("error", "finalize-timeout")
        }
        stopRecognition()
    }

    func run(locale: String) {
        publish("preparing")
        task = Task { [weak self] in
            guard let self else { return }
            let allowed = await AVCaptureDevice.requestAccess(for: .audio)
            guard self.active else { return }
            guard allowed else { self.finish("error", "microphone-denied"); return }
            do {
                #if compiler(>=6.2)
                if #available(macOS 26.0, *), SpeechTranscriber.isAvailable,
                   let supported = await SpeechTranscriber.supportedLocale(equivalentTo: Locale(identifier: locale)) {
                    guard self.active else { return }
                    try await self.modern(locale: supported)
                    return
                }
                #endif
                try await self.legacy(locale: Locale(identifier: locale))
            } catch {
                self.finish("error", "recognition-failed")
            }
        }
    }

    func beginAudio(_ format: AVAudioFormat, handler: @escaping AVAudioNodeTapBlock) throws {
        guard active else { return }
        guard format.sampleRate > 0, format.channelCount > 0 else {
            finish("error", "microphone-unavailable"); return
        }
        engine.inputNode.installTap(onBus: 0, bufferSize: 4096, format: format, block: handler)
        tapInstalled = true
        engine.prepare()
        try engine.start()
        observers.append(NotificationCenter.default.addObserver(forName: .AVAudioEngineConfigurationChange, object: engine, queue: .main) { [weak self] _ in
            Task { @MainActor in self?.finish("error", "microphone-unavailable") }
        })
        publish("recording")
        deadline = Task { [weak self] in
            try? await Task.sleep(nanoseconds: 300_000_000_000)
            guard !Task.isCancelled else { return }
            self?.stop()
        }
    }

    func legacy(locale: Locale) async throws {
        guard active else { return }
        guard let recognizer = SFSpeechRecognizer(locale: locale), recognizer.supportsOnDeviceRecognition else {
            finish("error", "local-unavailable"); return
        }
        let authorization = await withCheckedContinuation { continuation in
            SFSpeechRecognizer.requestAuthorization { continuation.resume(returning: $0) }
        }
        guard active else { return }
        guard authorization == .authorized else { finish("error", "speech-denied"); return }
        guard recognizer.isAvailable else { finish("error", "local-unavailable"); return }
        let request = SFSpeechAudioBufferRecognitionRequest()
        request.requiresOnDeviceRecognition = true
        request.shouldReportPartialResults = true
        let recognition = recognizer.recognitionTask(with: request) { [weak self] result, error in
            // Copy text before hopping off the framework callback queue.
            let transcript = result?.bestTranscription.formattedString
            let final = result?.isFinal ?? false
            let failed = error != nil
            Task { @MainActor in
                guard let self, self.active else { return }
                if let transcript {
                    self.text = transcript
                    if transcript.utf8.count > 65_536 { self.finish("error", "text-limit"); return }
                    self.publish(self.stopping ? "stopping" : "recording")
                }
                if final { self.finish("ready") }
                else if failed { self.finish("error", "recognition-failed") }
            }
        }
        cancelRecognition = { recognition.cancel() }
        stopRecognition = { request.endAudio() }
        try beginAudio(engine.inputNode.outputFormat(forBus: 0)) { buffer, _ in request.append(buffer) }
    }

    #if compiler(>=6.2)
    @available(macOS 26.0, *)
    func modern(locale: Locale) async throws {
        let transcriber = SpeechTranscriber(locale: locale, preset: .progressiveTranscription)
        if let download = try await AssetInventory.assetInstallationRequest(supporting: [transcriber]) {
            guard active else { return }
            publish("downloading")
            try await download.downloadAndInstall()
        }
        guard active else { return }
        let analyzer = SpeechAnalyzer(modules: [transcriber])
        let inputFormat = engine.inputNode.outputFormat(forBus: 0)
        guard inputFormat.sampleRate > 0, inputFormat.channelCount > 0,
              let format = await SpeechAnalyzer.bestAvailableAudioFormat(compatibleWith: [transcriber]),
              let converter = AVAudioConverter(from: inputFormat, to: format) else {
            finish("error", "microphone-unavailable"); return
        }
        guard active else { await analyzer.cancelAndFinishNow(); return }
        var continuation: AsyncStream<AnalyzerInput>.Continuation!
        let stream = AsyncStream<AnalyzerInput>(bufferingPolicy: .bufferingOldest(32)) { continuation = $0 }
        let input = continuation!
        cancelRecognition = { input.finish(); Task { await analyzer.cancelAndFinishNow() } }
        stopRecognition = { [weak self] in
            input.finish()
            Task { @MainActor in
                do {
                    try await analyzer.finalizeAndFinishThroughEndOfInput()
                    await self?.results?.value
                    self?.finish("ready")
                } catch { self?.finish("error", "recognition-failed") }
            }
        }
        results = Task { [weak self] in
            do {
                for try await result in transcriber.results {
                    guard let self, self.active else { return }
                    let segment = String(result.text.characters)
                    if result.isFinal { self.stable += segment }
                    self.text = self.stable + (result.isFinal ? "" : segment)
                    if self.text.utf8.count > 65_536 { self.finish("error", "text-limit"); return }
                    self.publish(self.stopping ? "stopping" : "recording")
                }
            } catch { self?.finish("error", "recognition-failed") }
        }
        try await analyzer.start(inputSequence: stream)
        guard active else { await analyzer.cancelAndFinishNow(); return }
        try beginAudio(inputFormat) { [weak self] buffer, _ in
            // Conversion produces an owned buffer; never retain the tap's reused input.
            let capacity = AVAudioFrameCount(ceil(Double(buffer.frameLength) * format.sampleRate / inputFormat.sampleRate)) + 32
            guard let converted = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: capacity) else { return }
            var supplied = false
            var error: NSError?
            let status = converter.convert(to: converted, error: &error) { _, flag in
                if supplied { flag.pointee = .noDataNow; return nil }
                supplied = true; flag.pointee = .haveData; return buffer
            }
            var failed = error != nil || status == .error
            if !failed, converted.frameLength > 0 {
                if case .dropped = input.yield(AnalyzerInput(buffer: converted)) { failed = true }
            }
            if failed { Task { @MainActor in self?.finish("error", "audio-overrun") } }
        }
    }
    #endif
}

@_cdecl("deck_speech_start")
public func deckSpeechStart(_ id: UInt64, _ locale: UnsafePointer<CChar>, _ callback: @escaping DeckSpeechCallback) {
    let language = String(cString: locale)
    DispatchQueue.main.async {
        if #available(macOS 12.0, *) {
            current?.finish("cancelled")
            let capture = Capture(id: id, callback: callback)
            current = capture
            capture.run(locale: language)
        } else {
            "error".withCString { s in "".withCString { t in "local-unavailable".withCString { c in callback(id, s, t, c) } } }
        }
    }
}

@_cdecl("deck_speech_stop")
public func deckSpeechStop(_ id: UInt64) {
    DispatchQueue.main.async { if #available(macOS 12.0, *), current?.id == id { current?.stop() } }
}

@_cdecl("deck_speech_cancel")
public func deckSpeechCancel(_ id: UInt64) {
    DispatchQueue.main.async {
        if #available(macOS 12.0, *), id == 0 || current?.id == id { current?.finish("cancelled"); current = nil }
    }
}
