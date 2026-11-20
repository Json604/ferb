import Foundation
import NotchCore
import ServiceManagement

/// Insomnia mode's root helper as macOS knows it (APP-986): an SMAppService daemon for FERB.app's bundled
/// Contents/Library/LaunchDaemons/com.ferb.power.plist. Tests stand in their own; nothing real is registered there.
@MainActor
protocol PowerHelperRegistering: AnyObject {
    var status: ServiceStatus { get }
    /// The bundled definition registered (already registered counts as done).
    func register() throws
    /// That registration removed, waited for.
    func unregister() async throws
    func openLoginItems()
}

@MainActor
final class PowerHelperService: PowerHelperRegistering {
    private let service = SMAppService.daemon(plistName: PowerHelperPlan.plistName)

    var status: ServiceStatus {
        switch service.status {
        case .enabled: return .enabled
        case .requiresApproval: return .requiresApproval
        case .notFound: return .notFound
        case .notRegistered: return .notRegistered
        @unknown default: return .notRegistered
        }
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

/// FERB.app's part with the helper: registered when the owner turns insomnia mode on from the Mac and macOS doesn't
/// list it yet, what the warning says about a closed lid on battery, and Uninstall. Registering never blocks the start.
@MainActor
final class PowerHelper {
    private let registrar: PowerHelperRegistering

    init(registrar: PowerHelperRegistering) { self.registrar = registrar }

    /// Before the warning shows: registers a helper macOS doesn't list (an error leaves it unregistered; the mode
    /// starts either way), then says what a closed lid on battery does now.
    func prepare() -> PowerHelperPlan.ClosedLid {
        if PowerHelperPlan.registers(registrar.status) { try? registrar.register() }
        return PowerHelperPlan.closedLid(registrar.status)
    }

    func openLoginItems() { registrar.openLoginItems() }

    /// Uninstall's step, like the background service's (APP-5): the helper unregistered, waited for and checked. Nil
    /// when macOS lists none afterwards, else why not (nothing is uninstalled then).
    func unregisterForUninstall() async -> String? {
        guard registrar.status.listed else { return nil }
        do { try await registrar.unregister() } catch { return InsomniaWords.helperNotRemoved(error.localizedDescription) }
        return registrar.status.listed ? InsomniaWords.helperNotRemoved(nil) : nil
    }
}
