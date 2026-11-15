import XCTest
@testable import FERBNotch
import NotchCore

@MainActor
final class NotchYieldModelTests: XCTestCase {
    private func makeModel(microphone: VoiceCapturing? = nil) -> NotchModel {
        NotchModel(preferences: scratchDefaults(), microphone: microphone)
    }

    // decision: APP-559
    func testReasonsOverlapAndOnlyTheLastOneEndingBringsFERBBack() {
        let model = makeModel()
        model.expand()
        var changes: [Bool] = []
        model.onYieldChange = { changes.append($0) }
        model.setFilePickerOpen(true)
        model.beginYield(.foreignOverlay)
        model.setFilePickerOpen(false)
        XCTAssertEqual(changes, [true])
        XCTAssertTrue(model.isYielded)
        model.endYield(.foreignOverlay)
        XCTAssertEqual(changes, [true, false])
        XCTAssertFalse(model.isYielded)
    }

    // decision: APP-559, APP-642, APP-743, APP-744
    func testYieldingToAnAppNeedsTheToursLineAndEndsWithTheLineOrTheChat() {
        let model = makeModel()
        model.expand()
        model.beginYield(.activeAppUnderTourLine)
        XCTAssertFalse(model.isYielded, "an open chat never steps behind an app (no pin since APP-743)")
        model.introLine = IntroLine(text: "Turn FERB on in this list.")
        model.beginYield(.activeAppUnderTourLine)
        XCTAssertTrue(model.isYielded)
        model.introLine = nil
        XCTAssertFalse(model.isYielded)
        model.introLine = IntroLine(text: "Turn FERB on in this list.")
        model.beginYield(.activeAppUnderTourLine)
        model.collapse()
        XCTAssertFalse(model.isYielded)
    }

    // decision: APP-559
    func testYieldingLeavesDraftAndAttachmentsUnchanged() {
        let model = makeModel()
        let file = URL(fileURLWithPath: "/tmp/brief.txt")
        model.expand()
        model.session.draft = "Keep my draft"
        model.session.attach([file])
        model.beginYield(.filePicker)
        model.beginYield(.foreignOverlay)
        model.endYield(.foreignOverlay)
        model.endYield(.filePicker)
        XCTAssertFalse(model.isYielded)
        XCTAssertEqual(model.session.draft, "Keep my draft")
        XCTAssertEqual(model.session.attachments, [file])
        XCTAssertTrue(model.bodyVisible)
    }

    // decision: APP-559
    func testDictationBringsFERBBackAndKeepsOtherAppsFromHidingIt() {
        let model = makeModel(microphone: SilentMicrophone())
        model.expand()
        model.introLine = IntroLine(text: "Turn FERB on in this list.")
        var changes: [Bool] = []
        model.onYieldChange = { changes.append($0) }
        model.beginYield(.foreignOverlay)
        model.beginYield(.activeAppUnderTourLine)
        model.beginHeldVoice()
        XCTAssertEqual(model.session.phase, .listening)
        XCTAssertFalse(model.isYielded)
        XCTAssertEqual(changes, [true, false])
        model.beginYield(.foreignOverlay)
        model.beginYield(.activeAppUnderTourLine)
        XCTAssertFalse(model.isYielded)
    }

    // decision: APP-586
    func testAQuestionThatArrivesWhileYieldedKeepsItsLightButACompletionDoesNot() {
        let model = makeModel()
        model.expand()
        model.beginYield(.foreignOverlay)
        model.showCompletionTrace()
        XCTAssertTrue(model.showsTraceGlow)
        XCTAssertFalse(model.tracerLightShows)
        model.announceAttention("test:question-while-yielded")
        XCTAssertTrue(model.isYielded)
        XCTAssertTrue(model.attentionTraceVisible)
        XCTAssertTrue(model.tracerLightShows)
        // The question is already shown in the chat body for when FERB comes back.
        XCTAssertTrue(model.bodyVisible)
        model.endYield(.foreignOverlay)
        XCTAssertTrue(model.tracerLightShows)
    }
}

@MainActor
private final class SilentMicrophone: VoiceCapturing {
    func start(update: @escaping (String) -> Void, level: @escaping ([Double], TimeInterval) -> Void,
               completion: @escaping (Result<String, Error>) -> Void) {}
    func finish() {}
    func cancel() {}
}
