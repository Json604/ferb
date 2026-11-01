import XCTest
@testable import FERBNotch
@testable import NotchCore

/// L34 D: FERB.app as the daemon's poster.
@MainActor
final class NotificationPosterTests: XCTestCase {
    private let question: [String: Any] = [
        "id": "question:a1", "category": "question", "title": "FERB needs you", "body": "FERB needs your OK: Send it?",
        "target": ["kind": "question", "approval_id": "a1", "task_id": "t1"], "when_hidden": true]
    private let reminder: [String: Any] = [
        "id": "reminder:r1", "category": "reminder", "title": "FERB reminder", "body": "call Mom",
        "target": ["kind": "reminder", "timer_id": "r1"], "when_hidden": false]

    // decision: APP-301
    func testAnOpenNotchHoldsBackOnlyWhatItAlreadyShows() async {
        let center = FakeNotificationCenter()
        let rpc = ScriptedRPC()
        rpc.events["notifications.subscribe"] = [question, reminder]
        let poster = NotificationPoster(center: center, settle: .zero)
        poster.configure(notchShowing: { true }, open: { _ in })
        _ = await poster.subscribeOnce(rpc: rpc)
        XCTAssertEqual(center.posted.map(\.id), ["reminder:r1"])
    }

    /// Owner 2026-10-26: one "Allow all" button for the setup rows: the two every task needs, Full Disk Access and
    /// Notifications. Screen Recording goes last: macOS restarts FERB after it, and setup reopens on this page.
    // decision: APP-508
    func testSetupAsksForFourAndAllowAllLeavesScreenRecordingLast() {
        XCTAssertEqual(SetupModel.askedAtSetup, ["accessibility", "screen_recording", "full_disk_access", "input_monitoring",
                                                 "notifications", "microphone", "clipboard"])
        func row(_ id: String, _ status: GrantStatus) -> PermissionRow {
            PermissionRow(id: id, status: status, process: "daemon", required: true, pane: nil, requestable: true)
        }
        let rows = [row("accessibility", .denied), row("screen_recording", .notDetermined),
                    row("full_disk_access", .denied), row("notifications", .notDetermined), row("microphone", .denied)]
        // Notifications second to last (2026-10-27): its banner is never waited on, and Screen Recording stays last
        XCTAssertEqual(SetupModel.allowAllOrder(rows), ["microphone", "accessibility", "full_disk_access", "notifications",
                                                        "screen_recording"])
        XCTAssertEqual(SetupModel.allowAllOrder([row("accessibility", .granted), row("screen_recording", .denied)]),
                       ["screen_recording"])
    }

    // decision: APP-301
    func testAClickOpensTheNotchAtItsItemEvenBeforeTheNotchIsReady() {
        let poster = NotificationPoster(center: FakeNotificationCenter())
        poster.clicked(["kind": "task", "task_id": "t9"])        // a click that launched FERB.app
        var opened: [NotificationTarget] = []
        poster.configure(notchShowing: { false }, open: { opened.append($0) })
        XCTAssertEqual(opened, [.task("t9")])
        poster.clicked(["kind": "question", "approval_id": "a1", "task_id": ""])
        XCTAssertEqual(opened.last, .question(approvalID: "a1", taskID: nil))
    }

    /// APP-301: the notch opens itself for a question or a finished task; the banner, which may arrive first, posts
    /// only if the notch still isn't showing once that has landed, and never for a question answered meanwhile.
    // decision: APP-301
    func testABannerTheNotchShowsItselfWaitsForTheNotchToOpen() async throws {
        let center = FakeNotificationCenter()
        var showing = false
        let poster = NotificationPoster(center: center, settle: .milliseconds(50))
        poster.configure(notchShowing: { showing }, open: { _ in })
        poster.receive(question)
        poster.receive(reminder)
        XCTAssertEqual(center.posted.map(\.id), ["reminder:r1"])     // a reminder always posts at once
        showing = true                                                  // the question opened the notch
        try await Task.sleep(for: .milliseconds(150))
        XCTAssertEqual(center.posted.map(\.id), ["reminder:r1"])

        showing = false                                                 // a full-screen app covers the notch
        var finished = question
        finished["id"] = "task:t1"
        finished["target"] = ["kind": "task", "task_id": "t1"]
        poster.receive(finished)
        poster.receive(question)
        poster.withdrawQuestion("a1")                                   // answered before the banner was judged
        try await Task.sleep(for: .milliseconds(150))
        XCTAssertEqual(center.posted.map(\.id), ["reminder:r1", "task:t1"])
    }
}
