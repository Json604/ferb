import Foundation

/// The temporary chat (owner 2026-11-23: "temporary chat - where there are all the tools n shit BUT no prior context
/// given"; "the output,messages are all in ememory"): one conversation the daemon holds in memory only, turned on and
/// off by the same small icon. FERB.app holds only what it shows of it and never saves any of it: no chat history row,
/// no archive, no entry-watch write.
///
/// It is a second data source for the one chat FERB.app has (owner 2026-11-24: "i told you to match normal chat and
/// temp chat UI, why are they even diff???"): `archive` is a conversation like the saved one, written by the same
/// pipeline (the composer's send, its stream's frames, a task's outcome, a question's placement, the daemon's entries
/// for the turns other devices sent), only never kept. This type is what differs: its id, its change number and the
/// marks its ids carry.
public struct TemporaryChat: Equatable {
    /// Every temporary conversation's id starts so (`temporary:<id>`).
    public static let conversationPrefix = "temporary:"
    /// Every temporary chat's request id starts so.
    public static let requestPrefix = "temporary-"

    public let id: String
    public let conversation: String
    public var rev: Int
    public var startedAt: String
    /// What the chat shows, written by the app's one chat pipeline as the saved chat's archive is.
    public var archive = ConversationArchive()

    /// From `chat.temporary.get` or `chat.temporary.set`'s reply: nil when it says off (or isn't a temporary chat).
    /// Its entries are read apart (`entries(of:)`), by the same entry path the saved chat's watch uses.
    public init?(reply: [String: Any]) {
        guard reply["on"] as? Bool == true, let id = reply["id"] as? String, !id.isEmpty else { return nil }
        self.id = id
        conversation = reply["conversation"] as? String ?? Self.conversationPrefix + id
        rev = (reply["rev"] as? NSNumber)?.intValue ?? 0
        startedAt = reply["started_at"] as? String ?? ""
    }

    /// The reply's entries, each one's newest, in the conversation's order (`pos`).
    public static func entries(of reply: [String: Any]) -> [ChatEntry] {
        var newest: [String: ChatEntry] = [:]
        for entry in ChatEntry.list(reply["entries"]) where (newest[entry.id]?.seq ?? Int.min) < entry.seq {
            newest[entry.id] = entry
        }
        return newest.values.sorted { $0.pos < $1.pos }
    }

    /// Whether a request id is a temporary chat's message's.
    public static func isTemporaryRequest(_ request: String?) -> Bool {
        request?.hasPrefix(requestPrefix) == true
    }

    /// Whether a conversation id is a temporary chat's.
    public static func isTemporary(_ conversation: String?) -> Bool {
        conversation?.hasPrefix(conversationPrefix) == true
    }

    /// Whether a task (or a step under it, `root.n`) is one of this chat's turns'.
    public func holds(taskID: String?) -> Bool {
        guard let taskID, !taskID.isEmpty else { return false }
        return archive.turns.contains { turn in
            guard let root = turn.backendTaskID, !root.isEmpty else { return false }
            return taskID == root || taskID.hasPrefix(root + ".")
        }
    }
}
