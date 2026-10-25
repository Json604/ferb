import SwiftUI
import NotchCore

extension NotchModel {
    static let lastBuildKey = "FERBLastLaunchedBuild"

    /// L34 Phase G: once per new build, after its setup settled, the notch says "Updated to <version>".
    func noteUpdate(info: [String: Any] = Bundle.main.infoDictionary ?? [:]) {
        let build = info["CFBundleVersion"] as? String ?? ""
        let version = info["CFBundleShortVersionString"] as? String ?? ""
        let previous = preferences.string(forKey: Self.lastBuildKey)
        if !build.isEmpty { preferences.set(build, forKey: Self.lastBuildKey) }
        guard let note = UpdatePlan.updatedNote(previousBuild: previous, build: build, version: version) else { return }
        if conversation.addNotice(id: "update-" + build, text: note, tag: "FERB") {
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
        if case .available = updates.status {
            Button("Install") { updates.checkNow() }
        } else {
            Button("Check for Updates…") { updates.checkNow() }
        }
        Text("FERB asks only GitHub's public release list; nothing about you is sent. An update installs when no task " +
             "is running, then FERB's background service restarts.")
            .font(.caption).foregroundStyle(.secondary)
    }
}
