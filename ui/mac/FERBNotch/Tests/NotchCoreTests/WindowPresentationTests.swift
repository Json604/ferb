import AppKit
import XCTest
import Darwin
@testable import FERBNotch
import NotchCore

@MainActor
final class WindowPresentationTests: XCTestCase {
    // decision: APP-630
    func testRequestedPageReopensClosedAndMinimizedWindow() async throws {
        _ = NSApplication.shared
        let controller = FERBWindowController(store: FERBWindowStore(rpc: ScriptedRPC()),
            actions: FERBAppActions(feedback: SoundFeedback(), cursor: CursorController(makePanels: false)))
        controller.show(.tasks)
        let window = try XCTUnwrap(controller.window)
        defer { window.orderOut(nil) }
        window.close()
        controller.show(.memory)
        XCTAssertTrue(window.isVisible)
        XCTAssertEqual(controller.selection.section, .memory)
        window.miniaturize(nil)
        try await Task.sleep(for: .milliseconds(500))
        XCTAssertTrue(window.isMiniaturized)
        controller.show(.schedules)
        try await Task.sleep(for: .milliseconds(500))
        XCTAssertFalse(window.isMiniaturized)
        XCTAssertTrue(window.isVisible)
        XCTAssertEqual(controller.selection.section, .schedules)
    }

    /// Owner, 2026-11-04: Command-Q in Settings quit FERB, notch included. Command-Q closes FERB's window when it's in
    /// front; with another window in front it quits as before. The window opens on the owner's Space without switching
    /// Spaces, and pointer moves reach the notch while it is in front.
    // decision: APP-654, APP-688, APP-656
    func testQuitKeyClosesOnlyTheFERBWindowInFront() throws {
        _ = NSApplication.shared
        let controller = FERBWindowController(store: FERBWindowStore(rpc: ScriptedRPC()),
            actions: FERBAppActions(feedback: SoundFeedback(), cursor: CursorController(makePanels: false)))
        controller.show(.settings)
        let window = try XCTUnwrap(controller.window)
        defer { window.orderOut(nil) }
        XCTAssertTrue(window.collectionBehavior.contains(.moveToActiveSpace))
        XCTAssertTrue(window.acceptsMouseMovedEvents)
        XCTAssertFalse(controller.closeOnQuitKey(keyWindow: nil))
        XCTAssertTrue(window.isVisible)
        XCTAssertTrue(controller.closeOnQuitKey(keyWindow: window))
        XCTAssertFalse(window.isVisible)
    }

    // decision: APP-630
    func testRequestedWindowRecoversAnOffscreenFrame() throws {
        _ = NSApplication.shared
        guard !NSScreen.screens.isEmpty else { throw XCTSkip("Needs a display") }
        let controller = FERBWindowController(store: FERBWindowStore(rpc: ScriptedRPC()),
            actions: FERBAppActions(feedback: SoundFeedback(), cursor: CursorController(makePanels: false)))
        controller.show(.tasks)
        let window = try XCTUnwrap(controller.window)
        defer { window.orderOut(nil) }
        window.setFrameOrigin(NSPoint(x: -20000, y: -20000))
        controller.show(.settings)
        XCTAssertTrue(NSScreen.screens.contains { $0.visibleFrame.intersects(window.frame) })
    }

    // decision: APP-630
    func testNativeWindowWithoutActiveSpaceCanBePresented() async throws {
        _ = NSApplication.shared
        typealias Main = @convention(c) () -> Int32
        typealias Active = @convention(c) (Int32) -> UInt64
        guard let library = dlopen("/System/Library/PrivateFrameworks/SkyLight.framework/Versions/A/SkyLight", RTLD_NOW) else {
            throw XCTSkip("SkyLight unavailable")
        }
        defer { dlclose(library) }
        guard let main = dlsym(library, "SLSMainConnectionID"), let active = dlsym(library, "SLSGetActiveSpace") else {
            throw XCTSkip("Space APIs unavailable")
        }
        let connection = unsafeBitCast(main, to: Main.self)()
        let activeSpace = unsafeBitCast(active, to: Active.self)
        let window = NSWindow(contentRect: NSRect(x: 200, y: 200, width: 500, height: 350),
                              styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
        defer { window.orderOut(nil) }
        NSApp.setActivationPolicy(.regular)
        window.makeKeyAndOrderFront(nil)
        try await Task.sleep(for: .milliseconds(150))
        // This macOS behavior needs a foreign full-screen Space; don't pretend a normal desktop reproduces it.
        guard window.isVisible, !window.isOnActiveSpace else {
            throw XCTSkip("AppKit placed this window normally; full-screen reproduction is not present")
        }
        let before = activeSpace(connection)
        AppWindowPresentation.show(window)
        try await Task.sleep(for: .milliseconds(150))
        XCTAssertTrue(window.isOnActiveSpace)
        XCTAssertEqual(activeSpace(connection), before)
        XCTAssertEqual(window.level, .normal)
    }
}

/// Owner, FERB 0.27.3, 2026-11-07 (Settings → AI, Other ways to connect): "nothign happens on clicking it, is there onyl
/// anthropic option???". Each click on the Provider picker opened its menu, but the window server put every one on
/// Space 3 while the owner and FERB's window were on Space 2896 (log: "window 11bdb joined space 3"). A menu opened from
/// a window FERB brought to the owner's Space now comes to the Space showing, for every pop-up, menu and context menu of
/// that window. No window is put on screen here: the windows, the menus' Spaces and the move are stand-ins.
@MainActor
final class MenuSpaceTests: XCTestCase {
    private var moved: [NSWindow] = []
    private var here: Set<ObjectIdentifier> = []
    private var menus: [NSWindow] = []
    private var origin: NSWindow?
    private var clock = Date(timeIntervalSinceReferenceDate: 0)

    override func setUp() async throws {
        _ = NSApplication.shared
        let saved = (AppWindowPresentation.menuWindows, AppWindowPresentation.menuOrigin,
                     AppWindowPresentation.menuIsHere, AppWindowPresentation.bringMenuHere, AppWindowPresentation.now)
        AppWindowPresentation.menuWindows = { [unowned self] in self.menus }
        AppWindowPresentation.menuOrigin = { [unowned self] in self.origin }
        AppWindowPresentation.menuIsHere = { [unowned self] in self.here.contains(ObjectIdentifier($0)) }
        AppWindowPresentation.bringMenuHere = { [unowned self] window in
            self.moved.append(window)
            self.here.insert(ObjectIdentifier(window))
        }
        AppWindowPresentation.now = { [unowned self] in self.clock }
        addTeardownBlock { @MainActor in
            (AppWindowPresentation.menuWindows, AppWindowPresentation.menuOrigin, AppWindowPresentation.menuIsHere,
             AppWindowPresentation.bringMenuHere, AppWindowPresentation.now) = saved
        }
    }

    private func window(level: NSWindow.Level = .normal) -> NSWindow {
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 300, height: 200), styleMask: [.titled],
                              backing: .buffered, defer: true)
        window.isReleasedWhenClosed = false
        window.level = level
        return window
    }

    private func post(_ name: Notification.Name, _ menu: NSMenu) {
        NotificationCenter.default.post(name: name, object: menu)
    }

    // decision: APP-790
    func testAMenuOpenedFromFERBsWindowOnAnotherSpaceComesToTheSpaceShowing() {
        let settings = window()
        AppWindowPresentation.adopt(settings)               // what showing FERB's window does (FERBWindowController)
        here.insert(ObjectIdentifier(settings))
        let list = window(level: .popUpMenu)               // the picker's list, ordered onto another Space
        menus = [list]
        origin = settings
        let picker = NSMenu()
        post(NSMenu.didBeginTrackingNotification, picker)
        XCTAssertEqual(moved, [list], "the open menu came to the Space the owner and the window are on")
        XCTAssertTrue(AppWindowPresentation.watchingMenus, "it is kept there while the menu is open")
        post(NSMenu.didEndTrackingNotification, picker)
        XCTAssertFalse(AppWindowPresentation.watchingMenus)

        // A menu already on the Space showing is left alone.
        moved = []
        post(NSMenu.didBeginTrackingNotification, picker)
        XCTAssertEqual(moved, [])
        post(NSMenu.didEndTrackingNotification, picker)
    }

    // decision: APP-790
    func testAPressInFERBsWindowBringsTheMenuItOpensEvenWithoutMenuTracking() {
        let settings = window()
        AppWindowPresentation.adopt(settings)
        AppWindowPresentation.pressed(in: settings)         // the picker opens its menu on the press
        XCTAssertTrue(AppWindowPresentation.watchingMenus)
        let list = window(level: .popUpMenu)
        menus = [list]
        AppWindowPresentation.menuWatchTick()
        XCTAssertEqual(moved, [list])
        clock = clock.addingTimeInterval(AppWindowPresentation.pressWatch)
        menus = []
        AppWindowPresentation.menuWatchTick()
        XCTAssertFalse(AppWindowPresentation.watchingMenus, "the watch ends once the press's moment is over")
    }

    // decision: APP-790
    func testTheNotchsOwnMenusStayWhereMacOSPutsThem() {
        let notch = window(level: NSWindow.Level(rawValue: NSWindow.Level.mainMenu.rawValue + 1))   // never shown here
        let profile = window(level: .popUpMenu)
        menus = [profile]
        origin = notch
        let menu = NSMenu()
        post(NSMenu.didBeginTrackingNotification, menu)
        XCTAssertEqual(moved, [], "the notch's menus belong to its overlay space")
        XCTAssertFalse(AppWindowPresentation.watchingMenus)
        AppWindowPresentation.pressed(in: notch)
        XCTAssertFalse(AppWindowPresentation.watchingMenus)
        post(NSMenu.didEndTrackingNotification, menu)

        // A press in FERB's window, then the notch's menu opens: the press's watch ends rather than moving that menu.
        let settings = window()
        AppWindowPresentation.adopt(settings)
        menus = []
        AppWindowPresentation.pressed(in: settings)
        XCTAssertTrue(AppWindowPresentation.watchingMenus)
        menus = [profile]
        post(NSMenu.didBeginTrackingNotification, menu)
        XCTAssertEqual(moved, [])
        XCTAssertFalse(AppWindowPresentation.watchingMenus)
        post(NSMenu.didEndTrackingNotification, menu)
    }
}
