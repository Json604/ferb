import XCTest
import AudioCapture
@testable import FERBNotch

final class MagneticEffectsTests: XCTestCase {
    func testDirectionalStrainPreservesOppositeEdgeAndReducedMotion() {
        var sample = MagneticDocking.Sample()
        sample.pill = CGRect(x: 100, y: 100, width: 58, height: 30)
        sample.direction = CGPoint(x: 0, y: 1)
        sample.strain = 0.018
        let strained = MagneticFieldGeometry.strainedPill(sample)
        XCTAssertEqual(strained.minY, sample.pill.minY)
        XCTAssertGreaterThan(strained.maxY, sample.pill.maxY)
        sample.reduceMotion = true
        XCTAssertEqual(MagneticFieldGeometry.strainedPill(sample), sample.pill)
    }
    func testSynthFiniteAndExactSilenceAfterRetreat() {
        let synth = FERBMagneticSynthCreate(48000)!
        defer { FERBMagneticSynthDestroy(synth) }
        var left = [Float](repeating: 0, count: 48000)
        var right = left
        FERBMagneticSynthSet(synth, 1, 0.18, 0, false)
        left.withUnsafeMutableBufferPointer { l in right.withUnsafeMutableBufferPointer { r in
            FERBMagneticSynthRender(synth, l.baseAddress!, r.baseAddress!, 48000)
        } }
        XCTAssertTrue(left.allSatisfy(\.isFinite))
        XCTAssertGreaterThan(left.map(abs).max()!, 0.001)
        FERBMagneticSynthSet(synth, 0, 0, 0, false)
        left.withUnsafeMutableBufferPointer { l in right.withUnsafeMutableBufferPointer { r in
            FERBMagneticSynthRender(synth, l.baseAddress!, r.baseAddress!, 48000)
        } }
        XCTAssertEqual(Array(left.suffix(100)), [Float](repeating: 0, count: 100))
    }
}

extension MagneticEffectsTests {
    private func pcm(_ synth: OpaquePointer, frames: Int) -> [Float] {
        var left = [Float](repeating: 0, count: frames)
        var right = left
        left.withUnsafeMutableBufferPointer { l in right.withUnsafeMutableBufferPointer { r in
            FERBMagneticSynthRender(synth, l.baseAddress!, r.baseAddress!, UInt32(frames))
        } }
        return left
    }

    func testPCMPhaseIsContinuousAcrossBufferBoundariesAndGainTracksProximity() {
        let a = FERBMagneticSynthCreate(48000)!, b = FERBMagneticSynthCreate(48000)!
        defer { FERBMagneticSynthDestroy(a); FERBMagneticSynthDestroy(b) }
        FERBMagneticSynthSet(a, 0.7, 0.18, -0.5, false)
        FERBMagneticSynthSet(b, 0.7, 0.18, -0.5, false)
        let whole = pcm(a, frames: 4096)
        let pieces = pcm(b, frames: 2048) + pcm(b, frames: 2048)
        XCTAssertEqual(whole, pieces)
        FERBMagneticSynthSet(a, 0.15, 0.18, 0, false)
        _ = pcm(a, frames: 48000)
        let distant = pcm(a, frames: 4800).map { $0*$0 }.reduce(0,+)
        FERBMagneticSynthSet(a, 1, 0.18, 0, false)
        _ = pcm(a, frames: 48000)
        let close = pcm(a, frames: 4800).map { $0*$0 }.reduce(0,+)
        XCTAssertGreaterThan(close, distant * 4)
        FERBMagneticSynthSet(a, 1, 0, 0, false)
        XCTAssertTrue(pcm(a, frames: 48000).suffix(100).allSatisfy { $0 == 0 })
    }

    func testFusionIsFiniteAndCancellationCannotLeaveDrone() {
        let synth = FERBMagneticSynthCreate(48000)!
        defer { FERBMagneticSynthDestroy(synth) }
        FERBMagneticSynthSet(synth, 0, 0.18, 0, true)
        let tail = pcm(synth, frames: 24000)
        XCTAssertGreaterThan(tail.map(abs).max()!, 0.001)
        XCTAssertTrue(tail.suffix(1000).allSatisfy { $0 == 0 })
        FERBMagneticSynthSet(synth, 1, 0.18, 0, true)
        _ = pcm(synth, frames: 1000)
        FERBMagneticSynthCancel(synth)
        XCTAssertTrue(pcm(synth, frames: 48000).suffix(1000).allSatisfy { $0 == 0 })
        // A completion cancelled before the audio thread sees it must never play.
        FERBMagneticSynthSet(synth, 0, 0.18, 0, true)
        FERBMagneticSynthCancel(synth)
        FERBMagneticSynthSet(synth, 0, 0.18, 0, false)
        XCTAssertTrue(pcm(synth, frames: 10000).allSatisfy { $0 == 0 })
    }

    @MainActor func testNativeFieldRestrictsResponseAndReducedMotionHasNoOscillation() {
        let view = MagneticFieldView(frame: CGRect(x: 0, y: 0, width: 400, height: 400))
        var sample = MagneticDocking.Sample()
        sample.phase = .dragging
        sample.pill = CGRect(x: 171, y: 200, width: 58, height: 30)
        sample.target = CGRect(x: 140, y: 300, width: 120, height: 30)
        sample.proximity = 0.8
        sample.direction = CGPoint(x: 0, y: 1)
        sample.reduceMotion = true
        view.update(sample, canvas: view.bounds)
        let layers = view.layer!.sublayers!.compactMap { $0 as? CAShapeLayer }
        XCTAssertEqual(layers.count, 6)
        let paths = layers.map { $0.path! }
        XCTAssertTrue(paths.prefix(3).allSatisfy { $0.boundingBox.minY > sample.pill.midY })
        XCTAssertTrue(paths.suffix(3).allSatisfy { $0.boundingBox.maxY < sample.target!.minY })
        sample.time = 7
        view.update(sample, canvas: view.bounds)
        for (before, after) in zip(paths, layers) { XCTAssertEqual(before, after.path) }
        sample.fusionProgress = 1
        view.update(sample, canvas: view.bounds)
        XCTAssertTrue(layers.allSatisfy { $0.opacity == 0 })
    }

    @MainActor func testOutputStartFailureIsContainedAndPreferencesUpdate() async {
        enum Failure: Error { case unavailable }
        let feedback = SoundFeedback()
        let saved = (feedback.enabled, feedback.volume, feedback.magneticEnabled)
        defer { feedback.enabled = saved.0; feedback.volume = saved.1; feedback.magneticEnabled = saved.2 }
        feedback.enabled = true
        feedback.volume = 0.18
        feedback.magneticEnabled = true
        var attempts = 0
        let audio = MagneticDockingAudio(feedback: feedback) { _ in attempts += 1; throw Failure.unavailable }
        var sample = MagneticDocking.Sample()
        sample.phase = .dragging
        sample.proximity = 1
        audio.update(sample)
        XCTAssertEqual(attempts, 1)
        XCTAssertNotNil(audio.startError)
        feedback.enabled = false
        audio.update(sample)
        XCTAssertEqual(attempts, 1)
        feedback.enabled = true
        feedback.magneticEnabled = false
        audio.update(sample)
        XCTAssertEqual(attempts, 1)
        audio.cancel()
    }
}

extension MagneticEffectsTests {
    @MainActor func testCompletionSchedulesEngineRetirementAndNeverReplaysOnPreferenceChange() async {
        let feedback = SoundFeedback()
        let saved = (feedback.enabled, feedback.volume, feedback.magneticEnabled)
        defer { feedback.enabled = saved.0; feedback.volume = saved.1; feedback.magneticEnabled = saved.2 }
        feedback.enabled = true; feedback.volume = 0.18; feedback.magneticEnabled = true
        var starts = 0
        let audio = MagneticDockingAudio(feedback: feedback) { _ in starts += 1 }
        var sample = MagneticDocking.Sample()
        sample.phase = .docked; sample.completed = true; sample.time = 1
        audio.update(sample)
        XCTAssertTrue(audio.hasPendingStop)
        XCTAssertEqual(starts, 1)
        audio.update(sample)
        XCTAssertEqual(starts, 1)
        try? await Task.sleep(for: .milliseconds(350))
        XCTAssertFalse(audio.hasPendingStop)
        XCTAssertEqual(starts, 1)
    }

    func testCancelDuringContactIsDeclicked() {
        let synth = FERBMagneticSynthCreate(48000)!
        defer { FERBMagneticSynthDestroy(synth) }
        FERBMagneticSynthSet(synth, 0, 0.18, 0, true)
        let before = pcm(synth, frames: 1300)
        FERBMagneticSynthCancel(synth)
        let after = pcm(synth, frames: 48000)
        XCTAssertLessThan(abs(after[0] - before.last!), 0.001)
        XCTAssertTrue(after.suffix(1000).allSatisfy { $0 == 0 })
    }
}

extension MagneticEffectsTests {
    @MainActor func testMutedDisplaySamplesDoNotPostponeEngineRetirement() async {
        let feedback = SoundFeedback()
        let saved = (feedback.enabled, feedback.volume, feedback.magneticEnabled)
        defer { feedback.enabled = saved.0; feedback.volume = saved.1; feedback.magneticEnabled = saved.2 }
        feedback.enabled = true; feedback.volume = 0.18; feedback.magneticEnabled = true
        let audio = MagneticDockingAudio(feedback: feedback) { _ in }
        var sample = MagneticDocking.Sample()
        sample.phase = .dragging; sample.proximity = 1
        audio.update(sample)
        feedback.enabled = false
        audio.update(sample)
        try? await Task.sleep(for: .milliseconds(200))
        audio.update(sample)
        try? await Task.sleep(for: .milliseconds(150))
        XCTAssertFalse(audio.hasPendingStop, "Later muted samples must not extend the original retirement deadline")
    }

    @MainActor func testRouteChangeRestartsResonanceButCannotReplayCompletedFusion() {
        let feedback = SoundFeedback()
        let saved = (feedback.enabled, feedback.volume, feedback.magneticEnabled)
        defer { feedback.enabled = saved.0; feedback.volume = saved.1; feedback.magneticEnabled = saved.2 }
        feedback.enabled = true; feedback.volume = 0.18; feedback.magneticEnabled = true
        var starts = 0
        let audio = MagneticDockingAudio(feedback: feedback) { _ in starts += 1 }
        var sample = MagneticDocking.Sample()
        sample.phase = .dragging; sample.proximity = 1
        audio.update(sample)
        audio.outputRouteChanged()
        XCTAssertEqual(starts, 2)
        sample.phase = .docked; sample.completed = true; sample.time = 1
        audio.update(sample)
        XCTAssertEqual(starts, 3)
        audio.outputRouteChanged()
        XCTAssertEqual(starts, 3)
        audio.cancel()
    }
}
