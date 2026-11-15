import AppKit
import Combine
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

    /// The alpha the window server sees at a point of the handle (its own unflipped coordinates): it passes a press on
    /// a fully transparent pixel through to the window below and delivers it on any other (APP-705).
    private func pressAlpha(_ control: NSView, at point: CGPoint) throws -> CGFloat {
        control.layoutSubtreeIfNeeded()
        let rep = try XCTUnwrap(control.bitmapImageRepForCachingDisplay(in: control.bounds))
        control.cacheDisplay(in: control.bounds, to: rep)
        let scale = CGFloat(rep.pixelsWide) / control.bounds.width
        let color = try XCTUnwrap(rep.colorAt(x: Int(point.x * scale), y: Int((control.bounds.height - point.y) * scale)))
        return color.alphaComponent
    }

    /// Owner, 2026-11-04: with "Open when I hover over FERB" off there was no way to grab the docked notch, because
    /// the closed handle drew nothing and every press went through to the menu bar. Only the camera housing takes
    /// the press; the strip below it and anything inside the open surface stay as they were.
    // decision: APP-705, APP-706
    func testOnlyTheClosedDockedHandlesHousingTakesAPress() throws {
        _ = NSApplication.shared
        let control = FerbIndicatorControl(model: NotchModel(rpc: ScriptedRPC(), preferences: scratchDefaults()))
        let cutout = CGRect(x: 600, y: 868, width: 200, height: 32)
        let region = NotchDocking.dragRegion(cutout: cutout)
        control.frame = CGRect(origin: .zero, size: region.size)
        // In a window (never shown), as in FERB.
        let window = FerbIndicatorPanel(contentRect: control.frame, styleMask: [.borderless, .nonactivatingPanel],
                                        backing: .buffered, defer: true)
        window.isOpaque = false
        window.backgroundColor = .clear
        window.contentView = control
        control.isDocked = true
        let housing = CGPoint(x: region.width / 2, y: region.height - 6)
        let strip = CGPoint(x: region.width / 2, y: NotchDocking.pullDepth / 2)
        XCTAssertGreaterThan(try pressAlpha(control, at: housing), 0, "the housing passed the press through")
        XCTAssertLessThan(try pressAlpha(control, at: housing), 0.02, "the housing's fill must stay invisible")
        XCTAssertEqual(try pressAlpha(control, at: strip), 0, "the strip under the housing took the press")
        XCTAssertEqual(try pressAlpha(control, at: CGPoint(x: 1, y: 1)), 0)
        control.isAttached = true
        XCTAssertEqual(try pressAlpha(control, at: housing), 0, "inside the open surface the surface takes the press")
    }

    /// A press on the closed handle, docked in the notch or on an edge, opens FERB with focused typing whatever the
    /// hover setting, and a press-and-drag from the housing tears it out of the notch.
    // decision: APP-705, APP-597
    func testAPressOnTheClosedHandleOpensOrDragsWithHoverOff() throws {
        _ = NSApplication.shared
        guard NSScreen.screens.first != nil else { throw XCTSkip("Needs a display") }
        for docked in [true, false] {
            for drag in [false, true] {
                let defaults = scratchDefaults()
                NotchDocking.save(docked, in: defaults)
                let model = NotchModel(rpc: ScriptedRPC(), preferences: defaults)
                model.setHoverEnabled(false)
                let coordinator = PanelCoordinator(model: model, defaults: defaults)
                defer { coordinator.tearDown() }
                coordinator.updateScreen()
                let control = try XCTUnwrap(coordinator.indicatorControl)
                let window = try XCTUnwrap(control.window)
                XCTAssertFalse(model.bodyVisible)
                // In the housing when docked (on a Mac without a notch it falls back to the pill), in the pill if not.
                let local = model.isNotchDocked ? CGPoint(x: control.bounds.midX, y: control.bounds.maxY - 6)
                                                : CGPoint(x: control.bounds.midX, y: control.bounds.midY)
                if model.isNotchDocked {
                    XCTAssertGreaterThan(try pressAlpha(control, at: local), 0, "the press on the housing went through")
                }
                var pointer = window.convertPoint(toScreen: control.convert(local, to: nil))
                control.pointerLocation = { pointer }
                func event(_ type: NSEvent.EventType) throws -> NSEvent {
                    try XCTUnwrap(NSEvent.mouseEvent(with: type, location: window.convertPoint(fromScreen: pointer),
                        modifierFlags: [], timestamp: 0, windowNumber: window.windowNumber, context: nil,
                        eventNumber: 1, clickCount: 1, pressure: 1))
                }
                let focused = drag ? nil : expectation(description: "focused typing")
                let watch = model.$focusRequest.dropFirst().sink { _ in focused?.fulfill() }
                defer { watch.cancel() }
                control.mouseDown(with: try event(.leftMouseDown))
                if drag {
                    pointer.y -= 130
                    control.mouseDragged(with: try event(.leftMouseDragged))
                    XCTAssertFalse(model.isNotchDocked, "docked \(docked): the drag didn't tear it out")
                    control.mouseUp(with: try event(.leftMouseUp))
                    XCTAssertFalse(model.bodyVisible, "a drag is not a click")
                } else {
                    control.mouseUp(with: try event(.leftMouseUp))
                    XCTAssertTrue(model.bodyVisible, "docked \(docked): the press didn't open FERB")
                    // The focus request follows on the main queue (present); this only drains it.
                    if let focused { wait(for: [focused], timeout: 5) }
                }
            }
        }
    }

    /// Owner, 0.27.1 on macOS 27.2: "drag works, but it only doesnt work at the extreme top of the notch, fix that, it
    /// should work at all notch area". The window server never gives a window the press on its own topmost point row,
    /// so the handle and the open surface, both flush with the screen's top edge, lost that row to the menu bar; and the
    /// pill's corner radius clipped the closed housing's fill at its square top corners. The edge row reaches above the
    /// screen's top edge across the housing alone, and a press there opens or drags FERB, closed or open.
    // decision: APP-710, APP-711, APP-705
    func testThePressReachesTheHousingsTopmostRowAndCorners() throws {
        // The geometry: the row reaches above the top edge, covers the topmost row, and is no wider than the housing.
        let cutout = CGRect(x: 600.5, y: 868, width: 179, height: 32)
        let row = NotchDocking.edgeRow(cutout: cutout)
        XCTAssertGreaterThanOrEqual(row.maxY, cutout.maxY + 1, "the row must reach above the screen's top edge")
        XCTAssertLessThanOrEqual(row.minY, cutout.maxY - 1, "the row must cover the topmost point row")
        XCTAssertEqual(row.minX, cutout.minX)
        XCTAssertEqual(row.maxX, cutout.maxX)

        // The closed, docked handle's fill reaches its square top corners (no pill corner radius clipping it).
        _ = NSApplication.shared
        let lone = FerbIndicatorControl(model: NotchModel(rpc: ScriptedRPC(), preferences: scratchDefaults()))
        let region = NotchDocking.dragRegion(cutout: cutout)
        lone.frame = CGRect(origin: .zero, size: region.size)
        let window = FerbIndicatorPanel(contentRect: lone.frame, styleMask: [.borderless, .nonactivatingPanel],
                                        backing: .buffered, defer: true)
        window.isOpaque = false
        window.backgroundColor = .clear
        window.contentView = lone
        lone.isDocked = true
        for x in [CGFloat(0.5), region.width - 0.5] {
            XCTAssertGreaterThan(try pressAlpha(lone, at: CGPoint(x: x, y: region.height - 0.5)), 0,
                                 "the housing's top corner at x \(x) passed the press through")
        }

        // The real coordinator: docked, the row's window sits over the housing above the top edge and takes the press.
        guard let screen = NSScreen.screens.first(where: { $0.safeAreaInsets.top > 0 && $0.auxiliaryTopLeftArea != nil })
        else { throw XCTSkip("Needs a display with a hardware notch") }
        for content in ["closed", "chat"] {
            for drag in [false, true] {
                let defaults = scratchDefaults()
                NotchDocking.save(true, in: defaults)
                if let id = screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber {
                    defaults.set(id.intValue, forKey: "notch.indicatorDisplayID")
                }
                let model = NotchModel(rpc: ScriptedRPC(), preferences: defaults)
                model.setHoverEnabled(false)
                let coordinator = PanelCoordinator(model: model, defaults: defaults)
                defer { coordinator.tearDown() }
                coordinator.updateScreen()
                XCTAssertTrue(model.isNotchDocked)
                if content == "chat" { model.expand(focus: false) }
                let edge = coordinator.edgeRowPanel
                XCTAssertTrue(edge.isVisible, "\(content): no window over the housing's topmost row")
                XCTAssertGreaterThanOrEqual(edge.frame.maxY, screen.frame.maxY + 1, "\(content): it stops at the edge")
                XCTAssertLessThanOrEqual(edge.frame.minY, screen.frame.maxY - 1)
                let housingX = ((screen.auxiliaryTopLeftArea?.maxX ?? 0) + (screen.auxiliaryTopRightArea?.minX ?? 0)) / 2
                XCTAssertTrue(edge.frame.minX < housingX && housingX < edge.frame.maxX)
                XCTAssertLessThanOrEqual(edge.frame.width, model.handleWidth + 1, "\(content): wider than the housing")
                let view = try XCTUnwrap(edge.contentView)
                XCTAssertGreaterThan(try pressAlpha(view, at: CGPoint(x: view.bounds.midX, y: view.bounds.midY)), 0)
                XCTAssertLessThan(try pressAlpha(view, at: CGPoint(x: view.bounds.midX, y: view.bounds.midY)), 0.02)
                let control = try XCTUnwrap(coordinator.indicatorControl)
                var pointer = CGPoint(x: housingX, y: screen.frame.maxY - 0.5)
                control.pointerLocation = { pointer }
                func event(_ type: NSEvent.EventType) throws -> NSEvent {
                    try XCTUnwrap(NSEvent.mouseEvent(with: type, location: edge.convertPoint(fromScreen: pointer),
                        modifierFlags: [], timestamp: 0, windowNumber: edge.windowNumber, context: nil,
                        eventNumber: 1, clickCount: 1, pressure: 1))
                }
                view.mouseDown(with: try event(.leftMouseDown))
                if drag {
                    pointer.y -= 130
                    view.mouseDragged(with: try event(.leftMouseDragged))
                    XCTAssertFalse(model.isNotchDocked, "\(content): a drag from the topmost row didn't tear it out")
                    view.mouseUp(with: try event(.leftMouseUp))
                    // Undocked on the top edge the row follows the pill (APP-730), which rests clear of the housing.
                    let housing = CGRect(x: housingX - 1, y: screen.frame.maxY - 1, width: 2, height: 1)
                    XCTAssertFalse(edge.isVisible && edge.frame.intersects(housing),
                                   "\(content): undocked, nothing may take the housing's row")
                } else {
                    view.mouseUp(with: try event(.leftMouseUp))
                    XCTAssertEqual(model.bodyVisible, content == "closed", "\(content): the press didn't toggle FERB")
                }
            }
        }
    }

    /// D31 (owner, 2026-11-05): the notchless top-edge pill takes a press on its very top row too, as the notch does.
    /// The row follows the pill and its resting line along the top edge, only over their drawn span, and hands the
    /// press to the handle; it is out while the handle is dragged and on any other edge.
    // decision: APP-730, APP-735, APP-705
    func testTheTopEdgePillTakesAPressOnItsTopmostRow() async throws {
        _ = NSApplication.shared
        guard let screen = NSScreen.screens.first else { throw XCTSkip("Needs a display") }
        let pill = CGRect(x: 700, y: 870, width: 58, height: 30)
        let row = NotchDocking.edgeRow(over: pill, cornerRadius: 15)
        XCTAssertEqual(row.maxY, pill.maxY + 1)
        XCTAssertEqual(row.minY, pill.maxY - 1)
        XCTAssertEqual(row.minX, pill.minX + 15)
        XCTAssertEqual(row.maxX, pill.maxX - 15)

        let defaults = scratchDefaults()
        if let id = screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber {
            defaults.set(id.intValue, forKey: "notch.indicatorDisplayID")
        }
        let model = NotchModel(rpc: ScriptedRPC(), preferences: defaults)
        model.setHoverEnabled(false)
        let coordinator = PanelCoordinator(model: model, defaults: defaults)
        defer { coordinator.tearDown() }
        coordinator.cameraHousing = { _ in (0, nil, nil) }       // a notchless display
        coordinator.fullScreenSpace = { _ in false }
        coordinator.restWait = { _ in }
        coordinator.updateScreen()
        XCTAssertFalse(model.isNotchDocked)
        let edge = coordinator.edgeRowPanel
        let control = try XCTUnwrap(coordinator.indicatorControl)
        let handle = try XCTUnwrap(control.window)
        let pillFrame = handle.frame
        XCTAssertTrue(edge.isVisible, "no window over the pill's topmost row")
        XCTAssertGreaterThanOrEqual(edge.frame.maxY, screen.frame.maxY + 1)
        XCTAssertTrue(edge.frame.minX >= handle.frame.minX && edge.frame.maxX <= handle.frame.maxX,
                      "the row reaches past the pill")
        var pointer = CGPoint(x: edge.frame.midX, y: screen.frame.maxY - 0.5)
        control.pointerLocation = { pointer }
        coordinator.pointerLocation = { pointer }
        let view = try XCTUnwrap(edge.contentView)
        func event(_ type: NSEvent.EventType) throws -> NSEvent {
            try XCTUnwrap(NSEvent.mouseEvent(with: type, location: edge.convertPoint(fromScreen: pointer),
                modifierFlags: [], timestamp: 0, windowNumber: edge.windowNumber, context: nil,
                eventNumber: 1, clickCount: 1, pressure: 1))
        }
        view.mouseDown(with: try event(.leftMouseDown))
        view.mouseUp(with: try event(.leftMouseUp))
        XCTAssertTrue(model.bodyVisible, "a press on the pill's topmost row didn't open FERB")
        model.collapse(keyboard: true)                          // Escape: no closing animation to wait for
        pointer = CGPoint(x: screen.frame.midX, y: screen.frame.minY + 20)
        coordinator.updatePointer()
        await coordinator.indicatorRestTask?.value
        XCTAssertTrue(coordinator.pillResting)
        XCTAssertTrue(edge.isVisible && edge.frame.maxY >= screen.frame.maxY + 1, "the resting line lost its row")
        // The resting line (half the virtual notch's width, centred where the pill was) is flat along the edge.
        XCTAssertEqual(edge.frame.midX, pillFrame.midX, accuracy: 0.5)
        XCTAssertEqual(edge.frame.width, model.handleWidth / 2, accuracy: 0.5)
    }

    /// Live 2026-11-02 (a friend's M1, no notch): the handle never sat at the top centre and jumped sideways when
    /// dropped near it, because the virtual notch there was avoided like a camera. Only a hardware notch is avoided.
    // decision: APP-640, APP-625
    func testTheTopHandleStepsAsideOnlyFromAHardwareNotch() {
        let screen = CGRect(x: 0, y: 0, width: 1440, height: 900)
        let middle = screen.midX - 29
        XCTAssertEqual(IndicatorPlacement.safeTopX(proposedX: middle, size: 58, screen: screen, notch: nil), middle)
        let camera = CGRect(x: 620, y: 868, width: 200, height: 32)
        let aside = IndicatorPlacement.safeTopX(proposedX: middle, size: 58, screen: screen, notch: camera)
        XCTAssertTrue(aside + 58 <= camera.minX || aside >= camera.maxX)
        XCTAssertEqual(IndicatorPlacement.safeTopX(proposedX: 2000, size: 58, screen: screen, notch: nil), 1382)
    }
}
