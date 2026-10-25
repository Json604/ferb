import XCTest
@testable import NotchCore

/// First launch of the packaged FERB.app (L27): what to run, decided from the installed LaunchAgent alone.
final class AppSetupTests: XCTestCase {
    let agentExecutable = "/Applications/FERB.app/Contents/Helpers/FERB.app/Contents/MacOS/FERB"

    private func agent(_ program: String, build: String?) -> [String: Any] {
        var env = ["FERB_HOME": "/Users/o/.ferb"]
        if let build { env["FERB_APP_BUILD"] = build }
        return ["ProgramArguments": [program, "-m", "ferb.daemon"], "EnvironmentVariables": env]
    }

    func testNoInstalledAgentRunsSetup() {
        XCTAssertEqual(AppSetupPlan.decide(agent: nil, agentExecutable: agentExecutable, build: "202610140201"), .install)
        XCTAssertEqual(AppSetupPlan.serviceArguments(for: .install), ["setup", "--service"])
    }

    func testAgentFromThisAppAndBuildNeedsNothing() {
        let installed = agent(agentExecutable, build: "202610140201")
        XCTAssertEqual(AppSetupPlan.decide(agent: installed, agentExecutable: agentExecutable, build: "202610140201"), .none)
        XCTAssertNil(AppSetupPlan.serviceArguments(for: .none))
    }

    func testReplacedAppUpdatesWithoutAsking() {
        let installed = agent(agentExecutable, build: "202610140201")
        XCTAssertEqual(AppSetupPlan.decide(agent: installed, agentExecutable: agentExecutable, build: "202610150900"), .update)
        XCTAssertEqual(AppSetupPlan.serviceArguments(for: .update), ["setup", "--service", "--update"])
    }

    func testAnotherInstallationIsReplacedOnlyAfterTheOwnerSaysSo() {
        let source = agent("/Users/o/.ferb/venv/bin/python", build: nil)
        let action = AppSetupPlan.decide(agent: source, agentExecutable: agentExecutable, build: "202610140201")
        XCTAssertEqual(action, .confirmReplace(current: "/Users/o/.ferb/venv/bin/python"))
        XCTAssertEqual(AppSetupPlan.serviceArguments(for: action), ["setup", "--service"])
        XCTAssertTrue(AppSetupPlan.needsConfirmation(action))
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

    func testAgentWithoutProgramIsTreatedAsMissing() {
        XCTAssertEqual(AppSetupPlan.decide(agent: ["Label": "com.ferb.daemon"], agentExecutable: agentExecutable, build: "1"), .install)
    }

    func testAgentFromThisAppThatIsntRunningIsRepairedByAnUpdate() {
        // e.g. setup was stopped while it reinstalled the service, or the daemon keeps failing to start
        let installed = agent(agentExecutable, build: "202610140201")
        XCTAssertEqual(AppSetupPlan.decide(agent: installed, agentExecutable: agentExecutable, build: "202610140201",
                                           daemonRunning: false), .update)
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
