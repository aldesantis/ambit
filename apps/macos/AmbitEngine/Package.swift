// swift-tools-version: 6.2
//
// The ambit engine for the macOS app: the Rust library (crates/ambit-ffi) as a binary target, and
// its generated Swift bindings as the `AmbitEngine` module. Both are produced by
// ../scripts/build-engine.sh and are not checked in; run it before building this package.

import PackageDescription

let package = Package(
    name: "AmbitEngine",
    platforms: [.macOS("26.0")],
    products: [
        .library(name: "AmbitEngine", targets: ["AmbitEngine"]),
    ],
    targets: [
        .binaryTarget(name: "AmbitEngineFFI", path: "AmbitEngineFFI.xcframework"),
        .target(
            name: "AmbitEngine",
            dependencies: ["AmbitEngineFFI"],
            path: "Sources/AmbitEngine",
            // Generated code: it is checked by UniFFI, not written for Swift 6 strict concurrency.
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .testTarget(
            name: "AmbitEngineTests",
            dependencies: ["AmbitEngine"],
            path: "Tests/AmbitEngineTests"
        ),
    ]
)
