import AppKit
import SwiftUI
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// The owner's live report on Sessions (2026-11-23): every tap shows it is on its way and then a result or the Mac's
/// reason (Hand to FERB, Take it back, Start), FERB's word on a hand-over shows in the session, a row always has a
/// name, and an agent's reply is drawn as one block of its parts (Markdown, thoughts, steps, its meta line).
@MainActor
final class SessionsHandOverTests: XCTestCase {
    typealias HoldingRPC = SessionsSendingTests.HoldingRPC

    private func opened(_ rpc: HoldingRPC, _ row: [String: Any]) async throws -> SessionsModel {
        rpc.scripted.replies["sessions.list"] = ["enabled": true, "sessions": [row]]
        rpc.scripted.replies["sessions.timeline"] = ["session": row, "more": false, "cards": [] as [Any],
                                                     "items": [["seq": 1, "kind": "user", "text": "hi"]]]
        let model = SessionsModel(rpc: rpc)
        await model.load()
        await model.open(try XCTUnwrap(model.sessions.first))
        return model
    }

    private func lays(_ view: some View, width: CGFloat = 700, height: CGFloat = 600) {
        let hosting = NSHostingView(rootView: view.frame(width: width, height: height))
        hosting.frame = NSRect(x: 0, y: 0, width: width, height: height)
        hosting.layout()
        XCTAssertGreaterThan(hosting.fittingSize.width, 0)
        XCTAssertGreaterThan(hosting.fittingSize.height, 0)
    }

    /// The Claude Code row, now FERB's or not.
    private func claude(delegate: String? = nil, goal: String? = nil, note: String = "") -> [String: Any] {
        var row = SessionsTests.claude
        row["delegate"] = delegate ?? NSNull()
        row["goal"] = goal ?? NSNull()
        row["ferb_note"] = note
        return row
    }

    // MARK: Row names

    func testARowsTitleIsNeverEmpty() throws {
        func heading(_ row: [String: Any]) throws -> String { try XCTUnwrap(AgentSessionRow(row)).heading }
        XCTAssertEqual(try heading(["id": "a", "title": "Tidy the styles", "window": "w", "cwd": "/Users/you/site"]),
                       "Tidy the styles")
        XCTAssertEqual(try heading(["id": "a", "window": "site — zsh — 80×24", "cwd": "/Users/you/site"]), "site — zsh — 80×24")
        XCTAssertEqual(try heading(["id": "a", "agent": "terminal", "cwd": "/Users/you/site"]), "site")
        // no title, window or folder: the agent's name (his screenshot: a row reading only "in the background · Idle")
        XCTAssertEqual(try heading(["id": "a", "agent": "terminal", "program": "zsh"]), "zsh")
        XCTAssertEqual(try heading(["id": "a", "agent": "terminal"]), "Terminal")
        XCTAssertEqual(try heading(["id": "a", "agent": "claude_code", "title": "  "]), "Claude Code")
        XCTAssertEqual(try heading(["id": "a", "agent": "app_chat", "host": ["name": "Cursor"]]), "Cursor")
        XCTAssertEqual(try heading(["id": "a"]), "Terminal")
    }

    // MARK: Take it back

    // decision: HAND-702
    func testTakeItBackStopsFERBsTaskShowingItAtOnceThenReadsTheSessionAgain() async throws {
        let rpc = HoldingRPC()
        let model = try await opened(rpc, claude(delegate: "task-9", goal: "Make the tests pass",
                                                 note: "FERB is on it: “Make the tests pass”"))
        let row = try XCTUnwrap(model.openSession)
        XCTAssertEqual(row.ferbTask, "task-9")
        XCTAssertEqual(row.ferbNote, "FERB is on it: “Make the tests pass”")
        rpc.held = ["task.stop"]
        let tapped = Task { await model.takeItBack() }
        try await rpc.waitForHeldCall()
        XCTAssertTrue(model.isTakingBack(row), "pending from the tap")
        lays(SessionDetailView(sessions: model, row: row, sheet: .constant(nil)))
        await model.takeItBack()                                         // a second tap does nothing meanwhile
        // the daemon: the task stopped, the session is his again
        let mine = claude(note: "FERB stopped. The session is yours again.")
        rpc.scripted.replies["sessions.list"] = ["enabled": true, "sessions": [mine]]
        rpc.scripted.replies["sessions.timeline"] = ["session": mine, "more": false, "cards": [] as [Any], "items": [] as [Any]]
        rpc.held = []
        let before = rpc.scripted.methods().count
        rpc.release()
        await tapped.value
        XCTAssertEqual(rpc.scripted.params(of: "task.stop").count, 1)
        XCTAssertEqual(rpc.scripted.params(of: "task.stop").first?["task_ids"] as? [String], ["task-9"])
        XCTAssertTrue(Array(rpc.scripted.methods().dropFirst(before)).contains("sessions.list"), "read again at once")
        XCTAssertNil(model.takingBack)
        XCTAssertEqual(model.openSession?.withFerb, false)
        XCTAssertEqual(model.openSession?.ferbNote, "FERB stopped. The session is yours again.")
        XCTAssertNil(model.problem)
    }

    func testTakeItBackStaysPendingUntilTheSessionNoLongerNamesTheTask() async throws {
        let rpc = HoldingRPC()
        let model = try await opened(rpc, claude(delegate: "task-9", goal: "g"))
        var clock = Date(timeIntervalSince1970: 1_000)
        model.now = { clock }
        await model.takeItBack()
        let row = try XCTUnwrap(model.openSession)
        XCTAssertTrue(model.isTakingBack(row), "the daemon still names the task: still on its way")
        clock += SessionsModel.takeBackWait
        await model.load(quiet: true)
        XCTAssertNil(model.takingBack, "never pending forever")
    }

    func testARefusedTakeItBackSaysWhy() async throws {
        let rpc = HoldingRPC()
        let model = try await opened(rpc, claude(delegate: "task-9", goal: "g"))
        rpc.scripted.failures["task.stop"] = DaemonRPCError.remote("FERB isn't running.", code: -32000)
        await model.takeItBack()
        XCTAssertEqual(model.problem, "Couldn't take it back: FERB isn't running.")
        XCTAssertNil(model.takingBack)
    }

    func testWhileFERBIsStartingThereIsNothingToTakeBack() async throws {
        let rpc = HoldingRPC()
        let model = try await opened(rpc, claude(goal: "Make the tests pass", note: "Handed to FERB: “Make the tests pass”"))
        let row = try XCTUnwrap(model.openSession)
        XCTAssertTrue(row.withFerb)
        XCTAssertEqual(row.ferbTask, "")
        await model.takeItBack()
        XCTAssertTrue(rpc.scripted.params(of: "task.stop").isEmpty)
        lays(SessionDetailView(sessions: model, row: row, sheet: .constant(nil)))
    }

    // MARK: Hand to FERB

    // decision: HAND-702
    func testHandToFERBIsPendingUntilTheMacAnswersThenReadsTheSessionAgain() async throws {
        let rpc = HoldingRPC()
        let model = try await opened(rpc, claude())
        rpc.held = ["sessions.delegate"]
        rpc.scripted.replies["sessions.delegate"] = ["handed_over": true, "message": "Take over my terminal…"]
        let tapped = Task { await model.delegate(goal: "Make the tests pass") }
        try await rpc.waitForHeldCall()
        XCTAssertTrue(model.handingOver, "pending from the tap")
        lays(HandOverView(sessions: model, close: {}))
        let again = await model.delegate(goal: "Make the tests pass")
        XCTAssertFalse(again, "never handed over twice")
        let handed = claude(goal: "Make the tests pass", note: "Handed to FERB: “Make the tests pass”")
        rpc.scripted.replies["sessions.list"] = ["enabled": true, "sessions": [handed]]
        rpc.scripted.replies["sessions.timeline"] = ["session": handed, "more": false, "cards": [] as [Any], "items": [] as [Any]]
        rpc.held = []
        rpc.release()
        let result = await tapped.value
        XCTAssertTrue(result)
        XCTAssertFalse(model.handingOver)
        XCTAssertNil(model.handOverProblem)
        XCTAssertEqual(rpc.scripted.params(of: "sessions.delegate").count, 1)
        XCTAssertEqual(model.openSession?.ferbNote, "Handed to FERB: “Make the tests pass”")
        XCTAssertEqual(model.openSession?.withFerb, true)
    }

    func testARefusedHandOverStaysOnTheSheetWithTheMacsReason() async throws {
        let rpc = HoldingRPC()
        let model = try await opened(rpc, claude())
        rpc.scripted.failures["sessions.delegate"] = DaemonRPCError.remote("That session has ended.",
                                                                          code: SessionRefusal.sessionGone)
        let handed = await model.delegate(goal: "Make the tests pass")
        XCTAssertFalse(handed)
        XCTAssertEqual(model.handOverProblem, "That session has ended.")
        XCTAssertNil(model.problem, "said under the sheet's button, not behind it")
        XCTAssertFalse(model.handingOver)
        lays(HandOverView(sessions: model, close: {}))
    }

    // MARK: FERB's note

    // decision: HAND-702
    func testFERBsNoteIsReadOnEveryReadAndShown() async throws {
        let rpc = HoldingRPC()
        let model = try await opened(rpc, claude(goal: "g", note: "Handed to FERB: “g”"))
        XCTAssertEqual(model.openSession?.ferbNote, "Handed to FERB: “g”")
        let asked = claude(note: "FERB asked you something in its chat before it starts: “Which branch?”")
        rpc.scripted.replies["sessions.timeline"] = ["session": asked, "more": false, "cards": [] as [Any], "items": [] as [Any]]
        await model.poll()
        XCTAssertEqual(model.openSession?.ferbNote, "FERB asked you something in its chat before it starts: “Which branch?”")
        XCTAssertEqual(model.openSession?.withFerb, false)
        lays(SessionDetailView(sessions: model, row: try XCTUnwrap(model.openSession), sheet: .constant(nil)))
        lays(SessionFerbNote(text: "FERB finished. The session is yours again."), width: 500, height: 40)
        rpc.scripted.replies["sessions.timeline"] = ["session": claude(), "more": false, "cards": [] as [Any], "items": [] as [Any]]
        await model.poll()
        XCTAssertEqual(model.openSession?.ferbNote, "")
    }

    // MARK: The session driver (HAND-707..709)

    /// FERB's keep-going question after the rounds the owner set: shown in the session as FERB's own card, answered with
    /// the choice through `sessions.answer` like the agent's.
    static let keepGoing: [String: Any] = [
        "id": "ferb-drive1-20", "session": "claude:abc", "kind": "ferb", "detail": "",
        "question": "I've gone 20 rounds on “polish the docs”. Keep going?",
        "choices": [["id": "keep_going", "label": "Keep going"], ["id": "stop", "label": "Stop"]],
        "asked_at": "2026-11-23T10:00:00+00:00"]

    // decision: HAND-708
    func testFERBsKeepGoingQuestionShowsInTheSessionAndKeepGoingAnswersIt() async throws {
        let rpc = HoldingRPC()
        let row = claude(delegate: "drive1", goal: "polish the docs", note: "FERB is waiting for your answer.")
        let model = try await opened(rpc, row)
        rpc.scripted.replies["sessions.timeline"] = ["session": row, "more": false, "cards": [Self.keepGoing], "items": [] as [Any]]
        await model.poll()
        let card = try XCTUnwrap(model.cards.first)
        XCTAssertEqual(card.kind, "ferb")
        XCTAssertEqual(card.heading(agentName: "Claude Code"), "FERB asks")
        XCTAssertEqual(card.question, "I've gone 20 rounds on “polish the docs”. Keep going?")
        XCTAssertEqual(card.choices.map(\.label), ["Keep going", "Stop"])
        lays(SessionCardView(card: card, agentName: "Claude Code", choose: { _ in }), width: 500, height: 200)
        rpc.scripted.replies["sessions.timeline"] = ["session": row, "more": false, "cards": [] as [Any], "items": [] as [Any]]
        await model.answer(card, card.choices[0])
        let sent = try XCTUnwrap(rpc.scripted.params(of: "sessions.answer").first)
        XCTAssertEqual(sent["card"] as? String, "ferb-drive1-20")
        XCTAssertEqual(sent["choice"] as? String, "keep_going")
        XCTAssertTrue(model.cards.isEmpty)
    }

    /// What FERB sent while it kept the session going is drawn on the owner's side, marked as FERB's.
    // decision: HAND-709
    func testAMessageFERBSentIsMarkedAsFERBs() throws {
        let sent = try XCTUnwrap(SessionItem(["seq": 6, "kind": "user", "text": "run the tests", "ferb": true]))
        XCTAssertEqual(sent.look(agent: "claude_code"), .ferbSent("run the tests"))
        XCTAssertEqual(SessionItem(["seq": 7, "kind": "user", "text": "hi"])?.look(agent: "claude_code"), .owner("hi"))
        lays(SessionItemView(item: sent, agent: "claude_code"), width: 500, height: 80)
    }

    // MARK: Replies in parts

    /// A reply modelled on the owner's Cursor screenshot (invented content).
    static let reply: [String: Any] = [
        "seq": 2, "kind": "assistant", "text": "This is a photo library export.",
        "parts": [["kind": "thinking", "label": "Thought 1s"],
                  ["kind": "text", "text": "I'll look through this folder and tell you what it is."],
                  ["kind": "tool", "label": "List zip sizes and contents", "detail": "ls, echo, unzip, head",
                   "text": "  Length  Name\n    4096  albums/index.json"],
                  ["kind": "text", "text": "This is a **photo library export**.\n\n| File | What's inside |\n| --- | --- |\n| `albums-000.zip` | 14 albums. |\n| `photos-000.zip` | About 2,100 photos. |\n\n- Open `photos-000.zip` first.\n- The other one is small.\n\n```\nunzip -l photos-000.zip\n```"],
                  ["kind": "image", "text": "An unknown kind reads as text."],
                  ["kind": "meta", "text": "Just now"]]]

    // decision: HAND-703
    func testAReplysPartsAreReadAndDrawnAsOneBlock() throws {
        let item = try XCTUnwrap(SessionItem(Self.reply))
        XCTAssertEqual(item.parts.map(\.kind), ["thinking", "text", "tool", "text", "image", "meta"])
        guard case .reply(let parts) = item.look(agent: "app_chat") else { return XCTFail("one reply block") }
        XCTAssertEqual(parts, item.parts)
        // a thought: a small row by its label, opening only when it has words
        XCTAssertEqual(parts[0].rowLabel, "Thought 1s")
        XCTAssertFalse(parts[0].expandable)
        XCTAssertTrue(SessionPart(kind: "thinking", text: "Look at the zips").expandable)
        // a step: its label, then what it ran, collapsed; its output opens in a fixed-width block
        XCTAssertEqual(parts[2].rowLabel, "List zip sizes and contents")
        XCTAssertEqual(parts[2].rowDetail, "ls, echo, unzip, head")
        XCTAssertTrue(parts[2].expandable)
        XCTAssertFalse(SessionPart(kind: "tool", label: "Read a file").expandable, "no output: no chevron")
        // the text is Markdown: a real table, a list, code
        let blocks = MarkdownDocument(parts[3].text).blocks
        XCTAssertTrue(blocks.contains(.table(["File", "What's inside"],
                                             [["`albums-000.zip`", "14 albums."], ["`photos-000.zip`", "About 2,100 photos."]],
                                             [.leading, .leading])))
        XCTAssertTrue(blocks.contains(.listItem("Open `photos-000.zip` first.", "•", 0)))
        XCTAssertTrue(blocks.contains(.code("unzip -l photos-000.zip", "")))
        let inline = MarkdownDocument.inline("Open `photos-000.zip` first.")
        XCTAssertTrue(inline.runs.contains { $0.inlinePresentationIntent?.contains(.code) == true })
        XCTAssertEqual(parts[4].shown, "text")
        XCTAssertEqual(parts[5].shown, "meta")
        lays(SessionItemView(item: item, agent: "app_chat"))
        lays(SessionReplyView(parts: parts + [SessionPart(kind: "tool", text: "out", label: "Run", detail: "ls")]))
    }

    // decision: HAND-703
    func testAnItemWithoutPartsIsMarkdownExceptATerminalsOutput() throws {
        let words = try XCTUnwrap(SessionItem(["seq": 3, "kind": "assistant", "text": "| a | b |\n| --- | --- |\n| 1 | 2 |"]))
        XCTAssertEqual(words.look(agent: "claude_code"), .reply([SessionPart(kind: "text", text: words.text)]))
        XCTAssertEqual(words.look(agent: "codex"), .reply([SessionPart(kind: "text", text: words.text)]))
        XCTAssertEqual(words.look(agent: "terminal"), .fixedWidth(words.text))
        XCTAssertEqual(SessionItem(["seq": 4, "kind": "user", "text": "**hi**"])?.look(agent: "codex"), .owner("**hi**"))
        XCTAssertEqual(SessionItem(["seq": 5, "kind": "tool", "tool": "Bash", "text": "npm test"])?.look(agent: "claude_code"),
                       .toolLine("Bash: npm test"))
        lays(SessionItemView(item: words, agent: "terminal"))
        lays(SessionItemView(item: words, agent: "claude_code"))
    }

    // decision: HAND-703
    func testAReplyWhosePartsGrowIsReplacedAndDrawnAgain() async throws {
        let rpc = HoldingRPC()
        let app: [String: Any] = ["id": "chat:1", "agent": "app_chat", "host": ["name": "Cursor"], "send_paths": ["foreground"]]
        let model = try await opened(rpc, app)
        var growing = Self.reply
        growing["seq"] = 2
        growing["parts"] = [["kind": "meta", "text": "Working…"]]
        rpc.scripted.replies["sessions.timeline"] = ["session": app, "more": false, "cards": [] as [Any], "items": [growing]]
        await model.poll()
        let first = SessionTranscript.rows(items: model.items, cards: [], outgoing: nil, loadingEarlier: false)
        rpc.scripted.replies["sessions.timeline"] = ["session": app, "more": false, "cards": [] as [Any], "items": [Self.reply]]
        await model.poll()
        XCTAssertEqual(model.items.map(\.seq), [1, 2])
        XCTAssertEqual(model.items.last?.parts.count, 6)
        let second = SessionTranscript.rows(items: model.items, cards: [], outgoing: nil, loadingEarlier: false)
        XCTAssertNotEqual(first, second, "the row is measured and drawn again")
    }

    // MARK: New session

    func testStartIsPendingAtOnceAndOpensTheNewSession() async throws {
        let rpc = HoldingRPC()
        let model = SessionsModel(rpc: rpc)
        rpc.scripted.replies["sessions.prepare"] = ["folders": ["/Users/you/code/site"], "host": "Terminal"]
        await model.prepareNew()
        XCTAssertEqual(model.startFolder, "/Users/you/code/site")
        rpc.held = ["sessions.start"]
        rpc.scripted.replies["sessions.start"] = ["started": true, "command": "", "session": "term:new", "host": "Terminal"]
        let new: [String: Any] = ["id": "term:new", "agent": "terminal", "program": "zsh", "cwd": "/Users/you/code/site",
                                  "host": ["name": "Terminal"], "send_paths": ["terminal_script"]]
        rpc.scripted.replies["sessions.list"] = ["enabled": true, "sessions": [new]]
        rpc.scripted.replies["sessions.timeline"] = ["session": new, "more": false, "cards": [] as [Any], "items": [] as [Any]]
        let tapped = Task { await model.planStart(folder: model.startFolder, command: "") }
        try await rpc.waitForHeldCall()
        XCTAssertTrue(model.starting, "pending from the tap")
        lays(NewSessionView(sessions: model, close: {}))
        rpc.held = []
        rpc.release()
        let outcome = await tapped.value
        XCTAssertEqual(outcome, .opened)
        XCTAssertFalse(model.starting)
        XCTAssertEqual(model.openID, "term:new")
        XCTAssertEqual(model.openSession?.heading, "site")
        XCTAssertNil(model.awaitingNewLine)
    }

    func testAStartWithoutItsSessionYetSaysItWillShowUntilItDoes() async throws {
        let rpc = HoldingRPC()
        let model = SessionsModel(rpc: rpc)
        rpc.scripted.replies["sessions.list"] = ["enabled": true, "sessions": [SessionsTests.claude]]
        await model.load()
        rpc.scripted.replies["sessions.start"] = ["started": true, "command": "", "session": "", "host": "Terminal"]
        let outcome = await model.planStart(folder: "/Users/you/code/site", command: "")
        XCTAssertEqual(outcome, .opened)
        XCTAssertNil(model.openID)
        XCTAssertEqual(model.awaitingNewLine, "Your new terminal will show here in a moment.")
        let store = FERBWindowStore(rpc: rpc)
        lays(SessionsPage(store: store, sessions: model))
        await model.load(quiet: true)
        XCTAssertNotNil(model.awaitingNewLine, "still not listed")
        rpc.scripted.replies["sessions.list"] = ["enabled": true, "sessions": [SessionsTests.claude, ["id": "term:new", "agent": "terminal"]]]
        await model.load(quiet: true)
        XCTAssertNil(model.awaitingNewLine)
    }

    func testARefusedStartStaysWithTheMacsReasonUnderStart() async throws {
        let rpc = HoldingRPC()
        let model = SessionsModel(rpc: rpc)
        rpc.scripted.failures["sessions.start"] = DaemonRPCError.remote("FERB couldn't open Terminal.", code: SessionRefusal.sendFailed)
        let outcome = await model.planStart(folder: "/Users/you/code/site", command: "")
        XCTAssertEqual(outcome, .refused)
        XCTAssertEqual(model.startProblem, "FERB couldn't open Terminal.")
        XCTAssertNil(model.problem)
        XCTAssertFalse(model.starting)
        lays(NewSessionView(sessions: model, close: {}))
        // after the Yes to a command: the form stays when the Mac refuses
        rpc.scripted.replies["sessions.prepare"] = ["needs_yes": ["question": "Open a new Terminal window in site and type this?",
                                                                  "detail": "agy", "covers": "…"]]
        let asks = await model.planStart(folder: "/Users/you/code/site", command: "agy")
        XCTAssertEqual(asks, .asks)
        let opened = await model.confirmYes()
        XCTAssertFalse(opened)
        XCTAssertEqual(model.startProblem, "FERB couldn't open Terminal.")
    }
}
