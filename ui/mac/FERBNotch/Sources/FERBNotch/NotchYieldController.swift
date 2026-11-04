import AppKit
import NotchCore

/// What a pointer move means while FERB may be yielding.
enum YieldHover: Equatable {
    /// FERB still yields: hover must not open, reveal, fold or dismiss anything.
    case hidden
    /// Entering the notch brought FERB back: handle the hover and focus the visible input, as hover always does.
    case broughtBack
    /// FERB wasn't yielding.
    case visible
}

/// Decides when FERB's notch windows yield to another app. PanelCoordinator feeds it AppKit events and FERB's
/// frames, and applies the result to its windows through `model.onYieldChange`. Everything here is in
/// window-server coordinates (origin at the top left of the primary display).
@MainActor
final class NotchYieldController {
    /// Where FERB is drawn, taken from the model rather than the live windows, so a yielded FERB (one layer back)
    /// keeps the same frames and doesn't flicker back on top.
    struct Layout {
        /// The frames another app's menu could sit on: the panel while expanded, the tab on a display without a
        /// camera housing, and the camera-edge light while it glows. Empty when nothing of FERB is visible.
        var occupied: [CGRect]
        /// The pinned chat's drawn surface (header, plus the body when shown); nil unless the chat is pinned.
        var pinnedChat: CGRect?
        /// The camera cutout, up to the physical top edge but not the band below it where menus begin. Entering or
        /// clicking it brings FERB back.
        var notch: CGRect
        /// The menu bar strip of FERB's display.
        var menuBar: CGRect
        var display: CGRect
    }

    private let model: NotchModel
    let ownPIDs: Set<Int32>
    var windows: () -> [ScreenWindow]
    var layout: () -> Layout?
    /// The bundle identifier of a process, to recognise system text-input windows.
    var bundleID: (Int32) -> String? = { NSRunningApplication(processIdentifier: $0)?.bundleIdentifier }
    /// Whether a process can ever be the active app: not an app extension, view service or other process whose
    /// activation is prohibited (those draw parts of another app's windows, APP-690).
    var canBeActive: (Int32) -> Bool = { pid in
        guard let app = NSRunningApplication(processIdentifier: pid) else { return false }
        return app.activationPolicy != .prohibited
    }
    /// The app in front now.
    var frontmostApp: () -> Int32? = { NSWorkspace.shared.frontmostApplication?.processIdentifier }
    /// A window that un-minimizes or unhides when its app comes forward isn't on screen yet at activation.
    var activationRecheckDelay: Duration = .milliseconds(500)
    /// The app FERB is yielding to because one of its windows lies under the pinned chat.
    private(set) var coveredApp: Int32?
    private var companionWindow: Int?
    /// Other apps' windows the owner chose to show FERB above: those over it when they opened or revealed FERB
    /// (shortcut, menu, click, hover), entered or clicked the notch, used the shortcut, or dictated. They stop hiding
    /// FERB until they leave the screen, so a window that never closes (a screen-share bar, another notch app) can't
    /// keep it hidden; a window that newly overlaps FERB hides it again.
    private(set) var overriddenOverlays = Set<UInt32>()
    /// The owner just opened or revealed FERB: the windows found over it at the next check are overridden.
    private var overrideAtNextCheck = false
    private var systemInputPIDs: [Int32: Bool] = [:]
    /// The other apps' windows over FERB at the last poll.
    private var currentOverlays: [ScreenWindow] = []
    /// Whether the pointer was on the notch at the last look. Only entering the notch brings FERB back, so a pointer
    /// that was already resting there when FERB stepped aside doesn't undo it.
    private var pointerOnNotch = false
    private var activationRecheck: Task<Void, Never>?

    init(model: NotchModel, ownPIDs: Set<Int32> = [getpid()],
         windows: @escaping () -> [ScreenWindow] = ScreenWindow.onScreen,
         layout: @escaping () -> Layout? = { nil }) {
        self.model = model
        self.ownPIDs = ownPIDs
        self.windows = windows
        self.layout = layout
    }

    private var yieldsToCoveredApp: Bool { model.yielding.reasons.contains(.activeAppUnderPinnedChat) }
    /// Yielding to another app (not to FERB's own file picker), which the owner can override.
    private var yieldsToAnotherApp: Bool { model.yielding.reasons.contains { $0 != .filePicker } }

    /// An intro companion was clicked. Its own-process identity must not prevent the notch stepping behind it.
    func companionFocused(_ window: Int?) {
        companionWindow = window
        if window != nil && model.introLine != nil { model.beginYield(.introCompanion) }
        else { model.endYield(.introCompanion) }
    }

    /// FERB just stepped aside; `pointer` is where the pointer is now.
    func yieldBegan(pointer: CGPoint) {
        pointerOnNotch = layout().map { isOnNotch(pointer, $0) } ?? false
    }

    /// One poll while FERB is on screen: yield while another process's menu, popover or floating window the owner
    /// hasn't overridden is over FERB; come back from the app under the pinned chat once none of its windows is
    /// there. Returns whether FERB just came back.
    @discardableResult
    func evaluate() -> Bool {
        let wasYielded = model.isYielded
        guard let layout = layout(), !layout.occupied.isEmpty else {
            // Overrides are kept, so reopening FERB over the same persistent window doesn't hide it again.
            currentOverlays = []
            model.endYield(.foreignOverlay)
            endCoveredApp()
            return wasYielded && !model.isYielded
        }
        let rows = windows()
        currentOverlays = NotchOverlap.foreignOverlays(in: rows, ferbFrames: layout.occupied, ownPIDs: ownPIDs,
                                                       menuBar: layout.menuBar, display: layout.display)
            .filter { !isSystemInput($0.ownerPID) }
        let overlays = Set(currentOverlays.map(\.windowID))
        // An override is forgotten only when its window leaves the screen, not when FERB folds away from it.
        overriddenOverlays.formIntersection(rows.map(\.windowID))
        // Opened or revealed by the owner, or dictating: FERB stays over the windows there now.
        if overrideAtNextCheck || model.session.phase == .listening { overriddenOverlays.formUnion(overlays) }
        overrideAtNextCheck = false
        if overlays.isSubset(of: overriddenOverlays) {
            model.endYield(.foreignOverlay)
        } else {
            model.beginYield(.foreignOverlay)
        }
        if yieldsToCoveredApp {
            let stillCovered = coveredApp.flatMap { app in
                layout.pinnedChat.map { NotchOverlap.appWindowUnderChat(in: rows, appPID: app, chat: $0) }
            } ?? false
            if !stillCovered { endCoveredApp() }
        }
        return wasYielded && !model.isYielded
    }

    /// An app came forward. Yield when one of its windows lies under the pinned chat. FERB itself coming forward,
    /// or an app with nothing under the chat, brings FERB back. Returns whether FERB just came back.
    @discardableResult
    func appActivated(_ pid: Int32) -> Bool {
        activationRecheck?.cancel()
        let wasYielded = model.isYielded
        if ownPIDs.contains(pid) {
            endCoveredApp()
        } else if !yieldIfUnderChat(pid) {
            endCoveredApp()
            let delay = activationRecheckDelay
            activationRecheck = Task { [weak self] in
                do { try await Task.sleep(for: delay) } catch { return }
                self?.yieldIfUnderChat(pid)
            }
        }
        return wasYielded && !model.isYielded
    }

    /// The owner clicked in another app. A click on the notch (not on another app's menu reaching under it) brings
    /// FERB back. A click into an app whose window lies under the pinned chat yields to it even when that app was
    /// already frontmost: a question that opens the chat over the app the owner is using causes no activation.
    /// Returns whether FERB just came back.
    @discardableResult
    func ownerClicked(at point: CGPoint) -> Bool {
        guard let layout = layout() else { return false }
        if yieldsToAnotherApp, isOnNotch(point, layout) { return summon() }
        guard !yieldsToCoveredApp, layout.pinnedChat != nil else { return false }
        let rows = windows()
        // A click into System Settings' pane lands on its extension's window: it counts for Settings (APP-690).
        if let app = NotchOverlap.clickedApp(at: point, in: rows, ownPIDs: ownPIDs, canBeActive: canBeActive,
                                             frontmost: frontmostApp) {
            yieldIfUnderChat(app, rows: rows)
        }
        return false
    }

    /// The pointer moved while FERB may be yielding. Entering the notch (not on another app's menu over it) brings
    /// FERB back from another app.
    func pointerMoved(to point: CGPoint) -> YieldHover {
        guard model.isYielded else { return .visible }
        guard let layout = layout() else { return .hidden }
        let onNotch = isOnNotch(point, layout)
        defer { pointerOnNotch = onNotch }
        if onNotch, !pointerOnNotch, yieldsToAnotherApp { summon() }
        return model.isYielded ? .hidden : .broughtBack
    }

    /// The owner opened or revealed FERB (a presentation that opens the body and asks for focus: shortcut, menu,
    /// click, hover-open, pinned reveal). FERB stays over the windows found over it at the next check; the pointer
    /// that opened it counts as entering the notch.
    func ownerOpened() {
        overrideAtNextCheck = true
    }

    /// The owner reached for FERB (the notch, its shortcut, menu or a control, or FERB coming forward): stop
    /// yielding to the app under the pinned chat, and show FERB over the windows now over it until a new one
    /// appears. Returns whether FERB just came back.
    @discardableResult
    func summon() -> Bool {
        activationRecheck?.cancel()
        let wasYielded = model.isYielded
        if model.yielding.reasons.contains(.foreignOverlay) {
            overriddenOverlays.formUnion(currentOverlays.map(\.windowID))
            model.endYield(.foreignOverlay)
        }
        endCoveredApp()
        companionFocused(nil)
        return wasYielded && !model.isYielded
    }

    /// On the notch, and not on another app's menu there. Any other window covering the notch (another notch app)
    /// is overridden by hovering or clicking it.
    private func isOnNotch(_ point: CGPoint, _ layout: Layout) -> Bool {
        guard layout.notch.contains(point) else { return false }
        return !(model.yielding.reasons.contains(.foreignOverlay) && currentOverlays.contains {
            $0.layer == NotchOverlap.popUpMenuLayer && $0.bounds.contains(point)
        })
    }

    private func isSystemInput(_ pid: Int32) -> Bool {
        if let known = systemInputPIDs[pid] { return known }
        let isInput = bundleID(pid).map(NotchOverlap.isSystemInput(bundleID:)) ?? false
        systemInputPIDs[pid] = isInput
        return isInput
    }

    func stop() {
        activationRecheck?.cancel()
        activationRecheck = nil
    }

    private func endCoveredApp() {
        coveredApp = nil
        model.endYield(.activeAppUnderPinnedChat)
    }

    @discardableResult
    private func yieldIfUnderChat(_ pid: Int32, rows: [ScreenWindow]? = nil) -> Bool {
        guard !ownPIDs.contains(pid), let chat = layout()?.pinnedChat,
              NotchOverlap.appWindowUnderChat(in: rows ?? windows(), appPID: pid, chat: chat) else { return false }
        // Known before the yield applies, so the panel steps back just behind this app's window.
        let previous = coveredApp
        coveredApp = pid
        model.beginYield(.activeAppUnderPinnedChat)
        guard yieldsToCoveredApp else {
            coveredApp = previous
            return false
        }
        return true
    }

    /// The window FERB's panel sits just behind while it yields: the covered app's frontmost window under the pinned
    /// chat. Nil while FERB yields only to menus, popovers, floating windows or its file picker (all of which are
    /// above ordinary windows), or doesn't yield.
    func coveringWindow() -> Int? {
        if model.yielding.reasons.contains(.introCompanion) { return companionWindow }
        guard yieldsToCoveredApp, let app = coveredApp, let chat = layout()?.pinnedChat else { return nil }
        return NotchOverlap.frontWindowUnderChat(in: windows(), appPID: app, chat: chat).map { Int($0.windowID) }
    }
}
