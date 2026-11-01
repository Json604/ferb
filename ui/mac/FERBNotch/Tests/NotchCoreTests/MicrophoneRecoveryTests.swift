import XCTest
@testable import FERBNotch

@MainActor
final class MicrophoneRecoveryTests: XCTestCase {
    func testBluetoothConfigurationBurstRebuildsOnlyLatestRoute() async throws {
        let recovery = MicrophoneRouteRecovery()
        var rebuilt: [Int] = []
        recovery.schedule { rebuilt.append(1) }
        recovery.schedule { rebuilt.append(2) }
        try await Task.sleep(for: .milliseconds(350))
        XCTAssertEqual(rebuilt, [2])
    }

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
