import XCTest
@testable import NotchCore

/// L34 Phase F: FERB.app opens at login. The fallback LaunchAgent opens the app in the background, and the default
/// applies once.
final class LoginItemPlanTests: XCTestCase {
    func testTheFallbackAgentOpensFERBInTheBackgroundAtLoginOnly() {
        let agent = LoginItemPlan.agent(appPath: "/Applications/FERB.app")
        XCTAssertEqual(agent["Label"] as? String, "com.ferb.FERB.login")
        XCTAssertEqual(agent["ProgramArguments"] as? [String], ["/usr/bin/open", "-g", "-a", "/Applications/FERB.app"])
        XCTAssertEqual(agent["RunAtLoad"] as? Bool, true)
        XCTAssertEqual(agent["LimitLoadToSessionType"] as? String, "Aqua")
        XCTAssertEqual(agent["AssociatedBundleIdentifiers"] as? [String], ["com.ferb.FERB"])
        XCTAssertNil(agent["KeepAlive"])                                   // opened once per login, never relaunched
    }

    func testOnlyFERBsOwnAgentCountsAsOurs() {
        XCTAssertTrue(LoginItemPlan.isOurs(LoginItemPlan.agent(appPath: "/Applications/FERB.app")))
        XCTAssertFalse(LoginItemPlan.isOurs(["Label": "com.example.other"]))
        XCTAssertFalse(LoginItemPlan.isOurs(nil))
    }

    func testTheDefaultAppliesOnce() {
        XCTAssertTrue(LoginItemPlan.shouldApplyDefault(applied: false))
        XCTAssertFalse(LoginItemPlan.shouldApplyDefault(applied: true))
    }

    func testTheNotesSayWhatHappened() {
        XCTAssertEqual(LoginItemPlan.approvalNote,
                       "macOS needs your OK: System Settings → General → Login Items → allow FERB.")
        XCTAssertEqual(LoginItemPlan.fallbackNote("Operation not permitted"),
                       "macOS refused FERB's login item (Operation not permitted), so a login LaunchAgent opens FERB instead.")
        XCTAssertEqual(LoginItemPlan.fallbackNote(nil),
                       "macOS refused FERB's login item, so a login LaunchAgent opens FERB instead.")
        XCTAssertEqual(LoginItemPlan.Refused().localizedDescription, "the login item didn't turn on")
    }
}
