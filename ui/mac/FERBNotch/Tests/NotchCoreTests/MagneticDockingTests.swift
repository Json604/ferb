import XCTest
@testable import FERBNotch

final class MagneticDockingTests: XCTestCase {
    let target = CGRect(x: 600, y: 870, width: 180, height: 30)
    func testContinuousApproachRetreatAndVelocity() {
        var state = MagneticDocking()
        state.begin(at: 0)
        let far = CGRect(x: 661, y: 650, width: 58, height: 30)
        XCTAssertEqual(state.update(pill: far, target: target, at: 0).proximity, 0)
        let near = far.offsetBy(dx: 0, dy: 130)
        let first = state.update(pill: near, target: target, at: 0.01)
        let held = state.update(pill: near, target: target, at: 0.06)
        XCTAssertGreaterThan(held.proximity, 0)
        XCTAssertGreaterThan(held.offset.y, first.offset.y)
        XCTAssertGreaterThan(held.velocity.y, 0)
        let retreat = state.update(pill: far, target: target, at: 0.061)
        XCTAssertGreaterThan(retreat.velocity.y, 0, "Reversal carries inertia")
        XCTAssertEqual(retreat.proximity, 0)
        XCTAssertLessThan(state.update(pill: far, target: target, at: 1).offset.y, 0.001)
    }
    func testTearOutAndNoNotch() {
        var state = MagneticDocking()
        state.begin(at: 0, tornOut: true)
        let near = CGRect(x: 661, y: 810, width: 58, height: 30)
        XCTAssertEqual(state.update(pill: near, target: target, at: 0.1).proximity, 0)
        _ = state.update(pill: near.offsetBy(dx: 0, dy: -300), target: target, at: 0.2)
        XCTAssertGreaterThan(state.update(pill: near, target: target, at: 0.3).proximity, 0)
        XCTAssertEqual(state.update(pill: near, target: nil, at: 0.4).proximity, 0)
    }
    func testFusionCompletesExactlyOnceAndCancellationRejectsIt() {
        var state = MagneticDocking()
        state.begin(at: 0)
        let pill = CGRect(x: 650, y: 820, width: 58, height: 30)
        _ = state.update(pill: pill, target: target, at: 0)
        state.release(to: target, at: 0.1)
        XCTAssertEqual(state.sample(at: 0.1).pill, pill)
        let end = state.sample(at: 0.4)
        XCTAssertEqual(end.pill, target)
        XCTAssertTrue(end.completed)
        XCTAssertFalse(state.sample(at: 0.5).completed)
        state.begin(at: 0.6)
        state.release(to: target, at: 0.7)
        state.cancel(at: 0.71)
        XCTAssertFalse(state.sample(at: 1).completed)
    }
    func testReducedMotionChangesDuringFusionAndDrag() {
        var state = MagneticDocking()
        state.begin(at: 0)
        let pill = CGRect(x: 650, y: 820, width: 58, height: 30)
        _ = state.update(pill: pill, target: target, at: 0)
        let reduced = state.update(pill: pill, target: target, at: 0.1, reduceMotion: true)
        XCTAssertGreaterThan(reduced.proximity, 0)
        XCTAssertEqual(reduced.offset, .zero)
        XCTAssertEqual(reduced.strain, 0)
        state.release(to: target, at: 0.2)
        XCTAssertTrue(state.sample(at: 0.21, reduceMotion: true).completed)
    }
}

extension MagneticDockingTests {
    func testRegrabPreservesDrawnPoseAndVelocity() {
        var state = MagneticDocking()
        state.begin(at: 0)
        let pill = CGRect(x: 650, y: 820, width: 58, height: 30)
        _ = state.update(pill: pill, target: target, at: 0, pointerVelocity: CGPoint(x: 10, y: 60))
        state.release(to: target, at: 0.01)
        let interrupted = state.sample(at: 0.07)
        state.begin(at: 0.07)
        let grabbed = state.update(pill: interrupted.pill, target: target, at: 0.07)
        XCTAssertEqual(grabbed.pill, interrupted.pill)
        XCTAssertEqual(grabbed.velocity, interrupted.velocity)
        XCTAssertFalse(state.sample(at: 1).completed)
    }

    func testFusionVelocityReachesRestContinuously() {
        var state = MagneticDocking()
        state.begin(at: 0)
        _ = state.update(pill: CGRect(x: 650, y: 820, width: 58, height: 30), target: target, at: 0)
        state.release(to: target, at: 0)
        let almost = state.sample(at: MagneticDocking.fusionDuration - 0.000001)
        XCTAssertLessThan(hypot(almost.velocity.x, almost.velocity.y), 0.01)
        XCTAssertEqual(state.sample(at: MagneticDocking.fusionDuration).pill, target)
    }
}

extension MagneticDockingTests {
    @MainActor func testNativeFusionKeepsMountedCardAndCompactsWithoutFinalPoseJump() {
        for scale: CGFloat in [1, 2] {
            let window = MagneticTestWindow(contentRect: .zero, styleMask: .borderless, backing: .buffered, defer: true)
            window.testScale = scale
            let presentation = NotchPresentation()
            window.contentView = presentation.backdrop
            let control = NSView(frame: .zero)
            presentation.backdrop.addSubview(control)
            let start = CGRect(x: 430, y: 200, width: 440, height: 500)
            let finish = CGRect(x: 470, y: 400, width: 440, height: 500)
            let canvas = start.union(finish).union(target)
            var state = MagneticDocking()
            state.begin(at: 0)
            _ = state.update(pill: CGRect(x: 650, y: 700, width: 58, height: 30), target: target, at: 0)
            state.release(to: target, at: 0)
            for tick in 0...24 {
                let sample = state.sample(at: Double(tick)/100)
                let card = sample.fusionRect(from: start, to: finish)
                DragSurfaceCanvas.place(window: window, presentation: presentation, control: control,
                    pill: sample.pill, card: card, canvas: canvas, bodyVisible: true)
                XCTAssertEqual(window.frame, canvas)
                XCTAssertEqual(presentation.size.width, 440, "Translation must not rewrap chat")
                XCTAssertEqual(window.convertToScreen(control.convert(control.bounds, to: nil)).minX,
                               sample.pill.minX, accuracy: 0.00001)
                XCTAssertEqual(presentation.expansion, 1)
            }
            let before = window.convertToScreen(control.convert(control.bounds, to: nil))
            DragSurfaceCanvas.place(window: window, presentation: presentation, control: control,
                pill: target, card: finish, canvas: finish.union(target), bodyVisible: true)
            XCTAssertEqual(window.convertToScreen(control.convert(control.bounds, to: nil)), before)
            XCTAssertEqual(presentation.size, finish.size)
        }
    }
}

import AppKit
private final class MagneticTestWindow: NSWindow {
    var testScale: CGFloat = 1
    override var backingScaleFactor: CGFloat { testScale }
}

extension MagneticDockingTests {
    @MainActor func testFastReleaseCarriesPillAndCardTogetherInsideStableCanvas() {
        let window = MagneticTestWindow(contentRect: .zero, styleMask: .borderless, backing: .buffered, defer: true)
        let presentation = NotchPresentation()
        window.contentView = presentation.backdrop
        let control = NSView(frame: .zero)
        presentation.backdrop.addSubview(control)
        let pill = CGRect(x: 661, y: 858, width: 58, height: 30)
        let start = CGRect(x: 470, y: 366, width: 440, height: 500)
        let finish = CGRect(x: 470, y: 400, width: 440, height: 500)
        let joinedPill = NotchDocking.dragRegion(cutout: target)
        for velocity in [CGPoint(x: 0, y: 2000), CGPoint(x: -4000, y: 2000)] {
            var state = MagneticDocking()
            state.begin(at: 0)
            _ = state.update(pill: pill, target: target, at: 0, pointerVelocity: velocity)
            state.release(to: joinedPill, at: 0)
            let excursion = CGFloat(MagneticDocking.fusionDuration) * 27 / 256
            let canvas = start.union(finish).union(pill).union(target)
                .insetBy(dx: -abs(velocity.x)*excursion, dy: -abs(velocity.y)*excursion).integral
            for tick in 0...22 {
                let sample = state.sample(at: Double(tick)/100)
                let card = sample.fusionRect(from: start, to: finish)
                XCTAssertTrue(card.intersects(sample.pill), "Shared inertia must not tear the surface apart")
                XCTAssertTrue(canvas.contains(card))
                XCTAssertTrue(canvas.contains(sample.pill))
                DragSurfaceCanvas.place(window: window, presentation: presentation, control: control,
                    pill: sample.pill, card: card, canvas: canvas, bodyVisible: true)
                XCTAssertEqual(window.frame, canvas)
                XCTAssertEqual(presentation.size.width, 440)
            }
        }
    }
}
