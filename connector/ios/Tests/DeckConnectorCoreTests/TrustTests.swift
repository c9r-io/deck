import CryptoKit
import Foundation
import Security
import Testing
@testable import DeckConnectorCore

private let ipLeafDER = "MIIDQjCCAiqgAwIBAgIUYpldqHsl5eDyP6tHCM3Zlahgh5YwDQYJKoZIhvcNAQELBQAwFjEUMBIGA1UEAwwLMTkyLjE2OC4xLjQwHhcNMjYwOTE5MDcyMDQ4WhcNMjcwOTE5MDcyMDQ4WjAWMRQwEgYDVQQDDAsxOTIuMTY4LjEuNDCCASIwDQYJKoZIhvcNAQEBBQADggEPADCCAQoCggEBAJ+c3oKlekwf2dLbXUbOG2UAfFA6OVt0tPeJ/FWGoffaSmmYeIgUJ6P/C0Rgta5/JD9JcPWTnYGf+l5MuLpA3ENILXFyxsTmk3Bv3ZPE2lkxzGEvceuAm8Y3G7CuAvG/ugIzYNkk1n5Qdl4hA4PvD1zinSPg2XXaDBnD4Z8Z7tzQvjDCIntoR60aZnaGK3kAF/olO7mM+2OADHcfol1lt6rz9LnBZpsgH556DkeRUwwwcpihQ67Q9lQaEngTRGg87ynFm9PR6VUnt+bhaGiwgzWoN7ZGZ4Ds6j6GQ8QpCPuDTbQTVYJp0r+JRXflvJ6o2UUlUtbvhaO0t1DqTFnJpJMCAwEAAaOBhzCBhDAdBgNVHQ4EFgQU6Vmm2YBQUXKFRkneA3YHbHvgd9YwHwYDVR0jBBgwFoAU6Vmm2YBQUXKFRkneA3YHbHvgd9YwDwYDVR0RBAgwBocEwKgBBDAMBgNVHRMBAf8EAjAAMA4GA1UdDwEB/wQEAwIFoDATBgNVHSUEDDAKBggrBgEFBQcDATANBgkqhkiG9w0BAQsFAAOCAQEALQtj5qrKLUccxxfWyWXd0erUebY/+gmJZNO0YqkoOPpHhUi/s8ddhJs0Ac0ajofD0T8ccrQIOg4qNv0mgClQWSEBm6wOvjV4XAe5mYdbe2D7nTe1VIvrxCHoC91k09Q94DkgmVosxtDMmWVGgOQUMXAe/JGNc3fUXLhJAdSAHfBdsIGd8FCJUT8Pio7hkvghr19QlNbAfuvYwYU04xCPcc644Yrjlr5z4ouzQDKFbWzXJ9oya/RqKOsEYYQIbRIUT6RuYBDuM3oD7hLX+SeoylK6XW+0Xhs4aEhFTg7tkZxVvNt6PImh84EB2MBPrUZs53s0Yw8VPWA1isGd1WWc2A=="

private func fixtureTrust(verifyAt: TimeInterval) throws -> (SecTrust, String) {
    let data = try #require(Data(base64Encoded: ipLeafDER))
    let certificate = try #require(SecCertificateCreateWithData(nil, data as CFData))
    var trust: SecTrust?
    #expect(SecTrustCreateWithCertificates(certificate, SecPolicyCreateSSL(true, "192.168.1.4" as CFString), &trust) == errSecSuccess)
    let value = try #require(trust)
    SecTrustSetVerifyDate(value, Date(timeIntervalSince1970: verifyAt) as CFDate)
    let pin = SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
    return (value, pin)
}

@Test func pinnedSelfSignedIPLeafIsSoleAnchorWithHostnameAndValidityChecks() throws {
    let (valid, pin) = try fixtureTrust(verifyAt: 1_798_761_600) // 2027-01-01, inside fixture validity.
    #expect(PinnedTrustEvaluator.evaluate(trust: valid, host: "192.168.1.4", fingerprint: pin))

    let (wrongHost, _) = try fixtureTrust(verifyAt: 1_798_761_600)
    #expect(!PinnedTrustEvaluator.evaluate(trust: wrongHost, host: "192.168.1.5", fingerprint: pin))

    let (wrongPin, _) = try fixtureTrust(verifyAt: 1_798_761_600)
    #expect(!PinnedTrustEvaluator.evaluate(trust: wrongPin, host: "192.168.1.4", fingerprint: String(repeating: "0", count: 64)))

    let (expired, _) = try fixtureTrust(verifyAt: 1_830_297_600) // 2028-01-01.
    #expect(!PinnedTrustEvaluator.evaluate(trust: expired, host: "192.168.1.4", fingerprint: pin))
}
