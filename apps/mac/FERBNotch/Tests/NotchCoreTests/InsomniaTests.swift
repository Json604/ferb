import AppKit
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// Insomnia mode, FERB.app's half (T-1274): the daemon's status as FERB.app reads it, a person's input told apart from
/// FERB's, whose an app's activation is, the cover's look over time, the heartbeat, a step's click-through and the
/// warning's start. A stand-in surface records what the cover's windows would do: no window is ordered on screen, no
/// event tap is made and no camera starts.
@MainActor
final class InsomniaTests: XCTestCase {
    private var time = Date(timeIntervalSince1970: 1_800_000_000)
    private let rpc = ScriptedRPC()
    private let surface = CoverStandIn()
    private lazy var clickThrough = ClickThroughReport(rpc: rpc)
    private static let ferb: Int32 = 4242

    private func controller(cover: Bool = true, screenYes: Bool = true) -> InsomniaController {
        let controller = InsomniaController(rpc: rpc, clickThrough: clickThrough,
                                            ownBundle: "com.ferb.FERB", ownPID: Self.ferb, now: { [unowned self] in self.time })
        controller.surface = surface
        if cover { controller.apply(Self.on(screenYes: screenYes)) }
        return controller
    }

    private static func on(screenYes: Bool = true, line: String = "Keeping your Mac working · 2 tasks running",
                           notice: [String: Any]? = nil, since: String = "2026-11-20T15:00:00+05:30") -> [String: Any] {
        var status: [String: Any] = ["on": true, "variant": "shield", "since": since, "by": "mac",
                                     "screen_yes": screenYes, "grace_minutes": 10, "working": 2, "ends_at": NSNull(),
                                     "shield_up": true, "mac_locked": false, "line": line,
                                     "camera": ["on": false, "state": "allowed"]]
        if let notice { status["notice"] = notice }
        return status
    }

    private func advance(_ seconds: TimeInterval) { time = time.addingTimeInterval(seconds) }

    private var reports: [[String: Any]] {
        rpc.calls.filter { $0.0 == "insomnia.report" }.map(\.1)
    }

    private func events(_ name: String) -> [[String: Any]] { reports.filter { $0["event"] as? String == name } }

    // MARK: The status

    // decision: APP-980
    func testTheDaemonsStatusIsReadWithEveryField() throws {
        let status = try XCTUnwrap(InsomniaStatus([
            "on": true, "variant": "shield", "since": "2026-11-20T15:00:00+05:30", "by": "mac", "screen_yes": true,
            "grace_minutes": 15, "working": 2, "ends_at": NSNull(), "shield_up": true, "mac_locked": false,
            "camera": ["on": true, "state": "allowed"],
            "power": ["battery": 64, "on_battery": true, "lid_closed": false, "thermal": "nominal"],
            "warnings": [["id": "w-1", "kind": "battery_low", "text": "Your Mac's battery is at 18%.",
                          "at": "2026-11-20T15:10:00+05:30"]],
            "notice": ["id": "n-1", "kind": "touched", "at": "2026-11-20T15:11:00+05:30",
                       "text": "Someone is using your Mac's keyboard or trackpad.", "app": NSNull(), "count": 3],
            "line": "Keeping your Mac working · 2 tasks running · battery 64%", "last_end": NSNull(),
        ]))
        XCTAssertTrue(status.on)
        XCTAssertTrue(status.wantsCover)
        XCTAssertEqual(status.since, "2026-11-20T15:00:00+05:30")
        XCTAssertTrue(status.screenYes)
        XCTAssertEqual(status.graceMinutes, 15)
        XCTAssertEqual(status.working, 2)
        XCTAssertNil(status.endsAt)
        XCTAssertEqual(status.camera, .init(on: true, state: "allowed"))
        XCTAssertEqual(status.power, .init(battery: 64, onBattery: true, lidClosed: false, thermal: "nominal"))
        XCTAssertEqual(status.warnings.map(\.kind), ["battery_low"])
        XCTAssertNotNil(status.warnings.first?.at)
        XCTAssertEqual(status.notice?.kind, "touched")
        XCTAssertEqual(status.notice?.count, 3)
        XCTAssertNil(status.notice?.app)
        XCTAssertEqual(status.line, "Keeping your Mac working · 2 tasks running · battery 64%")

        let locked = try XCTUnwrap(InsomniaStatus(["on": true, "variant": "locked", "since": "x"]))
        XCTAssertFalse(locked.wantsCover)                       // Lock my Mac: no cover

        let off = try XCTUnwrap(InsomniaStatus([
            "on": false, "power": ["battery": NSNull(), "on_battery": NSNull()], "task_awake": true, "line": "",
            "last_end": ["id": "e-1", "why": "back", "text": "Welcome back.", "at": "2026-11-20T16:00:00+05:30"],
        ]))
        XCTAssertFalse(off.on)
        XCTAssertFalse(off.wantsCover)
        XCTAssertTrue(off.taskAwake)
        XCTAssertNil(off.power.battery)
        XCTAssertEqual(off.lastEnd?.why, "back")
        XCTAssertEqual(off.graceMinutes, InsomniaStatus.defaultGraceMinutes)
        XCTAssertNil(InsomniaStatus("not an object"))
        XCTAssertNil(InsomniaStatus(nil))
    }

    // MARK: A person's input

    // decision: APP-983
    func testAPersonsInputIsToldFromSyntheticInputByItsSourceFields() {
        // a person: source pid 0 and the HID system state (1), whatever the kind
        XCTAssertEqual(PersonInput.classify(eventType: 10, sourcePID: 0, sourceStateID: 1), .keyboard)
        XCTAssertEqual(PersonInput.classify(eventType: 12, sourcePID: 0, sourceStateID: 1), .keyboard)
        XCTAssertEqual(PersonInput.classify(eventType: 1, sourcePID: 0, sourceStateID: 1), .pointer)
        XCTAssertEqual(PersonInput.classify(eventType: 5, sourcePID: 0, sourceStateID: 1), .pointer)
        XCTAssertEqual(PersonInput.classify(eventType: 22, sourcePID: 0, sourceStateID: 1), .pointer)
        XCTAssertEqual(PersonInput.classify(eventType: 29, sourcePID: 0, sourceStateID: 1), .trackpad)
        XCTAssertEqual(PersonInput.classify(eventType: 30, sourcePID: 0, sourceStateID: 1), .trackpad)
        // FERB's own events: its pid and a private source state
        XCTAssertNil(PersonInput.classify(eventType: 10, sourcePID: 4242, sourceStateID: 0x3E8_1234))
        XCTAssertNil(PersonInput.classify(eventType: 1, sourcePID: 4242, sourceStateID: 0x3E8_1234))
        // another program's synthetic events, even one claiming the HID state or pid 0 alone
        XCTAssertNil(PersonInput.classify(eventType: 10, sourcePID: 777, sourceStateID: 1))
        XCTAssertNil(PersonInput.classify(eventType: 10, sourcePID: 0, sourceStateID: 2))
        // not an input at all
        XCTAssertNil(PersonInput.classify(eventType: 0, sourcePID: 0, sourceStateID: 1))
    }

    // decision: APP-983
    func testAPersonsInputIsReportedAtMostOnceEveryTwoSecondsAndFERBsNever() async {
        let controller = controller()
        controller.sawInput(eventType: 10, sourcePID: Int64(Self.ferb), sourceStateID: 99)   // FERB typing: nothing
        controller.sawInput(eventType: 10, sourcePID: 0, sourceStateID: 1)
        advance(1)
        controller.sawInput(eventType: 1, sourcePID: 0, sourceStateID: 1)                    // within 2 s: not again
        advance(1)
        controller.sawInput(eventType: 29, sourcePID: 0, sourceStateID: 1)                   // 2 s after: again
        advance(0.5)
        controller.sawInput(eventType: 5, sourcePID: 0, sourceStateID: 1)
        await controller.settled()
        XCTAssertEqual(events("person_input").map { $0["input"] as? String }, ["keyboard", "trackpad"])
        XCTAssertEqual(events("person_input").map { $0["during_step"] as? Bool }, [false, false])
    }

    // MARK: Activations

    // decision: APP-983, APP-984, APP-994
    func testAnActivationFERBDidntAnnounceIsReportedAndTheFrontTakenBack() async {
        let controller = controller()
        surface.fronts = 0
        controller.activated(bundle: "com.ferb.FERB", name: "FERB", pid: Self.ferb)          // FERB.app itself
        XCTAssertEqual(surface.fronts, 0)

        controller.receive(topic: "insomnia.expect", data: ["bundle": "com.apple.TextEdit", "seconds": 5])
        advance(4)
        controller.activated(bundle: "com.apple.TextEdit", name: "TextEdit", pid: 501)        // announced, in time
        XCTAssertEqual(surface.fronts, 1)                                                      // front back, no report
        advance(2)
        controller.activated(bundle: "com.apple.TextEdit", name: "TextEdit", pid: 501)        // its seconds are over
        XCTAssertEqual(surface.fronts, 2)

        controller.activated(bundle: "com.apple.LocalAuthentication.UIAgent", name: "Touch ID", pid: 77)  // nobody asked
        XCTAssertEqual(surface.fronts, 3)
        surface.authResult = nil                                                               // FERB.app asks now
        controller.unlock()
        controller.activated(bundle: "com.apple.LocalAuthentication.UIAgent", name: "Touch ID", pid: 77)
        XCTAssertEqual(surface.fronts, 3)                                                      // the prompt keeps it
        surface.finishAuth(false)                                                              // cancelled
        XCTAssertEqual(surface.fronts, 4)                                                      // the cover comes back

        await controller.settled()
        XCTAssertEqual(events("activation").map { $0["bundle"] as? String },
                       ["com.apple.TextEdit", "com.apple.LocalAuthentication.UIAgent"])
        XCTAssertEqual(events("activation").first?["app"] as? String, "TextEdit")
    }

    // decision: APP-984, APP-994
    func testTheStepsAppKeepsTheFrontAndAPersonDuringTheStepIsSaidSo() async {
        let controller = controller()
        controller.receive(topic: "insomnia.step", data: ["step": "s-1", "phase": "begin", "pid": 501,
                                                          "bundle": "com.apple.TextEdit"])
        surface.fronts = 0
        controller.activated(bundle: "com.apple.TextEdit", name: "TextEdit", pid: 501)
        controller.activated(bundle: "com.example.Other", name: "Other", pid: 600)            // reported, front kept
        XCTAssertEqual(surface.fronts, 0)
        controller.sawInput(eventType: 1, sourcePID: 0, sourceStateID: 1)
        await controller.settled()
        XCTAssertEqual(events("activation").map { $0["bundle"] as? String }, ["com.example.Other"])
        XCTAssertEqual(events("person_input").last?["during_step"] as? Bool, true)
    }

    // MARK: The look (APP-982)

    // decision: APP-982
    func testTheLookDimsTenMinutesAfterTheLastEventAndEveryEventBringsItBack() {
        let controller = controller()
        XCTAssertEqual(surface.looks, [false])
        advance(599); controller.tick()
        XCTAssertFalse(controller.dimmed)
        advance(1); controller.tick()
        XCTAssertTrue(controller.dimmed)                                    // exactly 10 minutes with no event
        XCTAssertEqual(surface.looks, [false, true])

        var wakers: [(String, () -> Void)] = []
        wakers.append(("a person's input", { controller.sawInput(eventType: 10, sourcePID: 0, sourceStateID: 1) }))
        wakers.append(("a notice", {
            controller.apply(Self.on(notice: ["id": "n-1", "kind": "touched", "text": "Someone…", "count": 1]))
        }))
        wakers.append(("a status change", { controller.apply(Self.on(line: "Keeping your Mac working · battery 63%")) }))
        wakers.append(("a step", {
            controller.receive(topic: "insomnia.step", data: ["step": "s-9", "phase": "begin", "bundle": "com.x"])
        }))
        wakers.append(("the step's end", { controller.receive(topic: "insomnia.step", data: ["step": "s-9", "phase": "end"]) }))
        for (what, wake) in wakers {
            wake()
            XCTAssertFalse(controller.dimmed, "\(what) brings the look back")
            advance(599); controller.tick()
            XCTAssertFalse(controller.dimmed, "\(what) restarts the 10 minutes")
            advance(1); controller.tick()
            XCTAssertTrue(controller.dimmed, "\(what): dimmed again 10 minutes later")
        }
        // an unchanged status (the 5-second read) is no event
        controller.apply(Self.on(line: "Keeping your Mac working · battery 63%"))
        XCTAssertTrue(controller.dimmed)
    }

    // MARK: The heartbeat

    // decision: APP-980
    func testTheCoverSaysItIsUpAtEachChangeAndEveryThirtySeconds() async {
        let controller = controller()
        advance(29); controller.tick()
        advance(1); controller.tick()
        advance(29); controller.tick()
        advance(1); controller.tick()
        controller.apply(["on": false, "line": ""])
        await controller.settled()
        XCTAssertEqual(events("shield").map { $0["state"] as? String }, ["up", "up", "up", "down"])
        XCTAssertEqual(events("shield").first?["camera"] as? String, "allowed")
        XCTAssertEqual(surface.calls.filter { $0 == "raise" || $0 == "drop" }, ["raise", "drop"])
        // a restarted FERB.app reads the status and raises its cover again; a locked Mac gets none
        let again = self.controller(cover: false)
        again.apply(Self.on())
        XCTAssertTrue(again.coverUp)
        again.apply(["on": true, "variant": "locked", "since": "y"])
        XCTAssertFalse(again.coverUp)
    }

    // MARK: A step under the cover

    // decision: APP-984
    func testAStepMakesTheCoverClickThroughBeforeItSaysReadyAndItsEndTakesTheFrontBack() async {
        let controller = controller()
        surface.fronts = 0
        controller.receive(topic: "insomnia.step", data: ["step": "s-1", "phase": "begin", "pid": 501,
                                                          "bundle": "com.apple.TextEdit"])
        await controller.settled()
        XCTAssertEqual(surface.clickThrough, true)
        controller.receive(topic: "insomnia.step", data: ["step": "s-other", "phase": "end"])   // not this step
        XCTAssertEqual(surface.clickThrough, true)
        controller.receive(topic: "insomnia.step", data: ["step": "s-1", "phase": "end"])
        await controller.settled()
        await clickThrough.settled()
        XCTAssertEqual(surface.clickThrough, false)
        XCTAssertEqual(surface.fronts, 1)
        let order = rpc.calls.compactMap { call -> String? in
            switch call.0 {
            case "ui.click_through": return "click_through \(call.1["windows"] as? [Int] ?? [-1])"
            case "insomnia.report" where call.1["event"] as? String != "shield":
                return "\(call.1["event"] ?? "") \(call.1["step"] ?? "")"
            default: return nil
            }
        }
        XCTAssertEqual(order, ["click_through [701, 702]", "step_ready s-1", "click_through []"])
    }

    // decision: APP-984
    func testAStepIsRefusedOnlyWithoutTheCover() async {
        // a step's own Yes (his answer from the phone) lets it through as the session's one Yes does
        let noYes = controller(screenYes: false)
        noYes.beginStep("s-1", pid: 501, bundle: "com.apple.TextEdit")
        await noYes.settled()
        XCTAssertEqual(events("step_ready").map { $0["step"] as? String }, ["s-1"])
        noYes.endStep("s-1")
        let noCover = controller(cover: false)
        noCover.beginStep("s-2", pid: 501, bundle: "com.apple.TextEdit")
        await noCover.settled()
        XCTAssertEqual(events("step_failed").map { $0["step"] as? String }, ["s-2"])
    }

    // MARK: Unlock

    // decision: APP-980, APP-993
    func testTheOwnersUnlockDropsTheCoverAtOnceAndEndsTheModeAsBack() async {
        let controller = controller()
        rpc.failures["insomnia.end"] = DaemonRPCError.unavailable      // the daemon doesn't answer: the cover goes anyway
        let daemon = rpc
        var tries = 0
        controller.endRetryPause = { @MainActor in          // the third try reaches it
            tries += 1
            if tries == 2 {
                daemon.failures["insomnia.end"] = nil
                daemon.replies["insomnia.end"] = ["on": false, "line": "", "ended": true, "by": "mac"]
            }
        }
        surface.authResult = true
        controller.unlock()
        XCTAssertFalse(controller.coverUp)
        controller.apply(Self.on(line: "Keeping your Mac working · 1 task running"))   // the same session, still on
        XCTAssertFalse(controller.coverUp)                                              // never raised again
        for _ in 0..<1000 where controller.status?.on != false { await Task.yield() }
        XCTAssertEqual(controller.status?.on, false)
        let ends = rpc.calls.filter { $0.0 == "insomnia.end" }
        XCTAssertEqual(ends.count, 3)
        XCTAssertTrue(ends.allSatisfy { $0.1["why"] as? String == "back" })
        // a new session raises it again
        controller.apply(Self.on(since: "2026-11-20T18:00:00+05:30"))
        XCTAssertTrue(controller.coverUp)
    }

    // MARK: The warning and the menu

    // decision: APP-980, APP-984
    func testTheWarningStartsWithTheOwnersChoice() {
        XCTAssertEqual(InsomniaChoice().startParams as NSDictionary, ["variant": "shield", "screen_yes": true])
        XCTAssertEqual(InsomniaChoice(variant: .shield, screenYes: false).startParams as NSDictionary,
                       ["variant": "shield", "screen_yes": false])
        XCTAssertEqual(InsomniaChoice(variant: .locked, screenYes: true).startParams as NSDictionary,
                       ["variant": "locked", "screen_yes": false])     // the Yes is about apps under the cover

        let form = InsomniaDialog.Form()                                 // its buttons only: nothing is shown
        XCTAssertEqual(form.choice, InsomniaChoice(variant: .shield, screenYes: true))   // the defaults
        form.pick(.locked)
        XCTAssertFalse(form.screenYes.isEnabled)
        XCTAssertEqual(form.choice.startParams as NSDictionary, ["variant": "locked", "screen_yes": false])
        form.pick(.shield)
        form.screenYes.state = .off
        XCTAssertEqual(form.choice.startParams as NSDictionary, ["variant": "shield", "screen_yes": false])
        XCTAssertEqual(InsomniaWords.ends(minutes: 10),
                       "This ends when you're back, when you end it, or 10 minutes after FERB's work is done.")
    }

    // decision: APP-980
    func testTheMenuOffersTheModeOnlyWhenTheDaemonHasIt() {
        func menu(_ keeping: Bool?) -> [MenuEntry] {
            StatusMenu.entries(state: .ready, paused: false, stopped: false, appName: "FERB", canSetUp: false,
                               hasWindow: true, keepingAwake: keeping)
        }
        XCTAssertFalse(menu(nil).contains { if case .item(_, .keepWorking, _) = $0 { return true }; return false })
        XCTAssertTrue(menu(false).contains(.item("Keep my Mac working…", .keepWorking, key: "")))
        XCTAssertTrue(menu(true).contains(.item("End keeping my Mac working", .endKeepWorking, key: "")))
    }

    // decision: APP-980
    func testTheNotchFollowsTheModeFromTheBus() {
        let model = NotchModel(rpc: rpc, preferences: scratchDefaults("ferb-insomnia"))
        XCTAssertFalse(model.showsInsomniaLine)
        model.receiveBusEvent(["topic": "insomnia.changed", "data": Self.on()])
        XCTAssertTrue(model.showsInsomniaLine)
        XCTAssertEqual(model.insomniaLine, "Keeping your Mac working · 2 tasks running")
        XCTAssertFalse(model.insomnia.coverUp)                           // no cover windows outside FERB.app
        model.receiveBusEvent(["topic": "insomnia.changed", "data": ["on": false, "line": ""]])
        XCTAssertFalse(model.showsInsomniaLine)
    }

    // decision: APP-997, APP-1107
    func testTheEndIsSaidOnceAsFERBsOwnLine() {
        let model = NotchModel(rpc: rpc, preferences: scratchDefaults("ferb-insomnia-end"))
        let off: [String: Any] = ["on": false, "line": "", "last_end": ["id": "e-1", "why": "back",
                                                                          "text": "Welcome back.", "at": "2026-11-20T16:00:00+05:30"]]
        model.receiveBusEvent(["topic": "insomnia.changed", "data": Self.on()])
        model.receiveBusEvent(["topic": "insomnia.changed", "data": off])
        model.receiveBusEvent(["topic": "insomnia.changed", "data": Self.on(since: "later")])
        model.receiveBusEvent(["topic": "insomnia.changed", "data": off])              // the same end again
        // an alert above the composer (APP-1107), never a line kept in the chat
        XCTAssertEqual(model.alerts.filter { $0.id == "insomnia-end-e-1" }.map(\.text), ["Welcome back."])
        XCTAssertTrue(model.conversation.notices.isEmpty)
    }

    // decision: APP-995
    func testTheCameraIsAskedWhenTheOwnerStartsTheCoverAndBeforeTheStart() async {
        let model = NotchModel(rpc: rpc, preferences: scratchDefaults("ferb-insomnia-camera"))
        let saved = InsomniaCamera.askAccessIfNeeded
        defer { InsomniaCamera.askAccessIfNeeded = saved }
        let daemon = rpc
        let asked = AskLog()
        InsomniaCamera.askAccessIfNeeded = { asked.note(daemon.calls.filter { $0.0 == "insomnia.start" }.count) }
        model.startInsomnia(InsomniaChoice(variant: .shield))
        for _ in 0..<1000 where !rpc.calls.contains(where: { $0.0 == "insomnia.start" }) { await Task.yield() }
        model.startInsomnia(InsomniaChoice(variant: .locked))
        for _ in 0..<1000 where rpc.calls.filter({ $0.0 == "insomnia.start" }).count < 2 { await Task.yield() }
        XCTAssertEqual(asked.starts, [0])                 // asked once, for the cover, before its start went out
    }

    // decision: APP-990
    func testTheCoverTakesTheFrontOnlyThroughTheSpaceRule() {
        let saved = AppWindowPresentation.activate
        defer { AppWindowPresentation.activate = saved }
        var activated = 0
        AppWindowPresentation.activate = { activated += 1 }
        AppWindowPresentation.coverTakesFront()
        XCTAssertEqual(activated, 1)
        XCTAssertFalse(SourcePresenters.activations.contains { $0.hasPrefix("InsomniaCover.swift:") })
    }

    // decision: APP-991, APP-1003
    func testUnderTheCoverOnlyTheCoverTakesClicksAndEveryKeyAsksForTouchID() {
        typealias G = InsomniaCoverWindows
        XCTAssertEqual(G.gate(.keyDown, onCover: false).passes, false)          // the notch never gets typing
        XCTAssertEqual(G.gate(.keyDown, onCover: false).asks, true)
        XCTAssertEqual(G.gate(.keyDown, onCover: true).passes, false)
        XCTAssertEqual(G.gate(.flagsChanged, onCover: true).asks, false)
        XCTAssertEqual(G.gate(.leftMouseDown, onCover: false).passes, false)    // a FERB window beneath: dropped
        XCTAssertEqual(G.gate(.leftMouseDown, onCover: false).asks, false)
        XCTAssertEqual(G.gate(.leftMouseDown, onCover: true).asks, true)        // a click on the cover asks
        XCTAssertEqual(G.gate(.mouseMoved, onCover: true).passes, true)
        XCTAssertEqual(G.gate(.mouseMoved, onCover: true).touches, true)        // and wakes the look
        XCTAssertEqual(G.gate(.mouseMoved, onCover: false).passes, false)
    }

    // decision: APP-992
    func testTheTapWatchesKeysOnlyWithInputMonitoringAlreadyGranted() {
        let without = InsomniaCoverWindows.tapMask(listenAccess: false)
        let with = InsomniaCoverWindows.tapMask(listenAccess: true)
        XCTAssertEqual(without & InsomniaCoverWindows.keyMask, 0)
        XCTAssertNotEqual(without & (1 << 1), 0)                                 // clicks
        XCTAssertNotEqual(without & (1 << 29), 0)                                // trackpad gestures
        XCTAssertEqual(with, without | InsomniaCoverWindows.keyMask)
    }

    // MARK: The cover's windows

    // decision: APP-980, APP-988, APP-989, APP-1100
    func testTheCoverIsAShieldingPanelOnEverySpaceAndEveryApp() {
        let panel = ShieldPanel.make(frame: CGRect(x: 0, y: 0, width: 400, height: 300))   // never ordered in
        XCTAssertEqual(panel.level.rawValue, Int(CGShieldingWindowLevel()))
        XCTAssertTrue(panel.collectionBehavior.contains(.canJoinAllApplications))
        XCTAssertTrue(panel.collectionBehavior.contains(.canJoinAllSpaces))
        XCTAssertTrue(panel.collectionBehavior.contains(.fullScreenAuxiliary))
        XCTAssertTrue(panel.canBecomeKey)
        XCTAssertTrue(panel.worksWhenModal)
        XCTAssertFalse(panel.ignoresMouseEvents)
        XCTAssertFalse(panel.isVisible)
        XCTAssertTrue(panel.isOpaque)                                       // nothing behind it ever shows through
        XCTAssertEqual(panel.backgroundColor, .black)
        XCTAssertTrue(ShieldLookInUse.self == FrostedGrainLook.self)        // the one swap point
        XCTAssertLessThan(ShieldPanel.askingLevel.rawValue, 1000)          // under the system's Touch ID prompt
        // FERB is a regular app while the cover is up, active or not, and goes back by APP-600's rule after
        XCTAssertEqual(DockPresence.change(wants: false, current: .accessory, active: true, cover: true), .regular)
        XCTAssertNil(DockPresence.change(wants: false, current: .regular, active: false, cover: true))
        XCTAssertEqual(DockPresence.change(wants: false, current: .regular, active: true, cover: false), .accessory)
    }
}

/// Insomnia mode's root helper (APP-986) as FERB.app registers it, through a stand-in for SMAppService: nothing real is
/// registered.
@MainActor
final class PowerHelperTests: XCTestCase {
    private var reports: [[String: Any]] = []
    private var logged: [String] = []

    private func helper(_ macOS: HelperStandIn) -> PowerHelper {
        let helper = PowerHelper(registrar: macOS) { [unowned self] params in self.reports.append(params) }
        helper.log = { [unowned self] line in self.logged.append(line) }
        return helper
    }

    private var states: [String?] { reports.map { $0["state"] as? String } }

    private static func status(_ fields: [String: Any]) -> InsomniaStatus {
        InsomniaStatus(fields.merging(["since": "2026-11-20T15:00:00+05:30"]) { old, _ in old })!
    }

    // decision: APP-986, APP-996, APP-1103, APP-1104
    func testAHelperMacOSDoesntListIsRegisteredWhenTheBundleCarriesIt() async {
        for listed in [ServiceStatus.notRegistered, .notFound] {       // macOS says notFound for one never registered
            let macOS = HelperStandIn(listed, afterRegister: .requiresApproval)
            let said = await helper(macOS).prepare()
            XCTAssertEqual(said, .needsApproval, "\(listed)")
            XCTAssertEqual(macOS.calls, ["register"], "\(listed)")
        }
        XCTAssertEqual(states, ["requires_approval", "requires_approval"])   // the state read after register()
        let enabled = HelperStandIn(.notFound, afterRegister: .enabled)
        await helper(enabled).attempt()
        XCTAssertEqual(states.last, "enabled")
        XCTAssertEqual(InsomniaDialog.buttons(.needsApproval), ["Keep it working", "Not now", "Open Login Items…"])
    }

    // decision: APP-986, APP-996, APP-1103
    func testWithoutTheFilesNothingIsRegisteredAndTheDaemonIsTold() async {
        let macOS = HelperStandIn(.notFound)
        macOS.inBundle = false
        let said = await helper(macOS).prepare()
        XCTAssertEqual(said, .sleeps)
        XCTAssertEqual(macOS.calls, [])
        XCTAssertEqual(states, ["not_in_bundle"])
        XCTAssertEqual(logged.count, 1)
    }

    // decision: APP-986, APP-996
    func testAHelperWaitingForApprovalOrOnIsLeftAsItIsAndItsStateTold() async {
        for (status, said) in [(ServiceStatus.requiresApproval, PowerHelperPlan.ClosedLid.needsApproval),
                               (.enabled, .keepsWorking)] {
            let macOS = HelperStandIn(status)
            let closedLid = await helper(macOS).prepare()
            XCTAssertEqual(closedLid, said, "\(status)")
            XCTAssertEqual(macOS.calls, [], "\(status): nothing registered")
        }
        XCTAssertEqual(states, ["requires_approval", "enabled"])
        XCTAssertEqual(InsomniaDialog.buttons(.keepsWorking), ["Keep it working", "Not now"])
    }

    // decision: APP-986, APP-996, APP-1104
    func testAFailedRegistrationIsLoggedToldAndShownAndNeverStopsTheStart() async {
        let macOS = HelperStandIn(.notFound)
        macOS.registerFails = true
        let said = await helper(macOS).prepare()
        XCTAssertEqual(said, .needsApproval)                                     // the warning still shows
        XCTAssertEqual(states, ["failed"])
        let words = reports.first?["error"] as? String
        XCTAssertEqual(words, "The operation couldn’t be completed. (SMAppServiceErrorDomain error 1.) (SMAppServiceErrorDomain 1)")
        XCTAssertTrue(logged.first?.contains("SMAppServiceErrorDomain 1") ?? false)
        // the daemon's plain words come back in its status, and the warning and the menu show them
        let status = Self.status(["on": false, "lid_line": "The lid line.",
                                  "helper": ["state": "failed", "error": words ?? "", "problem": "FERB couldn't add its helper."]])
        XCTAssertEqual(status.helper?.problem, "FERB couldn't add its helper.")
        let text = InsomniaDialog.informative(minutes: 10, lidLine: status.lidLine!, problem: status.helper?.problem)
        XCTAssertTrue(text.contains("The lid line.\nFERB couldn't add its helper.\n\n"))
        let menu = StatusMenu.entries(state: .ready, paused: false, stopped: false, appName: "FERB", canSetUp: false,
                                      hasWindow: true, keepingAwake: false, helperProblem: status.helper?.problem)
        XCTAssertTrue(menu.contains(.status("FERB couldn't add its helper.")))
        XCTAssertNil(Self.status(["on": false]).helper)
    }

    // decision: APP-986, APP-996, APP-1101
    func testAModeStartedFromThePhoneTriesOncePerSession() async {
        let macOS = HelperStandIn(.notFound, afterRegister: .requiresApproval)
        let helper = helper(macOS)
        await helper.modeChanged(Self.status(["on": false, "closed_lid": "absent"]))     // off: nothing
        XCTAssertEqual(macOS.calls, [])
        await helper.modeChanged(Self.status(["on": true, "by": "phone-1", "closed_lid": "absent"]))
        XCTAssertEqual(macOS.calls, ["register"])
        XCTAssertEqual(states, ["requires_approval"])
        macOS.status = .notFound                                                        // macOS still doesn't list it
        await helper.modeChanged(Self.status(["on": true, "closed_lid": "unknown", "line": "changed"]))
        XCTAssertEqual(macOS.calls, ["register"])                                      // not twice in one session
        await helper.modeChanged(Self.status(["on": true, "closed_lid": "absent", "since": "2026-11-21T09:00:00+05:30"]))
        XCTAssertEqual(macOS.calls, ["register", "register"])                          // the next session tries again
        macOS.status = .notFound
        macOS.registerFails = true
        await helper.modeChanged(Self.status(["on": true, "closed_lid": "absent", "since": "2026-11-22T09:00:00+05:30"]))
        XCTAssertEqual(states.last, "failed")
    }

    // decision: APP-986, APP-996
    func testNoRegistrationWhenTheDaemonHasItReadyOrTheBundleLacksIt() async {
        let ready = HelperStandIn(.notRegistered)
        await helper(ready).modeChanged(Self.status(["on": true, "closed_lid": "ready"]))
        XCTAssertEqual(ready.calls, [])
        XCTAssertEqual(reports.count, 0)
        let noFiles = HelperStandIn(.notFound)
        noFiles.inBundle = false
        await helper(noFiles).modeChanged(Self.status(["on": true, "closed_lid": "absent"]))
        XCTAssertEqual(noFiles.calls, [])
        XCTAssertEqual(states, ["not_in_bundle"])
    }

    // decision: APP-986, APP-996, APP-1102
    func testTheWarningSaysTheDaemonsLidLineAndFallsBackTruthfully() {
        let daemon = Self.status(["on": false, "closed_lid": "absent", "lid_line": "The daemon's sentence."])
        XCTAssertEqual(InsomniaWords.lidLine(daemon, local: .keepsWorking), "The daemon's sentence.")
        let older = Self.status(["on": false])
        XCTAssertNil(older.lidLine)
        XCTAssertEqual(InsomniaWords.lidLine(older, local: .keepsWorking), InsomniaWords.closedLid(.sleeps))
        XCTAssertEqual(InsomniaWords.lidLine(older, local: .needsApproval), InsomniaWords.closedLid(.needsApproval))
        XCTAssertEqual(InsomniaWords.lidLine(Self.status(["on": false, "closed_lid": "ready"]), local: .keepsWorking),
                       InsomniaWords.closedLid(.keepsWorking))
        XCTAssertEqual(InsomniaWords.lidLine(nil, local: .sleeps), InsomniaWords.closedLid(.sleeps))
        let text = InsomniaDialog.informative(minutes: 10, lidLine: "The lid line.")
        XCTAssertTrue(text.hasPrefix(InsomniaWords.body + "\n" + "The lid line.\n\n"))
        XCTAssertTrue(text.hasSuffix(InsomniaWords.ends(minutes: 10)))
    }

    // decision: APP-986, APP-996, APP-5
    func testUninstallUnregistersTheHelperAndStopsWhenMacOSKeepsIt() async {
        let listed = HelperStandIn(.enabled, afterUnregister: .notRegistered)
        let removed = await helper(listed).unregisterForUninstall()
        XCTAssertNil(removed)
        XCTAssertEqual(listed.calls, ["unregister"])
        let none = HelperStandIn(.notRegistered)
        let nothing = await helper(none).unregisterForUninstall()
        XCTAssertNil(nothing)
        XCTAssertEqual(none.calls, [])
        let kept = HelperStandIn(.requiresApproval, afterUnregister: .requiresApproval)
        let refused = await helper(kept).unregisterForUninstall()
        XCTAssertEqual(refused, InsomniaWords.helperNotRemoved(nil))
        let failing = HelperStandIn(.enabled)
        failing.unregisterFails = true
        let failed = await helper(failing).unregisterForUninstall()
        XCTAssertNotNil(failed)
    }

    // decision: APP-986, APP-996
    func testTheStatusSaysWhetherTheHelperAnswers() {
        XCTAssertEqual(InsomniaStatus(["on": true, "closed_lid": "ready"])?.closedLid, "ready")
        XCTAssertEqual(InsomniaStatus(["on": true])?.closedLid, "unknown")
    }
}

@MainActor
private final class HelperStandIn: PowerHelperRegistering {
    var status: ServiceStatus
    var inBundle = true
    private let afterRegister: ServiceStatus?
    private let afterUnregister: ServiceStatus?
    var registerFails = false
    var unregisterFails = false
    var calls: [String] = []

    init(_ status: ServiceStatus, afterRegister: ServiceStatus? = nil, afterUnregister: ServiceStatus? = nil) {
        self.status = status
        self.afterRegister = afterRegister
        self.afterUnregister = afterUnregister
    }

    func register() throws {
        calls.append("register")
        if registerFails { throw NSError(domain: "SMAppServiceErrorDomain", code: 1) }
        if let afterRegister { status = afterRegister }
    }

    func unregister() async throws {
        calls.append("unregister")
        if unregisterFails { throw NSError(domain: "SMAppServiceErrorDomain", code: 1) }
        if let afterUnregister { status = afterUnregister }
    }

    func openLoginItems() { calls.append("open login items") }
}

private final class AskLog: @unchecked Sendable {
    private let lock = NSLock()
    private var seen: [Int] = []
    func note(_ starts: Int) { lock.lock(); seen.append(starts); lock.unlock() }
    var starts: [Int] { lock.lock(); defer { lock.unlock() }; return seen }
}

/// The cover's look over time (APP-1100): Write, then Breathe; still under Reduce Motion; nothing drawn while the
/// displays sleep or the screen is locked, or once stopped after 10 minutes; the dismiss's timing; the blur.
@MainActor
final class ShieldLookTests: XCTestCase {
    private let shown: TimeInterval = 5_000

    // decision: APP-1100, APP-998, APP-1004
    func testTheFIsWrittenThenTheDotBreathes() {
        let look = ShieldLookTimeline(shownAt: shown, reduced: false)
        XCTAssertEqual(look.phase(at: shown), .writing)
        XCTAssertEqual(look.framesPerSecond(at: shown + 1), 30)
        let start = look.mark(at: shown)
        XCTAssertEqual(start.fAlpha, 0)
        XCTAssertEqual(start.dotAlpha, 0)
        let writing = look.mark(at: shown + 1.3)
        XCTAssertGreaterThan(writing.reveal, 0.2)
        XCTAssertLessThan(writing.reveal, 0.8)                                  // the f half written
        XCTAssertGreaterThan(look.mark(at: shown + 2.2 + 0.2).dotLift, 0)       // the dot dropping in
        XCTAssertGreaterThan(look.mark(at: shown + 2.2 + 0.57).dotLift, 0)      // its one small bounce
        XCTAssertEqual(look.mark(at: shown + 2.2 + 0.8).dotLift, 0)
        XCTAssertGreaterThan(look.mark(at: shown + 2.9).energy, 0.2)            // the ripple through the grain
        XCTAssertEqual(look.phase(at: shown + 3.6), .breathing)
        XCTAssertEqual(look.framesPerSecond(at: shown + 60), 20)
        let full = look.mark(at: shown + 3.6), out = look.mark(at: shown + 3.6 + 2.5)
        XCTAssertEqual(full.reveal, 1)
        XCTAssertEqual(full.dotScale, 1, accuracy: 1e-9)
        XCTAssertEqual(out.dotScale, 0.88, accuracy: 1e-9)                      // the out-breath
        XCTAssertLessThan(out.glow, full.glow)
        XCTAssertEqual(look.mark(at: shown + 3.6 + 5 * 100).dotScale, 1, accuracy: 1e-6)   // for as long as it shows
        XCTAssertLessThan(look.fieldTime(at: shown + 30_000), ShieldLookTimeline.fieldClockWrap)
    }

    // decision: APP-1100, APP-1000
    func testReduceMotionIsOneStillFrame() {
        let look = ShieldLookTimeline(shownAt: shown, reduced: true)
        for t in [0.0, 1.3, 2.5, 60] {
            XCTAssertEqual(look.phase(at: shown + t), .still)
            XCTAssertEqual(look.framesPerSecond(at: shown + t), 0)
            XCTAssertEqual(look.mark(at: shown + t), .still)
            XCTAssertEqual(look.fieldTime(at: shown + t), ShieldLookTimeline.stillFieldTime)
        }
        XCTAssertEqual(ShieldDismiss.length(reduced: true), 0.3)
    }

    // decision: APP-1100, APP-982, APP-1000, APP-1004
    func testNothingIsDrawnWhileAsleepLockedOrStoppedAndItComesBack() {
        var look = ShieldLookTimeline(shownAt: shown, reduced: false)
        look.paused = true                                                       // the displays sleep or it's locked
        XCTAssertEqual(look.phase(at: shown + 1), .paused)
        XCTAssertEqual(look.framesPerSecond(at: shown + 1), 0)
        look.paused = false
        XCTAssertEqual(look.framesPerSecond(at: shown + 1), 30)
        look.stopped = true                                                      // 10 minutes with no event
        XCTAssertEqual(look.phase(at: shown + 600), .stopped)
        XCTAssertEqual(look.framesPerSecond(at: shown + 600), 0)                 // the field and the breath stop
        look.stopped = false                                                     // woken
        XCTAssertEqual(look.framesPerSecond(at: shown + 600), 20)                // breathing again, not written again
        look.dismissedAt = shown + 700
        XCTAssertEqual(look.phase(at: shown + 700), .dismissing)
        XCTAssertEqual(look.framesPerSecond(at: shown + 700), 0)
    }

    // decision: APP-1100, APP-1001
    func testTheDismissTakesAboutNineTenthsOfASecond() {
        XCTAssertEqual(ShieldDismiss.length(reduced: false), 0.9)
        XCTAssertEqual(ShieldDismiss.glow.start + ShieldDismiss.glow.length, 0.16, accuracy: 1e-9)
        XCTAssertEqual(ShieldDismiss.mark.start + ShieldDismiss.mark.length, 0.42, accuracy: 1e-9)
        XCTAssertEqual(ShieldDismiss.mark.scale, 0.94)
        XCTAssertEqual(ShieldDismiss.field.start + ShieldDismiss.field.length, ShieldDismiss.length, accuracy: 1e-9)
        XCTAssertLessThanOrEqual(ShieldDismiss.text.start + ShieldDismiss.text.length, ShieldDismiss.length)
    }

    // decision: APP-1100, APP-999
    func testTheFrostIsBlurrierThanThePrototypeAndTheRendererUsesIt() throws {
        XCTAssertGreaterThan(ShieldFrost.blurPoints, 20)                         // the prototype's σ was about 20 pt
        XCTAssertEqual(ShieldFrost.blurTexels(pixelsPerPoint: 2), ShieldFrost.blurPoints * 2 * 0.25)
        let pipeline = try XCTUnwrap(ShieldFieldPipeline.shared, "no Metal device")
        let renderer = ShieldFieldRenderer(pipeline: pipeline, pixelsPerPoint: 2)  // never drawn on screen
        XCTAssertEqual(renderer.blurSigma, Float(ShieldFrost.blurTexels(pixelsPerPoint: 2)))
        XCTAssertTrue(renderer.layer.isOpaque)
        let look = FrostedGrainLook(size: CGSize(width: 800, height: 500), scale: 2, main: true,
                                    state: ShieldLookState(), firstName: "Sam")   // its views only: nothing is shown
        XCTAssertEqual(look.field?.blurSigma, renderer.blurSigma)
        XCTAssertEqual(look.view.layer?.isOpaque, true)
    }

    // decision: APP-1100, APP-1002
    func testTheLinesAndTheFirstName() {
        XCTAssertEqual(ShieldWords.firstName(fullName: "Sam Rivera", account: "sam"), "Sam")
        XCTAssertEqual(ShieldWords.firstName(fullName: "", account: "sam"), "sam")
        XCTAssertEqual(ShieldWords.lockedTo("Sam"), "FERB is working · Locked to Sam")
        XCTAssertEqual(ShieldGrain.bytes().count, 256 * 256)
    }
}

/// What the cover's windows would do, recorded.
@MainActor
private final class CoverStandIn: InsomniaCoverSurface {
    var calls: [String] = []
    var looks: [Bool] = []
    var fronts = 0
    var clickThrough: Bool?
    /// What Touch ID answers: nil waits for `finishAuth`.
    var authResult: Bool?
    private var pending: (@MainActor (Bool) -> Void)?

    func raise() { calls.append("raise") }
    func drop() { calls.append("drop") }
    func setClickThrough(_ on: Bool) -> [Int] { clickThrough = on; return [701, 702] }
    func takeFront() { fronts += 1 }
    func show(dimmed: Bool) { looks.append(dimmed) }
    var lines: [String] = []
    func show(line: String) { lines.append(line) }
    func authenticate(_ done: @escaping @MainActor (Bool) -> Void) {
        if let authResult { done(authResult) } else { pending = done }
    }
    func finishAuth(_ ok: Bool) { pending?(ok); pending = nil }
    func startCamera() { calls.append("camera on") }
    func stopCamera() { calls.append("camera off") }
    var cameraAccess: String { "allowed" }
}
