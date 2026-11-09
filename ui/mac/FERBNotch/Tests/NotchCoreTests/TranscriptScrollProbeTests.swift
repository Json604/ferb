import AppKit
import SwiftUI
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// The scroll probe (owner 2026-11-09: "the UI lag is not better ... sometimes it shows nothing, laggy scroll"): a day of
/// 300 mixed entries (one-line and long answers, cards with long rows, recovered tasks, late replies, notices) opened in
/// History and scrolled from its newest entry to its oldest in 60-point steps, as the owner's trackpad does. After each
/// step: whether any message text is in view (a blank step), and how far each message still in view moved against the
/// 60 points asked (a jump: the content moved by anything else). The windows here are never ordered front.
@MainActor
final class TranscriptScrollProbeTests: XCTestCase {
    struct Probe {
        var openMs = 0.0
        var blankAtOpen = false
        var steps = 0
        var blank = 0
        var jumps = 0
        var jumpTotal = 0.0
        var stepMs: [Double] = []
        var reachedTop = false
        var shownAtTop = 0

        var mean: Double { stepMs.isEmpty ? 0 : stepMs.reduce(0, +) / Double(stepMs.count) }
        var p95: Double { stepMs.isEmpty ? 0 : stepMs.sorted()[min(stepMs.count - 1, Int(Double(stepMs.count) * 0.95))] }
        var worst: Double { stepMs.max() ?? 0 }
        var line: String {
            String(format: "PROBE open=%.0fms blankAtOpen=%@ steps=%d blank=%d jumps=%d jumpTotal=%.0fpt step mean=%.1fms p95=%.1fms worst=%.1fms top=%@ entriesAtTop=%d",
                   openMs, blankAtOpen ? "yes" : "no", steps, blank, jumps, jumpTotal, mean, p95, worst,
                   reachedTop ? "yes" : "no", shownAtTop)
        }
    }

    // decision: APP-806, APP-809
    func testScrollingALongDayToItsTopNeverShowsNothingAndNeverJumps() async throws {
        let (model, host, window) = try await mixedDay()
        let probe = try await run(model, host)
        print(probe.line)
        XCTAssertFalse(probe.blankAtOpen, "the day opens on its newest messages, never on empty space")
        XCTAssertTrue(probe.reachedTop, "scrolling up reaches the day's first entry")
        XCTAssertEqual(probe.shownAtTop, Self.entries, "every entry of the day came in on the way up")
        XCTAssertEqual(probe.blank, 0, "no step shows nothing")
        XCTAssertEqual(probe.jumps, 0, "the content moves exactly as scrolled (\(probe.jumpTotal) pt off)")
        if ProcessInfo.processInfo.environment["FERB_SCALE_BENCH"] == "1" {
            XCTAssertLessThan(probe.openMs, 300, "opening the day")
            // release (APP-815): a step's mean well within a frame; a step that measures the rows coming into view
            // costs one row's build (rows near the view are measured ahead once the owner pauses)
            XCTAssertLessThan(probe.mean, 16, "a scroll step within a frame (mean)")
            XCTAssertLessThan(probe.p95, 20, "a scroll step that builds a row (95th percentile)")
        }
        withExtendedLifetime(window) {}
    }

    /// Owner 2026-11-09: "i still saw blank screen whiel usign teh scroll indicato to go fastly to top or bottom". With
    /// the lazy stack (before APP-815) the same 30 entries measured anywhere from 8,951 to 14,473 pt as rows were built,
    /// so a jump to the bottom landed in estimated space with nothing in it (1 blank frame in 21 jumps on a day whose
    /// long answers were open).
    // decision: APP-815, APP-806, APP-809
    func testDraggingTheScrollThumbFastShowsMessagesInTheFirstFrame() async throws {
        for open in [false, true] {
            let (model, host, window) = try await mixedDay()
            if open {
                // heights far apart: every other long answer shown whole
                for (n, turn) in model.conversation.turns.enumerated() where turn.answer.count > 2000 && n % 2 == 0 {
                    model.expandedText.toggle(ExpansionState.key(turn: turn.id, item: "answer"))
                }
            }
            let jumps = try await thumb(model, host, window)
            print(jumps.line + (open ? " (long answers open)" : ""))
            XCTAssertEqual(jumps.blankFrames, 0, "a thumb jump shows messages (or the loader) in its first frame")
            XCTAssertEqual(jumps.moved, 0, "what a thumb jump shows stays where it was drawn (\(jumps.movedTotal) pt)")
            XCTAssertEqual(model.visibleEntries.count, Self.entries, "the whole day was in the list for the later jumps")
            if ProcessInfo.processInfo.environment["FERB_SCALE_BENCH"] == "1" {
                XCTAssertLessThan(jumps.meanMs, 16.7, "a thumb jump within a frame (mean)")
            }
            withExtendedLifetime(window) {}
        }
    }

    struct Thumb {
        var jumps = 0
        var blankFrames = 0
        var loaderFrames = 0
        var moved = 0
        var movedTotal = 0.0
        var ms: [Double] = []
        var meanMs: Double { ms.isEmpty ? 0 : ms.reduce(0, +) / Double(ms.count) }
        var worstMs: Double { ms.max() ?? 0 }
        var line: String {
            String(format: "THUMB jumps=%d blankFrames=%d loaderFrames=%d moved=%d movedTotal=%.0fpt mean=%.1fms worst=%.1fms",
                   jumps, blankFrames, loaderFrames, moved, movedTotal, meanMs, worstMs)
        }
    }

    /// The scroll thumb dragged fast: the view put straight at a far place (the top, the bottom, places between), then
    /// one frame (layout and display, as the display cycle does after a scroll event). Blank: no message text and no
    /// loader in view in that frame. Moved: a message drawn in that frame stood elsewhere once the transcript settled.
    func thumb(_ model: NotchModel, _ host: NSView, _ window: NSWindow) async throws -> Thumb {
        var out = Thumb()
        model.selectDay(Self.day, focus: false)
        await settle(host)
        let scroll = try XCTUnwrap(scrollView(in: host), "the transcript's scroll view")
        let clip = scroll.contentView
        func frame() {
            window.contentView?.layoutSubtreeIfNeeded()
            window.displayIfNeeded()
        }
        func jump(_ fraction: CGFloat) async {
            let most = max(0, (scroll.documentView?.frame.height ?? 0) - clip.bounds.height)
            let t = Date()
            clip.scroll(to: CGPoint(x: 0, y: (most * fraction).rounded()))
            scroll.reflectScrolledClipView(clip)
            frame()
            out.ms.append(Date().timeIntervalSince(t) * 1000)
            out.jumps += 1
            let drawn = visible(host, scroll)
            if drawn.isEmpty, ProcessInfo.processInfo.environment["FERB_PROBE_TRACE"] == "1", let table = scroll.documentView as? NSTableView {
                let r = table.rows(in: clip.bounds)
                print("BLANK y=\(clip.bounds.minY) rows=\(r.location)..<\(r.location + r.length) of \(table.numberOfRows)")
                for row in r.location..<(r.location + r.length) {
                    let cell = table.view(atColumn: 0, row: row, makeIfNecessary: false)
                    print("  row \(row) rect=\(table.rect(ofRow: row)) cell=\(cell.map { String(describing: type(of: $0)).prefix(30) } ?? "nil") cellFrame=\(cell?.frame ?? .zero) texts=\(cell.map { textViews(in: $0).count } ?? -1) rowView=\(table.rowView(atRow: row, makeIfNecessary: false) != nil)")
                }
            }
            if drawn.isEmpty {
                if loaderShown(host, scroll) { out.loaderFrames += 1 } else { out.blankFrames += 1 }
            }
            let docThen = scroll.documentView?.frame.height ?? 0, yThen = clip.bounds.minY
            await settle(host)
            let settled = visible(host, scroll)
            if ProcessInfo.processInfo.environment["FERB_PROBE_TRACE"] == "1" {
                print(String(format: "JUMPTO f=%.2f y=%.0f doc=%.0f drawn=%d -> settled y=%.0f doc=%.0f shown=%d entries=%d",
                             fraction, yThen, docThen, drawn.count, clip.bounds.minY,
                             scroll.documentView?.frame.height ?? 0, settled.count, model.visibleEntries.count))
            }
            let off = drawn.compactMap { text, top in settled[text].map { abs($0 - top) } }.max() ?? 0
            if off > 1 {
                out.moved += 1
                out.movedTotal += off
            }
        }
        // the day as it opens (earlier pages still to come), then with every entry in
        for fraction: CGFloat in [0, 1, 0, 1, 0.5, 0, 1] { await jump(fraction) }
        // every page in (as the near-top trigger brings them), then the whole day
        var guardSteps = 0
        while model.visibleEntries.count < Self.entries && guardSteps < 200 {
            model.loadEarlier()
            await settle(host)
            guardSteps += 1
        }
        for fraction: CGFloat in [1, 0, 1, 0, 0.5, 0.1, 0.9, 0.3, 0.7, 1, 0.05, 0.95, 0, 1] { await jump(fraction) }
        return out
    }

    private func loaderShown(_ host: NSView, _ scroll: NSScrollView) -> Bool {
        let shown = scroll.contentView.convert(scroll.contentView.bounds, to: scroll)
        func walk(_ view: NSView) -> Bool {
            if view is NSProgressIndicator, !view.isHiddenOrHasHiddenAncestor,
               view.convert(view.bounds, to: scroll).intersects(shown) { return true }
            return view.subviews.contains(where: walk)
        }
        return walk(host)
    }

    // MARK: - The probe

    static let entries = 300

    func run(_ model: NotchModel, _ host: NSView) async throws -> Probe {
        var probe = Probe()
        let start = Date()
        model.selectDay(Self.day, focus: false)
        await settle(host)
        probe.openMs = Date().timeIntervalSince(start) * 1000
        let scroll = try XCTUnwrap(scrollView(in: host), "the transcript's scroll view")
        let clip = scroll.contentView
        probe.blankAtOpen = visible(host, scroll).isEmpty
        var quiet = 0
        while probe.steps < 3000 {
            let y = clip.bounds.minY
            if y <= 0.5 {
                // at the top: what's still coming arrives here (a page in hand at once, or the daemon's)
                await settle(host)
                if clip.bounds.minY <= 0.5 { quiet += 1 } else { quiet = 0 }
                if quiet >= 3 { break }
                continue
            }
            let before = visible(host, scroll)
            let target = max(0, y - 60)
            let asked = y - target
            let t = Date()
            clip.scroll(to: CGPoint(x: 0, y: target))
            scroll.reflectScrolledClipView(clip)
            await settle(host)
            probe.stepMs.append(Date().timeIntervalSince(t) * 1000)
            probe.steps += 1
            let after = visible(host, scroll)
            if after.isEmpty { probe.blank += 1 }
            // each message in view before and after should have moved down by exactly what was asked
            let shifts = before.compactMap { id, top in after[id].map { $0 - top } }
            // nothing in view stayed: what was in view went (a jump the size of the view at least)
            let replaced = !before.isEmpty && !after.isEmpty && shifts.isEmpty ? CGFloat(scroll.contentView.bounds.height) : nil
            if let off = replaced ?? shifts.map({ abs($0 - asked) }).max(), off > 1 {
                probe.jumps += 1
                probe.jumpTotal += off
                if ProcessInfo.processInfo.environment["FERB_PROBE_TRACE"] == "1" {
                    print(String(format: "JUMP step=%d y=%.0f asked=%.0f off=%.0f docH=%.0f now=%.0f entries=%d", probe.steps, y,
                                 asked, off, scroll.documentView?.frame.height ?? 0, clip.bounds.minY,
                                 model.visibleEntries.count))
                }
            }
        }
        probe.reachedTop = clip.bounds.minY <= 0.5
        probe.shownAtTop = model.visibleEntries.count
        return probe
    }

    /// Each message text in view: its top in the scroll view's own coordinates, by its text (a lazy stack may draw the
    /// same message in a new view; a text shown twice in view isn't compared).
    private func visible(_ host: NSView, _ scroll: NSScrollView) -> [String: CGFloat] {
        var out: [String: CGFloat] = [:]
        var twice = Set<String>()
        let shown = scroll.contentView.convert(scroll.contentView.bounds, to: scroll)
        for view in textViews(in: host) where !view.isHiddenOrHasHiddenAncestor {
            let frame = view.convert(view.bounds, to: scroll)
            guard frame.intersects(shown) && frame.height > 0 else { continue }
            if out[view.string] != nil { twice.insert(view.string) }
            out[view.string] = frame.minY
        }
        for text in twice { out[text] = nil }
        return out
    }

    // MARK: - The day

    static var day: Date { Calendar.current.date(byAdding: .day, value: -1, to: Calendar.current.startOfDay(for: Date()))! }

    /// The chat body as the notch hosts it, today short, yesterday the long mixed day.
    func mixedDay() async throws -> (NotchModel, NSHostingView<AnyView>, NSWindow) {
        let rpc = ScriptedRPC()
        let model = NotchModel(rpc: rpc, preferences: scratchDefaults("ScrollProbe"))
        var archive = try Self.archive(day: Self.day)
        for i in 0..<3 { archive.begin(id: UUID(), request: "today \(i)", sources: []) }
        model.conversation = archive
        let host = NSHostingView(rootView: AnyView(ChatBodyView(model: model, animates: false)
            .environment(\.selectableTextGeneration, model.textRenderGeneration)))
        let window = NSWindow(contentRect: CGRect(x: 0, y: 0, width: 440, height: 520),
                              styleMask: .borderless, backing: .buffered, defer: true)
        window.contentView = host                                     // never ordered front or captured
        await settle(host)
        return (model, host, window)
    }

    /// `entries` entries on `day`: turns of every shape the chat draws, a notice and a late reply every 18 turns.
    static func archive(day: Date, entries: Int = entries) throws -> ConversationArchive {
        let start = Calendar.current.startOfDay(for: day).timeIntervalSinceReferenceDate + 60
        var turns: [[String: Any]] = []
        var replies: [[String: Any]] = []
        var notices: [[String: Any]] = []
        var at = start
        var i = 0
        while turns.count + replies.count + notices.count < entries {
            at += 30
            let id = UUID().uuidString
            var turn: [String: Any] = ["id": id, "createdAt": at, "sources": [], "artifacts": [], "status": "done",
                                       "backendTaskID": "T\(i)", "requestID": "R\(i)",
                                       "request": request(i), "answer": answer(i)]
            switch i % 6 {
            case 2: turn["verified"] = true
            case 3:
                turn["answer"] = "Done: 3 of 3."
                turn["card"] = ["requestID": "R\(i)", "taskID": "T\(i)", "total": 3, "done": 3, "line": "3 of 3 done",
                                "rows": (0..<3).map { r in
                                    ["part": "p\(r)", "label": "Part \(r) of request \(i)", "state": "done",
                                     "step": "s\(r)", "stepTaskID": "T\(i).s\(r)", "taskID": "T\(i)", "direct": false,
                                     "seq": "2026-11-08T10:00:0\(r)Z", "cardRequestID": "R\(i)",
                                     "text": r == 1 ? longRow(i) : "Part \(r) is done: the page lists it."] as [String: Any]
                                }] as [String: Any]
            case 4:
                turn["request"] = ""
                turn["taskGoal"] = "Check the opening hours of place \(i)"
                turn["cards"] = (0..<3).map { k in ["type": "link", "host": "site\(k)-\(i % 7).example.com",
                                                    "line": "Read site\(k)"] }
            case 5:
                turn["cards"] = [["type": "file", "name": "hours-\(i).pdf", "path": "/nonexistent/ferb-probe/hours-\(i).pdf"]]
            default: break
            }
            turns.append(turn)
            if i % 18 == 9 && turns.count + replies.count + notices.count < entries {
                at += 5
                notices.append(["id": "N\(i)", "entryID": UUID().uuidString, "createdAt": at, "tag": "noticed",
                                "text": "You copied the same flight number \(i) twice; it's in your notes now.",
                                "quotes": [["text": "LH \(400 + i)", "origin": "clipboard"]], "afterTurnID": id])
            }
            if i % 18 == 17 && turns.count >= 4 && turns.count + replies.count + notices.count < entries {
                at += 5
                let asked = turns[turns.count - 4]
                replies.append(["id": UUID().uuidString, "turnID": asked["id"]!, "createdAt": at, "afterTurnID": id,
                                "part": "answer"])
            }
            i += 1
        }
        let data = try JSONSerialization.data(withJSONObject: ["turns": turns, "replies": replies, "notices": notices])
        return try JSONDecoder().decode(ConversationArchive.self, from: data)
    }

    static func request(_ i: Int) -> String {
        i % 5 == 0 ? "Hi" : "Find the opening hours of place \(i) and tell me which ones are open late this week"
    }

    static func answer(_ i: Int) -> String {
        switch i % 6 {
        case 0: return "Done."
        case 2: return (0..<5).map { _ in LongChatScaleTests.answer(i) }.joined(separator: "\n\n")  // folds (APP-719)
        default: return LongChatScaleTests.answer(i)
        }
    }

    static func longRow(_ i: Int) -> String {
        (0..<14).map { "Line \($0 + 1) of part 1 of request \(i): the hours are posted on the place's own page." }
            .joined(separator: "\n")
    }

    // MARK: - Helpers

    func settle(_ host: NSView) async {
        for _ in 0..<2 {
            host.layoutSubtreeIfNeeded()
            await withCheckedContinuation { continuation in DispatchQueue.main.async { continuation.resume() } }
        }
        host.layoutSubtreeIfNeeded()
    }

    func textViews(in view: NSView) -> [MessageTextView] {
        (view as? MessageTextView).map { [$0] } ?? view.subviews.flatMap(textViews(in:))
    }

    func scrollView(in view: NSView) -> NSScrollView? {
        var found: [NSScrollView] = []
        func walk(_ v: NSView) {
            if let s = v as? NSScrollView { found.append(s) }
            v.subviews.forEach(walk)
        }
        walk(view)
        return found.max { ($0.documentView?.frame.height ?? 0) < ($1.documentView?.frame.height ?? 0) }
    }
}
