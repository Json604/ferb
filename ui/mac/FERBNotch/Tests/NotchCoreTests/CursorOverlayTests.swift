import XCTest
@testable import FERBNotch
@testable import NotchCore

@MainActor
final class CursorOverlayTests: XCTestCase {
    func testAPointShowsAPointerAtTheElement() {
        let c = CursorController(clock: { Date(timeIntervalSince1970: 100) }, makePanels: false)
        c.point(taskID: "t1", kind: "click", rect: CGRect(x: 100, y: 200, width: 40, height: 20), display: 1)
        XCTAssertEqual(c.pointers["t1"]?.target, CGPoint(x: 120, y: 210))
    }

    func testEachTaskHasItsOwnPointerAndEndsWithItsTask() {
        let c = CursorController(clock: { Date(timeIntervalSince1970: 100) }, makePanels: false)
        c.point(taskID: "t1", kind: "click", rect: CGRect(x: 0, y: 0, width: 10, height: 10), display: 1)
        c.point(taskID: "t2", kind: "type", rect: CGRect(x: 50, y: 50, width: 100, height: 20), display: 1)
        XCTAssertEqual(Set(c.pointers.keys), ["t1", "t2"])
        XCTAssertNotEqual(c.pointers["t1"]?.hue, c.pointers["t2"]?.hue)
        XCTAssertEqual(c.pointers["t2"]?.target, CGPoint(x: 62, y: 60))   // type: the field's leading edge
        c.end(taskID: "t1")
        XCTAssertEqual(Set(c.pointers.keys), ["t2"])
    }

    func testAPointerFadesAfterIdle() {
        var now = Date(timeIntervalSince1970: 100)
        let c = CursorController(clock: { now }, makePanels: false)
        c.point(taskID: "t1", kind: "click", rect: CGRect(x: 0, y: 0, width: 10, height: 10), display: 1)
        now = now.addingTimeInterval(1.6)
        c.sweep()
        XCTAssertNil(c.pointers["t1"])
    }

    func testTheSwitchOffShowsNothing() {
        let c = CursorController(clock: { Date() }, makePanels: false)
        c.enabled = false
        c.point(taskID: "t1", kind: "click", rect: CGRect(x: 0, y: 0, width: 10, height: 10), display: 1)
        XCTAssertTrue(c.pointers.isEmpty)
    }

    func testTheOverlayPanelNeverTakesTheMouseOrFocus() {
        let panel = CursorPanel(screenFrame: CGRect(x: 0, y: 0, width: 800, height: 600))
        XCTAssertTrue(panel.ignoresMouseEvents)
        XCTAssertFalse(panel.canBecomeKey)
        XCTAssertFalse(panel.canBecomeMain)
        XCTAssertTrue(panel.collectionBehavior.contains(.canJoinAllSpaces))
        XCTAssertFalse(panel.hidesOnDeactivate)
        XCTAssertEqual(panel.level.rawValue, NSWindow.Level.statusBar.rawValue - 1)
        XCTAssertTrue(panel.collectionBehavior.isSuperset(of: [.fullScreenAuxiliary, .stationary, .ignoresCycle]))
    }

    func testLocalPointSubtractsTheDisplayOrigin() {
        XCTAssertEqual(CursorController.localPoint(target: CGPoint(x: 100, y: 50), displayOrigin: CGPoint(x: -1440, y: 0)),
                       CGPoint(x: 1540, y: 50))
        XCTAssertEqual(CursorController.localPoint(target: CGPoint(x: 100, y: -300), displayOrigin: CGPoint(x: 0, y: -900)),
                       CGPoint(x: 100, y: 600))
    }

    func testABusEventReachesTheController() {
        let model = NotchModel(rpc: QuietCursorRPC(), preferences: UserDefaults(suiteName: UUID().uuidString)!)
        let c = CursorController(clock: { Date() }, makePanels: false)
        model.cursor = c
        model.receiveBusEvent(["topic": "cursor.point", "data": ["task_id": "t9", "kind": "click",
                               "rect": [10.0, 10.0, 20.0, 20.0], "display": 1] as [String: Any]])
        XCTAssertNotNil(c.pointers["t9"])
        model.receiveBusEvent(["topic": "task.result", "data": ["task_id": "t9"] as [String: Any]])
        XCTAssertNil(c.pointers["t9"])
    }
}

private final class QuietCursorRPC: DaemonRPCTransport {
    func call(_ method: String, params: [String: Any]) async throws -> [String: Any] { [:] }
    func stream(_ method: String, params: [String: Any]) -> AsyncThrowingStream<[String: Any], Error> {
        AsyncThrowingStream { _ in }
    }
}
