import AppKit
import AVFAudio
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// FERB's everyday sounds (kb area app, owner 2026-11-01): each event's sound from the decided family, the owner's
/// per-sound switches, quiet during calls, and one sound per notification. Sounds go to fakes or are only decided,
/// never played out loud.
@MainActor
final class SoundFeedbackTests: XCTestCase {
    // MARK: - Which sound each event plays

    // decision: APP-651
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
        XCTAssertEqual(sounds.played, [.error, .error])     // a finished answer plays no sound (APP-651)
    }

    // decision: APP-651, APP-533
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

    // decision: APP-651
    func testSummonAndPausedAreSilentByDefault() {
        let engine = makeEngine(defaults: scratchDefaults("SoundFeedbackTests"))
        XCTAssertNil(engine.decision(for: .summon))
        XCTAssertNil(engine.decision(for: .paused))
        for sound: FerbSound in [.needsYou, .error, .notification, .listeningStart, .listeningEnd,
                                 .firstLaunch, .intro, .orbDrip, .orbHop, .orbMelt] {
            XCTAssertEqual(engine.decision(for: sound), SoundDecision(subtle: false), "\(sound)")
        }
    }

    // decision: APP-651
    func testTurningASoundOffSilencesOnlyThatSound() {
        let defaults = scratchDefaults("SoundFeedbackTests")
        let feedback = SoundFeedback(defaults: defaults)
        let engine = makeEngine(defaults: defaults)
        feedback.set(.notification, on: false)
        XCTAssertEqual(defaults.object(forKey: "feedback.sound.notification") as? Bool, false)
        XCTAssertFalse(feedback.isOn(.notification))
        XCTAssertNil(engine.decision(for: .notification))
        for sound: FerbSound in [.needsYou, .error, .listeningStart, .listeningEnd, .intro] {
            XCTAssertNotNil(engine.decision(for: sound), "\(sound)")
        }
        feedback.set(.summon, on: true)                     // a sound off by default can be switched on
        XCTAssertNotNil(engine.decision(for: .summon))
        feedback.subtle = true                              // Settings' Standard/Subtle choice
        XCTAssertEqual(engine.decision(for: .needsYou), SoundDecision(subtle: true))
        feedback.enabled = false                            // the master switch silences every sound
        XCTAssertNil(engine.decision(for: .needsYou))
    }

    // decision: APP-651, APP-527
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

    // decision: APP-528
    func testDuringACallOnlyNeedsYouPlaysAtSubtle() {
        let defaults = scratchDefaults("SoundFeedbackTests")
        var running: [pid_t] = [1]                          // another app (not FERB) runs microphone input
        let engine = makeEngine(defaults: defaults, input: { running })
        XCTAssertEqual(engine.decision(for: .needsYou), SoundDecision(subtle: true))
        for sound: FerbSound in [.error, .notification, .listeningStart, .listeningEnd, .intro, .orbHop] {
            XCTAssertNil(engine.decision(for: sound), "\(sound)")
        }
        let feedback = SoundFeedback(defaults: defaults)
        feedback.player = engine
        XCTAssertFalse(feedback.bannerSounds(["n-call"]))  // a banner posts silently during a call

        running = [getpid()]                                // FERB's own dictation is never a call
        XCTAssertEqual(engine.decision(for: .needsYou), SoundDecision(subtle: false))
        XCTAssertEqual(engine.decision(for: .error), SoundDecision(subtle: false))
        running = []
        XCTAssertEqual(engine.decision(for: .notification), SoundDecision(subtle: false))
    }

    // MARK: - One sound per notification

    // decision: APP-532, APP-533, APP-622
    func testANoticeShownAsBannerAndInTheNotchSoundsOnce() async {
        let (model, sounds) = makeModel()
        let wait = ManualNoticeWait()
        model.feedback.sleep = { try await wait.sleep($0) }
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
        XCTAssertFalse(sounds.played.contains(.notification))
        XCTAssertEqual(wait.started, 0)                      // carried by its banner: nothing waits

        // The notice first: the notch waits for the banner, which carries the sound; the wait then running out
        // plays nothing.
        model.feedback.postingBanners = true
        model.receiveNotices([notice("n2", bannered: true)])
        poster.receive(loopBanner("n2"))
        await until { wait.started == 1 }
        wait.expire()
        await until { wait.ended == 1 }
        XCTAssertEqual(center.sounds, [true, true])
        XCTAssertFalse(sounds.played.contains(.notification))

        // The notch is open: the banner isn't posted, so the notch plays the notice's sound, once.
        notchShowing = true
        model.receiveNotices([notice("n3", bannered: true)])
        poster.receive(loopBanner("n3"))
        await until { wait.started == 2 }
        wait.expire()
        await until { wait.ended == 2 }
        XCTAssertEqual(center.sounds, [true, true])
        XCTAssertEqual(sounds.played, [.notification])

        // A notice no banner shows sounds in the notch at once: nothing to wait for.
        model.receiveNotices([notice("n4")])
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

    // decision: APP-533
    func testARecoveredOrScheduledTaskSoundsByItsStatus() {
        let (model, sounds) = makeModel()
        let (poster, center) = makePoster(model)
        model.receiveOutcome(["task_id": "t-fail", "status": "failed", "answer": "The page didn't load.", "goal": "Check"])
        XCTAssertEqual(sounds.played, [.error])
        poster.receive(taskBanner("t-fail"))                 // its banner posts silently: it sounded once
        XCTAssertEqual(center.sounds, [false])
        model.receiveOutcome(["task_id": "t-wait", "status": "needs_user", "answer": "Which file?", "goal": "Send"])
        model.receiveOutcome(["task_id": "t-stop", "status": "cancelled", "answer": "Stopped.", "goal": "Tidy"])
        model.receiveOutcome(["task_id": "t-done", "status": "done", "answer": "Done.", "goal": "Tidy"])
        XCTAssertEqual(sounds.played, [.error, .needsYou, .notification])
    }

    // decision: APP-533
    func testABannerNeverSilencesNeedsYouDoneOrError() async {
        let (model, sounds) = makeModel()
        let (poster, center) = makePoster(model)
        // A loop's banner for task T, then T fails: the error still plays.
        poster.receive(loopBanner("n-t", task: "t-loop"))
        XCTAssertEqual(center.sounds, [true])
        model.receiveOutcome(["task_id": "t-loop", "status": "failed", "answer": "It stopped.", "goal": "Watch"])
        XCTAssertEqual(sounds.played, [.error])
        // A question's banner before its approval event: needs you plays once, and the banner is silent.
        poster.receive(questionBanner("q-early"))
        model.receiveBusEvent(["topic": "approval.requested", "data": [
            "approval_id": "q-early", "kind": "owner_question", "reason": "Send it?"] as [String: Any]])
        XCTAssertEqual(center.sounds, [true, false])
        XCTAssertEqual(sounds.played, [.error, .needsYou])
    }

    // decision: APP-533, APP-528
    func testAHandoffSoundsAsNeedsYouAndDuringACallAtSubtle() {
        let defaults = scratchDefaults("SoundFeedbackTests")
        let feedback = SoundFeedback(defaults: defaults)
        let player = DecidingPlayer(makeEngine(defaults: defaults, input: { [1] }))        // another app is on a call
        feedback.player = player
        let center = FakeNotificationCenter()
        let poster = NotificationPoster(center: center, settle: .zero)
        poster.sounds = feedback
        poster.configure(notchShowing: { true }, open: { _ in })
        poster.receive(["id": "handoff:h1", "category": "question", "title": "FERB needs you for a moment",
                        "body": "example.com wants to check you're human.",
                        "target": ["kind": "handoff", "handoff_id": "h1", "task_id": "t1"], "when_hidden": false])
        XCTAssertEqual(center.sounds, [false])
        XCTAssertEqual(player.decided.map(\.0), [.needsYou])
        XCTAssertEqual(player.decided.first?.1, SoundDecision(subtle: true))
        poster.receive(["id": "reminder:r1", "category": "reminder", "title": "FERB reminder", "body": "Water the plants",
                        "target": ["kind": "reminder", "timer_id": "r1"], "when_hidden": false])
        XCTAssertEqual(center.sounds, [false, false])                 // a call: the notification banner is silent too
        XCTAssertEqual(player.decided.count, 1)
    }

    // decision: APP-521
    func testARestartsBacklogOfNoticesIsOneSound() {
        let (model, sounds) = makeModel()
        model.receiveNotices([notice("b1"), notice("b2"), notice("b3")])
        XCTAssertEqual(sounds.played, [.notification])
        model.receiveNotices([notice("b1"), notice("b2")])         // shown already: no sound again
        XCTAssertEqual(sounds.played, [.notification])
    }

    // decision: APP-533
    func testWhenMacOSHasFERBsBannerSoundOffTheNotchPlaysItOnce() async {
        let (model, sounds) = makeModel()
        let center = FakeNotificationCenter()
        center.soundAllowed = false
        let poster = NotificationPoster(center: center, settle: .zero)
        poster.sounds = model.feedback
        poster.configure(notchShowing: { false }, open: { _ in })
        let rpc = ScriptedRPC()
        rpc.events["notifications.subscribe"] = [loopBanner("m1")]
        _ = await poster.subscribeOnce(rpc: rpc)
        XCTAssertEqual(center.sounds, [false])
        model.receiveNotices([notice("m1", bannered: true)])
        XCTAssertEqual(sounds.played, [.notification])
    }

    // MARK: - Shortcut and pause wiring

    // MARK: - The banner's file

    // decision: APP-533
    func testTheBannerSoundIsAFinishedStereoMixOutsideTheSoundsBundle() throws {
        // The package root, where scripts/build-app.sh copies it from into FERB.app/Contents/Resources.
        let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent()
        let url = root.appendingPathComponent(FerbSoundFiles.bannerName)
        let file = try AVAudioFile(forReading: url)
        XCTAssertEqual(file.processingFormat.channelCount, 2)
        XCTAssertEqual(file.processingFormat.sampleRate, 48_000)
        XCTAssertLessThan(Double(file.length) / 48_000, 30)            // macOS plays banner sounds under 30 s
        let bundled = try XCTUnwrap(FerbSoundFiles.directory)
        XCTAssertFalse(FileManager.default.fileExists(atPath: bundled.appendingPathComponent(FerbSoundFiles.bannerName).path))
        for subtle in [false, true] {                                  // the in-app cue plays its own dry file
            XCTAssertFalse(FerbSoundFiles.files(for: .notification, subtle: subtle).isEmpty)
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

    private func notice(_ id: String, bannered: Bool = false) -> [String: Any] {
        var row: [String: Any] = ["id": id, "text": "The example loop paused.", "at": "2026-11-01T10:00:00Z"]
        if bannered { row["banner"] = true }
        return row
    }

    private func loopBanner(_ notice: String, task: String? = nil) -> [String: Any] {
        ["id": "loop:" + notice, "category": "loop", "title": "FERB loop", "body": "The example loop paused.",
         "target": ["kind": "task", "task_id": task ?? "tree-" + notice], "when_hidden": true]
    }

    private func taskBanner(_ task: String) -> [String: Any] {
        ["id": "task:" + task, "category": "task_finished", "title": "FERB", "body": "The page didn't load.",
         "target": ["kind": "task", "task_id": task], "when_hidden": true]
    }

    private func questionBanner(_ approval: String) -> [String: Any] {
        ["id": "question:" + approval, "category": "question", "title": "FERB needs you", "body": "Send it?",
         "target": ["kind": "question", "approval_id": approval], "when_hidden": true]
    }

    private func makePoster(_ model: NotchModel) -> (NotificationPoster, FakeNotificationCenter) {
        let center = FakeNotificationCenter()
        let poster = NotificationPoster(center: center, settle: .zero)
        poster.sounds = model.feedback
        poster.configure(notchShowing: { false }, open: { _ in })
        return (poster, center)
    }

    private func until(_ condition: () -> Bool) async {
        for _ in 0..<200 where !condition() { try? await Task.sleep(for: .milliseconds(10)) }
    }

}

/// The notch's wait for a banner, ended by the test: `ended` counts the waits whose code after the wait has run.
@MainActor
final class ManualNoticeWait {
    private var waiting: [CheckedContinuation<Void, Never>] = []
    private(set) var started = 0
    private(set) var ended = 0

    func sleep(_ duration: Duration) async throws {
        started += 1
        await withCheckedContinuation { waiting.append($0) }
        ended += 1
    }

    func expire() {
        let all = waiting
        waiting = []
        for waiter in all { waiter.resume() }
    }
}

/// Records what the engine would decide for each sound it is asked to play, and plays nothing.
@MainActor
private final class DecidingPlayer: FerbSoundPlaying {
    let engine: FerbSoundEngine
    var decided: [(FerbSound, SoundDecision?)] = []
    var outputLatency: TimeInterval = 0
    init(_ engine: FerbSoundEngine) { self.engine = engine }
    func play(_ s: FerbSound, at point: CGPoint?) { decided.append((s, engine.decision(for: s))) }
    func stopAll() {}
    func keepWarm(_ on: Bool) {}
    func allows(_ s: FerbSound) -> Bool { engine.allows(s) }
}
