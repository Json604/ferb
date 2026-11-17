import XCTest
@testable import NotchCore

/// L34 Phase G: a release build names FERB's public feed and key; the daemon's answer to `update.prepare` reads as
/// ready or as the work it waits for; the notch says "Updated to …" once per new build.
final class UpdatePlanTests: XCTestCase {
    private let release: [String: Any] = [
        "SUFeedURL": "https://ferb.kartikey.xyz/updates/appcast-arm64.xml",
        "SUPublicEDKey": "pfIShU4dEXqPd5ObYNfDBiQWcXozk7estwzTnF9BamQ="]

    func testOnlyAReleaseBuildWithFERBsFeedAndKeyChecks() {
        XCTAssertEqual(UpdatePlan.feedURL(release), release["SUFeedURL"] as? String)
        XCTAssertNil(UpdatePlan.feedURL([:]))                                         // a development build
        XCTAssertNil(UpdatePlan.feedURL(["SUFeedURL": release["SUFeedURL"]!]))         // no key: Sparkle would refuse
        XCTAssertNil(UpdatePlan.feedURL(["SUFeedURL": "https://example.org/appcast-arm64.xml",
                                         "SUPublicEDKey": "k"]))
    }

    // decision: REL-431, REL-432
    func testABuildAsksTheOwnersDomainFirstThenTheReleasesOwnAddressAndNothingElse() {
        XCTAssertEqual(UpdatePlan.feeds(release),
                       ["https://ferb.kartikey.xyz/updates/appcast-arm64.xml",
                        "https://github.com/Json604/ferb-update/releases/latest/download/appcast-arm64.xml"])
        // a build made with the second address keeps it first, and still knows the other
        let direct: [String: Any] = [
            "SUFeedURL": "https://github.com/Json604/ferb-update/releases/latest/download/appcast-x86_64.xml",
            "SUPublicEDKey": "k"]
        XCTAssertEqual(UpdatePlan.feeds(direct).last, "https://ferb.kartikey.xyz/updates/appcast-x86_64.xml")
        XCTAssertEqual(UpdatePlan.feeds([:]), [])
        // only a feed that couldn't be had moves on to the next address; the last one's failure is told
        XCTAssertTrue(UpdatePlan.feedFailed(domain: "SUSparkleErrorDomain", code: 1002))
        XCTAssertTrue(UpdatePlan.feedFailed(domain: "SUSparkleErrorDomain", code: 1000))
        XCTAssertFalse(UpdatePlan.feedFailed(domain: "SUSparkleErrorDomain", code: 2001))   // a download that failed
        XCTAssertFalse(UpdatePlan.feedFailed(domain: "NSURLErrorDomain", code: 1002))
        XCTAssertEqual(UpdatePlan.nextFeed(after: 0, of: UpdatePlan.feeds(release)), 1)
        XCTAssertNil(UpdatePlan.nextFeed(after: 1, of: UpdatePlan.feeds(release)))
        // the old address of the now private repository is no feed of this build
        XCTAssertNil(UpdatePlan.feedURL(["SUFeedURL": "https://github.com/Json604/ferb/releases/latest/download/appcast-arm64.xml",
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
