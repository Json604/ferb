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
            session("claude:3", "Add dark mode to settings", "notes-app", "working", "Terminal", withFerb: true, ferbTask: "t-3",
                    ferbNote: SessionSample.ferbNote),
            session("codex:4", "", "thesis-tools", "idle", ""),
        ], noChats: [])
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

    private func session(_ id: String, _ title: String, _ folder: String, _ status: String, _ host: String, withFerb: Bool = false,
                         ferbTask: String = "", ferbNote: String = "") -> AgentSession {
        AgentSession(id: id, agent: id.hasPrefix("codex") ? "codex" : "claude_code", agentName: id.hasPrefix("codex") ? "Codex" : "Claude Code",
                     title: title, folder: "/Users/owner/code/\(folder)", folderName: folder, status: status,
                     waiting: status == "waiting_for_you" ? "permission" : "none", host: host, canSend: true, noSend: "",
                     foreground: true, lastAt: "", lastText: "", controlledHere: false, control: "", withFerb: withFerb, goal: "", terminals: [],
                     program: "", window: "", noText: "", ferbTask: ferbTask, ferbNote: ferbNote)
    }

    private func item(_ seq: UInt64, _ kind: String, _ text: String, parts: [SessionPart] = [], ferb: Bool = false) -> SessionItem {
        SessionItem(seq: seq, at: "", kind: kind, text: text, tool: "", parts: parts, ferb: ferb)
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

    /// Opening a session, its first page not read yet: the main chat's waiting look, not an empty page and no error before
    /// the Mac answered (owner 2026-11-23).
    // decision: PHN-816
    func testOpeningASessionShowsTheWaitingLookUntilItsFirstPage() async {
        let vm = rig.model()
        await vm.loadSessions().value
        let o = OpenSession(id: "claude:2", session: vm.coding.row("claude:2"))
        XCTAssertFalse(o.loaded)
        XCTAssertNil(vm.ui.note, "nothing said before the Mac answered")
        same("session-waiting-402", sheet: { SessionScreenPage(model: vm, open: o, insets: .lab, still: true) },
             page: { SessionsTabPage(model: vm, insets: .lab, waiting: nil) })
        await vm.openSession("claude:2").value
        XCTAssertEqual(vm.coding.open?.loaded, true, "the first page read: the conversation replaces the waiting look")
        XCTAssertFalse(vm.coding.open?.items.isEmpty ?? true)
    }

    /// An agent's reply with parts, read through the real phone library's Markdown, draws the design's rich reply: one
    /// block, a thought, words, a step closed with its label and detail, a real table, a list, inline code, "Just now"
    /// (owner 2026-11-23).
    // decision: HAND-716
    func testAReplyWithPartsDrawsTheDesignsRichReply() async {
        func part(_ kind: String, _ text: String, label: String = "", detail: String = "") -> SessionPart {
            SessionPart(kind: kind, text: text, label: label, detail: detail)
        }
        rig.phone.codingItems["claude:2"] = [
            item(1, "user", "What's in this folder? I forgot"),
            item(2, "assistant", "", parts: [
                part("thinking", "", label: "Thought 1s"),
                part("text", "I'll look through this folder and tell you what it is."),
                part("tool", "  Length      Date    Time    Name\n---------  ---------- -----   ----\n     2048  01-01-2020 00:00   photos/\n",
                     label: "List the archives and their sizes", detail: "ls, unzip, head"),
                part("thinking", "Four zips with photo folders inside; one holds text files.", label: "Thought 4s"),
                part("text", SessionSample.richMarkdown),
                part("text", "So it's your old photo backup, sitting in four zips."),
                part("meta", "Just now"),
            ]),
        ]
        rig.phone.codingCards["claude:2"] = []
        let vm = rig.model()
        await vm.loadSessions().value
        await vm.openSession("claude:2").value
        guard var o = vm.coding.open else { return XCTFail("open") }
        let lines = SessionWords.lines(o.items, today: 0) { _ in "" }
        XCTAssertEqual(lines.map(\.kind), [.owner, .reply], "one owner bubble, one reply block")
        XCTAssertEqual(lines[1].parts, SessionSample.richParts, "the library's Markdown is the design's blocks: the table a table, the list a list")
        o.more = true
        same("session-rich-reply-402", sheet: { SessionScreenPage(model: vm, open: o, insets: .lab, still: true) },
             page: { SessionsTabPage(model: vm, insets: .lab, waiting: nil) })
    }

    /// A session FERB has: its menu has Take it back where Hand to FERB was, and FERB's word shows above the box: the
    /// design's goldens (owner 2026-11-23).
    // decision: HAND-702, ORCH-687
    func testASessionFERBHasDrawsTakeItBackAndFERBsWord() async {
        rig.phone.codingItems["claude:3"] = rig.phone.codingItems["claude:2"]
        let vm = rig.model()
        await vm.loadSessions().value
        await vm.openSession("claude:3").value
        guard var o = vm.coding.open else { return XCTFail("open") }
        same("session-ferb-note-402", sheet: { SessionScreenPage(model: vm, open: o, insets: .lab, still: true) },
             page: { SessionsTabPage(model: vm, insets: .lab, waiting: nil) })
        o.menu = true
        same("session-menu-take-back-402", sheet: { SessionScreenPage(model: vm, open: o, insets: .lab, still: true) },
             page: { SessionsTabPage(model: vm, insets: .lab, waiting: nil) })
        o.takingBack = true
        same("session-menu-taking-back-402", sheet: { SessionScreenPage(model: vm, open: o, insets: .lab, still: true) },
             page: { SessionsTabPage(model: vm, insets: .lab, waiting: nil) })
    }

    /// FERB kept the session going for 20 rounds and asks in the session whether to keep going; what it sent is marked as
    /// FERB's: drawn by the app from the Mac's card (kind ferb) and item (ferb), the design's golden.
    // decision: HAND-708, HAND-709
    func testFERBsKeepGoingQuestionAndWhatItSentDrawTheDesigns() async {
        rig.phone.codingList.sessions[2].ferbNote = SessionSample.keepGoingNote
        rig.phone.codingItems["claude:3"] = [
            item(1, "assistant", "The tests build. Two of them still fail on the login form."),
            item(2, "user", "Fix those two and run the tests again", ferb: true),
            item(3, "assistant", "Done: both pass now, and the whole suite is green."),
        ]
        rig.phone.codingCards["claude:3"] = [SessionCard(id: "ferb-t-3-20", kind: "ferb", tool: "", detail: "",
                                                          question: SessionSample.keepGoingQuestion,
                                                          choices: [SessionChoice(id: "keep_going", label: "Keep going"),
                                                                    SessionChoice(id: "stop", label: "Stop")])]
        let vm = rig.model()
        await vm.loadSessions().value
        await vm.openSession("claude:3").value
        guard let o = vm.coding.open else { return XCTFail("open") }
        XCTAssertEqual(SessionWords.ask(o.cards[0], agent: "Claude Code").title, "FERB asks")
        XCTAssertEqual(SessionWords.line(o.items[1]).by, "Sent by FERB")
        XCTAssertNil(SessionWords.line(o.items[0]).by)
        same("session-keep-going-402", sheet: { SessionScreenPage(model: vm, open: o, insets: .lab, still: true) },
             page: { SessionsTabPage(model: vm, insets: .lab, waiting: nil) })
    }

    /// A terminal FERB reads none of the words of (HAND-710): its screen is its window live (the Mac's picture through
    /// Watch's preview) in place of the conversation, the note on what a send does and "Type here"; no Hand to FERB (nor
    /// Watch: an idle one has no menu). Drawn by the app from the model: the design's golden.
    // decision: HAND-710
    func testAWatchSessionDrawsTheDesignsWindowScreen() async {
        var t = session("term:w7", "", "web-app", "idle", "Terminal")
        t.agent = "terminal"
        t.agentName = "zsh"
        t.program = "zsh"
        t.window = "web-app — zsh"
        t.watch = true
        rig.phone.codingList.sessions.append(t)
        let vm = rig.model()
        await vm.loadSessions().value
        await vm.openSession("term:w7").value
        guard let o = vm.coding.open else { return XCTFail("open") }
        XCTAssertEqual(SessionScreenPage.actions(model: vm, open: o).map(\.label), [], "no Hand to FERB, and its screen is its Watch already")
        await eventually("its window asked for") { self.rig.phone.sessionPreview != nil }
        rig.phone.sessionPreview?.frame(frame: PreviewFrame(jpeg: SessionSample.windowPicture(), seq: 1, state: "", line: nil, fresh: true,
                                                            busy: false, gone: false, emptyText: nil))
        await eventually("the picture") { vm.live.view?.frame != nil }
        same("session-watch-402", sheet: {
            SessionScreenPage(model: vm, open: o, insets: .lab, still: true).environment(\.ferbBlockScreenshots, false)
        }, page: { SessionsTabPage(model: vm, insets: .lab, waiting: nil) })
        // a Claude Code session keeps Hand to FERB
        XCTAssertEqual(SessionScreenPage.actions(model: vm, open: OpenSession(id: "claude:2", session: vm.coding.row("claude:2"))).map(\.label),
                       [L.s("sessions_watch"), L.s("sessions_hand_over"), L.s("sessions_stop_turn")])
    }

    /// New session at the tap of Start, and its folder picker inside a folder, drawn by the app: the design's goldens.
    // decision: PHN-821, PHN-822
    func testNewSessionStartingAndItsFolderPickerDrawTheDesigns() async {
        rig.phone.codingFolders = ["/Users/you/code/web-app", "/Users/you/code/blog", "/Users/you/code/notes-app"]
        rig.phone.folders["/Users/you/code"] = Folder(path: "/Users/you/code", parent: "/Users/you", locked: false, entries: [
            FileEntry(name: "blog", path: "/Users/you/code/blog", dir: true, link: false, size: 0, modified: 0, locked: false),
            FileEntry(name: "notes-app", path: "/Users/you/code/notes-app", dir: true, link: false, size: 0, modified: 0, locked: false),
            FileEntry(name: "README.md", path: "/Users/you/code/README.md", dir: false, link: false, size: 12, modified: 0, locked: false),
            FileEntry(name: "thesis-tools", path: "/Users/you/code/thesis-tools", dir: true, link: false, size: 0, modified: 0, locked: false),
            FileEntry(name: "web-app", path: "/Users/you/code/web-app", dir: true, link: false, size: 0, modified: 0, locked: false),
        ], total: 5, cut: false, from: 0, next: nil)
        let vm = rig.model()
        await vm.loadSessions().value
        await vm.newSession().value
        vm.newSessionText("claude")
        await vm.pickNewSessionFolder()?.value
        await vm.pickerOpen("/Users/you/code")?.value
        same("session-folder-picker-402", sheet: { NewSessionSheet(model: vm, insets: .lab, still: true) },
             page: { SessionsTabPage(model: vm, insets: .lab, waiting: nil) })
        vm.pickerClose()
        rig.phone.prepareRefusal = .Refused(detail: "There's no folder named web-app on your Mac any more.", linkCode: "ferb_error")
        let start = vm.startSession()
        same("session-new-starting-402", sheet: { NewSessionSheet(model: vm, insets: .lab, still: true) },
             page: { SessionsTabPage(model: vm, insets: .lab, waiting: nil) })
        await start.value
        same("session-new-refused-402", sheet: { NewSessionSheet(model: vm, insets: .lab, still: true) },
             page: { SessionsTabPage(model: vm, insets: .lab, waiting: nil) })
    }

    /// His words the moment he tapped Send, drawn by the app from its one outgoing state: the design's sending golden
    /// (owner 2026-11-23).
    // decision: PHN-812, PHN-814
    func testHisWordsWhileTheyGoDrawTheDesignsSendingBubble() async {
        rig.phone.codingItems["claude:2"] = (1...46).map { item($0, "tool", "Read: old-\($0)") } + rig.phone.codingItems["claude:2"]!
        let vm = rig.model()
        await vm.loadSessions().value
        await vm.openSession("claude:2").value
        guard var o = vm.coding.open else { return XCTFail("open") }
        o.items = Array(o.items.suffix(5))
        o.cards = []
        o.outgoing = SessionOutgoing(text: "Then deploy it to staging", state: .sending, after: o.last)
        same("session-sending-402", sheet: { SessionScreenPage(model: vm, open: o, insets: .lab, still: true) },
             page: { SessionsTabPage(model: vm, insets: .lab, waiting: nil) })
    }
}
#endif
