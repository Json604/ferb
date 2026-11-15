import Foundation

// Settings → Stored data (owner STORE-422: "i wanna see the db too … i wann see all teh data", "delete in db by user if
// they want"; agent STORE-423, STORE-424, STORE-425): what the daemon's `data.*` calls answer, read for the page. The
// daemon serves what it stored (redacted at capture); nothing here reads the Keychain or a sealed value.

/// A group of tables as a person reads them (Conversation, Tasks and results, What the AI was given, Memory, Activity
/// log, Safety records, Everything else).
public struct StoredGroup: Equatable, Identifiable {
    public let id: String
    public let title: String
}

/// One table of FERB's database.
public struct StoredTable: Equatable, Identifiable {
    public let name: String
    public let group: String
    public let rows: Int
    public let columns: [String]
    /// SQLite's own or a full-text index's: hidden unless asked for.
    public let isInternal: Bool
    /// What a row can have done to it here: "turn" (delete the turn), "forget" (forget the memory), or nil.
    public let action: String?
    public let note: String
    public var id: String { name }
}

/// `data.tables`.
public struct StoredTables: Equatable {
    public let groups: [StoredGroup]
    public let tables: [StoredTable]

    public init?(_ reply: [String: Any]) {
        guard let groups = reply["groups"] as? [[String: Any]], let tables = reply["tables"] as? [[String: Any]] else {
            return nil
        }
        self.groups = groups.compactMap { g in
            guard let id = g["id"] as? String else { return nil }
            return StoredGroup(id: id, title: g["title"] as? String ?? id)
        }
        self.tables = tables.compactMap { t in
            guard let name = t["name"] as? String else { return nil }
            return StoredTable(name: name, group: t["group"] as? String ?? "other", rows: StoredData.int(t["rows"]) ?? 0,
                               columns: t["columns"] as? [String] ?? [], isInternal: t["internal"] as? Bool ?? false,
                               action: t["action"] as? String, note: t["note"] as? String ?? "")
        }
    }

    /// The tables of one group, the internal ones only when asked for.
    public func tables(in group: String, internal showInternal: Bool) -> [StoredTable] {
        tables.filter { $0.group == group && (showInternal || !$0.isInternal) }
    }

    /// How many internal tables the list hides.
    public var hiddenCount: Int { tables.filter(\.isInternal).count }
}

/// One cell as stored.
public enum StoredCell: Equatable {
    case null
    case number(String)
    case text(String)
    case bytes(Int)

    init(_ value: Any?) {
        switch value {
        case nil, is NSNull: self = .null
        case let s as String: self = .text(s)
        case let n as NSNumber: self = .number(n.stringValue)
        case let o as [String: Any]: self = .bytes(StoredData.int(o["bytes"]) ?? 0)
        default: self = .text(String(describing: value!))
        }
    }

    /// One line for a list: text with its line breaks folded.
    public var short: String {
        switch self {
        case .null: return "—"
        case let .number(n): return n
        case let .text(t): return t.replacingOccurrences(of: "\n", with: " ")
        case let .bytes(n): return "\(n) bytes"
        }
    }

    public var text: String? { if case let .text(t) = self { return t } else { return nil } }
}

/// What a row can have done to it: `turn` deletes the turn its request id names; `forget` forgets the memory.
public struct StoredAction: Equatable {
    public let kind: String
    public let id: String

    init?(_ value: Any?) {
        guard let o = value as? [String: Any], let kind = o["do"] as? String, let id = o["id"] as? String else { return nil }
        self.kind = kind
        self.id = id
    }

    public var title: String { kind == "forget" ? "Forget this memory…" : "Delete this turn…" }
}

/// A row in a page or opened in full: its cells, the cut ones' full lengths, and what can be done to it.
public struct StoredRow: Equatable, Identifiable {
    public let rowid: Int?
    public var cells: [StoredCell]
    /// column → full length in characters, for the cells shown cut.
    public var cut: [String: Int]
    public let action: StoredAction?
    public let index: Int
    public var id: String { rowid.map(String.init) ?? "at-\(index)" }

    init(_ value: [String: Any], index: Int) {
        rowid = StoredData.int(value["rowid"])
        cells = (value["cells"] as? [Any] ?? []).map { StoredCell($0) }
        cut = (value["cut"] as? [String: Any] ?? [:]).compactMapValues { StoredData.int($0) }
        action = StoredAction(value["action"])
        self.index = index
    }
}

/// `data.rows`: a page, newest first.
public struct StoredPage: Equatable {
    public let table: String
    public let columns: [String]
    public var rows: [StoredRow]
    public let total: Int
    public var next: Int?
    public let note: String

    public init?(_ reply: [String: Any]) {
        guard let table = reply["table"] as? String, let rows = reply["rows"] as? [[String: Any]] else { return nil }
        self.table = table
        columns = reply["columns"] as? [String] ?? []
        self.rows = rows.enumerated().map { StoredRow($1, index: $0) }
        total = StoredData.int(reply["total"]) ?? rows.count
        next = StoredData.int(reply["next"])
        note = reply["note"] as? String ?? ""
    }

    /// The next page added below (`next` from it).
    public mutating func append(_ more: StoredPage) {
        let start = rows.count
        rows += more.rows.map { row in
            StoredRow(rowid: row.rowid, cells: row.cells, cut: row.cut, action: row.action, index: start + row.index)
        }
        next = more.next
    }

    /// The few cells a list line shows: the first text-like ones.
    public func summary(_ row: StoredRow) -> String {
        row.cells.prefix(4).map(\.short).joined(separator: "  ·  ")
    }
}

extension StoredRow {
    init(rowid: Int?, cells: [StoredCell], cut: [String: Int], action: StoredAction?, index: Int) {
        self.rowid = rowid
        self.cells = cells
        self.cut = cut
        self.action = action
        self.index = index
    }
}

/// `data.row`: one row in full.
public struct StoredFullRow: Equatable {
    public let table: String
    public let rowid: Int
    public let columns: [String]
    public var cells: [StoredCell]
    public var cut: [String: Int]
    public let action: StoredAction?
    public let note: String

    public init?(_ reply: [String: Any]) {
        guard let table = reply["table"] as? String, let rowid = StoredData.int(reply["rowid"]) else { return nil }
        self.table = table
        self.rowid = rowid
        columns = reply["columns"] as? [String] ?? []
        cells = (reply["cells"] as? [Any] ?? []).map { StoredCell($0) }
        cut = (reply["cut"] as? [String: Any] ?? [:]).compactMapValues { StoredData.int($0) }
        action = StoredAction(reply["action"])
        note = reply["note"] as? String ?? ""
    }

    /// The text read so far of a cut column, and how much is left.
    public func shown(_ column: String) -> (text: String, left: Int)? {
        guard let at = columns.firstIndex(of: column), let text = cells[at].text else { return nil }
        let full = cut[column] ?? text.count
        return (text, max(0, full - text.count))
    }

    /// A `data.cell` piece added to its column; the cut mark goes once the whole text is here.
    public mutating func add(piece reply: [String: Any]) {
        guard let column = reply["column"] as? String, let at = columns.firstIndex(of: column),
              let text = reply["text"] as? String, let offset = StoredData.int(reply["offset"]),
              let now = cells[at].text, offset == now.count else { return }
        cells[at] = .text(now + text)
        if reply["end"] as? Bool == true { cut[column] = nil }
    }
}

/// One match of `data.search`.
public struct StoredHit: Equatable, Identifiable {
    public let table: String
    public let group: String
    public let rowid: Int?
    public let column: String
    public let snippet: String
    public let index: Int
    public var id: String { "\(table)/\(rowid ?? -1)/\(column)/\(index)" }
}

/// `data.search`.
public struct StoredSearch: Equatable {
    public let text: String
    public let hits: [StoredHit]
    /// The search spent its time before reading every table.
    public let stopped: Bool

    public init?(_ reply: [String: Any]) {
        guard let hits = reply["hits"] as? [[String: Any]] else { return nil }
        text = reply["text"] as? String ?? ""
        self.hits = hits.enumerated().compactMap { index, h in
            guard let table = h["table"] as? String else { return nil }
            return StoredHit(table: table, group: h["group"] as? String ?? "other", rowid: StoredData.int(h["rowid"]),
                             column: h["column"] as? String ?? "", snippet: h["snippet"] as? String ?? "", index: index)
        }
        stopped = reply["stopped"] as? Bool ?? false
    }
}

/// `data.context`: the conversation the planner would read now, block by block.
public struct StoredContext: Equatable {
    public struct Block: Equatable, Identifiable {
        public let text: String
        public let origins: [String]
        public let index: Int
        public var id: Int { index }
    }
    public let conversation: String
    public let blocks: [Block]

    public init?(_ reply: [String: Any]) {
        guard let blocks = reply["blocks"] as? [[String: Any]] else { return nil }
        conversation = reply["conversation"] as? String ?? ""
        self.blocks = blocks.enumerated().map { index, b in
            Block(text: b["text"] as? String ?? "", origins: b["origins"] as? [String] ?? [], index: index)
        }
    }
}

/// `data.delete`'s preview or result, in plain lines.
public struct StoredDeletion: Equatable {
    public let requestID: String
    public let confirmed: Bool
    public let removes: [String]
    public let keeps: [String]
    /// What FERB learned only from this turn, in its own words: forgotten with it (STORE-426).
    public let forgets: [String]
    /// What FERB learned from this turn and elsewhere too: kept.
    public let keepsLearned: [String]

    public init?(_ reply: [String: Any]) {
        guard let request = reply["request_id"] as? String else { return nil }
        requestID = request
        confirmed = reply["confirmed"] as? Bool ?? false
        let gone = (reply[confirmed ? "deleted" : "will_delete"] as? [String: Any]) ?? [:]
        func n(_ key: String) -> Int { StoredData.int(gone[key]) ?? (gone[key] as? [Any])?.count ?? 0 }
        func plural(_ count: Int, _ one: String, _ many: String) -> String { "\(count) \(count == 1 ? one : many)" }
        var removes: [String] = []
        let rows = n("conversation_rows") + n("chat_history") + n("chat_entries")
        if rows > 0 { removes.append("the message and FERB's replies (\(rows) records)") }
        if n("tasks") > 0 { removes.append(plural(n("tasks"), "task with its results", "tasks with their results")) }
        if n("traces") > 0 { removes.append(plural(n("traces"), "trace record", "trace records")) }
        if n("screenshots") > 0 { removes.append(plural(n("screenshots"), "screenshot", "screenshots")) }
        if n("episodes") > 0 { removes.append(plural(n("episodes"), "record of how the task went", "records of how the tasks went")) }
        if n("summaries") > 0 { removes.append(plural(n("summaries"), "running summary of the chat", "running summaries of the chat") + " (FERB writes them again)") }
        if n("memories") > 0 { removes.append(plural(n("memories"), "memory FERB learned from it", "memories FERB learned from it")) }
        if n("learned") > 0 { removes.append(plural(n("learned"), "learned card built from it", "learned cards built from it") + " and what FERB derived from them") }
        if n("later_quotes_blanked") > 0 { removes.append("its words in \(plural(n("later_quotes_blanked"), "later trace record", "later trace records"))") }
        self.removes = removes
        func texts(_ value: Any?) -> [String] {
            (value as? [[String: Any]] ?? []).compactMap { ($0["text"] as? String).flatMap { $0.isEmpty ? nil : $0 } }
        }
        forgets = texts(gone["memories"]) + texts(gone["learned"])
        keepsLearned = texts(reply["keeps_memories"]) + texts(reply["keeps_learned"])
        let kept = reply["keeps"] as? [String: Any] ?? [:]
        func k(_ key: String) -> Int { StoredData.int(kept[key]) ?? 0 }
        var keeps: [String] = []
        if k("audit_log") > 0 { keeps.append("\(k("audit_log")) activity-log record(s) of what happened") }
        let safety = k("action_ledger") + k("confirmations") + k("standing_permissions") + k("recovery")
        if safety > 0 { keeps.append("\(safety) safety record(s): actions, your answers, permissions, recovery copies") }
        if StoredData.int(gone["screenshots_kept"]) ?? 0 > 0 {
            keeps.append("\(StoredData.int(gone["screenshots_kept"]) ?? 0) screenshot(s) FERB couldn't remove safely (they age out)")
        }
        let shared = k("memories_shared") + k("learned_shared")
        if shared > 0 { keeps.append(plural(shared, "memory", "memories") + " FERB also learned elsewhere") }
        self.keeps = keeps
    }

    /// The confirmation's words: what goes (the memories in their own words), what stays.
    public var question: String {
        var out = "Deleted from FERB's database: " + removes.joined(separator: "; ") + ". FERB won't read it again."
        if !forgets.isEmpty { out += "\n\nForgotten with it:\n" + forgets.map { "• " + $0 }.joined(separator: "\n") }
        if !keeps.isEmpty { out += "\n\nKept: " + keeps.joined(separator: "; ") + "." }
        if !keepsLearned.isEmpty {
            out += "\n\nKept, because FERB learned them elsewhere too:\n" + keepsLearned.map { "• " + $0 }.joined(separator: "\n")
        }
        return out + "\n\nThis can't be undone."
    }
}

public enum StoredData {
    /// The line a fresh conversation leaves in the chat (STORE-427), the same words the engine's entry carries.
    public static let freshMarker = "New conversation: FERB doesn't read the messages above."
    static func int(_ value: Any?) -> Int? {
        switch value {
        case let n as Int: return n
        case let n as NSNumber: return n.intValue
        default: return nil
        }
    }

    /// JSON text indented for reading; anything else as it is.
    public static func pretty(_ text: String) -> String {
        let trimmed = text.trimmingCharacters(in: .whitespaces)
        guard trimmed.first == "{" || trimmed.first == "[", let data = text.data(using: .utf8),
              let value = try? JSONSerialization.jsonObject(with: data, options: [.fragmentsAllowed]),
              let out = try? JSONSerialization.data(withJSONObject: value,
                                                    options: [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]),
              let pretty = String(data: out, encoding: .utf8) else { return text }
        return pretty
    }
}

extension StoredData {
    /// A stored value as a person reads it: JSON indented, and each long text with line breaks inside it (a model
    /// call's `context`, an answer) taken out and shown as written below the rest.
    public static func readable(_ text: String) -> String {
        guard let data = text.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return pretty(text) }
        var rest = object
        var long: [(String, String)] = []
        for (key, value) in object {
            if let s = value as? String, s.contains("\n") {
                long.append((key, s))
                rest[key] = "(shown below)"
            }
        }
        guard !long.isEmpty, let restData = try? JSONSerialization.data(withJSONObject: rest),
              let restText = String(data: restData, encoding: .utf8) else { return pretty(text) }
        return ([pretty(restText)] + long.sorted { $0.0 < $1.0 }.map { "── \($0.0) ──\n\($0.1)" })
            .joined(separator: "\n\n")
    }
}

extension ConversationArchive {
    /// The owner deleted these turns (`data.delete`): their turns and the replies to them leave the chat this app holds,
    /// so nothing shows or writes them back. Returns how many entries left.
    @discardableResult
    public mutating func removeTurns(requestIDs: Set<String>) -> Int {
        let gone = Set(turns.filter { $0.requestID.map(requestIDs.contains) ?? false }.map(\.id))
        guard !gone.isEmpty else { return 0 }
        let before = turns.count + replies.count
        turns.removeAll { gone.contains($0.id) }
        replies.removeAll { gone.contains($0.turnID) }
        return before - turns.count - replies.count
    }
}
