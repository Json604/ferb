import XCTest
@testable import FERBNotch
@testable import NotchCore

/// FERB.app's setup window (L34 C): what each step reads and does, with the daemon, the bundled command, macOS and the
/// Keychain all faked.
@MainActor
final class SetupModelTests: XCTestCase {
    func testWelcomeInstallsTheServiceThenShowsChooseYourAI() async {
        let rig = SetupRig()
        let model = rig.model(.install)
        XCTAssertEqual(model.primaryTitle, "Set Up FERB")
        await model.primary()
        XCTAssertEqual(rig.runner.runs, [["setup", "--service"]])
        XCTAssertTrue(model.serviceRunning && model.primaryDisabled)
        rig.runner.say("Setting up FERB's background service…")
        XCTAssertEqual(model.serviceLine, "Setting up FERB's background service…")
        rig.runner.finish(0)
        XCTAssertFalse(model.serviceRunning)
        XCTAssertEqual(model.step, .ai)
        XCTAssertEqual(model.serviceAction, .none)              // going back to Welcome never reinstalls
    }

    func testAFailedServiceRunShowsItsLastLinesAndStaysOnWelcome() async {
        let rig = SetupRig()
        let model = rig.model(.install)
        await model.primary()
        rig.runner.say("Setting up FERB's background service…")
        rig.runner.say("ferb: FERB is running from its disk image: drag FERB.app to Applications, then open it from there")
        rig.runner.finish(1)
        XCTAssertEqual(model.step, .welcome)
        XCTAssertTrue(model.serviceError?.contains("drag FERB.app to Applications") == true)
        XCTAssertEqual(model.primaryTitle, "Try Again")
        await model.primary()
        XCTAssertEqual(rig.runner.runs.count, 2)
        XCTAssertNil(model.serviceError)
    }

    func testAnInstalledServiceIsNeverReinstalledFromWelcome() async {
        let rig = SetupRig()
        let model = rig.model(.none)
        XCTAssertEqual(model.primaryTitle, "Continue")
        await model.primary()
        XCTAssertTrue(rig.runner.runs.isEmpty)
        XCTAssertEqual(model.step, .ai)
    }

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
        XCTAssertEqual(rig.app.requests, ["notifications"])
        XCTAssertTrue(rig.opener.opened.isEmpty)
        await model.grant(model.allRows.first { $0.id == "microphone" }!)
        XCTAssertEqual(rig.app.requests, ["notifications"])
        XCTAssertEqual(rig.opener.opened.last?.absoluteString, SetupReplies.pane + "Privacy_SpeechRecognition")
        XCTAssertEqual(model.allRows.first { $0.id == "notifications" }?.status, .granted)
    }

    func testHearingIsProbedOnEnteringTheStepAndAskedThroughFERBVoice() async {
        let rig = SetupRig()
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(hearing: "not_determined")
        let model = rig.model()
        await model.enter(.permissions)
        XCTAssertEqual(rig.rpc.params(of: "setup.permissions").first?["hearing"] as? Bool, true)
        XCTAssertEqual(model.hearingRow?.process, "voice")
        XCTAssertEqual(model.allRows.filter { $0.id == "hearing" }.count, 1)
        await model.refreshPermissions()                         // a poll: no probe, the row stays
        XCTAssertNil(rig.rpc.params(of: "setup.permissions").last?["hearing"])
        XCTAssertNotNil(model.hearingRow)
        await model.grant(model.hearingRow!)
        XCTAssertEqual(rig.rpc.params(of: "setup.permission_request").last?["permission"] as? String, "hearing")
    }

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

    func testAKeyTheKeychainLineCantCarryIsRefusedBeforeAnythingRuns() async {
        let rig = SetupRig()
        let model = rig.model()
        await model.saveAPIKey(.anthropic, key: "two words")
        XCTAssertTrue(rig.keys.providers.isEmpty)
        XCTAssertTrue(rig.rpc.calls.isEmpty)
    }

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

    func testUseClaudeChoosesItAndSaysWhetherItAnswered() async {
        let rig = SetupRig()
        rig.rpc.replies["runtimes.check"] = ["ok": false, "detail": "not logged in"]
        let model = rig.model()
        await model.useClaude()
        XCTAssertEqual(rig.rpc.params(of: "runtimes.use").first?["runtime"] as? String, "claude")
        XCTAssertEqual(model.aiMessage, "Claude didn't answer: not logged in")
    }

    func testPersonaSaveEditAndSkipAnswerTheOneQuestion() async {
        let pending: [String: Any] = ["state": "pending", "approval": [
            "approval_id": "p1", "reason": "Confirm or edit your app-use profile.", "proposal": "You code a lot.",
            "options": ["Save app-use profile", "Skip"]]]
        for action in ["save", "edit", "skip"] {
            let rig = SetupRig()
            rig.rpc.replies["setup.persona"] = pending
            let model = rig.model()
            await model.refreshPersona()
            switch action {
            case "save":
                await model.savePersona()
                let p = rig.rpc.params(of: "chat.approve").first
                XCTAssertEqual(p?["approval_id"] as? String, "p1")
                XCTAssertEqual(p?["granted"] as? Bool, true)
                XCTAssertEqual(p?["choice"] as? Int, 1)
            case "edit":
                await model.editPersona("I write Python and edit video")
                XCTAssertEqual(rig.rpc.params(of: "chat.answer").first?["text"] as? String, "I write Python and edit video")
            default:
                await model.skipPersona()
                XCTAssertEqual(rig.rpc.params(of: "chat.approve").first?["granted"] as? Bool, false)
            }
            XCTAssertEqual(model.persona, .finished, action)
        }
    }

    func testAPersonaAnswerFERBRefusesKeepsTheQuestionAndSaysWhy() async {
        let rig = SetupRig()
        rig.rpc.replies["setup.persona"] = ["state": "pending", "approval": ["approval_id": "p1", "options": ["Skip"]]]
        rig.rpc.failures["chat.answer"] = DaemonRPCError.remote("There is no app-use evidence to confirm.")
        let model = rig.model()
        await model.refreshPersona()
        await model.editPersona("yes")
        XCTAssertEqual(model.personaMessage, "There is no app-use evidence to confirm.")
        XCTAssertEqual(model.persona, .pending(approvalID: "p1", reason: "", proposal: "", canSave: false))
    }

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

    func testPollingThroughARestartKeepsTheStepAndSaysFERBIsAway() async {
        let rig = SetupRig()
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions()
        let model = rig.model()
        model.step = .permissions
        await model.refreshPermissions()
        rig.rpc.failures["setup.permissions"] = DaemonRPCError.unavailable
        await model.refreshPermissions()
        XCTAssertEqual(model.step, .permissions)
        XCTAssertEqual(model.message, SetupModel.notAnswering)
        XCTAssertEqual(model.permissions?.rows.count, 4)          // the last rows stay on screen
        rig.rpc.failures["setup.permissions"] = nil
        await model.refreshPermissions()
        XCTAssertNil(model.message)
    }

    func testDoneRecordsSetupAndOpensTheNotch() async {
        let rig = SetupRig()
        let model = rig.model()
        var opened = false
        model.onFinish = { opened = true }
        model.step = .done
        XCTAssertEqual(model.primaryTitle, "Open FERB")
        await model.primary()
        XCTAssertTrue(rig.rpc.methods().contains("setup.finish"))
        XCTAssertTrue(opened)
    }

    func testLaunchKnowsSetupIsDoneFromTheDaemon() async {
        let rig = SetupRig()
        let model = rig.model()
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(setupDone: true)
        let done = await model.setupDone(attempts: 1, pause: .zero)
        XCTAssertTrue(done)
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(setupDone: false)
        let notDone = await model.setupDone(attempts: 1, pause: .zero)
        XCTAssertFalse(notDone)
        rig.rpc.failures["setup.permissions"] = DaemonRPCError.unavailable
        let away = await model.setupDone(attempts: 2, pause: .zero)
        XCTAssertTrue(away)                                      // FERB isn't answering: never nag at launch
    }

    func testAQuietUpdateOpensPermissionsOnlyWhenARequiredGrantIsMissing() async {
        let rig = SetupRig()
        let model = rig.model()
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(accessibility: "granted", fda: "granted")
        let fine = await model.needsPermissions()
        XCTAssertFalse(fine)
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(accessibility: "denied", fda: "granted")
        let missing = await model.needsPermissions()
        XCTAssertTrue(missing)
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(accessibility: "granted", fda: "denied",
                                                                        fdaBlocked: "not yet")
        let blocked = await model.needsPermissions()
        XCTAssertFalse(blocked)                                   // a blocked row can't be granted yet
    }

    func testAnExistingInstallIsAskedForNotificationsOnceWhileMacOSHasntAsked() async {
        // owner decision 6: set up before this window existed, so its Permissions step never offered Notifications
        let rig = SetupRig()
        let model = rig.model()
        let defaults = UserDefaults(suiteName: "SetupModelTests-" + UUID().uuidString)!
        let asked = await model.askForNotificationsOnce(defaults: defaults)
        XCTAssertTrue(asked)
        XCTAssertEqual(rig.app.requests, ["notifications"])
        rig.app.notificationStatus = .notDetermined               // still undecided (dismissed): never asked again
        let again = await model.askForNotificationsOnce(defaults: defaults)
        XCTAssertFalse(again)
        XCTAssertEqual(rig.app.requests, ["notifications"])
        let decided = SetupRig()
        decided.app.notificationStatus = .denied                  // the owner already answered macOS: no question
        let fresh = UserDefaults(suiteName: "SetupModelTests-" + UUID().uuidString)!
        let none = await decided.model().askForNotificationsOnce(defaults: fresh)
        XCTAssertFalse(none)
        XCTAssertTrue(decided.app.requests.isEmpty)
    }
}
