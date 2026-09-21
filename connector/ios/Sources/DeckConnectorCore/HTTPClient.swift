@preconcurrency import Foundation
import CryptoKit
import Security

public protocol CommandTransport: Sendable {
    func post(command: CommandRequest) async throws -> CommandResult
    func query(id: String) async throws -> CommandResult
}

public final class DeckHTTPClient: NSObject, URLSessionDelegate, URLSessionTaskDelegate, URLSessionDataDelegate, CommandTransport, @unchecked Sendable {
    private final class CancellationBox: @unchecked Sendable {
        private let lock = NSLock()
        private var task: URLSessionDataTask?
        private var cancelled = false
        func install(_ task: URLSessionDataTask) -> Bool { lock.lock(); defer { lock.unlock() }; self.task = task; return cancelled }
        func cancel() -> URLSessionDataTask? { lock.lock(); defer { lock.unlock() }; cancelled = true; return task }
    }

    private static let maximumPendingRequests = 8
    private struct Pending {
        var data = Data()
        var response: URLResponse?
        let limit: Int
        let continuation: CheckedContinuation<(Data, URLResponse), Error>
    }

    private let credential: DeviceCredential
    public var hostID: String { credential.hostId }
    private let origin: HTTPSOrigin
    private let fingerprint: String
    private let lock = NSLock()
    private var pending: [Int: Pending] = [:]
    private var session: URLSession!

    private static func makeConfiguration() -> URLSessionConfiguration {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.urlCache = nil
        configuration.httpCookieStorage = nil
        configuration.urlCredentialStorage = nil
        configuration.requestCachePolicy = .reloadIgnoringLocalCacheData
        configuration.httpCookieAcceptPolicy = .never
        configuration.httpShouldSetCookies = false
        configuration.waitsForConnectivity = true
        configuration.timeoutIntervalForRequest = 30
        configuration.timeoutIntervalForResource = 45
        return configuration
    }

    public convenience init(credential: DeviceCredential) throws {
        try self.init(credential: credential, configuration: nil)
    }

    init(credential: DeviceCredential, configuration: URLSessionConfiguration?) throws {
        self.credential = credential
        self.origin = try HTTPSOrigin(credential.origin)
        guard credential.fingerprint.range(of: "^[0-9a-f]{64}$", options: .regularExpression) != nil else {
            throw ConnectorError.invalidPairingDescriptor
        }
        self.fingerprint = credential.fingerprint
        super.init()
        self.session = URLSession(configuration: configuration ?? Self.makeConfiguration(), delegate: self, delegateQueue: nil)
    }

    public convenience init(pairing: PairingPayload) throws {
        try self.init(credential: DeviceCredential(origin: pairing.origin, fingerprint: pairing.fingerprint, hostId: pairing.hostId, deviceId: "pairing", token: ""))
    }

    deinit { session?.invalidateAndCancel() }

    public func invalidate() { session.invalidateAndCancel() }

    public func pair(code: String, deviceName: String) async throws -> PairResponse {
        let body = try encode(PairRequest(code: code, deviceName: deviceName))
        return try await request(path: "/v1/pair", method: "POST", body: body, authenticated: false)
    }

    public func snapshot() async throws -> Snapshot {
        try await request(path: "/v1/snapshot", method: "GET", body: nil, authenticated: true)
    }

    public func output(cardID: String) async throws -> TerminalOutput {
        let output: TerminalOutput = try await request(path: "/v1/cards/\(encodePath(cardID))/output", method: "GET", body: nil, authenticated: true)
        try WireValidator.validate(output)
        return output
    }

    public func buffer(cardID: String) async throws -> CardBuffer {
        let value: CardBuffer = try await request(path: "/v1/cards/\(encodePath(cardID))/buffer", method: "GET", body: nil, authenticated: true, responseLimit: ConnectorLimits.serializedBufferBytes)
        try WireValidator.validate(value)
        return value
    }

    public func post(command: CommandRequest) async throws -> CommandResult {
        try WireValidator.validate(command)
        let body = try encode(command)
        return try await request(path: "/v1/commands", method: "POST", body: body, authenticated: true)
    }

    public func query(id: String) async throws -> CommandResult {
        try await request(path: "/v1/commands/\(encodePath(id))", method: "GET", body: nil, authenticated: true)
    }

    private func request<Response: Decodable>(path: String, method: String, body: Data?, authenticated: Bool, responseLimit: Int = ConnectorLimits.responseBytes) async throws -> Response {
        let url = try origin.endpoint(path)
        guard origin.matches(url) else { throw ConnectorError.invalidOrigin }
        if let body, body.count > ConnectorLimits.requestBytes { throw ConnectorError.requestTooLarge }
        var request = URLRequest(url: url, cachePolicy: .reloadIgnoringLocalCacheData, timeoutInterval: 30)
        request.httpMethod = method
        request.httpBody = body
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        if body != nil { request.setValue("application/json", forHTTPHeaderField: "Content-Type") }
        if authenticated {
            guard !credential.token.isEmpty, origin.matches(url) else { throw ConnectorError.invalidOrigin }
            request.setValue("Bearer \(credential.token)", forHTTPHeaderField: "Authorization")
        }

        let (data, response) = try await load(request, limit: responseLimit)
        guard let http = response as? HTTPURLResponse else { throw ConnectorError.invalidResponse }
        if http.statusCode == 401 || http.statusCode == 403 { throw ConnectorError.revoked }
        if http.statusCode == 404, path.hasPrefix("/v1/commands/") { throw ConnectorError.commandNotFound }
        if http.statusCode == 410, path.hasPrefix("/v1/commands") { throw ConnectorError.commandExpired }
        if http.statusCode == 426 { throw ConnectorError.upgradeRequired }
        guard (200..<300).contains(http.statusCode) else {
            let code = (try? JSONDecoder().decode(ErrorEnvelope.self, from: data).error.code) ?? "http-\(http.statusCode)"
            if http.statusCode == 409 || http.statusCode == 412 { throw ConnectorError.conflict(code) }
            if code == "unsupported-target" { throw ConnectorError.unsupportedTarget }
            throw ConnectorError.transport("Deck host error: \(code)")
        }
        do { return try JSONDecoder().decode(Response.self, from: data) }
        catch { throw ConnectorError.invalidResponse }
    }

    private func encode<Value: Encodable>(_ value: Value) throws -> Data {
        let data = try JSONEncoder().encode(value)
        guard data.count <= ConnectorLimits.requestBytes else { throw ConnectorError.requestTooLarge }
        return data
    }

    private func encodePath(_ value: String) -> String {
        value.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed.subtracting(CharacterSet(charactersIn: "/?#"))) ?? ""
    }

    private func load(_ request: URLRequest, limit: Int) async throws -> (Data, URLResponse) {
        let cancellation = CancellationBox()
        return try await withTaskCancellationHandler(operation: {
            try await withCheckedThrowingContinuation { continuation in
                let task = session.dataTask(with: request)
                lock.lock()
                guard pending.count < Self.maximumPendingRequests else {
                    lock.unlock()
                    continuation.resume(throwing: ConnectorError.capacityExceeded)
                    return
                }
                pending[task.taskIdentifier] = Pending(limit: limit, continuation: continuation)
                lock.unlock()
                if cancellation.install(task) {
                    task.cancel()
                    finish(taskID: task.taskIdentifier, result: .failure(CancellationError()))
                } else {
                    task.resume()
                }
            }
        }, onCancel: {
            if let task = cancellation.cancel() {
                task.cancel()
                self.finish(taskID: task.taskIdentifier, result: .failure(CancellationError()))
            }
        })
    }

    public func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive response: URLResponse, completionHandler: @escaping @Sendable (URLSession.ResponseDisposition) -> Void) {
        lock.lock()
        let limit = pending[dataTask.taskIdentifier]?.limit ?? ConnectorLimits.responseBytes
        lock.unlock()
        if response.expectedContentLength > Int64(limit) {
            finish(taskID: dataTask.taskIdentifier, result: .failure(ConnectorError.responseTooLarge))
            completionHandler(.cancel)
            return
        }
        lock.lock()
        if var item = pending[dataTask.taskIdentifier] { item.response = response; pending[dataTask.taskIdentifier] = item }
        lock.unlock()
        completionHandler(.allow)
    }

    public func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive data: Data) {
        lock.lock()
        guard var item = pending[dataTask.taskIdentifier] else { lock.unlock(); return }
        if item.data.count + data.count > item.limit {
            lock.unlock()
            dataTask.cancel()
            finish(taskID: dataTask.taskIdentifier, result: .failure(ConnectorError.responseTooLarge))
            return
        }
        item.data.append(data)
        pending[dataTask.taskIdentifier] = item
        lock.unlock()
    }

    public func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: (any Error)?) {
        if let error { finish(taskID: task.taskIdentifier, result: .failure(ConnectorError.transport(error.localizedDescription))) }
        else {
            lock.lock()
            let item = pending[task.taskIdentifier]
            lock.unlock()
            guard let item, let response = item.response else {
                finish(taskID: task.taskIdentifier, result: .failure(ConnectorError.invalidResponse)); return
            }
            finish(taskID: task.taskIdentifier, result: .success((item.data, response)))
        }
    }

    public func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse, newRequest request: URLRequest, completionHandler: @escaping @Sendable (URLRequest?) -> Void) {
        finish(taskID: task.taskIdentifier, result: .failure(ConnectorError.redirected))
        completionHandler(nil)
    }

    public func urlSession(_ session: URLSession, didReceive challenge: URLAuthenticationChallenge, completionHandler: @escaping @Sendable (URLSession.AuthChallengeDisposition, URLCredential?) -> Void) {
        evaluate(challenge, completionHandler: completionHandler)
    }

    public func urlSession(_ session: URLSession, task: URLSessionTask, didReceive challenge: URLAuthenticationChallenge, completionHandler: @escaping @Sendable (URLSession.AuthChallengeDisposition, URLCredential?) -> Void) {
        evaluate(challenge, completionHandler: completionHandler)
    }

    private func evaluate(_ challenge: URLAuthenticationChallenge, completionHandler: @escaping @Sendable (URLSession.AuthChallengeDisposition, URLCredential?) -> Void) {
        guard challenge.protectionSpace.authenticationMethod == NSURLAuthenticationMethodServerTrust,
              challenge.protectionSpace.host.lowercased() == origin.host,
              challenge.protectionSpace.port == origin.port,
              let trust = challenge.protectionSpace.serverTrust,
              PinnedTrustEvaluator.evaluate(trust: trust, host: origin.host, fingerprint: fingerprint) else {
            completionHandler(.cancelAuthenticationChallenge, nil); return
        }
        completionHandler(.useCredential, URLCredential(trust: trust))
    }

    private func finish(taskID: Int, result: Result<(Data, URLResponse), Error>) {
        lock.lock()
        let item = pending.removeValue(forKey: taskID)
        lock.unlock()
        guard let item else { return }
        item.continuation.resume(with: result)
    }
}

enum PinnedTrustEvaluator {
    static func evaluate(trust: SecTrust, host: String, fingerprint: String) -> Bool {
        guard let chain = SecTrustCopyCertificateChain(trust) as? [SecCertificate], let leaf = chain.first else { return false }
        let leafData = SecCertificateCopyData(leaf) as Data
        let actual = SHA256.hash(data: leafData).map { String(format: "%02x", $0) }.joined()
        guard actual == fingerprint else { return false }

        let policy = SecPolicyCreateSSL(true, host as CFString)
        guard SecTrustSetPolicies(trust, policy) == errSecSuccess,
              SecTrustSetAnchorCertificates(trust, [leaf] as CFArray) == errSecSuccess,
              SecTrustSetAnchorCertificatesOnly(trust, true) == errSecSuccess else { return false }
        var error: CFError?
        return SecTrustEvaluateWithError(trust, &error)
    }
}
