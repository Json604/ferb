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

    // decision: APP-805
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

    // decision: APP-726, GATE-510, ORCH-536
    func testEachCardTakesWordsOnlyWhereTheDaemonSaysAndASafetyChecksWordsAreACorrection() async throws {
        func card(_ extra: [String: Any]) -> DaemonApproval {
            DaemonApproval(["approval_id": "a", "kind": "owner_question", "options": ["Yes", "No"]].merging(extra) { $1 })!
        }
        XCTAssertEqual(card(["options_only": true]).answerBox, .none)                    // a check-in, "Resume?"
        XCTAssertEqual(card(["runtime_switch": ["runtime": "codex", "choices": ["claude"]]]).answerBox, .none)
        XCTAssertEqual(card(["capability": "memory.forget"]).answerBox, .none)           // no task: Yes / No only
        XCTAssertEqual(card(["from_task": true]).answerBox, .answer)                     // a task's question, in a row too
        XCTAssertEqual(card(["capability": "ferb.confirm", "corrects": true]).answerBox, .correction)
        XCTAssertEqual(card(["options": []]).answerBox, .answer)                         // text-only: as before
        XCTAssertEqual(card(["kind": "owner_secret", "options": []]).answerBox, .answer)

        let rpc = RecordingRPC()
        let model = makeModel(rpc)
        let check = card(["approval_id": "c1", "capability": "ferb.confirm", "corrects": true, "task_id": "t1"])
        model.pendingApprovals = [check]
        model.correctApproval(check, text: "yes but the cheaper one")
        XCTAssertTrue(model.pendingApprovals.isEmpty)                                    // the card goes at once
        await Task.yield()
        try await Task.sleep(for: .milliseconds(20))
        XCTAssertEqual(rpc.calls.first?.0, "chat.correct")
        XCTAssertEqual(rpc.calls.first?.1["approval_id"] as? String, "c1")
        XCTAssertFalse(rpc.calls.contains { $0.0 == "chat.approve" || $0.0 == "chat.answer" })   // never a Yes
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

    // decision: ORCH-518
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
