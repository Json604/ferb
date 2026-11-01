import AppKit
import XCTest
@testable import FERBNotch
@testable import NotchCore

@MainActor
final class TaskPresentationTests: XCTestCase {
    // decision: APP-584
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

    // decision: APP-571
    func testBackgroundReplyReopensDismissedWindowButProgressDoesNot() {
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
        XCTAssertTrue(model.bodyVisible)
        XCTAssertEqual(model.session.activeTaskID, second)
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

    func testCodeQuestionShowsItsSummaryFirstAndTheExactQuestionUnderDetails() {
        let reason = "Destination: https://appauth.b2clogin.com. Type the password or secret for it here (hidden; Enter cancels)."
        let summary = "appauth.b2clogin.com needs your password or another private answer. FERB fills it only there and doesn't save it."
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

    func testFERBsOwnQuestionsShowTheirTextVerbatimNeverAsMarkdown() {
        // D31, review round 1: backticks, asterisks, brackets and backslashes in a terminal command must show as they
        // are typed; Markdown would restyle or hide them. FERB's code-written questions are plain text.
        let command = "FERB wants to type \"echo `id` *x* [a](b) \\\\n\" into the terminal in Code, then press "
            + "Return there."
        let confirm = DaemonApproval(["approval_id": "t", "kind": "owner_question", "capability": "ferb.confirm",
                                      "reason": "FERB: this would type …. Go ahead?", "summary": command,
                                      "options": ["Yes", "No"]])!
        XCTAssertTrue(confirm.plainText)
        XCTAssertEqual(confirm.visibleReason, command)
        let codeSummary = DaemonApproval(["approval_id": "s", "kind": "owner_secret", "reason": "r", "summary": "x"])!
        XCTAssertTrue(codeSummary.plainText)
        let workerQuestion = DaemonApproval(["approval_id": "q", "kind": "owner_question", "capability": "owner.ask",
                                             "reason": "Which **file**?", "display_text": "Which file?"])!
        XCTAssertFalse(workerQuestion.plainText)                 // an ordinary question keeps its Markdown
    }

    func testSecretAndConsequentialApprovalsNeverHideTheirExactReason() {
        for kind in ["owner_login_code", "owner_admin_password", "owner_secret", "send", "approval", "owner_question"] {
            let approval = DaemonApproval(["approval_id": kind, "kind": kind, "reason": "Exact scope", "display_text": "Short"])
            XCTAssertEqual(approval?.visibleReason, "Exact scope")
        }
    }

    private func makeModel(_ rpc: DaemonRPCTransport = TaskListRPC()) -> NotchModel {
        NotchModel(rpc: rpc, preferences: scratchDefaults())
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
