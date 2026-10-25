import XCTest
@testable import NotchCore

/// D31 fix round 4 (H1, N1): the notch tells the daemon its click-through windows by number, so the daemon's click
/// check skips exactly them: each change once, every source's windows together, and again after a reconnection.
@MainActor
final class ClickThroughReportTests: XCTestCase {
    func testEachChangeIsToldOnceWithEveryWindowTogether() async {
        let rpc = ClickThroughRPC()
        let report = ClickThroughReport(rpc: rpc)
        report.set("cursor", windows: [300])
        await report.settled()
        report.set("cursor", windows: [300])                    // unchanged: not told again
        await report.settled()
        report.set("tracer", windows: [301, 0])                 // a window with no number yet is left out
        await report.settled()
        report.set("cursor", windows: [])                       // the overlay closed, then a new one opened:
        report.set("cursor", windows: [302])                    // what goes out is the windows as they are then
        await report.settled()
        XCTAssertEqual(rpc.told, [[300], [300, 301], [301, 302]])
    }

    func testAReconnectionTellsAgainAndAFailedSendIsTriedAgain() async {
        let rpc = ClickThroughRPC()
        let report = ClickThroughReport(rpc: rpc)
        report.reconnected()                                    // nothing to tell yet
        await report.settled()
        XCTAssertEqual(rpc.told, [])
        report.set("tracer", windows: [301])
        await report.settled()
        report.reconnected()                                    // the daemon may have restarted and forgotten
        await report.settled()
        XCTAssertEqual(rpc.told, [[301], [301]])
        rpc.fails = true
        report.set("tracer", windows: [])
        await report.settled()
        rpc.fails = false
        report.reconnected()                                    // the daemon's copy is unknown: told again
        await report.settled()
        XCTAssertEqual(rpc.told, [[301], [301], [], []])
    }

    /// A send the daemon refused (it didn't list the just-launched notch yet) goes again on the next pass; a known copy
    /// isn't sent again.
    func testARefusedSendGoesAgainOnTheNextPass() async {
        let rpc = ClickThroughRPC()
        let report = ClickThroughReport(rpc: rpc)
        rpc.fails = true
        report.set("tracer", windows: [301])
        await report.settled()
        rpc.fails = false
        report.retry()
        await report.settled()
        report.retry()
        await report.settled()
        XCTAssertEqual(rpc.told, [[301], [301]])
    }
}

private final class ClickThroughRPC: DaemonRPCTransport, @unchecked Sendable {
    private let recorded = RecordedCalls()
    var fails = false
    var told: [[Int]] {
        recorded.all.filter { $0.0 == "ui.click_through" }.compactMap { $0.1["windows"] as? [Int] }
    }
    func call(_ method: String, params: [String: Any]) async throws -> [String: Any] {
        recorded.append(method, params)
        if fails { throw DaemonRPCError.unavailable }
        return ["windows": (params["windows"] as? [Int])?.count ?? 0]
    }
    func stream(_ method: String, params: [String: Any]) -> AsyncThrowingStream<[String: Any], Error> {
        AsyncThrowingStream { $0.finish() }
    }
}
