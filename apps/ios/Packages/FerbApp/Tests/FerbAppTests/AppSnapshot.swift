#if canImport(UIKit)
import SwiftUI
import UIKit
import XCTest

// A copy of FerbDesign's screenshot helper (Tests/FerbDesignTests/Snapshot.swift), comparing the app's own screens with
// committed goldens: its own (Screenshots/app) or the design's (designGolden). A small screenshot helper (swift-snapshot-testing is not available offline, and a dependency for this is not
// needed). A view is drawn by SwiftUI's ImageRenderer at scale 1 (one pixel a point), in the light scheme, and
// compared with a committed PNG; under 0.1 % of pixels may differ (Android's threshold, kb PHN-455).
// FERB_RECORD_SNAPSHOTS=1 (TEST_RUNNER_FERB_RECORD_SNAPSHOTS=1 through xcodebuild) writes the goldens instead. On a
// failure the actual picture and a diff are written to a temporary folder and attached to the test.
//
// Why ImageRenderer: the package's tests run without a host app, and there a UIWindow has no render server, so
// UIView.drawHierarchy returns nothing ("Render server returned error"; tried first). ImageRenderer draws SwiftUI
// itself (text, shapes, canvases, blur, drawingGroup) but leaves out views UIKit draws (a scroll view, a text field,
// materials): the pages therefore draw a list that fits without a scroll view (FitOrScroll), as the lab does, and
// the editable composer and glass panels are not shot.

enum AppSnapshot {
    static var recording: Bool { ProcessInfo.processInfo.environment["FERB_RECORD_SNAPSHOTS"] == "1" }

    /// The changed-pixel share allowed (Roborazzi's changeThreshold on Android).
    static let threshold = 0.001

    /// The folder of the committed goldens, next to this file.
    static func golden(_ path: String, file: StaticString = #filePath) -> URL {
        URL(fileURLWithPath: "\(file)").deletingLastPathComponent().appendingPathComponent("Screenshots").appendingPathComponent(path)
    }

    /// Draws `view` at `size` points, 1 pixel a point. A nil size takes the view's own size.
    @MainActor
    static func render<V: View>(_ view: V, size: CGSize?) -> UIImage {
        let content = view.environment(\.colorScheme, .light).environment(\.displayScale, 1)
        let renderer = ImageRenderer(content: AnyView(content))
        if let size { renderer.proposedSize = ProposedViewSize(size) }
        renderer.scale = 1
        renderer.isOpaque = false
        return renderer.uiImage!
    }

    /// RGBA bytes in sRGB, 8 bits a channel.
    static func pixels(_ image: CGImage) -> (width: Int, height: Int, bytes: [UInt8]) {
        let w = image.width, h = image.height
        var bytes = [UInt8](repeating: 0, count: w * h * 4)
        bytes.withUnsafeMutableBytes { buf in
            let ctx = CGContext(data: buf.baseAddress, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4,
                                space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
            ctx.draw(image, in: CGRect(x: 0, y: 0, width: w, height: h))
        }
        return (w, h, bytes)
    }

    static func png(_ image: CGImage) -> Data { UIImage(cgImage: image).pngData()! }

    /// FerbDesign's committed golden at `path` (its Tests/FerbDesignTests/Screenshots): the app's screen must draw it.
    static func designGolden(_ path: String, file: StaticString = #filePath) -> URL {
        URL(fileURLWithPath: "\(file)").deletingLastPathComponent()
            .appendingPathComponent("../../../FerbDesign/Tests/FerbDesignTests/Screenshots").appendingPathComponent(path).standardized
    }

    /// Renders and compares with (or records) the golden at `path` under Screenshots/ (or at `at`, never recorded).
    @MainActor
    static func assert<V: View>(_ view: V, size: CGSize?, path: String, at: URL? = nil, allowed: Double = threshold,
                                file: StaticString = #filePath, line: UInt = #line) {
        let actual = render(view, size: size).cgImage!
        let url = at ?? golden(path)
        if recording && at == nil {
            try? FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            try! png(actual).write(to: url)
            print("recorded \(url.path)")
            return
        }
        guard let data = try? Data(contentsOf: url), let stored = UIImage(data: data)?.cgImage else {
            XCTFail("no golden at \(url.path): record with FERB_RECORD_SNAPSHOTS=1", file: file, line: line)
            return
        }
        let a = pixels(actual), b = pixels(stored)
        guard a.width == b.width, a.height == b.height else {
            fail(actual, nil, path, "size \(a.width)x\(a.height), golden \(b.width)x\(b.height)", file: file, line: line)
            return
        }
        var changed = 0
        var diff = [UInt8](repeating: 0, count: a.bytes.count)
        for i in stride(from: 0, to: a.bytes.count, by: 4) {
            let same = a.bytes[i] == b.bytes[i] && a.bytes[i + 1] == b.bytes[i + 1] && a.bytes[i + 2] == b.bytes[i + 2] && a.bytes[i + 3] == b.bytes[i + 3]
            if same {
                let grey = UInt8((Int(b.bytes[i]) + Int(b.bytes[i + 1]) + Int(b.bytes[i + 2])) / 3 / 4)
                diff[i] = grey; diff[i + 1] = grey; diff[i + 2] = grey; diff[i + 3] = 255
            } else {
                changed += 1
                diff[i] = 255; diff[i + 1] = 0; diff[i + 2] = 40; diff[i + 3] = 255
            }
        }
        let share = Double(changed) / Double(a.width * a.height)
        if share > allowed {
            let provider = CGDataProvider(data: Data(diff) as CFData)!
            let diffImage = CGImage(width: a.width, height: a.height, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: a.width * 4,
                                    space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedLast.rawValue),
                                    provider: provider, decode: nil, shouldInterpolate: false, intent: .defaultIntent)
            fail(actual, diffImage, path, String(format: "%d pixels differ (%.3f %%, allowed %.1f %%)", changed, share * 100, allowed * 100),
                 file: file, line: line)
        }
    }

    private static func fail(_ actual: CGImage, _ diff: CGImage?, _ path: String, _ why: String, file: StaticString, line: UInt) {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("FerbAppSnapshots")
        let name = path.replacingOccurrences(of: "/", with: "-").replacingOccurrences(of: ".png", with: "")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let actualURL = dir.appendingPathComponent("\(name)-actual.png")
        try? png(actual).write(to: actualURL)
        var note = "actual: \(actualURL.path)"
        if let diff {
            let diffURL = dir.appendingPathComponent("\(name)-diff.png")
            try? png(diff).write(to: diffURL)
            note += ", diff: \(diffURL.path)"
        }
        XCTContext.runActivity(named: path) { activity in
            activity.add(XCTAttachment(image: UIImage(cgImage: actual)))
            if let diff { activity.add(XCTAttachment(image: UIImage(cgImage: diff))) }
        }
        XCTFail("\(path): \(why); \(note)", file: file, line: line)
    }
}
#endif
