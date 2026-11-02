#if FERB_PLAYGROUND
import AppKit
import SwiftUI
import UniformTypeIdentifiers
import NotchCore

/// System Settings, faked: opening a privacy pane (or Login Items) shows a stand-in window with FERB's switch, so the
/// orb has a window to dock beside and the owner has a switch to flip. The real System Settings is never opened; a
/// web page (Claude's sign-in) is only logged; "Show in Finder" is only logged.
@MainActor
final class PlaygroundSettingsOpener: SystemOpening {
    private let state: PlaygroundState
    /// A switch that applies after a restart was flipped: "Quit & Reopen", as macOS offers (the delegate relaunches).
    var onQuitAndReopen: (() -> Void)?
    var onInteraction: ((Int?) -> Void)?
    var avoidFrame: (() -> CGRect?)?
    private(set) var window: NSPanel?
    private var pane: PlaygroundPane?
    private var autoFlip: Task<Void, Never>?

    init(state: PlaygroundState) { self.state = state }

    func open(_ url: URL) {
        guard url.scheme == "x-apple.systempreferences", let pane = PlaygroundPane(url: url) else {
            state.note("would open \(url.absoluteString) in the browser (not in the playground)")
            return
        }
        show(pane)
    }

    func reveal(_ path: String) { state.note("would show \((path as NSString).lastPathComponent) in Finder") }

    func showLoginItems() { show(.loginItems) }

    /// The stand-in window's frame while it is on screen (the orb's tracker follows it).
    var visibleFrame: CGRect? {
        guard let window, window.isVisible, !window.isMiniaturized else { return nil }
        return window.frame
    }

    private func show(_ next: PlaygroundPane) {
        state.note("System Settings (stand-in): \(next.title)")
        pane = next
        let panel = window ?? makeWindow()
        window = panel
        panel.title = "System Settings — \(next.title) (playground stand-in)"
        panel.contentView = NSHostingView(rootView: PlaygroundPaneView(pane: next, state: state, quitAndReopen: { [weak self] in
            self?.onQuitAndReopen?()
        }))
        onInteraction?(panel.windowNumber)
        panel.orderFrontRegardless()             // shown without moving keyboard focus
        autoFlip?.cancel()
        guard state.saved.answer == .allowSoon else { return }
        autoFlip = Task { [weak self, state] in
            await state.pause(3)
            guard !Task.isCancelled, let self, self.pane == next, self.window?.isVisible == true else { return }
            switch next {
            case .loginItems: state.saved.loginItemsAllowed = true
            case .permission(let id): state.set(id, .granted, fromSwitch: true)
            }
            state.note("the owner flipped FERB's switch in \(next.title) (auto)")
        }
    }

    private func makeWindow() -> NSPanel {
        let panel = PlaygroundSettingsPanel(contentRect: NSRect(x: 0, y: 0, width: 520, height: 420),
                            styleMask: [.titled, .closable, .nonactivatingPanel, .utilityWindow], backing: .buffered,
                            defer: false)
        panel.isReleasedWhenClosed = false
        panel.hidesOnDeactivate = false
        panel.becomesKeyOnlyIfNeeded = false
        panel.onInteraction = { [weak self, weak panel] in self?.onInteraction?(panel?.windowNumber) }
        panel.onClose = { [weak self] in self?.onInteraction?(nil) }
        if let screen = NSScreen.main {
            let visible = screen.visibleFrame
            var origin = CGPoint(x: visible.maxX - panel.frame.width - 24, y: visible.maxY - panel.frame.height - 40)
            if let avoid = avoidFrame?() {
                if avoid.maxX + 24 + panel.frame.width <= visible.maxX {
                    origin.x = avoid.maxX + 24
                } else if avoid.minX - 24 - panel.frame.width >= visible.minX {
                    origin.x = avoid.minX - 24 - panel.frame.width
                }
            }
            panel.setFrameOrigin(origin)
        }
        panel.setFrameAutosaveName("PlaygroundSettingsStandIn")
        return panel
    }
}

/// A click makes the stand-in frontmost even when it was already key and the notch was summoned again.
private final class PlaygroundSettingsPanel: NSPanel {
    var onInteraction: (() -> Void)?
    var onClose: (() -> Void)?
    override var canBecomeKey: Bool { true }
    override func sendEvent(_ event: NSEvent) {
        if event.type == .leftMouseDown { onInteraction?() }
        super.sendEvent(event)
    }
    override func close() { onClose?(); super.close() }
}

/// Which stand-in pane is showing.
enum PlaygroundPane: Equatable {
    case permission(String)
    case loginItems

    init?(url: URL) {
        let text = url.absoluteString
        if text.contains("Notifications-Settings") { self = .permission("notifications"); return }
        if text.localizedCaseInsensitiveContains("LoginItems") { self = .loginItems; return }
        let anchors = ["Privacy_Accessibility": "accessibility", "Privacy_ScreenCapture": "screen_recording",
                       "Privacy_AllFiles": "full_disk_access", "Privacy_ListenEvent": "input_monitoring",
                       "Privacy_Pasteboard": "clipboard", "Privacy_Microphone": "microphone",
                       "Privacy_SpeechRecognition": "microphone"]
        guard let id = anchors.first(where: { text.hasSuffix($0.key) })?.value else { return nil }
        self = .permission(id)
    }

    var title: String {
        switch self {
        case .loginItems: return "Login Items"
        case .permission(let id): return PermissionCopy.label(id)
        }
    }
}

private struct PlaygroundPaneView: View {
    let pane: PlaygroundPane
    @ObservedObject var state: PlaygroundState
    let quitAndReopen: () -> Void
    @State private var dropTargeted = false
    @State private var added = false

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Stand-in for System Settings. The real one is never opened by the playground.")
                .font(.caption).foregroundStyle(.secondary)
            Text(pane.title).font(.title2.bold())
            switch pane {
            case .loginItems:
                Text("Allow in the Background").font(.headline)
                row(on: Binding(get: { state.saved.loginItemsAllowed },
                                set: { state.saved.loginItemsAllowed = $0 }))
            case .permission(let id):
                Text(PermissionCopy.reason(id)).foregroundStyle(.secondary)
                row(on: Binding(get: { state.grant(id) == .granted || state.saved.afterRestart.contains(id) },
                                set: { state.set(id, $0 ? .granted : .denied, fromSwitch: true) }))
                if state.saved.afterRestart.contains(id) {
                    VStack(alignment: .leading, spacing: 8) {
                        Text("“\(AppName.current)” may not be able to use this until it is quit.").bold()
                        HStack {
                            Button("Later") {}
                            Button("Quit & Reopen") { quitAndReopen() }.keyboardShortcut(.defaultAction)
                        }
                    }
                    .padding(10)
                    .background(RoundedRectangle(cornerRadius: 8).fill(Color.orange.opacity(0.15)))
                }
                if !SetupModel.answeredInPopup.contains(id) { dropZone(id) }
            }
            Spacer()
        }
        .padding(20)
        .frame(minWidth: 480, minHeight: 380)
    }

    private func row(on: Binding<Bool>) -> some View {
        HStack {
            Image(nsImage: NSWorkspace.shared.icon(forFile: Bundle.main.bundlePath))
                .resizable().frame(width: 28, height: 28)
            Text(AppName.current)
            Spacer()
            Toggle("", isOn: on).toggleStyle(.switch).labelsHidden()
        }
        .padding(10)
        .background(RoundedRectangle(cornerRadius: 8).fill(Color.secondary.opacity(0.12)))
    }

    /// Where the orb's drag card can be dropped, as onto the real pane's list.
    private func dropZone(_ id: String) -> some View {
        Text(added ? (state.saved.afterRestart.contains(id)
            ? "FERB added. Choose Quit & Reopen above to finish this permission."
            : "FERB added to the list.") : "Drag the FERB logo here to add it")
            .font(.caption)
            .frame(maxWidth: .infinity, minHeight: 54)
            .padding(.horizontal, 10)
            .foregroundStyle(added ? Color.green : Color.primary)
            .background(RoundedRectangle(cornerRadius: 8).fill(Color.green.opacity(added || dropTargeted ? 0.12 : 0)))
            .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(
                added || dropTargeted ? Color.green : Color.secondary,
                style: StrokeStyle(lineWidth: 1, dash: added ? [] : [4])))
            .onDrop(of: [.fileURL], isTargeted: $dropTargeted) { _ in
                added = true
                state.set(id, .granted, fromSwitch: true)
                state.note("the app was dropped into \(PermissionCopy.label(id))")
                return true
            }
    }
}

/// The orb's tracker for the stand-in window (in place of SettingsWindowTracker, which follows the real System
/// Settings): the dock point beside the window while it is on screen, nil when it isn't.
@MainActor
final class PlaygroundSettingsTracker: SettingsWindowTracking {
    var onChange: ((CGPoint?) -> Void)?
    var settingsFrame: CGRect? { opener.visibleFrame }
    private let opener: PlaygroundSettingsOpener
    private var timer: Timer?
    private var reported: CGPoint?

    init(opener: PlaygroundSettingsOpener) { self.opener = opener }

    func start() {
        guard timer == nil else { return }
        let t = Timer(timeInterval: 0.25, repeats: true) { [weak self] timer in
            MainActor.assumeIsolated {
                if let self { self.poll() } else { timer.invalidate() }
            }
        }
        RunLoop.main.add(t, forMode: .common)
        timer = t
        poll()
    }

    func stop() {
        timer?.invalidate()
        timer = nil
        reported = nil
    }

    private func poll() {
        let point = opener.visibleFrame.flatMap { frame -> CGPoint? in
            let screen = NSScreen.screens.first { $0.frame.intersects(frame) }?.visibleFrame ?? frame
            let window = WindowInfo(owner: SettingsAnchor.bundleID, bounds: frame, onScreen: true, isFrontmostApp: true)
            return SettingsAnchor.beside(window, orb: OrbController.size, screen: screen)
        }
        switch (point, reported) {
        case (nil, nil): return
        case (let p?, let r?) where abs(p.x - r.x) <= 1 && abs(p.y - r.y) <= 1: return
        default:
            reported = point
            onChange?(point)
        }
    }
}
#endif
