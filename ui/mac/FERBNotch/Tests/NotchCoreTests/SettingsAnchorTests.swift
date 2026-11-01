import XCTest
import CoreGraphics
@testable import NotchCore

final class SettingsAnchorTests: XCTestCase {
    private let screen = CGRect(x: 0, y: 0, width: 1440, height: 900)
    private let orb = CGSize(width: 64, height: 64)

    private func settings(_ bounds: CGRect, onScreen: Bool = true, front: Bool = true) -> WindowInfo {
        WindowInfo(owner: SettingsAnchor.bundleID, bounds: bounds, onScreen: onScreen, isFrontmostApp: front)
    }

    func testDocksToTheLeftOfSettingsWhenThereIsRoom() {
        let w = settings(CGRect(x: 400, y: 200, width: 700, height: 500))
        let p = SettingsAnchor.dock(for: [w], orb: orb, screen: screen)
        XCTAssertEqual(p, CGPoint(x: 384, y: 450))
    }

    func testDocksToTheRightWhenSettingsHugsTheLeftEdge() {
        let w = settings(CGRect(x: 20, y: 200, width: 700, height: 500))
        let p = SettingsAnchor.dock(for: [w], orb: orb, screen: screen)
        XCTAssertEqual(p, CGPoint(x: 736, y: 450))
    }

    func testClampsVerticallyAndFailsWhenNeitherSideFits() {
        let low = settings(CGRect(x: 400, y: 0, width: 700, height: 20))
        XCTAssertEqual(SettingsAnchor.dock(for: [low], orb: orb, screen: screen)?.y, 32)
        let wide = settings(CGRect(x: 10, y: 200, width: 1420, height: 500))
        XCTAssertNil(SettingsAnchor.dock(for: [wide], orb: orb, screen: screen))
    }

    func testFollowsTheLargestWindowAndIgnoresOtherApps() {
        let sheet = settings(CGRect(x: 600, y: 300, width: 200, height: 100))
        let main = settings(CGRect(x: 400, y: 200, width: 700, height: 500))
        let other = WindowInfo(owner: "com.example.other", bounds: CGRect(x: 0, y: 0, width: 1400, height: 800),
                               onScreen: true, isFrontmostApp: true)
        XCTAssertEqual(SettingsAnchor.dock(for: [sheet, other, main], orb: orb, screen: screen)?.x, 384)
    }

    func testHidesWhenSettingsIsNotFrontmost() {
        let w = settings(CGRect(x: 400, y: 200, width: 700, height: 500), front: false)
        XCTAssertNil(SettingsAnchor.dock(for: [w], orb: orb, screen: screen))
    }

    func testHidesWhenSettingsClosedOrMinimised() {
        XCTAssertNil(SettingsAnchor.dock(for: [], orb: orb, screen: screen))
        let w = settings(CGRect(x: 400, y: 200, width: 700, height: 500), onScreen: false)
        XCTAssertNil(SettingsAnchor.dock(for: [w], orb: orb, screen: screen))
    }

    func testSettledNeedsTwoMatchingFrames() {
        let a = CGRect(x: 100, y: 100, width: 600, height: 400)
        XCTAssertFalse(SettingsAnchor.settled(nil, a))
        XCTAssertFalse(SettingsAnchor.settled(a, nil))
        XCTAssertTrue(SettingsAnchor.settled(a, a.offsetBy(dx: 1, dy: -1)))
        XCTAssertFalse(SettingsAnchor.settled(a, a.offsetBy(dx: 2, dy: 0)))
        XCTAssertFalse(SettingsAnchor.settled(a, CGRect(x: 100, y: 100, width: 640, height: 400)))
    }

    func testQuartzToAppKitFlipIncludingOtherDisplays() {
        // Primary is 900 tall. A window at the top of the primary display.
        XCTAssertEqual(SettingsAnchor.appKitRect(fromQuartz: CGRect(x: 50, y: 0, width: 300, height: 200),
                                                 primaryHeight: 900),
                       CGRect(x: 50, y: 700, width: 300, height: 200))
        // A display above the primary has negative Quartz y and lands past the primary's top in AppKit.
        XCTAssertEqual(SettingsAnchor.appKitRect(fromQuartz: CGRect(x: 100, y: -500, width: 300, height: 200),
                                                 primaryHeight: 900),
                       CGRect(x: 100, y: 1200, width: 300, height: 200))
        // A display to the left has negative x, which is unchanged.
        XCTAssertEqual(SettingsAnchor.appKitRect(fromQuartz: CGRect(x: -1000, y: 100, width: 300, height: 200),
                                                 primaryHeight: 900),
                       CGRect(x: -1000, y: 600, width: 300, height: 200))
    }
}
