import FerbAppModel
import FerbCore
import FerbMobile
import XCTest

/// "Lock FERB now" in This phone (kb PHN-847, PHN-848; Android's LockNowTest): the owner's tap locks the app at once by
/// the five minutes' own path (the lock page, the Mac told, the fetched files gone), and the app then opens only with
/// the prompt, even with "Face ID to open" off, until that prompt passes. The five minutes' lock is unchanged.
@MainActor
final class LockNowTests: XCTestCase {
    private var rig: Rig!
    private var phone: FakePhone { rig.phone }
    private var session: Session { rig.session }

    override func setUp() async throws { rig = Rig() }
    override func tearDown() async throws { rig.tearDown() }

    private func exists(_ u: URL) -> Bool { FileManager.default.fileExists(atPath: u.path) }

    /// The app leaves for `seconds` and comes back: inactive, background, then active again.
    private func away(_ vm: AppModel, _ s: Session, _ seconds: Double) async {
        vm.setWindowReady(false)
        s.foreground(false)
        rig.clock.t += seconds
        s.foreground(true)
        vm.setWindowReady(true)
        await settle()
    }

    /// The owner's lock switch off, as the Mac took it (PHN-846).
    private func lockOff() {
        rig.prefs.lockOnOpen = false
        phone.lockOff = true
        phone.mac = MacInfo(name: "Studio Mac", pairedAt: nil, phoneName: nil, uploadFolder: nil, lockOff: true)
    }

    private func locks() -> [String] { phone.calls.filter { $0.hasPrefix("lock") && !$0.hasPrefix("lockoff") } }

    // decision: PHN-847, PHN-848, PHN-15
    func testTheButtonLocksAtOnceTellsTheMacByHandAndTheFetchedFilesGo() async {
        let teller = LockTeller(session: session, api: phone, local: rig.local)
        let vm = rig.model()
        session.foreground(true)
        vm.setWindowReady(true)
        await eventually("open") { !session.locked && vm.ui.gate == .main }
        let file = rig.local.fetched.appendingPathComponent("k/a.txt")
        try? FileManager.default.createDirectory(at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
        try? Data("x".utf8).write(to: file)
        vm.show(.profile)
        phone.calls.removeAll()
        let prompts = rig.prompts.shown
        vm.lockNow()
        // the same moment: the sheet gone, the lock page
        XCTAssertNil(vm.sheet)
        XCTAssertTrue(session.locked)
        XCTAssertEqual(vm.ui.gate, .locked)
        await eventually("the wipe") { !exists(rig.local.fetched) }
        await eventually("told") { locks() == ["lock:hand"] }
        XCTAssertEqual(rig.prompts.shown, prompts, "no prompt by itself: the lock page waits for his tap (PHN-616)")
        // a second tap on a locked app does nothing more
        vm.lockNow()
        await settle()
        XCTAssertEqual(locks(), ["lock:hand"])
        // his unlock opens it; the five minutes' lock after it is the plain one again
        await vm.unlockApp().value
        XCTAssertFalse(session.locked)
        await away(vm, session, AppLock.lockAfter + 1)
        await eventually("told plainly") { locks() == ["lock:hand", "lock"] }
        withExtendedLifetime(teller) {}
    }

    // decision: PHN-847, PHN-846
    func testWithTheSwitchOffALockByHandOpensOnlyWithThePromptUntilItPasses() async {
        lockOff()
        let vm = rig.model()
        session.foreground(true)
        vm.setWindowReady(true)
        await eventually("the switch off: open unasked") { !session.locked && vm.ui.gate == .main }
        XCTAssertEqual(rig.prompts.shown, 0)
        vm.lockNow()
        await settle()
        XCTAssertTrue(session.locked)
        XCTAssertEqual(vm.ui.gate, .locked, "the lock page, though the switch is off")
        XCTAssertTrue(rig.prefs.lockedByHand)
        XCTAssertFalse(vm.settings.lockOn, "the switch itself stays as he set it")
        XCTAssertEqual(rig.prompts.shown, 0)
        // away and back: the prompt comes by itself; closed, it stays locked
        rig.prompts.cancel = true
        rig.prompts.localResult = false
        await away(vm, session, 1)
        await eventually("the prompt by itself") { rig.prompts.shown == 1 }
        await settle()
        XCTAssertTrue(session.locked)
        XCTAssertEqual(vm.ui.gate, .locked)
        // a new process (iOS ended the app): still the lock page
        let fresh = Session(now: { [clock = rig.clock] in clock.t }, sleep: { _ in try? await Task.sleep(nanoseconds: 3_600_000_000_000) })
        let vm2 = rig.model(session: fresh)
        XCTAssertEqual(vm2.ui.gate, .locked)
        fresh.foreground(true)
        vm2.setWindowReady(true)
        await settle()
        XCTAssertTrue(fresh.locked)
        XCTAssertEqual(vm2.ui.gate, .locked)
        XCTAssertTrue(rig.prefs.lockedByHand)
        // the prompt passes: open, and the one-shot is spent
        rig.prompts.cancel = false
        rig.prompts.localResult = true
        await vm2.unlockApp().value
        XCTAssertFalse(fresh.locked)
        await eventually("main") { vm2.ui.gate == .main }
        XCTAssertFalse(rig.prefs.lockedByHand)
        // so the five minutes' lock opens unasked again, as the switch says
        let before = rig.prompts.shown
        await away(vm2, fresh, AppLock.lockAfter + 1)
        await eventually("open again unasked") { !fresh.locked && vm2.ui.gate == .main }
        await settle()
        XCTAssertEqual(rig.prompts.shown, before, "no prompt")
        withExtendedLifetime(vm) {}
    }

    // decision: PHN-847
    func testACancelledPromptKeepsTheOneShot() async {
        lockOff()
        let vm = rig.model()
        session.foreground(true)
        vm.setWindowReady(true)
        await eventually("open") { !session.locked }
        vm.lockNow()
        rig.prompts.cancel = true
        rig.prompts.localResult = false
        await vm.unlockApp().value
        XCTAssertTrue(session.locked)
        XCTAssertTrue(rig.prefs.lockedByHand)
        vm.recheck()
        await settle()
        XCTAssertTrue(session.locked, "recheck doesn't open it unasked")
        XCTAssertEqual(vm.ui.gate, .locked)
    }
}
