import AppKit
import XCTest
@testable import FERBNotch
import NotchCore

/// Owner, 2026-11-04: with hover off, a chat opened by a click on the closed notch folded as soon as the pointer
/// left it. It now stays open until Escape, the shortcut, Hide or a click elsewhere; a hover-opened chat still folds
/// on hover-out. Driven through the real PanelCoordinator, with the pointer placed and the hover grace ended at once.
@MainActor
final class ClickOpenedChatTests: XCTestCase {
    private struct Harness {
        let model: NotchModel
        let coordinator: PanelCoordinator
        let control: FerbIndicatorControl
        let housing: CGPoint
        let away: CGPoint
    }

    private var pointer = CGPoint.zero

    private func harness(hover: Bool) throws -> Harness {
        _ = NSApplication.shared
        guard let screen = NSScreen.screens.first else { throw XCTSkip("Needs a display") }
        let defaults = scratchDefaults()
        NotchDocking.save(true, in: defaults)
        let model = NotchModel(rpc: ScriptedRPC(), preferences: defaults)
        model.setHoverEnabled(hover)
        let coordinator = PanelCoordinator(model: model, defaults: defaults)
        coordinator.updateScreen()
        coordinator.hoverWait = { _ in }
        coordinator.pointerLocation = { [unowned self] in self.pointer }
        let control = try XCTUnwrap(coordinator.indicatorControl)
        control.pointerLocation = { [unowned self] in self.pointer }
        let window = try XCTUnwrap(control.window)
        // The camera housing when docked (a Mac without a notch falls back to the edge pill).
        let local = model.isNotchDocked ? CGPoint(x: control.bounds.midX, y: control.bounds.maxY - 6)
                                        : CGPoint(x: control.bounds.midX, y: control.bounds.midY)
        let housing = window.convertPoint(toScreen: control.convert(local, to: nil))
        let away = CGPoint(x: screen.frame.midX, y: screen.frame.minY + 20)
        return Harness(model: model, coordinator: coordinator, control: control, housing: housing, away: away)
    }

    private func press(_ h: Harness) throws {
        pointer = h.housing
        let window = try XCTUnwrap(h.control.window)
        func event(_ type: NSEvent.EventType) throws -> NSEvent {
            try XCTUnwrap(NSEvent.mouseEvent(with: type, location: window.convertPoint(fromScreen: pointer),
                modifierFlags: [], timestamp: 0, windowNumber: window.windowNumber, context: nil,
                eventNumber: 1, clickCount: 1, pressure: 1))
        }
        h.control.mouseDown(with: try event(.leftMouseDown))
        h.control.mouseUp(with: try event(.leftMouseUp))
        h.coordinator.updatePointer()
    }

    private func move(_ h: Harness, to point: CGPoint) async {
        pointer = point
        h.coordinator.updatePointer()
        await h.coordinator.hoverTask?.value
    }

    private func escape(_ h: Harness) throws {
        let panel = try XCTUnwrap(NSApp.windows.first { $0.delegate === h.coordinator } as? NotchPanel)
        panel.cancelOperation(nil)
    }

    // decision: APP-707, APP-705
    func testAChatOpenedByAPressStaysOpenUntilEscapeOrAClickElsewhere() async throws {
        for hover in [false, true] {
            let h = try harness(hover: hover)
            defer { h.coordinator.tearDown() }
            try press(h)
            XCTAssertTrue(h.model.bodyVisible, "hover \(hover): the press didn't open FERB")
            await move(h, to: h.away)
            XCTAssertTrue(h.model.bodyVisible, "hover \(hover): the press-opened chat folded on hover-out")
            try escape(h)
            XCTAssertFalse(h.model.expanded, "hover \(hover): Escape didn't close it")

            try press(h)
            await move(h, to: h.away)
            XCTAssertTrue(h.model.bodyVisible, "hover \(hover)")
            h.coordinator.ownerClickedInAnotherApp()
            XCTAssertFalse(h.model.expanded, "hover \(hover): a click elsewhere didn't close it")

            try press(h)
            h.model.toggle()            // the double-tap shortcut
            XCTAssertFalse(h.model.expanded, "hover \(hover): the shortcut didn't close it")
        }
    }

    // decision: APP-709
    func testAChatOpenedByTheDoubleTapStaysOpenOnHoverOut() async throws {
        for hover in [false, true] {
            let h = try harness(hover: hover)
            defer { h.coordinator.tearDown() }
            pointer = h.away
            h.coordinator.modifierDoubleTapped()
            XCTAssertTrue(h.model.bodyVisible, "hover \(hover): the double-tap didn't open FERB")
            await move(h, to: h.housing)
            await move(h, to: h.away)
            XCTAssertTrue(h.model.bodyVisible, "hover \(hover): the double-tap-opened chat folded on hover-out")
            h.coordinator.modifierDoubleTapped()
            XCTAssertFalse(h.model.expanded, "hover \(hover): the double-tap didn't close it")
        }
    }

    // decision: APP-557, APP-707
    func testAHoverOpenedChatStillFoldsWhenThePointerLeaves() async throws {
        let h = try harness(hover: true)
        defer { h.coordinator.tearDown() }
        // A press-opened chat closed first: what follows is an ordinary hover open.
        try press(h)
        try escape(h)
        await move(h, to: h.away)
        await move(h, to: h.housing)
        XCTAssertTrue(h.model.bodyVisible, "hover didn't open FERB")
        await move(h, to: h.away)
        XCTAssertFalse(h.model.expanded, "the hover-opened chat stayed after the pointer left")
    }
}
