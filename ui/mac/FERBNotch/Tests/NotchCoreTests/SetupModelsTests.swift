import XCTest
@testable import NotchCore

/// FERB.app's setup window (L34 C): the daemon's replies as the window reads them, and what it may run.
final class SetupModelsTests: XCTestCase {
    private let pane = "x-apple.systempreferences:com.apple.preference.security?"

    func testStepsRunInTheSpecsOrder() {
        XCTAssertEqual(SetupStep.allCases, [.welcome, .ai, .permissions, .browsers, .persona, .done])
        XCTAssertEqual(SetupStep.welcome.next, .ai)
        XCTAssertNil(SetupStep.done.next)
    }

    func testPermissionReplyKeepsWhoseGrantPaneAndBlock() {
        let reply: [String: Any] = [
            "rows": [
                ["id": "accessibility", "status": "denied", "process": "daemon", "required": true, "requestable": true,
                 "restart_to_check": false, "blocked": "", "pane": pane + "Privacy_Accessibility"],
                ["id": "full_disk_access", "status": "denied", "process": "daemon", "required": true,
                 "requestable": false, "restart_to_check": true,
                 "blocked": "FERB's shell sandbox and network proxy must be working first", "pane": pane + "Privacy_AllFiles"],
                ["status": "granted"]],
            "automation": [["name": "Notes", "bundle_id": "com.apple.Notes", "status": "not_determined"], ["name": "x"]],
            "automation_pane": pane + "Privacy_Automation",
            "grant_target": "/Applications/FERB.app/Contents/Helpers/FERB.app",
            "app_panes": ["notifications": "x-apple.systempreferences:com.apple.Notifications-Settings.extension?id=com.ferb.FERB"],
            "setup_done": true]
        let snapshot = PermissionSnapshot(reply: reply)!
        XCTAssertEqual(snapshot.rows.map(\.id), ["accessibility", "full_disk_access"])
        XCTAssertEqual(snapshot.rows[0].status, .denied)
        XCTAssertEqual(snapshot.rows[0].process, "daemon")
        XCTAssertTrue(snapshot.rows[0].requestable && snapshot.rows[0].required)
        XCTAssertEqual(snapshot.rows[0].pane?.absoluteString, pane + "Privacy_Accessibility")
        XCTAssertFalse(snapshot.rows[1].blocked.isEmpty)
        XCTAssertTrue(snapshot.rows[1].restartToCheck)
        XCTAssertEqual(snapshot.automation, [AutomationApp(name: "Notes", bundleID: "com.apple.Notes", status: .notDetermined)])
        XCTAssertEqual(snapshot.automationPane?.absoluteString, pane + "Privacy_Automation")
        XCTAssertEqual(snapshot.grantTarget, "/Applications/FERB.app/Contents/Helpers/FERB.app")
        XCTAssertNotNil(snapshot.appPanes["notifications"])
        XCTAssertTrue(snapshot.setupDone)
        XCTAssertNil(PermissionSnapshot(reply: ["ok": true]))
    }

    func testStatusesFERBDoesntKnowReadAsUnknown() {
        XCTAssertEqual(GrantStatus(daemon: "maybe"), .unknown)
        XCTAssertEqual(GrantStatus(daemon: nil), .unknown)
        XCTAssertEqual(GrantStatus(daemon: "target_not_running"), .targetNotRunning)
    }

    func testMicrophoneRowNeedsBothMicrophoneAndSpeech() {
        XCTAssertEqual(GrantStatus.both(.granted, .granted), .granted)
        XCTAssertEqual(GrantStatus.both(.granted, .denied), .denied)
        XCTAssertEqual(GrantStatus.both(.notDetermined, .granted), .notDetermined)
        XCTAssertEqual(GrantStatus.both(.unknown, .granted), .unknown)
    }

    func testBrowserReplyNamesEachProfilesState() {
        let snapshot = BrowserSnapshot(reply: [
            "store_url": "https://chromewebstore.google.com/detail/plbfkifcmgkgnlfgikdeaaglgncdhkma",
            "extension_dir": "/Users/o/.ferb/extensions/chromium",
            "browsers": [["browser": "chrome", "label": "Google Chrome", "requested": true, "manual": false,
                          "unconfirmed": "", "profiles": [
                            ["folder": "Default", "name": "Personal", "state": "on", "ferb_identity": false, "connected": false],
                            ["folder": "Profile 1", "name": "Work", "state": "not_installed"],
                            ["folder": "Profile 2", "name": "Odd", "state": "sideways"]]],
                         ["label": "no id"]]])!
        XCTAssertEqual(snapshot.browsers.map(\.browser), ["chrome"])
        XCTAssertEqual(snapshot.browsers[0].profiles.map(\.state), [.on, .notInstalled])
        XCTAssertTrue(snapshot.browsers[0].profiles[1].state.canAdd)
        XCTAssertFalse(snapshot.browsers[0].profiles[0].state.canAdd)
        XCTAssertEqual(snapshot.browsers[0].profiles[1].id, "chrome/Profile 1")
        XCTAssertNotNil(snapshot.storeURL)
        XCTAssertNil(BrowserSnapshot(reply: [:]))
    }

    func testAIReplyKnowsTheRuntimeEveryRoleUses() {
        let reply: [String: Any] = ["roles": ["orchestrator": "codex", "worker": "codex", "checker": "codex"],
                                    "runtimes": [["name": "codex", "usable": true, "fix": "", "model": "gpt-5.6-sol"],
                                                 ["name": "claude", "usable": false, "fix": "x", "model": ""]]]
        let snapshot = AISnapshot(reply: reply)!
        XCTAssertEqual(snapshot.current, "codex")
        XCTAssertTrue(snapshot.usable("codex"))
        XCTAssertFalse(snapshot.usable("claude"))
        XCTAssertFalse(snapshot.usable("google"))
        let mixed = AISnapshot(reply: ["roles": ["orchestrator": "codex", "worker": "claude", "checker": "claude"],
                                       "runtimes": [[String: Any]]()])!
        XCTAssertNil(mixed.current)
        XCTAssertEqual(AISnapshot.label("codex"), "ChatGPT (Codex)")
    }

    func testPersonaStatesFromTheDaemon() {
        XCTAssertEqual(PersonaState(reply: ["state": "scanning"]), .scanning)
        XCTAssertEqual(PersonaState(reply: ["state": "not_started"]), .loading)
        XCTAssertEqual(PersonaState(reply: ["state": "reviewed"]), .finished)
        XCTAssertEqual(PersonaState(reply: ["state": "unavailable"]), .finished)
        XCTAssertEqual(PersonaState(reply: ["state": "failed", "error": "OSError: x"]), .failed("OSError: x"))
        let pending = PersonaState(reply: ["state": "pending", "approval": [
            "approval_id": "p1", "reason": "Confirm or edit your app-use profile.", "proposal": "You code a lot.",
            "options": ["Save app-use profile", "Skip"]]])
        XCTAssertEqual(pending, .pending(approvalID: "p1", reason: "Confirm or edit your app-use profile.",
                                         proposal: "You code a lot.", canSave: true))
        let skipOnly = PersonaState(reply: ["state": "pending", "approval": ["approval_id": "p2", "options": ["Skip"]]])
        XCTAssertEqual(skipOnly, .pending(approvalID: "p2", reason: "", proposal: "", canSave: false))
        XCTAssertEqual(PersonaState(reply: ["state": "pending"]), .scanning)      // the question is being asked
    }

    func testAPIKeyLineCarriesTheKeyAsOneWordOrNothing() {
        XCTAssertEqual(APIKeyEntry.command(provider: .openai, key: "sk-proj-AbC_123"),
                       "add-generic-password -U -s ferb -a api/openai -w sk-proj-AbC_123\n")
        XCTAssertEqual(APIKeyEntry.account(.google), "api/google")
        for bad in ["", "two words", "quote\"d", "it's", "back\\slash", "tab\tkey", "ключ", "line\nbreak"] {
            XCTAssertNil(APIKeyEntry.command(provider: .anthropic, key: bad), bad)
        }
    }

    func testLinesArriveWholeAndTheLastOneOnFinish() {
        var lines = LineSplitter()
        XCTAssertEqual(lines.append("Setting up FERB's back"), [])
        XCTAssertEqual(lines.append("ground service…\n\n  Done\nferb: no"), ["Setting up FERB's background service…", "Done"])
        XCTAssertEqual(lines.finish(), "ferb: no")
        XCTAssertNil(lines.finish())
    }

    func testRestartKickstartsThenLoadsAnInstalledAgent() {
        let plist = "/Users/o/Library/LaunchAgents/com.ferb.daemon.plist"
        XCTAssertEqual(LaunchctlPlan.restart(uid: 501, plist: plist, plistExists: true),
                       [["kickstart", "-k", "gui/501/com.ferb.daemon"], ["bootstrap", "gui/501", plist]])
        XCTAssertEqual(LaunchctlPlan.restart(uid: 501, plist: plist, plistExists: false),
                       [["kickstart", "-k", "gui/501/com.ferb.daemon"]])
    }

    func testEveryRowHasWordsForTheOwner() {
        for id in ["accessibility", "screen_recording", "full_disk_access", "input_monitoring", "clipboard",
                   "notifications", "microphone", "hearing"] {
            XCTAssertNotEqual(PermissionCopy.label(id), id)
            XCTAssertFalse(PermissionCopy.reason(id).isEmpty)
        }
        XCTAssertEqual(PermissionCopy.status(.granted), "On")
    }
}
