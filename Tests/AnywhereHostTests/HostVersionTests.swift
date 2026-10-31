import Testing
@testable import AnywhereHost

struct HostVersionTests {
    @Test func packagedVersionAndDevelopmentFallback() {
        #expect(HostVersion.display(info: ["CFBundleShortVersionString": "0.1.4", "CFBundleVersion": "7"]) == "0.1.4 (7)")
        #expect(HostVersion.display(info: [:]) == "开发构建")
    }
}
