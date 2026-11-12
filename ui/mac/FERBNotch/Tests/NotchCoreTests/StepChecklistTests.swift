import XCTest
@testable import NotchCore

/// APP-894 and APP-895, rebuilt on the live run of 2026-11-11 (task f67b5efe): the 14:14 single ask of four steps (its
/// stored plan has no labels, so each line is its goal's first clause, cut by the daemon) and the 14:41 card whose row
/// waited on steps that answer no row. The states and live lines are the ones the records give at each moment.
final class StepChecklistTests: XCTestCase {
    private let t0 = Date(timeIntervalSince1970: 1_800_000_000)

    private func steps() -> [[String: Any]] {
        [["id": "gather", "label": "Read the GitHub FERB repository and…", "kind": "do", "depends_on": [String](),
          "step_task_id": "T.step.gather"],
         ["id": "research", "label": "Thoroughly research effective startup…", "kind": "do", "depends_on": [String](),
          "step_task_id": "T.step.research"],
         ["id": "deck", "label": "Act as the separate deck-creation agent", "kind": "do",
          "depends_on": ["gather", "research"], "step_task_id": "T.step.deck"],
         ["id": "fill", "label": "Using the gathered FERB context", "kind": "do", "depends_on": ["gather", "deck"],
          "step_task_id": "T.step.fill"]]
    }

    private func state(_ status: String, seq: Int, waitsOn: [String] = []) -> [String: Any] {
        ["status": status, "waits_on": waitsOn, "attempt": 1, "seq": String(format: "2026-11-11T08:50:%02d.000001+00:00", seq)]
    }

    private func lines(_ checklist: StepChecklist, live: Bool, _ liveLines: [String: String] = [:]) -> [String] {
        checklist.steps.map { step in
            let line = ChecklistLine.make(step: step, state: checklist.states[step.id], ended: checklist.ended[step.id],
                                          live: live, liveLine: liveLines[step.id].map { LiveProgress(text: $0, startedAt: t0) })
            return "\(line.kind.rawValue) \(step.label)" + (line.words.map { " · " + $0 } ?? "")
        }
    }

    // decision: APP-894, ORCH-626
    func testTheLiveFourStepAskShowsItsStepsAtEachMoment() {
        // 14:19:19, the plan: both independent steps start, the others wait on them
        var checklist = StepChecklist(["steps": steps(), "states": [
            "gather": state("starting", seq: 1), "research": state("starting", seq: 2),
            "deck": state("waiting", seq: 3, waitsOn: ["gather", "research"]),
            "fill": state("waiting", seq: 4, waitsOn: ["gather", "deck"])]], taskID: "T")!
        XCTAssertEqual(lines(checklist, live: true), [
            "running Read the GitHub FERB repository and… · Starting…",
            "running Thoroughly research effective startup… · Starting…",
            "upcoming Act as the separate deck-creation agent",
            "upcoming Using the gathered FERB context"])
        XCTAssertEqual(ChecklistSummary.count(done: checklist.doneCount(live: true), total: 4), "0 of 4")
        // 14:21:15, while research runs: each running step its own live line (the records' tool calls)
        checklist.apply(step: ["step": "gather"].merging(state("running", seq: 5)) { $1 })
        checklist.apply(step: ["step": "research"].merging(state("running", seq: 6)) { $1 })
        XCTAssertEqual(lines(checklist, live: true, ["gather": "running a command",
                                                     "research": "reading news.ycombinator.com"]), [
            "running Read the GitHub FERB repository and… · Running a command",
            "running Thoroughly research effective startup… · Reading news.ycombinator.com",
            "upcoming Act as the separate deck-creation agent",
            "upcoming Using the gathered FERB context"])
        // 14:27:30: research (14:22:04) and gather (14:23:52) done, the deck builds
        for (key, seq) in [("research", 7), ("gather", 8)] {
            checklist.apply(step: ["step": key].merging(state("done", seq: seq)) { $1 })
        }
        checklist.apply(step: ["step": "deck"].merging(state("running", seq: 9)) { $1 })
        XCTAssertEqual(lines(checklist, live: true, ["deck": "moving ferb-pitch-deck.pdf to the Trash"]), [
            "done Read the GitHub FERB repository and…",
            "done Thoroughly research effective startup…",
            "running Act as the separate deck-creation agent · Moving ferb-pitch-deck.pdf to the Trash",
            "upcoming Using the gathered FERB context"])
        XCTAssertEqual(ChecklistSummary.count(done: checklist.doneCount(live: true), total: 4), "2 of 4")
        // 14:34:05, the outcome: every step done; one line, and it reads the same after a restore
        checklist.end(StepChecklist(["steps": steps(), "states": [
            "gather": state("done", seq: 8), "research": state("done", seq: 7), "deck": state("done", seq: 10),
            "fill": state("done", seq: 11)]], taskID: "T"))
        XCTAssertEqual(ChecklistSummary.collapsed(done: checklist.doneCount(live: false), total: 4), "4 steps done")
        let restored = try! JSONDecoder().decode(StepChecklist.self, from: JSONEncoder().encode(checklist))
        XCTAssertTrue(restored.finished)
        XCTAssertEqual(lines(restored, live: false).first, "done Read the GitHub FERB repository and…")
        XCTAssertEqual(ChecklistSummary.collapsed(done: restored.doneCount(live: false), total: 4), "4 steps done")
    }

    // decision: APP-894, ORCH-532, ORCH-613
    func testAnUnfinishedAskSaysHowEachStepEnded() {
        var checklist = StepChecklist(["steps": steps()], taskID: "T")!
        checklist.end(StepChecklist(["steps": steps(), "states": [
            "gather": state("done", seq: 1), "research": state("failed", seq: 2), "deck": state("wont_run", seq: 3),
            "fill": state("stopped", seq: 4)]], taskID: "T"))
        XCTAssertEqual(lines(checklist, live: false), [
            "done Read the GitHub FERB repository and…",
            "failed Thoroughly research effective startup… · Couldn't do",
            "wontRun Act as the separate deck-creation agent · Won't run: an earlier step couldn't be done",
            "stopped Using the gathered FERB context · Stopped by you"])
        XCTAssertEqual(ChecklistSummary.collapsed(done: checklist.doneCount(live: false), total: 4), "1 of 4 steps done")
        let read = checklist.steps.map { step in
            ChecklistLine.make(step: step, state: nil, ended: checklist.ended[step.id], live: false).stateName
        }
        XCTAssertEqual(read, ["done", "couldn't do", "won't run", "stopped by you"])
        // a step asking the owner, and a wait step's own time
        let asking = ChecklistLine.make(step: checklist.steps[0], state: StepState(state("running", seq: 5)), ended: nil,
                                        live: true, asking: true)
        XCTAssertEqual(asking.words, "Asking you")
        XCTAssertEqual(ChecklistLine.make(step: checklist.steps[2], state: StepState(state("waiting", seq: 6)),
                                          ended: nil, live: true).stateName, "not started")
    }

    // decision: APP-895
    func testARowWaitingOnStepsNoRowAnswersShowsThem() {
        // 14:45 on the 14:41 card: "Improved pitch deck" (answer step attach) waits on redesign, which waits on research
        // and guidance; no row answers those three
        let plan: [[String: Any]] = [
            ["id": "research", "label": "Research excellent startup pitch-deck…", "depends_on": [String](),
             "step_task_id": "T.step.research"],
            ["id": "guidance", "label": "Review the available animations.dev UI…", "depends_on": [String](),
             "step_task_id": "T.step.guidance"],
            ["id": "redesign", "label": "As the other agent responsible for…", "depends_on": ["research", "guidance"],
             "step_task_id": "T.step.redesign"],
            ["id": "attach", "label": "Verify the improved HTML and PDF", "depends_on": ["redesign"],
             "step_task_id": "T.step.attach"]]
        let card = TurnCard(["request_id": "R", "plan": plan, "rows": [
            ["part": "preferences", "label": "Writing and design rules", "state": "done", "direct": true],
            ["part": "deck", "label": "Improved pitch deck", "state": "working", "step": "attach",
             "step_task_id": "T.step.attach"]],
            "steps": ["attach": state("waiting", seq: 1, waitsOn: ["redesign"]), "research": state("running", seq: 2),
                      "guidance": state("running", seq: 3),
                      "redesign": state("waiting", seq: 4, waitsOn: ["research", "guidance"])]], taskID: "T")!
        let row = card.row("deck")!
        XCTAssertEqual(card.feeders(of: row).map(\.id), ["research", "guidance", "redesign"])
        let display = RowDisplay.make(row: row, steps: card.steps, rows: card.rows, live: true, plan: card.plan)
        XCTAssertEqual(display.words, "Waiting for As the other agent responsible for…")
        let shown = card.feeders(of: row).map { step -> String in
            let live = ["research": "reading sozai.app", "guidance": "reading developer.apple.com"][step.id]
            let line = ChecklistLine.make(step: step, state: card.steps[step.id], ended: nil, live: true,
                                          liveLine: live.map { LiveProgress(text: $0, startedAt: t0) })
            return "\(line.kind.rawValue) \(step.label)" + (line.words.map { " · " + $0 } ?? "")
        }
        XCTAssertEqual(shown, ["running Research excellent startup pitch-deck… · Reading sozai.app",
                               "running Review the available animations.dev UI… · Reading developer.apple.com",
                               "upcoming As the other agent responsible for…"])
        let saved = try! JSONDecoder().decode(TurnCard.self, from: JSONEncoder().encode(card))
        XCTAssertEqual(saved.plan.map(\.id), ["research", "guidance", "redesign", "attach"])
    }
}
