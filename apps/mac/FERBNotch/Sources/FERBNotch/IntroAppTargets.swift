import AppKit
import NotchCore

// Where the intro's orb points for a browser while Pair all runs (kb area app step 4): beside that browser's front
// window. Try's working step sends no orb anywhere (APP-693), so no app a model named is looked up here.

/// Finds where a browser is on screen, matching the names setup.browsers gave to running apps' names (a fact lookup);
/// the orb only points: nothing here activates, opens or launches anything.
@MainActor
protocol IntroAppLocating {
    /// The app owning the frontmost on-screen layer-0 window among `apps` (named as macOS shows them): a point beside
    /// that app's largest window.
    func frontWindow(of apps: [String]) -> CGPoint?
}

/// The real lookup, from injected sources (the window server, running apps, the screens).
@MainActor
struct IntroAppResolver: IntroAppLocating {
    struct RunningApp: Equatable {
        var pid: Int32
        var name: String
        var bundleID: String?
        var bundlePath: String?
    }

    var running: () -> [RunningApp] = {
        NSWorkspace.shared.runningApplications.map {
            RunningApp(pid: $0.processIdentifier, name: $0.localizedName ?? "", bundleID: $0.bundleIdentifier,
                       bundlePath: $0.bundleURL?.path)
        }
    }
    /// Front to back, as the window server lists them.
    var windows: () -> [RawWindow] = { SettingsWindowTracker.systemWindows() }
    var screens: () -> [(frame: CGRect, visible: CGRect)] = {
        NSScreen.screens.map { (frame: $0.frame, visible: $0.visibleFrame) }
    }
    var primaryHeight: () -> CGFloat = { NSScreen.screens.first { $0.frame.origin == .zero }?.frame.height ?? 0 }

    func frontWindow(of names: [String]) -> CGPoint? {
        let pids = Set(running().filter { app in
            names.contains { $0.caseInsensitiveCompare(app.name) == .orderedSame }
        }.map(\.pid))
        guard let front = windows().first(where: { $0.layer == 0 && pids.contains($0.pid) }) else { return nil }
        return beside(windowOf: [front.pid])
    }

    /// Beside the largest on-screen layer-0 window of these processes (SettingsAnchor's window rules and flip); a
    /// window too wide to stand beside gets the orb just inside its top edge.
    private func beside(windowOf pids: Set<Int32>) -> CGPoint? {
        let orb = OrbController.size
        let infos = SettingsAnchor.windows(windows(), owner: "app", primaryHeight: primaryHeight(), frontmost: false) {
            pids.contains($0)
        }
        guard let w = SettingsAnchor.largestWindow(of: "app", in: infos) else { return nil }
        // Fallback: a window on no known screen is its own bounds (the orb then stands just inside its top edge).
        let screen = SettingsAnchor.screen(holding: w, in: screens()) ?? w.bounds
        return SettingsAnchor.beside(w, orb: orb, screen: screen)
            ?? CGPoint(x: w.bounds.midX, y: w.bounds.maxY - orb.height * 1.5)
    }
}
