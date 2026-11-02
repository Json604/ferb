import XCTest
import CoreGraphics
@testable import NotchCore

final class SoundPolicyTests: XCTestCase {
    // decision: APP-523
    func testPanIsClampedToPointSix() {
        XCTAssertEqual(SoundPolicy.pan(x: 0, screenWidth: 1440), -0.6, accuracy: 1e-6)
        XCTAssertEqual(SoundPolicy.pan(x: 1440, screenWidth: 1440), 0.6, accuracy: 1e-6)
        XCTAssertEqual(SoundPolicy.pan(x: 720, screenWidth: 1440), 0, accuracy: 1e-6)
        XCTAssertEqual(SoundPolicy.pan(x: 5000, screenWidth: 1440), 0.6, accuracy: 1e-6)
    }

    // decision: APP-523
    func testDistanceLowersGainAtMostThreeDB() {
        XCTAssertEqual(SoundPolicy.distanceGainDB(fromNotch: 0, screenHeight: 900), 0, accuracy: 1e-6)
        XCTAssertEqual(SoundPolicy.distanceGainDB(fromNotch: 900, screenHeight: 900), -3, accuracy: 1e-6)
        XCTAssertEqual(SoundPolicy.distanceGainDB(fromNotch: 4000, screenHeight: 900), -3, accuracy: 1e-6)
        XCTAssertEqual(SoundPolicy.distanceGainDB(fromNotch: 450, screenHeight: 900), -1.5, accuracy: 1e-6)
    }

    // decision: APP-640
    func testNothingPlaysWhenSystemUISoundsAreOff() {
        func decide(ui: Bool, feedback: Bool) -> SoundDecision? {
            SoundPolicy.decide(.done, uiSoundsEnabled: ui, feedbackEnabled: feedback, switchOn: true, subtle: false,
                               onACall: { false })
        }
        XCTAssertNil(decide(ui: false, feedback: true))
        XCTAssertNil(decide(ui: true, feedback: false))
        XCTAssertEqual(decide(ui: true, feedback: true), SoundDecision(subtle: false))
    }
}
