import XCTest
@testable import NotchCore

/// L34 Phase E: the FERB window reads the daemon's replies into rows. A row without its id is dropped; any other
/// missing field reads as empty, so an older daemon still shows what it has.
final class WindowRowsTests: XCTestCase {
    func testDaemonTimesWithAndWithoutMicrosecondsParse() {
        XCTAssertNotNil(WindowTime.parse("2026-10-25T14:03:00.123456+05:30"))
        XCTAssertNotNil(WindowTime.parse("2026-10-25T14:03:00+05:30"))
        XCTAssertNil(WindowTime.parse("yesterday"))
        XCTAssertNil(WindowTime.parse(nil))
    }

    func testTaskRowsKeepTheirAnswerVerdictAndPagingTime() throws {
        let row = try XCTUnwrap(TaskHistoryRow(["task_id": "t-1", "goal": "Rename the invoices", "status": "done",
                                                "created_at": "2026-10-25T14:03:00+05:30", "answer": "Renamed 4.",
                                                "verdict": "pass"]))
        XCTAssertEqual(row.id, "t-1")
        XCTAssertEqual(row.createdAtText, "2026-10-25T14:03:00+05:30")
        XCTAssertNotNil(row.createdAt)
        XCTAssertEqual(row.verdict, "pass")
        XCTAssertFalse(row.isRunning)
        XCTAssertTrue(TaskHistoryRow(["task_id": "t-2", "status": "running"])!.isRunning)
        XCTAssertNil(TaskHistoryRow(["goal": "no id"]))
        XCTAssertNil(TaskHistoryRow(["task_id": "", "goal": "empty id"]))
    }

    func testScheduleRowsOfferOnlyTheActionsTheirStateAllows() throws {
        let active = try XCTUnwrap(ScheduleRow(["id": "sc-1", "request": "Post the update", "rule": "every day at 09:00",
                                                "state": "active", "next_run": "2026-10-26T09:00:00+00:00",
                                                "failures": 2, "last_result": ["at": "2026-10-25T09:01:00+00:00",
                                                                               "status": "done", "answer": "Posted."]]))
        XCTAssertEqual(active.actions, ["pause", "cancel"])
        XCTAssertEqual(active.failures, 2)
        XCTAssertEqual(active.lastResult?.answer, "Posted.")
        XCTAssertEqual(ScheduleRow(["id": "sc-2", "state": "paused"])?.actions, ["resume", "cancel"])
        XCTAssertEqual(ScheduleRow(["id": "sc-3", "state": "done"])?.actions, ["cancel"])
        XCTAssertNil(ScheduleRow(["id": "sc-4", "last_result": NSNull()])?.lastResult)
    }

    func testRemindersNoticesAndMemory() throws {
        let reminder = try XCTUnwrap(ReminderRow(["id": "tm-1", "kind": "reminder", "at": "2026-10-25T18:00:00+00:00",
                                                  "message": "Call the bank"]))
        XCTAssertEqual(reminder.message, "Call the bank")
        let notice = try XCTUnwrap(NoticeRow(["id": "n-1", "text": "Your build finished.", "shown": true,
                                              "quotes": [["text": "BUILD OK", "origin": "external:observation"]],
                                              "at": "2026-10-25T10:00:00+00:00"]))
        XCTAssertEqual(notice.quotes, [NoticeQuote(text: "BUILD OK", origin: "external:observation")])
        XCTAssertTrue(notice.shown)
        let item = try XCTUnwrap(MemoryRow(["id": "m-1", "kind": "fact", "subject": "owner", "key": "editor",
                                            "content": "Uses VS Code", "status": "active", "origin": "owner_stated"]))
        XCTAssertEqual(item.content, "Uses VS Code")
    }

    func testSettingValuesKeepBooleansApartFromNumbers() throws {
        let json = try JSONSerialization.jsonObject(with: Data(#"""
            {"values": {"sense.window_titles": true, "retention.trace_days": 1,
                        "privacy.exclude_domains": ["example.com"]}}
            """#.utf8)) as! [String: Any]
        let snapshot = SettingsSnapshot(json)
        XCTAssertEqual(snapshot.values["sense.window_titles"], .bool(true))
        XCTAssertEqual(snapshot.values["retention.trace_days"], .int(1))       // a 1 from JSON stays a number
        XCTAssertTrue(snapshot.bool("sense.window_titles"))
        XCTAssertEqual(snapshot.int("retention.trace_days"), 1)
        XCTAssertEqual(snapshot.strings("privacy.exclude_domains"), ["example.com"])
        XCTAssertEqual(SettingValue(true)?.json as? Bool, true)
        XCTAssertEqual(SettingValue(["a", "b"])?.json as? [String], ["a", "b"])
    }

    func testHealthLinesFromTheDaemonsSelfTestAndContainment() {
        let lines = HealthLine.lines(["selftest": [["name": "accessibility", "ok": true, "detail": ""],
                                                   ["name": "screenshot", "ok": false, "detail": "no window"]],
                                      "containment": ["profile_ok": true, "proxy_alive": false, "reason": ""]])
        XCTAssertEqual(lines.map(\.name), ["accessibility", "screenshot", "Shell containment"])
        XCTAssertEqual(lines.map(\.ok), [true, false, false])
        XCTAssertEqual(lines[1].detail, "no window")
    }
}
