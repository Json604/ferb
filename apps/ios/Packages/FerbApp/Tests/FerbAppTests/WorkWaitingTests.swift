import FerbAppModel
import FerbDesign
import FerbMobile
import XCTest

/// The Tasks tab: a task that waits for the owner is under "Waiting for you", never under Running, and listed once when
/// its open question is there already (owner 2026-11-23: two ChatGPT watches asking him were under Running).
final class WorkWaitingTests: XCTestCase {
    private func row(_ id: String, _ name: String, _ status: String) -> TaskRow {
        TaskRow(taskId: id, name: name, goal: "", status: status, createdAt: nil, updatedAt: nil, answer: "",
                stepsDone: nil, stepsTotal: nil)
    }

    // decision: PHN-802
    func testATaskWaitingForTheOwnerIsUnderWaitingOnce() {
        let asked = WorkItem(kind: .asking, title: "Renew the domain", detail: "Pay for another year?", approvalId: "ap-1", taskId: "t3")
        let w = WorkModel.fromMac([row("t1", "Bring in what ChatGPT knows", "needs_user"), row("t2", "Compare desks", "working"),
                                   row("t3", "Renew the domain", "needs_user")], waiting: [asked], chat: .empty)
        XCTAssertEqual(w.waiting.map(\.title), ["Renew the domain", "Bring in what ChatGPT knows"])
        XCTAssertEqual(w.waiting.map(\.approvalId), ["ap-1", nil])
        XCTAssertEqual(w.running.map(\.title), ["Compare desks"])
    }
}
