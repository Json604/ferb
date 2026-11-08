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

    // decision: RUN-429, APP-791
    /// The model chooser's list: the model in use always shows (one the owner typed included), a role's own model only
    /// for the connection it was chosen on, and the provider's words when it gave no list.
    func testTheModelListShowsTheModelInUseAndEachRolesOwn() throws {
        let list = try XCTUnwrap(ModelList(reply: [
            "runtime": "anthropic", "model": "claude-new-1", "models": ["claude-opus-5-5", "claude-sonnet-5-5"],
            "roles": ["worker": ["runtime": "anthropic", "model": "claude-sonnet-5-5"],
                      "checker": ["runtime": "codex", "model": "gpt-6.1-sol"]]]))
        XCTAssertEqual(list.models, ["claude-new-1", "claude-opus-5-5", "claude-sonnet-5-5"])
        XCTAssertEqual(list.roles, ["worker": "claude-sonnet-5-5"])
        XCTAssertNil(list.problem)
        let none = try XCTUnwrap(ModelList(reply: ["runtime": "google", "model": "gemini-3.8-flash", "models": [String](),
                                                   "problem": "HTTP 403: API key not valid"]))
        XCTAssertEqual(none.models, ["gemini-3.8-flash"])
        XCTAssertEqual(none.problem, "HTTP 403: API key not valid")
        XCTAssertNil(ModelList(reply: ["model": "x"]))
        XCTAssertEqual(RoleCopy.label("orchestrator"), "Planner")
    }
}
