import XCTest
@testable import FERBNotch
@testable import NotchCore

/// Send to developer (L35): a button under failed, interrupted and planning-failure answers. A click asks the daemon
/// for the report file (`reports.build`); the owner can view it, and Send has the daemon post it to the developer's
/// report relay (`reports.send`, owner decision 2026-11-01). A send that fails keeps the file and offers Send again.
@MainActor
final class FailureReportTests: XCTestCase {
    private let reply: [String: Any] = ["path": "/tmp/FERB-report-2026-10-25-task-1.txt",
                                        "subject": "FERB report: I couldn't finish that: the folder was protected.",
                                        "summary": "What happened: I couldn't finish that: the folder was protected."]

    func testOnlyFailedInterruptedAndPlanningFailureAnswersOfferTheButton() {
        func turn(_ status: String, task: String? = "task-1", request: String? = "req-1") -> ConversationTurn {
            var turn = ConversationTurn(id: UUID(), createdAt: Date(), request: "Rename the invoices", sources: [],
                                        backendTaskID: task)
            turn.status = status
            turn.requestID = request
            return turn
        }
        XCTAssertTrue(turn("failed").offersReport)
        XCTAssertTrue(turn("interrupted").offersReport)
        XCTAssertEqual(turn("failed").reportID, "task-1")
        // A planning failure that ended the turn has no task: the report is its request's.
        XCTAssertTrue(turn("failed", task: nil).offersReport)
        XCTAssertEqual(turn("failed", task: nil).reportID, "req-1")
        for status in ["done", "cancelled", "done_unverified", "needs_user", "working", "skipped"] {
            XCTAssertFalse(turn(status).offersReport, status)
        }
        // A draft the notch cut off before FERB confirmed it is the notch's own interruption, no task's.
        XCTAssertFalse(turn("interrupted", task: nil).offersReport)
        // Nothing FERB knows to name: nothing to report.
        XCTAssertFalse(turn("failed", task: nil, request: nil).offersReport)
    }

    func testAReplyThatEndedATurnOnAFailureMarksItFailedAndAnOrdinaryReplyDoesNot() throws {
        let model = makeModel(ReportRPC())
        let failed = begin("stop the invoice renaming", in: model)
        model.receiveChatEvent(["type": "ack", "request_id": "req-p", "text": "On it.", "at": at], localID: failed)
        model.receiveChatEvent(["type": "reply", "request_id": "req-p", "status": "failed", "at": at,
                                "text": "I couldn't start that: the plan still had problems after one repair."],
                               localID: failed)
        let planning = try XCTUnwrap(model.conversation.turns.first { $0.id == failed })
        XCTAssertEqual(planning.status, "failed")
        XCTAssertTrue(planning.offersReport)
        XCTAssertEqual(planning.reportID, "req-p")

        let model2 = makeModel(ReportRPC())
        let ordinary = begin("what time is it?", in: model2)
        model2.receiveChatEvent(["type": "reply", "request_id": "req-o", "text": "It's 11:40.", "at": at],
                                localID: ordinary)
        let answered = try XCTUnwrap(model2.conversation.turns.first)
        XCTAssertEqual(answered.status, "done")
        XCTAssertFalse(answered.offersReport)
    }

    /// Review 1, I2(c): an `interrupted` the notch inferred itself (the task is no longer running and FERB has no
    /// confirmed final response) is no record that the turn ended badly, so it offers no button; the daemon's own
    /// `interrupted` (a restart) still does, and a confirmed answer later clears the inference.
    func testAnInterruptionTheNotchInferredOffersNoButton() async throws {
        let rpc = ReportRPC()
        rpc.responses["chat.status"] = ["running": [[String: Any]](), "approvals": [[String: Any]](),
                                        "results": [[String: Any]]()]
        rpc.responses["trace.get"] = ["outcome": [String: Any]()]
        let model = makeModel(rpc)
        let id = begin("Rename the invoices", in: model)
        model.receiveChatEvent(["type": "ack", "request_id": "req-9", "text": "On it.", "at": at], localID: id)
        model.receiveChatEvent(["type": "plan", "request_id": "req-9", "task_id": "task-9", "text": "Renaming.",
                                "goal": "Rename", "at": at], localID: id)
        model.finishSubmission(localID: id)
        await model.refreshDaemonStatus()
        let inferred = try XCTUnwrap(model.conversation.turns.first { $0.id == id })
        XCTAssertEqual(inferred.status, "interrupted")
        XCTAssertEqual(inferred.statusInferred, true)
        XCTAssertFalse(inferred.offersReport)
        // The inference is saved with the turn, so a restart doesn't turn it into FERB's record.
        let saved = try JSONDecoder().decode(ConversationArchive.self,
                                             from: JSONEncoder().encode(model.conversation.forStorage()))
        XCTAssertFalse(try XCTUnwrap(saved.turns.first { $0.id == id }).offersReport)
        // FERB's own word on it replaces the inference.
        model.receiveOutcome(["task_id": "task-9", "request_id": "req-9", "status": "interrupted",
                              "answer": "This was interrupted when FERB restarted."], announce: false)
        let recorded = try XCTUnwrap(model.conversation.turns.first { $0.id == id })
        XCTAssertNil(recorded.statusInferred)
        XCTAssertTrue(recorded.offersReport)
    }

    func testTaskOutcomesOfferTheButtonOnlyWhenFailedOrInterrupted() {
        for (status, offered) in [("failed", true), ("interrupted", true), ("done", false), ("cancelled", false),
                                  ("done_unverified", false), ("needs_user", false)] {
            let model = makeModel(ReportRPC())
            model.receiveOutcome(["task_id": "task-" + status, "status": status, "answer": "Answer", "goal": "A task"],
                                 announce: false)
            XCTAssertEqual(model.conversation.turns.first?.offersReport, offered, status)
        }
    }

    func testTheReportIsPreparedThenSentOnlyWhenTheOwnerPressesSend() async throws {
        let rpc = ReportRPC()
        rpc.responses["reports.build"] = reply
        rpc.responses["reports.send"] = ["sent": true, "path": reply["path"]!, "report_id": "FERB-report-x"]
        let model = makeModel(rpc)
        var opened: [URL] = []
        model.openReportFile = { opened.append($0) }
        let turn = failedTurn(in: model)
        model.sendReport(for: turn)
        await settle()

        let call = try XCTUnwrap(rpc.calls.first { $0.0 == "reports.build" })
        XCTAssertEqual(call.1["task_id"] as? String, "task-1")
        XCTAssertEqual(call.1["request_id"] as? String, "req-1")
        XCTAssertEqual(rpc.waits["reports.build"], NotchModel.reportWait)
        let report = try XCTUnwrap(FailureReport(reply))
        XCTAssertEqual(model.reports[turn.id], .ready(report))
        XCTAssertFalse(rpc.calls.contains { $0.0 == "reports.send" })      // nothing leaves before Send
        model.openReportFile(report.file)
        XCTAssertEqual(opened, [URL(fileURLWithPath: "/tmp/FERB-report-2026-10-25-task-1.txt")])

        model.sendBuiltReport(for: turn.id)
        await settle()
        let send = try XCTUnwrap(rpc.calls.first { $0.0 == "reports.send" })
        XCTAssertEqual(send.1["path"] as? String, "/tmp/FERB-report-2026-10-25-task-1.txt")
        XCTAssertEqual(send.1["title"] as? String, "FERB report: I couldn't finish that: the folder was protected.")
        XCTAssertEqual(rpc.waits["reports.send"], ReportSender.sendWait)
        XCTAssertEqual(model.reports[turn.id], .sent)
        XCTAssertEqual(ReportState.sent.line, "Sent")
        // Sent: a second click on the old button does nothing.
        model.sendReport(for: turn)
        await settle()
        XCTAssertEqual(rpc.calls.filter { $0.0 == "reports.build" }.count, 1)
    }

    func testASendThatCouldNotGoKeepsTheReportAndOffersSendAgain() async throws {
        let rpc = ReportRPC()
        rpc.responses["reports.build"] = reply
        rpc.responses["reports.send"] = ["sent": false, "path": reply["path"]!,
                                         "error": "Sending reports to the developer isn't set up yet"]
        let model = makeModel(rpc)
        let turn = failedTurn(in: model)
        model.sendReport(for: turn)
        await settle()
        model.sendBuiltReport(for: turn.id)
        await settle()
        let report = try XCTUnwrap(FailureReport(reply))
        let unsent = ReportState.unsent(report, reason: "Sending reports to the developer isn't set up yet")
        XCTAssertEqual(model.reports[turn.id], unsent)
        XCTAssertTrue(unsent.isUnsent)
        XCTAssertEqual(unsent.line, "Couldn't send: Sending reports to the developer isn't set up yet "
                       + "The report is saved; you can send it again.")
        XCTAssertEqual(rpc.calls.filter { $0.0 == "reports.send" }.count, 1)     // no retry by itself
        // The connection failing is the same: kept, Send again.
        rpc.failures["reports.send"] = DaemonRPCError.remote("FERB isn't running")
        model.sendBuiltReport(for: turn.id)
        await settle()
        XCTAssertEqual(model.reports[turn.id], .unsent(report, reason: "FERB isn't running"))
        // Send again, now accepted.
        rpc.failures["reports.send"] = nil
        rpc.responses["reports.send"] = ["sent": true, "path": reply["path"]!, "report_id": "x"]
        model.sendBuiltReport(for: turn.id)
        await settle()
        XCTAssertEqual(model.reports[turn.id], .sent)
        XCTAssertEqual(rpc.calls.filter { $0.0 == "reports.send" }.count, 3)
    }

    func testFERBsRefusalShowsInlineAndSendsNothing() async {
        let rpc = ReportRPC()
        rpc.failures["reports.build"] = DaemonRPCError.remote("Only a failed or interrupted task can be sent")
        let model = makeModel(rpc)
        let turn = failedTurn(in: model)
        model.sendReport(for: turn)
        await settle()
        XCTAssertEqual(model.reports[turn.id], .failed("Only a failed or interrupted task can be sent"))
        model.sendBuiltReport(for: turn.id)                 // nothing prepared: nothing to send
        await settle()
        XCTAssertFalse(rpc.calls.contains { $0.0 == "reports.send" })
        // A second click asks again.
        rpc.failures["reports.build"] = nil
        rpc.responses["reports.build"] = reply
        model.sendReport(for: turn)
        await settle()
        XCTAssertEqual(model.reports[turn.id], .ready(FailureReport(reply)!))
    }

    func testADoneAnswerNeverAsksForAReport() async {
        let rpc = ReportRPC()
        let model = makeModel(rpc)
        model.receiveOutcome(["task_id": "task-2", "status": "done", "answer": "Done.", "goal": "A task"],
                             announce: false)
        model.sendReport(for: model.conversation.turns[0])
        await settle()
        XCTAssertFalse(rpc.calls.contains { $0.0 == "reports.build" })
    }

    // MARK: - Support

    private let at = "2026-10-25T11:40:00+05:30"

    private func makeModel(_ rpc: ReportRPC) -> NotchModel {
        NotchModel(rpc: rpc, preferences: scratchDefaults())
    }

    private func begin(_ request: String, in model: NotchModel) -> UUID {
        model.session.draft = request
        let id = model.session.beginWork()!
        model.conversation.begin(id: id, request: request, sources: [])
        return id
    }

    /// A turn as the notch holds it after its task failed: bound to its request at `ack` and to its task at `plan`.
    private func failedTurn(in model: NotchModel) -> ConversationTurn {
        let id = begin("Rename the invoices", in: model)
        model.receiveChatEvent(["type": "ack", "request_id": "req-1", "text": "On it.", "at": at], localID: id)
        model.receiveChatEvent(["type": "plan", "request_id": "req-1", "task_id": "task-1", "text": "Renaming.",
                                "goal": "Rename", "at": at], localID: id)
        model.receiveChatEvent(["type": "outcome", "request_id": "req-1", "task_id": "task-1", "status": "failed",
                                "answer": "I couldn't finish that: the folder was protected.", "at": at], localID: id)
        return model.conversation.turns.first { $0.id == id }!
    }

    private func settle() async {
        for _ in 0..<5 { await Task.yield() }
        try? await Task.sleep(for: .milliseconds(20))
    }
}

private final class ReportRPC: DaemonRPCTransport {
    private let recorded = RecordedCalls()
    var calls: [(String, [String: Any])] { recorded.all }
    var responses: [String: [String: Any]] = [:]
    var failures: [String: Error] = [:]
    var waits: [String: Int] = [:]
    func call(_ method: String, params: [String: Any]) async throws -> [String: Any] {
        recorded.append(method, params)
        if let failure = failures[method] { throw failure }
        return responses[method] ?? ["ok": true]
    }
    func call(_ method: String, params: [String: Any], wait seconds: Int) async throws -> [String: Any] {
        waits[method] = seconds
        return try await call(method, params: params)
    }
    func stream(_ method: String, params: [String: Any]) -> AsyncThrowingStream<[String: Any], Error> {
        recorded.append(method, params)
        return AsyncThrowingStream { $0.finish() }
    }
}
