// swift-tools-version: 5.9
import PackageDescription

// The phone library for the iPhone app (stage 4, kb PHN-446, PHN-492): engine/crates/ferb-mobile-ffi built for iOS as
// the static archives in FerbMobileFFI.xcframework, and the Swift bindings UniFFI generates for it (module
// FerbMobile). Both are made by apps/ios/tools/build-rust.sh into this folder (git-ignored); nothing here is written
// by hand except this manifest and the tests.
let package = Package(
    name: "FerbMobile",
    platforms: [.iOS(.v17)],
    products: [.library(name: "FerbMobile", targets: ["FerbMobile"])],
    targets: [
        .binaryTarget(name: "FerbMobileFFI", path: "FerbMobileFFI.xcframework"),
        .target(
            name: "FerbMobile",
            dependencies: ["FerbMobileFFI"],
            path: "Sources/FerbMobile",
            // What the Rust archive calls into besides libSystem (ios_api_check.py lists the imports): SQLCipher's
            // CommonCrypto and CFSTR (Security, CoreFoundation), iroh's network monitor (Network), the nameservers through libresolv (n0-dns-resolver,
            // kb PHN-478).
            linkerSettings: [
                .linkedFramework("Security"),
                .linkedFramework("CoreFoundation"),
                .linkedFramework("Network"),
                .linkedLibrary("resolv"),
            ]
        ),
        .testTarget(name: "FerbMobileTests", dependencies: ["FerbMobile"], path: "Tests/FerbMobileTests"),
    ]
)
