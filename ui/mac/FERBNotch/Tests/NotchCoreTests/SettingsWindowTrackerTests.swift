import XCTest
import CoreGraphics
@testable import FERBNotch
@testable import NotchCore

@MainActor
final class SettingsWindowTrackerTests: XCTestCase {
    private var windows: [RawWindow] = []
    private var front: String? = SettingsAnchor.bundleID

    private func makeTracker() -> (SettingsWindowTracker, () -> [CGPoint?]) {
        let tracker = SettingsWindowTracker(
            orb: CGSize(width: 64, height: 64),
            rawWindows: { [unowned self] in self.windows },
            bundleID: { $0 == 1 ? SettingsAnchor.bundleID : "com.example.other" },
            frontmostBundleID: { [unowned self] in self.front },
            screens: { [(frame: CGRect(x: 0, y: 0, width: 1440, height: 900),
                         visible: CGRect(x: 0, y: 0, width: 1440, height: 900)),
                        (frame: CGRect(x: 0, y: 900, width: 1440, height: 900),
                         visible: CGRect(x: 0, y: 900, width: 1440, height: 900))] },
            primaryHeight: { 900 })
        var seen: [CGPoint?] = []
        tracker.onChange = { seen.append($0) }
        return (tracker, { seen })
    }

    private func raw(x: CGFloat) -> RawWindow {
        RawWindow(pid: 1, bounds: CGRect(x: x, y: 100, width: 600, height: 400), layer: 0)
    }

    // decision: APP-510
    func testOpeningAnimationIsIgnoredUntilSettledThenMovesAreFollowed() {
        let (t, seen) = makeTracker()
        for x in [300.0, 340, 380] { windows = [raw(x: x)]; t.poll() }
        XCTAssertTrue(seen().isEmpty)
        windows = [raw(x: 400)]; t.poll()
        windows = [raw(x: 400)]; t.poll()
        XCTAssertEqual(seen().count, 1)
        windows = [raw(x: 500)]; t.poll()
        windows = [raw(x: 520)]; t.poll()
        windows = [raw(x: 520)]; t.poll()
        XCTAssertEqual(seen().compactMap { $0?.x }, [384, 484, 504])
    }

    // decision: APP-510
    func testHidesWhenNotFrontmost() {
        let (t, seen) = makeTracker()
        windows = [raw(x: 400)]; t.poll(); t.poll()
        front = "com.example.other"; t.poll()
        XCTAssertEqual(seen().count, 2)
        XCTAssertNil(seen().last!)
    }

    // decision: APP-510
    func testReportsNilWhenTheWindowClosesOrIsMinimised() {
        let (t, seen) = makeTracker()
        windows = [raw(x: 400)]; t.poll(); t.poll()
        XCTAssertEqual(seen().count, 1)
        XCTAssertNotNil(seen().last!)
        windows = []; t.poll()
        XCTAssertEqual(seen().count, 2)
        XCTAssertNil(seen().last!)
    }

    // decision: APP-510
    func testStopForgetsStateSoStartReportsAgainWithoutEmittingNil() {
        let (t, seen) = makeTracker()
        windows = [raw(x: 400)]; t.poll(); t.poll()
        t.stop(); t.stop()
        XCTAssertEqual(seen().count, 1)
        t.start(); t.stop()
        XCTAssertEqual(seen().count, 1)
        t.poll(); XCTAssertEqual(seen().count, 1)
        t.poll(); XCTAssertEqual(seen().count, 2)
        XCTAssertEqual(seen().last!?.x, 384)
    }

    // decision: APP-510
    func testWindowOnADisplayAboveThePrimary() {
        let (t, seen) = makeTracker()
        // Quartz y -600 on a 900-tall primary: AppKit y 1100..1500, midY 1300.
        windows = [RawWindow(pid: 1, bounds: CGRect(x: 400, y: -600, width: 600, height: 400), layer: 0)]
        t.poll(); t.poll()
        XCTAssertEqual(seen().last!, CGPoint(x: 384, y: 1300))
    }
}
