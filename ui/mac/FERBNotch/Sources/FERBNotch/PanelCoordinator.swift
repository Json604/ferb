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

/// The notch's glow: click-through windows reported to the daemon (ClickThroughReport, D31 fix round 4).
class NotchLightPanel: NSPanel {
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
    private var textRefreshQueued = false

    /// The transcript stays mounted while its window is hidden and compacted.
    /// Refresh retained selectable text after the reveal has finished, without
    /// replacing native views, text storage, or the owner's selected ranges.
    func scheduleTextDisplayRefresh() {
        guard !textRefreshQueued else { return }
        textRefreshQueued = true
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            self.textRefreshQueued = false
            self.layoutSubtreeIfNeeded()
            self.invalidateRetainedText(in: self)
        }
    }

    private func invalidateRetainedText(in view: NSView) {
        // Reading layoutManager on a TextKit 2 view switches it to compatibility
        // mode. Leave that renderer intact; its view invalidation below suffices.
        if let text = view as? NSTextView, text.textLayoutManager == nil, let storage = text.textStorage {
            text.layoutManager?.invalidateDisplay(forCharacterRange: NSRange(location: 0, length: storage.length))
        }
        view.needsDisplay = true
        for child in view.subviews { invalidateRetainedText(in: child) }
    }

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
    let model: NotchModel
    private let presentation = NotchPresentation()
    private weak var hostingView: TrackingHostingView<NotchView>?
    private let shortcuts = NotchShortcutMonitor()
    /// The owner's controls as they happen, for the intro's Control beat (IntroController).
    let introControls = IntroControlSignals()
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
    private let indicatorPanel = FerbIndicatorPanel(contentRect: CGRect(x: 0, y: 0, width: 58, height: 30),
                                                    styleMask: [.borderless, .nonactivatingPanel],
                                                    backing: .buffered, defer: false)
    private var activityPanel: TaskActivityPanel?
    private var activityTaskID: String?
    private var geometry: NotchGeometry?
    private var statusItem: NSStatusItem?
    private var hoverTask: Task<Void, Never>?
    private var indicatorRestTask: Task<Void, Never>?
    private(set) weak var indicatorControl: FerbIndicatorControl?
    private var presentedExpanded = false
    private var isDocked = false
    private var dockingRevision: UUID?
    let magneticFieldPanel = NotchLightPanel(contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    private let magneticField = MagneticFieldView(frame: .zero)
    var magneticFieldPresentationEnabled = true
    private var magneticDocking = MagneticDocking()
    private(set) var magneticSample = MagneticDocking.Sample()
    private var magneticClock: CADisplayLink?
    private var magneticRawPill = CGRect.zero
    private var magneticPointerVelocity = CGPoint.zero
    private var magneticPointerTime: TimeInterval = 0
    private var magneticLastPointer = CGPoint.zero
    private var fusionCardStart: CGRect?
    private var fusionCardTarget: CGRect?
    private var fusionCanvas = CGRect.zero
    private var pointerIsInside = false
    private var cutoutFrame = CGRect.zero
    private(set) var indicatorFrame = CGRect.zero
    private var indicatorEdge: FERBScreenEdge = .right
    private var indicatorScreen: NSScreen?
    private var indicatorFraction: CGFloat = 0.5
    private var indicatorDragPointer: CGPoint?
    private var indicatorDragFrame: CGRect?
    private var panelDragFrame: CGRect?
    private var dragCard: CGRect?
    private var dragMotion: DragChatMotion?
    private var dragCanvasActive = false
    private var dragPreferredSize = CGSize.zero
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
    var onSetUp: (() -> Void)?                   // set by the packaged FERB.app: Set Up FERB… replays the intro
    var onRepair: (() -> Void)?                  // set by the packaged FERB.app: the service set up again, no tour
    var poster: NotificationPoster?              // set by AppDelegate before start() (L34 D)
    var hasWindow = false                        // Phase E: the FERB window exists ("Open FERB Window")
    var onOpenWindow: (() -> Void)?              // Phase E
    var updateOffer: String?                     // L34 Phase G: an update waiting for the owner's click
    var onInstallUpdate: (() -> Void)?
    var onOpenSettings: (() -> Void)?            // the FERB window at Settings (L34 Phase E): ⌘, and Settings…
    var service: DaemonServicing = DaemonService()
    private var statusRefresh: AnyCancellable?
    private var statusRefreshQueued = false
    private var isStarted = false

    let cursor = CursorController()

    /// `model` is for tests (a scripted daemon); the app uses its own.
    init(model: NotchModel? = nil) {
        let model = model ?? NotchModel()
        self.model = model
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
        configureIndicator()
        model.onPresentationChange = { [weak self] animate in self?.present(animate: animate) }
        model.onAttentionArrival = { [weak self] in self?.hoverTask?.cancel() }
        model.onBackgroundChange = { [weak self] animate in self?.present(animate: animate, requestFocus: false) }
        model.onInteractionEnded = { [weak self] in
            guard let self, !self.pointerIsOverSurface else { return }
            self.pointerExited()
        }
        model.onInteraction = { [weak self] in self?.beginInteraction() }
        model.onPreviewWindowChange = { [weak self] id, window, attached in
            guard let self else { return }
            if attached {
                self.previewWindows[id] = window
                window.level = NSWindow.Level(rawValue: self.panel.level.rawValue + 1)
                self.overlaySpace.include(window)
            } else {
                self.previewWindows.removeValue(forKey: id)
                if !self.previewWindows.values.contains(where: { $0 === window }) {
                    self.overlaySpace.remove(window)
                }
            }
            self.updatePointer()
        }
        model.onDetachTaskActivity = { [weak self] taskID in self?.showTaskActivity(taskID) }
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
        isStarted = true
        model.connectDaemon()
        configureMenu()
        poster?.start(rpc: model.rpc, notchShowing: { [weak self] in self?.notchShowing ?? false },
                      open: { [weak self] target in self?.open(target) })
        model.onApprovalResolved = { [weak self] id in self?.poster?.withdrawQuestion(id) }
        statusRefresh = model.objectWillChange.sink { [weak self] _ in self?.queueStatusRefresh() }
        registerShortcut()
        shortcuts.onDoubleTap = { [weak self] in self?.modifierDoubleTapped() }
        shortcuts.onHoldBegan = { [weak self] in
            self?.hoverTask?.cancel()
            self?.model.beginHeldVoice()
            self?.introControls.saw(.talk)
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
            // Return in a field on FERB's intro line submits that field (its own onSubmit), never the composer.
            if self.model.introLine?.entry != nil, (self.panel.firstResponder as? NSTextView)?.isFieldEditor == true {
                return event
            }
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
        magneticFieldPanel.backgroundColor = .clear
        magneticFieldPanel.isOpaque = false
        magneticFieldPanel.hasShadow = false
        magneticFieldPanel.level = NSWindow.Level(rawValue: NSWindow.Level.mainMenu.rawValue + 2)
        magneticFieldPanel.collectionBehavior = Self.overlayBehavior
        magneticFieldPanel.contentView = magneticField
        overlaySpace.attach([panel, tracerPanel, indicatorPanel, magneticFieldPanel])
        NotificationCenter.default.addObserver(self, selector: #selector(displayParametersChanged),
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
            PreviewHoverRouter.shared.routeCurrentPointer()
            self?.updatePointer(revealPinnedHeader: true)
        }) { eventMonitors.append(monitor) }
        if let monitor = NSEvent.addLocalMonitorForEvents(matching: [.mouseMoved, .leftMouseDragged], handler: { [weak self] event in
            PreviewHoverRouter.shared.routeCurrentPointer()
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
        hostingView = hosting
        hosting.sizingOptions = []
        hosting.entered = { [weak self] in self?.updatePointer() }
        hosting.exited = { [weak self] in self?.updatePointer() }
        hosting.autoresizingMask = [.width, .height]
        let backdrop = presentation.backdrop
        panel.contentView = backdrop
        hosting.frame = backdrop.bounds
        backdrop.addSubview(hosting)
        let resizeOverlay = PanelResizeOverlay(model: model)
        resizeOverlay.frame = backdrop.bounds
        backdrop.addSubview(resizeOverlay)

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

    private func configureIndicator() {
        indicatorPanel.title = "FERB edge indicator"
        indicatorPanel.backgroundColor = .clear
        indicatorPanel.isOpaque = false
        indicatorPanel.hasShadow = false
        indicatorPanel.hidesOnDeactivate = false
        indicatorPanel.canBecomeVisibleWithoutLogin = false
        indicatorPanel.isReleasedWhenClosed = false
        indicatorPanel.level = NSWindow.Level(rawValue: NSWindow.Level.mainMenu.rawValue + 1)
        indicatorPanel.collectionBehavior = Self.overlayBehavior
        indicatorPanel.animationBehavior = .none
        indicatorPanel.acceptsMouseMovedEvents = true
        let control = FerbIndicatorControl(model: model)
        indicatorControl = control
        control.onPointerChanged = { [weak self] in self?.updatePointer() }
        control.onDragBegan = { [weak self] point in
            guard let self else { return }
            self.hoverTask?.cancel()
            let interruptedFusion = self.dockingRevision != nil
            self.dockingRevision = nil
            self.cancelMagneticEffects()
            self.magneticClock?.invalidate()
            self.magneticClock = nil
            self.fusionCardStart = nil
            self.fusionCardTarget = nil
            self.magneticDocking.begin(at: CACurrentMediaTime(), tornOut: self.isDocked)
            self.magneticSample = self.magneticDocking.current
            self.magneticPointerTime = CACurrentMediaTime()
            self.magneticLastPointer = point
            self.magneticPointerVelocity = .zero
            if !interruptedFusion { self.setIndicatorResting(false, animated: false) }
            self.presentation.stop()
            self.model.acknowledgement.dismiss()
            self.indicatorDragPointer = point
            self.indicatorDragFrame = self.indicatorFrame
            self.dragPreferredSize = self.model.surfaceSize
            self.panelDragFrame = self.panel.frame
            self.dragCard = CGRect(x: self.panel.frame.minX + self.presentation.surfaceOrigin.x,
                y: self.panel.frame.maxY - self.presentation.surfaceOrigin.y - self.presentation.size.height,
                width: self.presentation.size.width, height: self.presentation.size.height)
        }
        control.onDragged = { [weak self] point in self?.moveIndicator(with: point) }
        control.onDragEnded = { [weak self] point in self?.snapIndicator(at: point) }
        control.onClick = { [weak self] in
            guard let self else { return }
            self.cancelMagneticInteraction()
            self.beginInteraction()
            self.model.toggle()
        }
        indicatorPanel.contentView = control
    }

    private func showTaskActivity(_ taskID: String) {
        guard model.taskItems.contains(where: { $0.id == taskID && $0.isRunning }) else { return }
        if activityTaskID == taskID, let activityPanel {
            activityPanel.orderFrontRegardless()
            return
        }
        closeTaskActivity()
        let window = TaskActivityPanel(model: model, taskID: taskID) { [weak self] in self?.closeTaskActivity() }
        activityTaskID = taskID
        activityPanel = window
        overlaySpace.include(window)
        window.orderFrontRegardless()
    }

    private func closeTaskActivity() {
        guard let window = activityPanel else { return }
        activityPanel = nil
        activityTaskID = nil
        overlaySpace.remove(window)
        window.orderOut(nil)
    }

    /// The expanded control is a subview of the chat window: one WindowServer move.
    private func attachIndicator() {
        guard let control = indicatorControl else { return }
        if control.superview !== presentation.backdrop {
            indicatorPanel.contentView = nil
            control.removeFromSuperview()
            presentation.backdrop.addSubview(control)
        }
        control.autoresizingMask = []
        control.isAttached = true
        control.frame = CGRect(x: indicatorFrame.minX - panel.frame.minX,
                               y: panel.frame.maxY - indicatorFrame.maxY,
                               width: indicatorFrame.width, height: indicatorFrame.height)
        control.isDocked = isDocked
        // The camera and its reachable lower lip belong to tear-out, never resize.
        for case let resize as PanelResizeOverlay in presentation.backdrop.subviews {
            resize.dragExclusionRect = control.frame
        }
        presentation.backdrop.attachmentRect = isDocked ? nil : control.frame
        presentation.backdrop.attachmentEdge = indicatorEdge
        indicatorPanel.orderOut(nil)
    }

    private func detachIndicator() {
        guard let control = indicatorControl, control.isAttached else { return }
        control.removeFromSuperview()
        control.isAttached = false
        presentation.backdrop.attachmentRect = nil
        indicatorPanel.contentView = control
        indicatorPanel.setFrame(indicatorFrame, display: true)
    }

    private func moveIndicator(with point: CGPoint) {
        guard let origin = indicatorDragPointer, let start = indicatorDragFrame else { return }
        if isDocked {
            // The first real drag tears out; a click still just opens/closes chat.
            isDocked = false
            model.isNotchDocked = false
            presentation.backdrop.docked = false
            NotchDocking.save(false, in: .standard)
            indicatorControl?.isDocked = false
            indicatorEdge = .top
            model.indicatorEdge = "top"
            indicatorFrame = CGRect(x: origin.x - 29, y: origin.y - 15, width: 58, height: 30)
            indicatorDragFrame = indicatorFrame
            if model.expanded {
                // The pill emerges under the grabbed point, including a grab farther
                // down the empty header. Subsequent motion translates one window.
                let old = panel.frame
                panel.setFrame(CGRect(x: old.minX, y: origin.y + 15 - old.height - 28,
                                      width: old.width, height: old.height + 28), display: true)
                presentation.destination = CGPoint(x: 8, y: 28)
                presentation.present(size: model.surfaceSize, expanded: true, bodyVisible: model.bodyVisible,
                                     anchor: nil, animate: false, completion: {})
                indicatorFrame = CGRect(x: origin.x - 29, y: origin.y - 15, width: 58, height: 30)
                indicatorDragFrame = indicatorFrame
                attachIndicator()
                panelDragFrame = panel.frame
            } else {
                indicatorPanel.setFrame(indicatorFrame, display: true)
            }
            moveIndicator(with: point)
            return
        }
        let previous = indicatorFrame
        let now = CACurrentMediaTime()
        let dt = now - magneticPointerTime
        if dt > 0 {
            magneticPointerVelocity = CGPoint(x: (point.x-magneticLastPointer.x)/dt,
                                             y: (point.y-magneticLastPointer.y)/dt)
        }
        magneticPointerTime = now
        magneticLastPointer = point
        magneticRawPill = PanelLayout.draggedFrame(start: start, grabbedAt: origin, pointer: point)
        guard let screen = NSScreen.screens.first(where: { $0.frame.contains(point) }) ?? indicatorScreen else { return }
        if let previousScreen = indicatorScreen, displayID(previousScreen) != displayID(screen) {
            // A display link and its field belong to one physical display. Start a
            // new field without carrying attraction from a notch on another screen.
            magneticClock?.invalidate()
            magneticClock = nil
            cancelMagneticEffects()
            magneticDocking.cancel(at: now)
            magneticDocking.begin(at: now)
        }
        indicatorScreen = screen
        sampleMagneticDrag(at: now)
        startMagneticClock(on: screen)
        if model.expanded {
            let preferred = dragPreferredSize
            let target = LiveDragLayout.resolve(pill: indicatorFrame, screen: screen.frame,
                preferred: preferred, previous: indicatorEdge)
            if dragMotion == nil {
                dragMotion = DragChatMotion(card: dragCard ?? target.card, edge: indicatorEdge)
            }
            indicatorEdge = target.edge
            // Content order stays stable while held; only the four attachment states change.
            dragCard = dragMotion?.update(target:target.card, edge:target.edge,
                delta:CGPoint(x:indicatorFrame.minX-previous.minX,y:indicatorFrame.minY-previous.minY),
                at:CACurrentMediaTime(), reduceMotion:NSWorkspace.shared.accessibilityDisplayShouldReduceMotion,
                visibleCard:dragCard)
            renderDragCard()

        } else { indicatorPanel.setFrame(indicatorFrame, display: true) }
        pointerIsInside = true
    }

    private func renderDragCard() {
        guard var card = dragCard else { return }
        if let screen = indicatorScreen {
            card = LiveDragLayout.connectedCard(card, pill: indicatorFrame, screen: screen.frame)
        }
        dragCard = card
        if model.availableSurfaceSize != card.size { model.availableSurfaceSize = card.size }
        // Keep the native window stationary while the pointer is captured. Only
        // the chat moves relative to the pill; restore a tight canvas on release.
        guard let control = indicatorControl, let screen = indicatorScreen else { return }
        dragCanvasActive = true
        presentation.backdrop.performUpdates {
            DragSurfaceCanvas.place(window: panel, presentation: presentation, control: control,
                pill: indicatorFrame, card: card, canvas: screen.frame, bodyVisible: !model.isHeaderOnly)
            attachIndicator()
            // Follow the currently drawn boundary through a reversal rather than
            // switching the concave shoulder to the eventual side ahead of the card.
            if card.minX > indicatorFrame.midX { presentation.backdrop.attachmentEdge = .left }
            else if card.maxX < indicatorFrame.midX { presentation.backdrop.attachmentEdge = .right }
            else if card.minY > indicatorFrame.midY { presentation.backdrop.attachmentEdge = .bottom }
            else if card.maxY < indicatorFrame.midY { presentation.backdrop.attachmentEdge = .top }
        }
    }

    private func stopDragLayout() {
        // Shrink before docking or ordinary placement: never retain a transparent
        // display-sized event surface after mouse-up.
        if dragCanvasActive, let control = indicatorControl, model.expanded {
            let card = CGRect(x: panel.frame.minX + presentation.surfaceOrigin.x,
                y: panel.frame.maxY - presentation.surfaceOrigin.y - presentation.size.height,
                width: presentation.size.width, height: presentation.size.height)
            DragSurfaceCanvas.place(window: panel, presentation: presentation, control: control,
                pill: indicatorFrame, card: card, canvas: card.union(indicatorFrame), bodyVisible: !model.isHeaderOnly)
            attachIndicator()
        }
        dragCard = nil
        dragMotion = nil
        dragCanvasActive = false
    }

    private func snapIndicator(at point: CGPoint) {
        guard indicatorDragPointer != nil else { return }
        stopDragLayout()
        let screen = NSScreen.screens.first(where: { $0.frame.contains(point) }) ??
            NSScreen.screens.min(by: { distance(point, to: $0.frame) < distance(point, to: $1.frame) })
        guard let screen else { return }
        let targetGeometry = NotchGeometry(screen: screen.frame, topInset: screen.safeAreaInsets.top,
            leftAreaMaxX: screen.auxiliaryTopLeftArea?.maxX, rightAreaMinX: screen.auxiliaryTopRightArea?.minX,
            menuBarHeight: screen.frame.maxY - screen.visibleFrame.maxY)
        if NotchDocking.accepts(point, cutout: targetGeometry.cutoutFrame, hasHardwareNotch: targetGeometry.hasNotch) {
            dockIndicator(on: screen, cutout: targetGeometry.cutoutFrame)
            return
        }
        let frame = screen.frame
        let edge = LiveDragLayout.nearestEdge(pill:indicatorFrame,screen:frame,previous:indicatorEdge)
        indicatorScreen = screen
        indicatorEdge = edge
        model.indicatorEdge = edge.rawValue
        let size = CGSize(width: 58, height: 30)
        switch edge {
        case .top:
            let x = safeTopIndicatorX(point.x - size.width / 2, size: size, screen: screen)
            indicatorFraction = min(1, max(0, (x - frame.minX) / max(frame.width - size.width, 1)))
        case .bottom:
            let x = min(frame.maxX - size.width, max(frame.minX, point.x - size.width / 2))
            indicatorFraction = min(1, max(0, (x - frame.minX) / max(frame.width - size.width, 1)))
        case .left, .right:
            indicatorFraction = min(1, max(0, (point.y - frame.minY - size.height / 2) / max(frame.height - size.height, 1)))
        }
        magneticClock?.invalidate()
        magneticClock = nil
        magneticDocking.cancel(at: CACurrentMediaTime())
        magneticSample = magneticDocking.current
        cancelMagneticEffects()
        persistIndicatorAnchor(for: screen)
        indicatorDragPointer = nil
        indicatorDragFrame = nil
        pointerIsInside = false
        updateScreen()
        updatePointer()
    }

    private func startMagneticClock(on screen: NSScreen) {
        guard magneticClock == nil else { return }
        let clock = screen.displayLink(target: self, selector: #selector(tickMagnetic(_:)))
        magneticClock = clock
        clock.add(to: .main, forMode: .common)
    }

    private func sampleMagneticDrag(at time: TimeInterval) {
        guard let screen = indicatorScreen else { return }
        let notch = NotchGeometry(screen: screen.frame, topInset: screen.safeAreaInsets.top,
            leftAreaMaxX: screen.auxiliaryTopLeftArea?.maxX,
            rightAreaMinX: screen.auxiliaryTopRightArea?.minX,
            menuBarHeight: screen.frame.maxY-screen.visibleFrame.maxY)
        updateMagneticDrag(pill: magneticRawPill, target: notch.hasNotch ? notch.cutoutFrame : nil,
            at: time, reduceMotion: NSWorkspace.shared.accessibilityDisplayShouldReduceMotion,
            velocity: time-magneticPointerTime < 0.08 ? magneticPointerVelocity : .zero)
    }

    func updateMagneticDrag(pill: CGRect, target: CGRect?, at time: TimeInterval,
                            reduceMotion: Bool, velocity: CGPoint) {
        magneticSample = magneticDocking.update(pill: pill, target: target, at: time,
                                                reduceMotion: reduceMotion, pointerVelocity: velocity)
        indicatorFrame = MagneticFieldGeometry.strainedPill(magneticSample)
        applyMagneticEffects()
    }

    @objc private func tickMagnetic(_ link: CADisplayLink) {
        let time = CACurrentMediaTime()
        if dockingRevision != nil {
            advanceMagneticFusion(at: time,
                reduceMotion: NSWorkspace.shared.accessibilityDisplayShouldReduceMotion)
        } else if indicatorDragPointer != nil {
            let previous = indicatorFrame
            sampleMagneticDrag(at: time)
            if model.expanded {
                if let card = dragCard {
                    let delta = CGPoint(x: indicatorFrame.minX-previous.minX, y: indicatorFrame.minY-previous.minY)
                    // Translate the orientation spring too, preserving its velocity.
                    if let screen = indicatorScreen {
                        let target = LiveDragLayout.resolve(pill: indicatorFrame, screen: screen.frame,
                            preferred: dragPreferredSize, previous: indicatorEdge)
                        dragCard = dragMotion?.update(target: target.card, edge: target.edge, delta: delta,
                            at: time, reduceMotion: NSWorkspace.shared.accessibilityDisplayShouldReduceMotion,
                            visibleCard: card) ?? card.offsetBy(dx: delta.x, dy: delta.y)
                        indicatorEdge = target.edge
                    }
                }
                renderDragCard()
            } else { indicatorPanel.setFrame(indicatorFrame, display: true) }
        } else {
            magneticClock?.invalidate()
            magneticClock = nil
        }
    }

    private func dockIndicator(on screen: NSScreen, cutout: CGRect) {
        indicatorDragPointer = nil
        indicatorDragFrame = nil
        hoverTask?.cancel()
        indicatorRestTask?.cancel()
        indicatorRestTask = nil
        indicatorScreen = screen
        dockingRevision = UUID()
        presentation.stop()
        // Keep the expanded conversation mounted and visible throughout the join.
        // Both surfaces reach their final docked rectangles on this same clock.
        if model.expanded {
            fusionCardStart = CGRect(x: panel.frame.minX+presentation.surfaceOrigin.x,
                y: panel.frame.maxY-presentation.surfaceOrigin.y-presentation.size.height,
                width: presentation.size.width, height: presentation.size.height)
            let budget = PanelLayout.surfaceBudget(screen: screen.frame, edge: .top, docked: true)
            model.availableSurfaceSize = budget
            let size = model.surfaceSize
            fusionCardTarget = CGRect(x: cutout.midX-size.width/2, y: cutout.maxY-size.height,
                                      width: size.width, height: size.height)
            fusionCanvas = fusionCardStart!.union(fusionCardTarget!).union(indicatorFrame).union(cutout)
        } else {
            fusionCardStart = nil
            fusionCardTarget = nil
        }
        beginMagneticFusion(to: model.expanded ? NotchDocking.dragRegion(cutout: cutout) : cutout,
                            at: CACurrentMediaTime())
        // The velocity contribution t*(1-t/T)^3 peaks at T*27/256.
        // Reserve its complete excursion once; keep WindowServer and children
        // on a stable canvas throughout even a very fast release.
        let excursion = CGFloat(MagneticDocking.fusionDuration) * 27 / 256
        fusionCanvas = fusionCanvas.insetBy(dx: -abs(magneticDocking.current.velocity.x) * excursion,
                                           dy: -abs(magneticDocking.current.velocity.y) * excursion).integral
        magneticSample = magneticDocking.sample(at: CACurrentMediaTime(),
            reduceMotion: NSWorkspace.shared.accessibilityDisplayShouldReduceMotion)
        renderMagneticFusion()
        if magneticSample.completed { finishMagneticFusion() }
        else { startMagneticClock(on: screen) }
    }

    func beginMagneticFusion(to target: CGRect, at time: TimeInterval) {
        indicatorDragPointer = nil
        indicatorDragFrame = nil
        dockingRevision = UUID()
        magneticDocking.release(to: target, at: time)
    }

    func advanceMagneticFusion(at time: TimeInterval, reduceMotion: Bool) {
        guard dockingRevision != nil else { return }
        magneticSample = magneticDocking.sample(at: time, reduceMotion: reduceMotion)
        renderMagneticFusion()
        if magneticSample.completed { finishMagneticFusion() }
    }

    private func renderMagneticFusion() {
        presentation.backdrop.magneticJoin = magneticSample.fusionProgress
        indicatorFrame = MagneticFieldGeometry.strainedPill(magneticSample)
        if let start = fusionCardStart, let target = fusionCardTarget, let control = indicatorControl {
            let card = magneticSample.fusionRect(from: start, to: target)
            presentation.backdrop.performUpdates {
                DragSurfaceCanvas.place(window: panel, presentation: presentation, control: control,
                    pill: indicatorFrame, card: card, canvas: fusionCanvas, bodyVisible: !model.isHeaderOnly)
                attachIndicator()
            }
        } else { indicatorPanel.setFrame(indicatorFrame, display: true) }
        applyMagneticEffects()
    }

    private func applyMagneticEffects() {
        indicatorControl?.setMagneticFusion(magneticSample.fusionProgress)
        guard let target = magneticSample.target, magneticSample.proximity > 0,
              magneticSample.phase == .dragging || magneticSample.phase == .fusing else {
            magneticFieldPanel.orderOut(nil)
            return
        }
        let canvas = indicatorFrame.union(target).insetBy(dx: -8, dy: -8).integral
        magneticFieldPanel.setFrame(canvas, display: false)
        magneticField.frame = CGRect(origin: .zero, size: canvas.size)
        magneticField.update(magneticSample, canvas: canvas)
        if magneticFieldPresentationEnabled && !magneticFieldPanel.isVisible { magneticFieldPanel.orderFrontRegardless() }
    }

    var hasMagneticInteraction: Bool {
        indicatorDragPointer != nil || dockingRevision != nil || magneticClock != nil
    }

    /// Revoke completion ownership before stopping effects. Any queued display
    /// callback then observes idle, so it cannot persist or finish an old join.
    func cancelMagneticInteraction() {
        indicatorControl?.cancelPointerCapture()
        dockingRevision = nil
        magneticClock?.invalidate()
        magneticClock = nil
        indicatorDragPointer = nil
        indicatorDragFrame = nil
        panelDragFrame = nil
        stopDragLayout()
        fusionCardStart = nil
        fusionCardTarget = nil
        fusionCanvas = .zero
        magneticPointerVelocity = .zero
        magneticDocking.cancel(at: CACurrentMediaTime())
        magneticSample = magneticDocking.current
        cancelMagneticEffects()
    }

    func cancelMagneticEffects() {
        magneticFieldPanel.orderOut(nil)
        indicatorControl?.setMagneticFusion(0)
        presentation.backdrop.magneticJoin = nil
    }

    private func finishMagneticFusion() {
        guard dockingRevision != nil else { return }
        dockingRevision = nil
        magneticClock?.invalidate()
        magneticClock = nil
        isDocked = true
        NotchDocking.save(true, in: .standard)
        if let screen = indicatorScreen { persistIndicatorAnchor(for: screen) }
        fusionCardStart = nil
        fusionCardTarget = nil
        // updateScreen calls present without animation. No transparent reset frame,
        // secondary entrance spring, layer transform, or independent window fade.
        updateScreen()
        presentation.backdrop.magneticJoin = nil
        updatePointer()
    }

    private func distance(_ point: CGPoint, to rect: CGRect) -> CGFloat {
        let dx = max(max(rect.minX - point.x, 0), point.x - rect.maxX)
        let dy = max(max(rect.minY - point.y, 0), point.y - rect.maxY)
        return hypot(dx, dy)
    }

    private func displayID(_ screen: NSScreen) -> UInt32? {
        (screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint32Value
    }

    private func persistIndicatorAnchor(for screen: NSScreen) {
        let defaults = UserDefaults.standard
        defaults.set(indicatorEdge.rawValue, forKey: "notch.indicatorEdge")
        defaults.set(Double(indicatorFraction), forKey: "notch.indicatorFraction")
        if let id = displayID(screen) { defaults.set(Int(id), forKey: "notch.indicatorDisplayID") }
    }

    private func indicatorFrame(on screen: NSScreen) -> CGRect {
        let bounds = screen.frame
        let size = CGSize(width: 58, height: 30)
        switch indicatorEdge {
        case .top:
            let requestedX = bounds.minX + indicatorFraction * (bounds.width - size.width)
            return CGRect(x: safeTopIndicatorX(requestedX, size: size, screen: screen),
                          y: bounds.maxY - size.height, width: size.width, height: size.height)
        case .bottom:
            return CGRect(x: bounds.minX + indicatorFraction * (bounds.width - size.width),
                          y: bounds.minY, width: size.width, height: size.height)
        case .left:
            return CGRect(x: bounds.minX, y: bounds.minY + indicatorFraction * (bounds.height - size.height),
                          width: size.width, height: size.height)
        case .right:
            return CGRect(x: bounds.maxX - size.width,
                          y: bounds.minY + indicatorFraction * (bounds.height - size.height),
                          width: size.width, height: size.height)
        }
    }

    private func safeTopIndicatorX(_ proposedX: CGFloat, size: CGSize, screen: NSScreen) -> CGFloat {
        let bounds = screen.frame
        let notch = NotchGeometry(screen: bounds, topInset: screen.safeAreaInsets.top,
            leftAreaMaxX: screen.auxiliaryTopLeftArea?.maxX,
            rightAreaMinX: screen.auxiliaryTopRightArea?.minX,
            menuBarHeight: screen.frame.maxY - screen.visibleFrame.maxY).cutoutFrame
        return IndicatorPlacement.safeTopX(proposedX: proposedX, size: size.width, screen: bounds, notch: notch)
    }

    @objc func displayParametersChanged() {
        cancelMagneticInteraction()
        if isStarted { updateScreen() }
    }

    @objc private func updateScreen() {
        model.acknowledgement.dismiss()
        let defaults = UserDefaults.standard
        let savedDisplay = defaults.object(forKey: "notch.indicatorDisplayID") as? NSNumber
        let preferred = savedDisplay.flatMap { saved in NSScreen.screens.first { displayID($0) == saved.uint32Value } }
        guard let screen = preferred ?? NSScreen.screens.first(where: { $0.safeAreaInsets.top > 0 }) ?? NSScreen.main ?? NSScreen.screens.first else { return }
        indicatorScreen = screen
        indicatorEdge = FERBScreenEdge(rawValue: defaults.string(forKey: "notch.indicatorEdge") ?? "right") ?? .right
        indicatorFraction = defaults.object(forKey: "notch.indicatorFraction") == nil
            ? 0.5 : min(1, max(0, CGFloat(defaults.double(forKey: "notch.indicatorFraction"))))
        let current = NotchGeometry(screen: screen.frame, topInset: screen.safeAreaInsets.top,
                                    leftAreaMaxX: screen.auxiliaryTopLeftArea?.maxX,
                                    rightAreaMinX: screen.auxiliaryTopRightArea?.minX,
                                    menuBarHeight: screen.frame.maxY - screen.visibleFrame.maxY)
        geometry = current
        isDocked = NotchDocking.isSaved(in: .standard) && current.hasNotch
        model.isNotchDocked = isDocked
        presentation.backdrop.docked = isDocked
        indicatorControl?.isDocked = isDocked
        if isDocked { indicatorEdge = .top }
        model.indicatorEdge = indicatorEdge.rawValue
        screenFrame = screen.frame
        // The strip whose windows are always-present chrome (menu bar, status items), never something to yield to.
        menuBarHeight = max(screen.frame.maxY - screen.visibleFrame.maxY, screen.safeAreaInsets.top,
                            NSStatusBar.system.thickness)
        model.handleWidth = current.handleWidth
        model.hasHardwareNotch = current.hasNotch
        // The camera housing, or a virtual notch over the menu bar's centre: both begin at the screen's top edge.
        model.cutoutHeight = max(40, current.cutoutFrame.height)
        cutoutFrame = current.cutoutFrame
        indicatorFrame = isDocked ? NotchDocking.dragRegion(cutout: cutoutFrame) : indicatorFrame(on: screen)
        indicatorPanel.setFrame(indicatorFrame, display: true)
        if indicatorControl?.isAttached != true { indicatorPanel.orderFrontRegardless() }
        tracerPanel.setFrame(frame(for: CGSize(width: model.handleWidth + 40,
                                               height: model.cutoutHeight + 24)), display: true)
        pointerIsInside = false
        present(animate: false, requestFocus: false)
    }

    @objc private func screenDidWake() {
        screensAsleep = false
        updateScreen()
        windowYield.showPanel()
        if !model.expanded { indicatorPanel.orderFrontRegardless() }
        panel.resetCursorRects()
        updateTracerVisibility()
    }

    @objc func screensDidSleep() {
        cancelMagneticInteraction()
        model.acknowledgement.dismiss()
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
        model.acknowledgement.dismiss()
        // Space changes are not display changes. Keep the same geometry, draft,
        // pin state, running spring and current text responder. In particular,
        // do not resignKey here: that discarded input focus on every swipe.
        hoverTask?.cancel()
        if overlaySpace.isAttached {
            // macOS can drop unmanaged overlay windows during a desktop swipe without detaching our space handle.
            // Re-add the persistent indicator and light, and the chat unless it is intentionally yielding.
            overlaySpace.rejoin(tracerPanel)
            overlaySpace.rejoin(indicatorPanel)
            overlaySpace.rejoin(magneticFieldPanel)
            if let activityPanel { overlaySpace.rejoin(activityPanel) }
            if !model.isYielded { overlaySpace.rejoin(panel) }
            if indicatorPanel.isVisible { indicatorPanel.orderFrontRegardless() }
            if tracerPanel.isVisible { tracerPanel.orderFrontRegardless() }
            if magneticFieldPanel.isVisible { magneticFieldPanel.orderFrontRegardless() }
            if !model.isYielded && panel.isVisible { panel.orderFrontRegardless() }
        } else {
            windowYield.showPanel()
            updateTracerVisibility()
        }
        pointerIsInside = pointerIsOverSurface
    }

    private func present(animate: Bool, requestFocus: Bool = true) {
        // Explicit close must win even while a pointer/fusion owns geometry.
        if requestFocus && !model.surfaceVisible && hasMagneticInteraction {
            cancelMagneticInteraction()
        }
        guard geometry != nil, indicatorDragPointer == nil, dockingRevision == nil else { return }
        presentation.compactSize = isDocked ? cutoutFrame.size : CGSize(width: 58, height: 30)
        model.noteTextPresentation(expanded: model.surfaceVisible)
        let wasExpanded = presentedExpanded
        presentedExpanded = model.surfaceVisible
        let isOpening = model.bodyVisible && !presentedBodyVisible
        presentedBodyVisible = model.bodyVisible
        // The owner opened or revealed FERB (background updates don't request focus): stay over the windows
        // already there rather than stepping aside right after taking focus.
        if isOpening && requestFocus { yieldController.ownerOpened() }
        guard model.surfaceVisible else {
            presentedBodyVisible = false
            updateTracerVisibility()
            updateYieldWatch()
            setIndicatorResting(false)
            indicatorRestTask?.cancel()
            indicatorRestTask = nil
            let reduceMotion = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
            if animate && panel.isVisible && !reduceMotion, let screen = indicatorScreen {
                let anchor = CGPoint(x: indicatorFrame.midX - panel.frame.minX,
                                     y: isDocked ? cutoutFrame.height / 2 : panel.frame.maxY - indicatorFrame.midY)
                let revision = UUID()
                presentationRevision = revision
                presentation.animates = true
                presentation.present(size: isDocked ? cutoutFrame.size : CGSize(width: 58, height: 30), expanded: false,
                                     bodyVisible: !model.isHeaderOnly, anchor: anchor, animate: true) { [weak self] in
                    guard let self, self.presentationRevision == revision else { return }
                    self.detachIndicator()
                    self.panel.setFrame(self.isDocked ? NotchDocking.dragRegion(cutout: self.cutoutFrame) : self.indicatorFrame(on: screen), display: true)
                    self.panel.orderOut(nil)
                    self.model.finishCompactPreviewDismissal()
                    self.indicatorPanel.orderFrontRegardless()
                    self.scheduleIndicatorRest()
                }
                return
            }
            detachIndicator()
            panel.orderOut(nil)
            model.finishCompactPreviewDismissal()
            scheduleIndicatorRest()
            if indicatorControl?.isAttached != true { indicatorPanel.orderFrontRegardless() }
            return
        }
        indicatorRestTask?.cancel()
        indicatorRestTask = nil
        setIndicatorResting(false)
        if let screen = indicatorScreen {
            model.availableSurfaceSize = PanelLayout.surfaceBudget(screen: screen.frame, edge: indicatorEdge, docked: isDocked)
        }
        let targetFrame = frame(for: model.panelSize)
        let targetSize = model.surfaceSize
        let shouldAnimate = animate && (indicatorPanel.isVisible || panel.isVisible)
        let reduceMotion = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        let revision = UUID()
        presentationRevision = revision
        presentation.animates = shouldAnimate

        presentation.cutoutHeight = model.cutoutHeight
        panel.setFrame(targetFrame, display: true)
        attachIndicator()
        presentation.destination = CGPoint(x: indicatorEdge == .left ? 56 : 8,
                                            y: isDocked ? 0 : (indicatorEdge == .top ? 28 : (indicatorEdge == .bottom ? 12 : 0)))
        let anchor = CGPoint(x: indicatorFrame.midX - panel.frame.minX,
                             y: isDocked ? cutoutFrame.height / 2 : panel.frame.maxY - indicatorFrame.midY)
        presentation.present(size: targetSize, expanded: model.surfaceVisible,
                             bodyVisible: !model.isHeaderOnly,
                             anchor: wasExpanded ? nil : anchor,
                             animate: shouldAnimate && !reduceMotion) { [weak self] in
            guard let self, self.presentationRevision == revision else { return }
            if self.indicatorDragPointer == nil { self.panel.setFrame(targetFrame, display: true) }
            self.model.completeTextReveal()
            self.hostingView?.scheduleTextDisplayRefresh()
        }
        windowYield.showPanel()
        panel.resetCursorRects()
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
        if model.surfaceVisible, let screen = indicatorScreen {
            return PanelLayout.frame(size: size, screen: screen.frame, indicator: indicatorFrame,
                                     edge: indicatorEdge, docked: isDocked, cutout: cutoutFrame)
        }
        return CGRect(x: cutoutFrame.midX - size.width / 2, y: cutoutFrame.maxY - size.height,
                      width: size.width, height: size.height)
    }

    /// After three idle seconds the pill settles into a home-indicator line. Both
    /// states share the same center, so hovering unfolds it in place.
    private func setIndicatorResting(_ resting: Bool, animated: Bool = true) {
        let animated = animated && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        guard !isDocked, indicatorControl?.isAttached != true, let screen = indicatorScreen else { return }
        let pill = indicatorFrame(on: screen)
        let length = cutoutFrame.width / 2
        let target: CGRect
        if resting {
            switch indicatorEdge {
            case .top:
                target = CGRect(x: safeTopIndicatorX(pill.midX - length / 2, size: CGSize(width: length, height: 5), screen: screen), y: screen.frame.maxY - 5, width: length, height: 5)
            case .bottom:
                target = CGRect(x: min(screen.frame.maxX - length, max(screen.frame.minX, pill.midX - length / 2)), y: screen.frame.minY, width: length, height: 5)
            case .left:
                target = CGRect(x: screen.frame.minX, y: min(screen.frame.maxY - length, max(screen.frame.minY, pill.midY - length / 2)), width: 5, height: length)
            case .right:
                target = CGRect(x: screen.frame.maxX - 5, y: min(screen.frame.maxY - length, max(screen.frame.minY, pill.midY - length / 2)), width: 5, height: length)
            }
        } else {
            target = pill
        }
        indicatorControl?.setResting(resting, animated: animated)
        if indicatorPanel.frame != target {
            NSAnimationContext.runAnimationGroup { context in
                context.duration = animated ? 0.38 : 0
                context.timingFunction = CAMediaTimingFunction(controlPoints: 0.2, 0.8, 0.2, 1)
                indicatorPanel.animator().setFrame(target, display: true)
            }
        }
        if indicatorControl?.isAttached != true { indicatorPanel.orderFrontRegardless() }
    }

    private func scheduleIndicatorRest() {
        guard !isDocked, indicatorRestTask == nil, !model.surfaceVisible,
              !model.taskItems.contains(where: \.isRunning) else { return }
        indicatorRestTask = Task { [weak self] in
            do { try await Task.sleep(for: .seconds(3)) } catch { return }
            guard let self else { return }
            self.indicatorRestTask = nil
            guard !self.model.expanded, !self.pointerIsInside,
                  !self.model.taskItems.contains(where: \.isRunning) else { return }
            self.setIndicatorResting(true)
        }
    }

    private func updateTracerVisibility() {
        model.clickThrough.set("tracer", windows: [tracerPanel.windowNumber])
        model.clickThrough.set("magneticField", windows: [magneticFieldPanel.windowNumber])
        tracerPanel.orderOut(nil)
    }

    /// The notch windows sit in an unmanaged overlay space above every ordinary window and other apps' menus, so
    /// no window level lets another surface win there. While FERB yields, the panel steps one layer back and stays
    /// on screen: just behind the app window it yields to, or under the menu (see NotchWindowYield; mouse-event
    /// transparency is never touched); content, draft, attachments and pin are untouched. Coming back re-keys the
    /// panel only if its body shows, it was key when the yield began, and the owner hasn't clicked or switched to
    /// another app meanwhile.
    private func applyYield(_ yielded: Bool) {
        if yielded {
            model.acknowledgement.dismiss()
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
        if model.surfaceVisible { occupied.append(frame(for: model.surfaceSize)) }
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
        if model.compactPlanText != nil { model.expand() }
        // The owner reached for FERB (shortcut, menu or a control): stop yielding to an app under the pinned chat
        // and show FERB over the windows now over it.
        yieldController.summon()
        hoverTask?.cancel()
        indicatorRestTask?.cancel()
        indicatorRestTask = nil
        setIndicatorResting(false)
        if model.bodyVisible, !model.isYielded { panel.makeKey() }
    }

    private var previewWindows: [UUID: NSWindow] = [:]

    private var pointerIsOverSurface: Bool {
        let point = NSEvent.mouseLocation
        if model.expanded, previewWindows.values.contains(where: { $0.isVisible && $0.frame.contains(point) }) {
            return true
        }
        if isDocked, NotchDocking.accepts(point, cutout: cutoutFrame, hasHardwareNotch: model.hasHardwareNotch) {
            return true
        }
        let indicatorHitFrame = isDocked ? cutoutFrame.union(indicatorFrame) : indicatorFrame.insetBy(dx: 0, dy: -2)
        guard model.surfaceVisible else { return indicatorHitFrame.contains(point) || indicatorPanel.frame.contains(point) }
        let panelFrame = panel.frame.insetBy(dx: 0, dy: -2)
        if panelFrame.contains(point) { return true }
        // Keep the hover trigger hot as the compact indicator is handed off to the
        // expanding surface. This also prevents an edge coordinate conversion from
        // folding FERB while the pointer is still at its origin.
        return indicatorHitFrame.contains(point)
    }

    private func updatePointer(revealPinnedHeader: Bool = false) {
        guard indicatorDragPointer == nil, dockingRevision == nil else { return }
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
        indicatorRestTask?.cancel()
        indicatorRestTask = nil
        setIndicatorResting(false)
        if model.isPinned {
            model.revealPinnedBody()
            return
        }
        guard model.hoverOpensChat,
              !model.expanded else { return }
        hoverTask = Task { [weak self] in
            do { try await Task.sleep(for: .milliseconds(100)) } catch { return }
            guard let self else { return }
            self.model.expand()
        }
    }

    private func pointerExited() {
        guard indicatorDragPointer == nil, dockingRevision == nil else { return }
        hoverTask?.cancel()
        if model.isPinned {
            model.foldPinnedBody()
            return
        }
        guard !model.blocksAutomaticDismissal, model.expanded else {
            scheduleIndicatorRest()
            return
        }
        // Allow the pointer to cross the popover arrow/gap without folding either
        // surface. Once outside both windows, a preview must close with the chat.
        let delay = previewWindows.isEmpty ? 60 : 250
        hoverTask = Task { [weak self] in
            do { try await Task.sleep(for: .milliseconds(delay)) } catch { return }
            guard let self, !self.pointerIsOverSurface, !self.model.blocksAutomaticDismissal, self.model.expanded else { return }
            self.model.dismissAutomatically()
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
    private var notchShowing: Bool { (model.bodyVisible || model.compactPlanText != nil) && !model.isYielded && !screensAsleep }

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
            self.model.discardFinishedPreviews()
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
                if paused {
                    self.model.feedback.play(.paused)
                    self.introControls.saw(.pause)
                }
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
    /// packaged app sets it up again in the setup window (never the intro's tour) and a development build says how.
    private func restartDaemon() {
        Task { [weak self] in
            guard let self else { return }
            if await self.service.restart() {
                self.model.daemonLink.restarting()
            } else if let repair = self.onRepair {
                repair()
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

    /// The modifier double-tap: opens or closes the notch; opening it plays summon.
    func modifierDoubleTapped() {
        if summonFromYield() { model.feedback.play(.summon) }
        else {
            model.toggle()
            if model.expanded { model.feedback.play(.summon) }
        }
        introControls.saw(.toggle)
    }

    /// The Control-Option-Space hot key: opening FERB plays summon; closing it plays nothing.
    func shortcutPressed() {
        defer { introControls.saw(.summon) }
        if summonFromYield() { model.feedback.play(.summon); return }
        if model.expanded { model.collapse(keyboard: true); return }
        model.feedback.play(.summon)
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
        isStarted = false
        cancelMagneticInteraction()
        model.acknowledgement.dismiss()
        closeTaskActivity()
        poster?.stop()
        statusRefresh = nil
        model.disconnectDaemon()
        shortcuts.stop()
        model.cancelPinnedFold()
        presentation.stop()
        panel.orderOut(nil)
        tracerPanel.orderOut(nil)
        indicatorPanel.orderOut(nil)
        overlaySpace.stop()
        presentationRevision = UUID()
        hoverTask?.cancel()
        indicatorRestTask?.cancel()
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
