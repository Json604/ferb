#if FERB_PLAYGROUND
import XCTest
@testable import FERBNotch
import NotchCore

/// Play 0.27.1 (APP-754) can't silently rot: every beat runs against the playground workspace and the real NotchModel,
/// and each beat's own check of the UI state it leaves must hold (cards, counts matching rows, no timers, the "+N"
/// badge, the Couldn't confirm line only on the really unconfirmed part). The host stands in for the windows.
@MainActor
final class PlaygroundPlayTests: XCTestCase {
    private final class Host: PlayHost {
        var calls: [String] = []
        weak var model: NotchModel?
        func openNotch() { calls.append("open"); model?.expand() }
        func closeNotch() { calls.append("close"); model?.collapse() }
        func showSettings(_ tab: SettingsTab) { calls.append("settings \(tab)") }
        func closeSettings() { calls.append("close settings") }
        func setChatHost(_ host: ChatHost) { calls.append("host \(host.rawValue)") }
        func setHidePill(_ on: Bool) { calls.append("hide pill \(on)") }
    }

    // decision: APP-754, APP-619, APP-752, APP-753, ORCH-543
    func testEveryBeatOfThePlayLeavesTheUIItShould() async throws {
        let home = FileManager.default.temporaryDirectory.appendingPathComponent("ferb-play-\(UUID())")
        addTeardownBlock { try? FileManager.default.removeItem(at: home) }
        let services = PlaygroundServices.make(home: home, defaults: scratchDefaults("PlaygroundPlay"), fresh: true)
        services.state.setSpeed(100)
        let workspace = services.daemon.workspace
        try workspace.prepare()
        let model = services.notch
        model.feedback.enabled = false
        model.connectDaemon()
        defer { model.disconnectDaemon() }
        await eventually { model.conversationID != nil && !workspace.listeners.isEmpty }
        let host = Host()
        host.model = model
        let player = PlaygroundPlayer(workspace: workspace, model: model, host: host)
        var failures: [String] = []
        while player.index < player.count {
            let caption = player.beats[player.index].caption
            if let failure = await player.step() { failures.append("\(caption): \(failure)") }
        }
        XCTAssertEqual(failures, [])
        XCTAssertEqual(player.failures, [])
        // the play touched only its stand-ins: the windows, never a real setting
        XCTAssertTrue(host.calls.contains("host window") && host.calls.last == "open")
        XCTAssertTrue(host.calls.contains { $0.hasPrefix("settings") })
        XCTAssertTrue(workspace.running.isEmpty && workspace.playApprovals.isEmpty && workspace.playTurns.isEmpty)
        // a restart plays again with new ids, after the earlier run
        let first = player.ids
        player.restart(andPlay: false)
        XCTAssertNotEqual(player.ids, first)
        XCTAssertEqual(player.index, 0)
    }
}
#endif
