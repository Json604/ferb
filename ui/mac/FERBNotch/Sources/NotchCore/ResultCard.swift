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
    // reminder: a schedule's run, a reminder or a timer came due. `title` is its short name (T-1055), never the
    // instruction; `kind` (schedule | reminder | timer) says which, so a timer's card says it's a timer.
    public var title: String?
    public var due: String?
    public var repeatRule: String?
    public var nextRun: String?
    public var scheduleID: String?
    /// reminder and event: the schedule's recorded end (ORCH-320); the countdown is to it when present.
    public var until: String?
    // event: a reminder, timer or schedule just set (kind reminder | timer | schedule); `what` is its short name
    public var kind: String?
    public var id: String?
    public var what: String?
    public var when: String?
    public var watch: String?
    /// A timer's card (event or reminder): when it was set (APP-762), so its ring drains over its duration.
    public var started: String?
    /// A reminder card's timer (`timer_id`), else an event card's `id`: what Cancel and Repeat act on.
    public var timerID: String?
    /// A timer's delivery card: the first timer of its Repeat chain (`repeat_of`, DMN-549), whose box it is.
    public var repeatOf: String?
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
    // event (T-1066): a schedule changed in place: its timing and instructions before (`was_*`) and its instructions now
    public var changed: Bool = false
    public var wasRepeat: String?
    public var wasRequest: String?
    public var request: String?
    // schedules (T-1066): the owner's schedules as FERB's records hold them, drawn by the daemon's code
    public var count: Int?
    public var schedules: [ScheduleListRow]?

    public static let types: Set<String> = ["reminder", "event", "note", "payment", "send", "link", "file", "page",
                                            "schedules"]

    private enum CodingKeys: String, CodingKey {
        case type, line, title, due, repeatRule, nextRun, scheduleID, until, kind, id, what, when, watch, started
        case timerID, repeatOf, itemID, text, state, amount, merchant, site, savedCard, recipient, app, account, host
        case path, name, page, importedFrom, changed, wasRepeat, wasRequest, request, count, schedules
    }

    /// Read tolerantly (APP-801): only `type` is required; a key saved before it existed reads as a new card's value
    /// (2026-11-08: `changed` without it failed every older card, and so the whole chat), and a schedule row that
    /// can't be read is left out. A stored property added here gets a key above and a line here.
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        func words(_ key: CodingKeys) throws -> String? { try c.decodeIfPresent(String.self, forKey: key) }
        type = try c.decode(String.self, forKey: .type)
        line = try words(.line)
        title = try words(.title); due = try words(.due); repeatRule = try words(.repeatRule)
        nextRun = try words(.nextRun); scheduleID = try words(.scheduleID); until = try words(.until)
        kind = try words(.kind); id = try words(.id); what = try words(.what); when = try words(.when)
        watch = try words(.watch); started = try words(.started); timerID = try words(.timerID)
        repeatOf = try words(.repeatOf); itemID = try words(.itemID); text = try words(.text); state = try words(.state)
        amount = try words(.amount); merchant = try words(.merchant); site = try words(.site)
        savedCard = try words(.savedCard); recipient = try words(.recipient); app = try words(.app)
        account = try words(.account); host = try words(.host); path = try words(.path); name = try words(.name)
        page = try words(.page); importedFrom = try words(.importedFrom)
        changed = try c.value(.changed, or: false)
        wasRepeat = try words(.wasRepeat); wasRequest = try words(.wasRequest); request = try words(.request)
        count = try c.decodeIfPresent(Int.self, forKey: .count)
        schedules = try c.lossyList(ScheduleListRow.self, .schedules)
    }

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
        started = words("started"); timerID = words("timer_id") ?? words("id"); repeatOf = words("repeat_of")
        itemID = words("item_id"); text = words("text"); state = words("state")
        amount = words("amount"); merchant = words("merchant"); site = words("site"); savedCard = words("card")
        recipient = words("recipient"); app = words("app"); account = words("account")
        host = words("host"); path = words("path"); name = words("name")
        page = words("page")
        changed = value["changed"] as? Bool ?? false
        wasRepeat = words("was_repeat"); wasRequest = words("was_request"); request = words("request")
        count = value["count"] as? Int
        schedules = (value["schedules"] as? [Any]).map { $0.compactMap { ScheduleListRow($0) } }
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
        case "schedules": if schedules == nil { return nil }
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
    public var startedDate: Date? { LiveProgress.time(started) }
    /// When a timer ends: an event card's `when`, a reminder card's `due`.
    public var timerEnds: Date? { type == "event" ? whenDate : dueDate }

    /// A reminder or event card for a timer (its row's kind, T-1055).
    public var isTimer: Bool { kind == "timer" }

    /// The box this card is (APP-769, owner 2026-11-06: one box per timer, reminder and schedule): the id of the
    /// timer, reminder or schedule it was set as (an event card's `id`) or delivers (a reminder card's `schedule_id`,
    /// else the first timer of its Repeat chain, else its `timer_id`). Nil for every other card.
    public var box: String? {
        switch type {
        case "event": return ["timer", "reminder", "schedule"].contains(kind ?? "") ? id : nil
        case "reminder": return scheduleID ?? repeatOf ?? timerID
        default: return nil
        }
    }

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

/// One of the owner's schedules on a schedules card (T-1066): its name, the timing FERB's code wrote from its saved
/// rule, its state and its next run (none while paused).
public struct ScheduleListRow: Codable, Equatable {
    public let id: String
    public let name: String
    public let repeatRule: String
    public let state: String
    public let nextRun: String?
    public let until: String?
    public let watch: String?

    public init?(_ value: Any?) {
        guard let value = value as? [String: Any], let id = value["id"] as? String, let name = value["name"] as? String,
              let rule = value["repeat"] as? String else { return nil }
        self.id = id; self.name = name; repeatRule = rule
        state = value["state"] as? String ?? "active"
        nextRun = value["next_run"] as? String; until = value["until"] as? String; watch = value["watch"] as? String
    }

    public var nextRunDate: Date? { LiveProgress.time(nextRun) }

    private enum CodingKeys: String, CodingKey { case id, name, repeatRule, state, nextRun, until, watch }

    /// Read tolerantly (APP-801): its id, name and timing words are required; a missing state reads as active.
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        name = try c.decode(String.self, forKey: .name)
        repeatRule = try c.decode(String.self, forKey: .repeatRule)
        state = try c.value(.state, or: "active")
        nextRun = try c.decodeIfPresent(String.self, forKey: .nextRun)
        until = try c.decodeIfPresent(String.self, forKey: .until)
        watch = try c.decodeIfPresent(String.self, forKey: .watch)
    }
}

/// How the notch places a turn's cards, and the text left when the lines they stand for are left out.
public enum CardLayout {
    /// Cards drawn above FERB's text (a scheduled run's reminder: the text goes under it).
    public static func above(_ cards: [ResultCard]) -> [ResultCard] { cards.filter { $0.type == "reminder" } }
    /// Cards drawn one per surface under the text, in the daemon's order.
    public static func below(_ cards: [ResultCard]) -> [ResultCard] {
        cards.filter { ["event", "note", "payment", "send", "schedules"].contains($0.type) }
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

    /// What FERB's bubble beside a turn's cards says: the text without the lines the cards stand for, or nil when
    /// nothing is left and no note or line goes under it. A timer's reply is only its "Timer set" line, which its card
    /// says, so it draws no empty bubble (owner 2026-11-06: "i get these emptyu bbubbles eveytime on a timer request").
    public static func bubble(_ text: String, _ cards: [ResultCard], note: String?, lines: [String]) -> String? {
        let shown = strip(text, cards)
        let empty = shown.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        return empty && (note ?? "").trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && lines.isEmpty
            ? nil : shown
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
