import XCTest
import CoreGraphics
@testable import NotchCore

final class NotchOverlapTests: XCTestCase {
    private typealias F = YieldFixture

    private func covers(_ windows: [ScreenWindow], frames: [CGRect] = [F.headerPanel]) -> Bool {
        NotchOverlap.foreignOverlay(in: windows, ferbFrames: frames, ownPIDs: [F.ferb], menuBar: F.menuBar, display: F.display)
    }

    // decision: APP-559
    func testAnotherAppsMenuOverTheNotchYields() {
        XCTAssertTrue(covers([F.clipboardMenu]))
        XCTAssertTrue(covers([F.clipboardMenu], frames: [F.openPanel]))
        XCTAssertEqual(NotchOverlap.foreignOverlays(in: [F.maximizedBrowser, F.clipboardMenu, F.shareBar],
                                                    ferbFrames: [F.openPanel], ownPIDs: [F.ferb],
                                                    menuBar: F.menuBar, display: F.display), [F.clipboardMenu, F.shareBar])
    }

    // decision: APP-559
    func testAPopoverAtTheStatusLayerThatReachesBelowTheMenuBarYields() {
        XCTAssertTrue(covers([F.window(F.clipboardApp, layer: 25, 700, 30, 300, 300)]))
    }

    // decision: APP-559
    func testANormalWindowUnderTheNotchDoesNotYield() {
        XCTAssertFalse(covers([F.maximizedBrowser], frames: [F.openPanel]))
    }

    // decision: APP-559
    func testFERBsOwnWindowsAreIgnored() {
        XCTAssertFalse(covers([F.window(F.ferb, layer: 25, 547, 0, 376, 432), F.window(F.ferb, layer: 26, 626, 0, 219, 56),
                               F.window(F.ferb, layer: 8, 579, 127, 312, 402), F.window(F.ferb, layer: 101, 860, 40, 200, 180)],
                              frames: [F.openPanel]))
    }

    // decision: APP-559
    func testAlwaysPresentSystemChromeIsIgnored() {
        let chrome = [
            F.window(F.windowServer, layer: 24, 0, 0, 1470, 33),            // the menu bar
            F.window(301, layer: 25, 830, 0, 30, 33),                       // status items beside the notch
            F.window(302, layer: 25, 600, 0, 28, 24),
            F.window(303, layer: 27, 1400, 4, 8, 8),                        // the privacy indicator
            F.window(514, layer: NotchOverlap.dockLayer, 500, 0, 500, 100), // the Dock layer
            F.window(41154, layer: 26, 0, 0, 1470, 65, alpha: 0),           // an invisible helper window
            F.window(304, layer: 1000, 0, 0, 1470, 956),                    // a whole-display overlay
        ]
        XCTAssertFalse(covers(chrome, frames: [F.openPanel]))
        XCTAssertTrue(covers(chrome + [F.clipboardMenu], frames: [F.openPanel]))
    }

    /// Live 2026-11-05 (owner: "when notch chat is open and i shift spaces, it goes back and comes down again"): while
    /// the owner switches Spaces each Space's menu bar (MenuBarAgent's layer-25 strip and the window server's) slides
    /// across the display with its Space, reaching past its sides, so the strip no longer fit inside the menu bar's
    /// rect and counted as a window over FERB: the chat stepped back and rode the swipe. The menu bar's band never
    /// counts, wherever it sits horizontally; a menu reaching below it still does.
    // decision: APP-729, APP-732, APP-559
    func testAMenuBarSlidingWithItsSpaceIsNotAWindowOverFERB() {
        let sliding = [F.window(F.windowServer, layer: 25, 300, 0, 1470, 33), F.window(F.windowServer, layer: 24, -871, 0, 1470, 33),
                       F.window(F.windowServer, layer: 24, 663, 0, 1470, 33)]
        XCTAssertFalse(covers(sliding, frames: [F.openPanel]))
        XCTAssertTrue(covers(sliding + [F.clipboardMenu], frames: [F.openPanel]))
    }

    // decision: APP-559
    func testWindowsThatOnlyTouchFERBsEdgeDoNotYield() {
        XCTAssertFalse(covers([F.window(F.clipboardApp, layer: 101, 850, 44, 260, 400)]))
        XCTAssertTrue(covers([F.window(F.clipboardApp, layer: 101, 850, 43, 260, 400)]))
    }

    // decision: APP-559
    func testTheCollapsedTabOnADisplayWithoutANotchCounts() {
        let display = CGRect(x: 0, y: 0, width: 1920, height: 1080)
        let menuBar = CGRect(x: 0, y: 0, width: 1920, height: 24)
        let tab = CGRect(x: 878, y: 24, width: 164, height: 24)
        let menu = F.window(F.clipboardApp, layer: 101, 900, 24, 200, 300)
        XCTAssertTrue(NotchOverlap.foreignOverlay(in: [menu], ferbFrames: [tab], ownPIDs: [F.ferb], menuBar: menuBar, display: display))
    }

    // decision: APP-559
    func testNearlyInvisibleWindowsDoNotCount() {
        XCTAssertFalse(covers([F.window(F.clipboardApp, layer: 101, 850, 33, 260, 400, alpha: 0.03)]))
        XCTAssertTrue(covers([F.window(F.clipboardApp, layer: 101, 850, 33, 260, 400, alpha: 0.05)]))
        XCTAssertFalse(NotchOverlap.appWindowUnderChat(in: [F.window(F.browser, layer: 0, 0, 33, 1470, 923, alpha: 0.03)],
                                                       appPID: F.browser, chat: F.openSurface))
    }

    // decision: APP-559
    func testSystemInputProcessesAreRecognisedByBundleIdentifier() {
        for id in ["com.apple.PressAndHold", "com.apple.CharacterPaletteIM", "com.apple.TextInputSwitcher",
                   "com.apple.TextInputUI.xpc.CursorUIViewService", "com.apple.50onPaletteIM",
                   "com.apple.inputmethod.Kotoeri.RomajiTyping", "com.apple.inputmethod.SCIM"] {
            XCTAssertTrue(NotchOverlap.isSystemInput(bundleID: id), id)
        }
        // The input menu's dropdown is an ordinary status menu the owner opened; look-alike ids don't count.
        for id in ["com.apple.TextInputMenuAgent", "com.apple.controlcenter", "com.p0deje.Maccy", "com.apple.inputmethodx"] {
            XCTAssertFalse(NotchOverlap.isSystemInput(bundleID: id), id)
        }
    }

    // decision: APP-559
    func testNothingOfFERBOnScreenNeverYields() {
        XCTAssertFalse(covers([F.clipboardMenu], frames: []))
    }

    // decision: APP-559
    func testAnAppWindowCountsUnderTheChatOnlyWhereTheChatIsDrawn() {
        XCTAssertTrue(NotchOverlap.appWindowUnderChat(in: [F.maximizedBrowser], appPID: F.browser, chat: F.openSurface))
        // The folded header lives in the menu bar, where no ordinary window can be.
        XCTAssertFalse(NotchOverlap.appWindowUnderChat(in: [F.maximizedBrowser], appPID: F.browser, chat: F.headerSurface))
        XCTAssertFalse(NotchOverlap.appWindowUnderChat(in: [F.maximizedBrowser], appPID: F.finder, chat: F.openSurface))
        XCTAssertFalse(NotchOverlap.appWindowUnderChat(in: [F.window(F.browser, layer: 0, 0, 500, 400, 300)],
                                                       appPID: F.browser, chat: F.openSurface))
        XCTAssertFalse(NotchOverlap.appWindowUnderChat(in: [F.window(F.browser, layer: 3, 0, 33, 1470, 923)],
                                                       appPID: F.browser, chat: F.openSurface))
        XCTAssertFalse(NotchOverlap.appWindowUnderChat(in: [F.window(F.browser, layer: 0, 0, 33, 1470, 923, alpha: 0)],
                                                       appPID: F.browser, chat: F.openSurface))
    }

    // decision: APP-559
    func testTheClickedWindowIsTheFrontmostVisibleOneThatIsNotFERBs() {
        let point = CGPoint(x: 700, y: 40)
        let ferbPanel = F.window(F.ferb, layer: 25, 547, 0, 376, 432)
        let invisible = F.window(F.clipboardApp, layer: 26, 0, 0, 1470, 65, alpha: 0)
        let finderWindow = F.window(F.finder, layer: 0, 600, 35, 300, 200)
        XCTAssertEqual(NotchOverlap.owner(at: point, in: [ferbPanel, invisible, finderWindow, F.maximizedBrowser],
                                          ownPIDs: [F.ferb]), F.finder)
        XCTAssertEqual(NotchOverlap.owner(at: point, in: [ferbPanel, F.maximizedBrowser], ownPIDs: [F.ferb]), F.browser)
        XCTAssertNil(NotchOverlap.owner(at: CGPoint(x: 5, y: 5), in: [F.maximizedBrowser], ownPIDs: [F.ferb]))
    }
}

final class NotchYieldTests: XCTestCase {
    // decision: APP-559
    func testVisibilityChangesOnlyWhenTheReasonSetFillsOrEmpties() {
        var yield = NotchYield()
        XCTAssertTrue(yield.begin(.filePicker))
        XCTAssertFalse(yield.begin(.foreignOverlay))
        XCTAssertFalse(yield.begin(.foreignOverlay))
        XCTAssertFalse(yield.end(.filePicker))
        XCTAssertTrue(yield.isActive)
        XCTAssertTrue(yield.end(.foreignOverlay))
        XCTAssertFalse(yield.isActive)
        XCTAssertFalse(yield.end(.foreignOverlay))
    }

    // decision: APP-559
    func testOtherAppsWindowsNeverHideFERBDuringDictation() {
        var yield = NotchYield()
        XCTAssertFalse(yield.begin(.foreignOverlay, listening: true))
        XCTAssertFalse(yield.begin(.activeAppUnderPinnedChat, listening: true))
        XCTAssertFalse(yield.isActive)
        XCTAssertTrue(yield.begin(.filePicker, listening: true))
    }

    // decision: APP-559
    func testStartingDictationDropsOnlyTheWindowReasons() {
        var yield = NotchYield()
        yield.begin(.foreignOverlay)
        yield.begin(.activeAppUnderPinnedChat)
        XCTAssertTrue(yield.endWindowReasons())
        XCTAssertFalse(yield.isActive)
        yield.begin(.filePicker)
        yield.begin(.foreignOverlay)
        XCTAssertFalse(yield.endWindowReasons())
        XCTAssertEqual(yield.reasons, [.filePicker])
    }

    /// APP-644 (live 2026-11-02: with Boring Notch running FERB couldn't be dragged or clicked at all): a visible window
    /// of another app above FERB's handle layer and below menus, over the camera's centre, holds the notch, and FERB
    /// rests beside everything those windows cover. The menu bar, status items, FERB's own windows, menus and
    /// invisible windows never count.
    // decision: APP-644
    func testANotchAppsWindowOverTheCameraHoldsTheNotch() {
        let cutout = CGRect(x: 620, y: 0, width: 200, height: 32)            // window-server coordinates
        func row(_ pid: Int32, _ layer: Int, _ bounds: CGRect, alpha: Double = 1) -> ScreenWindow {
            ScreenWindow(windowID: UInt32(pid), ownerPID: pid, layer: layer, bounds: bounds, alpha: alpha)
        }
        let notchApp = row(77, 27, CGRect(x: 520, y: 0, width: 400, height: 40))
        XCTAssertEqual(NotchOverlap.notchHolder(in: [notchApp], cutout: cutout, ownPIDs: [1], ferbLayer: 25),
                       notchApp.bounds)
        let others = [row(10, 24, CGRect(x: 0, y: 0, width: 1440, height: 32)),         // the menu bar
                      row(11, 25, CGRect(x: 640, y: 0, width: 30, height: 32)),         // a status item
                      row(1, 27, CGRect(x: 600, y: 0, width: 240, height: 40)),         // FERB's own
                      row(12, NotchOverlap.popUpMenuLayer, CGRect(x: 600, y: 0, width: 300, height: 300)),
                      row(13, 27, CGRect(x: 600, y: 0, width: 240, height: 40), alpha: 0),
                      row(14, 27, CGRect(x: 1000, y: 0, width: 200, height: 40))]       // not over the camera
        XCTAssertNil(NotchOverlap.notchHolder(in: others, cutout: cutout, ownPIDs: [1], ferbLayer: 25))
    }
}
