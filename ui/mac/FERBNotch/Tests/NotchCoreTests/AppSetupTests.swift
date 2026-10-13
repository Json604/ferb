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
        XCTAssertEqual(AppSetupPlan.arguments(for: .install), ["setup"])
    }

    func testAgentFromThisAppAndBuildNeedsNothing() {
        let installed = agent(agentExecutable, build: "202610140201")
        XCTAssertEqual(AppSetupPlan.decide(agent: installed, agentExecutable: agentExecutable, build: "202610140201"), .none)
        XCTAssertNil(AppSetupPlan.arguments(for: .none))
    }

    func testReplacedAppUpdatesWithoutAsking() {
        let installed = agent(agentExecutable, build: "202610140201")
        XCTAssertEqual(AppSetupPlan.decide(agent: installed, agentExecutable: agentExecutable, build: "202610150900"), .update)
        XCTAssertEqual(AppSetupPlan.arguments(for: .update), ["setup", "--update"])
    }

    func testAnotherInstallationIsReplacedOnlyAfterTheOwnerSaysSo() {
        let source = agent("/Users/o/.ferb/venv/bin/python", build: nil)
        let action = AppSetupPlan.decide(agent: source, agentExecutable: agentExecutable, build: "202610140201")
        XCTAssertEqual(action, .confirmReplace(current: "/Users/o/.ferb/venv/bin/python"))
        XCTAssertEqual(AppSetupPlan.arguments(for: action), ["setup"])
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

    func testMenuRunsFullSetupOnlyForThisAppAndAsksBeforeReplacingAnother() {
        let installed = agent(agentExecutable, build: "202610140201")
        let same = AppSetupPlan.decide(agent: installed, agentExecutable: agentExecutable, build: "202610140201")
        XCTAssertEqual(AppSetupPlan.arguments(for: same, fromMenu: true), ["setup"])      // permissions again, etc.
        let replaced = AppSetupPlan.decide(agent: installed, agentExecutable: agentExecutable, build: "202610150900")
        XCTAssertEqual(AppSetupPlan.arguments(for: replaced, fromMenu: true), ["setup", "--update"])
        let other = AppSetupPlan.decide(agent: agent("/Users/o/.ferb/venv/bin/python", build: nil),
                                        agentExecutable: agentExecutable, build: "202610140201")
        XCTAssertTrue(AppSetupPlan.needsConfirmation(other))
        XCTAssertFalse(AppSetupPlan.needsConfirmation(same))
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
