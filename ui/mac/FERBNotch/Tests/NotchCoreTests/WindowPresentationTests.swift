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
