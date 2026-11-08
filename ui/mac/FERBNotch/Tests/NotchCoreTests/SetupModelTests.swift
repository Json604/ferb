import AppKit
import SwiftUI
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// FERB.app's setup (L34 C), for the tour and Settings: what each step reads and does, with the daemon, the bundled
/// command, macOS and the Keychain all faked.
@MainActor
final class SetupModelTests: XCTestCase {
    private func runService(_ model: SetupModel) -> () -> Bool? {
        var result: Bool?
        model.runService { result = $0 }
        return { result }
    }

    func testTheServiceRunInstallsItWithItsLiveLine() async {
        let rig = SetupRig()
        let model = rig.model(.install)
        let done = runService(model)
        XCTAssertEqual(rig.runner.runs, [["setup", "--service"]])
        XCTAssertTrue(model.serviceRunning)
        rig.runner.say("Setting up FERB's background service…")
        XCTAssertEqual(model.serviceLine, "Setting up FERB's background service…")
        rig.runner.finish(0)
        XCTAssertFalse(model.serviceRunning)
        XCTAssertEqual(done(), true)
        XCTAssertEqual(model.serviceAction, .none)              // a later run never reinstalls
        _ = runService(model)
        XCTAssertEqual(rig.runner.runs.count, 1)
    }

    /// DMN-513 (live 2026-11-02: a 2026-11 build's SMAppService registration carried that build's launch constraint, so
    /// macOS killed every later build's daemon). Setup unregisters a registration macOS still lists, once, before
    /// `ferb setup --service` writes and loads the LaunchAgent; nothing is registered.
    // decision: DMN-513, APP-602
    func testARegistrationLeftByA2026_11BuildIsUnregisteredOnceBeforeSetup() async {
        for listed in [ServiceStatus.enabled, .requiresApproval] {
            let rig = SetupRig()
            let background = FakeBackgroundService()
            background.registration = listed
            let model = rig.model(.update, background: background)
            _ = runService(model)
            await eventually { rig.runner.isRunning }
            XCTAssertEqual(background.unregistrations, 1)            // before the command runs
            XCTAssertEqual(rig.runner.runs, [["setup", "--service", "--update"]])
            rig.runner.finish(0)
            XCTAssertNil(model.serviceError)
            XCTAssertEqual(model.serviceAction, .none)
            model.serviceAction = .update
            _ = runService(model)                                    // a later run: nothing listed, nothing to undo
            XCTAssertEqual(background.unregistrations, 1)
            rig.runner.finish(0)
        }
    }

    func testANewInstallWithNoRegistrationRunsTheCommandAtOnce() async {
        let rig = SetupRig()
        let background = FakeBackgroundService()
        let model = rig.model(.install, background: background)
        _ = runService(model)
        XCTAssertEqual(rig.runner.runs, [["setup", "--service"]])  // no wait on macOS first
        rig.runner.finish(0)
        XCTAssertEqual(background.unregistrations, 0)
    }

    /// FERB switched off under Login Items → Allow in the Background: the command says so (its exit status), and the
    /// tour shows where to switch it on, never a crash loop.
    // decision: DMN-513, APP-602
    func testAServiceSwitchedOffInLoginItemsSaysWhere() async {
        let rig = SetupRig()
        let background = FakeBackgroundService()
        background.off = true
        let model = rig.model(.update, background: background)
        let done = runService(model)
        rig.runner.say("FERB's background service is switched off: System Settings → General → Login Items → …")
        rig.runner.finish(ServicePlan.switchedOffStatus)
        XCTAssertFalse(model.serviceRunning)
        XCTAssertEqual(done(), false)
        XCTAssertEqual(model.serviceError, ServicePlan.approval)
        XCTAssertTrue(model.serviceNeedsApproval)
        XCTAssertEqual(rig.runner.runs.count, 1)                    // nothing runs again by itself
        model.openLoginItems()
        XCTAssertEqual(background.openedSettings, 1)
        XCTAssertEqual(model.serviceAction, .update)                // Try again sets it up once the owner allowed it
    }

    // decision: APP-685
    func testAFailedServiceRunKeepsItsLastLinesForTheTour() async {
        let rig = SetupRig()
        let model = rig.model(.install)
        let done = runService(model)
        rig.runner.say("Setting up FERB's background service…")
        rig.runner.say("ferb: FERB is running from its disk image: drag FERB.app to Applications, then open it from there")
        rig.runner.finish(1)
        XCTAssertEqual(done(), false)
        XCTAssertTrue(model.serviceError?.contains("drag FERB.app to Applications") == true)
        _ = runService(model)                                       // Try again
        XCTAssertEqual(rig.runner.runs.count, 2)
        XCTAssertNil(model.serviceError)
    }

    // decision: APP-510
    func testGrantAsksTheDaemonForTheRequestThenOpensThatPane() async {
        let rig = SetupRig()
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions()
        let model = rig.model()
        await model.refreshPermissions()
        await model.grant(model.allRows.first { $0.id == "accessibility" }!)
        XCTAssertEqual(rig.rpc.params(of: "setup.permission_request").first?["permission"] as? String, "accessibility")
        XCTAssertEqual(rig.opener.opened.map(\.absoluteString), [SetupReplies.pane + "Privacy_Accessibility"])
        XCTAssertTrue(rig.opener.revealed.isEmpty)
        XCTAssertFalse(model.offersRestart)                      // Accessibility applies without a restart
    }

    // decision: APP-510
    func testFullDiskAccessShowsTheAgentAppAndOffersARestartToCheck() async {
        let rig = SetupRig()
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions()
        let model = rig.model()
        await model.refreshPermissions()
        await model.grant(model.allRows.first { $0.id == "full_disk_access" }!)
        XCTAssertTrue(rig.rpc.params(of: "setup.permission_request").isEmpty)   // no API: the owner adds FERB
        XCTAssertEqual(rig.opener.revealed, [SetupReplies.agent])
        XCTAssertEqual(rig.opener.opened.map(\.absoluteString), [SetupReplies.pane + "Privacy_AllFiles"])
        XCTAssertTrue(model.offersRestart && model.showsFullDiskHint)
        await model.restartService()
        XCTAssertEqual(rig.service.restarts, 1)
        XCTAssertFalse(model.offersRestart)
    }

    func testFullDiskAccessBeforeContainmentDoesNothing() async {
        let rig = SetupRig()
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(
            fdaBlocked: "FERB's shell sandbox and network proxy must be working first")
        let model = rig.model()
        await model.refreshPermissions()
        let calls = rig.rpc.calls.count
        await model.grant(model.allRows.first { $0.id == "full_disk_access" }!)
        XCTAssertEqual(rig.rpc.calls.count, calls)
        XCTAssertTrue(rig.opener.opened.isEmpty && rig.opener.revealed.isEmpty)
    }

    // decision: APP-510, APP-401
    func testFERBAppsOwnRowsAskMacOSWhenUndecidedAndOpenThePaneWhenOff() async {
        let rig = SetupRig()
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions()
        rig.app.microphoneStatus = .denied
        rig.app.microphoneDeniedPane = "speech_recognition"
        let model = rig.model()
        await model.refreshPermissions()
        let notifications = model.allRows.first { $0.id == "notifications" }!
        XCTAssertEqual(notifications.process, "app")
        await model.grant(notifications)
        await eventually { rig.app.requests == ["notifications"] }       // asked in the background: never awaited
        XCTAssertTrue(rig.opener.opened.isEmpty)
        await model.grant(model.allRows.first { $0.id == "microphone" }!)
        XCTAssertEqual(rig.app.requests, ["notifications"])
        XCTAssertEqual(rig.opener.opened.last?.absoluteString, SetupReplies.pane + "Privacy_SpeechRecognition")
        await eventually { model.allRows.first { $0.id == "notifications" }?.status == .granted }
    }

    // decision: APP-686
    func testAllowAllNeverWaitsOnTheNotificationBanner() async {
        // A friend's Allow all stalled on macOS's notification banner (2026-10-27): it is asked, hinted, never awaited
        let rig = SetupRig()
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(accessibility: "granted", fda: "granted")
        rig.app.microphoneStatus = .granted
        rig.app.notificationsHang = true
        let model = rig.model()
        await model.refreshPermissions()
        XCTAssertEqual(SetupModel.allowAllOrder(model.setupRows), ["notifications"])
        await model.allowQuickPermissions()                                          // returns though the banner is unanswered
        XCTAssertTrue(model.notificationHint && !model.allowingAll)
        XCTAssertEqual(SetupModel.allowAllSequence.suffix(2), ["notifications", "screen_recording"])
    }

    // decision: APP-510
    func testComingBackFromSystemSettingsRestartsOnceToCheckTheSwitch() async {
        // A friend's Screen Recording never ticked until the service restarted (2026-10-27)
        let rig = SetupRig()
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(screen: "denied")
        let model = rig.model()
        await model.refreshPermissions()
        await model.cameBack()
        XCTAssertEqual(rig.service.restarts, 0)                         // nothing asked: no restart
        await model.grant(model.allRows.first { $0.id == "screen_recording" }!)
        await model.cameBack()
        XCTAssertEqual(rig.service.restarts, 1)
        await model.cameBack()
        XCTAssertEqual(rig.service.restarts, 1)                         // once per Grant
        rig.rpc.replies["chat.status"] = ["running": [["task_id": "t1", "goal": "work"]]]
        await model.grant(model.allRows.first { $0.id == "screen_recording" }!)
        await model.cameBack()
        XCTAssertEqual(rig.service.restarts, 1)                         // never while a task runs
        XCTAssertTrue(model.offersRestart)                              // the button stays for that
    }

    /// Live 2026-11-04: the owner's Screen Recording switch was on, but the row ticked only after they left System
    /// Settings (the daemon caches the grant until it restarts). A grant's wait asks for a fresh reading of its row; the
    /// moment it reads on there, the service restarts at once (once), and the row ticks after the restart.
    // decision: APP-662
    func testASwitchTurnedOnRestartsTheServiceAtOnceWithoutLeavingSettings() async {
        let rig = SetupRig()
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(screen: "denied")
        let model = rig.model()
        model.frontmostApp = { SettingsAnchor.bundleID }                   // the owner stays in System Settings
        await model.refreshPermissions()
        XCTAssertNil(rig.rpc.params(of: "setup.permissions").first?["fresh"])   // nobody waiting: no fresh reading
        let row = model.allRows.first { $0.id == "screen_recording" }!
        rig.service.onRestart = {                                           // the restarted daemon reads it on
            rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(screen: "granted")
        }
        let waiting = Task { await model.grantAndConfirm(row, polls: 400) }
        await eventually { rig.rpc.params(of: "setup.permissions").contains { $0["fresh"] as? [String] == ["screen_recording"] } }
        XCTAssertEqual(rig.service.restarts, 0)                              // still off everywhere: no restart
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(screen: "denied", screenRestartNeeded: true)
        let granted = await waiting.value
        XCTAssertTrue(granted)
        XCTAssertEqual(rig.service.restarts, 1)
    }

    // decision: APP-510
    func testARowStillOffAfterItsGrantOffersFERBToDragIn() async {
        // A friend's newer macOS didn't list FERB in its pane (2026-10-27): the agent app can be dragged in
        let rig = SetupRig()
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions()
        let model = rig.model()
        await model.refreshPermissions()
        XCTAssertTrue(model.askedRows.isEmpty)
        await model.grant(model.allRows.first { $0.id == "accessibility" }!)
        XCTAssertEqual(model.askedRows, ["accessibility"])
        model.showAgentApp()
        XCTAssertEqual(rig.opener.revealed, [SetupReplies.agent])
    }

    // decision: APP-401, APP-536
    func testSetupPermissionsHaveNoSeparateHearingRow() async {
        // Hearing is the same Speech Recognition grant as dictation: the microphone row covers both
        XCTAssertEqual(SetupModel.askedAtSetup.count, 7)
        XCTAssertEqual(SetupModel.askedAtSetup, ["accessibility", "screen_recording", "full_disk_access",
                                                 "input_monitoring", "notifications", "microphone", "clipboard"])
        XCTAssertFalse(SetupModel.allowAllSequence.contains("hearing"))
        let rig = SetupRig()
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions()
        let model = rig.model()
        await model.refreshPermissions()
        XCTAssertNil(rig.rpc.params(of: "setup.permissions").first?["hearing"])   // no probe of a second app
        XCTAssertTrue(model.allRows.filter { $0.id == "hearing" }.isEmpty)
    }

    // decision: APP-508
    func testAutomationAllowAsksMacOSThroughTheDaemonOrOpensThePaneWhenOff() async {
        let rig = SetupRig()
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions()
        let model = rig.model()
        await model.refreshPermissions()
        let apps = model.permissions!.automation
        await model.allowAutomation(apps.first { $0.bundleID == "com.apple.mail" }!)
        XCTAssertEqual(rig.rpc.params(of: "automation.request").first?["bundle_ids"] as? [String], ["com.apple.mail"])
        await model.allowAutomation(apps.first { $0.bundleID == "com.apple.Notes" }!)   // off: macOS won't ask again
        XCTAssertEqual(rig.rpc.params(of: "automation.request").count, 1)
        XCTAssertEqual(rig.opener.opened.last?.absoluteString, SetupReplies.pane + "Privacy_Automation")
    }

    // decision: APP-507
    func testAPIKeyGoesOnlyToTheKeychainThenThatServiceIsChosenAndChecked() async {
        let rig = SetupRig()
        rig.rpc.replies["runtimes.check"] = ["ok": true, "detail": "answered"]
        let model = rig.model()
        let secret = "sk-test-0123456789abcdef"
        await model.saveAPIKey(.openai, key: secret)
        XCTAssertEqual(rig.keys.providers, [.openai])
        XCTAssertEqual(rig.rpc.params(of: "runtimes.use").first?["runtime"] as? String, "openai")
        XCTAssertEqual(rig.rpc.params(of: "runtimes.check").first?["runtime"] as? String, "openai")
        for (_, params) in rig.rpc.calls { XCTAssertFalse(String(describing: params).contains(secret)) }
        let state = Mirror(reflecting: model).children.map { String(describing: $0.value) }.joined()
        XCTAssertFalse(state.contains(secret))
        XCTAssertEqual(model.aiMessage, "your OpenAI API key answered. FERB uses it now.")
    }

    // decision: APP-593
    func testAKeyTheKeychainLineCantCarryIsRefusedBeforeAnythingRuns() async {
        let rig = SetupRig()
        let model = rig.model()
        await model.saveAPIKey(.anthropic, key: "two words")
        XCTAssertTrue(rig.keys.providers.isEmpty)
        XCTAssertTrue(rig.rpc.calls.isEmpty)
    }

    // decision: APP-507
    func testSignInWithChatGPTRunsFERBsOwnCodexLoginThenChoosesCodex() async {
        let rig = SetupRig()
        let model = rig.model()
        model.signInWithChatGPT()
        XCTAssertEqual(rig.runner.runs, [["setup", "--codex-login"]])
        XCTAssertTrue(model.signingIn)
        rig.runner.finish(0)
        XCTAssertFalse(model.signingIn)
        await eventually { rig.rpc.methods().contains("runtimes.use") }
        XCTAssertEqual(rig.rpc.params(of: "runtimes.use").first?["runtime"] as? String, "codex")
        XCTAssertTrue(rig.rpc.params(of: "runtimes.check").isEmpty)    // signing in already proved it
    }

    // decision: APP-507, APP-792
    func testUseClaudeChecksItFirstAndKeepsTheServiceInUseWhenItDoesntAnswer() async {
        let rig = SetupRig()
        rig.rpc.replies["runtimes.check"] = ["ok": false, "detail": "not logged in"]
        let model = rig.model()
        await model.useClaude()
        XCTAssertEqual(rig.rpc.params(of: "runtimes.check").first?["runtime"] as? String, "claude")
        XCTAssertTrue(rig.rpc.params(of: "runtimes.use").isEmpty)
        XCTAssertEqual(model.aiMessage, "Claude didn't answer, so FERB keeps the service it uses: not logged in")
    }

    // decision: APP-792
    /// Owner 2026-11-08: "when i chagen from anthropic or anu api key i cannot swithc bacj to it". After switching to
    /// ChatGPT, the saved Anthropic key still has its card and its Use, which switches back without any key.
    func testASavedKeyKeepsItsCardAndUseSwitchesBackWithoutTheKey() async {
        let rig = SetupRig()
        rig.rpc.replies["runtimes.status"] = [
            "roles": ["orchestrator": "codex", "worker": "codex", "checker": "codex"],
            "runtimes": [["name": "codex", "usable": true, "signed_in": true, "model": "gpt-5.6-sol"],
                         ["name": "anthropic", "usable": true, "signed_in": true, "model": "claude-opus-5-5"],
                         ["name": "openai", "usable": false, "signed_in": false, "model": "gpt-6.1-sol"]]]
        rig.rpc.replies["runtimes.check"] = ["ok": true, "detail": "answered"]
        let model = rig.model()
        await model.refreshAI()
        XCTAssertEqual(model.ai?.savedKeys, [.anthropic])
        XCTAssertEqual(model.ai?.unsavedKeys, [.openai, .google])
        XCTAssertTrue(model.ai?.inUse("codex") == true && model.ai?.inUse("anthropic") == false)
        await model.useAPIKey(.anthropic)
        XCTAssertEqual(rig.rpc.params(of: "runtimes.check").first?["runtime"] as? String, "anthropic")
        XCTAssertEqual(rig.rpc.params(of: "runtimes.use").first?["runtime"] as? String, "anthropic")
        XCTAssertTrue(rig.keys.providers.isEmpty)                         // no key entered again
        await model.removeAPIKey(.anthropic)
        XCTAssertEqual(rig.keys.removed, [.anthropic])
    }

    func testPersonaShowsWhatFERBSawWithNothingToAnswer() async {
        let rig = SetupRig()
        rig.rpc.replies["setup.persona"] = ["state": "seen", "summary": "Here's what I saw on your Mac: you code a lot."]
        let model = rig.model()
        await model.refreshPersona()
        XCTAssertEqual(model.persona, .seen(summary: "Here's what I saw on your Mac: you code a lot."))
        XCTAssertTrue(rig.rpc.params(of: "chat.approve").isEmpty && rig.rpc.params(of: "chat.answer").isEmpty)
    }

    // decision: APP-511
    func testAddToChromeAsksTheDaemonToOpenThatProfile() async {
        let rig = SetupRig()
        rig.rpc.replies["setup.browsers"] = ["store_url": NSNull(), "extension_dir": "/x", "browsers": [
            ["browser": "chrome", "label": "Google Chrome", "profiles": [["folder": "Profile 1", "name": "Work",
                                                                          "state": "not_installed"]]]]]
        let model = rig.model()
        await model.refreshBrowsers()
        await model.addExtension(model.browsers!.browsers[0].profiles[0])
        let p = rig.rpc.params(of: "setup.browser_open").first
        XCTAssertEqual(p?["browser"] as? String, "chrome")
        XCTAssertEqual(p?["folder"] as? String, "Profile 1")
    }

    /// Past bug (owner's question 2026-11-04): Pair all said nothing about profiles it passed over, so "Paired." showed
    /// when no browser had FERB's extension yet. Only a reply with every profile paired leaves no message.
    // decision: HAND-411
    func testPairAllNamesTheProfilesItDidNotPair() async {
        let rig = SetupRig()
        let model = rig.model()
        rig.rpc.replies["setup.browsers_pair_all"] = ["profiles": 2, "results": [[String: Any]](), "skipped": [
            ["name": "Work", "label": "Google Chrome", "reason": "requested", "step": "restart Google Chrome once"],
            ["name": "Home", "label": "Google Chrome", "reason": "off", "step": "turn FERB on"]]]
        await model.pairAll()
        XCTAssertNotNil(model.message)
        XCTAssertEqual(model.message?.split(separator: "\n").count, 2)
        rig.rpc.replies["setup.browsers_pair_all"] = ["profiles": 0, "results": [[String: Any]](),
                                                      "skipped": [[String: Any]]()]
        await model.pairAll()
        XCTAssertEqual(model.message, SetupModel.noBrowserProfiles)
        rig.rpc.replies["setup.browsers_pair_all"] = ["profiles": 1, "skipped": [[String: Any]](),
                                                      "results": [["name": "Work", "paired": true]]]
        await model.pairAll()
        XCTAssertNil(model.message)
    }

    // decision: DMN-508
    func testBrowsersSayWhatKeptFERBFromConnectingThem() async {
        let rig = SetupRig()
        rig.rpc.replies["setup.browsers"] = ["store_url": NSNull(), "extension_dir": "/x", "browsers": [],
                                             "waiting": "full_disk_access",
                                             "problem": "FERB can't look at your browsers until it has Full Disk Access"]
        let model = rig.model()
        await model.refreshBrowsers()
        XCTAssertEqual(model.message, "FERB can't look at your browsers until it has Full Disk Access")
        rig.rpc.replies["setup.browsers"] = ["store_url": NSNull(), "extension_dir": "/x", "browsers": []]
        model.message = nil
        await model.refreshBrowsers()
        XCTAssertNil(model.message)
    }

    private func aiReply(claude signIn: [String: Any], signedIn: Bool) -> [String: Any] {
        ["roles": ["orchestrator": "codex", "worker": "codex", "checker": "codex"],
         "runtimes": [["name": "codex", "usable": true, "fix": "", "model": "gpt-5.6-sol", "signed_in": true],
                      ["name": "claude", "usable": true, "fix": "", "model": "", "signed_in": signedIn,
                       "sign_in": signIn]]]
    }

    func testSignInWithClaudeRunsTheDaemonsSignInThenChoosesClaude() async {
        let rig = SetupRig()
        rig.rpc.replies["runtimes.sign_in"] = ["state": "waiting", "url": "", "why": ""]
        rig.rpc.replies["runtimes.status"] = aiReply(claude: ["state": "waiting",
                                                              "url": "https://claude.ai/oauth/authorize?x=1", "why": ""],
                                                     signedIn: false)
        rig.rpc.replies["runtimes.check"] = ["ok": true, "detail": "answered"]
        let model = rig.model()
        await model.signInWithClaude()
        XCTAssertEqual(rig.rpc.params(of: "runtimes.sign_in").first?["runtime"] as? String, "claude")
        XCTAssertTrue(model.claudeSigningIn)
        model.openClaudeSignInPage()
        XCTAssertEqual(rig.opener.opened.last?.host, "claude.ai")
        await model.sendClaudeCode("  fake-auth-code-0123456789 \n")
        XCTAssertEqual(rig.rpc.params(of: "runtimes.sign_in").last?["code"] as? String, "fake-auth-code-0123456789")
        let state = Mirror(reflecting: model).children.map { String(describing: $0.value) }.joined()
        XCTAssertFalse(state.contains("fake-auth-code-0123456789"))          // the code isn't kept in the window
        rig.rpc.replies["runtimes.status"] = aiReply(claude: ["state": "signed_in", "url": "", "why": ""], signedIn: true)
        await model.refreshAI()
        XCTAssertFalse(model.claudeSigningIn)
        XCTAssertEqual(rig.rpc.params(of: "runtimes.use").last?["runtime"] as? String, "claude")
        XCTAssertEqual(model.aiMessage, "Claude answered. FERB uses it now.")
    }

    func testAFailedClaudeSignInSaysWhy() async {
        let rig = SetupRig()
        let why = "Claude didn't confirm the sign-in. Try again."
        rig.rpc.replies["runtimes.sign_in"] = ["state": "waiting", "url": "", "why": ""]
        let model = rig.model()
        await model.signInWithClaude()
        rig.rpc.replies["runtimes.status"] = aiReply(claude: ["state": "failed", "url": "", "why": why], signedIn: false)
        await model.refreshAI()
        XCTAssertFalse(model.claudeSigningIn)
        XCTAssertEqual(model.aiMessage, why)
        XCTAssertTrue(rig.rpc.params(of: "runtimes.use").isEmpty)
    }

    func testCancelStopsTheClaudeSignIn() async {
        let rig = SetupRig()
        rig.rpc.replies["runtimes.sign_in"] = ["state": "waiting", "url": "", "why": ""]
        let model = rig.model()
        await model.signInWithClaude()
        await model.cancelClaudeSignIn()
        XCTAssertEqual(rig.rpc.params(of: "runtimes.sign_in").last?["cancel"] as? Bool, true)
        XCTAssertFalse(model.claudeSigningIn)
    }
}
