import AppKit
import SwiftUI
import XCTest
@testable import FERBNotch
import NotchCore

/// Window mode's header (owner, 2026-11-06: "cant open settings by clicking on icon, it flcikers sometime adn many times
/// dont even takes the click"): every header control takes a single click, also while another window has keyboard
/// focus, and no pin shows. Clicks go through AppKit's own event dispatch (posted to the
/// app's queue and sent by NSApp, as a real click is), never through the owner's input.
@MainActor
final class WindowHeaderClickTests: XCTestCase {
    private func coordinator(window: Bool) throws -> PanelCoordinator {
        _ = NSApplication.shared
        guard NSScreen.screens.first != nil else { throw XCTSkip("Needs a display") }
        // Clicks and keyboard focus need an unlocked session (a locked screen keeps every window from becoming key)
        if (CGSessionCopyCurrentDictionary() as? [String: Any])?["CGSSessionScreenIsLocked"] as? Bool == true {
            throw XCTSkip("The screen is locked")
        }
        let defaults = scratchDefaults()
        NotchDocking.save(true, in: defaults)
        if window { defaults.set(ChatHost.window.rawValue, forKey: ChatHost.defaultsKey) }
        let model = NotchModel(rpc: ScriptedRPC(), preferences: defaults)
        let coordinator = PanelCoordinator(model: model, defaults: defaults)
        coordinator.updateScreen()
        coordinator.hoverWait = { _ in }
        addTeardownBlock { coordinator.tearDown() }
        return coordinator
    }

    private func openWindow() throws -> (PanelCoordinator, ChatWindowController) {
        let coordinator = try coordinator(window: true)
        coordinator.toggleByShortcut()
        let window = try XCTUnwrap(coordinator.chatWindow)
        window.panel.contentView?.layoutSubtreeIfNeeded()
        RunLoop.main.run(until: Date().addingTimeInterval(0.3))        // SwiftUI draws the header once
        return (coordinator, window)
    }

    /// Another window takes keyboard focus (not a titled one, so the sticky window doesn't step down for it).
    private func focusElsewhere() -> NSPanel {
        final class Other: NSPanel { override var canBecomeKey: Bool { true } }
        let other = Other(contentRect: NSRect(x: 0, y: 0, width: 40, height: 40), styleMask: [.borderless],
                          backing: .buffered, defer: false)
        addTeardownBlock { other.orderOut(nil) }
        other.orderFrontRegardless()
        other.makeKey()
        return other
    }

    private func click(_ view: NSView, at local: NSPoint) throws {
        let window = try XCTUnwrap(view.window)
        let point = view.convert(local, to: nil)
        for (type, number) in [(NSEvent.EventType.leftMouseDown, 1), (.leftMouseUp, 2)] {
            let event = try XCTUnwrap(NSEvent.mouseEvent(with: type, location: point, modifierFlags: [],
                                                         timestamp: ProcessInfo.processInfo.systemUptime,
                                                         windowNumber: window.windowNumber, context: nil,
                                                         eventNumber: number, clickCount: 1,
                                                         pressure: type == .leftMouseDown ? 1 : 0))
            NSApp.postEvent(event, atStart: false)
        }
        let end = Date().addingTimeInterval(0.4)
        while Date() < end, let event = NSApp.nextEvent(matching: .any, until: Date().addingTimeInterval(0.02),
                                                         inMode: .default, dequeue: true) {
            NSApp.sendEvent(event)
        }
    }

    private func views<T: NSView>(_ type: T.Type, in root: NSView) -> [T] {
        ((root as? T).map { [$0] } ?? []) + root.subviews.flatMap { views(type, in: $0) }
    }

    // decision: APP-759, APP-556
    func testTheProfileButtonOpensItsMenuOnOneClickWithoutTheWindowMoving() throws {
        let (_, window) = try openWindow()
        let content = try XCTUnwrap(window.panel.contentView)
        let button = try XCTUnwrap(views(NSButton.self, in: content).first { $0.toolTip == "Profile and options" })
        var menus: [NSMenu] = []
        let present = ProfileMenuButton.present
        ProfileMenuButton.present = { menu, _ in menus.append(menu) }
        defer { ProfileMenuButton.present = present }
        _ = focusElsewhere()
        XCTAssertFalse(window.panel.isKeyWindow)
        let level = window.panel.level, frame = window.panel.frame
        try click(button, at: NSPoint(x: 13, y: 13))
        XCTAssertEqual(menus.count, 1, "one click opens the menu")
        XCTAssertTrue(menus.first?.items.contains { $0.title == "Settings…" } == true)
        XCTAssertTrue(window.panel.isKeyWindow, "the click made the window key without activating FERB")
        XCTAssertEqual(window.panel.level, level, "nothing stepped the window down or up")
        XCTAssertEqual(window.panel.frame, frame, "the press was never taken as a window drag")
        XCTAssertTrue(window.isShown)
        try click(button, at: NSPoint(x: 6, y: 20))                 // anywhere on it, again
        XCTAssertEqual(menus.count, 2)
    }

    // decision: APP-759, APP-556
    func testTheNotchsProfileMenuStillOpensOnOneClick() throws {
        let coordinator = try coordinator(window: false)
        coordinator.toggleByShortcut()
        let panel = try XCTUnwrap(NSApp.windows.first { $0.delegate === coordinator } as? NotchPanel)
        RunLoop.main.run(until: Date().addingTimeInterval(0.5))
        let content = try XCTUnwrap(panel.contentView)
        let button = try XCTUnwrap(views(NSButton.self, in: content).first { $0.toolTip == "Profile and options" })
        var opened = 0
        let present = ProfileMenuButton.present
        ProfileMenuButton.present = { _, _ in opened += 1 }
        defer { ProfileMenuButton.present = present }
        try click(button, at: NSPoint(x: 13, y: 13))
        XCTAssertEqual(opened, 1)
        XCTAssertTrue(coordinator.model.expanded, "the notch stays open under its menu")
    }

    // decision: APP-759, APP-758
    func testCloseTakesOneClickWhileAnotherWindowHasFocus() throws {
        let (coordinator, window) = try openWindow()
        let content = try XCTUnwrap(window.panel.contentView)
        _ = focusElsewhere()
        // the close button: the header's right end, centred in its 38-point height (the hosting view is flipped)
        let header = ChatWindowView.headerHeight
        let y = content.isFlipped ? header / 2 : content.bounds.maxY - header / 2
        try click(content, at: NSPoint(x: content.bounds.maxX - 18 - 12, y: y))
        XCTAssertFalse(window.isShown, "one click closed the window")
        XCTAssertFalse(coordinator.model.expanded)
    }

    // decision: APP-759, APP-745
    func testTheWindowServerNeverTakesAHeaderPressAndTheHeaderStillMovesTheWindow() throws {
        let (_, window) = try openWindow()
        XCTAssertFalse(window.panel.isMovable, "no window-server drag region under the header's controls")
        let content = try XCTUnwrap(window.panel.contentView)
        let drag = try XCTUnwrap(views(WindowDragArea.DragView.self, in: content).first)
        XCTAssertFalse(drag.mouseDownCanMoveWindow)
        var pointer = NSPoint(x: 500, y: 500)
        drag.pointer = { pointer }
        let start = window.panel.frame.origin
        let event = try XCTUnwrap(NSEvent.mouseEvent(with: .leftMouseDown, location: .zero, modifierFlags: [],
                                                     timestamp: 0, windowNumber: window.panel.windowNumber, context: nil,
                                                     eventNumber: 0, clickCount: 1, pressure: 1))
        drag.mouseDown(with: event)
        pointer = NSPoint(x: 460, y: 470)
        drag.mouseDragged(with: event)
        XCTAssertEqual(window.panel.frame.origin, NSPoint(x: start.x - 40, y: start.y - 30))
        drag.mouseUp(with: event)
        pointer = NSPoint(x: 0, y: 0)
        drag.mouseDragged(with: event)                              // after release nothing follows the pointer
        XCTAssertEqual(window.panel.frame.origin, NSPoint(x: start.x - 40, y: start.y - 30))
    }
}

/// The detached task activity window (APP-782, owner 2026-11-07: "im not able to drag the preview window when extracted
/// form te h chat - it shoudl be draggable, it was"): FERB's overlay space moves no window by the window server, so its
/// card drags it itself.
@MainActor
final class ActivityWindowDragTests: XCTestCase {
    // decision: APP-782
    func testTheDetachedActivityWindowFollowsADragOfItsCard() throws {
        let model = NotchModel(preferences: scratchDefaults())
        model.runningTasks["t-1"] = "Swiggy tailored resumes"
        let taskID = try XCTUnwrap(model.taskItems.first { $0.isRunning }?.id)
        let panel = TaskActivityPanel(model: model, taskID: taskID) {}
        XCTAssertFalse(panel.isMovable, "no window-server drag: it can't move windows in FERB's overlay space")
        let content = try XCTUnwrap(panel.contentView)
        content.layoutSubtreeIfNeeded()
        func drags(_ view: NSView) -> [WindowDragArea.DragView] {
            (view as? WindowDragArea.DragView).map { [$0] } ?? view.subviews.flatMap(drags)
        }
        let drag = try XCTUnwrap(drags(content).first, "the card has a drag area")
        var pointer = NSPoint(x: 300, y: 300)
        drag.pointer = { pointer }
        let start = panel.frame.origin
        let event = try XCTUnwrap(NSEvent.mouseEvent(with: .leftMouseDown, location: .zero, modifierFlags: [],
                                                     timestamp: 0, windowNumber: panel.windowNumber, context: nil,
                                                     eventNumber: 0, clickCount: 1, pressure: 1))
        drag.mouseDown(with: event)
        pointer = NSPoint(x: 350, y: 280)
        drag.mouseDragged(with: event)
        XCTAssertEqual(panel.frame.origin, NSPoint(x: start.x + 50, y: start.y - 20))
        drag.mouseUp(with: event)
    }
}
