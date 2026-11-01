import XCTest
@testable import FERBNotch

@MainActor
final class NotchUIPreferencesTests: XCTestCase {
    func testHoverPreferenceCanBeChangedWithoutAutomaticSuggestionState() {
        let preferences = scratchDefaults()
        let model = NotchModel(preferences: preferences)
        XCTAssertTrue(model.hoverEnabled)
        model.setHoverEnabled(false)
        XCTAssertFalse(model.hoverEnabled)
        model.setHoverEnabled(true)
        XCTAssertTrue(model.hoverEnabled)
        XCTAssertNil(preferences.object(forKey: "notch.untypedHoverOpens"))
    }

    func testExpandedPanelSizePersistsAcrossModelRestarts() {
        let preferences = scratchDefaults()
        let model = NotchModel(preferences: preferences)
        model.resizePanel(width: 520, transcriptHeight: 380, persist: true)

        let restarted = NotchModel(preferences: preferences)
        XCTAssertEqual(restarted.bodyWidth, 520)
        XCTAssertEqual(restarted.chatTranscriptHeight, 380)
    }

    func testExplicitFERBRequestChangesHoverOpeningLocally() {
        let preferences = scratchDefaults()
        let model = NotchModel(preferences: preferences)

        XCTAssertTrue(model.handleLocalNotchCommand("Ferb, could you turn hover back on?"))
        XCTAssertTrue(model.hoverEnabled)
        XCTAssertEqual(model.localSettingReply, "Hover opening is on.")
        XCTAssertEqual(model.session.draft, "")

        XCTAssertTrue(model.handleLocalNotchCommand("please turn hover off"))
        XCTAssertFalse(model.hoverEnabled)
        XCTAssertEqual(model.localSettingReply, "Hover opening is off.")
    }

    func testPastedPhraseCannotChangeHoverPreference() {
        let model = NotchModel(preferences: scratchDefaults())
        model.pasteProvenance.paste("turn hover off", over: NSRange(location: 0, length: 0), in: "")

        XCTAssertFalse(model.handleLocalNotchCommand("turn hover off"))
        XCTAssertTrue(model.hoverEnabled)
    }

    // decision: APP-564
    func testNewTaskResetsIndicatorFromAnOlderFailureToWorking() {
        let model = NotchModel(preferences: scratchDefaults())
        model.session.draft = "Earlier task"
        _ = model.session.beginWork()
        model.session.requireAttention("Earlier task failed")
        model.runningTasks["new-task"] = "New task"

        XCTAssertEqual(model.indicatorPhase, .working)
        model.runningTasks.removeValue(forKey: "new-task")
        XCTAssertEqual(model.indicatorPhase, .attention)
    }
    func testOpeningChatAcknowledgesIndicatorUntilANewTaskArrives() {
        let model = NotchModel(preferences: scratchDefaults())
        model.session.draft = "Earlier task"
        _ = model.session.beginWork()
        model.session.requireAttention("Failed earlier")
        XCTAssertEqual(model.indicatorPhase, .attention)
        model.expand()
        XCTAssertEqual(model.indicatorPhase, .ready)
        model.collapse()
        XCTAssertEqual(model.indicatorPhase, .ready)
        model.receiveBusEvent(["topic": "task.created", "data": ["task_id": "next", "goal": "New task"]])
        XCTAssertEqual(model.indicatorPhase, .working)
    }

    func testStatusChangingWhileChatIsOpenStaysAcknowledgedAfterClosing() {
        let model = NotchModel(preferences: scratchDefaults())
        model.session.draft = "Visible task"
        _ = model.session.beginWork()
        model.expand()
        model.session.requireAttention("Failure visible in chat")
        XCTAssertEqual(model.indicatorPhase, .ready)
        model.collapse()
        XCTAssertEqual(model.indicatorPhase, .ready)
    }

}
