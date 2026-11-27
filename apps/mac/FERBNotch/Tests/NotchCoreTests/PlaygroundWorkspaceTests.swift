#if FERB_PLAYGROUND
import XCTest
@testable import FERBNotch
import NotchCore

@MainActor
final class PlaygroundWorkspaceTests: XCTestCase {
    private func make() throws -> PlaygroundServices {
        let home = FileManager.default.temporaryDirectory.appendingPathComponent("ferb-review-\(UUID())")
        addTeardownBlock { try? FileManager.default.removeItem(at: home) }
        return PlaygroundServices.make(home: home, defaults: scratchDefaults(), fresh: true)
    }

    // decision: APP-619
    func testWindowMutationsOnlyChangeTheSelectedSample() async throws {
        let services = try make()
        let store = FERBWindowStore(rpc: services.daemon)
        await store.refresh(.schedules)
        XCTAssertGreaterThan(store.schedules.count, 1)
        XCTAssertGreaterThan(store.reminders.count, 1)
        let schedule = try XCTUnwrap(store.schedules.first)
        let other = store.schedules[1]
        await store.change(schedule, "pause")
        XCTAssertEqual(store.schedules.first(where: { $0.id == schedule.id })?.state, "paused")
        XCTAssertEqual(store.schedules.first(where: { $0.id == other.id }), other)
        let reminder = try XCTUnwrap(store.reminders.first)
        let count = store.reminders.count
        await store.cancel(reminder)
        XCTAssertEqual(store.reminders.count, count - 1)
        XCTAssertFalse(store.reminders.contains { $0.id == reminder.id })
        await store.refresh(.tasks)
        XCTAssertFalse(store.tasks.isEmpty)
        await store.refresh(.memory)
        XCTAssertFalse(store.activeMemory.isEmpty)
        await store.refresh(.noticed)
        XCTAssertFalse(store.notices.isEmpty)
        XCTAssertNil(store.problem)
    }

    // decision: APP-619
    func testChatQuestionAndCompletionTravelThroughTheRealNotch() async throws {
        let services = try make()
        services.state.setSpeed(100)
        let workspace = services.daemon.workspace
        try workspace.prepare()
        workspace.scenario = .question
        let model = services.notch
        model.feedback.enabled = false
        model.connectDaemon()
        defer { model.disconnectDaemon() }
        try await eventually { model.conversationID != nil && !workspace.listeners.isEmpty }
        model.send("A sample task for review")
        try await eventually { !model.pendingApprovals.isEmpty }
        let approval = try XCTUnwrap(model.pendingApprovals.first)
        XCTAssertEqual(approval.options.count, 2)
        XCTAssertEqual(workspace.running.count, 1)
        model.chooseApproval(approval, index: 0)
        try await eventually { model.pendingApprovals.isEmpty && workspace.running.isEmpty }
        try await eventually { model.conversation.turns.contains { $0.status == "done" && $0.backendTaskID == approval.taskID } }
        XCTAssertTrue(FileManager.default.fileExists(atPath: workspace.artifact.path))
        XCTAssertTrue(workspace.artifact.path.hasPrefix(services.home.path + "/"))
        workspace.deliverReminder()
        try await eventually { !model.conversation.notices.isEmpty }
        model.disconnectDaemon()
        try await eventually { workspace.listeners.isEmpty }
    }

    // decision: APP-619
    func testDecliningOrStoppingASampleApprovalClearsTheWait() async throws {
        let services = try make()
        services.state.setSpeed(100)
        let workspace = services.daemon.workspace
        try workspace.prepare()
        workspace.scenario = .approval
        for stopping in [false, true] {
            for try await _ in services.daemon.stream("chat.send", params: ["text": "Review an approval"]) {}
            let approval = try XCTUnwrap(workspace.approvals.first)
            let task = try XCTUnwrap(approval["task_id"] as? String)
            if stopping {
                _ = try await services.daemon.call("task.stop", params: ["task_ids": [task]])
            } else {
                _ = try await services.daemon.call("chat.approve", params: ["approval_id": approval["approval_id"]!, "granted": true, "choice": 2])
            }
            XCTAssertTrue(workspace.approvals.isEmpty)
            XCTAssertEqual(workspace.tasks.first(where: { $0["task_id"] as? String == task })?["status"] as? String, "cancelled")
        }
    }

    // decision: APP-619
    func testSettingsAndEvidenceAreSampleData() async throws {
        let services = try make()
        try services.daemon.workspace.prepare()
        let store = FERBWindowStore(rpc: services.daemon)
        await store.loadSettings()
        await store.set("sense.window_titles", .bool(false))
        await store.loadSettings()
        XCTAssertFalse(store.settings.bool("sense.window_titles"))
        await store.loadAbout()
        await store.loadRestore()
        await store.loadSecrets()
        XCTAssertNotNil(store.about)
        XCTAssertFalse(store.restoreRows.isEmpty)
        XCTAssertFalse(store.savedSecrets.isEmpty)
        let preview = try await services.daemon.call("tasks.preview", params: ["task_id": "sample-brief"])
        let seen = try await services.daemon.call("seen.image", params: ["id": "sample-capture"])
        XCTAssertEqual(preview["image"] as? String, seen["image"] as? String)
        XCTAssertFalse((seen["image"] as? String ?? "").isEmpty)
        await store.prepareDiagnostic(appVersion: "Playground")
        XCTAssertNotNil(store.diagnostic?.report)
        await store.sendDiagnostic(appVersion: "Playground")
        XCTAssertTrue(store.diagnostic?.isUnsent == true)
    }

    /// Reply to a message in the playground (owner 2026-11-27): FERB's sample answer can be quoted, the quote goes with
    /// the next message and the playground keeps it on that message's entry, as the engine does.
    func testAReplyToFERBsAnswerTravelsThroughTheSampleEntries() async throws {
        let services = try make()
        services.state.setSpeed(100)
        let workspace = services.daemon.workspace
        try workspace.prepare()
        workspace.scenario = .conversation
        let model = services.notch
        model.feedback.enabled = false
        model.connectDaemon()
        defer { model.disconnectDaemon() }
        try await eventually { model.conversationID != nil && model.entryWatch.caughtUp }
        model.send("Help me plan the afternoon")
        try await eventually { model.conversation.turns.first?.answerEntryID != nil && model.conversation.turns.first?.entryID != nil }
        let answered = try XCTUnwrap(model.conversation.turns.first)
        let quote = try XCTUnwrap(model.quote(entryID: answered.answerEntryID, kind: "answer", owner: false,
                                              text: answered.visibleAnswer))
        model.reply(to: quote)
        model.session.draft = "Make the focus block an hour"
        model.startWork()
        XCTAssertNil(model.shownComposerQuote)
        try await eventually { model.conversation.turns.count == 2 && model.conversation.turns[1].entryID != nil }
        let sent = model.conversation.turns[1]
        XCTAssertEqual(sent.replyTo?.entryID, answered.answerEntryID)
        XCTAssertEqual(sent.replyTo?.who, "ferb")
        XCTAssertTrue(answered.answer.hasPrefix(sent.replyTo?.excerpt ?? "-"), "the entry keeps the answer's opening words")
    }

    private func eventually(_ condition: () -> Bool) async throws {
        let deadline = ContinuousClock.now.advanced(by: .seconds(3))
        while !condition(), ContinuousClock.now < deadline { try await Task.sleep(for: .milliseconds(10)) }
        XCTAssertTrue(condition())
    }

    // decision: APP-619
    func testTheSampleProjectFolderCanBeRemoved() async throws {
        let services = try make()
        let store = FERBWindowStore(rpc: services.daemon)
        await store.loadProjects()
        XCTAssertEqual(store.projects.map(\.shown), ["~/code/site"])
        XCTAssertEqual(store.projects.first?.destinations, ["https://github.com/demo/site.git"])
        await store.removeProject(try XCTUnwrap(store.projects.first))
        XCTAssertTrue(store.projects.isEmpty)
        XCTAssertNil(store.problem)
    }

    // decision: APP-619
    func testUnknownCallsNeverReportSuccess() async throws {
        let services = try make()
        do {
            _ = try await services.daemon.call("not.a.simulated.operation", params: [:])
            XCTFail("Unknown operations must not pretend to succeed")
        } catch {}
    }
}
#endif
