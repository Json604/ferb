import AppKit

/// APP-600 (owner): FERB shows in the Dock only while one of its windows is open (setup, Settings, the preview player,
/// What FERB saw); with none open it is notch-only again. The notch, the orb and other floating panels never count.
@MainActor
final class DockPresence {
    /// Whether these windows put FERB in the Dock: a titled, non-panel window that is shown or minimized (a minimized
    /// window lives in the Dock, so FERB stays there with it).
    static func wantsDock(_ windows: [DockWindow]) -> Bool {
        windows.contains { !$0.isPanel && $0.isTitled && ($0.isVisible || $0.isMiniaturized) }
    }

    /// The policy to set now, or nil to leave it. FERB joins the Dock only while it isn't the active app (APP-796): the
    /// Dock takes joining as the launch of the front app "with no windows" and switched the owner to another Space
    /// (owner's logs, 2026-11-07/08). A join wanted while FERB is active waits until it resigns active.
    ///
    /// `cover`: insomnia mode's cover is up (APP-980). The cover is a regular-policy app's panels or a full-screen app's
    /// Space hides it and Cmd-Tab passes (x1 experiment, measured): FERB is regular for as long as it is up, active or
    /// not (its kiosk options hide the Dock), and goes back by this rule when it drops.
    static func change(wants: Bool, current: NSApplication.ActivationPolicy, active: Bool,
                       cover: Bool = false) -> NSApplication.ActivationPolicy? {
        if cover { return current == .regular ? nil : .regular }
        if wants { return current == .regular || active ? nil : .regular }
        return current == .accessory ? nil : .accessory
    }

    /// Insomnia mode's cover is up (InsomniaCover sets it and calls `settle`).
    static var coverUp = false

    struct DockWindow: Equatable {
        let isPanel: Bool, isTitled: Bool, isVisible: Bool, isMiniaturized: Bool
        init(isPanel: Bool, isTitled: Bool, isVisible: Bool, isMiniaturized: Bool) {
            self.isPanel = isPanel; self.isTitled = isTitled; self.isVisible = isVisible; self.isMiniaturized = isMiniaturized
        }
        init(_ window: NSWindow) {
            self.init(isPanel: window is NSPanel, isTitled: window.styleMask.contains(.titled),
                      isVisible: window.isVisible, isMiniaturized: window.isMiniaturized)
        }
    }

    private var observers: [NSObjectProtocol] = []

    func start() {
        let names: [Notification.Name] = [NSWindow.didBecomeKeyNotification, NSWindow.didBecomeMainNotification,
                                          NSWindow.willCloseNotification, NSWindow.didMiniaturizeNotification,
                                          NSWindow.didDeminiaturizeNotification,
                                          NSWindow.didChangeOcclusionStateNotification,
                                          NSApplication.didResignActiveNotification]
        for name in names {
            observers.append(NotificationCenter.default.addObserver(forName: name, object: nil, queue: .main) {
                [weak self] _ in MainActor.assumeIsolated { self?.update() }
            })
        }
        update()
    }

    /// After the current event, so a closing window has already left the screen when it is counted.
    func update() {
        DispatchQueue.main.async { Self.settle() }
    }

    /// The policy as the windows (and the cover) want it, now.
    static func settle() {
        if let policy = change(wants: wantsDock(NSApp.windows.map(DockWindow.init)), current: NSApp.activationPolicy(),
                               active: NSApp.isActive, cover: coverUp) {
            NSApp.setActivationPolicy(policy)
        }
    }
}
