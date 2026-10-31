import XCTest
@testable import FERBNotch
@testable import NotchCore

/// L34 D: the facts behind the menu bar, and a clicked notification opening the notch at its item.
@MainActor
final class MenuBarModelTests: XCTestCase {
    private func makeModel(_ rpc: ScriptedRPC) -> NotchModel {
        NotchModel(rpc: rpc, preferences: scratchDefaults("MenuBarModelTests"))
    }

    private func status(running: [[String: Any]] = [], paused: Bool = false, stopped: Bool = false) -> [String: Any] {
        ["running": running, "approvals": [[String: Any]](), "results": [[String: Any]](), "paused": paused,
         "stopped": stopped]
    }

    func testTwoRefusedConnectionsMeanFERBIsntRunning() async {
        let rpc = ScriptedRPC()
        rpc.failures["chat.status"] = DaemonRPCError.unavailable
        let model = makeModel(rpc)
        await model.refreshDaemonStatus()
        XCTAssertEqual(model.menuBarState, .connecting)
        await model.refreshDaemonStatus()
        XCTAssertEqual(model.menuBarState, .notRunning)
        rpc.failures["chat.status"] = nil
        rpc.replies["chat.status"] = status(running: [["task_id": "t1", "goal": "Rename files"]])
        await model.refreshDaemonStatus()
        XCTAssertEqual(model.menuBarState, .working(1))
    }

    func testPausedAndStoppedComeFromTheDaemon() async {
        let rpc = ScriptedRPC()
        let model = makeModel(rpc)
        rpc.replies["chat.status"] = status(paused: true)
        await model.refreshDaemonStatus()
        XCTAssertEqual(model.menuBarState, .paused)
        rpc.replies["chat.status"] = status(stopped: true)
        await model.refreshDaemonStatus()
        XCTAssertEqual(model.menuBarState, .stopped)
        rpc.replies["chat.status"] = status()
        await model.refreshDaemonStatus()
        XCTAssertEqual(model.menuBarState, .ready)
    }

    func testAClickedQuestionOpensTheNotchAtItsField() {
        let model = makeModel(ScriptedRPC())
        model.pendingApprovals = [DaemonApproval(["approval_id": "a1", "kind": "owner_question", "reason": "Send it?"])!]
        model.open(.question(approvalID: "a1", taskID: nil))
        XCTAssertTrue(model.expanded)
        XCTAssertEqual(model.questionFocus?.id, "a1")
    }

    func testAClickedTaskOpensTheNotchAtTasks() {
        let model = makeModel(ScriptedRPC())
        model.runningTasks = ["t9": "Rename files"]
        model.open(.task("t9"))
        XCTAssertTrue(model.expanded && model.showingTasks)
        model.collapse()
        model.open(.task("gone"))                                // a task this notch doesn't know: just the notch
        XCTAssertTrue(model.expanded)
        XCTAssertFalse(model.showingTasks)
    }

    func testAnAnsweredQuestionTellsThePosterToWithdrawIt() {
        let model = makeModel(ScriptedRPC())
        var withdrawn: [String] = []
        model.onApprovalResolved = { withdrawn.append($0) }
        model.pendingApprovals = [DaemonApproval(["approval_id": "a1", "kind": "owner_question", "reason": "Send it?"])!]
        model.receiveBusEvent(["topic": "approval.resolved", "data": ["approval_id": "a1"]])
        XCTAssertEqual(withdrawn, ["a1"])
        XCTAssertTrue(model.pendingApprovals.isEmpty)
    }
}
