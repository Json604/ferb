#if canImport(UIKit)
import FerbAppModel
import FerbCore
import FerbDesign
import FerbMobile
import SwiftUI
import XCTest
@testable import FerbAppUI

/// The Sessions tab and one session drawn by the app from the model over a fake phone (kb HAND-651): with the design's
/// invented sessions as the Mac's, the app draws the design's goldens (Android's screenshots do the same).
@MainActor
final class SessionScreensTests: XCTestCase {
    private var rig: Rig!

    override func setUp() async throws {
        rig = Rig()
        rig.session.unlocked()
        rig.session.foreground(true)
        rig.phone.view = emptyView()
        rig.phone.codingList = SessionsList(enabled: true, sessions: [
            session("claude:1", "Fix the flaky login test", "web-app", "working", "Terminal"),
            session("claude:2", "Move the blog to the new theme", "blog", "waiting_for_you", "Visual Studio Code"),
            session("claude:3", "Add dark mode to settings", "notes-app", "working", "Terminal", withFerb: true),
            session("codex:4", "", "thesis-tools", "idle", ""),
        ])
        rig.phone.codingItems["claude:2"] = [
            item(100, "user", "Move the blog to the new theme and keep the old URLs working"),
            item(101, "assistant", "I'll switch the theme in the config, then add redirects for the old post URLs."),
            item(102, "tool", "Read: config/site.toml"),
            item(103, "tool", "Edit: config/site.toml"),
            item(104, "assistant", "The theme builds. Now I need to run the build to check the redirects."),
        ]
        rig.phone.codingCards["claude:2"] = [SessionCard(id: "card-1", kind: "permission", tool: "Bash", detail: "npm run build", question: "",
                                                          choices: [SessionChoice(id: "a", label: "Yes"),
                                                                    SessionChoice(id: "b", label: "Yes, and don't ask again"),
                                                                    SessionChoice(id: "c", label: "No")])]
    }
    override func tearDown() async throws { rig.tearDown() }

    private func session(_ id: String, _ title: String, _ folder: String, _ status: String, _ host: String, withFerb: Bool = false) -> AgentSession {
        AgentSession(id: id, agent: id.hasPrefix("codex") ? "codex" : "claude_code", agentName: id.hasPrefix("codex") ? "Codex" : "Claude Code",
                     title: title, folder: "/Users/owner/code/\(folder)", folderName: folder, status: status,
                     waiting: status == "waiting_for_you" ? "permission" : "none", host: host, canSend: true, noSend: "",
                     foreground: true, lastAt: "", lastText: "", controlledHere: false, control: "", withFerb: withFerb, goal: "")
    }

    private func item(_ seq: UInt64, _ kind: String, _ text: String) -> SessionItem {
        SessionItem(seq: seq, at: "", kind: kind, text: text, tool: "")
    }

    private func same<S: View, P: View>(_ name: String, @ViewBuilder sheet: () -> S, @ViewBuilder page: () -> P) {
        let view = PhoneScreen(moment: .lab(12.5), glide: false, sheet: sheet, page: page)
            .frame(width: PhoneInsets.labSize.width, height: PhoneInsets.labSize.height)
            .stillMotion()
        AppSnapshot.assert(view, size: PhoneInsets.labSize, path: name, at: AppSnapshot.designGolden("screens/\(name).png"), allowed: 0.002)
    }

    // decision: PHN-416, PHN-509, PHN-797
    func testTheTabDrawsTheDesignsSessionsAndItsOffCard() async {
        let vm = rig.model()
        await vm.loadSessions().value
        same("sessions-noon", sheet: { EmptyView() }, page: { SessionsTabPage(model: vm, insets: .lab, waiting: nil) })
        rig.phone.codingList.enabled = false
        await vm.loadSessions().value
        same("sessions-off-noon", sheet: { EmptyView() }, page: { SessionsTabPage(model: vm, insets: .lab, waiting: nil) })
    }

    // decision: PHN-416, PHN-509, PHN-797, HAND-663
    func testASessionDrawsTheDesignsSession() async {
        // more before the first page: Show earlier at its top
        rig.phone.codingItems["claude:2"] = (1...46).map { item($0, "tool", "Read: old-\($0)") } + rig.phone.codingItems["claude:2"]!
        let vm = rig.model()
        await vm.loadSessions().value
        await vm.openSession("claude:2").value
        guard var o = vm.coding.open else { return XCTFail("open") }
        // the design's five lines (the page held the newest 50; the earlier ones stay behind Show earlier)
        o.items = Array(o.items.suffix(5))
        XCTAssertTrue(o.more)
        same("session-noon", sheet: { SessionScreenPage(model: vm, open: o, insets: .lab, still: true) }, page: { SessionsTabPage(model: vm, insets: .lab, waiting: nil) })
    }
}
#endif
