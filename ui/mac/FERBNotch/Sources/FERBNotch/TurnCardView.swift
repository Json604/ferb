import SwiftUI
import NotchCore

/// Answers in steps (ORCH-319, 20), drawn as the approved mock (demo/answers-in-steps): the first
/// reply's bubble with the plan line, the turn card with one row per ask and its footer, the typing
/// dots. Not the mock's pink dashed explanation labels or "code" chips.

/// FERB's first reply: the asks answered at once, code's receipts, then the plan line and code's lines under it ("I'll
/// work in …"). While the plan streams, its words so far stand where the plan line will be.
struct FirstReplyView: View {
    let turn: ConversationTurn

    var body: some View {
        if let line = turn.planLine {
            // a receipt drawn as its card (step 8e) isn't said again here
            let direct = turn.direct ?? [], lines = turn.planLines ?? []
            let receipts = CardLayout.remaining(turn.receipts ?? [], turn.receiptCards ?? [])
            FerbBubble {
                NativeMessageText(MessageTextAttributes.joined(
                    direct.map { MarkdownTextAttributes.make($0, size: 12) }
                    + receipts.map { NativeMessageText($0).text }
                    + [NativeMessageText(line).text]
                    + lines.map { NativeMessageText($0, secondary: true).text }
                ))
            }
        } else if let plan = turn.plan {
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
        let live = running || ["working", "needs_user", "previewRunning"].contains(turn.status)
        VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(rows.enumerated()), id: \.element.id) { index, row in
                if index > 0 { Rectangle().fill(.white.opacity(0.07)).frame(height: 1) }
                CardRowView(row: row, questions: questions[row.part] ?? [], live: live, model: model)
            }
            if card.finished { footer }
        }
        .padding(.vertical, 4)
        .background {
            RoundedRectangle(cornerRadius: 15, style: .continuous)
                .fill(.white.opacity(0.10))
                .overlay { RoundedRectangle(cornerRadius: 15, style: .continuous).fill(surfaceTint.opacity(0.14)) }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    // One colour belongs to the complete card, including its footer; the row's words carry its state.
    private var surfaceTint: Color {
        let states = Set(card.rows.filter { !$0.direct }.map(\.state))
        if !model.rowQuestions(turn).isEmpty || states.contains("asks") { return Palette.amber }
        if states.contains("failed") { return Palette.red }
        if !states.isDisjoint(with: ["paused", "updated"]) { return Palette.chatBlue }
        return .clear
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

/// One ask: its label and what it says now. A question about it (FERB's own, or a confirmation
/// the gateway asks for its step) is answered here, with its buttons.
struct CardRowView: View {
    let row: CardRow
    let questions: [DaemonApproval]
    /// Its turn is still under way; otherwise a working row stopped where it was.
    let live: Bool
    @ObservedObject var model: NotchModel

    /// An open confirmation for its step shows the row as asking; a working row of a turn that ended, as stopped.
    private var state: String {
        if !questions.isEmpty && row.state != "asks" && row.state != "paused" { return "asks" }
        if row.state == "preview" { return live ? "previewRunning" : "preview" }    // the intro's demo: nothing done
        return row.state == "working" && !live ? "stopped" : row.state
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 6) {
                Text(row.label).font(.system(size: 11, weight: .semibold)).foregroundStyle(Palette.secondary)
                if state == "updated" { UpdatedTag() }
                Spacer(minLength: 0)
            }
            content.padding(.top, 2)
        }
        .padding(.horizontal, 11).padding(.vertical, 9)
        .frame(maxWidth: .infinity, alignment: .leading)
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
            case "preview":
                Text("Preview: I'd do this step").font(.system(size: 12)).foregroundStyle(.tertiary)
            case "previewRunning":
                Text("Preview: as if working…").font(.system(size: 12)).foregroundStyle(.tertiary)
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

    static func stateName(_ state: String) -> String {
        ["working": "working", "done": "done", "updated": "updated", "asks": "asks you", "failed": "couldn't do",
         "paused": "paused", "stopped": "not finished", "preview": "preview, nothing done",
         "previewRunning": "preview, nothing done"][state] ?? state
    }


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

/// A one-ask answer's own lines (ORCH-319): "Couldn't confirm", the correction, and code's lines other than the
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
