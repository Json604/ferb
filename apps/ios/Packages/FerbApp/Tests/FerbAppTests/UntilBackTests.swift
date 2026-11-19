import FerbAppModel
import FerbCore
import FerbMobile
import XCTest

/// "Until I'm back at my Mac" from the phone (kb PHN-610, PHN-611; Android's UntilBackTest): offered only when the Mac
/// offers it, sent as its own answer, and the line while it holds.
@MainActor
final class UntilBackTests: XCTestCase {
    private var rig: Rig!
    private var phone: FakePhone { rig.phone }

    override func setUp() async throws { rig = Rig(); rig.session.unlocked() }
    override func tearDown() async throws { rig.tearDown() }

    private func screenQuestion(untilBack: Bool, options: [String] = ["Yes", "No"]) -> StatusQuestion {
        StatusQuestion(approvalId: "ap-s", kind: "approve", capability: "screen.foreground", taskId: "t-1", reason: "Use your screen in Keynote?",
                       options: options, held: false,
                       needs: QuestionNeeds(canAnswer: true, needsUnlock: false, hidden: false, fingerprint: false, challenge: "ch-1",
                                            problem: nil, untilBack: untilBack))
    }

    private func status(_ q: StatusQuestion?, screenUse: ScreenUse? = nil) -> PhoneStatus {
        PhoneStatus(locked: false, words: true, stopped: false, questions: q.map { [$0] } ?? [], running: [], results: [], screenUse: screenUse)
    }

    // decision: PHN-610, PHN-611
    func testOfferedOnlyWhenTheMacOffersIt() async {
        phone.view = emptyView()
        phone.status = status(screenQuestion(untilBack: true))
        let vm = rig.model()
        await vm.refresh()?.value
        XCTAssertEqual(vm.ui.questions.first?.untilBack, true)
        phone.status = status(screenQuestion(untilBack: false))
        await vm.refresh()?.value
        XCTAssertEqual(vm.ui.questions.first?.untilBack, false)
        phone.status = status(screenQuestion(untilBack: true, options: ["Small", "Large"]))
        await vm.refresh()?.value
        XCTAssertEqual(vm.ui.questions.first?.untilBack, false, "only on the gateway's Yes/No")
    }

    // decision: PHN-611, PHN-13
    func testChosenWhileTheMacsWindowIsClosedItUnlocksAndIsSentOnceMoreWithoutFaceID() async {
        phone.view = emptyView()
        phone.status = status(screenQuestion(untilBack: true))
        let vm = rig.model()
        await vm.refresh()?.value
        phone.answerScript = [.failed(FakePhone.refused("locked"))]
        await vm.answer("ap-s", .untilBack).value
        XCTAssertEqual(phone.answers.map(\.0), [.untilBack, .untilBack])
        XCTAssertTrue(phone.calls.contains("unlock"))
        XCTAssertEqual(rig.prompts.signers.map(\.kind), [.lock], "the app's lock key only, for the unlock")
        XCTAssertTrue(phone.answers.allSatisfy { $0.1 == nil }, "never signed itself")
    }

    // decision: PHN-611
    func testARefusalSaysTheMacsReasonAndTheQuestionStaysForYesOrNo() async {
        phone.view = emptyView()
        phone.status = status(screenQuestion(untilBack: true))
        let vm = rig.model()
        await vm.refresh()?.value
        phone.answerScript = [.failed(.Refused(detail: "Input Monitoring is off for FERB on the Mac.", linkCode: "screen_use_unavailable"))]
        await vm.answer("ap-s", .untilBack).value
        XCTAssertEqual(vm.ui.note, Note("err_mac_said", "Input Monitoring is off for FERB on the Mac."))
        XCTAssertTrue(vm.ui.questions.contains { $0.approvalId == "ap-s" }, "the question stays open")
        await vm.answer("ap-s", .approve(granted: true, choice: 1)).value
        XCTAssertNotNil(phone.answers.last)
    }

    // decision: PHN-611
    func testWhileItHoldsTheLineShowsAndAskMeAgainEndsIt() async {
        phone.view = emptyView()
        let active = ScreenUse(active: true, since: "2026-11-17T10:00:00+05:30", phone: "phone-1", phoneName: "iPhone", canOffer: true)
        phone.screenUseNow = active
        phone.status = status(nil, screenUse: active)
        let vm = rig.model()
        await vm.refresh()?.value
        XCTAssertEqual(vm.ui.status?.screenUse?.active, true)
        await vm.endScreenUse().value
        XCTAssertTrue(phone.calls.contains("end-screen-use"))
        XCTAssertEqual(vm.ui.status?.screenUse?.active, false)
        XCTAssertEqual(vm.ui.note, Note("screen_use_ended"))
        // ended on the Mac (he came back): the status says so and the line goes, nothing is posted
        phone.status = status(nil, screenUse: ScreenUse(active: false, since: nil, phone: nil, phoneName: nil, canOffer: true))
        await vm.refresh()?.value
        XCTAssertEqual(vm.ui.status?.screenUse?.active, false)
    }

    /// The notification's third action is "Yes, until I'm back at my Mac" when the Mac offers it.
    // decision: PHN-611
    func testTheNotificationOffersItWhenTheMacDoes() async {
        let planner = NotificationPlanner(prefs: rig.prefs)
        let q = Questions.fromStatus(screenQuestion(untilBack: true))
        let planned = planner.question(q, lockedNow: false, quietly: false)
        XCTAssertEqual(planned.buttons.map(\.choice), [1, 0, NotificationButton.untilBack])
        XCTAssertEqual(planned.buttons.last?.label, "Yes, until I'm back at my Mac")
    }
}
