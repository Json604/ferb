import AVFAudio
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// FERB's everyday sounds (notes/sound.md, owner 2026-11-01): each event's sound from the decided family, the owner's
/// per-sound switches, quiet during calls, and one sound per notification. Sounds go to fakes or are only decided,
/// never played out loud.
@MainActor
final class SoundFeedbackTests: XCTestCase {
    // MARK: - Which sound each event plays

    func testAFailedTaskPlaysErrorNotNeedsYou() {
        let (model, sounds) = makeModel()
        let a = begin("Rename the example files", in: model)
        model.receiveChatEvent(["type": "plan", "request_id": "req-a", "task_id": "task-a", "text": "Renaming them.",
                                "goal": "Rename"], localID: a)
        model.receiveChatEvent(["type": "outcome", "request_id": "req-a", "task_id": "task-a", "status": "failed",
                                "answer": "The folder wasn't there."], localID: a)
        XCTAssertEqual(sounds.played, [.error])

        let b = beginAnother("What's the weather?", in: model)
        model.receiveChatEvent(["type": "reply", "request_id": "req-b", "status": "failed",
                                "text": "The AI service didn't answer."], localID: b)
        XCTAssertEqual(sounds.played, [.error, .error])

        let c = beginAnother("And tomorrow?", in: model)
        model.receiveChatEvent(["type": "reply", "request_id": "req-c", "text": "Sunny."], localID: c)
        XCTAssertEqual(sounds.played, [.error, .error, .done])
    }

    func testAQuestionPlaysNeedsYou() {
        let (model, sounds) = makeModel()
        model.receiveBusEvent(["topic": "approval.requested", "data": [
            "approval_id": "q-1", "kind": "owner_question", "reason": "Which account should I post from?",
            "options": ["Work", "Personal"]] as [String: Any]])
        XCTAssertEqual(sounds.played, [.needsYou])

        // FERB's own question in reply: needs you, once, never a completion.
        let a = begin("Book the table", in: model)
        model.receiveChatEvent(["type": "clarify", "request_id": "req-a", "text": "For how many people?"], localID: a)
        XCTAssertEqual(sounds.played, [.needsYou, .needsYou])

        // A task that stopped to wait on the owner.
        let b = beginAnother("Post the update", in: model)
        model.receiveChatEvent(["type": "plan", "request_id": "req-b", "task_id": "task-b", "text": "Posting it.",
                                "goal": "Post"], localID: b)
        model.receiveChatEvent(["type": "outcome", "request_id": "req-b", "task_id": "task-b", "status": "needs_user",
                                "answer": "Which account, work or personal?"], localID: b)
        XCTAssertEqual(sounds.played, [.needsYou, .needsYou, .needsYou])
    }

    // MARK: - Switches and defaults

    func testSummonAndPausedAreSilentByDefault() {
        let engine = makeEngine(defaults: scratchDefaults("SoundFeedbackTests"))
        XCTAssertNil(engine.decision(for: .summon))
        XCTAssertNil(engine.decision(for: .paused))
        for sound: FerbSound in [.needsYou, .error, .done, .notification, .listeningStart, .listeningEnd,
                                 .firstLaunch, .intro, .orbDrip, .orbHop, .orbMelt] {
            XCTAssertEqual(engine.decision(for: sound), SoundDecision(subtle: false), "\(sound)")
        }
    }

    func testTurningASoundOffSilencesOnlyThatSound() {
        let defaults = scratchDefaults("SoundFeedbackTests")
        let feedback = SoundFeedback(defaults: defaults)
        let engine = makeEngine(defaults: defaults)
        feedback.set(.done, on: false)
        XCTAssertEqual(defaults.object(forKey: "feedback.sound.done") as? Bool, false)
        XCTAssertFalse(feedback.isOn(.done))
        XCTAssertNil(engine.decision(for: .done))
        for sound: FerbSound in [.needsYou, .error, .notification, .listeningStart, .listeningEnd, .intro] {
            XCTAssertNotNil(engine.decision(for: sound), "\(sound)")
        }
        feedback.set(.summon, on: true)                     // a sound off by default can be switched on
        XCTAssertNotNil(engine.decision(for: .summon))
        feedback.subtle = true                              // Settings' Standard/Subtle choice
        XCTAssertEqual(engine.decision(for: .needsYou), SoundDecision(subtle: true))
        feedback.enabled = false                            // the master switch silences every sound
        XCTAssertNil(engine.decision(for: .needsYou))
    }

    func testTheMasterSwitchAndVolumeAreKeptOnUpgradeAndTheMagneticSettingGoes() {
        let defaults = scratchDefaults("SoundFeedbackTests")
        defaults.set(false, forKey: "feedback.enabled")
        defaults.set(0.3, forKey: "feedback.volume")
        defaults.set(true, forKey: "feedback.magneticEnabled")
        let feedback = SoundFeedback(defaults: defaults)
        XCTAssertFalse(feedback.enabled)
        XCTAssertEqual(feedback.volume, 0.3)
        XCTAssertFalse(feedback.subtle)                     // Standard unless the owner chose Subtle
        XCTAssertNil(defaults.object(forKey: "feedback.magneticEnabled"))
    }

    // MARK: - Calls

    func testDuringACallOnlyNeedsYouPlaysAtSubtle() {
        let defaults = scratchDefaults("SoundFeedbackTests")
        var running: [pid_t] = [1]                          // another app (not FERB) runs microphone input
        let engine = makeEngine(defaults: defaults, input: { running })
        XCTAssertEqual(engine.decision(for: .needsYou), SoundDecision(subtle: true))
        for sound: FerbSound in [.done, .error, .notification, .listeningStart, .listeningEnd, .intro, .orbHop] {
            XCTAssertNil(engine.decision(for: sound), "\(sound)")
        }
        let feedback = SoundFeedback(defaults: defaults)
        feedback.player = engine
        XCTAssertFalse(feedback.bannerSounds(["n-call"]))  // a banner posts silently during a call

        running = [getpid()]                                // FERB's own dictation is never a call
        XCTAssertEqual(engine.decision(for: .needsYou), SoundDecision(subtle: false))
        XCTAssertEqual(engine.decision(for: .done), SoundDecision(subtle: false))
        running = []
        XCTAssertEqual(engine.decision(for: .notification), SoundDecision(subtle: false))
    }

    // MARK: - One sound per notification

    func testANoticeShownAsBannerAndInTheNotchSoundsOnce() async {
        let (model, sounds) = makeModel()
        model.feedback.noticeWait = .milliseconds(150)
        let center = FakeNotificationCenter()
        let poster = NotificationPoster(center: center, settle: .zero)
        poster.sounds = model.feedback
        var notchShowing = false
        poster.configure(notchShowing: { notchShowing }, open: { _ in })

        // The banner first (posted while FERB.app holds the subscription), then the same notice in the notch.
        let rpc = ScriptedRPC()
        rpc.events["notifications.subscribe"] = [loopBanner("n1")]
        _ = await poster.subscribeOnce(rpc: rpc)
        XCTAssertEqual(center.sounds, [true])
        model.receiveNotices([notice("n1")])
        await settle(model)
        XCTAssertFalse(sounds.played.contains(.notification))

        // The notice first: the notch waits for the banner, which carries the sound.
        model.feedback.postingBanners = true
        model.receiveNotices([notice("n2")])
        poster.receive(loopBanner("n2"))
        await settle(model)
        XCTAssertEqual(center.sounds, [true, true])
        XCTAssertFalse(sounds.played.contains(.notification))

        // The notch is open: the banner isn't posted, so the notch plays the notice's sound, once.
        notchShowing = true
        model.receiveNotices([notice("n3")])
        poster.receive(loopBanner("n3"))
        await settle(model)
        XCTAssertEqual(center.sounds, [true, true])
        XCTAssertEqual(sounds.played, [.notification])

        // A notice no banner shows sounds in the notch.
        model.receiveNotices([notice("n4")])
        await settle(model)
        XCTAssertEqual(sounds.played, [.notification, .notification])

        // The notch already sounded for a question: its banner posts silently.
        notchShowing = false
        model.receiveBusEvent(["topic": "approval.requested", "data": [
            "approval_id": "q-9", "kind": "owner_question", "reason": "Send it?"] as [String: Any]])
        poster.receive(["id": "question:q-9", "category": "question", "title": "FERB needs you", "body": "Send it?",
                        "target": ["kind": "question", "approval_id": "q-9"], "when_hidden": true])
        XCTAssertEqual(center.sounds, [true, true, false])
        XCTAssertEqual(sounds.played, [.notification, .notification, .needsYou])
        model.feedback.postingBanners = false
    }

    // MARK: - The banner's file

    func testTheBannerSoundIsAFinishedStereoMixAndNeverAnInAppVariant() throws {
        let url = try XCTUnwrap(FerbSoundFiles.bannerFile)
        let file = try AVAudioFile(forReading: url)
        XCTAssertEqual(file.processingFormat.channelCount, 2)
        XCTAssertEqual(file.processingFormat.sampleRate, 48_000)
        XCTAssertLessThan(Double(file.length) / 48_000, 30)            // macOS plays banner sounds under 30 s
        for subtle in [false, true] {
            XCTAssertFalse(FerbSoundFiles.files(for: .notification, subtle: subtle).contains(url))
        }
    }

    // MARK: - Helpers

    private func makeModel() -> (NotchModel, FakeSoundPlayer) {
        let model = NotchModel(rpc: ScriptedRPC(), preferences: scratchDefaults("SoundFeedbackTests"))
        let sounds = FakeSoundPlayer()
        model.feedback.player = sounds
        return (model, sounds)
    }

    private func makeEngine(defaults: UserDefaults, input: @escaping @MainActor () -> [pid_t] = { [] }) -> FerbSoundEngine {
        FerbSoundEngine(defaults: defaults, preference: { _ in nil }, headphones: { false }, screens: { [] },
                        measureLatency: { 0 }, inputProcesses: input)
    }

    private func begin(_ request: String, in model: NotchModel) -> UUID {
        model.session.draft = request
        let id = model.session.beginWork()!
        model.conversation.begin(id: id, request: request, sources: [])
        return id
    }

    private func beginAnother(_ request: String, in model: NotchModel) -> UUID {
        if let active = model.session.activeTaskID { model.workSessions[active] = model.session }
        model.session = NotchSession()
        return begin(request, in: model)
    }

    private func notice(_ id: String) -> [String: Any] {
        ["id": id, "text": "The example loop paused.", "at": "2026-11-01T10:00:00Z"]
    }

    private func loopBanner(_ notice: String) -> [String: Any] {
        ["id": "loop:" + notice, "category": "loop", "title": "FERB loop", "body": "The example loop paused.",
         "target": ["kind": "task", "task_id": "tree-" + notice], "when_hidden": true]
    }

    /// Past the notch's wait for a banner.
    private func settle(_ model: NotchModel) async {
        try? await Task.sleep(for: .milliseconds(300))
        for _ in 0..<5 { await Task.yield() }
    }
}
