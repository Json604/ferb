import XCTest
@testable import FERBNotch

@MainActor
final class TaskPreviewHoverTests: XCTestCase {
    func testLeavingPreviewCanDismissNotchAndClearPresentation() {
        let model = NotchModel()
        model.expand()
        let id = UUID()
        model.setPreviewPopover(id, presented: true)
        XCTAssertFalse(model.blocksAutomaticDismissal)
        let revision = model.previewDismissalRevision
        model.dismissAutomatically()
        XCTAssertFalse(model.expanded)
        XCTAssertNotEqual(model.previewDismissalRevision, revision)
    }

    func testRepeatedLifecycleCallbacksAreIdempotentAndRetryDismissal() {
        let model = NotchModel()
        let old = UUID(), new = UUID()
        var ended = 0
        model.onInteractionEnded = { ended += 1 }
        model.setPreviewPopover(old, presented: true)
        model.setPreviewPopover(old, presented: true)
        XCTAssertEqual(model.previewPopoverCount, 1)
        model.setPreviewPopover(new, presented: true)
        model.setPreviewPopover(old, presented: false)
        model.setPreviewPopover(old, presented: false)
        XCTAssertEqual(model.previewPopoverCount, 1)
        XCTAssertEqual(ended, 1)
        model.setPreviewPopover(new, presented: false)
        XCTAssertEqual(model.previewPopoverCount, 0)
        XCTAssertEqual(ended, 2)
    }

    func testDetachingOrFinishingPreviewDoesNotKeepChatOpen() {
        let model = NotchModel()
        model.expand()
        let id = UUID()
        model.setPreviewPopover(id, presented: true)
        model.setPreviewPopover(id, presented: false)
        model.dismissAutomatically()
        XCTAssertFalse(model.expanded)
    }

    func testPreviewDoesNotRemoveOtherInteractionBlockers() {
        let model = NotchModel()
        model.expand()
        model.setPreviewPopover(UUID(), presented: true)
        model.setProfileMenuOpen(true)
        model.dismissAutomatically()
        XCTAssertTrue(model.expanded)
        model.setProfileMenuOpen(false)
        model.dismissAutomatically()
        XCTAssertFalse(model.expanded)
    }
}
