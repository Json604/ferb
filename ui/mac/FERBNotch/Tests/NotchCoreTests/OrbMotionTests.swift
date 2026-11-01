import XCTest
import CoreGraphics
@testable import NotchCore

final class OrbMotionTests: XCTestCase {
    // decision: APP-504
    func testRetargetMidFlightKeepsVelocity() {
        var orb = OrbMotion(origin: CGPoint(x: 100, y: 100))
        orb.retarget(CGPoint(x: 600, y: 300), spec: .open)
        for _ in 0..<10 { _ = orb.step(dt: 1.0 / 60) }
        let before = orb.velocity
        XCTAssertGreaterThan(abs(before.dx), 1)
        orb.retarget(CGPoint(x: 200, y: 700), spec: .open)
        XCTAssertEqual(orb.velocity.dx, before.dx, accuracy: 1e-9)
        XCTAssertEqual(orb.velocity.dy, before.dy, accuracy: 1e-9)
    }

    // decision: APP-504
    func testCloseSpringNeverOvershoots() {
        for dt in [1.0 / 60, 1.0 / 120] {
            var orb = OrbMotion(origin: CGPoint(x: 0, y: 0))
            orb.retarget(CGPoint(x: 400, y: 300), spec: .close)
            for _ in 0..<600 {
                let p = orb.step(dt: dt)
                XCTAssertLessThanOrEqual(p.x, 400 + 1e-9)
                XCTAssertLessThanOrEqual(p.y, 300 + 1e-9)
            }
            XCTAssertTrue(orb.settled)
        }
    }

    // decision: APP-504
    func testOpenSpringOvershootsSlightlyThenSettles() {
        var orb = OrbMotion(origin: .zero)
        orb.retarget(CGPoint(x: 400, y: 0), spec: .open)
        var peak: CGFloat = 0
        for _ in 0..<300 { peak = max(peak, orb.step(dt: 1.0 / 60).x) }
        XCTAssertGreaterThan(peak, 400)
        XCTAssertTrue(orb.settled)
    }

    // decision: APP-518
    func testOrbLaunchesFromTheVirtualNotchOnANotchlessScreen() {
        let frame = CGRect(x: 1000, y: 0, width: 1440, height: 900)
        XCTAssertEqual(OrbOrigin.notchCentre(screenFrame: frame, cutout: nil), CGPoint(x: 1720, y: 888))
        XCTAssertEqual(OrbOrigin.notchCentre(screenFrame: frame, cutout: nil, menuBarHeight: 40),
                       CGPoint(x: 1720, y: 880))
    }

    // decision: APP-501
    func testOrbLaunchesFromTheCutoutCentreOnANotchedScreen() {
        let frame = CGRect(x: 0, y: 0, width: 1512, height: 982)
        let cutout = CGRect(x: 656, y: 950, width: 200, height: 32)
        XCTAssertEqual(OrbOrigin.notchCentre(screenFrame: frame, cutout: cutout), CGPoint(x: 756, y: 966))
    }
}
