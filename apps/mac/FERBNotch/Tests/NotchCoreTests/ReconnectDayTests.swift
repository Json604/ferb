import XCTest
@testable import FERBNotch
@testable import NotchCore

/// Owner 2026-11-19, on FERB.app's Today page after the first restart of a new day: "btw why am i being shown this taks
/// message now??? is it due to your breakign the messages into non-day breask or what??" The task this chat sent last
/// (`FERBActiveTaskID`, never cleared) ended the day before and its result was shown; the launch took it up again,
/// asked for its trace, and filed its old answer as a new turn dated the restart, on today's page, beside the turn it
/// already had on yesterday's (APP-975). Each test is a launch of FERB.app against the same daemon (`HistoryRPC`, the
/// chat in FERB's database as APP-805 keeps it): its history read, then the status read a connection makes.
@MainActor
final class ReconnectDayTests: XCTestCase {
    static let iso = Date.ISO8601FormatStyle(includingFractionalSeconds: true)
    static var yesterday: Date { Calendar.current.date(byAdding: .day, value: -1, to: Calendar.current.startOfDay(for: Date()))! }

    /// FERB.app started against `rpc` with `defaults`: today's page and the waiting turns read, then the status.
    func launch(_ rpc: HistoryRPC, _ defaults: UserDefaults) async -> NotchModel {
        let model = NotchModel(rpc: rpc, preferences: defaults)
        await model.prepareHistory()
        XCTAssertEqual(model.chatStore.store, .daemon)
        await model.refreshDaemonStatus()
        await model.writeHistory()
        return model
    }

    /// Yesterday's turn of `task`, sent from this chat and answered, as the daemon keeps it; the task remembered.
    func yesterdaysTask(_ rpc: HistoryRPC, _ defaults: UserDefaults, task: String, answer: String) async -> UUID {
        let earlier = NotchModel(rpc: rpc, preferences: defaults)
        await earlier.prepareHistory()
        let id = UUID()
        earlier.conversation.begin(id: id, request: "find them through mailmeteor", sources: [],
                                   date: Self.yesterday.addingTimeInterval(4 * 3600))
        earlier.conversation.bind(id: id, taskID: task)
        earlier.conversation.finish(id: id, answer: answer, status: "done_with_gaps", artifacts: [])
        earlier.saveConversation()
        await earlier.writeHistory()
        defaults.set(task, forKey: "FERBActiveTaskID")
        return id
    }

    func rows(_ rpc: HistoryRPC, task: String) -> [[String: Any]] {
        rpc.rows.values.filter { (($0["body"] as? [String: Any])?["backendTaskID"] as? String) == task }
    }

    // decision: APP-975, APP-974
    func testARestartOnALaterDayNeverShowsAFinishedTaskAgain() async throws {
        let rpc = HistoryRPC()
        let defaults = scratchDefaults("ReconnectDay")
        let answer = "Stopped and left the unsent Gmail draft open for your review."
        let id = await yesterdaysTask(rpc, defaults, task: "task-1", answer: answer)
        let kept = try XCTUnwrap(rpc.held(id)) as NSDictionary
        // its trace still answers, as the daemon's does for any task
        rpc.traces["task-1"] = ["outcome": ["result": ["status": "done_with_gaps", "answer": answer]]]

        let model = await launch(rpc, defaults)
        XCTAssertEqual(rpc.rows.count, 1, "no new entry: the finished task isn't filed again")
        XCTAssertTrue(rpc.rows.values.allSatisfy { $0["day"] as? String == HistoryEntry.day(Self.yesterday) },
                      "nothing on today's page")
        XCTAssertEqual(rpc.held(id) as NSDictionary?, kept, "yesterday's turn is as it was: its day, time and answer")
        XCTAssertTrue(model.visibleEntries.isEmpty, "today's page is empty")
        XCTAssertFalse(model.conversation.turns.contains { $0.backendTaskID == "task-1" })
        XCTAssertEqual(rpc.count("trace.get"), 0, "a task that ended and was shown isn't looked up")
        XCTAssertNotEqual(model.session.phase, .working)
        XCTAssertNil(defaults.string(forKey: "FERBActiveTaskID"), "the remembered task is forgotten")

        // and the next launch, another day or the same, adds nothing either
        _ = await launch(rpc, defaults)
        XCTAssertEqual(rows(rpc, task: "task-1").count, 1)
    }

    // decision: APP-975, APP-585
    func testATaskThatEndedWhileTheAppWasAwayIsShownOnceAtItsOwnTime() async throws {
        let rpc = HistoryRPC()
        let defaults = scratchDefaults("ReconnectDay")
        let ended = Self.yesterday.addingTimeInterval(22 * 3600)
        rpc.results = [["task_id": "task-2", "status": "done", "answer": "Booked the table for eight.",
                        "goal": "Book a table", "at": Self.iso.format(ended)]]
        _ = await launch(rpc, defaults)
        let filed = rows(rpc, task: "task-2")
        XCTAssertEqual(filed.count, 1, "filed once")
        XCTAssertEqual(filed.first?["day"] as? String, HistoryEntry.day(ended), "on the day it ended")
        let at = try XCTUnwrap((filed.first?["at"] as? String).flatMap { try? Self.iso.parse($0) })
        XCTAssertEqual(at.timeIntervalSince1970, ended.timeIntervalSince1970, accuracy: 1, "at the time it ended")
        for _ in 0..<200 where !rpc.results.isEmpty { try await Task.sleep(nanoseconds: 2_000_000) }
        XCTAssertTrue(rpc.results.isEmpty, "marked shown once placed")

        let again = await launch(rpc, defaults)
        XCTAssertEqual(rows(rpc, task: "task-2").count, 1, "never filed a second time")
        XCTAssertTrue(again.visibleEntries.isEmpty, "today's page has nothing of it")
    }

    // decision: APP-975
    func testATaskStillRunningAfterARestartShowsAsRunningAndItsAnswerKeepsItsTime() async throws {
        let rpc = HistoryRPC()
        let defaults = scratchDefaults("ReconnectDay")
        defaults.set("task-3", forKey: "FERBActiveTaskID")
        rpc.running = [["task_id": "task-3", "goal": "Export the photos",
                        "started_at": Self.iso.format(Date().addingTimeInterval(-600))]]
        let model = await launch(rpc, defaults)
        XCTAssertEqual(model.session.backendTaskID, "task-3")
        XCTAssertEqual(model.session.phase, .working, "shown as running")
        XCTAssertNotNil(model.runningTasks["task-3"])
        XCTAssertTrue(rows(rpc, task: "task-3").isEmpty, "no answer filed for a task still running")
        XCTAssertEqual(defaults.string(forKey: "FERBActiveTaskID"), "task-3", "still remembered while it runs")

        // it ends: its turn is dated when it ended, and it is no longer remembered
        let ended = Date().addingTimeInterval(-90)
        rpc.running = []
        model.receiveOutcome(["task_id": "task-3", "status": "done", "answer": "Exported 40 photos.",
                              "at": Self.iso.format(ended)])
        let turn = try XCTUnwrap(model.conversation.turns.first { $0.backendTaskID == "task-3" })
        XCTAssertEqual(turn.createdAt.timeIntervalSince1970, ended.timeIntervalSince1970, accuracy: 1)
        XCTAssertEqual(turn.request, "", "never shown as the owner's words")
        XCTAssertNil(defaults.string(forKey: "FERBActiveTaskID"))
    }
}
