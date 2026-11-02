import AppKit
import SwiftUI
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// L34 Phase E: every section and Settings tab builds and lays out with real rows, and ⌘, lands on Settings.
@MainActor
final class FERBWindowViewTests: XCTestCase {
    func actions() -> FERBAppActions {
        FERBAppActions(feedback: SoundFeedback(), cursor: CursorController(makePanels: false))
    }

    func lays(_ view: some View) {
        let hosting = NSHostingView(rootView: view.frame(width: 900, height: 600))
        hosting.frame = NSRect(x: 0, y: 0, width: 900, height: 600)
        hosting.layout()
        XCTAssertGreaterThan(hosting.fittingSize.width, 0)
    }

    // decision: APP-5
    func testFERBsOwnCommandsShowTheirOutputAndUninstallRunsItsHookFirst() async {
        var order: [String] = []
        var packaged = actions()
        packaged.beforeUninstall = { order.append("login item"); return nil }
        packaged.runFerb = { arguments, line, done in
            order.append(arguments.joined(separator: " "))
            line("`ferb` command: /Users/o/.local/bin/ferb")
            done(0)
            return true
        }
        let output = FerbOutput()
        output.run(["setup", "--cli"], with: packaged)
        XCTAssertEqual(output.lines, ["`ferb` command: /Users/o/.local/bin/ferb"])
        XCTAssertEqual(output.status, 0)
        output.uninstall(with: packaged)
        await settled(output)
        XCTAssertEqual(order, ["setup --cli", "login item", "uninstall"])
        XCTAssertNil(actions().runFerb)                                    // a development build runs nothing
    }

    /// Past bug (12e review): the service's unregister was fire-and-forget, so `ferb uninstall` could run while macOS
    /// still had FERB's background service registered. Now uninstall waits for it and doesn't run if it's still on.
    // decision: APP-5
    func testUninstallWaitsForTheServiceToUnregisterAndStopsIfItDidnt() async {
        let background = FakeBackgroundService()
        background.registration = .enabled
        background.afterUnregister = .enabled                              // macOS still lists it
        var ran: [String] = []
        var packaged = actions()
        packaged.beforeUninstall = { await background.unregisterForUninstall() }
        packaged.runFerb = { arguments, _, done in ran.append(arguments.joined(separator: " ")); done(0); return true }
        let output = FerbOutput()
        output.uninstall(with: packaged, purge: true)
        await settled(output)
        XCTAssertEqual(background.unregistrations, 1)
        XCTAssertEqual(ran, [])
        XCTAssertTrue(output.lines.first?.contains("nothing was uninstalled") ?? false)
        background.unregisterError = NSError(domain: "SMAppServiceErrorDomain", code: 1)
        output.uninstall(with: packaged)
        await settled(output)
        XCTAssertEqual(ran, [])
        background.unregisterError = nil
        background.afterUnregister = .notRegistered
        output.uninstall(with: packaged, purge: true)
        await settled(output)
        XCTAssertEqual(ran, ["uninstall --purge --yes"])
    }

    private func settled(_ output: FerbOutput) async {
        for _ in 0..<200 where output.running { await Task.yield() }
        await Task.yield()
    }
}
