import XCTest
@testable import FERBNotch
@testable import NotchCore

/// L7 and rule 14: the saved conversation holds the message as the daemon redacted it, never the raw typed text.
@MainActor
final class RedactedHistoryTests: XCTestCase {
    private let raw = "My password is hunter2-secret, log in to the bank"
    private let redacted = "My password is [REDACTED], log in to the bank"

    func testAckMessageReplacesRawTextInArchiveAndOnScreen() throws {
        let (model, id) = sent()
        model.receiveChatEvent(["type": "ack", "request_id": "req-1", "at": "2026-10-13T10:00:00+00:00",
                                "text": "On it.", "message": redacted], localID: id)
        XCTAssertEqual(model.conversation.turns[0].request, redacted)
        let saved = try savedText(model)
        XCTAssertTrue(saved.contains("[REDACTED]"))
        XCTAssertFalse(saved.contains("hunter2"))
    }

    func testSaveBeforeAckHoldsNoRawText() throws {
        let (model, id) = sent()
        model.saveConversation()
        XCTAssertFalse(try savedText(model).contains("hunter2"))
        // Another event before the ack still saves nothing raw.
        model.receiveChatEvent(["type": "plan", "task_id": "t1", "text": "Working."], localID: id)
        XCTAssertFalse(try savedText(model).contains("hunter2"))
        XCTAssertEqual(model.conversation.turns[0].request, raw, "raw text stays in memory only")
    }

    func testAckWithoutMessageKeepsRawTextOutOfStorage() throws {
        let (model, id) = sent()
        model.receiveChatEvent(["type": "ack", "request_id": "req-1", "text": "On it."], localID: id)
        XCTAssertFalse(try savedText(model).contains("hunter2"))
    }

    private func sent() -> (NotchModel, UUID) {
        let model = NotchModel(rpc: QuietRPC(), preferences: UserDefaults(suiteName: UUID().uuidString)!)
        model.session.draft = raw
        model.startWork()
        return (model, model.conversation.turns[0].id)
    }

    private func savedText(_ model: NotchModel) throws -> String {
        let data = try XCTUnwrap(model.preferences.data(forKey: "FERBConversationArchive"))
        return String(decoding: data, as: UTF8.self)
    }
}

private final class QuietRPC: DaemonRPCTransport {
    func call(_ method: String, params: [String: Any]) async throws -> [String: Any] {
        ["session_id": "test", "capabilities": ["input_paths"]]
    }
    func stream(_ method: String, params: [String: Any]) -> AsyncThrowingStream<[String: Any], Error> {
        AsyncThrowingStream { _ in }
    }
}
