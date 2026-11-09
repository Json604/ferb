import AppKit
import SwiftUI
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// A long day's chat (owner 2026-11-08: "a bit lagginess in UI after so much tasks now"; 2026-11-09: "when i open
/// today's hcat that is short there no lag. but when i pressed on 8nov(prev day) - it is laggggin sooooo much"): a
/// synthetic 300-turn day shaped like the run chat (answers of about 1.5 KB of Markdown, a sites row on every turn, a
/// file card on every third, a multi-part card on every tenth). The windows here are never ordered front.
@MainActor
final class LongChatScaleTests: XCTestCase {
    static let turns = 300

    // decision: APP-800
    func testALongDayBuildsOnlyTheEntriesNearTheView() async throws {
        let (model, host, window) = try await longDay()
        model.selectDay(Self.yesterday, focus: false)
        await settle(host)
        let built = textViews(in: host).count
        XCTAssertGreaterThan(built, 0, "the newest entries are drawn")
        XCTAssertLessThan(built, 60, "a 300-turn day builds the entries near the view, not all \(Self.turns)")
        // an update builds nothing more
        model.noteTaskActivity(taskID: "T-live", title: "Reading a page")
        await settle(host)
        XCTAssertLessThan(textViews(in: host).count, 60)
        withExtendedLifetime(window) {}
    }

    // decision: APP-800, APP-720
    func testAnItemInAnEntryNotBuiltYetIsStillBroughtForward() async throws {
        let (model, host, window) = try await longDay()
        model.selectDay(Date(), focus: false)
        model.conversation = try Self.archive(turns: 120, day: Date())
        await settle(host)
        let scroll = try XCTUnwrap(scrollView(in: host))
        let bottom = scroll.contentView.bounds.minY
        XCTAssertGreaterThan(bottom, 2000, "today's chat opens at its newest entry")
        // the oldest turn's card footer, far above and not built: the transcript reaches it through its entry
        let oldest = try XCTUnwrap(model.visibleEntries.first(where: {
            if case let .turn(turn, _) = $0 { return turn.card != nil } else { return false } }))
        model.requestScroll(TranscriptTarget.footer(turn: oldest.id), .needsYou)
        for _ in 0..<6 { await settle(host) }
        XCTAssertLessThan(scroll.contentView.bounds.minY, bottom / 2, "the view moved up to the oldest card, near the top")
        let shown = textViews(in: host).filter { view in
            view.convert(view.bounds, to: scroll.contentView).intersects(scroll.contentView.bounds)
        }
        XCTAssertTrue(shown.contains { $0.string.hasPrefix("3 of 3 done") }, "its footer is in view")
        withExtendedLifetime(window) {}
    }

    // decision: APP-800
    func testWhereAnItemNotBuiltYetStands() {
        let ids = (0..<5).map { _ in UUID() }
        var turns = ids.map { ConversationTurn(id: $0, createdAt: Date(), request: "r", sources: []) }
        turns[1].answer = "a"
        let reply = ConversationReply(id: UUID(), turnID: ids[1], createdAt: Date(), afterTurnID: ids[4], part: .answer)
        let entries: [TranscriptEntry] = turns.enumerated().map { .turn($1, answeredBelow: $0 == 1) }
            + [.reply(reply, answering: turns[1])]
        // a row or footer is drawn by its turn; an answer given below by the reply
        XCTAssertEqual(TranscriptTarget.entry(holding: TranscriptTarget.row(turn: ids[2], part: "a-b"), in: entries), ids[2])
        XCTAssertEqual(TranscriptTarget.entry(holding: TranscriptTarget.footer(turn: ids[3]), in: entries), ids[3])
        XCTAssertEqual(TranscriptTarget.entry(holding: TranscriptTarget.answer(turn: ids[1]), in: entries), reply.id)
        XCTAssertEqual(TranscriptTarget.entry(holding: TranscriptTarget.answer(turn: ids[0]), in: entries), ids[0])
        XCTAssertNil(TranscriptTarget.entry(holding: TranscriptTarget.question("A1"), in: entries))
        // only entries 2 and 3 are laid out: an item before them is above the view, after them below it
        let laidOut = [TranscriptTarget.entry(ids[2]): CGRect(x: 0, y: 0, width: 10, height: 100),
                       TranscriptTarget.entry(ids[3]): CGRect(x: 0, y: 100, width: 10, height: 100)]
        let above = TranscriptTarget.outOfView(TranscriptTarget.footer(turn: ids[0]), entries: entries, laidOut: laidOut,
                                               viewport: 300)
        XCTAssertLessThan(try XCTUnwrap(above).upperBound, 0)
        let below = TranscriptTarget.outOfView(TranscriptTarget.answer(turn: ids[1]), entries: entries, laidOut: laidOut,
                                               viewport: 300)
        XCTAssertGreaterThan(try XCTUnwrap(below).lowerBound, 300)
        XCTAssertNil(TranscriptTarget.outOfView(TranscriptTarget.row(turn: ids[2], part: "x"), entries: entries,
                                                laidOut: laidOut, viewport: 300), "its entry is laid out: unknown")
        guard case let .pill(pill) = TranscriptFollow().decide(
            TranscriptScrollRequest(target: "t", reason: .quietFill, generation: 1, revealing: false),
            target: above, viewport: 300, reduceMotion: false, at: Date()) else { return XCTFail("a pill") }
        XCTAssertTrue(pill.up, "the pill points up to it")
    }

    // decision: APP-800
    func testAnUnchangedStatusReadRedrawsNothing() async {
        let rpc = ScriptedRPC()
        rpc.replies["chat.status"] = ["results": [], "approvals": [], "loops": [], "notices": [], "paused": false,
                                      "stopped": false,
                                      "running": [["task_id": "T1", "goal": "Check the hours", "name": "Hours",
                                                   "started_at": "2026-11-08T10:00:00+00:00"]]]
        let model = NotchModel(rpc: rpc, preferences: scratchDefaults("LongChatStatus"))
        await model.refreshDaemonStatus()
        XCTAssertEqual(model.runningTasks["T1"], "Hours")
        var announced = 0
        let watch = model.objectWillChange.sink { _ in announced += 1 }
        await model.refreshDaemonStatus()
        XCTAssertEqual(announced, 0, "the 5 s status read found nothing new and redrew nothing")
        withExtendedLifetime(watch) {}
    }

    // decision: APP-800
    func testTheShownDayFollowsTheConversationAndTheDay() throws {
        let model = NotchModel(rpc: ScriptedRPC(), preferences: scratchDefaults("LongChatShown"))
        model.conversation = try Self.archive(turns: 4, day: Self.yesterday)
        XCTAssertTrue(model.visibleEntries.isEmpty, "today has nothing yet")
        let id = UUID()
        model.conversation.begin(id: id, request: "now", sources: [])
        XCTAssertEqual(model.visibleEntries.map(\.id), [id], "a new turn shows at once")
        model.selectDay(Self.yesterday, focus: false)
        XCTAssertEqual(model.visibleEntries.count, 4)
        model.selectDay(Date(), focus: false)
        XCTAssertEqual(model.visibleEntries.map(\.id), [id])
    }

    /// Prints the numbers the report quotes (FERB_SCALE_BENCH=1; wall-clock, so never part of the suite): today's short
    /// chat, then opening the long day in History (the owner's slow path), an unrelated model change while it shows (a
    /// tool call's activity, what `tool.called` does), the 5-second status read, a card row's live line, scroll steps.
    /// FERB_SCALE_ARCHIVE names a saved archive's JSON to use as the long day.
    func testBenchmarkLongDay() async throws {
        guard ProcessInfo.processInfo.environment["FERB_SCALE_BENCH"] == "1" else { throw XCTSkip("benchmark: FERB_SCALE_BENCH=1") }
        var started = Date()
        let (model, host, window) = try await longDay()
        let today = Date().timeIntervalSince(started)
        started = Date()
        model.selectDay(Self.yesterday, focus: false)
        await settle(host)
        let open = Date().timeIntervalSince(started)
        let activity = await timed(30, host) { i in model.noteTaskActivity(taskID: "T-live", title: "tool \(i)") }
        let status = await timed(10, host) { _ in await model.refreshDaemonStatus() }
        let live = await timed(30, host) { i in model.stepLive["S-live"] = LiveProgress(text: "step \(i)", startedAt: Date()) }
        let scroll = try XCTUnwrap(scrollView(in: host), "the transcript's scroll view")
        started = Date()
        var steps = 0
        var y = scroll.contentView.bounds.minY
        while y > 0 && steps < 120 {
            y = max(0, y - 60)
            scroll.contentView.scroll(to: CGPoint(x: 0, y: y))
            scroll.reflectScrolledClipView(scroll.contentView)
            await settle(host)
            steps += 1
        }
        let perScroll = Date().timeIntervalSince(started) / Double(max(1, steps))
        let afterScroll = await timed(30, host) { i in model.noteTaskActivity(taskID: "T-live", title: "again \(i)") }
        print(String(format: "LONGDAY entries=%d todayFirst=%.0fms openLongDay=%.0fms activity=%.1fms statusRead=%.1fms stepLive=%.1fms scroll=%.1fms/step(%d) activityAfterScroll=%.1fms textViews=%d",
                     model.visibleEntries.count, today * 1000, open * 1000, activity * 1000, status * 1000, live * 1000,
                     perScroll * 1000, steps, afterScroll * 1000, textViews(in: host).count))
        withExtendedLifetime(window) {}
    }

    // MARK: - Helpers

    static var yesterday: Date { Calendar.current.date(byAdding: .day, value: -1, to: Calendar.current.startOfDay(for: Date()))! }

    /// The chat body showing today's short chat, with yesterday's long day in History, as the notch hosts it.
    private func longDay() async throws -> (NotchModel, NSHostingView<AnyView>, NSWindow) {
        let rpc = ScriptedRPC()
        rpc.replies["chat.status"] = ["results": [], "approvals": [], "loops": [], "notices": [], "paused": false,
                                      "stopped": false, "running": [["task_id": "T-live", "goal": "A running task"]]]
        let model = NotchModel(rpc: rpc, preferences: scratchDefaults("LongChatScale"))
        var archive = try Self.archive(day: Self.yesterday)
        for i in 0..<3 { archive.begin(id: UUID(), request: "today \(i)", sources: []) }
        model.conversation = archive
        model.runningTasks = ["T-live": "A running task"]
        let host = NSHostingView(rootView: AnyView(ChatBodyView(model: model, animates: false)
            .environment(\.selectableTextGeneration, model.textRenderGeneration)))
        let window = NSWindow(contentRect: CGRect(x: 0, y: 0, width: 440, height: 520),
                              styleMask: .borderless, backing: .buffered, defer: true)
        window.contentView = host                                     // never ordered front or captured
        await settle(host)
        return (model, host, window)
    }

    private func timed(_ count: Int, _ host: NSView, _ change: (Int) async -> Void) async -> TimeInterval {
        var total: TimeInterval = 0
        for i in 0..<count {
            let start = Date()
            await change(i)
            await settle(host)
            total += Date().timeIntervalSince(start)
        }
        return total / Double(count)
    }

    private func settle(_ host: NSView) async {
        for _ in 0..<2 {
            host.layoutSubtreeIfNeeded()
            await withCheckedContinuation { continuation in DispatchQueue.main.async { continuation.resume() } }
        }
        host.layoutSubtreeIfNeeded()
    }

    private func textViews(in view: NSView) -> [MessageTextView] {
        (view as? MessageTextView).map { [$0] } ?? view.subviews.flatMap(textViews(in:))
    }

    private func scrollView(in view: NSView) -> NSScrollView? {
        var found: [NSScrollView] = []
        func walk(_ v: NSView) {
            if let s = v as? NSScrollView { found.append(s) }
            v.subviews.forEach(walk)
        }
        walk(view)
        return found.max { ($0.documentView?.frame.height ?? 0) < ($1.documentView?.frame.height ?? 0) }
    }

    /// A day of `turns` turns on `day`, shaped like the run chat.
    static func archive(turns: Int = turns, day: Date) throws -> ConversationArchive {
        if let path = ProcessInfo.processInfo.environment["FERB_SCALE_ARCHIVE"], turns == Self.turns {
            var archive = try JSONDecoder().decode(ConversationArchive.self, from: Data(contentsOf: URL(fileURLWithPath: path)))
            archive.move(to: day)
            return archive
        }
        let start = Calendar.current.startOfDay(for: day).timeIntervalSinceReferenceDate + 60
        let rows: [[String: Any]] = (0..<turns).map { i in
            var turn: [String: Any] = [
                "id": UUID().uuidString, "createdAt": start + Double(i) * 30,
                "request": i % 4 == 0 ? "" : "Find the opening hours of place \(i) and tell me which are open late",
                "sources": [], "artifacts": [], "status": "done", "verified": i % 2 == 0,
                "backendTaskID": "T\(i)", "answer": answer(i),
                "codeLines": ["Apps used: Safari", "Sites contacted: 3"]
            ]
            if i % 4 == 0 { turn["taskGoal"] = "Check the opening hours of place \(i)" }
            var cards: [[String: Any]] = (0..<3).map { k in
                ["type": "link", "host": "site\(k)-\(i % 7).example.com", "line": "Read site\(k)", "changed": false]
            }
            if i % 3 == 0 {
                cards.append(["type": "file", "name": "hours-\(i).pdf", "path": "/nonexistent/ferb-scale/hours-\(i).pdf",
                              "changed": true])
            }
            turn["cards"] = cards
            if i % 10 == 5 {
                turn["card"] = ["requestID": "R\(i)", "taskID": "T\(i)", "total": 3, "done": 3, "line": "3 of 3 done",
                                "lines": ["Apps used: Safari"],
                                "rows": (0..<3).map { r in
                                    ["part": "p\(r)", "label": "Part \(r) of request \(i)", "state": r == 2 ? "failed" : "done",
                                     "step": "s\(r)", "stepTaskID": "T\(i).s\(r)", "taskID": "T\(i)", "direct": false,
                                     "seq": "2026-11-08T10:00:0\(r)Z", "cardRequestID": "R\(i)",
                                     "text": "Part \(r) of request \(i): " + answer(i + r).prefix(400)] as [String: Any]
                                }] as [String: Any]
            }
            return turn
        }
        let data = try JSONSerialization.data(withJSONObject: ["turns": rows, "replies": [], "notices": []])
        return try JSONDecoder().decode(ConversationArchive.self, from: data)
    }

    static func answer(_ i: Int) -> String {
        let items = (1...6).map { k in
            "- **Place \(i)-\(k)** is open until \(8 + k) pm on weekdays; see [its page](https://site\(k).example.com/p/\(i)) for holidays and `notes`."
        }.joined(separator: "\n")
        return """
        I checked the opening hours of the places you asked about (request \(i)). Most of them close early on Sundays, \
        and two of them changed their hours this month, so I read each place's own page rather than a listing site.

        \(items)

        The ones open late are listed first. Holiday hours were not published for every place, so treat those as \
        unconfirmed until the day before; I can check again then if you want.
        """
    }
}

extension ConversationArchive {
    /// A saved archive's entries moved into one day, keeping their order (the benchmark's long day).
    mutating func move(to day: Date) {
        let times = turns.map(\.createdAt.timeIntervalSinceReferenceDate)
        guard let first = times.min(), let last = times.max(), last > first else { return }
        let from = Calendar.current.startOfDay(for: day).timeIntervalSinceReferenceDate + 60
        let scale = min(1, (86_000 - 120) / (last - first))
        let data = (try? JSONEncoder().encode(self)).flatMap { try? JSONSerialization.jsonObject(with: $0) as? [String: Any] } ?? [:]
        func moved(_ rows: Any?) -> [[String: Any]] {
            (rows as? [[String: Any]] ?? []).map { row in
                var row = row
                for key in ["createdAt", "noticedAt"] {
                    if let at = row[key] as? Double { row[key] = from + max(0, min(last, at) - first) * scale }
                }
                return row
            }
        }
        let shifted: [String: Any] = ["turns": moved(data["turns"]), "replies": moved(data["replies"]), "notices": moved(data["notices"])]
        if let json = try? JSONSerialization.data(withJSONObject: shifted),
           let archive = try? JSONDecoder().decode(ConversationArchive.self, from: json) { self = archive }
    }
}
