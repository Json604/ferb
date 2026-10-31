import XCTest
@testable import NotchCore

/// Launch of the packaged FERB.app (L27): what to run, from the old LaunchAgent, the app service's status and setup's record.
final class AppSetupTests: XCTestCase {
    let agentExecutable = "/Applications/FERB.app/Contents/Helpers/FERB.app/Contents/MacOS/FERB"
    let app = "/Applications/FERB.app"

    private func legacy(_ program: String) -> [String: Any] {
        ["Label": "com.ferb.daemon", "ProgramArguments": [program, "-m", "ferb.daemon"]]
    }

    private func decide(legacy agent: [String: Any]? = nil, _ service: ServiceStatus, record: [String: Any]?,
                        build: String = "202610140201", running: Bool = true) -> AppSetupAction {
        AppSetupPlan.decide(legacyAgent: agent, service: service, record: record, agentExecutable: agentExecutable,
                            appPath: app, build: build, daemonRunning: running)
    }

    func testNothingSetUpRunsSetup() {
        XCTAssertEqual(decide(.notRegistered, record: nil), .install)
        XCTAssertEqual(AppSetupPlan.serviceArguments(for: .install), ["setup", "--service"])
    }

    func testThisAppsBuildSetUpAndRunningNeedsNothing() {
        XCTAssertEqual(decide(.enabled, record: ["build": "202610140201", "app": app]), .none)
        XCTAssertNil(AppSetupPlan.serviceArguments(for: .none))
    }

    func testReplacedAppUpdatesWithoutAsking() {
        XCTAssertEqual(decide(.enabled, record: ["build": "202610140201", "app": app], build: "202610150900"), .update)
        XCTAssertEqual(AppSetupPlan.serviceArguments(for: .update), ["setup", "--service", "--update"])
    }

    /// Storage decision 5: an install from before the app registered its own service has the old user LaunchAgent
    /// running this very app; it is retired by a quiet update, never by a question.
    func testAnOldLaunchAgentOfThisAppIsRetiredByAQuietUpdate() {
        XCTAssertEqual(decide(legacy: legacy(agentExecutable), .notRegistered, record: nil), .update)
        XCTAssertEqual(decide(legacy: legacy(agentExecutable), .enabled, record: ["build": "202610140201", "app": app]),
                       .update)
    }

    func testAnotherInstallationIsReplacedOnlyAfterTheOwnerSaysSo() {
        let action = decide(legacy: legacy("/Users/o/.ferb/venv/bin/python"), .notRegistered, record: nil)
        XCTAssertEqual(action, .confirmReplace(current: "/Users/o/.ferb/venv/bin/python"))
        XCTAssertEqual(AppSetupPlan.serviceArguments(for: action), ["setup", "--service"])
        XCTAssertTrue(AppSetupPlan.needsConfirmation(action))
    }

    func testAServiceTheOwnerTurnedOffOrThatIsntRunningIsRepairedByAnUpdate() {
        let record: [String: Any] = ["build": "202610140201", "app": app]
        XCTAssertEqual(decide(.requiresApproval, record: record), .update)
        XCTAssertEqual(decide(.notRegistered, record: record), .update)
        XCTAssertEqual(decide(.enabled, record: record, running: false), .update)
        XCTAssertEqual(decide(.enabled, record: ["build": "202610140201", "app": "/Users/o/Downloads/FERB.app"]), .update)
        XCTAssertEqual(decide(.enabled, record: nil), .update)
    }

    func testTheServiceProblemsAreSaidInTheAppsWords() {
        XCTAssertNil(ServicePlan.problem(.enabled))
        XCTAssertTrue(ServicePlan.problem(.requiresApproval)?.contains("Login Items") == true)
        XCTAssertTrue(ServicePlan.problem(.notFound)?.contains("Download FERB again") == true)
        XCTAssertEqual(ServicePlan.plistName, "com.ferb.daemon.plist")
    }

    func testLaunchOpensTheWindowOnlyWhenSomethingIsLeft() {
        XCTAssertEqual(AppSetupPlan.atLaunch(.install, setupDone: false), .window(.welcome))
        XCTAssertEqual(AppSetupPlan.atLaunch(.confirmReplace(current: "/x"), setupDone: true), .window(.welcome))
        XCTAssertEqual(AppSetupPlan.atLaunch(.update, setupDone: true), .quietUpdate)
        XCTAssertEqual(AppSetupPlan.atLaunch(.none, setupDone: true), .nothing)
        XCTAssertEqual(AppSetupPlan.atLaunch(.none, setupDone: false), .window(.welcome))
        XCTAssertFalse(AppSetupPlan.needsConfirmation(.none))
    }

    /// Owner 2026-10-26: after Screen Recording's "Quit & Reopen" FERB came back without its setup window (setup counted
    /// as done from an earlier install). A Grant in progress reopens the window on that step, done or not.
    func testLaunchReopensTheStepAGrantRestartedFrom() {
        XCTAssertEqual(AppSetupPlan.atLaunch(.none, setupDone: true, resume: .permissions), .window(.permissions))
        XCTAssertEqual(AppSetupPlan.atLaunch(.none, setupDone: false, resume: .permissions), .window(.permissions))
        XCTAssertEqual(AppSetupPlan.atLaunch(.none, setupDone: true, resume: nil), .nothing)
        XCTAssertEqual(AppSetupPlan.atLaunch(.update, setupDone: true, resume: .permissions), .quietUpdate)
    }

    func testSignInToCodexRunsFERBsOwnCodexLogin() {
        XCTAssertEqual(AppSetupPlan.codexLoginArguments, ["setup", "--codex-login"])
    }

    func testConsoleKeepsCharactersSplitAcrossReads() {
        var console = ConsoleDecoder()
        let bytes = Array("✓ granted\n".utf8)                   // ✓ is three bytes
        XCTAssertEqual(console.append(Data(bytes[0..<2])), "")
        XCTAssertEqual(console.append(Data(bytes[2...])), "✓ granted\n")
        XCTAssertEqual(console.append(Data("Press Enter: ".utf8)), "Press Enter: ")   // prompts end without a newline
    }

    func testConsoleReplacesInvalidBytesInsteadOfStalling() {
        var console = ConsoleDecoder()
        XCTAssertEqual(console.append(Data([0x66, 0xFF, 0xFE, 0x67])), "f\u{FFFD}\u{FFFD}g")
    }
}
