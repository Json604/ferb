import XCTest
@testable import FERBNotch
import NotchCore

/// The browser account beside every profile name (HAND-424, APP-736). Invented addresses only.
final class ProfileNameTests: XCTestCase {
    // decision: APP-736
    func testANameCarriesItsAccountOnlyWhenThereIsOne() {
        let work = ProfileName(label: "Google Chrome", name: "Work", email: "work@example.com")
        XCTAssertEqual(work.title, "Google Chrome “Work”")
        XCTAssertEqual(work.text, "Google Chrome “Work” · work@example.com")
        XCTAssertEqual(work.quoted, "“Work” · work@example.com")
        XCTAssertEqual(ProfileName(label: "Google Chrome", name: "Home").text, "Google Chrome “Home”")
    }

    // decision: APP-736, DMN-546
    func testTheBrowsersSnapshotKeepsEachProfilesAccount() {
        let snapshot = BrowserSnapshot(reply: ["browsers": [["browser": "chrome", "label": "Google Chrome", "profiles": [
            ["folder": "Default", "name": "Work", "email": "work@example.com", "state": "paired"],
            ["folder": "Profile 1", "name": "Home", "state": "on"]]]]])
        let rows = snapshot?.browsers.first?.profiles ?? []
        XCTAssertEqual(rows.map(\.email), ["work@example.com", ""])
    }

    // decision: APP-736, DMN-546
    @MainActor func testPairAllKeepsTheAccountAndAResultReplacesThePlans() {
        var progress = PairProgress()
        progress.apply(["type": "plan", "profiles": 2,
                        "todo": [["browser": "chrome", "folder": "Default", "name": "Work", "label": "Google Chrome",
                                  "email": "old@example.com"],
                                 ["browser": "chrome", "folder": "Profile 1", "name": "Home", "label": "Google Chrome"]]])
        XCTAssertEqual(progress.rows.first?.text, "Google Chrome “Work” · old@example.com")
        progress.apply(["type": "result", "browser": "chrome", "folder": "Default", "name": "Work",
                        "label": "Google Chrome", "email": "work@example.com", "paired": true])
        XCTAssertEqual(progress.rows.first { $0.id == "chrome/Default" }?.email, "work@example.com")
        let line = IntroCopy.pairing(progress)
        // all in one browser: the rows name the profile alone (owner 2026-11-09, a friend's 13 Chrome profiles)
        XCTAssertEqual(line.checklist.map(\.title), ["“Work”", "“Home”"])
        XCTAssertEqual(line.checklist.map(\.secondary), ["work@example.com", ""])     // drawn apart from the name
        XCTAssertTrue(line.text.contains("“Home”") && !line.text.contains("@"))
        // profiles in two browsers keep their browser's name
        progress.apply(["type": "result", "browser": "brave", "folder": "Default", "name": "Play", "label": "Brave",
                        "paired": true])
        XCTAssertEqual(IntroCopy.pairing(progress).checklist.map(\.title),
                       ["Google Chrome “Work”", "Google Chrome “Home”", "Brave “Play”"])
    }

    // decision: APP-736, HAND-411
    @MainActor func testPairAllsMessageNamesEachProfileWithItsAccount() {
        let message = SetupModel.pairAllMessage(["profiles": 2, "skipped": [
            ["name": "Work", "label": "Google Chrome", "email": "work@example.com", "step": "turn FERB on there"],
            ["name": "Work", "label": "Google Chrome", "email": "", "step": "add FERB there"]]])
        XCTAssertEqual(message, "Google Chrome “Work” · work@example.com: turn FERB on there.\n"
                       + "Google Chrome “Work”: add FERB there.")
    }

    // decision: APP-736, DMN-546
    func testTheIdentityStateAndAboutNameProfilesByName() {
        let state = IdentityState(["current": ["address": "ferb@example.com", "browser": "chrome", "profile": "Profile 2",
                                               "profile_name": "FERB", "profile_email": "ferb@example.com",
                                               "owner_profile": "chrome/Default", "owner_profile_name": "Work",
                                               "owner_profile_email": "work@example.com", "paired": true]])
        XCTAssertEqual(state.profileShown.quoted, "“FERB” · ferb@example.com")
        XCTAssertEqual(state.ownerProfileEmail, "work@example.com")
        let unread = IdentityState(["current": ["address": "ferb@example.com", "browser": "chrome",
                                                "profile": "Profile 2", "paired": false]])
        XCTAssertEqual(unread.profileShown.quoted, "“Profile 2”")              // Full Disk Access off: its folder
        let about = AboutDocument(["account": ["email": "ferb@example.com", "browser": "chrome", "profile": "Profile 2",
                                               "profile_name": "FERB", "paired": true]])
        XCTAssertEqual(about.account.first?.note, "chrome / “FERB” · paired")
    }
}
