import Foundation
import NotchCore

/// Where a turn's conversation comes from and goes: the saved chat (the chat history, APP-805) or the temporary chat
/// (the daemon's memory, never kept). The chat pipeline is one; only this differs.
enum ChatSource: Equatable {
    case saved
    case temporary
}

/// The temporary chat's housekeeping (TemporaryChat): which reads and taps are newer, which turns, tasks and questions
/// are its own, and the state of its entries as the shared entry path reads them.
@MainActor
final class TemporaryChatState {
    /// Bumped by every read of `chat.temporary.get` and every tap of the icon: an answer to an older one is stale.
    var reads = 0
    var taps = 0
    /// The turns this app sent into a temporary chat this run: their frames go to that chat, never the saved one.
    var turns: Set<UUID> = []
    /// Tasks known to be a temporary chat's this run (chat.status rows marked `temporary`, those of chats that ended):
    /// with the tasks its turns hold, they never reach the saved chat.
    var tasks: Set<String> = []
    /// Open questions a temporary chat's task asks (marked so by the daemon): shown only in the temporary chat.
    var approvalIDs: Set<String> = []
    /// The temporary chat's entries as the shared entry path keeps them (`EntryWatch.swift`): a new one per chat.
    var entries = TemporaryChatState.newEntries()

    /// A temporary chat is all entries (the daemon keeps it so, whatever `chat.attach` says of the saved chat).
    static func newEntries() -> EntryWatchState {
        let entries = EntryWatchState()
        entries.supported = true
        return entries
    }
}

/// The temporary chat (owner 2026-11-23: "temporary chat - where there are all the tools n shit BUT no prior context
/// given"; "it shodln be the same small icon that turned it on taht turns it off"). One icon in the chat's header turns
/// it on and off (`chat.temporary.set`); while it is on the chat shows only it and the composer sends into it
/// (`chat.send` with `fresh`). It goes through the same pipeline as the saved chat (owner 2026-11-24: "i told you to
/// match normal chat and temp chat UI, why are they even diff???"): its messages are sent by `begin`/`submitToDaemon`,
/// their frames handled by `receiveChatEvent`, its tasks' outcomes by `receiveOutcome`, its questions placed by
/// `announceApproval`, the turns other devices sent by the entry watch's `applyEntry`, each run on its conversation by
/// `inChat(.temporary)`. So its answers open the notch, glow and sound as the saved chat's do. What differs is the
/// data source: its conversation is read from the daemon's memory and is never saved (`saveConversation` writes only
/// the saved chat), relaunching FERB.app doesn't end it, and the daemon saying it is off drops it here.
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
    var shownArchive: ConversationArchive { temporaryChat?.archive ?? savedConversation }

    /// The chat the composer sends into now.
    var composerSource: ChatSource { inTemporaryChat ? .temporary : .saved }

    /// The chat a turn of this app's is in.
    func source(ofTurn id: UUID) -> ChatSource { temporary.turns.contains(id) ? .temporary : .saved }

    /// The chat a task is in.
    func source(ofTask id: String?) -> ChatSource { isTemporaryTask(id) ? .temporary : .saved }

    /// The chat a chat.status row or a bus event's data is about.
    func source(of row: [String: Any]) -> ChatSource { isTemporaryRow(row) ? .temporary : .saved }

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

    /// Reads the temporary chat as the daemon holds it (at each connection, on its bus event, on a status that names a
    /// change not held yet). An unanswered read changes nothing; an answer older than a newer read or a tap is dropped.
    func refreshTemporaryChat() async {
        temporary.reads += 1
        let read = temporary.reads, taps = temporary.taps
        guard let reply = try? await rpc.call("chat.temporary.get", params: [:]),
              read == temporary.reads, taps == temporary.taps else { return }
        applyTemporaryChat(reply)
    }

    /// A reply of `chat.temporary.get` or `.set`: shown when on (its entries applied as the saved chat's watch applies
    /// another device's), dropped when off.
    func applyTemporaryChat(_ reply: [String: Any]) {
        guard let next = TemporaryChat(reply: reply) else { endTemporaryChat(); return }
        if let current = temporaryChat, current.id == next.id {
            var kept = current
            kept.rev = next.rev
            kept.startedAt = next.startedAt
            changeIfNeeded(\.temporaryChat, kept)
        } else {
            if let current = temporaryChat { forgetTemporaryChat(current) }   // another temporary chat: the old one's over
            temporaryChat = next
            // the temporary chat shows with its composer, wherever the saved chat was
            showingHistory = false
            if !viewingToday {
                followsToday = true
                selectedDay = Calendar.current.startOfDay(for: Date())
            }
        }
        applyTemporaryEntries(TemporaryChat.entries(of: reply))
        onBackgroundChange?(true)
    }

    /// The daemon's entries of the temporary chat, through the saved chat's entry path (`applyEntry`): another device's
    /// turns are made and filled from them; this app's own turns are its streams' (as in the saved chat), unless no turn
    /// here holds them (the chat was read again after a relaunch): those are taken up from the entries, as a resent
    /// message the engine already had is (`receiveOwnAck`).
    private func applyTemporaryEntries(_ entries: [ChatEntry]) {
        inChat(.temporary) {
            // A message sent from here still waiting for its request id could be one of these: none is taken up yet.
            let sending = conversation.turns.contains { $0.requestID == nil && submissions[$0.id] != nil }
            for entry in entries where entry.kind == "message" {
                guard let request = entry.requestID else { continue }
                entryWatch.positions[request] = entry.pos
                if entry.clientID == clientID, !sending, conversation.turn(requestID: request) == nil {
                    entryWatch.adopted.insert(request)
                }
            }
            for entry in entries { applyEntry(entry) }
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

    /// A temporary chat is over: its streams stop (the daemon stops its tasks), its work sessions and running rows go,
    /// and its tasks stay known as its own, so nothing late of it reaches the saved chat.
    private func forgetTemporaryChat(_ chat: TemporaryChat) {
        temporary.tasks.formUnion(chat.archive.turns.compactMap(\.backendTaskID).filter { !$0.isEmpty })
        for id in temporary.turns {
            submissions.removeValue(forKey: id)?.cancel()
            workSessions.removeValue(forKey: id)
            pendingStops.remove(id)
        }
        if let active = session.activeTaskID, temporary.turns.contains(active) {
            // the composer stays as the owner left it
            let draft = session.draft, sources = session.attachments, expanded = session.isExpanded
            session = NotchSession()
            session.draft = draft
            session.attach(sources)
            if expanded { session.expand() }
        }
        let ended = runningTasks.keys.filter { isTemporaryTask($0) }
        if !ended.isEmpty { runningTasks = runningTasks.filter { !ended.contains($0.key) } }
        temporary.entries = TemporaryChatState.newEntries()
        taskListCache.invalidate()
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

    /// Whether a task belongs to a temporary chat (the one on, or one that ended this run): it is filed there and never
    /// in the saved chat.
    func isTemporaryTask(_ taskID: String?) -> Bool {
        guard let taskID, !taskID.isEmpty else { return false }
        if temporaryChat?.holds(taskID: taskID) == true { return true }
        return temporary.tasks.contains { taskID == $0 || taskID.hasPrefix($0 + ".") }
    }

    /// A chat.status row (running, a result, a question) or a bus event's data of a temporary chat's: marked
    /// `temporary` by the daemon, a temporary request (`temporary-…`) or conversation, or one of its tasks.
    func isTemporaryRow(_ row: [String: Any]) -> Bool {
        row["temporary"] as? Bool == true || TemporaryChat.isTemporaryRequest(row["request_id"] as? String)
            || TemporaryChat.isTemporaryRequest((row["card"] as? [String: Any])?["request_id"] as? String)
            || isTemporaryTask(row["task_id"] as? String) || TemporaryChat.isTemporary(row["conversation"] as? String)
    }

    /// Notes a row (chat.status's running row or question, approval.requested's) the daemon marks as a temporary chat's:
    /// its task and its question are that chat's from now on.
    func noteTemporaryRow(_ row: [String: Any]) {
        guard isTemporaryRow(row) else { return }
        if let task = row["task_id"] as? String, !task.isEmpty, temporary.tasks.insert(task).inserted {
            taskListCache.invalidate()
        }
        if let id = row["approval_id"] as? String, !id.isEmpty { temporary.approvalIDs.insert(id) }
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
}
