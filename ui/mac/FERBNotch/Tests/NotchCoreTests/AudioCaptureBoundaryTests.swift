import AppKit
import AudioCapture
import AudioCaptureTestSupport
import XCTest

@MainActor
final class AudioCaptureBoundaryTests: XCTestCase {
    func testTapUsesChangedHardwareFormatInsteadOfStaleOutputFormat() {
        let engine = FERBTestAudioEngine()
        XCTAssertNil(FERBInstallMicrophoneTap(engine) { _, _ in })
        XCTAssertEqual(engine.installedSampleRate, 16_000)
        XCTAssertEqual(engine.outputFormatReads, 0)
        XCTAssertNil(FERBStopMicrophoneEngine(engine, true))
        XCTAssertEqual(engine.removedTaps, 1)
    }

    func testAudioExceptionsBecomeErrorsWithoutUnwindingSwiftTask() async {
        for operation in ["input", "format", "install", "prepare", "start", "reset", "stop", "remove"] {
            let engine = FERBTestAudioEngine()
            engine.failingOperation = operation
            let error: Error?
            switch operation {
            case "input", "format", "install": error = FERBInstallMicrophoneTap(engine) { _, _ in }
            case "prepare", "start": error = FERBStartMicrophoneEngine(engine)
            case "reset": error = FERBResetMicrophoneEngine(engine)
            default: error = FERBStopMicrophoneEngine(engine, true)
            }
            XCTAssertEqual((error as NSError?)?.domain, "FERBAudioCapture", operation)
            XCTAssertFalse(error?.localizedDescription.contains("fixture-private-reason") ?? true)
            // An exception must never pass through this task's executor frame.
            await Task.yield()
            MainActor.assertIsolated()
        }
    }

    func testFailedTapIsRemovedAndNextCaptureCanStart() {
        let engine = FERBTestAudioEngine()
        engine.failingOperation = "install"
        XCTAssertNotNil(FERBInstallMicrophoneTap(engine) { _, _ in })
        XCTAssertEqual(engine.removedTaps, 1)
        engine.failingOperation = ""
        XCTAssertNil(FERBInstallMicrophoneTap(engine) { _, _ in })
        XCTAssertNil(FERBStartMicrophoneEngine(engine))
    }

    func testStartNSErrorIsPreservedAndStopStillRemovesTapWhenStopThrows() {
        let engine = FERBTestAudioEngine()
        engine.failingOperation = "start-error"
        XCTAssertEqual((FERBStartMicrophoneEngine(engine) as NSError?)?.domain, "FixtureStartError")
        engine.failingOperation = "stop"
        XCTAssertNotNil(FERBStopMicrophoneEngine(engine, true))
        XCTAssertEqual(engine.removedTaps, 1)
    }
}
