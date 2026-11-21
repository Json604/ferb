import Foundation
import NotchCore
import ServiceManagement

/// Insomnia mode's root helper as macOS knows it (APP-986): an SMAppService daemon for FERB.app's bundled
/// Contents/Library/LaunchDaemons/com.ferb.power.plist, running Contents/Helpers/ferb-power. Tests stand in their own;
/// nothing real is registered there.
@MainActor
protocol PowerHelperRegistering: AnyObject {
    var status: ServiceStatus { get }
    /// The bundle carries the helper: its plist and its program are on disk.
    var inBundle: Bool { get }
    /// The bundled definition registered (already registered counts as done).
    func register() throws
    /// That registration removed, waited for.
    func unregister() async throws
    func openLoginItems()
}

@MainActor
final class PowerHelperService: PowerHelperRegistering {
    private let service = SMAppService.daemon(plistName: PowerHelperPlan.plistName)
    private let bundle: URL

    init(bundle: URL = Bundle.main.bundleURL) { self.bundle = bundle }

    var status: ServiceStatus {
        switch service.status {
        case .enabled: return .enabled
        case .requiresApproval: return .requiresApproval
        case .notFound: return .notFound
        case .notRegistered: return .notRegistered
        @unknown default: return .notRegistered
        }
    }

    var inBundle: Bool {
        let files = FileManager.default
        return files.fileExists(atPath: bundle.appendingPathComponent("Contents/Library/LaunchDaemons/"
                                                                      + PowerHelperPlan.plistName).path)
            && files.fileExists(atPath: bundle.appendingPathComponent("Contents/Helpers/ferb-power").path)
    }

    func register() throws {
        do {
            try service.register()
        } catch let error as NSError where error.code == kSMErrorAlreadyRegistered {
            return
        }
    }

    func unregister() async throws { try await service.unregister() }

    func openLoginItems() { SMAppService.openSystemSettingsLoginItems() }
}

/// FERB.app's part with the helper: registered when insomnia mode is turned on (the Mac's warning, or any mode
/// session the daemon reports on), what the warning says about a closed lid, and Uninstall. Every attempt is logged and
/// told to the daemon (`insomnia.report {"event": "helper"}`), errors with their words; registering never blocks the
/// start.
@MainActor
final class PowerHelper {
    private let registrar: PowerHelperRegistering
    /// Sends one `insomnia.report`; the app sends it to the daemon (tests record it).
    private let report: @MainActor ([String: Any]) async -> Void
    /// Where an attempt's line goes (NSLog; tests stand in their own).
    var log: (String) -> Void = { NSLog("%@", $0) }

    init(registrar: PowerHelperRegistering, report: @escaping @MainActor ([String: Any]) async -> Void) {
        self.registrar = registrar
        self.report = report
    }

    /// Registers when the bundle carries the helper and macOS doesn't list it; logs and reports what happened (the
    /// state read again after a registration). Returns what a closed lid does now.
    @discardableResult
    func attempt() async -> PowerHelperPlan.ClosedLid {
        let inBundle = registrar.inBundle
        var params: [String: Any] = ["event": "helper"]
        if !inBundle {
            params["state"] = "not_in_bundle"
            log("FERB power helper: not in this bundle, not registered")
        } else if PowerHelperPlan.registers(registrar.status, inBundle: true) {
            do {
                try registrar.register()
                params["state"] = PowerHelperPlan.reported(registrar.status)
                log("FERB power helper: registered, now \(params["state"] ?? "")")
            } catch {
                let words = PowerHelperPlan.describe(error)
                params["state"] = "failed"
                params["error"] = words
                log("FERB power helper: register failed: " + words)
            }
        } else {
            params["state"] = PowerHelperPlan.reported(registrar.status)
        }
        await report(params)
        return PowerHelperPlan.closedLid(registrar.status, inBundle: inBundle)
    }

    /// Before the warning shows (the owner turning the mode on from the Mac).
    func prepare() async -> PowerHelperPlan.ClosedLid { await attempt() }

    func openLoginItems() { registrar.openLoginItems() }

    /// The session last tried (its `since`).
    private(set) var triedSession: String?

    /// Every status from the daemon: a session that is on tries once, whoever started it, while the daemon doesn't
    /// have the helper ready; macOS shows its own notice.
    func modeChanged(_ status: InsomniaStatus) async {
        guard PowerHelperPlan.triesForSession(status, tried: triedSession) else { return }
        triedSession = status.since
        await attempt()
    }

    /// Uninstall's step, like the background service's (APP-5): the helper unregistered, waited for and checked. Nil
    /// when macOS lists none afterwards, else why not (nothing is uninstalled then).
    func unregisterForUninstall() async -> String? {
        guard registrar.status.listed else { return nil }
        do { try await registrar.unregister() } catch { return InsomniaWords.helperNotRemoved(error.localizedDescription) }
        return registrar.status.listed ? InsomniaWords.helperNotRemoved(nil) : nil
    }
}
