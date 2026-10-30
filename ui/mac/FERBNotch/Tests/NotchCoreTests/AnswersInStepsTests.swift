import XCTest
@testable import FERBNotch
@testable import NotchCore

/// Answers in steps in the notch (orchestration decisions 19-21), driven by events shaped as the daemon sends them
/// (step 8d): the plan event with its card, `part` events on the stream and the bus, the outcome's footer, `merged`,
/// and `chat.status` after a reconnect.
@MainActor
final class AnswersInStepsTests: XCTestCase {
    // ---------- the first reply and the card ----------

    func testThePlanEventDrawsDirectAnswersReceiptsThePlanLineAndOneRowPerOtherAsk() throws {
        let model = makeModel()
        let id = begin("who are you, research 3 keyboards, remind me at 9", in: model)
        model.receiveChatEvent(["type": "ack", "request_id": "R", "text": ""], localID: id)
        XCTAssertTrue(turn(id, model).showsTyping)                        // the typing dots from the acknowledgement
        model.receiveChatEvent(["type": "plan_delta", "request_id": "R", "text": "I'll research "], localID: id)
        XCTAssertFalse(turn(id, model).showsTyping)                       // until FERB's first words
        XCTAssertEqual(turn(id, model).streamingPlan, "I'll research ")   // which stand where the plan line will be
        model.receiveChatEvent(plan(), localID: id)

        let shown = turn(id, model)
        XCTAssertNil(shown.streamingPlan)
        XCTAssertEqual(shown.direct, ["I'm FERB, your Mac assistant."])
        XCTAssertEqual(shown.receipts, ["🗓 Scheduled: call mom, tomorrow 9:00"])
        XCTAssertEqual(shown.planLine, "I'll research keyboards.")
        XCTAssertEqual(shown.planLines, ["I'll work in ~/Research."])
        XCTAssertEqual(shown.backendTaskID, "T")
        let card = try XCTUnwrap(shown.card)
        XCTAssertEqual(card.rows.map(\.part), ["who", "kb"])
        XCTAssertEqual(card.rows.filter { !$0.direct }.map(\.label), ["3 best keyboards"])
        XCTAssertEqual(card.row("kb")?.stepTaskID, "T.step.kb")
        XCTAssertEqual(card.done, 1)
        XCTAssertFalse(card.finished)
        // History keeps the card (the history view draws it from the saved archive).
        XCTAssertEqual(try savedArchive(model).turns.first?.card, card)
    }

    func testAPlanWithoutACardIsTheOneAskTurnAsBefore() {
        let model = makeModel()
        let id = begin("easy piano pieces", in: model)
        model.receiveChatEvent(["type": "plan", "request_id": "R", "task_id": "T", "text": "I'll list some pieces.",
                                "plan_line": "I'll list some pieces.", "lines": [String]()], localID: id)
        XCTAssertNil(turn(id, model).card)
        XCTAssertEqual(turn(id, model).plan, "I'll list some pieces.")
        XCTAssertEqual(turn(id, model).planLine, "I'll list some pieces.")
    }

    // ---------- rows ----------

    func testEachRowChangeAppliesOnceInSeqOrderFromTheStreamAndTheBus() throws {
        let model = makeModel()
        let id = begin("book a table and order lunch", in: model)
        model.receiveChatEvent(plan(), localID: id)
        let asks = part("kb", "asks", seq: "2026-10-31T10:00:01.000001+00:00",
                        extra: ["question": ["approval_id": "A1", "text": "Which one?", "options": ["K2", "RK84"]]])
        model.receiveChatEvent(asks, localID: id)
        model.receiveBusEvent(["topic": "task.part", "data": asks])           // the same change again, on the bus
        var row = try XCTUnwrap(turn(id, model).card?.row("kb"))
        XCTAssertEqual(row.state, "asks")
        XCTAssertEqual(row.question?.options, ["K2", "RK84"])

        model.receiveBusEvent(["topic": "task.part", "data": part("kb", "working", seq: "2026-10-31T10:00:02+00:00",
                                                                    extra: ["chosen": "K2"])])
        row = try XCTUnwrap(turn(id, model).card?.row("kb"))
        XCTAssertEqual(row.state, "working")
        XCTAssertEqual(row.chosen, "K2")
        XCTAssertNil(row.question)

        let done = part("kb", "done", seq: "2026-10-31T10:00:03+00:00",
                        extra: ["text": "Keychron K2, ₹8,999.", "note": "today's price", "verified": false])
        model.receiveChatEvent(done, localID: id)
        model.receiveChatEvent(asks, localID: id)                            // an older change arriving late: ignored
        row = try XCTUnwrap(turn(id, model).card?.row("kb"))
        XCTAssertEqual(row.state, "done")
        XCTAssertEqual(row.text, "Keychron K2, ₹8,999.")
        XCTAssertEqual(row.note, "today's price")
        XCTAssertNil(row.chosen)
        XCTAssertEqual(turn(id, model).card?.done, 2)
        XCTAssertEqual(try savedArchive(model).turns.first?.card?.row("kb")?.state, "done")
    }

    func testARowFillingQuietlyNeverOpensTheNotch() {
        let model = makeModel()
        let id = begin("two things", in: model)
        model.receiveChatEvent(plan(), localID: id)
        model.collapse()
        model.receiveBusEvent(["topic": "task.part", "data": part("kb", "done", seq: "2026-10-31T10:00:03+00:00",
                                                                    extra: ["text": "Done."])])
        XCTAssertEqual(turn(id, model).card?.row("kb")?.state, "done")
        XCTAssertFalse(model.expanded)
        // A row's question does (it arrives as approval.requested), and the whole task's end does.
        model.receiveBusEvent(["topic": "approval.requested", "data": ["approval_id": "A9", "task_id": "T.step.kb",
                                                                       "options": ["Yes", "No"], "reason": "Send it?"]])
        XCTAssertTrue(model.expanded)
        XCTAssertTrue(NotchModel.opensNotch("done_unverified"))
        XCTAssertFalse(NotchModel.opensNotch("cancelled"))
    }

    func testAContinuedRunUpdatesAPartWhereverItsCardShowsIt() throws {
        var chat = ConversationArchive()
        let first = UUID(), second = UUID()
        chat.begin(id: first, request: "flight time and airport metro", sources: [])
        chat.showPlan(id: first, text: "On it.", details: ["task_id": "T", "plan_line": "On it.", "card": [
            "request_id": "R0", "done": 2, "total": 2, "rows": [
                ["part": "flight", "label": "Flight", "state": "done", "text": "7:15 am", "seq": "2026-10-31T09:00:00+00:00"],
                ["part": "metro", "label": "Metro", "state": "done", "text": "Not yet", "seq": "2026-10-31T09:00:01+00:00"]]]])
        chat.begin(id: second, request: "check again", sources: [])
        chat.showPlan(id: second, text: "Checking.", details: ["task_id": "T", "plan_line": "Checking.", "card": [
            "request_id": "R1", "done": 2, "total": 3, "rows": [["part": "bus", "label": "Bus", "state": "working"]]]])

        let updated = chat.applyPart(["task_id": "T", "request_id": "R1", "card_request_id": "R0", "part": "flight",
                                      "label": "Flight", "state": "updated", "text": "7:55 am", "why": "changed after your follow-up",
                                      "seq": "2026-10-31T10:00:00+00:00"])
        XCTAssertEqual(updated, first)
        XCTAssertEqual(chat.turns[0].card?.row("flight")?.state, "updated")
        XCTAssertEqual(chat.turns[0].card?.row("flight")?.why, "changed after your follow-up")
        XCTAssertNil(chat.turns[1].card?.row("flight"))
        // A part no card shows yet joins this run's card.
        XCTAssertEqual(chat.applyPart(["task_id": "T", "request_id": "R1", "part": "new", "label": "New", "state": "working",
                                       "seq": "2026-10-31T10:00:01+00:00"]), second)
        XCTAssertEqual(chat.turns[1].card?.rows.map(\.part), ["bus", "new"])
        // Another task's part changes nothing.
        XCTAssertNil(chat.applyPart(["task_id": "U", "request_id": "R1", "part": "bus", "state": "done", "seq": "x"]))
    }

    // ---------- questions in their row ----------

    func testAStepsConfirmationAndFerbsOwnQuestionSitInTheirRowsNotAtTheEnd() async throws {
        let rpc = ScriptedRPC()
        let model = makeModel(rpc)
        let id = begin("two things", in: model)
        model.receiveChatEvent(plan(), localID: id)
        model.receiveBusEvent(["topic": "approval.requested", "data": [
            "approval_id": "G1", "task_id": "T.step.kb.verify", "capability": "ferb.confirm", "reason": "Pay ₹349?",
            "summary": "Pay ₹349 to Zomato?", "options": ["Pay ₹349", "Don't pay"]]])
        model.receiveBusEvent(["topic": "approval.requested", "data": [
            "approval_id": "F1", "task_id": "T", "part": "kb", "capability": "owner.ask", "reason": "Which one?",
            "options": ["K2", "RK84"]]])
        model.receiveBusEvent(["topic": "approval.requested", "data": [
            "approval_id": "O1", "task_id": "Other", "reason": "Unrelated?", "options": ["Yes"]]])

        XCTAssertEqual(model.pendingApprovals.first { $0.id == "G1" }?.taskID, "T")      // filed under its root task
        XCTAssertEqual(model.rowQuestions(turn(id, model))["kb"]?.map(\.id), ["G1", "F1"])
        XCTAssertEqual(model.inlineQuestions.map(\.id), ["O1"])

        // chat.status names only the root task: the row tie the live event gave is kept.
        rpc.replies["chat.status"] = ["running": [["task_id": "T", "goal": "two things"]], "results": [[String: Any]](),
                                      "approvals": [["approval_id": "G1", "task_id": "T", "reason": "Pay ₹349?",
                                                     "options": ["Pay ₹349", "Don't pay"], "capability": "ferb.confirm"],
                                                    ["approval_id": "O1", "reason": "Unrelated?", "options": ["Yes"]]]]
        await model.refreshDaemonStatus()
        XCTAssertEqual(model.rowQuestions(turn(id, model))["kb"]?.map(\.id), ["G1"])
        XCTAssertEqual(model.inlineQuestions.map(\.id), ["O1"])
    }

    // ---------- the end ----------

    func testTheOutcomeDrawsTheFooterInPlaceAndNeverAReplyBelow() throws {
        let model = makeModel()
        let id = begin("two things", in: model)
        model.receiveChatEvent(plan(), localID: id)
        _ = beginAnother("something else", in: model)                         // a later message in the chat
        model.receiveOutcome(["type": "outcome", "task_id": "T", "request_id": "R", "status": "done",
                              "answer": "Who I am: I'm FERB.\n\n3 best keyboards: K2\n\n2 of 2 done\nSites contacted: amazon.in",
                              "card": ["request_id": "R", "done": 2, "total": 2, "line": "2 of 2 done",
                                       "lines": ["Sites contacted: amazon.in"], "rows": [
                                        ["part": "kb", "label": "3 best keyboards", "state": "done", "text": "K2",
                                         "seq": "2026-10-31T10:00:05+00:00"]]]])
        let finished = try XCTUnwrap(model.conversation.turns.first { $0.id == id })
        XCTAssertEqual(finished.status, "done")
        XCTAssertEqual(finished.card?.line, "2 of 2 done")
        XCTAssertEqual(finished.card?.lines, ["Sites contacted: amazon.in"])
        XCTAssertEqual(finished.card?.row("kb")?.text, "K2")
        XCTAssertTrue(model.conversation.replies.isEmpty)                   // the card filled in place
    }

    func testAOneAskAnswerSplitsItsOwnLinesFromTheText() {
        let answer = "The flight leaves at 7:15.\n\nCouldn't confirm: the gate. Correction: it's T1.\n\nRemembered: gate\nSites contacted: goindigo.in\n(ran late)"
        let split = AnswerParts(answer: answer, note: "the gate", correction: "it's T1",
                                lines: ["Remembered: gate", "Sites contacted: goindigo.in"])
        XCTAssertEqual(split.body, "The flight leaves at 7:15.")
        XCTAssertEqual(split.note, "the gate")
        XCTAssertEqual(split.correction, "it's T1")
        XCTAssertEqual(split.lines, ["Remembered: gate", "Sites contacted: goindigo.in", "(ran late)"])
        // A piece the text doesn't end with stays in the text and isn't drawn twice.
        let other = AnswerParts(answer: "Done.", note: "something else", correction: nil, lines: ["Sites contacted: x"])
        XCTAssertEqual(other.body, "Done.")
        XCTAssertNil(other.note)
        XCTAssertTrue(other.lines.isEmpty)
    }

    func testAOneAskOutcomeKeepsItsStructuredFieldsAndAFailedOneOffersAnotherService() {
        let model = makeModel()
        let id = begin("flight", in: model)
        model.receiveChatEvent(["type": "plan", "request_id": "R", "task_id": "T", "text": "Checking."], localID: id)
        model.receiveChatEvent(["type": "outcome", "request_id": "R", "task_id": "T", "status": "failed",
                                "answer": "I couldn't finish that: usage limit.", "lines": ["Sites contacted: x.com"],
                                "runtime_switch": ["runtime": "codex", "problem": "usage_limit", "role": "worker",
                                                   "choices": ["claude"]]], localID: id)
        let shown = turn(id, model)
        XCTAssertEqual(shown.codeLines, ["Sites contacted: x.com"])
        XCTAssertEqual(shown.runtimeSwitch?.choices, ["claude"])
        XCTAssertEqual(RuntimeSwitch.label("claude"), "Claude")
    }

    // ---------- merged, reconnect ----------

    func testAMessageReadWithAnotherEndsQuietlyWithItsLabel() {
        let model = makeModel()
        let lead = begin("who am i", in: model)
        model.receiveChatEvent(["type": "ack", "request_id": "R1", "text": ""], localID: lead)
        let merged = beginAnother("and what time is it", in: model)
        model.receiveChatEvent(["type": "ack", "request_id": "R2", "text": ""], localID: merged)
        model.receiveChatEvent(["type": "merged", "request_id": "R2", "into": "R1"], localID: merged)
        model.finishSubmission(localID: merged)                              // its stream then ends
        let shown = turn(merged, model)
        XCTAssertEqual(shown.status, "merged")
        XCTAssertEqual(shown.mergedInto, "R1")
        XCTAssertFalse(shown.showsTyping)
        XCTAssertNil(shown.responseNotice)                                   // not "the response ended before…"
        XCTAssertNil(model.workSessions[merged])
        XCTAssertFalse(shown.offersReport)
    }

    func testAReconnectRebuildsLiveRowsFromChatStatusAndAdoptsATaskWithoutATurn() async {
        let rpc = ScriptedRPC()
        let model = makeModel(rpc)
        let id = begin("two things", in: model)
        model.receiveChatEvent(plan(), localID: id)
        let card: [String: Any] = ["request_id": "R", "done": 2, "total": 2, "rows": [
            ["part": "kb", "label": "3 best keyboards", "state": "done", "text": "K2", "seq": "2026-10-31T10:00:05+00:00"]]]
        let other: [String: Any] = ["request_id": "RX", "done": 0, "total": 2, "rows": [
            ["part": "a", "label": "A", "state": "working"], ["part": "b", "label": "B", "state": "working"]]]
        rpc.replies["chat.status"] = ["running": [["task_id": "T", "goal": "two things", "card": card],
                                                 ["task_id": "X", "goal": "Earlier task", "card": other]],
                                      "results": [[String: Any]](),
                                      "approvals": [[String: Any]]()]
        await model.refreshDaemonStatus()
        XCTAssertEqual(turn(id, model).card?.row("kb")?.state, "done")
        XCTAssertEqual(turn(id, model).card?.done, 2)
        let adopted = model.conversation.turns.first { $0.backendTaskID == "X" }
        XCTAssertEqual(adopted?.taskGoal, "Earlier task")
        XCTAssertEqual(adopted?.card?.rows.map(\.part), ["a", "b"])
        await model.refreshDaemonStatus()                                    // read again: nothing doubles
        XCTAssertEqual(model.conversation.turns.filter { $0.backendTaskID == "X" }.count, 1)
    }

    func testAnOlderArchiveWithoutCardsStillReads() throws {
        let json = #"{"turns":[{"id":"7C9E6679-7425-40DE-944B-E07FC1F90AE7","createdAt":0,"request":"hi","sources":[],"#
            + #""answer":"Hello","status":"done","artifacts":[],"plan":"Saying hi.","parts":["Done one."]}]}"#
        let archive = try JSONDecoder().decode(ConversationArchive.self, from: Data(json.utf8))
        XCTAssertNil(archive.turns[0].card)
        XCTAssertNil(archive.turns[0].planLine)
        XCTAssertEqual(archive.turns[0].ferbLines, ["Saying hi.", "Done one."])
    }

    // ---------- the card's own rules ----------

    func testASnapshotNeverReplacesANewerLiveRow() throws {
        var card = try XCTUnwrap(TurnCard(["request_id": "R", "done": 0, "total": 2, "rows": [
            ["part": "a", "label": "A", "state": "working"], ["part": "b", "label": "B", "state": "working"]]], taskID: "T"))
        card.apply(part: ["task_id": "T", "request_id": "R", "part": "a", "state": "done", "text": "yes",
                          "seq": "2026-10-31T10:00:02+00:00"])
        XCTAssertEqual(card.done, 1)
        let stale = try XCTUnwrap(TurnCard(["request_id": "R", "done": 0, "total": 2, "rows": [
            ["part": "a", "label": "A", "state": "asks", "seq": "2026-10-31T10:00:01+00:00"],
            ["part": "b", "label": "B (renamed)", "state": "working"]]], taskID: "T"))
        card.merge(stale)
        XCTAssertEqual(card.row("a")?.state, "done")
        XCTAssertEqual(card.row("b")?.label, "B (renamed)")
        XCTAssertTrue(CardRow.sortsAfter("2026-10-31T10:00:00.000002+00:00", "2026-10-31T10:00:00.000001+00:00"))
        XCTAssertFalse(CardRow.sortsAfter(nil, "2026-10-31T10:00:00+00:00"))
        XCTAssertTrue(CardRow.sortsAfter("2026-10-31T10:00:00+00:00", nil))
    }

    func testARowClaimsOnlyItsOwnQuestions() throws {
        let row = try XCTUnwrap(CardRow(["part": "visa", "label": "Visa", "state": "working",
                                         "step_task_id": "T.step.visa"], taskID: "T"))
        func approval(_ fields: [String: Any]) -> DaemonApproval {
            DaemonApproval(["approval_id": UUID().uuidString, "task_id": "T"].merging(fields) { $1 })!
        }
        XCTAssertTrue(row.asks(approval(["source_task_id": "T.step.visa"])))
        XCTAssertTrue(row.asks(approval(["source_task_id": "T.step.visa.verify"])))
        XCTAssertFalse(row.asks(approval(["source_task_id": "T.step.visa2"])))
        XCTAssertTrue(row.asks(approval(["part": "visa"])))
        XCTAssertFalse(row.asks(approval(["part": "visa", "task_id": "U"])))
        XCTAssertFalse(row.asks(approval([:])))
    }

    // ---------- helpers ----------

    private func plan() -> [String: Any] {
        ["type": "plan", "request_id": "R", "task_id": "T", "goal": "two things",
         "text": "I'm FERB, your Mac assistant.\n\n🗓 Scheduled: call mom, tomorrow 9:00\n\nI'll research keyboards.\nI'll work in ~/Research.",
         "plan_line": "I'll research keyboards.", "lines": ["I'll work in ~/Research."], "work_in": "I'll work in ~/Research.",
         "direct": [["part": "who", "label": "Who I am", "text": "I'm FERB, your Mac assistant."]],
         "receipts": ["🗓 Scheduled: call mom, tomorrow 9:00"],
         "card": ["request_id": "R", "done": 1, "total": 2, "rows": [
            ["part": "who", "label": "Who I am", "state": "done", "direct": true, "text": "I'm FERB, your Mac assistant.",
             "seq": "2026-10-31T10:00:00+00:00"],
            ["part": "kb", "label": "3 best keyboards", "state": "working", "direct": false, "step": "kb",
             "step_task_id": "T.step.kb"]]]]
    }

    private func part(_ id: String, _ state: String, seq: String, extra: [String: Any] = [:]) -> [String: Any] {
        ["type": "part", "task_id": "T", "request_id": "R", "card_request_id": "R", "part": id, "label": "3 best keyboards",
         "state": state, "step": id, "step_task_id": "T.step." + id, "seq": seq].merging(extra) { $1 }
    }

    private func makeModel(_ rpc: DaemonRPCTransport = ScriptedRPC()) -> NotchModel {
        NotchModel(rpc: rpc, preferences: UserDefaults(suiteName: "AnswersInSteps-" + UUID().uuidString)!)
    }

    private func begin(_ request: String, in model: NotchModel) -> UUID {
        model.session.draft = request
        let id = model.session.beginWork()!
        model.conversation.begin(id: id, request: request, sources: [])
        return id
    }

    private func beginAnother(_ request: String, in model: NotchModel) -> UUID {
        if let active = model.session.activeTaskID { model.workSessions[active] = model.session }
        model.session = NotchSession()
        return begin(request, in: model)
    }

    private func turn(_ id: UUID, _ model: NotchModel) -> ConversationTurn {
        model.conversation.turns.first { $0.id == id }!
    }

    private func savedArchive(_ model: NotchModel) throws -> ConversationArchive {
        try JSONDecoder().decode(ConversationArchive.self, from: XCTUnwrap(model.preferences.data(forKey: "FERBConversationArchive")))
    }
}

/// The live preview's answers (notes/preview.md decisions 1-4): the status line over the frozen frame, when to ask
/// again, and when not to ask at all.
final class PreviewStateTests: XCTestCase {
    func testTheStatusLineShowsWheneverFERBIsntWorkingInTheWindowOrTheFrameIsOld() {
        let live = PreviewAnswer(["state": "working", "line": "Working in Safari", "image": "AAAA", "fresh": true])
        XCTAssertNil(live.overlay)
        XCTAssertEqual(live.nextRequestDelay, .zero)                       // the capture's own rate: no cap
        let asking = PreviewAnswer(["state": "asking", "line": "Asking you: Send it?", "image": "AAAA", "fresh": false])
        XCTAssertEqual(asking.overlay, "Asking you: Send it?")
        XCTAssertEqual(asking.nextRequestDelay, .seconds(1))
        let busy = PreviewAnswer(["state": "working", "line": "Working in Safari", "image": "AAAA", "fresh": false, "busy": true])
        XCTAssertEqual(busy.overlay, "Working in Safari")
        XCTAssertEqual(busy.nextRequestDelay, .milliseconds(200))
        let failed = PreviewAnswer(["state": "working", "line": "Working in Safari", "image": "AAAA", "fresh": false,
                                    "problem": ["kind": "not_allowed", "text": "Screen Recording isn't allowed for FERB"]])
        XCTAssertEqual(failed.overlay, "Screen Recording isn't allowed for FERB")
        XCTAssertEqual(failed.problemKind, "not_allowed")
        let done = PreviewAnswer(["state": "done", "line": "Done"])
        XCTAssertFalse(done.dropsLastFrame)                                 // the app keeps its last frame
        let none = PreviewAnswer(["state": "none", "line": "No window to show yet"])
        XCTAssertTrue(none.dropsLastFrame)
        XCTAssertEqual(none.emptyText, "No window to show yet")
    }

    func testARefusalStopsAskingAndNobodyCapturesWhileNobodyCanSee() {
        XCTAssertEqual(PreviewRefusal(message: "tasks.preview is for FERB.app only"), .notThisApp)
        XCTAssertEqual(PreviewRefusal(message: "tasks.preview not allowed for cli"), .daemonTooOld)
        XCTAssertEqual(PreviewRefusal(message: "no method tasks.preview"), .daemonTooOld)
        XCTAssertNil(PreviewRefusal(message: "task T isn't running"))
        var gate = PreviewGate()
        XCTAssertTrue(gate.mayCapture)
        gate.screenLocked = true
        XCTAssertFalse(gate.mayCapture)
        gate.screenLocked = false
        gate.displaysAsleep = true
        XCTAssertFalse(gate.mayCapture)
    }

    func testTypingDotsBlinkLikeTheMock() {
        XCTAssertEqual(TypingDots.blink(0), 0.25, accuracy: 0.001)
        XCTAssertEqual(TypingDots.blink(0.48), 1, accuracy: 0.001)
        XCTAssertEqual(TypingDots.blink(1.0), 0.25, accuracy: 0.001)
    }
}
