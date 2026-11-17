import FerbAppModel
import FerbCore
import FerbMobile
import XCTest

/// A recogniser driven by the test: what it was asked, and the events it gives.
@MainActor
final class FakeVoice: VoiceInput {
    var offers: SpeechKind
    var tasks: [(SpeechKind, Int)] = []
    var events: (@MainActor (VoiceEvent) -> Void)?
    var finished = 0
    var stopped = 0
    init(_ offers: SpeechKind = .onDevice) { self.offers = offers }
    func kind() -> SpeechKind { offers }
    func startTask(kind: SpeechKind, task: Int, events: @escaping @MainActor (VoiceEvent) -> Void) { tasks.append((kind, task)); self.events = events }
    func finishAudio() { finished += 1 }
    func stop() { stopped += 1; events = nil }
    func heard(_ text: String, final: Bool = false) { events?(.heard(task: tasks.last!.1, RecognizedSpeech(text, isFinal: final))) }
    func failed(noSpeech: Bool = false, languageMissing: Bool = false) {
        events?(.failed(task: tasks.last!.1, noSpeech: noSpeech, languageMissing: languageMissing))
    }
}

/// FERB.app's dictation model on the phone: every sequence of Android's shared table
/// (apps/android/app/src/test/resources/dictation-cases.json) through the Swift port.
final class DictationTests: XCTestCase {
    // decision: PHN-605, APP-673, APP-675
    func testEverySharedSequenceKeepsEveryWordUntilTheOwnerLetsGo() throws {
        let url = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("android/app/src/test/resources/dictation-cases.json")
        let table = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any])
        let cases = try XCTUnwrap(table["cases"] as? [[String: Any]])
        XCTAssertGreaterThanOrEqual(cases.count, 8)
        for c in cases {
            let name = c["name"] as? String ?? "?"
            var capture = DictationCapture()
            for (j, s) in (c["steps"] as? [[String: Any]] ?? []).enumerated() {
                var step: DictationCapture.Step?
                if let begin = s["begin"] as? Int { XCTAssertEqual(capture.beginTask(), begin, name) }
                else if let heard = s["heard"] as? String {
                    step = capture.receive(RecognizedSpeech(heard, isFinal: s["final"] as? Bool ?? false, utteranceEnded: s["ended"] as? Bool ?? false,
                                                            start: s["start"] as? Double, end: s["end"] as? Double), task: s["task"] as! Int)
                } else if let task = s["fail"] as? Int { step = capture.fail(task: task, noSpeech: s["noSpeech"] as! Bool) }
                else if s["leave"] != nil { capture.leaveTask() }
                else if s["finish"] != nil { step = capture.finish() }
                else if let text = s["text"] as? String { XCTAssertEqual(capture.text, text, name) }
                else { XCTFail("unknown step in \(name): \(s)") }
                if let want = s["step"] as? String { XCTAssertEqual(step.map(Self.words), want, "\(name), step \(j)") }
            }
        }
    }

    static func words(_ step: DictationCapture.Step) -> String {
        switch step {
        case .keepListening: "keep"
        case .restart: "restart"
        case .failed: "failed"
        case .finished(let text): "finished:\(text)"
        }
    }
}

/// Speaking to FERB from the phone (kb PHN-603..PHN-605): told first, asked in context, held, never sent by itself
/// (Android's VoiceTest).
@MainActor
final class VoiceTests: XCTestCase {
    private var rig: Rig!
    private var voice: FakeVoice!
    private var mic = false

    override func setUp() async throws {
        rig = Rig()
        voice = FakeVoice()
        rig.session.unlocked()
        rig.session.foreground(true)
    }

    override func tearDown() async throws { rig.tearDown() }

    private func model(_ input: VoiceInput? = nil) async -> AppModel {
        let c = rig.clock
        let m = AppModel(api: rig.phone, session: rig.session, prefs: rig.prefs, local: rig.local, screenLock: { true }, corpus: nil,
                         now: { Date(timeIntervalSince1970: c.t) }, voice: input ?? voice, micAllowed: { [weak self] in self?.mic ?? false })
        m.prompts = rig.prompts
        await eventually("main") { m.ui.gate == .main }
        return m
    }

    // decision: PHN-603, PHN-604
    func testTheFirstPressSaysWhatHappensToTheVoiceThenIOSAsksThenItListens() async {
        let vm = await model()
        XCTAssertEqual(vm.voicePress(), .done)
        XCTAssertEqual(vm.sheet, .voice(.onDevice))
        XCTAssertTrue(voice.tasks.isEmpty, "nothing listens before the owner has read it")
        vm.voiceTold(.onDevice)
        XCTAssertNil(vm.sheet)
        XCTAssertEqual(vm.voicePress(), .askMicrophone, "iOS's questions come next")
        XCTAssertTrue(voice.tasks.isEmpty)
        mic = true
        vm.microphoneAnswered(true)
        XCTAssertEqual(vm.voicePress(), .done)
        XCTAssertEqual(voice.tasks.map(\.1), [0])
        XCTAssertEqual(voice.tasks.first?.0, .onDevice)
        XCTAssertTrue(vm.listening)
    }

    // decision: PHN-603, PHN-605, APP-675
    func testHeldWordsLandAfterTheDraftAndStayAndNothingIsSent() async {
        rig.prefs.voiceTold = [SpeechKind.onDevice.rawValue]; mic = true
        let vm = await model()
        vm.typed("Remind me")
        vm.voicePress()
        voice.heard("to water")
        XCTAssertEqual(vm.draft, "Remind me to water")
        voice.heard("to water the plants.", final: true)
        XCTAssertEqual(voice.tasks.count, 2, "a new task takes the microphone")
        rig.clock.t += 4
        voice.heard("At six")
        vm.voiceRelease()
        XCTAssertEqual(voice.finished, 1)
        XCTAssertTrue(vm.listening, "still waiting for the last words")
        voice.heard("At six.", final: true)
        XCTAssertFalse(vm.listening)
        XCTAssertEqual(vm.draft, "Remind me to water the plants. At six.")
        await settle()
        XCTAssertFalse(rig.phone.calls.contains { $0.hasPrefix("send") }, "never sent by itself")
    }

    // decision: PHN-603, PHN-605
    func testWithoutALastAnswerWhatWasSaidStaysAfterTheGrace() async {
        rig.prefs.voiceTold = [SpeechKind.onDevice.rawValue]; mic = true
        let vm = await model()
        vm.voicePress()
        voice.heard("Book the train")
        rig.clock.t += 2
        vm.voiceRelease()
        XCTAssertTrue(vm.listening)
        await eventually("the grace", timeout: 5) { !vm.listening }
        XCTAssertEqual(vm.draft, "Book the train")
        XCTAssertGreaterThanOrEqual(voice.stopped, 1)
    }

    // decision: PHN-603, PHN-605
    func testAQuickTapSaysToHoldAndAFailureLeavesTheDraft() async {
        rig.prefs.voiceTold = [SpeechKind.onDevice.rawValue]; mic = true
        let vm = await model()
        vm.typed("Hi")
        vm.voicePress()
        rig.clock.t += 0.1
        vm.voiceRelease()
        XCTAssertEqual(vm.ui.note, Note("voice_hold"))
        XCTAssertFalse(vm.listening)
        XCTAssertEqual(vm.draft, "Hi")
        vm.voicePress()
        voice.failed()
        XCTAssertFalse(vm.listening)
        XCTAssertEqual(vm.draft, "Hi")
        XCTAssertEqual(vm.ui.note, Note("voice_failed"))
    }

    // decision: PHN-604
    func testNoOnDeviceSpeechForTheLanguageTellsTheOwnerBeforeApplesServiceIsUsed() async {
        rig.prefs.voiceTold = [SpeechKind.onDevice.rawValue]; mic = true
        let vm = await model()
        vm.voicePress()
        voice.offers = .service
        voice.failed(languageMissing: true)
        XCTAssertEqual(vm.sheet, .voice(.service))
        vm.voiceTold(.service)
        vm.voicePress()
        XCTAssertEqual(voice.tasks.last?.0, .service)
    }

    // decision: PHN-603, PHN-605, PHN-15
    func testLeavingTheAppStopsTheMicrophoneAndKeepsTheWords() async {
        rig.prefs.voiceTold = [SpeechKind.onDevice.rawValue]; mic = true
        let vm = await model()
        vm.voicePress()
        voice.heard("Call the bank")
        rig.session.foreground(false)
        await eventually("stopped") { !vm.listening }
        XCTAssertGreaterThanOrEqual(voice.stopped, 1)
        XCTAssertEqual(vm.draft, "Call the bank")
    }

    // decision: PHN-603, PHN-605
    func testAPhoneWithoutSpeechRecognitionSaysSo() async {
        let vm = await model(FakeVoice(.none))
        vm.voicePress()
        XCTAssertEqual(vm.ui.note, Note("voice_unavailable"))
        XCTAssertNil(vm.sheet)
    }

    /// Where a file sent from the phone landed: the Mac's words once the library carries them, else the default place.
    // decision: PHN-608
    func testTheDefaultPlaceForFilesFromThePhoneIsAndroidsWords() {
        XCTAssertEqual(L.s("tr_place_default"), "Downloads › FERB from phone")
        XCTAssertNil(UploadPlace.shown(Uploaded(uploadId: "u-1", saved: "/Users/someone/Downloads/FERB from phone/u-1-a.pdf", size: 3, savedIn: nil)))
        XCTAssertEqual(UploadPlace.shown(Uploaded(uploadId: "u-1", saved: "/Users/someone/Documents/From my phone/u-1-a.pdf", size: 3,
                                                  savedIn: "Documents › From my phone")), "Documents › From my phone")
        XCTAssertEqual(L.s("tr_place", L.s("tr_place_default")), "Saved in Downloads › FERB from phone")
    }
}
