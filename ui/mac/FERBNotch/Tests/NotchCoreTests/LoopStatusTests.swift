import XCTest
@testable import FERBNotch
@testable import NotchCore

/// L31: a loop is standing work. Its row says what FERB keeps, how often it fixed it and when it last did; the
/// "active tasks" spinner turns only while a round is acting.
@MainActor
final class LoopStatusTests: XCTestCase {
    func testALoopShowsAsStandingWorkWithItsCounts() {
        let model = NotchModel(rpc: QuietRPC(), preferences: UserDefaults(suiteName: UUID().uuidString)!)
        model.receiveBusEvent(["topic": "task.created", "data": ["task_id": "t1", "goal": "Keep the tab playing"]])
        let end = Date().addingTimeInterval(-120).timeIntervalSince1970
        model.receiveBusEvent(["topic": "loop.status", "data": [
            "task_id": "t1", "step": "keep", "goal": "Keep the tab playing", "state": "watching", "rounds": 4,
            "last_round_end": end, "gap_seconds": 0.0] as [String: Any]])
        XCTAssertEqual(model.standingLine("t1", now: Date()), "Keeping: Keep the tab playing · fixed 4 times · last 2 min ago")
        XCTAssertEqual(model.keptGoingCount, 1)
        XCTAssertEqual(model.activeTaskCount, 0)          // watching: no spinner
        model.receiveBusEvent(["topic": "loop.status", "data": [
            "task_id": "t1", "step": "keep", "goal": "Keep the tab playing", "state": "acting", "rounds": 4,
            "last_round_end": end, "gap_seconds": 0.0] as [String: Any]])
        XCTAssertEqual(model.activeTaskCount, 1)          // a round is acting
        model.receiveBusEvent(["topic": "loop.status", "data": [
            "task_id": "t1", "step": "keep", "goal": "Keep the tab playing", "state": "stopped", "rounds": 4,
            "last_round_end": end, "gap_seconds": 0.0] as [String: Any]])
        XCTAssertNil(model.standingWork["t1"])
    }

    func testRoundEndArrivesAsAWholeNumberOrNullAndTheCopyFollowsTheState() {
        let model = makeModel()
        model.receiveBusEvent(["topic": "task.created", "data": ["task_id": "t1", "goal": "Keep the tab playing"]])
        let now = Date()
        // JSON gives a whole number for a timestamp with no fraction; a Swift Int stands for it here.
        let wholeEnd = Int(now.timeIntervalSince1970) - 600
        model.receiveBusEvent(loopStatus("t1", state: "backing_off", rounds: 1, lastRoundEnd: wholeEnd, gap: 300))
        XCTAssertEqual(model.standingLine("t1", now: now),
                       "Keeping: Keep the tab playing · fixed 1 time · last 10 min ago · waiting longer between fixes")
        XCTAssertEqual(model.standingWork["t1"]?.gapSeconds, 300)
        model.receiveBusEvent(loopStatus("t1", state: "paused", rounds: 2, lastRoundEnd: Double(wholeEnd), gap: 0))
        XCTAssertEqual(model.standingLine("t1", now: now),
                       "Keeping: Keep the tab playing · fixed 2 times · last 10 min ago · paused: needs you")
        model.receiveBusEvent(loopStatus("t1", state: "watching", rounds: 0, lastRoundEnd: NSNull(), gap: 0))
        XCTAssertEqual(model.standingLine("t1", now: now), "Keeping: Keep the tab playing")
        model.receiveBusEvent(loopStatus("t1", state: "watching", rounds: 3, lastRoundEnd: now.timeIntervalSince1970 - 20, gap: 0))
        XCTAssertEqual(model.standingLine("t1", now: now), "Keeping: Keep the tab playing · fixed 3 times · last just now")
        model.receiveBusEvent(loopStatus("t1", state: "watching", rounds: 3, lastRoundEnd: now.timeIntervalSince1970 - 7300, gap: 0))
        XCTAssertEqual(model.standingLine("t1", now: now), "Keeping: Keep the tab playing · fixed 3 times · last 2 hr ago")
    }

    func testDecodedDaemonJSONIsRead() throws {
        let model = makeModel()
        model.receiveBusEvent(["topic": "task.created", "data": ["task_id": "t1", "goal": "Keep the tab playing"]])
        let end = Int(Date().timeIntervalSince1970) - 180
        let json = #"{"topic": "loop.status", "data": {"task_id": "t1", "step": "keep", "goal": "Keep the tab playing", "state": "watching", "rounds": 5, "last_round_end": \#(end), "gap_seconds": 0}}"#
        let event = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any])
        model.receiveBusEvent(event)
        XCTAssertEqual(model.standingLine("t1", now: Date()), "Keeping: Keep the tab playing · fixed 5 times · last 3 min ago")
    }

    func testTheTaskRowCarriesTheStandingLineAndStaysStoppable() async {
        let rpc = LoopRPC()
        let model = makeModel(rpc)
        model.receiveBusEvent(["topic": "task.created", "data": ["task_id": "t1", "goal": "Keep the tab playing"]])
        model.receiveBusEvent(loopStatus("t1", state: "watching", rounds: 4, lastRoundEnd: Date().timeIntervalSince1970 - 120, gap: 0))
        let row = try? XCTUnwrap(model.taskItems.first { $0.backendID == "t1" })
        XCTAssertEqual(row?.standing, "Keeping: Keep the tab playing · fixed 4 times · last 2 min ago")
        XCTAssertEqual(row?.status, "standing")
        XCTAssertFalse(row?.isRunning ?? true)
        XCTAssertTrue(row?.isOngoing ?? false)
        XCTAssertEqual(model.workRowText, "1 kept going")
        let stopped = expectation(description: "The loop's task is stopped")
        rpc.onCall = { method in if method == "task.stop" { stopped.fulfill() } }
        model.stopTask(row!)
        await fulfillment(of: [stopped], timeout: 2)
        XCTAssertEqual(rpc.calls.first { $0.0 == "task.stop" }?.1["task_ids"] as? [String], ["t1"])
    }

    func testTheWorkRowCountsActiveTasksAndLoopsKeptGoing() {
        let model = makeModel()
        XCTAssertFalse(model.showsWorkRow)
        model.receiveBusEvent(["topic": "task.created", "data": ["task_id": "a", "goal": "Book the table"]])
        model.receiveBusEvent(["topic": "task.created", "data": ["task_id": "t1", "goal": "Keep the tab playing"]])
        model.receiveBusEvent(loopStatus("t1", state: "watching", rounds: 1, lastRoundEnd: NSNull(), gap: 0))
        XCTAssertTrue(model.showsWorkRow)
        XCTAssertEqual(model.workRowText, "1 active task · 1 kept going")
        model.receiveBusEvent(loopStatus("t1", state: "acting", rounds: 1, lastRoundEnd: NSNull(), gap: 0))
        XCTAssertEqual(model.workRowText, "2 active tasks")
    }

    func testTheTaskResultRemovesTheStandingRowAndALateStatusDoesNotBringItBack() {
        let model = makeModel()
        model.receiveBusEvent(["topic": "task.created", "data": ["task_id": "t1", "goal": "Keep the tab playing"]])
        model.receiveBusEvent(loopStatus("t1", state: "watching", rounds: 2, lastRoundEnd: NSNull(), gap: 0))
        XCTAssertNotNil(model.standingWork["t1"])
        model.receiveBusEvent(["topic": "task.result", "data": ["task_id": "t1", "status": "done", "answer": "Stopped keeping it."]])
        XCTAssertNil(model.standingWork["t1"])
        model.receiveBusEvent(loopStatus("t1", state: "watching", rounds: 2, lastRoundEnd: NSNull(), gap: 0))
        XCTAssertNil(model.standingWork["t1"])
        XCTAssertEqual(model.keptGoingCount, 0)
        XCTAssertFalse(model.showsWorkRow)
    }

    func testAFinishedLoopLeavesTheRow() {
        let model = makeModel()
        model.receiveBusEvent(["topic": "task.created", "data": ["task_id": "t1", "goal": "Keep the tab playing"]])
        model.receiveBusEvent(loopStatus("t1", state: "watching", rounds: 2, lastRoundEnd: NSNull(), gap: 0))
        model.receiveBusEvent(loopStatus("t1", state: "finished", rounds: 2, lastRoundEnd: NSNull(), gap: 0))
        XCTAssertNil(model.standingWork["t1"])
        XCTAssertNil(model.taskItems.first { $0.backendID == "t1" }?.standing)
    }

    func testTheStatusRefreshRebuildsStandingWorkFromChatStatusLoops() async {
        let rpc = LoopRPC()
        let end = Date().timeIntervalSince1970 - 300
        rpc.status = [
            "running": [["task_id": "t1", "goal": "Keep the tab playing"], ["task_id": "t2", "goal": "Keep the inbox at zero"]],
            "approvals": [[String: Any]](), "results": [[String: Any]](),
            "loops": [
                ["task_id": "t1", "step": "keep", "goal": "Keep the tab playing", "state": "watching", "rounds": 4,
                 "last_round_end": end, "gap_seconds": 0],
                ["task_id": "t2", "step": "inbox", "goal": "Keep the inbox at zero", "state": "stopped", "rounds": 1,
                 "last_round_end": end, "gap_seconds": 0],
                ["task_id": "gone", "step": "keep", "goal": "Keep something", "state": "watching", "rounds": 1,
                 "last_round_end": end, "gap_seconds": 0],
            ] as [[String: Any]],
        ]
        let model = makeModel(rpc)
        model.standingWork["old"] = StandingWork(goal: "Old", state: "watching", rounds: 1, lastRoundEnd: nil, gapSeconds: 0)
        await model.refreshDaemonStatus()
        XCTAssertEqual(Set(model.standingWork.keys), ["t1"])
        XCTAssertEqual(model.standingLine("t1", now: Date()), "Keeping: Keep the tab playing · fixed 4 times · last 5 min ago")
        XCTAssertEqual(model.keptGoingCount, 1)
        XCTAssertEqual(model.activeTaskCount, 1)          // t2 runs on; its loop step stopped
    }

    /// The chat that asked for the loop keeps its session open until the loop's outcome; between rounds it is standing
    /// work, not "Working" (no amber dot, not in the work status), while an acting round still shows working.
    func testTheRequestingChatIsReadyBetweenRoundsAndWorkingWhileARoundActs() {
        let model = makeModel()
        model.session.draft = "keep this tab playing"
        let local = model.session.beginWork()!
        model.session.bindBackendTask("t1", localID: local)
        model.session.noteBackendActivity("Keeping the tab playing")
        model.receiveBusEvent(["topic": "task.created", "data": ["task_id": "t1", "goal": "Keep the tab playing"]])
        model.receiveBusEvent(loopStatus("t1", state: "acting", rounds: 0, lastRoundEnd: NSNull(), gap: 0))
        XCTAssertEqual(model.displayPhase, .working)
        XCTAssertEqual(model.workStatus, "Keeping the tab playing")
        for state in ["watching", "backing_off", "paused"] {
            model.receiveBusEvent(loopStatus("t1", state: state, rounds: 1, lastRoundEnd: NSNull(), gap: 0))
            XCTAssertEqual(model.displayPhase, .ready, state)
            XCTAssertEqual(model.workStatus, "Working…", state)
        }
        model.receiveBusEvent(loopStatus("t1", state: "acting", rounds: 1, lastRoundEnd: NSNull(), gap: 0))
        XCTAssertEqual(model.displayPhase, .working)
    }

    func testAStandingLoopInABackgroundSessionLeavesOtherWorkShowing() {
        let model = makeModel()
        model.session.draft = "keep this tab playing"
        let loopLocal = model.session.beginWork()!
        model.session.bindBackendTask("t1", localID: loopLocal)
        model.workSessions[loopLocal] = model.session
        model.session = NotchSession()
        model.session.draft = "book the table"
        let local = model.session.beginWork()!
        model.session.bindBackendTask("a", localID: local)
        model.session.noteBackendActivity("Booking the table")
        model.receiveBusEvent(["topic": "task.created", "data": ["task_id": "t1", "goal": "Keep the tab playing"]])
        model.receiveBusEvent(["topic": "task.created", "data": ["task_id": "a", "goal": "Book the table"]])
        model.receiveBusEvent(loopStatus("t1", state: "watching", rounds: 1, lastRoundEnd: NSNull(), gap: 0))
        XCTAssertEqual(model.displayPhase, .working)       // the booking runs
        XCTAssertEqual(model.workStatus, "Booking the table")
    }

    private func makeModel(_ rpc: DaemonRPCTransport = QuietRPC()) -> NotchModel {
        NotchModel(rpc: rpc, preferences: UserDefaults(suiteName: UUID().uuidString)!)
    }

    private func loopStatus(_ taskID: String, state: String, rounds: Int, lastRoundEnd: Any, gap: Double) -> [String: Any] {
        ["topic": "loop.status", "data": [
            "task_id": taskID, "step": "keep", "goal": "Keep the tab playing", "state": state, "rounds": rounds,
            "last_round_end": lastRoundEnd, "gap_seconds": gap] as [String: Any]]
    }
}

private final class QuietRPC: DaemonRPCTransport {
    func call(_ method: String, params: [String: Any]) async throws -> [String: Any] { [:] }
    func stream(_ method: String, params: [String: Any]) -> AsyncThrowingStream<[String: Any], Error> {
        AsyncThrowingStream { _ in }
    }
}

private final class LoopRPC: DaemonRPCTransport {
    private let recorded = RecordedCalls()
    var calls: [(String, [String: Any])] { recorded.all }
    var onCall: ((String) -> Void)?
    var status: [String: Any] = [:]
    func call(_ method: String, params: [String: Any]) async throws -> [String: Any] {
        recorded.append(method, params)
        onCall?(method)
        return method == "chat.status" ? status : [:]
    }
    func stream(_ method: String, params: [String: Any]) -> AsyncThrowingStream<[String: Any], Error> {
        AsyncThrowingStream { _ in }
    }
}
