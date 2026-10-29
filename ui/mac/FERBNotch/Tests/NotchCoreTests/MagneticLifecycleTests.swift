import AppKit
import XCTest
@testable import FERBNotch

@MainActor
final class MagneticLifecycleTests: XCTestCase {
    private func coordinator() -> PanelCoordinator {
        _ = NSApplication.shared
        let coordinator = PanelCoordinator()
        coordinator.magneticHapticsOutputEnabled = false
        coordinator.magneticFieldPresentationEnabled = false
        return coordinator
    }

    func testExplicitCloseCancelsNativePointerCaptureBeforeGeometryGuard() {
        let coordinator = coordinator()
        defer { coordinator.tearDown() }
        coordinator.indicatorControl?.onDragBegan?(CGPoint(x: 100, y: 100))
        XCTAssertTrue(coordinator.hasMagneticInteraction)
        coordinator.model.collapse()
        XCTAssertFalse(coordinator.hasMagneticInteraction)
        XCTAssertEqual(coordinator.magneticSample.phase, .idle)
        XCTAssertFalse(coordinator.magneticFieldPanel.isVisible)
    }

    func testSleepAndDisplayReconfigurationCancelNativeDrag() {
        for interrupt in [ { (c: PanelCoordinator) in c.screensDidSleep() },
                           { (c: PanelCoordinator) in c.displayParametersChanged() } ] {
            let coordinator = coordinator()
            coordinator.indicatorControl?.onDragBegan?(CGPoint(x: 100, y: 100))
            XCTAssertTrue(coordinator.hasMagneticInteraction)
            interrupt(coordinator)
            XCTAssertFalse(coordinator.hasMagneticInteraction)
            XCTAssertEqual(coordinator.magneticSample.phase, .idle)
            coordinator.indicatorControl?.onDragEnded?(CGPoint(x: 100, y: 100))
            XCTAssertFalse(coordinator.hasMagneticInteraction)
            XCTAssertEqual(coordinator.magneticSample.phase, .idle)
            XCTAssertFalse(coordinator.magneticFieldPanel.isVisible)
            coordinator.tearDown()
        }
    }

    func testInterruptedFusionCannotCompleteOrPersistFromQueuedTick() {
        let savedDocking = NotchDocking.isSaved(in: .standard)
        defer { NotchDocking.save(savedDocking, in: .standard) }
        for progress in [0.01, 0.1, 0.2] {
            for expanded in [false, true] {
                let coordinator = coordinator()
                if expanded { coordinator.model.expand() }
                coordinator.indicatorControl?.onDragBegan?(CGPoint(x: 100, y: 100))
                let now = CACurrentMediaTime()
                let target = CGRect(x: 80, y: 180, width: 100, height: 30)
                coordinator.updateMagneticDrag(pill: CGRect(x: 100, y: 130, width: 58, height: 30),
                    target: target, at: now, reduceMotion: false, velocity: .zero)
                coordinator.beginMagneticFusion(to: target, at: now)
                coordinator.advanceMagneticFusion(at: now + progress, reduceMotion: false)
                XCTAssertEqual(coordinator.magneticSample.phase, .fusing)
                coordinator.model.collapse()
                XCTAssertFalse(coordinator.hasMagneticInteraction)
                coordinator.advanceMagneticFusion(at: now + 1, reduceMotion: false)
                XCTAssertEqual(coordinator.magneticSample.phase, .idle)
                XCTAssertFalse(coordinator.magneticSample.completed)
                XCTAssertEqual(NotchDocking.isSaved(in: .standard), savedDocking)
                XCTAssertFalse(coordinator.magneticFieldPanel.isVisible)
                coordinator.tearDown()
            }
        }
    }

    func testNativeMouseUpAfterCloseCannotReopen() {
        let coordinator = coordinator()
        defer { coordinator.tearDown() }
        let control = coordinator.indicatorControl!
        control.pointerLocation = { CGPoint(x: 100, y: 100) }
        let down = NSEvent.mouseEvent(with: .leftMouseDown, location: .zero,
            modifierFlags: [], timestamp: 0, windowNumber: 0, context: nil,
            eventNumber: 0, clickCount: 1, pressure: 1)!
        let up = NSEvent.mouseEvent(with: .leftMouseUp, location: .zero,
            modifierFlags: [], timestamp: 1, windowNumber: 0, context: nil,
            eventNumber: 1, clickCount: 1, pressure: 0)!
        control.mouseDown(with: down)
        coordinator.model.collapse()
        control.mouseUp(with: up)
        XCTAssertFalse(coordinator.model.surfaceVisible)
        XCTAssertFalse(coordinator.hasMagneticInteraction)
    }

    func testRegrabDuringFusionKeepsDrawnPillAndRejectsOldCompletion() {
        let savedDocking = NotchDocking.isSaved(in: .standard)
        defer { NotchDocking.save(savedDocking, in: .standard) }
        for progress in [0.01, 0.1, 0.2] {
            let coordinator = coordinator()
            coordinator.indicatorControl?.onDragBegan?(CGPoint(x: 100, y: 100))
            let now = CACurrentMediaTime()
            let target = CGRect(x: 80, y: 180, width: 100, height: 30)
            coordinator.updateMagneticDrag(pill: CGRect(x: 100, y: 130, width: 58, height: 30),
                target: target, at: now, reduceMotion: false, velocity: .zero)
            coordinator.beginMagneticFusion(to: target, at: now)
            coordinator.advanceMagneticFusion(at: now + progress, reduceMotion: false)
            let drawn = coordinator.indicatorFrame
            coordinator.indicatorControl?.onDragBegan?(CGPoint(x: drawn.midX, y: drawn.midY))
            XCTAssertEqual(coordinator.indicatorFrame, drawn)
            coordinator.advanceMagneticFusion(at: now + 1, reduceMotion: false)
            XCTAssertEqual(coordinator.magneticSample.phase, .dragging)
            XCTAssertEqual(coordinator.indicatorFrame, drawn)
            XCTAssertEqual(NotchDocking.isSaved(in: .standard), savedDocking)
            coordinator.tearDown()
        }
    }

    func testClickAfterRegrabAndTeardownLeaveNoInteraction() {
        let coordinator = coordinator()
        coordinator.indicatorControl?.onDragBegan?(CGPoint(x: 100, y: 100))
        coordinator.indicatorControl?.onDragBegan?(CGPoint(x: 110, y: 105))
        coordinator.indicatorControl?.onClick?()
        XCTAssertFalse(coordinator.hasMagneticInteraction)
        XCTAssertEqual(coordinator.magneticSample.phase, .idle)
        coordinator.indicatorControl?.onDragBegan?(CGPoint(x: 110, y: 105))
        coordinator.tearDown()
        XCTAssertFalse(coordinator.hasMagneticInteraction)
        XCTAssertEqual(coordinator.magneticSample.phase, .idle)
        XCTAssertFalse(coordinator.magneticFieldPanel.isVisible)
    }
}
