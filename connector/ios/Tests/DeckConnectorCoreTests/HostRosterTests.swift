import Foundation
import Testing
@testable import DeckConnectorCore

private final class MemoryVault: CredentialVault, @unchecked Sendable {
    var items: [String: Data] = [:]
    var refuseWrites = false
    func all() throws -> [String: Data] { items }
    func set(_ data: Data, account: String) throws {
        if refuseWrites { throw ConnectorError.transport("refused") }
        items[account] = data
    }
    func remove(account: String) throws { items[account] = nil }
}

private func credential(_ host: String, device: String = "d1", name: String? = nil) -> DeviceCredential {
    DeviceCredential(origin: "https://192.168.1.2:47731", fingerprint: String(repeating: "a", count: 64),
                     hostId: host, deviceId: device, token: "t-\(host)-\(device)", hostName: name)
}

/// What a build before several Macs wrote: one item, no host name.
private let legacyJSON = #"{"origin":"https://192.168.1.2:47731","fingerprint":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","hostId":"host_old","deviceId":"d0","token":"t-old"}"#

@Test func earlierSingleItemBecomesThatMacsItem() throws {
    let vault = MemoryVault()
    vault.items[HostRoster.legacyAccount] = Data(legacyJSON.utf8)
    let hosts = try HostRoster(vault: vault).load()
    #expect(hosts.map(\.hostId) == ["host_old"])
    #expect(hosts[0].token == "t-old" && hosts[0].deviceId == "d0" && hosts[0].hostName == nil)
    #expect(Set(vault.items.keys) == ["host:host_old"])
    #expect(try HostRoster(vault: vault).load() == hosts)
}

@Test func earlierItemStaysWhenTheNewOneCannotBeWritten() throws {
    let vault = MemoryVault()
    vault.items[HostRoster.legacyAccount] = Data(legacyJSON.utf8)
    vault.refuseWrites = true
    #expect(try HostRoster(vault: vault).load().map(\.hostId) == ["host_old"])
    #expect(Set(vault.items.keys) == [HostRoster.legacyAccount])
    vault.refuseWrites = false
    #expect(try HostRoster(vault: vault).load().map(\.hostId) == ["host_old"])
    #expect(Set(vault.items.keys) == ["host:host_old"])
}

@Test func aSecondMacIsAddedAndTheSameMacIsReplaced() throws {
    let vault = MemoryVault()
    let roster = HostRoster(vault: vault)
    try roster.save(credential("host_b", name: "Work"))
    try roster.save(credential("host_a", name: "home"))
    #expect(try roster.load().map(\.hostId) == ["host_a", "host_b"])
    try roster.save(credential("host_b", device: "d2", name: "Work"))
    let hosts = try roster.load()
    #expect(hosts.count == 2)
    #expect(hosts.first { $0.hostId == "host_b" }?.deviceId == "d2")
    #expect(hosts.first { $0.hostId == "host_a" }?.token == "t-host_a-d1")
}

@Test func removingOneMacLeavesTheOther() throws {
    let vault = MemoryVault()
    let roster = HostRoster(vault: vault)
    try roster.save(credential("host_a"))
    try roster.save(credential("host_b"))
    try roster.remove(hostID: "host_a")
    #expect(try roster.load().map(\.hostId) == ["host_b"])
    // an old item that could not be migrated does not bring a removed Mac back
    vault.items[HostRoster.legacyAccount] = Data(legacyJSON.utf8)
    try roster.remove(hostID: "host_old")
    #expect(try roster.load().map(\.hostId) == ["host_b"])
}

@Test func anItemThatIsNotItsAccountsMacIsNotListedOrTouched() throws {
    let vault = MemoryVault()
    let roster = HostRoster(vault: vault)
    try roster.save(credential("host_a"))
    vault.items["host:host_x"] = try JSONEncoder().encode(credential("host_a"))
    vault.items["host:host_y"] = Data("not json".utf8)
    vault.items["something-else"] = try JSONEncoder().encode(credential("host_z"))
    #expect(try roster.load().map(\.hostId) == ["host_a"])
    #expect(vault.items.count == 4)
}

@Test func theShownMacIsTheOneLastChosenWhileStillPaired() {
    let hosts = [credential("host_a"), credential("host_b")]
    #expect(HostRoster.current(hosts, preferred: "host_b")?.hostId == "host_b")
    #expect(HostRoster.current(hosts, preferred: "host_gone")?.hostId == "host_a")
    #expect(HostRoster.current(hosts, preferred: nil)?.hostId == "host_a")
    #expect(HostRoster.current([], preferred: "host_a") == nil)
}

@Test func aCodeForAnAlreadyPairedMacOnlyChangesWhereToReachIt() {
    let known = DeviceCredential(origin: "https://192.168.31.16:47631", fingerprint: String(repeating: "a", count: 64),
                                 hostId: "H1", deviceId: "D1", token: "T1", hostName: "Work Mac")
    let moved = HostRoster.readdressed([known], hostId: "H1", fingerprint: known.fingerprint,
                                       origin: "https://192.168.31.44:47631", hostName: nil)
    #expect(moved == DeviceCredential(origin: "https://192.168.31.44:47631", fingerprint: known.fingerprint,
                                      hostId: "H1", deviceId: "D1", token: "T1", hostName: "Work Mac"))
    // a Mac that was reset has another certificate: an ordinary new pairing
    #expect(HostRoster.readdressed([known], hostId: "H1", fingerprint: String(repeating: "b", count: 64),
                                   origin: "https://192.168.31.44:47631", hostName: nil) == nil)
    // another Mac
    #expect(HostRoster.readdressed([known], hostId: "H2", fingerprint: known.fingerprint,
                                   origin: "https://192.168.31.44:47631", hostName: nil) == nil)
}
