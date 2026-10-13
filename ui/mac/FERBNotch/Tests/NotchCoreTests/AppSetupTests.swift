import XCTest
@testable import NotchCore

/// First launch of the packaged FERB.app (L27): what to run, decided from the installed LaunchAgent alone.
final class AppSetupTests: XCTestCase {
    let python = "/Applications/FERB.app/Contents/Resources/python/bin/python3.12"

    private func agent(_ program: String, build: String?) -> [String: Any] {
        var env = ["FERB_HOME": "/Users/o/.ferb"]
        if let build { env["FERB_APP_BUILD"] = build }
        return ["ProgramArguments": [program, "-I", "-B", "-m", "ferb.daemon"], "EnvironmentVariables": env]
    }

    func testNoInstalledAgentRunsSetup() {
        XCTAssertEqual(AppSetupPlan.decide(agent: nil, bundlePython: python, build: "202610140201"), .install)
        XCTAssertEqual(AppSetupPlan.arguments(for: .install), ["setup"])
    }

    func testAgentFromThisAppAndBuildNeedsNothing() {
        let installed = agent(python, build: "202610140201")
        XCTAssertEqual(AppSetupPlan.decide(agent: installed, bundlePython: python, build: "202610140201"), .none)
        XCTAssertNil(AppSetupPlan.arguments(for: .none))
    }

    func testReplacedAppUpdatesWithoutAsking() {
        let installed = agent(python, build: "202610140201")
        XCTAssertEqual(AppSetupPlan.decide(agent: installed, bundlePython: python, build: "202610150900"), .update)
        XCTAssertEqual(AppSetupPlan.arguments(for: .update), ["setup", "--update"])
    }

    func testAnotherInstallationIsReplacedOnlyAfterTheOwnerSaysSo() {
        let source = agent("/Users/o/.ferb/venv/bin/python", build: nil)
        let action = AppSetupPlan.decide(agent: source, bundlePython: python, build: "202610140201")
        XCTAssertEqual(action, .confirmReplace(current: "/Users/o/.ferb/venv/bin/python"))
        XCTAssertEqual(AppSetupPlan.arguments(for: action), ["setup"])
    }

    func testAgentWithoutProgramIsTreatedAsMissing() {
        XCTAssertEqual(AppSetupPlan.decide(agent: ["Label": "com.ferb.daemon"], bundlePython: python, build: "1"), .install)
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
