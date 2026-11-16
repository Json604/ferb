// swift-tools-version: 5.9
import PackageDescription

// FerbCore: the iPhone app's side of the phone library (stage 4, slice I4; the twin of Android's :core): FerbPhone,
// the one object the app uses (the store and the link started from the Vault, callbacks delivered on the main actor,
// typed outcomes), the Vault (Keychain items and the excluded folder), the three signing keys (the Secure Enclave on a
// phone, software keys on the simulator and in tests, one wire form), the system prompt and the phone's signals.
let package = Package(
    name: "FerbCore",
    platforms: [.iOS(.v17)],
    products: [.library(name: "FerbCore", targets: ["FerbCore"])],
    dependencies: [.package(path: "../FerbMobile")],
    targets: [
        .target(name: "FerbCore", dependencies: [.product(name: "FerbMobile", package: "FerbMobile")]),
        .testTarget(name: "FerbCoreTests", dependencies: ["FerbCore"]),
    ]
)
