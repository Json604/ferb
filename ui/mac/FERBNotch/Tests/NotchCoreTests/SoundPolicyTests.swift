import XCTest
import CoreGraphics
@testable import NotchCore

final class SoundPolicyTests: XCTestCase {
    // decision: APP-523
    func testPanIsClampedToPointSix() {
        XCTAssertEqual(SoundPolicy.pan(x: 0, screenWidth: 1440), -0.6, accuracy: 1e-6)
        XCTAssertEqual(SoundPolicy.pan(x: 1440, screenWidth: 1440), 0.6, accuracy: 1e-6)
        XCTAssertEqual(SoundPolicy.pan(x: 720, screenWidth: 1440), 0, accuracy: 1e-6)
        XCTAssertEqual(SoundPolicy.pan(x: 5000, screenWidth: 1440), 0.6, accuracy: 1e-6)
    }

    // decision: APP-523
    func testDistanceLowersGainAtMostThreeDB() {
        XCTAssertEqual(SoundPolicy.distanceGainDB(fromNotch: 0, screenHeight: 900), 0, accuracy: 1e-6)
        XCTAssertEqual(SoundPolicy.distanceGainDB(fromNotch: 900, screenHeight: 900), -3, accuracy: 1e-6)
        XCTAssertEqual(SoundPolicy.distanceGainDB(fromNotch: 4000, screenHeight: 900), -3, accuracy: 1e-6)
        XCTAssertEqual(SoundPolicy.distanceGainDB(fromNotch: 450, screenHeight: 900), -1.5, accuracy: 1e-6)
    }

    // decision: APP-653
    func testNothingPlaysWhenSystemUISoundsAreOff() {
        func decide(ui: Bool, feedback: Bool) -> SoundDecision? {
            SoundPolicy.decide(.error, uiSoundsEnabled: ui, feedbackEnabled: feedback, switchOn: true, subtle: false,
                               onACall: { false })
        }
        XCTAssertNil(decide(ui: false, feedback: true))
        XCTAssertNil(decide(ui: true, feedback: false))
        XCTAssertEqual(decide(ui: true, feedback: true), SoundDecision(subtle: false))
    }

    /// APP-643 (owner, 2026-11-03: a friend's M1 played no startup music): the first-launch music plays with the
    /// system's interface sounds off; every other sound still follows that switch, and FERB's own switches still apply.
    // decision: APP-643
    func testTheFirstLaunchMusicPlaysWithInterfaceSoundsOff() {
        XCTAssertNotNil(SoundPolicy.decide(.firstLaunch, uiSoundsEnabled: false, feedbackEnabled: true, switchOn: true,
                                           subtle: false, onACall: { false }))
        XCTAssertNil(SoundPolicy.decide(.error, uiSoundsEnabled: false, feedbackEnabled: true, switchOn: true,
                                        subtle: false, onACall: { false }))
        XCTAssertNil(SoundPolicy.decide(.firstLaunch, uiSoundsEnabled: false, feedbackEnabled: false, switchOn: true,
                                        subtle: false, onACall: { false }))
    }

    /// APP-651 (owner, 2026-11-04): a finished task or answer plays no sound; the rest of the outcomes keep theirs.
    // decision: APP-651
    func testAFinishedTaskPlaysNoSound() {
        XCTAssertNil(SoundPolicy.outcomeSound("done"))
        XCTAssertNil(SoundPolicy.outcomeSound("done_unverified"))
        XCTAssertEqual(SoundPolicy.outcomeSound("needs_user"), .needsYou)
        XCTAssertEqual(SoundPolicy.outcomeSound("failed"), .error)
    }
}

/// APP-528's "another app is using the microphone" counts apps, not system daemons (APP-660; audit U1: Siri's
/// always-on listener, corespeechd, silenced the first-launch music and every cue on the owner's Mac).
final class MicrophoneUseTests: XCTestCase {
    /// Invented processes: a speech daemon with no app (40); a call app (50); a browser (60) whose helper (61) runs
    /// the call; a helper inside the browser's bundle (62); a service launchd started for the browser (70); FERB (90)
    /// and its helper (91).
    private let facts: [pid_t: ProcessFacts] = [
        40: ProcessFacts(responsible: 40, parent: 1),
        50: ProcessFacts(appName: "Example Call", responsible: 50, parent: 1),
        60: ProcessFacts(appName: "Example Browser", responsible: 60, parent: 1),
        61: ProcessFacts(responsible: 60, parent: 60),
        62: ProcessFacts(bundleApp: "Example Browser", parent: 1),
        70: ProcessFacts(responsible: 60, parent: 1),
        90: ProcessFacts(appName: "FERB", parent: 1),
        91: ProcessFacts(responsible: 90, parent: 90),
    ]

    private func caller(_ inputs: [pid_t]) -> String? {
        MicrophoneUse.callingApp(inputs: inputs, isOwn: { $0 == 90 }, facts: { self.facts[$0] })
    }

    private func decide(_ s: FerbSound, inputs: [pid_t]) -> SoundDecision? {
        SoundPolicy.decide(s, uiSoundsEnabled: true, feedbackEnabled: true, switchOn: true, subtle: false,
                           onACall: { caller(inputs) != nil })
    }

    // decision: APP-660
    func testADaemonNoAppOwnsNeverCountsAsACall() {
        XCTAssertNil(caller([40]))
        XCTAssertEqual(decide(.firstLaunch, inputs: [40]), SoundDecision(subtle: false))   // the startup music plays
        XCTAssertEqual(decide(.notification, inputs: [40]), SoundDecision(subtle: false))
        XCTAssertNil(caller([12345]))                                                       // a process gone: no app
    }

    // decision: APP-660, APP-528
    func testAnAppHoldingTheMicrophonePlaysOnlyNeedsYouSoftly() {
        XCTAssertEqual(caller([40, 50]), "Example Call")
        XCTAssertNil(decide(.firstLaunch, inputs: [50]))
        XCTAssertNil(decide(.error, inputs: [50]))
        XCTAssertEqual(decide(.needsYou, inputs: [50]), SoundDecision(subtle: true))
    }

    // decision: APP-660
    func testAnAppsHelperCountsAsThatApp() {
        XCTAssertEqual(caller([61]), "Example Browser")          // the browser's helper process, by parent/responsible
        XCTAssertEqual(caller([62]), "Example Browser")          // a helper inside the browser's bundle
        XCTAssertEqual(caller([70]), "Example Browser")          // a service launchd started, responsible to the browser
        XCTAssertNil(decide(.firstLaunch, inputs: [61]))
    }

    // decision: APP-528
    func testFERBsOwnProcessesNeverCount() {
        XCTAssertNil(caller([90]))
        XCTAssertNil(caller([91]))
    }

    private func onMicrophone(_ uses: [InputUse], running: Bool) -> [pid_t] {
        MicrophoneUse.microphoneInputs(uses, microphoneRunning: running, isOwn: { $0 == 90 })
    }

    /// 2026-11-14: an audio relay app (a menu-bar app, pid 50 here) ran input all the time on a tap of the Mac's own
    /// sound, naming no device, with the Mac's microphone off; every sound of the owner's first setup was silenced as
    /// "a call". Input on virtual devices or a tap isn't the microphone; input on a microphone still is.
    // decision: APP-908, APP-528
    func testInputThatIsntAMicrophoneIsntACall() {
        let relay = InputUse(pid: 50, sources: [])
        XCTAssertEqual(onMicrophone([relay], running: false), [])
        XCTAssertEqual(decide(.firstLaunch, inputs: onMicrophone([relay], running: false)), SoundDecision(subtle: false))
        let loopback = InputUse(pid: 50, sources: [.virtual, .virtual])
        XCTAssertEqual(onMicrophone([loopback], running: true), [])           // the room's microphone is someone else's
        XCTAssertEqual(onMicrophone([InputUse(pid: 50, sources: [.virtual, .microphone])], running: false), [50])
        // a call app on the microphone, directly or through an aggregate holding it
        XCTAssertEqual(onMicrophone([InputUse(pid: 50, sources: [.microphone])], running: true), [50])
        XCTAssertNil(decide(.firstLaunch, inputs: onMicrophone([InputUse(pid: 50, sources: [.microphone])],
                                                                running: true)))
        // Core Audio names nothing or can't say: a call only while a microphone runs that FERB's dictation doesn't explain
        XCTAssertEqual(onMicrophone([relay], running: true), [50])
        XCTAssertEqual(onMicrophone([InputUse(pid: 50, sources: [.unknown])], running: true), [50])
        XCTAssertEqual(onMicrophone([relay, InputUse(pid: 90, sources: [.microphone])], running: true), [90])
        XCTAssertNil(caller(onMicrophone([relay, InputUse(pid: 90, sources: [.microphone])], running: true)))
    }
}
