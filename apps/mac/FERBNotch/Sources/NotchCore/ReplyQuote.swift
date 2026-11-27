import Foundation

// Reply to a message (owner 2026-11-27: "gotta [add] quote any ferb's reply message, either slide or an on hover icon
// maybe. many apps have that"; "yeah my messages too"). The owner quotes one message of the chat, his own or FERB's,
// when he sends a new one: `chat.send`'s `reply_to` names the quoted entry by the engine's id, and the daemon keeps
// `reply_to` {entry_id, kind, who, excerpt} on his message's entry, so every client shows the quote. Only a message
// whose engine entry id the app knows can be quoted.

/// What the owner's message quotes: the quoted entry, who said it and its opening words.
public struct ReplyQuote: Codable, Equatable {
    /// The engine's entry id of the quoted message.
    public let entryID: String
    /// The quoted entry's kind (message, answer, plan, notice…), as the daemon names it.
    public let kind: String
    /// "owner" or "ferb".
    public let who: String
    /// Its opening words, as shown.
    public let excerpt: String

    public init(entryID: String, kind: String, who: String, excerpt: String) {
        self.entryID = entryID
        self.kind = kind
        self.who = who
        self.excerpt = excerpt
    }

    /// The daemon's `reply_to` ({entry_id, kind, who, excerpt}); nil without an entry id.
    public init?(_ wire: Any?) {
        guard let wire = wire as? [String: Any], let id = wire["entry_id"] as? String, !id.isEmpty else { return nil }
        self.init(entryID: id, kind: wire["kind"] as? String ?? "", who: wire["who"] as? String ?? "ferb",
                  excerpt: wire["excerpt"] as? String ?? "")
    }

    /// A quote made here, of words the chat shows: their opening, as the daemon keeps them (its first 160 characters).
    public static func of(entryID: String, kind: String, owner: Bool, text: String) -> ReplyQuote {
        ReplyQuote(entryID: entryID, kind: kind, who: owner ? "owner" : "ferb", excerpt: String(oneLine(text).prefix(160)))
    }

    public var byOwner: Bool { who == "owner" }
    /// Who said it, as the quote names them.
    public var speaker: String { byOwner ? "You" : "FERB" }
    /// The excerpt on one line, without Markdown's marks.
    public var line: String { Self.oneLine(excerpt) }

    /// Text as one line: links as their words, Markdown's emphasis, code and list marks out, whitespace collapsed.
    public static func oneLine(_ text: String) -> String {
        var shown = text.replacingOccurrences(of: #"\[([^\]]*)\]\([^)]*\)"#, with: "$1", options: .regularExpression)
        shown = shown.replacingOccurrences(of: #"(?m)^\s*(#{1,6}\s+|[-*+]\s+|>\s*|\d+[.)]\s+)"#, with: "",
                                           options: .regularExpression)
        for mark in ["**", "__", "`"] { shown = shown.replacingOccurrences(of: mark, with: "") }
        return shown.split(whereSeparator: \.isWhitespace).joined(separator: " ")
    }

    private enum CodingKeys: String, CodingKey { case entryID, kind, who, excerpt }

    /// Read tolerantly (APP-801): only the entry id is required.
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        entryID = try c.decode(String.self, forKey: .entryID)
        kind = try c.value(.kind, or: "")
        who = try c.value(.who, or: "ferb")
        excerpt = try c.value(.excerpt, or: "")
    }
}

extension ConversationArchive {
    /// Where the transcript shows the entry the engine calls `entryID`: the owner's message or FERB's first reply at
    /// its turn's top, an answer by its own place, a later answer or a notice by its entry; nil when no entry here is it.
    public func shownTarget(ofEntry entryID: String) -> String? {
        if let turn = turns.first(where: { $0.entryID == entryID || $0.planEntryID == entryID }) {
            return TranscriptTarget.entry(turn.id)
        }
        if let turn = turns.first(where: { $0.answerEntryID == entryID }) { return TranscriptTarget.answer(turn: turn.id) }
        if let reply = replies.first(where: { $0.entryID == entryID }) { return TranscriptTarget.entry(reply.id) }
        if let notice = notices.first(where: { $0.sourceEntryID == entryID }) { return TranscriptTarget.entry(notice.entryID) }
        return nil
    }

    /// The owner's message `id` quotes `quote` (nil: none).
    public mutating func quote(id: UUID, _ quote: ReplyQuote?) {
        guard let index = turns.firstIndex(where: { $0.id == id }) else { return }
        turns[index].replyTo = quote
    }

    /// The engine's id of the notice `noticeID`, when it had none.
    public mutating func noteNoticeEntry(noticeID: String, entryID: String) {
        for index in notices.indices where notices[index].id == noticeID && notices[index].sourceEntryID == nil {
            notices[index].sourceEntryID = entryID
        }
    }

    /// The engine's id of the owner's message `id` (its ack's `entry_id`).
    public mutating func noteMessageEntry(id: UUID, entryID: String) {
        guard !entryID.isEmpty, let index = turns.firstIndex(where: { $0.id == id }) else { return }
        turns[index].entryID = entryID
    }

    /// One of this app's own entries, which its streams already drew (EntryWatch leaves those turns alone): only the
    /// entry's id is kept on what shows it, so it can be quoted, and the message's quote as the engine kept it (none:
    /// the engine took no quote). `shown`: the pos of the answer each request's turn shows. Whether anything changed.
    @discardableResult
    public mutating func noteOwnEntry(_ entry: ChatEntry, shown: inout [String: Int]) -> Bool {
        guard !entry.deleted else { return false }
        let before = (turns, replies, notices)
        switch entry.kind {
        case "notice", "post":
            guard let ref = entry.ref else { break }
            for index in notices.indices where notices[index].id == ref && notices[index].sourceEntryID == nil {
                notices[index].sourceEntryID = entry.id
            }
        case "message":
            guard let request = entry.requestID, let index = turns.lastIndex(where: { $0.requestID == request })
            else { break }
            turns[index].entryID = entry.id
            turns[index].replyTo = ReplyQuote(entry.body["reply_to"])
        case "plan":
            guard let request = entry.requestID, let index = turns.lastIndex(where: { $0.requestID == request })
            else { break }
            turns[index].planEntryID = entry.id
        case "answer", "error":
            guard let request = entry.requestID, let index = turns.lastIndex(where: { $0.requestID == request })
            else { break }
            if EntryFold.follows(entry) {
                // a later answer (APP-914) this app filed from its stream: the reply saying these words
                let words = entry.body["text"] as? String ?? ""
                let turnID = turns[index].id
                if let found = replies.lastIndex(where: { $0.turnID == turnID && $0.said?.answer == words }) {
                    replies[found].entryID = entry.id
                }
            } else if entry.pos >= shown[request] ?? Int.min {
                shown[request] = entry.pos
                turns[index].answerEntryID = entry.id
            }
        default: break
        }
        return (turns, replies, notices) != before
    }
}
