import FerbAppModel
import FerbMobile
import Foundation
import XCTest

/// "Can't reach your Mac" (kb PHN-750, PHN-753; Android's ConnectionTest): the owner saw it in amber at every open while
/// the link was only connecting. With a short count on the fake phone: connecting never says it; past the point it
/// does; coming back online ends it; every open counts again; stopped is at once.
@MainActor
final class ConnectionTests: XCTestCase {
    private var rig: Rig!

    override func setUp() async throws {
        rig = Rig()
        rig.timing.linkLost = 0.3
    }

    override func tearDown() async throws { rig.tearDown() }

    private func open() {
        rig.session.foreground(true)
        rig.session.unlocked()
    }

    // decision: PHN-750, PHN-758
    func testAnOpenThatConnectsNeverSaysItCantReachTheMac() async {
        rig.phone.link = FakePhone.status(.connecting)
        let vm = rig.model()
        open()
        try? await Task.sleep(nanoseconds: 150_000_000)
        rig.phone.link = FakePhone.status(.waiting)
        try? await Task.sleep(nanoseconds: 100_000_000)
        XCTAssertFalse(vm.ui.linkLost, "still connecting")
        rig.phone.link = FakePhone.status()
        await settle()
        try? await Task.sleep(nanoseconds: 400_000_000)
        XCTAssertFalse(vm.ui.linkLost)
    }

    // decision: PHN-758, PHN-757
    func testConnectingThatFailsPastThePointSaysItCantReachAndAReconnectEndsIt() async {
        XCTAssertEqual(UnlockTiming().linkLost, 10, "the owner's point (PHN-757)")
        rig.phone.link = FakePhone.status(.waiting)
        let vm = rig.model()
        open()
        await eventually("past the point") { vm.ui.linkLost }
        rig.phone.link = FakePhone.status(path: .publicRelay)
        await eventually("back online") { !vm.ui.linkLost }
    }

    // decision: PHN-758
    func testEveryOpenCountsAgainAndStoppedIsAtOnce() async {
        rig.phone.link = FakePhone.status(.waiting)
        let vm = rig.model()
        open()
        await eventually("past the point") { vm.ui.linkLost }
        rig.session.foreground(false)
        await settle()
        rig.session.foreground(true)
        await settle()
        XCTAssertFalse(vm.ui.linkLost, "a new open counts again")
        await eventually("past the point again") { vm.ui.linkLost }
        rig.phone.link = FakePhone.status(.stopped)
        await settle()
        XCTAssertTrue(vm.ui.linkLost)
    }
}
