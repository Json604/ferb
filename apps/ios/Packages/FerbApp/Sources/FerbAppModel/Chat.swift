import FerbDesign
import FerbMobile
import Foundation

// The chat as the phone draws it: the core's transcript (the Mac's own fold, transcripts.json's shape) turned into
// rows FerbDesign's pieces draw (Android: model/Chat.kt). Pure: every scenario of the engine's fixtures goes through
// it in the tests, through the real library.

/// A task's parts in one white card; `live` while a part runs or asks (Watch live and Stop show then).
public struct ChatTask: Equatable, Sendable {
    public let key: String
    public let taskId: String?
    public let headline: String
    public let done: Int
    public let total: Int
    public let parts: [PlanPart]

    public init(key: String, taskId: String?, headline: String, done: Int, total: Int, parts: [PlanPart]) {
        self.key = key
        self.taskId = taskId
        self.headline = headline
        self.done = done
        self.total = total
        self.parts = parts
    }

    public var live: Bool { parts.contains { $0.kind == .running || $0.kind == .asking } }
}

/// One line of a result card or a file. A file a finished task made carries its `taskId` and `index` (its place
/// among the outcome's sorted files): a tap fetches it from the Mac by those (kb PHN-535).
public struct CardLine: Equatable, Sendable {
    public let title: String
    public let detail: String?
    public var file: Bool = false
    public var taskId: String? = nil
    public var index: Int? = nil

    public init(title: String, detail: String?, file: Bool = false, taskId: String? = nil, index: Int? = nil) {
        self.title = title
        self.detail = detail
        self.file = file
        self.taskId = taskId
        self.index = index
    }
}

/// One row of the chat, top to bottom. `key` is stable across reads (an entry id and a part).
public enum ChatRow: Equatable, Identifiable {
    /// The owner's words (from this phone, the Mac or another device: drawn alike, as the Mac does, APP-916).
    case owner(key: String, text: String, files: [String], note: String?)
    /// FERB's words in a black bubble.
    case ferb(key: String, text: String)
    /// A short line on the sky: a notice, the Mac's note under a message, a live activity line.
    case line(key: String, text: String, quotes: [String])
    case task(ChatTask)
    /// Typed result cards and the files a task made.
    case cards(key: String, cards: [CardLine])
    /// FERB is writing.
    case typing(key: String)
    /// An open question, drawn where the chat reaches it; tapping opens its sheet.
    case asking(key: String, approvalId: String, title: String?)
    /// A message in the outbox (PHN-401): waiting, sending or failed, cancellable.
    case outbox(key: String, item: OutboxItem)
    /// The top of the chat: older messages can be fetched, or wait for a better path.
    case older(key: String, state: OlderState)

    public var key: String {
        switch self {
        case .owner(let k, _, _, _), .ferb(let k, _), .line(let k, _, _), .cards(let k, _), .typing(let k),
             .asking(let k, _, _), .outbox(let k, _), .older(let k, _):
            return k
        case .task(let t):
            return t.key
        }
    }

    public var id: String { key }
}

public enum ChatModel {
    /// The rows of `view` with the outbox at the end.
    public static func rows(_ view: TranscriptView?) -> [ChatRow] {
        guard let view else { return [] }
        var out: [ChatRow] = []
        switch view.older {
        case .available, .needsBetterPath: out.append(.older(key: "older", state: view.older))
        default: break
        }
        var live: [String: LiveView] = [:]
        for l in view.live { live[l.entry] = l }
        var turns: [String: TurnItem] = [:]
        for item in view.items { if case .turn(let t) = item { turns[t.entry] = t } }
        let open = view.questions.filter(\.open)
        var placed = Set<String>()
        for item in view.items {
            switch item {
            case .turn(let t):
                turn(t, live[t.entry], &out)
                for q in open {
                    guard let id = q.approvalId else { continue }
                    let mine = (q.requestId != nil && q.requestId == t.requestId) || (q.taskId != nil && q.taskId == t.taskId)
                    if mine && placed.insert(id).inserted { out.append(.asking(key: "q:\(id)", approvalId: id, title: Questions.parse(q).title)) }
                }
            case .reply(let r):
                let turn = turns[r.answers]
                if r.quotesRequest, let text = turn?.words?.text { out.append(.line(key: "\(r.entry):quote", text: "“\(oneLine(text))”", quotes: [])) }
                if let said = r.said {
                    answer(r.entry, said, &out, taskId: turn?.taskId)
                } else if r.part == "question" {
                    if let q = turn?.question { out.append(.ferb(key: "\(r.entry):q", text: plain(q))) }
                } else if let a = turn?.answer {
                    answer(r.entry, a, &out, taskId: turn?.taskId)
                }
            case .notice(let n), .reminder(let n):
                notice(n, &out)
            }
        }
        for q in open {
            guard let id = q.approvalId else { continue }
            if placed.insert(id).inserted { out.append(.asking(key: "q:\(id)", approvalId: id, title: Questions.parse(q).title)) }
        }
        for o in view.outbox { out.append(.outbox(key: "o:\(o.id)", item: o)) }
        return out
    }

    private static func notice(_ n: NoticeItem, _ out: inout [ChatRow]) {
        out.append(.line(key: n.entry, text: n.text, quotes: n.quotes.map { "“\($0.text)” (\($0.origin))" }))
        if let cards = n.cards, !cards.isEmpty { out.append(.cards(key: "\(n.entry):cards", cards: cards.map(cardLine))) }
    }

    private static func turn(_ t: TurnItem, _ live: LiveView?, _ out: inout [ChatRow]) {
        let e = t.entry
        if t.from == "owner", let words = t.words {
            out.append(.owner(key: e, text: words.text, files: words.files.map(fileName), note: words.hiddenNotice))
        } else if let goal = t.goal {
            out.append(.line(key: "\(e):goal", text: goal, quotes: []))
        }
        if let n = t.notice { out.append(.line(key: "\(e):notice", text: n, quotes: [])) }
        let first = t.firstReply
        let hasCard = t.card != nil || t.checklist != nil
        if let first, first.says {
            var lines = first.direct + first.receipts
            if !hasCard { lines += [first.planLine ?? first.plan].compactMap { $0 } + first.planLines }
            let text = lines.filter { !$0.isBlank }.joined(separator: "\n")
            if !text.isEmpty { out.append(.ferb(key: "\(e):first", text: plain(text))) }
            if let rc = first.receiptCards, !rc.isEmpty { out.append(.cards(key: "\(e):receipts", cards: rc.map(cardLine))) }
        }
        if let c = t.card { out.append(.task(card(e, t, c, first?.planLine, live))) }
        if let c = t.checklist { out.append(.task(checklist(e, t, c, first?.planLine, live))) }
        if !t.parts.isEmpty { out.append(.ferb(key: "\(e):parts", text: t.parts.joined(separator: "\n"))) }
        if t.answer != nil, let q = t.question { out.append(.ferb(key: "\(e):asked", text: plain(q))) }
        if let live {
            if let p = live.streamingPlan, !p.isBlank, !hasCard { out.append(.ferb(key: "\(e):plan-live", text: plain(p))) }
            if let r = live.streamingReply, !r.isBlank { out.append(.ferb(key: "\(e):reply-live", text: plain(r))) }
            if let n = live.replyNotice { out.append(.line(key: "\(e):reply-notice", text: n, quotes: [])) }
            if !hasCard, let l = live.line?.text { out.append(.line(key: "\(e):line", text: l, quotes: [])) }
            if let a = live.activity { out.append(.line(key: "\(e):activity", text: a, quotes: [])) }
            if live.typing && (live.streamingReply ?? "").isBlank { out.append(.typing(key: "\(e):typing")) }
        }
        if !t.answeredBelow {
            if let a = t.answer { answer(e, a, &out, taskId: t.taskId) }
        } else if t.answer == nil, let q = t.question {
            out.append(.ferb(key: "\(e):asked", text: plain(q)))
        }
    }

    private static func answer(_ key: String, _ a: AnswerView, _ out: inout [ChatRow], taskId: String?) {
        let text = plain(a.displayText ?? a.text)
        let extra = (a.lines ?? []).filter { !$0.isBlank }
        let body = ([text] + extra).filter { !$0.isBlank }.joined(separator: "\n")
        if !body.isEmpty { out.append(.ferb(key: "\(key):answer", text: body)) }
        let cards = (a.cards ?? []).map(cardLine)
            + a.artifacts.enumerated().map { i, p in CardLine(title: fileName(p), detail: folderOf(p), file: true, taskId: taskId, index: i) }
        if !cards.isEmpty { out.append(.cards(key: "\(key):cards", cards: cards)) }
        if let n = a.note, !n.isBlank { out.append(.line(key: "\(key):note", text: n, quotes: [])) }
    }

    private static func card(_ e: String, _ t: TurnItem, _ c: CardView, _ planLine: String?, _ live: LiveView?) -> ChatTask {
        var liveRows: [String: LiveRow] = [:]
        for r in live?.rows ?? [] { liveRows[r.id] = r }
        let parts = c.rows.map { r -> PlanPart in
            let l = liveRows[r.row.part]
            let kind = l.map { markOf($0.kind) } ?? markOf(r.shows.kind)
            return PlanPart(kind: kind, title: r.row.label, detail: l?.words ?? r.shows.words ?? r.row.note)
        }
        return ChatTask(key: "\(e):card", taskId: c.taskId.isEmpty ? t.taskId : c.taskId,
                        headline: c.headline ?? c.line ?? planLine ?? t.taskName ?? "", done: Int(c.done), total: Int(c.total), parts: parts)
    }

    private static func checklist(_ e: String, _ t: TurnItem, _ c: ChecklistView, _ planLine: String?, _ live: LiveView?) -> ChatTask {
        var liveRows: [String: LiveRow] = [:]
        for r in live?.checklist ?? [] { liveRows[r.id] = r }
        var lines: [String: ChecklistLine] = [:]
        for l in c.lines { lines[l.step] = l }
        let parts = c.steps.map { s -> PlanPart in
            let l = liveRows[s.id]
            let kind: MarkKind
            if let l { kind = markOf(l.kind) }
            else if let end = c.ended[s.id] { kind = markOf(end) }
            else if live != nil { kind = .waiting }
            else if t.status == "working" || t.status == "needs_user" { kind = .waiting }
            else { kind = markOf(t.status) }
            return PlanPart(kind: kind, title: s.label, detail: l?.words ?? lines[s.id]?.words)
        }
        return ChatTask(key: "\(e):checklist", taskId: c.taskId.isEmpty ? t.taskId : c.taskId,
                        headline: planLine ?? c.collapsed ?? t.taskName ?? "", done: c.ended.count, total: c.steps.count, parts: parts)
    }

    /// A row's, a line's or a turn's state as the phone's marks draw it (each shape its own, never colour alone).
    public static func markOf(_ kind: String) -> MarkKind {
        switch kind {
        case "done", "updated", "gaps", "done_with_gaps", "done_unverified": return .done
        case "needsYou", "needs_you", "needs_user", "asks": return .asking
        case "failed", "notFinished", "wontRun", "stopped", "cancelled", "interrupted": return .failed
        case "waiting", "paused", "sleeping": return .waiting
        default: return .running // working, running, starting, retrying, checking, previewRunning
        }
    }

    public static func cardLine(_ c: ResultCard) -> CardLine {
        let title = c.title ?? c.what ?? c.text ?? c.name ?? c.line ?? c.kindOf
        let detail = [c.due ?? c.when ?? c.nextRun, c.repeatRule, c.path.flatMap(folderOf)].compactMap { $0 }.first
            ?? (c.line != title ? c.line : nil)
        return CardLine(title: title, detail: detail, file: c.kindOf == "file")
    }

    public static func fileName(_ path: String) -> String {
        let p = trimEnd(path)
        return p.lastIndex(of: "/").map { String(p[p.index(after: $0)...]) } ?? p
    }

    public static func folderOf(_ path: String) -> String? {
        let p = trimEnd(path)
        guard let slash = p.lastIndex(of: "/") else { return nil }
        let parent = String(p[..<slash])
        let name = parent.lastIndex(of: "/").map { String(parent[parent.index(after: $0)...]) } ?? parent
        return name.isEmpty ? nil : name
    }

    private static func trimEnd(_ s: String) -> String {
        var s = Substring(s)
        while s.last == "/" { s = s.dropLast() }
        return String(s)
    }

    private static func oneLine(_ s: String) -> String {
        let first = s.lines.first ?? ""
        return first.count > 80 ? String(first.prefix(79)) + "…" : first
    }

    private static let heading = try! NSRegularExpression(pattern: "^#{1,6}\\s+")
    private static let bullet = try! NSRegularExpression(pattern: "^\\s*[-*]\\s+")
    private static let mdLink = try! NSRegularExpression(pattern: "\\[([^\\]]+)]\\(([^)]+)\\)")

    /// FERB's answers are Markdown; the phone's bubble draws plain text (the lab's bubble has no rich text), so the
    /// marks that only style are taken out: bold and italic stars, code ticks, heading hashes, a link's brackets.
    public static func plain(_ md: String) -> String {
        func sub(_ re: NSRegularExpression, _ s: String, _ with: String) -> String {
            re.stringByReplacingMatches(in: s, range: NSRange(s.startIndex..., in: s), withTemplate: with)
        }
        let out = md.lines.map { line -> String in
            var l = sub(heading, line, "")
            l = sub(bullet, l, "• ")
            l = l.replacingOccurrences(of: "**", with: "").replacingOccurrences(of: "__", with: "").replacingOccurrences(of: "`", with: "")
            return sub(mdLink, l, "$1 ($2)")
        }.joined(separator: "\n")
        return out.trimmingCharacters(in: .whitespacesAndNewlines)
    }
}
