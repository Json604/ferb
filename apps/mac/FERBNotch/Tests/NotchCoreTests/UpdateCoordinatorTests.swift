import XCTest
@testable import FERBNotch
@testable import NotchCore

@MainActor
private final class FakeUpdater: UpdaterDriving {
    var automaticallyDownloadsUpdates = true
    /// Sparkle's probing check (no window, no alert) and Sparkle's update window (the owner's Install).
    var quietChecks = 0
    var checks = 0
    var answer: QuietCheck = .started
    func checkQuietly() -> QuietCheck { quietChecks += 1; return answer }
    func installOffered() { checks += 1 }
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
        updates.install()
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

    // decision: APP-903, APP-906
    func testTheSwitchUpdateAsksFirstEvenWithAutomaticUpdatesOn() {
        let updates = UpdateCoordinator(rpc: ScriptedRPC())
        updates.automatic = true
        updates.found(version: "0.28.0")
        XCTAssertEqual(updates.status, .quiet)                       // an ordinary update installs by itself
        XCTAssertNil(updates.offer)
        updates.found(version: "0.28.0", asksFirst: true)            // Sparkle's major upgrade: the signer switch
        XCTAssertEqual(updates.status, .asking("0.28.0"))
        XCTAssertEqual(updates.offer?.version, "0.28.0")
        XCTAssertEqual(updates.offer?.asks, true)
        XCTAssertEqual(updates.status.line, UpdatePlan.askLine(version: "0.28.0"))
        let menu = StatusMenu.entries(state: .ready, paused: false, stopped: false, appName: "FERB", canSetUp: true,
                                      hasWindow: true, updateOffer: "0.28.0", updateAsks: true)
        XCTAssertTrue(menu.contains { if case .item(_, .installUpdate, _) = $0 { return true }; return false })
    }

    /// Owner, 2026-11-17, after Check for Updates… left Sparkle's alert hidden and FERB unable to take a click: "what
    /// if this happens on real user devices??? … fix it". The button runs a quiet check whose answer is Settings' line;
    /// a failed check is said plainly and never reaches Sparkle's window or alert.
    // decision: APP-945, APP-946, APP-947
    func testCheckForUpdatesIsQuietAndAnswersOnSettingsLine() {
        let (updates, driver) = make(ScriptedRPC())
        XCTAssertEqual(updates.button, .check)
        XCTAssertEqual(updates.button.title, "Check for Updates…")

        updates.check()                                                   // the feed answers 404
        XCTAssertEqual(updates.status, .checking)
        XCTAssertEqual(updates.status.line, "Checking for updates…")
        XCTAssertEqual(updates.button, .checking)
        XCTAssertEqual(updates.button.title, "Checking…")
        updates.check()                                                   // a second press while it runs
        XCTAssertEqual(driver.quietChecks, 1)
        updates.aborted("A network error occurred while downloading https://example.org/appcast.xml. not found (404)")
        updates.checkFinished()
        XCTAssertEqual(updates.status, .checkFailed)
        XCTAssertEqual(updates.status.line, "FERB couldn't check for updates just now. It will try again later.")
        XCTAssertEqual(updates.lastResult?.failed, true)
        XCTAssertTrue(updates.lastResult?.text.contains("not found (404)") == true, "the reason stays, small")
        XCTAssertEqual(updates.button, .check)
        XCTAssertEqual(driver.checks, 0, "a check never opens Sparkle's window or its alert")

        updates.check()
        updates.notFound()
        updates.checkFinished()
        XCTAssertEqual(updates.status, .upToDate)
        XCTAssertEqual(updates.status.line, "FERB is up to date.")

        updates.automatic = true
        updates.check()
        updates.found(version: "0.28.0")                                 // the owner asked: offered even when automatic
        updates.checkFinished()
        XCTAssertEqual(updates.status, .available("0.28.0"))
        XCTAssertEqual(updates.status.line, "Update available: FERB 0.28.0")
        XCTAssertEqual(updates.button, .install(title: "Install"))
        XCTAssertEqual(driver.quietChecks, 3)
        XCTAssertEqual(driver.checks, 0)
        for line in [UpdatePlan.checkingLine, UpdatePlan.checkFailedLine, UpdatePlan.installFailedLine] {
            XCTAssertFalse(line.contains("AI"))
            XCTAssertFalse(line.localizedCaseInsensitiveContains("error"))
        }
    }

    /// A scheduled check already running answers the press; a check with no other answer stops saying "Checking"
    /// when Sparkle's session ends; an updater that never started is a plain failure, not a wait.
    // decision: APP-946
    func testACheckAlwaysEndsWithAnAnswer() {
        let (updates, driver) = make(ScriptedRPC())
        driver.answer = .alreadyRunning
        updates.check()
        XCTAssertEqual(updates.status, .checking)
        updates.checkFinished()
        XCTAssertEqual(updates.status, .quiet)
        driver.answer = .unavailable
        updates.check()
        XCTAssertEqual(updates.status, .checkFailed)
        XCTAssertEqual(updates.lastResult?.failed, true)
        let none = UpdateCoordinator(rpc: ScriptedRPC())
        none.check()
        XCTAssertEqual(none.status, .checkFailed)
    }

    /// The owner's Install opens Sparkle's window; a download or signature that fails there closes it without an alert
    /// and FERB says so on Settings' line and once in the chat. A background failure stays in the last result only.
    // decision: APP-947
    func testAFailedInstallTheOwnerChoseIsSaidInFERBsOwnWords() {
        let (updates, driver) = make(ScriptedRPC())
        var said: [String] = []
        updates.onInstallFailed = { said.append($0) }
        updates.aborted("The update feed couldn't be reached.")           // a scheduled check
        XCTAssertEqual(updates.status, .quiet)
        XCTAssertTrue(said.isEmpty)
        updates.automatic = false
        updates.found(version: "0.28.0")
        updates.install()
        XCTAssertEqual(driver.checks, 1)
        XCTAssertTrue(updates.ownerInstalling)
        updates.aborted("The update is improperly signed and could not be validated.")
        updates.checkFinished()
        XCTAssertEqual(updates.status.line, "FERB couldn't install the update just now. It will try again later.")
        XCTAssertEqual(said, [UpdatePlan.installFailedLine])
        XCTAssertFalse(updates.ownerInstalling)
        updates.install()                                                 // nothing offered now: nothing opens
        XCTAssertEqual(driver.checks, 1)
    }

    /// Sparkle wraps the real reason (a 404) in its own "An error occurred while downloading the update"; FERB keeps
    /// the inner one.
    // decision: APP-946
    func testTheReasonKeptIsSparklesInnerOne() {
        let inner = NSError(domain: "SUSparkleErrorDomain", code: 2001,
                            userInfo: [NSLocalizedDescriptionKey: "A network error occurred. not found (404)"])
        let outer = NSError(domain: "SUSparkleErrorDomain", code: 2001,
                            userInfo: [NSLocalizedDescriptionKey: "An error occurred while downloading the update.",
                                       NSUnderlyingErrorKey: inner])
        XCTAssertEqual(UpdatePlan.reason(outer), "A network error occurred. not found (404)")
        XCTAssertEqual(UpdatePlan.reason(inner), "A network error occurred. not found (404)")
    }
}
