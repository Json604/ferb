import XCTest
@testable import FERBNotch
@testable import NotchCore

/// DMN-593 (owner, 2026-11-14): the Rust build runs its background service through FERB.app's own registration
/// (SMAppService) and in no other way. No LaunchAgent around a refusal, nothing moved from a self-signed install (its
/// service, login item or permissions); when macOS refuses, the owner reads why and how to allow it.
@MainActor
final class InAppServiceTests: XCTestCase {
    private let team = "team:TEAM123456"
    private let leaf = "leaf:0123456789abcdef0123456789abcdef01234567"

    private struct Refused: LocalizedError { var errorDescription: String? { "Operation not permitted" } }

    // decision: DMN-593, DMN-594
    func testEveryRustFERBAppRegistersInAppAndOnlyABuildWithoutATeamRegistersAfresh() throws {
        XCTAssertTrue(AppServicePlan.registersInApp(packaged: true, holdsPython: false))
        XCTAssertFalse(AppServicePlan.registersInApp(packaged: true, holdsPython: true))    // the Python build
        XCTAssertFalse(AppServicePlan.registersInApp(packaged: false, holdsPython: false))  // `swift run`
        XCTAssertFalse(AppServicePlan.registersAfresh(signer: team))   // a Developer ID registration is kept
        XCTAssertTrue(AppServicePlan.registersAfresh(signer: leaf))
        XCTAssertTrue(AppServicePlan.registersAfresh(signer: "adhoc"))
        XCTAssertEqual(Signer.words(team: "TEAM123456", leafSHA1: "AB"), team)
        XCTAssertEqual(Signer.words(team: nil, leafSHA1: "0123456789ABCDEF0123456789ABCDEF01234567"), leaf)
        XCTAssertEqual(Signer.words(team: nil, leafSHA1: nil), "adhoc")

        // read from what the bundle holds: its `ferb` program and agent, and no Python inside
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("in-app-\(UUID())")
        defer { try? FileManager.default.removeItem(at: root) }
        let app = root.appendingPathComponent("FERB.app")
        for part in ["Contents/Resources/bin/ferb", "Contents/Helpers/FERB.app/Contents/MacOS/FERB"] {
            let file = app.appendingPathComponent(part)
            try FileManager.default.createDirectory(at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
            try Data().write(to: file)
            try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: file.path)
        }
        XCTAssertTrue(InAppService.holds(app))
        try FileManager.default.createDirectory(at: app.appendingPathComponent("Contents/Resources/python"),
                                                withIntermediateDirectories: true)
        XCTAssertFalse(InAppService.holds(app))
        XCTAssertFalse(InAppService.holds(root))
    }

    // decision: DMN-593, DMN-594
    func testTheLaunchReadsTheBuildNeverTheOldSigner() {
        let defaults = scratchDefaults("InAppService")
        let setup = AppSetupController(model: SetupRig().model(), defaults: defaults)
        setup.inAppService = true
        setup.ownSigner = team
        setup.serviceAtLaunch()
        XCTAssertTrue(setup.model.appService)
        XCTAssertFalse(setup.model.registersAfresh)
        setup.ownSigner = "adhoc"                                     // a developer's local Rust build
        setup.serviceAtLaunch()
        XCTAssertTrue(setup.model.appService && setup.model.registersAfresh)
        setup.inAppService = false                                    // the Python build: its LaunchAgent
        setup.serviceAtLaunch()
        XCTAssertFalse(setup.model.appService || setup.model.registersAfresh)
        XCTAssertTrue(defaults.dictionaryRepresentation().keys.allSatisfy { !$0.hasPrefix("signerMove") })
    }

    // decision: DMN-593
    func testTheAppsRegistrationIsSetUpWhileListedAloneAndItsOwnAndAnythingElseIsSetUpAgain() {
        func decide(_ record: [String: Any]?, agent: [String: Any]? = nil, registration: ServiceStatus = .enabled,
                    running: Bool = true) -> AppSetupAction {
            AppSetupPlan.decide(launchAgent: agent, launchAgentDigest: agent == nil ? nil : "d1",
                                registration: registration, record: record,
                                agentExecutable: "/Applications/FERB.app/Contents/Helpers/FERB.app/Contents/MacOS/FERB",
                                appPath: "/Applications/FERB.app", build: "202612010900", daemonRunning: running,
                                appService: true)
        }
        let inApp: [String: Any] = ["build": "202612010900", "app": "/Applications/FERB.app", "service": "app",
                                    "signer": team]
        XCTAssertEqual(decide(inApp), .none)
        XCTAssertEqual(decide(inApp, registration: .notRegistered), .update)
        XCTAssertEqual(decide(inApp, running: false), .update)
        XCTAssertEqual(decide(inApp, agent: ["Label": "com.ferb.daemon", "ProgramArguments": ["/x", "daemon"]]),
                       .update)                                          // a LaunchAgent is left: setup retires it
        var selfSigned = inApp                                           // an older self-signed build's record: the
        selfSigned["service"] = "launch_agent"                           // same setup and registration, nothing moved
        selfSigned["signer"] = leaf
        XCTAssertEqual(decide(selfSigned), .update)
        XCTAssertEqual(decide(nil, registration: .notRegistered), .install)
        XCTAssertEqual(AppSetupPlan.serviceArguments(for: .update, appService: true),
                       ["setup", "--service", "--update", "--app-service"])
        XCTAssertEqual(AppSetupPlan.serviceArguments(for: .install, appService: true),
                       ["setup", "--service", "--app-service"])
    }

    // decision: DMN-593
    func testMacOSsAnswerIsWaitedOnGuidedOrReportedNeverWorkedAround() {
        XCTAssertEqual(AppServicePlan.afterRegister(error: nil, deniedByUser: false, status: .enabled), .waitForFERB)
        XCTAssertEqual(AppServicePlan.afterRegister(error: nil, deniedByUser: true, status: .notRegistered), .approval)
        XCTAssertEqual(AppServicePlan.afterRegister(error: nil, deniedByUser: false, status: .requiresApproval),
                       .approval)
        XCTAssertEqual(AppServicePlan.afterRegister(error: "Invalid signature", deniedByUser: false,
                                                    status: .notRegistered), .refused("Invalid signature"))
        guard case .refused = AppServicePlan.afterRegister(error: nil, deniedByUser: false, status: .notFound) else {
            return XCTFail("a status that stays off is a refusal")
        }
        // the reason, then the Login Items guidance in the words the approval already uses
        XCTAssertEqual(ServicePlan.approval, "macOS needs your OK to run FERB in the background: System Settings → " +
                       "General → Login Items → allow FERB.")
        let refusal = AppServicePlan.refusal("Invalid signature")
        XCTAssertTrue(refusal.hasPrefix("macOS didn't start FERB's background service (Invalid signature)."))
        XCTAssertTrue(refusal.contains(ServicePlan.loginItemsPlace))
        XCTAssertFalse(refusal.contains("\n"))                          // the tour's service line shows its last line
    }

    // decision: DMN-593
    func testARestartOfTheAppsRegistrationNeverLoadsAPlist() {
        XCTAssertEqual(LaunchctlPlan.restart(uid: 501, launchAgent: nil), [["kickstart", "-k", "gui/501/com.ferb.daemon"]])
        XCTAssertEqual(LaunchctlPlan.restart(uid: 501, launchAgent: "/Users/o/Library/LaunchAgents/com.ferb.daemon.plist"),
                       [["kickstart", "-k", "gui/501/com.ferb.daemon"],
                        ["bootstrap", "gui/501", "/Users/o/Library/LaunchAgents/com.ferb.daemon.plist"]])
    }

    private func appRig(_ background: FakeBackgroundService, afresh: Bool = false) -> (SetupRig, SetupModel) {
        let rig = SetupRig()
        let model = rig.model(.update, background: background)
        model.appService = true
        model.registersAfresh = afresh
        model.serviceStartWait = .milliseconds(50)
        model.serviceWaitPoll = .milliseconds(5)
        return (rig, model)
    }

    /// An update from one Developer ID build to the next, and the first Developer ID launch over anything an older
    /// build left: the command runs with --app-service, then the registration (kept, never unregistered) and the wait.
    // decision: DMN-593
    func testADeveloperIDBuildRegistersAndKeepsItsRegistration() async {
        let background = FakeBackgroundService()
        background.registration = .enabled
        background.status = .enabled
        let (rig, model) = appRig(background)
        var result: Bool?
        model.runService { result = $0 }
        await eventually { rig.runner.isRunning }
        XCTAssertTrue(background.steps.isEmpty)                      // nothing unregistered before the command
        XCTAssertEqual(rig.runner.runs, [["setup", "--service", "--update", "--app-service"]])
        rig.runner.finish(0)
        await eventually { result != nil }
        XCTAssertEqual(result, true)
        XCTAssertEqual(background.steps, ["register"])
        XCTAssertEqual(model.serviceAction, .none)
        XCTAssertNil(model.serviceError)
        XCTAssertTrue(rig.rpc.calls.contains { $0.0 == "ping" })     // registered, then waited for FERB
    }

    // decision: DMN-593, DMN-594
    func testABuildWithoutATeamRegistersAfresh() async {
        let background = FakeBackgroundService()
        background.registration = .enabled
        background.status = .enabled
        let (rig, model) = appRig(background, afresh: true)
        var result: Bool?
        model.runService { result = $0 }
        await eventually { rig.runner.isRunning }
        rig.runner.finish(0)
        await eventually { result != nil }
        XCTAssertEqual(result, true)
        XCTAssertEqual(background.steps, ["unregister", "register"])
        XCTAssertEqual(rig.runner.runs.count, 1)
    }

    // decision: DMN-593
    func testARefusedRegistrationSaysWhyAndHowToAllowItAndNothingRunsAroundIt() async {
        let background = FakeBackgroundService()
        background.registerError = Refused()
        let (rig, model) = appRig(background)
        var result: Bool?
        model.runService { result = $0 }
        await eventually { rig.runner.isRunning }
        rig.runner.finish(0)
        await eventually { result != nil }
        XCTAssertEqual(result, false)
        XCTAssertEqual(rig.runner.runs.count, 1)                     // no LaunchAgent: no second command
        XCTAssertEqual(background.steps, ["register"])
        XCTAssertEqual(model.serviceError, AppServicePlan.refusal("Operation not permitted"))
        XCTAssertFalse(model.serviceNeedsApproval)
        XCTAssertEqual(model.serviceAction, .update)                 // Try again sets it up once it's allowed
    }

    // decision: DMN-593
    func testARegistrationFERBNeverAnswersIsReportedNotReplaced() async {
        let background = FakeBackgroundService()
        let (rig, model) = appRig(background)
        rig.rpc.failures["ping"] = DaemonRPCError.unavailable
        var result: Bool?
        model.runService { result = $0 }
        await eventually { rig.runner.isRunning }
        rig.runner.finish(0)
        await eventually { result != nil }
        XCTAssertEqual(result, false)
        XCTAssertEqual(background.steps, ["register"])               // kept registered: macOS may still start it
        XCTAssertEqual(rig.runner.runs.count, 1)
        XCTAssertEqual(model.serviceError, AppServicePlan.refusal(AppServicePlan.notStarted))
    }

    // decision: DMN-593, APP-506
    func testFERBSwitchedOffInLoginItemsGetsTheLoginItemsGuidance() async {
        for macOS in ["denied", "requiresApproval"] {
            let background = FakeBackgroundService()
            if macOS == "denied" { background.registerError = AppServicePlan.DeniedByUser() }
            else { background.afterRegister = .requiresApproval }
            let (rig, model) = appRig(background)
            var result: Bool?
            model.runService { result = $0 }
            await eventually { rig.runner.isRunning }
            rig.runner.finish(0)
            await eventually { result != nil }
            XCTAssertEqual(result, false)
            XCTAssertTrue(model.serviceNeedsApproval, macOS)
            XCTAssertEqual(model.serviceError, ServicePlan.approval)
            XCTAssertEqual(rig.runner.runs.count, 1)                 // no LaunchAgent around the owner's switch
            XCTAssertEqual(IntroCopy.loginItems.choices.map(\.id), [IntroCopy.Pick.openLoginItems])
        }
    }
}
