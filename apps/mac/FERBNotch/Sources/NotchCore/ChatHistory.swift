import Foundation

// APP-805 (owner 2026-11-09: "A. FERB's database only"): the chat lives in FERB's database, one row per entry in the
// shape the app draws it; the app holds what it shows and writes each entry that changed. APP-806 (owner: "30, but on
// scroll up it should automatically load those"): a day opens on its newest 30 entries, and earlier ones come in as the
// owner scrolls up.

/// One saved entry of the chat as the app holds it.
public enum StoredEntry: Equatable {
    case turn(ConversationTurn)
    case reply(ConversationReply)
    case notice(ConversationNotice)

    public var id: UUID {
        switch self {
        case let .turn(turn): return turn.id
        case let .reply(reply): return reply.id
        case let .notice(notice): return notice.entryID
        }
    }

    public var createdAt: Date {
        switch self {
        case let .turn(turn): return turn.createdAt
        case let .reply(reply): return reply.createdAt
        case let .notice(notice): return notice.createdAt
        }
    }

    var kind: String {
        switch self {
        case .turn: return "turn"
        case .reply: return "reply"
        case .notice: return "notice"
        }
    }

    /// A turn still waiting for FERB's final answer: the app keeps these in hand from launch, whatever their day.
    var open: Bool {
        if case let .turn(turn) = self { return ["working", "needs_user"].contains(turn.status) }
        return false
    }

    var ref: String? {
        if case let .notice(notice) = self { return notice.id }
        return nil
    }

    func encoded() throws -> Data {
        let encoder = JSONEncoder()
        encoder.outputFormatting = .sortedKeys
        switch self {
        case let .turn(turn): return try encoder.encode(turn)
        case let .reply(reply): return try encoder.encode(reply)
        case let .notice(notice): return try encoder.encode(notice)
        }
    }
}

/// An entry as `chat.history.*` carries it: `{id, kind, at, day, open, ref?, body}`.
public struct HistoryEntry: Equatable {
    public let value: StoredEntry
    public let day: String
    public let body: Data

    public init(_ value: StoredEntry, calendar: Calendar = .current) throws {
        self.value = value
        day = HistoryEntry.day(value.createdAt, calendar: calendar)
        body = try value.encoded()
    }

    public static func day(_ date: Date, calendar: Calendar = .current) -> String {
        let parts = calendar.dateComponents([.year, .month, .day], from: date)
        return String(format: "%04d-%02d-%02d", parts.year ?? 0, parts.month ?? 0, parts.day ?? 0)
    }

    /// The day `day` names, at its start in `calendar`.
    public static func date(_ day: String, calendar: Calendar = .current) -> Date? {
        let parts = day.split(separator: "-").compactMap { Int($0) }
        guard parts.count == 3 else { return nil }
        return calendar.date(from: DateComponents(year: parts[0], month: parts[1], day: parts[2]))
    }

    public var wire: [String: Any] {
        var out: [String: Any] = ["id": value.id.uuidString, "kind": value.kind, "day": day, "open": value.open,
                                  "at": Date.ISO8601FormatStyle(includingFractionalSeconds: true).format(value.createdAt),
                                  "body": (try? JSONSerialization.jsonObject(with: body)) ?? [:]]
        if let ref = value.ref { out["ref"] = ref }
        return out
    }

    /// Reads one entry as the store gave it back, tolerantly (APP-801): nil when it can't be read.
    public static func read(_ wire: Any) -> StoredEntry? {
        guard let wire = wire as? [String: Any], let kind = wire["kind"] as? String, let body = wire["body"],
              JSONSerialization.isValidJSONObject(body), let data = try? JSONSerialization.data(withJSONObject: body)
        else { return nil }
        let decoder = JSONDecoder()
        switch kind {
        case "turn": return (try? decoder.decode(ConversationTurn.self, from: data)).map {
            var turn = $0
            turn.request = PasteMarkers.removed(from: turn.request)
            return .turn(turn)
        }
        case "reply": return (try? decoder.decode(ConversationReply.self, from: data)).map { .reply($0) }
        case "notice": return (try? decoder.decode(ConversationNotice.self, from: data)).map { .notice($0) }
        default: return nil
        }
    }
}

extension ConversationArchive {
    /// Every entry as it may be saved (`forStorage`), in the archive's order.
    public var storedEntries: [StoredEntry] {
        let saved = forStorage()
        return saved.turns.map(StoredEntry.turn) + saved.replies.map(StoredEntry.reply) + saved.notices.map(StoredEntry.notice)
    }

    /// Entries the store gave back, added where they belong by date. An entry this chat already holds (the same id, a
    /// turn for a request a turn here shows, a notice already posted) stays as it is here: what the app holds is the
    /// newer. Returns the entries added.
    @discardableResult
    public mutating func absorb(_ entries: [StoredEntry]) -> [StoredEntry] {
        var page = ConversationArchive()
        for entry in entries {
            switch entry {
            case let .turn(turn): page.turns.append(turn)
            case let .reply(reply): page.replies.append(reply)
            case let .notice(notice): page.notices.append(notice)
            }
        }
        page.turns.sort { $0.createdAt < $1.createdAt }
        page.replies.sort { $0.createdAt < $1.createdAt }
        page.notices.sort { $0.createdAt < $1.createdAt }
        let before = Set(turns.map(\.id) + replies.map(\.id) + notices.map(\.entryID))
        restore(page)
        let added = Set(turns.map(\.id) + replies.map(\.id) + notices.map(\.entryID)).subtracting(before)
        return entries.filter { added.contains($0.id) }
    }
}

/// What the store holds of the entries the app has in hand, so each save writes only the entries that changed
/// (`chat.history.put`), never the whole chat.
public struct HistorySync {
    private var held: [UUID: StoredEntry] = [:]
    private var bodies: [UUID: Data] = [:]

    public init() {}

    /// The entries that differ from what the store holds. An entry whose saved form is unchanged (only live state moved)
    /// is noted as held and not written.
    public mutating func changes(in archive: ConversationArchive, calendar: Calendar = .current) -> [HistoryEntry] {
        var out: [HistoryEntry] = []
        for value in archive.storedEntries where held[value.id] != value {
            guard let entry = try? HistoryEntry(value, calendar: calendar) else { continue }
            if bodies[value.id] == entry.body { held[value.id] = value } else { out.append(entry) }
        }
        return out
    }

    /// The store now holds these.
    public mutating func stored(_ entries: [HistoryEntry]) {
        for entry in entries {
            held[entry.value.id] = entry.value
            bodies[entry.value.id] = entry.body
        }
    }

    /// The store gave these back: they are held as they are.
    public mutating func loaded(_ entries: [StoredEntry], calendar: Calendar = .current) {
        stored(entries.compactMap { try? HistoryEntry($0, calendar: calendar) })
    }

    public var isEmpty: Bool { held.isEmpty }
}

/// Which of a day's entries are shown (APP-806): from the newest back to `from`; earlier ones come in on scroll, from
/// what the app already holds first, then from the store a page at a time.
public struct DayWindow: Equatable {
    /// Entries made before this aren't shown yet; nil: every entry the app holds for the day is shown.
    public var from: Date?
    /// The store's `before` for the day's next earlier page; nil when the store has nothing earlier (or isn't used).
    public var cursor: String?
    /// A page for this day is being read from the store.
    public var loading = false

    public init(from: Date? = nil, cursor: String? = nil, loading: Bool = false) {
        self.from = from
        self.cursor = cursor
        self.loading = loading
    }

    public static let page = 30

    /// The window over entries the app holds (no store, or what it holds is enough): the newest `page` of them.
    public static func newest(_ entries: [TranscriptEntry], page: Int = page) -> DayWindow {
        DayWindow(from: entries.count > page ? entries[entries.count - page].createdAt : nil)
    }

    public func shown(_ entries: [TranscriptEntry]) -> [TranscriptEntry] {
        guard let from else { return entries }
        return entries.filter { $0.createdAt >= from }
    }

    /// Whether anything earlier can come in: held entries not shown yet, or a page the store still has.
    public func hasEarlier(_ entries: [TranscriptEntry]) -> Bool {
        cursor != nil || (from.map { from in entries.contains { $0.createdAt < from } } ?? false)
    }

    /// Shows up to `page` more of the held entries; false when none is hidden (the store's next page is needed).
    public mutating func revealHeld(_ entries: [TranscriptEntry], page: Int = page) -> Bool {
        guard let from else { return false }
        let hidden = entries.filter { $0.createdAt < from }
        guard !hidden.isEmpty else { return false }
        self.from = hidden.count > page ? hidden[hidden.count - page].createdAt
            : (cursor == nil ? nil : hidden[0].createdAt)
        return true
    }

    /// The store's earlier page was read ahead and is held, not shown yet: only where the store goes on changes.
    public mutating func held(oldest: Date?, cursor: String?, more: Bool) {
        self.cursor = more ? cursor : nil
        if from == nil, let oldest { from = oldest }
    }

    /// The store's page arrived: its oldest entry is the new edge (`more`: the store has earlier ones).
    public mutating func received(oldest: Date?, cursor: String?, more: Bool) {
        loading = false
        self.cursor = more ? cursor : nil
        from = more ? oldest ?? from : nil
    }
}
