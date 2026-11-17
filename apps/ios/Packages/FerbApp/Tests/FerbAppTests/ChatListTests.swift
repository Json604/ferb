import FerbAppModel
import FerbMobile
import Foundation
import XCTest

/// The chat as the owner met it on his phone (kb PHN-625..PHN-628; Android's ChatListTest): the home screen only for an
/// empty chat (a first-time user or a new day with nothing said), the chat as soon as something was said today; no
/// toggle back to home from the Chat tab; a day's start between days.
@MainActor
final class ChatListTests: XCTestCase {
    private var rig: Rig!

    override func setUp() async throws { rig = Rig() }
    override func tearDown() async throws { rig.tearDown() }

    /// A line of FERB's made `daysAgo` days ago at 09:00 here (its time counts from 2001-01-01, as the library hands it).
    private func said(_ id: String, _ text: String, daysAgo: Int) -> Item {
        let cal = Calendar.current
        let day = cal.date(byAdding: .day, value: -daysAgo, to: cal.startOfDay(for: Date()))!
        let at = cal.date(bySettingHour: 9, minute: 0, second: 0, of: day)!.timeIntervalSinceReferenceDate
        return .notice(notice: NoticeItem(entry: id, at: at, legacy: false, reference: id, after: nil, text: text, quotes: [], cards: nil))
    }

    private func view(_ items: Item...) -> TranscriptView {
        TranscriptView(conversation: "c1", items: items, questions: [], live: [], outbox: [], older: .complete)
    }

    // decision: PHN-625, PHN-626
    func testOpeningWithSomethingSaidTodayShowsTheChatNotTheHomeScreen() async {
        rig.phone.view = view(said("e1", "Your backup finished.", daysAgo: 0))
        let vm = rig.model()
        rig.session.unlocked()
        await eventually("the chat") { !vm.home && !vm.rows.isEmpty }
    }

    // decision: PHN-625, PHN-626
    func testANewDayWithNothingSaidShowsHomeUntilTheFirstWordOfTheDay() async {
        rig.phone.view = view(said("e1", "Yesterday's answer.", daysAgo: 1))
        let vm = rig.model()
        rig.session.unlocked()
        await eventually("rows") { !vm.rows.isEmpty }
        XCTAssertTrue(vm.home, "a new day with nothing said yet: the home screen")
        rig.phone.view = view(said("e1", "Yesterday's answer.", daysAgo: 1), said("e2", "Your 9:00 call moved.", daysAgo: 0))
        rig.phone.emit(.transcript(conversation: "c1"))
        await eventually("FERB posted today: the chat") { !vm.home }
    }

    // decision: PHN-626
    func testTappingChatOnTheChatStaysOnTheChat() async {
        rig.phone.view = view(said("e1", "Done.", daysAgo: 0))
        let vm = rig.model()
        rig.session.unlocked()
        await eventually("the chat") { !vm.home }
        vm.choose(.chat)
        vm.choose(.chat)
        XCTAssertFalse(vm.home)
    }

    // decision: PHN-696
    func testTheChatHasADaysStartBetweenDays() {
        let v = view(said("e1", "Older.", daysAgo: 3), said("e2", "Yesterday.", daysAgo: 1), said("e3", "Now.", daysAgo: 0))
        let today = ChatModel.day(Date(), Calendar.current)
        let rows = ChatModel.withDays(ChatModel.rows(v), ChatModel.days(v, Calendar.current), today: today)
        let days = rows.compactMap { if case .day(_, let d) = $0 { return d } else { return nil } }
        XCTAssertEqual(days, [today - 3, today - 1, today])
        if case .day = rows.first! {} else { XCTFail("the chat starts with its first day") }
    }

    // decision: PHN-662, PHN-664
    func testTheChatTabTappedOnTheChatTakesItToTheEnd() async {
        rig.phone.view = view(said("e1", "Done.", daysAgo: 0))
        let vm = rig.model()
        rig.session.unlocked()
        await eventually("the chat") { !vm.home }
        vm.choose(.tasks)
        vm.choose(.chat)
        XCTAssertEqual(vm.chatToEnd, 0, "coming from another tab keeps his place")
        vm.choose(.chat)
        XCTAssertEqual(vm.chatToEnd, 1, "Chat tapped on the chat: to its end")
    }

    // decision: PHN-663
    func testTheDayOfEachRow() {
        let rows: [ChatRow] = [.older(key: "older", state: .complete), .day(key: "day:9", day: 9), .ferb(key: "a", text: "A"),
                               .day(key: "day:10", day: 10), .ferb(key: "b", text: "B")]
        XCTAssertEqual((0..<rows.count).map { ChatModel.dayAt(rows, $0) }, [9, 9, 9, 10, 10])
        XCTAssertNil(ChatModel.dayAt([.ferb(key: "a", text: "A")], 0))
    }

    // decision: PHN-694, PHN-698
    func testEarlierDaysAreTheirOwnPlaceAndTodayIsOneTapBack() async {
        rig.phone.view = view(said("e1", "Yesterday's answer.", daysAgo: 1))
        let vm = rig.model()
        rig.session.unlocked()
        await eventually("rows") { !vm.rows.isEmpty }
        XCTAssertTrue(vm.home, "nothing said today: today's home page")
        vm.openChat()
        XCTAssertTrue(vm.earlier)
        XCTAssertFalse(vm.home)
        vm.choose(.chat)
        XCTAssertFalse(vm.earlier, "the Chat tab in the earlier days: back to today")
        XCTAssertTrue(vm.home)
        vm.openChat()
        vm.backToToday()
        XCTAssertFalse(vm.earlier)
        XCTAssertTrue(vm.home)
    }

    // decision: PHN-696
    func testTypingNeverLeavesAnEmptyTodayAndHisSendIsAloneInToday() async {
        rig.phone.view = view(said("e1", "Yesterday's answer.", daysAgo: 1))
        let vm = rig.model()
        rig.session.unlocked()
        await eventually("rows") { !vm.rows.isEmpty }
        for n in 1...8 {
            vm.typed(String("Book the".prefix(n)))
            XCTAssertTrue(vm.home, "still today's home page while typing")
        }
        await vm.send().value
        await eventually("his send: today's chat") { !vm.home }
        let day = ChatModel.chatDay(Date(), Calendar.current)
        let today = ChatModel.split(ChatModel.withDays(vm.rows, vm.days, today: day), day: day).today
        XCTAssertEqual(today.map(\.key).filter { !$0.hasPrefix("older") }.count, 1, "only his message: \(today.map(\.key))")
        XCTAssertTrue(today.allSatisfy { if case .outbox = $0 { return true } else { return false } })
    }

    // decision: PHN-696
    func testTheSplitPutsEachRowInItsPlace() {
        let rows: [ChatRow] = [.older(key: "older", state: .complete), .day(key: "day:9", day: 9), .ferb(key: "a", text: "A"),
                               .asking(key: "q", approvalId: "ap-1", title: nil, reason: nil, hidden: false), .day(key: "day:10", day: 10), .ferb(key: "b", text: "B")]
        let p = ChatModel.split(rows, day: 10)
        XCTAssertEqual(p.earlier.map(\.key), ["older", "day:9", "a"])
        XCTAssertEqual(p.today.map(\.key), ["b", "q"], "today's rows, then the open question of an earlier day")
    }

    // decision: PHN-697
    func testADayChangeWhileTheAppIsOpenWaitsForANaturalMoment() async {
        let cal = Calendar.current
        var clock = cal.date(bySettingHour: 23, minute: 58, second: 0, of: Date())!
        let late = cal.date(bySettingHour: 23, minute: 0, second: 0, of: Date())!.timeIntervalSinceReferenceDate
        rig.phone.view = view(.notice(notice: NoticeItem(entry: "t1", at: late, legacy: false, reference: "t1", after: nil, text: "Late answer.", quotes: [], cards: nil)))
        let vm = rig.model(now: { clock })
        rig.session.unlocked()
        await eventually("today's chat") { !vm.home && !vm.rows.isEmpty }
        vm.typed("And one more")
        // midnight passes while he types, and something is read again: the page stays where it is
        clock = clock.addingTimeInterval(4 * 60)
        rig.phone.emit(.transcript(conversation: "c1"))
        await settle()
        XCTAssertFalse(vm.home, "not moved under him")
        XCTAssertEqual(vm.draft, "And one more")
        // the Chat tab chosen: the day moves; nothing said on the new day yet: its home page
        vm.choose(.chat)
        XCTAssertTrue(vm.home, "the new day's home page")
        XCTAssertEqual(vm.draft, "And one more")
    }
}
