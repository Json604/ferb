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
                    await self.refreshDaemonStatus()
                    for try await event in self.rpc.stream("logs.subscribe", params: [:]) {
                        guard !Task.isCancelled else { return }
                        self.receiveBusEvent(event)
                    }
                    self.connectionStatus = "Reconnecting to FERB…"
                } catch {
                    self.connectionStatus = error.localizedDescription
                }
                do { try await Task.sleep(for: .seconds(2)) } catch { return }
            }
        }
        reconciliationTask = Task { [weak self] in
            while !Task.isCancelled {
                do { try await Task.sleep(for: .seconds(5)) } catch { return }
                await self?.refreshDaemonStatus()
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
        connectionStatus = "Connected to FERB"
    }

    func refreshDaemonStatus() async {
        guard !refreshingStatus else { return }
        refreshingStatus = true
        // Before the snapshot below, so this extra read never makes it stale.
        if bodyVisible && viewingToday { await fetchLearningDigest() }
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
            let results = status["results"] as? [[String: Any]] ?? []
            for result in results { receiveOutcome(result, announce: false, recordEvent: false) }
            acknowledgeResults(results.compactMap { $0["task_id"] as? String })
            receiveNotices(status["notices"] as? [[String: Any]] ?? [])
            // A newer event wins over this older request's snapshot. The existing
            // reconciliation cadence will fetch a fresh snapshot next time.
            guard taskStateRevision == snapshotRevision else { return }
            connectionStatus = "Connected to FERB"
            let running = status["running"] as? [[String: Any]] ?? []
            runningTasks = Dictionary(running.compactMap { row in
                guard let id = row["task_id"] as? String, !id.isEmpty else { return nil }
                return (id, row["goal"] as? String ?? "Running task")
            }, uniquingKeysWith: { _, last in last })
            let approvals = (status["approvals"] as? [[String: Any]] ?? []).compactMap(normalizeApproval)
            let changed = pendingApprovals != approvals
            pendingApprovals = approvals
            if changed { promptError = nil; onBackgroundChange?(true) }
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
                    if !approvals.contains(where: { $0.taskID == taskID }) { session.resumeAfterOwnerAnswer() }
                } else if let result = results.first(where: { $0["task_id"] as? String == taskID }) {
                    receiveOutcome(result, announce: true, recordEvent: false)
                } else {
                    await recoverTask(taskID)
                    guard taskStateRevision == snapshotRevision else { return }
                }
            }
            if let approval = approvals.first(where: { $0.taskID == session.backendTaskID && $0.taskID != nil }) { session.waitForOwner(approval.reason) }
        } catch { connectionStatus = error.localizedDescription }
    }

    /// The daemon's goal is shown as FERB's description of the task, never as the owner's message; empty when unknown.
    private func restoreResult(_ result: [String: Any]) {
        guard let id = result["task_id"] as? String, !knownCompletedTasks.contains(id) else { return }
        session.resumeBackendTask(id, goal: result["goal"] as? String ?? "")
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
                        displayText: turn.displayText, verified: turn.isVerified)
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
            let notice = conversation.turns.first(where: { $0.id == localID })?.responseNotice
                ?? "The response ended before FERB confirmed it. Check tasks before resubmitting."
            conversation.abortReply(id: localID, reason: notice)
            session.detachInterruptedReply(notice)
            pendingStops.remove(localID)
            saveConversation()
        }
    }

    func receiveChatEvent(_ event: [String: Any], localID: UUID) {
        var applied = false
        withWorkSession(localID) { applied = handleChatEvent(event, localID: localID) }
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
        case "ack":
            conversation.acknowledge(id: localID, text: event["text"] as? String ?? "", message: event["message"] as? String)
            saveConversation()
        case "plan":
            guard let id = event["task_id"] as? String else { return false }
            taskStateRevision += 1
            session.bindBackendTask(id, localID: localID)
            conversation.clearProvisionalReply(id: localID)
            conversation.bind(id: localID, taskID: id)
            conversation.showPlan(id: localID, text: event["text"] as? String ?? "")
            saveConversation()
            preferences.set(id, forKey: "FERBActiveTaskID")
            runningTasks[id] = event["goal"] as? String ?? session.draft
            if pendingStops.remove(localID) != nil { requestDaemonStop() }
        case "progress":
            // Live state only, so it is not saved.
            conversation.showProgress(id: localID, text: event["text"] as? String ?? "",
                startedAt: LiveProgress.time(event["started_at"]) ?? LiveProgress.time(event["at"]) ?? Date())
        case "reply_delta":
            guard session.backendTaskID == nil else { return false }
            conversation.append(id: localID, delta: event["text"] as? String ?? "")
        case "reply_abort":
            guard session.backendTaskID == nil else { return false }
            conversation.abortReply(id: localID, reason: event["reason"] as? String ?? "")
            saveConversation()
        case "reply", "clarify":
            _ = session.finishBackendWork(taskID: nil, status: "done", answer: event["text"] as? String ?? "", artifacts: [])
            feedback.play(.completed)
            showCompletionTrace()
            recordAnswer(announce: true, displayText: event["display_text"] as? String)
        case "outcome": receiveOutcome(event)
        case "error":
            let message = event["text"] as? String ?? "FERB could not start this request."
            conversation.finish(id: localID, answer: message, status: "failed", artifacts: [])
            saveConversation()
            session.requireAttention(message)
            promptError = message
            feedback.play(.attention)
            onBackgroundChange?(true)
        case "subtask":
            let text = event["summary"] as? String ?? event["status"] as? String ?? "Task updated"
            session.noteBackendActivity(text)
        case "verification": session.noteBackendActivity("Checking the result", symbol: "checkmark.shield")
        case "memory", "timer": session.noteBackendActivity(event["text"] as? String ?? "Task updated")
        case "event":
            if event["topic"] as? String == "approval.requested" { receiveBusEvent(event) }
        default: break
        }
        return true
    }

    func normalizeApproval(_ value: [String: Any]) -> DaemonApproval? {
        var normalized = value
        if let taskID = value["task_id"] as? String {
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
            if let taskID, let goal = data["goal"] as? String { runningTasks[taskID] = goal }
        case "task.created":
            if let taskID {
                runningTasks[taskID] = data["goal"] as? String ?? "Running task"
                knownCompletedTasks.remove(taskID)
                reconciledMissingTasks.remove(taskID)
            }
        case "task.result":
            var result = data
            if let taskID, let goal = runningTasks.removeValue(forKey: taskID), result["goal"] == nil { result["goal"] = goal }
            if result["at"] == nil { result["at"] = event["at"] }
            receiveOutcome(result)
            // A result no attached chat showed would be replayed by `ferb chat`; this chat showed it.
            if let taskID, data["shown"] as? Bool == false { acknowledgeResults([taskID]) }
        case "tool.called":
            guard let taskID else { return }
            let matches: (String?) -> Bool = { root in
                guard let root else { return false }
                return taskID == root || taskID.hasPrefix(root + ".")
            }
            let title = data["tool"] as? String ?? "Working"
            if matches(session.backendTaskID) { session.noteBackendActivity(title) }
            else if let local = workSessions.first(where: { matches($0.value.backendTaskID) })?.key {
                withWorkSession(local) { session.noteBackendActivity(title) }
            }
        case "task.resumed_event":
            if taskID == session.backendTaskID, data["type"] as? String == "reply" {
                session.noteBackendActivity(data["text"] as? String ?? "Task resumed")
            }
        case "attention.notice":
            receiveNotices([data])
        case "bus.overflow", "bus.dropped":
            // Events were lost; re-read the state they would have changed.
            Task { await refreshDaemonStatus() }
        default: break
        }
    }

    func receiveOutcome(_ event: [String: Any], announce: Bool = true, recordEvent: Bool = true) {
        guard let taskID = event["task_id"] as? String else { return }
        if recordEvent { taskStateRevision += 1 }
        if session.backendTaskID == taskID, session.activeTaskID != nil { handleOutcome(event, announce: announce); return }
        if let localID = workSessions.first(where: { $0.value.backendTaskID == taskID })?.key {
            withWorkSession(localID) { handleOutcome(event, announce: announce) }
        } else if let status = event["status"] as? String {
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
                conversation.recordRecovered(taskID: taskID, goal: event["goal"] as? String ?? "",
                    answer: event["answer"] as? String ?? event["summary"] as? String ?? "", status: status,
                    artifacts: artifacts, displayText: event["display_text"] as? String,
                    verified: event["verified"] as? Bool ?? false, at: LiveProgress.time(event["at"]) ?? Date())
            }
            knownCompletedTasks.insert(taskID)
            runningTasks.removeValue(forKey: taskID)
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

    /// Notices go into the chat (L9) and are saved before they are acknowledged, so a shown notice is never lost. Every
    /// delivered id is acknowledged, including one shown earlier whose acknowledgement didn't arrive.
    func receiveNotices(_ rows: [[String: Any]]) {
        var ids: [String] = [], added = false
        for row in rows {
            guard let id = row["id"] as? String, !id.isEmpty else { continue }
            // A notice with nothing to say is still acknowledged, so it isn't delivered again on every read.
            guard let text = row["text"] as? String, !text.isEmpty else { ids.append(id); continue }
            let quotes = (row["quotes"] as? [[String: Any]] ?? []).compactMap { quote -> NoticeQuote? in
                guard let text = quote["text"] as? String, !text.isEmpty else { return nil }
                return NoticeQuote(text: text, origin: quote["origin"] as? String ?? "")
            }
            if conversation.addNotice(id: id, text: text, quotes: quotes, noticedAt: LiveProgress.time(row["at"])) { added = true }
            ids.append(id)
        }
        guard !ids.isEmpty else { return }
        if added {
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
        let proposals = (digest["proposals"] as? [[String: Any]] ?? []).compactMap { proposal -> LearningProposal? in
            guard let id = proposal["id"] as? String, !id.isEmpty, let text = proposal["text"] as? String else { return nil }
            return LearningProposal(id: id, text: text)
        }
        guard !proposals.isEmpty else { return }
        let day = digest["day"] as? String ?? proposals.map(\.id).joined(separator: ",")
        guard conversation.addNotice(id: "learned:" + day, text: "Here are things I learned that need your confirmation:",
                                     tag: nil, proposals: proposals) else { return }
        saveConversation()
        onBackgroundChange?(true)
    }

    /// The owner's answer to one learned item: Confirm keeps it (`confirm`); Not right retires it so it isn't proposed
    /// again (`retire`, a state change, not a deletion).
    func decideLearning(noticeID: String, proposalID: String, confirm: Bool) {
        Task {
            do {
                _ = try await rpc.call("memory.learning", params: ["op": confirm ? "confirm" : "retire", "id": proposalID])
                conversation.decide(noticeID: noticeID, proposalID: proposalID, decision: confirm ? "confirmed" : "not right")
                saveConversation()
            } catch { promptError = "That answer about what FERB learned wasn't recorded. Try again." }
        }
    }

    /// An answered question no longer holds its task: that task's work resumes, whichever turn is in front.
    func resolveApproval(_ approvalID: String?) {
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
        if announce && !alreadyAnnounced && status == "done" { feedback.play(.completed); showCompletionTrace() }
        else if announce && !alreadyAnnounced && status != "cancelled" { feedback.play(.attention) }
        let verified = event["verified"] as? Bool ?? false
        recordAnswer(announce: announce, displayText: event["display_text"] as? String, verified: verified)
        for turn in conversation.turns where turn.backendTaskID == id && ["working", "interrupted", "needs_user"].contains(turn.status) {
            conversation.finish(id: turn.id, answer: answer, status: status, artifacts: session.result?.artifacts ?? [],
                displayText: event["display_text"] as? String, verified: verified)
        }
        saveConversation()
    }

    func requestDaemonStop() {
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
                // Do not replay an answer after disconnect: it may already have been consumed.
                promptError = "The answer could not be confirmed. Rechecking the pending request."
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
}
