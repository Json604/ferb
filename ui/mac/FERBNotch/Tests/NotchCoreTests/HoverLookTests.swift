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

    // decision: APP-753, APP-756, APP-757
    func testTheAppKitHoverSourceDrivesTheHoverFlagAndThePointingHand() throws {
        let view = HoverSourceView(frame: CGRect(x: 0, y: 0, width: 80, height: 20))
        var seen: [Bool] = []
        view.onChange = { seen.append($0) }
        view.pointing = true
        let enter = try XCTUnwrap(NSEvent.enterExitEvent(with: .mouseEntered, location: .zero, modifierFlags: [],
                                                         timestamp: 0, windowNumber: 0, context: nil, eventNumber: 0,
                                                         trackingNumber: 0, userData: nil))
        let exit = try XCTUnwrap(NSEvent.enterExitEvent(with: .mouseExited, location: .zero, modifierFlags: [],
                                                        timestamp: 0, windowNumber: 0, context: nil, eventNumber: 0,
                                                        trackingNumber: 0, userData: nil))
        view.mouseEntered(with: enter)
        XCTAssertTrue(view.hovered)
        XCTAssertEqual(view.cursorAsked, NSCursor.pointingHand)
        view.mouseExited(with: exit)
        XCTAssertFalse(view.hovered)
        XCTAssertEqual(view.cursorAsked, NSCursor.arrow)             // never left behind
        view.mouseEntered(with: enter)
        view.viewDidHide()                                          // the notch closed or the window hid
        XCTAssertFalse(view.hovered)
        XCTAssertEqual(view.cursorAsked, NSCursor.arrow)
        XCTAssertEqual(seen, [true, false, true, false])
        XCTAssertNil(view.hitTest(CGPoint(x: 10, y: 10)))           // the card below keeps every click
        // tracking that works while another app is frontmost
        view.updateTrackingAreas()
        XCTAssertTrue(view.trackingAreas.contains { $0.options.contains(.activeAlways) })
        XCTAssertTrue(view.trackingAreas.contains { $0.options.contains(.cursorUpdate) })
        let plain = HoverSourceView()
        plain.updateTrackingAreas()
        XCTAssertFalse(plain.trackingAreas.contains { $0.options.contains(.cursorUpdate) })   // never resets a link's hand
        plain.mouseEntered(with: enter)
        XCTAssertTrue(plain.hovered)                                // colour only: no hand on a plain card
        XCTAssertNil(plain.cursorAsked)
    }

    // decision: APP-757
    func testEachVisibleLinkGetsItsOwnAlwaysActiveArea() {
        let window = NSWindow(contentRect: CGRect(x: 0, y: 0, width: 420, height: 80), styleMask: [.borderless],
                              backing: .buffered, defer: true)
        let view = MessageTextView()
        view.frame = CGRect(x: 0, y: 0, width: 400, height: 40)
        window.contentView?.addSubview(view)
        view.update(MessageTextAttributes.make("[example.com](https://example.com) then plain words",
                                               size: 12, secondary: false, markdown: true, bold: false,
                                               monospaced: false, alignment: .left), generation: 0)
        view.layout()
        view.updateTrackingAreas()
        let pieces = view.trackingAreas.filter { $0.userInfo?["location"] != nil }
        XCTAssertEqual(pieces.count, 1)
        XCTAssertTrue(pieces.allSatisfy { $0.options.contains(.activeAlways) && !$0.rect.isEmpty })
        XCTAssertTrue(pieces.allSatisfy { $0.owner is LinkHoverOwner })
    }

    // decision: APP-757, APP-756
    func testALiftedLinkRestsWhenItsWindowHidesOrTheViewIsHidden() {
        let window = NSWindow(contentRect: CGRect(x: 0, y: 0, width: 420, height: 80), styleMask: [.borderless],
                              backing: .buffered, defer: true)                 // never ordered in: not visible
        let view = MessageTextView()
        view.frame = CGRect(x: 0, y: 0, width: 400, height: 40)
        window.contentView?.addSubview(view)
        view.update(MessageTextAttributes.make("[example.com](https://example.com) then plain words",
                                               size: 12, secondary: false, markdown: true, bold: false,
                                               monospaced: false, alignment: .left), generation: 0)
        view.layout()
        guard let manager = view.layoutManager, let container = view.textContainer else { return XCTFail("no layout") }
        manager.ensureLayout(for: container)
        let range = (view.string as NSString).range(of: "example.com")
        let rect = manager.boundingRect(forGlyphRange: manager.glyphRange(forCharacterRange: range,
                                                                          actualCharacterRange: nil), in: container)
        let point = NSPoint(x: rect.midX + view.textContainerOrigin.x, y: rect.midY + view.textContainerOrigin.y)
        view.hover(at: point)
        XCTAssertEqual(view.hoveredLink, range)
        // the window folds or yields (ordered out, no exit from AppKit)
        NotificationCenter.default.post(name: NSWindow.didChangeOcclusionStateNotification, object: window)
        XCTAssertNil(view.hoveredLink, "the link stayed lifted after its window hid")
        view.hover(at: point)
        view.viewDidHide()                                           // hidden with an ancestor
        XCTAssertNil(view.hoveredLink)
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
