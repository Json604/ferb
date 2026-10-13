// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "FERBNotch",
    platforms: [.macOS(.v14)],
    products: [.executable(name: "FERBNotch", targets: ["FERBNotch"])],
    targets: [
        .target(name: "NotchCore"),
        .target(name: "AudioCapture"),
        .executableTarget(name: "FERBNotch", dependencies: ["NotchCore", "AudioCapture"]),
        .target(name: "AudioCaptureTestSupport", path: "Tests/AudioCaptureTestSupport"),
        .testTarget(name: "NotchCoreTests", dependencies: ["NotchCore", "FERBNotch", "AudioCapture", "AudioCaptureTestSupport"])
    ],
    swiftLanguageModes: [.v5]
)
