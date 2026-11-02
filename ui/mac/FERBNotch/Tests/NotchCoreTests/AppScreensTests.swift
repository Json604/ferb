import XCTest
@testable import FERBNotch
@testable import NotchCore

/// Step 12c's screens (DMN-6, APP-1, APP-2, STORE-402): the models read the daemon's replies
/// and the `ferb` commands' JSON; the stores and the setup model call the right methods with the right parameters.
@MainActor
final class AppScreensTests: XCTestCase {
    func testTheAboutDocumentReadsTheDaemonsSelfDescription() {
        let about = AboutDocument([
            "version": ["ferb": "0.27.0"], "models": ["planner": "Codex gpt-x", "worker": "Claude sonnet"],
            "account": ["email": "", "note": "FERB has no Google account of its own"],
            "permissions": [["id": "screen_recording", "status": "granted"]],
            "settings": [["key": "sense.window_titles", "value": true, "where": "Settings → Privacy"],
                         ["key": "privacy.exclude_domains", "value": [String](), "where": "Settings → Privacy"]],
            "records": [["records": "each task's full trace", "where": "FERB's database", "kept": "30 days after it ends"]],
            "health": ["database_bytes": 1_700_000_000, "cleanup_waiting_for_ok": true, "cpu_share_last_day": 0.092],
            "cost": ["last_7_days_usd": 1.5, "last_30_days_usd": 4.25, "basis": "API-equivalent"]])
        XCTAssertEqual(about.version, "0.27.0")
        XCTAssertEqual(about.models.map(\.name), ["planner", "worker"])
        XCTAssertEqual(about.account.first?.value, "not set up")
        XCTAssertEqual(about.settings.map(\.value), ["on", "none"])
        XCTAssertEqual(about.records.first?.value, "30 days after it ends")
        XCTAssertEqual(about.health.first?.name, "Database")
        XCTAssertFalse(about.health.first?.note.isEmpty ?? true)          // the clean-up waits for the owner's OK
        XCTAssertEqual(about.cost.map(\.value), ["$1.50", "$4.25"])
    }

    // decision: APP-4
    func testTheAppGuideNamesWhereEverySettingLives() {
        let keys = ["privacy.exclude_apps", "privacy.exclude_domains", "sense.window_titles", "sense.trail_retention_days",
                    "retention.trace_days", "retention.decision_days", "retention.evidence_days", "retention.recovery_days",
                    "retention.resource_days", "retention.operational_days", "browsing.owner_profile"]
        XCTAssertEqual(Set(AppGuide.places.keys), Set(keys))
        let payload = AppGuide.payload
        XCTAssertEqual((payload["howtos"] as? [[String: String]])?.count, AppGuide.howtos.count)
        XCTAssertEqual((payload["shortcuts"] as? [[String: String]])?.count, AppGuide.shortcuts.count)
    }

    // decision: APP-2
    func testRestoreRowsAndSavedSecretsCarryNamesNeverValues() {
        let trash = RestoreRow(["id": "r1", "kind": "trash", "original": "~/Desktop/a.txt", "reason": "trash",
                                "at": "2026-10-31T10:00:00+00:00"])
        XCTAssertEqual(trash?.restorable, true)
        XCTAssertEqual(trash?.what, "Moved to the Trash")
        XCTAssertEqual(RestoreRow(["id": "s1", "kind": "snapshot", "saved_to": "com.apple.TimeMachine.x"])?.restorable, false)
        XCTAssertNil(RestoreRow(["kind": "clone"]))
        let card = SavedSecretRow(["place": "https://shop.example", "name": "permit number", "kind": "card_number",
                                   "unusable": false, "value": "4111"])
        XCTAssertEqual(card?.kindLabel, "card number")
        XCTAssertFalse(String(describing: card as Any).contains("4111"))
    }

    func testTheCleanupPlanAndResultComeFromTheCommandsJSONLines() {
        let preview = ["{\"steps\": [{\"table\": \"decisions\", \"rows\": 131567, \"what\": \"Jev's decisions\"}, "
                       + "{\"table\": \"owner_questions\", \"rows\": 0, \"what\": \"never used; the table is dropped\", \"drop\": true}], "
                       + "\"compact\": true, \"rows\": 131567, \"ok\": \"abc123\", \"bytes\": 3000000000, "
                       + "\"needed\": 5000000000, \"free\": 90000000000}"]
        let plan = CleanupPlan(FerbJSON.last(preview))
        XCTAssertEqual(plan?.needed, 5000000000)
        XCTAssertEqual(plan?.steps.count, 2)
        XCTAssertEqual(plan?.steps.last?.dropped, true)
        XCTAssertEqual(plan?.applyArguments, ["setup", "--cleanup", "--ok", "abc123"])
        XCTAssertEqual(plan?.nothingToDo, false)
        XCTAssertEqual(CleanupPlan(["steps": [], "compact": false, "rows": 0, "ok": NSNull()])?.applyArguments, nil)
        let run = ["Emptying decisions…", "Removed 131,567 rows.",
                   "{\"removed\": {\"decisions\": 131567}, \"stopped\": false, \"compacted\": true, "
                   + "\"bytes_before\": 3000, \"bytes_after\": 1000}"]
        let done = CleanupResult(FerbJSON.last(run), status: 0)
        XCTAssertEqual(done.removed, 131567)
        XCTAssertTrue(done.compacted && !done.refused && done.problem == nil)
        let refused = CleanupResult(["refused": true, "plan": [:]], status: 2)
        XCTAssertTrue(refused.refused)
        XCTAssertNil(refused.problem)
        XCTAssertNotNil(CleanupResult(["not_run": "a FERB daemon still answers"], status: 1).problem)
        let roomless = CleanupResult(["removed": ["decisions": 7], "stopped": false, "compacted": false,
                                      "needed": 5000000000, "free": 1000], status: 0)
        XCTAssertFalse(roomless.compacted)
        XCTAssertEqual(roomless.needed, 5000000000)
    }

    // decision: APP-1
    func testTheIdentityStateListsOnlyProfilesHoldingFERBsAccountAlone() {
        let state = IdentityState(["current": ["address": "", "browser": "", "profile": "", "paired": false,
                                               "owner_profile": "chrome/Default"],
                                   "candidates": [["browser": "chrome", "browser_name": "Google Chrome",
                                                   "folder": "Profile 7", "name": "FERB", "only": true, "other_accounts": 0],
                                                  ["browser": "brave", "folder": "Default", "name": "Me", "only": false,
                                                   "other_accounts": 1]]])
        XCTAssertFalse(state.isSetUp)
        XCTAssertEqual(state.usable.map(\.id), ["chrome/Profile 7"])
        XCTAssertEqual(state.ownerProfile, "chrome/Default")
    }

    // decision: APP-1
    func testChoosingFERBsProfileWritesItThroughTheDaemonThenPairs() async {
        let rig = SetupRig()
        rig.rpc.replies["identity.find"] = ["current": ["address": ""], "candidates": [
            ["browser": "chrome", "folder": "Profile 7", "name": "FERB", "only": true]]]
        rig.rpc.replies["identity.choose"] = ["address": "ferb@gmail.com", "browser": "chrome", "profile": "Profile 7"]
        rig.rpc.replies["identity.pair"] = ["paired": true, "profile": "Profile 7"]
        let model = rig.model()
        model.identityAddress = "FERB@gmail.com"
        await model.refreshIdentity()
        XCTAssertEqual(rig.rpc.params(of: "identity.find").last?["address"] as? String, "ferb@gmail.com")
        guard let candidate = model.identity?.usable.first else { return XCTFail("no candidate") }
        await model.useIdentity(candidate)
        let chose = rig.rpc.params(of: "identity.choose").last
        XCTAssertEqual(chose?["browser"] as? String, "chrome")
        XCTAssertEqual(chose?["folder"] as? String, "Profile 7")
        XCTAssertTrue(rig.rpc.methods().contains("identity.pair"))
        await model.browseAsOwner(in: "chrome/Default")
        let set = rig.rpc.params(of: "settings.set").last
        XCTAssertEqual(set?["key"] as? String, "browsing.owner_profile")
        XCTAssertEqual(set?["value"] as? String, "chrome/Default")
    }

    // decision: APP-2
    func testDataPageCallsRestoreAndDeleteWithTheRowsIdentity() async {
        let rpc = ScriptedRPC()
        rpc.replies["recovery.list"] = ["days": 7, "total": 1, "records": [["id": "r1", "kind": "trash",
                                                                          "original": "~/a.txt", "reason": "trash"]]]
        rpc.replies["recovery.restore"] = ["id": "r1", "original": "~/a.txt", "restored_to": "~/a.txt", "in_place": true]
        rpc.replies["secrets.saved"] = ["entries": [["place": "https://x.example", "name": "password", "kind": "secret"]]]
        rpc.replies["secrets.delete"] = ["place": "https://x.example", "name": "password", "deleted": true]
        let store = FERBWindowStore(rpc: rpc)
        await store.loadRestore()
        await store.restore(store.restoreRows[0])
        XCTAssertEqual(rpc.params(of: "recovery.restore").last?["id"] as? String, "r1")
        await store.loadSecrets()
        await store.deleteSecret(store.savedSecrets[0])
        let deleted = rpc.params(of: "secrets.delete").last
        XCTAssertEqual(deleted?["place"] as? String, "https://x.example")
        XCTAssertEqual(deleted?["name"] as? String, "password")
    }

    // decision: APP-5, APP-2
    func testUninstallingEverythingRunsThePurgeTheOwnerConfirmedInTheApp() {
        XCTAssertEqual(FerbOutput.uninstallArguments(purge: false), ["uninstall"])
        XCTAssertEqual(FerbOutput.uninstallArguments(purge: true), ["uninstall", "--purge", "--yes"])
    }

    func testAnImportNoticeKeepsItsPageCardForTheMemoryPage() {
        let cards = ResultCard.list([["type": "note", "item_id": "m1", "text": "runs 3 times a week", "state": "saved"],
                                     ["type": "page", "page": "memory", "filter": ["imported_from": "chatgpt"],
                                      "line": "Open the Memory page"]])
        XCTAssertEqual(cards?.map(\.type), ["note", "page"])
        XCTAssertEqual(CardLayout.pages(cards ?? []).first?.importedFrom, "chatgpt")
        var archive = ConversationArchive()
        XCTAssertTrue(archive.addNotice(id: "n1", text: "Imported 1 fact from ChatGPT", cards: cards))
        XCTAssertEqual(archive.notices.first?.cards?.count, 2)
    }

    func testHealthSaysWhenTheCleanupWaitsForTheOwnersOK() {
        let lines = HealthLine.lines(["database": ["bytes": 3_000_000_000, "cleanup_pending": true]])
        XCTAssertEqual(lines.first?.name, "Database")
        XCTAssertEqual(lines.first?.ok, false)
    }

    // MARK: What FERB saw (step 12e)

    // decision: DMN-27
    func testWhatFerbSawReadsTheDaemonsCapturesNewestFirst() {
        let list = SeenList(["captures": [
            ["id": "e2", "at": "2026-10-31T10:00:05.123456+00:00", "kind": "screenshot", "task_id": "t1",
             "app": "Notes", "site": "", "title": "", "tool": "mac.screenshot"],
            ["id": "e1", "at": "2026-10-31T10:00:00+00:00", "kind": "front_window", "task_id": "r1",
             "app": "Google Chrome", "site": "docs.example", "title": "Quarterly plan"],
            ["kind": "screenshot"]],                                         // no id: skipped
            "total": 5, "days": 7])
        XCTAssertEqual(list.captures.map(\.id), ["e2", "e1"])
        XCTAssertEqual(list.captures[1].label, "docs.example — Quarterly plan")    // the site, then the page
        XCTAssertEqual(list.captures[0].label, "Notes")                            // no site: the app
        XCTAssertEqual(list.captures[1].what, "Your front window when you sent it")
        XCTAssertNotNil(list.captures[0].at)
        XCTAssertNotNil(list.captures[1].at)
        XCTAssertEqual(list.moreLine, "Showing the newest 2 of 5.")
        XCTAssertNil(SeenList(["captures": [], "total": 0]).moreLine)
        XCTAssertEqual(SeenList([:]).days, 7)
    }

    // decision: DMN-27
    func testWhatFerbSawAsksForAMessagesOrATasksScreenshots() {
        XCTAssertEqual(SeenScope(requestID: "r1", taskID: "t1"), .request("r1"))   // a message: by its request
        XCTAssertEqual(SeenScope(requestID: nil, taskID: "t1"), .task("t1"))       // a task this chat didn't send
        XCTAssertNil(SeenScope(requestID: "", taskID: nil))
        XCTAssertEqual(SeenScope.request("r1").params["request_id"] as? String, "r1")
        XCTAssertEqual(SeenScope.task("t1").params["task_id"] as? String, "t1")
        XCTAssertTrue(SeenScope.all.params.isEmpty)
    }

    // decision: DMN-27
    func testWhatFerbSawLoadsTheListThenEachImageOnce() async {
        let rpc = ScriptedRPC()
        rpc.replies["seen.list"] = ["captures": [["id": "e1", "kind": "front_window", "app": "Mail"]], "total": 1]
        let pixel = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 1, pixelsHigh: 1, bitsPerSample: 8,
                                     samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
                                     bytesPerRow: 0, bitsPerPixel: 0)
        let png = pixel?.representation(using: .png, properties: [:]) ?? Data()
        rpc.replies["seen.image"] = ["id": "e1", "image": png.base64EncodedString()]
        let model = SeenModel(rpc: rpc, scope: .request("r1"))
        await model.load()
        XCTAssertEqual(model.list?.captures.map(\.id), ["e1"])
        XCTAssertEqual(rpc.params(of: "seen.list").first?["request_id"] as? String, "r1")
        let capture = model.list!.captures[0]
        await model.image(capture)
        await model.image(capture)
        XCTAssertEqual(rpc.params(of: "seen.image").count, 1)
        XCTAssertEqual(rpc.params(of: "seen.image").first?["id"] as? String, "e1")
    }
}
