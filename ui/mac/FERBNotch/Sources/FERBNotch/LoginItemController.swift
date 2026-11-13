import AppKit
import ServiceManagement
import SwiftUI
import NotchCore

/// Something that can make FERB.app open at login (L34 Phase F).
@MainActor
protocol LoginItemService {
    var enabled: Bool { get }
    /// macOS registered the item but waits for the owner's OK in System Settings → General → Login Items.
    var needsApproval: Bool { get }
    func enable() throws
    func disable() throws
}

/// macOS's own login item for this app.
@MainActor
struct AppServiceLoginItem: LoginItemService {
    var enabled: Bool { SMAppService.mainApp.status == .enabled }
    var needsApproval: Bool { SMAppService.mainApp.status == .requiresApproval }
    func enable() throws { try SMAppService.mainApp.register() }
    func disable() throws { try SMAppService.mainApp.unregister() }
}

/// The fallback when macOS refuses the login item without a Developer ID: a user LaunchAgent that opens FERB.app in
/// the background at login. launchd loads it at the next login; nothing starts now. Only FERB's own file is removed.
@MainActor
struct LaunchAgentLoginItem: LoginItemService {
    let plist: URL
    let appPath: String

    var enabled: Bool { LoginItemPlan.isOurs(NSDictionary(contentsOf: plist) as? [String: Any]) }
    var needsApproval: Bool { false }

    func enable() throws {
        try FileManager.default.createDirectory(at: plist.deletingLastPathComponent(), withIntermediateDirectories: true)
        let data = try PropertyListSerialization.data(fromPropertyList: LoginItemPlan.agent(appPath: appPath),
                                                      format: .xml, options: 0)
        try data.write(to: plist, options: .atomic)
    }

    func disable() throws {
        guard enabled else { return }
        try FileManager.default.removeItem(at: plist)
    }
}

/// Open FERB at login (L34 Phase F): macOS's login item first; the LaunchAgent only when macOS refuses it, and the
/// note says so. On by default once, after setup; then only the owner's switch decides.
@MainActor
final class LoginItemController: ObservableObject {
    static let defaultAppliedKey = "loginItem.defaultApplied"
    static let refusalKey = "loginItem.refusal"

    @Published private(set) var enabled = false
    @Published private(set) var needsApproval = false
    @Published private(set) var note: String?
    /// The installed FERB.app only; a development build registers nothing.
    let available: Bool

    private let appService: LoginItemService
    private let launchAgent: LoginItemService
    private let defaults: UserDefaults

    init(appService: LoginItemService, launchAgent: LoginItemService, defaults: UserDefaults = .standard,
         available: Bool) {
        self.appService = appService
        self.launchAgent = launchAgent
        self.defaults = defaults
        self.available = available
        refresh()
    }

    static func installed(available: Bool) -> LoginItemController {
        let agents = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/LaunchAgents")
        let agent = LaunchAgentLoginItem(plist: agents.appendingPathComponent(LoginItemPlan.label + ".plist"),
                                         appPath: Bundle.main.bundleURL.resolvingSymlinksInPath().path)
        return LoginItemController(appService: AppServiceLoginItem(), launchAgent: agent, available: available)
    }

    func refresh() {
        enabled = appService.enabled || launchAgent.enabled
        needsApproval = appService.needsApproval
        if needsApproval { note = LoginItemPlan.approvalNote }
        else if launchAgent.enabled { note = LoginItemPlan.fallbackNote(defaults.string(forKey: Self.refusalKey)) }
        else { note = nil }
    }

    func setEnabled(_ on: Bool) {
        guard available else { return }
        note = nil
        if on { turnOn() } else { turnOff() }
        let failure = note                             // turnOn's "couldn't" outlasts refresh()
        refresh()
        if let failure { note = failure }
    }

    private func turnOn() {
        do {
            try appService.enable()
            guard appService.enabled || appService.needsApproval else { throw LoginItemPlan.Refused() }
            try? launchAgent.disable()                 // macOS accepted it: the fallback would open FERB twice
            defaults.removeObject(forKey: Self.refusalKey)
        } catch {
            defaults.set(error.localizedDescription, forKey: Self.refusalKey)
            do {
                try launchAgent.enable()
            } catch let failure {
                note = "FERB couldn't set itself to open at login: \(failure.localizedDescription)"
            }
        }
    }

    private func turnOff() {
        try? appService.disable()
        try? launchAgent.disable()
    }

    /// On by default once, after setup: a later "off" (here or in System Settings) is never undone.
    func applyDefaultAfterSetup() {
        guard available, LoginItemPlan.shouldApplyDefault(applied: defaults.bool(forKey: Self.defaultAppliedKey)) else {
            return
        }
        defaults.set(true, forKey: Self.defaultAppliedKey)
        setEnabled(true)
    }

    /// Settings → Advanced → Uninstall: the login item goes with the background service.
    func removeForUninstall() {
        turnOff()
        refresh()
    }

    func openSystemSettings() { SMAppService.openSystemSettingsLoginItems() }
}

/// Settings → General → Startup.
struct LoginItemRow: View {
    @ObservedObject var loginItems: LoginItemController

    var body: some View {
        Toggle("Open FERB at login", isOn: Binding(get: { loginItems.enabled }, set: { loginItems.setEnabled($0) }))
            .disabled(!loginItems.available)
        if let note = loginItems.note { Text(note).font(.caption).foregroundStyle(.secondary) }
        if loginItems.needsApproval { Button("Open Login Items Settings") { loginItems.openSystemSettings() } }
        if !loginItems.available {
            Text("Only the installed FERB.app opens at login.").font(.caption).foregroundStyle(.secondary)
        }
    }
}
