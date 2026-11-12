import XCTest
@testable import FERBNotch
@testable import NotchCore

/// The switch to the Developer ID (T-27): the signer change is noticed only on a team-signed build over an install
/// set up under another signer; then the service moves into the app (with the LaunchAgent as its fallback), the login
/// item moves, FERB's own permission entries are cleared once, and the switch update asks first.
@MainActor
final class DeveloperIDSwitchTests: XCTestCase {
    private let team = "team:TEAM123456"
    private let leaf = "leaf:0123456789abcdef0123456789abcdef01234567"

    // decision: APP-904
    func testOnlyATeamSignedBuildOverAnotherSignersSetupIsTheSwitch() {
        XCTAssertEqual(Signer.words(team: "TEAM123456", leafSHA1: "AB"), team)
        XCTAssertEqual(Signer.words(team: nil, leafSHA1: "0123456789ABCDEF0123456789ABCDEF01234567"), leaf)
        XCTAssertEqual(Signer.words(team: nil, leafSHA1: nil), "adhoc")
        XCTAssertTrue(SignerMove.starts(record: ["signer": leaf], own: team))
        XCTAssertTrue(SignerMove.starts(record: ["build": "202611070749"], own: team))   // a record from before T-27
        XCTAssertFalse(SignerMove.starts(record: ["signer": team], own: team))           // a later Developer ID build
        XCTAssertFalse(SignerMove.starts(record: nil, own: team))                        // a new install: nothing to move
        XCTAssertFalse(SignerMove.starts(record: ["build": "1"], own: leaf))             // today's self-signed build
        XCTAssertFalse(SignerMove.starts(record: ["signer": leaf], own: "adhoc"))
    }

    // decision: APP-902
    func testThePermissionResetNamesOnlyFERBsOwnAppIDs() {
        let resets = SignerMove.permissionResets()
        XCTAssertEqual(Set(resets.map { $0[2] }), ["com.ferb.FERB", "com.ferb.daemon"])
        XCTAssertTrue(resets.allSatisfy { $0.count == 3 && $0[0] == "reset" && $0[1] != "All" })
        XCTAssertEqual(resets.count, SignerMove.services.count * 2)
    }

    // decision: APP-902, APP-904
    func testTheSwitchClearsFERBsPermissionEntriesOnceAndMovesTheLoginItem() async {
        let defaults = UserDefaults(suiteName: "signer-\(UUID())")!
        let setup = AppSetupController(model: SetupRig().model(), defaults: defaults)
        var ran: [[String]] = []
        var moves = 0
        setup.tccutil = { ran.append($0); return 0 }
        setup.onSignerMove = { moves += 1 }

        setup.ownSigner = leaf                                       // today's self-signed build: nothing at all
        await setup.signerAtLaunch(record: ["signer": "leaf:old"])
        XCTAssertTrue(ran.isEmpty && moves == 0)
        XCTAssertFalse(setup.model.appService || setup.model.signerChanged)

        setup.ownSigner = team                                       // the first Developer ID launch
        await setup.signerAtLaunch(record: ["signer": leaf])
        XCTAssertEqual(ran, SignerMove.permissionResets())
        XCTAssertEqual(moves, 1)
        XCTAssertTrue(setup.model.appService && setup.model.signerChanged)

        ran = []                                                     // a quit cut the move short: it resumes,
        await setup.signerAtLaunch(record: ["signer": team])          // the record already names the new signer,
        XCTAssertTrue(ran.isEmpty)                                    // and permissions are never cleared twice
        XCTAssertEqual(moves, 2)

        defaults.removeObject(forKey: SignerMove.pendingKey)          // the service ran under the new signer
        let later = AppSetupController(model: SetupRig().model(), defaults: defaults)
        later.ownSigner = team
        later.tccutil = { ran.append($0); return 0 }
        later.onSignerMove = { moves += 1 }
        await later.signerAtLaunch(record: ["signer": team])          // the next Developer ID build
        XCTAssertTrue(ran.isEmpty && moves == 2 && !later.model.signerChanged && later.model.appService)
    }

    // decision: DMN-587, APP-904
    func testTheAppsOwnRegistrationIsSetUpWhileListedAndAloneAndTheSwitchAlwaysSetsUp() {
        func decide(_ record: [String: Any], agent: [String: Any]? = nil, registration: ServiceStatus = .enabled,
                    moving: Bool = false) -> AppSetupAction {
            AppSetupPlan.decide(launchAgent: agent, launchAgentDigest: agent == nil ? nil : "d1",
                                registration: registration, record: record,
                                agentExecutable: "/Applications/FERB.app/Contents/Helpers/FERB.app/Contents/MacOS/FERB",
                                appPath: "/Applications/FERB.app", build: "202612010900", appService: true,
                                signerMoving: moving)
        }
        let inApp: [String: Any] = ["build": "202612010900", "app": "/Applications/FERB.app", "service": "app",
                                    "signer": team]
        XCTAssertEqual(decide(inApp), .none)
        XCTAssertEqual(decide(inApp, registration: .notRegistered), .update)
        XCTAssertEqual(decide(inApp, agent: ["Label": "com.ferb.daemon", "ProgramArguments": ["/x", "-m", "ferb.daemon"]]),
                       .update)                                          // a LaunchAgent is left: retire it
        XCTAssertEqual(decide(inApp, moving: true), .update)
        XCTAssertEqual(AppSetupPlan.serviceArguments(for: .update, appService: true),
                       ["setup", "--service", "--update", "--app-service"])
        XCTAssertEqual(AppSetupPlan.serviceArguments(for: .install, appService: true),
                       ["setup", "--service", "--app-service"])
    }

    // decision: DMN-587, APP-905
    func testTheFallbackRule() {
        XCTAssertEqual(AppServicePlan.afterRegister(error: nil, deniedByUser: false, status: .enabled), .waitForFERB)
        XCTAssertEqual(AppServicePlan.afterRegister(error: nil, deniedByUser: true, status: .notRegistered), .approval)
        XCTAssertEqual(AppServicePlan.afterRegister(error: nil, deniedByUser: false, status: .requiresApproval),
                       .approval)
        XCTAssertEqual(AppServicePlan.afterRegister(error: "Invalid signature", deniedByUser: false,
                                                    status: .notRegistered), .fallback("Invalid signature"))
        guard case .fallback = AppServicePlan.afterRegister(error: nil, deniedByUser: false, status: .notFound) else {
            return XCTFail("a status that stays off falls back")
        }
    }

    private func appRig(_ background: FakeBackgroundService, signerChanged: Bool = false) -> (SetupRig, SetupModel) {
        let rig = SetupRig()
        let model = rig.model(.update, background: background)
        model.appService = true
        model.signerChanged = signerChanged
        model.serviceStartWait = .milliseconds(50)
        return (rig, model)
    }

    // decision: DMN-587, DMN-589, APP-904
    func testTheSwitchUnregistersAnOldRegistrationThenRetiresTheLaunchAgentThenRegistersAndWaits() async {
        let background = FakeBackgroundService()
        background.registration = .enabled                           // a 2026-11 self-signed build's registration
        let (rig, model) = appRig(background, signerChanged: true)
        var result: Bool?
        model.runService { result = $0 }
        await eventually { rig.runner.isRunning }
        XCTAssertEqual(background.steps, ["unregister"])            // before the command
        XCTAssertEqual(rig.runner.runs, [["setup", "--service", "--update", "--app-service"]])
        rig.runner.finish(0)
        await eventually { result != nil }
        XCTAssertEqual(background.steps, ["unregister", "register"])
        XCTAssertEqual(result, true)
        XCTAssertEqual(model.serviceAction, .none)
        XCTAssertNil(model.serviceFallback)
        XCTAssertTrue(rig.rpc.calls.contains { $0.0 == "ping" })     // registered, then waited for FERB
    }

    // decision: DMN-587
    func testALaterUpdateKeepsTheAppsOwnRegistration() async {
        let background = FakeBackgroundService()
        background.registration = .enabled                           // this app's own, running
        background.status = .enabled
        let (rig, model) = appRig(background)
        var result: Bool?
        model.runService { result = $0 }
        await eventually { rig.runner.isRunning }
        rig.runner.finish(0)
        await eventually { result != nil }
        XCTAssertEqual(background.unregistrations, 0)
        XCTAssertEqual(result, true)
    }

    // decision: DMN-587, APP-905
    func testARefusedRegistrationFallsBackToTheLaunchAgentAndSaysWhy() async {
        struct Refused: LocalizedError { var errorDescription: String? { "Operation not permitted" } }
        let background = FakeBackgroundService()
        background.registerError = Refused()
        let (rig, model) = appRig(background, signerChanged: true)
        var result: Bool?
        model.runService { result = $0 }
        await eventually { rig.runner.isRunning }
        rig.runner.finish(0)                                         // --app-service: the LaunchAgent is retired
        await eventually { rig.runner.runs.count == 2 }
        XCTAssertEqual(rig.runner.runs.last, ["setup", "--service", "--update"])   // T-29's path, loaded by setup
        XCTAssertEqual(model.serviceFallback, "Operation not permitted")
        XCTAssertTrue(model.serviceLine.contains("Operation not permitted"))
        rig.runner.finish(0)
        XCTAssertEqual(result, true)
        XCTAssertEqual(model.serviceAction, .none)
    }

    // decision: DMN-587, APP-905
    func testARegistrationFERBNeverAnswersIsUnregisteredBeforeTheFallback() async {
        let background = FakeBackgroundService()
        let (rig, model) = appRig(background)
        rig.rpc.failures["ping"] = DaemonRPCError.unavailable
        var result: Bool?
        model.runService { result = $0 }
        await eventually { rig.runner.isRunning }
        rig.runner.finish(0)
        await eventually { rig.runner.runs.count == 2 }
        XCTAssertEqual(background.steps, ["register", "unregister"])   // one label can't run twice
        XCTAssertEqual(model.serviceFallback, AppServicePlan.notStarted)
        rig.runner.finish(0)
        XCTAssertEqual(result, true)
    }

    // decision: DMN-587, APP-905, APP-506
    func testFERBSwitchedOffInLoginItemsIsGuidedNotWorkedAround() async {
        let background = FakeBackgroundService()
        background.registerError = AppServicePlan.DeniedByUser()
        let (rig, model) = appRig(background)
        var result: Bool?
        model.runService { result = $0 }
        await eventually { rig.runner.isRunning }
        rig.runner.finish(0)
        await eventually { result != nil }
        XCTAssertEqual(result, false)
        XCTAssertTrue(model.serviceNeedsApproval)
        XCTAssertEqual(model.serviceError, ServicePlan.approval)
        XCTAssertEqual(rig.runner.runs.count, 1)                     // no LaunchAgent around the owner's switch
    }

    // decision: APP-904
    func testTheLoginItemMovesToMacOSsOwnAndAnOwnersOffStaysOff() {
        final class Item: LoginItemService {
            var enabled = false
            var needsApproval = false
            var refuses = false
            var calls: [String] = []
            func enable() throws {
                calls.append("enable")
                if refuses { throw LoginItemPlan.Refused() }
                enabled = true
            }
            func disable() throws { calls.append("disable"); enabled = false }
        }
        let defaults = UserDefaults(suiteName: "login-\(UUID())")!
        let app = Item(), agent = Item()
        agent.enabled = true                                         // the fallback a self-signed build needed
        LoginItemController(appService: app, launchAgent: agent, defaults: defaults, available: true)
            .moveAfterSignerChange()
        XCTAssertTrue(app.enabled && !agent.enabled)

        let registered = Item(), none = Item()
        registered.enabled = true                                    // macOS's own, under the old signer
        LoginItemController(appService: registered, launchAgent: none, defaults: defaults, available: true)
            .moveAfterSignerChange()
        XCTAssertEqual(registered.calls, ["disable", "enable"])
        XCTAssertTrue(registered.enabled)

        let refusing = Item(), fallback = Item()
        refusing.refuses = true
        fallback.enabled = true
        LoginItemController(appService: refusing, launchAgent: fallback, defaults: defaults, available: true)
            .moveAfterSignerChange()
        XCTAssertTrue(fallback.enabled)                              // still refused: the fallback stays

        let off = Item(), offAgent = Item()
        LoginItemController(appService: off, launchAgent: offAgent, defaults: defaults, available: true)
            .moveAfterSignerChange()
        XCTAssertTrue(off.calls.isEmpty && offAgent.calls.isEmpty)
    }

    // decision: APP-903, APP-906
    func testTheSwitchUpdateAsksFirstEvenWithAutomaticUpdatesOn() {
        let updates = UpdateCoordinator(rpc: ScriptedRPC())
        updates.automatic = true
        updates.found(version: "0.28.0")
        XCTAssertEqual(updates.status, .quiet)                       // an ordinary update installs by itself
        XCTAssertNil(updates.offer)
        updates.found(version: "0.28.0", asksFirst: true)            // Sparkle's major upgrade: the signer switch
        XCTAssertEqual(updates.status, .asking("0.28.0"))
        XCTAssertEqual(updates.offer?.version, "0.28.0")
        XCTAssertEqual(updates.offer?.asks, true)
        XCTAssertEqual(updates.status.line, UpdatePlan.askLine(version: "0.28.0"))
        let menu = StatusMenu.entries(state: .ready, paused: false, stopped: false, appName: "FERB", canSetUp: true,
                                      hasWindow: true, updateOffer: "0.28.0", updateAsks: true)
        XCTAssertTrue(menu.contains { if case .item(_, .installUpdate, _) = $0 { return true }; return false })
    }
}
