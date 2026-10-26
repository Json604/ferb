import XCTest
@testable import FERBNotch
@testable import NotchCore

/// L34 Phase E: each window button is one RPC; a failure is the window's problem line, never a crash or a silent
/// empty list.
@MainActor
final class FERBWindowStoreTests: XCTestCase {
    private func task(_ id: String, _ status: String = "done", at: String = "2026-10-25T10:00:00+00:00") -> [String: Any] {
        ["task_id": id, "goal": "goal \(id)", "status": status, "created_at": at, "answer": "", "verdict": NSNull()]
    }

    func testSendDiagnosticsOpensANewEmailToTheDeveloperWithTheFile() async {
        // Owner 2026-10-27: Settings → Send diagnostics to developer, like a failure report; FERB sends nothing itself
        let rpc = ScriptedRPC()
        rpc.replies["reports.diagnostic"] = ["path": "/Users/me/.ferb/reports/FERB-diagnostic-2026-10-27-020000.txt",
                                             "summary": "FERB 0.27, macOS 15.6", "address": "dev@example.com",
                                             "subject": "FERB diagnostics (FERB 0.27, macOS 15.6)"]
        let mailer = FakeMailer(composes: true)
        let store = FERBWindowStore(rpc: rpc, mailer: mailer)
        await store.sendDiagnostic(appVersion: "0.27 (1)")
        XCTAssertEqual(rpc.params(of: "reports.diagnostic").first?["send"] as? Bool, true)
        XCTAssertEqual(mailer.composed.first?.recipients, ["dev@example.com"])
        XCTAssertEqual(mailer.composed.first?.subject, "FERB diagnostics (FERB 0.27, macOS 15.6)")
        XCTAssertTrue((mailer.composed.first?.items.first as? String)?.contains("diagnostic file") == true)
        XCTAssertEqual(store.diagnosticLine, "A new email with the diagnostic file is open in Mail.")
        let noMail = FakeMailer(composes: false)
        let other = FERBWindowStore(rpc: rpc, mailer: noMail)
        await other.sendDiagnostic(appVersion: nil)
        XCTAssertEqual(noMail.revealed.map(\.lastPathComponent), ["FERB-diagnostic-2026-10-27-020000.txt"])
        XCTAssertEqual(other.diagnosticLine, "Send this file to dev@example.com")
    }

    func testRefreshTasksReadsTheDaemonsPageAndStopState() async {
        let rpc = ScriptedRPC()
        rpc.replies["tasks.list"] = ["tasks": [task("t-2"), task("t-1")], "more": true]
        rpc.replies["ping"] = ["stopped": true]
        let store = FERBWindowStore(rpc: rpc)
        await store.refresh(.tasks)
        XCTAssertEqual(store.tasks.map(\.id), ["t-2", "t-1"])
        XCTAssertTrue(store.moreTasks)
        XCTAssertTrue(store.stopped)
        XCTAssertNil(store.problem)
    }

    func testShowingOlderTasksAsksForTheNextPageBeforeTheLastRow() async {
        let rpc = ScriptedRPC()
        rpc.queue("tasks.list", [["tasks": [task("t-2", at: "2026-10-25T10:00:02+00:00")], "more": true],
                                 ["tasks": [task("t-1", at: "2026-10-25T10:00:01+00:00")], "more": false]])
        let store = FERBWindowStore(rpc: rpc)
        await store.loadTasks()
        await store.loadTasks(more: true)
        XCTAssertEqual(store.tasks.map(\.id), ["t-2", "t-1"])
        XCTAssertEqual(rpc.params(of: "tasks.list").last?["before"] as? String, "2026-10-25T10:00:02+00:00")
        XCTAssertFalse(store.moreTasks)
    }

    func testStoppingOneTaskSendsOnlyItsID() async {
        let rpc = ScriptedRPC()
        rpc.replies["tasks.list"] = ["tasks": [task("t-run", "running"), task("t-done")], "more": false]
        let store = FERBWindowStore(rpc: rpc)
        await store.loadTasks()
        await store.stop(store.tasks[0])
        await store.stop(store.tasks[1])                      // ended: nothing to stop
        XCTAssertEqual(rpc.params(of: "task.stop").count, 1)
        XCTAssertEqual(rpc.params(of: "task.stop").first?["task_ids"] as? [String], ["t-run"])
    }

    func testVerdictsGoToVerdictSet() async {
        let rpc = ScriptedRPC()
        rpc.replies["tasks.list"] = ["tasks": [task("t-1")], "more": false]
        let store = FERBWindowStore(rpc: rpc)
        await store.loadTasks()
        await store.setVerdict(store.tasks[0], "fail")
        XCTAssertEqual(rpc.params(of: "verdict.set").first?["task_id"] as? String, "t-1")
        XCTAssertEqual(rpc.params(of: "verdict.set").first?["verdict"] as? String, "fail")
    }

    func testStopEverythingAndResume() async {
        let rpc = ScriptedRPC()
        let store = FERBWindowStore(rpc: rpc)
        await store.stopEverything()
        XCTAssertTrue(store.stopped)
        await store.resumeEverything()
        XCTAssertFalse(store.stopped)
        XCTAssertEqual(rpc.methods(), ["stop.engage", "stop.lift"])
    }

    func testScheduleActionsAndReminderCancel() async {
        let rpc = ScriptedRPC()
        rpc.replies["schedules.list"] = ["schedules": [["id": "sc-1", "request": "Post", "state": "active"]]]
        rpc.replies["reminders.list"] = ["pending": [["id": "tm-1", "kind": "reminder", "message": "Call"]]]
        rpc.replies["schedules.change"] = ["text": "Paused: Post"]
        let store = FERBWindowStore(rpc: rpc)
        await store.loadSchedules()
        await store.change(store.schedules[0], "pause")
        await store.cancel(store.reminders[0])
        XCTAssertEqual(rpc.params(of: "schedules.change").first?["id"] as? String, "sc-1")
        XCTAssertEqual(rpc.params(of: "schedules.change").first?["action"] as? String, "pause")
        XCTAssertEqual(rpc.params(of: "reminders.cancel").first?["id"] as? String, "tm-1")
    }

    func testForgetAsksWithTheDaemonsListBeforeDeleting() async throws {
        let rpc = ScriptedRPC()
        rpc.queue("memory.forget", [["confirmed": false, "will_delete": ["memory item “Uses VS Code”", "2 trail row(s)"]],
                                    ["confirmed": true, "deleted": [String]()]])
        let item = try XCTUnwrap(MemoryRow(["id": "m-1", "content": "Uses VS Code", "status": "active"]))
        let store = FERBWindowStore(rpc: rpc)
        let list = await store.forgetPreview(item)
        XCTAssertEqual(list, ["memory item “Uses VS Code”", "2 trail row(s)"])
        XCTAssertNil(rpc.params(of: "memory.forget").first?["confirm"])      // the preview deletes nothing
        await store.forget(item)
        XCTAssertEqual(rpc.params(of: "memory.forget").last?["confirm"] as? Bool, true)
    }

    func testAFailedSettingWriteRestoresTheShownValueAndSaysWhy() async {
        let rpc = ScriptedRPC()
        rpc.replies["settings.get"] = ["values": ["sense.window_titles": true]]
        rpc.failures["settings.set"] = DaemonRPCError.remote("sense.window_titles must be true or false")
        let store = FERBWindowStore(rpc: rpc)
        await store.loadSettings()
        await store.set("sense.window_titles", .bool(false))
        XCTAssertTrue(store.settings.bool("sense.window_titles"))
        XCTAssertEqual(store.problem, "sense.window_titles must be true or false")
    }

    func testASettingWriteShowsWhatTheDaemonStored() async {
        let rpc = ScriptedRPC()
        rpc.replies["settings.set"] = ["key": "privacy.exclude_domains", "value": ["example.com"]]
        let store = FERBWindowStore(rpc: rpc)
        await store.set("privacy.exclude_domains", .strings([".Example.COM"]))
        XCTAssertEqual(store.settings.strings("privacy.exclude_domains"), ["example.com"])
        XCTAssertEqual(rpc.params(of: "settings.set").first?["value"] as? [String], [".Example.COM"])
    }

    func testBannerSwitchesGoThroughPlanDsCategories() async {
        let rpc = ScriptedRPC()
        rpc.replies["notifications.categories"] = ["categories": ["question": true, "reminder": true, "alert": true,
                                                                  "task_finished": false, "loop": true]]
        let store = FERBWindowStore(rpc: rpc)
        await store.loadBanners()
        XCTAssertEqual(store.banners["task_finished"], false)
        await store.setBanner(.reminder, false)
        XCTAssertEqual(rpc.params(of: "notifications.categories").last?["set"] as? [String: Bool], ["reminder": false])
    }

    func testRefreshWithoutTheDaemonShowsWhyNotAnEmptyList() async {
        let rpc = ScriptedRPC()
        rpc.failures["schedules.list"] = DaemonRPCError.unavailable
        rpc.failures["reminders.list"] = DaemonRPCError.unavailable
        let store = FERBWindowStore(rpc: rpc)
        await store.refresh(.schedules)
        XCTAssertEqual(store.problem, DaemonRPCError.unavailable.localizedDescription)
        XCTAssertTrue(store.schedules.isEmpty)
    }

    func testCheckingTheRuntimeWaitsForItsAnswer() async {
        let rpc = ScriptedRPC()
        rpc.replies["runtimes.check"] = ["ok": false, "detail": "no answer"]
        let store = FERBWindowStore(rpc: rpc)
        await store.checkRuntime("codex")
        XCTAssertEqual(rpc.params(of: "runtimes.check").first?["runtime"] as? String, "codex")
        XCTAssertEqual(rpc.waits["runtimes.check"], FERBWindowStore.runtimeWait)
        XCTAssertEqual(store.note, "codex didn't answer: no answer")
    }

    func testExportingADiagnosticFileWaitsAndReturnsItsPath() async {
        let rpc = ScriptedRPC()
        rpc.replies["reports.diagnostic"] = ["path": "/tmp/FERB-diagnostic-2026-10-25-101500.txt", "summary": ""]
        let store = FERBWindowStore(rpc: rpc)
        let url = await store.exportDiagnostic(appVersion: "0.28.0 (1)")
        XCTAssertEqual(url?.path, "/tmp/FERB-diagnostic-2026-10-25-101500.txt")
        XCTAssertEqual(rpc.params(of: "reports.diagnostic").first?["app_version"] as? String, "0.28.0 (1)")
        XCTAssertEqual(rpc.waits["reports.diagnostic"], FERBWindowStore.diagnosticWait)
    }

    func testInspectWritesTheTraceAndOpensIt() async throws {
        let rpc = ScriptedRPC()
        rpc.replies["trace.get"] = ["task_id": "t-1", "spans": [[String: Any]]()]
        var opened: [URL] = []
        let store = FERBWindowStore(rpc: rpc)
        store.open = { opened.append($0) }
        let row = try XCTUnwrap(TaskHistoryRow(task("t-1")))
        await store.inspect(row)
        XCTAssertEqual(rpc.params(of: "trace.get").first?["task_id"] as? String, "t-1")
        XCTAssertEqual(opened.count, 1)
        XCTAssertEqual(opened.first?.pathExtension, "json")
    }
}
