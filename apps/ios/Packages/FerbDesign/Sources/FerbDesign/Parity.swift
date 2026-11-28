// What FERB.app shows, drawn in the phone's design (kb PHN-720..PHN-729), the twin of Android's :design Rich.kt,
// Results.kt and Work.kt: FERB's words in rich text that folds as FERB.app's does, the sites row, each result card by
// its kind, the work row above the message box and a task's short view. Nothing here parses: the blocks come ready from
// the phone library (ferb-mobile `rich`, `shown`), read off the main actor and kept per entry.

import SwiftUI
import UIKit

// MARK: Rich text

/// A run of words with one look; `link` an address to open (http, https or mailto: the library makes no other).
public struct RichRun: Equatable, Sendable {
    public var text: String
    public var bold = false, italic = false, code = false, strike = false
    public var link: String?
    public init(_ text: String, bold: Bool = false, italic: Bool = false, code: Bool = false, strike: Bool = false, link: String? = nil) {
        self.text = text; self.bold = bold; self.italic = italic; self.code = code; self.strike = strike; self.link = link
    }
}

/// One block of FERB's words (FERB.app's MarkdownDocument.Block).
public enum RichBlock: Equatable, Sendable {
    case paragraph([RichRun])
    case heading(Int, [RichRun])
    case item(marker: String, level: Int, [RichRun])
    case quote([RichRun])
    case code(String, language: String)
    case table(header: [[RichRun]], rows: [[[RichRun]]])
    case divider

    public static func plain(_ text: String) -> [RichBlock] { [.paragraph([RichRun(text)])] }
}

/// How the words look on their fill.
public struct RichLook: Sendable {
    public var size: CGFloat, ink: Color, dim: Color, link: Color, codeFill: Color
    public static let onBlack = RichLook(size: 16, ink: .white, dim: Ink.onDark, link: Color(hex: 0x9CC7FF), codeFill: Ink.darkFill)
    public static let onWhite = RichLook(size: 15, ink: .black, dim: Ink.detail, link: Color(hex: 0x1F6FEB), codeFill: Ink.fill)
}

/// The blocks as one attributed text, so selection and Copy run across the whole message (APP-637).
public func richText(_ blocks: [RichBlock], _ look: RichLook, tail: [(String, Bool)] = []) -> AttributedString {
    var out = AttributedString()
    func runs(_ rs: [RichRun], size: CGFloat, bold: Bool = false, color: Color? = nil) -> AttributedString {
        var a = AttributedString()
        for r in rs {
            var p = AttributedString(r.text)
            var font = r.code ? Font.system(size: size * 0.92, design: .monospaced) : Font.system(size: size, weight: (r.bold || bold) ? .semibold : .regular)
            if r.italic { font = font.italic() }
            p.font = font
            p.foregroundColor = color ?? look.ink
            if r.code { p.backgroundColor = look.codeFill }
            if r.strike { p.strikethroughStyle = .single }
            if let link = r.link, let url = URL(string: link) {
                p.link = url
                p.foregroundColor = look.link
                p.underlineStyle = .single
            }
            a += p
        }
        return a
    }
    func plain(_ s: String, size: CGFloat, color: Color, bold: Bool = false) -> AttributedString {
        var p = AttributedString(s)
        p.font = .system(size: size, weight: bold ? .semibold : .regular)
        p.foregroundColor = color
        return p
    }
    var first = true
    for b in blocks {
        if !first { out += plain("\n\n", size: look.size * 0.5, color: look.ink) }
        first = false
        switch b {
        case .paragraph(let r): out += runs(r, size: look.size)
        case .heading(let level, let r):
            out += runs(r, size: look.size * (level == 1 ? 17 / 12 : level == 2 ? 15 / 12 : 13 / 12), bold: true)
        case .item(let marker, let level, let r):
            out += plain(String(repeating: "    ", count: level) + marker + "  ", size: look.size, color: look.dim)
            out += runs(r, size: look.size)
        case .quote(let r): out += plain("▎ ", size: look.size, color: look.dim) + runs(r, size: look.size, color: look.dim)
        case .code(let text, let language):
            if !language.isEmpty { out += plain(language + "\n", size: look.size * 0.75, color: look.dim) }
            var c = AttributedString(text)
            c.font = .system(size: look.size * 11 / 12, design: .monospaced)
            c.foregroundColor = look.ink
            c.backgroundColor = look.codeFill
            out += c
        case .table(let header, let rows):
            for (i, row) in ([header] + rows).enumerated() {
                if i > 0 { out += plain("\n", size: look.size, color: look.ink) }
                for (j, cell) in row.enumerated() {
                    if j > 0 { out += plain("  ·  ", size: look.size, color: look.dim) }
                    out += runs(cell, size: look.size, bold: i == 0)
                }
            }
        case .divider: out += plain("────────────", size: look.size, color: look.dim)
        }
    }
    // FERB.app's annotations (MarkdownContent.swift:36-47): the note at 11.5 and code's lines at 11 under its 12, grey,
    // "Couldn't confirm:" amber and bold (kb PHN-773)
    for (line, label) in tail {
        if !first { out += plain("\n\n", size: look.size * 0.5, color: look.ink) }
        first = false
        if label, let at = line.firstIndex(of: ":") {
            let head = String(line[...at]), rest = String(line[line.index(after: at)...])
            var h = plain(head, size: look.size * 11.5 / 12, color: Ink.amber, bold: true)
            h.foregroundColor = Ink.amber
            out += h + plain(rest, size: look.size * 11.5 / 12, color: look.dim)
        } else {
            out += plain(line, size: look.size * 11 / 12, color: look.dim)
        }
    }
    return out
}

/// The fold FERB.app's CollapsePolicy applies to a text.
public enum Fold: Sendable { case none, answer, row }

/// FERB.app's CollapsePolicy (CardRowState.swift:192-216): the lines a text folds to, or nil when it shows whole.
public enum FoldRule {
    public static func lines(_ fold: Fold, total: Int, lineHeight: CGFloat, viewport: CGFloat) -> Int? {
        guard lineHeight > 0, total > 0 else { return nil }
        let shown: Int
        switch fold {
        case .none: return nil
        case .row: guard total > 8 else { return nil }; shown = 3
        case .answer:
            guard viewport > 0, CGFloat(total) * lineHeight > viewport else { return nil }
            shown = max(1, Int((viewport / 2 / lineHeight).rounded(.down)))
        }
        return total - shown >= 3 ? shown : nil
    }
}

/// The chat list's tallest height (FERB.app folds answers to the chat's height, there the height the owner set).
private struct ChatViewportKey: EnvironmentKey { static let defaultValue: CGFloat = 0 }
/// Whether cards draw live state (today's chat); the earlier days are a record (APP-769).
private struct CardsLiveKey: EnvironmentKey { static let defaultValue = true }
/// A fixed clock for countdowns and the work row's time (snapshots); nil: the phone's.
private struct CardClockKey: EnvironmentKey { static let defaultValue: Date? = nil }
/// Snapshots: every folded text drawn opened, as after "Show all".
private struct FoldsOpenKey: EnvironmentKey { static let defaultValue = false }
public extension EnvironmentValues {
    var chatViewport: CGFloat { get { self[ChatViewportKey.self] } set { self[ChatViewportKey.self] = newValue } }
    var cardsLive: Bool { get { self[CardsLiveKey.self] } set { self[CardsLiveKey.self] = newValue } }
    var cardClock: Date? { get { self[CardClockKey.self] } set { self[CardClockKey.self] = newValue } }
    var foldsOpen: Bool { get { self[FoldsOpenKey.self] } set { self[FoldsOpenKey.self] = newValue } }
}

/// FERB's words folded as FERB.app folds them (`fold` answer: past the chat's height to about half; row: past 8 lines
/// to 3), with "Show all"/"Show less"; a long press copies. Whether he opened it is kept by `key` while the app runs.
public struct RichWords: View {
    let text: AttributedString
    let look: RichLook
    let key: String
    var fold: Fold = .none
    var more = "Show all", less = "Show less"
    var whenFolded: AttributedString? = nil
    var width: CGFloat = 288
    @Environment(\.chatViewport) private var viewport
    @Environment(\.foldsOpen) private var foldsOpen
    @State private var opened = false
    private var open: Bool { opened || foldsOpen }

    public init(_ text: AttributedString, look: RichLook, key: String, fold: Fold = .none, more: String = "Show all",
                less: String = "Show less", whenFolded: AttributedString? = nil, width: CGFloat = 288) {
        self.text = text; self.look = look; self.key = key; self.fold = fold; self.more = more; self.less = less
        self.whenFolded = whenFolded; self.width = width
    }

    private var lineHeight: CGFloat { UIFont.systemFont(ofSize: look.size).lineHeight + 2 }

    public var body: some View {
        let shown = fold == .none || (fold == .answer && viewport <= 0) ? nil
            : FoldRule.lines(fold, total: Int((RichMeasure.height(text, size: look.size, width: width) / lineHeight).rounded()), lineHeight: lineHeight, viewport: viewport)
        VStack(alignment: .leading, spacing: 6) {
            Text(text).lineSpacing(2).lineLimit(shown != nil && !open ? shown : nil)
                .textSelection(.enabled)
            if shown != nil, !open, let whenFolded { Text(whenFolded).lineSpacing(2) }
            if shown != nil {
                Button { opened.toggle() } label: {
                    Text(open ? less : more).font(.system(size: look.size - 1, weight: .semibold)).foregroundStyle(look.link)
                        .padding(.vertical, 6)
                }.buttonStyle(.plain)
            }
        }
    }
}

/// The height of a text's words at a width, in the bubble's type (an estimate of the drawn text's own: what decides
/// FERB.app's fold), kept per text so a redraw never measures again.
enum RichMeasure {
    nonisolated(unsafe) private static let kept = NSCache<NSString, NSNumber>()

    static func height(_ text: AttributedString, size: CGFloat, width: CGFloat) -> CGFloat {
        let words = String(text.characters)
        let key = "\(size)|\(width)|\(words.hashValue)|\(words.count)" as NSString
        if let h = kept.object(forKey: key) { return CGFloat(h.doubleValue) }
        let style = NSMutableParagraphStyle()
        style.lineSpacing = 2
        let h = NSAttributedString(string: words, attributes: [.font: UIFont.systemFont(ofSize: size), .paragraphStyle: style])
            .boundingRect(with: CGSize(width: width, height: .greatestFiniteMagnitude), options: [.usesLineFragmentOrigin, .usesFontLeading], context: nil).height
        kept.setObject(NSNumber(value: Double(h)), forKey: key)
        return h
    }
}

/// FERB's black bubble around rich words (FERB.app's FerbBubble).
public struct FerbRichBubble<Content: View>: View {
    let content: Content
    var copy = ""
    public init(copy: String = "", @ViewBuilder content: () -> Content) { self.copy = copy; self.content = content() }
    public var body: some View {
        VStack(alignment: .leading, spacing: 8) { content }
            .padding(.horizontal, 16).padding(.vertical, 11)
            .background(Ink.black, in: UnevenRoundedRectangle(topLeadingRadius: 22, bottomLeadingRadius: 6, bottomTrailingRadius: 22, topTrailingRadius: 22, style: .continuous))
            .frame(maxWidth: 320, alignment: .leading)
            .frame(maxWidth: .infinity, alignment: .leading)
    }
}

// MARK: The sites row (APP-700)

/// The globe, the first site as a link, "+N" opening the list of all of them.
public struct SitesChip: View {
    let sites: [String]
    var onOpen: (String) -> Void
    var onAll: () -> Void
    public init(sites: [String], onOpen: @escaping (String) -> Void, onAll: @escaping () -> Void) { self.sites = sites; self.onOpen = onOpen; self.onAll = onAll }
    public var body: some View {
        if let first = sites.first {
            HStack(spacing: 7) {
                Image(systemName: "globe").font(.system(size: 14)).foregroundStyle(.white.opacity(0.7))
                Button { onOpen(first) } label: {
                    Text(first).font(.system(size: 14, weight: .medium)).underline().foregroundStyle(RichLook.onBlack.link).lineLimit(1)
                }.buttonStyle(.plain).accessibilityLabel("Open \(first)")
                if sites.count > 1 {
                    Button(action: onAll) {
                        Text("+\(sites.count - 1)").font(.system(size: 13, weight: .semibold)).monospacedDigit().foregroundStyle(.white)
                            .padding(.horizontal, 7).padding(.vertical, 2)
                            .background(.white.opacity(0.14), in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                    }.buttonStyle(.plain).accessibilityLabel("Show all \(sites.count) visited sites")
                }
            }
            .padding(.horizontal, 12).padding(.vertical, 7)
            .background(.black.opacity(0.55), in: Capsule())
            .contentShape(Capsule())
            .onTapGesture { sites.count > 1 ? onAll() : onOpen(first) }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

// MARK: Result cards (FERB.app ResultCards.swift)

/// What a card counts down to: its time, and what it says before and once it is due.
public struct CountdownShown: Equatable, Sendable {
    public var at: Date, caption: String, dueCaption: String
    public init(at: Date, caption: String, dueCaption: String) { self.at = at; self.caption = caption; self.dueCaption = dueCaption }
}

/// FERB.app's Countdown.text without "in ": "2h 59m", "3d 4h", "12m", "40s"; "Now" once due.
public func countdownText(to at: Date, now: Date) -> String {
    let s = Int(at.timeIntervalSince(now).rounded(.down))
    if s <= 0 { return "Now" }
    if s < 60 { return "\(s)s" }
    let m = s / 60
    if m < 60 { return "\(m)m" }
    let h = m / 60
    if h < 24 { return "\(h)h \(m % 60)m" }
    return "\(h / 24)d \(h % 24)h"
}

public struct ScheduleShown: Equatable, Sendable {
    public var name: String, rule: String, state: String?, day: String?, time: String?
    public init(name: String, rule: String, state: String?, day: String?, time: String?) { self.name = name; self.rule = rule; self.state = state; self.day = day; self.time = time }
}

/// One result card's words, as the app read them from the card (FERB.app's views by `type`).
public enum ResultShown: Equatable, Sendable {
    case reminder(timer: Bool, title: String, words: String?, due: String?, repeats: Bool, countdown: CountdownShown?)
    case event(meta: String, symbol: String, what: String, day: String?, time: String?, repeats: Bool, rule: String?, changes: [String], countdown: CountdownShown?)
    case note(meta: String, text: String, forgotten: Bool)
    case payment(title: String, site: String?, savedCard: String?, amount: String?)
    case send(initials: String, title: String, verb: String, symbol: String, account: String?)
    case schedules(meta: String, rows: [ScheduleShown])
}

/// One card by its kind, on the phone's white card.
public struct ResultCardView: View {
    let card: ResultShown
    @Environment(\.cardsLive) private var live
    @Environment(\.cardClock) private var clock
    @Environment(\.onFerb) private var onFerb
    private var ink: CardInk { CardInk(onFerb) }
    public init(_ card: ResultShown) { self.card = card }

    public var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            switch card {
            case let .reminder(timer, title, words, due, repeats, countdown):
                meta(timer ? "timer" : "alarm", timer ? "Timer" : "Reminder")
                Text(title).type(17, .semibold, ink.primary)
                if let words { Text(words).type(15, .regular, ink.primary).lineSpacing(2) }
                chips { if let due { chip("clock", due) }; if repeats { repeatsBadge } }
                if let countdown, live { countdownView(countdown) }
            case let .event(m, symbol, what, day, time, repeats, rule, changes, countdown):
                meta(symbol, m)
                Text(what).type(17, .semibold, ink.primary)
                ForEach(changes, id: \.self) { Text($0).type(13.5, .regular, ink.secondary).lineLimit(3) }
                chips { if let day { chip("calendar", day) }; if let time { chip("clock", time) }; if repeats { repeatsBadge } }
                if let rule { Text(rule).type(13.5, .regular, ink.secondary) }
                if let countdown, live { countdownView(countdown) }
            case let .note(m, text, forgotten):
                meta("brain", m)
                Text(text).type(15.5, .regular, forgotten ? ink.secondary : ink.primary).strikethrough(forgotten).lineSpacing(2)
            case let .payment(title, site, savedCard, amount):
                HStack(spacing: 12) {
                    Image(systemName: "creditcard.fill").font(.system(size: 18)).foregroundStyle(ink.primary).frame(width: 40, height: 40)
                        .background(ink.fill, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                    VStack(alignment: .leading, spacing: 6) {
                        Text(title).type(17, .semibold, ink.primary).lineLimit(2)
                        chips { chip("checkmark.seal", "You approved"); if let site { chip("globe", site) }; if let savedCard { chip("creditcard", savedCard) } }
                    }
                    Spacer(minLength: 8)
                    Text(amount ?? "Amount not read").type(amount == nil ? 13.5 : 22, .semibold, amount == nil ? ink.secondary : ink.primary).lineLimit(1).monospacedDigit()
                }
            case let .send(initials, title, verb, symbol, account):
                HStack(alignment: .top, spacing: 12) {
                    Text(initials).type(15, .semibold, ink.primary).frame(width: 40, height: 40).background(ink.fill, in: Circle())
                    VStack(alignment: .leading, spacing: 4) {
                        Text(title).type(17, .semibold, ink.primary).lineLimit(2)
                        HStack(spacing: 6) { Image(systemName: symbol).font(.system(size: 12)).foregroundStyle(ink.secondary); Text(verb).type(14, .regular, ink.secondary).lineLimit(1) }
                        if let account { Text("From " + account).type(13.5, .regular, ink.faint) }
                    }
                }
            case let .schedules(m, rows):
                meta("calendar", m)
                ForEach(Array(rows.enumerated()), id: \.offset) { _, r in
                    VStack(alignment: .leading, spacing: 3) {
                        Text(r.name).type(16, .semibold, ink.primary)
                        Text(r.rule).type(13.5, .regular, ink.secondary)
                        chips { if let s = r.state { chip(nil, s) }; if let d = r.day { chip("calendar", d) }; if let t = r.time { chip("clock", t) } }
                    }
                }
            }
        }
        .padding(.horizontal, 16).padding(.vertical, 14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(ink.card, in: RoundedRectangle(cornerRadius: 22, style: .continuous))
        .accessibilityElement(children: .combine)
    }

    private func meta(_ symbol: String, _ text: String) -> some View {
        Label { Text(text).type(13, .medium, ink.secondary) } icon: { Image(systemName: symbol).font(.system(size: 12)).foregroundStyle(ink.secondary) }
    }

    private func chips<C: View>(@ViewBuilder _ c: () -> C) -> some View { ChipFlow(spacing: 6) { c() } }

    private func chip(_ symbol: String?, _ text: String) -> some View {
        HStack(spacing: 5) {
            if let symbol { Image(systemName: symbol).font(.system(size: 12)).foregroundStyle(ink.secondary) }
            Text(text).type(13.5, .medium, ink.primary).lineLimit(1)
        }
        .padding(.horizontal, 10).padding(.vertical, 5).background(ink.fill, in: Capsule())
    }

    private var repeatsBadge: some View {
        HStack(spacing: 4) { Image(systemName: "repeat").font(.system(size: 11, weight: .semibold)); Text("Repeats").font(.system(size: 12.5, weight: .semibold)) }
            .foregroundStyle(ink.card).padding(.horizontal, 9).padding(.vertical, 4).background(ink.primary, in: Capsule())
    }

    private func countdownView(_ c: CountdownShown) -> some View {
        TimelineView(.periodic(from: .now, by: 30)) { ctx in
            let now = clock ?? ctx.date
            HStack(alignment: .lastTextBaseline, spacing: 8) {
                Text(countdownText(to: c.at, now: now)).type(24, .semibold, ink.primary).monospacedDigit()
                Text(c.at <= now ? c.dueCaption : c.caption).type(13.5, .medium, ink.secondary)
            }
        }
    }
}

// MARK: The work row and a task's short view (FERB.app ChatBody.swift WorkRowView, TaskActivityBubble.swift)

/// One running task as the work row shows it; `picture` its live picture while one is shown (in memory only).
public struct WorkTask: Equatable, Identifiable {
    public var id: String, title: String, line: String, waiting: Bool, paused: Bool
    public var picture: UIImage?
    public init(id: String, title: String, line: String, waiting: Bool = false, paused: Bool = false, picture: UIImage? = nil) {
        self.id = id; self.title = title; self.line = line; self.waiting = waiting; self.paused = paused; self.picture = picture
    }
}

/// The row's words either side of its time (FERB.app's WorkRowWords).
public struct WorkWords: Equatable, Sendable {
    public var lead: String, trail: String
    public static func of(active: Int, paused: Int, kept: Int) -> WorkWords {
        func tasks(_ n: Int) -> String { n == 1 ? "task" : "tasks" }
        var rest: [String] = []
        if paused > 0 { rest.append(active > 0 ? "\(paused) paused" : "\(paused) paused \(tasks(paused))") }
        if kept > 0 { rest.append("\(kept) kept going") }
        return active > 0 ? WorkWords(lead: "\(active) active \(tasks(active))", trail: rest.joined(separator: " · "))
            : WorkWords(lead: rest.joined(separator: " · "), trail: "")
    }
}

/// SwiftUI's `.timer` words: "0:42", "12:05", "1:02:03".
public func elapsedText(_ seconds: Int) -> String {
    let s = max(0, seconds), h = s / 3600, m = (s % 3600) / 60, r = s % 60
    return h > 0 ? String(format: "%d:%02d:%02d", h, m, r) : String(format: "%d:%02d", m, r)
}

/// The work row (FERB.app's WorkRowView): the running mark (a paused glyph when nothing runs), its words and time, the round pictures of the
/// first two tasks with their amber ring and dot, "+N", and "View tasks".
public struct ActiveWorkRow: View {
    let words: WorkWords
    let running: Bool
    let since: Date?
    let tasks: [WorkTask]
    var onPicture: (WorkTask) -> Void
    var onMore: () -> Void
    var onViewTasks: () -> Void
    var viewTasks = "View tasks"
    @Environment(\.cardClock) private var clock

    public init(words: WorkWords, running: Bool, since: Date?, tasks: [WorkTask], viewTasks: String = "View tasks",
                onPicture: @escaping (WorkTask) -> Void, onMore: @escaping () -> Void, onViewTasks: @escaping () -> Void) {
        self.words = words; self.running = running; self.since = since; self.tasks = tasks; self.viewTasks = viewTasks
        self.onPicture = onPicture; self.onMore = onMore; self.onViewTasks = onViewTasks
    }

    public var body: some View {
        HStack(spacing: 8) {
            HStack(spacing: 8) {
                if running { Mark(kind: .running, size: 16).colorInvert() } else { Image(systemName: "pause.circle").foregroundStyle(.white.opacity(0.7)) }
                TimelineView(.periodic(from: .now, by: 1)) { ctx in
                    let time = (running ? since : nil).map { elapsedText(Int((clock ?? ctx.date).timeIntervalSince($0))) }
                    Text([words.lead, time, words.trail.isEmpty ? nil : words.trail].compactMap { $0 }.joined(separator: " · "))
                        .font(.system(size: 14, weight: .medium)).monospacedDigit().foregroundStyle(.white.opacity(0.7)).lineLimit(1)
                }
                .layoutPriority(-1)
                ForEach(tasks.prefix(2)) { t in
                    Button { onPicture(t) } label: { RoundPicture(task: t, size: 30) }.buttonStyle(.plain)
                        .accessibilityLabel("Live preview of \(t.title)")
                }
                if tasks.count > 2 {
                    Button(action: onMore) {
                        Text("+\(tasks.count - 2)").font(.system(size: 13, weight: .semibold)).foregroundStyle(.white)
                            .frame(width: 30, height: 30).background(.black.opacity(0.55), in: Circle())
                    }.buttonStyle(.plain)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            Button(action: onViewTasks) { Text(viewTasks).font(.system(size: 14, weight: .semibold)).foregroundStyle(.white).fixedSize() }
                .buttonStyle(.plain).frame(minHeight: 40)
        }
        .frame(minHeight: 40)
    }
}

/// A task's round live picture with its amber ring and dot.
public struct RoundPicture: View {
    let task: WorkTask
    let size: CGFloat
    public init(task: WorkTask, size: CGFloat) { self.task = task; self.size = size }
    public var body: some View {
        ZStack(alignment: .bottomTrailing) {
            Group {
                if let p = task.picture { Image(uiImage: p).resizable().scaledToFill() } else { Ink.darkFill }
            }
            .frame(width: size, height: size).clipShape(Circle())
            .overlay(Circle().stroke(Ink.amber.opacity(0.65), lineWidth: 1))
            Circle().fill(Ink.amber).frame(width: 7, height: 7).offset(x: 1, y: 1)
        }
    }
}

/// A task's short view (FERB.app's TaskShortView): its larger picture (or why there is none), its title, what it is
/// doing now, Watch live and Stop.
public struct TaskShortView: View {
    let task: WorkTask
    var empty: String?
    var onWatch: (() -> Void)?
    var onStop: (() -> Void)?
    /// Draws the larger picture out of screenshots (the app's screenshot shield, as Watch live's frame).
    var shield: (AnyView) -> AnyView = { $0 }
    public init(task: WorkTask, empty: String?, onWatch: (() -> Void)?, onStop: (() -> Void)?, shield: @escaping (AnyView) -> AnyView = { $0 }) {
        self.task = task; self.empty = empty; self.onWatch = onWatch; self.onStop = onStop; self.shield = shield
    }
    public var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            ZStack {
                Ink.darkFill
                if let p = task.picture { shield(AnyView(Image(uiImage: p).resizable().scaledToFit())) }
                else if let empty { Text(empty).type(14.5, .medium, Ink.onDark).multilineTextAlignment(.center).padding(20) }
            }
            .aspectRatio(16 / 9, contentMode: .fit)
            .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
            HStack(spacing: 10) {
                if task.paused { Image(systemName: "pause.circle").font(.system(size: 18)) } else { Mark(kind: .running, size: 18) }
                VStack(alignment: .leading, spacing: 2) {
                    Text(task.title).type(16.5, .semibold).lineLimit(2)
                    Text(task.line).type(14.5, .regular, task.waiting ? CardInk(false).amberText : Ink.detail).lineLimit(2)
                }
            }
            .padding(.horizontal, 4)
            HStack(spacing: 8) {
                Pill(title: "Watch live", kind: .grey, symbol: "eye", height: 46, onClick: onWatch)
                Pill(title: "Stop", kind: .black, symbol: "stop.fill", height: 46, onClick: onStop)
            }
        }
    }
}


/// Chips laid out left to right, wrapping to the next line (FERB.app's FlowRow).
struct ChipFlow: Layout {
    var spacing: CGFloat = 6

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let rows = arrange(subviews, width: proposal.width ?? .infinity)
        return CGSize(width: proposal.width ?? rows.map(\.width).max() ?? 0, height: rows.last.map { $0.y + $0.height } ?? 0)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        for row in arrange(subviews, width: bounds.width) {
            var x = bounds.minX
            for i in row.items {
                let size = subviews[i].sizeThatFits(.unspecified)
                subviews[i].place(at: CGPoint(x: x, y: bounds.minY + row.y), proposal: ProposedViewSize(size))
                x += size.width + spacing
            }
        }
    }

    private struct Row { var items: [Int] = []; var y: CGFloat = 0; var width: CGFloat = 0; var height: CGFloat = 0 }

    private func arrange(_ subviews: Subviews, width: CGFloat) -> [Row] {
        var rows = [Row()]
        for i in subviews.indices {
            let size = subviews[i].sizeThatFits(.unspecified)
            if !rows[rows.count - 1].items.isEmpty && rows[rows.count - 1].width + spacing + size.width > width {
                let last = rows[rows.count - 1]
                rows.append(Row(y: last.y + last.height + spacing))
            }
            rows[rows.count - 1].width += (rows[rows.count - 1].items.isEmpty ? 0 : spacing) + size.width
            rows[rows.count - 1].height = max(rows[rows.count - 1].height, size.height)
            rows[rows.count - 1].items.append(i)
        }
        return rows.filter { !$0.items.isEmpty }
    }
}

// MARK: The card of a request in parts (kb PHN-737; FERB.app's TurnCardView)

/// What colours the card as a whole (FERB.app's surfaceTint): amber while it asks, red when a part failed, blue when updated or paused.
public enum CardTint: Sendable { case none, asking, failed, updated
    var color: Color? {
        switch self {
        case .none: return nil
        case .asking: return Color(hex: 0xFFB020)
        case .failed: return Color(hex: 0xE5281B)
        case .updated: return Color(hex: 0x3B82F6)
        }
    }
}

/// The card as FERB.app draws it (the owner: "see these box cut outs for rows, see hte mac ones … so clean while
/// running adn after too. please make it clean liek mac"): one card on FERB's surface, no fill behind any part, no
/// buttons. A running or waiting part is one line: a thin spinner or a small hollow circle, its label, its state words
/// in the quieter ink (each of the two at most half the line when both are long, cut with "…"), its own picture at the
/// far right. Under a waiting part, the steps it waits on that no part answers are compact lines (FERB.app's
/// ChecklistLineView under a waiting CardRowView, APP-895): their marks under the part's label, a smaller label that
/// keeps the line before its state words do, and the step's own picture while it runs, once per step. A part that ended
/// is its label as a small heading with its answer under it, hairlines between them; the footer is the done line in bold
/// with code's lines after it. A tap on a running part with no picture opens its task's short view (Watch live and Stop
/// are there).
public struct PartsCard: View {
    let headline: String
    let progress: String
    let parts: [PlanPart]
    var tint: CardTint = .none
    var status: String?
    var footer: String?
    var footerLines: [String] = []
    var watching: [Bool] = []
    var pictures: [UIImage?] = []
    var onPicture: ((Int) -> Void)?
    var onPart: ((Int) -> Void)?
    /// Per part, per step under it (`PlanPart.sub`): whether that step shows its own picture, the picture, and its tap.
    var subWatching: [[Bool]] = []
    var subPictures: [[UIImage?]] = []
    var onSubPicture: ((Int, Int) -> Void)?
    var more = "Show more", less = "Show less"
    @Environment(\.onFerb) private var onFerb

    public init(headline: String, progress: String, parts: [PlanPart], tint: CardTint = .none, status: String? = nil, footer: String? = nil,
                footerLines: [String] = [], watching: [Bool] = [], pictures: [UIImage?] = [], onPicture: ((Int) -> Void)? = nil,
                onPart: ((Int) -> Void)? = nil, subWatching: [[Bool]] = [], subPictures: [[UIImage?]] = [],
                onSubPicture: ((Int, Int) -> Void)? = nil, more: String = "Show more", less: String = "Show less") {
        self.headline = headline; self.progress = progress; self.parts = parts; self.tint = tint; self.status = status; self.footer = footer
        self.footerLines = footerLines; self.watching = watching; self.pictures = pictures; self.onPicture = onPicture; self.onPart = onPart
        self.subWatching = subWatching; self.subPictures = subPictures; self.onSubPicture = onSubPicture
        self.more = more; self.less = less
    }

    /// The nested steps' column (FERB.app's feeder lines sit at the row's own text edge): the part's label starts after
    /// its padding, its 14-point mark and the 8 between them.
    static let subIndent: CGFloat = 14 + 14 + 8
    /// FERB.app's RowSpinner draws its 9-point circle with a 1.4 line; the card here is drawn about 1.2 times its size.
    static let spinnerWidth: CGFloat = 1.7

    private static func live(_ k: MarkKind) -> Bool { k == .running || k == .waiting }

    public var body: some View {
        let ink = CardInk(onFerb)
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text(headline).type(13, .semibold, ink.secondary).lineLimit(1)
                Spacer(minLength: 8)
                Text(progress).type(13, .medium, ink.secondary).monospacedDigit()
            }
            .padding(.horizontal, 14).padding(.top, 6).padding(.bottom, 4)
            ForEach(Array(parts.enumerated()), id: \.offset) { i, p in
                if Self.live(p.kind) {
                    let watch = i < watching.count && watching[i]
                    line(p, ink, picture: watch ? .some(i < pictures.count ? pictures[i] : nil) : nil, index: i)
                    if !p.sub.isEmpty {
                        VStack(alignment: .leading, spacing: 0) {
                            ForEach(Array(p.sub.enumerated()), id: \.offset) { j, s in
                                let w = i < subWatching.count && j < subWatching[i].count && subWatching[i][j]
                                let pic = i < subPictures.count && j < subPictures[i].count ? subPictures[i][j] : nil
                                subLine(s, ink, picture: w ? .some(pic) : nil) { onSubPicture?(i, j) }
                            }
                        }
                        .padding(.leading, Self.subIndent).padding(.trailing, 14).padding(.bottom, 4)
                    }
                } else {
                    if i > 0 && !Self.live(parts[i - 1].kind) { hairline(ink) }
                    ended(p, ink)
                }
            }
            if let status {
                hairline(ink)
                Text(status).type(13, .medium, ink.secondary).lineLimit(1).padding(.horizontal, 14).padding(.top, 6).padding(.bottom, 2)
            }
            if footer != nil || !footerLines.isEmpty {
                hairline(ink)
                (Text(footer ?? "").fontWeight(.semibold).foregroundColor(ink.primary)
                 + Text((footer != nil && !footerLines.isEmpty ? "   " : "") + footerLines.joined(separator: "   ")).foregroundColor(ink.secondary))
                    .font(.system(size: 14))
                    .padding(.horizontal, 14).padding(.top, 8).padding(.bottom, 4)
            }
        }
        .padding(.vertical, 6)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(ink.card, in: RoundedRectangle(cornerRadius: 22, style: .continuous))
        .overlay { if let c = tint.color { RoundedRectangle(cornerRadius: 22, style: .continuous).fill(c.opacity(0.14)).allowsHitTesting(false) } }
    }

    private func hairline(_ ink: CardInk) -> some View { Rectangle().fill(ink.line).frame(height: 1).padding(.horizontal, 14) }

    @ViewBuilder private func line(_ p: PlanPart, _ ink: CardInk, picture: UIImage??, index: Int?) -> some View {
        let waiting = p.kind == .waiting
        HStack(spacing: 8) {
            Group {
                if waiting { Circle().stroke(ink.faint, lineWidth: 1.2).frame(width: 10, height: 10) }
                else { RunningArc(color: ink.secondary, width: Self.spinnerWidth).padding(1) }
            }
            .frame(width: 14, height: 14)
            ViewThatFits(in: .horizontal) {
                HStack(spacing: 8) { label(p, ink, waiting).fixedSize(); words(p, ink).fixedSize() }
                HStack(spacing: 8) { label(p, ink, waiting); words(p, ink) }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            if let picture {
                Button { if let index { onPicture?(index) } } label: {
                    Group { if let image = picture { Image(uiImage: image).resizable().scaledToFill() } else { Ink.darkFill } }
                        .frame(width: 24, height: 24).clipShape(Circle())
                        .overlay(Circle().stroke(Ink.amber.opacity(0.65), lineWidth: 1))
                }
                .buttonStyle(.plain).accessibilityLabel("Live preview of \(p.title)")
            }
        }
        .padding(.leading, 14).padding(.trailing, 14).padding(.vertical, 3)
        .frame(minHeight: 34)
        .contentShape(Rectangle())
        .onTapGesture { if picture == nil, let index { onPart?(index) } }
        .accessibilityElement(children: .combine)
    }

    /// A step under a waiting part (FERB.app's ChecklistLineView): its small mark, its label (semibold while it runs,
    /// greyed once done) that keeps the line before its state words do, those words, and its picture while it runs.
    private func subLine(_ p: PlanPart, _ ink: CardInk, picture: UIImage??, onPicture: @escaping () -> Void) -> some View {
        HStack(spacing: 8) {
            subMark(p.kind, ink).frame(width: 14, height: 14)
            Text(p.title).type(14, p.kind == .running ? .semibold : .regular,
                               p.kind == .done ? ink.faint : p.kind == .running || p.kind == .asking ? ink.primary : ink.secondary)
                .lineLimit(1).truncationMode(.tail).layoutPriority(1)
            if let d = p.detail, !d.isEmpty {
                Text(d).type(13.5, .regular, p.kind == .asking ? ink.amberText : p.kind == .failed ? ink.red : ink.secondary)
                    .lineLimit(1).truncationMode(.tail)
            }
            Spacer(minLength: 0)
            if let picture {
                Button(action: onPicture) {
                    Group { if let image = picture { Image(uiImage: image).resizable().scaledToFill() } else { Ink.darkFill } }
                        .frame(width: 24, height: 24).clipShape(Circle())
                        .overlay(Circle().stroke(Ink.amber.opacity(0.65), lineWidth: 1))
                }
                .buttonStyle(.plain).accessibilityLabel("Live preview of \(p.title)")
            }
        }
        .padding(.vertical, 1)
        .frame(minHeight: 26)
        .accessibilityElement(children: .combine)
    }

    /// FERB.app's checklist marks at the phone's size: a check, the turning arc, a hollow circle, a question, a cross.
    @ViewBuilder private func subMark(_ k: MarkKind, _ ink: CardInk) -> some View {
        switch k {
        case .done: Image(systemName: "checkmark").font(.system(size: 9, weight: .bold)).foregroundStyle(ink.secondary)
        case .running: RunningArc(color: ink.secondary, width: Self.spinnerWidth).padding(1)
        case .waiting: Circle().stroke(ink.faint, lineWidth: 1.2).frame(width: 10, height: 10)
        case .asking: Image(systemName: "questionmark.circle.fill").font(.system(size: 10)).foregroundStyle(Ink.amber)
        case .failed: Image(systemName: "xmark").font(.system(size: 9, weight: .bold)).foregroundStyle(ink.red)
        }
    }

    private func label(_ p: PlanPart, _ ink: CardInk, _ waiting: Bool) -> some View {
        Text(p.title).type(15, waiting ? .regular : .semibold, waiting ? ink.secondary : ink.primary).lineLimit(1)
    }

    @ViewBuilder private func words(_ p: PlanPart, _ ink: CardInk) -> some View {
        if let d = p.detail, !d.isEmpty { Text(d).type(14, .regular, p.tone.color(onFerb)).lineLimit(1) }
    }

    private func ended(_ p: PlanPart, _ ink: CardInk) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 6) {
                Text(p.title).type(13, .semibold, ink.secondary).lineLimit(2)
                if p.updated {
                    Text("UPDATED").font(.system(size: 10, weight: .bold)).tracking(0.5).foregroundStyle(.white)
                        .padding(.horizontal, 5).background(ink.link, in: RoundedRectangle(cornerRadius: 4))
                }
            }
            if let d = p.detail { Text(d).type(15, .regular, p.tone == .dim ? ink.primary : p.tone.color(onFerb)).lineSpacing(2) }
            if let rich = p.rich {
                let look = RichLook(size: 15, ink: ink.primary, dim: ink.secondary, link: ink.link, codeFill: ink.fill)
                RichBlocksView(rich, look: look, key: p.key, fold: .row, more: more, less: less, width: 300)
            }
            ForEach(Array(p.notes.enumerated()), id: \.offset) { _, n in
                (Text(n.label ?? "").fontWeight(.semibold).foregroundColor(n.tone.color(onFerb)) + Text(n.text).foregroundColor(n.label == nil ? n.tone.color(onFerb) : ink.secondary))
                    .font(.system(size: 13.5))
            }
        }
        .padding(.horizontal, 14).padding(.vertical, 8)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// FERB's blocks drawn whole: words as one selectable text that folds as FERB.app's (`RichWords`); a table as a grid
/// that scrolls sideways when wider than its place (at most 12 columns and 200 rows reach here). With a table the
/// text doesn't fold.
public struct RichBlocksView: View {
    let blocks: [RichBlock]
    let look: RichLook
    let key: String
    var fold: Fold = .none
    var more = "Show all", less = "Show less"
    var tail: [(String, Bool)] = []
    var width: CGFloat = 288

    public init(_ blocks: [RichBlock], look: RichLook, key: String, fold: Fold = .none, more: String = "Show all", less: String = "Show less",
                tail: [(String, Bool)] = [], width: CGFloat = 288) {
        self.blocks = blocks; self.look = look; self.key = key; self.fold = fold; self.more = more; self.less = less; self.tail = tail; self.width = width
    }

    public var body: some View {
        if !blocks.contains(where: { if case .table = $0 { return true } else { return false } }) {
            // kept per message (RichTextCache): a body run again, or a row scrolling back in, builds nothing (kb PHN-716)
            RichWords(RichTextCache.text(key, blocks, look, tail: tail), look: look, key: key, fold: fold, more: more, less: less,
                      whenFolded: tail.isEmpty ? nil : RichTextCache.text(key + ":tail", [], look, tail: tail), width: width)
        } else {
            VStack(alignment: .leading, spacing: 10) {
                ForEach(Array(segments.enumerated()), id: \.offset) { i, seg in
                    switch seg {
                    case .words(let bs): Text(RichTextCache.text("\(key):\(i)", bs, look, tail: i == segments.count - 1 ? tail : [])).lineSpacing(2).textSelection(.enabled)
                    case .table(let header, let rows): grid(header, rows)
                    }
                }
            }
        }
    }

    private enum Segment { case words([RichBlock]), table([[RichRun]], [[[RichRun]]]) }

    private var segments: [Segment] {
        var out: [Segment] = [], run: [RichBlock] = []
        for b in blocks {
            if case .table(let h, let r) = b { if !run.isEmpty { out.append(.words(run)); run = [] }; out.append(.table(h, r)) } else { run.append(b) }
        }
        if !run.isEmpty { out.append(.words(run)) }
        return out
    }

    private func grid(_ header: [[RichRun]], _ rows: [[[RichRun]]]) -> some View {
        ScrollView(.horizontal, showsIndicators: false) {
            Grid(alignment: .leading, horizontalSpacing: 14, verticalSpacing: 6) {
                ForEach(Array(([header] + rows).enumerated()), id: \.offset) { r, row in
                    GridRow {
                        ForEach(Array(row.enumerated()), id: \.offset) { _, cell in
                            Text(richText([.paragraph(cell)], RichLook(size: look.size - 1, ink: look.ink, dim: look.dim, link: look.link, codeFill: look.codeFill)))
                                .fontWeight(r == 0 ? .semibold : .regular).frame(minWidth: 64, maxWidth: 220, alignment: .leading)
                        }
                    }
                }
            }
        }
    }
}

/// `text` and the time since `since`, again each second in this text alone (FERB.app's LiveLineView, "… · 0:42").
public struct TickingText: View {
    let text: String
    let since: Date
    let size: CGFloat
    let color: Color
    @Environment(\.cardClock) private var clock
    public init(_ text: String, since: Date, size: CGFloat, color: Color) { self.text = text; self.since = since; self.size = size; self.color = color }
    public var body: some View {
        TimelineView(.periodic(from: .now, by: 1)) { ctx in
            Text(text + " · " + elapsedText(max(0, Int((clock ?? ctx.date).timeIntervalSince(since)))))
                .type(size, .medium, color).monospacedDigit().lineLimit(1)
        }
    }
}

/// FERB's words as built for the chat, kept for the messages seen most recently (Android's RichCache, kb PHN-716): a
/// body run again, or a row scrolling back into the list, finds its text here instead of building it on that frame.
/// Kept per message key and checked against its blocks (the model's own array: equal at once while unchanged; a
/// streamed word makes new ones), its lines under the words and its look's size.
public enum RichTextCache {
    private struct Kept { let blocks: [RichBlock]; let tail: [String]; let size: CGFloat; let text: AttributedString }
    private static let lock = NSLock()
    nonisolated(unsafe) private static var kept: [String: Kept] = [:]
    nonisolated(unsafe) private static var order: [String] = []
    /// How many texts were built (tests: a message seen again adds none).
    nonisolated(unsafe) public private(set) static var builds = 0

    public static func text(_ key: String, _ blocks: [RichBlock], _ look: RichLook, tail: [(String, Bool)] = []) -> AttributedString {
        let t = tail.map { "\($0.1 ? 1 : 0)\($0.0)" }
        lock.lock()
        if let k = kept[key], k.size == look.size, k.tail == t, k.blocks == blocks { lock.unlock(); return k.text }
        lock.unlock()
        let made = richText(blocks, look, tail: tail)
        lock.lock()
        builds += 1
        if kept[key] == nil {
            order.append(key)
            if order.count > 400 { kept[order.removeFirst()] = nil }
        }
        kept[key] = Kept(blocks: blocks, tail: t, size: look.size, text: made)
        lock.unlock()
        return made
    }
}
