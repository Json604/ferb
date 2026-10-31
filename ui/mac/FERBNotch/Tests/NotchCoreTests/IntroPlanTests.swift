import XCTest
@testable import NotchCore

final class IntroPlanTests: XCTestCase {
    private let all8 = ["accessibility", "screen_recording", "full_disk_access", "input_monitoring",
                        "notifications", "microphone", "hearing", "clipboard"]

    func testNewInstallGetsEveryBeatInOrder() {
        let needs = IntroNeeds(setupDone: false, signedIn: false, missingPermissions: all8, unpairedBrowsers: true)
        XCTAssertEqual(IntroPlan.beats(needs, replay: false), IntroBeat.allCases)
    }

    func testFinishedSetupGetsNoTourUnlessReplayed() {
        let needs = IntroNeeds(setupDone: true, signedIn: true, missingPermissions: [], unpairedBrowsers: false)
        XCTAssertEqual(IntroPlan.beats(needs, replay: false), [])
        XCTAssertEqual(IntroPlan.beats(needs, replay: true), IntroBeat.allCases)
    }

    func testMissingGrantsOnlyGetsThePermissionsBeatAndHome() {
        let needs = IntroNeeds(setupDone: false, signedIn: true, missingPermissions: ["screen_recording"],
                               unpairedBrowsers: false)
        XCTAssertEqual(IntroPlan.beats(needs, replay: false), [.permissions, .home])
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
        var second = IntroProgress(defaults: UserDefaults(suiteName: suite)!)
        XCTAssertEqual(second.resumeBeat, .permissions)
        second.clear()
        XCTAssertNil(second.resumeBeat)
    }
}
