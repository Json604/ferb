import Foundation

/// The temporary chat (owner 2026-11-23: "temporary chat - where there are all the tools n shit BUT no prior context
/// given"; "the output,messages are all in ememory"): one conversation the daemon holds in memory only, turned on and
/// off by the same small icon. FERB.app holds only what it shows of it, read from `chat.temporary.get` (its entries,
/// ENG-8, folded by `EntryFold`), and never saves any of it: no chat history row, no archive, no entry-watch write.
/// The messages this app is sending now wait here (`pending`) until their entries come.
public struct TemporaryChat: Equatable {
    /// Every temporary conversation's id starts so (`temporary:<id>`).
    public static let conversationPrefix = "temporary:"

    public let id: String
    public let conversation: String
    public private(set) var rev: Int
    public private(set) var startedAt: String
    /// The daemon's entries, folded as the app folds every conversation's.
    public private(set) var folded = ConversationArchive()
    /// The requests and tasks of its turns: the questions they ask show here, their results never reach the saved chat.
    public private(set) var requests: Set<String> = []
    public private(set) var tasks: Set<String> = []
    /// The messages sent from here whose stream is still open, or whose entry hasn't come yet.
    public private(set) var pending: [PendingSend] = []
    /// What the chat shows: the folded entries, with the messages still on their way.
    public private(set) var archive = ConversationArchive()

    /// One message sent from this app: its turn as drawn until its entry comes, and its stream's state.
    public struct PendingSend: Equatable {
        public var turn: ConversationTurn
        public var streamOpen = true
    }

    /// From `chat.temporary.get` or `chat.temporary.set`'s reply: nil when it says off (or isn't a temporary chat).
    public init?(reply: [String: Any]) {
        guard reply["on"] as? Bool == true, let id = reply["id"] as? String, !id.isEmpty else { return nil }
        self.id = id
        conversation = reply["conversation"] as? String ?? Self.conversationPrefix + id
        rev = (reply["rev"] as? NSNumber)?.intValue ?? 0
        startedAt = reply["started_at"] as? String ?? ""
        read(ChatEntry.list(reply["entries"]))
    }

    /// Every temporary chat's request id starts so.
    public static let requestPrefix = "temporary-"

    /// Whether a request id is a temporary chat's message's.
    public static func isTemporaryRequest(_ request: String?) -> Bool {
        request?.hasPrefix(requestPrefix) == true
    }

    /// Whether a conversation id is a temporary chat's.
    public static func isTemporary(_ conversation: String?) -> Bool {
        conversation?.hasPrefix(conversationPrefix) == true
    }

    /// A newer reply for the same chat: its entries replace the ones held; a message whose stream ended and whose
    /// entry came stops waiting.
    public mutating func take(_ newer: TemporaryChat) {
        guard newer.id == id else { return }
        rev = newer.rev
        startedAt = newer.startedAt
        folded = newer.folded
        requests = newer.requests
        tasks = newer.tasks.union(pending.compactMap(\.turn.backendTaskID))
        pending.removeAll { !$0.streamOpen && $0.turn.requestID.map(requests.contains) == true }
        rebuild()
    }

    /// Whether a task (or a step under it, `root.n`) is one of this chat's.
    public func owns(taskID: String?) -> Bool {
        guard let taskID, !taskID.isEmpty else { return false }
        return tasks.contains { taskID == $0 || taskID.hasPrefix($0 + ".") }
    }

    /// Whether a chat.status row (running, a result) is one of this chat's tasks: by its task, request or card.
    public func owns(row: [String: Any]) -> Bool {
        if owns(taskID: row["task_id"] as? String) { return true }
        if Self.isTemporary(row["conversation"] as? String) { return true }
        let request = row["request_id"] as? String ?? (row["card"] as? [String: Any])?["request_id"] as? String
        return request.map { requests.contains($0) || Self.isTemporaryRequest($0) } ?? false
    }

    // MARK: a message sent from here

    /// The owner sent `text` (with `sources`): it shows at once, as its own turn, until its entry comes.
    public mutating func send(id: UUID, text: String, sources: [URL], at date: Date) {
        guard !pending.contains(where: { $0.turn.id == id }) else { return }
        pending.append(PendingSend(turn: ConversationTurn(id: id, createdAt: date, request: text, sources: sources)))
        rebuild()
    }

    /// A frame of its stream named its request (every frame does): its entry is found by it. An `ack` starts the
    /// typing dots.
    public mutating func bind(id: UUID, requestID: String?, acknowledged: Bool) {
        change(id) { send in
            if let requestID, !requestID.isEmpty, send.turn.requestID == nil { send.turn.requestID = requestID }
            if acknowledged { send.turn.awaitingReply = true }
        }
    }

    /// Its task started (`plan`): its questions and results belong here from now on.
    public mutating func planned(id: UUID, taskID: String) {
        guard !taskID.isEmpty else { return }
        tasks.insert(taskID)
        change(id) { $0.turn.backendTaskID = taskID }
    }

    /// The reply's words as they stream (`reply_delta`), shown until the answer's entry comes.
    public mutating func stream(id: UUID, delta: String) {
        guard !delta.isEmpty else { return }
        change(id) { $0.turn.answer += delta }
    }

    /// The stream failed or FERB couldn't start the request: the message says so where it is.
    public mutating func failed(id: UUID, reason: String) {
        change(id) { send in
            send.streamOpen = false
            send.turn.awaitingReply = false
            send.turn.answer = reason
            send.turn.status = "failed"
        }
    }

    /// Its stream ended: a message whose entry came stops waiting; one whose entry never came says its reply wasn't
    /// confirmed.
    public mutating func ended(id: UUID, unconfirmed: String) {
        guard let index = pending.firstIndex(where: { $0.turn.id == id }) else { return }
        if let request = pending[index].turn.requestID, requests.contains(request) {
            pending.remove(at: index)
        } else if pending[index].streamOpen {
            pending[index].streamOpen = false
            pending[index].turn.awaitingReply = false
            if pending[index].turn.status == "working" {
                pending[index].turn.answer = ""
                pending[index].turn.status = "interrupted"
                pending[index].turn.responseNotice = unconfirmed
                pending[index].turn.statusInferred = true
            }
        }
        rebuild()
    }

    // MARK: building what shows

    private mutating func change(_ id: UUID, _ edit: (inout PendingSend) -> Void) {
        guard let index = pending.firstIndex(where: { $0.turn.id == id }) else { return }
        edit(&pending[index])
        rebuild()
    }

    private mutating func read(_ entries: [ChatEntry]) {
        folded = EntryFold.archive(entries)
        var requests = Set<String>(), tasks = Set<String>()
        for entry in entries {
            if entry.kind == "message", let request = entry.requestID { requests.insert(request) }
            if let task = entry.taskID, !task.isEmpty { tasks.insert(task) }
        }
        self.requests = requests
        self.tasks = tasks
        rebuild()
    }

    /// The folded entries, then each message still on its way: one whose entry came lends it its streamed words and
    /// typing dots while its stream is open; one whose entry hasn't come is its own turn, after the others.
    private mutating func rebuild() {
        var shown = folded
        for send in pending {
            if let request = send.turn.requestID, let index = shown.turns.lastIndex(where: { $0.requestID == request }) {
                guard send.streamOpen, shown.turns[index].status == "working" else { continue }
                if shown.turns[index].answer.isEmpty, !send.turn.answer.isEmpty, shown.turns[index].backendTaskID == nil {
                    shown.turns[index].answer = send.turn.answer
                }
                shown.turns[index].awaitingReply = send.turn.awaitingReply
            } else {
                shown.turns.append(send.turn)
            }
        }
        archive = shown
    }
}
