import XCTest
@testable import NotchCore

/// Owner 2026-11-10, the EF Bridge run: after "cancel the task" the stop was told twice, once by the stop message's
/// reply and once by the task's own end posted below it. The reply now tells it, and the end it carries (`told_in`) is
/// filed on the task's own turn as its state (ORCH-579, APP-1113).
final class StopToldOnceTests: XCTestCase {
    // decision: ORCH-579, APP-1113
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
        // ORCH-688, APP-1113: the words are the stop's reply's; the task's turn shows its state alone
        XCTAssertEqual(archive.turns.first?.answer, "")
        XCTAssertEqual(archive.turns.last?.answer, "Stopped, as you asked.")

        // without told_in (a stop the reply couldn't tell: still finishing), the end is a reply below, as before
        var other = ConversationArchive()
        other.begin(id: task, request: "fill in the EF application", sources: [], date: start)
        other.begin(id: stop, request: "cancel the task", sources: [], date: start + 60)
        other.finish(id: stop, answer: "I've asked it to stop.", status: "done", artifacts: [], at: start + 70)
        other.finish(id: task, answer: "Stopped, as you asked.", status: "cancelled", artifacts: [], at: start + 90)
        XCTAssertEqual(other.replies.map(\.turnID), [task])
    }

    /// Owner 2026-11-22 (ORCH-688): the stop's reply and the task's own turn each showed "Stopped, as you asked. This is
    /// where things stood: …", its sent card and its sites chip. The end's words, lines and cards are the reply's; the
    /// task's turn reads Cancelled, in either order of the two.
    // decision: ORCH-688, APP-1113
    func testAStopsEndIsShownOnceWithItsCardsUnderTheStopMessage() {
        let words = "Stopped, as you asked. This is where things stood:\n\nQuestions 1–6 are saved.\n\nSites contacted: seek.example"
        let cards: [[String: Any]] = [["type": "send", "kind": "send", "recipient": "seek.example", "site": "seek.example"]]
        for outcomeFirst in [true, false] {
            var archive = ConversationArchive()
            let task = UUID(), stop = UUID()
            let start = Date(timeIntervalSince1970: 1_000)
            archive.begin(id: task, request: "solve the graded ones of week 7", sources: [], date: start)
            _ = archive.bindRequest(id: task, requestID: "req-task")
            archive.bind(id: task, taskID: "t1")
            archive.begin(id: stop, request: "lets call this here", sources: [], date: start + 60)
            _ = archive.bindRequest(id: stop, requestID: "req-stop")
            let outcome: [String: Any] = ["type": "outcome", "task_id": "t1", "request_id": "req-task", "status": "cancelled",
                                          "answer": words, "lines": ["Sites contacted: seek.example"], "cards": cards,
                                          "artifacts": ["/tmp/a.pdf"], "told_in": "req-stop"]
            func end() {
                _ = archive.applyOutcome(outcome)
                archive.finish(id: task, answer: words, status: "cancelled", artifacts: [URL(fileURLWithPath: "/tmp/a.pdf")],
                               at: start + 90)
                _ = archive.applyOutcome(outcome)
            }
            if outcomeFirst { end() }
            archive.finish(id: stop, answer: words, status: "done", artifacts: [], at: start + 91)
            archive.showCards(id: stop, cards)
            if !outcomeFirst { end() }
            let own = archive.turns[0]
            XCTAssertEqual(own.status, "cancelled")
            XCTAssertEqual(own.answer, "")
            XCTAssertNil(own.cards)
            XCTAssertNil(own.codeLines)
            XCTAssertTrue(own.artifacts.isEmpty)
            XCTAssertEqual(archive.turns[1].answer, words)
            XCTAssertEqual(archive.turns[1].cards?.count, 1)          // the card and chip once, under the stop
            XCTAssertEqual(archive.replies.count, 0, "outcome first \(outcomeFirst)")
            XCTAssertEqual(archive.turns.filter { $0.answer == words }.count, 1)
        }
    }

    /// A restart's first run (ORCH-683) is filed in place with its own line: `keeps_words`.
    // decision: ORCH-683, APP-1113
    func testARestartsFirstRunKeepsItsLineOnItsOwnTurn() {
        var archive = ConversationArchive()
        let task = UUID(), follow = UUID()
        let start = Date(timeIntervalSince1970: 1_000)
        archive.begin(id: task, request: "watch the build", sources: [], date: start)
        _ = archive.bindRequest(id: task, requestID: "req-task")
        archive.bind(id: task, taskID: "t1")
        archive.begin(id: follow, request: "do both at once", sources: [], date: start + 60)
        _ = archive.bindRequest(id: follow, requestID: "req-follow")
        let line = "Stopped here to start again with your change."
        let outcome: [String: Any] = ["type": "outcome", "task_id": "t1", "request_id": "req-task", "status": "cancelled",
                                      "answer": line, "told_in": "req-follow", "keeps_words": true]
        _ = archive.applyOutcome(outcome)
        archive.finish(id: task, answer: line, status: "cancelled", artifacts: [], at: start + 90)
        XCTAssertEqual(archive.turns[0].answer, line)
        XCTAssertEqual(archive.replies.count, 0)
    }
}
