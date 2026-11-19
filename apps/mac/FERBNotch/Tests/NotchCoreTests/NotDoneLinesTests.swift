import AppKit
import SwiftUI
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// Owner 2026-11-19, asked "'Not done:' lines smaller and coloured on both apps? On the Mac today they're plain answer
/// text": "do all". Code's "Not done:" lines under a delivered-with-gaps answer (ferb-supervisor `with_gap_lines`) are
/// drawn as the "Couldn't confirm:" line is: small, "Not done:" bold amber, the reason in the secondary ink, one line per
/// reason, inside the answer's bubble (APP-977, APP-978).
@MainActor
final class NotDoneLinesTests: XCTestCase {
    /// What the engine writes (crates/ferb-supervisor/tests/gap_lines.rs).
    static let gapped = "Left the draft open, unsent.\n\nNot done: A recipient's address is in the draft twice (you asked me to stop before I removed it)."
    static let two = "The rubric page is open.\n\nNot done: only you can bring real participants.\nNot done: The rubric isn't on the course page."

    // decision: APP-978
    func testOnlyTheBlockCodeWritesIsRecognised() {
        let one = GapLines(Self.gapped)
        XCTAssertEqual(one.body, "Left the draft open, unsent.")
        XCTAssertEqual(one.reasons, ["A recipient's address is in the draft twice (you asked me to stop before I removed it)."])
        XCTAssertEqual(GapLines(Self.two).reasons, ["only you can bring real participants.", "The rubric isn't on the course page."])
        // with the unconfirmed line after them, and an answer that was only the lines
        let after = GapLines(Self.gapped + "\n\nI couldn't confirm part of this.")
        XCTAssertEqual(after.body, "Left the draft open, unsent.\n\nI couldn't confirm part of this.")
        XCTAssertEqual(after.reasons.count, 1)
        XCTAssertEqual(GapLines("Not done: The form wasn't sent.").reasons, ["The form wasn't sent."])
        // words, not code's block: the answer's own first paragraph, a paragraph mixing other lines, a label in a sentence
        for words in ["Not done: yet, I'm still reading.\n\nMore soon.",
                      "Two parts.\n\nNot done: the rubric.\nThe rest is done.",
                      "Two parts.\n\nThe form is Not done: it needs your name.",
                      "Two parts.\n\nNot done:"] {
            XCTAssertEqual(GapLines(words).body, words, words)
            XCTAssertTrue(GapLines(words).reasons.isEmpty, words)
        }
    }

    // decision: APP-977, APP-978
    func testNotDoneIsDrawnSmallAmberAndSecondaryInsideTheAnswer() throws {
        let text = MarkdownTextAttributes.annotations(note: nil, lines: [], gaps: GapLines(Self.two).reasons)
        let string = text.string
        XCTAssertEqual(string.components(separatedBy: "\n").count, 2, "one line per reason")
        for line in ["Not done: only you can bring real participants.", "Not done: The rubric isn't on the course page."] {
            let range = (string as NSString).range(of: line)
            XCTAssertNotEqual(range.location, NSNotFound, line)
            let label = text.attributes(at: range.location, effectiveRange: nil)
            let font = try XCTUnwrap(label[.font] as? NSFont)
            XCTAssertEqual(font.pointSize, 11.5, "11.5 under the body's 12, as the note")
            XCTAssertTrue(font.fontDescriptor.symbolicTraits.contains(.bold), "the label bold")
            XCTAssertEqual(label[.foregroundColor] as? NSColor, NSColor(Palette.amber), "the label amber")
            let reason = text.attributes(at: range.location + GapLines.label.count + 2, effectiveRange: nil)
            XCTAssertEqual((reason[.font] as? NSFont)?.pointSize, 11.5)
            XCTAssertEqual(reason[.foregroundColor] as? NSColor, NSColor.secondaryLabelColor, "the reason secondary")
        }
        // the answer's words no longer carry them
        let answer = MarkdownTextAttributes.make(GapLines(Self.two).body, size: 12).string
        XCTAssertFalse(answer.contains("Not done"))
    }

    /// On the dark bubble: amber and the secondary ink at least 4.5:1 against FERB's black glass under a bubble's 8% white.
    // decision: APP-978
    func testTheColoursReadOnTheDarkSurface() throws {
        func luminance(_ color: NSColor) throws -> CGFloat {
            let c = try XCTUnwrap(color.usingColorSpace(.sRGB))
            func lin(_ v: CGFloat) -> CGFloat { v <= 0.03928 ? v / 12.92 : pow((v + 0.055) / 1.055, 2.4) }
            return 0.2126 * lin(c.redComponent) + 0.7152 * lin(c.greenComponent) + 0.0722 * lin(c.blueComponent)
        }
        let surface = NSColor(srgbRed: 0.10, green: 0.10, blue: 0.10, alpha: 1)       // black glass and the bubble's fill
        var secondary = NSColor.secondaryLabelColor
        NSAppearance(named: .darkAqua)?.performAsCurrentDrawingAppearance {
            let c = NSColor.secondaryLabelColor.usingColorSpace(.sRGB)!
            // its alpha over the surface
            secondary = NSColor(srgbRed: c.redComponent * c.alphaComponent + 0.10 * (1 - c.alphaComponent),
                                green: c.greenComponent * c.alphaComponent + 0.10 * (1 - c.alphaComponent),
                                blue: c.blueComponent * c.alphaComponent + 0.10 * (1 - c.alphaComponent), alpha: 1)
        }
        let bg = try luminance(surface)
        for (name, ink) in [("amber", NSColor(Palette.amber)), ("secondary", secondary)] {
            let ratio = (try luminance(ink) + 0.05) / (bg + 0.05)
            print("CONTRAST \(name) \(String(format: "%.1f", ratio)):1")
            XCTAssertGreaterThanOrEqual(ratio, 4.5, name)
        }
    }

    /// FERB_GOLDEN_DIR=<folder>: the answer bubble with its lines drawn to a PNG there, to look at (not compared: the Mac
    /// has no pixel goldens; fonts differ between Macs).
    func testDrawTheAnswerForLooking() throws {
        guard let folder = ProcessInfo.processInfo.environment["FERB_GOLDEN_DIR"] else { throw XCTSkip("FERB_GOLDEN_DIR") }
        let view = FerbBubble {
            MarkdownContent(Self.two + "\n\nI couldn't confirm part of this.", note: nil, lines: ["Sites contacted: 5 sites"])
        }
        .frame(width: 390).padding(20).background(Color.black).environment(\.colorScheme, .dark)
        let host = NSHostingView(rootView: view)
        host.appearance = NSAppearance(named: .darkAqua)
        host.frame = CGRect(x: 0, y: 0, width: 430, height: 260)
        let window = NSWindow(contentRect: host.frame, styleMask: .borderless, backing: .buffered, defer: true)
        window.appearance = NSAppearance(named: .darkAqua)
        window.contentView = host
        for _ in 0..<3 { host.layoutSubtreeIfNeeded(); RunLoop.main.run(until: Date().addingTimeInterval(0.05)) }
        let rep = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
        host.cacheDisplay(in: host.bounds, to: rep)
        let png = try XCTUnwrap(rep.representation(using: .png, properties: [:]))
        try png.write(to: URL(fileURLWithPath: folder).appendingPathComponent("mac-answer-not-done.png"))
    }
}
