import XCTest
@testable import FERBNotch
@testable import NotchCore

@MainActor
private final class FakeUpdater: UpdaterDriving {
    var automaticallyDownloadsUpdates = true
    var checks = 0
    func checkForUpdates() { checks += 1 }
}

/// L34 Phase G: an update installs only when FERB's daemon says nothing runs that a restart would cut, and FERB has
/// stopped; a quit while a task runs asks first; a failed install starts FERB again.
@MainActor
final class UpdateCoordinatorTests: XCTestCase {
    private func make(_ rpc: ScriptedRPC) -> (UpdateCoordinator, FakeUpdater) {
        let driver = FakeUpdater()
        let updates = UpdateCoordinator(rpc: rpc, driver: driver)
        updates.retryInterval = .milliseconds(10)
        updates.exitWait = .milliseconds(200)
        updates.restartDaemon = {}
        return (updates, driver)
    }

    private func busy(_ goal: String = "Rename the invoices") -> [String: Any] {
        ["ready": false, "running": [["task_id": "t-1", "goal": goal]]]
    }

    private func waitForAsk(_ rpc: ScriptedRPC) async throws {
        for _ in 0..<200 where rpc.params(of: "update.prepare").isEmpty { try await Task.sleep(for: .milliseconds(5)) }
    }

    func testAnIdleFERBStopsThenSparkleInstalls() async {
        let rpc = ScriptedRPC()
        rpc.replies["update.prepare"] = ["ready": true, "running": [[String: Any]]()]
        rpc.failures["ping"] = DaemonRPCError.unavailable                  // the daemon has exited
        let (updates, _) = make(rpc)
        var installed = 0
        await updates.readyToInstall(version: "0.28.0", install: { installed += 1 }).value
        XCTAssertEqual(installed, 1)
        XCTAssertEqual(updates.status, .installing("0.28.0"))
        XCTAssertEqual(rpc.params(of: "update.prepare").first?["version"] as? String, "0.28.0")
        XCTAssertNil(rpc.params(of: "update.prepare").first?["even_if_busy"])
    }

    func testARunningTaskHoldsTheInstallUntilItEnds() async {
        let rpc = ScriptedRPC()
        rpc.queue("update.prepare", [busy(), busy()])
        rpc.replies["update.prepare"] = ["ready": true, "running": [[String: Any]]()]
        rpc.failures["ping"] = DaemonRPCError.unavailable
        let (updates, _) = make(rpc)
        var installed = 0
        await updates.readyToInstall(version: "0.28.0", install: { installed += 1 }).value
        XCTAssertEqual(rpc.params(of: "update.prepare").count, 3)
        XCTAssertEqual(installed, 1)
    }

    func testWhileWaitingTheStatusNamesTheWork() async throws {
        let rpc = ScriptedRPC()
        rpc.replies["update.prepare"] = busy()
        let (updates, _) = make(rpc)
        updates.retryInterval = .seconds(30)
        let job = updates.readyToInstall(version: "0.28.0", install: { XCTFail("installed while a task ran") })
        try await waitForAsk(rpc)
        try await Task.sleep(for: .milliseconds(20))
        XCTAssertEqual(updates.status, .waiting(version: "0.28.0", work: ["Rename the invoices"]))
        XCTAssertEqual(updates.status.line, "FERB 0.28.0 installs when this finishes: Rename the invoices")
        job.cancel()
    }

    func testNoDaemonMeansNoTaskCanBeRunning() async {
        let rpc = ScriptedRPC()
        rpc.failures["update.prepare"] = DaemonRPCError.unavailable
        let (updates, _) = make(rpc)
        var installed = 0
        await updates.readyToInstall(version: "0.28.0", install: { installed += 1 }).value
        XCTAssertEqual(installed, 1)
    }

    func testAnUnclearAnswerIsTreatedAsBusy() async throws {
        let rpc = ScriptedRPC()
        rpc.failures["update.prepare"] = DaemonRPCError.internalFault
        let (updates, _) = make(rpc)
        updates.retryInterval = .seconds(30)
        let job = updates.readyToInstall(version: "0.28.0", install: { XCTFail("installed without a clear answer") })
        try await waitForAsk(rpc)
        try await Task.sleep(for: .milliseconds(20))
        guard case .waiting = updates.status else { return XCTFail("expected waiting, got \(updates.status)") }
        job.cancel()
    }

    func testAnUpdateThatFailedAfterFERBStoppedStartsFERBAgain() async {
        let rpc = ScriptedRPC()
        rpc.replies["update.prepare"] = ["ready": true, "running": [[String: Any]]()]
        rpc.failures["ping"] = DaemonRPCError.unavailable
        let (updates, _) = make(rpc)
        var restarts = 0
        updates.restartDaemon = { restarts += 1 }
        await updates.readyToInstall(version: "0.28.0", install: {}).value
        updates.aborted("The update is improperly signed.")
        XCTAssertEqual(restarts, 1)
        XCTAssertEqual(updates.status, .failed("The update is improperly signed."))
        updates.aborted("No update")                                      // nothing was stopped this time
        XCTAssertEqual(restarts, 1)
    }

    func testQuittingWithATaskRunningAsksAndKeepFERBOpenInstallsNothing() async throws {
        let rpc = ScriptedRPC()
        rpc.replies["update.prepare"] = busy()
        let (updates, _) = make(rpc)
        updates.retryInterval = .seconds(30)
        var asked: [String] = []
        updates.confirmQuit = { _, work in asked = work; return false }
        let job = updates.readyToInstall(version: "0.28.0", install: { XCTFail("installed while a task ran") })
        try await waitForAsk(rpc)
        let allowed = await withCheckedContinuation { c in updates.beforeQuit { c.resume(returning: $0) } }
        XCTAssertFalse(allowed)
        XCTAssertEqual(asked, ["Rename the invoices"])
        XCTAssertFalse(rpc.params(of: "update.prepare").contains { $0["even_if_busy"] as? Bool == true })
        job.cancel()
    }

    func testQuitAndInstallStopsFERBEvenWithATaskRunning() async throws {
        let rpc = ScriptedRPC()
        rpc.queue("update.prepare", [busy(), busy()])
        rpc.replies["update.prepare"] = ["ready": true, "running": [["task_id": "t-1", "goal": "Rename the invoices"]]]
        rpc.failures["ping"] = DaemonRPCError.unavailable
        let (updates, _) = make(rpc)
        updates.retryInterval = .seconds(30)
        updates.confirmQuit = { _, _ in true }
        let job = updates.readyToInstall(version: "0.28.0", install: {})
        try await waitForAsk(rpc)
        let allowed = await withCheckedContinuation { c in updates.beforeQuit { c.resume(returning: $0) } }
        XCTAssertTrue(allowed)
        XCTAssertEqual(rpc.params(of: "update.prepare").last?["even_if_busy"] as? Bool, true)
        XCTAssertTrue(updates.prepared)
        job.cancel()
    }

    func testQuittingWithoutAnUpdateWaitingNeverAsksFERB() async {
        let rpc = ScriptedRPC()
        let (updates, _) = make(rpc)
        let allowed = await withCheckedContinuation { c in updates.beforeQuit { c.resume(returning: $0) } }
        XCTAssertTrue(allowed)
        XCTAssertTrue(rpc.calls.isEmpty)
    }

    func testTheAutomaticSwitchDrivesSparkleAndAnOfferWaitsForAClick() {
        let (updates, driver) = make(ScriptedRPC())
        updates.automatic = false
        XCTAssertFalse(driver.automaticallyDownloadsUpdates)
        updates.found(version: "0.28.0")
        XCTAssertEqual(updates.status, .available("0.28.0"))
        XCTAssertEqual(updates.status.line, "Update available: FERB 0.28.0")
        updates.checkNow()
        XCTAssertEqual(driver.checks, 1)
        updates.automatic = true
        updates.found(version: "0.28.1")
        XCTAssertEqual(updates.status, .quiet)                            // Sparkle downloads it silently
        updates.notFound()
        XCTAssertEqual(updates.status, .upToDate)
    }

    func testTheFeedComesOnlyFromTheBuild() {
        let updates = UpdateCoordinator(rpc: ScriptedRPC())
        XCTAssertNil(SparkleUpdater(coordinator: updates, info: [:]))     // no feed in the build: no updater at all
        XCTAssertNil(SparkleUpdater(coordinator: updates, info: ["SUFeedURL": "https://example.org/appcast-arm64.xml",
                                                                  "SUPublicEDKey": "k"]))
    }

    /// U5: a background check or download that fails leaves a line in Settings until the next result, kept across
    /// launches; "no update found" and the owner's own cancel aren't failures.
    // decision: APP-658
    func testAFailedUpdateAttemptIsKeptUntilTheNextResult() {
        let defaults = scratchDefaults("UpdateCoordinatorTests")
        let updates = UpdateCoordinator(rpc: ScriptedRPC(), defaults: defaults)
        updates.restartDaemon = {}
        updates.aborted("The update feed couldn't be reached.")
        XCTAssertEqual(updates.status, .quiet)                            // nothing was armed: only the record says so
        XCTAssertEqual(updates.lastResult?.failed, true)
        let relaunched = UpdateCoordinator(rpc: ScriptedRPC(), defaults: defaults)
        XCTAssertEqual(relaunched.lastResult, updates.lastResult)
        relaunched.notFound()
        XCTAssertEqual(relaunched.lastResult?.failed, false)
        XCTAssertFalse(UpdatePlan.abortIsFailure(domain: "SUSparkleErrorDomain", code: 1001))
        XCTAssertFalse(UpdatePlan.abortIsFailure(domain: "SUSparkleErrorDomain", code: 4007))
        XCTAssertTrue(UpdatePlan.abortIsFailure(domain: "SUSparkleErrorDomain", code: 3001))
    }
}
