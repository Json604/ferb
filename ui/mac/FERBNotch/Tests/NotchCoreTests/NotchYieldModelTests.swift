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

    // decision: APP-559
    func testYieldingToAnAppNeedsAPinnedChatAndEndsWithThePinOrTheChat() {
        let model = makeModel()
        model.expand()
        model.beginYield(.activeAppUnderPinnedChat)
        XCTAssertFalse(model.isYielded)
        model.togglePin()
        model.beginYield(.activeAppUnderPinnedChat)
        XCTAssertTrue(model.isYielded)
        model.togglePin()
        XCTAssertFalse(model.isYielded)
        model.togglePin()
        model.beginYield(.activeAppUnderPinnedChat)
        model.collapse()
        XCTAssertFalse(model.isYielded)
    }

    // decision: APP-559
    func testYieldingLeavesDraftAttachmentsPinAndFoldUnchanged() {
        let model = makeModel()
        let file = URL(fileURLWithPath: "/tmp/brief.txt")
        model.expand()
        model.session.draft = "Keep my draft"
        model.session.attach([file])
        model.togglePin()
        model.revealPinnedBody()
        model.beginYield(.activeAppUnderPinnedChat)
        model.beginYield(.foreignOverlay)
        model.endYield(.foreignOverlay)
        model.endYield(.activeAppUnderPinnedChat)
        XCTAssertFalse(model.isYielded)
        XCTAssertEqual(model.session.draft, "Keep my draft")
        XCTAssertEqual(model.session.attachments, [file])
        XCTAssertTrue(model.isPinned)
        XCTAssertTrue(model.bodyVisible)
    }

    // decision: APP-559
    func testDictationBringsFERBBackAndKeepsOtherAppsFromHidingIt() {
        let model = makeModel(microphone: SilentMicrophone())
        model.expand()
        model.togglePin()
        var changes: [Bool] = []
        model.onYieldChange = { changes.append($0) }
        model.beginYield(.foreignOverlay)
        model.beginYield(.activeAppUnderPinnedChat)
        model.beginHeldVoice()
        XCTAssertEqual(model.session.phase, .listening)
        XCTAssertFalse(model.isYielded)
        XCTAssertEqual(changes, [true, false])
        model.beginYield(.foreignOverlay)
        model.beginYield(.activeAppUnderPinnedChat)
        XCTAssertFalse(model.isYielded)
    }

    // decision: APP-586
    func testAQuestionThatArrivesWhileYieldedKeepsItsLightButACompletionDoesNot() {
        let model = makeModel()
        model.expand()
        model.togglePin()
        model.beginYield(.activeAppUnderPinnedChat)
        model.showCompletionTrace()
        XCTAssertTrue(model.showsTraceGlow)
        XCTAssertFalse(model.tracerLightShows)
        model.announceAttention("test:question-while-yielded")
        XCTAssertTrue(model.isYielded)
        XCTAssertTrue(model.attentionTraceVisible)
        XCTAssertTrue(model.tracerLightShows)
        // The question is already shown in the chat body for when FERB comes back.
        XCTAssertTrue(model.bodyVisible)
        model.endYield(.activeAppUnderPinnedChat)
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
