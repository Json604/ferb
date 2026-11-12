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
    // decision: DMN-513, APP-904
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
    // decision: DMN-513, APP-904
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

    // decision: APP-904
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
    // decision: APP-635, APP-685
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
        XCTAssertEqual(AppSetupPlan.atLaunch(.none, needs: needs(done: false), tour: IntroBeat.allCases),
                       .intro(IntroBeat.allCases))
        XCTAssertEqual(AppSetupPlan.afterUpdate(needs(done: true), tour: [.permissions, .home]),
                       .intro([.permissions, .home]))
        XCTAssertTrue(AppSetupPlan.needsConfirmation(.confirmReplace(current: "/x")))
        XCTAssertFalse(AppSetupPlan.needsConfirmation(.none))
    }

    /// An existing install that has finished setup sees nothing new (Settings offers Replay intro), as before the
    /// intro; a grant turned off since gets its beats plus Home (APP-686); a replaced build still updates quietly first.
    // decision: APP-635, APP-686
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
    // decision: APP-685, APP-668
    func testReplayFromSettingsRunsEveryBeat() {
        XCTAssertEqual(AppSetupPlan.replay(needs(done: true)), .intro(IntroBeat.allCases))
        XCTAssertEqual(AppSetupPlan.replay(needs(done: false, missing: ["accessibility"])), .intro(IntroBeat.allCases))
    }

    /// A quit tour resumes with the beats it was running (a replay's, an upgrade's, a first run's).
    // decision: APP-685
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

    /// Owner 2026-11-04: macOS's Quit & Reopen during Full Disk Access, then FERB opened by hand, showed the old compact
    /// setup window ("why is this even here??? isnt it notch only??") instead of resuming the tour. A launch with a tour
    /// a quit interrupted resumes that tour at its beat, setup done or not; nothing launches a window.
    // decision: APP-685, APP-686
    func testALaunchAfterAQuitResumesTheTourNeverAWindow() {
        XCTAssertEqual(AppSetupPlan.atLaunch(.none, needs: needs(done: false), tour: [.permissions, .home]),
                       .intro([.permissions, .home]))
        XCTAssertEqual(AppSetupPlan.atLaunch(.none, needs: needs(done: true), tour: [.permissions, .home]),
                       .intro([.permissions, .home]))
        XCTAssertEqual(AppSetupPlan.atLaunch(.none, needs: needs(done: true)), .nothing)
        XCTAssertEqual(AppSetupPlan.atLaunch(.update, needs: needs(done: false), tour: [.permissions]), .quietUpdate)
    }

    /// Live 2026-11-04: the launch's quiet update failed ("restarting the daemon failed:") and the compact window came
    /// up. Now the failure is the tour's service line in the notch, with the reason and Try again, no window; once the
    /// service is set up the launch resumes the tour a quit interrupted, at its beat.
    @MainActor
    // decision: APP-685, APP-686
    func testAFailedQuietUpdateShowsInTheNotchAndThenResumesTheTour() async {
        _ = NSApplication.shared
        let rig = SetupRig()
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions()
        rig.rpc.replies["runtimes.status"] = IntroControllerTests.connected     // signed in: sign-in isn't run again
        let defaults = scratchDefaults("AppSetupTests")
        var progress = IntroProgress(defaults: defaults)
        progress.plan = IntroBeat.allCases                          // a tour Quit & Reopen ended at the permissions beat
        progress.resumeBeat = .permissions
        let controller = AppSetupController(model: rig.model(), defaults: defaults)
        controller.makeTourParts = { (FakeOrb(), FakeSoundPlayer()) }
        controller.makeArrival = { nil }                            // a newer build replays the startup (APP-645)
        controller.introSleeper = FakeIntroSleeper()
        let notch = NotchModel(rpc: rig.rpc, preferences: defaults)
        controller.notch = notch
        controller.introControls = FakeControlFeed()
        let windowsBefore = Set(NSApp.windows.map(ObjectIdentifier.init))
        await controller.launch(.quietUpdate, action: .update, needs: needs(done: false))
        XCTAssertEqual(rig.runner.runs.last, ["setup", "--service", "--update"])
        rig.runner.say("ferb: restarting the daemon failed (launchctl exited 113: Could not find service)")
        rig.runner.finish(1)
        await eventually { notch.introLine?.choices.contains { $0.id == IntroCopy.Pick.retry } == true }
        XCTAssertTrue(notch.introLine?.detail?.contains("launchctl exited 113") == true)    // the real reason
        XCTAssertTrue(controller.touring)
        XCTAssertTrue(NSApp.windows.filter { !windowsBefore.contains(ObjectIdentifier($0)) && $0.isVisible }.isEmpty)
        notch.onIntroAction?(.choose(IntroCopy.Pick.retry))         // Try again
        await eventually { rig.runner.runs.count == 2 }
        rig.runner.finish(0)
        await eventually { notch.introSkippable }                   // the tour resumed at its permissions beat
        XCTAssertEqual(IntroProgress(defaults: defaults).resumeBeat, .permissions)
        XCTAssertTrue(NSApp.windows.filter { !windowsBefore.contains(ObjectIdentifier($0)) && $0.isVisible }.isEmpty)
        notch.onIntroAction?(.skipTour)
        await eventually { !controller.touring }
    }

    /// Restart's repair while a tour runs: the tour is interrupted where it is (orb, sound and lines torn down, its
    /// beat kept) and the service step shows alone in the notch; no window.
    @MainActor
    // decision: APP-685
    func testARepairDuringATourKeepsItsPlaceAndShowsTheServiceInTheNotch() async {
        _ = NSApplication.shared
        let rig = SetupRig()
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions()
        let defaults = scratchDefaults("AppSetupTests")
        let controller = AppSetupController(model: rig.model(), defaults: defaults)
        let orb = FakeOrb()
        let sound = FakeSoundPlayer()
        controller.makeTourParts = { (orb, sound) }
        let notch = NotchModel(rpc: rig.rpc, preferences: defaults)
        controller.notch = notch
        controller.introControls = FakeControlFeed()
        var progress = IntroProgress(defaults: defaults)
        progress.resumeBeat = .permissions
        await controller.launch(.intro([.permissions, .home]), action: .none, needs: needs(done: false))
        await eventually { notch.introSkippable }
        let windowsBefore = Set(NSApp.windows.map(ObjectIdentifier.init))
        controller.repair(.none)            // a service set up but not starting: set up again
        XCTAssertEqual(orb.tearDowns, 1)
        XCTAssertEqual(IntroProgress(defaults: defaults).resumeBeat, .permissions)
        await eventually { rig.runner.runs.last == ["setup", "--service", "--update"] }
        XCTAssertTrue(controller.touring)
        XCTAssertTrue(NSApp.windows.filter { !windowsBefore.contains(ObjectIdentifier($0)) && $0.isVisible }.isEmpty)
        rig.runner.finish(0)
        await eventually { notch.introSkippable }                   // back at the permissions beat
        notch.onIntroAction?(.skipTour)
        await eventually { !controller.touring }
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
    /// (a first run still gets the whole tour).
    // decision: APP-686
    func testALaunchNoticesAGrantSwitchedOff() {
        XCTAssertEqual(AppSetupPlan.atLaunch(.none, needs: needs(done: true, missing: ["accessibility"])),
                       .intro([.permissions, .home]))
        XCTAssertEqual(AppSetupPlan.atLaunch(.none, needs: needs(done: true, missing: ["accessibility"], firstRun: true)),
                       .intro(IntroBeat.allCases))
    }

    /// Live 2026-11-04: SMAppService's status for FERB's bundled plist read enabled at the relaunch because FERB's own
    /// LaunchAgent ran under the same label, and the unregister that followed booted the running daemon out. It counts
    /// only when launchd runs the label from another definition, or nothing runs and no LaunchAgent names it.
    // decision: APP-683
    func testOnlyAnotherDefinitionsJobCountsAsA2026_11Registration() {
        let launchAgent = "/Users/o/Library/LaunchAgents/com.ferb.daemon.plist"
        let bundled = "/Applications/FERB.app/Contents/Library/LaunchAgents/com.ferb.daemon.plist"
        XCTAssertEqual(ServiceStatus.retired(reported: .enabled, loaded: true, loadedFrom: launchAgent,
                                             launchAgent: launchAgent, launchAgentExists: true), .notRegistered)
        XCTAssertEqual(ServiceStatus.retired(reported: .enabled, loaded: false, loadedFrom: nil,
                                             launchAgent: launchAgent, launchAgentExists: true), .notRegistered)
        XCTAssertEqual(ServiceStatus.retired(reported: .enabled, loaded: true, loadedFrom: bundled,
                                             launchAgent: launchAgent, launchAgentExists: false), .enabled)
        XCTAssertEqual(ServiceStatus.retired(reported: .requiresApproval, loaded: false, loadedFrom: nil,
                                             launchAgent: launchAgent, launchAgentExists: false), .requiresApproval)
        XCTAssertEqual(LaunchctlPlan.loadedPath("gui/501/com.ferb.daemon = {\n\tactive count = 1\n\tpath = \(launchAgent)\n"
                                                + "\tstate = running\n"), launchAgent)
        XCTAssertNil(LaunchctlPlan.loadedPath("state = running\n"))
    }
}
