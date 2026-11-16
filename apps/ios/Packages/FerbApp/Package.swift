// swift-tools-version: 5.9
import PackageDescription

// FerbApp: the iPhone app itself (stage 4, slice I5; the twin of Android's :app). FerbAppModel: what the screens show
// and every action (the app model over a PhoneApi seam, the chat's rows, questions, tasks, files, Watch live, FERB's
// settings, the lock), with the app's one string catalog. FerbAppUI: the screens, drawn with FerbDesign, the system
// pieces (the scanner, notifications, the privacy cover) and the app's root view. The Xcode project's FERB target is
// only `@main` over FerbAppUI.
let package = Package(
    name: "FerbApp",
    defaultLocalization: "en",
    platforms: [.iOS(.v17)],
    products: [
        .library(name: "FerbAppModel", targets: ["FerbAppModel"]),
        .library(name: "FerbAppUI", targets: ["FerbAppUI"]),
    ],
    dependencies: [
        .package(path: "../FerbCore"),
        .package(path: "../FerbDesign"),
        .package(path: "../FerbMobile"),
    ],
    targets: [
        .target(
            name: "FerbAppModel",
            dependencies: [
                .product(name: "FerbCore", package: "FerbCore"),
                .product(name: "FerbMobile", package: "FerbMobile"),
                .product(name: "FerbDesign", package: "FerbDesign"),
            ],
            resources: [.process("Resources")]
        ),
        .target(
            name: "FerbAppUI",
            dependencies: [
                "FerbAppModel",
                .product(name: "FerbCore", package: "FerbCore"),
                .product(name: "FerbMobile", package: "FerbMobile"),
                .product(name: "FerbDesign", package: "FerbDesign"),
            ]
        ),
        .testTarget(name: "FerbAppTests", dependencies: ["FerbAppModel", "FerbAppUI"], exclude: ["Screenshots"]),
    ]
)
