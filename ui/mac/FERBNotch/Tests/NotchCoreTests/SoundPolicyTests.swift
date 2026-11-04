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

    // decision: APP-653
    func testNothingPlaysWhenSystemUISoundsAreOff() {
        func decide(ui: Bool, feedback: Bool) -> SoundDecision? {
            SoundPolicy.decide(.error, uiSoundsEnabled: ui, feedbackEnabled: feedback, switchOn: true, subtle: false,
                               onACall: { false })
        }
        XCTAssertNil(decide(ui: false, feedback: true))
        XCTAssertNil(decide(ui: true, feedback: false))
        XCTAssertEqual(decide(ui: true, feedback: true), SoundDecision(subtle: false))
    }

    /// APP-643 (owner, 2026-11-03: a friend's M1 played no startup music): the first-launch music plays with the
    /// system's interface sounds off; every other sound still follows that switch, and FERB's own switches still apply.
    // decision: APP-643
    func testTheFirstLaunchMusicPlaysWithInterfaceSoundsOff() {
        XCTAssertNotNil(SoundPolicy.decide(.firstLaunch, uiSoundsEnabled: false, feedbackEnabled: true, switchOn: true,
                                           subtle: false, onACall: { false }))
        XCTAssertNil(SoundPolicy.decide(.error, uiSoundsEnabled: false, feedbackEnabled: true, switchOn: true,
                                        subtle: false, onACall: { false }))
        XCTAssertNil(SoundPolicy.decide(.firstLaunch, uiSoundsEnabled: false, feedbackEnabled: false, switchOn: true,
                                        subtle: false, onACall: { false }))
    }

    /// APP-651 (owner, 2026-11-04): a finished task or answer plays no sound; the rest of the outcomes keep theirs.
    // decision: APP-651
    func testAFinishedTaskPlaysNoSound() {
        XCTAssertNil(SoundPolicy.outcomeSound("done"))
        XCTAssertNil(SoundPolicy.outcomeSound("done_unverified"))
        XCTAssertEqual(SoundPolicy.outcomeSound("needs_user"), .needsYou)
        XCTAssertEqual(SoundPolicy.outcomeSound("failed"), .error)
    }
}
