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
