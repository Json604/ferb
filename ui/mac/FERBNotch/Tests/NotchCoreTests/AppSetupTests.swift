import AppKit
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// Launch of the packaged FERB.app (L27): what to run, from the old LaunchAgent, the app service's status and setup's record.
final class AppSetupTests: XCTestCase {
    let agentExecutable = "/Applications/FERB.app/Contents/Helpers/FERB.app/Contents/MacOS/FERB"
    let app = "/Applications/FERB.app"

    private func legacy(_ program: String) -> [String: Any] {
        ["Label": "com.ferb.daemon", "ProgramArguments": [program, "-m", "ferb.daemon"]]
    }

    private func decide(legacy agent: [String: Any]? = nil, _ service: ServiceStatus, record: [String: Any]?,
                        build: String = "202610140201", running: Bool = true) -> AppSetupAction {
        AppSetupPlan.decide(legacyAgent: agent, service: service, record: record, agentExecutable: agentExecutable,
                            appPath: app, build: build, daemonRunning: running)
    }

    func testNothingSetUpRunsSetup() {
        XCTAssertEqual(decide(.notRegistered, record: nil), .install)
        XCTAssertEqual(AppSetupPlan.serviceArguments(for: .install), ["setup", "--service"])
    }

    func testThisAppsBuildSetUpAndRunningNeedsNothing() {
        XCTAssertEqual(decide(.enabled, record: ["build": "202610140201", "app": app]), .none)
        XCTAssertNil(AppSetupPlan.serviceArguments(for: .none))
    }

    func testReplacedAppUpdatesWithoutAsking() {
        XCTAssertEqual(decide(.enabled, record: ["build": "202610140201", "app": app], build: "202610150900"), .update)
        XCTAssertEqual(AppSetupPlan.serviceArguments(for: .update), ["setup", "--service", "--update"])
    }

    /// STORE-403: an install from before the app registered its own service has the old user LaunchAgent
    /// running this very app; it is retired by a quiet update, never by a question.
    func testAnOldLaunchAgentOfThisAppIsRetiredByAQuietUpdate() {
        XCTAssertEqual(decide(legacy: legacy(agentExecutable), .notRegistered, record: nil), .update)
        XCTAssertEqual(decide(legacy: legacy(agentExecutable), .enabled, record: ["build": "202610140201", "app": app]),
                       .update)
    }

    func testAnotherInstallationIsReplacedOnlyAfterTheOwnerSaysSo() {
        let action = decide(legacy: legacy("/Users/o/.ferb/venv/bin/python"), .notRegistered, record: nil)
        XCTAssertEqual(action, .confirmReplace(current: "/Users/o/.ferb/venv/bin/python"))
        XCTAssertEqual(AppSetupPlan.serviceArguments(for: action), ["setup", "--service"])
        XCTAssertTrue(AppSetupPlan.needsConfirmation(action))
    }

    func testAServiceTheOwnerTurnedOffOrThatIsntRunningIsRepairedByAnUpdate() {
        let record: [String: Any] = ["build": "202610140201", "app": app]
        XCTAssertEqual(decide(.requiresApproval, record: record), .update)
        XCTAssertEqual(decide(.notRegistered, record: record), .update)
        XCTAssertEqual(decide(.enabled, record: record, running: false), .update)
        XCTAssertEqual(decide(.enabled, record: ["build": "202610140201", "app": "/Users/o/Downloads/FERB.app"]), .update)
        XCTAssertEqual(decide(.enabled, record: nil), .update)
    }

    func testTheServiceProblemsAreSaidInTheAppsWords() {
        XCTAssertNil(ServicePlan.problem(.enabled))
        XCTAssertTrue(ServicePlan.problem(.requiresApproval)?.contains("Login Items") == true)
        XCTAssertTrue(ServicePlan.problem(.notFound)?.contains("Download FERB again") == true)
        XCTAssertEqual(ServicePlan.plistName, "com.ferb.daemon.plist")
    }

    private func needs(done: Bool, signedIn: Bool = true, missing: [String] = [], unpaired: Bool = false,
                       firstRun: Bool = false) -> IntroNeeds {
        IntroNeeds(setupDone: done, signedIn: signedIn, missingPermissions: missing, unpairedBrowsers: unpaired,
                   firstRun: firstRun)
    }

    /// kb area app: a new install (or another installation, after the owner's yes) meets FERB through the
    /// whole tour, with the first-launch clip; so does an install that never finished setup nor saw the tour. A quit
    /// tour picks up its own plan, and the intro's resume wins over the old window's.
    // decision: APP-517, APP-516
    func testNewInstallLaunchesTheIntro() {
        let fresh = needs(done: false, signedIn: false, missing: ["accessibility"], unpaired: true, firstRun: true)
        XCTAssertEqual(AppSetupPlan.atLaunch(.install, needs: fresh), .intro(IntroBeat.allCases))
        XCTAssertEqual(AppSetupPlan.atLaunch(.confirmReplace(current: "/x"), needs: needs(done: true)),
                       .intro(IntroBeat.allCases))
        XCTAssertTrue(AppSetupPlan.firstRun(.install, setupDone: false, introDone: true, resuming: true))
        XCTAssertTrue(AppSetupPlan.firstRun(.confirmReplace(current: "/x"), setupDone: true, introDone: true,
                                            resuming: false))
        XCTAssertTrue(AppSetupPlan.firstRun(.none, setupDone: false, introDone: false, resuming: false))
        XCTAssertEqual(AppSetupPlan.atLaunch(.none, needs: needs(done: false, firstRun: true)), .intro(IntroBeat.allCases))
        // Quit at Screen Recording and reopen: the same tour at the same beat, no clip again.
        XCTAssertFalse(AppSetupPlan.firstRun(.none, setupDone: false, introDone: false, resuming: true))
        XCTAssertEqual(AppSetupPlan.atLaunch(.none, needs: needs(done: false), tour: IntroBeat.allCases,
                                             resume: .permissions), .intro(IntroBeat.allCases))
        XCTAssertEqual(AppSetupPlan.afterUpdate(needs(done: true), tour: [.permissions, .home]),
                       .intro([.permissions, .home]))
        XCTAssertTrue(AppSetupPlan.needsConfirmation(.confirmReplace(current: "/x")))
        XCTAssertFalse(AppSetupPlan.needsConfirmation(.none))
    }

    /// An existing install that has finished setup sees nothing new (Settings offers Replay intro), as before the
    /// intro, even with a grant turned off since; a replaced build still updates quietly first.
    // decision: APP-517
    func testFinishedSetupLaunchesNothing() {
        XCTAssertFalse(AppSetupPlan.firstRun(.none, setupDone: true, introDone: false, resuming: false))
        XCTAssertEqual(AppSetupPlan.atLaunch(.none, needs: needs(done: true)), .nothing)
        XCTAssertEqual(AppSetupPlan.atLaunch(.none, needs: needs(done: true, missing: ["accessibility"])), .nothing)
        XCTAssertEqual(AppSetupPlan.atLaunch(.update, needs: needs(done: true)), .quietUpdate)
        XCTAssertEqual(AppSetupPlan.afterUpdate(needs(done: true)), .nothing)
        XCTAssertEqual(AppSetupPlan.atLaunch(.none, needs: needs(done: false)), .nothing)     // the tour seen, all set
    }

    /// An install missing a grant gets only that beat, then home: after a quiet update even with setup done (the old
    /// window's permissions step), and at launch while setup isn't finished.
    // decision: APP-517, APP-508
    func testMissingGrantLaunchesOnlyThatBeat() {
        let missing = needs(done: true, missing: ["screen_recording"])
        XCTAssertEqual(AppSetupPlan.afterUpdate(missing), .intro([.permissions, .home]))
        XCTAssertEqual(AppSetupPlan.atLaunch(.none, needs: needs(done: false, missing: ["screen_recording"])),
                       .intro([.permissions, .home]))
        XCTAssertEqual(AppSetupPlan.atLaunch(.none, needs: needs(done: false, signedIn: false)), .intro([.signIn, .home]))
        XCTAssertEqual(AppSetupPlan.atLaunch(.update, needs: missing), .quietUpdate)
        // A finished install after an update: Allow all and home only, whatever else a read left unknown or unpaired.
        XCTAssertEqual(AppSetupPlan.afterUpdate(needs(done: true, signedIn: false, missing: ["screen_recording"],
                                                      unpaired: true)), .intro([.permissions, .home]))
        XCTAssertEqual(AppSetupPlan.afterUpdate(needs(done: true, signedIn: false, unpaired: true)), .nothing)
    }

    /// Replay intro (Settings → General, and the menu's Set Up FERB…): every beat, whatever is set up already.
    // decision: APP-516, APP-505
    func testReplayFromSettingsRunsEveryBeat() {
        XCTAssertEqual(AppSetupPlan.replay(needs(done: true)), .intro(IntroBeat.allCases))
        XCTAssertEqual(AppSetupPlan.replay(needs(done: false, missing: ["accessibility"])), .intro(IntroBeat.allCases))
    }

    /// Skipping (or a grant still off at Home) opens the compact setup window on the first required step left:
    /// the background service, then sign-in, then permissions; nothing left opens nothing.
    // decision: APP-516
    func testSkippingTheIntroOpensTheCompactSetupOnWhatsLeft() {
        XCTAssertEqual(AppSetupPlan.afterIntro(["sign_in", "accessibility"], serviceSetUp: true), .ai)
        XCTAssertEqual(AppSetupPlan.afterIntro(["accessibility"], serviceSetUp: true), .permissions)
        XCTAssertNil(AppSetupPlan.afterIntro([], serviceSetUp: true))
        XCTAssertEqual(AppSetupPlan.afterIntro(["sign_in"], serviceSetUp: false), .welcome)
    }

    /// A quit tour resumes with the beats it was running (a replay's, an upgrade's, a first run's).
    // decision: APP-516
    func testAQuitTourResumesWithItsOwnPlan() {
        var progress = IntroProgress(defaults: scratchDefaults("AppSetupTests"))
        XCTAssertNil(progress.interrupted)
        progress.plan = [.permissions, .home]
        XCTAssertNil(progress.interrupted)                          // finished: nothing to resume
        progress.resumeBeat = .permissions
        XCTAssertEqual(progress.interrupted, [.permissions, .home])
        progress.plan = nil
        XCTAssertEqual(progress.interrupted, IntroBeat.allCases)
        progress.clear()
        XCTAssertNil(progress.interrupted)
    }

    /// Owner 2026-10-26: after Screen Recording's "Quit & Reopen" FERB came back without its setup window (setup counted
    /// as done from an earlier install). A Grant in progress in the compact window reopens it on that step, done or not.
    func testLaunchReopensTheStepAGrantRestartedFrom() {
        XCTAssertEqual(AppSetupPlan.atLaunch(.none, needs: needs(done: true), resume: .permissions), .window(.permissions))
        XCTAssertEqual(AppSetupPlan.atLaunch(.none, needs: needs(done: false), resume: .permissions),
                       .window(.permissions))
        XCTAssertEqual(AppSetupPlan.atLaunch(.none, needs: needs(done: true), resume: nil), .nothing)
        XCTAssertEqual(AppSetupPlan.atLaunch(.update, needs: needs(done: true), resume: .permissions), .quietUpdate)
    }

    /// Restart from the menu with no service to restart repairs it in the setup window; it never replays the tour.
    @MainActor
    func testAFailedRestartNeverStartsATour() async {
        _ = NSApplication.shared
        let coordinator = PanelCoordinator()
        coordinator.magneticFieldPresentationEnabled = false
        defer { coordinator.tearDown() }
        let service = FakeService()
        service.result = false
        coordinator.service = service
        var toured = false
        var repaired = false
        coordinator.onSetUp = { toured = true }
        coordinator.onRepair = { repaired = true }
        coordinator.perform(.restart)
        await eventually { repaired }
        XCTAssertEqual(service.restarts, 1)
        XCTAssertFalse(toured)
    }

    /// Restart's repair while a tour runs: the tour ends through its own skip (orb, sound and lines torn down) and
    /// exactly one compact window shows, on Welcome, where the tour's end leaves it.
    @MainActor
    // decision: APP-516
    func testARepairDuringATourEndsItAndShowsOneWindowOnWelcome() async {
        _ = NSApplication.shared
        let rig = SetupRig()
        let defaults = scratchDefaults("AppSetupTests")
        let controller = AppSetupController(model: rig.model(), defaults: defaults)
        let orb = FakeOrb()
        let sound = FakeSoundPlayer()
        controller.makeTourParts = { (orb, sound) }
        controller.notch = NotchModel(rpc: rig.rpc, preferences: defaults)
        controller.introControls = FakeControlFeed()
        await controller.launch(.intro(IntroBeat.allCases), action: .none, needs: needs(done: false))
        XCTAssertTrue(controller.touring)
        controller.repair(.none)            // a service set up but not starting: the tour's end would pick sign-in
        await eventually { !controller.touring }
        XCTAssertEqual(orb.tearDowns, 1)
        XCTAssertEqual(sound.warm.last, false)
        XCTAssertNil(controller.notch?.introLine)
        let windows = NSApp.windows.filter { $0.delegate === controller && $0.isVisible }
        XCTAssertEqual(windows.count, 1)
        XCTAssertEqual(controller.model.step, .welcome)
        controller.setupWindow?.close()
    }

    /// The quiet update's decision reads its needs here: a required grant is missing as in Allow all (FERB.app's own
    /// rows included, a blocked row never), and a fact the daemon didn't answer asks for nothing.
    @MainActor
    // decision: APP-509, APP-508
    func testTheNeedsCountOnlyGrantsThatCanBeGrantedNow() async {
        let rig = SetupRig()
        rig.app.notificationStatus = .granted
        rig.app.microphoneStatus = .granted
        let controller = AppSetupController(model: rig.model(), defaults: scratchDefaults("AppSetupTests"))
        var read = controller.snapshotNeeds(setupDone: true, firstRun: false)
        XCTAssertEqual(read.missingPermissions, [])                 // nothing read yet: nothing missing
        XCTAssertTrue(read.signedIn)
        XCTAssertFalse(read.unpairedBrowsers)
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(accessibility: "granted", fda: "granted")
        await controller.model.refreshPermissions()
        XCTAssertEqual(controller.snapshotNeeds(setupDone: true, firstRun: false).missingPermissions, [])
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(accessibility: "denied", fda: "granted")
        await controller.model.refreshPermissions()
        read = controller.snapshotNeeds(setupDone: true, firstRun: false)
        XCTAssertEqual(read.missingPermissions, ["accessibility"])
        XCTAssertEqual(AppSetupPlan.afterUpdate(read), .intro([.permissions, .home]))
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(accessibility: "granted", fda: "denied",
                                                                        fdaBlocked: "not yet")
        await controller.model.refreshPermissions()
        XCTAssertEqual(controller.snapshotNeeds(setupDone: true, firstRun: false).missingPermissions, [])
        rig.app.microphoneStatus = .notDetermined                   // every permission is required, FERB.app's too
        await controller.model.refreshPermissions()
        XCTAssertEqual(controller.snapshotNeeds(setupDone: true, firstRun: false).missingPermissions, ["microphone"])
    }

    /// A finished launch reads whether setup is done and nothing else.
    @MainActor
    func testAFinishedLaunchReadsOnlyWhatItsDecisionNeeds() async {
        let rig = SetupRig()
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(setupDone: true)
        let controller = AppSetupController(model: rig.model(), defaults: scratchDefaults("AppSetupTests"))
        var settled = false
        controller.onSettled = { settled = true }
        await controller.decideAtLaunch(.none)
        XCTAssertEqual(rig.rpc.methods(), ["setup.permissions"])
        XCTAssertTrue(settled)
    }

    /// The tour is behind it and every grant is on, but setup was never recorded: launch records it quietly (the
    /// daemon checks every row again), and leaves no "isn't answering" line from a slow read behind.
    @MainActor
    func testANothingLaunchRecordsAnUnrecordedFinishedSetup() async {
        let rig = SetupRig()
        rig.app.notificationStatus = .granted
        rig.app.microphoneStatus = .granted
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(accessibility: "granted", fda: "granted")
        rig.rpc.failures["runtimes.status"] = DaemonRPCError.unavailable
        rig.rpc.failures["setup.browsers"] = DaemonRPCError.unavailable
        let defaults = scratchDefaults("AppSetupTests")
        var progress = IntroProgress(defaults: defaults)
        progress.done = true
        let controller = AppSetupController(model: rig.model(), defaults: defaults)
        var settled = false
        controller.onSettled = { settled = true }
        await controller.decideAtLaunch(.none)
        XCTAssertTrue(rig.rpc.methods().contains("setup.finish"))
        XCTAssertTrue(settled)
        XCTAssertNil(controller.model.message)

        // Never recorded without the tour behind it, or without the grants read.
        let unseen = SetupRig()
        let fresh = AppSetupController(model: unseen.model(), defaults: scratchDefaults("AppSetupTests"))
        await fresh.launch(.nothing, action: .none, needs: needs(done: false))
        XCTAssertFalse(unseen.rpc.methods().contains("setup.finish"))
        let unread = SetupRig()
        let seen = scratchDefaults("AppSetupTests")
        var seenProgress = IntroProgress(defaults: seen)
        seenProgress.done = true
        await AppSetupController(model: unread.model(), defaults: seen).launch(.nothing, action: .none,
                                                                               needs: needs(done: false))
        XCTAssertFalse(unread.rpc.methods().contains("setup.finish"))
    }

    func testSignInToCodexRunsFERBsOwnCodexLogin() {
        XCTAssertEqual(AppSetupPlan.codexLoginArguments, ["setup", "--codex-login"])
    }

    func testConsoleKeepsCharactersSplitAcrossReads() {
        var console = ConsoleDecoder()
        let bytes = Array("✓ granted\n".utf8)                   // ✓ is three bytes
        XCTAssertEqual(console.append(Data(bytes[0..<2])), "")
        XCTAssertEqual(console.append(Data(bytes[2...])), "✓ granted\n")
        XCTAssertEqual(console.append(Data("Press Enter: ".utf8)), "Press Enter: ")   // prompts end without a newline
    }

    func testConsoleReplacesInvalidBytesInsteadOfStalling() {
        var console = ConsoleDecoder()
        XCTAssertEqual(console.append(Data([0x66, 0xFF, 0xFE, 0x67])), "f\u{FFFD}\u{FFFD}g")
    }
}
