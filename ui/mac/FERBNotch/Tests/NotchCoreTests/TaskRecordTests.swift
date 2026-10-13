import Foundation
import XCTest
@testable import FERBNotch
@testable import NotchCore

final class TaskRecordTests: XCTestCase {
    func testCompleteLargeRecordIsPrivateFileRatherThanInlineText() async throws {
        let records = TaskRecordFiles()
        let payload: [String: Any] = ["events": Array(repeating: ["text": String(repeating: "full evidence ", count: 100)], count: 4_000), "last": "retained"]
        let url = try await records.write(payload)
        let decoded = try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as! NSDictionary
        XCTAssertEqual(decoded, payload as NSDictionary)
        XCTAssertEqual(try FileManager.default.attributesOfItem(atPath: url.path)[.posixPermissions] as? Int, 0o600)
        XCTAssertEqual(try FileManager.default.attributesOfItem(atPath: url.deletingLastPathComponent().path)[.posixPermissions] as? Int, 0o700)
        let next = try await records.write(["next": true])
        XCTAssertNotEqual(next, url)
        XCTAssertTrue(FileManager.default.fileExists(atPath: url.path))
    }

    func testConcurrentExportsKeepBothCompleteFilesPrivate() async throws {
        let records = TaskRecordFiles()
        async let first = records.write(["task": "first"])
        async let second = records.write(["task": "second"])
        let urls = try await [first, second]
        XCTAssertNotEqual(urls[0], urls[1])
        for (url, expected) in zip(urls, ["first", "second"]) {
            let record = try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: String]
            XCTAssertEqual(record, ["task": expected])
            XCTAssertEqual(try FileManager.default.attributesOfItem(atPath: url.path)[.posixPermissions] as? Int, 0o600)
            XCTAssertEqual(try FileManager.default.attributesOfItem(atPath: url.deletingLastPathComponent().path)[.posixPermissions] as? Int, 0o700)
        }
    }

    @MainActor
    func testHiddenInspectionDoesNotPublishLateTrace() async throws {
        let rpc = DelayedTraceRPC()
        let model = NotchModel(rpc: rpc, preferences: UserDefaults(suiteName: UUID().uuidString)!)
        model.runningTasks = ["first": "First"]
        let started = expectation(description: "Inspection started")
        rpc.onRequest = { _ in started.fulfill() }
        model.inspectTask(model.taskItems[0])
        let job = model.taskInspectionJob
        await fulfillment(of: [started], timeout: 2)
        model.clearTaskInspection()
        rpc.pending.removeValue(forKey: "first")?.resume(returning: ["stale": true])
        await job?.value
        XCTAssertNil(model.taskRecordURL)
        XCTAssertNil(model.taskInspection)
        XCTAssertNil(model.inspectedTaskID)
    }

    @MainActor
    func testSwitchingTasksRejectsLateTraceAndKeepsCurrentFile() async throws {
        let rpc = DelayedTraceRPC()
        let model = NotchModel(rpc: rpc, preferences: UserDefaults(suiteName: UUID().uuidString)!)
        model.runningTasks = ["first": "First", "second": "Second"]
        let firstStarted = expectation(description: "First inspection started")
        rpc.onRequest = { _ in firstStarted.fulfill() }
        model.inspectTask(model.taskItems.first { $0.id == "first" }!)
        let firstJob = model.taskInspectionJob
        await fulfillment(of: [firstStarted], timeout: 2)
        let secondStarted = expectation(description: "Second inspection started")
        rpc.onRequest = { _ in secondStarted.fulfill() }
        model.inspectTask(model.taskItems.first { $0.id == "second" }!)
        await fulfillment(of: [secondStarted], timeout: 2)
        rpc.pending.removeValue(forKey: "second")?.resume(returning: ["selected": "second"])
        await model.taskInspectionJob?.value
        let current = try XCTUnwrap(model.taskRecordURL)
        rpc.pending.removeValue(forKey: "first")?.resume(throwing: DaemonRPCError.disconnected)
        await firstJob?.value
        XCTAssertEqual(model.taskRecordURL, current)
        XCTAssertNil(model.taskInspection)
        XCTAssertEqual(model.inspectedTaskID, "second")
        let trace = try JSONSerialization.jsonObject(with: Data(contentsOf: current)) as? [String: String]
        XCTAssertEqual(trace, ["selected": "second"])
        model.clearTaskInspection()
        XCTAssertNil(model.taskRecordURL)
        XCTAssertNil(model.inspectedTaskID)
    }
}

private final class DelayedTraceRPC: DaemonRPCTransport {
    var pending: [String: CheckedContinuation<[String: Any], Error>] = [:]
    var onRequest: ((String) -> Void)?
    func call(_ method: String, params: [String: Any]) async throws -> [String: Any] {
        try await withCheckedThrowingContinuation {
            let id = params["task_id"] as! String
            pending[id] = $0
            onRequest?(id)
        }
    }
    func stream(_ method: String, params: [String: Any]) -> AsyncThrowingStream<[String: Any], Error> {
        AsyncThrowingStream { $0.finish() }
    }
}
