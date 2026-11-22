#if canImport(UIKit)
import FerbAppModel
import FerbDesign
import FerbMobile
import SwiftUI
import XCTest
@testable import FerbAppUI

/// The memory import's live card (kb MEM-456; the owner's words MEM-455): while the Mac's status has one, one row keyed
/// "memory-import" at the end of today's chat, after the questions the chat doesn't hold yet; none in the earlier days;
/// gone once the status no longer has it (its notice arrives as a chat entry). Every word is the Mac's.
@MainActor
final class MemoryImportTests: XCTestCase {
    private var rig: Rig!

    override func setUp() async throws { rig = Rig() }
    override func tearDown() async throws { rig.tearDown() }

    private let card = ImportCard(title: "Bringing in what Claude Code knows about you", rows: [
        ImportRow(key: "run:r1", source: "claude_code", name: "Claude Code", stage: "reading", line: "Reading what you've typed", detail: "",
                  done: 33, total: 40, count: "33 of 40", found: 221, foundLine: "221 things found so far", note: ""),
    ])

    private func status(_ importing: ImportCard?) -> PhoneStatus {
        PhoneStatus(locked: false, words: true, stopped: false, questions: [], running: [], results: [], screenUse: nil, memoryImport: importing)
    }

    /// A line of FERB's made `daysAgo` days ago at 09:00 here (its time counts from 2001-01-01, as the library hands it).
    private func said(_ id: String, _ text: String, daysAgo: Int) -> Item {
        let cal = Calendar.current
        let day = cal.date(byAdding: .day, value: -daysAgo, to: cal.startOfDay(for: Date()))!
        let at = cal.date(bySettingHour: 9, minute: 0, second: 0, of: day)!.timeIntervalSinceReferenceDate
        return .notice(notice: NoticeItem(entry: id, at: at, legacy: false, reference: id, after: nil, text: text, quotes: [], cards: nil))
    }

    private let loose: [ChatRow] = [.asking(key: "q:ap-1", approvalId: "ap-1", title: "Book it?", reason: nil, hidden: false)]

    // decision: MEM-456, PHN-696
    func testTheCardIsLastInTodaysChatWhileTheStatusHasOneAndNeverInTheEarlierDays() async {
        rig.phone.view = TranscriptView(conversation: "c1", items: [said("e1", "Yesterday's answer.", daysAgo: 1), said("e2", "Your backup finished.", daysAgo: 0)],
                                        questions: [], live: [], outbox: [], older: .complete)
        rig.phone.status = status(card)
        let vm = rig.model()
        rig.session.unlocked()
        await vm.refresh()?.value
        await eventually("the Mac's status and today's chat") { vm.ui.status?.memoryImport != nil && !vm.places.today.isEmpty }

        // today: its rows, the loose question, then the card
        let today = ChatModel.shown(vm.places, earlier: vm.earlier, loose: loose, importing: vm.ui.status?.memoryImport)
        XCTAssertEqual(today.map(\.key), ["earlier-link", "e2", "q:ap-1", ChatModel.importKey])
        guard case .importProgress(_, let shown) = today.last else { return XCTFail("the card is last") }
        XCTAssertEqual(shown, card, "the Mac's card as it came")
        XCTAssertEqual(ChatModel.words(today.last!), "Bringing in what Claude Code knows about you\nReading what you've typed\n33 of 40\n221 things found so far")

        // the earlier days never have it
        vm.openChat()
        XCTAssertTrue(vm.earlier)
        let earlier = ChatModel.shown(vm.places, earlier: vm.earlier, loose: loose, importing: vm.ui.status?.memoryImport)
        XCTAssertFalse(earlier.contains { $0.key == ChatModel.importKey })
        XCTAssertEqual(earlier.last?.key, "e1")
        vm.backToToday()

        // the import ended: the status no longer has it, and nothing of it stays
        rig.phone.status = status(nil)
        await vm.refresh()?.value
        await eventually("the next status") { vm.ui.status != nil && vm.ui.status?.memoryImport == nil }
        let after = ChatModel.shown(vm.places, earlier: vm.earlier, loose: loose, importing: vm.ui.status?.memoryImport)
        XCTAssertEqual(after.map(\.key), ["earlier-link", "e2", "q:ap-1"])
        XCTAssertFalse(vm.rows.contains { $0.key == ChatModel.importKey }, "live state, never a chat entry")
    }

    // decision: MEM-456
    func testTheCardAloneIsTodaysChatWithNoWayToEarlierDaysAboveIt() {
        let places = ChatModel.split([.day(key: "day:9", day: 9), .ferb(key: "a", text: "A")], day: 10)
        XCTAssertEqual(ChatModel.shown(places, earlier: false, loose: [], importing: card).map(\.key), [ChatModel.importKey])
        XCTAssertEqual(ChatModel.shown(places, earlier: false, loose: [], importing: nil).map(\.key), [])
        XCTAssertEqual(ChatModel.shown(places, earlier: true, loose: [], importing: card).map(\.key), ["day:9", "a"])
    }

    /// Today's chat as the owner sees it with the card at its end, drawn by the chat's own rows (noon sky).
    // decision: MEM-456, PHN-734
    func testGoldenTheCardAtTheEndOfTodaysChat() {
        let m = rig.model()
        let places = ChatModel.split([
            .owner(key: "m1", text: "Bring in what Claude Code knows about me", files: [], note: nil),
            .ferb(key: "m1:first", text: "Thanks! I'm reading through Claude Code now and I'll tell you what I learned."),
        ], day: 10)
        let rows = ChatModel.shown(places, earlier: false, loose: [], importing: card)
        let page = PhoneScreen(moment: .lab(12.5), glide: false) {
            VStack(spacing: 12) {
                ForEach(rows) { ChatRowView(model: m, row: $0, today: chatDayOf(m.ui), olderLoading: false) }
            }
            .padding(.horizontal, 16).padding(.top, 90).frame(maxHeight: .infinity, alignment: .top)
        }
        .frame(width: 402, height: 874)
        .stillMotion()
        AppSnapshot.assert(page, size: CGSize(width: 402, height: 874), path: "app/memory-import-chat-noon.png")
    }

    /// A new day with nothing said yet: the chat tab is the home page (kb PHN-626), and the card shows there too, above
    /// the message box, at noon and at night; without an import it has none. Drawn with the lab's still box: the app's
    /// own text field can't draw offscreen.
    // decision: MEM-457
    func testGoldenTheCardOnTheHomePage() {
        XCTAssertNil(homeImportCard(nil))
        XCTAssertNil(homeImportCard(ImportCard(title: "t", rows: [])))
        for (hour, name) in [(12.5, "noon"), (22.5, "night")] {
            let page = PhoneScreen(moment: .lab(hour), glide: false) {
                HomePage(texts: HomeTexts(greeting: "Hello", subline: ""), away: [], insets: .lab, earlier: L.s("older_available"),
                         live: homeImportCard(card))
            }
            .frame(width: PhoneInsets.labSize.width, height: PhoneInsets.labSize.height)
            .stillMotion()
            AppSnapshot.assert(page, size: PhoneInsets.labSize, path: "app/memory-import-home-\(name).png")
        }
    }
}
#endif
