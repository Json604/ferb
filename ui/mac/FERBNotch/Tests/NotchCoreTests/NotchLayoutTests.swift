import AppKit
import SwiftUI
import XCTest
@testable import FERBNotch

@MainActor
final class NotchLayoutTests: XCTestCase {
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

    private func profileButton(in view: NSView) -> NSButton? {
        if let button = view as? NSButton, button.toolTip == "Profile and options" { return button }
        return view.subviews.lazy.compactMap { self.profileButton(in: $0) }.first
    }
}
