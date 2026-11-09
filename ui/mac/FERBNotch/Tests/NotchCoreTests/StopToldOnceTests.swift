import XCTest
@testable import NotchCore

/// Owner 2026-11-10, the EF Bridge run: after "cancel the task" the stop was told twice, once by the stop message's
/// reply and once by the task's own end posted below it. The reply now tells it, and the end it carries (`told_in`) is
/// filed on the task's own turn (ORCH-579, APP-825).
final class StopToldOnceTests: XCTestCase {
    // decision: ORCH-579, APP-825
    func testATasksEndTheStopReplyTellsIsFiledOnItsOwnTurn() {
        var archive = ConversationArchive()
        let task = UUID(), stop = UUID()
        let start = Date(timeIntervalSince1970: 1_000)
        archive.begin(id: task, request: "fill in the EF application", sources: [], date: start)
        _ = archive.bindRequest(id: task, requestID: "req-task")
        archive.bind(id: task, taskID: "t1")
        archive.begin(id: stop, request: "cancel the task", sources: [], date: start + 60)
        _ = archive.bindRequest(id: stop, requestID: "req-stop")
        // the task's outcome arrives first, naming the stop's request as the reply that tells it
        let outcome: [String: Any] = ["type": "outcome", "task_id": "t1", "request_id": "req-task",
                                      "status": "cancelled", "answer": "Stopped, as you asked.", "told_in": "req-stop"]
        _ = archive.applyOutcome(outcome)
        archive.finish(id: task, answer: "Stopped, as you asked.", status: "cancelled", artifacts: [], at: start + 90)
        _ = archive.applyOutcome(outcome)                        // applied again once filed: nothing pending after
        archive.finish(id: stop, answer: "Stopped, as you asked.", status: "done", artifacts: [], at: start + 91)
        XCTAssertEqual(archive.replies.count, 0)                   // told once: the stop's reply, in place
        XCTAssertEqual(archive.turns.first?.status, "cancelled")
        XCTAssertEqual(archive.turns.first?.answer, "Stopped, as you asked.")

        // without told_in (a stop the reply couldn't tell: still finishing), the end is a reply below, as before
        var other = ConversationArchive()
        other.begin(id: task, request: "fill in the EF application", sources: [], date: start)
        other.begin(id: stop, request: "cancel the task", sources: [], date: start + 60)
        other.finish(id: stop, answer: "I've asked it to stop.", status: "done", artifacts: [], at: start + 70)
        other.finish(id: task, answer: "Stopped, as you asked.", status: "cancelled", artifacts: [], at: start + 90)
        XCTAssertEqual(other.replies.map(\.turnID), [task])
    }
}
