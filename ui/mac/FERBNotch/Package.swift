// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "FERBNotch",
    platforms: [.macOS(.v14)],
    products: [.executable(name: "FERBNotch", targets: ["FERBNotch"])],
    dependencies: [
        // Automatic updates (L34 Phase G). Pinned exactly; a bump is its own reviewed change.
        .package(url: "https://github.com/sparkle-project/Sparkle", exact: "2.10.0"),
    ],
    targets: [
        .target(name: "NotchCore"),
        .target(name: "AudioCapture"),
        .executableTarget(name: "FERBNotch", dependencies: ["NotchCore", "AudioCapture",
                                                            .product(name: "Sparkle", package: "Sparkle")],
                          resources: [.copy("Resources/ferb-mark.svg"), .copy("Resources/Sounds")],
                          // The intro playground (Sources/FERBNotch/Playground, PLAYGROUND.md) is compiled into debug
                          // builds only: FERB.app is built release (scripts/build-app.sh) and carries none of it.
                          swiftSettings: [.define("FERB_PLAYGROUND", .when(configuration: .debug))]),
        .testTarget(name: "NotchCoreTests", dependencies: ["NotchCore", "FERBNotch", "AudioCapture"],
                    swiftSettings: [.define("FERB_PLAYGROUND", .when(configuration: .debug))])
    ],
    swiftLanguageModes: [.v5]
)
