import Foundation

/// The owner's coding sessions (Claude Code, Codex) as the daemon's `sessions.*` calls describe them. A row the daemon
/// sent without its id is dropped; every other missing field reads as empty, so an older or newer daemon still shows
/// what it has. Nothing here decides anything: the words shown are the daemon's facts in the contract's plain words.

private func string(_ value: Any?) -> String { value as? String ?? "" }
private func words(_ value: Any?) -> String? { (value as? String).flatMap { $0.isEmpty ? nil : $0 } }
private func number(_ value: Any?) -> Int? { (value as? NSNumber)?.intValue }

/// The app that shows a session: a fact from the Mac's process tree.
public struct SessionHost: Equatable {
    public let bundleID: String
    public let name: String
    public let pid: Int?
    public let via: String
    public let tty: String

    public init?(_ row: [String: Any]?) {
        guard let row else { return nil }
        bundleID = string(row["bundle_id"])
        name = string(row["name"])
        pid = number(row["pid"])
        via = string(row["via"])
        tty = string(row["tty"])
    }
}

/// One session (`sessions.list` `sessions`, or `sessions.timeline` `session`).
public struct AgentSessionRow: Identifiable, Equatable {
    public let id: String
    /// "claude_code" or "codex".
    public let agent: String
    public let cwd: String
    /// "" before the agent named it.
    public let title: String
    /// "working", "waiting_for_you" or "idle".
    public let status: String
    /// "none", "permission" or "question".
    public let waiting: String
    public let host: SessionHost?
    /// The ways FERB can deliver the owner's text, in order; empty: nothing can.
    public let sendPaths: [String]
    /// Why nothing can deliver, in the owner's words.
    public let noSend: String?
    public let lastAt: Date?
    public let lastText: String
    /// "mac", "phone:<id>" or nil.
    public let control: String?
    /// The FERB task working on it, when it was handed to FERB.
    public let delegate: String?
    public let goal: String?
    /// The caller (this app) controls it: Stop controlling shows.
    public let yours: Bool
    /// HAND-683: FERB couldn't tell which terminal in its app is this session's: the ones it can see, for the owner to
    /// choose (empty otherwise).
    public let terminals: [SessionChoice]
    /// HAND-692: the program in front in its terminal ("agy", "zsh", "claude"; "" when unknown), and its window in its
    /// app's own words.
    public let program: String
    public let window: String
    /// HAND-693: why FERB can't show this terminal's text (nil when it can).
    public let noText: String?
    /// HAND-702: FERB's latest word on the owner's hand-over, in the daemon's plain words, shown as it is ("" for none).
    public let ferbNote: String

    public init?(_ row: [String: Any]) {
        guard let id = row["id"] as? String, !id.isEmpty else { return nil }
        self.id = id
        agent = string(row["agent"])
        cwd = string(row["cwd"])
        title = string(row["title"])
        status = string(row["status"])
        waiting = words(row["waiting"]) ?? "none"
        host = SessionHost(row["host"] as? [String: Any])
        sendPaths = (row["send_paths"] as? [Any] ?? []).compactMap { $0 as? String }
        noSend = words(row["no_send"])
        lastAt = WindowTime.parse(row["last_at"])
        lastText = string(row["last_text"])
        control = words(row["control"])
        delegate = words(row["delegate"])
        goal = words(row["goal"])
        yours = (row["yours"] as? NSNumber)?.boolValue ?? false
        terminals = (row["terminals"] as? [[String: Any]] ?? []).compactMap { t in
            guard let id = t["id"] as? String, !id.isEmpty else { return nil }
            return SessionChoice(id: id, label: string(t["label"]))
        }
        program = string(row["program"])
        window = string(row["window"])
        noText = words(row["no_text"])
        ferbNote = string(row["ferb_note"])
    }

    /// The agent as the owner calls it.
    public var agentName: String {
        // HAND-668: an app chat is named by its app
        if agent == "app_chat", let name = host?.name, !name.isEmpty { return name }
        // HAND-692: a terminal by the program in front in it
        if agent == "terminal", !program.isEmpty { return program }
        return Self.agentName(agent)
    }

    public static func agentName(_ agent: String) -> String {
        switch agent {
        case "claude_code": return "Claude Code"
        case "codex": return "Codex"
        case "app_chat": return "App chat"
        case "terminal", "": return "Terminal"
        default: return agent.replacingOccurrences(of: "_", with: " ")
        }
    }

    /// The folder's last part ("site" for /Users/o/code/site); the whole path when it has none.
    public var folderName: String {
        let name = (cwd as NSString).lastPathComponent
        return name.isEmpty || name == "/" ? cwd : name
    }

    /// The row's heading: the session's title, else its window's in its app's words (HAND-692), else its folder's name,
    /// else its agent's name: never empty (owner 2026-11-23: a row read "in the background · Idle" with no title).
    public var heading: String {
        [title, window, folderName, agentName].first { !$0.trimmingCharacters(in: .whitespaces).isEmpty } ?? "Terminal"
    }

    /// Where it runs: "<program> · <folder> · in <app>" (HAND-692; no program: "<folder> · in <app>"), or "in the
    /// background" when no app shows it.
    public var whereLine: String {
        let app = host?.name ?? ""
        return [program, folderName, app.isEmpty ? "in the background" : "in \(app)"].filter { !$0.isEmpty }.joined(separator: " · ")
    }

    /// HAND-697, HAND-705: an app open with no agent chat FERB could find.
    public static func noChatLine(_ app: String) -> String {
        "No agent chat open in \(app). Start one there to continue here."
    }

    public var canSend: Bool { !sendPaths.isEmpty }
    /// Sending brings its app forward on the Mac.
    public var foreground: Bool { sendPaths.first == "foreground" }
    /// FERB's task keeping this session going ("" while none has started: FERB is still starting on the hand-over).
    public var ferbTask: String { delegate ?? "" }
    /// Handed to FERB (its task runs, or it is starting on his goal): the "FERB is on it" badge.
    public var withFerb: Bool { delegate != nil || goal != nil }

    /// The state chip's words.
    public var stateWords: String {
        switch status {
        case "working": return "Working"
        case "waiting_for_you": return "Waiting for you"
        default: return "Idle"
        }
    }
}

/// One line of a session's conversation (`sessions.timeline` `items`).
public struct SessionItem: Identifiable, Equatable {
    public var id: Int { seq }
    public let seq: Int
    public let at: Date?
    /// "user", "assistant", "tool", "interrupted" or "error".
    public let kind: String
    public let text: String
    public let tool: String?
    /// HAND-703: the reply's parts in order, as its app shows them (an app chat's turn); empty: the item is its text.
    public let parts: [SessionPart]
    /// HAND-709: a message FERB sent while it kept the session going (drawn as FERB's, not the owner's).
    public let ferb: Bool

    public init?(_ row: [String: Any]) {
        guard let seq = number(row["seq"]) else { return nil }
        self.seq = seq
        at = WindowTime.parse(row["at"])
        kind = string(row["kind"])
        self.text = string(row["text"])
        tool = words(row["tool"])
        parts = (row["parts"] as? [[String: Any]] ?? []).map(SessionPart.init)
        ferb = (row["ferb"] as? NSNumber)?.boolValue ?? false
    }

    /// A tool row as one line: "Bash: npm test".
    public var line: String {
        guard let tool else { return text }
        return text.isEmpty ? tool : "\(tool): \(text)"
    }

    /// How the conversation draws this item. `agent` is the session's ("terminal": its output is fixed-width text,
    /// never Markdown).
    public func look(agent: String) -> SessionItemLook {
        switch kind {
        case "user": return ferb ? .ferbSent(text) : .owner(text)
        case "tool": return .toolLine(line)
        case "interrupted": return .interrupted(text)
        case "error": return .error(text)
        default:
            // one reply block: its parts in order, or its text as one Markdown part (Claude Code, Codex, an app chat)
            if !parts.isEmpty { return .reply(parts) }
            if agent == "terminal" { return .fixedWidth(text) }
            return .reply([SessionPart(kind: "text", text: text)])
        }
    }
}

/// HAND-703: one part of a reply (`items[].parts`).
public struct SessionPart: Equatable {
    /// "text" (Markdown), "thinking", "tool" or "meta"; any other kind is drawn as text.
    public let kind: String
    public let text: String
    /// A thought's or a step's own row ("Thought 1s", "List zip sizes and contents").
    public let label: String
    /// A step's second words, dimmer on its row ("ls, echo, unzip, head").
    public let detail: String

    public init(kind: String, text: String = "", label: String = "", detail: String = "") {
        self.kind = kind
        self.text = text
        self.label = label
        self.detail = detail
    }

    public init(_ row: [String: Any]) {
        self.init(kind: string(row["kind"]), text: string(row["text"]), label: string(row["label"]),
                  detail: string(row["detail"]))
    }

    /// The kind as drawn: an unknown kind is text.
    public var shown: String { ["text", "thinking", "tool", "meta"].contains(kind) ? kind : "text" }
    /// A thought or a step opens to show its words only when it has some (no chevron otherwise).
    public var expandable: Bool { (shown == "thinking" || shown == "tool") && !text.isEmpty }
    /// A thought's or a step's row: its label; a step without one goes by what it ran, a thought by "Thinking".
    public var rowLabel: String {
        if !label.isEmpty { return label }
        if shown == "tool", !detail.isEmpty { return detail }
        return shown == "thinking" ? "Thinking" : ""
    }
    /// The step's second words on its row (not repeated when they stand in for the label).
    public var rowDetail: String { label.isEmpty ? "" : detail }
}

/// How one conversation item is drawn.
public enum SessionItemLook: Equatable {
    /// The owner's words: one bubble, plain text.
    case owner(String)
    /// HAND-709: words FERB sent while it kept the session going: the owner's side, marked as FERB's.
    case ferbSent(String)
    /// The agent's reply: one block holding its parts in order (meta lines under it, never in a bubble).
    case reply([SessionPart])
    /// A terminal's output: fixed-width text exactly as it came.
    case fixedWidth(String)
    /// A transcript's tool call: one small line.
    case toolLine(String)
    case interrupted(String)
    case error(String)
}

/// One answer a card offers.
public struct SessionChoice: Identifiable, Equatable {
    public let id: String
    public let label: String
    public init(id: String, label: String) {
        self.id = id
        self.label = label
    }
}

/// The agent's open permission prompt or question (`sessions.timeline` `cards`).
public struct SessionCard: Identifiable, Equatable {
    public let id: String
    public let session: String
    /// "permission", "question", or "ferb" (FERB's own question about its drive of the session, HAND-709).
    public let kind: String
    public let tool: String?
    /// The exact thing asked about (a command, a file), shown in monospace.
    public let detail: String
    public let question: String?
    public let choices: [SessionChoice]
    public let askedAt: Date?
    public let approvalID: String?

    public init?(_ row: [String: Any]) {
        guard let id = row["id"] as? String, !id.isEmpty else { return nil }
        self.id = id
        session = string(row["session"])
        kind = string(row["kind"])
        tool = words(row["tool"])
        detail = string(row["detail"])
        question = words(row["question"])
        choices = (row["choices"] as? [[String: Any]] ?? []).compactMap { choice in
            guard let id = choice["id"] as? String, !id.isEmpty else { return nil }
            return SessionChoice(id: id, label: string(choice["label"]))
        }
        askedAt = WindowTime.parse(row["asked_at"])
        approvalID = words(row["approval_id"])
    }

    /// The card's heading: "Claude Code wants to use Bash", "Codex asks", or FERB's own question (HAND-709).
    public func heading(agentName: String) -> String {
        if kind == "permission" { return "\(agentName) wants to use \(tool ?? "a tool")" }
        if kind == "ferb" { return "FERB asks" }
        return "\(agentName) asks"
    }
}

/// One `sessions.timeline` answer.
public struct SessionTimeline: Equatable {
    public let session: AgentSessionRow?
    public let items: [SessionItem]
    /// There are earlier items (page back with `before` = the first seq held).
    public let more: Bool
    public let cards: [SessionCard]

    public init(_ reply: [String: Any]) {
        session = (reply["session"] as? [String: Any]).flatMap(AgentSessionRow.init)
        items = (reply["items"] as? [[String: Any]] ?? []).compactMap(SessionItem.init).sorted { $0.seq < $1.seq }
        more = (reply["more"] as? NSNumber)?.boolValue ?? false
        cards = (reply["cards"] as? [[String: Any]] ?? []).compactMap(SessionCard.init)
    }
}

/// What a send, a start or an interrupt asks first (`needs_yes`): the question, the exact text in monospace, and what
/// the Yes covers. Nothing was sent or run yet.
public struct SessionYes: Equatable {
    public let question: String
    public let detail: String
    public let covers: String

    public init(question: String, detail: String, covers: String) {
        self.question = question
        self.detail = detail
        self.covers = covers
    }

    public init?(_ reply: [String: Any]?) {
        guard let row = reply?["needs_yes"] as? [String: Any] else { return nil }
        question = string(row["question"])
        detail = string(row["detail"])
        covers = string(row["covers"])
    }
}

/// The daemon's refusal codes for `sessions.*` (the message is the owner's words and is shown as it is).
public enum SessionRefusal {
    public static let sessionGone = -32040
    public static let notInFront = -32041
    public static let noSendPath = -32042
    public static let cardOpen = -32043
    public static let sessionsOff = -32044
    /// The caller doesn't control the session: the owner's Yes (`sessions.control`) first.
    public static let notTheSession = -32045
    public static let cardGone = -32046
    public static let ownersOnly = -32047
    public static let sendFailed = -32048
    /// HAND-683: FERB can't tell which terminal is the session's: the session lists them for the owner to choose.
    public static let chooseTerminal = -32049
}
