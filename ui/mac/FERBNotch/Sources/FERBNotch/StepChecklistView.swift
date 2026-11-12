import SwiftUI
import NotchCore

/// APP-894 (owner 2026-11-11: "3 or more than three steops in a single ask. tehn show it liek teh row"; the approved
/// mock): a single ask's steps on its card. The task's name and "2 of 4" on top; a line per step: a tick, greyed, when
/// done; the running step highlighted with its live line and its own small live preview (APP-888, shown once:
/// APP-891); an empty circle for a step not started; a stopped, failed or won't-run step in the card rows' looks
/// (ORCH-532, ORCH-613). State only: the one answer arrives under the card as before. Once the task ended it is one
/// line ("4 steps done", "2 of 4 steps done") that a click opens.
struct StepChecklistView: View {
    let checklist: StepChecklist
    let turn: ConversationTurn
    @ObservedObject var model: NotchModel

    var body: some View {
        let running = model.runningTasks[checklist.taskID] != nil
        // A checklist whose turn ended without its outcome (interrupted, read from history) isn't moving.
        let live = !checklist.finished && (running || ["working", "needs_user"].contains(turn.status))
        let lines = Self.lines(checklist, live: live, model: model)
        let done = lines.filter { $0.kind == .done }.count
        let key = ExpansionState.key(turn: turn.id, item: "checklist")
        let ended = checklist.finished || !live
        let open = !ended || model.expandedText.isOpen(key)
        VStack(alignment: .leading, spacing: 0) {
            if ended {
                Button { model.toggleExpanded(key, target: TranscriptTarget.answer(turn: turn.id)) } label: {
                    HStack(spacing: 6) {
                        Image(systemName: open ? "chevron.down" : "chevron.right")
                            .font(.system(size: 9, weight: .semibold)).foregroundStyle(Palette.secondary)
                        Text(ChecklistSummary.collapsed(done: done, total: lines.count))
                            .font(.system(size: 11.5, weight: .semibold)).foregroundStyle(Palette.secondary)
                        Spacer(minLength: 0)
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(ChecklistSummary.collapsed(done: done, total: lines.count))
                .accessibilityHint(open ? "Hides the steps" : "Shows the steps")
            } else {
                HStack(spacing: 6) {
                    Text(turn.taskName ?? "Steps").font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(Palette.secondary).lineLimit(1)
                    Spacer(minLength: 8)
                    Text(ChecklistSummary.count(done: done, total: lines.count))
                        .font(.system(size: 10.5)).foregroundStyle(Palette.secondary)
                        .accessibilityLabel("\(done) of \(lines.count) steps done")
                }
            }
            if open {
                VStack(alignment: .leading, spacing: 5) {
                    ForEach(lines) { line in
                        ChecklistLineView(line: line, task: checklist.taskID, model: model,
                                          preview: running && !turn.preview && line.isRunning)
                    }
                }
                .padding(.top, 7)
            }
        }
        .padding(.horizontal, 11).padding(.vertical, 9)
        .background {
            RoundedRectangle(cornerRadius: 15, style: .continuous).fill(.white.opacity(0.10))
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .contain)
    }

    /// Each step's line now: its live state (or what it ended as), its own live line, an open question for its step
    /// task, its loop's round.
    @MainActor
    static func lines(_ checklist: StepChecklist, live: Bool, model: NotchModel) -> [ChecklistLine] {
        checklist.steps.map { step in
            let state = checklist.states[step.id]
            return ChecklistLine.make(step: step, state: state, ended: checklist.ended[step.id], live: live,
                                      liveLine: model.stepLive[step.stepTaskID],
                                      asking: Self.asks(model.pendingApprovals, step.stepTaskID),
                                      loop: Self.round(state, model.standingWork[checklist.taskID]))
        }
    }

    static func asks(_ approvals: [DaemonApproval], _ stepTask: String) -> Bool {
        guard !stepTask.isEmpty else { return false }
        return approvals.contains { ($0.sourceTaskID ?? "") == stepTask || ($0.sourceTaskID ?? "").hasPrefix(stepTask + ".") }
    }

    static func round(_ state: StepState?, _ work: StandingWork?) -> LoopRound? {
        guard state?.loop == true, let work else { return nil }
        return LoopRound(rounds: work.rounds, nextCheck: work.lastRoundEnd.map { $0.addingTimeInterval(work.gapSeconds) })
    }
}

/// One step's line (APP-894, APP-895): its mark, its words (the planner's label, else its goal's first clause, cut),
/// what it is doing or how it ended, and its live preview while it runs. Read as "<label>, <state>".
struct ChecklistLineView: View {
    let line: ChecklistLine
    let task: String
    @ObservedObject var model: NotchModel
    var preview: Bool

    var body: some View {
        HStack(alignment: .center, spacing: 7) {
            mark.frame(width: 12, height: 12)
            Text(line.step.label)
                .font(.system(size: 12, weight: line.isRunning ? .semibold : .regular))
                .foregroundStyle(labelColor)
                .lineLimit(1).truncationMode(.tail)
                .layoutPriority(1)
            if let words = line.words {
                Text(words).font(.system(size: 11.5)).foregroundStyle(wordsColor)
                    .lineLimit(1).truncationMode(.tail)
            }
            Spacer(minLength: 0)
            if preview, !line.step.stepTaskID.isEmpty {
                RowPreview(model: model, preview: model.preview(task: task, worker: line.step.stepTaskID),
                           label: line.step.label, task: task)
            }
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel(line.step.label + ", " + line.stateName + (line.isRunning ? line.words.map { ": " + $0 } ?? "" : ""))
    }

    @ViewBuilder private var mark: some View {
        switch line.kind {
        case .done: Image(systemName: "checkmark").font(.system(size: 9, weight: .bold)).foregroundStyle(Palette.secondary)
        case .running, .loop: RowSpinner(paused: !model.bodyVisible)
        case .upcoming: Image(systemName: "circle").font(.system(size: 9)).foregroundStyle(.tertiary)
        case .waiting: Image(systemName: "clock").font(.system(size: 9)).foregroundStyle(Palette.secondary)
        case .needsYou: Image(systemName: "questionmark.circle.fill").font(.system(size: 10)).foregroundStyle(Palette.amber)
        case .failed: Image(systemName: "xmark").font(.system(size: 9, weight: .bold)).foregroundStyle(Palette.redSoft)
        case .wontRun: Image(systemName: "minus.circle").font(.system(size: 10)).foregroundStyle(Palette.redSoft)
        // ORCH-613: the owner's decision, in the neutral secondary a stopped task has, never the failure's red
        case .stopped: Image(systemName: "stop.circle").font(.system(size: 10)).foregroundStyle(Palette.secondary)
        case .notFinished: Image(systemName: "circle.dashed").font(.system(size: 10)).foregroundStyle(Palette.secondary)
        }
    }

    private var labelColor: AnyShapeStyle {
        switch line.kind {
        case .done: return AnyShapeStyle(.tertiary)                      // done: greyed
        case .running, .loop, .needsYou: return AnyShapeStyle(Palette.ink)   // the step working now: highlighted
        default: return AnyShapeStyle(Palette.secondary)
        }
    }

    private var wordsColor: AnyShapeStyle {
        switch line.kind {
        case .needsYou: return AnyShapeStyle(Palette.amber)
        case .failed, .wontRun: return AnyShapeStyle(Palette.redSoft)
        default: return AnyShapeStyle(Palette.secondary)
        }
    }
}
