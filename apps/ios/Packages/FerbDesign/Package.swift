// swift-tools-version: 5.9
import PackageDescription

// FerbDesign: the glass lab's phone design (~/Desktop/ferb-glass-lab at 23a0fde) as a SwiftUI library for FERB's
// iPhone app (stage 4, slice I3): tokens, the sky, the season layer, the phone's pieces, the twelve screens as
// stateless views, and the home screen's words. FerbGallery: every token, piece and screen with invented data.
// The macOS platform is here only so the pure arithmetic can also be tested on the Mac host.
let package = Package(
    name: "FerbDesign",
    platforms: [.iOS(.v17), .macOS(.v14)],
    products: [
        .library(name: "FerbDesign", targets: ["FerbDesign"]),
        .library(name: "FerbGallery", targets: ["FerbGallery"]),
    ],
    targets: [
        .target(name: "FerbDesign", resources: [.copy("Resources/corpus.json"), .copy("Resources/zone-places.json")]),
        .target(name: "FerbGallery", dependencies: ["FerbDesign"]),
        .testTarget(name: "FerbDesignTests", dependencies: ["FerbDesign", "FerbGallery"], exclude: ["Screenshots"]),
    ]
)
