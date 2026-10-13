import AppKit
import XCTest
@testable import FERBNotch
@testable import NotchCore

@MainActor
final class ContinuousChatTests: XCTestCase {
    func testTwoMessagesCanRunAndFinishOutOfOrderWithoutErasingNextDraft() {
        let rpc = ChatRPC()
        let model = NotchModel(rpc: rpc, preferences: UserDefaults(suiteName: UUID().uuidString)!)
        model.session.draft = "First task"
        model.startWork()
        let first = model.conversation.turns[0].id
        XCTAssertEqual(model.session.draft, "")
        XCTAssertTrue(model.canCompose)
        model.receiveChatEvent(["type": "plan", "task_id": "first"], localID: first)
        model.session.draft = "Second task"
        model.startWork()
        let second = model.conversation.turns[1].id
        model.receiveChatEvent(["type": "plan", "task_id": "second"], localID: second)
        model.session.draft = "Still typing"
        model.receiveOutcome(["task_id": "first", "status": "done", "answer": "First answer"])
        XCTAssertEqual(model.session.draft, "Still typing")
        XCTAssertEqual(model.session.backendTaskID, "second")
        XCTAssertEqual(model.conversation.turns[0].answer, "First answer")
        XCTAssertTrue(model.hasRunningWork)
        model.receiveOutcome(["task_id": "second", "status": "done", "answer": "Second answer"])
        XCTAssertEqual(model.session.draft, "Still typing")
        XCTAssertFalse(model.hasRunningWork)
        XCTAssertEqual(model.conversation.turns[1].answer, "Second answer")
        model.disconnectDaemon()
    }

    func testTranscriptHasReadableMinimumAndCapsLongConversations() {
        let model = NotchModel(preferences: UserDefaults(suiteName: UUID().uuidString)!)
        model.measuredTranscriptHeight = 40
        XCTAssertEqual(model.chatTranscriptHeight, 240)
        model.measuredTranscriptHeight = 800
        XCTAssertEqual(model.chatTranscriptHeight, 320)
    }

    func testReturnIsConsumedOnceAndShiftReturnInsertsNewline() {
        let editor = ChatTextView()
        var submits = 0
        editor.submit = { submits += 1 }
        func key(_ flags: NSEvent.ModifierFlags) -> NSEvent {
            NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: flags,
                timestamp: 0, windowNumber: 0, context: nil, characters: "\r",
                charactersIgnoringModifiers: "\r", isARepeat: false, keyCode: 36)!
        }
        editor.keyDown(with: key([]))
        XCTAssertEqual(submits, 1)
        XCTAssertEqual(editor.string, "")
        editor.keyDown(with: key(.shift))
        XCTAssertEqual(submits, 1)
        XCTAssertEqual(editor.string, "\n")
    }
}

private final class ChatRPC: DaemonRPCTransport {
    func call(_ method: String, params: [String: Any]) async throws -> [String: Any] {
        ["session_id": "test", "capabilities": ["input_paths"]]
    }
    func stream(_ method: String, params: [String: Any]) -> AsyncThrowingStream<[String: Any], Error> {
        AsyncThrowingStream { _ in }
    }
}
