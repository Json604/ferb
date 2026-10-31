import XCTest
@testable import FERBNotch
@testable import NotchCore

/// Send to developer (L35): a button under failed, interrupted and planning-failure answers. A click asks the daemon
/// for the report file (`reports.build`), then opens a new email in the owner's own Mail with the file attached; FERB
/// sends nothing. Without a Mail account the file is shown in Finder with the address to send it to.
@MainActor
final class FailureReportTests: XCTestCase {
    private let reply: [String: Any] = ["path": "/tmp/FERB-report-2026-10-25-task-1.txt",
                                        "subject": "FERB report: I couldn't finish that: the folder was protected.",
                                        "summary": "What happened: I couldn't finish that: the folder was protected.",
                                        "address": "reports@example.org"]

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

    func testComposeGetsTheAddressSubjectAndAttachment() async throws {
        let rpc = ReportRPC()
        rpc.responses["reports.build"] = reply
        let mailer = FakeMailer(composes: true)
        let model = makeModel(rpc)
        model.reportMailer = mailer
        let turn = failedTurn(in: model)
        model.sendReport(for: turn)
        await settle()

        let call = try XCTUnwrap(rpc.calls.first { $0.0 == "reports.build" })
        XCTAssertEqual(call.1["task_id"] as? String, "task-1")
        XCTAssertEqual(call.1["request_id"] as? String, "req-1")
        XCTAssertEqual(rpc.waits["reports.build"], NotchModel.reportWait)
        let mail = try XCTUnwrap(mailer.composed.first)
        XCTAssertEqual(mail.recipients, ["reports@example.org"])
        XCTAssertEqual(mail.subject, "FERB report: I couldn't finish that: the folder was protected.")
        XCTAssertEqual(mail.items.compactMap { $0 as? URL },
                       [URL(fileURLWithPath: "/tmp/FERB-report-2026-10-25-task-1.txt")])
        let body = try XCTUnwrap(mail.items.compactMap { $0 as? String }.first)
        XCTAssertEqual(body, "What happened: I couldn't finish that: the folder was protected.\n\n"
            + "The attached file is everything FERB recorded about this task, with secrets removed. "
            + "Open it to see exactly what you're sending.")
        XCTAssertTrue(mailer.revealed.isEmpty)
        XCTAssertEqual(model.reports[turn.id], .composed)
    }

    func testWithoutMailTheFileIsRevealedAndTheAddressShown() async {
        let rpc = ReportRPC()
        rpc.responses["reports.build"] = reply
        let mailer = FakeMailer(composes: false)
        let model = makeModel(rpc)
        model.reportMailer = mailer
        let turn = failedTurn(in: model)
        model.sendReport(for: turn)
        await settle()
        XCTAssertEqual(mailer.revealed, [URL(fileURLWithPath: "/tmp/FERB-report-2026-10-25-task-1.txt")])
        XCTAssertEqual(model.reports[turn.id], .revealed(address: "reports@example.org"))
        XCTAssertEqual(ReportState.revealed(address: "reports@example.org").line,
                       "Send this file to reports@example.org")
    }

    func testAComposeThatFailsAfterItStartedRevealsTheFile() async {
        let rpc = ReportRPC()
        rpc.responses["reports.build"] = reply
        let mailer = FakeMailer(composes: true)
        let model = makeModel(rpc)
        model.reportMailer = mailer
        let turn = failedTurn(in: model)
        model.sendReport(for: turn)
        await settle()
        XCTAssertEqual(model.reports[turn.id], .composed)
        mailer.composed.first?.failed()
        XCTAssertEqual(mailer.revealed, [URL(fileURLWithPath: "/tmp/FERB-report-2026-10-25-task-1.txt")])
        XCTAssertEqual(model.reports[turn.id], .revealed(address: "reports@example.org"))
    }

    func testFERBsRefusalShowsInlineAndOpensNothing() async {
        let rpc = ReportRPC()
        rpc.failures["reports.build"] = DaemonRPCError.remote("No feedback address set")
        let mailer = FakeMailer(composes: true)
        let model = makeModel(rpc)
        model.reportMailer = mailer
        let turn = failedTurn(in: model)
        model.sendReport(for: turn)
        await settle()
        XCTAssertEqual(model.reports[turn.id], .failed("No feedback address set"))
        XCTAssertTrue(mailer.composed.isEmpty && mailer.revealed.isEmpty)
        // A second click asks again.
        rpc.failures["reports.build"] = nil
        rpc.responses["reports.build"] = reply
        model.sendReport(for: turn)
        await settle()
        XCTAssertEqual(model.reports[turn.id], .composed)
    }

    func testADoneAnswerNeverAsksForAReport() async {
        let rpc = ReportRPC()
        let model = makeModel(rpc)
        model.reportMailer = FakeMailer(composes: true)
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
