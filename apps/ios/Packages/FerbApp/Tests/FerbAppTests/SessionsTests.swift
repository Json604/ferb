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
        ], noChats: [])
    }
    override func tearDown() async throws { rig.tearDown() }

    private func session(_ id: String, title: String, folder: String = "/Users/owner/code/garden-app", status: String = "idle",
                         host: String = "Terminal", withFerb: Bool = false, canSend: Bool = true, ferbTask: String = "",
                         ferbNote: String = "") -> AgentSession {
        AgentSession(id: id, agent: id.hasPrefix("codex") ? "codex" : "claude_code", agentName: id.hasPrefix("codex") ? "Codex" : "Claude Code",
                     title: title, folder: folder, folderName: (folder as NSString).lastPathComponent, status: status,
                     waiting: status == "waiting_for_you" ? "permission" : "none", host: host, canSend: canSend,
                     noSend: canSend ? "" : "No app on your Mac shows this session.", foreground: host == "Terminal",
                     lastAt: "2026-01-02T10:00:00Z", lastText: "", controlledHere: false, control: "", withFerb: withFerb, goal: "", terminals: [],
                     program: "", window: "", noText: "", ferbTask: ferbTask, ferbNote: ferbNote)
    }

    private func items(_ range: ClosedRange<UInt64>) -> [SessionItem] {
        range.map { n in SessionItem(seq: n, at: "2026-01-02T10:00:00Z", kind: n % 3 == 0 ? "tool" : (n % 2 == 0 ? "assistant" : "user"),
                                     text: "line \(n)", tool: n % 3 == 0 ? "Bash" : "", parts: []) }
    }

    private func opened(_ vm: AppModel) async {
        await vm.loadSessions().value
        await vm.openSession(id).value
    }

    // ---- the list ---------------------------------------------------------------------------------------------------

    // decision: HAND-696, PHN-112, PHN-133
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

    /// A row's name is never empty (owner 2026-11-23: "in the background · Idle" with no name): its title, else its
    /// window's words, else its folder's name, else its agent's name.
    // decision: PHN-797
    func testARowsNameFallsBackToTheAgentsNameAndIsNeverEmpty() {
        var s = session("terminal:1", title: "", folder: "")
        s.agent = "terminal"
        s.agentName = "Terminal"
        XCTAssertEqual(SessionWords.title(s), "Terminal")
        s.window = "zsh — 80×24"
        XCTAssertEqual(SessionWords.title(s), "zsh — 80×24")
        s.window = ""
        s.folderName = "garden-app"
        XCTAssertEqual(SessionWords.title(s), "garden-app")
        s.title = "Fix the test"
        XCTAssertEqual(SessionWords.title(s), "Fix the test")
        s = session("x:1", title: "", folder: "")
        s.agentName = ""
        XCTAssertFalse(SessionWords.title(s).isEmpty, "the agent's id at the very least")
    }

    /// The menu offers Hand to FERB while FERB doesn't have it, Take it back while it does, and "FERB is starting…"
    /// (no tap) between the two.
    // decision: HAND-702, HAND-705
    func testTheMenuOffersHandToFERBTakeItBackOrFERBIsStarting() {
        XCTAssertEqual(L.s("sessions_take_back"), "Take it back")
        XCTAssertEqual(L.s("sessions_ferb_starting"), "FERB is starting…")
        XCTAssertEqual(L.s("sessions_no_chat", "Codex"), "No agent chat open in Codex. Start one there to continue here.")
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
        phone.codingItems[id]! += [SessionItem(seq: 61, at: "", kind: "interrupted", text: "", tool: "", parts: [])]
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

    // ---- what he sees while it goes (owner 2026-11-23, PHN-812) -----------------------------------------------------------

    // decision: PHN-812, PHN-814
    func testASendShowsAtOnceAsSendingThenAsSentAndNeverTwice() async {
        phone.controlled.insert(id)
        phone.codingEchoes = false
        let vm = rig.model()
        await opened(vm)
        vm.sessionTyped("Run the login test ten times")
        let sending = vm.sendToSession()
        // the instant he taps Send, before the Mac answers: the box is empty, his words show as sending
        XCTAssertEqual(vm.coding.draft, "", "the box clears at once")
        XCTAssertEqual(vm.coding.open?.outgoing?.text, "Run the login test ten times")
        XCTAssertEqual(vm.coding.open?.outgoing?.state, .sending)
        XCTAssertNil(vm.coding.open?.unsent, "never as not sent while it goes")
        await sending.value
        XCTAssertEqual(phone.codingSent.map(\.1), ["Run the login test ten times"])
        XCTAssertEqual(vm.coding.open?.outgoing?.state, .sent, "sent: his bubble stays until the conversation has it")
        // the transcript carries it: the bubble goes, so it never shows twice
        let n = UInt64((phone.codingItems[id]?.last?.seq ?? 0) + 1)
        phone.codingItems[id, default: []].append(SessionItem(seq: n, at: "2026-01-02T10:00:00Z", kind: "user",
                                                              text: "Run the login test ten times", tool: "", parts: []))
        await vm.sessionsTick()
        XCTAssertNil(vm.coding.open?.outgoing)
        XCTAssertEqual(vm.coding.open?.items.filter { $0.kind == "user" && $0.text == "Run the login test ten times" }.count, 1)
        // when the transcript has it by the time the send answers, no own bubble is left at all
        phone.codingEchoes = true
        vm.sessionTyped("And then commit")
        await vm.sendToSession().value
        XCTAssertNil(vm.coding.open?.outgoing)
        XCTAssertEqual(vm.coding.open?.items.filter { $0.text == "And then commit" }.count, 1)
    }

    // decision: PHN-812, PHN-800
    func testASendThatFailsGoesFromSendingToNotSentWithItsWordsKept() async {
        phone.controlled.insert(id)
        let why = "FERB couldn't tell that the terminal in front in Code is this session's, so it typed nothing."
        let vm = rig.model()
        await opened(vm)
        phone.codingRefusal = .Refused(detail: why, linkCode: "not_the_session")
        vm.sessionTyped("Run the login test ten times")
        let sending = vm.sendToSession()
        XCTAssertEqual(vm.coding.open?.outgoing?.state, .sending)
        await sending.value
        XCTAssertEqual(vm.coding.open?.unsent?.text, "Run the login test ten times", "the words kept, as not sent")
        XCTAssertEqual(vm.coding.open?.unsent?.reason.text, why)
        XCTAssertTrue(phone.codingSent.isEmpty)
        // Try again shows it as sending again at once, then sent
        let again = vm.retrySessionSend()
        XCTAssertEqual(vm.coding.open?.outgoing?.state, .sending)
        XCTAssertNil(vm.coding.open?.unsent)
        await again.value
        XCTAssertEqual(phone.codingSent.map(\.1), ["Run the login test ten times"])
        XCTAssertNil(vm.coding.open?.unsent)
    }

    // decision: PHN-812, GATE-622
    func testUnderTheMacsQuestionItStaysSendingYesSendsItCancelPutsItBack() async {
        let vm = rig.model()
        await opened(vm)
        vm.sessionTyped("Delete the build folder")
        await vm.sendToSession().value
        XCTAssertEqual(vm.sheet, .sessionYes)
        XCTAssertEqual(vm.coding.open?.outgoing?.state, .sending, "nothing sent yet, but never a frozen box")
        vm.sessionNo()
        XCTAssertNil(vm.coding.open?.outgoing, "Cancel: nothing was sent, no bubble")
        XCTAssertEqual(vm.coding.draft, "Delete the build folder", "his words back in the box")
        // again, and Yes: it goes
        await vm.sendToSession().value
        XCTAssertEqual(vm.coding.draft, "")
        await vm.sessionYes().value
        XCTAssertEqual(phone.codingSent.map(\.1), ["Delete the build folder"])
        XCTAssertNil(vm.coding.open?.outgoing, "the conversation carries it now")
        XCTAssertEqual(vm.sheet, .session(id: id))
    }

    // decision: PHN-813
    func testTheConversationHasADaysStartBetweenDaysAsTheMainChat() {
        func item(_ seq: UInt64, _ at: String) -> SessionItem { SessionItem(seq: seq, at: at, kind: "assistant", text: "line \(seq)", tool: "", parts: []) }
        var utc = Calendar(identifier: .gregorian)
        utc.timeZone = TimeZone(identifier: "UTC")!
        let today = ChatModel.day(ISO8601DateFormatter().date(from: "2026-01-02T10:00:00Z")!, utc)
        let label = { (d: Int) in d == today ? "Today" : "Day \(d)" }
        let onlyToday = SessionWords.lines([item(1, "2026-01-02T09:00:00Z"), item(2, "2026-01-02T10:00:00Z")], today: today, calendar: utc, label: label)
        XCTAssertEqual(onlyToday.map(\.kind), [.agent, .agent], "today's alone: no day's start, as today's chat")
        let two = SessionWords.lines([item(1, "2026-01-01T22:00:00.250Z"), item(2, "garbled"), item(3, "2026-01-02T09:00:00Z")],
                                     today: today, calendar: utc, label: label)
        XCTAssertEqual(two.map(\.kind), [.day, .agent, .agent, .day, .agent])
        XCTAssertEqual(two.map(\.text), ["Day \(today - 1)", "line 1", "line 2", "Today", "line 3"])
        XCTAssertEqual(Set(two.map(\.key)).count, two.count, "every line keeps its own key")
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

    // decision: ORCH-687, HAND-702
    func testHandToFERBTakesTheGoalSigned() async {
        let vm = rig.model()
        await opened(vm)
        vm.askHandToFerb()
        XCTAssertEqual(vm.sheet, .sessionHand(id: id))
        await vm.handToFerb(goal: "  ").value
        XCTAssertFalse(phone.calls.contains { $0.hasPrefix("session-delegate") }, "no goal: nothing handed over")
        XCTAssertFalse(vm.coding.handing)
        let reads = phone.calls.filter { $0 == "sessions" }.count
        let hand = vm.handToFerb(goal: "Keep going until the tests pass")
        XCTAssertTrue(vm.coding.handing, "at the tap: Handing it to FERB…")
        await hand.value
        XCTAssertFalse(vm.coding.handing)
        XCTAssertTrue(phone.calls.contains("session-delegate:\(id):Keep going until the tests pass"))
        XCTAssertEqual(phone.signed.count, 1)
        XCTAssertEqual(vm.ui.note, Note("sessions_handed"))
        XCTAssertEqual(vm.sheet, .session(id: id), "the sheet closes")
        XCTAssertGreaterThan(phone.calls.filter { $0 == "sessions" }.count, reads, "the session read again at once")
    }

    /// A hand-over the Mac refuses keeps the sheet and says the Mac's reason under the button (owner 2026-11-23).
    // decision: HAND-702, ORCH-687
    func testAHandOverTheMacRefusesKeepsTheSheetWithItsReason() async {
        let why = "FERB is already on another session in this folder."
        phone.delegateRefusal = .Refused(detail: why, linkCode: "ferb_error")
        let vm = rig.model()
        await opened(vm)
        vm.askHandToFerb()
        await vm.handToFerb(goal: "Keep going").value
        XCTAssertEqual(vm.sheet, .sessionHand(id: id), "the sheet stays, his words in it")
        XCTAssertFalse(vm.coding.handing)
        XCTAssertEqual(vm.coding.handRefused?.text, why)
        XCTAssertNil(vm.ui.note)
        vm.askHandToFerb()
        XCTAssertNil(vm.coding.handRefused, "opened again: nothing old said")
    }

    /// FERB has the session: the menu's Take it back stops FERB's task by its id (the task stop every client has), says
    /// "Taking it back…" at once, closes the menu when done and reads the session again; a refusal is said (owner
    /// 2026-11-23). Between the hand-over and FERB's task the row says FERB is starting and takes no tap.
    // decision: HAND-702
    func testTakeItBackStopsFERBsTaskByItsIdWithItsPendingStateAndItsFailureSaid() async {
        phone.codingList.sessions[0] = session(id, title: "Fix the flaky login test", status: "working", withFerb: true, ferbTask: "t-9",
                                               ferbNote: "FERB is on it: “Keep going”")
        let vm = rig.model()
        await opened(vm)
        XCTAssertEqual(vm.coding.open?.session?.ferbTask, "t-9")
        vm.sessionMenu(true)
        let take = vm.takeSessionBack()
        XCTAssertEqual(vm.coding.open?.takingBack, true, "at the tap: Taking it back…")
        XCTAssertEqual(vm.coding.open?.menu, true, "the menu stays open for the result")
        let reads = phone.calls.filter { $0 == "sessions" }.count
        phone.codingList.sessions[0] = session(id, title: "Fix the flaky login test", status: "idle", ferbNote: "FERB stopped. The session is yours again.")
        await take.value
        XCTAssertTrue(phone.calls.contains("stop:t-9"), "the task stop, with FERB's task id")
        XCTAssertEqual(vm.coding.open?.takingBack, false)
        XCTAssertEqual(vm.coding.open?.menu, false, "done: the menu closes")
        XCTAssertGreaterThan(phone.calls.filter { $0 == "sessions" }.count, reads, "the session read again at once")
        XCTAssertEqual(vm.coding.open?.session?.ferbTask, "", "the badge goes when the Mac says so")
        XCTAssertEqual(vm.coding.open?.session?.ferbNote, "FERB stopped. The session is yours again.")
        // refused: said, and the row is Take it back again
        phone.codingList.sessions[0] = session(id, title: "Fix the flaky login test", status: "working", withFerb: true, ferbTask: "t-10")
        await vm.sessionsTick()
        phone.stopResult = .failed(.Refused(detail: "That task already finished.", linkCode: "ferb_error"))
        vm.sessionMenu(true)
        await vm.takeSessionBack().value
        XCTAssertEqual(vm.ui.note?.text, String(format: L.s("sessions_take_back_failed"), "That task already finished."))
        XCTAssertEqual(vm.ui.note?.text, "Couldn't take it back: That task already finished.")
        XCTAssertEqual(vm.coding.open?.takingBack, false)
    }

    // decision: HAND-691, HAND-695
    func testANewSessionIsATerminalAndOnlyItsCommandIsAskedAbout() async {
        let vm = rig.model()
        await vm.newSession().value
        XCTAssertEqual(vm.sheet, .sessionNew)
        XCTAssertEqual(vm.coding.new?.folders, phone.codingFolders)
        XCTAssertEqual(vm.coding.new?.chosen, phone.codingFolders[0], "the first folder is chosen")
        vm.newSessionFolder("/Users/owner/code/recipes")
        vm.newSessionText(" agy ")
        await vm.startSession().value
        XCTAssertTrue(phone.calls.contains("session-prepare:terminal:/Users/owner/code/recipes:agy"))
        XCTAssertEqual(vm.sheet, .sessionYes)
        XCTAssertEqual(vm.coding.asking?.yes.detail, "agy", "exactly the command")
        XCTAssertFalse(phone.calls.contains { $0.hasPrefix("session-start") }, "nothing started before the Yes")
        XCTAssertEqual(vm.coding.new?.starting, false, "the Mac's question shows, not the form's pending state")
        vm.sessionNo()
        XCTAssertEqual(vm.sheet, .sessionNew, "Cancel: back to the choices")
        XCTAssertEqual(vm.coding.new?.text, " agy ", "kept")
        // his Yes: back on the form, Start on its way, then the new terminal opens
        phone.startedSession = "term:300"
        phone.codingList.sessions.append(session("term:300", title: "", folder: "/Users/owner/code/recipes"))
        await vm.startSession().value
        await vm.sessionYes().value
        XCTAssertTrue(phone.calls.contains("session-start:terminal:/Users/owner/code/recipes:agy"))
        XCTAssertNil(vm.coding.new)
        XCTAssertEqual(vm.sheet, .session(id: "term:300"), "the new session opens at once")
        XCTAssertEqual(vm.coding.open?.id, "term:300")
    }

    /// Start at once says "Opening a terminal…" and the form waits (owner 2026-11-23); with no command it opens (signed,
    /// nothing asked); a terminal not shown yet: the list says so in one quiet line until it shows.
    // decision: PHN-822
    func testStartShowsItsPendingStateAndANewTerminalNotShownYetIsSaidUntilItShows() async {
        let vm = rig.model()
        vm.choose(.sessions)
        await eventually("the list read") { vm.coding.list != nil }
        await vm.newSession().value
        vm.newSessionFolder("/Users/owner/code/recipes")
        let start = vm.startSession()
        XCTAssertEqual(vm.coding.new?.starting, true, "at the tap, before the Mac answers")
        vm.newSessionFolder("/Users/owner/code/garden-app")
        XCTAssertEqual(vm.coding.new?.chosen, "/Users/owner/code/recipes", "the form takes no tap while it starts")
        vm.closeNewSession()
        XCTAssertEqual(vm.sheet, .sessionNew, "nor a close")
        await start.value
        XCTAssertTrue(phone.calls.contains("session-start:terminal:/Users/owner/code/recipes:"))
        XCTAssertEqual(phone.signed.count, 1)
        XCTAssertNil(vm.sheet, "the id was empty: back to the list")
        XCTAssertNil(vm.coding.new)
        XCTAssertTrue(vm.coding.soon, "one quiet line until it shows")
        XCTAssertNil(vm.ui.note, "no toast: the list says it")
        await vm.sessionsTick()
        XCTAssertTrue(vm.coding.soon, "a read without it: still said")
        phone.codingList.sessions.append(session("term:301", title: "", folder: "/Users/owner/code/recipes"))
        await vm.sessionsTick()
        XCTAssertFalse(vm.coding.soon, "it shows: the line goes")
    }

    /// A start the Mac refuses keeps the sheet and says why under Start (owner 2026-11-23).
    // decision: PHN-822
    func testAStartTheMacRefusesStaysOnTheSheetWithItsReason() async {
        let why = "There's no folder named recipes on your Mac any more."
        phone.startRefusal = .Refused(detail: why, linkCode: "ferb_error")
        let vm = rig.model()
        await vm.newSession().value
        vm.newSessionText("")
        await vm.startSession().value
        XCTAssertEqual(vm.sheet, .sessionNew, "the sheet stays")
        XCTAssertEqual(vm.coding.new?.starting, false)
        XCTAssertEqual(vm.coding.new?.refused?.text, why, "the Mac's words under Start")
        XCTAssertNil(vm.ui.note)
        // a prepare the Mac refuses says it the same way
        phone.codingRefusal = nil
        vm.newSessionFolder("/Users/owner/code/recipes")
        XCTAssertNil(vm.coding.new?.refused, "a new choice clears it")
    }

    /// "Another folder…" opens the Mac's Files browser, folders only; Use this folder puts the folder shown at the top of
    /// the list, chosen; Close goes back to the form as it was.
    // decision: PHN-821
    func testAnotherFolderOpensTheFilesBrowserForFoldersAndChoosesOne() async {
        phone.recent = RecentFiles(files: [
            FoundFile(name: "README.md", path: "/Users/owner/code/recipes/README.md", dir: false, size: 10, modified: 0, source: "fetched", at: nil, taskId: nil, taskName: nil),
            FoundFile(name: "blog", path: "/Users/owner/code/blog", dir: true, size: 0, modified: 0, source: "made", at: nil, taskId: nil, taskName: nil),
            FoundFile(name: "notes.txt", path: "/Users/owner/code/recipes/notes.txt", dir: false, size: 10, modified: 0, source: "fetched", at: nil, taskId: nil, taskName: nil),
        ], skipped: 0)
        phone.folders["~/Desktop"] = Folder(path: "/Users/owner/Desktop", parent: "/Users/owner", locked: false, entries: [
            FileEntry(name: "site", path: "/Users/owner/Desktop/site", dir: true, link: false, size: 0, modified: 0, locked: false),
            FileEntry(name: "photo.jpg", path: "/Users/owner/Desktop/photo.jpg", dir: false, link: false, size: 9, modified: 0, locked: false),
        ], total: 2, cut: false, from: 0, next: nil)
        phone.folders["/Users/owner/Desktop/site"] = Folder(path: "/Users/owner/Desktop/site", parent: "/Users/owner/Desktop", locked: false,
                                                             entries: [], total: 0, cut: false, from: 0, next: nil)
        let vm = rig.model()
        await vm.newSession().value
        let before = vm.coding.new
        await vm.pickNewSessionFolder()?.value
        XCTAssertEqual(vm.coding.new?.picker?.recent, ["/Users/owner/code/recipes", "/Users/owner/code/blog"],
                       "recent folders: the folders the recent files are in, and recent folders, each once")
        vm.pickerClose()
        XCTAssertEqual(vm.coding.new, before, "Close: the form as it was")
        await vm.pickNewSessionFolder()?.value
        await vm.pickerOpen("~/Desktop")?.value
        XCTAssertEqual(vm.coding.new?.picker?.folders.map(\.name), ["site"], "files hidden")
        await vm.pickerOpen("/Users/owner/Desktop/site")?.value
        XCTAssertEqual(vm.coding.new?.picker?.folder?.path, "/Users/owner/Desktop/site")
        XCTAssertEqual(vm.coding.new?.picker?.folders, [], "no folders in here")
        vm.pickerUse()
        XCTAssertNil(vm.coding.new?.picker, "back to the form")
        XCTAssertEqual(vm.coding.new?.chosen, "/Users/owner/Desktop/site", "the folder shown is chosen")
        XCTAssertEqual(vm.coding.new?.folders, ["/Users/owner/Desktop/site"] + phone.codingFolders, "at the top, the rest kept")
        XCTAssertEqual(vm.sheet, .sessionNew)
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

    // decision: PHN-454, PHN-444, PHN-797, PHN-810
    func testTheWindowIsNeverAskedForOnThePublicRelayAndFollowsTheScreenshotSwitch() async {
        let vm = rig.model()
        await opened(vm)
        // PHN-800, PHN-810: the session's conversation and its window live both follow the owner's switch
        XCTAssertFalse(vm.mustSecure, "the session's conversation follows the owner's switch")
        phone.link = FakePhone.status(path: .publicRelay)
        vm.watchCodingSession()
        XCTAssertEqual(vm.ui.note, Note("not_on_shared_relay"), "Watch live's words")
        XCTAssertFalse(phone.calls.contains { $0.hasPrefix("watch-session") })
        XCTAssertEqual(vm.sheet, .session(id: id))
        phone.link = FakePhone.status(path: .direct)
        vm.watchCodingSession()
        XCTAssertEqual(vm.sheet, .sessionWatch(id: id))
        XCTAssertFalse(vm.mustSecure, "its window live follows the switch too (PHN-810)")
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
                    "sessions_hand_note", "sessions_hand_go", "sessions_handed", "sessions_new_folder",
                    "sessions_new_browse", "sessions_new_command", "sessions_new_command_hint",
                    "sessions_no_chat", "sessions_start", "sessions_started", "sessions_watch_title", "live_close", "live_again",
                    "live_waiting", "err_try_again", "sessions_more", "sessions_not_sent", "sessions_try_again", "sessions_pick_title", "sessions_pick_note",
                    "sessions_pick_no", "sessions_take_back", "sessions_taking_back", "sessions_take_back_failed", "sessions_ferb_starting",
                    "sessions_hand_going", "sessions_opening", "sessions_coming", "sessions_browse_title", "sessions_browse_home",
                    "sessions_browse_use", "sessions_browse_no_folders", "files_recent", "files_back", "files_opening"]
        XCTAssertEqual(keys.filter { !L.has($0) }, [], "keys the sessions' screens name but the catalog lacks")
    }
}
