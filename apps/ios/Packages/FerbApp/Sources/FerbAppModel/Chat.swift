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
    /// FERB.app's running status line ("1 of 4 done · 2 running"), and once ended its footer and code's lines (kb PHN-726).
    public var status: String? = nil
    public var footer: String? = nil
    public var footerLines: [String] = []
    /// Each part's own live picture, by its step task id while its step works (FERB.app's RowPreview.watch); nil: none.
    public var watch: [String?] = []
    /// The card's colour as a whole (FERB.app's surfaceTint).
    public var tint: CardTint = .none

    public init(key: String, taskId: String?, headline: String, done: Int, total: Int, parts: [PlanPart],
                status: String? = nil, footer: String? = nil, footerLines: [String] = [], watch: [String?] = [], tint: CardTint = .none) {
        self.watch = watch
        self.tint = tint
        self.status = status
        self.footer = footer
        self.footerLines = footerLines
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
    case line(key: String, text: String, quotes: [String], since: Date? = nil)
    /// FERB.app's status words, notices and the draft line under a reply: smaller than the words and grey,
    /// "Delivered, with parts not done" amber (kb PHN-773).
    case small(key: String, text: String, amber: Bool)
    case task(ChatTask)
    /// Typed result cards and the files a task made.
    case cards(key: String, cards: [CardLine])
    /// FERB is writing.
    case typing(key: String)
    /// An open question, drawn where the chat reaches it; tapping opens its sheet. `title` and `reason` are the words
    /// FERB.app's inline question leads with (kb PHN-729).
    case asking(key: String, approvalId: String, title: String?, reason: String?, hidden: Bool)
    /// FERB's Markdown as FERB.app draws it (kb PHN-721): its blocks, its words for Copy, quieter lines under it, its fold.
    case rich(key: String, blocks: [FerbDesign.RichBlock], copy: String, tail: [String], fold: Fold)
    /// A turn's answer as FERB.app draws it (kb PHN-723, PHN-724).
    case answer(key: String, ui: AnswerUi)
    /// FERB's own message in its bubble with its boxes, quotes, facts and pages (NoticeView).
    case notice(key: String, text: String, quotes: [Quote], ui: NoticeUi)
    /// Result cards by kind (a first reply's receipts).
    case results(key: String, cards: [ResultShown])
    /// A message in the outbox (PHN-401): waiting, sending or failed, cancellable.
    case outbox(key: String, item: OutboxItem)
    /// The top of the chat: older messages can be fetched, or wait for a better path.
    case older(key: String, state: OlderState)
    /// A day's start in the one continuous chat (kb PHN-627): "Today", "Yesterday", or the date; `day` in epoch days.
    case day(key: String, day: Int)
    /// A message he sent with files, held on the phone until its files are on the Mac (kb PHN-691).
    case withFiles(key: String, item: Sending)
    /// At the top of today's chat: the way to earlier days, as the home page has it (kb PHN-698).
    case earlierLink(key: String)
    /// The memory import's live card at the end of today's chat while one runs (kb MEM-456): live state from the Mac's
    /// status, never an entry of the chat; its notice takes its place when it ends.
    case importProgress(key: String, card: ImportCard)

    public var key: String {
        switch self {
        case .owner(let k, _, _, _), .ferb(let k, _), .line(let k, _, _, _), .small(let k, _, _), .cards(let k, _), .typing(let k),
             .asking(let k, _, _, _, _), .outbox(let k, _), .older(let k, _), .day(let k, _), .withFiles(let k, _), .earlierLink(let k),
             .rich(let k, _, _, _, _), .answer(let k, _), .notice(let k, _, _, _), .results(let k, _), .importProgress(let k, _):
            return k
        case .task(let t):
            return t.key
        }
    }

    public var id: String { key }
}

public enum ChatModel {
    /// When `item`'s entry was made (the library's `at`: seconds since 2001-01-01, Foundation's reference date).
    public static func date(_ item: Item) -> Date {
        switch item {
        case .turn(let t): Date(timeIntervalSinceReferenceDate: t.at)
        case .reply(let r): Date(timeIntervalSinceReferenceDate: r.at)
        case .notice(let n), .reminder(let n): Date(timeIntervalSinceReferenceDate: n.at)
        }
    }

    /// The calendar day of `date` in `calendar`'s zone, as epoch days (Android's LocalDate.toEpochDay()).
    public static func day(_ date: Date, _ calendar: Calendar) -> Int {
        let c = calendar.dateComponents([.year, .month, .day], from: date)
        var utc = Calendar(identifier: .gregorian)
        utc.timeZone = TimeZone(identifier: "UTC")!
        let start = utc.date(from: DateComponents(year: c.year, month: c.month, day: c.day))!
        return Int((start.timeIntervalSince1970 / 86_400).rounded(.down))
    }

    private static func entry(_ item: Item) -> String {
        switch item {
        case .turn(let t): t.entry
        case .reply(let r): r.entry
        case .notice(let n), .reminder(let n): n.entry
        }
    }

    /// Each entry's day (epoch days), for `withDays`.
    public static func days(_ view: TranscriptView?, _ calendar: Calendar) -> [String: Int] {
        var out: [String: Int] = [:]
        for item in view?.items ?? [] { out[entry(item)] = day(date(item), calendar) }
        return out
    }

    /// Something was said today (kb PHN-626): an entry of today, or a message of his still in the outbox. False for an
    /// empty chat: then the chat tab shows the home screen (a first-time user, or a new day with nothing said yet).
    /// What counts as "today" for the chat (kb PHN-697; Android's ChatModel.chatDay): the phone's local calendar day
    /// of `date`. The one place this rule lives (the owner will decide what midnight does mid-conversation); the model
    /// holds its answer and moves it only at a natural moment (the app opened or unlocked or back in front, the Chat
    /// tab chosen), never under him.
    public static func chatDay(_ date: Date, _ calendar: Calendar) -> Int { day(date, calendar) }

    /// The chat's two places (kb PHN-696): today's rows, and the earlier days'.
    public struct Places: Equatable {
        public let today: [ChatRow]
        public let earlier: [ChatRow]
    }

    /// Splits `rows` (as `withDays` made them) at `day` (kb PHN-696, FERB.app's day separation): today has the rows of
    /// that day and after, his outbox and messages with files, and the open questions of earlier days at its end (FERB.app
    /// shows questions only in today's chat), with no day starts; earlier has every row before it with their day starts
    /// and the "earlier messages" row at its top, and no questions. Rows with no day (no times known) are today's.
    public static func split(_ rows: [ChatRow], day: Int) -> Places {
        var today: [ChatRow] = []
        var earlier: [ChatRow] = []
        var asked: [ChatRow] = []
        var current: Int?
        for r in rows {
            switch r {
            case .day(_, let d):
                current = d
                if d < day { earlier.append(r) }
            case .older: earlier.append(r)
            case .outbox, .withFiles: today.append(r)
            case .asking: if let c = current, c < day { asked.append(r) } else { today.append(r) }
            default: if let c = current, c < day { earlier.append(r) } else { today.append(r) }
            }
        }
        return Places(today: today + asked, earlier: earlier)
    }

    /// The memory import card's one key: it stays while the import runs, so its bar moves in place (kb MEM-456).
    public static let importKey = "memory-import"

    /// What the chat draws (kb PHN-696): the earlier days as they are; or today's rows with the way to earlier days at
    /// their top when there are some, then the questions the chat doesn't hold yet (`loose`, kb PHN-770), then the
    /// memory import's card while the Mac's status has one (kb MEM-456; FERB.app's ConversationTranscript rows,
    /// Android's Chat()). The card is never in the earlier days.
    public static func shown(_ places: Places, earlier: Bool, loose: [ChatRow], importing: ImportCard?) -> [ChatRow] {
        if earlier { return places.earlier }
        let hasEarlier = places.earlier.contains { if case .day = $0 { return false } else { return true } }
        let card: [ChatRow] = importing.map { [.importProgress(key: importKey, card: $0)] } ?? []
        let today = places.today + loose + card
        return hasEarlier && !places.today.isEmpty ? [.earlierLink(key: "earlier-link")] + today : today
    }

    /// Something was said on `day` or after (kb PHN-696): one of its rows, or a message of his still in the outbox.
    /// Rows with no day (no times known) count as said.
    public static func said(_ rows: [ChatRow], _ days: [String: Int], from day: Int) -> Bool {
        rows.contains { r in
            switch r {
            case .older, .day, .earlierLink, .asking: return false
            case .outbox, .withFiles: return true
            default: return (days[String(r.key.split(separator: ":", maxSplits: 1).first ?? "")] ?? day) >= day
            }
        }
    }

    public static func saidToday(_ view: TranscriptView?, today: Int, _ calendar: Calendar) -> Bool {
        guard let view else { return false }
        return !view.outbox.isEmpty || view.items.contains { day(date($0), calendar) >= today }
    }

    /// `rows` with a day's start before the first row of each day (kb PHN-627); the outbox is today; a question shares
    /// the day of what is above it.
    public static func withDays(_ rows: [ChatRow], _ days: [String: Int], today: Int) -> [ChatRow] {
        guard !days.isEmpty, !rows.isEmpty else { return rows }
        var out: [ChatRow] = []
        var current: Int?
        for r in rows {
            let d: Int?
            switch r {
            case .older: d = nil
            case .outbox, .withFiles: d = today
            case .asking: d = current
            default: d = days[String(r.key.split(separator: ":", maxSplits: 1).first ?? "")]
            }
            if let d, d != current {
                out.append(.day(key: "day:\(d)", day: d))
                current = d
            }
            out.append(r)
        }
        return out
    }

    /// The day row `index` of `rows` (as `withDays` made them) belongs to (kb PHN-663): the day of the nearest day's
    /// start at or above it, or below it for a row before the first (the "earlier messages" row). Nil without days.
    public static func dayAt(_ rows: [ChatRow], _ index: Int) -> Int? {
        guard rows.indices.contains(index) else { return nil }
        for i in stride(from: index, through: 0, by: -1) { if case .day(_, let d) = rows[i] { return d } }
        for i in (index + 1)..<rows.count { if case .day(_, let d) = rows[i] { return d } }
        return nil
    }

    /// Lays out every answer, notice and Markdown bubble of `view` through the phone library and keeps them (Present),
    /// so `rows` on the main actor finds them all. Off the main actor: the library is called here.
    public static func prepare(_ view: TranscriptView) { _ = rows(view) }

    /// The rows of `view` with the outbox at the end. FERB's words come laid out by the phone library, kept per entry.
    public static func rows(_ view: TranscriptView?) -> [ChatRow] {
        guard let view else { return [] }
        var out: [ChatRow] = []
        switch view.older {
        case .available, .needsBetterPath: out.append(.older(key: "older", state: view.older))
        default: break
        }
        var live: [String: LiveView] = [:]
        for l in view.live { live[l.entry] = l }
        var turnList: [TurnItem] = []
        for item in view.items { if case .turn(let t) = item { turnList.append(t) } }
        var turns: [String: TurnItem] = [:]
        for t in turnList { turns[t.entry] = t }
        let open = view.questions.filter(\.open)
        var placed = Set<String>()
        var questionBelow = Set<String>()
        for item in view.items { if case .reply(let r) = item, r.part == "question" { questionBelow.insert(r.answers) } }
        func ask(_ q: QuestionItem) {
            guard let id = q.approvalId, placed.insert(id).inserted else { return }
            let p = Questions.parse(q)
            // FERB.app's card (kb PHN-739): its one question, with its header ("Check before FERB goes on") under it
            if let s = p.shown {
                out.append(.asking(key: "q:\(id)", approvalId: id, title: s.question, reason: s.header.isEmpty ? nil : s.header, hidden: p.hidden))
            } else {
                out.append(.asking(key: "q:\(id)", approvalId: id, title: p.title, reason: p.reason.isBlank ? nil : p.reason, hidden: p.hidden))
            }
        }
        for item in view.items {
            switch item {
            case .turn(let t):
                var merged: String?
                if let lead = t.mergedInto {
                    let at = turnList.firstIndex { $0.requestId == lead }
                    let mine = turnList.firstIndex { $0.entry == t.entry } ?? 0
                    merged = at.map { "Read together with your message " + ($0 < mine ? "above" : "below") } ?? "Read together with your other message"
                }
                turn(t, live[t.entry], merged: merged, questionBelow: questionBelow.contains(t.entry), &out)
                // where FERB.app draws an open question (ConversationTranscript, `rowPlacements`): a card row's in its
                // turn; every other one at the end of the chat, below what was said since (kb PHN-770). Never under
                // the first turn of its task or request: a message read with an earlier one shares its task, and its
                // question then sat above his newer words (owner 2026-11-23).
                for q in open where q.row.map({ !$0.turn.isEmpty && $0.turn == t.entry }) == true { ask(q) }
            case .reply(let r):
                let turn = turns[r.answers]
                if r.quotesRequest {
                    let quote = (turn?.goal).flatMap { $0.isEmpty ? nil : $0 } ?? (turn?.words?.text).flatMap { $0.isBlank ? nil : $0 }
                    out.append(.line(key: "\(r.entry):quote", text: "↩ " + (quote.map(oneLine) ?? "A task from earlier"), quotes: []))
                }
                if let said = r.said {
                    answer(r.entry, said, card: false, hidden: r.hidden, &out, taskId: turn?.taskId, status: nil)
                } else if r.part == "question", let turn, let q = turn.question {
                    out.append(rich("\(r.entry):q", q))
                    let below = view.items.contains { if case .reply(let x) = $0 { return x.answers == r.answers && x.part == "answer" } else { return false } }
                    if !below, let a = turn.answer { answer(r.entry, a, card: turn.card != nil, hidden: turn.hidden, &out, taskId: turn.taskId, status: turn.statusWords, gapped: turn.status == gaps) }
                } else if let turn, let a = turn.answer {
                    answer(r.entry, a, card: turn.card != nil, hidden: turn.hidden, &out, taskId: turn.taskId, status: turn.statusWords, gapped: turn.status == gaps)
                }
            case .notice(let n), .reminder(let n):
                out.append(.notice(key: n.entry, text: n.text, quotes: n.quotes, ui: Present.notice(n.text, quotes: n.quotes.count, cards: n.cards ?? [], hidden: n.hidden)))
            }
        }
        for q in open { ask(q) }
        for o in view.outbox { out.append(.outbox(key: "o:\(o.id)", item: o)) }
        return out
    }

    private static func rich(_ key: String, _ text: String, tail: [String] = [], fold: Fold = .none, keep: Bool = true) -> ChatRow {
        .rich(key: key, blocks: keep ? Present.rich(text) : Present.richNow(text), copy: text, tail: tail, fold: fold)
    }

    private static func turn(_ t: TurnItem, _ live: LiveView?, merged: String?, questionBelow: Bool, _ out: inout [ChatRow]) {
        let e = t.entry
        if t.from == "owner", let words = t.words {
            out.append(.owner(key: e, text: words.text, files: words.files.map(fileName), note: words.hiddenNotice))
        } else if let goal = t.goal {
            out.append(.line(key: "\(e):goal", text: goal.isEmpty ? "A task from earlier" : goal, quotes: []))
        }
        if let merged { out.append(.line(key: "\(e):merged", text: merged, quotes: [])) }
        let first = t.firstReply
        if let first, first.says {
            // FERB.app's FirstReplyView: the asks answered at once (Markdown) only without a card, the receipts no card
            // stands for, the plan line, code's lines under it quieter; a long one folds (APP-911)
            let line = first.planLine ?? first.plan
            let said = (t.card == nil ? first.direct : []) + first.receiptsShown + [line].compactMap { $0 }
            let text = said.filter { !$0.isBlank }.joined(separator: "\n\n")
            let tail = first.planLine != nil ? first.planLines.filter { !$0.isBlank } : []
            if !text.isEmpty || !tail.isEmpty { out.append(rich("\(e):first", text, tail: tail, fold: .answer)) }
        } else if first == nil, let p = live?.streamingPlan, !p.isBlank {
            out.append(.rich(key: "\(e):first", blocks: FerbDesign.RichBlock.plain(p), copy: p, tail: [], fold: .none))
        }
        if let rc = first?.receiptCards, !rc.isEmpty {
            let cards = Present.cards(rc, hidden: t.hidden)
            if !cards.isEmpty { out.append(.results(key: "\(e):receipts", cards: cards)) }
        }
        if t.card == nil, let c = t.checklist { out.append(.task(checklist(e, t, c, first?.planLine, live))) }
        if let c = t.card {
            out.append(.task(card(e, t, c, first?.planLine, live)))
            if let a = t.answer { answer("\(e):card", a, card: true, hidden: t.hidden, &out, taskId: t.taskId, status: nil) }
        }
        for (i, p) in t.parts.enumerated() where !p.isBlank { out.append(rich("\(e):parts:\(i)", p)) }
        // the connection line with its time (FERB.app's LiveLineView, "… · 0:42")
        if let l = live?.line { out.append(.line(key: "\(e):line", text: l.text, quotes: [], since: Present.time(l.startedAt))) }
        if let live, live.typing && (live.streamingReply ?? "").isBlank { out.append(.typing(key: "\(e):typing")) }
        if let q = t.question, !questionBelow { out.append(rich("\(e):asked", q)) }
        if t.answeredBelow {
            out.append(.line(key: "\(e):below", text: "Answered below", quotes: []))
        } else if t.card == nil {
            if let streaming = live?.streamingReply, !streaming.isBlank, t.answer == nil {
                out.append(rich("\(e):answer", streaming, keep: false))
                // FERB.app's `isProvisional`: a reply that isn't a task's, still coming
                if t.taskId == nil { out.append(.small(key: "\(e):draft", text: "Draft · still responding", amber: false)) }
            } else if let a = t.answer {
                answer(e, a, card: false, hidden: t.hidden, &out, taskId: t.taskId, status: nil)
            }
        }
        if let n = live?.replyNotice { out.append(.small(key: "\(e):reply-notice", text: n, amber: false)) }
        if let n = t.notice { out.append(.small(key: "\(e):notice", text: n, amber: false)) }
        else if let w = t.statusWords { out.append(.small(key: "\(e):status", text: w, amber: t.status == gaps)) }
    }

    /// ORCH-651's status: delivered with parts not done, said in amber (FERB.app ConversationTranscript.swift:678-683).
    private static let gaps = "done_with_gaps"

    private static func answer(_ key: String, _ a: AnswerView, card: Bool, hidden: [String], _ out: inout [ChatRow], taskId: String?, status: String?,
                               gapped: Bool = false) {
        let ui = Present.answer(a, card: card, hidden: hidden, taskId: taskId)
        if ui.bubble || !ui.above.isEmpty || !ui.below.isEmpty || !ui.sites.isEmpty || !ui.files.isEmpty || !ui.pages.isEmpty {
            out.append(.answer(key: "\(key):answer", ui: ui))
        }
        if let w = status ?? ui.statusWords { out.append(.small(key: "\(key):status", text: w, amber: status != nil ? gapped : a.status == gaps)) }
    }

    /// A row's words without their look (tests, and what reading the chat aloud would say).
    public static func words(_ row: ChatRow) -> String {
        switch row {
        case .owner(_, let t, _, _), .ferb(_, let t), .line(_, let t, _, _), .small(_, let t, _): return t
        case .rich(_, _, let copy, let tail, _): return ([copy] + tail).joined(separator: "\n")
        case .answer(_, let ui):
            return ([ui.copy] + ui.gaps.map { GapLines.label + " " + $0 } + [ui.note].compactMap { $0 } + ui.lines)
                .filter { !$0.isEmpty }.joined(separator: "\n")
        case .notice(_, let text, _, _): return text
        case .task(let t): return t.headline + "\n" + t.parts.map(\.title).joined(separator: "\n")
        case .importProgress(_, let c):
            return ([c.title] + c.rows.flatMap { r in [c.rows.count > 1 ? r.name : "", r.line, r.count, r.foundLine, r.detail, r.note] })
                .filter { !$0.isEmpty }.joined(separator: "\n")
        default: return ""
        }
    }

    private static func card(_ e: String, _ t: TurnItem, _ c: CardView, _ planLine: String?, _ live: LiveView?) -> ChatTask {
        var liveRows: [String: LiveRow] = [:]
        for r in live?.rows ?? [] { liveRows[r.id] = r }
        // APP-895: under a row still waiting, the steps it waits on that no row answers, each its own line
        let feeders = Dictionary(grouping: live?.feeders ?? [], by: \.part)
        let parts = c.rows.map { r -> PlanPart in
            let l = liveRows[r.row.part]
            var p = part(r.row, display: l?.kind ?? r.shows.kind, words: l?.words ?? r.shows.words, key: "\(e):\(r.row.part)")
            p.sub = (feeders[r.row.part] ?? []).map { f in PlanPart(kind: markOf(f.line.kind), title: f.label, detail: f.line.words) }
            return p
        }
        // FERB.app's surfaceTint: amber while it asks, red when a part failed, blue when one was updated or paused
        let states = Set(c.rows.map(\.row.state))
        let tint: CardTint = !(live?.openQuestions ?? []).isEmpty || states.contains("asks") ? .asking
            : states.contains("failed") ? .failed
            : states.contains("paused") || states.contains("updated") ? .updated : .none
        // FERB.app's RowPreview.watch: a row whose own step works now (starting, running, retrying, checking) and
        // that isn't answered at once or settled shows that step's picture
        let watch = c.rows.map { r -> String? in
            guard let l = liveRows[r.row.part], ["starting", "running", "retrying", "checking", "working"].contains(l.kind), !r.row.direct,
                  let step = r.row.stepTaskId, !step.isEmpty else { return nil }
            return step
        }
        return ChatTask(key: "\(e):card", taskId: c.taskId.isEmpty ? t.taskId : c.taskId,
                        headline: c.headline ?? planLine ?? t.taskName ?? "", done: Int(c.done), total: Int(c.total), parts: parts,
                        status: live?.cardLine, footer: c.line, footerLines: c.linesShown, watch: watch, tint: tint)
    }

    /// One part of a card as FERB.app's CardRowView shows it (kb PHN-733): by what the row shows now (`display`, the
    /// live RowDisplay or the stored Shows), its answer in rich text (done, updated, delivered with gaps, needs you), a
    /// failure's or stop's words, else its state words; then "Couldn't confirm", "Updated: …" and "You chose …".
    static func part(_ row: FerbMobile.CardRow, display: String, words: String?, key: String) -> PlanPart {
        var p = PlanPart(kind: markOf(display), title: row.label, key: key)
        // code's "Not done:" lines under a part's words: small notes, "Not done:" amber, as FERB.app (kb APP-978)
        let gapped = row.text.map(GapLines.init)
        let text = gapped.flatMap { $0.body.isBlank ? nil : $0.body }
        switch display {
        case "preview": p.detail = "Preview: I'd do this step"
        case "previewRunning": p.detail = "Preview: as if working…"
        case "failed": p.detail = text ?? "Couldn't do it."; p.tone = .red
        case "wontRun": p.detail = words; p.tone = .red
        case "stopped": p.detail = text
        case "done", "updated": p.rich = text.map(Present.rich)
        case "gaps": p.detail = words; p.tone = .amber; p.rich = text.map(Present.rich)
        case "needsYou", "paused":
            if let text { p.rich = Present.rich(text) } else if row.question == nil { p.detail = words; p.tone = .amber }
        default: p.detail = words
        }
        for reason in gapped?.reasons ?? [] { p.notes.append(PartNote(label: GapLines.label + " ", text: reason, tone: .amber)) }
        if let note = row.note { p.notes.append(PartNote(label: "Couldn't confirm: ", text: note, tone: .amber)) }
        if let why = row.why, display == "updated" { p.notes.append(PartNote(text: why.hasPrefix("Updated") ? why : "Updated: " + why, tone: .blue)) }
        if let chosen = row.chosen { p.notes.append(PartNote(text: "You chose " + chosen, tone: .dim)) }
        if let q = row.question, display == "needsYou" || display == "paused" { p.notes.append(PartNote(text: q.text, tone: .dim)) }
        p.updated = display == "updated"
        return p
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
        // a checklist line whose step runs shows its picture (FERB.app's ChecklistLineView)
        let watch = c.steps.map { s -> String? in liveRows[s.id]?.kind == "running" && !s.stepTaskId.isEmpty ? s.stepTaskId : nil }
        return ChatTask(key: "\(e):checklist", taskId: c.taskId.isEmpty ? t.taskId : c.taskId,
                        headline: c.collapsed ?? planLine ?? t.taskName ?? "", done: c.ended.count, total: c.steps.count, parts: parts, watch: watch)
    }

    /// A row's, a line's or a turn's state as the phone's marks draw it (each shape its own, never colour alone).
    public static func markOf(_ kind: String) -> MarkKind {
        switch kind {
        case "done", "updated", "gaps", "done_with_gaps", "done_unverified": return .done
        case "needsYou", "needs_you", "needs_user", "asks": return .asking
        case "failed", "notFinished", "wontRun", "stopped", "cancelled", "interrupted": return .failed
        case "waiting", "paused", "sleeping", "upcoming": return .waiting
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
            return sub(mdLink, l, "$1")
        }.joined(separator: "\n")
        return out.trimmingCharacters(in: .whitespacesAndNewlines)
    }
}
