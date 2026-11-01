import SwiftUI
import NotchCore

private struct TranscriptHeight: PreferenceKey {
    static var defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) { value = nextValue() }
}

private struct TranscriptContentBottom: PreferenceKey {
    static var defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) { value = nextValue() }
}

struct ConversationTranscript: View {
    let entries: [TranscriptEntry]
    /// Open questions, shown after the newest entry; they are live state and never enter history.
    var questions: [DaemonApproval] = []
    @ObservedObject var model: NotchModel
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var followsLatest = true
    @State private var hasUnreadBelow = false

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
                        }.id(entry.id)
                    }
                    ForEach(questions) { question in
                        ApprovalView(model: model, approval: question).id("question-" + question.id)
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
                        .preference(key: TranscriptContentBottom.self,
                                    value: proxy.frame(in: .named("ferb-transcript-scroll")).maxY)
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
            .onPreferenceChange(TranscriptContentBottom.self) { bottom in
                let atBottom = bottom <= model.chatTranscriptHeight + 28
                followsLatest = atBottom
                if atBottom { hasUnreadBelow = false }
            }
            .overlay(alignment: .bottomTrailing) {
                if hasUnreadBelow {
                    Button {
                        hasUnreadBelow = false
                        followsLatest = true
                        withAnimation(reduceMotion ? nil : .easeOut(duration: 0.18)) {
                            proxy.scrollTo("transcript-bottom", anchor: .bottom)
                        }
                    } label: {
                        Label("New message", systemImage: "arrow.down")
                            .font(.system(size: 10, weight: .medium))
                            .padding(.horizontal, 10).padding(.vertical, 6)
                            .background(.regularMaterial, in: Capsule())
                    }
                    .buttonStyle(.plain)
                    .padding(.trailing, 9).padding(.bottom, 7)
                    .transition(.opacity.combined(with: .move(edge: .bottom)))
                }
            }
            // A question the owner chose to answer is brought into view with its field.
            .onChange(of: model.questionFocus) { _, request in
                guard let request else { return }
                followsLatest = false
                withAnimation(reduceMotion ? nil : .easeOut(duration: 0.2)) { proxy.scrollTo("question-" + request.id, anchor: .bottom) }
            }
            .onChange(of: entries) { _, _ in
                if followsLatest {
                    proxy.scrollTo("transcript-bottom", anchor: .bottom)
                    hasUnreadBelow = false
                } else {
                    hasUnreadBelow = true
                }
            }
            .onChange(of: questions) { _, _ in
                if followsLatest {
                    proxy.scrollTo("transcript-bottom", anchor: .bottom)
                    hasUnreadBelow = false
                } else {
                    hasUnreadBelow = true
                }
            }
            .onAppear {
                if let request = model.questionFocus { proxy.scrollTo("question-" + request.id, anchor: .bottom) }
                else { proxy.scrollTo("transcript-bottom", anchor: .bottom) }
            }
        }
    }
}

/// The owner's request, then FERB's substantive plan, connection line, question and answer.
/// Canned transport acknowledgements stay archived but are not presented (owner, 2026-10-29). Ordinary steps
/// never reach the live line (owner, 2026-10-24: only the "N active tasks" row stays; DaemonConnection's `progress`).
/// Several asks (decision 19): the first reply's bubble, then the card, whose rows and footer are the answer.
private struct TurnView: View {
    let turn: ConversationTurn
    let answeredBelow: Bool
    @ObservedObject var model: NotchModel

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            if turn.preview {
                // The intro's suggested task (notes/setup-intro.md step 6): how FERB would do it, nothing done.
                Label("Preview · nothing here is done", systemImage: "eye")
                    .font(.system(size: 10.5, weight: .medium)).foregroundStyle(Palette.secondary)
            }
            RequestView(turn: turn)
            if let scope = SeenScope(requestID: turn.requestID, taskID: turn.backendTaskID) {
                SeenButton(scope: scope, model: model)
            }
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
            if turn.preview && turn.status == "previewRunning" {
                // Practising Stop (the tour's controls): the same words as a running task's button; nothing runs.
                Button("Stop this task") { model.introAction(.stopPreview) }.buttonStyle(FerbButtonStyle())
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
                    NativeMessageText(turn.request)
                    ForEach(turn.sources, id: \.self) { url in
                        Label(url.lastPathComponent, systemImage: "paperclip").font(.system(size: 10))
                    }
                }
                .padding(.horizontal, 11).padding(.vertical, 8)
                .background(Color.accentColor, in: RoundedRectangle(cornerRadius: 15))
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

/// Something FERB noticed (L9): FERB's words with a small "noticed" tag, then any outside text it quotes, shown as a
/// quote with its origin. Both are plain text, so nothing in them becomes a link or an instruction.
private struct NoticeView: View {
    let notice: ConversationNotice
    @ObservedObject var model: NotchModel

    var body: some View {
        FerbBubble {
            VStack(alignment: .leading, spacing: 6) {
                if let tag = notice.tag {
                    Label(tag, systemImage: "eye").font(.system(size: 10)).foregroundStyle(Palette.secondary)
                }
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

    /// The answer's own lines drawn on their own (decision 19), cut from its text.
    private var split: AnswerParts {
        AnswerParts(answer: turn.visibleAnswer, note: turn.note, correction: turn.correction, lines: turn.codeLines ?? [])
    }
    private var codeSites: String? {
        split.lines.first { $0.hasPrefix("Sites contacted:") }.map {
            $0.dropFirst("Sites contacted:".count).trimmingCharacters(in: .whitespaces)
        }
    }
    private var visibleParts: SitesContactedParts { SitesContactedParts(split.body) }
    private var fullParts: SitesContactedParts { SitesContactedParts(turn.answer) }
    private var displayedSites: String { codeSites ?? (visibleParts.sites.isEmpty ? fullParts.sites : visibleParts.sites) }

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
                        MarkdownContent(CardLayout.strip(visibleParts.answer, cards)).equatable()
                        AnswerLinesView(note: split.note, correction: split.correction,
                                        lines: CardLayout.remaining(split.lines.filter { !$0.hasPrefix("Sites contacted:") },
                                                                    cards))
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
