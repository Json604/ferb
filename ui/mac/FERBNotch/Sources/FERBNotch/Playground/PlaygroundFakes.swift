#if FERB_PLAYGROUND
import AppKit
import NotchCore

/// `ferb setup --service` and `ferb setup --codex-login`, faked: a few lines of output, then success (or the failure
/// the control panel asked for). Nothing is run.
@MainActor
final class PlaygroundRunner: SetupCommandRunning {
    private let state: PlaygroundState
    private var job: Task<Void, Never>?
    private var done: ((Int32) -> Void)?
    private(set) var isRunning = false

    init(state: PlaygroundState) { self.state = state }

    func run(_ arguments: [String], line: @escaping (String) -> Void, done: @escaping (Int32) -> Void) -> Bool {
        guard !isRunning else { return false }
        isRunning = true
        self.done = done
        state.note("ferb " + arguments.joined(separator: " ") + " (faked)")
        let codex = arguments == AppSetupPlan.codexLoginArguments
        job = Task { [weak self, state] in
            let lines = codex ? ["Opening the sign-in page in your browser…", "Waiting for the browser…"]
                : ["Checking this Mac…", "Writing FERB's background service files…", "Starting FERB…"]
            for text in lines {
                await state.pause(codex ? 1.5 : 0.8)
                guard !Task.isCancelled else { return }
                line(text)
            }
            let fails = codex ? state.saved.signInFails : state.saved.serviceInstallFails
            if fails { line(codex ? "Sign-in was cancelled in the browser." : "Setup stopped: the playground is set to fail here.") }
            if !codex && !fails { state.saved.serviceInstalled = true }
            self?.finish(fails ? 1 : 0)
        }
        return true
    }

    func cancel() {
        guard isRunning else { return }
        job?.cancel()
        finish(130)
    }

    private func finish(_ status: Int32) {
        guard isRunning else { return }
        isRunning = false
        let finished = done
        done = nil
        finished?(status)
    }
}

/// FERB.app's own permissions (Notifications, Microphone with Speech Recognition), read from the playground's state;
/// a request is answered as the control panel says. macOS is never asked.
@MainActor
final class PlaygroundAppPermissions: AppPermissionsProviding {
    private let state: PlaygroundState
    init(state: PlaygroundState) { self.state = state }

    func notifications() async -> GrantStatus { state.grant("notifications") }
    func requestNotifications() async -> GrantStatus { await state.ask("notifications") }
    func microphone() -> GrantStatus { state.grant("microphone") }
    func requestMicrophone() async -> GrantStatus { await state.ask("microphone") }
    var microphoneDeniedPane: String { "microphone" }
}

/// The Keychain, faked: an API key is accepted and dropped at once; nothing is stored anywhere.
@MainActor
struct PlaygroundKeys: APIKeyStoring {
    func store(provider: APIProvider, key: String) async -> Bool { !key.isEmpty }
}

/// launchd's restart of FERB's background service, faked: switches that apply only after a restart are read now.
@MainActor
final class PlaygroundService: DaemonServicing {
    private let state: PlaygroundState
    init(state: PlaygroundState) { self.state = state }

    func restart() async -> Bool {
        state.note("background service restarted (faked)")
        await state.pause(1.0)
        state.applyRestart()
        return true
    }

    func start() async -> Bool {
        state.note("background service started (faked)")
        return true
    }
}

/// SMAppService for FERB's background service, faked. With "Login Items needs your OK" on, registering leaves it
/// waiting until the owner allows it in the stand-in Login Items pane (or the control panel).
@MainActor
final class PlaygroundBackground: BackgroundServiceRegistering {
    private let state: PlaygroundState
    private let opener: PlaygroundSettingsOpener
    private var registered = false

    init(state: PlaygroundState, opener: PlaygroundSettingsOpener) {
        self.state = state
        self.opener = opener
    }

    var status: ServiceStatus {
        guard registered || state.saved.serviceInstalled else { return .notRegistered }
        return state.saved.loginItemsNeedOK && !state.saved.loginItemsAllowed ? .requiresApproval : .enabled
    }

    func register() throws {
        registered = true
        state.note("background service registered (faked): \(status)")
    }

    func unregister() async throws { registered = false }

    func openLoginItems() { opener.showLoginItems() }
}

/// Dictation, faked: holding the talk keys "hears" a short sentence; the microphone is never opened.
@MainActor
final class PlaygroundMicrophone: VoiceCapturing {
    private var completion: ((Result<String, Error>) -> Void)?
    private var job: Task<Void, Never>?
    private let words = "Find three things on my Mac I could tidy up"

    func start(update: @escaping (String) -> Void, completion: @escaping (Result<String, Error>) -> Void) {
        cancel()
        self.completion = completion
        job = Task {
            var heard: [Substring] = []
            for word in words.split(separator: " ") {
                try? await Task.sleep(for: .milliseconds(220))
                guard !Task.isCancelled else { return }
                heard.append(word)
                update(heard.joined(separator: " "))
            }
        }
    }

    func finish() {
        job?.cancel()
        let done = completion
        completion = nil
        done?(.success(words))
    }

    func cancel() {
        job?.cancel()
        completion = nil
    }
}

/// The real sound engine behind a switch: the control panel's Sound off stops what's playing and plays nothing.
@MainActor
final class PlaygroundSound: FerbSoundPlaying {
    private let engine: FerbSoundPlaying
    private let state: PlaygroundState

    init(engine: FerbSoundPlaying, state: PlaygroundState) {
        self.engine = engine
        self.state = state
    }

    func play(_ s: FerbSound, at point: CGPoint?) {
        guard state.saved.soundOn else { return }
        engine.play(s, at: point)
    }
    func stopAll() { engine.stopAll() }
    func keepWarm(_ on: Bool) { engine.keepWarm(on) }
    var outputLatency: TimeInterval { engine.outputLatency }
    func allows(_ s: FerbSound) -> Bool { state.saved.soundOn && engine.allows(s) }
}

/// The tour's waits at the playground's speed (AppSetupController.introSleeper).
@MainActor
struct PlaygroundSleeper: IntroSleeping {
    let state: PlaygroundState
    func sleep(seconds: Double) async { await state.pause(seconds) }
}

/// The orb's frame clock at the playground's speed: each tick's time is scaled, so springs, fades and hops run faster
/// or slower exactly as they would in time.
@MainActor
final class PlaygroundClock: OrbClock {
    private let clock: OrbClock
    private let state: PlaygroundState

    init(clock: OrbClock, state: PlaygroundState) {
        self.clock = clock
        self.state = state
    }

    func start(_ tick: @escaping (Double) -> Void) {
        clock.start { [state] dt in tick(dt * state.speed) }
    }

    func stop() { clock.stop() }
}
#endif
