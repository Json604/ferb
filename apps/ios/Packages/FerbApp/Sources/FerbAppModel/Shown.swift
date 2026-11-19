import FerbCore
import FerbDesign
import FerbMobile
import Foundation

// FERB's words and answers as FERB.app draws them (kb PHN-720..; Android: model/Shown.kt): the phone library lays them
// out (its `richText`, `answerShown`, `noticeShown`), this turns its records into FerbDesign's pieces with dates in the
// phone's own format. The library is called off the main actor (PhoneApi.transcript prepares every row) and each
// result is kept per entry and text, so a redraw, a streamed word or a live line never parses again.

/// An answer ready to draw.
public struct AnswerUi: Equatable, Sendable {
    public var bubble: Bool
    public var blocks: [FerbDesign.RichBlock]
    public var copy: String
    public var note: String?
    public var lines: [String]
    public var details: [FerbDesign.RichBlock]
    public var sites: [String]
    public var above: [ResultShown]
    public var below: [ResultShown]
    public var pages: [String]
    public var files: [CardLine]
    public var statusWords: String?
    /// Code's "Not done:" reasons, drawn small under the words with the note (kb APP-978); not in `blocks`.
    public var gaps: [String] = []
}

/// The "Not done:" lines code writes under a delivered-with-gaps answer (ferb-supervisor `with_gap_lines`; FERB.app's
/// GapLines, TurnCard.swift; Android's GapLines): after the answer and a blank line, one line per reason. Only that block
/// is recognised: a paragraph every line of which starts with "Not done: " and has a reason, after the answer's first
/// paragraph (or the whole text when the answer was empty). The same words inside the answer's own paragraphs stay words
/// (kb APP-978).
public struct GapLines: Equatable, Sendable {
    public static let label = "Not done:"
    public let body: String
    public let reasons: [String]

    public init(_ text: String) {
        let paragraphs = text.components(separatedBy: "\n\n")
        let lead = Self.label + " "
        var kept: [String] = [], reasons: [String] = []
        for (i, paragraph) in paragraphs.enumerated() {
            let lines = paragraph.components(separatedBy: "\n")
            let block = (i > 0 || paragraphs.count == 1)
                && lines.allSatisfy { $0.hasPrefix(lead) && !$0.dropFirst(lead.count).allSatisfy(\.isWhitespace) }
            if block { reasons += lines.map { String($0.dropFirst(lead.count)) } } else { kept.append(paragraph) }
        }
        body = reasons.isEmpty ? text : kept.joined(separator: "\n\n")
        self.reasons = reasons
    }
}

/// A notice's layout (FERB.app's NoticeView).
public struct NoticeUi: Equatable, Sendable {
    public var boxSays: Bool
    public var above: [ResultShown]
    public var notes: [ResultShown]
    public var pages: [String]
    static let none = NoticeUi(boxSays: false, above: [], notes: [], pages: [])
}

public enum Present {
    private static let lock = NSLock()
    nonisolated(unsafe) private static var kept: [String: Any] = [:]
    nonisolated(unsafe) private static var order: [String] = []
    private static let keep = 4000

    private static func kept<T>(_ key: String, _ make: () -> T) -> T {
        lock.lock()
        if let v = kept[key] as? T { lock.unlock(); return v }
        lock.unlock()
        let v = make()
        lock.lock()
        if kept[key] == nil { order.append(key) }
        kept[key] = v
        if order.count > keep { kept[order.removeFirst()] = nil }
        lock.unlock()
        return v
    }

    /// FERB's Markdown as FerbDesign's blocks.
    public static func rich(_ text: String) -> [FerbDesign.RichBlock] {
        if let p = plainOnly(text) { return p }
        return kept("r\u{1}" + text) {
            MainGuard.offMain("rich_text")
            return FerbMobile.richText(source: text).map(block)
        }
    }

    /// A reply as it streams: read each time (only its newest words matter).
    public static func richNow(_ text: String) -> [FerbDesign.RichBlock] {
        if let p = plainOnly(text) { return p }
        MainGuard.offMain("rich_text")
        return FerbMobile.richText(source: text).map(block)
    }

    /// Words with nothing Markdown or a link could start: the library's answer made here without a call across
    /// (Android's Present.plainOnly; the tests hold the two equal).
    public static func plainOnly(_ text: String) -> [FerbDesign.RichBlock]? {
        if text.count > 4000 || text.contains(where: { "*_`[]<>#|~\\!@&".contains($0) }) || text.contains("://") || text.contains("www.") { return nil }
        var out: [FerbDesign.RichBlock] = []
        var run: [String] = []
        for line in text.replacingOccurrences(of: "\r\n", with: "\n").split(separator: "\n", omittingEmptySubsequences: false).map(String.init) {
            if line.allSatisfy(\.isWhitespace) {
                if !run.isEmpty { out.append(.paragraph([RichRun(run.joined(separator: "\n"))])); run = [] }
                continue
            }
            if let f = line.first, f == "-" || f == "+" || f.isNumber || f.isWhitespace { return nil }
            run.append(line)
        }
        if !run.isEmpty { out.append(.paragraph([RichRun(run.joined(separator: "\n"))])) }
        return out
    }

    /// An answer as FERB.app draws it; `card`: its turn has a card of parts; `hidden`: boxes a later item draws.
    public static func answer(_ a: AnswerView, card: Bool, hidden: [String], taskId: String?) -> AnswerUi {
        kept("a\u{1}\(a.hashValue)\u{1}\(a.text.count)\u{1}\(card)\u{1}\(hidden)\u{1}\(taskId ?? "")") {
            MainGuard.offMain("answer_shown")
            // code's "Not done:" lines leave the words and are drawn small under them (kb APP-978)
            let full = GapLines(a.text)
            let gaps = a.displayText.map { GapLines($0).reasons }.flatMap { $0.isEmpty ? nil : $0 } ?? full.reasons
            var words = a
            if !gaps.isEmpty {
                words.text = full.body
                words.displayText = a.displayText.map { GapLines($0).body }
            }
            let s = FerbMobile.answerShown(answer: words, card: card, hidden: hidden)
            return AnswerUi(
                bubble: s.bubble || !gaps.isEmpty, blocks: s.blocks.map(block), copy: s.copy, note: s.note, lines: s.lines,
                details: s.details.map(block), sites: s.sites,
                above: s.above.compactMap { shown($0, words: nil) }, below: s.below.compactMap { shown($0, words: nil) },
                pages: s.pages.map(page),
                files: s.files.map { f in
                    CardLine(title: ChatModel.fileName(f.path), detail: ChatModel.folderOf(f.path), file: true,
                             taskId: f.index != nil ? taskId : nil, index: f.index.map(Int.init))
                },
                statusWords: s.statusWords, gaps: gaps)
        }
    }

    /// A notice's cards as FERB.app lays them out.
    public static func notice(_ text: String, quotes: Int, cards: [ResultCard], hidden: [String]) -> NoticeUi {
        if cards.isEmpty { return .none }
        return kept("n\u{1}\(text)\u{1}\(quotes)\u{1}\(cards.hashValue)\u{1}\(hidden)") {
            MainGuard.offMain("notice_shown")
            let s = FerbMobile.noticeShown(text: text, quotes: UInt32(quotes), cards: cards, hidden: hidden)
            return NoticeUi(boxSays: s.boxSays, above: s.above.compactMap { shown($0, words: s.boxWords) },
                            notes: s.notes.compactMap { shown($0, words: nil) }, pages: s.pages.map(page))
        }
    }

    /// Receipt cards under a first reply, by kind.
    public static func cards(_ cards: [ResultCard], hidden: [String]) -> [ResultShown] {
        cards.filter { box($0).map { !hidden.contains($0) } ?? true }
            .filter { ["reminder", "event", "note", "payment", "send", "schedules"].contains($0.kindOf) }
            .compactMap { shown($0, words: nil) }
    }

    private static func box(_ c: ResultCard) -> String? {
        switch c.kindOf {
        case "event": return ["timer", "reminder", "schedule"].contains(c.kind ?? "") ? c.id : nil
        case "reminder": return c.scheduleId ?? c.repeatOf ?? c.timerId
        default: return nil
        }
    }

    private static func page(_ c: ResultCard) -> String { c.line ?? (c.page == "memory" ? "Open the Memory page" : "Open Phone settings") }

    /// The page a site row opens (FERB.app's VisitedSites.link): https:// and a plain host only.
    public static func siteLink(_ host: String) -> URL? {
        guard !host.isEmpty, host.allSatisfy({ $0.isLetter || $0.isNumber || ".-:[]".contains($0) }) else { return nil }
        return URL(string: "https://" + host)
    }

    private static func runs(_ s: [RichSpan]) -> [RichRun] { s.map { RichRun($0.text, bold: $0.bold, italic: $0.italic, code: $0.code, strike: $0.strike, link: $0.link) } }

    private static func block(_ b: FerbMobile.RichBlock) -> FerbDesign.RichBlock {
        switch b {
        case .paragraph(let s): return .paragraph(runs(s))
        case .heading(let level, let s): return .heading(Int(level), runs(s))
        case .listItem(let marker, let level, let s): return .item(marker: marker, level: Int(level), runs(s))
        case .quote(let s): return .quote(runs(s))
        case .code(let text, let language): return .code(text, language: language)
        case .table(let header, let rows, _): return .table(header: header.map { runs($0.spans) }, rows: rows.map { $0.cells.map { runs($0.spans) } })
        case .divider: return .divider
        }
    }

    // MARK: the cards' words (FERB.app's ResultCards.swift)

    public static func time(_ text: String?) -> Date? {
        guard let text else { return nil }
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let d = f.date(from: text) { return d }
        f.formatOptions = [.withInternetDateTime]
        return f.date(from: text)
    }

    /// CardTime.day: Today, Tomorrow, a weekday within the week, else a short date.
    public static func day(_ date: Date, now: Date = Date(), calendar: Calendar = .current) -> String {
        let days = calendar.dateComponents([.day], from: calendar.startOfDay(for: now), to: calendar.startOfDay(for: date)).day ?? 0
        if days == 0 { return "Today" }
        if days == 1 { return "Tomorrow" }
        if days > 1 && days < 7 { return date.formatted(.dateTime.weekday(.wide)) }
        return date.formatted(.dateTime.weekday(.abbreviated).month(.abbreviated).day())
    }

    public static func clock(_ date: Date) -> String { date.formatted(date: .omitted, time: .shortened) }

    private static func cap(_ s: String) -> String { s.prefix(1).capitalized + s.dropFirst() }

    private static func countdown(_ c: ResultCard) -> CountdownShown? {
        guard c.kindOf == "reminder" || c.kindOf == "event" else { return nil }
        if let end = time(c.until) {
            let parts = Calendar.current.dateComponents([.hour, .minute, .second], from: end)
            let caption = parts.hour == 0 && parts.minute == 0 && parts.second == 0
                ? "until it ends, last day " + day(end.addingTimeInterval(-1)) : "until it ends, " + day(end) + " " + clock(end)
            return CountdownShown(at: end, caption: caption, dueCaption: "ended")
        }
        if c.kindOf == "reminder", let next = time(c.nextRun) {
            return CountdownShown(at: next, caption: "until the next one, " + day(next) + " " + clock(next), dueCaption: "next run due")
        }
        return nil
    }

    public static func shown(_ c: ResultCard, words: String?) -> ResultShown? {
        switch c.kindOf {
        case "reminder":
            return .reminder(timer: c.kind == "timer", title: c.title ?? "Scheduled reminder", words: words,
                             due: time(c.due).map { "Due " + day($0) + ", " + clock($0) }, repeats: c.repeatRule != nil, countdown: countdown(c))
        case "event":
            let timer = c.kind == "timer"
            let symbol = timer ? "timer" : c.kind == "reminder" ? "alarm" : "calendar"
            let meta = timer ? "Timer set" : c.kind == "reminder" ? "Reminder set" : c.changed ? "Schedule changed" : "Scheduled"
            var changes: [String] = []
            if c.changed {
                if let was = c.wasRepeat { changes += ["Was: " + was, "Now: " + (c.repeatRule ?? "")] }
                if let was = c.wasRequest { changes += ["Instructions were: " + was, "Instructions now: " + (c.request ?? "")] }
            }
            let rule = c.repeatRule.map { cap($0) + (c.watch.map { ", " + $0 } ?? "") } ?? c.watch.map(cap)
            let w = time(c.when)
            return .event(meta: meta, symbol: symbol, what: c.what ?? "", day: w.map { day($0) }, time: w.map(clock), repeats: c.repeatRule != nil,
                          rule: rule, changes: changes, countdown: countdown(c))
        case "note":
            let meta = ["proposed": "Saved in my words · waits for your Keep", "corrected": "Remembered · corrected", "forgotten": "Forgotten"][c.state ?? ""] ?? "Remembered"
            return .note(meta: meta, text: c.text ?? "", forgotten: c.state == "forgotten")
        case "payment":
            return .payment(title: c.merchant ?? c.site ?? "Payment", site: c.site == c.merchant ? nil : c.site, savedCard: c.savedCard, amount: c.amount)
        case "send":
            let place = c.app ?? c.site ?? ""
            let verb = ["call": "Called", "mailbox_send": "Emailed", "post": "Posted", "reply": "Replied"][c.kind ?? ""] ?? "Sent"
            let symbol = ["call": "phone.fill", "mailbox_send": "envelope.fill", "post": "square.and.pencil", "reply": "arrowshape.turn.up.left.fill"][c.kind ?? ""] ?? "paperplane.fill"
            return .send(initials: initials(c.recipient ?? ""), title: c.recipient ?? place, verb: verb + (c.recipient == nil ? "" : " · " + place),
                         symbol: symbol, account: c.account)
        case "schedules":
            let rows = c.schedules ?? []
            return .schedules(meta: rows.isEmpty ? "No schedules" : rows.count == 1 ? "1 schedule" : "\(rows.count) schedules", rows: rows.map { r in
                let next = time(r.nextRun)
                return ScheduleShown(name: r.name, rule: cap(r.repeatRule) + (r.watch.map { ", " + $0 } ?? ""), state: r.state == "active" ? nil : r.state,
                                     day: next.map { day($0) }, time: next.map(clock))
            })
        default:
            return nil   // a kind this build doesn't draw: its line stays in FERB's words, as on the Mac
        }
    }

    /// FERB.app's ResultCard.initials.
    public static func initials(_ name: String) -> String {
        let person = name.split(separator: "@").first.map(String.init) ?? name
        let letters = person.split(whereSeparator: { !$0.isLetter && !$0.isNumber }).prefix(2).compactMap { $0.first.map { String($0).uppercased() } }.joined()
        return letters.isEmpty ? "?" : letters
    }
}
