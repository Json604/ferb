#if FERB_PLAYGROUND
import Foundation
import NotchCore

/// The fake world behind the playground's intro: what the fake daemon, the fake permissions and the fake services
/// report, and the control panel's knobs. Saved in the playground's home after every change, so a simulated quit and
/// reopen comes back to the same world.
@MainActor
final class PlaygroundState: ObservableObject {
    /// How the "owner" answers when FERB asks for a permission (macOS's popup, or a switch in System Settings).
    enum Answer: String, Codable, CaseIterable, Identifiable {
        case allowSoon = "Allows after a moment"
        case deny = "Says no"
        case waitForMe = "Wait for me (panel / stand-in window)"
        var id: String { rawValue }
    }

    /// How a daemon call the intro waits on behaves.
    enum Outcome: String, Codable, CaseIterable, Identifiable {
        case normal = "Normal"
        case slow = "Slow"
        case fails = "Fails"
        var id: String { rawValue }
    }

    /// The seven permissions Allow all asks for, in its order (SetupModel.allowAllSequence).
    nonisolated static let permissions = ["microphone", "clipboard", "input_monitoring", "accessibility",
                                          "full_disk_access", "notifications", "screen_recording"]
    /// FERB.app's own (macOS asks the app); the rest are the daemon's (FERB's agent app).
    nonisolated static let appPermissions: Set<String> = ["notifications", "microphone"]
    /// A new grant of these reaches a running process only after it restarts (ferb/device/permissions.py).
    nonisolated static let restartToCheck: Set<String> = ["screen_recording", "input_monitoring", "full_disk_access"]

    struct Saved: Codable {
        var grants: [String: String] = Dictionary(uniqueKeysWithValues: PlaygroundState.permissions.map {
            ($0, GrantStatus.notDetermined.rawValue)
        })
        var afterRestart: [String] = []
        var runtime: String?
        var claudeSignIn: String?
        var persona = "not_started"
        var browsersPaired = false
        var identityAddress = ""
        var identityPaired = false
        var paused = false
        var setupDone = false
        var serviceInstalled = false
        var answer: Answer = .allowSoon
        var grantsNeedRestart = true
        var loginItemsNeedOK = false
        var loginItemsAllowed = false
        var serviceInstallFails = false
        var signInFails = false
        var scanSeesSomething = true
        var pairingFails = false
        var suggest: Outcome = .normal
        var preview: Outcome = .normal
        var soundOn = true
        var speed = 1.0
    }

    @Published var saved: Saved { didSet { save() } }
    /// The questions waiting for the owner's answer in the control panel ("Wait for me"), by permission id.
    @Published private(set) var asking: [String] = []
    /// The fake daemon's calls and the fakes' actions, newest last.
    @Published private(set) var log: [String] = []
    /// Runs when the speed changes (the setup model's polls follow it).
    var onSpeedChange: (() -> Void)?
    private var answers: [String: [(GrantStatus) -> Void]] = [:]
    private let file: URL

    init(file: URL, fresh: Bool) {
        self.file = file
        if !fresh, let data = try? Data(contentsOf: file), let read = try? JSONDecoder().decode(Saved.self, from: data) {
            saved = read
        } else {
            saved = Saved()
        }
        save()
    }

    private func save() {
        if let data = try? JSONEncoder().encode(saved) { try? data.write(to: file, options: .atomic) }
    }

    // MARK: Facts

    func grant(_ id: String) -> GrantStatus { GrantStatus(rawValue: saved.grants[id] ?? "") ?? .unknown }

    /// The owner flipped a switch or answered a popup. A switch that applies only after a restart waits for one (the
    /// fake service restart, or a simulated quit and reopen), as on a real Mac.
    func set(_ id: String, _ status: GrantStatus, fromSwitch: Bool = false) {
        if fromSwitch && status == .granted && saved.grantsNeedRestart && Self.restartToCheck.contains(id) {
            if !saved.afterRestart.contains(id) { saved.afterRestart.append(id) }
            note("\(PermissionCopy.label(id)) switched on; it applies after FERB restarts")
            return
        }
        saved.afterRestart.removeAll { $0 == id }
        saved.grants[id] = status.rawValue
        note("\(PermissionCopy.label(id)): \(PermissionCopy.status(status))")
    }

    /// A restart (the service's, or the whole app's quit and reopen): switches flipped since are read now.
    func applyRestart() {
        for id in saved.afterRestart { saved.grants[id] = GrantStatus.granted.rawValue }
        if !saved.afterRestart.isEmpty { note("restart applied: " + saved.afterRestart.map(PermissionCopy.label).joined(separator: ", ")) }
        saved.afterRestart = []
    }

    var allGranted: Bool { Self.permissions.allSatisfy { grant($0) == .granted } }

    /// Every step before a beat done, for a jump: signed in, every permission on, browsers paired, the scan seen.
    func completeEarlierSteps(before beat: IntroBeat) {
        var next = saved
        if beat.rawValue > IntroBeat.meet.rawValue {
            next.serviceInstalled = true
            next.loginItemsAllowed = true
        }
        if beat.rawValue > IntroBeat.signIn.rawValue {
            next.runtime = next.runtime ?? "codex"
            next.persona = next.scanSeesSomething ? "seen" : "finished"
        }
        if beat.rawValue > IntroBeat.permissions.rawValue {
            for id in Self.permissions { next.grants[id] = GrantStatus.granted.rawValue }
            next.afterRestart = []
        }
        if beat.rawValue > IntroBeat.browsers.rawValue { next.browsersPaired = true }
        saved = next
    }

    // MARK: Asking the owner

    /// macOS's question for a permission, answered by the panel's choice: at once, after a moment, or when the owner
    /// answers in the control panel. Returns the answer (the grant is set already).
    func ask(_ id: String) async -> GrantStatus {
        note("macOS asks: \(PermissionCopy.label(id))")
        switch saved.answer {
        case .allowSoon:
            await pause(1.5)
            set(id, .granted)
            return .granted
        case .deny:
            await pause(0.8)
            set(id, .denied)
            return .denied
        case .waitForMe:
            if !asking.contains(id) { asking.append(id) }
            let status = await withCheckedContinuation { (continuation: CheckedContinuation<GrantStatus, Never>) in
                answers[id, default: []].append { continuation.resume(returning: $0) }
            }
            return status
        }
    }

    /// The owner's answer from the control panel to a question waiting there.
    func answer(_ id: String, _ status: GrantStatus) {
        asking.removeAll { $0 == id }
        set(id, status)
        let waiting = answers.removeValue(forKey: id) ?? []
        for resume in waiting { resume(status) }
    }

    // MARK: Time and the log

    var speed: Double { max(saved.speed, 0.1) }

    func setSpeed(_ value: Double) {
        saved.speed = value
        onSpeedChange?()
    }

    /// A wait in the fake world, scaled by the speed.
    func pause(_ seconds: Double) async {
        try? await Task.sleep(for: .milliseconds(Int(seconds * 1000 / speed)))
    }

    func note(_ line: String) {
        let stamp = Self.clock.string(from: Date())
        log = Array((log + ["\(stamp)  \(line)"]).suffix(200))
    }

    private static let clock: DateFormatter = {
        let formatter = DateFormatter()
        formatter.dateFormat = "HH:mm:ss"
        return formatter
    }()
}
#endif
