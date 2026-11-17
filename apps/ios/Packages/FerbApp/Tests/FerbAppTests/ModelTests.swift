import FerbAppModel
import FerbCore
import FerbDesign
import FerbMobile
import XCTest

final class AppLockTests: XCTestCase {
    // decision: PHN-8, PHN-15, PHN-505
    func testLockedOnAColdStartAndAgainAfterFiveMinutesAway() {
        var t = 0.0
        let lock = AppLock { t }
        XCTAssertTrue(lock.locked, "a cold start is locked")
        lock.unlocked()
        lock.wentToBackground()
        t += AppLock.lockAfter // exactly five minutes: still open
        lock.cameToFront()
        XCTAssertFalse(lock.locked)
        lock.wentToBackground()
        t += AppLock.lockAfter + 0.001
        lock.cameToFront()
        XCTAssertTrue(lock.locked, "away longer than five minutes")
    }

    // decision: PHN-615, PHN-616
    func testOnePromptIsDuePerArrivalInFrontWhileLocked() {
        var t = 0.0
        let lock = AppLock { t }
        XCTAssertFalse(lock.promptDue, "a new process not yet in front")
        lock.cameToFront()
        XCTAssertTrue(lock.takePrompt())
        XCTAssertFalse(lock.takePrompt(), "taken once")
        lock.cameToFront()
        XCTAssertFalse(lock.takePrompt(), "told again while in front")
        lock.promptAgain()
        XCTAssertTrue(lock.takePrompt())
        lock.unlocked()
        lock.lock()
        XCTAssertFalse(lock.takePrompt(), "his own lock waits for his tap")
        lock.wentToBackground()
        lock.promptAgain()
        XCTAssertFalse(lock.promptDue, "nothing is due while away")
        lock.cameToFront()
        XCTAssertTrue(lock.takePrompt(), "locked and back: due again")
        lock.unlocked()
        lock.wentToBackground()
        t += 60
        lock.cameToFront()
        XCTAssertFalse(lock.promptDue, "open again inside five minutes")
        lock.wentToBackground()
        t += AppLock.lockAfter + 1
        XCTAssertTrue(lock.backgroundTimer())
        XCTAssertFalse(lock.promptDue, "locked in the background: nothing shows while away")
        lock.cameToFront()
        XCTAssertTrue(lock.takePrompt())
    }

    // decision: PHN-15, PHN-505
    func testTheBackgroundTimerLocksOnlyWhenTheTimeHasPassedAndTheAppIsStillAway() {
        var t = 0.0
        let lock = AppLock { t }
        lock.unlocked()
        XCTAssertFalse(lock.backgroundTimer(), "in front: the timer does nothing")
        lock.wentToBackground()
        t += 60
        XCTAssertFalse(lock.backgroundTimer(), "a minute away: not yet")
        t += AppLock.lockAfter
        XCTAssertTrue(lock.backgroundTimer(), "five minutes away: locks now")
        XCTAssertTrue(lock.locked)
        XCTAssertFalse(lock.backgroundTimer(), "already locked: once")
    }

    // decision: PHN-15, PHN-505
    @MainActor
    func testTheSessionLocksFromItsBackgroundTimerAndOnReturn() {
        let clock = TestClock()
        let s = Session(now: { clock.t }, sleep: { _ in try? await Task.sleep(nanoseconds: 3_600_000_000_000) })
        XCTAssertTrue(s.locked)
        s.unlocked()
        s.foreground(true)
        s.foreground(false)
        clock.t += AppLock.lockAfter + 1
        s.backgroundTimerFired()
        XCTAssertTrue(s.locked, "five minutes in the background: locked there")
        s.unlocked()
        s.foreground(true)
        s.foreground(false)
        clock.t += AppLock.lockAfter + 1 // iOS suspended the app: no timer ran
        s.foreground(true)
        XCTAssertTrue(s.locked, "back after more than five minutes: locked")
    }
}

final class QuestionTests: XCTestCase {
    private func q(_ options: [String], labels: [String] = [], kind: String = "owner_question") -> Question {
        Question(approvalId: "ap-1", taskId: "t-1", title: nil, reason: "Go ahead?", exact: nil, kind: kind, capability: "ferb.confirm",
                 options: options, labels: labels, hidden: Question.secretKinds.contains(kind))
    }

    func testYesNoIsAPlainNoAndYesAsOptionOneLikeTheMac() {
        let c = q(["Yes", "No"], labels: ["Change it once", "Don't"]).choices
        XCTAssertEqual(c.map(\.label), ["Don't", "Change it once"])
        XCTAssertEqual(c[0].input, .approve(granted: false, choice: nil))
        XCTAssertEqual(c[1].input, .approve(granted: true, choice: 1))
    }

    func testOtherOptionsAreNumberedFromOneAndNoOptionsTakeWords() {
        XCTAssertEqual(q(["Use Codex", "Use Claude", "Stop"]).choices.map(\.input),
                       [.approve(granted: true, choice: 1), .approve(granted: true, choice: 2), .approve(granted: true, choice: 3)])
        XCTAssertTrue(q([]).choices.isEmpty)
        XCTAssertTrue(q(["Yes", "No"], kind: "owner_login_code").choices.isEmpty, "a private answer has no buttons")
    }

    /// An option the Mac adds after Yes and No (its own number) is offered under them; No stays a plain No.
    // decision: PHN-607
    func testAnExtraOptionAfterYesAndNoKeepsTheSheetAndItsOwnNumber() {
        let question = q(["Yes", "No", "Until I'm back"], labels: ["Allow", "Don't", "Until I'm back at my Mac"])
        XCTAssertTrue(question.yesNo)
        XCTAssertEqual(question.choices.map(\.label), ["Don't", "Allow", "Until I'm back at my Mac"])
        XCTAssertEqual(question.choices.map(\.input), [.approve(granted: false, choice: nil), .approve(granted: true, choice: 1),
                                                       .approve(granted: true, choice: 3)])
        XCTAssertFalse(q(["Use Codex", "Use Claude", "Stop"]).yesNo)
    }
}

@MainActor
final class AnswerFlowTests: XCTestCase {
    private let phone = FakePhone()
    private var keys = KeyHealth(device: .ok, lock: .ok, strong: .ok)
    private var signers: [FakeSigner] = []

    private func flow(cancel: Bool = false) -> AnswerFlow {
        AnswerFlow(api: phone, keys: { [unowned self] in keys }, signer: { [unowned self] k in
            let s = FakeSigner(k, cancel: cancel)
            signers.append(s)
            return s
        })
    }

    // decision: PHN-417, PHN-470, PHN-503
    func testARoutineAnswerGoesWithNoPrompt() async {
        let r = await flow().answer("ap-1", .approve(granted: true, choice: 1))
        XCTAssertEqual(r, .sent)
        XCTAssertEqual(phone.calls, ["answer:ap-1:false"])
        XCTAssertTrue(signers.isEmpty)
    }

    // decision: PHN-137, PHN-129, PHN-13, PHN-503
    func testAYesTheMacWantsProvedIsSignedOnceByTheFaceIdKey() async {
        phone.answerScript = [.failed(FakePhone.refused("unlock_required"))]
        let r = await flow().answer("ap-1", .approve(granted: true, choice: 1))
        XCTAssertEqual(r, .sent)
        XCTAssertEqual(phone.calls, ["answer:ap-1:false", "answer:ap-1:true"])
        XCTAssertEqual(signers.map(\.kind), [.strong])
    }

    // decision: PHN-129
    func testAPhoneWithoutFaceIdSignsWithItsLockKey() async {
        keys = KeyHealth(device: .ok, lock: .ok, strong: .none)
        phone.answerScript = [.failed(FakePhone.refused("unlock_required"))]
        let r = await flow().answer("ap-1", .approve(granted: true, choice: 1))
        XCTAssertEqual(r, .sent)
        XCTAssertEqual(signers.map(\.kind), [.lock])
    }

    // decision: PHN-133, PHN-503
    func testChangedFacesOrFingersMeanPairAgainNotAPasscode() async {
        keys = KeyHealth(device: .ok, lock: .ok, strong: .gone)
        phone.answerScript = [.failed(FakePhone.refused("unlock_required"))]
        let r = await flow().answer("ap-1", .approve(granted: true, choice: 1))
        XCTAssertEqual(r, .needsPairingAgain)
        XCTAssertEqual(phone.calls.count, 1)
    }

    // decision: PHN-125, PHN-13, PHN-503
    func testAClosedMacWindowUnlocksAndAsksOnceMore() async {
        phone.answerScript = [.failed(FakePhone.refused("locked"))]
        let r = await flow().answer("ap-1", .words(text: "the blue one"))
        XCTAssertEqual(r, .sent)
        XCTAssertEqual(phone.calls, ["answer:ap-1:false", "unlock", "answer:ap-1:false"])
        // and never more than once
        phone.calls = []
        phone.answerScript = [.failed(FakePhone.refused("locked")), .failed(FakePhone.refused("locked"))]
        if case .failed = await flow().answer("ap-2", .words(text: "x")) {} else { XCTFail("refused twice: failed") }
        XCTAssertEqual(phone.calls.count, 3)
    }

    // decision: PHN-110, PHN-109
    func testAPrivateAnswerAlwaysGoesSignedAndACancelledPromptSendsNothing() async {
        let secret = SecretInput()
        for u in "123456".utf16 { secret.push(unit: u) }
        let r = await flow().answerHidden("ap-1", secret: secret)
        XCTAssertEqual(r, .sent)
        XCTAssertEqual(phone.calls, ["hidden:ap-1:6"])
        XCTAssertEqual(signers.map(\.kind), [.lock])
        XCTAssertTrue(secret.isEmpty(), "emptied whatever happens")
        let again = SecretInput()
        again.push(unit: 0x31)
        let cancelled = await flow(cancel: true).answerHidden("ap-1", secret: again)
        XCTAssertEqual(cancelled, .notSent)
    }

    // decision: PHN-302
    func testARefusalComesBackWithTheMacsWords() async {
        phone.answerScript = [.failed(.Refused(detail: "That question isn't waiting any more, so nothing was sent.", linkCode: "question_not_waiting"))]
        let r = await flow().answer("ap-1", .approve(granted: true, choice: 1))
        guard case .failed(let e) = r else { return XCTFail("\(r)") }
        XCTAssertEqual(e.linkCode, "question_not_waiting")
    }
}

@MainActor
final class WordsTests: XCTestCase {
    private var rig: Rig!
    override func setUp() async throws { rig = Rig() }
    override func tearDown() async throws { rig.tearDown() }

    private func sky(_ hour: Double) -> SkyMoment { SkyMoment.at(.delhi, year: 2026, month: 11, day: 14, hour: hour) }

    // decision: PHN-507
    func testTheFirstRunAndTheFirstOpenAfterPairingHaveTheirOwnWords() async {
        rig.phone.link = FakePhone.status(.idle, paired: false)
        let first = try! XCTUnwrap(rig.model(corpus: .bundled).homeWords(sky(10)))
        XCTAssertTrue(["Hello", "Hi, I'm FERB"].contains(first.greeting))
        XCTAssertEqual(first.subline, "Pair with your Mac and ask for anything. It happens there.")
        let other = Rig()
        defer { other.tearDown() }
        let m = other.model(corpus: .bundled) // paired at its start: the first open after pairing
        XCTAssertTrue(other.prefs.justPaired)
        XCTAssertTrue(["All set", "You're connected"].contains(m.homeWords(sky(10))!.greeting))
    }

    // decision: PHN-507
    func testAMacThatCantBeReachedIsSaidAndTheSameLineHoldsAllDay() async {
        rig.prefs.pairedSince = 1
        rig.phone.link = FakePhone.status(.waiting, paired: true, path: .offline)
        let m = rig.model(corpus: .bundled)
        let a = m.homeWords(sky(9))!
        XCTAssertTrue(a.subline.contains("Mac"))
        XCTAssertEqual(a.subline, m.homeWords(sky(16))!.subline)
    }

    // decision: PHN-507
    func testConditionsThePhoneDoesNotMeasureNeverHold() {
        // weather is paused (PHN-412) and the owner's habits are not kept: no rule on them can speak
        XCTAssertFalse(Corpus.holds("weather", "storm", nil))
        XCTAssertTrue(Corpus.holds("hour", "22-4", 23.5))
        XCTAssertTrue(Corpus.holds("hour", "22-4", 3.0))
        XCTAssertFalse(Corpus.holds("hour", "22-4", 4.0))
        XCTAssertTrue(Corpus.holds("awayHours", ">=168", 200.0))
        XCTAssertTrue(Corpus.holds("phase", ["dawn", "sunrise"], "dawn"))
        XCTAssertNil(rig.model(corpus: nil).homeWords(sky(10)), "no corpus: no words")
    }
}

final class MacSettingsTests: XCTestCase {
    /// Jev is gone for good: an older Mac that still sends its day count gets no row, and no word of the app names it.
    // decision: PHN-596
    func testJevIsNeverShownEvenWhenAnOlderMacSendsIt() throws {
        let rows = MacSettings.rows([SettingValue(key: "retention.trace_days", valueJson: "14"),
                                     SettingValue(key: "retention.decision_days", valueJson: "30")])
        XCTAssertEqual(rows.map(\.spec.key), ["retention.trace_days"])
        XCTAssertNil(MacSettings.spec("retention.decision_days"))
        let catalog = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Sources/FerbAppModel/Resources/Localizable.xcstrings")
        let words = try String(contentsOf: catalog, encoding: .utf8)
        XCTAssertNil(words.range(of: "jev", options: .caseInsensitive), "the catalog names Jev")
    }
}
