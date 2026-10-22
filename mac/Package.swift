// swift-tools-version:5.10
// The Mac shell: menu bar app, drag detection, conversion wheel and
// progress HUD. All conversion work happens in the bundled `veer` helper,
// which runs `veer-vision` for text recognition and background removal.
//
// VeerKit holds the app's pure logic (protocol models, line framing, wheel
// geometry, job progress, diagnostics log) so it can be unit-tested.
// VeerUI draws and drives the wheel; the app and the Wheel Lab share it.
import PackageDescription

let package = Package(
    name: "Veer",
    platforms: [.macOS(.v14)],
    targets: [
        .target(name: "VeerKit", path: "Sources/VeerKit"),
        .target(name: "VeerUI", dependencies: ["VeerKit"], path: "Sources/VeerUI"),
        .executableTarget(name: "Veer", dependencies: ["VeerKit", "VeerUI"], path: "Sources/Veer"),
        // A storybook for the wheel: every style and state, drawn by the
        // app's own code, with a live skin editor. `swift run VeerLab`.
        .executableTarget(name: "VeerLab", dependencies: ["VeerKit", "VeerUI"], path: "Sources/VeerLab"),
        // Text recognition, subject masks and searchable PDFs, run by the
        // engine as a helper program (see its main.swift for the contract).
        .executableTarget(name: "veer-vision", path: "Sources/veer-vision"),
        .testTarget(
            name: "VeerKitTests",
            dependencies: ["VeerKit"],
            path: "Tests/VeerKitTests",
            resources: [.copy("Fixtures")]
        ),
    ],
    swiftLanguageVersions: [.v5]
)
