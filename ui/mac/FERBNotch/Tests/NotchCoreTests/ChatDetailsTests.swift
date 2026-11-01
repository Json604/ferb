import XCTest
import AppKit
import SwiftUI
@testable import FERBNotch
@testable import NotchCore

@MainActor
final class ChatDetailsTests: XCTestCase {
    // decision: APP-573
    func testArtifactPrimaryActionUsesQuickLookNotDefaultApp() {
        let model = NotchModel(preferences: scratchDefaults())
        let url = URL(fileURLWithPath: "/tmp/report.md")
        var previewed: URL?
        var opened = false
        model.onQuickLook = { previewed = $0 }
        model.onOpenDocument = { _ in opened = true }
        ArtifactRow(url: url, model: model).openPreview()
        XCTAssertEqual(previewed, url)
        XCTAssertFalse(opened)
    }

    // decision: APP-571
    func testFreshRepliesAndErrorsRevealWithoutFocusAndDuplicatesStayClosed() {
        for type in ["reply", "error"] {
            let model = NotchModel(preferences: scratchDefaults())
            model.session.draft = "Task"
            let id = model.session.beginWork()!
            model.conversation.begin(id: id, request: "Task", sources: [])
            model.collapse()
            var focus = 0
            model.onPresentationChange = { _ in focus += 1 }
            model.receiveChatEvent(["type": type, "text": "Result"], localID: id)
            XCTAssertTrue(model.bodyVisible, type)
            XCTAssertEqual(focus, 0)
            model.collapse()
            model.receiveChatEvent(["type": type, "text": "Result"], localID: id)
            XCTAssertFalse(model.bodyVisible, type)
        }
    }

    // decision: APP-571
    func testFreshExternalOutcomesOpenButHistoryAndRepeatedEventsDoNot() {
        for status in ["done", "failed"] {
            let model = NotchModel(preferences: scratchDefaults())
            let event: [String: Any] = ["task_id": "task", "status": status, "answer": "Result"]
            model.receiveOutcome(event)
            XCTAssertTrue(model.bodyVisible)
            model.collapse()
            model.receiveOutcome(event)
            XCTAssertFalse(model.bodyVisible)
            model.receiveOutcome(["task_id": "history", "status": status, "answer": "Old"], announce: false)
            XCTAssertFalse(model.bodyVisible)
        }
    }

    // decision: APP-571, APP-577
    func testBackgroundBoundCompletionExpandsForegroundWithoutReplacingDraft() {
        for status in ["done", "failed"] {
            let model = NotchModel(preferences: scratchDefaults())
            model.session.draft = "Background task"
            let id = model.session.beginWork()!
            model.session.bindBackendTask("background", localID: id)
            model.conversation.begin(id: id, request: "Background task", sources: [])
            model.conversation.bind(id: id, taskID: "background")
            model.workSessions[id] = model.session
            model.session = NotchSession()
            model.session.draft = "Next request"
            model.receiveOutcome(["task_id": "background", "status": status, "answer": "Result"])
            XCTAssertTrue(model.bodyVisible)
            XCTAssertEqual(model.session.draft, "Next request")
            model.collapse()
            model.receiveOutcome(["task_id": "background", "status": status, "answer": "Result"])
            XCTAssertFalse(model.bodyVisible)
        }
    }
}
