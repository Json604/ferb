import Foundation
import NotchCore

/// The temporary chat's housekeeping (TemporaryChat): which reads and taps are newer, the streams of the messages sent
/// into it, and the tasks of chats that ended this run (their late rows never reach the saved chat).
@MainActor
final class TemporaryChatState {
    /// Bumped by every read of `chat.temporary.get` and every tap of the icon: an answer to an older one is stale.
    var reads = 0
    var taps = 0
    /// The streams of the messages sent into the temporary chat, by their turn.
    var sends: [UUID: Task<Void, Never>] = [:]
    /// Tasks of temporary chats that ended while this app ran.
    var formerTasks: Set<String> = []
    /// Open questions a temporary chat's task asks (marked so by the daemon): shown only in the temporary chat.
    var approvalIDs: Set<String> = []
}

/// The temporary chat (owner 2026-11-23: "temporary chat - where there are all the tools n shit BUT no prior context
/// given"; "it shodln be the same small icon that turned it on taht turns it off"). One icon in the chat's header turns
/// it on and off (`chat.temporary.set`); while it is on the chat shows only it, read from the daemon, and the composer
/// sends into it (`chat.send` with `fresh`). Nothing of it is saved here: it never touches `conversation`, so no
/// chat history row, archive or entry-watch write can carry it. Relaunching FERB.app doesn't end it (it is read again
/// at each connection); the daemon saying it is off (its bus event, its status, its reply) drops it here.
@MainActor
extension NotchModel {
    // The owner-facing words (contract, 2026-11-23)
    static let temporaryChatLabel = "Temporary chat"
    static let endTemporaryChatLabel = "End temporary chat"
    static let temporaryChatBanner = "Temporary chat · FERB won't use what it knows or keep this chat"
    static let temporaryChatEmpty = "This chat starts fresh. Tap the dashed bubble again to end it."
    static let temporaryChatStartFailed = "Couldn't start a temporary chat. Try again."
    static let temporaryChatEndFailed = "Couldn't end the temporary chat. Try again."

    var inTemporaryChat: Bool { temporaryChat != nil }

    /// The archive the chat draws now: the temporary chat's while it is on, else the saved one.
    var shownArchive: ConversationArchive { temporaryChat?.archive ?? conversation }

    /// The icon: on when off, off when on. A failure says so and changes nothing.
    func toggleTemporaryChat() {
        guard !temporaryToggling else { return }
        let on = temporaryChat == nil
        temporaryToggling = true
        temporary.taps += 1                     // a read begun before this answer is stale
        Task { [weak self] in
            guard let self else { return }
            defer { self.temporaryToggling = false }
            do {
                let reply = try await self.rpc.call("chat.temporary.set", params: ["on": on])
                self.temporary.taps += 1
                if [Self.temporaryChatStartFailed, Self.temporaryChatEndFailed].contains(self.promptError) {
                    self.promptError = nil
                }
                self.applyTemporaryChat(reply)
            } catch {
                self.promptError = on ? Self.temporaryChatStartFailed : Self.temporaryChatEndFailed
                self.onBackgroundChange?(true)
            }
        }
    }

    /// Reads the temporary chat as the daemon holds it (at each connection, on its bus event, after a frame of an own
    /// message). An unanswered read changes nothing; an answer older than a newer read or a tap is dropped.
    func refreshTemporaryChat() async {
        temporary.reads += 1
        let read = temporary.reads, taps = temporary.taps
        guard let reply = try? await rpc.call("chat.temporary.get", params: [:]),
              read == temporary.reads, taps == temporary.taps else { return }
        applyTemporaryChat(reply)
    }

    /// A reply of `chat.temporary.get` or `.set`: shown when on, dropped when off.
    func applyTemporaryChat(_ reply: [String: Any]) {
        guard var next = TemporaryChat(reply: reply) else { endTemporaryChat(); return }
        if let current = temporaryChat {
            if current.id == next.id {
                var kept = current
                kept.take(next)
                next = kept
            } else {
                forgetTemporaryChat(current)            // another temporary chat: the old one's sends are over
            }
        } else {
            // the temporary chat shows with its composer, wherever the saved chat was
            showingHistory = false
            if !viewingToday {
                followsToday = true
                selectedDay = Calendar.current.startOfDay(for: Date())
            }
        }
        changeIfNeeded(\.temporaryChat, next)
        adoptTemporaryTasks()
        onBackgroundChange?(true)
    }

    /// A running task the saved chat took before its turn here named it (`task.created` comes before the temporary
    /// chat's entries are read again) moves to the temporary chat's work row: added there first, so its time stays.
    private func adoptTemporaryTasks() {
        guard let chat = temporaryChat else { return }
        for (id, heading) in runningTasks where chat.owns(taskID: id) {
            temporaryRunning[id] = heading
            runningTasks.removeValue(forKey: id)
        }
    }

    /// The temporary chat ended (the icon here or on another device, or the daemon restarted): its copy here goes and
    /// the saved chat shows again, as it was.
    func endTemporaryChat() {
        guard let chat = temporaryChat else { return }
        forgetTemporaryChat(chat)
        temporaryChat = nil
        onBackgroundChange?(true)
    }

    private func forgetTemporaryChat(_ chat: TemporaryChat) {
        temporary.formerTasks.formUnion(chat.tasks)
        temporaryRunning = [:]                  // its tasks end with it (the daemon stops them)
        temporary.sends.values.forEach { $0.cancel() }
        temporary.sends.removeAll()
    }

    /// The bus's `chat.temporary` (`{on, id, rev}`): off drops it; a change it doesn't hold yet is read.
    func receiveTemporaryEvent(_ data: [String: Any]) {
        if data["on"] as? Bool == false { endTemporaryChat(); return }
        let rev = (data["rev"] as? NSNumber)?.intValue
        if let chat = temporaryChat, chat.id == data["id"] as? String, let rev, rev <= chat.rev { return }
        Task { await refreshTemporaryChat() }
    }

    /// chat.status's `temporary` (present only while one is on): its absence drops the copy here; a chat or a change
    /// not held yet is read.
    func receiveTemporaryStatus(_ value: Any?) {
        guard let row = value as? [String: Any], let id = row["id"] as? String else {
            endTemporaryChat()
            return
        }
        let rev = (row["rev"] as? NSNumber)?.intValue ?? Int.max
        if let chat = temporaryChat, chat.id == id, rev <= chat.rev { return }
        Task { await refreshTemporaryChat() }
    }

    /// Whether a task belongs to the temporary chat (now, or one that ended this run): its results and rows never reach
    /// the saved chat.
    func isTemporaryTask(_ taskID: String?) -> Bool {
        guard let taskID, !taskID.isEmpty else { return false }
        if temporaryChat?.owns(taskID: taskID) == true { return true }
        return temporary.formerTasks.contains { taskID == $0 || taskID.hasPrefix($0 + ".") }
    }

    /// A chat.status row (running, a result, a question) or a bus event's data of the temporary chat's: marked
    /// `temporary` by the daemon, a temporary request (`temporary-…`), or one of its tasks.
    func isTemporaryRow(_ row: [String: Any]) -> Bool {
        row["temporary"] as? Bool == true || TemporaryChat.isTemporaryRequest(row["request_id"] as? String)
            || isTemporaryTask(row["task_id"] as? String) || temporaryChat?.owns(row: row) == true
            || TemporaryChat.isTemporary(row["conversation"] as? String)
    }

    /// Notes a question's row (chat.status's, approval.requested's): one of a temporary chat's is shown only there.
    func noteTemporaryApproval(_ row: [String: Any]) {
        guard let id = row["approval_id"] as? String, !id.isEmpty, isTemporaryRow(row) else { return }
        temporary.approvalIDs.insert(id)
    }

    /// Whether an open question belongs to a temporary chat (never shown in the saved chat).
    func isTemporaryApproval(_ approval: DaemonApproval) -> Bool {
        temporary.approvalIDs.contains(approval.id) || isTemporaryTask(approval.taskID)
            || isTemporaryTask(approval.sourceTaskID)
    }

    /// The open questions the shown chat draws: the saved chat never shows a temporary chat's; the temporary chat
    /// shows its own and, since a question is live and never kept, the others still waiting.
    var shownApprovals: [DaemonApproval] {
        inTemporaryChat ? pendingApprovals : pendingApprovals.filter { !isTemporaryApproval($0) }
    }

    /// The composer's message into the temporary chat (`chat.send` with `fresh`): shown at once, then as its entries
    /// come. Its stream's frames move only this chat: nothing goes to the saved chat or its history.
    func sendTemporary(text: String, outgoing: String, sources: [URL]) {
        let id = UUID()
        localSettingReply = nil
        promptError = nil
        changeTemporary { $0.send(id: id, text: text, sources: sources, at: Date()) }
        session.clearComposer()
        pasteProvenance = PasteProvenance()
        shownPaste = nil
        receivedFiles.sent(sources)            // a pasted picture sent stays for its task (APP-933)
        showingSources = false
        showingHistory = false
        showingActivity = false
        onInteraction?()
        onPresentationChange?(true)
        temporary.sends[id] = Task { [weak self] in
            guard let self else { return }
            do {
                if self.conversationID == nil { try await self.attachDaemon() }
                var params: [String: Any] = ["text": outgoing, "fresh": true]
                if let session = self.conversationID { params["session_id"] = session }
                if !sources.isEmpty && self.supportsAttachments { params["input_paths"] = sources.map(\.path) }
                for try await event in self.rpc.stream("chat.send", params: params) {
                    guard !Task.isCancelled else { return }
                    self.receiveTemporaryFrame(event, id: id)
                }
                guard !Task.isCancelled else { return }
                await self.refreshTemporaryChat()
                self.changeTemporary { $0.ended(id: id, unconfirmed: "The stream ended before FERB confirmed its reply.") }
            } catch {
                guard !Task.isCancelled else { return }
                self.changeTemporary { $0.failed(id: id, reason: "Connection interrupted before FERB confirmed its reply.") }
                self.connectionStatus = error.localizedDescription
            }
            self.temporary.sends.removeValue(forKey: id)
        }
    }

    /// One frame of an own message's stream: its request and task are noted, its streamed words shown, a question it
    /// asks goes where every question goes; what changed the chat is read again from the daemon.
    func receiveTemporaryFrame(_ event: [String: Any], id: UUID) {
        let type = event["type"] as? String ?? ""
        changeTemporary { $0.bind(id: id, requestID: event["request_id"] as? String, acknowledged: type == "ack") }
        switch type {
        case "reply_delta":
            changeTemporary { $0.stream(id: id, delta: event["text"] as? String ?? "") }
        case "plan":
            if let task = event["task_id"] as? String, !task.isEmpty {
                changeTemporary { $0.planned(id: id, taskID: task) }
                // its task runs in this chat's work row from now, never the saved chat's
                let sent = temporaryChat?.pending.first(where: { $0.turn.id == id })?.turn.request
                temporaryRunning[task] = Self.heading(event, empty: temporaryRunning[task] ?? runningTasks[task]
                                                         ?? sent ?? "Running task")
                runningTasks.removeValue(forKey: task)
            }
        case "error":
            changeTemporary { $0.failed(id: id, reason: event["text"] as? String ?? "FERB could not start this request.") }
        case "event":
            if event["topic"] as? String == "approval.requested" { receiveBusEvent(event) }
        default: break
        }
        if ["ack", "plan", "reply", "clarify", "error", "merged", "outcome"].contains(type) {
            if type == "reply", let sound = SoundPolicy.outcomeSound(event["status"] as? String ?? "done") {
                feedback.play(sound)
            }
            Task { await refreshTemporaryChat() }
        }
    }

    private func changeTemporary(_ edit: (inout TemporaryChat) -> Void) {
        guard var chat = temporaryChat else { return }
        edit(&chat)
        changeIfNeeded(\.temporaryChat, chat)
    }
}
