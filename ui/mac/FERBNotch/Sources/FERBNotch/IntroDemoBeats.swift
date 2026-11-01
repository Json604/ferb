import Foundation
import NotchCore

private typealias Pick = IntroCopy.Pick

/// The tour's demo beats (notes/setup-intro.md steps 6–7): Try looks and tells, it never acts (the daemon's
/// `intro.preview` is a planner turn with no run token and no worker, every event marked preview); Control is taught by
/// doing, with Stop practised on that preview replayed as running. Nothing here sends `chat.send`.
/// A suggested first task (`intro.suggest`) and the one app it would use, as macOS names it ("" for the browser).
struct IntroSuggestion: Equatable {
    var request: String
    var app: String
}

extension IntroController {
    // MARK: 6. Try

    /// What the scan saw (when it saw something), two suggested first tasks, the picked one's plan shown in the real
    /// chat as preview rows, "Show the other one", and "Do it for real", which is sent as the owner's message only
    /// after Home.
    func tryIt() async {
        await setup.refreshPersona()
        guard live else { return }
        if case .seen(let summary) = setup.persona, !summary.isEmpty {
            say(IntroCopy.saw(summary))
            await dwell(Timing.scan)
        }
        guard live, let offered = await suggestions() else { return leaveChat() }
        guard case .choice(let picked)? = await ask(IntroCopy.suggest(offered.map(\.request))),
              var shown = offered.indices.first(where: { Pick.suggestion($0) == picked }) else { return leaveChat() }
        var otherShown = false
        var failure = await preview(offered[shown])
        while live {
            let line = failure.map(IntroCopy.previewFailed) ?? IntroCopy.previewed(otherShown: otherShown)
            say(line)
            await orbTrip?.value                     // the plan is shown: the orb comes back
            orbTrip = nil
            await orb.flyHome()
            switch await ask(line) {
            case .choice(Pick.otherSuggestion)? where !otherShown:
                shown = 1 - shown
                otherShown = true
                failure = await preview(offered[shown])
            case .choice(Pick.doItForReal)? where failure == nil:
                realTask = offered[shown].request    // the owner's tap: sent as their words once the tour ends
                say(IntroCopy.willDoIt)
                await dwell(Timing.line * 2)
                return leaveChat()
            default:
                return leaveChat()
            }
        }
    }

    /// `intro.suggest`: always two requests from the daemon (the scan's, or two general ones), each with the one app
    /// it would use ("" for the default browser).
    func suggestions() async -> [IntroSuggestion]? {
        while live {
            say(IntroCopy.thinking)
            if let reply = try? await rpc.call("intro.suggest", params: [:], wait: 60) {
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

    /// The suggestion's plan, fetched from `intro.preview` and shown in the open chat as a preview turn, while the orb
    /// points at the app it would use (it only points: the app is never opened or brought forward). Returns why it
    /// couldn't be shown, or nil.
    func preview(_ suggestion: IntroSuggestion) async -> String? {
        say(IntroCopy.previewing)
        openChat()
        let target = apps.target(app: suggestion.app)
        orbTrip = Task { [orb] in
            await orb.launch(to: target.point, instruction: IntroCopy.wouldUse(target, browser: suggestion.app.isEmpty))
        }
        do {
            let events = try await fetchPreview(suggestion.request)
            guard live else { return nil }
            lastPreview = (suggestion.request, events)
            show(suggestion.request, events, running: false)
            return nil
        } catch {
            return error.localizedDescription
        }
    }

    /// Only events marked preview are kept: anything else on the stream is never shown as part of the demo.
    func fetchPreview(_ request: String) async throws -> [[String: Any]] {
        var events: [[String: Any]] = []
        for try await event in rpc.stream("intro.preview", params: ["request": request]) {
            guard live else { break }
            if event["preview"] as? Bool == true { events.append(event) }
        }
        return events
    }

    @discardableResult
    func show(_ request: String, _ events: [[String: Any]], running: Bool) -> UUID {
        let id = UUID()
        notch.conversation.beginPreview(id: id, request: request, running: running)
        for event in events { notch.conversation.applyPreview(id: id, event) }
        return id
    }

    /// The real chat, on today, where the preview turn shows. FERB opens it, not the owner: no keyboard focus is taken
    /// (rule 10); the panel becomes key only on the owner's click or shortcut.
    func openChat() {
        if !notch.viewingToday { notch.selectDay(Date(), focus: false) }
        if !notch.expanded { notch.expand(focus: false) }
    }

    func leaveChat() {
        if notch.expanded { notch.collapse() }
    }

    // MARK: 7. Control

    /// Each control once, in any order: double-tap right ⌥, hold right ⌘+⌥, Control-Option-Space, Stop on the replayed
    /// preview, Pause FERB from the menu (lifted at once). The beat goes on once each has been seen. A FERB the owner
    /// had paused before the beat stays paused: the step is explained, not practised.
    func control() async {
        seenControls = []
        replayID = nil
        let pausedByOwner = notch.daemonPaused
        controls.onControl = { [weak self] control in self?.saw(control) }
        var lifted = false
        while live, let next = IntroControl.allCases.first(where: { !seenControls.contains($0) }) {
            if next == .stop && replayID == nil {
                await replayForStop()
                continue
            }
            if next == .pause && pausedByOwner {
                say(IntroCopy.alreadyPaused)
                await dwell(Timing.line * 2)
                seenControls.insert(.pause)
                continue
            }
            say(IntroCopy.control(next))
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

    /// The owner used a control. Stop counts only on the replayed preview.
    func saw(_ control: IntroControl) {
        guard running, beat == .control else { return }
        if control == .stop {
            guard let id = replayID else { return }
            notch.conversation.stopPreview(id: id)
        }
        seenControls.insert(control)
        let waiting = controlWaiter
        controlWaiter = nil
        waiting?()
    }

    /// Try's preview again, as if running, in the open chat. Without one (a resumed tour), a suggestion is previewed
    /// first; if none can be, Stop isn't practised.
    private func replayForStop() async {
        if lastPreview == nil, let request = await suggestions()?.first?.request,
           let events = try? await fetchPreview(request), live {
            lastPreview = (request, events)
        }
        guard live else { return }
        guard let last = lastPreview else {
            seenControls.insert(.stop)
            return
        }
        openChat()
        replayID = show(last.request, last.events, running: true)
    }
}
