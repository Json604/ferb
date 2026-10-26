import AppKit
import Combine
import SwiftUI
import Carbon
import Quartz
import NotchCore

final class NotchPanel: NSPanel {
    var onEscape: (() -> Void)?
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }
    override func constrainFrameRect(_ frameRect: NSRect, to screen: NSScreen?) -> NSRect { frameRect }
    override func cancelOperation(_ sender: Any?) { onEscape?() }
    override func keyDown(with event: NSEvent) {
        if event.keyCode == 53 { onEscape?(); return }
        super.keyDown(with: event)
    }
    /// The Edit menu's shortcuts don't reach this panel while FERB isn't the active app (it never activates), so
    /// Command-C, X and A go to the focused view directly: text selected in the chat can be copied (owner,
    /// 2026-10-24: "i cant copy by dragging"). The composer handles Command-V itself (ChatTextView).
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask).subtracting([.capsLock, .numericPad])
        let action: Selector? = switch (flags == .command ? event.charactersIgnoringModifiers?.lowercased() : nil) {
        case "c": #selector(NSText.copy(_:))
        case "x": #selector(NSText.cut(_:))
        case "a": #selector(NSResponder.selectAll(_:))
        default: nil
        }
        if let action, NSApp.sendAction(action, to: nil, from: self) { return true }
        return super.performKeyEquivalent(with: event)
    }
}

/// The notch's glow: a click-through window, reported to the daemon as one (ClickThroughReport, D31 fix round 4).
final class NotchLightPanel: NSPanel {
    override init(contentRect: NSRect, styleMask style: NSWindow.StyleMask, backing backingStoreType: NSWindow.BackingStoreType,
                  defer flag: Bool) {
        super.init(contentRect: contentRect, styleMask: style, backing: backingStoreType, defer: flag)
        // AppKit's own property, so the window server is told; hidden from Accessibility, so the system's
        // Accessibility hit test passes through too
        ignoresMouseEvents = true
        setAccessibilityElement(false)
        setAccessibilityHidden(true)
    }
    /// Always click-through: setting false still sets true, through AppKit's own setter.
    override var ignoresMouseEvents: Bool {
        get { super.ignoresMouseEvents }
        set { super.ignoresMouseEvents = true }
    }
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }
    override func constrainFrameRect(_ frameRect: NSRect, to screen: NSScreen?) -> NSRect { frameRect }
}

final class TrackingHostingView<Content: View>: NSHostingView<Content> {
    var entered: (() -> Void)?
    var exited: (() -> Void)?
    private var tracking: NSTrackingArea?

    override func updateTrackingAreas() {
        if let tracking { removeTrackingArea(tracking) }
        let area = NSTrackingArea(rect: .zero, options: [.activeAlways, .mouseEnteredAndExited, .inVisibleRect],
                                  owner: self, userInfo: nil)
        addTrackingArea(area)
        tracking = area
        super.updateTrackingAreas()
    }
    override func mouseEntered(with event: NSEvent) { entered?() }
    override func mouseExited(with event: NSEvent) { exited?() }
}

@MainActor
final class PanelCoordinator: NSObject, NSWindowDelegate, NSMenuDelegate, @preconcurrency QLPreviewPanelDataSource, QLPreviewPanelDelegate {
    let model = NotchModel()
    private let presentation = NotchPresentation()
    private let shortcuts = NotchShortcutMonitor()
    private let overlaySpace = NotchOverlaySpace()
    private static let overlayBehavior: NSWindow.CollectionBehavior = [
        .canJoinAllSpaces, .canJoinAllApplications, .stationary,
        .ignoresCycle, .fullScreenAuxiliary, .fullScreenDisallowsTiling
    ]
    private var presentationRevision = UUID()
    private var presentedBodyVisible = false
    private let panel: NotchPanel
    private let tracerPanel = NotchLightPanel(contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel],
                                              backing: .buffered, defer: false)
    private var geometry: NotchGeometry?
    private var statusItem: NSStatusItem?
    private var hoverTask: Task<Void, Never>?
    private var pointerIsInside = false
    private var cutoutFrame = CGRect.zero
    private var eventMonitors: [Any] = []
    private var hotKey: EventHotKeyRef?
    private var hotKeyHandler: EventHandlerRef?
    private var previewURL: URL?
    private let yieldController: NotchYieldController
    private let windowYield: NotchWindowYield
    private var yieldWatch: Task<Void, Never>?
    private var screensAsleep = false
    private var screenFrame = CGRect.zero
    private var menuBarHeight: CGFloat = 0
    /// How often the window list is read while FERB is on screen. A menu that opens over the notch is uncovered
    /// within about a fifth of a second, before the pointer can reach its first item, and one window-list query
    /// five times a second costs far below a percent of one core. A responsiveness detail, not a behaviour cap.
    /// A pinned FERB is always on screen, so it polls continuously while the displays are awake.
    private static let yieldPollInterval: Duration = .milliseconds(200)
    var onSetUp: (() -> Void)?                   // set by the packaged FERB.app: run `ferb setup` again
    var poster: NotificationPoster?              // set by AppDelegate before start() (L34 D)
    var hasWindow = false                        // Phase E: the FERB window exists ("Open FERB Window")
    var onOpenWindow: (() -> Void)?              // Phase E
    var updateOffer: String?                     // L34 Phase G: an update waiting for the owner's click
    var onInstallUpdate: (() -> Void)?
    var onOpenSettings: (() -> Void)?            // the FERB window at Settings (L34 Phase E): ⌘, and Settings…
    private let service = DaemonService()
    private var statusRefresh: AnyCancellable?
    private var statusRefreshQueued = false

    let cursor = CursorController()

    override init() {
        panel = NotchPanel(contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel],
                           backing: .buffered, defer: false)
        yieldController = NotchYieldController(model: model)
        windowYield = NotchWindowYield(panel: panel, tracer: tracerPanel, rejoinOverlay: { [overlaySpace] window in
            if let window = window as? NSWindow { overlaySpace.rejoin(window) }
        }, leaveOverlay: { [overlaySpace] window in
            if let window = window as? NSWindow { overlaySpace.leave(window) }
        })
        super.init()
        model.cursor = cursor
        // FERB's click-through windows, told to the daemon so its click check skips exactly them (D31 fix round 4)
        cursor.onPanelsChange = { [weak self] numbers in self?.model.clickThrough.set("cursor", windows: numbers) }
        yieldController.layout = { [weak self] in self?.yieldLayout() }
        configurePanel()
        model.onPresentationChange = { [weak self] animate in self?.present(animate: animate) }
        model.onAttentionArrival = { [weak self] in self?.hoverTask?.cancel() }
        model.onBackgroundChange = { [weak self] animate in self?.present(animate: animate, requestFocus: false) }
        model.onInteractionEnded = { [weak self] in
            guard let self, !self.pointerIsOverSurface else { return }
            self.pointerExited()
        }
        model.onInteraction = { [weak self] in self?.beginInteraction() }
        model.onYieldChange = { [weak self] yielded in self?.applyYield(yielded) }
        model.onOpenDocument = { [weak self] url in
            if NSWorkspace.shared.open(url) { self?.model.dismissAutomatically() }
            else {
                let alert = NSAlert()
                alert.messageText = "The artifact couldn’t be opened."
                alert.informativeText = "Try Quick Look, or reveal the file in Finder."
                alert.runModal()
            }
        }
        model.onQuickLook = { [weak self] url in self?.quickLook(url) }
        model.onSettings = { [weak self] in self?.showSettings() }
        model.onStatusChange = { [weak self] in
            self?.updateTracerVisibility()
            self?.updateYieldWatch()
        }
        model.onPinChange = { [weak self] pinned in
            guard let self else { return }
            self.hoverTask?.cancel()
            if !pinned {
                if !self.pointerIsInside { self.pointerExited() }
            }
        }
    }

    func start() {
        model.connectDaemon()
        configureMenu()
        poster?.start(rpc: model.rpc, notchShowing: { [weak self] in self?.notchShowing ?? false },
                      open: { [weak self] target in self?.open(target) })
        model.onApprovalResolved = { [weak self] id in self?.poster?.withdrawQuestion(id) }
        statusRefresh = model.objectWillChange.sink { [weak self] _ in self?.queueStatusRefresh() }
        registerShortcut()
        shortcuts.onDoubleTap = { [weak self] in
            guard let self else { return }
            if !self.summonFromYield() { self.model.toggle() }
        }
        shortcuts.onHoldBegan = { [weak self] in
            self?.hoverTask?.cancel()
            self?.model.beginHeldVoice()
        }
        shortcuts.onHoldEnded = { [weak self] in self?.model.finishHeldVoice() }
        shortcuts.onHoldCancelled = { [weak self] in
            guard let self else { return }
            self.model.cancelHeldVoice()
            if !self.pointerIsInside { self.pointerExited() }
        }
        shortcuts.start()
        if let monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown, handler: { [weak self] event in
            guard let self, self.model.bodyVisible, event.window === self.panel, self.panel.isKeyWindow else { return event }
            if self.model.session.phase == .result, !self.model.showingHistory, !self.model.showingSources,
               !self.model.showingActivity, !(self.panel.firstResponder is NSTextView) {
                if event.keyCode == 49 { self.model.quickLook(); return nil }
                if event.keyCode == 36 || event.keyCode == 76 { self.model.openDocument(); return nil }
                if event.keyCode == 15 && event.modifierFlags.contains(.command) { self.model.revealDocument(); return nil }
            }
            if self.panel.firstResponder is ChatTextView { return event }
            guard self.model.canCompose,
                  event.keyCode == 36 || event.keyCode == 76,
                  let editor = self.panel.firstResponder as? NSTextView,
                  !editor.hasMarkedText() else { return event }
            if event.modifierFlags.contains(.shift) {
                editor.insertNewlineIgnoringFieldEditor(nil)
            } else {
                self.model.startWork()
            }
            return nil
        }) { eventMonitors.append(monitor) }
        updateScreen()
        overlaySpace.attach([panel, tracerPanel])
        NotificationCenter.default.addObserver(self, selector: #selector(updateScreen),
                                              name: NSApplication.didChangeScreenParametersNotification, object: nil)
        NSWorkspace.shared.notificationCenter.addObserver(self, selector: #selector(screenDidWake),
                                                          name: NSWorkspace.screensDidWakeNotification, object: nil)
        NSWorkspace.shared.notificationCenter.addObserver(self, selector: #selector(activeSpaceChanged),
                                                          name: NSWorkspace.activeSpaceDidChangeNotification, object: nil)
        NSWorkspace.shared.notificationCenter.addObserver(self, selector: #selector(applicationActivated(_:)),
                                                          name: NSWorkspace.didActivateApplicationNotification, object: nil)
        NSWorkspace.shared.notificationCenter.addObserver(self, selector: #selector(screensDidSleep),
                                                          name: NSWorkspace.screensDidSleepNotification, object: nil)
        if let monitor = NSEvent.addGlobalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown], handler: { [weak self] _ in
            guard let self else { return }
            let location = NSEvent.mouseLocation
            let cameBack = self.yieldController.ownerClicked(at: self.windowServerPoint(location))
            // The click went to another app: coming back must not take the focus the owner moved there.
            self.windowYield.ownerMovedElsewhere()
            if cameBack {
                // A click on the notch brought FERB back: focus the visible input, like hover and the shortcut.
                self.updatePointer()
                self.focusVisibleInput()
            }
            self.restackYield()
            guard self.model.expanded, !self.panel.frame.contains(location) else { return }
            self.model.dismissAutomatically()
        }) { eventMonitors.append(monitor) }
        // Global monitors do not receive clicks in our Settings/Quick Look windows.
        if let monitor = NSEvent.addLocalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown], handler: { [weak self] event in
            guard let self else { return event }
            // A click on FERB while it sits one layer back brings it to the front; the click then goes on to what
            // was clicked (the text input, a button).
            if event.window === self.panel && self.model.isYielded { self.beginInteraction() }
            if event.window !== self.panel && !self.pointerIsOverSurface {
                self.model.dismissAutomatically()
            }
            return event
        }) { eventMonitors.append(monitor) }
        // The physical camera has no visible app surface. Watch its screen coordinates
        // as well as the panel so hovering the real notch can open FERB.
        if let monitor = NSEvent.addGlobalMonitorForEvents(matching: [.mouseMoved, .leftMouseDragged], handler: { [weak self] _ in
            self?.updatePointer(revealPinnedHeader: true)
        }) { eventMonitors.append(monitor) }
        if let monitor = NSEvent.addLocalMonitorForEvents(matching: [.mouseMoved, .leftMouseDragged], handler: { [weak self] event in
            self?.updatePointer(revealPinnedHeader: true)
            return event
        }) { eventMonitors.append(monitor) }
    }

    private func configurePanel() {
        panel.title = "FERB Notch"
        panel.identifier = NSUserInterfaceItemIdentifier("FERBNotchPanel")
        panel.backgroundColor = .clear
        panel.isOpaque = false
        panel.hasShadow = false
        panel.isFloatingPanel = true
        panel.hidesOnDeactivate = false
        panel.canBecomeVisibleWithoutLogin = false
        panel.isMovable = false
        panel.acceptsMouseMovedEvents = true
        panel.appearance = NSAppearance(named: .darkAqua)
        panel.isReleasedWhenClosed = false
        panel.delegate = self
        panel.level = NSWindow.Level(rawValue: NSWindow.Level.mainMenu.rawValue + 1)
        panel.collectionBehavior = Self.overlayBehavior
        panel.animationBehavior = .none
        panel.onEscape = { [weak self] in self?.model.collapse(keyboard: true) }
        let hosting = TrackingHostingView(rootView: NotchView(model: model, presentation: presentation))
        hosting.sizingOptions = []
        hosting.entered = { [weak self] in self?.updatePointer() }
        hosting.exited = { [weak self] in self?.updatePointer() }
        hosting.autoresizingMask = [.width, .height]
        let backdrop = presentation.backdrop
        panel.contentView = backdrop
        hosting.frame = backdrop.bounds
        backdrop.addSubview(hosting)

        // This window never resizes with the dialog and never captures a click.
        // Its larger transparent canvas keeps the light outside the camera cutout.
        tracerPanel.title = "FERB Notch Light"
        tracerPanel.backgroundColor = .clear
        tracerPanel.isOpaque = false
        tracerPanel.hasShadow = false
        tracerPanel.hidesOnDeactivate = false
        tracerPanel.canBecomeVisibleWithoutLogin = false
        tracerPanel.isReleasedWhenClosed = false         // (click-through by NotchLightPanel itself)
        tracerPanel.level = NSWindow.Level(rawValue: panel.level.rawValue + 1)
        tracerPanel.collectionBehavior = Self.overlayBehavior
        tracerPanel.animationBehavior = .none
        let tracerHosting = NSHostingView(rootView: NotchTracerOverlay(model: model))
        tracerHosting.sizingOptions = []
        tracerPanel.contentView = tracerHosting
    }

    @objc private func updateScreen() {
        guard let screen = NSScreen.screens.first(where: { $0.safeAreaInsets.top > 0 }) ?? NSScreen.main ?? NSScreen.screens.first else { return }
        let current = NotchGeometry(screen: screen.frame, topInset: screen.safeAreaInsets.top,
                                    leftAreaMaxX: screen.auxiliaryTopLeftArea?.maxX,
                                    rightAreaMinX: screen.auxiliaryTopRightArea?.minX,
                                    menuBarHeight: screen.frame.maxY - screen.visibleFrame.maxY)
        geometry = current
        screenFrame = screen.frame
        // The strip whose windows are always-present chrome (menu bar, status items), never something to yield to.
        menuBarHeight = max(screen.frame.maxY - screen.visibleFrame.maxY, screen.safeAreaInsets.top,
                            NSStatusBar.system.thickness)
        model.handleWidth = current.handleWidth
        model.hasHardwareNotch = current.hasNotch
        // The camera housing, or a virtual notch over the menu bar's centre: both begin at the screen's top edge.
        model.cutoutHeight = current.cutoutFrame.height
        cutoutFrame = current.cutoutFrame
        tracerPanel.setFrame(frame(for: CGSize(width: model.handleWidth + 40,
                                               height: model.cutoutHeight + 24)), display: true)
        pointerIsInside = false
        present(animate: false, requestFocus: false)
    }

    @objc private func screenDidWake() {
        screensAsleep = false
        updateScreen()
        windowYield.showPanel()
        updateTracerVisibility()
    }

    @objc private func screensDidSleep() {
        screensAsleep = true
        updateYieldWatch()
    }

    @objc private func applicationActivated(_ notification: Notification) {
        guard let app = notification.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication else { return }
        let cameBack = yieldController.appActivated(app.processIdentifier)
        // Another app came forward: coming back must not take the focus the owner moved there.
        if app.processIdentifier != getpid() { windowYield.ownerMovedElsewhere() }
        if cameBack { updatePointer() }
        restackYield()
    }

    @objc private func activeSpaceChanged() {
        // Space changes are not display changes. Keep the same geometry, draft,
        // pin state, running spring and current text responder. In particular,
        // do not resignKey here: that discarded input focus on every swipe.
        hoverTask?.cancel()
        if !overlaySpace.isAttached {
            windowYield.showPanel()
            updateTracerVisibility()
        }
        pointerIsInside = pointerIsOverSurface
    }

    private func present(animate: Bool, requestFocus: Bool = true) {
        guard geometry != nil else { return }
        let isOpening = model.bodyVisible && !presentedBodyVisible
        presentedBodyVisible = model.bodyVisible
        // The owner opened or revealed FERB (background updates don't request focus): stay over the windows
        // already there rather than stepping aside right after taking focus.
        if isOpening && requestFocus { yieldController.ownerOpened() }
        let targetFrame = frame(for: model.panelSize)
        let targetSize = model.surfaceSize
        let shouldAnimate = animate && panel.isVisible
        let reduceMotion = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        let revision = UUID()
        presentationRevision = revision
        presentation.animates = shouldAnimate

        presentation.cutoutHeight = model.cutoutHeight
        if shouldAnimate && !reduceMotion {
            let canvas = CGSize(width: max(panel.frame.width, targetFrame.width),
                                height: max(panel.frame.height, targetFrame.height))
            panel.setFrame(frame(for: canvas), display: true)
        } else {
            panel.setFrame(targetFrame, display: true)
        }
        presentation.present(size: targetSize, expanded: model.expanded,
                             bodyVisible: !model.isHeaderOnly,
                             animate: shouldAnimate && !reduceMotion) { [weak self] in
            guard let self, self.presentationRevision == revision else { return }
            self.panel.setFrame(targetFrame, display: true)
        }
        windowYield.showPanel()
        updateTracerVisibility()
        updateYieldWatch()
        if !model.bodyVisible {
            panel.makeFirstResponder(nil)
            panel.resignKey()
        } else if requestFocus && !model.voiceIsHeld && !model.isYielded && (isOpening || panel.isKeyWindow) {
            // The held voice chord leaves the current application's keyboard
            // focus intact until release. Background task updates also leave
            // focus with the user's active app, including after a Space change.
            // An invisible (yielded) panel never takes focus.
            panel.makeKey()
            panel.contentView?.layoutSubtreeIfNeeded()
            DispatchQueue.main.async { [weak self] in
                guard let self, self.model.bodyVisible, self.presentationRevision == revision else { return }
                self.model.focusRequest = UUID()
            }
        }
    }

    private func frame(for size: CGSize) -> CGRect {
        CGRect(x: cutoutFrame.midX - size.width / 2, y: cutoutFrame.maxY - size.height,
               width: size.width, height: size.height)
    }

    private func updateTracerVisibility() {
        model.clickThrough.set("tracer", windows: [tracerPanel.windowNumber])
        windowYield.updateTracer(glowing: model.showsTraceGlow, lightShows: model.tracerLightShows)
    }

    /// The notch windows sit in an unmanaged overlay space above every ordinary window and other apps' menus, so
    /// no window level lets another surface win there. While FERB yields, the panel steps one layer back and stays
    /// on screen: just behind the app window it yields to, or under the menu (see NotchWindowYield; mouse-event
    /// transparency is never touched); content, draft, attachments and pin are untouched. Coming back re-keys the
    /// panel only if its body shows, it was key when the yield began, and the owner hasn't clicked or switched to
    /// another app meanwhile.
    private func applyYield(_ yielded: Bool) {
        if yielded {
            hoverTask?.cancel()
            yieldController.yieldBegan(pointer: windowServerPoint(NSEvent.mouseLocation))
            windowYield.yield(behind: yieldController.coveringWindow())
            // Left the overlay space for the ordinary Spaces (NotchOverlaySpace.leave); say so if AppKit didn't put
            // it on the current one, which would leave it off screen while it still counts as visible.
            if overlaySpace.isAttached && !panel.isOnActiveSpace {
                NSLog("FERB Notch: the stepped-back panel isn't on the current Space.")
            }
        } else {
            windowYield.comeBack(refocus: model.bodyVisible && !model.voiceIsHeld)
        }
        updateTracerVisibility()
    }

    /// FERB's drawn frames in window-server coordinates, from the model (see NotchYieldController.Layout).
    private func yieldLayout() -> NotchYieldController.Layout? {
        guard geometry != nil, !screenFrame.isEmpty else { return nil }
        let height = primaryDisplayHeight
        func server(_ rect: CGRect) -> CGRect { NotchOverlap.windowServerRect(rect, primaryDisplayHeight: height) }
        // FERB's drawn surface, not the panel window: its transparent margins pass clicks through.
        var occupied: [CGRect] = []
        if model.expanded { occupied.append(frame(for: model.surfaceSize)) }
        // On a notched display the collapsed pill sits inside the camera housing, where nothing can be drawn.
        else if !model.hasHardwareNotch { occupied.append(cutoutFrame) }
        if model.showsTraceGlow { occupied.append(tracerPanel.frame) }
        let menuBar = CGRect(x: screenFrame.minX, y: screenFrame.maxY - menuBarHeight,
                             width: screenFrame.width, height: menuBarHeight)
        return .init(occupied: occupied.map(server),
                     pinnedChat: model.isPinned && model.expanded ? server(frame(for: model.surfaceSize)) : nil,
                     // The cutout up to the physical top edge, not the band below it where menus begin.
                     notch: server(CGRect(x: cutoutFrame.minX, y: cutoutFrame.minY, width: cutoutFrame.width,
                                          height: cutoutFrame.height + 2)),
                     menuBar: server(menuBar), display: server(screenFrame))
    }

    private var primaryDisplayHeight: CGFloat { NSScreen.screens.first?.frame.maxY ?? screenFrame.maxY }

    private func windowServerPoint(_ point: CGPoint) -> CGPoint {
        NotchOverlap.windowServerPoint(point, primaryDisplayHeight: primaryDisplayHeight)
    }

    /// Reads the window list only while some part of FERB is on screen (or FERB is yielding and waiting to come
    /// back); with nothing of FERB visible, or the displays asleep, polling stops.
    private func updateYieldWatch() {
        let onScreen = !screensAsleep && !(yieldLayout()?.occupied.isEmpty ?? true)
        guard onScreen else {
            yieldWatch?.cancel()
            yieldWatch = nil
            // Nothing of FERB is visible, so another app's menu or window no longer applies. This runs inside
            // present(); hover is left to the next pointer move rather than re-entering the presentation here.
            yieldController.evaluate()
            return
        }
        guard yieldWatch == nil else { return }
        yieldWatch = Task { [weak self] in
            repeat {
                self?.evaluateYield()
                do { try await Task.sleep(for: Self.yieldPollInterval, tolerance: .milliseconds(50)) } catch { return }
            } while self != nil
        }
    }

    private func evaluateYield() {
        // Coming back after another app's menu or window went away re-evaluates hover, like closing a menu.
        if yieldController.evaluate() { updatePointer() }
        restackYield()
    }

    /// While FERB yields, keep the panel just behind what it yields to now (it moves only when that changed). Never
    /// while its own file picker is open: ordering the panel then could put it over the picker.
    private func restackYield() {
        if model.isYielded && !model.filePickerOpen { windowYield.restack(behind: yieldController.coveringWindow()) }
    }

    /// A shortcut while FERB yields to another app (not to its own file picker): FERB sits one layer back, so instead
    /// of closing it the shortcut brings it back over those windows and opens the chat with focused typing. Returns
    /// whether it handled the shortcut; when FERB is collapsed, the normal open path runs.
    private func summonFromYield() -> Bool {
        guard model.yielding.reasons.contains(where: { $0 != .filePicker }) else { return false }
        beginInteraction()
        guard model.expanded else { return false }
        if model.isHeaderOnly { model.revealPinnedBody() }
        else { focusVisibleInput() }
        return true
    }

    /// Hover, a click on the notch and the shortcut focus the visible text input; never while FERB still yields.
    private func focusVisibleInput() {
        if windowYield.focusInput(bodyVisible: model.bodyVisible) { model.focusRequest = UUID() }
    }

    private func beginInteraction() {
        // The owner reached for FERB (shortcut, menu or a control): stop yielding to an app under the pinned chat
        // and show FERB over the windows now over it.
        yieldController.summon()
        hoverTask?.cancel()
        if model.bodyVisible, !model.isYielded { panel.makeKey() }
    }

    private var pointerIsOverSurface: Bool {
        let point = NSEvent.mouseLocation
        let inCutout = cutoutFrame.insetBy(dx: 0, dy: -2).contains(point)
        let visibleSize = model.expanded ? model.panelSize : presentation.size
        // CGRect.contains excludes maxY. Include the physical screen edge over
        // the entire header, not just the narrow camera cutout.
        let hoverFrame = frame(for: visibleSize).insetBy(dx: 0, dy: -2)
        let inSurface = (model.expanded || presentation.expansion > 0.01) && hoverFrame.contains(point)
        return inCutout || inSurface
    }

    private func updatePointer(revealPinnedHeader: Bool = false) {
        // While FERB yields it is ordered out: hover must not open, reveal, fold or dismiss it. Entering the notch
        // brings it back, and like any hover that focuses the visible input.
        var broughtBack = false
        if model.isYielded {
            switch yieldController.pointerMoved(to: windowServerPoint(NSEvent.mouseLocation)) {
            case .hidden: return
            case .broughtBack: broughtBack = true
            case .visible: break
            }
        }
        defer { if broughtBack { focusVisibleInput() } }
        let inside = pointerIsOverSurface
        if inside && revealPinnedHeader && model.isPinned {
            // Only real pointer motion reveals the tucked body. Tracking-area
            // changes during the shrink must not immediately open it again.
            model.revealPinnedBody()
        }
        guard inside != pointerIsInside else { return }
        pointerIsInside = inside
        inside ? pointerEntered() : pointerExited()
    }

    private func pointerEntered() {
        hoverTask?.cancel()
        if model.isPinned {
            model.revealPinnedBody()
            return
        }
        guard !model.expanded else { return }
        hoverTask = Task { [weak self] in
            do { try await Task.sleep(for: .milliseconds(100)) } catch { return }
            self?.model.expand()
        }
    }

    private func pointerExited() {
        hoverTask?.cancel()
        if model.isPinned {
            model.foldPinnedBody()
            return
        }
        guard !model.blocksAutomaticDismissal, model.expanded else { return }
        hoverTask = Task { [weak self] in
            do { try await Task.sleep(for: .milliseconds(60)) } catch { return }
            self?.model.dismissAutomatically()
        }
    }

    private func configureMenu() {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        let menu = NSMenu()
        menu.delegate = self
        item.menu = menu
        statusItem = item
        updateStatusItem()
        rebuildMenu(menu)
        if let appMenu = NSApp.mainMenu?.items.first?.submenu {
            let settings = NSMenuItem(title: "Settings…", action: #selector(showSettings), keyEquivalent: ",")
            settings.target = self
            appMenu.insertItem(settings, at: 0)
            appMenu.insertItem(.separator(), at: 1)
        }
    }

    /// The notch is in front of the owner: open, not stepped back, and the displays awake.
    private var notchShowing: Bool { model.bodyVisible && !model.isYielded && !screensAsleep }

    /// A clicked notification: the owner reached for FERB, so it comes forward at that item.
    func open(_ target: NotificationTarget) {
        beginInteraction()
        model.open(target)
    }

    private func queueStatusRefresh() {
        guard !statusRefreshQueued else { return }
        statusRefreshQueued = true
        DispatchQueue.main.async { [weak self] in        // objectWillChange fires before the change lands
            guard let self else { return }
            self.statusRefreshQueued = false
            self.updateStatusItem()
        }
    }

    private func updateStatusItem() {
        let state = model.menuBarState
        statusItem?.button?.image = NSImage(systemSymbolName: state.symbol, accessibilityDescription: state.statusLine)
        statusItem?.button?.toolTip = state.statusLine
    }

    func menuNeedsUpdate(_ menu: NSMenu) { rebuildMenu(menu) }

    private func rebuildMenu(_ menu: NSMenu) {
        menu.removeAllItems()
        for entry in StatusMenu.entries(state: model.menuBarState, paused: model.daemonPaused,
                                        stopped: model.daemonStopped, appName: AppName.current,
                                        canSetUp: onSetUp != nil, hasWindow: hasWindow, updateOffer: updateOffer) {
            switch entry {
            case .status(let text):
                let line = NSMenuItem(title: text, action: nil, keyEquivalent: "")
                line.isEnabled = false
                menu.addItem(line)
            case .separator:
                menu.addItem(.separator())
            case .item(let title, let action, let key):
                let item = NSMenuItem(title: title, action: #selector(menuAction(_:)), keyEquivalent: key)
                item.target = self
                item.representedObject = action.rawValue
                menu.addItem(item)
            }
        }
    }

    @objc private func menuAction(_ sender: NSMenuItem) {
        guard let raw = sender.representedObject as? String, let action = MenuAction(rawValue: raw) else { return }
        perform(action)
    }

    func perform(_ action: MenuAction) {
        switch action {
        case .openFERB: showFromMenu()
        case .openWindow: onOpenWindow?()
        case .pause: setPaused(true)
        case .resume: setPaused(false)
        case .liftStop: liftStop()
        case .restart: restartDaemon()
        case .settings: showSettings()
        case .setUp: onSetUp?()
        case .quit: NSApp.terminate(nil)
        case .installUpdate: onInstallUpdate?()
        }
    }

    /// Pause FERB is `ferb pause` (sense.pause): nothing is recorded and no real mouse or keyboard input until Resume.
    private func setPaused(_ paused: Bool) {
        Task { [weak self] in
            guard let self else { return }
            do {
                _ = try await self.model.rpc.call(paused ? "sense.pause" : "sense.resume", params: [:])
                self.model.daemonPaused = paused
            } catch { self.model.promptError = error.localizedDescription }
        }
    }

    private func liftStop() {
        Task { [weak self] in
            guard let self else { return }
            do {
                _ = try await self.model.rpc.call("stop.lift", params: [:])
                self.model.daemonStopped = false
            } catch { self.model.promptError = error.localizedDescription }
        }
    }

    /// FERB isn't running — Restart: launchd starts the installed background service again; with none installed, the
    /// packaged app opens Set Up FERB… and a development build says how.
    private func restartDaemon() {
        Task { [weak self] in
            guard let self else { return }
            if await self.service.restart() {
                self.model.daemonLink.restarting()
            } else if let setUp = self.onSetUp {
                setUp()
            } else {
                let alert = NSAlert()
                alert.messageText = "FERB's background service isn't installed."
                alert.informativeText = "From a checkout, run `ferb setup`."
                NSApp.activate(ignoringOtherApps: true)
                alert.runModal()
            }
        }
    }

    @objc func showFromMenu() { beginInteraction(); model.expand() }

    private func registerShortcut() {
        var eventType = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
        let context = Unmanaged.passUnretained(self).toOpaque()
        InstallEventHandler(GetApplicationEventTarget(), { _, _, context in
            guard let context else { return OSStatus(eventNotHandledErr) }
            let coordinator = Unmanaged<PanelCoordinator>.fromOpaque(context).takeUnretainedValue()
            Task { @MainActor in coordinator.shortcutPressed() }
            return noErr
        }, 1, &eventType, context, &hotKeyHandler)
        let id = EventHotKeyID(signature: OSType(0x46455242), id: 1)
        let status = RegisterEventHotKey(UInt32(kVK_Space), UInt32(controlKey | optionKey), id,
                                        GetApplicationEventTarget(), 0, &hotKey)
        if status != noErr { NSLog("FERB Notch: shortcut unavailable; use the menu-bar control (%d).", status) }
    }

    private func shortcutPressed() {
        if summonFromYield() { return }
        if model.expanded { model.collapse(keyboard: true); return }
        beginInteraction()
        if model.session.phase == .ready || model.session.phase == .draft { model.edit(keyboard: true) }
        else { model.expand(keyboard: true); panel.makeKey() }
    }

    @objc private func showSettings() {
        if !model.isPinned { model.collapse() }
        onOpenSettings?()
    }

    private func quickLook(_ url: URL) {
        previewURL = url
        beginInteraction()
        guard let preview = QLPreviewPanel.shared() else { return }
        preview.updateController()
        preview.makeKeyAndOrderFront(nil)
        if !model.isPinned { model.collapse() }
        if preview.currentController as AnyObject? === self { preview.reloadData() }
    }

    override func acceptsPreviewPanelControl(_ panel: QLPreviewPanel!) -> Bool { previewURL != nil }

    override func beginPreviewPanelControl(_ panel: QLPreviewPanel!) {
        panel.dataSource = self
        panel.delegate = self
        panel.reloadData()
    }

    override func endPreviewPanelControl(_ panel: QLPreviewPanel!) {
        panel.dataSource = nil
        panel.delegate = nil
    }

    func numberOfPreviewItems(in panel: QLPreviewPanel!) -> Int { previewURL == nil ? 0 : 1 }

    func previewPanel(_ panel: QLPreviewPanel!, previewItemAt index: Int) -> (any QLPreviewItem)! {
        previewURL as NSURL?
    }

    func tearDown() {
        poster?.stop()
        statusRefresh = nil
        model.disconnectDaemon()
        shortcuts.stop()
        model.cancelPinnedFold()
        presentation.stop()
        panel.orderOut(nil)
        tracerPanel.orderOut(nil)
        overlaySpace.stop()
        presentationRevision = UUID()
        hoverTask?.cancel()
        yieldWatch?.cancel()
        yieldWatch = nil
        yieldController.stop()
        if let hotKey { UnregisterEventHotKey(hotKey) }
        if let hotKeyHandler { RemoveEventHandler(hotKeyHandler) }
        eventMonitors.forEach(NSEvent.removeMonitor)
        NotificationCenter.default.removeObserver(self)
        NSWorkspace.shared.notificationCenter.removeObserver(self)
        if let statusItem { NSStatusBar.system.removeStatusItem(statusItem) }
    }
}
