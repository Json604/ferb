import AppKit
import NotchCore

/// Where the intro's orb points for an app (notes/setup-intro.md steps 4 and 6): beside the app's main window when it
/// is running and on screen, else a resting point below the notch. `iconPath`: the app's bundle, for the bubble's icon.
struct IntroAppTarget: Equatable {
    var point: CGPoint
    /// The app as the owner reads it ("" for the default browser when it couldn't be named).
    var name: String
    var iconPath: String?
    /// The point is beside one of the app's windows (not the resting point).
    var onWindow: Bool
}

/// Finds where an app is on screen. Matching the model's app name to a running app's name is a fact lookup; the orb
/// only points: nothing here activates, opens or launches anything.
@MainActor
protocol IntroAppLocating {
    /// `app`: the name macOS shows ("Finder"), or "" for the owner's default browser.
    func target(app: String) -> IntroAppTarget
    /// The app owning the frontmost on-screen layer-0 window among `apps` (named as macOS shows them): a point beside
    /// that app's largest window.
    func frontWindow(of apps: [String]) -> CGPoint?
}

/// The real lookup, from injected sources (the window server, running apps, Launch Services, the app folders).
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
    /// The app that opens web links: the owner's default browser.
    var defaultBrowser: () -> URL? = { NSWorkspace.shared.urlForApplication(toOpen: URL(string: "https://example.com")!) }
    var bundleID: (URL) -> String? = { Bundle(url: $0)?.bundleIdentifier }
    /// "<name>.app" in /Applications, /System/Applications or ~/Applications.
    var installed: (String) -> String? = { IntroAppResolver.installedApp(named: $0) }
    var screens: () -> [(frame: CGRect, visible: CGRect)] = {
        NSScreen.screens.map { (frame: $0.frame, visible: $0.visibleFrame) }
    }
    var primaryHeight: () -> CGFloat = { NSScreen.screens.first { $0.frame.origin == .zero }?.frame.height ?? 0 }
    /// The notch the orb leaves from (FERB's screen holds it).
    var notch: () -> CGPoint = { OrbController.notchHome() }

    func target(app: String) -> IntroAppTarget {
        let apps = running()
        let match: RunningApp?
        let name: String
        var icon: String?
        if app.isEmpty {
            let browser = defaultBrowser()
            let id = browser.flatMap(bundleID)
            match = id.flatMap { id in apps.first { $0.bundleID == id } }
            name = match?.name ?? browser.map { $0.deletingPathExtension().lastPathComponent } ?? ""
            icon = browser?.path
        } else {
            match = apps.first { $0.name.caseInsensitiveCompare(app) == .orderedSame }
            name = match?.name ?? app
            icon = installed(app) ?? match?.bundlePath
        }
        if let match, let point = beside(windowOf: [match.pid]) {
            return IntroAppTarget(point: point, name: name, iconPath: icon, onWindow: true)
        }
        return IntroAppTarget(point: restingPoint(), name: name, iconPath: icon, onWindow: false)
    }

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

    /// The app folders an installed app is looked for in, by name.
    nonisolated static let appFolders = ["/Applications", "/System/Applications",
                                         FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Applications").path]

    /// "<name>.app" directly inside one of `folders`, or nil. The name comes from a model (rule 9: data, never a path):
    /// one with "/", ":" or NUL, or starting with ".", is refused before anything is probed, and the candidate must sit
    /// directly in its folder.
    nonisolated static func installedApp(named name: String, folders: [String] = appFolders,
                                         exists: (String) -> Bool = { FileManager.default.fileExists(atPath: $0) })
        -> String? {
        guard !name.isEmpty, !name.hasPrefix("."), !name.contains("/"), !name.contains(":"),
              !name.contains("\0") else { return nil }
        for folder in folders {
            let base = URL(fileURLWithPath: folder).standardizedFileURL
            let candidate = base.appendingPathComponent(name + ".app").standardizedFileURL
            guard candidate.deletingLastPathComponent().path == base.path else { continue }
            if exists(candidate.path) { return candidate.path }
        }
        return nil
    }

    /// Below the notch, about a third of the way down FERB's screen.
    private func restingPoint() -> CGPoint {
        let home = notch()
        let all = screens()
        let screen = all.first { $0.frame.contains(home) }?.frame ?? all.first?.frame
            ?? CGRect(origin: .zero, size: CGSize(width: home.x * 2, height: home.y))
        return CGPoint(x: home.x, y: screen.maxY - screen.height / 3)
    }
}
