import XCTest
@testable import NotchCore
@testable import FERBNotch

final class ConversationTests: XCTestCase {
    func testAFinishedPartIsShownUnderThePlanAndSavedBeforeTheAnswer() throws {
        // A friend's feedback 2026-10-27: "when ferb did one it should say so and not wait"
        var chat = ConversationArchive()
        let id = UUID()
        chat.begin(id: id, request: "delete the android sdk and tell me what homebrew stores", sources: [])
        chat.showPlan(id: id, text: "On it.")
        chat.addPart(id: id, text: "Deleted the Android SDK — 4.2 GB freed.")
        chat.addPart(id: id, text: "")                                       // nothing to tell: nothing added
        chat.addPart(id: id, text: "Homebrew holds 9 GB, mostly llvm.")
        XCTAssertEqual(chat.turns[0].ferbLines, ["On it.", "Deleted the Android SDK — 4.2 GB freed.",
                                                 "Homebrew holds 9 GB, mostly llvm."])
        let saved = try JSONDecoder().decode(ConversationArchive.self, from: JSONEncoder().encode(chat))
        XCTAssertEqual(saved.turns[0].parts, ["Deleted the Android SDK — 4.2 GB freed.", "Homebrew holds 9 GB, mostly llvm."])
        var older = try JSONSerialization.jsonObject(with: JSONEncoder().encode(chat.turns[0])) as! [String: Any]
        older.removeValue(forKey: "parts")                                  // a chat saved before parts existed
        let turn = try JSONDecoder().decode(ConversationTurn.self, from: JSONSerialization.data(withJSONObject: older))
        XCTAssertNil(turn.parts)
    }

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

    @MainActor func testIncrementalReplyKeepsOneTurnAndFinalTextReplacesChunks() {
        let model = NotchModel(preferences: UserDefaults(suiteName: UUID().uuidString)!)
        model.session.draft = "Hello"
        let id = model.session.beginWork()!
        model.conversation.begin(id: id, request: "Hello", sources: [])
        model.receiveChatEvent(["type": "reply_delta", "text": "Hel"], localID: id)
        model.receiveChatEvent(["type": "reply_delta", "text": "lo"], localID: id)
        XCTAssertEqual(model.conversation.turns.first?.answer, "Hello")
        XCTAssertEqual(model.session.activeTaskID, id)
        model.receiveChatEvent(["type": "reply", "text": "Hello!"], localID: id)
        model.receiveChatEvent(["type": "reply_delta", "text": "late"], localID: id)
        XCTAssertEqual(model.conversation.turns.count, 1)
        XCTAssertEqual(model.conversation.turns.first?.answer, "Hello!")
    }

    @MainActor func testTaskCompletionKeepsPinnedHeaderTuckedWithoutRequestingKeyboardFocus() {
        let model = NotchModel(preferences: UserDefaults(suiteName: UUID().uuidString)!)
        model.session.draft = "Find notes"
        let id = model.session.beginWork()!
        model.conversation.begin(id: id, request: "Find notes", sources: [])
        model.session.bindBackendTask("task-1", localID: id)
        model.expand()
        model.togglePin()
        XCTAssertTrue(model.isHeaderOnly)
        var foregroundRequests = 0
        model.onPresentationChange = { _ in foregroundRequests += 1 }
        model.receiveOutcome(["task_id": "task-1", "status": "done", "answer": "Found them"])
        XCTAssertFalse(model.bodyVisible)
        XCTAssertTrue(model.isPinned)
        XCTAssertEqual(foregroundRequests, 0)
    }

    @MainActor func testReplyKeepsCollapsedPanelClosedAndPersistsOnlyOnce() {
        let defaults = UserDefaults(suiteName: UUID().uuidString)!
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
        XCTAssertFalse(model.bodyVisible)
        XCTAssertEqual(model.conversation.turns.count, 1)
        XCTAssertEqual(model.conversation.turns.first?.answer, "Hi")
        XCTAssertEqual(model.session.draft, "")
        XCTAssertGreaterThan(backgroundChanges, 0)
        XCTAssertEqual(NotchModel(preferences: defaults).conversation.turns.first?.answer, "Hi")
    }
}
