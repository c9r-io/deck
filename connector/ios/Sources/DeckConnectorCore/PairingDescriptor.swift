import Foundation

public struct HTTPSOrigin: Equatable, Sendable {
    public let url: URL
    public let host: String
    public let port: Int

    public init(_ raw: String) throws {
        guard let components = URLComponents(string: raw),
              components.scheme?.lowercased() == "https",
              let host = components.host?.lowercased(), !host.isEmpty,
              components.user == nil, components.password == nil,
              components.query == nil, components.fragment == nil,
              components.path.isEmpty || components.path == "/" else { throw ConnectorError.invalidOrigin }
        let port = components.port ?? 443
        guard (1...65_535).contains(port), let url = components.url else { throw ConnectorError.invalidOrigin }
        self.url = url
        self.host = host
        self.port = port
    }

    public func matches(_ candidate: URL) -> Bool {
        guard candidate.scheme?.lowercased() == "https", candidate.host?.lowercased() == host else { return false }
        return (candidate.port ?? 443) == port
    }

    public func endpoint(_ path: String) throws -> URL {
        guard path.hasPrefix("/"), !path.hasPrefix("//") else { throw ConnectorError.invalidOrigin }
        var components = URLComponents(url: url, resolvingAgainstBaseURL: false)
        components?.percentEncodedPath = path
        components?.query = nil
        components?.fragment = nil
        guard let result = components?.url, matches(result) else { throw ConnectorError.invalidOrigin }
        return result
    }
}

public enum PairingDescriptor {
    public static func parse(_ raw: String, now: Date = Date()) throws -> PairingPayload {
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed.utf8.count <= ConnectorLimits.pairingDescriptorBytes,
              let components = URLComponents(string: trimmed),
              components.scheme == "deck-connector", components.host == "pair",
              components.user == nil, components.password == nil,
              components.path.isEmpty, components.fragment == nil,
              let queryItems = components.queryItems, queryItems.count == 1,
              queryItems[0].name == "data", let encoded = queryItems[0].value,
              encoded.utf8.count <= ConnectorLimits.pairingDescriptorBytes,
              encoded.range(of: "^[A-Za-z0-9_-]+$", options: .regularExpression) != nil,
              let data = Data(base64URL: encoded),
              data.count <= ConnectorLimits.pairingDescriptorBytes,
              let payload = try? JSONDecoder().decode(PairingPayload.self, from: data),
              payload.version == 1,
              !payload.hostId.isEmpty, !payload.hostName.isEmpty, !payload.code.isEmpty,
              payload.fingerprint.range(of: "^[0-9a-f]{64}$", options: .regularExpression) != nil else {
            throw ConnectorError.invalidPairingDescriptor
        }
        _ = try HTTPSOrigin(payload.origin)
        guard payload.expiresAt >= Int64(now.timeIntervalSince1970) else { throw ConnectorError.expiredPairingDescriptor }
        return payload
    }
}

extension Data {
    init?(base64URL: String) {
        var value = base64URL.replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/")
        value.append(String(repeating: "=", count: (4 - value.count % 4) % 4))
        self.init(base64Encoded: value)
    }

    public var base64URLEncodedString: String {
        base64EncodedString().replacingOccurrences(of: "+", with: "-").replacingOccurrences(of: "/", with: "_").replacingOccurrences(of: "=", with: "")
    }
}
