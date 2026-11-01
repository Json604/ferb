import XCTest
@testable import FERBNotch

@MainActor
final class NotchUIPreferencesTests: XCTestCase {
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
}
