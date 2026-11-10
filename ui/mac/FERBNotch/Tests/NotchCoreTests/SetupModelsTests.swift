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
    // decision: APP-882
    func testTheServiceInUseIsTheOneTheRolesRunOn() throws {
        let ai = try XCTUnwrap(AISnapshot(reply: [
            "roles": ["orchestrator": "anthropic", "worker": "anthropic", "checker": "anthropic"],
            "runtimes": [["name": "codex", "usable": true], ["name": "anthropic", "usable": true]]]))
        XCTAssertTrue(ai.usable("codex"))
        XCTAssertFalse(ai.inUse("codex"))
        XCTAssertTrue(ai.inUse("anthropic"))
        XCTAssertEqual(ai.apiKeyInUse, .anthropic)
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

    // decision: APP-882
    /// Owner 2026-11-10: "settings correct data ... the green tick and other thigns not being shown or hidden". Each
    /// card says which roles run on it; mixed roles list each connection once; a refusal stays with its time and step.
    func testEachCardSaysWhichRolesRunOnItAndItsLastRefusal() throws {
        let ai = try XCTUnwrap(AISnapshot(reply: [
            "roles": ["orchestrator": "claude", "worker": "codex", "checker": "codex"],
            "role_details": ["orchestrator": ["runtime": "claude", "model": "claude-opus-5-5", "effort": "medium"]],
            "runtimes": [["name": "codex", "usable": true, "effort": "high"],
                         ["name": "claude", "usable": true, "refused": ["account": "signed_out",
                                                                         "at": "2026-11-10T14:02:03.123456+00:00",
                                                                         "fix": "sign in to Claude again in Settings → AI"]]]]))
        XCTAssertNil(ai.current)
        XCTAssertEqual(ai.runtimesInUse, ["claude", "codex"])
        XCTAssertEqual(ai.rolesText("codex"), "Worker · Checker")
        XCTAssertEqual(ai.rolesText("claude"), "Planner")
        XCTAssertEqual(ai.rolesText("anthropic"), "")
        XCTAssertEqual(ai.roleDetails["orchestrator"], RoleAI(runtime: "claude", model: "claude-opus-5-5", effort: "medium"))
        XCTAssertEqual(ai.row("codex")?.effort, "high")
        XCTAssertTrue(ai.runs("claude", role: "orchestrator") && !ai.runs("claude", role: "worker"))
        let refused = try XCTUnwrap(ai.row("claude")?.refused)
        XCTAssertEqual(refused.text(timeZone: TimeZone(identifier: "UTC")!).hasPrefix("Last call refused: signed out at "), true)
        XCTAssertTrue(refused.text().hasSuffix("Sign in to Claude again in Settings → AI."))
        XCTAssertNil(ai.row("codex")?.refused)
    }

    // decision: APP-881, APP-885
    /// "show the effort level along with model selection(along with text field if the effort level in our settigns
    /// is not applicable to that)": each model's own levels, its default, a set level kept even when unlisted.
    func testTheEffortPickerFollowsTheChosenModel() throws {
        let list = try XCTUnwrap(ModelList(reply: [
            "runtime": "codex", "model": "gpt-6.1-sol", "models": ["gpt-6.1-sol", "gpt-x"], "effort": "high",
            "levels": ["gpt-6.1-sol": ["low", "medium", "high", "xhigh", "max"]],
            "default_efforts": ["gpt-6.1-sol": "low"],
            "roles": ["orchestrator": ["runtime": "codex", "model": "", "effort": "medium"]]]))
        XCTAssertEqual(list.effort(for: nil), "high")
        XCTAssertEqual(list.effort(for: "orchestrator"), "medium")
        XCTAssertEqual(list.current(for: "orchestrator"), "")                  // an effort alone keeps the model
        XCTAssertEqual(list.levels(for: ""), ["low", "medium", "high", "xhigh", "max"])
        XCTAssertEqual(list.levels(for: "gpt-x"), [])                          // undocumented: Default and Other…
        XCTAssertEqual(list.levels(for: "gpt-x", keeping: "turbo"), ["turbo"])
        XCTAssertEqual(list.defaultLabel(for: "gpt-6.1-sol"), "Default (low)")
        XCTAssertEqual(list.defaultLabel(for: "gpt-x"), "Default (the model's own)")
    }

    // decision: APP-882
    /// "FERB uses … now" only for a connection (or role) that runs there; otherwise it says it's saved for later.
    func testTheSavedLineSaysUsesNowOnlyWhereARoleRuns() {
        XCTAssertEqual(ModelSaveLine.saved(model: "gpt-6.1-sol", effort: "high", connectionModel: "", runtime: "codex",
                                           role: nil, running: true), "FERB uses gpt-6.1-sol at high effort now.")
        XCTAssertEqual(ModelSaveLine.saved(model: "claude-sonnet-5-5", effort: "", connectionModel: "", runtime: "claude",
                                           role: nil, running: false),
                       "Saved for Claude; FERB uses it when a role runs on Claude.")
        XCTAssertEqual(ModelSaveLine.saved(model: "", effort: "medium", connectionModel: "claude-opus-5-5",
                                           runtime: "claude", role: "orchestrator", running: true),
                       "The planner uses claude-opus-5-5 at medium effort now.")
        // live 2026-11-10 (APP-827): the Claude card said "FERB uses claude-sonnet-5-5 now." while FERB ran on ChatGPT
        let elsewhere = ModelSaveLine.saved(model: "claude-opus-5-5", effort: "", connectionModel: "", runtime: "claude",
                                            role: "worker", running: false)
        XCTAssertFalse(elsewhere.contains("now"))
        XCTAssertEqual(elsewhere, "Saved for the worker on Claude; it's used when the worker runs on Claude.")
    }

    // decision: APP-883
    func testABrowserRowSaysItsOwnStateVersionAndProblem() throws {
        let snapshot = try XCTUnwrap(BrowserSnapshot(reply: ["browsers": [["browser": "chrome", "profiles": [
            ["folder": "Default", "state": "paired", "connected": true, "version": "0.19.4"],
            ["folder": "P1", "state": "paired", "connected": false, "problem": "it refused the connection"],
            ["folder": "P2", "state": "on", "version": "0.1.0", "too_old": true],
            ["folder": "P3", "state": "paired", "connected": false]]]]]))
        let rows = snapshot.browsers[0].profiles
        XCTAssertEqual(rows.map(\.stateText), ["Paired · connected", "Not connected: it refused the connection",
                                               "Too old (version 0.1.0): update FERB's extension in this profile",
                                               "Paired · not connected (open this profile)"])
        XCTAssertEqual(rows.map(\.needsAttention), [false, true, true, false])
        XCTAssertEqual(rows[0].version, "0.19.4")
    }

    // decision: APP-884
    func testASettingFERBDidntGiveIsUnknownNotADefault() {
        let snapshot = SettingsSnapshot(["values": ["sense.window_titles": false]])
        XCTAssertTrue(snapshot.has("sense.window_titles"))
        XCTAssertFalse(snapshot.has("retention.trace_days"))
        XCTAssertEqual(PermissionCopy.status(.denied), "Off")
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
