import XCTest
@testable import NotchCore
@testable import FERBNotch

@MainActor
final class ReplyStreamingTests: XCTestCase {
    func testInterleavedDraftsStayProvisionalAndFinalReplacesOnlyItsTurn() {
        let model = makeModel()
        let first = begin("First", in: model)
        model.workSessions[first] = model.session
        model.session = NotchSession()
        let second = begin("Second", in: model)
        model.session.draft = "My next request"
        var focusRequests = 0
        model.onPresentationChange = { _ in focusRequests += 1 }

        model.receiveChatEvent(["type": "reply_delta", "text": "First draft"], localID: first)
        model.receiveChatEvent(["type": "reply_delta", "text": "Second draft"], localID: second)
        XCTAssertEqual(model.conversation.turns.map(\.answer), ["First draft", "Second draft"])
        XCTAssertTrue(model.conversation.turns.allSatisfy(\.isProvisional))
        XCTAssertEqual(model.session.draft, "My next request")
        XCTAssertFalse(model.completionTraceVisible)
        XCTAssertEqual(focusRequests, 0)

        model.receiveChatEvent(["type": "reply", "text": "Second final"], localID: second)
        model.receiveChatEvent(["type": "reply_delta", "text": " stale"], localID: second)
        model.receiveChatEvent(["type": "reply", "text": "Duplicate"], localID: second)
        XCTAssertEqual(model.conversation.turns.map(\.answer), ["First draft", "Second final"])
        XCTAssertTrue(model.conversation.turns[0].isProvisional)
        XCTAssertFalse(model.conversation.turns[1].isProvisional)
        XCTAssertEqual(model.conversation.turns.count, 2)
        XCTAssertEqual(model.session.draft, "My next request")
    }

    func testAuthoritativePlanClearsDraftAndRejectsLateDraftDeltas() {
        let model = makeModel()
        let id = begin("Find my notes", in: model)
        model.receiveChatEvent(["type": "reply_delta", "text": "Provisional reply"], localID: id)
        model.receiveChatEvent(["type": "plan", "task_id": "real-task", "text": "Finding your notes"], localID: id)
        model.receiveChatEvent(["type": "reply_delta", "text": "late draft"], localID: id)
        XCTAssertEqual(model.conversation.turns.first?.answer, "")
        XCTAssertEqual(model.conversation.turns.first?.backendTaskID, "real-task")
        XCTAssertEqual(model.conversation.turns.first?.status, "working")
        XCTAssertEqual(model.session.activity.last?.title, "Finding your notes")
    }

    func testAbortClearsDraftAndPersistsInterruptionUntilAuthoritativeResult() throws {
        let model = makeModel()
        let id = begin("Hello", in: model)
        model.receiveChatEvent(["type": "reply_delta", "text": "Unfinished draft"], localID: id)
        model.receiveChatEvent(["type": "reply_abort", "reason": "Response interrupted"], localID: id)
        model.receiveChatEvent(["type": "reply_delta", "text": "stale"], localID: id)
        XCTAssertEqual(model.conversation.turns.first?.answer, "")
        XCTAssertEqual(model.conversation.turns.first?.status, "interrupted")
        XCTAssertEqual(model.conversation.turns.first?.responseNotice, "Response interrupted")
        XCTAssertFalse(model.completionTraceVisible)
        let saved = try XCTUnwrap(model.preferences.data(forKey: "FERBConversationArchive"))
        XCTAssertFalse(String(decoding: saved, as: UTF8.self).contains("Unfinished draft"))
        let restored = try JSONDecoder().decode(ConversationArchive.self, from: saved)
        XCTAssertEqual(restored.turns.first?.status, "interrupted")
        model.receiveChatEvent(["type": "clarify", "text": "Could you clarify?"], localID: id)
        XCTAssertEqual(model.conversation.turns.first?.answer, "Could you clarify?")
        XCTAssertNil(model.conversation.turns.first?.responseNotice)
    }

    func testPlanAfterAbortRestoresWorkingStateWithoutProvisionalText() {
        let model = makeModel()
        let id = begin("Find notes", in: model)
        model.receiveChatEvent(["type": "reply_delta", "text": "Draft"], localID: id)
        model.receiveChatEvent(["type": "reply_abort", "reason": "Draft discarded"], localID: id)
        model.receiveChatEvent(["type": "plan", "task_id": "actual-task", "text": "Finding notes"], localID: id)
        XCTAssertEqual(model.conversation.turns.first?.status, "working")
        XCTAssertNil(model.conversation.turns.first?.responseNotice)
        XCTAssertEqual(model.conversation.turns.first?.answer, "")
        XCTAssertFalse(model.conversation.turns.first!.isProvisional)
    }

    func testRestoringProvisionalDraftDoesNotPretendTheOldStreamIsStillRunning() throws {
        let defaults = UserDefaults(suiteName: UUID().uuidString)!
        var archive = ConversationArchive()
        let id = UUID()
        archive.begin(id: id, request: "Hello", sources: [])
        archive.append(id: id, delta: "Not final")
        defaults.set(try JSONEncoder().encode(archive), forKey: "FERBConversationArchive")
        let restored = NotchModel(preferences: defaults)
        XCTAssertEqual(restored.conversation.turns.first?.status, "interrupted")
        XCTAssertEqual(restored.conversation.turns.first?.answer, "")
        XCTAssertFalse(restored.bodyVisible)
        XCTAssertFalse(restored.completionTraceVisible)
    }

    func testExistingArchiveWithoutStreamingNoticeStillDecodes() throws {
        var archive = ConversationArchive()
        archive.begin(id: UUID(), request: "Existing history", sources: [])
        archive.finish(id: archive.turns[0].id, answer: "Saved answer", status: "done", artifacts: [])
        let data = try JSONEncoder().encode(archive)
        var object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        var turns = try XCTUnwrap(object["turns"] as? [[String: Any]])
        turns[0].removeValue(forKey: "responseNotice")
        object["turns"] = turns
        let restored = try JSONDecoder().decode(ConversationArchive.self, from: JSONSerialization.data(withJSONObject: object))
        XCTAssertEqual(restored.turns.first?.answer, "Saved answer")
    }

    private func makeModel() -> NotchModel {
        NotchModel(preferences: UserDefaults(suiteName: UUID().uuidString)!)
    }

    private func begin(_ request: String, in model: NotchModel) -> UUID {
        model.session.draft = request
        let id = model.session.beginWork()!
        model.conversation.begin(id: id, request: request, sources: [])
        return id
    }
}
