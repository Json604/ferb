import FerbAppModel
import FerbCore
import FerbDesign
import FerbMobile
import XCTest

/// The owner's coding sessions on the iPhone (kb HAND-651; Android's SessionsTest): the list and its words, turning them on
/// (a signed settings change), one session's pages and its poll, a send that asks first and goes as `sessionControl`
/// with the same words and a signer, Stop its turn on a session this phone doesn't control, the agent's question answered
/// by its choice's id, Hand to FERB, a new session, Stop controlling, the window (never on the public relay), and the
/// screens kept out of captures and dropped at the lock.
@MainActor
final class SessionsTests: XCTestCase {
    private var rig: Rig!
    private var phone: FakePhone { rig.phone }
    private let id = "claude:abc"

    override func setUp() async throws {
        rig = Rig()
        rig.session.unlocked()
        rig.session.foreground(true)
        phone.view = emptyView()
        phone.codingList = SessionsList(enabled: true, sessions: [
            session(id, title: "Fix the flaky login test", status: "waiting_for_you", host: "Terminal"),
            session("codex:t-2", title: "", folder: "/Users/owner/code/recipes", status: "working", host: "", withFerb: true),
        ])
    }
    override func tearDown() async throws { rig.tearDown() }

    private func session(_ id: String, title: String, folder: String = "/Users/owner/code/garden-app", status: String = "idle",
                         host: String = "Terminal", withFerb: Bool = false, canSend: Bool = true) -> AgentSession {
        AgentSession(id: id, agent: id.hasPrefix("codex") ? "codex" : "claude_code", agentName: id.hasPrefix("codex") ? "Codex" : "Claude Code",
                     title: title, folder: folder, folderName: (folder as NSString).lastPathComponent, status: status,
                     waiting: status == "waiting_for_you" ? "permission" : "none", host: host, canSend: canSend,
                     noSend: canSend ? "" : "No app on your Mac shows this session.", foreground: host == "Terminal",
                     lastAt: "2026-01-02T10:00:00Z", lastText: "", controlledHere: false, control: "", withFerb: withFerb, goal: "", terminals: [])
    }

    private func items(_ range: ClosedRange<UInt64>) -> [SessionItem] {
        range.map { n in SessionItem(seq: n, at: "2026-01-02T10:00:00Z", kind: n % 3 == 0 ? "tool" : (n % 2 == 0 ? "assistant" : "user"),
                                     text: "line \(n)", tool: n % 3 == 0 ? "Bash" : "") }
    }

    private func opened(_ vm: AppModel) async {
        await vm.loadSessions().value
        await vm.openSession(id).value
    }

    // ---- the list ---------------------------------------------------------------------------------------------------

    // decision: HAND-666, PHN-112, PHN-133
    // decision: PHN-798
    func testOffUntilTurnedOnAndTurnOnIsASignedSettingsChange() async {
        phone.codingList.enabled = false
        let vm = rig.model()
        vm.choose(.sessions)
        await eventually("the list read") { vm.coding.list != nil }
        XCTAssertEqual(vm.coding.list?.enabled, false, "the tab shows the off card")
        XCTAssertTrue(rig.prompts.signers.isEmpty, "reading asks nothing")
        phone.codingList.enabled = true
        await vm.turnOnSessions().value
        XCTAssertTrue(phone.calls.contains("set:sessions.enabled=true"), "the setting, through the settings path")
        XCTAssertEqual(rig.prompts.signers.last?.kind, .strong, "the Face ID / Touch ID key signs it")
        XCTAssertEqual(rig.prompts.whys.last, .setting)
        XCTAssertEqual(phone.signed.count, 1)
        XCTAssertEqual(vm.coding.list?.enabled, true, "read again once it is on")
        XCTAssertEqual(vm.coding.list?.sessions.count, 2)
    }

    // decision: PHN-797
    func testARowSaysItsTitleOrFolderWhereItRunsItsStateAndFERBIsOnIt() {
        let a = SessionWords.row(phone.codingList.sessions[0])
        XCTAssertEqual(a.title, "Fix the flaky login test")
        XCTAssertEqual(a.subtitle, "garden-app · in Terminal")
        XCTAssertEqual(a.state, .waiting)
        XCTAssertEqual(a.stateText, L.s("sessions_waiting"))
        XCTAssertNil(a.ferb)
        let b = SessionWords.row(phone.codingList.sessions[1])
        XCTAssertEqual(b.title, "recipes", "no title yet: the folder's name")
        XCTAssertEqual(b.subtitle, "recipes · " + L.s("sessions_in_background"))
        XCTAssertEqual(b.stateText, L.s("sessions_working"))
        XCTAssertEqual(b.ferb, L.s("sessions_with_ferb"))
        XCTAssertEqual(SessionWords.stateText(session("x", title: "", status: "idle")), L.s("sessions_idle"))
    }

    // decision: PHN-797
    func testTheListPollsOnlyWhileTheTabShowsUnlockedAndInFront() async {
        let vm = rig.model()
        await vm.sessionsTick()
        XCTAssertFalse(phone.calls.contains("sessions"), "the Chat tab: no poll")
        vm.choose(.sessions)
        await eventually("the first read") { vm.coding.list != nil }
        let n = phone.calls.filter { $0 == "sessions" }.count
        await vm.sessionsTick()
        XCTAssertEqual(phone.calls.filter { $0 == "sessions" }.count, n + 1)
        rig.session.foreground(false)
        await vm.sessionsTick()
        XCTAssertEqual(phone.calls.filter { $0 == "sessions" }.count, n + 1, "away from the screen: nothing")
    }

    // ---- one session -------------------------------------------------------------------------------------------------

    // decision: PHN-797
    func testASessionOpensOnItsNewestPageShowsEarlierAndPollsAfterItsLast() async {
        phone.codingItems[id] = items(1...60)
        let vm = rig.model()
        await opened(vm)
        XCTAssertEqual(vm.sheet, .session(id: id))
        XCTAssertEqual(vm.coding.open?.items.map(\.seq), Array(11...60), "the newest 50")
        XCTAssertEqual(vm.coding.open?.more, true)
        // 11: the owner's, 12: a tool row (one fixed-width line), 14: the agent's
        XCTAssertEqual(SessionWords.line(vm.coding.open!.items[0]), SessionLine("11", .owner, "line 11"))
        XCTAssertEqual(SessionWords.line(vm.coding.open!.items[1]), SessionLine("12", .tool, "line 12"))
        XCTAssertEqual(SessionWords.line(vm.coding.open!.items[3]), SessionLine("14", .agent, "line 14"))
        await vm.sessionEarlier().value
        XCTAssertTrue(phone.calls.contains("timeline:\(id):0:11"), "Show earlier asks before the first one shown")
        XCTAssertEqual(vm.coding.open?.items.map(\.seq), Array(1...60))
        XCTAssertEqual(vm.coding.open?.more, false)
        phone.codingItems[id]! += [SessionItem(seq: 61, at: "", kind: "interrupted", text: "", tool: "")]
        await vm.sessionsTick()
        XCTAssertTrue(phone.calls.contains("timeline:\(id):60:0"), "the poll asks after the last one")
        XCTAssertEqual(vm.coding.open?.items.last?.seq, 61)
        XCTAssertEqual(SessionWords.line(vm.coding.open!.items.last!), SessionLine("61", .interrupted, L.s("sessions_interrupted")))
        vm.closeSession()
        XCTAssertNil(vm.coding.open, "closed: its conversation goes and nothing polls it")
    }

    // decision: GATE-622, PHN-112
    func testASendAsksFirstThenGoesAsControlWithTheSameWordsSigned() async {
        let vm = rig.model()
        await opened(vm)
        vm.sessionTyped("Run the login test ten times")
        await vm.sendToSession().value
        XCTAssertEqual(vm.sheet, .sessionYes, "the Mac's question first")
        guard case .send(let s, let text, let yes)? = vm.coding.asking else { return XCTFail("asks to send") }
        XCTAssertEqual(s, id)
        XCTAssertEqual(text, "Run the login test ten times")
        XCTAssertEqual(yes.detail, "Run the login test ten times", "the exact text")
        XCTAssertTrue(phone.codingSent.isEmpty, "nothing sent before his Yes")
        XCTAssertTrue(rig.prompts.signers.isEmpty)
        await vm.sessionYes().value
        XCTAssertTrue(phone.calls.contains("session-control:\(id):Run the login test ten times"))
        XCTAssertEqual(phone.codingSent.map(\.1), ["Run the login test ten times"])
        XCTAssertEqual(rig.prompts.signers.count, 1, "one prompt for the Yes")
        XCTAssertEqual(phone.signed.count, 1)
        XCTAssertEqual(vm.sheet, .session(id: id), "back to the session")
        XCTAssertEqual(vm.coding.draft, "", "the box empties")
        XCTAssertNil(vm.coding.asking)
        XCTAssertEqual(vm.coding.open?.session?.controlledHere, true)
        // controlled from here now: the next send goes without asking; not seen arriving yet: said
        phone.codingDelivered = false
        vm.sessionTyped("And then commit")
        await vm.sendToSession().value
        XCTAssertEqual(phone.codingSent.map(\.1).last, "And then commit")
        XCTAssertEqual(rig.prompts.signers.count, 1, "no second prompt")
        XCTAssertEqual(vm.ui.note, Note("sessions_not_seen"))
    }

    // decision: PHN-800
    func testASendTheMacRefusesAfterTheYesShowsAsNotSentWithWhyAndGoesOnTryAgain() async {
        let why = "FERB couldn't tell that the terminal in front in Code is this session's, so it typed nothing."
        let vm = rig.model()
        await opened(vm)
        vm.sessionTyped("Run the login test ten times")
        await vm.sendToSession().value
        phone.codingControlRefusal = .Refused(detail: why, linkCode: "not_the_session")
        await vm.sessionYes().value
        XCTAssertNil(vm.coding.asking, "the question closes: never stuck on the Yes")
        XCTAssertEqual(vm.sheet, .session(id: id))
        XCTAssertEqual(vm.coding.open?.unsent?.text, "Run the login test ten times")
        XCTAssertEqual(vm.coding.open?.unsent?.reason.text, why)
        XCTAssertEqual(vm.coding.draft, "", "the words left the box for the not-sent bubble")
        XCTAssertTrue(phone.codingSent.isEmpty, "nothing shows as sent")
        // Try again once the Mac can send: it goes, and the not-sent bubble leaves
        await vm.retrySessionSend().value
        XCTAssertEqual(phone.codingSent.map(\.1), ["Run the login test ten times"])
        XCTAssertNil(vm.coding.open?.unsent)
        // controlled now: a direct send the Mac refuses shows as not sent too
        phone.codingRefusal = .Refused(detail: why, linkCode: "not_the_session")
        vm.sessionTyped("And then commit")
        await vm.sendToSession().value
        XCTAssertEqual(vm.coding.open?.unsent?.text, "And then commit")
    }

    // decision: HAND-683
    func testWhenTheMacCantTellTheTerminalHeChoosesOneSignedOrDoesntSend() async {
        let why = "FERB couldn't tell which terminal in Code is this session's, so it typed nothing. Choose it, and FERB types only there."
        let offered = [SessionChoice(id: "front", label: "The terminal in front in Code now (Terminal 1, 2.1.330)"),
                       SessionChoice(id: "w7:1:Terminal 2, 2.1.330", label: "main.rs — app · Terminal 2, 2.1.330")]
        if let i = phone.codingList.sessions.firstIndex(where: { $0.id == id }) { phone.codingList.sessions[i].terminals = offered }
        let vm = rig.model()
        await opened(vm)
        vm.sessionTyped("Run the login test ten times")
        await vm.sendToSession().value
        phone.codingControlRefusal = .Refused(detail: why, linkCode: "choose_terminal")
        await vm.sessionYes().value
        XCTAssertNil(vm.coding.asking, "the Yes closes")
        XCTAssertEqual(vm.sheet, .sessionPick, "the terminals to choose from")
        XCTAssertEqual(vm.coding.open?.pick?.terminals, offered)
        XCTAssertEqual(vm.coding.open?.pick?.why, why)
        XCTAssertEqual(vm.coding.open?.unsent?.text, "Run the login test ten times")
        XCTAssertTrue(phone.codingSent.isEmpty, "nothing typed")
        // Don't send: back to the session, the words stay as not sent
        vm.pickNoTerminal()
        XCTAssertEqual(vm.sheet, .session(id: id))
        XCTAssertNil(vm.coding.open?.pick)
        // Try again: the Mac still can't tell, so he is asked again; his choice, signed as control is: the words go there
        phone.codingRefusal = .Refused(detail: why, linkCode: "choose_terminal")
        await vm.retrySessionSend().value
        XCTAssertEqual(vm.sheet, .sessionPick)
        let prompts = rig.prompts.signers.count
        await vm.pickTerminal("w7:1:Terminal 2, 2.1.330").value
        XCTAssertTrue(phone.calls.contains("session-control:\(id):Run the login test ten times:terminal=w7:1:Terminal 2, 2.1.330"))
        XCTAssertEqual(rig.prompts.signers.count, prompts + 1)
        XCTAssertEqual(phone.codingSent.map(\.1), ["Run the login test ten times"])
        XCTAssertEqual(vm.sheet, .session(id: id))
        XCTAssertNil(vm.coding.open?.unsent)
    }

    // decision: GATE-622
    func testCancelSendsNothingAndKeepsTheWords() async {
        let vm = rig.model()
        await opened(vm)
        vm.sessionTyped("Delete the build folder")
        await vm.sendToSession().value
        vm.sessionNo()
        XCTAssertEqual(vm.sheet, .session(id: id))
        XCTAssertNil(vm.coding.asking)
        XCTAssertEqual(vm.coding.draft, "Delete the build folder")
        XCTAssertFalse(phone.calls.contains { $0.hasPrefix("session-control") })
        XCTAssertTrue(phone.codingSent.isEmpty)
    }

    // decision: GATE-622, PHN-797
    func testStopItsTurnOnASessionNotControlledHereAsksThenStopsIt() async {
        let vm = rig.model()
        await opened(vm)
        await vm.stopSessionTurn().value
        XCTAssertEqual(vm.sheet, .sessionYes, "refused: the same Yes, with nothing to type")
        guard case .interrupt(let s, let yes)? = vm.coding.asking else { return XCTFail("asks to control") }
        XCTAssertEqual(s, id)
        XCTAssertEqual(yes.detail, "")
        XCTAssertTrue(phone.calls.contains("session-send:\(id):"), "the Mac's question for an empty text")
        await vm.sessionYes().value
        XCTAssertTrue(phone.calls.contains("session-control:\(id):"))
        XCTAssertEqual(phone.calls.filter { $0 == "session-interrupt:\(id)" }.count, 2, "refused, then stopped")
        XCTAssertEqual(vm.ui.note, Note("sessions_interrupted"))
        XCTAssertTrue(phone.codingSent.isEmpty, "nothing typed")
    }

    // decision: HAND-663
    func testTheAgentsQuestionIsAnsweredWithItsChoicesIdSigned() async {
        phone.codingCards[id] = [SessionCard(id: "card-1", kind: "permission", tool: "Bash", detail: "npm test", question: "",
                                             choices: [SessionChoice(id: "allow", label: "Yes"), SessionChoice(id: "deny", label: "No")])]
        let vm = rig.model()
        await opened(vm)
        let card = SessionWords.ask(vm.coding.open!.cards[0], agent: "Claude Code")
        XCTAssertEqual(card.title, L.s("sessions_wants_tool", "Claude Code", "Bash"))
        XCTAssertEqual(card.detail, "npm test")
        XCTAssertEqual(card.choices, ["Yes", "No"])
        await vm.answerSession(card: "card-1", choice: "deny").value
        XCTAssertTrue(phone.calls.contains("session-answer:\(id):card-1:deny"), "the choice's id, never its label")
        XCTAssertEqual(rig.prompts.signers.count, 1, "signed")
        XCTAssertEqual(phone.signed.count, 1)
        XCTAssertTrue(vm.coding.open?.cards.isEmpty ?? false)
        // a question with no buttons is answered on the Mac
        let none = SessionWords.ask(SessionCard(id: "c", kind: "question", tool: "", detail: "", question: "Which branch?", choices: []),
                                    agent: "Codex")
        XCTAssertEqual(none.title, L.s("sessions_asks", "Codex"))
        XCTAssertEqual(none.onMac, L.s("sessions_answer_on_mac"))
        XCTAssertTrue(none.choices.isEmpty, "no buttons: the card says to answer it on the Mac")
    }

    // decision: ORCH-687
    func testHandToFERBTakesTheGoalSigned() async {
        let vm = rig.model()
        await opened(vm)
        vm.askHandToFerb()
        XCTAssertEqual(vm.sheet, .sessionHand(id: id))
        await vm.handToFerb(goal: "  ").value
        XCTAssertFalse(phone.calls.contains { $0.hasPrefix("session-delegate") }, "no goal: nothing handed over")
        await vm.handToFerb(goal: "Keep going until the tests pass").value
        XCTAssertTrue(phone.calls.contains("session-delegate:\(id):Keep going until the tests pass"))
        XCTAssertEqual(phone.signed.count, 1)
        XCTAssertEqual(vm.ui.note, Note("sessions_handed"))
        XCTAssertEqual(vm.sheet, .session(id: id))
    }

    // decision: HAND-665
    func testANewSessionShowsTheCommandAndStartsOnlyAfterTheYes() async {
        let vm = rig.model()
        await vm.newSession().value
        XCTAssertEqual(vm.sheet, .sessionNew)
        XCTAssertEqual(vm.coding.new?.folders, phone.codingFolders)
        XCTAssertEqual(vm.coding.new?.folder, phone.codingFolders[0], "the first folder is chosen")
        vm.newSessionFolder("/Users/owner/code/recipes")
        vm.newSessionText("Tidy the README")
        await vm.startSession().value
        XCTAssertTrue(phone.calls.contains("session-prepare:claude_code:/Users/owner/code/recipes:Tidy the README"))
        XCTAssertEqual(vm.sheet, .sessionYes)
        XCTAssertEqual(vm.coding.asking?.yes.detail, "cd '/Users/owner/code/recipes' && claude 'Tidy the README'", "the exact command")
        XCTAssertFalse(phone.calls.contains { $0.hasPrefix("session-start") }, "nothing started before the Yes")
        vm.sessionNo()
        XCTAssertEqual(vm.sheet, .sessionNew, "Cancel: back to the choices")
        XCTAssertEqual(vm.coding.new?.text, "Tidy the README", "kept")
        vm.newSessionAgent("codex")
        vm.newSessionText("")
        await vm.startSession().value
        await vm.sessionYes().value
        XCTAssertTrue(phone.calls.contains("session-start:codex:/Users/owner/code/recipes:"))
        XCTAssertEqual(phone.signed.count, 1)
        XCTAssertEqual(vm.ui.note, Note("sessions_started"))
        XCTAssertNil(vm.sheet)
        XCTAssertNil(vm.coding.new)
    }

    // decision: GATE-622
    func testStopControllingLetsGo() async {
        phone.controlled = [id]
        let vm = rig.model()
        await opened(vm)
        XCTAssertEqual(vm.coding.open?.session?.controlledHere, true)
        await vm.releaseSession().value
        XCTAssertTrue(phone.calls.contains("session-release:\(id)"))
        XCTAssertEqual(vm.coding.open?.session?.controlledHere, false)
        XCTAssertTrue(rig.prompts.signers.isEmpty, "letting go asks nothing")
    }

    // ---- its window, captures and the lock -----------------------------------------------------------------------------

    // decision: PHN-454, PHN-444, PHN-797
    func testTheWindowIsNeverAskedForOnThePublicRelayAndIsKeptOutOfCaptures() async {
        let vm = rig.model()
        await opened(vm)
        // PHN-800: the session's conversation follows the owner's switch; only its window live is kept out always
        XCTAssertFalse(vm.mustSecure, "the session's conversation follows the owner's switch")
        phone.link = FakePhone.status(path: .publicRelay)
        vm.watchCodingSession()
        XCTAssertEqual(vm.ui.note, Note("not_on_shared_relay"), "Watch live's words")
        XCTAssertFalse(phone.calls.contains { $0.hasPrefix("watch-session") })
        XCTAssertEqual(vm.sheet, .session(id: id))
        phone.link = FakePhone.status(path: .direct)
        vm.watchCodingSession()
        XCTAssertEqual(vm.sheet, .sessionWatch(id: id))
        XCTAssertTrue(vm.mustSecure)
        await eventually("the preview asked for") { phone.calls.contains("watch-session:\(id)") }
        vm.closeLive()
        XCTAssertEqual(vm.sheet, .session(id: id), "back to the session")
        await eventually("the preview stopped") { phone.previewStopped == 1 }
    }

    // decision: PHN-15, GATE-622
    func testTheLockClosesTheSessionAndDropsWhatWasRead() async {
        let vm = rig.model()
        await opened(vm)
        vm.sessionTyped("half a thought")
        rig.session.lockNow()
        await eventually("closed") { vm.sheet == nil }
        XCTAssertNil(vm.coding.list)
        XCTAssertNil(vm.coding.open)
        XCTAssertEqual(vm.coding.draft, "")
    }
}

/// Every word the sessions' screens name is in the catalog (made from Android's strings.xml): no ⟦key⟧ on screen.
final class SessionsCatalogTests: XCTestCase {
    // decision: PHN-567
    func testEveryKeyTheSessionsScreensNameIsInTheCatalog() {
        let keys = ["sessions_title", "sessions_off", "sessions_off_more", "sessions_turn_on", "sessions_none", "sessions_working",
                    "sessions_waiting", "sessions_idle", "sessions_with_ferb", "sessions_in_app", "sessions_in_background", "sessions_new",
                    "sessions_message_hint", "sessions_send", "sessions_watch", "sessions_hand_over", "sessions_stop_turn", "sessions_let_go",
                    "sessions_older", "sessions_wants_tool", "sessions_asks", "sessions_answer_on_mac", "sessions_yes_send", "sessions_cancel",
                    "sessions_not_seen", "sessions_interrupted", "sessions_hand_title", "sessions_hand_label", "sessions_hand_hint",
                    "sessions_hand_note", "sessions_hand_go", "sessions_handed", "sessions_new_agent", "sessions_new_folder",
                    "sessions_new_first", "sessions_start", "sessions_started", "sessions_watch_title", "live_close", "live_again",
                    "live_waiting", "err_try_again", "sessions_more", "sessions_not_sent", "sessions_try_again", "sessions_pick_title", "sessions_pick_note",
                    "sessions_pick_no"]
        XCTAssertEqual(keys.filter { !L.has($0) }, [], "keys the sessions' screens name but the catalog lacks")
    }
}
