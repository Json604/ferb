import XCTest
@testable import FERBNotch
@testable import NotchCore

/// What arrives in a chat (an answer, a task's end, a question) is shown the same way in the saved chat and the
/// temporary chat: the notch opens without taking focus, the camera edge glows, the sound plays once. The owner's live
/// case (2026-11-24): "issue in temp mode, it doesnt open when it answer is complete. i told you to match normal chat and
/// temp chat UI, why are they even diff???". Each test runs the same steps on both chats.
@MainActor
final class BothChatsArrivalTests: XCTestCase {
    // An answer that completes the turn opens the notch (no focus) and glows; a done answer plays no sound of its own
    // (APP-533: only a failure or a wait on the owner sounds), so it sounds exactly as the saved chat's.
    // decision: APP-1126, APP-571, APP-564, APP-533
    func testACompletedAnswerOpensTheNotchWithoutFocusAndGlows() {
        for source in bothChats {
            let (model, sounds, _) = chatModel(source, "AnswerArrives")
            let id = model.sendFromComposer("What's 2+2?")
            model.collapse()
            var focus = 0
            model.onPresentationChange = { _ in focus += 1 }

            model.receiveChatEvent(["type": "reply", "request_id": "r1", "text": "4", "status": "done"], localID: id)

            XCTAssertTrue(model.bodyVisible, "\(source): it opens")
            XCTAssertEqual(focus, 0, "\(source): without taking focus")
            XCTAssertTrue(model.showsCompletionGlow, "\(source): the completion glow")
            XCTAssertEqual(sounds.played, [], "\(source): a done answer sounds as in the saved chat")
            XCTAssertEqual(model.shownTurn(id)?.answer, "4", "\(source)")
            XCTAssertEqual(model.shownTurn(id)?.status, "done", "\(source)")
            model.disconnectDaemon()
        }
    }

    // A failed answer opens the notch and sounds the error once.
    // decision: APP-1126, APP-571, APP-533
    func testAFailedAnswerOpensTheNotchAndSoundsOnce() {
        for source in bothChats {
            for frame: [String: Any] in [["type": "reply", "text": "The page didn't load.", "status": "failed"],
                                          ["type": "error", "text": "FERB could not start this request."]] {
                let (model, sounds, _) = chatModel(source, "FailedArrives")
                let id = model.sendFromComposer("Open the page")
                model.collapse()
                model.receiveChatEvent(frame, localID: id)
                XCTAssertTrue(model.bodyVisible, "\(source) \(frame["type"]!)")
                XCTAssertEqual(sounds.played, [.error], "\(source) \(frame["type"]!): once")
                model.disconnectDaemon()
            }
        }
    }

    // A task the message started: its end (its stream's outcome) opens the notch, glows and files its answer in the
    // chat it was asked in; its running row leaves that chat's work row.
    // decision: APP-1126, APP-571, APP-564, APP-1125
    func testATasksEndOpensTheNotchAndGlows() {
        for source in bothChats {
            let (model, sounds, _) = chatModel(source, "TaskEnds")
            let id = model.sendFromComposer("Tidy my desktop")
            model.receiveChatEvent(["type": "plan", "task_id": "task-1", "text": "I'll tidy it.", "name": "Tidy"],
                                   localID: id)
            XCTAssertEqual(model.workRowText, "1 active task", "\(source): its work row")
            XCTAssertEqual(model.taskItems.map(\.id), ["task-1"], "\(source)")
            model.collapse()
            var focus = 0
            model.onPresentationChange = { _ in focus += 1 }

            model.receiveChatEvent(["type": "outcome", "task_id": "task-1", "status": "done", "answer": "Tidied."],
                                   localID: id)

            XCTAssertTrue(model.bodyVisible, "\(source): it opens")
            XCTAssertEqual(focus, 0, "\(source)")
            XCTAssertTrue(model.showsCompletionGlow, "\(source)")
            XCTAssertEqual(sounds.played, [], "\(source)")
            XCTAssertEqual(model.shownTurn(id)?.answer, "Tidied.", "\(source)")
            XCTAssertFalse(model.showsWorkRow, "\(source): its row ends")
            model.disconnectDaemon()
        }
    }

    // A question a chat's task asks opens the notch (no focus), glows as attention and sounds needs you once, where
    // that chat draws it.
    // decision: APP-1126, APP-301, APP-571, APP-533
    func testAQuestionOpensTheNotchAndSoundsNeedsYouOnce() {
        for source in bothChats {
            let (model, sounds, _) = chatModel(source, "QuestionArrives")
            let id = model.sendFromComposer("Send the file to Sam")
            model.receiveChatEvent(["type": "plan", "task_id": "task-q", "text": "I'll send it."], localID: id)
            model.collapse()
            var focus = 0
            model.onPresentationChange = { _ in focus += 1 }

            let question: [String: Any] = ["approval_id": "q-1", "task_id": "task-q", "kind": "owner_question",
                                           "reason": "Send to Sam for the first time?", "options": ["Yes", "No"]]
            model.receiveBusEvent(["topic": "approval.requested", "data": question])

            XCTAssertTrue(model.bodyVisible, "\(source): it opens")
            XCTAssertEqual(focus, 0, "\(source)")
            XCTAssertTrue(model.attentionTraceVisible, "\(source)")
            XCTAssertEqual(sounds.played, [.needsYou], "\(source): once")
            XCTAssertEqual(model.inlineQuestions.map(\.id), ["q-1"], "\(source): drawn in the chat it belongs to")
            XCTAssertEqual(model.isTemporaryApproval(model.pendingApprovals[0]), source == .temporary, "\(source)")

            model.collapse()                                  // the same question again: it doesn't open twice
            model.receiveBusEvent(["topic": "approval.requested", "data": question])
            XCTAssertFalse(model.bodyVisible, "\(source)")
            XCTAssertEqual(sounds.played, [.needsYou], "\(source)")
            model.disconnectDaemon()
        }
    }

    // The same pipeline, the data source apart: a temporary chat's message goes with `fresh`, and nothing of it reaches
    // the saved chat (its conversation or its history).
    // decision: APP-1126, APP-1122, ORCH-699
    func testOnlyTheDataSourceDiffers() async {
        for source in bothChats {
            let (model, _, rpc) = chatModel(source, "DataSource")
            let id = model.sendFromComposer("Draft a haiku")
            model.receiveChatEvent(["type": "reply", "text": "Rain on the window", "status": "done"], localID: id)
            let deadline = Date().addingTimeInterval(30)
            while rpc.calls.filter({ $0.0 == "chat.send" }).isEmpty && Date() < deadline {
                try? await Task.sleep(for: .milliseconds(5))
            }
            let send = rpc.calls.first { $0.0 == "chat.send" }?.1
            XCTAssertEqual(send?["fresh"] as? Bool, source == .temporary ? true : nil, "\(source)")
            XCTAssertEqual(model.savedConversation.turns.map(\.id), source == .saved ? [id] : [], "\(source)")
            XCTAssertEqual(model.temporaryChat?.archive.turns.map(\.id) ?? [], source == .temporary ? [id] : [],
                           "\(source)")
            model.disconnectDaemon()
        }
    }
}
