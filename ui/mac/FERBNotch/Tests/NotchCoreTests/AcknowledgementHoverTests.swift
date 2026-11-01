import AppKit
import SwiftUI
import XCTest
@testable import FERBNotch
@testable import NotchCore

@MainActor
final class AcknowledgementHoverTests: XCTestCase {
    // decision: APP-579, APP-571
    func testFreshPlanIsImmediateWithoutExpandingAndDoesNotReplay() {
        let model = NotchModel(preferences: scratchDefaults())
        model.session.draft = "Check my task"
        let id = model.session.beginWork()!
        model.conversation.begin(id: id, request: "Check my task", sources: [])
        model.collapse()
        var focus = 0
        model.onPresentationChange = { _ in focus += 1 }
        model.receiveChatEvent(["type": "ack", "text": "Sure, starting now."], localID: id)
        XCTAssertNil(model.acknowledgement.text)
        model.receiveChatEvent(["type": "plan", "task_id": "task", "text": "Checking now"], localID: id)
        XCTAssertEqual(model.acknowledgement.text, "Checking now")
        XCTAssertEqual(model.conversation.turns.first?.plan, "Checking now")
        XCTAssertFalse(model.expanded)
        XCTAssertEqual(focus, 0)
        model.acknowledgement.dismiss()
        model.receiveChatEvent(["type": "ack", "text": "Sure, starting now."], localID: id)
        XCTAssertNil(model.acknowledgement.text)
        model.receiveChatEvent(["type": "plan", "task_id": "task", "text": "Checking now"], localID: id)
        XCTAssertNil(model.acknowledgement.text)
        model.expand()
        model.receiveChatEvent(["type": "plan", "task_id": "task", "text": "Still checking"], localID: id)
        XCTAssertNil(model.acknowledgement.text)
        XCTAssertEqual(model.conversation.turns.first?.plan, "Still checking")
    }

    // decision: APP-571
    func testBackgroundPlanUsesForegroundVisibilityAndHistoryNeverReplays() {
        let preferences = scratchDefaults()
        let model = NotchModel(preferences: preferences)
        model.session.draft = "Background task"
        let id = model.session.beginWork()!
        model.conversation.begin(id: id, request: "Background task", sources: [])
        model.workSessions[id] = model.session
        model.session = NotchSession()
        model.session.collapse()
        model.receiveChatEvent(["type": "plan", "task_id": "task", "text": "Checking background"], localID: id)
        XCTAssertEqual(model.acknowledgement.text, "Checking background")
        XCTAssertFalse(model.expanded)
        model.expand()
        model.receiveChatEvent(["type": "plan", "task_id": "task", "text": "New acknowledgement"], localID: id)
        XCTAssertNil(model.acknowledgement.text)
        XCTAssertTrue(model.expanded)
        let restored = NotchModel(preferences: preferences)
        XCTAssertNil(restored.acknowledgement.text)
        XCTAssertEqual(restored.conversation.turns.first?.plan, "New acknowledgement")
    }

}
