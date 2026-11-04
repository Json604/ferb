import AppKit
import AVFoundation
import Foundation
import NotchCore
import ServiceManagement
@preconcurrency import Speech
import UserNotifications

/// Runs FERB.app's own `ferb` command for setup (L34 C): the background service (`setup --service [--update]`) and
/// FERB's Codex sign-in (`setup --codex-login`). One at a time; output arrives line by line.
@MainActor
protocol SetupCommandRunning: AnyObject {
    var isRunning: Bool { get }
    func run(_ arguments: [String], line: @escaping (String) -> Void, done: @escaping (Int32) -> Void) -> Bool
    func cancel()
}

/// FERB.app's own permissions: Notifications, and Microphone with Speech Recognition (dictation).
@MainActor
protocol AppPermissionsProviding: AnyObject {
    func notifications() async -> GrantStatus
    func requestNotifications() async -> GrantStatus
    func microphone() -> GrantStatus
    func requestMicrophone() async -> GrantStatus
    /// Which pane to open while the microphone row is off: "microphone" or "speech_recognition".
    var microphoneDeniedPane: String { get }
}

@MainActor
protocol SystemOpening: AnyObject {
    func open(_ url: URL)
    func reveal(_ path: String)
}

@MainActor
protocol APIKeyStoring {
    func store(provider: APIProvider, key: String) async -> Bool
}

@MainActor
protocol DaemonServicing: AnyObject {
    func restart() async -> Bool
    /// Starts the service if it isn't running; a running one (even one still starting) is left alone.
    func start() async -> Bool
}

/// FERB's background service as macOS knows it (DMN-513): a user LaunchAgent that `ferb setup --service` writes and
/// loads, listed under Login Items → Allow in the Background, where the owner can switch it off; plus any SMAppService
/// registration a 2026-11 build left, which FERB.app unregisters once before setup (macOS would keep starting that
/// build's job, and without a Team ID its launch constraint kills every later build's daemon).
@MainActor
protocol BackgroundServiceManaging: AnyObject {
    /// A 2026-11 build's SMAppService registration of FERB's plist, as macOS lists it.
    var registration: ServiceStatus { get }
    /// That registration removed, waited for; nothing when macOS lists none.
    func unregister() async throws
    /// The owner switched FERB's LaunchAgent off (launchd lists com.ferb.daemon as disabled).
    func switchedOff() async -> Bool
    func openLoginItems()
}

/// SMAppService.agent for Contents/Library/LaunchAgents/com.ferb.daemon.plist (only to unregister it: SMAppService
/// returns with the Developer ID, T-27) and launchd's list of switched-off services.
@MainActor
final class LaunchAgentService: BackgroundServiceManaging {
    private let service = SMAppService.agent(plistName: ServicePlan.plistName)
    private let launchAgent = FileManager.default.homeDirectoryForCurrentUser
        .appendingPathComponent("Library/LaunchAgents/\(ServicePlan.plistName)").path

    /// A 2026-11 build's registration only (APP-683): SMAppService also reports FERB's own LaunchAgent job as
    /// enabled, and unregistering that boots the running daemon out.
    var registration: ServiceStatus {
        let reported: ServiceStatus
        switch service.status {
        case .enabled: reported = .enabled
        case .requiresApproval: reported = .requiresApproval
        case .notFound: reported = .notFound
        case .notRegistered: reported = .notRegistered
        @unknown default: reported = .notRegistered
        }
        guard reported.listed else { return reported }
        let printed = Self.launchdJob()
        let agentNamesLabel = (try? Data(contentsOf: URL(fileURLWithPath: launchAgent))).flatMap {
            try? PropertyListSerialization.propertyList(from: $0, format: nil) as? [String: Any]
        }?["Label"] as? String == LaunchctlPlan.label
        return ServiceStatus.retired(reported: reported, loaded: printed != nil,
                                     loadedFrom: printed.flatMap(LaunchctlPlan.loadedPath), launchAgent: launchAgent,
                                     launchAgentExists: agentNamesLabel)
    }

    /// `launchctl print gui/<uid>/com.ferb.daemon` (it only reads launchd's state); nil when launchd has no such job.
    private static func launchdJob() -> String? {
        let launchctl = Process()
        launchctl.executableURL = URL(fileURLWithPath: "/bin/launchctl")
        launchctl.arguments = ["print", "gui/\(getuid())/\(LaunchctlPlan.label)"]
        let pipe = Pipe()
        launchctl.standardOutput = pipe
        launchctl.standardError = FileHandle.nullDevice
        do { try launchctl.run() } catch { return nil }
        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        launchctl.waitUntilExit()
        return launchctl.terminationStatus == 0 ? String(decoding: data, as: UTF8.self) : nil
    }

    func unregister() async throws {
        guard registration.listed else { return }
        try await service.unregister()
    }

    /// `launchctl print-disabled gui/<uid>` only reads launchd's state.
    func switchedOff() async -> Bool {
        let output: String = await Task.detached {
            let launchctl = Process()
            launchctl.executableURL = URL(fileURLWithPath: "/bin/launchctl")
            launchctl.arguments = ["print-disabled", "gui/\(getuid())"]
            let pipe = Pipe()
            launchctl.standardOutput = pipe
            launchctl.standardError = FileHandle.nullDevice
            do { try launchctl.run() } catch { return "" }
            let data = pipe.fileHandleForReading.readDataToEndOfFile()
            launchctl.waitUntilExit()
            return String(decoding: data, as: UTF8.self)
        }.value
        return LaunchctlPlan.disabled(output)
    }

    func openLoginItems() { SMAppService.openSystemSettingsLoginItems() }
}

extension BackgroundServiceManaging {
    /// Uninstall's first step: a 2026-11 build's registration unregistered from macOS, waited for and checked (`ferb
    /// uninstall` then boots the LaunchAgent out and deletes it). Nil when macOS lists none, else why not (`ferb
    /// uninstall` then doesn't run: macOS would keep starting FERB).
    func unregisterForUninstall() async -> String? {
        do { try await unregister() } catch {
            return "macOS didn't turn off FERB's background service (\(error.localizedDescription)), so nothing was " +
                "uninstalled. Try again, or turn FERB off in System Settings → General → Login Items first."
        }
        if !registration.listed { return nil }
        return "macOS still lists FERB's background service as on, so nothing was uninstalled. Try again, or turn " +
            "FERB off in System Settings → General → Login Items first."
    }
}

enum AppBundle {
    /// Only an app bundle has a notification center (a `swift run` build has none).
    static var isApp: Bool { Bundle.main.bundleURL.pathExtension == "app" && Bundle.main.bundleIdentifier != nil }
}

/// The bundled `ferb` command, run from FERB.app's own Python. Quitting FERB stops it; `ferb setup` finishes
/// reinstalling the background service first (`_finish_install`). Setup never waits for an answer here (stdin is empty).
@MainActor
final class SetupCommand: SetupCommandRunning {
    private let launcher: URL
    private var process: Process?
    private var decoder = ConsoleDecoder()
    private var splitter = LineSplitter()
    private var outputEnded = false
    private var exitStatus: Int32?
    private var onLine: ((String) -> Void)?
    private var onDone: ((Int32) -> Void)?

    init(launcher: URL) { self.launcher = launcher }

    var isRunning: Bool { process != nil }

    func run(_ arguments: [String], line: @escaping (String) -> Void, done: @escaping (Int32) -> Void) -> Bool {
        guard process == nil else { return false }
        signal(SIGPIPE, SIG_IGN)
        decoder = ConsoleDecoder()
        splitter = LineSplitter()
        outputEnded = false
        exitStatus = nil
        onLine = line
        onDone = done
        let child = Process()
        child.executableURL = launcher
        child.arguments = arguments
        var environment = ProcessInfo.processInfo.environment
        environment["PYTHONUNBUFFERED"] = "1"
        // `setup --service` and `--cleanup` move to a session of their own, so FERB quitting can't cut a bootout and
        // bootstrap in half (ferb/cli own_session, DMN-537); a quit still reaches them as cancel's SIGTERM.
        environment["FERB_OWN_SESSION"] = "1"
        child.environment = environment
        let pipe = Pipe()
        child.standardOutput = pipe
        child.standardError = pipe
        child.standardInput = FileHandle.nullDevice
        pipe.fileHandleForReading.readabilityHandler = { [weak self] handle in
            let data = handle.availableData
            if data.isEmpty { handle.readabilityHandler = nil }
            DispatchQueue.main.async { self?.receive(data) }
        }
        child.terminationHandler = { [weak self] finished in
            let status = finished.terminationStatus
            DispatchQueue.main.async {
                self?.exitStatus = status
                self?.finishIfDone()
            }
        }
        do {
            try child.run()
        } catch {
            pipe.fileHandleForReading.readabilityHandler = nil
            onLine = nil
            onDone = nil
            return false
        }
        process = child
        return true
    }

    func cancel() {
        if let process, process.isRunning { process.terminate() }
    }

    private func receive(_ data: Data) {
        if data.isEmpty {
            outputEnded = true
            finishIfDone()
            return
        }
        for text in splitter.append(decoder.append(data)) { onLine?(text) }
    }

    private func finishIfDone() {
        guard outputEnded, let status = exitStatus, process != nil else { return }
        if let rest = splitter.finish() { onLine?(rest) }
        process = nil
        let done = onDone
        onLine = nil
        onDone = nil
        done?(status)
    }
}

/// FERB.app's own grants, read and requested by FERB.app itself.
@MainActor
final class SystemAppPermissions: AppPermissionsProviding {
    private var center: UNUserNotificationCenter? { AppBundle.isApp ? UNUserNotificationCenter.current() : nil }

    func notifications() async -> GrantStatus {
        guard let center else { return .unknown }
        return Self.grant(await center.notificationSettings().authorizationStatus)
    }

    func requestNotifications() async -> GrantStatus {
        guard let center else { return .unknown }
        _ = try? await center.requestAuthorization(options: [.alert, .sound])
        return await notifications()
    }

    func microphone() -> GrantStatus {
        GrantStatus.both(Self.grant(AVCaptureDevice.authorizationStatus(for: .audio)),
                         Self.grant(SFSpeechRecognizer.authorizationStatus()))
    }

    var microphoneDeniedPane: String {
        Self.grant(AVCaptureDevice.authorizationStatus(for: .audio)) == .denied ? "microphone" : "speech_recognition"
    }

    func requestMicrophone() async -> GrantStatus {
        if AVCaptureDevice.authorizationStatus(for: .audio) == .notDetermined {
            _ = await AVCaptureDevice.requestAccess(for: .audio)
        }
        if SFSpeechRecognizer.authorizationStatus() == .notDetermined {
            _ = await withCheckedContinuation { continuation in
                SFSpeechRecognizer.requestAuthorization { continuation.resume(returning: $0) }
            }
        }
        return microphone()
    }

    static func grant(_ status: UNAuthorizationStatus) -> GrantStatus {
        switch status {
        case .authorized, .provisional: return .granted
        case .denied: return .denied
        case .notDetermined: return .notDetermined
        @unknown default: return .unknown
        }
    }

    static func grant(_ status: AVAuthorizationStatus) -> GrantStatus {
        switch status {
        case .authorized: return .granted
        case .denied, .restricted: return .denied
        case .notDetermined: return .notDetermined
        @unknown default: return .unknown
        }
    }

    static func grant(_ status: SFSpeechRecognizerAuthorizationStatus) -> GrantStatus {
        switch status {
        case .authorized: return .granted
        case .denied, .restricted: return .denied
        case .notDetermined: return .notDetermined
        @unknown default: return .unknown
        }
    }
}

@MainActor
final class SystemOpener: SystemOpening {
    func open(_ url: URL) { NSWorkspace.shared.open(url) }
    func reveal(_ path: String) { NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)]) }
}

/// An API key goes from the setup window straight into the Keychain (L34 C, rule 14): written on `security`'s stdin
/// (`security -i`), never as an argument other processes could read, never to the daemon, a log or FERB's settings. The
/// item is the one the daemon's runtimes read (ferb/runtimes/keys.py), so the daemon's runtimes read it the same way. Only its presence is checked
/// afterwards, never its value.
@MainActor
struct KeychainAPIKeyStore: APIKeyStoring {
    func store(provider: APIProvider, key: String) async -> Bool {
        guard let command = APIKeyEntry.command(provider: provider, key: key) else { return false }
        let account = APIKeyEntry.account(provider)
        return await Task.detached {
            let writer = Process()
            writer.executableURL = URL(fileURLWithPath: "/usr/bin/security")
            writer.arguments = ["-i"]
            let input = Pipe()
            writer.standardInput = input
            writer.standardOutput = FileHandle.nullDevice
            writer.standardError = FileHandle.nullDevice
            do { try writer.run() } catch { return false }
            try? input.fileHandleForWriting.write(contentsOf: Data((command + "quit\n").utf8))
            try? input.fileHandleForWriting.close()
            writer.waitUntilExit()
            let check = Process()
            check.executableURL = URL(fileURLWithPath: "/usr/bin/security")
            check.arguments = ["find-generic-password", "-s", "ferb", "-a", account]   // no -w: presence only
            check.standardOutput = FileHandle.nullDevice
            check.standardError = FileHandle.nullDevice
            do { try check.run() } catch { return false }
            check.waitUntilExit()
            return check.terminationStatus == 0
        }.value
    }
}

/// FERB's background service through launchd (`LaunchctlPlan`): restarted after a Screen Recording or Full Disk Access
/// grant (macOS applies those to a new process), and by the menu's Restart (Phase D). Running tasks stop. The service is
/// the user LaunchAgent (DMN-513), loaded again from its plist when launchd has no job to restart.
@MainActor
final class DaemonService: DaemonServicing {
    private let plist = FileManager.default.homeDirectoryForCurrentUser
        .appendingPathComponent("Library/LaunchAgents/com.ferb.daemon.plist")

    func restart() async -> Bool {
        let commands = LaunchctlPlan.restart(uid: getuid(), plist: plist.path,
                                             plistExists: FileManager.default.fileExists(atPath: plist.path))
        for command in commands where await Self.launchctl(command) == 0 { return true }
        return false
    }

    /// `launchctl kickstart` without -k: launchd starts the job only when it isn't running. A clean exit (SIGTERM, as
    /// macOS's "Quit & Reopen" may send) isn't restarted by launchd (KeepAlive SuccessfulExit false).
    func start() async -> Bool {
        await Self.launchctl(LaunchctlPlan.start(uid: getuid())) == 0
    }

    private static func launchctl(_ arguments: [String]) async -> Int32 {
        await Task.detached {
            let process = Process()
            process.executableURL = URL(fileURLWithPath: "/bin/launchctl")
            process.arguments = arguments
            process.standardOutput = FileHandle.nullDevice
            process.standardError = FileHandle.nullDevice
            do { try process.run() } catch { return -1 }
            process.waitUntilExit()
            return process.terminationStatus
        }.value
    }
}
