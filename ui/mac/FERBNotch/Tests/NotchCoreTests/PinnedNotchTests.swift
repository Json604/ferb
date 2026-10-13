import XCTest
@testable import FERBNotch

@MainActor
final class PinnedNotchTests: XCTestCase {
    func testMenuOnlyDefersDismissalUntilItCloses() {
        let model = NotchModel()
        model.expand()
        model.session.draft = "Keep my draft"
        model.setProfileMenuOpen(true)
        model.dismissAutomatically()
        XCTAssertTrue(model.expanded)
        var ended = false
        model.onInteractionEnded = { ended = true }
        model.setProfileMenuOpen(false)
        XCTAssertTrue(ended)
        XCTAssertFalse(model.blocksAutomaticDismissal)
        model.dismissAutomatically()
        XCTAssertFalse(model.expanded)
        XCTAssertEqual(model.session.draft, "Keep my draft")
    }

    func testPinnedFoldKeepsHeaderAndDraftThenHoverRestoresBody() {
        let model = NotchModel()
        model.session.draft = "Keep this draft"
        model.expand()
        let openSize = model.panelSize
        model.togglePin()
        XCTAssertTrue(model.expanded)
        XCTAssertTrue(model.isPinned)
        XCTAssertFalse(model.bodyVisible)
        XCTAssertEqual(model.panelSize.width, openSize.width)
        XCTAssertLessThan(model.panelSize.height, openSize.height)
        model.revealPinnedBody()
        XCTAssertTrue(model.bodyVisible)
        XCTAssertEqual(model.session.draft, "Keep this draft")
        XCTAssertEqual(model.panelSize, openSize)
        model.collapse()
    }

    func testToggleClosesBothAnOpenDialogAndAPinnedHeaderWithoutClearingDraft() {
        let model = NotchModel()
        model.session.draft = "Keep me"
        model.toggle()
        XCTAssertTrue(model.bodyVisible)
        model.toggle()
        XCTAssertFalse(model.expanded)
        model.toggle()
        model.togglePin()
        model.foldPinnedBody()
        model.toggle()
        XCTAssertFalse(model.expanded)
        XCTAssertFalse(model.isPinned)
        XCTAssertFalse(model.isHeaderOnly)
        XCTAssertEqual(model.session.draft, "Keep me")
    }

    func testOpenMenuAndVoicePreventFolding() {
        let model = NotchModel()
        model.expand()
        model.togglePin()
        model.revealPinnedBody()
        model.setProfileMenuOpen(true)
        model.foldPinnedBody()
        XCTAssertTrue(model.bodyVisible)
        model.setProfileMenuOpen(false)
        XCTAssertTrue(model.isHeaderOnly)
        model.revealPinnedBody()
        model.session.beginVoicePreview()
        model.foldPinnedBody()
        XCTAssertTrue(model.bodyVisible)
        model.collapse()
    }

    func testWorkContinuesWhilePinnedHeaderSurvivesAutomaticDismissal() {
        let model = NotchModel()
        model.session.draft = "Write a brief"
        model.expand()
        model.togglePin()
        let taskID = model.session.beginWork()
        model.foldPinnedBody()
        model.dismissAutomatically()
        XCTAssertTrue(model.isHeaderOnly)
        XCTAssertTrue(model.expanded)
        XCTAssertEqual(model.session.phase, .working)
        XCTAssertEqual(model.session.activeTaskID, taskID)
        model.collapse()
    }

    func testUnpinRevealsBodyAndPreventsFurtherFolding() {
        let model = NotchModel()
        model.expand()
        model.togglePin()
        model.foldPinnedBody()
        model.togglePin()
        XCTAssertTrue(model.bodyVisible)
        model.foldPinnedBody()
        XCTAssertTrue(model.bodyVisible)
        XCTAssertFalse(model.isPinned)
    }

    func testPinAndHoverExitFoldImmediatelyWhileHoverRevealsBody() {
        let model = NotchModel()
        model.expand()
        model.togglePin()
        XCTAssertTrue(model.isHeaderOnly)
        model.revealPinnedBody()
        XCTAssertTrue(model.bodyVisible)
        model.foldPinnedBody()
        XCTAssertTrue(model.isHeaderOnly)
        model.collapse()
    }

    func testHoverCancelsADeferredFoldBeforeTheMenuCloses() {
        let model = NotchModel()
        model.expand()
        model.togglePin()
        model.revealPinnedBody()
        model.setProfileMenuOpen(true)
        model.foldPinnedBody()
        model.revealPinnedBody()
        model.setProfileMenuOpen(false)
        XCTAssertTrue(model.bodyVisible)
        model.collapse()
    }
}
