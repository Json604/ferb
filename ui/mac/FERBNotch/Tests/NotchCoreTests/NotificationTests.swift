import XCTest
@testable import NotchCore

/// L34 D: banners as the daemon sends them, and what a click opens.
final class NotificationTests: XCTestCase {
    // decision: APP-301
    func testOnlyBannersTheNotchShowsWaitForItToClose() {
        let question = BannerItem(["id": "q", "title": "t", "body": "b", "when_hidden": true])!
        let reminder = BannerItem(["id": "r", "title": "t", "body": "b", "when_hidden": false])!
        XCTAssertFalse(NotificationGate.shouldPost(question, notchShowing: true))
        XCTAssertTrue(NotificationGate.shouldPost(question, notchShowing: false))
        XCTAssertTrue(NotificationGate.shouldPost(reminder, notchShowing: true))
    }
}
