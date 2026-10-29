import XCTest
@testable import FERBNotch

final class IndicatorPlacementTests: XCTestCase {
    private let screen = CGRect(x: 0, y: 0, width: 1470, height: 956)
    private let notch = CGRect(x: 635, y: 926, width: 200, height: 30)

    func testTopIndicatorDroppedUnderNotchMovesToNearestSide() {
        let x = IndicatorPlacement.safeTopX(proposedX: 700, size: 58, screen: screen, notch: notch)

        XCTAssertEqual(x, 577)
        XCTAssertLessThanOrEqual(x + 58, notch.minX)
    }

    func testTopIndicatorCanRemainAnywhereOutsideNotch() {
        let x = IndicatorPlacement.safeTopX(proposedX: 900, size: 58, screen: screen, notch: notch)

        XCTAssertEqual(x, 900)
    }
}
