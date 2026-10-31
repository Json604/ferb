import XCTest
@testable import FERBNotch
@testable import NotchCore

@MainActor
final class AttentionReconciliationTests: XCTestCase {
    func testNewApprovalRevealsOnceWithoutRequestingFocusAcrossPollingAndRelaunch() async {
        let rpc = AttentionRPC()
        let preferences = scratchDefaults()
        let model = NotchModel(rpc: rpc, preferences: preferences)
        let approval: [String: Any] = ["approval_id": "new-question", "kind": "owner_question", "reason": "Which app?"]
        var focus = 0
        model.onPresentationChange = { _ in focus += 1 }
        model.receiveBusEvent(["topic": "approval.requested", "data": approval])
        XCTAssertTrue(model.bodyVisible)
        XCTAssertTrue(model.attentionTraceVisible)
        XCTAssertEqual(focus, 0)
        let started = model.attentionStartedAt
        model.collapse()
        rpc.status = ["approvals": [approval]]
        await model.refreshDaemonStatus()
        XCTAssertFalse(model.bodyVisible)
        XCTAssertEqual(model.attentionStartedAt, started)
        let restored = NotchModel(rpc: rpc, preferences: preferences)
        await restored.refreshDaemonStatus()
        XCTAssertFalse(restored.bodyVisible)
        XCTAssertFalse(restored.attentionTraceVisible)
    }

    func testNewFollowupRevealsButOrdinaryReplyStaysQuiet() {
        let model = NotchModel(preferences: scratchDefaults())
        model.session.draft = "Question"
        let local = model.session.beginWork()!
        model.conversation.begin(id: local, request: "Question", sources: [])
        model.collapse()
        model.receiveChatEvent(["type": "clarify", "text": "Which app?"], localID: local)
        XCTAssertTrue(model.bodyVisible)
        XCTAssertTrue(model.attentionTraceVisible)
        model.collapse()
        model.receiveChatEvent(["type": "clarify", "text": "Which app?"], localID: local)
        XCTAssertFalse(model.bodyVisible)
    }

    func testEndedUnboundStreamRetiresEmptyAndInterruptedRequests() {
        for hasDraft in [false, true] {
            let model = NotchModel(preferences: scratchDefaults())
            model.session.draft = "Set volume"
            let local = model.session.beginWork()!
            model.conversation.begin(id: local, request: "Set volume", sources: [])
            if hasDraft { model.receiveChatEvent(["type": "reply_delta", "text": "Working"], localID: local) }
            model.finishSubmission(localID: local)
            XCTAssertNil(model.session.activeTaskID)
            XCTAssertEqual(model.activeTaskCount, 0)
            XCTAssertEqual(model.conversation.turns.first?.status, "interrupted")
            XCTAssertFalse(model.conversation.turns.first?.isProvisional ?? true)
        }
    }

    func testMissingBackgroundTasksRecoverAllResultsWithoutChangingDraft() async {
        let rpc = AttentionRPC()
        let model = NotchModel(rpc: rpc, preferences: scratchDefaults())
        for task in ["first", "second"] {
            model.session = NotchSession()
            model.session.draft = task
            let local = model.session.beginWork()!
            model.session.bindBackendTask(task, localID: local)
            model.conversation.begin(id: local, request: task, sources: [])
            model.conversation.bind(id: local, taskID: task)
            model.workSessions[local] = model.session
            rpc.traces[task] = ["outcome": ["result": ["status": "done", "answer": task + " done"]]]
        }
        model.session.draft = "Next draft"
        await model.refreshDaemonStatus()
        XCTAssertEqual(model.activeTaskCount, 0)
        XCTAssertTrue(model.workSessions.isEmpty)
        XCTAssertEqual(model.conversation.turns.map(\.status), ["done", "done"])
        XCTAssertEqual(model.session.draft, "Next draft")
        XCTAssertEqual(Set(rpc.traceIDs), ["first", "second"])
    }
    func testStatusRefreshDoesNotRetireAnInFlightRequestWithoutBackendIdentity() async {
        let rpc = AttentionRPC()
        let model = NotchModel(rpc: rpc, preferences: scratchDefaults())
        model.session.draft = "Still submitting"
        let local = model.session.beginWork()!
        model.conversation.begin(id: local, request: "Still submitting", sources: [])
        await model.refreshDaemonStatus()
        XCTAssertEqual(model.session.activeTaskID, local)
        XCTAssertEqual(model.activeTaskCount, 1)
    }

    func testMissingTaskWithoutRecordedFinalIsRetiredWithoutFabricatingSuccess() async {
        let rpc = AttentionRPC()
        let preferences = scratchDefaults()
        let model = NotchModel(rpc: rpc, preferences: preferences)
        model.session.draft = "Old Spotify task"
        let local = model.session.beginWork()!
        model.session.bindBackendTask("old", localID: local)
        model.conversation.begin(id: local, request: "Old Spotify task", sources: [])
        model.conversation.bind(id: local, taskID: "old")
        model.workSessions[local] = model.session
        preferences.set("old", forKey: "FERBActiveTaskID")
        await model.refreshDaemonStatus()
        await model.refreshDaemonStatus()
        XCTAssertEqual(model.activeTaskCount, 0)
        XCTAssertNil(model.session.activeTaskID)
        XCTAssertTrue(model.workSessions.isEmpty)
        XCTAssertEqual(model.conversation.turns.first?.status, "interrupted")
        XCTAssertEqual(rpc.traceIDs, ["old"])
    }

    func testStartupRepairsAndPersistsAnEmptyUnboundAbandonedTurn() throws {
        let preferences = scratchDefaults()
        var archive = ConversationArchive()
        archive.begin(id: UUID(), request: "Old empty pending turn", sources: [])
        preferences.set(try JSONEncoder().encode(archive), forKey: "FERBConversationArchive")
        let model = NotchModel(preferences: preferences)
        XCTAssertEqual(model.conversation.turns.first?.status, "interrupted")
        let saved = try JSONDecoder().decode(ConversationArchive.self, from: XCTUnwrap(preferences.data(forKey: "FERBConversationArchive")))
        XCTAssertEqual(saved.turns.first?.status, "interrupted")
        XCTAssertEqual(model.activeTaskCount, 0)
    }

    func testCompletedRootRetiresDuplicateRecoveredLocalSessions() {
        let model = NotchModel(preferences: scratchDefaults())
        for _ in 0..<2 {
            model.session = NotchSession()
            model.session.draft = "Same backend task"
            let local = model.session.beginWork()!
            model.session.bindBackendTask("root", localID: local)
            model.conversation.begin(id: local, request: "Same backend task", sources: [])
            model.conversation.bind(id: local, taskID: "root")
            model.workSessions[local] = model.session
        }
        model.receiveOutcome(["task_id": "root", "status": "done", "answer": "Completed"])
        XCTAssertTrue(model.workSessions.isEmpty)
        XCTAssertEqual(model.activeTaskCount, 0)
        XCTAssertTrue(model.conversation.turns.allSatisfy { $0.status == "done" })
    }

    func testOlderStatusCannotResurrectCompletedRoot() async {
        let rpc = BlockedStatusRPC()
        let model = NotchModel(rpc: rpc, preferences: scratchDefaults())
        model.session.draft = "Finish task"
        let local = model.session.beginWork()!
        model.session.bindBackendTask("root", localID: local)
        model.conversation.begin(id: local, request: "Finish task", sources: [])
        model.conversation.bind(id: local, taskID: "root")
        model.runningTasks["root"] = "Finish task"
        let started = expectation(description: "Status snapshot requested")
        rpc.onStatus = { started.fulfill() }
        let refresh = Task { await model.refreshDaemonStatus() }
        await fulfillment(of: [started], timeout: 2)
        model.receiveBusEvent(["topic": "task.result", "data": ["task_id": "root", "status": "done", "answer": "Done"]])
        rpc.complete(["running": [["task_id": "root", "goal": "Finish task"]]])
        await refresh.value
        XCTAssertEqual(model.activeTaskCount, 0)
        XCTAssertNil(model.session.activeTaskID)
        XCTAssertNil(model.runningTasks["root"])
        XCTAssertTrue(model.knownCompletedTasks.contains("root"))
    }

    func testOlderStatusCannotRestoreResolvedApproval() async {
        let rpc = BlockedStatusRPC()
        let model = NotchModel(rpc: rpc, preferences: scratchDefaults())
        let approval: [String: Any] = ["approval_id": "resolved", "reason": "Choose"]
        model.receiveBusEvent(["topic": "approval.requested", "data": approval])
        model.collapse()
        let started = expectation(description: "Status requested")
        rpc.onStatus = { started.fulfill() }
        let refresh = Task { await model.refreshDaemonStatus() }
        await fulfillment(of: [started], timeout: 2)
        model.receiveBusEvent(["topic": "approval.resolved", "data": ["approval_id": "resolved"]])
        rpc.complete(["approvals": [approval]])
        await refresh.value
        XCTAssertTrue(model.pendingApprovals.isEmpty)
        XCTAssertFalse(model.bodyVisible)
    }

    func testOlderStatusCannotEraseNewApproval() async {
        let rpc = BlockedStatusRPC()
        let model = NotchModel(rpc: rpc, preferences: scratchDefaults())
        let started = expectation(description: "Status requested")
        rpc.onStatus = { started.fulfill() }
        let refresh = Task { await model.refreshDaemonStatus() }
        await fulfillment(of: [started], timeout: 2)
        model.receiveBusEvent(["topic": "approval.requested", "data": ["approval_id": "new", "reason": "Choose"]])
        rpc.complete(["approvals": []])
        await refresh.value
        XCTAssertEqual(model.pendingApprovals.map(\.id), ["new"])
        XCTAssertTrue(model.bodyVisible)
    }

    func testClarificationLeavesPriorActivityViewSoQuestionIsVisible() {
        let model = NotchModel(preferences: scratchDefaults())
        model.session.draft = "Question"
        let local = model.session.beginWork()!
        model.conversation.begin(id: local, request: "Question", sources: [])
        model.showingActivity = true
        model.receiveChatEvent(["type": "clarify", "text": "Which app?"], localID: local)
        XCTAssertFalse(model.showingActivity)
        XCTAssertTrue(model.bodyVisible)
    }

}

private final class AttentionRPC: DaemonRPCTransport {
    var status: [String: Any] = [:]
    var traces: [String: [String: Any]] = [:]
    var traceIDs: [String] = []
    func call(_ method: String, params: [String: Any]) async throws -> [String: Any] {
        if method == "chat.status" { return status }
        if method == "trace.get", let id = params["task_id"] as? String { traceIDs.append(id); return traces[id] ?? [:] }
        return [:]
    }
    func stream(_ method: String, params: [String: Any]) -> AsyncThrowingStream<[String: Any], Error> {
        AsyncThrowingStream { _ in }
    }
}

private final class BlockedStatusRPC: DaemonRPCTransport {
    var onStatus: (() -> Void)?
    private var continuation: CheckedContinuation<[String: Any], Error>?
    func call(_ method: String, params: [String: Any]) async throws -> [String: Any] {
        guard method == "chat.status" else { return [:] }
        return try await withCheckedThrowingContinuation { continuation in
            self.continuation = continuation
            onStatus?()
        }
    }
    func complete(_ status: [String: Any]) { continuation?.resume(returning: status); continuation = nil }
    func stream(_ method: String, params: [String: Any]) -> AsyncThrowingStream<[String: Any], Error> {
        AsyncThrowingStream { _ in }
    }
}
