import AppKit
import NotchCore

/// What drives Sparkle (SparkleUpdater), so the coordinator is testable without it.
@MainActor
protocol UpdaterDriving: AnyObject {
    var automaticallyDownloadsUpdates: Bool { get set }
    /// Sparkle's own window: the owner checks now, or installs an update FERB offered.
    func checkForUpdates()
}

/// FERB's automatic updates (L34 Phase G). Sparkle finds, downloads and verifies an update; this decides when it may
/// install: only once FERB's daemon says nothing runs that a restart would cut (`update.prepare`), which also stops the
/// daemon, so nothing runs from FERB.app while Sparkle replaces it. The new app's quiet service update starts it again.
@MainActor
final class UpdateCoordinator: ObservableObject {
    enum Status: Equatable {
        case quiet
        case upToDate
        case available(String)
        case waiting(version: String, work: [String])
        case installing(String)
        case failed(String)

        var line: String? {
            switch self {
            case .quiet: return nil
            case .upToDate: return "FERB is up to date."
            case .available(let version): return "Update available: FERB \(version)"
            case .waiting(let version, let work): return UpdatePlan.waitingLine(version: version, work: work)
            case .installing(let version): return "Installing FERB \(version)…"
            case .failed(let message): return "The update didn't install: \(message)"
            }
        }
    }

    @Published private(set) var status: Status = .quiet
    /// The last check, download or install result (APP-658); kept in `defaults` when given (FERB.app's own).
    @Published private(set) var lastResult: UpdateResult?
    private let defaults: UserDefaults?
    var now: () -> Date = Date.init
    /// Settings → General → Install updates automatically (Sparkle keeps it; default on from the build's Info.plist).
    @Published var automatic: Bool { didSet { driver?.automaticallyDownloadsUpdates = automatic } }

    let rpc: DaemonRPCTransport
    weak var driver: UpdaterDriving?
    /// Starts FERB's background service again when an update it stopped for didn't install (plan C's DaemonService).
    var restartDaemon: @MainActor () -> Void = { Task { _ = await DaemonService().restart() } }
    /// Between asks while a task runs, and how long the daemon gets to close its socket.
    var retryInterval: Duration = .seconds(60)
    var exitWait: Duration = .seconds(60)
    /// The owner's answer when they quit while a task runs and an update waits: true is Quit and Install.
    var confirmQuit: @MainActor (_ version: String, _ work: [String]) -> Bool = { version, work in
        UpdateCoordinator.askBeforeQuitting(version: version, work: work)
    }

    private(set) var pendingVersion: String?
    private var pendingInstall: (() -> Void)?
    /// FERB's daemon stopped for this update (or wasn't running).
    private(set) var prepared = false
    private var installTask: Task<Void, Never>?

    init(rpc: DaemonRPCTransport, driver: UpdaterDriving? = nil, defaults: UserDefaults? = nil) {
        self.rpc = rpc
        self.driver = driver
        self.defaults = defaults
        automatic = driver?.automaticallyDownloadsUpdates ?? true
        lastResult = UpdateResult(defaults?.object(forKey: UpdateResult.key))
    }

    /// Kept until the next result replaces it, and written to the system log as well.
    private func record(_ text: String, failed: Bool) {
        let result = UpdateResult(at: now(), failed: failed, text: text)
        lastResult = result
        defaults?.set(result.stored, forKey: UpdateResult.key)
        NSLog("FERB update: %@", text)
    }

    func attach(_ driver: UpdaterDriving) {
        self.driver = driver
        automatic = driver.automaticallyDownloadsUpdates
    }

    /// Sparkle downloaded an update and will install it when FERB.app quits.
    var installArmed: Bool { pendingInstall != nil }

    func found(version: String) {
        record("Found FERB \(version).", failed: false)
        guard pendingInstall == nil, !prepared else { return }
        status = automatic ? .quiet : .available(version)
    }

    func notFound() {
        record("FERB was up to date.", failed: false)
        guard pendingInstall == nil, !prepared else { return }
        status = .upToDate
    }

    /// Settings or the menu: Install for an offered update, or Check for Updates…: Sparkle's own window.
    func checkNow() { driver?.checkForUpdates() }

    /// Sparkle is ready to install `version`; `install` replaces FERB.app and relaunches it.
    @discardableResult
    func readyToInstall(version: String, install: @escaping () -> Void) -> Task<Void, Never> {
        record("FERB \(version) downloaded and verified.", failed: false)
        pendingVersion = version
        pendingInstall = install
        installTask?.cancel()
        let job = Task { [weak self] in
            if let self { await self.installWhenIdle() }
        }
        installTask = job
        return job
    }

    private func installWhenIdle() async {
        while let install = pendingInstall, !Task.isCancelled {
            let version = pendingVersion ?? ""
            switch await prepare(evenIfBusy: false) {
            case .ready:
                status = .installing(version)
                pendingInstall = nil
                install()
                return
            case .waiting(let work):
                status = .waiting(version: version, work: work)
                try? await Task.sleep(for: retryInterval)
            }
        }
    }

    /// Asks FERB's daemon to stop for the update; `.ready` means it stopped (or wasn't running).
    func prepare(evenIfBusy: Bool) async -> UpdatePlan.Prepared {
        var params: [String: Any] = ["version": pendingVersion ?? ""]
        if evenIfBusy { params["even_if_busy"] = true }
        do {
            let outcome = UpdatePlan.prepared(try await rpc.call("update.prepare", params: params))
            if outcome == .ready {
                prepared = true
                await waitForDaemonExit()
            }
            return outcome
        } catch DaemonRPCError.unavailable {
            prepared = true                              // FERB isn't running: no task can be
            return .ready
        } catch {
            return .waiting(["FERB couldn't say whether a task is running (\(error.localizedDescription))"])
        }
    }

    /// Until FERB's socket is gone (its shutdown ends by closing it), at most `exitWait`.
    private func waitForDaemonExit() async {
        let clock = ContinuousClock()
        let end = clock.now.advanced(by: exitWait)
        while clock.now < end {
            do { _ = try await rpc.call("ping", params: [:]) } catch { return }
            try? await Task.sleep(for: .milliseconds(500))
        }
    }

    /// Sparkle gave up: an unreachable feed, a failed download, a refused signature or certificate, a failed install.
    /// Always recorded, a scheduled background check's included (APP-658); the status line changes only for an
    /// install FERB had armed or stopped for.
    func aborted(_ message: String) {
        record("Failed: \(message)", failed: true)
        let stoppedFERB = prepared
        if stoppedFERB { restartDaemon() }               // stopped for nothing: start it again now
        if stoppedFERB || pendingInstall != nil { status = .failed(message) }
        prepared = false
        pendingInstall = nil
        installTask?.cancel()
    }

    /// The owner quits FERB.app while Sparkle waits to install on quit: only with nothing running, unless they choose
    /// Quit and Install. `reply(true)` lets the quit go on.
    func beforeQuit(_ reply: @escaping (Bool) -> Void) {
        guard installArmed, !prepared else { reply(true); return }
        Task {
            switch await prepare(evenIfBusy: false) {
            case .ready:
                reply(true)
            case .waiting(let work):
                if confirmQuit(pendingVersion ?? "", work) {
                    _ = await prepare(evenIfBusy: true)
                    reply(true)
                } else {
                    reply(false)
                }
            }
        }
    }

    static func askBeforeQuitting(version: String, work: [String]) -> Bool {
        let alert = NSAlert()
        alert.messageText = "FERB \(version) is ready to install"
        alert.informativeText = "FERB is still working on: " + work.joined(separator: "; ") +
            ". Quitting now stops it and installs the update; you can ask FERB to pick it up again after."
        alert.addButton(withTitle: "Keep FERB Open")
        alert.addButton(withTitle: "Quit and Install")
        NSApp.activate(ignoringOtherApps: true)
        return alert.runModal() == .alertSecondButtonReturn
    }
}
