// swift-tools-version: 6.0
import PackageDescription

var products: [Product] = [
    .library(name: "DBCore", targets: ["DBCore"]),
]

var targets: [Target] = [
    // Platform-agnostic core: models, driver protocol, drivers.
    // Must stay free of AppKit/SwiftUI so it builds on Linux.
    .target(name: "DBCore"),
    .testTarget(name: "DBCoreTests", dependencies: ["DBCore"]),
]

#if os(macOS)
products.append(.executable(name: "DBGuiMac", targets: ["DBGuiMac"]))
targets.append(.executableTarget(name: "DBGuiMac", dependencies: ["DBCore"]))
#endif

let package = Package(
    name: "DBGui",
    platforms: [.macOS(.v15)],
    products: products,
    targets: targets
)
