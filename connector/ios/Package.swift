// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "DeckConnector",
    platforms: [.macOS(.v13), .iOS(.v17)],
    products: [
        .library(name: "DeckConnectorCore", targets: ["DeckConnectorCore"]),
    ],
    targets: [
        .target(name: "DeckConnectorCore"),
        .testTarget(name: "DeckConnectorCoreTests", dependencies: ["DeckConnectorCore"]),
    ]
)
