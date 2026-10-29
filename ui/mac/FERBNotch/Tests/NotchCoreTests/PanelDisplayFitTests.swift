import XCTest
import AppKit
import NotchCore
@testable import FERBNotch

@MainActor
final class PanelDisplayFitTests: XCTestCase {
    func testOversizedPreferencesFitDisplayAndRecoverOnLargerDisplay() {
        let model = NotchModel()
        model.preferredPanelWidth = 4000
        model.preferredTranscriptHeight = 3000
        model.availableSurfaceSize = CGSize(width: 1300, height: 800)
        XCTAssertEqual(model.bodyWidth, 1300)
        XCTAssertLessThanOrEqual(model.cutoutHeight + model.chatTranscriptHeight + model.conversationChromeHeight, 800)
        model.availableSurfaceSize = CGSize(width: 5000, height: 4000)
        XCTAssertEqual(model.bodyWidth, 4000)
        XCTAssertEqual(model.chatTranscriptHeight, 3000)
    }

    func testWholePanelFitsEveryEdgeIncludingNotchAndOffsetDisplays() {
        let screen = CGRect(x: -1512, y: 300, width: 1512, height: 982)
        let cutout = CGRect(x: screen.midX - 90, y: screen.maxY - 38, width: 180, height: 38)
        for edge: FERBScreenEdge in [.left, .right, .top, .bottom] {
            for docked in [false, true] {
                let budget = PanelLayout.surfaceBudget(screen: screen, edge: edge, docked: docked)
                let frame = PanelLayout.frame(size: CGSize(width: budget.width + 16, height: budget.height + 12),
                    screen: screen, indicator: cutout, edge: edge, docked: docked, cutout: cutout)
                XCTAssertEqual(frame, screen)
                let oversized = PanelLayout.frame(size: CGSize(width: 9000, height: 9000), screen: screen,
                    indicator: cutout, edge: edge, docked: docked, cutout: cutout)
                XCTAssertEqual(oversized, screen)
            }
        }
    }

    func testDragPreservesGrabOffsetThroughDirectionChanges() {
        let start = CGRect(x: 100, y: 200, width: 58, height: 30)
        let grabbed = CGPoint(x: 110, y: 225)
        for pointer in [CGPoint(x: 700, y: 800), CGPoint(x: -150, y: 100), grabbed] {
            let frame = PanelLayout.draggedFrame(start: start, grabbedAt: grabbed, pointer: pointer)
            XCTAssertEqual(pointer.x - frame.minX, 10)
            XCTAssertEqual(pointer.y - frame.minY, 25)
        }
    }

    func testGlobalPointerDoesNotInheritMovingWindowCoordinates() {
        let model = NotchModel()
        let control = FerbIndicatorControl(model: model)
        var pointer = CGPoint(x: 300, y: 400)
        control.pointerLocation = { pointer }
        var positions: [CGPoint] = []
        control.onDragBegan = { positions.append($0) }
        control.onDragged = { positions.append($0) }
        let event = NSEvent.mouseEvent(with: .leftMouseDown, location: CGPoint(x: 10, y: 10),
            modifierFlags: [], timestamp: 0, windowNumber: 0, context: nil, eventNumber: 1,
            clickCount: 1, pressure: 1)!
        control.mouseDown(with: event)
        pointer = CGPoint(x: 520, y: 230)
        control.mouseDragged(with: event)
        pointer = CGPoint(x: 305, y: 405)
        control.mouseDragged(with: event)
        XCTAssertEqual(positions, [CGPoint(x: 300,y: 400), CGPoint(x: 520,y: 230), CGPoint(x: 305,y: 405)])
    }

    func testHardwareNotchUsesReportedSizeAndRejectsInvalidOrAbsentAreas() {
        let screen = CGRect(x: -1500, y: 200, width: 1500, height: 1000)
        for width: CGFloat in [164, 180, 220] {
            let geometry = NotchGeometry(screen: screen, topInset: 38,
                leftAreaMaxX: screen.midX - width / 2, rightAreaMinX: screen.midX + width / 2, menuBarHeight: 38)
            XCTAssertTrue(geometry.hasNotch)
            XCTAssertEqual(geometry.cutoutFrame.width, width)
            XCTAssertEqual(geometry.cutoutFrame.maxY, screen.maxY)
        }
        for areas: (CGFloat?, CGFloat?) in [(nil,nil), (10,5)] {
            let geometry = NotchGeometry(screen: screen, topInset: 38,
                leftAreaMaxX: areas.0, rightAreaMinX: areas.1, menuBarHeight: 24)
            XCTAssertFalse(geometry.hasNotch)
            XCTAssertFalse(NotchDocking.accepts(geometry.cutoutFrame.center, cutout: geometry.cutoutFrame, hasHardwareNotch: geometry.hasNotch))
        }
    }
}
private extension CGRect { var center: CGPoint { CGPoint(x: midX, y: midY) } }
