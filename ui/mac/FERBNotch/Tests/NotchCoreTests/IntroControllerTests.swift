import XCTest
import Combine
@testable import FERBNotch
import NotchCore

/// The first-run tour (kb area app): the beats in order, each grant confirmed by the system, a preview that
/// never acts, the controls taught by doing, and a skip or quit that leaves nothing behind.
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
        resolver.defaultBrowser = { URL(fileURLWithPath: "/Applications/Example Browser.app") }
        resolver.bundleID = { $0.lastPathComponent == "Example Browser.app" ? "com.example.browser" : nil }
        resolver.installed = { ["Notes": "/System/Applications/Notes.app"][$0] }
        resolver.screens = { [(frame: CGRect(x: 0, y: 0, width: 1440, height: 900),
                               visible: CGRect(x: 0, y: 0, width: 1440, height: 875))] }
        resolver.primaryHeight = { 900 }
        resolver.notch = { CGPoint(x: 720, y: 888) }
        return resolver
    }
    static let besideFinder = CGPoint(x: 384, y: 600)      // left of Finder's window (AppKit 400…1000, 400…800)
    static let resting = CGPoint(x: 720, y: 600)           // below the notch, a third of the way down

    /// `realOrb`: the tour flies a real OrbController (on a fake surface and clock) sharing the tour's sound player,
    /// instead of the fake orb.
    func tour(_ action: AppSetupAction = .none, background: BackgroundServiceManaging? = nil,
              clipSeconds: Double = 7.3, apps: IntroAppLocating? = nil, realOrb: Bool = false) -> Tour {
        let rig = SetupRig()
        rig.rpc.replies["runtimes.status"] = Self.connected
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(accessibility: "granted", fda: "granted")
        rig.rpc.replies["setup.persona"] = ["state": "scanning"]
        rig.rpc.replies["chat.attach"] = ["session_id": "chat-1"]
        rig.rpc.replies["intro.suggest"] = ["suggestions": Self.suggestions.map { ["request": $0] }, "fits": true]
        rig.rpc.events["intro.preview"] = Self.previewEvents
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
                                         apps: apps ?? Self.mac(), sleeper: sleeper, clipSeconds: clipSeconds)
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
        XCTAssertFalse(t.sound.played.contains(.done))
        XCTAssertEqual(t.orb.flyHomes, 0)
        t.rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(accessibility: "denied", fda: "granted")
        await eventually { t.notch.introLine?.checklist.first { $0.id == "full_disk_access" }?.complete == true }
        XCTAssertEqual(t.orb.flyHomes, 0)
        t.notch.introAction(.choose("permission.accessibility"))
        await eventually { t.rig.opener.opened.contains { $0.absoluteString.hasSuffix("Privacy_Accessibility") } }
        t.rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(accessibility: "granted", fda: "granted")
        await eventually { t.progress.done }
        XCTAssertEqual(t.sound.played.filter { $0 == .done }.count, 1)
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
        XCTAssertFalse(t.sound.played.contains(.done))
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
        XCTAssertEqual(t.rig.rpc.methods().filter { $0 == "setup.browsers_pair_all" }.count, 1)
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

    // decision: APP-513
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

    // decision: APP-513
    func testTryShowsThePreviewAndActsNever() async {
        let t = tour()
        t.controller.start(beats: [.tryIt, .home])
        await t.pick("suggestion.0")
        await eventually { t.notch.conversation.turns.contains { $0.preview && $0.card?.rows.count == 2 } }
        XCTAssertTrue(t.rig.rpc.methods().contains("intro.suggest"))
        XCTAssertEqual(t.rig.rpc.params(of: "intro.preview").map { $0["request"] as? String },
                       [Self.suggestions[0]])
        let shown = t.notch.conversation.turns.first { $0.preview }
        XCTAssertEqual(shown?.request, Self.suggestions[0])
        XCTAssertEqual(shown?.card?.rows.map(\.state), ["preview", "preview"])   // each step marked a preview
        XCTAssertTrue(t.notch.expanded)                                          // in the real chat
        XCTAssertFalse(t.rig.rpc.methods().contains("chat.send"))
        await t.pick("otherSuggestion")
        await eventually { t.rig.rpc.params(of: "intro.preview").count == 2 }
        XCTAssertEqual(t.rig.rpc.params(of: "intro.preview").last?["request"] as? String, Self.suggestions[1])
        await t.pick("notNow")
        await eventually { t.progress.done }
        XCTAssertFalse(t.rig.rpc.methods().contains("chat.send"))
        XCTAssertFalse(t.notch.conversation.forStorage().turns.contains { $0.preview })   // never saved
    }

    // decision: APP-514
    func testDoItForRealStartsTheTaskAfterHome() async {
        let t = tour()
        t.sleeper.hold(IntroController.Timing.home)
        t.controller.start(beats: [.tryIt, .home])
        await t.pick("suggestion.1")
        await eventually { t.notch.conversation.turns.contains { $0.preview && $0.card != nil } }
        await t.pick("doItForReal")
        await eventually { t.controller.beat == .home && t.sleeper.isWaiting(IntroController.Timing.home) }
        XCTAssertFalse(t.rig.rpc.methods().contains("chat.send"))        // nothing runs during the tour
        t.sleeper.release(IntroController.Timing.home)
        await eventually { t.rig.rpc.methods().contains("chat.send") }
        XCTAssertTrue(t.progress.done)
        XCTAssertEqual(t.rig.rpc.params(of: "chat.send").first?["text"] as? String, Self.suggestions[1])
    }

    // MARK: Control

    // decision: APP-505, APP-514
    func testControlBeatWaitsForEachShortcut() async {
        let t = tour()
        t.controller.start(beats: [.control, .home])
        await eventually { t.controls.onControl != nil }
        t.controls.send(.toggle)
        t.controls.send(.talk)
        t.controls.send(.summon)
        // Stopping is practised on the preview, replayed as running.
        await eventually { t.notch.conversation.turns.contains { $0.preview && $0.status == "previewRunning" } }
        XCTAssertEqual(t.controller.beat, .control)
        t.notch.introAction(.stopPreview)
        await eventually { !t.notch.conversation.turns.contains { $0.status == "previewRunning" } }
        await eventually { t.notch.introLine == IntroCopy.control(.pause) }   // waiting on the last control
        XCTAssertEqual(t.controller.beat, .control)                       // Pause FERB not seen yet
        XCTAssertFalse(t.progress.done)
        t.notch.daemonPaused = true
        t.controls.send(.pause)
        await eventually { t.progress.done }
        XCTAssertTrue(t.rig.rpc.methods().contains("sense.resume"))       // the pause is lifted at once
        XCTAssertFalse(t.notch.daemonPaused)
        XCTAssertFalse(t.rig.rpc.methods().contains("chat.send"))
    }

    // MARK: Skip, resume, the notch

    // decision: APP-618
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

    // decision: APP-618
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
        await eventually { t.notch.expanded && t.notch.conversation.turns.contains { $0.preview && $0.card != nil } }
        await t.pick("notNow")
        await eventually { t.controls.onControl != nil }
        t.controls.send(.toggle)
        t.controls.send(.talk)
        t.controls.send(.summon)
        await eventually { t.notch.expanded && t.notch.conversation.turns.contains { $0.status == "previewRunning" } }
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
        let ready = t.lines.all.firstIndex { $0 == IntroCopy.gettingReady }
        let everything = t.lines.all.firstIndex { $0 == IntroCopy.allAllowed }
        XCTAssertNotNil(ready)
        XCTAssertLessThan(ready ?? .max, everything ?? -1)                // never "everything" before it answered
        XCTAssertEqual(t.lines.all.filter { $0 == IntroCopy.allAllowed }.count, 1)
    }

    // decision: APP-514
    func testSkipStillStartsARealTaskTheOwnerTapped() async {
        let t = tour()
        t.sleeper.hold(IntroController.Timing.home)
        t.controller.start(beats: [.tryIt, .control, .home])
        await t.pick("suggestion.0")
        await t.pick("doItForReal")
        await eventually { t.controller.beat == .control }
        XCTAssertFalse(t.rig.rpc.methods().contains("chat.send"))
        t.controller.skip()
        await eventually { t.rig.rpc.methods().contains("chat.send") }
        XCTAssertEqual(t.rig.rpc.params(of: "chat.send").first?["text"] as? String, Self.suggestions[0])
    }

    // decision: APP-618
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
    /// starting): the tour waits on "Getting my background part ready…" and starts the service, instead of reading the
    /// silence as signed out and asking for sign-in again (which also moved the resume point back to sign-in).
    // decision: APP-618, APP-598
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
        XCTAssertTrue(t.lines.all.contains(IntroCopy.gettingReady))
        XCTAssertFalse(t.lines.all.contains(IntroCopy.signIn))            // never asked to sign in again
        XCTAssertEqual(t.rig.service.starts, 1)
        XCTAssertTrue(t.rig.runner.runs.isEmpty)                          // Meet isn't run again
    }

    // MARK: Round 2

    // decision: APP-618
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

    // decision: APP-513
    func testTryPointsTheOrbAtTheAppThePreviewWouldUse() async {
        let t = tour()
        t.rig.rpc.replies["intro.suggest"] = ["suggestions": [["request": Self.suggestions[0], "app": "Finder"],
                                                              ["request": Self.suggestions[1], "app": "Notes"]],
                                              "fits": true]
        var shownWhenHome: [Bool] = []
        t.orb.onFlyHome = { [notch = t.notch] in
            shownWhenHome.append(notch.conversation.turns.contains { $0.preview && $0.card != nil })
        }
        t.controller.start(beats: [.tryIt, .home])
        await t.pick("suggestion.0")
        await eventually { t.orb.launches.count == 1 }
        XCTAssertEqual(t.orb.launches.map(\.point), [Self.besideFinder])          // beside Finder's window
        XCTAssertEqual(t.orb.launches.first?.instruction?.text, "I'd use Finder for this")
        XCTAssertTrue(shownWhenHome.isEmpty)                                   // stays out while the owner compares
        await t.pick("otherSuggestion")
        await eventually { t.orb.launches.count == 2 }
        XCTAssertEqual(t.orb.launches.last?.point, Self.resting)                // Notes isn't running
        XCTAssertEqual(t.orb.launches.last?.instruction?.text, "I'd use Notes for this")
        XCTAssertEqual(t.orb.launches.last?.instruction?.iconPath, "/System/Applications/Notes.app")
        XCTAssertEqual(t.orb.launches.count, 2)
        await t.pick("notNow")
        await eventually { t.progress.done }
        XCTAssertEqual(t.orb.flyHomes, 1)
        XCTAssertEqual(shownWhenHome, [true])
    }

    // decision: APP-511
    func testTheBrowsersBeatPointsTheOrbWhilePairing() async {
        let t = tour(apps: Self.mac(browser: true))
        t.rig.rpc.replies["setup.browsers"] = ["browsers": [["browser": "example", "label": "Example Browser",
                                                              "profiles": [[String: Any]]()]]]
        var pairedWhenHome: [Bool] = []
        t.orb.onFlyHome = { [rpc = t.rig.rpc] in pairedWhenHome.append(rpc.methods().contains("setup.browsers_pair_all")) }
        t.controller.start(beats: [.browsers, .home])
        await t.pick("pairAll")
        await eventually { t.progress.done }
        XCTAssertEqual(t.orb.launches.count, 1)
        XCTAssertEqual(t.orb.launches.first?.instruction, IntroCopy.pairingOrb)
        XCTAssertEqual(t.orb.launches.first?.point, CGPoint(x: 720, y: 791))    // inside a window too wide to stand beside
        XCTAssertEqual(pairedWhenHome, [true])                                  // home after the pair-all reply
    }

    // MARK: Final review

    /// The sound engine is shared with everyday sounds: the real orb's teardown stops only its own motion, and only a
    /// skip stops what is playing; the natural end lets a ringing sound finish.
    // decision: APP-618
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

    // decision: APP-514
    func testDoItForRealWaitsForDictationToEndThenSends() async {
        let t = tour()
        t.sleeper.hold(IntroController.Timing.home)
        var ended = 0
        t.controller.onFinished = { ended += 1 }
        t.controller.start(beats: [.tryIt, .home])
        await t.pick("suggestion.1")
        await t.pick("doItForReal")
        await eventually { t.controller.beat == .home && t.sleeper.isWaiting(IntroController.Timing.home) }
        t.notch.session.beginVoicePreview()                               // the owner is dictating as the tour ends
        t.sleeper.release(IntroController.Timing.home)
        await eventually { ended == 1 }
        XCTAssertFalse(t.notch.conversation.turns.contains { !$0.preview && $0.request == Self.suggestions[1] })
        t.notch.session.finishVoicePreview()
        await eventually { t.rig.rpc.methods().contains("chat.send") }
        XCTAssertEqual(t.rig.rpc.params(of: "chat.send").map { $0["text"] as? String }, [Self.suggestions[1]])
    }
}

/// Where the intro's orb points for an app, from injected sources (no real app is looked up, opened or brought forward).
@MainActor
final class IntroAppTargetsTests: XCTestCase {
    // decision: APP-594
    func testAModelNamedAppIsNeverAPath() {
        var probed: [String] = []
        let exists: (String) -> Bool = { probed.append($0); return true }
        for name in ["../x", "a/b", ".hidden", "Notes:x", "No\0tes", ""] {
            XCTAssertNil(IntroAppResolver.installedApp(named: name, folders: ["/Applications"], exists: exists), name)
        }
        XCTAssertEqual(probed, [])                                              // refused before anything is looked at
        XCTAssertEqual(IntroAppResolver.installedApp(named: "Notes", folders: ["/Applications", "/System/Applications"],
                                                     exists: { $0 == "/System/Applications/Notes.app" }),
                       "/System/Applications/Notes.app")
    }
}
