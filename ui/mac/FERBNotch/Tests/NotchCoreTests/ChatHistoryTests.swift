import AppKit
import SwiftUI
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// The chat in FERB's database (APP-805, APP-806, APP-808): the old saved chat goes over once and the preferences
/// archive is left exactly as it was; a later launch reads the chat from the daemon a page at a time; a save writes only
/// the entries that changed; an older daemon keeps the chat in the preferences; a downgrade's own messages come over
/// when this build runs again. The daemon here is `HistoryRPC` (in memory, as ferb/store/chat_history.py keeps it).
@MainActor
final class ChatHistoryTests: XCTestCase {
    func fixture(_ version: String) throws -> Data {
        let url = try XCTUnwrap(Bundle.module.url(forResource: "archive-" + version, withExtension: "json",
                                                  subdirectory: "Fixtures"))
        return try Data(contentsOf: url)
    }

    /// A preferences suite as an older FERB.app left it: its archive, and a copy an earlier build set aside.
    func olderApp() throws -> UserDefaults {
        let defaults = scratchDefaults("ChatHistory")
        defaults.set(try fixture("0.27.3"), forKey: ArchiveStore.archiveKey)
        defaults.set(try fixture("0.27.0"), forKey: ArchiveStore.restoredPrefix + "2026-11-08T02:42:50Z")
        return defaults
    }

    // decision: APP-805, APP-808, STORE-403
    func testTheOldSavedChatGoesOverOnceAndThePreferencesArchiveIsLeftAsItWas() async throws {
        let defaults = try olderApp()
        let bytes = defaults.data(forKey: ArchiveStore.archiveKey)
        let rpc = HistoryRPC()
        let model = NotchModel(rpc: rpc, preferences: defaults)
        let kept = model.everythingKept().storedEntries
        XCTAssertGreaterThan(kept.count, 10)
        await model.prepareHistory()
        XCTAssertEqual(model.chatStore.store, .daemon)
        XCTAssertEqual(rpc.rows.count, kept.count, "every entry of the archive and of the copy set aside")
        for entry in kept { XCTAssertNotNil(rpc.held(entry.id)) }
        XCTAssertNotNil(model.historyImport, "the import is recorded once the daemon held every entry")
        XCTAssertEqual(model.historyImport?.archiveSHA256, NotchModel.archiveSHA256(bytes))

        // A new message is written to the daemon, never to the preferences archive
        let id = UUID()
        model.conversation.begin(id: id, request: "Find the train times", sources: [])
        model.conversation.acknowledge(id: id, text: "On it.", message: "Find the train times")
        model.saveConversation()
        await model.writeHistory()
        XCTAssertNotNil(rpc.held(id))
        XCTAssertEqual(defaults.data(forKey: ArchiveStore.archiveKey), bytes, "the archive is never written again")

        // The next launch reads the chat from the daemon: today's page, and any day History lists
        let next = NotchModel(rpc: rpc, preferences: defaults)
        XCTAssertTrue(next.conversation.turns.isEmpty, "the preferences archive isn't read")
        await next.prepareHistory()
        XCTAssertEqual(rpc.count("chat.history.import"), 1, "imported once")
        XCTAssertTrue(next.conversation.turns.contains { $0.id == id })
        XCTAssertEqual(next.conversation.turns.first?.request, "Find the train times")
        let day = try XCTUnwrap(kept.first?.createdAt)
        await next.loadDay(day)
        next.selectDay(day, focus: false)
        let saved = model.everythingKept().transcript(on: day).map(\.id)
        XCTAssertEqual(Set(next.visibleEntries.map(\.id)), Set(saved.suffix(DayWindow.page)))
        XCTAssertTrue(next.listedDays.contains(Calendar.current.startOfDay(for: day)) || next.historyDays.isEmpty)
    }

    // decision: APP-808, STORE-403
    func testAfterADowngradeWroteTheArchiveItsNewMessagesComeOverAndNothingNewerIsTakenBack() async throws {
        let defaults = try olderApp()
        let rpc = HistoryRPC()
        let model = NotchModel(rpc: rpc, preferences: defaults)
        await model.prepareHistory()
        // a turn the daemon now holds a newer answer for
        let first = try XCTUnwrap(model.conversation.turns.first)
        model.conversation.finish(id: first.id, answer: "The newer answer", status: "done", artifacts: [])
        await model.writeHistory()

        // an older FERB.app runs and saves its whole archive again, with a message of its own
        var older = try ConversationArchive.read(XCTUnwrap(defaults.data(forKey: ArchiveStore.archiveKey))).archive
        let added = UUID()
        older.begin(id: added, request: "Sent from the older build", sources: [])
        older.acknowledge(id: added, text: "", message: "Sent from the older build")
        defaults.set(try JSONEncoder().encode(older.forStorage()), forKey: ArchiveStore.archiveKey)

        let again = NotchModel(rpc: rpc, preferences: defaults)
        XCTAssertTrue(again.chatStore.importPending, "the archive changed since the import")
        await again.prepareHistory()
        XCTAssertEqual(rpc.count("chat.history.import") > 1, true)
        XCTAssertNotNil(rpc.held(added), "the older build's message came over")
        let body = try XCTUnwrap(rpc.held(first.id)?["body"] as? [String: Any])
        XCTAssertEqual(body["answer"] as? String, "The newer answer", "an import never takes back a newer write")
        XCTAssertFalse(again.chatStore.importPending)
        // a launch after that imports nothing
        let third = NotchModel(rpc: rpc, preferences: defaults)
        XCTAssertFalse(third.chatStore.importPending)
    }

    // decision: APP-810
    func testAnOlderDaemonKeepsTheChatInThePreferences() async throws {
        let defaults = try olderApp()
        let rpc = HistoryRPC()
        rpc.refuse = true
        let model = NotchModel(rpc: rpc, preferences: defaults)
        await model.prepareHistory()
        XCTAssertEqual(model.chatStore.store, .archive)
        XCTAssertNil(model.historyImport)
        let id = UUID()
        model.conversation.begin(id: id, request: "hello", sources: [])
        model.conversation.acknowledge(id: id, text: "", message: "hello")
        model.saveConversation()
        let saved = try ConversationArchive.read(XCTUnwrap(defaults.data(forKey: ArchiveStore.archiveKey))).archive
        XCTAssertTrue(saved.turns.contains { $0.id == id }, "the archive is still where the chat is kept")
    }

    // decision: APP-810, APP-808
    func testAfterFERBsUpdateTheNewDaemonGetsTheChatAtItsFirstConnection() async throws {
        // FERB.app opened a few seconds before its updated daemon: the first connection met the older one
        let defaults = try olderApp()
        let rpc = HistoryRPC()
        rpc.refuse = true
        let model = NotchModel(rpc: rpc, preferences: defaults)
        await model.prepareHistory()
        XCTAssertEqual(model.chatStore.store, .archive)
        let id = UUID()
        model.conversation.begin(id: id, request: "sent while the older daemon ran", sources: [])
        model.saveConversation()
        // the new daemon runs; the app reconnects
        rpc.refuse = false
        await model.prepareHistory()
        XCTAssertEqual(model.chatStore.store, .daemon, "the next connection asks again and imports the chat")
        XCTAssertNotNil(model.historyImport)
        XCTAssertNotNil(rpc.held(id), "what was said meanwhile went over too")
    }

    // decision: APP-805
    func testASaveWritesOnlyTheEntriesThatChangedAndTheRawTextNever() async throws {
        let defaults = try olderApp()
        let rpc = HistoryRPC()
        let model = NotchModel(rpc: rpc, preferences: defaults)
        await model.prepareHistory()
        let puts = rpc.count("chat.history.put")
        await model.writeHistory()
        XCTAssertEqual(rpc.count("chat.history.put"), puts, "nothing changed, nothing written")
        let id = UUID()
        model.conversation.begin(id: id, request: "my password is hunter2", sources: [], unredacted: true)
        await model.writeHistory()
        let written = try XCTUnwrap(rpc.last("chat.history.put")?["entries"] as? [[String: Any]])
        XCTAssertEqual(written.count, 1)
        let body = try XCTUnwrap(written.first?["body"] as? [String: Any])
        XCTAssertEqual(body["request"] as? String, ConversationArchive.unsavedPlaceholder,
                       "the raw text before the daemon's redaction is never written")
        XCTAssertEqual(written.first?["open"] as? Bool, true, "a turn waiting for its answer")
    }

    // decision: APP-810
    func testEntriesNotWrittenWaitInTheOutboxAndComeBackAtTheNextLaunch() async throws {
        let defaults = try olderApp()
        let rpc = HistoryRPC()
        let model = NotchModel(rpc: rpc, preferences: defaults)
        await model.prepareHistory()
        rpc.failPuts = true
        let id = UUID()
        model.conversation.begin(id: id, request: "while the daemon was away", sources: [])
        model.conversation.acknowledge(id: id, text: "", message: "while the daemon was away")
        await model.writeHistory()
        XCTAssertNil(rpc.held(id))
        XCTAssertEqual((defaults.array(forKey: NotchModel.historyOutboxKey) ?? []).count, 1)
        rpc.failPuts = false
        let next = NotchModel(rpc: rpc, preferences: defaults)
        XCTAssertTrue(next.conversation.turns.contains { $0.id == id }, "shown before the daemon answers")
        await next.prepareHistory()
        XCTAssertNotNil(rpc.held(id))
        XCTAssertNil(defaults.array(forKey: NotchModel.historyOutboxKey), "the outbox empties once written")
    }

    // decision: APP-806
    func testADayWindowShowsTheNewestPageAndRevealsEarlierOnesInOrder() throws {
        let entries = try TranscriptScrollProbeTests.archive(day: Date(), entries: 75).transcript()
        var window = DayWindow.newest(entries)
        XCTAssertEqual(window.shown(entries).map(\.id), entries.suffix(30).map(\.id))
        XCTAssertTrue(window.hasEarlier(entries))
        XCTAssertTrue(window.revealHeld(entries))
        XCTAssertEqual(window.shown(entries).map(\.id), entries.suffix(60).map(\.id))
        XCTAssertTrue(window.revealHeld(entries))
        XCTAssertEqual(window.shown(entries).count, 75)
        XCTAssertNil(window.from)
        XCTAssertFalse(window.hasEarlier(entries))
        XCTAssertFalse(window.revealHeld(entries))
        // with the store: its page sets the edge and the cursor; a page read ahead only moves the cursor
        var stored = DayWindow()
        stored.received(oldest: entries[45].createdAt, cursor: "c1", more: true)
        XCTAssertEqual(stored.shown(entries).count, 30)
        stored.held(oldest: entries[15].createdAt, cursor: "c2", more: true)
        XCTAssertEqual(stored.shown(entries).count, 30, "read ahead, not shown yet")
        XCTAssertTrue(stored.revealHeld(entries))
        XCTAssertEqual(stored.shown(entries).count, 60)
        XCTAssertEqual(stored.cursor, "c2")
    }

    // decision: APP-805
    func testAnEntryReadsBackFromTheStoreAsItWasWritten() throws {
        let archive = try ConversationArchive.read(fixture("0.27.3")).archive
        for entry in archive.storedEntries {
            let wire = try HistoryEntry(entry).wire
            let json = try JSONSerialization.data(withJSONObject: wire)
            let back = try XCTUnwrap(HistoryEntry.read(try JSONSerialization.jsonObject(with: json)))
            XCTAssertEqual(back, entry)
        }
        var sync = HistorySync()
        XCTAssertEqual(sync.changes(in: archive).count, archive.storedEntries.count)
        sync.loaded(archive.storedEntries)
        XCTAssertTrue(sync.changes(in: archive).isEmpty)
        var empty = ConversationArchive()
        XCTAssertEqual(empty.absorb(archive.storedEntries).count, archive.storedEntries.count)
        XCTAssertEqual(empty.transcript().map(\.id), archive.transcript().map(\.id))
    }

    // decision: APP-814, APP-806, APP-587
    func testADayNotLoadedYetOpensAtTheChatsHeightAndKeepsIt() async throws {
        // Owner 2026-11-09: "on opeening a new chat its height get short the first time"
        let day = TranscriptScrollProbeTests.day
        let rpc = HistoryRPC()
        let archive = try TranscriptScrollProbeTests.archive(day: day, entries: 40)
        _ = try await rpc.call("chat.history.import",
                               params: ["entries": archive.storedEntries.map { try HistoryEntry($0).wire }])
        let defaults = scratchDefaults("ChatHistoryHeight")
        defaults.set(try JSONEncoder().encode(HistoryImport(at: Date(), entries: 0, archiveSHA256: "")),
                     forKey: NotchModel.historyImportKey)
        let model = NotchModel(rpc: rpc, preferences: defaults)
        var presented = 0
        model.onBackgroundChange = { _ in presented += 1 }
        model.selectDay(day, focus: false)
        XCTAssertTrue(model.visibleEntries.isEmpty, "its first page is on its way")
        XCTAssertTrue(model.shownDayLoading)
        let loading = model.bodyHeight
        XCTAssertEqual(loading, model.chatTranscriptHeight + model.conversationChromeHeight,
                       "the chat keeps its height while the day loads, never the empty height")
        presented = 0
        for _ in 0..<200 where model.visibleEntries.isEmpty { await Task.yield() }
        XCTAssertFalse(model.visibleEntries.isEmpty, "the day's entries came")
        XCTAssertEqual(model.bodyHeight, loading, "the same height with its entries")
        XCTAssertGreaterThan(presented, 0, "the chat is sized again when the page comes")
    }

    // decision: APP-806, APP-809
    func testADayReadFromTheDaemonScrollsToItsTopWithNothingBlankAndNoJump() async throws {
        let probe = TranscriptScrollProbeTests()
        let day = TranscriptScrollProbeTests.day
        let rpc = HistoryRPC()
        let archive = try TranscriptScrollProbeTests.archive(day: day)
        _ = try await rpc.call("chat.history.import",
                               params: ["entries": archive.storedEntries.map { try HistoryEntry($0).wire }])
        let defaults = scratchDefaults("ChatHistoryProbe")
        let record = HistoryImport(at: Date(), entries: 0, archiveSHA256: "")
        defaults.set(try JSONEncoder().encode(record), forKey: NotchModel.historyImportKey)
        let model = NotchModel(rpc: rpc, preferences: defaults)
        XCTAssertEqual(model.chatStore.store, .daemon)
        let host = NSHostingView(rootView: AnyView(ChatBodyView(model: model, animates: false)
            .environment(\.selectableTextGeneration, model.textRenderGeneration)))
        let window = NSWindow(contentRect: CGRect(x: 0, y: 0, width: 440, height: 520),
                              styleMask: .borderless, backing: .buffered, defer: true)
        window.contentView = host
        await model.loadDay(day)
        XCTAssertEqual(model.dayWindows[Calendar.current.startOfDay(for: day)]?.cursor != nil, true)
        let result = try await probe.run(model, host)
        print(result.line.replacingOccurrences(of: "PROBE", with: "PROBE-DAEMON"))
        XCTAssertFalse(result.blankAtOpen)
        XCTAssertEqual(result.shownAtTop, TranscriptScrollProbeTests.entries, "every page came in on the way up")
        XCTAssertEqual(result.blank, 0)
        XCTAssertEqual(result.jumps, 0, "\(result.jumpTotal) pt off")
        XCTAssertEqual(rpc.count("chat.history.page") >= 10, true, "a page of 30 at a time")
        withExtendedLifetime(window) {}
    }
}
