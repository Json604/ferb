import AppKit
import XCTest
@testable import FERBNotch

/// One detached activity window per running task (APP-898, owner 2026-11-12: "i cant open multiple live preview
/// extracted windows. i shdoul be able to of all the tasks running"). No window here is ordered front.
@MainActor
final class TaskActivityWindowsTests: XCTestCase {
    // decision: APP-898, APP-406
    func testEachRunningTaskGetsItsOwnWindowAndAgainBringsItsOwnForward() throws {
        let model = NotchModel(preferences: scratchDefaults("ActivityWindows"))
        model.runningTasks["t-1"] = "Amazon order"
        model.runningTasks["t-2"] = "Swiggy resumes"
        let ids = model.taskItems.filter(\.isRunning).map(\.id)
        XCTAssertEqual(ids.count, 2)
        var forward: [NSWindow] = []
        var released: [NSWindow] = []
        let windows = TaskActivityWindows(
            make: { taskID, close in TaskActivityPanel(model: model, taskID: taskID, onClose: close) },
            bringForward: { forward.append($0) }, release: { released.append($0) })

        windows.open(ids[0])
        windows.open(ids[1])
        let first = try XCTUnwrap(windows.window(for: ids[0]))
        let second = try XCTUnwrap(windows.window(for: ids[1]))
        XCTAssertFalse(first === second, "another task's activity opens a second window")
        XCTAssertTrue(released.isEmpty, "opening the second closes nothing")
        XCTAssertNotEqual(first.frame.origin, second.frame.origin, "the second doesn't cover the first exactly")
        for window in [first, second] {
            XCTAssertEqual(window.sharingType, .none, "every activity window is hidden from capture (APP-406)")
        }

        windows.open(ids[0])
        XCTAssertEqual(windows.windows.count, 2, "the same task's activity opens no new window")
        XCTAssertTrue(forward.last === first, "it brings that task's window forward")

        let close = try XCTUnwrap(first as? TaskActivityPanel)
        _ = close                                                    // its ✕ and its task ending both call onClose
        windows.close(ids[0])
        XCTAssertNil(windows.window(for: ids[0]))
        XCTAssertTrue(windows.window(for: ids[1]) === second, "closing one leaves the other open")
        XCTAssertTrue(released.count == 1 && released[0] === first)

        windows.closeAll()
        XCTAssertTrue(windows.windows.isEmpty)
    }

    // decision: APP-898
    func testATaskEndingClosesOnlyItsOwnWindow() async throws {
        let model = NotchModel(preferences: scratchDefaults("ActivityWindowsEnd"))
        model.runningTasks["t-1"] = "Amazon order"
        model.runningTasks["t-2"] = "Swiggy resumes"
        let ids = model.taskItems.filter(\.isRunning).map(\.id)
        let windows = TaskActivityWindows(
            make: { taskID, close in TaskActivityPanel(model: model, taskID: taskID, onClose: close) },
            bringForward: { $0.contentView?.layoutSubtreeIfNeeded() }, release: { _ in })
        for id in ids { windows.open(id) }
        let ending = try XCTUnwrap(model.taskItems.first { $0.id == ids[0] })
        model.runningTasks.removeValue(forKey: ending.backendID ?? ending.id)
        for _ in 0..<3 {
            for window in windows.windows.map(\.window) { window.contentView?.layoutSubtreeIfNeeded() }
            await withCheckedContinuation { continuation in DispatchQueue.main.async { continuation.resume() } }
        }
        XCTAssertNil(windows.window(for: ids[0]), "its task ended: its window closed")
        XCTAssertNotNil(windows.window(for: ids[1]), "the other task's window stays")
    }
}
