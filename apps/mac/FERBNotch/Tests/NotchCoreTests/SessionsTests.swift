import AppKit
import SwiftUI
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// Sessions in FERB.app: the daemon's `sessions.*` rows read leniently; each button is one call; whatever would send or
/// run something shows the daemon's question first and acts only on the owner's Yes, with exactly the text he read.
@MainActor
final class SessionsTests: XCTestCase {
    static let claude: [String: Any] = [
        "id": "claude:abc", "agent": "claude_code", "cwd": "/Users/o/code/site", "title": "Tidy the styles",
        "status": "working", "waiting": "none",
        "host": ["bundle_id": "com.apple.Terminal", "name": "Terminal", "pid": 412, "via": "direct", "tty": "/dev/ttys004"],
        "send_paths": ["terminal_script", "foreground"], "last_at": "2026-11-22T10:00:00+00:00",
        "last_text": "Running the tests", "control": "mac", "delegate": "task-9", "goal": "Make the tests pass",
        "yours": true]
    static let codex: [String: Any] = [
        "id": "codex:def", "agent": "codex", "cwd": "/Users/o/code/notes", "title": "", "status": "waiting_for_you",
        "waiting": "permission", "host": NSNull(), "send_paths": [] as [Any],
        "no_send": "FERB can't reach this session's window.", "last_at": "2026-11-22T09:00:00.123456+00:00",
        "last_text": "", "yours": false]
    static let card: [String: Any] = [
        "id": "card-1", "session": "codex:def", "kind": "permission", "tool": "shell",
        "detail": "npm install --save-dev vitest", "asked_at": "2026-11-22T09:00:00+00:00", "approval_id": "ap-1",
        "choices": [["id": "approved", "label": "Yes"], ["id": "denied", "label": "No"]]]

    // MARK: Rows

    func testASessionRowReadsEveryField() throws {
        let row = try XCTUnwrap(AgentSessionRow(Self.claude))
        XCTAssertEqual(row.id, "claude:abc")
        XCTAssertEqual(row.agentName, "Claude Code")
        XCTAssertEqual(row.folderName, "site")
        XCTAssertEqual(row.heading, "Tidy the styles")
        XCTAssertEqual(row.host?.name, "Terminal")
        XCTAssertEqual(row.host?.pid, 412)
        XCTAssertEqual(row.host?.tty, "/dev/ttys004")
        XCTAssertEqual(row.sendPaths, ["terminal_script", "foreground"])
        XCTAssertTrue(row.canSend)
        XCTAssertFalse(row.foreground)
        XCTAssertNotNil(row.lastAt)
        XCTAssertEqual(row.control, "mac")
        XCTAssertTrue(row.withFerb)
        XCTAssertEqual(row.goal, "Make the tests pass")
        XCTAssertTrue(row.yours)
        XCTAssertTrue(row.whereLine.contains("site") && row.whereLine.contains("Terminal"))
    }

    func testAMissingFieldReadsEmptyAndARowWithoutAnIdIsDropped() throws {
        let row = try XCTUnwrap(AgentSessionRow(Self.codex))
        XCTAssertEqual(row.heading, "notes")                         // no title: the folder's name
        XCTAssertNil(row.host)
        XCTAssertFalse(row.canSend)
        XCTAssertEqual(row.noSend, "FERB can't reach this session's window.")
        XCTAssertNotNil(row.lastAt)                                  // microseconds
        XCTAssertFalse(row.withFerb)
        XCTAssertNotEqual(row.whereLine, AgentSessionRow(Self.claude)?.whereLine)
        let bare = try XCTUnwrap(AgentSessionRow(["id": "codex:x"]))
        XCTAssertEqual(bare.waiting, "none")
        XCTAssertTrue(bare.sendPaths.isEmpty)
        XCTAssertNil(bare.lastAt)
        XCTAssertFalse(bare.yours)
        XCTAssertNil(AgentSessionRow(["agent": "codex", "cwd": "/x"]))
        XCTAssertNil(AgentSessionRow(["id": ""]))
        let states = ["working", "waiting_for_you", "idle"].compactMap { AgentSessionRow(["id": "a", "status": $0])?.stateWords }
        XCTAssertEqual(Set(states).count, 3)
    }

    func testATimelineReadsItsItemsInOrderAndItsCards() throws {
        let timeline = SessionTimeline([
            "session": Self.codex, "more": true, "cards": [Self.card, ["kind": "question"]],
            "items": [["seq": 7, "kind": "tool", "tool": "Bash", "text": "npm test", "at": "2026-11-22T10:00:00+00:00"],
                      ["seq": 5, "kind": "user", "text": "Run the tests"], ["kind": "assistant", "text": "no seq"]]])
        XCTAssertEqual(timeline.session?.id, "codex:def")
        XCTAssertTrue(timeline.more)
        XCTAssertEqual(timeline.items.map(\.seq), [5, 7])
        XCTAssertTrue(timeline.items[1].line.contains("Bash") && timeline.items[1].line.contains("npm test"))
        XCTAssertEqual(timeline.items[0].line, "Run the tests")
        XCTAssertEqual(timeline.cards.map(\.id), ["card-1"])
        let card = timeline.cards[0]
        XCTAssertEqual(card.choices.map(\.id), ["approved", "denied"])
        XCTAssertEqual(card.approvalID, "ap-1")
        XCTAssertTrue(card.heading(agentName: "Codex").contains("shell"))
        XCTAssertEqual(SessionTimeline([:]), SessionTimeline(["items": [] as [Any], "cards": [] as [Any]]))
        XCTAssertNil(SessionYes(["sent": true]))
        XCTAssertEqual(SessionYes(["needs_yes": ["question": "Q", "detail": "D", "covers": "C"]]),
                       SessionYes(question: "Q", detail: "D", covers: "C"))
    }

    // MARK: Flows

    private func listed(_ rpc: ScriptedRPC) async throws -> SessionsModel {
        rpc.replies["sessions.list"] = ["enabled": true, "sessions": [Self.claude, Self.codex, ["title": "no id"]]]
        rpc.replies["sessions.timeline"] = ["session": Self.claude, "more": false, "cards": [] as [Any],
                                            "items": [["seq": 1, "kind": "user", "text": "hi"]]]
        let model = SessionsModel(rpc: rpc)
        await model.load()
        XCTAssertEqual(model.sessions.map(\.id), ["claude:abc", "codex:def"])
        XCTAssertEqual(model.enabled, true)
        await model.open(try XCTUnwrap(model.sessions.first))
        XCTAssertEqual(model.items.map(\.seq), [1])
        return model
    }

    // decision: APP-1112
    // decision: GATE-622
    func testSendAsksFirstAndTheYesSendsExactlyThatText() async throws {
        let rpc = ScriptedRPC()
        let model = try await listed(rpc)
        rpc.replies["sessions.send"] = ["needs_yes": ["question": "Send this to Claude Code in Terminal?",
                                                      "detail": "run the linter", "covers": "Yes lets you send…"]]
        rpc.replies["sessions.control"] = ["sent": true, "path": "terminal_script", "delivered": false, "seconds": 2,
                                           "control": "mac"]
        model.draft = "run the linter\n"
        await model.send()
        XCTAssertEqual(rpc.params(of: "sessions.send").last?["text"] as? String, "run the linter")
        XCTAssertTrue(rpc.params(of: "sessions.control").isEmpty)       // nothing sent before the Yes
        let asked = try XCTUnwrap(model.confirm)
        XCTAssertEqual(asked.yes.detail, "run the linter")
        await model.confirmYes()
        XCTAssertEqual(rpc.params(of: "sessions.control").first?["text"] as? String, "run the linter")
        XCTAssertEqual(rpc.params(of: "sessions.control").first?["session"] as? String, "claude:abc")
        XCTAssertNil(model.confirm)
        XCTAssertEqual(model.draft, "")
        XCTAssertNotNil(model.note)                                     // delivered=false is said
        // the poll after it asks only for what is new
        XCTAssertEqual(rpc.params(of: "sessions.timeline").last?["after"] as? Int, 1)
    }

    func testCancelSendsNothing() async throws {
        let rpc = ScriptedRPC()
        let model = try await listed(rpc)
        rpc.replies["sessions.send"] = ["needs_yes": ["question": "Q", "detail": "x", "covers": ""]]
        model.draft = "x"
        await model.send()
        model.cancelConfirm()
        XCTAssertNil(model.confirm)
        XCTAssertTrue(rpc.params(of: "sessions.control").isEmpty)
        XCTAssertEqual(model.draft, "x")                                // kept for the owner to change
    }

    func testACardsButtonAnswersWithThatChoice() async throws {
        let rpc = ScriptedRPC()
        let model = try await listed(rpc)
        rpc.queue("sessions.timeline", [["session": Self.codex, "items": [] as [Any], "more": false, "cards": [Self.card]]])
        await model.poll()
        let card = try XCTUnwrap(model.cards.first)
        rpc.replies["sessions.timeline"] = ["items": [] as [Any], "more": false, "cards": [] as [Any]]
        await model.answer(card, card.choices[1])
        let sent = try XCTUnwrap(rpc.params(of: "sessions.answer").first)
        XCTAssertEqual(sent["session"] as? String, "codex:def")
        XCTAssertEqual(sent["card"] as? String, "card-1")
        XCTAssertEqual(sent["choice"] as? String, "denied")
        XCTAssertTrue(model.cards.isEmpty)
    }

    func testHandToFERBPassesTheChatsClientID() async throws {
        let rpc = ScriptedRPC()
        let model = try await listed(rpc)
        model.clientID = { "client-7" }
        rpc.replies["sessions.delegate"] = ["handed_over": true, "message": "FERB is on it."]
        let handed = await model.delegate(goal: "  Keep going until the tests pass ")
        XCTAssertTrue(handed)
        let sent = try XCTUnwrap(rpc.params(of: "sessions.delegate").first)
        XCTAssertEqual(sent["session"] as? String, "claude:abc")
        XCTAssertEqual(sent["goal"] as? String, "Keep going until the tests pass")
        XCTAssertEqual(sent["client_id"] as? String, "client-7")
        XCTAssertNotNil(model.note)
        let empty = await model.delegate(goal: "  ")
        XCTAssertFalse(empty)
        XCTAssertEqual(rpc.params(of: "sessions.delegate").count, 1)
    }

    func testStopItsTurnWithoutControlAsksFirstThenInterrupts() async throws {
        let rpc = ScriptedRPC()
        let model = try await listed(rpc)
        rpc.failures["sessions.interrupt"] = DaemonRPCError.remote("Say Yes to sending to this session first.",
                                                                  code: SessionRefusal.notTheSession)
        rpc.replies["sessions.send"] = ["needs_yes": ["question": "Send this to Claude Code in Terminal?", "detail": "",
                                                      "covers": "Yes lets you send…"]]
        await model.interrupt()
        XCTAssertEqual(rpc.params(of: "sessions.send").first?["text"] as? String, "")
        XCTAssertNotNil(model.confirm)
        XCTAssertNil(model.problem)
        rpc.failures["sessions.interrupt"] = nil
        rpc.replies["sessions.interrupt"] = ["interrupted": true]
        rpc.replies["sessions.control"] = ["control": "mac"]
        await model.confirmYes()
        let order = rpc.methods().filter { ["sessions.control", "sessions.interrupt"].contains($0) }
        XCTAssertEqual(order, ["sessions.interrupt", "sessions.control", "sessions.interrupt"])
        XCTAssertEqual(rpc.params(of: "sessions.control").first?["text"] as? String, "")
        XCTAssertNotNil(model.note)
    }

    func testARefusalShowsTheDaemonsWords() async throws {
        let rpc = ScriptedRPC()
        let model = try await listed(rpc)
        rpc.failures["sessions.send"] = DaemonRPCError.remote("Claude Code is asking something. Answer that first.",
                                                             code: SessionRefusal.cardOpen)
        model.draft = "go on"
        await model.send()
        XCTAssertEqual(model.problem, "Claude Code is asking something. Answer that first.")
        XCTAssertNil(model.confirm)
    }

    func testShowEarlierPagesBackFromTheFirstItemHeld() async throws {
        let rpc = ScriptedRPC()
        rpc.replies["sessions.list"] = ["enabled": true, "sessions": [Self.claude]]
        rpc.queue("sessions.timeline", [["items": [["seq": 51, "kind": "user", "text": "b"]], "more": true, "cards": [] as [Any]],
                                        ["items": [["seq": 50, "kind": "assistant", "text": "a"]], "more": false, "cards": [] as [Any]]])
        let model = SessionsModel(rpc: rpc)
        await model.load()
        await model.open(try XCTUnwrap(model.sessions.first))
        XCTAssertTrue(model.more)
        await model.showEarlier()
        XCTAssertEqual(rpc.params(of: "sessions.timeline").last?["before"] as? Int, 51)
        XCTAssertEqual(model.items.map(\.seq), [50, 51])
        XCTAssertFalse(model.more)
    }

    func testANewSessionShowsTheExactCommandAndStartsOnlyOnYes() async throws {
        let rpc = ScriptedRPC()
        let model = SessionsModel(rpc: rpc)
        rpc.replies["sessions.prepare"] = ["folders": ["/Users/o/code/site"], "agents": ["claude_code", "codex"], "host": "Terminal"]
        await model.prepareNew()
        XCTAssertEqual(model.folders, ["/Users/o/code/site"])
        rpc.replies["sessions.prepare"] = ["folders": ["/Users/o/code/site"], "agents": ["claude_code", "codex"],
                                           "needs_yes": ["question": "Start Codex in a new Terminal window?",
                                                         "detail": "cd '/Users/o/code/site' && codex 'add tests'", "covers": "…"]]
        let planned = await model.planStart(agent: "codex", folder: "/Users/o/code/site", text: " add tests ")
        XCTAssertTrue(planned)
        let asked = try XCTUnwrap(rpc.params(of: "sessions.prepare").last)
        XCTAssertEqual(asked["agent"] as? String, "codex")
        XCTAssertEqual(asked["text"] as? String, "add tests")
        XCTAssertEqual(model.confirm?.yes.detail, "cd '/Users/o/code/site' && codex 'add tests'")
        XCTAssertTrue(rpc.params(of: "sessions.start").isEmpty)
        rpc.replies["sessions.start"] = ["started": true, "command": "…"]
        await model.confirmYes()
        let started = try XCTUnwrap(rpc.params(of: "sessions.start").first)
        XCTAssertEqual(started["agent"] as? String, "codex")
        XCTAssertEqual(started["folder"] as? String, "/Users/o/code/site")
        XCTAssertEqual(started["text"] as? String, "add tests")
    }

    func testTurnOnWritesTheSwitchAndReadsTheListAgain() async throws {
        let rpc = ScriptedRPC()
        rpc.queue("sessions.list", [["enabled": false, "sessions": [] as [Any]]])
        rpc.replies["sessions.list"] = ["enabled": true, "sessions": [Self.claude]]
        rpc.replies["settings.set"] = ["ok": true, "value": true]
        let store = FERBWindowStore(rpc: rpc)
        await store.refresh(.sessions)
        XCTAssertEqual(store.sessions.enabled, false)
        await store.turnOnSessions()
        let set = try XCTUnwrap(rpc.params(of: "settings.set").first)
        XCTAssertEqual(set["key"] as? String, "sessions.enabled")
        XCTAssertEqual(set["value"] as? Bool, true)
        XCTAssertEqual(store.sessions.enabled, true)
        XCTAssertEqual(store.sessions.sessions.count, 1)
        // Settings → Privacy's switch writes the same key
        store.toggle("sessions.enabled").wrappedValue = false
        for _ in 0..<50 where rpc.params(of: "settings.set").count < 2 { await Task.yield() }
        XCTAssertEqual(rpc.params(of: "settings.set").last?["key"] as? String, "sessions.enabled")
        XCTAssertEqual(rpc.params(of: "settings.set").last?["value"] as? Bool, false)
    }

    func testTheDaemonsSessionEventsReachThePage() async throws {
        let model = NotchModel(rpc: ScriptedRPC(), preferences: scratchDefaults("ferb-sessions-events"))
        final class Heard: @unchecked Sendable { var names: [String?] = [] }
        let heard = Heard()
        let observer = NotificationCenter.default.addObserver(forName: .ferbSessionsChanged, object: nil, queue: nil) {
            heard.names.append($0.object as? String)
        }
        defer { NotificationCenter.default.removeObserver(observer) }
        model.receiveBusEvent(["topic": "sessions.changed", "data": ["count": 2]])
        model.receiveBusEvent(["topic": "sessions.card", "data": ["session": "codex:def", "card": "c"]])
        model.receiveBusEvent(["topic": "sessions.card_gone", "data": ["session": "codex:def", "card": "c"]])
        model.receiveBusEvent(["topic": "sessions.finished", "data": ["session": "claude:abc"]])
        model.receiveBusEvent(["topic": "timer.changed", "data": [:] as [String: Any]])
        XCTAssertEqual(heard.names, [nil, "codex:def", "codex:def", "claude:abc"])
    }

    // MARK: Uninstall

    // decision: APP-5
    // decision: HAND-666
    func testUninstallTakesTheHookOutBeforeTheServiceIsTurnedOffAndAFailureDoesntStopIt() async throws {
        let rpc = ScriptedRPC()
        rpc.replies["sessions.remove_hooks"] = ["removed": true]
        let removed = await SessionsModel.removeHooks(rpc: rpc)
        XCTAssertTrue(removed)
        XCTAssertEqual(rpc.methods(), ["sessions.remove_hooks"])
        rpc.failures["sessions.remove_hooks"] = DaemonRPCError.unavailable
        let failed = await SessionsModel.removeHooks(rpc: rpc)
        XCTAssertFalse(failed)

        var order: [String] = []
        var packaged = FERBAppActions(feedback: SoundFeedback(), cursor: CursorController(makePanels: false))
        packaged.removeBrowserFiles = { order.append("browser files"); return BrowserFilesRemoval(lines: [], done: true) }
        packaged.removeSessionHooks = { order.append("hooks"); _ = await SessionsModel.removeHooks(rpc: rpc) }
        packaged.beforeUninstall = { order.append("service off"); return nil }
        packaged.runFerb = { arguments, _, done in order.append(arguments.joined(separator: " ")); done(0); return true }
        let output = FerbOutput()
        output.uninstall(with: packaged)
        for _ in 0..<200 where output.running { await Task.yield() }
        await Task.yield()
        XCTAssertEqual(order, ["browser files", "hooks", "service off", "uninstall --skip-browser-files"])
    }

    // MARK: Layout

    private func lays(_ view: some View, width: CGFloat = 700, height: CGFloat = 600) {
        let hosting = NSHostingView(rootView: view.frame(width: width, height: height))
        hosting.frame = NSRect(x: 0, y: 0, width: width, height: height)
        hosting.layout()
        XCTAssertGreaterThan(hosting.fittingSize.width, 0)
        XCTAssertGreaterThan(hosting.fittingSize.height, 0)
    }

    // decision: APP-1112
    func testThePageLaysOutInEachState() async throws {
        let rpc = ScriptedRPC()
        let store = FERBWindowStore(rpc: rpc)
        lays(SessionsPage(store: store, sessions: store.sessions))                          // not read yet
        rpc.queue("sessions.list", [["enabled": false, "sessions": [] as [Any]],
                                    ["enabled": true, "sessions": [] as [Any]]])
        rpc.replies["sessions.list"] = ["enabled": true, "sessions": [Self.claude, Self.codex]]
        for _ in 0..<3 {
            await store.sessions.load()
            lays(SessionsPage(store: store, sessions: store.sessions))                      // off, none, two
        }
        rpc.replies["sessions.timeline"] = ["session": Self.codex, "more": true, "cards": [Self.card, ["id": "q", "kind": "question",
                                            "question": "Which file?", "detail": "", "choices": [] as [Any]]],
                                            "items": [["seq": 1, "kind": "user", "text": "Add tests"],
                                                      ["seq": 2, "kind": "assistant", "text": "On it."],
                                                      ["seq": 3, "kind": "tool", "tool": "Bash", "text": "npm test"],
                                                      ["seq": 4, "kind": "interrupted", "text": ""],
                                                      ["seq": 5, "kind": "error", "text": "API error"]]]
        await store.sessions.open(try XCTUnwrap(store.sessions.sessions.last))             // no composer: no_send
        lays(SessionsPage(store: store, sessions: store.sessions))
        store.sessions.close()
        await store.sessions.open(try XCTUnwrap(store.sessions.sessions.first))            // a composer
        lays(SessionsPage(store: store, sessions: store.sessions))

        let row = try XCTUnwrap(store.sessions.openSession)
        var sheet: SessionsSheet? = .handOver
        let binding = Binding(get: { sheet }, set: { sheet = $0 })
        for shown in [SessionsSheet.handOver, .start, .watch(row)] {
            sheet = shown
            lays(SessionsSheetView(sessions: store.sessions, previewAccess: PreviewAccess(), sheet: binding))
        }
        lays(SessionConfirmView(confirm: .init(yes: SessionYes(question: "Send this to Claude Code in Terminal?",
                                                               detail: "run the linter", covers: "Yes lets you send…"),
                                               action: .send(session: row.id, text: "run the linter")),
                                yes: {}, cancel: {}))
    }

    func testTheSessionsSectionIsInTheWindow() {
        XCTAssertTrue(FERBSection.allCases.contains(.sessions))
        XCTAssertFalse(FERBSection.sessions.title.isEmpty)
        XCTAssertNotNil(NSImage(systemSymbolName: FERBSection.sessions.symbol, accessibilityDescription: nil))
    }
}
