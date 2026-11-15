import Foundation
import NotchCore

/// The engine keeps one conversation on the Mac and every client reads it (ENG-10, ENG-14). FERB.app keeps writing and
/// paging its own history as before (`chat.history.*`, APP-805); what it adds is the turns it did not send itself (a
/// phone's): `chat.entries.watch` brings the engine's entries, and only those of another client's turns are applied to
/// the chat, with the fold's own per-entry mapping (`EntryFold`), then saved like any turn. Own turns keep coming from
/// their own `chat.send` stream, so nothing shows twice. With a daemon that has no entries ("entries" missing from
/// `chat.attach`'s capabilities) none of this runs.
@MainActor
final class EntryWatchState {
    /// The daemon keeps the chat as entries.
    var supported = false
    var task: Task<Void, Never>?
    /// The running watch, so an old one ending never clears a newer one.
    var token = UUID()
    /// The first read since the watch (re)started is done (`caught_up`).
    var caughtUp = false
    /// Each message entry's place in the conversation (`pos`), by request, own messages included: a turn another
    /// client sent goes before a turn of this app made after it.
    var positions: [String: Int] = [:]
    /// The pos of the answer each watched turn shows (the newest that follows no other), by request.
    var shown: [String: Int] = [:]
    /// The seq the engine gave each own message this run (its `ack`), while its entry hasn't come by the watch.
    var ackSeq: [UUID: Int] = [:]
    /// Own requests this run fills from entries: a resent message the engine already had (`repeat`), whose stream
    /// carries nothing more.
    var adopted: Set<String> = []
    var adoptedTurns: Set<UUID> = []
    /// This app's own entries for requests no turn here holds yet (a send whose ack never came), newest by entry id:
    /// adopting the request applies them.
    var unclaimed: [String: [String: ChatEntry]] = [:]
    /// Results filed this run for a request no turn here held (`recordRecovered`), by request: if that request's message
    /// comes by the watch (another device's, its entry late), it fills that turn rather than making a second one.
    var recovered: [String: UUID] = [:]
    /// The newest pos seen, and the marks last sent.
    var newestPos: Int?
    var deliveredMark: Int?
    var readMark: Int?
    /// How long a watch that ended waits before watching again, and how long a connection waits for the first read.
    var retryWait: Duration = .seconds(1)
    var catchUpWait: Duration = .seconds(3)
}

/// Where the watch reads from next: the conversation, its epoch and the last change saved in the history.
struct EntryCursor: Equatable {
    var conversation: String
    var epoch: String
    var seq: Int
}

@MainActor
extension NotchModel {
    static let clientIDKey = "FERBClientID"
    static let entryCursorKey = "FERBEntriesCursor"

    /// This install's id among the conversation's clients (ENG-14), made once and kept.
    var clientID: String {
        if let id = preferences.string(forKey: Self.clientIDKey), !id.isEmpty, id.count <= 200 { return id }
        let id = UUID().uuidString.lowercased()
        preferences.set(id, forKey: Self.clientIDKey)
        return id
    }

    /// The saved cursor of `conversation`; one of another conversation is dropped.
    func savedEntryCursor(_ conversation: String) -> EntryCursor? {
        guard let saved = preferences.dictionary(forKey: Self.entryCursorKey) else { return nil }
        guard let id = saved["conversation"] as? String, id == conversation, let epoch = saved["epoch"] as? String,
              let seq = (saved["seq"] as? NSNumber)?.intValue else {
            preferences.removeObject(forKey: Self.entryCursorKey)
            return nil
        }
        return EntryCursor(conversation: id, epoch: epoch, seq: seq)
    }

    func dropEntryCursor() {
        preferences.removeObject(forKey: Self.entryCursorKey)
        entryWatch.deliveredMark = nil
    }

    // MARK: the watch

    /// After each connection, before the status read files results: the watch runs, and its first read (what changed
    /// since the saved cursor) is applied, so a result finds the turn another device sent.
    func catchUpEntries() async {
        guard entryWatch.supported else { stopEntryWatch(); return }
        startEntryWatch()
        let clock = ContinuousClock()
        let until = clock.now.advanced(by: entryWatch.catchUpWait)
        while !entryWatch.caughtUp && entryWatch.task != nil && clock.now < until {
            do { try await Task.sleep(for: .milliseconds(20)) } catch { return }
        }
    }

    func startEntryWatch() {
        guard entryWatch.supported, entryWatch.task == nil else { return }
        let token = UUID()
        entryWatch.token = token
        entryWatch.task = Task { [weak self] in
            while !Task.isCancelled {
                guard let self, self.entryWatch.supported, let conversation = self.conversationID else { break }
                guard await self.watchEntries(conversation) else { break }
                do { try await Task.sleep(for: self.entryWatch.retryWait) } catch { return }
            }
            if let self, self.entryWatch.token == token { self.entryWatch.task = nil }
        }
    }

    func stopEntryWatch() {
        entryWatch.task?.cancel()
        entryWatch.task = nil
        entryWatch.token = UUID()
        entryWatch.caughtUp = false
    }

    /// One watch stream, from the saved cursor (or from now, without one: nothing earlier is read, so turns this app
    /// sent before are never shown again). Returns false when the daemon refuses the watch (no retry this connection).
    private func watchEntries(_ conversation: String) async -> Bool {
        let cursor = savedEntryCursor(conversation)
        var params: [String: Any] = ["conversation": conversation]
        if let cursor {
            params["epoch"] = cursor.epoch
            params["after_seq"] = cursor.seq
        }
        entryWatch.caughtUp = false
        var epoch = cursor?.epoch
        var catchingUp = cursor != nil
        var batch: [ChatEntry] = []
        do {
            for try await event in rpc.stream("chat.entries.watch", params: params) {
                guard !Task.isCancelled else { return true }
                switch event["type"] as? String {
                case "change":
                    guard let entry = ChatEntry(event["entry"]) else { continue }
                    if catchingUp && event["catchup"] as? Bool == true { batch.append(entry); continue }
                    let wrote = await applyEntryChange(entry, live: true)
                    if let epoch { await commitEntryCursor(conversation, epoch: epoch, seq: entry.seq, wrote: wrote) }
                case "caught_up":
                    epoch = event["epoch"] as? String ?? epoch
                    let wrote = await applyCatchUp(batch)
                    batch = []
                    catchingUp = false
                    if let epoch, let head = (event["head"] as? NSNumber)?.intValue {
                        await commitEntryCursor(conversation, epoch: epoch, seq: head, wrote: wrote)
                    }
                    entryWatch.caughtUp = true
                case "live":
                    receiveEntryLive(event["request_id"] as? String, event["frame"] as? [String: Any] ?? [:])
                case "reset_required", "cursor_ahead":
                    // another store, or a cursor it never gave: watched again from now; what the history holds stays,
                    // and a change seen again finds its turn by request and entry id
                    dropEntryCursor()
                default:
                    break       // `behind`: the stream ends here and is watched again from the cursor
                }
            }
        } catch {
            if let code = (error as? DaemonRPCError)?.code, [-32601, -32005, -32001].contains(code) {
                Self.historyLog.notice("chat.entries.watch isn't available from this FERB (\(code, privacy: .public))")
                return false
            }
        }
        return true
    }

    /// The first read's changes, each entry's newest once, in the conversation's order (`pos`), as the fold reads them.
    func applyCatchUp(_ batch: [ChatEntry]) async -> Bool {
        var newest: [String: ChatEntry] = [:]
        for entry in batch where (newest[entry.id]?.seq ?? Int.min) < entry.seq { newest[entry.id] = entry }
        let ordered = newest.values.sorted { $0.pos < $1.pos }
        for entry in ordered where entry.kind == "message" {
            if let request = entry.requestID { entryWatch.positions[request] = entry.pos }
        }
        var wrote = false
        for entry in ordered {
            if await applyEntryChange(entry, live: false) { wrote = true }
        }
        return wrote
    }

    /// The cursor moves only once what it covers is in the history: a change applied is written first (with FERB's
    /// database as the store, what isn't written waits in the outbox, kept across a quit). A cursor behind is safe (a
    /// change seen again is applied again, harmlessly); one ahead could lose a turn.
    private func commitEntryCursor(_ conversation: String, epoch: String, seq: Int, wrote: Bool) async {
        if wrote {
            guard !archiveBlocked else { return }
            if chatStore.store == .daemon { await writeHistory() }
        }
        guard conversationID == conversation else { return }
        let saved = savedEntryCursor(conversation)
        if saved == nil || saved?.epoch != epoch || seq > saved?.seq ?? Int.min {
            preferences.set(["conversation": conversation, "epoch": epoch, "seq": seq], forKey: Self.entryCursorKey)
        }
        markEntries(delivered: seq)
    }

    // MARK: applying a change

    /// Whether the watch writes the turn of `request`: one another client sent, or an own one this run adopted.
    func watchWrites(_ turn: ConversationTurn) -> Bool {
        turn.sentElsewhere == true || (turn.requestID.map(entryWatch.adopted.contains) ?? false)
    }

    /// The turn of a task or request that the watch writes, if it is one (results and running cards leave it alone).
    func watchedTurn(taskID: String?, requestID: String?) -> ConversationTurn? {
        let turn = requestID.flatMap(conversation.turn(requestID:))
            ?? taskID.flatMap { id in conversation.turns.last { $0.backendTaskID == id } }
        guard let turn, watchWrites(turn) else { return nil }
        return turn
    }

    /// One changed entry. Applied only to a turn this app did not send: an own entry (this install's client id) is
    /// left to the turn's own stream, unless this run adopted its request. Questions (held apart, `pendingApprovals`,
    /// from the bus), notices, posts, imported entries and entries of no message's turn are left as today's paths show
    /// them. Quiet (`live` or not): no sound, no notch opening; what it changed is saved. Returns whether it did.
    @discardableResult
    func applyEntryChange(_ entry: ChatEntry, live: Bool) async -> Bool {
        guard !entry.deleted else { return false }
        if entry.kind == "message", let request = entry.requestID { entryWatch.positions[request] = entry.pos }
        entryWatch.newestPos = max(entryWatch.newestPos ?? entry.pos, entry.pos)
        guard let request = entry.requestID else { return false }
        let own = entry.clientID == clientID
        if own && !entryWatch.adopted.contains(request) {
            if let turn = conversation.turn(requestID: request) {
                if entry.kind == "message" { entryWatch.ackSeq.removeValue(forKey: turn.id) }   // its pos counts now
            }
            if conversation.turn(requestID: request) == nil {
                if (entryWatch.unclaimed[request]?[entry.id]?.seq ?? Int.min) < entry.seq {
                    entryWatch.unclaimed[request, default: [:]][entry.id] = entry
                }
            } else {
                entryWatch.unclaimed.removeValue(forKey: request)
            }
            return false
        }
        guard ["message", "plan", "answer", "error"].contains(entry.kind) else { return false }
        if !own { await loadEntryDay(entry) }
        let changed: UUID?
        if entry.kind == "message" {
            if conversation.turn(requestID: request) == nil, let filed = entryWatch.recovered.removeValue(forKey: request),
               conversation.turns.contains(where: { $0.id == filed && $0.requestID == nil }) {
                changeConversation { _ = $0.bindRequest(id: filed, requestID: request) }
            }
            if let held = conversation.turn(requestID: request), !watchWrites(held), !held.isRecovered { return false }
            let index = entryTurnIndex(entry)
            changed = changeConversation { $0.applyMessageEntry(entry, at: index, elsewhere: !own) }
        } else {
            guard let turn = conversation.turn(requestID: request), watchWrites(turn) else { return false }
            if entry.kind == "plan" {
                changed = changeConversation { $0.applyPlanEntry(entry) }
            } else {
                var shown = entryWatch.shown
                changed = changeConversation { $0.applyAnswerEntry(entry, shown: &shown) }
                entryWatch.shown = shown
            }
        }
        guard changed != nil else { return false }
        saveConversation()
        onBackgroundChange?(true)
        return true
    }

    /// Where a turn another client sent goes among the turns: before the first turn the engine placed after it (by
    /// `pos`; an own message of this run by the seq its ack gave, while its entry hasn't come; an own message still
    /// waiting for its ack), else at the end.
    func entryTurnIndex(_ entry: ChatEntry) -> Int {
        conversation.turns.firstIndex { turn in
            if let request = turn.requestID, let pos = entryWatch.positions[request] { return pos > entry.pos }
            if let seq = entryWatch.ackSeq[turn.id] { return entry.rev == 1 && seq > entry.seq }
            return turn.requestID == nil && submissions[turn.id] != nil && !turn.isRecovered
        } ?? conversation.turns.count
    }

    /// With FERB's database as the store, the entry's day is read first, so a turn already kept there is found.
    private func loadEntryDay(_ entry: ChatEntry) async {
        guard chatStore.store == .daemon, !entry.day.isEmpty, !chatStore.requestedDays.contains(entry.day),
              let day = HistoryEntry.date(entry.day) else { return }
        await loadDay(day)
    }

    /// A live frame of a watched turn's stream: at most the live line the app shows for its own turns (a connection
    /// line, or FERB looking at the plan's files before the task exists); it goes when the plan or the answer comes.
    func receiveEntryLive(_ requestID: String?, _ frame: [String: Any]) {
        guard let requestID, let turn = conversation.turn(requestID: requestID), watchWrites(turn),
              frame["type"] as? String == "progress", frame["connection"] != nil || frame["planning"] as? Bool == true
        else { return }
        changeConversation {
            $0.showProgress(id: turn.id, text: frame["text"] as? String ?? "",
                            startedAt: LiveProgress.time(frame["started_at"]) ?? LiveProgress.time(frame["at"]) ?? Date())
        }
    }

    // MARK: own sends

    /// `chat.send`'s params for this install (ENG-14): its client id and a new submission id, so a resend after a
    /// dropped link never starts a second turn. Unchanged with a daemon without entries.
    func entrySendParams(_ params: [String: Any]) -> [String: Any] {
        guard entryWatch.supported else { return params }
        var sent = params
        sent["client_id"] = clientID
        sent["submission_id"] = UUID().uuidString.lowercased()
        return sent
    }

    /// An own message's ack: its seq places other devices' turns around it until its entry comes. A repeat (the engine
    /// had the message already, from the send whose link dropped) ends the stream with nothing more for the turn: this
    /// run then fills that own turn from the entries, those already seen first.
    func receiveOwnAck(_ event: [String: Any], localID: UUID) async {
        if let seq = (event["seq"] as? NSNumber)?.intValue { entryWatch.ackSeq[localID] = seq }
        guard event["repeat"] as? Bool == true, let request = event["request_id"] as? String,
              conversation.turn(requestID: request)?.id == localID else { return }
        entryWatch.adopted.insert(request)
        entryWatch.adoptedTurns.insert(localID)
        let waiting = (entryWatch.unclaimed.removeValue(forKey: request) ?? [:]).values.sorted { $0.pos < $1.pos }
        for entry in waiting { await applyEntryChange(entry, live: true) }
    }

    /// Whether a stream failed on the link itself, before FERB could answer.
    static func linkFailed(_ error: Error) -> Bool {
        if case DaemonRPCError.unavailable = error { return true }
        if case DaemonRPCError.disconnected = error { return true }
        return false
    }

    // MARK: marks

    /// The conversation's marks for this install (`chat.entries.mark`): what it has saved (`delivered`) and, while
    /// today's chat shows, the newest entry seen (`read_pos`). Sent only when one moves. Nothing visible changes.
    func markEntries(delivered: Int? = nil) {
        guard entryWatch.supported, let conversation = conversationID else { return }
        var params: [String: Any] = ["client_id": clientID, "conversation": conversation]
        if let delivered, delivered > entryWatch.deliveredMark ?? Int.min {
            params["delivered_seq"] = delivered
            entryWatch.deliveredMark = delivered
        }
        if bodyVisible && viewingToday, let pos = entryWatch.newestPos, pos > entryWatch.readMark ?? Int.min {
            params["read_pos"] = pos
            entryWatch.readMark = pos
        }
        guard params.count > 2 else { return }
        Task { [rpc] in _ = try? await rpc.call("chat.entries.mark", params: params) }
    }
}
