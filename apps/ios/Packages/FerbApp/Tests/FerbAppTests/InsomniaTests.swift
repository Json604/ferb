import FerbAppModel
import FerbCore
import FerbMobile
import XCTest

/// Insomnia mode on the iPhone (APP-980..APP-984; Android's InsomniaTest): the Mac tab's switch opens the warning and
/// Keep it working is signed with Face ID / Touch ID; End, Lock my Mac and It's fine ask nothing; the notice and the
/// warnings as notifications, each posted once and removed when the Mac's status no longer has them, the notice's three
/// buttons answered from the notification; how it ended said once; the camera view pulls only while it shows, kept out
/// of captures, and says the library's words on the shared relay.
@MainActor
final class InsomniaTests: XCTestCase {
    private var rig: Rig!
    private var phone: FakePhone { rig.phone }

    override func setUp() async throws {
        rig = Rig()
        rig.session.unlocked()
        phone.view = emptyView()
        phone.status = status(phone.insomnia)
    }
    override func tearDown() async throws { rig.tearDown() }

    private func status(_ i: Insomnia?) -> PhoneStatus {
        PhoneStatus(locked: false, words: true, stopped: false, questions: [], running: [], results: [], screenUse: nil, insomnia: i)
    }

    private func on(_ notice: InsomniaNotice? = nil, warnings: [InsomniaWarning] = []) -> Insomnia {
        var i = phone.insomnia
        i.on = true
        i.variant = "shield"
        i.line = "Keeping your Mac working · 2 tasks running · battery 64%"
        i.notice = notice
        i.warnings = warnings
        return i
    }

    private let touched = InsomniaNotice(id: "n-1", kind: "touched", at: nil, text: "Someone is using your Mac's keyboard or trackpad.",
                                         app: nil, count: 1)
    private let battery = InsomniaWarning(id: "w-1", kind: "battery_low", text: "Your Mac's battery is at 18%.", at: nil)

    // ---- the switch, the warning and the signed start --------------------------------------------------------------

    // decision: APP-980, APP-984, PHN-112, PHN-133, PHN-774
    func testTheSwitchOpensTheWarningAndKeepItWorkingIsSignedWithFaceID() async {
        let vm = rig.model()
        await vm.refresh()?.value
        XCTAssertNotNil(vm.ui.status?.insomnia, "the Mac says it can: the row shows")
        vm.askInsomnia()
        XCTAssertEqual(vm.sheet, .insomnia)
        XCTAssertFalse(phone.calls.contains { $0.hasPrefix("insomnia-start") }, "nothing is turned on before Keep it working")
        rig.prompts.cancel = true
        await vm.startInsomnia(variant: "shield", screenYes: true).value
        XCTAssertTrue(phone.signed.isEmpty, "cancelled: nothing signed")
        XCTAssertEqual(vm.sheet, .insomnia)
        rig.prompts.cancel = false
        await vm.startInsomnia(variant: "shield", screenYes: true).value
        XCTAssertTrue(phone.calls.contains("insomnia-start:shield:true"))
        XCTAssertEqual(rig.prompts.signers.last?.kind, .strong, "the Face ID / Touch ID key, for this change")
        XCTAssertEqual(rig.prompts.whys.last, .insomnia)
        XCTAssertNil(vm.sheet, "the sheet goes once the Mac took it")
        XCTAssertEqual(vm.ui.status?.insomnia?.on, true)
        await vm.startInsomnia(variant: "locked", screenYes: false).value
        XCTAssertEqual(vm.ui.note, Note("insomnia_already"))
    }

    // decision: APP-980, APP-983
    func testEndLockMyMacAndItsFineAskNothing() async {
        phone.insomnia = on(touched)
        phone.status = status(phone.insomnia)
        let vm = rig.model()
        await vm.refresh()?.value
        await vm.insomniaFine("n-1").value
        XCTAssertTrue(phone.calls.contains("insomnia-fine:n-1"))
        XCTAssertNil(vm.ui.status?.insomnia?.notice)
        await vm.lockMac().value
        XCTAssertTrue(phone.calls.contains("lock-mac"))
        XCTAssertEqual(vm.ui.note, Note("err_mac_said", "Your Mac is locked."))
        await vm.endInsomnia().value
        XCTAssertEqual(vm.ui.status?.insomnia?.on, false)
        XCTAssertTrue(rig.prompts.signers.isEmpty, "no prompt for any of them")
    }

    /// How it ended is the Mac's alert on every device (kb APP-1108, APP-1109): the app adds no note of its own.
    // decision: APP-980, APP-1109
    func testHowItEndedIsNotSaidAgainByTheApp() async {
        rig.session.foreground(true)
        let vm = rig.model()
        var ended = phone.insomnia
        ended.lastEnd = InsomniaLastEnd(id: "e-1", why: "back", text: "Welcome back. FERB finished 2 tasks.", at: nil)
        phone.status = status(ended)
        await vm.refresh()?.value
        XCTAssertNil(vm.ui.note, "the Mac's alert says it; a note would say it twice")
    }

    // ---- the notifications -------------------------------------------------------------------------------------------

    private func posted(_ a: [NotificationAction]) -> [PlannedNotification] {
        a.compactMap { if case .post(let p) = $0 { return p } else { return nil } }
    }
    private func removed(_ a: [NotificationAction]) -> [String] {
        a.compactMap { if case .remove(let id) = $0 { return id } else { return nil } }
    }

    // decision: APP-983, APP-981, PHN-776
    func testTheNoticeIsPostedOnceWithItsThreeButtonsAndGoesWhenAnswered() {
        let planner = NotificationPlanner(prefs: rig.prefs)
        func look(_ i: Insomnia?, front: Bool = false) -> [NotificationAction] {
            planner.look(status: status(i), view: nil, front: front, unlocked: true, lockedNow: false)
        }
        let first = posted(look(on(touched)))
        XCTAssertEqual(first.count, 1)
        let n = first[0]
        XCTAssertEqual(n.id, "in:n-1")
        XCTAssertEqual(n.kind, .mac)
        XCTAssertEqual(n.title, L.s("n_insomnia_touched"))
        XCTAssertEqual(n.body, touched.text)
        XCTAssertFalse(n.quietly, "a new notice alerts")
        XCTAssertEqual(n.buttons.map(\.label), ["Show me", "Lock my Mac", "It's fine"])
        XCTAssertEqual(n.buttons.map(\.choice), [NotificationButton.showMe, NotificationButton.lockMac, NotificationButton.fine])
        XCTAssertTrue(look(on(touched)).isEmpty, "posted once")
        var again = touched
        again.count = 2
        XCTAssertEqual(posted(look(on(again))).map(\.quietly), [true], "a later touch updates it quietly")
        XCTAssertEqual(removed(look(on())), ["in:n-1"], "answered: gone")
        // posted with the app on screen too: someone at his Mac is news wherever he is
        let app = InsomniaNotice(id: "n-2", kind: "activation", at: nil, text: "Terminal opened on your Mac.", app: "Terminal", count: 1)
        XCTAssertEqual(posted(look(on(app), front: true)).map(\.title), [L.s("n_insomnia_activation")])
    }

    // decision: APP-981
    func testEachWarningIsPostedOnceWithoutButtonsAndGoesWhenGoneOrTheModeEnds() {
        let planner = NotificationPlanner(prefs: rig.prefs)
        func look(_ i: Insomnia?) -> [NotificationAction] {
            planner.look(status: status(i), view: nil, front: false, unlocked: true, lockedNow: false)
        }
        let hot = InsomniaWarning(id: "w-2", kind: "hot", text: "Your Mac is hot.", at: nil)
        let first = posted(look(on(warnings: [battery, hot])))
        XCTAssertEqual(Set(first.map(\.body)), [battery.text, hot.text])
        XCTAssertTrue(first.allSatisfy { $0.buttons.isEmpty && $0.title == L.s("n_insomnia_warning") && $0.tap == .app })
        XCTAssertTrue(look(on(warnings: [battery, hot])).isEmpty, "posted once")
        XCTAssertEqual(removed(look(on(warnings: [battery]))), ["iw:w-2"])
        var ended = phone.insomnia
        ended.lastEnd = InsomniaLastEnd(id: "e-9", why: "back", text: "Welcome back.", at: nil)
        let end = look(ended)
        XCTAssertEqual(removed(end), ["iw:w-1"], "the mode ended: what it posted goes")
        XCTAssertEqual(posted(end).map(\.body), ["Welcome back."])
        XCTAssertEqual(posted(end).map(\.quietly), [true], "said quietly")
        XCTAssertTrue(look(ended).isEmpty, "said once")
        // turned off by the owner himself: nothing to tell him (kb APP-1108)
        var off = phone.insomnia
        off.lastEnd = InsomniaLastEnd(id: "e-10", why: "turned_off", text: "Done. Your Mac can sleep as usual again.", at: nil)
        XCTAssertTrue(posted(look(off)).isEmpty, "his own turning off is not told")
        _ = look(on(touched))
        XCTAssertEqual(removed(look(nil)), ["in:n-1"], "an older Mac leaves nothing behind")
    }

    /// Lock my Mac and It's fine from the notification: sent with no prompt; gone when the Mac took it, else why not.
    // decision: APP-983, PHN-302
    func testLockMyMacAndItsFineAreAnsweredFromTheNotification() async {
        var delivered: [NotificationAction] = []
        let notifier = Notifier(api: phone, prefs: rig.prefs, session: rig.session, lockedNow: { true }, deliver: { delivered += $0 })
        let fine = await notifier.insomniaAnswer("n-1", lock: false)
        XCTAssertEqual(fine, .remove(id: "in:n-1"))
        XCTAssertTrue(phone.calls.contains("insomnia-fine:n-1"))
        phone.insomniaRefusal = .Offline(detail: "Your Mac isn't reachable.", linkCode: nil)
        let lock = await notifier.insomniaAnswer("n-1", lock: true)
        guard case .post(let why) = lock else { return XCTFail("\(lock)") }
        XCTAssertEqual(why.id, "in:n-1")
        XCTAssertEqual(why.body, AppModel.errorNote(.Offline(detail: "Your Mac isn't reachable.", linkCode: nil)).text)
        XCTAssertTrue(phone.calls.contains("lock-mac"))
        XCTAssertEqual(delivered.count, 2)
        XCTAssertTrue(rig.prompts.signers.isEmpty, "no prompt")
    }

    // ---- the camera view ---------------------------------------------------------------------------------------------

    // decision: APP-983, PHN-454, PHN-775
    func testTheCameraPullsOnlyWhileItsViewShowsAndIsKeptOutOfCaptures() async {
        rig.session.foreground(true)
        let vm = rig.model()
        vm.showCamera()
        XCTAssertEqual(vm.sheet, .camera)
        XCTAssertTrue(vm.mustSecure)
        await eventually("asked") { phone.calls.filter { $0 == "camera" }.count == 1 }
        vm.show(nil)
        await eventually("stopped when it leaves") { phone.cameraStopped == 1 }
        XCTAssertFalse(vm.camera.showing)
        XCTAssertFalse(vm.mustSecure)
        vm.showCamera()
        await eventually("asked again") { phone.calls.filter { $0 == "camera" }.count == 2 }
        rig.session.foreground(false)
        await eventually("stopped in the background") { phone.cameraStopped == 2 }
        XCTAssertNil(vm.sheet)
        rig.session.foreground(true)
        vm.showCamera()
        await eventually("asked a third time") { phone.calls.filter { $0 == "camera" }.count == 3 }
        rig.session.lockNow()
        await eventually("stopped when the app locks") { phone.cameraStopped == 3 }
        XCTAssertNil(vm.sheet)
    }

    /// A frame is kept in memory; off drops it; on the shared relay the library's words are shown.
    // decision: APP-983, PHN-444
    func testFramesStayInMemoryAndTheSharedRelaySaysTheLibrarysWords() async {
        let watch = CameraWatch(api: phone)
        await watch.start().value
        XCTAssertEqual(watch.view?.state, "starting")
        phone.camera?.frame(frame: CameraFrame(jpeg: Data([1, 2, 3]), seq: 4, state: "live", text: nil))
        await eventually("the frame") { watch.view?.frame == Data([1, 2, 3]) }
        phone.camera?.frame(frame: CameraFrame(jpeg: nil, seq: 4, state: "off", text: "Your Mac's camera is off."))
        await eventually("off: the last frame goes") { watch.view?.frame == nil && watch.view?.text == "Your Mac's camera is off." }
        let words = "Your Mac's camera is available when connected directly or through your relay."
        phone.camera?.ended(code: "needs_better_path", detail: words)
        await eventually("the library's words") { watch.view?.ended == Note("err_mac_said", words) }
        watch.stop()
        XCTAssertNil(watch.view)
        phone.cameraRefusal = .NeedsBetterPath(detail: words, linkCode: "needs_better_path")
        await watch.start().value
        XCTAssertEqual(watch.view?.ended, AppModel.errorNote(.NeedsBetterPath(detail: words, linkCode: "needs_better_path")))
    }

    // ---- the lid line, the helper, the Mac asleep, the waiting messages (round 2) -----------------------------------

    /// The warning's closed-lid sentence is the Mac's own (true to its helper's state); an older Mac's: the app's words.
    // decision: APP-980, APP-981, PHN-777
    func testTheWarningTakesTheLidLineFromTheMacAndFallsBackForAnOlderOne() {
        let open = L.s("insomnia_body_open"), fallback = L.s("insomnia_body_closed")
        var ready = phone.insomnia
        ready.lidLine = "With the lid closed, FERB keeps your Mac running as if it were open, on battery too."
        ready.closedLid = "ready"
        XCTAssertEqual(MacAsleep.body(open: open, fallback: fallback, ready), "\(open) \(ready.lidLine!)")
        XCTAssertEqual(MacAsleep.body(open: open, fallback: fallback, phone.insomnia), "\(open) \(fallback)", "an older Mac")
        XCTAssertEqual(MacAsleep.body(open: open, fallback: fallback, nil), "\(open) \(fallback)")
        // the helper's problem: the Mac's words, nothing when it is fine or the Mac doesn't say
        var waiting = phone.insomnia
        waiting.helper = InsomniaHelper(state: "requires_approval", error: nil,
                                        problem: "FERB's helper is waiting for your OK in System Settings → General → Login Items.")
        XCTAssertEqual(MacAsleep.helperProblem(waiting), waiting.helper?.problem)
        waiting.helper = InsomniaHelper(state: "enabled", error: nil, problem: nil)
        XCTAssertNil(MacAsleep.helperProblem(waiting))
        XCTAssertNil(MacAsleep.helperProblem(phone.insomnia))
    }

    /// The lid-on-battery warning follows the helper: new words under the same id update it quietly, never twice.
    // decision: APP-981, PHN-789
    func testAWarningWithNewWordsUnderTheSameIdIsUpdatedQuietly() {
        let planner = NotificationPlanner(prefs: rig.prefs)
        func look(_ i: Insomnia?) -> [NotificationAction] {
            planner.look(status: status(i), view: nil, front: false, unlocked: true, lockedNow: false)
        }
        let lid = InsomniaWarning(id: "w-lid", kind: "lid_closed_battery", text: "Your Mac's lid is closed on battery: it will sleep.", at: nil)
        XCTAssertEqual(posted(look(on(warnings: [lid]))).map(\.quietly), [false])
        var ready = lid
        ready.text = "Your Mac's lid is closed on battery: FERB keeps it working."
        let update = posted(look(on(warnings: [ready])))
        XCTAssertEqual(update.map(\.body), [ready.text])
        XCTAssertEqual(update.map(\.id), ["iw:w-lid"])
        XCTAssertEqual(update.map(\.quietly), [true])
        XCTAssertTrue(look(on(warnings: [ready])).isEmpty, "the same words: not posted again")
    }

    /// The Mac can't be reached while insomnia mode was on: one quiet notification, gone when the link is back; never
    /// when the mode was off.
    // decision: APP-981, PHN-778, PHN-779
    func testTheMacAsleepIsSaidOnceQuietlyAndGoesWhenTheLinkIsBack() async {
        var delivered: [NotificationAction] = []
        phone.status = status(on())
        let notifier = Notifier(api: phone, prefs: rig.prefs, session: rig.session, lockedNow: { false },
                                deliver: { delivered += $0 }, lostAfter: 0.02)
        notifier.start()
        defer { notifier.stop() }
        func asleepPosts() -> [PlannedNotification] { posted(delivered).filter { $0.id == "mac_asleep" } }
        phone.link = FakePhone.status(.connecting)
        await eventually("said") { asleepPosts().count == 1 }
        XCTAssertEqual(asleepPosts().first?.body, L.s("mac_asleep"))
        XCTAssertEqual(asleepPosts().first?.quietly, true)
        phone.link = FakePhone.status(.waiting)
        await settle()
        XCTAssertEqual(asleepPosts().count, 1, "said once")
        phone.link = FakePhone.status(.online)
        await eventually("gone when the link is back") { removed(delivered).contains("mac_asleep") }
        // the mode off: nothing
        delivered = []
        phone.status = status(phone.insomnia)
        phone.link = FakePhone.status(.connecting)
        await settle()
        XCTAssertTrue(asleepPosts().isEmpty)
    }

    /// A message waiting for the Mac: "Your Mac is asleep; this goes when it wakes." once the link is lost while insomnia
    /// mode was on; else today's "Waiting for your Mac" (and "Waiting for you to unlock").
    // decision: APP-981, PHN-401
    func testAWaitingMessageSaysTheMacIsAsleepOnlyWhileItMayBe() {
        let lost = FakePhone.status(.connecting)
        XCTAssertTrue(MacAsleep.shown(status(on()), lost, lost: true))
        XCTAssertEqual(MacAsleep.waitingLine("link", asleep: true), "outbox_mac_asleep")
        XCTAssertEqual(L.s("outbox_mac_asleep"), "Your Mac is asleep; this goes when it wakes.")
        XCTAssertEqual(MacAsleep.waitingLine("unlock", asleep: true), "outbox_waiting_unlock")
        XCTAssertEqual(MacAsleep.waitingLine("link", asleep: false), "outbox_waiting")
        XCTAssertFalse(MacAsleep.shown(status(on()), lost, lost: false), "just connecting: not yet")
        XCTAssertFalse(MacAsleep.shown(status(phone.insomnia), lost, lost: true), "insomnia mode off")
        XCTAssertFalse(MacAsleep.shown(status(nil), lost, lost: true), "an older Mac")
        XCTAssertFalse(MacAsleep.shown(status(on()), FakePhone.status(.stopped), lost: true), "removed: not asleep")
    }
}
