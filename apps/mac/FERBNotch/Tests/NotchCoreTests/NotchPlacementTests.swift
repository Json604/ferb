import AppKit
import XCTest
@testable import FERBNotch
import NotchCore

/// Where FERB sits and how it moves (owner's live run of the intro build, 2026-11-01: "the notch was on the side, not
/// movable").
@MainActor
final class NotchPlacementTests: XCTestCase {
    /// A first run (nothing saved) sits in the hardware notch; a Mac without one gets the handle at the top centre.
    /// Only the owner's own drag changes that: tearing it out of the notch, or dropping it on another edge.
    // decision: APP-550, APP-597
    func testAFirstRunSitsInTheNotchOrAtTheTopCentre() {
        let fresh = scratchDefaults("NotchPlacement")
        XCTAssertTrue(NotchDocking.isDocked(in: fresh, hasNotch: true))
        XCTAssertFalse(NotchDocking.isDocked(in: fresh, hasNotch: false))
        XCTAssertEqual(NotchDocking.edge(in: fresh), .top)
        XCTAssertEqual(NotchDocking.fraction(in: fresh), 0.5)

        let moved = scratchDefaults("NotchPlacement")
        NotchDocking.save(false, in: moved)                                // the owner tore it out of the notch
        moved.set("left", forKey: "notch.indicatorEdge")
        moved.set(0.25, forKey: "notch.indicatorFraction")
        XCTAssertFalse(NotchDocking.isDocked(in: moved, hasNotch: true))
        XCTAssertEqual(NotchDocking.edge(in: moved), .left)
        XCTAssertEqual(NotchDocking.fraction(in: moved), 0.25)
    }

    /// The handle follows the pointer whenever FERB's surface shows, not only with the chat open: during the tour a
    /// line is always showing, the handle is attached to that surface, and dragging moved an empty, hidden handle
    /// window instead, so FERB looked stuck where it was.
    // decision: APP-597
    func testTheHandleFollowsTheDragWhileATourLineShows() throws {
        _ = NSApplication.shared
        guard let screen = NSScreen.screens.first else { throw XCTSkip("A display is required to lay FERB out") }
        let defaults = scratchDefaults("NotchPlacement")
        NotchDocking.save(false, in: defaults)
        defaults.set("right", forKey: "notch.indicatorEdge")
        defaults.set(0.5, forKey: "notch.indicatorFraction")
        defaults.set(NSNumber(value: Self.displayID(screen)), forKey: "notch.indicatorDisplayID")
        let model = NotchModel(rpc: ScriptedRPC(), preferences: defaults)
        let coordinator = PanelCoordinator(model: model, defaults: defaults)
        defer { coordinator.tearDown() }
        coordinator.updateScreen()
        model.introLine = IntroCopy.hello                                  // the tour speaks: the surface shows
        let control = try XCTUnwrap(coordinator.indicatorControl)
        XCTAssertTrue(control.isAttached)
        let before = try XCTUnwrap(Self.screenFrame(of: control))
        let grab = CGPoint(x: before.midX, y: before.midY)
        let pointer = CGPoint(x: grab.x - 300, y: grab.y - 120)
        control.onDragBegan?(grab)
        control.onDragged?(CGPoint(x: grab.x - 10, y: grab.y - 4))
        control.onDragged?(pointer)
        let during = try XCTUnwrap(Self.screenFrame(of: control))
        XCTAssertEqual(during.midX, pointer.x, accuracy: 2)
        XCTAssertEqual(during.midY, pointer.y, accuracy: 2)
        control.onDragEnded?(pointer)
    }

    /// A first run docks in the notch, and the tour's line shows under it: the first drag tears FERB out with its
    /// surface, under the pointer.
    // decision: APP-597
    func testAFirstRunTearsOutOfTheNotchWhileATourLineShows() throws {
        _ = NSApplication.shared
        guard let screen = NSScreen.screens.first(where: { $0.safeAreaInsets.top > 0 }) else {
            throw XCTSkip("A display with a hardware notch is required")
        }
        let defaults = scratchDefaults("NotchPlacement")                  // nothing saved: a first run
        defaults.set(NSNumber(value: Self.displayID(screen)), forKey: "notch.indicatorDisplayID")
        let model = NotchModel(rpc: ScriptedRPC(), preferences: defaults)
        let coordinator = PanelCoordinator(model: model, defaults: defaults)
        defer { coordinator.tearDown() }
        coordinator.updateScreen()
        XCTAssertTrue(model.isNotchDocked)
        model.introLine = IntroCopy.hello
        let control = try XCTUnwrap(coordinator.indicatorControl)
        let notch = try XCTUnwrap(Self.screenFrame(of: control))
        let grab = CGPoint(x: notch.midX, y: notch.midY)
        let pointer = CGPoint(x: grab.x - 250, y: grab.y - 300)
        control.onDragBegan?(grab)
        control.onDragged?(CGPoint(x: grab.x, y: grab.y - 6))
        control.onDragged?(pointer)
        XCTAssertFalse(model.isNotchDocked)
        let during = try XCTUnwrap(Self.screenFrame(of: control))
        XCTAssertEqual(during.midX, pointer.x, accuracy: 2)
        XCTAssertEqual(during.midY, pointer.y, accuracy: 2)
        control.onDragEnded?(pointer)
    }

    private static func screenFrame(of view: NSView) -> CGRect? {
        guard let window = view.window else { return nil }
        return window.convertToScreen(view.convert(view.bounds, to: nil))
    }

    private static func displayID(_ screen: NSScreen) -> UInt32 {
        (screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint32Value ?? 0
    }
}
