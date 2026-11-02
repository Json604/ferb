import AppKit
import XCTest
@testable import FERBNotch
import NotchCore

@MainActor
final class NotchDragTests: XCTestCase {
    // decision: APP-624
    func testClickingTheHandleOpensItsReminderInsteadOfClosingIt() throws {
        _ = NSApplication.shared
        guard NSScreen.screens.first != nil else { throw XCTSkip("Needs a display") }
        let defaults = scratchDefaults()
        let model = NotchModel(rpc: ScriptedRPC(), preferences: defaults)
        let coordinator = PanelCoordinator(model: model, defaults: defaults)
        defer { coordinator.tearDown() }
        coordinator.updateScreen()
        model.onInteraction = { model.openReminderChat() }
        model.presentReminder(card: ResultCard(["type": "reminder", "title": "Review the brief"])!, text: "Review the brief")
        let control = try XCTUnwrap(coordinator.indicatorControl)
        control.pointerLocation = { .zero }
        func event(_ type: NSEvent.EventType) throws -> NSEvent {
            try XCTUnwrap(NSEvent.mouseEvent(with: type, location: .zero, modifierFlags: [], timestamp: 0,
                windowNumber: control.window?.windowNumber ?? 0, context: nil, eventNumber: 1, clickCount: 1, pressure: 1))
        }
        control.mouseDown(with: try event(.leftMouseDown))
        XCTAssertNotNil(model.compactReminderCard)
        control.mouseUp(with: try event(.leftMouseUp))
        XCTAssertTrue(model.bodyVisible)
        XCTAssertNil(model.acknowledgement.reminderCard)
    }

    // decision: APP-597
    func testNativeHitTargetDragsTheVisibleSurface() throws {
        _ = NSApplication.shared
        guard NSScreen.screens.first != nil else { throw XCTSkip("Needs a display") }
        for docked in [false, true] {
            for content in ["closed", "chat", "reminder"] {
                let defaults = scratchDefaults()
                NotchDocking.save(docked, in: defaults)
                defaults.set("left", forKey: "notch.indicatorEdge")
                let model = NotchModel(rpc: ScriptedRPC(), preferences: defaults)
                let coordinator = PanelCoordinator(model: model, defaults: defaults)
                defer { coordinator.tearDown() }
                coordinator.updateScreen()
                if content == "chat" { model.expand(focus: false) }
                if content == "reminder" {
                    model.presentReminder(card: ResultCard(["type": "reminder", "title": "Test reminder"])!, text: "Test reminder")
                }
                let control = try XCTUnwrap(coordinator.indicatorControl)
                let window = try XCTUnwrap(control.window)
                let root = try XCTUnwrap(window.contentView)
                let frame = window.convertToScreen(control.convert(control.bounds, to: nil))
                var pointer = CGPoint(x: frame.midX, y: frame.minY + 6)
                control.pointerLocation = { pointer }
                let point = window.convertPoint(fromScreen: pointer)
                let hit = try XCTUnwrap(root.hitTest(root.superview?.convert(point, from: nil) ?? point), "\(docked) \(content)")
                var owner: NSView? = hit
                while owner != nil && owner !== control { owner = owner?.superview }
                XCTAssertTrue(owner === control, "\(docked) \(content): \(type(of: hit))")
                func event(_ type: NSEvent.EventType) throws -> NSEvent {
                    try XCTUnwrap(NSEvent.mouseEvent(with: type, location: window.convertPoint(fromScreen: pointer),
                        modifierFlags: [], timestamp: 0, windowNumber: window.windowNumber, context: nil,
                        eventNumber: 1, clickCount: 1, pressure: 1))
                }
                hit.mouseDown(with: try event(.leftMouseDown))
                pointer.x += docked ? -200 : 200
                pointer.y -= 130
                hit.mouseDragged(with: try event(.leftMouseDragged))
                let moved = try XCTUnwrap(control.window).convertToScreen(control.convert(control.bounds, to: nil))
                XCTAssertNotEqual(moved.origin, frame.origin, "\(docked) \(content)")
                XCTAssertFalse(model.isNotchDocked, "\(content)")
                if content == "reminder" { XCTAssertNotNil(model.compactReminderCard) }
                hit.mouseUp(with: try event(.leftMouseUp))
            }
        }
    }
}
