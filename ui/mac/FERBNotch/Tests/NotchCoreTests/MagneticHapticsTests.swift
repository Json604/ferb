import AppKit
import XCTest
@testable import FERBNotch

private final class HapticOutputSpy: MagneticContinuousOutput {
    var unavailable = false
    var values: [MagneticHapticSignal] = []
    var stops = 0
    func update(_ signal: MagneticHapticSignal) { values.append(signal) }
    func stop() { stops += 1 }
}

@MainActor final class MagneticHapticsTests: XCTestCase {
    func testProximityRetreatFusionAndExactlyOneCompletion() {
        let output = HapticOutputSpy()
        var taps: [NSHapticFeedbackManager.FeedbackPattern] = []
        let haptics = MagneticDockingHaptics(output: output, tap: { taps.append($0) })
        var sample = MagneticDocking.Sample()
        sample.phase = .dragging; sample.time = 1; sample.proximity = 0.2
        haptics.update(sample)
        let low = output.values.last!
        sample.proximity = 0.0001
        haptics.update(sample)
        XCTAssertLessThan(output.values.last!.amplitude, 0.0001)
        sample.time = 2; sample.proximity = 0.9
        haptics.update(sample)
        XCTAssertGreaterThan(output.values.last!.amplitude, low.amplitude)
        XCTAssertGreaterThan(output.values.last!.frequency, low.frequency)
        sample.time = 3; sample.proximity = 0.2
        haptics.update(sample)
        XCTAssertEqual(output.values.last!, low)
        sample.phase = .fusing; sample.time = 4; sample.fusionProgress = 0.5
        haptics.update(sample)
        XCTAssertGreaterThan(output.values.last!.frequency, low.frequency)
        sample.phase = .docked; sample.completed = true; sample.time = 5
        haptics.update(sample); haptics.update(sample)
        XCTAssertEqual(taps.count, 1)
        XCTAssertEqual(taps.first, .alignment)
        XCTAssertGreaterThan(output.stops, 0)
    }

    func testDisableAndCancellationRejectStaleCompletionAndSamples() {
        let output = HapticOutputSpy()
        var taps = 0
        let haptics = MagneticDockingHaptics(output: output, tap: { _ in taps += 1 })
        var sample = MagneticDocking.Sample()
        sample.phase = .dragging; sample.proximity = 1; sample.time = 1
        haptics.update(sample)
        haptics.enabled = false
        haptics.update(sample)
        XCTAssertEqual(output.values.count, 1)
        haptics.enabled = true
        haptics.cancel()
        haptics.update(sample)
        sample.phase = .docked; sample.completed = true; sample.time = 2
        haptics.update(sample)
        XCTAssertEqual(taps, 0)
        XCTAssertEqual(output.values.count, 1)
    }

    func testFallbackSparseTapsStopAtExitAndCompletionDoesNotReplay() {
        let output = HapticOutputSpy(); output.unavailable = true
        var taps = 0
        let haptics = MagneticDockingHaptics(output: output, tap: { _ in taps += 1 })
        var sample = MagneticDocking.Sample()
        sample.phase = .dragging; sample.proximity = 0.8; sample.time = 1
        haptics.update(sample)
        sample.time = 1.01; haptics.update(sample)
        XCTAssertEqual(taps, 1)
        sample.time = 1.5; haptics.update(sample)
        XCTAssertEqual(taps, 2)
        sample.proximity = 0; sample.time = 2; haptics.update(sample)
        XCTAssertEqual(taps, 2)
        sample.phase = .docked; sample.completed = true; sample.time = 3
        haptics.update(sample); haptics.enabled = false; haptics.enabled = true
        haptics.update(sample)
        XCTAssertEqual(taps, 3)
    }
}

private final class ActuatorSpy: MagneticActuator {
    var onPlay: (MagneticHapticSignal) -> TimeInterval? = { _ in 0.01 }
    var onClose: () -> Void = {}
    func play(_ signal: MagneticHapticSignal) -> TimeInterval? { onPlay(signal) }
    func close() { onClose() }
}

final class MagneticActuatorLifecycleTests: XCTestCase {
    func testMissingActuatorFallsBackAndReprobesAfterNewInteraction() async {
        let failed = expectation(description: "Probe failed")
        let recovered = expectation(description: "Reconnect plays")
        let closed = expectation(description: "Actuator closed")
        let spy = ActuatorSpy()
        spy.onPlay = { _ in recovered.fulfill(); return 1 }
        spy.onClose = { closed.fulfill() }
        var probes = 0
        let output = TrackpadMagneticOutput {
            probes += 1
            if probes == 1 { failed.fulfill(); return nil }
            return spy
        }
        output.update(.init(amplitude: 0.1, frequency: 150))
        await fulfillment(of: [failed], timeout: 1)
        // Factory returns before the worker publishes its failure state.
        let deadline = ContinuousClock.now + .seconds(1)
        while !output.unavailable, ContinuousClock.now < deadline {
            try? await Task.sleep(for: .milliseconds(1))
        }
        XCTAssertTrue(output.unavailable)
        output.update(.init(amplitude: 0.2, frequency: 180))
        XCTAssertTrue(output.unavailable, "Do not continuously probe a failed device")
        output.stop()
        output.update(.init(amplitude: 0.1, frequency: 150))
        await fulfillment(of: [recovered], timeout: 1)
        XCTAssertFalse(output.unavailable)
        output.stop()
        await fulfillment(of: [closed], timeout: 1)
    }

    func testPlayFailureClosesDeviceAndReportsFallback() async {
        let closed = expectation(description: "Failed device closed")
        let spy = ActuatorSpy()
        spy.onPlay = { _ in nil }
        spy.onClose = { closed.fulfill() }
        let output = TrackpadMagneticOutput { spy }
        output.update(.init(amplitude: 0.1, frequency: 150))
        await fulfillment(of: [closed], timeout: 1)
        XCTAssertTrue(output.unavailable)
        output.stop()
    }

    func testLatestSampleReplacesBacklogAndStopPreventsNextSegment() async {
        let first = expectation(description: "First segment")
        let latest = expectation(description: "Latest sample")
        let closed = expectation(description: "Closed")
        let spy = ActuatorSpy()
        var plays = 0
        spy.onPlay = { signal in
            plays += 1
            if plays == 1 { first.fulfill(); return 0.1 }
            XCTAssertEqual(signal.amplitude, 0.19, accuracy: 0.0001)
            XCTAssertEqual(plays, 2)
            latest.fulfill()
            return 1
        }
        spy.onClose = { closed.fulfill() }
        let output = TrackpadMagneticOutput { spy }
        output.update(.init(amplitude: 0.01, frequency: 100))
        await fulfillment(of: [first], timeout: 1)
        for value in 1...19 { output.update(.init(amplitude: Double(value) / 100, frequency: 180)) }
        await fulfillment(of: [latest], timeout: 1)
        output.stop()
        await fulfillment(of: [closed], timeout: 1)
    }

    func testSlowProbeCannotBlockStopOrMailboxUpdate() async {
        let probing = expectation(description: "Probe entered")
        let returned = expectation(description: "UI operations returned")
        let closed = expectation(description: "Cancelled probe resource closed")
        let release = DispatchSemaphore(value: 0)
        let spy = ActuatorSpy()
        spy.onPlay = { _ in XCTFail("Cancelled probe must not play"); return 0.01 }
        spy.onClose = { closed.fulfill() }
        let output = TrackpadMagneticOutput {
            probing.fulfill()
            _ = release.wait(timeout: .now() + 2)
            return spy
        }
        output.update(.init(amplitude: 0.1, frequency: 110))
        await fulfillment(of: [probing], timeout: 1)
        DispatchQueue.global().async {
            output.update(.init(amplitude: 0.2, frequency: 120))
            _ = output.unavailable
            output.stop()
            returned.fulfill()
        }
        await fulfillment(of: [returned], timeout: 0.5)
        release.signal()
        await fulfillment(of: [closed], timeout: 1)
    }

    func testCloseWaitsUntilNativeSubmissionReturnsAndNeverPlaysAgain() async {
        let entered = expectation(description: "In native submission")
        let closed = expectation(description: "Closed after submission")
        let stopped = expectation(description: "Stop returned")
        let release = DispatchSemaphore(value: 0)
        let spy = ActuatorSpy()
        var submitted = false
        spy.onPlay = { _ in
            entered.fulfill()
            _ = release.wait(timeout: .now() + 1)
            submitted = true
            return 0.01
        }
        spy.onClose = { XCTAssertTrue(submitted); closed.fulfill() }
        let output = TrackpadMagneticOutput { spy }
        output.update(.init(amplitude: 0.1, frequency: 150))
        await fulfillment(of: [entered], timeout: 1)
        DispatchQueue.global().async { output.stop(); stopped.fulfill() }
        release.signal()
        await fulfillment(of: [stopped, closed], timeout: 1)
    }
}
