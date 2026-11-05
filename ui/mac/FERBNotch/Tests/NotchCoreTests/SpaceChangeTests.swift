import AppKit
import XCTest
@testable import FERBNotch
import NotchCore

/// Owner, 2026-11-05: "when notch chat is open and i shift spaces, it goes back and comes down again. IT SHOULD STICK
/// ... It Even pops up on space shift when on hover is turned off in the m1 no notch mac." Driven through the real
/// PanelCoordinator with the real Space-change notification (NSWorkspace's centre, in this process only), its windows
/// in the real overlay space or AppKit's fallback, the pointer placed and every wait ended at once.
@MainActor
final class SpaceChangeTests: XCTestCase {
    private struct Harness {
        let model: NotchModel
        let coordinator: PanelCoordinator
        let panel: NSWindow
        let control: FerbIndicatorControl
        let housing: CGPoint
        let away: CGPoint
    }

    private var pointer = CGPoint.zero

    private func harness(hover: Bool, notchless: Bool = false, overlay: Bool = true,
                         fullScreen: Bool = false) throws -> Harness {
        _ = NSApplication.shared
        guard let screen = NSScreen.screens.first else { throw XCTSkip("Needs a display") }
        let defaults = scratchDefaults()
        NotchDocking.save(true, in: defaults)
        if let id = screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber {
            defaults.set(id.intValue, forKey: "notch.indicatorDisplayID")
        }
        let model = NotchModel(rpc: ScriptedRPC(), preferences: defaults)
        model.setHoverEnabled(hover)
        let coordinator = PanelCoordinator(model: model, defaults: defaults,
                                           overlaySpace: NotchOverlaySpace(available: overlay))
        if notchless { coordinator.cameraHousing = { _ in (0, nil, nil) } }
        coordinator.fullScreenSpace = { _ in fullScreen }
        coordinator.missionControlShowing = { _ in false }
        coordinator.activeSpace = { 1 }
        coordinator.hoverWait = { _ in }
        coordinator.restWait = { _ in }
        coordinator.pointerLocation = { [unowned self] in self.pointer }
        coordinator.updateScreen()
        coordinator.attachOverlay()
        coordinator.observeWorkspace()
        let control = try XCTUnwrap(coordinator.indicatorControl)
        control.pointerLocation = { [unowned self] in self.pointer }
        let window = try XCTUnwrap(control.window)
        let local = model.isNotchDocked ? CGPoint(x: control.bounds.midX, y: control.bounds.maxY - 6)
                                        : CGPoint(x: control.bounds.midX, y: control.bounds.midY)
        let housing = window.convertPoint(toScreen: control.convert(local, to: nil))
        let panel = try XCTUnwrap(NSApp.windows.first { $0.delegate === coordinator })
        pointer = CGPoint(x: screen.frame.midX, y: screen.frame.minY + 20)
        return Harness(model: model, coordinator: coordinator, panel: panel, control: control, housing: housing,
                       away: pointer)
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
    }

    private func move(_ h: Harness, to point: CGPoint) async {
        pointer = point
        h.coordinator.updatePointer()
        await h.coordinator.hoverTask?.value
    }

    /// The Space change as macOS delivers it, several times in a row, each followed by the screen-parameters notice
    /// macOS may post with it (nothing in FERB's layout changed).
    private func switchSpaces(_ h: Harness, times: Int = 3) {
        for space in 0..<times {
            h.coordinator.activeSpace = { UInt64(10 + space) }
            NSWorkspace.shared.notificationCenter.post(name: NSWorkspace.activeSpaceDidChangeNotification, object: nil)
            NotificationCenter.default.post(name: NSApplication.didChangeScreenParametersNotification, object: nil)
        }
    }

    private struct Snapshot: Equatable {
        let expanded: Bool, body: Bool, pinned: Bool, byPress: Bool, yielded: Bool, frame: CGRect, visible: Bool
        let key: Bool, presents: Int, hoverTask: Task<Void, Never>?, plan: String?, scroll: Int?
    }

    private func snapshot(_ h: Harness) -> Snapshot {
        Snapshot(expanded: h.model.expanded, body: h.model.bodyVisible, pinned: h.model.isPinned,
                 byPress: h.model.staysOpenOnHoverOut, yielded: h.model.isYielded, frame: h.panel.frame,
                 visible: h.panel.isVisible, key: h.panel.isKeyWindow, presents: h.coordinator.presentCount,
                 hoverTask: h.coordinator.hoverTask, plan: h.model.acknowledgement.text,
                 scroll: h.model.scrollRequest?.generation)
    }

    /// An open chat, however it was opened, stays exactly as it is through repeated Space switches: open, in place,
    /// no new presentation, the same key state, no yield and the same pending hover; its windows stay on no ordinary
    /// Space, so they never ride the swipe.
    // decision: APP-729, APP-731, APP-733, APP-562, APP-558
    func testAnOpenChatStaysExactlyAsItIsThroughSpaceSwitches() async throws {
        for opening in ["hover", "press", "double-tap", "pinned"] {
            for overlay in [true, false] {
                let h = try harness(hover: opening == "hover", overlay: overlay)
                defer { h.coordinator.tearDown() }
                switch opening {
                case "hover": await move(h, to: h.housing)
                case "press": try press(h)
                case "double-tap": h.coordinator.modifierDoubleTapped()
                default:
                    h.model.expand(focus: false)
                    h.model.togglePin()
                    h.model.revealPinnedBody()
                }
                XCTAssertTrue(h.model.expanded, "\(opening): didn't open")
                h.model.requestScroll("turn", .answer)
                let before = snapshot(h)
                switchSpaces(h)
                XCTAssertEqual(snapshot(h), before, "\(opening), overlay \(overlay): the Space switch changed the chat")
                if overlay, let place = h.coordinator.overlayMembership(of: h.panel) {
                    XCTAssertTrue(place.member, "\(opening): the panel left the overlay space")
                    XCTAssertEqual(place.ordinary, [], "\(opening): the panel is on an ordinary Space (rides the swipe)")
                }
            }
        }
    }

    /// Notchless, hover off, closed (the friend's M1): Space switches leave the panel out and the resting line
    /// resting; nothing of a closed FERB can yield, so no yield can order the empty panel in.
    // decision: APP-729, APP-731, APP-733, APP-518
    func testAClosedNotchlessFERBShowsNothingNewOnASpaceSwitch() async throws {
        for overlay in [true, false] {
            let h = try harness(hover: false, notchless: true, overlay: overlay)
            defer { h.coordinator.tearDown() }
            XCTAssertFalse(h.model.isNotchDocked)
            await h.coordinator.indicatorRestTask?.value
            XCTAssertTrue(h.coordinator.pillResting, "the pill didn't rest after its idle seconds")
            let before = snapshot(h)
            switchSpaces(h)
            h.coordinator.updatePointer()                       // the pointer never moved
            XCTAssertEqual(snapshot(h), before, "overlay \(overlay)")
            XCTAssertFalse(h.panel.isVisible, "overlay \(overlay): the closed panel was ordered in")
            XCTAssertTrue(h.coordinator.pillResting, "overlay \(overlay): the pill popped up")
            XCTAssertEqual(h.coordinator.yieldLayout()?.occupied ?? [], [], "a closed FERB counts as on screen")
        }
    }

    /// D14: on a full-screen app's Space the closed edge pill is its resting line at once, and a background update
    /// doesn't pop it up; the owner reaching for it still does.
    // decision: APP-729, APP-735
    func testOnAFullScreenSpaceTheClosedPillStaysItsRestingLine() async throws {
        let h = try harness(hover: false, notchless: true, fullScreen: true)
        defer { h.coordinator.tearDown() }
        h.coordinator.indicatorRestTask?.cancel()
        switchSpaces(h, times: 1)
        XCTAssertTrue(h.coordinator.pillResting)
        h.model.onBackgroundChange?(true)
        XCTAssertTrue(h.coordinator.pillResting, "a background update popped the pill up")
        await move(h, to: h.housing)
        XCTAssertFalse(h.coordinator.pillResting, "the owner's pointer didn't bring the pill up")
    }

    /// The pointer resting on the notch through a Space change opens nothing; leaving and coming back opens it.
    // decision: APP-729, APP-734, APP-733, APP-553
    func testHoverAfterASpaceChangeNeedsTheOwnersOwnMovement() async throws {
        let h = try harness(hover: true)
        defer { h.coordinator.tearDown() }
        await move(h, to: h.housing)
        XCTAssertTrue(h.model.bodyVisible)
        h.panel.cancelOperation(nil)                            // Escape, the pointer still on the notch
        XCTAssertFalse(h.model.expanded)
        switchSpaces(h)
        h.coordinator.updatePointer()
        await h.coordinator.hoverTask?.value
        XCTAssertFalse(h.model.expanded, "the Space change opened the chat under a pointer that never moved")
        await move(h, to: h.away)
        await move(h, to: h.housing)
        XCTAssertTrue(h.model.bodyVisible, "hovering the notch again didn't open it")
    }

    /// The pointer crossing the notch on its way to Mission Control's Spaces bar opens nothing.
    // decision: APP-729, APP-734
    func testCrossingTheNotchDuringMissionControlOpensNothing() async throws {
        let h = try harness(hover: true, notchless: true)
        defer { h.coordinator.tearDown() }
        await h.coordinator.indicatorRestTask?.value
        h.coordinator.missionControlShowing = { _ in true }
        await move(h, to: h.housing)
        XCTAssertFalse(h.model.expanded)
        XCTAssertTrue(h.coordinator.pillResting, "the pill popped up for Mission Control")
        await move(h, to: h.away)
        h.coordinator.missionControlShowing = { _ in false }
        await move(h, to: h.housing)
        XCTAssertTrue(h.model.bodyVisible)
    }

    /// D15: the six-second planning preview survives a Space switch (APP-624).
    // decision: APP-729, APP-624
    func testThePlanningPreviewSurvivesASpaceSwitch() throws {
        let h = try harness(hover: false)
        defer { h.coordinator.tearDown() }
        h.model.acknowledgement.show("Planning the trip")
        XCTAssertNotNil(h.model.acknowledgement.text)
        switchSpaces(h)
        XCTAssertEqual(h.model.acknowledgement.text, "Planning the trip")
    }

    /// A real display change still lays FERB out again; an unchanged layout is no notice at all.
    // decision: APP-733
    func testOnlyAChangedScreenLayoutLaysFERBOutAgain() throws {
        let h = try harness(hover: false)
        defer { h.coordinator.tearDown() }
        let presents = h.coordinator.presentCount
        h.coordinator.displayParametersChanged()
        XCTAssertEqual(h.coordinator.presentCount, presents)
        h.coordinator.cameraHousing = { _ in (0, nil, nil) }   // the housing went away: a different display
        h.coordinator.displayParametersChanged()
        XCTAssertGreaterThan(h.coordinator.presentCount, presents)
        XCTAssertFalse(h.model.isNotchDocked)
    }

    // decision: APP-733
    func testScreenLayoutAndTheMenuBarHeightIgnoreWhatSpacesChange() {
        let display = ScreenLayout.Display(id: 1, frame: CGRect(x: 0, y: 0, width: 1440, height: 900), topInset: 0,
                                           leftArea: nil, rightArea: nil, scale: 2)
        XCTAssertEqual(ScreenLayout(displays: [display]), ScreenLayout(displays: [display]))
        var moved = display
        moved.frame.origin.x = -1440
        XCTAssertNotEqual(ScreenLayout(displays: [display]), ScreenLayout(displays: [moved]))
        // A full-screen Space or an auto-hiding menu bar reports no gap: the height last seen stands.
        var heights = MenuBarHeights()
        XCTAssertEqual(heights.height(display: 1, visibleGap: 24), 24)
        XCTAssertEqual(heights.height(display: 1, visibleGap: 0), 24)
        XCTAssertEqual(heights.height(display: 2, visibleGap: 0), 0)
        let shown = NotchGeometry(screen: display.frame, topInset: 0, leftAreaMaxX: nil, rightAreaMinX: nil,
                                  menuBarHeight: heights.height(display: 1, visibleGap: 24))
        let hidden = NotchGeometry(screen: display.frame, topInset: 0, leftAreaMaxX: nil, rightAreaMinX: nil,
                                   menuBarHeight: heights.height(display: 1, visibleGap: 0))
        XCTAssertEqual(shown.cutoutFrame, hidden.cutoutFrame)
    }

    // decision: APP-734
    func testMissionControlIsTheWindowManagersWholeDisplayWindowAboveTheNormalLayer() {
        let display = CGRect(x: 0, y: 0, width: 1470, height: 956)
        let manager: Int32 = 400
        let dock = ScreenWindow(windowID: 72, ownerPID: 500, layer: NotchOverlap.dockLayer, bounds: display, alpha: 1)
        let wallpaper = ScreenWindow(windowID: 20, ownerPID: manager, layer: -2147483624, bounds: display, alpha: 1)
        let spacesBar = ScreenWindow(windowID: 33831, ownerPID: manager, layer: 14,
                                     bounds: CGRect(x: 0, y: 0, width: 1470, height: 128), alpha: 1)
        let mission = ScreenWindow(windowID: 33830, ownerPID: manager, layer: 19, bounds: display, alpha: 1)
        func showing(_ rows: [ScreenWindow]) -> Bool {
            SpaceFacts.missionControlShowing(in: rows, display: display) { $0 == manager }
        }
        XCTAssertFalse(showing([dock, wallpaper, spacesBar]))
        XCTAssertTrue(showing([dock, wallpaper, mission, spacesBar]))
        XCTAssertFalse(SpaceFacts.missionControlShowing(in: [mission], display: display) { _ in false })
    }
}
