import XCTest
@testable import NotchCore

/// L34 D: the menu bar icon and menu show the daemon's real state.
final class MenuBarStateTests: XCTestCase {
    func testOneMissIsNotNotRunning() {
        var link = DaemonLink()
        XCTAssertEqual(link.state, .connecting)
        link.failed(unreachable: true)
        XCTAssertEqual(link.state, .connecting)
        link.failed(unreachable: true)
        XCTAssertEqual(link.state, .unreachable)
        link.succeeded()
        XCTAssertEqual(link.state, .connected)
        link.failed(unreachable: false)                          // a dropped connection reconnects
        XCTAssertEqual(link.state, .connecting)
        link.failed(unreachable: true)
        link.restarting()
        link.failed(unreachable: true)
        XCTAssertEqual(link.state, .connecting)                  // Restart starts the count again
    }

    func testTheStateShowsWhatMattersMost() {
        XCTAssertEqual(MenuBarState.derive(link: .unreachable, paused: true, stopped: true, questions: 2, running: 1),
                       .notRunning)
        XCTAssertEqual(MenuBarState.derive(link: .connecting, paused: false, stopped: false, questions: 0, running: 0),
                       .connecting)
        XCTAssertEqual(MenuBarState.derive(link: .connected, paused: true, stopped: false, questions: 1, running: 3),
                       .needsYou(1))
        XCTAssertEqual(MenuBarState.derive(link: .connected, paused: true, stopped: true, questions: 0, running: 3),
                       .stopped)
        XCTAssertEqual(MenuBarState.derive(link: .connected, paused: true, stopped: false, questions: 0, running: 3),
                       .paused)
        XCTAssertEqual(MenuBarState.derive(link: .connected, paused: false, stopped: false, questions: 0, running: 2),
                       .working(2))
        XCTAssertEqual(MenuBarState.derive(link: .connected, paused: false, stopped: false, questions: 0, running: 0),
                       .ready)
        XCTAssertEqual(MenuBarState.notRunning.statusLine, "FERB isn't running — Restart")
        XCTAssertEqual(MenuBarState.working(1).statusLine, "Working on 1 task")
        XCTAssertEqual(MenuBarState.needsYou(2).symbol, "exclamationmark.bubble")
    }

    func testTheMenuOffersWhatTheStateNeeds() {
        func titles(_ state: MenuBarState, paused: Bool = false, stopped: Bool = false, setUp: Bool = true,
                    window: Bool = false) -> [String] {
            StatusMenu.entries(state: state, paused: paused, stopped: stopped, appName: "FERB", canSetUp: setUp,
                               hasWindow: window).compactMap {
                if case .item(let title, _, _) = $0 { return title }
                return nil
            }
        }
        XCTAssertEqual(titles(.ready), ["Open FERB", "Pause FERB", "Settings…", "Set Up FERB…", "Quit FERB"])
        XCTAssertEqual(titles(.notRunning), ["Open FERB", "Restart FERB", "Settings…", "Set Up FERB…", "Quit FERB"])
        XCTAssertEqual(titles(.paused, paused: true, setUp: false, window: true),
                       ["Open FERB", "Open FERB Window", "Resume FERB", "Settings…", "Quit FERB"])
        XCTAssertEqual(titles(.needsYou(1), paused: true, stopped: true),
                       ["Open FERB", "Lift Emergency Stop", "Resume FERB", "Settings…", "Set Up FERB…", "Quit FERB"])
        XCTAssertEqual(titles(.connecting), ["Open FERB", "Settings…", "Set Up FERB…", "Quit FERB"])
        let entries = StatusMenu.entries(state: .working(2), paused: false, stopped: false, appName: "FERB",
                                         canSetUp: false, hasWindow: false)
        XCTAssertEqual(entries.first, .status("Working on 2 tasks"))
        XCTAssertTrue(entries.contains(.item("Settings…", .settings, key: ",")))
        XCTAssertTrue(entries.contains(.item("Quit FERB", .quit, key: "q")))
    }
}
