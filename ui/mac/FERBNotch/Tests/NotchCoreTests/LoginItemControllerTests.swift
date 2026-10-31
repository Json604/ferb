import AppKit
import SwiftUI
import XCTest
@testable import FERBNotch
@testable import NotchCore

@MainActor
private final class FakeLoginItem: LoginItemService {
    var enabled = false
    var needsApproval = false
    var refuse: Error?
    var staysOff = false
    var asksApproval = false
    var enables = 0
    var disables = 0

    func enable() throws {
        enables += 1
        if let refuse { throw refuse }
        if asksApproval { needsApproval = true; return }
        if !staysOff { enabled = true }
    }

    func disable() throws {
        disables += 1
        enabled = false
        needsApproval = false
    }
}

/// L34 Phase F: on by default once after setup; macOS's login item first; the LaunchAgent only when macOS refuses it.
@MainActor
final class LoginItemControllerTests: XCTestCase {
    private var defaults: UserDefaults!

    override func setUp() {
        super.setUp()
        defaults = scratchDefaults("ferb-login")
    }

    private func make(_ app: FakeLoginItem, _ agent: FakeLoginItem, available: Bool = true) -> LoginItemController {
        LoginItemController(appService: app, launchAgent: agent, defaults: defaults, available: available)
    }

    func testDefaultAfterSetupTurnsTheLoginItemOnOnce() {
        let app = FakeLoginItem(), agent = FakeLoginItem()
        let items = make(app, agent)
        items.applyDefaultAfterSetup()
        XCTAssertTrue(items.enabled)
        XCTAssertEqual(app.enables, 1)
        items.setEnabled(false)
        items.applyDefaultAfterSetup()                 // a later setup or update settles again
        XCTAssertFalse(items.enabled)
        XCTAssertEqual(app.enables, 1)
        XCTAssertEqual(agent.enables, 0)
    }

    func testARefusedLoginItemFallsBackToTheLaunchAgentAndSaysSo() {
        let app = FakeLoginItem(), agent = FakeLoginItem()
        app.refuse = NSError(domain: "SMAppServiceErrorDomain", code: 1,
                             userInfo: [NSLocalizedDescriptionKey: "Operation not permitted"])
        let items = make(app, agent)
        items.setEnabled(true)
        XCTAssertTrue(items.enabled)
        XCTAssertEqual(agent.enables, 1)
        XCTAssertEqual(items.note, LoginItemPlan.fallbackNote("Operation not permitted"))
        XCTAssertFalse(items.needsApproval)
    }

    func testARegisterThatLeavesItOffFallsBack() {
        let app = FakeLoginItem(), agent = FakeLoginItem()
        app.staysOff = true
        let items = make(app, agent)
        items.setEnabled(true)
        XCTAssertTrue(agent.enabled)
        XCTAssertEqual(items.note, LoginItemPlan.fallbackNote("the login item didn't turn on"))
    }

    func testApprovalNeededIsNotARefusal() {
        let app = FakeLoginItem(), agent = FakeLoginItem()
        app.asksApproval = true
        let items = make(app, agent)
        items.setEnabled(true)
        XCTAssertEqual(agent.enables, 0)               // never two launches at login
        XCTAssertTrue(items.needsApproval)
        XCTAssertEqual(items.note, LoginItemPlan.approvalNote)
    }

    func testAnAcceptedLoginItemRemovesTheFallback() {
        let app = FakeLoginItem(), agent = FakeLoginItem()
        agent.enabled = true                            // an earlier build used the fallback
        let items = make(app, agent)
        items.setEnabled(true)
        XCTAssertTrue(app.enabled)
        XCTAssertFalse(agent.enabled)
        XCTAssertNil(items.note)
    }

    func testTurningOffRemovesBothAndUninstallDoesToo() {
        let app = FakeLoginItem(), agent = FakeLoginItem()
        app.enabled = true
        agent.enabled = true
        let items = make(app, agent)
        items.setEnabled(false)
        XCTAssertFalse(app.enabled || agent.enabled)
        app.enabled = true
        items.removeForUninstall()
        XCTAssertFalse(app.enabled)
        XCTAssertFalse(items.enabled)
    }

    func testADevelopmentBuildRegistersNothing() {
        let app = FakeLoginItem(), agent = FakeLoginItem()
        let items = make(app, agent, available: false)
        items.applyDefaultAfterSetup()
        items.setEnabled(true)
        XCTAssertEqual(app.enables + agent.enables, 0)
        XCTAssertFalse(defaults.bool(forKey: LoginItemController.defaultAppliedKey))
    }

    func testTheLaunchAgentRemovesOnlyItsOwnFile() throws {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("ferb-agents-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: folder) }
        let plist = folder.appendingPathComponent(LoginItemPlan.label + ".plist")
        let agent = LaunchAgentLoginItem(plist: plist, appPath: "/Applications/FERB.app")
        XCTAssertFalse(agent.enabled)
        try agent.enable()
        XCTAssertTrue(agent.enabled)
        let written = try XCTUnwrap(NSDictionary(contentsOf: plist) as? [String: Any])
        XCTAssertEqual(written["ProgramArguments"] as? [String], ["/usr/bin/open", "-g", "-a", "/Applications/FERB.app"])
        try agent.disable()
        XCTAssertFalse(FileManager.default.fileExists(atPath: plist.path))
        try (["Label": "com.example.mine"] as NSDictionary).write(to: plist)
        try agent.disable()
        XCTAssertTrue(FileManager.default.fileExists(atPath: plist.path))     // the owner's own file stays
    }

    func testTheStartupSectionLaysOut() {
        let items = make(FakeLoginItem(), FakeLoginItem())
        let page = GeneralSettingsPage(feedback: SoundFeedback(), cursor: CursorController(makePanels: false),
                                       loginItems: items)
        let hosting = NSHostingView(rootView: page.frame(width: 700, height: 500))
        hosting.frame = NSRect(x: 0, y: 0, width: 700, height: 500)
        hosting.layout()
        XCTAssertGreaterThan(hosting.fittingSize.width, 0)
    }
}
