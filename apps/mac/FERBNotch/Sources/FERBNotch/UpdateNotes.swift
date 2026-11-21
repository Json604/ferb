import SwiftUI
import NotchCore

extension NotchModel {
    static let lastBuildKey = "FERBLastLaunchedBuild"
    static let lastVersionKey = "FERBLastLaunchedVersion"

    /// L34 Phase G, APP-749: once per new version, after its setup settled, the notch says "Updated to <version>"; a
    /// relaunch, reinstall or rebuild of the same version says nothing.
    func noteUpdate(info: [String: Any] = Bundle.main.infoDictionary ?? [:]) {
        let build = info["CFBundleVersion"] as? String ?? ""
        let version = info["CFBundleShortVersionString"] as? String ?? ""
        let previousBuild = preferences.string(forKey: Self.lastBuildKey)
        let previousVersion = preferences.string(forKey: Self.lastVersionKey)
        if !build.isEmpty { preferences.set(build, forKey: Self.lastBuildKey) }
        if !version.isEmpty { preferences.set(version, forKey: Self.lastVersionKey) }
        let id = "update-" + version
        // An older FERB keyed its line by build ("update-<build>"): the same version's line already in the chat counts
        let said = conversation.notices.contains {
            $0.id == id || ($0.id.hasPrefix("update-") && $0.text == "Updated to \(version)")
        } || alerts.contains { $0.id == id }
        guard let note = UpdatePlan.updatedNote(previousVersion: previousVersion, previousBuild: previousBuild,
                                                build: build, version: version, alreadySaid: said) else { return }
        showOwnAlert(id: id, text: note)      // an alert above the composer, never kept in the chat (APP-1107)
    }

    /// A line setup writes for the owner (APP-921: a restart a running task defers), as one of FERB's alerts above the
    /// composer, never kept in the chat (APP-1107), once per id.
    func noteFromSetup(id: String, text: String) {
        showOwnAlert(id: id, text: text)
    }

    /// The switch to the Developer ID waits for the owner (APP-903): FERB asks once per version in the chat; the
    /// menu's entry and Settings → General → Updates install it.
    func noteUpdateAsk(version: String) {
        showOwnAlert(id: "update-ask-" + version, text: UpdatePlan.askLine(version: version) +
                     " Install it from FERB's menu or Settings → General → Updates.")
    }
}

/// Settings → General → Updates.
struct UpdatesSection: View {
    @ObservedObject var updates: UpdateCoordinator

    var body: some View {
        Toggle("Install updates automatically", isOn: $updates.automatic)
        if let line = updates.status.line { Text(line).textSelection(.enabled) }
        // Sparkle's reason, small and quiet: the line above says it plainly (APP-946)
        if let last = updates.lastResult {
            Text("Last update check, \(last.at.formatted(date: .abbreviated, time: .shortened)): \(last.text)")
                .font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
        }
        switch updates.button {
        case .install(let title): Button(title) { updates.install() }
        case .checking:
            HStack(spacing: 6) {
                ProgressView().controlSize(.small)
                Button(updates.button.title) {}.disabled(true)
            }
        case .check: Button(updates.button.title) { updates.check() }
        }
        Text("FERB asks only GitHub's public release list; nothing about you is sent. An update installs when no task " +
             "is running, then FERB's background service restarts.")
            .font(.caption).foregroundStyle(.secondary)
    }
}
