import AppKit
import SwiftUI
import XCTest
@testable import FERBNotch

@MainActor
final class NotchLayoutTests: XCTestCase {
    // decision: APP-561, APP-558
    func testPinnedHeaderDoesNotShiftWhenTheWindowCanvasShrinks() throws {
        let model = NotchModel()
        model.expand()
        model.togglePin()
        model.revealPinnedBody()
        let presentation = NotchPresentation()
        let hosting = NSHostingView(rootView: NotchView(model: model, presentation: presentation))
        hosting.sizingOptions = []
        hosting.frame = CGRect(origin: .zero, size: model.panelSize)
        presentation.present(size: CGSize(width: model.bodyWidth, height: model.cutoutHeight + model.bodyHeight),
                             expanded: true, bodyVisible: true, animate: false) {}
        hosting.layoutSubtreeIfNeeded()
        let profile = try XCTUnwrap(profileButton(in: hosting))
        let initial = profile.convert(profile.bounds, to: hosting)

        model.foldPinnedBody()
        presentation.present(size: CGSize(width: model.bodyWidth, height: model.cutoutHeight),
                             expanded: true, bodyVisible: false, animate: false) {}
        hosting.layoutSubtreeIfNeeded()
        // This is the canvas resize at the END of the shared spring. It must not
        // recenter the still-mounted, full-height content above the screen edge.
        hosting.frame.size = model.panelSize
        hosting.layoutSubtreeIfNeeded()
        let foldedProfile = try XCTUnwrap(profileButton(in: hosting))
        let folded = foldedProfile.convert(foldedProfile.bounds, to: hosting)
        XCTAssertEqual(folded.midY, initial.midY, accuracy: 0.5)
        XCTAssertEqual(folded.midY, model.cutoutHeight / 2, accuracy: 0.5)
        XCTAssertEqual(folded.midX, initial.midX, accuracy: 0.5)
        model.collapse()
    }

    // decision: APP-557
    func testOpeningFromCompactDoesNotSlideBodySeparatelyFromSurface() {
        let presentation = NotchPresentation()
        presentation.backdrop.frame = CGRect(x: 0, y: 0, width: 456, height: 500)
        presentation.present(size: CGSize(width: 440, height: 480), expanded: true,
                             bodyVisible: true, anchor: CGPoint(x: 228, y: 15), animate: true) {}
        XCTAssertEqual(presentation.bodyExpansion, 1)
        presentation.stop()
    }

    // decision: APP-557
    func testReopeningDuringCloseKeepsCurrentSurfaceInsteadOfRestartingAtPill() throws {
        guard NSScreen.main != nil else { throw XCTSkip("A display is required for the presentation clock") }
        let presentation = NotchPresentation()
        presentation.backdrop.frame = CGRect(x: 0, y: 0, width: 456, height: 500)
        presentation.present(size: CGSize(width: 440, height: 480), expanded: true,
                             bodyVisible: true, animate: false) {}
        presentation.present(size: CGSize(width: 58, height: 30), expanded: false,
                             bodyVisible: true, anchor: CGPoint(x: 228, y: 15), animate: true) {}
        let visibleSize = presentation.size
        let visibleOrigin = presentation.surfaceOrigin
        presentation.present(size: CGSize(width: 440, height: 480), expanded: true,
                             bodyVisible: true, anchor: CGPoint(x: 228, y: 15), animate: true) {}
        XCTAssertEqual(presentation.size, visibleSize)
        XCTAssertEqual(presentation.surfaceOrigin, visibleOrigin)
        presentation.stop()
    }

    private func profileButton(in view: NSView) -> NSButton? {
        if let button = view as? NSButton, button.toolTip == "Profile and options" { return button }
        return view.subviews.lazy.compactMap { self.profileButton(in: $0) }.first
    }
}
