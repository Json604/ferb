import AppKit
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// One hover look for file cards, site chips, their "+N" box and links in answers (APP-753). Structure, not wording.
@MainActor
final class HoverLookTests: XCTestCase {
    // decision: APP-753
    func testHoverChangesColourAndReduceMotionChangesItAtOnce() {
        XCTAssertGreaterThan(HoverLook.fill(hovered: true), HoverLook.fill(hovered: false))
        XCTAssertGreaterThan(HoverLook.badgeFill(hovered: false), HoverLook.fill(hovered: false))   // the box stands out
        XCTAssertGreaterThan(HoverLook.badgeFill(hovered: true), HoverLook.badgeFill(hovered: false))
        XCTAssertNotEqual(NSColor.linkColor.hoverLifted(true), NSColor.linkColor.hoverLifted(false))
        XCTAssertEqual(NSColor.linkColor.hoverLifted(false), NSColor.linkColor)
        XCTAssertNil(HoverLook.duration(reduceMotion: true))
        XCTAssertNotNil(HoverLook.duration(reduceMotion: false))
        XCTAssertEqual(HoverLook.overflow(2), "+2")
        XCTAssertNil(HoverLook.overflow(0))
    }

    // decision: APP-753, APP-589
    func testAnAnswersLinkLiftsUnderThePointerAndPlainTextDoesNot() {
        let view = MessageTextView()
        view.frame = CGRect(x: 0, y: 0, width: 400, height: 40)
        view.update(MessageTextAttributes.make("[example.com](https://example.com) then plain words",
                                               size: 12, secondary: false, markdown: true, bold: false,
                                               monospaced: false, alignment: .left), generation: 0)
        view.layout()
        guard let manager = view.layoutManager, let container = view.textContainer else { return XCTFail("no layout") }
        manager.ensureLayout(for: container)
        func point(of word: String) -> NSPoint {
            let range = (view.string as NSString).range(of: word)
            let glyphs = manager.glyphRange(forCharacterRange: range, actualCharacterRange: nil)
            let rect = manager.boundingRect(forGlyphRange: glyphs, in: container)
            return NSPoint(x: rect.midX + view.textContainerOrigin.x, y: rect.midY + view.textContainerOrigin.y)
        }
        view.hover(at: point(of: "example"))
        XCTAssertEqual(view.hoveredLink, (view.string as NSString).range(of: "example.com"))
        view.hover(at: point(of: "plain"))
        XCTAssertNil(view.hoveredLink)
        view.hover(at: point(of: "example"))
        view.hover(at: nil)
        XCTAssertNil(view.hoveredLink)
        // an executable link is never a link, so it never lifts (APP-589)
        view.update(MessageTextAttributes.make("[run me](javascript:alert(1)) text", size: 12, secondary: false,
                                               markdown: true, bold: false, monospaced: false, alignment: .left),
                    generation: 1)
        view.layout()
        manager.ensureLayout(for: container)
        view.hover(at: point(of: "run"))
        XCTAssertNil(view.hoveredLink)
    }
}
