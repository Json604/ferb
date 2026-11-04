import AppKit
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// Launch of the packaged FERB.app (L27): what to run, from the user LaunchAgent (DMN-513), a 2026-11 build's
/// SMAppService registration and setup's record.
final class AppSetupTests: XCTestCase {
    let agentExecutable = "/Applications/FERB.app/Contents/Helpers/FERB.app/Contents/MacOS/FERB"
    let app = "/Applications/FERB.app"
    let digest = "d1"

    private func agent(_ arguments: [String]) -> [String: Any] {
        ["Label": "com.ferb.daemon", "ProgramArguments": arguments]
    }

    private var ours: [String: Any] { agent([agentExecutable, "-m", "ferb.daemon"]) }

    private func record(build: String = "202610140201", app: String? = nil, definition: String? = nil) -> [String: Any] {
        ["build": build, "app": app ?? self.app, "label": "com.ferb.daemon", "definition": definition ?? digest]
    }

    private func decide(_ launchAgent: [String: Any]?, digest: String? = nil, _ registration: ServiceStatus = .notRegistered,
                        record: [String: Any]?, build: String = "202610140201", running: Bool = true) -> AppSetupAction {
        AppSetupPlan.decide(launchAgent: launchAgent, launchAgentDigest: launchAgent == nil ? nil : digest ?? self.digest,
                            registration: registration, record: record, agentExecutable: agentExecutable,
                            appPath: app, build: build, daemonRunning: running)
    }

    func testNothingSetUpRunsSetup() {
        XCTAssertEqual(decide(nil, record: nil), .install)
        XCTAssertEqual(AppSetupPlan.serviceArguments(for: .install), ["setup", "--service"])
    }

    // decision: DMN-513
    func testThisAppsBuildSetUpAndRunningNeedsNothing() {
        XCTAssertEqual(decide(ours, record: record()), .none)
        XCTAssertNil(AppSetupPlan.serviceArguments(for: .none))
    }

    /// Build A, then build B over it: B's launch updates quietly (the setup command rewrites and reloads the plist).
    // decision: DMN-513
    func testAReplacedAppUpdatesWithoutAsking() {
        XCTAssertEqual(decide(ours, record: record(), build: "202610150900"), .update)
        XCTAssertEqual(AppSetupPlan.serviceArguments(for: .update), ["setup", "--service", "--update"])
    }

    /// A friend's Mac: the LaunchAgent an older FERB wrote (an older build's agent at another path, or ~/.ferb/venv's
    /// python without -P) is replaced by a quiet update, never by a question.
    // decision: DMN-513, APP-602
    func testAnOlderInstallsLaunchAgentIsReplacedByAQuietUpdate() {
        let olderApp = agent(["/Users/o/Downloads/FERB.app/Contents/Helpers/FERB.app/Contents/MacOS/FERB", "-m",
                              "ferb.daemon"])
        let venv = agent(["/Users/o/.ferb/venv/bin/python", "-m", "ferb.daemon"])
        for older in [olderApp, venv, ours] {
            XCTAssertEqual(decide(older, record: nil), .update)
        }
        XCTAssertEqual(decide(venv, digest: "other", record: record()), .update)
    }

    /// The owner's Mac: a 2026-11 build left an SMAppService registration (and a record, but no LaunchAgent).
    // decision: DMN-513, APP-602
    func testA2026_11BuildsRegistrationIsMovedByAQuietUpdate() {
        XCTAssertEqual(decide(nil, .enabled, record: ["build": "202611020100", "app": app, "label": "com.ferb.daemon"]),
                       .update)
        XCTAssertEqual(decide(nil, .requiresApproval, record: nil), .update)
        XCTAssertEqual(decide(ours, .enabled, record: record()), .update)     // set up, but still listed
    }

    /// Rule 6 (DMN-514): a LaunchAgent that isn't what setup wrote (its digest differs from the record's) is set up again.
    // decision: DMN-514
    func testAChangedLaunchAgentIsSetUpAgain() {
        XCTAssertEqual(decide(ours, digest: "tampered", record: record()), .update)
    }

    // decision: APP-602
    func testACheckoutsLaunchAgentIsReplacedOnlyAfterTheOwnerSaysSo() {
        let checkout = agent(["/Users/o/.ferb/venv/bin/python", "-P", "-m", "ferb.daemon"])
        let action = decide(checkout, record: record())
        XCTAssertEqual(action, .confirmReplace(current: "/Users/o/.ferb/venv/bin/python"))
        XCTAssertEqual(AppSetupPlan.serviceArguments(for: action), ["setup", "--service"])
        XCTAssertTrue(AppSetupPlan.needsConfirmation(action))
    }

    func testAServiceThatIsntRunningIsRepairedByAnUpdate() {
        XCTAssertEqual(decide(ours, record: record(), running: false), .update)
        XCTAssertEqual(decide(ours, record: record(app: "/Users/o/Downloads/FERB.app")), .update)
        XCTAssertEqual(decide(nil, record: record()), .update)               // the LaunchAgent was deleted
    }

    // decision: DMN-513
    func testLaunchdsSwitchedOffListIsRead() {
        XCTAssertTrue(LaunchctlPlan.disabled("disabled services = {\n\t\"com.apple.x\" => enabled\n" +
                                             "\t\"com.ferb.daemon\" => disabled\n}\n"))
        XCTAssertTrue(LaunchctlPlan.disabled("\t\"com.ferb.daemon\" => true\n"))
        XCTAssertFalse(LaunchctlPlan.disabled("\t\"com.ferb.daemon\" => enabled\n\t\"com.ferb.daemon.x\" => disabled\n"))
        XCTAssertFalse(LaunchctlPlan.disabled(""))
        XCTAssertEqual(ServicePlan.plistName, "com.ferb.daemon.plist")
        XCTAssertEqual(AppSetupPlan.digest(Data("abc".utf8)),
                       "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
    }

    private func needs(done: Bool, signedIn: Bool = true, missing: [String] = [], unpaired: Bool = false,
                       firstRun: Bool = false) -> IntroNeeds {
        IntroNeeds(setupDone: done, signedIn: signedIn, missingPermissions: missing, unpairedBrowsers: unpaired,
                   firstRun: firstRun)
    }

    /// kb area app: a new install (or another installation, after the owner's yes) meets FERB through the
    /// whole tour, with the first-launch clip; so does an install that never finished setup nor saw the tour. A quit
    /// tour picks up its own plan, and the intro's resume wins over the old window's.
    // decision: APP-635, APP-638
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
    /// intro; a grant turned off since gets its beats plus Home (APP-657); a replaced build still updates quietly first.
    // decision: APP-635, APP-657
    func testFinishedSetupLaunchesNothing() {
        XCTAssertFalse(AppSetupPlan.firstRun(.none, setupDone: true, introDone: false, resuming: false))
        XCTAssertEqual(AppSetupPlan.atLaunch(.none, needs: needs(done: true)), .nothing)
        XCTAssertEqual(AppSetupPlan.atLaunch(.none, needs: needs(done: true, missing: ["accessibility"])),
                       .intro([.permissions, .home]))
        XCTAssertEqual(AppSetupPlan.atLaunch(.update, needs: needs(done: true)), .quietUpdate)
        XCTAssertEqual(AppSetupPlan.afterUpdate(needs(done: true)), .nothing)
        XCTAssertEqual(AppSetupPlan.atLaunch(.none, needs: needs(done: false)), .nothing)     // the tour seen, all set
    }

    /// An install that finished setup on an older FERB plays this version of the tour once, as a first run (clip and
    /// every beat), after its quiet update; once it has started the whole tour, launches settle as before. A tour a
    /// quit interrupted still resumes where it was.
    // decision: APP-635
    func testAnUpgradeFromAnOlderTourPlaysTheWholeTourOnce() {
        XCTAssertTrue(AppSetupPlan.firstRun(.update, setupDone: true, introDone: true, resuming: false, tourSeen: false))
        XCTAssertTrue(AppSetupPlan.firstRun(.none, setupDone: true, introDone: true, resuming: false, tourSeen: false))
        XCTAssertFalse(AppSetupPlan.firstRun(.none, setupDone: true, introDone: true, resuming: true, tourSeen: false))
        XCTAssertFalse(AppSetupPlan.firstRun(.none, setupDone: true, introDone: true, resuming: false, tourSeen: true))
        let upgraded = needs(done: true, missing: ["screen_recording"], firstRun: true)
        XCTAssertEqual(AppSetupPlan.atLaunch(.update, needs: upgraded), .quietUpdate)
        XCTAssertEqual(AppSetupPlan.afterUpdate(upgraded), .intro(IntroBeat.allCases))
        XCTAssertEqual(AppSetupPlan.atLaunch(.none, needs: needs(done: true, firstRun: true)), .intro(IntroBeat.allCases))
        XCTAssertEqual(AppSetupPlan.afterUpdate(upgraded, tour: [.permissions, .home]), .intro([.permissions, .home]))

        let defaults = scratchDefaults("TourVersion")
        var progress = IntroProgress(defaults: defaults)
        XCTAssertFalse(progress.tourSeen)                                   // an install from before the marker
        defaults.set(IntroProgress.currentTour - 1, forKey: IntroProgress.tourKey)
        XCTAssertFalse(progress.tourSeen)                                   // an older tour
        progress.tourSeen = true
        XCTAssertTrue(IntroProgress(defaults: defaults).tourSeen)
    }

    /// An install missing a grant gets only that beat, then home: after a quiet update even with setup done (the old
    /// window's permissions step), and at launch while setup isn't finished.
    // decision: APP-635, APP-508
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
    // decision: APP-638, APP-505
    func testReplayFromSettingsRunsEveryBeat() {
        XCTAssertEqual(AppSetupPlan.replay(needs(done: true)), .intro(IntroBeat.allCases))
        XCTAssertEqual(AppSetupPlan.replay(needs(done: false, missing: ["accessibility"])), .intro(IntroBeat.allCases))
    }

    /// Skipping (or a grant still off at Home) opens the compact setup window on the first required step left:
    /// the background service, then sign-in, then permissions; nothing left opens nothing.
    // decision: APP-638
    func testSkippingTheIntroOpensTheCompactSetupOnWhatsLeft() {
        XCTAssertEqual(AppSetupPlan.afterIntro(["sign_in", "accessibility"], serviceSetUp: true), .ai)
        XCTAssertEqual(AppSetupPlan.afterIntro(["accessibility"], serviceSetUp: true), .permissions)
        XCTAssertNil(AppSetupPlan.afterIntro([], serviceSetUp: true))
        XCTAssertEqual(AppSetupPlan.afterIntro(["sign_in"], serviceSetUp: false), .welcome)
    }

    /// A quit tour resumes with the beats it was running (a replay's, an upgrade's, a first run's).
    // decision: APP-638
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
    // decision: APP-595
    func testLaunchReopensTheStepAGrantRestartedFrom() {
        XCTAssertEqual(AppSetupPlan.atLaunch(.none, needs: needs(done: true), resume: .permissions), .window(.permissions))
        XCTAssertEqual(AppSetupPlan.atLaunch(.none, needs: needs(done: false), resume: .permissions),
                       .window(.permissions))
        XCTAssertEqual(AppSetupPlan.atLaunch(.none, needs: needs(done: true), resume: nil), .nothing)
        XCTAssertEqual(AppSetupPlan.atLaunch(.update, needs: needs(done: true), resume: .permissions), .quietUpdate)
    }

    /// Restart's repair while a tour runs: the tour ends through its own skip (orb, sound and lines torn down) and
    /// exactly one compact window shows, on Welcome, where the tour's end leaves it.
    @MainActor
    // decision: APP-638
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

    /// U3/D4: with setup recorded done, a grant switched off since brings back Allow all and home at the next launch
    /// (a first run still gets the whole tour). U2: a Grant from Settings that succeeded leaves no setup window to
    /// reopen; one in an unfinished setup still resumes (APP-595).
    @MainActor
    // decision: APP-657
    func testALaunchNoticesAGrantSwitchedOffAndForgetsAGrantThatSucceeded() async {
        XCTAssertEqual(AppSetupPlan.atLaunch(.none, needs: needs(done: true, missing: ["accessibility"])),
                       .intro([.permissions, .home]))
        XCTAssertEqual(AppSetupPlan.atLaunch(.none, needs: needs(done: true, missing: ["accessibility"]),
                                             resume: .permissions), .intro([.permissions, .home]))
        XCTAssertEqual(AppSetupPlan.atLaunch(.none, needs: needs(done: true, missing: ["accessibility"], firstRun: true)),
                       .intro(IntroBeat.allCases))
        let rig = SetupRig()
        rig.app.notificationStatus = .granted
        rig.app.microphoneStatus = .granted
        let defaults = scratchDefaults("AppSetupTests")
        let controller = AppSetupController(model: rig.model(), defaults: defaults)
        defaults.set(SetupStep.permissions.rawValue, forKey: SetupModel.resumeKey)      // a Grant was pressed
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(accessibility: "granted", fda: "granted")
        await controller.model.refreshPermissions()
        XCTAssertEqual(SetupModel.pendingResume(defaults), .permissions)                 // setup not done: it resumes
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(accessibility: "granted", fda: "granted",
                                                                        setupDone: true)
        await controller.model.refreshPermissions()
        XCTAssertNil(SetupModel.pendingResume(defaults))
    }
}
