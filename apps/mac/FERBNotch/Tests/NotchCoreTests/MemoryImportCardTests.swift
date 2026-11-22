import AppKit
import SwiftUI
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// The memory import's live card (MEM-455, MEM-456): the daemon's card read as FERB.app draws it (the bar from parts
/// done of parts planned, the stages' words), at the end of today's chat while it lasts and gone when the import's one
/// notice comes in; the Memory page's last runs. `FERB_MEMORY_SHOTS` (a folder) also draws it as PNGs for review.
@MainActor
final class MemoryImportCardTests: XCTestCase {
    /// The owner's run of 2026-11-22 as the daemon's card says it (ferb-learning `import_live`).
    static let reading: [String: Any] = [
        "title": "Bringing in what Claude Code knows about you",
        "rows": [["key": "run:r1", "source": "claude_code", "name": "Claude Code", "stage": "reading",
                  "line": "Reading what you've typed", "detail": "", "done": 33, "total": 40, "count": "33 of 40",
                  "found": 221, "found_line": "221 things found so far", "note": ""]],
    ]
    static let placing: [String: Any] = [
        "title": "Bringing in what Claude Code knows about you",
        "rows": [["key": "run:r1", "source": "claude_code", "name": "Claude Code", "stage": "placing",
                  "line": "Putting together what I found",
                  "detail": "Many things come up more than once, so I'm joining them up.", "done": 3, "total": 7,
                  "count": "3 of 7", "found": 274, "found_line": "274 things found", "note": ""]],
    ]
    static let sites: [String: Any] = [
        "title": "Bringing in what ChatGPT and Claude know about you",
        "rows": [["key": "site:chatgpt", "source": "chatgpt", "name": "ChatGPT", "stage": "waiting_mail",
                  "line": "Waiting for ChatGPT's email — asked at 23:40",
                  "detail": "I check your email every 15 minutes. It can take a day or two.", "done": 0, "total": 0,
                  "count": "", "found": 0, "found_line": "", "note": ""],
                 ["key": "site:claude", "source": "claude", "name": "Claude", "stage": "waiting_mail",
                  "line": "Waiting for Claude's email — asked yesterday at 23:41",
                  "detail": "I check your email every 15 minutes. It can take a day or two.", "done": 0, "total": 0,
                  "count": "", "found": 0, "found_line": "",
                  "note": "The last try didn't go through. I'll try again at 00:25."]],
    ]
    static var gap: [String: Any] {
        var card = reading
        var row = (card["rows"] as! [[String: Any]])[0]
        row["note"] = "1 part couldn't be read just now. I'll go back to it next time."
        card["rows"] = [row]
        return card
    }

    // decision: MEM-456
    func testTheCardReadsTheDaemonsWordsAndItsBarFromParts() throws {
        let card = try XCTUnwrap(MemoryImportCard(Self.reading))
        XCTAssertEqual(card.title, "Bringing in what Claude Code knows about you")
        XCTAssertFalse(card.namesRows, "one source: the title names it")
        let row = try XCTUnwrap(card.rows.first)
        XCTAssertEqual(row.line, "Reading what you've typed")
        XCTAssertEqual(row.fraction ?? 0, 33.0 / 40.0, accuracy: 1e-9)
        XCTAssertEqual(row.count, "33 of 40")
        XCTAssertEqual(row.foundLine, "221 things found so far")
        let placing = try XCTUnwrap(MemoryImportCard(Self.placing)?.rows.first)
        XCTAssertEqual(placing.stage, "placing")
        XCTAssertEqual(placing.fraction ?? 0, 3.0 / 7.0, accuracy: 1e-9)
        // a website waiting for its mail: no bar; two sources name their rows
        let sites = try XCTUnwrap(MemoryImportCard(Self.sites))
        XCTAssertTrue(sites.namesRows)
        XCTAssertNil(sites.rows[0].fraction)
        XCTAssertEqual(sites.rows[1].note, "The last try didn't go through. I'll try again at 00:25.")
        // the card left: null, or nothing with a row
        XCTAssertNil(MemoryImportCard(NSNull()))
        XCTAssertNil(MemoryImportCard(nil))
        XCTAssertNil(MemoryImportCard(["title": "x", "rows": []]))
        // a fill never runs past its track
        XCTAssertEqual(MemoryImportCard.Row(key: "k", line: "", done: 9, total: 4).fraction, 1)
    }

    // decision: MEM-456, MEM-445
    func testTheCardIsLiveStateAtTheEndOfTodayAndLeavesWithTheNotice() async throws {
        let rpc = ScriptedRPC()
        rpc.replies["memory.import.status"] = ["current": NSNull(), "runs": [], "live": Self.reading]
        let model = NotchModel(rpc: rpc, preferences: scratchDefaults("ferb-memory-card"))
        // FERB.app attaching while an import goes on shows it at once
        await model.refreshMemoryImport()
        XCTAssertEqual(model.memoryImport?.rows.first?.count, "33 of 40")
        XCTAssertEqual(model.inlineMemoryImport, model.memoryImport)
        // the bus moves it on
        model.receiveBusEvent(["topic": "memory.import.live", "data": ["card": Self.placing]])
        XCTAssertEqual(model.memoryImport?.rows.first?.stage, "placing")
        // another day's chat is a record: no live card there
        model.selectDay(Calendar.current.date(byAdding: .day, value: -1, to: Date())!)
        XCTAssertNil(model.inlineMemoryImport)
        model.selectDay(Date())
        XCTAssertNotNil(model.inlineMemoryImport)
        // the import ended: the card leaves (its notice is the one message, an entry like any other)
        model.receiveBusEvent(["topic": "memory.import.live", "data": ["card": NSNull()]])
        XCTAssertNil(model.memoryImport)
        XCTAssertNil(model.inlineMemoryImport)
        XCTAssertTrue(model.conversation.turns.isEmpty, "the card never becomes a chat entry")
        // a status with nothing going on keeps it gone
        rpc.replies["memory.import.status"] = ["current": NSNull(), "runs": []]
        await model.refreshMemoryImport()
        XCTAssertNil(model.memoryImport)
    }

    // decision: MEM-456
    func testTheMemoryPageSaysEachSourcesLastRun() {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "Asia/Kolkata")!
        let now = ISO8601DateFormatter().date(from: "2026-11-23T10:00:00+05:30")!
        let runs: [[String: Any]] = [
            ["source": "codex", "name": "Codex CLI", "status": "running", "started_at": "2026-11-23T09:59:00+05:30",
             "counts": [:]],
            ["source": "claude_code", "name": "Claude Code", "status": "done",
             "started_at": "2026-11-22T23:36:54.960407+05:30", "finished_at": "2026-11-22T23:58:10.1+05:30",
             "counts": ["profile": ["new lines": 38, "lines reworded by a newer fact": 2]]],
            ["source": "claude_code", "name": "Claude Code", "status": "done", "started_at": "2026-11-20T10:00:00+05:30",
             "counts": [:]],
            ["source": "chatgpt", "name": "ChatGPT", "status": "failed", "started_at": "2026-11-03T08:00:00+05:30",
             "finished_at": "2026-11-03T08:05:00+05:30", "counts": [:]],
        ]
        let lines = MemoryImportLastRun.lines(runs, calendar: calendar, now: now)
        XCTAssertEqual(lines.map(\.name), ["Codex", "Claude Code", "ChatGPT"], "the newest run of each source")
        XCTAssertEqual(lines.map(\.line), [
            "Bringing it in now",
            "Last read yesterday at 23:58: 40 new things learned",
            "Didn't finish on 3 Nov. I'll go on from where I stopped next time.",
        ])
        let quiet = MemoryImportLastRun.lines([["source": "codex", "name": "Codex CLI", "status": "done",
                                                "finished_at": "2026-11-23T09:00:00+05:30",
                                                "counts": ["profile": ["new lines": 0]]]],
                                              calendar: calendar, now: now)
        XCTAssertEqual(quiet.first?.line, "Last read today at 09:00: nothing new")
    }

    // decision: MEM-459 — each switch goes to the daemon's switch, which starts or stops the import; FERB's answer shows
    func testTheSwitchesTurnTheImportOnAndOffThroughTheDaemon() async {
        let rpc = ScriptedRPC()
        rpc.replies["settings.get"] = ["values": ["memory.local_import": false, "memory.sites_import": false]]
        rpc.replies["memory.import.status"] = ["current": NSNull(), "runs": []]
        rpc.replies["memory.import.switch"] = ["which": "local", "on": true,
                                               "said": "Thanks! I'm reading through Claude Code now and I'll tell you what I learned."]
        let store = FERBWindowStore(rpc: rpc)
        await store.loadSettings()
        XCTAssertFalse(store.importSwitch(.local).wrappedValue)
        await store.setImportSwitch(.local, true)
        let sent = rpc.calls.filter { $0.0 == "memory.import.switch" }
        XCTAssertEqual(sent.count, 1)
        XCTAssertEqual(sent.first?.1["which"] as? String, "local")
        XCTAssertEqual(sent.first?.1["on"] as? Bool, true)
        XCTAssertFalse(rpc.calls.contains { $0.0 == "settings.set" }, "never a bare setting")
        XCTAssertTrue(store.importSwitch(.local).wrappedValue)
        XCTAssertEqual(store.importSwitchSaid, "Thanks! I'm reading through Claude Code now and I'll tell you what I learned.")
        // FERB not answering: the switch goes back
        rpc.failures["memory.import.switch"] = DaemonRPCError.invalidResponse
        await store.setImportSwitch(.sites, true)
        XCTAssertFalse(store.importSwitch(.sites).wrappedValue)
        XCTAssertEqual(MemoryImportSwitch.sites.setting, "memory.sites_import")
        XCTAssertFalse((MemoryImportSwitch.allCases.map(\.title) + MemoryImportSwitch.allCases.map(\.detail))
            .contains { $0.contains("AI") || $0.contains("3 months") })
    }

    // decision: MEM-456 — every state draws (and, with FERB_MEMORY_SHOTS, is written out for the owner's review)
    func testTheCardDrawsInTheChatAndOnTheMemoryPage() throws {
        let states: [(String, [String: Any])] = [("reading", Self.reading), ("placing", Self.placing),
                                                 ("websites", Self.sites), ("gap", Self.gap)]
        for (name, value) in states {
            let card = try XCTUnwrap(MemoryImportCard(value))
            let chat = VStack(alignment: .leading, spacing: 10) {
                HStack {
                    Spacer(minLength: 60)
                    Text("Yes, bring it in").font(.system(size: 12.5)).foregroundStyle(.white)
                        .padding(.horizontal, 12).padding(.vertical, 8)
                        .background(Color.accentColor.opacity(0.9), in: RoundedRectangle(cornerRadius: 15))
                }
                Text("Thanks! I'm reading through Claude Code now and I'll tell you what I learned.")
                    .font(.system(size: 12.5)).foregroundStyle(.white)
                    .padding(.horizontal, 12).padding(.vertical, 8)
                    .background(.white.opacity(0.10), in: RoundedRectangle(cornerRadius: 15))
                    .frame(maxWidth: ChatSide.width, alignment: .leading)
                MemoryImportCardView(card: card).frame(maxWidth: ChatSide.width, alignment: .leading)
            }
            .padding(16)
            .frame(width: 440, alignment: .topLeading)
            .background(Color.black)
            .environment(\.colorScheme, .dark)
            XCTAssertGreaterThan(try draw(chat, "mac-memory-chat-\(name).png").height, 80)
        }
        let page = VStack(alignment: .leading, spacing: 12) {
            WindowCard {
                MemoryImportCardView(card: MemoryImportCard(Self.reading)!, onPage: true)
                    .padding(.horizontal, -12).padding(.vertical, -10)
            }
            Text("Correct a memory by telling FERB in chat. “Not right” stops using it and keeps its history; “Forget” deletes it permanently.")
                .font(.system(size: 12)).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            WindowCard { VStack(alignment: .leading, spacing: 12) {
                WindowSectionLabel(title: "Import from other assistants")
                ForEach(MemoryImportLastRun.lines([
                    ["source": "claude_code", "name": "Claude Code", "status": "running", "counts": [:]],
                    ["source": "codex", "name": "Codex CLI", "status": "done", "finished_at": ISO8601DateFormatter().string(from: Date()),
                     "counts": ["profile": ["new lines": 12]]],
                ])) { run in
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        Text(run.name).font(.caption.weight(.semibold))
                        Text(run.line).font(.caption).foregroundStyle(.secondary)
                        Spacer(minLength: 0)
                    }
                }
            } }
        }
        .padding(30)
        .frame(width: 620, alignment: .topLeading)
        .background(Color(white: 0.09))
        .environment(\.colorScheme, .dark)
        XCTAssertGreaterThan(try draw(page, "mac-memory-page.png").height, 120)
    }

    // decision: MEM-459 — the two switches as Settings → Privacy and the Memory page draw them
    func testTheSwitchesDraw() async throws {
        let rpc = ScriptedRPC()
        rpc.replies["settings.get"] = ["values": ["memory.local_import": true, "memory.sites_import": false]]
        let store = FERBWindowStore(rpc: rpc)
        await store.loadSettings()
        store.importSwitchSaid = "Which browser are you signed in to ChatGPT and Claude on? I've asked in the chat."
        let page = WindowCard { MemoryImportSwitches(store: store) }
            .padding(30).frame(width: 620, alignment: .topLeading).background(Color(white: 0.09))
            .environment(\.colorScheme, .dark)
        XCTAssertGreaterThan(try draw(page, "mac-memory-switches.png").height, 100)
    }

    private func draw<V: View>(_ view: V, _ name: String) throws -> NSSize {
        let host = NSHostingView(rootView: view)
        host.frame = CGRect(origin: .zero, size: host.fittingSize)
        host.layoutSubtreeIfNeeded()
        guard let dir = ProcessInfo.processInfo.environment["FERB_MEMORY_SHOTS"], !dir.isEmpty else {
            return host.fittingSize
        }
        let rep = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
        host.cacheDisplay(in: host.bounds, to: rep)
        try XCTUnwrap(rep.representation(using: .png, properties: [:]))
            .write(to: URL(fileURLWithPath: dir).appendingPathComponent(name))
        return host.fittingSize
    }
}
