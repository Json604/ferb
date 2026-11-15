import XCTest
@testable import FERBNotch
@testable import NotchCore

/// One box per timer, reminder and schedule (APP-769, owner 2026-11-06: "also whyh doesbly boxes for end and start???
/// why not use the same??? this is for other schedules and other type of boxes too"). Driven by events shaped as the
/// daemon sends them: a reply's event card, a timer's or reminder's arrival through the notification path, a
/// schedule's runs as task results.
@MainActor
final class OneBoxTests: XCTestCase {
    private let tea = "tm-1a2b"

    // decision: APP-769
    func testATimersBoxMovesToTheBottomWhenItGoesOffAndNoBubbleRepeatsIt() throws {
        let model = makeModel()
        let id = begin("set a 10 second timer", in: model)
        let line = "⏱ Timer set: 10-second timer — ends 6:46 PM"
        model.receiveChatEvent(["type": "reply", "request_id": "R", "text": line,
                                "cards": [["type": "event", "kind": "timer", "id": tea, "what": "10-second timer",
                                           "when": "2026-11-06T13:16:39+00:00", "started": "2026-11-06T13:16:29+00:00",
                                           "line": line]]], localID: id)
        XCTAssertTrue(CardBoxes.moved(model.visibleEntries).isEmpty)        // set and running: the box is the set card
        model.receiveReminder(id: tea, text: "Your 10-second timer is up.",
                              card: ResultCard(["type": "reminder", "title": "10-second timer", "kind": "timer",
                                                "timer_id": tea, "due": "2026-11-06T13:16:39+00:00",
                                                "started": "2026-11-06T13:16:29+00:00"]))
        let entries = model.visibleEntries
        XCTAssertEqual(entries.count, 2)
        guard case .turn(let turn, _) = entries[0], case .notice(let notice) = entries[1] else {
            return XCTFail("expected the turn, then the arrival at the bottom")
        }
        let moved = CardBoxes.moved(entries)
        XCTAssertEqual(moved[turn.id], [tea])                                // its old spot shows nothing of it
        XCTAssertNil(moved[notice.entryID])                                  // the bottom draws it, done
        XCTAssertTrue(CardBoxes.shown(turn.cards ?? [], hiding: moved[turn.id] ?? []).isEmpty)
        XCTAssertNil(CardLayout.bubble(turn.answer, turn.cards ?? [], note: nil, lines: []))   // and no empty bubble
        XCTAssertTrue(CardBoxes.boxSays(notice))                             // no "Your 10-second timer is up."
        XCTAssertNil(CardBoxes.words(notice))
        XCTAssertEqual(CardLayout.above(notice.cards ?? []).first?.box, tea)
        // A restart draws the same one box: the rule reads the saved archive alone.
        let saved = try JSONDecoder().decode(ConversationArchive.self,
                                             from: XCTUnwrap(model.preferences.data(forKey: "FERBConversationArchive")))
        XCTAssertEqual(CardBoxes.moved(saved.transcript(on: Date())), moved)
    }

    // decision: APP-769
    func testAReminderIsOneBoxWithItsWordsInsideAndARepeatIsTheSameBox() throws {
        let model = makeModel()
        let id = begin("remind me at 10 to submit", in: model)
        model.receiveChatEvent(["type": "reply", "request_id": "R", "text": "Set.",
                                "cards": [["type": "event", "kind": "reminder", "id": "tm-r", "what": "Buildathon",
                                           "when": "2026-11-13T04:30:00+00:00"]]], localID: id)
        model.receiveReminder(id: "tm-r", text: "Submit your product for the Startup Buildathon.",
                              card: ResultCard(["type": "reminder", "title": "Buildathon", "kind": "reminder",
                                                "timer_id": "tm-r"]))
        guard case .notice(let reminder)? = model.visibleEntries.last else { return XCTFail("no arrival") }
        XCTAssertTrue(CardBoxes.boxSays(reminder))
        XCTAssertEqual(CardBoxes.words(reminder), "Submit your product for the Startup Buildathon.")   // nothing lost
        // A timer, then the timer its Repeat started (`repeat_of`: the chain's first timer, DMN-549): one box.
        model.receiveReminder(id: tea, text: "Tea's ready.",
                              card: ResultCard(["type": "reminder", "title": "Tea", "kind": "timer", "timer_id": tea]))
        model.receiveReminder(id: "tm-again", text: "Tea's ready.",
                              card: ResultCard(["type": "reminder", "title": "Tea", "kind": "timer",
                                                "timer_id": "tm-again", "repeat_of": tea]))
        let entries = model.visibleEntries
        let moved = CardBoxes.moved(entries)
        XCTAssertEqual(moved[entries[entries.count - 2].id], [tea])
        XCTAssertNil(moved[entries[entries.count - 1].id])
        guard case .notice(let again) = entries[entries.count - 1] else { return XCTFail("no repeat arrival") }
        XCTAssertEqual(again.cards?.first?.timerID, "tm-again")              // Stop and Repeat act on the new timer
        // A notice with real content keeps its bubble under the box.
        model.receiveNotices([["id": "quoted", "text": "From your calendar:", "quotes": [["text": "Standup", "origin": "Calendar"]],
                               "cards": [["type": "reminder", "title": "Standup", "kind": "reminder", "timer_id": "tm-s"]]]])
        guard case .notice(let quoted)? = model.visibleEntries.last else { return XCTFail("no quoted notice") }
        XCTAssertFalse(CardBoxes.boxSays(quoted))
    }

    // decision: APP-769
    func testAScheduleIsOneBoxAtItsLatestRunAndEachRunKeepsItsResult() {
        let model = makeModel()
        let id = begin("every day at 9 send me the news", in: model)
        model.receiveChatEvent(["type": "reply", "request_id": "R", "text": "🗓 Scheduled: News — every day at 9",
                                "cards": [["type": "event", "kind": "schedule", "id": "sc-news", "what": "News",
                                           "when": "2026-11-06T03:30:00+00:00", "repeat": "every day at 09:00",
                                           "line": "🗓 Scheduled: News — every day at 9"]]], localID: id)
        for (run, found) in [("1", "Three stories today."), ("2", "Two more stories.")] {
            model.receiveBusEvent(["topic": "task.result", "data": [
                "task_id": "sc-news:" + run, "status": "done", "goal": "News", "answer": found,
                "cards": [["type": "reminder", "title": "News", "kind": "schedule", "schedule_id": "sc-news",
                           "due": "2026-11-06T03:30:00+00:00", "repeat": "every day at 09:00",
                           "next_run": "2026-11-07T03:30:00+00:00"]]]])
        }
        let entries = model.visibleEntries
        XCTAssertEqual(entries.count, 3)
        let moved = CardBoxes.moved(entries)
        XCTAssertEqual(moved[entries[0].id], ["sc-news"])                     // the set card leaves
        XCTAssertEqual(moved[entries[1].id], ["sc-news"])                     // the first run's box leaves
        XCTAssertNil(moved[entries[2].id])                                    // the latest run shows it, with the next
        guard case .turn(let first, _) = entries[1], case .turn(let latest, _) = entries[2] else { return XCTFail() }
        XCTAssertEqual(CardLayout.bubble(first.answer, first.cards ?? [], note: nil, lines: []), "Three stories today.")
        XCTAssertEqual(CardLayout.above(latest.cards ?? []).first?.nextRun, "2026-11-07T03:30:00+00:00")
        // Other cards are never boxes.
        XCTAssertNil(ResultCard(["type": "note", "item_id": "m", "text": "x"])?.box)
        XCTAssertNil(ResultCard(["type": "event", "what": "x"])?.box)
    }

    // decision: APP-769
    func testABoxSetOnAnEarlierDayShowsTodayAtTheBottomAndThatDayKeepsItsRecord() throws {
        var archive = ConversationArchive()
        let yesterday = Date().addingTimeInterval(-86_400)
        let id = UUID()
        archive.begin(id: id, request: "remind me tomorrow to call the bank", sources: [], date: yesterday)
        archive.showCards(id: id, [["type": "event", "kind": "reminder", "id": "tm-bank", "what": "Call the bank",
                                    "when": "2026-11-06T04:30:00+00:00"]])
        archive.addNotice(id: "reminder:tm-bank", text: "Call the bank.", tag: nil,
                          cards: [try XCTUnwrap(ResultCard(["type": "reminder", "title": "Call the bank",
                                                            "kind": "reminder", "timer_id": "tm-bank"]))])
        XCTAssertTrue(CardBoxes.moved(archive.transcript(on: yesterday)).isEmpty)   // that day: its set card, as stored
        let today = archive.transcript(on: Date())
        XCTAssertEqual(today.count, 1)                                              // today: the one box at the bottom
        XCTAssertTrue(CardBoxes.moved(today).isEmpty)
    }

    // ---------- helpers ----------

    private func makeModel() -> NotchModel {
        let model = NotchModel(rpc: ScriptedRPC(), preferences: scratchDefaults("OneBox"))
        model.feedback.player = FakeSoundPlayer()
        model.collapse()
        return model
    }

    private func begin(_ request: String, in model: NotchModel) -> UUID {
        model.session.draft = request
        let id = model.session.beginWork()!
        model.conversation.begin(id: id, request: request, sources: [])
        return id
    }
}
