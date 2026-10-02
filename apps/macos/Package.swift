// swift-tools-version: 6.0
import PackageDescription

// The SwiftUI frontend. All database logic lives in the Rust core (../../crates);
// run ../../scripts/build-core.sh first to produce DBCoreFFI.xcframework + bindings.
let package = Package(
    name: "DBGui",
    platforms: [.macOS(.v15)],
    products: [
        .executable(name: "DBGuiMac", targets: ["DBGuiMac"]),
    ],
    targets: [
        // Rust static library + C header (generated).
        .binaryTarget(name: "DBCoreFFIBinary", path: "Frameworks/DBCoreFFI.xcframework"),
        // UniFFI-generated Swift API over the binary (generated, do not edit).
        .target(
            name: "DBCoreFFI",
            dependencies: ["DBCoreFFIBinary"],
            swiftSettings: [.swiftLanguageMode(.v5)],
            // System frameworks the Rust static lib needs (tokio-postgres → whoami).
            linkerSettings: [.linkedFramework("SystemConfiguration")]
        ),
        // Swift-facing models + DatabaseDriver protocol; adapts the generated API.
        .target(name: "DBKit", dependencies: ["DBCoreFFI"]),
        .executableTarget(name: "DBGuiMac", dependencies: ["DBKit"]),
        .testTarget(name: "DBKitTests", dependencies: ["DBKit"]),
    ]
)
