import AppKit
import XCTest
@testable import FERBNotch
import NotchCore

@MainActor
final class OverlaySpaceTests: XCTestCase {
    /// v0.27.0 on macOS 27.2 (owner: "the notch is not draggable"): the move and show calls return 3 even when every
    /// window joined and the space shows, and FERB gave the space up on that code. Only what the window server lists
    /// decides; the calls' results aren't even an input.
    // decision: APP-704
    func testTheSpaceCountsWhenItsListsSaySoWhateverTheCallsReturned() {
        let windows = [11, 12, 13, 14]                       // panel, light, handle, magnetic field
        XCTAssertTrue(OverlaySpaceCheck.attached(windows: windows, levelStatus: 0, members: Set(windows),
                                                 shownBefore: [13], onScreen: [13, 99]))
        // A window the space doesn't list, a space that doesn't show (the handle on screen before is gone), or a
        // level that didn't take is no space: FERB keeps its ordinary windows and logs why.
        XCTAssertFalse(OverlaySpaceCheck.attached(windows: windows, levelStatus: 0, members: [11, 12, 13],
                                                  shownBefore: [13], onScreen: [13]))
        XCTAssertFalse(OverlaySpaceCheck.attached(windows: windows, levelStatus: 0, members: Set(windows),
                                                  shownBefore: [13], onScreen: [99]))
        XCTAssertFalse(OverlaySpaceCheck.attached(windows: windows, levelStatus: 1000, members: Set(windows),
                                                  shownBefore: [13], onScreen: [13]))
    }

    /// The real attach on this Mac's window server: the windows join FERB's fixed overlay space (so macOS 27's menu-bar
    /// window, on the same layer, can't cover the docked handle at the camera), a window ordered out and in stays in it,
    /// and stopping leaves nothing behind.
    // decision: APP-704
    func testTheNotchWindowsJoinTheFixedOverlaySpace() throws {
        _ = NSApplication.shared
        guard NSScreen.screens.first != nil else { throw XCTSkip("Needs a display") }
        let sky = dlopen("/System/Library/PrivateFrameworks/SkyLight.framework/Versions/A/SkyLight", RTLD_NOW)
        guard let sky, dlsym(sky, "SLSSpaceAddWindowsAndRemoveFromSpaces") != nil,
              dlsym(sky, "SLSCopyWindowsWithOptionsAndTags") != nil else {
            throw XCTSkip("No SkyLight overlay spaces here: FERB keeps ordinary windows (APP-563)")
        }
        dlclose(sky)
        func panel(shown: Bool) -> NSPanel {
            let window = NSPanel(contentRect: CGRect(x: 0, y: 0, width: 2, height: 2),
                                 styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
            window.backgroundColor = .clear
            window.isOpaque = false
            window.isReleasedWhenClosed = false
            if shown { window.orderFrontRegardless() }
            return window
        }
        let handle = panel(shown: true)
        let hidden = panel(shown: false)                    // the folded panel and the light are ordered out
        let space = NotchOverlaySpace()
        defer {
            space.stop()
            handle.orderOut(nil)
        }
        space.attach([handle, hidden])
        XCTAssertTrue(space.isAttached)
        handle.orderOut(nil)
        handle.orderFrontRegardless()
        space.rejoin(handle)
        XCTAssertTrue(space.isAttached)
        space.stop()
        XCTAssertFalse(space.isAttached)
    }

    /// Owner, 2026-11-05: "IT SHOULD STICK". A notch window on an ordinary Space rides the desktop swipe. After it is
    /// made key and ordered front it is on no ordinary Space, and when AppKit puts it back on the ordinary Spaces (as
    /// a collection-behaviour reset or a yield does) `ensure` moves it back; an already-placed window isn't moved.
    // decision: APP-729, APP-731, APP-563
    func testEnsureKeepsANotchWindowOnNoOrdinarySpace() throws {
        _ = NSApplication.shared
        guard NSScreen.screens.first != nil else { throw XCTSkip("Needs a display") }
        let window = NotchPanel(contentRect: CGRect(x: 0, y: 0, width: 2, height: 2),
                                styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.collectionBehavior = [.canJoinAllSpaces, .stationary, .fullScreenAuxiliary]
        window.orderFrontRegardless()
        let space = NotchOverlaySpace()
        defer {
            space.stop()
            window.orderOut(nil)
        }
        space.attach([window])
        guard space.isAttached else { throw XCTSkip("No SkyLight overlay spaces here: FERB keeps ordinary windows (APP-563)") }
        window.makeKey()
        window.orderFrontRegardless()
        space.ensure(window)
        XCTAssertEqual(space.membership(of: window)?.member, true)
        XCTAssertEqual(space.membership(of: window)?.ordinary, [])
        XCTAssertFalse(space.ensure(window), "a window in place moved")
        let behavior = window.collectionBehavior
        window.collectionBehavior = []
        window.collectionBehavior = behavior
        space.leave(window)                                     // what a yield does: back on the ordinary Spaces
        window.level = .normal
        window.orderFrontRegardless()
        let left = try XCTUnwrap(space.membership(of: window))
        XCTAssertTrue(!left.member || !left.ordinary.isEmpty, "the window never left the overlay space")
        XCTAssertTrue(space.ensure(window))
        XCTAssertEqual(space.membership(of: window)?.member, true)
        XCTAssertEqual(space.membership(of: window)?.ordinary, [], "ensure left it on an ordinary Space")
    }
}
