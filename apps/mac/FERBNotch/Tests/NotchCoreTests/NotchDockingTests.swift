import XCTest
import SwiftUI
@testable import FERBNotch

final class NotchDockingTests: XCTestCase {
    let notch = CGRect(x: 600, y: 870, width: 180, height: 30)
}

final class PillShoulderTests: XCTestCase {
}

extension NotchDockingTests {
    // decision: APP-553
    func testHoverIncludesExactPhysicalTopBoundary() {
        XCTAssertTrue(NotchDocking.accepts(CGPoint(x: notch.midX, y: notch.maxY), cutout: notch, hasHardwareNotch: true))
    }
}
