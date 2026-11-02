#if FERB_PLAYGROUND
import XCTest
@testable import FERBNotch
import NotchCore

/// The intro playground (PLAYGROUND.md) runs the real intro with nothing real behind it: only under its own bundle id,
/// with every service a fake (no daemon connection, no Keychain, no launchd, no SMAppService, no macOS permission
/// request), and a home that is temporary and never FERB's real ~/.ferb.
@MainActor
final class PlaygroundModeTests: XCTestCase {
    // decision: APP-617
    func testPreviewSoundsUseTheSimulatedCallAndStillRespectSoundSettings() throws {
        let state = PlaygroundState(file: try folder("sound").appendingPathComponent("state.json"), fresh: true)
        let defaults = scratchDefaults("PlaygroundSound")
        var uiSounds = true
        let sound = PlaygroundSound(state: state, defaults: defaults, preference: {
            $0 == "com.apple.sound.uiaudio.enabled" ? NSNumber(value: uiSounds) : nil
        })
        let cues: [FerbSound] = [.firstLaunch, .intro, .orbDrip, .orbHop, .orbMelt, .done]
        for cue in cues { XCTAssertTrue(sound.allows(cue), "\(cue)") }
        state.simulatesCall = true
        for cue in cues { XCTAssertFalse(sound.allows(cue), "\(cue)") }
        XCTAssertTrue(sound.allows(.needsYou))
        state.simulatesCall = false
        XCTAssertTrue(sound.allows(.intro))
        state.saved.soundOn = false
        XCTAssertFalse(sound.allows(.intro))
        state.saved.soundOn = true
        uiSounds = false
        XCTAssertFalse(sound.allows(.intro))
    }

    private func folder(_ name: String) throws -> URL {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("FERBNotchTests/playground-\(name)-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: url) }
        return url
    }

    // decision: APP-519
    func testOnlyItsOwnBundleIDRunsThePlayground() {
        XCTAssertTrue(PlaygroundMode.isPlayground(bundleID: "dev.ferb.playground.concept"))
        XCTAssertTrue(PlaygroundMode.isPlayground(bundleID: "dev.ferb.playground.arrival"))
        for other in ["dev.ferb.playground", "com.ferb.FERB", "dev.ferb.notch", "com.ferb.secrets", nil] {
            XCTAssertFalse(PlaygroundMode.isPlayground(bundleID: other), "\(other ?? "no bundle id")")
        }
        XCTAssertFalse(PlaygroundMode.requested)                      // the test runner is not the playground
    }

    // decision: APP-519
    func testHomeIsTemporaryAndNeverTheRealFerbHome() throws {
        let root = try folder("home")
        let account = root.appendingPathComponent("account/.ferb", isDirectory: true)
        try FileManager.default.createDirectory(at: account, withIntermediateDirectories: true)
        let temporary = root.appendingPathComponent("tmp", isDirectory: true)
        try FileManager.default.createDirectory(at: temporary, withIntermediateDirectories: true)
        let link = root.appendingPathComponent("link-to-ferb")
        try FileManager.default.createSymbolicLink(at: link, withDestinationURL: account)

        // Unset: a folder in the temporary directory.
        guard case .home(let home) = PlaygroundHome.resolve(given: nil, account: account.path, temporary: temporary) else {
            return XCTFail("an unset FERB_HOME should give the temporary home")
        }
        XCTAssertEqual(home.path, PlaygroundHome.canonical(temporary.appendingPathComponent("ferb-setup-concept").path))
        XCTAssertTrue(home.path.hasPrefix(PlaygroundHome.canonical(temporary.path)))
        XCTAssertEqual(PlaygroundHome.resolve(given: "", account: account.path, temporary: temporary), .home(home))

        // Elsewhere: taken as given.
        let elsewhere = root.appendingPathComponent("elsewhere").path
        XCTAssertEqual(PlaygroundHome.resolve(given: elsewhere, account: account.path, temporary: temporary),
                       .home(URL(fileURLWithPath: PlaygroundHome.canonical(elsewhere), isDirectory: true)))

        // The real ~/.ferb however it is spelled, a folder inside it, and a folder holding it: refused.
        for given in [account.path, account.path + "/", account.path + "/run", account.path + "/../.ferb", link.path,
                      link.path + "/workspace", account.deletingLastPathComponent().path, "/"] {
            guard case .refused = PlaygroundHome.resolve(given: given, account: account.path, temporary: temporary) else {
                return XCTFail("\(given) should be refused")
            }
        }
        // No account record: refused (it couldn't tell the real home apart).
        guard case .refused = PlaygroundHome.resolve(given: nil, account: nil, temporary: temporary) else {
            return XCTFail("no account home should be refused")
        }
    }

    // decision: APP-519
    func testEveryServiceBehindTheIntroIsAFake() throws {
        let home = try folder("services").appendingPathComponent("home", isDirectory: true)
        let services = PlaygroundServices.make(home: home, defaults: scratchDefaults("Playground"), fresh: true)

        // No daemon connection object anywhere: the notch and the setup model talk to the fake daemon.
        XCTAssertTrue(services.notch.rpc is PlaygroundDaemon)
        XCTAssertFalse(services.notch.rpc is DaemonRPC)
        XCTAssertTrue(services.setup.rpc is PlaygroundDaemon)
        XCTAssertFalse(services.setup.rpc is DaemonRPC)
        // No Keychain, no setup command, no macOS permission request, no System Settings, no launchd, no SMAppService,
        // no microphone.
        XCTAssertTrue(services.setup.keys is PlaygroundKeys)
        XCTAssertFalse(services.setup.keys is KeychainAPIKeyStore)
        XCTAssertTrue(services.setup.runner is PlaygroundRunner)
        XCTAssertFalse(services.setup.runner is SetupCommand)
        XCTAssertTrue(services.setup.app is PlaygroundAppPermissions)
        XCTAssertFalse(services.setup.app is SystemAppPermissions)
        XCTAssertTrue(services.setup.system is PlaygroundSettingsOpener)
        XCTAssertFalse(services.setup.system is SystemOpener)
        XCTAssertTrue(services.setup.service is PlaygroundService)
        XCTAssertFalse(services.setup.service is DaemonService)
        XCTAssertTrue(services.setup.background is PlaygroundBackground)
        XCTAssertFalse(services.setup.background is LaunchAgentService)
        XCTAssertTrue(services.notch.microphone is PlaygroundMicrophone)
        XCTAssertFalse(services.notch.microphone is Microphone)
        // The playground's own setup controller uses that model (never building the real one).
        let setup = AppSetupController(model: services.setup, defaults: scratchDefaults("PlaygroundSetup"))
        XCTAssertTrue(setup.model === services.setup)

        // Its state lives in the temporary home, and only there.
        XCTAssertEqual(services.home, home)
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: home.path), ["state.json"])
    }

    // decision: APP-519
    func testAQuitAndReopenKeepsTheFakeWorldAndAFreshStartDropsIt() throws {
        let home = try folder("reopen")
        let first = PlaygroundServices.make(home: home, defaults: scratchDefaults("Playground"), fresh: true)
        first.state.set("accessibility", .granted)
        first.state.set("screen_recording", .granted, fromSwitch: true)        // applies only after a restart
        XCTAssertEqual(first.state.grant("screen_recording"), .notDetermined)

        let reopened = PlaygroundServices.make(home: home, defaults: scratchDefaults("Playground"), fresh: false)
        XCTAssertEqual(reopened.state.grant("accessibility"), .granted)
        reopened.state.applyRestart()
        XCTAssertEqual(reopened.state.grant("screen_recording"), .granted)

        let fresh = PlaygroundServices.make(home: home, defaults: scratchDefaults("Playground"), fresh: true)
        XCTAssertEqual(fresh.state.grant("accessibility"), .notDetermined)
    }

    // decision: APP-519
    func testTheFakeDaemonAnswersWhatTheIntroAsks() async throws {
        let services = PlaygroundServices.make(home: try folder("daemon"), defaults: scratchDefaults("Playground"),
                                               fresh: true)
        services.state.setSpeed(100)
        let daemon = services.daemon

        let offered = try await daemon.call("intro.suggest", params: [:], wait: 60)
        XCTAssertEqual((offered["suggestions"] as? [[String: Any]])?.count, 2)

        var events: [[String: Any]] = []
        for try await event in daemon.stream("intro.preview", params: ["request": PlaygroundDaemon.suggestions[0].request]) {
            events.append(event)
        }
        XCTAssertEqual(events.first?["type"] as? String, "plan")
        XCTAssertEqual(events.last?["type"] as? String, "done")
        XCTAssertTrue(events.allSatisfy { $0["preview"] as? Bool == true })
        XCTAssertGreaterThan(events.filter { $0["type"] as? String == "part" }.count, 1)

        let snapshot = PermissionSnapshot(reply: try await daemon.call("setup.permissions", params: [:]))
        XCTAssertEqual(snapshot?.rows.map(\.id).sorted(),
                       ["accessibility", "clipboard", "full_disk_access", "input_monitoring", "screen_recording"])
        await services.setup.refreshPermissions()
        XCTAssertEqual(Set(SetupModel.allowAllOrder(services.setup.setupRows)), Set(SetupModel.allowAllSequence))

        services.state.saved.suggest = .fails
        do {
            _ = try await daemon.call("intro.suggest", params: [:], wait: 60)
            XCTFail("a failing suggest should throw")
        } catch {}
        services.state.saved.preview = .fails
        do {
            for try await _ in daemon.stream("intro.preview", params: ["request": "anything"]) {}
            XCTFail("a failing preview should end with its reason")
        } catch {
            XCTAssertFalse(error.localizedDescription.isEmpty)
        }
    }
}
#endif
