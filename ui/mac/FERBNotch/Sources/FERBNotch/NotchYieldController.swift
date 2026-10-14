import Darwin
import CoreGraphics
import NotchCore

/// Decides when FERB's notch windows yield to another app. PanelCoordinator feeds it AppKit events and FERB's
/// frames, and applies the result to its windows through `model.onYieldChange`. Everything here is in
/// window-server coordinates (origin at the top left of the primary display).
@MainActor
final class NotchYieldController {
    /// Where FERB is drawn, taken from the model rather than the live windows, so a yielded (invisible) FERB keeps
    /// the same frames and doesn't flicker back on.
    struct Layout {
        /// The frames another app's menu could sit on: the panel while expanded, the tab on a display without a
        /// camera housing, and the camera-edge light while it glows. Empty when nothing of FERB is visible.
        var occupied: [CGRect]
        /// The pinned chat's drawn surface (header, plus the body when shown); nil unless the chat is pinned.
        var pinnedChat: CGRect?
        /// The camera cutout with the hover tolerance. Hovering or clicking it brings FERB back.
        var notch: CGRect
        /// The menu bar strip of FERB's display.
        var menuBar: CGRect
        var display: CGRect
    }

    private let model: NotchModel
    let ownPIDs: Set<Int32>
    var windows: () -> [ScreenWindow]
    var layout: () -> Layout?
    /// A window that un-minimizes or unhides when its app comes forward isn't on screen yet at activation.
    var activationRecheckDelay: Duration = .milliseconds(500)
    /// The app FERB is yielding to because one of its windows lies under the pinned chat.
    private(set) var coveredApp: Int32?
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

    /// One poll while FERB is on screen: yield while another process's menu, popover or floating window is over
    /// FERB; come back from the app under the pinned chat once none of its windows is there. Returns whether FERB
    /// just came back.
    @discardableResult
    func evaluate() -> Bool {
        let wasYielded = model.isYielded
        guard let layout = layout(), !layout.occupied.isEmpty else {
            model.endYield(.foreignOverlay)
            endCoveredApp()
            return wasYielded && !model.isYielded
        }
        let rows = windows()
        if NotchOverlap.foreignOverlay(in: rows, ferbFrames: layout.occupied, ownPIDs: ownPIDs,
                                       menuBar: layout.menuBar, display: layout.display) {
            model.beginYield(.foreignOverlay)
        } else {
            model.endYield(.foreignOverlay)
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

    /// The owner clicked in another app. A click on the notch brings FERB back. A click into an app whose window
    /// lies under the pinned chat yields to it even when that app was already frontmost: an answer that opens the
    /// chat over the app the owner is using causes no activation. Returns whether FERB just came back.
    @discardableResult
    func ownerClicked(at point: CGPoint) -> Bool {
        guard let layout = layout() else { return false }
        if yieldsToCoveredApp {
            return layout.notch.contains(point) ? summon() : false
        }
        guard layout.pinnedChat != nil else { return false }
        let rows = windows()
        if let app = NotchOverlap.owner(at: point, in: rows, ownPIDs: ownPIDs) { yieldIfUnderChat(app, rows: rows) }
        return false
    }

    /// The pointer moved while FERB yields. Hovering the notch brings FERB back from the app under the pinned chat.
    /// Returns whether FERB still yields; if so, hover must not open, reveal, fold or dismiss anything.
    func pointerMoved(to point: CGPoint) -> Bool {
        if yieldsToCoveredApp, let layout = layout(), layout.notch.contains(point) { summon() }
        return model.isYielded
    }

    /// The owner reached for FERB (its shortcut, menu or a control, or FERB coming forward): stop yielding to the
    /// app under the pinned chat. Returns whether FERB just came back.
    @discardableResult
    func summon() -> Bool {
        activationRecheck?.cancel()
        let wasYielded = model.isYielded
        endCoveredApp()
        return wasYielded && !model.isYielded
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
        model.beginYield(.activeAppUnderPinnedChat)
        guard yieldsToCoveredApp else { return false }
        coveredApp = pid
        return true
    }
}
