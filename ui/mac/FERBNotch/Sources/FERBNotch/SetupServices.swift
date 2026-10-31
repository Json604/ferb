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
}

/// FERB's background service as FERB.app registers it with macOS (notes/daemon.md decision 2): its definition is inside
/// the signed app, so nothing can edit it, deleting the app removes it, and Login Items lists it under FERB.
@MainActor
protocol BackgroundServiceRegistering: AnyObject {
    var status: ServiceStatus { get }
    func register() throws
    func unregister() async throws
    func openLoginItems()
}

/// SMAppService.agent for Contents/Library/LaunchAgents/com.ferb.daemon.plist.
@MainActor
final class AppServiceAgent: BackgroundServiceRegistering {
    private let service = SMAppService.agent(plistName: ServicePlan.plistName)

    var status: ServiceStatus {
        switch service.status {
        case .enabled: return .enabled
        case .requiresApproval: return .requiresApproval
        case .notFound: return .notFound
        case .notRegistered: return .notRegistered
        @unknown default: return .notRegistered
        }
    }

    /// Registering again is harmless: macOS reports it as already registered, which counts as on.
    func register() throws {
        do { try service.register() } catch let error as NSError where error.code == kSMErrorAlreadyRegistered {}
    }

    func unregister() async throws {
        guard service.status != .notRegistered else { return }
        try await service.unregister()
    }

    func openLoginItems() { SMAppService.openSystemSettingsLoginItems() }
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
/// the app's registered one (an old user LaunchAgent is loaded only while one is still there).
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
