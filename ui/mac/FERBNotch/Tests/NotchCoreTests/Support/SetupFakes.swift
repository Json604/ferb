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
        microphoneStatus = .granted
        return .granted
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

/// One setup window's collaborators, all faked.
@MainActor
struct SetupRig {
    let rpc = ScriptedRPC()
    let runner = FakeSetupRunner()
    let app = FakeAppPermissions()
    let opener = FakeOpener()
    let keys = FakeKeys()
    let service = FakeService()

    func model(_ action: AppSetupAction = .none) -> SetupModel {
        let model = SetupModel(rpc: rpc, runner: runner, app: app, system: opener, keys: keys, service: service)
        model.serviceAction = action
        model.requestSettle = .zero
        return model
    }
}

/// `setup.permissions` replies.
enum SetupReplies {
    static let pane = "x-apple.systempreferences:com.apple.preference.security?"
    static let agent = "/Applications/FERB.app/Contents/Helpers/FERB.app"

    static func permissions(accessibility: String = "denied", screen: String = "granted", fda: String = "denied",
                            fdaBlocked: String = "", setupDone: Bool = false, hearing: String? = nil) -> [String: Any] {
        func row(_ id: String, _ status: String, _ anchor: String, required: Bool, requestable: Bool,
                 restart: Bool = false, blocked: String = "", process: String = "daemon") -> [String: Any] {
            ["id": id, "status": status, "process": process, "required": required, "pane": pane + anchor,
             "requestable": requestable, "restart_to_check": restart, "blocked": blocked]
        }
        var rows = [row("accessibility", accessibility, "Privacy_Accessibility", required: true, requestable: true),
                    row("screen_recording", screen, "Privacy_ScreenCapture", required: true, requestable: true,
                        restart: true),
                    row("full_disk_access", fda, "Privacy_AllFiles", required: true, requestable: false, restart: true,
                        blocked: fdaBlocked),
                    row("input_monitoring", "granted", "Privacy_ListenEvent", required: false, requestable: true)]
        if let hearing {
            rows.append(row("hearing", hearing, "Privacy_SpeechRecognition", required: false, requestable: true,
                            process: "voice"))
        }
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
