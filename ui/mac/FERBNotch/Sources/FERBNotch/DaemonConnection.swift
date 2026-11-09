import AppKit
import Foundation
import NotchCore

@MainActor
extension NotchModel {
    func connectDaemon() {
        guard eventsTask == nil else { return }
        eventsTask = Task { [weak self] in
            while !Task.isCancelled {
                guard let self else { return }
                do {
                    try await self.attachDaemon()
                    self.daemonLink.succeeded()
                    self.clickThrough.reconnected()     // the daemon may have restarted: its click-through windows
                    await self.prepareHistory()         // the chat in hand before the status read places anything
                    await self.refreshDaemonStatus()
                    await self.refreshTimers()
                    for try await event in self.rpc.stream("logs.subscribe", params: [:]) {
                        guard !Task.isCancelled else { return }
                        self.receiveBusEvent(event)
                    }
                    self.connectionStatus = "Reconnecting to FERB…"
                } catch {
                    self.connectionStatus = error.localizedDescription
                    self.daemonLink.failed(unreachable: Self.unreachable(error))
                }
                do { try await Task.sleep(for: .seconds(2)) } catch { return }
            }
        }
        reconciliationTask = Task { [weak self] in
            while !Task.isCancelled {
                do { try await Task.sleep(for: .seconds(5)) } catch { return }
                await self?.refreshDaemonStatus()
                self?.clickThrough.retry()              // a click-through report that didn't get through
            }
        }
    }

    func disconnectDaemon() {
        eventsTask?.cancel(); eventsTask = nil
        reconciliationTask?.cancel(); reconciliationTask = nil
        submissionTask?.cancel(); submissionTask = nil
        submissions.values.forEach { $0.cancel() }; submissions.removeAll()
        // Closing UI sockets only detaches; task.stop is exclusively an owner action.
    }

    func attachDaemon() async throws {
        var params: [String: Any] = ["new": false]
        if let conversationID { params["resume"] = conversationID }
        let attached = try await rpc.call("chat.attach", params: params)
        guard let id = attached["session_id"] as? String else { throw DaemonRPCError.invalidResponse }
        conversationID = id
        preferences.set(id, forKey: "FERBConversationID")
        supportsAttachments = (attached["capabilities"] as? [String] ?? []).contains("input_paths")
        previewAccess.reconnected()         // the daemon may have been updated: a refused preview is asked for again
        connectionStatus = "Connected to FERB"
        // FERB's self-description says what this app shows (DMN-6): its how-tos, shortcuts, settings' places and its
        // own settings with their current values; sent again whenever one of those changes (`appSettingsChanged`).
        sendGuide()
    }

    /// Hands the daemon FERB.app's guide as it stands now (APP-4, APP-728).
    func sendGuide() {
        guideResend?.cancel(); guideResend = nil
        guideSent = AppSettings.current(preferences)
        let payload = AppGuide.payload(preferences)
        Task { [rpc] in _ = try? await rpc.call("about.guide", params: payload) }
    }

    /// A preference changed somewhere in the app: when a declared setting's value differs from what FERB's guide
    /// last carried, the guide is sent again once things settle (one send for a slider's many writes). Nothing is sent
    /// before the first connection, which sends it anyway.
    func appSettingsChanged() {
        guard let sent = guideSent, AppSettings.current(preferences) != sent else { return }
        guideResend?.cancel()
        let wait = guideDebounce
        guideResend = Task { [weak self] in
            try? await Task.sleep(for: wait)
            guard !Task.isCancelled else { return }
            self?.sendGuide()
        }
    }

    func refreshDaemonStatus() async {
        guard !refreshingStatus else { return }
        refreshingStatus = true
        // Before the snapshot below, so this extra read never makes it stale.
        if bodyVisible && viewingToday { await fetchLearningDigest(); await fetchGuessBacklog() }
        let rootsBefore = Set(workSessions.values.compactMap(\.backendTaskID))
            .union(session.backendTaskID.map { [$0] } ?? [])
            .union(conversation.turns.filter { ["working", "interrupted", "needs_user"].contains($0.status) }.compactMap(\.backendTaskID))
        let snapshotRevision = taskStateRevision
        defer { refreshingStatus = false }
        let previousSize = panelSize
        defer { if panelSize != previousSize { onBackgroundChange?(true) } }
        do {
            // A peek: reading marks nothing shown. Results and notices are final facts, placed in the chat even when
            // a newer event makes the rest of the snapshot stale, and then acknowledged one by one, so `ferb chat`
            // replays only what this chat didn't show.
            let status = try await rpc.call("chat.status", params: ["peek": true])
            // Every 5 s: each value is set only when it changed, since setting one, even unchanged, redraws every view
            // of the model (the whole chat).
            var link = daemonLink
            link.succeeded()
            changeIfNeeded(\.daemonLink, link)
            changeIfNeeded(\.daemonPaused, status["paused"] as? Bool ?? false)
            changeIfNeeded(\.daemonStopped, status["stopped"] as? Bool ?? false)
            let results = status["results"] as? [[String: Any]] ?? []
            for result in results { receiveOutcome(result, announce: false, recordEvent: false) }
            acknowledgeResults(results.compactMap { $0["task_id"] as? String })
            receiveNotices(status["notices"] as? [[String: Any]] ?? [])
            // A newer event wins over this older request's snapshot. The existing
            // reconciliation cadence will fetch a fresh snapshot next time.
            guard taskStateRevision == snapshotRevision else { return }
            changeIfNeeded(\.connectionStatus, "Connected to FERB")
            let running = status["running"] as? [[String: Any]] ?? []
            changeIfNeeded(\.runningTasks, Dictionary(running.compactMap { row in
                guard let id = row["task_id"] as? String, !id.isEmpty else { return nil }
                return (id, Self.heading(row))
            }, uniquingKeysWith: { _, last in last }))
            for row in running {
                if let id = row["task_id"] as? String, runningTasks[id] != nil,
                   let started = LiveProgress.time(row["started_at"]), runningSince[id] != started { runningSince[id] = started }
            }
            // A multi-part task's rows as they stand (ORCH-319): live rows after a reconnect, or for a task this
            // chat has no turn for.
            var cardsChanged = false
            for row in running {
                guard let id = row["task_id"] as? String, let card = TurnCard(row["card"], taskID: id) else { continue }
                if changeConversation({ $0.showCard(card, goal: Self.heading(row, empty: "")) }) { cardsChanged = true }
            }
            if cardsChanged { saveConversation() }
            // Standing work only for a task still running (L31).
            let loops = status["loops"] as? [[String: Any]] ?? []
            changeIfNeeded(\.standingWork, Dictionary(loops.compactMap { row in
                guard let id = row["task_id"] as? String, runningTasks[id] != nil,
                      let work = StandingWork(loopStatus: row) else { return nil }
                return (id, work)
            }, uniquingKeysWith: { _, last in last }))
            // chat.status names a question's root task only: what the live event said (the step's task id, the part,
            // a runtime offer) is kept, so a question stays in its card row.
            let approvals = (status["approvals"] as? [[String: Any]] ?? []).map(keepKnownFields).compactMap(normalizeApproval)
            let changed = pendingApprovals != approvals
            if changed { pendingApprovals = approvals; promptError = nil; onBackgroundChange?(true) }
            for approval in approvals { announceApproval(approval) }
            if let approval = approvals.first(where: { $0.taskID == session.backendTaskID && $0.taskID != nil }) { session.waitForOwner(approval.reason) }
            for taskID in rootsBefore where runningTasks[taskID] == nil && !knownCompletedTasks.contains(taskID)
                && !approvals.contains(where: { $0.taskID == taskID }) {
                await reconcileMissingTask(taskID)
                guard taskStateRevision == snapshotRevision else { return }
            }
            if session.backendTaskID == nil, session.activeTaskID == nil {
                let remembered = preferences.string(forKey: "FERBActiveTaskID")
                if let id = remembered, let goal = runningTasks[id] {
                    session.resumeBackendTask(id, goal: goal)
                } else if let remembered,
                          let result = results.first(where: { $0["task_id"] as? String == remembered }) {
                    restoreResult(result)
                } else if let id = running.last?["task_id"] as? String, let goal = runningTasks[id] {
                    session.resumeBackendTask(id, goal: goal)
                    preferences.set(id, forKey: "FERBActiveTaskID")
                } else if let remembered {
                    await recoverTask(remembered)
                    guard taskStateRevision == snapshotRevision else { return }
                } else if let result = results.last { restoreResult(result) }
            }
            if let taskID = session.backendTaskID, session.activeTaskID == nil,
               let goal = runningTasks[taskID] {
                knownCompletedTasks.remove(taskID)
                session.resumeBackendTask(taskID, goal: goal)
            }
            if let taskID = session.backendTaskID, session.activeTaskID != nil {
                if runningTasks[taskID] != nil {
                    if !approvals.contains(where: { $0.taskID == taskID }),
                       session.phase != .working || session.attentionMessage != nil { session.resumeAfterOwnerAnswer() }
                } else if let result = results.first(where: { $0["task_id"] as? String == taskID }) {
                    receiveOutcome(result, announce: true, recordEvent: false)
                } else {
                    await recoverTask(taskID)
                    guard taskStateRevision == snapshotRevision else { return }
                }
            }
            if let approval = approvals.first(where: { $0.taskID == session.backendTaskID && $0.taskID != nil }) { session.waitForOwner(approval.reason) }
        } catch {
            connectionStatus = error.localizedDescription
            daemonLink.failed(unreachable: Self.unreachable(error))
        }
    }

    /// The daemon's goal is shown as FERB's description of the task, never as the owner's message; empty when unknown.
    private func restoreResult(_ result: [String: Any]) {
        guard let id = result["task_id"] as? String, !knownCompletedTasks.contains(id) else { return }
        session.resumeBackendTask(id, goal: Self.heading(result, empty: ""))
        receiveOutcome(result, announce: false, recordEvent: false)
    }

    private func recoverTask(_ taskID: String) async {
        guard !knownCompletedTasks.contains(taskID), !reconciledMissingTasks.contains(taskID) else { return }
        if session.backendTaskID == nil, session.activeTaskID == nil {
            session.resumeBackendTask(taskID, goal: "")
        }
        await reconcileMissingTask(taskID)
    }

    private func reconcileMissingTask(_ taskID: String) async {
        guard !knownCompletedTasks.contains(taskID), !reconciledMissingTasks.contains(taskID) else { return }
        let traceRevision = taskStateRevision
        do {
            let trace = try await rpc.call("trace.get", params: ["task_id": taskID])
            guard taskStateRevision == traceRevision, runningTasks[taskID] == nil,
                  !knownCompletedTasks.contains(taskID), !pendingApprovals.contains(where: { $0.taskID == taskID }) else { return }
            let outcome = trace["outcome"] as? [String: Any] ?? [:]
            if var result = outcome["result"] as? [String: Any], result["status"] != nil {
                result["task_id"] = taskID
                receiveOutcome(result, announce: false, recordEvent: false)
            } else {
                let notice = "This task is no longer running; FERB has no confirmed final response."
                if session.backendTaskID == taskID { session.detachUnavailableTask(notice) }
                workSessions = workSessions.filter { $0.value.backendTaskID != taskID }
                for turn in conversation.turns where turn.backendTaskID == taskID && ["working", "needs_user"].contains(turn.status) {
                    conversation.finish(id: turn.id, answer: turn.answer, status: "interrupted", artifacts: turn.artifacts,
                        displayText: turn.displayText, verified: turn.isVerified, inferred: true)
                }
                saveConversation()
            }
            reconciledMissingTasks.insert(taskID)
        } catch { connectionStatus = error.localizedDescription }
    }

    func selectRunningTask(_ taskID: String) {
        guard let goal = runningTasks[taskID] else { return }
        if let active = session.activeTaskID { workSessions[active] = session }
        if let stored = workSessions.values.first(where: { $0.backendTaskID == taskID }) {
            let draft = session.draft
            let sources = session.attachments
            session = stored
            session.clearComposer()
            session.draft = draft
            session.attach(sources)
        } else { session.resumeBackendTask(taskID, goal: goal) }
        preferences.set(taskID, forKey: "FERBActiveTaskID")
        expand()
    }

    /// Typed or dictated, a message is sent the same way: FERB replies in text only, never aloud (VOICE-409).
    func submitToDaemon(localID: UUID, text: String, sources: [URL]) {
        submissions[localID] = Task { [weak self] in
            guard let self else { return }
            defer {
                // Progress arrives only on this stream; once it ends, a live line would show a stale step.
                self.conversation.clearLiveLine(id: localID)
                self.finishSubmission(localID: localID)
                self.submissions.removeValue(forKey: localID)
            }
            do {
                if self.conversationID == nil { try await self.attachDaemon() }
                if !sources.isEmpty && !self.supportsAttachments {
                    self.receiveChatEvent(["type": "error", "text": "The installed FERB daemon needs an update before it can receive attachments. Your request was not sent."], localID: localID)
                    return
                }
                var params: [String: Any] = ["session_id": self.conversationID!, "text": text]
                if !sources.isEmpty { params["input_paths"] = sources.map(\.path) }
                for try await event in self.rpc.stream("chat.send", params: params) {
                    guard !Task.isCancelled else { return }
                    self.receiveChatEvent(event, localID: localID)
                }
                if self.conversation.turns.contains(where: { $0.id == localID && $0.isProvisional }) {
                    self.receiveChatEvent(["type": "reply_abort", "reason": "The stream ended before FERB confirmed its reply."], localID: localID)
                    self.withWorkSession(localID) {
                        self.session.waitForOwner("The response was interrupted. It has not been confirmed.")
                    }
                }
            } catch {
                self.receiveChatEvent(["type": "reply_abort", "reason": "Connection interrupted before FERB confirmed its reply."], localID: localID)
                self.withWorkSession(localID) {
                    self.session.waitForOwner(error.localizedDescription + " Check running tasks before resubmitting.")
                }
                self.connectionStatus = error.localizedDescription
                self.promptError = self.session.attentionMessage
                self.onBackgroundChange?(true)
                await self.refreshDaemonStatus()
            }
        }
    }

    func finishSubmission(localID: UUID) {
        withWorkSession(localID) {
            guard session.backendTaskID == nil else { return }
            acknowledgement.abort(turn: localID)
            let notice = conversation.turns.first(where: { $0.id == localID })?.responseNotice
                ?? "The response ended before FERB confirmed it. Check tasks before resubmitting."
            conversation.abortReply(id: localID, reason: notice)
            session.detachInterruptedReply(notice)
            pendingStops.remove(localID)
            saveConversation()
        }
    }

    func receiveChatEvent(_ event: [String: Any], localID: UUID) {
        let previousPlan = conversation.turns.first(where: { $0.id == localID })?.plan
        var applied = false
        withWorkSession(localID) { applied = handleChatEvent(event, localID: localID) }
        if (ResultCard.list(event["cards"]) ?? []).contains(where: { $0.type == "event" && $0.isTimer }) {
            Task { await refreshRunningTimers() }          // a timer was just set: the closed notch counts it down
        }
        // Presentation uses the restored foreground session, never a background task snapshot.
        if applied {
            switch event["type"] as? String {
            case "plan_delta":
                // The sticky window shows the plan line in the chat; the notch shows no compact preview then (APP-745).
                // APP-787: the closed notch never shows a task's reply while it streams: the daemon may still withdraw
                // it (a plan sent back for repair, `plan_abort`). It shows the reply once FERB accepted it (`plan`).
                if expanded || chatInWindow { acknowledgement.suppress(turn: localID) }
            case "plan_abort", "plan_end": break      // nothing streamed into the closed notch, so nothing to withdraw
            case "plan":
                if expanded || chatInWindow { acknowledgement.suppress(turn: localID) }
                else if let text = event["text"] as? String, text != previousPlan {
                    acknowledgement.finish(text, turn: localID)
                }
            default: break
            }
        }
        // An event the request gate dropped was never this turn's, so it can't be its follow-up either.
        if applied, event["type"] as? String == "clarify", conversation.turns.contains(where: { $0.id == localID && $0.status == "done" }) {
            followsToday = true
            selectedDay = Date()
            announceAttention("followup:" + localID.uuidString)
        }
    }

    /// Returns whether the event was applied to this turn.
    private func handleChatEvent(_ event: [String: Any], localID: UUID) -> Bool {
        guard session.activeTaskID == localID else { return false }
        // Every contract event names its request. One naming another turn's request is never applied to this turn.
        if let requestID = event["request_id"] as? String, !requestID.isEmpty,
           !conversation.bindRequest(id: localID, requestID: requestID) { return false }
        switch event["type"] as? String {
        case "plan_delta", "plan_abort", "plan_end":
            // Provisional presentation only; the canonical plan owns archive and task binding. The streamed plan line
            // shows where the plan event's plan line will be (one path, kb area app).
            guard session.backendTaskID == nil else { return false }
            switch event["type"] as? String {
            case "plan_delta": conversation.streamPlan(id: localID, delta: event["text"] as? String ?? "")
            case "plan_abort": conversation.abortPlanStream(id: localID)
            default: break
            }
        case "ack":
            let acknowledgement = event["text"] as? String ?? ""
            conversation.acknowledge(id: localID, text: acknowledgement, message: event["message"] as? String,
                                     hidden: event["hidden"] as? [String])
            saveConversation()
        case "plan":
            guard let id = event["task_id"] as? String else { return false }
            taskStateRevision += 1
            session.bindBackendTask(id, localID: localID)
            conversation.clearProvisionalReply(id: localID)
            conversation.bind(id: localID, taskID: id)
            // Several asks (ORCH-319): the asks answered at once, receipts, the plan line, and the card.
            conversation.showPlan(id: localID, text: event["text"] as? String ?? "", details: event)
            saveConversation()
            preferences.set(id, forKey: "FERBActiveTaskID")
            runningTasks[id] = Self.heading(event, empty: session.draft)
            if pendingStops.remove(localID) != nil { requestDaemonStop() }
        case "part":
            // A row of the card changed (ORCH-319). It fills quietly: no opening, no banner (APP-301).
            receivePart(event)
        case "progress":
            // Live state only, so it is not saved. The chat shows only the AI service connection's lines (marked
            // `connection`: offline, reconnected); ordinary steps ("opening …") stay out of it (owner, 2026-10-24:
            // only the "N active tasks" row) and remain in the task's Inspect view.
            if event["connection"] != nil {
                conversation.showProgress(id: localID, text: event["text"] as? String ?? "",
                    startedAt: LiveProgress.time(event["started_at"]) ?? LiveProgress.time(event["at"]) ?? Date())
            } else if let step = event["step_task_id"] as? String, !step.isEmpty,
                      conversation.turns.contains(where: { $0.card?.rows.contains { $0.stepTaskID == step } == true }) {
                // A card row's own live line (APP-718): what that step is doing now, shown in its row only.
                stepLive[step] = LiveProgress(text: event["text"] as? String ?? "",
                    startedAt: LiveProgress.time(event["started_at"]) ?? Date())
            }
        case "step":
            receiveStep(event)
        case "reply_delta":
            guard session.backendTaskID == nil else { return false }
            let delta = event["text"] as? String ?? ""
            if !delta.isEmpty, conversation.turns.first(where: { $0.id == localID })?.answer.isEmpty ?? true {
                requestScroll(TranscriptTarget.answer(turn: localID), .answer)
                presentIncomingAnswer()
            }
            conversation.append(id: localID, delta: delta)
        case "reply_abort":
            guard session.backendTaskID == nil else { return false }
            acknowledgement.abort(turn: localID)
            conversation.abortReply(id: localID, reason: event["reason"] as? String ?? "")
            saveConversation()
        case "reply", "clarify":
            // A reply that ended the turn on a failure says so (`status: failed`), and offers Send to developer (L35).
            let status = event["status"] as? String ?? "done"
            _ = session.finishBackendWork(taskID: nil, status: status, answer: event["text"] as? String ?? "", artifacts: [])
            if status == "done" { showCompletionTrace() }
            // FERB's question (clarify) sounds as its follow-up's needs you (receiveChatEvent), never as a completion.
            if event["type"] as? String == "reply", let sound = SoundPolicy.outcomeSound(status) { feedback.play(sound) }
            requestScroll(TranscriptTarget.answer(turn: localID), event["type"] as? String == "clarify" ? .needsYou : .answer)
            presentResult(event)
            recordAnswer(announce: true, displayText: event["display_text"] as? String)
            // A planner turn that ended on a usage limit or a sign-in problem offers another AI service (ORCH-315).
            conversation.offerRuntimeSwitch(id: localID, RuntimeSwitch(event["runtime_switch"]))
            // Step 8e: the cards of what code did with the turn's memory, reminders and schedules.
            conversation.showCards(id: localID, event["cards"])
            saveConversation()
        case "merged":
            // Read together with another message (ORCH-318): that message's reply answers this one too.
            conversation.merge(id: localID, into: event["into"] as? String ?? "")
            session.finishMerged()
            saveConversation()
            onBackgroundChange?(true)
        case "outcome": receiveOutcome(event)
        case "error":
            let message = event["text"] as? String ?? "FERB could not start this request."
            conversation.finish(id: localID, answer: message, status: "failed", artifacts: [])
            saveConversation()
            session.requireAttention(message)
            requestScroll(TranscriptTarget.answer(turn: localID), .answer)
            presentTerminalAnswer()
            promptError = message
            feedback.play(.error)
            onBackgroundChange?(true)
        case "subtask":
            let text = event["summary"] as? String ?? event["status"] as? String ?? "Task updated"
            if let taskID = session.backendTaskID { noteTaskActivity(taskID: taskID, title: text) }
            session.noteBackendActivity(text)
        case "verification":
            if let taskID = session.backendTaskID {
                noteTaskActivity(taskID: taskID, title: "Checking the result", symbol: "checkmark.shield")
            }
            session.noteBackendActivity("Checking the result", symbol: "checkmark.shield")
        case "memory", "timer": session.noteBackendActivity(event["text"] as? String ?? "Task updated")
        case "event":
            if event["topic"] as? String == "approval.requested" { receiveBusEvent(event) }
        default: break
        }
        return true
    }

    private func keepKnownFields(_ value: [String: Any]) -> [String: Any] {
        guard let known = pendingApprovals.first(where: { $0.id == value["approval_id"] as? String }) else { return value }
        var kept = value
        if let source = known.sourceTaskID { kept["source_task_id"] = source }
        if kept["part"] == nil, let part = known.part { kept["part"] = part }
        if kept["runtime_switch"] == nil, let offer = known.runtimeSwitch {
            kept["runtime_switch"] = ["runtime": offer.runtime, "problem": offer.problem, "role": offer.role,
                                      "choices": offer.choices]
        }
        return kept
    }

    func normalizeApproval(_ value: [String: Any]) -> DaemonApproval? {
        var normalized = value
        if let taskID = value["task_id"] as? String {
            normalized["source_task_id"] = value["source_task_id"] ?? taskID     // ties a step's question to its row
            var roots = Set(runningTasks.keys)
            roots.formUnion(workSessions.values.compactMap(\.backendTaskID))
            roots.formUnion(conversation.turns.compactMap(\.backendTaskID))
            if let current = session.backendTaskID { roots.insert(current) }
            if let root = roots.filter({ !$0.isEmpty && (taskID == $0 || taskID.hasPrefix($0 + ".")) })
                .max(by: { $0.count < $1.count }) {
                normalized["task_id"] = root
            }
        }
        return DaemonApproval(normalized)
    }

    func receiveBusEvent(_ event: [String: Any]) {
        let data = event["data"] as? [String: Any] ?? [:]
        let taskID = data["task_id"] as? String
        if let topic = event["topic"] as? String,
           ["approval.requested", "approval.resolved", "task.created", "task.result", "task.steered"].contains(topic) {
            taskStateRevision += 1
        }
        switch event["topic"] as? String {
        case "timer.fired":
            timerFired(data["timer_id"] as? String)
            Task { await refreshRunningTimers() }
        case "timer.changed":                            // DMN-548: set, cancelled (by any path) or fired
            receiveTimerChange(data)
        case "approval.requested":
            guard let approval = normalizeApproval(data), !pendingApprovals.contains(where: { $0.id == approval.id }) else { return }
            pendingApprovals.append(approval)
            if approval.taskID == session.backendTaskID, approval.taskID != nil { session.waitForOwner(approval.reason) }
            announceApproval(approval)
            onBackgroundChange?(true)
        case "approval.resolved":
            resolveApproval(data["approval_id"] as? String)
            promptError = nil
            onBackgroundChange?(true)
        case "task.steered":
            if let taskID, data["goal"] is String || data["name"] is String { runningTasks[taskID] = Self.heading(data) }
        case "task.created":
            if let taskID {
                runningTasks[taskID] = Self.heading(data)
                taskActivities[taskID] = []
                knownCompletedTasks.remove(taskID)
                reconciledMissingTasks.remove(taskID)
            }
        case "task.result":
            var result = data
            if let taskID, let name = runningTasks.removeValue(forKey: taskID), result["goal"] == nil,
               result["name"] == nil { result["name"] = name }
            if let taskID { taskActivities.removeValue(forKey: taskID) }
            if result["at"] == nil { result["at"] = event["at"] }
            receiveOutcome(result)
            if let taskID { cursor?.end(taskID: taskID) }
            // A result no attached chat showed would be replayed by `ferb chat`; this chat showed it.
            if let taskID, data["shown"] as? Bool == false { acknowledgeResults([taskID]) }
        case "cursor.point":
            if let taskID, let kind = data["kind"] as? String,
               let r = (data["rect"] as? [Any])?.compactMap({ ($0 as? NSNumber)?.doubleValue }), r.count == 4 {
                let display = (data["display"] as? NSNumber)?.uint32Value ?? CGMainDisplayID()
                cursor?.point(taskID: taskID, kind: kind, rect: CGRect(x: r[0], y: r[1], width: r[2], height: r[3]),
                              display: display)
            }
        case "tool.called":
            guard let taskID else { return }
            let matches: (String?) -> Bool = { root in
                guard let root else { return false }
                return taskID == root || taskID.hasPrefix(root + ".")
            }
            let title = data["tool"] as? String ?? "Working"
            var roots = Set(runningTasks.keys)
            roots.formUnion(workSessions.values.compactMap(\.backendTaskID))
            if let current = session.backendTaskID { roots.insert(current) }
            let activityID = roots.filter { taskID == $0 || taskID.hasPrefix($0 + ".") }
                .max(by: { $0.count < $1.count }) ?? taskID
            noteTaskActivity(taskID: activityID, title: title, symbol: "globe")
            if matches(session.backendTaskID) { session.noteBackendActivity(title) }
            else if let local = workSessions.first(where: { matches($0.value.backendTaskID) })?.key {
                withWorkSession(local) { session.noteBackendActivity(title) }
            }
        case "task.part":
            receivePart(data)
        case "task.step":
            receiveStep(data)
        case "task.resumed_event":
            if taskID == session.backendTaskID, data["type"] as? String == "reply" {
                session.noteBackendActivity(data["text"] as? String ?? "Task resumed")
            }
            // A multi-part task resumed after a restart: its card from the record (its rows' changes come as task.part).
            if data["type"] as? String == "plan", let taskID, let card = TurnCard(data["card"], taskID: taskID),
               conversation.showCard(card, goal: data["goal"] != nil || data["name"] != nil
                                     ? Self.heading(data, empty: "") : runningTasks[taskID] ?? "") {
                saveConversation()
                onBackgroundChange?(true)
            }
        case "loop.status":
            // A loop's state change (L31). A finished or stopped loop leaves; a status arriving after its task's
            // result never brings the row back.
            guard let taskID else { return }
            if runningTasks[taskID] != nil, let work = StandingWork(loopStatus: data) { standingWork[taskID] = work }
            else { standingWork.removeValue(forKey: taskID) }
        case "attention.notice":
            receiveNotices([data])
        case "bus.overflow", "bus.dropped":
            // Events were lost; re-read the state they would have changed.
            Task { await refreshDaemonStatus() }
        default: break
        }
    }

    /// A row's change (ORCH-319), from the turn's stream or the bus: applied once, by its seq, to the card that
    /// shows it. Rows fill quietly: the notch opens only for a question or the task's end (APP-301).
    func receivePart(_ event: [String: Any]) {
        guard let turnID = conversation.applyPart(event) else { return }
        saveConversation()
        onBackgroundChange?(true)
        // A filled row never moves the view: a pill names it when it is out of sight (APP-720).
        if let part = event["part"] as? String, ["done", "updated", "failed"].contains(event["state"] as? String ?? "") {
            let label = conversation.turns.first { $0.id == turnID }?.card?.row(part)?.label ?? ""
            requestScroll(TranscriptTarget.row(turn: turnID, part: part), .quietFill, label: label)
        }
    }

    /// A plan step's state (ORCH-531): its row's words and spinner. A step no longer running drops its live line.
    func receiveStep(_ event: [String: Any]) {
        guard conversation.applyStep(event) != nil else { return }
        if let step = event["step_task_id"] as? String, event["status"] as? String != "running" {
            stepLive.removeValue(forKey: step)
        }
    }

    /// A task's end brings its card's footer, or its answer, forward (APP-720); a question it ended on, as a question.
    func requestOutcomeScroll(taskID: String, requestID: String?, status: String) {
        guard let turn = conversation.turns.last(where: { requestID != nil && $0.requestID == requestID })
                ?? conversation.turns.last(where: { $0.backendTaskID == taskID }) else { return }
        if turn.card != nil {
            requestScroll(TranscriptTarget.footer(turn: turn.id), status == "needs_user" ? .needsYou : .taskDone)
        } else {
            requestScroll(TranscriptTarget.answer(turn: turn.id), status == "needs_user" ? .needsYou : .answer)
        }
    }

    /// The runtime switch offered with a failed answer (ORCH-315): the role that failed uses `runtime` from
    /// its next call (`runtimes.use`); the task already ended, so the owner sends the request again.
    func useRuntime(_ runtime: String, for offer: RuntimeSwitch, turn: UUID) {
        guard runtimeChoices[turn] == nil else { return }
        runtimeChoices[turn] = "Switching to \(RuntimeSwitch.label(runtime))…"
        Task {
            do {
                _ = try await rpc.call("runtimes.use", params: ["runtime": runtime, "roles": [offer.role]])
                runtimeChoices[turn] = "FERB uses \(RuntimeSwitch.label(runtime)) now. Send your request again to retry it."
            } catch {
                runtimeChoices[turn] = nil
                promptError = error.localizedDescription
            }
        }
    }

    func receiveOutcome(_ event: [String: Any], announce: Bool = true, recordEvent: Bool = true) {
        guard let taskID = event["task_id"] as? String else { return }
        if recordEvent { taskStateRevision += 1 }
        // The answer's own fields (ORCH-319) on its turn, before and after it is filed: a card fills in place.
        let placed = conversation.applyOutcome(event)
        defer {
            if conversation.applyOutcome(event) || placed { saveConversation() }
        }
        // The task ended, so its loops did too (L31), and its rows' live lines.
        standingWork.removeValue(forKey: taskID)
        stepLive = stepLive.filter { !$0.key.hasPrefix(taskID + ".") }
        if session.backendTaskID == taskID, session.activeTaskID != nil {
            handleOutcome(event, announce: announce)
            taskActivities.removeValue(forKey: taskID)
            return
        }
        if let localID = workSessions.first(where: { $0.value.backendTaskID == taskID })?.key {
            withWorkSession(localID) { handleOutcome(event, announce: announce) }
            taskActivities.removeValue(forKey: taskID)
        } else if let status = event["status"] as? String {
            let fresh = !knownCompletedTasks.contains(taskID)
            let artifacts = (event["artifacts"] as? [String] ?? []).filter { $0.hasPrefix("/") }.map { URL(fileURLWithPath: $0) }
            if let turn = (event["request_id"] as? String).flatMap(conversation.turn(requestID:))
                ?? conversation.turns.first(where: { $0.backendTaskID == taskID }) {
                // Bound here too, so Tasks and Inspect reach a turn found by its request.
                conversation.bind(id: turn.id, taskID: taskID)
                conversation.finish(id: turn.id, answer: event["answer"] as? String ?? event["summary"] as? String ?? turn.answer,
                    status: status, artifacts: artifacts, displayText: event["display_text"] as? String,
                    verified: event["verified"] as? Bool ?? false)
            } else {
                // A task this chat has no turn for (started elsewhere, or before a restart): FERB's message,
                // described by the daemon's goal and filed when its result arrived.
                conversation.recordRecovered(taskID: taskID, goal: Self.heading(event, empty: ""),
                    answer: event["answer"] as? String ?? event["summary"] as? String ?? "", status: status,
                    artifacts: artifacts, displayText: event["display_text"] as? String,
                    verified: event["verified"] as? Bool ?? false, at: LiveProgress.time(event["at"]) ?? Date())
            }
            knownCompletedTasks.insert(taskID)
            // A task this chat didn't start (a schedule's run, one from before a restart): done is a notification; a
            // failure or a wait on the owner sounds as itself (its banner then posts silently).
            if announce && fresh {
                if SoundPolicy.isDone(status) { feedback.notification(for: [taskID], bannerMayFollow: true) }
                else if let sound = SoundPolicy.outcomeSound(status) { feedback.play(sound, for: taskID) }
            }
            if announce && fresh && Self.opensNotch(status) {
                requestOutcomeScroll(taskID: taskID, requestID: event["request_id"] as? String, status: status)
                presentResult(event)
            }
            runningTasks.removeValue(forKey: taskID)
            taskActivities.removeValue(forKey: taskID)
            saveConversation()
        }
    }

    /// Marks shown, one by one, the results this chat placed (`results.ack`), so `ferb chat` doesn't replay them.
    /// Saved first: a result is acknowledged only once it is in the saved conversation.
    func acknowledgeResults(_ taskIDs: [String]) {
        let placed = taskIDs.filter { id in
            conversation.turns.contains { $0.backendTaskID == id && !["working", "interrupted"].contains($0.status) }
        }
        guard !placed.isEmpty else { return }
        Task { _ = try? await rpc.call("results.ack", params: ["task_ids": placed]) }
    }

    /// A task's heading (T-1055): the daemon's short name, else its goal's first clause, cut; `empty` when it has
    /// neither. The full goal shows only where the owner opens the task in the FERB window.
    static func heading(_ row: [String: Any], empty: String = "Running task") -> String {
        let name = ShortName.of(row, fallback: "goal")
        return name.isEmpty ? empty : name
    }

    /// Reminder notifications retain their timer identity in chat, so redelivery cannot repeat the arrival. The card's
    /// heading is the reminder's short name and says timer or reminder from its row's kind (`card`, T-1055).
    func receiveReminder(id: String, text: String, card: ResultCard? = nil, at: Any? = nil) {
        guard !id.isEmpty, !text.isEmpty else { return }
        var shown: [String: Any] = ["type": "reminder", "title": card?.title ?? ShortName.clause(text),
                                    "kind": card?.kind ?? "reminder"]
        if let due = card?.due { shown["due"] = due }
        if let started = card?.started { shown["started"] = started }    // a timer's: its done alert repeats its duration
        if let first = card?.repeatOf { shown["repeat_of"] = first }      // a Repeat's: its first timer's box (APP-769)
        shown["timer_id"] = card?.timerID ?? id
        if card?.isTimer == true { timerFired(card?.timerID ?? id) }      // its arrival: it fired (DMN-548)
        receiveNotices([["id": "reminder:" + id, "text": text, "at": at ?? ISO8601DateFormatter().string(from: Date()),
                         "banner": true, "cards": [shown]]], soundIDs: [id])
    }

    /// Notices go into the chat (L9) and are saved before they are acknowledged, so a shown notice is never lost. Every
    /// delivered id is acknowledged, including one shown earlier whose acknowledgement didn't arrive.
    func receiveNotices(_ rows: [[String: Any]], soundIDs: [String]? = nil) {
        var ids: [String] = [], added: [String] = [], bannered = false
        for row in rows {
            guard let id = row["id"] as? String, !id.isEmpty else { continue }
            // A notice with nothing to say is still acknowledged, so it isn't delivered again on every read.
            guard let text = row["text"] as? String, !text.isEmpty else { ids.append(id); continue }
            let quotes = (row["quotes"] as? [[String: Any]] ?? []).compactMap { quote -> NoticeQuote? in
                guard let text = quote["text"] as? String, !text.isEmpty else { return nil }
                return NoticeQuote(text: text, origin: quote["origin"] as? String ?? "")
            }
            if conversation.addNotice(id: id, text: text, quotes: quotes, noticedAt: LiveProgress.time(row["at"]),
                                      cards: ResultCard.list(row["cards"])) {
                if let card = CardLayout.above(ResultCard.list(row["cards"]) ?? []).first {
                    presentReminder(card: card, text: text)
                }
                added.append(id)
                bannered = bannered || row["banner"] as? Bool == true    // the daemon banners it too
            }
            ids.append(id)
        }
        guard !ids.isEmpty else { return }
        // One notification sound for the batch (a restart's backlog is one sound), carried by a banner when it has one.
        feedback.notification(for: added.isEmpty ? [] : soundIDs ?? added, bannerMayFollow: bannered)
        if !added.isEmpty {
            saveConversation()
            onBackgroundChange?(true)
        }
        Task { _ = try? await rpc.call("attention.ack", params: ["ids": ids]) }
    }

    /// The day's learning digest (`memory.learning {op: digest}`, the read `ferb chat` makes when it opens). It marks
    /// the digest seen for the day, so it is read only while the owner has today's chat open, and shown at once.
    func fetchLearningDigest() async {
        guard let reply = try? await rpc.call("memory.learning", params: ["op": "digest"]),
              let digest = reply["digest"] as? [String: Any] else { return }
        let proposals = learningProposals(digest["proposals"], kind: nil)
        guard !proposals.isEmpty else { return }
        let day = digest["day"] as? String ?? proposals.map(\.id).joined(separator: ",")
        showLearningNotice(id: "learned:" + day, text: "Here are things I learned that need your confirmation:",
                           proposals: proposals)
    }

    /// FERB's guesses that were waiting before it asked about each at once (L36): offered once as one list
    /// (`memory.learning {op: backlog}`, which marks it offered), shown at once in today's chat.
    func fetchGuessBacklog() async {
        guard let reply = try? await rpc.call("memory.learning", params: ["op": "backlog"]),
              let backlog = reply["backlog"] as? [String: Any], let id = backlog["id"] as? String,
              let text = backlog["text"] as? String else { return }
        let proposals = learningProposals(backlog["proposals"], kind: "guess")
        guard !proposals.isEmpty else { return }
        showLearningNotice(id: "guesses:" + id, text: text, proposals: proposals)
    }

    /// The items of a learning notice from a daemon reply; `kind` marks what they are ("guess", or nil for a digest item).
    private func learningProposals(_ items: Any?, kind: String?) -> [LearningProposal] {
        (items as? [[String: Any]] ?? []).compactMap { item -> LearningProposal? in
            guard let id = item["id"] as? String, !id.isEmpty, let text = item["text"] as? String else { return nil }
            return LearningProposal(id: id, text: text, kind: kind)
        }
    }

    private func showLearningNotice(id: String, text: String, proposals: [LearningProposal]) {
        guard changeConversation({ $0.addNotice(id: id, text: text, tag: nil, proposals: proposals) }) else { return }
        saveConversation()
        onBackgroundChange?(true)
    }

    /// The owner's answer to one learned item: Confirm keeps it (`confirm`); Not right retires it so it isn't proposed
    /// again (`retire`, a state change, not a deletion).
    func decideLearning(noticeID: String, proposalID: String, confirm: Bool) {
        settleLearning(noticeID: noticeID, proposalID: proposalID, params: ["op": confirm ? "confirm" : "retire"],
                       decision: confirm ? "confirmed" : "not right",
                       failure: "That answer about what FERB learned wasn't recorded. Try again.")
    }

    /// The owner's answer to one older guess: Keep, Drop, or their own words saved instead (`edit`).
    /// `done(true)` once recorded, `done(false)` when it wasn't (a refused edit keeps the owner's typed words).
    func decideGuess(noticeID: String, proposalID: String, action: String, text: String?,
                     done: ((Bool) -> Void)? = nil) {
        var params: [String: Any] = ["op": "guess", "action": action]
        if let text { params["text"] = text }
        settleLearning(noticeID: noticeID, proposalID: proposalID, params: params,
                       decision: ["keep": "kept", "drop": "dropped", "edit": "edited"][action] ?? action,
                       failure: "That answer about FERB's guess wasn't recorded. Try again.", guess: true, done: done)
    }

    private func settleLearning(noticeID: String, proposalID: String, params: [String: Any], decision: String,
                                failure: String, guess: Bool = false, done: ((Bool) -> Void)? = nil) {
        Task {
            do {
                let reply = try await rpc.call("memory.learning", params: params.merging(["id": proposalID]) { $1 })
                // A guess shows how it stands now: one already settled shows what happened to it, not this answer;
                // one FERB says isn't settled (still waiting) wasn't changed by it (review Q-M2).
                if guess, reply["state"] != nil, settledGuess(reply) == nil {
                    promptError = failure
                    done?(false)
                    return
                }
                conversation.decide(noticeID: noticeID, proposalID: proposalID, decision: settledGuess(reply) ?? decision)
                saveConversation()
                done?(true)
            } catch {
                // FERB's own reason for refusing an answer (L36) is shown. A fault or a lost reply may still have
                // recorded it: a guess FERB says is settled shows that (live 2026-10-25); otherwise the generic line.
                if case DaemonRPCError.remote(let reason, _) = error {
                    promptError = reason
                } else if guess, let reply = try? await rpc.call("memory.learning", params: ["op": "guess_state", "id": proposalID]),
                          let state = settledGuess(reply) {
                    conversation.decide(noticeID: noticeID, proposalID: proposalID, decision: state)
                    saveConversation()
                    done?(true)
                    return
                } else {
                    promptError = failure
                }
                done?(false)
            }
        }
    }

    /// How a guess stands by FERB's reply (`state`), when it is no longer waiting for the owner.
    private func settledGuess(_ reply: [String: Any]) -> String? {
        guard let state = reply["state"] as? String, ["kept", "dropped", "edited"].contains(state) else { return nil }
        return state
    }

    /// An answered question no longer holds its task: that task's work resumes, whichever turn is in front.
    func resolveApproval(_ approvalID: String?) {
        if let approvalID { onApprovalResolved?(approvalID) }
        let resolved = pendingApprovals.first { $0.id == approvalID }
        pendingApprovals.removeAll { $0.id == approvalID }
        if questionFocus?.id == approvalID { questionFocus = nil }
        guard let taskID = resolved?.taskID else {
            if pendingApprovals.isEmpty { session.resumeAfterOwnerAnswer() }
            return
        }
        guard !pendingApprovals.contains(where: { $0.taskID == taskID }) else { return }
        if session.backendTaskID == taskID { session.resumeAfterOwnerAnswer() }
        else if let local = workSessions.first(where: { $0.value.backendTaskID == taskID })?.key {
            withWorkSession(local) { session.resumeAfterOwnerAnswer() }
        }
    }

    private func handleOutcome(_ event: [String: Any], announce: Bool) {
        guard let id = event["task_id"] as? String, id == session.backendTaskID else { return }
        let alreadyAnnounced = knownCompletedTasks.contains(id)
        let status = event["status"] as? String ?? "failed"
        let answer = event["answer"] as? String ?? event["summary"] as? String ?? "No answer was recorded."
        guard session.finishBackendWork(taskID: id, status: status, answer: answer,
                                       artifacts: event["artifacts"] as? [String] ?? []) else { return }
        workSessions = workSessions.filter { $0.value.backendTaskID != id }
        knownCompletedTasks.insert(id)
        runningTasks.removeValue(forKey: id)
        stoppingTaskIDs.remove(id)
        if announce && !alreadyAnnounced, let sound = SoundPolicy.outcomeSound(status) { feedback.play(sound, for: id) }
        if announce && !alreadyAnnounced && status == "done" { showCompletionTrace() }
        if announce && !alreadyAnnounced && Self.opensNotch(status) {
            requestOutcomeScroll(taskID: id, requestID: event["request_id"] as? String, status: status)
            presentResult(event)
        }
        let verified = event["verified"] as? Bool ?? false
        recordAnswer(announce: announce, displayText: event["display_text"] as? String, verified: verified)
        for turn in conversation.turns where turn.backendTaskID == id && ["working", "interrupted", "needs_user"].contains(turn.status) {
            conversation.finish(id: turn.id, answer: answer, status: status, artifacts: session.result?.artifacts ?? [],
                displayText: event["display_text"] as? String, verified: verified)
        }
        saveConversation()
    }

    func requestDaemonStop() {
        acknowledgement.dismiss()
        guard let taskID = session.backendTaskID else {
            if let localID = session.activeTaskID { pendingStops.insert(localID) }
            else { return }
            session.noteBackendActivity("Stop requested; waiting for task identity", symbol: "pause")
            return
        }
        guard !taskID.isEmpty else { return }
        let localID = session.activeTaskID
        Task {
            do {
                _ = try await rpc.call("task.stop", params: ["task_ids": [taskID]])
                if let localID { withWorkSession(localID) { session.noteBackendActivity("Stop requested", symbol: "pause") } }
                await refreshDaemonStatus()
            } catch { promptError = error.localizedDescription }
        }
    }

    func answerApproval(_ approval: DaemonApproval, text: String) {
        guard !sendingAnswer else { return }
        sendingAnswer = true
        promptError = nil
        Task {
            do {
                _ = try await rpc.call("chat.answer", params: ["approval_id": approval.id, "text": text])
                taskStateRevision += 1
                resolveApproval(approval.id)
            } catch {
                if case DaemonRPCError.remote(let reason, _) = error {
                    promptError = reason        // FERB refused this answer and says why (L36); the question waits
                } else {
                    // Do not replay an answer after disconnect: it may already have been consumed.
                    promptError = "The answer could not be confirmed. Rechecking the pending request."
                }
                await refreshDaemonStatus()
            }
            sendingAnswer = false
            onBackgroundChange?(true)
        }
    }

    /// The owner's words in a safety check's box (GATE-510): a correction of that check's task, never a Yes. The card
    /// goes at once; a refusal (the question closed, no task) brings back what the daemon still has open.
    func correctApproval(_ approval: DaemonApproval, text: String) {
        guard !sendingAnswer, approval.corrects, !approval.isSecret else { return }
        sendingAnswer = true
        promptError = nil
        resolveApproval(approval.id)
        Task {
            do {
                _ = try await rpc.call("chat.correct", params: ["approval_id": approval.id, "text": text])
                taskStateRevision += 1
            } catch {
                if case DaemonRPCError.remote(let reason, _) = error { promptError = reason }
                else { promptError = "The message could not be confirmed. Rechecking the pending request." }
                await refreshDaemonStatus()
            }
            sendingAnswer = false
            onBackgroundChange?(true)
        }
    }

    func chooseApproval(_ approval: DaemonApproval, index: Int) {
        guard !sendingAnswer, approval.options.indices.contains(index), !approval.isSecret else { return }
        sendingAnswer = true
        promptError = nil
        Task {
            do {
                _ = try await rpc.call("chat.approve", params: ["approval_id": approval.id, "granted": true, "choice": index + 1])
                taskStateRevision += 1
                resolveApproval(approval.id)
            } catch {
                promptError = "Could not confirm your choice. Rechecking the request."
                await refreshDaemonStatus()
            }
            sendingAnswer = false
            onBackgroundChange?(true)
        }
    }

    func declineApproval(_ approval: DaemonApproval) {
        guard !sendingAnswer else { return }
        sendingAnswer = true
        Task {
            do {
                _ = try await rpc.call("chat.approve", params: ["approval_id": approval.id, "granted": false])
                taskStateRevision += 1
                resolveApproval(approval.id)
            } catch { promptError = "Could not confirm cancellation. Rechecking the request."; await refreshDaemonStatus() }
            sendingAnswer = false
            onBackgroundChange?(true)
        }
    }

    /// A finished task opens the notch by itself, without taking focus (APP-301): an answer, an unconfirmed answer,
    /// a failure, or a question. The owner's own stop doesn't.
    static func opensNotch(_ status: String) -> Bool { ["done", "done_unverified", "failed", "needs_user"].contains(status) }

    /// The socket refused or isn't there: FERB's daemon isn't running (or is restarting).
    static func unreachable(_ error: Error) -> Bool {
        if case DaemonRPCError.unavailable = error { return true }
        return false
    }
}
