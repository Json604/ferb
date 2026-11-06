import AppKit
import XCTest
@testable import FERBNotch
import NotchCore

/// Owner, 2026-11-06: "the notch open close on space change is fully fixed - ive testes. BUT the open close haoppens on
/// the window on space switch have ot fix that too". Window mode driven through the real PanelCoordinator with the real
/// Space-change notification (in this process only), the sticky window in FERB's real overlay space, and other apps'
/// windows stood in as rows (never the owner's).
@MainActor
final class WindowSpaceTests: XCTestCase {
    private struct Harness {
        let defaults: UserDefaults
        let model: NotchModel
        let coordinator: PanelCoordinator
        let window: ChatWindowController
    }

    private var rows: [ScreenWindow] = []

    private func harness(keepOnTop: Bool = false) throws -> Harness {
        _ = NSApplication.shared
        guard NSScreen.screens.first != nil else { throw XCTSkip("Needs a display") }
        // Clicks and keyboard focus need an unlocked session (a locked screen keeps every window from becoming key)
        if (CGSessionCopyCurrentDictionary() as? [String: Any])?["CGSSessionScreenIsLocked"] as? Bool == true {
            throw XCTSkip("The screen is locked")
        }
        let defaults = scratchDefaults()
        NotchDocking.save(true, in: defaults)
        defaults.set(ChatHost.window.rawValue, forKey: ChatHost.defaultsKey)
        defaults.set(keepOnTop, forKey: ChatHost.keepOnTopKey)
        let model = NotchModel(rpc: ScriptedRPC(), preferences: defaults)
        let coordinator = PanelCoordinator(model: model, defaults: defaults, overlaySpace: NotchOverlaySpace())
        coordinator.activeSpace = { 1 }
        coordinator.missionControlShowing = { _ in false }
        coordinator.hoverWait = { _ in }
        coordinator.restWait = { _ in }
        coordinator.updateScreen()
        coordinator.attachOverlay()
        coordinator.observeWorkspace()
        addTeardownBlock { coordinator.tearDown() }
        coordinator.toggleByShortcut()
        let window = try XCTUnwrap(coordinator.chatWindow)
        guard window.space?.isAttached == true else { throw XCTSkip("FERB's overlay space isn't available here") }
        window.windowList = { [unowned self] in self.rows }
        window.onAnotherSpace = { _ in false }
        return Harness(defaults: defaults, model: model, coordinator: coordinator, window: window)
    }

    private func place(_ h: Harness) -> (member: Bool, ordinary: [UInt64])? {
        h.coordinator.overlayMembership(of: h.window.panel)
    }

    private func assertFixed(_ h: Harness, _ message: String, file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertFalse(h.window.steppedBack, message, file: file, line: line)
        XCTAssertEqual(place(h)?.member, true, "\(message): not in the fixed space", file: file, line: line)
        XCTAssertEqual(place(h)?.ordinary, [], "\(message): on an ordinary Space (rides the swipe)", file: file, line: line)
    }

    private func assertSteppedBack(_ h: Harness, _ message: String, file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertTrue(h.window.steppedBack, message, file: file, line: line)
        XCTAssertEqual(place(h)?.member, false, "\(message): still in the fixed space", file: file, line: line)
        XCTAssertEqual(h.window.panel.level, .normal, message, file: file, line: line)
        XCTAssertTrue(h.window.isShown, "\(message): left the screen", file: file, line: line)
    }

    private func switchSpaces(_ h: Harness, times: Int = 3) {
        for space in 0..<times {
            h.coordinator.activeSpace = { UInt64(10 + space) }
            NSWorkspace.shared.notificationCenter.post(name: NSWorkspace.activeSpaceDidChangeNotification, object: nil)
            NotificationCenter.default.post(name: NSApplication.didChangeScreenParametersNotification, object: nil)
        }
    }

    private var frame: (Harness) -> CGRect = { h in
        NotchOverlap.windowServerRect(h.window.panel.frame, primaryDisplayHeight: NSScreen.screens.first?.frame.maxY ?? 0)
    }

    /// The sticky window is in FERB's fixed space, on no ordinary Space, and stays exactly where it is through
    /// repeated Space switches, with Keep on top off (the owner's setting) and on.
    // decision: APP-760, APP-745
    func testTheWindowStaysPutThroughSpaceSwitches() throws {
        for keepOnTop in [false, true] {
            let h = try harness(keepOnTop: keepOnTop)
            assertFixed(h, "opened, keep on top \(keepOnTop)")
            let before = (h.window.panel.frame, h.window.panel.isKeyWindow, h.window.isShown)
            switchSpaces(h)
            assertFixed(h, "after the switches, keep on top \(keepOnTop)")
            XCTAssertEqual(h.window.panel.frame, before.0, "the window moved")
            XCTAssertEqual(h.window.panel.isKeyWindow, before.1)
            XCTAssertTrue(h.window.isShown)
            h.coordinator.tearDown()
        }
    }

    /// FERB's own windows, its file picker and what it opens in another app show above the window; it comes back after.
    // decision: APP-760, APP-749
    func testFERBsOwnWindowsThePickerAndAHandOffShowAboveIt() throws {
        let h = try harness()
        let settings = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 300, height: 200),
                                styleMask: [.titled, .closable], backing: .buffered, defer: false)
        settings.isReleasedWhenClosed = false
        addTeardownBlock { settings.orderOut(nil) }
        settings.orderFrontRegardless()
        h.window.ownWindowsChanged()
        RunLoop.main.run(until: Date().addingTimeInterval(0.05))
        assertSteppedBack(h, "Settings shows")
        settings.orderOut(nil)
        h.window.ownWindowsChanged()
        RunLoop.main.run(until: Date().addingTimeInterval(0.05))
        assertFixed(h, "Settings closed")
        h.model.setFilePickerOpen(true)
        assertSteppedBack(h, "the picker shows")
        h.model.setFilePickerOpen(false)
        assertFixed(h, "the picker closed")
        HandOff.announce()
        assertSteppedBack(h, "a file shown in Finder")
        h.window.show(focus: true)                         // the owner comes back to the window
        assertFixed(h, "the owner came back")
    }

    /// Another app's menu or popover over the window: it steps one layer back so the menu shows, and comes back when
    /// the menu closes. One over it when the owner opened it (a persistent floating window) is the owner's choice.
    // decision: APP-760, APP-559
    func testAnotherAppsMenuOverItShowsAboveAndTheWindowComesBack() throws {
        let h = try harness()
        let menu = ScreenWindow(windowID: 9001, ownerPID: 4242, layer: NotchOverlap.popUpMenuLayer,
                                bounds: frame(h).insetBy(dx: 40, dy: 40), alpha: 1)
        h.window.evaluate()                                 // the look after opening finds nothing over it
        rows = [menu]
        h.window.evaluate()
        assertSteppedBack(h, "a menu over it")
        rows = []
        h.window.evaluate()
        assertFixed(h, "the menu closed")
        rows = [menu]                                       // over it when the owner opens it: their choice
        h.window.show(focus: true)
        h.window.evaluate()
        assertFixed(h, "a window the owner opened FERB over")
        // a Space switch never makes a window there count
        switchSpaces(h)
        assertFixed(h, "after the switches")
    }

    /// Keep on top off means another app's window the owner brings forward over the sticky window is in front of it;
    /// the new Space's own front app (a Space switch) isn't. With Keep on top on, nothing of the kind steps it back.
    // decision: APP-760, APP-745
    func testWithKeepOnTopOffAnAppBroughtForwardOverItIsInFront() throws {
        let h = try harness(keepOnTop: false)
        let editor = ScreenWindow(windowID: 7001, ownerPID: 5151, layer: 0, bounds: frame(h).offsetBy(dx: 60, dy: 60),
                                  alpha: 1)
        rows = [editor]
        h.window.otherAppActivated(5151, bySpaceChange: true)
        assertFixed(h, "the new Space's front app")
        var clock = Date()
        h.window.now = { clock }
        switchSpaces(h, times: 1)                          // macOS may post the activation just after the change
        h.window.otherAppActivated(5151, bySpaceChange: false)
        assertFixed(h, "the new Space's front app, activated after the Space change")
        clock = clock.addingTimeInterval(ChatWindowController.spaceChangeGrace + 0.1)
        h.window.otherAppActivated(5151, bySpaceChange: false)
        assertSteppedBack(h, "the owner brought the editor forward")
        XCTAssertEqual(h.window.coveredBy, 5151)
        rows = []                                           // its window moved away or closed
        h.window.evaluate()
        assertFixed(h, "nothing covers it any more")
        rows = [editor]
        h.window.ownerClicked(at: CGPoint(x: editor.bounds.maxX - 5, y: editor.bounds.maxY - 5))
        assertSteppedBack(h, "a click into the editor already in front")
        h.window.show(focus: true)
        assertFixed(h, "the owner opened FERB again")
        h.defaults.set(true, forKey: ChatHost.keepOnTopKey)
        h.coordinator.applyChatHost()
        h.window.otherAppActivated(5151, bySpaceChange: false)
        assertFixed(h, "keep on top on")
    }

    /// A file drag reaches the window: it leaves the fixed space while the drag lasts (no drag reaches a window there,
    /// APP-742) and comes back after.
    // decision: APP-760, APP-742
    func testAFileDragReachesTheWindowAndItComesBackAfter() throws {
        let h = try harness()
        h.coordinator.beginFileDrag()
        XCTAssertEqual(place(h)?.member, false, "held out of the fixed space for the drag")
        h.coordinator.endFileDrag()
        assertFixed(h, "the drag ended")
    }

    /// The profile menu (APP-759) opens on one click with the window in the fixed space and another window focused.
    // decision: APP-760, APP-759
    func testTheProfileMenuOpensOnOneClickInTheFixedSpace() throws {
        let h = try harness()
        RunLoop.main.run(until: Date().addingTimeInterval(0.3))
        func views(_ root: NSView) -> [NSView] { [root] + root.subviews.flatMap(views) }
        let button = try XCTUnwrap(views(try XCTUnwrap(h.window.panel.contentView))
            .compactMap { $0 as? NSButton }.first { $0.toolTip == "Profile and options" })
        var opened = 0
        let present = ProfileMenuButton.present
        ProfileMenuButton.present = { _, _ in opened += 1 }
        defer { ProfileMenuButton.present = present }
        let point = button.convert(NSPoint(x: 13, y: 13), to: nil)
        for (type, number) in [(NSEvent.EventType.leftMouseDown, 1), (.leftMouseUp, 2)] {
            NSApp.postEvent(try XCTUnwrap(NSEvent.mouseEvent(with: type, location: point, modifierFlags: [],
                timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: h.window.panel.windowNumber,
                context: nil, eventNumber: number, clickCount: 1, pressure: 1)), atStart: false)
        }
        let end = Date().addingTimeInterval(0.4)
        while Date() < end, let event = NSApp.nextEvent(matching: .any, until: Date().addingTimeInterval(0.02),
                                                         inMode: .default, dequeue: true) { NSApp.sendEvent(event) }
        XCTAssertEqual(opened, 1)
        assertFixed(h, "after the menu")
    }
}
