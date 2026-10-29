import XCTest
import SwiftUI
@testable import FERBNotch

final class NotchDockingTests: XCTestCase {
    let notch = CGRect(x: 600, y: 870, width: 180, height: 30)

    func testOnlyPhysicalNotchAndItsPullStripAcceptDocking() {
        XCTAssertTrue(NotchDocking.accepts(CGPoint(x: 690, y: 885), cutout: notch, hasHardwareNotch: true))
        XCTAssertTrue(NotchDocking.accepts(CGPoint(x: 690, y: 865), cutout: notch, hasHardwareNotch: true))
        XCTAssertFalse(NotchDocking.accepts(CGPoint(x: 590, y: 885), cutout: notch, hasHardwareNotch: true))
        XCTAssertFalse(NotchDocking.accepts(CGPoint(x: 690, y: 840), cutout: notch, hasHardwareNotch: true))
        XCTAssertFalse(NotchDocking.accepts(CGPoint(x: 690, y: 885), cutout: notch, hasHardwareNotch: false))
    }

    func testPullStripTouchesCameraWithoutCoveringIt() {
        let strip = NotchDocking.pullStrip(cutout: notch)
        XCTAssertEqual(strip.maxY, notch.minY)
        XCTAssertEqual(strip.width, notch.width)
        XCTAssertEqual(strip.midX, notch.midX)
        XCTAssertFalse(strip.intersects(notch.insetBy(dx: 1, dy: 1)))
    }

    func testDragRegionIncludesWholeCameraAndItsLowerLip() {
        let region = NotchDocking.dragRegion(cutout: notch)
        XCTAssertEqual(region.maxY, notch.maxY)
        XCTAssertEqual(region.minY, notch.minY - 12)
        for point in [CGPoint(x: 601, y: 899), CGPoint(x: 779, y: 875), CGPoint(x: 690, y: 860)] {
            XCTAssertTrue(region.contains(point))
        }
        XCTAssertFalse(region.contains(CGPoint(x: 599, y: 880)))
    }

    @MainActor
    func testDockedCameraDragTakesPriorityOverTopResizeWithoutCoveringWings() {
        let model = NotchModel(preferences: UserDefaults(suiteName: UUID().uuidString)!)
        model.expand()
        let backdrop = NotchBackdropView(frame: CGRect(x: 0, y: 0, width: 456, height: 500))
        let resize = PanelResizeOverlay(model: model)
        resize.frame = CGRect(x: 8, y: 0, width: 440, height: 480)
        backdrop.addSubview(resize)
        let control = FerbIndicatorControl(model: model)
        control.isDocked = true
        control.frame = CGRect(x: 138, y: 0, width: 180, height: 42)
        backdrop.addSubview(control)
        resize.dragExclusionRect = control.frame
        backdrop.docked = true
        backdrop.updateSurface(size: CGSize(width: 440, height: 480), expansion: 1,
                               cutoutHeight: 30, origin: CGPoint(x: 8, y: 0))
        for point in [CGPoint(x: 140, y: 2), CGPoint(x: 228, y: 15), CGPoint(x: 315, y: 40)] {
            XCTAssertNil(resize.hitTest(point))
            XCTAssertTrue(control.hitTest(point) === control)
        }
        XCTAssertTrue(resize.hitTest(CGPoint(x: 50, y: 5)) === resize)
        XCTAssertTrue(resize.hitTest(CGPoint(x: 10, y: 150)) === resize)
        XCTAssertTrue(resize.hitTest(CGPoint(x: 228, y: 478)) === resize)
        XCTAssertFalse(backdrop.hitTest(CGPoint(x: 80, y: 25)) === control)
    }

    func testSwallowScalesEntireCanvasIntoNotchWithoutClippingEitherEdge() {
        let canvas = CGSize(width: 504, height: 620)
        for anchor in [CGPoint(x: 0.5, y: 0.5), .zero, CGPoint(x: 0, y: 1), CGPoint(x: 0.3, y: 0.8)] {
            let anchored = CGRect(x: -anchor.x * canvas.width, y: -anchor.y * canvas.height,
                                  width: canvas.width, height: canvas.height)
            for flipped in [true, false] {
                let transformed = anchored.applying(NotchDocking.swallowTransform(
                    canvas: canvas, target: notch.size, flipped: flipped, anchor: anchor))
                XCTAssertEqual(transformed.width, notch.width, accuracy: 0.001)
                XCTAssertEqual(transformed.height, notch.height, accuracy: 0.001)
                XCTAssertEqual(transformed.minX, anchored.minX, accuracy: 0.001)
                XCTAssertEqual(flipped ? transformed.minY : transformed.maxY,
                               flipped ? anchored.minY : anchored.maxY, accuracy: 0.001)
            }
        }
    }

    func testDockedPanelUsesScreenTopRatherThanBottomOfCamera() {
        let frame = NotchDocking.panelFrame(cutout: notch, size: CGSize(width: 456, height: 500))
        XCTAssertEqual(frame.maxY, notch.maxY)
        XCTAssertEqual(frame.midX, notch.midX)
    }

    func testOriginalNotchShouldersReachScreenEdgeWhileFloatingCardStaysRounded() {
        let rect = CGRect(x: 0, y: 0, width: 440, height: 500)
        XCTAssertTrue(NotchSurfaceShape(expansion: 1, docked: true).path(in: rect).contains(CGPoint(x: 5, y: 0.1)))
        XCTAssertFalse(NotchSurfaceShape(expansion: 1, docked: false).path(in: rect).contains(CGPoint(x: 5, y: 0.1)))
    }

    func testDockingPreferenceSurvivesReloadAndUndocking() {
        let defaults = UserDefaults(suiteName: UUID().uuidString)!
        XCTAssertFalse(NotchDocking.isSaved(in: defaults))
        NotchDocking.save(true, in: defaults)
        XCTAssertTrue(NotchDocking.isSaved(in: defaults))
        NotchDocking.save(false, in: defaults)
        XCTAssertFalse(NotchDocking.isSaved(in: defaults))
    }
}

final class PillShoulderTests: XCTestCase {
    func testShoulderGrowsBeforeTheFinalThreePointsInsteadOfPoppingIn() {
        let pill = CGRect(x: 0, y: 100, width: 58, height: 30)
        // The chat is already growing out of the handle, but its edge is still
        // more than three points from the handle tip.
        let card = CGRect(x: 45, y: 0, width: 350, height: 240)
        let path = PillShoulder.path(pill: pill, card: card, edge: .left, expansion: 0.8)
        XCTAssertTrue(path.contains(CGPoint(x: 44, y: 98)))
    }

    func testSharedSpringGeometryIsContinuousOnEveryScreenEdgeInBothDirections() {
        let pill = CGRect(x: 0, y: 100, width: 58, height: 30)
        let transforms: [(FERBScreenEdge, CGAffineTransform)] = [
            (.left, .identity),
            (.right, CGAffineTransform(a: -1, b: 0, c: 0, d: 1, tx: 600, ty: 0)),
            (.top, CGAffineTransform(a: 0, b: 1, c: 1, d: 0, tx: 0, ty: 0)),
            (.bottom, CGAffineTransform(a: 0, b: -1, c: 1, d: 0, tx: 0, ty: 600))
        ]
        for (edge, transform) in transforms {
            let values = (0...100).map { CGFloat($0) / 100 }
            for progress in values + values.reversed() {
                let card = CGRect(x: 56 * progress, y: 100 * (1 - progress),
                                  width: 58 + 382 * progress, height: 30 + 220 * progress)
                let original = PillShoulder.path(pill: pill, card: card, edge: .left, expansion: progress)
                let reflected = PillShoulder.path(pill: pill.applying(transform), card: card.applying(transform),
                                                 edge: edge, expansion: progress)
                XCTAssertEqual(original.isEmpty, reflected.isEmpty)
                if !original.isEmpty {
                    let expected = original.boundingBoxOfPath.applying(transform)
                    XCTAssertEqual(reflected.boundingBoxOfPath.minX, expected.minX, accuracy: 0.001)
                    XCTAssertEqual(reflected.boundingBoxOfPath.minY, expected.minY, accuracy: 0.001)
                    XCTAssertEqual(reflected.boundingBoxOfPath.width, expected.width, accuracy: 0.001)
                    XCTAssertEqual(reflected.boundingBoxOfPath.height, expected.height, accuracy: 0.001)
                    XCTAssertLessThanOrEqual(original.boundingBoxOfPath.maxX, card.minX + 1.001)
                }
            }
        }
    }

    func testShoulderDoesNotJumpAcrossFormerContactThreshold() {
        let pill = CGRect(x: 0, y: 100, width: 58, height: 30)
        let before = PillShoulder.path(pill: pill, card: CGRect(x: 54.99, y: 0, width: 440, height: 250), edge: .left, expansion: 0.98)
        let after = PillShoulder.path(pill: pill, card: CGRect(x: 55.01, y: 0, width: 440, height: 250), edge: .left, expansion: 0.981)
        XCTAssertFalse(before.isEmpty)
        XCTAssertLessThan(abs(after.boundingBoxOfPath.height - before.boundingBoxOfPath.height), 0.1)
    }
}

extension NotchDockingTests {
    func testHoverIncludesExactPhysicalTopBoundary() {
        XCTAssertTrue(NotchDocking.accepts(CGPoint(x: notch.midX, y: notch.maxY), cutout: notch, hasHardwareNotch: true))
    }

    @MainActor
    func testCameraControlTracksPointerWithoutNeedingAKeyWindow() {
        let model = NotchModel(preferences: UserDefaults(suiteName: UUID().uuidString)!)
        let control = FerbIndicatorControl(model: model)
        control.isDocked = true
        control.frame.size = CGSize(width: 180, height: 42)
        control.updateTrackingAreas()
        XCTAssertTrue(control.trackingAreas.contains { area in
            area.options.contains(.activeAlways) && area.options.contains(.mouseEnteredAndExited)
                && area.options.contains(.mouseMoved) && area.options.contains(.inVisibleRect)
        })
    }
}
