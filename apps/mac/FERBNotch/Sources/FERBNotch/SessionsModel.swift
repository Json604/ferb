import AppKit
import SwiftUI
import NotchCore

/// The FERB window's Sessions page data: the owner's Claude Code and Codex sessions through the daemon's `sessions.*`
/// calls. Each button is one call; whatever the daemon asks before sending or starting (`needs_yes`) is shown to the
/// owner first, exactly, and only the owner's Yes makes the call that acts. A refusal is shown as the daemon's words.
@MainActor
final class SessionsModel: ObservableObject {
    /// What waits for the owner's Yes: the daemon's question and the call it leads to.
    enum Action: Equatable {
        case send(session: String, text: String)
        case interrupt(session: String)
        case start(agent: String, folder: String, text: String)
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
    @Published var confirm: Confirm?
    @Published var pick: Pick?
    /// New session: the folders to choose from and the agents, from `sessions.prepare {}`.
    @Published private(set) var folders: [String] = []
    @Published private(set) var agents: [String] = ["claude_code", "codex"]
    /// Why the last call failed (the daemon's own words), and the last action's one line.
    @Published var problem: String?
    @Published var note: String?
    @Published private(set) var busy = false

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
    }

    // MARK: One session

    func open(_ row: AgentSessionRow) async {
        guard openID != row.id else { return }
        openID = row.id
        openSession = row
        items = []
        cards = []
        more = false
        draft = ""
        note = nil
        problem = nil
        await loadTimeline()
    }

    func close() {
        openID = nil
        openSession = nil
        items = []
        cards = []
        more = false
        draft = ""
        note = nil
    }

    /// The newest page of the open session.
    func loadTimeline() async {
        guard let id = openID else { return }
        guard let reply = await ask("sessions.timeline", ["session": id, "limit": Self.page]), openID == id else { return }
        let timeline = SessionTimeline(reply)
        items = timeline.items
        more = timeline.more
        cards = timeline.cards
        if let session = timeline.session { openSession = session }
    }

    /// What is new since the last item held (every ~2 s while the page shows, and on the daemon's events).
    func poll() async {
        guard let id = openID else { return }
        guard let last = items.last?.seq else { return await loadTimeline() }
        guard let reply = await ask("sessions.timeline", ["session": id, "after": last, "limit": Self.page * 2],
                                    quiet: true), openID == id else { return }
        let timeline = SessionTimeline(reply)
        let held = Set(items.map(\.seq))
        items += timeline.items.filter { !held.contains($0.seq) }
        cards = timeline.cards
        if let session = timeline.session { openSession = session }
    }

    /// Show earlier: the page before the first item held.
    func showEarlier() async {
        guard let id = openID, let first = items.first?.seq else { return }
        guard let reply = await ask("sessions.timeline", ["session": id, "before": first, "limit": Self.page]),
              openID == id else { return }
        let timeline = SessionTimeline(reply)
        let held = Set(items.map(\.seq))
        items = timeline.items.filter { !held.contains($0.seq) } + items
        more = timeline.more
    }

    // MARK: Sending, answering, stopping

    /// The composer's Send: the daemon either sends (the owner already said Yes here) or asks first, showing the
    /// exact text; nothing is sent until that Yes.
    func send() async {
        guard let id = openID else { return }
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, !busy else { return }
        busy = true
        defer { busy = false }
        guard let reply = await sending("sessions.send", ["session": id, "text": text], session: id, text: text) else { return }
        if let yes = SessionYes(reply) {
            confirm = Confirm(yes: yes, action: .send(session: id, text: text))
            return
        }
        sent(reply, text: text)
        await poll()
    }

    private func sent(_ reply: [String: Any], text: String) {
        guard reply["sent"] as? Bool == true else { return }
        if draft.trimmingCharacters(in: .whitespacesAndNewlines) == text { draft = "" }
        note = reply["delivered"] as? Bool == false ? "Sent, but FERB couldn't see it arrive yet." : nil
    }

    /// The owner's Yes to the question shown: control of the session from here and the text he read, the interrupt
    /// he asked for, or the new session's command.
    func confirmYes() async {
        guard let pending = confirm else { return }
        confirm = nil
        busy = true
        defer { busy = false }
        switch pending.action {
        case .send(let id, let text):
            guard let reply = await sending("sessions.control", ["session": id, "text": text], session: id, text: text) else { return }
            sent(reply, text: text)
            await load(quiet: true)
            await poll()
        case .interrupt(let id):
            guard await ask("sessions.control", ["session": id, "text": ""]) != nil else { return }
            guard await ask("sessions.interrupt", ["session": id]) != nil else { return }
            note = "You stopped its turn."
            await load(quiet: true)
            await poll()
        case .start(let agent, let folder, let text):
            guard await ask("sessions.start", ["agent": agent, "folder": folder, "text": text]) != nil else { return }
            note = "Starting it in Terminal on your Mac."
            await load(quiet: true)
        }
    }

    func cancelConfirm() { confirm = nil }

    /// A send's call; when the daemon can't tell which terminal is the session's (HAND-683), the terminals it can see
    /// come from the session, for the owner to choose.
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
            if !terminals.isEmpty {
                pick = Pick(session: session, text: text, why: error.localizedDescription, terminals: terminals)
            }
            return nil
        } catch {
            problem = error.localizedDescription
            return nil
        }
    }

    /// HAND-683: the owner's choice of terminal for these words: FERB types only there, keeps it for the session while
    /// he controls it from here, and asks again if it changes.
    func choose(_ terminal: SessionChoice) async {
        guard let p = pick else { return }
        pick = nil
        busy = true
        defer { busy = false }
        guard let reply = await sending("sessions.control", ["session": p.session, "text": p.text, "terminal": terminal.id],
                                        session: p.session, text: p.text) else { return }
        sent(reply, text: p.text)
        await load(quiet: true)
        await poll()
    }

    /// Don't send: nothing is typed; the words stay in the box.
    func cancelPick() { pick = nil }

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

    /// Hand to FERB: FERB works the session toward the owner's goal, in his chat.
    @discardableResult
    func delegate(goal: String) async -> Bool {
        guard let id = openID else { return false }
        let goal = goal.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !goal.isEmpty else { return false }
        var params: [String: Any] = ["session": id, "goal": goal]
        if let client = clientID(), !client.isEmpty { params["client_id"] = client }
        guard await ask("sessions.delegate", params) != nil else { return false }
        note = "FERB is on it. You'll see it in the chat."
        await load(quiet: true)
        return true
    }

    // MARK: New session

    func prepareNew() async {
        guard let reply = await ask("sessions.prepare") else { return }
        folders = reply["folders"] as? [String] ?? []
        if let named = reply["agents"] as? [String], !named.isEmpty { agents = named }
    }

    /// Start: the daemon shows the exact command it would run; it runs only after the owner's Yes.
    @discardableResult
    func planStart(agent: String, folder: String, text: String) async -> Bool {
        guard !folder.isEmpty else { return false }
        let first = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let reply = await ask("sessions.prepare", ["agent": agent, "folder": folder, "text": first]),
              let yes = SessionYes(reply) else { return false }
        confirm = Confirm(yes: yes, action: .start(agent: agent, folder: folder, text: first))
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
