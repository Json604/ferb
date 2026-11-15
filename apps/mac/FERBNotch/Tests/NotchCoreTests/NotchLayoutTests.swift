import AppKit
import SwiftUI
import XCTest
@testable import FERBNotch

@MainActor
final class NotchLayoutTests: XCTestCase {
    /// The body stays mounted while the chat closes; its full height never recentres the header when AppKit trims the
    /// canvas at the end of the spring.
    // decision: APP-561
    func testTheHeaderDoesNotShiftWhenTheWindowCanvasShrinksOnClose() throws {
        let model = NotchModel(preferences: scratchDefaults())
        model.expand()
        let presentation = NotchPresentation()
        let hosting = NSHostingView(rootView: NotchView(model: model, presentation: presentation))
        hosting.sizingOptions = []
        hosting.frame = CGRect(origin: .zero, size: model.panelSize)
        presentation.present(size: CGSize(width: model.bodyWidth, height: model.cutoutHeight + model.bodyHeight),
                             expanded: true, animate: false) {}
        hosting.layoutSubtreeIfNeeded()
        let profile = try XCTUnwrap(profileButton(in: hosting))
        let initial = profile.convert(profile.bounds, to: hosting)

        presentation.present(size: CGSize(width: model.bodyWidth, height: model.cutoutHeight),
                             expanded: true, animate: false) {}
        hosting.layoutSubtreeIfNeeded()
        hosting.frame.size = CGSize(width: model.panelSize.width, height: model.cutoutHeight + 12)
        hosting.layoutSubtreeIfNeeded()
        let shrunkProfile = try XCTUnwrap(profileButton(in: hosting))
        let shrunk = shrunkProfile.convert(shrunkProfile.bounds, to: hosting)
        XCTAssertEqual(shrunk.midY, initial.midY, accuracy: 0.5)
        XCTAssertEqual(shrunk.midX, initial.midX, accuracy: 0.5)
    }

    // decision: APP-557
    func testReopeningDuringCloseKeepsCurrentSurfaceInsteadOfRestartingAtPill() throws {
        guard NSScreen.main != nil else { throw XCTSkip("A display is required for the presentation clock") }
        let presentation = NotchPresentation()
        presentation.backdrop.frame = CGRect(x: 0, y: 0, width: 456, height: 500)
        presentation.present(size: CGSize(width: 440, height: 480), expanded: true, animate: false) {}
        presentation.present(size: CGSize(width: 58, height: 30), expanded: false,
                             anchor: CGPoint(x: 228, y: 15), animate: true) {}
        let visibleSize = presentation.size
        let visibleOrigin = presentation.surfaceOrigin
        presentation.present(size: CGSize(width: 440, height: 480), expanded: true,
                             anchor: CGPoint(x: 228, y: 15), animate: true) {}
        XCTAssertEqual(presentation.size, visibleSize)
        XCTAssertEqual(presentation.surfaceOrigin, visibleOrigin)
        presentation.stop()
    }

    private func profileButton(in view: NSView) -> NSButton? {
        if let button = view as? NSButton, button.toolTip == "Profile and options" { return button }
        return view.subviews.lazy.compactMap { self.profileButton(in: $0) }.first
    }
}
