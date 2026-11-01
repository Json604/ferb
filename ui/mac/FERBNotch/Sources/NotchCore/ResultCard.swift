import Foundation

/// Typed cards (step 8e, owner 2026-10-31; looks from Shapeshift, MIT, rebuilt natively): the daemon attaches `cards`
/// next to the text of a reply, a first reply, an outcome or a scheduled run's result, each built from FERB's own
/// records (a timer, a schedule row, a memory item, a tool call's facts, the sites list, the artifacts). Which card shows
/// is the daemon's `type`; nothing here reads the text to choose one. The text stays for history, voice and older apps:
/// a card that stands for one of code's lines names that line (`line`), and the notch leaves exactly that line out.
public struct ResultCard: Codable, Equatable {
    /// reminder, event, note, payment, send, link or file. Another type (a newer daemon's) isn't drawn.
    public let type: String
    /// The line of the text this card stands for, exactly as the text carries it.
    public var line: String?
    // reminder: a schedule's run came due
    public var title: String?
    public var due: String?
    public var repeatRule: String?
    public var nextRun: String?
    public var scheduleID: String?
    /// reminder and event: the schedule's recorded end (ORCH-320); the countdown is to it when present.
    public var until: String?
    // event: a reminder or schedule just set (kind reminder | schedule)
    public var kind: String?
    public var id: String?
    public var what: String?
    public var when: String?
    public var watch: String?
    // note: a remembered item (state saved | proposed; corrected | forgotten after the owner acts on it here)
    public var itemID: String?
    public var text: String?
    public var state: String?
    // payment: the page's amount and merchant, as code read them
    public var amount: String?
    public var merchant: String?
    public var site: String?
    /// payment: the name of the owner's saved card entry that filled the checkout (ORCH-321); never digits.
    public var savedCard: String?
    // send: the call's checked destination (kind send, reply, post, mailbox_send, call)
    public var recipient: String?
    public var app: String?
    public var account: String?
    // link
    public var host: String?
    // file
    public var path: String?
    public var name: String?
    // page (step 12c): a FERB window page to open, with its filter (the memory import's notice: Memory, imported_from)
    public var page: String?
    public var importedFrom: String?

    public static let types: Set<String> = ["reminder", "event", "note", "payment", "send", "link", "file", "page"]

    public init?(_ value: Any?) {
        guard let value = value as? [String: Any], let type = value["type"] as? String, Self.types.contains(type) else {
            return nil
        }
        func words(_ key: String) -> String? { (value[key] as? String).flatMap { $0.isEmpty ? nil : $0 } }
        self.type = type
        line = words("line")
        title = words("title"); due = words("due"); repeatRule = words("repeat"); nextRun = words("next_run")
        scheduleID = words("schedule_id"); until = words("until")
        kind = words("kind"); id = words("id"); what = words("what"); when = words("when"); watch = words("watch")
        itemID = words("item_id"); text = words("text"); state = words("state")
        amount = words("amount"); merchant = words("merchant"); site = words("site"); savedCard = words("card")
        recipient = words("recipient"); app = words("app"); account = words("account")
        host = words("host"); path = words("path"); name = words("name")
        page = words("page")
        importedFrom = ((value["filter"] as? [String: Any])?["imported_from"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        // A card with nothing to show isn't one.
        switch type {
        case "note": if itemID == nil || text == nil { return nil }
        case "link": if host == nil { return nil }
        case "file": if path == nil { return nil }
        case "send": if recipient == nil && site == nil && app == nil { return nil }
        case "payment": if amount == nil && merchant == nil { return nil }
        case "event": if what == nil { return nil }
        case "page": if page == nil { return nil }
        default: break
        }
    }

    /// The cards of an event's `cards` field, in the daemon's order; nil when it has none (an older daemon).
    public static func list(_ value: Any?) -> [ResultCard]? {
        guard let items = value as? [Any] else { return nil }
        let cards = items.compactMap { ResultCard($0) }
        return cards.isEmpty ? nil : cards
    }

    public var dueDate: Date? { LiveProgress.time(due) }
    public var nextRunDate: Date? { LiveProgress.time(nextRun) }
    public var whenDate: Date? { LiveProgress.time(when) }
    public var untilDate: Date? { LiveProgress.time(until) }

    /// What a reminder or event card counts down to: the schedule's end when it has one (ORCH-320), else a
    /// reminder's next run; nothing for an event without an end (its first time shows as chips).
    public var countdown: CountdownTarget? {
        guard type == "reminder" || type == "event" else { return nil }
        if let end = untilDate { return .end(end) }
        if type == "reminder", let next = nextRunDate { return .nextRun(next) }
        return nil
    }
    public var fileURL: URL? { path.map { URL(fileURLWithPath: $0) } }

    /// The letter on a link's tile: the host without "www.", its first character.
    public var monogram: String {
        let shown = (host ?? "").hasPrefix("www.") ? String((host ?? "").dropFirst(4)) : (host ?? "")
        return shown.first.map { String($0).uppercased() } ?? "·"
    }

    /// The recipient's initials for a contact card: the first letters of its first two words (an address's name part).
    public var initials: String { Self.initials(recipient ?? "") }

    public static func initials(_ name: String) -> String {
        let person = name.split(separator: "@").first.map(String.init) ?? name
        let words = person.split(whereSeparator: { !$0.isLetter && !$0.isNumber }).prefix(2)
        let letters = words.compactMap { $0.first.map { String($0).uppercased() } }.joined()
        return letters.isEmpty ? "?" : letters
    }
}

/// How the notch places a turn's cards, and the text left when the lines they stand for are left out.
public enum CardLayout {
    /// Cards drawn above FERB's text (a scheduled run's reminder: the text goes under it).
    public static func above(_ cards: [ResultCard]) -> [ResultCard] { cards.filter { $0.type == "reminder" } }
    /// Cards drawn one per surface under the text, in the daemon's order.
    public static func below(_ cards: [ResultCard]) -> [ResultCard] {
        cards.filter { ["event", "note", "payment", "send"].contains($0.type) }
    }
    public static func links(_ cards: [ResultCard]) -> [ResultCard] { cards.filter { $0.type == "link" } }
    public static func files(_ cards: [ResultCard]) -> [ResultCard] { cards.filter { $0.type == "file" } }
    /// Buttons that open a page of FERB's window (step 12c).
    public static func pages(_ cards: [ResultCard]) -> [ResultCard] { cards.filter { $0.type == "page" } }

    /// The lines the cards stand for.
    public static func lines(_ cards: [ResultCard]) -> Set<String> { Set(cards.compactMap(\.line)) }

    /// `text` without the lines the cards stand for (whole lines only, compared exactly); blank runs they leave
    /// collapse. Nothing else in the text is read or changed.
    public static func strip(_ text: String, _ cards: [ResultCard]) -> String {
        let drop = lines(cards)
        guard !drop.isEmpty else { return text }
        let kept = text.components(separatedBy: "\n").filter { !drop.contains($0) }
        guard kept.count < text.components(separatedBy: "\n").count else { return text }
        var out: [String] = []
        for line in kept where !(line.trimmingCharacters(in: .whitespaces).isEmpty
                                 && (out.last?.trimmingCharacters(in: .whitespaces).isEmpty ?? true)) {
            out.append(line)
        }
        return out.joined(separator: "\n").trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// Code's lines the cards don't stand for.
    public static func remaining(_ lines: [String], _ cards: [ResultCard]) -> [String] {
        let drop = Self.lines(cards)
        return lines.filter { !drop.contains($0) }
    }
}

/// The time a card counts down to, and which one it is.
public enum CountdownTarget: Equatable {
    case end(Date)
    case nextRun(Date)

    public var date: Date {
        switch self {
        case .end(let date), .nextRun(let date): return date
        }
    }

    /// The day an end at local midnight closes ("until Nov 3" ends as Nov 4 begins: shown as the end of Nov 3); nil
    /// for an end at another time of day, which shows as itself.
    public static func lastDay(of end: Date, calendar: Calendar = .current) -> Date? {
        let parts = calendar.dateComponents([.hour, .minute, .second], from: end)
        guard parts.hour == 0, parts.minute == 0, parts.second == 0 else { return nil }
        return calendar.date(byAdding: .second, value: -1, to: end)
    }
}

/// A countdown in FERB's short words: "in 2h 59m", "in 3d 4h", "in 12m", "in 40s"; "now" once it is due.
public enum Countdown {
    public static func text(to date: Date, now: Date) -> String {
        let seconds = Int(date.timeIntervalSince(now).rounded(.down))
        if seconds <= 0 { return "now" }
        if seconds < 60 { return "in \(seconds)s" }
        let minutes = seconds / 60
        if minutes < 60 { return "in \(minutes)m" }
        let hours = minutes / 60
        if hours < 24 { return "in \(hours)h \(minutes % 60)m" }
        return "in \(hours / 24)d \(hours % 24)h"
    }
}
