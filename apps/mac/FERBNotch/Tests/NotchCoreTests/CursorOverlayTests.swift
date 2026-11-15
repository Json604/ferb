import XCTest
@testable import FERBNotch
@testable import NotchCore

@MainActor
final class CursorOverlayTests: XCTestCase {
    // decision: APP-588
    func testTheOverlayPanelNeverTakesTheMouseOrFocus() {
        let panel = CursorPanel(screenFrame: CGRect(x: 0, y: 0, width: 800, height: 600))
        XCTAssertTrue(panel.ignoresMouseEvents)
        XCTAssertFalse(panel.canBecomeKey)
        XCTAssertFalse(panel.canBecomeMain)
        XCTAssertTrue(panel.collectionBehavior.contains(.canJoinAllSpaces))
        XCTAssertFalse(panel.hidesOnDeactivate)
        XCTAssertEqual(panel.level.rawValue, NSWindow.Level.statusBar.rawValue - 1)
        XCTAssertTrue(panel.collectionBehavior.isSuperset(of: [.fullScreenAuxiliary, .stationary, .ignoresCycle]))
    }

    /// D31 fix round 4 (U1): the overlay's click-through is AppKit's own property, read through NSWindow's own getter,
    /// so the window server is told and a click on the drawn arrow passes through; setting false can't turn it off;
    /// and Accessibility's hit test passes through it too.
    // decision: APP-588
    func testTheOverlayIsClickThroughByAppKitsOwnPropertyAndHiddenFromAccessibility() {
        let panel = CursorPanel(screenFrame: CGRect(x: 0, y: 0, width: 800, height: 600))
        XCTAssertTrue(appKitIgnoresMouseEvents(panel))
        panel.ignoresMouseEvents = false
        XCTAssertTrue(appKitIgnoresMouseEvents(panel))
        XCTAssertFalse(panel.isAccessibilityElement())
        XCTAssertTrue(panel.isAccessibilityHidden())
    }

    // decision: APP-588
    func testTheNotchGlowIsClickThroughTheSameWay() {
        let panel = NotchLightPanel(contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel],
                                    backing: .buffered, defer: false)
        XCTAssertTrue(appKitIgnoresMouseEvents(panel))
        panel.ignoresMouseEvents = false
        XCTAssertTrue(appKitIgnoresMouseEvents(panel))
        XCTAssertFalse(panel.isAccessibilityElement())
        XCTAssertTrue(panel.isAccessibilityHidden())
        XCTAssertFalse(panel.canBecomeKey)
    }

    private func appKitIgnoresMouseEvents(_ window: NSWindow) -> Bool {
        let selector = #selector(getter: NSWindow.ignoresMouseEvents)
        typealias Getter = @convention(c) (AnyObject, Selector) -> Bool
        guard let imp = class_getMethodImplementation(NSWindow.self, selector) else { return false }
        return unsafeBitCast(imp, to: Getter.self)(window, selector)
    }
}
