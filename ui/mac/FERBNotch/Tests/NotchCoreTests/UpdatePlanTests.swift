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

    func testUpdatedToOncePerNewBuild() {
        XCTAssertEqual(UpdatePlan.updatedNote(previousBuild: "202610250900", build: "202611010900", version: "0.28.0"),
                       "Updated to 0.28.0")
        XCTAssertNil(UpdatePlan.updatedNote(previousBuild: "202611010900", build: "202611010900", version: "0.28.0"))
        XCTAssertNil(UpdatePlan.updatedNote(previousBuild: nil, build: "202611010900", version: "0.28.0"))
    }
}
