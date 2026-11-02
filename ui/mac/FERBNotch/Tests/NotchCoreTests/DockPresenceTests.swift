import XCTest
@testable import FERBNotch

@MainActor
final class DockPresenceTests: XCTestCase {
    typealias W = DockPresence.DockWindow

    // decision: APP-600
    func testFERBIsInTheDockOnlyWhileOneOfItsWindowsIsOpen() {
        let notch = W(isPanel: true, isTitled: false, isVisible: true, isMiniaturized: false)
        let setup = W(isPanel: false, isTitled: true, isVisible: true, isMiniaturized: false)
        let closed = W(isPanel: false, isTitled: true, isVisible: false, isMiniaturized: false)
        let minimized = W(isPanel: false, isTitled: true, isVisible: false, isMiniaturized: true)
        let titledPanel = W(isPanel: true, isTitled: true, isVisible: true, isMiniaturized: false)
        XCTAssertFalse(DockPresence.wantsDock([]))
        XCTAssertFalse(DockPresence.wantsDock([notch, titledPanel, closed]))   // the notch and panels never count
        XCTAssertTrue(DockPresence.wantsDock([notch, setup]))
        XCTAssertTrue(DockPresence.wantsDock([notch, minimized]))              // a minimized window keeps FERB there
    }
}
