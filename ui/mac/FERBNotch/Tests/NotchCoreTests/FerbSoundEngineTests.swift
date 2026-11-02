import XCTest
import AVFAudio
@testable import FERBNotch
@testable import NotchCore

/// The sound engine's settings and lifetime (fix round 1). The engine under test renders offline (manual rendering
/// mode), so nothing reaches the speakers; no call is running (the Mac's real microphone users are never read).
@MainActor
final class FerbSoundEngineTests: XCTestCase {
    // decision: APP-640
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

    private func settle() async {
        for _ in 0..<10 { await Task.yield() }
    }
}
