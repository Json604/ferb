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
                                          NSWindow.didChangeOcclusionStateNotification]
        for name in names {
            observers.append(NotificationCenter.default.addObserver(forName: name, object: nil, queue: .main) {
                [weak self] _ in MainActor.assumeIsolated { self?.update() }
            })
        }
        update()
    }

    /// After the current event, so a closing window has already left the screen when it is counted.
    func update() {
        DispatchQueue.main.async {
            let policy: NSApplication.ActivationPolicy =
                Self.wantsDock(NSApp.windows.map(DockWindow.init)) ? .regular : .accessory
            if NSApp.activationPolicy() != policy { NSApp.setActivationPolicy(policy) }
        }
    }
}
