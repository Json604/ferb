import XCTest
@testable import NotchCore

/// L34 Phase G: a release build names FERB's public feed and key; the daemon's answer to `update.prepare` reads as
/// ready or as the work it waits for; the notch says "Updated to …" once per new build.
final class UpdatePlanTests: XCTestCase {
    private let release: [String: Any] = [
        "SUFeedURL": "https://github.com/Json604/ferb/releases/latest/download/appcast-arm64.xml",
        "SUPublicEDKey": "pfIShU4dEXqPd5ObYNfDBiQWcXozk7estwzTnF9BamQ="]

    func testOnlyAReleaseBuildWithFERBsFeedAndKeyChecks() {
        XCTAssertEqual(UpdatePlan.feedURL(release), release["SUFeedURL"] as? String)
        XCTAssertNil(UpdatePlan.feedURL([:]))                                         // a development build
        XCTAssertNil(UpdatePlan.feedURL(["SUFeedURL": release["SUFeedURL"]!]))         // no key: Sparkle would refuse
        XCTAssertNil(UpdatePlan.feedURL(["SUFeedURL": "https://example.org/appcast-arm64.xml",
                                         "SUPublicEDKey": "k"]))
    }

    func testTheDaemonsAnswerReadsAsReadyOrTheWorkItWaitsFor() {
        XCTAssertEqual(UpdatePlan.prepared(["ready": true, "running": [[String: Any]]()]), .ready)
        XCTAssertEqual(UpdatePlan.prepared(["ready": false, "running": [["task_id": "t-1", "goal": "Rename"],
                                                                        ["task_id": "t-2", "goal": ""]]]),
                       .waiting(["Rename", "t-2"]))
        XCTAssertEqual(UpdatePlan.prepared(["ready": false]), .waiting(["a task"]))
        XCTAssertEqual(UpdatePlan.waitingLine(version: "0.28.0", work: ["Rename", "process: npm run dev"]),
                       "FERB 0.28.0 installs when this finishes: Rename; process: npm run dev")
    }

    // decision: APP-750
    func testUpdatedToOncePerNewVersion() {
        func note(_ previousVersion: String?, _ previousBuild: String?, said: Bool = false) -> String? {
            UpdatePlan.updatedNote(previousVersion: previousVersion, previousBuild: previousBuild, build: "202611010900",
                                   version: "0.28.0", alreadySaid: said)
        }
        XCTAssertEqual(note("0.27.1", "202610250900"), "Updated to 0.28.0")
        XCTAssertNil(note("0.28.0", "202610311200"))                 // a rebuild or reinstall of the same version
        XCTAssertNil(note("0.28.0", "202611010900"))
        XCTAssertNil(note(nil, nil))                                 // a first launch
        // An older FERB recorded only its build: the line shows once, unless the chat already has it
        XCTAssertEqual(note(nil, "202610250900"), "Updated to 0.28.0")
        XCTAssertNil(note(nil, "202610250900", said: true))
        XCTAssertNil(note(nil, "202611010900"))
    }
}
