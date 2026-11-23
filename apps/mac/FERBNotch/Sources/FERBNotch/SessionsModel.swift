import AppKit
import SwiftUI
import NotchCore

/// The FERB window's Sessions page data: every terminal on the Mac (HAND-691), Claude Code and Codex among them, and the
/// agent apps' windows (HAND-710), through the daemon's `sessions.*`
/// calls. Each button is one call; whatever the daemon asks before sending or starting (`needs_yes`) is shown to the
/// owner first, exactly, and only the owner's Yes makes the call that acts. A refusal is shown as the daemon's words.
@MainActor
final class SessionsModel: ObservableObject {
    /// What waits for the owner's Yes: the daemon's question and the call it leads to.
    enum Action: Equatable {
        case send(session: String, text: String)
        case interrupt(session: String)
        case start(folder: String, command: String)
    }

    /// HAND-683: the words FERB couldn't type because it can't tell which terminal is the session's, the daemon's
    /// reason, and the terminals it can see, for the owner to choose (or not send).
    struct Pick: Identifiable, Equatable {
        let id = UUID()
        let session: String
        let text: String
        let why: String
        let terminals: [SessionChoice]
    }

    /// PHN-812 (owner 2026-11-23): his words from the moment he presses Send. Typing into a terminal takes seconds, so
    /// they show at once as a bubble that is sending; the daemon's yes makes it a sent bubble until the session's own
    /// record of them arrives; a refusal leaves them as not sent, with the daemon's reason and Try again, never lost.
    struct Outgoing: Equatable {
        enum State: Equatable {
            case sending
            case sent
            case notSent(String)
        }
        let session: String
        let text: String
        var state: State
        /// The newest item held when he sent: the session's own copy of his words comes after it.
        let after: Int
    }

    struct Confirm: Identifiable, Equatable {
        let id = UUID()
        let yes: SessionYes
        let action: Action
    }

    /// How many timeline items one read asks for (the daemon's default and at most 100).
    static let page = 50

    let rpc: DaemonRPCTransport
    /// The chat's own client id (EntryWatch.clientID): FERB's work on a handed-over session shows in that chat.
    var clientID: () -> String? = { nil }

    /// Whether the daemon has sessions on (`sessions.enabled`); nil until it answered.
    @Published private(set) var enabled: Bool?
    @Published private(set) var sessions: [AgentSessionRow] = []
    /// The session open now, its conversation (oldest first), whether earlier items exist, and its open cards.
    @Published private(set) var openID: String?
    @Published private(set) var openSession: AgentSessionRow?
    @Published private(set) var items: [SessionItem] = []
    @Published private(set) var more = false
    @Published private(set) var cards: [SessionCard] = []
    /// The composer's text for the open session.
    @Published var draft = ""
    /// His last words to the open session while they are sending, sent but not yet in its record, or not sent (PHN-812).
    @Published private(set) var outgoing: Outgoing?
    /// The session just opened and its first read hasn't answered yet: the page shows its loading look, never an error
    /// (PHN-816).
    @Published private(set) var opening = false
    /// The problem line as the page shows it: none while a session opens; the daemon's words once it has answered.
    var problemShown: String? { opening ? nil : problem }
    /// The page of earlier items is on its way (PHN-813: it comes as he scrolls up, as the chat's earlier messages do).
    @Published private(set) var loadingEarlier = false
    @Published var confirm: Confirm?
    @Published var pick: Pick?
    /// New session (HAND-691): a terminal; the folders to choose from and the app it opens in, from `sessions.prepare {}`.
    @Published private(set) var folders: [String] = []
    @Published private(set) var host = "Terminal"
    /// Why the last call failed (the daemon's own words), and the last action's one line.
    @Published var problem: String?
    @Published var note: String?
    @Published private(set) var busy = false
    /// Hand to FERB is on its way: the sheet's button shows it and waits for the Mac's answer; a refusal stays on the
    /// sheet under the button, with his words kept.
    @Published private(set) var handingOver = false
    @Published var handOverProblem: String?
    /// Take it back: FERB's task being stopped. It stays pending from the tap until the session no longer names that
    /// task (or `takeBackWait` passes), so the button never flips back while the daemon catches up.
    @Published private(set) var takingBack: String?
    private var takenBackAt: Date?
    static let takeBackWait: TimeInterval = 10
    /// New session's form (kept here so the daemon's question about its command can come and go over it), Start on
    /// its way, and why the Mac refused it (shown under Start).
    @Published var startFolder = ""
    @Published var startCommand = ""
    @Published private(set) var starting = false
    @Published var startProblem: String?
    /// The terminal just opened hasn't shown in the list yet: the ids listed when it was started, until a new one shows.
    @Published private(set) var awaitingNew: Set<String>?
    var now: () -> Date = Date.init

    init(rpc: DaemonRPCTransport) {
        self.rpc = rpc
    }

    /// One call; a refusal becomes the problem line and nil. `quiet`: a background poll keeps the last problem.
    private func ask(_ method: String, _ params: [String: Any] = [:], quiet: Bool = false) async -> [String: Any]? {
        do {
            let reply = try await rpc.call(method, params: params)
            if !quiet { problem = nil }
            return reply
        } catch {
            if !quiet || problem == nil { problem = error.localizedDescription }
            return nil
        }
    }

    // MARK: List

    func load(quiet: Bool = false) async {
        guard let reply = await ask("sessions.list", quiet: quiet) else { return }
        enabled = (reply["enabled"] as? NSNumber)?.boolValue ?? false
        sessions = (reply["sessions"] as? [[String: Any]] ?? []).compactMap(AgentSessionRow.init)
        if let openID, let row = sessions.first(where: { $0.id == openID }) { openSession = row }
        if let known = awaitingNew, sessions.contains(where: { !known.contains($0.id) }) { awaitingNew = nil }
        settleTakeBack()
    }

    /// The new terminal's line: shows until it appears in the list.
    var awaitingNewLine: String? { awaitingNew == nil ? nil : "Your new terminal will show here in a moment." }

    // MARK: One session

    func open(_ row: AgentSessionRow) async {
        guard openID != row.id else { return }
        openID = row.id
        openSession = row
        items = []
        cards = []
        more = false
        draft = ""
        outgoing = nil
        note = nil
        problem = nil
        // HAND-710: a watched session has no conversation to read: its screen is its window, live
        guard !row.watch else { return }
        opening = true
        await loadTimeline()
        if openID == row.id { opening = false }
    }

    /// The open session is watched (HAND-710): `sessions.timeline` always answers it nothing, so it is never asked;
    /// the session itself is read again with the list (~3 s and on the daemon's events).
    var watching: Bool { openSession?.watch == true }

    private func isWatched(_ id: String) -> Bool {
        (openSession?.id == id ? openSession : sessions.first { $0.id == id })?.watch == true
    }

    func close() {
        openID = nil
        openSession = nil
        items = []
        cards = []
        more = false
        draft = ""
        outgoing = nil
        opening = false
        note = nil
    }

    /// The newest page of the open session.
    func loadTimeline() async {
        guard let id = openID, !watching else { return }
        guard let reply = await ask("sessions.timeline", ["session": id, "limit": Self.page]), openID == id else { return }
        let timeline = SessionTimeline(reply)
        items = timeline.items
        more = timeline.more
        cards = timeline.cards
        if let session = timeline.session { openSession = session }
        settleOutgoing()
        settleTakeBack()
    }

    /// What is new since the last item held (every ~2 s while the page shows, and on the daemon's events).
    func poll() async {
        guard let id = openID, !opening, !watching else { return }   // the first read is still on its way
        guard let last = items.last?.seq else { return await loadTimeline() }
        guard let reply = await ask("sessions.timeline", ["session": id, "after": last, "limit": Self.page * 2],
                                    quiet: true), openID == id else { return }
        let timeline = SessionTimeline(reply)
        let held = Set(items.map(\.seq))
        // an item already held is replaced: an app chat's turn grows its parts as it is written
        let fresh = Dictionary(timeline.items.map { ($0.seq, $0) }, uniquingKeysWith: { _, b in b })
        items = items.map { fresh[$0.seq] ?? $0 } + timeline.items.filter { !held.contains($0.seq) }
        cards = timeline.cards
        if let session = timeline.session { openSession = session }
        settleOutgoing()
        settleTakeBack()
    }

    /// A sent message goes once the session's record has his words: any message of his after the one held when he sent
    /// (the agent may record them with different spacing), so they never show twice.
    private func settleOutgoing() {
        guard let o = outgoing, o.state == .sent, o.session == openID else { return }
        if items.contains(where: { $0.kind == "user" && $0.seq > o.after }) { outgoing = nil }
    }

    /// Show earlier: the page before the first item held (asked once at a time, as he scrolls near the top).
    func showEarlier() async {
        guard let id = openID, let first = items.first?.seq, !loadingEarlier else { return }
        loadingEarlier = true
        defer { loadingEarlier = false }
        guard let reply = await ask("sessions.timeline", ["session": id, "before": first, "limit": Self.page]),
              openID == id else { return }
        let timeline = SessionTimeline(reply)
        let held = Set(items.map(\.seq))
        items = timeline.items.filter { !held.contains($0.seq) } + items
        more = timeline.more
    }

    // MARK: Sending, answering, stopping

    /// The composer's Send: the box empties at once and his words show as sending (PHN-812); the daemon either sends
    /// (the owner already said Yes here) or asks first, showing the exact text; nothing is sent until that Yes.
    func send() async {
        guard let id = openID else { return }
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, !busy else { return }
        draft = ""
        await deliver(text, to: id)
    }

    /// Try again on words that weren't sent: the same words, the same way.
    func tryAgain() async {
        guard let o = outgoing, case .notSent = o.state, o.session == openID, !busy else { return }
        await deliver(o.text, to: o.session)
    }

    private func deliver(_ text: String, to id: String) async {
        outgoing = Outgoing(session: id, text: text, state: .sending, after: items.last?.seq ?? 0)
        busy = true
        defer { busy = false }
        guard let reply = await sending("sessions.send", ["session": id, "text": text], session: id, text: text) else { return }
        if let yes = SessionYes(reply) {
            confirm = Confirm(yes: yes, action: .send(session: id, text: text))
            return
        }
        sent(reply, session: id, text: text)
        await poll()
    }

    /// The daemon's answer to a send: sent (the bubble stays as sent until the record has it), or not sent. A watched
    /// session has no record to wait for (HAND-710): its words are done once the daemon says they were typed.
    private func sent(_ reply: [String: Any], session: String, text: String) {
        guard reply["sent"] as? Bool == true else {
            notSent(session: session, text: text, why: reply["message"] as? String ?? "FERB couldn't send it.")
            return
        }
        if outgoing?.session == session, outgoing?.text == text {
            if isWatched(session) { outgoing = nil } else { outgoing?.state = .sent }
        }
        note = reply["delivered"] as? Bool == false ? "Sent, but FERB couldn't see it arrive yet." : nil
    }

    private func notSent(session: String, text: String, why: String) {
        guard openID == session else { return }
        outgoing = Outgoing(session: session, text: text, state: .notSent(why), after: outgoing?.after ?? (items.last?.seq ?? 0))
    }

    /// Nothing was sent and he chose not to (Cancel, Don't send): his words go back in the box.
    private func takeBack(_ text: String) {
        if outgoing?.text == text { outgoing = nil }
        if draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { draft = text }
    }

    /// The owner's Yes to the question shown: control of the session from here and the text he read, the interrupt
    /// he asked for, or the new session's command. Whether it went through (a new session: whether it opened).
    @discardableResult
    func confirmYes() async -> Bool {
        guard let pending = confirm else { return false }
        confirm = nil
        busy = true
        defer { busy = false }
        switch pending.action {
        case .send(let id, let text):
            if outgoing?.text == text { outgoing?.state = .sending }
            guard let reply = await sending("sessions.control", ["session": id, "text": text], session: id, text: text) else { return false }
            sent(reply, session: id, text: text)
            await load(quiet: true)
            await poll()
        case .interrupt(let id):
            guard await ask("sessions.control", ["session": id, "text": ""]) != nil else { return false }
            guard await ask("sessions.interrupt", ["session": id]) != nil else { return false }
            note = "You stopped its turn."
            await load(quiet: true)
            await poll()
        case .start(let folder, let command):
            return await open(folder: folder, command: command)
        }
        return true
    }

    func cancelConfirm() {
        if case .send(_, let text)? = confirm?.action { takeBack(text) }
        confirm = nil
    }

    /// A send's call; when the daemon can't tell which terminal is the session's (HAND-683), the terminals it can see
    /// come from the session, for the owner to choose (his words stay sending meanwhile). Any other refusal leaves them
    /// not sent, with the daemon's own words (PHN-812).
    private func sending(_ method: String, _ params: [String: Any], session: String, text: String) async -> [String: Any]? {
        do {
            let reply = try await rpc.call(method, params: params)
            problem = nil
            return reply
        } catch let error as DaemonRPCError where error.code == SessionRefusal.chooseTerminal {
            problem = error.localizedDescription
            await load(quiet: true)
            await poll()
            let terminals = (openSession?.id == session ? openSession : sessions.first { $0.id == session })?.terminals ?? []
            if terminals.isEmpty {
                notSent(session: session, text: text, why: error.localizedDescription)
            } else {
                pick = Pick(session: session, text: text, why: error.localizedDescription, terminals: terminals)
            }
            return nil
        } catch {
            notSent(session: session, text: text, why: error.localizedDescription)
            return nil
        }
    }

    /// HAND-683: the owner's choice of terminal for these words: FERB types only there, keeps it for the session while
    /// he controls it from here, and asks again if it changes.
    func choose(_ terminal: SessionChoice) async {
        guard let p = pick else { return }
        pick = nil
        problem = nil
        if outgoing?.text == p.text { outgoing?.state = .sending }
        busy = true
        defer { busy = false }
        guard let reply = await sending("sessions.control", ["session": p.session, "text": p.text, "terminal": terminal.id],
                                        session: p.session, text: p.text) else { return }
        sent(reply, session: p.session, text: p.text)
        await load(quiet: true)
        await poll()
    }

    /// Don't send: nothing is typed; the words go back in the box.
    func cancelPick() {
        if let p = pick { takeBack(p.text) }
        pick = nil
    }

    /// A card's button: the agent gets that choice.
    func answer(_ card: SessionCard, _ choice: SessionChoice) async {
        guard await ask("sessions.answer", ["session": card.session.isEmpty ? (openID ?? "") : card.session,
                                            "card": card.id, "choice": choice.id]) != nil else {
            await poll()
            return
        }
        cards.removeAll { $0.id == card.id }
        await load(quiet: true)
        await poll()
    }

    /// Stop its turn. Without control from here the daemon refuses; then the owner's Yes first (its question comes
    /// from a send of no text, which never sends anything), and the interrupt after it.
    func interrupt() async {
        guard let id = openID else { return }
        do {
            _ = try await rpc.call("sessions.interrupt", params: ["session": id])
            problem = nil
            note = "You stopped its turn."
            await poll()
        } catch let error as DaemonRPCError where error.code == SessionRefusal.notTheSession {
            guard let reply = await ask("sessions.send", ["session": id, "text": ""]) else { return }
            if let yes = SessionYes(reply) { confirm = Confirm(yes: yes, action: .interrupt(session: id)) }
        } catch {
            problem = error.localizedDescription
        }
    }

    /// Stop controlling: sends from here ask again.
    func release() async {
        guard let id = openID else { return }
        guard await ask("sessions.release", ["session": id]) != nil else { return }
        await load(quiet: true)
    }

    /// Hand to FERB: FERB works the session toward the owner's goal, in his chat. Pending from the tap until the Mac
    /// answers; a yes reads the session again at once (its `ferb_note` says what became of it), a refusal is kept for
    /// the sheet to show under its button.
    @discardableResult
    func delegate(goal: String) async -> Bool {
        guard let id = openID, !handingOver else { return false }
        let goal = goal.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !goal.isEmpty else { return false }
        var params: [String: Any] = ["session": id, "goal": goal]
        if let client = clientID(), !client.isEmpty { params["client_id"] = client }
        handingOver = true
        handOverProblem = nil
        defer { handingOver = false }
        do {
            _ = try await rpc.call("sessions.delegate", params: params)
        } catch {
            handOverProblem = error.localizedDescription
            return false
        }
        await load(quiet: true)
        await poll()
        return true
    }

    /// Take it back: FERB's task on this session stops (the same `task.stop` as the task list's Stop, for exactly that
    /// task), and the session is read again at once. A refusal says why on the page.
    func takeItBack() async {
        guard let row = openSession, !row.ferbTask.isEmpty, takingBack == nil else { return }
        let task = row.ferbTask
        takingBack = task
        takenBackAt = nil
        problem = nil
        do {
            _ = try await rpc.call("task.stop", params: ["task_ids": [task]])
        } catch {
            takingBack = nil
            problem = "Couldn't take it back: \(error.localizedDescription)"
            return
        }
        takenBackAt = now()
        await load(quiet: true)
        await poll()
    }

    /// Whether this session's Take it back is on its way.
    func isTakingBack(_ row: AgentSessionRow) -> Bool { takingBack != nil && takingBack == row.ferbTask }

    /// Take it back is done once the session no longer names the stopped task, or after `takeBackWait`.
    private func settleTakeBack() {
        guard let task = takingBack, let at = takenBackAt else { return }
        let held = openSession?.ferbTask == task || sessions.contains { $0.ferbTask == task }
        if !held || now().timeIntervalSince(at) >= Self.takeBackWait {
            takingBack = nil
            takenBackAt = nil
        }
    }

    // MARK: New session

    /// New session: a fresh form (the first of the daemon's folders chosen) and the folders to choose from.
    func prepareNew() async {
        startCommand = ""
        startProblem = nil
        let reply: [String: Any]
        do { reply = try await rpc.call("sessions.prepare", params: [:]) } catch {
            startProblem = error.localizedDescription
            return
        }
        folders = reply["folders"] as? [String] ?? []
        if let app = reply["host"] as? String, !app.isEmpty { host = app }
        if startFolder.isEmpty, let first = folders.first { startFolder = first }
    }

    /// What Start's tap led to: the terminal opened, the daemon asks about its command first, or nothing (refused:
    /// `startProblem` says why).
    enum StartOutcome: Equatable { case opened, asks, refused }

    /// Start (HAND-695): a new terminal in the folder. With a command, the daemon shows exactly it first and it is typed
    /// only after the owner's Yes; with none, the terminal opens at once and nothing is typed. Pending from the tap.
    @discardableResult
    func planStart(folder: String, command: String) async -> StartOutcome {
        guard !folder.isEmpty, !starting else { return .refused }
        let command = command.trimmingCharacters(in: .whitespacesAndNewlines)
        starting = true
        startProblem = nil
        let reply: [String: Any]
        do {
            reply = try await rpc.call("sessions.prepare", params: ["folder": folder, "command": command])
        } catch {
            starting = false
            startProblem = error.localizedDescription
            return .refused
        }
        if let yes = SessionYes(reply) {
            starting = false
            confirm = Confirm(yes: yes, action: .start(folder: folder, command: command))
            return .asks
        }
        return await open(folder: folder, command: command) ? .opened : .refused
    }

    /// The terminal opens (and his command is typed, when he gave one and said Yes to it). The new session opens here
    /// when the daemon named it; else the list says it will show in a moment until it does.
    @discardableResult
    private func open(folder: String, command: String) async -> Bool {
        starting = true
        startProblem = nil
        let reply: [String: Any]
        do {
            reply = try await rpc.call("sessions.start", params: ["folder": folder, "command": command])
        } catch {
            starting = false
            startProblem = error.localizedDescription
            return false
        }
        starting = false
        startCommand = ""
        let id = reply["session"] as? String ?? ""
        if id.isEmpty { awaitingNew = Set(sessions.map(\.id)) }
        await load(quiet: true)
        if !id.isEmpty {
            awaitingNew = nil
            await open(sessions.first { $0.id == id } ?? AgentSessionRow(["id": id])!)
        }
        return true
    }

    // MARK: Uninstall

    /// FERB.app's Uninstall: the daemon takes FERB's hook out of Claude Code's settings while it still runs. A failure
    /// never stops the uninstall.
    static func removeHooks(rpc: DaemonRPCTransport) async -> Bool {
        let reply = try? await rpc.call("sessions.remove_hooks", params: [:], wait: 30)
        return reply?["removed"] as? Bool ?? false
    }
}

extension Notification.Name {
    /// The daemon's `sessions.changed`, `sessions.card`, `sessions.card_gone` or `sessions.finished`: the Sessions page
    /// reads its list and the open session again. `object` is the session id when the event named one.
    static let ferbSessionsChanged = Notification.Name("FERBSessionsChanged")
}
