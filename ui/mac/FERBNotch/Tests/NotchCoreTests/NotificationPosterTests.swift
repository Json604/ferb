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

    func testFERBAppPostsWhatTheDaemonSendsExactlyAsWritten() async {
        let center = FakeNotificationCenter()
        let rpc = ScriptedRPC()
        rpc.events["notifications.subscribe"] = [question, reminder]
        let poster = NotificationPoster(center: center)
        poster.configure(notchShowing: { false }, open: { _ in })
        let subscribed = await poster.subscribeOnce(rpc: rpc)
        XCTAssertTrue(subscribed)
        XCTAssertEqual(rpc.params(of: "notifications.subscribe").first?["authorized"] as? Bool, true)
        XCTAssertEqual(center.posted.map(\.id), ["question:a1", "reminder:r1"])
        XCTAssertEqual(center.posted[0].title, "FERB needs you")
        XCTAssertEqual(center.posted[0].body, "FERB needs your OK: Send it?")
        XCTAssertEqual(center.posted[0].userInfo, ["kind": "question", "approval_id": "a1", "task_id": "t1"])
        XCTAssertEqual(center.posted[1].thread, "reminder")
    }

    func testAnOpenNotchHoldsBackOnlyWhatItAlreadyShows() async {
        let center = FakeNotificationCenter()
        let rpc = ScriptedRPC()
        rpc.events["notifications.subscribe"] = [question, reminder]
        let poster = NotificationPoster(center: center)
        poster.configure(notchShowing: { true }, open: { _ in })
        _ = await poster.subscribeOnce(rpc: rpc)
        XCTAssertEqual(center.posted.map(\.id), ["reminder:r1"])
    }

    func testAnUnauthorizedAppNeverSubscribes() async {
        let center = FakeNotificationCenter()
        center.allowed = false
        let rpc = ScriptedRPC()
        let poster = NotificationPoster(center: center)
        let subscribed = await poster.subscribeOnce(rpc: rpc)
        XCTAssertFalse(subscribed)
        XCTAssertTrue(rpc.calls.isEmpty)
    }

    /// Owner 2026-10-26: "ask when first needed, like Claude and ChatGPT": the first banner brings macOS's prompt.
    func testTheFirstBannerAsksMacOSAndPostsOnAYes() async {
        let center = FakeNotificationCenter()
        center.allowed = false
        center.undecidedState = true
        let rpc = ScriptedRPC()
        rpc.events["notifications.subscribe"] = [question, reminder]
        let poster = NotificationPoster(center: center)
        poster.configure(notchShowing: { false }, open: { _ in })
        let subscribed = await poster.subscribeOnce(rpc: rpc)
        XCTAssertTrue(subscribed)
        XCTAssertEqual(center.requests, 1)
        XCTAssertEqual(center.posted.map(\.id), ["question:a1", "reminder:r1"])
    }

    func testANoToTheFirstBannerPostsNothingAndHandsBackPosting() async {
        let center = FakeNotificationCenter()
        center.allowed = false
        center.undecidedState = true
        center.answer = false
        let rpc = ScriptedRPC()
        rpc.events["notifications.subscribe"] = [question, reminder]
        let poster = NotificationPoster(center: center)
        poster.configure(notchShowing: { false }, open: { _ in })
        _ = await poster.subscribeOnce(rpc: rpc)
        XCTAssertEqual(center.requests, 1)
        XCTAssertTrue(center.posted.isEmpty)
        let again = await poster.subscribeOnce(rpc: rpc)            // denied now: never subscribes again
        XCTAssertFalse(again)
    }

    /// Owner 2026-10-26: one "Allow all" button for the setup rows: the two every task needs, Full Disk Access and
    /// Notifications. Screen Recording goes last: macOS restarts FERB after it, and setup reopens on this page.
    func testSetupAsksForFourAndAllowAllLeavesScreenRecordingLast() {
        XCTAssertEqual(SetupModel.askedAtSetup, ["accessibility", "screen_recording", "full_disk_access", "notifications"])
        func row(_ id: String, _ status: GrantStatus) -> PermissionRow {
            PermissionRow(id: id, status: status, process: "daemon", required: true, pane: nil, requestable: true)
        }
        let rows = [row("accessibility", .denied), row("screen_recording", .notDetermined),
                    row("full_disk_access", .denied), row("notifications", .notDetermined), row("microphone", .denied)]
        XCTAssertEqual(SetupModel.allowAllOrder(rows), ["notifications", "accessibility", "full_disk_access", "screen_recording"])
        XCTAssertEqual(SetupModel.allowAllOrder([row("accessibility", .granted), row("screen_recording", .denied)]),
                       ["screen_recording"])
    }

    func testWithoutANotificationCenterThePosterNeverSubscribes() async {
        let rpc = ScriptedRPC()
        let poster = NotificationPoster(center: nil)
        let subscribed = await poster.subscribeOnce(rpc: rpc)
        XCTAssertFalse(subscribed)
        poster.start(rpc: rpc, notchShowing: { false }, open: { _ in })
        try? await Task.sleep(for: .milliseconds(50))
        XCTAssertTrue(rpc.calls.isEmpty)
        poster.receive(question)                                 // nothing to post with: no crash
        poster.stop()
    }

    func testAClickOpensTheNotchAtItsItemEvenBeforeTheNotchIsReady() {
        let poster = NotificationPoster(center: FakeNotificationCenter())
        poster.clicked(["kind": "task", "task_id": "t9"])        // a click that launched FERB.app
        var opened: [NotificationTarget] = []
        poster.configure(notchShowing: { false }, open: { opened.append($0) })
        XCTAssertEqual(opened, [.task("t9")])
        poster.clicked(["kind": "question", "approval_id": "a1", "task_id": ""])
        XCTAssertEqual(opened.last, .question(approvalID: "a1", taskID: nil))
    }

    func testAnAnsweredQuestionWithdrawsItsBanner() {
        let center = FakeNotificationCenter()
        let poster = NotificationPoster(center: center)
        poster.withdrawQuestion("a1")
        XCTAssertEqual(center.withdrawn, ["question:a1"])
    }
}
