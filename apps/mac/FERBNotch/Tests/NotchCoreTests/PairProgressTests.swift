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
}
