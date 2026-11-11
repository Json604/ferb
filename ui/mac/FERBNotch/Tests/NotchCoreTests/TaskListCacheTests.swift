import XCTest
@testable import FERBNotch
@testable import NotchCore

/// The notch lagged while typing, opening and closing (owner, 2026-10-25): the task list was rebuilt and sorted on
/// every read, and the pointer, every animation frame and every keystroke read it several times. It is now built once
/// per change of what it reads.
@MainActor
final class TaskListCacheTests: XCTestCase {
    // decision: APP-590
    func testTheKeptListMatchesAFreshBuildAcrossEveryChange() {
        let model = makeModel()
        model.conversation.begin(id: UUID(), request: "Small talk", sources: [])
        let older = UUID()
        model.conversation.begin(id: older, request: "Find the receipt", sources: [])
        model.conversation.bind(id: older, taskID: "older")
        model.conversation.finish(id: older, answer: "Found it.", status: "done", artifacts: [])
        assertFresh(model, "history")

        // A new turn bound to a task.
        let added = UUID()
        model.conversation.begin(id: added, request: "Book the table", sources: [])
        model.conversation.bind(id: added, taskID: "book")
        assertFresh(model, "a turn added")
        XCTAssertEqual(model.taskItems.first?.backendID, "book")
        XCTAssertEqual(model.taskItems.first?.status, "Status unavailable")

        // The daemon starts it, then a second task this chat didn't send.
        model.receiveBusEvent(["topic": "task.created", "data": ["task_id": "book", "goal": "Book the table"]])
        assertFresh(model, "task running")
        XCTAssertEqual(model.taskItems.first { $0.id == "book" }?.status, "working")
        model.receiveBusEvent(["topic": "task.created", "data": ["task_id": "elsewhere", "goal": "Tidy the desktop"]])
        assertFresh(model, "a new running task")
        XCTAssertEqual(model.activeTaskCount, 2)
        XCTAssertEqual(model.workRowText, "2 active tasks")

        // A question for the booking.
        model.receiveBusEvent(["topic": "approval.requested", "data": ["approval_id": "q", "task_id": "book",
                                                                       "reason": "Which time?"]])
        assertFresh(model, "pending approval")
        XCTAssertEqual(model.taskItems.first { $0.id == "book" }?.status, "needs_user")
        model.receiveBusEvent(["topic": "approval.resolved", "data": ["approval_id": "q"]])
        assertFresh(model, "approval resolved")
        XCTAssertEqual(model.taskItems.first { $0.id == "book" }?.status, "working")

        // A loop kept going changes state.
        model.receiveBusEvent(["topic": "task.created", "data": ["task_id": "loop", "goal": "Keep the tab playing"]])
        for state in ["watching", "acting", "backing_off", "paused", "watching"] {
            model.receiveBusEvent(loopStatus("loop", state: state, rounds: 2))
            assertFresh(model, "loop " + state)
            XCTAssertEqual(model.taskItems.first { $0.id == "loop" }?.status,
                           state == "acting" ? "working" : "standing")
        }
        XCTAssertEqual(model.workRowText, "2 active tasks · 1 kept going")
        model.receiveBusEvent(loopStatus("loop", state: "stopped", rounds: 2))
        assertFresh(model, "loop stopped")
        XCTAssertEqual(model.keptGoingCount, 0)

        // A task finishes: never still shown as running.
        model.receiveBusEvent(["topic": "task.result",
                               "data": ["task_id": "book", "status": "done", "answer": "Booked."]])
        assertFresh(model, "task finished")
        XCTAssertEqual(model.taskItems.first { $0.id == "book" }?.status, "done")
        XCTAssertEqual(model.taskItems.first { $0.id == "book" }?.answer, "Booked.")
        model.receiveBusEvent(["topic": "task.result",
                               "data": ["task_id": "elsewhere", "status": "failed", "answer": "No."]])
        assertFresh(model, "recovered task finished")

        // The window's session: a request sent from the notch, its activity, then a second one beside it.
        model.session.draft = "Draft the reply"
        model.startWork()
        let first = model.session.activeTaskID!
        assertFresh(model, "request sent")
        XCTAssertEqual(model.taskItems.first?.title, "Draft the reply")
        model.receiveChatEvent(["type": "plan", "task_id": "reply", "goal": "Draft the reply"], localID: first)
        assertFresh(model, "request planned")
        model.receiveBusEvent(["topic": "task.progress", "data": ["task_id": "reply", "step_task_id": "reply.worker",
                                                                  "text": "drafting in Mail"]])
        assertFresh(model, "activity")
        XCTAssertEqual(model.taskItems.first { $0.id == "reply" }?.activity.last?.title, "drafting in Mail")
        model.session.draft = "Water the plants"
        model.startWork()
        assertFresh(model, "second request")
        model.receiveChatEvent(["type": "subtask", "summary": "Opening the reply"], localID: first)
        assertFresh(model, "background session activity")
        XCTAssertEqual(model.taskItems.first { $0.id == "reply" }?.activity.last?.title, "Opening the reply")

        // Switching the window to the first task and back.
        model.selectRunningTask("reply")
        XCTAssertEqual(model.session.backendTaskID, "reply")
        assertFresh(model, "switched sessions")
        model.receiveChatEvent(["type": "error", "text": "FERB could not start this request."],
                               localID: model.workSessions.keys.first { $0 != first }!)
        assertFresh(model, "background request failed")
        model.session.draft = "typed while it runs"
        assertFresh(model, "typing")
        model.collapse()
        model.expand()
        assertFresh(model, "closed and opened")
    }

    // decision: APP-590
    func testEachTaskFieldOfASessionRebuildsTheListOnItsOwn() {
        let model = makeModel()
        model.session.draft = "Sort the photos"
        let local = model.session.beginWork()!
        model.conversation.begin(id: local, request: "Sort the photos", sources: [])
        assertFresh(model, "window session")
        model.session.waitForOwner("Which album?")
        assertFresh(model, "window session needs the owner")
        XCTAssertEqual(model.taskItems.first?.status, "needs_user")
        model.session.resumeAfterOwnerAnswer()
        assertFresh(model, "window session resumed")
        XCTAssertEqual(model.taskItems.first?.status, "working")
        model.session.noteBackendActivity("Opening Photos")
        assertFresh(model, "window session activity")
        XCTAssertEqual(model.taskItems.first?.activity.last?.title, "Opening Photos")
        model.session.bindBackendTask("photos", localID: local)
        assertFresh(model, "window session bound")
        XCTAssertEqual(model.taskItems.first?.id, "photos")

        // A background session changed on its own, with the window's session untouched.
        var background = NotchSession()
        background.draft = "Water the plants"
        let other = background.beginWork()!
        model.workSessions[other] = background
        assertFresh(model, "background session added")
        XCTAssertEqual(model.taskItems.first { $0.localID == other }?.title, "Water the plants")
        model.workSessions[other]?.waitForOwner("Which plants?")
        assertFresh(model, "background session needs the owner")
        XCTAssertEqual(model.taskItems.first { $0.localID == other }?.status, "needs_user")
        model.workSessions.removeValue(forKey: other)
        assertFresh(model, "background session gone")
        XCTAssertNil(model.taskItems.first { $0.localID == other })
    }

    // decision: APP-590
    func testTheDraftCountsOnlyWhenItIsTheTasksTitle() {
        let model = makeModel()
        // A task picked up from the daemon with no goal, no turn and no daemon row yet: its title is the draft.
        model.session.resumeBackendTask("earlier", goal: "")
        XCTAssertEqual(model.taskItems.first?.title, "A task from earlier")
        let builds = model.taskListCache.builds
        model.session.draft = "what I am typing"
        assertFresh(model, "the draft names it")
        XCTAssertEqual(model.taskItems.first?.title, "what I am typing")
        XCTAssertEqual(model.taskListCache.builds, builds + 1)

        // Once the daemon names it, typing never rebuilds the list.
        model.runningTasks["earlier"] = "Earlier task"
        XCTAssertEqual(model.taskItems.first?.title, "Earlier task")
        let named = model.taskListCache.builds
        for text in ["w", "wh", "wha", "what"] { model.session.draft = text; _ = model.panelSize }
        XCTAssertEqual(model.taskListCache.builds, named)
        assertFresh(model, "typing after the daemon named it")
    }

    // decision: APP-590
    func testTheListIsBuiltOncePerChangeNotPerRead() {
        let model = makeHistory(running: true)
        model.session.expand()
        _ = model.taskItems
        let builds = model.taskListCache.builds
        for _ in 0..<200 {
            _ = model.panelSize
            _ = model.surfaceSize
            _ = model.showsWorkRow
            _ = model.workRowText
            _ = model.displayPhase
            _ = model.activeTaskCount
            _ = model.keptGoingCount
            _ = model.taskItems
        }
        XCTAssertEqual(model.taskListCache.builds, builds, "reads never rebuild")

        // Typing, opening, closing and the views the owner opens touch no input.
        for text in ["H", "He", "Hel", "Hell", "Hello"] { model.session.draft = text; _ = model.panelSize }
        model.collapse(); _ = model.panelSize
        model.expand(); _ = model.panelSize
        model.toggleTasks(); _ = model.panelSize
        model.toggleTasks(); _ = model.panelSize
        XCTAssertEqual(model.taskListCache.builds, builds, "typing, opening and closing never rebuild")

        // The status refresh sets the same running tasks, loops and questions again: nothing changed.
        model.runningTasks = model.runningTasks
        model.standingWork = model.standingWork
        model.pendingApprovals = model.pendingApprovals
        model.standingWork.removeValue(forKey: "absent")
        _ = model.panelSize
        XCTAssertEqual(model.taskListCache.builds, builds, "an unchanged refresh never rebuilds")

        // One change of an input: one build, however many reads follow.
        model.runningTasks["new"] = "A new task"
        for _ in 0..<50 { _ = model.panelSize; _ = model.taskItems }
        XCTAssertEqual(model.taskListCache.builds, builds + 1)
        model.conversation.begin(id: UUID(), request: "Another", sources: [])
        for _ in 0..<50 { _ = model.panelSize }
        XCTAssertEqual(model.taskListCache.builds, builds + 2)
    }

    // decision: APP-590
    func testTheLoopLineStillFollowsTheClock() async throws {
        let model = makeModel()
        model.receiveBusEvent(["topic": "task.created", "data": ["task_id": "loop", "goal": "Keep the tab playing"]])
        model.receiveBusEvent(loopStatus("loop", state: "watching", rounds: 1,
                                         lastRoundEnd: Date().timeIntervalSince1970 - 59.5))
        XCTAssertEqual(model.taskItems.first?.standing, "Keeping: Keep the tab playing · fixed 1 time · last just now")
        let builds = model.taskListCache.builds
        try await Task.sleep(for: .milliseconds(800))
        // No input changed and the list wasn't rebuilt; its line still reads the clock.
        XCTAssertEqual(model.taskItems.first?.standing, "Keeping: Keep the tab playing · fixed 1 time · last 1 min ago")
        XCTAssertEqual(model.taskListCache.builds, builds)
        XCTAssertEqual(model.taskItems.first?.standing, model.standingLine("loop", now: Date()))
    }

    // decision: APP-590
    func testTheComposerIsMeasuredForItsDraftAndWidth() {
        let model = makeModel()
        let long = String(repeating: "a long line that wraps in the composer ", count: 6)
        XCTAssertEqual(model.composerHeight, 36)
        model.session.draft = long
        let tall = model.composerHeight
        XCTAssertGreaterThan(tall, 36)
        XCTAssertEqual(tall, freshComposerHeight(draft: long, panelWidth: model.bodyWidth))
        model.resizePanel(width: model.bodyWidth + 100, transcriptHeight: model.preferredTranscriptHeight)
        XCTAssertEqual(model.composerHeight, freshComposerHeight(draft: long, panelWidth: model.bodyWidth))
        XCTAssertLessThan(model.composerHeight, tall)
        model.session.draft = ""
        XCTAssertEqual(model.composerHeight, 36)
    }

    /// The owner's history when it lagged: 254 turns, 105 of them tasks, about 287 KB. Prints 1000 reads' time.
    // decision: APP-590
    func testAThousandReadsOfTheGeometryOnALongHistory() {
        for running in [false, true] {
            let model = makeHistory(running: running)
            model.session.expand()
            let clock = ContinuousClock()
            var size = CGSize.zero
            let panel = clock.measure { for _ in 0..<1000 { size = model.panelSize } }
            var shows = false
            let row = clock.measure { for _ in 0..<1000 { shows = model.showsWorkRow } }
            XCTAssertGreaterThan(size.height, 0)
            XCTAssertEqual(shows, running)
            XCTAssertEqual(model.taskListCache.builds, 1)
            print("NOTCH-LAG running=\(running): 1000 panelSize \(panel), 1000 showsWorkRow \(row)")
        }
    }

    // MARK: - Support

    private func assertFresh(_ model: NotchModel, _ step: String, file: StaticString = #filePath, line: UInt = #line) {
        let kept = model.taskItems
        let fresh = model.makeTaskList().rows
        XCTAssertEqual(kept, fresh, step, file: file, line: line)
        XCTAssertEqual(model.activeTaskCount, fresh.filter(\.isRunning).count, step, file: file, line: line)
        XCTAssertEqual(model.keptGoingCount, fresh.filter { $0.status == "standing" }.count, step,
                       file: file, line: line)
    }

    private func freshComposerHeight(draft: String, panelWidth: CGFloat) -> CGFloat {
        let model = makeModel()
        model.resizePanel(width: panelWidth, transcriptHeight: model.preferredTranscriptHeight)
        model.session.draft = draft
        return model.composerHeight
    }

    private func makeModel() -> NotchModel {
        NotchModel(rpc: QuietTaskRPC(), preferences: scratchDefaults())
    }

    private func loopStatus(_ taskID: String, state: String, rounds: Int,
                            lastRoundEnd: Double = Date().timeIntervalSince1970 - 150) -> [String: Any] {
        ["topic": "loop.status", "data": [
            "task_id": taskID, "step": "keep", "goal": "Keep the tab playing", "state": state, "rounds": rounds,
            "last_round_end": lastRoundEnd, "gap_seconds": 0.0] as [String: Any]]
    }

    /// 254 turns over ten days, 105 bound to a task; with `running`, a task runs in a background session and another
    /// keeps a loop.
    private func makeHistory(running: Bool) -> NotchModel {
        let model = makeModel()
        let start = Date().addingTimeInterval(-9 * 86_400)
        let words = String(repeating: "FERB read the page, compared the options and wrote down what it found. ",
                           count: 14)
        var archive = model.conversation
        for index in 0..<254 {
            let local = UUID()
            let date = start.addingTimeInterval(Double(index) * 9 * 86_400 / 254)
            archive.begin(id: local, request: "Request \(index): look into the thing I mentioned earlier today",
                          sources: [], date: date)
            if index * 105 / 254 != (index + 1) * 105 / 254 {          // 105 tasks, spread over the history
                let hex = String(format: "%08x", index * 2_654_435_761 % 0xFFFF_FFFF)
                archive.bind(id: local, taskID: hex + String(format: "-task-%03d", index))
            }
            let status = index % 17 == 0 ? "failed" : index % 23 == 0 ? "needs_user"
                : index % 29 == 0 ? "cancelled" : "done"
            archive.finish(id: local, answer: words, status: status, artifacts: [], at: date)
        }
        model.conversation = archive
        XCTAssertEqual(model.conversation.turns.count, 254)
        XCTAssertEqual(model.conversation.turns.filter { $0.backendTaskID != nil }.count, 105)
        if running {
            model.receiveBusEvent(["topic": "task.created",
                                   "data": ["task_id": "loop", "goal": "Keep the tab playing"]])
            model.receiveBusEvent(loopStatus("loop", state: "watching", rounds: 2))
            model.session.draft = "Book the table"
            let local = model.session.beginWork()!
            model.session.bindBackendTask("book", localID: local)
            model.conversation.begin(id: local, request: "Book the table", sources: [])
            model.conversation.bind(id: local, taskID: "book")
            model.runningTasks["book"] = "Book the table"
            model.workSessions[local] = model.session
            model.session = NotchSession()
        }
        return model
    }
}

private final class QuietTaskRPC: DaemonRPCTransport {
    func call(_ method: String, params: [String: Any]) async throws -> [String: Any] { [:] }
    func stream(_ method: String, params: [String: Any]) -> AsyncThrowingStream<[String: Any], Error> {
        AsyncThrowingStream { _ in }
    }
}
