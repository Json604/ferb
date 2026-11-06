#if FERB_PLAYGROUND
import AppKit
import SwiftUI
import NotchCore

/// The product's own windows, connected to the playground transport and services.
@MainActor
final class PlaygroundAppReview: PlayHost {
    let services: PlaygroundServices
    /// Play 0.27.1: the scripted conversation through the real notch (PlaygroundPlay).
    private(set) lazy var player: PlaygroundPlayer = {
        let made = PlaygroundPlayer(workspace: services.daemon.workspace, model: coordinator.model, host: self)
        made.mode = ChatHost.setting(.standard)          // Notch | Window starts as the playground's own setting
        made.pauseBefore = PlaygroundMode.playPauseBefore
        return made
    }()
    let coordinator: PanelCoordinator
    let window: FERBWindowController
    let updates: UpdateCoordinator
    private let updater: PlaygroundUpdater
    private var previewWindow: NSWindow?
    private let openReview: () -> Void

    init(services: PlaygroundServices, coordinator: PanelCoordinator,
         openReview: @escaping () -> Void, replayIntro: @escaping () -> Void) {
        self.services = services
        self.coordinator = coordinator
        self.openReview = openReview
        let updates = UpdateCoordinator(rpc: services.daemon)
        self.updates = updates
        let updater = PlaygroundUpdater()
        self.updater = updater
        updater.onCheck = { [weak updates] in updates?.notFound() }
        updates.attach(updater)
        updates.restartDaemon = {} // the review has no installed service
        var actions = FERBAppActions(feedback: coordinator.model.feedback, cursor: coordinator.cursor)
        actions.setup = services.setup
        actions.appVersion = "Playground"
        actions.replayIntro = replayIntro
        actions.setHoverEnabled = { [weak model = coordinator.model] in model?.setHoverEnabled($0) }
        actions.hotKeys = coordinator.hotKeys
        actions.loginItems = LoginItemController(appService: PlaygroundLoginItem(), launchAgent: PlaygroundLoginItem(enabled: false), available: true)
        actions.updates = updates
        actions.logsDirectory = services.home.appendingPathComponent("logs")
        actions.openNotificationSettings = { [opener = services.opener] in
            opener.open(URL(string: "x-apple.systempreferences:com.apple.Notifications-Settings.extension")!)
        }
        actions.runFerb = { arguments, line, done in
            if arguments.contains("--cleanup") {
                let reply: [String: Any] = arguments.contains("--ok")
                    ? ["removed": ["samples": 0], "compacted": false, "bytes_before": 2048000, "bytes_after": 2048000]
                    : ["steps": [], "rows": 0, "bytes": 2048000, "needed": 2048000, "free": 100000000, "ok": "sample-cleanup"]
                if let data = try? JSONSerialization.data(withJSONObject: reply), let text = String(data: data, encoding: .utf8) { line(text) }
            } else { line("Simulated in the playground. No installed services, files or accounts were changed.") }
            done(0)
            return true
        }
        actions.beforeUninstall = { nil }
        actions.correctMemory = { [weak coordinator] text in
            coordinator?.model.prepareMemoryCorrection(text)
            coordinator?.showFromMenu()
        }
        window = FERBWindowController(store: FERBWindowStore(rpc: services.daemon), actions: actions)
        coordinator.onSetUp = replayIntro
        coordinator.hasWindow = true
        coordinator.onOpenWindow = { [weak self] in self?.show(.tasks) }
        coordinator.onOpenSettings = { [weak self] in self?.show(.settings) }
        coordinator.model.onOpenMemory = { [weak self] source in self?.window.showMemory(importedFrom: source) }
        coordinator.onInstallUpdate = { [weak updates] in updates?.checkNow() }
    }

    func prepare() throws {
        try services.daemon.workspace.prepare()
        services.state.exploringApp = true
        for id in PlaygroundState.permissions { services.state.set(id, .granted) }
        services.state.saved.runtime = "codex"
        services.state.saved.serviceInstalled = true
        services.state.saved.setupDone = true
        services.state.saved.browsersPaired = true
        services.state.saved.identityAddress = "ferb@studio.example"
        services.state.saved.identityPaired = true
        var progress = IntroProgress(defaults: .standard)
        progress.done = true
        progress.resumeBeat = nil
        progress.plan = nil
        if coordinator.model.conversation.turns.isEmpty {
            let id = UUID()
            coordinator.model.conversation.begin(id: id, request: "Can you keep my morning briefs short?", sources: [], date: Date().addingTimeInterval(-86400))
            coordinator.model.conversation.finish(id: id, answer: "Yes. I'll lead with decisions and keep the rest brief.", status: "done", artifacts: [])
            coordinator.model.saveConversation()
        }
    }

    func show(_ section: FERBSection) {
        guard services.state.exploringApp else { openReview(); return }
        window.show(section)
    }

    func play(_ scene: PlaygroundChatScenario) {
        guard services.state.exploringApp else { openReview(); return }
        services.daemon.workspace.scenario = scene
        if scene == .reminder || scene == .schedule {
            coordinator.model.collapse()
            if scene == .reminder { services.daemon.workspace.deliverReminder() }
            else { services.daemon.workspace.deliverScheduledResult() }
            services.daemon.workspace.scenario = .conversation
        } else {
            coordinator.showFromMenu()
            services.notch.send(scene.request)
        }
    }

    /// Starts Play 0.27.1 (from the top when it already finished).
    func startPlay() {
        guard services.state.exploringApp else { openReview(); return }
        if player.index >= player.count { player.restart() } else { player.play() }
    }

    // MARK: PlayHost

    func openNotch() { coordinator.showFromMenu() }
    func closeNotch() { coordinator.model.collapse() }
    func showSettings(_ tab: SettingsTab) { window.show(.settings, tab: tab) }
    func closeSettings() { window.window?.orderOut(nil) }
    func setChatHost(_ host: ChatHost) { UserDefaults.standard.set(host.rawValue, forKey: ChatHost.defaultsKey) }
    func setHidePill(_ on: Bool) { UserDefaults.standard.set(on, forKey: ChatHost.hidePillKey) }

    func showPreview() {
        guard services.state.exploringApp else { openReview(); return }
        if previewWindow == nil {
            let created = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 760, height: 480),
                styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
            created.title = "FERB — task preview"
            created.isReleasedWhenClosed = false
            created.contentView = NSHostingView(rootView: TaskVideoView(preview: services.notch.preview(task: "sample-brief")))
            created.center()
            previewWindow = created
        }
        if let previewWindow { AppWindowPresentation.show(previewWindow) }
    }
}

@MainActor
private final class PlaygroundLoginItem: LoginItemService {
    var enabled: Bool
    init(enabled: Bool = true) { self.enabled = enabled }
    var needsApproval = false
    func enable() throws { enabled = true }
    func disable() throws { enabled = false }
}

@MainActor
private final class PlaygroundUpdater: UpdaterDriving {
    var automaticallyDownloadsUpdates = true
    var onCheck: (() -> Void)?
    func checkForUpdates() { onCheck?() }
}
#endif
