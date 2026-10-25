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
                                                            .product(name: "Sparkle", package: "Sparkle")]),
        .target(name: "AudioCaptureTestSupport", path: "Tests/AudioCaptureTestSupport"),
        .testTarget(name: "NotchCoreTests", dependencies: ["NotchCore", "FERBNotch", "AudioCapture", "AudioCaptureTestSupport"])
    ],
    swiftLanguageModes: [.v5]
)
