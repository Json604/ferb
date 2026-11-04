import XCTest
@testable import NotchCore

/// FERB.app's setup window (L34 C): the daemon's replies as the window reads them, and what it may run.
final class SetupModelsTests: XCTestCase {
    private let pane = "x-apple.systempreferences:com.apple.preference.security?"

    // decision: APP-401, APP-536
    func testMicrophoneRowNeedsBothMicrophoneAndSpeech() {
        XCTAssertEqual(GrantStatus.both(.granted, .granted), .granted)
        XCTAssertEqual(GrantStatus.both(.granted, .denied), .denied)
        XCTAssertEqual(GrantStatus.both(.notDetermined, .granted), .notDetermined)
        XCTAssertEqual(GrantStatus.both(.unknown, .granted), .unknown)
    }

    // decision: APP-593
    func testAPIKeyLineCarriesTheKeyAsOneWordOrNothing() {
        XCTAssertEqual(APIKeyEntry.command(provider: .openai, key: "sk-proj-AbC_123"),
                       "add-generic-password -U -s ferb -a api/openai -w sk-proj-AbC_123\n")
        XCTAssertEqual(APIKeyEntry.account(.google), "api/google")
        for bad in ["", "two words", "quote\"d", "it's", "back\\slash", "tab\tkey", "ключ", "line\nbreak"] {
            XCTAssertNil(APIKeyEntry.command(provider: .anthropic, key: bad), bad)
        }
    }

    /// The tick is on the service FERB uses (the daemon's roles), not on every one signed in (owner, 2026-11-04:
    /// ChatGPT was ticked while FERB ran on the Anthropic API key).
    // decision: APP-655
    func testTheServiceInUseIsTheOneTheRolesRunOn() throws {
        let ai = try XCTUnwrap(AISnapshot(reply: [
            "roles": ["orchestrator": "anthropic", "worker": "anthropic", "checker": "anthropic"],
            "runtimes": [["name": "codex", "usable": true], ["name": "anthropic", "usable": true]]]))
        XCTAssertTrue(ai.usable("codex"))
        XCTAssertFalse(ai.inUse("codex"))
        XCTAssertTrue(ai.inUse("anthropic"))
        XCTAssertEqual(ai.apiKeyInUse, .anthropic)
    }
}
