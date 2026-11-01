#if FERB_PLAYGROUND
import AppKit
import Darwin
import NotchCore

/// The intro playground (PLAYGROUND.md): the real first-run intro (notch, orb, narration, beats, sounds, copy) with
/// everything behind it faked. Compiled into debug builds only (Package.swift's FERB_PLAYGROUND) and chosen only when
/// the app runs under its own bundle id, so it never shares preferences, permission grants or Keychain access with
/// FERB.app (`com.ferb.FERB`) or the development notch (`dev.ferb.notch`). Nothing real is behind it: no daemon socket,
/// no ~/.ferb, no Keychain, no Sparkle, no login item or background service, no permission request, no network.
enum PlaygroundMode {
    static let bundleID = "dev.ferb.playground"
    /// Keeps the playground's state across a simulated quit and reopen; without it a launch starts fresh.
    static let reopenArgument = "--reopen"

    static var requested: Bool { isPlayground(bundleID: Bundle.main.bundleIdentifier) }

    static func isPlayground(bundleID: String?) -> Bool { bundleID == Self.bundleID }

    /// Runs the playground app and never returns. A home that is (or holds, or is inside) the real ~/.ferb is refused
    /// before anything else runs.
    @MainActor
    static func run() -> Never {
        let given = getenv("FERB_HOME").map { String(cString: $0) }
        let home: URL
        switch PlaygroundHome.resolve(given: given, account: accountFerbHome(),
                                      temporary: FileManager.default.temporaryDirectory) {
        case .home(let url):
            home = url
        case .refused(let why):
            fputs("FERB playground: \(why)\n", stderr)
            let alert = NSAlert()
            alert.messageText = "The FERB playground didn't start."
            alert.informativeText = why
            alert.runModal()
            exit(78)
        }
        setenv("FERB_HOME", home.path, 1)        // anything that reads FERB_HOME gets the playground's own home
        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)      // as FERB.app (LSUIElement): no Dock icon; the control panel is a window
        let delegate = PlaygroundDelegate(home: home, fresh: !CommandLine.arguments.contains(reopenArgument))
        app.delegate = delegate
        withExtendedLifetime(delegate) { app.run() }
        exit(0)
    }
}

/// Where the playground keeps its fake daemon's state: `FERB_HOME` when set, else a folder in the temporary directory.
/// Never the account's real ~/.ferb, a folder inside it, or a folder that holds it (compared after resolving symlinks,
/// and by device and inode when both exist).
enum PlaygroundHome {
    enum Outcome: Equatable {
        case home(URL)
        case refused(String)
    }

    static let folderName = "ferb-playground"

    static func resolve(given: String?, account: String?, temporary: URL) -> Outcome {
        guard let account, !account.isEmpty else {
            return .refused("It can't find your account's home folder, so it can't tell your real ~/.ferb apart.")
        }
        let raw = given.flatMap { $0.isEmpty ? nil : URL(fileURLWithPath: $0) }
            ?? temporary.appendingPathComponent(folderName, isDirectory: true)
        let home = canonical(raw.path)
        let real = canonical(account)
        if home == real || home.hasPrefix(real + "/") || sameFile(home, real) {
            return .refused("FERB_HOME is your real ~/.ferb (\(account)) or a folder inside it. The playground never " +
                            "reads or writes FERB's real home; unset FERB_HOME or point it at a temporary folder.")
        }
        if real.hasPrefix(home + "/") || home == "/" {
            return .refused("FERB_HOME (\(home)) holds your real ~/.ferb. Point it at a temporary folder instead.")
        }
        return .home(URL(fileURLWithPath: home, isDirectory: true))
    }

    /// The path with `.`/`..` removed and every existing part's symlinks resolved (the part that doesn't exist yet is
    /// kept as written).
    static func canonical(_ path: String) -> String {
        let standard = (path as NSString).standardizingPath
        var existing = standard
        var rest: [String] = []
        while existing != "/" && !FileManager.default.fileExists(atPath: existing) {
            rest.insert((existing as NSString).lastPathComponent, at: 0)
            existing = (existing as NSString).deletingLastPathComponent
        }
        var resolved = (existing as NSString).resolvingSymlinksInPath
        for part in rest { resolved = (resolved as NSString).appendingPathComponent(part) }
        return resolved
    }

    private static func sameFile(_ a: String, _ b: String) -> Bool {
        var first = stat(), second = stat()
        guard stat(a, &first) == 0, stat(b, &second) == 0 else { return false }
        return first.st_dev == second.st_dev && first.st_ino == second.st_ino
    }
}

/// Everything the playground puts behind the real intro: one fake for each seam FERB.app's AppDelegate fills with the
/// real thing (the daemon transport, the setup command, FERB.app's own permissions, System Settings, the Keychain,
/// launchd, SMAppService, the microphone), all driven by one `PlaygroundState` the control panel edits.
@MainActor
struct PlaygroundServices {
    let home: URL
    let state: PlaygroundState
    let daemon: PlaygroundDaemon
    let setup: SetupModel
    let notch: NotchModel
    let opener: PlaygroundSettingsOpener
    let service: PlaygroundService
    let sleeper: PlaygroundSleeper

    /// `fresh`: a new run (the saved state is dropped); otherwise the state a quit left is read back.
    static func make(home: URL, defaults: UserDefaults, fresh: Bool) -> PlaygroundServices {
        try? FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
        let state = PlaygroundState(file: home.appendingPathComponent("state.json"), fresh: fresh)
        let daemon = PlaygroundDaemon(state: state)
        let opener = PlaygroundSettingsOpener(state: state)
        let service = PlaygroundService(state: state)
        let setup = SetupModel(rpc: daemon, runner: PlaygroundRunner(state: state),
                               app: PlaygroundAppPermissions(state: state), system: opener, keys: PlaygroundKeys(),
                               service: service, background: PlaygroundBackground(state: state, opener: opener))
        let notch = NotchModel(rpc: daemon, preferences: defaults, microphone: PlaygroundMicrophone())
        let services = PlaygroundServices(home: home, state: state, daemon: daemon, setup: setup, notch: notch,
                                          opener: opener, service: service, sleeper: PlaygroundSleeper(state: state))
        services.applySpeed()
        return services
    }

    /// The setup model's own polls follow the speed too.
    func applySpeed() {
        setup.grantPoll = .milliseconds(Int(1500 / state.speed))
        setup.requestSettle = .milliseconds(Int(1500 / state.speed))
        setup.serviceStartWait = .seconds(5)
    }
}
#endif
