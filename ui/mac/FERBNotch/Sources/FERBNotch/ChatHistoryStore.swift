import Foundation
import CryptoKit
import NotchCore

/// Where the chat is kept (APP-805, APP-808).
/// - `archive`: in the app's preferences, as every build before this did: the whole chat in hand, written whole. Used
///   until the daemon has the old chat (the one-time import), and for good with a daemon that has no chat history.
/// - `daemon`: in FERB's database. The app holds the entries it shows, writes each one that changed and reads a day a
///   page at a time; the preferences archive is left exactly as it was at the import, never written again.
@MainActor
final class HistoryState {
    enum Store { case archive, daemon }
    var store: Store = .archive
    /// What the daemon holds of the entries in hand.
    var sync = HistorySync()
    /// The old chat to send once more: everything kept in the preferences (APP-808).
    var importPending = false
    /// The daemon refused the chat history RPCs (an older daemon, or not FERB.app's own process): the archive stays.
    var unavailable = false
    /// Days whose newest page the daemon has given (or is giving) this run.
    var requestedDays = Set<String>()
    var writing: Task<Void, Never>?
    /// Days whose next earlier page is being read ahead.
    var prefetching = Set<Date>()
    /// Writes that failed, retried at the next connection.
    var retry = false
}

/// The import's record (APP-808): when the daemon confirmed it held every entry, and the archive's bytes then. An
/// archive that no longer has those bytes was written by an older FERB.app after it (a downgrade): what it added is sent
/// once more.
struct HistoryImport: Codable, Equatable {
    var at: Date
    var entries: Int
    var archiveSHA256: String
}

@MainActor
extension NotchModel {
    static let historyImportKey = "FERBChatHistoryImport"
    static let historyOutboxKey = "FERBChatHistoryOutbox"
    /// Ruling (APP-808): a write or import call carries at most this many bytes of entries (the socket takes 1 MB).
    nonisolated static let historyCallBytes = 400_000

    static func archiveSHA256(_ data: Data?) -> String {
        guard let data else { return "" }
        return SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
    }

    var historyImport: HistoryImport? {
        preferences.data(forKey: Self.historyImportKey).flatMap { try? JSONDecoder().decode(HistoryImport.self, from: $0) }
    }

    /// At launch: with the import done the chat comes from the daemon (only entries a failed write left in the outbox
    /// are in hand); before it, the preferences archive is read as before. Returns true when the archive was read.
    func openHistory() -> Bool {
        guard let record = historyImport else {
            chatStore.store = .archive
            chatStore.importPending = true
            return true
        }
        chatStore.store = .daemon
        // An older FERB.app wrote the archive after the import (a downgrade, then this build again): send it again.
        chatStore.importPending = Self.archiveSHA256(preferences.data(forKey: ArchiveStore.archiveKey)) != record.archiveSHA256
        let outbox = (preferences.array(forKey: Self.historyOutboxKey) ?? []).compactMap(HistoryEntry.read)
        if !outbox.isEmpty { conversation.absorb(outbox) }
        return false
    }

    /// Everything the preferences keep: the archive and every copy set aside (APP-801), merged as recovery merges them.
    func everythingKept() -> ConversationArchive {
        // Read only: after the import the archive's bytes never change (APP-808)
        var all: ConversationArchive
        if chatStore.store == .archive {
            all = conversation                                    // the archive as read at launch, and this run since
        } else {
            all = preferences.data(forKey: ArchiveStore.archiveKey)
                .flatMap { try? ConversationArchive.read($0).archive } ?? ConversationArchive()
            all.interruptRestoredDrafts()
        }
        let copies = preferences.dictionaryRepresentation().keys.filter {
            $0.hasPrefix(ArchiveStore.unreadablePrefix) || $0.hasPrefix(ArchiveStore.restoredPrefix)
                || $0.hasPrefix(ArchiveStore.partialPrefix)
        }.sorted()
        for key in copies {
            guard let data = preferences.data(forKey: key), var older = try? ConversationArchive.read(data).archive
            else { continue }
            older.interruptRestoredDrafts()
            all.restore(older)
        }
        return all
    }

    /// After each connection, before the status read places anything: the old chat imported if it is due, the
    /// outbox written, today's newest page and every waiting turn read.
    func prepareHistory() async {
        guard !chatStore.unavailable else { return }
        if chatStore.importPending {
            guard await importArchive() else { return }
        }
        guard chatStore.store == .daemon else { return }
        await flushOutbox()
        let today = HistoryEntry.day(Date())
        if !chatStore.requestedDays.contains(today) { await loadDay(Date(), open: true) }
        if chatStore.retry { chatStore.retry = false; writeHistorySoon() }
        refreshHistoryDays()
    }

    /// Sends the old chat (APP-808); true once the daemon holds every entry. A refusal (no such RPC, not FERB.app)
    /// leaves the chat in the preferences for this run.
    private func importArchive() async -> Bool {
        let keptBytes = preferences.data(forKey: ArchiveStore.archiveKey)
        let all = everythingKept()
        let entries = all.storedEntries.compactMap { try? HistoryEntry($0) }
        do {
            for batch in Self.batches(entries) {
                let reply = try await rpc.call("chat.history.import", params: ["entries": batch.map(\.wire)])
                guard (reply["held"] as? NSNumber)?.intValue == batch.count else {
                    Self.historyLog.error("chat import: the daemon didn't confirm every entry; tried again next time")
                    return false
                }
                chatStore.sync.stored(batch)
            }
        } catch {
            if let code = (error as? DaemonRPCError)?.code, [-32601, -32005, -32001].contains(code) {
                chatStore.unavailable = true
                Self.historyLog.notice("chat history isn't available from this FERB (\(code, privacy: .public)); the app keeps it")
            }
            return false
        }
        let record = HistoryImport(at: Date(), entries: entries.count, archiveSHA256: Self.archiveSHA256(keptBytes))
        if let data = try? JSONEncoder().encode(record) { preferences.set(data, forKey: Self.historyImportKey) }
        chatStore.importPending = false
        if chatStore.store == .archive {
            chatStore.store = .daemon
            // Today and the days already open keep what is in hand; the daemon's pages fill in from here.
            chatStore.requestedDays.insert(HistoryEntry.day(Date()))
        }
        Self.historyLog.notice("chat import: the daemon holds all \(entries.count, privacy: .public) entries; the preferences archive is kept as it is and no longer written")
        writeHistorySoon()
        return true
    }

    /// Entries in calls under `historyCallBytes` each.
    nonisolated static func batches(_ entries: [HistoryEntry]) -> [[HistoryEntry]] {
        var out: [[HistoryEntry]] = []
        var current: [HistoryEntry] = []
        var bytes = 0
        for entry in entries {
            if !current.isEmpty && (bytes + entry.body.count > historyCallBytes || current.count >= 400) {
                out.append(current)
                current = []
                bytes = 0
            }
            current.append(entry)
            bytes += entry.body.count
        }
        if !current.isEmpty { out.append(current) }
        return out
    }

    /// A save (`saveConversation`) with the daemon as the store: the entries that changed are written shortly after,
    /// many changes in one call.
    func writeHistorySoon() {
        guard chatStore.store == .daemon, chatStore.writing == nil else { return }
        chatStore.writing = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(250))
            guard let self else { return }
            self.chatStore.writing = nil
            await self.writeHistory()
        }
    }

    func writeHistory() async {
        let changed = chatStore.sync.changes(in: conversation)
        guard !changed.isEmpty else { return }
        var unsent: [HistoryEntry] = []
        for batch in Self.batches(changed) {
            do {
                _ = try await rpc.call("chat.history.put", params: ["entries": batch.map(\.wire)])
                chatStore.sync.stored(batch)
            } catch { unsent += batch }
        }
        keepInOutbox(unsent, written: changed.filter { entry in !unsent.contains { $0.value.id == entry.value.id } })
        if !unsent.isEmpty { chatStore.retry = true }
    }

    /// Entries not yet written survive a quit (the outbox, only while there are any); written ones leave it.
    private func keepInOutbox(_ unsent: [HistoryEntry], written: [HistoryEntry]) {
        let old = preferences.array(forKey: Self.historyOutboxKey) ?? []
        guard !old.isEmpty || !unsent.isEmpty else { return }
        let drop = Set((written + unsent).map { $0.value.id.uuidString })
        let kept = old.filter { item in
            guard let id = (item as? [String: Any])?["id"] as? String else { return false }
            return !drop.contains(id.uppercased())
        } + unsent.map(\.wire)
        if kept.isEmpty { preferences.removeObject(forKey: Self.historyOutboxKey) }
        else { preferences.set(kept, forKey: Self.historyOutboxKey) }
    }

    private func flushOutbox() async {
        let waiting = (preferences.array(forKey: Self.historyOutboxKey) ?? []).compactMap(HistoryEntry.read)
        guard !waiting.isEmpty else { return }
        await writeHistory()
    }

    /// FERB.app is quitting: what changed and isn't written yet goes to the outbox, written at the next launch.
    func keepUnsavedHistory() {
        guard chatStore.store == .daemon else { return }
        chatStore.writing?.cancel()
        let changed = chatStore.sync.changes(in: conversation)
        keepInOutbox(changed, written: [])
    }

    // MARK: Days and pages (APP-806)

    /// A day's newest page; `open`: with every turn still waiting for FERB (launch).
    func loadDay(_ day: Date, open: Bool = false) async {
        let start = Calendar.current.startOfDay(for: day)
        let key = HistoryEntry.day(start)
        chatStore.requestedDays.insert(key)
        updateWindow(start) { $0.loading = true }
        do {
            let page = try await rpc.call("chat.history.page",
                                          params: ["day": key, "limit": DayWindow.page, "open": open])
            let entries = (page["entries"] as? [Any] ?? []).compactMap(HistoryEntry.read)
            let more = page["more"] as? Bool ?? false
            receivePage(start, entries: entries, context: (page["context"] as? [Any] ?? []) + (page["open"] as? [Any] ?? []),
                        cursor: page["cursor"] as? String, more: more, launch: open)
        } catch {
            chatStore.requestedDays.remove(key)
            updateWindow(start) { $0.loading = false }
        }
    }

    private func receivePage(_ day: Date, entries: [StoredEntry], context: [Any], cursor: String?, more: Bool,
                             launch: Bool) {
        let extra = context.compactMap(HistoryEntry.read)
        chatStore.sync.loaded(entries + extra)
        changeConversation { archive in
            archive.absorb(entries + extra)
            // A draft FERB.app was showing when it quit never finishes now (as the archive's launch did)
            if launch { archive.interruptRestoredDrafts() }
        }
        updateWindow(day) { $0.received(oldest: entries.first?.createdAt, cursor: cursor, more: more) }
        if launch { writeHistorySoon() }
        prefetchEarlier(day)
    }

    /// The owner scrolled near the top of the shown day: earlier entries come in, those in hand at once, else the
    /// daemon's next page (the loader shows while it comes).
    func loadEarlier() {
        let day = shownDay
        let all = conversation.transcript(on: day)
        var window = window(for: day, all: all)
        guard !window.loading else { return }
        if window.revealHeld(all) {
            beforeEarlierInsert?()
            updateWindow(day) { $0 = window }
            prefetchEarlier(day)
            return
        }
        guard chatStore.store == .daemon, let cursor = window.cursor else { return }
        updateWindow(day) { $0.loading = true }
        Task { [weak self] in
            guard let self else { return }
            do {
                let page = try await self.rpc.call("chat.history.page", params: [
                    "day": HistoryEntry.day(day), "before": cursor, "limit": DayWindow.page])
                let entries = (page["entries"] as? [Any] ?? []).compactMap(HistoryEntry.read)
                if day == self.shownDay { self.beforeEarlierInsert?() }
                self.receivePage(day, entries: entries, context: page["context"] as? [Any] ?? [],
                                 cursor: page["cursor"] as? String, more: page["more"] as? Bool ?? false, launch: false)
            } catch {
                self.updateWindow(day) { $0.loading = false }
            }
        }
    }

    /// The daemon's next earlier page of a day, read ahead and held without showing (APP-806): the owner's next scroll
    /// up shows it at once, with no loader and no wait.
    func prefetchEarlier(_ day: Date) {
        guard chatStore.store == .daemon, let window = dayWindows[day], let cursor = window.cursor, !window.loading,
              !chatStore.prefetching.contains(day) else { return }
        chatStore.prefetching.insert(day)
        Task { [weak self] in
            guard let self else { return }
            defer { self.chatStore.prefetching.remove(day) }
            guard let page = try? await self.rpc.call("chat.history.page", params: [
                "day": HistoryEntry.day(day), "before": cursor, "limit": DayWindow.page]) else { return }
            let entries = (page["entries"] as? [Any] ?? []).compactMap(HistoryEntry.read)
            let extra = (page["context"] as? [Any] ?? []).compactMap(HistoryEntry.read)
            self.chatStore.sync.loaded(entries + extra)
            self.changeConversation { $0.absorb(entries + extra) }
            let more = page["more"] as? Bool ?? false
            self.updateWindow(day) { window in
                window.held(oldest: entries.first?.createdAt, cursor: page["cursor"] as? String, more: more)
            }
        }
    }

    var shownDay: Date { Calendar.current.startOfDay(for: followsToday ? Date() : selectedDay) }

    /// The shown day's window: what is shown of it and whether more is coming.
    var shownWindow: DayWindow { window(for: shownDay, all: conversation.transcript(on: shownDay)) }
    var hasEarlierEntries: Bool { shownWindow.hasEarlier(conversation.transcript(on: shownDay)) }

    var earlierEntries: EarlierEntries {
        let all = conversation.transcript(on: shownDay)
        let window = window(for: shownDay, all: all)
        return EarlierEntries(more: window.hasEarlier(all), loading: window.loading)
    }

    /// A day's window, made on first use: with the daemon as the store its newest page is asked for; otherwise the
    /// newest entries in hand.
    func window(for day: Date, all: [TranscriptEntry]) -> DayWindow {
        if let window = dayWindows[day] { return window }
        var window = DayWindow.newest(all)
        if chatStore.store == .daemon && !chatStore.requestedDays.contains(HistoryEntry.day(day)) {
            chatStore.requestedDays.insert(HistoryEntry.day(day))
            window.loading = true
            Task { [weak self] in await self?.loadDay(day) }
        }
        dayWindows[day] = window       // kept, so a new entry never pushes the oldest shown one out of view
        return window
    }

    func updateWindow(_ day: Date, _ change: (inout DayWindow) -> Void) {
        var window = dayWindows[day] ?? DayWindow.newest(conversation.transcript(on: day))
        change(&window)
        guard window != dayWindows[day] else { return }
        objectWillChange.send()
        dayWindows[day] = window
        shownTranscript = nil
    }

    /// A day opened again shows its newest page again; what was read stays in hand.
    func resetWindow(_ day: Date) {
        let start = Calendar.current.startOfDay(for: day)
        guard let old = dayWindows[start] else { return }
        var fresh = DayWindow.newest(conversation.transcript(on: start))
        fresh.cursor = old.cursor
        fresh.loading = old.loading
        if fresh.from == nil, old.cursor != nil { fresh.from = old.from }
        updateWindow(start) { $0 = fresh }
    }

    /// An item FERB brings forward in an entry not shown yet: the window opens down to it first.
    func revealForScroll(_ target: String) {
        let day = shownDay
        let all = conversation.transcript(on: day)
        guard let from = dayWindows[day]?.from,
              let holder = TranscriptTarget.entry(holding: target, in: all),
              let entry = all.first(where: { $0.id == holder }), entry.createdAt < from else { return }
        updateWindow(day) { $0.from = entry.createdAt }
    }

    /// The days History lists: the daemon's, with any in hand.
    var listedDays: [Date] {
        Array(Set(historyDays + conversation.days)).sorted(by: >)
    }

    func refreshHistoryDays() {
        guard chatStore.store == .daemon else { return }
        Task { [weak self] in
            guard let self, let reply = try? await self.rpc.call("chat.history.days", params: [:]) else { return }
            let days = (reply["days"] as? [String] ?? []).compactMap { HistoryEntry.date($0) }
            self.changeIfNeeded(\.historyDays, days)
        }
    }
}
