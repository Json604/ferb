import XCTest
import Foundation
@testable import FERBNotch
@testable import NotchCore

@MainActor
final class DaemonConnectionTests: XCTestCase {
    func testStartupProfileUsesExistingApprovalWithCompleteEvidenceAndNoTaskAssociation() async {
        let rpc = RecordingRPC()
        let evidence = ["Local scan: 2026-10-11", "Code: 3733 Spotlight uses; last used 2026-10-10."]
        rpc.responses["chat.status"] = ["approvals": [["approval_id": "persona", "title": "Getting to know you",
            "capability": "setup.persona", "kind": "owner_question", "reason": "Confirm or edit your app-use profile.",
            "options": ["Save app-use profile", "Skip"], "evidence": evidence]], "running": [[String: Any]](),
            "results": [[String: Any]]()]
        let model = makeModel(rpc)
        model.session.draft = "My next request"
        await model.refreshDaemonStatus()
        XCTAssertEqual(model.pendingApprovals.first?.title, "Getting to know you")
        XCTAssertEqual(model.pendingApprovals.first?.evidence, evidence)
        XCTAssertNil(model.pendingApprovals.first?.taskID)
        XCTAssertEqual(model.session.draft, "My next request")
        XCTAssertFalse(model.pendingApprovals.first!.isSecret)
        XCTAssertTrue(model.conversation.turns.isEmpty)
    }
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

    func testDuplicateResultsDoNotReplayOrCreateAnExampleArtifact() {
        let model = makeModel(RecordingRPC())
        model.session.resumeBackendTask("real", goal: "Answer a question")
        let event: [String: Any] = ["task_id": "real", "status": "done", "answer": "An actual answer", "artifacts": [String]()]
        model.receiveOutcome(event, announce: false)
        model.receiveOutcome(event, announce: false)
        XCTAssertEqual(model.session.history.count, 1)
        XCTAssertEqual(model.session.result?.answer, "An actual answer")
        XCTAssertNil(model.session.result?.documentURL)
    }

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

    func testReconnectRestoresActualPreviouslyShownOutcomeWithoutResubmission() async {
        let rpc = RecordingRPC()
        rpc.responses["trace.get"] = ["outcome": ["result": ["status": "done", "answer": "Restored answer", "artifacts": ["/tmp/real.txt"]]]]
        let model = makeModel(rpc)
        model.preferences.set("remembered", forKey: "FERBActiveTaskID")
        await model.refreshDaemonStatus()
        XCTAssertEqual(model.session.result?.backendTaskID, "remembered")
        XCTAssertEqual(model.session.result?.answer, "Restored answer")
        XCTAssertFalse(rpc.calls.contains { $0.0 == "chat.send" })
        XCTAssertFalse(model.completionTraceVisible)
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

    func testARefusedTextAnswerShowsFERBsReasonAndTheQuestionStays() async throws {
        // L36: FERB refuses an Edit answer that came back hidden as a secret and says why; the notch showed a generic
        // "could not be confirmed" instead. A lost connection keeps the generic line (the answer may have landed).
        let rpc = RecordingRPC()
        let question: [String: Any] = ["approval_id": "edit-1", "kind": "owner_question", "capability": "memory.edit",
                                       "reason": "How should FERB remember it?", "options": [String]()]
        rpc.responses["chat.status"] = ["running": [[String: Any]](), "approvals": [question], "results": [[String: Any]]()]
        let reason = "Those words looked like a secret, so FERB hid them and saved nothing."
        rpc.failures["chat.answer"] = DaemonRPCError.remote(reason)
        let model = makeModel(rpc)
        await model.refreshDaemonStatus()
        model.answerApproval(try XCTUnwrap(model.pendingApprovals.first), text: "hunter2")
        await Task.yield()
        try await Task.sleep(for: .milliseconds(20))
        XCTAssertEqual(model.promptError, reason)
        XCTAssertEqual(model.pendingApprovals.map(\.id), ["edit-1"])
        for failure in [DaemonRPCError.disconnected, DaemonRPCError.internalFault] {   // never FERB's refusal
            model.promptError = nil
            rpc.failures["chat.answer"] = failure
            model.answerApproval(try XCTUnwrap(model.pendingApprovals.first), text: "My dentist is Dr. Rao")
            await Task.yield()
            try await Task.sleep(for: .milliseconds(20))
            XCTAssertEqual(model.promptError, "The answer could not be confirmed. Rechecking the pending request.")
        }
    }

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
        let defaults = UserDefaults(suiteName: "FERBNotchTests-" + UUID().uuidString)!
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
