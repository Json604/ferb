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
        model.stopTimerAlert()
        XCTAssertNil(model.compactPlanText, "Stop dismissed the alert")
        XCTAssertFalse(rpc.calls.contains { $0.0 == "timers.repeat" }, "Stop never starts a timer")
        model.repeatTimer("tm-1")
        for _ in 0..<50 where model.runningTimers.isEmpty { try await Task.sleep(for: .milliseconds(20)) }
        XCTAssertEqual(rpc.calls.first { $0.0 == "timers.repeat" }?.1["id"] as? String, "tm-1")
        XCTAssertEqual(model.runningTimers.map(\.id), ["tm-2"])
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
