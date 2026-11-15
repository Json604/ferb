import XCTest
@testable import FERBNotch

@MainActor
final class MicrophoneRecoveryTests: XCTestCase {
    // decision: APP-568
    func testReleaseOrCancelPreventsDelayedMicrophoneRestart() async throws {
        let recovery = MicrophoneRouteRecovery()
        var restarts = 0
        recovery.schedule { restarts += 1 }
        recovery.cancel()
        try await Task.sleep(for: .milliseconds(350))
        XCTAssertEqual(restarts, 0)
    }
}
