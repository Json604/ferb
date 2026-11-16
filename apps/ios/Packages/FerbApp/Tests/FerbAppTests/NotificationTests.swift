import FerbAppModel
import FerbCore
import FerbMobile
import XCTest

/// What FERB's notifications carry, decided as plain data (Android's Notifier.kt and AnswerReceiver, PHN-415, PHN-417,
/// PHN-422, PHN-126): buttons only for a routine question, never words for a private answer or held words, FERB's own
/// checks wordless on a locked phone, nothing new while the owner looks at the app, and the answer from a button.
@MainActor
final class NotificationTests: XCTestCase {
    private var rig: Rig!
    private var planner: NotificationPlanner!
    override func setUp() async throws {
        rig = Rig()
        planner = NotificationPlanner(prefs: rig.prefs)
    }
    override func tearDown() async throws { rig.tearDown() }

    private func q(_ n: QuestionNeeds?, capability: String? = "plan.ask", held: Bool = false, hidden: Bool = false,
                   options: [String] = ["Yes", "No"]) -> Question {
        Question(approvalId: "ap-1", taskId: "t-1", title: "Book the table?", reason: "For two at 8", exact: nil,
                 kind: hidden ? "owner_secret" : "owner_question", capability: capability, options: options, labels: [], hidden: hidden,
                 needs: n, held: held)
    }

    private func posted(_ a: [NotificationAction]) -> [PlannedNotification] {
        a.compactMap { if case .post(let p) = $0 { return p } else { return nil } }
    }

    // decision: PHN-417, PHN-470, PHN-519
    func testButtonsOnlyForARoutineQuestionYesFirst() {
        let routine = planner.question(q(needs(unlock: false)), lockedNow: false, quietly: false)
        XCTAssertEqual(routine.buttons.map(\.choice), [1, 0], "Yes first")
        XCTAssertEqual(routine.title, "Book the table?")
        XCTAssertFalse(routine.wordless)
        let typed = planner.question(q(needs(unlock: false), options: []), lockedNow: false, quietly: false)
        XCTAssertEqual(typed.buttons.count, 1)
        XCTAssertNil(typed.buttons[0].choice, "no options: a typed answer")
        for n in [needs(unlock: true), needs(unlock: true, fingerprint: true), needs(unlock: false, can: false)] {
            XCTAssertTrue(planner.question(q(n), lockedNow: false, quietly: false).buttons.isEmpty, "\(n): opens FERB instead")
        }
        XCTAssertTrue(planner.question(q(nil), lockedNow: false, quietly: false).buttons.isEmpty, "unknown needs: no buttons")
    }

    // decision: PHN-422, PHN-504, PHN-126, PHN-110, PHN-573
    func testWordsLeaveThePrivateTheHeldAndFerbsChecksOnALockedPhone() {
        let secret = planner.question(q(needs(unlock: true, hidden: true), hidden: true), lockedNow: false, quietly: false)
        XCTAssertTrue(secret.wordless)
        XCTAssertFalse(secret.body.contains("For two"))
        let held = planner.question(q(needs(unlock: false), held: true), lockedNow: false, quietly: false)
        XCTAssertTrue(held.wordless)
        XCTAssertTrue(held.buttons.isEmpty)
        let check = q(needs(unlock: false), capability: "downloads.delete")
        let locked = planner.question(check, lockedNow: true, quietly: false)
        XCTAssertTrue(locked.wordless)
        XCTAssertTrue(locked.buttons.isEmpty)
        XCTAssertEqual(locked.kind, .check)
        XCTAssertFalse(planner.question(check, lockedNow: false, quietly: false).wordless, "unlocked: worded")
        rig.prefs.hideChecksOnLockScreen = false
        XCTAssertFalse(planner.question(check, lockedNow: true, quietly: false).wordless, "the owner's switch off: worded")
        rig.prefs.hideChecksOnLockScreen = true
        // FERB about to be suspended: the check on show loses its words
        let again = posted(planner.lockScreen(locked: true))
        XCTAssertEqual(again.map(\.id), ["q:ap-1"])
        XCTAssertTrue(again[0].wordless && again[0].quietly)
    }

    // decision: PHN-415, PHN-421, PHN-519
    func testWhatWasThereAtStartIsNotNewsAndAQuestionIsAnnouncedOnce() {
        let first = phoneStatus(asked("ap-1", needs(unlock: false)))
        XCTAssertTrue(planner.status(first, chat: nil, quiet: false, lockedNow: false).isEmpty, "primed: not news")
        let second = phoneStatus(asked("ap-1", needs(unlock: false)), asked("ap-2", needs(unlock: true)),
                                 results: [StatusResult(taskId: "t-9", requestId: nil, at: nil, status: "failed", name: "Invoice", answer: "", held: false)])
        let a = posted(planner.status(second, chat: nil, quiet: false, lockedNow: false))
        XCTAssertEqual(a.map(\.id), ["q:ap-2", "t:t-9"])
        XCTAssertEqual(a[1].title, L.s("n_task_failed"))
        XCTAssertTrue(planner.status(second, chat: nil, quiet: false, lockedNow: false).isEmpty, "once")
        // its needs changed: re-posted quietly; gone from the Mac: removed
        let changed = phoneStatus(asked("ap-2", needs(unlock: false)))
        let b = planner.status(changed, chat: nil, quiet: false, lockedNow: false)
        XCTAssertEqual(posted(b).map(\.id), ["q:ap-2"])
        XCTAssertTrue(posted(b)[0].quietly)
        XCTAssertEqual(planner.status(phoneStatus(), chat: nil, quiet: false, lockedNow: false), [.remove(id: "q:ap-2")])
        // the owner looking at the open app: marked, not posted
        XCTAssertTrue(planner.status(phoneStatus(asked("ap-3", needs(unlock: false))), chat: nil, quiet: true, lockedNow: false).isEmpty)
        XCTAssertTrue(rig.prefs.announced.contains("q:ap-3"))
    }

    // decision: PHN-421, PHN-302
    func testARemovedPhoneIsToldOnce() {
        let gone = FakePhone.status(.stopped, paired: true, problem: LinkProblem(code: "device_revoked", detail: "removed", allow: nil))
        XCTAssertEqual(posted(planner.link(gone)).map(\.id), ["link"])
        XCTAssertTrue(planner.link(gone).isEmpty, "once")
        XCTAssertTrue(planner.link(FakePhone.status(.stopped, paired: true, problem: LinkProblem(code: "mac_unreachable", detail: "", allow: nil))).isEmpty)
    }

    // decision: PHN-415, PHN-417, PHN-503, PHN-519
    func testAnAnswerFromANotificationGoesUnprovedOrSendsTheOwnerToTheApp() async {
        var delivered: [NotificationAction] = []
        let notifier = Notifier(api: rig.phone, prefs: rig.prefs, session: rig.session, lockedNow: { true }, deliver: { delivered += $0 })
        rig.phone.status = phoneStatus(asked("ap-1", needs(unlock: false)), asked("ap-2", needs(unlock: true, fingerprint: true)))
        let yes = await notifier.answer("ap-1", choice: 1, typed: nil)
        XCTAssertEqual(yes, .remove(id: "q:ap-1"))
        XCTAssertEqual(rig.phone.calls.filter { $0.hasPrefix("answer") }, ["answer:ap-1:false"])
        XCTAssertEqual(rig.phone.answers.first?.0, .approve(granted: true, choice: 1))
        guard case .post(let p)? = await notifier.answer("ap-2", choice: 1, typed: nil) else { return XCTFail("needs the app") }
        XCTAssertEqual(p.tap, .question("ap-2"))
        XCTAssertEqual(rig.phone.calls.filter { $0.hasPrefix("answer") }.count, 1, "nothing sent for a Yes that needs Face ID")
        rig.phone.answerScript = [.failed(FakePhone.refused("unlock_required"))]
        guard case .post(let r)? = await notifier.answer("ap-1", choice: 0, typed: nil) else { return XCTFail("refused: the app") }
        XCTAssertEqual(r.title, L.bio("n_needs_fingerprint_title"))
        guard case .post(let gone)? = await notifier.answer("ap-9", choice: 0, typed: nil) else { return XCTFail("gone") }
        XCTAssertEqual(gone.body, L.s("question_gone"))
        XCTAssertEqual(delivered.count, 4)
    }
}
