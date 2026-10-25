import AppKit
import SwiftUI
import NotchCore

enum AppName {
    static var current: String { Bundle.main.object(forInfoDictionaryKey: "CFBundleName") as? String ?? "FERB" }
}

/// FERB.app's setup (L27, L34 C). A first launch shows the native setup window: Welcome installs the background service
/// from this app (`ferb setup --service`, no questions), and every later step reads and acts through the daemon. After the
/// app is replaced by a new build, or when launchd doesn't have FERB running, launch updates the service quietly
/// (`ferb setup --service --update`) and shows the window only if that fails or a required grant is missing. Another
/// installation (a checkout, or FERB.app elsewhere) is replaced only after the owner says so, from the menu too. Closing
/// the window doesn't stop a service run (Set Up FERB… shows it again); quitting FERB stops it, and setup finishes
/// reinstalling the service first. A development build, which has no Python inside, never installs anything.
@MainActor
final class AppSetupController: NSObject, NSWindowDelegate {
    private let bundle = Bundle.main.bundleURL.resolvingSymlinksInPath()
    private var window: NSWindow?
    var onFinished: (() -> Void)?
    /// Setup has nothing left to do (L34 Phase F): at launch when this build is set up, after a quiet update that
    /// needs no permission, or at the setup window's Done. The app applies the login-item default here (plan G adds
    /// "Updated to …").
    var onSettled: (() -> Void)?

    private var python: URL { bundle.appendingPathComponent("Contents/Resources/python/bin/python3.12") }
    private var agentExecutable: URL { bundle.appendingPathComponent("Contents/Helpers/FERB.app/Contents/MacOS/FERB") }
    private var launcher: URL { bundle.appendingPathComponent("Contents/Resources/bin/ferb") }
    private var agentPlist: URL {
        FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/LaunchAgents/com.ferb.daemon.plist")
    }

    lazy var model = SetupModel(rpc: DaemonRPC(), runner: SetupCommand(launcher: launcher),
                                        app: SystemAppPermissions(), system: SystemOpener(),
                                        keys: KeychainAPIKeyStore(), service: DaemonService())

    var isPackaged: Bool {
        FileManager.default.isExecutableFile(atPath: launcher.path) && FileManager.default.fileExists(atPath: python.path)
            && FileManager.default.isExecutableFile(atPath: agentExecutable.path)
    }

    func startAtLaunch() {
        guard isPackaged else { return }
        let action = currentAction()
        Task { @MainActor in
            let done = action == .none ? await self.model.setupDone() : false
            let resume = SetupModel.pendingResume(.standard)
            switch AppSetupPlan.atLaunch(action, setupDone: done, resume: resume) {
            case .nothing:
                _ = await self.model.askForNotificationsOnce(defaults: .standard)   // owner decision 6
                self.onSettled?()
            case .quietUpdate: self.quietUpdate()
            case .window(let step):
                if case .confirmReplace(let current) = action, !self.confirmReplacing(current) { return }
                self.show(step, action: action)
            }
        }
    }

    /// Set Up FERB…: the window again; Welcome reinstalls only when this app's build isn't the one running.
    func runSetupFromMenu() {
        guard isPackaged else { return }
        let action = currentAction()
        if case .confirmReplace(let current) = action, !confirmReplacing(current) { return }
        show(.welcome, action: action)
    }

    /// FERB is quitting: stop a running setup command; it finishes reinstalling the background service first.
    func stop() { model.runner.cancel() }

    private func currentAction() -> AppSetupAction {
        let agent = NSDictionary(contentsOf: agentPlist) as? [String: Any]
        let build = Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? ""
        return AppSetupPlan.decide(agent: agent, agentExecutable: agentExecutable.path, build: build,
                                   daemonRunning: daemonRunning())
    }

    private func quietUpdate() {
        model.serviceAction = .update
        model.runService { [weak self] ok in
            guard let self else { return }
            guard ok else { self.show(.welcome, action: .update); return }
            Task { @MainActor in
                if await self.model.needsPermissions() { self.show(.permissions, action: .none) }
                else {
                    _ = await self.model.askForNotificationsOnce(defaults: .standard)     // owner decision 6
                    self.onSettled?()
                }
            }
        }
    }

    /// Whether launchd has FERB's agent running now (`launchctl print` only reads launchd's state).
    private func daemonRunning() -> Bool {
        let launchctl = Process()
        launchctl.executableURL = URL(fileURLWithPath: "/bin/launchctl")
        launchctl.arguments = ["print", "gui/\(getuid())/com.ferb.daemon"]
        let pipe = Pipe()
        launchctl.standardOutput = pipe
        launchctl.standardError = FileHandle.nullDevice
        do { try launchctl.run() } catch { return false }
        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        launchctl.waitUntilExit()
        return launchctl.terminationStatus == 0 && String(decoding: data, as: UTF8.self).contains("state = running")
    }

    private func confirmReplacing(_ current: String) -> Bool {
        let alert = NSAlert()
        alert.messageText = "Set up FERB from this app?"
        alert.informativeText = "FERB's background service currently runs \(current). Setting up from this app " +
            "runs it from \(bundle.path) instead. Your data in ~/.ferb stays."
        alert.addButton(withTitle: "Set Up From This App")
        alert.addButton(withTitle: "Not Now")
        NSApp.activate(ignoringOtherApps: true)
        return alert.runModal() == .alertFirstButtonReturn
    }

    private func show(_ step: SetupStep, action: AppSetupAction) {
        if !model.serviceRunning { model.serviceAction = action }
        if window == nil {
            let created = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 560, height: 500),
                                   styleMask: [.titled, .closable, .miniaturizable], backing: .buffered, defer: false)
            created.title = "Set Up \(AppName.current)"
            created.isReleasedWhenClosed = false
            created.delegate = self
            created.contentView = NSHostingView(rootView: SetupView(model: model))
            created.center()
            window = created
            model.onFinish = { [weak self] in
                self?.window?.close()
                self?.onFinished?()
                self?.onSettled?()
            }
        }
        model.go(step)
        model.startPolling()
        NSApp.activate(ignoringOtherApps: true)          // the owner opened FERB or chose Set Up FERB…
        window?.makeKeyAndOrderFront(nil)
    }

    func windowWillClose(_ notification: Notification) {
        model.stopPolling()           // a service run keeps going: stopping it halfway could leave the service unloaded
    }
}
