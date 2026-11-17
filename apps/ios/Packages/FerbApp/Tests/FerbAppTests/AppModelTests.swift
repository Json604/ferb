import FerbAppModel
import FerbCore
import FerbMobile
import XCTest

/// The app's state over a fake phone: the gate, unlocking, the outbox, questions and Stop (Android's ViewModelTest).
@MainActor
final class AppModelTests: XCTestCase {
    private var rig: Rig!
    private var phone: FakePhone { rig.phone }
    private var session: Session { rig.session }

    override func setUp() async throws { rig = Rig() }
    override func tearDown() async throws { rig.tearDown() }

    // decision: PHN-8, PHN-125, PHN-505
    func testNothingShowsBeforeThePasscodeAndTheUnlock() async {
        phone.state = .starting
        let vm = rig.model()
        XCTAssertEqual(vm.ui.gate, .starting)
        phone.state = .open(keys: KeyHealth(device: .ok, lock: .ok, strong: .ok), wiped: nil)
        rig.screenLock = false
        vm.recheck()
        XCTAssertEqual(vm.ui.gate, .noScreenLock)
        rig.screenLock = true
        vm.recheck()
        XCTAssertEqual(vm.ui.gate, .locked)
        session.unlocked()
        await eventually("main") { vm.ui.gate == .main }
        phone.link = FakePhone.status(.idle, paired: false)
        await eventually("pair") { vm.ui.gate == .pair }
        phone.link = FakePhone.status(.pairingWaitForMac, paired: false, code: "482 719")
        await eventually("codes") { vm.ui.gate == .codes }
    }

    // decision: PHN-13, PHN-125, PHN-505
    func testOnePromptOpensTheAppAndUnlocksTheMac() async {
        let vm = rig.model()
        await vm.unlockApp().value
        XCTAssertFalse(session.locked)
        XCTAssertEqual(rig.prompts.signers.map(\.kind), [.lock])
        XCTAssertTrue(phone.calls.contains("unlock"))
        XCTAssertFalse(vm.ui.needsMacUnlock)
    }

    // decision: PHN-8
    func testACancelledPromptLeavesTheAppLocked() async {
        rig.prompts.cancel = true
        rig.prompts.localResult = false
        let vm = rig.model()
        await vm.unlockApp().value
        XCTAssertTrue(session.locked)
    }

    /// The app leaves for `seconds` and comes back: inactive, background, then active again.
    private func away(_ vm: AppModel, _ seconds: Double) async {
        vm.setWindowReady(false)
        session.foreground(false)
        rig.clock.t += seconds
        session.foreground(true)
        vm.setWindowReady(true)
        await settle()
    }

    // decision: PHN-615, PHN-616, PHN-8
    func testThePromptAppearsByItselfOnceWhenTheAppOpens() async {
        let vm = rig.model()
        vm.setWindowReady(true)
        await settle()
        XCTAssertEqual(rig.prompts.shown, 0, "nothing before the app is in front")
        vm.setWindowReady(false)
        session.foreground(true)
        await settle()
        XCTAssertEqual(rig.prompts.shown, 0, "nothing while the scene isn't active")
        vm.setWindowReady(true)
        await eventually("open") { !session.locked }
        XCTAssertEqual(rig.prompts.shown, 1)
        vm.setWindowReady(false) // Face ID's sheet: inactive, then active
        session.foreground(true)
        vm.setWindowReady(true)
        await settle()
        XCTAssertEqual(rig.prompts.shown, 1, "the prompt's own sheet closing asks nothing more")
    }

    // decision: PHN-615, PHN-15
    func testBackAfterTheRelockItAsksOnceAndAShortAbsenceAsksNothing() async {
        let vm = rig.model()
        session.foreground(true)
        vm.setWindowReady(true)
        await eventually("open") { !session.locked }
        await away(vm, 120)
        XCTAssertFalse(session.locked, "inside five minutes the app stays open")
        XCTAssertEqual(rig.prompts.shown, 1)
        await away(vm, AppLock.lockAfter + 1)
        await eventually("open again") { !session.locked }
        XCTAssertEqual(rig.prompts.shown, 2, "one prompt on the return after the relock")
    }

    // decision: PHN-615, PHN-616
    func testACancelledPromptWaitsForTheButtonOrAReturn() async {
        rig.prompts.cancel = true
        rig.prompts.localResult = false
        let vm = rig.model()
        session.foreground(true)
        vm.setWindowReady(true)
        await eventually("asked") { rig.prompts.shown == 1 }
        await settle()
        XCTAssertTrue(session.locked)
        XCTAssertEqual(vm.ui.gate, .locked, "the lock page stays")
        vm.setWindowReady(false)
        session.foreground(true)
        vm.setWindowReady(true)
        await settle()
        XCTAssertEqual(rig.prompts.shown, 1, "no loop after a cancel")
        await vm.unlockApp().value
        XCTAssertEqual(rig.prompts.shown, 2, "the button asks again")
        vm.setWindowReady(false)
        vm.setWindowReady(true)
        await settle()
        XCTAssertEqual(rig.prompts.shown, 2)
        await away(vm, 10)
        await eventually("asked on return") { rig.prompts.shown == 3 }
    }

    // decision: PHN-616
    func testTheOwnersOwnLockWaitsForHisTap() async {
        let vm = rig.model()
        session.foreground(true)
        session.unlocked()
        vm.setWindowReady(true)
        await settle()
        session.lockNow()
        await eventually("locked") { vm.ui.gate == .locked }
        vm.setWindowReady(false)
        vm.setWindowReady(true)
        await settle()
        XCTAssertEqual(rig.prompts.shown, 0)
    }

    // decision: PHN-615, PHN-616
    func testATurnOfThePhoneNeverAsksTwice() async {
        rig.prompts.cancel = true
        rig.prompts.localResult = false
        let vm = rig.model()
        session.foreground(true)
        vm.setWindowReady(true)
        await eventually("asked") { rig.prompts.shown == 1 }
        vm.setWindowReady(false)
        vm.setWindowReady(true)
        vm.setWindowReady(true)
        await settle()
        XCTAssertEqual(rig.prompts.shown, 1)
    }

    // decision: PHN-615, PHN-13
    func testAColdStartWaitingForTheLinkAsksOnceAndOnlyOnTheOwnersScreen() async {
        phone.link = FakePhone.status(.connecting)
        let vm = rig.model()
        session.foreground(true)
        vm.setWindowReady(true)
        await settle()
        XCTAssertEqual(rig.prompts.shown, 0, "waiting for the link (one prompt for the app and the Mac)")
        vm.setWindowReady(false) // a system alert over FERB while the link was awaited
        phone.link = FakePhone.status()
        await settle()
        await settle()
        XCTAssertEqual(rig.prompts.shown, 0)
        XCTAssertTrue(session.locked)
        vm.setWindowReady(true)
        await eventually("open") { !session.locked }
        XCTAssertEqual(rig.prompts.shown, 1)
    }

    private var locked: Outcome<Void> { .failed(.NeedsUnlock(detail: "locked", linkCode: "locked")) }

    // decision: PHN-640, PHN-641, PHN-13
    func testTheAppOpensWhenThePromptPassesNotWhenTheSlowMacAnswers() async {
        phone.answerHeld = true
        let vm = rig.model()
        vm.unlockApp()
        await eventually("open at once after Face ID") { !session.locked }
        XCTAssertEqual(rig.prompts.shown, 1)
        XCTAssertEqual(vm.ui.macLine, .opening)
        await eventually("the lock button is free again") { !vm.ui.busy.contains("unlock") }
        await eventually("a slow Mac is said, calmly") { vm.ui.macLine == .slow }
        phone.answerHeld = false
        await eventually("the line clears by itself") { vm.ui.macLine == nil }
        XCTAssertFalse(vm.ui.needsMacUnlock)
        XCTAssertEqual(rig.prompts.shown, 1, "nothing asked twice")
    }

    // decision: PHN-640, PHN-641
    func testAMacThatCantBeReachedNeverHoldsThePromptOrTheApp() async {
        phone.link = FakePhone.status(.connecting)
        let vm = rig.model()
        vm.unlockApp()
        await settle()
        XCTAssertEqual(rig.prompts.shown, 0, "waiting briefly for the link")
        await eventually("open by the prompt alone") { !session.locked }
        XCTAssertEqual(rig.prompts.localAsked, 1)
        XCTAssertTrue(rig.prompts.signers.isEmpty, "no challenge prompt")
        XCTAssertFalse(vm.ui.needsMacUnlock, "no control to tap: the line says the Mac can't be reached")
    }

    // decision: PHN-640, PHN-641
    func testASlowChallengeGivesThePromptAloneAndNeverALateSecondPrompt() async {
        phone.challengeHeld = true
        let vm = rig.model()
        vm.unlockApp()
        await eventually("open by the prompt alone") { !session.locked }
        XCTAssertEqual(rig.prompts.localAsked, 1)
        phone.challengeHeld = false // the challenge comes after all: no prompt appears out of nowhere
        await settle()
        XCTAssertEqual(rig.prompts.signers.reduce(0) { $0 + $1.asked }, 0)
    }

    // decision: PHN-640, PHN-641
    func testASecondTapWhileWaitingShowsThePromptAtOnce() async {
        phone.link = FakePhone.status(.connecting)
        let vm = rig.model()
        vm.unlockApp()
        await settle()
        XCTAssertEqual(rig.prompts.shown, 0)
        vm.unlockApp()
        await eventually("the second tap is never a silent no-op", timeout: 0.25) { !session.locked }
        XCTAssertEqual(rig.prompts.shown, 1)
    }

    // decision: PHN-640, PHN-641, PHN-225
    func testAReconnectWhileOpenNeitherLocksNorAsks() async {
        let vm = rig.model()
        await vm.unlockApp().value
        await eventually("open") { !session.locked && vm.ui.macLine == nil }
        phone.syncScript = [locked]
        phone.link = FakePhone.status(.connecting)
        await settle()
        phone.link = FakePhone.status()
        await settle()
        await settle()
        XCTAssertFalse(session.locked)
        XCTAssertEqual(rig.prompts.shown, 1, "nothing asked again")
        XCTAssertFalse(vm.ui.needsMacUnlock)
    }

    // decision: PHN-640, PHN-641, PHN-13
    func testTheMacLockedAfterTheOpenIsUnlockedOnceByItselfThenWaitsForATap() async {
        let vm = rig.model()
        vm.setWindowReady(true)
        await vm.unlockApp().value
        await eventually("open") { !session.locked && vm.ui.macLine == nil }
        phone.syncScript = [locked, locked, locked]
        phone.link = FakePhone.status(.connecting)
        await settle()
        phone.link = FakePhone.status()
        await eventually("one automatic unlock") { rig.prompts.shown == 2 }
        await eventually("its answer") { vm.ui.macLine == nil }
        XCTAssertFalse(vm.ui.needsMacUnlock)
        phone.syncScript = [locked, locked, locked]
        phone.link = FakePhone.status(.connecting)
        await settle()
        phone.link = FakePhone.status()
        await eventually("then the line he can tap") { vm.ui.needsMacUnlock }
        XCTAssertEqual(rig.prompts.shown, 2, "never a loop")
    }

    // decision: PHN-401, PHN-476
    func testAMessageWithTheMacOffWaitsInTheOutboxAndCanBeCancelled() async {
        session.unlocked()
        phone.view = emptyView()
        let vm = rig.model()
        vm.typed("Water the plants at 6")
        await vm.send().value
        XCTAssertEqual(vm.draft, "")
        guard case .outbox(_, let item)? = vm.ui.rows.last else { return XCTFail("the outbox row: \(vm.ui.rows)") }
        XCTAssertEqual(item.state, "waiting")
        await vm.cancelOutbox(item.id).value
        XCTAssertFalse(vm.ui.rows.contains { if case .outbox = $0 { return true } else { return false } })
        phone.sendWaitingFor = "unlock"
        phone.syncScript = Array(repeating: .failed(.NeedsUnlock(detail: "locked", linkCode: "locked")), count: 3)
        vm.typed("And the balcony")
        await vm.send().value
        await eventually("a message waiting for the unlock says so") { vm.ui.needsMacUnlock }
    }

    // decision: PHN-415, PHN-302
    func testAQuestionSlidesUpOnceAndGoesWhenAnsweredElsewhere() async {
        session.unlocked()
        session.foreground(true)
        phone.view = view(questionItem("ap-1"))
        let vm = rig.model()
        await vm.refresh()?.value
        XCTAssertEqual(vm.sheet, .asking(approvalId: "ap-1"))
        vm.show(nil)
        await vm.refresh()?.value
        XCTAssertNil(vm.sheet, "shown once: not again by itself")
        vm.show(.asking(approvalId: "ap-1"))
        phone.view = view(questionItem("ap-1", open: false))
        await vm.refresh()?.value
        XCTAssertNil(vm.sheet)
        XCTAssertEqual(vm.ui.note?.key, "question_gone")
        XCTAssertEqual(vm.saved.presented, ["ap-1"], "what was shown is kept across a relaunch, by id only")
    }

    // decision: PHN-125
    func testStopNeedsNoUnlock() async {
        session.unlocked()
        let vm = rig.model()
        await vm.stop("t-9").value
        XCTAssertEqual(phone.calls.filter { $0.hasPrefix("stop") }, ["stop:t-9"])
        XCTAssertTrue(rig.prompts.signers.isEmpty, "no prompt for Stop")
    }

    // decision: PHN-442, PHN-302
    func testPairingProblemsAreSaidInPlainWords() async {
        session.unlocked()
        phone.link = FakePhone.status(.idle, paired: false)
        let vm = rig.model()
        await vm.pair("FERB-ABC").value
        XCTAssertTrue(vm.ui.pairing)
        phone.link = FakePhone.status(.stopped, paired: false, problem: LinkProblem(code: "pair_window_closed", detail: "window closed", allow: nil))
        await eventually("pairing ended") { !vm.ui.pairing }
        XCTAssertEqual(vm.ui.pairError?.key, "pair_expired")
        XCTAssertEqual(AppModel.pairNote("device_limit", nil).key, "pair_limit")
        XCTAssertEqual(AppModel.pairNote("pair_secret_wrong", nil).key, "pair_wrong_code")
    }

    // decision: PHN-133, PHN-497
    func testAMacThatRemovedThisPhoneOrLostKeysAsksToPairAgain() async {
        session.unlocked()
        let vm = rig.model()
        phone.link = FakePhone.status(.stopped, paired: true, problem: LinkProblem(code: "device_revoked", detail: "removed", allow: nil))
        await eventually("removed") { vm.ui.gate == .removed }
        XCTAssertEqual(vm.ui.pairReason, .removed)
        phone.link = FakePhone.status()
        phone.state = .open(keys: KeyHealth(device: .ok, lock: .gone, strong: .gone), wiped: nil)
        await eventually("keys") { vm.ui.pairReason == .keys }
        XCTAssertEqual(vm.ui.gate, .removed)
    }

    // decision: PHN-302
    func testErrorsAreSaidInTheAppsWordsOrTheMacsOwn() {
        XCTAssertEqual(AppModel.errorNote(.Offline(detail: "no link", linkCode: nil)).key, "err_offline")
        XCTAssertEqual(AppModel.errorNote(.NeedsBetterPath(detail: "bulk", linkCode: "needs_direct_or_own_relay")).key, "err_needs_better_path")
        let mac = AppModel.errorNote(.Refused(detail: "That question isn't waiting any more, so nothing was sent.", linkCode: "question_not_waiting"))
        XCTAssertEqual(mac.key, "err_mac_said")
        XCTAssertEqual(mac.text, "That question isn't waiting any more, so nothing was sent.")
        XCTAssertEqual(FerbError.Offline(detail: "x", linkCode: nil).code, "offline")
    }

    // decision: PHN-15, PHN-505, PHN-571
    func testTheTabTheHomeScreenAndTheDraftComeBackAfterARelaunch() async {
        session.unlocked()
        let vm = rig.model()
        vm.choose(.tasks)
        vm.typed("Half a thought")
        let saved = vm.saved
        session.lockNow() // a relaunch starts locked (an unlock brings the home screen back, as on Android)
        let again = AppModel(api: phone, session: session, prefs: rig.prefs, local: rig.local, screenLock: { true }, corpus: nil, saved: saved)
        XCTAssertEqual(again.tab, .tasks)
        XCTAssertEqual(again.draft, "Half a thought")
        XCTAssertFalse(again.home)
    }
}
