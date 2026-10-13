import SwiftUI
import NotchCore

private struct TranscriptHeight: PreferenceKey {
    static var defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) { value = nextValue() }
}

struct ConversationTranscript: View {
    let entries: [TranscriptEntry]
    /// Open questions, shown after the newest entry; they are live state and never enter history.
    var questions: [DaemonApproval] = []
    @ObservedObject var model: NotchModel

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 10) {
                ForEach(entries) { entry in
                    Group {
                        switch entry {
                        case let .turn(turn, answeredBelow): TurnView(turn: turn, answeredBelow: answeredBelow, model: model)
                        case let .reply(reply, turn): LateReplyView(reply: reply, turn: turn, model: model)
                        case let .notice(notice): NoticeView(notice: notice)
                        }
                    }.id(entry.id)
                }
                ForEach(questions) { question in
                    ApprovalView(model: model, approval: question).id("question-" + question.id)
                }
            }
            .font(.system(size: 12))
            .padding(.top, 36)
            .padding(.bottom, 2)
            .frame(maxWidth: .infinity)
            .background(GeometryReader { proxy in
                Color.clear.preference(key: TranscriptHeight.self, value: proxy.size.height)
            })
        }
        .defaultScrollAnchor(.bottom)
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
    }
}

/// The owner's request, then FERB's lines in arrival order: acknowledgement, plan line, live step, question, answer.
private struct TurnView: View {
    let turn: ConversationTurn
    let answeredBelow: Bool
    @ObservedObject var model: NotchModel

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            RequestView(turn: turn)
            ForEach(Array(turn.ferbLines.enumerated()), id: \.offset) { _, line in
                FerbBubble { Text(line).textSelection(.enabled) }
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
                    Text(turn.request).textSelection(.enabled)
                    ForEach(turn.sources, id: \.self) { url in
                        Label(url.lastPathComponent, systemImage: "paperclip").font(.system(size: 10))
                    }
                }
                .padding(.horizontal, 11).padding(.vertical, 8)
                .background(Color.accentColor, in: RoundedRectangle(cornerRadius: 15))
                .help(turn.createdAt.formatted(date: .omitted, time: .shortened))
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

    var body: some View {
        FerbBubble {
            VStack(alignment: .leading, spacing: 6) {
                if let tag = notice.tag {
                    Label(tag, systemImage: "eye").font(.system(size: 10)).foregroundStyle(Palette.secondary)
                }
                Text(notice.text).textSelection(.enabled)
                ForEach(Array(notice.quotes.enumerated()), id: \.offset) { _, quote in
                    VStack(alignment: .leading, spacing: 2) {
                        Text(quote.text).textSelection(.enabled).font(.system(size: 11))
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

/// The current step and its elapsed time, replaced in place and removed when the answer arrives (L8).
private struct LiveLineView: View {
    let progress: LiveProgress

    var body: some View {
        // Ticks on whole elapsed seconds; a start time ahead of this Mac's clock ticks from now.
        TimelineView(.periodic(from: min(progress.startedAt, Date()), by: 1)) { context in
            Text(progress.text + " · " + progress.elapsedText(at: context.date))
                .lineLimit(1).truncationMode(.tail)
                .font(.system(size: 11)).foregroundStyle(Palette.secondary)
        }
        .accessibilityLabel("Current step: " + progress.text)
    }
}

private struct AnswerView: View {
    let turn: ConversationTurn
    @ObservedObject var model: NotchModel

    var body: some View {
        if !turn.answer.isEmpty {
            FerbBubble(verified: turn.isVerified) {
                VStack(alignment: .leading, spacing: 6) {
                    MarkdownContent(turn.visibleAnswer).equatable()
                    if turn.displayText != nil && turn.visibleAnswer != turn.answer {
                        DisclosureGroup("Details") {
                            MarkdownContent(turn.answer).equatable()
                                .frame(maxWidth: .infinity, alignment: .leading)
                        }.font(.system(size: 11))
                    }
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
        if turn.responseNotice == nil && turn.status != "done" && turn.status != "working" {
            Text(turn.status.replacingOccurrences(of: "_", with: " ").capitalized)
                .font(.system(size: 10)).foregroundStyle(Palette.secondary)
        }
        ForEach(turn.artifacts, id: \.self) { url in
            Button { model.onOpenDocument?(url) } label: {
                Label(url.lastPathComponent, systemImage: "doc").lineLimit(1)
            }
            .buttonStyle(FerbButtonStyle(quiet: true))
            .onDrag { NSItemProvider(object: url as NSURL) }
            .contextMenu {
                Button("Open") { model.onOpenDocument?(url) }
                Button("Quick Look") { model.onQuickLook?(url) }
                Button("Reveal in Finder") { NSWorkspace.shared.activateFileViewerSelecting([url]) }
            }
        }
    }
}

/// A neutral FERB message on the left; a verified answer carries a small check mark.
private struct FerbBubble<Content: View>: View {
    var verified = false
    @ViewBuilder let content: Content

    var body: some View {
        HStack(alignment: .bottom, spacing: 5) {
            content
                .padding(.horizontal, 11).padding(.vertical, 8)
                .background(.white.opacity(0.10), in: RoundedRectangle(cornerRadius: 15))
            if verified {
                Image(systemName: "checkmark")
                    .font(.system(size: 9, weight: .semibold)).foregroundStyle(Palette.green)
                    .padding(.bottom, 5)
                    .help("Verified").accessibilityLabel("Verified")
            }
            Spacer(minLength: 28)
        }
    }
}
