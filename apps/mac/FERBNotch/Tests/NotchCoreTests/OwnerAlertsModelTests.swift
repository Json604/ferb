import XCTest
@testable import FERBNotch
@testable import NotchCore

/// The daemon's alerts in FERB.app (APP-1107): above the composer, never in the chat, acknowledged only when put away.
@MainActor
final class OwnerAlertsModelTests: XCTestCase {
    // decision: APP-1105
    // decision: APP-1107
    func testADaemonAlertShowsAboveTheComposerAndIsAcknowledgedOnlyWhenPutAway() async throws {
        let rpc = ScriptedRPC()
        let model = NotchModel(rpc: rpc, preferences: scratchDefaults("ferb-owner-alerts"))
        let hot: [String: Any] = ["id": "alert:insomnia-warning-w1", "text": "Your Mac is getting hot.", "at": "2026-11-21T10:21:29+05:30",
                                  "quotes": [], "alert": true]
        model.receiveBusEvent(["topic": "attention.notice", "data": hot])
        // an older daemon's alert, named only by its id, the same way
        model.receiveNotices([["id": "set-aside-ferb.db", "text": "FERB couldn't read its database.", "quotes": []]])
        // an ordinary notice still goes into the chat
        model.receiveNotices([["id": "memory-always-on-1", "text": "Now always on: tea", "quotes": []]])
        XCTAssertEqual(model.alerts.map(\.id), ["alert:insomnia-warning-w1", "set-aside-ferb.db"])
        XCTAssertEqual(model.conversation.notices.map(\.id), ["memory-always-on-1"])
        try await Task.sleep(for: .milliseconds(50))
        let acked = rpc.calls.filter { $0.0 == "attention.ack" }.flatMap { $0.1["ids"] as? [String] ?? [] }
        XCTAssertEqual(acked, ["memory-always-on-1"])                    // not the alerts: they wait for the owner
        // put away here: gone, and acknowledged so every device drops it
        model.dismissAlert("alert:insomnia-warning-w1")
        try await Task.sleep(for: .milliseconds(50))
        XCTAssertEqual(model.alerts.map(\.id), ["set-aside-ferb.db"])
        XCTAssertTrue(rpc.calls.contains { $0.0 == "attention.ack" && ($0.1["ids"] as? [String]) == ["alert:insomnia-warning-w1"] })
        // put away on the phone: the Mac hears it and drops it too
        model.receiveBusEvent(["topic": "attention.acked", "data": ["ids": ["set-aside-ferb.db"]]])
        XCTAssertTrue(model.alerts.isEmpty)
        // the status lists one not put away anywhere: it shows; a status without it settles it away
        let slept: [String: Any] = ["id": "alert:insomnia-slept-x", "text": "Your Mac slept.", "alert": true]
        model.receiveNotices([slept], complete: true)
        XCTAssertEqual(model.alerts.map(\.id), ["alert:insomnia-slept-x"])
        model.receiveNotices([], complete: true)
        XCTAssertTrue(model.alerts.isEmpty)
    }

    // decision: APP-1108
    // decision: APP-1109
    func testAToastLeavesByItselfUnlessItNeedsTheOwnerAndAPutAwayOneNeverComesBack() async throws {
        let rpc = ScriptedRPC()
        let model = NotchModel(rpc: rpc, preferences: scratchDefaults("ferb-owner-toasts"))
        let hot: [String: Any] = ["id": "alert:insomnia-warning-w1", "text": "Your Mac is getting hot.", "alert": true]
        model.receiveNotices([hot])
        model.showOwnAlert(id: "phone-pair-r1", text: "Pixel 8 wants to pair.",
                           card: ResultCard(["type": "page", "page": "phone", "line": PhoneWords.openPhoneSettings]))
        model.showOwnAlert(id: "update-ask-0.29.0", text: "FERB 0.29.0 is ready.")
        model.showOwnAlert(id: "update-0.28.1", text: "Updated to 0.28.1")
        // its six seconds in front of him: seen, so it leaves (a daemon's on every device); one that needs him stays
        model.alertToastLapsed("alert:insomnia-warning-w1")
        model.alertToastLapsed("update-0.28.1")
        model.alertToastLapsed("phone-pair-r1")
        model.alertToastLapsed("update-ask-0.29.0")
        XCTAssertEqual(model.alerts.map(\.id), ["phone-pair-r1", "update-ask-0.29.0"])
        try await Task.sleep(for: .milliseconds(50))
        XCTAssertTrue(rpc.calls.contains { $0.0 == "attention.ack" && ($0.1["ids"] as? [String]) == ["alert:insomnia-warning-w1"] })
        // a status read before the daemon took the ack (it still lists the alert): it doesn't come back
        model.receiveNotices([hot], complete: true)
        model.receiveBusEvent(["topic": "attention.notice", "data": hot])
        XCTAssertFalse(model.alerts.contains { $0.id == "alert:insomnia-warning-w1" })
        // the stack closes once one alert is left
        model.alertsExpanded = true
        model.dismissAlert("phone-pair-r1")
        XCTAssertFalse(model.alertsExpanded)
        XCTAssertEqual(OwnerAlerts.toast(model.alerts)?.id, "update-ask-0.29.0")
    }
}
