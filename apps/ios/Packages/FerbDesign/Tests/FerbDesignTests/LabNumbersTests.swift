import CoreGraphics
import Foundation
import SwiftUI
import XCTest
@testable import FerbDesign

/// The Swift tokens against Android's checked-in copy of the glass lab's numbers
/// (apps/android/design/src/test/resources/lab/lab-numbers.json, made by apps/android/tools/lab_numbers.py with the lab
/// commit it read): the two phones read the same numbers, so neither can drift from the lab or from the other.
final class LabNumbersTests: XCTestCase {
    static let repo = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()

    static func androidLab(_ name: String) throws -> [String: Any] {
        let url = repo.appendingPathComponent("apps/android/design/src/test/resources/lab/\(name)")
        return try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any], "\(url.path)")
    }

    private var lab: [String: Any] { get throws { try Self.androidLab("lab-numbers.json") } }

    /// "#RRGGBB" and the alpha of a SwiftUI colour, in sRGB.
    static func hex(_ c: Color) -> (String, Double) {
        let cg = c.resolve(in: EnvironmentValues()).cgColor
        let srgb = cg.converted(to: CGColorSpace(name: CGColorSpace.sRGB)!, intent: .defaultIntent, options: nil) ?? cg
        let p = srgb.components ?? [0, 0, 0, 1]
        let rgb = p.count >= 3 ? Array(p[0..<3]) : [p[0], p[0], p[0]]
        let s = rgb.map { String(format: "%02X", Int(($0 * 255).rounded())) }.joined()
        return ("#" + s, Double(p.last ?? 1))
    }

    private func check(_ section: String, _ swift: [String: Color]) throws {
        let want = try XCTUnwrap(try lab[section] as? [String: [Any]])
        XCTAssertEqual(Set(want.keys), Set(swift.keys), "\(section): the same names")
        for (name, v) in want {
            guard let c = swift[name] else { continue }
            let (hex, alpha) = Self.hex(c)
            XCTAssertEqual(hex, v[0] as? String, "\(section).\(name) colour")
            XCTAssertEqual(alpha, (v[1] as? NSNumber)?.doubleValue ?? -1, accuracy: 0.002, "\(section).\(name) alpha")
        }
    }

    // decision: PHN-416, PHN-457, PHN-569
    func testThePhonesInkIsTheLabsInk() throws {
        try check("ink", ["black": Ink.black, "white": Ink.white, "detail": Ink.detail, "faint": Ink.faint, "fill": Ink.fill,
                          "line": Ink.line, "darkFill": Ink.darkFill, "onDark": Ink.onDark, "amber": Ink.amber, "red": Ink.red])
    }

    func testTheWarmRampAndLampsAreTheLabs() throws {
        try check("tone", ["ink0": Tone.ink0, "ink1": Tone.ink1, "ink2": Tone.ink2, "ink3": Tone.ink3, "ink4": Tone.ink4,
                           "line": Tone.line, "text": Tone.text, "textDim": Tone.textDim, "textFaint": Tone.textFaint,
                           "fill1": Tone.fill1, "fill2": Tone.fill2, "fill3": Tone.fill3, "well": Tone.well, "hairline": Tone.hairline])
        try check("lamp", ["amber": Lamp.amber, "green": Lamp.green, "red": Lamp.red])
    }

    func testRadiiAndSpacingAreTheBooks() throws {
        let r = try XCTUnwrap(try lab["radius"] as? [String: NSNumber])
        XCTAssertEqual(Set(r.keys), Set(Radius.all.keys))
        for (k, v) in r { XCTAssertEqual(Double(Radius.all[k] ?? -1), v.doubleValue, accuracy: 1e-9, "radius \(k)") }
        let space = try XCTUnwrap(try lab["space"] as? [NSNumber]).map(\.doubleValue)
        XCTAssertEqual(space, Space.all.map(Double.init))
    }

    // decision: PHN-409
    func testTheTypeIsTheBooksAndTrackingFollowsTheLab() throws {
        let type = try XCTUnwrap(try lab["type"] as? [String: [String: Any]])
        XCTAssertEqual(Set(type.keys), Set(MacType.all.keys))
        for (name, role) in type {
            let ours = try XCTUnwrap(MacType.all[name])
            XCTAssertEqual(Double(ours.size), (role["size"] as? NSNumber)?.doubleValue ?? -1, "type \(name) size")
            XCTAssertEqual(ours.weight, role["weight"] as? String, "type \(name) weight")
            XCTAssertEqual(Double(ours.tracking), (role["tracking"] as? NSNumber)?.doubleValue ?? 0, accuracy: 1e-9, "type \(name) tracking")
        }
        // Mobile.swift View.type: -0.4 from 20 points up, -0.1 below (Android's FerbType.style holds the same).
        XCTAssertEqual(PhoneType.tracking(20), -0.4)
        XCTAssertEqual(PhoneType.tracking(19.9), -0.1)
        XCTAssertEqual(PhoneType.tracking(42), -0.4)
    }

    func testTheFileBadgesAreTheBooks() throws {
        let b = try XCTUnwrap(try lab["fileBadge"] as? [String: String])
        XCTAssertEqual(Self.hex(FileKind.pdf.badge).0, b["pdf"])
        XCTAssertEqual(Self.hex(FileKind.document.badge).0, b["document"])
        XCTAssertEqual(Self.hex(FileKind.sheet.badge).0, b["sheet"])
        XCTAssertEqual(Self.hex(FileKind.slides.badge).0, b["slides"])
        XCTAssertEqual(Self.hex(FileKind.other.badge).0, b["other"])
        XCTAssertEqual(Self.hex(FileKind.image.badge).0, b["other"])
    }

    /// The iOS copy, Android's numbers and Android's sky goldens all come from one lab commit, and each lab file both
    /// recorded has the same hash.
    // decision: PHN-416, PHN-457
    func testBothPhonesCopiedTheSameLabCommit() throws {
        XCTAssertEqual(try lab["labCommit"] as? String, LabSource.commit)
        XCTAssertEqual(try Self.androidLab("sky-golden.json")["labCommit"] as? String, LabSource.commit)
        let files = try XCTUnwrap(try lab["files"] as? [String: String])
        XCTAssertFalse(files.isEmpty)
        for (path, hash) in files { XCTAssertEqual(LabSource.files[path], hash, "\(path): Android and iOS copied different versions") }
    }

    /// Every colour literal in the lab's phone files is drawn by this package, or is listed here with why not.
    // decision: PHN-416, PHN-412
    func testEveryColourTheLabsPhoneDrawsIsAccountedFor() throws {
        let notOnIPhone: [String: String] = [
            "#A3ABB5": "Weather.swift weathered(): the day veil under weather; weather is paused (PHN-412)",
            "#1A1E24": "Weather.swift weathered(): the night veil under weather; paused (PHN-412)",
            "#CFE0FF": "Weather.swift lightning(): storms are weather; paused (PHN-412)",
            "#1E1D1B": "Mobile.swift Device: the iPhone frame's body (the phone itself; the lab's stage only)",
            "#6E6A63": "Mobile.swift Device: the iPhone frame's edge (the stage only)",
            "#0B0B0C": "Mobile.swift Device: the Android frame's body (the other phone's hardware)",
            "#A8C7FA": "Conversation.swift Biometric: Android's fingerprint glyph; the system draws Face ID",
        ]
        let sources = Self.repo.appendingPathComponent("apps/ios/Packages/FerbDesign/Sources/FerbDesign")
        var ours = Set<String>(["#000000", "#FFFFFF"])
        let regex = try NSRegularExpression(pattern: "0x([0-9A-Fa-f]{6})\\b")
        for case let url as URL in FileManager.default.enumerator(at: sources, includingPropertiesForKeys: nil)! where url.pathExtension == "swift" {
            let text = try String(contentsOf: url, encoding: .utf8)
            for m in regex.matches(in: text, range: NSRange(text.startIndex..., in: text)) {
                ours.insert("#" + String(text[Range(m.range(at: 1), in: text)!]).uppercased())
            }
        }
        let literals = try XCTUnwrap(try lab["literalColours"] as? [String: Any])
        let missing = literals.keys.filter { !ours.contains($0) && notOnIPhone[$0] == nil }.sorted()
        XCTAssertEqual(missing, [], "lab colours with no Swift counterpart and no reason")
    }
}
