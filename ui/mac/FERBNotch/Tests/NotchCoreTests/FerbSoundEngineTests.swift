import XCTest
import AVFAudio
@testable import FERBNotch
@testable import NotchCore

/// The sound engine's settings and lifetime (fix round 1). The engine under test renders offline (manual rendering
/// mode), so nothing reaches the speakers; no call is running (the Mac's real microphone users are never read).
@MainActor
final class FerbSoundEngineTests: XCTestCase {
    // decision: APP-653
    func testTheAlertVolumeScalesTheGainAndUISoundsOffSilenceIt() {
        let defaults = scratchDefaults("FerbSoundEngineTests")
        var prefs: [String: Any] = [:]
        let engine = FerbSoundEngine(defaults: defaults, preference: { prefs[$0] }, headphones: { false },
                                     screens: { [] }, measureLatency: { 0 }, inputProcesses: { [] })
        XCTAssertEqual(engine.settingsGain, 1)               // absent: full alert volume; FERB at its default volume
        prefs["com.apple.sound.beep.volume"] = NSNumber(value: 0.5)
        XCTAssertEqual(engine.settingsGain, 0.5)
        defaults.set(0.36, forKey: "feedback.volume")
        XCTAssertEqual(engine.settingsGain ?? 0, 1, accuracy: 1e-6)
        prefs["com.apple.sound.uiaudio.enabled"] = NSNumber(value: 0)
        XCTAssertNil(engine.settingsGain)
    }

    // decision: APP-523
    func testKeepingWarmHoldsTheEngineThroughIdleUntilReleased() async throws {
        let audio = AVAudioEngine()
        try audio.enableManualRenderingMode(.offline, format: AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2)!,
                                            maximumFrameCount: 4096)
        let engine = FerbSoundEngine(engine: audio, defaults: scratchDefaults("FerbSoundEngineTests"),
                                     preference: { _ in nil }, headphones: { false }, screens: { [] },
                                     measureLatency: { 0.12 }, idleSleep: { _ in }, inputProcesses: { [] })
        XCTAssertEqual(engine.outputLatency, 0.12 + FerbSoundEngine.scheduleLead, accuracy: 1e-9)
        engine.play(.orbHop, at: nil)
        XCTAssertTrue(engine.isRunning)
        await settle()
        XCTAssertFalse(engine.isRunning)                     // idle: it stops by itself
        engine.keepWarm(true)
        XCTAssertTrue(engine.isRunning)                      // pre-rolled for the tour
        engine.play(.orbHop, at: nil)
        await settle()
        XCTAssertTrue(engine.isRunning)                      // warm: it never idles out
        engine.keepWarm(false)
        await settle()
        XCTAssertFalse(engine.isRunning)
        engine.keepWarm(true)
        engine.stopAll()
        XCTAssertFalse(engine.isRunning)
        engine.play(.orbHop, at: nil)
        await settle()
        XCTAssertFalse(engine.isRunning)                     // stopAll ended keeping warm too
    }

    /// 2026-11-14: with an audio relay's tap running input (no device named, the Mac's microphone off), the first-launch
    /// clip and the tour's sounds are played from their files; the same app on the microphone still silences them.
    // decision: APP-908, APP-529
    func testARelaysInputDoesntSilenceTheFirstLaunchClip() throws {
        func engine(_ use: InputUse) throws -> FerbSoundEngine {
            let audio = AVAudioEngine()
            try audio.enableManualRenderingMode(.offline, format: AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2)!,
                                                maximumFrameCount: 4096)
            // invented process facts (TEST-508): pid 4242 is an app
            return FerbSoundEngine(engine: audio, defaults: scratchDefaults("FerbSoundEngineTests"),
                                   preference: { _ in nil }, headphones: { false }, screens: { [] },
                                   measureLatency: { 0 }, idleSleep: { _ in },
                                   inputProcesses: { MicrophoneUse.microphoneInputs([use], microphoneRunning: false,
                                                                                     isOwn: { _ in false }) },
                                   ownProcess: { _ in false },
                                   processFacts: { _ in ProcessFacts(appName: "Example Relay") })
        }
        let relayed = try engine(InputUse(pid: 4242, sources: []))
        relayed.play(.firstLaunch, at: nil)
        XCTAssertEqual(relayed.lastPlayed?.lastPathComponent, "ferb-first-launch.caf")
        XCTAssertTrue(relayed.isRunning)
        relayed.play(.orbDrip, at: nil)
        XCTAssertEqual(relayed.lastPlayed?.lastPathComponent, "ferb-orb-drip.caf")

        let calling = try engine(InputUse(pid: 4242, sources: [.microphone]))
        calling.play(.firstLaunch, at: nil)
        XCTAssertNil(calling.lastPlayed)
        XCTAssertFalse(calling.isRunning)
        calling.stopAll()
        relayed.stopAll()
    }

    private func settle() async {
        for _ in 0..<10 { await Task.yield() }
    }
}
