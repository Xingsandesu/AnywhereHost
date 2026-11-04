import Foundation
import Testing
@testable import AnywhereHost

struct HostPairingTests {
    @Test func pairingLinkPreservesTheSystemComputerName() throws {
        let name = "工作室的 Mac + Mini & 测试 #1"
        let value: JSONValue = .object([
            "hostId": .string(String(repeating: "a", count: 64)),
            "ticket": .string(String(repeating: "b", count: 64)),
            "relayUrls": .array([.string("https://relay.example.com")])
        ])
        let link = try HostClient.pairingLink(value, computerName: name)
        let components = try #require(URLComponents(string: link))
        #expect(components.queryItems?.first { $0.name == "hostName" }?.value == name)
        #expect(components.fragment == nil)
        #expect(components.queryItems?.contains { $0.name == "relays" } == false)
        #expect(throws: HostFailure.self) { try HostClient.pairingLink(value, computerName: " \n ") }
    }
}
