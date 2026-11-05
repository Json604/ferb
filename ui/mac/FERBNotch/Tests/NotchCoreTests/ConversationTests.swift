import XCTest
@testable import NotchCore
@testable import FERBNotch

final class ConversationTests: XCTestCase {
    // decision: APP-569
    func testTurnsPersistAndGroupByLocalDayWithoutLosingEarlierReplies() throws {
        var chat = ConversationArchive()
        let day = Calendar.current.startOfDay(for: Date())
        let yesterday = Calendar.current.date(byAdding: .day, value: -1, to: day)!
        let first = UUID(), second = UUID()
        chat.begin(id: first, request: "Yesterday", sources: [], date: yesterday)
        chat.finish(id: first, answer: "Older answer", status: "done", artifacts: [])
        chat.begin(id: second, request: "Today", sources: [], date: day)
        chat.append(id: second, delta: "Hello ")
        chat.append(id: second, delta: "again")
        XCTAssertEqual(chat.turns.last?.answer, "Hello again")
        chat.finish(id: second, answer: "Hello again!", status: "done", artifacts: [])
        let restored = try JSONDecoder().decode(ConversationArchive.self, from: JSONEncoder().encode(chat))
        XCTAssertEqual(restored.turns(on: yesterday).first?.answer, "Older answer")
        XCTAssertEqual(restored.turns(on: day).count, 1)
        XCTAssertEqual(restored.turns.last?.answer, "Hello again!")
    }

    // decision: APP-571, APP-301
    @MainActor func testTaskCompletionOpensTheClosedChatWithoutRequestingKeyboardFocus() {
        let model = NotchModel(preferences: scratchDefaults())
        model.session.draft = "Find notes"
        let id = model.session.beginWork()!
        model.conversation.begin(id: id, request: "Find notes", sources: [])
        model.session.bindBackendTask("task-1", localID: id)
        model.collapse()
        XCTAssertFalse(model.bodyVisible)
        var foregroundRequests = 0
        model.onPresentationChange = { _ in foregroundRequests += 1 }
        model.receiveOutcome(["task_id": "task-1", "status": "done", "answer": "Found them"])
        XCTAssertTrue(model.bodyVisible)
        XCTAssertEqual(foregroundRequests, 0)
    }

    // decision: APP-571
    @MainActor func testReplyOpensCollapsedPanelAndPersistsOnlyOnce() {
        let defaults = scratchDefaults()
        let model = NotchModel(preferences: defaults)
        model.session.draft = "Hello"
        let id = model.session.beginWork()!
        model.conversation.begin(id: id, request: "Hello", sources: [])
        model.session.clearComposer()
        model.session.collapse()
        var backgroundChanges = 0
        model.onBackgroundChange = { _ in backgroundChanges += 1 }
        model.receiveChatEvent(["type": "reply", "text": "Hi"], localID: id)
        model.receiveChatEvent(["type": "reply", "text": "Hi"], localID: id)
        XCTAssertTrue(model.bodyVisible)
        XCTAssertEqual(model.conversation.turns.count, 1)
        XCTAssertEqual(model.conversation.turns.first?.answer, "Hi")
        XCTAssertEqual(model.session.draft, "")
        XCTAssertGreaterThan(backgroundChanges, 0)
        XCTAssertEqual(NotchModel(preferences: defaults).conversation.turns.first?.answer, "Hi")
    }
}
