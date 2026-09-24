import Foundation
import Testing
@testable import DeckConnectorCore

private final class ProtocolCounters: @unchecked Sendable {
    private let lock = NSLock()
    private var starts = 0
    private var stops = 0
    func started() { lock.lock(); starts += 1; lock.unlock() }
    func stopped() { lock.lock(); stops += 1; lock.unlock() }
    func values() -> (Int, Int) { lock.lock(); defer { lock.unlock() }; return (starts, stops) }
}

private final class HangingURLProtocol: URLProtocol, @unchecked Sendable {
    static let counters = ProtocolCounters()
    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func startLoading() { Self.counters.started() }
    override func stopLoading() { Self.counters.stopped() }
}

private func hangingClient() throws -> DeckHTTPClient {
    let configuration = URLSessionConfiguration.ephemeral
    configuration.protocolClasses = [HangingURLProtocol.self]
    configuration.timeoutIntervalForResource = 45
    let credential = DeviceCredential(origin: "https://deck.test", fingerprint: String(repeating: "0", count: 64), hostId: "host", deviceId: "device", token: "token")
    return try DeckHTTPClient(credential: credential, configuration: configuration)
}

private func waitForStarts(_ count: Int) async {
    for _ in 0..<10_000 {
        if HangingURLProtocol.counters.values().0 >= count { return }
        await Task.yield()
    }
}

/// Answers every request with `status` and an error envelope.
private final class StatusURLProtocol: URLProtocol, @unchecked Sendable {
    nonisolated(unsafe) static var status = 200
    nonisolated(unsafe) static var code = "expired"
    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func startLoading() {
        let response = HTTPURLResponse(url: request.url!, statusCode: Self.status, httpVersion: "HTTP/1.1", headerFields: ["Content-Type": "application/json"])!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: Data("{\"error\":{\"code\":\"\(Self.code)\"}}".utf8))
        client?.urlProtocolDidFinishLoading(self)
    }
    override func stopLoading() {}
}

@Suite(.serialized)
struct HTTPClientTests {
@Test func goneCommandIsExpiredNotATransportError() async throws {
    let configuration = URLSessionConfiguration.ephemeral
    configuration.protocolClasses = [StatusURLProtocol.self]
    let credential = DeviceCredential(origin: "https://deck.test", fingerprint: String(repeating: "0", count: 64), hostId: "host", deviceId: "device", token: "token")
    let client = try DeckHTTPClient(credential: credential, configuration: configuration)
    defer { client.invalidate() }
    StatusURLProtocol.status = 410
    StatusURLProtocol.code = "expired"
    await #expect(throws: ConnectorError.commandExpired) { try await client.query(id: "op-1") }
    await #expect(throws: ConnectorError.commandExpired) {
        try await client.post(command: CommandRequest(id: "op-1", kind: "send-message", cardId: "card", expectedGeneration: "g", payload: ["text": .string("x")], seq: 1))
    }
    StatusURLProtocol.status = 404
    await #expect(throws: ConnectorError.commandNotFound) { try await client.query(id: "op-1") }
}

/// `Fixtures/http-status-map.json` is also read by the host's server tests:
/// every (status, code) the host answers a failed command query with, and
/// the ConnectorError the phone raises for it.
@Test func hostStatusMapMatchesThePhoneErrors() async throws {
    let url = try #require(Bundle.module.url(forResource: "http-status-map", withExtension: "json", subdirectory: "Fixtures"))
    let map = try #require(JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any])
    let responses = try #require(map["responses"] as? [[String: Any]])
    let configuration = URLSessionConfiguration.ephemeral
    configuration.protocolClasses = [StatusURLProtocol.self]
    let credential = DeviceCredential(origin: "https://deck.test", fingerprint: String(repeating: "0", count: 64), hostId: "host", deviceId: "device", token: "token")
    let client = try DeckHTTPClient(credential: credential, configuration: configuration)
    defer { client.invalidate() }
    for response in responses {
        let status = try #require(response["status"] as? Int)
        let code = try #require(response["code"] as? String)
        StatusURLProtocol.status = status
        StatusURLProtocol.code = code
        let raised: ConnectorError
        do { _ = try await client.query(id: "op-1"); Issue.record("\(status) \(code) succeeded"); continue }
        catch let error as ConnectorError { raised = error }
        let name: String = switch raised {
        case .revoked: "revoked"
        case .commandNotFound: "commandNotFound"
        case .commandExpired: "commandExpired"
        case .upgradeRequired: "upgradeRequired"
        case .conflict: "conflict"
        case .unsupportedTarget: "unsupportedTarget"
        case .transport: "transport"
        default: "\(raised)"
        }
        #expect(name == response["phone"] as? String, "\(status) \(code)")
    }
}

@Test func unsupportedTargetHasAnActionableClientError() async throws {
    let configuration = URLSessionConfiguration.ephemeral
    configuration.protocolClasses = [StatusURLProtocol.self]
    let credential = DeviceCredential(origin: "https://deck.test", fingerprint: String(repeating: "0", count: 64), hostId: "host", deviceId: "device", token: "token")
    let client = try DeckHTTPClient(credential: credential, configuration: configuration)
    defer { client.invalidate() }
    StatusURLProtocol.status = 400
    StatusURLProtocol.code = "unsupported-target"
    await #expect(throws: ConnectorError.unsupportedTarget) { try await client.buffer(cardID: "shell-card") }
}

@Test func cancellingSwiftTaskCancelsUnderlyingRequestAndResumesOnce() async throws {
    let client = try hangingClient()
    defer { client.invalidate() }
    let task = Task { try await client.snapshot() }
    await waitForStarts(1)
    task.cancel()
    await #expect(throws: CancellationError.self) { try await task.value }
    for _ in 0..<200 where HangingURLProtocol.counters.values().1 == 0 { try await Task.sleep(for: .milliseconds(1)) }
    #expect(HangingURLProtocol.counters.values().1 >= 1)
}

@Test func pendingRequestCapRejectsNinthOverlappingPoll() async throws {
    let baseline = HangingURLProtocol.counters.values().0
    let client = try hangingClient()
    defer { client.invalidate() }
    let tasks = (0..<8).map { _ in Task { try await client.snapshot() } }
    await waitForStarts(baseline + 8)
    await #expect(throws: ConnectorError.capacityExceeded) { try await client.snapshot() }
    tasks.forEach { $0.cancel() }
    for task in tasks { _ = try? await task.value }
}
}
