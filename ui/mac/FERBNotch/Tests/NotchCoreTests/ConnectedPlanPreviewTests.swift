import XCTest
@testable import FERBNotch
@testable import NotchCore

@MainActor
final class ConnectedPlanPreviewTests: XCTestCase {
    func testFirstDeltaUsesCompactConnectedSurfaceWithoutExpandingSession() {
        let model = NotchModel(preferences: UserDefaults(suiteName: UUID().uuidString)!)
        model.session.draft = "Check Gmail"
        let id = model.session.beginWork()!
        model.conversation.begin(id: id, request: "Check Gmail", sources: [])
        model.collapse()
        var focused = false
        model.onPresentationChange = { _ in focused = true }
        model.receiveChatEvent(["type": "plan_delta", "text": "I will check "], localID: id)
        XCTAssertEqual(model.compactPlanText, "I will check ")
        XCTAssertTrue(model.surfaceVisible)
        XCTAssertFalse(model.expanded)
        XCTAssertFalse(focused)
        XCTAssertLessThan(model.surfaceSize.height, model.cutoutHeight + model.bodyHeight)
        model.receiveChatEvent(["type": "plan_delta", "text": "Gmail."], localID: id)
        XCTAssertEqual(model.compactPlanText, "I will check Gmail.")
        model.receiveChatEvent(["type": "plan", "task_id": "gmail", "text": "I will inspect Gmail."], localID: id)
        XCTAssertEqual(model.compactPlanText, "I will inspect Gmail.")
        XCTAssertEqual(model.conversation.turns.first?.plan, "I will inspect Gmail.")
        model.expand()
        XCTAssertNil(model.compactPlanText)
        XCTAssertTrue(model.expanded)
    }

    func testStreamDoesNotExpireUntilThreeSecondsAfterFinalAndReplacementIsOwned() async throws {
        let notice = AcknowledgementNotice(duration: .milliseconds(60))
        let first = UUID(), second = UUID()
        notice.append("First", turn: first)
        try await Task.sleep(for: .milliseconds(90))
        XCTAssertEqual(notice.text, "First")
        notice.finish("First final", turn: first)
        try await Task.sleep(for: .milliseconds(30))
        notice.append("Second", turn: second)
        try await Task.sleep(for: .milliseconds(60))
        XCTAssertEqual(notice.text, "Second")
        notice.abort(turn: first)
        XCTAssertEqual(notice.text, "Second")
        notice.finish("Second final", turn: second)
        try await Task.sleep(for: .milliseconds(90))
        XCTAssertNil(notice.text)
    }

    func testDismissedStreamCannotReopenAndAbortClearsOnlyItsOwnPreview() {
        let notice = AcknowledgementNotice()
        let first = UUID()
        notice.append("First", turn: first)
        notice.dismiss()
        notice.append(" more", turn: first)
        notice.finish("First more", turn: first)
        XCTAssertNil(notice.text)
        let next = UUID()
        notice.append("Next", turn: next)
        notice.abort(turn: next)
        XCTAssertNil(notice.text)
    }
    func testBackgroundStreamUsesRestoredForegroundAndTerminalPromotes() {
        let model = NotchModel(preferences: UserDefaults(suiteName: UUID().uuidString)!)
        model.session.draft = "Check Gmail"
        let id = model.session.beginWork()!
        model.conversation.begin(id: id, request: "Check Gmail", sources: [])
        model.workSessions[id] = model.session
        model.session = NotchSession()
        model.session.collapse()
        model.receiveChatEvent(["type": "ack", "request_id": "correct", "text": "Starting"], localID: id)
        model.receiveChatEvent(["type": "plan_delta", "request_id": "wrong", "text": "Wrong"], localID: id)
        XCTAssertNil(model.compactPlanText)
        model.receiveChatEvent(["type": "plan_delta", "request_id": "correct", "text": "Checking Gmail"], localID: id)
        XCTAssertEqual(model.compactPlanText, "Checking Gmail")
        XCTAssertFalse(model.expanded)
        model.receiveChatEvent(["type": "plan_abort", "request_id": "correct"], localID: id)
        XCTAssertNil(model.compactPlanText)
        model.acknowledgement.show("Checking again")
        model.presentTerminalAnswer()
        XCTAssertTrue(model.expanded)
        XCTAssertNil(model.compactPlanText)
    }

    func testSameLineChunksDoNotRestartRevealAndUserOpenedChatRemainsOpen() async throws {
        let model = NotchModel(preferences: UserDefaults(suiteName: UUID().uuidString)!)
        model.collapse()
        var presentations = 0
        model.onBackgroundChange = { _ in presentations += 1 }
        let id = UUID()
        model.acknowledgement.append("Checking", turn: id)
        XCTAssertEqual(presentations, 1)
        model.acknowledgement.append(" Gmail", turn: id)
        XCTAssertEqual(presentations, 1)
        model.acknowledgement.finish("Checking Gmail", turn: id)
        model.expand()
        try await Task.sleep(for: .milliseconds(3100))
        XCTAssertTrue(model.expanded)
        XCTAssertNil(model.compactPlanText)
    }

    func testCompactTextSurvivesClosingSurfaceWithoutExposingTranscript() {
        let model = NotchModel(preferences: UserDefaults(suiteName: UUID().uuidString)!)
        model.collapse()
        model.acknowledgement.show("Checking Gmail")
        model.acknowledgement.dismiss()
        XCTAssertFalse(model.surfaceVisible)
        XCTAssertEqual(model.compactPlanDisplayText, "Checking Gmail")
        model.finishCompactPreviewDismissal()
        XCTAssertNil(model.compactPlanDisplayText)
        model.acknowledgement.show("Checking again")
        model.expand()
        XCTAssertNil(model.compactPlanDisplayText)
        XCTAssertTrue(model.expanded)
    }

    func testPlanEndStartsExpiryAndCanonicalPlanDoesNotRestartIt() async throws {
        let notice = AcknowledgementNotice(duration: .milliseconds(100))
        let id = UUID()
        notice.append("Checking Gmail", turn: id)
        notice.complete(turn: UUID())
        try await Task.sleep(for: .milliseconds(130))
        XCTAssertEqual(notice.text, "Checking Gmail")
        notice.complete(turn: id)
        try await Task.sleep(for: .milliseconds(65))
        notice.finish("Canonical plan", turn: id)
        XCTAssertEqual(notice.text, "Canonical plan")
        try await Task.sleep(for: .milliseconds(65))
        XCTAssertNil(notice.text)
        notice.finish("Late canonical plan", turn: id)
        XCTAssertNil(notice.text)
    }

    func testInterruptedOlderSubmissionDoesNotDismissNewerPlanPreview() {
        let model = NotchModel(preferences: UserDefaults(suiteName: UUID().uuidString)!)
        model.session.draft = "Old task"
        let old = model.session.beginWork()!
        model.conversation.begin(id: old, request: "Old task", sources: [])
        model.workSessions[old] = model.session
        model.session = NotchSession()
        model.session.collapse()
        let current = UUID()
        model.acknowledgement.append("New plan", turn: current)
        model.finishSubmission(localID: old)
        XCTAssertEqual(model.compactPlanText, "New plan")
    }

}
