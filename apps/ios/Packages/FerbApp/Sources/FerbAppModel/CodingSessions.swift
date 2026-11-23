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
    /// His words on their way into the session, just sent, or refused (PHN-812, PHN-800): one bubble after the last line.
    public var outgoing: SessionOutgoing?
    /// A message the Mac refused (PHN-800): shown as not sent, with why and Try again; never as sent.
    public var unsent: Unsent? {
        if case .notSent(let reason) = outgoing?.state, let o = outgoing { return Unsent(text: o.text, reason: reason) }
        return nil
    }
    /// The session's menu is open (PHN-800).
    public var menu = false
    /// HAND-683: FERB can't tell which terminal is the session's: he chooses one for these words, or doesn't send.
    public var pick: TerminalPick?
    /// Take it back is on its way (owner 2026-11-23): its menu row shows a small arc and "Taking it back…".
    public var takingBack = false

    public init(id: String, session: AgentSession?) {
        self.id = id
        self.session = session
    }

    /// HAND-710: FERB reads none of this session's words (a terminal, an app's window): its screen is its window, live,
    /// and what he sends is typed into it.
    public var watch: Bool { session?.watch == true }

    /// The newest item's number: the poll asks for what came after it.
    public var last: UInt64 { items.last?.seq ?? 0 }
    /// The oldest item's number: Show earlier asks for what came before it.
    public var first: UInt64 { items.first?.seq ?? 0 }
}

/// His words to the session from the moment he taps Send (owner 2026-11-23, kb PHN-812, PHN-814): sending (a dimmed
/// bubble with a small spinner and "Sending…"), sent (his bubble, until the conversation carries it), or not sent (the
/// Mac refused or never got it, PHN-800: why, and Try again). One state for all three, so the box is never frozen and the
/// screen never looks stuck.
public struct SessionOutgoing: Equatable {
    public enum State: Equatable {
        case sending
        case sent
        case notSent(Note)
    }
    public let text: String
    public var state: State
    /// The newest item's number when it was sent: an owner's item after it is this message arriving.
    public var after: UInt64
    public init(text: String, state: State, after: UInt64) { self.text = text; self.state = state; self.after = after }
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

/// What waits for the owner's Yes before FERB types for him (rule 6, GATE-622, HAND-695): his words into a session, a stop
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

/// A new session as the owner chooses it (HAND-691, PHN-821): a terminal, so only where (the Mac's list, the first by
/// default, or a folder chosen in the Mac's Files browser, which joins the list at its top) and an optional command
/// (`text`). No agent to pick: he types the one he wants.
public struct NewSessionDraft: Equatable {
    public var folder: String?
    public var text = ""
    /// The folder it opens in.
    public var chosen: String? { folder }
    /// The folders the Mac offers (its running sessions' first, then recent ones), after any he chose in the browser;
    /// empty while `loading`.
    public var folders: [String] = []
    public var loading = true
    /// Start is on its way (owner 2026-11-23): "Opening a terminal…", the form takes no tap.
    public var starting = false
    /// Why the Mac refused the start, under Start (the form as it was).
    public var refused: Note?
    /// "Another folder…": the Mac's Files browser, folders only, open over the form.
    public var picker: FolderPicker?
    public init() {}
}

/// The new session's folder picker (PHN-821): the Files tab's browser over `phone.files` and `phone.recent`, folders only.
/// At the top (`folder` nil) the places and the recent folders; in a folder, its folders. Its own state: the Files tab's
/// is left as it was.
public struct FolderPicker: Equatable {
    public var folder: Folder?
    /// The path being opened (its row says "Opening…").
    public var loading: String?
    public var problem: Note?
    /// The recent folders: the Mac's recent entries that are folders, and the folders the recent files are in.
    public var recent: [String] = []
    public init() {}

    /// The folders of the folder shown (files hidden).
    public var folders: [FileEntry] { folder?.entries.filter(\.dir) ?? [] }

    /// The recent folders from the Mac's recent files, newest first, each once.
    public static func recentFolders(_ r: RecentFiles, limit: Int = 8) -> [String] {
        var out: [String] = []
        for f in r.files {
            let path = f.dir ? f.path : (f.path as NSString).deletingLastPathComponent
            if path.isEmpty || path == "/" || out.contains(path) { continue }
            out.append(path)
            if out.count == limit { break }
        }
        return out
    }
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
    /// Hand it over is on its way (owner 2026-11-23): "Handing it to FERB…", the sheet waits for the Mac.
    public internal(set) var handing = false
    /// Why the Mac refused the hand-over, under the sheet's button (his words kept in the sheet).
    public internal(set) var handRefused: Note?
    /// A new terminal started that the list doesn't show yet: the ids the list had then (nil: none waited for).
    public internal(set) var awaited: Set<String>?
    /// "Your new terminal will show here in a moment." while a started terminal isn't in the list yet.
    public var soon: Bool { awaited != nil }

    public init() {}

    /// The poll's pace while the tab shows (seconds) and while a session is open.
    public static let listEvery: Double = 3
    public static let openEvery: Double = 2
    /// The newest items a session's screen asks for at once, and per Show earlier.
    public static let page: UInt32 = 50
    /// At most this many items per poll after the newest one held (the Mac's own limit is 100).
    public static let afterPage: UInt32 = 100
    /// HAND-691: what a new session is (the only kind FERB opens).
    public static let terminal = "terminal"

    /// The app locked or forgot the Mac: nothing of the sessions stays.
    func locked() {
        list = nil
        problem = nil
        open = nil
        asking = nil
        new = nil
        draft = ""
        handing = false
        handRefused = nil
        awaited = nil
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
            // the terminal started without an id shows: the quiet line goes
            if let known = coding.awaited, l.sessions.contains(where: { !known.contains($0.id) }) { coding.awaited = nil }
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

    /// Turn on (HAND-696): FERB's own setting, signed like any change of FERB's settings (PHN-112, PHN-133); the Mac adds
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
        // a session FERB reads no words of shows its window live from here (show starts it, HAND-710)
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
                // the row wasn't known at the tap: a session FERB reads no words of shows its window now (HAND-710)
                watchWindowIfShown()
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
                Self.arrived(&now)
                coding.open = now
                if !t.more || t.items.isEmpty { return }
            case .failed(let e):
                readFailed(e, prompt: false)
                return
            }
        }
    }

    /// PHN-812: his sent words leave their own bubble once the conversation carries a message of his after it was sent
    /// (never shown twice; his words as the agent's transcript has them may differ in spaces, so any owner's item counts).
    static func arrived(_ o: inout OpenSession) {
        guard let out = o.outgoing, out.state == .sent else { return }
        if o.items.contains(where: { $0.kind == "user" && $0.seq > out.after }) { o.outgoing = nil }
    }

    /// A read of the open session that failed: a poll says nothing (a closed window is the card's), the owner's own tap does.
    private func readFailed(_ e: FerbError, prompt: Bool) {
        if case .NeedsUnlock = e, !prompt { macLocked(); return }
        if case .NotSigned = e { return }
        if prompt { failed(e) }
    }

    public func sessionTyped(_ text: String) { coding.draft = text }

    /// Send: the box clears and his words show as sending at once, before anything is asked of the Mac (owner 2026-11-23,
    /// kb PHN-812); the Mac sends it when this phone controls the session, else asks first with the exact text (nothing
    /// was sent); his Yes is [sessionYes].
    @discardableResult
    public func sendToSession() -> Task<Void, Never> {
        let text = coding.draft
        if let id = coding.open?.id, !ui.busy.contains("session-send"), !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            sending(id, text)
            coding.draft = ""
        }
        return busy("session-send", tap: true) { [self] in
            guard let id = coding.open?.id, coding.open?.outgoing?.state == .sending, let out = coding.open?.outgoing else { return }
            await sendWords(id, out.text)
        }
    }

    /// Try again on a message the Mac refused (PHN-800): the same words, the same way (asked first if not controlled),
    /// shown as sending again at once (PHN-812).
    @discardableResult
    public func retrySessionSend() -> Task<Void, Never> {
        if let o = coding.open, let u = o.unsent, !ui.busy.contains("session-send") { sending(o.id, u.text) }
        return busy("session-send", tap: true) { [self] in
            guard let o = coding.open, let out = o.outgoing, out.state == .sending else { return }
            await sendWords(o.id, out.text)
        }
    }

    /// His words as sending in the open session (PHN-812).
    private func sending(_ id: String, _ text: String) {
        guard var o = coding.open, o.id == id else { return }
        o.outgoing = SessionOutgoing(text: text, state: .sending, after: o.last)
        coding.open = o
    }

    private func sendWords(_ id: String, _ text: String) async {
        if text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { return }
        switch await unlocker.run({ await api.sessionSend(id, text: text) }) {
        case .ok(let r):
            if let yes = r.needsYes {
                // still sending under the Mac's question: his Yes sends it, Cancel puts it back in the box
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
        if var o = coding.open {
            o.outgoing = SessionOutgoing(text: text, state: .notSent(Self.errorNote(e)), after: o.outgoing?.after ?? o.last)
            coding.open = o
        }
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
            // his choice made: his words show as sending while FERB types them there (PHN-812)
            sending(o.id, pick.text)
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

    /// What a send did: his words show as sent (his bubble, until the conversation carries them, PHN-812); the Mac
    /// couldn't see it arrive yet: said. A send that went nowhere without a reason puts his words back in the box.
    private func sent(_ r: SessionSent, _ text: String) {
        if coding.draft == text { coding.draft = "" }
        guard r.sent else {
            if coding.open?.outgoing?.text == text { coding.open?.outgoing = nil }
            if coding.draft.isEmpty { coding.draft = text }
            return
        }
        if var o = coding.open, o.outgoing?.text == text {
            o.outgoing?.state = .sent
            Self.arrived(&o)
            // HAND-710: no record of its words will ever carry them: the Mac's answer ends the sending state
            if o.watch { o.outgoing = nil }
            coding.open = o
        }
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
    /// or the new session's command (signed, HAND-695).
    @discardableResult
    public func sessionYes() -> Task<Void, Never> {
        busy("session-yes", tap: true) { [self] in
            guard let ask = coding.asking else { return }
            guard let signer = await freshSigner(.yes) else { return }
            if case .send(let id, let text, _) = ask, coding.open?.outgoing?.text != text { sending(id, text) }
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
                // back on the form, Start on its way (owner 2026-11-23)
                coding.asking = nil
                coding.new?.starting = true
                coding.new?.refused = nil
                if sheet != .sessionNew { show(.sessionNew) }
                let known = Set(coding.list?.sessions.map(\.id) ?? [])
                switch await unlocker.run({ await api.sessionStart(agent: agent, folder: folder, text: text, signer: signer) }) {
                case .ok(let started):
                    coding.new = nil
                    if !started.session.isEmpty {
                        // the new terminal itself, at once
                        await readSessions(prompt: false)
                        openSession(started.session)
                    } else {
                        // not shown yet: the list says it is coming until it does
                        show(nil)
                        coding.awaited = known
                        await readSessions(prompt: false)
                    }
                case .failed(let e):
                    coding.new?.starting = false
                    if case .NotSigned = e { return }
                    coding.new?.refused = Self.errorNote(e)
                }
            }
        }
    }

    /// Cancel on the question: nothing is sent; back where it was asked, his words back in the box (PHN-812).
    public func sessionNo() {
        guard let ask = coding.asking else { return show(nil) }
        coding.asking = nil
        switch ask {
        case .send(let id, let text, _):
            if coding.open?.outgoing?.text == text, coding.open?.outgoing?.state == .sending {
                coding.open?.outgoing = nil
                if coding.draft.isEmpty { coding.draft = text }
            }
            show(.session(id: id))
        case .interrupt(let id, _): show(.session(id: id))
        case .start:
            coding.new?.starting = false
            show(.sessionNew)
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
        // HAND-710: only Claude Code and Codex sessions are handed to FERB; one FERB reads no words of isn't
        guard let id = coding.open?.id, coding.open?.watch == false else { return }
        coding.handing = false
        coding.handRefused = nil
        show(.sessionHand(id: id))
    }

    /// Hand it over (signed, ORCH-687): the owner's goal starts FERB's session driver on the Mac (HAND-707). At the tap the button says
    /// "Handing it to FERB…" and waits for the Mac (owner 2026-11-23); its Yes closes the sheet and reads the session at
    /// once; a refusal keeps the sheet with his words and says the Mac's reason under the button.
    @discardableResult
    public func handToFerb(goal: String) -> Task<Void, Never> {
        let goal = goal.trimmingCharacters(in: .whitespacesAndNewlines)
        if !goal.isEmpty, coding.open != nil, !coding.handing {
            coding.handing = true
            coding.handRefused = nil
        }
        return busy("session-hand", tap: true) { [self] in
            guard let id = coding.open?.id, !goal.isEmpty else { coding.handing = false; return }
            defer { coding.handing = false }
            guard let signer = await freshSigner(.yes) else { return }
            switch await unlocker.run({ await api.sessionDelegate(id, goal: goal, signer: signer) }) {
            case .ok:
                show(.session(id: id))
                note(Note("sessions_handed"))
                await readSessions(prompt: false)
                await readTimeline(earlier: false)
            case .failed(let e):
                if case .NotSigned = e { return }
                coding.handRefused = Self.errorNote(e)
            }
        }
    }

    /// Take it back (owner 2026-11-23): FERB's task on this session stops (the task stop every client has; no new
    /// call; HAND-702). At the tap its menu row says "Taking it back…" and takes no tap; done, the menu closes and the session is
    /// read again at once (the badge goes when the Mac says so); refused, "Couldn't take it back: <why>".
    @discardableResult
    public func takeSessionBack() -> Task<Void, Never> {
        let task = coding.open?.session?.ferbTask ?? ""
        if !task.isEmpty, coding.open?.takingBack == false { coding.open?.takingBack = true }
        return busy("session-take-back", tap: true) { [self] in
            guard let id = coding.open?.id, !task.isEmpty else { return }
            let r = await api.stop(taskIds: [task])
            if coding.open?.id == id { coding.open?.takingBack = false }
            switch r {
            case .ok:
                if coding.open?.id == id { coding.open?.menu = false }
                await readSessions(prompt: false)
                await readTimeline(earlier: false)
            case .failed(let e):
                note(Note("sessions_take_back_failed", Self.errorNote(e).text))
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

    public func newSessionFolder(_ folder: String) {
        if coding.new?.starting == true { return }
        coding.new?.folder = folder
        coding.new?.refused = nil
    }
    public func newSessionText(_ text: String) { coding.new?.text = text }

    public func closeNewSession() {
        if coding.new?.starting == true { return }
        if sheet == .sessionNew { show(nil) }
        coding.new = nil
    }

    /// Start: at the tap "Opening a terminal…" and the form takes no tap (owner 2026-11-23, PHN-822); with a command, the Mac's
    /// question showing exactly it first (nothing runs yet); with none, it opens. Refused: the Mac's reason under Start.
    @discardableResult
    public func startSession() -> Task<Void, Never> {
        if coding.new?.chosen != nil, coding.new?.starting == false {
            coding.new?.starting = true
            coding.new?.refused = nil
        }
        return busy("session-prepare", tap: true) { [self] in
            guard let n = coding.new, let folder = n.chosen else { return }
            let command = n.text.trimmingCharacters(in: .whitespaces)
            let kind = CodingSessions.terminal
            switch await unlocker.run({ await api.sessionPrepare(agent: kind, folder: folder, text: command) }) {
            case .ok(let p):
                guard let yes = p.needsYes else {
                    // the Mac asks nothing more: the owner's Start was the Yes (still signed)
                    coding.asking = .start(agent: kind, folder: folder, text: command, yes: SessionYes(question: "", detail: "", covers: ""))
                    await sessionYes().value
                    // a cancelled prompt: the form again
                    if coding.asking != nil { coding.asking = nil; coding.new?.starting = false }
                    return
                }
                coding.new?.starting = false
                coding.asking = .start(agent: kind, folder: folder, text: command, yes: yes)
                show(.sessionYes)
            case .failed(let e):
                coding.new?.starting = false
                if case .NotSigned = e { return }
                coding.new?.refused = Self.errorNote(e)
            }
        }
    }

    // ---- the new session's folder picker (PHN-821) -----------------------------------------------------------------

    /// "Another folder…": the Mac's Files browser over the form, folders only, at its top (the places and the recent
    /// folders).
    @discardableResult
    public func pickNewSessionFolder() -> Task<Void, Never>? {
        guard coding.new != nil, coding.new?.starting == false else { return nil }
        coding.new?.picker = FolderPicker()
        return busy("session-folder-recent") { [self] in
            switch await unlocker.run({ await api.recentFiles(limit: Self.recentFilesCount) }) {
            case .ok(let r): coding.new?.picker?.recent = FolderPicker.recentFolders(r)
            case .failed: break  // the places still show; recent folders are a help, not the browser
            }
        }
    }

    /// A folder opens in the picker; a key place asks for Face ID / Touch ID for that very path first (PHN-121).
    @discardableResult
    public func pickerOpen(_ path: String, keyPlace: Bool = false) -> Task<Void, Never>? {
        guard coding.new?.picker != nil else { return nil }
        coding.new?.picker?.loading = path
        coding.new?.picker?.problem = nil
        return busy("session-folder") { [self] in
            let r = await unlocker.withProof(keyPlace, .file) { signer in await api.files(path: path, signer: signer) }
            guard coding.new?.picker != nil else { return }
            coding.new?.picker?.loading = nil
            switch r {
            case .ok(let f): coding.new?.picker?.folder = f
            case .failed(let e):
                coding.new?.picker?.problem = { if case .NotSigned = e { return Note("files_not_opened") }; return Self.errorNote(e) }()
            }
        }
    }

    /// Back: up one folder; from a place, the picker's top.
    public func pickerUp() {
        guard let f = coding.new?.picker?.folder else { return }
        if let parent = f.parent, !Self.places.contains(f.path), f.path != "~", !parent.isEmpty {
            pickerOpen(parent)
        } else {
            coding.new?.picker?.folder = nil
            coding.new?.picker?.problem = nil
        }
    }

    /// Use this folder: the folder shown becomes the chosen one, at the top of the list; back to the form.
    public func pickerUse() {
        guard var n = coding.new, let f = n.picker?.folder else { return }
        n.folders.removeAll { $0 == f.path }
        n.folders.insert(f.path, at: 0)
        n.folder = f.path
        n.refused = nil
        n.picker = nil
        coding.new = n
    }

    /// Close: back to the form as it was.
    public func pickerClose() { coding.new?.picker = nil }

    // ---- its window -------------------------------------------------------------------------------------------------

    /// The link can't carry a live picture (the public relay, or one that allows no bulk): Watch live's rule.
    private var noLivePictures: Bool {
        let l = api.link
        return l?.path == .publicRelay || (l?.phase == .online && l?.bulkAllowed == false)
    }

    /// Watch: the session's window, live; never asked for on the public relay (its rule is said instead, as Watch
    /// live's).
    public func watchCodingSession() {
        guard let id = coding.open?.id else { return }
        if noLivePictures {
            note(Note("not_on_shared_relay"))
            return
        }
        show(.sessionWatch(id: id))
        live.startSession(id)
    }

    /// The session's window on screen: its own Watch page, or the screen of a session FERB reads no words of (HAND-710).
    var watchedSession: String? {
        switch sheet {
        case .sessionWatch(let id): id
        case .session(let id): coding.open?.id == id && coding.open?.watch == true ? id : nil
        default: nil
        }
    }

    /// HAND-710: a session FERB reads no words of shows its window live while its screen shows, the app is in front and
    /// unlocked: the same preview as Watch (memory only, the owner's screenshot switch), asked for once per showing. On
    /// the public relay it is never asked for: the picture's place says Watch live's rule instead.
    func watchWindowIfShown() {
        guard case .session(let id) = sheet, watchedSession == id, session.front, !session.locked else { return }
        if live.view?.taskId == id { return }
        if noLivePictures { live.refused(id, Note("not_on_shared_relay")) } else { live.startSession(id) }
    }

    /// The Mac's window closed under the picture: unlock (one prompt) and watch again (its Watch page, or the screen of a
    /// session FERB reads no words of).
    @discardableResult
    public func watchSessionAgain() -> Task<Void, Never> {
        busy("live") { [self] in
            guard let id = watchedSession, let p = prompts else { return }
            if live.view?.needsUnlock == true {
                guard let s = p.signer(.lock, why: .open) else { return }
                if case .failed(let e) = await api.unlock(s) {
                    if case .NotSigned = e {} else { failed(e) }
                    return
                }
            }
            if noLivePictures { live.refused(id, Note("not_on_shared_relay")) } else { live.startSession(id) }
        }
    }
}

/// How a session and its conversation read on the phone (the same words as FERB.app and Android's Sessions).
public enum SessionWords {
    /// The agent's own title, else its window's in its app's words (HAND-692), else the folder's name, else the agent's
    /// name ("Claude Code", "Codex", the app's, the terminal program, "Terminal"): never empty (owner 2026-11-23).
    public static func title(_ s: AgentSession) -> String {
        for t in [s.title, s.window, s.folderName, s.folder, s.agentName, s.agent] where !t.trimmingCharacters(in: .whitespaces).isEmpty {
            return t
        }
        return ""
    }

    /// A terminal's output is fixed text, never Markdown; every agent's chat is (HAND-703).
    public static func markdown(_ s: AgentSession?) -> Bool { s?.agent != CodingSessions.terminal }

    /// HAND-710: an app's own window (Cursor, the Claude app, ChatGPT…), one row per open window.
    public static let appChat = "app_chat"

    /// "<program> · <folder> · in <app>" (HAND-692; no program or folder: without them), or "in the background" when
    /// no app shows it. An app's window: the app's name alone (HAND-710).
    public static func subtitle(_ s: AgentSession) -> String {
        if s.agent == appChat { return s.host.isEmpty ? s.agentName : s.host }
        let place = s.host.isEmpty ? L.s("sessions_in_background") : L.s("sessions_in_app", s.host)
        let folder = s.folderName.isEmpty ? s.folder : s.folderName
        return [s.program, folder, place].filter { !$0.isEmpty }.joined(separator: " · ")
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
    /// none), an error. The agent's reply with parts is one block of them in order (HAND-703); without parts, a chat
    /// agent's words are Markdown (plain words stay the plain line), a terminal's output is drawn as it is.
    public static func line(_ i: SessionItem, markdown: Bool = true) -> SessionLine {
        let key = String(i.seq)
        switch i.kind {
        // HAND-709: a message FERB sent while it kept the session going is marked as FERB's
        case "user": return SessionLine(key, .owner, i.text, by: i.ferb ? L.s("sessions_sent_by_ferb") : nil)
        case "tool": return SessionLine(key, .tool, i.text.isEmpty ? i.tool : i.text)
        case "interrupted": return SessionLine(key, .interrupted, i.text.isEmpty ? L.s("sessions_interrupted") : i.text)
        case "error": return SessionLine(key, .error, i.text)
        default:
            if !i.parts.isEmpty { return SessionLine(key, .reply, i.text, parts: i.parts.compactMap(part)) }
            if markdown, Present.plainOnly(i.text) == nil { return SessionLine(key, .reply, i.text, parts: [.text(Present.rich(i.text))]) }
            return SessionLine(key, .agent, i.text)
        }
    }

    /// One part as drawn (HAND-703): text is Markdown; a kind this app doesn't know is drawn as text.
    static func part(_ p: SessionPart) -> SessionPartShown? {
        switch p.kind {
        case "thinking": return .thinking(label: p.label.isEmpty ? p.text : p.label, text: p.label.isEmpty ? "" : p.text)
        case "tool": return .tool(label: p.label, detail: p.detail, text: p.text)
        case "meta":
            let words = p.text.isEmpty ? p.label : p.text
            return words.isEmpty ? nil : .meta(words)
        default: return p.text.isEmpty ? nil : .text(Present.rich(p.text))
        }
    }

    /// The Markdown an item's lines will read, laid out here (off the main actor) so the screen finds it kept.
    public static func prepare(_ items: [SessionItem]) {
        for i in items where i.kind != "user" && i.kind != "tool" {
            if i.parts.isEmpty { _ = Present.rich(i.text) }
            for p in i.parts where p.kind != "thinking" && p.kind != "tool" && p.kind != "meta" { _ = Present.rich(p.text) }
        }
    }

    /// The conversation's lines with a day's start between days, as the main chat's earlier days have them (kb PHN-813,
    /// PHN-627): none while every item is today's (today's chat has none); else one before each day's first item,
    /// "Today" included. `label` says a day (days since 1970, the chat's own count); an item whose time can't be read
    /// stays in the day before it.
    public static func lines(_ items: [SessionItem], today: Int, calendar: Calendar = .current, markdown: Bool = true,
                             label: (Int) -> String) -> [SessionLine] {
        let days = items.map { day($0.at, calendar) }
        guard days.contains(where: { $0 != nil && $0 != today }) else { return items.map { line($0, markdown: markdown) } }
        var out: [SessionLine] = []
        var current: Int?
        for (i, item) in items.enumerated() {
            if let d = days[i], d != current {
                out.append(SessionLine("day:\(d)", .day, label(d)))
                current = d
            }
            out.append(line(item, markdown: markdown))
        }
        return out
    }

    /// An item's day (the chat's count: days since 1970 of its local date), from its ISO 8601 time; nil when unreadable.
    static func day(_ at: String, _ calendar: Calendar) -> Int? {
        let plain = ISO8601DateFormatter()
        let fraction = ISO8601DateFormatter()
        fraction.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        guard let date = plain.date(from: at) ?? fraction.date(from: at) else { return nil }
        return ChatModel.day(date, calendar)
    }

    /// The agent's question: "<agent> wants to use <tool>" or "<agent> asks", its words, the exact detail, its buttons
    /// (none: answered on the Mac).
    public static func ask(_ c: SessionCard, agent: String) -> SessionAsk {
        // HAND-709: FERB's own question about its drive of the session (keep going?)
        let title = c.kind == "ferb" ? L.s("sessions_ferb_asks")
            : c.kind == "permission" && !c.tool.isEmpty ? L.s("sessions_wants_tool", agent, c.tool) : L.s("sessions_asks", agent)
        return SessionAsk(title: title, question: c.question, detail: c.detail, choices: c.choices.map(\.label),
                          onMac: L.s("sessions_answer_on_mac"), key: c.id)
    }
}
