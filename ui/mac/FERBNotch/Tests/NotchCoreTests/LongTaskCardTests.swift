import XCTest
@testable import FERBNotch
@testable import NotchCore

/// The long-task card (APP-718..720, daemon ORCH-531/532): each row's state from its step, collapsed text, and the
/// answer coming forward. Structure, not wording (AGENTS.md).
@MainActor
final class LongTaskCardTests: XCTestCase {
    private let t0 = Date(timeIntervalSince1970: 1_800_000_000)

    // ---------- the row's state ----------

    private func row(_ part: String, state: String = "working", step: String? = nil, label: String? = nil) -> CardRow {
        CardRow(["part": part, "label": label ?? part.capitalized, "state": state, "step": step ?? part,
                 "step_task_id": "T.step." + (step ?? part)], taskID: "T")!
    }

    private func step(_ status: String, waitsOn: [String] = [], attempt: Int = 1, loop: Bool = false,
                      seq: String = "2026-11-05T10:00:00.000001+00:00") -> StepState {
        StepState(["status": status, "waits_on": waitsOn, "attempt": attempt, "loop": loop, "seq": seq,
                   "started_at": "2026-11-05T10:00:00+00:00"])!
    }

    // decision: APP-718, APP-724
    func testEachStepStateShowsItsOwnRowState() {
        let mail = row("mail"), leads = row("leads"), rows = [mail, leads]
        func kind(_ status: String, waitsOn: [String] = [], attempt: Int = 1) -> RowDisplay {
            RowDisplay.make(row: leads, steps: ["leads": step(status, waitsOn: waitsOn, attempt: attempt)], rows: rows,
                            live: true)
        }
        let waiting = kind("waiting", waitsOn: ["mail"])
        XCTAssertEqual(waiting.kind, .waiting)
        XCTAssertFalse(waiting.spinner)
        XCTAssertTrue(waiting.words?.contains("Mail") == true)          // names the row it waits for
        XCTAssertFalse(kind("waiting", waitsOn: ["feeder"]).words?.contains("Mail") == true)   // a step with no row
        XCTAssertEqual(kind("starting").kind, .starting)
        XCTAssertTrue(kind("starting").spinner)
        let running = RowDisplay.make(row: leads, steps: ["leads": step("running")], rows: rows, live: true,
                                      liveLine: LiveProgress(text: "reading mail.example.com", startedAt: t0))
        XCTAssertEqual(running.kind, .running)
        XCTAssertTrue(running.spinner)
        XCTAssertNotNil(running.since)
        XCTAssertTrue(running.words?.contains("mail.example.com") == true)   // its own live line
        XCTAssertEqual(kind("checking").kind, .checking)
        XCTAssertNotNil(kind("checking").since)
        XCTAssertEqual(kind("retrying", attempt: 2).kind, .retrying)
        XCTAssertTrue(kind("retrying", attempt: 2).words?.contains("2") == true)
        // APP-724: a retrying row shows its attempt and its step's live line with the time, on one line
        let retrying = RowDisplay.make(row: leads, steps: ["leads": step("retrying", attempt: 2)], rows: rows, live: true,
                                       liveLine: LiveProgress(text: "reading mail.example.com", startedAt: t0))
        XCTAssertTrue(retrying.spinner)
        XCTAssertNotNil(retrying.since)
        XCTAssertTrue(retrying.words?.contains("2") == true && retrying.words?.contains("mail.example.com") == true)
        XCTAssertFalse(retrying.text(at: t0)?.contains("\n") ?? true)
        XCTAssertEqual(kind("needs_you").kind, .needsYou)
        XCTAssertEqual(kind("not_finished").kind, .notFinished)
        XCTAssertEqual(kind("wont_run").kind, .wontRun)
        // the row's own posted state wins over its step's
        let failed = row("leads", state: "failed")
        XCTAssertEqual(RowDisplay.make(row: failed, steps: ["leads": step("wont_run")], rows: rows, live: true).kind, .wontRun)
        XCTAssertEqual(RowDisplay.make(row: failed, steps: ["leads": step("failed")], rows: rows, live: true).kind, .failed)
        XCTAssertEqual(RowDisplay.make(row: row("leads", state: "done"), steps: [:], rows: rows, live: true).kind, .done)
        XCTAssertEqual(RowDisplay.make(row: leads, steps: ["leads": step("running")], rows: rows, live: true,
                                       hasQuestion: true).kind, .needsYou)
        // a loop's row shows its round
        let loop = RowDisplay.make(row: leads, steps: ["leads": step("starting", loop: true)], rows: rows, live: true,
                                   loop: LoopRound(rounds: 3, nextCheck: t0))
        XCTAssertEqual(loop.kind, .loop)
        XCTAssertTrue(loop.words?.contains("3") == true)
        // an older daemon sends no steps: the row is still under way
        XCTAssertTrue(RowDisplay.make(row: leads, steps: [:], rows: rows, live: true).spinner)
    }

    // decision: APP-718
    func testHistoryAndAStoppedTaskNeverShowLiveStates() {
        let leads = row("leads")
        for status in ["waiting", "starting", "running", "checking", "retrying"] {
            let shown = RowDisplay.make(row: leads, steps: ["leads": step(status)], rows: [leads], live: false)
            XCTAssertEqual(shown.kind, .notFinished, status)
            XCTAssertFalse(shown.spinner)
            XCTAssertNil(shown.since)
        }
    }

    // decision: APP-723
    func testTheStatusLineCountsRowsByState() {
        let card = TurnCard(["request_id": "R", "done": 1, "total": 4, "rows": []], taskID: "T")!
        let line = CardStatusLine.text(card: card, displays: [
            RowDisplay(kind: .done), RowDisplay(kind: .running, spinner: true), RowDisplay(kind: .checking),
            RowDisplay(kind: .waiting)], since: t0, at: t0.addingTimeInterval(723))
        let pieces = line.components(separatedBy: " · ")
        XCTAssertEqual(pieces.count, 4)                                     // done, running, waiting, the time
        XCTAssertTrue(pieces[0].contains("1") && pieces[0].contains("4"))
        XCTAssertTrue(pieces[1].hasPrefix("2"))
        XCTAssertTrue(pieces[2].hasPrefix("1"))
        XCTAssertEqual(pieces[3], "12:03")
        XCTAssertEqual(RowDisplay.elapsed(from: t0, to: t0.addingTimeInterval(161)), "2:41")
        XCTAssertEqual(RowDisplay.elapsed(from: t0, to: t0.addingTimeInterval(-5)), "0:00")
    }

    // decision: ORCH-531
    func testStepEventsApplyByOrderAndThePayloadNeverOverridesANewerState() {
        var card = TurnCard(["request_id": "R", "rows": [["part": "leads", "label": "Leads", "state": "working",
                                                           "step": "leads"]],
                             "steps": ["leads": ["status": "waiting", "waits_on": ["mail"]]]], taskID: "T")!
        XCTAssertEqual(card.steps["leads"]?.status, "waiting")
        XCTAssertTrue(card.apply(step: ["step": "leads", "status": "running", "seq": "2026-11-05T10:00:02+00:00"]))
        XCTAssertFalse(card.apply(step: ["step": "leads", "status": "starting", "seq": "2026-11-05T10:00:01+00:00"]))
        XCTAssertEqual(card.steps["leads"]?.status, "running")
        // a snapshot recorded before (no seq) never takes it back
        card.merge(TurnCard(["request_id": "R", "rows": [], "steps": ["leads": ["status": "waiting"]]], taskID: "T")!)
        XCTAssertEqual(card.steps["leads"]?.status, "running")
        // never saved: history never shows live states
        let saved = try! JSONDecoder().decode(TurnCard.self, from: JSONEncoder().encode(card))
        XCTAssertTrue(saved.steps.isEmpty)
    }

    // decision: ORCH-531, APP-718, APP-723
    func testAStepEventAndItsLiveLineReachTheirRowFromTheStreamAndTheBus() {
        let model = makeModel()
        let id = begin("mail and leads", in: model)
        model.receiveChatEvent(plan(), localID: id)
        model.receiveChatEvent(["type": "step", "request_id": "R", "task_id": "T", "step": "mail", "status": "running",
                                "step_task_id": "T.step.mail", "seq": "2026-11-05T10:00:01+00:00"], localID: id)
        XCTAssertEqual(turn(id, model).card?.steps["mail"]?.status, "running")
        model.receiveChatEvent(["type": "progress", "request_id": "R", "task_id": "T", "step_task_id": "T.step.mail",
                                "text": "reading mail.example.com", "started_at": "2026-11-05T10:00:02+00:00"], localID: id)
        XCTAssertEqual(model.stepLive["T.step.mail"]?.text, "reading mail.example.com")
        XCTAssertNil(turn(id, model).liveLine)                          // the work row's live line stays as decided
        model.receiveBusEvent(["topic": "task.step", "data": ["request_id": "R", "task_id": "T", "step": "mail",
                                                              "status": "checking", "step_task_id": "T.step.mail",
                                                              "seq": "2026-11-05T10:00:03+00:00"]])
        XCTAssertEqual(turn(id, model).card?.steps["mail"]?.status, "checking")
        XCTAssertNil(model.stepLive["T.step.mail"])
    }

    // ---------- collapsed text ----------

    // decision: APP-725, APP-721
    func testOnlyAnswersAndFailuresCollapseAndTheOpenedOnesSurviveUpdates() {
        XCTAssertTrue(RowDisplay(kind: .done).collapses)
        XCTAssertTrue(RowDisplay(kind: .failed).collapses)
        XCTAssertFalse(RowDisplay(kind: .needsYou).collapses)            // a row that needs the owner starts open
        XCTAssertFalse(RowDisplay(kind: .paused).collapses)
        XCTAssertNil(CollapsePolicy.clamp(.row, natural: 3 * 15, lineHeight: 15))
        XCTAssertEqual(CollapsePolicy.clamp(.row, natural: 12 * 15, lineHeight: 15), 3 * 15)
        let model = makeModel()
        let id = begin("mail and leads", in: model)
        model.receiveChatEvent(plan(), localID: id)
        let key = ExpansionState.key(turn: id, item: "mail")
        model.toggleExpanded(key, target: TranscriptTarget.row(turn: id, part: "mail"))
        model.receiveBusEvent(["topic": "task.part", "data": ["type": "part", "task_id": "T", "request_id": "R",
                                                              "part": "mail", "state": "done", "text": "Three mails.",
                                                              "seq": "2026-11-05T10:01:00+00:00"]])
        XCTAssertTrue(model.expandedText.isOpen(key))                   // the row's update keeps it open
        model.toggleExpanded(key, target: TranscriptTarget.row(turn: id, part: "mail"))
        XCTAssertFalse(model.expandedText.isOpen(key))
        XCTAssertEqual(model.scrollRequest?.reason, .collapse)          // collapsing keeps its top in view
    }

    // decision: APP-725
    func testOnlyTextsThatAreReallyLongCollapse() {
        let line: CGFloat = 15
        // a chat answer: only when taller than the chat area the owner set (300 pt), then about half of it shows
        XCTAssertNil(CollapsePolicy.clamp(.answer(viewport: 300), natural: 13 * line, lineHeight: line))   // fits
        XCTAssertNil(CollapsePolicy.clamp(.answer(viewport: 300), natural: 300, lineHeight: line))
        XCTAssertNil(CollapsePolicy.clamp(.answer(viewport: 300), natural: 7 * line, lineHeight: line))   // past the
        // old 6-line clamp: whole. Hiding under 3 lines shows it whole too (a small area, just over it)
        XCTAssertNil(CollapsePolicy.clamp(.answer(viewport: 60), natural: 65, lineHeight: line))
        XCTAssertEqual(CollapsePolicy.clamp(.answer(viewport: 60), natural: 75, lineHeight: line), 30)
        let tall = CollapsePolicy.clamp(.answer(viewport: 300), natural: 40 * line, lineHeight: line)
        XCTAssertEqual(tall, 150)                                        // half the area, whole lines
        // the threshold follows the owner's resize
        XCTAssertNil(CollapsePolicy.clamp(.answer(viewport: 600), natural: 30 * line, lineHeight: line))
        XCTAssertEqual(CollapsePolicy.clamp(.answer(viewport: 260), natural: 30 * line, lineHeight: line), 120)
        // a card row: only past ~8 lines, then 3 lines
        XCTAssertNil(CollapsePolicy.clamp(.row, natural: 8 * line, lineHeight: line))
        XCTAssertNil(CollapsePolicy.clamp(.row, natural: 8 * line + line / 3, lineHeight: line))
        XCTAssertEqual(CollapsePolicy.clamp(.row, natural: 9 * line, lineHeight: line), 3 * line)
    }

    // ---------- the answer comes forward ----------

    private func request(_ reason: TranscriptScrollRequest.Reason, revealing: Bool = false,
                         target: String = "item") -> TranscriptScrollRequest {
        TranscriptScrollRequest(target: target, reason: reason, label: "Leads", generation: 1, revealing: revealing)
    }

    // decision: APP-720, APP-722
    func testGrowthStaysPinnedAndTheOwnersScrollIsToldApart() {
        var follow = TranscriptFollow()
        follow.observe(offset: 700, contentHeight: 1000, viewport: 300, at: t0)
        XCTAssertTrue(follow.observe(offset: 700, contentHeight: 1100, viewport: 300, at: t0))   // growth: pinned
        XCTAssertTrue(follow.following)
        XCTAssertFalse(follow.ownerScrolledRecently(at: t0))
        follow.observe(offset: 300, contentHeight: 1100, viewport: 300, at: t0)                // the owner scrolled
        XCTAssertFalse(follow.following)
        XCTAssertTrue(follow.ownerScrolledRecently(at: t0.addingTimeInterval(9)))
        XCTAssertFalse(follow.ownerScrolledRecently(at: t0.addingTimeInterval(11)))
        XCTAssertFalse(follow.observe(offset: 300, contentHeight: 1200, viewport: 300, at: t0)) // not following: stays
        // FERB's own scroll never counts as the owner's
        var mine = TranscriptFollow()
        mine.observe(offset: 0, contentHeight: 1000, viewport: 300, at: t0)
        mine.scrolledByFERB(at: t0)
        mine.observe(offset: 400, contentHeight: 1000, viewport: 300, at: t0.addingTimeInterval(0.1))
        XCTAssertFalse(mine.ownerScrolledRecently(at: t0.addingTimeInterval(0.2)))
    }

    // decision: APP-720
    func testAnArrivalScrollsToItsTopUnlessTheOwnerJustScrolledThenAPill() {
        var follow = TranscriptFollow()
        let below: ClosedRange<CGFloat> = 500...800
        XCTAssertEqual(follow.decide(request(.needsYou), target: below, viewport: 300, reduceMotion: false, at: t0),
                       .scroll(target: "item", animated: true))
        XCTAssertEqual(follow.decide(request(.needsYou), target: below, viewport: 300, reduceMotion: true, at: t0),
                       .scroll(target: "item", animated: false))             // Reduce Motion: no animation
        XCTAssertEqual(follow.decide(request(.answer), target: 20...200, viewport: 300, reduceMotion: false, at: t0),
                       TranscriptDecision.none)                               // already in view
        follow.observe(offset: 0, contentHeight: 2000, viewport: 300, at: t0)
        follow.observe(offset: 120, contentHeight: 2000, viewport: 300, at: t0.addingTimeInterval(1))
        guard case let .pill(pill) = follow.decide(request(.taskDone), target: below, viewport: 300,
                                                   reduceMotion: false, at: t0.addingTimeInterval(3)) else {
            return XCTFail("the owner scrolled 2 s ago: a pill")
        }
        XCTAssertEqual(pill.target, "item")
        XCTAssertFalse(pill.up)
        // after 10 s it scrolls again
        XCTAssertEqual(follow.decide(request(.taskDone), target: below, viewport: 300, reduceMotion: false,
                                     at: t0.addingTimeInterval(12)), .scroll(target: "item", animated: true))
    }

    // decision: APP-720, APP-722
    func testAHiddenNotchLandsOnTheItemWithoutAnimating() {
        var follow = TranscriptFollow()
        follow.observe(offset: 0, contentHeight: 2000, viewport: 300, at: t0)
        follow.observe(offset: 120, contentHeight: 2000, viewport: 300, at: t0)        // even right after a scroll
        XCTAssertEqual(follow.decide(request(.needsYou, revealing: true), target: 900...1000, viewport: 300,
                                     reduceMotion: false, at: t0), .scroll(target: "item", animated: false))
    }

    // decision: APP-720, APP-722
    func testAQuietFillNeverMovesTheViewAndQuestionsCountInOnePill() {
        let follow = TranscriptFollow()
        XCTAssertEqual(follow.decide(request(.quietFill), target: 40...90, viewport: 300, reduceMotion: false, at: t0),
                       TranscriptDecision.none)
        guard case let .pill(above) = follow.decide(request(.quietFill), target: -400 ... -300, viewport: 300,
                                                    reduceMotion: false, at: t0) else { return XCTFail("a pill") }
        XCTAssertTrue(above.up)
        let first = TranscriptPill(reason: .needsYou, target: "q1", label: "", count: 1, up: true)
        let second = TranscriptPill(reason: .needsYou, target: "q2", label: "", count: 1, up: false)
        let both = TranscriptPill.merged(TranscriptPill.merged(nil, first), second)
        XCTAssertEqual(both.count, 2)
        XCTAssertEqual(both.target, "q1")                                     // the oldest
        XCTAssertEqual(TranscriptPill.merged(both, above).reason, .needsYou)  // a question outranks a fill
    }

    // decision: APP-720
    func testArrivalsNameTheirItem() {
        let model = makeModel()
        let id = begin("mail and leads", in: model)
        model.receiveChatEvent(plan(), localID: id)
        model.collapse()
        // a row's question targets its row; FERB opens the notch, so the request lands without animating
        model.receiveBusEvent(["topic": "approval.requested", "data": ["approval_id": "A1", "task_id": "T.step.leads",
                                                                       "options": ["Yes", "No"], "reason": "Use it?"]])
        XCTAssertEqual(model.scrollRequest?.target, TranscriptTarget.row(turn: id, part: "leads"))
        XCTAssertEqual(model.scrollRequest?.reason, .needsYou)
        XCTAssertEqual(model.scrollRequest?.revealing, true)
        XCTAssertTrue(model.expanded)
        // a quiet fill names its row
        model.receiveBusEvent(["topic": "task.part", "data": ["type": "part", "task_id": "T", "request_id": "R",
                                                              "part": "mail", "state": "done", "text": "Three mails.",
                                                              "seq": "2026-11-05T10:01:00+00:00"]])
        XCTAssertEqual(model.scrollRequest?.reason, .quietFill)
        XCTAssertEqual(model.scrollRequest?.target, TranscriptTarget.row(turn: id, part: "mail"))
        XCTAssertEqual(model.scrollRequest?.revealing, false)
        // the task's end targets the card's footer, above the bottom
        model.receiveChatEvent(["type": "outcome", "request_id": "R", "task_id": "T", "status": "done",
                                "answer": "2 of 2 done", "card": ["request_id": "R", "done": 2, "total": 2,
                                                                 "line": "2 of 2 done", "rows": []]], localID: id)
        XCTAssertEqual(model.scrollRequest?.target, TranscriptTarget.footer(turn: id))
        XCTAssertEqual(model.scrollRequest?.reason, .taskDone)
    }

    // decision: APP-571, APP-722
    func testRestoringResultsAtStartupAsksForNoScroll() {
        let model = makeModel()
        let id = begin("mail and leads", in: model)
        model.receiveChatEvent(plan(), localID: id)
        let before = model.scrollRequest
        model.receiveOutcome(["task_id": "T", "request_id": "R", "status": "done", "answer": "2 of 2 done"],
                             announce: false, recordEvent: false)
        XCTAssertEqual(model.scrollRequest, before)
    }

    // ---------- helpers ----------

    private func plan() -> [String: Any] {
        ["type": "plan", "request_id": "R", "task_id": "T", "goal": "mail and leads", "text": "I'll read your mail.",
         "plan_line": "I'll read your mail.", "lines": [],
         "card": ["request_id": "R", "done": 0, "total": 2, "rows": [
            ["part": "mail", "label": "Mail", "state": "working", "step": "mail", "step_task_id": "T.step.mail"],
            ["part": "leads", "label": "Leads", "state": "working", "step": "leads", "step_task_id": "T.step.leads"]],
                  "steps": ["leads": ["status": "waiting", "waits_on": ["mail"]]]]]
    }

    private func makeModel() -> NotchModel {
        NotchModel(rpc: ScriptedRPC(), preferences: scratchDefaults("LongTaskCard"))
    }

    private func begin(_ request: String, in model: NotchModel) -> UUID {
        model.session.draft = request
        let id = model.session.beginWork()!
        model.conversation.begin(id: id, request: request, sources: [])
        return id
    }

    private func turn(_ id: UUID, _ model: NotchModel) -> ConversationTurn {
        model.conversation.turns.first { $0.id == id }!
    }
}
