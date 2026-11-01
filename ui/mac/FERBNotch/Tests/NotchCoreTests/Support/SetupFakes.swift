import Foundation
import XCTest
@testable import FERBNotch
import NotchCore

@MainActor
final class FakeSetupRunner: SetupCommandRunning {
    private(set) var runs: [[String]] = []
    private(set) var isRunning = false
    var startFails = false
    private var line: ((String) -> Void)?
    private var done: ((Int32) -> Void)?

    func run(_ arguments: [String], line: @escaping (String) -> Void, done: @escaping (Int32) -> Void) -> Bool {
        guard !startFails else { return false }
        runs.append(arguments)
        isRunning = true
        self.line = line
        self.done = done
        return true
    }

    func cancel() { finish(130) }
    func say(_ text: String) { line?(text) }

    func finish(_ status: Int32) {
        isRunning = false
        let finished = done
        done = nil
        line = nil
        finished?(status)
    }
}

@MainActor
final class FakeAppPermissions: AppPermissionsProviding {
    var notificationStatus: GrantStatus = .notDetermined
    var microphoneStatus: GrantStatus = .notDetermined
    var microphoneDeniedPane = "microphone"
    /// What the owner answers in macOS's microphone prompt.
    var microphoneAnswer: GrantStatus = .granted
    private(set) var requests: [String] = []

    func notifications() async -> GrantStatus { notificationStatus }
    var notificationsHang = false                   // the banner is never answered
    func requestNotifications() async -> GrantStatus {
        requests.append("notifications")
        if notificationsHang { try? await Task.sleep(for: .seconds(3600)) }
        notificationStatus = .granted
        return .granted
    }
    func microphone() -> GrantStatus { microphoneStatus }
    func requestMicrophone() async -> GrantStatus {
        requests.append("microphone")
        microphoneStatus = microphoneAnswer
        return microphoneAnswer
    }
}

@MainActor
final class FakeOpener: SystemOpening {
    private(set) var opened: [URL] = []
    private(set) var revealed: [String] = []
    func open(_ url: URL) { opened.append(url) }
    func reveal(_ path: String) { revealed.append(path) }
}

@MainActor
final class FakeKeys: APIKeyStoring {
    private(set) var providers: [APIProvider] = []
    var accept = true
    func store(provider: APIProvider, key: String) async -> Bool {
        providers.append(provider)                  // the key itself is never kept, here or anywhere
        return accept && !key.isEmpty
    }
}

@MainActor
final class FakeService: DaemonServicing {
    private(set) var restarts = 0
    var result = true
    func restart() async -> Bool {
        restarts += 1
        return result
    }
}

/// macOS's SMAppService for FERB's background service, faked: `register()` moves it to `after`.
@MainActor
final class FakeBackgroundService: BackgroundServiceRegistering {
    var status: ServiceStatus = .notRegistered
    var after: ServiceStatus = .enabled
    var refusal: Error?
    private(set) var registrations = 0
    private(set) var unregistrations = 0
    private(set) var openedSettings = 0

    func register() throws {
        registrations += 1
        if let refusal { throw refusal }
        status = after
    }
    var afterUnregister: ServiceStatus = .notRegistered
    var unregisterError: Error?

    func unregister() async throws {
        unregistrations += 1
        if let unregisterError { throw unregisterError }
        status = afterUnregister
    }
    func openLoginItems() { openedSettings += 1 }
}

/// One setup window's collaborators, all faked.
@MainActor
struct SetupRig {
    let rpc = ScriptedRPC()
    let runner = FakeSetupRunner()
    let app = FakeAppPermissions()
    let opener = FakeOpener()
    let keys = FakeKeys()
    let service = FakeService()

    func model(_ action: AppSetupAction = .none, background: BackgroundServiceRegistering? = nil) -> SetupModel {
        let model = SetupModel(rpc: rpc, runner: runner, app: app, system: opener, keys: keys, service: service,
                               background: background)
        model.serviceStartWait = .milliseconds(200)
        model.serviceAction = action
        model.requestSettle = .zero
        model.grantPoll = .milliseconds(5)
        return model
    }
}

/// `setup.permissions` replies.
enum SetupReplies {
    static let pane = "x-apple.systempreferences:com.apple.preference.security?"
    static let agent = "/Applications/FERB.app/Contents/Helpers/FERB.app"

    static func permissions(accessibility: String = "denied", screen: String = "granted", fda: String = "denied",
                            fdaBlocked: String = "", setupDone: Bool = false) -> [String: Any] {
        func row(_ id: String, _ status: String, _ anchor: String, required: Bool, requestable: Bool,
                 restart: Bool = false, blocked: String = "", process: String = "daemon") -> [String: Any] {
            ["id": id, "status": status, "process": process, "required": required, "pane": pane + anchor,
             "requestable": requestable, "restart_to_check": restart, "blocked": blocked]
        }
        let rows = [row("accessibility", accessibility, "Privacy_Accessibility", required: true, requestable: true),
                    row("screen_recording", screen, "Privacy_ScreenCapture", required: true, requestable: true,
                        restart: true),
                    row("full_disk_access", fda, "Privacy_AllFiles", required: true, requestable: false, restart: true,
                        blocked: fdaBlocked),
                    row("input_monitoring", "granted", "Privacy_ListenEvent", required: false, requestable: true)]
        return ["rows": rows, "automation": [["name": "Notes", "bundle_id": "com.apple.Notes", "status": "denied"],
                                             ["name": "Mail", "bundle_id": "com.apple.mail", "status": "not_determined"]],
                "automation_pane": pane + "Privacy_Automation", "grant_target": agent,
                "app_panes": ["notifications": "x-apple.systempreferences:com.apple.Notifications-Settings.extension?id=com.ferb.FERB",
                              "microphone": pane + "Privacy_Microphone",
                              "speech_recognition": pane + "Privacy_SpeechRecognition"],
                "setup_done": setupDone]
    }
}

/// Waits (at most 2 s) for something an unstructured Task does.
@MainActor
func eventually(_ condition: @escaping () -> Bool, file: StaticString = #filePath, line: UInt = #line) async {
    let end = ContinuousClock.now + .seconds(2)
    while !condition() {
        if ContinuousClock.now > end { XCTFail("condition never held", file: file, line: line); return }
        try? await Task.sleep(for: .milliseconds(10))
    }
}

/// The intro's orb, faked: what it was asked to do. `holdFlyHome` keeps a flight home in the air until `tearDown`.
@MainActor
final class FakeOrb: OrbControlling {
    var onHomeArrival: (@MainActor () -> Void)?
    private(set) var docks: [(url: URL?, instruction: IntroLine?)] = []
    private(set) var launches: [(point: CGPoint, instruction: IntroLine?)] = []
    /// Runs as each flight home begins (a test reads what had happened by then).
    var onFlyHome: (() -> Void)?
    private(set) var hops = 0
    private(set) var flyHomes = 0
    private(set) var tearDowns = 0
    var holdFlyHome = false
    private var inFlight: CheckedContinuation<Void, Never>?
    var flying: Bool { inFlight != nil }

    func launch(to target: CGPoint, instruction: IntroLine?) async { launches.append((target, instruction)) }
    func dockBesideSettings(dragging url: URL?, instruction: IntroLine?) { docks.append((url, instruction)) }
    func hop() { hops += 1 }
    func flyHome() async {
        flyHomes += 1
        onFlyHome?()
        guard holdFlyHome else { return }
        await withCheckedContinuation { inFlight = $0 }
    }
    func tearDown() {
        tearDowns += 1
        inFlight?.resume()
        inFlight = nil
    }
}

/// The intro's waits, instant unless held: a held duration waits until released.
@MainActor
final class FakeIntroSleeper: IntroSleeping {
    private(set) var asked: [Double] = []
    private var held: Set<Double> = []
    private var waiting: [Double: [CheckedContinuation<Void, Never>]] = [:]

    func hold(_ seconds: Double) { held.insert(seconds) }
    func release(_ seconds: Double) {
        held.remove(seconds)
        for waiter in waiting.removeValue(forKey: seconds) ?? [] { waiter.resume() }
    }
    func isWaiting(_ seconds: Double) -> Bool { !(waiting[seconds] ?? []).isEmpty }

    func sleep(seconds: Double) async {
        asked.append(seconds)
        if held.contains(seconds) {
            await withCheckedContinuation { waiting[seconds, default: []].append($0) }
            return
        }
        try? await Task.sleep(for: .milliseconds(1))
    }
}

/// The owner's shortcuts and menu actions, faked.
@MainActor
final class FakeControlFeed: IntroControlFeeding {
    var onControl: ((IntroControl) -> Void)?
    func send(_ control: IntroControl) { onControl?(control) }
}
