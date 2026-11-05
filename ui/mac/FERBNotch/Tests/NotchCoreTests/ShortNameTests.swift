import XCTest
@testable import FERBNotch
import NotchCore

/// T-1055 (owner D32): every heading shows a short name; the full instruction only where the owner opens the item.
@MainActor
final class ShortNameTests: XCTestCase {
    private let instruction = "Check Coursera in my Chrome “scaler” profile for new BITS assignments. Keep a note of "
        + "each one you find, with its due date, and tell me only about new ones since the last run."

    // decision: APP-727
    func testRowsCarryTheNameAndKeepTheFullInstructionForTheOpenedDetail() {
        let schedule = ScheduleRow(["id": "sc-1", "request": instruction, "name": "Coursera BITS assignments"])!
        XCTAssertEqual(schedule.name, "Coursera BITS assignments")
        XCTAssertEqual(schedule.request, instruction)
        let task = TaskHistoryRow(["task_id": "t-1", "goal": instruction, "name": "Coursera BITS assignments"])!
        XCTAssertEqual(task.name, "Coursera BITS assignments")
        XCTAssertEqual(task.goal, instruction)
        let timer = ReminderRow(["id": "tm-1", "kind": "timer", "message": "Your 1-minute timer is up.",
                                 "name": "1-minute timer"])!
        XCTAssertTrue(timer.isTimer)
        XCTAssertEqual(timer.name, "1-minute timer")
        XCTAssertTrue(timer.hasDetail)
        // A reminder the owner phrased short keeps their words: nothing more to open.
        let short = ReminderRow(["id": "tm-2", "kind": "reminder", "message": "Call Mom", "name": "Call Mom"])!
        XCTAssertFalse(short.hasDetail)
    }

    // decision: APP-727
    func testARowWithoutANameIsHeadedByItsFirstClauseCut() {
        // A row made before names (or from an older daemon): structural cut, never the whole instruction.
        let schedule = ScheduleRow(["id": "sc-old", "request": instruction])!
        XCTAssertLessThanOrEqual(schedule.name.count, ShortName.cut)
        XCTAssertTrue(schedule.name.hasSuffix("…"))
        XCTAssertTrue(instruction.hasPrefix(String(schedule.name.dropLast())))
        XCTAssertEqual(ShortName.clause("Your 1-minute timer is up."), "Your 1-minute timer is up")
        XCTAssertEqual(ShortName.clause("Water the plants, then text Sam"), "Water the plants")
    }

    // decision: APP-727
    func testATaskRowInTheNotchIsHeadedByThePlansName() {
        let model = NotchModel(rpc: ScriptedRPC(), preferences: scratchDefaults())
        let local = UUID()
        model.conversation.begin(id: local, request: instruction, sources: [])
        model.conversation.bind(id: local, taskID: "t-9")
        model.conversation.showPlan(id: local, text: "On it.",
                                    details: ["task_id": "t-9", "goal": instruction, "name": "Coursera BITS assignments"])
        XCTAssertEqual(model.taskItems.first { $0.backendID == "t-9" }?.title, "Coursera BITS assignments")
        model.runningTasks = ["t-10": NotchModel.heading(["goal": instruction])]
        XCTAssertLessThanOrEqual(model.runningTasks["t-10"]?.count ?? 99, ShortName.cut)
    }
}
