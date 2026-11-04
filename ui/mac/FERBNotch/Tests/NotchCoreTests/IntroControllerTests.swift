import XCTest
import Combine
@testable import FERBNotch
import NotchCore

/// The first-run tour (kb area app): the beats in order, each grant confirmed by the system, every wait shown alive,
/// Try's task run for real through the normal chat path, the controls taught by doing, and a skip or quit that leaves
/// nothing behind.
@MainActor
final class IntroControllerTests: XCTestCase {
    static let suggestions = ["Find three things on my Mac I could tidy up.", "Compare three kettles on the web."]
    static let connected: [String: Any] = ["roles": ["planner": "anthropic"],
                                           "runtimes": [["name": "anthropic", "usable": true]]]
    static let notConnected: [String: Any] = ["roles": ["planner": "codex"],
                                              "runtimes": [["name": "codex", "usable": false]]]
    static let previewEvents: [[String: Any]] = [
        ["type": "plan", "preview": true, "request_id": "p1", "text": "I'll look through your Downloads folder.",
         "plan_line": "I'll look through your Downloads folder.", "goal": "tidy up", "total": 2],
        ["type": "part", "preview": true, "request_id": "p1", "part": "1", "step": "1", "kind": "do",
         "label": "Look through Downloads", "state": "preview"],
        ["type": "part", "preview": true, "request_id": "p1", "part": "2", "step": "2", "kind": "do",
         "label": "Suggest what to tidy", "state": "preview"],
        ["type": "done", "preview": true, "request_id": "p1"]]
    /// A real task's turn as `chat.send` streams it (fake daemon): the ack, the plan with its task, the outcome.
    static func taskEvents(_ id: String, status: String = "done") -> [[String: Any]] {
        [["type": "ack", "request_id": id, "text": "On it."],
         ["type": "plan", "request_id": id, "task_id": "task-" + id, "text": "I'll look through your Downloads folder.",
          "plan_line": "I'll look through your Downloads folder.", "goal": "tidy up", "total": 1]]
            + (status.isEmpty ? [] : [outcome(id, status: status)])
    }
    static func outcome(_ id: String, status: String) -> [String: Any] {
        ["type": "outcome", "request_id": id, "task_id": "task-" + id, "status": status, "answer": "Here's what I found."]
    }
    /// Pair all, live, for one profile that pairs.
    static let pairing: [[String: Any]] = [
        ["type": "plan", "profiles": 1, "skipped": [[String: Any]](),
         "todo": [["browser": "chrome", "folder": "Default", "name": "Personal", "label": "Google Chrome"]]],
        ["type": "result", "browser": "chrome", "folder": "Default", "name": "Personal", "label": "Google Chrome",
         "paired": true],
        ["type": "done", "profiles": 1, "skipped": [[String: Any]](), "paired": 1,
         "results": [["browser": "chrome", "folder": "Default", "name": "Personal", "paired": true]]]]

    private var subscriptions: Set<AnyCancellable> = []

    /// One tour with every collaborator faked; the daemon answers as a finished, granted install unless a test says.
    @MainActor
    struct Tour {
        let rig: SetupRig
        let setup: SetupModel
        let notch: NotchModel
        let orb: FakeOrb
        let sound: FakeSoundPlayer
        let sleeper: FakeIntroSleeper
        let controls: FakeControlFeed
        let defaults: UserDefaults
        let controller: IntroController
        let lines: LineLog

        var progress: IntroProgress { IntroProgress(defaults: defaults) }

        /// Waits until FERB's line offers `id`, then picks it, as a click in the notch does.
        func pick(_ id: String, file: StaticString = #filePath, line: UInt = #line) async {
            await eventually({ notch.introLine?.choices.contains { $0.id == id } == true }, file: file, line: line)
            notch.introAction(.choose(id))
        }
    }

    final class LineLog {
        var all: [IntroLine] = []
    }

    /// A Mac with one 1440×900 screen, Finder running with a window, Notes installed but not running.
    static func mac(browser: Bool = false) -> IntroAppResolver {
        var resolver = IntroAppResolver()
        resolver.running = {
            [IntroAppResolver.RunningApp(pid: 10, name: "Finder", bundleID: "com.apple.finder",
                                         bundlePath: "/System/Library/CoreServices/Finder.app")]
                + (browser ? [IntroAppResolver.RunningApp(pid: 20, name: "Example Browser",
                                                          bundleID: "com.example.browser",
                                                          bundlePath: "/Applications/Example Browser.app")] : [])
        }
        resolver.windows = {
            [RawWindow(pid: 10, bounds: CGRect(x: 400, y: 100, width: 600, height: 400), layer: 0),
             RawWindow(pid: 10, bounds: CGRect(x: 420, y: 120, width: 100, height: 50), layer: 0)]
                + (browser ? [RawWindow(pid: 20, bounds: CGRect(x: 0, y: 25, width: 1440, height: 875), layer: 0)] : [])
        }
        resolver.screens = { [(frame: CGRect(x: 0, y: 0, width: 1440, height: 900),
                               visible: CGRect(x: 0, y: 0, width: 1440, height: 875))] }
        resolver.primaryHeight = { 900 }
        return resolver
    }

    /// `realOrb`: the tour flies a real OrbController (on a fake surface and clock) sharing the tour's sound player,
    /// instead of the fake orb.
    func tour(_ action: AppSetupAction = .none, background: BackgroundServiceManaging? = nil,
              clipSeconds: Double = 7.3, apps: IntroAppLocating? = nil, realOrb: Bool = false,
              arrival: IntroArrivalShowing? = nil) -> Tour {
        let rig = SetupRig()
        rig.rpc.replies["runtimes.status"] = Self.connected
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(accessibility: "granted", fda: "granted")
        rig.rpc.replies["setup.persona"] = ["state": "scanning"]
        rig.rpc.replies["chat.attach"] = ["session_id": "chat-1"]
        rig.rpc.replies["intro.suggest"] = ["suggestions": Self.suggestions.map { ["request": $0] }, "fits": true]
        rig.rpc.events["intro.preview"] = Self.previewEvents
        rig.rpc.events["chat.send"] = Self.taskEvents("r1")
        rig.rpc.events["setup.browsers_pairing"] = Self.pairing
        rig.app.notificationStatus = .granted
        rig.app.microphoneStatus = .granted
        let setup = rig.model(action, background: background)
        let defaults = scratchDefaults("IntroController")
        let notch = NotchModel(rpc: rig.rpc, preferences: defaults)
        let orb = FakeOrb()
        let sound = FakeSoundPlayer()
        let sleeper = FakeIntroSleeper()
        let controls = FakeControlFeed()
        let flown: OrbControlling = realOrb
            ? OrbController(surface: FakeOrbSurface(), sound: sound, clock: ManualClock(), home: { CGPoint(x: 720, y: 888) },
                            reduceMotion: { false }, makeTracker: { FakeTracker() })
            : orb
        let controller = IntroController(setup: setup, orb: flown, sound: sound, notch: notch,
                                         progress: IntroProgress(defaults: defaults), controls: controls,
                                         apps: apps ?? Self.mac(), sleeper: sleeper, clipSeconds: clipSeconds,
                                         arrival: arrival)
        let log = LineLog()
        notch.$introLine.compactMap { $0 }.sink { log.all.append($0) }.store(in: &subscriptions)
        addTeardownBlock { @MainActor in controller.skip() }
        return Tour(rig: rig, setup: setup, notch: notch, orb: orb, sound: sound, sleeper: sleeper, controls: controls,
                    defaults: defaults, controller: controller, lines: log)
    }

    // MARK: Meet

    /// DMN-513: FERB switched off under Login Items → Allow in the Background makes `ferb setup --service` exit with
    /// `ServicePlan.switchedOffStatus`; the orb guides that one visit, then the tour sets the service up again.
    // decision: APP-506, DMN-513
    func testMeetInstallsTheServiceAndGuidesLoginItemsOnlyWhenAsked() async {
        for needsOK in [true, false] {
            let service = FakeBackgroundService()
            service.off = needsOK
            let t = tour(.install, background: service)
            t.rig.rpc.replies["runtimes.status"] = Self.notConnected      // the tour waits at sign-in
            t.controller.start(beats: [.meet, .signIn, .home])
            await eventually { t.rig.runner.isRunning }                  // installed silently, nothing asked
            t.rig.runner.finish(needsOK ? ServicePlan.switchedOffStatus : 0)
            if needsOK {
                await eventually { t.notch.introLine?.choices.contains { $0.id == "openLoginItems" } == true }
                XCTAssertEqual(service.openedSettings, 0)                 // System Settings opens only on the owner's tap
                XCTAssertEqual(t.orb.docks.count, 0)
                await t.pick("openLoginItems")
                await eventually { t.orb.docks.count == 1 }
                XCTAssertEqual(service.openedSettings, 1)                 // the one Login Items visit
                XCTAssertNil(t.orb.docks[0].url)                          // nothing to drag there
                XCTAssertEqual(t.controller.beat, .meet)                  // sign-in waits for the service
                service.off = false                                       // the owner switched FERB on
                await eventually { t.rig.runner.isRunning }               // set up again, now loaded
                XCTAssertEqual(t.rig.runner.runs.count, 2)
                t.rig.runner.finish(0)
            }
            await eventually { t.controller.beat == .signIn }
            XCTAssertEqual(t.orb.docks.count, needsOK ? 1 : 0)
            XCTAssertEqual(service.openedSettings, needsOK ? 1 : 0)
            XCTAssertEqual(t.orb.hops, needsOK ? 1 : 0)
            t.controller.skip()
        }
    }

    // decision: APP-529
    func testFirstLaunchClipPlaysOnceThenSilenceThenTheLogo() async {
        XCTAssertEqual(IntroController.firstLaunchClipSeconds() ?? 0, 7.34, accuracy: 0.05)   // the bundled clip

        let t = tour(.install, clipSeconds: 7.3)
        t.rig.rpc.replies["runtimes.status"] = Self.notConnected
        t.sleeper.hold(7.3)
        t.controller.start(beats: IntroBeat.allCases, firstRun: true)
        XCTAssertEqual(t.sound.played, [.firstLaunch])
        await eventually { t.sleeper.isWaiting(7.3) }
        // Meet's install starts only once Meet has let go of the main actor: on the clip, or after the logo if it
        // didn't wait for the clip.
        await eventually { t.rig.runner.isRunning }
        XCTAssertEqual(t.sound.played, [.firstLaunch])                    // silence while the clip plays
        t.sleeper.release(7.3)
        await eventually { t.sound.played.contains(.intro) }
        XCTAssertEqual(Array(t.sound.played.prefix(2)), [.firstLaunch, .intro])
        XCTAssertEqual(t.sound.played.filter { $0 == .firstLaunch }.count, 1)
        XCTAssertEqual(t.sound.warm.first, true)                          // the engine stays warm for the tour

        let replay = tour()
        replay.rig.rpc.replies["runtimes.status"] = Self.notConnected
        replay.controller.start(beats: IntroBeat.allCases)
        await eventually { replay.sound.played.contains(.intro) }
        XCTAssertFalse(replay.sound.played.contains(.firstLaunch))

        let resumed = tour()
        resumed.rig.rpc.replies["runtimes.status"] = Self.notConnected
        var progress = resumed.progress
        progress.resumeBeat = .signIn
        resumed.controller.start(beats: IntroBeat.allCases, firstRun: true)
        await eventually { resumed.controller.beat == .signIn }
        XCTAssertFalse(resumed.sound.played.contains(.firstLaunch))
        XCTAssertFalse(resumed.sound.played.contains(.intro))
    }

    /// The first run's Dense entrance is drawn for exactly the clip and is gone when the clip ends; a replay draws
    /// none, and a tour torn down mid-clip takes it down with it.
    // decision: APP-685, APP-612
    func testFirstRunDrawsTheStartupArrivalOverTheClipOnly() async {
        let arrival = FakeArrival()
        let t = tour(.install, clipSeconds: 7.3, arrival: arrival)
        t.rig.rpc.replies["runtimes.status"] = Self.notConnected
        t.sleeper.hold(7.3)
        t.controller.start(beats: IntroBeat.allCases, firstRun: true)
        XCTAssertEqual(arrival.shown, [7.3])
        XCTAssertTrue(arrival.visible)
        await eventually { t.sleeper.isWaiting(7.3) }
        t.sleeper.release(7.3)
        await eventually { t.sound.played.contains(.intro) }
        XCTAssertFalse(arrival.visible)

        let quiet = FakeArrival()
        let replay = tour(arrival: quiet)
        replay.rig.rpc.replies["runtimes.status"] = Self.notConnected
        replay.controller.start(beats: IntroBeat.allCases)
        await eventually { replay.sound.played.contains(.intro) }
        XCTAssertEqual(quiet.shown, [])

        let cut = FakeArrival()
        let ended = tour(.install, arrival: cut)
        ended.rig.rpc.replies["runtimes.status"] = Self.notConnected
        ended.sleeper.hold(7.3)
        ended.controller.start(beats: IntroBeat.allCases, firstRun: true)
        XCTAssertTrue(cut.visible)
        ended.controller.skip()
        XCTAssertFalse(cut.visible)
    }

    /// APP-685: Skip tour shows from the permissions beat to the end, never before; it ends the tour the way the
    /// tour's own interruption does (what is still required waits in the compact window).
    // decision: APP-685
    func testSkipTourIsOfferedFromPermissionsOnwardOnly() async {
        let early = tour()
        early.rig.rpc.replies["runtimes.status"] = Self.notConnected        // it waits at sign-in
        early.controller.start(beats: [.signIn, .permissions, .home])
        await eventually { early.controller.beat == .signIn }
        XCTAssertFalse(early.notch.introSkippable)
        early.notch.introAction(.skipTour)
        await Task.yield()
        XCTAssertTrue(early.controller.running)                           // not before permissions

        let t = tour()
        var finished = 0
        t.controller.onFinished = { finished += 1 }
        t.rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(accessibility: "denied", fda: "granted")
        t.controller.start(beats: [.permissions, .home])                  // it waits for Accessibility
        await eventually { t.controller.beat == .permissions }
        XCTAssertTrue(t.notch.introSkippable)
        t.notch.introAction(.skipTour)
        await eventually { finished == 1 }
        XCTAssertFalse(t.controller.running)
        XCTAssertFalse(t.notch.introSkippable)
    }

    /// Live 2026-11-02 (two Macs): after Full Disk Access's Quit & Reopen, FERB came back in front of System Settings
    /// and covered it, since Settings never "came forward". A resumed tour and every dock beside Settings ask the notch
    /// to step behind Settings' window.
    // decision: APP-642
    func testAResumedTourAndADockBesideSettingsStepBehindSettings() async {
        let t = tour()
        var steps = 0
        t.notch.onStepBehindSettings = { steps += 1 }
        t.rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(accessibility: "denied", fda: "granted")
        var progress = t.progress
        progress.resumeBeat = .permissions                                   // reopened by Quit & Reopen
        t.controller.start(beats: [.permissions, .home])
        await eventually { steps == 1 }
        await t.pick("permissions.next")
        await eventually { t.orb.docks.count == 1 }
        XCTAssertEqual(steps, 2)
    }

    /// Owner, 2026-11-04: "when the permission is added, the ferb screen doesnt automatically comes forward to
    /// acknowledge that." The grant macOS confirms brings the tour forward over System Settings (the notch stops
    /// yielding, PanelCoordinator), with the line saying it is on; the next grant's dock steps behind Settings again.
    // decision: APP-687
    func testAConfirmedGrantBringsTheTourForwardToAcknowledgeIt() async {
        let t = tour()
        var forward = 0
        var behind = 0
        var lineWhenForward: String?
        t.notch.onComeForward = { forward += 1; lineWhenForward = t.notch.introLine?.text }
        t.notch.onStepBehindSettings = { behind += 1 }
        let off = SetupReplies.permissions(accessibility: "denied", fda: "granted")
        t.rig.rpc.replies["setup.permissions"] = off
        t.controller.start(beats: [.permissions, .home])
        await t.pick("permissions.next")
        await eventually { t.orb.docks.count == 1 }
        XCTAssertEqual(behind, 1)                                         // docked beside Settings: stepped behind it
        XCTAssertEqual(forward, 0)                                        // nothing confirmed yet
        t.rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(accessibility: "granted", fda: "granted")
        await eventually { forward == 1 }
        XCTAssertTrue(lineWhenForward?.contains("is on") == true)
        await eventually { t.progress.done }
        XCTAssertEqual(forward, 1)
    }

    /// After macOS's Quit & Reopen the resumed beat finds the grant FERB was waiting on confirmed: it is acknowledged
    /// in front of Settings too (after the resume's own step behind Settings, APP-642).
    // decision: APP-687
    func testAGrantConfirmedWhileFERBWasQuitIsAcknowledgedOnResume() async {
        let t = tour()
        var order: [String] = []
        t.notch.onComeForward = { order.append("forward") }
        t.notch.onStepBehindSettings = { order.append("behind") }
        t.rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(accessibility: "denied", fda: "granted")
        var progress = t.progress
        progress.resumeBeat = .permissions                                // reopened by Quit & Reopen
        progress.pendingGrant = "full_disk_access"                        // FERB quit while waiting on it
        t.controller.start(beats: [.permissions, .home])
        await eventually { order.contains("forward") }
        XCTAssertEqual(order.prefix(2), ["behind", "forward"])
        XCTAssertTrue(t.notch.introLine?.text.contains("is on") == true)
        XCTAssertNil(t.progress.pendingGrant)
        t.notch.introAction(.skipTour)
        await eventually { !t.controller.running }
    }

    /// APP-645 (owner: "replay"): a newer build installed mid-tour plays the startup again, the clip with the Dense
    /// entrance, and the resumed beat waits until it ends.
    // decision: APP-645
    func testANewerBuildReplaysTheStartupBeforeTheResumedBeat() async {
        let arrival = FakeArrival()
        let t = tour(clipSeconds: 7.3, arrival: arrival)
        t.rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(accessibility: "denied", fda: "granted")
        var progress = t.progress
        progress.resumeBeat = .permissions
        t.sleeper.hold(7.3)
        t.controller.start(beats: [.permissions, .home], startup: true)
        XCTAssertEqual(t.sound.played.first, .firstLaunch)
        XCTAssertEqual(arrival.shown, [7.3])
        await eventually { t.sleeper.isWaiting(7.3) }
        XCTAssertNil(t.controller.beat)                                    // the beat waits for the startup
        t.sleeper.release(7.3)
        await eventually { t.controller.beat == .permissions }
        XCTAssertFalse(arrival.visible)
    }

    /// APP-646, now APP-659 (owner): Full Disk Access gets both FERB.app and its agent app in one drag, with "both
    /// switches" wording; a list outside `bothFERBLists` keeps the agent alone.
    // decision: APP-659
    func testFullDiskAccessDragsBothFERBsAndOtherListsOnlyTheAgent() async {
        // Paths, not URLs: URL(fileURLWithPath:) adds a trailing slash only when the folder exists on this Mac (TEST-508)
        func paths(_ urls: [URL]) -> [String] { urls.map(\.path) }
        XCTAssertEqual(paths(IntroController.dragItems(SetupReplies.agent, both: true)),
                       [SetupReplies.agent, "/Applications/FERB.app"])
        XCTAssertEqual(paths(IntroController.dragItems(SetupReplies.agent, both: false)), [SetupReplies.agent])
        XCTAssertEqual(paths(IntroController.dragItems("/opt/python3.12", both: true)), ["/opt/python3.12"])

        let t = tour()
        t.rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(accessibility: "granted", fda: "denied")
        t.controller.start(beats: [.permissions, .home])
        await t.pick("permissions.next")
        await eventually { t.orb.docks.count == 1 }
        XCTAssertEqual(t.orb.dragged.first?.count, 2)
        XCTAssertEqual(t.orb.docks[0].instruction?.detail, "then turn both FERB switches on")
    }

    /// APP-659 (owner, 2026-11-04: "add that to screen recording too, cause its stuck"): Screen & System Audio
    /// Recording also lists two "FERB" rows, so its drag carries both apps with the same "both switches" wording;
    /// Accessibility and Input Monitoring keep the agent alone.
    // decision: APP-659
    func testScreenRecordingDragsBothFERBsLikeFullDiskAccess() async {
        XCTAssertEqual(IntroController.bothFERBLists, ["full_disk_access", "screen_recording"])
        let t = tour()
        t.rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(accessibility: "granted", screen: "denied",
                                                                         fda: "granted")
        t.controller.start(beats: [.permissions, .home])
        await t.pick("permissions.next")                                   // Screen Recording is the one left
        await eventually { t.orb.docks.count == 1 }
        XCTAssertEqual(t.orb.dragged.first?.map(\.path), [SetupReplies.agent, "/Applications/FERB.app"])   // TEST-508
        XCTAssertEqual(t.orb.docks[0].instruction?.detail, "then turn both FERB switches on")
    }

    // MARK: Sign in

    // decision: APP-507
    func testSignInStartsTheScanInTheBackground() async {
        let t = tour()
        t.rig.rpc.queue("runtimes.status", [Self.notConnected])
        t.controller.start(beats: [.signIn, .permissions, .home])
        await t.pick("otherWays")
        await t.pick("key.anthropic")
        await eventually { t.notch.introLine?.entry?.secure == true }
        t.notch.introAction(.enter("sk-example-key"))
        await eventually { t.rig.rpc.methods().contains("setup.permissions") }
        let methods = t.rig.rpc.methods()
        let scan = methods.firstIndex(of: "setup.persona")
        XCTAssertNotNil(scan)
        XCTAssertLessThan(scan ?? .max, methods.firstIndex(of: "setup.permissions") ?? -1)
        XCTAssertTrue(t.rig.rpc.params(of: "setup.persona").first?.isEmpty ?? false)   // `{}` starts the scan
        XCTAssertEqual(t.rig.keys.providers, [.anthropic])
        XCTAssertFalse(t.lines.all.contains { "\($0)".contains("sk-example-key") })      // the key is never shown
    }

    // MARK: Allow all

    // decision: APP-510, APP-503
    func testEachListPermissionFliesTheOrbAndWaitsForTheSystem() async {
        let t = tour()
        let off = SetupReplies.permissions(accessibility: "denied", fda: "granted")
        t.rig.rpc.replies["setup.permissions"] = off
        t.controller.start(beats: [.permissions, .home])
        await t.pick("permissions.next")
        await eventually { t.orb.docks.count == 1 }
        XCTAssertEqual(t.orb.docks[0].url, URL(fileURLWithPath: SetupReplies.agent))   // the agent app to drag
        XCTAssertNotNil(t.orb.docks[0].instruction)
        XCTAssertTrue(t.rig.opener.opened.contains { $0.absoluteString.hasSuffix("Privacy_Accessibility") })
        XCTAssertNotNil(t.orb.onHomeArrival)                              // the notch swells as it takes the orb back
        t.rig.rpc.queue("setup.permissions", [off, off, off])
        t.rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(accessibility: "granted", fda: "granted")
        await eventually { t.progress.done }
        XCTAssertTrue(t.rig.rpc.queued["setup.permissions"]?.isEmpty ?? true)   // every poll before it was seen
        XCTAssertGreaterThanOrEqual(t.rig.rpc.params(of: "setup.permissions").count, 5)
        XCTAssertEqual(t.orb.docks.count, 1)
        XCTAssertEqual(t.orb.hops, 1)
        XCTAssertEqual(t.orb.flyHomes, 1)
    }

    // decision: APP-510
    func testADeniedPromptKeepsTheBeatOpen() async {
        let t = tour()
        t.rig.app.microphoneStatus = .notDetermined
        t.rig.app.microphoneAnswer = .denied
        t.controller.start(beats: [.permissions, .home])
        await t.pick("permissions.next")
        await eventually { t.notch.introLine?.choices.contains { $0.id == "permission.microphone" } == true }
        XCTAssertEqual(t.rig.app.requests, ["microphone"])                // macOS's own prompt, where the owner is
        // The line is shown as the beat suspends on the owner's answer: nothing moves on until they pick.
        XCTAssertEqual(t.controller.beat, .permissions)
        XCTAssertFalse(t.progress.done)
        await t.pick("permission.microphone")
        await eventually { t.rig.opener.opened.contains { $0.absoluteString.hasSuffix("Privacy_Microphone") } }
        XCTAssertEqual(t.controller.beat, .permissions)                   // still waiting for the system
        t.rig.app.microphoneStatus = .granted
        await eventually { t.progress.done }
    }

    /// Owner's live run, 2026-11-01: "the permission accepting ... is broken". Screen Recording (and Input Monitoring,
    /// Full Disk Access) reach the running daemon only after it restarts; FERB.app is an accessory app whose panels never
    /// activate it, so "FERB came back to the front" never happened and the switch was never checked. Leaving System
    /// Settings is the owner's signal: the service restarts then, each time while the switch still reads off.
    // decision: APP-510, APP-598
    func testASwitchThatAppliesAfterARestartIsCheckedWhenTheOwnerLeavesSettings() async {
        let t = tour()
        var front: String? = "com.ferb.FERB"
        t.setup.frontmostApp = { front }
        let off = SetupReplies.permissions(accessibility: "granted", screen: "denied", fda: "granted")
        t.rig.rpc.replies["setup.permissions"] = off
        t.controller.start(beats: [.permissions, .home])
        await t.pick("permissions.next")
        await eventually { t.orb.docks.count == 1 }                       // Screen Recording's pane, the orb beside it
        front = SettingsAnchor.bundleID
        var polls = t.rig.rpc.params(of: "setup.permissions").count
        await eventually { t.rig.rpc.params(of: "setup.permissions").count >= polls + 2 }
        XCTAssertEqual(t.rig.service.restarts, 0)                         // still in Settings: nothing restarted
        front = "com.apple.finder"                                        // left before switching it on
        await eventually { t.rig.service.restarts == 1 }
        XCTAssertEqual(t.controller.beat, .permissions)                   // still off: the beat waits
        front = SettingsAnchor.bundleID                                   // back, switched on, left again
        polls = t.rig.rpc.params(of: "setup.permissions").count
        await eventually { t.rig.rpc.params(of: "setup.permissions").count >= polls + 2 }
        t.rig.service.onRestart = { [rpc = t.rig.rpc] in
            rpc.replies["setup.permissions"] = SetupReplies.permissions(accessibility: "granted", fda: "granted")
        }
        front = "com.apple.finder"
        await eventually { t.progress.done }
        XCTAssertEqual(t.rig.service.restarts, 2)
        XCTAssertEqual(t.orb.hops, 1)                                     // confirmed by the system, then the hop
    }

    /// macOS's "Quit & Reopen" can end the daemon with a clean exit, which launchd doesn't restart (KeepAlive
    /// SuccessfulExit false), and its reopen of the agent app is refused (only `-m ferb.daemon` runs). A grant's wait
    /// that stops hearing from the daemon starts it again, once per silence, and never kills a running one.
    // decision: APP-510, APP-598
    func testAGrantWaitStartsTheServiceWhenItStopsAnswering() async {
        let t = tour()
        t.rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(accessibility: "granted", screen: "denied",
                                                                          fda: "granted")
        t.controller.start(beats: [.permissions, .home])
        await t.pick("permissions.next")
        await eventually { t.orb.docks.count == 1 }
        t.rig.rpc.failures["setup.permissions"] = DaemonRPCError.invalidResponse   // the daemon is gone
        await eventually { t.rig.service.starts == 1 }
        let polls = t.rig.rpc.params(of: "setup.permissions").count
        await eventually { t.rig.rpc.params(of: "setup.permissions").count >= polls + 4 }
        XCTAssertEqual(t.rig.service.starts, 1)                           // once per silence
        XCTAssertEqual(t.rig.service.restarts, 0)                         // never a kill
        t.rig.rpc.failures = [:]
        t.rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(accessibility: "granted", fda: "granted")
        await eventually { t.progress.done }
    }

    // decision: APP-613, APP-614, APP-615
    func testIndividualPermissionDropWaitsForConfirmationAndTheLogoReturnsOnlyAtHome() async {
        let t = tour()
        t.rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(accessibility: "denied", fda: "denied")
        t.controller.start(beats: [.permissions, .home])
        await eventually { t.notch.introLine?.checklist.contains { $0.actionID == "permission.full_disk_access" } == true }
        t.notch.introAction(.choose("permission.full_disk_access"))
        await eventually { t.orb.docks.count == 1 }
        XCTAssertFalse(t.rig.opener.opened.contains { $0.absoluteString.hasSuffix("Privacy_Accessibility") })
        t.orb.onDropAccepted?()
        XCTAssertFalse(t.notch.introLine?.checklist.first { $0.id == "full_disk_access" }?.complete ?? true)
        XCTAssertFalse(t.sound.played.contains(.allAllowed))
        XCTAssertEqual(t.orb.flyHomes, 0)
        t.rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(accessibility: "denied", fda: "granted")
        await eventually { t.notch.introLine?.checklist.first { $0.id == "full_disk_access" }?.complete == true }
        XCTAssertEqual(t.orb.flyHomes, 0)
        t.notch.introAction(.choose("permission.accessibility"))
        await eventually { t.rig.opener.opened.contains { $0.absoluteString.hasSuffix("Privacy_Accessibility") } }
        t.rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(accessibility: "granted", fda: "granted")
        await eventually { t.progress.done }
        XCTAssertEqual(t.sound.played.filter { $0 == .allAllowed }.count, 1)
        XCTAssertEqual(t.orb.flyHomes, 1)
    }

    // decision: APP-613
    func testQuickPermissionsLeaveRestartPermissionsForAnIndividualChoice() async {
        let t = tour()
        t.rig.app.microphoneStatus = .notDetermined
        t.rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(accessibility: "granted", fda: "denied")
        t.controller.start(beats: [.permissions, .home])
        await t.pick("permissions.next")
        await eventually { t.rig.app.requests.contains("microphone") }
        await eventually { t.notch.introLine?.checklist.first { $0.id == "microphone" }?.complete == true }
        XCTAssertFalse(t.rig.opener.opened.contains { $0.absoluteString.hasSuffix("Privacy_AllFiles") })
        XCTAssertFalse(t.sound.played.contains(.allAllowed))
        XCTAssertEqual(t.controller.beat, .permissions)
        await t.pick("permissions.next")
        await eventually { t.rig.opener.opened.contains { $0.absoluteString.hasSuffix("Privacy_AllFiles") } }
    }

    // MARK: Browsers and FERB's email

    // decision: APP-511, APP-512
    func testPairAllRunsOnceAndIdentityCanBeSkipped() async {
        let t = tour()
        var finished = 0
        t.controller.onFinished = { finished += 1 }
        t.controller.start(beats: [.browsers, .identity, .home])
        await t.pick("pairAll")
        await t.pick("identityLater")
        await eventually { t.progress.done }
        XCTAssertEqual(t.rig.rpc.methods().filter { $0 == "setup.browsers_pairing" }.count, 1)
        XCTAssertFalse(t.rig.rpc.methods().contains { $0.hasPrefix("identity.") })
        // The tour ended: the engine is released (a ringing sound may finish: only a skip stops it), the orb gone,
        // the notch quiet.
        await eventually { finished == 1 }
        XCTAssertEqual(t.sound.warm, [true, false])
        XCTAssertEqual(t.sound.stops, 0)
        XCTAssertEqual(t.orb.tearDowns, 1)
        XCTAssertNil(t.notch.introLine)
        XCTAssertNil(t.progress.resumeBeat)
    }

    // MARK: Try

    // decision: APP-693
    func testTheScanLineIsShownBeforeTheSuggestion() async {
        let t = tour()
        let summary = "You spend most days in a code editor and a design app."
        t.rig.rpc.replies["setup.persona"] = ["state": "seen", "summary": summary]
        t.controller.start(beats: [.tryIt, .home])
        await eventually { t.notch.introLine?.choices.contains { $0.id == "suggestion.0" } == true }
        let scan = t.lines.all.firstIndex { ($0.text + ($0.detail ?? "")).contains("code editor") }
        let suggestion = t.lines.all.firstIndex { $0.choices.contains { $0.id == "suggestion.0" } }
        XCTAssertNotNil(scan)
        XCTAssertLessThan(scan ?? .max, suggestion ?? -1)
        let offered = t.notch.introLine?.choices.map(\.title) ?? []
        XCTAssertTrue(offered.contains(Self.suggestions[0]) && offered.contains(Self.suggestions[1]))
    }

    /// Owner decision 2026-11-04 (APP-666): the picked suggestion runs for real, through the same `chat.send` as a typed
    /// message, in the open chat while the notch follows it; then the other one, or on. No plan-only preview.
    // decision: APP-693
    func testTryRunsThePickedTaskForRealThroughTheChat() async {
        let t = tour()
        t.controller.start(beats: [.tryIt, .home])
        await t.pick("suggestion.0")
        await eventually { t.rig.rpc.params(of: "chat.send").count == 1 }
        XCTAssertEqual(t.rig.rpc.params(of: "chat.send").first?["text"] as? String, Self.suggestions[0])
        XCTAssertTrue(t.notch.expanded)                                          // in the real chat
        await eventually { t.notch.conversation.turns.contains { $0.request == Self.suggestions[0] && $0.status == "done" } }
        await t.pick("otherSuggestion")                                          // the result line offers the other one
        await eventually { t.rig.rpc.params(of: "chat.send").count == 2 }
        XCTAssertEqual(t.rig.rpc.params(of: "chat.send").last?["text"] as? String, Self.suggestions[1])
        await t.pick("next")
        await eventually { t.progress.done }
        XCTAssertFalse(t.rig.rpc.methods().contains("intro.preview"))
        XCTAssertFalse(t.notch.conversation.turns.contains { $0.preview })
        let starting = t.lines.all.filter { $0.text == IntroCopy.starting.text || $0.text == "On it…" }
        XCTAssertFalse(starting.isEmpty)
        XCTAssertTrue(starting.allSatisfy { $0.since != nil })                  // a live line while it runs
    }

    /// A task that runs long never holds the tour: Continue the tour leaves it running, as any task.
    // decision: APP-693
    func testATryTaskThatRunsOnNeverHoldsTheTour() async {
        let t = tour()
        t.rig.rpc.events["chat.send"] = Self.taskEvents("r1", status: "")
        t.rig.rpc.heldStreams = ["chat.send"]
        t.controller.start(beats: [.tryIt, .home])
        await t.pick("suggestion.1")
        await eventually { t.notch.introLine?.text == "On it…" }
        await t.pick("goOn")
        await t.pick("next")
        await eventually { t.progress.done }
        XCTAssertFalse(t.rig.rpc.methods().contains("task.stop"))               // it keeps running
        t.rig.rpc.endStream("chat.send")
    }

    // MARK: Control

    /// Stop is practised on a real task the tour starts (APP-667): Stop this task answers at once with the real
    /// `task.stop` for that task, and the beat goes on once the chat shows it stopped. Past report (owner, 2026-11-04):
    /// the old stop step seemed stuck.
    // decision: APP-667, APP-668
    func testControlBeatPractisesStopOnARealTask() async {
        let t = tour()
        t.rig.rpc.events["chat.send"] = Self.taskEvents("r1", status: "")
        t.rig.rpc.heldStreams = ["chat.send"]
        t.controller.start(beats: [.control, .home])
        await eventually { t.controls.onControl != nil }
        t.controls.send(.toggle)
        t.controls.send(.talk)
        await eventually { t.rig.rpc.params(of: "chat.send").count == 1 }
        await eventually { t.notch.conversation.turns.contains { $0.backendTaskID == "task-r1" } }
        await t.pick("stopTask")
        XCTAssertEqual(t.notch.introLine?.text, IntroCopy.stopping.text)        // at once, before the daemon answers
        await eventually { t.rig.rpc.methods().contains("task.stop") }
        XCTAssertEqual(t.rig.rpc.params(of: "task.stop").first?["task_ids"] as? [String], ["task-r1"])
        t.rig.rpc.endStream("chat.send", with: [Self.outcome("r1", status: "stopped")])
        await eventually { t.notch.introLine == IntroCopy.control(.pause) }    // waiting on the last control
        XCTAssertFalse(t.progress.done)
        t.notch.daemonPaused = true
        t.controls.send(.pause)
        await eventually { t.progress.done }
        XCTAssertTrue(t.rig.rpc.methods().contains("sense.resume"))             // the pause is lifted at once
        XCTAssertFalse(t.notch.daemonPaused)
    }

    /// The stop practice never hangs: a task that ends before Stop is told and the beat goes on; Go on leaves a stop
    /// the daemon hasn't confirmed.
    // decision: APP-667
    func testTheStopPracticeCannotHang() async {
        let finished = tour()                                                   // ends before the owner stops it
        finished.controller.start(beats: [.control, .home])
        await eventually { finished.controls.onControl != nil }
        finished.controls.send(.toggle)
        finished.controls.send(.talk)
        await eventually { finished.notch.introLine == IntroCopy.control(.pause) }
        XCTAssertTrue(finished.lines.all.contains(IntroCopy.finishedBeforeStop))

        let unconfirmed = tour()
        unconfirmed.rig.rpc.events["chat.send"] = Self.taskEvents("r1", status: "")
        unconfirmed.rig.rpc.heldStreams = ["chat.send"]
        unconfirmed.controller.start(beats: [.control, .home])
        await eventually { unconfirmed.controls.onControl != nil }
        unconfirmed.controls.send(.toggle)
        unconfirmed.controls.send(.talk)
        await eventually { unconfirmed.notch.conversation.turns.contains { $0.backendTaskID == "task-r1" } }
        await unconfirmed.pick("stopTask")
        await unconfirmed.pick("goOn")                                          // the daemon never confirmed
        await eventually { unconfirmed.notch.introLine == IntroCopy.control(.pause) }
        unconfirmed.rig.rpc.endStream("chat.send")
    }

    // MARK: Skip, resume, the notch

    // decision: APP-685
    func testSkipMidFlightTearsEverythingDown() async {
        let t = tour()
        var finished = 0
        t.controller.onFinished = { finished += 1 }
        t.orb.holdFlyHome = true
        let off = SetupReplies.permissions(accessibility: "denied", fda: "granted")
        t.rig.rpc.replies["setup.permissions"] = off
        t.controller.start(beats: [.permissions, .home])
        await t.pick("permissions.next")
        await eventually { t.orb.docks.count == 1 }
        t.rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(accessibility: "granted", fda: "granted")
        await eventually { t.orb.flying }                                 // the one flight home at the end
        t.rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(accessibility: "granted", fda: "denied")
        t.controller.skip()
        XCTAssertEqual(t.orb.tearDowns, 1)
        XCTAssertGreaterThanOrEqual(t.sound.stops, 1)
        XCTAssertEqual(t.sound.warm, [true, false])
        XCTAssertNil(t.notch.introLine)
        await eventually { finished == 1 }
        XCTAssertEqual(t.controller.remainingRequired, ["full_disk_access"])   // only required steps are left
        XCTAssertFalse(t.rig.rpc.methods().contains("setup.finish"))      // a grant is off: setup isn't done
        // The cut flight home has returned to the beat, which ran on to its next wait or its end.
        await eventually { t.orb.flightsLanded == 1 }
        XCTAssertFalse(t.rig.opener.opened.contains { $0.absoluteString.hasSuffix("Privacy_AllFiles") })
        XCTAssertFalse(t.rig.rpc.methods().contains("setup.browsers_pair_all"))
        XCTAssertEqual(t.orb.tearDowns, 1)
    }

    // decision: APP-685
    func testQuitAndReopenResumesTheBeat() async {
        let t = tour()
        t.rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(screen: "denied", fda: "granted")
        var progress = t.progress
        progress.resumeBeat = .permissions                                // quit at Screen Recording's Quit & Reopen
        t.controller.start(beats: IntroBeat.allCases, firstRun: true)
        await eventually { t.controller.beat == .permissions }
        XCTAssertTrue(t.rig.runner.runs.isEmpty)                          // Meet and sign-in aren't run again
        XCTAssertFalse(t.sound.played.contains(.firstLaunch))
        XCTAssertEqual(t.progress.resumeBeat, .permissions)               // kept until the beat is done

        let fresh = tour()
        fresh.rig.rpc.replies["runtimes.status"] = Self.notConnected
        fresh.controller.start(beats: [.signIn, .permissions, .home])
        await eventually { fresh.controller.beat == .signIn }
        XCTAssertEqual(fresh.progress.resumeBeat, .signIn)                // a quit here comes back here
    }

    // MARK: Fix round 1

    // decision: APP-508, APP-509
    func testATourWithNothingLeftRecordsSetupDone() async {
        let complete = tour()
        complete.controller.start(beats: [.browsers, .home])
        await complete.pick("browsersLater")
        await eventually { complete.progress.done }
        let finish = complete.rig.rpc.params(of: "setup.finish")
        XCTAssertEqual(finish.count, 1)                                   // as the setup window's Done
        XCTAssertEqual((finish.first?["app"] as? [String: String])?["microphone"], "granted")   // FERB.app's own rows

        let missing = tour()
        missing.rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(accessibility: "granted", fda: "denied")
        missing.controller.start(beats: [.browsers, .home])
        await missing.pick("browsersLater")
        await eventually { missing.progress.done }
        XCTAssertFalse(missing.rig.rpc.methods().contains("setup.finish"))   // a grant is off: not done
        XCTAssertEqual(missing.controller.remainingRequired, ["full_disk_access"])
    }

    // decision: APP-588
    func testTheTourOpensTheChatWithoutTakingFocus() async {
        let t = tour()
        var focused = 0, quiet = 0, interactions = 0
        t.notch.onPresentationChange = { [notch = t.notch] _ in if notch.expanded { focused += 1 } }
        t.notch.onBackgroundChange = { _ in quiet += 1 }
        t.notch.onInteraction = { interactions += 1 }
        t.controller.start(beats: [.tryIt, .control, .home])
        await t.pick("suggestion.0")
        await eventually { t.notch.expanded && t.notch.conversation.turns.contains { $0.request == Self.suggestions[0] } }
        await t.pick("next")
        await eventually { t.controls.onControl != nil }
        t.controls.send(.toggle)
        t.controls.send(.talk)
        await eventually { t.notch.expanded && t.rig.rpc.params(of: "chat.send").count == 2 }
        XCTAssertEqual(focused, 0)                                        // no open the tour made asked for focus
        XCTAssertEqual(interactions, 0)
        XCTAssertGreaterThan(quiet, 0)
    }

    // decision: APP-593
    func testTypedTextIsEmptiedWhenTheLineChanges() {
        let notch = NotchModel(rpc: ScriptedRPC(), preferences: scratchDefaults("IntroTyped"))
        notch.introLine = IntroCopy.pasteKey(.anthropic)
        notch.introTyped = "sk-example-secret"
        notch.introLine = IntroCopy.pasteKey(.anthropic)                  // the same line again: kept
        XCTAssertEqual(notch.introTyped, "sk-example-secret")
        notch.introLine = IntroCopy.identityAddress                       // another field: never carries it over
        XCTAssertEqual(notch.introTyped, "")
        notch.introTyped = "someone@example.com"
        notch.introLine = nil
        XCTAssertEqual(notch.introTyped, "")
    }

    // decision: APP-510
    func testAllowAllWaitsForTheDaemonsAnswer() async {
        let t = tour()
        t.rig.rpc.failFirst["setup.permissions"] = 3                      // the daemon isn't answering yet
        t.controller.start(beats: [.permissions, .home])
        await eventually { t.progress.done }
        XCTAssertGreaterThanOrEqual(t.rig.rpc.params(of: "setup.permissions").count, 4)
        let ready = t.lines.all.firstIndex { $0.text == IntroCopy.gettingReady.text && $0.since != nil }
        let everything = t.lines.all.firstIndex { $0 == IntroCopy.allAllowed }
        XCTAssertNotNil(ready)
        XCTAssertLessThan(ready ?? .max, everything ?? -1)                // never "everything" before it answered
        XCTAssertEqual(t.lines.all.filter { $0 == IntroCopy.allAllowed }.count, 1)
    }

    // decision: APP-685
    func testResumeRunsEarlierStepsOnlyWhenStillNeeded() async {
        let signedOut = tour()
        signedOut.rig.rpc.replies["runtimes.status"] = Self.notConnected
        var progress = signedOut.progress
        progress.resumeBeat = .control                                    // quit at Control, signed out since
        signedOut.controller.start(beats: IntroBeat.allCases)
        await eventually { signedOut.controller.beat == .signIn }
        XCTAssertTrue(signedOut.rig.runner.runs.isEmpty)                  // Meet isn't run again

        let allSet = tour()
        var kept = allSet.progress
        kept.resumeBeat = .control
        allSet.controller.start(beats: IntroBeat.allCases)
        await eventually { allSet.controller.beat == .control }
        XCTAssertFalse(allSet.lines.all.contains { $0 == IntroCopy.signIn || $0 == IntroCopy.allAllowed })

        let stale = tour()
        stale.rig.rpc.replies["runtimes.status"] = Self.notConnected
        var old = stale.progress
        old.resumeBeat = .tryIt                                           // not in this plan: the whole plan runs
        stale.controller.start(beats: [.signIn, .permissions, .home])
        await eventually { stale.controller.beat == .signIn }
    }

    /// A tour quit at Allow all and reopened while the daemon is down (macOS's "Quit & Reopen" ended it, or it is still
    /// starting): the tour says it is picking up and waits, then starts the service, instead of reading the
    /// silence as signed out and asking for sign-in again (which also moved the resume point back to sign-in).
    // decision: APP-685, APP-598
    func testAResumedTourWaitsForTheServiceBeforeAnyBeat() async {
        let t = tour()
        var progress = t.progress
        progress.resumeBeat = .permissions
        for method in ["ping", "runtimes.status", "setup.permissions"] {
            t.rig.rpc.failures[method] = DaemonRPCError.invalidResponse   // nothing answers yet
        }
        t.rig.service.onStart = { [rpc = t.rig.rpc] in rpc.failures = [:] }   // started, it answers
        t.controller.start(beats: IntroBeat.allCases)
        await eventually { t.controller.beat == .browsers }
        for wait in [IntroCopy.resuming.text, IntroCopy.startingService.text] {       // alive, saying which (APP-679)
            XCTAssertTrue(t.lines.all.contains { $0.text == wait && $0.since != nil }, wait)
        }
        XCTAssertFalse(t.lines.all.contains(IntroCopy.signIn))            // never asked to sign in again
        XCTAssertEqual(t.rig.service.starts, 1)
        XCTAssertTrue(t.rig.runner.runs.isEmpty)                          // Meet isn't run again
    }

    // MARK: Round 2

    // decision: APP-685
    func testSkipWithNothingLeftRecordsSetupDone() async {
        let t = tour()
        var finished = 0
        t.controller.onFinished = { finished += 1 }
        t.controller.start(beats: [.browsers, .home])
        await eventually { t.notch.introLine?.choices.contains { $0.id == "pairAll" } == true }
        t.controller.skip()
        await eventually { finished == 1 }
        XCTAssertEqual(t.controller.remainingRequired, [])
        XCTAssertEqual(t.rig.rpc.params(of: "setup.finish").count, 1)
    }

    // decision: APP-693
    func testTryRunsItsTaskWithNoOrb() async {
        // Past complaint (owner, build 202611040536): the orb flew to the task's app with "I'm working in Visual
        // Studio Code"; the owner asked for it gone from this step. The chat's turn card and the notch line remain.
        let t = tour()
        t.rig.rpc.replies["intro.suggest"] = ["suggestions": [["request": Self.suggestions[0], "app": "Finder"],
                                                              ["request": Self.suggestions[1], "app": "Notes"]],
                                              "fits": true]
        t.controller.start(beats: [.tryIt, .home])
        await t.pick("suggestion.0")
        await eventually { t.notch.conversation.turns.contains { $0.request == Self.suggestions[0] && $0.status == "done" } }
        await t.pick("otherSuggestion")
        await eventually { t.rig.rpc.params(of: "chat.send").count == 2 }
        await t.pick("next")
        await eventually { t.progress.done }
        XCTAssertEqual(t.orb.launches.count, 0)
        XCTAssertLessThanOrEqual(t.orb.flyHomes, 1)                             // only Home's own call
        XCTAssertFalse(t.lines.all.contains { $0.text.hasPrefix("I'm working in") })
    }

    // decision: APP-511
    func testTheBrowsersBeatPointsTheOrbWhilePairing() async {
        let t = tour(apps: Self.mac(browser: true))
        t.rig.rpc.replies["setup.browsers"] = ["browsers": [["browser": "example", "label": "Example Browser",
                                                              "profiles": [[String: Any]]()]]]
        var pairedWhenHome: [Bool] = []
        t.orb.onFlyHome = { [rpc = t.rig.rpc] in pairedWhenHome.append(rpc.methods().contains("setup.browsers_pairing")) }
        t.controller.start(beats: [.browsers, .home])
        await t.pick("pairAll")
        await eventually { t.progress.done }
        XCTAssertEqual(t.orb.launches.count, 1)
        XCTAssertEqual(t.orb.launches.first?.instruction, IntroCopy.pairingOrb)
        XCTAssertEqual(t.orb.launches.first?.point, CGPoint(x: 720, y: 791))    // inside a window too wide to stand beside
        XCTAssertEqual(pairedWhenHome, [true])                                  // home after the pair-all reply
    }

    /// An installed Chrome-family browser FERB doesn't know is named after Pair all, never passed over (HAND-415).
    // decision: HAND-415
    func testPairAllNamesABrowserFERBDoesntSupportYet() async {
        let t = tour()
        t.rig.rpc.replies["setup.browsers"] = [
            "browsers": [["browser": "chrome", "label": "Google Chrome", "profiles": [[String: Any]]()]],
            "unsupported": [["app": "Example Browser", "bundle_id": "com.example.browser",
                             "message": "Example Browser isn't supported by FERB yet"]]]
        t.controller.start(beats: [.browsers, .home])
        await t.pick("pairAll")
        await eventually { t.progress.done }
        XCTAssertEqual(t.setup.browsers?.unsupported.map(\.app), ["Example Browser"])
        XCTAssertTrue(t.lines.all.contains {
            $0.text == IntroCopy.paired.text && $0.detail == "Example Browser isn't supported by FERB yet."
        })
    }

    // MARK: Final review

    /// The sound engine is shared with everyday sounds: the real orb's teardown stops only its own motion, and only a
    /// skip stops what is playing; the natural end lets a ringing sound finish.
    // decision: APP-685
    func testOnlyASkipStopsTheSharedSoundsWithTheRealOrb() async {
        let finished = tour(realOrb: true)
        var ended = 0
        finished.controller.onFinished = { ended += 1 }
        finished.controller.start(beats: [.browsers, .home])
        await finished.pick("browsersLater")
        await eventually { ended == 1 }
        XCTAssertEqual(finished.sound.warm, [true, false])
        XCTAssertEqual(finished.sound.stops, 0)

        let skipped = tour(realOrb: true)
        skipped.controller.start(beats: [.browsers, .home])
        await eventually { skipped.notch.introLine?.choices.contains { $0.id == "pairAll" } == true }
        skipped.controller.skip()
        XCTAssertEqual(skipped.sound.stops, 1)
        XCTAssertEqual(skipped.sound.warm, [true, false])
    }

    /// Pair all is shown live, a profile at a time (HAND-412), and a profile FERB's store page can fix offers Add to:
    /// the page opens in that profile, the notch waits until the extension is on there, and pairing waits for the
    /// owner's Pair (HAND-413).
    // decision: HAND-412, HAND-413, APP-665
    func testPairAllIsLiveAndAddToWaitsThenOffersToPair() async {
        let t = tour()
        t.rig.rpc.events["setup.browsers_pairing"] = [
            ["type": "plan", "profiles": 2,
             "todo": [["browser": "chrome", "folder": "Default", "name": "Personal", "label": "Google Chrome"]],
             "skipped": [["browser": "chrome", "folder": "Profile 1", "name": "Work", "label": "Google Chrome",
                          "reason": "removed", "step": "add FERB from its store page", "can_add": true]]],
            ["type": "result", "browser": "chrome", "folder": "Default", "name": "Personal", "paired": true],
            ["type": "done", "profiles": 2, "paired": 1,
             "results": [["browser": "chrome", "folder": "Default", "name": "Personal", "paired": true]],
             "skipped": [["browser": "chrome", "folder": "Profile 1", "name": "Work", "label": "Google Chrome",
                          "reason": "removed", "step": "add FERB from its store page", "can_add": true]]]]
        func browsers(_ state: String) -> [String: Any] {
            ["browsers": [["browser": "chrome", "label": "Google Chrome",
                           "profiles": [["folder": "Default", "name": "Personal", "state": "paired"],
                                        ["folder": "Profile 1", "name": "Work", "state": state]]]]]
        }
        t.rig.rpc.replies["setup.browsers"] = browsers("removed")
        t.controller.start(beats: [.browsers, .home])
        await t.pick("pairAll")
        await eventually { t.notch.introLine?.checklist.contains { $0.actionID == "add.chrome/Profile 1" } == true }
        let live = t.lines.all.filter { !$0.checklist.isEmpty && $0.choices.isEmpty }
        XCTAssertFalse(live.isEmpty)                                            // the live pairing line, with its rows
        XCTAssertTrue(live.allSatisfy { $0.since != nil })
        t.notch.introAction(.choose("add.chrome/Profile 1"))
        await eventually { t.rig.rpc.methods().contains("setup.browser_open") }
        XCTAssertEqual(t.rig.rpc.params(of: "setup.browser_open").first?["folder"] as? String, "Profile 1")
        await eventually { t.notch.introLine?.choices.contains { $0.id == "stopWaiting" } == true }
        XCTAssertNotNil(t.notch.introLine?.since)
        XCTAssertFalse(t.rig.rpc.methods().contains("setup.browser_pair"))      // nothing pairs by itself
        t.rig.rpc.replies["setup.browsers"] = browsers("on")
        t.rig.rpc.replies["setup.browser_pair"] = ["paired": true]
        await t.pick("pairOne")
        await eventually { t.progress.done }
        XCTAssertEqual(t.rig.rpc.params(of: "setup.browser_pair").first?["folder"] as? String, "Profile 1")
    }

    /// Every wait of the tour says what FERB is doing and stays alive (APP-665): the service install, the suggestion,
    /// the pairing.
    // decision: APP-665
    func testEveryAwaitedStepShowsALiveLine() async {
        let t = tour(.install)
        t.controller.start(beats: [.meet, .browsers, .tryIt, .home])
        await eventually { t.rig.runner.isRunning }
        await eventually { t.notch.introLine?.text == IntroCopy.gettingReady.text }
        XCTAssertNotNil(t.notch.introLine?.since)
        t.rig.runner.finish(0)
        await t.pick("pairAll")
        await t.pick("suggestion.0")
        await t.pick("next")
        await eventually { t.progress.done }
        for wait in [IntroCopy.lookingAtBrowsers.text, IntroCopy.thinking.text, IntroCopy.starting.text] {
            let shown = t.lines.all.filter { $0.text == wait }
            XCTAssertFalse(shown.isEmpty, wait)
            XCTAssertTrue(shown.allSatisfy { $0.since != nil }, wait)
        }
    }

    /// Owner, 2026-11-04: "theres no feedback or whatsoever shown in notch what is happening, it seems stuck". Every
    /// step of a permission's wait shows in the notch as it happens, alive (a since), with its row saying the same:
    /// macOS's question, Settings opening, the owner's switch, each check, the restart on leaving Settings (APP-598),
    /// the silent service started again, the restart once a fresh reading sees the switch on (APP-662); the row ticks
    /// only once the system reports it on (APP-510).
    // decision: APP-679, APP-665, APP-510, APP-598, APP-662
    func testEachStepOfAPermissionWaitShowsALiveLine() async {
        let t = tour()
        var front: String? = SettingsAnchor.bundleID
        t.setup.frontmostApp = { front }
        t.rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(accessibility: "granted", screen: "denied",
                                                                          fda: "granted")
        t.controller.start(beats: [.permissions, .home])
        await t.pick("permissions.next")                                   // Screen Recording, the one left
        await eventually { t.orb.docks.count == 1 }
        var polls = t.rig.rpc.params(of: "setup.permissions").count
        await eventually { t.rig.rpc.params(of: "setup.permissions").count >= polls + 2 }
        front = "com.apple.finder"                                         // left Settings, the switch still off
        await eventually { t.rig.service.restarts == 1 }
        t.rig.service.onStart = { [rpc = t.rig.rpc] in rpc.failures = [:] }
        t.rig.rpc.failures["setup.permissions"] = DaemonRPCError.invalidResponse   // the daemon is gone
        await eventually { t.rig.service.starts == 1 }
        polls = t.rig.rpc.params(of: "setup.permissions").count
        await eventually { t.rig.rpc.params(of: "setup.permissions").count >= polls + 2 }
        var onAt: Int?                                                     // lines shown before the system read it on
        t.rig.service.onRestart = { [rpc = t.rig.rpc, lines = t.lines] in  // the restarted daemon reads it on
            onAt = lines.all.count
            rpc.replies["setup.permissions"] = SetupReplies.permissions(accessibility: "granted", fda: "granted")
        }
        t.rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(accessibility: "granted", screen: "denied",
                                                                          fda: "granted", screenRestartNeeded: true)
        await eventually { t.progress.done }

        let first = t.lines.all.first
        XCTAssertEqual(first?.text, IntroCopy.checkingPermissions.text)
        XCTAssertNotNil(first?.since)
        let row = PermissionRow(id: "screen_recording", status: .denied, process: "daemon", required: true, pane: nil,
                                requestable: true, restartToCheck: true)
        let steps: [GrantWait.Step] = [.asking, .opening, .waitingForSwitch, .checking, .restarting(seenOn: false),
                                       .startingService, .restarting(seenOn: true)]
        for step in steps {
            let wait = GrantWait(id: row.id, step: step, at: Date())
            let text = IntroCopy.grantWaitLine(wait, row: row, added: false).text
            let status = IntroCopy.grantWaitStatus(wait, row: row, added: false)
            let shown = t.lines.all.filter { line in
                line.text == text && line.checklist.contains { $0.id == row.id && $0.status == status }
            }
            XCTAssertFalse(shown.isEmpty, "\(step)")
            XCTAssertTrue(shown.allSatisfy { $0.since != nil }, "\(step)")
        }
        // The row ticks only from the system's answer: never on a line before the restarted daemon read it on, and the
        // line then says macOS confirmed it.
        XCTAssertNotNil(onAt)
        XCTAssertFalse(t.lines.all.prefix(onAt ?? 0).contains { line in
            line.checklist.contains { $0.id == row.id && $0.complete }
        })
        XCTAssertTrue(t.lines.all.dropFirst(onAt ?? 0).contains { line in
            line.since == nil && line.text.hasPrefix(PermissionCopy.label(row.id))
                && line.checklist.contains { $0.id == row.id && $0.complete }
        })
        XCTAssertEqual(t.rig.service.restarts, 2)
        XCTAssertEqual(t.rig.service.starts, 1)
    }
}

/// Records the first-launch entrance's shows and hides.
@MainActor
final class FakeArrival: IntroArrivalShowing {
    var shown: [Double] = []
    var visible = false
    func show(seconds: Double) { shown.append(seconds); visible = true }
    func hide() { visible = false }
}
