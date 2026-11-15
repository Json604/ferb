import XCTest
@testable import FERBNotch
@testable import NotchCore

/// Typed cards in the notch (step 8e), driven by events shaped as the daemon sends them: which card shows is the
/// daemon's `type`; a card stands for exactly the line it names; History keeps the cards; a turn saved before cards
/// draws as before. Also the 8d protocol gaps on the app's side: the daemon's part count, questions from
/// `chat.status` placed in their rows.
@MainActor
final class TypedCardsTests: XCTestCase {
    // ---------- decoding and selection ----------

    func testEachCardTypeDecodesItsOwnFieldsAndAnUnknownTypeIsNotDrawn() throws {
        let cards = try XCTUnwrap(ResultCard.list([
            ["type": "reminder", "title": "Apply to YC", "due": "2026-10-31T09:00:00+00:00", "repeat": "every 180 minutes",
             "next_run": "2026-10-31T12:00:00+00:00", "schedule_id": "sc-1"],
            ["type": "event", "kind": "schedule", "id": "sc-2", "what": "write report.md",
             "when": "2026-11-01T03:30:00+00:00", "repeat": "every day at 09:00", "line": "🗓 Scheduled: write report.md"],
            ["type": "note", "item_id": "m-1", "text": "my dentist is Dr. Rao", "state": "saved",
             "line": "Remembered: my dentist is Dr. Rao"],
            ["type": "payment", "amount": "₹248.75", "merchant": "Orderly", "site": "www.orderly.example"],
            ["type": "send", "kind": "send", "recipient": "alex@work.example", "site": "mail.google.com"],
            ["type": "link", "host": "www.mysite.example", "line": "Sites contacted: www.mysite.example"],
            ["type": "file", "path": "/Users/me/notes.md", "name": "notes.md"],
            ["type": "hologram", "text": "a newer daemon's card"],          // not drawn by this app
            ["type": "note", "text": "no item id"],                          // nothing to act on: not a card
        ]))
        XCTAssertEqual(cards.map(\.type), ["reminder", "event", "note", "payment", "send", "link", "file"])
        XCTAssertEqual(cards[0].repeatRule, "every 180 minutes")
        XCTAssertEqual(cards[0].nextRunDate, LiveProgress.time("2026-10-31T12:00:00+00:00"))
        XCTAssertEqual(cards[1].kind, "schedule")
        XCTAssertEqual(cards[2].itemID, "m-1")
        XCTAssertEqual(cards[3].amount, "₹248.75")
        XCTAssertEqual(cards[4].initials, "A")                          // an address: its name part
        XCTAssertEqual(ResultCard.initials("Priya Sharma"), "PS")
        XCTAssertEqual(cards[5].monogram, "M")
        XCTAssertEqual(cards[6].fileURL?.path, "/Users/me/notes.md")
        XCTAssertEqual(CardLayout.above(cards).map(\.type), ["reminder"])
        XCTAssertEqual(CardLayout.below(cards).map(\.type), ["event", "note", "payment", "send"])
        XCTAssertNil(ResultCard.list(nil))                                   // an older daemon sends none
        XCTAssertNil(ResultCard.list([["type": "hologram"]]))
    }

    // decision: APP-789
    /// T-1066: the owner's schedules as the daemon's records hold them, and a schedule changed in place.
    func testTheSchedulesCardAndAChangedSchedulesCardDecode() throws {
        let cards = try XCTUnwrap(ResultCard.list([
            ["type": "schedules", "count": 2, "line": "🗓 Your schedules (2): Visa checks — every 360 minutes; Notes",
             "schedules": [["id": "sc-1", "name": "Visa checks", "repeat": "every 360 minutes", "state": "active",
                            "next_run": "2026-11-08T00:30:00+00:00"],
                           ["id": "sc-2", "name": "Notes", "repeat": "every day at 09:00", "state": "paused"],
                           ["name": "no id"]]],
            ["type": "schedules", "count": 0, "line": "🗓 You have no schedules.", "schedules": [Any]()],
            ["type": "event", "kind": "schedule", "id": "sc-1", "what": "Visa checks", "line": "🗓 Changed: Visa checks",
             "repeat": "every 360 minutes", "changed": true, "was_repeat": "every day at 05:20",
             "was_request": "check at 5:20", "request": "check every 6 hours"],
        ]))
        XCTAssertEqual(cards.map(\.type), ["schedules", "schedules", "event"])
        XCTAssertEqual(cards[0].schedules?.map(\.name), ["Visa checks", "Notes"])     // a row without an id isn't one
        XCTAssertEqual(cards[0].schedules?[0].nextRunDate, LiveProgress.time("2026-11-08T00:30:00+00:00"))
        XCTAssertNil(cards[0].schedules?[1].nextRun)
        XCTAssertEqual(cards[1].schedules, [])
        XCTAssertTrue(cards[2].changed)
        XCTAssertEqual(cards[2].wasRepeat, "every day at 05:20")
        XCTAssertEqual(cards[2].request, "check every 6 hours")
        XCTAssertEqual(CardLayout.below(cards).map(\.type), ["schedules", "schedules", "event"])
        XCTAssertEqual(CardLayout.strip("Here they are.\n🗓 You have no schedules.", [cards[1]]), "Here they are.")
    }

    // decision: APP-700
    /// Live 2026-11-04: three link chips ("example.com", "github.com", "history") under one answer. Link cards
    /// show as the sites row: the first site, "+2", and the list of all of them behind it.
    func testLinkCardsCollapseToOneSiteAndACountOfTheRest() throws {
        let line = "Sites contacted: example.com, github.com, docs.python.org"
        let cards = try XCTUnwrap(ResultCard.list(["example.com", "github.com", "docs.python.org"].map {
            ["type": "link", "host": $0, "line": line] as [String: Any]
        } + [["type": "file", "path": "/tmp/report.md"]]))
        let sites = LinkCardsRow.sites(cards)
        XCTAssertEqual(sites.inline, "example.com")
        XCTAssertEqual(sites.remainingCount, 2)
        XCTAssertEqual(sites.all, ["example.com", "github.com", "docs.python.org"])
        XCTAssertEqual(LinkCardsRow.sites(Array(cards.prefix(1))).remainingCount, 0)      // one site: no "+0"
        XCTAssertEqual(VisitedSites(hosts: ["a.example", "a.example", " "]).all, ["a.example"])
    }

    // decision: ORCH-329
    func testACardStandsForExactlyItsLineAndNothingElseInTheTextIsRead() {
        let note = ResultCard(["type": "note", "item_id": "m", "text": "x", "line": "Remembered: my dentist is Dr. Rao"])!
        let text = "Okay.\nRemembered: my dentist is Dr. Rao\nYou said Remembered: my dentist is Dr. Rao earlier."
        XCTAssertEqual(CardLayout.strip(text, [note]),
                       "Okay.\nYou said Remembered: my dentist is Dr. Rao earlier.")   // only the whole line goes
        XCTAssertEqual(CardLayout.strip("Okay.", [note]), "Okay.")
        let link = ResultCard(["type": "link", "host": "a.com", "line": "Sites contacted: a.com, b.com"])!
        XCTAssertEqual(CardLayout.remaining(["Sites contacted: a.com, b.com", "(ran late)"], [link]), ["(ran late)"])
    }

    func testTheCountdownSaysTheTimeLeftInShortWords() {
        let now = Date(timeIntervalSince1970: 1_000_000)
        XCTAssertEqual(Countdown.text(to: now.addingTimeInterval(40), now: now), "in 40s")
        XCTAssertEqual(Countdown.text(to: now.addingTimeInterval(12 * 60 + 5), now: now), "in 12m")
        XCTAssertEqual(Countdown.text(to: now.addingTimeInterval(2 * 3600 + 59 * 60), now: now), "in 2h 59m")
        XCTAssertEqual(Countdown.text(to: now.addingTimeInterval(3 * 86400 + 4 * 3600), now: now), "in 3d 4h")
        XCTAssertEqual(Countdown.text(to: now.addingTimeInterval(-5), now: now), "now")
    }

    // decision: ORCH-320
    func testACardCountsDownToTheSchedulesEndWhenItHasOneElseToTheNextRun() throws {
        let end = "2026-11-03T18:30:00+00:00", next = "2026-10-31T12:00:00+00:00"
        let cards = try XCTUnwrap(ResultCard.list([
            ["type": "reminder", "title": "Apply to YC", "next_run": next, "until": end],
            ["type": "reminder", "title": "Apply to YC", "next_run": next],                 // no end: as before
            ["type": "reminder", "title": "Apply to YC"],                                   // its last run: none
            ["type": "event", "kind": "schedule", "what": "Apply to YC", "when": next, "until": end],
            ["type": "event", "kind": "schedule", "what": "write report.md", "when": next],  // no end: chips only
        ]))
        let endDate = try XCTUnwrap(LiveProgress.time(end)), nextDate = try XCTUnwrap(LiveProgress.time(next))
        XCTAssertEqual(cards.map(\.countdown), [.end(endDate), .nextRun(nextDate), nil, .end(endDate), nil])
        XCTAssertEqual(cards[0].until, end)
        var utc = Calendar(identifier: .gregorian)
        utc.timeZone = TimeZone(identifier: "UTC")!
        let midnight = try XCTUnwrap(LiveProgress.time("2026-11-04T00:00:00+00:00"))
        XCTAssertEqual(CountdownTarget.lastDay(of: midnight, calendar: utc), midnight.addingTimeInterval(-1))
        XCTAssertNil(CountdownTarget.lastDay(of: endDate, calendar: utc))           // 18:30 shows as itself
    }

    // decision: ORCH-321
    func testAPaymentCardNamesTheSavedCardAndNothingElse() throws {
        let cards = try XCTUnwrap(ResultCard.list([
            ["type": "payment", "amount": "₹248.75", "merchant": "Orderly", "card": "Personal Visa"],
            ["type": "payment", "amount": "₹248.75", "merchant": "Orderly"],              // a one-time hidden input
        ]))
        XCTAssertEqual(cards.map(\.savedCard), ["Personal Visa", nil])
    }

    // ---------- turns ----------

    func testAReplysCardsAreKeptOnTheTurnAndInHistory() throws {
        let model = makeModel()
        let id = begin("remember that my dentist is Dr. Rao", in: model)
        model.receiveChatEvent(["type": "reply", "request_id": "R",
                                "text": "Okay.\nRemembered: my dentist is Dr. Rao",
                                "cards": [["type": "note", "item_id": "m-1", "text": "my dentist is Dr. Rao",
                                           "state": "saved", "line": "Remembered: my dentist is Dr. Rao"]]],
                               localID: id)
        let shown = turn(id, model)
        XCTAssertEqual(shown.cards?.map(\.itemID), ["m-1"])
        XCTAssertEqual(shown.answer, "Okay.\nRemembered: my dentist is Dr. Rao")   // the text stays as it was
        XCTAssertEqual(try savedArchive(model).turns.first?.cards, shown.cards)
    }

    // decision: APP-768
    /// Owner 2026-11-06: "i get these emptyu bbubbles eveytime on a timer request". A timer's reply is only the line
    /// its card stands for, so nothing is left for FERB's bubble: none is drawn. Words beside the line, a note or a
    /// code line keep it. A first reply whose plan line was only the direct answer above a card draws none either.
    func testAReplyThatIsOnlyItsCardsLinesDrawsNoBubble() throws {
        let line = "⏱ Timer set: Tea — ends 10:41"
        let timer = try XCTUnwrap(ResultCard(["type": "event", "kind": "timer", "id": "tm-1", "what": "Tea",
                                              "when": "2026-11-06T10:41:00+00:00", "line": line]))
        XCTAssertNil(CardLayout.bubble(line, [timer], note: nil, lines: []))
        XCTAssertNil(CardLayout.bubble(line + "\n\n", [timer], note: " ", lines: []))
        XCTAssertEqual(CardLayout.bubble("Done.\n" + line, [timer], note: nil, lines: []), "Done.")
        XCTAssertEqual(CardLayout.bubble(line, [timer], note: "Couldn't confirm", lines: []), "")
        XCTAssertEqual(CardLayout.bubble(line, [timer], note: nil, lines: ["(ran late)"]), "")
        XCTAssertEqual(CardLayout.bubble("Okay.", [], note: nil, lines: []), "Okay.")

        let model = makeModel()
        let id = begin("what's 2+2 and research laptops", in: model)
        model.receiveChatEvent(["type": "plan", "request_id": "R", "task_id": "T", "text": "4",
            "plan_line": "4", "lines": [String](), "direct": [["part": "sum", "label": "2+2", "text": "4"]],
            "card": ["request_id": "R", "done": 1, "total": 2, "rows": [
                ["part": "sum", "label": "2+2", "state": "done", "direct": true, "text": "4"],
                ["part": "laptops", "label": "Laptops", "state": "working"]]]], localID: id)
        XCTAssertEqual(turn(id, model).shownPlanLine, "")
        XCTAssertFalse(turn(id, model).firstReplySays)
        let plain = makeModel()
        let other = begin("research laptops", in: plain)
        plain.receiveChatEvent(["type": "plan", "request_id": "R", "task_id": "T", "text": "I'll look.",
                                "plan_line": "I'll look.", "lines": [String]()], localID: other)
        XCTAssertTrue(turn(other, plain).firstReplySays)
    }

    func testAnOutcomesCardsAndAFirstRepliesReceiptCardsAreKept() {
        let model = makeModel()
        let id = begin("check the gate and remind me in 10 minutes to stretch", in: model)
        model.receiveChatEvent(["type": "plan", "request_id": "R", "task_id": "T", "text": "Checking.",
                                "plan_line": "Checking.", "lines": [], "receipts": ["⏰ Reminder set for Sat: stretch"],
                                "cards": [["type": "event", "kind": "reminder", "id": "tm-1", "what": "stretch",
                                           "when": "2026-10-31T10:10:00+00:00", "line": "⏰ Reminder set for Sat: stretch"]]],
                               localID: id)
        XCTAssertEqual(turn(id, model).receiptCards?.map(\.what), ["stretch"])
        model.receiveChatEvent(["type": "outcome", "request_id": "R", "task_id": "T", "status": "done",
                                "answer": "Gate B12.\n\nSites contacted: example.com", "lines": ["Sites contacted: example.com"],
                                "cards": [["type": "link", "host": "example.com", "line": "Sites contacted: example.com"]]],
                               localID: id)
        XCTAssertEqual(turn(id, model).cards?.map(\.host), ["example.com"])
    }

    func testAScheduledRunsReminderCardReachesTheTurnItIsFiledAs() throws {
        let model = makeModel()
        model.receiveBusEvent(["topic": "task.result", "data": [
            "task_id": "sc-1:202610310900", "status": "done", "goal": "Apply to YC", "answer": "You wanted to apply.",
            "at": "2026-10-31T09:00:05+00:00",
            "cards": [["type": "reminder", "title": "Apply to YC", "due": "2026-10-31T09:00:00+00:00",
                       "repeat": "every 180 minutes", "next_run": "2026-10-31T12:00:00+00:00", "schedule_id": "sc-1"]]]])
        let filed = try XCTUnwrap(model.conversation.turns.first { $0.backendTaskID == "sc-1:202610310900" })
        XCTAssertEqual(filed.cards?.first?.type, "reminder")
        XCTAssertEqual(filed.answer, "You wanted to apply.")
    }

    func testATurnSavedBeforeCardsDecodesAndHasNone() throws {
        let old = #"{"turns":[{"id":"6F9619FF-8B86-D011-B42D-00C04FC964FF","createdAt":0,"request":"hi","sources":[],"#
            + #""answer":"Okay.\nRemembered: x","status":"done","artifacts":[]}]}"#
        let archive = try JSONDecoder().decode(ConversationArchive.self, from: Data(old.utf8))
        XCTAssertNil(archive.turns.first?.cards)
        XCTAssertNil(archive.turns.first?.receiptCards)
    }

    func testCorrectAndForgetUseTheMemoryRPCsAndTheCardSaysSo() async throws {
        let rpc = ScriptedRPC()
        rpc.replies["memory.forget"] = ["confirmed": false, "will_delete": ["memory item “my dentist is Dr. Rao”"]]
        let model = makeModel(rpc)
        let id = begin("remember that my dentist is Dr. Rao", in: model)
        model.receiveChatEvent(["type": "reply", "request_id": "R", "text": "Okay.",
                                "cards": [["type": "note", "item_id": "m-1", "text": "my dentist is Dr. Rao",
                                           "state": "saved", "line": "Remembered: my dentist is Dr. Rao"]]], localID: id)
        let corrected = expectation(description: "corrected")
        model.correctMemory(itemID: "m-1", text: "my dentist is Dr. Rao at Indiranagar") { ok in
            XCTAssertTrue(ok); corrected.fulfill()
        }
        await fulfillment(of: [corrected], timeout: 2)
        XCTAssertEqual(rpc.calls.last?.0, "memory.edit")
        XCTAssertEqual(rpc.calls.last?.1["content"] as? String, "my dentist is Dr. Rao at Indiranagar")
        XCTAssertEqual(turn(id, model).cards?.first?.state, "corrected")
        let planned = expectation(description: "plan")
        model.forgetPlan(itemID: "m-1") { plan in
            XCTAssertEqual(plan, ["memory item “my dentist is Dr. Rao”"]); planned.fulfill()
        }
        await fulfillment(of: [planned], timeout: 2)
        XCTAssertEqual(rpc.calls.last?.1["confirm"] as? Bool, false)          // a plan deletes nothing
        XCTAssertEqual(turn(id, model).cards?.first?.state, "corrected")
        let forgot = expectation(description: "forgot")
        model.forgetMemory(itemID: "m-1") { _ in forgot.fulfill() }
        await fulfillment(of: [forgot], timeout: 2)
        XCTAssertEqual(rpc.calls.last?.1["confirm"] as? Bool, true)
        XCTAssertEqual(turn(id, model).cards?.first?.state, "forgotten")
        XCTAssertEqual(try savedArchive(model).turns.first?.cards?.first?.state, "forgotten")
        // the same cards arriving again (an outcome read twice) don't undo the owner's forget
        model.conversation.showCards(id: id, [["type": "note", "item_id": "m-1", "text": "my dentist is Dr. Rao",
                                               "state": "saved"]])
        XCTAssertEqual(turn(id, model).cards?.first?.state, "forgotten")
    }

    // ---------- the 8d protocol gaps ----------

    func testThePartCountIsTheDaemons() throws {
        var card = try XCTUnwrap(TurnCard(["request_id": "R", "done": 0, "total": 3, "rows": [
            ["part": "a", "label": "A", "state": "working"], ["part": "b", "label": "B", "state": "working"]]],
            taskID: "T"))
        card.apply(part: ["part": "a", "state": "done", "seq": "2026-10-31T10:00:00+00:00", "done": 2, "total": 3])
        XCTAssertEqual(card.done, 2)                       // a direct part done elsewhere counts: the daemon says so
        card.apply(part: ["part": "b", "state": "done", "seq": "2026-10-31T10:00:01+00:00"])   // an older daemon
        XCTAssertEqual(card.done, 3)
    }

    func testAStepsQuestionFromChatStatusLandsInItsRow() throws {
        let model = makeModel()
        let id = begin("book a table and order lunch", in: model)
        model.receiveChatEvent(["type": "plan", "request_id": "R", "task_id": "T", "text": "On it.", "plan_line": "On it.",
                                "card": ["request_id": "R", "done": 0, "total": 2, "rows": [
                                    ["part": "book", "label": "Table", "state": "working", "step": "book",
                                     "step_task_id": "T.step.book"],
                                    ["part": "food", "label": "Lunch", "state": "working", "step": "food",
                                     "step_task_id": "T.step.food"]]]], localID: id)
        // as `chat.status` lists a gateway confirmation now: the root task, and the step task that asked
        let approval = try XCTUnwrap(model.normalizeApproval(["approval_id": "ap-1", "task_id": "T",
                                                              "source_task_id": "T.step.food.sub", "reason": "Pay?",
                                                              "capability": "ferb.confirm", "options": ["Yes", "No"]]))
        XCTAssertEqual(model.conversation.rowPlacements([approval])["ap-1"], RowPlace(turnID: id, part: "food"))
    }

    // ---------- helpers ----------

    private func makeModel(_ rpc: DaemonRPCTransport = ScriptedRPC()) -> NotchModel {
        NotchModel(rpc: rpc, preferences: scratchDefaults("TypedCards"))
    }

    private func begin(_ request: String, in model: NotchModel) -> UUID {
        model.session.draft = request
        let id = model.session.beginWork()!
        model.conversation.begin(id: id, request: request, sources: [])
        return id
    }

    private func turn(_ id: UUID, _ model: NotchModel) -> ConversationTurn {
        model.conversation.turns.first { $0.id == id }!
    }

    private func savedArchive(_ model: NotchModel) throws -> ConversationArchive {
        try JSONDecoder().decode(ConversationArchive.self, from: XCTUnwrap(model.preferences.data(forKey: "FERBConversationArchive")))
    }
}
