// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "AnywhereHost",
    platforms: [.macOS(.v14)],
    products: [.executable(name: "AnywhereHost", targets: ["AnywhereHost"])],
    dependencies: [.package(url: "https://github.com/sparkle-project/Sparkle", exact: "2.10.1")],
    targets: [
        .executableTarget(name: "AnywhereHost", dependencies: [.product(name: "Sparkle", package: "Sparkle")], path: "Sources/AnywhereHost"),
        .testTarget(name: "AnywhereHostTests", dependencies: ["AnywhereHost"])
    ]
)
