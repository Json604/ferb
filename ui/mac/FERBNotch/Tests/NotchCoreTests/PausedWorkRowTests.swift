import XCTest
@testable import FERBNotch
@testable import NotchCore

/// The owner paused his only task and the row under the chat still turned its spinner and counted "1 active task ·
/// 10:22" (owner, 2026-11-12: "this paused task shoudl also relfect in these line, loader adn time").
@MainActor
final class PausedWorkRowTests: XCTestCase {
    // decision: APP-901
    func testTheRowsWordsCountPausedTasksApartFromTheTimer() {
        func words(_ active: Int, _ paused: Int, _ kept: Int) -> [String] {
            let made = WorkRowWords(active: active, paused: paused, kept: kept)
            return [made.lead, made.trail]
        }
        XCTAssertEqual(words(1, 0, 0), ["1 active task", ""])
        XCTAssertEqual(words(0, 1, 0), ["1 paused task", ""])
        XCTAssertEqual(words(0, 2, 0), ["2 paused tasks", ""])
        XCTAssertEqual(words(1, 1, 0), ["1 active task", "1 paused"])
        XCTAssertEqual(words(2, 1, 1), ["2 active tasks", "1 paused · 1 kept going"])
        XCTAssertEqual(words(0, 1, 1), ["1 paused task · 1 kept going", ""])
        XCTAssertEqual(WorkRowWords(active: 1, paused: 1, kept: 0).text, "1 active task · 1 paused")
    }

    // decision: APP-901
    func testAPausedTaskHasNoSpinnerAndNoTimeUntilItCarriesOn() {
        let model = NotchModel(rpc: PausedTasksRPC(paused: []), preferences: scratchDefaults())
        model.receiveBusEvent(["topic": "task.created", "data": ["task_id": "read", "goal": "Read the page"]])
        model.receiveBusEvent(["topic": "task.created", "data": ["task_id": "sort", "goal": "Sort the photos"]])
        model.runningSince = ["read": Date(timeIntervalSinceNow: -600), "sort": Date(timeIntervalSinceNow: -60)]
        XCTAssertEqual(model.workRowText, "2 active tasks")
        XCTAssertEqual(model.workSince, model.runningSince["read"])

        // Mixed: the time is the running task's only.
        model.receiveBusEvent(["topic": "task.paused", "data": ["task_id": "read"]])
        XCTAssertEqual(model.taskItems.first { $0.id == "read" }?.status, "paused")
        XCTAssertEqual(model.activeTaskCount, 1)
        XCTAssertEqual(model.pausedTaskCount, 1)
        XCTAssertTrue(model.hasRunningWork)
        XCTAssertEqual(model.workSince, model.runningSince["sort"])
        XCTAssertEqual(model.workRowWords, WorkRowWords(active: 1, paused: 1, kept: 0))

        // All paused: the row stays, with no spinner and no time; the picture stays too.
        model.receiveBusEvent(["topic": "task.paused", "data": ["task_id": "sort"]])
        XCTAssertFalse(model.hasRunningWork)
        XCTAssertTrue(model.showsWorkRow)
        XCTAssertNil(model.workSince)
        XCTAssertEqual(model.workRowText, "2 paused tasks")
        XCTAssertTrue(model.taskItems.allSatisfy { $0.isUnderway && $0.isOngoing })

        // A question of its own still reads as waiting for you.
        model.receiveBusEvent(["topic": "approval.requested", "data": ["approval_id": "q", "task_id": "read",
                                                                       "reason": "Resume?"]])
        XCTAssertEqual(model.taskItems.first { $0.id == "read" }?.status, "needs_user")
        model.receiveBusEvent(["topic": "approval.resolved", "data": ["approval_id": "q"]])

        // Carrying on counts it again; an ended task is no longer paused.
        model.receiveBusEvent(["topic": "task.resumed", "data": ["task_id": "sort"]])
        XCTAssertEqual(model.workRowText, "1 active task · 1 paused")
        XCTAssertEqual(model.workSince, model.runningSince["sort"])
        model.receiveBusEvent(["topic": "task.result", "data": ["task_id": "read", "status": "cancelled",
                                                                "answer": "Stopped, as you asked."]])
        XCTAssertFalse(model.pausedTasks.contains("read"))
        XCTAssertEqual(model.workRowText, "1 active task")
        XCTAssertEqual(model.makeTaskList().rows, model.taskItems)
    }

    // decision: APP-901
    func testAReconnectReadsWhichTasksArePausedFromTheirRecordedStatus() async {
        let model = NotchModel(rpc: PausedTasksRPC(paused: ["read"]), preferences: scratchDefaults())
        model.runningTasks = ["read": "Read the page", "sort": "Sort the photos"]
        await model.refreshPausedTasks()
        XCTAssertEqual(model.pausedTasks, ["read"])
        XCTAssertEqual(model.workRowText, "1 active task · 1 paused")
    }
}

/// tasks.list as the daemon answers it: each root task with its recorded status.
private final class PausedTasksRPC: DaemonRPCTransport {
    let paused: Set<String>
    init(paused: Set<String>) { self.paused = paused }
    func call(_ method: String, params: [String: Any]) async throws -> [String: Any] {
        guard method == "tasks.list" else { return [:] }
        return ["tasks": ["read", "sort", "older"].map { id in
            ["task_id": id, "status": paused.contains(id) ? "paused" : id == "older" ? "done" : "running"]
        }, "more": false]
    }
    func stream(_ method: String, params: [String: Any]) -> AsyncThrowingStream<[String: Any], Error> {
        AsyncThrowingStream { _ in }
    }
}
