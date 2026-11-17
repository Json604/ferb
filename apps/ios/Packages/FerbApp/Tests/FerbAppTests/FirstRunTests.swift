import XCTest
@testable import FerbAppModel

/// The first run's asks on fakes (kb PHN-635..639; Android's FirstRunTest): all allowed, some declined, already
/// decided by iOS (no page), a relaunch (never again), an update that adds an ask (only it), and the pairing page
/// reachable with everything declined.
final class FirstRunTests: XCTestCase {
    private var decided = Set<FirstRun.Ask>()
    private var asked = Set<String>()
    private func make(_ wanted: [FirstRun.Ask] = FirstRun.Ask.allCases) -> FirstRun {
        FirstRun(wanted: wanted, decided: { [unowned self] in decided.contains($0) }, asked: { [unowned self] in asked },
                 record: { [unowned self] in asked = $0 })
    }

    // decision: PHN-635, PHN-636
    func testAllAllowed() {
        let r = make()
        XCTAssertEqual(r.pending(), [.notifications, .location, .camera, .microphone])
        XCTAssertTrue(r.showsOver(unlockedPage: true))
        XCTAssertFalse(r.showsOver(unlockedPage: false), "never before the unlock")
        while let a = r.pending().first { decided.insert(a); r.answered(a) }
        XCTAssertFalse(r.showsOver(unlockedPage: true))
    }

    // decision: PHN-636, PHN-639
    func testSomeDeclinedAndEverythingDeclinedStillPairs() {
        let r = make()
        r.answered(.notifications)
        decided.insert(.location); r.answered(.location)
        r.answered(.camera)
        r.answered(.microphone)
        XCTAssertTrue(r.pending().isEmpty)
        XCTAssertFalse(make().showsOver(unlockedPage: true), "the pairing page shows: nothing holds it")
        asked = []; decided = []
        make().notNow()
        XCTAssertEqual(asked, Set(FirstRun.Ask.allCases.map(\.rawValue)))
    }

    // decision: PHN-636
    func testAlreadyDecidedShowsNoPage() {
        decided = Set(FirstRun.Ask.allCases)
        XCTAssertTrue(make().pending().isEmpty)
        decided.remove(.camera)
        XCTAssertEqual(make().pending(), [.camera])
    }

    // decision: PHN-636
    func testARelaunchNeverAsksAgain() {
        make().notNow()
        decided = []
        XCTAssertTrue(make().pending().isEmpty)
    }

    // decision: PHN-636
    func testAnUpdateAddingAnAskShowsOnlyIt() {
        asked = ["notifications", "location", "camera"]
        XCTAssertTrue(make([.notifications, .location, .camera]).pending().isEmpty)
        XCTAssertEqual(make().pending(), [.microphone])
    }
}
