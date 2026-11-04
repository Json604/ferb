import XCTest
@testable import FERBNotch
import NotchCore

/// Owner's fresh install, 2026-11-04: before setup got going the menu bar showed "FERB isn't running" with an error
/// icon, though the background service simply wasn't installed yet. While FERB.app sets the service up (a first run
/// before Meet, a quiet update, a restart) the menu bar says it is setting up; only a service that should run and
/// doesn't is the error with Restart.
@MainActor
final class MenuBarStateTests: XCTestCase {
    // decision: APP-698
    func testSettingUpIsNeverTheErrorAndAServiceDownAfterSetupIs() {
        let firstRun = MenuBarState.derive(link: .unreachable, paused: false, stopped: false, questions: 0, running: 0,
                                           settingUp: true)
        XCTAssertEqual(firstRun, .settingUp)
        XCTAssertNotEqual(firstRun.symbol, MenuBarState.notRunning.symbol)
        let duringSetup = StatusMenu.entries(state: firstRun, paused: false, stopped: false, appName: "FERB",
                                             canSetUp: true, hasWindow: true)
        XCTAssertFalse(duringSetup.contains(.item("Restart FERB", .restart, key: "")))

        let afterSetup = MenuBarState.derive(link: .unreachable, paused: false, stopped: false, questions: 0, running: 0,
                                             settingUp: false)
        XCTAssertEqual(afterSetup, .notRunning)
        XCTAssertTrue(StatusMenu.entries(state: afterSetup, paused: false, stopped: false, appName: "FERB",
                                         canSetUp: true, hasWindow: true)
            .contains(.item("Restart FERB", .restart, key: "")))
        // Once the daemon answers, setting up changes nothing.
        XCTAssertEqual(MenuBarState.derive(link: .connected, paused: false, stopped: false, questions: 0, running: 1,
                                           settingUp: true), .working(1))
    }

    /// Which setup states count as setting the service up: an install not yet run (a first run before Meet) and its
    /// command running count; a failed install (its reason shown in the tour) and a service set up don't.
    // decision: APP-698
    func testTheServiceCountsAsSettingUpUntilItIsInstalledOrFails() {
        let rig = SetupRig()
        let model = rig.model(.install)
        XCTAssertTrue(model.settingUpService)                             // first run, before Meet installs it
        var result: Bool?
        model.runService { result = $0 }
        XCTAssertTrue(model.settingUpService)                             // `ferb setup --service` running
        rig.runner.finish(1)
        XCTAssertEqual(result, false)
        XCTAssertFalse(model.settingUpService)                            // failed: the error, with its reason

        let installed = rig.model(.install)
        installed.runService { result = $0 }
        rig.runner.finish(0)
        XCTAssertEqual(result, true)
        XCTAssertFalse(installed.settingUpService)                        // set up: a silent daemon is the error
    }
}
