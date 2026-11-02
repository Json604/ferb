import XCTest
@testable import FERBNotch
import NotchCore

@MainActor
final class ReminderArrivalTests: XCTestCase {
    private func make() -> (NotchModel, FakeSoundPlayer) {
        let model = NotchModel(rpc: ScriptedRPC(), preferences: scratchDefaults())
        let sound = FakeSoundPlayer()
        model.feedback.player = sound
        model.collapse()
        return (model, sound)
    }

    private var scheduled: [String: Any] {
        ["task_id": "morning-1", "status": "done", "goal": "Morning brief", "answer": "Your brief is ready.",
         "cards": [["type": "reminder", "title": "Morning brief", "schedule_id": "morning"]]]
    }

    // decision: APP-622
    func testScheduledResultArrivesCompactOnceAndPersistsWithoutFocus() throws {
        let (model, sound) = make()
        var focuses = 0
        model.onPresentationChange = { _ in focuses += 1 }
        model.receiveOutcome(scheduled)
        XCTAssertFalse(model.expanded)
        XCTAssertEqual(model.compactContent, .reminder(card: ResultCard(["type": "reminder", "title": "Morning brief", "schedule_id": "morning"])!, text: "Your brief is ready."))
        XCTAssertEqual(model.conversation.turns.last?.cards?.first?.type, "reminder")
        XCTAssertEqual(sound.played, [.notification])
        XCTAssertEqual(focuses, 0)
        let data = try XCTUnwrap(model.preferences.data(forKey: "FERBConversationArchive"))
        XCTAssertEqual(try JSONDecoder().decode(ConversationArchive.self, from: data).turns.last?.answer, "Your brief is ready.")
        model.acknowledgement.dismiss()
        model.receiveOutcome(scheduled)
        XCTAssertNil(model.compactPlanText)
        XCTAssertEqual(sound.played, [.notification])
        XCTAssertEqual(model.conversation.turns.count, 1)
    }

    // decision: APP-622
    func testOpenChatIsPreservedAndHistoryDoesNotAnnounce() {
        let (model, _) = make()
        model.expand(focus: false)
        model.session.draft = "A message I'm writing"
        model.receiveOutcome(scheduled)
        XCTAssertTrue(model.bodyVisible)
        XCTAssertNil(model.compactPlanText)
        XCTAssertEqual(model.session.draft, "A message I'm writing")
        model.collapse()
        var historical = scheduled
        historical["task_id"] = "morning-older"
        model.receiveOutcome(historical, announce: false)
        XCTAssertNil(model.compactPlanText)
        XCTAssertFalse(model.expanded)
        // Ordinary task completions still open their answer.
        model.receiveOutcome(["task_id": "manual", "status": "done", "answer": "Finished."])
        XCTAssertTrue(model.bodyVisible)
    }

    // decision: APP-622
    func testReminderNotificationUsesNotchAndSoundsOnceOnRedelivery() {
        let (model, sound) = make()
        let center = FakeNotificationCenter()
        let poster = NotificationPoster(center: center, settle: .zero)
        model.feedback.postingBanners = true
        poster.sounds = model.feedback
        poster.configure(notchShowing: { model.surfaceVisible }, open: { model.open($0) })
        poster.onReminder = { item in
            if case .reminder(let id) = item.target { model.receiveReminder(id: id, text: item.body) }
        }
        let event: [String: Any] = ["id": "reminder:water", "category": "reminder", "title": "FERB reminder",
            "body": "Get some water.", "target": ["kind": "reminder", "timer_id": "water"], "when_hidden": false]
        poster.receive(event)
        poster.receive(event)
        XCTAssertFalse(model.expanded)
        XCTAssertEqual(model.compactContent, .reminder(card: ResultCard(["type": "reminder", "title": "Get some water."])!, text: "Get some water."))
        XCTAssertEqual(model.conversation.notices.count, 1)
        XCTAssertEqual(model.conversation.notices.first?.cards?.first?.type, "reminder")
        XCTAssertTrue(center.posted.isEmpty)
        XCTAssertEqual(sound.played, [.notification])
        model.openReminderChat()
        XCTAssertTrue(model.bodyVisible)
        XCTAssertNil(model.compactPlanText)
        XCTAssertTrue(model.viewingToday)
    }

    // decision: APP-622
    func testHiddenNotchKeepsBannerFallbackAndSavesTheReminder() {
        let (model, _) = make()
        let center = FakeNotificationCenter()
        let poster = NotificationPoster(center: center, settle: .zero)
        poster.configure(notchShowing: { false }, open: { _ in })
        poster.onReminder = { item in
            if case .reminder(let id) = item.target { model.receiveReminder(id: id, text: item.body) }
        }
        poster.receive(["id": "reminder:water", "category": "reminder", "title": "FERB reminder",
                        "body": "Get some water.", "target": ["kind": "reminder", "timer_id": "water"]])
        XCTAssertEqual(center.posted.map(\.id), ["reminder:water"])
        XCTAssertEqual(model.conversation.notices.count, 1)
    }

    // decision: APP-622
    func testPinnedHeaderCanShowReminderWithoutUnpinningOrOpeningChat() {
        let (model, _) = make()
        model.expand(focus: false)
        model.togglePin()
        model.foldPinnedBody()
        XCTAssertTrue(model.isHeaderOnly)
        model.receiveOutcome(scheduled)
        XCTAssertTrue(model.isPinned)
        XCTAssertFalse(model.bodyVisible)
        XCTAssertEqual(model.compactContent, .reminder(card: ResultCard(["type": "reminder", "title": "Morning brief", "schedule_id": "morning"])!, text: "Your brief is ready."))
        XCTAssertEqual(model.surfaceSize.height, model.compactPlanHeight)
        model.acknowledgement.dismiss()
        model.finishCompactPreviewDismissal()
        XCTAssertTrue(model.isHeaderOnly)
        XCTAssertNil(model.compactContent)
    }

    // decision: APP-622
    func testTypedReminderNoticeKeepsItsCardAndGenericNoticesStayQuiet() {
        let (model, _) = make()
        model.receiveNotices([["id": "quiet", "text": "A background update."]])
        XCTAssertNil(model.compactPlanText)
        model.receiveNotices([["id": "due", "text": "Your review is due.",
                               "cards": [["type": "reminder", "title": "Review"]]]])
        XCTAssertEqual(model.compactContent, .reminder(card: ResultCard(["type": "reminder", "title": "Review"])!, text: "Your review is due."))
        XCTAssertEqual(model.conversation.notices.last?.cards?.first?.title, "Review")
    }
    // decision: APP-624
    func testSummoningACompactReminderOpensChatEvenWhenInteractionAlreadyOpensIt() {
        let (model, _) = make()
        model.receiveOutcome(scheduled)
        model.onInteraction = { [weak model] in
            if model?.compactPlanText != nil { model?.openReminderChat() }
        }
        model.toggle()
        XCTAssertTrue(model.bodyVisible)
        XCTAssertNil(model.compactPlanText)
    }

}
