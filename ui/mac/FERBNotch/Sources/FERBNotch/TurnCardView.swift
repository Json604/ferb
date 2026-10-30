import SwiftUI
import NotchCore

/// Answers in steps (orchestration decisions 19, 20), drawn as the approved mock (demo/answers-in-steps): the first
/// reply's bubble with the plan line under a left rule, the turn card with one row per ask and its footer, the typing
/// dots. Not the mock's pink dashed explanation labels or "code" chips.

/// FERB's first reply: the asks answered at once, code's receipts, then the plan line and code's lines under it ("I'll
/// work in …") beside a thin rule. While the plan streams, its words so far stand where the plan line will be.
struct FirstReplyView: View {
    let turn: ConversationTurn

    var body: some View {
        if let line = turn.planLine {
            let direct = turn.direct ?? [], receipts = turn.receipts ?? [], lines = turn.planLines ?? []
            FerbBubble {
                VStack(alignment: .leading, spacing: 7) {
                    ForEach(Array(direct.enumerated()), id: \.offset) { _, text in MarkdownContent(text).equatable() }
                    ForEach(Array(receipts.enumerated()), id: \.offset) { _, receipt in NativeMessageText(receipt) }
                    if !line.isEmpty || !lines.isEmpty { PlanLineBlock(line: line, lines: lines) }
                }
            }
        } else if let plan = turn.plan {
            FerbBubble { NativeMessageText(plan) }            // saved before the plan line was its own field
        } else if let streaming = turn.streamingPlan, !streaming.isEmpty {
            FerbBubble { PlanLineBlock(line: streaming, lines: []) }
        }
    }
}

/// The plan line beside a thin rule (the mock's `.plan`).
struct PlanLineBlock: View {
    let line: String
    let lines: [String]

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            if !line.isEmpty { NativeMessageText(line) }
            ForEach(Array(lines.enumerated()), id: \.offset) { _, code in NativeMessageText(code, secondary: true) }
        }
        .padding(.leading, 8)
        .overlay(alignment: .leading) { Rectangle().fill(.white.opacity(0.22)).frame(width: 2) }
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

/// Under a message FERB read together with another (decision 18): that message's reply answers both.
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

/// The turn card: a row per ask in the owner's order (the asks answered in the first reply aren't rows), then the
/// footer code wrote once the task ended.
struct TurnCardView: View {
    let card: TurnCard
    let turn: ConversationTurn
    @ObservedObject var model: NotchModel

    var body: some View {
        let questions = model.rowQuestions(turn)
        let rows = card.rows.filter { !$0.direct }
        let running = model.runningTasks[card.taskID] != nil
        // A row still working in a turn that ended without its footer (interrupted, or read from history) isn't moving.
        let live = running || ["working", "needs_user"].contains(turn.status)
        VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(rows.enumerated()), id: \.element.id) { index, row in
                if index > 0 { Rectangle().fill(.white.opacity(0.07)).frame(height: 1) }
                CardRowView(row: row, questions: questions[row.part] ?? [], running: running, live: live, model: model)
            }
            if card.finished { footer }
        }
        .padding(.vertical, 4)
        .background(.white.opacity(0.10))
        .clipShape(RoundedRectangle(cornerRadius: 15))
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    /// "2 of 3 done" and code's lines ("Sites contacted: …"); a stop's headline above them.
    private var footer: some View {
        VStack(alignment: .leading, spacing: 4) {
            if let headline = card.headline { Text(headline).font(.system(size: 12)).foregroundStyle(Palette.ink) }
            (Text(card.line ?? "").fontWeight(.semibold).foregroundStyle(Palette.ink)
                + Text(card.lines.isEmpty ? "" : "   " + card.lines.joined(separator: "   ")).foregroundStyle(Palette.secondary))
                .font(.system(size: 11.5))
                .textSelection(.enabled)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 11).padding(.top, 8).padding(.bottom, 5)
        .overlay(alignment: .top) { Rectangle().fill(.white.opacity(0.07)).frame(height: 1) }
    }
}

/// One ask: its state's icon, its label, and what it says now. A question about it (FERB's own, or a confirmation
/// the gateway asks for its step) is answered here, with its buttons.
struct CardRowView: View {
    let row: CardRow
    let questions: [DaemonApproval]
    /// The task still runs: the row's work can be watched.
    let running: Bool
    /// Its turn is still under way; otherwise a working row stopped where it was.
    let live: Bool
    @ObservedObject var model: NotchModel
    @State private var watching = false

    /// An open confirmation for its step shows the row as asking; a working row of a turn that ended, as stopped.
    private var state: String {
        if !questions.isEmpty && row.state != "asks" && row.state != "paused" { return "asks" }
        return row.state == "working" && !live ? "stopped" : row.state
    }

    var body: some View {
        HStack(alignment: .top, spacing: 9) {
            RowIcon(state: state).frame(width: 18, height: 18).padding(.top, 1)
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: 6) {
                    Text(row.label).font(.system(size: 11, weight: .semibold)).foregroundStyle(Palette.secondary)
                    if state == "updated" { UpdatedTag() }
                    Spacer(minLength: 0)
                    if running, !row.isSettled, let step = row.stepTaskID, model.previewAccess.refusal == nil {
                        watchButton(step)
                    }
                }
                content.padding(.top, 2)
            }
        }
        .padding(.horizontal, 11).padding(.vertical, 9)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(tint.opacity(state == "updated" ? 0 : 0.09))
        .overlay(alignment: .leading) {
            if !["working", "done", "stopped"].contains(state) { Rectangle().fill(tint).frame(width: 2) }
        }
        .onChange(of: row.isSettled) { _, settled in if settled { watching = false } }
        .onChange(of: model.previewDismissalRevision) { _, _ in watching = false }
        .accessibilityElement(children: .contain)
        .accessibilityLabel(row.label + ", " + Self.stateName(state))
    }

    @ViewBuilder private var content: some View {
        VStack(alignment: .leading, spacing: 4) {
            switch state {
            case "working":
                Text("working…").font(.system(size: 12)).foregroundStyle(.tertiary)
            case "stopped":
                Text("Not finished").font(.system(size: 12)).foregroundStyle(.tertiary)
            case "failed":
                Text(verbatim: row.text ?? "Couldn't do it.").font(.system(size: 12)).foregroundStyle(Palette.redSoft)
                    .textSelection(.enabled)
            default:
                if let text = row.text { MarkdownContent(text).equatable() }
            }
            if let note = row.note {
                (Text("Couldn't confirm: ").fontWeight(.semibold).foregroundStyle(Palette.amber)
                    + Text(note).foregroundStyle(Palette.secondary)).font(.system(size: 11.5))
            }
            if let correction = row.correction {
                (Text("Correction: ").fontWeight(.semibold).foregroundStyle(Palette.ink)
                    + Text(correction).foregroundStyle(Palette.secondary)).font(.system(size: 11.5))
            }
            if let why = row.why, state == "updated" {
                Text(why.hasPrefix("Updated") ? why : "Updated: " + why)
                    .font(.system(size: 11.5)).foregroundStyle(Palette.cyan)
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

    private var tint: Color {
        switch state {
        case "asks": return Palette.amber
        case "failed": return Palette.red
        case "paused": return Palette.violet
        case "updated": return Palette.cyan
        default: return .clear
        }
    }

    static func stateName(_ state: String) -> String {
        ["working": "working", "done": "done", "updated": "updated", "asks": "asks you", "failed": "couldn't do",
         "paused": "paused", "stopped": "not finished"][state] ?? state
    }

    /// Watch FERB work on this ask: the live preview of the step that answers it (notes/preview.md).
    private func watchButton(_ step: String) -> some View {
        Button { watching.toggle() } label: {
            Image(systemName: "eye").font(.system(size: 10)).frame(width: 18, height: 15)
        }
        .buttonStyle(.plain)
        .foregroundStyle(Palette.secondary)
        .help("Watch FERB work on this")
        .accessibilityLabel("Watch FERB work on " + row.label)
        .popover(isPresented: $watching, arrowEdge: .bottom) {
            VStack(alignment: .leading, spacing: 8) {
                TaskVideoView(preview: model.preview(task: row.taskID, worker: step))
                    .frame(width: 300, height: 170)
                    .clipShape(RoundedRectangle(cornerRadius: 9))
                Text(row.label).font(.system(size: 11, weight: .medium)).lineLimit(1)
            }
            .padding(11)
            .background(TaskPreviewWindow(model: model, hidesFromCapture: true))
        }
    }
}

/// The row's state as the mock draws it: a spinner, a check, an amber question mark, a red cross, a violet pause.
private struct RowIcon: View {
    let state: String

    var body: some View {
        switch state {
        case "done", "updated":
            Image(systemName: "checkmark").font(.system(size: 12, weight: .bold)).foregroundStyle(Palette.green)
        case "asks":
            Text("?").font(.system(size: 10, weight: .heavy)).foregroundStyle(Palette.amber)
                .frame(width: 15, height: 15)
                .overlay(Circle().stroke(Palette.amber, lineWidth: 1.5))
        case "failed":
            Image(systemName: "xmark").font(.system(size: 11, weight: .bold)).foregroundStyle(Palette.redSoft)
        case "paused":
            Image(systemName: "pause.fill").font(.system(size: 11)).foregroundStyle(Palette.violet)
        case "stopped":
            Circle().stroke(.white.opacity(0.3), lineWidth: 1.4).frame(width: 12, height: 12)
        default:
            RowSpinner()
        }
    }
}

private struct RowSpinner: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        TimelineView(.animation(minimumInterval: 1.0 / 30, paused: reduceMotion)) { context in
            let turn = context.date.timeIntervalSinceReferenceDate.truncatingRemainder(dividingBy: 0.9) / 0.9
            Circle().stroke(.white.opacity(reduceMotion ? 0.55 : 0.18), lineWidth: 1.6)
                .overlay {
                    if !reduceMotion {
                        Circle().trim(from: 0, to: 0.25)
                            .stroke(.white.opacity(0.75), style: StrokeStyle(lineWidth: 1.6, lineCap: .round))
                            .rotationEffect(.degrees(turn * 360))
                    }
                }
                .frame(width: 12, height: 12)
        }
    }
}

private struct UpdatedTag: View {
    var body: some View {
        Text("UPDATED")
            .font(.system(size: 9.5, weight: .bold)).tracking(0.5)
            .foregroundStyle(Color(.sRGB, red: 0, green: 0.1, blue: 0.14, opacity: 1))
            .padding(.horizontal, 5)
            .background(Palette.cyan, in: RoundedRectangle(cornerRadius: 4))
    }
}

/// A one-ask answer's own lines (decision 19): "Couldn't confirm", the correction, and code's lines other than the
/// sites (which keep their chip).
struct AnswerLinesView: View {
    let note: String?
    let correction: String?
    let lines: [String]

    var body: some View {
        if let note {
            (Text("Couldn't confirm: ").fontWeight(.semibold).foregroundStyle(Palette.amber)
                + Text(note).foregroundStyle(Palette.secondary)).font(.system(size: 11.5)).textSelection(.enabled)
        }
        if let correction {
            (Text("Correction: ").fontWeight(.semibold).foregroundStyle(Palette.ink)
                + Text(correction).foregroundStyle(Palette.secondary)).font(.system(size: 11.5)).textSelection(.enabled)
        }
        ForEach(Array(lines.enumerated()), id: \.offset) { _, line in
            Text(verbatim: line).font(.system(size: 11)).foregroundStyle(Palette.secondary).textSelection(.enabled)
        }
    }
}

/// A failed answer's offer of another AI service (owner decision 16): one tap per signed-in alternative.
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
