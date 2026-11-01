import XCTest
@testable import FERBNotch
@testable import NotchCore

@MainActor
final class AttentionReconciliationTests: XCTestCase {
    // decision: APP-301, APP-571
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

    // decision: APP-301
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

    // decision: APP-577
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
