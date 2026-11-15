import XCTest
@testable import NotchCore

final class IntroPlanTests: XCTestCase {
    // decision: APP-635, APP-685
    func testFinishedSetupGetsNoTourUnlessReplayed() {
        let needs = IntroNeeds(setupDone: true, signedIn: true, missingPermissions: [], unpairedBrowsers: false, firstRun: false)
        XCTAssertEqual(IntroPlan.beats(needs, replay: false), [])
        XCTAssertEqual(IntroPlan.beats(needs, replay: true), IntroBeat.allCases)
    }

    // decision: APP-635
    func testMissingGrantsOnlyGetsThePermissionsBeatAndHome() {
        let needs = IntroNeeds(setupDone: false, signedIn: true, missingPermissions: ["screen_recording"],
                               unpairedBrowsers: false, firstRun: false)
        XCTAssertEqual(IntroPlan.beats(needs, replay: false), [.permissions, .home])
    }

    // decision: APP-635
    func testFirstRunGetsEveryBeatEvenIfAlreadySignedIn() {
        let needs = IntroNeeds(setupDone: false, signedIn: true, missingPermissions: [], unpairedBrowsers: false,
                               firstRun: true)
        XCTAssertEqual(IntroPlan.beats(needs, replay: false), IntroBeat.allCases)
    }

    // decision: APP-685
    func testResumeSurvivesARelaunch() {
        let suite = Self.scratchSuiteName("IntroPlan")
        addTeardownBlock { Self.removeScratchSuite(suite) }
        var first = IntroProgress(defaults: UserDefaults(suiteName: suite)!)
        first.resumeBeat = .permissions
        first.done = true
        var second = IntroProgress(defaults: UserDefaults(suiteName: suite)!)
        XCTAssertEqual(second.resumeBeat, .permissions)
        XCTAssertTrue(second.done)
        second.clear()
        XCTAssertNil(second.resumeBeat)
        XCTAssertFalse(second.done)
        let third = IntroProgress(defaults: UserDefaults(suiteName: suite)!)
        XCTAssertNil(third.resumeBeat)
        XCTAssertFalse(third.done)
    }
}
