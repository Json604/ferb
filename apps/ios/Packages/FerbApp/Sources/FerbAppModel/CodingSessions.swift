import FerbCore
import FerbDesign
import FerbMobile
import Foundation
import Observation

/// One coding session open on the phone: what the Mac gave of its conversation (newest last), whether there is more
/// before it, the agent's open questions, and the session's own row as last read.
public struct OpenSession: Equatable {
    public let id: String
    public var session: AgentSession?
    public var items: [SessionItem] = []
    public var more = false
    public var cards: [SessionCard] = []
    /// The first page came back (until then the screen shows the row's words only).
    public var loaded = false
    /// A message the Mac refused (PHN-800): shown as not sent, with why and Try again; never as sent.
    public var unsent: Unsent?
    /// The session's menu is open (PHN-800).
    public var menu = false
    /// HAND-683: FERB can't tell which terminal is the session's: he chooses one for these words, or doesn't send.
    public var pick: TerminalPick?

    public init(id: String, session: AgentSession?) {
        self.id = id
        self.session = session
    }

    /// The newest item's number: the poll asks for what came after it.
    public var last: UInt64 { items.last?.seq ?? 0 }
    /// The oldest item's number: Show earlier asks for what came before it.
    public var first: UInt64 { items.first?.seq ?? 0 }
}

/// A message the Mac refused (PHN-800): his words and why, in the Mac's plain words.
public struct Unsent: Equatable {
    public let text: String
    public let reason: Note
    public init(text: String, reason: Note) { self.text = text; self.reason = reason }
}

/// HAND-683: the words waiting for his choice of terminal, the Mac's reason, and the terminals it can see.
public struct TerminalPick: Equatable {
    public let text: String
    public let why: String
    public let terminals: [SessionChoice]
    public init(text: String, why: String, terminals: [SessionChoice]) { self.text = text; self.why = why; self.terminals = terminals }
}

/// What waits for the owner's Yes before FERB types for him (rule 6, GATE-622, HAND-665): his words into a session, a stop
/// of its turn (FERB presses the agent's own interrupt key), or a new session's command.
public enum SessionConfirm: Equatable {
    case send(session: String, text: String, yes: SessionYes)
    case interrupt(session: String, yes: SessionYes)
    case start(agent: String, folder: String, text: String, yes: SessionYes)

    public var yes: SessionYes {
        switch self {
        case .send(_, _, let y), .interrupt(_, let y), .start(_, _, _, let y): y
        }
    }
}

/// A new session as the owner chooses it (HAND-665): the agent, the folder (the Mac's list; the first by default), the
/// first message.
public struct NewSessionDraft: Equatable {
    public var agent = CodingSessions.agents[0].id
    public var folder: String?
    public var text = ""
    /// The folders the Mac offers (its running sessions' first, then recent ones); empty while `loading`.
    public var folders: [String] = []
    public var loading = true
    public init() {}
}

/// The owner's coding sessions as the phone shows them (kb HAND-651; Android: model/Sessions.kt): the list, the session
/// open, what waits for his Yes, the new session's folders, the message box. Memory only: nothing here is kept on the
/// phone, and it all goes when the app locks.
@MainActor
@Observable
public final class CodingSessions {
    /// The list as last read (nil before it was read, or after the lock).
    public internal(set) var list: SessionsList?
    /// Why the list couldn't be read, said on the tab (a poll never puts up a note).
    public internal(set) var problem: Note?
    public internal(set) var open: OpenSession?
    public internal(set) var asking: SessionConfirm?
    /// A new session being chosen (its sheet and the Yes after it), or nil.
    public internal(set) var new: NewSessionDraft?
    /// The open session's message box.
    public internal(set) var draft = ""

    public init() {}

    /// The poll's pace while the tab shows (seconds) and while a session is open.
    public static let listEvery: Double = 3
    public static let openEvery: Double = 2
    /// The newest items a session's screen asks for at once, and per Show earlier.
    public static let page: UInt32 = 50
    /// At most this many items per poll after the newest one held (the Mac's own limit is 100).
    public static let afterPage: UInt32 = 100
    /// The agents a new session may run (the Mac's `agents`; their names are the agents' own).
    public static let agents: [(id: String, name: String)] = [("claude_code", "Claude Code"), ("codex", "Codex")]

    /// The app locked or forgot the Mac: nothing of the sessions stays.
    func locked() {
        list = nil
        problem = nil
        open = nil
        asking = nil
        new = nil
        draft = ""
    }

    /// The session's sheets went: its conversation goes too (nothing polls it).
    func closed() {
        open = nil
        draft = ""
    }

    /// A session's row as the list last had it.
    public func row(_ id: String) -> AgentSession? { list?.sessions.first { $0.id == id } }
}

extension AppModel {
    /// A coding session's sheets (the session, its Yes, the hand-over, its window).
    static func isSessionSheet(_ s: AppSheet?) -> Bool {
        switch s {
        case .session, .sessionYes, .sessionPick, .sessionHand, .sessionWatch: true
        default: false
        }
    }

    /// The open session's id, from the sheet on screen.
    var openSessionId: String? {
        switch sheet {
        case .session(let id), .sessionHand(let id), .sessionWatch(let id): id
        case .sessionYes, .sessionPick: coding.open?.id
        default: nil
        }
    }

    // ---- the list ---------------------------------------------------------------------------------------------------

    /// The Sessions tab was chosen: the list now (its unlock asked if the Mac's window closed).
    @discardableResult
    public func loadSessions() -> Task<Void, Never> {
        busy("sessions") { [self] in await readSessions(prompt: true) }
    }

    /// One read of the list. A poll (`prompt` false) never asks for the unlock: a closed window says so once, as the
    /// Tasks tab's read does.
    func readSessions(prompt: Bool) async {
        if session.locked || api.link?.phase != .online { return }
        let read = prompt ? await unlocker.run({ await api.sessionsList() }) : await api.sessionsList()
        switch read {
        case .ok(let l):
            coding.list = l
            coding.problem = nil
            if var o = coding.open, let s = l.sessions.first(where: { $0.id == o.id }) {
                o.session = s
                coding.open = o
            }
        case .failed(let e):
            if case .NeedsUnlock = e, !prompt { macLocked(); return }
            if case .NotSigned = e { return }
            coding.problem = Self.errorNote(e)
        }
    }

    /// One round of the polls (the screen calls it at its pace while it shows): the open session's new items while a
    /// session's sheet shows, else the list while the Sessions tab shows. Nothing while locked, away or offline.
    public func sessionsTick() async {
        if session.locked || !session.front || api.link?.phase != .online { return }
        if Self.isSessionSheet(sheet), coding.open != nil {
            await readTimeline(earlier: false)
        } else if tab == .sessions, sheet == nil {
            await readSessions(prompt: false)
        }
    }

    /// Turn on (HAND-666): FERB's own setting, signed like any change of FERB's settings (PHN-112, PHN-133); the Mac adds
    /// its hook to Claude Code's settings.
    @discardableResult
    public func turnOnSessions() -> Task<Void, Never> {
        busy("sessions-on", tap: true) { [self] in
            guard let signer = await freshSigner(.setting) else { return }
            switch await unlocker.run({ await api.setSetting(key: "sessions.enabled", valueJson: "true", signer: signer) }) {
            case .ok: await readSessions(prompt: true)
            case .failed(let e): if case .NotSigned = e {} else { failed(e) }
            }
        }
    }

    // ---- one session ------------------------------------------------------------------------------------------------

    /// Opens a session: its newest page, then its new items while it shows.
    @discardableResult
    public func openSession(_ id: String) -> Task<Void, Never> {
        coding.open = OpenSession(id: id, session: coding.row(id))
        coding.draft = ""
        show(.session(id: id))
        return busy("session-open") { [self] in await readTimeline(earlier: false, prompt: true) }
    }

    /// Show earlier: the page before the oldest item shown.
    @discardableResult
    public func sessionEarlier() -> Task<Void, Never> {
        busy("session-earlier", tap: true) { [self] in await readTimeline(earlier: true, prompt: true) }
    }

    /// The first page, what came after the newest item (a page at a time while the Mac says there is more after it), or
    /// (earlier) the page before the oldest.
    func readTimeline(earlier: Bool, prompt: Bool = false) async {
        guard let o = coding.open else { return }
        if earlier {
            if !o.more || o.items.isEmpty { return }
            let first = o.first
            switch await unlocker.run({ await api.sessionTimeline(o.id, after: 0, before: first, limit: CodingSessions.page) }) {
            case .ok(let t):
                guard var now = coding.open, now.id == o.id else { return }
                let held = now.first
                now.items = t.items.filter { $0.seq < held }.sorted { $0.seq < $1.seq } + now.items
                now.more = t.more
                coding.open = now
            case .failed(let e): if case .NotSigned = e {} else { failed(e) }
            }
            return
        }
        if !o.loaded {
            let call = { [api] in await api.sessionTimeline(o.id, after: 0, before: 0, limit: CodingSessions.page) }
            switch prompt ? await unlocker.run(call) : await call() {
            case .ok(let t):
                guard var now = coding.open, now.id == o.id else { return }
                if let s = t.session { now.session = s }
                now.items = t.items.sorted { $0.seq < $1.seq }
                now.more = t.more
                now.cards = t.cards
                now.loaded = true
                coding.open = now
            case .failed(let e): readFailed(e, prompt: prompt)
            }
            return
        }
        for _ in 0..<5 {
            guard let held = coding.open, held.id == o.id else { return }
            switch await api.sessionTimeline(o.id, after: held.last, before: 0, limit: CodingSessions.afterPage) {
            case .ok(let t):
                guard var now = coding.open, now.id == o.id else { return }
                if let s = t.session { now.session = s }
                let last = now.last
                now.items += t.items.filter { $0.seq > last }.sorted { $0.seq < $1.seq }
                now.cards = t.cards
                coding.open = now
                if !t.more || t.items.isEmpty { return }
            case .failed(let e):
                readFailed(e, prompt: false)
                return
            }
        }
    }

    /// A read of the open session that failed: a poll says nothing (a closed window is the card's), the owner's own tap does.
    private func readFailed(_ e: FerbError, prompt: Bool) {
        if case .NeedsUnlock = e, !prompt { macLocked(); return }
        if case .NotSigned = e { return }
        if prompt { failed(e) }
    }

    public func sessionTyped(_ text: String) { coding.draft = text }

    /// Send: the Mac sends it when this phone controls the session, else asks first with the exact text (nothing was
    /// sent); his Yes is [sessionYes].
    @discardableResult
    public func sendToSession() -> Task<Void, Never> {
        busy("session-send", tap: true) { [self] in
            guard let id = coding.open?.id else { return }
            await sendWords(id, coding.draft)
        }
    }

    /// Try again on a message the Mac refused (PHN-800): the same words, the same way (asked first if not controlled).
    @discardableResult
    public func retrySessionSend() -> Task<Void, Never> {
        busy("session-send", tap: true) { [self] in
            guard let o = coding.open, let u = o.unsent else { return }
            await sendWords(o.id, u.text)
        }
    }

    private func sendWords(_ id: String, _ text: String) async {
        if text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { return }
        switch await unlocker.run({ await api.sessionSend(id, text: text) }) {
        case .ok(let r):
            if let yes = r.needsYes {
                coding.asking = .send(session: id, text: text, yes: yes)
                show(.sessionYes)
            } else {
                sent(r, text)
                await readTimeline(earlier: false)
            }
        case .failed(let e): await notSent(id, text, e)
        }
    }

    /// PHN-800: a send the Mac refused (or that never reached it) never looks sent: the words leave the box into a dimmed
    /// bubble with why, in the Mac's plain words, and Try again; the question, if one was open, closes.
    private func notSent(_ id: String, _ text: String, _ e: FerbError) async {
        if case .NotSigned = e { return }
        if coding.asking != nil || coding.open?.pick != nil {
            coding.asking = nil
            coding.open?.pick = nil
            show(.session(id: id))
        }
        guard coding.open?.id == id else { return }
        coding.open?.unsent = Unsent(text: text, reason: Self.errorNote(e))
        if coding.draft == text { coding.draft = "" }
        // HAND-683: FERB can't tell which terminal is the session's: the ones it can see, for him to choose
        if case .Refused(let why, let code) = e, code == "choose_terminal" {
            await readTimeline(earlier: false)
            let terminals = coding.open?.session?.terminals ?? []
            if !terminals.isEmpty, coding.open?.id == id {
                coding.open?.pick = TerminalPick(text: text, why: why, terminals: terminals)
                show(.sessionPick)
            }
        }
    }

    /// HAND-683: his choice of terminal for these words, signed as control is (GATE-622): the Mac types only there,
    /// keeps it for the session while he controls it, and asks again if it changes.
    @discardableResult
    public func pickTerminal(_ terminal: String) -> Task<Void, Never> {
        busy("session-pick", tap: true) { [self] in
            guard let o = coding.open, let pick = o.pick else { return }
            guard let signer = await freshSigner(.yes) else { return }
            switch await unlocker.run({ await api.sessionControl(o.id, text: pick.text, terminal: terminal, signer: signer) }) {
            case .ok(let r):
                coding.open?.pick = nil
                show(.session(id: o.id))
                sent(r, pick.text)
                await readTimeline(earlier: false)
            case .failed(let e): await notSent(o.id, pick.text, e)
            }
        }
    }

    /// Don't send: nothing is typed; his words stay as not sent, with Try again.
    public func pickNoTerminal() {
        coding.open?.pick = nil
        if let id = coding.open?.id { show(.session(id: id)) }
    }

    /// What a send did: what was sent leaves the box (unless he typed on meanwhile), and a not-sent bubble of the same
    /// words goes; the Mac couldn't see it arrive yet: said.
    private func sent(_ r: SessionSent, _ text: String) {
        guard r.sent else { return }
        if coding.draft == text { coding.draft = "" }
        if coding.open?.unsent?.text == text { coding.open?.unsent = nil }
        if !r.delivered { note(Note("sessions_not_seen")) }
    }

    /// The session's menu (PHN-800).
    public func sessionMenu(_ open: Bool) { coding.open?.menu = open }

    /// Stop its turn: the agent's own interrupt. A session this phone doesn't control yet asks first (the same Yes as a
    /// send, with nothing to type), then stops it.
    @discardableResult
    public func stopSessionTurn() -> Task<Void, Never> {
        busy("session-stop", tap: true) { [self] in
            guard let o = coding.open else { return }
            switch await unlocker.run({ await api.sessionInterrupt(o.id) }) {
            case .ok: note(Note("sessions_interrupted"))
            case .failed(let e):
                // refused: when it is because this phone doesn't control the session (not_the_session), the Mac's send
                // asks its question (nothing is sent with no words) and his Yes takes control before the stop; else the
                // Mac's words
                guard case .Refused = e, case .ok(let r) = await api.sessionSend(o.id, text: ""), let yes = r.needsYes else { return failed(e) }
                coding.asking = .interrupt(session: o.id, yes: yes)
                show(.sessionYes)
            }
        }
    }

    /// The owner's Yes on the question: control of the session from this phone (signed, GATE-622) with his words, a stop,
    /// or the new session's command (signed, HAND-665).
    @discardableResult
    public func sessionYes() -> Task<Void, Never> {
        busy("session-yes", tap: true) { [self] in
            guard let ask = coding.asking else { return }
            guard let signer = await freshSigner(.yes) else { return }
            switch ask {
            case .send(let id, let text, _):
                switch await unlocker.run({ await api.sessionControl(id, text: text, terminal: "", signer: signer) }) {
                case .ok(let r):
                    coding.asking = nil
                    show(.session(id: id))
                    sent(r, text)
                    await readTimeline(earlier: false)
                // PHN-800: refused after his Yes: the question closes and the words show as not sent, never stuck open
                case .failed(let e): await notSent(id, text, e)
                }
            case .interrupt(let id, _):
                switch await unlocker.run({ await api.sessionControl(id, text: "", terminal: "", signer: signer) }) {
                case .ok:
                    coding.asking = nil
                    show(.session(id: id))
                    switch await api.sessionInterrupt(id) {
                    case .ok: note(Note("sessions_interrupted"))
                    case .failed(let e): failed(e)
                    }
                    await readTimeline(earlier: false)
                case .failed(let e):
                    if case .NotSigned = e { break }
                    // PHN-800: never stuck on the question
                    coding.asking = nil
                    show(.session(id: id))
                    failed(e)
                }
            case .start(let agent, let folder, let text, _):
                switch await unlocker.run({ await api.sessionStart(agent: agent, folder: folder, text: text, signer: signer) }) {
                case .ok:
                    coding.asking = nil
                    coding.new = nil
                    show(nil)
                    note(Note("sessions_started"))
                    await readSessions(prompt: false)
                case .failed(let e): if case .NotSigned = e {} else { failed(e) }
                }
            }
        }
    }

    /// Cancel on the question: nothing is sent; back where it was asked.
    public func sessionNo() {
        guard let ask = coding.asking else { return show(nil) }
        coding.asking = nil
        switch ask {
        case .send(let id, _, _), .interrupt(let id, _): show(.session(id: id))
        case .start: show(.sessionNew)
        }
    }

    /// One of the card's buttons (the choice's id), signed each time (HAND-663).
    @discardableResult
    public func answerSession(card: String, choice: String) -> Task<Void, Never> {
        busy("session-answer:\(card)", tap: true) { [self] in
            guard let id = coding.open?.id else { return }
            guard let signer = await freshSigner(.yes) else { return }
            switch await unlocker.run({ await api.sessionAnswer(id, card: card, choice: choice, signer: signer) }) {
            case .ok:
                if var o = coding.open, o.id == id {
                    o.cards.removeAll { $0.id == card }
                    coding.open = o
                }
                await readTimeline(earlier: false)
            case .failed(let e): if case .NotSigned = e {} else { failed(e) }
            }
        }
    }

    /// Stop controlling: this phone's control of the session ends (plain: it only takes something away).
    @discardableResult
    public func releaseSession() -> Task<Void, Never> {
        busy("session-release", tap: true) { [self] in
            guard let id = coding.open?.id else { return }
            switch await api.sessionRelease(id) {
            case .ok:
                if var o = coding.open, o.id == id { o.session?.controlledHere = false; coding.open = o }
                await readTimeline(earlier: false)
            case .failed(let e): failed(e)
            }
        }
    }

    /// Hand to FERB: the sheet that asks for the goal.
    public func askHandToFerb() {
        guard let id = coding.open?.id else { return }
        show(.sessionHand(id: id))
    }

    /// Hand it over (signed, ORCH-687): the owner's goal becomes FERB's task in the chat.
    @discardableResult
    public func handToFerb(goal: String) -> Task<Void, Never> {
        busy("session-hand", tap: true) { [self] in
            guard let id = coding.open?.id else { return }
            let goal = goal.trimmingCharacters(in: .whitespacesAndNewlines)
            if goal.isEmpty { return }
            guard let signer = await freshSigner(.yes) else { return }
            switch await unlocker.run({ await api.sessionDelegate(id, goal: goal, signer: signer) }) {
            case .ok:
                show(.session(id: id))
                note(Note("sessions_handed"))
                await readTimeline(earlier: false)
            case .failed(let e): if case .NotSigned = e {} else { failed(e) }
            }
        }
    }

    /// Back from a session's hand-over or its window to the session itself, or out of the session.
    public func backToSession() {
        if let id = openSessionId { show(.session(id: id)) } else { show(nil) }
    }

    public func closeSession() { show(nil) }

    // ---- a new session ------------------------------------------------------------------------------------------------

    /// New session: its sheet, and the folders the Mac offers (the first chosen); none could be read: the sheet goes.
    @discardableResult
    public func newSession() -> Task<Void, Never> {
        coding.new = NewSessionDraft()
        show(.sessionNew)
        return busy("session-new") { [self] in
            switch await unlocker.run({ await api.sessionPrepare(agent: "", folder: "", text: "") }) {
            case .ok(let p):
                guard var n = coding.new else { return }
                n.loading = false
                n.folders = p.folders
                if n.folder == nil { n.folder = p.folders.first }
                coding.new = n
            case .failed(let e):
                if case .NotSigned = e {} else { failed(e) }
                closeNewSession()
            }
        }
    }

    public func newSessionAgent(_ agent: String) { coding.new?.agent = agent }
    public func newSessionFolder(_ folder: String) { coding.new?.folder = folder }
    public func newSessionText(_ text: String) { coding.new?.text = text }

    public func closeNewSession() {
        if sheet == .sessionNew { show(nil) }
        coding.new = nil
    }

    /// Start: the Mac's exact command and its question first (nothing runs yet); the owner's Yes starts it.
    @discardableResult
    public func startSession() -> Task<Void, Never> {
        busy("session-prepare", tap: true) { [self] in
            guard let n = coding.new, let folder = n.folder else { return }
            switch await unlocker.run({ await api.sessionPrepare(agent: n.agent, folder: folder, text: n.text) }) {
            case .ok(let p):
                guard let yes = p.needsYes else {
                    // the Mac asks nothing more: the owner's Start was the Yes (still signed)
                    coding.asking = .start(agent: n.agent, folder: folder, text: n.text, yes: SessionYes(question: "", detail: "", covers: ""))
                    await sessionYes().value
                    return
                }
                coding.asking = .start(agent: n.agent, folder: folder, text: n.text, yes: yes)
                show(.sessionYes)
            case .failed(let e): if case .NotSigned = e {} else { failed(e) }
            }
        }
    }

    // ---- its window -------------------------------------------------------------------------------------------------

    /// Watch: the session's window, live; never asked for on the public relay (its rule is said instead, as Watch
    /// live's).
    public func watchCodingSession() {
        guard let id = coding.open?.id else { return }
        let l = api.link
        if l?.path == .publicRelay || (l?.phase == .online && l?.bulkAllowed == false) {
            note(Note("not_on_shared_relay"))
            return
        }
        show(.sessionWatch(id: id))
        live.startSession(id)
    }

    /// The Mac's window closed under the picture: unlock (one prompt) and watch again.
    @discardableResult
    public func watchSessionAgain() -> Task<Void, Never> {
        busy("live") { [self] in
            guard case .sessionWatch(let id) = sheet, let p = prompts else { return }
            if live.view?.needsUnlock == true {
                guard let s = p.signer(.lock, why: .open) else { return }
                if case .failed(let e) = await api.unlock(s) {
                    if case .NotSigned = e {} else { failed(e) }
                    return
                }
            }
            live.startSession(id)
        }
    }
}

/// How a session and its conversation read on the phone (the same words as FERB.app and Android's Sessions).
public enum SessionWords {
    /// The agent's own title, else the folder's name.
    public static func title(_ s: AgentSession) -> String {
        !s.title.isEmpty ? s.title : (!s.folderName.isEmpty ? s.folderName : s.folder)
    }

    /// "<folder> · in <app>", or "in the background" when no app shows it.
    public static func subtitle(_ s: AgentSession) -> String {
        let place = s.host.isEmpty ? L.s("sessions_in_background") : L.s("sessions_in_app", s.host)
        let folder = s.folderName.isEmpty ? s.folder : s.folderName
        return folder.isEmpty ? place : "\(folder) · \(place)"
    }

    public static func state(_ s: AgentSession) -> SessionState {
        switch s.status {
        case "working": .working
        case "waiting_for_you": .waiting
        default: .idle
        }
    }

    public static func stateText(_ s: AgentSession) -> String {
        switch state(s) {
        case .working: L.s("sessions_working")
        case .waiting: L.s("sessions_waiting")
        case .idle: L.s("sessions_idle")
        }
    }

    public static func row(_ s: AgentSession) -> SessionRowData {
        SessionRowData(title: title(s), subtitle: subtitle(s), state: state(s), stateText: stateText(s),
                       ferb: s.withFerb ? L.s("sessions_with_ferb") : nil)
    }

    /// One item as a line: the owner's words, the agent's, a tool row, a stopped turn (the app's words when the Mac left
    /// none), an error.
    public static func line(_ i: SessionItem) -> SessionLine {
        let key = String(i.seq)
        switch i.kind {
        case "user": return SessionLine(key, .owner, i.text)
        case "tool": return SessionLine(key, .tool, i.text.isEmpty ? i.tool : i.text)
        case "interrupted": return SessionLine(key, .interrupted, i.text.isEmpty ? L.s("sessions_interrupted") : i.text)
        case "error": return SessionLine(key, .error, i.text)
        default: return SessionLine(key, .agent, i.text)
        }
    }

    /// The agent's question: "<agent> wants to use <tool>" or "<agent> asks", its words, the exact detail, its buttons
    /// (none: answered on the Mac).
    public static func ask(_ c: SessionCard, agent: String) -> SessionAsk {
        let title = c.kind == "permission" && !c.tool.isEmpty ? L.s("sessions_wants_tool", agent, c.tool) : L.s("sessions_asks", agent)
        return SessionAsk(title: title, question: c.question, detail: c.detail, choices: c.choices.map(\.label),
                          onMac: L.s("sessions_answer_on_mac"), key: c.id)
    }
}
