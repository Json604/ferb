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
        // L6: the first answer arrived after the second request, so it appears at the bottom quoting its question.
        XCTAssertEqual(model.visibleEntries.map(describe), ["First task (answered below)", "Second task", "reply to First task: First answer"])
        XCTAssertTrue(model.hasRunningWork)
        model.receiveOutcome(["task_id": "second", "status": "done", "answer": "Second answer"])
        XCTAssertEqual(model.session.draft, "Still typing")
        XCTAssertFalse(model.hasRunningWork)
        XCTAssertEqual(model.conversation.turns[1].answer, "Second answer")
        // The second answer arrived after the first reply, so it goes below it: the chat reads in arrival order.
        XCTAssertEqual(model.visibleEntries.map(describe), ["First task (answered below)", "Second task (answered below)",
                                                            "reply to First task: First answer", "reply to Second task: Second answer"])
        model.disconnectDaemon()
    }

    func testFirstStreamedAnswerReopensFERBAndShowsAttentionWithoutTakingFocus() {
        let model = NotchModel(rpc: ChatRPC(), preferences: UserDefaults(suiteName: UUID().uuidString)!)
        model.session.draft = "First task"
        model.startWork()
        let turnID = model.conversation.turns[0].id
        model.collapse()
        let focusRequest = model.focusRequest

        model.receiveChatEvent(["type": "reply_delta", "text": "First words"], localID: turnID)

        XCTAssertTrue(model.expanded)
        XCTAssertTrue(model.showsTraceGlow)
        XCTAssertEqual(model.focusRequest, focusRequest)
        XCTAssertEqual(model.conversation.turns[0].answer, "First words")
        model.disconnectDaemon()
    }

    private func describe(_ entry: TranscriptEntry) -> String {
        switch entry {
        case let .turn(turn, answeredBelow): return turn.request + (answeredBelow ? " (answered below)" : "")
        case let .reply(_, answering): return "reply to " + answering.quote + ": " + answering.visibleAnswer
        case let .notice(notice): return "notice: " + notice.text
        }
    }

    func testTranscriptStartsRoomyAndUsesTheSavedResizableHeight() {
        let model = NotchModel(preferences: UserDefaults(suiteName: UUID().uuidString)!)
        XCTAssertEqual(model.bodyWidth, 440)
        XCTAssertEqual(model.chatTranscriptHeight, 300)
        model.resizePanel(width: 520, transcriptHeight: 380, persist: true)
        XCTAssertEqual(model.bodyWidth, 520)
        XCTAssertEqual(model.chatTranscriptHeight, 380)
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
