import AppKit
import XCTest
@testable import FERBNotch
@testable import NotchCore

@MainActor
final class TaskPresentationTests: XCTestCase {
    func testCommandPasteReplacesSelectionAndRecordsPlainTextOnce() {
        let editor = ChatTextView()
        editor.isRichText = false
        editor.pasteboard = NSPasteboard.withUniqueName()
        defer { editor.pasteboard.releaseGlobally() }
        editor.pasteboard.setString("pasted text", forType: .string)
        editor.string = "before selected after"
        editor.setSelectedRange(NSRange(location: 7, length: 8))
        var provenance: [String] = []
        var selections: [NSRange] = []
        editor.didPaste = { text, selection in provenance.append(text); selections.append(selection) }
        let event = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: .command,
            timestamp: 0, windowNumber: 0, context: nil, characters: "v",
            charactersIgnoringModifiers: "v", isARepeat: false, keyCode: 9)!
        XCTAssertTrue(editor.performKeyEquivalent(with: event))
        XCTAssertEqual(editor.string, "before pasted text after")
        XCTAssertEqual(provenance, ["pasted text"])
        XCTAssertEqual(selections, [NSRange(location: 7, length: 8)])
    }

    func testBackgroundReplyAndProgressNeverReopenDismissedWindow() {
        let model = makeModel()
        model.session.draft = "First task"
        let first = model.session.beginWork()!
        model.session.expand()
        model.workSessions[first] = model.session
        model.conversation.begin(id: first, request: "First task", sources: [])
        model.session = NotchSession()
        model.session.draft = "Second task"
        let second = model.session.beginWork()!
        model.conversation.begin(id: second, request: "Second task", sources: [])
        model.collapse()
        model.receiveChatEvent(["type": "subtask", "summary": "Progress"], localID: first)
        XCTAssertFalse(model.bodyVisible)
        model.receiveChatEvent(["type": "reply", "text": "Done"], localID: first)
        XCTAssertFalse(model.bodyVisible)
        XCTAssertEqual(model.session.activeTaskID, second)
    }

    func testTaskListDeduplicatesLocalAndBackendAndIncludesRecoveredTasks() {
        let model = makeModel()
        model.session.draft = "First task"
        let local = model.session.beginWork()!
        model.session.bindBackendTask("first", localID: local)
        model.workSessions[local] = model.session
        model.conversation.begin(id: local, request: "First task", sources: [])
        model.conversation.bind(id: local, taskID: "first")
        model.runningTasks = ["first": "First task", "recovered": "Recovered task"]
        XCTAssertEqual(model.taskItems.count, 2)
        XCTAssertEqual(Set(model.taskItems.compactMap(\.backendID)), ["first", "recovered"])
    }

    func testTaskHistoryListsTheNewestFirst() {
        // Owner 2026-10-25: the list showed older tasks on top (it sorted by the random task id).
        let model = makeModel()
        for (request, taskID) in [("Oldest", "zzz"), ("Middle", "aaa"), ("Newest", "mmm")] {
            let local = UUID()
            model.conversation.begin(id: local, request: request, sources: [])
            model.conversation.bind(id: local, taskID: taskID)
        }
        XCTAssertEqual(model.taskItems.compactMap(\.backendID), ["mmm", "aaa", "zzz"])
    }

    func testStopOneUsesOnlySelectedBackendIdentity() async {
        let rpc = TaskListRPC()
        let model = makeModel(rpc)
        model.runningTasks = ["one": "One", "two": "Two"]
        let stopped = expectation(description: "Exact stop sent")
        rpc.onCall = { method in if method == "task.stop" { stopped.fulfill() } }
        model.stopTask(model.taskItems.first { $0.backendID == "two" }!)
        await fulfillment(of: [stopped], timeout: 2)
        XCTAssertEqual(rpc.calls.first?.0, "task.stop")
        XCTAssertEqual(rpc.calls.first?.1["task_ids"] as? [String], ["two"])
    }

    func testDisplayTextPreservesFullAnswerInArchive() throws {
        let model = makeModel()
        model.session.draft = "Question"
        let local = model.session.beginWork()!
        model.conversation.begin(id: local, request: "Question", sources: [])
        model.receiveChatEvent(["type": "reply", "text": "Full explanation with evidence.", "display_text": "Done."], localID: local)
        XCTAssertEqual(model.conversation.turns.first?.answer, "Full explanation with evidence.")
        XCTAssertEqual(model.conversation.turns.first?.displayText, "Done.")
        let restored = try JSONDecoder().decode(ConversationArchive.self, from: JSONEncoder().encode(model.conversation))
        XCTAssertEqual(restored.turns.first?.displayText, "Done.")
    }

    func testApprovalSummaryPreservesFullQuestionAndLegacyFallback() {
        let value: [String: Any] = ["approval_id": "question", "reason": "Full question and context.", "display_text": "Which app?", "kind": "owner_question", "capability": "owner.ask"]
        let approval = DaemonApproval(value)!
        XCTAssertEqual(approval.visibleReason, "Which app?")
        XCTAssertEqual(approval.reason, "Full question and context.")
        XCTAssertEqual(DaemonApproval(["approval_id": "old", "reason": "Original?"])?.visibleReason, "Original?")
    }

    func testStopWaitingIdentityNeverStopsAnotherTask() async {
        let rpc = TaskListRPC()
        let model = makeModel(rpc)
        model.runningTasks = ["other": "Other task"]
        model.session.draft = "New task"
        let local = model.session.beginWork()!
        model.conversation.begin(id: local, request: "New task", sources: [])
        model.stopTask(model.taskItems.first { $0.localID == local }!)
        XCTAssertEqual(model.pendingStops, [local])
        XCTAssertTrue(rpc.calls.isEmpty)
        let stopped = expectation(description: "Stop pending identity only")
        rpc.onCall = { method in if method == "task.stop" { stopped.fulfill() } }
        model.receiveChatEvent(["type": "plan", "task_id": "new", "goal": "New task"], localID: local)
        await fulfillment(of: [stopped], timeout: 2)
        XCTAssertEqual(rpc.calls.first?.1["task_ids"] as? [String], ["new"])
        XCTAssertEqual(rpc.calls.filter { $0.0 == "task.stop" }.count, 1)
    }

    func testInspectPreservesComposerSelectionAndPasteProvenance() async {
        let rpc = TaskListRPC()
        let model = makeModel(rpc)
        model.runningTasks = ["other": "Other task"]
        model.session.draft = "My pasted draft"
        model.pasteProvenance.paste("pasted", over: NSRange(location: 3, length: 0), in: "My  draft")
        let provenance = model.pasteProvenance
        let focus = model.focusRequest
        let read = expectation(description: "Read exact trace")
        rpc.onCall = { method in if method == "trace.get" { read.fulfill() } }
        model.inspectTask(model.taskItems[0])
        await fulfillment(of: [read], timeout: 2)
        XCTAssertEqual(rpc.calls.first?.1["task_id"] as? String, "other")
        XCTAssertEqual(model.session.draft, "My pasted draft")
        XCTAssertEqual(model.pasteProvenance, provenance)
        XCTAssertEqual(model.focusRequest, focus)
        XCTAssertFalse(model.bodyVisible)
    }

    func testWorkerApprovalMapsToKnownRootWithoutGuessingUnscopedApproval() {
        let model = makeModel()
        model.runningTasks = ["root": "Task", "other": "Another"]
        model.receiveBusEvent(["topic": "approval.requested", "data": ["approval_id": "q", "task_id": "root.worker", "reason": "Which?"]])
        XCTAssertEqual(model.pendingApprovals.first?.taskID, "root")
        XCTAssertEqual(model.taskItems.first(where: { $0.backendID == "root" })?.status, "needs_user")
        XCTAssertNil(model.normalizeApproval(["approval_id": "unscoped"])?.taskID)
        XCTAssertEqual(model.normalizeApproval(["approval_id": "unknown", "task_id": "rootish.worker"])?.taskID, "rootish.worker")
    }

    func testCodeQuestionShowsItsSummaryFirstAndTheExactQuestionUnderDetails() {
        let reason = "Destination: https://atlasauth.b2clogin.com. Type the password or secret for it here (hidden; Enter cancels)."
        let summary = "atlasauth.b2clogin.com needs your password or another private answer. FERB fills it only there and doesn't save it."
        let secret = DaemonApproval(["approval_id": "s", "kind": "owner_secret", "capability": "secret.fill",
                                     "reason": reason, "summary": summary, "display_text": "Model words"])!
        XCTAssertEqual(secret.visibleReason, summary)
        XCTAssertEqual(secret.details, reason)
        let confirm: [String: Any] = ["approval_id": "c", "kind": "owner_question", "capability": "ferb.confirm",
                                      "reason": "FERB: this sends to bob@x.com, whom you didn't name in this request. Go ahead?",
                                      "options": ["Yes", "No"], "display_text": "Model short"]
        XCTAssertEqual(DaemonApproval(confirm)?.visibleReason, confirm["reason"] as? String)   // D33: model text never
        XCTAssertNil(DaemonApproval(confirm)?.details)
        var withSummary = confirm
        withSummary["summary"] = "FERB wants to send to bob@x.com, whom you didn't name."
        XCTAssertEqual(DaemonApproval(withSummary)?.visibleReason, "FERB wants to send to bob@x.com, whom you didn't name.")
        XCTAssertEqual(DaemonApproval(withSummary)?.details, confirm["reason"] as? String)
        XCTAssertNil(DaemonApproval(["approval_id": "plain", "reason": "Which file?"])?.details)
    }

    func testSecretAndConsequentialApprovalsNeverHideTheirExactReason() {
        for kind in ["owner_login_code", "owner_admin_password", "owner_secret", "send", "approval", "owner_question"] {
            let approval = DaemonApproval(["approval_id": kind, "kind": kind, "reason": "Exact scope", "display_text": "Short"])
            XCTAssertEqual(approval?.visibleReason, "Exact scope")
        }
    }

    private func makeModel(_ rpc: DaemonRPCTransport = TaskListRPC()) -> NotchModel {
        NotchModel(rpc: rpc, preferences: UserDefaults(suiteName: UUID().uuidString)!)
    }
}

private final class TaskListRPC: DaemonRPCTransport {
    private let recorded = RecordedCalls()
    var calls: [(String, [String: Any])] { recorded.all }
    var onCall: ((String) -> Void)?
    func call(_ method: String, params: [String: Any]) async throws -> [String: Any] {
        recorded.append(method, params)
        onCall?(method)
        return [:]
    }
    func stream(_ method: String, params: [String: Any]) -> AsyncThrowingStream<[String: Any], Error> {
        AsyncThrowingStream { _ in }
    }
}
