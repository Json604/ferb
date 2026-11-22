import Foundation

/// The memory import's live card (MEM-455, MEM-456), in the daemon's own words: drawn at the end of today's chat and at
/// the top of the Memory page from the moment an import FERB tells the owner about starts until its one notice comes in.
/// Live state only (the bus's `memory.import.live` `card`, `memory.import.status`'s `live`): never a chat entry, never
/// saved. Both phones draw the same card from ferb-mobile's `ImportCard`.
public struct MemoryImportCard: Equatable {
    public struct Row: Equatable, Identifiable {
        /// Stable while the row lives ("run:<id>", "site:<source>"): the bar animates within one row.
        public let key: String
        public let source: String
        public let name: String
        /// reading, placing, asking, waiting_mail, checking_mail.
        public let stage: String
        public let line: String
        public let detail: String
        public let done: Int
        public let total: Int
        /// "33 of 40", beside the bar.
        public let count: String
        public let foundLine: String
        /// A gap, or a try that didn't go through, with what happens next.
        public let note: String

        public var id: String { key }

        /// How full the bar is (0 to 1); nil: no bar (nothing planned yet, or a website's waiting stage).
        public var fraction: Double? { total > 0 ? Double(min(max(done, 0), total)) / Double(total) : nil }

        public init(key: String, source: String = "", name: String = "", stage: String = "", line: String,
                    detail: String = "", done: Int = 0, total: Int = 0, count: String = "", foundLine: String = "",
                    note: String = "") {
            self.key = key; self.source = source; self.name = name; self.stage = stage; self.line = line
            self.detail = detail; self.done = done; self.total = total; self.count = count
            self.foundLine = foundLine; self.note = note
        }
    }

    public let title: String
    public let rows: [Row]

    /// Each row says its source's name only when the card has more than one.
    public var namesRows: Bool { rows.count > 1 }

    public init(title: String, rows: [Row]) {
        self.title = title
        self.rows = rows
    }

    /// From the daemon's card; nil for anything that isn't one with a row (the card left: `null`).
    public init?(_ value: Any?) {
        guard let card = value as? [String: Any] else { return nil }
        let number = { (v: Any?) -> Int in (v as? NSNumber)?.intValue ?? 0 }
        let rows: [Row] = (card["rows"] as? [[String: Any]] ?? []).compactMap { r in
            guard let key = r["key"] as? String, !key.isEmpty else { return nil }
            return Row(key: key, source: said(r["source"]), name: said(r["name"]), stage: said(r["stage"]),
                       line: said(r["line"]), detail: said(r["detail"]), done: number(r["done"]),
                       total: number(r["total"]), count: said(r["count"]), foundLine: said(r["found_line"]),
                       note: said(r["note"]))
        }
        guard !rows.isEmpty else { return nil }
        self.init(title: said(card["title"]), rows: rows)
    }
}

/// The Memory page's line for each source's last import (`memory.import.status` `runs`, newest first): one per source.
public struct MemoryImportLastRun: Equatable, Identifiable {
    public let source: String
    public let name: String
    public let line: String

    public var id: String { source }

    /// The newest run of each source, in the order they were last run.
    public static func lines(_ runs: [[String: Any]], calendar: Calendar = .current, now: Date = Date())
        -> [MemoryImportLastRun] {
        var seen = Set<String>(), out: [MemoryImportLastRun] = []
        for run in runs {
            let source = said(run["source"])
            guard !source.isEmpty, seen.insert(source).inserted else { continue }
            let name = source == "codex" ? "Codex" : said(run["name"])
            let counts = run["counts"] as? [String: Any] ?? [:]
            let profile = counts["profile"] as? [String: Any] ?? [:]
            let number = { (v: Any?) -> Int in (v as? NSNumber)?.intValue ?? 0 }
            let learned = profile.isEmpty ? nil
                : number(profile["new lines"]) + number(profile["lines reworded by a newer fact"])
            let at = date(said(run["finished_at"]).isEmpty ? said(run["started_at"]) : said(run["finished_at"]))
            let when = at.map { spoken($0, calendar: calendar, now: now) } ?? ""
            let line: String
            switch said(run["status"]) {
            case "running": line = "Bringing it in now"
            case "done":
                if number(counts["failed_chunks"]) > 0 && (learned ?? 0) == 0 {
                    line = "Last read \(when). Part of it couldn't be read; I'll go on from there next time."
                } else if let learned {
                    line = "Last read \(when): " + (learned == 0 ? "nothing new" :
                        "\(learned) new thing\(learned == 1 ? "" : "s") learned")
                } else {
                    line = "Last read \(when)"
                }
            case "cancelled": line = "Stopped \(when). Start it again to go on from there."
            default: line = "Didn't finish \(when). I'll go on from where I stopped next time."
            }
            out.append(MemoryImportLastRun(source: source, name: name, line: line))
        }
        return out
    }

    private static func date(_ iso: String) -> Date? {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let d = f.date(from: iso) { return d }
        f.formatOptions = [.withInternetDateTime]
        return f.date(from: iso)
    }

    /// "today at 23:54", "yesterday at 08:05", "on 3 Nov".
    static func spoken(_ date: Date, calendar: Calendar, now: Date) -> String {
        let time = DateFormatter()
        time.calendar = calendar
        time.timeZone = calendar.timeZone
        time.locale = Locale(identifier: "en_GB")
        time.dateFormat = "HH:mm"
        if calendar.isDate(date, inSameDayAs: now) { return "today at \(time.string(from: date))" }
        if let y = calendar.date(byAdding: .day, value: -1, to: now), calendar.isDate(date, inSameDayAs: y) {
            return "yesterday at \(time.string(from: date))"
        }
        time.dateFormat = "d MMM"
        return "on \(time.string(from: date))"
    }
}

/// The daemon's value as text ("" for none).
private func said(_ value: Any?) -> String {
    switch value {
    case let s as String: return s
    case let n as NSNumber: return n.stringValue
    default: return ""
    }
}

/// The owner's two switches for the memory import (MEM-459): the daemon's `memory.import.switch` `which`, the setting
/// it keeps, and their words.
public enum MemoryImportSwitch: String, CaseIterable, Identifiable {
    case local, sites

    public var id: String { rawValue }

    public var setting: String { self == .local ? "memory.local_import" : "memory.sites_import" }

    public var title: String {
        switch self {
        case .local: return "Bring in what your coding assistants know"
        case .sites: return "Bring in what ChatGPT and Claude know"
        }
    }

    public var detail: String {
        switch self {
        case .local:
            return "FERB reads what you've typed into Claude Code and Codex on this Mac over the last month, and keeps "
                + "up with it every hour. Turn it off and FERB reads nothing more; what it already learned stays on the "
                + "Memory page."
        case .sites:
            return "FERB asks each for a copy of your data in your browser, waits for their email, reads your last "
                + "month of chats and then deletes the file. Turn it off and FERB asks for nothing and downloads "
                + "nothing; what it already learned stays on the Memory page."
        }
    }
}
