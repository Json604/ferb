import AppKit
import SwiftUI
import NotchCore

/// Answers in steps (ORCH-319, 20), drawn as the approved mock (demo/answers-in-steps): the first
/// reply's bubble with the plan line, the turn card with one row per ask and its footer, the typing
/// dots. Not the mock's pink dashed explanation labels or "code" chips.

/// FERB's first reply: code's receipts, then the plan line and code's lines under it ("Working in …"); the asks answered
/// at once only when there is no card (with one, each is its own row, APP-755). While the plan streams, its words so far stand where the plan line will be.
struct FirstReplyView: View {
    let turn: ConversationTurn

    var body: some View {
        if let line = turn.shownPlanLine {
            // a receipt drawn as its card (step 8e) isn't said again here; with a card, a part answered at once is only
            // its row (APP-755)
            let direct = turn.shownDirect, lines = turn.planLines ?? []
            let receipts = CardLayout.remaining(turn.receipts ?? [], turn.receiptCards ?? [])
            FerbBubble {
                NativeMessageText(MessageTextAttributes.joined(
                    direct.map { MarkdownTextAttributes.make($0, size: 12) }
                    + receipts.map { NativeMessageText($0).text }
                    + [NativeMessageText(line).text]
                    + lines.map { NativeMessageText($0, secondary: true).text }
                ))
            }
        } else if let plan = turn.shownPlan {
            FerbBubble { NativeMessageText(plan) }            // saved before the plan line was its own field
        } else if let streaming = turn.streamingPlan, !streaming.isEmpty {
            FerbBubble { PlanLineBlock(line: streaming, lines: []) }
        }
    }
}

/// The plan and its supporting lines share the reply bubble.
struct PlanLineBlock: View {
    let line: String
    let lines: [String]

    var body: some View {
        NativeMessageText(MessageTextAttributes.joined([NativeMessageText(line).text]
            + lines.map { NativeMessageText($0, secondary: true).text }))
    }
}

/// FERB has the message and is reading it: three dots, from the acknowledgement until its first words.
struct TypingDots: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        TimelineView(.animation(minimumInterval: 1.0 / 30, paused: reduceMotion)) { context in
            let t = context.date.timeIntervalSinceReferenceDate
            HStack(spacing: 4) {
                ForEach(0..<3, id: \.self) { index in
                    Circle().fill(Palette.secondary)
                        .frame(width: 5, height: 5)
                        .opacity(reduceMotion ? 0.6 : Self.blink(t - Double(index) * 0.2))
                }
            }
        }
        .padding(.horizontal, 12).padding(.vertical, 9)
        .background(.white.opacity(0.10), in: RoundedRectangle(cornerRadius: 15))
        .accessibilityLabel("FERB is reading")
    }

    /// The mock's blink: faint, bright at 40 % of each 1.2 s cycle, faint again by 80 %.
    static func blink(_ t: Double) -> Double {
        let f = (t.truncatingRemainder(dividingBy: 1.2) + 1.2).truncatingRemainder(dividingBy: 1.2) / 1.2
        if f < 0.4 { return 0.25 + 0.75 * f / 0.4 }
        if f < 0.8 { return 1 - 0.75 * (f - 0.4) / 0.4 }
        return 0.25
    }
}

/// Under a message FERB read together with another (ORCH-318): that message's reply answers both.
struct MergedLabel: View {
    let lead: ConversationTurn?
    let leadIsAbove: Bool

    var body: some View {
        HStack {
            Spacer(minLength: 28)
            Text(lead == nil ? "Read together with your other message"
                 : "Read together with your message " + (leadIsAbove ? "above" : "below"))
                .font(.system(size: 10)).foregroundStyle(Palette.secondary)
        }
    }
}

/// The turn card: a row per ask in the owner's order, an ask answered in the first reply too (a done row with its
/// answer, so the rows and "N of M" always agree: ORCH-543), a status line while it runs (APP-718), then the footer code
/// wrote once the task ended.
struct TurnCardView: View {
    let card: TurnCard
    let turn: ConversationTurn
    @ObservedObject var model: NotchModel

    var body: some View {
        let questions = model.rowQuestions(turn)
        let rows = card.shownRows
        let running = model.runningTasks[card.taskID] != nil
        // A row still working in a turn that ended without its footer (interrupted, or read from history) isn't moving.
        let live = running || ["working", "needs_user", "previewRunning"].contains(turn.status)
        let displays = rows.map { display($0, rows: rows, live: live, questions: questions) }
        VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(rows.enumerated()), id: \.element.id) { index, row in
                if index > 0 { Rectangle().fill(.white.opacity(0.07)).frame(height: 1) }
                CardRowView(row: row, display: displays[index], questions: questions[row.part] ?? [], turnID: turn.id,
                            model: model)
                    .transcriptAnchor(TranscriptTarget.row(turn: turn.id, part: row.part))
            }
            if card.finished {
                footer.transcriptAnchor(TranscriptTarget.footer(turn: turn.id))
            } else if live && running && !rows.isEmpty && !turn.preview {
                statusLine(displays)
            }
        }
        .padding(.vertical, 4)
        .background {
            RoundedRectangle(cornerRadius: 15, style: .continuous)
                .fill(.white.opacity(0.10))
                .overlay { RoundedRectangle(cornerRadius: 15, style: .continuous).fill(surfaceTint.opacity(0.14)) }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func display(_ row: CardRow, rows: [CardRow], live: Bool, questions: [String: [DaemonApproval]]) -> RowDisplay {
        let step = row.step.flatMap { card.steps[$0] }
        let loop: LoopRound? = step?.loop == true ? model.standingWork[card.taskID].map { work in
            LoopRound(rounds: work.rounds, nextCheck: work.lastRoundEnd.map { $0.addingTimeInterval(work.gapSeconds) })
        } : nil
        return RowDisplay.make(row: row, steps: card.steps, rows: rows, live: live,
                               hasQuestion: !(questions[row.part] ?? []).isEmpty,
                               liveLine: row.stepTaskID.flatMap { model.stepLive[$0] }, loop: loop)
    }

    // One colour belongs to the complete card, including its footer; the row's words carry its state.
    private var surfaceTint: Color {
        let states = Set(card.rows.map(\.state))
        if !model.rowQuestions(turn).isEmpty || states.contains("asks") { return Palette.amber }
        if states.contains("failed") { return Palette.red }
        if !states.isDisjoint(with: ["paused", "updated"]) { return Palette.chatBlue }
        return .clear
    }

    /// "1 of 4 done · 2 running · 1 waiting" while the task runs; no clock (APP-752).
    private func statusLine(_ displays: [RowDisplay]) -> some View {
        Text(CardStatusLine.text(card: card, displays: displays))
            .font(.system(size: 10.5)).foregroundStyle(Palette.secondary)
            .lineLimit(1).truncationMode(.tail)
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 11).padding(.top, 7).padding(.bottom, 5)
        .overlay(alignment: .top) { Rectangle().fill(.white.opacity(0.07)).frame(height: 1) }
    }

    /// "2 of 3 done" and code's lines ("Sites contacted: …"); a stop's headline above them. A line a typed card stands
    /// for (step 8e: the sites, drawn as link cards under the card) isn't said again.
    private var footer: some View {
        let lines = CardLayout.remaining(card.lines, turn.cards ?? [])
        return VStack(alignment: .leading, spacing: 4) {
            if let headline = card.headline { Text(headline).font(.system(size: 12)).foregroundStyle(Palette.ink) }
            (Text(card.line ?? "").fontWeight(.semibold).foregroundStyle(Palette.ink)
                + Text(lines.isEmpty ? "" : "   " + lines.joined(separator: "   ")).foregroundStyle(Palette.secondary))
                .font(.system(size: 11.5))
                .textSelection(.enabled)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 11).padding(.top, 8).padding(.bottom, 5)
        .overlay(alignment: .top) { Rectangle().fill(.white.opacity(0.07)).frame(height: 1) }
    }
}

/// One ask: its label and what it says now (RowDisplay, APP-718). A question about it (FERB's own, or a confirmation
/// the gateway asks for its step) is answered here, with its buttons. A done or failed row shows its first lines with
/// the rest on Show more (APP-719).
struct CardRowView: View {
    let row: CardRow
    let display: RowDisplay
    let questions: [DaemonApproval]
    let turnID: UUID
    @ObservedObject var model: NotchModel

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 6) {
                Text(row.label).font(.system(size: 11, weight: .semibold)).foregroundStyle(Palette.secondary)
                if display.kind == .updated { UpdatedTag() }
                Spacer(minLength: 0)
            }
            content.padding(.top, 2)
        }
        .padding(.horizontal, 11).padding(.vertical, 9)
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .contain)
        .accessibilityLabel(row.label + ", " + Self.stateName(display.kind))
    }

    @ViewBuilder private var content: some View {
        VStack(alignment: .leading, spacing: 4) {
            switch display.kind {
            case .preview:
                Text("Preview: I'd do this step").font(.system(size: 12)).foregroundStyle(.tertiary)
            case .previewRunning:
                Text("Preview: as if working…").font(.system(size: 12)).foregroundStyle(.tertiary)
            case .failed:
                collapsible {
                    Text(verbatim: row.text ?? "Couldn't do it.").font(.system(size: 12)).foregroundStyle(Palette.redSoft)
                        .textSelection(.enabled)
                }
            case .wontRun:
                Text(display.words ?? "").font(.system(size: 12)).foregroundStyle(Palette.redSoft)
                    .textSelection(.enabled)
            case .done, .updated:
                if let text = row.text { collapsible { MarkdownContent(text).equatable() } }
            case .needsYou, .paused:
                if let text = row.text { MarkdownContent(text).equatable() }
                else if let words = display.words, questions.isEmpty, row.question == nil {
                    Text(words).font(.system(size: 12)).foregroundStyle(Palette.amber)
                }
            default:
                RowStateLine(display: display, visible: model.bodyVisible)
            }
            if let note = row.note {
                (Text("Couldn't confirm: ").fontWeight(.semibold).foregroundStyle(Palette.amber)
                    + Text(note).foregroundStyle(Palette.secondary)).font(.system(size: 11.5))
            }
            if let why = row.why, display.kind == .updated {
                Text(why.hasPrefix("Updated") ? why : "Updated: " + why)
                    .font(.system(size: 11.5)).foregroundStyle(Palette.chatBlue)
            }
            if let chosen = row.chosen {
                Text("You chose " + chosen).font(.system(size: 11)).foregroundStyle(.tertiary)
            }
            if !questions.isEmpty {
                ForEach(questions) { question in
                    ApprovalView(model: model, approval: question, inRow: true).padding(.top, 2)
                }
            } else if let question = row.question {
                // Its approval isn't open here (answered elsewhere, or the chat is catching up): the words only.
                Text(verbatim: question.text).font(.system(size: 12, weight: .medium)).padding(.top, 2)
            }
        }
    }

    private func collapsible<Content: View>(@ViewBuilder _ content: @escaping () -> Content) -> some View {
        CollapsibleText(key: ExpansionState.key(turn: turnID, item: row.part), kind: .row,
                        target: TranscriptTarget.row(turn: turnID, part: row.part), more: "Show more", model: model,
                        content: content)
    }

    static func stateName(_ kind: RowDisplay.Kind) -> String {
        [.waiting: "waiting", .starting: "starting", .running: "working", .checking: "checking the result",
         .retrying: "trying again", .needsYou: "asks you", .paused: "paused", .done: "done", .updated: "updated",
         .failed: "couldn't do", .wontRun: "won't run", .notFinished: "not finished", .loop: "watching",
         .working: "working", .preview: "preview, nothing done", .previewRunning: "preview, nothing done"][kind]
            ?? kind.rawValue
    }
}

/// A row's state words, then a small spinner while its step works (a still dot under Reduce Motion). No leading icon
/// (APP-628, refined by APP-718) and no clock (APP-752).
struct RowStateLine: View {
    let display: RowDisplay
    let visible: Bool

    var body: some View {
        HStack(spacing: 6) {
            Text(display.words ?? "")
                .font(.system(size: 12)).foregroundStyle(.secondary)
                // a retrying row is one line (APP-724); its middle gives way so the attempt and the live line's end stay
                .lineLimit(display.kind == .retrying ? 1 : 2)
                .truncationMode(display.kind == .retrying ? .middle : .tail)
            if display.spinner { RowSpinner(paused: !visible) }
        }
    }
}

/// The small inline spinner after a running row's words: an arc turning, or a still dot under Reduce Motion.
struct RowSpinner: View {
    let paused: Bool
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        if reduceMotion {
            Circle().fill(Palette.secondary).frame(width: 5, height: 5).accessibilityHidden(true)
        } else {
            TimelineView(.animation(minimumInterval: 1.0 / 30, paused: paused)) { context in
                let turn = context.date.timeIntervalSinceReferenceDate.truncatingRemainder(dividingBy: 0.9) / 0.9
                Circle().trim(from: 0, to: 0.7)
                    .stroke(Palette.secondary, style: StrokeStyle(lineWidth: 1.4, lineCap: .round))
                    .frame(width: 9, height: 9)
                    .rotationEffect(.degrees(turn * 360))
            }
            .accessibilityHidden(true)
        }
    }
}

/// A text shown to its first lines with a fade and Show more / Show less (APP-719), when `CollapsePolicy` says it is
/// long enough for its `kind` (APP-725); the same text either way. The control is explicit (selection on the text keeps working, APP-637). Collapsing
/// keeps the item's top in view.
struct CollapsibleText<Content: View>: View {
    let key: String
    let kind: CollapsePolicy.Kind
    let target: String
    let more: String
    @ObservedObject var model: NotchModel
    /// Shown under the fade only while collapsed: lines that must stay visible but sit at the text's end.
    var whenCollapsed: AnyView? = nil
    @ViewBuilder let content: () -> Content
    @State private var natural: CGFloat = 0

    private static var lineHeight: CGFloat { ceil(NSLayoutManager().defaultLineHeight(for: .systemFont(ofSize: 12))) }

    var body: some View {
        let lineHeight = Self.lineHeight
        let clamp = CollapsePolicy.clamp(kind, natural: natural, lineHeight: lineHeight)
        let collapsible = clamp != nil
        let collapsed = collapsible && !model.expandedText.isOpen(key)
        VStack(alignment: .leading, spacing: 3) {
            content()
                .fixedSize(horizontal: false, vertical: true)
                .background(GeometryReader { proxy in
                    Color.clear.preference(key: NaturalTextHeight.self, value: proxy.size.height)
                })
                .onPreferenceChange(NaturalTextHeight.self) { height in
                    if abs(height - natural) > 0.5 { natural = height }
                }
                .frame(height: collapsed ? clamp : nil, alignment: .top)
                .clipped()
                .mask {
                    LinearGradient(stops: [.init(color: .black, location: 0),
                                           .init(color: .black, location: collapsed ? 0.62 : 1),
                                           .init(color: collapsed ? .clear : .black, location: 1)],
                                   startPoint: .top, endPoint: .bottom)
                }
            if collapsed, let whenCollapsed { whenCollapsed }
            if collapsible {
                Button(collapsed ? more : "Show less") { model.toggleExpanded(key, target: target) }
                    .buttonStyle(.plain)
                    .font(.system(size: 10.5, weight: .medium))
                    .foregroundStyle(Palette.chatBlue)
                    .accessibilityHint(collapsed ? "Shows the whole text" : "Shows the first lines")
            }
        }
    }
}

private struct NaturalTextHeight: PreferenceKey {
    static var defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) { value = max(value, nextValue()) }
}

private struct UpdatedTag: View {
    var body: some View {
        Text("UPDATED")
            .font(.system(size: 9.5, weight: .bold)).tracking(0.5)
            .foregroundStyle(.white)
            .padding(.horizontal, 5)
            .background(Palette.chatBlue, in: RoundedRectangle(cornerRadius: 4))
    }
}

/// A one-ask answer's own lines (ORCH-319): "Couldn't confirm" and code's lines other than the sites (which keep their
/// chip). A checker's evidence is never one of them (ORCH-540).
struct AnswerLinesView: View {
    let note: String?
    let lines: [String]

    var body: some View {
        if let note {
            (Text("Couldn't confirm: ").fontWeight(.semibold).foregroundStyle(Palette.amber)
                + Text(note).foregroundStyle(Palette.secondary)).font(.system(size: 11.5)).textSelection(.enabled)
        }
        ForEach(Array(lines.enumerated()), id: \.offset) { _, line in
            Text(verbatim: line).font(.system(size: 11)).foregroundStyle(Palette.secondary).textSelection(.enabled)
        }
    }
}

/// A failed answer's offer of another AI service (ORCH-315): one tap per signed-in alternative.
struct RuntimeSwitchRow: View {
    let turn: ConversationTurn
    let offer: RuntimeSwitch
    @ObservedObject var model: NotchModel

    var body: some View {
        if let said = model.runtimeChoices[turn.id] {
            Text(said).font(.system(size: 10)).foregroundStyle(Palette.secondary)
        } else {
            HStack(spacing: 6) {
                ForEach(offer.choices, id: \.self) { choice in
                    Button("Use " + RuntimeSwitch.label(choice)) { model.useRuntime(choice, for: offer, turn: turn.id) }
                        .buttonStyle(FerbButtonStyle())
                }
            }
        }
    }
}
