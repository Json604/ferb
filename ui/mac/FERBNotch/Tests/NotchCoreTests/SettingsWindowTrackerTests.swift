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
            screens: { [CGRect(x: 0, y: 0, width: 1440, height: 900)] })
        var seen: [CGPoint?] = []
        tracker.onChange = { seen.append($0) }
        return (tracker, { seen })
    }

    private func raw(x: CGFloat) -> RawWindow {
        RawWindow(pid: 1, bounds: CGRect(x: x, y: 100, width: 600, height: 400), layer: 0)
    }

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

    func testHidesWhenNotFrontmostOrClosedAndStopIsSafeTwice() {
        let (t, seen) = makeTracker()
        windows = [raw(x: 400)]; t.poll(); t.poll()
        front = "com.example.other"; t.poll()
        XCTAssertEqual(seen().count, 2)
        XCTAssertNil(seen().last!)
        front = SettingsAnchor.bundleID; windows = []; t.poll()
        XCTAssertEqual(seen().count, 2)
        t.start(); t.stop(); t.stop()
    }
}
