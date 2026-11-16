import XCTest
@testable import NotchCore

/// Pair all followed live (HAND-412) and the tour's live waits (APP-665), in NotchCore.
final class PairProgressTests: XCTestCase {
    // decision: HAND-412, HAND-413
    func testEachProfilesStateFollowsTheEventsAsTheyLand() {
        var progress = PairProgress()
        progress.apply(["type": "plan", "profiles": 3,
                        "todo": [["browser": "chrome", "folder": "Default", "name": "Personal", "label": "Google Chrome"],
                                 ["browser": "brave", "folder": "Default", "name": "Work", "label": "Brave"]],
                        "skipped": [["browser": "chrome", "folder": "Profile 1", "name": "Old", "label": "Google Chrome",
                                     "reason": "removed", "step": "add it", "can_add": true]]])
        XCTAssertEqual(progress.pairing.count, 2)
        XCTAssertEqual(progress.tried, 2)
        XCTAssertEqual(progress.landed, 0)
        XCTAssertEqual(progress.rows.last?.canAdd, true)
        progress.apply(["type": "result", "browser": "brave", "folder": "Default", "name": "Work", "paired": false,
                        "why": "turn it on"])
        XCTAssertEqual(progress.landed, 1)
        XCTAssertEqual(progress.rows.first { $0.id == "brave/Default" }?.state, .failed)
        XCTAssertEqual(progress.rows.first { $0.id == "brave/Default" }?.why, "turn it on")
        progress.apply(["type": "result", "browser": "chrome", "folder": "Default", "name": "Personal", "paired": true])
        XCTAssertTrue(progress.pairing.isEmpty)
        XCTAssertFalse(progress.finished)
        progress.apply(["type": "done", "profiles": 3, "paired": 1])
        XCTAssertTrue(progress.finished)
        XCTAssertEqual(progress.reply?["paired"] as? Int, 1)
        XCTAssertFalse(progress.allPaired)
    }

    // decision: APP-665
    func testAWaitShowsItsElapsedTimeAfterAFewSeconds() {
        let start = Date(timeIntervalSince1970: 1000)
        XCTAssertNil(IntroLine.elapsed(since: start, now: start.addingTimeInterval(1)))   // the spinner says enough
        XCTAssertEqual(IntroLine.elapsed(since: start, now: start.addingTimeInterval(7)), "0:07")
        XCTAssertEqual(IntroLine.elapsed(since: start, now: start.addingTimeInterval(62)), "1:02")
        XCTAssertEqual(IntroLine(text: "Waiting").waiting(since: start).since, start)
    }

    /// HAND-564 (live 2026-11-16): a pairing whose result landed failed while its profile then connected (its new key's
    /// hello came 80 ms later) reads paired once FERB reads it connected; a row the owner has a step for keeps it.
    // decision: HAND-564, APP-920
    func testAProfileFERBReadsConnectedAfterItsResultIsPaired() {
        var progress = PairProgress()
        progress.apply(["type": "plan", "profiles": 3,
                        "todo": [["browser": "chrome", "folder": "Default", "name": "Personal"],
                                 ["browser": "chrome", "folder": "Profile 2", "name": "Spare"]],
                        "skipped": [["browser": "brave", "folder": "Default", "name": "Work", "reason": "off",
                                     "step": "turn FERB on in Brave"]]])
        progress.apply(["type": "result", "browser": "chrome", "folder": "Default", "paired": false, "reason": "problem",
                        "why": "extension pairing failed (bad_mac); pair this profile again"])
        progress.apply(["type": "result", "browser": "chrome", "folder": "Profile 2", "paired": false,
                        "reason": "no_answer", "why": "the page got no answer"])
        XCTAssertEqual(progress.rows.first { $0.id == "chrome/Default" }?.reason, "problem")
        XCTAssertTrue(progress.reconcile(connected: ["chrome/Default"]))
        let row = progress.rows.first { $0.id == "chrome/Default" }
        XCTAssertEqual(row?.state, .paired)
        XCTAssertEqual(row?.why, "")
        XCTAssertFalse(progress.reconcile(connected: ["chrome/Default"]))      // nothing more to change
        // what's left: the off profile needs the owner; the unanswered one is FERB's to try again
        XCTAssertEqual(progress.needOwner.map(\.id), ["brave/Default"])
        XCTAssertFalse(progress.rows.first { $0.id == "chrome/Profile 2" }!.needsOwner)
    }

    /// HAND-564: only a reason that names the owner's own step needs the owner; a pairing that didn't take doesn't.
    // decision: HAND-564, APP-920
    func testOnlyAReasonWithAnOwnersStepNeedsTheOwner() {
        for (reason, owner) in [("off", true), ("too_old", true), ("removed", true), ("problem", true),
                                ("no_answer", false), ("not_connected", false), ("", false)] {
            var progress = PairProgress()
            progress.apply(["type": "plan", "profiles": 1, "todo": [["browser": "chrome", "folder": "Default"]]])
            progress.apply(["type": "result", "browser": "chrome", "folder": "Default", "paired": false,
                            "reason": reason, "why": "why"])
            XCTAssertEqual(progress.rows[0].needsOwner, owner, reason)
        }
    }
}
