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

    // decision: HAND-523, APP-736, APP-909, HAND-524
    func testAFallbackEventGivesThatProfileOneLineUntilItsResult() {
        var progress = PairProgress()
        progress.apply(["type": "plan", "profiles": 3,
                        "todo": [["browser": "chrome", "folder": "Default", "name": "Personal", "label": "Google Chrome"],
                                 ["browser": "brave", "folder": "Default", "name": "Work", "label": "Brave"],
                                 ["browser": "chrome", "folder": "Profile 1", "name": "Personal",
                                  "email": "a@example.com", "label": "Google Chrome"]]])
        XCTAssertEqual(progress.fallbackLines, [])                  // no event, no line
        XCTAssertTrue(progress.rows.allSatisfy { $0.fallbackLine.isEmpty })
        for (browser, folder) in [("chrome", "Default"), ("brave", "Default"), ("chrome", "Profile 1")] {
            progress.apply(["type": "fallback", "browser": browser, "folder": folder])
        }
        progress.apply(["type": "fallback", "browser": "chrome", "folder": "Nowhere"])     // no such row: nothing
        XCTAssertEqual(progress.fallbackLines, [
            "Google Chrome “Personal” didn't answer quietly, so I'm opening it to finish pairing.",
            "Brave “Work” didn't answer quietly, so I'm opening it to finish pairing.",
            "Google Chrome “Personal” · a@example.com didn't answer quietly, so I'm opening it to finish pairing."])
        // its result replaces the line
        progress.apply(["type": "result", "browser": "brave", "folder": "Default", "name": "Work", "label": "Brave",
                        "paired": true])
        XCTAssertEqual(progress.rows.first { $0.id == "brave/Default" }?.fallbackLine, "")
        XCTAssertEqual(progress.fallbackLines.count, 2)
        // a fallback told after the result (out of order) adds nothing
        progress.apply(["type": "fallback", "browser": "brave", "folder": "Default"])
        XCTAssertEqual(progress.fallbackLines.count, 2)
    }

    // decision: HAND-412, APP-909
    func testADaemonThatNeverSendsAFallbackLeavesNoLine() {
        var progress = PairProgress()
        progress.apply(["type": "plan", "profiles": 1,
                        "todo": [["browser": "chrome", "folder": "Default", "name": "Personal", "label": "Google Chrome"]]])
        progress.apply(["type": "something_new", "browser": "chrome", "folder": "Default"])
        XCTAssertEqual(progress.fallbackLines, [])
        progress.apply(["type": "result", "browser": "chrome", "folder": "Default", "name": "Personal",
                        "label": "Google Chrome", "paired": false, "why": "turn it on"])
        progress.apply(["type": "done", "profiles": 1, "paired": 0])
        XCTAssertEqual(progress.fallbackLines, [])
        XCTAssertFalse(progress.rows[0].fellBack)
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
