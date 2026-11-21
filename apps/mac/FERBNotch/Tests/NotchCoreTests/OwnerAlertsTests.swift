import XCTest
@testable import NotchCore

/// FERB's alerts are shown above the composer for a while and never kept in the chat (owner, 2026-11-21).
final class OwnerAlertsTests: XCTestCase {
    // decision: APP-1105
    // decision: APP-1106
    func testAnAlertIsNamedByItsIdOrMarkedByTheDaemon() {
        for id in ["alert:insomnia-warning-w1", "insomnia-ended-x", "insomnia-end-x", "set-aside-ferb.db", "window-shown-t-s",
                   "refused-sp", "phone-unused-p-d", "screen-until-back-S-why", "update-0.28.1", "update-ask-0.29",
                   "update-failed-1", "restart-after-tasks-t1-1", "phone-pair-r1", "screen-back-S-1"] {
            XCTAssertTrue(OwnerAlerts.isAlert(id: id), id)
        }
        for id in ["memory-always-on-1", "loop-s1-abc", "reminder:t1", "learned:2026-11-21", "guesses:g1", "conversation:c1"] {
            XCTAssertFalse(OwnerAlerts.isAlert(id: id), id)
        }
        XCTAssertTrue(OwnerAlerts.isAlert(row: ["id": "x-1", "alert": true]))
        XCTAssertFalse(OwnerAlerts.isAlert(row: ["id": "x-1", "text": "t"]))
    }

    // decision: APP-1107
    func testAlertsAreShownOnceRewordedInPlaceAndLeaveWhenPutAwayElsewhere() {
        var alerts: [OwnerAlert] = []
        XCTAssertTrue(OwnerAlerts.show(OwnerAlert(id: "alert:a", text: "Your Mac is getting hot.", fromDaemon: true), in: &alerts))
        XCTAssertFalse(OwnerAlerts.show(OwnerAlert(id: "alert:a", text: "Your Mac is getting hot.", fromDaemon: true), in: &alerts))
        XCTAssertTrue(OwnerAlerts.show(OwnerAlert(id: "update-0.28.1", text: "Updated to 0.28.1", fromDaemon: false), in: &alerts))
        XCTAssertFalse(OwnerAlerts.show(OwnerAlert(id: "alert:b", text: "", fromDaemon: true), in: &alerts))
        XCTAssertEqual(alerts.map(\.id), ["alert:a", "update-0.28.1"])
        // the daemon's status no longer lists it (a phone put it away): it leaves; FERB.app's own stays
        XCTAssertTrue(OwnerAlerts.settle(daemon: [], in: &alerts))
        XCTAssertEqual(alerts.map(\.id), ["update-0.28.1"])
        XCTAssertFalse(OwnerAlerts.settle(daemon: [], in: &alerts))
    }

    // decision: APP-1105
    // decision: APP-1107
    func testAlertsSavedInTheChatEarlierAreKeptButNotDrawnAndNewOnesAreNotSaved() throws {
        let json = """
        {"turns": [], "replies": [], "notices": [
          {"id": "insomnia-warning-w1", "entryID": "6F9619FF-8B86-D011-B42D-00C04FC964F1", "createdAt": 100,
           "text": "Your Mac is getting hot.", "quotes": []},
          {"id": "memory-always-on-1", "entryID": "6F9619FF-8B86-D011-B42D-00C04FC964F2", "createdAt": 200,
           "text": "Now always on: tea", "quotes": []}
        ]}
        """
        var archive = try JSONDecoder().decode(ConversationArchive.self, from: Data(json.utf8))
        XCTAssertEqual(archive.notices.count, 2)                                   // nothing deleted
        XCTAssertEqual(archive.shownNotices.map(\.id), ["memory-always-on-1"])
        let drawn = archive.transcript().compactMap { entry -> String? in
            if case let .notice(n) = entry { return n.id }
            return nil
        }
        XCTAssertEqual(drawn, ["memory-always-on-1"])
        XCTAssertFalse(archive.addNotice(id: "alert:set-aside-ferb.db", text: "FERB couldn't read its database."))
        XCTAssertFalse(archive.addNotice(id: "update-0.28.1", text: "Updated to 0.28.1", tag: "FERB"))
        XCTAssertTrue(archive.addNotice(id: "notice-1", text: "Something FERB noticed"))
    }
}
