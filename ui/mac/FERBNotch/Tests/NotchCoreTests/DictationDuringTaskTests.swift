import XCTest
@testable import NotchCore

/// Dictation while a task runs (APP-781, owner 2026-11-07: "it opens listens for a seconds or 2 adn shuts down"): the
/// status refresh brought the running task into the dictation composer, which ended listening.
final class DictationDuringTaskTests: XCTestCase {
    // decision: APP-781
    func testARunningTaskIsNeverBroughtIntoTheListeningComposer() {
        var composer = NotchSession()
        composer.draft = "remind me"
        composer.beginVoicePreview()
        composer.resumeBackendTask("t-running", goal: "Swiggy tailored resumes")
        XCTAssertEqual(composer.phase, .listening)
        XCTAssertNil(composer.backendTaskID)
        XCTAssertNil(composer.activeTaskID)
        composer.finishVoicePreview()                        // dictation ended: the task shows again
        composer.resumeBackendTask("t-running", goal: "Swiggy tailored resumes")
        XCTAssertEqual(composer.backendTaskID, "t-running")
        XCTAssertEqual(composer.phase, .working)
        XCTAssertEqual(composer.draft, "remind me")          // what was dictated stays in the box
    }
}
