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

    func testEditingOneOfTwoIdenticalSubstringsNeverPromotesPastedCopy() {
        let model = makeModel(RecordingRPC())
        model.pastedTexts = ["send example"]
        model.lastProvenanceDraft = "send example send example"
        model.session.draft = "send example send changed"
        model.preservePasteProvenance()
        let request = PastedRequest(text: model.session.draft, pastes: model.pastedTexts, editedAfterPaste: model.pasteWasEdited)
        XCTAssertTrue(request.isEntirelyPasted)
        XCTAssertTrue(request.markedText.hasPrefix("\u{1b}[200~"))
    }

    private func makeModel(_ rpc: RecordingRPC) -> NotchModel {
        let defaults = UserDefaults(suiteName: "FERBNotchTests-" + UUID().uuidString)!
        return NotchModel(rpc: rpc, preferences: defaults)
    }
}

private final class RecordingRPC: DaemonRPCTransport {
    var calls: [(String, [String: Any])] = []
    var responses: [String: [String: Any]] = ["chat.status": ["running": [[String: Any]](), "approvals": [[String: Any]](), "results": [[String: Any]]()]]
    func call(_ method: String, params: [String: Any]) async throws -> [String: Any] {
        calls.append((method, params))
        return responses[method] ?? ["ok": true]
    }
    func stream(_ method: String, params: [String: Any]) -> AsyncThrowingStream<[String: Any], Error> {
        calls.append((method, params))
        return AsyncThrowingStream { $0.finish() }
    }
}
