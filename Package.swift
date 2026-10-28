// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "AnywhereHost",
    platforms: [.macOS(.v14)],
    products: [.executable(name: "AnywhereHost", targets: ["AnywhereHost"])],
    targets: [.executableTarget(name: "AnywhereHost", path: "Sources/AnywhereHost")]
)
