import XCTest
@testable import FERBNotch
import NotchCore

@MainActor
final class NotchDismissalTests: XCTestCase {
    // decision: APP-558
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

    // decision: APP-552, APP-743
    func testToggleClosesTheOpenChatWithoutClearingTheDraft() {
        let model = NotchModel()
        model.session.draft = "Keep me"
        model.toggle()
        XCTAssertTrue(model.bodyVisible)
        model.toggle()
        XCTAssertFalse(model.expanded)
        XCTAssertEqual(model.session.draft, "Keep me")
    }

    // decision: APP-564, APP-743
    func testWorkContinuesAfterTheChatCloses() {
        let model = NotchModel()
        model.session.draft = "Write a brief"
        model.expand()
        let taskID = model.session.beginWork()
        model.dismissAutomatically()
        XCTAssertFalse(model.expanded)
        XCTAssertEqual(model.session.phase, .working)
        XCTAssertEqual(model.session.activeTaskID, taskID)
    }

    // decision: APP-798
    /// Owner 2026-11-08: "idk if htats the correct fix or will it malfuntion other thigns. but why overcomplicate
    /// eveyrhitng/?? its just ui - boxes, just fix them ." A press in Settings with the notch closed changes nothing
    /// (no presentation change that reorders the notch's windows mid-drag); with the notch open it still closes it.
    func testAPressInAnotherFERBWindowClosesOnlyWhatShows() {
        let model = NotchModel()
        var changes = 0
        model.onPresentationChange = { _ in changes += 1 }
        model.pressedInAnotherFERBWindow()
        XCTAssertEqual(changes, 0)
        model.expand()
        changes = 0
        model.pressedInAnotherFERBWindow()
        XCTAssertFalse(model.expanded)
        XCTAssertEqual(changes, 1)
    }

    // decision: APP-558, APP-743
    func testVoiceHoldsTheChatOpenUntilItEnds() {
        let model = NotchModel()
        model.expand()
        model.session.beginVoicePreview()
        model.dismissAutomatically()
        XCTAssertTrue(model.expanded)
        model.collapse()
        XCTAssertFalse(model.expanded)
    }
}

@MainActor
final class FilePickerVisibilityTests: XCTestCase {
    // decision: APP-558
    func testPickerStateNotifiesCoordinatorAndKeepsDraft() {
        let model = NotchModel()
        model.expand()
        model.session.draft = "Keep my draft"
        var changes: [Bool] = []
        model.onYieldChange = { changes.append($0) }
        model.setFilePickerOpen(true)
        model.setFilePickerOpen(true)
        XCTAssertTrue(model.filePickerOpen)
        XCTAssertEqual(model.yielding.reasons, [.filePicker])
        XCTAssertTrue(model.blocksAutomaticDismissal)
        model.dismissAutomatically()
        XCTAssertTrue(model.expanded)
        model.setFilePickerOpen(false)
        XCTAssertEqual(changes, [true, false])
        XCTAssertFalse(model.filePickerOpen)
        XCTAssertFalse(model.isYielded)
        XCTAssertEqual(model.session.draft, "Keep my draft")
    }
}
