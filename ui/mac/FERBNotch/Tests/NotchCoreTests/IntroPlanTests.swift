import XCTest
@testable import NotchCore

final class IntroPlanTests: XCTestCase {
    private let allRequired = ["accessibility", "screen_recording", "full_disk_access", "input_monitoring",
                        "notifications", "microphone", "clipboard"]

    func testNewInstallGetsEveryBeatInOrder() {
        let needs = IntroNeeds(setupDone: false, signedIn: false, missingPermissions: allRequired, unpairedBrowsers: true, firstRun: true)
        XCTAssertEqual(IntroPlan.beats(needs, replay: false), IntroBeat.allCases)
    }

    func testFinishedSetupGetsNoTourUnlessReplayed() {
        let needs = IntroNeeds(setupDone: true, signedIn: true, missingPermissions: [], unpairedBrowsers: false, firstRun: false)
        XCTAssertEqual(IntroPlan.beats(needs, replay: false), [])
        XCTAssertEqual(IntroPlan.beats(needs, replay: true), IntroBeat.allCases)
    }

    func testMissingGrantsOnlyGetsThePermissionsBeatAndHome() {
        let needs = IntroNeeds(setupDone: false, signedIn: true, missingPermissions: ["screen_recording"],
                               unpairedBrowsers: false, firstRun: false)
        XCTAssertEqual(IntroPlan.beats(needs, replay: false), [.permissions, .home])
    }

    func testFirstRunGetsEveryBeatEvenIfAlreadySignedIn() {
        let needs = IntroNeeds(setupDone: false, signedIn: true, missingPermissions: [], unpairedBrowsers: false,
                               firstRun: true)
        XCTAssertEqual(IntroPlan.beats(needs, replay: false), IntroBeat.allCases)
    }

    func testSignedOutOnlyGetsSignInAndHome() {
        let needs = IntroNeeds(setupDone: false, signedIn: false, missingPermissions: [], unpairedBrowsers: false,
                               firstRun: false)
        XCTAssertEqual(IntroPlan.beats(needs, replay: false), [.signIn, .home])
    }

    func testNothingMissingAndNotFirstRunGetsNoBeats() {
        let needs = IntroNeeds(setupDone: false, signedIn: true, missingPermissions: [], unpairedBrowsers: false,
                               firstRun: false)
        XCTAssertEqual(IntroPlan.beats(needs, replay: false), [])
    }

    func testNextSkipsToTheFollowingBeatOfThePlan() {
        let plan: [IntroBeat] = [.permissions, .home]
        XCTAssertEqual(IntroPlan.next(after: .permissions, in: plan), .home)
        XCTAssertNil(IntroPlan.next(after: .home, in: plan))
    }

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
