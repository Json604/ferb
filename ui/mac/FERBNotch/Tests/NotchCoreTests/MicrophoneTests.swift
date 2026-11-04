import XCTest
@testable import FERBNotch

@MainActor
final class MicrophoneTests: XCTestCase {
    // decision: APP-568
    func testHeldVoiceReturnsRealTranscriptToDraftWithoutSubmitting() {
        let mic = FakeMicrophone()
        let model = NotchModel(preferences: scratchDefaults(), microphone: mic)
        model.session.draft = "Please"
        model.beginHeldVoice()
        XCTAssertEqual(model.session.phase, .listening)
        mic.update?("find my notes")
        XCTAssertEqual(model.voiceTranscript, "find my notes")
        model.finishHeldVoice()
        XCTAssertTrue(mic.finished)
        mic.completion?(.success("find my notes"))
        XCTAssertEqual(model.session.draft, "Please find my notes")
        XCTAssertEqual(model.session.phase, .draft)
        XCTAssertNil(model.session.activeTaskID)
    }

    // decision: APP-568, APP-552
    func testCancelStopsRecorderAndRestoresSnapshot() {
        let mic = FakeMicrophone()
        let model = NotchModel(preferences: scratchDefaults(), microphone: mic)
        model.session.draft = "Keep me"
        model.beginHeldVoice()
        mic.update?("discard this")
        model.cancelHeldVoice()
        XCTAssertTrue(mic.cancelled)
        XCTAssertEqual(model.session.draft, "Keep me")
        XCTAssertFalse(model.expanded)
    }

    // decision: APP-568
    func testPermissionFailurePreservesDraftAndExplainsRecovery() {
        let mic = FakeMicrophone()
        let model = NotchModel(preferences: scratchDefaults(), microphone: mic)
        model.session.draft = "Keep me"
        model.startDictation()
        mic.completion?(.failure(NSError(domain: "test", code: 1, userInfo: [NSLocalizedDescriptionKey: "Allow microphone in Settings"])))
        XCTAssertEqual(model.session.draft, "Keep me")
        XCTAssertEqual(model.promptError, "Allow microphone in Settings")
        XCTAssertTrue(model.canCompose)
    }
}

@MainActor
private final class FakeMicrophone: VoiceCapturing {
    var update: ((String) -> Void)?
    var completion: ((Result<String, Error>) -> Void)?
    var finished = false
    var cancelled = false
    var level: (([Double], TimeInterval) -> Void)?
    func start(update: @escaping (String) -> Void, level: @escaping ([Double], TimeInterval) -> Void,
               completion: @escaping (Result<String, Error>) -> Void) {
        self.update = update; self.level = level; self.completion = completion
    }
    func finish() { finished = true }
    func cancel() { cancelled = true }
}
