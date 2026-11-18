// The Macs this phone is paired with. One Keychain item per Mac (account
// `host:<host id>`), each holding that Mac's origin, certificate pin, device
// id and token; nothing is shared between them. The single item an earlier
// build kept (account `paired-device`) becomes that Mac's item on the first
// read and is removed only after the new item was written, so a failed write
// loses nothing. An item that does not decode, or whose host id differs from
// its account, is left untouched and not listed.
import Foundation
import Security

public protocol CredentialVault: Sendable {
    func all() throws -> [String: Data]
    func set(_ data: Data, account: String) throws
    func remove(account: String) throws
}

public struct HostRoster: Sendable {
    public static let legacyAccount = "paired-device"
    static let prefix = "host:"
    private let vault: any CredentialVault

    public init(vault: any CredentialVault) { self.vault = vault }

    /// Paired Macs, by name and then host id.
    public func load() throws -> [DeviceCredential] {
        let items = try vault.all()
        var hosts: [String: DeviceCredential] = [:]
        for (account, data) in items where account.hasPrefix(Self.prefix) {
            guard let credential = try? JSONDecoder().decode(DeviceCredential.self, from: data),
                  Self.prefix + credential.hostId == account else { continue }
            hosts[credential.hostId] = credential
        }
        if let data = items[Self.legacyAccount],
           let legacy = try? JSONDecoder().decode(DeviceCredential.self, from: data) {
            if hosts[legacy.hostId] == nil {
                hosts[legacy.hostId] = legacy
                // keep the old item when the new one cannot be written
                if (try? save(legacy)) != nil { try? vault.remove(account: Self.legacyAccount) }
            } else {
                try? vault.remove(account: Self.legacyAccount)
            }
        }
        return hosts.values.sorted {
            let a = ($0.hostName ?? "").lowercased(), b = ($1.hostName ?? "").lowercased()
            return a == b ? $0.hostId < $1.hostId : a < b
        }
    }

    /// Adds a Mac, or replaces the pairing with the same Mac.
    public func save(_ credential: DeviceCredential) throws {
        try vault.set(try JSONEncoder().encode(credential), account: Self.prefix + credential.hostId)
    }

    public func remove(hostID: String) throws {
        try vault.remove(account: Self.prefix + hostID)
        // an old item for this Mac that could not be migrated must not bring it back
        if let data = try vault.all()[Self.legacyAccount],
           (try? JSONDecoder().decode(DeviceCredential.self, from: data))?.hostId == hostID {
            try vault.remove(account: Self.legacyAccount)
        }
    }

    /// A pairing code scanned for a Mac this phone is already paired with:
    /// the same host and the same pinned certificate, so only where to reach
    /// it can differ. Returns the kept pairing at the code's address; nil
    /// when the code is for another Mac or for a Mac that was reset (another
    /// certificate), which is an ordinary new pairing. The caller proves the
    /// kept token still works there before storing the result.
    public static func readdressed(_ hosts: [DeviceCredential], hostId: String, fingerprint: String, origin: String, hostName: String?) -> DeviceCredential? {
        guard let known = hosts.first(where: { $0.hostId == hostId }), known.fingerprint == fingerprint else { return nil }
        return DeviceCredential(origin: origin, fingerprint: known.fingerprint, hostId: known.hostId, deviceId: known.deviceId,
                                token: known.token, hostName: hostName ?? known.hostName)
    }

    /// The Mac to show: the one last chosen while it is still paired,
    /// otherwise the first listed. Never a Mac chosen by reachability.
    public static func current(_ hosts: [DeviceCredential], preferred: String?) -> DeviceCredential? {
        hosts.first { $0.hostId == preferred } ?? hosts.first
    }
}

public struct KeychainVault: CredentialVault, Sendable {
    private let service: String

    public init(service: String = "io.c9r.deck.connector") { self.service = service }

    public func all() throws -> [String: Data] {
        var query = base
        query[kSecReturnAttributes as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitAll
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        if status == errSecItemNotFound { return [:] }
        guard status == errSecSuccess, let rows = result as? [[String: Any]] else { throw failure(status) }
        var items: [String: Data] = [:]
        for row in rows {
            guard let account = row[kSecAttrAccount as String] as? String else { continue }
            var one = base
            one[kSecAttrAccount as String] = account
            one[kSecReturnData as String] = true
            one[kSecMatchLimit as String] = kSecMatchLimitOne
            var value: CFTypeRef?
            let read = SecItemCopyMatching(one as CFDictionary, &value)
            if read == errSecItemNotFound { continue }
            guard read == errSecSuccess, let data = value as? Data else { throw failure(read) }
            items[account] = data
        }
        return items
    }

    public func set(_ data: Data, account: String) throws {
        var item = base
        item[kSecAttrAccount as String] = account
        var add = item
        add[kSecValueData as String] = data
        add[kSecAttrAccessible as String] = kSecAttrAccessibleWhenUnlockedThisDeviceOnly
        let status = SecItemAdd(add as CFDictionary, nil)
        if status == errSecDuplicateItem {
            let update: [String: Any] = [
                kSecValueData as String: data,
                kSecAttrAccessible as String: kSecAttrAccessibleWhenUnlockedThisDeviceOnly,
            ]
            let updated = SecItemUpdate(item as CFDictionary, update as CFDictionary)
            guard updated == errSecSuccess else { throw failure(updated) }
        } else if status != errSecSuccess {
            throw failure(status)
        }
    }

    public func remove(account: String) throws {
        var item = base
        item[kSecAttrAccount as String] = account
        let status = SecItemDelete(item as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else { throw failure(status) }
    }

    private var base: [String: Any] {
        [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: service]
    }

    private func failure(_ status: OSStatus) -> ConnectorError {
        .transport(SecCopyErrorMessageString(status, nil) as String? ?? "Keychain error")
    }
}
