import SwiftUI
import NotchCore

private struct TranscriptHeight: PreferenceKey {
    static var defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) { value = nextValue() }
}

private struct TranscriptContentFrame: PreferenceKey {
    static var defaultValue: CGRect = .zero
    static func reduce(value: inout CGRect, nextValue: () -> CGRect) { value = nextValue() }
}

/// Where the items a scroll request can name are, in the transcript's visible coordinates (APP-720).
struct TranscriptAnchors: PreferenceKey {
    static var defaultValue: [String: CGRect] = [:]
    static func reduce(value: inout [String: CGRect], nextValue: () -> [String: CGRect]) {
        value.merge(nextValue()) { $1 }
    }
}

extension View {
    /// An item a scroll request can bring forward: its id for the scroll view and its place for the follow rules.
    func transcriptAnchor(_ id: String) -> some View {
        self.id(id).background(GeometryReader { proxy in
            Color.clear.preference(key: TranscriptAnchors.self,
                                   value: [id: proxy.frame(in: .named("ferb-transcript-scroll"))])
        })
    }
}

/// The transcript's measurements and follow state. A reference, so scrolling never re-renders the transcript.
@MainActor
private final class TranscriptPlace {
    var follow = TranscriptFollow()
    var anchors: [String: CGRect] = [:]
}

struct ConversationTranscript: View {
    let entries: [TranscriptEntry]
    /// Open questions, shown after the newest entry; they are live state and never enter history.
    var questions: [DaemonApproval] = []
    @ObservedObject var model: NotchModel
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    /// Shown by the sticky window rather than the notch (ChatWindowView sets it).
    @Environment(\.chatInWindowHost) private var inWindow
    @State private var place = TranscriptPlace()
    @State private var hasUnreadBelow = false
    @State private var pill: TranscriptPill?

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                VStack(alignment: .leading, spacing: 10) {
                    ForEach(entries) { entry in
                        Group {
                            switch entry {
                            case let .turn(turn, answeredBelow): TurnView(turn: turn, answeredBelow: answeredBelow, model: model)
                            case let .reply(reply, turn): LateReplyView(reply: reply, turn: turn, model: model)
                            case let .notice(notice): NoticeView(notice: notice, model: model)
                            }
                        }.transcriptAnchor(TranscriptTarget.entry(entry.id))
                    }
                    ForEach(questions) { question in
                        ApprovalView(model: model, approval: question).transcriptAnchor(TranscriptTarget.question(question.id))
                    }
                    Color.clear.frame(height: 1).id("transcript-bottom")
                }
                .font(.system(size: 12))
                .padding(.top, 36)
                .padding(.bottom, 2)
                .frame(maxWidth: .infinity)
                .background(GeometryReader { proxy in
                    Color.clear
                        .preference(key: TranscriptHeight.self, value: proxy.size.height)
                        .preference(key: TranscriptContentFrame.self, value: proxy.frame(in: .named("ferb-transcript-scroll")))
                })
            }
            .coordinateSpace(name: "ferb-transcript-scroll")
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
            .onPreferenceChange(TranscriptHeight.self) { height in
                guard abs(model.measuredTranscriptHeight - height) > 1 else { return }
                model.measuredTranscriptHeight = height
                model.onBackgroundChange?(true)
            }
            .onPreferenceChange(TranscriptContentFrame.self) { frame in
                // Growth while following stays pinned to the bottom; the owner's own scrolling is remembered (APP-720).
                let viewport = model.chatTranscriptHeight
                if place.follow.observe(offset: -frame.minY, contentHeight: frame.height, viewport: viewport, at: Date()) {
                    toBottom(proxy, animated: false)
                }
                if frame.maxY <= viewport + TranscriptFollow.bottomSlack, hasUnreadBelow { hasUnreadBelow = false }
            }
            .onPreferenceChange(TranscriptAnchors.self) { anchors in
                place.anchors = anchors
                // A pill goes once its item is in view.
                if let shown = pill, let frame = anchors[shown.target],
                   frame.minY >= -1, frame.minY < model.chatTranscriptHeight - 8 { pill = nil }
            }
            .overlay(alignment: .bottomTrailing) {
                if let pill {
                    jump(pill.text, systemImage: pill.up ? "arrow.up" : "arrow.down") {
                        self.pill = nil
                        bring(pill.target, proxy, animated: !reduceMotion)
                    }
                } else if hasUnreadBelow {
                    jump("New message", systemImage: "arrow.down") {
                        hasUnreadBelow = false
                        place.follow.follow()
                        toBottom(proxy, animated: !reduceMotion)
                    }
                }
            }
            // A question the owner chose to answer is brought into view with its field.
            .onChange(of: model.questionFocus) { _, request in
                guard let request else { return }
                bring(TranscriptTarget.question(request.id), proxy, animated: !reduceMotion, anchor: .bottom)
            }
            .onChange(of: model.scrollRequests) { _, _ in consume(proxy) }
            .onChange(of: model.viewingToday) { _, today in if today { consume(proxy) } }
            .onChange(of: entries) { _, _ in followNewest(proxy) }
            .onChange(of: questions) { _, _ in followNewest(proxy) }
            .onAppear {
                // The chat moved to the other host (APP-745): back where the owner left it.
                let resume = model.transcriptResume
                model.transcriptResume = nil
                if let request = model.questionFocus { proxy.scrollTo(TranscriptTarget.question(request.id), anchor: .bottom) }
                else if let resume, !resume.following, let top = resume.topEntry {
                    place.follow.resume(following: false)
                    place.follow.scrolledByFERB(at: Date(), settle: 0.3)
                    proxy.scrollTo(top, anchor: .top)
                } else { proxy.scrollTo("transcript-bottom", anchor: .bottom) }
                consume(proxy)
            }
            .onDisappear {
                // Only when the chat moved to the other host; a day, Tasks or history switch keeps today's behaviour.
                guard model.chatInWindow != inWindow else { return }
                model.transcriptResume = TranscriptResume.at(place.anchors, following: place.follow.following)
            }
        }
    }

    private func jump(_ title: String, systemImage: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Label(title, systemImage: systemImage)
                .font(.system(size: 10, weight: .medium))
                .padding(.horizontal, 10).padding(.vertical, 6)
                .background(.regularMaterial, in: Capsule())
        }
        .buttonStyle(.plain)
        .padding(.trailing, 9).padding(.bottom, 7)
        .transition(.opacity.combined(with: .move(edge: .bottom)))
    }

    /// New entries: pinned to the bottom while following, else the "New message" control.
    private func followNewest(_ proxy: ScrollViewProxy) {
        if place.follow.following {
            toBottom(proxy, animated: false)
            hasUnreadBelow = false
        } else if pill == nil {
            hasUnreadBelow = true
        }
    }

    /// The model's scroll requests not applied yet, once each (APP-720): the most urgent one that scrolls brings its
    /// item's top into view; the others become one pill or nothing. Requests wait while another day is shown
    /// (history), then apply to today's chat.
    private func consume(_ proxy: ScrollViewProxy) {
        let pending = model.scrollRequests.filter { $0.generation > model.scrollHandled }
        guard let newest = pending.last, model.viewingToday else { return }
        model.scrollHandled = newest.generation
        // After this update's layout, so a new item has its place.
        DispatchQueue.main.async {
            var scroll: (TranscriptScrollRequest.Reason, String, Bool)?
            for request in pending {
                let frame = place.anchors[request.target]
                switch place.follow.decide(request, target: frame.map { $0.minY...max($0.minY, $0.maxY) },
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
                bring(target, proxy, animated: animated)
            }
        }
    }

    private func bring(_ target: String, _ proxy: ScrollViewProxy, animated: Bool, anchor: UnitPoint = .top) {
        place.follow.scrolledByFERB(at: Date(), settle: animated ? 0.6 : 0.15)
        withAnimation(animated ? .spring(duration: 0.4, bounce: 0) : nil) { proxy.scrollTo(target, anchor: anchor) }
    }

    private func toBottom(_ proxy: ScrollViewProxy, animated: Bool) {
        place.follow.scrolledByFERB(at: Date(), settle: animated ? 0.3 : 0.1)
        withAnimation(animated ? .easeOut(duration: 0.18) : nil) { proxy.scrollTo("transcript-bottom", anchor: .bottom) }
    }
}

/// The owner's request, then FERB's substantive plan, connection line, question and answer.
/// Canned transport acknowledgements stay archived but are not presented (owner, 2026-10-29). Ordinary steps
/// never reach the live line (owner, 2026-10-24: only the "N active tasks" row stays; DaemonConnection's `progress`).
/// Several asks (ORCH-319): the first reply's bubble, then the card, whose rows and footer are the answer.
private struct TurnView: View {
    let turn: ConversationTurn
    let answeredBelow: Bool
    @ObservedObject var model: NotchModel

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            if turn.preview {
                // The intro's suggested task (kb area app): how FERB would do it, nothing done.
                Label("Preview · nothing here is done", systemImage: "eye")
                    .font(.system(size: 10.5, weight: .medium)).foregroundStyle(Palette.secondary)
            }
            RequestView(turn: turn)
            if let lead = turn.mergedInto {
                let turns = model.conversation.turns
                let leadIndex = turns.firstIndex { $0.requestID == lead }
                MergedLabel(lead: leadIndex.map { turns[$0] },
                            leadIsAbove: (leadIndex ?? 0) < (turns.firstIndex { $0.id == turn.id } ?? 0))
            }
            if turn.showsTyping { TypingDots() }
            FirstReplyView(turn: turn)
            if let cards = turn.receiptCards { ResultCardsView(cards: cards, turnID: turn.id, model: model) }
            if let card = turn.card {
                TurnCardView(card: card, turn: turn, model: model)
                // Step 8e: the task's cards (payments, sends, links, files) under its card's footer.
                if let cards = turn.cards { ResultCardsView(cards: cards, turnID: turn.id, model: model) }
            }
            ForEach(Array((turn.parts ?? []).enumerated()), id: \.offset) { _, part in    // parts saved before cards
                FerbBubble { MarkdownContent(part).equatable() }
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
                AnswerView(turn: turn, model: model)
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
            }
            // A secret FERB hid in the message (task 11): code's reminder under it
            if let notice = turn.hiddenNotice {
                HStack {
                    Spacer(minLength: 28)
                    Label(notice, systemImage: "eye.slash")
                        .font(.system(size: 10)).foregroundStyle(Palette.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
    }
}

/// A late entry (L6): a one-line quote of its request, then FERB's question or the answer.
private struct LateReplyView: View {
    let reply: ConversationReply
    let turn: ConversationTurn
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
                    AnswerView(turn: turn, model: model)
                }
            } else {
                AnswerView(turn: turn, model: model)
            }
        }
    }
}

/// A background message, with typed reminder cards above it and attributed outside text kept as a quote.
private struct NoticeView: View {
    let notice: ConversationNotice
    @ObservedObject var model: NotchModel

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            ForEach(Array(CardLayout.above(notice.cards ?? []).enumerated()), id: \.offset) { _, card in
                ReminderCard(card: card).padding(.trailing, 28)
            }
            // A plain timer's entire message is already the reminder card's title.
            if !(notice.cards?.count == 1 && notice.cards?.first?.type == "reminder"
                 && notice.cards?.first?.title == notice.text && notice.quotes.isEmpty
                 && (notice.proposals ?? []).isEmpty) {
                message
            }
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
                    .textFieldStyle(.plain).padding(6)
                    .background(.white.opacity(0.07), in: RoundedRectangle(cornerRadius: 8))
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

/// Acknowledgements appear as soon as the daemon delivers them.
struct QuickReplyBubble: View {
    let text: String
    var body: some View {
        NativeMessageText(text, size: 11, secondary: true)
            .padding(.horizontal, 9).padding(.vertical, 4)
            .background(.white.opacity(0.07), in: Capsule())
    }
}

private struct AnswerView: View {
    let turn: ConversationTurn
    @ObservedObject var model: NotchModel

    /// The answer's own lines drawn on their own (ORCH-319), cut from its text.
    private var split: AnswerParts {
        AnswerParts(answer: turn.visibleAnswer, note: turn.note, lines: turn.codeLines ?? [])
    }
    private var codeSites: String? {
        split.lines.first { $0.hasPrefix("Sites contacted:") }.map {
            $0.dropFirst("Sites contacted:".count).trimmingCharacters(in: .whitespaces)
        }
    }
    private var visibleParts: SitesContactedParts { SitesContactedParts(split.body) }
    private var fullParts: SitesContactedParts { SitesContactedParts(turn.answer) }
    private var displayedSites: String { codeSites ?? (visibleParts.sites.isEmpty ? fullParts.sites : visibleParts.sites) }

    private var codeLines: [String] {
        CardLayout.remaining(split.lines.filter { !$0.hasPrefix("Sites contacted:") }, cards)
    }
    /// A collapsed answer's own lines ("Couldn't confirm", code's lines) stay in view under the fade.
    private var collapsedLines: AnyView? {
        guard split.note != nil || !codeLines.isEmpty else { return nil }
        return AnyView(AnswerLinesView(note: split.note, lines: codeLines))
    }

    /// Typed cards (step 8e): each drawn in place of the line it stands for; a turn saved before cards has none.
    private var cards: [ResultCard] { turn.cards ?? [] }
    private var cardFiles: Set<String> { Set(CardLayout.files(cards).compactMap(\.path)) }

    var body: some View {
        // A card's rows and footer are the answer; its text is kept for history, voice and older clients.
        if !turn.answer.isEmpty && turn.card == nil {
            VStack(alignment: .leading, spacing: 4) {
                ForEach(Array(CardLayout.above(cards).enumerated()), id: \.offset) { _, card in
                    ReminderCard(card: card).padding(.trailing, 28)       // a reminder: FERB's text under it
                }
                FerbBubble {
                    VStack(alignment: .leading, spacing: 6) {
                        // An answer taller than the chat area shows about half of it, the rest on Show all: the
                        // same text (APP-719, APP-725). The threshold follows the height the owner set (APP-587).
                        CollapsibleText(key: ExpansionState.key(turn: turn.id, item: "answer"),
                                        kind: .answer(viewport: model.preferredTranscriptHeight),
                                        target: TranscriptTarget.answer(turn: turn.id),
                                        more: "Show all", model: model, whenCollapsed: collapsedLines) {
                            MarkdownContent(CardLayout.strip(visibleParts.answer, cards),
                                            note: split.note, lines: codeLines).equatable()
                        }
                        if turn.displayText != nil && turn.visibleAnswer != turn.answer {
                            DisclosureGroup("Details") {
                                MarkdownContent(fullParts.answer).equatable()
                                    .frame(maxWidth: .infinity, alignment: .leading)
                            }.font(.system(size: 11))
                        }
                    }
                }
                ResultCardsView(cards: cards, turnID: turn.id, model: model)
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
