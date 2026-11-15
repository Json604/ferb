import Foundation
import NotchCore

private typealias Pick = IntroCopy.Pick

/// The tour's demo beats (kb area app steps 6–7). Try runs the suggestion the owner picks for real, through the same
/// path as a typed message (`chat.send`: the real daemon and gateway, their usual questions and confirmations; APP-666),
/// in the open chat with its turn card and live preview, while the notch says where it stands; no orb flies in this
/// working step (APP-693). Control is taught by doing; the tour starts no task of its own: Stop is offered on the
/// owner's Try task while it still runs, else only shown (APP-696).
/// A suggested first task (`intro.suggest`) and the one app it would use, as macOS names it ("" for the browser).
struct IntroSuggestion: Equatable {
    var request: String
    var app: String
}

extension IntroController {
    enum DemoTiming {
        /// How often a task the tour started is read for the line in the notch (its card in the chat updates live).
        static let watch = Timing.poll
    }

    // MARK: 6. Try

    /// What the scan saw (when it saw something), two suggested first tasks, and the picked one run for real while the
    /// owner watches; then the other one, or on.
    func tryIt() async {
        await working(IntroCopy.checking(nil)) { await setup.refreshPersona() }
        guard live else { return }
        if case .seen(let summary) = setup.persona, !summary.isEmpty {
            say(IntroCopy.saw(summary))
            await dwell(Timing.scan)
        }
        guard live, let suggestions = await suggestions() else { return leaveChat() }
        guard case .choice(let picked)? = await ask(IntroCopy.suggest(suggestions.map(\.request))),
              var index = suggestions.indices.first(where: { Pick.suggestion($0) == picked }) else { return leaveChat() }
        while live {
            let outcome = await runForReal(suggestions[index])
            guard live else { return }
            let other = suggestions.indices.first { !ran.contains(suggestions[$0].request) }
            guard case .choice(Pick.otherSuggestion)? = await ask(IntroCopy.ran(outcome, otherLeft: other != nil)),
                  let other else { return leaveChat() }
            index = other
        }
    }

    /// `intro.suggest`: always two requests from the daemon (the scan's, or two general ones), each with the one app
    /// it would use ("" for the default browser).
    func suggestions() async -> [IntroSuggestion]? {
        while live {
            let reply = await working(IntroCopy.thinking) {
                try? await rpc.call("intro.suggest", params: [:], wait: 60)
            }
            if let reply {
                let offered = (reply["suggestions"] as? [[String: Any]] ?? []).compactMap { row -> IntroSuggestion? in
                    guard let request = row["request"] as? String, !request.isEmpty else { return nil }
                    return IntroSuggestion(request: request, app: row["app"] as? String ?? "")
                }
                if offered.count >= 2 { return Array(offered.prefix(2)) }
            }
            guard live, case .choice(Pick.retry)? = await ask(IntroCopy.noSuggestions) else { return nil }
        }
        return nil
    }

    /// The suggestion as the owner's message (APP-666), shown in the open chat (no orb, APP-693); the notch follows its
    /// turn (planning, the step it is on, a question for the owner) with the time it has run, until
    /// it ends or the owner goes on without waiting (it keeps running as any task does).
    func runForReal(_ suggestion: IntroSuggestion) async -> IntroCopy.Outcome {
        ran.insert(suggestion.request)
        openChat()
        let since = Date()
        say(IntroCopy.starting.waiting(since: since))
        guard let id = await startTask(suggestion.request) else { return .notStarted }
        tryTasks.append(id)
        let answer = await whileRunning(id) { [weak self] in
            IntroCopy.running(self?.taskState(id) ?? .starting).waiting(since: since)
        }
        guard answer == nil else { return .stillRunning }
        return outcome(id)
    }

    /// Sends `request` as the owner's message through the normal path (NotchModel.send → `chat.send`); the composer
    /// takes typed answers meanwhile (a question the task asks). Its turn's id, or nil when it couldn't start.
    func startTask(_ request: String) async -> UUID? {
        let turn: UUID?? = await suspend { [weak self] (resume: @escaping (UUID?) -> Void) in
            guard let self else { return resume(nil) }
            self.notch.send(request) { resume($0) }
        }
        return turn ?? nil
    }

    /// Shows `line()` and follows the turn until it is no longer working or waiting on the owner; the owner's answer
    /// to the line (Continue the tour) ends the wait. The composer starts tasks meanwhile (the task's own questions may
    /// need a typed answer).
    func whileRunning(_ id: UUID, showing line: @escaping () -> IntroLine) async -> IntroAnswer? {
        notch.introHoldsWork = false
        defer { if running { notch.introHoldsWork = true } }
        return await wait(showing: line, every: DemoTiming.watch) { [weak self] in self?.ended(id) ?? true }
    }

    func turn(_ id: UUID) -> ConversationTurn? { notch.conversation.turns.first { $0.id == id } }

    /// The turn has ended: no longer working and not waiting on the owner (gone counts as ended).
    func ended(_ id: UUID) -> Bool {
        guard let turn = turn(id) else { return true }
        return turn.status != "working" && turn.status != "needs_user"
    }

    /// Where the tour's task stands, from its turn and the open questions.
    func taskState(_ id: UUID) -> IntroCopy.TaskState {
        guard let turn = turn(id) else { return .starting }
        if turn.status == "needs_user"
            || notch.pendingApprovals.contains(where: { $0.taskID != nil && $0.taskID == turn.backendTaskID }) {
            return .asking
        }
        if let card = turn.card, let index = card.rows.firstIndex(where: { $0.state == "working" }) {
            return .step(index + 1, max(card.total, card.rows.count), card.rows[index].label)
        }
        if let plan = turn.planLine ?? turn.plan, !plan.isEmpty { return .planned(plan) }
        return turn.backendTaskID == nil ? .starting : .working
    }

    func outcome(_ id: UUID) -> IntroCopy.Outcome {
        switch turn(id)?.status {
        case "done"?: return .done
        case "stopped"?, "interrupted"?: return .stopped
        default: return .failed
        }
    }

    /// The real chat, on today, where the task's turn shows. FERB opens it, not the owner: no keyboard focus is taken
    /// (rule 10); the panel becomes key only on the owner's click or shortcut.
    func openChat() {
        if !notch.viewingToday { notch.selectDay(Date(), focus: false) }
        if !notch.expanded { notch.expand(focus: false) }
    }

    func leaveChat() {
        if notch.expanded { notch.collapse() }
    }

    // MARK: 7. Control

    /// Each control once, in any order: double-tap right ⌥, hold right ⌘+⌥, Stop (APP-696), Pause FERB
    /// from the menu (lifted at once). The beat goes on once each has been seen. A FERB the owner had paused before the
    /// beat stays paused: the step is explained, not practised.
    func control() async {
        seenControls = []
        let pausedByOwner = notch.daemonPaused
        controls.onControl = { [weak self] control in self?.saw(control) }
        var lifted = false
        while live, let next = IntroControl.allCases.first(where: { !seenControls.contains($0) }) {
            if next == .stop {
                await stopStep()
                seenControls.insert(.stop)
                continue
            }
            if next == .pause && pausedByOwner {
                say(IntroCopy.alreadyPaused)
                await dwell(Timing.line * 2)
                seenControls.insert(.pause)
                continue
            }
            let open = IntroCopy.OpenKeys.now(notch.preferences)
            if next == .toggle && !open.hasKeys {
                // No keyboard shortcut opens FERB (double-tap off, none recorded): the step is explained, not practised.
                say(IntroCopy.control(next, open: open))
                await dwell(Timing.line * 2)
                seenControls.insert(.toggle)
                continue
            }
            say(IntroCopy.control(next, open: open))
            _ = await suspend { [weak self] (resume: @escaping (Bool?) -> Void) in self?.controlWaiter = { resume(true) } }
            if seenControls.contains(.pause) && !lifted && !pausedByOwner {
                lifted = true
                _ = try? await rpc.call("sense.resume", params: [:])
                notch.daemonPaused = false
            }
        }
        controls.onControl = nil
        guard live else { return }
        leaveChat()
        say(IntroCopy.controlsDone)
        await dwell(Timing.line)
    }

    /// The owner used a control.
    func saw(_ control: IntroControl) {
        guard running, beat == .control else { return }
        seenControls.insert(control)
        let waiting = controlWaiter
        controlWaiter = nil
        waiting?()
    }

    /// Stop, without starting anything (APP-696; owner, 2026-11-04: "it started doing the task i said not now, why???
    /// it didnt listen to me"). The owner's own Try task still running is offered: Stop it now acts at once (the same
    /// `task.stop` as the task list's Stop task, "Stopping it…" at once, the line follows the turn until it shows
    /// stopped, Go on never leaves the owner waiting), Let it finish goes on and leaves it running. With nothing
    /// running (Not now in Try, a task already done, a resumed tour) the line only says where Stop is and the beat
    /// goes on. Nothing here can hang.
    func stopStep() async {
        guard let id = tryTasks.last(where: { !ended($0) }) else {
            say(IntroCopy.control(.stop))
            return await dwell(Timing.line * 2)
        }
        openChat()
        let since = Date()
        var pressed: Date?
        sideChoices[Pick.stopTask] = { [weak self] in
            guard let self, self.live, pressed == nil else { return }
            pressed = Date()
            self.notch.stopTurn(id)
            self.say(IntroCopy.stopping.waiting(since: pressed ?? Date()))
        }
        defer { sideChoices[Pick.stopTask] = nil }
        let request = turn(id)?.request ?? ""
        let answer = await whileRunning(id) {
            if let pressed { return IntroCopy.stopping.waiting(since: pressed) }
            return IntroCopy.stopOffer(request).waiting(since: since)
        }
        guard live else { return }
        if pressed != nil { say(IntroCopy.stopped) }
        else if answer == .choice(Pick.letFinish) { say(IntroCopy.lettingItFinish) }
        else { say(IntroCopy.finishedFirst) }
        await dwell(Timing.line * 2)
    }
}
