import Foundation
import NotchCore

/// Insomnia mode in the notch and the menu bar (APP-980): the daemon's status as FERB.app shows it, the owner's start
/// (after the warning, InsomniaDialog.swift) and end. The cover itself is InsomniaCover.swift; the logic is NotchCore's
/// InsomniaController.
@MainActor
extension NotchModel {
    /// The notch's line shows while the mode is on (the daemon's line, with End).
    var showsInsomniaLine: Bool { insomniaStatus?.on == true }

    var insomniaLine: String {
        let line = insomniaStatus?.line ?? ""
        return line.isEmpty ? InsomniaWords.workingLine : line
    }

    /// A status from the daemon reached the controller.
    func insomniaChanged(_ status: InsomniaStatus) {
        let showed = showsInsomniaLine
        changeIfNeeded(\.insomniaStatus, status)
        if let powerHelper { Task { await powerHelper.modeChanged(status) } }   // a phone start registers it too
        // the end's words ("Welcome back. …"), said once here, quietly, as FERB's own line in the chat
        if !status.on, let end = status.lastEnd, !end.text.isEmpty {
            noteFromSetup(id: "insomnia-end-" + end.id, text: end.text)
        }
        if showed != showsInsomniaLine { onBackgroundChange?(true) }
    }

    /// At each (re)connect: the mode as the daemon has it, so a restarted FERB.app raises its cover again. A daemon
    /// without insomnia mode answers an error: nothing shows.
    func refreshInsomnia() async {
        guard let reply = try? await rpc.call("insomnia.status", params: [:]) else { return }
        insomnia.apply(reply)
    }

    /// The warning's Keep it working. With the cover, FERB.app asks for the camera now (for the phone's Show me), when
    /// the owner is at the Mac and before the cover goes up, never at the moment of an alert.
    func startInsomnia(_ choice: InsomniaChoice) {
        Task {
            if choice.variant == .shield { await InsomniaCamera.askAccessIfNeeded() }
            do { try await insomnia.start(choice) } catch { promptError = error.localizedDescription }
        }
    }

    /// The notch's End, the menu bar's End keeping my Mac working.
    func endInsomnia() {
        Task {
            do { try await insomnia.end() } catch { promptError = error.localizedDescription }
        }
    }
}
