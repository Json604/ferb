import XCTest
@testable import NotchCore
@testable import FERBNotch

@MainActor
final class ReplyStreamingTests: XCTestCase {
    // decision: APP-577
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

    // decision: APP-579
    func testAuthoritativePlanClearsDraftShowsPlanLineAndRejectsLateDraftDeltas() {
        let model = makeModel()
        let id = begin("Find my notes", in: model)
        model.receiveChatEvent(["type": "reply_delta", "text": "Provisional reply"], localID: id)
        model.receiveChatEvent(["type": "plan", "task_id": "real-task", "text": "Finding your notes"], localID: id)
        model.receiveChatEvent(["type": "reply_delta", "text": "late draft"], localID: id)
        XCTAssertEqual(model.conversation.turns.first?.answer, "")
        XCTAssertEqual(model.conversation.turns.first?.backendTaskID, "real-task")
        XCTAssertEqual(model.conversation.turns.first?.status, "working")
        // L7: the plan line is a FERB line in the chat, not an entry in the hidden activity feed.
        XCTAssertEqual(model.conversation.turns.first?.plan, "Finding your notes")
        XCTAssertEqual(model.conversation.turns.first?.ferbLines, ["Finding your notes"])
        XCTAssertFalse(model.session.activity.contains { $0.title == "Finding your notes" })
    }

    // decision: APP-571, APP-570
    func testRestoringProvisionalDraftDoesNotPretendTheOldStreamIsStillRunning() throws {
        let defaults = scratchDefaults()
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

    // decision: APP-569
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
        NotchModel(preferences: scratchDefaults())
    }

    private func begin(_ request: String, in model: NotchModel) -> UUID {
        model.session.draft = request
        let id = model.session.beginWork()!
        model.conversation.begin(id: id, request: request, sources: [])
        return id
    }
}
