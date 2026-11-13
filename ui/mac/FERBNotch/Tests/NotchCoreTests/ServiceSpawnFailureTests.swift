import XCTest
@testable import FERBNotch
import NotchCore

/// launchd can't start FERB's background service (live 2026-11-14): FERB.app's in-app registration (DMN-587) had been
/// tied to the Background Task Management record of the user LaunchAgent it replaced; once macOS dropped that record,
/// every start failed (`job state = spawn failed`, `last exit code = 78: EX_CONFIG`) and the tour waited on Input
/// Monitoring for good after its restart-to-check. These cover the states launchd prints and what FERB.app does in
/// each: nothing while the job runs or only waits for its throttle; a fresh registration once; then the LaunchAgent.
@MainActor
final class ServiceSpawnFailureTests: XCTestCase {
    private let failedPrint = """
        gui/501/com.ferb.daemon = {
        \tactive count = 0
        \tstate = spawn scheduled
        \tprogram identifier = Contents/Helpers/FERB.app/Contents/MacOS/FERB (mode: 2)
        \tparent bundle identifier = com.ferb.FERB
        \truns = 21
        \tlast exit code = 78: EX_CONFIG
        \tjob state = spawn failed
        }
        """

    // decision: DMN-591
    func testOnlyAJobLaunchdSaysItCantStartIsAFailure() {
        XCTAssertEqual(LaunchctlPlan.spawnFailure(failedPrint),
                       "launchd couldn't start it (last exit code 78: EX_CONFIG)")
        let running = "\tstate = running\n\tpid = 74203\n\tlast exit code = (never exited)\n\tjob state = running\n"
        XCTAssertNil(LaunchctlPlan.spawnFailure(running))
        // waiting out its throttle after a crash, or a clean exit nobody restarts: not launchd failing to start it
        XCTAssertNil(LaunchctlPlan.spawnFailure("\tstate = not running\n\tlast exit code = 1\n\tjob state = exited\n"))
        XCTAssertNil(LaunchctlPlan.spawnFailure(""))                  // launchd has no such job, or printed nothing
        XCTAssertEqual(LaunchctlPlan.spawnFailure("\tjob state = spawn failed\n"), "launchd couldn't start it")
        XCTAssertNil(LaunchctlPlan.spawnFailure("\tpid = 5\n\tjob state = spawn failed\n"))   // it runs: no failure
    }

    private func rig(_ background: FakeBackgroundService, appService: Bool = true) -> (SetupRig, SetupModel) {
        let rig = SetupRig()
        let model = rig.model(.update, background: background)
        model.appService = appService
        model.serviceStartWait = .seconds(20)          // a repair must never need the whole wait
        model.serviceWaitPoll = .milliseconds(5)
        model.serviceCheckAfterStart = .seconds(2)
        return (rig, model)
    }

    // decision: DMN-591
    func testARegistrationLaunchdCantStartIsRegisteredAgainAtOnceAndThenAnswers() async {
        let background = FakeBackgroundService()
        background.spawnFails = "launchd couldn't start it (last exit code 78: EX_CONFIG)"
        background.failingRegistrations = 1                          // the first registration stays broken
        let (rig, model) = rig(background)
        rig.rpc.failures["ping"] = DaemonRPCError.unavailable
        background.onRegister = { if background.spawnFails == nil { rig.rpc.failures["ping"] = nil } }
        let began = ContinuousClock.now
        var result: Bool?
        model.runService { result = $0 }
        await eventually { rig.runner.isRunning }
        rig.runner.finish(0)
        await eventually { result != nil }
        XCTAssertEqual(result, true)
        XCTAssertEqual(background.steps, ["register", "unregister", "register"])
        XCTAssertNil(model.serviceFallback)
        XCTAssertEqual(rig.runner.runs.count, 1)                     // no LaunchAgent: the app's own service runs
        XCTAssertLessThan(ContinuousClock.now - began, .seconds(10))
    }

    // decision: DMN-591, DMN-513
    func testARegistrationLaunchdStillCantStartFallsBackToTheLaunchAgentWithLaunchdsWords() async {
        let background = FakeBackgroundService()
        background.spawnFails = "launchd couldn't start it (last exit code 78: EX_CONFIG)"
        background.failingRegistrations = 99
        let (rig, model) = rig(background)
        rig.rpc.failures["ping"] = DaemonRPCError.unavailable
        let began = ContinuousClock.now
        var result: Bool?
        model.runService { result = $0 }
        await eventually { rig.runner.isRunning }
        rig.runner.finish(0)
        await eventually { rig.runner.runs.count == 2 }
        XCTAssertEqual(rig.runner.runs.last, ["setup", "--service", "--update"])
        XCTAssertEqual(model.serviceFallback, "launchd couldn't start it (last exit code 78: EX_CONFIG)")
        XCTAssertLessThan(ContinuousClock.now - began, .seconds(10))  // not the 20 s wait
        rig.runner.finish(0)
        await eventually { result != nil }
        XCTAssertEqual(result, true)
    }

    // decision: DMN-591, APP-662
    func testARestartToCheckAGrantThatLaunchdCantStartIsRepairedNotWaitedOn() async {
        let background = FakeBackgroundService()
        background.registration = .enabled
        background.status = .enabled
        let (rig, model) = rig(background)
        rig.rpc.failures["ping"] = DaemonRPCError.unavailable
        rig.service.onRestart = {                                    // kickstart -k: the old daemon goes, and the
            background.spawnFails = "launchd couldn't start it"     // new one never starts
            background.failingRegistrations = 0
        }
        background.onRegister = { if background.spawnFails == nil { rig.rpc.failures["ping"] = nil } }
        await model.restartService()
        XCTAssertEqual(rig.service.restarts, 1)
        XCTAssertEqual(background.steps, ["unregister", "register"])  // registered again, then FERB answered
        XCTAssertNil(model.message)                                  // no "couldn't be restarted"
        XCTAssertNil(model.serviceFallback)
    }

    // decision: DMN-591, DMN-513
    func testALaunchAgentLaunchdCantStartIsInstalledAgain() async {
        let background = FakeBackgroundService()                     // nothing registered: the LaunchAgent runs it
        let (rig, model) = rig(background, appService: false)
        rig.rpc.failures["ping"] = DaemonRPCError.unavailable
        rig.service.onRestart = { background.spawnFails = "launchd couldn't start it (last exit code 78: EX_CONFIG)" }
        let restart = Task { await model.restartService() }
        await eventually { rig.runner.isRunning }
        XCTAssertEqual(rig.runner.runs.last, ["setup", "--service", "--update"])
        XCTAssertEqual(background.registrations, 0)
        rig.rpc.failures["ping"] = nil
        rig.runner.finish(0)
        await restart.value
        XCTAssertNil(model.message)
    }

    // decision: DMN-591
    func testARestartLaunchdCompletesIsLeftAlone() async {
        let background = FakeBackgroundService()
        background.registration = .enabled
        background.status = .enabled
        let (rig, model) = rig(background)
        await model.restartService()                                 // FERB answers again: nothing to repair
        XCTAssertEqual(rig.service.restarts, 1)
        XCTAssertTrue(background.steps.isEmpty)
        XCTAssertTrue(rig.runner.runs.isEmpty)
        XCTAssertNil(model.message)
    }
}
