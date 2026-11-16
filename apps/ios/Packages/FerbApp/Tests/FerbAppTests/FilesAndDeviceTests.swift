import FerbAppModel
import FerbCore
import FerbMobile
import XCTest

final class LocalFilesTests: XCTestCase {
    private var root: URL!
    override func setUp() { root = FileManager.default.temporaryDirectory.appendingPathComponent("ferb-files-\(UUID().uuidString)", isDirectory: true) }
    override func tearDown() { try? FileManager.default.removeItem(at: root) }

    // decision: PHN-515, PHN-11, PHN-574
    func testFilesLiveOutOfBackupsProtectedAndNothingClimbsOut() throws {
        let local = LocalFiles(root: root)
        let target = local.fetchTarget("k1", "../../etc/passwd")
        XCTAssertEqual(target.lastPathComponent, "passwd")
        XCTAssertEqual(target.deletingLastPathComponent().lastPathComponent, "k1")
        XCTAssertEqual(try root.resourceValues(forKeys: [.isExcludedFromBackupKey]).isExcludedFromBackup, true)
        #if !targetEnvironment(simulator)
        // the simulator has no Data Protection (the attribute reads back as none): checked on a phone only
        let attrs = try FileManager.default.attributesOfItem(atPath: local.fetched.path)
        XCTAssertEqual(attrs[.protectionKey] as? FileProtectionType, .completeUnlessOpen)
        #endif
        XCTAssertEqual(LocalFiles.safe(".."), "file")
        XCTAssertEqual(LocalFiles.safeRel("a/../b\\c"), "a/file/b/c")
    }

    // decision: PHN-16, PHN-515
    func testNothingFetchedIsKeptLongerThanADay() throws {
        let local = LocalFiles(root: root)
        let old = local.fetchTarget("old", "a.txt")
        let fresh = local.fetchTarget("new", "b.txt")
        try Data("a".utf8).write(to: old)
        try Data("b".utf8).write(to: fresh)
        let now = Date()
        try FileManager.default.setAttributes([.modificationDate: now.addingTimeInterval(-LocalFiles.maxAge - 60)], ofItemAtPath: old.path)
        local.dropOld(now: now)
        XCTAssertFalse(FileManager.default.fileExists(atPath: old.deletingLastPathComponent().path))
        XCTAssertTrue(FileManager.default.fileExists(atPath: fresh.path))
        local.wipeFetched()
        XCTAssertFalse(FileManager.default.fileExists(atPath: local.fetched.path))
    }
}

@MainActor
final class DeviceTests: XCTestCase {
    // decision: SENSE-420
    func testTheBatteryCountsInStepsAndThePlaceOnlyWhileShared() {
        let r = DeviceReporter(DeviceFacts(name: "iPhone", model: "iPhone17,1", os: "iOS 18.6"))
        r.battery(percent: 81, charging: false)
        r.battery(percent: 83, charging: false)
        XCTAssertEqual(r.facts.batteryPercent, 81, "the same 5-point step: unchanged")
        r.battery(percent: 79, charging: false)
        XCTAssertEqual(r.facts.batteryPercent, 79)
        let place = DevicePlace(city: "Springfield", region: "Region", country: "Country", countryCode: "XX")
        r.location(DeviceFacts.locationOn, place: place)
        XCTAssertEqual(r.facts.report().place, place)
        r.location(DeviceFacts.locationOff, place: nil)
        XCTAssertNil(r.facts.report().place, "never a place while off")
    }

    // decision: SENSE-420
    func testTheCityIsLookedUpAgainOnlyAfterAKilometre() async {
        var asked = 0
        let keeper = PlaceKeeper { _, _ in
            asked += 1
            return DevicePlace(city: "Springfield", region: "", country: "", countryCode: "XX")
        }
        _ = await keeper.fix(latitude: 10.0, longitude: 20.0)
        _ = await keeper.fix(latitude: 10.001, longitude: 20.001) // about 150 m
        XCTAssertEqual(asked, 1)
        _ = await keeper.fix(latitude: 10.02, longitude: 20.0) // about 2 km
        XCTAssertEqual(asked, 2)
        XCTAssertEqual(keeper.coarse, Coarse(latitude: 10.02, longitude: 20.0))
        keeper.forget()
        XCTAssertNil(keeper.place)
    }

    // decision: SENSE-420
    func testTheReportGoesWhenItChangesAndThePhoneIsOpen() async {
        let phone = FakePhone()
        let r = DeviceReporter(DeviceFacts(name: "iPhone"))
        r.start(phone)
        await eventually("the first report") { phone.reports.count == 1 }
        r.network("wifi")
        await eventually("the change") { phone.reports.count == 2 }
        XCTAssertEqual(phone.reports.last?.network, "wifi")
        r.stop()
    }
}

/// Every catalog key the model names exists (the screens never show a ⟦key⟧).
final class CatalogTests: XCTestCase {
    func testEveryKeyTheModelNamesIsInTheCatalog() {
        let keys = [
            "question_gone", "keys_registered", "outbox_may_have_arrived", "stopped", "stop_all_done", "answer_needs_pairing",
            "files_not_opened", "files_tree_empty", "files_folder_unreadable", "files_folder_empty", "files_unreadable",
            "not_on_shared_relay", "live_needs_unlock", "live_ended", "set_days_invalid", "needs_pairing_again", "exact_sent",
            "exact_not_sent", "city_denied", "loc_why_denied", "loc_why_none", "loc_why_no", "err_offline", "err_needs_better_path",
            "err_needs_unlock", "err_mac_said", "err_other", "pair_expired", "pair_wrong_code", "pair_limit", "pair_denied",
            "pair_busy", "pair_already", "pair_unreachable", "feature_voice", "not_yet_voice",
            "n_done_held", "n_held_text", "n_task_failed", "n_task_done", "n_link_removed_title", "n_link_removed_text",
            "n_public_question", "n_public_done", "n_question_title", "n_private_title", "n_private_text", "n_check_title",
            "n_check_text", "n_on_mac", "n_needs_fingerprint_text", "n_needs_fingerprint", "n_needs_unlock", "n_answer",
            "n_answer_hint", "n_needs_fingerprint_title", "n_not_answered_title",
        ] + MacSettings.specs.map(\.titleKey) + Why.allCases.map(\.subtitleKey)
        let missing = keys.filter { !L.has($0) }
        XCTAssertEqual(missing, [], "keys the model names but the catalog lacks")
    }
}
