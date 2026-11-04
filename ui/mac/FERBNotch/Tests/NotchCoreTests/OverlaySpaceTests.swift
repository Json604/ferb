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
}
