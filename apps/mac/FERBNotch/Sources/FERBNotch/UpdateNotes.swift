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
        }
        guard let note = UpdatePlan.updatedNote(previousVersion: previousVersion, previousBuild: previousBuild,
                                                build: build, version: version, alreadySaid: said) else { return }
        if conversation.addNotice(id: id, text: note, tag: "FERB") {
            saveConversation()
            onBackgroundChange?(true)
        }
    }

    /// The switch to the Developer ID waits for the owner (APP-903): FERB asks once per version in the chat; the
    /// menu's entry and Settings → General → Updates install it.
    func noteUpdateAsk(version: String) {
        if conversation.addNotice(id: "update-ask-" + version, text: UpdatePlan.askLine(version: version) +
                                  " Install it from FERB's menu or Settings → General → Updates.", tag: "FERB") {
            saveConversation()
            onBackgroundChange?(true)
        }
    }
}

/// Settings → General → Updates.
struct UpdatesSection: View {
    @ObservedObject var updates: UpdateCoordinator

    var body: some View {
        Toggle("Install updates automatically", isOn: $updates.automatic)
        if let line = updates.status.line { Text(line).textSelection(.enabled) }
        if let last = updates.lastResult {
            Text("Last update check, \(last.at.formatted(date: .abbreviated, time: .shortened)): \(last.text)")
                .font(.caption).foregroundStyle(last.failed ? .red : .secondary).textSelection(.enabled)
        }
        if let offer = updates.offer {
            Button(offer.asks ? "Update Now" : "Install") { updates.checkNow() }
        } else {
            Button("Check for Updates…") { updates.checkNow() }
        }
        Text("FERB asks only GitHub's public release list; nothing about you is sent. An update installs when no task " +
             "is running, then FERB's background service restarts.")
            .font(.caption).foregroundStyle(.secondary)
    }
}
