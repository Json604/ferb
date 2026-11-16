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

    /// T-1003: the background service (Full Disk Access) removes FERB's browser files first, while it still runs; then
    /// the service is turned off; then `ferb uninstall` leaves the browsers' folders alone. A file it couldn't remove is
    /// listed, and then `ferb uninstall` tries it again itself.
    // decision: DMN-598, APP-5
    func testUninstallHasTheServiceRemoveBrowserFilesBeforeItIsTurnedOff() async {
        var order: [String] = []
        var packaged = actions()
        packaged.removeBrowserFiles = {
            order.append("browser files")
            return BrowserFilesRemoval(reply: ["done": true, "failed": [] as [Any],
                                              "removed": ["/u/Library/Application Support/Google/Chrome/NativeMessagingHosts/com.ferb.native_host.json"]])
        }
        packaged.beforeUninstall = { order.append("service off"); return nil }
        packaged.runFerb = { arguments, line, done in
            order.append(arguments.joined(separator: " "))
            line("removed /u/.ferb")
            done(0)
            return true
        }
        let output = FerbOutput()
        output.uninstall(with: packaged, purge: true)
        await settled(output)
        XCTAssertEqual(order, ["browser files", "service off", "uninstall --purge --yes --skip-browser-files"])
        XCTAssertEqual(output.lines, ["removed /u/Library/Application Support/Google/Chrome/NativeMessagingHosts/com.ferb.native_host.json",
                                      "removed /u/.ferb"])

        order = []
        packaged.removeBrowserFiles = {
            order.append("browser files")
            return BrowserFilesRemoval(reply: ["done": false, "removed": [] as [Any],
                                              "failed": [["path": "/u/B/com.ferb.native_host.json", "error": "Permission denied"]]])
        }
        output.uninstall(with: packaged)
        await settled(output)
        XCTAssertEqual(order, ["browser files", "service off", "uninstall"])
        XCTAssertEqual(output.lines.first, "couldn't remove /u/B/com.ferb.native_host.json: Permission denied")

        // a service that didn't answer: `ferb uninstall` does it all, as before
        XCTAssertEqual(BrowserFilesRemoval(reply: nil), BrowserFilesRemoval(lines: [], done: false))
    }

    private func settled(_ output: FerbOutput) async {
        for _ in 0..<200 where output.running { await Task.yield() }
        await Task.yield()
    }
}
