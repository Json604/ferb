import XCTest
@testable import FERBNotch

/// Final review M5 (Task 20 / L26 ruling): the notch's dictation is recognized on this Mac only; raw voice never goes
/// to Apple's servers. Without on-device support FERB says so instead of listening.
final class OnDeviceSpeechTests: XCTestCase {
    func testNoRequestWithoutOnDeviceRecognition() {
        XCTAssertNil(OnDeviceSpeech.request(supportsOnDevice: false))
    }

    func testEveryRequestRequiresOnDeviceRecognition() throws {
        let request = try XCTUnwrap(OnDeviceSpeech.request(supportsOnDevice: true))
        XCTAssertTrue(request.requiresOnDeviceRecognition)
        XCTAssertTrue(request.shouldReportPartialResults)
    }

    func testTheRefusalSaysWhyAndOffersTyping() {
        XCTAssertTrue(OnDeviceSpeech.unavailable.contains("on this Mac"))
        XCTAssertTrue(OnDeviceSpeech.unavailable.contains("type"))
    }

    func testTheUsageStringNeverMentionsAppleServers() throws {
        let plist = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().appendingPathComponent("Info.plist")
        let info = try XCTUnwrap(NSDictionary(contentsOf: plist))
        let usage = try XCTUnwrap(info["NSSpeechRecognitionUsageDescription"] as? String)
        XCTAssertFalse(usage.contains("server"))
        XCTAssertTrue(usage.contains("on this Mac"))
    }
}
