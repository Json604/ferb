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
    func testAnAPIKeyIsOneWordOrNothing() {
        XCTAssertTrue(APIKeyEntry.isStorable("sk-proj-AbC_123"))
        for bad in ["", "two words", "quote\"d", "it's", "back\\slash", "tab\tkey", "ключ", "line\nbreak"] {
            XCTAssertFalse(APIKeyEntry.isStorable(bad), bad)
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

    /// A model picked on a connection FERB doesn't use now is saved for it, never "FERB uses it now" (live 2026-11-10:
    /// the Claude card said "FERB uses claude-sonnet-5-5 now." while FERB ran on ChatGPT).
    // decision: APP-827
    func testAModelSavedOnAConnectionNotInUseSaysItIsSavedForIt() throws {
        let ai = try XCTUnwrap(AISnapshot(reply: [
            "roles": ["orchestrator": "codex", "worker": "codex", "checker": "claude"],
            "runtimes": [["name": "codex", "usable": true], ["name": "claude", "usable": true],
                         ["name": "anthropic", "usable": true]]]))
        XCTAssertEqual(ai.modelSaved("codex", "gpt-6.1-sol"), "FERB uses gpt-6.1-sol now.")
        XCTAssertFalse(ai.modelSaved("anthropic", "claude-sonnet-5-5").contains("now"))
        XCTAssertTrue(ai.modelSaved("anthropic", "claude-sonnet-5-5").contains("when"))
        XCTAssertTrue(ai.modelSaved("claude", "claude-opus-5-5", role: "checker").contains("now"))
        XCTAssertFalse(ai.modelSaved("claude", "claude-opus-5-5", role: "worker").contains("now"))
    }

    // decision: RUN-429, APP-791, APP-793
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
        // APP-793: a role's picker is the connection's list plus its own typed model; none chosen is the connection's
        XCTAssertEqual(list.current(for: "worker"), "claude-sonnet-5-5")
        XCTAssertEqual(list.current(for: "checker"), "")
        XCTAssertEqual(list.current(for: nil), "claude-new-1")
        let typed = try XCTUnwrap(ModelList(reply: ["runtime": "anthropic", "model": "claude-opus-5-5",
                                                     "models": ["claude-opus-5-5"],
                                                     "roles": ["worker": ["runtime": "anthropic", "model": "claude-x-9"]]]))
        XCTAssertEqual(typed.options(for: "worker"), ["claude-x-9", "claude-opus-5-5"])
        XCTAssertEqual(typed.options(for: "planner"), ["claude-opus-5-5"])
        XCTAssertEqual(RoleCopy.label("orchestrator"), "Planner")
    }

    // decision: RUN-434
    /// Settings → AI's Check: one line per call shape, the provider's words after a failure, and the total.
    func testTheConnectionCheckSaysEachCallsResultAndTheTotal() {
        let check = ConnectionCheck(reply: [
            "ok": false, "total_cost_usd": 0.0123,
            "lines": [["name": "Planner call", "ok": false, "detail": "HTTP 400: The compiled grammar is too large",
                       "model": "claude-sonnet-5-5", "typed": "strict", "seconds": 1.5, "input_tokens": 900,
                       "output_tokens": 0],
                      ["name": "Tool call with no arguments", "ok": true, "detail": "", "model": "claude-sonnet-5-5",
                       "typed": "stated_after_refusal", "seconds": 0.0, "input_tokens": 0, "output_tokens": 0,
                       "cost_usd": 0.0021]]])
        XCTAssertEqual(check.lines.count, 2)
        XCTAssertEqual(check.lines[0].text, "✗ Planner call — claude-sonnet-5-5, typed strict, 1.5 s, 900 tokens: "
                       + "HTTP 400: The compiled grammar is too large")
        XCTAssertTrue(check.lines[1].text.hasPrefix("✓ Tool call with no arguments — claude-sonnet-5-5, typed "
                                                    + "stated_after_refusal"))
        XCTAssertEqual(check.summary, "1 of 2 checks passed. Cost: $0.0123.")
    }

    // decision: RUN-439
    /// Owner 2026-11-08: "checking is stuck on gogole api(most prob cause its free and its stucj in rate limt or
    /// sometign). even is hift provider the chedck box didtn refrsh". A call given up says so with its limit; the
    /// tool-call lines say they ran inside the worker call; the result names its connection.
    func testACheckGivenUpSaysItsLimitAndToolLinesSayWhereTheyRan() {
        let check = ConnectionCheck(reply: [
            "runtime": "google", "ok": false,
            "lines": [["name": "Tool call with no arguments", "ok": true, "detail": "", "model": "gemini-3.8-flash",
                       "typed": "strict", "seconds": 0.0, "input_tokens": 0, "output_tokens": 0, "limit_s": 60.0,
                       "part_of": "worker call"],
                      ["name": "Worker call (tools, its typed report)", "ok": false, "limit_s": 60.0,
                       "detail": "no answer after 60 s; the provider answered a rate or usage limit (HTTP 429) 2 times",
                       "model": "gemini-3.8-flash", "typed": "none", "seconds": 60.0, "input_tokens": 0,
                       "output_tokens": 0]]])
        XCTAssertEqual(check.runtime, "google")
        XCTAssertEqual(check.lines[0].text, "✓ Tool call with no arguments — ran inside the worker call")
        XCTAssertTrue(check.lines[1].text.hasSuffix(": no answer after 60 s; the provider answered a rate or usage "
                                                     + "limit (HTTP 429) 2 times"))
        XCTAssertEqual(check.summary, "1 of 2 checks passed. Each call is given up after 60 s.")
    }
}
