import XCTest
@testable import NotchCore

/// L34 D: banners as the daemon sends them, and what a click opens.
final class NotificationTests: XCTestCase {
    func testABannerArrivesAsTheDaemonWroteIt() {
        let item = BannerItem(["id": "question:a1", "category": "question", "title": "FERB needs you",
                               "body": "FERB needs your OK: Send the email to Sam?",
                               "target": ["kind": "question", "approval_id": "a1", "task_id": "t1"],
                               "when_hidden": true])!
        XCTAssertEqual(item.category, .question)
        XCTAssertEqual(item.title, "FERB needs you")
        XCTAssertEqual(item.body, "FERB needs your OK: Send the email to Sam?")
        XCTAssertEqual(item.target, .question(approvalID: "a1", taskID: "t1"))
        XCTAssertTrue(item.whenHidden)
        XCTAssertNil(BannerItem(["category": "question", "title": "x", "body": "y"]))       // no id
        XCTAssertNil(BannerItem(["id": "x", "body": "y"]))                                   // no title
        XCTAssertNil(BannerItem(["id": "n", "category": "", "title": "FERB noticed", "body": "x"])!.category)
    }

    func testOnlyBannersTheNotchShowsWaitForItToClose() {
        let question = BannerItem(["id": "q", "title": "t", "body": "b", "when_hidden": true])!
        let reminder = BannerItem(["id": "r", "title": "t", "body": "b", "when_hidden": false])!
        XCTAssertFalse(NotificationGate.shouldPost(question, notchShowing: true))
        XCTAssertTrue(NotificationGate.shouldPost(question, notchShowing: false))
        XCTAssertTrue(NotificationGate.shouldPost(reminder, notchShowing: true))
    }

    func testAClickedNotificationFindsItsItemAgain() {
        for target in [NotificationTarget.question(approvalID: "a1", taskID: "t1"),
                       .question(approvalID: "a2", taskID: nil), .task("t9"), .reminder("r1"), .handoff("h1"), .notch] {
            let info: [AnyHashable: Any] = target.userInfo.reduce(into: [:]) { $0[$1.key] = $1.value }
            XCTAssertEqual(NotificationTarget(userInfo: info), target)
        }
        XCTAssertEqual(NotificationTarget(["kind": "task"]), .notch)              // no id: just the notch
        XCTAssertEqual(NotificationGate.questionID("a1"), "question:a1")
    }

    func testEveryCategoryHasASettingsTitle() {
        XCTAssertEqual(BannerCategory.allCases.map(\.rawValue), ["question", "reminder", "alert", "task_finished", "loop"])
        for category in BannerCategory.allCases { XCTAssertFalse(category.title.isEmpty) }
    }
}
