import SwiftUI
import NotchCore

extension View {
    /// An item a scroll request can bring forward: a marker view the transcript list finds it by once its row is built
    /// (APP-806, APP-815: a nested item is brought by its own place).
    func transcriptAnchor(_ id: String) -> some View {
        self.id(id).background(AnchorMark(id: id))
    }
}

/// The transcript's built items by id, as their marker views registered them.
@MainActor
final class AnchorMarks {
    private var views: [String: Weak] = [:]
    private struct Weak { weak var view: NSView? }

    func register(_ id: String, _ view: NSView) {
        views[id] = Weak(view: view)
        if views.count > 400 { views = views.filter { $0.value.view?.window != nil } }
    }
    func view(_ id: String) -> NSView? { views[id]?.view.flatMap { $0.window != nil ? $0 : nil } }
}

private struct AnchorMarksKey: EnvironmentKey {
    static let defaultValue: AnchorMarks? = nil
}
extension EnvironmentValues {
    var transcriptMarks: AnchorMarks? {
        get { self[AnchorMarksKey.self] }
        set { self[AnchorMarksKey.self] = newValue }
    }
}

/// A view as large as the item it marks, registered under the item's id.
private struct AnchorMark: NSViewRepresentable {
    let id: String
    @Environment(\.transcriptMarks) private var marks

    final class Mark: NSView {
        override func hitTest(_ point: NSPoint) -> NSView? { nil }
        override func isAccessibilityElement() -> Bool { false }
    }

    func makeNSView(context: Context) -> Mark { Mark() }
    func updateNSView(_ view: Mark, context: Context) { marks?.register(id, view) }
}

/// The transcript's follow state and its list. A reference, so scrolling never re-renders the transcript.
@MainActor
private final class TranscriptPlace {
    var follow = TranscriptFollow()
    /// When FERB last moved the view itself (a newest entry, an item brought forward): earlier entries come in only
    /// after the owner scrolled since (APP-806).
    var ferbScrolledAt = Date.distantPast
    let marks = AnchorMarks()
    lazy var list = TranscriptList(marks: marks)
}

/// Whether the shown day has entries before the first one shown, and whether they are on their way (APP-806).
struct EarlierEntries: Equatable {
    var more = false
    var loading = false
}

/// The top of a day while the daemon's page of earlier entries is on its way (APP-806): a small loader.
private struct EarlierLoader: View {
    var body: some View {
        HStack {
            Spacer()
            ProgressView().controlSize(.small).accessibilityLabel("Loading earlier messages")
            Spacer()
        }
        .frame(height: 24)
    }
}

struct ConversationTranscript: View {
    let entries: [TranscriptEntry]
    /// Earlier entries of the shown day (APP-806).
    var earlier = EarlierEntries()
    /// Open questions, shown after the newest entry; they are live state and never enter history.
    var questions: [DaemonApproval] = []
    @ObservedObject var model: NotchModel
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    /// Shown by the sticky window rather than the notch (ChatWindowView sets it).
    @Environment(\.chatInWindowHost) private var inWindow
    @State private var place = TranscriptPlace()
    @State private var hasUnreadBelow = false
    @State private var pill: TranscriptPill?

    /// The list's rows: the space under the fade, the loader while an earlier page comes, the entries, the questions.
    private var rows: [TranscriptRow] {
        // One box per timer, reminder and schedule: drawn only at its last entry (APP-769)
        let moved = CardBoxes.moved(entries)
        return [.top] + (earlier.loading ? [.loader] : [])
            + entries.map { .entry($0, hidden: moved[$0.id] ?? []) }
            + questions.map { .question($0) } + [.bottom]
    }

    var body: some View {
        // An AppKit list (APP-815): every row's height is known before a frame is drawn, so the view can go anywhere at
        // once (the scroll thumb dragged to either end) and shows what is there; rows are built only near the view
        // (owner 2026-11-09: "when i pressed on 8nov(prev day) - it is laggggin sooooo much"; "i still saw blank screen
        // whiel usign teh scroll indicato to go fastly to top or bottom").
        TranscriptListView(list: place.list, rows: rows, viewport: model.chatTranscriptHeight,
                           content: { [model] row in Self.content(row, model: model) },
                           scrolled: { offset, height in scrolled(offset, height) })
            .frame(height: model.chatTranscriptHeight)
            .mask {
                // Fade the content itself into the existing native glass. A material
                // overlay here creates a visible gray rectangle over the messages.
                VStack(spacing: 0) {
                    LinearGradient(stops: [
                        .init(color: .clear, location: 0),
                        .init(color: .black.opacity(0.08), location: 0.15),
                        .init(color: .black.opacity(0.35), location: 0.38),
                        .init(color: .black.opacity(0.72), location: 0.65),
                        .init(color: .black.opacity(0.94), location: 0.85),
                        .init(color: .black, location: 1)
                    ], startPoint: .top, endPoint: .bottom)
                    .frame(height: 36)
                    Rectangle().fill(.black)
                }
            }
            .overlay(alignment: .bottomTrailing) {
                if let pill {
                    jump(pill.text, systemImage: pill.up ? "arrow.up" : "arrow.down") {
                        self.pill = nil
                        bring(pill.target, animated: !reduceMotion)
                    }
                } else if hasUnreadBelow {
                    jump("New message", systemImage: "arrow.down") {
                        hasUnreadBelow = false
                        place.follow.follow()
                        toBottom(animated: !reduceMotion)
                    }
                }
            }
            // A question the owner chose to answer is brought into view with its field.
            .onChange(of: model.questionFocus) { _, request in
                guard let request else { return }
                bring(TranscriptTarget.question(request.id), animated: !reduceMotion, anchor: .bottom)
            }
            .onChange(of: model.scrollRequests) { _, _ in consume() }
            .onChange(of: model.viewingToday) { _, today in if today { consume() } }
            .onChange(of: entries) { old, new in
                // Earlier entries coming in above (APP-806) are no new message: the view stays where the owner is
                if new.count > old.count, new.last?.id == old.last?.id, new.first?.id != old.first?.id,
                   new.suffix(old.count).map(\.id) == old.map(\.id) { return }
                followNewest()
            }
            .onChange(of: questions) { _, _ in followNewest() }
            .onAppear {
                // The chat moved to the other host (APP-745): back where the owner left it.
                let resume = model.transcriptResume
                model.transcriptResume = nil
                if let request = model.questionFocus {
                    place.list.reveal(TranscriptTarget.question(request.id), holder: nil, anchor: .bottom, animated: false)
                } else if let resume, !resume.following, let top = resume.topEntry {
                    place.follow.resume(following: false)
                    place.follow.scrolledByFERB(at: Date(), settle: 0.3)
                    place.list.reveal(top, holder: nil, anchor: .top, animated: false)
                } else {
                    place.list.toEnd(animated: false)
                }
                consume()
            }
            .onDisappear {
                // Only when the chat moved to the other host; a day, Tasks or history switch keeps today's behaviour.
                guard model.chatInWindow != inWindow else { return }
                model.transcriptResume = TranscriptResume(following: place.follow.following,
                                                          topEntry: place.follow.following ? nil : place.list.topEntry())
            }
    }

    /// The list moved or its content changed (APP-720): the follow rules, the earlier page near the top (APP-806), the
    /// pills.
    private func scrolled(_ offset: CGFloat, _ height: CGFloat) {
        let viewport = model.chatTranscriptHeight
        if place.follow.observe(offset: offset, contentHeight: height, viewport: viewport, at: Date()) {
            toBottom(animated: false)
        }
        // Within a screen of the top: earlier entries come in (APP-806)
        // (only once the owner scrolled, or when the day so far doesn't fill two screens: never as it opens)
        if offset < viewport, earlier.more, !earlier.loading,
           (place.follow.ownerScrolledAt ?? .distantPast) > place.ferbScrolledAt || height < 2 * viewport {
            model.loadEarlier()
        }
        if offset + viewport >= height - TranscriptFollow.bottomSlack, hasUnreadBelow { hasUnreadBelow = false }
        // A pill goes once its item is in view.
        if let shown = pill, let frame = frame(of: shown.target), frame.minY >= -1, frame.minY < viewport - 8 { pill = nil }
    }

    private func frame(of target: String) -> CGRect? {
        place.list.frame(of: target, holder: place.list.holder(of: target))
    }

    /// One row of the list, as the transcript draws it.
    @MainActor
    private static func content(_ row: TranscriptRow, model: NotchModel) -> AnyView {
        switch row {
        case .top, .bottom: return AnyView(Color.clear)
        case .loader: return AnyView(EarlierLoader())
        case let .question(question):
            return AnyView(ApprovalView(model: model, approval: question)
                .frame(maxWidth: ChatSide.width, alignment: .leading)
                .transcriptAnchor(TranscriptTarget.question(question.id)))
        case let .entry(entry, hidden):
            let shown: AnyView
            switch entry {
            case let .turn(turn, answeredBelow):
                shown = AnyView(TurnView(turn: turn, answeredBelow: answeredBelow, hidden: hidden, model: model))
            case let .reply(reply, turn):
                shown = AnyView(LateReplyView(reply: reply, turn: turn, hidden: hidden, model: model)
                    .frame(maxWidth: ChatSide.width, alignment: .leading))
            case let .notice(notice):
                shown = AnyView(NoticeView(notice: notice, hidden: hidden, model: model)
                    .frame(maxWidth: ChatSide.width, alignment: .leading))
            }
            // Another day's chat is a record: its cards draw without live state (APP-769)
            return AnyView(shown.transcriptAnchor(TranscriptTarget.entry(entry.id))
                .environment(\.cardsLive, model.viewingToday))
        }
    }

    private func jump(_ title: String, systemImage: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Label(title, systemImage: systemImage)
                .font(.system(size: 10, weight: .medium))
                .padding(.horizontal, 10).padding(.vertical, 6)
                .ferbGlass(in: Capsule())                                  // APP-785
        }
        .buttonStyle(.plain)
        .padding(.trailing, 9).padding(.bottom, 7)
        .transition(.opacity.combined(with: .move(edge: .bottom)))
    }

    /// New entries: pinned to the bottom while following, else the "New message" control.
    private func followNewest() {
        if place.follow.following {
            toBottom(animated: false)
            hasUnreadBelow = false
        } else if pill == nil {
            hasUnreadBelow = true
        }
    }

    /// The model's scroll requests not applied yet, once each (APP-720): the most urgent one that scrolls brings its
    /// item's top into view; the others become one pill or nothing. Requests wait while another day is shown
    /// (history), then apply to today's chat.
    private func consume() {
        let pending = model.scrollRequests.filter { $0.generation > model.scrollHandled }
        guard let newest = pending.last, model.viewingToday else { return }
        model.scrollHandled = newest.generation
        // After this update's rows are in the list, so a new item has its place.
        DispatchQueue.main.async {
            var scroll: (TranscriptScrollRequest.Reason, String, Bool)?
            for request in pending {
                let target = frame(of: request.target).map { $0.minY...max($0.minY, $0.maxY) }
                switch place.follow.decide(request, target: target,
                                           viewport: model.chatTranscriptHeight, reduceMotion: reduceMotion, at: Date()) {
                case .none:
                    break
                case let .scroll(target, animated):
                    if scroll.map({ request.reason >= $0.0 }) ?? true { scroll = (request.reason, target, animated) }
                case let .pill(shown):
                    pill = TranscriptPill.merged(pill, shown)
                    hasUnreadBelow = false
                }
            }
            if let (_, target, animated) = scroll {
                if pill?.target == target { pill = nil }
                bring(target, animated: animated)
            }
        }
    }

    private func bring(_ target: String, animated: Bool, anchor: UnitPoint = .top) {
        place.follow.scrolledByFERB(at: Date(), settle: animated ? 0.6 : 0.15)
        place.ferbScrolledAt = Date().addingTimeInterval(animated ? 0.6 : 0.15)
        place.list.reveal(target, holder: place.list.holder(of: target), anchor: anchor, animated: animated)
    }

    private func toBottom(animated: Bool) {
        place.follow.scrolledByFERB(at: Date(), settle: animated ? 0.3 : 0.1)
        place.ferbScrolledAt = Date().addingTimeInterval(animated ? 0.3 : 0.1)
        place.list.toEnd(animated: animated)
    }
}

/// The owner's request, then FERB's substantive plan, connection line, question and answer.
/// Canned transport acknowledgements stay archived but are not presented (owner, 2026-10-29). Ordinary steps
/// never reach the live line (owner, 2026-10-24: only the "N active tasks" row stays; DaemonConnection's `progress`).
/// Several asks (ORCH-319): the first reply's bubble, then the card, whose rows and footer are the answer.
/// How wide a message gets (owner 2026-11-09: "the space between ferb res and my msg shodl increase with reply boxes adn
/// ohther boxes not affected"): what it has in the chat at its standard width (440 pt less the chat's 25 pt sides). A
/// wider chat keeps FERB's messages on the left and the owner's on the right at this size, the extra width between them.
enum ChatSide {
    static let width: CGFloat = ChatWindowFrames.standardSize.width - 50
}

private struct TurnView: View {
    let turn: ConversationTurn
    let answeredBelow: Bool
    /// The boxes a later entry draws (APP-769).
    var hidden: Set<String> = []
    @ObservedObject var model: NotchModel

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            if turn.preview {
                // The intro's suggested task (kb area app): how FERB would do it, nothing done.
                Label("Preview · nothing here is done", systemImage: "eye")
                    .font(.system(size: 10.5, weight: .medium)).foregroundStyle(Palette.secondary)
            }
            RequestView(turn: turn)
            ferbSide.frame(maxWidth: ChatSide.width, alignment: .leading)
        }
    }

    /// FERB's side of the turn, at most a message's width (ChatSide).
    private var ferbSide: some View {
        VStack(alignment: .leading, spacing: 7) {
            if let lead = turn.mergedInto {
                let turns = model.conversation.turns
                let leadIndex = turns.firstIndex { $0.requestID == lead }
                MergedLabel(lead: leadIndex.map { turns[$0] },
                            leadIsAbove: (leadIndex ?? 0) < (turns.firstIndex { $0.id == turn.id } ?? 0))
            }
            if turn.showsTyping { TypingDots() }
            FirstReplyView(turn: turn)
            if let cards = turn.receiptCards {
                ResultCardsView(cards: CardBoxes.shown(cards, hiding: hidden), turnID: turn.id, model: model)
            }
            if let checklist = turn.checklist, turn.card == nil {
                StepChecklistView(checklist: checklist, turn: turn, model: model)      // APP-894
            }
            if let card = turn.card {
                TurnCardView(card: card, turn: turn, model: model)
                // Step 8e: the task's cards (payments, sends, links, files) under its card's footer.
                if let cards = turn.cards {
                    ResultCardsView(cards: CardBoxes.shown(cards, hiding: hidden), turnID: turn.id, model: model)
                }
            }
            ForEach(Array((turn.parts ?? []).enumerated()), id: \.offset) { _, part in    // parts saved before cards
                if !part.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                    FerbBubble { MarkdownContent(part).equatable() }
                }
            }
            if let live = turn.liveLine { LiveLineView(progress: live) }
            // A question FERB asked stays here unless it was itself shown below.
            if let question = turn.question,
               !model.conversation.repliesBelow(turn.id).contains(where: { $0.part == .question }) {
                FerbBubble { MarkdownContent(question).equatable() }
            }
            if answeredBelow {
                Text("Answered below").font(.system(size: 10)).foregroundStyle(Palette.secondary)
            } else {
                AnswerView(turn: turn, hidden: hidden, model: model)
            }
        }
    }
}

/// The owner's message on the right. A task this chat didn't send has no owner message: its goal is FERB's
/// description, shown on FERB's side.
private struct RequestView: View {
    let turn: ConversationTurn

    var body: some View {
        if let goal = turn.taskGoal {
            Label(goal.isEmpty ? "A task from earlier" : goal, systemImage: "arrow.triangle.2.circlepath")
                .font(.system(size: 11)).foregroundStyle(Palette.secondary).lineLimit(2)
                .help("A task this chat didn’t send")
                .frame(maxWidth: ChatSide.width, alignment: .leading)
        } else {
            HStack {
                Spacer(minLength: 28)
                VStack(alignment: .leading, spacing: 5) {
                    if !turn.request.isEmpty { NativeMessageText(turn.request) }     // files sent without words
                    ForEach(turn.sources, id: \.self) { url in
                        Label(url.lastPathComponent, systemImage: "paperclip").font(.system(size: 10))
                    }
                }
                .padding(.horizontal, 11).padding(.vertical, 8)
                .background(Palette.chatBlue, in: RoundedRectangle(cornerRadius: 15))
                .help(turn.createdAt.formatted(date: .omitted, time: .shortened))
                .frame(maxWidth: ChatSide.width - 28, alignment: .trailing)     // as wide as at the standard width
            }
            // A secret FERB hid in the message (task 11): code's reminder under it
            if let notice = turn.hiddenNotice {
                HStack {
                    Spacer(minLength: 28)
                    Label(notice, systemImage: "eye.slash")
                        .font(.system(size: 10)).foregroundStyle(Palette.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                        .frame(maxWidth: ChatSide.width - 28, alignment: .trailing)
                }
            }
        }
    }
}

/// A late entry (L6): a one-line quote of its request, then FERB's question or the answer.
private struct LateReplyView: View {
    let reply: ConversationReply
    let turn: ConversationTurn
    var hidden: Set<String> = []
    @ObservedObject var model: NotchModel

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            Label {
                Text(turn.quote.isEmpty ? "A task from earlier" : turn.quote).lineLimit(1).truncationMode(.tail)
            } icon: {
                Image(systemName: "arrowshape.turn.up.left")
            }
            .font(.system(size: 10)).foregroundStyle(Palette.secondary)
            .help(turn.taskGoal ?? turn.request)
            .accessibilityLabel("Reply to: " + (turn.quote.isEmpty ? "a task from earlier" : turn.quote))
            if reply.part == .question, let question = turn.question {
                // The question was shown here; the final answer follows it unless it went further below.
                FerbBubble { MarkdownContent(question).equatable() }
                if !model.conversation.repliesBelow(turn.id).contains(where: { $0.part == .answer }) {
                    AnswerView(turn: turn, hidden: hidden, model: model)
                }
            } else {
                AnswerView(turn: turn, hidden: hidden, model: model)
            }
        }
    }
}

/// A background message, with typed reminder cards above it and attributed outside text kept as a quote. A timer's or
/// reminder's arrival is its one box (APP-769): no bubble repeats it, and a reminder's own words show inside the box.
private struct NoticeView: View {
    let notice: ConversationNotice
    var hidden: Set<String> = []
    @ObservedObject var model: NotchModel

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            let boxSays = CardBoxes.boxSays(notice)
            ForEach(Array(CardLayout.above(CardBoxes.shown(notice.cards ?? [], hiding: hidden)).enumerated()),
                    id: \.offset) { _, card in
                ReminderCard(card: card, words: CardBoxes.words(notice), model: model).padding(.trailing, 28)
            }
            if !boxSays { message }
        }
        .help((notice.noticedAt ?? notice.createdAt).formatted(date: .omitted, time: .shortened))
    }

    private var message: some View {
        FerbBubble {
            VStack(alignment: .leading, spacing: 6) {
                NativeMessageText(notice.text)
                // What FERB learned, each confirmed or turned down here.
                ForEach(notice.proposals ?? [], id: \.id) { proposal in
                    if proposal.kind == "guess" {
                        GuessRow(notice: notice, proposal: proposal, model: model)
                    } else {
                        VStack(alignment: .leading, spacing: 5) {
                            NativeMessageText(proposal.text)
                            if let decision = proposal.decision {
                                Text(decision == "confirmed" ? "Confirmed" : "Marked not right")
                                    .font(.system(size: 10)).foregroundStyle(Palette.secondary)
                            } else {
                                HStack(spacing: 8) {
                                    Button("Confirm") { model.decideLearning(noticeID: notice.id, proposalID: proposal.id, confirm: true) }
                                        .buttonStyle(FerbButtonStyle())
                                    Button("Not right") { model.decideLearning(noticeID: notice.id, proposalID: proposal.id, confirm: false) }
                                        .buttonStyle(FerbButtonStyle(quiet: true))
                                }
                            }
                        }
                        .padding(.top, 2)
                    }
                }
                // Step 12c: its cards (an import's examples as note cards) and a button for each page it names.
                ForEach(Array((notice.cards ?? []).filter { $0.type == "note" }.enumerated()), id: \.offset) { _, card in
                    NoteCard(card: card, model: model)
                }
                ForEach(Array(CardLayout.pages(notice.cards ?? []).enumerated()), id: \.offset) { _, card in
                    if card.page == "memory" {
                        Button(card.line ?? "Open the Memory page") { model.onOpenMemory?(card.importedFrom) }
                            .buttonStyle(FerbButtonStyle())
                            .pointingHand("page")
                    }
                }
                ForEach(Array(notice.quotes.enumerated()), id: \.offset) { _, quote in
                    VStack(alignment: .leading, spacing: 2) {
                        NativeMessageText(quote.text, size: 11)
                        if !quote.origin.isEmpty {
                            Text(quote.origin).font(.system(size: 10)).foregroundStyle(Palette.secondary)
                        }
                    }
                    .padding(.leading, 8)
                    .overlay(alignment: .leading) { Capsule().fill(Palette.secondary.opacity(0.5)).frame(width: 2) }
                    .accessibilityLabel("Quote from \(quote.origin): \(quote.text)")
                }
            }
        }
        .help((notice.noticedAt ?? notice.createdAt).formatted(date: .omitted, time: .shortened))
    }
}

/// One of FERB's older guesses (L36): Keep, Drop, or the owner's own words saved instead.
private struct GuessRow: View {
    let notice: ConversationNotice
    let proposal: LearningProposal
    @ObservedObject var model: NotchModel
    @State private var words = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            NativeMessageText(proposal.text)
            if let decision = proposal.decision {
                Text(["kept": "Kept", "dropped": "Dropped", "edited": "Saved in your words"][decision] ?? decision)
                    .font(.system(size: 10)).foregroundStyle(Palette.secondary)
            } else {
                HStack(spacing: 8) {
                    Button("Keep") {
                        model.decideGuess(noticeID: notice.id, proposalID: proposal.id, action: "keep", text: nil)
                    }.buttonStyle(FerbButtonStyle())
                    Button("Drop") {
                        model.decideGuess(noticeID: notice.id, proposalID: proposal.id, action: "drop", text: nil)
                    }.buttonStyle(FerbButtonStyle(quiet: true))
                }
                TextField("Or type it in your own words…", text: $words)
                    .ferbField(inset: 6)
                    .onSubmit {
                        let typed = words.trimmingCharacters(in: .whitespacesAndNewlines)
                        guard !typed.isEmpty else { return }
                        model.decideGuess(noticeID: notice.id, proposalID: proposal.id, action: "edit", text: typed) { ok in
                            if ok { words = "" }       // a refused edit keeps the typed words to fix
                        }
                    }
            }
        }
        .padding(.top, 2)
    }
}

/// A connection line (offline, reconnected) and its elapsed time, replaced in place and removed when the answer
/// arrives (L8, fix wave 7).
private struct LiveLineView: View {
    let progress: LiveProgress

    var body: some View {
        // Ticks on whole elapsed seconds; a start time ahead of this Mac's clock ticks from now.
        TimelineView(.periodic(from: min(progress.startedAt, Date()), by: 1)) { context in
            Text(progress.text + " · " + progress.elapsedText(at: context.date))
                .lineLimit(1).truncationMode(.tail)
                .font(.system(size: 11)).foregroundStyle(Palette.secondary)
        }
        .accessibilityLabel("Connection: " + progress.text)
    }
}

private struct AnswerView: View {
    let turn: ConversationTurn
    /// The boxes a later entry draws (APP-769): their cards aren't drawn here; their lines still leave the text.
    var hidden: Set<String> = []
    @ObservedObject var model: NotchModel

    /// What the body draws, worked out from the turn once per body rather than once per read (each read parsed the
    /// whole answer again; a long chat redraws many answers).
    private struct Parts {
        /// The answer's own lines drawn on their own (ORCH-319), cut from its text.
        let split: AnswerParts
        let visibleParts: SitesContactedParts
        let fullParts: SitesContactedParts
        let displayedSites: String
        let codeLines: [String]
        /// Typed cards (step 8e): each drawn in place of the line it stands for; a turn saved before cards has none.
        let cards: [ResultCard]
        let drawn: [ResultCard]
        let cardFiles: Set<String>

        init(turn: ConversationTurn, hidden: Set<String>) {
            split = AnswerParts(answer: turn.visibleAnswer, note: turn.note, lines: turn.codeLines ?? [])
            let codeSites = split.lines.first { $0.hasPrefix("Sites contacted:") }.map {
                $0.dropFirst("Sites contacted:".count).trimmingCharacters(in: .whitespaces)
            }
            visibleParts = SitesContactedParts(split.body)
            fullParts = SitesContactedParts(turn.answer)
            displayedSites = codeSites ?? (visibleParts.sites.isEmpty ? fullParts.sites : visibleParts.sites)
            cards = turn.cards ?? []
            drawn = CardBoxes.shown(cards, hiding: hidden)
            cardFiles = Set(CardLayout.files(cards).compactMap(\.path))
            codeLines = CardLayout.remaining(split.lines.filter { !$0.hasPrefix("Sites contacted:") }, cards)
        }

        /// A collapsed answer's own lines ("Couldn't confirm", code's lines) stay in view under the fade.
        var collapsedLines: NSAttributedString? {
            guard split.note != nil || !codeLines.isEmpty else { return nil }
            return MarkdownTextAttributes.annotations(note: split.note, lines: codeLines)
        }
    }

    var body: some View {
        let parts = Parts(turn: turn, hidden: hidden)
        let split = parts.split, visibleParts = parts.visibleParts, fullParts = parts.fullParts
        let displayedSites = parts.displayedSites, codeLines = parts.codeLines, cards = parts.cards
        let drawn = parts.drawn, cardFiles = parts.cardFiles, collapsedLines = parts.collapsedLines
        // A card's rows and footer are the answer; its text is kept for history, voice and older clients.
        if !turn.answer.isEmpty && turn.card == nil {
            VStack(alignment: .leading, spacing: 4) {
                ForEach(Array(CardLayout.above(drawn).enumerated()), id: \.offset) { _, card in
                    ReminderCard(card: card, model: model).padding(.trailing, 28)  // a reminder: FERB's text under it
                }
                // Nothing left once the cards say their lines (a timer's "Timer set"): no empty bubble
                let details = turn.displayText != nil && turn.visibleAnswer != turn.answer
                let shown = CardLayout.bubble(visibleParts.answer, cards, note: split.note, lines: codeLines)
                if shown != nil || details {
                    FerbBubble {
                        VStack(alignment: .leading, spacing: 6) {
                            if let shown {
                                // An answer taller than the chat area shows about half of it, the rest on Show all:
                                // the same text (APP-719, APP-725). The threshold follows the height the owner set
                                // (APP-587).
                                CollapsibleText(key: ExpansionState.key(turn: turn.id, item: "answer"),
                                                kind: .answer(viewport: model.preferredTranscriptHeight),
                                                target: TranscriptTarget.answer(turn: turn.id),
                                                more: "Show all", model: model, whenCollapsed: collapsedLines) {
                                    MarkdownContent(shown, note: split.note, lines: codeLines).equatable()
                                }
                            }
                            if details {
                                DisclosureGroup("Details") {
                                    MarkdownContent(fullParts.answer).equatable()
                                        .frame(maxWidth: .infinity, alignment: .leading)
                                }.font(.system(size: 11))
                            }
                        }
                    }
                }
                ResultCardsView(cards: drawn, turnID: turn.id, model: model)
                if !displayedSites.isEmpty && CardLayout.links(cards).isEmpty {     // a turn from before link cards
                    VisitedSitesView(sites: VisitedSites(displayedSites), model: model)
                }
            }
            .transcriptAnchor(TranscriptTarget.answer(turn: turn.id))
        }
        if turn.isProvisional {
            Text("Draft · still responding")
                .font(.system(size: 10)).foregroundStyle(Palette.secondary)
                .accessibilityLabel("Provisional response. FERB is still responding.")
        }
        if let notice = turn.responseNotice {
            Text(notice).font(.system(size: 10)).foregroundStyle(Palette.secondary)
        }
        if turn.responseNotice == nil && !["done", "working", "merged"].contains(turn.status) && turn.card == nil
            && !turn.preview {
            Text(turn.status.replacingOccurrences(of: "_", with: " ").capitalized)
                .font(.system(size: 10)).foregroundStyle(Palette.secondary)
        }
        if turn.offersReport { ReportRow(turn: turn, model: model) }
        if turn.status == "failed", let offer = turn.runtimeSwitch { RuntimeSwitchRow(turn: turn, offer: offer, model: model) }
        ForEach(turn.artifacts.filter { !cardFiles.contains($0.path) }, id: \.self) { url in   // not drawn as a chip
            ArtifactRow(url: url, model: model)
        }
    }
}

/// A neutral FERB message on the left.
struct FerbBubble<Content: View>: View {
    @ViewBuilder let content: Content

    var body: some View {
        bubble
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.trailing, 28)
    }

    var bubble: some View {
        content
            .padding(.horizontal, 11).padding(.vertical, 8)
            .background(.white.opacity(0.10), in: RoundedRectangle(cornerRadius: 15))
    }
}

private struct SitesContactedParts {
    let answer: String
    let sites: String

    init(_ source: String) {
        guard let range = source.range(of: #"(?m)^Sites contacted:\s*(.+)$"#, options: .regularExpression) else {
            answer = source
            sites = ""
            return
        }
        let line = String(source[range])
        sites = line.replacingOccurrences(of: #"^Sites contacted:\s*"#, with: "",
                                          options: .regularExpression).trimmingCharacters(in: .whitespacesAndNewlines)
        answer = String(source[..<range.lowerBound]).trimmingCharacters(in: .whitespacesAndNewlines)
    }
}
