import AppKit
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// Timers that flow like Apple's (owner, 2026-11-06, APP-762): the timer card's countdown and draining ring, the closed
/// notch's live activity for the soonest running timer, and the done alert's Stop and Repeat through the daemon.
@MainActor
final class TimerFlowTests: XCTestCase {
    // decision: APP-762
    func testTheCountdownAndTheRingFollowTheTimersStartAndEnd() {
        XCTAssertEqual(TimerClock.countdown(299.2), "5:00")
        XCTAssertEqual(TimerClock.countdown(59), "0:59")
        XCTAssertEqual(TimerClock.countdown(3723), "1:02:03")
        XCTAssertEqual(TimerClock.countdown(-4), "0:00")
        let start = Date(timeIntervalSince1970: 1000), end = start.addingTimeInterval(600)
        XCTAssertEqual(TimerClock.remaining(started: start, ends: end, now: start), 1)
        XCTAssertEqual(TimerClock.remaining(started: start, ends: end, now: start.addingTimeInterval(150)), 0.75)
        XCTAssertEqual(TimerClock.remaining(started: start, ends: end, now: end.addingTimeInterval(5)), 0)
        XCTAssertNil(TimerClock.remaining(started: nil, ends: end, now: start))          // an older daemon: no ring
        let soon = RunningTimer(id: "a", name: "Tea", started: start, ends: start.addingTimeInterval(60))
        let later = RunningTimer(id: "b", name: "Pasta", started: start, ends: start.addingTimeInterval(600))
        XCTAssertEqual(RunningTimer.soonest([later, soon], now: start), soon)
        XCTAssertEqual(RunningTimer.soonest([later, soon], now: start.addingTimeInterval(61)), later)
        XCTAssertNil(RunningTimer(row: ["id": "r", "kind": "reminder", "at": "2026-11-06T10:00:00Z"]))
        let card = ResultCard(["type": "event", "kind": "timer", "id": "tm-1", "what": "Tea",
                               "when": "2026-11-06T10:10:00Z", "started": "2026-11-06T10:00:00Z"])
        XCTAssertEqual(card?.timerID, "tm-1")
        XCTAssertEqual(card?.timerEnds?.timeIntervalSince(card?.startedDate ?? .distantPast), 600)
    }

    // decision: APP-762, APP-622
    func testStopDismissesTheAlertAndRepeatStartsTheSameTimerThroughTheDaemon() async throws {
        let rpc = ScriptedRPC()
        let ends = ISO8601DateFormatter().string(from: Date().addingTimeInterval(300))
        rpc.replies["timers.repeat"] = ["timer": "tm-2", "kind": "timer", "at": ends, "name": "Tea",
                                        "started": ISO8601DateFormatter().string(from: Date())]
        rpc.replies["reminders.list"] = ["pending": [["id": "tm-2", "kind": "timer", "at": ends, "name": "Tea"]]]
        let model = NotchModel(rpc: rpc, preferences: scratchDefaults())
        model.acknowledgement.show("Your tea is ready.", reminder: ResultCard(["type": "reminder", "kind": "timer",
                                                                                "title": "Tea", "timer_id": "tm-1"]))
        XCTAssertNotNil(model.compactPlanText)
        model.stopTimerAlert("tm-1")
        XCTAssertNil(model.compactPlanText, "Stop dismissed the alert")
        XCTAssertFalse(rpc.calls.contains { $0.0 == "timers.repeat" }, "Stop never starts a timer")
        model.repeatTimer("tm-1")
        for _ in 0..<50 where model.runningTimers.isEmpty { try await Task.sleep(for: .milliseconds(20)) }
        XCTAssertEqual(rpc.calls.first { $0.0 == "timers.repeat" }?.1["id"] as? String, "tm-1")
        XCTAssertEqual(model.runningTimers.map(\.id), ["tm-2"])
    }

    // decision: DMN-548, APP-764, APP-762
    func testATimerCancelledAnywhereStopsEverywhereAndConnectReadsTheDaemonsRows() async throws {
        // Review 2, finding 1: a timer cancelled in chat or in Settings kept counting down in the closed notch and on
        // its card, and a relaunch called a cancelled timer "Timer done".
        let rpc = ScriptedRPC()
        let ends = ISO8601DateFormatter().string(from: Date().addingTimeInterval(300))
        let model = NotchModel(rpc: rpc, preferences: scratchDefaults())
        model.receiveBusEvent(["topic": "timer.changed", "data": ["timer_id": "tm-1", "kind": "timer", "state": "running",
                                                                  "at": ends, "name": "Tea"]])
        XCTAssertEqual(model.soonestTimer()?.id, "tm-1", "a timer set anywhere counts down in the closed notch")
        model.receiveBusEvent(["topic": "timer.changed", "data": ["timer_id": "tm-1", "kind": "timer",
                                                                  "state": "cancelled", "at": ends]])
        XCTAssertNil(model.soonestTimer(), "cancelled in chat or Settings: the closed notch stops counting")
        XCTAssertEqual(model.timerRecords["tm-1"]?.state, .cancelled, "its card says Cancelled")
        XCTAssertFalse(model.timerAlertOpen("tm-1"))
        // relaunch: nothing in memory; the cards' states come from FERB's rows
        let relaunched = NotchModel(rpc: rpc, preferences: scratchDefaults())
        rpc.replies["reminders.list"] = ["pending": []]
        rpc.replies["timers.states"] = ["timers": ["tm-1": ["kind": "timer", "state": "cancelled"],
                                                   "tm-2": ["kind": "timer", "state": "fired"]]]
        relaunched.needTimerState("tm-1")
        relaunched.needTimerState("tm-2")
        for _ in 0..<50 where relaunched.timerRecords.count < 2 { try await Task.sleep(for: .milliseconds(20)) }
        XCTAssertEqual(relaunched.timerRecords["tm-1"]?.state, .cancelled, "a cancelled timer never reads Done")
        XCTAssertEqual(relaunched.timerRecords["tm-2"]?.state, .fired)
        // a reconnect asks again for every timer shown (a cancel while the app was away)
        rpc.replies["timers.states"] = ["timers": ["tm-2": ["kind": "timer", "state": "fired", "repeated_by": "tm-3"]]]
        await relaunched.refreshTimers()
        let asked = rpc.calls.last { $0.0 == "timers.states" }?.1["ids"] as? [String]
        XCTAssertEqual(asked, ["tm-1", "tm-2"])
        XCTAssertEqual(relaunched.timerRecords["tm-2"]?.repeatedBy, "tm-3")
    }

    // decision: DMN-548, APP-764, APP-622
    func testStopAndRepeatComeFromTheModelAndStopDismissesOnlyThatTimersArrival() async throws {
        // Review 2, finding 6: Repeat on the alert and again on the chat card started two timers; Stop dismissed
        // whatever arrival was showing.
        let rpc = ScriptedRPC()
        let ends = ISO8601DateFormatter().string(from: Date().addingTimeInterval(300))
        rpc.replies["timers.repeat"] = ["timer": "tm-2", "kind": "timer", "at": ends, "name": "Tea", "state": "running",
                                        "repeat_of": "tm-1", "started": ISO8601DateFormatter().string(from: Date())]
        rpc.replies["reminders.list"] = ["pending": [["id": "tm-2", "kind": "timer", "at": ends, "name": "Tea"]]]
        let model = NotchModel(rpc: rpc, preferences: scratchDefaults())
        model.receiveBusEvent(["topic": "timer.fired", "data": ["timer_id": "tm-1", "kind": "timer"]])
        XCTAssertTrue(model.timerAlertOpen("tm-1"), "fired and not repeated: the arrival and its card offer Stop and Repeat")
        model.acknowledgement.show("Water break", reminder: ResultCard(["type": "reminder", "kind": "reminder",
                                                                         "title": "Water", "timer_id": "rm-9"]))
        model.stopTimerAlert("tm-1")
        XCTAssertNotNil(model.compactPlanText, "Stop on the tea timer's card leaves another arrival alone")
        XCTAssertFalse(model.timerAlertOpen("tm-1"), "stopped: its Stop and Repeat go everywhere")
        model.stopTimerAlert("rm-9")
        XCTAssertNil(model.compactPlanText, "that reminder's own Done dismisses it")
        let other = NotchModel(rpc: rpc, preferences: scratchDefaults())
        other.timerFired("tm-1")
        other.repeatTimer("tm-1")
        for _ in 0..<50 where other.timerRecords["tm-1"]?.repeatedBy == nil { try await Task.sleep(for: .milliseconds(20)) }
        XCTAssertEqual(other.timerRecords["tm-1"]?.repeatedBy, "tm-2")
        XCTAssertFalse(other.timerAlertOpen("tm-1"), "repeated: no second Repeat anywhere")
        XCTAssertEqual(other.runningTimers.map(\.id), ["tm-2"])
    }

    // decision: APP-765, APP-762, APP-622
    func testTheArrivalsStopRepeatAndDoneAreItsAccessibilityActions() throws {
        // Review 2, finding 7: the arrival was one combined element whose only action opened the chat.
        let rpc = ScriptedRPC()
        let model = NotchModel(rpc: rpc, preferences: scratchDefaults())
        model.timerFired("tm-1")
        let timer = try XCTUnwrap(ResultCard(["type": "reminder", "kind": "timer", "title": "Tea", "timer_id": "tm-1",
                                              "started": "2026-11-06T10:00:00Z", "due": "2026-11-06T10:10:00Z"]))
        let reminder = try XCTUnwrap(ResultCard(["type": "reminder", "kind": "reminder", "title": "Water",
                                                 "timer_id": "rm-1"]))
        XCTAssertEqual(model.arrivalActions(timer).map(\.name), ["Stop", "Repeat"])
        XCTAssertEqual(model.arrivalActions(reminder).map(\.name), ["Done"])
        // ReminderArrivalSurface lists exactly these as its named accessibility actions (`accessibilityActions`); an
        // in-process test can't read SwiftUI's accessibility tree (it is built only for an assistive client)
        _ = ReminderArrivalSurface(card: timer, text: "Your tea is ready.", model: model)
        model.arrivalActions(timer).first { $0.name == "Stop" }?.run()
        XCTAssertFalse(model.timerAlertOpen("tm-1"))
        XCTAssertEqual(model.arrivalActions(timer).map(\.name), [], "stopped: nothing left to offer")
    }

    // decision: APP-762, APP-550, APP-588
    func testAClosedNotchShowsTheSoonestTimerBesideTheCameraAndNeverTakesAClick() throws {
        _ = NSApplication.shared
        guard NSScreen.screens.first != nil else { throw XCTSkip("Needs a display") }
        let defaults = scratchDefaults()
        NotchDocking.save(true, in: defaults)
        let model = NotchModel(rpc: ScriptedRPC(), preferences: defaults)
        let coordinator = PanelCoordinator(model: model, defaults: defaults)
        coordinator.updateScreen()
        coordinator.hoverWait = { _ in }
        addTeardownBlock { coordinator.tearDown() }
        XCTAssertNil(coordinator.timerActivityPlace(), "no timer: nothing beside the camera")
        model.runningTimers = [RunningTimer(id: "tm-1", name: "Tea", started: Date(), ends: Date().addingTimeInterval(300))]
        coordinator.updateTimerActivity()
        let place = try XCTUnwrap(coordinator.timerActivityPlace())
        XCTAssertTrue(coordinator.timerActivityPanel.isVisible)
        XCTAssertTrue(coordinator.timerActivityPanel.ignoresMouseEvents, "press, drag and hover stay the handle's")
        XCTAssertEqual(place.frame.width, place.gap + 2 * PanelCoordinator.timerEarWidth)
        model.toggle(byPress: true)                                 // FERB open: the chat shows instead
        coordinator.updateTimerActivity()
        XCTAssertNil(coordinator.timerActivityPlace())
        XCTAssertFalse(coordinator.timerActivityPanel.isVisible)
        model.collapse()
        model.runningTimers = [RunningTimer(id: "tm-1", name: "Tea", started: Date(), ends: Date().addingTimeInterval(-1))]
        coordinator.updateTimerActivity()
        XCTAssertFalse(coordinator.timerActivityPanel.isVisible, "it ends with the timer")
    }
}

/// The live activity and the edge pill (APP-766, review 2 findings 3-5), driven through the real PanelCoordinator on a
/// notchless layout (the pill on an edge); notifications are posted in this process only.
@MainActor
final class TimerPillTests: XCTestCase {
    private var fullScreen = false

    private func coordinator(edge: String = "top") throws -> (NotchModel, PanelCoordinator) {
        _ = NSApplication.shared
        guard let screen = NSScreen.screens.first else { throw XCTSkip("Needs a display") }
        let defaults = scratchDefaults()
        defaults.set(edge, forKey: "notch.indicatorEdge")
        if let id = screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber {
            defaults.set(id.intValue, forKey: "notch.indicatorDisplayID")
        }
        let model = NotchModel(rpc: ScriptedRPC(), preferences: defaults)
        model.setHoverEnabled(false)
        let coordinator = PanelCoordinator(model: model, defaults: defaults, overlaySpace: NotchOverlaySpace(available: false))
        coordinator.cameraHousing = { _ in (0, nil, nil) }
        coordinator.fullScreenSpace = { [unowned self] _ in self.fullScreen }
        coordinator.missionControlShowing = { _ in false }
        coordinator.activeSpace = { 1 }
        coordinator.hoverWait = { _ in }
        coordinator.restWait = { _ in }
        let away = CGPoint(x: screen.frame.midX, y: screen.frame.midY)
        coordinator.pointerLocation = { away }
        coordinator.updateScreen()
        coordinator.observeWorkspace()
        addTeardownBlock { coordinator.tearDown() }
        XCTAssertFalse(model.isNotchDocked)
        return (model, coordinator)
    }

    private func settle(_ seconds: TimeInterval = 0.1) { RunLoop.main.run(until: Date().addingTimeInterval(seconds)) }

    private func tea(_ model: NotchModel) {
        model.runningTimers = [RunningTimer(id: "tm-1", name: "Tea", started: Date(), ends: Date().addingTimeInterval(3600))]
    }

    // decision: APP-766, APP-763
    func testTheCountdownComesBackByItselfAfterADropAndADisplayWake() throws {
        let (model, coordinator) = try coordinator()
        tea(model)
        coordinator.updateTimerActivity()
        XCTAssertTrue(coordinator.timerActivityPanel.isVisible)
        let control = try XCTUnwrap(coordinator.indicatorControl)
        let pill = coordinator.indicatorPanel.frame
        control.onDragBegan?(CGPoint(x: pill.midX, y: pill.midY))
        settle(1.2)                                         // the drag hides it (its 1 s look stops while hidden)
        XCTAssertFalse(coordinator.timerActivityPanel.isVisible, "drawn while the pill is dragged")
        control.onDragEnded?(CGPoint(x: pill.midX, y: pill.midY))
        settle()
        XCTAssertTrue(coordinator.timerActivityPanel.isVisible, "the countdown didn't come back after the drop")
        let center = NSWorkspace.shared.notificationCenter
        center.post(name: NSWorkspace.screensDidSleepNotification, object: nil)
        settle(1.2)
        XCTAssertFalse(coordinator.timerActivityPanel.isVisible, "drawn while the displays sleep")
        center.post(name: NSWorkspace.screensDidWakeNotification, object: nil)
        settle()
        XCTAssertTrue(coordinator.timerActivityPanel.isVisible, "the countdown didn't come back after the wake")
    }

    // decision: APP-766, APP-735
    func testNothingIsDrawnAroundThePillsRestingLine() throws {
        let (model, coordinator) = try coordinator()
        tea(model)
        coordinator.updateTimerActivity()
        XCTAssertTrue(coordinator.timerActivityPanel.isVisible)
        fullScreen = true                                   // a full-screen app's Space: the pill rests (D14)
        coordinator.activeSpace = { 2 }
        NSWorkspace.shared.notificationCenter.post(name: NSWorkspace.activeSpaceDidChangeNotification, object: nil)
        settle()
        XCTAssertTrue(coordinator.pillResting)
        XCTAssertNil(coordinator.timerActivityPlace())
        XCTAssertFalse(coordinator.timerActivityPanel.isVisible, "black ears around the 5 pt resting line")
    }

    // decision: APP-766, APP-763
    func testATimerHoldsThePillUpOnlyWhereItsCountdownDrawsAndItsEndLetsItRest() async throws {
        let (side, onSide) = try coordinator(edge: "left")
        await onSide.indicatorRestTask?.value
        XCTAssertTrue(onSide.pillResting)
        tea(side)
        onSide.updateTimerActivity()
        XCTAssertTrue(onSide.pillResting, "on a side edge nothing can draw: the timer doesn't hold the pill up")
        XCTAssertFalse(onSide.timerActivityPanel.isVisible)
        let (model, coordinator) = try coordinator()
        await coordinator.indicatorRestTask?.value
        tea(model)
        coordinator.updateTimerActivity()
        XCTAssertFalse(coordinator.pillResting, "on the top edge the timer holds the pill up for its countdown")
        model.receiveTimerChange(["timer_id": "tm-1", "kind": "timer", "state": "cancelled"])   // cancelled elsewhere
        coordinator.updateTimerActivity()
        await coordinator.indicatorRestTask?.value
        XCTAssertTrue(coordinator.pillResting, "the timer ended: the pill rests again by itself")
        XCTAssertFalse(coordinator.timerActivityPanel.isVisible)
    }
}
