// swift-tools-version:5.10
// The Mac shell: menu bar app, drag detection, conversion wheel and
// progress HUD. All conversion work happens in the bundled `radlet` helper,
// which runs `radlet-vision` for text recognition and background removal.
//
// RadletKit holds the app's pure logic (protocol models, line framing, wheel
// geometry, job progress, diagnostics log) so it can be unit-tested.
// RadletUI draws and drives the wheel; the app and the Wheel Lab share it.
// Sparkle (MIT) installs updates; it's pinned so every build is the same.
import PackageDescription

let package = Package(
    name: "Radlet",
    platforms: [.macOS(.v14)],
    dependencies: [
        .package(url: "https://github.com/sparkle-project/Sparkle", exact: "2.10.1"),
    ],
    targets: [
        .target(name: "RadletKit", path: "Sources/RadletKit"),
        .target(name: "RadletUI", dependencies: ["RadletKit"], path: "Sources/RadletUI"),
        .executableTarget(
            name: "Radlet",
            dependencies: ["RadletKit", "RadletUI", .product(name: "Sparkle", package: "Sparkle")],
            path: "Sources/Radlet"
        ),
        // A storybook for the wheel: every style and state, drawn by the
        // app's own code, with a live skin editor. `swift run RadletLab`.
        .executableTarget(name: "RadletLab", dependencies: ["RadletKit", "RadletUI"], path: "Sources/RadletLab"),
        // Text recognition, subject masks and searchable PDFs, run by the
        // engine as a helper program (see its main.swift for the contract).
        .executableTarget(name: "radlet-vision", path: "Sources/radlet-vision"),
        .testTarget(
            name: "RadletKitTests",
            dependencies: ["RadletKit"],
            path: "Tests/RadletKitTests",
            resources: [.copy("Fixtures")]
        ),
    ],
    swiftLanguageVersions: [.v5]
)
