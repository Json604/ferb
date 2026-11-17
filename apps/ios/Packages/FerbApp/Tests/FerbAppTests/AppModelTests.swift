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

    // decision: PHN-8, PHN-125, PHN-505, PHN-633
    func testNothingShowsBeforeThePasscodeAndTheUnlock() async {
        // PHN-633: the lock page needs neither the library, the store nor the link: it shows while the phone starts
        phone.state = .starting
        rig.screenLock = false
        let vm = rig.model()
        XCTAssertEqual(vm.ui.gate, .noScreenLock)
        rig.screenLock = true
        vm.recheck()
        XCTAssertEqual(vm.ui.gate, .locked)
        // unlocked before the phone opened: still nothing of the app's content until it is open (PHN-8)
        session.unlocked()
        await eventually("starting") { vm.ui.gate == .starting }
        session.lockNow()
        await eventually("locked") { vm.ui.gate == .locked }
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
        await eventually("the statement, not a second prompt") { phone.calls.contains("statement") }
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

    private var lockedRefusal: Outcome<Void> { .failed(.NeedsUnlock(detail: "locked", linkCode: "locked")) }

    // decision: PHN-649, PHN-640
    func testAColdStartAsksAtOnceBeforeThePhoneIsOpenAndTheStatementGoesWhenItIs() async {
        phone.state = .starting
        phone.link = nil
        let vm = rig.model()
        session.foreground(true)
        vm.setWindowReady(true)
        await eventually("open at once") { !session.locked }
        XCTAssertEqual(rig.prompts.shown, 1)
        XCTAssertTrue(phone.statements.isEmpty)
        rig.clock.t += 4
        phone.state = .open(keys: KeyHealth(device: .ok, lock: .ok, strong: .ok), wiped: nil)
        phone.link = FakePhone.status()
        await eventually("handed over once the link is up, with its age") { !phone.statements.isEmpty }
        XCTAssertEqual(phone.statements.map(\.1), [4000])
        XCTAssertEqual(rig.prompts.shown, 1, "exactly one prompt")
        XCTAssertFalse(phone.calls.contains("unlock"))
        XCTAssertTrue(phone.statements[0].0.contains("\"domain\":\"ferb.unlock-statement.v1\""))
    }

    // decision: PHN-649
    func testAPhoneNotPairedGetsThePromptAlone() async {
        phone.statementOk = false
        let vm = rig.model()
        await vm.unlockApp().value
        XCTAssertEqual(rig.prompts.localAsked, 1)
        XCTAssertTrue(rig.prompts.signers.isEmpty)
        XCTAssertFalse(session.locked)
    }

    // decision: PHN-649, PHN-641
    func testAMacThatIsntThereNeverHoldsThePromptAndAsksOnceWhenItComesTooLate() async {
        phone.link = FakePhone.status(.connecting)
        let vm = rig.model()
        vm.setWindowReady(true)
        await vm.unlockApp().value
        XCTAssertEqual(rig.prompts.shown, 1)
        XCTAssertFalse(session.locked)
        rig.clock.t += 200
        await settle()
        XCTAssertTrue(phone.statements.isEmpty, "too old: never sent")
        XCTAssertNil(vm.ui.macNeed)
        phone.syncScript = [lockedRefusal, lockedRefusal, lockedRefusal]
        phone.link = FakePhone.status()
        await eventually("the card, once, with why") { vm.ui.macNeed == .old }
        XCTAssertEqual(rig.prompts.shown, 1, "no prompt by itself")
        await vm.unlockMac().value
        XCTAssertEqual(rig.prompts.shown, 2)
        XCTAssertNil(vm.ui.macNeed)
    }

    // decision: PHN-649, PHN-641, PHN-225
    func testAStormOfReconnectsNeitherLocksNorAsks() async {
        let vm = rig.model()
        await vm.unlockApp().value
        await eventually("handed") { !phone.statements.isEmpty }
        for _ in 0..<5 {
            phone.syncScript = [lockedRefusal]
            phone.link = FakePhone.status(.connecting)
            await settle()
            phone.link = FakePhone.status()
            await settle()
        }
        XCTAssertFalse(session.locked)
        XCTAssertEqual(rig.prompts.shown, 1)
        XCTAssertNil(vm.ui.macNeed)
    }

    // decision: PHN-649
    func testAStatementBeforeTheConnectionProvedItselfIsSentAgain() async {
        phone.statementResults = [lockedRefusal, lockedRefusal]
        let vm = rig.model()
        await vm.unlockApp().value
        await eventually("the third try") { phone.statements.count == 3 }
        XCTAssertEqual(rig.prompts.shown, 1)
        XCTAssertNil(vm.ui.macNeed)
    }

    // decision: PHN-649
    func testAStatementTheMacFoundTooOldAsksOnceWithTheCard() async {
        phone.statementResults = [.failed(.NeedsUnlock(detail: "too long ago", linkCode: "unlock_required"))]
        let vm = rig.model()
        await vm.unlockApp().value
        await eventually("the card") { vm.ui.macNeed == .old }
        rig.prompts.cancel = true
        await vm.unlockMac().value
        XCTAssertEqual(vm.ui.macNeed, .cancelled, "he closed the prompt: the card says so")
    }

    // decision: PHN-649
    func testAMacThatDoesntKnowThisPhonesKeysAsksToPairAgain() async {
        phone.statementResults = [.failed(.NeedsUnlock(detail: "not this phone's", linkCode: "unlock_invalid"))]
        let vm = rig.model()
        await vm.unlockApp().value
        await eventually("pair again") { vm.ui.gate == .removed }
        XCTAssertEqual(vm.ui.pairReason, .unknown)
    }

    // decision: PHN-649, PHN-313
    func testAnOlderMacGetsItsOwnUnlock() async {
        var l = FakePhone.status()
        l.macOlder = true
        phone.link = l
        let vm = rig.model()
        await vm.unlockApp().value
        await eventually("the Mac's own unlock") { phone.calls.contains("unlock") }
        XCTAssertTrue(phone.statements.isEmpty)
        XCTAssertEqual(rig.prompts.shown, 2, "one more prompt, only with an older Mac")
    }

    // decision: PHN-648
    func testTheMacIsToldTheAppIsOpenOnlyWhileItIsUnlockedAndInFront() async {
        let vm = rig.model()
        session.foreground(true)
        await vm.unlockApp().value
        await eventually("open") { phone.opens.last == true }
        session.foreground(false)
        await eventually("left") { phone.opens.last == false }
        _ = vm
    }

    // decision: PHN-646
    func testForgettingAMacThatCouldntBeToldSaysItStillListsThisPhone() async {
        session.unlocked()
        phone.macForgot = false
        let vm = rig.model()
        await vm.forget().value
        XCTAssertEqual(vm.ui.note, Note("forget_mac_kept"))
    }

    // decision: PHN-401, PHN-476
    func testAMessageWithTheMacOffWaitsInTheOutboxAndCanBeCancelled() async {
        session.unlocked()
        phone.view = emptyView()
        let vm = rig.model()
        vm.typed("Water the plants at 6")
        await vm.send().value
        XCTAssertEqual(vm.draft, "")
        guard case .outbox(_, let item)? = vm.rows.last else { return XCTFail("the outbox row: \(vm.rows)") }
        XCTAssertEqual(item.state, "waiting")
        await vm.cancelOutbox(item.id).value
        XCTAssertFalse(vm.rows.contains { if case .outbox = $0 { return true } else { return false } })
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

    // decision: PHN-15, PHN-505, PHN-571, PHN-696
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
        // typing never left today's home page (kb PHN-696): nothing was said, so it is still the home page
        XCTAssertTrue(again.home)
    }
}
