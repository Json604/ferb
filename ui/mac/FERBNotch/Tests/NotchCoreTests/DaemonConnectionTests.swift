import XCTest
import Foundation
@testable import FERBNotch
@testable import NotchCore

@MainActor
final class DaemonConnectionTests: XCTestCase {
    func testStopBeforePlanWaitsForExactBackendID() async throws {
        let rpc = RecordingRPC()
        let model = makeModel(rpc)
        model.session.draft = "Find my file"
        let id = model.session.beginWork()!
        model.requestDaemonStop()
        XCTAssertTrue(rpc.calls.isEmpty)
        model.receiveChatEvent(["type": "plan", "task_id": "backend-real", "goal": "Find file"], localID: id)
        await Task.yield()
        try await Task.sleep(for: .milliseconds(20))
        let stop = rpc.calls.first { $0.0 == "task.stop" }
        XCTAssertEqual(stop?.1["task_ids"] as? [String], ["backend-real"])
        XCTAssertFalse(rpc.calls.contains { $0.0 == "chat.send" })
    }

    // decision: APP-569
    func testSecretAnswerUsesOnlyDedicatedApprovalRPCAndIsNotSessionText() async throws {
        let rpc = RecordingRPC()
        let model = makeModel(rpc)
        model.session.draft = "Sign in"
        let approval = DaemonApproval(["approval_id": "otp-1", "kind": "owner_login_code", "reason": "Enter code"])!
        model.pendingApprovals = [approval]
        model.answerApproval(approval, text: "private-test-value")
        await Task.yield()
        try await Task.sleep(for: .milliseconds(20))
        XCTAssertEqual(rpc.calls.first?.0, "chat.answer")
        XCTAssertEqual(rpc.calls.first?.1["approval_id"] as? String, "otp-1")
        XCTAssertEqual(model.session.draft, "Sign in")
        XCTAssertTrue(model.session.history.isEmpty)
        XCTAssertFalse(model.session.activity.contains { $0.title.contains("private-test-value") })
        XCTAssertTrue(model.pendingApprovals.isEmpty)
    }

    func testDuplicateOptionUsesExactOneBasedChoiceAndDenialDoesNotGrant() async throws {
        let rpc = RecordingRPC()
        let model = makeModel(rpc)
        let approval = DaemonApproval(["approval_id": "duplicate", "options": ["Same", "Same"]])!
        model.chooseApproval(approval, index: 1)
        await Task.yield()
        try await Task.sleep(for: .milliseconds(20))
        XCTAssertEqual(rpc.calls.first?.0, "chat.approve")
        XCTAssertEqual(rpc.calls.first?.1["choice"] as? Int, 2)
        XCTAssertEqual(rpc.calls.first?.1["granted"] as? Bool, true)
        model.declineApproval(approval)
        await Task.yield()
        try await Task.sleep(for: .milliseconds(20))
        XCTAssertEqual(rpc.calls.last?.1["granted"] as? Bool, false)
        XCTAssertNil(rpc.calls.last?.1["choice"])
    }

    // decision: APP-584
    func testEditingOneOfTwoIdenticalSubstringsNeverPromotesPastedCopy() {
        let model = makeModel(RecordingRPC())
        model.pasteProvenance.edit(NSRange(location: 0, length: 0), with: "send example ", in: "")
        model.pasteProvenance.paste("send example", over: NSRange(location: 13, length: 0), in: "send example ")
        // An edit no text view reported, reaching the pasted copy.
        model.session.draft = "send example send changed"
        model.preservePasteProvenance()
        let request = PastedRequest(text: model.session.draft, provenance: model.pasteProvenance)
        XCTAssertEqual(request.markedText, "\u{1b}[200~send example send changed\u{1b}[201~")
    }

    private func makeModel(_ rpc: RecordingRPC) -> NotchModel {
        let defaults = scratchDefaults()
        return NotchModel(rpc: rpc, preferences: defaults)
    }
}

private final class RecordingRPC: DaemonRPCTransport {
    private let recorded = RecordedCalls()
    var calls: [(String, [String: Any])] { recorded.all }
    var responses: [String: [String: Any]] = ["chat.status": ["running": [[String: Any]](), "approvals": [[String: Any]](), "results": [[String: Any]]()]]
    var failures: [String: Error] = [:]
    func call(_ method: String, params: [String: Any]) async throws -> [String: Any] {
        recorded.append(method, params)
        if let failure = failures[method] { throw failure }
        return responses[method] ?? ["ok": true]
    }
    func stream(_ method: String, params: [String: Any]) -> AsyncThrowingStream<[String: Any], Error> {
        recorded.append(method, params)
        return AsyncThrowingStream { $0.finish() }
    }
}
